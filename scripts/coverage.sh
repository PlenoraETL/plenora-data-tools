#!/usr/bin/env bash
# Coverage del workspace con cargo-llvm-cov nel container rust:1.92 (come
# AGENTS.md: stessa toolchain di CI). Prima run: lento (build strumentata).
# Uso: scripts/coverage.sh [--html]
#
# Output: target/tmp/lcov.info (+ target/tmp/coverage/ se --html) e tabella
# riepilogativa su stdout. I binari cargo-llvm-cov vivono nel volume
# plenora-cargo-bin (installati automaticamente alla prima esecuzione).
#
# Le soglie sono le stesse del job `coverage` di .github/workflows/ci.yml:
# questo script e' il modo di riprodurre in locale il verdetto della CI. Se
# cambiano qui vanno cambiate anche la', e viceversa. Perimetro: feature di
# default — i backend nativi (geos static, proj bundled) non sono
# strumentati, come in CI.
set -euo pipefail

IMAGE=rust:1.92
# Pin del tool: un aggiornamento di cargo-llvm-cov puo' spostare i conteggi
# e quindi il verdetto del gate (stesso pin del job `coverage` in CI).
LLVM_COV_VERSION=0.8.7
MSYS_NO_PATHCONV=1
export MSYS_NO_PATHCONV

docker run --rm \
  -v "$PWD:/work" \
  -v plenora-cargo:/usr/local/cargo/registry \
  -v plenora-cargo-bin:/opt/cargo-bin \
  -w /work "$IMAGE" bash -c "
set -e
export PATH=/opt/cargo-bin/bin:\$PATH
export CARGO_TARGET_DIR=target-cov
if ! command -v cargo-llvm-cov >/dev/null; then
  rustup component add llvm-tools-preview >/dev/null
  CARGO_INSTALL_ROOT=/opt/cargo-bin cargo install cargo-llvm-cov --version $LLVM_COV_VERSION --locked --quiet
fi
mkdir -p target/tmp
cargo llvm-cov --workspace --locked --no-report
if [ \"${1:-}\" = '--html' ]; then
  cargo llvm-cov report --html --output-dir target/tmp/coverage
else
  cargo llvm-cov report --lcov --output-path target/tmp/lcov.info
fi
# Stesso verdetto del job \`coverage\` in CI: se questo comando fallisce in
# locale, fallira' anche la CI.
cargo llvm-cov report --summary-only \
  --fail-under-lines 90 \
  --fail-under-functions 85 \
  --fail-under-regions 89
"
