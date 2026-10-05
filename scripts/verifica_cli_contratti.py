"""Verifica indipendente della CLI `plenora-data` con `jsonschema`.

I test Rust della CLI (`crates/plenora-cli/tests/`) validano con un
validatore minimo scritto nel workspace; questo script ripete le verifiche di
scoperta e d'errore con l'implementazione di riferimento di JSON Schema
(`jsonschema`, draft 2020-12) e con i controlli semantici dei contratti
(`tools/conformance_checks.py`), leggendo schemi e cataloghi dal checkout di
`plenora-contracts` al commit fissato.

Uso (con il Python che ha `jsonschema` e `referencing`, per esempio il venv
dei contratti):

    python scripts/verifica_cli_contratti.py \
        --binario <target>/debug/plenora-data.exe --contratti ../plenora-contracts

Esce con 0 se ogni verifica passa; altrimenti stampa le verifiche fallite ed
esce con 1.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path

REVISIONE = "23fed27d5736e5f32906a0116553fed16a1fb239"


class Verifica:
    def __init__(self, contratti: Path, binario: Path) -> None:
        from jsonschema import Draft202012Validator
        from referencing import Registry, Resource

        self.contratti = contratti
        self.binario = binario
        self.fallite: list[str] = []
        schemi = {}
        for nome in [
            "cli-envelope-v2",
            "error-v1",
            "capabilities-v2",
            "row-diagnostics-v1",
            "operation-registry-v1",
        ]:
            schema = json.loads(
                (contratti / "schemas" / f"{nome}.schema.json").read_text(encoding="utf-8")
            )
            Draft202012Validator.check_schema(schema)
            schemi[nome] = schema
        registro = Registry().with_resources(
            (schema["$id"], Resource.from_contents(schema)) for schema in schemi.values()
        )
        self.validatori = {
            nome: Draft202012Validator(schema, registry=registro) for nome, schema in schemi.items()
        }
        specifica = importlib.util.spec_from_file_location(
            "conformance_checks", contratti / "tools" / "conformance_checks.py"
        )
        self.semantica = importlib.util.module_from_spec(specifica)
        specifica.loader.exec_module(self.semantica)

    def controlla(self, condizione: bool, cosa: str) -> None:
        if not condizione:
            self.fallite.append(cosa)

    def valido(self, schema: str, documento: object, cosa: str) -> None:
        errori = list(self.validatori[schema].iter_errors(documento))
        self.controlla(not errori, f"{cosa}: {len(errori)} errori contro {schema}")

    def invoca(self, *argomenti: str) -> tuple[int, dict]:
        esito = subprocess.run(
            [str(self.binario), *argomenti], capture_output=True, check=False
        )
        cosa = " ".join(argomenti) or "(nessun argomento)"
        self.controlla(esito.stderr == b"", f"{cosa}: stderr non vuoto")
        testo = esito.stdout.decode("utf-8")
        self.controlla(
            testo.endswith("\n") and testo.count("\n") == 1, f"{cosa}: non un solo documento"
        )
        documento = json.loads(testo)
        self.valido("cli-envelope-v2", documento, cosa)
        self.controlla(
            (esito.returncode == 0) == (documento["status"] == "ok"),
            f"{cosa}: exit 0 se e solo se ok",
        )
        if documento["status"] == "error":
            errore = documento["error"]
            self.controlla(
                len(json.dumps(errore, separators=(",", ":")).encode()) <= 524_288,
                f"{cosa}: ERR-011",
            )
            diagnostica = errore.get("details", {}).get("row_diagnostics")
            if diagnostica is not None:
                self.valido("row-diagnostics-v1", diagnostica, f"{cosa}: row_diagnostics")
        return esito.returncode, documento


def verifica(contratti: Path, binario: Path) -> list[str]:
    v = Verifica(contratti, binario)
    codice, documento = v.invoca("--version", "--format", "json")
    v.controlla(codice == 0 and documento["result"]["protocol_version"] == 2, "version")

    codice, documento = v.invoca("capabilities", "--format", "json")
    capacita = documento["result"]
    v.valido("capabilities-v2", capacita, "capabilities")
    v.controlla(not v.semantica.capability_errors(capacita), "capabilities: CAP-005/CAP-007")
    catalogo = json.loads((contratti / "catalogs" / "data-tools-v2.json").read_text("utf-8"))
    pubbliche = {
        (op["id"], op["version"], op["input"]["contract"], op["output"]["contract"], op["side_effect"])
        for op in catalogo["operations"]
        if "cli" in op["surfaces"]
    }
    nostre = {
        (op["id"], op["version"], op["input"]["contract"], op["output"]["contract"], op["side_effect"])
        for op in capacita["operations"]
    }
    v.controlla(pubbliche == nostre, "capabilities: operazioni CLI, contratti ed effetti del catalogo v2")

    codice, documento = v.invoca("catalog", "--format", "json")
    v.controlla(documento["contract"] == "plenora-data-catalog-result-v2", "catalog: contratto del risultato")
    v.valido("operation-registry-v1", documento["result"]["registry"], "catalog: registry")
    registro = json.loads((contratti / "catalogs" / "data-kernels-v2.json").read_text("utf-8"))
    comuni = {(op["id"], op["version"], op["family"]) for op in registro["operations"]}
    kernel = documento["result"]["kernels"]
    nostri = {(k["id"], k["version"], k["family"]) for k in kernel}
    v.controlla(comuni == nostri, "catalog: id, versioni e famiglie del registro v2 (DT-001)")
    disponibili = {(k["id"], k["version"], k["family"]) for k in kernel if k["status"] == "available"}
    nel_registro = {(op["id"], op["version"], op["family"]) for op in documento["result"]["registry"]["operations"]}
    v.controlla(disponibili == nel_registro, "catalog: registry = kernel disponibili (DT-001)")
    v.controlla(
        all(k.get("reason") for k in kernel if k["status"] != "available"),
        "catalog: motivo dei kernel non disponibili (DT-001)",
    )

    codice, _ = v.invoca("--help", "--format", "json")
    v.controlla(codice == 0, "help json")
    codice, documento = v.invoca("frobnicate")
    v.controlla(
        codice == 2 and documento["error"]["category"] == "invalid_configuration",
        "comando sconosciuto",
    )
    with tempfile.TemporaryDirectory() as cartella:
        mancante = str(Path(cartella) / "manca.arrow")
        codice, documento = v.invoca("describe", "--input", mancante, "--format", "json")
        v.controlla(codice == 5 and documento["error"]["category"] == "not_found", "describe")
        v.controlla(cartella not in json.dumps(documento), "describe: percorso nel messaggio")
    return v.fallite


def main() -> int:
    argomenti = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    argomenti.add_argument("--binario", type=Path, required=True)
    argomenti.add_argument("--contratti", type=Path, required=True)
    letti = argomenti.parse_args()
    testa = subprocess.run(
        ["git", "-C", str(letti.contratti), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if testa != REVISIONE:
        print("il checkout dei contratti non e' al commit fissato")
        return 1
    fallite = verifica(letti.contratti, letti.binario)
    for cosa in fallite:
        print(f"FALLITA: {cosa}")
    print(f"{'ok' if not fallite else 'errori'}: {len(fallite)} verifiche fallite")
    return 1 if fallite else 0


if __name__ == "__main__":
    sys.exit(main())
