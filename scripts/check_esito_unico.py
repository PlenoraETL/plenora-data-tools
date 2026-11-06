#!/usr/bin/env python3
"""Rifiuta controlli deboli degli esiti nelle prove Rust e Python del workspace.

Guardia sintattica conservativa, non certificazione degli oracoli: la scelta
di valori, categoria e fase richiede la verifica dello scenario.
L'allowlist contiene occorrenze esatte e una motivazione, mai interi file.
"""

from __future__ import annotations

from rust_esito_tokens import matches_findings, external_test_modules, unprotected_test_entries, has_method_exemption
import json
import re
from functools import lru_cache
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Ogni regola ha esempi positivi e negativi enumerati nei test.
PATTERNS = {
    "errore_ignorato": ("rust", r"\bErr\s*\(\s*(?:ref\s+)?\w+\s*\)\s*=>\s*\{?\s*continue\b"),
    "rifiuto_jolly": ("rust_tokens", "pattern Err con jolly"),
    "rifiuti_alternativi": ("rust_tokens", "alternative nel pattern di Err"),
    "solo_messaggio": ("rust_message", r"(?:\b\w+\s*\.\s*to_string\s*\(\s*\)|\b(?:messaggio|message|testo|msg))\s*\.contains\s*\("),
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


from esito_python_ast import python_findings


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
    found = [(source.count("\n", 0, pos) + 1, rule, source.splitlines()[source.count("\n", 0, pos)].strip()) for pos, rule in matches_findings(source, integration)]
    functions = [(a, b) for _, a, b in functions_in(code)]
    for rule, (kind, expression) in PATTERNS.items():
        if kind in {"python", "rust_tokens"}:
            continue
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
                # Una stringa d'ingresso non e' il messaggio di un errore.
                # I parametri testuali passati da altri helper richiedono
                # verifica del chiamante, non inferenza dal solo nome.
                signature = code[max(0, code.rfind('fn ', 0, a)):a]
                if (re.search(r'\b' + receiver + r'\s*:\s*&\s*str\b', signature)
                        and not re.search(r'\blet\s+(?:mut\s+)?' + receiver + r'\b', context)):
                    continue
                prefix = code[max(a, code.rfind(";", a, pos) + 1):pos]
                if re.search(r"!\s*$|!matches!\([^;]*$", prefix) or category_proven(code, context, receiver):
                    continue
                if not re.search(r"expect_err|unwrap_err|\bErr\s*\(|\b(?:errore|error|messaggio|message)\b", context):
                    continue
            line = source.count("\n", 0, pos) + 1
            found.append((line, rule, source.splitlines()[line - 1].strip()))
    return sorted(found)


def test_files(root: Path, sources: dict[Path, str]) -> set[Path]:
    """Propaga cfg(test) attraverso mod esterni usando token e attributi."""
    selected = {p for p in sources if "tests" in p.relative_to(root).parts}
    while True:
        added = set()
        for path, source in list(sources.items()):
            for name, explicit, inline in external_test_modules(source, path in selected):
                base = path.parent if path.stem in {"lib", "main", "mod"} else path.with_suffix("")
                base = base.joinpath(*inline)
                if explicit:
                    candidates = [(base if inline else path.parent) / explicit]
                else:
                    candidates = [base / (name + ".rs"), base / name / "mod.rs"]
                for candidate in candidates:
                    candidate = candidate.resolve()
                    if candidate.is_relative_to(root) and candidate.is_file():
                        if candidate not in sources:
                            sources[candidate] = candidate.read_text(encoding="utf-8")
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
    # Le radici esentano la produzione anche nel test harness. Nessun nuovo
    # ingresso test puo' ereditare quell'allow senza riattivare il lint.
    production_exempt = any(has_method_exemption(s) for s in sources.values())
    if production_exempt:
        for path, source in sorted(sources.items()):
            if path not in selected:
                for pos in unprotected_test_entries(source):
                    line = source.count('\n', 0, pos) + 1
                    errors.append(f'{path.relative_to(root).as_posix()}:{line}: prove_senza_lint')
    paths = list(sources)
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
