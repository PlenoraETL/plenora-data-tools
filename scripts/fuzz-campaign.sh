#!/usr/bin/env bash
# fuzz-campaign.sh — campagna lunga di fuzzing per plenora-data-tools.
#
# Esegue OGNI target cargo-fuzz in sequenza, ciascuno per FUZZ_HOURS_PER_TARGET
# ore (default 1), dentro il container nightly con risorse limitate. Corpus e
# crash artifact restano sulla macchina host (volume /work):
#
#   fuzz/corpus/<target>/            corpus accumulato (riusato tra run)
#   fuzz/artifacts/<target>/         crash artifact (input minimizzabile)
#   fuzz/campaign-logs/<target>.log  log libFuzzer dell'ultimo run
#
# Uso:
#   scripts/fuzz-campaign.sh                      # tutti i target, 1h ciascuno
#   FUZZ_HOURS_PER_TARGET=2 scripts/fuzz-campaign.sh
#   FUZZ_TARGETS="plan_v5_parse diff_kernels" scripts/fuzz-campaign.sh
#   FUZZ_JOBS=4 FUZZ_WORKERS=4 scripts/fuzz-campaign.sh   # fork paralleli
#
# Per una campagna 1-2 giorni: FUZZ_HOURS_PER_TARGET=1.5 circa copre 17
# target in ~26 ore. Interruzione pulita con Ctrl-C (il run corrente viene
# terminato, il corpus e' gia' su disco).

set -euo pipefail

IMAGE="${FUZZ_IMAGE:-plenora-rust:nightly-fuzz}"
# cargo-fuzz non e' installato nell'immagine: si usa il binario precompilato
# Linux condiviso dei progetti plenora (montato read-only su /fuzzbin).
CARGO_FUZZ="${FUZZ_CARGO_FUZZ:-/fuzzbin/cargo-fuzz}"
# Default sotto la home e non `C:/tmp/...`: quel percorso era una macchina
# sola, e su ogni altra il mount sarebbe vuoto. Ricetta in docs/release.md,
# sezione «Fuzzing».
FUZZBIN_HOST="${FUZZBIN_HOST:-$HOME/.plenora-fuzz/bin}"
HOURS="${FUZZ_HOURS_PER_TARGET:-1}"
CPUS="${FUZZ_CPUS:-4}"
MEMORY="${FUZZ_MEMORY:-10g}"
JOBS="${FUZZ_JOBS:-1}"
WORKERS="${FUZZ_WORKERS:-1}"

ALL_TARGETS=(
    # Portati da plenora-nogeo-tools
    plan_contract string_chain candidate_chain binary_ops
    reshape_policies extended_ops advanced_ops
    # Portati da plenora-geo-tools-arrow
    wkb_contract wkt_operations arrow_envelope arrow_ipc_decode arrow_transform
    # Nuovi (Fase 2A)
    plan_v5_parse analyze_table analyze_geo diff_kernels executor_dag
    # Fase 4
    protocollo_frame
)
TARGETS=(${FUZZ_TARGETS:-${ALL_TARGETS[@]}})

SECONDS_PER_TARGET=$(awk "BEGIN { printf \"%d\", $HOURS * 3600 }")

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Gli stessi due prerequisiti dello smoke, controllati prima di partire. Qui
# conta il doppio: una campagna si lancia e si lascia andare per ore, e
# scoprire dopo che il primo target e' morto sul mount vuoto significa aver
# perso la notte, non un minuto.
mancanti=0
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "ERRORE: l'immagine '$IMAGE' non esiste in locale; si costruisce," >&2
    echo "        e la ricetta e' in docs/release.md, sezione «Fuzzing»." >&2
    mancanti=1
fi
if [ ! -f "$FUZZBIN_HOST/cargo-fuzz" ]; then
    echo "ERRORE: '$FUZZBIN_HOST/cargo-fuzz' non c'e'; stessa ricetta." >&2
    echo "        Se lo tieni altrove: FUZZBIN_HOST=..." >&2
    mancanti=1
fi
if [ "$mancanti" -ne 0 ]; then
    exit 1
fi

mkdir -p "$PROJECT_ROOT/fuzz/campaign-logs"

echo "== campagna fuzz: ${#TARGETS[@]} target x ${HOURS}h (image ${IMAGE}, cpus=${CPUS}, mem=${MEMORY})"

# Corpus e artifact vivono in /dev/shm (tmpfs, RAM della VM) dentro il
# container del run: gli hang kernel ripetuti (fuzzer in stato D,
# __vma_start_write <- do_mmap) sono stati ricondotti via sysrq a stalli di
# I/O del disco virtuale WSL2 (kjournald2 bloccato in jbd2, vhdx su Windows):
# qualunque file su ext4 della VM puo' piantare il page-fault path del fuzzer.
# Soluzione: nessun file del fuzzer tocca ext4 durante il run. Il corpus
# viene seminato dall'host a inizio target e risincronizzato a fine target
# dentro lo stesso container (un crash della VM perde solo il target in corso).
# Anche il binario viene pre-caricato in page cache (e' sul bind mount 9p).
SHM_SIZE="${FUZZ_SHM_SIZE:-4g}"

for target in "${TARGETS[@]}"; do
    mkdir -p "$PROJECT_ROOT/fuzz/artifacts/$target" "$PROJECT_ROOT/fuzz/corpus/$target"
    log="$PROJECT_ROOT/fuzz/campaign-logs/$target.log"
    echo "== $(date -Is) start $target (max_total_time=${SECONDS_PER_TARGET}s)"
    MSYS_NO_PATHCONV=1 docker run --rm \
        --cpus="$CPUS" --memory="$MEMORY" --shm-size="$SHM_SIZE" \
        -v "$PROJECT_ROOT:/work" \
        -v "$FUZZBIN_HOST:/fuzzbin:ro" \
        -w /work/fuzz \
        -e CARGO_TERM_COLOR=never \
        -e GLIBC_TUNABLES="glibc.malloc.mmap_threshold=16777216:glibc.malloc.arena_max=2" \
        "$IMAGE" sh -c "
            set -u
            shm=/dev/shm/fuzzdata
            mkdir -p \$shm/corpus/$target \$shm/artifacts/$target
            cp -a /work/fuzz/corpus/$target/. \$shm/corpus/$target/ 2>/dev/null || true
            # pre-warm: binario e dipendenze in page cache, poi nessun I/O
            cat target/x86_64-unknown-linux-gnu/release/$target > /dev/null 2>/dev/null || true
            $CARGO_FUZZ fuzz run $target -- \
                -max_total_time=$SECONDS_PER_TARGET \
                -artifact_prefix=\$shm/artifacts/$target/ \
                -jobs=$JOBS -workers=$WORKERS \
                -timeout=60 -rss_limit_mb=8192 \
                \$shm/corpus/$target
            rc=\$?
            cp -a \$shm/corpus/$target/. /work/fuzz/corpus/$target/
            cp -a \$shm/artifacts/$target/. /work/fuzz/artifacts/$target/
            exit \$rc" \
        2>&1 | tee "$log" || true
    crashes=$(find "$PROJECT_ROOT/fuzz/artifacts/$target" -type f ! -name '.*' | wc -l)
    echo "== $(date -Is) end   $target (crash artifact: $crashes)"
done

echo "== campagna terminata: $(date -Is)"
find "$PROJECT_ROOT/fuzz/artifacts" -type f ! -name '.*' -print
