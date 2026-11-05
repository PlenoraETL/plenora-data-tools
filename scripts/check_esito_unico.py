#!/usr/bin/env python3
"""Rifiuta controlli deboli degli esiti nelle prove Rust e Python del workspace.

Guardia sintattica conservativa, non certificazione degli oracoli: la scelta
di valori, categoria e fase richiede la verifica dello scenario.
L'allowlist contiene occorrenze esatte e una motivazione, mai interi file.
"""

from __future__ import annotations

import ast
import json
import re
from functools import lru_cache
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Ogni regola ha esempi positivi e negativi enumerati nei test.
PATTERNS = {
    "rifiuto_booleano": ("rust", r"\.\s*is_err\s*\(\s*\)"),
    "errore_ignorato": ("rust", r"\bErr\s*\(\s*(?:ref\s+)?\w+\s*\)\s*=>\s*\{?\s*continue\b"),
    "successo_o_rifiuto": ("rust", r"\.\s*is_ok\s*\(\s*\)\s*\|\|"),
    "confronto_booleano": ("rust_assert", r"\.\s*is_ok\s*\(\s*\)"),
    "rifiuto_jolly": ("rust_macro", r"\bErr\s*\(\s*_\s*\)"),
    "rifiuti_alternativi": ("rust_macro", r"\bErr\s*\([^;]*?(?<!\|)\|(?!\|)"),
    "solo_messaggio": ("rust_message", r"(?:\b\w+\.to_string\(\)|\b(?:messaggio|message|testo|msg))\s*\.contains\s*\("),
    "raises_generico": ("python", "raises senza categoria esatta"),
    "messaggio_python": ("python", "assert frammento in str(errore) senza categoria"),
    "except_ignorato": ("python", "except Exception/BaseException: pass/continue"),
    "zip_python_troncante": ("python", "zip senza strict=True"),
}


def mask(source: str) -> str:
    """Nasconde commenti e letterali mantenendo offset e righe."""
    out = list(source)
    i = 0
    while i < len(source):
        end = i
        if source.startswith("//", i):
            end = source.find("\n", i)
            if end == -1:
                end = len(source)
        elif source.startswith("/*", i):
            end, depth = i + 2, 1
            while end < len(source) and depth:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            if depth:
                raise ValueError("commento Rust non terminato")
        else:
            raw = re.compile(r'(?:br|r)(#*)"').match(source, i)
            if raw:
                stop = '"' + raw[1]
                closing = source.find(stop, i + len(raw[0]))
                if closing == -1:
                    raise ValueError("raw string Rust non terminata")
                end = closing + len(stop)
            elif source[i] == '"':
                end = i + 1
                while end < len(source):
                    if source[end] == "\\":
                        end += 2
                    elif source[end] == '"':
                        end += 1
                        break
                    else:
                        end += 1
            elif source[i] == "'":
                char = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", source[i:])
                if char:
                    end = i + len(char[0])
        if end > i:
            for pos in range(i, end):
                if out[pos] != "\n":
                    out[pos] = " "
            i = end
        else:
            i += 1
    return "".join(out)


def closing(code: str, opening: int, left: str = "{", right: str = "}") -> int:
    depth = 1
    for end in range(opening + 1, len(code)):
        depth += (code[end] == left) - (code[end] == right)
        if not depth:
            return end + 1
    raise ValueError("delimitatore Rust non terminato")


def body_start(code: str, start: int) -> int:
    """Salta firme con array, generici e parentesi prima del corpo."""
    parens = brackets = 0
    for pos in range(start, len(code)):
        char = code[pos]
        if char == "(":
            parens += 1
        elif char == ")":
            parens -= 1
        elif char == "[":
            brackets += 1
        elif char == "]":
            brackets -= 1
        elif not parens and not brackets:
            if char == ";":
                return -1
            if char == "{":
                return pos
    return -1


@lru_cache(maxsize=32)
def functions_in(code: str) -> list[tuple[str, int, int]]:
    result = []
    for fn in re.finditer(r"\bfn\s+(\w+)", code):
        start = body_start(code, fn.end())
        if start >= 0:
            result.append((fn[1], start, closing(code, start)))
    return result


def test_ranges(code: str, integration: bool) -> list[tuple[int, int]]:
    if integration:
        return [(0, len(code))]
    ranges = []
    for mark in re.finditer(r"#\s*\[\s*(?:test|cfg\s*\(\s*test\s*\))\s*\]", code):
        opening = body_start(code, mark.end())
        if opening >= 0:
            ranges.append((opening, closing(code, opening)))
    functions = functions_in(code)
    # Gli helper con asserzioni sono prove anche fuori da cfg(test), e
    # trascinano gli helper chiamati: non dipende dal nome del modulo.
    for _, start, end in functions:
        if re.search(r"\b(?:assert\w*|prop_assert\w*)!", code[start:end]) or re.search(PATTERNS["errore_ignorato"][1], code[start:end]):
            ranges.append((start, end))
    # Anche un helper senza assert (per esempio Err(e) => continue) entra
    # se tutti i suoi chiamanti locali sono prove. I corpi di produzione
    # richiamati anche fuori dalle prove non diventano test per questo.
    while True:
        added = []
        for name, start, end in functions:
            if any(a <= start < b for a, b in ranges):
                continue
            declaration = code[max(0, code.rfind("fn ", 0, start) - 30):start]
            if re.search(r"\bself\b", declaration) or (re.search(r"\bpub(?:\([^)]*\))?\s+(?:async\s+)?fn", declaration) and not re.search(r"->\s*bool\s*$", declaration)):
                continue
            calls = [m.start() for m in re.finditer(r"\b" + name + r"\b", code)
                     if not re.search(r"\bfn\s*$", code[max(0, m.start() - 20):m.start()])]
            if calls and all(any(a <= pos < b for a, b in ranges) for pos in calls):
                added.append((start, end))
        if not added:
            return ranges
        ranges.extend(added)


def python_findings(source: str) -> list[tuple[int, str, str]]:
    tree = ast.parse(source)
    found = []
    def add(node, rule):
        found.append((node.lineno, rule, source.splitlines()[node.lineno - 1].strip()))
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "zip":
            if not any(k.arg == "strict" and isinstance(k.value, ast.Constant) and k.value.value is True for k in node.keywords):
                add(node, "zip_python_troncante")
        if isinstance(node, ast.Call) and ((isinstance(node.func, ast.Attribute) and node.func.attr == "raises") or (isinstance(node.func, ast.Name) and node.func.id == "raises")):
            category = ast.unparse(node.args[0]) if node.args else ""
            # Le sottoclassi SDK identificano una categoria; Exception,
            # PlenoraError e tuple accettano invece esiti differenti.
            if not category or category.split(".")[-1] in {"Exception", "BaseException", "PlenoraError"} or isinstance(node.args[0], ast.Tuple):
                add(node, "raises_generico")
        if isinstance(node, ast.ExceptHandler):
            if node.type is None or ast.unparse(node.type) in {"Exception", "BaseException"}:
                if any(isinstance(n, (ast.Pass, ast.Continue)) for n in node.body):
                    add(node, "except_ignorato")
        if isinstance(node, ast.Assert) and isinstance(node.test, ast.Compare):
            if any(isinstance(op, ast.In) for op in node.test.ops) and any(
                isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "str"
                for n in ast.walk(node.test)
            ):
                # Un controllo sul testo supplementare e' ammesso solo con
                # un controllo esplicito di categoria nello stesso test.
                owner = next((n for n in ast.walk(tree) if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.lineno <= node.lineno <= n.end_lineno), tree)
                context = "\n".join(ast.unparse(n.test) for n in ast.walk(owner) if isinstance(n, ast.Assert))
                values = [ast.unparse(n.args[0]) for n in ast.walk(node.test)
                          if isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "str" and n.args]
                proven = all(
                    re.search(re.escape(value) + r"\.category\s*==", context)
                    or re.search(r"isinstance\(" + re.escape(value) + r", (?:\w+\.)?Plenora(?!Error)\w+", context)
                    or any(isinstance(w, ast.With) and any(
                        item.optional_vars is not None and ast.unparse(item.optional_vars) + ".value" == value
                        and isinstance(item.context_expr, ast.Call) and item.context_expr.args
                        and re.fullmatch(r"(?:\w+\.)?Plenora(?!Error)\w+", ast.unparse(item.context_expr.args[0]))
                        for item in w.items) for w in ast.walk(owner))
                    for value in values
                )
                if not proven:
                    add(node, "messaggio_python")
    return sorted(found)


def split_arguments(text: str) -> list[str]:
    parts, start, depth = [], 0, 0
    for pos, char in enumerate(text):
        if char in "([{<":
            depth += 1
        elif char in ")]}>" and depth:
            depth -= 1
        elif char == "," and not depth:
            parts.append(text[start:pos])
            start = pos + 1
    parts.append(text[start:])
    return parts


def category_proven(code: str, context: str, receiver: str, seen=None) -> bool:
    """Cerca una prova tipizzata del medesimo errore, anche via helper.

    Non basta nominare una categoria o asserire un errore diverso.
    Questa analisi sintattica non dimostra la semantica dei chiamati.
    """
    seen = set() if seen is None else seen
    key = (context, receiver)
    if key in seen:
        return False
    seen.add(key)
    category = r"(?:\w*Error|ErrorCategory|Categoria|Category)::|\.(?:category|classify)\s*\("
    for macro in re.finditer(r"\b(?:assert\w*|prop_assert\w*)!\s*\(", context):
        end = closing(context, macro.end() - 1, "(", ")")
        assertion = context[macro.end():end]
        if re.search(r"\b" + receiver + r"\b", assertion) and re.search(category, assertion):
            return True
    if re.search(r"let\s+\w*Error::[^;]*=\s*&?\s*" + receiver + r"\b", context):
        return True
    if re.search(r"\w*Error::\w+\s*(?:\([^;]*?\b" + receiver + r"\b[^;]*?\)|\{[^;]*?\b" + receiver + r"\b[^;]*?\})\s*(?:\)|=>|=)", context):
        return True
    if re.search(r"match\s+&?\s*" + receiver + r"\s*\{\s*(?:\w+::)*\w*Error::", context):
        return True
    if re.search(r"\b" + receiver + r"\.category\s*\(\s*\)\s*==", context):
        return True
    # Segue un binding Err(message) al Result controllato prima del match.
    for match in re.finditer(r"match\s+(\w+)\s*\{\s*Err\s*\(\s*" + receiver + r"\s*\)", context):
        if category_proven(code, context, match[1], seen):
            return True
    if re.search(r"\b" + receiver + r"[^;]*\.verifica\s*\(\s*Categoria::", context):
        return True
    # Le proiezioni pubbliche preservano la categoria dell'errore.
    for projection in re.finditer(r"let\s+(\w+)\s*=\s*" + receiver + r"\.public_projection\(\)", context):
        if category_proven(code, context, projection[1], seen):
            return True
    # Il messaggio puo' essere derivato dall'errore tipizzato o da un
    # helper che lo ha gia' verificato. Segue il valore assegnato, non
    # tutte le categorie nominate altrove nella funzione.
    bindings = list(re.finditer(r"let\s+(?:mut\s+)?" + receiver + r"\b[^=;]*=([^;]+);", context))
    expression = bindings[-1][1] if bindings else ""
    if "assert_rifiuto!" in expression:
        return True
    for alias in set(re.findall(r"\b\w+\b", expression)) - {receiver}:
        if category_proven(code, context, alias, seen):
            return True
    for name, a, b in functions_in(code):
        called = re.search(r"\b" + name + r"(?:\s*::\s*<[^;]*?>)?\s*\(", expression)
        helper = code[a:b]
        signature = code[max(0, code.rfind("fn ", 0, a)):a]
        parameters = re.findall(r"(?:^|,)\s*(\w+)\s*:", signature[signature.find("(") + 1:])
        for call in re.finditer(r"\b" + name + r"(?:\s*::\s*<[^;]*?>)?\s*\(", context):
            end = closing(context, call.end() - 1, "(", ")")
            arguments = split_arguments(context[call.end():end - 1])
            for parameter, argument in zip(parameters, arguments, strict=False):
                if re.search(r"\b" + receiver + r"\b", argument) and category_proven(code, helper, parameter, seen):
                    return True
        if called:
            # Il valore proviene da un helper che verifica il proprio
            # risultato prima di restituirlo (es. assert_crs/rifiuto).
            for macro in re.finditer(r"\b(?:assert\w*|prop_assert\w*)!\s*\(", helper):
                end = closing(helper, macro.end() - 1, "(", ")")
                if re.search(category, helper[macro.end():end]):
                    return True
            if "assert_rifiuto!" in helper or re.search(r"match\s+\w+\s*\{\s*(?:\w+::)*\w*Error::", helper):
                return True
            for parameter in parameters:
                if category_proven(code, helper, parameter, seen):
                    return True
    return False


def findings(source: str, integration: bool, language: str = "rust") -> list[tuple[int, str, str]]:
    if language == "python":
        return python_findings(source)
    code = mask(source)
    ranges = test_ranges(code, integration)
    found = []
    functions = [(a, b) for _, a, b in functions_in(code)]
    for rule, (kind, expression) in PATTERNS.items():
        if kind == "python":
            continue
        candidates = []
        if kind in {"rust_macro", "rust_assert"}:
            macro_pattern = r"\bmatches!\s*\(" if kind == "rust_macro" else r"\b(?:prop_)?assert_eq!\s*\("
            for macro in re.finditer(macro_pattern, code):
                end = closing(code, macro.end() - 1, "(", ")")
                if len(re.findall(expression, code[macro.end():end], re.S)) >= (2 if kind == "rust_assert" else 1):
                    candidates.append(macro.start())
        else:
            candidates = [m.start() for m in re.finditer(expression, code)]
        for pos in candidates:
            if not any(start <= pos < end for start, end in ranges):
                continue
            if kind == "rust_message":
                # Categoria tipizzata o asserzione categoria/fase nel corpo
                # piu' interno: il testo non e' l'unico controllo dell'esito.
                bodies = [(a, b) for a, b in functions if a <= pos < b]
                a, b = max(bodies) if bodies else (0, len(code))
                context = code[a:b]
                receiver = re.match(r"\w+", code[pos:])[0]
                prefix = code[max(a, code.rfind(";", a, pos) + 1):pos]
                if re.search(r"!\s*$|!matches!\([^;]*$", prefix) or category_proven(code, context, receiver):
                    continue
                if not re.search(r"expect_err|unwrap_err|\bErr\s*\(|\b(?:errore|error|messaggio|message)\b", context):
                    continue
            line = source.count("\n", 0, pos) + 1
            found.append((line, rule, source.splitlines()[line - 1].strip()))
    return sorted(found)


def test_files(root: Path, sources: dict[Path, str]) -> set[Path]:
    """Propaga cfg(test) attraverso mod esterni, anche con #[path]."""
    selected = {p for p in sources if "tests" in p.relative_to(root).parts
                or p.stem.endswith("tests") or "oracolo" in p.stem}
    while True:
        added = set()
        for path, source in sources.items():
            code = mask(source)
            ranges = test_ranges(code, path in selected)
            for module in re.finditer(r"\bmod\s+(\w+)\s*;", code):
                prefix = source[max(code.rfind(";", 0, module.start()), code.rfind("}", 0, module.start()), 0):module.start()]
                if not (any(a <= module.start() < b for a, b in ranges)
                        or re.search(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", prefix)):
                    continue
                base = path.parent if path.stem in {"lib", "main", "mod"} else path.with_suffix("")
                inline = []
                for parent in re.finditer(r"\bmod\s+(\w+)\s*\{", code[:module.start()]):
                    if closing(code, parent.end() - 1) > module.start():
                        inline.append(parent[1])
                base = base.joinpath(*inline)
                explicit = re.findall(r'#\s*\[\s*path\s*=\s*"([^"\n]+)"\s*\]', prefix)
                if explicit:
                    candidates = [(base if inline else path.parent) / explicit[-1]]
                else:
                    candidates = [base / (module[1] + ".rs"), base / module[1] / "mod.rs"]
                for candidate in candidates:
                    candidate = candidate.resolve()
                    if candidate in sources:
                        added.add(candidate)
        added -= selected
        if not added:
            return selected
        selected.update(added)


def check(root: Path = ROOT) -> list[str]:
    allow = json.loads((root / "scripts/esito_unico_allowlist.json").read_text(encoding="utf-8"))
    entries = {}
    for item in allow:
        if not item["motivo"].strip():
            raise ValueError("eccezione senza motivazione")
        key = (item["file"], item["riga"], item["regola"], item["testo"])
        if key in entries:
            raise ValueError("eccezione duplicata")
        entries[key] = item["motivo"]
    errors, used = [], set()
    root = root.resolve()
    paths = list((root / "crates").rglob("*.rs"))
    sources = {p: p.read_text(encoding="utf-8") for p in paths}
    selected = test_files(root, sources)
    paths.extend((root / "crates/plenora-data-py/python/tests").rglob("*.py"))
    for path in sorted(paths):
        relative = path.relative_to(root).as_posix()
        integration = path in selected
        for line, rule, text in findings(path.read_text(encoding="utf-8"), integration, "python" if path.suffix == ".py" else "rust"):
            key = (relative, line, rule, text)
            if key in entries:
                used.add(key)
            else:
                errors.append(f"{relative}:{line}: {rule}")
    errors.extend(f"eccezione non usata: {key}" for key in sorted(entries.keys() - used))
    return errors


if __name__ == "__main__":
    problems = check()
    print("\n".join(problems) if problems else "esito unico: OK")
    raise SystemExit(bool(problems))
