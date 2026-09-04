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
# Se il processo `$1` porta ancora il marcatore `$2`.
#
# Un pid da solo non identifica niente: fra la morte di un processo e la riga
# che lo guarda il kernel puo' aver dato quel numero a qualcun altro, e agire su
# quel numero colpirebbe un estraneo. L'identita' si legge dagli **argomenti**,
# che il marcatore rende unici.
e_ancora_il_nostro() {
  local pid="$1" marcatore="$2" riga="/proc/$1/cmdline"
  [[ -r "$riga" ]] || return 1
  tr '\0' ' ' < "$riga" 2>/dev/null | grep -qF "$marcatore"
}

# Aspetta che `$1` sparisca, ricontrollando l'identita' a ogni giro.
#
# Rende 0 se il processo non c'e' piu' — o se quel pid non e' piu' il nostro,
# che e' la stessa cosa ai fini della sonda — e 1 se e' ancora li' allo scadere.
attendi_che_sparisca() {
  local pid="$1" marcatore="$2" giri="$3"
  local passato=0
  while [[ "$passato" -lt "$giri" ]]; do
    e_ancora_il_nostro "$pid" "$marcatore" || return 0
    sleep 0.2
    passato=$((passato + 1))
  done
  return 1
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
  if attendi_che_sparisca "$nipote" "$marcatore" 10; then
    echo "il timeout esterno elimina anche un nipote (misurato, pid $nipote)"
    return 0
  fi

  # Il nipote e' vivo, ed e' proprio il nostro: la rete esterna non copre il
  # gruppo. Lo si chiude — prima con garbo, poi per forza — e **si guarda ogni
  # volta di nuovo**: dichiarare chiuso cio' che si e' solo segnalato e' lo
  # stesso difetto che questa sonda esiste per trovare.
  kill -TERM "$nipote" 2>/dev/null || true
  if ! attendi_che_sparisca "$nipote" "$marcatore" 25; then
    if e_ancora_il_nostro "$nipote" "$marcatore"; then
      kill -KILL "$nipote" 2>/dev/null || true
    fi
    if ! attendi_che_sparisca "$nipote" "$marcatore" 25; then
      echo "PERSO: il nipote $nipote e' sopravvissuto al timeout e non si lascia chiudere: la macchina resta con un processo della sonda addosso" >&2
      return 1
    fi
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
