#!/usr/bin/env python3
"""Controlla che i documenti restino risolvibili e coerenti con il codice.

Non valida lo stile della prosa. Protegge proprietà oggettive: link e ancore
locali, comandi Python che esistono, esempi Python che compilano, documenti
generati aggiornati, niente UTF-8 codificato due volte, e la coerenza fra le
superfici documentate (crate e guide elencati negli indici, gate canonici
nel README). Controlla anche i rimandi dei commenti del codice alle guide
(`docs/runner.md, «Budget di memoria»`): il titolo citato deve esistere nel
documento citato.

Uso:

    python scripts/check_docs.py
"""

from __future__ import annotations

import ast
import re
import sys
import tomllib
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT / "scripts") not in sys.path:
    sys.path.insert(0, str(ROOT / "scripts"))

import check_comments  # noqa: E402  (stessa cartella, dopo il percorso)

SKIP_PARTS = {
    ".git",
    ".venv",
    "__pycache__",
    "node_modules",
    "patches",
    "target",
    "venv",
}
LINK = re.compile(r"!?\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
# Definizione di un link a riferimento: `[nome]: destinazione "titolo"`.
REFERENCE_DEFINITION = re.compile(r"^ {0,3}\[[^\]]+\]:\s*<?([^\s>]+)>?")
HEADING = re.compile(r"^(#{1,6})\s+(.+?)\s*#*\s*$")
BOLD_LABEL = re.compile(r"\*\*([^*]+?)\*\*")
PYTHON_COMMAND = re.compile(r"\bpython(?:3)?\s+((?:scripts[\\/])[A-Za-z0-9_.\\/-]+\.py)")
PYTHON_FENCE = re.compile(r"```python\s*\n(.*?)```", re.DOTALL)
# Un rimando di un commento a un documento: il percorso (una guida sotto
# `docs/`, o il README) seguito dal titolo fra caporali, dopo una virgola o
# fra parentesi.
DOC_REFERENCE = re.compile(
    r"(?P<doc>docs/[A-Za-z0-9_./-]+\.md|README(?:\.md)?)"
    r"(?:,\s*|\s*\(\s*|\s+)(?:voce\s+|sezione\s+)?«(?P<title>[^»]+)»"
)
# Le schede non si leggono da sole: entrano in `docs/operazioni.md`, e i loro
# link (anche le ancore senza file, verso le altre schede) si leggono come se
# fossero in quel documento.
ASSEMBLED_INTO = {Path("docs/schede"): Path("docs/operazioni.md")}
# Documenti generati: la sorgente e il comando che li rigenera.
GENERATED = {
    Path("docs/inventario.md"): "python scripts/genera_inventario.py",
    Path("docs/operazioni.md"): (
        "PLENORA_RIGENERA_DOC=1 cargo test -p plenora-io --test operazioni_doc"
    ),
}
# Gate che il README deve nominare: sono quelli che la CI esegue.
CANONICAL_GATES = (
    "cargo fmt --all --check",
    "cargo clippy --workspace --all-targets --locked -- -D warnings",
    "PLENORA_TEST_LUNGHI=1 cargo test --workspace --locked",
    "python scripts/genera_costi_operazioni.py --verifica",
    "python scripts/genera_costi_geo.py --verifica",
    "python scripts/genera_inventario.py --check",
    "python scripts/check_docs.py",
    "python scripts/check_comments.py",
)


@dataclass(frozen=True)
class Violation:
    path: Path
    reason: str


def skipped(relative: Path) -> bool:
    """Vale per i documenti che non sono di questo repository.

    Di `vendor/` si leggono solo le provenienze: il resto è documentazione
    upstream, con link verso file che qui non esistono.
    """

    parts = relative.parts
    if SKIP_PARTS.intersection(parts):
        return True
    return "vendor" in parts and not relative.name.startswith("PROVENANCE")


def markdown_documents(root: Path = ROOT) -> list[Path]:
    return [
        path
        for path in sorted(root.rglob("*.md"))
        if not skipped(path.relative_to(root))
    ]


def github_slug(title: str) -> str:
    """L'ancora che GitHub dà a un titolo.

    Minuscole; via tutto ciò che non è lettera, cifra, spazio, `-` o `_`
    (le lettere accentate restano); ogni spazio diventa `-`, senza
    comprimere i trattini: «WGS 84 = ETRS89» dà `wgs-84--etrs89`.
    """

    text = re.sub(r"<[^>]+>", "", title.strip().lower())
    return "".join(
        "-" if char == " " else char
        for char in text
        if char.isalnum() or char in " -_"
    )


def outside_fences(text: str) -> list[str]:
    """Le righe fuori dai blocchi di codice."""

    lines: list[str] = []
    fence = False
    for line in text.split("\n"):
        if line.lstrip().startswith("```"):
            fence = not fence
            continue
        if not fence:
            lines.append(line)
    return lines


def headings(path: Path) -> list[str]:
    return [
        match.group(2)
        for line in outside_fences(path.read_text(encoding="utf-8"))
        if (match := HEADING.match(line))
    ]


def anchors(path: Path) -> set[str]:
    counts: dict[str, int] = {}
    found: set[str] = set()
    for heading in headings(path):
        base = github_slug(heading)
        occurrence = counts.get(base, 0)
        counts[base] = occurrence + 1
        found.add(base if occurrence == 0 else f"{base}-{occurrence}")
    return found


def reading_context(root: Path, path: Path) -> Path:
    """Il documento nel quale `path` si legge: sé stesso, o quello che lo
    assembla."""

    assembled = ASSEMBLED_INTO.get(path.relative_to(root).parent)
    return root / assembled if assembled is not None else path


def link_targets(text: str) -> list[str]:
    """Le destinazioni dei link fuori dai blocchi di codice: in linea
    (`[testo](destinazione)`) e definizioni dei link a riferimento
    (`[nome]: destinazione`)."""

    lines = outside_fences(text)
    targets = LINK.findall("\n".join(lines))
    targets += [
        match.group(1) for line in lines if (match := REFERENCE_DEFINITION.match(line))
    ]
    return targets


def validate_links(root: Path, documents: list[Path]) -> list[Violation]:
    violations: list[Violation] = []
    cache: dict[Path, set[str]] = {}
    for path in documents:
        context = reading_context(root, path)
        for raw in link_targets(path.read_text(encoding="utf-8")):
            target = raw.strip().strip("<>")
            if target.startswith(("http://", "https://", "mailto:")):
                continue
            file_part, separator, anchor = target.partition("#")
            destination = (
                context if not file_part else (context.parent / unquote(file_part)).resolve()
            )
            if not destination.exists():
                violations.append(Violation(path, f"link locale inesistente: {raw}"))
                continue
            if separator and anchor and destination.suffix.lower() == ".md":
                known = cache.setdefault(destination, anchors(destination))
                if unquote(anchor).lower() not in known:
                    violations.append(Violation(path, f"ancora inesistente: {raw}"))
    return violations


def validate_commands(root: Path, documents: list[Path]) -> list[Violation]:
    violations: list[Violation] = []
    for path in documents:
        text = path.read_text(encoding="utf-8")
        for command_path in PYTHON_COMMAND.findall(text):
            normalized = command_path.replace("\\", "/")
            if not (root / normalized).is_file():
                violations.append(
                    Violation(path, f"comando Python punta a un file assente: {command_path}")
                )
    return violations


def validate_python_examples(documents: list[Path]) -> list[Violation]:
    """Ogni esempio Python deve essere almeno sintatticamente eseguibile."""

    violations: list[Violation] = []
    for path in documents:
        text = path.read_text(encoding="utf-8")
        for position, source in enumerate(PYTHON_FENCE.findall(text), start=1):
            try:
                compile(
                    source,
                    f"{path}#python-{position}",
                    "exec",
                    flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT,
                )
            except SyntaxError as exc:
                violations.append(
                    Violation(path, f"esempio Python {position} non valido: riga {exc.lineno}")
                )
    return violations


def validate_encoding(documents: list[Path]) -> list[Violation]:
    """Niente testo UTF-8 decodificato due volte nei documenti."""

    return [
        Violation(path, f"doppia codifica UTF-8: {run}")
        for path in documents
        for run in check_comments.double_encoded(path.read_text(encoding="utf-8"))
    ]


def normalized_title(text: str) -> str:
    """Un titolo confrontabile con le sue citazioni nei commenti.

    I commenti scrivono a volte le vocali accentate con l'apostrofo
    (`piu'` per `più`): accenti e apostrofi finali si tolgono da entrambe le
    parti, l'apostrofo dentro la parola (`d'ingresso`) resta.
    """

    text = text.replace("`", "").replace("’", "'")
    text = unicodedata.normalize("NFKD", text)
    text = "".join(char for char in text if not unicodedata.combining(char))
    text = re.sub(r"([aeiouAEIOU])'(?=\W|$)", r"\1", text)
    return " ".join(text.split()).rstrip(".:").lower()


def labels(path: Path) -> set[str]:
    """Titoli ed etichette in grassetto di un documento, normalizzati."""

    found: set[str] = set()
    for line in outside_fences(path.read_text(encoding="utf-8")):
        if match := HEADING.match(line):
            found.add(normalized_title(match.group(2)))
        found.update(normalized_title(label) for label in BOLD_LABEL.findall(line))
    return found


def validate_code_references(root: Path) -> list[Violation]:
    """I rimandi dei commenti a una guida nominano un titolo che c'è.

    Un rimando può abbreviare il titolo («Precisione delle operazioni
    geografiche» per il titolo intero): vale se un titolo o un'etichetta in
    grassetto del documento comincia così.
    """

    violations: list[Violation] = []
    cache: dict[Path, set[str]] = {}
    for path in sorted(check_comments.source_files(root)):
        source = path.read_text(encoding="utf-8")
        comments = list(check_comments.comments_for(path, source))
        # I commenti consecutivi formano una frase: un rimando può andare a
        # capo. Il resto del marcatore (`!` di `//!`, `/` di `///`) si toglie.
        text = "\n".join(comment.text for comment in comments)
        text = re.sub(r"\n\s*[/!]*\s*", " ", text)
        for match in DOC_REFERENCE.finditer(text):
            name = match.group("doc")
            document = root / ("README.md" if name.startswith("README") else name)
            relative = path.relative_to(root).as_posix()
            if not document.is_file():
                violations.append(Violation(path, f"rimanda a un documento assente: {name}"))
                continue
            known = cache.setdefault(document, labels(document))
            title = normalized_title(match.group("title"))
            if not any(label.startswith(title) for label in known):
                violations.append(
                    Violation(path, f"{relative}: «{match.group('title')}» non è in {name}")
                )
    return violations


def validate_generated(root: Path) -> list[Violation]:
    violations: list[Violation] = []
    for relative, command in GENERATED.items():
        path = root / relative
        if not path.is_file():
            violations.append(Violation(path, "documento generato assente"))
            continue
        head = "\n".join(path.read_text(encoding="utf-8").split("\n")[:6])
        if "Generato da" not in head or command.split()[-1] not in head:
            violations.append(
                Violation(path, f"documento generato senza intestazione: {command}")
            )
    if root == ROOT:
        import genera_inventario

        target = genera_inventario.DESTINAZIONE
        current = target.read_text(encoding="utf-8") if target.is_file() else ""
        if current != genera_inventario.render():
            violations.append(Violation(target, "documento generato non aggiornato"))
    return violations


def validate_semantics(root: Path) -> list[Violation]:
    """Indici e README nominano ciò che il repository contiene."""

    violations: list[Violation] = []
    readme_path = root / "README.md"
    readme = readme_path.read_text(encoding="utf-8")
    for gate in CANONICAL_GATES:
        if gate not in readme:
            violations.append(Violation(readme_path, f"gate canonico assente: {gate}"))

    for manifest in sorted((root / "crates").glob("*/Cargo.toml")):
        name = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["name"]
        if f"`{name}`" not in readme:
            violations.append(Violation(readme_path, f"crate senza riga nel README: {name}"))
        crate_readme = manifest.parent / "README.md"
        if crate_readme.is_file():
            link = crate_readme.relative_to(root).as_posix()
            if f"({link}" not in readme:
                violations.append(
                    Violation(readme_path, f"README del crate non collegato: {link}")
                )

    index_path = root / "docs" / "README.md"
    index = index_path.read_text(encoding="utf-8") if index_path.is_file() else ""
    if not index:
        violations.append(Violation(index_path, "indice delle guide assente"))
    for guide in sorted((root / "docs").glob("*.md")):
        if guide.name != "README.md" and f"]({guide.name})" not in index:
            violations.append(Violation(index_path, f"guida senza riga nell'indice: {guide.name}"))
    return violations


def scan(root: Path = ROOT) -> tuple[int, list[Violation]]:
    documents = markdown_documents(root)
    violations: list[Violation] = []
    for path in documents:
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            violations.append(Violation(path, f"documento illeggibile: {exc}"))
            continue
        if sum(line.lstrip().startswith("```") for line in text.split("\n")) % 2:
            violations.append(Violation(path, "blocco di codice non chiuso"))
    violations += validate_links(root, documents)
    violations += validate_commands(root, documents)
    violations += validate_python_examples(documents)
    violations += validate_encoding(documents)
    violations += validate_code_references(root)
    violations += validate_generated(root)
    violations += validate_semantics(root)
    return len(documents), violations


def main() -> int:
    checked, violations = scan()
    if violations:
        for violation in violations:
            try:
                relative = violation.path.relative_to(ROOT).as_posix()
            except ValueError:
                relative = violation.path.as_posix()
            print(f"{relative}: docs: {violation.reason}")
        print(f"docs: {len(violations)} violazioni in {checked} documenti")
        return 1
    print(f"docs: {checked} documenti controllati, nessuna violazione")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
