# -*- coding: utf-8 -*-
"""Gate: il nome che la v4 dava al budget di memoria non torna nel codice.

ADR 15 ha deciso che il limite della libreria si chiama
`max_governed_memory_bytes`, e che **non c'e' alias**: il nome della v4 non
deve continuare a funzionare, perche' un nome che continua a funzionare
continua a promettere un tetto sull'intero processo che in-process non
esiste.

Una decisione del genere marcisce nel modo piu' banale: qualcuno riaggiunge
il vecchio nome «per compatibilita'», e nessuno se ne accorge finche' un
utente non ci costruisce sopra. Questo script lo impedisce.

Verifica quattro cose:

1. il vecchio nome NON compare nel codice di produzione, tranne che nel
   modulo di migrazione, che e' l'unico posto autorizzato a conoscerlo;
2. compare ancora **li dentro**: un allowlist che non serve piu' e' un
   allowlist che nasconde;
3. nei test compare solo nei file che esercitano deliberatamente il rifiuto
   e la migrazione. Un file di test nuovo che lo nomina va guardato, non
   ignorato;
4. `serde(alias` non compare nelle strutture che dichiarano i limiti: e' il
   modo tecnico in cui l'alias rientrerebbe senza che il nome ricompaia in
   chiaro. Il controllo e' mirato a quei file e non al workspace: altrove
   `alias` e' una scelta legittima gia' presa (per esempio
   `sort_alphabetical` in `columns.rs`), e un gate che vieta tutto viene
   disattivato al primo falso positivo.

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

# L'unico modulo di produzione che puo' nominare il formato v4.
MIGRAZIONE = 'crates/plenora-engine/src/plan/migrazione_v4.rs'

# I file che dichiarano i limiti: e' li' che un alias riaprirebbe il vecchio
# nome, ed e' li' che il divieto ha senso.
PORTATORI_DI_LIMITI = [
    'crates/plenora-core/src/limits.rs',
    'crates/plenora-engine/src/plan.rs',
    MIGRAZIONE,
]

# I file di test che lo nominano deliberatamente, e che cosa dimostrano.
TEST_AMMESSI = {
    'crates/plenora-engine/src/plan/migrazione_v4/tests.rs':
        'le prove della migrazione e dei tre modi di sbagliare nome',
    'crates/plenora-engine/src/planner/tests.rs':
        'un piano v5 col nome della v4 non valida, un v4 col nome nuovo nemmeno',
    'crates/plenora-cli/tests/dag_v4_cli.rs':
        'gli stessi rifiuti attraverso la CLI, sul canale congelato',
}

# `max_memory_bytes` NON e' sottostringa di `max_governed_memory_bytes`
# (quello contiene `governed_memory_bytes`), quindi basta cercarlo cosi'
# com'e'. La lookbehind protegge comunque da un futuro `foo_max_memory_bytes`.
OCCORRENZA = re.compile(r'(?<![A-Za-z0-9_])' + VECCHIO + r'(?![A-Za-z0-9_])')
ALIAS = re.compile(r'serde\s*\(\s*[^)]*\balias\b')


def normalizza(percorso):
    return percorso.replace(os.sep, '/')


def e_test(percorso):
    """Un file di test: modulo `tests.rs`, cartella `tests/`, o `#[cfg(test)]`."""
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
    visto_nella_migrazione = False

    for percorso in sorgenti():
        contenuto = testo(percorso)
        occorrenze = righe_con(contenuto, OCCORRENZA)

        if percorso == MIGRAZIONE:
            visto_nella_migrazione = bool(occorrenze)
            continue

        if occorrenze:
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
                    '  `%s` e\' il nome della v4. Il nome della libreria e\' '
                    '`%s`; il vecchio vive solo in %s (ADR 15).'
                    % (percorso, occorrenze[0][0], occorrenze[0][1],
                       VECCHIO, NUOVO, MIGRAZIONE))

        if percorso in PORTATORI_DI_LIMITI:
            for numero, riga in righe_con(contenuto, ALIAS):
                problemi.append(
                    'ALIAS %s:%d: %s\n'
                    '  ADR 15: nessun alias sui limiti. Un nome che continua a '
                    'funzionare continua a promettere.' % (percorso, numero, riga))

    for percorso in PORTATORI_DI_LIMITI:
        if not os.path.exists(percorso):
            problemi.append(
                'PORTATORE DI LIMITI ASSENTE: %s non esiste. Se la struttura '
                'si e\' spostata, il divieto di alias va spostato con lei.'
                % percorso)

    if not os.path.exists(MIGRAZIONE):
        problemi.append(
            'ALLOWLIST STANTIA: %s non esiste piu\'. Se la migrazione v4 e\' '
            'stata rimossa, va rimosso anche questo permesso.' % MIGRAZIONE)
    elif not visto_nella_migrazione:
        problemi.append(
            'ALLOWLIST STANTIA: %s non nomina piu\' `%s`. Un permesso che non '
            'serve piu\' nasconde il prossimo che servira\'.'
            % (MIGRAZIONE, VECCHIO))

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
            'il nome del budget di memoria non e\' quello deciso da ADR 15:\n\n')
        for problema in problemi:
            sys.stderr.write('- %s\n' % problema)
        raise SystemExit(1)

    print('nome del budget di memoria coerente con ADR 15: `%s` in produzione, '
          '`%s` solo in %s e in %d file di test dichiarati'
          % (NUOVO, VECCHIO, MIGRAZIONE, len(TEST_AMMESSI)))


main()
