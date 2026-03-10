"""Assemble the leanck Claude Code and Codex plugin from repository sources.

Run through `just package-plugin`, which builds and stages the release
binaries first. The output directory is generated: it is replaced as a whole
and must not be edited by hand. Sources are:

- packaging/plugin/       manifests, MCP configurations, and README
- leanck-skills/          skills (shared by Claude Code and Codex)
- leanck-subagents/       Claude Code agents (*.md) and Codex agents (*.codex.toml)
- leanck-hooks/           Claude Code hooks
- --bin-dir               staged leanck executable
"""

import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import tomllib

BINARIES = ("leanck",)
MARKER = ".leanck-package.json"


class PackageError(Exception):
    pass


def frontmatter(path):
    """Return the top-level keys of a Markdown file's YAML frontmatter."""
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0] != "---":
        raise PackageError(f"{path}: missing YAML frontmatter")
    try:
        end = lines.index("---", 1)
    except ValueError:
        raise PackageError(f"{path}: unterminated YAML frontmatter") from None
    keys = {}
    for line in lines[1:end]:
        if not line or line[0] in " \t-#":
            continue
        key, sep, value = line.partition(":")
        if not sep or not key.replace("-", "").replace("_", "").isalnum():
            raise PackageError(f"{path}: malformed frontmatter line {line!r}")
        keys[key] = value.strip().strip("\"'")
    for required in ("name", "description"):
        if not keys.get(required):
            raise PackageError(f"{path}: frontmatter lacks {required!r}")
    return keys


def check_json(path):
    try:
        json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise PackageError(f"{path}: invalid JSON: {error}") from None


def copy_tree(source, destination):
    shutil.copytree(source, destination, dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns(".DS_Store", "__pycache__"))


def add_skills(repo, out):
    skills = sorted(p for p in (repo / "leanck-skills").iterdir() if p.is_dir())
    for skill in skills:
        keys = frontmatter(skill / "SKILL.md")
        if keys["name"] != skill.name:
            raise PackageError(f"{skill / 'SKILL.md'}: name {keys['name']!r} differs from directory")
        copy_tree(skill, out / "skills" / skill.name)
    return [skill.name for skill in skills]


def add_agents(repo, out):
    source = repo / "leanck-subagents"
    claude, codex = [], []
    for path in sorted(source.glob("*.md")):
        frontmatter(path)
        (out / "agents").mkdir(exist_ok=True)
        shutil.copy2(path, out / "agents" / path.name)
        claude.append(path.stem)
    for path in sorted(source.glob("*.codex.toml")):
        try:
            agent = tomllib.loads(path.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError as error:
            raise PackageError(f"{path}: invalid TOML: {error}") from None
        for required in ("name", "description", "developer_instructions"):
            if not agent.get(required):
                raise PackageError(f"{path}: lacks {required!r}")
        name = path.name.removesuffix(".codex.toml")
        if agent["name"] != name:
            raise PackageError(f"{path}: name {agent['name']!r} differs from file name")
        (out / "codex" / "agents").mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, out / "codex" / "agents" / f"{name}.toml")
        codex.append(name)
    return claude, codex


def add_hooks(repo, out):
    source = repo / "leanck-hooks"
    check_json(source / "hooks.json")
    copy_tree(source, out / "hooks")


def add_binaries(bin_dir, out, *, sign=True):
    (out / "bin").mkdir()
    for name in BINARIES:
        source = bin_dir / name
        if not source.is_file() or not os.access(source, os.X_OK):
            raise PackageError(f"{source}: staged executable is missing")
        target = out / "bin" / name
        shutil.copy2(source, target)
        if sign and platform.system() == "Darwin":
            subprocess.run(["codesign", "--force", "-s", "-", str(target)],
                           check=True, capture_output=True)


def revision(repo):
    def git(*args):
        try:
            result = subprocess.run(["git", "-C", str(repo), *args],
                                    capture_output=True, text=True)
        except FileNotFoundError:
            return None
        return result.stdout.strip() if result.returncode == 0 else None
    return {"commit": git("rev-parse", "HEAD"),
            "dirty": bool(git("status", "--porcelain"))}


def replace(staged, output):
    """Swap the assembled package into place, refusing to delete foreign data."""
    if output.exists() or output.is_symlink():
        if output.is_symlink() or not (output / MARKER).is_file():
            raise PackageError(f"{output} exists and is not a generated package; remove it manually")
        retired = output.with_name(f".{output.name}.old")
        shutil.rmtree(retired, ignore_errors=True)
        output.rename(retired)
        staged.rename(output)
        shutil.rmtree(retired)
    else:
        staged.rename(output)


def package(repo, bin_dir, output, *, sign_binaries=True):
    overlay = repo / "packaging" / "plugin"
    for manifest in overlay.rglob("*.json"):
        check_json(manifest)
    output.parent.mkdir(parents=True, exist_ok=True)
    staged = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    try:
        copy_tree(overlay, staged)
        skills = add_skills(repo, staged)
        claude, codex = add_agents(repo, staged)
        add_hooks(repo, staged)
        if bin_dir is not None:
            add_binaries(bin_dir, staged, sign=sign_binaries)
        source = revision(repo)
        (staged / MARKER).write_text(json.dumps({
            "generated_by": "just package-plugin",
            "source": source,
            "skills": skills,
            "claude_agents": claude,
            "codex_agents": codex,
            "binaries": list(BINARIES) if bin_dir is not None else [],
        }, indent=2) + "\n", encoding="utf-8")
        staged.chmod(0o755)
        replace(staged, output)
    except BaseException:
        shutil.rmtree(staged, ignore_errors=True)
        raise
    print(f"packaged {output}")
    print(f"  skills: {', '.join(skills)}")
    print(f"  claude agents: {', '.join(claude)}")
    print(f"  codex agents: {', '.join(codex)}")
    if source["dirty"]:
        print("  note: built from a working tree with uncommitted changes")


def main():
    parser = argparse.ArgumentParser(
        description="Assemble the leanck Claude Code and Codex plugin from repository sources.")
    parser.add_argument("--bin-dir", type=Path, required=True,
                        help="directory holding the staged leanck executable")
    parser.add_argument("--output", type=Path, required=True,
                        help="generated plugin directory to create or replace")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    try:
        package(repo, args.bin_dir.resolve(), args.output.absolute())
    except PackageError as error:
        sys.exit(f"package-plugin: {error}")


if __name__ == "__main__":
    main()
