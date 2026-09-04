features := ""
libc := "gnu"
arch := "" # use the default architecture
os := "" # use the default os

_features := if features == "all" {
        "--all-features"
    } else if features != "" {
        "--features=" + features
    } else { "" }

_arch := if arch == "" {
        arch()
    } else if arch == "amd64" {
        "x86_64"
    } else if arch == "x86_64" {
        "x86_64"
    } else if arch == "arm64" {
        "aarch64"
    } else if  arch == "aarch64" {
        "aarch64"
    } else {
        error("unsupported arch=" + arch)
    }

_os := if os == "" {
        os()
    } else {
        os
    }

_os_target := if _os == "macos" {
        "apple-darwin"
    } else if _os == "linux" {
        "unknown-linux"
    } else {
        error("unsupported os=" + _os)
    }

_default_target := `rustc -vV | sed -n 's|host: ||p'`
target := _arch + "-" + _os_target + if _os == "linux" { "-" + libc } else { "" }
_resolved_target := if target != _default_target { target } else { "" }
_target-option := if _resolved_target != "" { "--target " + _resolved_target } else { "" }

clean:
    cargo clean

fmt:
    cargo fmt --all

check-fmt:
    cargo fmt --all -- --check

clippy: (_target-installed target)
    cargo clippy {{ _target-option }} --all-targets --all-features --workspace -- -D warnings

# Runs all lints (fmt, clippy, deny)
lint: check-fmt clippy

# wasm32-unknown-unknown has no std clock and no Tokio runtime: `std::time::{Instant,
# SystemTime}::now()` and `tokio::time::*` compile but abort at runtime. Two guards:
# the `disallowed-methods` lint in wasm-check/clippy.toml over the SDK's own source
# (selected via CLIPPY_CONF_DIR so native clippy is unaffected), and a release build
# of `wasm-check` — which reaches every SDK path — searched for the panic strings
# those calls compile to, catching them inside dependencies too.
# Run by its own CI job, not by `verify`.
check-wasm32: (_target-installed "wasm32-unknown-unknown")
    #!/usr/bin/env bash
    set -euo pipefail
    CLIPPY_CONF_DIR="$PWD/wasm-check" cargo clippy --target wasm32-unknown-unknown -p restate-sdk -p restate-sdk-wasm-check \
        --no-default-features --features restate-sdk/rand,restate-sdk/uuid,restate-sdk/rust_crypto -- -D warnings
    cargo build --release --target wasm32-unknown-unknown -p restate-sdk-wasm-check
    wasm=target/wasm32-unknown-unknown/release/restate_sdk_wasm_check.wasm
    for needle in 'time not implemented on this platform' 'there is no reactor running'; do
        if grep -a -q "$needle" "$wasm"; then
            echo "error: '$needle' reachable in $wasm — a std::time / tokio::time clock is on a wasm32 path" >&2
            exit 1
        fi
    done

# Checks the SDK's minimal build and both tunnel crypto-provider configurations.
check-sdk-features: (_target-installed target)
    cargo check {{ _target-option }} -p restate-sdk --no-default-features
    cargo check {{ _target-option }} -p restate-sdk --no-default-features --features tunnel,rust_crypto
    cargo check {{ _target-option }} -p restate-sdk --no-default-features --features tunnel,aws_lc_rs
    cargo check {{ _target-option }} -p restate-sdk --features tunnel --example tunnel
    cargo tree -p restate-sdk --no-default-features

build *flags: (_target-installed target)
    cargo build {{ _target-option }} {{ _features }} {{ flags }}

print-target:
    @echo {{ _resolved_target }}

test: (_target-installed target)
    cargo nextest run {{ _target-option }} --all-features --workspace

test-tunnel: (_target-installed target)
    cargo test {{ _target-option }} -p restate-sdk --no-default-features --features tunnel,rust_crypto
    cargo test {{ _target-option }} -p restate-sdk --no-default-features --features tunnel,aws_lc_rs
    cargo test {{ _target-option }} -p restate-sdk --doc --features tunnel

doctest: (_target-installed target)
    cargo test {{ _target-option }} --doc --workspace --all-features

# Runs lints and tests
verify: lint check-sdk-features test-tunnel test doctest

udeps *flags:
    RUSTC_BOOTSTRAP=1 cargo udeps --all-features --all-targets {{ flags }}

_target-installed target:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! rustup target list --installed |grep -qF '{{ target }}' 2>/dev/null ; then
        rustup target add '{{ target }}'
    fi
