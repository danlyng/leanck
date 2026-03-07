"""Offline end-to-end tests: real archives, temporary homes, mocked network/hosts.

Run with Python 3.11+: python3 -m unittest discover -s test-support -p test_installer.py -v
"""

import hashlib
import io
import json
import os
import time
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

from installer_terminal import (InstallerTerminal, DOWN, UP, LEFT, RIGHT, ENTER, ESCAPE,
                                PLUGIN_PROMPT, STANDALONE_PROMPT)


REPO = Path(__file__).resolve().parent.parent
TARGET = "x86_64-unknown-linux-musl"
ARCHIVE = f"leanck-{TARGET}.tar.gz"


def executable(path, content):
    path.write_text(content)
    path.chmod(0o755)


class InstallerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.shared = tempfile.TemporaryDirectory(prefix="leanck-installer-tests-")
        cls.fixture = Path(cls.shared.name)
        binaries = cls.fixture / "bin"
        binaries.mkdir()
        executable(binaries / "leanck", '''#!/bin/sh
if [ -n "${LEANCK_TEST_BINARY_EXECUTIONS:-}" ]; then
    printf 'leanck %s %s\\n' "${1:-}" "${2:-}" >> "$LEANCK_TEST_BINARY_EXECUTIONS"
fi
if [ "${1:-}" = _completions ]; then
    [ -z "${LEANCK_TEST_COMPLETION_FAILURE:-}" ] || exit 1
    case "$2" in
        fish) printf 'complete -c leanck -f\\n' ;;
        zsh) printf '#compdef leanck\\n' ;;
        bash|powershell|elvish) printf '# completions for %s\\n' "$2" ;;
        *) exit 2 ;;
    esac
else
    printf 'leanck 1.2.3\\n'
fi
''')
        executable(binaries / "codesign", "#!/bin/sh\nexit 0\n")
        env = os.environ | {"PATH": f"{binaries}:{os.environ['PATH']}"}
        subprocess.run([
            sys.executable, str(REPO / "packaging/package_release.py"),
            "--bin-dir", str(binaries), "--output", str(cls.fixture / "assets"),
            "--target", TARGET, "--version", "v1.2.3",
        ], env=env, check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.shared.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="leanck-installer-case-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.commands = self.root / "commands"
        self.commands.mkdir()
        self.assets = self.root / "assets"
        shutil.copytree(self.fixture / "assets", self.assets)
        self.log = self.root / "calls.jsonl"
        self.host_executions = self.root / "host-executions.jsonl"
        self.binary_executions = self.root / "binary-executions"
        self.downloads = self.root / "downloads"
        self.downloads.mkdir()
        self.env = os.environ | {
            "HOME": str(self.home),
            "XDG_DATA_HOME": str(self.home / "data"),
            "XDG_STATE_HOME": str(self.home / "state"),
            "XDG_CONFIG_HOME": str(self.home / "config"),
            "SHELL": "/bin/zsh",
            "CLAUDE_CONFIG_DIR": str(self.home / "claude"),
            "CODEX_HOME": str(self.home / "codex"),
            "PATH": f"{self.commands}:{os.environ['PATH']}",
            "LEANCK_TEST_ASSETS": str(self.assets),
            "LEANCK_TEST_LOG": str(self.log),
            "LEANCK_TEST_SYSTEM": "Linux",
            "LEANCK_TEST_ARCH": "x86_64",
            "LEANCK_TEST_HOST_EXECUTIONS": str(self.host_executions),
            "LEANCK_TEST_BINARY_EXECUTIONS": str(self.binary_executions),
            "TMPDIR": str(self.downloads),
        }
        python = sys.executable
        executable(self.commands / "curl", f"#!{python}\n" + '''
import json, os, pathlib, shutil, sys
args = sys.argv[1:]
with open(os.environ['LEANCK_TEST_LOG'], 'a') as log:
    log.write(json.dumps(['curl', *args]) + '\\n')
if os.environ.get('LEANCK_TEST_DOWNLOAD_FAILURE'):
    sys.exit(22)
status = os.environ.get('LEANCK_TEST_HTTP_STATUS')
only_asset = os.environ.get('LEANCK_TEST_HTTP_ASSET', '')
if status and (not only_asset or args[-1].endswith(only_asset)):
    sys.stdout.write(status)
    print('simulated HTTP ' + status, file=sys.stderr)
    sys.exit(int(os.environ.get('LEANCK_TEST_CURL_EXIT', '22')))
source = pathlib.Path(os.environ['LEANCK_TEST_ASSETS']) / args[-1].rsplit('/', 1)[-1]
shutil.copyfile(source, args[args.index('--output') + 1])
sys.stdout.write('200')
''')
        executable(self.commands / "uname", '''#!/bin/sh
case "$1" in
  -s) printf '%s\\n' "$LEANCK_TEST_SYSTEM" ;;
  -m) printf '%s\\n' "$LEANCK_TEST_ARCH" ;;
esac
''')
        host = f"#!{python}\n" + '''
import json, os, pathlib, sys, time
name = pathlib.Path(sys.argv[0]).name
with open(os.environ['LEANCK_TEST_HOST_EXECUTIONS'], 'a') as log:
    log.write(json.dumps([name, *sys.argv[1:]]) + '\\n')
if os.environ.get('LEANCK_TEST_HOST_CONFIG_LOG'):
    with open(os.environ['LEANCK_TEST_HOST_CONFIG_LOG'], 'a') as log:
        log.write(json.dumps([name, os.environ.get('CODEX_HOME'), os.environ.get('CLAUDE_CONFIG_DIR')]) + '\\n')
if '--help' in sys.argv:
    sys.exit(0)
if os.environ.get('LEANCK_TEST_HOST_DELAY') == name:
    time.sleep(10)
with open(os.environ['LEANCK_TEST_LOG'], 'a') as log:
    log.write(json.dumps([name, *sys.argv[1:]]) + '\\n')
sys.exit(1 if os.environ.get('LEANCK_TEST_HOST_FAILURE') == name else 0)
'''
        for name in ("claude", "codex", "codesign"):
            executable(self.commands / name, host)

    def run_install(self, *args, success=True, approve=True, standalone=True):
        # Existing component tests explicitly exercise standalone mode.
        if standalone and "--plugin" not in args and "--all" not in args:
            args = ("--standalone", *args)
        # Feed the script over stdin exactly as curl | sh -s -- would.
        result = subprocess.run([os.environ.get("LEANCK_INSTALLER_SHELL", "sh"), "-s", "--", *(["--yes"] if approve else []), *args], input=(REPO / "install.sh").read_bytes(),
                                env=self.env, capture_output=True, timeout=30)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr.decode())
        else:
            self.assertNotEqual(result.returncode, 0, result.stderr.decode())
        return result

    def run_interactive(self, drive, *args, env=None, stdout_pipe=False, size=(32, 120)):
        """Exercise the real terminal UI while the script arrives through stdin."""
        child_env = self.env | {"TERM": "xterm-256color"}
        child_env.pop("NO_COLOR", None)
        child_env.update(env or {})
        terminal = InstallerTerminal(self, REPO / "install.sh", args, child_env, stdout_pipe, size)
        try:
            drive(terminal)
            return terminal.finish()
        finally:
            terminal.close()

    def standalone_config(self, terminal, *, binary_dir="", skills="", agents="", completions="", path=""):
        terminal.choose("Install as plugin or standalone?", "Standalone")
        terminal.prompt("Install the CLI?")
        terminal.prompt("Install location", str(binary_dir))
        terminal.prompt("Standalone skills", skills)
        terminal.prompt("Custom agents", agents)
        if skills in ("claude", "both") or agents in ("claude", "both"):
            terminal.prompt("Claude configuration directory")
        if skills in ("codex", "both"):
            terminal.prompt("Codex skills directory")
        if agents in ("codex", "both"):
            terminal.prompt("Codex configuration directory")
        terminal.prompt("Shell completions (", completions)
        if completions and completions != "none":
            terminal.prompt("Completions directory")
        terminal.prompt("Add leanck to PATH?", path)
        if path == "yes":
            terminal.prompt("Shell profile to update")

    def open_plugin_config(self, terminal):
        terminal.choose("Install as plugin or standalone?", "Plugin (recommended)")
        terminal.choose(PLUGIN_PROMPT, "Edit advanced config...")

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def snapshot_home(self):
        snapshot = {}
        for path in [self.home, *self.home.rglob("*")]:
            metadata = path.lstat()
            data = path.readlink() if path.is_symlink() else path.read_bytes() if path.is_file() else None
            snapshot[str(path.relative_to(self.home))] = (metadata.st_mode, metadata.st_mtime_ns, data)
        return snapshot

    def assert_dry_run_unchanged(self, before):
        self.assertEqual(self.snapshot_home(), before)
        self.assertFalse(self.host_executions.exists())
        self.assertFalse(self.binary_executions.exists())
        self.assertEqual(list(self.downloads.iterdir()), [])

    def rewrite_archive(self, mutate):
        path = self.assets / ARCHIVE
        with tarfile.open(path) as archive:
            members = [(m, archive.extractfile(m).read() if m.isfile() else None) for m in archive]
        with tarfile.open(path, "w:gz") as archive:
            for member, data in members:
                archive.addfile(member, io.BytesIO(data) if data is not None else None)
            mutate(archive)
        checksum = hashlib.sha256(path.read_bytes()).hexdigest()
        (self.assets / f"{ARCHIVE}.sha256").write_text(f"{checksum}  {ARCHIVE}\n")

    def source_checkout(self):
        source = self.root / "local source with spaces"
        manifest = source / "crates/leanck-cli/Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text('[package]\nname = "leanck-cli"\nversion = "9.8.7"\n')
        skill = source / "leanck-skills/new-proof/SKILL.md"
        skill.parent.mkdir(parents=True)
        skill.write_text("local skill")
        agents = source / "leanck-subagents"
        agents.mkdir()
        (agents / "new-expert.md").write_text("Claude agent")
        (agents / "new-grind.codex.toml").write_text("Codex agent")
        return source

    def test_local_repo_preview_uses_source_inventory_without_network_or_build(self):
        source = self.source_checkout()
        self.env["LEANCK_TEST_DOWNLOAD_FAILURE"] = "1"
        before = self.snapshot_home()
        result = self.run_install("--from-repo", str(source), "--dry-run", "--skills", "both", "--agents", "both", "--json", approve=False)
        data = json.loads(result.stdout)
        self.assertEqual(data["status"], "preview")
        self.assertIn("v9.8.7", data["version"])
        self.assertIn("release unverified", data["version"])
        for name in ("new-proof", "new-expert.md", "new-grind.toml"):
            self.assertTrue(any(name in action for action in data["actions"]))
        self.assertEqual(self.calls(), [])
        self.assert_dry_run_unchanged(before)

    def test_local_repo_requires_dry_run_and_a_valid_checkout(self):
        before = self.snapshot_home()
        result = self.run_install("--from-repo", str(REPO), success=False)
        self.assertIn(b"requires --dry-run", result.stderr)
        result = self.run_install("--from-repo", str(self.root), "--dry-run", success=False)
        self.assertIn(b"not a leanck source checkout", result.stderr)
        self.assertEqual(self.calls(), [])
        self.assert_dry_run_unchanged(before)

    def test_offline_terminal_plugin_preview_can_open_editor_without_changes(self):

        source = self.source_checkout()
        before = self.snapshot_home()
        def drive(t):
            self.open_plugin_config(t)
            t.action("Save changes")
            t.choose(PLUGIN_PROMPT, "Continue")
        code, output = self.run_interactive(drive, "--from-repo", str(source), "--dry-run")
        self.assertEqual(code, 0, output.decode())
        self.assertIn(b"Welcome to the leanck v9.8.7", output)
        self.assertIn(b"Dry run complete", output)
        self.assertIn(b"[bundled with plugin]", output)
        self.assertNotIn(b"Standalone skills", output)
        self.assertNotIn(b"Install the CLI?", output)
        self.assertEqual(self.calls(), [])
        self.assert_dry_run_unchanged(before)

    def test_offline_terminal_preview_can_handle_conflicts_without_changes(self):

        source = self.source_checkout()
        skill = self.home / "claude/skills/new-proof"
        skill.mkdir(parents=True)
        (skill / "custom.txt").write_text("keep this")
        before = self.snapshot_home()
        def drive(t):
            self.standalone_config(t, skills="claude")
            t.prompt("Keep it, back up and replace", "replace")
            t.choose(STANDALONE_PROMPT, "Edit advanced config...")
            t.action("Save changes")
            t.choose(STANDALONE_PROMPT, "Continue")
        code, output = self.run_interactive(drive, "--from-repo", str(source), "--dry-run", "--verbose")
        self.assertEqual(code, 0, output.decode())
        self.assertIn(b"Back up and replace conflict", output)
        self.assertEqual(self.calls(), [])
        self.assert_dry_run_unchanged(before)

    def test_online_terminal_dry_run_also_prompts_but_never_installs(self):

        before = self.snapshot_home()
        def drive(t):
            self.standalone_config(t)
            t.choose(STANDALONE_PROMPT, "Continue")
        code, output = self.run_interactive(drive, "--dry-run")
        self.assertEqual(code, 0, output.decode())
        self.assertIn(b"Dry run complete", output)
        self.assert_dry_run_unchanged(before)

    def test_noninteractive_requires_explicit_approval_even_with_force(self):
        before = self.snapshot_home()
        for args in ((), ("--force",), ("--json",)):
            result = self.run_install(*args, success=False, approve=False)
            self.assertIn(b"approval is required", result.stderr)
            if "--json" in args:
                self.assertEqual(json.loads(result.stdout)["status"], "failed")
        self.assertEqual(self.calls(), [])
        self.assertEqual(self.snapshot_home(), before)

    def test_json_preview_and_errors_are_valid_and_keep_stdout_clean(self):
        result = self.run_install("--all", "--dry-run", "--json", approve=False)
        data = json.loads(result.stdout)
        self.assertEqual(data["status"], "preview")
        self.assertEqual(data["version"], "v1.2.3")
        self.assertTrue(any("codex plugin add" in action for action in data["actions"]))
        self.assertFalse(self.host_executions.exists())
        self.assertFalse(self.binary_executions.exists())
        result = self.run_install("--json", "--wat", success=False)
        data = json.loads(result.stdout)
        self.assertEqual(data["status"], "failed")
        self.assertIn("unknown option", data["message"])
        self.assertNotIn(b"\x1b", result.stdout + result.stderr)
        result = self.run_install("--json", "--quiet")
        self.assertEqual(json.loads(result.stdout)["status"], "installed")

    def test_terminal_configuration_and_explicit_approval_before_writes(self):

        custom = self.root / "chosen bin"
        def drive(t):
            self.standalone_config(t, binary_dir=custom)
            t.wait(lambda: t.screen.selected(STANDALONE_PROMPT), "confirmation")
            self.assertFalse(custom.exists())
            self.assertFalse(self.host_executions.exists())
            self.assertFalse(self.binary_executions.exists())
            t.choose(STANDALONE_PROMPT, "Continue")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertTrue((custom / "leanck").is_file())
        self.assertIn(b"\x1b[36m", output)

    def test_terminal_default_decline_and_ctrl_c_leave_installation_unchanged(self):

        before = self.snapshot_home()
        def decline(t):
            t.choose("Install as plugin or standalone?", "Plugin (recommended)")
            t.wait(lambda: t.screen.selected(PLUGIN_PROMPT) == "Cancel", "default Cancel")
            t.send(ENTER)
        code, output = self.run_interactive(decline)
        self.assertEqual(code, 0, output.decode())
        self.assertEqual(self.snapshot_home(), before)
        self.assertFalse(self.binary_executions.exists())
        def interrupt(t):
            t.wait(lambda: not t.screen.cursor_visible, "mode menu")
            t.send(b"\x03")
        code, output = self.run_interactive(interrupt)
        self.assertNotEqual(code, 0, output.decode())
        self.assertEqual(self.snapshot_home(), before)

    def test_terminal_plugin_default_and_arrow_key_confirmation(self):

        def drive(t):
            t.wait(lambda: t.screen.selected("Install as plugin or standalone?") == "Plugin (recommended)", "default plugin")
            t.send(ENTER)
            t.wait(lambda: t.screen.selected(PLUGIN_PROMPT) == "Cancel", "confirmation")
            self.assertFalse((self.home / "data/leanck").exists())
            self.assertFalse(self.host_executions.exists())
            self.assertFalse(self.binary_executions.exists())
            t.choose(PLUGIN_PROMPT, "Continue")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertTrue((self.home / "data/leanck/bin/leanck").is_file())
        self.assertIn(b"Welcome to the leanck v1.2.3 installer", output)
        self.assertIn(b"Detected coding agents: Claude Code, Codex", output)
        self.assertIn(b"\x1b[1m\x1b[36mInstall as plugin or standalone?\x1b[0m", output)
        self.assertIn(b"\x1b[1m\x1b[32m> Plugin (recommended)\x1b[0m", output)
        self.assertIn(b"\x1b[1m\x1b[32m> Continue\x1b[0m", output)
        self.assertNotIn(b"Standalone skills", output)
        self.assertNotIn(b"Custom agents (", output)
        self.assertNotIn(b"Use \xe2\x86\x91", output)

    def test_arrow_keys_switch_modes_and_open_editor_without_approving(self):

        before = self.snapshot_home()
        def drive(t):
            title = "Install as plugin or standalone?"
            t.wait(lambda: t.screen.selected(title), title)
            t.send(DOWN)
            t.wait(lambda: t.screen.selected(title) == "Standalone", "standalone selection")
            t.send(UP)
            t.wait(lambda: t.screen.selected(title) == "Plugin (recommended)", "plugin selection")
            t.send(ENTER)
            t.choose(PLUGIN_PROMPT, "Edit advanced config...")
            t.focus("Plugin data directory")
            form = "\n".join(t.screen.section("Advanced configuration"))
            for label in ("Install skills", "Install agents", "Codex agents directory", "Shell completions", "Add leanck to PATH", "Register plugins", "Save changes", "Cancel"):
                self.assertIn(label, form)
            for label in ("Installer state directory", "Command timeout", "Back to confirmation"):
                self.assertNotIn(label, form)
            t.action("Save changes")
            t.choose(PLUGIN_PROMPT, "Cancel")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertEqual(self.snapshot_home(), before)
        self.assertFalse(self.host_executions.exists())
        self.assertFalse(self.binary_executions.exists())

    def test_terminal_color_controls_and_piped_stdout(self):

        def cancel(t):
            t.choose("Install as plugin or standalone?", "Plugin (recommended)")
            t.choose(PLUGIN_PROMPT, "Cancel")
        for args, env in ((("--no-color",), {}), ((), {"NO_COLOR": "1"})):
            with self.subTest(args=args, env=env):
                code, output = self.run_interactive(cancel, *args, env=env)
                self.assertEqual(code, 0, output.decode())
                self.assertNotRegex(output, rb"\x1b\[[0-9;]*m")
        def plain(t):
            t.prompt("Install as plugin or standalone?", "plugin")
            t.prompt(PLUGIN_PROMPT, "edit")
            t.prompt("Edit advanced config", "save")
            t.prompt(PLUGIN_PROMPT, "cancel")
        code, output = self.run_interactive(plain, env={"TERM": "dumb"})
        self.assertEqual(code, 0, output.decode())
        self.assertNotRegex(output, rb"\x1b\[[0-9;]*m")
        def piped(t):
            self.standalone_config(t, path="no")
            t.choose(STANDALONE_PROMPT, "Cancel")
        code, output = self.run_interactive(piped, stdout_pipe=True)
        self.assertEqual(code, 0, output.decode())
        self.assertIn(b"\x1b[1m\x1b[36mAdd leanck to PATH?\x1b[0m", output)
        self.assertIn(b"\x1b[0mno\r\n\x1b[0m", output)
        self.assertNotIn(b"Downloading\xe2\x80\xa6", output)

    def test_advanced_plugin_edits_apply_only_after_confirmation_and_recheck_conflicts(self):

        data = self.root / "edited plugin data"
        binary_dir = self.root / "edited executable directory"
        codex_dir = self.root / "edited codex config"
        foreign_agent = codex_dir / "agents/leanck/lean-expert.toml"
        foreign_agent.parent.mkdir(parents=True)
        foreign_agent.write_text("keep this custom agent")
        config_log = self.root / "host-config.jsonl"
        def drive(t):
            self.open_plugin_config(t)
            t.edit("Plugin data directory", data)
            t.edit("Executable directory", binary_dir)
            t.enum("Register plugins", "codex")
            t.edit("Codex configuration", codex_dir)
            t.action("Save changes")
            t.prompt("Keep it, back up and replace", "keep")
            t.wait(lambda: t.screen.selected(PLUGIN_PROMPT), "final approval")
            for destination in (data, binary_dir, self.home / "state/leanck/installer"):
                self.assertFalse(destination.exists())
            self.assertFalse(self.host_executions.exists())
            self.assertFalse(self.binary_executions.exists())
            t.choose(PLUGIN_PROMPT, "Continue")
        code, output = self.run_interactive(drive, env={"LEANCK_TEST_HOST_CONFIG_LOG": str(config_log)})
        self.assertEqual(code, 0, output.decode())
        mcp = json.loads((data / "bundle/plugin/.mcp.codex.json").read_text())
        self.assertEqual(mcp["mcpServers"]["leanck"]["command"], str(binary_dir / "leanck"))
        self.assertTrue((binary_dir / "leanck").is_file())
        self.assertEqual(foreign_agent.read_text(), "keep this custom agent")
        self.assertEqual((codex_dir / "agents/leanck/lean-grind.toml").resolve(), (data / "agents/lean-grind.toml").resolve())
        self.assertTrue((self.home / "state/leanck/installer/selections.tsv").is_file())
        self.assertFalse((self.home / "data/leanck").exists())
        self.assertFalse((self.home / "codex").exists())
        self.assertEqual(len([call for call in self.calls() if call[0] == "curl"]), 2)
        configurations = [json.loads(line) for line in config_log.read_text().splitlines()]
        self.assertTrue(all(host == "codex" and config == str(codex_dir) for host, config, _ in configurations))
        self.run_install("--codex-dir", str(codex_dir), "--dry-run", standalone=False, success=False)
        result = self.run_install("--codex-dir", str(codex_dir), "--dry-run", "--force", standalone=False)
        self.assertIn(f"Would install {binary_dir / 'leanck'}".encode(), result.stderr)

    def test_advanced_data_directory_edit_moves_default_binary_without_writing_on_cancel(self):

        data = self.root / "preview edited data"
        before = self.snapshot_home()
        def drive(t):
            self.open_plugin_config(t)
            t.edit("Plugin data directory", data)
            t.action("Save changes")
            t.choose(PLUGIN_PROMPT, "Edit advanced config...")
            t.action("Save changes")
            t.choose(PLUGIN_PROMPT, "Cancel")
        code, output = self.run_interactive(drive, "--verbose")
        self.assertEqual(code, 0, output.decode())
        self.assertIn(f"Would install {data / 'bin/leanck'}".encode(), output)
        self.assertFalse(data.exists())
        self.assertEqual(self.snapshot_home(), before)
        self.assertFalse(self.host_executions.exists())
        self.assertFalse(self.binary_executions.exists())

    def test_advanced_cancel_confirms_dirty_draft_and_discards_only_after_approval(self):
        before = self.snapshot_home()
        changed = self.root / "discarded plugin data"
        def drive(t):
            self.open_plugin_config(t)
            t.focus("Plugin data directory")
            original = t.value()
            t.edit("Plugin data directory", changed)
            t.action("Cancel")
            t.wait(lambda: t.screen.selected("Discard advanced config changes?") == "Keep editing", "safe discard default")
            t.choose("Discard advanced config changes?", "Keep editing")
            t.action("Cancel")
            t.choose("Discard advanced config changes?", "Discard changes")
            t.choose(PLUGIN_PROMPT, "Edit advanced config...")
            t.focus("Plugin data directory")
            self.assertEqual(t.value(), original)
            t.action("Cancel")
            t.choose(PLUGIN_PROMPT, "Cancel")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertEqual(self.snapshot_home(), before)
        self.assertFalse(changed.exists())
        self.assertFalse(self.host_executions.exists())
        self.assertFalse(self.binary_executions.exists())

    def test_advanced_enum_cycles_keep_selection_and_restore_clean_state(self):
        def drive(t):
            self.open_plugin_config(t)
            for value in ("none", "claude", "codex", "both"):
                t.enum("Register plugins", value)
            for value in ("bash", "zsh", "fish", "powershell", "elvish", "none"):
                t.enum("Shell completions", value)
            t.send(LEFT)
            t.wait(lambda: t.value() == "elvish", "reverse completion cycle")
            t.send(RIGHT)
            t.wait(lambda: t.value() == "none", "forward completion cycle")
            t.enum("Add leanck to PATH", "yes")
            t.enum("Shell", "fish")
            t.focus("Shell profile")
            self.assertIn("fish/config.fish", t.value())
            t.enum("Shell", "zsh")
            t.enum("Add leanck to PATH", "no")
            form = "\n".join(t.screen.section("Advanced configuration"))
            self.assertNotRegex(form, r"(?m)^.*\bShell\s*:")
            self.assertNotIn("Shell profile", form)
            t.action("Cancel")
            t.choose(PLUGIN_PROMPT, "Cancel")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertNotIn(b"Discard advanced config changes?", output)
        self.assertFalse(self.binary_executions.exists())

    def test_advanced_path_escape_and_paste_do_not_apply_unintended_input(self):
        before = self.snapshot_home()
        def drive(t):
            self.open_plugin_config(t)
            t.focus("Plugin data directory")
            original = t.value()
            t.send(b"IGNORED-NAVIGATION-TEXT")
            t.send(ENTER)
            t.wait(lambda: t.screen.cursor_visible, "path edit")
            t.send(b"\x15discard-this-path")
            t.wait(lambda: "discard-this-path" in "\n".join(t.screen.lines), "edited text")
            t.send(ESCAPE)
            t.wait(lambda: not t.screen.cursor_visible, "cancelled field edit")
            self.assertEqual(t.value(), original)
            t.send(ENTER)
            t.wait(lambda: t.screen.cursor_visible, "paste target")
            t.send(b"\x1b[200~bad\npasted\ncommands\x1b[201~")
            t.wait(lambda: "Paste a single-line value." in "\n".join(t.screen.lines), "rejected multiline paste")
            self.assertTrue(t.screen.cursor_visible)
            t.send(ESCAPE)
            t.wait(lambda: not t.screen.cursor_visible, "discarding pasted edit")
            t.action("Cancel")
            t.choose(PLUGIN_PROMPT, "Cancel")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertNotIn(b"IGNORED-NAVIGATION-TEXT", output)
        self.assertNotIn(b"Discard advanced config changes?", output)
        self.assertEqual(self.snapshot_home(), before)

    def test_advanced_ctrl_c_restores_terminal_during_navigation_and_editing(self):
        before = self.snapshot_home()
        for editing in (False, True):
            with self.subTest(editing=editing):
                def drive(t):
                    self.open_plugin_config(t)
                    t.focus("Plugin data directory")
                    if editing:
                        t.send(ENTER)
                        t.wait(lambda: t.screen.cursor_visible, "text edit")
                    t.send(b"\x03")
                code, output = self.run_interactive(drive)
                self.assertNotEqual(code, 0, output.decode())
                self.assertEqual(self.snapshot_home(), before)
                self.assertFalse(self.binary_executions.exists())

    def test_advanced_form_scrolls_and_preserves_selection_on_small_terminals(self):
        custom = self.root / ("long executable directory " * 4)
        def drive(t):
            self.open_plugin_config(t)
            t.enum("Register plugins", "codex")
            t.enum("Add leanck to PATH", "yes")
            t.enum("Shell", "bash")
            t.edit("Executable directory", custom)
            t.action("Save changes")
            t.choose(PLUGIN_PROMPT, "Cancel")
        code, output = self.run_interactive(drive, "--verbose", size=(12, 64))
        self.assertEqual(code, 0, output.decode())
        self.assertIn(f"Would install {custom / 'leanck'}".encode(), output)
        self.assertFalse(custom.exists())
        self.assertFalse(self.binary_executions.exists())

    def test_advanced_plugin_additional_locations_and_completion_destination(self):
        skills, claude, codex = [self.root / name for name in ("extra skills", "extra Claude agents", "extra Codex agents")]
        completions = self.root / "custom completions"
        def drive(t):
            self.open_plugin_config(t)
            for label, path in (("Add skill location...", skills), ("Add Claude location...", claude), ("Add Codex location...", codex)):
                t.edit(label, path)
            t.enum("Shell completions", "bash")
            t.edit("Completions directory", completions)
            t.action("Save changes")
            for path in (skills, claude, codex, completions):
                self.assertFalse(path.exists())
            t.choose(PLUGIN_PROMPT, "Continue")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertTrue((skills / "lean-proving/SKILL.md").is_file())
        self.assertTrue((claude / "lean-expert.md").is_file())
        self.assertTrue((codex / "lean-expert.toml").is_file())
        self.assertIn("completions for bash", (completions / "leanck").read_text())
        state = (self.home / "state/leanck/installer/selections.tsv").read_text()
        for key, path in (("extra_skills", skills), ("extra_claude_agents", claude), ("extra_codex_agents", codex)):
            self.assertIn(f"{key}\t{path}\n", state)

    def test_advanced_removing_new_location_restores_clean_state(self):
        def drive(t):
            self.open_plugin_config(t)
            t.edit("Add skill location...", self.root / "removed location")
            t.edit("Additional location", "")
            t.action("Cancel")
            t.choose(PLUGIN_PROMPT, "Cancel")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertNotIn(b"Discard advanced config changes?", output)
        self.assertFalse((self.root / "removed location").exists())

    def test_advanced_standalone_component_enums_install_selected_hosts(self):
        def drive(t):
            self.standalone_config(t)
            t.choose(STANDALONE_PROMPT, "Edit advanced config...")
            t.enum("Install executable", "no")
            t.enum("Install executable", "yes")
            t.enum("Install skills", "both")
            t.enum("Install agents", "codex")
            t.action("Save changes")
            t.choose(STANDALONE_PROMPT, "Continue")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        for path in (".local/bin/leanck", "claude/skills/lean-proving/SKILL.md", ".agents/skills/lean-proving/SKILL.md", "codex/agents/leanck/lean-expert.toml"):
            self.assertTrue((self.home / path).is_file(), path)
        self.assertFalse((self.home / "claude/agents").exists())
        self.assertFalse(self.host_executions.exists())

    def test_saved_choices_and_flags_override_them(self):
        custom = self.root / "saved bin"
        self.run_install("--bin-dir", str(custom), "--skills", "claude")
        result = self.run_install("--dry-run")
        self.assertIn(str(custom / "leanck").encode(), result.stderr)
        self.assertIn(b"claude/skills/lean-proving", result.stderr)
        result = self.run_install("--skills", "codex", "--dry-run")
        self.assertIn(b".agents/skills/lean-proving", result.stderr)
        self.assertNotIn(b"claude/skills/lean-proving", result.stderr)

    def test_locally_edited_managed_files_require_force_and_are_backed_up(self):
        self.run_install("--skills", "claude", "--agents", "codex")
        skill = self.home / "claude/skills/lean-proving/SKILL.md"
        skill.write_text("local edit")
        result = self.run_install(success=False)
        self.assertIn(b"local edits", result.stderr)
        self.assertEqual(skill.read_text(), "local edit")
        self.run_install("--force")
        backup_root = self.home / "state/leanck/installer/backups"
        self.assertTrue(any(p.read_text() == "local edit" for p in backup_root.rglob("SKILL.md")))
        agent = self.home / "codex/agents/leanck/lean-expert.toml"
        agent.write_text("edited agent")
        self.run_install(success=False)
        self.assertEqual(agent.read_text(), "edited agent")
        self.run_install("--force")
        self.assertTrue(any(p.is_file() and p.read_text() == "edited agent"
                            for p in backup_root.glob("*/*") if p.name != "manifest.tsv"))

    def test_completion_and_path_preview_and_idempotent_install(self):
        profile = self.home / "config/fish/config.fish"
        profile.parent.mkdir(parents=True)
        profile.write_text("# existing profile\n")
        profile.chmod(0o644)
        before = self.snapshot_home()
        self.run_install("--shell", "fish", "--add-to-path", "--dry-run")
        self.assert_dry_run_unchanged(before)
        self.run_install("--shell", "fish", "--add-to-path")
        completion = self.home / "config/fish/completions/leanck.fish"
        self.assertIn("complete -c leanck", completion.read_text())
        self.assertIn("# existing profile", profile.read_text())
        self.run_install()
        self.assertEqual(profile.read_text().count("# >>> leanck PATH >>>"), 1)
        self.assertEqual(profile.read_text().count("fish_add_path"), 1)
        self.assertEqual(profile.stat().st_mode & 0o777, 0o644)

    def test_all_supported_completion_shells_use_correct_destinations_and_arguments(self):
        destinations = {
            "bash": "data/bash-completion/completions/leanck",
            "zsh": "data/zsh/site-functions/_leanck",
            "fish": "config/fish/completions/leanck.fish",
            "powershell": "config/powershell/Completions/leanck.ps1",
            "elvish": "config/elvish/lib/leanck.elv",
        }
        for shell, relative in destinations.items():
            with self.subTest(shell=shell):
                self.run_install("--completions", shell)
                script = self.home / relative
                self.assertTrue(script.is_file())
                self.assertIn(f"leanck _completions {shell}", self.binary_executions.read_text())
                self.run_install("--completions", shell)
                if shell == "zsh":
                    self.assertTrue(script.read_text().startswith("#compdef leanck\n"))
        self.assertFalse((self.home / ".zshrc").exists())
        self.assertFalse((self.home / ".bashrc").exists())
        self.assertFalse((self.home / "config/fish/config.fish").exists())
        self.run_install("--completions", "unsupported", success=False)

    def test_custom_completion_directory_is_previewed_and_remembered(self):
        directory = self.root / "chosen completions"
        before = self.snapshot_home()
        result = self.run_install("--completions", "bash", "--completions-dir", str(directory), "--dry-run")
        self.assertIn(str(directory / "leanck").encode(), result.stderr)
        self.assertFalse(directory.exists())
        self.assert_dry_run_unchanged(before)
        self.run_install("--completions", "bash", "--completions-dir", str(directory))
        script = directory / "leanck"
        script.write_text("local completion edits")
        self.run_install(success=False)
        self.assertEqual(script.read_text(), "local completion edits")
        self.run_install("--force")
        self.assertIn("completions for bash", script.read_text())

    def test_additional_locations_are_repeatable_remembered_and_rolled_back(self):
        skills, skills2, claude, codex = [self.root / name for name in ("skills one", "skills two", "Claude extras", "Codex extras")]
        args = ("--plugin", "codex", "--extra-skills-dir", str(skills), "--extra-skills-dir", str(skills2),
                "--extra-skills-dir", str(skills), "--extra-claude-agents-dir", str(claude),
                "--extra-codex-agents-dir", str(codex))
        before = self.snapshot_home()
        self.run_install(*args, "--dry-run")
        self.assert_dry_run_unchanged(before)
        for path in (skills, skills2, claude, codex):
            self.assertFalse(path.exists())
        self.run_install(*args)
        for path in (skills / "lean-proving/SKILL.md", skills2 / "lean-proving/SKILL.md", claude / "lean-expert.md", codex / "lean-expert.toml"):
            self.assertTrue(path.is_file(), path)
        self.run_install("--plugin", "codex")
        state = (self.home / "state/leanck/installer/selections.tsv").read_text()
        self.assertEqual(state.count("extra_skills\t"), 2)
        originals = {skills / "lean-proving/SKILL.md": "first local edit", skills2 / "lean-proving/SKILL.md": "second local edit", claude / "lean-expert.md": "custom Claude agent"}
        for path, content in originals.items():
            path.write_text(content)
        self.env["LEANCK_TEST_COMPLETION_FAILURE"] = "1"
        result = self.run_install("--plugin", "codex", "--force", "--completions", "fish", success=False)
        self.assertIn(b"Restoring replaced files", result.stderr)
        for path, content in originals.items():
            self.assertEqual(path.read_text(), content)
        self.assertEqual((self.home / "state/leanck/installer/selections.tsv").read_text(), state)

    def test_additional_skills_can_be_installed_without_executable_or_host(self):
        directory = self.root / "skills only"
        self.run_install("--no-cli", "--extra-skills-dir", str(directory))
        self.assertTrue((directory / "lean-proving/SKILL.md").is_file())
        self.assertFalse((self.home / ".local/bin/leanck").exists())
        self.assertFalse(self.host_executions.exists())

    def test_codex_home_and_explicit_config_override_remembered_destination(self):
        remembered, configured, explicit = [self.root / name for name in ("remembered Codex", "environment Codex", "explicit Codex")]
        self.run_install("--plugin", "codex", "--codex-dir", str(remembered))
        self.assertTrue((remembered / "agents/leanck/lean-expert.toml").is_symlink())
        self.env["CODEX_HOME"] = str(configured)
        self.run_install("--plugin", "codex")
        self.assertTrue((configured / "agents/leanck/lean-expert.toml").is_symlink())
        self.run_install("--plugin", "codex", "--codex-dir", str(explicit))
        self.assertTrue((explicit / "agents/leanck/lean-expert.toml").is_symlink())

    def test_custom_codex_agent_directory_is_independent_of_host_config(self):
        destination = self.root / "custom Codex agents"
        self.run_install("--plugin", "codex", "--codex-agents-dir", str(destination))
        agent = destination / "lean-expert.toml"
        self.assertTrue(agent.is_symlink())
        self.assertEqual(agent.resolve(), (self.home / "data/leanck/agents/lean-expert.toml").resolve())
        self.assertFalse((self.home / "codex/agents").exists())
        self.run_install("--plugin", "codex")
        self.assertTrue(agent.is_symlink())

    def test_flat_managed_codex_agents_migrate_but_local_edits_and_unrelated_files_stay(self):
        self.run_install("--no-cli", "--agents", "codex")
        parent = self.home / "codex/agents"
        namespace = parent / "leanck"
        for path in namespace.iterdir():
            path.rename(parent / path.name)
        namespace.rmdir()
        edited = parent / "lean-expert.toml"
        edited.write_text("local legacy edit")
        unrelated = parent / "other.toml"
        unrelated.write_text("unrelated agent")
        self.run_install("--plugin", "codex")
        self.assertEqual(edited.read_text(), "local legacy edit")
        self.assertEqual(unrelated.read_text(), "unrelated agent")
        self.assertTrue((namespace / "lean-expert.toml").is_symlink())
        for name in ("lean-grind", "lean-style-worker"):
            self.assertTrue((namespace / f"{name}.toml").is_symlink())
            for suffix in ("", ".leanck-installed", ".leanck-sha256"):
                self.assertFalse((parent / f"{name}.toml{suffix}").exists())
        backups = self.home / "state/leanck/installer/backups"
        self.assertTrue(any(str(parent / "lean-grind.toml") in path.read_text() for path in backups.glob("*/manifest.tsv")))

    def test_flat_managed_agent_links_migrate_to_custom_destination_before_source_replacement(self):
        self.run_install("--plugin", "codex")
        parent = self.home / "codex/agents"
        namespace = parent / "leanck"
        for path in namespace.iterdir():
            path.rename(parent / path.name)
        namespace.rmdir()
        destination = self.root / "migrated agents"
        self.run_install("--plugin", "codex", "--codex-agents-dir", str(destination))
        self.assertTrue((destination / "lean-expert.toml").is_symlink())
        self.assertFalse((parent / "lean-expert.toml").is_symlink())
        self.assertFalse((parent / "lean-expert.toml.leanck-installed").exists())

    def test_failed_file_install_rolls_back_prior_replacements(self):
        binary = self.home / ".local/bin/leanck"
        binary.parent.mkdir(parents=True)
        binary.write_text("previous binary")
        self.env["LEANCK_TEST_COMPLETION_FAILURE"] = "1"
        result = self.run_install("--completions", "fish", success=False)
        self.assertIn(b"Restoring replaced files", result.stderr)
        self.assertEqual(binary.read_text(), "previous binary")
        self.assertFalse((self.home / "state/leanck/installer/selections.tsv").exists())

    def test_force_replaces_a_symlink_without_touching_its_target(self):
        foreign = self.root / "foreign skill"
        foreign.mkdir()
        (foreign / "keep.txt").write_text("keep")
        skill = self.home / "claude/skills/lean-proving"
        skill.parent.mkdir(parents=True)
        skill.symlink_to(foreign)
        self.run_install("--skills", "claude", "--force")
        self.assertFalse(skill.is_symlink())
        self.assertTrue((skill / "SKILL.md").is_file())
        self.assertEqual((foreign / "keep.txt").read_text(), "keep")
        self.assertEqual(list(foreign.iterdir()), [foreign / "keep.txt"])

    def test_terminal_can_preserve_a_conflicting_skill(self):

        skill = self.home / "claude/skills/lean-proving"
        skill.mkdir(parents=True)
        (skill / "custom.txt").write_text("keep")
        def drive(t):
            self.standalone_config(t, skills="claude")
            t.prompt("Keep it, back up and replace", "keep")
            t.choose(STANDALONE_PROMPT, "Continue")
        code, output = self.run_interactive(drive)
        self.assertEqual(code, 0, output.decode())
        self.assertEqual((skill / "custom.txt").read_text(), "keep")
        self.assertFalse((skill / "SKILL.md").exists())
        self.assertTrue((self.home / "claude/skills/lean-style/SKILL.md").is_file())

    def test_partial_host_failure_preserves_files_and_reports_json(self):
        self.env["LEANCK_TEST_HOST_FAILURE"] = "claude"
        result = self.run_install("--plugin", "claude", "--json", success=False)
        self.assertEqual(json.loads(result.stdout)["status"], "partial")
        self.assertIn(b"registration may be partial", result.stderr)
        self.assertTrue((self.home / "data/leanck/bin/leanck").is_file())
        self.assertTrue((self.home / "data/leanck/bundle/plugin/bin/leanck").is_file())
        del self.env["LEANCK_TEST_HOST_FAILURE"]
        result = self.run_install("--json")
        self.assertEqual(json.loads(result.stdout)["status"], "installed")

    def test_host_commands_time_out_and_report_partial_installation(self):
        self.env["LEANCK_TEST_HOST_DELAY"] = "claude"
        started = time.monotonic()
        result = self.run_install("--plugin", "claude", "--timeout", "1", "--json", success=False)
        self.assertLess(time.monotonic() - started, 8)
        self.assertIn(b"Command timed out", result.stderr)
        self.assertEqual(json.loads(result.stdout)["status"], "partial")
        self.assertTrue((self.home / "data/leanck/bin/leanck").is_file())

    def test_multiline_values_and_malformed_profile_are_rejected(self):
        before = self.snapshot_home()
        self.run_install("--bin-dir", "bad\npath", success=False)
        self.run_install("--bin-dir", "bad\tpath", success=False)
        self.assertEqual(self.snapshot_home(), before)
        self.assertEqual(self.calls(), [])
        profile = self.home / ".zshrc"
        profile.write_text("# >>> leanck PATH >>>\nkeep this text\n")
        result = self.run_install("--add-to-path", success=False)
        self.assertIn(b"unbalanced leanck PATH markers", result.stderr)
        self.assertEqual(profile.read_text(), "# >>> leanck PATH >>>\nkeep this text\n")
        self.assertFalse((self.home / ".local/bin/leanck").exists())

    def test_profile_quoting_handles_metacharacters_as_literal_paths(self):
        custom = self.root / "a'b\\c $(touch NEVER)"
        self.run_install("--bin-dir", str(custom), "--shell", "bash", "--add-to-path")
        profile = self.home / ".bashrc"
        result = subprocess.run(["bash", "--noprofile", "--norc", "-c", '. "$1"; printf "%s" "$PATH"',
                                 "profile-test", str(profile)], env=self.env, capture_output=True, check=True)
        self.assertEqual(result.stdout.decode().split(":")[0], str(custom))
        self.assertFalse((REPO / "NEVER").exists())
        if shutil.which("fish"):
            self.run_install("--bin-dir", str(custom), "--shell", "fish", "--add-to-path")
            profile = self.home / "config/fish/config.fish"
            result = subprocess.run(["fish", "--no-config", "-c",
                                     'function fish_add_path; printf "%s" "$argv[2]"; end; source "$argv[1]"',
                                     str(profile)], env=self.env, capture_output=True, check=True)
            self.assertEqual(result.stdout.decode(), str(custom))

    def test_removed_signing_flag_fails_without_network(self):
        self.run_install("--signing-identity", "A" * 40, success=False)
        self.assertEqual(self.calls(), [])

    def test_default_plugin_pins_mcp_and_links_bundled_codex_agents(self):
        self.run_install(standalone=False)
        data = self.home / "data/leanck"
        binary = data / "bin/leanck"
        self.assertTrue(os.access(binary, os.X_OK))
        mcp = json.loads((data / "bundle/plugin/.mcp.codex.json").read_text())
        self.assertEqual(mcp["mcpServers"]["leanck"]["command"], str(binary))
        self.assertEqual(mcp["mcpServers"]["leanck"]["args"], ["serve", "mcp"])
        for agent in (data / "agents").glob("*.toml"):
            link = self.home / "codex/agents/leanck" / agent.name
            self.assertTrue(link.is_symlink())
            self.assertEqual(link.resolve(), agent.resolve())
        self.assertFalse((self.home / ".local/bin").exists())
        self.assertFalse((self.home / "claude/skills").exists())
        self.assertFalse((self.home / "claude/agents").exists())
        self.assertFalse((self.home / ".agents/skills").exists())
        self.run_install(standalone=False)

    def test_plugin_default_without_detected_hosts_uses_local_share(self):
        # Restrict PATH so the developer's real hosts cannot be found or invoked.
        for host in ("claude", "codex"):
            (self.commands / host).unlink()
        self.env["PATH"] = f"{self.commands}:/usr/bin:/bin:/usr/sbin:/sbin"
        self.env.pop("XDG_DATA_HOME")
        result = self.run_install(standalone=False)
        data = self.home / ".local/share/leanck"
        self.assertTrue((data / "bin/leanck").is_file())
        self.assertTrue((data / "bundle/plugin/.claude-plugin/plugin.json").is_file())
        self.assertTrue((data / "bundle/plugin/.codex-plugin/plugin.json").is_file())
        self.assertTrue((data / "agents/lean-expert.toml").is_file())
        self.assertIn(b"Detected coding agents: none", result.stderr)
        self.assertFalse(self.host_executions.exists())

    def test_plugin_can_migrate_managed_standalone_codex_agents(self):
        self.run_install("--no-cli", "--agents", "codex")
        self.run_install(standalone=False)
        agent = self.home / "codex/agents/leanck/lean-expert.toml"
        self.assertTrue(agent.is_symlink())
        self.assertEqual(agent.resolve(), (self.home / "data/leanck/agents/lean-expert.toml").resolve())

    def test_plugin_failure_rolls_back_binary_bundle_and_agent_links(self):
        self.env["LEANCK_TEST_COMPLETION_FAILURE"] = "1"
        self.run_install("--plugin", "codex", "--completions", "fish", success=False)
        for path in ("data/leanck/bin/leanck", "data/leanck/bundle", "data/leanck/agents",
                     "codex/agents/leanck/lean-expert.toml", "state/leanck/installer/selections.tsv"):
            self.assertFalse((self.home / path).exists(), path)
            self.assertFalse((self.home / path).is_symlink(), path)

    def test_standalone_installs_only_cli_and_can_upgrade(self):
        result = self.run_install()
        binary = self.home / ".local/bin/leanck"
        self.assertTrue(os.access(binary, os.X_OK))
        self.assertFalse((self.home / "data").exists())
        self.assertFalse((self.home / "claude").exists())
        self.assertIn(b"Add ", result.stderr)
        self.assertTrue(all(call[0] == "curl" for call in self.calls()))
        self.run_install()
        self.assertEqual(list(binary.parent.iterdir()), [binary])

    def test_skills_both_without_cli_and_pinned_version(self):
        self.run_install("--no-cli", "--skills", "both", "--version", "1.2.3")
        for directory in (self.home / "claude/skills", self.home / ".agents/skills"):
            self.assertEqual(len(list(directory.iterdir())), 3)
            self.assertTrue((directory / "lean-proving/reference/search-queries.md").is_file())
        self.assertFalse((self.home / ".local/bin/leanck").exists())
        self.assertIn("/download/v1.2.3/", self.calls()[0][-1])

    def test_dry_run_all_preserves_existing_files_and_never_executes_hosts_or_binary(self):
        self.env["LEANCK_TEST_SYSTEM"] = "Darwin"
        self.env["LEANCK_TEST_ARCH"] = "arm64"
        mac_archive = "leanck-aarch64-apple-darwin.tar.gz"
        shutil.copyfile(self.assets / ARCHIVE, self.assets / mac_archive)
        shutil.copyfile(self.assets / f"{ARCHIVE}.sha256", self.assets / f"{mac_archive}.sha256")
        binary = self.home / "data/leanck/bin/leanck"
        binary.parent.mkdir(parents=True)
        binary.write_text("previous executable")
        config = self.home / "codex/config.toml"
        config.parent.mkdir()
        config.write_text("# existing host config\n")
        before = self.snapshot_home()
        result = self.run_install("--all", "--skills", "both", "--agents", "both", "--dry-run")
        self.assertIn(f"Would install {binary}".encode(), result.stderr)
        self.assertNotIn(b"Would sign ", result.stderr)
        self.assertIn(b"Would run: claude plugin install", result.stderr)
        self.assertIn(b"Would run: codex plugin add", result.stderr)
        self.assertIn(b"codex/agents/leanck/lean-expert.toml", result.stderr)
        self.assertNotIn(b"claude/skills/", result.stderr)
        self.assertNotIn(b".agents/skills/", result.stderr)
        self.assertNotIn(b"Would install " + str(self.home / "claude/agents").encode(), result.stderr)
        self.assertIn(b"Dry run complete", result.stderr)
        self.assertEqual(len(self.calls()), 2)
        self.assertTrue(all(call[0] == "curl" for call in self.calls()))
        self.assert_dry_run_unchanged(before)

    def test_dry_run_standalone_components_with_force_preserves_foreign_files(self):
        skill = self.home / "claude/skills/lean-proving"
        skill.mkdir(parents=True)
        (skill / "custom.txt").write_text("keep")
        agent = self.home / "codex/agents/leanck/lean-expert.toml"
        agent.parent.mkdir(parents=True)
        agent.write_text("custom agent")
        before = self.snapshot_home()
        result = self.run_install("--no-cli", "--skills", "both", "--agents", "both",
                                  "--force", "--dry-run", "--version", "v1.2.3")
        for destination in ("claude/skills/lean-proving", ".agents/skills/lean-proving",
                            "claude/agents/lean-expert.md", "codex/agents/leanck/lean-expert.toml"):
            self.assertIn(f"Would install {self.home / destination}".encode(), result.stderr)
        self.assertNotIn(b"/.local/bin/leanck", result.stderr)
        self.assertIn("/download/v1.2.3/", self.calls()[0][-1])
        self.assert_dry_run_unchanged(before)

    def test_dry_run_rejects_conflicts_without_mutation(self):
        skill = self.home / "claude/skills/lean-proving"
        skill.mkdir(parents=True)
        (skill / "custom.txt").write_text("keep")
        before = self.snapshot_home()
        result = self.run_install("--skills", "claude", "--dry-run", success=False)
        self.assertIn(b"already exists", result.stderr)
        self.assert_dry_run_unchanged(before)

    def test_dry_run_validates_release_version_and_checksum(self):
        before = self.snapshot_home()
        result = self.run_install("--dry-run", "--version", "9.0.0", success=False)
        self.assertIn(b"version does not match", result.stderr)
        self.assert_dry_run_unchanged(before)
        (self.assets / ARCHIVE).write_bytes(b"corrupt")
        result = self.run_install("--dry-run", success=False)
        self.assertIn(b"checksum mismatch", result.stderr)
        self.assert_dry_run_unchanged(before)

    def test_dry_run_custom_destinations_are_not_created(self):
        binary_dir = self.root / "custom bin"
        data_dir = self.root / "custom data"
        before = self.snapshot_home()
        result = self.run_install("--dry-run", "--plugin", "claude", "--bin-dir", str(binary_dir),
                                  "--data-dir", str(data_dir))
        self.assertIn(f"Would install {binary_dir / 'leanck'}".encode(), result.stderr)
        self.assertIn(f"Would install {data_dir / 'bundle'}".encode(), result.stderr)
        self.assertFalse(binary_dir.exists())
        self.assertFalse(data_dir.exists())
        self.assert_dry_run_unchanged(before)

    def test_dry_run_archive_404_previews_full_installation_without_mutation(self):
        self.env["LEANCK_TEST_HTTP_STATUS"] = "404"
        before = self.snapshot_home()
        result = self.run_install("--all", "--dry-run")
        self.assertIn(b"HTTP 404", result.stderr)
        self.assertIn(b"latest (unresolved; unverified)", result.stderr)
        self.assertIn(b"Could not verify contents, checksum, or version", result.stderr)
        self.assertIn(b"Would run: claude plugin install", result.stderr)
        self.assertIn(b"Would run: codex plugin add", result.stderr)
        self.assertIn(b"codex/agents/leanck/lean-expert.toml", result.stderr)
        self.assertIn(b"Dry run complete", result.stderr)
        self.assertEqual(len(self.calls()), 1)
        self.assert_dry_run_unchanged(before)

    def test_dry_run_checksum_404_previews_expected_skills_and_agents_for_pinned_version(self):
        self.env["LEANCK_TEST_HTTP_STATUS"] = "404"
        self.env["LEANCK_TEST_HTTP_ASSET"] = ".sha256"
        before = self.snapshot_home()
        result = self.run_install("--dry-run", "--skills", "both", "--agents", "both", "--version", "9.0.0")
        self.assertIn(b"v9.0.0 (unverified)", result.stderr)
        # Keep the fallback inventory aligned with what release packaging actually ships.
        with tarfile.open(self.assets / ARCHIVE) as archive:
            package = json.load(archive.extractfile("leanck/plugin/.leanck-package.json"))
        for skill in package["skills"]:
            for directory in (self.home / "claude/skills", self.home / ".agents/skills"):
                self.assertIn(f"Would install {directory / skill}".encode(), result.stderr)
        for agent in package["claude_agents"]:
            self.assertIn(f"Would install {self.home / 'claude/agents' / (agent + '.md')}".encode(), result.stderr)
        for agent in package["codex_agents"]:
            self.assertIn(f"Would install {self.home / 'codex/agents/leanck' / (agent + '.toml')}".encode(), result.stderr)
        self.assertEqual(len(self.calls()), 2)
        self.assert_dry_run_unchanged(before)

    def test_dry_run_handles_curl_transport_http_404_exit(self):
        self.env["LEANCK_TEST_HTTP_STATUS"] = "404"
        self.env["LEANCK_TEST_CURL_EXIT"] = "56"
        before = self.snapshot_home()
        result = self.run_install("--dry-run")
        self.assertIn(b"HTTP 404", result.stderr)
        self.assertIn(b"Dry run complete", result.stderr)
        self.assert_dry_run_unchanged(before)

    def test_real_installs_still_fail_on_archive_or_checksum_404(self):
        self.env["LEANCK_TEST_HTTP_STATUS"] = "404"
        before = self.snapshot_home()
        for asset in ("", ".sha256"):
            with self.subTest(asset=asset):
                self.env["LEANCK_TEST_HTTP_ASSET"] = asset
                result = self.run_install(success=False)
                self.assertIn(b"download failed", result.stderr)
                self.assertNotIn(b"Would install", result.stderr)
                self.assert_dry_run_unchanged(before)

    def test_dry_run_still_fails_on_other_http_and_transport_errors(self):
        before = self.snapshot_home()
        for status, exit_code in (("403", "22"), ("500", "22"), ("000", "7"), ("000", "56"), ("404", "28")):
            with self.subTest(status=status, exit_code=exit_code):
                self.env["LEANCK_TEST_HTTP_STATUS"] = status
                self.env["LEANCK_TEST_CURL_EXIT"] = exit_code
                result = self.run_install("--dry-run", success=False)
                self.assertIn(b"download failed", result.stderr)
                self.assertNotIn(b"Dry run complete", result.stderr)
                self.assert_dry_run_unchanged(before)

    def test_dry_run_404_still_checks_conflicting_files(self):
        self.env["LEANCK_TEST_HTTP_STATUS"] = "404"
        skill = self.home / "claude/skills/lean-proving"
        skill.mkdir(parents=True)
        (skill / "custom.txt").write_text("keep")
        before = self.snapshot_home()
        result = self.run_install("--dry-run", "--skills", "claude", success=False)
        self.assertIn(b"already exists", result.stderr)
        self.assert_dry_run_unchanged(before)
        self.run_install("--dry-run", "--skills", "claude", "--force")
        self.assert_dry_run_unchanged(before)

    def test_all_registers_plugins_and_installs_codex_agents_without_duplicate_skills(self):
        result = self.run_install("--all", "--skills", "both", "--agents", "claude")
        bundle = self.home / "data/leanck/bundle"
        self.assertTrue((bundle / "plugin/bin/leanck").is_file())
        self.assertTrue((bundle / ".agents/plugins/marketplace.json").is_file())
        self.assertFalse((self.home / ".agents/skills").exists())
        self.assertFalse((self.home / "claude/skills").exists())
        self.assertFalse((self.home / "claude/agents").exists())
        self.assertEqual(len(list((self.home / "codex/agents/leanck").glob("*.toml"))), 3)
        calls = [c for c in self.calls() if c[0] != "curl"]
        self.assertEqual(calls, [
            ["claude", "plugin", "marketplace", "add", str(bundle)],
            ["claude", "plugin", "install", "leanck@leanck-installer", "--scope", "user"],
            ["codex", "plugin", "marketplace", "add", str(bundle)],
            ["codex", "plugin", "add", "leanck@leanck-installer"],
        ])
        self.assertIn(b"Add ", result.stderr)  # PATH hint survives the installer's internal PATH change.
        self.run_install("--all")

    def test_conflicting_skill_aborts_before_installing_cli_and_force_replaces_it(self):
        skill = self.home / "claude/skills/lean-proving"
        skill.mkdir(parents=True)
        (skill / "custom.txt").write_text("keep")
        self.run_install("--skills", "claude", success=False)
        self.assertEqual((skill / "custom.txt").read_text(), "keep")
        self.assertFalse((self.home / ".local/bin").exists())
        self.run_install("--skills", "claude", "--force")
        self.assertFalse((skill / "custom.txt").exists())
        self.run_install("--skills", "claude")

    def test_foreign_bundle_is_protected_even_with_force(self):
        bundle = self.home / "data/leanck/bundle"
        bundle.mkdir(parents=True)
        (bundle / "custom.txt").write_text("keep")
        self.run_install("--plugin", "claude", "--force", success=False)
        self.assertEqual((bundle / "custom.txt").read_text(), "keep")

    def test_custom_agents_preserve_unrelated_files_and_require_force(self):
        agents = self.home / "codex/agents/leanck"
        agents.mkdir(parents=True)
        custom = agents / "lean-expert.toml"
        custom.write_text("custom")
        unrelated = agents / "other.toml"
        unrelated.write_text("unrelated")
        self.run_install("--agents", "codex", success=False)
        self.assertEqual(custom.read_text(), "custom")
        self.run_install("--agents", "both", "--force")
        self.assertIn("developer_instructions", custom.read_text())
        self.assertEqual(unrelated.read_text(), "unrelated")
        self.assertTrue((self.home / "claude/agents/lean-expert.md").is_file())
        self.run_install("--agents", "both")

    def test_checksum_failure_preserves_previous_install(self):
        binary = self.home / ".local/bin/leanck"
        binary.parent.mkdir(parents=True)
        binary.write_text("previous")
        (self.assets / ARCHIVE).write_bytes(b"corrupt")
        result = self.run_install(success=False)
        self.assertIn(b"checksum mismatch", result.stderr)
        self.assertEqual(binary.read_text(), "previous")

    def test_archive_traversal_and_links_are_rejected(self):
        def traversal(archive):
            member = tarfile.TarInfo("leanck/../../escaped")
            archive.addfile(member, io.BytesIO(b""))
        self.rewrite_archive(traversal)
        result = self.run_install(success=False)
        self.assertIn(b"unsafe archive member", result.stderr)
        shutil.copytree(self.fixture / "assets", self.assets, dirs_exist_ok=True)
        def symlink(archive):
            member = tarfile.TarInfo("leanck/link")
            member.type = tarfile.SYMTYPE
            member.linkname = "/tmp"
            archive.addfile(member)
        self.rewrite_archive(symlink)
        self.assertIn(b"link or special file", self.run_install(success=False).stderr)
        self.assertFalse((self.home / ".local/bin").exists())

    def test_invalid_arguments_fail_without_network(self):
        for args in (("--wat",), ("--skills",), ("--skills", "bad"), ("--no-cli",),
                     ("--version", "../../bad"), ("--release-url", "http://example.com")):
            with self.subTest(args=args):
                self.run_install(*args, success=False)
        self.assertEqual(self.calls(), [])

    def test_unsupported_platform_fails_without_network(self):
        self.env["LEANCK_TEST_SYSTEM"] = "FreeBSD"
        self.run_install(success=False)
        self.assertEqual(self.calls(), [])

    def test_download_and_plugin_registration_failures_propagate(self):
        self.env["LEANCK_TEST_DOWNLOAD_FAILURE"] = "1"
        self.assertIn(b"download failed", self.run_install(success=False).stderr)
        self.assertFalse((self.home / ".local/bin").exists())
        del self.env["LEANCK_TEST_DOWNLOAD_FAILURE"]
        self.env["LEANCK_TEST_HOST_FAILURE"] = "claude"
        self.run_install("--plugin", "claude", success=False)
        self.assertTrue((self.home / "data/leanck/bin/leanck").exists())

    def test_custom_paths_and_mirror(self):
        binary_dir = self.root / "custom bin"
        data_dir = self.root / "custom data"
        self.run_install("--bin-dir", str(binary_dir), "--data-dir", str(data_dir),
                         "--release-url", "https://mirror.example/releases/", "--plugin", "claude")
        self.assertTrue((binary_dir / "leanck").is_file())
        self.assertTrue((data_dir / "bundle/plugin/bin/leanck").is_file())
        self.assertTrue(self.calls()[0][-1].startswith("https://mirror.example/releases/latest/download/"))

    def test_macos_arm64_detection_without_signing(self):
        self.env["LEANCK_TEST_SYSTEM"] = "Darwin"
        self.env["LEANCK_TEST_ARCH"] = "arm64"
        mac_archive = "leanck-aarch64-apple-darwin.tar.gz"
        shutil.copyfile(self.assets / ARCHIVE, self.assets / mac_archive)
        shutil.copyfile(self.assets / f"{ARCHIVE}.sha256", self.assets / f"{mac_archive}.sha256")
        self.run_install("--plugin", "claude")
        self.assertIn(mac_archive, self.calls()[0][-1])
        self.assertEqual([c for c in self.calls() if c[0] == "codesign"], [])

    def test_pinned_version_mismatch_fails_before_installing(self):
        self.assertIn(b"version does not match", self.run_install("--version", "9.0.0", success=False).stderr)
        self.assertFalse((self.home / ".local/bin").exists())

    def test_packaged_catalogs_and_plugin_versions_match(self):
        with tarfile.open(self.assets / ARCHIVE) as archive:
            self.assertEqual(archive.extractfile("leanck/.leanck-release").read(), b"v1.2.3\n")
            for host in ("claude", "codex"):
                manifest = json.load(archive.extractfile(f"leanck/plugin/.{host}-plugin/plugin.json"))
                self.assertEqual(manifest["version"], "1.2.3")
            catalog = json.load(archive.extractfile("leanck/.agents/plugins/marketplace.json"))
            self.assertEqual(catalog["plugins"][0]["source"]["path"], "./plugin")


if __name__ == "__main__":
    unittest.main()
