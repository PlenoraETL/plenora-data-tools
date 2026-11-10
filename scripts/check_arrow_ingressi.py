"""Censimento delle firme Rust e delle superfici Python Arrow, fail closed."""
from __future__ import annotations

import inspect
import ast
import hashlib
import json
from pathlib import Path
import re
from functools import lru_cache
from genera_arrow_items import type_names

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "scripts" / "arrow_ingressi.json"
ARROW_PATH = re.compile(r'\b(?:arrow\w*|parquet)(?:::|\s+as\b)')


@lru_cache(maxsize=1)
def arrow_pattern():
    """Sovrainsieme nominale dai sorgenti upstream, senza lista manuale."""
    return re.compile(r'\b(?:' + '|'.join(re.escape(n) for n in sorted(type_names())) + r')\b')


def arguments_opening(text, start):
    """La prima parentesi fuori dai generici, inclusi i bound Fn(...)."""
    generics = 0
    for pos in range(start, len(text)):
        char = text[pos]
        if char == '(' and generics == 0:
            return pos
        if char == '<':
            generics += 1
        elif char == '>' and text[pos - 1] != '-':
            generics -= 1
    raise AssertionError('firma Rust senza parametri')


def impl_bounds(header):
    """Bound completi anche con Fn(A, B), lifetime e argomenti annidati."""
    for match in re.finditer(r'\bimpl\s+', header):
        level = 0
        end = match.end()
        while end < len(header):
            char = header[end]
            if level == 0 and char in ',)>;{' and not (char == '>' and header[end - 1] == '-'):
                break
            if char in '(<[':
                level += 1
            elif char in ')>]':
                if char != '>' or header[end - 1] != '-':
                    level -= 1
            end += 1
        yield header[match.start():end]


def rust_forbidden(source):
    """Forme non risolte: rifiuto conservativo, mai inferenza degli alias."""
    clean = production_rust(source)
    forbidden = set()
    imported = set()
    for use in re.finditer(r'\buse\s+[^;]+;', clean):
        if ARROW_PATH.search(use[0]):
            imported.update(re.findall(r'\b[A-Z]\w*\b', use[0]))
    imported_pattern = '|'.join(re.escape(name) for name in sorted(imported)) or r'(?!)'
    for match in re.finditer(r'\buse\s+[^;]+;|\btype\s+\w+[^;]*;', clean):
        item = match[0]
        if ((item.startswith('use') and re.search(r'\bas\b', item)) or
                (item.startswith('type') and '=' in item)):
            target = item.split('=', 1)[1] if item.startswith('type') else item
            if ARROW_PATH.search(target) or arrow_pattern().search(target) or re.search(rf'\b(?:{imported_pattern})\b', target):
                forbidden.add(' '.join(item.split()))
    # Include i vincoli del contenitore: un metodo puo' usare solo &T.
    # Il divieto conservativo vale anche per header privati e trait locali.
    for match in re.finditer(r'\b(?:impl|fn|trait|struct|enum)\b[^{};]*', clean):
        header = match[0]
        trait = rf'(?:{arrow_pattern().pattern}|{imported_pattern})'
        arrow_trait_path = rf'(?:(?:\w+::)*{trait}\b|(?:\w+::)*(?:arrow\w*|parquet)::\w+(?:::\w+)*)'
        bound = rf'(?:(?<!:):(?!:)\s*|\+\s*|\bimpl\s+)(?:for\s*<[^>]*>\s*)?\??\s*{arrow_trait_path}'
        # I parametri ordinari (&dyn Array compreso) non sono generici.
        generic = header[:arguments_opening(header, 3)] if header.startswith('fn ') else header
        where = header.split('where', 1)[1] if 'where' in header else ''
        if generic.startswith('impl '):
            generic = generic[5:]
        opaque = ' '.join(impl_bounds(header)) if header.startswith('fn ') else ''
        if re.search(bound, generic + ' ' + where + ' ' + opaque):
            forbidden.add(' '.join(header.split()))
    return forbidden


def production_rust(source):
    """Esclude i moduli inline esplicitamente marcati cfg(test)."""
    clean = masked(source)
    for match in list(re.finditer(r'#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?mod\s+\w+\s*\{', clean))[::-1]:
        depth, end = 1, match.end()
        while depth:
            depth += (clean[end] == '{') - (clean[end] == '}')
            end += 1
        clean = clean[:match.start()] + ' ' * (end - match.start()) + clean[end:]
    return clean


def python_source_functions(source):
    """Definizioni pubbliche a livello modulo/classe, anche sotto if/try.

    Non visita i corpi delle funzioni: le closure non sono API pubbliche.
    Nessun filtro per parametri, annotazioni o uso di Arrow.
    """
    result = {}
    def visit(node, prefix=''):
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if not child.name.startswith('_') or child.name in {'__init__', '__arrow_c_stream__', '__arrow_c_array__'}:
                    result[prefix + child.name] = ast.unparse(child.args)
            elif isinstance(child, ast.ClassDef):
                visit(child, prefix + child.name + '.')
            else:
                visit(child, prefix)
    visit(ast.parse(source))
    return result


def python_source_inventory():
    base = ROOT / 'crates/plenora-data-py/python/plenora_data'
    result = {}
    for path in sorted([*base.rglob('*.py'), *base.rglob('*.pyi')]):
        for name, signature in python_source_functions(path.read_text(encoding='utf-8')).items():
            result[f'{path.relative_to(ROOT).as_posix()}::{name}'] = signature
    # Le pyfunction Rust vengono censite anche se lo stub non e' aggiornato.
    for path in sorted((ROOT / 'crates/plenora-data-py/src').rglob('*.rs')):
        source = masked(path.read_text(encoding='utf-8'))
        for match in re.finditer(r'#\[pyfunction[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?(?:const\s+)?fn\s+(\w+)', source):
            result[f'{path.relative_to(ROOT).as_posix()}::{match[1]}'] = 'pyfunction'
        for match in re.finditer(r'#\[pymethods\]\s*impl\s+(\w+)\s*\{', source):
            depth, end = 1, match.end()
            while depth:
                depth += (source[end] == '{') - (source[end] == '}')
                end += 1
            for method in re.finditer(r'\bfn\s+(\w+)', source[match.end():end]):
                result[f'{path.relative_to(ROOT).as_posix()}::{match[1]}.{method[1]}'] = 'pymethod'
    return result


def check_python_sources():
    actual = python_source_inventory()
    registered = json.loads(REGISTRY.read_text(encoding='utf-8'))['python_sources']
    check_members(actual, registered)
    for name, signature in actual.items():
        entry = registered[name]
        assert entry['signature'] == signature, name
        assert entry['reason'], name
        assert entry['classification'] in {'ingresso guardato', 'non riceve dati Arrow esterni', 'helper con precondizione dichiarata'}, name
        if entry['classification'] == 'helper con precondizione dichiarata':
            assert entry['precondition'], name
    return len(actual)


def masked(source):
    """Nasconde commenti e stringhe, preservando posizioni e lifetime Rust."""
    pattern = r"//[^\n]*|/\*[\s\S]*?\*/|r(?P<hash>\#*)\"[\s\S]*?\"(?P=hash)|\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\\n])'"
    return re.sub(pattern,
                  lambda m: ''.join('\n' if c == '\n' else ' ' for c in m[0]), source, flags=re.DOTALL)



def split_args(text):
    parts, start, level = [], 0, 0
    for i, char in enumerate(text):
        if char in '(<[':
            level += 1
        elif char in ')>]':
            if char != '>' or i == 0 or text[i - 1] != '-':
                level -= 1
        elif char == ',' and level == 0:
            parts.append(text[start:i].strip())
            start = i + 1
    parts.append(text[start:].strip())
    return [part for part in parts if part]


def attached_docs(source, start):
    """Solo doc comment e attributi contigui della funzione corrente."""
    lines = source[:start].splitlines()
    docs = []
    index = len(lines) - 1
    while index >= 0:
        line = lines[index].strip()
        if not line:
            index -= 1
            continue
        if line.startswith('///'):
            docs.append(line)
            index -= 1
        elif line.endswith(']'):
            # Gli attributi Rust possono occupare piu' righe.
            while index >= 0 and not lines[index].lstrip().startswith('#['):
                index -= 1
            index -= 1
        else:
            break
    return '\n'.join(reversed(docs))


def rust_functions(source):
    clean = production_rust(source)
    # Ogni impl di trait e' inclusa conservativamente: anche i trait importati
    # sono pubblici senza una dichiarazione locale. Esclude le closure e fn locali.
    containers = []
    for item in re.finditer(r'\b(?:pub\s+(?:(?:unsafe|auto)\s+)*trait|impl)\b[^{};]*\{', clean):
        header = item[0]
        if not header.startswith('pub ') and not re.search(r'\bfor\b', header):
            continue
        depth, close = 1, item.end()
        while depth:
            depth += (clean[close] == '{') - (clean[close] == '}')
            close += 1
        containers.append((item.end(), close - 1))
    result = {}
    counts = {}
    for match in re.finditer(r'\b(?:(pub)\s+)?(?:(?:async|const|unsafe|extern)\s+)*fn\s+(\w+)', clean):
        implicit = any(start <= match.start() < stop and
                       clean[start:match.start()].count('{') == clean[start:match.start()].count('}')
                       for start, stop in containers)
        if not match[1] and not implicit:
            continue
        name = match[2]
        opening = arguments_opening(clean, match.end())
        depth, end = 1, opening + 1
        while depth:
            depth += (clean[end] == '(') - (clean[end] == ')')
            end += 1
        body_match = re.search(r'[;{]', clean[end:])
        assert body_match is not None, 'firma Rust senza terminatore'
        body = end + body_match.start()
        signature = source[match.start():body].strip()
        depth, close = int(clean[body] == '{'), body + 1
        while depth:
            depth += (clean[close] == '{') - (clean[close] == '}')
            close += 1
        counts[name] = counts.get(name, 0) + 1
        key = name if counts[name] == 1 else f'{name}#{counts[name]}'
        result[key] = dict(signature=' '.join(signature.split()),
                           args=split_args(source[opening + 1:end - 1]),
                           returns=source[end:body].strip(), body=source[body+1:close-1],
                           opening=body, start=match.start(),
                           docs=attached_docs(source, match.start()))
    return result


def rust_inventory():
    result = {}
    import tomllib
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']
    paths = set()
    for member in workspace['members']:
        for crate in ROOT.glob(member):
            paths.update(p for p in crate.rglob('*.rs')
                         if not {'tests', 'vendor', 'target'}.intersection(p.relative_to(crate).parts)
                         and not p.name.startswith('test'))
    exceptions = json.loads(REGISTRY.read_text(encoding='utf-8')).get('rust_variants', {})
    found_variants = set()
    for path in sorted(paths):
        source = path.read_text(encoding='utf-8')
        for variant in rust_forbidden(source):
            key = f'{path.relative_to(ROOT).as_posix()}::{variant}'
            assert key in exceptions, f'forma Rust vietata non classificata: {key}'
            assert exceptions[key]['reason'], key
            assert exceptions[key]['classification'] in {'ingresso guardato', 'non riceve dati Arrow esterni', 'helper con precondizione dichiarata'}, key
            found_variants.add(key)
        if 'src' not in path.relative_to(ROOT).parts:
            continue
        for name, item in rust_functions(source).items():
            result[f'{path.relative_to(ROOT).as_posix()}::{name}'] = item
    check_members(found_variants, exceptions)
    return result


def python_inventory(module):
    """Tutte le routine pubbliche: include firme senza annotazioni native.

    Il sovrainsieme impedisce che un nuovo ingest(table) non annotato sfugga.
    Include metodi dichiarati o ereditati dal pacchetto, costruttori e
    protocollo Arrow; esclude i metodi delle classi Python di base.
    """
    result = {}
    for name, value in inspect.getmembers(module):
        if name.startswith('_'):
            continue
        if inspect.isroutine(value):
            result[name] = value
        elif inspect.isclass(value):
            for method, function in inspect.getmembers(value, inspect.isroutine):
                if method not in vars(value) and not str(getattr(function, '__module__', '')).startswith('plenora_data'):
                    continue
                if (not method.startswith('_') or method in {'__init__', '__arrow_c_stream__', '__arrow_c_array__'}) and inspect.isroutine(function):
                    result[f'{name}.{method}'] = function
    return result


def check_members(actual, registered):
    missing = set(actual) - set(registered)
    stale = set(registered) - set(actual)
    if missing or stale:
        raise AssertionError(f'non classificati: {sorted(missing)}; assenti: {sorted(stale)}')


def check_rust():
    registry = json.loads(REGISTRY.read_text(encoding='utf-8'))['rust']
    actual = rust_inventory()
    check_members(actual, registry)
    for name, item in actual.items():
        entry = registry[name]
        assert entry['signature'] == item['signature'], f'firma cambiata: {name}'
        assert entry['reason'], name
        category = entry['classification']
        if category == 'ingresso guardato':
            assert entry['checks'], name
            for check in entry['checks']:
                target, fragment = check['function'], check['contains']
                assert re.sub(r'\s+', '', fragment) in re.sub(r'\s+', '', actual[target]['body']), (name, target, fragment)
            if 'implementation_sha256' in entry:
                digest = hashlib.sha256(item['body'].encode()).hexdigest()
                assert digest == entry['implementation_sha256'], f'confine cambiato senza revisione: {name}'
            if not name.startswith('crates/plenora-core/src/valida_arrow.rs::'):
                prefix = entry.get('prefix', ''.join(c['contains'] for c in entry['checks']))
                assert isinstance(prefix, str) and prefix, name
                assert re.sub(r'\s+', '', masked(item['body'])).startswith(re.sub(r'\s+', '', masked(prefix))), f'guardia tardiva: {name}'
        elif category == 'helper con precondizione dichiarata':
            assert 'Precondizione Arrow' in item['docs'], name
            assert entry['limit'] == 'docs/limiti.md#helper-arrow-con-precondizione', name
        else:
            assert category == 'non riceve dati esterni', name
            # Anche Field e gli altri item upstream richiedono una decisione
            # esplicita nel registro, non un elenco manuale di tipi innocui.
            names = sorted(set(arrow_pattern().findall(' '.join(item['args']))))
            assert entry['arrow_arguments'] == names, f'tipi Arrow da riclassificare: {name}'
            assert not any(re.search(r'\bIngresso\b', arg) for arg in item['args']) or name.startswith('crates/plenora-pipeline/src/budget.rs::'), name
    return len(actual)


if __name__ == '__main__':
    print(f'{check_rust()} firme Rust Arrow classificate e verificate')
    print(f'{check_python_sources()} definizioni Python/native classificate e verificate')

