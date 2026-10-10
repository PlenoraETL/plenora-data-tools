"""Prove di `scripts/verifica_sdk_python.py` senza wheel né pytest.

`raccolte` lancia `python -m pytest --collect-only`: qui un `pytest` finto,
messo per primo nel `PYTHONPATH` del sottoprocesso, stampa degli id di
prova e poi esce con il codice scelto. Così si prova la regola del gate
senza dipendere da pytest (la CI delle guardie usa la sola libreria
standard).

Uso: python -m unittest discover -s scripts -p "test_*.py"
"""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

import verifica_sdk_python

FINTO = """
import sys
print("tests/test_a.py::test_uno")
print("tests/test_a.py::test_due")
print("tests/test_b.py::test_tre")
sys.exit({codice})
"""


class RaccolteTests(unittest.TestCase):
    def raccogli(self, codice: int) -> tuple[int | None, list[str]]:
        with tempfile.TemporaryDirectory(prefix="plenora-raccolte-") as cartella:
            radice = Path(cartella)
            finto = radice / "finto" / "pytest"
            finto.mkdir(parents=True)
            (finto / "__init__.py").write_text("", encoding="utf-8")
            (finto / "__main__.py").write_text(FINTO.format(codice=codice), encoding="utf-8")
            ambiente = {**os.environ, "PYTHONPATH": str(radice / "finto")}
            fallite: list[str] = []
            conteggio = verifica_sdk_python.raccolte(
                radice / "tests", radice, ambiente, fallite, "-m", "sonde"
            )
            return conteggio, fallite

    def test_una_raccolta_riuscita_conta_gli_id(self) -> None:
        conteggio, fallite = self.raccogli(0)
        self.assertEqual(conteggio, 3)
        self.assertEqual(fallite, [])

    def test_una_raccolta_fallita_dopo_gli_id_non_conta(self) -> None:
        # Gli id stampati prima dell'errore (come fa pytest con un errore
        # di raccolta in un altro file) non diventano un conteggio.
        for codice in (1, 2, 5):
            conteggio, fallite = self.raccogli(codice)
            self.assertIsNone(conteggio, codice)
            self.assertEqual(fallite, [f"raccolta delle prove fallita (codice {codice})"])


if __name__ == "__main__":
    unittest.main()
