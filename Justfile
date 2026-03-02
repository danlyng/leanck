set shell := ["bash", "-cu"]

plugin_bin := justfile_directory() / "plugin" / "bin"

default:
    @just --list

# Build all components: stage plugin binaries (leanck-mcp + leanckd), install the `leanck` CLI on PATH.
build: build-mcp build-leanckd install-cli

# Run the whole test suite (Rust workspace).
test:
    cargo test --workspace --locked

# Compile the Rust MCP server in release mode and stage it in plugin/bin/.
build-mcp:
    cargo build --release -p leanck-mcp --locked
    mkdir -p "{{plugin_bin}}"
    cp target/release/leanck-mcp "{{plugin_bin}}"/leanck-mcp
    if [ "$(uname)" = "Darwin" ]; then codesign --force -s - "{{plugin_bin}}"/leanck-mcp; fi

# Compile the leanckd daemon in release mode and stage it in plugin/bin/.
build-leanckd:
    cargo build --release -p leanckd --locked
    mkdir -p "{{plugin_bin}}"
    cp target/release/leanckd "{{plugin_bin}}"/leanckd
    if [ "$(uname)" = "Darwin" ]; then codesign --force -s - "{{plugin_bin}}"/leanckd; fi

# Install the `leanck` CLI and the `leanckd` daemon onto PATH
install-cli:
    cargo install --path leanck-cli --locked
    cargo install --path leanckd --locked

# Remove staged plugin binaries (does not touch target/).
clean-plugin-bin:
    rm -f "{{plugin_bin}}"/leanck-mcp "{{plugin_bin}}"/leanckd "{{plugin_bin}}"/leanck-lake

# Stage the CLI, MCP server, and daemon together without installing on PATH.
stage destination:
    cargo build --release --workspace --locked
    mkdir -p "{{destination}}"
    for binary in leanck leanck-mcp leanckd; do cp "target/release/$binary" "{{destination}}/$binary"; if [ "$(uname)" = "Darwin" ]; then codesign --force -s - "{{destination}}/$binary"; fi; done

# Verify a temporary release distribution against a disposable Lean template.
verify-distribution project:
    #!/usr/bin/env bash
    set -euo pipefail
    staging_dir=$(mktemp -d "${TMPDIR:-/tmp}/leanck-distribution.XXXXXX")
    trap 'rm -rf "$staging_dir"' EXIT
    just stage "$staging_dir"
    python3 test-support/verify_distribution.py --bin-dir "$staging_dir" --project "{{project}}"
