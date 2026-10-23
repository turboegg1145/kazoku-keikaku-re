#!/usr/bin/env bash
# 交叉编译 Rust 引擎成 iOS 静态库。
set -euo pipefail
cd "$(dirname "$0")/../../rust"

TARGET=${1:-aarch64-apple-ios}
rustup target add "$TARGET"
cargo build --release --target "$TARGET"
echo "-> $(pwd)/target/$TARGET/release/libkzn.a"