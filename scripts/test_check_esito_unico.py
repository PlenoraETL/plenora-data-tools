"""Prove della guardia sintattica degli esiti."""

import unittest
import tempfile
import json
from pathlib import Path

from check_esito_unico import PATTERNS, check, findings


# Tabella separata dalla produzione: ogni voce deve dimostrare sia il
# rifiuto della forma debole sia l'accettazione di un controllo preciso.
EXAMPLES = {
    "zip_python_troncante": ("zip(a, b)", "zip(a, b, strict=True)"),
    "errore_ignorato": ("match f() { Err(e) => continue, Ok(x) => x }", "match f() { Err(e) => panic!(), Ok(x) => x }"),
    "rifiuto_jolly": ("assert!(matches!(f(), Err(_)));", "assert!(matches!(f(), Err(E::A)));"),
    "rifiuti_alternativi": ("assert!(matches!(f(), Err(A) | Err(B)));", "assert!(matches!(f(), Err(A)));"),
    "solo_messaggio": ('let errore = f().unwrap_err(); assert!(errore.to_string().contains("bad"));', 'let errore = assert_rifiuto!(f(), C::Schema, P::Validate); assert!(errore.to_string().contains("bad"));'),
    "raises_generico": ("with pytest.raises(Exception):\n    f()", "with pytest.raises(pd.PlenoraInvalidPlanError):\n    f()"),
    "messaggio_python": ('assert "bad" in str(e)', 'assert e.category == "invalid_plan"\nassert "bad" in str(e)'),
    "except_ignorato": ("try:\n    f()\nexcept Exception as e:\n    continue", "try:\n    f()\nexcept Exception:\n    raise"),
}


class EsitoUnicoTests(unittest.TestCase):
    def test_python_espressioni_e_import_non_retroattivi(self):
        for source, rule in [
            ('f = lambda a, b: zip(a, b)', 'zip_python_troncante'),
            ('f = lambda a=zip([], []): a', 'zip_python_troncante'),
            ('def f(a=zip([], [])): pass', 'zip_python_troncante'),
            ('@deco(zip([], []))\nclass C: pass', 'zip_python_troncante'),
            ('class C(list(zip([], []))): pass', 'zip_python_troncante'),
            ('class C(metaclass=deco(zip([], []))): pass', 'zip_python_troncante'),
            ('@decorate(zip([], []))\ndef f(): pass', 'zip_python_troncante'),
            ('from pytest import raises as r\nr(Exception)\nfrom other import raises as r', 'raises_generico'),
            ('from pytest import raises as r; r(Exception); from other import raises as r', 'raises_generico'),
            ('try:\n    f()\nexcept Exception as e:\n    if False:\n        raise\n    log(e)', 'except_ignorato'),
        ]:
            with self.subTest(source=source):
                self.assertIn(rule, [r for _, r, _ in findings(source, True, 'python')])
        self.assertEqual(findings('f = lambda a, b: zip(a, b, strict=True)', True, 'python'), [])

    def test_path_esterno_a_crates_e_modulo_pubblico(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'scripts').mkdir()
            (root / 'scripts/esito_unico_allowlist.json').write_text('[]')
            src = root / 'crates/demo/src'
            src.mkdir(parents=True)
            (root / 'supporto').mkdir()
            (src / 'lib.rs').write_text('#[cfg(test)] #[path="../../../supporto/prova.rs"] pub(crate) mod prova;')
            (root / 'supporto/prova.rs').write_text('use core::{matches as m}; fn t() { m! { f(), Err(_) }; }')
            self.assertEqual(len(check(root)), 1)
            self.assertIn('supporto/prova.rs', check(root)[0])

    def test_python_alias_import_e_formattazione(self):
        for prefix, call in [
            ('import pytest as p', 'p.raises'),
            ('from pytest import raises as x', 'x'),
            ('from pytest import raises', 'raises'),
            ('from pytest import *', 'raises'),
        ]:
            for exception in ['Exception', 'BaseException', 'pd.PlenoraError']:
                source = f'{prefix}\nwith {call}(\n    {exception}\n):\n    f()'
                self.assertIn('raises_generico', [r for _, r, _ in findings(source, True, 'python')])
            source = f'{prefix}\nwith {call}(pd.PlenoraSchemaError):\n    f()'
            self.assertEqual(findings(source, True, 'python'), [])

    def test_python_except_generico_alias_logging_e_scope(self):
        for body in ['log(e)', 'pass', 'continue', 'assert e',
                     'def nested():\n        raise']:
            source = 'from builtins import Exception as E\ntry:\n    f()\nexcept E as e:\n    ' + body
            self.assertIn('except_ignorato', [r for _, r, _ in findings(source, True, 'python')])
        for body in ['raise', 'assert e.category == "schema"',
                     'assert isinstance(e, pd.PlenoraSchemaError)']:
            source = 'try:\n    f()\nexcept BaseException as e:\n    ' + body
            self.assertEqual(findings(source, True, 'python'), [])

    def test_python_messaggio_categoria_fissata(self):
        for assertion in ['assert other.category == "schema"',
                          'assert e.category == other.category',
                          'e.category == "schema"',
                          'assert isinstance(e, Exception)']:
            source = assertion + '\nassert str(e).startswith("bad")'
            self.assertIn('messaggio_python', [r for _, r, _ in findings(source, True, 'python')])
        for assertion in ['assert e.category == "schema"',
                          'assert "schema" == e.category',
                          'assert type(e) is pd.PlenoraSchemaError']:
            self.assertEqual(findings(assertion + '\nassert str(e) == "bad"', True, 'python'), [])
        source = ('from pytest import raises as x\n'
                  'with x(pd.PlenoraSchemaError) as e:\n    f()\n'
                  'assert "bad" in str(e.value)')
        self.assertEqual(findings(source, True, 'python'), [])

    def test_python_zip_alias_e_strict_letterale(self):
        prefix = 'from builtins import zip as z\n'
        for call in ['z(a, b)', 'z(a, b, strict=False)', 'z(a, b, strict=flag)']:
            self.assertIn('zip_python_troncante', [r for _, r, _ in findings(prefix + call, True, 'python')])
        self.assertEqual(findings(prefix + 'z(a, b, strict=True)', True, 'python'), [])

    def test_tabella_ed_esempi_coincidono(self):
        self.assertEqual(set(PATTERNS), set(EXAMPLES))
        for rule, (positive, negative) in EXAMPLES.items():
            language = "python" if PATTERNS[rule][0] == "python" else "rust"
            with self.subTest(rule=rule):
                self.assertIn(rule, [r for _, r, _ in findings(positive, True, language)])
                self.assertNotIn(rule, [r for _, r, _ in findings(negative, True, language)])

    def test_helper_fuori_modulo(self):
        source = "fn helper() { assert!(f().is_err()); } #[test] fn t() { helper(); }"
        self.assertEqual(findings(source, False), []) # Produzione esclusa

    def test_helper_senza_asserzioni_raggiunto_dalle_prove(self):
        source = "fn helper() { match f() { Err(e) => continue, Ok(x) => x } } #[test] fn t() { helper(); }"
        self.assertEqual([r for _, r, _ in findings(source, False)], ["errore_ignorato"])

    def test_varianti_deboli(self):
        for source, rule in [
            ("#[cfg(test)] pub fn helper() { match f() { Err(e) => continue, _ => {} } } #[test] fn t() { helper(); }", "errore_ignorato"),
            ("#[cfg(test)] fn helper(a: [u8; 2]) { match f() { Err(ref e) => continue, _ => {} } } #[test] fn t() { helper([0, 0]); }", "errore_ignorato"),
            ("assert!(matches!(f(), Err(E::A | E::B)));", "rifiuti_alternativi"),
            ("assert!(matches!(f(), Err::<(), E>(_)));", "rifiuto_jolly"),
            ("assert!(matches!(f(), Err::<(), E>(E::A | E::B)));", "rifiuti_alternativi"),
            ('#[test] fn t() { let _ = ErrorCategory::Schema; let e = f().unwrap_err(); assert!(e.to_string().contains("bad")); }', "solo_messaggio"),
            ('#[test] fn t() { assert_eq!(other.category(), ErrorCategory::Schema); let e = f().unwrap_err(); assert!(e.to_string().contains("bad")); }', "solo_messaggio"),
        ]:
            with self.subTest(source=source):
                self.assertIn(rule, [r for _, r, _ in findings(source, "fn " not in source)])

    def test_categoria_deve_essere_asserita_sullo_stesso_valore(self):
        for source in [
            'let e = f().unwrap_err(); let _ = matches!(e, PlenoraError::Schema(_)); assert!(e.to_string().contains("bad"));',
            'fn helper(e: &PlenoraError) { assert_eq!(other.category(), ErrorCategory::Schema); } #[test] fn t() { let e = f().unwrap_err(); helper(&e); assert!(e.to_string().contains("bad")); }',
        ]:
            self.assertIn("solo_messaggio", [r for _, r, _ in findings(source, True)])
        source = 'def test_t():\n    e.category == "invalid_plan"\n    assert "bad" in str(e)'
        self.assertIn("messaggio_python", [r for _, r, _ in findings(source, True, "python")])
        source = 'pub fn helper() -> bool { f().is_err() } #[test] fn t() { assert!(helper()); }'
        self.assertEqual(findings(source, False), []) # Clippy sul codice compilato

    def test_categoria_dello_stesso_errore_e_messaggio_aggiuntivo(self):
        source = 'let e = f().unwrap_err(); assert_eq!(e.category(), ErrorCategory::Schema); assert!(e.to_string().contains("bad"));'
        self.assertEqual(findings(source, True), [])
        source = 'assert!(matches!(f(), Err(E::A) if a || b));'
        self.assertEqual(findings(source, True), [])

    def test_python_non_basta_un_controllo_estraneo(self):
        source = 'def test_t():\n    assert isinstance(42, int)\n    assert "bad" in str(e)'
        self.assertIn("messaggio_python", [r for _, r, _ in findings(source, True, "python")])
        for name in ["PlenoraError", "pd.PlenoraError", "Exception", "BaseException"]:
            self.assertIn("raises_generico", [r for _, r, _ in findings(f"with pytest.raises({name}):\n    f()", True, "python")])

    def test_moduli_esterni_e_python_scoperti(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            (root / "scripts/esito_unico_allowlist.json").write_text(json.dumps([]))
            src = root / "crates/demo/src"
            src.mkdir(parents=True)
            (src / "lib.rs").write_text('#[cfg(test)] mod support;\n#[cfg(test)] #[path = "elsewhere.rs"] mod altro;')
            (src / "support.rs").write_text('fn helper() { matches!(f(), Err(_)); }')
            (src / "elsewhere.rs").write_text('fn helper() { matches!(f(), Err(_)); }')
            py = root / "crates/plenora-data-py/python/tests"
            py.mkdir(parents=True)
            (py / "support.py").write_text('with pytest.raises(Exception):\n    f()')
            errors = check(root)
            self.assertEqual(len(errors), 3, errors)
            self.assertTrue(any("elsewhere.rs" in e for e in errors))
            self.assertTrue(any("support.py" in e for e in errors))

    def test_rifiuto_booleano_anche_multilinea(self):
        self.assertEqual(findings("assert!(f().\n is_err ( ));", True), []) # Clippy

    def test_commenti_stringhe_e_caratteri_non_sono_codice(self):
        source = '''// .is_err()
        /* esterno /* .is_err() */ interno */
        let x = r##".is_err()"##;
        let y = "\\\".is_err()";
        let z = '"';'''
        self.assertEqual(findings(source, True), [])

    def test_produzione_esclusa_e_modulo_prove_incluso(self):
        source = "fn f() { matches!(x, Err(_)); } #[cfg(test)] mod tests { fn t() { matches!(x, Err(_)); } }"
        self.assertEqual(len(findings(source, False)), 1)

    def test_test_diretto_e_modulo_esterno(self):
        source = "#[cfg(test)] mod tests; fn f() { matches!(x, Err(_)); } #[test] fn t() { matches!(x, Err(_)); }"
        self.assertEqual(len(findings(source, False)), 1)

    def test_errori_saltati_e_alternativa_successo(self):
        source = "match f() { Err(_) => continue, _ => {} }; x.is_ok() || matches!(x, Err(_));"
        self.assertEqual(len(findings(source, True)), 2)

    def test_aiuto_tipizzato_non_segnalato(self):
        self.assertEqual(findings("assert_rifiuto!(f(), ErrorCategory::Schema, ErrorPhase::Validate);", True), [])
