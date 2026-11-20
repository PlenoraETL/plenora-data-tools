"""Il minimo Arrow protegge anche i consumatori autonomi del fork."""
from pathlib import Path
import importlib.util
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("aggiorna_dependency_policy", ROOT / ".ci/aggiorna_dependency_policy.py")
POLICY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(POLICY)


class RiconciliazionePolicyTests(unittest.TestCase):
    DATA = b'''baseline = "conservata"
[[exceptions]]
repo = "plenora-data-tools2"
key = "orfana"
[[exceptions]]
repo = "altro"
key = "altra"
[[alignments]]
key = "conservato"
'''

    def report(self, rule="stale_exception", repo=POLICY.NAME, key="orfana"):
        return {"errors": [dict(rule=rule, repo=repo, key=key)]}

    def test_rimozione_conservativa(self):
        parsed = tomllib.loads(self.DATA.decode())
        result = POLICY.rimuovi_orfane(self.DATA, parsed, self.report())
        expected = dict(parsed, exceptions=[parsed["exceptions"][1]])
        self.assertEqual(tomllib.loads(result.decode()), expected)
        self.assertEqual(POLICY.rimuovi_orfane(result, expected, {"errors": []}), result)

    def test_rifiuta_devizioni_attive_altri_repo_e_chiavi_assenti(self):
        for report in (self.report(rule="checksum"), self.report(rule="fork"),
                       self.report(rule="unsound"), self.report(repo="altro"),
                       self.report(key="assente")):
            with self.subTest(report=report), self.assertRaises(ValueError):
                POLICY.rimuovi_orfane(self.DATA, tomllib.loads(self.DATA.decode()), report)

    def test_qualifica_idempotente_e_senza_nuove_eccezioni(self):
        data = (ROOT / ".ci/plenora-dependencies/plenora-dependencies.toml").read_bytes()
        result = POLICY.qualifica(data, ROOT)
        self.assertEqual(POLICY.qualifica(result, ROOT), result)
        before, after = (tomllib.loads(d.decode()) for d in (data, result))
        for key in ("exceptions", "alignments", "interfaces", "baseline", "fork_names"):
            self.assertEqual(before[key], after[key])
        fork = next(f for f in after["forks"] if f["crate"] == "plenora-rstar")
        self.assertEqual(fork["removal"], "quando rstar pubblica una versione con heapless corretta")
        self.assertEqual(fork["source"], "path:" + POLICY.FORK_PATH)

    def test_aggiunge_qualifiche_mancanti(self):
        data = (ROOT / ".ci/plenora-dependencies/plenora-dependencies.toml").read_bytes()
        expected = POLICY.qualifica(data, ROOT)
        missing = b"".join(p for p in POLICY.blocchi(expected)
                           if not (p.startswith(b"[[references]]") and b'line = "0.9"' in p and b'crate = "heapless"' in p)
                           and not (p.startswith(b"[[forks]]") and b'crate = "plenora-rstar"' in p))
        self.assertEqual(POLICY.qualifica(missing, ROOT), expected)

    def test_rifiuta_heapless_non_qualificata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = (ROOT / "Cargo.lock").read_text("utf8")
            (root / "Cargo.lock").write_text(data.replace('name = "heapless"\nversion = "0.9.4"',
                                                         'name = "heapless"\nversion = "0.8.0"'), "utf8")
            with self.assertRaisesRegex(ValueError, "lock diverso"):
                POLICY.qualifica(b"", root)

    def test_snapshot_corrotto_non_scrive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home = root / ".ci/plenora-dependencies"
            home.mkdir(parents=True)
            original = ROOT / ".ci/plenora-dependencies"
            for path in original.iterdir():
                if path.is_file():
                    (home / path.name).write_bytes(path.read_bytes())
            checker = home / "controlla_dipendenze.py"
            checker.write_bytes(checker.read_bytes() + b"\n# alterato\n")
            before = {p.name: p.read_bytes() for p in home.iterdir() if p.is_file()}
            with self.assertRaisesRegex(ValueError, "snapshot iniziale"):
                POLICY.aggiorna(root)
            self.assertEqual(before, {p.name: p.read_bytes() for p in home.iterdir() if p.is_file()})


class AllineamentoDipendenzeTests(unittest.TestCase):
    def test_minimo_arrow_del_fork(self):
        manifest = tomllib.loads((ROOT / "vendor/parquet-60.0.0-eof/Cargo.toml").read_text("utf-8"))
        found = []
        for section in ("dependencies", "dev-dependencies"):
            for name, requirement in manifest[section].items():
                if name == "arrow" or name.startswith("arrow-"):
                    found.append(name)
                    self.assertEqual(requirement["version"], ">=60.1.0, <61", name)
        self.assertEqual(len(found), 9)

    def test_versioni_comuni_nei_tre_lock(self):
        for name in ("Cargo.lock", "fuzz/Cargo.lock", "vendor/parquet-60.0.0-eof/Cargo.lock"):
            packages = tomllib.loads((ROOT / name).read_text("utf-8"))["package"]
            for package in packages:
                dependency, version = package["name"], package["version"]
                if dependency == "arrow" or dependency.startswith("arrow-"):
                    self.assertEqual(version, "60.1.0", (name, dependency))
                if dependency in ("serde_json", "geo-types"):
                    self.assertEqual(version, {"serde_json": "1.0.154", "geo-types": "0.7.20"}[dependency])
                if dependency in ("thiserror", "thiserror-impl") and version.startswith("2."):
                    self.assertEqual(version, "2.0.21")


if __name__ == "__main__":
    unittest.main()
