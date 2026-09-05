# -*- coding: utf-8 -*-
"""Gate: gli elenchi dei target fuzz coincidono con i `[[bin]]` dichiarati.

I target sono elencati in **quattro** posti — il manifesto `fuzz/Cargo.toml`,
la matrice notturna, lo smoke locale e la campagna lunga — e tre di quegli
elenchi sono copie a mano. Una copia a mano diverge, e diverge in silenzio:

- una rinomina (`plan_v4_parse` in `plan_v5_parse`) puo' aggiornare la matrice
  notturna e non i due script locali. La campagna lunga si ferma allora sul
  target inesistente, e lo smoke registra l'errore nel riepilogo **uscendo
  0**, quindi passa per riuscito;
- l'effetto non e' «un nome sbagliato»: e' che la copertura nuova non viene
  mai esercitata in locale, mentre il riepilogo dice il contrario.

Questo gate confronta i tre elenchi col manifesto, che e' l'unico verificato
dal compilatore.

# La quarantena e' dichiarata, non dedotta

`arrow_transform` e' escluso dalla **sola** matrice notturna, e il motivo sta
in `fuzz.yml`: `libfuzzer-sys` installa un hook di panico che chiama
`abort()` prima dell'unwinding, quindi il target resterebbe rosso a barriera
funzionante, e un job perennemente rosso smette di essere letto.

Quell'esclusione va DICHIARATA qui. Se il gate la deducesse dalla differenza
fra gli elenchi, qualunque target dimenticato nella matrice diventerebbe una
quarantena implicita, che e' il modo in cui una copertura sparisce senza che
nessuno decida di toglierla. E vale per la matrice soltanto: nelle campagne
locali il target c'e' comunque.

Come gli altri gate del progetto, si autoverifica: inietta mutazioni
sintetiche e pretende di vederle.

    python scripts/verifica_target_fuzz.py
"""
import os
import re
import sys

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

MANIFESTO = 'fuzz/Cargo.toml'
MATRICE = '.github/workflows/fuzz.yml'
SCRIPT = ('scripts/fuzz-smoke.sh', 'scripts/fuzz-campaign.sh')

# Escluso dalla sola matrice notturna. Il perche' sta in `fuzz.yml`, accanto
# alla riga commentata; qui c'e' il fatto che l'esclusione e' voluta.
QUARANTENA_NOTTURNA = {
    'arrow_transform':
        "l'hook di panico di libfuzzer-sys chiama abort() prima "
        "dell'unwinding, quindi il target resterebbe rosso a barriera "
        "funzionante (apache/arrow-rs#10575)",
}


def testo(percorso):
    with open(os.path.join(RADICE, percorso), encoding='utf-8') as sorgente:
        return sorgente.read()


def bin_del_manifesto(contenuto):
    """I `[[bin]]` dichiarati: l'unico elenco che il compilatore verifica.

    Il primo `name` e' quello del package, non un target.
    """
    return re.findall(r'^name\s*=\s*"([^"]+)"', contenuto, re.MULTILINE)[1:]


def elenco_dello_script(contenuto, percorso):
    blocco = re.search(r'ALL_TARGETS=\((.*?)\n\)', contenuto, re.DOTALL)
    if blocco is None:
        raise SystemExit('%s: nessun ALL_TARGETS=( ... )' % percorso)
    return re.sub(r'#[^\n]*', ' ', blocco.group(1)).split()


def elenco_della_matrice(contenuto):
    blocco = re.search(r'\n        target:\n(.*?)\n    env:', contenuto, re.DOTALL)
    if blocco is None:
        raise SystemExit('%s: nessuna matrice `target:`' % MATRICE)
    return re.findall(r'^\s*- ([A-Za-z0-9_]+)\s*$', blocco.group(1), re.MULTILINE)


def differenze(atteso, trovato, dove):
    guasti = []
    for nome in atteso:
        if nome not in trovato:
            guasti.append(
                '%s: manca il target `%s`, dichiarato in %s' % (dove, nome, MANIFESTO)
            )
    for nome in trovato:
        if nome not in atteso:
            guasti.append(
                '%s: elenca `%s`, che non e\' un `[[bin]]` di %s. Una campagna su '
                'un target inesistente non esercita nulla' % (dove, nome, MANIFESTO)
            )
    return guasti


def controlla(sorgenti):
    dichiarati = bin_del_manifesto(sorgenti[MANIFESTO])
    if not dichiarati:
        return ['%s: nessun `[[bin]]` trovato' % MANIFESTO]
    guasti = []
    for percorso in SCRIPT:
        guasti += differenze(
            dichiarati, elenco_dello_script(sorgenti[percorso], percorso), percorso
        )
    attesi = [nome for nome in dichiarati if nome not in QUARANTENA_NOTTURNA]
    guasti += differenze(attesi, elenco_della_matrice(sorgenti[MATRICE]), MATRICE)
    for nome in QUARANTENA_NOTTURNA:
        if nome not in dichiarati:
            guasti.append(
                '%s: `%s` e\' dichiarato in quarantena da questo gate ma non '
                'esiste piu\': la quarantena va rimossa insieme al target'
                % (MANIFESTO, nome)
            )
    return guasti


SORGENTI_SINTETICHE = {
    MANIFESTO: '\n'.join([
        '[package]',
        'name = "sintetico"',
        '',
        '[[bin]]',
        'name = "alfa"',
        '',
        '[[bin]]',
        'name = "beta"',
        '',
        '[[bin]]',
        'name = "arrow_transform"',
        '',
    ]),
    SCRIPT[0]: 'ALL_TARGETS=(\n    alfa beta arrow_transform\n)\n',
    SCRIPT[1]: 'ALL_TARGETS=(\n    alfa beta arrow_transform\n)\n',
    MATRICE: '\n        target:\n          - alfa\n          - beta\n    env:\n',
}


def prova_di_mutazione():
    """Il gate deve vedere le forme in cui un elenco puo' divergere.

    Le sorgenti sono **sintetiche**, non quelle del repository: mutare i file
    veri renderebbe la prova dipendente dallo stato che deve giudicare. Col
    difetto gia' presente la mutazione non si applica, e il gate abortisce
    con «mutazione non applicata» invece di segnalare il difetto che ha
    davanti.
    """
    if controlla(SORGENTI_SINTETICHE):
        raise SystemExit('le sorgenti sintetiche di controllo non sono coerenti')
    prove = [
        ('rinomina non propagata', SCRIPT[0],
         lambda t: t.replace('alfa', 'alpha', 1)),
        ('target mancante nella campagna', SCRIPT[1],
         lambda t: t.replace(' beta', '', 1)),
        ('target in piu\' nella matrice', MATRICE,
         lambda t: t.replace('- beta', '- beta\n          - inventato', 1)),
        # La quarantena vale per la matrice, NON per le campagne locali: un
        # target quarantenato rimosso dallo smoke resta un buco di copertura.
        ('quarantenato rimosso dallo smoke', SCRIPT[0],
         lambda t: t.replace(' arrow_transform', '', 1)),
        # E la quarantena non deve sopravvivere al proprio target.
        ('quarantena senza target', MANIFESTO,
         lambda t: t.replace('name = "arrow_transform"', 'name = "gamma"', 1)),
    ]
    for nome, percorso, muta in prove:
        mutate = dict(SORGENTI_SINTETICHE)
        mutate[percorso] = muta(SORGENTI_SINTETICHE[percorso])
        if mutate[percorso] == SORGENTI_SINTETICHE[percorso]:
            raise SystemExit(
                'mutazione %s non applicata: il gate non e\' provato' % nome
            )
        if not controlla(mutate):
            raise SystemExit('mutazione %s NON vista: il gate non serve' % nome)
    return len(prove)


def main():
    viste = prova_di_mutazione()
    sorgenti = {percorso: testo(percorso)
                for percorso in (MANIFESTO, MATRICE, *SCRIPT)}
    guasti = controlla(sorgenti)
    if guasti:
        print('gli elenchi dei target fuzz non coincidono con i `[[bin]]`:\n',
              file=sys.stderr)
        for guasto in guasti:
            print('- %s' % guasto, file=sys.stderr)
        return 1
    dichiarati = bin_del_manifesto(sorgenti[MANIFESTO])
    print(
        'target fuzz coerenti: %d `[[bin]]`, tre elenchi allineati '
        '(%d in quarantena notturna dichiarata, presente comunque nelle '
        'campagne locali), %d mutazioni iniettate e viste'
        % (len(dichiarati), len(QUARANTENA_NOTTURNA), viste)
    )
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
