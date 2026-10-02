#!/usr/bin/env python3
"""Controlla le regole oggettive dei commenti nel repository.

Il controllo non misura quanto un commento sia elegante. Presidia soltanto le
regole che si applicano senza interpretazione: niente debito anonimo, niente
cronaca del processo di sviluppo (chi ha rivisto il codice e in quale giro,
numeri di PR, date e autori delle decisioni, etichette di fase) e niente
UTF-8 codificato due volte.
Motivazioni, invarianti, limiti, provenienza del codice e compatibilità
correnti restano invece contenuto utile.

Il codice vendorizzato (`vendor/`, `patches/`) è upstream e non si controlla;
i documenti Markdown li controlla `scripts/check_docs.py`.

Uso:

    python scripts/check_comments.py
"""

from __future__ import annotations

import ast
import io
import re
import tokenize
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Iterator

ROOT = Path(__file__).resolve().parents[1]

SKIP_PARTS = {
    ".git",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    "__pycache__",
    "build",
    "dist",
    "docs",
    "node_modules",
    "patches",
    "target",
    "vendor",
    "venv",
}

PYTHON_SUFFIXES = {".py", ".pyi"}
C_LIKE_SUFFIXES = {".c", ".cc", ".cpp", ".h", ".hpp", ".js", ".rs", ".ts"}
HASH_SUFFIXES = {".cfg", ".ini", ".ps1", ".sh", ".toml", ".yaml", ".yml"}
HASH_NAMES = {".gitattributes", ".gitignore"}
DASH_SUFFIXES = {".sql"}

DEBT_MARKER = re.compile(r"\b(?:TODO|FIXME|HACK|XXX)\b")
HISTORY_MARKERS: tuple[re.Pattern[str], ...] = tuple(
    re.compile(pattern, re.IGNORECASE)
    for pattern in (
        # Le stesse regole di plenora-database-tools.
        r"\bprima (?:stesura|versione|implementazione|esecuzione|campagna)\b",
        r"\b(?:versione|forma|stesura|commento|contratto) precedente\b",
        r"\bvecchio contratto\b",
        r"\bprecedente al fix\b",
        r"\bsweep ha\b",
        r"\bqui c[’']era\b",
        r"\bera rimast",
        r"\bprima era\b",
        r"\berano inline\b",
        r"\bda allora\b",
        r"\bentrata per ultima\b",
        r"\bnello stesso commit\b",
        r"\ball[’']epoca\b",
        r"\bfino a poco fa\b",
        r"\bera stat[oa] (?:aggiunt[oa]|rimoss[oa])\b",
        r"\bprima mancava\b",
        r"\bpoi mancava\b",
        r"\bprima duplicava\b",
        r"\bprima diceva\b",
        r"\b(?:commento|contratto|documentazione|messaggio|riga di aiuto) diceva\b",
        r"\bcosa dicevano\b",
        r"\baveva (?:concluso|provato)\b",
        r"\bfino alla separazione\b",
        r"\bnon e piu uno scaffold\b",
        r"\bdiceva il contrario\b",
        r"\bquesta guardia diceva\b",
        r"\bquando e stata scritta\b",
        r"\bcio che e cambiato\b",
        r"\broadmap\b",
        r"\bmilestone\b",
        r"\bf\d+(?:[.-]\d+)+(?:[a-z])?\b",
        r"\bf\d+[a-z]\b",
        r"\bp\d+\.\d+\b",
        r"(?:^|=+\s*)[ab]\d+(?:[a-z+.-]\w*)?\s*:",
        r"\bopz\s+\d+\b",
        r"\bfase\s+a\d+\b",
        r"\badapter temporaneo\b",
        r"\bplaceholder for future\b",
        r"\bpost-review\b",
        r"\bpre-fix\b",
        r"\bfix review\b",
        r"\btranche\b",
        r"\bADR[- ]+\d+\b",
        r"\bprima di questo fix\b",
        # Chi ha rivisto il codice e in quale giro: il commento dice la
        # regola, non chi ha trovato il caso. Un contratto citato per commit
        # è provenienza, non cronaca.
        r"\bcodex\b",
        r"\bclaude\b",
        r"\bagent[ei]\b",
        r"\bsecondo lettore\b",
        r"\breview(?:er|ers|s)?\b",
        r"\brevisor[ei]\b",
        r"\brevisione\b(?!\s*`?[0-9a-f]{7,})",
        r"\b(?:prima|seconda|terza|quarta|quinta|sesta) lettura\b",
        r"\b(?:terzo|quarto|quinto|sesto|settimo|ottavo|nono|decimo|\w+esimo) giro\b",
        # Numeri di PR e di difetto, date e autori delle decisioni.
        r"\bPR\s*-?\s*#?\d+\b",
        r"\bdifetto\s+\d+\b",
        r"\bciclo dei difetti\b",
        r"\bmaintainer\b",
        r"\bdecisione dell?[’']?\s*(?:utente|maintainer)\b",
        r"\b\d{1,2}\s+(?:gennaio|febbraio|marzo|aprile|maggio|giugno|luglio|agosto"
        r"|settembre|ottobre|novembre|dicembre)\s+20\d\d\b",
    )
)
# Etichette di fase del piano di sviluppo (una F maiuscola e una cifra):
# sensibili alle maiuscole, perché `f64` è un tipo e non una fase.
CASE_SENSITIVE_HISTORY: tuple[re.Pattern[str], ...] = (re.compile(r"\bF[1-9]\b"),)

# Un carattere che in cp1252 (o Latin-1) è un byte non ASCII: una sequenza
# di questi caratteri che, riportata a byte, è UTF-8 valido è testo UTF-8
# decodificato una seconda volta (una «è» diventa due caratteri latini).
_CP1252_RUN = re.compile(
    "[\u0080-ÿŒœŠšŸŽžƒˆ˜"
    "–—‘-‚“-„†-•…‰‹›"
    "€™]{2,}"
)
_UTF8_SEQUENCE = re.compile(
    rb"[\xc2-\xdf][\x80-\xbf]|[\xe0-\xef][\x80-\xbf]{2}|[\xf0-\xf4][\x80-\xbf]{3}"
)
_RUST_CHAR = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|.)|[^\\'\n])'")


def double_encoded(text: str) -> list[str]:
    """Le sequenze di `text` che sono UTF-8 decodificato due volte."""

    found: list[str] = []
    for match in _CP1252_RUN.finditer(text):
        run = match.group(0)
        try:
            raw = run.encode("cp1252")
        except UnicodeEncodeError:
            raw = bytes(ord(char) for char in run if ord(char) < 256)
        if _UTF8_SEQUENCE.search(raw):
            found.append(run)
    return found


@dataclass(frozen=True)
class Comment:
    line: int
    text: str


@dataclass(frozen=True)
class Violation:
    path: Path
    line: int
    rule: str
    excerpt: str


def _python_comments(source: str) -> Iterator[Comment]:
    """Estrae commenti e docstring senza confonderli con stringhe ordinarie."""

    try:
        tokens = tokenize.generate_tokens(io.StringIO(source).readline)
        for token in tokens:
            if token.type == tokenize.COMMENT:
                yield Comment(token.start[0], token.string.removeprefix("#").strip())
    except (IndentationError, tokenize.TokenError):
        return

    try:
        tree = ast.parse(source)
    except SyntaxError:
        return
    for node in ast.walk(tree):
        if not isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if not node.body:
            continue
        first = node.body[0]
        if isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant):
            if isinstance(first.value.value, str):
                yield Comment(first.lineno, first.value.value)


def _c_like_comments(source: str) -> Iterator[Comment]:
    """Estrae commenti lineari e a blocco, ignorando stringhe e caratteri.

    Riconosce le stringhe ordinarie, le raw string Rust e i letterali di
    carattere (`'"'`): senza questi ultimi un apice doppio fra apici
    singoli aprirebbe una stringa che non esiste.
    """

    index = 0
    line = 1
    length = len(source)
    while index < length:
        raw = re.match(r"(?:br|rb|r)(?P<hashes>#{0,255})\"", source[index : index + 260])
        if raw and (index == 0 or not (source[index - 1].isalnum() or source[index - 1] == "_")):
            delimiter = '"' + raw.group("hashes")
            start = index + raw.end()
            end = source.find(delimiter, start)
            if end < 0:
                return
            segment = source[index : end + len(delimiter)]
            line += segment.count("\n")
            index = end + len(delimiter)
            continue
        if source[index] == "'":
            char = _RUST_CHAR.match(source, index)
            index = char.end() if char else index + 1
            continue
        if source[index] == '"':
            index += 1
            while index < length:
                if source[index] == "\\":
                    if index + 1 < length and source[index + 1] == "\n":
                        line += 1
                    index += 2
                    continue
                if source[index] == '"':
                    index += 1
                    break
                if source[index] == "\n":
                    line += 1
                index += 1
            continue
        if source.startswith("//", index):
            start_line = line
            end = source.find("\n", index + 2)
            if end < 0:
                end = length
            yield Comment(start_line, source[index + 2 : end].strip())
            index = end
            continue
        if source.startswith("/*", index):
            start_line = line
            start = index + 2
            index = start
            depth = 1
            while index < length and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    if source[index] == "\n":
                        line += 1
                    index += 1
            yield Comment(start_line, source[start : max(start, index - 2)].strip())
            continue
        if source[index] == "\n":
            line += 1
        index += 1


@dataclass(frozen=True)
class LineSyntax:
    """Le regole delle stringhe di un formato con commenti fino a fine riga.

    Ogni formato ha le sue: in TOML `'...'` è una stringa letterale, senza
    escape (`'C:\\'` è chiusa), mentre in `"..."` la barra rovescia
    protegge l'apice; YAML e SQL raddoppiano l'apice singolo; in YAML e
    nella shell `#` apre un commento solo a inizio riga o dopo uno spazio.
    """

    marker: str = "#"
    # Apici nei quali la barra rovescia protegge il carattere seguente.
    backslash_in: frozenset[str] = frozenset()
    # Apici che si proteggono raddoppiandoli (`''` dentro `'...'`).
    doubled_in: frozenset[str] = frozenset()
    # Delimitatori di stringhe su più righe, con l'escape che vale dentro.
    multiline: tuple[tuple[str, bool], ...] = ()
    # Un apice apre una stringa solo dopo spazio, inizio riga o separatore.
    quote_at_boundary: bool = False
    # Il marcatore apre un commento solo dopo spazio o a inizio riga.
    marker_at_boundary: bool = False
    # Commenti solo su righe intere (`.gitignore`, `.ini`).
    whole_line_only: bool = False


_TOML = LineSyntax(
    backslash_in=frozenset('"'),
    multiline=(('"""', True), ("'''", False)),
)
_YAML = LineSyntax(
    backslash_in=frozenset('"'),
    doubled_in=frozenset("'"),
    quote_at_boundary=True,
    marker_at_boundary=True,
)
_SHELL = LineSyntax(backslash_in=frozenset('"'), marker_at_boundary=True)
_POWERSHELL = LineSyntax(doubled_in=frozenset("'\""), marker_at_boundary=True)
_SQL = LineSyntax(marker="--", doubled_in=frozenset("'"))
_WHOLE_LINE = LineSyntax(whole_line_only=True)
LINE_SYNTAX = {
    ".toml": _TOML,
    ".yaml": _YAML,
    ".yml": _YAML,
    ".sh": _SHELL,
    ".ps1": _POWERSHELL,
    ".sql": _SQL,
    ".cfg": _WHOLE_LINE,
    ".ini": _WHOLE_LINE,
}
_QUOTE_OPENERS = " \t[{(,:=-?"


def _line_comments(source: str, syntax: LineSyntax) -> Iterator[Comment]:
    """Estrae commenti da formati nei quali il marcatore vale fino a EOL."""

    marker = syntax.marker
    open_multiline: tuple[str, bool] | None = None
    for line_number, line in enumerate(source.splitlines(), 1):
        if syntax.whole_line_only:
            if line.lstrip().startswith(marker):
                yield Comment(line_number, line.lstrip()[len(marker) :].strip())
            continue
        index = 0
        if open_multiline is not None:
            delimiter, escapes = open_multiline
            while index < len(line):
                if escapes and line[index] == "\\":
                    index += 2
                    continue
                if line.startswith(delimiter, index):
                    index += len(delimiter)
                    open_multiline = None
                    break
                index += 1
            if open_multiline is not None:
                continue
        quote: str | None = None
        while index < len(line):
            char = line[index]
            if quote is not None:
                if char == "\\" and quote in syntax.backslash_in:
                    index += 2
                    continue
                if char == quote:
                    if quote in syntax.doubled_in and line.startswith(quote * 2, index):
                        index += 2
                        continue
                    quote = None
                index += 1
                continue
            started = next(
                (pair for pair in syntax.multiline if line.startswith(pair[0], index)), None
            )
            if started is not None:
                delimiter, escapes = started
                index += len(delimiter)
                while index < len(line):
                    if escapes and line[index] == "\\":
                        index += 2
                        continue
                    if line.startswith(delimiter, index):
                        index += len(delimiter)
                        break
                    index += 1
                else:
                    open_multiline = started
                continue
            at_boundary = index == 0 or line[index - 1] in _QUOTE_OPENERS
            if char in {'"', "'"} and (at_boundary or not syntax.quote_at_boundary):
                quote = char
                index += 1
                continue
            if line.startswith(marker, index) and (
                not syntax.marker_at_boundary or index == 0 or line[index - 1].isspace()
            ):
                yield Comment(line_number, line[index + len(marker) :].strip())
                break
            index += 1


def commentable(path: Path) -> bool:
    """Il file ha un formato del quale si sanno leggere i commenti."""

    suffix = path.suffix.lower()
    return (
        suffix in PYTHON_SUFFIXES
        or suffix in C_LIKE_SUFFIXES
        or suffix in HASH_SUFFIXES
        or suffix in DASH_SUFFIXES
        or path.name in HASH_NAMES
        or path.name.startswith("Dockerfile")
    )


def comments_for(path: Path, source: str) -> Iterable[Comment]:
    """Seleziona l'estrattore in base al formato del file."""

    suffix = path.suffix.lower()
    if suffix in PYTHON_SUFFIXES:
        return _python_comments(source)
    if suffix in C_LIKE_SUFFIXES:
        return _c_like_comments(source)
    if suffix in LINE_SYNTAX:
        return _line_comments(source, LINE_SYNTAX[suffix])
    if path.name in HASH_NAMES or path.name.startswith("Dockerfile"):
        return _line_comments(source, _WHOLE_LINE)
    return ()


def source_files(root: Path = ROOT) -> Iterator[Path]:
    """Visita i formati commentabili, inclusi i file non ancora tracciati."""

    for path in root.rglob("*"):
        if not path.is_file() or any(part in SKIP_PARTS for part in path.relative_to(root).parts):
            continue
        if commentable(path):
            yield path


def check_comment(comment: Comment) -> list[tuple[str, str]]:
    """Le regole violate da un commento, con l'estratto che le viola."""

    compact = " ".join(comment.text.split())
    found: list[tuple[str, str]] = []
    if match := DEBT_MARKER.search(compact):
        found.append(("debito anonimo", match.group(0)))
    for pattern in HISTORY_MARKERS + CASE_SENSITIVE_HISTORY:
        if match := pattern.search(compact):
            found.append(("cronaca obsoleta", match.group(0)))
    for run in double_encoded(comment.text):
        found.append(("doppia codifica UTF-8", run))
    return found


def check_file(path: Path, root: Path = ROOT) -> list[Violation]:
    """Restituisce tutte le violazioni trovate in un file."""

    source = path.read_text(encoding="utf-8")
    return [
        Violation(path.relative_to(root), comment.line, rule, excerpt)
        for comment in comments_for(path, source)
        for rule, excerpt in check_comment(comment)
    ]


def check_repository(root: Path = ROOT) -> tuple[int, list[Violation]]:
    """Controlla il repository e restituisce numero di file ed errori."""

    paths = sorted(source_files(root))
    violations = [violation for path in paths for violation in check_file(path, root)]
    return len(paths), violations


def main() -> int:
    checked, violations = check_repository()
    for violation in violations:
        print(
            f"{violation.path.as_posix()}:{violation.line}: {violation.rule}: "
            f"{violation.excerpt}"
        )
    if violations:
        print(f"commenti: {len(violations)} violazioni in {checked} file controllati")
        return 1
    print(f"commenti: {checked} file controllati, nessuna violazione")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
