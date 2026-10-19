"""Duplicati nei documenti di controllo, anche annidati e con escape."""

import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

try:
    from . import strict_json, avvisi_crate, check_coverage, modello_costi
except ImportError:
    import strict_json
    import avvisi_crate
    import check_coverage
    import modello_costi


DUPLICATI = (
    '{"SENTINELLA": 1, "SENTINELLA": 2}',
    '{"outer": [{"SENTINELLA": 1, "SENTINELLA": 2}]}',
    '{"a": 1, "\\u0061": 2}',
)


class StrictJsonTests(unittest.TestCase):
    def test_loads_e_load_rifiutano_duplicati_senza_dati(self):
        for text in DUPLICATI:
            for read in (strict_json.loads, lambda value: strict_json.load(io.StringIO(value))):
                with self.subTest(text=text, read=read):
                    with self.assertRaises(json.JSONDecodeError) as error:
                        read(text)
                    self.assertEqual(str(error.exception), "chiave JSON duplicata: line 1 column 1 (char 0)")
                    self.assertEqual(error.exception.doc, "")

    def test_oggetti_distinti_e_tipi_json(self):
        text = '{"a": [{"k": 1}, {"k": 2}], "b": true, "c": null}'
        self.assertEqual(strict_json.loads(text), json.loads(text))
        self.assertEqual(strict_json.load(io.BytesIO(text.encode())), json.loads(text))
        for text in ('{', '{} {}'):
            with self.assertRaises(json.JSONDecodeError):
                strict_json.loads(text)

    def test_lettori_di_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "controllo.json"
            for text in DUPLICATI:
                path.write_text(text, encoding="utf-8")
                with self.subTest(text=text):
                    with patch.object(avvisi_crate, "REGISTRO", path):
                        with self.assertRaises(json.JSONDecodeError):
                            avvisi_crate.registro()
                    with self.assertRaises(check_coverage.CoverageError):
                        check_coverage.load_json(path, "budget")
                    with self.assertRaises(json.JSONDecodeError):
                        modello_costi.estrai(path)
                    with patch.object(modello_costi, "MISURE", path):
                        with self.assertRaises(json.JSONDecodeError):
                            modello_costi.carica()

    def test_helper_delle_fixture_sdk(self):
        path = Path(__file__).resolve().parents[1] / "crates/plenora-data-py/python/tests/json_fixture.py"
        spec = importlib.util.spec_from_file_location("json_fixture", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for text in DUPLICATI:
            with self.assertRaises(json.JSONDecodeError):
                module.loads(text)
        self.assertEqual(module.loads('{"a": [{"k": 1}, {"k": 2}]}'), {"a": [{"k": 1}, {"k": 2}]})

    def test_hook_inline_rilascio(self):
        path = Path(__file__).resolve().parents[1] / ".github/workflows/rilascio.yml"
        source = path.read_text(encoding="utf-8")
        hook = source.split("          def unique_pairs(pairs):\n", 1)[1].split('          with open(', 1)[0]
        source = "def unique_pairs(pairs):\n" + "\n".join(line[10:] for line in hook.splitlines())
        namespace = {}
        exec(source, namespace)
        for text in DUPLICATI:
            with self.assertRaisesRegex(ValueError, "^chiave JSON duplicata$"):
                json.loads(text, object_pairs_hook=namespace["unique_pairs"])


if __name__ == "__main__":
    unittest.main()
