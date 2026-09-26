#!/usr/bin/env bash
# Qualifica del profilo isolato sul percorso di **produzione**: il binario
# distribuito, `plenora-data-tools run`, con un dominio cgroup2 reale e il
# worker con un'identita' distinta. Un caso per riga della matrice §10 di
# docs/isolamento.md che si possa provocare da fuori, e l'envelope su stdout
# come oracolo.
#
# CHE COSA AGGIUNGE AGLI ALTRI SCRIPT DI QUALIFICA
#
#   `qualifica_sotto_limite.sh` e `qualifica_worker_reale.sh` guidano il
#   worker con un supervisore di qualificazione (`prova::sul_canale`). Qui il
#   supervisore e' quello di produzione — `esegui_isolato` e
#   `macchina::conduci_isolato` con gli adattatori reali — raggiunto dalla CLI
#   come lo raggiunge un utente.
#
# I CASI
#
#   riga 1   successo verificato e pubblicato, nessun residuo
#   riga 2   errore tipizzato del worker: un batch d'ingresso oltre il tetto
#            fisso di `max_batch_bytes`, che il worker rifiuta da se'
#   riga 5   OOM attribuito, PRIMA della conduzione: un dominio troppo piccolo
#            perche' il worker arrivi a lavorare
#   riga 5   OOM attribuito, DURANTE la conduzione: il worker parte e poi
#            supera il tetto del dominio lavorando
#   riga 6a  il worker ucciso da fuori (SIGKILL), senza evidenza di memoria
#   riga 7   timeout di esecuzione
#   riga 8   cancellazione (SIGINT al processo di `run`)
#
#   Le righe 3, 9, 10, 12, 13 non hanno un innesco da fuori con il binario di
#   produzione: le coprono i test, e questo script non finge di provarle.
#
# CHE COSA NON E'
#
#   Non e' una prova di prestazioni, e i tetti dei casi di OOM sono tarati sul
#   worker di questa build: su un'altra macchina possono servire valori diversi,
#   e lo script lo dice invece di tacere quando il caso non si innesca.
#
# USO (come root, con cgroup2 e il controller memory disponibile)
#
#   cargo build --release -p plenora-cli --locked
#   cargo build --release -p plenora-engine --example ingressi_qualifica --locked
#   sudo scripts/qualifica_profilo_isolato.sh \
#       target/release/plenora-data-tools \
#       target/release/examples/ingressi_qualifica \
#       [UID:GID del worker, default 65534:65534]
#
# In un container: `docker run --privileged --cgroupns=host ...`, perche' il
# dominio deve stare nella gerarchia vera e non in una copia.

set -Eeuo pipefail

BINARIO="${1:?binario plenora-data-tools}"
GENERATORE="${2:?binario ingressi_qualifica}"
WORKER="${3:-65534:65534}"

muori() { echo "qualifica: $*" >&2; exit 2; }

[ "$(id -u)" = 0 ] || muori "serve root: crea il dominio e cede l'identita' al worker"
[ "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs ] || muori "serve cgroup2"
grep -qw memory /sys/fs/cgroup/cgroup.controllers || muori "manca il controller memory"
[ -x "$BINARIO" ] || muori "binario non eseguibile: $BINARIO"
[ -x "$GENERATORE" ] || muori "generatore non eseguibile: $GENERATORE"

# La radice delegata e la directory di lavoro sono di questa esecuzione e
# spariscono in ogni caso, anche su errore.
RADICE="/sys/fs/cgroup/plenora-qualifica-$$"
LAVORO="$(mktemp -d /tmp/plenora-qualifica.XXXXXX)"
chmod 0755 "$LAVORO"
pulisci() {
    if [ -d "$RADICE" ]; then
        find "$RADICE" -mindepth 1 -depth -type d -exec rmdir {} + 2>/dev/null || true
        rmdir "$RADICE" 2>/dev/null || echo "qualifica: RESIDUO $RADICE" >&2
    fi
    rm -rf "$LAVORO"
}
trap pulisci EXIT
mkdir "$RADICE"
echo "+memory +pids" > "$RADICE/cgroup.subtree_control"

echo "== ambiente"
echo "kernel      $(uname -r)"
echo "binario     $(sha256sum "$BINARIO" | cut -d' ' -f1)"
echo "radice      $RADICE"
echo "worker      $WORKER"

# --- gli ingressi -----------------------------------------------------------
"$GENERATORE" "$LAVORO/piccolo.arrow" 1000 1000
# Un solo batch oltre i 64 MiB che l'executor accetta per batch d'ingresso.
"$GENERATORE" "$LAVORO/batch-enorme.arrow" 3000000 3000000
# Molti batch piccoli: il lavoro cresce con le righe, non col singolo batch.
"$GENERATORE" "$LAVORO/grande.arrow" 3000000 65536
chmod 0644 "$LAVORO"/*.arrow

# Un piano v6: `max_domain_memory_bytes` chiede il profilo isolato.
piano() { # file, tetto del dominio, budget governato, nodi JSON
    cat > "$1" <<EOF
{
  "schema_version": 6,
  "inputs": ["righe"],
  "limits": { "max_domain_memory_bytes": $2, "max_governed_memory_bytes": $3 },
  "nodes": $4,
  "output": "fine"
}
EOF
}
FILTRO='[{"id": "fine", "op": "table.filter", "in": ["righe"],
          "config": {"column": "id", "operator": ">", "value": 10}}]'
ORDINA='[{"id": "fine", "op": "table.sort", "in": ["righe"],
          "config": {"columns": ["nome"], "ascending": false}}]'
MiB=$((1024 * 1024))

esiti=()
guasti=0

# Legge l'envelope e confronta. Argomenti: nome, riga, categoria attesa
# ("ok" per il successo), stdout, codice d'uscita, output.
giudica() {
    local nome="$1" riga="$2" atteso="$3" uscita="$4" codice="$5" output="$6"
    local ottenuto
    ottenuto="$(python3 - "$uscita" <<'EOF'
import json, sys
try:
    documento = json.load(open(sys.argv[1]))
except Exception:
    print("envelope-non-json")
    sys.exit(0)
if documento.get("status") == "ok":
    pulizia = documento.get("temp_cleanup", {}).get("state")
    print("ok" if pulizia == "removed" else "ok-residuo-" + str(pulizia))
else:
    print(documento.get("error", {}).get("category", "senza-categoria"))
EOF
)"
    local residui
    residui="$(find "$RADICE" -mindepth 1 -type d | wc -l)"
    local verdetto="VERDE"
    if [ "$ottenuto" != "$atteso" ]; then verdetto="ROSSO"; fi
    if [ "$atteso" = ok ] && [ ! -s "$output" ]; then verdetto="ROSSO"; fi
    if [ "$atteso" != ok ] && [ -e "$output" ]; then verdetto="ROSSO"; fi
    if [ "$atteso" != ok ] && [ "$codice" = 0 ]; then verdetto="ROSSO"; fi
    if [ "$residui" != 0 ]; then verdetto="ROSSO"; fi
    [ "$verdetto" = VERDE ] || guasti=$((guasti + 1))
    esiti+=("$(printf '%-8s %-34s atteso=%-30s ottenuto=%-30s exit=%-4s domini-residui=%s %s' \
        "$riga" "$nome" "$atteso" "$ottenuto" "$codice" "$residui" "$verdetto")")
}

# Esegue `run` con l'ambiente del dispiegamento e nient'altro. Argomenti:
# nome, piano, ingresso, timeout in secondi, tetto dell'host in byte.
esegui() {
    local nome="$1" piano_file="$2" ingresso="$3" tempo="$4" host="$5"
    env -i PATH=/usr/bin:/bin \
        PLENORA_ISOLATION_HOST_MAX_MEMORY_BYTES="$host" \
        PLENORA_ISOLATION_CGROUP_ROOT="$RADICE" \
        PLENORA_ISOLATION_WORKER_UIDGID="$WORKER" \
        PLENORA_ISOLATION_EXECUTION_TIMEOUT_SECONDS="$tempo" \
        "$BINARIO" run --plan "$piano_file" --input "righe=$ingresso" \
        --output "$LAVORO/$nome.out.arrow" \
        > "$LAVORO/$nome.stdout" 2> "$LAVORO/$nome.stderr"
}

# Aspetta che il dominio del worker esista e abbia un processo: e' il momento
# in cui un segnale da fuori colpisce un worker che lavora.
attendi_il_worker() {
    local i
    for i in $(seq 1 200); do
        local procs
        procs="$(find "$RADICE" -mindepth 1 -name cgroup.procs -exec cat {} + 2>/dev/null || true)"
        if [ -n "$procs" ]; then echo "$procs"; return 0; fi
        sleep 0.05
    done
    return 1
}

caso_sincrono() { # nome, riga, atteso, piano, ingresso, timeout, host
    local codice=0
    esegui "$1" "$4" "$5" "$6" "$7" || codice=$?
    giudica "$1" "$2" "$3" "$LAVORO/$1.stdout" "$codice" "$LAVORO/$1.out.arrow"
}

echo "== casi"

piano "$LAVORO/r1.json" $((512 * MiB)) $((256 * MiB)) "$FILTRO"
caso_sincrono successo "1" ok "$LAVORO/r1.json" "$LAVORO/piccolo.arrow" 60 $((1024 * MiB))

piano "$LAVORO/r2.json" $((1024 * MiB)) $((512 * MiB)) "$FILTRO"
caso_sincrono batch-oltre-il-tetto "2" resource_limit "$LAVORO/r2.json" "$LAVORO/batch-enorme.arrow" 60 $((2048 * MiB))

piano "$LAVORO/r5a.json" $((4 * MiB)) $((2 * MiB)) "$FILTRO"
caso_sincrono oom-prima-della-conduzione "5" resource_limit "$LAVORO/r5a.json" "$LAVORO/piccolo.arrow" 60 $((1024 * MiB))

piano "$LAVORO/r5b.json" "${TETTO_OOM_DURANTE:-$((96 * MiB))}" "${GOVERNATO_OOM_DURANTE:-$((80 * MiB))}" "$ORDINA"
caso_sincrono oom-durante-la-conduzione "5" resource_limit "$LAVORO/r5b.json" "$LAVORO/grande.arrow" 120 $((1024 * MiB))

piano "$LAVORO/r7.json" $((2048 * MiB)) $((1024 * MiB)) "$ORDINA"
caso_sincrono timeout "7" timeout "$LAVORO/r7.json" "$LAVORO/grande.arrow" 1 $((4096 * MiB))

# riga 6a: il worker ucciso da fuori
piano "$LAVORO/r6.json" $((2048 * MiB)) $((1024 * MiB)) "$ORDINA"
esegui ucciso-da-fuori "$LAVORO/r6.json" "$LAVORO/grande.arrow" 120 $((4096 * MiB)) &
pid_run=$!
if procs="$(attendi_il_worker)"; then
    for p in $procs; do kill -9 "$p" 2>/dev/null || true; done
fi
codice=0; wait "$pid_run" || codice=$?
giudica ucciso-da-fuori "6a" internal "$LAVORO/ucciso-da-fuori.stdout" "$codice" "$LAVORO/ucciso-da-fuori.out.arrow"

# riga 8: la cancellazione
esegui cancellato "$LAVORO/r6.json" "$LAVORO/grande.arrow" 120 $((4096 * MiB)) &
pid_run=$!
attendi_il_worker > /dev/null || true
kill -INT "$pid_run"
codice=0; wait "$pid_run" || codice=$?
giudica cancellato "8" cancelled "$LAVORO/cancellato.stdout" "$codice" "$LAVORO/cancellato.out.arrow"

echo
printf '%s\n' "${esiti[@]}"
echo
if [ "$guasti" -ne 0 ]; then
    echo "qualifica: $guasti casi ROSSI. stdout e stderr di ogni caso restano in $LAVORO per la diagnosi" >&2
    trap - EXIT
    find "$RADICE" -mindepth 1 -depth -type d -exec rmdir {} + 2>/dev/null || true
    rmdir "$RADICE" 2>/dev/null || true
    exit 1
fi
echo "qualifica: tutti i casi VERDI"
