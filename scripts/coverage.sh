#!/usr/bin/env bash
# Coverage del workspace con cargo-llvm-cov nel container rust:1.92 (come
# AGENTS.md: stessa toolchain di CI). Prima run: lento (build strumentata).
# Uso: scripts/coverage.sh [--html]
#
# Output: target/tmp/lcov.info (+ target/tmp/coverage/ se --html) e tabella
# riepilogativa su stdout. I binari cargo-llvm-cov vivono nel volume
# plenora-cargo-bin (installati automaticamente alla prima esecuzione).
set -euo pipefail

IMAGE=rust:1.92
MSYS_NO_PATHCONV=1
export MSYS_NO_PATHCONV

docker run --rm \
  -v "$PWD:/work" \
  -v plenora-cargo:/usr/local/cargo/registry \
  -v plenora-cargo-bin:/opt/cargo-bin \
  -w /work "$IMAGE" bash -c "
set -e
export PATH=/opt/cargo-bin/bin:\$PATH
if ! command -v cargo-llvm-cov >/dev/null; then
  rustup component add llvm-tools-preview >/dev/null
  CARGO_INSTALL_ROOT=/opt/cargo-bin cargo install cargo-llvm-cov --locked --quiet
fi
if [ \"${1:-}\" = '--html' ]; then
  CARGO_TARGET_DIR=target-cov cargo llvm-cov --workspace --locked --html --output-dir target/tmp/coverage --summary-only
else
  CARGO_TARGET_DIR=target-cov cargo llvm-cov --workspace --locked --lcov --output-path target/tmp/lcov.info --summary-only
fi
"
