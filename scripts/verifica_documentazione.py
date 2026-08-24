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
4. **ogni puntatore interno ha una definizione corrente**, e ogni
   definizione e' citata. Vale per tutte le famiglie — `D`, `M`, `V`, `E`,
   `I`, `P`, `G` — con l'identificatore intero, anche multilivello. Un
   commento che scrive `D14.5.6` o `M3` deve poterlo risolvere, altrimenti
   e' un puntatore morto come un link rotto; e una definizione che nessuno
   cita torna a essere un archivio. Sono cercati **solo nei commenti**: `const
   M1: usize` e `I64(&Int64Array)` hanno la forma di un puntatore e sono
   codice;
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
    # Progetto tecnico della fase 4 (esecuzione isolata). Aggiunto il
    # 2026-08-24: e' una decisione, non un effetto collaterale — il
    # disegno non stava in stato-e-roadmap.md senza soffocarlo.
    'docs/isolamento.md',
    'docs/prototipi-isolamento.md',
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
    # Anche il richiamo nudo: «l'ADR dichiarava» manda a cercare un file
    # che non c'e' piu', esattamente come un link rotto.
    (r'\bADR\b', 'riferimento a una ADR eliminata'),
    (r'\bverbal[ei]\b', 'i verbali di review sono stati eliminati'),
    (r'\breview del \d{4}-\d{2}-\d{2}', 'riferimento a un verbale di review'),
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

# ---------------------------------------------------------------------------
# Puntatori malformati
# ---------------------------------------------------------------------------
#
# Due residui che la sostituzione delle vecchie sigle ha lasciato dietro di
# se', e che nessuno degli altri controlli vede perche' non sono link rotti:
# il file c'e', l'ancora c'e', e' cio' che sta INTORNO a essere sbagliato.
#
#   1. il frammento di sezione orfano. «ADR 15 §3» nominava la terza sezione
#      di quella ADR; diventato «errori-e-limiti.md#memoria-governata §3»,
#      quel «§3» manda a cercare una numerazione che nei documenti nuovi non
#      esiste. Il 2026-08-22: sette siti con «§» — fra cui due «§em.», la
#      sezione degli emendamenti di una ADR cancellata — e cinque con «par.»;
#   2. il riferimento incollato dalla barra. «ADR 6/ADR 5» separava due sigle
#      brevi; diventato «errori-e-limiti.md/architettura.md#planner-ed-executor»
#      si legge come un percorso di filesystem che non esiste. Undici siti:
#      tre trovati a mano, gli altri otto da questo controllo — cercandoli a
#      mano si guarda un `.md` anche a destra della barra, e «/5» o «/R12»
#      non lo e'.
#
# Un «§» resta legittimo quando dice DI CHE COSA e' la sezione: l'ICD e i
# contratti trasversali hanno sezioni numerate, e citarle e' giusto. Il
# controllo scatta solo su un «§» che segue un puntatore a un documento
# nostro senza attribuzione: e' li' che il numero e' rimasto orfano.
#
# Il frammento orfano si scrive in due modi — «§3» e «par. 3» — e il secondo
# e' arrivato dai documenti di radice cancellati («Architetture.md par. 3.3»
# diventato «architettura.md e par. 2»). Vale la stessa regola: subito dopo
# un puntatore a un documento nostro, un numero di sezione o di paragrafo
# senza attribuzione e' un rinvio a una numerazione che non esiste.
SEZIONE_ATTRIBUITA = re.compile(
    r'(?:ICD|v\d[\w.-]*|Appendice\s+\w+|tabella|sezione)\s*(?:§|par(?:\.|agrafo))')
FRAMMENTO = re.compile(r'§|\bpar(?:\.|agrafo)\s*\d')
RIFERIMENTO_MD = re.compile(r'[\w-]+\.md(?:#[\w-]+)?')
INCOLLATO = re.compile(r'[\w-]+\.md(?:#[\w-]+)?/')

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
# nomina e' descritta, non in un registro unico: `M3` e' una milestone
# della roadmap. L'esempio eseguibile non ha piu' un
# identificatore: nessuno lo citava come `E1`, e un puntatore che
# nessuno usa e' un archivio di una voce sola.
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
# l'accento, e i 79 riferimenti nei commenti Rust lo scrivevano senza,
# puntando a un'ancora che non esisteva. Un id esplicito e' ASCII, stabile, e
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
    sulla forma che abbiamo appena adottato (l'ha scoperto la prova negativa
    di `prova_di_mutazione_ancore`, che senza questa distinzione falliva).

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


def puntatori_malformati(riga):
    """I difetti di forma in una riga: lista di (difetto, spiegazione).

    Qui i backtick NON si tolgono, al contrario che nel controllo dei
    puntatori interni. La ragione e' opposta: li' un backtick dice «questo e'
    un tipo Rust, non un puntatore»; qui la forma con i backtick —
    `docs/errori-e-limiti.md` — e' il modo NORMALE di citare un documento, e
    ignorarla renderebbe il controllo cieco proprio sui siti piu' comuni.
    Tre riferimenti a un file cancellato erano sopravvissuti cosi'.
    """
    pulita = riga
    trovati = []

    incollato = INCOLLATO.search(pulita)
    if incollato is not None:
        trovati.append((
            incollato.group(0),
            'un puntatore a documento seguito da `/` si legge come un '
            'percorso. Due riferimenti sono due riferimenti: separali con '
            'una virgola.'))

    md = RIFERIMENTO_MD.search(pulita)
    if md is not None:
        for frammento in FRAMMENTO.finditer(pulita, md.end()):
            # L'attribuzione deve stare ATTACCATA al frammento: cercarla in
            # tutta la riga assolverebbe un «§3» orfano solo perche' altrove
            # c'e' un «ICD §9» legittimo.
            inizio = max(0, frammento.start() - 24)
            contesto = pulita[inizio:frammento.end()]
            attribuzione = SEZIONE_ATTRIBUITA.search(contesto)
            if attribuzione is not None and attribuzione.end() == len(contesto):
                continue
            trovati.append((
                frammento.group(0).strip(),
                'frammento di sezione orfano dopo %r: i documenti nuovi non '
                'hanno sezioni numerate, e chi legge cerca qualcosa che non '
                'esiste.' % md.group(0)))
            break
    return trovati


def controlla_puntatori_malformati(file_tracciati, problemi):
    for percorso in file_tracciati:
        if ESENTI_DAI_MORTI.match(percorso):
            continue
        if not percorso.endswith(TESTUALI):
            continue
        if percorso == 'scripts/verifica_documentazione.py':
            continue  # e' l'elenco stesso
        if not os.path.exists(percorso):
            continue
        for numero, riga in enumerate(testo(percorso).split('\n'), 1):
            for difetto, spiegazione in puntatori_malformati(riga):
                problemi.append(
                    'PUNTATORE MALFORMATO %s:%d: %r\n  %s'
                    % (percorso, numero, difetto, spiegazione))


# Le due forme che il controllo deve vedere, e le tre che deve lasciar
# passare. Un rilevatore che segnala tutto e' inutile quanto uno cieco.
MUTAZIONI_PUNTATORI = [
    'residuo (errori-e-limiti.md#memoria-governata §3) da rimuovere',
    'residuo (errori-e-limiti.md/architettura.md#planner-ed-executor) qui',
    'canone GeoArrow-WKB (architettura.md e par. 2)',
    # Un frammento legittimo sulla stessa riga non deve assolvere l'orfano:
    # e' l'errore che una ricerca sull'intera riga farebbe.
    'publish (errori-e-limiti.md#publish-e-cleanup, ICD §9) e poi §3 orfano',
]
LEGITTIMI_PUNTATORI = [
    'rename atomico di publish (errori-e-limiti.md#publish-e-cleanup, ICD §9)',
    'assi §9 di R9.7, e piano-v5.md#contratti-di-input per il resto',
    'due riferimenti (errori-e-limiti.md, architettura.md#planner-ed-executor)',
]


def prova_di_mutazione_puntatori():
    """Inietta le due forme e pretende che il controllo le veda."""
    problemi = []
    for mutazione in MUTAZIONI_PUNTATORI:
        if not puntatori_malformati(mutazione):
            problemi.append(
                'IL GATE E\' CIECO: %r non viene visto.\n'
                '  Senza questo controllo i residui della riscrittura dei '
                'riferimenti rientrano al primo cambio.' % mutazione)
    for legittimo in LEGITTIMI_PUNTATORI:
        difetti = puntatori_malformati(legittimo)
        if difetti:
            problemi.append(
                'IL GATE E\' RUMOROSO: %r segnalato come malformato (%r).\n'
                '  Un controllo che segnala anche cio\' che e\' giusto '
                'smette di essere letto.' % (legittimo, difetti[0][0]))
    return problemi


# Un riferimento in chiaro a una sezione: `piano-v5.md#alias-legacy` dentro
# un commento Rust, dove la sintassi dei link Markdown non esiste. Il
# controllo dei collegamenti non li vede — guarda i soli `[testo](file#anc)`
# nei documenti pubblici — e infatti 79 riferimenti a un'ancora inesistente
# sono vissuti per tutto il reset senza che nulla protestasse.
RIFERIMENTO_TESTUALE = re.compile(r'([\w-]+\.md)#([\w-]+)')


def ancore_note(file_tracciati):
    """nome del documento -> ancore, per tutti i `.md` tracciati.

    La chiave e' il NOME, non il percorso, perche' in chiaro si cita
    `piano-v5.md#...`, non `docs/piano-v5.md#...`. Due documenti con lo stesso
    nome (c'e' un `README.md` anche sotto `examples/`) mettono in comune le
    proprie ancore: e' la lettura piu' generosa, e in cambio non serve
    indovinare quale dei due intendesse chi scriveva.
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
        if ESENTI_DAI_MORTI.match(percorso):
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
        if ESENTI_DAI_MORTI.match(percorso):
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


def main():
    problemi = []
    file_tracciati = tracciati()
    controlla_superficie(file_tracciati, problemi)
    controlla_collegamenti(problemi)
    controlla_riferimenti_morti(file_tracciati, problemi)
    controlla_puntatori_malformati(file_tracciati, problemi)
    problemi.extend(prova_di_mutazione_puntatori())
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
        and not ESENTI_DAI_MORTI.match(percorso)
        and percorso != 'scripts/verifica_documentazione.py'
        for riga in testo(percorso).split('\n'))
    print('documentazione coerente: %d documenti pubblici, %d eccezioni, '
          '%d puntatori definiti e tutti citati, nessun riferimento morto, '
          'nessun puntatore malformato (%d mutazioni iniettate e viste, %d '
          'forme legittime lasciate passare), %d riferimenti testuali a una '
          'sezione tutti risolti e nessuna ancora duplicata, catalogo e '
          'operazioni allineati, nessun artefatto tracciato'
          % (len(PUBBLICI), len(ECCEZIONI), definiti,
             len(MUTAZIONI_PUNTATORI), len(LEGITTIMI_PUNTATORI), riferimenti))


main()
