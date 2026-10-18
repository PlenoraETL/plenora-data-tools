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

`--artefatto NOME|SUPERFICIE|PERCORSO` si ripete (la superficie Rust è
l'archivio del sorgente, `plenora-data-tools-<versione>-source.tar.gz`, che il
workflow di rilascio produce con `git archive`). Un wheel dell'SDK Python
(`crates/plenora-data-py`) si passa con la superficie `python_sdk` e i suoi
modi d'API (adoption manifest v4, `api_modes`):

    --artefatto "plenora_data-0.1.0-cp310-abi3-win_amd64.whl|python_sdk|dist/plenora_data-0.1.0-cp310-abi3-win_amd64.whl|sync,async"

Il nome del wheel deve portare `--versione`. La sorgente dichiara
`plenora-python-sdk-v1` conforme: senza almeno un wheel il manifesto non si
scrive (un contratto conforme senza l'artefatto che lo prova). Una
deviazione della sorgente con `surfaces` vale per ognuna delle superfici
elencate e diventa una deviazione per superficie. Non scrive nulla se il
checkout dei contratti non è al commit della sorgente.
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
MODI_API = {"sync", "async"}
CONTRATTO_PYTHON = "plenora-python-sdk-v1"


def digest(percorso: Path) -> str:
    valore = hashlib.sha256()
    with percorso.open("rb") as file:
        for blocco in iter(lambda: file.read(1024 * 1024), b""):
            valore.update(blocco)
    return f"sha256:{valore.hexdigest()}"


def artefatto(testo: str, versione: str, verifiche: list[str]) -> dict:
    parti = testo.split("|")
    if len(parti) not in (3, 4):
        raise SystemExit("--artefatto: atteso NOME|SUPERFICIE|PERCORSO[|MODI]")
    nome, superficie, percorso = parti[:3]
    if superficie not in SUPERFICI:
        raise SystemExit("--artefatto: superficie sconosciuta")
    percorso = Path(percorso).resolve()
    if not percorso.is_file():
        raise SystemExit("--artefatto: file assente")
    voce = {
        "name": nome,
        "surface": superficie,
        "version": versione,
        "digest": digest(percorso),
        "verification": verifiche,
    }
    if superficie == "python_sdk":
        if len(parti) != 4:
            raise SystemExit("--artefatto: un wheel python_sdk vuole i modi d'API (sync,async)")
        modi = parti[3].split(",")
        if not modi or len(set(modi)) != len(modi) or not set(modi) <= MODI_API:
            raise SystemExit("--artefatto: modi d'API ammessi: sync, async")
        if not percorso.name.startswith(f"plenora_data-{versione}-") or percorso.suffix != ".whl":
            raise SystemExit("--artefatto: il wheel non porta la versione data")
        voce["api_modes"] = modi
    elif len(parti) == 4:
        raise SystemExit("--artefatto: i modi d'API valgono solo per python_sdk")
    return voce


def deviazioni(sorgente: dict) -> list[dict]:
    """Le deviazioni della sorgente, una per superficie."""
    uscita = []
    for deviazione in sorgente["deviations"]:
        superfici = deviazione.get("surfaces")
        if superfici is None:
            uscita.append(deviazione)
            continue
        base = {chiave: valore for chiave, valore in deviazione.items() if chiave != "surfaces"}
        uscita.extend({**base, "surface": superficie} for superficie in superfici)
    return uscita


def manifesto(sorgente: dict, versione: str, artefatti: list[str], verifiche: list[str]) -> dict:
    voci = [artefatto(testo, versione, verifiche) for testo in artefatti]
    python = any(voce["surface"] == "python_sdk" for voce in voci)
    if (CONTRATTO_PYTHON in sorgente["conforming"]) != python:
        raise SystemExit(
            f"{CONTRATTO_PYTHON} conforme vuole almeno un artefatto python_sdk, e viceversa"
        )
    return {
        "schema_version": 4,
        "component": sorgente["component"],
        "contracts_source": sorgente["contracts_source"],
        "profile": sorgente["profile"],
        "artifacts": voci,
        "contracts": [
            {"id": contratto, "status": "conforming", "verification": verifiche}
            for contratto in sorgente["conforming"]
        ]
        + [{"id": contratto, "status": "not_applicable"} for contratto in sorgente["not_applicable"]],
        "deviations": deviazioni(sorgente),
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
    # HEAD giusto non basta: i confronti SHA-256 leggono i blob Git, ma le
    # prove del pin eseguono i file del worktree. Un file modificato -- o
    # anche solo aggiunto: un modulo ombra cambia cio' che Python importa --
    # renderebbe verde un gate che non ha eseguito le prove adottate.
    #
    # La classe intera e' «stato del worktree nascosto al controllo», e si
    # chiude qui per i modi che git offre di nasconderlo:
    # `--untracked-files=all` vince su `status.showUntrackedFiles=no`;
    # `--ignored=matching` mostra anche i file esclusi da `.gitignore` o da
    # `core.excludesFile` (un `__pycache__` ignorato conta: Python legge i
    # .pyc esistenti anche con -B); le strutture di accelerazione --
    # fsmonitor e cache degli untracked -- si disabilitano per questa
    # invocazione, perche' una cache stantia o bugiarda farebbe credere
    # pulito un file modificato: la scansione e' piena per costruzione.
    # `ls-files -v` scopre i bit assume-unchanged e skip-worktree, e
    # `ls-files -f` i fsmonitor-valid, che nascondono le modifiche ai file
    # tracciati. Chi controlla la macchina puo' alterare il gate stesso: il
    # controllo prova l'integrita' del checkout, non difende da un ostile.
    stato = subprocess.run(
        [
            "git", "-C", str(contratti),
            "-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false",
            "status", "--porcelain",
            "--untracked-files=all", "--ignored=matching",
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if stato:
        raise SystemExit(
            "il checkout dei contratti non e' pulito (anche un file ignorato "
            "conta): le prove del pin si eseguono solo su un worktree "
            "identico alla revisione fissata"
        )
    for opzione, nomi in (("-v", "assume-unchanged o skip-worktree"),
                          ("-f", "fsmonitor-valid")):
        voci = subprocess.run(
            ["git", "-C", str(contratti), "ls-files", opzione],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        if any(riga and (riga[0].islower() or riga[0] == "S") for riga in voci):
            raise SystemExit(
                f"il checkout dei contratti ha voci {nomi}: lo stato "
                "nascosto all'indice non si accetta"
            )


def valida(documento: dict, contratti: Path) -> None:
    from jsonschema import Draft202012Validator

    schema = json.loads(
        (contratti / "schemas" / "adoption-manifest-v4.schema.json").read_text(encoding="utf-8")
    )
    Draft202012Validator.check_schema(schema)
    errori = list(Draft202012Validator(schema).iter_errors(documento))
    if errori:
        raise SystemExit(f"manifesto non valido per lo schema v4: {len(errori)} errori")
    sys.path.insert(0, str(contratti / "tools"))
    specifica = importlib.util.spec_from_file_location(
        "conformance_checks", contratti / "tools" / "conformance_checks.py"
    )
    modulo = importlib.util.module_from_spec(specifica)
    specifica.loader.exec_module(modulo)
    semantici = modulo.adoption_errors(documento)
    if semantici:
        raise SystemExit(f"manifesto non valido nei controlli semantici: {len(semantici)} errori")


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
