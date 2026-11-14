"""Mutazioni dei registri e raccolta completa dopo errori intermedi."""

import contextlib
import io
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import campagna
import check_prove_fonti as guard
import esegui_guardie
import fonti_prove
import verifica_adozione
import verifica_interop_crs
import verifica_sdk_python


class FontiTests(unittest.TestCase):
    def test_sdk_importabile_senza_tomllib(self):
        # Sottoprocesso pulito: non puo' riusare un import gia' in cache.
        code = """
import builtins, sys
sys.path.insert(0, sys.argv[1])
original = builtins.__import__
def guarded(name, *args, **kwargs):
    if name == 'tomllib':
        raise ModuleNotFoundError('tomllib non disponibile')
    return original(name, *args, **kwargs)
builtins.__import__ = guarded
import verifica_sdk_python as sdk
assert sdk.SONDE
assert repr(sdk.SONDE) in sdk.SONDA
"""
        result = subprocess.run([sys.executable, "-I", "-c", code, str(fonti_prove.ROOT / "scripts")],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_censimento_identico_con_separatori_e_ordinamenti_diversi(self):
        names = guard.file_tracciati(fonti_prove.ROOT)
        documents = []
        for kind in (PurePosixPath, PureWindowsPath):
            paths = sorted(map(kind, names))
            raw = ("\0".join(map(str, paths)) + "\0").encode("utf-8")
            with patch.object(guard.subprocess, "check_output", return_value=raw):
                documents.append(json.dumps(guard.inventario(), ensure_ascii=False, indent=2))
        self.assertEqual(documents[0], documents[1])

    def test_censimento_ignora_non_tracciati_e_file_ignorati(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            def write(path, content):
                path = root / path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content.encode("utf-8"))
            write("fuzz/Cargo.toml", '[[bin]]\nname = "piano"\npath = "fuzz_targets/piano.rs"\n')
            write("fuzz/fuzz_targets/piano.rs", "")
            write("crates/plenora-data-py/src/sonde.rs", '#[pyo3(name = "_sonda_prova")]\n')
            script = Path("scripts") / "sample.py"
            fixture = Path("crates/example/tests")
            write(script, "def test_sample():\n    pass\n")
            write(fixture / "PROVENANCE.md", "fixture\n")
            write(fixture / "epoch.arrow", "fixture\n")
            write(".gitignore", "ignored.py\n")
            for command in (["git", "init", "-q"], ["git", "add", "."]):
                subprocess.run(command, cwd=root, check=True, capture_output=True)
            before = guard.inventario(root)
            self.assertEqual(before["fixtures"], [(fixture / n).as_posix()
                                                for n in ("PROVENANCE.md", "epoch.arrow")])
            write(Path("scripts") / "ignored.py", "not valid Python!")
            write(Path("scripts") / "untracked.py", "not valid Python!")
            write(fixture / "extra.arrow", "extra")
            write("fuzz/fuzz_targets/untracked.rs", "")
            self.assertEqual(guard.inventario(root), before)
            write(script, "def test_sample():\r\n    pass\r\n")
            self.assertEqual(guard.inventario(root), before)
            (root / script).unlink()
            with self.assertRaises(FileNotFoundError):
                guard.inventario(root)

    def test_git_fallito_o_percorsi_ambigui_non_diventano_inventario(self):
        for names in (["../outside"], ["/absolute"], ["a//b"], ["a/b", "a\\b"]):
            with self.subTest(names=names), self.assertRaises(ValueError):
                guard.normalizza_percorsi(names)
        with patch.object(guard.subprocess, "check_output", side_effect=subprocess.CalledProcessError(1, "git")):
            with self.assertRaises(subprocess.CalledProcessError):
                guard.inventario()

    def test_inventario_riproducibile(self):
        self.assertEqual(guard.inventario(), guard.inventario())
        self.assertEqual(guard.check(), [])

    def test_sonde_del_sottoprocesso_generate(self):
        self.assertEqual(list(verifica_sdk_python.SONDE), fonti_prove.sonde_sdk())
        self.assertIn(repr(verifica_sdk_python.SONDE), verifica_sdk_python.SONDA)
        self.assertNotIn("__SONDE__", verifica_sdk_python.SONDA)

    def test_target_aggiunto_tolto_duplicato(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            shutil.copytree(fonti_prove.ROOT / "fuzz", root / "fuzz",
                            ignore=shutil.ignore_patterns("target", "corpus", "artifacts"))
            (root / ".github/workflows").mkdir(parents=True)
            shutil.copytree(fonti_prove.ROOT / "scripts", root / "scripts",
                            ignore=shutil.ignore_patterns("__pycache__"))
            workflow = root / ".github/workflows/fuzz.yml"
            source = (fonti_prove.ROOT / ".github/workflows/fuzz.yml").read_text("utf-8")
            for replacement in ("", "          - piano\n          - piano\n",
                                "          - piano\n          - fantasma\n"):
                workflow.write_text(source.replace("          - piano\n", replacement), encoding="utf-8")
                with self.subTest(replacement=replacement), self.assertRaises(ValueError):
                    guard.verifica_fuzz(root)
            workflow.write_text(source, encoding="utf-8")
            (root / "fuzz/fuzz_targets/nuovo.rs").write_text("", encoding="utf-8")
            with self.assertRaises(ValueError):
                guard.verifica_fuzz(root)

    def test_blocco_eventi_non_riconosciuto_rifiutato(self):
        with self.assertRaises(ValueError):
            guard.workflow_events("on: [push, pull_request]\n")
        self.assertEqual(guard.workflow_events("on:\n  pull_request:\n  schedule:\n    - cron: 'x'\njobs:\n"),
                         ["pull_request", "schedule"])

    def test_fixture_alterata_e_copia_non_censita(self):
        import hashlib
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            folder = root / "crates/example/tests/fixtures"
            folder.mkdir(parents=True)
            data = b"fixture"
            (folder / "a.json").write_bytes(data)
            (folder / "provenienza.json").write_text(json.dumps({"file": {
                "a.json": hashlib.sha256(data).hexdigest()}}), encoding="utf-8")
            guard.verifica_copie(root)
            (folder / "b.json").write_bytes(data)
            with self.assertRaises(ValueError):
                guard.verifica_copie(root)
            (folder / "b.json").unlink()
            (folder / "a.json").write_bytes(b"alterata")
            with self.assertRaises(ValueError):
                guard.verifica_copie(root)

    def test_prova_nuova_non_collegata_rifiutata(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "scripts").mkdir()
            (root / ".github/workflows").mkdir(parents=True)
            example = Path("scripts") / "verifica_nuova.py"
            (root / example).write_text("", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "verifica_nuova"):
                guard.verifica_collegamenti(root)
            (root / ".github/workflows/finto.yml").write_text(
                f"# python {example.as_posix()}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "verifica_nuova"):
                guard.verifica_collegamenti(root)

    def test_discovery_copre_ogni_metodo(self):
        # Caricamento senza esecuzione: individua anche moduli che importano
        # male, TestCase rinominati e prove rimaste fuori dalla discovery.
        import ast
        for path in sorted((fonti_prove.ROOT / "scripts").glob("test_*.py")):
            tree = ast.parse(path.read_text("utf-8-sig"))
            expected = sorted(f"{path.stem}.{cls.name}.{fn.name}"
                              for cls in tree.body if isinstance(cls, ast.ClassDef)
                              for fn in cls.body if isinstance(fn, ast.FunctionDef) and fn.name.startswith("test_"))
            suite = unittest.defaultTestLoader.loadTestsFromName(path.stem)
            def ids(suite):
                for item in suite:
                    if isinstance(item, unittest.TestSuite):
                        yield from ids(item)
                    else:
                        yield item.id()
            self.assertEqual(sorted(ids(suite)), expected, path.name)


class RaccoltaTests(unittest.TestCase):
    def test_cli_documento_rotto_non_nasconde_gli_altri_passaggi(self):
        from verifica_cli_contratti import Verifica
        verifier = Verifica.__new__(Verifica)
        verifier.fallite = []
        seen = []
        for name in ("version", "capabilities", "catalog"):
            with verifier.passaggio(name):
                seen.append(name)
                if name == "capabilities":
                    raise ValueError("dato riservato")
        self.assertEqual(seen, ["version", "capabilities", "catalog"])
        self.assertEqual(verifier.fallite, ["capabilities: esecuzione o documento non valido"])

    def test_guardie_proseguono_e_escono_rosse(self):
        seen = []
        def run(command, **kwargs):
            seen.append(command[-1])
            if command[-1] == "b":
                raise OSError("dato riservato")
            return SimpleNamespace(returncode=int(command[-1] == "a"))
        with contextlib.redirect_stdout(io.StringIO()) as output:
            code = esegui_guardie.esegui([[n] for n in "abc"], run)
        self.assertEqual(seen, list("abc"))
        self.assertEqual(code, 1)
        self.assertNotIn("dato riservato", output.getvalue())

    def test_tutti_i_casi_dopo_errore_e_diagnostica_senza_dati(self):
        cases = verifica_interop_crs.matrice()
        def execute(case):
            if case == cases[0]:
                raise ValueError("dato riservato")
            return "ok"
        results = campagna.raccogli(cases, execute)
        self.assertEqual([r["caso"] for r in results], cases)
        self.assertEqual(results[-1]["stato"], "superato")
        self.assertTrue(campagna.fallita(results))
        self.assertNotIn("dato riservato", repr(results))
        with self.assertRaises(ValueError):
            campagna.raccogli([], execute)

    def test_prove_contratti_non_si_fermano(self):
        with patch.object(verifica_adozione.subprocess, "run", return_value=SimpleNamespace(returncode=1)) as run:
            with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(ValueError):
                verifica_adozione.prove_contratti(Path("contracts"))
            # I nomi vengono dalla dichiarazione adottata, non da un totale.
            source = (fonti_prove.ROOT / "crates/plenora-cli/adozione.json").read_text("utf-8")
            for call in run.call_args_list:
                self.assertIn(Path(call.args[0][-1]).name, source)
            self.assertGreater(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()
