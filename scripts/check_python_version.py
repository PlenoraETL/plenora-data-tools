"""Fonte unica del minor Python: metadata, ABI, workflow e riferimenti propri."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def declared(root=ROOT):
    value = (root / '.python-version').read_text('utf-8').strip()
    if not re.fullmatch(r'3\.\d+', value):
        raise ValueError('.python-version deve dichiarare un solo minor')
    return value


def reference_errors(path, source, version):
    errors = []
    patterns = (
        r'(?i)python(?:[\s*:=><~!"\x27_-]|version|language|minimo|requires|\|){0,35}(3[._]\d+)(?:\.\d+)?',
        r'(?i)(?:cp|abi3-py)(3\d{1,2})\b',
    )
    for number, line in enumerate(source.splitlines(), 1):
        for pattern in patterns:
            for match in re.finditer(pattern, line):
                raw = match[1].replace('_', '.')
                actual = raw if '.' in raw else raw[0] + '.' + raw[1:]
                if actual != version:
                    errors.append(f'{path}:{number}: riferimento Python divergente')
        if re.search(r'(?:from|import)\s+(?:tomli|typing_extensions)\b', line):
            errors.append(f'{path}:{number}: import di compatibilita obsoleto')
    return errors


def wheel_errors(version, requires_python, tags, interpreter):
    errors = []
    if requires_python != '>=' + version:
        errors.append('Requires-Python del wheel divergente')
    prefix = 'cp' + version.replace('.', '') + '-abi3-'
    if not tags or any(not tag.startswith(prefix) for tag in tags):
        errors.append('ABI del wheel divergente')
    if tuple(map(int, version.split('.'))) != tuple(interpreter[:2]):
        errors.append('interprete di qualifica diverso dal minor dichiarato')
    return errors


def workflow_errors(source):
    errors = []
    # I workflow propri hanno passi canonici a sei spazi: una forma diversa
    # del setup viene rifiutata dal conteggio, non ignorata.
    setups = re.findall(r'^      - uses: actions/setup-python@[^\n]*\n(.*?)(?=^      -|\Z)', source, re.M | re.S)
    if len(setups) != source.count('uses: actions/setup-python@'):
        errors.append('setup-python non canonico')
    for block in setups:
        if not re.search(r"^          python-version-file: ['\"]?\.python-version['\"]?\s*$", block, re.M):
            errors.append('setup-python deve leggere .python-version')
    if re.search(r'^ {8,}(?:python-version|python):', source, re.M):
        errors.append('versione o matrice Python duplicata nel workflow')
    if 'PyO3/maturin-action@' in source:
        if source.count("--interpreter ${{ runner.os == 'Windows' && 'python' || format('python{0}', env.PYTHON_WHEEL) }}") != source.count('PyO3/maturin-action@'):
            errors.append('maturin senza interprete esplicito')
        if 'PYTHON_WHEEL=$(cat .python-version)' not in source:
            errors.append('interprete wheel non derivato dalla fonte')
    return errors


def check(root=ROOT, runtime=False):
    version = declared(root)
    errors = []
    if runtime and tuple(map(int, version.split('.'))) != sys.version_info[:2]:
        errors.append('la guardia richiede il minor Python dichiarato')
    project = tomllib.loads((root / 'crates/plenora-data-py/pyproject.toml').read_text('utf-8'))['project']
    if project['requires-python'] != '>=' + version:
        errors.append('requires-python divergente')
    classifiers = {c.rsplit(' :: ', 1)[-1] for c in project['classifiers'] if re.search(r'Python :: 3\.', c)}
    if classifiers != {version}:
        errors.append('classificatori Python divergenti')
    cargo = tomllib.loads((root / 'Cargo.toml').read_text('utf-8'))
    features = cargo['workspace']['dependencies']['pyo3']['features']
    if {f for f in features if f.startswith('abi3-py')} != {'abi3-py' + version.replace('.', '')}:
        errors.append('ABI PyO3 divergente')
    history = json.loads((root / 'scripts/python_historical_evidence.json').read_text('utf-8'))
    names = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard'], cwd=root).decode().splitlines()
    for name in sorted(set(names)):
        if name.startswith(('vendor/', 'assurance/licenze-crate/', '.ci/plenora-dependencies/', '.plenora-contracts/')):
            continue
        path = root / name
        if path.suffix not in {'.py', '.pyi', '.rs', '.md', '.toml', '.yml', '.yaml', '.txt', '.sh'}:
            continue
        source = path.read_text('utf-8')
        if name in history:
            if hashlib.sha256(source.encode()).hexdigest() != history[name]:
                errors.append(f'{name}: evidenza storica modificata')
            continue
        errors.extend(reference_errors(name, source, version))
        if name.startswith('.github/workflows/'):
            errors.extend(f'{name}: {e}' for e in workflow_errors(source))
    return errors


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', action='store_true')
    failures = check(runtime=parser.parse_args().runtime)
    for failure in failures:
        print(failure)
    print(f'python-policy: {len(failures)} violazioni')
    raise SystemExit(bool(failures))
