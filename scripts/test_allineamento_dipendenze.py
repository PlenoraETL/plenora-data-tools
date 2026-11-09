"""Il minimo Arrow protegge anche i consumatori autonomi del fork."""
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


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
