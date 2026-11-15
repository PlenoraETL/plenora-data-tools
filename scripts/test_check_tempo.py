"""Mutazioni sentinella della guardia temporale."""
import unittest
import check_tempo


class Tempo(unittest.TestCase):
    def test_conversioni_infallibili_e_alias(self):
        for source in [
            "fn f() { let s = SystemTime::from(external); }",
            "use std::time::SystemTime as Clock; fn f() { Clock::from(x); }",
            "type Clock = std::time::SystemTime; fn f() { Clock::from(x); }",
            "fn f() { let x: SystemTime = external.into(); }",
            "fn f() { Utc.timestamp_millis_opt(raw).unwrap(); }",
            "fn f() { DateTime::from_timestamp(raw, 0).expect(\"bad\"); }",
            "fn f() { Duration::from_secs_f64(raw); }",
            "fn f() { value.timestamp_nanos(); }",
            "fn f() { Utc.timestamp_opt(v,0).single().unwrap_or(epoch); }",
            "fn f(v: DateTime<Utc>) -> SystemTime { v.into() }",
            "fn f() { <SystemTime as From<_>>::from(v); }",
            "fn f() { Utc.timestamp_opt(v,0).single().unwrap(); }",
        ]:
            with self.subTest(source=source):
                self.assertGreater(len(check_tempo.violations(source)), 0)

    def test_commenti_letterali_e_test_esclusi(self):
        self.assertEqual(check_tempo.violations(
            '// SystemTime::from(x)\nfn f() { let s = "DateTime::from(x)"; }'
            '#[cfg(test)] mod tests { fn f() { SystemTime::from(x); } }'), [])

    def test_frazione_assente_non_e_fallback_di_data(self):
        self.assertEqual(check_tempo.violations(
            'fn f() { DateTime::from_timestamp(seconds, fraction.unwrap_or(0))?; }'), [])

    def test_conversione_checked(self):
        self.assertEqual(check_tempo.violations(
            'fn f() { DateTime::from_timestamp(raw, 0).ok_or_else(error)?; }'), [])

    def test_barriera_prima_di_arrow_e_senza_gruppi(self):
        source = (
            "fn verifica_colonne(m: Metadata) { "
            "if m.schema_descr().columns().iter().any(|c| c.physical_type() == TipoFisico::INT96) {"
            "return Err(PlenoraError::Unsupported(message)); } }"
            "pub fn leggi() { verifica_colonne(&letti)?; ArrowReaderMetadata::try_new(input); }"
            "pub fn verifica_schema() {}")
        self.assertEqual(check_tempo.barrier_errors(source), [])
        for old, new in [
            ("verifica_colonne(&letti)?;", ""),
            ("schema_descr().columns()", "row_groups()"),
            ("TipoFisico::INT96", "TipoFisico::INT64"),
            ("verifica_colonne(&letti)?; ArrowReaderMetadata::try_new(input);",
             "ArrowReaderMetadata::try_new(input); verifica_colonne(&letti)?;"),
        ]:
            with self.subTest(old=old):
                self.assertEqual(len(check_tempo.barrier_errors(source.replace(old, new))), 1)
