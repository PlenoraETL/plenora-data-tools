//! Impalcature condivise dai test del crate: costruttori di batch e confronti
//! esatti tra un percorso veloce e il suo oracolo. Nessuna logica di
//! produzione: il modulo esiste solo sotto `cfg(test)`.

use std::fmt::Debug;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, BooleanArray, Date32Array, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::Result;

/// Batch con un campo nullable per colonna, del tipo della colonna.
pub fn nullable_batch(pairs: Vec<(&str, ArrayRef)>) -> RecordBatch {
    let fields = pairs
        .iter()
        .map(|(name, column)| Field::new(*name, column.data_type().clone(), true))
        .collect::<Vec<_>>();
    let columns = pairs.into_iter().map(|(_, column)| column).collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("batch di test")
}

/// Batch da campi e colonne dichiarati dal test.
pub fn batch_from_fields(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("fixture")
}

/// Batch di una sola colonna, con nome, tipo e nullabilita' espliciti.
pub fn single_column_batch(
    name: &str,
    column: ArrayRef,
    data_type: DataType,
    nullable: bool,
) -> RecordBatch {
    batch_from_fields(vec![Field::new(name, data_type, nullable)], vec![column])
}

/// Confronto rigoroso tra l'output di un percorso veloce e quello del suo
/// oracolo. Per ogni batch: numero di righe e colonne, metadata dello schema.
/// Per ogni campo: nome, tipo, nullabilita' e maschera null riga per riga.
/// Per i valori, secondo il tipo della colonna:
/// - `Int64`, `UInt64`, `Boolean`, `Utf8`, `Date32`: valore riga per riga
///   sulle righe non null, poi uguaglianza degli `ArrayData`;
/// - `Float64`: bit riga per riga (`to_bits`: payload NaN e zeri con segno
///   distinti), poi uguaglianza degli `ArrayData`;
/// - ogni altro tipo (`Int32`, `LargeUtf8`, dictionary, timestamp, decimal,
///   list, ...): solo uguaglianza degli `ArrayData` (tipo, lunghezza, null e
///   valori logici, buffer primitivi byte per byte).
///
/// Deviazione dalle copie locali che questo helper sostituisce: le copie di
/// reshape, aggregation e security confrontavano gli altri tipi via
/// `scalar_as_string`, quella di joins li rifiutava con un panic. Qui il
/// confronto sugli altri tipi e' affidato solo agli `ArrayData`, che lo
/// coprono in modo esatto anche dove il profilo scalare non arriva (`Int32`
/// nei test di setops); in cambio i messaggi di errore non indicano la riga.
/// Rientro: un tipo che serva diagnosticare riga per riga si aggiunge al
/// `match` con il suo confronto tipizzato.
pub fn assert_batches_identical(fast: &RecordBatch, reference: &RecordBatch) {
    assert_eq!(fast.num_rows(), reference.num_rows(), "righe");
    assert_eq!(fast.num_columns(), reference.num_columns(), "colonne");
    let fast_schema = fast.schema();
    let reference_schema = reference.schema();
    assert_eq!(
        fast_schema.metadata(),
        reference_schema.metadata(),
        "metadata schema"
    );
    for index in 0..fast.num_columns() {
        let fast_field = fast_schema.field(index);
        let reference_field = reference_schema.field(index);
        let name = fast_field.name();
        assert_eq!(name, reference_field.name(), "nome colonna {index}");
        assert_eq!(
            fast_field.data_type(),
            reference_field.data_type(),
            "tipo colonna {name}"
        );
        assert_eq!(
            fast_field.is_nullable(),
            reference_field.is_nullable(),
            "nullabilita' colonna {name}"
        );
        let fast_column = fast.column(index).as_ref();
        let reference_column = reference.column(index).as_ref();
        for row in 0..fast.num_rows() {
            assert_eq!(
                fast_column.is_null(row),
                reference_column.is_null(row),
                "null riga {row} colonna {name}"
            );
        }
        let (a, b, v) = (fast_column, reference_column, "valore");
        match fast_field.data_type() {
            DataType::Int64 => assert_values(a, b, v, name, |c: &Int64Array, r| c.value(r)),
            DataType::UInt64 => assert_values(a, b, v, name, |c: &UInt64Array, r| c.value(r)),
            DataType::Float64 => {
                assert_values(a, b, "bit", name, |c: &Float64Array, r| {
                    c.value(r).to_bits()
                });
            }
            DataType::Boolean => assert_values(a, b, v, name, |c: &BooleanArray, r| c.value(r)),
            DataType::Utf8 => {
                assert_values(a, b, v, name, |c: &StringArray, r| c.value(r).to_owned());
            }
            DataType::Date32 => assert_values(a, b, v, name, |c: &Date32Array, r| c.value(r)),
            // Gli altri tipi (Int32, dictionary, timestamp, decimal, ...)
            // si confrontano solo tramite `ArrayData`, qui sotto.
            _ => {}
        }
        assert_eq!(a.to_data(), b.to_data(), "buffer colonna {name}");
    }
}

/// Confronta riga per riga i valori non null di due colonne dello stesso tipo
/// Arrow, estratti da `value`; `what` nomina il confronto nel messaggio.
fn assert_values<A: Array + 'static, T: PartialEq + Debug>(
    fast: &dyn Array,
    reference: &dyn Array,
    what: &str,
    name: &str,
    value: impl Fn(&A, usize) -> T,
) {
    let fast = fast.as_any().downcast_ref::<A>().expect("downcast fast");
    let reference = reference
        .as_any()
        .downcast_ref::<A>()
        .expect("downcast reference");
    for row in 0..fast.len() {
        if fast.is_valid(row) {
            assert_eq!(
                value(fast, row),
                value(reference, row),
                "{what} riga {row} colonna {name}"
            );
        }
    }
}

/// Equivalenza fast/generico su qualunque esito: stessi batch, oppure
/// stessa categoria, stesso messaggio e stessa diagnostica row-scoped.
pub fn assert_same_outcome(fast: Result<RecordBatch>, generic: Result<RecordBatch>) {
    match (fast, generic) {
        (Ok(fast), Ok(generic)) => assert_eq!(fast, generic),
        (Err(fast), Err(generic)) => {
            assert_eq!(fast.category(), generic.category());
            assert_eq!(fast.to_string(), generic.to_string());
            assert_eq!(fast.row_diagnostics(), generic.row_diagnostics());
        }
        (fast, generic) => panic!(
            "fast e generico divergono: fast ok={}, generico ok={}",
            fast.is_ok(),
            generic.is_ok()
        ),
    }
}

/// Come [`assert_same_outcome`], con il confronto dei batch di
/// [`assert_batches_identical`]: i `Float64` si confrontano per bit, quindi
/// un NaN nell'output non rende diversi due batch identici.
pub fn assert_same_outcome_bits(fast: Result<RecordBatch>, reference: Result<RecordBatch>) {
    match (fast, reference) {
        (Ok(fast), Ok(reference)) => assert_batches_identical(&fast, &reference),
        (Err(fast), Err(reference)) => {
            assert_eq!(fast.category(), reference.category());
            assert_eq!(fast.to_string(), reference.to_string());
            assert_eq!(fast.row_diagnostics(), reference.row_diagnostics());
        }
        (fast, reference) => panic!(
            "percorso veloce e oracolo divergono: veloce ok={}, oracolo ok={}",
            fast.is_ok(),
            reference.is_ok()
        ),
    }
}

// --- suite lunga -------------------------------------------------------------

/// `true` con `PLENORA_TEST_LUNGHI=1`: la suite lunga, obbligatoria prima del
/// merge (README, «Suite lunga»). Assente o `0`: la suite di default, con un
/// sottoinsieme deterministico degli stessi casi. Un altro valore e' un
/// errore del chiamante e ferma il test, invece di scegliere in silenzio.
pub fn test_lunghi() -> bool {
    match std::env::var("PLENORA_TEST_LUNGHI") {
        Err(std::env::VarError::NotPresent) => false,
        Ok(valore) if valore == "0" => false,
        Ok(valore) if valore == "1" => true,
        _ => panic!("PLENORA_TEST_LUNGHI vale 1 (suite lunga) o 0"),
    }
}

/// `pieni` nella suite lunga, `ridotti` in quella di default.
pub fn casi(ridotti: u32, pieni: u32) -> u32 {
    if test_lunghi() {
        pieni
    } else {
        ridotti
    }
}
