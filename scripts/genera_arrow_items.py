"""Catalogo conservativo degli item pubblici Arrow/Parquet dei sorgenti nel lock.

Rigenera: python scripts/genera_arrow_items.py
Verifica CI (dopo cargo fetch --locked): python scripts/genera_arrow_items.py --check
Non espande macro o re-export: ambito in docs/limiti.md.
"""
import argparse
import json
import os
from pathlib import Path
import re
try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10: solo le guardie (>=3.11) leggono i manifest
    tomllib = None


def _toml():
    if tomllib is None:
        raise RuntimeError('lettura dei manifest Cargo: serve Python >= 3.11 (tomllib)')
    return tomllib

ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / 'scripts/arrow_items.json'


def packages():
    lock = _toml().loads((ROOT / 'Cargo.lock').read_text(encoding='utf-8'))
    return sorted((p['name'], p['version']) for p in lock['package']
                  if p['name'].startswith('arrow') or p['name'] in {'parquet', 'plenora-parquet'})


def generate():
    from check_arrow_ingressi import masked
    cache = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo')) / 'registry/src'
    result = []
    for name, version in packages():
        candidates = list(cache.glob(f'*/{name}-{version}'))
        if name == 'plenora-parquet':
            workspace = _toml().loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))
            candidates = [ROOT / workspace['workspace']['dependencies']['parquet']['path']]
        assert candidates, f'sorgenti Cargo mancanti: {name} {version}; eseguire cargo fetch --locked'
        variants = []
        for base in candidates:
            manifest = _toml().loads((base / 'Cargo.toml').read_text(encoding='utf-8'))
            assert (manifest['package']['name'], manifest['package']['version']) == (name, version)
            items = set()
            for path in sorted((base / 'src').rglob('*.rs')):
                clean = masked(path.read_text(encoding='utf-8'))
                for match in re.finditer(
                        r'\bpub\s+(?:(?:unsafe|async|const|extern)\s+)*'
                        r'(struct|enum|union|trait|type|fn|mod|const|static)\s+(\w+)', clean):
                    items.add((match[1], match[2]))
            assert items, f'catalogo vuoto: {name}'
            variants.append(sorted(items))
        assert all(v == variants[0] for v in variants), f'cache discordanti: {name}'
        result.append(dict(name=name, version=version, items=variants[0]))
    return result


def type_names():
    catalog = json.loads(CATALOG.read_text(encoding='utf-8'))
    assert [(p['name'], p['version']) for p in catalog] == packages(), 'catalogo Arrow non allineato al lock'
    return {name for package in catalog for kind, name in package['items']
            if kind in {'struct', 'enum', 'union', 'trait', 'type'}}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    content = json.dumps(generate(), indent=2, ensure_ascii=False) + '\n'
    if args.check:
        assert CATALOG.read_text(encoding='utf-8') == content, 'rigenerare scripts/arrow_items.json'
        print('Catalogo Arrow/Parquet verificato contro i sorgenti del lock')
    else:
        CATALOG.write_text(content, encoding='utf-8')
