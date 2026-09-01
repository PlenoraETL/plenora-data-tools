#!/usr/bin/env bash
#
# Il gate ostile del dominio di isolamento (F4-15).
#
# Prova le tre cose che i casi deterministici non possono provare, perche' non
# riguardano la procedura ma l'ambiente:
#
#   1. la sentinella sul dispatch: l'immagine rieseguita arriva in modalita'
#      spawner con **un task solo**;
#   2. l'immagine sostituita: il binario sostitutivo non parte mai, e i due
#      esiti ammessi si distinguono con una barriera esplicita;
#   3. la separazione di privilegio: il worker spogliato non riscrive i quattro
#      controlli, non esce dal dominio, e dopo una `unshare` lascia lo stato
#      **identico** a quello di partenza.
#
# # Perche' ogni braccio ostile ha una controprova privilegiata
#
# Perche' «il worker non ci riesce» e' vero anche quando il bersaglio non
# esiste, e' di sola lettura per tutti, o il gate lo ha scritto male. Senza una
# controprova, un gate rotto e' indistinguibile da un isolamento che regge — ed
# e' verde.
#
# La controprova non riscrive il valore che c'e' gia': quella non proverebbe
# niente, perche' una scrittura che non cambia nulla puo' essere accettata, o
# scartata dal kernel, e in entrambi i casi la rilettura torna uguale per la
# ragione sbagliata. Scrive un valore **diverso**, lo rilegge, rimette
# l'originale e **rilegge di nuovo**. Quattro passi, e nessuno dei quattro puo'
# fallire in silenzio: senza il ripristino verificato, una corsa lascerebbe il
# dominio con un valore che non e' quello dichiarato, e la corsa dopo
# misurerebbe altro.
#
# # Perche' «stato invariato» si confronta invece di dedurlo
#
# Un tentativo che non dice `RIUSCITO` non e' un tentativo che non ha cambiato
# niente: una scrittura puo' essere rifiutata dopo aver troncato il file, e un
# rifiuto che arriva a meta' lascia un valore diverso da quello di prima. Il
# worker riporta `prima` e `dopo` per ogni bersaglio, e il gate li
# **confronta**, uno per uno, anche dopo la `unshare`.
#
# # Perche' nessuna attesa passa dall'orologio
#
# I due bracci sulla sostituzione hanno bisogno di sapere **dove** si trova il
# supervisore quando il gate rinomina il binario. Un `sleep` non lo sa: un esito
# giusto sarebbe indistinguibile da un esito fortunato. Le fifo sono
# appuntamenti — aprirne una blocca finche' non arriva l'altro — e i timeout
# servono solo a non restare appesi per sempre, mai a sincronizzare.
#
# # Il verde autoritativo
#
# Arriva **solo** da una VM Linux dedicata con cgroup v2 e sottoalbero delegato.
# Un container privilegiato serve a iterare: condivide il kernel con l'host e
# non prova niente su F4-15.
#
# # Uso
#
#   sudo scripts/verifica_isolamento_linux.sh <directory-evidenza-nuova>
#
# Variabili: PROFILO (release|dev), RADICE (un nome, non un percorso),
# WORKER_UID, WORKER_GID, TETTO_BYTE, ATTESA_MASSIMA.

set -euo pipefail

PROFILO="${PROFILO:-release}"
WORKER_UID="${WORKER_UID:-65534}"
WORKER_GID="${WORKER_GID:-65534}"
TETTO_BYTE="${TETTO_BYTE:-67108864}"
ATTESA_MASSIMA="${ATTESA_MASSIMA:-120}"
DOVE="${1:-$PWD/evidenza-isolamento}"
RADICE_RELATIVA="${RADICE:-plenora-qualificazione}"

PUNTO=""
TEMPORANEA=""
RADICE_ASSOLUTA=""
DOMINIO=""
VICINO=""
MEMORY_ABILITATO_DA_NOI=0
CONFRONTATI=0
FALLIMENTI=0

# --- pulizia, segnali e verdetto ----------------------------------------------
#
# Tre responsabilita' distinte, e la distinzione non e' stilistica.
#
# `pulisci` **verifica cio' che rimuove**. Un `rmdir` che fallisce lascia un
# cgroup con un tetto e un sigillo che nessuno rimuovera', e la corsa successiva
# lo troverebbe e si fermerebbe; peggio, un `-memory` non riuscito lascia la
# macchina diversa da come il gate l'ha trovata. Un residuo e' quindi un
# **braccio rosso**, non un avviso.
#
# `verdetto` corre su `EXIT`, chiama `pulisci` e **solo allora** decide. E' il
# motivo per cui il verdetto non si stampa nel corpo: stampato li', direbbe
# «qualificato» prima che la pulizia abbia avuto occasione di fallire.
#
# La regola su cui poggia: la pulizia puo' rendere un verde **rosso**, mai un
# rosso verde. Se il corpo e' gia' uscito non-zero, quel codice si conserva.
#
# I segnali hanno un gestore proprio che esce con `128 + n`. Senza, un `INT`
# arriverebbe al gestore `EXIT` con lo stato dell'ultimo comando riuscito —
# cioe' zero — e un gate interrotto a meta' si leggerebbe come un gate passato.
pulisci() {
  set +e
  if [ -n "$DOMINIO" ] && [ -d "$DOMINIO" ]; then
    [ -e "$DOMINIO/cgroup.kill" ] && echo 1 >"$DOMINIO/cgroup.kill" 2>/dev/null
    local _giro=0
    while [ -s "$DOMINIO/cgroup.procs" ] && [ "$_giro" -lt 50 ]; do
      local _pid
      while read -r _pid; do
        [ -n "$_pid" ] && kill -9 "$_pid" 2>/dev/null
      done <"$DOMINIO/cgroup.procs"
      _giro=$((_giro + 1))
      sleep 0.1
    done
    rmdir "$DOMINIO" 2>/dev/null
    [ -d "$DOMINIO" ] && fallisce "pulizia: $DOMINIO non si rimuove"
  fi
  if [ -n "$VICINO" ] && [ -d "$VICINO" ]; then
    [ -e "$VICINO/cgroup.kill" ] && echo 1 >"$VICINO/cgroup.kill" 2>/dev/null
    rmdir "$VICINO" 2>/dev/null
    [ -d "$VICINO" ] && fallisce "pulizia: $VICINO non si rimuove"
  fi
  if [ -n "$RADICE_ASSOLUTA" ] && [ -d "$RADICE_ASSOLUTA" ]; then
    rmdir "$RADICE_ASSOLUTA" 2>/dev/null
    [ -d "$RADICE_ASSOLUTA" ] && fallisce "pulizia: $RADICE_ASSOLUTA non si rimuove"
  fi
  # La delega globale si rimette **solo** se e' stata accesa qui, e la rimozione
  # si rilegge: lasciare acceso un controllore che il gate ha acceso cambia la
  # macchina per chiunque venga dopo, e lo fa senza dirlo.
  if [ "$MEMORY_ABILITATO_DA_NOI" = "1" ] && [ -n "$PUNTO" ]; then
    echo "-memory" >"$PUNTO/cgroup.subtree_control" 2>/dev/null
    if grep -qw memory "$PUNTO/cgroup.subtree_control" 2>/dev/null; then
      fallisce "pulizia: memory resta abilitato in $PUNTO/cgroup.subtree_control"
    fi
  fi
  if [ -n "$TEMPORANEA" ] && [ -d "$TEMPORANEA" ]; then
    rm -rf "$TEMPORANEA" 2>/dev/null
    [ -d "$TEMPORANEA" ] && fallisce "pulizia: $TEMPORANEA non si rimuove"
  fi
  set -e
}

verdetto() {
  local _stato=$?
  pulisci
  if [ -d "$DOVE" ]; then
    {
      printf 'bracci_rossi=%s\n' "$FALLIMENTI"
      printf 'stato_del_corpo=%s\n' "$_stato"
      printf 'evidenza=%s\n' "$DOVE"
    } >"$DOVE/esito.txt" 2>/dev/null
  fi
  if [ "$_stato" -ne 0 ]; then
    printf 'ISOLAMENTO NON QUALIFICATO: uscita %s, %s bracci rossi. Evidenza in %s\n' \
      "$_stato" "$FALLIMENTI" "$DOVE" >&2
    exit "$_stato"
  fi
  if [ "$FALLIMENTI" -gt 0 ]; then
    printf 'ISOLAMENTO NON QUALIFICATO: %s bracci rossi. Evidenza in %s\n' \
      "$FALLIMENTI" "$DOVE" >&2
    exit 1
  fi
  printf 'ISOLAMENTO QUALIFICATO su %s. Evidenza in %s\n' "$(uname -r)" "$DOVE"
  exit 0
}

al_segnale() {
  printf 'INTERROTTO da %s\n' "$1" >&2
  exit "$2"
}

trap 'al_segnale SIGINT 130' INT
trap 'al_segnale SIGTERM 143' TERM
trap verdetto EXIT

manca() {
  printf 'PREREQUISITO ASSENTE: %s\n' "$1" >&2
  exit 2
}

fallisce() {
  printf 'BRACCIO ROSSO: %s\n' "$1" >&2
  FALLIMENTI=$((FALLIMENTI + 1))
}

nota() {
  printf '%s\n' "$1"
}

# Il valore di una chiave nell'evidenza del helper.
#
# La chiave dev'essere **unica**: prendere l'ultima occorrenza sceglierebbe
# secondo l'ordine delle righe, che qui non ha quel significato. Due righe con
# la stessa chiave vogliono dire che il programma ha riportato due volte cose
# diverse, e leggerne una sola nasconde l'altra — che e' esattamente il modo in
# cui un rapporto contraddittorio si legge come coerente.
valore() {
  local _quante
  _quante="$(grep -c "^QI $2=" "$1" 2>/dev/null || true)"
  if [ "$_quante" != "1" ]; then
    fallisce "evidenza: la chiave «$2» compare $_quante volte in $(basename "$1"), attesa una sola"
    printf '<<chiave non unica>>'
    return
  fi
  sed -n "s/^QI $2=//p" "$1"
}

inode_di() {
  stat -c '%d:%i' "$1"
}

# Che due valori coincidano, o e' un braccio rosso.
uguali() {
  if [ "$2" != "$3" ]; then
    fallisce "$1: atteso «$2», trovato «$3»"
  fi
}

# --- prerequisiti: assenti significa rosso, mai verde per salto ---------------
[ "$(uname -s)" = "Linux" ] || manca "questo gate vale solo su Linux"
[ "$(id -u)" = "0" ] || manca "serve root: il control plane crea il dominio e cambia identita'"

for STRUMENTO in cargo mkfifo timeout stat getent awk sed grep; do
  command -v "$STRUMENTO" >/dev/null || manca "$STRUMENTO non c'e'"
done

# Gli argomenti si validano **prima** di usarli, e non e' pedanteria: il gate
# gira come root. Un `RADICE` con una barra o un `..` porterebbe `mkdir` e
# `rmdir` fuori dal perimetro dichiarato; un tetto o un timeout non numerici
# finirebbero in una scrittura sul kernel o in un'attesa che non scade.
case "$RADICE_RELATIVA" in
'' | . | ..) manca "RADICE non puo' essere vuoto, «.» o «..»" ;;
*[!A-Za-z0-9._-]*) manca "RADICE dev'essere un nome solo: «$RADICE_RELATIVA» contiene altro" ;;
esac
for COPPIA in "WORKER_UID=$WORKER_UID" "WORKER_GID=$WORKER_GID" \
  "TETTO_BYTE=$TETTO_BYTE" "ATTESA_MASSIMA=$ATTESA_MASSIMA"; do
  NOME="${COPPIA%%=*}"
  VALORE="${COPPIA#*=}"
  case "$VALORE" in
  '' | *[!0-9]*) manca "$NOME dev'essere un intero: «$VALORE»" ;;
  esac
done
[ "$WORKER_UID" != "0" ] || manca "WORKER_UID non puo' essere 0: il worker deve essere un altro"
[ "$WORKER_GID" != "0" ] || manca "WORKER_GID non puo' essere 0"
[ "$TETTO_BYTE" -ge 1048576 ] || manca "TETTO_BYTE sotto 1 MiB non e' un tetto governabile"
[ "$ATTESA_MASSIMA" -ge 5 ] || manca "ATTESA_MASSIMA sotto 5 secondi non e' un'attesa"
getent passwd "$WORKER_UID" >/dev/null || manca "l'uid $WORKER_UID non esiste"

# La directory dell'evidenza dev'essere **nuova e vuota**, e non un
# collegamento. Scrivere in una gia' popolata mescolerebbe i file di due corse —
# e chi legge non ha modo di dire quale riga viene da quale — mentre un symlink
# porterebbe le scritture di root altrove.
[ -L "$DOVE" ] && manca "$DOVE e' un collegamento simbolico: l'evidenza non si scrive attraverso un link"
if [ -e "$DOVE" ]; then
  [ -d "$DOVE" ] || manca "$DOVE esiste e non e' una directory"
  [ -z "$(ls -A "$DOVE" 2>/dev/null)" ] \
    || manca "$DOVE non e' vuota: l'evidenza di due corse nello stesso posto non si distingue"
else
  # `mkdir` e non `mkdir -p`: il gate gira come root, e `-p` su un percorso che
  # arriva dal chiamante creerebbe in silenzio tutta la catena dei genitori. La
  # directory dell'evidenza si mette dove qualcosa gia' esiste.
  mkdir "$DOVE" || manca "$DOVE non si crea: la directory che lo contiene deve gia' esistere"
fi

# Il montaggio cgroup2 dev'essere **uno**. Con piu' di uno, registrarne uno e
# calcolare l'appartenenza su un altro significa dire due cose su due filesystem
# diversi credendo di parlare dello stesso — ed e' il difetto che il preflight
# rifiuta, quindi il gate non lo introduce.
mapfile -t MONTAGGI < <(awk '{ for (i = 7; i <= NF; i++) if ($i == "-") { if ($(i + 1) == "cgroup2") print $5; break } }' /proc/self/mountinfo)
[ "${#MONTAGGI[@]}" -ge 1 ] || manca "nessun montaggio cgroup2 in /proc/self/mountinfo"
[ "${#MONTAGGI[@]}" -eq 1 ] || manca "piu' montaggi cgroup2 (${MONTAGGI[*]}): quale valga non lo dichiara nessuno"
PUNTO="${MONTAGGI[0]}"
[ -e "$PUNTO/cgroup.controllers" ] || manca "$PUNTO non ha cgroup.controllers: non e' una radice cgroup v2"
grep -qw memory "$PUNTO/cgroup.controllers" \
  || manca "il controllore memory non e' disponibile in $PUNTO"

# --- l'immagine del gate ------------------------------------------------------
case "$PROFILO" in
release) SOTTODIRECTORY=release ;;
dev) SOTTODIRECTORY=debug ;;
*) manca "PROFILO vale release o dev, non «$PROFILO»" ;;
esac

# I `RUSTFLAGS` ereditati si **registrano** prima di usarli. Il gate ci
# aggiunge il proprio `--cfg`, ma cio' che arriva dall'ambiente cambia cio' che
# viene compilato — un `--cfg` in piu', una `-C target-feature`, un `-D` — e un
# esito senza quel dato non dice che cosa e' stato qualificato.
RUSTFLAGS_EREDITATI="${RUSTFLAGS:-}"
RUSTFLAGS_EFFETTIVI="$RUSTFLAGS_EREDITATI --cfg qualificazione_isolamento"

nota "costruisco l'immagine di qualificazione ($PROFILO)"
RUSTFLAGS="$RUSTFLAGS_EFFETTIVI" \
  cargo build --locked --features internals --profile "$PROFILO" \
  --example qualificazione_isolamento >/dev/null \
  || manca "l'immagine di qualificazione non si costruisce"

SORGENTE_IMMAGINE="${CARGO_TARGET_DIR:-target}/$SOTTODIRECTORY/examples/qualificazione_isolamento"
[ -x "$SORGENTE_IMMAGINE" ] || manca "l'immagine costruita non si trova in $SORGENTE_IMMAGINE"

# --- lo spazio di lavoro ------------------------------------------------------
TEMPORANEA="$(mktemp -d)"
# Il worker gira con un'altra identita' e deve poter **attraversare** questa
# directory per essere eseguito: `mktemp -d` la crea a 0700, che lo escluderebbe
# e produrrebbe un rifiuto che non parla di isolamento.
chmod 0755 "$TEMPORANEA"
RADICE_ASSOLUTA="$PUNTO/$RADICE_RELATIVA"
DOMINIO="$RADICE_ASSOLUTA/dominio"
VICINO="$RADICE_ASSOLUTA/vicino"

if [ -e "$RADICE_ASSOLUTA" ]; then
  PERCORSO="$RADICE_ASSOLUTA"
  RADICE_ASSOLUTA=""
  DOMINIO=""
  VICINO=""
  manca "$PERCORSO esiste gia': il gate non riusa un dominio altrui"
fi

# La delega alla radice del montaggio e' una **mutazione globale**: si tocca
# solo se serve, e si registra per riportarla al valore di partenza. `pids` non serve a niente
# di cio' che qui si prova, quindi non si accende: un controllore acceso e mai
# usato e' una modifica alla macchina senza contropartita.
# Senza `|| true`: se la delega non si legge, tutto cio' che segue —
# accenderla, e soprattutto rimetterla come stava — poggerebbe su un valore che
# nessuno ha letto.
SUBTREE_PRIMA="$(cat "$PUNTO/cgroup.subtree_control")" \
  || manca "$PUNTO/cgroup.subtree_control non si legge: la delega non e' accertabile"
if grep -qw memory <<<"$SUBTREE_PRIMA"; then
  nota "memory gia' delegato in $PUNTO: il gate non tocca la delega globale"
else
  nota "abilito memory in $PUNTO/cgroup.subtree_control (la pulizia lo toglie)"
  echo "+memory" >"$PUNTO/cgroup.subtree_control" \
    || manca "$PUNTO/cgroup.subtree_control non accetta +memory: delega assente"
  grep -qw memory "$PUNTO/cgroup.subtree_control" \
    || manca "+memory scritto ma non riletto in $PUNTO/cgroup.subtree_control"
  MEMORY_ABILITATO_DA_NOI=1
fi

mkdir "$RADICE_ASSOLUTA"
echo "+memory" >"$RADICE_ASSOLUTA/cgroup.subtree_control" \
  || manca "il sottoalbero di $RADICE_ASSOLUTA non accetta +memory"
mkdir "$DOMINIO"
# Il fratello foglia: e' la via d'uscita che esiste davvero. Il `cgroup.procs`
# del **padre** non e' scrivibile da nessuno — nemmeno dal control plane —
# perche' in cgroup v2 un cgroup con figli e controllori delegati non ospita
# processi. Un rifiuto li' parla della gerarchia e non del worker; un rifiuto
# sul fratello parla del worker, ed e' quello che discrimina.
mkdir "$VICINO"

I_QUATTRO=(memory.max memory.swap.max memory.oom.group cgroup.max.depth)

for RICHIESTO in "${I_QUATTRO[@]}" cgroup.events cgroup.procs; do
  [ -e "$DOMINIO/$RICHIESTO" ] \
    || manca "$DOMINIO non ha $RICHIESTO: senza, il preflight fallirebbe per assenza e non per difetto"
done
[ -e "$VICINO/cgroup.procs" ] || manca "$VICINO non ha cgroup.procs: la via d'uscita non e' costruibile"

copia_immagine() {
  cp "$SORGENTE_IMMAGINE" "$1"
  chmod 0755 "$1"
}

# Che cosa e' stato qualificato: l'albero, il binario, lo script e il
# compilatore. Un esito che dicesse solo «qualificato» non sarebbe riferibile a
# niente — e fra un mese nessuno saprebbe dire su quale codice sia stato dato.
#
# L'albero si identifica con il commit **e** con l'impronta di cio' che non e'
# committato: qualificare un worktree sporco e' legittimo, dichiararlo pulito
# non lo e'. Dove non c'e' un repository — il gate gira anche su un albero
# copiato — si registra l'assenza e si ripiega sull'impronta dei sorgenti, che
# identifica lo stesso cio' che e' stato compilato.
impronta_dell_albero() {
  if command -v git >/dev/null && git rev-parse --git-dir >/dev/null 2>&1; then
    printf 'commit=%s\n' "$(git rev-parse HEAD 2>/dev/null || printf 'ignoto')"
    printf 'diff_non_committato=%s\n' "$(git diff HEAD 2>/dev/null | sha256sum | cut -d' ' -f1)"
    printf 'non_tracciati=%s\n' \
      "$(git ls-files --others --exclude-standard 2>/dev/null | sort | xargs -r sha256sum 2>/dev/null | sha256sum | cut -d' ' -f1)"
    printf 'albero_pulito=%s\n' \
      "$([ -z "$(git status --porcelain 2>/dev/null)" ] && printf 'si' || printf 'no')"
  else
    printf 'commit=assente (albero non versionato)\n'
    printf 'impronta_sorgenti=%s\n' \
      "$(find crates scripts Cargo.toml Cargo.lock rust-toolchain.toml -type f 2>/dev/null | sort | xargs -r sha256sum | sha256sum | cut -d' ' -f1)"
  fi
}

{
  printf 'kernel=%s\n' "$(uname -r)"
  printf 'montaggio=%s\n' "$PUNTO"
  printf 'opzioni_superblocco=%s\n' \
    "$(awk -v p="$PUNTO" '$5 == p { print $NF }' /proc/self/mountinfo | tail -n 1)"
  printf 'dominio=%s\n' "$DOMINIO"
  printf 'vicino=%s\n' "$VICINO"
  printf 'worker=%s:%s\n' "$WORKER_UID" "$WORKER_GID"
  printf 'tetto=%s\n' "$TETTO_BYTE"
  printf 'profilo=%s\n' "$PROFILO"
  printf 'subtree_control_prima=%s\n' "$SUBTREE_PRIMA"
  printf 'memory_abilitato_dal_gate=%s\n' "$MEMORY_ABILITATO_DA_NOI"
  impronta_dell_albero
  printf 'sha256_immagine=%s\n' "$(sha256sum "$SORGENTE_IMMAGINE" | cut -d' ' -f1)"
  printf 'sha256_gate=%s\n' "$(sha256sum "$0" | cut -d' ' -f1)"
  printf 'rustc=%s\n' "$(rustc --version 2>/dev/null || printf 'ignoto')"
  printf 'cargo=%s\n' "$(cargo --version 2>/dev/null || printf 'ignoto')"
  printf 'rustflags_ereditati=%s\n' "$RUSTFLAGS_EREDITATI"
  printf 'rustflags_effettivi=%s\n' "$RUSTFLAGS_EFFETTIVI"
} >"$DOVE/preflight.txt"

: >"$DOVE/stato.txt"
stato_dei_bersagli() {
  local _quando="$1" _file
  {
    for _file in "${I_QUATTRO[@]}" cgroup.events; do
      printf '%s %s=%s\n' "$_quando" "$_file" "$(tr '\n' ' ' <"$DOMINIO/$_file" 2>&1)"
    done
    printf '%s padre/cgroup.procs=%s\n' "$_quando" \
      "$(tr '\n' ' ' <"$RADICE_ASSOLUTA/cgroup.procs" 2>&1)"
    printf '%s vicino/cgroup.procs=%s\n' "$_quando" \
      "$(tr '\n' ' ' <"$VICINO/cgroup.procs" 2>&1)"
    printf '%s dominio_permessi=%s\n' "$_quando" "$(stat -c '%U:%G %a' "$DOMINIO")"
  } >>"$DOVE/stato.txt"
}

# --- bracci 1 e 3: sentinella, transizione, e i tentativi ostili --------------
nota "bracci 1 e 3: transizione, sentinella e tentativi ostili"
IMMAGINE_1="$TEMPORANEA/immagine-1"
copia_immagine "$IMMAGINE_1"

stato_dei_bersagli prima_ostile
set +e
"$IMMAGINE_1" supervisore "$DOMINIO" "$RADICE_ASSOLUTA" "$TETTO_BYTE" \
  "$WORKER_UID" "$WORKER_GID" -- "$IMMAGINE_1" ostile "$DOMINIO" "$RADICE_ASSOLUTA" \
  >"$DOVE/braccio-ostile.txt" 2>&1
USCITA_OSTILE=$?
set -e
nota "  uscita=$USCITA_OSTILE"
stato_dei_bersagli dopo_ostile

OSTILE="$DOVE/braccio-ostile.txt"
uguali "uscita del supervisore ostile" "0" "$USCITA_OSTILE"
uguali "preflight" "riuscito" "$(valore "$OSTILE" preflight)"
uguali "avvio" "riuscito" "$(valore "$OSTILE" avvio)"
uguali "braccio 1, sentinella sul dispatch" "1" "$(valore "$OSTILE" sentinella_task)"
uguali "rapporto del worker ostile" "concluso" "$(valore "$OSTILE" ostile)"
uguali "uscita del worker ostile" "0" "$(valore "$OSTILE" figlio_uscita)"

# Nessun tentativo dice RIUSCITO.
if grep -q '^QI tentativo_.*RIUSCITO' "$OSTILE"; then
  grep '^QI tentativo_.*RIUSCITO' "$OSTILE" >&2
  fallisce "braccio 3: un tentativo ostile e' riuscito"
fi

# ...e tutti i tentativi attesi sono stati fatti: cio' che non si tenta non si
# esclude, e una riga assente si leggerebbe come un rifiuto.
for ATTESO in tentativo_controllo_memory.max tentativo_controllo_memory.swap.max \
  tentativo_controllo_memory.oom.group tentativo_controllo_cgroup.max.depth \
  tentativo_fuga_padre tentativo_fuga_vicino tentativo_unshare \
  tentativo_dopo_unshare tentativo_dopo_unshare_fuga tentativo_dopo_unshare_fuga_vicino; do
  grep -q "^QI $ATTESO=" "$OSTILE" || fallisce "braccio 3: manca il tentativo $ATTESO"
done
grep -q "^QI tentativo_fuga_vicino=bersaglio=$VICINO/cgroup.procs " "$OSTILE" \
  || fallisce "braccio 3: il worker non ha tentato la via d'uscita su $VICINO"

# --- stato invariato: si confronta, non si deduce ----------------------------
# I nove tentativi che portano `prima`/`dopo`: i quattro controlli, le due vie
# d'uscita, e le tre ripetizioni dopo la `unshare`. L'elenco e' **esplicito** e
# il conteggio e' **esatto**: «almeno nove» accetterebbe un decimo tentativo che
# nessuno ha previsto, e nove righe di cui due sulla stessa chiave.
CON_PRIMA_E_DOPO=(
  tentativo_controllo_memory.max
  tentativo_controllo_memory.swap.max
  tentativo_controllo_memory.oom.group
  tentativo_controllo_cgroup.max.depth
  tentativo_fuga_padre
  tentativo_fuga_vicino
  tentativo_dopo_unshare
  tentativo_dopo_unshare_fuga
  tentativo_dopo_unshare_fuga_vicino
)

nota "stato invariato: confronto prima/dopo di ogni tentativo"
for CHIAVE in "${CON_PRIMA_E_DOPO[@]}"; do
  QUANTE="$(grep -c "^QI $CHIAVE=" "$OSTILE" || true)"
  if [ "$QUANTE" != "1" ]; then
    fallisce "stato invariato: «$CHIAVE» compare $QUANTE volte, attesa una sola"
    continue
  fi
  RIGA="$(grep "^QI $CHIAVE=" "$OSTILE")"
  case "$RIGA" in
  *"prima=«"*"» dopo=«"*) : ;;
  *)
    fallisce "stato invariato: «$CHIAVE» non porta prima/dopo"
    continue
    ;;
  esac
  PRIMA="$(sed -n 's/.*prima=«\(.*\)» dopo=«\(.*\)».*/\1/p' <<<"$RIGA")"
  DOPO="$(sed -n 's/.*prima=«\(.*\)» dopo=«\(.*\)».*/\2/p' <<<"$RIGA")"
  CONFRONTATI=$((CONFRONTATI + 1))
  uguali "stato invariato su $CHIAVE" "$PRIMA" "$DOPO"
done
RIGHE_CON_PRIMA="$(grep -cE '^QI tentativo_.*prima=' "$OSTILE" || true)"
uguali "numero di tentativi con prima/dopo" "${#CON_PRIMA_E_DOPO[@]}" "$RIGHE_CON_PRIMA"
uguali "confronti prima/dopo riusciti" "${#CON_PRIMA_E_DOPO[@]}" "$CONFRONTATI"
nota "  confronti prima/dopo: $CONFRONTATI"

# E lo stato che il **gate** vede da fuori, alla fine della corsa ostile: il
# worker potrebbe riportare due valori uguali e averne cambiato un terzo che non
# riporta.
#
# Il confronto e' contro i valori che il **preflight** stabilisce, non contro
# quelli di prima della corsa: prima del preflight il dominio e' vergine
# (`memory.max` vale `max`), e confrontare con quelli direbbe che il preflight
# ha cambiato qualcosa — che e' il suo mestiere.
attesi_dal_preflight() {
  case "$1" in
  memory.max) printf '%s' "$TETTO_BYTE" ;;
  memory.swap.max) printf '0' ;;
  memory.oom.group) printf '1' ;;
  cgroup.max.depth) printf '0' ;;
  esac
}
for FILE in "${I_QUATTRO[@]}"; do
  TROVATO="$(sed -n "s/^dopo_ostile $FILE=//p" "$DOVE/stato.txt" | head -1)"
  uguali "il dominio dopo la corsa ostile, su $FILE" "$(attesi_dal_preflight "$FILE") " "$TROVATO"
done

# --- l'identita' del worker, campo per campo ---------------------------------
nota "identita' del worker spogliato"
uguali "worker: /proc leggibile" "si" "$(valore "$OSTILE" id_prima_unshare_leggibile)"
uguali "worker: uid" "$WORKER_UID,$WORKER_UID,$WORKER_UID,$WORKER_UID" \
  "$(valore "$OSTILE" id_prima_unshare_uid)"
uguali "worker: gid" "$WORKER_GID,$WORKER_GID,$WORKER_GID,$WORKER_GID" \
  "$(valore "$OSTILE" id_prima_unshare_gid)"
uguali "worker: gruppi supplementari" "" "$(valore "$OSTILE" id_prima_unshare_gruppi)"
uguali "worker: no_new_privs" "1" "$(valore "$OSTILE" id_prima_unshare_no_new_privs)"
for MASCHERA in cap_effective cap_permitted cap_inheritable cap_ambient; do
  uguali "worker: $MASCHERA" "0" "$(valore "$OSTILE" "id_prima_unshare_$MASCHERA")"
done

# --- il ramo `unshare`, con i due esiti ammessi ------------------------------
#
# `no_new_privs` **non** impedisce `unshare(CLONE_NEWUSER)`: vieta di acquisire
# privilegi attraverso una `execve`, e non tocca `unshare`. Gli esiti ammessi
# sono due, e il gate non ne pretende uno: pretende che **in entrambi** il
# control plane resti fuori portata — cosa che i confronti prima/dopo qui sopra
# hanno gia' verificato riga per riga.
UNSHARE="$(valore "$OSTILE" tentativo_unshare)"
nota "  unshare: $UNSHARE"
case "$UNSHARE" in
riuscita)
  # Se riesce, dev'essere successo **davvero**: namespace utente diverso e
  # capability piene dentro. Un `unshare` che rendesse «riuscita» senza cambiare
  # niente farebbe passare questo braccio senza che nessuno abbia tentato la
  # cosa che il braccio esiste per escludere.
  NS_PRIMA="$(valore "$OSTILE" id_prima_unshare_ns_user)"
  NS_DOPO="$(valore "$OSTILE" id_dopo_unshare_ns_user)"
  uguali "unshare: /proc leggibile dopo" "si" "$(valore "$OSTILE" id_dopo_unshare_leggibile)"
  if [ -z "$NS_PRIMA" ] || [ -z "$NS_DOPO" ]; then
    fallisce "unshare: manca lo user namespace prima («$NS_PRIMA») o dopo («$NS_DOPO»)"
  elif [ "$NS_PRIMA" = "$NS_DOPO" ]; then
    fallisce "unshare: dichiarata riuscita ma lo user namespace non e' cambiato ($NS_DOPO)"
  fi
  CAP_DOPO="$(valore "$OSTILE" id_dopo_unshare_cap_effective)"
  if [ -z "$CAP_DOPO" ] || [ "$CAP_DOPO" = "0" ]; then
    fallisce "unshare: dichiarata riuscita ma senza capability nel namespace figlio (CapEff=«$CAP_DOPO»)"
  fi
  # L'identita' numerica **non** deve essere tornata a zero fuori dal
  # namespace: dentro puo' apparire mappata, ma i quattro identificatori
  # restano quelli che il kernel riporta.
  uguali "unshare: uid dopo" "$(valore "$OSTILE" id_prima_unshare_uid)" \
    "$(valore "$OSTILE" id_dopo_unshare_uid)"
  uguali "unshare: no_new_privs dopo" "1" "$(valore "$OSTILE" id_dopo_unshare_no_new_privs)"
  nota "    namespace $NS_PRIMA -> $NS_DOPO, CapEff $CAP_DOPO"
  ;;
rifiutata*)
  nota "    rifiutata dalla policy dell'host: e' il primo dei due esiti ammessi"
  ;;
*)
  fallisce "unshare: esito non riconosciuto «$UNSHARE»"
  ;;
esac

# --- controprova privilegiata -------------------------------------------------
nota "controprova privilegiata sui bersagli"
: >"$DOVE/controprova.txt"
controprova() {
  local _bersaglio="$1" _alternativo="$2" _originale _riletto
  if ! _originale="$(cat "$_bersaglio" 2>/dev/null)"; then
    fallisce "controprova: $_bersaglio non si legge"
    return
  fi
  if [ "$_originale" = "$_alternativo" ]; then
    fallisce "controprova: $_bersaglio vale gia' «$_alternativo», riscriverlo non proverebbe niente"
    return
  fi
  if ! printf '%s' "$_alternativo" >"$_bersaglio" 2>/dev/null; then
    printf '%s scrivibile_dal_control_plane=no originale=%s\n' "$_bersaglio" "$_originale" \
      >>"$DOVE/controprova.txt"
    fallisce "controprova: il control plane non scrive $_bersaglio, quindi il rifiuto del worker non dice niente"
    return
  fi
  _riletto="$(cat "$_bersaglio")"
  if [ "$_riletto" != "$_alternativo" ]; then
    printf '%s scrittura_accettata_ma_non_riletta atteso=%s riletto=%s\n' \
      "$_bersaglio" "$_alternativo" "$_riletto" >>"$DOVE/controprova.txt"
    fallisce "controprova: $_bersaglio accetta la scrittura ma rilegge «$_riletto» invece di «$_alternativo»"
  fi
  if ! printf '%s' "$_originale" >"$_bersaglio" 2>/dev/null; then
    fallisce "controprova: $_bersaglio non torna a «$_originale»: il dominio resta alterato"
    return
  fi
  _riletto="$(cat "$_bersaglio")"
  if [ "$_riletto" != "$_originale" ]; then
    fallisce "controprova: $_bersaglio ripristinato a «$_riletto» invece di «$_originale»"
    return
  fi
  printf '%s originale=%s alternativo_scritto_e_riletto=%s ripristinato_e_riletto=%s\n' \
    "$_bersaglio" "$_originale" "$_alternativo" "$_riletto" >>"$DOVE/controprova.txt"
}
controprova "$DOMINIO/memory.max" "$((TETTO_BYTE / 2))"
controprova "$DOMINIO/memory.swap.max" "max"
controprova "$DOMINIO/memory.oom.group" "0"
controprova "$DOMINIO/cgroup.max.depth" "1"

# La via d'uscita non si prova con un pid inventato — direbbe «rifiutato» per la
# ragione sbagliata — e nemmeno spostandoci il gate, che resterebbe in un cgroup
# da rimuovere se qualcosa andasse storto. Si usa un processo usa-e-getta, e lo
# si rilegge: una scrittura accettata che non sposta niente non e' una prova.
sleep 60 &
CAVIA=$!
if echo "$CAVIA" >"$VICINO/cgroup.procs" 2>/dev/null \
  && grep -qx "$CAVIA" "$VICINO/cgroup.procs"; then
  printf '%s scrivibile_dal_control_plane=si (pid %s spostato e riletto)\n' \
    "$VICINO/cgroup.procs" "$CAVIA" >>"$DOVE/controprova.txt"
else
  printf '%s scrivibile_dal_control_plane=no\n' "$VICINO/cgroup.procs" >>"$DOVE/controprova.txt"
  fallisce "controprova: il control plane non sposta un processo in $VICINO, e senza quello il rifiuto del worker li' non dice niente"
fi
kill -9 "$CAVIA" 2>/dev/null || true
wait "$CAVIA" 2>/dev/null || true

# Il `cgroup.procs` del padre si **registra** e non si conta: nessuno lo scrive,
# per una regola strutturale di cgroup v2, e pretendere che il control plane ci
# riesca renderebbe rosso un gate corretto.
if echo $$ >"$RADICE_ASSOLUTA/cgroup.procs" 2>/dev/null; then
  printf '%s scrivibile_dal_control_plane=si (inatteso: il padre ha figli e controllori delegati)\n' \
    "$RADICE_ASSOLUTA/cgroup.procs" >>"$DOVE/controprova.txt"
else
  printf '%s scrivibile_dal_control_plane=no (regola dei processi interni: non discrimina)\n' \
    "$RADICE_ASSOLUTA/cgroup.procs" >>"$DOVE/controprova.txt"
fi

# --- la finestra fra il cambio d'identita' e la `exec` -----------------------
#
# E' l'intervallo in cui vive il settimo passo, e nessun altro braccio lo
# attraversa: il worker sta **dopo** la `exec`, quando il kernel ha gia'
# restituito `/proc/<pid>` al nuovo proprietario. Si registra e non si pretende:
# su un kernel che concedesse la lettura anche li', portare avanti namespace e
# descrittori resterebbe corretto, solo non sarebbe piu' obbligato.
nota "misura della finestra fra il cambio d'identita' e la exec"
set +e
"$IMMAGINE_1" finestra "$WORKER_UID" "$WORKER_GID" >"$DOVE/finestra.txt" 2>&1
USCITA_FINESTRA=$?
set -e
nota "  uscita=$USCITA_FINESTRA"
uguali "uscita della misura della finestra" "0" "$USCITA_FINESTRA"
uguali "misura della finestra" "conclusa" "$(valore "$DOVE/finestra.txt" finestra)"
grep '^QI proc_leggibile_' "$DOVE/finestra.txt" >>"$DOVE/stato.txt" || true
grep '^QI proc_leggibile_' "$OSTILE" >>"$DOVE/stato.txt" || true

# --- braccio 2a: sostituzione osservata prima del controllo -------------------
#
# Il supervisore si ferma **prima del preflight**, il gate rinomina sopra il suo
# pathname, e solo allora lo lascia andare: quando l'accertamento corre, il
# bersaglio di /proc/self/exe porta ` (deleted)` e il rifiuto e' quello atteso.
nota "braccio 2a: sostituzione prima del controllo"
PRONTO_A="$TEMPORANEA/pronto-a"
VIA_A="$TEMPORANEA/via-a"
mkfifo "$PRONTO_A" "$VIA_A"
IMMAGINE_2A="$TEMPORANEA/immagine-2a"
SOSTITUTIVA_2A="$TEMPORANEA/sostitutiva-2a"
copia_immagine "$IMMAGINE_2A"
copia_immagine "$SOSTITUTIVA_2A"
[ "$(inode_di "$IMMAGINE_2A")" != "$(inode_di "$SOSTITUTIVA_2A")" ] \
  || manca "le due copie hanno lo stesso inode: la sostituzione non sarebbe osservabile"

set +e
"$IMMAGINE_2A" supervisore "$DOMINIO" "$RADICE_ASSOLUTA" "$TETTO_BYTE" \
  "$WORKER_UID" "$WORKER_GID" --attendi "$PRONTO_A" "$VIA_A" -- /bin/true \
  >"$DOVE/braccio-sostituzione-prima.txt" 2>&1 &
SUPERVISORE_2A=$!
if timeout "$ATTESA_MASSIMA" cat "$PRONTO_A" >/dev/null; then
  mv -f "$SOSTITUTIVA_2A" "$IMMAGINE_2A"
  timeout "$ATTESA_MASSIMA" sh -c "printf 'via\n' > '$VIA_A'"
else
  fallisce "braccio 2a: il supervisore non ha raggiunto l'attesa iniziale entro $ATTESA_MASSIMA s"
  kill -9 "$SUPERVISORE_2A" 2>/dev/null
fi
wait "$SUPERVISORE_2A"
USCITA_2A=$?
set -e
nota "  uscita=$USCITA_2A"
# Qui l'uscita **dev'essere** non-zero: il braccio prova che l'avvio si rifiuta,
# e un supervisore che uscisse a zero avrebbe avviato qualcosa.
if [ "$USCITA_2A" -eq 0 ]; then
  fallisce "braccio 2a: il supervisore e' uscito a zero, ma l'avvio doveva fallire"
fi

# --- braccio 2b: sostituzione dopo il controllo, con barriera -----------------
nota "braccio 2b: sostituzione dopo il controllo"
PRONTO_B="$TEMPORANEA/pronto-b"
VIA_B="$TEMPORANEA/via-b"
mkfifo "$PRONTO_B" "$VIA_B"
IMMAGINE_2B="$TEMPORANEA/immagine-2b"
SOSTITUTIVA_2B="$TEMPORANEA/sostitutiva-2b"
copia_immagine "$IMMAGINE_2B"
copia_immagine "$SOSTITUTIVA_2B"
INODE_2B="$(inode_di "$IMMAGINE_2B")"
INODE_SOSTITUTIVA_2B="$(inode_di "$SOSTITUTIVA_2B")"
[ "$INODE_2B" != "$INODE_SOSTITUTIVA_2B" ] \
  || manca "le due copie hanno lo stesso inode: la sostituzione non sarebbe osservabile"

set +e
"$IMMAGINE_2B" supervisore "$DOMINIO" "$RADICE_ASSOLUTA" "$TETTO_BYTE" \
  "$WORKER_UID" "$WORKER_GID" --barriera "$PRONTO_B" "$VIA_B" \
  -- "$IMMAGINE_2B" ostile "$DOMINIO" "$RADICE_ASSOLUTA" \
  >"$DOVE/braccio-sostituzione-dopo.txt" 2>&1 &
SUPERVISORE_2B=$!
if timeout "$ATTESA_MASSIMA" cat "$PRONTO_B" >/dev/null; then
  mv -f "$SOSTITUTIVA_2B" "$IMMAGINE_2B"
  timeout "$ATTESA_MASSIMA" sh -c "printf 'via\n' > '$VIA_B'"
else
  fallisce "braccio 2b: il supervisore non ha raggiunto la barriera entro $ATTESA_MASSIMA s"
  kill -9 "$SUPERVISORE_2B" 2>/dev/null
fi
wait "$SUPERVISORE_2B"
USCITA_2B=$?
set -e
nota "  uscita=$USCITA_2B"
uguali "uscita del supervisore 2b" "0" "$USCITA_2B"
uguali "uscita del worker in 2b" "0" \
  "$(valore "$DOVE/braccio-sostituzione-dopo.txt" figlio_uscita)"
stato_dei_bersagli fine

# --- giudizio sul braccio 2 ---------------------------------------------------
CAUSA_2A="$(valore "$DOVE/braccio-sostituzione-prima.txt" errore)"
uguali "braccio 2a, avvio" "fallito" "$(valore "$DOVE/braccio-sostituzione-prima.txt" avvio)"
case "$CAUSA_2A" in
*"rimossa o sostituita"*) : ;;
*) fallisce "braccio 2a: il rifiuto non nomina la sostituzione, dice «$CAUSA_2A»" ;;
esac

INODE_FIGLIO="$(valore "$DOVE/braccio-sostituzione-dopo.txt" immagine_inode)"
uguali "braccio 2b, avvio" "riuscito" "$(valore "$DOVE/braccio-sostituzione-dopo.txt" avvio)"
if [ -z "$INODE_FIGLIO" ]; then
  fallisce "braccio 2b: il figlio non ha dichiarato il proprio inode, quindi non si sa che cosa sia partito"
elif [ "$INODE_FIGLIO" = "$INODE_SOSTITUTIVA_2B" ]; then
  fallisce "braccio 2b: e' partita l'immagine sostitutiva ($INODE_FIGLIO)"
elif [ "$INODE_FIGLIO" != "$INODE_2B" ]; then
  fallisce "braccio 2b: e' partito l'inode $INODE_FIGLIO, ne' l'iniziale $INODE_2B ne' la sostitutiva"
fi

{
  printf 'inode_iniziale_2b=%s\n' "$INODE_2B"
  printf 'inode_sostitutiva_2b=%s\n' "$INODE_SOSTITUTIVA_2B"
  printf 'inode_partito_2b=%s\n' "$INODE_FIGLIO"
  printf 'confronti_prima_dopo=%s\n' "$CONFRONTATI"
} >>"$DOVE/preflight.txt"

# --- l'esito ------------------------------------------------------------------
#
# Non si stampa qui. Lo stampa `verdetto`, che corre su `EXIT` **dopo** la
# pulizia: un «qualificato» scritto in questo punto direbbe che tutto e' andato
# bene mentre l'ultimo cgroup e' ancora li'.
