"""Verifica del wheel dell'SDK Python `plenora-data`, installato.

Python SDK 1.0, sezione 10: il wheel si prova dopo l'installazione, in un
ambiente che non importa il checkout per sbaglio. Lo script:

1. legge il wheel: nome (`plenora_data-<versione>-cp310-abi3-<piattaforma>`),
   versione dei metadati, marcatore PEP 561 (`py.typed`), stub del modulo
   nativo (`_native.pyi`), modulo nativo;
2. confronta la versione con quella del workspace (`Cargo.toml`, la fonte
   di `plenora_data.version()`);
3. in un processo nuovo, fuori dal checkout, importa il pacchetto
   installato: deve venire da `site-packages`, `version()` deve uguagliare
   i metadati installati e il wheel, e il modulo nativo installato deve
   essere byte per byte quello del wheel;
4. copia `crates/plenora-data-py/python/tests` in una cartella di lavoro e
   ci esegue pytest con `PLENORA_DATA_CONTRATTI` (le copie dei contratti con
   SHA-256): fallisce con un test fallito, saltato o con nessun test
   eseguito (un controllo saltato non è un controllo passato);
5. le sonde delle prove (`_native._sonda_consegna`, `_native._sonda_lavoro`,
   feature Cargo `sonde-di-prova`): un wheel delle prove le deve avere; con
   `--rilascio` il wheel non le deve avere, né come attributi del modulo né
   come nomi nel binario nativo, e la suite gira senza le prove marcate
   `sonde`, che devono essere almeno una e le uniche deselezionate.

Uso, con il wheel già installato nell'interprete che esegue lo script e le
dipendenze di requirements-sdk-tests.txt:

    python scripts/verifica_sdk_python.py --wheel dist/plenora_data-...whl
    python scripts/verifica_sdk_python.py --wheel dist/plenora_data-...whl --rilascio

Esce con 0 se tutto passa, 1 altrimenti.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

RADICE = Path(__file__).resolve().parents[1]
PROVE = RADICE / "crates" / "plenora-data-py" / "python" / "tests"
CONTRATTI = RADICE / "crates" / "plenora-cli" / "tests" / "fixtures" / "contratti"
NOME_WHEEL = re.compile(
    r"^plenora_data-(?P<versione>[0-9][0-9A-Za-z.+-]*)-cp310-abi3-(?P<piattaforma>[A-Za-z0-9_.]+)\.whl$"
)
NATIVO = re.compile(r"^plenora_data/_native(\.[A-Za-z0-9_-]+)*\.(pyd|so)$")
# Le sonde delle prove: nomi nel modulo nativo e testi che solo il loro
# codice porta. Nessuno deve stare nel binario di un wheel di rilascio.
SONDE = ("_sonda_consegna", "_sonda_lavoro")
TRACCE_DELLE_SONDE = (*(nome.encode() for nome in SONDE), b"sonda del lavoro")


def versione_del_workspace() -> str:
    testo = (RADICE / "Cargo.toml").read_text(encoding="utf-8")
    sezione = testo.split("[workspace.package]", 1)[1].split("\n[", 1)[0]
    trovata = re.search(r'^version = "([^"]+)"', sezione, re.MULTILINE)
    if trovata is None:
        raise SystemExit("versione del workspace non trovata in Cargo.toml")
    return trovata.group(1)


def leggi_wheel(wheel: Path, fallite: list[str]) -> tuple[str, str, list[str]]:
    """Versione del wheel, SHA-256 del suo modulo nativo e tracce delle
    sonde trovate nel binario nativo."""
    nome = NOME_WHEEL.match(wheel.name)
    if nome is None:
        fallite.append(f"nome del wheel inatteso: {wheel.name}")
        return "", "", []
    versione = nome.group("versione")
    with zipfile.ZipFile(wheel) as archivio:
        voci = set(archivio.namelist())
        for richiesta in (
            "plenora_data/__init__.py",
            "plenora_data/errors.py",
            "plenora_data/py.typed",
            "plenora_data/_native.pyi",
        ):
            if richiesta not in voci:
                fallite.append(f"wheel senza {richiesta}")
        if any(voce.startswith("tests/") or "/tests/" in voce for voce in voci):
            fallite.append("il wheel contiene le prove")
        nativi = [voce for voce in voci if NATIVO.match(voce)]
        if len(nativi) != 1:
            fallite.append(f"moduli nativi nel wheel: {sorted(nativi)}")
            return versione, "", []
        binario = archivio.read(nativi[0])
        impronta = hashlib.sha256(binario).hexdigest()
        tracce = [traccia.decode() for traccia in TRACCE_DELLE_SONDE if traccia in binario]
        metadati = archivio.read(f"plenora_data-{versione}.dist-info/METADATA").decode("utf-8")
    if f"\nVersion: {versione}\n" not in metadati:
        fallite.append("METADATA del wheel con un'altra versione")
    if "\nRequires-Python: >=3.10\n" not in metadati:
        fallite.append("METADATA del wheel senza Requires-Python >=3.10")
    return versione, impronta, tracce


SONDA = """
import hashlib, importlib.metadata, json, pathlib, sys
import plenora_data
from plenora_data import _native
radice = pathlib.Path(plenora_data.__file__).resolve().parent
nativo = [p for p in radice.iterdir() if p.name.startswith("_native.") and p.suffix in (".pyd", ".so")]
print(json.dumps({
    "origine": str(radice),
    "versione": plenora_data.version(),
    "metadati": importlib.metadata.version("plenora-data"),
    "componente": plenora_data.capabilities()["component_version"],
    "nativo": [hashlib.sha256(p.read_bytes()).hexdigest() for p in nativo],
    "tipi": (radice / "py.typed").is_file() and (radice / "_native.pyi").is_file(),
    "sonde": sorted(nome for nome in ("_sonda_consegna", "_sonda_lavoro") if hasattr(_native, nome)),
}))
"""


def sonda(
    cartella: Path, versione: str, impronta: str, rilascio: bool, fallite: list[str]
) -> None:
    esito = subprocess.run(
        [sys.executable, "-I", "-c", SONDA],
        cwd=cartella,
        capture_output=True,
        text=True,
        check=False,
    )
    if esito.returncode != 0:
        fallite.append(f"import del pacchetto installato non riuscito: {esito.stderr.strip()}")
        return
    trovato = json.loads(esito.stdout)
    origine = Path(trovato["origine"])
    if "site-packages" not in origine.parts or origine.is_relative_to(RADICE):
        fallite.append(f"pacchetto importato da {origine}, non dal wheel installato")
    for chiave in ("versione", "metadati", "componente"):
        if trovato[chiave] != versione:
            fallite.append(f"{chiave} {trovato[chiave]} diversa dal wheel {versione}")
    if trovato["nativo"] != [impronta]:
        fallite.append("il modulo nativo installato non e' quello del wheel")
    if not trovato["tipi"]:
        fallite.append("py.typed o _native.pyi assenti dal pacchetto installato")
    attese = [] if rilascio else sorted(SONDE)
    if trovato["sonde"] != attese:
        fallite.append(
            f"sonde del modulo nativo {trovato['sonde']}, attese {attese} "
            f"({'wheel di rilascio' if rilascio else 'wheel delle prove'})"
        )


def raccolte(
    copia: Path, cartella: Path, ambiente: dict[str, str], fallite: list[str], *selezione: str
) -> int | None:
    """Le prove raccolte senza eseguirle, con la selezione data; `None`, con
    una verifica fallita, se la raccolta non esce con 0. Gli id stampati
    prima di un errore di raccolta non contano: il conteggio vale solo per
    una raccolta riuscita. Il messaggio dice solo il codice d'uscita."""
    esito = subprocess.run(
        [sys.executable, "-m", "pytest", str(copia), "--collect-only", "-q",
         "-p", "no:cacheprovider", *selezione],
        cwd=cartella,
        env=ambiente,
        capture_output=True,
        text=True,
        check=False,
    )
    if esito.returncode != 0:
        fallite.append(f"raccolta delle prove fallita (codice {esito.returncode})")
        return None
    return sum(1 for riga in esito.stdout.splitlines() if "::" in riga)


def prove(cartella: Path, rilascio: bool, fallite: list[str]) -> None:
    copia = cartella / "tests"
    if copia.exists():
        shutil.rmtree(copia)
    shutil.copytree(PROVE, copia, ignore=shutil.ignore_patterns("__pycache__"))
    esito_xml = cartella / "esito.xml"
    ambiente = {**os.environ, "PLENORA_DATA_CONTRATTI": str(CONTRATTI)}
    ambiente.pop("PYTHONPATH", None)
    esito = subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            str(copia),
            "-q",
            "-rs",
            "-p",
            "no:cacheprovider",
            "--basetemp",
            str(cartella / "temporanei"),
            "--junitxml",
            str(esito_xml),
            *(["-m", "not sonde"] if rilascio else []),
        ],
        cwd=cartella,
        env=ambiente,
        check=False,
    )
    if esito.returncode != 0:
        fallite.append(f"pytest uscito con {esito.returncode}")
    if not esito_xml.is_file():
        fallite.append("pytest non ha scritto l'esito")
        return
    suite = list(ET.parse(esito_xml).getroot().iter("testsuite"))
    raccolti = sum(int(s.get("tests", 0)) for s in suite)
    saltati = sum(int(s.get("skipped", 0)) for s in suite)
    falliti = sum(int(s.get("failures", 0)) + int(s.get("errors", 0)) for s in suite)
    print(f"verifica-sdk: {raccolti} raccolti, {saltati} saltati, {falliti} falliti")
    if raccolti == 0 or saltati or falliti:
        fallite.append("la suite non e' passata per intero")
    if rilascio:
        # Le sole prove escluse sono quelle delle sonde, e ce ne sono.
        marcate = raccolte(copia, cartella, ambiente, fallite, "-m", "sonde")
        tutte = raccolte(copia, cartella, ambiente, fallite)
        if marcate is None or tutte is None:
            return
        print(f"verifica-sdk: {marcate} prove delle sonde deselezionate (rilascio)")
        if marcate == 0 or raccolti + marcate != tutte:
            fallite.append("le prove deselezionate non sono esattamente quelle delle sonde")


def main() -> int:
    argomenti = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    argomenti.add_argument("--wheel", type=Path, required=True)
    argomenti.add_argument(
        "--rilascio",
        action="store_true",
        help="wheel di rilascio: senza sonde, suite senza le prove marcate `sonde`",
    )
    argomenti.add_argument(
        "--cartella",
        type=Path,
        help="cartella di lavoro fuori dal checkout (default: una temporanea)",
    )
    letti = argomenti.parse_args()
    fallite: list[str] = []
    versione, impronta, tracce = leggi_wheel(letti.wheel.resolve(), fallite)
    if letti.rilascio and tracce:
        fallite.append(f"il wheel di rilascio porta le sonde nel modulo nativo: {tracce}")
    if not letti.rilascio and not tracce:
        fallite.append("il wheel delle prove non ha le sonde (feature sonde-di-prova)")
    atteso = versione_del_workspace()
    if versione != atteso:
        fallite.append(f"wheel {versione}, workspace {atteso}")
    with tempfile.TemporaryDirectory(prefix="plenora-sdk-") as temporanea:
        cartella = (letti.cartella or Path(temporanea)).resolve()
        cartella.mkdir(parents=True, exist_ok=True)
        if cartella.is_relative_to(RADICE):
            fallite.append("la cartella di lavoro e' dentro il checkout")
        elif not fallite:
            sonda(cartella, versione, impronta, letti.rilascio, fallite)
            prove(cartella, letti.rilascio, fallite)
    for cosa in fallite:
        print(f"FALLITA: {cosa}")
    print(f"{'ok' if not fallite else 'errori'}: {len(fallite)} verifiche fallite")
    return 1 if fallite else 0


if __name__ == "__main__":
    sys.exit(main())
