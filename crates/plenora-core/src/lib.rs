//! plenora-core — fondamenta condivise del workspace (architettura.md).
//!
//! Ospita:
//! - il re-export unico di Arrow (decisione D0: un solo punto di versione);
//! - [`error`]: `PlenoraError`, l'unico tipo d'errore del workspace;
//! - [`limits`]: le tre famiglie di limiti (decisione D19, errori-e-limiti.md);
//! - [`catalog`]: `OperationDescriptor` unificato con versioni per-componente
//!   (decisione D17, piano-v5.md#identita-e-fingerprint);
//! - [`contract`]: contratti dati del grafo (`DataContract`, `FieldId`,
//!   provenienza/scope delle proprietà, `RuntimeStatistic`, `BatchSequence`) —
//!   decisioni D6/D16/D25, architettura.md#determinismo e architettura.md#planner-ed-executor;
//! - [`panic_policy`]: politica di processo per i panici, valida anche per
//!   chi ci usa come libreria;
//! - [`crs`]: contratto CRS fail-closed, indipendente dal backend.

pub mod capabilities;
pub mod catalog;
pub mod contract;
pub mod crs;
pub mod diagnostics;
pub mod error;
pub mod json;
pub mod limits;

// Dalla radice esce solo cio' che `Plan Budget 1.0` obbliga a pubblicare
// (`PLAN-013`); gli altri default restano in [`limits`], perche' toglierli
// dalla facciata dopo sarebbe una rottura.
pub use limits::DEFAULT_MAX_GOVERNED_MEMORY_BYTES;
pub mod panic_policy;

pub use error::{
    DiagnosticaSupplementare, ErrorCategory, ErrorPhase, EvidenzaDiLimite,
    FormaDegliAntenatiInvalida, LivelloAntenato, PlenoraError, PressioneDegliAntenati,
    RemoteEffect, Result, RetryDisposition, MAX_ANTENATI_OSSERVATI,
};

/// Costruisce un `RecordBatch` DICHIARANDO il numero di righe.
///
/// `RecordBatch::try_new` deriva le righe dalla prima colonna e rifiuta un
/// vettore di colonne vuoto. Un batch a zero colonne e righe positive e'
/// pero' legittimo (per esempio un `select_columns` che non seleziona nulla),
/// e con `try_new` un'operazione legittima fallirebbe.
///
/// Va usata in qualunque crate ovunque le colonne derivino dall'input; vive
/// qui perche' serve anche all'engine. `try_new` resta legittimo dove il
/// vettore non puo' essere vuoto per costruzione.
///
/// `rows_if_empty` conta solo senza colonne: altrimenti la cardinalita' la
/// decidono le colonne, come in `try_new`, cosi' la conversione di un sito
/// resta meccanica e non puo' dichiarare un numero sbagliato.
///
/// # Errors
///
/// [`PlenoraError::Arrow`] se le colonne non sono coerenti fra loro o con lo
/// schema.
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

/// Re-export unico di Arrow: tutti i crate del workspace dipendono da Arrow
/// solo tramite questo modulo.
pub mod arrow {
    pub use arrow_array as array;
    pub use arrow_ipc as ipc;
    pub use arrow_schema as schema;
    pub use arrow_select as select;

    pub use arrow_array::RecordBatch;
    pub use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};

    /// Versione dei crate Arrow in uso (`arrow-schema` & co., unica per
    /// decisione D0).
    ///
    /// I crate Arrow non espongono la versione a runtime: i test sotto la
    /// tengono allineata ai pin del workspace e del progetto fuzz. Entra
    /// nell'identita' dei grafi (piano-v5.md#identita-e-fingerprint), quindi
    /// un bump fa respingere i grafi gia' validati con `GRAPH_MISMATCH`; non
    /// entra nel `plan_hash`.
    pub const VERSION: &str = "59.2.0";
}

#[cfg(test)]
mod tests {
    /// I quattro crate Arrow del workspace sono un solo numero di versione
    /// (decisione D0): il test li verifica tutti, non solo `arrow-schema`.
    const CRATE_ARROW: [&str; 4] = ["arrow-array", "arrow-schema", "arrow-ipc", "arrow-select"];

    /// `arrow::VERSION` deve restare allineata al pin del workspace: un bump
    /// di Arrow senza aggiornarla renderebbe silenziosamente falso il check
    /// di versione nell'identita' dei grafi (piano-v5.md#identita-e-fingerprint).
    ///
    /// La versione dichiarata e' una sola (D0), ma i pin che la incarnano
    /// sono quattro: sorvegliarne uno solo lascerebbe agli altri tre la
    /// liberta' di divergere in silenzio.
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

    /// Il progetto fuzz ha un manifesto e un lock propri: ridichiara i crate
    /// Arrow e non partecipa al `cargo update` del workspace. Se resta
    /// indietro, il confine sugli input ostili viene esercitato su una libreria
    /// diversa da quella che il componente spedisce — cioe' i fuzz target
    /// smettono di dire qualcosa sul prodotto senza fallire.
    #[test]
    fn arrow_version_matches_the_fuzz_pin() {
        let manifest = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fuzz/Cargo.toml"
        ))
        .expect("Cargo.toml del progetto fuzz leggibile");
        for nome in CRATE_ARROW {
            let dichiarato = format!("{nome} = ");
            if !manifest.contains(&dichiarato) {
                continue; // il progetto fuzz non usa tutti e quattro i crate
            }
            let pin = format!("{nome} = \"={}\"", super::arrow::VERSION);
            assert!(
                manifest.contains(&pin),
                "pin fuzz di {nome} non allineato ad arrow::VERSION: atteso `{pin}`"
            );
        }
    }

    /// `capabilities::ARROW_VERSION` e' la versione che il componente dichiara
    /// verso l'esterno; `arrow::VERSION` e' quella che impone nel confronto di
    /// compatibilita' dei grafi. Devono coincidere: sono un alias, e il test
    /// lo tiene vero anche se tornasse un letterale.
    #[test]
    fn la_versione_dichiarata_e_quella_imposta_coincidono() {
        assert_eq!(crate::capabilities::ARROW_VERSION, super::arrow::VERSION);
    }
}
