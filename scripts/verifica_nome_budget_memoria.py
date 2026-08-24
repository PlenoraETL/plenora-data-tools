# -*- coding: utf-8 -*-
"""Gate: il nome della v4 non torna nel codice, e il nome della v5 non entra
dove non appartiene.

errori-e-limiti.md#memoria-governata ha deciso che il limite della libreria si chiama
`max_governed_memory_bytes`, e che **non c'e' alias**: un nome che continua a
funzionare continua a promettere un tetto sull'intero processo che in-process
non esiste.

«Nessun alias» non significa pero' «un solo nome ovunque». Ci sono tre formati
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

Verifica sei cose:

1. il nome della v4 NON compare nel codice di produzione, fuori dai due
   moduli autorizzati;
2. compare ancora **dentro** ciascuno di essi: un permesso che non serve piu'
   nasconde il prossimo che servira';
3. nei test compare solo nei file che esercitano deliberatamente il rifiuto,
   la migrazione e il formato v1. Un file di test nuovo che lo nomina va
   guardato, non ignorato;
4. nessun attributo `#[serde(...)]` delle strutture che dichiarano i limiti
   contiene `alias`: e' il modo tecnico in cui l'alias rientrerebbe senza che
   il nome ricompaia in chiaro. L'attributo e' analizzato **per intero**,
   anche quando e' spezzato su piu' righe, perche' un controllo riga per riga
   lascerebbe passare la forma indentata — che e' proprio quella che rustfmt
   produce quando l'attributo cresce. Il controllo e' mirato a quei file e non
   al workspace: altrove `alias` e' una scelta legittima gia' presa (per
   esempio `sort_alphabetical` in `columns.rs`), e un gate che vieta tutto
   viene disattivato al primo falso positivo;
5. i due moduli autorizzati esistono ancora dove dichiarato;
6. **il gate vede davvero l'alias**: su ciascun modulo autorizzato viene
   iniettato in memoria un alias multilinea sul campo della v4, e il
   rilevatore deve accorgersene. Un gate che non e' mai stato visto fallire
   non e' un gate, e' una riga di log.

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
    'crates/plenora-engine/tests/oracoli_identita.rs':
        'l\'oracolo dell\'identita\' tiene un piano v4 che scrive il nome '
        'della v4, per fissare che la migrazione produce lo STESSO plan_hash '
        'del suo equivalente v5: senza il nome vecchio il piano non sarebbe v4',
}

# `max_memory_bytes` NON e' sottostringa di `max_governed_memory_bytes`
# (quello contiene `governed_memory_bytes`), quindi basta cercarlo cosi'
# com'e'. La lookbehind protegge comunque da un futuro `foo_max_memory_bytes`.
OCCORRENZA = re.compile(r'(?<![A-Za-z0-9_])' + VECCHIO + r'(?![A-Za-z0-9_])')
PAROLA_ALIAS = re.compile(r'\balias\b')


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


def attributi(contenuto):
    """Ogni attributo esterno `#[...]` come `(riga, testo completo)`.

    Consuma fino alla parentesi quadra che chiude, contando le parentesi
    annidate e saltando il contenuto delle stringhe: un attributo puo'
    occupare piu' righe, e leggerlo riga per riga significa non leggerlo.
    """
    trovati = []
    posizione = 0
    lunghezza = len(contenuto)
    while True:
        inizio = contenuto.find('#[', posizione)
        if inizio < 0:
            return trovati
        profondita = 0
        in_stringa = False
        fuga = False
        indice = inizio + 1
        while indice < lunghezza:
            carattere = contenuto[indice]
            if in_stringa:
                if fuga:
                    fuga = False
                elif carattere == '\\':
                    fuga = True
                elif carattere == '"':
                    in_stringa = False
            elif carattere == '"':
                in_stringa = True
            elif carattere in '[(':
                profondita += 1
            elif carattere in '])':
                profondita -= 1
                if profondita == 0:
                    break
            indice += 1
        if indice >= lunghezza:
            # Attributo non chiuso: il file non compilerebbe, ma il gate non
            # deve andare in loop ne' tacere.
            trovati.append((contenuto.count('\n', 0, inizio) + 1,
                            contenuto[inizio:]))
            return trovati
        trovati.append((contenuto.count('\n', 0, inizio) + 1,
                        contenuto[inizio:indice + 1]))
        posizione = indice + 1


def alias_negli_attributi(contenuto):
    """Attributi `serde` che contengono `alias`, riga e forma compattata.

    Solo l'ATTRIBUTO, non la parola: il modulo di migrazione spiega a voce
    alta perche' `serde(alias)` non e' stato usato, e un gate che non sa
    distinguere una spiegazione da un'attuazione vieta di spiegarsi.
    """
    return [(riga, ' '.join(attributo.split()))
            for riga, attributo in attributi(contenuto)
            if 'serde' in attributo and PAROLA_ALIAS.search(attributo)]


# L'alias che il gate deve vedere: spezzato su piu' righe, cioe' nella forma
# in cui rustfmt lo scriverebbe se qualcuno lo aggiungesse davvero.
MUTAZIONE = ('#[serde(\n'
             '    alias = "%s"\n'
             ')]\n' % NUOVO)


def prova_di_mutazione():
    """Inietta l'alias in ciascun modulo autorizzato e pretende il rifiuto."""
    problemi = []
    for percorso in sorted(PRODUZIONE_AMMESSA):
        if not os.path.exists(percorso):
            continue
        originale = testo(percorso)
        if alias_negli_attributi(originale):
            continue  # gia' segnalato dal controllo normale
        ancora = OCCORRENZA.search(originale)
        if ancora is None:
            continue  # gia' segnalato come permesso stantio
        mutato = originale[:ancora.start()] + MUTAZIONE + originale[ancora.start():]
        if not alias_negli_attributi(mutato):
            problemi.append(
                'IL GATE E\' CIECO: un alias multilinea iniettato in %s non '
                'viene visto.\n  Il rilevatore va corretto: senza, il divieto '
                'di alias e\' una riga di log.' % percorso)
    return problemi


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
            for numero, attributo in alias_negli_attributi(contenuto):
                problemi.append(
                    'ALIAS %s:%d: %s\n'
                    '  errori-e-limiti.md#memoria-governata: nessun alias sui limiti. Una traduzione fra '
                    'formati e\' un\'altra cosa, e rifiuta il nome dell\'altro '
                    'formato in modo simmetrico.' % (percorso, numero, attributo))

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

    problemi.extend(prova_di_mutazione())

    if problemi:
        sys.stderr.write(
            'i nomi del budget di memoria non sono quelli decisi da errori-e-limiti.md#memoria-governata:\n\n')
        for problema in problemi:
            sys.stderr.write('- %s\n' % problema)
        raise SystemExit(1)

    print('nomi del budget di memoria coerenti con errori-e-limiti.md#memoria-governata: `%s` in produzione, '
          '`%s` in %d moduli autorizzati e in %d file di test dichiarati; '
          'alias multilinea iniettato e rifiutato in entrambi i moduli'
          % (NUOVO, VECCHIO, len(PRODUZIONE_AMMESSA), len(TEST_AMMESSI)))


main()
