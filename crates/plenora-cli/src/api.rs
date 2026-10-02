//! La superficie Rust delle quattro operazioni pubbliche.
//!
//! Ogni funzione restituisce il documento JSON dell'operazione, lo stesso
//! che la CLI mette in `result`: la CLI è una spellatura di queste funzioni
//! (Public Surfaces 1.0, SURF-017), non una seconda implementazione. Gli
//! errori sono `PlenoraError`, con i quattro assi di `plenora-error-v1`.
//!
//! La mappa operazione → export è [`crate::capacita::mappa_rust`]; il test
//! `tests/superficie_rust.rs` la compila come un crate consumatore che
//! importa solo questi export.

use std::io::Read;
use std::path::Path;

use plenora_core::arrow::schema::Schema;
use plenora_core::contract::arrow_metadata::{
    GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, PLENORA_CONTRACT_VERSION_KEY,
    PLENORA_FIELD_ID_KEY, PLENORA_GEOMETRY_CRS_DEFINITION_KEY, PLENORA_GEOMETRY_NAMESPACE_PREFIX,
};
use plenora_core::contract::arrow_schema::{
    arrow_schema_from_contract, contract_from_arrow_schema,
};
use plenora_core::crs::resolve_crs;
use plenora_core::limits::PlanLimits;
use plenora_core::tipo_arrow::descrivi_tipo;
use plenora_core::{ErrorPhase, PlenoraError, Result, DEFAULT_MAX_GOVERNED_MEMORY_BYTES};
use plenora_io::{
    esegui_da_file_interrompibile, leggi_tabella, valida_da_file, FileIngresso, FileUscita,
    Formato, OpzioniScrittura,
};
use plenora_pipeline::{normalizza_schema, Interruzione, Pipeline};
use serde_json::{json, Value};

use crate::catalogo::FORMATO_PIANO;
use crate::operazioni::{ARROW_FILE, ARROW_STREAM, PARQUET};

/// `data.catalog`: il registro dei kernel di questo artefatto
/// (`plenora-data-kernel-catalog-v1`).
#[must_use]
pub fn catalogo() -> Value {
    crate::catalogo::documento()
}

/// Legge un piano da file: al più `max_plan_json_bytes` byte, UTF-8, poi
/// [`Pipeline::from_json`].
///
/// # Errors
///
/// `Io` dall'apertura e dalla lettura (messaggio pubblico senza percorso);
/// `InvalidPlan` oltre il limite di byte, per un testo non UTF-8 e per gli
/// errori di [`Pipeline::from_json`].
pub fn leggi_piano(percorso: &Path) -> Result<Pipeline> {
    let massimo = PlanLimits::default().max_plan_json_bytes;
    let errore_io =
        |errore| PlenoraError::io_con_contesto("piano", errore).with_phase(ErrorPhase::Read);
    let file = std::fs::File::open(percorso).map_err(errore_io)?;
    let mut byte = Vec::new();
    // Un byte oltre il massimo basta a sapere che il piano non ci sta, senza
    // leggere un file arbitrariamente grande.
    let da_leggere = u64::try_from(massimo).unwrap_or(u64::MAX).saturating_add(1);
    file.take(da_leggere)
        .read_to_end(&mut byte)
        .map_err(errore_io)?;
    if byte.len() > massimo {
        return Err(PlenoraError::InvalidPlan(format!(
            "piano oltre max_plan_json_bytes {massimo}"
        )));
    }
    let testo = String::from_utf8(byte)
        .map_err(|_| PlenoraError::InvalidPlan("piano non UTF-8".to_owned()))?;
    Pipeline::from_json(&testo)
}

/// `data.describe`: descrive una tabella senza modificarla
/// (`plenora-data-description-v1`).
///
/// La tabella si legge intera (`plenora-io`, con il budget di default
/// `DEFAULT_MAX_GOVERNED_MEMORY_BYTES`) e se ne legge il contratto come lo
/// leggerebbe un piano: stessa normalizzazione, stesse regole sui metadati
/// (una contraddizione è un errore, mai una descrizione). Le colonne
/// geometriche riportano il blocco canonico `plenora.geometry.*` del
/// contratto, senza `crs_definition` (testo libero e lungo; l'identità del
/// CRS è `crs_id`).
///
/// # Errors
///
/// Quelli della lettura (con il contesto `input`, fase `read`) e del
/// contratto; `Cancelled`
/// o `Timeout` prima della lettura (fase `read`) o prima della descrizione
/// (fase `finalize`).
pub fn descrivi(ingresso: &Path, interruzione: &Interruzione) -> Result<Value> {
    interruzione
        .verifica("prima di leggere l'input")
        .map_err(|errore| errore.with_phase(ErrorPhase::Read))?;
    let tabella = leggi_tabella(ingresso, None, DEFAULT_MAX_GOVERNED_MEMORY_BYTES)
        .map_err(|errore| nel_contesto("input", errore).with_phase(ErrorPhase::Read))?;
    interruzione
        .verifica("prima di descrivere l'input")
        .map_err(|errore| errore.with_phase(ErrorPhase::Finalize))?;
    let normalizzato = normalizza_schema(&tabella.schema());
    let contratto = contract_from_arrow_schema(normalizzato, resolve_crs)
        .map_err(|errore| nel_contesto("input", errore))?;
    contratto
        .validate()
        .map_err(|errore| nel_contesto("input", errore))?;
    let canonico = arrow_schema_from_contract(&contratto)?;
    let attiva = contratto.active_geometry.and_then(|attiva| {
        contratto
            .geometries
            .iter()
            .find(|geometria| geometria.field_id == attiva)
            .map(|geometria| geometria.name.clone())
    });
    Ok(json!({
        "rows": tabella.num_rows(),
        "contract_version": versione_contratto(&canonico)?,
        "columns": colonne(&canonico)?,
        "active_geometry": attiva,
    }))
}

/// `data.validate`: valida un piano contro gli schemi dei suoi input letti
/// da file, senza eseguirlo (`plenora-data-plan-validation-result-v1`).
///
/// Il risultato elenca gli input, i passi e lo schema pubblicato di ogni
/// output, quello con cui `data.run` lo scriverebbe.
///
/// # Errors
///
/// Quelli di [`plenora_io::valida_da_file`].
pub fn valida(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
    interruzione: &Interruzione,
) -> Result<Value> {
    let validata = valida_da_file(piano, ingressi, interruzione)?;
    interruzione
        .verifica("prima di consegnare la validazione")
        .map_err(|errore| errore.with_phase(ErrorPhase::Finalize))?;
    let uscite = piano
        .outputs
        .iter()
        .map(|nome| {
            let schema = validata.schema_uscita(nome).ok_or_else(|| {
                PlenoraError::Internal(format!("output `{nome}` senza schema validato"))
            })?;
            Ok(json!({"name": nome, "columns": colonne(schema)?}))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "plan_format": FORMATO_PIANO,
        "plan_version": piano.version,
        "inputs": piano.inputs,
        "steps": piano
            .steps
            .iter()
            .map(|passo| json!({"out": passo.out, "op": passo.op}))
            .collect::<Vec<_>>(),
        "outputs": uscite,
    }))
}

/// `data.run`: esegue un piano da file a file
/// (`plenora-data-execution-result-v1`).
///
/// I dati escono nei file d'uscita; il documento dice per ogni output nome,
/// tipo di contenuto, righe e colonne (mai il percorso), e per ogni passo
/// operazione, righe e righe con una divisione per zero resa null.
///
/// # Errors
///
/// Quelli di [`plenora_io::esegui_da_file_interrompibile`]; dopo il primo
/// output scritto l'effetto è `partial`.
pub fn esegui(
    piano: &Pipeline,
    ingressi: &[FileIngresso],
    uscite: &[FileUscita],
    opzioni: &OpzioniScrittura,
    interruzione: &Interruzione,
) -> Result<Value> {
    let esito = esegui_da_file_interrompibile(piano, ingressi, uscite, opzioni, interruzione)?;
    Ok(json!({
        "outputs": esito
            .uscite
            .iter()
            .map(|uscita| json!({
                "name": uscita.nome,
                "content_type": tipo_di_contenuto(uscita.formato),
                "rows": uscita.righe,
                "columns": uscita.colonne,
            }))
            .collect::<Vec<_>>(),
        "steps": esito
            .report
            .passi
            .iter()
            .map(|passo| json!({
                "out": passo.out,
                "op": passo.op,
                "rows_in": passo.righe_in,
                "rows_out": passo.righe_out,
                "division_by_zero_rows": passo.righe_divisione_per_zero,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// Il tipo di contenuto pubblico di un formato di file.
#[must_use]
pub const fn tipo_di_contenuto(formato: Formato) -> &'static str {
    match formato {
        Formato::ArrowIpc => ARROW_FILE,
        Formato::ArrowIpcStream => ARROW_STREAM,
        Formato::Parquet => PARQUET,
    }
}

/// Antepone un contesto senza cambiare la categoria; per l'I/O il messaggio
/// pubblico resta il testo fisso dell'`ErrorKind` (mai il percorso).
fn nel_contesto(contesto: &str, errore: PlenoraError) -> PlenoraError {
    match errore {
        PlenoraError::Io(causa) => PlenoraError::io_con_contesto(contesto, causa),
        altro => altro.con_contesto(contesto),
    }
}

/// `plenora.contract.version` dello schema, se c'è.
fn versione_contratto(schema: &Schema) -> Result<Option<u32>> {
    schema
        .metadata()
        .get(PLENORA_CONTRACT_VERSION_KEY)
        .map(|testo| {
            testo.parse::<u32>().map_err(|_| {
                PlenoraError::Internal("versione del contratto non verificata".to_owned())
            })
        })
        .transpose()
}

/// Le colonne di uno schema: nome, tipo (senza metadati né fuso,
/// `descrivi_tipo`), nullabilità, identità del campo se c'è, e per una
/// geometria il blocco canonico `plenora.geometry.*` senza la definizione
/// del CRS.
fn colonne(schema: &Schema) -> Result<Vec<Value>> {
    schema
        .fields()
        .iter()
        .map(|campo| {
            let metadati = campo.metadata();
            let mut voce = serde_json::Map::new();
            voce.insert("name".to_owned(), json!(campo.name()));
            voce.insert("type".to_owned(), json!(descrivi_tipo(campo.data_type())));
            voce.insert("nullable".to_owned(), json!(campo.is_nullable()));
            if let Some(testo) = metadati.get(PLENORA_FIELD_ID_KEY) {
                let identita = testo.parse::<u32>().map_err(|_| {
                    PlenoraError::Internal(format!(
                        "colonna `{}`: identita' del campo non verificata",
                        campo.name()
                    ))
                })?;
                voce.insert("field_id".to_owned(), json!(identita));
            }
            let geometrica = metadati
                .get(GEOARROW_EXTENSION_KEY)
                .is_some_and(|estensione| estensione == GEOARROW_WKB_EXTENSION);
            if geometrica {
                let blocco: serde_json::Map<String, Value> = metadati
                    .iter()
                    .filter(|(chiave, _)| {
                        chiave.starts_with(PLENORA_GEOMETRY_NAMESPACE_PREFIX)
                            && chiave.as_str() != PLENORA_GEOMETRY_CRS_DEFINITION_KEY
                    })
                    .map(|(chiave, valore)| (chiave.clone(), json!(valore)))
                    .collect();
                voce.insert("geometry".to_owned(), Value::Object(blocco));
            }
            Ok(Value::Object(voce))
        })
        .collect()
}
