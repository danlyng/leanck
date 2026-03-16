"""Package a native leanck binary plus integrations for install.sh.

Requires Python 3.11+. Build the binary for --target before running this script.
The release workflow runs this on each target's native operating system/CPU.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile

from package_plugin import package


TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-musl",
)


def assemble_distribution(repo, bin_dir, root, version, *, sign_binaries=True):
    """Assemble an unpacked distribution shared by releases and local installs."""
    version = version.removeprefix("v")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?", version):
        raise ValueError("version must be a semantic version, optionally prefixed with v")
    if bin_dir is not None:
        binary = bin_dir / "leanck"
        result = subprocess.run([str(binary), "--version"], check=True, capture_output=True, text=True)
        if result.stdout.strip() != f"leanck {version}":
            raise ValueError(f"release v{version} does not match binary: {result.stdout.strip()!r}")
    root.mkdir()
    package(repo, bin_dir, root / "plugin", sign_binaries=sign_binaries)
    # Host plugin caches use this version when refreshing their installed copy.
    for host in ("claude", "codex"):
        manifest = root / "plugin" / f".{host}-plugin" / "plugin.json"
        data = json.loads(manifest.read_text())
        data["version"] = version
        manifest.write_text(json.dumps(data, indent=2) + "\n")
    catalogs = {
        ".claude-plugin/marketplace.json": {
            "name": "leanck",
            "owner": {"name": "Dan Lyng"},
            "plugins": [{"name": "leanck", "source": "./plugin"}],
        },
        ".agents/plugins/marketplace.json": {
            "name": "leanck",
            "plugins": [{
                "name": "leanck",
                "source": {"source": "local", "path": "./plugin"},
                "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
                "category": "Engineering",
            }],
        },
    }
    for name, catalog in catalogs.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(catalog, indent=2) + "\n")
    (root / ".leanck-release").write_text(f"v{version}\n")
    (root / ".leanck-update-protocol").write_text("1\n")
    shutil.copy2(repo / "install.sh", root / "install.sh")
    shutil.copy2(repo / "uninstall.sh", root / "uninstall.sh")


def package_release(bin_dir, output, target, version):
    repo = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="leanck-release-") as temp:
        root = Path(temp) / "leanck"
        assemble_distribution(repo, bin_dir, root, version)
        output.mkdir(parents=True, exist_ok=True)
        archive = output / f"leanck-{target}.tar.gz"
        with tarfile.open(archive, "w:gz", format=tarfile.PAX_FORMAT) as tar:
            tar.add(root, arcname="leanck")
        with archive.open("rb") as stream:
            checksum = hashlib.file_digest(stream, "sha256").hexdigest()
        archive.with_suffix(".gz.sha256").write_text(f"{checksum}  {archive.name}\n")
        print(f"release archive: {archive}")
        return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--version", required=True, help="release tag matching leanck --version")
    args = parser.parse_args()
    package_release(args.bin_dir.resolve(), args.output.resolve(), args.target, args.version)


if __name__ == "__main__":
    main()
