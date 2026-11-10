"""Prove del censimento delle firme Arrow, comprese mutazioni in memoria."""
import unittest
from unittest.mock import patch
import check_arrow_ingressi as guard


class ArrowIngressiTests(unittest.TestCase):
    def test_field_importato_scenario_coordinatore(self):
        source = 'use arrow_schema::Field; pub fn ingest(x: &Field) {}'
        self.assertIn('Field', guard.type_names())
        actual = guard.rust_functions(source)
        self.assertIn('ingest', actual)
        with self.assertRaisesRegex(AssertionError, 'non classificati.*ingest'):
            guard.check_members(actual, {})

    def test_impl_secondo_bound_scenario_coordinatore(self):
        source = 'pub fn ingest(x: impl Send + Array) {}'
        self.assertTrue(guard.rust_forbidden(source))

    def test_bound_arrow_in_ogni_posizione(self):
        for source in (
            'pub fn ingest(x: impl Send + Sync + Array) {}',
            "pub fn ingest(x: impl Send + 'static + Array) {}",
            'pub fn ingest(x: impl Fn() + Array) {}',
            'pub fn ingest(x: impl Fn(A, B) + Array) {}',
            'pub fn ingest<T: Fn(A, B) + Array>(x: T) {}',
            'pub fn ingest(x: impl Fn(A, B) -> bool + Array) {}',
            "pub fn ingest<T: Send + for<'a> Array>(x: T) {}",
            'pub fn ingest<T: Send + Array>(x: T) {}',
            'pub fn ingest<T>(x: T) where T: Send + Sync + Array {}',
            'impl<T> Receiver<T> where T: Send + Array { pub fn ingest(x: T) {} }',
            'pub trait Receiver<T: Send + Array> { fn ingest(x: T); }',
            'pub fn ingest(x: impl Send + arrow_array::Array) {}',
            'pub fn ingest<T>(x: T) where T: Send + arrow_array::Array {}',
            'pub trait Receiver { type Y = arrow_schema::Field; }',
            'impl Receiver for Thing { type Y = arrow_schema::Field; }',
            'use arrow_schema::Field; pub trait Receiver { type Y = Field; }',
            'use arrow_schema::Field; impl Receiver for Thing { type Y = Field; }',
            'use arrow_schema::Field as F; pub fn ingest(x: &F) {}',
        ):
            with self.subTest(source=source):
                self.assertTrue(guard.rust_forbidden(source))

    def test_metodi_trait_e_impl_pubblici(self):
        for source in (
            'pub trait Receiver { fn ingest(x: &Field); }',
            'pub unsafe trait Receiver { fn ingest(x: &Field); }',
            'pub trait Receiver { fn ingest(x: &Field) {} }',
            'pub trait Receiver { fn ingest(x: &Field); } impl Receiver for Thing { fn ingest(x: &Field) {} }',
            'impl imported::Receiver for Thing { fn ingest(x: &Field) {} }',
            'pub fn ingest(x: usize) {}',
        ):
            with self.subTest(source=source):
                actual = guard.rust_functions(source)
                self.assertIn('ingest', actual)
                with self.assertRaisesRegex(AssertionError, 'non classificati.*ingest'):
                    guard.check_members(actual, {})
        found = guard.rust_functions('pub trait Receiver { fn ingest(x: &Field); fn other(); }')
        self.assertEqual(found['ingest']['body'], '')
        self.assertEqual(set(found), {'ingest', 'other'})
        self.assertEqual(guard.rust_functions('trait Private { fn hidden(); }'), {})

    def test_catalogo_lock_e_nuovi_tipi(self):
        import genera_arrow_items as catalog
        names = catalog.type_names()
        self.assertTrue({'Field', 'Array', 'Buffer', 'DataType'} <= names)
        with patch.object(catalog, 'packages', return_value=[]):
            with self.assertRaisesRegex(AssertionError, 'non allineato al lock'):
                catalog.type_names()
        with patch.object(guard, 'type_names', return_value={'UnTipoUpstreamNuovo'}):
            guard.arrow_pattern.cache_clear()
            try:
                self.assertTrue(guard.rust_forbidden('type X = UnTipoUpstreamNuovo;'))
                self.assertTrue(guard.rust_forbidden('pub fn f(x: impl Send + UnTipoUpstreamNuovo) {}'))
            finally:
                guard.arrow_pattern.cache_clear()

    def test_varianti_rust_rifiutate(self):
        for source in (
            'impl<T: Array> Thing<T> { pub fn ingest(&self, x: &T) {} }',
            'use arrow_array::Int64Array as Values; pub fn ingest(x: &Values) {}',
            'use arrow_array::{Int64Array as Values}; pub fn ingest(x: &Values) {}',
            'use arrow_array as arrays; pub fn ingest(x: &arrays::Int64Array) {}',
            'type Values = arrow_array::Int64Array; pub fn ingest(x: &Values) {}',
            'use arrow_schema::Field; type Values = Field;',
            'pub fn ingest<T: ArrayAccessor>(x: T) {}',
            'pub fn ingest<T>(x: T) where T: RecordBatchReader {}',
            'pub fn ingest(x: impl arrow_array::Array) {}',
            'pub fn ingest<T: arrow_array::builder::ArrayBuilder>(x: T) {}',
            'pub fn ingest(x: impl arrow_array::builder::ArrayBuilder) {}',
            'pub use arrow_array::Int64Array as Values;',
        ):
            with self.subTest(source=source):
                self.assertTrue(guard.rust_forbidden(source))
                with patch.object(guard, 'production_rust', return_value=source):
                    with self.assertRaisesRegex(AssertionError, 'forma Rust vietata'):
                        guard.rust_inventory()

    def test_reexport_e_firme_dirette(self):
        self.assertFalse(guard.rust_forbidden('pub use arrow_array::Int64Array;'))
        self.assertEqual(guard.rust_functions('pub use arrow_array::Int64Array;'), {})
        for source in (
            'pub async fn ingest(x: &arrow_array::Int64Array) {}',
            'impl Thing { pub fn ingest(&self, x: &arrow_array::Int64Array) {} }',
        ):
            self.assertIn('ingest', guard.rust_functions(source))
            with self.assertRaisesRegex(AssertionError, 'non classificati.*ingest'):
                guard.check_members(guard.rust_functions(source), {})

    def test_python_statico_senza_filtro(self):
        for source in (
            'def ingest(data): return data.to_batches()',
            'async def ingest(data): return data.to_batches()',
            'class Reader:\n def ingest(self, data): return data.to_batches()',
            'class Reader:\n async def ingest(self, data): return data.to_batches()',
        ):
            with self.subTest(source=source):
                actual = guard.python_source_functions(source)
                with self.assertRaisesRegex(AssertionError, 'non classificati.*ingest'):
                    guard.check_members(actual, {})
                inventory = guard.python_source_inventory()
                inventory.update({'nuovo.py::' + key: value for key, value in actual.items()})
                with patch.object(guard, 'python_source_inventory', return_value=inventory):
                    with self.assertRaisesRegex(AssertionError, 'non classificati.*ingest'):
                        guard.check_python_sources()

    def test_registro_python_statico(self):
        self.assertGreater(guard.check_python_sources(), 30)

    def test_registro_conforme(self):
        self.assertGreater(guard.check_rust(), 200)

    def test_nuovo_ingresso_rust_non_classificato(self):
        actual = guard.rust_inventory()
        added = guard.rust_functions("pub fn ingest(table: &RecordBatch) -> Result<()> { Ok(()) }")
        actual["crates/futuro/src/lib.rs::ingest"] = added["ingest"]
        with patch.object(guard, "rust_inventory", return_value=actual):
            with self.assertRaisesRegex(AssertionError, "non classificati.*ingest"):
                guard.check_rust()

    def test_guardia_tardiva_fallisce(self):
        actual = guard.rust_inventory()
        key = "crates/plenora-io/src/ipc.rs::scrivi"
        actual[key]["body"] = "tabella.slice(0, 1);" + actual[key]["body"]
        with patch.object(guard, "rust_inventory", return_value=actual):
            with self.assertRaisesRegex(AssertionError, "guardia tardiva"):
                guard.check_rust()

    def test_parser_multilinea_commenti_output_e_alias(self):
        source = """// pub fn falso(table: &RecordBatch) {}
        pub fn ingest<'a>(
            table: &'a RecordBatch,
            arrays: Vec<ArrayRef>,
        ) -> Result<RecordBatch> { let _ = "{ }"; todo!() }
        pub fn output() -> ArrayData { todo!() }
        pub fn ingressi(input: Vec<Ingresso>) -> Result<()> { todo!() }
        """
        found = guard.rust_functions(source)
        self.assertEqual(set(found), {"ingest", "output", "ingressi"})
        self.assertEqual(len(found["ingest"]["args"]), 2)

    def test_generici_con_parentesi(self):
        source = "pub fn ingest<F: Fn(&RecordBatch) -> ArrayRef>(table: &RecordBatch, f: F) -> Result<()> { Ok(()) }"
        found = guard.rust_functions(source)
        self.assertEqual(found["ingest"]["args"], ["table: &RecordBatch", "f: F"])

    def test_import_prima_di_prepara_fallisce(self):
        actual = guard.rust_inventory()
        key = "crates/plenora-data-py/src/arrow_py.rs::importa"
        actual[key]["body"] = 'let _ = oggetto.call_method0("to_batches");' + actual[key]["body"]
        with patch.object(guard, "rust_inventory", return_value=actual):
            with self.assertRaisesRegex(AssertionError, "confine cambiato"):
                guard.check_rust()

    def test_helper_senza_propria_precondizione_fallisce(self):
        source = """/// Precondizione Arrow: precedente.
        pub fn primo(batch: &RecordBatch) -> usize { 0 }
        /// Solo descrizione.
        #[must_use]
        pub fn secondo(batch: &RecordBatch) -> usize { 0 }
        """
        self.assertNotIn("Precondizione Arrow", guard.rust_functions(source)["secondo"]["docs"])
        actual = guard.rust_inventory()
        actual["crates/plenora-core/src/memoria.rs::byte_dati"]["docs"] = "Solo descrizione"
        with patch.object(guard, "rust_inventory", return_value=actual):
            with self.assertRaises(AssertionError):
                guard.check_rust()
