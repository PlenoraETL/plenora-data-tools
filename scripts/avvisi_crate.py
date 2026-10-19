"""Avvisi crate per crate, offline, per i quattro grafi del rilascio.

Il registro fissa il grafo e i testi: --aggiorna li raccoglie dalla cache Cargo.
Un testo mancante resta registrato con una ragione e fa fallire il gate.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

try:
    from . import strict_json
except ImportError:
    import strict_json

ROOT = Path(__file__).resolve().parents[1]
REGISTRO = ROOT / 'assurance/registries/avvisi-crate.json'
TESTI = ROOT / 'assurance/licenze-crate'
TARGETS = ('x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc')
PRODOTTI = ('cli', 'wheel')
NOMI = re.compile(r'^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)', re.I)
# Un COPYING o un NOTICE possono essere soli rinvii (copyright e link alle
# condizioni): l'avviso completo pretende almeno un testo con nome LICENSE.
CONDIZIONI = re.compile(r'^(LICEN[CS]E|UNLICENSE)', re.I)
# Le eccezioni dichiarate alla regola dei nomi: un COPYING che **e'** il testo
# pieno delle condizioni. L'impronta ancora l'eccezione a quei byte: se il
# testo cambia, l'eccezione muore e la regola torna a valere.
COPYING_COMPLETO = {
    # BSD-3-Clause intero (redistribution and use... provided that the
    # following conditions are met), non un rinvio.
    ('snap-1.1.2', '7ca1297d23644e30bd489193a82a33f324e5fe33f25df4195649b91b883df967'),
}
BLOB = re.compile(r'[0-9a-f]{40}')


def impronta(data: bytes) -> str:
    """Normalizza solo i fine riga, anche nei checkout Windows."""
    return hashlib.sha256(data.replace(b'\r\n', b'\n')).hexdigest()


def impronte_blob(data: bytes) -> tuple[str, str]:
    """Le impronte git possibili del testo conservato.

    L'impronta del blob e' sui byte esatti dell'upstream, ma il checkout
    locale puo' normalizzare i fine riga: si ricostruiscono le due forme
    pure, LF e CRLF, e il blob dichiarato deve essere una delle due.
    """
    lf = data.replace(b'\r\n', b'\n')
    crlf = lf.replace(b'\n', b'\r\n')
    return tuple(hashlib.sha1(b'blob %d\x00' % len(v) + v).hexdigest() for v in (lf, crlf))


def errori_provenienza(key: str, item: dict, data: bytes) -> None:
    """Un testo supplementare entra solo con provenienza provata dai byte.

    `origine` deve nominare l'URL https del testo e `git_blob` l'impronta che
    Git da' a quel blob: il gate la ricalcola offline dai byte conservati.
    """
    origine = str(item.get('origine', ''))
    blob = str(item.get('git_blob', ''))
    if not origine.startswith('https://') or not BLOB.fullmatch(blob):
        raise ValueError(f'{key}: testo supplementare senza provenienza (origine https e git_blob)')
    if blob not in impronte_blob(data):
        raise ValueError(f'{key}: il testo non e il blob git dichiarato dalla origine')


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode('utf-8')


def cargo(*args: str) -> str:
    # Il colore si spegne per questa invocazione: la CI imposta
    # CARGO_TERM_COLOR=always e le sequenze ANSI romperebbero il parser.
    ambiente = {**os.environ, 'CARGO_TERM_COLOR': 'never'}
    return subprocess.check_output(
        ['cargo', *args, '--offline', '--locked'],
        cwd=ROOT, text=True, encoding='utf-8', env=ambiente,
    )


def grafo() -> tuple[dict, dict]:
    """Cargo sceglie cfg e feature; normal esclude dev/build e proc-macro.

    cargo tree non ha --release: i profili non cambiano la risoluzione.
    Le feature sono quelle di cargo build --release e di maturin release.
    """
    metadata = json.loads(cargo('metadata', '--format-version', '1'))
    packages = {}
    for p in metadata['packages']:
        key = f"{p['name']}-{p['version']}"
        if key in packages:
            raise ValueError('identita Cargo ambigua')
        p['nostro'] = p['id'] in metadata['workspace_members']
        packages[key] = p
    graphs = {}
    for target in TARGETS:
        for prodotto in PRODOTTI:
            root = 'plenora-cli' if prodotto == 'cli' else 'plenora-data-py'
            args = ['tree', '-p', root, '--target', target, '-e', 'normal,no-proc-macro',
                    '--prefix', 'none', '--format', '{p}']
            if prodotto == 'wheel':
                args += ['--features', 'pyo3/extension-module']
            keys = set()
            lines = cargo(*args).splitlines()
            for line in lines:
                match = re.fullmatch(r'(\S+) v(\S+)(?: \([^\n]*\))?(?: \(\*\))?', line)
                if not match:
                    raise ValueError('formato cargo tree non riconosciuto')
                key = f'{match[1]}-{match[2]}'
                keys.add(key)
            if not keys:
                raise ValueError('grafo Cargo vuoto')
            if prodotto == 'wheel':
                # Anche i wheel delle prove vengono caricati dalla CI. Le sonde
                # devono mantenere lo stesso insieme di crate della release.
                probe_args = args.copy()
                probe_args[-1] += ',sonde-di-prova'
                if set(cargo(*probe_args).splitlines()) != set(lines):
                    raise ValueError('le sonde cambiano il grafo: registrare un profilo dedicato')
            graphs[f'{prodotto}/{target}'] = sorted(keys)
    return graphs, {k: packages[k] for k in sorted(set().union(*map(set, graphs.values())))}


def file_del_pacchetto(package: dict) -> dict[str, bytes]:
    """Include NOTICE e licenze native annidate, senza appiattire i nomi."""
    root = Path(package['manifest_path']).parent.resolve()
    paths = {p for p in root.rglob('*') if p.is_file() and NOMI.match(p.name)}
    if package.get('license_file'):
        path = (root / package['license_file']).resolve()
        if not path.is_relative_to(root) or not path.is_file():
            raise ValueError('license-file assente o esterno al pacchetto')
        paths.add(path)
    result = {}
    # Ordine per byte del percorso POSIX: il confronto fra Path su Windows
    # ignora le maiuscole, e produrrebbe un registro con un ordine che Linux
    # non riproduce (visto su aws-lc-sys in database-tools).
    for path in sorted(paths, key=Path.as_posix):
        if path.is_symlink() or not path.resolve().is_relative_to(root):
            raise ValueError('testo di licenza esterno al pacchetto')
        data = path.read_bytes().replace(b'\r\n', b'\n')
        if not data.strip():
            raise ValueError('testo di licenza vuoto')
        result[path.relative_to(root).as_posix()] = data
    if package.get('nostro') and (ROOT / 'LICENSE').is_file():
        result['LICENSE'] = (ROOT / 'LICENSE').read_bytes().replace(b'\r\n', b'\n')
    return result


def percorso(value: str) -> str:
    if (not isinstance(value, str) or not value or '\\' in value or ':' in value
            or any(p in ('', '.', '..') for p in value.split('/'))):
        raise ValueError('percorso di avviso non valido')
    return value


def registro() -> dict:
    return strict_json.loads(REGISTRO.read_text(encoding='utf-8'))


def contenuti(dati: dict, *, incompleti: bool = False) -> dict[str, bytes]:
    """Valida il registro prima di usarlo: niente eccezioni implicite."""
    if dati['schema_version'] != 1:
        raise ValueError('schema avvisi sconosciuto')
    packages = dati['crate']
    required = set().union(*map(set, dati['grafi'].values()))
    if not required or required != set(packages):
        raise ValueError('registro e grafi discordanti')
    output = {}
    missing = []
    for key, entry in sorted(packages.items()):
        percorso(key)
        if not entry['avvisi']:
            if not entry.get('mancante'):
                raise ValueError('crate senza avviso ne ragione')
            missing.append(key)
        elif entry.get('mancante'):
            raise ValueError('ragione di assenza obsoleta')
        for item in entry['avvisi']:
            path = key + '/' + percorso(item['file'])
            if path in output:
                raise ValueError('avviso duplicato')
            data = (TESTI / path).read_bytes().replace(b'\r\n', b'\n')
            if not data.strip() or impronta(data) != item['sha256']:
                raise ValueError('testo di licenza alterato: ' + path)
            if item.get('origine') != 'pacchetto':
                errori_provenienza(key, item, data)
            output[path] = data
        if entry['avvisi'] and not any(
            CONDIZIONI.match(item['file'].split('/')[-1])
            or (key, item['sha256']) in COPYING_COMPLETO
            for item in entry['avvisi']
        ):
            raise ValueError(f'{key}: soli rinvii (COPYING/NOTICE), manca il testo delle condizioni')
    actual = {p.relative_to(TESTI).as_posix() for p in TESTI.rglob('*') if p.is_file()}
    if actual != set(output):
        raise ValueError('testi non registrati o mancanti')
    if missing and not incompleti:
        raise ValueError('testi assenti nel checkout/cache, rilascio bloccato: ' + ', '.join(missing))
    return output


def verifica(dati: dict) -> None:
    graphs, packages = grafo()
    if dati['grafi'] != graphs:
        raise ValueError('grafo degli avvisi obsoleto: rigenerare il registro')
    for key, p in packages.items():
        entry = dati['crate'][key]
        if entry['licenza'] != p['license']:
            raise ValueError('licenza del manifesto cambiata: ' + key)
        files = file_del_pacchetto(p)
        registered = {
            v['file']: v['sha256'] for v in entry['avvisi'] if v.get('origine') == 'pacchetto'
        }
        if files and {k: impronta(v) for k, v in files.items()} != registered:
            raise ValueError('testi del pacchetto diversi dal registro: ' + key)
        if not files and entry['avvisi'] and any(v['origine'] == 'pacchetto' for v in entry['avvisi']):
            raise ValueError('testi del pacchetto non piu presenti: ' + key)
    contenuti(dati)


def aggiorna() -> None:
    graphs, packages = grafo()
    old = registro() if REGISTRO.exists() else {'crate': {}}
    entries = {}
    for key, p in packages.items():
        files = file_del_pacchetto(p)
        previous = old['crate'].get(key, {})
        entry = {'nome': p['name'], 'versione': p['version'], 'licenza': p['license'],
                 'repository': p['repository'], 'autori': p['authors'], 'nostro': p['nostro'], 'avvisi': []}
        for name, data in files.items():
            dest = TESTI / key / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(data)
            entry['avvisi'].append({'file': name, 'sha256': impronta(data), 'origine': 'pacchetto'})
        # I supplementari convivono coi testi del pacchetto (un COPYING di
        # soli rinvii non basta) e si tengono solo verificati: impronta dei
        # byte conservati e provenienza (blob git ricalcolato offline), come
        # al gate. Un testo che non regge non si trascina nel registro nuovo.
        for item in previous.get('avvisi', []):
            if item.get('origine') == 'pacchetto':
                continue
            data = (TESTI / key / item['file']).read_bytes().replace(b'\r\n', b'\n')
            if impronta(data) != item['sha256']:
                raise ValueError('testo supplementare alterato: ' + key)
            errori_provenienza(key, item, data)
            entry['avvisi'].append(item)
        if not files:
            if not entry['avvisi']:
                vcs = Path(p['manifest_path']).parent / '.cargo_vcs_info.json'
                entry['mancante'] = {
                    'ragione': ('Licenza di famiglia source-available Plenora (indicazione del titolare): testo non presente nel checkout. Nessuna licenza alternativa assegnata; occorre il testo autorevole in LICENSE.' if p['nostro'] else 'Nessun testo nel pacchetto Cargo in cache; rete non usata. Occorre recuperare e verificare i testi upstream prima del rilascio.'),
                    'vcs': strict_json.loads(vcs.read_text()) if vcs.exists() else None,
                }
        entries[key] = entry
    REGISTRO.parent.mkdir(parents=True, exist_ok=True)
    REGISTRO.write_bytes(json_bytes({'schema_version': 1, 'profilo': 'release', 'grafi': graphs, 'crate': entries}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--aggiorna', action='store_true')
    args = parser.parse_args()
    try:
        if args.aggiorna:
            aggiorna()
        data = registro()
        verifica(data)
        print(f"Avvisi verificati: {len(data['crate'])} crate, quattro grafi release")
        return 0
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f'Avvisi crate: {error}')
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
