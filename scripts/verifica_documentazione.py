# -*- coding: utf-8 -*-
"""Gate: la documentazione resta risolvibile e allineata al codice.

Verifica cinque cose, tutte oggettive:

1. **nessun collegamento locale rotto** nei Markdown tracciati, comprese le
   ancore `#sezione`: un link che non porta da nessuna parte e' peggio di
   nessun link, perche' promette;
2. **ogni puntatore interno ha una definizione corrente**, e ogni
   definizione e' citata. Vale per tutte le famiglie — `D`, `M`, `V`, `E`,
   `I`, `P`, `G` — con l'identificatore intero, anche multilivello. Sono
   cercati **solo nei commenti**: `const M1: usize` ha la forma di un
   puntatore ed e' codice. Ogni riferimento testuale a una sezione
   (`errori-e-limiti.md#...`) si risolve, e nessuna ancora e' duplicata;
3. **catalogo e `operazioni.md` allineati**: stesse operazioni, per nome;
4. **il documento generato non diverge**, delegando ad `assemble.py --verify`;
5. **nessun PDF, bytecode o artefatto di build tracciato**.

Non giudica la prosa e non tiene un elenco dei documenti ammessi: aggiungere
un documento e' una decisione che si prende in PR, e il gate ne controlla i
collegamenti come per tutti gli altri.

I manifesti di rilascio sotto `release/` sono esclusi: sono documenti
immutabili, e il testo dell'epoca serve al gate di rilascio.

    python scripts/verifica_documentazione.py

Esce 1 al primo scostamento.
"""
import io
import os
import re
import subprocess
import sys

# Sorgenti della generazione di `operazioni.md`: non sono documenti pubblici,
# sono input di un artefatto.
SORGENTI_GENERATE = re.compile(r'^docs/_fragments/[0-9]+\.md$')

# I manifesti sono immutabili: il testo dell'epoca serve al gate di rilascio.
ESENTI = re.compile(r'^(release/|\.git/)')

# Estensioni testuali in cui cercare i riferimenti a una sezione.
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
# I puntatori interni: `D16`, `D14.5.6`, `M3`, `E1`. Sette famiglie, un
# solo sistema di regole.
#
# La sequenza puntata e' di profondita' ARBITRARIA: esistono `D14.5.6` come
# esistono `D14.5` e `D16`. Fermarsi a un livello tronca l'identificatore e ne
# inventa uno che nessuno ha definito — il gate direbbe «tutto risolto»
# confrontando la cosa sbagliata.
#
# Al massimo DUE cifre per livello, e **solo nei commenti**. Le due regole
# insieme tengono fuori cio' che non e' un puntatore: `const M1: usize`,
# `I64(&Int64Array)` e `soundex("Pfister") == "P236"` sono codice, non prosa;
# `# noqa: E402` e' un codice di flake8, e ha tre cifre. Un gate che li
# segnalasse verrebbe disattivato entro una settimana.
FAMIGLIE = 'DMVEIPG'
PUNTATORE = re.compile(
    r'(?<![A-Za-z0-9_])([%s][0-9]{1,2}(?:\.[0-9]{1,2})*)(?![A-Za-z0-9_])'
    % FAMIGLIE)
# Una definizione e' una riga di tabella `| **D16** | ... |` oppure un
# titolo `### M3 — ...`: le due forme che i documenti attuali usano.
DEFINIZIONE = re.compile(
    r'^(?:\|\s*\*\*([%s][0-9.]+)\*\*\s*\||#{1,4}\s+([%s][0-9.]+)\s+[—-])'
    % (FAMIGLIE, FAMIGLIE), re.M)

# Dove ogni famiglia si definisce. Un puntatore vive dove la cosa che
# nomina e' descritta, non in un registro unico: `M3` sta nella roadmap.
# L'esempio eseguibile non ha un identificatore: un puntatore che nessuno
# cita e' un archivio di una voce sola.
DOVE_SI_DEFINISCE = [
    'docs/architettura.md',
    'docs/stato-e-roadmap.md',
]

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


# Un id HTML esplicito: `<a id="..."></a>` prima di un titolo. Serve quando
# l'ancora generata dal titolo non e' scrivibile comodamente altrove — il
# titolo «Identità e fingerprint» genera `identità-e-fingerprint`, con
# l'accento, e i riferimenti nei commenti Rust lo scrivono senza, puntando
# a un'ancora che non esiste. Un id esplicito e' ASCII, stabile, e
# non cambia se un giorno il titolo viene riformulato.
ID_ESPLICITO = re.compile(r'<a\s+id="([\w-]+)"\s*>\s*</a>')


def ancore_di(percorso):
    """Le ancore di un documento: dai titoli e dagli id HTML espliciti."""
    contenuto = testo(percorso)
    trovate = {ancora(riga.lstrip('#').strip())
               for riga in contenuto.split('\n')
               if riga.startswith('#')}
    trovate.update(ID_ESPLICITO.findall(contenuto))
    return trovate


def _doppie(contenuto):
    """Le ancore che rimandano a DUE PUNTI DIVERSI dello stesso documento.

    Non basta contare le occorrenze. Un id esplicito messo sopra un titolo —
    `<a id="alias-legacy"></a>` seguito da `## Alias legacy` — genera lo
    stesso nome due volte, ma indica un solo punto: e' il modo idiomatico di
    dare a una sezione un'ancora ASCII stabile, e segnalarlo sarebbe rumore
    sulla forma idiomatica adottata qui: senza questa distinzione la prova
    negativa di `prova_di_mutazione_ancore` fallirebbe.

    Il difetto vero e' un'ancora che porta in due posti: allora chi la cita
    finisce nel primo, che non e' detto sia quello giusto. Ogni definizione
    dichiara quindi il proprio BERSAGLIO — la riga del titolo che precede, o
    la propria — e l'ancora e' doppia solo se i bersagli sono piu' di uno.

    Prende il testo e non il percorso perche' cosi' la mutazione di prova puo'
    passargli un documento finto.
    """
    righe = contenuto.split('\n')
    bersagli = {}
    for numero, riga in enumerate(righe):
        if riga.startswith('#'):
            bersagli.setdefault(ancora(riga.lstrip('#').strip()),
                                set()).add(numero)
            continue
        for identificatore in ID_ESPLICITO.findall(riga):
            # Il titolo che segue, saltando le righe vuote: e' quello che
            # l'id sta battezzando.
            successiva = numero + 1
            while successiva < len(righe) and not righe[successiva].strip():
                successiva += 1
            titolo = (successiva if successiva < len(righe)
                      and righe[successiva].startswith('#') else numero)
            bersagli.setdefault(identificatore, set()).add(titolo)
    return {nome for nome, punti in bersagli.items()
            if nome and len(punti) > 1}


def ancore_doppie(percorso):
    return _doppie(testo(percorso))


def documenti(file_tracciati):
    """I Markdown tracciati, esclusi i sorgenti della generazione e i manifesti."""
    return [p for p in file_tracciati
            if p.endswith('.md') and not SORGENTI_GENERATE.match(p)
            and not ESENTI.match(p)]


def controlla_collegamenti(file_tracciati, problemi):
    for percorso in documenti(file_tracciati):
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


# Un riferimento in chiaro a una sezione: `piano-v5.md#alias-legacy` dentro
# un commento Rust, dove la sintassi dei link Markdown non esiste e il
# controllo dei collegamenti non guarda.
RIFERIMENTO_TESTUALE = re.compile(r'([\w-]+\.md)#([\w-]+)')


def ancore_note(file_tracciati):
    """nome del documento -> ancore, per tutti i `.md` tracciati.

    La chiave e' il NOME, non il percorso, perche' in chiaro si cita
    `piano-v5.md#...`, non `docs/piano-v5.md#...`. Due documenti con lo stesso
    nome (c'e' un `README.md` anche sotto `examples/`) mettono in comune le
    proprie ancore: e' la lettura piu' generosa, e in cambio non serve
    indovinare quale dei due intenda chi scrive.
    """
    fuori = {}
    for percorso in file_tracciati:
        if not percorso.endswith('.md') or not os.path.exists(percorso):
            continue
        fuori.setdefault(os.path.basename(percorso), set()).update(
            ancore_di(percorso))
    return fuori


def ancore_rotte(riga, note):
    """I riferimenti testuali della riga che non risolvono."""
    fuori = []
    for nome, frammento in RIFERIMENTO_TESTUALE.findall(riga):
        if nome not in note:
            fuori.append(('%s#%s' % (nome, frammento),
                          'non esiste un documento tracciato con questo nome'))
        elif frammento not in note[nome]:
            fuori.append(('%s#%s' % (nome, frammento),
                          'il documento c\'e\', la sezione no'))
    return fuori


def controlla_ancore_testuali(file_tracciati, problemi):
    note = ancore_note(file_tracciati)

    for percorso in sorted(file_tracciati):
        if not percorso.endswith('.md') or not os.path.exists(percorso):
            continue
        doppie = ancore_doppie(percorso)
        if doppie:
            problemi.append(
                'ANCORA DUPLICATA %s: %s\n'
                '  Due definizioni della stessa ancora rendono il rinvio '
                'ambiguo: si finisce nella prima, che non e\' detto sia '
                'quella giusta.' % (percorso, ', '.join(sorted(doppie))))

    for percorso in file_tracciati:
        if ESENTI.match(percorso):
            continue
        if not percorso.endswith(TESTUALI):
            continue
        if percorso == 'scripts/verifica_documentazione.py':
            continue  # contiene le mutazioni di prova
        if not os.path.exists(percorso):
            continue
        for numero, riga in enumerate(testo(percorso).split('\n'), 1):
            for bersaglio, motivo in ancore_rotte(riga, note):
                problemi.append(
                    'ANCORA INESISTENTE %s:%d -> %s\n  %s.' % (
                        percorso, numero, bersaglio, motivo))


def prova_di_mutazione_ancore(file_tracciati):
    """Mutazione su un riferimento nudo: l'ancora storpiata va vista."""
    note = ancore_note(file_tracciati)
    problemi = []
    vero = '/// Identita\' del grafo (piano-v5.md#identita-e-fingerprint).'
    if ancore_rotte(vero, note):
        problemi.append(
            'IL GATE E\' RUMOROSO: %r segnalato come rotto.\n'
            '  Se il controllo boccia i riferimenti giusti, il primo effetto '
            'e\' che si smette di leggerlo.' % vero)
    for mutato in (vero.replace('#identita-e-fingerprint', '#identita'),
                   vero.replace('piano-v5.md', 'piano-v4.md')):
        if not ancore_rotte(mutato, note):
            problemi.append(
                'IL GATE E\' CIECO: %r non viene visto.\n'
                '  I riferimenti in chiaro non hanno la sintassi dei link, e '
                'senza questo controllo nessuno li verifica: e\' cosi\' che '
                '79 rinvii a un\'ancora inesistente sono sopravvissuti al '
                'reset.' % mutato)

    # E la duplicazione, nelle due forme in cui si presenta: due titoli
    # uguali, e un id esplicito ripetuto. Un ramo di controllo che non si e'
    # mai visto fallire non e' un controllo, e' un'intenzione.
    doppioni = [
        '## Sezione\n\ntesto\n\n## Sezione\n',
        '<a id="x"></a>\n\n## Uno\n\n<a id="x"></a>\n\n## Due\n',
    ]
    for documento in doppioni:
        if not _doppie(documento):
            problemi.append(
                'IL GATE E\' CIECO: un\'ancora duplicata non viene vista.\n'
                '  Due definizioni della stessa ancora mandano il lettore '
                'nella prima, che non e\' detto sia quella giusta.')
    if _doppie('## Uno\n\n<a id="due"></a>\n\n## Due\n'):
        problemi.append(
            'IL GATE E\' RUMOROSO: un documento senza duplicati e\' stato '
            'segnalato.')
    return problemi


def _definizioni(problemi):
    """Gli identificatori definiti, e quelli definiti piu' di una volta."""
    trovate = []
    for percorso in DOVE_SI_DEFINISCE:
        if not os.path.exists(percorso):
            problemi.append('SORGENTE DI DEFINIZIONI ASSENTE: %s' % percorso)
            continue
        for coppia in DEFINIZIONE.findall(testo(percorso)):
            trovate.append(coppia[0] or coppia[1])
    doppie = sorted({i for i in trovate if trovate.count(i) > 1}, key=ordine)
    if doppie:
        problemi.append(
            "PUNTATORI DEFINITI PIU\' DI UNA VOLTA: %r\n"
            "  Due definizioni dello stesso identificatore sono due contratti "
            "per la stessa cosa, e chi legge non sa quale valga." % doppie)
    return set(trovate)


def ordine(ident):
    return (ident[0], [int(pezzo) for pezzo in ident[1:].split('.')])


CODE_SPAN = re.compile(r'`[^`]*`')


def _senza_code_span(riga):
    """La riga senza cio' che sta fra backtick.

    Un backtick dice «questo e' codice»: `I64` in un commento nomina un tipo
    Rust, non un puntatore interno. Contarlo produrrebbe un falso positivo
    ogni volta che un commento cita una variante di enum, ed e' cosi' che un
    gate smette di essere letto.
    """
    return CODE_SPAN.sub(' ', riga)


def _commenti(percorso, contenuto):
    """Le sole righe di PROSA: commenti nei sorgenti, tutto nei documenti.

    Un puntatore vive in un commento. `const M1: usize = 1_000_000` e
    `I64(&Int64Array)` sono codice: hanno la forma di un identificatore e non
    lo sono, e un gate che non sa distinguerli produce falsi positivi finche'
    qualcuno non lo spegne.
    """
    if not percorso.endswith('.rs'):
        return contenuto.split(chr(10))
    righe = []
    for riga in contenuto.split(chr(10)):
        nuda = riga.lstrip()
        if nuda.startswith('//'):
            righe.append(riga)
        elif '//' in riga:
            righe.append(riga[riga.index('//'):])
        else:
            righe.append('')
    return righe


def controlla_decisioni(file_tracciati, problemi):
    """Ogni puntatore citato dev\'essere definito, e ogni definizione citata.

    Gli identificatori interni sono un secondo sistema di riferimenti, accanto
    ai collegamenti Markdown, e si rompe allo stesso modo: la cosa nominata
    sparisce e il commento che la cita resta, convincente e senza referente.
    I documenti attuali definiscono solo cio\' che vale adesso, quindi un
    puntatore citato e non definito e\' una cosa superata che qualcuno
    continua a invocare.
    """
    definiti = _definizioni(problemi)
    if not definiti:
        problemi.append(
            "NESSUN PUNTATORE DEFINITO.\n"
            "  Se non ne esistono piu\', vanno tolti anche i riferimenti "
            "nel codice.")
        return

    citazioni = {}
    for percorso in file_tracciati:
        if ESENTI.match(percorso):
            continue
        if not percorso.endswith(TESTUALI):
            continue
        if percorso in DOVE_SI_DEFINISCE:
            continue
        if percorso == 'scripts/verifica_documentazione.py':
            continue
        if not os.path.exists(percorso):
            continue
        for numero, riga in enumerate(_commenti(percorso, testo(percorso)), 1):
            for trovato in PUNTATORE.finditer(_senza_code_span(riga)):
                citazioni.setdefault(trovato.group(1), (percorso, numero))

    orfani = sorted(set(citazioni) - definiti, key=ordine)
    if orfani:
        righe = [('    %-9s %s:%d' % (i, citazioni[i][0], citazioni[i][1]))
                 for i in orfani]
        problemi.append(
            "PUNTATORI CITATI E NON DEFINITI (%d):\n%s\n"
            "  I documenti attuali definiscono solo cio\' che vale adesso.\n"
            "  Un puntatore superato non si archivia: si sostituisce con la "
            "descrizione o con la sezione corrente."
            % (len(orfani), chr(10).join(righe)))

    inutilizzati = sorted(definiti - set(citazioni), key=ordine)
    if inutilizzati:
        problemi.append(
            "PUNTATORI DEFINITI E MAI CITATI: %r\n"
            "  Un registro che cresce e non viene letto torna a essere un "
            "archivio. Se la cosa vale ancora ma nessuno la cita, il "
            "riferimento va messo dove e\' attuata; se non vale piu\', va "
            "tolta." % inutilizzati)


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


#: Nomi di base, dentro vendor/, che questo progetto scrive di suo — non
#: codice o testo upstream. Un file PER NOME, non un pattern largo: ogni
#: aggiunta e' una decisione dichiarata qui.
BASENAME_NOSTRI_IN_VENDOR = ('PROVENANCE.md', 'PROVENANCE-FILTRO-SPERIMENTALE.md')


def _e_terza_parte(percorso):
    """True solo per il codice/testo upstream dentro vendor/, non per i file
    che questo progetto ci ha scritto sopra (oggi: i PROVENANCE.md e il
    PROVENANCE-FILTRO-SPERIMENTALE.md del candidato sperimentale).

    L'eccezione esiste per il testo che non abbiamo scritto — un commento di
    `geo` che contiene «E2», un `CHANGES.md` upstream con due sezioni
    «changed» non sono uno scostamento di QUESTA documentazione. Non deve
    diventare un'esenzione per la documentazione nostra: questi file restano
    soggetti alle stesse regole di tutto il resto — puntatori e ancore."""
    return (percorso.startswith('vendor/')
            and os.path.basename(percorso) not in BASENAME_NOSTRI_IN_VENDOR)


def main():
    problemi = []
    file_tracciati = [p for p in tracciati() if not _e_terza_parte(p)]
    # `controlla_artefatti`, sotto, non usa questa lista — enumera da se' con
    # `git ls-files`, quindi resta attivo su tutto `vendor/` invariato: un
    # binario vendorizzato per errore continua a essere rilevato anche nel
    # codice upstream escluso qui.
    controlla_collegamenti(file_tracciati, problemi)
    controlla_ancore_testuali(file_tracciati, problemi)
    problemi.extend(prova_di_mutazione_ancore(file_tracciati))
    controlla_decisioni(file_tracciati, problemi)
    controlla_catalogo(problemi)
    controlla_generato(problemi)
    controlla_artefatti(problemi)

    if problemi:
        sys.stderr.write('la documentazione non e\' quella decisa:\n\n')
        for problema in problemi:
            sys.stderr.write('- %s\n' % problema)
        raise SystemExit(1)

    definiti = len(_definizioni([]))
    riferimenti = sum(
        len(RIFERIMENTO_TESTUALE.findall(riga))
        for percorso in file_tracciati
        if percorso.endswith(TESTUALI) and os.path.exists(percorso)
        and not ESENTI.match(percorso)
        and percorso != 'scripts/verifica_documentazione.py'
        for riga in testo(percorso).split('\n'))
    print('documentazione coerente: %d documenti, %d puntatori definiti e '
          'tutti citati, %d riferimenti testuali a una sezione tutti risolti e '
          'nessuna ancora duplicata, catalogo e operazioni allineati, nessun '
          'artefatto tracciato'
          % (len(documenti(file_tracciati)), definiti, riferimenti))


main()
