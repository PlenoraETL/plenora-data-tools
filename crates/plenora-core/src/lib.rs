//! plenora-core: le fondamenta condivise del workspace.
//!
//! Ospita:
//! - il re-export unico di Arrow ([`arrow`]: un solo punto di versione per
//!   tutto il workspace);
//! - [`error`]: `PlenoraError`, l'unico tipo d'errore del workspace, e i
//!   suoi assi (categoria, fase, effetto, ritentativo);
//! - [`limits`]: i limiti di risorsa (`Limits`), quelli di complessità del
//!   piano (`PlanLimits`) e i controlli d'espansione;
//! - [`catalog`]: il catalogo delle operazioni (`OperationDescriptor`), con
//!   versioni per componente;
//! - [`contract`]: i contratti dati (`DataContract`, `FieldId`, provenienza
//!   e ambito delle proprietà, `RuntimeStatistic`, `BatchSequence`) e la loro
//!   lettura da e verso uno schema Arrow;
//! - [`diagnostics`]: il payload della diagnostica per riga
//!   (`plenora-row-diagnostics-v1`);
//! - [`panic_policy`]: politica di processo per i panici, valida anche per
//!   chi ci usa come libreria;
//! - [`crs`]: contratto CRS fail-closed, la tabella dei CRS integrati e la
//!   riproiezione fra di loro in Rust puro;
//! - [`json`]: lettura del JSON di controllo che rifiuta le chiavi ripetute;
//! - [`esadecimale`]: esadecimale minuscolo per digest e identificativi;
//! - [`memoria`]: byte Arrow di tabelle (per allocazione) e colonne (per
//!   vista), la misura unica di runner e kernel.

pub mod catalog;
pub mod contract;
pub mod crs;
pub mod diagnostics;
pub mod error;
pub mod esadecimale;
pub mod json;
pub mod limits;
pub mod memoria;

// Dalla radice esce solo il default del budget di memoria, che il contratto
// del budget del progetto d'origine obbligava a pubblicare; gli altri
// default restano in [`limits`], perché toglierli dalla facciata dopo
// sarebbe una rottura.
pub use limits::DEFAULT_MAX_GOVERNED_MEMORY_BYTES;
pub mod panic_policy;
pub mod tipo_arrow;

pub use error::{ErrorCategory, ErrorPhase, PlenoraError, RemoteEffect, Result, RetryDisposition};

/// Costruisce un `RecordBatch` DICHIARANDO il numero di righe.
///
/// `RecordBatch::try_new` deriva le righe dalla prima colonna e rifiuta un
/// vettore di colonne vuoto. Un batch a zero colonne e righe positive e'
/// pero' legittimo (per esempio un `select_columns` che non seleziona nulla),
/// e con `try_new` un'operazione legittima fallirebbe.
///
/// Va usata in qualunque crate ovunque le colonne derivino dall'input; vive
/// qui perché la usano kernel, runner e `plenora-io`. `try_new` resta
/// legittimo dove il vettore non può essere vuoto per costruzione.
///
/// `rows_if_empty` conta solo senza colonne: altrimenti la cardinalita' la
/// decidono le colonne, come in `try_new`, cosi' la conversione di un sito
/// resta meccanica e non puo' dichiarare un numero sbagliato.
///
/// # Errors
///
/// [`PlenoraError::DataMapping`] (`arrow error: <codice>`) se le colonne non
/// sono coerenti fra loro o con lo schema.
pub fn batch_with_rows(
    schema: std::sync::Arc<arrow::schema::Schema>,
    columns: Vec<arrow::array::ArrayRef>,
    rows_if_empty: usize,
) -> Result<arrow::array::RecordBatch> {
    let rows = columns
        .first()
        .map_or(rows_if_empty, arrow::array::Array::len);
    let options = arrow::array::RecordBatchOptions::new().with_row_count(Some(rows));
    Ok(arrow::array::RecordBatch::try_new_with_options(
        schema, columns, &options,
    )?)
}

/// Errore comune dei costruttori senza panico: il tipo non ammette array.
fn tipo_senza_array() -> PlenoraError {
    PlenoraError::Schema("schema Arrow non valido: un tipo non ammette nessun array".to_owned())
}

/// L'array vuoto di `tipo`, senza panico (`new_empty_array` va in panico su
/// un tipo che non ammette array, come [`batch_vuoto`]).
///
/// # Errors
///
/// [`PlenoraError::Schema`] se il tipo non ammette nessun array.
pub fn array_vuoto(tipo: &arrow::schema::DataType) -> Result<arrow::array::ArrayRef> {
    panic_policy::barriera_di_dipendenza(|| arrow::array::new_empty_array(tipo))
        .map_err(|_| tipo_senza_array())
}

/// `righe` null di `tipo`, senza panico (`new_null_array` va in panico su un
/// tipo che non ammette array, per esempio una `Union` senza figli, anche
/// con zero righe).
///
/// # Errors
///
/// [`PlenoraError::Schema`] se il tipo non ammette nessun array.
pub fn array_null(tipo: &arrow::schema::DataType, righe: usize) -> Result<arrow::array::ArrayRef> {
    panic_policy::barriera_di_dipendenza(|| arrow::array::new_null_array(tipo, righe))
        .map_err(|_| tipo_senza_array())
}

/// La tabella vuota di `schema`, senza panico.
///
/// `RecordBatch::new_empty` e' infallibile e va in panico se un tipo dello
/// schema non corrisponde a nessun array costruibile (un `Map` il cui figlio
/// non e' una struct): uno schema letto da un file o ricevuto da un
/// chiamante puo' esserlo, e senza righe nessun decoder lo avrebbe gia'
/// rifiutato. La costruzione sta in una barriera di dipendenza.
///
/// # Errors
///
/// [`PlenoraError::Schema`] se lo schema non ammette una tabella vuota.
pub fn batch_vuoto(
    schema: std::sync::Arc<arrow::schema::Schema>,
) -> Result<arrow::array::RecordBatch> {
    panic_policy::barriera_di_dipendenza(|| arrow::array::RecordBatch::new_empty(schema))
        .map_err(|_| tipo_senza_array())
}

/// Re-export unico di Arrow: tutti i crate del workspace dipendono da Arrow
/// solo tramite questo modulo.
pub mod arrow {
    pub use arrow_array as array;
    pub use arrow_ipc as ipc;
    pub use arrow_schema as schema;
    pub use arrow_select as select;

    pub use arrow_array::RecordBatch;
    pub use arrow_schema::{ArrowError, DataType, Field, Metadata, Schema, SchemaRef};

    /// Versione dei crate Arrow in uso (`arrow-schema` e gli altri: una sola
    /// per tutto il workspace).
    ///
    /// I crate Arrow non espongono la versione a runtime: il test sotto la
    /// tiene allineata ai pin del workspace. Nessun codice di questo
    /// repository la legge; nel progetto d'origine entrava nell'identità dei
    /// grafi validati.
    pub const VERSION: &str = "60.0.0";
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::arrow::schema::{DataType, Field, Schema};

    /// Uno schema `Map` con un figlio che non e' una struct: nessun array lo
    /// incarna, e la tabella vuota e' un errore, non un panico.
    #[test]
    fn batch_vuoto_rifiuta_uno_schema_senza_array() {
        let mappa = DataType::Map(Arc::new(Field::new("voci", DataType::UInt64, false)), false);
        let schema = Arc::new(Schema::new(vec![Field::new("m", mappa, true)]));
        let errore = super::batch_vuoto(schema).unwrap_err();
        assert_eq!(errore.category(), super::ErrorCategory::Schema);
        let valido = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, true)]));
        assert_eq!(super::batch_vuoto(valido).unwrap().num_rows(), 0);
    }

    /// Gli stessi tipi senza array: errore anche per l'array vuoto e per i
    /// null, e una `Union` senza figli anche con zero righe.
    #[test]
    fn array_vuoto_e_array_null_rifiutano_i_tipi_senza_array() {
        let mappa = DataType::Map(Arc::new(Field::new("voci", DataType::UInt64, false)), false);
        let unione = DataType::Union(
            crate::arrow::schema::UnionFields::empty(),
            crate::arrow::schema::UnionMode::Dense,
        );
        assert!(super::array_vuoto(&mappa).is_err());
        assert!(super::array_null(&mappa, 3).is_err());
        assert!(super::array_null(&unione, 0).is_err());
        assert_eq!(
            super::array_null(&DataType::Utf8, 2).unwrap().null_count(),
            2
        );
        assert_eq!(super::array_vuoto(&DataType::Int64).unwrap().len(), 0);
    }

    /// I quattro crate Arrow del workspace sono un solo numero di versione:
    /// il test li verifica tutti, non solo `arrow-schema`.
    const CRATE_ARROW: [&str; 4] = ["arrow-array", "arrow-schema", "arrow-ipc", "arrow-select"];

    /// `arrow::VERSION` deve restare allineata al pin del workspace: un bump
    /// di Arrow senza aggiornarla la renderebbe falsa in silenzio.
    ///
    /// La versione dichiarata è una sola, ma i pin che la incarnano sono
    /// quattro: sorvegliarne uno solo lascerebbe agli altri tre la libertà
    /// di divergere in silenzio.
    #[test]
    fn arrow_version_matches_the_workspace_pin() {
        let manifest =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .expect("Cargo.toml del workspace leggibile");
        for nome in CRATE_ARROW {
            let pin = format!("{nome} = \"={}\"", super::arrow::VERSION);
            assert!(
                manifest.contains(&pin),
                "pin di {nome} non allineato ad arrow::VERSION: atteso `{pin}`"
            );
        }
    }
}
