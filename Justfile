set shell := ["bash", "-cu"]

plugin_bin := justfile_directory() / "plugin" / "bin"
plugin_package := justfile_directory() / "leanck-plugin"

default:
    @just --list

# Stage the unified executable for the plugin and install it on PATH.
install: build-plugin-bin install-cli

# Run the whole test suite (Rust workspace).
test:
    cargo test --workspace --locked

# Compile and stage the CLI, MCP server, and daemon as one executable.
build-plugin-bin:
    cargo build --release -p leanck-cli --locked
    mkdir -p "{{plugin_bin}}"
    cp target/release/leanck "{{plugin_bin}}"/leanck
    if [ "$(uname)" = "Darwin" ]; then codesign --force -s - "{{plugin_bin}}"/leanck; fi

# Install `leanck` via Cargo. For production use, prefer `install.sh`.
install-cli:
    cargo install --path crates/leanck-cli --locked

# Remove staged plugin binaries (does not touch target/).
clean-plugin-bin:
    rm -f "{{plugin_bin}}"/leanck "{{plugin_bin}}"/leanck-mcp "{{plugin_bin}}"/leanckd "{{plugin_bin}}"/leanck-lake

# Stage the unified executable without installing on PATH.
stage destination:
    cargo build --release --workspace --locked
    mkdir -p "{{destination}}"
    cp target/release/leanck "{{destination}}/leanck"
    if [ "$(uname)" = "Darwin" ]; then codesign --force -s - "{{destination}}/leanck"; fi

# Verify a temporary release distribution; the template defaults to the in-repo project.
verify-distribution project=(justfile_directory() / "test-support/lean-project"):
    #!/usr/bin/env bash
    set -euo pipefail
    staging_dir=$(mktemp -d "${TMPDIR:-/tmp}/leanck-distribution.XXXXXX")
    trap 'rm -rf "$staging_dir"' EXIT
    just stage "$staging_dir"
    python3 test-support/verify_distribution.py --bin-dir "$staging_dir" --project "{{project}}"

# Assemble the Claude Code and Codex plugin from repository sources and fresh release binaries.
package-plugin destination=plugin_package:
    #!/usr/bin/env bash
    set -euo pipefail
    staging_dir=$(mktemp -d "${TMPDIR:-/tmp}/leanck-plugin-bin.XXXXXX")
    trap 'rm -rf "$staging_dir"' EXIT
    just stage "$staging_dir"
    cargo xtask package-plugin --bin-dir "$staging_dir" --output "{{destination}}"

# Validate a packaged plugin with the Claude Code validator.
validate-plugin destination=plugin_package:
    claude plugin validate --strict "{{destination}}"
