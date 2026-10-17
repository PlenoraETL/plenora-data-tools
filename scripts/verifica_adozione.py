"""Verifica offline pin, copie esatte e ambito dell'adozione dei contratti.

Uso: python scripts/verifica_adozione.py --contratti <checkout>
Il checkout deve essere gia' disponibile; non scarica dipendenze o sorgenti.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from genera_manifesto_adozione import valida, verifica_checkout

RADICE = Path(__file__).resolve().parents[1]


def verifica(contratti: Path) -> None:
    sorgente = json.loads((RADICE / "crates/plenora-cli/adozione.json").read_text("utf-8"))
    revisione = sorgente["contracts_source"]["revision"]
    verifica_checkout(contratti, revisione)
    cartella = RADICE / "crates/plenora-cli/tests/fixtures/contratti"
    provenienza = json.loads((cartella / "provenienza.json").read_text("utf-8"))
    if provenienza["commit"] != revisione:
        raise ValueError("pin di provenienza incoerente")
    if {p.name for p in cartella.iterdir()} != set(provenienza["file"]) | {"provenienza.json"}:
        raise ValueError("inventario delle copie incoerente")
    for nome, (percorso, digest) in provenienza["file"].items():
        originale = subprocess.run(
            ["git", "-C", str(contratti), "show", f"{revisione}:{percorso}"],
            capture_output=True, check=True,
        ).stdout
        copia = (cartella / nome).read_bytes()
        if copia != originale or hashlib.sha256(copia).hexdigest() != digest:
            raise ValueError("copia dei contratti o digest incoerente")
    catalogo = json.loads((cartella / "data-tools-v3.json").read_text("utf-8"))
    from jsonschema import Draft202012Validator

    schema = json.loads((cartella / "public-catalog-v2.schema.json").read_text("utf-8"))
    Draft202012Validator.check_schema(schema)
    if not Draft202012Validator(schema).is_valid(catalogo):
        raise ValueError("catalogo non valido")
    if catalogo["profile"] != sorgente["profile"]:
        raise ValueError("profilo incoerente")
    if catalogo["contract"] not in sorgente["conforming"]:
        raise ValueError("contratto del catalogo non adottato")
    for operazione in catalogo["operations"]:
        if operazione["id"] == "data.run":
            vietate = {"cli", "python_sdk"} if operazione["version"] == 3 else {"runtime"}
            if vietate.intersection(operazione["surfaces"]):
                raise ValueError("DT-ABS-001/DT-ABS-002")
    for relativo in ["scripts/verifica_cli_contratti.py", ".github/workflows/ci.yml",
                     ".github/workflows/rilascio.yml"]:
        if revisione not in (RADICE / relativo).read_text("utf-8"):
            raise ValueError("pin del gate incoerente")
    for requisito in sorgente["requirements"]:
        if requisito["status"] not in {"implemented", "deviation", "not_applicable"}:
            raise ValueError("stato del requisito non valido")
        if not requisito["reason"]:
            raise ValueError("motivazione del requisito assente")
    # Valida la forma del manifesto con un artefatto di prova esplicitamente
    # identificato: non e' un manifesto di rilascio e non viene pubblicato.
    documento = {
        "schema_version": 4, "component": sorgente["component"],
        "contracts_source": sorgente["contracts_source"], "profile": sorgente["profile"],
        "artifacts": [{"name": "verifica-adozione", "surface": "rust", "version": "0.0.0",
                       "digest": "sha256:" + hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                       "verification": ["python scripts/verifica_adozione.py"]}],
        "contracts": [{"id": c, "status": "conforming", "verification": ["verifica locale"]}
                      for c in sorgente["conforming"]]
                     + [{"id": c, "status": "not_applicable"} for c in sorgente["not_applicable"]],
        "deviations": sorgente["deviations"],
    }
    valida(documento, contratti)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--contratti", type=Path, required=True)
    args = parser.parse_args()
    try:
        verifica(args.contratti)
    except Exception:
        # Non propagare valori letti o diagnostica delle dipendenze nel gate.
        print("adozione: verifica fallita (pin, copie, catalogo o manifesto)")
        return 1
    print("adozione: pin, copie SHA-256, catalogo, requisiti e schema manifesto validi")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
