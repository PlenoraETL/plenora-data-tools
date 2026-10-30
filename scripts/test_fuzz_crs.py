"""Guardie offline: corpus riproducibile, elenchi allineati e confronti esatti."""

import hashlib
from pathlib import Path
import tomllib
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import struct

import corpus_definizioni_crs as corpus
from verifica_interop_crs import json_esatto, json_uguali, verifica, PREFISSO

RADICE = Path(__file__).resolve().parents[1]


class FuzzCrs(unittest.TestCase):
    def test_corpus_riproducibile_e_limiti(self):
        a = list(corpus.semi())
        b = list(corpus.semi())
        self.assertEqual(a, b)
        self.assertGreater(len(set(a)), 5000)
        self.assertTrue(any(len(x) == 65537 for x in a))
        self.assertTrue(any(b'SENTINELLA-7f3a9c' in x for x in a))
        self.assertEqual(len({x for x in a if x.startswith(b'EPSG:') or x == b'OGC:CRS84'}), 169)
        self.assertEqual([hashlib.sha1(x).hexdigest() for x in a],
                         [hashlib.sha1(x).hexdigest() for x in b])

    def test_elenchi_target_allineati(self):
        manifesto = tomllib.loads((RADICE / "fuzz/Cargo.toml").read_text(encoding="utf-8"))
        workflow = (RADICE / ".github/workflows/fuzz.yml").read_text(encoding="utf-8")
        readme = (RADICE / "README.md").read_text(encoding="utf-8")
        for voce in manifesto["bin"]:
            self.assertIn("          - " + voce["name"] + "\n", workflow)
            self.assertIn('`' + voce["name"] + '`', readme)
            self.assertTrue((RADICE / "fuzz" / voce["path"]).is_file())

    def test_interop_decimali_esatti_e_duplicati(self):
        self.assertEqual(json_esatto('{"x":500000}'), json_esatto('{"x":5e5}'))
        self.assertNotEqual(json_esatto('{"x":500000}'), json_esatto('{"x":500000.00000000001}'))
        for numero, booleano in (("0", "false"), ("1", "true")):
            self.assertFalse(json_uguali(json_esatto('{"x":[' + numero + ']}'),
                                        json_esatto('{"x":[' + booleano + ']}')))
        self.assertTrue(json_uguali(json_esatto('{"x":5e5}'), json_esatto('{"x":500000}')))
        for testo in ('{"x":1,"x":2}', '{"x":{"y":1,"y":2}}', '{"x":NaN}'):
            with self.assertRaises(ValueError):
                json_esatto(testo)

    def test_interop_rifiuta_crs_assi_e_coordinate_mutati(self):
        atteso = '{"coordinate_system":{},"id":{"code":4326}}'
        punti = [(12.5, 45.25)]
        metadata = {PREFISSO + b"crs_definition": atteso.encode(),
                    PREFISSO + b"crs_definition_format": b"projjson",
                    PREFISSO + b"axis_order": b"lon_lat",
                    PREFISSO + b"crs_resolution": b"resolved"}

        class Tabella:
            num_rows = 1
            column_names = ["geometry"]

            def __init__(self, md, xy):
                self.schema = SimpleNamespace(field=lambda _: SimpleNamespace(metadata=md))
                self.xy = xy

            def __getitem__(self, _):
                return SimpleNamespace(to_pylist=lambda: [struct.pack("<BIdd", 1, 1, *self.xy)])

        def controlla(md, xy):
            with patch("verifica_interop_crs.leggi", return_value=Tabella(md, xy)):
                return verifica(Path("file.arrow"), punti, atteso, "projjson", "lon_lat", json_esatto(atteso))

        self.assertEqual(controlla(metadata, punti[0]), atteso.encode())
        for chiave, valore in ((b"crs_definition", atteso.replace("4326", "6707").encode()),
                               (b"axis_order", b"lat_lon"),
                               (b"crs_resolution", b"declared_unresolved"),
                               (b"crs_definition_format", b"wkt2")):
            with self.assertRaises(ValueError):
                controlla({**metadata, PREFISSO + chiave: valore}, punti[0])
        with self.assertRaises(ValueError):
            controlla(metadata, (45.25, 12.5))


if __name__ == "__main__":
    unittest.main()
