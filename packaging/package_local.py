"""Assemble local source for install.sh after the user approves installation."""

import argparse
import json
from pathlib import Path

from package_release import assemble_distribution
from package_plugin import PackageError


def package_local(repo, bin_dir, output, version, inventory):
    assemble_distribution(repo, bin_dir, output, version, sign_binaries=False)
    metadata = json.loads((output / "plugin" / ".leanck-package.json").read_text())
    expected = {
        "skills": sorted(path.name for path in (inventory / "plugin/skills").iterdir()),
        "claude_agents": sorted(path.stem for path in (inventory / "plugin/agents").glob("*.md")),
        "codex_agents": sorted(path.stem for path in (inventory / "plugin/codex/agents").glob("*.toml")),
    }
    for key, names in expected.items():
        if metadata[key] != names:
            raise ValueError("local component names changed after configuration; rerun the installer")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, help="omit for standalone skills and agents only")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--inventory", type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    try:
        package_local(repo, args.bin_dir.resolve() if args.bin_dir else None,
                      args.output.resolve(), args.version, args.inventory.resolve())
    except (PackageError, ValueError) as error:
        parser.exit(1, f"package-local: {error}\n")


if __name__ == "__main__":
    main()
