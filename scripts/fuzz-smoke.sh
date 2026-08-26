#!/usr/bin/env bash
# fuzz-smoke.sh — smoke test di tutti i target (default 90s ciascuno).
# Scrive un riepilogo in fuzz/campaign-logs/smoke-summary.txt.
set -uo pipefail

IMAGE="${FUZZ_IMAGE:-plenora-rust:nightly-fuzz}"
SECONDS_PER_TARGET="${FUZZ_SMOKE_SECONDS:-90}"
CARGO_FUZZ="${FUZZ_CARGO_FUZZ:-/fuzzbin/cargo-fuzz}"

ALL_TARGETS=(
    plan_contract string_chain candidate_chain binary_ops
    reshape_policies extended_ops advanced_ops
    wkb_contract wkt_operations arrow_envelope arrow_ipc_decode arrow_transform
    plan_v5_parse analyze_table analyze_geo diff_kernels executor_dag
    protocollo_frame
)
TARGETS=(${FUZZ_TARGETS:-${ALL_TARGETS[@]}})

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$PROJECT_ROOT/fuzz/campaign-logs"
SUMMARY="$PROJECT_ROOT/fuzz/campaign-logs/smoke-summary.txt"
echo "== smoke $(date -Is): ${TARGETS[*]} (${SECONDS_PER_TARGET}s)" >> "$SUMMARY"

# Esito accumulato. Lo smoke ESEGUE tutti i target — fermarsi al primo
# fallimento nasconderebbe gli altri — ma deve terminare non-zero se almeno
# uno fallisce: prima registrava l'errore nel riepilogo e usciva 0, quindi un
# target inesistente o crashato passava per uno smoke riuscito.
falliti=()

for target in "${TARGETS[@]}"; do
    mkdir -p "$PROJECT_ROOT/fuzz/artifacts/$target"
    log="$PROJECT_ROOT/fuzz/campaign-logs/smoke-$target.log"
    MSYS_NO_PATHCONV=1 docker run --rm --cpus=4 --memory=10g \
        -v "$PROJECT_ROOT:/work" \
        -v "C:/tmp/plenora-geo-tools-arrow/.fuzz-cargo/bin:/fuzzbin:ro" \
        -w /work/fuzz -e CARGO_TERM_COLOR=never \
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
