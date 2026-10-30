"""Guardia offline del modello generato e del corpus PROJJSON versionato."""

import json
import unittest
from decimal import Decimal

import genera_semantica_crs as generatore


class SemanticaCrs(unittest.TestCase):
    def test_corpus_e_modello(self):
        modelli = json.loads(generatore.DESTINAZIONE.read_text(encoding="utf-8"))
        corpus = generatore.leggi((generatore.RADICE / "crates/plenora-io/data/projjson_integrati.json").read_text(encoding="utf-8"))
        per_id = {f"{m['id']['authority']}:{m['id']['code']}": m for m in modelli}
        self.assertEqual(len(per_id), len(modelli))
        self.assertEqual(set(per_id), set(corpus))
        for chiave, m in per_id.items():
            with self.subTest(crs=chiave):
                d = corpus[chiave]
                self.assertEqual(m["tipo"], d["type"])
                self.assertEqual(m["sistema"], generatore.sistema(d["coordinate_system"]))
                base = d.get("base_crs", d)
                datum = base.get("datum_ensemble", base.get("datum"))
                self.assertEqual(m["base"]["id"], base["id"])
                self.assertEqual(m["base"]["sistema"], generatore.sistema(base["coordinate_system"]))
                atteso = m["base"]["datum"]
                self.assertEqual(atteso["identita"]["nome"], datum["name"])
                self.assertEqual(atteso["membri"], [generatore.identita(x) for x in datum.get("members", [])])
                self.assertEqual(atteso["accuratezza"], datum.get("accuracy"))
                ellissoide = atteso["ellissoide"]
                self.assertEqual(Decimal(ellissoide["semiasse"]), Decimal(datum["ellipsoid"]["semi_major_axis"]))
                self.assertEqual(Decimal(ellissoide["seconda_misura"]), Decimal(datum["ellipsoid"][ellissoide["forma"]]))
                if "conversion" in d:
                    c = m["conversione"]
                    self.assertEqual(c["metodo"], generatore.identita(d["conversion"]["method"]))
                    self.assertEqual(c["parametri"], [
                        {"identita": generatore.identita(p), "valore": p["value"], "unita": generatore.unita(p["unit"])}
                        for p in d["conversion"]["parameters"]])
                else:
                    self.assertIsNone(m["conversione"])

    def test_unita_sconosciuta_rifiutata(self):
        with self.assertRaises(KeyError):
            generatore.unita("unknown")


if __name__ == "__main__":
    unittest.main()
