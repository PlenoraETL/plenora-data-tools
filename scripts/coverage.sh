#!/usr/bin/env bash
# Coverage del workspace con cargo-llvm-cov nel container rust:1.98 (come
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
# GLI ARTEFATTI DELLA CAMPAGNA PRECEDENTE si puliscono PRIMA della misura, e
# i profili anche DOPO, sempre, pure se la misura fallisce (`trap`). Le
# classi di residuo sono due e i sintomi opposti:
#
#   - i `.profraw` hanno nomi che contengono il pid, e il pid il sistema lo
#     ricicla: con i profili di esecuzioni precedenti ancora sul disco un
#     processo nuovo trova il proprio nome occupato, LLVM scrive l'errore su
#     **stderr** e i test che pretendono stderr vuoto falliscono: bastano
#     poche migliaia di profili accumulati perche' succeda;
#   - i **binari strumentati** di una build precedente portano con se' la
#     mappa di copertura del codice di allora: le loro righe entrano nel
#     denominatore senza che nessun test le esegua, e il gate diventa non
#     ermetico — percentuali piu' basse del vero, cioe' rossi falsi.
#
# Le rimuove entrambe `scripts/pulisci_coverage.py`, unica sorgente della
# logica di pulizia insieme al job `coverage` della CI. Gira DENTRO il
# container perche' la seconda classe la rimuove `cargo llvm-cov clean
# --workspace`, e cargo-llvm-cov qui vive solo li'.
#
# La pulizia finale, qui in locale, tocca i soli profili: gli artefatti
# strumentati restano, perche' dopo una campagna rossa servono a rieseguire
# `cargo llvm-cov report --html` e vedere DOVE e' scesa la coverage. In CI
# quel bisogno lo copre l'artifact LCOV, e li' la pulizia finale e' completa
# per non spendere la quota di cache in artefatti che il job successivo
# cancella comunque.
set -euo pipefail

IMAGE=rust:1.98
# Pin del tool: un aggiornamento di cargo-llvm-cov puo' spostare i conteggi
# e quindi il verdetto del gate (stesso pin del job `coverage` in CI).
LLVM_COV_VERSION=0.8.7
MSYS_NO_PATHCONV=1
export MSYS_NO_PATHCONV

RADICE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# La pulizia finale gira **dentro il container**, e non sull'host.
#
# PERCHE'
#
#   Il container gira senza `--user`, quindi cargo-llvm-cov scrive i profili
#   come **root**. Una pulizia sull'host — dove l'utente e' quello che ha
#   lanciato lo script — trova quei file e non li puo' togliere: `Permission
#   denied`, campagna rossa, e i profili restano proprio dove la campagna
#   successiva li riuserebbe. Un gate che finisce cosi' e' irriproducibile fuori
#   dalla CI, che questo script non lo invoca affatto.
#
#   Chi ha scritto i file e' l'unico che li puo' togliere senza privilegi presi
#   altrove: la pulizia va dove sta lo scrittore. Non si aggiunge `--user` al
#   container per la ragione opposta — cambierebbe l'utente sotto cui la misura
#   avviene, cioe' proprio cio' che deve restare identico alla CI.
#
#   Si rifa' anche il `chown`? No: cambiare proprietario ai file di una misura
#   in corso e' un'altra scrittura che nessuno ha chiesto, e lascerebbe l'albero
#   in uno stato che dipende da chi ha lanciato lo script.
pulisci_profili() {
  docker run --rm -v "$RADICE:/work" -w /work "$IMAGE" \
    python3 scripts/pulisci_coverage.py --solo-profili
}

# La pulizia PRIMA della misura non e' qui: gira dentro il container, subito
# prima di `cargo llvm-cov --workspace`, perche' rimuove anche gli artefatti
# strumentati e per farlo le serve cargo-llvm-cov. E' fatale come questa
# (`set -e` nel container): misurare sopra i residui di un'altra campagna e'
# il difetto che quella pulizia esiste per evitare, e proseguire lo
# riprodurrebbe.

# Dopo, sempre, anche su errore o interruzione: i profili non devono
# sopravvivere alla campagna che li ha prodotti, ne' finire nella cache.
#
# Se la pulizia finale fallisce e la misura e' andata bene, la campagna
# FALLISCE: dichiarare successo lasciando i residui significa consegnare alla
# prossima campagna — o alla cache della CI — esattamente il difetto che
# questa pulizia esiste per evitare. Se la misura e' gia' fallita, l'esito
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
# Unica sorgente della pulizia, identica a quella del job \`coverage\` in CI:
# artefatti strumentati stantii (cargo llvm-cov clean --workspace) e profili
# grezzi rimasti ovunque sotto target-cov. Fatale per \`set -e\`.
python3 scripts/pulisci_coverage.py
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
