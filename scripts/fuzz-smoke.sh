#!/usr/bin/env bash
# fuzz-smoke.sh — smoke test di tutti i target (default 90s ciascuno).
# Scrive un riepilogo in fuzz/campaign-logs/smoke-summary.txt.
set -uo pipefail

IMAGE="${FUZZ_IMAGE:-plenora-rust:nightly-fuzz}"
SECONDS_PER_TARGET="${FUZZ_SMOKE_SECONDS:-90}"
CARGO_FUZZ="${FUZZ_CARGO_FUZZ:-/fuzzbin/cargo-fuzz}"
# Dove sta `cargo-fuzz` sull'host, montato read-only su /fuzzbin.
#
# Il default e' **sotto la home**, non `C:/tmp/...`: quel percorso era la
# macchina di chi ha scritto lo script scritta dentro lo script, e su
# qualunque altra non esiste. `$HOME` esiste ovunque, Git Bash compreso, e
# Docker Desktop sa montare da li'. Chi lo tiene altrove passa `FUZZBIN_HOST`,
# che e' lo stesso nome gia' usato da `fuzz-campaign.sh`: due script che
# montano la stessa cosa non possono avere due nomi per dirlo.
FUZZBIN_HOST="${FUZZBIN_HOST:-$HOME/.plenora-fuzz/bin}"

ALL_TARGETS=(
    plan_contract string_chain candidate_chain binary_ops
    reshape_policies extended_ops advanced_ops
    wkb_contract wkt_operations arrow_envelope arrow_ipc_decode arrow_transform
    plan_v5_parse analyze_table analyze_geo diff_kernels executor_dag
    protocollo_frame
)
TARGETS=(${FUZZ_TARGETS:-${ALL_TARGETS[@]}})

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# I due prerequisiti si controllano PRIMA di lanciare qualunque target, e si
# nominano. Senza, Docker fallisce con «pull access denied for plenora-rust»
# quando manca l'immagine — un messaggio che manda a cercare credenziali per
# un'immagine che non e' su nessun registry — e con un mount vuoto quando
# manca il binario, cioe' «exec /fuzzbin/cargo-fuzz: no such file». Nessuno dei
# due dice che cosa fare, e li ha gia' fatti perdere un'ora a qualcuno.
mancanti=0
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "ERRORE: l'immagine '$IMAGE' non esiste in locale." >&2
    echo "        Non e' su un registry: si costruisce, e la ricetta e' in" >&2
    echo "        docs/release.md, sezione «Fuzzing»." >&2
    mancanti=1
fi
if [ ! -f "$FUZZBIN_HOST/cargo-fuzz" ]; then
    echo "ERRORE: '$FUZZBIN_HOST/cargo-fuzz' non c'e'." >&2
    echo "        E' il binario che lo script monta su /fuzzbin; si installa" >&2
    echo "        con la stessa ricetta. Se lo tieni altrove: FUZZBIN_HOST=..." >&2
    mancanti=1
fi
if [ "$mancanti" -ne 0 ]; then
    exit 1
fi

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
        -v "$FUZZBIN_HOST:/fuzzbin:ro" \
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
