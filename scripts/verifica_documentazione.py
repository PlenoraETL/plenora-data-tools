# -*- coding: utf-8 -*-
"""Gate: la superficie documentale resta quella decisa, e non si sfilaccia.

Il 2026-08-21 il repository e' stato ripulito: sessantasei documenti Markdown
sono diventati nove, e la storia e' rimasta in Git invece che nel worktree.
Una pulizia del genere si disfa nel modo piu' banale — qualcuno aggiunge un
`NOTE.md`, qualcun altro un `docs/vecchio/`, e in sei mesi ci sono due fonti
di verita' che nessuno tiene allineate.

Questo script lo impedisce. Verifica sette cose:

1. **nessun Markdown pubblico fuori dalla allowlist**. L'elenco e' esatto, non
   un glob: aggiungere un documento e' una decisione, non un effetto
   collaterale;
2. **nessun collegamento locale rotto**, comprese le ancore `#sezione`: un
   link che non porta da nessuna parte e' peggio di nessun link, perche'
   promette;
3. **nessun riferimento a un documento eliminato**, in tutto il repository:
   ADR, deroghe, review, checkpoint, verbali e i loro nomi di file;
4. **ogni riferimento `D*` ha una definizione corrente** nel registro delle
   decisioni di `docs/architettura.md`, e ogni decisione definita e' citata
   da qualche parte. Gli identificatori sono riferimenti stabili: un commento
   che scrive `D16` deve poterlo risolvere, altrimenti e' un puntatore morto
   come un link rotto — e un registro che nessuno cita torna a essere un
   archivio;
5. **catalogo e `operazioni.md` allineati**: stesso numero di operazioni, e le
   stesse. Non un conteggio, un confronto per nome;
6. **il documento generato non diverge**, delegando ad `assemble.py --verify`;
7. **nessun PDF, bytecode o artefatto di build tracciato**.

I manifesti di rilascio sotto `release/` sono **esclusi** dai punti 3 e 4:
sono documenti immutabili, e il testo dell'epoca serve al gate di rilascio.

    python scripts/verifica_documentazione.py

Esce 1 al primo scostamento.
"""
import io
import os
import re
import subprocess
import sys

# ---------------------------------------------------------------------------
# La superficie, esatta
# ---------------------------------------------------------------------------

PUBBLICI = [
    'README.md',
    'AGENTS.md',
    'docs/architettura.md',
    'docs/piano-v5.md',
    'docs/cli.md',
    'docs/operazioni.md',
    'docs/errori-e-limiti.md',
    'docs/stato-e-roadmap.md',
    'docs/release.md',
]

# Documentazione viva, eseguita dalla suite: eccezione esatta, non un glob.
ECCEZIONI = [
    'examples/e1-filtro-ordinamento/README.md',
]

# Sorgenti della generazione di `operazioni.md`: non sono documenti pubblici,
# sono input di un artefatto.
SORGENTI_GENERATE = re.compile(r'^docs/_fragments/[0-9]+\.md$')

# ---------------------------------------------------------------------------
# Ciò che non deve più comparire
# ---------------------------------------------------------------------------

MORTI = [
    (r'\bADR[-\s]0*\d{1,2}\b', 'riferimento a una ADR eliminata'),
    (r'\bDER-\d{3}\b', 'riferimento a una deroga eliminata'),
    (r'\bdocs/adr/', 'la directory delle ADR non esiste piu\''),
    (r'\bdocs/misure/', 'i grezzi di misura non esistono piu\''),
    (r'\bdocs/assurance/', 'la directory assurance non esiste piu\''),
    (r'\breview-\d+-fix-\d{4}-\d{2}-\d{2}', 'riferimento a un verbale di review'),
    (r'\bcheckpoint-[a-z]+-\d{4}-\d{2}-\d{2}', 'riferimento a un checkpoint'),
    (r'\bhotfix-\d{4}-\d{2}-\d{2}', 'riferimento a un hotfix'),
    (r'\bArchitetture\.md\b', 'sostituito da docs/architettura.md'),
    (r'\bPrestazioni\.md\b', 'eliminato'),
    (r'\bderoghe\.md\b', 'i limiti stanno in docs/errori-e-limiti.md'),
    (r'\bpiano-usabilita\.md\b', 'sostituito da docs/stato-e-roadmap.md'),
    (r'\bkernel-signatures\.md\b', 'sostituito da docs/operazioni.md'),
    (r'\bapi-breaking-\d{4}-\d{2}-\d{2}\.md\b', 'assorbito da docs/release.md'),
    (r'\bder011-censimento', 'assorbito da docs/errori-e-limiti.md'),
    (r'\bgenera_verbale_', 'gli script dei verbali sono stati eliminati'),
    (r'\bverifica_censimento_der011', 'ora scripts/verifica_memoria_governata.py'),
    (r'\bRIPRODUCIBILITA\.md\b', 'assorbito da docs/release.md'),
]

# I manifesti sono immutabili: il testo dell'epoca serve al gate di rilascio.
ESENTI_DAI_MORTI = re.compile(r'^(release/|\.git/)')

# Estensioni testuali in cui cercare i riferimenti morti.
TESTUALI = ('.rs', '.md', '.py', '.toml', '.yml', '.yaml', '.sh', '.json')

# ---------------------------------------------------------------------------
# Artefatti che non vanno tracciati
# ---------------------------------------------------------------------------

ARTEFATTI = [
    ('*.pdf', 'i PDF sono stati ritirati il 2026-08-21'),
    ('*.pyc', 'bytecode Python'),
    ('*.pyo', 'bytecode Python'),
    ('__pycache__/*', 'bytecode Python'),
    ('*.rlib', 'artefatto di build'),
    ('*.rmeta', 'artefatto di build'),
]

ARCHITETTURA = 'docs/architettura.md'

# `D16`, `D12.7`: identificatori di decisione. Il registro li definisce
# come righe di tabella `| **D16** | ... |`.
# La sequenza puntata e' di profondita' ARBITRARIA: esistono `D14.5.6`
# come esistono `D14.5` e `D16`. Fermarsi a un livello tronca
# l'identificatore e ne inventa uno che nessuno ha definito — il
# gate direbbe «tutto risolto» confrontando la cosa sbagliata.
RIFERIMENTO_D = re.compile(r'(?<![A-Za-z0-9_])D([0-9]+(?:\.[0-9]+)*)(?![A-Za-z0-9_])')
DEFINIZIONE_D = re.compile(r'^\|\s*\*\*(D[0-9]+(?:\.[0-9]+)*)\*\*\s*\|', re.M)

CATALOGO = 'crates/plenora-core/src/catalog.rs'
OPERAZIONI = 'docs/operazioni.md'
GENERATORE = 'docs/_build/assemble.py'

COLLEGAMENTO = re.compile(r'\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)')


def tracciati():
    fuori = subprocess.run(['git', 'ls-files'], capture_output=True,
                           text=True, encoding='utf-8', check=True)
    return [r for r in fuori.stdout.split('\n') if r.strip()]


def testo(percorso):
    return io.open(percorso, encoding='utf-8', newline='').read()


def ancora(titolo):
    """L'ancora GitHub di un titolo Markdown."""
    minuscolo = titolo.strip().lower()
    minuscolo = re.sub(r'[^\w\s-]', '', minuscolo, flags=re.UNICODE)
    return re.sub(r'[\s_]+', '-', minuscolo).strip('-')


def ancore_di(percorso):
    return {ancora(riga.lstrip('#').strip())
            for riga in testo(percorso).split('\n')
            if riga.startswith('#')}


def controlla_superficie(file_tracciati, problemi):
    ammessi = set(PUBBLICI) | set(ECCEZIONI)
    for percorso in file_tracciati:
        if not percorso.endswith('.md'):
            continue
        if percorso in ammessi or SORGENTI_GENERATE.match(percorso):
            continue
        problemi.append(
            'MARKDOWN FUORI SUPERFICIE: %s\n'
            '  La superficie pubblica e\' chiusa: %s.\n'
            '  Aggiungere un documento e\' una decisione: va messo in '
            'PUBBLICI qui dentro, con la ragione.'
            % (percorso, ', '.join(PUBBLICI)))
    tracciati_set = set(file_tracciati)
    for percorso in PUBBLICI + ECCEZIONI:
        if not os.path.exists(percorso):
            problemi.append('DOCUMENTO ATTESO ASSENTE: %s' % percorso)
        elif percorso not in tracciati_set:
            # Esistere sul disco non basta: un documento non tracciato non
            # arriva a chi clona il repository, e il gate direbbe di si'
            # guardando una copia locale che nessun altro ha.
            problemi.append(
                "DOCUMENTO NON TRACCIATO: %s esiste ma non e' in Git."
                % percorso)


def controlla_collegamenti(problemi):
    for percorso in PUBBLICI + ECCEZIONI:
        if not os.path.exists(percorso):
            continue
        cartella = os.path.dirname(percorso) or '.'
        for numero, riga in enumerate(testo(percorso).split('\n'), 1):
            for bersaglio in COLLEGAMENTO.findall(riga):
                if bersaglio.startswith(('http://', 'https://', 'mailto:')):
                    continue
                if bersaglio.startswith('#'):
                    file_bersaglio, frammento = percorso, bersaglio[1:]
                else:
                    parti = bersaglio.split('#', 1)
                    file_bersaglio = os.path.normpath(
                        os.path.join(cartella, parti[0]))
                    frammento = parti[1] if len(parti) > 1 else ''
                if not os.path.exists(file_bersaglio):
                    problemi.append(
                        'COLLEGAMENTO ROTTO %s:%d -> %s\n'
                        '  Il file non esiste. Un link che promette e non '
                        'porta da nessuna parte e\' peggio di nessun link.'
                        % (percorso, numero, bersaglio))
                elif frammento and file_bersaglio.endswith('.md'):
                    if frammento not in ancore_di(file_bersaglio):
                        problemi.append(
                            'ANCORA ROTTA %s:%d -> %s\n'
                            '  Il file c\'e\', la sezione no.'
                            % (percorso, numero, bersaglio))


def controlla_riferimenti_morti(file_tracciati, problemi):
    compilati = [(re.compile(pattern), motivo) for pattern, motivo in MORTI]
    for percorso in file_tracciati:
        if ESENTI_DAI_MORTI.match(percorso):
            continue
        if not percorso.endswith(TESTUALI):
            continue
        if percorso == 'scripts/verifica_documentazione.py':
            continue  # e' l'elenco stesso
        if not os.path.exists(percorso):
            continue
        contenuto = testo(percorso)
        for numero, riga in enumerate(contenuto.split('\n'), 1):
            for pattern, motivo in compilati:
                trovato = pattern.search(riga)
                if trovato:
                    problemi.append(
                        'RIFERIMENTO MORTO %s:%d: %r\n  %s'
                        % (percorso, numero, trovato.group(0), motivo))
                    break


def controlla_decisioni(file_tracciati, problemi):
    """Ogni `D*` citato dev'essere definito, e ogni definizione dev'essere citata.

    Gli identificatori di decisione sono un secondo sistema di riferimenti,
    accanto ai collegamenti Markdown, e si rompe allo stesso modo: la
    decisione sparisce e il commento che la cita resta, convincente e senza
    referente. Il registro di `architettura.md` contiene solo le decisioni
    ancora attive, quindi un `D*` citato e non definito e' una decisione
    superata che qualcuno continua a invocare.
    """
    if not os.path.exists(ARCHITETTURA):
        problemi.append("%s assente: non c\'e\' registro delle decisioni"
                        % ARCHITETTURA)
        return
    trovate = DEFINIZIONE_D.findall(testo(ARCHITETTURA))
    definite = set(trovate)
    doppie = sorted({i for i in trovate if trovate.count(i) > 1},
                    key=lambda i: [int(p) for p in i[1:].split('.')])
    if doppie:
        problemi.append(
            "DECISIONI DEFINITE PIU' DI UNA VOLTA: %r\n"
            "  Due righe per lo stesso identificatore sono due contratti per "
            "la stessa decisione, e chi legge non sa quale valga." % doppie)
    if not definite:
        problemi.append(
            "REGISTRO DELLE DECISIONI VUOTO in %s.\n"
            "  Se le decisioni non hanno piu\' identificatori, vanno tolti "
            "anche i riferimenti nel codice." % ARCHITETTURA)
        return

    citazioni = {}
    for percorso in file_tracciati:
        if ESENTI_DAI_MORTI.match(percorso):
            continue
        if not percorso.endswith(TESTUALI):
            continue
        if percorso in (ARCHITETTURA, 'scripts/verifica_documentazione.py'):
            continue
        if not os.path.exists(percorso):
            continue
        for numero, riga in enumerate(testo(percorso).split(chr(10)), 1):
            for trovato in RIFERIMENTO_D.finditer(riga):
                citazioni.setdefault(trovato.group(0), (percorso, numero))

    def ordine(ident):
        return [int(pezzo) for pezzo in ident[1:].split('.')]

    orfani = sorted(set(citazioni) - definite, key=ordine)
    if orfani:
        righe = [('    %-8s %s:%d' % (i, citazioni[i][0], citazioni[i][1]))
                 for i in orfani]
        problemi.append(
            "DECISIONI CITATE E NON DEFINITE (%d):\n%s\n"
            "  Il registro di %s contiene solo le decisioni ancora attive.\n"
            "  Una decisione superata non si archivia: si tolgono i suoi "
            "riferimenti." % (len(orfani), chr(10).join(righe), ARCHITETTURA))

    inutilizzate = sorted(definite - set(citazioni), key=ordine)
    if inutilizzate:
        problemi.append(
            "DECISIONI DEFINITE E MAI CITATE: %r\n"
            "  Un registro che cresce e non viene letto torna a essere un "
            "archivio. Se la decisione vale ancora ma nessuno la cita, il "
            "riferimento va messo dove la decisione e\' attuata; se non vale "
            "piu\', va tolta." % inutilizzate)


def controlla_catalogo(problemi):
    if not (os.path.exists(CATALOGO) and os.path.exists(OPERAZIONI)):
        problemi.append('catalogo o documento delle operazioni assenti')
        return
    # Il catalogo si legge col parser del generatore, non con una regex
    # propria: due letture dello stesso file possono divergere, e la
    # divergenza sarebbe fra due controlli invece che fra codice e documento.
    import importlib.util
    spec = importlib.util.spec_from_file_location('assemble', GENERATORE)
    assemble = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(assemble)
    try:
        catalogate = set(assemble.parse_catalog())
    except assemble.ErroreCatalogo as errore:
        problemi.append('CATALOGO NON LEGGIBILE: %s' % errore)
        return
    documentate = set(re.findall(r'^### ((?:table|geo)\.[a-z0-9_]+)\s*$',
                                 testo(OPERAZIONI), re.M))
    mancanti = sorted(catalogate - documentate)
    inventate = sorted(documentate - catalogate)
    if mancanti or inventate:
        problemi.append(
            'CATALOGO E DOCUMENTO NON ALLINEATI (%d catalogate, %d documentate)\n'
            '  senza firma: %r\n  documentate ma non catalogate: %r\n'
            '  La copertura e\' totale per contratto.'
            % (len(catalogate), len(documentate), mancanti, inventate))


def controlla_generato(problemi):
    esito = subprocess.run([sys.executable, GENERATORE, '--verify'],
                           capture_output=True, text=True, encoding='utf-8')
    if esito.returncode != 0:
        problemi.append(
            'DOCUMENTO GENERATO DIVERGENTE:\n  %s'
            % (esito.stdout or esito.stderr).strip().replace('\n', '\n  '))


def controlla_artefatti(problemi):
    for pattern, motivo in ARTEFATTI:
        esito = subprocess.run(['git', 'ls-files', pattern],
                               capture_output=True, text=True,
                               encoding='utf-8', check=True)
        trovati = [r for r in esito.stdout.split('\n') if r.strip()]
        if trovati:
            problemi.append(
                'ARTEFATTO TRACCIATO (%s): %r\n  %s'
                % (pattern, trovati, motivo))


def main():
    problemi = []
    file_tracciati = tracciati()
    controlla_superficie(file_tracciati, problemi)
    controlla_collegamenti(problemi)
    controlla_riferimenti_morti(file_tracciati, problemi)
    controlla_decisioni(file_tracciati, problemi)
    controlla_catalogo(problemi)
    controlla_generato(problemi)
    controlla_artefatti(problemi)

    if problemi:
        sys.stderr.write('la documentazione non e\' quella decisa:\n\n')
        for problema in problemi:
            sys.stderr.write('- %s\n' % problema)
        raise SystemExit(1)

    definite = len(set(DEFINIZIONE_D.findall(testo(ARCHITETTURA))))
    print('documentazione coerente: %d documenti pubblici, %d eccezioni, '
          '%d decisioni definite e tutte citate, nessun riferimento morto, '
          'catalogo e operazioni allineati, nessun artefatto tracciato'
          % (len(PUBBLICI), len(ECCEZIONI), definite))


main()
