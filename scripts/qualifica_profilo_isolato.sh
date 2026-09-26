#!/usr/bin/env bash
# Qualifica del profilo isolato sul percorso di **produzione**: il binario
# distribuito, `plenora-data-tools run`, con un dominio cgroup2 reale e il
# worker con un'identita' distinta. Un caso per riga della matrice §10 di
# docs/isolamento.md che si possa provocare da fuori, e l'envelope su stdout
# come oracolo: categoria **e** messaggio, perche' la categoria sola non dice
# quale riga ha prodotto l'esito.
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
#   riga 2   errore tipizzato del worker: un `table.type_cast` con
#            `errors: "raise"` su testo non numerico, che fallisce nel worker
#            mentre esegue
#   riga 5   OOM attribuito PRIMA della conduzione: un dominio troppo piccolo
#            perche' il worker arrivi a lavorare
#   riga 5   OOM attribuito DURANTE la conduzione, con il controllo positivo:
#            lo stesso carico con un tetto largo completa
#   riga 6a  il worker ucciso da fuori (SIGKILL) mentre lavora; da fuori non
#            si distingue dal crash della riga 4, e la categoria e' la stessa
#   riga 7   timeout di esecuzione
#   riga 8   cancellazione (SIGINT al processo di `run`) mentre il worker lavora
#
#   Le righe 3, 6b, 9, 10 e da 11 a 16 non hanno un innesco da fuori con il
#   binario di produzione: le coprono i test, e questo script non finge di
#   provarle.
#
# CHE COSA NON E'
#
#   Non e' una prova di prestazioni. I tetti dei casi di OOM e l'attesa prima
#   dei segnali sono tarati sul worker di questa build e su questa macchina:
#   altrove possono servire valori diversi (variabili sotto), e il caso lo dice
#   con un verdetto rosso invece di tacere.
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
#   CONSERVA_LAVORO=1 tiene stdout e stderr dei casi anche se tutto e' verde.

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

# La radice delegata e la directory di lavoro sono di questa esecuzione.
RADICE="/sys/fs/cgroup/plenora-qualifica-$$"
LAVORO="$(mktemp -d /tmp/plenora-qualifica.XXXXXX)"
chmod 0755 "$LAVORO"
IN_CORSO=()

# Termina un dominio e aspetta che si svuoti: `cgroup.kill` e poi
# `populated 0`, entro cinque secondi. Rende non zero se non ci riesce.
svuota() {
    local dominio="$1" i
    [ -e "$dominio/cgroup.kill" ] && echo 1 > "$dominio/cgroup.kill" 2>/dev/null || true
    for i in $(seq 1 100); do
        grep -q '^populated 0$' "$dominio/cgroup.events" 2>/dev/null && return 0
        sleep 0.05
    done
    return 1
}

# Solo sotto la radice di questa campagna: non tocca altro.
pulisci() {
    local codice=$? residuo=0 pid dominio
    for pid in "${IN_CORSO[@]:-}"; do
        [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    if [ -d "$RADICE" ]; then
        while IFS= read -r dominio; do
            svuota "$dominio" || { echo "qualifica: dominio abitato $dominio" >&2; residuo=1; }
            rmdir "$dominio" 2>/dev/null || { echo "qualifica: RESIDUO $dominio" >&2; residuo=1; }
        done < <(find "$RADICE" -mindepth 1 -maxdepth 1 -type d)
        rmdir "$RADICE" 2>/dev/null || { echo "qualifica: RESIDUO $RADICE" >&2; residuo=1; }
    fi
    if [ "$residuo" != 0 ] && [ "$codice" = 0 ]; then codice=3; fi
    if [ "$codice" = 0 ] && [ -z "${CONSERVA_LAVORO:-}" ]; then
        rm -rf "$LAVORO"
    else
        echo "qualifica: uscita $codice, i log dei casi restano in $LAVORO" >&2
    fi
    exit "$codice"
}
trap pulisci EXIT
mkdir "$RADICE"
echo "+memory +pids" > "$RADICE/cgroup.subtree_control"

echo "== ambiente"
echo "kernel      $(uname -r)"
echo "binario     $(sha256sum "$BINARIO" | cut -d' ' -f1)"
echo "radice      $RADICE"
echo "worker      $WORKER"

# Il binario si installa come in un dispiegamento: di root, 0755. Il preflight
# rifiuta uno spawner che il worker potrebbe riscrivere, e un binario lasciato
# dove l'ha scritto la build — di un utente, scrivibile dal gruppo — cade li'.
install -o root -g root -m 0755 "$BINARIO" "$LAVORO/plenora-data-tools"
BINARIO="$LAVORO/plenora-data-tools"

# --- gli ingressi -----------------------------------------------------------
"$GENERATORE" "$LAVORO/piccolo.arrow" 1000 1000
# Molti batch piccoli: il lavoro cresce con le righe, non col singolo batch,
# e nessun batch si avvicina al tetto per batch dell'executor.
"$GENERATORE" "$LAVORO/grande.arrow" "${RIGHE_GRANDE:-6000000}" 65536
# Per l'ordinamento: la concatenazione deve restare sotto il tetto per batch,
# cosi' con un tetto largo il carico completa.
"$GENERATORE" "$LAVORO/medio.arrow" "${RIGHE_MEDIO:-1500000}" 65536
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
CAST_CHE_FALLISCE='[{"id": "fine", "op": "table.type_cast", "in": ["righe"],
          "config": {"column": "nome", "target_type": "int", "errors": "raise"}}]'
# Lungo e in streaming: una sostituzione per regex su ogni cifra di ogni riga.
LUNGO='[{"id": "fine", "op": "table.replace", "in": ["righe"],
         "config": {"column": "nome", "old_value": "[0-9]", "new_value": "x", "regex": true}}]'
ORDINA='[{"id": "fine", "op": "table.sort", "in": ["righe"],
          "config": {"columns": ["nome"], "ascending": false}}]'
MiB=$((1024 * 1024))

esiti=()
guasti=0

# Legge l'envelope e confronta. Argomenti: nome, riga, categoria attesa ("ok"
# per il successo), frase che il messaggio deve contenere, frase che non deve
# contenere (vuote = nessun vincolo), codice d'uscita, e un motivo che rende il
# caso non valido a prescindere dall'esito (segnale non inviato).
giudica() {
    local nome="$1" riga="$2" atteso="$3" deve="$4" non_deve="$5" codice="$6" invalido="${7:-}"
    local uscita="$LAVORO/$nome.stdout" output="$LAVORO/$nome.out.arrow"
    local lettura ottenuto messaggio
    lettura="$(python3 - "$uscita" <<'EOF'
import json, sys
try:
    documento = json.load(open(sys.argv[1]))
except Exception:
    print("envelope-non-json\t")
    sys.exit(0)
if documento.get("status") == "ok":
    pulizia = documento.get("temp_cleanup", {}).get("state")
    print(("ok" if pulizia == "removed" else "ok-residuo-" + str(pulizia)) + "\t")
else:
    errore = documento.get("error", {})
    print(errore.get("category", "senza-categoria") + "\t" + errore.get("message", "").replace("\n", " "))
EOF
)"
    ottenuto="${lettura%%$'\t'*}"
    messaggio="${lettura#*$'\t'}"
    local residui verdetto="VERDE" perche=""
    residui="$(find "$RADICE" -mindepth 1 -type d | wc -l)"
    if [ "$ottenuto" != "$atteso" ]; then verdetto="ROSSO"; perche="categoria"; fi
    if [ -n "$deve" ] && [[ "$messaggio" != *"$deve"* ]]; then verdetto="ROSSO"; perche="$perche messaggio"; fi
    if [ -n "$non_deve" ] && [[ "$messaggio" == *"$non_deve"* ]]; then verdetto="ROSSO"; perche="$perche messaggio"; fi
    if [ "$atteso" = ok ] && [ ! -s "$output" ]; then verdetto="ROSSO"; perche="$perche output-assente"; fi
    if [ "$atteso" != ok ] && [ -e "$output" ]; then verdetto="ROSSO"; perche="$perche output-presente"; fi
    if [ "$atteso" != ok ] && [ "$codice" = 0 ]; then verdetto="ROSSO"; perche="$perche exit-zero"; fi
    if [ "$atteso" = ok ] && [ "$codice" != 0 ]; then verdetto="ROSSO"; perche="$perche exit-non-zero"; fi
    if [ "$residui" != 0 ]; then verdetto="ROSSO"; perche="$perche domini-residui"; fi
    if [ -n "$invalido" ]; then verdetto="ROSSO"; perche="$perche $invalido"; fi
    [ "$verdetto" = VERDE ] || guasti=$((guasti + 1))
    esiti+=("$(printf '%-6s %-32s atteso=%-15s ottenuto=%-22s exit=%-4s %s %s' \
        "$riga" "$nome" "$atteso" "$ottenuto" "$codice" "$verdetto" "$perche")")
}

# Esegue `run` con l'ambiente del dispiegamento e nient'altro. `exec`: il
# processo e' la CLI stessa, cosi' il PID di un caso asincrono e' il suo.
# Argomenti: nome, piano, ingresso, timeout in secondi, tetto dell'host.
esegui() {
    local nome="$1" piano_file="$2" ingresso="$3" tempo="$4" host="$5"
    exec env -i PATH=/usr/bin:/bin \
        PLENORA_ISOLATION_HOST_MAX_MEMORY_BYTES="$host" \
        PLENORA_ISOLATION_CGROUP_ROOT="$RADICE" \
        PLENORA_ISOLATION_WORKER_UIDGID="$WORKER" \
        PLENORA_ISOLATION_EXECUTION_TIMEOUT_SECONDS="$tempo" \
        "$BINARIO" run --plan "$piano_file" --input "righe=$ingresso" \
        --output "$LAVORO/$nome.out.arrow" \
        > "$LAVORO/$nome.stdout" 2> "$LAVORO/$nome.stderr"
}

caso_sincrono() { # nome, riga, atteso, deve, non_deve, piano, ingresso, timeout, host
    local codice=0
    ( esegui "$1" "$6" "$7" "$8" "$9" ) || codice=$?
    giudica "$1" "$2" "$3" "$4" "$5" "$codice"
}

# Aspetta il dominio del **worker** (`plenora-isolato-*`; quello del
# verificatore e' `plenora-verifica-*`) con un processo dentro, poi il tempo
# perche' lavori, e rende quel dominio se e' ancora abitato. Niente, se il
# worker non e' mai comparso o ha gia' finito: il caso allora non prova la
# riga, e lo dice invece di colpire un altro processo.
worker_al_lavoro() {
    local i dominio
    for i in $(seq 1 200); do
        # Il contenuto, non la dimensione: i file di cgroupfs dichiarano 0 byte.
        for dominio in "$RADICE"/plenora-isolato-*; do
            if [ -n "$(cat "$dominio/cgroup.procs" 2>/dev/null)" ]; then
                sleep "${ATTESA_PRIMA_DEL_SEGNALE:-0.5}"
                [ -n "$(cat "$dominio/cgroup.procs" 2>/dev/null)" ] && echo "$dominio"
                return 0
            fi
        done
        sleep 0.05
    done
    return 0
}

# Toglie un'esecuzione gia' attesa dall'elenco che la pulizia ferma: dopo
# `wait` il suo PID puo' tornare in uso, e un segnale colpirebbe un estraneo.
togli_da_in_corso() {
    local pid="$1" resto=() p
    for p in "${IN_CORSO[@]:-}"; do [ -n "$p" ] && [ "$p" != "$pid" ] && resto+=("$p"); done
    IN_CORSO=("${resto[@]:-}")
}

echo "== casi"

piano "$LAVORO/r1.json" $((512 * MiB)) $((256 * MiB)) "$FILTRO"
caso_sincrono successo "1" ok "" "" "$LAVORO/r1.json" "$LAVORO/piccolo.arrow" 60 $((1024 * MiB))

piano "$LAVORO/r2.json" $((512 * MiB)) $((256 * MiB)) "$CAST_CHE_FALLISCE"
caso_sincrono errore-del-worker "2" "${CATEGORIA_RIGA_2:-data_mapping}" "" "" "$LAVORO/r2.json" "$LAVORO/piccolo.arrow" 60 $((1024 * MiB))
# L'errore e' quello dichiarato dal worker, non uno del coordinatore.
if ! grep -q "con errore dichiarato" "$LAVORO/errore-del-worker.stderr"; then
    esiti[-1]="${esiti[-1]} ROSSO(errore-non-dichiarato-dal-worker)"
    guasti=$((guasti + 1))
fi

piano "$LAVORO/r5a.json" "${TETTO_OOM_PRIMA:-$((1 * MiB))}" "${GOVERNATO_OOM_PRIMA:-$((512 * 1024))}" "$FILTRO"
caso_sincrono oom-prima-della-conduzione "5" resource_limit "prima della conduzione" "" \
    "$LAVORO/r5a.json" "$LAVORO/piccolo.arrow" 60 $((1024 * MiB))

# Il controllo positivo: lo stesso carico, con un tetto largo, completa. Senza
# questo, l'OOM sotto potrebbe coprire un carico che fallirebbe comunque.
piano "$LAVORO/r5c.json" $((2048 * MiB)) $((1024 * MiB)) "$ORDINA"
caso_sincrono controllo-del-carico-oom "5c" ok "" "" "$LAVORO/r5c.json" "$LAVORO/medio.arrow" 120 $((4096 * MiB))

piano "$LAVORO/r5b.json" "${TETTO_OOM_DURANTE:-$((64 * MiB))}" "${GOVERNATO_OOM_DURANTE:-$((48 * MiB))}" "$ORDINA"
caso_sincrono oom-durante-la-conduzione "5" resource_limit "ha raggiunto il proprio tetto" "prima della conduzione" \
    "$LAVORO/r5b.json" "$LAVORO/medio.arrow" 120 $((1024 * MiB))

piano "$LAVORO/r7.json" $((2048 * MiB)) $((1024 * MiB)) "$LUNGO"
caso_sincrono timeout "7" timeout "non ha concluso entro" "" "$LAVORO/r7.json" "$LAVORO/grande.arrow" 1 $((4096 * MiB))

piano "$LAVORO/r6.json" $((2048 * MiB)) $((1024 * MiB)) "$LUNGO"

# riga 6a: il worker ucciso da fuori mentre lavora
( esegui ucciso-da-fuori "$LAVORO/r6.json" "$LAVORO/grande.arrow" 120 $((4096 * MiB)) ) &
pid_run=$!; IN_CORSO+=("$pid_run")
invalido=""
dominio="$(worker_al_lavoro)"
if [ -z "$dominio" ]; then
    invalido="worker-non-al-lavoro"
else
    colpiti=0
    for p in $(cat "$dominio/cgroup.procs" 2>/dev/null); do
        kill -9 "$p" 2>/dev/null && colpiti=$((colpiti + 1))
    done
    [ "$colpiti" -gt 0 ] || invalido="segnale-non-inviato"
fi
codice=0; wait "$pid_run" || codice=$?
togli_da_in_corso "$pid_run"
giudica ucciso-da-fuori "6a" internal "il worker isolato e' terminato in modo ambiguo" "" "$codice" "$invalido"

# riga 8: la cancellazione mentre il worker lavora. Il segnale va alla CLI,
# che e' il processo stesso del caso grazie all'`exec` di `esegui`.
( esegui cancellato "$LAVORO/r6.json" "$LAVORO/grande.arrow" 120 $((4096 * MiB)) ) &
pid_run=$!; IN_CORSO+=("$pid_run")
invalido=""
dominio="$(worker_al_lavoro)"
if [ -z "$dominio" ]; then
    invalido="worker-non-al-lavoro"
elif ! kill -INT "$pid_run" 2>/dev/null; then
    invalido="segnale-non-inviato"
fi
codice=0; wait "$pid_run" || codice=$?
togli_da_in_corso "$pid_run"
giudica cancellato "8" cancelled "l'esecuzione isolata del worker e' stata cancellata" "" "$codice" "$invalido"

echo
printf '%s\n' "${esiti[@]}"
echo
if [ "$guasti" -ne 0 ]; then
    echo "qualifica: $guasti casi ROSSI" >&2
    exit 1
fi
echo "qualifica: tutti i casi VERDI"
