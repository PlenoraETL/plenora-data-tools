"""Prove della guardia sintattica degli esiti."""

import unittest

from check_esito_unico import findings


class EsitoUnicoTests(unittest.TestCase):
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
        self.assertEqual(len(findings(source, True)), 2)

    def test_aiuto_tipizzato_non_segnalato(self):
        self.assertEqual(findings("assert_rifiuto!(f(), ErrorCategory::Schema, ErrorPhase::Validate);", True), [])
