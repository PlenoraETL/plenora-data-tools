//! plenora-core — fondamenta condivise del workspace (architettura.md).
//!
//! Ospita:
//! - il re-export unico di Arrow (decisione D0: un solo punto di versione);
//! - [`error`]: `PlenoraError`, fusione di `EngineError` e `GeoEngineError`;
//! - [`limits`]: le tre famiglie di limiti (decisione D19, errori-e-limiti.md);
//! - [`catalog`]: `OperationDescriptor` unificato con versioni per-componente
//!   (decisione D17, piano-v5.md#identita-e-fingerprint);
//! - [`contract`]: contratti dati del grafo (`DataContract`, `FieldId`,
//!   provenienza/scope delle proprietà, `RuntimeStatistic`, `BatchSequence`) —
//!   Fase 2A, decisioni D6/D16/D25, architettura.md#determinismo e architettura.md#planner-ed-executor;
//! - [`panic_policy`]: politica di processo per i panici, valida anche per
//!   chi ci usa come libreria;
//! - [`crs`]: contratto CRS fail-closed (trasloco da `crs.rs` di
//!   plenora-geo-tools-arrow previsto in Fase 1).

pub mod capabilities;
pub mod catalog;
pub mod contract;
pub mod crs;
pub mod diagnostics;
pub mod error;
pub mod json;
pub mod limits;
pub mod panic_policy;

pub use error::{
    DiagnosticaSupplementare, ErrorCategory, ErrorPhase, EvidenzaDiLimite, PlenoraError,
    PressioneDegliAntenati, RemoteEffect, Result, RetryDisposition, MAX_ANTENATI_OSSERVATI,
};

/// Costruisce un `RecordBatch` DICHIARANDO il numero di righe.
///
/// `RecordBatch::try_new` deriva le righe dalla prima colonna. Con un vettore
/// di colonne VUOTO non c'e' una prima colonna, e arrow **rifiuta** di
/// costruire il batch: `must either specify a row count or at least one
/// column`. Un batch a zero colonne e righe positive e' pero' legittimo in
/// Arrow e il progetto ne produce: una tabella puo' avere una cardinalita'
/// senza avere attributi, ed e' il risultato naturale di un `select_columns`
/// che non seleziona nulla o di un input che nasce cosi'.
///
/// Ricostruirlo con `try_new` fa quindi FALLIRE un'operazione legittima:
/// `concat` di due e tre righe non produceva cinque righe, non produceva
/// nulla. E' un difetto di disponibilita', non di correttezza — vale la pena
/// dirlo con precisione, perche' la prima stesura di questa doc parlava di
/// «zero righe restituite in silenzio» e non era vero.
///
/// # Dove va usato
///
/// Ovunque l'insieme delle colonne DERIVI dall'input, in **qualunque** crate:
/// se le colonne vengono dall'input, possono essere zero. Vive qui e non nei
/// kernel proprio per questo — la prima versione stava in
/// `plenora-kernels-table` e l'engine, che non poteva vederla, e' rimasto
/// indietro in tre punti (rivestimento dello schema in pubblicazione,
/// compattazione dello staging, normalizzazione `LargeUtf8`). Un invariante
/// del workspace non puo' abitare in una foglia.
///
/// Resta legittimo `RecordBatch::try_new` dove le colonne sono COSTRUITE
/// dall'operazione e il vettore non puo' essere vuoto per costruzione.
///
/// # Semantica di `rows_if_empty`
///
/// Quando `columns` NON e' vuoto la cardinalita' la decidono le colonne,
/// esattamente come faceva `try_new`: `rows_if_empty` viene ignorato. Serve
/// solo per il caso senza colonne, l'unico in cui arrow non ha da dove
/// dedurla.
///
/// E' voluto che sia cosi'. La conversione di un sito diventa meccanica —
/// si passa la cardinalita' della sorgente piu' vicina — e non introduce il
/// rischio di dichiarare un numero sbagliato per un batch che le colonne ce
/// le ha: se ci fosse da ragionare caso per caso, e' proprio il ragionamento
/// che e' andato storto due volte.
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
    /// I crate Arrow non espongono la propria versione a runtime: la costante
    /// e' tenuta allineata da test dedicati a TUTTI i pin che la incarnano —
    /// i quattro del `Cargo.toml` di root e quelli del progetto fuzz, che ha
    /// un lock proprio. Entra nell'identita' dei grafi validati
    /// (piano-v5.md#identita-e-fingerprint): un bump la fa divergere dai grafi
    /// gia' validati, che `check_compatibility` respinge con `GRAPH_MISMATCH`.
    /// Non entra invece nel `plan_hash`, che resta stabile fra versioni Arrow.
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
    /// La versione dichiarata e' una sola (D0), ma i pin che la incarnano sono
    /// quattro: sorvegliarne uno solo lasciava agli altri tre la liberta' di
    /// divergere in silenzio.
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
    /// compatibilita' dei grafi. Devono essere lo stesso valore: oggi lo sono
    /// per costruzione (alias), e questo test lo tiene vero se qualcuno
    /// reintroduce un letterale.
    #[test]
    fn la_versione_dichiarata_e_quella_imposta_coincidono() {
        assert_eq!(crate::capabilities::ARROW_VERSION, super::arrow::VERSION);
    }
}
