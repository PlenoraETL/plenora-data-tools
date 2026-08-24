# -*- coding: utf-8 -*-
"""Gate: nessuna macro di assert nel codice di produzione.

`docs/errori-e-limiti.md` dichiara che nel codice di produzione non esistono
primitive di panico. Il gate Clippy R6 nega `panic!`, `unwrap`, `expect`,
`unreachable!`, `todo!` e `unimplemented!` — ma non `assert!`,
`assert_eq!`, `assert_ne!` ne' le loro varianti `debug_assert*`, che sono
primitive di panico a tutti gli effetti: quelle `debug_*` non compaiono in
release e quindi sembrano innocue, ma fanno abortire ogni consumatore che
compili in debug o esegua i propri test contro questa libreria. Un panico e'
inoltre la peggiore uscita possibile dai punti in cui la libreria tiene un
lock: lascia il mutex avvelenato senza che nessuno abbia dichiarato corrotto
lo stato.

Il perimetro e' lo stesso del gate R6: le `lib` di tutti i crate e il binario
della CLI, cioe' tutto cio' che sta sotto `crates/*/src`, meno il codice di
test. Nei test le macro di assert restano legittime: sono la loro ragione
d'essere.

**La scansione non e' per riga.** Una prima versione applicava la regex a una
riga per volta e riconosceva i blocchi di test contando le graffe sul testo
grezzo: aveva due falsi negativi immediati, entrambi sintassi Rust valida —
`assert` e `!` separati da un a capo, e un `#[cfg(test)]` dentro un
**commento**, che faceva saltare tutto il blocco successivo di codice di
produzione. Qui il sorgente viene prima ripulito (commenti, stringhe, stringhe
grezze e letterali di carattere sostituiti da spazi, così gli offset e i
numeri di riga restano quelli veri), poi analizzato per intero: le graffe
dentro una stringa non contano piu', e le macro si riconoscono anche se
spezzate su piu' righe.

Il riconoscimento del codice di test e' **stretto di proposito**: solo la
forma esatta `#[cfg(test)]`. Una forma non riconosciuta produce al massimo un
falso positivo — rumoroso e visibile — mentre riconoscerne una di troppo
lascerebbe passare codice di produzione, che e' il verso sbagliato.

Come gli altri gate del progetto, si autoverifica: inietta mutazioni
sintetiche e pretende di vederle, e forme legittime e pretende di lasciarle
passare.

    python scripts/verifica_assenza_assert.py
"""
import io
import os
import re
import sys

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# I nomi vietati fuori dai test. `debug_assert*` sono inclusi: la loro
# assenza in release non li rende innocui, li rende invisibili.
#
# Si cerca il NOME, non l'invocazione. Pretendere il `!` subito dopo lasciava
# passare tutto cio' che nomina la macro senza chiamarla sul posto: un import
# raggruppato (`use std::{assert as check};`), un alias raw
# (`as r#check`), o il nome passato a un'altra macro che poi lo espande
# (`call!(assert)`). Il nome vietato non ha ragione di comparire nel codice di
# produzione in nessuna forma, e un falso positivo qui e' rumore visibile —
# il verso giusto per un gate che protegge una garanzia.
#
# `r#` e' ammesso davanti al nome proprio per intercettare gli alias raw.
MACRO = re.compile(
    r'(?<![A-Za-z0-9_])(?:r#)?'
    r'(debug_assert_eq|debug_assert_ne|debug_assert|assert_eq|assert_ne|assert)'
    r'(?![A-Za-z0-9_])')

# Attributo di test, nella sola forma esatta (spaziatura libera).
CFG_TEST = re.compile(r'#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]')

# Caratteri che possono precedere un attributo in posizione di elemento.
# Servono a distinguere un vero `#[cfg(test)]` da uno che compare DENTRO
# qualcos'altro — per esempio `#[doc = stringify!(#[cfg(test)])]`, dove il
# testo dell'attributo e' un argomento di macro e non governa nulla.
PRIMA_DI_ATTRIBUTO = ';{}]'

# `mod nome;` — dichiarazione di modulo su file, senza blocco.
#
# La regione fra l'attributo e il `;` deve contenere SOLO questo, niente
# altro: cercare la sottosequenza ovunque faceva scambiare per dichiarazione
# di modulo anche `register!(mod vittima;)`, e un file di produzione finiva
# nell'insieme escluso senza essere mai esaminato.
MOD_SU_FILE = re.compile(r'^\s*(?:pub\s+(?:\([^)]*\)\s*)?)?mod\s+([A-Za-z0-9_]+)\s*$')


def ripulisci(sorgente):
    """Sostituisce commenti e letterali con spazi, preservando gli offset.

    Il risultato ha la stessa lunghezza dell'originale e le stesse posizioni
    di riga: si puo' cercare qui e riportare le coordinate di la'. Copre i
    commenti di riga, quelli a blocco (annidati, come li ammette Rust), le
    stringhe con escape, le stringhe grezze `r"..."` / `r#"..."#` e i
    letterali di carattere.
    """
    fuori = list(sorgente)
    i = 0
    n = len(sorgente)

    def spegni(inizio, fine):
        for k in range(inizio, min(fine, n)):
            if fuori[k] != '\n':
                fuori[k] = ' '

    while i < n:
        c = sorgente[i]
        # Commento di riga.
        if sorgente.startswith('//', i):
            fine = sorgente.find('\n', i)
            fine = n if fine < 0 else fine
            spegni(i, fine)
            i = fine
            continue
        # Commento a blocco, annidabile.
        if sorgente.startswith('/*', i):
            profondita = 1
            j = i + 2
            while j < n and profondita > 0:
                if sorgente.startswith('/*', j):
                    profondita += 1
                    j += 2
                elif sorgente.startswith('*/', j):
                    profondita -= 1
                    j += 2
                else:
                    j += 1
            spegni(i, j)
            i = j
            continue
        # Stringa GREZZA: `r"..."`, `r#"..."#`, `br"..."`, `br#"..."#`. Il
        # prefisso deve contenere una `r`: senza, `b"..."` e' una byte
        # string NORMALE, con gli escape. Trattarla come grezza faceva
        # chiudere il letterale sulla quote escapata di `b"\""` e lasciava
        # una quote orfana, che apriva una stringa fantasma e spegneva tutto
        # il codice fino alla quote successiva del file — un assert di
        # produzione poteva finirci dentro e sparire.
        if c in 'rb' and (
                c == 'r' or (i + 1 < n and sorgente[i + 1] == 'r')):
            j = i
            while j < n and sorgente[j] in 'rb':
                j += 1
            cancelletti = 0
            k = j
            while k < n and sorgente[k] == '#':
                cancelletti += 1
                k += 1
            if k < n and sorgente[k] == '"':
                chiusura = '"' + '#' * cancelletti
                fine = sorgente.find(chiusura, k + 1)
                fine = n if fine < 0 else fine + len(chiusura)
                spegni(i, fine)
                i = fine
                continue
        # Stringa normale.
        if c == '"':
            j = i + 1
            while j < n:
                if sorgente[j] == '\\':
                    j += 2
                    continue
                if sorgente[j] == '"':
                    j += 1
                    break
                j += 1
            spegni(i, j)
            i = j
            continue
        # Letterale di carattere: distinguerlo da una lifetime (`'a`).
        if c == "'":
            j = i + 1
            if j < n and sorgente[j] == '\\':
                j += 2
                while j < n and sorgente[j] != "'":
                    j += 1
                j = min(j + 1, n)
                spegni(i, j)
                i = j
                continue
            if j + 1 < n and sorgente[j + 1] == "'":
                spegni(i, j + 2)
                i = j + 2
                continue
        i += 1
    return ''.join(fuori)


# Invocazione di macro (`nome!(...)`) oppure DEFINIZIONE
# (`macro_rules! nome { ... }`). La definizione ha un identificatore fra il
# `!` e il delimitatore, quindi il pattern dell'invocazione non la vedeva: e
# il corpo di una definizione e' token puri, dove un `#[cfg(test)]` non
# governa nulla. `macro_rules! dormant { () => { ; #[cfg(test)] ignored } }`
# faceva prendere per blocco di test la funzione che seguiva.
# `r#` e' ammesso sia sul nome della macro sia su quello che segue
# `macro_rules!`: `macro_rules! r#dormant` e' Rust valido, e senza il prefisso
# raw il corpo non veniva riconosciuto come token di macro.
INVOCAZIONE_MACRO = re.compile(
    r'(?<![A-Za-z0-9_])(?:r#)?[A-Za-z_][A-Za-z0-9_]*\s*!\s*'
    r'(?:(?:r#)?[A-Za-z_][A-Za-z0-9_]*\s*)?([({\[])')

# Attributo `#[path = "..."]`: dirotta il modulo su un file che NON si chiama
# come il modulo. Il valore non e' leggibile qui — la ripulitura ha spento il
# contenuto delle stringhe — quindi in sua presenza non si esclude nulla:
# dedurre il file dal nome del modulo escludeva il file sbagliato, e quello
# giusto restava fuori dalla scansione.
ATTRIBUTO_PATH = re.compile(r'#\s*\[\s*path\b')

CHIUSURA = {'(': ')', '{': '}', '[': ']'}


def intervalli_macro(pulito):
    """Intervalli [inizio, fine) dei corpi delle invocazioni di macro.

    Dentro una macro ci sono **token**, non elementi: un `#[cfg(test)]` che
    compare li' non governa nulla, e cercare in avanti la prima graffa lo
    faceva scavalcare la chiusura della macro e prendere per corpo di test
    l'elemento che seguiva. `discard! { ; #[cfg(test)] ignored } fn f() {
    assert!(true); }` e' Rust valido e nascondeva l'assert.
    """
    intervalli = []
    for invocazione in INVOCAZIONE_MACRO.finditer(pulito):
        apertura = invocazione.group(1)
        chiusura = CHIUSURA[apertura]
        inizio = invocazione.end() - 1
        profondita = 0
        k = inizio
        while k < len(pulito):
            if pulito[k] == apertura:
                profondita += 1
            elif pulito[k] == chiusura:
                profondita -= 1
                if profondita == 0:
                    break
            k += 1
        intervalli.append((inizio, min(k + 1, len(pulito))))
    return intervalli


def dentro(intervalli, posizione):
    return any(a <= posizione < b for a, b in intervalli)


# `include!` o `#[path]` il cui argomento NON e' un letterale: il file
# incluso si conosce solo espandendo le macro, cosa che questo gate non fa.
# Lo spazio va DENTRO il lookahead, non prima: con `\s*(?![r"])` il motore
# torna indietro fino a consumare zero spazi e trova soddisfatto il
# lookahead sullo spazio stesso, marcando come opaco anche
# `#[path = r"file.rs"]`, che e' un letterale risolvibile. Se ne e' accorta
# l'autoverifica, che pretende di NON dichiarare opaco cio' che e' leggibile.
INCLUSIONE_NON_LETTERALE = re.compile(
    r'(?:#\s*\[\s*path\s*=|\binclude\s*!\s*\(|\binclude_str\s*!\s*\(|'
    r'\binclude_bytes\s*!\s*\()(?!\s*[r"])')


# Un letterale di percorso che contiene un ESCAPE: `include!("sha\u{72}ed.rs")`
# risolve a `shared.rs` per Rust, ma il testo fra virgolette non lo dice. Non
# si decodifica la semantica degli escape di Rust — si dichiara il sorgente
# non analizzabile.
PATH_CON_ESCAPE = re.compile(
    r'(?:#\s*\[\s*path\s*=|\binclude\s*!\s*\(|\binclude_str\s*!\s*\(|'
    r'\binclude_bytes\s*!\s*\()\s*"[^"]*\\')


def inclusioni_non_risolvibili(sorgente):
    """`True` se il sorgente include un file per via non letterale.

    Copre due forme: l'argomento costruito da macro
    (`include!(concat!(...))`) e il letterale con escape, il cui testo grezzo
    non e' il nome del file.
    """
    return (INCLUSIONE_NON_LETTERALE.search(sorgente) is not None
            or PATH_CON_ESCAPE.search(sorgente) is not None)


def file_di_produzione(sorgente, pulito, percorso):
    """File che questo sorgente include in PRODUZIONE, non sotto cfg(test).

    Serve a non escludere mai un file che, oltre a essere incluso come
    modulo di test da qualche parte, e' incluso anche come modulo di
    produzione altrove. L'esclusione era per percorso e globale: bastava un
    `#[cfg(test)] mod condiviso;` in un file perche' `condiviso.rs` sparisse
    dalla scansione, anche quando un altro lo includeva davvero in
    produzione con `#[path = "condiviso.rs"] pub mod produzione;`.

    Il valore di `#[path]` si legge dal sorgente ORIGINALE: nel testo
    ripulito il contenuto delle stringhe e' spento.
    """
    cartella = os.path.dirname(percorso)
    radice_modulo = os.path.join(cartella, os.path.basename(percorso)[:-3])
    test = blocchi_test(pulito)
    # Le dichiarazioni su file governate da `cfg(test)` non aprono un blocco,
    # quindi `blocchi_test` non le copre: vanno escluse a parte, altrimenti
    # ogni `#[cfg(test)] mod tests;` verrebbe contato come produzione e
    # l'esclusione non funzionerebbe piu' per nessuno.
    fini_di_test = {fine for _, fine in dichiarazioni_di_test(pulito)}
    inclusi = set()

    def registra(nome_file):
        # `os.path.normpath` prima del confronto: `./condiviso.rs` e
        # `sotto/../condiviso.rs` indicano lo stesso file di
        # `condiviso.rs`, e confrontarli come testo lasciava il file
        # nell'insieme escluso senza mai reinserirlo fra quelli di
        # produzione.
        inclusi.add(os.path.normpath(os.path.join(cartella, nome_file)))
        inclusi.add(os.path.normpath(os.path.join(radice_modulo, nome_file)))

    # Dichiarazioni su file, fuori dai blocchi e non governate da cfg(test).
    # `r#` e' ammesso: `mod r#shared;` e' un identificatore raw, e il file si
    # chiama comunque `shared.rs`.
    for dichiarazione in re.finditer(r'\bmod\s+(?:r#)?([A-Za-z0-9_]+)\s*;', pulito):
        if dentro(test, dichiarazione.start()):
            continue
        if dichiarazione.end() in fini_di_test:
            continue
        registra(dichiarazione.group(1) + '.rs')
    # Riferimenti a un file per PERCORSO: `#[path = "..."]` in tutte le forme
    # di stringa che Rust ammette, e `include!("...")`, che incorpora il file
    # senza dichiarare alcun modulo. Si prendono da qualunque occorrenza,
    # senza distinguere test da produzione: sotto-escludere costa un falso
    # positivo, sovra-escludere costa un assert di produzione mai guardato.
    riferimenti = re.finditer(
        r'(?:#\s*\[\s*path\s*=|\binclude\s*!\s*\(|\binclude_str\s*!\s*\(|'
        r'\binclude_bytes\s*!\s*\()\s*'
        r'(?:r(#*)"(?P<grezza>.*?)"\1|"(?P<normale>[^"]*)")',
        sorgente,
        re.DOTALL,
    )
    for riferimento in riferimenti:
        indicato = riferimento.group('grezza')
        if indicato is None:
            indicato = riferimento.group('normale')
        registra(indicato.replace('/', os.sep))
    return inclusi


def in_posizione_di_attributo(pulito, inizio):
    """`True` se l'attributo che inizia a `inizio` governa un elemento.

    Un attributo vero segue la fine dell'elemento precedente (`;`, `}`), una
    parentesi graffa aperta o un altro attributo (`]`), oppure apre il file.
    Se lo precede qualcos'altro — una parentesi tonda, un `=`, una virgola —
    quel testo e' dentro un'altra costruzione (tipicamente l'argomento di una
    macro) e non ha alcun effetto su cio' che segue.
    """
    k = inizio - 1
    while k >= 0 and pulito[k].isspace():
        k -= 1
    return k < 0 or pulito[k] in PRIMA_DI_ATTRIBUTO


def blocchi_test(pulito):
    """Intervalli [inizio, fine) di offset governati da `#[cfg(test)]`.

    Dall'attributo si cerca in avanti il primo `{` o `;`: una graffa apre un
    elemento con corpo (modulo, funzione) e l'intervallo arriva alla graffa
    che la chiude; un punto e virgola e' una dichiarazione su file
    (`mod nome;`) e non contiene codice.
    """
    intervalli = []
    macro = intervalli_macro(pulito)
    for attributo in CFG_TEST.finditer(pulito):
        if dentro(macro, attributo.start()):
            continue
        if not in_posizione_di_attributo(pulito, attributo.start()):
            continue
        j = attributo.end()
        while j < len(pulito) and pulito[j] not in '{;':
            j += 1
        if j >= len(pulito) or pulito[j] == ';':
            continue
        profondita = 0
        k = j
        while k < len(pulito):
            if pulito[k] == '{':
                profondita += 1
            elif pulito[k] == '}':
                profondita -= 1
                if profondita == 0:
                    break
            k += 1
        intervalli.append((attributo.start(), min(k + 1, len(pulito))))
    return intervalli


def dichiarazioni_di_test(pulito):
    """(nome, fine) per ogni `#[cfg(test)] ... mod nome;` di questo sorgente.

    `fine` e' l'offset subito dopo il `;`: serve a distinguere queste
    dichiarazioni da quelle di produzione, che hanno la stessa forma
    testuale ma non sono governate dall'attributo.
    """
    trovate = []
    macro = intervalli_macro(pulito)
    for attributo in CFG_TEST.finditer(pulito):
        if dentro(macro, attributo.start()):
            continue
        if not in_posizione_di_attributo(pulito, attributo.start()):
            continue
        j = attributo.end()
        while j < len(pulito) and pulito[j] not in '{;':
            j += 1
        if j >= len(pulito) or pulito[j] != ';':
            continue
        # La regione fra l'attributo e il `;` deve essere ESATTAMENTE una
        # dichiarazione di modulo, eventuali altri attributi a parte. Qui
        # `fullmatch` e' la differenza fra riconoscere `mod tests;` e farsi
        # ingannare da `register!(mod vittima;)`.
        regione = pulito[attributo.end():j]
        # Con `#[path]` il file non si chiama come il modulo: non si esclude
        # nulla, e il file resta da esaminare.
        if ATTRIBUTO_PATH.search(regione):
            continue
        # Attributi intermedi (`#[allow(...)]`) sono legittimi e vanno tolti
        # prima del confronto.
        regione_senza_attributi = re.sub(r'#\s*\[[^\]]*\]', ' ', regione)
        dichiarazione = MOD_SU_FILE.fullmatch(regione_senza_attributi)
        if dichiarazione is None:
            continue
        trovate.append((dichiarazione.group(1), j + 1))
    return trovate


def file_di_test(pulito, percorso):
    """File inclusi da `#[cfg(test)] ... mod nome;` in questo sorgente."""
    esclusi = set()
    cartella = os.path.dirname(percorso)
    radice_modulo = os.path.join(cartella, os.path.basename(percorso)[:-3])
    for nome, _fine in dichiarazioni_di_test(pulito):
        esclusi.add(os.path.normpath(os.path.join(cartella, nome + '.rs')))
        esclusi.add(os.path.normpath(os.path.join(radice_modulo, nome + '.rs')))
    return esclusi


def occorrenze(sorgente):
    """(riga, testo) per ogni macro di assert fuori dai blocchi di test."""
    pulito = ripulisci(sorgente)
    intervalli = blocchi_test(pulito)
    righe = sorgente.split('\n')
    trovate = []

    def registra(posizione):
        if any(a <= posizione < b for a, b in intervalli):
            return
        numero = pulito.count('\n', 0, posizione)
        trovate.append((numero + 1, righe[numero].strip()))

    for macro in MACRO.finditer(pulito):
        registra(macro.start())
    return trovate


def sorgenti():
    """Sorgenti Rust del perimetro di produzione.

    Sono i file sotto `crates/*/src`, **piu' la chiusura** dei file che quelli
    includono per percorso letterale: un `include!("../generato.rs")` o un
    `#[path = "../altrove.rs"]` compila codice che sta fuori da `src`, e
    fermarsi all'albero delle directory lo lasciava fuori dalla scansione pur
    essendo compilato nella libreria.
    """
    trovati = []
    crates = os.path.join(RADICE, 'crates')
    for crate in sorted(os.listdir(crates)):
        base = os.path.join(crates, crate, 'src')
        if not os.path.isdir(base):
            continue
        for cartella, _, file in os.walk(base):
            for nome in sorted(file):
                if nome.endswith('.rs'):
                    trovati.append(os.path.normpath(os.path.join(cartella, nome)))
    # Chiusura transitiva sui riferimenti letterali: un file appena aggiunto
    # puo' a sua volta includerne un altro.
    da_esaminare = list(trovati)
    visti = set(trovati)
    while da_esaminare:
        percorso = da_esaminare.pop()
        try:
            sorgente = testo(percorso)
        except OSError:
            continue
        pulito = ripulisci(sorgente)
        for candidato in file_di_produzione(sorgente, pulito, percorso):
            if (candidato not in visti and candidato.endswith('.rs')
                    and os.path.isfile(candidato)):
                visti.add(candidato)
                trovati.append(candidato)
                da_esaminare.append(candidato)
    return sorted(visti)


def testo(percorso):
    return io.open(percorso, encoding='utf-8', newline='').read()


def autoverifica_esclusione_file():
    """`file_di_test` deve escludere i moduli di test e SOLO quelli.

    Escludere un file di produzione e' il falso negativo peggiore possibile:
    quel file non viene mai esaminato, e il gate resta verde qualunque cosa
    contenga.
    """
    finto = os.path.join(RADICE, 'crates', 'x', 'src', 'lib.rs')
    atteso = os.path.join(RADICE, 'crates', 'x', 'src', 'tests.rs')

    def esclusi(sorgente):
        return file_di_test(ripulisci(sorgente), finto)

    # Dichiarazione vera, anche con attributi e commenti in mezzo.
    if atteso not in esclusi('#[cfg(test)]\nmod tests;\n'):
        raise SystemExit('autoverifica fallita: modulo di test non escluso')
    if atteso not in esclusi(
            '#[cfg(test)]\n// commento\n#[allow(deprecated)]\nmod tests;\n'):
        raise SystemExit(
            'autoverifica fallita: modulo di test con attributi non escluso')
    # Dichiarazione FINTA dentro una macro: non governa nulla, e il file
    # omonimo resta codice di produzione da esaminare.
    vittima = os.path.join(RADICE, 'crates', 'x', 'src', 'vittima.rs')
    if vittima in esclusi('#[cfg(test)]\nregister!(mod vittima;);\nmod vittima;\n'):
        raise SystemExit(
            'autoverifica fallita: un file di produzione e\' stato escluso '
            'da una dichiarazione dentro una macro')
    if vittima in esclusi('#[cfg(test)]\nconst X: &str = concat!(mod vittima;);\n'):
        raise SystemExit(
            'autoverifica fallita: esclusione da una sottosequenza qualunque')
    # Con `#[path]` il file non si chiama come il modulo: dedurlo dal nome
    # escludeva il file sbagliato e lasciava fuori dalla scansione quello
    # giusto. In quel caso non si esclude nulla.
    if esclusi('#[cfg(test)]\n#[path = "tests.rs"]\nmod vittima;\n'):
        raise SystemExit(
            'autoverifica fallita: esclusione dedotta dal nome del modulo '
            'in presenza di #[path]')
    # L'attributo dentro una macro non dichiara nessun modulo di test.
    if vittima in esclusi('#[cfg(test)]\nregistra!( ; mod vittima; );\n'):
        raise SystemExit(
            'autoverifica fallita: esclusione da una dichiarazione dentro '
            'una macro con punto e virgola iniziale')

    # Un file incluso ANCHE in produzione non e' un file di test. La regola
    # sta in `main`, che sottrae le inclusioni di produzione: qui si verifica
    # che `file_di_produzione` le veda, e che NON scambi per produzione le
    # dichiarazioni governate da `cfg(test)`.
    condiviso = os.path.join(RADICE, 'crates', 'x', 'src', 'condiviso.rs')

    def produzione(sorgente):
        return file_di_produzione(sorgente, ripulisci(sorgente), finto)

    doppia = ('#[cfg(test)]\nmod condiviso;\n'
              '#[path = "condiviso.rs"]\npub mod riuso;\n')
    if condiviso not in produzione(doppia):
        raise SystemExit(
            'autoverifica fallita: inclusione di produzione via #[path] non '
            'riconosciuta, il file resterebbe escluso')
    if os.path.join(RADICE, 'crates', 'x', 'src', 'tests.rs') in produzione(
            '#[cfg(test)]\nmod tests;\n'):
        raise SystemExit(
            'autoverifica fallita: una dichiarazione governata da cfg(test) '
            'e\' stata scambiata per produzione')
    if condiviso not in produzione('mod condiviso;\n'):
        raise SystemExit(
            'autoverifica fallita: dichiarazione di produzione semplice non '
            'riconosciuta')
    # Forme di inclusione che una regex ingenua non vede, tutte Rust valido.
    altre_forme = [
        'mod r#condiviso;\n',
        '#[path = r"condiviso.rs"]\nmod riuso;\n',
        '#[path = r#"condiviso.rs"#]\nmod riuso;\n',
        'fn f() {\n    include!("condiviso.rs");\n}\n',
        # Percorsi EQUIVALENTI: confrontarli come testo lasciava il file
        # nell'insieme escluso senza mai reinserirlo fra quelli di
        # produzione.
        'fn f() {\n    include!("./condiviso.rs");\n}\n',
        '#[path = "sotto/../condiviso.rs"]\nmod riuso;\n',
    ]
    # Anche FUORI dall'albero di `src`: quel file e' compilato lo stesso, e
    # `sorgenti()` lo aggiunge alla scansione seguendo il riferimento.
    fuori = os.path.normpath(os.path.join(RADICE, 'crates', 'x', 'generato.rs'))
    if fuori not in produzione('fn f() {\n    include!("../generato.rs");\n}\n'):
        raise SystemExit(
            'autoverifica fallita: inclusione di produzione fuori da src non '
            'riconosciuta')
    # Percorsi che il gate NON sa risolvere: dichiararli tali e' l'unica
    # risposta onesta. Un letterale con escape non dice il nome del file —
    # `include!("sha\\u{72}ed.rs")` per Rust e' `shared.rs` — e un argomento
    # costruito da macro nemmeno.
    opachi = [
        'fn f() {\n    include!("condivis' + '\\u{6f}' + '.rs");\n}\n',
        '#[path = "condivis' + '\\u{6f}' + '.rs"]\nmod riuso;\n',
        'fn f() {\n    include!(concat!("condiviso", ".rs"));\n}\n',
    ]
    for sorgente in opachi:
        if not inclusioni_non_risolvibili(sorgente):
            raise SystemExit(
                'autoverifica fallita: inclusione non risolvibile non '
                'dichiarata tale in:\n%s' % sorgente)
    for sorgente in ['fn f() {\n    include!("condiviso.rs");\n}\n',
                     '#[path = r"condiviso.rs"]\nmod riuso;\n']:
        if inclusioni_non_risolvibili(sorgente):
            raise SystemExit(
                'autoverifica fallita: un letterale risolvibile e\' stato '
                'dichiarato opaco:\n%s' % sorgente)
    for sorgente in altre_forme:
        if condiviso not in produzione(sorgente):
            raise SystemExit(
                'autoverifica fallita: inclusione di produzione non '
                'riconosciuta in:\n%s' % sorgente)
    return 10 + len(altre_forme) + len(opachi) + 2


def autoverifica():
    """Il rilevatore deve vedere le mutazioni e non inventarne."""
    mutazioni = [
        # Forme monoriga.
        'fn f(x: u64) -> u64 {\n    assert!(x > 0);\n    x\n}\n',
        'fn f(x: u64) -> u64 {\n    debug_assert_eq!(x, 1);\n    x\n}\n',
        'fn f(x: u64) -> u64 {\n    assert_ne!(x, 0, "motivo");\n    x\n}\n',
        # Macro spezzata su piu' righe: sintassi Rust valida.
        'fn f(x: u64) -> u64 {\n    assert\n        !(x > 0);\n    x\n}\n',
        'fn f(x: u64) -> u64 {\n    debug_assert_eq\n    !(x, 1);\n    x\n}\n',
        # `#[cfg(test)]` dentro un COMMENTO: non governa nulla, e il codice
        # che segue e' di produzione.
        '// #[cfg(test)]\nfn f(x: u64) -> u64 {\n    assert!(x > 0);\n    x\n}\n',
        # `#[cfg(test)]` dentro una stringa: idem.
        'const S: &str = "#[cfg(test)]";\nfn f(x: u64) {\n    assert!(x > 0);\n}\n',
        # Dichiarazione su file: non apre un blocco, il codice dopo resta
        # di produzione.
        '#[cfg(test)]\nmod tests;\n\nfn f(x: u64) {\n    assert!(x > 0);\n}\n',
        # Graffe dentro un letterale: non devono spostare la chiusura del
        # blocco di test.
        '#[cfg(test)]\nmod tests {\n    fn t() {\n        let _ = "}";\n    }\n}\n'
        'fn f(x: u64) {\n    assert!(x > 0);\n}\n',
        # Byte string NORMALE con quote escapata: non e' grezza, e trattarla
        # come tale spegneva il resto del file.
        'fn f() {\n    let _ = b"\\"";\n    assert!(true);\n}\n',
        'fn f() {\n    let _ = b"}\\"{";\n    debug_assert!(true);\n}\n',
        # `#[cfg(test)]` come TESTO dentro un'altra costruzione: non e' un
        # attributo e non governa la funzione che segue.
        '#[doc = stringify!(#[cfg(test)])]\nfn f() {\n    assert!(true);\n}\n',
        # Alias della macro: il nome cambia, la primitiva di panico no.
        'use std::assert as check;\nfn f() {\n    check!(true);\n}\n',
        'use core::assert_eq as uguali;\nfn f() {\n    uguali!(1, 1);\n}\n',
        # Import RAGGRUPPATO: la stessa cosa con le graffe.
        'use std::{assert as check, mem};\nfn f() {\n    check!(true);\n}\n',
        # Alias raw: `r#check` non e' un identificatore qualunque.
        'use std::assert as r#check;\nfn f() {\n    r#check!(true);\n}\n',
        # Il nome passato a un'altra macro, che poi lo espande.
        'fn f() {\n    call!(assert);\n}\n',
        'fn f() {\n    inoltra!(debug_assert_eq, 1, 1);\n}\n',
        # Percorso completo, senza import.
        'fn f() {\n    std::assert!(true);\n}\n',
        # `#[cfg(test)]` fra i TOKEN di una macro: non governa l'elemento che
        # segue, e cercare la prima graffa scavalcava la chiusura della macro
        # prendendo per corpo di test una funzione di produzione.
        'discard! { ; #[cfg(test)] ignored }\nfn f() {\n    assert!(true);\n}\n',
        'scarta!( ; #[cfg(test)] x );\nfn f() {\n    debug_assert!(true);\n}\n',
        # DEFINIZIONE di macro: il corpo e' token puri, e l'identificatore
        # fra `!` e graffa nascondeva la definizione al riconoscitore.
        'macro_rules! dormant { () => { ; #[cfg(test)] ignored } }\n'
        'fn f() {\n    assert!(true);\n}\n',
        # ...anche con nome RAW: `macro_rules! r#dormant` e' Rust valido.
        'macro_rules! r#dormant { () => { ; #[cfg(test)] ignored } }\n'
        'fn f() {\n    assert!(true);\n}\n',
    ]
    for sorgente in mutazioni:
        if not occorrenze(sorgente):
            raise SystemExit(
                'autoverifica fallita: mutazione non vista in:\n%s' % sorgente)
    legittimi = [
        # Assert dentro un modulo di test in linea.
        'fn f() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n'
        '    fn t() {\n        assert_eq!(1, 1);\n    }\n}\n',
        # Il nome della macro citato in un commento non e' una macro.
        "fn f() {}\n// qui un tempo c'era un debug_assert!(x)\n",
        # ...ne' dentro una stringa, grezza o meno.
        'fn f() -> &\'static str {\n    "assert!(x)"\n}\n',
        'fn f() -> &\'static str {\n    r#"debug_assert!(x)"#\n}\n',
        # Commento a blocco, anche annidato.
        'fn f() {}\n/* assert!(x) /* assert_eq!(1, 1) */ */\n',
        # Funzione di sola prova governata da cfg(test).
        '#[cfg(test)]\nfn aiuto() {\n    assert!(true);\n}\n',
        # Identificatori che contengono il nome della macro ma non lo sono.
        'fn assertivo() {}\nstruct Asserzione;\nfn f() { let assert_x = 1; }\n',
        # Byte string grezza: qui la chiusura e' davvero la prima quote.
        'fn f() -> &\'static [u8] {\n    br#"assert!(x)"#\n}\n',
        # Import legittimi che nominano altro.
        'use std::assert_matches_qualcosa_di_diverso::X;\nfn f() {}\n',
    ]
    for sorgente in legittimi:
        trovate = occorrenze(sorgente)
        if trovate:
            raise SystemExit(
                'autoverifica fallita: falso positivo %r su:\n%s'
                % (trovate, sorgente))
    return len(mutazioni), len(legittimi)


def main():
    esclusioni = autoverifica_esclusione_file()
    mutazioni, legittimi = autoverifica()
    percorsi = sorgenti()
    contenuti = dict((p, testo(p)) for p in percorsi)
    esclusi = set()
    produzione = set()
    opache = []
    for percorso, sorgente in contenuti.items():
        pulito = ripulisci(sorgente)
        esclusi |= file_di_test(pulito, percorso)
        produzione |= file_di_produzione(sorgente, pulito, percorso)
        if inclusioni_non_risolvibili(sorgente):
            opache.append(os.path.relpath(percorso, RADICE))
    # Un'inclusione costruita da macro — `include!(concat!("x", ".rs"))` —
    # non dice quale file includa senza espandere le macro. Non sapendo che
    # cosa protegge, il gate rinuncia a escludere QUALUNQUE file: esamina
    # tutto. Costa qualche falso positivo sui file di test, che sono
    # rumorosi e visibili; l'alternativa e' un file di produzione saltato in
    # silenzio.
    if opache:
        esclusi = set()
    # Un file incluso ANCHE in produzione non e' un file di test, per quanto
    # qualcuno lo dichiari sotto `cfg(test)` da qualche altra parte. In
    # dubbio si esamina: un falso positivo e' rumore, un file di produzione
    # mai guardato e' il gate che non impone piu' nulla.
    esclusi -= produzione
    problemi = []
    for percorso in percorsi:
        if percorso in esclusi:
            continue
        for numero, riga in occorrenze(contenuti[percorso]):
            problemi.append((os.path.relpath(percorso, RADICE), numero, riga))
    if problemi:
        sys.stderr.write(
            'macro di assert nel codice di produzione (docs/errori-e-limiti.md'
            ' dichiara che non ce ne sono):\n\n')
        for percorso, numero, riga in problemi:
            sys.stderr.write('- %s:%d: %s\n' % (percorso, numero, riga))
        sys.stderr.write(
            "\nUn'invariante interna si verifica in modo fallibile e si "
            'riporta come errore `Internal`, non con un panico.\n')
        raise SystemExit(1)
    print('nessuna macro di assert nel codice di produzione: %d file '
          'esaminati, %d di test esclusi, %d mutazioni iniettate e viste, '
          '%d forme legittime lasciate passare, %d prove sulla regola di '
          'esclusione dei file'
          % (len(percorsi), len(esclusi & set(percorsi)), mutazioni, legittimi,
             esclusioni))


main()
