"""Token Rust e delimitatori per la guardia degli esiti, senza dipendenze.

I letterali sono token opachi; commenti (anche annidati) vengono scartati.
Non espande macro e non risolve import dinamici: i limiti sono dichiarati.
"""

from dataclasses import dataclass
from functools import lru_cache


@dataclass(frozen=True)
class Token:
    value: str
    offset: int


@lru_cache(maxsize=512)
def tokenize(source):
    tokens = []
    i = 0
    while i < len(source):
        start = i
        c = source[i]
        if c.isspace():
            i += 1
            continue
        if source.startswith('//', i):
            end = source.find('\n', i)
            i = len(source) if end < 0 else end
            continue
        if source.startswith('/*', i):
            depth = 1
            i += 2
            while i < len(source) and depth:
                if source.startswith('/*', i):
                    depth += 1
                    i += 2
                elif source.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            if depth:
                raise ValueError('commento Rust non terminato')
            continue
        # Raw strings: r, br e cr; r#ident resta un identificatore.
        prefix = next((p for p in ('br', 'cr', 'r') if source.startswith(p, i)), '')
        j = i + len(prefix)
        while prefix and j < len(source) and source[j] == '#':
            j += 1
        if prefix and j < len(source) and source[j] == '"':
            stop = '"' + '#' * (j - i - len(prefix))
            end = source.find(stop, j + 1)
            if end < 0:
                raise ValueError('raw string Rust non terminata')
            i = end + len(stop)
            tokens.append(Token(source[start:i], start))
            continue
        j = i + (1 if c in 'bc' and source[i + 1:i + 2] in ('"', "'") else 0)
        if source[j:j + 1] in ('"', "'"):
            quote = source[j]
            k = j + 1
            while k < len(source):
                if source[k] == '\\':
                    k += 2
                elif source[k] == quote:
                    break
                elif source[k] == '\n' and quote == "'":
                    break
                else:
                    k += 1
            # Apostrofo + identificatore senza chiusura: lifetime/label.
            if quote == '"' or (k < len(source) and source[k] == "'" and
                                (k == j + 2 or source[j + 1:j + 2] == '\\')):
                if k >= len(source):
                    raise ValueError('stringa Rust non terminata')
                i = k + 1
                tokens.append(Token(source[start:i], start))
                continue
        if c.isalpha() or c == '_':
            i += 1
            if source[start:i + 1] == 'r#':
                i += 1
            while i < len(source) and (source[i].isalnum() or source[i] == '_'):
                i += 1
            identifier = source[start:i]
            tokens.append(Token(identifier if identifier == 'r#if' else identifier.removeprefix('r#'), start))
        else:
            tokens.append(Token(c, i))
            i += 1
    return tokens


def pairs(tokens):
    stack, result = [], {}
    for i, token in enumerate(tokens):
        v = token.value
        if v in ('(', '[', '{'):
            stack.append(i)
        elif v in (')', ']', '}'):
            if not stack or tokens[stack[-1]].value != {')': '(', ']': '[', '}': '{'}[v]:
                raise ValueError('delimitatori Rust non bilanciati')
            start = stack.pop()
            result[start] = i
    if stack:
        raise ValueError('delimitatore Rust non terminato')
    return result


def aliases(tokens, symbol="matches"):
    """Espande gli alberi use di std/core, compresi gruppi annidati."""
    values = [t.value for t in tokens]
    names = {symbol}
    targets = ([root, 'matches'] for root in ('std', 'core')) if symbol == 'matches' else ([root, 'result', 'Result', 'Err'] for root in ('std', 'core'))
    targets = list(targets)

    def walk(start, end, prefix):
        path = list(prefix)
        i = start
        while i < end:
            v = values[i]
            if v == '{':
                stop = brackets[i]
                walk(i + 1, stop, path)
                i = stop + 1
            elif v == ',':
                path = list(prefix)
                i += 1
            elif v == 'as':
                target = path[:-1] if path[-1:] == ['self'] else path
                if target in targets:
                    names.add(values[i + 1])
                i += 2
            elif v == ':':
                i += 1
            else:
                path.append(v)
                i += 1

    brackets = pairs(tokens)
    for i, v in enumerate(values):
        if v == 'use':
            end = next((j for j in range(i + 1, len(values)) if values[j] == ';'), len(values))
            walk(i + 1, end, [])
    return names


def test_ranges(tokens, integration):
    if integration:
        return [(0, len(tokens))]
    brackets = pairs(tokens)
    values = [t.value for t in tokens]
    ranges = []
    for i in range(len(tokens) - 2):
        if values[i:i + 2] != ['#', '[']:
            continue
        end = brackets[i + 1]
        if values[i + 2:end] not in (['cfg', '(', 'test', ')'], ['test']):
            continue
        j = end + 1
        while j < len(tokens):
            if values[j] == ';':
                break
            if values[j] == '{':
                ranges.append((j, brackets[j] + 1))
                break
            if values[j] in ('[', '('):
                j = brackets[j]
            j += 1
    return ranges


def matches_findings(source, integration):
    tokens = tokenize(source)
    brackets = pairs(tokens)
    names = aliases(tokens)
    errors = aliases(tokens, "Err")
    ranges = test_ranges(tokens, integration)
    values = [t.value for t in tokens]
    result = []
    for i in range(len(tokens) - 2):
        if values[i] not in names or values[i + 1] != '!' or i + 2 not in brackets:
            continue
        if not any(a <= i < b for a, b in ranges):
            continue
        end = brackets[i + 2]
        j = i + 3
        angles = 0
        while j < end:
            if values[j] == '<' and (angles or values[max(0, j - 2):j] == [':', ':'] or j == i + 3):
                angles += 1
            elif values[j] == '>' and angles and values[j - 1] != '-':
                angles -= 1
            elif values[j] == ',' and not angles:
                break
            j = brackets.get(j, j) + 1
        pattern = []
        for k in range(j + 1, end):
            if values[k] == 'if':
                break
            pattern.append(values[k])
        arguments = []
        for k, value in enumerate(pattern):
            if value not in errors:
                continue
            start = k + 1
            # Anche il costruttore nel pattern ammette il turbofish.
            if pattern[start:start + 3] == [':', ':', '<']:
                start += 3
                depth = 1
                while start < len(pattern) and depth:
                    if pattern[start] == '<':
                        depth += 1
                    elif pattern[start] == '>' and pattern[start - 1] != '-':
                        depth -= 1
                    start += 1
            if pattern[start:start + 1] == ['(']:
                arguments.append(start + 1)
        has_err = bool(arguments)
        wildcard = any(pattern[k:k + 2] == ['_', ')']
                       or pattern[k:k + 3] == ['_', ',', ')']
                       for k in arguments)
        rules = []
        if wildcard:
            rules.append('rifiuto_jolly')
        if has_err and '|' in (pattern[1:] if pattern[:1] == ['|'] else pattern):
            rules.append('rifiuti_alternativi')
        for rule in rules:
            result.append((tokens[i].offset, rule))
    return result


def external_test_modules(source, integration):
    """Moduli esterni, attributi path e directory dei moduli inline."""
    tokens = tokenize(source)
    brackets = pairs(tokens)
    values = [t.value for t in tokens]
    ranges = test_ranges(tokens, integration)
    reverse = {end: start for start, end in brackets.items()}
    modules = []
    for i in range(len(tokens) - 2):
        if values[i] != 'mod' or values[i + 2] != ';':
            continue
        j = i - 1
        if j in reverse and values[j] == ')' and values[reverse[j] - 1] == 'pub':
            j = reverse[j] - 2
        elif j >= 0 and values[j] == 'pub':
            j -= 1
        attributes = []
        while j in reverse and values[j] == ']':
            start = reverse[j]
            if start == 0 or values[start - 1] != '#':
                break
            attributes.append(values[start + 1:j])
            j = start - 2
        is_test = any(a <= i < b for a, b in ranges) or ['cfg', '(', 'test', ')'] in attributes
        if not is_test:
            continue
        explicit = next((a[2] for a in attributes if len(a) == 3 and a[:2] == ['path', '=']), None)
        if explicit is not None:
            if not (explicit.startswith('"') and explicit.endswith('"')) or '\\' in explicit:
                raise ValueError('attributo path Rust non supportato dalla guardia')
            explicit = explicit[1:-1]
        inline = [values[k + 1] for k in range(i - 2) if values[k] == 'mod'
                  and values[k + 2] == '{' and brackets[k + 2] > i]
        modules.append((values[i + 1], explicit, inline))
    return modules


def unprotected_test_entries(source):
    """Ingressi test che erediterebbero l'esenzione di produzione.

    Gli import non introducono un corpo. Un ingresso dentro un altro corpo
    test eredita il deny del padre, verificato separatamente.
    """
    tokens = tokenize(source)
    brackets = pairs(tokens)
    values = [t.value for t in tokens]
    ranges = test_ranges(tokens, False)
    found = []
    i = 0
    while i < len(tokens) - 2:
        if values[i:i + 2] != ['#', '[']:
            i += 1
            continue
        first, attributes = i, []
        while values[i:i + 2] == ['#', '[']:
            end = brackets[i + 1]
            attributes.append(values[i + 2:end])
            i = end + 1
        if not any(a in (['test'], ['cfg', '(', 'test', ')']) for a in attributes):
            continue
        item = i
        if values[item:item + 1] == ['pub']:
            item += 1
            if values[item:item + 1] == ['(']:
                item = brackets[item] + 1
        if values[item:item + 1] not in (['fn'], ['mod'], ['async'], ['impl'], ['const'], ['unsafe']):
            continue
        if any(a <= first < b for a, b in ranges):
            continue
        if not any(a[:2] == ['deny', '('] and 'disallowed_methods' in a for a in attributes):
            found.append(tokens[first].offset)
    return found


def has_method_exemption(source):
    """Attributo di lint effettivo, indipendente dal testo della motivazione."""
    tokens = tokenize(source)
    brackets = pairs(tokens)
    values = [t.value for t in tokens]
    for i in range(len(tokens) - 3):
        if values[i:i + 3] == ['#', '!', '[']:
            attr = values[i + 3:brackets[i + 2]]
            if attr[:2] == ['allow', '('] and 'disallowed_methods' in attr:
                return True
    return False


def residual_candidates(source):
    """Regole residue sui token: nessuna dipendenza dalla formattazione."""
    tokens = tokenize(source)
    values = [t.value for t in tokens]
    brackets = pairs(tokens)
    errors = aliases(tokens, 'Err')
    found = []
    for i, value in enumerate(values):
        if value in errors and values[i + 1:i + 2] == ['(']:
            end = brackets[i + 1]
            binding = values[i + 2:end]
            if binding[:1] == ['ref']:
                binding = binding[1:]
            tail = values[end + 1:end + 5]
            if (len(binding) == 1 and binding[0].isidentifier()
                    and tail[:2] == ['=', '>']
                    and (tail[2:3] == ['continue'] or tail[2:] == ['{', 'continue'])):
                found.append((tokens[i].offset, 'errore_ignorato'))
        if not value.isidentifier():
            continue
        end = i + 1
        if values[end:end + 4] == ['.', 'to_string', '(', ')']:
            end += 4
        elif value not in {'messaggio', 'message', 'testo', 'msg'}:
            continue
        if values[end:end + 3] == ['.', 'contains', '(']:
            found.append((tokens[i].offset, 'solo_messaggio'))
    return found
