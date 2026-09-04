#!/usr/bin/env bash
# Qualifica **sotto limite**: il worker reale esegue dentro un dominio cgroup2
# con `memory.max`, raggiunto attraverso lo spawner.
#
# CHE COSA QUALIFICA, E CHE COSA IL PERCORSO FRA DUE PIPE NON QUALIFICA
#
#   `scripts/qualifica_worker_reale.sh` prova il cablaggio: handshake, incarico,
#   progresso, esito, artefatto riverificato, EOF e raccolta — ma fra due pipe
#   nude, senza spawner e senza dominio. Qui il canale nasce dallo spawner, il
#   worker riceve i propri descrittori da lui, e l'esecuzione avviene dentro un
#   cgroup2 con i quattro controlli scritti dal preflight: `memory.max`,
#   `memory.swap.max` a zero, `memory.oom.group` a uno, `cgroup.max.depth` a
#   zero.
#
# LE DUE IMMAGINI, E PERCHE' SONO DUE
#
#   Il **supervisore** e' l'immagine di qualificazione (un example, dietro
#   `--cfg qualificazione_isolamento`): e' lei che `/proc/self/exe` rieseguira'
#   in modalita' spawner, e per questo deve conoscere quel namespace di argv.
#
#   Il **worker** e' l'immagine di produzione, compilata senza `internals` in un
#   target suo e fissata da uno SHA-256. E' il binario che si distribuisce, ed e'
#   l'unico di cui questa qualificazione parli.
#
# PERCHE' ROOT, E CHE COSA NE FA
#
#   Serve a delegare il sottoalbero cgroup2 e a creare il dominio; il worker
#   invece gira con le credenziali che gli si passano, e il preflight le
#   verifica. Lo script non lascia niente dietro: il dominio si smonta in ogni
#   caso, anche su errore.
#
# USO
#
#   sudo scripts/qualifica_sotto_limite.sh

set -Eeuo pipefail

RADICE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$RADICE"

TARGET_PRODUZIONE="$RADICE/target-immagine-produzione"
TETTO_BYTE=$((512 * 1024 * 1024))
NOME="plenora-sotto-limite-$$"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "questo percorso esiste solo su Linux" >&2
  exit 2
fi
if [[ "$(id -u)" -ne 0 ]]; then
  echo "serve root: la delega del sottoalbero cgroup2 e la creazione del dominio non si fanno da utente" >&2
  exit 2
fi
if [[ $# -ne 0 ]]; then
  echo "riga di comando non ammessa: «$*» — questo script non prende argomenti" >&2
  exit 2
fi

PUNTO="$(awk '$3 == "cgroup2" { print $2; uscite++ } END { exit uscite != 1 }' /proc/self/mounts)" || {
  echo "il montaggio cgroup2 dev'essere uno solo: con piu' di uno non si sa quale si stia governando" >&2
  exit 1
}

# --- chi sara' il worker ----------------------------------------------------
#
# Un utente che esiste gia' e non e' root. Il dominio **non** e' suo, e non deve
# esserlo: resta del control plane, e il preflight pretende l'opposto del
# possesso — che il worker non possa scriverlo, ne' lui ne' nessuno dei suoi
# antenati fino alla radice. Un worker che potesse scriverlo riscriverebbe da se'
# il tetto che lo governa, e l'identita' distinta non servirebbe a niente.
#
# Dentro il dominio l'identita' non privilegiata ce la mette lo **spawner**,
# finche' e' ancora privilegiato. Qui serve solo che sia distinta e senza
# privilegi: con root non ci sarebbe niente da misurare, perche' i permessi non
# lo fermerebbero comunque.
CHI="${SUDO_USER:-nobody}"
UID_W="$(id -u "$CHI")"
GID_W="$(id -g "$CHI")"

DOMINIO=""
STANZA=""
# Zero fino a quando il corpo non ha attraversato tutto. Chi legge il verdetto lo
# legge da `pulizia`, che senza questa variabile non saprebbe distinguere «e'
# andata bene» da «non ci siamo ancora arrivati»: due cose che, in un'uscita
# zero, si somigliano molto.
QUALIFICATO=0

# Se il dominio e' ancora abitato, secondo la **fonte che lo dichiara**.
#
# PERCHE' `cgroup.events` E NON `cgroup.procs`
#
#   Perche' `-s` chiede se un file ha dimensione maggiore di zero, e i file di
#   `cgroup2` sono virtuali: la loro dimensione dichiarata e' zero anche quando
#   il contenuto non lo e'. `[[ -s cgroup.procs ]]` e' quindi **sempre falso**, e
#   un'attesa che ci si appoggi finisce al primo giro dicendo «vuoto» qualunque
#   cosa ci sia dentro.
#
#   `cgroup.events` porta una riga `populated 0|1`, che e' esattamente la domanda
#   e la risposta del kernel. La si legge, non la si deduce.
# Rende **tre** stati, non due:
#
#   0  abitato
#   1  vuoto
#   2  l'osservazione non si e' potuta fare, o non si e' potuta credere
#
# PERCHE' TRE E NON DUE
#
#   Perche' «vuoto» e «non l'ho potuto guardare» sono cose diverse, e un codice
#   solo per entrambe le rende indistinguibili proprio dove la distinzione
#   serve: se l'osservazione fallisce e `rmdir` riesce, la pulizia conclude
#   verde senza aver mai visto il dominio svuotarsi. Quel verde direbbe
#   «quiescente» avendo misurato niente, ed e' un fail-open — la specie di
#   difetto che questo percorso esiste per non ammettere.
#
# CHE COSA PRETENDE
#
#   Esattamente **una** riga `populated`, con valore `0` oppure `1`. Nessuna
#   riga, due righe, o un valore che non e' nessuno dei due: e' un file che non
#   si sa leggere, e non lo si interpreta a maggioranza.
abitato() {
  local eventi="$1/cgroup.events"
  [[ -r "$eventi" ]] || return 2
  # Si guarda anche `NF`: una riga «populated 0 spazzatura» ha il secondo campo
  # giusto e non e' la riga che il kernel scrive. Controllare il solo `$2`
  # accetterebbe un file che **assomiglia** a quello atteso senza esserlo, ed e'
  # la stessa specie di indulgenza che il tri-stato serve a togliere.
  awk '
    $1 == "populated" { righe++; valore = (NF == 2 ? $2 : "") }
    END {
      if (righe != 1) { exit 2 }
      if (valore == "1") { exit 0 }
      if (valore == "0") { exit 1 }
      exit 2
    }' "$eventi"
}

# La pulizia, con un tetto e la capacita' di far diventare rosso un verde.
#
# PERCHE' PUO' CAMBIARE L'ESITO
#
#   Perche' un dominio che resta e' un difetto della campagna, non una nota a
#   margine: il giro successivo troverebbe un cgroup con quel nome, o peggio dei
#   processi ancora dentro un tetto che nessuno governa piu'. Stampare `PERSO` e
#   uscire zero direbbe a chi legge il riepilogo che e' andato tutto bene.
pulizia() {
  local esito=$?
  local guasto=0
  if [[ -n "$DOMINIO" && -d "$DOMINIO" ]]; then
    # Prima si uccide cio' che c'e' dentro, poi si toglie: un cgroup abitato non
    # si rimuove, e `rmdir` fallirebbe lasciando il dominio in giro.
    if [[ -e "$DOMINIO/cgroup.kill" ]]; then
      echo 1 >"$DOMINIO/cgroup.kill" 2>/dev/null || true
    fi
    # `cgroup.kill` e' asincrono: la scrittura torna, i processi muoiono dopo.
    # Il tetto e' di cinque secondi, cinquanta giri da un decimo: oltre, non e'
    # lentezza, e' qualcosa che non muore.
    #
    # Lo stato si raccoglie con `|| stato=$?` e non si guarda con `if`: dentro
    # un gestore, sotto `set -e`, una funzione che rende non-zero come comando a
    # se' stante farebbe uscire lo script a meta' della pulizia.
    local giro=0 stato=0
    while :; do
      stato=0
      abitato "$DOMINIO" || stato=$?
      # Si aspetta solo finche' e' **abitato**: vuoto o non osservabile, si esce
      # subito e a decidere e' il blocco qui sotto.
      [[ "$stato" -eq 0 ]] || break
      [[ "$giro" -lt 50 ]] || break
      sleep 0.1
      giro=$((giro + 1))
    done
    case "$stato" in
      1)
        if ! rmdir "$DOMINIO" 2>/dev/null; then
          echo "PERSO: il dominio $DOMINIO non si e' rimosso" >&2
          guasto=1
        fi
        ;;
      0)
        echo "PERSO: il dominio $DOMINIO e' ancora abitato dopo cgroup.kill" >&2
        guasto=1
        ;;
      *)
        # **Prima la prova, poi la bonifica**, e in quest'ordine perche' l'ordine
        # e' la cosa che conta.
        #
        # Il rosso si decide qui e non torna piu' indietro: la quiescenza non e'
        # stata osservata, e nessuna rimozione riuscita dopo puo' diventarne una
        # dimostrazione. Interpretare un `rmdir` andato bene come «era vuoto»
        # sarebbe il fail-open, scritto due righe piu' in la'.
        echo "PERSO: la quiescenza di $DOMINIO non e' osservabile: cgroup.events non si legge o non si interpreta" >&2
        guasto=1
        # La bonifica invece serve, e non prova niente: senza, un dominio
        # resterebbe sulla macchina e il giro dopo troverebbe un cgroup con quel
        # nome. Il suo esito non si guarda proprio perche' non e' evidenza.
        rmdir "$DOMINIO" 2>/dev/null || true
        ;;
    esac
  fi
  if [[ -n "$STANZA" && -d "$STANZA" ]] && ! rm -rf "$STANZA"; then
    echo "PERSO: la fixture $STANZA non si e' rimossa" >&2
    guasto=1
  fi
  # Un guasto della pulizia **non sostituisce** una causa gia' presente: su un
  # percorso gia' rosso resta il codice di quello, che dice di piu'.
  if [[ "$guasto" -ne 0 && "$esito" -eq 0 ]]; then
    esito=1
  fi
  # Il verdetto si pronuncia **qui**, e solo qui: e' l'ultimo punto in cui si
  # sappia tutto, pulizia compresa. Servono entrambe le condizioni — il corpo ha
  # armato il verdetto, e l'esito e' rimasto zero — perche' un'uscita rossa che
  # stampasse comunque VINTO direbbe a chi legge il contrario di cio' che dice a
  # chi automatizza.
  if [[ "$esito" -eq 0 && "$QUALIFICATO" -eq 1 ]]; then
    echo "VINTO: il worker reale ha eseguito dentro il dominio, sotto memory.max"
    echo "  digest dichiarato e verificato prima e dopo: $DIGEST"
  fi
  exit "$esito"
}
trap pulizia EXIT

# I segnali hanno un gestore proprio che esce con `128 + n`. Senza, un `INT`
# arriverebbe a `pulizia` con lo stato dell'ultimo comando riuscito — cioe' zero
# — e una qualificazione interrotta a meta' stamperebbe VINTO.
for _segnale in INT TERM HUP; do
  # shellcheck disable=SC2064
  trap "exit \$((128 + \$(kill -l "$_segnale")))" "$_segnale"
done

# --- le due immagini --------------------------------------------------------
echo "== immagine di produzione (release, senza internals) =="
CARGO_TARGET_DIR="$TARGET_PRODUZIONE" cargo build -p plenora-cli --release --locked
WORKER="$(readlink -f "$TARGET_PRODUZIONE/release/plenora-data-tools")"
DIGEST="$(sha256sum "$WORKER" | cut -d' ' -f1)"
echo "worker:  $WORKER"
echo "digest:  $DIGEST"

echo "== immagine di qualificazione (supervisore e spawner) =="
RUSTFLAGS='--cfg qualificazione_isolamento' \
  cargo build --locked --features internals --example qualificazione_isolamento
SUPERVISORE="$RADICE/target/debug/examples/qualificazione_isolamento"

# --- il dominio -------------------------------------------------------------
echo "== dominio =="
echo "+memory" >"$PUNTO/cgroup.subtree_control" 2>/dev/null || true
DOMINIO="$PUNTO/$NOME"
mkdir "$DOMINIO"
# Il dominio **non** si da' al worker: se potesse scriverlo, l'identita' distinta
# non servirebbe a niente — riscriverebbe da se' il tetto che lo governa. Il
# preflight lo pretende, e lo verifica invece di fidarsi. Chi ci mette dentro il
# worker e' lo spawner, che a quel punto e' ancora privilegiato.
echo "dominio: $DOMINIO   worker come $CHI ($UID_W:$GID_W)   tetto: $TETTO_BYTE byte"

# --- la fixture -------------------------------------------------------------
STANZA="$(mktemp -d -t plenora-sotto-limite.XXXXXXXX)"
chmod 755 "$STANZA"
INGRESSO="$STANZA/citta.arrow"
TEMPORANEO="$STANZA/artefatto.arrow"

# --- il percorso ------------------------------------------------------------
echo "== percorso =="
ESITO=0
# Il digest che si **dichiara** al supervisore. E' misurato qui, da fuori; il
# supervisore ne fa una misura propria sul binario che esegue davvero, e
# l'oracolo confronta le due. E' l'indipendenza delle due misure a rendere il
# confronto una prova: un processo che misura e poi si confronta col proprio
# valore direbbe sempre di si'.
ATTESO="$DIGEST"
timeout --signal=KILL 300 "$SUPERVISORE" sotto-limite \
  "$DOMINIO" "$PUNTO" "$TETTO_BYTE" "$UID_W" "$GID_W" "$INGRESSO" "$TEMPORANEO" "$ATTESO" \
  -- "$WORKER" plenora-worker-1 || ESITO=$?

DOPO="$(sha256sum "$WORKER" | cut -d' ' -f1)"
if [[ "$DIGEST" != "$DOPO" ]]; then
  echo "PERSO: l'immagine e' cambiata durante il percorso" >&2
  exit 1
fi
if [[ "$ESITO" -ne 0 ]]; then
  echo "PERSO: il percorso sotto limite non ha qualificato l'immagine (uscita $ESITO)" >&2
  exit "$ESITO"
fi

# Il verdetto **non si stampa qui**: da qui in poi manca ancora la pulizia, e
# dirlo adesso vorrebbe dire dire «VINTO» prima di sapere se il dominio si
# smonta. Un'uscita rossa con un testo che contiene VINTO non e' un verdetto:
# chi legge sceglie a quale dei due credere, e le due fonti si contraddicono.
# Qui si **arma** soltanto; a stampare sara' `pulizia`, che e' l'ultima a sapere.
QUALIFICATO=1
