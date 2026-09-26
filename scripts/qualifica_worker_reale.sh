#!/usr/bin/env bash
# Qualifica end-to-end del worker reale: un'immagine di produzione percorre la
# sequenza intera su un canale vero.
#
# GRAMMATICA, ED E' ESATTA
#
#   scripts/qualifica_worker_reale.sh                 # qualificazione
#   scripts/qualifica_worker_reale.sh --iterazione    # iterazione, non qualifica
#
#   Nient'altro. Nessuna variabile d'ambiente cambia che cosa si qualifica, e
#   un argomento non riconosciuto e' un errore invece di ricadere nella
#   qualificazione: una riga di comando storta che qualifica comunque dichiara
#   di aver provato qualcosa che nessuno ha chiesto.
#
# CHE COSA QUALIFICA
#
#   Cablaggio, eredita' dei descrittori, handshake, incarico, progresso, esito,
#   artefatto **riverificato** con i passi da 3 a 8-bis, EOF e raccolta del
#   processo, con l'immagine distribuita — release, senza `internals`.
#
# CHE COSA NON QUALIFICA
#
#   «Sotto limite». Qui non c'e' ne' lo spawner ne' un dominio cgroup2: il
#   worker nasce dall'harness e vive con la memoria che il sistema gli concede.
#   Che esegua sotto `memory.max` e' un'altra affermazione, e la si fa sulla VM
#   attraversando spawner e dominio vero.
#
# PERCHE' SEMPRE RELEASE
#
#   Perche' «immagine di produzione» e' quella che si distribuisce, e un profilo
#   scelto da fuori produrrebbe un binario diverso con la stessa etichetta. Un
#   debug qualificato come produzione e' esattamente l'armatura che certifica
#   un'esecuzione diversa da quella dichiarata.
#
# PERCHE' DUE TARGET SEPARATI
#
#   Perche' una compilazione con `internals` non deve poter **sostituire** il
#   binario appena qualificato. Con un solo target, un `cargo build --features
#   internals` lanciato dopo riscriverebbe il file allo stesso percorso, e il
#   digest registrato parlerebbe di un binario che non c'e' piu'. Il controllo
#   finale se ne accorgerebbe; ma e' meglio che non possa accadere.
#
# PERCHE' IL DIGEST SI VERIFICA DUE VOLTE
#
#   Prima, per dire quale binario si sta qualificando; e dopo, per dire che e'
#   ancora quello. Il valore passa anche all'harness, che lo confronta con
#   l'immagine che ha davvero eseguito: cosi' lo script e il percorso parlano
#   dello stesso file, e non due volte di se stessi.
#
# PERCHE' UN GUARDIANO ANCHE QUI
#
#   L'harness ha i propri tetti sulle letture e sulla raccolta, ma sono suoi: un
#   harness che si inceppasse prima di installarli — o una compilazione che non
#   finisce — appenderebbe comunque la campagna. Il `timeout` di fuori e' la
#   rete sotto la rete, e trasforma un blocco in un rosso.

set -Eeuo pipefail

RADICE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$RADICE"
SEGNALA="$RADICE/scripts/segnala_verificato.py"
# L'eseguibile del nipote, risolto una volta: un percorso che non si risolve
# ferma la sonda qui, invece di far sembrare «un altro processo» ogni nipote.
SLEEP_REALE="$(readlink -f "$(command -v sleep)")" || SLEEP_REALE=""
if [[ -z "$SLEEP_REALE" ]]; then
  echo "PERSO: il percorso di sleep non si risolve: la sonda non puo' riconoscere il nipote" >&2
  exit 1
fi

# Questo percorso **rifiuta** root, e il rifiuto arriva prima di ogni build.
#
# PERCHE' NON BASTA LA CONVENZIONE
#
#   Perche' «questo script si lancia da utente» e' una frase, e una frase non
#   ferma un `sudo`. Le due cache qui sotto appartengono al percorso non
#   privilegiato; `cargo` scrive gli artefatti con l'utente che lo esegue, quindi
#   un solo giro fatto come root le riempie di file che l'utente non puo' piu'
#   toccare. Il comando successivo non fallisce dicendo «cache di root»: fallisce
#   con `Permission denied` su `.cargo-build-lock`, che somiglia a tutt'altro.
#
# PERCHE' PRIMA DI QUALUNQUE COMPILAZIONE
#
#   Perche' un rifiuto che arrivasse dopo avrebbe gia' contaminato cio' che deve
#   proteggere. Qui non si e' ancora creato niente.
#
#   I percorsi in cui root serve davvero — la qualificazione sotto limite e il
#   gate ostile — hanno cache proprie, dichiarate root-only.
if [[ "$(id -u)" -eq 0 ]]; then
  echo "questo percorso non richiede privilegi, e come root non deve girare:" >&2
  echo "  le due cache che usa — target-qualificazione e target-immagine-produzione —" >&2
  echo "  appartengono al percorso non privilegiato, e root le renderebbe" >&2
  echo "  inutilizzabili per l'utente che le trovera' dopo." >&2
  exit 2
fi

TARGET_PRODUZIONE="$RADICE/target-immagine-produzione"
TARGET_HARNESS="$RADICE/target-qualificazione"

# Quanto si concede al percorso, dall'esterno. Il tetto interno dell'harness e'
# di trenta secondi sulle letture: questo e' piu' largo perche' copre anche il
# suo avvio e la sua uscita, e serve solo a impedire un blocco.
TETTO_DEL_PERCORSO=180

MODO="qualificazione"
if [[ $# -eq 1 && "$1" == "--iterazione" ]]; then
  MODO="iterazione"
elif [[ $# -ne 0 ]]; then
  echo "riga di comando non ammessa: «$*»" >&2
  echo "  scripts/qualifica_worker_reale.sh                 # qualificazione" >&2
  echo "  scripts/qualifica_worker_reale.sh --iterazione    # non qualifica" >&2
  exit 2
fi

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "questo percorso esiste solo su Linux: l'esecuzione isolata non e' supportata altrove" >&2
  exit 2
fi

# --- il timeout uccide anche un nipote? -------------------------------------
#
# PERCHE' SI MISURA INVECE DI DARLO PER BUONO
#
#   Perche' e' una classe di perdita gia' incontrata: un processo che ne genera
#   un altro e muore lascia il nipote vivo, e il nipote tiene aperto l'estremo
#   della pipe che nessuno chiudera' piu'. GNU `timeout` senza `--foreground`
#   mette il comando in un **gruppo suo** e segnala il gruppo, quindi il nipote
#   dovrebbe morire con lui — ma «dovrebbe» non e' una misura, e su
#   un'implementazione diversa di `timeout`, o con un `--foreground` aggiunto un
#   domani, la rete si aprirebbe senza che niente lo dica.
#
#   Il nipote e' un `sleep` con una durata che nessun percorso ragionevole
#   raggiunge: se sopravvive, lo si trova.
# Se il processo `$1` e' ancora il nostro: l'eseguibile di `sleep` e il
# marcatore `$2` come argomento esatto.
#
# Un pid da solo non identifica niente: fra la morte di un processo e la riga
# che lo guarda il kernel puo' aver dato quel numero a qualcun altro, e agire su
# quel numero colpirebbe un estraneo. L'identita' si legge dall'eseguibile e
# dagli **argomenti**, che il marcatore rende unici.
#
# Rende 0 se e' ancora il nostro, 1 se non c'e' piu' o e' un altro, e 2 se non
# si lascia osservare: un processo illeggibile non e' un processo sparito, e
# solo «non esiste» e' assenza.
e_ancora_il_nostro() {
  local pid="$1" marcatore="$2" eseguibile grezzo argomento
  esiste_il_pid "$pid" || return $?
  if ! eseguibile="$(readlink "/proc/$pid/exe" 2>/dev/null)"; then
    esiste_il_pid "$pid" || return $?
    return 2
  fi
  [[ "$eseguibile" == "$SLEEP_REALE" ]] || return 1
  # `cat` e non `read`: `read` non distingue la fine del file da un errore di
  # lettura, `cat` fallisce. I NUL diventano \001, perche' una sostituzione di
  # comando non li porta; un argomento che contenesse gia' \001 si spezzerebbe,
  # e al piu' farebbe dire «ancora nostro», il verso prudente.
  if ! grezzo="$(set -o pipefail; cat "/proc/$pid/cmdline" 2>/dev/null | tr '\0' '\001')"; then
    esiste_il_pid "$pid" || return $?
    return 2
  fi
  while [[ -n "$grezzo" ]]; do
    argomento="${grezzo%%$'\001'*}"
    [[ "$argomento" == "$marcatore" ]] && return 0
    if [[ "$grezzo" == *$'\001'* ]]; then
      grezzo="${grezzo#*$'\001'}"
    else
      grezzo=""
    fi
  done
  return 1
}

# Se `/proc/$1` esiste: 0 si', 1 no, 2 non si sa. Solo «non esiste» e' 1.
esiste_il_pid() {
  local errore
  errore="$(LC_ALL=C stat -c %i "/proc/$1" 2>&1 >/dev/null)" && return 0
  [[ "$errore" == *"No such file or directory"* ]] && return 1
  return 2
}

# Aspetta che `$1` sparisca, ricontrollando l'identita' a ogni giro.
#
# Rende 0 se il processo non c'e' piu' — o se quel pid non e' piu' il nostro,
# che e' la stessa cosa ai fini della sonda — 1 se e' ancora li' allo scadere,
# e 2 se allo scadere non si lascia osservare.
attendi_che_sparisca() {
  local pid="$1" marcatore="$2" giri="$3"
  local passato=0
  local stato=0
  while [[ "$passato" -lt "$giri" ]]; do
    stato=0
    e_ancora_il_nostro "$pid" "$marcatore" || stato=$?
    [[ "$stato" -eq 1 ]] && return 0
    sleep 0.2
    passato=$((passato + 1))
  done
  [[ "$stato" -eq 2 ]] && return 2
  return 1
}

# Il nipote che non si lascia osservare non e' ne' vivo ne' sparito: la sonda
# lo dice e perde.
non_osservabile() {
  echo "PERSO: il nipote $1 non si lascia osservare in /proc: non si puo' dire se sia sparito" >&2
}

verifica_il_nipote() {
  # Un marcatore che nessun altro processo puo' avere: entra negli **argomenti**
  # del nipote, non in un file, cosi' l'identita' si legge da `/proc` e non da
  # cio' che qualcuno ha scritto. `sleep` accetta i decimali, quindi il
  # marcatore e' anche una durata valida.
  local marcatore="600.$$$(date +%N)"
  local dove
  dove="$(mktemp -t plenora-nipote.XXXXXXXXXX)" || {
    echo "PERSO: non si crea il file della sonda" >&2
    return 1
  }
  # Il file si toglie in ogni caso, anche se la sonda esce a meta'.
  trap 'rm -f "$dove"' RETURN

  # Il figlio genera il nipote e poi aspetta: `timeout` uccidera' il figlio, e
  # cio' che si misura e' che cosa succede al nipote.
  timeout --signal=KILL 1 sh -c "sleep $marcatore & echo \$! >&2; sleep 600" \
    2>"$dove" || true

  local nipote
  nipote="$(tr -dc '0-9' < "$dove")"
  if [[ -z "$nipote" ]]; then
    echo "PERSO: la sonda non ha prodotto un pid: non si puo' dire niente" >&2
    return 1
  fi

  # Un attimo perche' il segnale arrivi a tutto il gruppo, ricontrollando: se
  # sparisce prima, non si aspetta inutilmente.
  local esito=0
  attendi_che_sparisca "$nipote" "$marcatore" 10 || esito=$?
  if [[ "$esito" -eq 0 ]]; then
    echo "il timeout esterno elimina anche un nipote (misurato, pid $nipote)"
    return 0
  fi
  local primo="$esito"

  # Il nipote e' vivo, ed e' proprio il nostro: la rete esterna non copre il
  # gruppo. Lo si chiude — prima con garbo, poi per forza — e **si guarda ogni
  # volta di nuovo**: dichiarare chiuso cio' che si e' solo segnalato e' lo
  # stesso difetto che questa sonda esiste per trovare.
  # Il segnale passa da un pidfd aperto prima di verificare il marcatore: fra
  # la verifica e il segnale il PID non puo' cambiare processo.
  python3 "$SEGNALA" "$nipote" TERM --eseguibile "$SLEEP_REALE" \
    --argomento "$marcatore" || true
  esito=0
  attendi_che_sparisca "$nipote" "$marcatore" 25 || esito=$?
  if [[ "$esito" -ne 0 ]]; then
    python3 "$SEGNALA" "$nipote" KILL --eseguibile "$SLEEP_REALE" \
      --argomento "$marcatore" || true
    esito=0
    attendi_che_sparisca "$nipote" "$marcatore" 25 || esito=$?
    if [[ "$esito" -eq 2 ]]; then
      non_osservabile "$nipote"
      return 1
    fi
    if [[ "$esito" -ne 0 ]]; then
      echo "PERSO: il nipote $nipote e' sopravvissuto al timeout e non si lascia chiudere: la macchina resta con un processo della sonda addosso" >&2
      return 1
    fi
  fi
  # Chiuso da noi: che cosa si dice dipende dalla prima attesa.
  if [[ "$primo" -eq 2 ]]; then
    non_osservabile "$nipote"
    return 1
  fi
  echo "PERSO: il nipote $nipote e' sopravvissuto al timeout: la rete esterna non copre il gruppo, e un worker che genera un processo puo' restare vivo con la pipe aperta" >&2
  return 1
}

echo "== rete esterna =="
verifica_il_nipote

# --- l'harness, con internals, in un target suo -----------------------------
#
# Si compila per primo in entrambi i modi: se non compila, non c'e' niente da
# qualificare e non ha senso spendere una build di release.
echo "== harness (internals, target separato) =="
CARGO_TARGET_DIR="$TARGET_HARNESS" cargo build \
  -p plenora-engine --features internals --bin plenora-qualifica-worker --locked
HARNESS="$TARGET_HARNESS/debug/plenora-qualifica-worker"

if [[ "$MODO" == "iterazione" ]]; then
  # L'immagine dell'iterazione sta nel target condiviso e nessun digest la
  # fissa: e' la stessa che si ricompila venti volte al giorno, e proprio per
  # questo non qualifica.
  echo "== immagine di iterazione (target condiviso, non fissata) =="
  CARGO_TARGET_DIR="$TARGET_HARNESS" cargo build -p plenora-cli --locked
  IMMAGINE="$(readlink -f "$TARGET_HARNESS/debug/plenora-data-tools")"
  exec timeout --signal=KILL "$TETTO_DEL_PERCORSO" \
    "$HARNESS" --immagine "$IMMAGINE" --etichetta iterazione
fi

# --- l'immagine di produzione, senza internals, in un altro target ----------
echo "== immagine di produzione (release, senza internals) =="
CARGO_TARGET_DIR="$TARGET_PRODUZIONE" cargo build -p plenora-cli --release --locked
IMMAGINE="$TARGET_PRODUZIONE/release/plenora-data-tools"

if [[ ! -x "$IMMAGINE" ]]; then
  echo "l'immagine di produzione non c'e': $IMMAGINE" >&2
  exit 1
fi
IMMAGINE="$(readlink -f "$IMMAGINE")"

# --- il digest, prima ------------------------------------------------------
PRIMA="$(sha256sum "$IMMAGINE" | cut -d' ' -f1)"
echo "immagine:  $IMMAGINE"
echo "digest:    $PRIMA"

# --- il percorso -----------------------------------------------------------
#
# Il codice d'uscita dell'harness non si perde: `set -e` lo lascerebbe passare
# dentro un `if`, quindi lo si cattura e si decide dopo aver riverificato il
# digest — un binario sostituito durante il percorso e' un difetto anche se il
# percorso e' andato bene.
echo "== percorso =="
ESITO=0
timeout --signal=KILL "$TETTO_DEL_PERCORSO" \
  "$HARNESS" --immagine "$IMMAGINE" --etichetta produzione --digest "$PRIMA" || ESITO=$?
if [[ "$ESITO" -eq 137 ]]; then
  echo "PERSO: il percorso non e' finito entro $TETTO_DEL_PERCORSO secondi ed e' stato ucciso" >&2
fi

# --- il digest, dopo -------------------------------------------------------
DOPO="$(sha256sum "$IMMAGINE" | cut -d' ' -f1)"
if [[ "$PRIMA" != "$DOPO" ]]; then
  echo "PERSO: l'immagine e' cambiata durante il percorso" >&2
  echo "  prima: $PRIMA" >&2
  echo "  dopo:  $DOPO" >&2
  exit 1
fi

if [[ "$ESITO" -ne 0 ]]; then
  echo "PERSO: il percorso non ha qualificato l'immagine (uscita $ESITO)" >&2
  exit "$ESITO"
fi

echo "VINTO: l'immagine di produzione ha percorso la sequenza"
echo "  digest verificato prima e dopo: $PRIMA"
echo "  questo NON prova «sotto limite»: spawner e dominio cgroup restano da attraversare sulla VM"
