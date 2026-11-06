"""Sonda compilata dei divieti Result, separata dalle euristiche testuali."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ClippyEsitoTests(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("PLENORA_PROVA_CLIPPY") == "1", "sonda compilata eseguita nel job Rust con PLENORA_PROVA_CLIPPY=1")
    def test_metodi_risolti_e_produzione_esente(self):
        # Questo test richiede la toolchain fissata, quindi gira nel job Rust.
        with tempfile.TemporaryDirectory(prefix='esito-clippy-') as directory:
            root = Path(directory)
            (root / 'src').mkdir()
            (root / 'tests').mkdir()
            (root / 'Cargo.toml').write_text(
                '[package]\nname="sonda-esito"\nversion="0.0.0"\nedition="2021"\n[workspace]\n',
                encoding='utf-8')
            shutil.copyfile(ROOT / 'clippy.toml', root / 'clippy.toml')
            shutil.copyfile(ROOT / 'rust-toolchain.toml', root / 'rust-toolchain.toml')
            production = '''#![allow(clippy::disallowed_methods, reason="produzione della sonda")]
pub fn production(r: Result<(), ()>) -> bool { r.is_ok() }
#[deny(clippy::disallowed_methods)]
#[cfg(test)] mod tests {
    #[test] fn precise() { assert_eq!(std::hint::black_box(Ok::<_, ()>(3)).expect("successo"), 3); }
}
'''
            forbidden = '''use std::result::Result as R;
#[test] fn forbidden() {
    let r: R<u8, u8> = std::hint::black_box(Err(1));
    let _ = r .
        is_err ( );
    let _ = R::is_ok(&r);
    let _ = { r.unwrap_err() };
    let _ = R::err(r);
}
'''
            external = '\n#[deny(clippy::disallowed_methods)]\n#[cfg(test)]\n#[path="esterno.rs"] mod esterno;\n'
            (root / 'src/lib.rs').write_text(production + external +
                '\n#[deny(clippy::disallowed_methods)]\n#[cfg(test)] mod vietati {\n' + forbidden + '\n}', encoding='utf-8')
            (root / 'src/esterno.rs').write_text(forbidden, encoding='utf-8')
            (root / 'tests/prova.rs').write_text(forbidden, encoding='utf-8')
            command = ['cargo', 'clippy', '--offline', '--all-targets', '--keep-going', '--message-format=json', '--', '-D', 'warnings']
            result = subprocess.run(command, cwd=root, capture_output=True, text=True, encoding='utf-8', check=False)
            self.assertNotEqual(result.returncode, 0)
            diagnostics = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
            errors = [d['message'] for d in diagnostics if d.get('reason') == 'compiler-message'
                      and d['message']['level'] == 'error']
            self.assertEqual(len(errors), 12, result.stderr + result.stdout)
            self.assertTrue(all(e['code']['code'] == 'clippy::disallowed_methods' for e in errors))
            for method in ['is_err', 'is_ok', 'unwrap_err', 'err']:
                self.assertTrue(any(f'::{method}`' in e['message'] for e in errors))
            precise = '#[test] fn precise() { assert_eq!(std::hint::black_box(Ok::<_, ()>(3)).expect("successo"), 3); }'
            # Limite dichiarato: il nome is_err di un trait proprio non e'
            # Result::is_err. La sonda compila anche con -D warnings.
            precise += """
#[test] fn trait_proprio_fuori_ambito() {
    enum Stato { Fallito, Riuscito }
    trait Esito { fn is_err(&self) -> bool; }
    impl Esito for Stato {
        fn is_err(&self) -> bool { matches!(self, Self::Fallito) }
    }
    assert!(Stato::Fallito.is_err());
    assert!(!Stato::Riuscito.is_err());
}
"""
            (root / 'src/lib.rs').write_text(production + external, encoding='utf-8')
            (root / 'src/esterno.rs').write_text(precise, encoding='utf-8')
            (root / 'tests/prova.rs').write_text(precise, encoding='utf-8')
            result = subprocess.run(command, cwd=root, capture_output=True, text=True, encoding='utf-8', check=False)
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
