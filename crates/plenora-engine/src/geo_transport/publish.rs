//! Livello comandi del trasporto geo: verifica semantica del CRS e
//! pubblicazione atomica dell'output.
//!
//! Port Fase 1 ("coesistenza") da `main.rs` di plenora-geo-tools-arrow:
//! stessa logica, stessi messaggi; gli errori `Box<dyn Error>` del sorgente
//! sono mappati su [`PlenoraError`] (`Contract` per le violazioni di
//! contratto, `Crs` per gli errori CRS, `Io` per gli errori I/O).
//!
//! Sequenza attesa dal chiamante (come nel sorgente): parse dello schema
//! JSON → controllo `schema_version` → `validate_parameters` →
//! `validate_transform_arrow_crs`/`validate_pair_arrow_crs` → esecuzione
//! (`transform_arrow`/`pair_arrow`) → `publish_atomic`.

use std::io::{BufWriter, Write};
use std::path::Path;

use plenora_core::catalog::{find_operation, CrsRequirement};
use plenora_core::crs::{required_definition, validate_requirement};
use plenora_core::PlenoraError;

#[cfg(not(feature = "proj-backend"))]
use plenora_core::crs::resolve_crs;
#[cfg(feature = "proj-backend")]
use plenora_kernels_geo::crs::resolve_crs;

use super::transport::{ArrowOperation, PairArrowSchema, TransformArrowSchema};

/// Verifica semantica del CRS per `transform_arrow` (righe 249-265 del
/// `main.rs` sorgente). Deve essere chiamata DOPO
/// [`TransformArrowSchema::validate_parameters`] e prima di toccare i dati.
///
/// Il requisito CRS dell'operazione e' risolto dal catalogo core tramite il
/// `catalog_name` legacy (gli alias `geo_*` -> `geo.*` sono gia' nel
/// catalogo). Un requisito `None` e' trattato come `CrsRequirement::Known`:
/// nel catalogo core tutte le operazioni geo hanno `Some(_)`, quindi il ramo
/// e' irraggiungibile e il comportamento resta identico al sorgente.
///
/// # Errors
/// Restituisce `PlenoraError::Crs` se il CRS manca, e' invalido, non e'
/// risolvibile senza backend PROJ o non soddisfa il requisito
/// dell'operazione; `PlenoraError::Contract` se l'operazione e' assente dal
/// catalogo.
pub fn validate_transform_arrow_crs(schema: &TransformArrowSchema) -> Result<(), PlenoraError> {
    let definition = required_definition(schema.crs.as_deref(), "crs")?;
    let crs = resolve_crs(definition, "crs")?;
    if schema.operation == ArrowOperation::Reproject {
        let target_definition = required_definition(schema.target_crs.as_deref(), "target_crs")?;
        let target = resolve_crs(target_definition, "target_crs")?;
        validate_requirement(CrsRequirement::Reprojection, &[&crs, &target])?;
        return Ok(());
    }
    let catalog_name = schema.operation.catalog_name();
    let descriptor = find_operation(catalog_name).ok_or_else(|| {
        PlenoraError::Contract(format!("operazione {catalog_name} assente dal catalogo"))
    })?;
    validate_requirement(
        descriptor.crs_requirement.unwrap_or(CrsRequirement::Known),
        &[&crs],
    )?;
    Ok(())
}

/// Verifica semantica del CRS per `pair_arrow` (righe 300-313 del `main.rs`
/// sorgente). Deve essere chiamata DOPO
/// [`PairArrowSchema::validate_parameters`] e prima di toccare i dati.
///
/// # Errors
/// Restituisce `PlenoraError::Crs` se uno dei CRS manca, e' invalido, non
/// e' risolvibile senza backend PROJ o non soddisfa il requisito
/// dell'operazione; `PlenoraError::Contract` se l'operazione e' assente dal
/// catalogo.
pub fn validate_pair_arrow_crs(schema: &PairArrowSchema) -> Result<(), PlenoraError> {
    let left_definition = required_definition(schema.left_crs.as_deref(), "left_crs")?;
    let right_definition = required_definition(schema.right_crs.as_deref(), "right_crs")?;
    let left_crs = resolve_crs(left_definition, "left_crs")?;
    let right_crs = resolve_crs(right_definition, "right_crs")?;
    let catalog_name = schema.operation.catalog_name();
    let descriptor = find_operation(catalog_name).ok_or_else(|| {
        PlenoraError::Contract(format!("operazione {catalog_name} assente dal catalogo"))
    })?;
    validate_requirement(
        descriptor.crs_requirement.unwrap_or(CrsRequirement::Known),
        &[&left_crs, &right_crs],
    )?;
    Ok(())
}

/// Pubblicazione atomica dell'output (righe 315-349 del `main.rs` sorgente).
///
/// Rifiuta un output esistente, scrive su un tempfile `.plenora-geo-*.partial`
/// nella directory di destinazione, flush + sync e `persist_noclobber`.
///
/// # Errors
/// Restituisce `PlenoraError::Contract` se l'output esiste gia' o la
/// directory di destinazione non esiste; `PlenoraError::Io` per i fallimenti
/// di scrittura, sync o persist; propaga l'errore della closure `write`.
pub fn publish_atomic<T>(
    output_path: &Path,
    write: impl FnOnce(&mut dyn Write) -> Result<T, PlenoraError>,
) -> Result<T, PlenoraError> {
    if output_path.exists() {
        return Err(PlenoraError::Contract(format!(
            "output gia' esistente: {}",
            output_path.display()
        )));
    }
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(PlenoraError::Contract(format!(
            "directory output inesistente: {}",
            parent.display()
        )));
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".plenora-geo-")
        .suffix(".partial")
        .tempfile_in(parent)?;
    let result = {
        let mut output_writer = BufWriter::with_capacity(1024 * 1024, temporary.as_file_mut());
        let result = write(&mut output_writer)?;
        output_writer.flush()?;
        result
    };
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(output_path)
        .map_err(|error| error.error)?;
    Ok(result)
}
