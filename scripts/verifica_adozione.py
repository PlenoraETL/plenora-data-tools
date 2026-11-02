"""Verifica offline pin, copie esatte e ambito dell'adozione dei contratti.

Uso: python scripts/verifica_adozione.py --contratti <checkout>
Il checkout deve essere gia' disponibile; non scarica dipendenze o sorgenti.
"""

from __future__ import annotations

import argparse
import hashlib
import subprocess
import sys
from pathlib import Path

from genera_manifesto_adozione import deviazioni, valida, verifica_checkout

try:
    from . import strict_json
except ImportError:
    import strict_json

RADICE = Path(__file__).resolve().parents[1]


def prove_contratti(contratti: Path) -> None:
    """Esegue anche le prove del gate esterno dichiarate nell'adozione."""
    for nome in (
        "validate_specs.py", "test_messages.py", "test_gate_fuzz.py",
        "test_message_sentinels.py",
    ):
        esito = subprocess.run(
            [sys.executable, "-B", str(contratti.resolve() / "tools" / nome)],
            capture_output=True, check=False,
        )
        # I test esterni possono includere dati nelle assertion: non ristamparli.
        if esito.returncode != 0:
            print(f"contratti: {nome} fallita", flush=True)
            raise ValueError("prova dei contratti fallita")
        print(f"contratti: {nome} ok", flush=True)


def verifica_regole_geometriche(sorgente: dict) -> None:
    """Ogni regola adottata ha uno stato e ogni deviazione e pubblicata."""
    contratto = "plenora-arrow-geometry-semantics-v1"
    if contratto not in sorgente["conforming"] or contratto in sorgente["not_applicable"]:
        raise ValueError("contratto geometrico non adottato univocamente")
    regole = {f"GEO-{i:03}" for i in range(21)} | {f"REJ-{i:03}" for i in range(1, 5)}
    censite = [r for r in sorgente["requirements"] if any(x in regole for x in r["rules"])]
    if len([x for r in censite for x in r["rules"]]) != 25 or {x for r in censite for x in r["rules"]} != regole:
        raise ValueError("censimento geometrico incompleto o ripetuto")
    for requisito in censite:
        if requisito["status"] == "deviation":
            if not all(requisito.get(k) for k in ("scope", "hazard", "reentry")):
                raise ValueError("deviazione geometrica senza ambito, hazard o rientro")
            if not any(d["rule"] == requisito["rules"][0] for d in sorgente["deviations"]):
                raise ValueError("deviazione geometrica non pubblicata")
        elif requisito["status"] != "implemented":
            raise ValueError("regola geometrica non dichiarata")
        elif any(d["rule"] == requisito["rules"][0] for d in sorgente["deviations"]):
            raise ValueError("regola geometrica conforme e deviata insieme")


def verifica(contratti: Path) -> None:
    sorgente = strict_json.loads((RADICE / "crates/plenora-cli/adozione.json").read_text("utf-8"))
    revisione = sorgente["contracts_source"]["revision"]
    verifica_checkout(contratti, revisione)
    cartella = RADICE / "crates/plenora-cli/tests/fixtures/contratti"
    provenienza = strict_json.loads((cartella / "provenienza.json").read_text("utf-8"))
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
    geometria = RADICE / "crates/plenora-cli/tests/fixtures/arrow-data-v1"
    prova = strict_json.loads((geometria / "provenienza.json").read_text("utf-8"))
    if prova["commit"] != revisione:
        raise ValueError("pin dei vettori geometrici incoerente")
    originali = subprocess.run(
        ["git", "-C", str(contratti), "ls-tree", "-r", "--name-only", revisione,
         "vectors/arrow-data-v1"], capture_output=True, check=True, text=True,
    ).stdout.splitlines()
    nomi = {Path(nome).name for nome in originali}
    if nomi != set(prova["file"]) or {p.name for p in geometria.iterdir()} != nomi | {"provenienza.json"}:
        raise ValueError("inventario dei vettori geometrici incoerente")
    for percorso in originali:
        nome = Path(percorso).name
        originale = subprocess.run(
            ["git", "-C", str(contratti), "show", f"{revisione}:{percorso}"],
            capture_output=True, check=True,
        ).stdout
        copia = (geometria / nome).read_bytes()
        if copia != originale or hashlib.sha256(copia).hexdigest() != prova["file"][nome]:
            raise ValueError("vettore geometrico o digest incoerente")
    verifica_regole_geometriche(sorgente)
    catalogo = strict_json.loads((cartella / "data-tools-v3.json").read_text("utf-8"))
    from jsonschema import Draft202012Validator

    schema = strict_json.loads((cartella / "public-catalog-v2.schema.json").read_text("utf-8"))
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
        "deviations": deviazioni(sorgente),
    }
    valida(documento, contratti)
    prove_contratti(contratti)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--contratti", type=Path, required=True)
    args = parser.parse_args()
    try:
        verifica(args.contratti)
    except Exception:
        # Non propagare valori letti o diagnostica delle dipendenze nel gate.
        print("adozione: verifica fallita (pin, copie, catalogo, manifesto o prove dei contratti)")
        return 1
    print("adozione: pin, copie SHA-256, catalogo, requisiti, schema manifesto e prove dei contratti validi")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
