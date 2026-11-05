#!/usr/bin/env python3
"""Rifiuta asserzioni booleane sugli esiti nelle prove Rust del workspace.

Guardia sintattica conservativa, non certificazione degli oracoli: la scelta
di valori, categoria e fase richiede la verifica dello scenario.
L'allowlist contiene occorrenze esatte e una motivazione, mai interi file.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PATTERNS = {
    "rifiuto_booleano": re.compile(r"\.\s*is_err\s*\(\s*\)"),
    "errore_ignorato": re.compile(r"\bErr\s*\(\s*_\s*\)\s*=>\s*\{?\s*continue\b"),
    "successo_o_rifiuto": re.compile(r"\.\s*is_ok\s*\(\s*\)\s*\|\|"),
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


def findings(source: str, integration: bool) -> list[tuple[int, str, str]]:
    """Nei sorgenti considera solo i corpi marcati test/cfg(test)."""
    code = mask(source)
    ranges = [(0, len(code))] if integration else []
    if not integration:
        for mark in re.finditer(r"#\s*\[\s*(?:test|cfg\s*\(\s*test\s*\))\s*\]", code):
            opening = code.find("{", mark.end())
            semicolon = code.find(";", mark.end())
            if opening == -1 or 0 <= semicolon < opening:
                continue
            depth, end = 1, opening + 1
            while end < len(code) and depth:
                depth += (code[end] == "{") - (code[end] == "}")
                end += 1
            ranges.append((opening, end))
    found = []
    for rule, pattern in PATTERNS.items():
        for match in pattern.finditer(code):
            if any(start <= match.start() < end for start, end in ranges):
                line = source.count("\n", 0, match.start()) + 1
                found.append((line, rule, source.splitlines()[line - 1].strip()))
    return sorted(found)


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
    for path in sorted((root / "crates").rglob("*.rs")):
        relative = path.relative_to(root).as_posix()
        integration = ("tests" in path.relative_to(root).parts or path.stem.endswith("tests") or "oracolo" in path.stem)
        for line, rule, text in findings(path.read_text(encoding="utf-8"), integration):
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
