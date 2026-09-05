# -*- coding: utf-8 -*-
"""Gate: i commenti dicono il presente, non il diario del lavoro.

# Perche' serve

Un commento sopravvive al lavoro che lo ha prodotto. «Fase 2A», «nono giro,
finding 4», «prima del fix», «il 2026-08-06 il gate non esisteva» dicono una
cosa vera il giorno in cui vengono scritte e falsa poco dopo: chi legge il
codice non ha modo di sapere quale meta' della frase valga ancora. La
responsabilita', l'invariante, l'hazard e l'ordine operativo vanno tenuti; il
diario intorno a loro no.

Il gate impedisce che la cronaca rientri. Non riscrive nulla e non ha un
elenco di deroghe per file: o la forma non c'e', o e' ammessa per una ragione
LEGGIBILE NEL TESTO STESSO — un pin, un atto normativo datato, un artefatto
immutabile citato con il proprio qualificatore, un valore che la data
traduce. I puntatori governati dal gate documentale non sono un'ammissione:
sono spenti prima delle regole, perche' la loro forma non venga scambiata
per una sigla di campagna.

# Che cosa cerca

1. **`marcatore-anonimo`** — `TODO`, `FIXME`, `HACK`, `XXX`. Un marcatore
   senza proprietario e senza scadenza non e' un impegno: e' un residuo che
   nessuno raccoglie.
2. **`data-di-calendario`** — ammessa solo dove FISSA qualcosa: un pin
   (`@ 2026-08-23`, `nightly-2026-08-01`), un atto normativo datato
   (`emendamento 2026-08-17`), un artefatto immutabile citato con il proprio
   qualificatore (`run 31166125840`, `baseline af812aa`), o una data che e'
   un DATO — e allora la relazione che lo dice le sta attaccata
   (`2026-07-25 = 20659 giorni`), oppure il valore sta sulla riga di codice
   E il commento e' soltanto quella data, non una frase che la contiene.
3. **`giro-di-review`** — `finding`, `hotfix`, `PR-3`, `nono giro`. Il numero
   del giro dice quando e' stato trovato un difetto, non quale proprieta'
   sorvegli il codice. Resta ammesso il giro come ITERAZIONE («dal secondo
   giro in poi lo schema e' stabile»), che e' un fatto di esecuzione.
4. **`tappa-di-campagna`** — `Fase 2A`, `Milestone D`, `secondo batch`,
   `B1.3`, `filone`, `cantiere`. Una tappa e' finita per definizione, e
   `milestone` non ha eccezioni: la voce di roadmap si nomina con il proprio
   puntatore — `M3` — che il gate documentale definisce e questo gate non
   tocca.
5. **`stesura-precedente`** — «la prima versione», «la versione precedente»,
   «prima del fix», «come prima». Descrivono un testo o un codice che non
   esiste piu' in albero, quindi non sono verificabili da chi legge.
6. **`difetto-all-imperfetto`** — «lasciava passare», «non veniva
   riconosciuto», «faceva scambiare». La spiegazione tecnica e' preziosa e
   deve restare: va scritta al controfattuale presente — «cercare solo il
   nome LETTERALE lascerebbe passare i siti che usano un helper» — che dice
   la stessa cosa ed e' vera anche fra un anno.

# Come guarda i sorgenti

Guarda **soltanto la prosa**: i commenti di Rust, Python, shell, YAML, TOML e
Dockerfile, e per Python anche le stringhe triple, che qui sono docstring. Le
stringhe di codice restano fuori: un messaggio d'errore che cita una data e'
comportamento osservabile, e si giudica come tale, non come commento.

Rust ha un lexer proprio (stringhe grezze `r#"..."#`, commenti annidati),
Python usa `tokenize`, il resto e' a righe con le stringhe rimosse prima di
cercare il `#`.

# Che cosa NON fa

Non valida i puntatori interni: quelli sono di `verifica_documentazione.py`,
che ne verifica la reciprocita' con i documenti. Qui le definizioni si
IMPORTANO da quel gate, per non avere due elenchi che si scostano.

    python scripts/verifica_commenti.py

Esce 1 al primo commento che racconta invece di dichiarare.
"""
import importlib.util
import io
import os
import re
import sys
import tokenize

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

#: Le radici del codice, per NOME. Non un walk della radice del repository:
#: `target*/`, gli artefatti dei container e le directory di scratch non sono
#: codice, e un gate che le attraversa dipende da che cosa c'e' sul disco.
#: `docs/_build` e' codice come gli altri — genera un documento verificato da
#: un gate — anche se abita sotto `docs/`. I `.md` restano invece fuori: sono
#: documentazione, e si governano altrove.
PERIMETRO = ('crates', 'fuzz', 'scripts', 'prototipi', 'benchmarks', '.github',
             'docs/_build')

#: File di radice che sono configurazione del progetto.
RADICE_FILE = ('Cargo.toml', 'rust-toolchain.toml')

#: `.ps1` NON e' qui. PowerShell ha il commento di blocco `<# ... #>` e le
#: proprie regole di quoting: leggerlo a righe scambia una stringa per un
#: commento, e mezzo lexer e' peggio di nessun lexer. Nel repository non
#: c'e' PowerShell; il giorno che ci fosse, prima il lexer.
ESTENSIONI = ('.rs', '.py', '.sh', '.toml', '.yml', '.yaml')

#: Directory che non contengono prosa da sorvegliare, a qualunque profondita'.
SALTA = ('target', 'corpus', 'artifacts', '__pycache__', '.git', 'raw')


def sorgenti_del_repository():
    """Percorso relativo -> contenuto, per ogni sorgente del perimetro."""
    trovati = {}
    for nome in RADICE_FILE:
        percorso = os.path.join(RADICE, nome)
        if os.path.exists(percorso):
            trovati[nome] = leggi(percorso)
    for radice in PERIMETRO:
        for cartella, sottocartelle, file in os.walk(os.path.join(RADICE, radice)):
            sottocartelle[:] = [
                s for s in sottocartelle
                if not s.startswith('.') and not s.startswith('target')
                and s not in SALTA
            ]
            for nome in sorted(file):
                if not nome.endswith(ESTENSIONI) and not nome.startswith('Dockerfile'):
                    continue
                percorso = os.path.join(cartella, nome)
                rel = os.path.relpath(percorso, RADICE).replace(os.sep, '/')
                trovati[rel] = leggi(percorso)
    return trovati


def leggi(percorso):
    with open(percorso, 'rb') as sorgente:
        return sorgente.read().decode('utf-8')


# ---------------------------------------------------------------------------
# Estrazione della prosa
# ---------------------------------------------------------------------------
#
# Un gate che cerca parole in TUTTO il file segnala i messaggi d'errore, i
# nomi dei test e i dati delle fixture. Sono comportamento osservabile, non
# commenti, e si giudicano altrove: qui vanno esclusi, altrimenti il gate
# diventa rumoroso e chi lo legge impara a spegnerlo.
#
# Distinguere le due cose richiede di seguire la sintassi, e un sorgente che
# non si lascia seguire non e' un sorgente pulito: una stringa aperta e mai
# chiusa nasconde tutti i commenti che seguono, e un gate che tace su quel
# file dichiara «nessuna violazione» dopo non aver guardato niente. Ogni
# lessico si ferma quindi con un errore che dice file e riga.


class ErroreLessicale(Exception):
    """Un sorgente che non si lascia leggere fino in fondo.

    Non e' un difetto dei commenti, ed e' peggio: e' la prova che il verdetto
    su quel file non vale. Sale fino a [`controlla`], che la registra come
    violazione con la riga da cui la lettura non e' piu' affidabile.
    """

    def __init__(self, riga, motivo):
        super().__init__(motivo)
        self.riga = riga
        self.motivo = motivo


def commenti_rust(contenuto):
    """(riga, testo) di ogni commento Rust, stringhe escluse.

    Serve un lexer, non una regex: `"// non e' un commento"` e' una stringa,
    `r#"..."#` puo' contenere qualunque cosa, e `/* /* */ */` e' annidato.
    """
    fuori = []
    riga = 1
    i = 0
    n = len(contenuto)
    while i < n:
        c = contenuto[i]
        if c == '\n':
            riga += 1
            i += 1
        elif contenuto.startswith('//', i):
            fine = contenuto.find('\n', i)
            fine = n if fine < 0 else fine
            fuori.append((riga, contenuto[i:fine]))
            i = fine
        elif contenuto.startswith('/*', i):
            partenza = riga
            profondita = 0
            inizio = i
            while i < n:
                if contenuto.startswith('/*', i):
                    profondita += 1
                    i += 2
                elif contenuto.startswith('*/', i):
                    profondita -= 1
                    i += 2
                    if profondita == 0:
                        break
                else:
                    if contenuto[i] == '\n':
                        riga += 1
                    i += 1
            if profondita:
                raise ErroreLessicale(partenza, 'commento di blocco mai chiuso')
            fuori.append((partenza, contenuto[inizio:i]))
        elif c == 'r' and re.match(r'r#*"', contenuto[i:i + 16] or ''):
            cancelletti = len(re.match(r'r(#*)"', contenuto[i:]).group(1))
            chiusura = '"' + '#' * cancelletti
            fine = contenuto.find(chiusura, i + cancelletti + 2)
            if fine < 0:
                raise ErroreLessicale(riga, 'stringa grezza mai chiusa')
            fine += len(chiusura)
            riga += contenuto.count('\n', i, fine)
            i = fine
        elif c in '"\'':
            # Il letterale di carattere e la lifetime hanno lo stesso apice:
            # `'a` non apre nulla, `'\n'` si'. Si distingue dalla chiusura.
            if c == '\'' and not re.match(r"'(?:\\.|[^\\'])'", contenuto[i:i + 5] or ''):
                i += 1
                continue
            partenza = riga
            i += 1
            while i < n and contenuto[i] != c:
                if contenuto[i] == '\\':
                    # La continuazione di riga (`\` a fine riga) e' una
                    # sequenza di escape come le altre, ma il carattere che
                    # nasconde e' un a capo: saltarlo senza contarlo sposta
                    # OGNI riga successiva, e il gate indica il posto
                    # sbagliato.
                    i += 1
                    if i < n and contenuto[i] == '\n':
                        riga += 1
                elif contenuto[i] == '\n':
                    riga += 1
                i += 1
            if i >= n:
                raise ErroreLessicale(partenza, 'stringa mai chiusa')
            i += 1
        else:
            i += 1
    return fuori


def commenti_python(contenuto):
    """(riga, testo) dei commenti Python e delle stringhe triple.

    Le stringhe triple entrano perche' qui SONO prosa: ogni script di gate
    dichiara in docstring che cosa sorveglia e perche'. Le stringhe normali
    no: sono messaggi.
    """
    fuori = []
    lettore = io.StringIO(contenuto).readline
    try:
        for token in tokenize.generate_tokens(lettore):
            if token.type == tokenize.COMMENT:
                fuori.append((token.start[0], token.string))
            elif token.type == tokenize.STRING:
                nudo = token.string.lstrip('rbufRBUF')
                if nudo.startswith('"""') or nudo.startswith("'''"):
                    fuori.append((token.start[0], token.string))
    except tokenize.TokenError as guasto:
        # `TokenError` e' proprio il caso pericoloso: una tripla mai chiusa
        # inghiotte il resto del file, e i commenti che seguono non arrivano
        # mai qui. Tacere significherebbe dichiarare pulito cio' che non e'
        # stato letto.
        posizione = guasto.args[1] if len(guasto.args) > 1 else (0, 0)
        raise ErroreLessicale(posizione[0] if isinstance(posizione, tuple) else 0,
                              'sorgente Python non tokenizzabile: %s'
                              % guasto.args[0]) from guasto
    except (IndentationError, SyntaxError) as guasto:
        raise ErroreLessicale(getattr(guasto, 'lineno', 0) or 0,
                              'sorgente Python non tokenizzabile: %s'
                              % guasto.msg) from guasto
    return fuori


APICI = re.compile(r'"(?:\\.|[^"\\])*"' + r"|'(?:\\.|[^'\\])*'")

def commenti_a_cancelletto(contenuto):
    """(riga, testo) per shell, YAML, TOML e Dockerfile.

    Le stringhe si tolgono PRIMA di cercare il `#`, altrimenti
    `echo "# non un commento"` diventa un commento. Il `#` conta solo a
    inizio riga o dopo uno spazio: `sha256#frammento` e `${VAR#prefisso}`
    non aprono niente.
    """
    fuori = []
    for numero, riga in enumerate(contenuto.split('\n'), start=1):
        nuda = APICI.sub(lambda m: ' ' * len(m.group(0)), riga)
        trovato = re.search(r'(?:^|\s)#', nuda)
        if not trovato:
            continue
        inizio = trovato.end() - 1
        if riga.lstrip().startswith('#!') and numero == 1:
            continue
        fuori.append((numero, riga[inizio:]))
    return fuori


def con_codice(commenti, contenuto):
    """A ogni commento si affianca il CODICE che lo precede sulla sua riga.

    Un commento in coda a una riga parla di quella riga: `// 2022-01-01`
    accanto a `assert_eq!(values.value(2), 18_993)` traduce un valore, non
    data un lavoro. Senza la riga, quella distinzione non e' visibile.
    """
    righe = contenuto.split('\n')
    fuori = []
    for riga, testo in commenti:
        intera = righe[riga - 1] if riga - 1 < len(righe) else ''
        taglio = intera.find(testo.split('\n')[0])
        fuori.append((riga, testo, intera[:taglio].strip() if taglio > 0 else ''))
    return fuori


def raggruppa(commenti):
    """Righe di commento CONSECUTIVE unite in un blocco solo.

    Chi legge vede un blocco, non sei righe: la data sta su una riga e
    l'artefatto che la giustifica su quella prima. Giudicarle separatamente
    darebbe un falso positivo a ogni commento che va a capo.

    Si uniscono solo commenti della **stessa specie** e con NIENTE codice
    davanti: un blocco `/* */` e la riga `//` che lo segue sono due commenti,
    e un commento in coda a una riga di codice parla di quella riga — se
    assorbisse le righe successive porterebbe con se' un contesto che non e'
    il loro.
    """
    blocchi = []
    for riga, testo, codice in commenti:
        if blocchi and not codice:
            prima_riga, precedente, codice_precedente = blocchi[-1]
            righe_precedenti = precedente.count('\n') + 1
            contiguo = riga == prima_riga + righe_precedenti
            stessa_specie = precedente.lstrip().startswith(testo[:2])
            if contiguo and stessa_specie and not codice_precedente:
                blocchi[-1] = (prima_riga, precedente + '\n' + testo, '')
                continue
        blocchi.append((riga, testo, codice))
    return blocchi


def prosa(nome, contenuto):
    if nome.endswith('.rs'):
        commenti = commenti_rust(contenuto)
    elif nome.endswith('.py'):
        commenti = commenti_python(contenuto)
    else:
        commenti = commenti_a_cancelletto(contenuto)
    return raggruppa(con_codice(commenti, contenuto))


# ---------------------------------------------------------------------------
# Le forme, e le sole ammissioni
# ---------------------------------------------------------------------------

DATA = re.compile(r'\b\d{4}-\d{2}-\d{2}\b')

#: Un artefatto immutabile e' citato PER NOME: `run 31166125840`,
#: `commit 4b9edda`, `tag v1.0.3`, `revisione 3598259`. Il qualificatore e'
#: obbligatorio: senza, un numero qualunque — la cardinalita' di un caso
#: limite, un valore a fondo scala — passerebbe per identificatore e
#: assolverebbe la data che gli sta accanto.
ARTEFATTO = re.compile(
    r'\b(?:run|commit|sha|tag|revisione|artefatto|corpus|esecuzione|baseline)\b'
    r'[\s:=(]*(?:annotat[oi]\s+)?[`"\']*'
    r'(?=[0-9a-z._-]*[0-9])[0-9a-z._-]{5,}', re.IGNORECASE)

#: La data FISSA qualcosa: e' un pin, o l'atto normativo che la porta.
DATA_CHE_FISSA = re.compile(
    r'@\s*\d{4}-\d{2}-\d{2}'
    r'|\bnightly-\d{4}-\d{2}-\d{2}'
    r'|\b(?:emendament[oi]|revisione|rilascio|policy|pin|scadenza|valid[oa] dal)\s+'
    r'(?:del\s+)?\d{4}-\d{2}-\d{2}', re.IGNORECASE)

ORDINALI = (r'primo|secondo|terzo|quarto|quinto|sesto|settimo|ottavo|nono|decimo'
            r'|undicesimo|dodicesimo|tredicesimo|quattordicesimo|ultimo')

#: Il giro come ITERAZIONE, non come `giro di review`: cio' che lo precede
#: lo dice senza ambiguita'.
GIRO_AMMESSO = re.compile(r'\b(?:dal|al|nel|del|dopo il|entro il|a ogni|ogni)\s+$',
                          re.IGNORECASE)

IMPERFETTO = re.compile(
    r"\b\w{3,}(?:ava|eva|iva|avano|evano|ivano)\b"
    r"|\b(?:era|erano|c'era|c'erano)\b", re.IGNORECASE)

#: Sostantivi, aggettivi e congiuntivi che finiscono come un imperfetto.
#: In italiano gli aggettivi in `-tiva`/`-siva` sono la classe piu' grossa
#: (normativa, esaustiva, decisiva) e si escludono per forma; i verbi che
#: condividono quella desinenza si nominano invece uno per uno, sotto.
NON_IMPERFETTO = re.compile(
    r'^(?:'
    # Aggettivi: normativa, esaustiva, decisiva, successiva.
    r'\w*[ts]iva'
    # Sostantivi e aggettivi che finiscono in -iva senza esserlo.
    r'|deriva|arriva|riva|saliva|oliva|diva|priva|schiva|gengiva|tardiva'
    # Presente indicativo di verbi il cui tema finisce in -av/-ev/-iv: la
    # desinenza plurale coincide con quella dell\'imperfetto di un\'altra
    # coniugazione, e solo il lessico le distingue.
    r'|arrivano|derivano|attivano|motivano|coltivano|privano'
    r'|sollevano|rilevano|prelevano|elevano|allevano'
    # Presente, congiuntivo e sostantivi in -ava/-eva/-iva.
    r'|prova|trova|osserva|conserva|riserva|serva|preleva|leva|solleva|rileva'
    r'|ricava|scava|cava|lava|salva|brava|schiava|nuova|piova|attiva'
    r'|beva|scriva|scrivano|riceva|deva|neva|eleva|rinnova|approva'
    r'|sopravviva|sopravvivano|conviva|convivano'
    r')$', re.IGNORECASE)

#: I verbi in `-tiva`/`-siva` che sono davvero imperfetti: la forma non li
#: distingue dagli aggettivi, quindi si dichiarano.
IMPERFETTO_COMUNQUE = frozenset((
    'sentiva', 'sentivano', 'partiva', 'partivano', 'ripartiva', 'ripartivano',
    'avvertiva', 'avvertivano', 'convertiva', 'convertivano', 'invertiva',
    'invertivano', 'divertiva', 'esistiva', 'insistiva', 'resistiva',
    'assistiva', 'consistiva', 'decideva', 'uccideva', 'chiudeva',
))

MARCATORE = re.compile(r'\b(?:TODO|FIXME|HACK|XXX)\b')

#: La parola `review` come EVENTO — «il caso della review», «questa serie di
#: review ha gia' trovato», «trovato in tre giri di review» — dice quando un
#: difetto e' stato visto, non quale proprieta' il codice garantisce. Resta
#: ammesso `in review`, che non e' un evento passato ma il modo in cui un
#: artefatto viene letto: «un diff leggibile in review» vale oggi come domani.
GIRO = re.compile(r'\bfinding\b|\bhotfix\b|\bPR[\s-]*\d+\b|\bquesta PR\b'
                  r'|(?<!\bin )\breview\b'
                  r'|\b(?:%s)\s+giro\b' % ORDINALI, re.IGNORECASE)

#: `Fase 2A` e' una tappa; `fase e' gia' chiusa` no. E il batch: `il primo
#: batch dello stream` e' un fatto di esecuzione, `batch 4 ottimizzazioni`
#: e' una tornata di lavoro. Le forme si distinguono per cio' che segue.
TAPPA = re.compile(r'\b[Ff]ase\s+(?:\d|[A-Z]\b|post-)'
                   r'|\b[Mm]ilestone\b|\bB\d\.\d\b|\bfilone\b|\bcantiere\b'
                   r'|\bbatch\s+\d+\s+(?:di\s+)?ottimizzazion'
                   r'|\b(?:%s)\s+batch\s+(?:di\s+)?ottimizzazion'
                   r'|\brimandat[oa] a\b' % ORDINALI)

STESURA = re.compile(
    r'\bprima (?:versione|stesura)\b|\b(?:versione|stesura) precedente\b'
    r'|\bcommento precedente\b|\bin precedenza\b|\bprima del fix\b'
    r'|\bcome prima\b|\bartefatto originale\b|\bmodulo originario\b'
    r'|\bquesto commento diceva\b|\bmodello precedente\b'
    r'|\bcensimento precedente\b', re.IGNORECASE)


def _senza_puntatori(testo, puntatori):
    """Il testo senza i puntatori che il gate documentale definisce.

    `M3` e' una voce di roadmap governata: il commento che la cita nomina un
    documento, non una tappa di lavoro. Le definizioni si importano da
    `verifica_documentazione.py` invece di essere ricopiate, cosi' i due gate
    non possono scostarsi.
    """
    if not puntatori:
        return testo
    forma = r'\b(?:%s)\b' % '|'.join(re.escape(p) for p in sorted(puntatori))
    return re.sub(forma, ' ', testo)


CITAZIONE = re.compile(r'`[^`]*`|«[^»]*»')


def _senza_citazioni(testo):
    """Il testo senza cio' che e' CITATO invece che detto.

    Fra backtick o virgolette c'e' un frammento nominato: un identificatore,
    un messaggio d'errore, la forma che una regola vieta. `Fase 2A` in un
    elenco di forme proibite non e' una tappa di lavoro, e questo gate
    sarebbe il primo file a fallire sul proprio elenco. Le stesse virgolette
    sono la ragione per cui la regola resta leggibile dove serve.
    """
    return CITAZIONE.sub(' ', testo)


#: Il valore che una data traduce sta sulla RIGA DI CODICE che il commento
#: annota: `assert_eq!(giorni, 18_993);  // 2022-01-01`. Dentro la prosa un
#: numero non e' una relazione, e' un numero.
VALORE_ANNOTATO = re.compile(r'-?\b\d[\d_]*\b')

#: L'ora del giorno, che accompagna una data senza farne una frase.
ORA = re.compile(r'\b\d{2}:\d{2}(?::\d{2}(?:[.,]\d+)?)?\b')


def _solo_una_data(compatto):
    """Il commento e' SOLTANTO una data, al piu' con l'ora e la punteggiatura.

    E' la seconda meta' della deroga sul valore annotato, e senza di essa la
    prima non basta: `const LIMITE: usize = 5;  // Corretto il 2026-07-27.`
    ha un numero sulla riga, ma il commento non lo sta traducendo — sta
    dicendo quando qualcuno ha toccato il file.
    """
    resto = ORA.sub(' ', DATA.sub(' ', compatto))
    return not re.sub(r'[^0-9A-Za-zÀ-ÿ]', '', resto)

MARCATORE_DI_COMMENTO = re.compile(r'(?:^|\s)(?://+[/!]?|#+|"""|\'\'\'|/\*+|\*+/?)')


def _senza_marker(testo):
    """Il testo senza i segni che aprono e chiudono un commento."""
    return MARCATORE_DI_COMMENTO.sub(' ', testo)


#: La data e' un DATO quando la relazione che lo dice le e' ATTACCATA: la
#: data (con l'eventuale ora) e subito dopo l'uguale, la freccia, il «vale» o
#: il dominio temporale che la interpreta. `2026-07-25 = 20659 giorni` e'
#: un'equivalenza; «corretto il 2026-07-27: overflow in ms» ha un `ms` nella
#: stessa frase e non dice niente di quella data — e' cronaca con un termine
#: tecnico accanto.
RELAZIONE = (r'=|->|→|\bvale\b|\bvalgono\b|\bcorrisponde\b'
             r'|\b(?:epoch|epoca|UTC|giorni|ms|millis|micros)\b')

DATO_A_DESTRA = re.compile(
    r'\d{4}-\d{2}-\d{2}'
    r'(?:[T ]\d{2}:\d{2}(?::\d{2}(?:[.,]\d+)?)?)?'
    r'[\s)\]]*(?:%s)' % RELAZIONE, re.IGNORECASE)

DATO_A_SINISTRA = re.compile(r'(?:%s)[\s(\[]*$' % RELAZIONE, re.IGNORECASE)


def difetti_del_commento(testo, puntatori, codice=''):
    """Le forme di diario in un singolo commento: (regola, brano)."""
    trovati = []
    # I marker se ne vanno per primi: un blocco va a capo dove capita, e
    # `emendamento` su una riga con la sua data sulla successiva e' la stessa
    # frase per chi legge.
    # Gli spazi si richiudono DOPO aver tolto marker e citazioni: un blocco
    # che va a capo lascia `in /// review`, e con due spazi in mezzo non e'
    # piu' la locuzione che l'ammissione riconosce.
    compatto = ' '.join(
        _senza_citazioni(_senza_marker(' '.join(testo.split()))).split())

    for colpo in MARCATORE.finditer(compatto):
        trovati.append(('marcatore-anonimo', colpo.group(0)))

    # Una data e' un DATO quando la relazione che lo dice le sta ATTACCATA,
    # oppure quando ANNOTA un valore: la riga di codice porta il numero E il
    # commento e' soltanto quella data — `assert_eq!(giorni, 18_993);
    # // 2022-01-01`. Le due condizioni valgono insieme: da sola, la prima
    # assolve qualunque cronaca scritta in coda a una riga che contiene una
    # cifra. Una data isolata, senza codice accanto, non e' un dato: non dice
    # di che cosa sia il numero, e nove volte su dieci e' la data in cui
    # qualcuno ha toccato il file.
    annota_un_valore = (bool(VALORE_ANNOTATO.search(codice))
                        and _solo_una_data(compatto))
    for colpo in DATA.finditer(compatto):
        if annota_un_valore:
            continue
        # Cio' che ammette una data dev'essere ACCANTO alla data e dev'essere
        # ESPLICITO. Un termine tecnico che capita nella stessa frase non e'
        # una relazione con quella data: «corretto il 2026-07-27: overflow in
        # ms» non dice che quella data valga dei millisecondi.
        stretto = compatto[max(0, colpo.start() - 24):colpo.end() + 4]
        if DATA_CHE_FISSA.search(stretto):
            continue
        vicino = DATA.sub(' ', compatto[max(0, colpo.start() - 32):colpo.end() + 32])
        if ARTEFATTO.search(vicino):
            continue
        if DATO_A_DESTRA.match(compatto[colpo.start():]):
            continue
        if DATO_A_SINISTRA.search(compatto[:colpo.start()]):
            continue
        trovati.append(('data-di-calendario', colpo.group(0)))

    for colpo in GIRO.finditer(compatto):
        if GIRO_AMMESSO.search(compatto[:colpo.start()]):
            continue
        trovati.append(('giro-di-review', colpo.group(0)))

    # I puntatori governati si spengono PRIMA di cercare una tappa: la loro
    # forma non deve poter essere scambiata per una sigla di campagna. Non
    # esiste invece un'ammissione per `milestone`: quella parola resta
    # vietata sempre, e la voce di roadmap si nomina con il suo puntatore.
    for colpo in TAPPA.finditer(_senza_puntatori(compatto, puntatori)):
        trovati.append(('tappa-di-campagna', colpo.group(0)))

    for colpo in STESURA.finditer(compatto):
        trovati.append(('stesura-precedente', colpo.group(0)))

    for colpo in IMPERFETTO.finditer(compatto):
        parola = colpo.group(0).lower()
        if parola not in IMPERFETTO_COMUNQUE and NON_IMPERFETTO.match(parola):
            continue
        trovati.append(('difetto-all-imperfetto', colpo.group(0)))

    return trovati


def controlla(sorgenti, puntatori=frozenset()):
    """Le violazioni di tutte le sorgenti: (file, riga, regola, brano, testo).

    Un sorgente illeggibile e' una violazione a se': `errore-lessicale`. Non
    e' un difetto dei commenti, ma senza di esso il verdetto su quel file
    sarebbe «pulito» per non aver letto niente.
    """
    guasti = []
    for nome in sorted(sorgenti):
        try:
            commenti = prosa(nome, sorgenti[nome])
        except ErroreLessicale as guasto:
            guasti.append((nome, guasto.riga, 'errore-lessicale', guasto.motivo,
                           'il file non e\' stato letto fino in fondo: '
                           'il verdetto sui suoi commenti non vale'))
            continue
        for riga, testo, codice in commenti:
            for regola, brano in difetti_del_commento(testo, puntatori, codice):
                compatto = ' '.join(testo.split())
                guasti.append((nome, riga, regola, brano, compatto[:120]))
    return guasti


INVOCAZIONE = re.compile(r'^main\(\)\s*$', re.MULTILINE)


def puntatori_governati():
    """Gli identificatori che `verifica_documentazione.py` definisce.

    Presi da LI', non ricopiati: due elenchi della stessa cosa si scostano, e
    quello che si scosta e' sempre il duplicato.

    Quel gate invoca `main()` a livello di modulo — e' uno script, non una
    libreria — quindi qui se ne esegue il sorgente SENZA quella riga: serve
    la sua tabella delle definizioni, non il suo verdetto.

    Le due condizioni su cui poggia questa lettura si verificano invece di
    darle per buone: che l'invocazione sia stata davvero tolta — altrimenti
    il gate documentale girerebbe qui dentro, e il suo verdetto diventerebbe
    il nostro — e che la tabella esista e non sia vuota. Un insieme vuoto
    spegnerebbe in silenzio l'ammissione dei puntatori governati.
    """
    percorso = os.path.join(RADICE, 'scripts', 'verifica_documentazione.py')
    sorgente = INVOCAZIONE.sub('', leggi(percorso))
    if INVOCAZIONE.search(sorgente):
        raise SystemExit('%s invoca ancora main(): non si puo\' leggerne le '
                         'definizioni senza eseguirlo' % percorso)
    spazio = {'__name__': 'verifica_documentazione', '__file__': percorso}
    corrente = os.getcwd()
    try:
        os.chdir(RADICE)
        exec(compile(sorgente, percorso, 'exec'), spazio)  # noqa: S102
        definizioni = spazio['_definizioni']([])
    finally:
        os.chdir(corrente)
    if not definizioni:
        raise SystemExit('nessun puntatore governato: il gate documentale non '
                         'definisce piu' + ' nulla, o la sua API e\' cambiata')
    return definizioni


# ---------------------------------------------------------------------------
# Le prove, a ogni invocazione
# ---------------------------------------------------------------------------
#
# Le sorgenti sono SINTETICHE. Mutare i file veri renderebbe la prova
# dipendente dallo stato che deve giudicare: col difetto gia' presente la
# mutazione non si applica, e il gate abortisce invece di segnalare.

PULITO = {
    'esempio.rs': '\n'.join((
        '//! Un modulo che dichiara, invece di raccontare.',
        '//!',
        '//! Il tetto vale per input: superarlo e\' un errore di risorsa.',
        'const MESSAGGIO: &str = "TODO: una stringa non e\' un commento";',
        'const DATA: &str = "2026-08-06 in un messaggio d\'errore";',
        'const GREZZA: &str = r#"// nemmeno questa e\' un commento"#;',
        'const APICE: char = \'"\';  // la quote nel char non apre nulla',
        'fn f<\'a>(x: &\'a str) -> &\'a str { x }  // la lifetime nemmeno',
        '/* commento di blocco /* annidato */ ancora dentro */',
        '// Dal secondo giro in poi lo schema e\' stabile.',
        '// Pin dell\'action: @ 2026-08-23, alzato a mano.',
        '// Il piano lo dichiara: piano-v5.md, emendamento 2026-08-17.',
        '// Verdetto dell\'ultima esecuzione riuscita (run 31166125840,',
        '// 2026-08-07): la revisione dichiarata coincide.',
        '// La forma normativa resta valida.',
        '// Lo snapshot si committa con la modifica: il diff resta leggibile',
        '// in review.',
        'assert_eq!(giorni, 18_993);  // 2022-01-01',
    )),
    'esempio.py': '\n'.join((
        '# -*- coding: utf-8 -*-',
        '"""Docstring: dice che cosa sorveglia, non quando e\' nato."""',
        'MESSAGGIO = "finding 4: una stringa normale non e\' prosa"',
        'def f():',
        '    """Che cosa rende: il conto, mai negativo."""',
        '    return 1  # niente da dichiarare',
    )),
    'esempio.yml': '\n'.join((
        'run: echo "# non e\' un commento"',
        'chiave: ${VAR#prefisso}',
        '# Toolchain pinnata: nightly-2026-08-01.',
    )),
}


#: Un sorgente in cui la riga del commento e' facile da sbagliare: una
#: stringa continuata con `\`, una grezza multiriga, un blocco annidato.
#: Il numero di riga e' l'unica cosa che chi legge il verdetto usa per
#: arrivare al commento: se e' sbagliato il gate manda nel posto sbagliato.
RIGHE_DIFFICILI = '\n'.join((
    'const A: &str = "prima riga \\',                      # 1
    '                 seconda riga";',                     # 2
    'const B: &str = r#"',                                 # 3
    '// non e\' un commento',                              # 4
    '"#;',                                                 # 5
    '/* blocco /* annidato */ ancora */',                  # 6
    '// primo commento vero',                              # 7
    'const C: char = \'\\n\';  // dopo un carattere di escape',   # 8
))


def prova_delle_righe():
    """Il numero di riga dev'essere quello vero, non uno vicino."""
    atteso = [(6, '/* blocco'), (7, '// primo commento vero'),
              (8, '// dopo un carattere di escape')]
    trovati = [(riga, testo.split('\n')[0])
               for riga, testo, _ in prosa('difficile.rs', RIGHE_DIFFICILI)]
    fuori = len(trovati) != len(atteso) or any(
        riga != atteso[i][0] or not testo.startswith(atteso[i][1])
        for i, (riga, testo) in enumerate(trovati))
    if fuori:
        raise SystemExit('le righe dei commenti non tornano: %r invece di %r'
                         % (trovati, atteso))
    return len(atteso)


#: (nome, file, iniezione, CLASSE ATTESA).
#:
#: La classe attesa non e' un ornamento. Senza, una mutazione puo' passare
#: perche' e' inciampata in un'altra regola — «la prima stesura diceva» cade
#: sull'imperfetto anche a regola `stesura-precedente` rotta — e la prova
#: dichiara sorvegliato cio' che non lo e' piu'.
MUTAZIONI = (
    ('marcatore anonimo', 'esempio.rs', '// TODO: rivedere',
     'marcatore-anonimo'),
    ('data nuda', 'esempio.rs',
     '// Fino al 2026-07-27 il gate non esiste.', 'data-di-calendario'),
    # Cio' che assolve una data dev'essere accanto a lei ed esplicito: un
    # numero grande nella frase non e' una relazione, e senza qualificatore
    # non e' nemmeno un artefatto.
    ('data con un numero grande accanto', 'esempio.rs',
     '// La classe chiusa qui: 9007199254740992 collassa col successivo,\n'
     '// e la cosa si vede dal 2026-07-27.', 'data-di-calendario'),
    ('data con un identificatore senza qualificatore', 'esempio.rs',
     '// Chiuso in 4b9edda9 il 2026-08-07, e da allora il ramo tiene.',
     'data-di-calendario'),
    ('giro di review', 'esempio.rs',
     '// Nono giro: la stessa proprieta\', un altro nome.', 'giro-di-review'),
    ('review come evento', 'esempio.rs',
     '// Il caso della review: due chiavi canoniche co-presenti.',
     'giro-di-review'),
    ('data isolata', 'esempio.rs', '// 2026-07-27', 'data-di-calendario'),
    ('data con un termine tecnico nella stessa frase', 'esempio.rs',
     '// Corretto il 2026-07-27: overflow in ms.', 'data-di-calendario'),
    ('data con un uguale altrove nella frase', 'esempio.rs',
     '// Corretto il 2026-07-27: limite = 5.', 'data-di-calendario'),
    # Il numero sulla riga di codice non basta: il commento deve essere QUELLA
    # data, non una frase che la contiene.
    ('data in una frase, in coda a una riga con un numero', 'esempio.rs',
     'const LIMITE: usize = 5;  // Corretto il 2026-07-27.',
     'data-di-calendario'),
    ('tappa di campagna', 'esempio.rs', '// Fase 2A: wiring del catalogo.',
     'tappa-di-campagna'),
    ('milestone da sola', 'esempio.rs', '// Milestone D, chiusa.',
     'tappa-di-campagna'),
    # `milestone` non ha ammissioni: un puntatore governato nello stesso
    # blocco non la assolve, o basterebbe nominarne uno per dire qualunque
    # cosa.
    ('milestone accanto a un puntatore governato', 'esempio.rs',
     '// M3 governa lo scheduler. Milestone B conclusa.', 'tappa-di-campagna'),
    ('stesura precedente', 'esempio.rs',
     '// La prima stesura di questo oracolo usa la forma sbagliata.',
     'stesura-precedente'),
    ('imperfetto', 'esempio.rs',
     '// Il controllo lasciava passare gli helper.', 'difetto-all-imperfetto'),
    ('imperfetto in -iva', 'esempio.rs', '// Il tipo non veniva riconosciuto.',
     'difetto-all-imperfetto'),
    ('cronaca in docstring', 'esempio.py', '"""Prima versione della sonda."""',
     'stesura-precedente'),
    ('cronaca in commento yaml', 'esempio.yml',
     '# Esteso il 2026-08-06 alla CLI.', 'data-di-calendario'),
    # I lessici devono fallire CHIUSI: un file che non si lascia leggere fino
    # in fondo non e' un file pulito, e il commento vietato che segue non lo
    # vedrebbe nessuno.
    ('stringa Rust mai chiusa', 'esempio.rs',
     'const R: &str = "aperta e mai chiusa;\n// TODO: nascosto dalla stringa',
     'errore-lessicale'),
    ('docstring Python mai chiusa', 'esempio.py',
     '"""aperta e mai chiusa\n# TODO: nascosto dalla docstring',
     'errore-lessicale'),
)


def prova_di_mutazione(puntatori):
    """Ogni regola su un difetto iniettato, con la CLASSE che deve produrre.

    Rende (mutazioni viste, ammissioni positive verificate): sono due conti
    diversi e vanno detti come tali.
    """
    if controlla(PULITO, puntatori):
        raise SystemExit('le sorgenti sintetiche di controllo non sono pulite:\n%s'
                         % '\n'.join(repr(g) for g in controlla(PULITO, puntatori)))
    # La classe attesa e' l'INSIEME, non un elemento fra tanti: una mutazione
    # che scatena anche altro sta provando altro, e la regola che dichiara di
    # sorvegliare potrebbe essere rotta senza che nessuno se ne accorga.
    for nome, dove, iniezione, atteso in MUTAZIONI:
        mutate = dict(PULITO)
        mutate[dove] = mutate[dove] + '\n' + iniezione
        viste = {regola for _, _, regola, _, _ in controlla(mutate, puntatori)}
        if viste != {atteso}:
            raise SystemExit(
                'mutazione %s: attesa la sola classe %s, viste %s — la regola '
                'non sorveglia piu\' cio\' che dichiara'
                % (nome, atteso, sorted(viste) or 'nessuna'))

    # Le ammissioni sono la meta' fragile: se si allargano, il gate tace.
    # Ognuna si prova nei DUE versi — cio' che deve passare passa, e cio' che
    # le sta accanto no.
    ammissioni = 0

    # I puntatori governati si spengono prima delle regole, cosi' la loro
    # forma non puo' essere scambiata per una sigla di campagna. Nessuna
    # famiglia governata ha oggi la forma `B1.3`; la prova usa un insieme
    # SINTETICO che ce l'ha, perche' il meccanismo va verificato adesso e non
    # il giorno in cui servira'.
    finto = frozenset({'B1.3'})
    if difetti_del_commento('// B1.3 vale ancora.', finto):
        raise SystemExit('un puntatore governato viene letto come tappa')
    if not difetti_del_commento('// B1.3 vale ancora.', frozenset()):
        raise SystemExit('senza puntatori governati la forma `B1.3` passa: '
                         'l\'esclusione non e\' quella che si crede')
    ammissioni += 1

    # `in review` e' il modo in cui si legge un artefatto, non una tornata di
    # lavoro: sta gia' in PULITO, e toglierlo dall'ammissione lo farebbe
    # cadere. Qui si verifica che la distinzione regga su una frase sola.
    if difetti_del_commento('// Un diff leggibile in review.', puntatori):
        raise SystemExit('`in review` non e\' piu\' ammesso: l\'ammissione e\' rotta')
    if not difetti_del_commento('// Il diff della review.', puntatori):
        raise SystemExit('`review` come evento non e\' piu\' vista')
    ammissioni += 1

    # La data-dato: passa con la relazione attaccata e con il valore sulla
    # riga di codice, non per il solo fatto di essere isolata.
    if difetti_del_commento('// 2026-07-25 = 20659 giorni dall\'epoch.', puntatori):
        raise SystemExit('la data con la relazione attaccata non passa piu\'')
    if difetti_del_commento('// 1970-01-01', puntatori, 'assert_eq!(v, 0);'):
        raise SystemExit('la data che annota un valore sulla riga non passa piu\'')
    ammissioni += 1

    return len(MUTAZIONI), ammissioni


def main():
    puntatori = puntatori_governati()
    mutazioni, ammissioni = prova_di_mutazione(puntatori)
    righe = prova_delle_righe()
    sorgenti = sorgenti_del_repository()
    guasti = controlla(sorgenti, puntatori)
    if guasti:
        print('COMMENTI CHE RACCONTANO INVECE DI DICHIARARE: %d\n' % len(guasti),
              file=sys.stderr)
        for nome, riga, regola, brano, testo in guasti:
            print('%s:%d [%s: %s]\n    %s' % (nome, riga, regola, brano, testo),
                  file=sys.stderr)
        print('\nLa spiegazione tecnica va tenuta, al controfattuale presente:\n'
              '  «cercare solo il nome LETTERALE lascerebbe passare gli helper».',
              file=sys.stderr)
        return 1
    commenti = sum(len(prosa(nome, testo)) for nome, testo in sorgenti.items())
    print('commenti al presente: %d commenti in %d sorgenti, %d puntatori '
          'governati importati dal gate documentale, %d mutazioni viste con la '
          'classe attesa, %d ammissioni verificate nei due versi, %d righe di '
          'commento controllate una per una'
          % (commenti, len(sorgenti), len(puntatori), mutazioni, ammissioni, righe))
    return 0


if __name__ == '__main__':
    sys.exit(main())
