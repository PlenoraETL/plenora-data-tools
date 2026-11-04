"""Guardia lessicale chiusa del vocabolario geometrico Rust, senza dipendenze.

Le eccezioni sono contesti di letterali non geometrici, non nomi globalmente
permessi: una nuova occorrenza richiede classificazione. I token strutturali
sono distinti dalle proprieta' C1-C4. Non interpreta il contenuto PROJJSON.
"""
from pathlib import Path
import json
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
POLICY = ROOT / 'crates/plenora-core/src/contract/geometry_metadata_policy.rs'
AUDIT = ROOT / 'scripts/metadati_literal_contexts.json'
WORD = re.compile(r'[A-Za-z_][A-Za-z0-9_.:-]*\Z')


def tokens(source):
    """Commenti annidati, stringhe raw e ordinarie; rifiuta stringhe troncate."""
    result = []
    i = 0
    while i < len(source):
        if source[i].isspace():
            i += 1
        elif source.startswith('//', i):
            end = source.find('\n', i)
            i = len(source) if end < 0 else end
        elif source.startswith('/*', i):
            depth = 1
            i += 2
            while depth:
                if i >= len(source):
                    raise ValueError('commento troncato')
                if source.startswith('/*', i):
                    depth += 1
                    i += 2
                elif source.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
        else:
            raw = re.match(r'(?:b|c)?r(#+)?"', source[i:])
            normal = re.match(r'(?:b|c)?"', source[i:])
            char = re.match(r"'(?:[^'\\\n]|\\.)'", source[i:])
            if raw:
                start = i + raw.end()
                close = '"' + (raw[1] or '')
                end = source.find(close, start)
                if end < 0:
                    raise ValueError('stringa raw troncata')
                result.append(('string', source[start:end]))
                i = end + len(close)
            elif normal:
                start = i + normal.end() - 1
                end = start + 1
                while end < len(source):
                    if source[end] == '\\':
                        end += 2
                    elif source[end] == '"':
                        break
                    else:
                        end += 1
                if end >= len(source):
                    raise ValueError('stringa troncata')
                value = source[start + 1:end]
                # Le chiavi ASCII non richiedono escape. Gli escape in una
                # chiave candidata si rifiutano invece di reinterpretarli.
                result.append(('string', value))
                i = end + 1
            elif char:
                result.append(('char', char[0]))
                i += char.end()
            else:
                word = re.match(r'[A-Za-z_][A-Za-z_0-9]*', source[i:])
                value = word[0] if word else source[i]
                result.append(('token', value))
                i += len(value)
    return result


def production(items):
    """Esclude solo l'item marcato cfg(test), conservando il codice successivo."""
    result = []
    i = 0
    marker = ['#', '[', 'cfg', '(', 'test', ')', ']']
    while i < len(items):
        if [v for _, v in items[i:i + 7]] == marker:
            i += 7
            while i < len(items) and items[i][1] not in ('{', ';'):
                i += 1
            if i < len(items) and items[i][1] == '{':
                depth = 1
                i += 1
                while i < len(items) and depth:
                    if items[i] == ('token', '{'):
                        depth += 1
                    elif items[i] == ('token', '}'):
                        depth -= 1
                    i += 1
            else:
                i += 1
        else:
            result.append(items[i])
            i += 1
    return result


def vocabulary():
    source = POLICY.read_text(encoding='utf-8')
    keys = source.split('pub const KEYS:', 1)[1].split('];', 1)[0]
    structure = source.split('pub const STRUCTURAL_MEMBERS:', 1)[1].split('];', 1)[0]
    return {v for kind, v in tokens(keys + structure) if kind == 'string'}


def candidates(source):
    items = production(tokens(source))
    geometric = any(v in ('geo', 'ARROW:extension:metadata', 'GEO_METADATA_KEY',
                         'GEOARROW_EXTENSION_METADATA_KEY', 'PLENORA_GEOMETRY_NAMESPACE_PREFIX',
                         'PLENORA_NAMESPACE_PREFIX') or v.startswith(('geometry.', 'plenora.geometry.')) for _, v in items)
    found = []
    for i, (kind, value) in enumerate(items):
        if kind != 'string':
            continue
        namespace = value.startswith('plenora.geometry.')
        before = [v for _, v in items[max(0, i - 15):i]]
        sink = (len(before) >= 2 and before[-1] == '(' and before[-2] in
                ('get', 'get_mut', 'insert', 'remove', 'entry', 'contains_key')) or (before and before[-1] == '[')
        constant = 'const' in before and '=' in before
        json_member = i + 1 < len(items) and items[i + 1] == ('token', ':')
        if namespace and '\\' in value:
            found.append((value, 'chiave geometrica'))
            continue
        if namespace and value.endswith('.'):
            if [v for _, v in items[max(0, i - 3):i]] == ['.', 'starts_with', '(']:
                continue
            if 'PLENORA_GEOMETRY_NAMESPACE_PREFIX' in before and 'const' in before:
                continue
            found.append((value, 'prefisso costruito fuori da starts_with'))
        elif namespace and (WORD.fullmatch(value) or '{' in value):
            found.append((value, 'chiave geometrica'))
        elif (geometric or sink or constant or json_member) and (WORD.fullmatch(value) or sink or json_member):
            context = json.dumps(items[max(0, i - 5):i + 6], ensure_ascii=True, separators=(',', ':'))
            found.append((value, context))
    # Una costruzione non risolvibile non si considera una chiave nota.
    # I format diagnostici sono contesti auditati separatamente.
    if geometric:
        for i, item in enumerate(items):
            method = (item[1] in ('concat', 'join', 'push_str') and i > 0 and
                      items[i - 1] == ('token', '.'))
            addition = (item == ('token', '+') and i > 0 and i + 1 < len(items) and
                        (items[i - 1][0] == 'string' or items[i + 1][0] == 'string'))
            if method or addition:
                context = json.dumps(items[max(0, i - 8):i + 9], ensure_ascii=True, separators=(',', ':'))
                found.append(('costruzione dinamica ' + item[1], context))
    for i in range(len(items) - 3):
        if items[i][1] in ('concat', 'format') and [v for _, v in items[i + 1:i + 3]] == ['!', '(']:
            end = i + 3
            depth = 1
            fragments = []
            while end < len(items) and depth:
                if items[end] == ('token', '('):
                    depth += 1
                elif items[end] == ('token', ')'):
                    depth -= 1
                elif items[end][0] == 'string':
                    fragments.append(items[end][1])
                end += 1
            joined = ''.join(fragments).replace('{PLENORA_GEOMETRY_NAMESPACE_PREFIX}', 'plenora.geometry.').replace('{PLENORA_NAMESPACE_PREFIX}', 'plenora.')
            before = [v for _, v in items[max(0, i - 2):i]]
            member = before and (before[-1] == '[' or
                (len(before) == 2 and before[-1] == '(' and before[-2] in
                 ('get', 'get_mut', 'insert', 'remove', 'entry', 'contains_key')))
            if 'geometry' in joined or member or geometric:
                if items[i][1] == 'concat' and WORD.fullmatch(joined) and all('{' not in f for f in fragments):
                    found.append((joined, 'chiave geometrica'))
                else:
                    context = json.dumps(items[i:end], ensure_ascii=True, separators=(',', ':'))
                    found.append((joined, context))

    return found


def errors(source, known, exceptions=()):
    allowed = set(exceptions)
    return [(name, context) for name, context in candidates(source)
            if name not in known and (context in ('chiave geometrica', 'prefisso costruito fuori da starts_with')
                                      or context not in allowed)]


class GeometryMetadataGuard(unittest.TestCase):
    def test_repository(self):
        known = vocabulary()
        audit = json.loads(AUDIT.read_text(encoding='utf-8'))
        failures = []
        for path in sorted((ROOT / 'crates').glob('*/src/**/*.rs')):
            relative = path.relative_to(ROOT).as_posix()
            for name, _ in errors(path.read_text(encoding='utf-8-sig'), known, audit.get(relative, [])):
                failures.append(f'{relative}: {name}')
        self.assertEqual([], failures)
        document = (ROOT / 'docs/metadati-arrow.md').read_text(encoding='utf-8')
        for name in known:
            self.assertIn(f'| `{name}` |', document)

    def test_native_primary_is_provenance_only(self):
        # Le chiavi native sono provenienza: classificate, mai lette per decidere.
        forbidden = {'plenora.geometry.native.geoparquet.primary_column',
                     'plenora.geometry.native.geoparquet.altre_colonne'}
        for path in sorted((ROOT / 'crates').glob('*/src/**/*.rs')):
            if path == POLICY:
                continue
            items = production(tokens(path.read_text(encoding='utf-8-sig')))
            self.assertFalse(forbidden.intersection(v for kind, v in items if kind == 'string'), str(path))

    def test_new_members_and_constructions_fail(self):
        known = vocabulary()
        for source in [
            'let s = "geo"; object.insert("new_member", v);',
            'let s = "ARROW:extension:metadata"; object["new_member"] = v;',
            'let key = format!("plenora.geometry.{suffix}");',
            'let key = concat!("plenora.", "geometry.", "new_member");',
            'let prefix = "plenora.geometry."; let key = format!("{prefix}{suffix}");',
            '#[cfg(test)] mod tests {} fn live() { let key = "plenora.geometry.new_member"; }',
            'let key = "plenora.geometry.\\x66oo";',
            'let key = format!("{}.geometry.{}", "plenora", "new_member");',
            'let p = PLENORA_NAMESPACE_PREFIX; let key = format!("{p}geometry.new_member");',
            'const KEY: &str = concat!("plenora.", concat!("geometry.", "new_member"));',
            'fn update(o: &mut Value) { o["new_member"] = json!(1); }',
            'let s = "geo"; let keys = ["new_member"]; for key in keys { o.get(key); }',
            'let s = "geo"; object.insert(concat!("crs", "epoch"), v);',
            'let s = "geo"; object.insert(format!("{}{}", "crs", "epoch"), v);',
            'let s = "geo"; let key = concat!("crs", "epoch"); object.insert(key, v);',
            'let s = "geo"; let key = format!("{}{}", "crs", "epoch"); object.insert(key, v);',
            'let s = "geo"; let value = json!({"new member": 1});',
            'fn make() -> Value { json!({"new member": 1}) }',
            'let key = format!("{}{}", PLENORA_GEOMETRY_NAMESPACE_PREFIX, "new_member");',
            'let s = "geo"; let key = ["crs", "epoch"].concat();',
            'let s = "geo"; let key = ["crs", "epoch"].join("");',
            'let s = "geo"; let key = String::from("crs") + "epoch";',
            'let s = "geo"; key.push_str("epoch");',
        ]:
            with self.subTest(source=source):
                self.assertTrue(errors(source, known))

    def test_lexer_and_known_keys(self):
        self.assertEqual([], errors('/* "plenora.geometry.fake" */ fn f() { o.get("plenora.geometry.precision"); }', vocabulary()))
        self.assertEqual([], errors('#[cfg(test)] mod tests { let x = "plenora.geometry.fake"; }', vocabulary()))
        self.assertTrue(errors('let x = r#"plenora.geometry.fake"#;', vocabulary()))


if __name__ == '__main__':
    unittest.main()
