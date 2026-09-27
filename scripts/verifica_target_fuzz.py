# -*- coding: utf-8 -*-
"""Gate: ogni target fuzz installa l'hook comune dei panici.

`fuzz_targets/comune/aggancio.rs` distingue il panico atteso di una
dipendenza, dentro una `barriera_di_dipendenza`, da ogni altro panico. Un
target che non lo installa torna all'hook di `libfuzzer-sys`, che interrompe
anche sul primo: resterebbe rosso a barriera funzionante. Il gate pretende
l'installazione in ogni `[[bin]]` di `fuzz/Cargo.toml`.

L'elenco dei target ha una fonte sola, il manifesto: la matrice notturna, lo
smoke e la campagna lunga lo leggono da li', e non ne tengono una copia.

Come gli altri gate del progetto, si autoverifica: inietta una mutazione
sintetica e pretende di vederla.

    python scripts/verifica_target_fuzz.py
"""
import os
import re
import sys

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

MANIFESTO = 'fuzz/Cargo.toml'

# La riga che ogni target deve contenere: l'hook comune nel blocco `init:`.
INSTALLAZIONE_DELL_HOOK = 'fuzz_target!(init: aggancio::installa(),'


def sorgente_del_target(nome):
    return 'fuzz/fuzz_targets/%s.rs' % nome


def testo(percorso):
    with open(os.path.join(RADICE, percorso), encoding='utf-8') as sorgente:
        return sorgente.read()


def bin_del_manifesto(contenuto):
    """I `[[bin]]` dichiarati. Il primo `name` e' quello del package."""
    return re.findall(r'^name\s*=\s*"([^"]+)"', contenuto, re.MULTILINE)[1:]


def controlla(sorgenti):
    dichiarati = bin_del_manifesto(sorgenti[MANIFESTO])
    if not dichiarati:
        return ['%s: nessun `[[bin]]` trovato' % MANIFESTO]
    guasti = []
    for nome in dichiarati:
        percorso = sorgente_del_target(nome)
        if INSTALLAZIONE_DELL_HOOK not in sorgenti.get(percorso, ''):
            guasti.append(
                '%s: il target non installa l\'hook comune (`%s`): tornerebbe '
                'all\'hook di libfuzzer-sys, che interrompe anche sul panico '
                'atteso di una dipendenza' % (percorso, INSTALLAZIONE_DELL_HOOK)
            )
    return guasti


SORGENTI_SINTETICHE = {
    MANIFESTO: '[package]\nname = "sintetico"\n\n[[bin]]\nname = "alfa"\n\n'
               '[[bin]]\nname = "beta"\n',
    **{
        sorgente_del_target(nome): INSTALLAZIONE_DELL_HOOK + ' |payload: &[u8]| {});\n'
        for nome in ('alfa', 'beta')
    },
}


def prova_di_mutazione():
    """Il gate deve vedere un target senza hook. Le sorgenti sono sintetiche:
    mutare i file veri renderebbe la prova dipendente dallo stato che deve
    giudicare."""
    if controlla(SORGENTI_SINTETICHE):
        raise SystemExit('le sorgenti sintetiche di controllo non sono coerenti')
    mutate = dict(SORGENTI_SINTETICHE)
    percorso = sorgente_del_target('beta')
    mutate[percorso] = mutate[percorso].replace(INSTALLAZIONE_DELL_HOOK, 'fuzz_target!(', 1)
    if not controlla(mutate):
        raise SystemExit('mutazione «target senza hook comune» NON vista: il gate non serve')
    return 1


def main():
    viste = prova_di_mutazione()
    sorgenti = {MANIFESTO: testo(MANIFESTO)}
    for nome in bin_del_manifesto(sorgenti[MANIFESTO]):
        percorso = sorgente_del_target(nome)
        if os.path.exists(os.path.join(RADICE, percorso)):
            sorgenti[percorso] = testo(percorso)
    guasti = controlla(sorgenti)
    if guasti:
        print('target fuzz senza l\'hook comune:\n', file=sys.stderr)
        for guasto in guasti:
            print('- %s' % guasto, file=sys.stderr)
        return 1
    print('hook comune in ogni target fuzz: %d `[[bin]]`, %d mutazione iniettata e vista'
          % (len(bin_del_manifesto(sorgenti[MANIFESTO])), viste))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
