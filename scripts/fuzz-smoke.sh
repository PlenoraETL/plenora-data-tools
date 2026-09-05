#!/usr/bin/env bash
# fuzz-smoke.sh — smoke test di tutti i target (default 90s ciascuno).
# Scrive un riepilogo in fuzz/campaign-logs/smoke-summary.txt.
set -uo pipefail

IMAGE="${FUZZ_IMAGE:-plenora-rust:nightly-fuzz}"
SECONDS_PER_TARGET="${FUZZ_SMOKE_SECONDS:-90}"
CARGO_FUZZ="${FUZZ_CARGO_FUZZ:-/fuzzbin/cargo-fuzz}"
# Dove sta `cargo-fuzz` sull'host, montato read-only su /fuzzbin.
#
# Il default e' **sotto la home**, non `C:/tmp/...`: un percorso del genere
# e' la macchina di chi scrive lo script scritta dentro lo script, e su
# qualunque altra non esiste. `$HOME` esiste ovunque, Git Bash compreso, e
# Docker Desktop sa montare da li'. Chi lo tiene altrove passa `FUZZBIN_HOST`,
# che e' lo stesso nome gia' usato da `fuzz-campaign.sh`: due script che
# montano la stessa cosa non possono avere due nomi per dirlo.
FUZZBIN_HOST="${FUZZBIN_HOST:-$HOME/.plenora-fuzz/bin}"

ALL_TARGETS=(
    plan_contract string_chain candidate_chain binary_ops
    reshape_policies extended_ops advanced_ops
    wkb_contract wkt_operations arrow_envelope arrow_ipc_decode arrow_transform
    geo_frame_stream
    plan_v5_parse analyze_table analyze_geo diff_kernels executor_dag
    protocollo_frame verifica_artefatto
)
TARGETS=(${FUZZ_TARGETS:-${ALL_TARGETS[@]}})

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=scripts/fuzz-preflight.sh
. "$PROJECT_ROOT/scripts/fuzz-preflight.sh"
fuzz_preflight "$IMAGE" "$CARGO_FUZZ" "$FUZZBIN_HOST" || exit 1

mkdir -p "$PROJECT_ROOT/fuzz/campaign-logs"
SUMMARY="$PROJECT_ROOT/fuzz/campaign-logs/smoke-summary.txt"
echo "== smoke $(date -Is): ${TARGETS[*]} (${SECONDS_PER_TARGET}s)" >> "$SUMMARY"

# Esito accumulato. Lo smoke ESEGUE tutti i target — fermarsi al primo
# fallimento nasconderebbe gli altri — ma deve terminare non-zero se almeno
# uno fallisce: registrando l'errore nel riepilogo e uscendo 0, un target
# inesistente o crashato passerebbe per uno smoke riuscito.
falliti=()

# `TMPDIR=/dev/shm` e non il `/tmp` del container: un target che scrive
# file temporanei — l'harness del verificatore lo fa — li mette nel tmpfs
# invece che su ext4 della VM, che e' la stessa disciplina della campagna.
# `/dev/shm` esiste gia' in ogni container, quindi non serve crearlo ne'
# avvolgere il comando in una shell: l'invocazione resta diretta e docker
# conserva i confini degli argomenti.
for target in "${TARGETS[@]}"; do
    mkdir -p "$PROJECT_ROOT/fuzz/artifacts/$target"
    log="$PROJECT_ROOT/fuzz/campaign-logs/smoke-$target.log"
    MSYS_NO_PATHCONV=1 docker run --rm --cpus=4 --memory=10g \
        -v "$PROJECT_ROOT:/work" \
        -v "$FUZZBIN_HOST:/fuzzbin:ro" \
        -w /work/fuzz -e CARGO_TERM_COLOR=never \
        -e TMPDIR=/dev/shm \
        "$IMAGE" \
        "$CARGO_FUZZ" fuzz run "$target" -- \
            -max_total_time="$SECONDS_PER_TARGET" \
            -rss_limit_mb=8192 -timeout=60 \
            -artifact_prefix="/work/fuzz/artifacts/$target/" \
        > "$log" 2>&1
    code=$?
    crashes=$(find "$PROJECT_ROOT/fuzz/artifacts/$target" -type f ! -name '.*' 2>/dev/null | wc -l)
    execs=$(grep -oE '#[0-9]+[[:space:]]+(INITED|NEW)' "$log" | tail -1 || true)
    echo "$target exit=$code crash_artifacts=$crashes last=[$execs]" | tee -a "$SUMMARY"
    if [ "$code" -ne 0 ] || [ "$crashes" -ne 0 ]; then
        falliti+=("$target")
    fi
done

if [ "${#falliti[@]}" -ne 0 ]; then
    echo "smoke FALLITO su: ${falliti[*]}" | tee -a "$SUMMARY"
    exit 1
fi
echo "smoke: ${#TARGETS[@]} target, nessun fallimento" | tee -a "$SUMMARY"
