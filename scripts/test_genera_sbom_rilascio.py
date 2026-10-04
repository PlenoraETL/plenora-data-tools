"""Prove di `scripts/genera_sbom_rilascio.py` su un wheel sintetico.

Uso: python -m unittest discover -s scripts -p "test_*.py"
"""

from __future__ import annotations

import tempfile
import tomllib
import unittest
import zipfile
from pathlib import Path

import genera_sbom_rilascio as sbom

VERSIONE = tomllib.loads((sbom.ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
NATIVO = {"bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1, "components": [], "dependencies": []}


def wheel(cartella: Path, nome: str = "plenora-data", versione: str = VERSIONE, requisiti=None) -> Path:
    percorso = cartella / f"plenora_data-{versione}-cp310-abi3-linux_x86_64.whl"
    righe = [f"Name: {nome}", f"Version: {versione}"]
    righe += [f"Requires-Dist: {r}" for r in (sorted(sbom.DIPENDENZE_RUNTIME) if requisiti is None else requisiti)]
    with zipfile.ZipFile(percorso, "w") as archivio:
        archivio.writestr(f"plenora_data-{versione}.dist-info/METADATA", "\n".join(righe) + "\n")
    return percorso


class SbomRilascioTests(unittest.TestCase):
    def test_genera_e_verifica(self) -> None:
        with tempfile.TemporaryDirectory() as cartella:
            dist = Path(cartella)
            wheel(dist)
            bom = sbom.render(sbom.ROOT, dist, NATIVO)
            sbom.validate(sbom.ROOT, dist, bom)
            self.assertEqual(bom["metadata"]["component"]["name"], "plenora-data-tools")
            nomi = {componente["name"] for componente in bom["components"]}
            self.assertIn("parquet", nomi)  # il vendor con patch
            self.assertIn("pyarrow", nomi)  # pin di qualifica

    def test_identita_del_wheel_diversa_si_rifiuta(self) -> None:
        with tempfile.TemporaryDirectory() as cartella:
            wheel(Path(cartella), nome="plenora-database")
            with self.assertRaises(ValueError):
                sbom.render(sbom.ROOT, Path(cartella), NATIVO)

    def test_dipendenza_runtime_non_dichiarata_si_rifiuta(self) -> None:
        with tempfile.TemporaryDirectory() as cartella:
            wheel(Path(cartella), requisiti=["pyarrow>=25,<26", "numpy"])
            with self.assertRaises(ValueError):
                sbom.render(sbom.ROOT, Path(cartella), NATIVO)

    def test_un_lock_cambiato_rende_lo_sbom_stantio(self) -> None:
        with tempfile.TemporaryDirectory() as cartella:
            dist = Path(cartella)
            wheel(dist)
            bom = sbom.render(sbom.ROOT, dist, NATIVO)
            bom["components"] = bom["components"][1:]
            with self.assertRaises(ValueError):
                sbom.validate(sbom.ROOT, dist, bom)


if __name__ == "__main__":
    unittest.main()
