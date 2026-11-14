"""Verifica inventari di prova contro i sorgenti su ogni PR.

--scrivi rigenera il censimento, non modifica oracoli, hash o soglie.
La scansione comprende i file propri; vendor e target sono esclusi.
"""

import argparse
import ast
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib

from fonti_prove import ROOT, crs_integrati, sonde_sdk, target_fuzz, unici
import strict_json

DEST = "docs/prove-fonti.json"


def python_tests(source):
    tree = ast.parse(source.lstrip("\ufeff"))
    return sorted(n.name for n in ast.walk(tree)
                  if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
                  and n.name.startswith("test_"))


def workflow_events(source):
    # Forma canonica dei workflow del repository. Forme diverse richiedono
    # un adattamento esplicito, non vengono interpretate come elenchi vuoti.
    match = re.search(r'^on:\n((?:[ \t].*\n|\n|#.*\n)+)', source, re.M)
    if not match:
        raise ValueError("workflow: blocco on non riconosciuto")
    return unici(re.findall(r'^  ([a-z_]+):', match[1], re.M), "eventi workflow")


def normalizza_percorsi(names):
    result = []
    for name in names:
        name = str(name).replace("\\", "/")
        path = PurePosixPath(name)
        if not name or path.is_absolute() or ".." in path.parts or ":" in name or str(path) != name:
            raise ValueError("percorso del censimento non relativo o non canonico")
        result.append(name)
    return unici(result, "file tracciati")


def file_tracciati(root):
    raw = subprocess.check_output(
        ["git", "ls-files", "--cached", "--full-name", "-z"], cwd=root,
    )
    if not raw.endswith(b"\0"):
        raise ValueError("elenco Git vuoto o non terminato")
    return normalizza_percorsi(raw[:-1].decode("utf-8").split("\0"))


def inventario(root=ROOT):
    scripts, tests, fixtures, workflows = {}, {}, [], {}
    paths = file_tracciati(root)
    required = {"fuzz/Cargo.toml", "crates/plenora-data-py/src/sonde.rs"}
    if not required <= set(paths):
        raise ValueError("sorgenti degli inventari non tracciate")
    for rel in paths:
        relative = PurePosixPath(rel)
        path = root / relative
        if relative.parts[0] in {"scripts", "crates", "fuzz"}:
            if path.suffix == ".py":
                source = path.read_text("utf-8")
                names = python_tests(source)
                scripts[rel] = names
            if path.suffix == ".rs" and ("tests" in relative.parts or "src" in relative.parts):
                names = re.findall(r'#\[test\]\s*(?:#\[[^\n]+\]\s*)*(?:pub\s+)?fn\s+(\w+)', path.read_text("utf-8"))
                if names:
                    tests[rel] = names
            if "tests" in relative.parts and path.suffix not in {".rs", ".py", ".pyc"}:
                fixtures.append(rel)
        if relative.parent == PurePosixPath(".github/workflows") and relative.suffix == ".yml":
            source = path.read_text("utf-8")
            workflows[path.name] = {
                "events": workflow_events(source),
                "jobs": re.findall(r'^  ([\w-]+):\s*$', source.split("\njobs:\n", 1)[1], re.M),
                "python_scripts": sorted(set(re.findall(r'\b(?:python|python3)\s+(scripts/[\w.-]+\.py)', source))),
            }
    return {"python": scripts, "rust_tests": tests, "fixtures": fixtures,
            "workflows": workflows, "fuzz": target_fuzz(root, paths), "sonde_sdk": sonde_sdk(root)}


def verifica_fuzz(root):
    expected = target_fuzz(root)
    source = (root / ".github/workflows/fuzz.yml").read_text("utf-8")
    match = re.search(r'^        target:\n((?:          - [\w]+\n)+)', source, re.M)
    if not match or unici(re.findall(r'- (\w+)', match[1]), "matrice fuzz") != expected:
        raise ValueError("matrice fuzz diversa dai binari del manifest")
    # Il generatore deve produrre semi per ciascun target, senza target fantasma.
    names = set()
    for name in ("genera_corpus_fuzz.py", "corpus_definizioni_crs.py"):
        tree = ast.parse((root / "scripts" / name).read_text("utf-8"))
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "salva":
                if not node.args or not isinstance(node.args[0], ast.Constant):
                    raise ValueError("target corpus non letterale: aggiornare la verifica")
                names.add(node.args[0].value)
    if names != set(expected):
        raise ValueError("target corpus diversi dai binari del manifest")


def verifica_crs(root):
    expected = set(crs_integrati(root))
    for name in ("crates/plenora-io/data/projjson_integrati.json",
                 "crates/plenora-core/src/crs/definizioni/wkt_corpus.json"):
        if set(strict_json.loads((root / name).read_text("utf-8"))) != expected:
            raise ValueError("corpus CRS diverso dai sorgenti")


def verifica_copie(root):
    errors = []
    for path in sorted((root / "crates").rglob("*provenienza.json"),
                       key=lambda p: p.relative_to(root).as_posix()):
        doc = strict_json.loads(path.read_text("utf-8"))
        entries = doc.get("file", doc.get("files"))
        if entries is None:
            continue  # Rapporti storici, non manifesti di copie locali.
        folder = path.parent
        if path.name == "primary-composition-provenienza.json":
            folder /= "primary-composition-v1"
        if path.name == "provenienza.json" or folder.name == "primary-composition-v1":
            actual = {p.name for p in folder.iterdir() if p.is_file() and p != path}
            if actual != set(entries):
                errors.append(f"{path.relative_to(root).as_posix()}: inventario copie")
        for name, value in entries.items():
            digest = value[-1] if isinstance(value, list) else value
            copy = folder / name
            if not copy.is_file() or hashlib.sha256(copy.read_bytes()).hexdigest() != digest:
                errors.append(f"{path.relative_to(root).as_posix()}: digest copia")
    if errors:
        raise ValueError("; ".join(errors))


def verifica_copertura(root):
    budget = strict_json.loads((root / "scripts/coverage_budget.json").read_text("utf-8"))
    manifest = tomllib.loads((root / "Cargo.toml").read_text("utf-8"))
    members = manifest["workspace"]["members"]
    actual = set()
    for pattern in members:
        for member in root.glob(pattern):
            if (member / "src").is_dir():
                actual.add(member.relative_to(root).as_posix() + "/src/")
    surfaces = budget["surfaces"]
    covered = surfaces["rust_product"]["sources"] + surfaces["python_binding"]["sources"]
    if len(covered) != len(set(covered)) or set(covered) != actual:
        raise ValueError("copertura: sorgenti diverse dai membri del workspace")


def verifica_ci(root):
    ci = (root / ".github/workflows/ci.yml").read_text("utf-8")
    if "pull_request" not in workflow_events(ci):
        raise ValueError("gate senza trigger PR")
    trigger = ci.split("  pull_request:", 1)[1].split("\n\n", 1)[0]
    if re.search(r'\b(?:paths|paths-ignore|branches|branches-ignore):', trigger):
        raise ValueError("gate PR filtrato")
    block = ci.split("  guardie:\n", 1)[1].split("\n  gate:", 1)[0]
    if not re.search(r'^        run: python scripts/esegui_guardie.py$', block, re.M) or re.search(r'^\s+if:', block, re.M):
        raise ValueError("runner guardie non incondizionato")
    for path in sorted((root / ".github/workflows").glob("*.yml"), key=lambda p: p.name):
        text = path.read_text("utf-8")
        workflow_events(text)
        for script in re.findall(r'\b(?:python|python3)\s+(scripts/[\w.-]+\.py)', text):
            if not (root / script).is_file():
                raise ValueError("workflow richiama uno script assente")
        if "matrix:" in text and "fail-fast: false" not in text:
            raise ValueError("matrice workflow senza raccolta completa")


def verifica_collegamenti(root):
    scripts = {p.stem: p for p in (root / "scripts").glob("*.py")}
    roots = set()
    for path in sorted((root / ".github/workflows").glob("*.yml"), key=lambda p: p.name):
        source = path.read_text("utf-8")
        roots.update(re.findall(r'^(?:        run: |          )(?:python|python3)\s+scripts/([\w.-]+)\.py', source, re.M))
    # La discovery del runner esegue tutti i test_*.py: il suo comando
    # e' verificato separatamente, non basta un riferimento in prosa.
    import esegui_guardie
    if ["-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"] not in esegui_guardie.comandi():
        raise ValueError("discovery self-test rimossa")
    roots.update(p.stem for p in (root / "scripts").glob("test_*.py"))
    roots.update(Path(command[0]).stem for command in esegui_guardie.comandi()
                 if command[0].startswith("scripts/"))
    probes = {name for name in scripts if name.startswith(("check_", "check-", "verifica_", "test_"))}
    if probes - roots:
        raise ValueError("prove senza collegamento CI: " + ", ".join(sorted(probes - roots)))


def verifica_conteggi_rust(root):
    # Queste assertion restano oracoli indipendenti nei test Rust. Il gate
    # statico impedisce che il numero copiato resti un contratto obsoleto.
    import genera_inventario
    expected_crs = len(crs_integrati(root))
    expected_catalog = len(genera_inventario.voci_catalogo())
    for path in sorted((root / "crates").rglob("*.rs"), key=lambda p: p.relative_to(root).as_posix()):
        source = path.read_text("utf-8")
        counts = re.findall(r'assert_eq!\(CATALOG\.len\(\),\s*(\d+)\)', source)
        if any(int(n) != expected_catalog for n in counts):
            raise ValueError("conteggio del catalogo Rust obsoleto")
        if any(marker in source for marker in ("wkt_corpus.json", "projjson_integrati.json")) or path.as_posix().endswith("crs/riproiezione/tests.rs"):
            counts = re.findall(r'assert_eq!\((?:contati|(?:ids|tabella|corpus|proj|wkt)(?:\.as_object\(\)\.unwrap\(\))?\.len\(\)),\s*(\d+)', source)
            if any(int(n) != expected_crs for n in counts):
                raise ValueError("conteggio dei corpus CRS Rust obsoleto")


def verifica_evidenze(root):
    source = strict_json.loads((root / "crates/plenora-cli/adozione.json").read_text("utf-8"))
    def strings(value):
        if isinstance(value, str):
            yield value
        elif isinstance(value, list):
            for item in value:
                yield from strings(item)
        elif isinstance(value, dict):
            for key, item in value.items():
                if key in {"verification", "evidence"}:
                    yield from strings(item)
                elif isinstance(item, (dict, list)):
                    yield from strings(item)
    for text in strings(source):
        for relative in re.findall(r'\b((?:crates|scripts|docs|tests)/[\w./-]+\.(?:rs|py|md))', text):
            path = root / relative
            if relative.startswith("tests/") and not path.is_file():
                path = root / "crates/plenora-cli" / relative
            if not path.is_file():
                raise ValueError("evidenza locale assente: " + relative)


def check(root=ROOT, write=False):
    errors = []
    for function in (verifica_fuzz, verifica_crs, verifica_copie, verifica_copertura, verifica_ci,
                     verifica_collegamenti, verifica_conteggi_rust, verifica_evidenze):
        try:
            function(root)
        except (ValueError, KeyError, IndexError, OSError) as error:
            errors.append(f"{function.__name__}: {error}")
    try:
        rendered = json.dumps(inventario(root), ensure_ascii=False, indent=2) + "\n"
        if write:
            (root / DEST).write_text(rendered, encoding="utf-8", newline="\n")
        elif not (root / DEST).is_file() or (root / DEST).read_text("utf-8") != rendered:
            errors.append("censimento non aggiornato: python scripts/check_prove_fonti.py --scrivi")
    except (ValueError, KeyError, IndexError, OSError, SyntaxError, subprocess.CalledProcessError) as error:
        errors.append(f"censimento: {error}")
    return errors


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scrivi", action="store_true")
    args = parser.parse_args()
    problems = check(write=args.scrivi)
    print("\n".join(problems) if problems else "fonti delle prove: OK")
    sys.exit(bool(problems))
