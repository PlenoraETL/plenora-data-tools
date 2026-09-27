# -*- coding: utf-8 -*-
"""Presidio AGGIUNTIVO sulla privacy dei messaggi di errore (vedi
`errori-e-limiti.md#privacy-dei-messaggi`), non prova completa.

Che cosa INTERCETTA: una chiamata DIRETTA e testuale, in un modulo
percorso-dati (`kernels-*`/`executor`), a un pattern noto di libreria
esterna di parsing/errore (`serde_json::from_*`, `geozero::...to_string()`
o simili elencati sotto) che NON compare gia' nell'elenco `CONSENTITI` —
cioe' un NUOVO sito che nessuno ha ancora esaminato e classificato.

Che cosa NON garantisce, per costruzione (dichiaralo, non nasconderlo):

- non dimostra che gli adattatori GIA' presenti negli usi elencati in
  `CONSENTITI` sanifichino correttamente oggi — quello lo dimostrano solo i
  canary con dati riconoscibili (vedi `errori-e-limiti.md#privacy-dei-messaggi`
  per l'elenco), non questo script;
- e' un controllo TESTUALE per pattern letterale: un alias
  (`use serde_json as sj;`), un wrapper locale, o una chiamata transitiva
  (una nostra funzione che internamente chiama la libreria, chiamata da un
  terzo sito) lo eludono senza sforzo. Non e' un analizzatore di flusso;
- copre SOLO i moduli elencati in `SORVEGLIATI`: codice sotto `examples/`,
  `benchmarks/`, `fuzz/fuzz_targets/` resta fuori perimetro anche se in
  futuro diventasse un percorso dati reale;
- il taglio euristico alla dichiarazione `mod tests` (vedi `solo_produzione`)
  assume che quel modulo sia l'ultimo del file: codice di produzione REALE
  in un modulo FRATELLO dichiarato dopo un `mod tests` annidato altrove nello
  stesso file (non il caso comune in questo repo, dove `mod tests` e' unico
  e in coda) sfuggirebbe alla scansione. Non e' il blind spot gia' chiuso
  (un `#[cfg(test)]` isolato su un singolo item non tronca piu' nulla,
  verificato su `executor/validation.rs` e
  `kernels-table/src/aggregation/compare.rs`, che non hanno affatto
  `mod tests` e vengono ora scansionati per intero) — resta comunque un
  limite di un controllo testuale, non di un parser dell'albero dei moduli;
- la regex `\bmod\s+tests\b` e' un controllo LESSICALE su testo grezzo, non
  su token del compilatore: non distingue una dichiarazione di modulo reale
  da un'occorrenza dentro un COMMENTO o una STRINGA letterale (es. una
  doc-string che nomina `mod tests` come esempio troncherebbe il file nello
  stesso punto sbagliato di un vero modulo), e non riconosce nomi
  alternativi del modulo test (`mod test` al singolare, `mod unit_tests`,
  o qualunque nome diverso dalla convenzione osservata in questo repo). Che
  OGGI non esista nel perimetro sorvegliato un caso concreto di nessuno dei
  due non e' una garanzia sintattica della regex — e' un fatto sull'attuale
  stato del codice, che un refactor puo' cambiare senza che questo script se
  ne accorga;
- copre SOLO `serde_json::from_*`. Cercare solo il nome LETTERALE di
  `geozero`/`geos::Error`/`proj4rs` per sola sottostringa lascerebbe passare
  distinzioni che contano e ne creerebbe di false: quei nomi compaiono anche
  nei semplici `use` che importano un tipo/trait mai coinvolto nella
  formattazione di un errore, indistinguibili con un controllo testuale da
  un sito che propaga davvero — un pattern del genere segnala piu' rumore
  che rischio reale. La propagazione effettiva dell'errore (non
  l'importazione) resta da presidiare con un controllo mirato o dai canary,
  non da questo gate.

Uso:

    python scripts/verifica_privacy_dipendenze.py
"""
import io
import os
import re
import sys

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Directory percorso-dati coperte da questo gate. Ampliarle e' una decisione
# separata, non implicita in questo script.
SORVEGLIATI = [
    'crates/plenora-kernels-table/src',
    'crates/plenora-kernels-geo/src',
    'crates/plenora-engine/src/executor',
]

# Pattern di chiamata diretta a librerie esterne il cui errore/testo puo'
# portare dati dell'ingresso se propagato senza adattamento. Aggiungerne
# uno nuovo qui e' la stessa decisione di aggiungere una dipendenza al
# perimetro sorvegliato — non implicita, va fatta apposta.
PROIBITI = [
    r'serde_json::from_',
]

# (file relativo alla radice del repo, pattern) gia' esaminati, DENTRO il
# perimetro di SORVEGLIATI: adattatore presente, o motivo per cui il
# pattern in quel punto preciso non porta dati dell'ingresso. Un file/
# pattern NON qui, dentro SORVEGLIATI, e' un sito nuovo, non un errore
# dello script.
#
# `crates/plenora-core/src/error.rs` (l'adattatore che tronca/sanifica
# `serde_json` sul piano/config) resta FUORI da questo elenco perche' e'
# fuori da `SORVEGLIATI` per costruzione: non e' un percorso dati, e'
# l'adattatore stesso — includerlo qui lo farebbe sembrare un sito
# sorvegliato quando non lo e'.
CONSENTITI = {
    (
        'crates/plenora-kernels-table/src/analysis.rs',
        r'serde_json::from_',
    ): "l'errore e' scartato (Err(_) =>), nessun testo propagato",
    (
        'crates/plenora-kernels-table/src/analyze/helpers.rs',
        r'serde_json::from_',
    ): (
        "helper `typed()`: deserializza la CONFIG del nodo (piano), non una "
        "cella di dati — stessa eccezione deliberata gia' motivata in "
        "errori-e-limiti.md#privacy-dei-messaggi per il piano/config"
    ),
    (
        'crates/plenora-kernels-geo/src/analyze/helpers.rs',
        r'serde_json::from_',
    ): "helper `parse_config()`: stessa eccezione config del punto precedente",
    ('crates/plenora-kernels-geo/src/crs.rs', r'serde_json::from_'): (
        "parsing del PROJJSON prodotto da PROJ per una DEFINIZIONE di CRS "
        "(configurazione, non una riga di dati) — stessa classe di eccezione "
        "gia' motivata per le definizioni CRS in errori-e-limiti.md"
    ),
}


def elenca_sorgenti():
    for radice_relativa in SORVEGLIATI:
        radice_assoluta = os.path.join(RADICE, radice_relativa.replace('/', os.sep))
        for cartella, _, nomi in os.walk(radice_assoluta):
            for nome in nomi:
                if not nome.endswith('.rs'):
                    continue
                percorso = os.path.join(cartella, nome)
                relativo = os.path.relpath(percorso, RADICE).replace(os.sep, '/')
                yield relativo, percorso


TEST_MODULE = re.compile(r'\bmod\s+tests\b')


def solo_produzione(testo):
    """Taglia il testo alla dichiarazione `mod tests`, non a un `#[cfg(test)]`
    isolato.

    Un `#[cfg(test)]` su un singolo item (una funzione, un blocco dentro
    un'altra funzione) e' legittimo codice di produzione con un ramo
    solo-test, non l'apertura del modulo test — troncare li' escluderebbe
    codice di produzione reale che segue. Verificato apposta: in
    `executor/validation.rs` e `kernels-table/src/aggregation/compare.rs` un
    `#[cfg(test)]` isolato compare a meta' file (o il file non ha affatto
    `mod tests`, i test vivono altrove), con produzione vera dopo.

    Non si cerca la coppia `#[cfg(test)]` + `mod tests` insieme: fra i due
    possono comparire altri attributi o un commento (verificato in
    `kernels-table/src/columns.rs:709-714`, un `#[allow(...)]` e un commento
    fra i due) — richiedere l'adiacenza diretta li mancherebbe. Il nome
    letterale `mod tests` e' gia' l'ancora affidabile in questo repo
    (convenzione osservata ovunque in questa sessione), senza bisogno di
    verificare anche l'attributo che lo precede.

    Se il file non ha `mod tests` (i suoi test vivono in un file separato,
    come `executor/tests.rs`), il testo non si taglia affatto: e' tutto
    produzione, e va scansionato per intero.
    """
    trovato = TEST_MODULE.search(testo)
    return testo if trovato is None else testo[: trovato.start()]


def main():
    problemi = []
    for relativo, percorso in elenca_sorgenti():
        with io.open(percorso, encoding='utf-8', newline='') as sorgente:
            testo = solo_produzione(sorgente.read())
        for pattern in PROIBITI:
            if not re.search(pattern, testo):
                continue
            chiave = (relativo, pattern)
            if chiave in CONSENTITI:
                continue
            problemi.append(
                'NUOVO SITO in %s: pattern %r non presente in CONSENTITI.\n'
                '  Se e\' un percorso dati non sanificato, va corretto o '
                'documentato come rischio accettato. Se non porta dati '
                'dell\'ingresso, aggiungerlo a CONSENTITI con il motivo.'
                % (relativo, pattern)
            )
    # Anche l'inverso: un'ancora in CONSENTITI che non trova piu' il proprio
    # pattern nel file e' un elenco che mente per omissione — la stessa
    # disciplina di verifica_memoria_governata.py.
    cache = {}
    for (relativo, pattern), _motivo in CONSENTITI.items():
        percorso = os.path.join(RADICE, relativo.replace('/', os.sep))
        if relativo not in cache:
            if not os.path.exists(percorso):
                cache[relativo] = None
            else:
                with io.open(percorso, encoding='utf-8', newline='') as sorgente:
                    cache[relativo] = solo_produzione(sorgente.read())
        testo = cache[relativo]
        if testo is None:
            problemi.append('ANCORA PERSA: %s non esiste piu\'' % relativo)
        elif not re.search(pattern, testo):
            problemi.append(
                'ANCORA PERSA in %s: pattern %r non c\'e\' piu\' — la voce in '
                'CONSENTITI e\' ora un\'esenzione senza sito reale, va rimossa'
                % (relativo, pattern)
            )
    if problemi:
        sys.stderr.write(
            'privacy dipendenze: %d problema/i (presidio aggiuntivo, non '
            'prova di sanificazione completa):\n\n' % len(problemi)
        )
        for problema in problemi:
            sys.stderr.write('- %s\n' % problema)
        raise SystemExit(1)
    totale_pattern = sum(1 for _ in PROIBITI)
    print(
        'gate privacy dipendenze pulito: %d pattern sorvegliati su %d directory, '
        '%d siti gia\' esaminati — presidio aggiuntivo, non dimostra la '
        'sanificazione degli adattatori esistenti (vedi i canary dedicati)'
        % (totale_pattern, len(SORVEGLIATI), len(CONSENTITI))
    )


main()
