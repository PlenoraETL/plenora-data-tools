"""Guardia delle conversioni temporali esterne e del confine Parquet.

Riusa il lexer delle guardie: commenti, stringhe e blocchi cfg(test) non
sono codice produttivo. Non e' un analizzatore di flusso Rust: gli alias
locali dei tipi sono risolti, le conversioni generiche implicite richiedono
anche il controllo dei confini e il lint anti-panic.
"""
from pathlib import Path
import re
import sys
from check_esito_unico import mask, closing, body_start

ROOT = Path(__file__).resolve().parents[1]
VENDOR = "vendor/parquet-60.0.0-eof/src/record"


def production(source):
    code = mask(source)
    for mark in reversed(list(re.finditer(r"#\s*\[\s*(?:test|cfg\s*\(\s*test\s*\))\s*\]", code))):
        start = body_start(code, mark.end())
        if start >= 0:
            end = closing(code, start)
            code = code[:start] + re.sub(r"[^\n]", " ", code[start:end]) + code[end:]
    for statement in re.findall(r"\buse\b[^;]*;", code):
        for original, alias in re.findall(r"\b(SystemTime|DateTime|Duration|TimeDelta)\s+as\s+(\w+)", statement):
            code = re.sub(r"\b" + re.escape(alias) + r"\b", original, code)
    for alias, original in re.findall(r"\btype\s+(\w+)\s*=\s*(?:[\w]+::)*(SystemTime|DateTime|Duration|TimeDelta)\b", code):
        code = re.sub(r"\b" + re.escape(alias) + r"\b", original, code)
    return code


def violations(source):
    code = production(source)
    rules = {
        "conversione_clock_infallibile": r"\b(?:SystemTime|DateTime)(?:\s*::\s*<[^;]+?>)?\s*::\s*from\s*\(",
        "conversione_clock_trait": r"(?:Into|From)\s*::?\s*<[^;]*(?:SystemTime|DateTime)[^;]*>\s*::\s*(?:into|from)\s*\(",
        "conversione_clock_trait_qualificato": r"<[^;]*(?:SystemTime|DateTime)[^;]*\bas\s+(?:[\w]+::)*From\s*<[^;]*>\s*::\s*from\s*\(",
        "conversione_clock_ritorno": r"\bfn\s+\w+[^;{]*->\s*(?:[\w]+::)*(?:SystemTime|DateTime)[^;{]*\{[^}]*\.\s*into\s*\(",
        "conversione_clock_implicita": r"\blet\s+(?:mut\s+)?\w+\s*:\s*(?:[\w]+::)*(?:SystemTime|DateTime)[^;=]*=[^;]*\.\s*into\s*\(",
        "durata_float_infallibile": r"\bDuration\s*::\s*from_secs_f(?:32|64)\s*\(",
        "timestamp_nanos_infallibile": r"\.\s*timestamp_nanos\s*\(",
    }
    found = [(name, code.count("\n", 0, match.start()) + 1)
             for name, pattern in rules.items() for match in re.finditer(pattern, code)]
    calls = r"\b(?:timestamp(?:_millis_opt|_opt|_micros)?|from_timestamp(?:_millis|_micros)?|from_ymd_opt)\s*\("
    for match in re.finditer(calls, code):
        end = closing(code, match.end() - 1, "(", ")")
        if re.match(r"(?:\s*\.\s*single\s*\(\))?\s*\.\s*(?:unwrap|expect|unwrap_or|unwrap_or_else)\s*\(", code[end:]):
            found.append(("calendario_unwrap", code.count("\n", 0, match.start()) + 1))
    return found


def barrier_errors(source):
    code = production(source)
    try:
        read = code[code.index("pub fn leggi("):code.index("pub fn verifica_schema(")]
        if read.index("verifica_colonne(&letti)?") >= read.index("ArrowReaderMetadata::try_new"):
            return ["barriera INT96 successiva alla conversione Arrow"]
        start = code.index("fn verifica_colonne(")
        opening = body_start(code, start)
        body = re.sub(r"\s+", "", code[opening:closing(code, opening)])
        if not all(part in body for part in ["schema_descr().columns()", "physical_type()==TipoFisico::INT96", "PlenoraError::Unsupported"]):
            return ["barriera INT96 incompleta sullo schema"]
    except ValueError:
        return ["barriera INT96 assente o forma da qualificare"]
    return []


def check(root=ROOT):
    errors = []
    paths = sorted((root / "crates").glob("*/src/**/*.rs"))
    paths += sorted((root / VENDOR).glob("*.rs"))
    for path in paths:
        source = path.read_text(encoding="utf-8")
        rel = path.relative_to(root).as_posix()
        errors += [f"{rel}:{line}: {name}" for name, line in violations(source)]
        code = production(source)
        if rel.startswith("crates/"):
            for match in re.finditer(r"\bparquet\s*::\s*record\b|\.\s*to_(?:nanos|micros|millis|seconds)\s*\(", code):
                errors.append(f"{rel}: API temporale vendor senza barriera")
            if rel != "crates/plenora-io/src/parquet_io.rs" and re.search(
                r"\b(?:ParquetRecordBatchReaderBuilder|ArrowReaderMetadata|SerializedFileReader)\b", code):
                errors.append(f"{rel}: nuovo ingresso Parquet da qualificare")
    path = root / "crates/plenora-io/src/parquet_io.rs"
    errors += barrier_errors(path.read_text(encoding="utf-8"))
    return errors


if __name__ == "__main__":
    errors = check()
    print("\n".join(errors) if errors else "tempo: conversioni e barriera verificate")
    sys.exit(bool(errors))
