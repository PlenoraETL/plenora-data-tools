# -*- coding: utf-8 -*-
"""Gate: il nome della v4 non torna nel codice, e il nome della v5 non entra
dove non appartiene.

ADR 15 ha deciso che il limite della libreria si chiama
`max_governed_memory_bytes`, e che **non c'e' alias**: un nome che continua a
funzionare continua a promettere un tetto sull'intero processo che in-process
non esiste.

«Nessun alias» non significa pero' «un solo nome ovunque». Ci sono due formati
sul filo, e ciascuno conserva il proprio:

- il piano canonico **v5** e l'API Rust usano `max_governed_memory_bytes`;
- il piano lineare **v1**, che e' un formato pubblicato e distinguibile
  proprio da `schema_version: 1`, continua a scrivere `max_memory_bytes` e
  viene tradotto all'ingresso;
- il piano **v4** e' letto solo dalla migrazione, che lo porta al canonico.

La differenza fra traduzione e alias e' la simmetria del rifiuto: nessuno dei
due nomi funziona in entrambi i formati. Questo script presidia quella
simmetria e i due soli moduli autorizzati a conoscere il nome della v4.

Una decisione del genere marcisce nel modo piu' banale: qualcuno riaggiunge il
vecchio nome «per compatibilita'» in un terzo posto, e nessuno se ne accorge
finche' un utente non ci costruisce sopra.

Verifica cinque cose:

1. il nome della v4 NON compare nel codice di produzione, fuori dai due
   moduli autorizzati;
2. compare ancora **dentro** ciascuno di essi: un permesso che non serve piu'
   nasconde il prossimo che servira';
3. nei test compare solo nei file che esercitano deliberatamente il rifiuto,
   la migrazione e il formato v1. Un file di test nuovo che lo nomina va
   guardato, non ignorato;
4. `serde(alias` non compare nelle strutture che dichiarano i limiti: e' il
   modo tecnico in cui l'alias rientrerebbe senza che il nome ricompaia in
   chiaro. Il controllo e' mirato a quei file e non al workspace: altrove
   `alias` e' una scelta legittima gia' presa (per esempio
   `sort_alphabetical` in `columns.rs`), e un gate che vieta tutto viene
   disattivato al primo falso positivo;
5. i due moduli autorizzati esistono ancora dove dichiarato.

    python scripts/verifica_nome_budget_memoria.py

Esce 1 al primo scostamento.
"""
import io
import os
import re
import sys

VECCHIO = 'max_memory_bytes'
NUOVO = 'max_governed_memory_bytes'

RADICI = ['crates', os.path.join('fuzz', 'fuzz_targets')]

# I soli moduli di produzione che possono nominare il formato v4, e il
# perche'. Ognuno deve continuare a nominarlo: se smette, il permesso e'
# stantio.
PRODUZIONE_AMMESSA = {
    'crates/plenora-engine/src/plan/migrazione_v4.rs':
        'la migrazione v4 -> v5: legge il nome della v4 e lo traduce',
    'crates/plenora-engine/src/table_engine/contract.rs':
        'la struttura wire del piano lineare v1, che conserva il proprio nome',
}

# I file che dichiarano i limiti: e' li' che un alias riaprirebbe il vecchio
# nome, ed e' li' che il divieto ha senso.
PORTATORI_DI_LIMITI = [
    'crates/plenora-core/src/limits.rs',
    'crates/plenora-kernels-table/src/lib.rs',
    'crates/plenora-engine/src/plan.rs',
] + sorted(PRODUZIONE_AMMESSA)

# I file di test che lo nominano deliberatamente, e che cosa dimostrano.
TEST_AMMESSI = {
    'crates/plenora-engine/src/plan/migrazione_v4/tests.rs':
        'le prove della migrazione e dei tre modi di sbagliare nome',
    'crates/plenora-engine/src/planner/tests.rs':
        'un piano v5 col nome della v4 non valida, un v4 col nome nuovo nemmeno',
    'crates/plenora-cli/tests/dag_v4_cli.rs':
        'gli stessi rifiuti attraverso la CLI, sul canale congelato, piu\' il '
        'formato v1 che conserva il proprio nome',
    'crates/plenora-cli/tests/matrice_cli.rs':
        'i piani lineari v1 della matrice, che scrivono il nome della v1',
}

# `max_memory_bytes` NON e' sottostringa di `max_governed_memory_bytes`
# (quello contiene `governed_memory_bytes`), quindi basta cercarlo cosi'
# com'e'. La lookbehind protegge comunque da un futuro `foo_max_memory_bytes`.
OCCORRENZA = re.compile(r'(?<![A-Za-z0-9_])' + VECCHIO + r'(?![A-Za-z0-9_])')
# Solo l'ATTRIBUTO, non la parola: il modulo di migrazione spiega a voce alta
# perche' `serde(alias)` non e' stato usato, e un gate che non sa distinguere
# una spiegazione da un'attuazione vieta di spiegarsi.
ALIAS = re.compile(r'#\s*\[\s*serde\s*\([^)]*\balias\b')


def normalizza(percorso):
    return percorso.replace(os.sep, '/')


def e_test(percorso):
    """Un file di test: modulo `tests.rs` oppure sotto una cartella `tests/`.

    I moduli `#[cfg(test)]` in linea dentro un sorgente di produzione NON
    sono esentati: il file resta di produzione, e un nome che rientra da un
    modulo di test in linea rientra comunque nel file sbagliato.
    """
    parti = normalizza(percorso).split('/')
    return parti[-1] == 'tests.rs' or 'tests' in parti[:-1]


def sorgenti():
    for radice in RADICI:
        for cartella, _, nomi in os.walk(radice):
            for nome in sorted(nomi):
                if nome.endswith('.rs'):
                    yield normalizza(os.path.join(cartella, nome))


def testo(percorso):
    return io.open(percorso, encoding='utf-8', newline='').read()


def righe_con(contenuto, pattern):
    return [(numero, riga.strip())
            for numero, riga in enumerate(contenuto.split('\n'), 1)
            if pattern.search(riga)]


def main():
    problemi = []
    visti = set()

    for percorso in sorgenti():
        contenuto = testo(percorso)
        occorrenze = righe_con(contenuto, OCCORRENZA)

        if percorso in PRODUZIONE_AMMESSA:
            if occorrenze:
                visti.add(percorso)
        elif occorrenze:
            if e_test(percorso):
                if percorso not in TEST_AMMESSI:
                    problemi.append(
                        'TEST NON PREVISTO %s: nomina `%s` alla riga %d.\n'
                        '  I test che possono nominarlo sono elencati in questo '
                        'script; aggiungerne uno e\' una decisione, non un '
                        'dettaglio.' % (percorso, VECCHIO, occorrenze[0][0]))
            else:
                problemi.append(
                    'PRODUZIONE %s:%d: %s\n'
                    '  `%s` e\' il nome della v4 e della v1. Il nome della '
                    'libreria e\' `%s`; il vecchio vive solo in:\n%s'
                    % (percorso, occorrenze[0][0], occorrenze[0][1],
                       VECCHIO, NUOVO,
                       '\n'.join('    - %s (%s)' % (chiave, motivo)
                                 for chiave, motivo
                                 in sorted(PRODUZIONE_AMMESSA.items()))))

        if percorso in PORTATORI_DI_LIMITI:
            for numero, riga in righe_con(contenuto, ALIAS):
                problemi.append(
                    'ALIAS %s:%d: %s\n'
                    '  ADR 15: nessun alias sui limiti. Una traduzione fra '
                    'formati e\' un\'altra cosa, e rifiuta il nome dell\'altro '
                    'formato in modo simmetrico.' % (percorso, numero, riga))

    for percorso, motivo in sorted(PRODUZIONE_AMMESSA.items()):
        if not os.path.exists(percorso):
            problemi.append(
                'PERMESSO STANTIO: %s non esiste piu\'. Se %s e\' stata '
                'rimossa, va rimosso anche il permesso.' % (percorso, motivo))
        elif percorso not in visti:
            problemi.append(
                'PERMESSO STANTIO: %s non nomina piu\' `%s`, ma dovrebbe '
                'essere %s. Un permesso che non serve piu\' nasconde il '
                'prossimo che servira\'.' % (percorso, VECCHIO, motivo))

    for percorso in PORTATORI_DI_LIMITI:
        if not os.path.exists(percorso):
            problemi.append(
                'PORTATORE DI LIMITI ASSENTE: %s non esiste. Se la struttura '
                'si e\' spostata, il divieto di alias va spostato con lei.'
                % percorso)

    for percorso in sorted(TEST_AMMESSI):
        if not os.path.exists(percorso):
            problemi.append(
                'TEST AMMESSO ASSENTE: %s non esiste (%s).'
                % (percorso, TEST_AMMESSI[percorso]))
        elif not OCCORRENZA.search(testo(percorso)):
            problemi.append(
                'TEST AMMESSO SENZA PROVA: %s non nomina piu\' `%s`, ma '
                'dovrebbe dimostrare che %s.'
                % (percorso, VECCHIO, TEST_AMMESSI[percorso]))

    if problemi:
        sys.stderr.write(
            'i nomi del budget di memoria non sono quelli decisi da ADR 15:\n\n')
        for problema in problemi:
            sys.stderr.write('- %s\n' % problema)
        raise SystemExit(1)

    print('nomi del budget di memoria coerenti con ADR 15: `%s` in produzione, '
          '`%s` in %d moduli autorizzati e in %d file di test dichiarati'
          % (NUOVO, VECCHIO, len(PRODUZIONE_AMMESSA), len(TEST_AMMESSI)))


main()
