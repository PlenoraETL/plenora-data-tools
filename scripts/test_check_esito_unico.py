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
    "rifiuto_booleano": ("assert!(f().is_err());", "assert_rifiuto!(f(), C::Schema, P::Validate);"),
    "errore_ignorato": ("match f() { Err(e) => continue, Ok(x) => x }", "match f() { Err(e) => panic!(), Ok(x) => x }"),
    "successo_o_rifiuto": ("x.is_ok() || matches!(x, Err(E::A))", "assert_eq!(x.unwrap(), 3);"),
    "confronto_booleano": ("assert_eq!(a.is_ok(), b.is_ok());", "assert_eq!(a, b);"),
    "rifiuto_jolly": ("assert!(matches!(f(), Err(_)));", "assert!(matches!(f(), Err(E::A)));"),
    "rifiuti_alternativi": ("assert!(matches!(f(), Err(A) | Err(B)));", "assert!(matches!(f(), Err(A)));"),
    "solo_messaggio": ('let errore = f().unwrap_err(); assert!(errore.to_string().contains("bad"));', 'let errore = assert_rifiuto!(f(), C::Schema, P::Validate); assert!(errore.to_string().contains("bad"));'),
    "raises_generico": ("with pytest.raises(Exception):\n    f()", "with pytest.raises(pd.PlenoraInvalidPlanError):\n    f()"),
    "messaggio_python": ('assert "bad" in str(e)', 'assert e.category == "invalid_plan"\nassert "bad" in str(e)'),
    "except_ignorato": ("try:\n    f()\nexcept Exception as e:\n    continue", "try:\n    f()\nexcept Exception:\n    raise"),
}


class EsitoUnicoTests(unittest.TestCase):
    def test_tabella_ed_esempi_coincidono(self):
        self.assertEqual(set(PATTERNS), set(EXAMPLES))
        for rule, (positive, negative) in EXAMPLES.items():
            language = "python" if PATTERNS[rule][0] == "python" else "rust"
            with self.subTest(rule=rule):
                self.assertIn(rule, [r for _, r, _ in findings(positive, True, language)])
                self.assertNotIn(rule, [r for _, r, _ in findings(negative, True, language)])

    def test_helper_fuori_modulo(self):
        source = "fn helper() { assert!(f().is_err()); } #[test] fn t() { helper(); }"
        self.assertEqual(len(findings(source, False)), 1)

    def test_helper_senza_asserzioni_raggiunto_dalle_prove(self):
        source = "fn helper() { match f() { Err(e) => continue, Ok(x) => x } } #[test] fn t() { helper(); }"
        self.assertEqual([r for _, r, _ in findings(source, False)], ["errore_ignorato"])

    def test_varianti_deboli(self):
        for source, rule in [
            ("pub fn helper() { match f() { Err(e) => continue, _ => {} } } #[test] fn t() { helper(); }", "errore_ignorato"),
            ("fn helper(a: [u8; 2]) { match f() { Err(ref e) => continue, _ => {} } } #[test] fn t() { helper([0, 0]); }", "errore_ignorato"),
            ("assert!(matches!(f(), Err(E::A | E::B)));", "rifiuti_alternativi"),
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
        self.assertIn("rifiuto_booleano", [r for _, r, _ in findings(source, False)])

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
            (src / "support.rs").write_text('fn helper() { f().is_err(); }')
            (src / "elsewhere.rs").write_text('fn helper() { f().is_err(); }')
            py = root / "crates/plenora-data-py/python/tests"
            py.mkdir(parents=True)
            (py / "support.py").write_text('with pytest.raises(Exception):\n    f()')
            errors = check(root)
            self.assertEqual(len(errors), 3, errors)
            self.assertTrue(any("elsewhere.rs" in e for e in errors))
            self.assertTrue(any("support.py" in e for e in errors))

    def test_rifiuto_booleano_anche_multilinea(self):
        self.assertEqual(len(findings("assert!(f().\n is_err ( ));", True)), 1)

    def test_commenti_stringhe_e_caratteri_non_sono_codice(self):
        source = '''// .is_err()
        /* esterno /* .is_err() */ interno */
        let x = r##".is_err()"##;
        let y = "\\\".is_err()";
        let z = '"';'''
        self.assertEqual(findings(source, True), [])

    def test_produzione_esclusa_e_modulo_prove_incluso(self):
        source = "fn f() { x.is_err(); } #[cfg(test)] mod tests { fn t() { x.is_err(); } }"
        self.assertEqual(len(findings(source, False)), 1)

    def test_test_diretto_e_modulo_esterno(self):
        source = "#[cfg(test)] mod tests; fn f() { x.is_err(); } #[test] fn t() { x.is_err(); }"
        self.assertEqual(len(findings(source, False)), 1)

    def test_errori_saltati_e_alternativa_successo(self):
        source = "match f() { Err(_) => continue, _ => {} }; x.is_ok() || matches!(x, Err(_));"
        self.assertEqual(len(findings(source, True)), 3)

    def test_aiuto_tipizzato_non_segnalato(self):
        self.assertEqual(findings("assert_rifiuto!(f(), ErrorCategory::Schema, ErrorPhase::Validate);", True), [])
