# -*- coding: utf-8 -*-
"""Genera `docs/STATO.md` leggendo il codice, e verifica che sia aggiornato.

Versione, toolchain, formati del piano, catalogo, comandi, feature, gate, job
di CI, target fuzz e numero di test vivono nel codice. Questo generatore li
rende senza duplicarli a mano nei documenti; in modalita' `--check` pretende
che una rigenerazione non produca differenze.

    python scripts/genera_stato.py            # riscrive il documento
    python scripts/genera_stato.py --check    # esce 1 se e' disallineato
"""
import glob
import io
import json
import os
import re
import sys
from collections import Counter

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DESTINAZIONE = os.path.join(RADICE, 'docs', 'STATO.md')


def leggi(percorso):
    with open(os.path.join(RADICE, percorso), encoding='utf-8') as sorgente:
        return sorgente.read().replace('\r\n', '\n')


def unico(schema, testo, dove):
    """L'unica occorrenza di `schema` in `testo`: zero o due sono un errore."""
    trovati = re.findall(schema, testo, re.MULTILINE)
    if len(trovati) != 1:
        raise SystemExit('%s: attesa una sola occorrenza di %r, trovate %d'
                         % (dove, schema, len(trovati)))
    return trovati[0]


def versione_e_toolchain():
    cargo = leggi('Cargo.toml')
    sezione = cargo[cargo.index('[workspace.package]'):]
    versione = unico(r'^version\s*=\s*"([^"]+)"', sezione.split('\n[', 1)[0], 'Cargo.toml')
    toolchain = unico(r'^channel\s*=\s*"([^"]+)"', leggi('rust-toolchain.toml'),
                      'rust-toolchain.toml')
    return versione, toolchain


def formati_del_piano():
    piano = leggi('crates/plenora-engine/src/plan.rs')
    versioni = re.findall(r'^pub const PLAN_SCHEMA_VERSION_V(\d+): u16 = (\d+);', piano,
                          re.MULTILINE)
    if not versioni:
        raise SystemExit('plan.rs: nessuna costante PLAN_SCHEMA_VERSION_V*')
    for nome, valore in versioni:
        if nome != valore:
            raise SystemExit('plan.rs: PLAN_SCHEMA_VERSION_V%s vale %s' % (nome, valore))
    return sorted(int(v) for _, v in versioni)


def catalogo():
    voci = json.loads(leggi('crates/plenora-engine/tests/catalog_snapshot.snap'))
    if not voci:
        raise SystemExit('catalog_snapshot.snap: catalogo vuoto')
    return (len(voci), Counter(v['family'] for v in voci),
            Counter(v['maturity'] for v in voci))


def comandi():
    main = leggi('crates/plenora-cli/src/main.rs')
    # Il dispatch comincia dall'aiuto e finisce col braccio di rifiuto `_ =>`.
    inizio = main.index('Some("--help" | "-h") =>')
    fine = main.index('\n        _ =>', inizio)
    blocco = main[inizio:fine]
    nomi = []
    for braccio in re.findall(r'Some\(((?:"[^"]+"\s*\|?\s*)+)\)\s*=>', blocco):
        alternative = re.findall(r'"([^"]+)"', braccio)
        if alternative[0].startswith('-'):
            continue
        nomi.append(alternative)
    if not nomi:
        raise SystemExit('main.rs: nessun comando trovato nel dispatch')
    return nomi


def feature():
    cli = leggi('crates/plenora-cli/Cargo.toml')
    sezione = cli[cli.index('[features]'):].split('\n[', 1)[0]
    return re.findall(r'^([a-z][a-z0-9-]*)\s*=\s*(\[.*\])', sezione, re.MULTILINE)


def job(workflow):
    """(id, nome) dei job di un workflow, nell'ordine in cui compaiono."""
    testo = leggi(workflow)
    corpo = testo[testo.index('\njobs:\n'):]
    return re.findall(r'^  ([a-z0-9_-]+):\n(?:    #.*\n)*    name: (.+)$', corpo, re.MULTILINE)


def gate_python():
    gate = []
    for percorso in sorted(glob.glob(os.path.join(RADICE, 'scripts', 'verifica_*.py'))):
        testo = leggi(os.path.relpath(percorso, RADICE))
        doc = re.search(r'"""(.*?)(?:\n\s*\n|""")', testo, re.DOTALL)
        prima = ' '.join(doc.group(1).split()) if doc else ''
        gate.append((os.path.basename(percorso), prima))
    return gate


def target_fuzz():
    manifesto = leggi('fuzz/Cargo.toml')
    sezioni = manifesto.count('\n[[bin]]')
    nomi = re.findall(r'^\[\[bin\]\]\nname\s*=\s*"([^"]+)"', manifesto, re.MULTILINE)
    if len(nomi) != sezioni or not nomi:
        raise SystemExit('fuzz/Cargo.toml: %d [[bin]] e %d nomi letti' % (sezioni, len(nomi)))
    return nomi


def test_per_crate():
    conti = Counter()
    for percorso in glob.glob(os.path.join(RADICE, 'crates', '*', '**', '*.rs'), recursive=True):
        relativo = os.path.relpath(percorso, RADICE).replace(os.sep, '/')
        crate = relativo.split('/')[1]
        conti[crate] += len(re.findall(r'#\[test\]', leggi(relativo)))
    return sorted(conti.items())


def rendi():
    versione, toolchain = versione_e_toolchain()
    formati = formati_del_piano()
    operazioni, famiglie, maturita = catalogo()
    righe = [
        '# Stato',
        '',
        '<!-- Generato da `python scripts/genera_stato.py`: non si modifica a mano. -->',
        '',
        'Che cosa il codice dichiara oggi, letto dal codice. Che cosa manca sta in',
        '[`stato-e-roadmap.md`](stato-e-roadmap.md).',
        '',
        '## Versioni',
        '',
        '| | |',
        '|---|---|',
        '| workspace | `%s` |' % versione,
        '| toolchain Rust | `%s` |' % toolchain,
        '| versioni del piano DAG riconosciute | %s — la v4 si migra alla v5 '
        '([`piano-v5.md`](piano-v5.md)) |' % ', '.join('v%d' % v for v in formati),
        '',
        '## Catalogo',
        '',
        '%d operazioni, in [`operazioni.md`](operazioni.md).' % operazioni,
        '',
        '| famiglia | operazioni |',
        '|---|---|',
    ]
    righe += ['| `%s` | %d |' % (f, n) for f, n in sorted(famiglie.items())]
    righe += ['', '| maturità | operazioni |', '|---|---|']
    righe += ['| `%s` | %d |' % (m, n) for m, n in sorted(maturita.items())]
    righe += ['', '## Comandi della CLI', '']
    for alternative in comandi():
        alias = ' (anche %s)' % ', '.join('`%s`' % a for a in alternative[1:]) \
            if len(alternative) > 1 else ''
        righe.append('- `%s`%s' % (alternative[0], alias))
    righe += ['', '## Feature della CLI', '', '| feature | abilita |', '|---|---|']
    righe += ['| `%s` | `%s` |' % (nome, valore) for nome, valore in feature()]
    righe += ['', '## Job di CI', '']
    for workflow in ('.github/workflows/ci.yml', '.github/workflows/fuzz.yml'):
        righe.append('`%s`:' % workflow)
        righe.append('')
        righe += ['- %s' % nome.strip() for _, nome in job(workflow)]
        righe.append('')
    righe += ['## Gate Python', '', '| script | che cosa presidia |', '|---|---|']
    righe += ['| `%s` | %s |' % (nome, prima.replace('|', '\\|'))
              for nome, prima in gate_python()]
    fuzz = target_fuzz()
    righe += ['', '## Target fuzz', '', '%d target, dai `[[bin]]` di `fuzz/Cargo.toml`: %s.'
              % (len(fuzz), ', '.join('`%s`' % t for t in fuzz))]
    conti = test_per_crate()
    righe += ['', '## Test', '',
              'Occorrenze di `#[test]` nel sorgente, per crate: un conteggio statico, '
              'non l\'esito di un\'esecuzione, e comprende i test dietro feature e '
              'piattaforme.', '',
              '| crate | `#[test]` |', '|---|---|']
    righe += ['| `%s` | %d |' % (crate, n) for crate, n in conti]
    righe += ['| **totale** | **%d** |' % sum(n for _, n in conti), '']
    return '\n'.join(righe)


def main():
    atteso = rendi()
    if '--check' in sys.argv[1:]:
        attuale = leggi('docs/STATO.md') if os.path.exists(DESTINAZIONE) else ''
        if attuale != atteso:
            print('docs/STATO.md non e\' allineato al codice: rigeneralo con '
                  '`python scripts/genera_stato.py`', file=sys.stderr)
            return 1
        print('docs/STATO.md allineato al codice')
        return 0
    with open(DESTINAZIONE, 'w', encoding='utf-8', newline='\n') as uscita:
        uscita.write(atteso)
    print('docs/STATO.md rigenerato')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
