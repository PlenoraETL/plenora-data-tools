# -*- coding: utf-8 -*-
"""Pulisce gli artefatti di coverage prima di ogni misura.

# Perche' serve

I residui che falsano una campagna di coverage sono di **due** classi, non
una.

1. I profili grezzi `*.profraw`. Il pid nel nome viene riciclato, LLVM trova
   il file gia' occupato e scrive su stderr; i golden di canale falliscono.
   E' la classe chiusa il 2026-08-21 da `scripts/pulisci_profili_coverage.py`,
   che resta il gestore di quella classe e non e' cambiato.

2. I **binari strumentati e le mappe di copertura** delle build precedenti.
   E' la classe che questo script aggiunge. `cargo llvm-cov` unisce i dati di
   profilo con le mappe di copertura trovate negli artefatti del target
   directory: un binario compilato da un albero sorgente piu' vecchio porta
   con se' la mappa di quel codice, e le sue righe entrano nel
   **denominatore** senza che nessun test le esegua. Il sintomo non e' un
   errore: e' una percentuale piu' bassa del vero, con righe mai eseguite che
   nessuno riesce a ricondurre al codice attuale. Il gate diventa **non
   ermetico** e produce rossi falsi. Non e' fail-open — non lascia passare
   codice scoperto — ma un banco di misura che sbaglia in difesa e' rotto
   quanto uno che sbaglia in accusa: insegna a non fidarsi del numero.

# Che cosa fa, e che cosa non fa

La seconda classe la rimuove `cargo llvm-cov clean --workspace`, che e' il
comando del tool per questo scopo. Nessuna cancellazione decisa da noi sul
filesystem: che cosa sia "artefatto del workspace" lo sa cargo, non questo
script.

Misura del 2026-08-22 su `target-cov` (12 521 file accumulati), inventario
confrontato prima e dopo il comando:

- rimossi 11 254 file: `debug/incremental/` (10 992), `debug/.fingerprint/`
  dei **soli cinque membri del workspace** (168), `debug/deps/` con i binari
  di test strumentati, i loro `.d`, `libplenora_core.rlib` e `.rmeta` (92),
  il binario `debug/plenora-data-tools` e `work.profdata`;
- sopravvissuti 1 267 file: `.fingerprint`, `deps` e `build` delle
  **dipendenze di terze parti**, che non vengono ricompilate.

Nessun fingerprint `plenora-*` e' sopravvissuto e nessun artefatto di terze
parti e' stato toccato: il perimetro e' esattamente il workspace, e la cache
delle dipendenze continua a pagare.

Il peso, misurato a valle di una singola campagna: `target-cov` passa da
5 748 file / 7,49 GiB a 1 267 file / 0,71 GiB, cioe' 4 481 file e 6,78 GiB —
il 90% dei byte — che erano artefatti del workspace.

Le due cifre di dimensione che si incontrano qui NON sono lo stesso numero, e
la differenza non e' un errore di nessuno dei due: `cargo` somma le dimensioni
per PERCORSO, `du` conta ogni INODE una volta. Gli artefatti "uplifted"
(`.rlib`, `.rmeta`, binari) esistono a due percorsi con lo stesso inode e per
cargo contano due volte. Verificato su un crate minimo: `du -sb` 42 665 byte,
`du -slb` (ogni link contato) 51 041, e cargo dichiara "49.8KiB" = 51 041
byte esatti. Le cifre qui sopra sono su base `du`, cioe' spazio realmente
occupato.

Nella stessa misura, tre `.profraw` piantati a mano hanno mostrato che
`clean --workspace` rimuove **solo** quello nella radice di
`target-cov/llvm-cov-target/`: quello annidato in `debug/deps/` e quello in
`target-cov/` sono sopravvissuti. Per questo la spazzata ricorsiva della
classe 1 resta necessaria e gira **dopo** — mai al posto di.

# Uso

    python scripts/pulisci_coverage.py                # prima della misura
    python scripts/pulisci_coverage.py --solo-profili # dopo, prima della cache

La modalita' completa richiede `cargo llvm-cov` sul PATH e
`CARGO_TARGET_DIR=target-cov`: e' il modo di rendere impossibile pulire una
directory e misurare in un'altra. La modalita' `--solo-profili` non invoca
cargo e quindi non pretende nulla dall'ambiente.

Chi chiama che cosa: prima della misura la pulizia e' completa in entrambi i
percorsi, sempre. Dopo la misura la CI ripulisce di nuovo tutto (la cache non
deve portarsi dietro artefatti che il job successivo cancella comunque),
mentre in locale `scripts/coverage.sh` toglie i soli profili: li' gli
artefatti servono a rieseguire `report --html` dopo una campagna rossa, e
cache da proteggere non ce n'e'.

Le regressioni girano a ogni invocazione, come per lo script della classe 1.
"""
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import pulisci_profili_coverage as profili  # noqa: E402

NOME_ATTESO = profili.NOME_ATTESO
COMANDO = ('cargo', 'llvm-cov', 'clean', '--workspace')
VARIABILE = 'CARGO_TARGET_DIR'

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class AmbienteNonAmmesso(Exception):
    """`CARGO_TARGET_DIR` non punta alla directory di coverage attesa.

    Pulire `target-cov` mentre la misura scrive in `target/` non fallisce:
    non fa niente, e la campagna prosegue sopra gli artefatti stantii che
    questo script esiste per rimuovere. Una pulizia che non pulisce e non lo
    dice e' la failure silenziosa peggiore.
    """


class PuliziaFallita(Exception):
    """`cargo llvm-cov clean --workspace` non ha ripulito il workspace."""


class ChiamanteNonProtetto(Exception):
    """Un percorso di misura non invoca questa pulizia prima di misurare.

    La pulizia protegge solo le campagne che la chiamano. I percorsi sono due
    — `scripts/coverage.sh` e il job `coverage` della CI — e se uno dei due
    misura senza pulire il difetto e' tornato meta' aperto senza che nulla
    diventi rosso: il sintomo e' un numero piu' basso, non un errore.
    """


def _esegui(comando, cwd, ambiente):
    """Esegue il comando e restituisce il codice di uscita."""
    return subprocess.call(list(comando), cwd=cwd, env=ambiente)


def verifica_ambiente(radice, ambiente):
    """Pretende `CARGO_TARGET_DIR` == la directory di coverage attesa.

    Accetta le forme equivalenti (`target-cov`, `./target-cov`, il percorso
    assoluto): il confronto e' sul percorso risolto, non sulla stringa.
    """
    valore = ambiente.get(VARIABILE)
    if valore is None:
        raise AmbienteNonAmmesso(
            "%s non e' impostata: la misura userebbe un'altra directory e "
            "questa pulizia non toccherebbe gli artefatti che finiscono nel "
            "report. Esporta %s=%s." % (VARIABILE, VARIABILE, NOME_ATTESO))
    atteso = os.path.realpath(os.path.join(radice, NOME_ATTESO))
    indicato = os.path.realpath(os.path.join(radice, valore))
    if indicato != atteso:
        raise AmbienteNonAmmesso(
            "%s=%r punta a %r, la pulizia agisce su %r: pulire una directory "
            "e misurare in un'altra lascia intatti gli artefatti stantii."
            % (VARIABILE, valore, indicato, atteso))


def pulisci_artefatti(radice=RADICE, ambiente=None, esegui=_esegui):
    """Rimuove gli artefatti strumentati del workspace.

    Qualunque esito diverso da zero — eseguibile assente compreso — e'
    fatale: proseguire significherebbe misurare esattamente sopra i residui
    da rimuovere.
    """
    if ambiente is None:
        ambiente = os.environ
    verifica_ambiente(radice, ambiente)
    try:
        codice = esegui(COMANDO, radice, dict(ambiente))
    except OSError as errore:
        raise PuliziaFallita(
            "%s non eseguibile (%s): senza questa pulizia la misura "
            "includerebbe le mappe di copertura delle build precedenti."
            % (' '.join(COMANDO), errore))
    if codice != 0:
        raise PuliziaFallita(
            "%s e' uscito con codice %d" % (' '.join(COMANDO), codice))


def pulisci_tutto(radice=RADICE, ambiente=None, esegui=_esegui):
    """Entrambe le classi, nell'ordine che conta. Torna i profili rimossi.

    Prima gli artefatti, poi i profili: `clean --workspace` cancella anche il
    `work.profdata` e i `.profraw` della sola radice di `llvm-cov-target`, e
    la spazzata ricorsiva raccoglie tutti gli altri. Invertire l'ordine
    lascerebbe sul disco i profili che il primo comando non vede.
    """
    pulisci_artefatti(radice, ambiente, esegui)
    return profili.pulisci(os.path.join(radice, NOME_ATTESO), radice)


def _blocco_job_coverage(testo):
    """Le righe del job `coverage` del workflow, fino al job successivo."""
    dentro = False
    blocco = []
    for riga in testo.splitlines():
        if riga.startswith('  coverage:'):
            dentro = True
            continue
        if dentro:
            # Un nuovo job: due spazi di indentazione e due punti finali.
            nuovo_job = (riga.startswith('  ') and not riga.startswith('   ')
                         and riga.rstrip().endswith(':'))
            if nuovo_job:
                break
            blocco.append(riga)
    return blocco


def _prima_occorrenza(righe, ago):
    for indice, riga in enumerate(righe):
        if ago in riga:
            return indice
    return None


def verifica_chiamanti(radice=RADICE):
    """Pretende che i due percorsi di misura puliscano prima di misurare."""
    nome = 'pulisci_coverage.py'
    percorsi = [
        (os.path.join(radice, 'scripts', 'coverage.sh'), None),
        (os.path.join(radice, '.github', 'workflows', 'ci.yml'),
         _blocco_job_coverage),
    ]
    for percorso, estrai in percorsi:
        if not os.path.isfile(percorso):
            raise ChiamanteNonProtetto(
                "%s non trovato: non si puo' verificare che pulisca prima di "
                "misurare." % percorso)
        sorgente = open(percorso, 'rb')
        try:
            testo = sorgente.read().decode('utf-8')
        finally:
            sorgente.close()
        righe = estrai(testo) if estrai else testo.splitlines()

        misura = _prima_occorrenza(righe, 'cargo llvm-cov --workspace')
        if misura is None:
            raise ChiamanteNonProtetto(
                "%s non contiene la misura attesa (`cargo llvm-cov "
                "--workspace`): il controllo non sa piu' che cosa sta "
                "proteggendo." % percorso)
        pulizia = _prima_occorrenza(righe, nome)
        if pulizia is None or pulizia > misura:
            raise ChiamanteNonProtetto(
                "%s misura senza invocare prima %s: la campagna girerebbe "
                "sopra i binari strumentati delle build precedenti, e il "
                "denominatore includerebbe righe che nessun test esegue."
                % (percorso, nome))


def _scrivi(percorso, contenuto='x'):
    destinazione = open(percorso, 'wb')
    try:
        destinazione.write(contenuto.encode('utf-8'))
    finally:
        destinazione.close()


def _ambiente_valido():
    return {VARIABILE: NOME_ATTESO}


def autotest():
    """Regressioni. Girano sempre: questo script cancella artefatti."""
    import shutil
    import tempfile

    base = tempfile.mkdtemp(prefix='pulisci-coverage-')
    try:
        _autotest_ambiente(base)
        _autotest_esito(base)
        _autotest_classe_completa(base)
        _autotest_chiamanti(base)
    finally:
        shutil.rmtree(base, ignore_errors=True)
    verifica_chiamanti()


def _autotest_ambiente(base):
    """`CARGO_TARGET_DIR` assente o diverso: rifiuto, non pulizia inutile."""
    chiamate = []

    def spia(comando, cwd, ambiente):
        chiamate.append((comando, cwd, ambiente))
        return 0

    for ambiente in ({}, {VARIABILE: 'target'}, {VARIABILE: '/altrove'}):
        try:
            pulisci_artefatti(base, ambiente, spia)
        except AmbienteNonAmmesso:
            pass
        else:
            raise SystemExit(
                "autotest: accettato %s=%r. Pulire una directory e misurare "
                "in un'altra e' una pulizia che non pulisce."
                % (VARIABILE, ambiente.get(VARIABILE)))
    if chiamate:
        raise SystemExit(
            "autotest: cargo invocato nonostante l'ambiente rifiutato")

    # Le forme equivalenti della stessa directory devono passare.
    for valore in (NOME_ATTESO, os.path.join('.', NOME_ATTESO),
                   os.path.join(base, NOME_ATTESO)):
        pulisci_artefatti(base, {VARIABILE: valore}, spia)
    if len(chiamate) != 3:
        raise SystemExit('autotest: forme equivalenti di %s rifiutate'
                         % VARIABILE)

    comando, cwd, ambiente = chiamate[0]
    if tuple(comando) != COMANDO:
        raise SystemExit('autotest: comando %r invece di %r'
                         % (comando, COMANDO))
    if cwd != base:
        raise SystemExit('autotest: cargo invocato in %r invece che in %r'
                         % (cwd, base))
    if ambiente.get(VARIABILE) != NOME_ATTESO:
        raise SystemExit('autotest: %s non propagata al processo figlio'
                         % VARIABILE)


def _autotest_esito(base):
    """Un clean fallito e' fatale: la misura non deve partire lo stesso."""
    def fallisce(comando, cwd, ambiente):
        return 101

    try:
        pulisci_artefatti(base, _ambiente_valido(), fallisce)
    except PuliziaFallita:
        pass
    else:
        raise SystemExit(
            "autotest: un clean fallito e' passato per riuscito. La misura "
            "sarebbe partita sopra gli artefatti stantii.")

    def assente(comando, cwd, ambiente):
        raise OSError(2, 'No such file or directory')

    try:
        pulisci_artefatti(base, _ambiente_valido(), assente)
    except PuliziaFallita:
        pass
    except OSError:
        raise SystemExit(
            'autotest: cargo mancante riportato come OSError grezzo invece '
            'che come pulizia fallita')
    else:
        raise SystemExit('autotest: cargo mancante non ha fermato la pulizia')


def _autotest_classe_completa(base):
    """La classe intera: artefatti strumentati **e** profili, in ordine.

    Lo stub riproduce cio' che il comando reale fa e cio' che non fa: rimuove
    gli artefatti del workspace e il `.profraw` della sola radice di
    `llvm-cov-target`, lascia le dipendenze di terze parti, e scrive un
    profilo nuovo — come farebbe un processo strumentato che muore durante la
    pulizia. Se la spazzata dei profili girasse prima del clean, o non
    girasse affatto, quel profilo resterebbe sul disco e il test fallirebbe.
    """
    radice = os.path.join(base, 'repo-completo')
    coverage = os.path.join(radice, NOME_ATTESO)
    interna = os.path.join(coverage, 'llvm-cov-target')
    deps = os.path.join(interna, 'debug', 'deps')
    os.makedirs(deps)

    strumentati = [
        os.path.join(deps, 'plenora_core-0123456789abcdef'),
        os.path.join(deps, 'libplenora_core.rlib'),
        os.path.join(interna, 'work.profdata'),
    ]
    terze_parti = [
        os.path.join(deps, 'libserde-fedcba9876543210.rlib'),
        os.path.join(deps, 'libarrow-1122334455667788.rmeta'),
    ]
    profraw = [
        os.path.join(coverage, 'work-1-aaa_0.profraw'),
        os.path.join(interna, 'work-2-bbb_0.profraw'),
        os.path.join(deps, 'work-3-ccc_0.profraw'),
    ]
    for percorso in strumentati + terze_parti + profraw:
        _scrivi(percorso)

    tardivo = os.path.join(deps, 'work-4-ddd_0.profraw')

    def clean_finto(comando, cwd, ambiente):
        for percorso in strumentati:
            if os.path.exists(percorso):
                os.remove(percorso)
        # Solo la radice di `llvm-cov-target`, come il comando reale.
        os.remove(os.path.join(interna, 'work-2-bbb_0.profraw'))
        _scrivi(tardivo)
        return 0

    rimossi = pulisci_tutto(radice, _ambiente_valido(), clean_finto)

    sopravvissuti = [p for p in strumentati if os.path.exists(p)]
    if sopravvissuti:
        raise SystemExit(
            'autotest: artefatti strumentati sopravvissuti: %r'
            % sopravvissuti)
    rimasti = [p for p in profraw + [tardivo] if os.path.exists(p)]
    if rimasti:
        raise SystemExit(
            "autotest: profili sopravvissuti alla pulizia completa: %r.\n"
            "  Il profilo scritto durante il clean e' la prova che la "
            "spazzata deve girare DOPO, non prima." % rimasti)
    if rimossi != 3:
        raise SystemExit('autotest: riportati %d profili rimossi invece di 3'
                         % rimossi)
    persi = [p for p in terze_parti if not os.path.exists(p)]
    if persi:
        raise SystemExit(
            "autotest: rimossi artefatti di terze parti: %r. Il perimetro e' "
            "il workspace; il resto e' cache che va conservata." % persi)


def _autotest_chiamanti(base):
    """Un chiamante che misura senza pulire deve essere respinto."""
    radice = os.path.join(base, 'repo-chiamanti')
    os.makedirs(os.path.join(radice, 'scripts'))
    os.makedirs(os.path.join(radice, '.github', 'workflows'))
    copertura = os.path.join(radice, 'scripts', 'coverage.sh')
    workflow = os.path.join(radice, '.github', 'workflows', 'ci.yml')

    def scrivi_workflow(righe):
        _scrivi(workflow, '\n'.join(
            ['  test:', '    steps: []', '  coverage:'] + righe
            + ['  altro-job:', '    steps: []']) + '\n')

    misura = '      - run: cargo llvm-cov --workspace --no-report'
    pulizia = '      - run: python scripts/pulisci_coverage.py'

    # Misura senza pulizia: respinta in entrambi i file, uno alla volta.
    _scrivi(copertura, 'cargo llvm-cov --workspace --locked --no-report\n')
    scrivi_workflow(['    steps:', misura])
    for chiamante in ('coverage.sh', 'ci.yml'):
        try:
            verifica_chiamanti(radice)
        except ChiamanteNonProtetto:
            pass
        else:
            raise SystemExit(
                'autotest: accettato un chiamante che misura senza pulire '
                '(atteso il rifiuto su %s)' % chiamante)
        # Sistemato il primo, il controllo deve fallire ancora sul secondo.
        _scrivi(copertura, 'python3 scripts/pulisci_coverage.py\n'
                           'cargo llvm-cov --workspace --locked --no-report\n')

    # Pulizia dopo la misura: non protegge nulla, va respinta come l'assenza.
    scrivi_workflow(['    steps:', misura, pulizia])
    try:
        verifica_chiamanti(radice)
    except ChiamanteNonProtetto:
        pass
    else:
        raise SystemExit(
            'autotest: accettata una pulizia successiva alla misura')

    # Ordine giusto: accettato.
    scrivi_workflow(['    steps:', pulizia, misura])
    verifica_chiamanti(radice)

    # La pulizia di un ALTRO job non conta: il blocco letto e' quello del job
    # `coverage`, e fuori di li' la misura resta scoperta.
    _scrivi(workflow, '\n'.join(
        ['  test:', '    steps:', pulizia, '  coverage:', '    steps:',
         misura]) + '\n')
    try:
        verifica_chiamanti(radice)
    except ChiamanteNonProtetto:
        pass
    else:
        raise SystemExit(
            "autotest: la pulizia di un altro job e' stata contata per quella "
            "del job coverage")


def main(argomenti):
    solo_profili = False
    if argomenti == ['--solo-profili']:
        solo_profili = True
    elif argomenti:
        print('uso: pulisci_coverage.py [--solo-profili]')
        return 2

    try:
        autotest()
    except ChiamanteNonProtetto as errore:
        print('ERRORE: %s' % errore)
        return 1

    percorso = os.path.join(RADICE, NOME_ATTESO)
    try:
        if solo_profili:
            rimossi = profili.pulisci(percorso, RADICE)
        else:
            rimossi = pulisci_tutto(RADICE)
    except (AmbienteNonAmmesso, PuliziaFallita,
            profili.PercorsoNonAmmesso, profili.ScansioneFallita) as errore:
        print('ERRORE: %s' % errore)
        return 1

    if not solo_profili:
        print('artefatti strumentati del workspace rimossi da: %s'
              % ' '.join(COMANDO))
    print('profili di coverage rimossi: %d (%s)' % (rimossi, percorso))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
