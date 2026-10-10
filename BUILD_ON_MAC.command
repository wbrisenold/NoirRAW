#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
command -v cargo >/dev/null || { echo 'Install Rust from https://rustup.rs first'; exit 2; }
[[ "$(uname -s)" == Darwin ]] || { echo 'Run this build script on macOS'; exit 3; }
cargo generate-lockfile
cargo test --locked --all-targets
cargo run --release -- --smoke-gpu
cargo build --release --target x86_64-apple-darwin
bash scripts/make_app.sh
open dist
