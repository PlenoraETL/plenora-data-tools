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
#
# I PROFILI GREZZI si puliscono PRIMA e DOPO, sempre, anche se la misura
# fallisce (`trap`). I `.profraw` hanno nomi che contengono il pid, e il pid
# il sistema lo ricicla: con i profili di esecuzioni precedenti ancora sul
# disco, un processo nuovo trova il proprio nome occupato e LLVM scrive
# l'errore su **stderr**, facendo fallire i test che pretendono stderr vuoto.
# Il 2026-08-21, con 6 770 profili accumulati, due campagne consecutive sono
# fallite per questo; dopo la pulizia, zero errori. La pulizia finale serve
# anche a non lasciare residui alla cache della CI.
set -euo pipefail

IMAGE=rust:1.92
# Pin del tool: un aggiornamento di cargo-llvm-cov puo' spostare i conteggi
# e quindi il verdetto del gate (stesso pin del job `coverage` in CI).
LLVM_COV_VERSION=0.8.7
MSYS_NO_PATHCONV=1
export MSYS_NO_PATHCONV

RADICE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Percorso RELATIVO dalla radice, e nessun argomento: su Git Bash per Windows
# `$RADICE` e' un percorso MSYS (`/c/...`) che il `python.exe` nativo non sa
# risolvere — lo interpreta come `C:\c\...` e non trova il file. Senza
# argomenti lo script ricava la radice da `__file__`, in forma nativa.
pulisci_profili() {
  ( cd "$RADICE" && python scripts/pulisci_profili_coverage.py )
}

# Prima della misura: fatale se fallisce. Misurare sopra profili altrui e' il
# difetto che questa pulizia esiste per evitare, e proseguire lo
# riprodurrebbe.
pulisci_profili

# Dopo, sempre, anche su errore o interruzione: i profili non devono
# sopravvivere alla campagna che li ha prodotti, ne' finire nella cache.
#
# Se la pulizia finale fallisce e la misura era andata bene, la campagna
# FALLISCE: dichiarare successo lasciando i residui significa consegnare alla
# prossima campagna — o alla cache della CI — esattamente il difetto che
# questa pulizia esiste per evitare. Se la misura era gia' fallita, l'esito
# originale si conserva: e' quello che chi legge deve diagnosticare.
pulizia_finale() {
  esito=$?
  if ! pulisci_profili; then
    echo "ERRORE: pulizia finale dei profili fallita; i residui verrebbero" >&2
    echo "        riusati dalla prossima campagna." >&2
    if [ "$esito" -eq 0 ]; then
      esito=1
    fi
  fi
  exit "$esito"
}

trap pulizia_finale EXIT

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
