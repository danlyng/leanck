"""Verify staged release binaries using disposable projects and runtime files.

Run through `just verify-distribution [TEMPLATE]`, which defaults to the in-repo
`test-support/lean-project`, or pass --bin-dir and --project directly.
--benchmark-only compares CLI query timings without running the MCP checks.
"""

import argparse
import asyncio
import ctypes
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import statistics
import struct
import subprocess
import tempfile
import time


class MCP:
    def __init__(self, process):
        self.process = process
        self.pending = {}
        self.next_id = 0
        self.reader = asyncio.create_task(self.read())

    @classmethod
    async def start(cls, binary, project, env):
        process = await asyncio.create_subprocess_exec(
            str(binary), "serve", "mcp", "--workspace", str(project), env=env,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )
        client = cls(process)
        try:
            await client.call("initialize", {
                "protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": "distribution-check", "version": "1"},
            })
            await client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            return client
        except BaseException:
            await client.close()
            raise

    async def read(self):
        try:
            while line := await self.process.stdout.readline():
                value = json.loads(line)
                future = self.pending.pop(value.get("id"), None)
                if future is not None and not future.done():
                    future.set_result(value)
        finally:
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(RuntimeError("MCP stdout closed"))

    async def send(self, value):
        self.process.stdin.write(json.dumps(value).encode() + b"\n")
        await self.process.stdin.drain()

    async def call(self, method, params, allow_error=False):
        self.next_id += 1
        request_id = self.next_id
        future = asyncio.get_running_loop().create_future()
        self.pending[request_id] = future
        try:
            await self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
            response = await asyncio.wait_for(future, 120)
            if allow_error:
                return response
            assert "error" not in response, response
            return response["result"]
        finally:
            self.pending.pop(request_id, None)

    async def tool(self, name, arguments):
        result = await self.call("tools/call", {"name": name, "arguments": arguments})
        assert not result.get("isError", False), result
        return json.loads(result["content"][0]["text"])

    async def close(self):
        self.process.stdin.close()
        try:
            await asyncio.wait_for(self.process.wait(), 5)
        except asyncio.TimeoutError:
            self.process.kill()
            await self.process.wait()
            raise AssertionError("MCP did not exit after stdin closed")
        finally:
            await self.reader
        assert self.process.returncode == 0, self.process.returncode


async def cli(binary, project, env, *arguments, success=True):
    process = await asyncio.create_subprocess_exec(
        str(binary), *arguments, "--workspace", str(project), "--json", env=env,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), 120)
    except BaseException:
        process.kill()
        await process.wait()
        raise
    assert (process.returncode == 0) == success, stderr.decode(errors="replace")
    return json.loads(stdout)


async def control(path, operation):
    """Test-only control fixture; frontends use leanck-client."""
    reader, writer = await asyncio.open_unix_connection(str(path))
    try:
        body = json.dumps({"id": "distribution", "op": operation, "params": {}}).encode()
        writer.write(struct.pack(">I", len(body)) + body)
        await writer.drain()
        size = struct.unpack(">I", await reader.readexactly(4))[0]
        assert size <= 16 * 1024 * 1024
        result = json.loads(await reader.readexactly(size))
        assert result["ok"], result
        return result["result"]
    finally:
        writer.close()
        await writer.wait_closed()


async def shutdown(path):
    if path.exists():
        await asyncio.wait_for(control(path, "shutdown"), 10)
    deadline = time.monotonic() + 10
    while path.exists() or path.with_suffix(".pid").exists():
        assert time.monotonic() < deadline, "daemon did not remove socket/PID"
        await asyncio.sleep(0.05)
    lock = path.with_suffix(".lock")
    if lock.exists():
        with lock.open() as file:
            fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)


def stage_copy(source, destination, names):
    destination.mkdir()
    for name in names:
        shutil.copy2(source / name, destination / name)
        if os.uname().sysname == "Darwin":
            subprocess.run(["codesign", "--force", "-s", "-", str(destination / name)], check=True, capture_output=True)


def check_daemon_binary(socket_path, expected):
    pid = socket_path.with_suffix(".pid").read_text().strip()
    command = subprocess.check_output(["ps", "-p", pid, "-o", "args="], text=True).strip()
    assert command.startswith("leanckd serve daemon "), command
    if os.uname().sysname == "Darwin":
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        libproc.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
        libproc.proc_pidpath.restype = ctypes.c_int
        buffer = ctypes.create_string_buffer(4096)
        assert libproc.proc_pidpath(int(pid), buffer, len(buffer)) > 0, ctypes.get_errno()
        executable = Path(os.fsdecode(buffer.value))
    else:
        executable = Path(f"/proc/{pid}/exe").resolve(strict=True)
    assert executable.resolve() == expected.resolve(), (expected, executable)
    return pid


def clean_report(value, project):
    assert value["total"] == 0 and value["diagnostics"] == [], value
    assert value["uri"] == (project / "TestProject/Distribution.lean").as_uri(), value


async def verify(args, scratch):
    env = dict(os.environ)
    env.pop("LEANCK_CONFIG", None)
    env["XDG_CACHE_HOME"] = str(scratch / "cache")
    env["XDG_CONFIG_HOME"] = str(scratch / "config")
    env["LEANCKD_LOG"] = "error"
    owned = []
    clients = []
    timings = {}
    try:
        for mode in (["self"] if args.benchmark_only else ["self", "renamed", "configured"]):
            project = scratch / mode
            shutil.copytree(args.project, project, ignore=shutil.ignore_patterns(".git", ".lake", "target", "build"))
            project = project.resolve()
            selected = dict(env)
            frontends = args.bin_dir
            if mode != "self":
                frontends = scratch / (mode + "-bin")
                stage_copy(args.bin_dir, frontends, ["leanck"])
            binary = frontends / "leanck"
            daemon = binary
            if mode == "renamed":
                binary = frontends / "renamed-leanck"
                (frontends / "leanck").rename(binary)
                daemon = binary
                if os.uname().sysname == "Darwin":
                    subprocess.run(["codesign", "--force", "-s", "-", str(binary)], check=True, capture_output=True)
            elif mode == "configured":
                daemon_dir = scratch / "configured-daemon"
                stage_copy(args.bin_dir, daemon_dir, ["leanck"])
                daemon = daemon_dir / "leanck"
            overlay = scratch / (mode + ".toml")
            settings = "[cache]\nenabled = false\n"
            if mode == "configured":
                settings += "[client]\ndaemon_bin = " + json.dumps(str(daemon)) + "\n"
            overlay.write_text(settings)
            selected["LEANCK_CONFIG"] = str(overlay)
            probe = await cli(binary, project, selected, "doctor")
            path = Path(probe["socket_path"])
            owned.append(path)
            assert not path.exists() and not path.with_suffix(".pid").exists(), "doctor spawned a daemon"
            if not args.benchmark_only:
                error = await cli(binary, project, selected, "build", "--timeout", "0", success=False)
                assert error["success"] is False and error["error"]["kind"] == "timeout", error
                assert not path.exists(), "standalone build started a daemon"
            start = time.perf_counter()
            clean_report(await cli(binary, project, selected, "diagnostics", "TestProject/Distribution.lean"), project)
            cold = time.perf_counter() - start
            pid = check_daemon_binary(path, daemon)
            samples = []
            for _ in range(10):
                start = time.perf_counter()
                clean_report(await cli(binary, project, selected, "diagnostics", "TestProject/Distribution.lean"), project)
                samples.append(time.perf_counter() - start)
            start = time.perf_counter()
            reports = await asyncio.gather(*[cli(binary, project, selected, "diagnostics", "TestProject/Distribution.lean") for _ in range(24)])
            burst = time.perf_counter() - start
            for report in reports:
                clean_report(report, project)
            assert check_daemon_binary(path, daemon) == pid
            timings[mode] = {"cold_ms": round(cold * 1000, 2), "warm_median_ms": round(statistics.median(samples) * 1000, 2), "cli_24_ms": round(burst * 1000, 2)}
            if not args.benchmark_only:
                mcp = await MCP.start(binary, project, selected)
                clients.append(mcp)
                health = await mcp.tool("health", {"json": True})
                assert health["ok"] and health["lsp"]["workspace"] == str(project), health
                error = await mcp.call("tools/call", {
                    "name": "diagnostics", "arguments": {"file": "missing.lean"},
                }, allow_error=True)
                assert error["error"]["code"] == -32602 and error["error"]["data"]["kind"] == "not_found", error
                reports = await asyncio.gather(*[
                    cli(binary, project, selected, "diagnostics", "TestProject/Distribution.lean") if index % 2 == 0 else
                    mcp.tool("diagnostics", {"file": "TestProject/Distribution.lean", "verbose": True}) for index in range(24)
                ])
                for report in reports:
                    clean_report(report, project)
                assert check_daemon_binary(path, daemon) == pid, "CLI/MCP spawned distinct daemons"
                other = scratch / (mode + "-other")
                shutil.copytree(project, other, ignore=shutil.ignore_patterns(".lake"))
                other = other.resolve()
                switched = await mcp.tool("set_workspace", {"path": str(other)})
                other_socket = Path(switched["socket_path"])
                owned.append(other_socket)
                assert switched["ok"] and not other_socket.exists(), "set_workspace started Lean"
                other_health, other_report = await asyncio.gather(
                    mcp.tool("health", {"json": True}),
                    cli(binary, other, selected, "diagnostics", "TestProject/Distribution.lean"),
                )
                assert other_health["ok"] and other_health["lsp"]["workspace"] == str(other), other_health
                clean_report(other_report, other)
                other_pid = check_daemon_binary(other_socket, daemon)
                assert other_pid != pid
                clean_report(await mcp.tool("diagnostics", {"file": "TestProject/Distribution.lean", "verbose": True}), other)
                await mcp.tool("set_workspace", {"path": str(project)})
                assert (await mcp.tool("health", {"json": True}))["lsp"]["workspace"] == str(project)
                assert check_daemon_binary(path, daemon) == pid, "switching discarded the first daemon"
                await mcp.close()
                clients.remove(mcp)
                await shutdown(other_socket)
            await shutdown(path)
            print(json.dumps({"discovery": mode, "passed": True, **timings[mode]}), flush=True)
        if args.output:
            args.output.write_text(json.dumps(timings, indent=2) + "\n")
    finally:
        failures = []
        for client in clients:
            try:
                await client.close()
            except Exception as error:
                failures.append(str(error))
        for path in owned:
            try:
                await shutdown(path)
            except Exception as error:
                failures.append(str(error))
                # Panic fallback touches only this unique scratch runtime.
                try:
                    pid = int(path.with_suffix(".pid").read_text())
                    if pid > 1:
                        os.killpg(pid, signal.SIGTERM)
                except (FileNotFoundError, ProcessLookupError):
                    pass
        assert not failures, failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--benchmark-only", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    args.bin_dir = args.bin_dir.resolve(strict=True)
    args.project = args.project.resolve(strict=True)
    for name in ["leanck"]:
        assert (args.bin_dir / name).is_file(), name
        if os.uname().sysname == "Darwin":
            subprocess.run(["codesign", "--verify", "--strict", str(args.bin_dir / name)], check=True, capture_output=True)
    with tempfile.TemporaryDirectory(prefix="lck-dist-", dir="/tmp") as temporary:
        asyncio.run(verify(args, Path(temporary)))


if __name__ == "__main__":
    main()
