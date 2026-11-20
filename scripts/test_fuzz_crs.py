"""Guardie offline: corpus riproducibile, elenchi allineati e confronti esatti."""

import copy
import json
import tempfile
import hashlib
import interop_pin
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
        self.assertEqual({x.decode() for x in a if x.startswith(b'EPSG:') or x == b'OGC:CRS84'},
                         set(corpus.crs_integrati()))
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


class InteropRelease(unittest.TestCase):
    def test_release_corrente_tag_leggero_annotato_e_major_successiva(self):
        for tag in ("v9.0.0", "v10.0.0", "10.0.0"):
            release = {"tag_name": tag, "draft": False, "prerelease": False, "id": 123}
            ref = "refs/tags/" + tag
            raw = "a" * 40 + "\t" + ref + "\n"
            pin = interop_pin.risolvi(release, raw)
            self.assertEqual(pin["revision"], "a" * 40)
            self.assertEqual(pin["tag"], tag)
            annotated = raw + "b" * 40 + "\t" + ref + "^{}\n"
            self.assertEqual(interop_pin.risolvi(release, annotated)["revision"], "b" * 40)
            self.assertEqual(interop_pin.risolvi(release, annotated),
                             interop_pin.risolvi(release, annotated))

    def test_release_rifiuta_ambiguita_e_nessun_fallback(self):
        release = {"tag_name": "v9.0.0", "draft": False, "prerelease": False, "id": 123}
        raw = "a" * 40 + "\trefs/tags/v9.0.0\n"
        for fields in ({"draft": True}, {"prerelease": True}, {"tag_name": "main"},
                       {"tag_name": "v9.0.0\nrevision=bad"}, {"tag_name": "v9.0.0-rc1"}):
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                interop_pin.risolvi({**release, **fields}, raw)
        for refs in ("", raw + raw, raw.replace("tags", "heads"), raw.replace("a" * 40, "abc")):
            with self.subTest(refs=refs), self.assertRaises(ValueError):
                interop_pin.risolvi(release, refs)
        with patch("interop_pin.urlopen", side_effect=OSError), self.assertRaises(OSError):
            interop_pin.corrente()

    def test_contesti_tutti_workflow_e_mutazioni(self):
        import yaml
        self.assertEqual(interop_pin.verifica_workflow(), 7)
        workflow = yaml.load((RADICE / ".github/workflows/interop.yml").read_text("utf-8"), Loader=yaml.BaseLoader)
        self.assertEqual(set(workflow["on"]), {"workflow_dispatch"})
        for key, value in (("env", {"RUSTUP_HOME": "${{ runner.temp }}/rustup"}),
                           ("if", "matrix.os == 'ubuntu-latest'"),
                           ("runs-on", "${{ hashFiles('x') }}")):
            changed = copy.deepcopy(workflow)
            changed["jobs"]["catene"][key] = value
            self.assertTrue(interop_pin.contesti_workflow(changed))
        for expression in ("${{ github.sha }}${{ runner.temp", "${{ secrets.TOKEN }}", "secrets['TOKEN'] != ''"):
            changed = copy.deepcopy(workflow)
            changed["jobs"]["catene"]["steps"][0]["if"] = expression
            self.assertTrue(interop_pin.contesti_workflow(changed))
        self.assertFalse(interop_pin.contesti_workflow({"jobs": {"j": {"steps": [
            {"run": "echo ${{ runner['temp'] }} ${{ format('runner.x {0}', github.sha) }}"}
        ]}}}))

    def test_multigeometria_oracolo_indipendente(self):
        import verifica_multigeometria_io700 as multi
        metadata = {b"plenora.geometry.crs_definition": b'{"x":500000}',
                    b"plenora.field_id": b"1", b"plenora.geometry.axis_order": b"lon_lat",
                    b"plenora.geometry.dimensions": b"xy"}
        class Table:
            num_rows = 3
            column_names = ["traccia", "posizione"]
            schema = SimpleNamespace(field=lambda _: SimpleNamespace(metadata=metadata))
            def __getitem__(self, _):
                return SimpleNamespace(to_pylist=lambda: [b"wkb", None, b"wkb2"])
        geo = json_esatto('{"primary_column":"posizione","columns":' +
                         '{"traccia":{"crs":{"x":500000},"encoding":"WKB","geometry_types":["Point"]},' +
                         '"posizione":{"crs":{"x":500000},"encoding":"WKB","geometry_types":["Point"]}}}')
        multi.verifica_tabella(Table(), Table(), geo)
        for mutation in ("500000.00000000001", "true"):
            changed = copy.deepcopy(geo)
            changed["columns"]["traccia"]["crs"] = json_esatto('{"x":' + mutation + '}')
            with self.assertRaises(ValueError):
                multi.verifica_tabella(Table(), Table(), changed)
        changed = copy.deepcopy(geo)
        changed["primary_column"] = "traccia"
        with self.assertRaises(ValueError):
            multi.verifica_tabella(Table(), Table(), changed)
        changed = Table()
        changed.num_rows = 2
        with self.assertRaises(ValueError):
            multi.verifica_tabella(Table(), changed, geo)

    def test_multigeometria_errore_io_conserva_pin_e_bloccati(self):
        import verifica_multigeometria_io700 as multi
        import sys
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            source = work / "source.arrow"
            source.write_bytes(b"input")
            pin = {"version": "9.0.0", "tag": "v9.0.0", "revision": "a" * 40}
            (work / "pin.json").write_text(json.dumps(pin), encoding="utf-8")
            args = SimpleNamespace(work=work, io=Path("io"), data=Path("data"), pin=work / "pin.json")
            reader = unittest.mock.MagicMock()
            ipc = SimpleNamespace(open_file=lambda _: reader)
            pq = SimpleNamespace(read_table=unittest.mock.Mock(side_effect=FileNotFoundError))
            pa = SimpleNamespace(__version__="test", ipc=ipc, parquet=pq)
            with patch.dict(sys.modules, {"pyarrow": pa, "pyarrow.ipc": ipc, "pyarrow.parquet": pq}), \
                 patch.object(multi, "prepara", return_value=source), \
                 patch.object(multi.subprocess, "run", side_effect=OSError):
                self.assertEqual(multi.esegui_catena(args), 1)
            report = json.loads((work / "report.json").read_text("utf-8"))
            self.assertEqual(report["io_release"], pin)
            self.assertEqual([r["stato"] for r in report["results"][:4]],
                             ["fallito", "bloccato", "bloccato", "bloccato"])

    def test_multigeometria_preparazione_fallita_scrive_report(self):
        import verifica_multigeometria_io700 as multi
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory) / "work"
            argv = ["multi", "--io", "io", "--data", "data", "--work", str(work), "--pin", "pin.json"]
            with patch("sys.argv", argv), patch.object(multi, "esegui_catena", side_effect=ValueError("segreto")):
                self.assertEqual(multi.main(), 1)
            report = (work / "report.json").read_text("utf-8")
            self.assertEqual(json.loads(report)["stato"], "fallito")
            self.assertNotIn("segreto", report)


if __name__ == "__main__":
    unittest.main()
