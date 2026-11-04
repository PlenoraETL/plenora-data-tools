"""Classificazione chiave/testo dei chiamanti degli analyzer tabellari.

La tabella versionata rende obbligatoria la classificazione di ogni nuovo
chiamante; le prove del catalogo verificano la semantica sulle celle WKB.
"""

from __future__ import annotations

import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
TABLE = Path(__file__).with_name("geometry_text_classification.json")
CALL = re.compile(r"\b(require_(?:key_field|keys|key|text_value_field|text_values|text_value))\s*\(")
FUNCTION = re.compile(r"\bfn\s+(\w+)\s*(?:<[^\n]*>)?\s*\(")


def callers() -> dict[str, list[str]]:
    result: dict[str, list[str]] = {}
    folder = ROOT / "crates/plenora-kernels-table/src/analyze"
    for path in sorted(folder.glob("*.rs")):
        if path.name == "helpers.rs":
            continue
        function = ""
        text = path.read_text(encoding="utf-8")
        if "require_scalar_string" in text:
            raise AssertionError(f"helper non classificato in {path.name}")
        for line in text.splitlines():
            line = line.split("//", 1)[0]
            match = FUNCTION.search(line)
            if match:
                function = match[1]
            for call in CALL.finditer(line):
                result.setdefault(f"{path.name}:{function}", []).append(call[1])
    return result


class GeometryTextClassificationTests(unittest.TestCase):
    def test_every_caller_has_a_reviewed_role(self) -> None:
        expected = json.loads(TABLE.read_text(encoding="utf-8"))
        self.assertEqual(expected, callers())


if __name__ == "__main__":
    unittest.main()
