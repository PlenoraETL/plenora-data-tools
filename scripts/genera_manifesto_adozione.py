"""Genera il manifesto di adozione v4 dei contratti dagli artefatti costruiti.

La sorgente (`crates/plenora-cli/adozione.json`) tiene il pin dei contratti,
i contratti adottati e le deviazioni; versione e digest SHA-256 vengono dagli
artefatti esatti passati qui (ADOPTION.md, «Immutable artifact identity»):
scriverli prima della build darebbe un manifesto valido nella forma e falso
nella sostanza. Il manifesto si valida contro lo schema v4 e i controlli
semantici di `tools/conformance_checks.py`, presi dal checkout dei contratti
al commit fissato.

Uso (con il Python che ha `jsonschema`, per esempio il venv dei contratti):

    python scripts/genera_manifesto_adozione.py \
        --contratti ../plenora-contracts \
        --versione 0.1.0 \
        --artefatto "plenora-data|cli|target/release/plenora-data.exe" \
        --verifica "cargo test -p plenora-cli --locked" \
        --uscita manifesto-adozione.json

`--artefatto NOME|SUPERFICIE|PERCORSO` si ripete (la superficie Rust è un
archivio `.crate` di `cargo package`). Non scrive nulla se il checkout dei
contratti non è al commit della sorgente.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

RADICE = Path(__file__).resolve().parents[1]
SORGENTE = RADICE / "crates" / "plenora-cli" / "adozione.json"
SUPERFICI = {"rust", "cli", "python_sdk", "runtime"}


def digest(percorso: Path) -> str:
    valore = hashlib.sha256()
    with percorso.open("rb") as file:
        for blocco in iter(lambda: file.read(1024 * 1024), b""):
            valore.update(blocco)
    return f"sha256:{valore.hexdigest()}"


def artefatto(testo: str, versione: str, verifiche: list[str]) -> dict:
    parti = testo.split("|")
    if len(parti) != 3:
        raise SystemExit("--artefatto: atteso NOME|SUPERFICIE|PERCORSO")
    nome, superficie, percorso = parti
    if superficie not in SUPERFICI or superficie == "python_sdk":
        raise SystemExit("--artefatto: superficie non ammessa per questo componente")
    percorso = Path(percorso).resolve()
    if not percorso.is_file():
        raise SystemExit("--artefatto: file assente")
    return {
        "name": nome,
        "surface": superficie,
        "version": versione,
        "digest": digest(percorso),
        "verification": verifiche,
    }


def manifesto(sorgente: dict, versione: str, artefatti: list[str], verifiche: list[str]) -> dict:
    return {
        "schema_version": 4,
        "component": sorgente["component"],
        "contracts_source": sorgente["contracts_source"],
        "profile": sorgente["profile"],
        "artifacts": [artefatto(testo, versione, verifiche) for testo in artefatti],
        "contracts": [
            {"id": contratto, "status": "conforming", "verification": verifiche}
            for contratto in sorgente["conforming"]
        ]
        + [{"id": contratto, "status": "not_applicable"} for contratto in sorgente["not_applicable"]],
        "deviations": sorgente["deviations"],
    }


def verifica_checkout(contratti: Path, revisione: str) -> None:
    testa = subprocess.run(
        ["git", "-C", str(contratti), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if testa != revisione:
        raise SystemExit("il checkout dei contratti non e' al commit fissato nella sorgente")


def valida(documento: dict, contratti: Path) -> None:
    from jsonschema import Draft202012Validator

    schema = json.loads(
        (contratti / "schemas" / "adoption-manifest-v4.schema.json").read_text(encoding="utf-8")
    )
    Draft202012Validator.check_schema(schema)
    errori = list(Draft202012Validator(schema).iter_errors(documento))
    if errori:
        raise SystemExit(f"manifesto non valido per lo schema v4: {len(errori)} errori")
    specifica = importlib.util.spec_from_file_location(
        "conformance_checks", contratti / "tools" / "conformance_checks.py"
    )
    modulo = importlib.util.module_from_spec(specifica)
    specifica.loader.exec_module(modulo)
    semantici = modulo.adoption_errors(documento)
    if semantici:
        raise SystemExit(f"manifesto non valido nei controlli semantici: {semantici}")


def main() -> int:
    argomenti = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    argomenti.add_argument("--contratti", type=Path, required=True)
    argomenti.add_argument("--versione", required=True)
    argomenti.add_argument("--artefatto", action="append", required=True)
    argomenti.add_argument("--verifica", action="append", required=True)
    argomenti.add_argument("--uscita", type=Path, required=True)
    letti = argomenti.parse_args()
    sorgente = json.loads(SORGENTE.read_text(encoding="utf-8"))
    verifica_checkout(letti.contratti, sorgente["contracts_source"]["revision"])
    documento = manifesto(sorgente, letti.versione, letti.artefatto, letti.verifica)
    valida(documento, letti.contratti)
    letti.uscita.write_text(
        json.dumps(documento, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
