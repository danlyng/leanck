set shell := ["bash", "-cu"]

plugin_bin := justfile_directory() / "plugin" / "bin"

# Path to the already-built forked `lake` (the one with `--json` support).
# Override with `LEANCK_LAKE_SRC=/path/to/lake just stage-lake`. The fork lives
# outside this workspace (a Lean source tree, not a cargo crate), so building it
# is a separate, manual step; this only stages the resulting binary.
lake_src := env_var_or_default("LEANCK_LAKE_SRC", justfile_directory() / ".." / "__clones" / "lean4" / "build" / "release" / "stage1" / "bin" / "lake")

default:
    @just --list

# Build all components: stage plugin binaries (leanck-mcp + leanckd), install the `leanck` CLI on PATH.
build: build-mcp build-leanckd install-cli

# Run the whole test suite (Rust workspace).
test: test-rust

# Run all Rust workspace tests.
test-rust:
    cargo test --workspace

# Compile the Rust MCP server in release mode and stage it in plugin/bin/.
# macOS gives Rust binaries a linker-signed adhoc signature that `cp` does
# not preserve faithfully — the kernel SIGKILLs the destination on first
# launch (`CODE SIGNING: rejecting invalid page ... tainted:1`). Re-sign
# ad-hoc after copy so the staged binary actually runs.
build-mcp:
    cargo build --release -p leanck-mcp
    mkdir -p {{plugin_bin}}
    cp target/release/leanck-mcp {{plugin_bin}}/leanck-mcp
    if [ "$(uname)" = "Darwin" ]; then codesign --force -s - {{plugin_bin}}/leanck-mcp; fi

# Compile the leanckd daemon in release mode and stage it in plugin/bin/.
# leanck-mcp finds it as a sibling and auto-spawns it. The macOS ad-hoc
# re-sign is mandatory: the daemon is long-lived and an unsigned-after-`cp`
# binary gets SIGKILLed on launch (same reason as build-mcp).
build-leanckd:
    cargo build --release -p leanckd
    mkdir -p {{plugin_bin}}
    cp target/release/leanckd {{plugin_bin}}/leanckd
    if [ "$(uname)" = "Darwin" ]; then codesign --force -s - {{plugin_bin}}/leanckd; fi

# Install the `leanck` CLI and the `leanckd` daemon onto PATH (same cargo bin
# dir), so the CLI finds `leanckd` as a sibling of its own binary and can
# auto-spawn it on the first live-Lean query.
install-cli:
    cargo install --path leanck-cli --locked
    cargo install --path leanckd --locked

# Stage the forked `lake` (with `--json`) into plugin/bin/ as `leanck-lake`.
# The `build` MCP tool / `leanck build` resolve it as a sibling of their own
# binary (or via `LEANCK_LAKE_BIN`); it is deliberately *not* named `lake`, so
# the conventional lake on PATH is never invoked. Build the fork first
# (separate Lean source tree), then run this.
#
# We symlink rather than copy: lake locates its toolchain libraries via
# `@executable_path/../lib`, so a bare copy into plugin/bin/ (with no sibling
# `../lib`) aborts on launch. A symlink keeps `@executable_path` resolving to
# the in-tree binary's directory, so the libraries are found and no re-sign is
# needed. (For distribution to another machine, set `LEANCK_LAKE_BIN` instead.)
stage-lake:
    mkdir -p {{plugin_bin}}
    ln -sf {{lake_src}} {{plugin_bin}}/leanck-lake

# Remove staged plugin binaries (does not touch target/).
clean-plugin-bin:
    rm -f {{plugin_bin}}/leanck-mcp {{plugin_bin}}/leanckd {{plugin_bin}}/leanck-lake
