#!/usr/bin/env bash
# Tocca una decisione per volta del **qualificatore**, e pretende che diventi
# rosso.
#
# Due mutazioni rompono un passo che deve esserci — la cessione della proprieta'
# delle pipe, l'imposizione della variabile del canale. La terza fa il contrario:
# dichiara un guasto di pulizia su un percorso che riesce, e pretende che **cambi
# l'esito**. La quarta dichiara al supervisore un digest che non e' quello
# dell'immagine, e distingue un confronto vero da un confronto con se stessi. La
# quinta rende illeggibile la fonte della quiescenza, e distingue «vuoto» da
# «non l'ho potuto guardare». Sono i versi della stessa domanda: che il
# qualificatore guardi cio' che dice di guardare.
#
# PERCHE' SI CONTANO ANCHE I DOMINI RESIDUI
#
#   Perche' «il qualificatore e' diventato rosso» e «il qualificatore non ha
#   sporcato la macchina» sono due cose, e un mutante puo' ottenere la prima
#   fallendo la seconda. Un cgroup lasciato indietro non si vede nel codice
#   d'uscita, e il giro dopo lo troverebbe li'.
#
# PERCHE' NON STANNO NELL'HARNESS DELLE MUTAZIONI
#
#   Perche' quello giudica col comando `cargo test`, e queste decisioni nessun
#   caso di unita' le attraversa: il passaggio della proprieta' delle pipe e la
#   variabile del canale si vedono solo quando un worker vero, con altre
#   credenziali, prova a riaprire i propri estremi dentro un dominio vero. Un
#   mutante di quel tipo nell'harness sopravviverebbe sempre — non perche' la
#   batteria sia debole, ma perche' sta guardando altrove.
#
#   Il giudice giusto e' quindi il qualificatore stesso: si tocca una cosa, si
#   lancia, e si pretende che **non** dica VINTO.
#
# PERCHE' SI PRETENDONO DUE COSE, E NON UNA
#
#   Il **codice d'uscita** e' cio' su cui una campagna decide; il **testo** e'
#   cio' che legge una persona. Sono due canali, e un qualificatore che uscisse
#   rosso stampando comunque «VINTO» direbbe a ciascuno l'opposto di cio' che
#   dice all'altro: chi legge crederebbe al testo, chi automatizza al codice, e
#   nessuno dei due saprebbe di essere in disaccordo con l'altro.
#
#   Per questo la promessa non e' «esce non-zero», e' «non dice VINTO» — e la si
#   pretende **insieme** al codice d'uscita, non al suo posto: il solo testo
#   lascerebbe passare un rosso arrivato per un'altra ragione.
#
# USO
#
#   sudo scripts/mutazioni_del_qualificatore.sh

set -Eeuo pipefail

RADICE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$RADICE"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "serve root: le mutazioni si giudicano con il qualificatore vero, che crea un dominio" >&2
  exit 2
fi
if [[ $# -ne 0 ]]; then
  echo "riga di comando non ammessa: «$*» — questo script non prende argomenti" >&2
  exit 2
fi

SPAWNER=crates/plenora-engine/src/isolamento/spawner.rs
QUALIFICA=scripts/qualifica_sotto_limite.sh

# Dove il qualificatore crea i propri domini, per poter guardare che non ne
# resti nessuno. Si trova come lo trova lui: un montaggio `cgroup2`, e uno solo.
PUNTO="$(awk '$3 == "cgroup2" { print $2; uscite++ } END { exit uscite != 1 }' /proc/self/mounts)" || {
  echo "il montaggio cgroup2 dev'essere uno solo: con piu' di uno non si sa quale si stia guardando" >&2
  exit 2
}
NOME_DEL_DOMINIO='plenora-sotto-limite-*'

# Quanti domini del qualificatore ci sono adesso.
#
# PERCHE' SI CONTANO
#
#   Perche' un mutante puo' far uscire rosso il qualificatore **e** lasciare un
#   cgroup sulla macchina, e le due cose sono difetti diversi: il rosso e' cio'
#   che si pretende, il residuo e' cio' che nessuno ha chiesto. Contarli
#   separatamente evita di dichiarare vinto un giro che ha sporcato la macchina
#   per il giro dopo.
domini_residui() {
  find "$PUNTO" -maxdepth 1 -type d -name "$NOME_DEL_DOMINIO" 2>/dev/null | wc -l
}

# I due tetti, e perche' sono due.
#
#   Quello per giro ferma un qualificatore che non finisce — una compilazione
#   bloccata, un figlio che nessuno raccoglie. Quello totale ferma la corsa: dei
#   giri tutti appena sotto il proprio tetto sommerebbero un'attesa senza fine
#   pur non superandolo mai una volta.
#
#   Il segnale e' `TERM` e non `KILL`, con trenta secondi di grazia: il
#   qualificatore ha un gestore che porta la sua pulizia a termine, e un `KILL`
#   diretto lascerebbe in giro proprio il dominio che questo script pretende
#   sappia smontarsi.
TETTO_PER_GIRO=1200
TETTO_TOTALE=5400

VINTI=0
PERSI=0
BERSAGLIO=""
COPIA=""
IMPRONTA_SANA=""

# Rimette il bersaglio nello stato sano, dalla copia.
#
# PERCHE' UNA COPIA SU DISCO E NON UNA VARIABILE
#
#   Perche' `$(cat file)` toglie le newline finali e `printf '%s'` non le rimette:
#   un albero «ripristinato» cosi' differisce dall'originale di un byte, che
#   basta a cambiare l'impronta di una baseline. Una copia byte per byte non ha
#   questa asimmetria.
#
# PERCHE' IL `touch`
#
#   Perche' cargo decide se ricompilare confrontando le date, e il file rimesso
#   a posto sarebbe piu' **vecchio** del binario costruito dal mutante: cargo lo
#   giudicherebbe fresco e terrebbe l'eseguibile mutato. Il giro successivo
#   misurerebbe allora il mutante precedente credendo di misurare l'albero sano.
ripristina() {
  if [[ -n "$BERSAGLIO" && -n "$COPIA" && -f "$COPIA" ]]; then
    mv -f "$COPIA" "$BERSAGLIO"
    touch "$BERSAGLIO"
  fi
  BERSAGLIO=""
  COPIA=""
}
trap 'ripristina' EXIT

# Applica una mutazione, lancia il qualificatore, pretende il rosso.
prova_mutante() {
  local nome="$1" file="$2" sano="$3" malato="$4"
  local riscontri
  riscontri="$(grep -cF -- "$sano" "$file" || true)"
  if [[ "$riscontri" -ne 1 ]]; then
    echo "PERSO: «$nome»: $riscontri riscontri del frammento sano invece di uno" >&2
    PERSI=$((PERSI + 1))
    return
  fi

  BERSAGLIO="$file"
  COPIA="$file.sano"
  cp "$file" "$COPIA"
  # La sostituzione e' **letterale**, e va detto perche' non lo e' quasi mai: i
  # frammenti contengono parentesi e punti, e sia `sed` sia `sub()` di `awk`
  # leggerebbero il testo cercato come un'espressione regolare — cioe' non
  # troverebbero la riga che c'e' davvero. `index` piu' `substr` cercano e
  # tagliano sui caratteri, che e' quello che qui si vuole.
  awk -v sano="$sano" -v malato="$malato" '
    {
      dove = index($0, sano)
      if (dove > 0) {
        $0 = substr($0, 1, dove - 1) malato substr($0, dove + length(sano))
      }
      print
    }' "$file" >"$file.mutato"
  mv "$file.mutato" "$file"

  local esito=0
  timeout --signal=TERM --kill-after=30 "$TETTO_PER_GIRO" \
    bash "$QUALIFICA" >/tmp/mutante-qualificatore.txt 2>&1 || esito=$?
  ripristina

  # L'albero torna alla baseline, e lo si **verifica** invece di darlo per fatto.
  # Un ripristino che fallisse in silenzio farebbe giudicare il mutante
  # successivo su un albero che porta ancora il precedente, e i due difetti si
  # coprirebbero a vicenda: il secondo risultato non direbbe piu' niente.
  local ora
  ora="$(python3 scripts/mutazioni_isolamento.py --impronta)"
  if [[ "$ora" != "$IMPRONTA_SANA" ]]; then
    echo "PERSO: dopo «$nome» l'albero non e' tornato alla baseline" >&2
    echo "  sana    $IMPRONTA_SANA" >&2
    echo "  adesso  $ora" >&2
    PERSI=$((PERSI + 1))
    return
  fi

  # Nessun dominio lasciato indietro. Vale anche sul rosso che questo giro
  # pretende: il rosso e' l'esito atteso, il residuo non lo e'.
  local residui
  residui="$(domini_residui)"
  if [[ "$residui" -ne 0 ]]; then
    echo "PERSO: «$nome» ha lasciato $residui dominio/i sotto $PUNTO" >&2
    find "$PUNTO" -maxdepth 1 -type d -name "$NOME_DEL_DOMINIO" >&2
    PERSI=$((PERSI + 1))
    return
  fi

  # Due pretese, non una. Il **codice d'uscita** e' cio' su cui una campagna
  # decide; il **testo** e' cio' che legge una persona. Un qualificatore che
  # uscisse rosso stampando comunque VINTO passerebbe il primo controllo e
  # direbbe a chi legge l'opposto: la promessa non e' «esce non-zero», e'
  # «non dice VINTO».
  local dice_vinto=0
  if grep -q '^VINTO' /tmp/mutante-qualificatore.txt; then
    dice_vinto=1
  fi
  if [[ "$esito" -eq 0 || "$dice_vinto" -eq 1 ]]; then
    echo "PERSO: «$nome» e' sopravvissuto (uscita $esito, VINTO nel testo: $dice_vinto)" >&2
    PERSI=$((PERSI + 1))
  else
    echo "vinto: «$nome» esce $esito e non dice VINTO"
    VINTI=$((VINTI + 1))
  fi

  if [[ "$SECONDS" -gt "$TETTO_TOTALE" ]]; then
    echo "PERSO: tetto totale superato dopo «$nome» ($SECONDS s)" >&2
    PERSI=$((PERSI + 1))
    return 1
  fi
}

# L'impronta dell'albero sano si prende **prima** di toccare qualunque cosa: e'
# il riferimento contro cui ogni ripristino si verifica.
IMPRONTA_SANA="$(python3 scripts/mutazioni_isolamento.py --impronta)"
echo "impronta sana: $IMPRONTA_SANA"

# Si parte da una macchina pulita, o non si sa di chi sia il residuo che si
# trova alla fine.
if [[ "$(domini_residui)" -ne 0 ]]; then
  echo "DOMINI GIA' PRESENTI sotto $PUNTO: un residuo trovato dopo non si saprebbe attribuire." >&2
  find "$PUNTO" -maxdepth 1 -type d -name "$NOME_DEL_DOMINIO" >&2
  exit 1
fi

echo "== l'albero sano qualifica =="
timeout --signal=TERM --kill-after=30 "$TETTO_PER_GIRO" \
  bash "$QUALIFICA" >/tmp/sano-qualificatore.txt 2>&1 || {
  echo "L'ALBERO SANO E' GIA' ROSSO: non si misura niente. Fermo." >&2
  tail -5 /tmp/sano-qualificatore.txt >&2
  exit 1
}
if ! grep -q '^VINTO' /tmp/sano-qualificatore.txt; then
  echo "L'ALBERO SANO ESCE ZERO SENZA DIRE VINTO: il verdetto non e' quello che si crede." >&2
  exit 1
fi
echo "sano: VINTO"

echo "== le mutazioni =="

# 1. La proprieta' delle pipe non passa al worker.
#
#    Un `chown` con `None` non e' un errore: e' un cambio che non cambia niente.
#    E' la forma piu' vicina a «il passo c'e' ma non fa il suo lavoro», che e'
#    proprio il difetto che si vuole poter distinguere da «il passo manca».
prova_mutante "proprieta' delle pipe non ceduta" "$SPAWNER" \
  '            Some(Uid::from_raw(worker.uid)),' \
  '            None,'

# 2. La variabile del canale non si impone.
#
#    Il valore si calcola ancora, e finisce in un nome che nessuno legge: il
#    worker trova l'ambiente come gli e' arrivato — cioe' senza — e non sa dove
#    siano i propri estremi.
prova_mutante "variabile del canale non imposta" "$SPAWNER" \
  '                canale::VARIABILE_DEL_CANALE,' \
  '                "PLENORA_CANALE_ALTROVE",'

# 3. La pulizia dichiara un guasto, e il qualificatore deve accorgersene.
#
#    Non si rompe la pulizia — un dominio che si rimuove regolarmente resterebbe
#    verde comunque, e il mutante non direbbe niente. Si dichiara il guasto e si
#    pretende che **cambi l'esito**: e' quella la proprieta', ed e' l'unica che
#    un percorso riuscito puo' mettere alla prova.
prova_mutante "un guasto di pulizia dichiarato" "$QUALIFICA" \
  '  local guasto=0' \
  '  local guasto=1'

# 4. Il digest dichiarato non e' quello dell'immagine.
#
#    E' la mutazione che distingue un confronto da una tautologia. Se il
#    supervisore misurasse l'immagine e la confrontasse col **proprio** valore,
#    direbbe VINTO qualunque cosa gli si dichiari, e questo mutante
#    sopravvivrebbe. Sopravvive anche se il digest non arriva affatto fino
#    all'oracolo: e' lo stesso difetto visto da un'altra parte.
prova_mutante "digest dichiarato diverso dall'immagine" "$QUALIFICA" \
  'ATTESO="$DIGEST"' \
  'ATTESO="0000000000000000000000000000000000000000000000000000000000000000"'

# 5. La quiescenza non si puo' osservare.
#
#    E' il caso fail-open. Senza tre stati distinti, «non l'ho potuto guardare»
#    sarebbe indistinguibile da «vuoto»: il dominio si rimuoverebbe lo stesso —
#    `cgroup.kill` ha gia' fatto il suo — e la pulizia direbbe quiescente avendo
#    misurato niente. Qui `cgroup.events` diventa un file che non c'e', e si
#    pretende il rosso.
prova_mutante "quiescenza non osservabile" "$QUALIFICA" \
  '  local eventi="$1/cgroup.events"' \
  '  local eventi="$1/cgroup.questo-file-non-esiste"'

echo
echo "mutanti del qualificatore: $((VINTI + PERSI))"
echo "  vinti (esce rosso e non dice VINTO): $VINTI"
echo "  PERSI:                               $PERSI"
[[ "$PERSI" -eq 0 ]]
