#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Mutazioni mirate sul supervisore: la batteria discrimina, oppure non prova.

E' uno script di qualificazione **manuale**, come il gate ostile: si esegue su
una macchina Linux dedicata prima di chiudere il lavoro sul supervisore, non a
ogni commit. Non entra in CI perche' ogni mutante ricompila `plenora-engine`, e
un gate che dura mezz'ora smette di essere eseguito.

# Che cosa misura, e perche' non basta la batteria verde

Una batteria verde dice che il codice passa i propri casi. Non dice che i casi
**distinguano**: un caso che non guarda la proprieta' che dichiara resta verde
anche quando quella proprieta' sparisce. La mutazione capovolge la domanda —
si rompe una decisione per volta, e si pretende che qualcuno se ne accorga.

Chi sopravvive non e' un difetto del codice: e' un difetto della batteria, e
indica esattamente quale proprieta' nessun caso sta guardando.

# Le due impronte, che non sono la stessa cosa

L'**impronta dell'albero Rust** e' quella che governa questo script: i soli
`.rs` sotto `crates/`, che sono i file che i mutanti toccano. La calcola
[`impronta`], la stampa `--impronta`, e la pretende `--prepara`.

L'**impronta del delta** di una PR e' un'altra cosa e sta altrove: comprende
documenti, manifesti Cargo e questo script stesso. Serve a verificare un
trasferimento, non a riconoscere un albero mutato. Confonderle darebbe una
falsa sicurezza in tutte e due le direzioni: una modifica a un documento
farebbe rifiutare una baseline valida, e una modifica a un `.rs` fuori da
`crates/` passerebbe inosservata.

# Le regole, e perche' sono queste

- **Il giudizio e' sul codice di uscita**, in due fasi, e mai sul testo. Prima
  `cargo test --no-run`: se non compila, il mutante e' stato rifiutato dal
  compilatore. Poi l'esecuzione dei casi. Distinguere le due cose cercando
  `error[E` nell'output sarebbe un giudizio sul testo travestito: un caso che
  stampasse quella stringa — riportando cio' che ha osservato — si farebbe
  contare come errore di compilazione.

- **L'uscita di cargo va su un file, mai su una pipe.** Leggere da una pipe
  aspetta l'EOF, non l'uscita del processo: un nipote che sopravvive alla
  mutazione — un `/bin/sleep` che nessuno termina piu' — tiene aperto quel
  descrittore, e il tetto di tempo misura la vita del nipote invece
  dell'esecuzione. Il mutante sembra allora **appeso** invece che ucciso, e chi
  guarda pensa a uno stallo della macchina.

- **Dopo ogni uscita di cargo si guarda il gruppo**, non solo dopo un tetto di
  tempo scaduto. `cargo` che finisce ordinatamente non dice niente sui nipoti: un
  caso mutato puo' terminare lasciando vivo un `/bin/sleep` che avrebbe dovuto
  uccidere — e' esattamente la classe di difetto che questi mutanti cercano, e
  sarebbe assurdo che l'harness la lasciasse passare su di se'. Se il gruppo e'
  ancora abitato lo si ferma, prima del ripristino.

  Quando lo si ferma: **si ferma il gruppo, si raccoglie il figlio diretto, e si
  pretende che il gruppo si svuoti.** La formulazione e' esatta apposta: gli
  unici processi che questo script puo' *raccogliere* sono i propri figli, e
  `cargo` e' l'unico. Di `rustc` e dei binari di test si puo' soltanto chiedere
  la fine e verificarla — sono nipoti, e chi li aspetta e' `cargo`. Scrivere
  «raccoglie il gruppo» prometterebbe una cosa che nessun processo puo' fare per
  i figli altrui.

  Se il gruppo **non** si svuota, i sorgenti **non** si ripristinano e il giro si
  ferma: rimetterli con dei superstiti vivi romperebbe l'invariante che questa
  regola esiste per tenere. L'albero resta mutato, e la riparazione del giro
  successivo lo riconosce.

- **L'elenco dei mutanti e' canonico.** Gli identificativi stanno scritti a
  parte, e la tabella deve corrispondervi: stesso numero, stessi nomi, stesso
  ordine. Un mutante cancellato per sbaglio farebbe altrimenti scendere il
  denominatore senza che nessuno se ne accorga, e il punteggio salirebbe
  **perche' si misura di meno**. Per la stessa ragione il blocco richiesto deve
  produrre **esattamente** il numero di risultati che copre.

- **I trentadue alberi mutati devono essere distinti**, e lo si accerta prima di
  cominciare. E' la precondizione perche' il conteggio delle corrispondenze in
  [`ripara`] significhi qualcosa: se due mutanti producessero lo stesso albero,
  una campagna interrotta lascerebbe uno stato che la riparazione non sa
  attribuire, e si fermerebbe per sempre su un albero che invece saprebbe
  rimettere a posto. Verificarlo solo altrove non basta: qui serve **prima** del
  primo mutante.

- **La riparazione e' fail-closed, e non scrive per scoprire.** Un giro ucciso a
  meta' lascia una mutazione sul posto. Si confronta l'impronta corrente con
  quella di tutti e trentadue gli alberi mutati **virtuali** — calcolati senza
  toccare niente — e si ripristina solo se la corrispondenza e' **esattamente
  una**. Zero corrispondenze vuol dire che c'e' dell'altro; piu' di una, che i
  mutanti non si distinguono. In entrambi i casi si dichiara e non si tocca:
  sovrascrivere un albero che non si sa cosa sia cancellerebbe una modifica vera
  insieme a una mutazione.

- **L'impronta identifica l'insieme dei file, non solo il loro numero.** Entrano
  nel digest i **percorsi** effettivamente presenti e i loro contenuti: cosi'
  un file sostituito con un altro cambia l'impronta, mentre un conteggio la
  lascerebbe uguale.

  Ogni pezzo entra **preceduto dalla propria lunghezza**. Concatenare senza
  confini strutturali lascerebbe che la fine di un contenuto e l'inizio del
  percorso successivo si possano spartire diversamente fra due alberi che
  darebbero lo stesso digest: un separatore che il contenuto puo' contenere non
  e' un confine. E una radice senza `crates/`, o senza nemmeno un sorgente, e'
  un rifiuto: l'hash dell'insieme vuoto e' un valore perfettamente valido, e
  sarebbe il verde piu' facile da ottenere per sbaglio.

# Che cosa rende immutabile la baseline, e che cosa no

**Non i permessi.** Toglierne la scrittura protegge dagli incidenti — una
sovrascrittura distratta, un `cp` di troppo — ma il proprietario puo' sempre
rimetterla, e `root` non li guarda nemmeno.

L'autorita' e' il **manifesto**: l'impronta stabilita *prima* di `--prepara` e
passata sulla riga di comando. Al momento di fissare la baseline si verifica che
l'albero sia quello dichiarato; a ogni avvio successivo si verifica che la
**copia** corrisponda ancora al proprio manifesto, ricalcolandola dai file. Una
baseline gia' presente si verifica o si rifiuta, e non si sovrascrive mai:
quella sul disco potrebbe essere l'unica copia di riferimento di un giro in
corso.

# Uso

    python3 scripts/mutazioni_supervisore.py --impronta
    python3 scripts/mutazioni_supervisore.py --prepara <impronta-attesa>
    python3 scripts/mutazioni_supervisore.py [primo [ultimo]]

`--impronta` stampa l'impronta dell'albero Rust corrente, che e' cio' che
`--prepara` pretende. Senza quell'argomento `--prepara` rifiuta: una baseline
che si autocertifica non certifica niente.

# Codici di uscita

    0  nessun mutante compilabile e' sopravvissuto o scaduto, albero alla
       baseline. I mutanti che il compilatore rifiuta non sono un difetto della
       batteria e non impediscono lo zero
    1  argomenti non validi, elenco non canonico, baseline assente,
       albero sano gia' rosso, o conteggio dei risultati inatteso
    2  stato non riconosciuto, ripristino imperfetto, o processi superstiti:
       in nessuno di questi casi si e' scritto per indovinare
    3  qualcuno e' sopravvissuto o e' scaduto
"""
import hashlib
import io
import os
import shutil
import signal
import stat
import subprocess
import sys
import time

#: La radice dell'albero da mutare, e la copia di riferimento accanto.
RADICE = os.environ.get('MUTAZIONI_RADICE', os.path.expanduser('~/pr8'))
BASELINE = RADICE + '-baseline'
USCITA = os.path.join(os.path.dirname(BASELINE), 'mutante-uscita.txt')

#: Il comando della batteria. E' una costante e non una riga scritta dentro
#: [`esegui`] perche' i casi di `test_mutazioni_supervisore.py` la sostituiscono
#: con un comando che lascia un nipote vivo: la regola sui superstiti si prova
#: cosi', in un secondo, invece che sperando in un mutante che si comporti male.
COMANDO_BASE = ['cargo', 'test', '-p', 'plenora-engine', '--lib', '--locked']

#: Il filtro dei casi. Il perimetro dei mutanti e' il supervisore, e far girare
#: l'intera batteria per ognuno moltiplicherebbe il tempo senza aggiungere
#: discriminazione: cio' che vive fuori da `isolamento` non tocca queste
#: decisioni.
FILTRO = 'isolamento'

#: Oltre questo, un mutante non e' lento: e' appeso. Il valore e' largo apposta
#: — una ricompilazione onesta ci sta comodamente dentro — perche' un tetto
#: stretto trasformerebbe una macchina carica in un falso superstite.
TETTO_SECONDI = 900

#: Quanto si aspetta che il gruppo di processi si svuoti dopo averlo ucciso.
#: `SIGKILL` non e' istantaneo: fra il segnale e l'uscita c'e' il kernel.
ATTESA_DEL_GRUPPO = 30

I = 'crates/plenora-engine/src/isolamento'
COND = f'{I}/macchina/conduzione.rs'
MACC = f'{I}/macchina.rs'
CODA = f'{I}/macchina/coda.rs'
PROD = f'{I}/macchina/produttori.rs'
FIGL = f'{I}/figlio.rs'

#: L'elenco **canonico**. La tabella qui sotto deve corrispondervi: e' cio' che
#: impedisce a un mutante di sparire senza rumore.
IDENTIFICATORI = [
    'M01', 'M02', 'M03', 'M04', 'M05', 'M06', 'M07', 'M08',
    'M09', 'M10', 'M11', 'M12', 'M13', 'M14', 'M15', 'M16',
    'M17', 'M18', 'M19', 'M20', 'M21', 'M22', 'M23', 'M24',
    'M25', 'M26', 'M27', 'M28', 'M29', 'M30', 'M31', 'M32',
]

#: (identificativo, nome, file, sano, malato). Ogni `sano` deve comparire
#: **una sola volta** nel file: un frammento ambiguo muterebbe un punto diverso
#: da quello dichiarato, e il nome del mutante direbbe il falso.
MUTANTI = [
    # --- la barriera causale -------------------------------------------------
    ('M01', 'barriera: la quiescenza non blocca piu', MACC,
     '        if !self.quiescente {\n            return Err(Impedimento::BarrieraIncompleta(manca));\n        }',
     '        if false {\n            return Err(Impedimento::BarrieraIncompleta(manca));\n        }'),
    ('M02', 'barriera: i difetti non entrano piu', MACC,
     '        for difetto in difetti_della_conduzione {\n            manca.push(format!("difetto della conduzione: {difetto}"));\n        }',
     '        for difetto in difetti_della_conduzione {\n            let _ = difetto;\n        }'),
    ('M03', "barriera: la quiescenza non e' piu' richiesta al resto", MACC,
     '        if !self.quiescente {\n            manca.push("il dominio non e\' quiescente".to_owned());\n        }',
     '        if false {\n            manca.push("il dominio non e\' quiescente".to_owned());\n        }'),
    ('M04', 'barriera: da_verificare passa con la barriera incompleta', MACC,
     '        if matches!(classificato, EsitoClassificato::DaVerificare { .. }) && !manca.is_empty() {',
     '        if matches!(classificato, EsitoClassificato::DaVerificare { .. }) && false {'),

    # --- la raccolta dei fatti fermi -----------------------------------------
    ('M05', "fatti fermi: non si raccolgono prima dell'evidenza", COND,
     '    for fatto in coda.raccogli_i_fermi() {\n        registro.applica(fatto);\n    }',
     '    for fatto in coda.raccogli_i_fermi() {\n        let _ = fatto;\n    }'),

    # --- l'evidenza si legge solo su un dominio quiescente --------------------
    ('M06', 'evidenza: si legge anche sul dominio abitato', COND,
     '    if !dominio_quiescente {',
     '    if false {'),

    # --- la forzatura e cio' che segue ---------------------------------------
    ('M07', 'forzatura: si smette di ascoltare subito dopo', COND,
     '                continue;\n            }\n            if gia_forzato && std::time::Instant::now() >= scadenza {',
     '                break;\n            }\n            if gia_forzato && std::time::Instant::now() >= scadenza {'),
    ('M08', 'forzatura: si forza a ogni giro', COND,
     '            if !gia_forzato && std::time::Instant::now() >= scadenza {',
     '            if std::time::Instant::now() >= scadenza {'),
    ('M09', 'forzatura: si forza senza aspettare il margine', COND,
     '            if let Some(quando) = std::time::Instant::now().checked_add(margine_di_cortesia) {\n                scadenza_di_cortesia = Some(quando);',
     '            if let Some(_quando) = std::time::Instant::now().checked_add(margine_di_cortesia) {\n                scadenza_di_cortesia = Some(std::time::Instant::now());'),
    ('M10', 'overflow: la seconda scadenza torna muta', COND,
     '                        "attesa della quiescenza: {} ms non sono rappresentabili come scadenza, e non si aspetta",',
     '                        "attesa della quiescenza: {} ms, e non si aspetta",'),
    ('M11', 'overflow: il margine torna muto', COND,
     '"margine di cortesia: {} ms non sono rappresentabili come scadenza, e si \\\n                     forza subito",',
     '"margine di cortesia: {} ms, e si forza subito",'),

    # --- la cancellazione ----------------------------------------------------
    ('M12', "cancellazione: l'Annulla non parte piu' sul filo", COND,
     '            if registro.cancellazione_richiesta() {\n                if let Err(motivo) = manda_annulla(ingresso_verso_il_worker) {',
     '            if false {\n                if let Err(motivo) = manda_annulla(ingresso_verso_il_worker) {'),
    ('M13', "cancellazione: il tempo scaduto manda comunque l'Annulla", COND,
     '            if registro.cancellazione_richiesta() {',
     '            if registro.si_deve_chiudere() {'),

    # --- la conversione dell'uscita ------------------------------------------
    ('M14', 'uscita: NonRappresentabile si scarta come None', COND,
     '        Some(Uscita::NonRappresentabile) => {\n            let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {',
     '        Some(Uscita::NonRappresentabile) if false => {\n            let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {'),
    ('M15', 'uscita: il codice diventa un segnale', COND,
     '        Some(Uscita::Codice(codice)) => {\n            let _ = raccoglitore.manda(Fatto::UscitaDelWorker(UscitaOsservata::Codice(codice)));',
     '        Some(Uscita::Codice(codice)) => {\n            let _ = raccoglitore.manda(Fatto::UscitaDelWorker(UscitaOsservata::Segnale(codice)));'),

    # --- la proprieta' del figlio --------------------------------------------
    ('M16', 'figlio: la guardia non risale (cammino ordinario)', COND,
     '            difetti.raccolta = riunisci(&quali);\n            difetti.figlio_non_raccolto = Some(guardia);\n            None',
     '            difetti.raccolta = riunisci(&quali);\n            guardia.smonta();\n            None'),
    ('M17', 'figlio: la guardia non risale (rinuncia)', COND,
     '            difetti.raccolta = riunisci(&quali);\n            difetti.figlio_non_raccolto = Some(guardia);\n        }',
     '            difetti.raccolta = riunisci(&quali);\n            guardia.smonta();\n        }'),
    ('M18', 'figlio: il processo esce dalla guardia prima della raccolta', FIGL,
     '        let Some(processo) = self.processo.as_mut() else {',
     '        let Some(mut processo_estratto) = self.processo.take() else {'),
    ('M19', "figlio: la terminazione rifiutata non e' un difetto", FIGL,
     '            Err(errore) => difetti.push(format!("non si riesce a terminare il figlio: {errore}")),',
     '            Err(_errore) => (),'),
    ('M20', 'figlio: la resa non termina', FIGL,
     '                (pid, ultimo_tentativo_di_terminazione(&mut processo))',
     '                (pid, "segnale di terminazione inviato; uscita non osservata".to_owned())'),
    ('M21', 'figlio: la resa dice terminato', FIGL,
     '        Ok(()) => "segnale di terminazione inviato; uscita non osservata".to_owned(),',
     '        Ok(()) => "terminato".to_owned(),'),
    ('M22', "figlio: InvalidInput diventa gia' uscito", FIGL,
     '            "processo non piu\' terminabile; uscita non osservata".to_owned()',
     '            "gia\' uscito".to_owned()'),
    ('M23', 'figlio: la sentinella non termina', FIGL,
     '        let colpo = ultimo_tentativo_di_terminazione(&mut processo);\n        eprintln!(\n            "plenora: il figlio {pid} e\' sfuggito alla guardia',
     '        let colpo = "segnale di terminazione inviato; uscita non osservata".to_owned();\n        eprintln!(\n            "plenora: il figlio {pid} e\' sfuggito alla guardia'),

    # --- la rinuncia a una nascita parziale -----------------------------------
    ('M24', 'rinuncia: non guarda la quiescenza', COND,
     '    guarda_che_si_sia_svuotato(\n        osservatore,\n        attesa_della_quiescenza,\n        &mut raccoglitore,\n        &mut difetti,\n    );',
     '    drop(osservatore);'),
    ('M25', 'rinuncia: non chiude il dominio', COND,
     '    if let Err(motivo) = terminatore.termina() {\n        difetti.terminazione = Some(motivo);\n    }\n\n    // 2. E si **guarda**',
     '    if false {\n        difetti.terminazione = terminatore.termina().err();\n    }\n\n    // 2. E si **guarda**'),
    ('M26', 'rinuncia: non drena', COND,
     '    let (tardivi, difetto_di_drenaggio) = coda.chiudi_e_drena_entro(tetto_del_drenaggio);\n    difetti.drenaggio = difetto_di_drenaggio;\n    let mut registro = Registro::default();',
     '    let (tardivi, difetto_di_drenaggio) = (Vec::new(), None);\n    difetti.drenaggio = difetto_di_drenaggio;\n    drop(coda);\n    let mut registro = Registro::default();'),
    ('M27', 'rinuncia: senza osservatore tace', COND,
     '        let _ = raccoglitore.manda(Fatto::OsservazioneImpossibile {\n            chi: "quiescenza",\n            motivo: "non c\'e\' nessuno che possa guardare il dominio".to_owned(),\n        });',
     '        ()'),
    ('M28', 'rinuncia: il tempo finito non si riporta', COND,
     '            difetti.resoconti.push(format!(\n                "il dominio non si e\' svuotato entro {} ms dalla forzatura",\n                entro.as_millis()\n            ));',
     '            ()'),

    # --- i produttori --------------------------------------------------------
    ('M29', 'annullatore: il lucchetto avvelenato fa rinunciare', PROD,
     '        self.bocchetta\n            .lock()\n            .unwrap_or_else(std::sync::PoisonError::into_inner)\n            .take()',
     '        self.bocchetta.lock().ok()?.take()'),
    ('M30', "sorvegliante: l'osservatore non torna indietro", PROD,
     '        Err(errore) => Err((\n            errore,\n            cella\n                .lock()\n                .unwrap_or_else(std::sync::PoisonError::into_inner)\n                .take(),\n        )),',
     '        Err(errore) => Err((errore, None)),'),
    ('M31', 'sorvegliante: la quiescenza si accoda in ritardo', PROD,
     '                Ok(true) => {\n                    let _ = bocchetta.manda(Fatto::DominioQuiescente);',
     '                Ok(true) => {\n                    std::thread::sleep(passo);\n                    let _ = bocchetta.manda(Fatto::DominioQuiescente);'),

    # --- la coda -------------------------------------------------------------
    ('M32', "coda: i fatti fermi non si possono piu' raccogliere", CODA,
     '    pub(super) fn raccogli_i_fermi(&self) -> Vec<Fatto> {',
     '    #[allow(dead_code)]\n    pub(super) fn raccogli_i_fermi_inutile(&self) -> Vec<Fatto> {'),
]


# --- l'elenco canonico -------------------------------------------------------

def elenco_e_canonico():
    """La tabella corrisponde agli identificativi, o non si parte."""
    presenti = [m[0] for m in MUTANTI]
    if presenti == IDENTIFICATORI:
        return True
    mancanti = [x for x in IDENTIFICATORI if x not in presenti]
    estranei = [x for x in presenti if x not in IDENTIFICATORI]
    print('ELENCO NON CANONICO: la tabella non corrisponde agli identificativi.')
    print(f'  attesi {len(IDENTIFICATORI)}, presenti {len(presenti)}')
    if mancanti:
        print(f'  mancanti: {", ".join(mancanti)}')
    if estranei:
        print(f'  estranei: {", ".join(estranei)}')
    if not mancanti and not estranei:
        print('  stessi identificativi, ordine diverso')
    return False


def tutti_applicabili_una_volta(radice):
    """Ogni `sano` compare esattamente una volta nel proprio file.

    Si controlla **prima** di eseguire, e sulla baseline: un frammento diventato
    ambiguo, o sparito perche' il codice e' cambiato, va detto subito. Scoprirlo
    a meta' giro lascerebbe un blocco parziale con un conteggio che non si sa
    come leggere.
    """
    guai = []
    for identificativo, nome, relativo, sano, _ in MUTANTI:
        percorso = os.path.join(radice, relativo)
        if not os.path.exists(percorso):
            guai.append(f'{identificativo} {nome}: il file non esiste ({relativo})')
            continue
        with io.open(percorso, encoding='utf-8') as aperto:
            quante = aperto.read().count(sano)
        if quante != 1:
            guai.append(f'{identificativo} {nome}: {quante} riscontri invece di uno')
    if guai:
        print('MUTANTI NON APPLICABILI UNA SOLA VOLTA:')
        for riga in guai:
            print(f'  {riga}')
    return not guai


def tutti_i_mutanti_sono_distinti(radice, attesa):
    """I trentadue alberi mutati sono diversi fra loro, e dalla baseline.

    E' la precondizione perche' contare le corrispondenze in [`ripara`]
    significhi qualcosa. Due mutanti che producessero lo stesso albero
    lascerebbero, dopo una campagna interrotta, uno stato che la riparazione non
    sa attribuire: si fermerebbe su un albero che invece saprebbe rimettere a
    posto, e la campagna non ripartirebbe piu' da sola.

    Un mutante che produce **la baseline** e' l'altro caso: non muta niente, e
    un caso che non se ne accorge non e' un difetto della batteria.
    """
    per_impronta = {}
    for identificativo, nome, relativo, sano, malato in MUTANTI:
        percorso = os.path.join(radice, relativo)
        with io.open(percorso, encoding='utf-8') as aperto:
            testo = aperto.read()
        firma = impronta(radice, (relativo, testo.replace(sano, malato)))
        per_impronta.setdefault(firma, []).append((identificativo, nome))

    guai = []
    for firma, quali in per_impronta.items():
        if len(quali) > 1:
            elenco = ', '.join(f'{i} ({n})' for i, n in quali)
            guai.append(f'stesso albero mutato: {elenco}')
        if firma == attesa:
            elenco = ', '.join(i for i, _ in quali)
            guai.append(f'non muta niente: {elenco}')
    if guai:
        print('ALBERI MUTATI NON DISTINTI:')
        for riga in guai:
            print(f'  {riga}')
    return not guai


# --- l'impronta dell'albero Rust ---------------------------------------------

def file_dei_sorgenti(radice):
    """I `.rs` sotto `crates/`, relativi e ordinati."""
    trovati = []
    for cartella, sotto, file in os.walk(os.path.join(radice, 'crates')):
        sotto[:] = sorted(s for s in sotto if s != 'target')
        for nome in sorted(file):
            if nome.endswith('.rs'):
                intero = os.path.join(cartella, nome)
                trovati.append(os.path.relpath(intero, radice).replace(os.sep, '/'))
    return sorted(trovati)


class AlberoSenzaSorgenti(Exception):
    """La radice non ha `crates/`, o non ha nemmeno un `.rs`.

    Non e' un albero da improntare: e' un percorso sbagliato, o una copia mai
    riuscita. L'hash dell'insieme vuoto sarebbe un valore perfettamente valido,
    e due radici vuote coinciderebbero — il verde piu' facile da ottenere per
    sbaglio.
    """


def _con_lunghezza(digest, dati):
    """Un pezzo, preceduto dalla propria lunghezza.

    Senza, la fine di un contenuto e l'inizio del percorso successivo non hanno
    un confine strutturale: due alberi diversi possono spartire gli stessi byte
    in modo diverso e dare lo stesso digest. Un separatore non basta, perche' il
    contenuto puo' contenerlo; la lunghezza no.
    """
    digest.update(str(len(dati)).encode())
    digest.update(b':')
    digest.update(dati)


def impronta(radice, sostituzione=None):
    """L'impronta dell'albero Rust: i **percorsi presenti** e i loro contenuti.

    Entrano nel digest i percorsi effettivamente trovati, non un elenco atteso e
    non il loro numero: cosi' un file sostituito con un altro cambia l'impronta,
    mentre un conteggio la lascerebbe uguale — stesso numero, stessi digest, e
    due alberi diversi con la stessa firma.

    `sostituzione` e' `(relativo, contenuto)` e serve a calcolare l'impronta di
    un albero mutato **senza scriverlo**: e' cio' che permette di riconoscere
    uno stato prima di toccarlo. Rende `None` se quel percorso non e' fra i file
    presenti, perche' allora quella mutazione non e' una spiegazione possibile.

    # Errors

    [`AlberoSenzaSorgenti`] se la radice non ha `crates/` o non ha sorgenti.
    """
    if not os.path.isdir(os.path.join(radice, 'crates')):
        raise AlberoSenzaSorgenti(f'{radice}: non c\'e\' `crates/`')
    percorsi = file_dei_sorgenti(radice)
    if not percorsi:
        raise AlberoSenzaSorgenti(f'{radice}: `crates/` non ha nemmeno un `.rs`')
    if sostituzione is not None and sostituzione[0] not in percorsi:
        return None
    digest = hashlib.sha256()
    for relativo in percorsi:
        _con_lunghezza(digest, relativo.encode('utf-8'))
        if sostituzione is not None and relativo == sostituzione[0]:
            _con_lunghezza(digest, sostituzione[1].encode('utf-8'))
            continue
        with open(os.path.join(radice, relativo), 'rb') as aperto:
            _con_lunghezza(digest, aperto.read())
    return digest.hexdigest()


# --- la baseline -------------------------------------------------------------

def percorso_del_manifesto():
    return os.path.join(BASELINE, 'MANIFESTO')


def leggi_manifesto():
    percorso = percorso_del_manifesto()
    if not os.path.exists(percorso):
        return None, None
    with io.open(percorso, encoding='utf-8') as aperto:
        righe = aperto.read().splitlines()
    if not righe:
        return None, None
    return righe[0], righe[1:]


def baseline_coerente(attesa, elenco):
    """La copia di riferimento corrisponde ancora al proprio manifesto.

    Si ricalcola dai **file della copia**, non dal primo campo del manifesto
    confrontato con qualcos'altro: un manifesto che si limitasse a citare se
    stesso accetterebbe una baseline alterata.
    """
    sua = impronta(BASELINE)
    suoi = file_dei_sorgenti(BASELINE)
    if sua == attesa and suoi == elenco:
        return True
    print('BASELINE ALTERATA: la copia di riferimento non corrisponde al manifesto.')
    print(f'  manifesto {attesa}')
    print(f'  copia     {sua}')
    if suoi != elenco:
        mancanti = [x for x in elenco if x not in suoi]
        estranei = [x for x in suoi if x not in elenco]
        if mancanti:
            print(f'  mancanti nella copia: {len(mancanti)} (primo: {mancanti[0]})')
        if estranei:
            print(f'  estranei nella copia: {len(estranei)} (primo: {estranei[0]})')
    return False


def prepara_baseline(attesa_dichiarata):
    """Fissa la copia di riferimento, dopo aver verificato l'albero.

    Rifiuta se una baseline esiste gia' ed e' diversa: quella sul disco potrebbe
    essere la copia di riferimento di un giro in corso, e sovrascriverla
    toglierebbe l'unico modo di riparare un albero mutato.
    """
    corrente = impronta(RADICE)
    if corrente != attesa_dichiarata:
        print("L'ALBERO NON E' QUELLO DICHIARATO: la baseline non si fissa.")
        print(f'  dichiarata {attesa_dichiarata}')
        print(f'  corrente   {corrente}')
        return 1

    elenco = file_dei_sorgenti(RADICE)
    if os.path.exists(BASELINE):
        vecchia, suo_elenco = leggi_manifesto()
        if vecchia is None:
            print('BASELINE GIA\' PRESENTE E SENZA MANIFESTO: non si sovrascrive.')
            return 2
        if not baseline_coerente(vecchia, suo_elenco):
            return 2
        if vecchia != corrente:
            print("BASELINE GIA' PRESENTE E DIVERSA DALL'ALBERO: non si sovrascrive.")
            print(f'  baseline {vecchia}')
            print(f'  albero   {corrente}')
            print('  rimuoverla a mano, dopo aver stabilito che nessun giro la sta usando.')
            return 2
        print(f"baseline gia' presente, verificata: impronta {corrente[:16]}")
        return 0

    os.makedirs(BASELINE)
    for relativo in elenco:
        destinazione = os.path.join(BASELINE, relativo)
        os.makedirs(os.path.dirname(destinazione), exist_ok=True)
        shutil.copyfile(os.path.join(RADICE, relativo), destinazione)
    with io.open(percorso_del_manifesto(), 'w', encoding='utf-8', newline='\n') as f:
        f.write(corrente + '\n')
        f.write('\n'.join(elenco) + '\n')
    # I permessi **non** rendono immutabile: il proprietario puo' rimetterli, e
    # `root` non li guarda. Servono a fermare gli incidenti; l'autorita' resta il
    # manifesto, ricalcolato a ogni avvio.
    for cartella, _, file in os.walk(BASELINE):
        for nome in file:
            p = os.path.join(cartella, nome)
            os.chmod(p, os.stat(p).st_mode & ~stat.S_IWUSR & ~stat.S_IWGRP & ~stat.S_IWOTH)
    print(f'baseline fissata: {len(elenco)} file, impronta {corrente[:16]}')
    return 0


# --- la riparazione, fail-closed ---------------------------------------------

def ripara(attesa):
    """Riporta l'albero alla baseline, **solo** se lo stato e' riconosciuto.

    Non scrive per scoprire: prima si calcolano le impronte di tutti e trentadue
    gli alberi mutati virtuali, poi si ripristina **solo** se la corrispondenza
    e' esattamente una. Zero vuol dire che c'e' dell'altro oltre alla mutazione;
    piu' di una, che due mutanti non si distinguono — e nessuno dei due casi si
    risolve scrivendo.
    """
    corrente = impronta(RADICE)
    if corrente == attesa:
        return True

    candidati = []
    for identificativo, nome, relativo, sano, malato in MUTANTI:
        origine = os.path.join(BASELINE, relativo)
        if not os.path.exists(origine):
            continue
        with io.open(origine, encoding='utf-8') as aperto:
            testo = aperto.read()
        if testo.count(sano) != 1:
            continue
        if impronta(BASELINE, (relativo, testo.replace(sano, malato))) == corrente:
            candidati.append((identificativo, nome, relativo, testo))

    if len(candidati) != 1:
        print("ALBERO NON RICONOSCIUTO: non e' la baseline, e non c'e' una sola")
        print('mutazione nota che lo spieghi. Non si tocca niente.')
        print(f'  attesa      {attesa}')
        print(f'  corrente    {corrente}')
        print(f'  spiegazioni {len(candidati)}')
        for identificativo, nome, _, _ in candidati:
            print(f'    - {identificativo} {nome}')
        return False

    identificativo, nome, relativo, testo = candidati[0]
    with io.open(os.path.join(RADICE, relativo), 'w', encoding='utf-8', newline='') as aperto:
        aperto.write(testo)
    if impronta(RADICE) != attesa:
        print(f'RIPARAZIONE INCOMPLETA: tolto {identificativo}, e l\'albero non torna alla baseline.')
        return False
    print(f'riparato: restava {identificativo} ({nome})')
    return True


# --- l'esecuzione ------------------------------------------------------------

def gruppo_vivo(gruppo):
    """Se nel gruppo c'e' ancora qualcuno.

    `PermissionError` significa che il gruppo **esiste** e non e' nostro, non
    che sia sparito: trattarlo come assenza direbbe «raccolto» su processi vivi,
    che e' il falso positivo peggiore fra i due.
    """
    try:
        os.killpg(gruppo, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def ferma_il_gruppo(figlio):
    """Ferma il gruppo, **raccoglie il figlio diretto**, e pretende che il
    gruppo si svuoti.

    La formulazione e' esatta apposta. Gli unici processi che si possono
    *raccogliere* sono i propri figli, e qui il figlio e' uno: `cargo`. Di
    `rustc` e dei binari di test si puo' soltanto chiedere la fine e
    verificarla — sono nipoti, e chi li aspetta e' `cargo`, che intanto sta
    morendo. Dire «raccoglie il gruppo» prometterebbe una cosa che nessun
    processo puo' fare per i figli altrui.

    Il gruppo si nomina con `figlio.pid`: `start_new_session=True` rende il
    figlio leader della propria sessione e del proprio gruppo, quindi il PGID
    **e'** quel pid. Chiederlo con `getpgid` aggiungerebbe un modo di
    sbagliare: se il leader se ne fosse gia' andato, la chiamata direbbe «non
    esiste» e la si leggerebbe come «gruppo vuoto» — mentre i discendenti
    possono benissimo essere ancora li'.

    Il figlio si raccoglie **dentro** l'attesa: uno zombie tiene vivo il gruppo,
    e un ciclo che non chiamasse `poll` scadrebbe per la propria omissione
    invece che per dei superstiti veri.

    Rende `True` se il gruppo si e' svuotato entro l'attesa.
    """
    gruppo = figlio.pid
    for segnale in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(gruppo, segnale)
        except ProcessLookupError:
            # Nessuno nel gruppo: il figlio puo' essere ancora uno zombie da
            # raccogliere, e raccoglierlo e' cio' che chiude il conto.
            figlio.poll()
            return True
        except PermissionError:
            print(f'  il gruppo {gruppo} non e\' nostro: non lo si puo\' fermare')
            return False
        scadenza = time.monotonic() + ATTESA_DEL_GRUPPO / 2
        while time.monotonic() < scadenza:
            figlio.poll()
            if not gruppo_vivo(gruppo):
                return True
            time.sleep(0.2)
    return False


def esegui(argomenti):
    """Esegue la batteria, e rende `(codice, raccolto)`.

    `codice` e' `None` quando il tetto di tempo e' scaduto.

    Il gruppo si guarda **dopo ogni uscita**, non solo dopo un timeout. Che
    `cargo` finisca ordinatamente non dice niente sui nipoti: un caso mutato puo'
    terminare lasciando vivo un processo che avrebbe dovuto uccidere, e quel
    processo continuerebbe a girare mentre il mutante successivo viene
    giudicato. E' la stessa classe di difetto che questi mutanti cercano nel
    supervisore.

    L'uscita va su un **file**: vedi la nota in testa.
    """
    with open(USCITA, 'wb') as dove:
        figlio = subprocess.Popen(
            COMANDO_BASE + [FILTRO] + argomenti,
            cwd=RADICE, stdout=dove, stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL, start_new_session=True,
        )
        try:
            codice = figlio.wait(timeout=TETTO_SECONDI)
        except subprocess.TimeoutExpired:
            return None, ferma_il_gruppo(figlio)
    if gruppo_vivo(figlio.pid):
        return codice, ferma_il_gruppo(figlio)
    return codice, True


def coda_dell_uscita(quante=3000):
    if not os.path.exists(USCITA):
        return '(nessuna uscita)'
    with io.open(USCITA, encoding='utf-8', errors='replace') as letto:
        return letto.read()[-quante:]


# --- il giro -----------------------------------------------------------------

def rimetti(percorso, originale, attesa):
    """Rimette il file originale, e verifica l'albero **intero**."""
    with io.open(percorso, 'w', encoding='utf-8', newline='') as aperto:
        aperto.write(originale)
    return impronta(RADICE) == attesa


def giro(primo, ultimo):
    if not elenco_e_canonico():
        return 1
    print(f'mutanti canonici: {len(IDENTIFICATORI)} — {IDENTIFICATORI[0]}..{IDENTIFICATORI[-1]}')

    attesa, elenco = leggi_manifesto()
    if attesa is None:
        print('BASELINE ASSENTE: eseguire prima `--prepara <impronta-attesa>`.')
        return 1
    if not baseline_coerente(attesa, elenco):
        return 2
    print(f'baseline: {len(elenco)} file, impronta {attesa[:16]}')

    if not tutti_applicabili_una_volta(BASELINE):
        return 1
    if not tutti_i_mutanti_sono_distinti(BASELINE, attesa):
        return 1
    if not ripara(attesa):
        return 2

    codice, raccolto = esegui([])
    if not raccolto:
        print('PROCESSI SUPERSTITI gia\' sulla batteria di riferimento: fermo.')
        return 2
    if codice != 0:
        print("L'ALBERO SANO E' GIA' ROSSO: non si misura niente. Fermo.")
        print(coda_dell_uscita())
        return 1

    uccisi, dal_compilatore, scaduti, sopravvissuti = [], [], [], []

    for indice, (identificativo, nome, relativo, sano, malato) in enumerate(MUTANTI, 1):
        if indice < primo or indice > ultimo:
            continue
        percorso = os.path.join(RADICE, relativo)
        with io.open(percorso, encoding='utf-8') as aperto:
            originale = aperto.read()
        with io.open(percorso, 'w', encoding='utf-8', newline='') as aperto:
            aperto.write(originale.replace(sano, malato))

        # La prima domanda: **compila?** Il giudizio e' il codice di uscita
        # di `--no-run`, non una stringa cercata nell'output.
        codice, raccolto = esegui(['--no-run'])
        if not raccolto:
            print(f'[{identificativo}] {nome}: PROCESSI SUPERSTITI dopo il tetto di tempo.')
            print('  i sorgenti restano mutati di proposito: rimetterli con dei processi')
            print('  vivi romperebbe proprio la regola per cui il gruppo si raccoglie.')
            print('  il giro successivo riconosce questo stato e lo ripara.')
            return 2
        if codice is None:
            scaduti.append((identificativo, nome))
            stato = 'SCADUTO (in compilazione)'
        elif codice != 0:
            dal_compilatore.append((identificativo, nome))
            stato = 'rifiutato dal compilatore'
        else:
            # La seconda: **i casi se ne accorgono?**
            codice, raccolto = esegui([])
            if not raccolto:
                print(f'[{identificativo}] {nome}: PROCESSI SUPERSTITI dopo il tetto di tempo.')
                print('  i sorgenti restano mutati di proposito, come sopra.')
                return 2
            if codice is None:
                scaduti.append((identificativo, nome))
                stato = 'SCADUTO (nei casi)'
            elif codice == 0:
                sopravvissuti.append((identificativo, nome))
                stato = 'SOPRAVVISSUTO'
            else:
                uccisi.append((identificativo, nome))
                stato = 'ucciso'

        if not rimetti(percorso, originale, attesa):
            print(f'[{identificativo}] {nome}: RIPRISTINO FALLITO — l\'albero non torna alla baseline. Fermo.')
            return 2
        print(f'[{identificativo}] {nome}: {stato}')

    attesi = ultimo - primo + 1
    ottenuti = len(uccisi) + len(dal_compilatore) + len(scaduti) + len(sopravvissuti)
    print()
    print("ripristino verificato dopo ogni mutante: l'albero e' la baseline.")
    print()
    print(f'mutanti del blocco {primo}-{ultimo}: {ottenuti} risultati su {attesi} attesi')
    print(f'  uccisi dai casi:            {len(uccisi)}')
    print(f'  rifiutati dal compilatore:  {len(dal_compilatore)}')
    print(f'  scaduti:                    {len(scaduti)}')
    print(f'  SOPRAVVISSUTI:              {len(sopravvissuti)}')
    for identificativo, nome in sopravvissuti:
        print(f'    - {identificativo} {nome}')
    for identificativo, nome in scaduti:
        print(f'    ! {identificativo} {nome} (scaduto)')
    if ottenuti != attesi:
        print()
        print(f'CONTEGGIO INATTESO: il blocco copre {attesi} mutanti e ne ha resi {ottenuti}.')
        return 1
    return 3 if (sopravvissuti or scaduti) else 0


# --- gli argomenti -----------------------------------------------------------

def intervallo(argomenti):
    """Le sole forme ammesse, e nient'altro.

    Un intervallo vuoto — `0 0`, `33 33`, gli estremi invertiti — eseguirebbe
    zero mutanti e finirebbe con successo: un verde che non ha misurato niente
    e' peggio di un rosso.
    """
    if len(argomenti) > 2:
        print(f'argomenti di troppo: {" ".join(argomenti[2:])}')
        return None
    numeri = []
    for testo in argomenti:
        try:
            valore = int(testo)
        except ValueError:
            print(f'«{testo}» non e\' un numero intero.')
            return None
        # La forma canonica e nient'altro: `01`, `+1` e ` 1` valgono uno per
        # `int`, e sono tre modi di scrivere un argomento che nessuno ha
        # inteso. Accettarli farebbe passare per intenzionale un refuso.
        if testo != str(valore):
            print(f'«{testo}» non e\' scritto in forma canonica: {valore}.')
            return None
        numeri.append(valore)
    primo = numeri[0] if numeri else 1
    ultimo = numeri[1] if len(numeri) > 1 else len(MUTANTI)
    if not 1 <= primo <= ultimo <= len(MUTANTI):
        print(f'intervallo non valido: {primo}-{ultimo}.')
        print(f'ammesso 1 <= primo <= ultimo <= {len(MUTANTI)}.')
        return None
    return primo, ultimo


def principale(argomenti):
    if '--impronta' in argomenti:
        if len(argomenti) != 1:
            print('`--impronta` non vuole altri argomenti.')
            return 1
        print(impronta(RADICE))
        return 0
    if '--prepara' in argomenti:
        if len(argomenti) != 2 or argomenti[0] != '--prepara':
            print("`--prepara` vuole esattamente l'impronta attesa, presa dal")
            print('manifesto di trasferimento e non da qui: una baseline che si')
            print('autocertifica non certifica niente.')
            return 1
        return prepara_baseline(argomenti[1])
    estremi = intervallo(argomenti)
    if estremi is None:
        return 1
    return giro(*estremi)


if __name__ == '__main__':
    # Una radice senza sorgenti si dichiara, non si propaga come traccia: chi
    # esegue lo script ha sbagliato percorso o ha una copia mai riuscita, e
    # merita una riga che lo dica invece di uno stack.
    try:
        sys.exit(principale(sys.argv[1:]))
    except AlberoSenzaSorgenti as vuoto:
        print(f'AlberoSenzaSorgenti: {vuoto}')
        sys.exit(1)
