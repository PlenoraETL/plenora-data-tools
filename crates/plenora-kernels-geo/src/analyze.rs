//! Inferenza a secco dei `DataContract` per le 69 operazioni `geo.*`
//! (Architetture.md par. 4.3 e 6.1, ADR 5 — Fase 2A-2b).
//!
//! [`analyze_geo_contract`] e' l'`analyze_contract` del catalogo per la
//! famiglia geo: dato l'id dell'operazione, i contratti di input, la config
//! JSON del nodo e il CRS di piano, produce il contratto dell'arco in uscita
//! oppure fallisce **in validazione** (fail-closed), mai a runtime.
//!
//! # Scelta sul riuso delle config (documentata, richiesta del task)
//!
//! Le config legacy `TransformArrowSchema`/`PairArrowSchema` vivono in
//! `plenora-engine::geo_transport::transport` e sono legate al protocollo di
//! trasporto v3 (conteggi righe obbligatori, limiti di trasporto
//! `MAX_ROWS`/`MAX_PAIRS`, errori `ArrowTransportError`): spostarle in
//! `kernels-geo` trascinerebbe dettagli del trasporto nel livello di analisi
//! semantica. Si e' scelta la **duplicazione minimale**: struct serde nuove,
//! con gli stessi nomi di parametro e gli stessi domini di validazione di
//! `validate_parameters` (finitezza, segni, range `[0,1]`, vincoli incrociati
//! `start_ratio <= end_ratio`, 6 coefficienti affini). Gli enum semantici
//! ([`crate::spatial_join::JoinPredicate`], [`crate::topology::OverlayMode`])
//! sono gia' in `kernels-geo` e sono riusati; per `cap`/`policy` si definiscono
//! enum di config locali, lo stesso pattern seguito dall'engine.
//!
//! # Convenzioni di output (v1)
//!
//! - trasformazioni 1:1 (centroid, buffer, simplify, ...): schema invariato,
//!   geometria trasformata in place con lo stesso `FieldId`;
//! - misure/predicati: la geometria resta e si **aggiunge** una colonna
//!   (`Float64` aree/lunghezze/distanze, `Boolean` predicati, `UInt64`
//!   `vertex_count`/`count`, `Utf8` `to_wkt`); nome da `output_column` o
//!   default documentato (nome breve dell'op, `wkt`, `within`, `count`);
//! - `bounds_extractor`: quattro colonne `{geometria}_minx/miny/maxx/maxy`
//!   (convenzione del trasporto legacy);
//! - `explode`/`delaunay`/`split`: schema invariato piu' `__parent_index`
//!   (`UInt64`, non null); piu' righe per riga di input;
//! - `dissolve`/`line_builder`/`polygon_builder`/`polygonize`/`line_merge`:
//!   aggregazione a sole geometrie (le colonne attributo non sono propagate,
//!   come nel kernel legacy; il group-by resta a `table.aggregate`);
//! - `sjoin`: schema left + `__right_index` (`UInt64`, non null): join con
//!   lineage, gli attributi right si agganciano con `table.join` a valle;
//! - `nearest`: schema left + `__right_index` + `distance` (entrambe nullable:
//!   righe left senza match entro `max_distance` producono null);
//! - `overlay`: sola geometria + `__left_index`/`__right_index` nullable
//!   (convenzione del kernel legacy);
//! - `clip`/`intersection`/`union`/`difference`/`symmetric_difference`:
//!   schema left invariato, geometria sostituita in place (allineate alle
//!   righe left nel protocollo legacy);
//! - `within`/`count_points_in_polygons`: schema left + colonna scalare
//!   (`within` Boolean, `count` UInt64), allineate alle righe left;
//! - `reproject`: schema invariato, `GeometryColumnContract.crs` e metadato
//!   `geo.crs` aggiornati al target (unico step che modifica il CRS);
//! - `from_coords`: aggiunge la colonna geometria (nuovo `FieldId`,
//!   `nullable=false` da specifica: coordinate null sono errore a runtime,
//!   non geometria null);
//! - `from_wkt`: come `from_coords` (input non geometrico, nuovo `FieldId`),
//!   ma la colonna geometria e' **nullable** (celle WKT null o invalide con
//!   `on_error: null` producono geometria null); CRS da config `crs` o di
//!   piano, requisito `Known`;
//! - `geometry_accessors`: aggiunge fino a 6 colonne per riga
//!   (`geometry_type` Utf8, `num_geometries`/`num_interior_rings` UInt64,
//!   `start_point`/`end_point` Utf8, `is_closed` Boolean, tutte nullable),
//!   filtrabili con `fields` e prefissabili con `output_prefix`;
//! - `collect`: aggregazione a sole geometrie come `dissolve`, piu' le
//!   colonne chiave di `group_by` (copiate dallo schema di input; gli altri
//!   attributi non sono propagati);
//! - `line_locate_point`: aggiunge `fraction` Float64 nullable (punto da
//!   config `point_wkb`, stessa convenzione D16 di `other_wkb`);
//! - `geometry_diagnostics`: la colonna geometria e' **sostituita** dalle 10
//!   colonne diagnostiche [`DIAGNOSTIC_COLUMNS`] (il contratto diventa
//!   non-geografico), come nel kernel legacy.
//!
//! # Operand i binari "unari" (decisione v1)
//!
//! Il catalogo marca `Unary` predicati, distanze a due colonne e `split`
//! ("due colonne dello stesso input"), ma la v1 (D16) ammette **una sola**
//! colonna geometria per input: il secondo operando e' quindi fornito dalla
//! config come `other_wkb` (WKB hex), validato strutturalmente in analisi
//! con il validatore del kernel. La sua CRS e' assunta uguale a quella
//! dell'input (stesso requisito `SameProjected`/`Geographic` dell'op).
//! Punto aperto per 2A-3: input multi-geometria post-v1.
//!
//! # Verifiche fail-closed in analisi
//!
//! - op presente nel catalogo e di famiglia geo, arieta' rispettata;
//! - ogni input ha esattamente una colonna geometria attiva (v1), salvo
//!   `from_coords` che ne richiede zero;
//! - `crs_requirement` del descriptor verificato con
//!   [`plenora_core::crs::validate_requirement`] sui CRS dei contratti
//!   (per `reproject`: sorgente + target; per `from_coords`: CRS di output);
//! - config deserializzata con `deny_unknown_fields` e domini validati;
//! - `required_capabilities` non e' verificata qui: il descriptor la dichiara
//!   e il controllo sui backend compilati spetta al planner (par. 6.1,
//!   passo 5); l'analisi registra il requisito risolvendo l'op dal catalogo.
//!
//! # Risoluzione CRS in analisi
//!
//! Il CRS di piano (`plan_crs`) e' gia' risolto dal planner: una definizione
//! testualmente uguale in config (`target_crs`, `crs` di `from_coords`) lo
//! riusa senza chiamare il backend. Altrimenti si invoca `resolve_crs`, che
//! senza feature `proj-backend` fallisce chiuso (`CRS_BACKEND_UNAVAILABLE`),
//! come nel sorgente.
//!
//! # Proprieta' del contratto
//!
//! Le op 1:1 allineate alle righe preservano `sorted_by`/`row_count`;
//! `explode`/`delaunay`/`split` preservano `sorted_by` (espansione stabile)
//! ma eliminano `row_count`; join e aggregazioni eliminano entrambe
//! (declassamento obbligatorio, par. 4.3).

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Schema};
use plenora_core::catalog::{find_operation, Arity, CrsRequirement, Family, OperationDescriptor};
use plenora_core::contract::{
    ContractProperties, DataContract, FieldAllocator, GeometryColumnContract, GeometryDimensions,
};
use plenora_core::crs::{required_definition, validate_requirement, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::arrow_adapter::{
    geo_metadata_json, DEFAULT_GEOMETRY_COLUMN, GEO_METADATA_KEY, GEOARROW_EXTENSION_KEY,
    GEOARROW_WKB_EXTENSION,
};
use crate::spatial_join::JoinPredicate;
use crate::topology::OverlayMode;

/// Colonna con l'indice della riga madre nelle espansioni 1:N.
pub const PARENT_INDEX_COLUMN: &str = "__parent_index";
/// Lineage lato left per `overlay`.
pub const LEFT_INDEX_COLUMN: &str = "__left_index";
/// Lineage lato right per `sjoin`/`nearest`/`overlay`.
pub const RIGHT_INDEX_COLUMN: &str = "__right_index";
/// Colonna distanza per `nearest`.
pub const DISTANCE_COLUMN: &str = "distance";
/// Default della colonna Boolean di `within`.
pub const WITHIN_COLUMN: &str = "within";
/// Default della colonna conteggio di `count_points_in_polygons`.
pub const COUNT_COLUMN: &str = "count";
/// Colonna di classificazione dei pezzi di `polygonize`.
pub const CLASS_COLUMN: &str = "__class";
/// Default della colonna WKT di `to_wkt`.
pub const WKT_COLUMN: &str = "wkt";
/// Default della colonna X di `from_coords`.
pub const DEFAULT_X_COLUMN: &str = "x";
/// Default della colonna Y di `from_coords`.
pub const DEFAULT_Y_COLUMN: &str = "y";
/// Default della colonna frazione di `line_locate_point`.
pub const FRACTION_COLUMN: &str = "fraction";

/// Le 6 colonne di `geometry_accessors`, in ordine canonico di output
/// (indipendente dall'ordine di `fields` in config).
pub const ACCESSOR_COLUMNS: [(&str, DataType); 6] = [
    ("geometry_type", DataType::Utf8),
    ("num_geometries", DataType::UInt64),
    ("num_interior_rings", DataType::UInt64),
    ("start_point", DataType::Utf8),
    ("end_point", DataType::Utf8),
    ("is_closed", DataType::Boolean),
];

/// Le 10 colonne diagnostiche di `geometry_diagnostics`, nella posizione
/// della colonna geometria che sostituiscono (come nel kernel legacy).
pub const DIAGNOSTIC_COLUMNS: [(&str, DataType); 10] = [
    ("geometry_type", DataType::Utf8),
    ("coordinate_count", DataType::UInt64),
    ("is_empty", DataType::Boolean),
    ("is_finite", DataType::Boolean),
    ("is_valid", DataType::Boolean),
    ("validity_reason", DataType::Utf8),
    ("bounds_minx", DataType::Float64),
    ("bounds_miny", DataType::Float64),
    ("bounds_maxx", DataType::Float64),
    ("bounds_maxy", DataType::Float64),
];

// ---------------------------------------------------------------------------
// Config serde minimali (duplicazione documentata: stessi nomi e domini del
// protocollo legacy, senza i parametri di trasporto).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyConfig {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputColumnConfig {
    output_column: Option<String>,
}

/// Stile di cap per `buffer` (default round, come il kernel).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum BufferCapParam {
    Round,
    Flat,
    Square,
}

/// Politica di `simplify`: Douglas-Peucker (default) o topology-preserving.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SimplifyPolicyParam {
    DouglasPeucker,
    PreserveTopology,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BufferConfig {
    distance: f64,
    cap: Option<BufferCapParam>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimplifyConfig {
    tolerance: f64,
    policy: Option<SimplifyPolicyParam>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReprojectConfig {
    target_crs: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AffineTransformConfig {
    coefficients: Vec<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TranslateConfig {
    x_offset: f64,
    y_offset: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScaleConfig {
    x_factor: f64,
    y_factor: f64,
    x_origin: Option<f64>,
    y_origin: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RotateConfig {
    degrees: f64,
    x_origin: Option<f64>,
    y_origin: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConcaveHullConfig {
    concavity: f64,
    length_threshold: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DensifyConfig {
    max_segment_length: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapToGridConfig {
    grid_size: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineSubstringConfig {
    start_ratio: f64,
    end_ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineInterpolatePointConfig {
    ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanTopologyConfig {
    snap_tolerance: f64,
    remove_overlaps: Option<bool>,
    fill_gaps: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VoronoiConfig {
    max_points: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolygonizeConfig {
    node_input: Option<bool>,
    require_complete: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FromCoordsConfig {
    x_column: Option<String>,
    y_column: Option<String>,
    geometry_column: Option<String>,
    crs: Option<String>,
}

/// Secondo operando geometrico da config (v1, D16: una sola colonna
/// geometria per input): WKB codificato esadecimale, validato in analisi.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OtherWkbConfig {
    other_wkb: String,
    output_column: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitConfig {
    other_wkb: String,
    tolerance: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SJoinConfig {
    predicate: JoinPredicate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NearestConfig {
    max_distance: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayConfig {
    mode: OverlayMode,
}

/// `from_wkt`: colonna Utf8 con il testo WKT; la politica `on_error`
/// (default `null`) e' semantica di runtime, qui solo validata.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FromWktConfig {
    wkt_column: String,
    output_column: Option<String>,
    on_error: Option<crate::extensions::OnWktError>,
    crs: Option<String>,
}

/// Campo accessorio richiedibile in `geometry_accessors.fields`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum AccessorFieldParam {
    GeometryType,
    NumGeometries,
    NumInteriorRings,
    StartPoint,
    EndPoint,
    IsClosed,
}

impl AccessorFieldParam {
    /// Indice in [`ACCESSOR_COLUMNS`] (ordine canonico di output).
    fn column_index(self) -> usize {
        match self {
            AccessorFieldParam::GeometryType => 0,
            AccessorFieldParam::NumGeometries => 1,
            AccessorFieldParam::NumInteriorRings => 2,
            AccessorFieldParam::StartPoint => 3,
            AccessorFieldParam::EndPoint => 4,
            AccessorFieldParam::IsClosed => 5,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeometryAccessorsConfig {
    fields: Option<Vec<AccessorFieldParam>>,
    output_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectConfig {
    group_by: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineLocatePointConfig {
    point_wkb: String,
    output_column: Option<String>,
}

// ---------------------------------------------------------------------------
// Errori e validazioni di dominio (messaggi coerenti col protocollo legacy).
// ---------------------------------------------------------------------------

fn invalid_param(op: &str, name: &'static str, reason: &'static str) -> PlenoraError {
    PlenoraError::Contract(format!("{op}: parametro `{name}` non valido: {reason}"))
}

fn parse_config<T: serde::de::DeserializeOwned>(op: &str, config: &Value) -> Result<T> {
    serde_json::from_value(config.clone())
        .map_err(|error| PlenoraError::Contract(format!("{op}: config non valida: {error}")))
}

fn ensure_finite(op: &str, name: &'static str, value: f64) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_param(op, name, "deve essere finito"))
    }
}

fn ensure_non_negative(op: &str, name: &'static str, value: f64) -> Result<()> {
    ensure_finite(op, name, value)?;
    if value < 0.0 {
        return Err(invalid_param(op, name, "deve essere non negativo"));
    }
    Ok(())
}

fn ensure_positive(op: &str, name: &'static str, value: f64) -> Result<()> {
    ensure_finite(op, name, value)?;
    if value <= 0.0 {
        return Err(invalid_param(op, name, "deve essere maggiore di zero"));
    }
    Ok(())
}

fn ensure_ratio(op: &str, name: &'static str, value: f64) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid_param(
            op,
            name,
            "deve essere finito e compreso tra zero e uno",
        ));
    }
    Ok(())
}

fn ensure_name(name: &str) -> bool {
    !name.trim().is_empty()
}

/// Nome della colonna aggiunta: `output_column` da config o default
/// documentato; vuoto rifiutato.
fn output_name<'a>(op: &str, configured: Option<&'a str>, default: &'a str) -> Result<&'a str> {
    let name = configured.unwrap_or(default);
    if ensure_name(name) {
        Ok(name)
    } else {
        Err(invalid_param(op, "output_column", "non deve essere vuoto"))
    }
}

/// Id breve dell'operazione (senza namespace `geo.`): default dei nomi di
/// colonna per misure e predicati.
fn short_id(op: &str) -> &str {
    op.strip_prefix("geo.").unwrap_or(op)
}

/// Decodifica e valida strutturalmente un WKB esadecimale da config.
fn validate_wkb_hex(op: &str, name: &'static str, hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 || hex.is_empty() {
        return Err(invalid_param(op, name, "WKB esadecimale non valido"));
    }
    let bytes: std::result::Result<Vec<u8>, _> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16))
        .collect();
    let bytes = bytes.map_err(|_| invalid_param(op, name, "WKB esadecimale non valido"))?;
    crate::validate_wkb_contract(&bytes)?;
    Ok(bytes)
}

/// Decodifica e valida strutturalmente il WKB hex del secondo operando.
fn validate_other_wkb(op: &str, hex: &str) -> Result<()> {
    validate_wkb_hex(op, "other_wkb", hex).map(|_| ())
}

// ---------------------------------------------------------------------------
// Helper su contratti e schemi.
// ---------------------------------------------------------------------------

/// v1: esattamente una colonna geometria attiva per input (D16).
fn single_geometry<'a>(op: &str, input: &'a DataContract) -> Result<&'a GeometryColumnContract> {
    if input.geometries.len() != 1 {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input deve avere esattamente una colonna geometria attiva (v1), trovate {}",
            input.geometries.len()
        )));
    }
    Ok(&input.geometries[0])
}

fn output_fields(input: &DataContract) -> Vec<Field> {
    input
        .schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect()
}

fn ensure_name_free(op: &str, fields: &[Field], name: &str) -> Result<()> {
    if fields.iter().any(|field| field.name() == name) {
        return Err(PlenoraError::Schema(format!(
            "{op}: la colonna di output `{name}` esiste gia' nello schema"
        )));
    }
    Ok(())
}

fn rebuild(input: &DataContract, fields: Vec<Field>, properties: ContractProperties) -> Result<DataContract> {
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        input.geometries.clone(),
        input.active_geometry,
        properties,
    )
}

/// Copia del campo geometria con nullability aggiornata (per gli output a
/// sole geometrie, dove l'aggregazione puo' produrre null).
fn geometry_field(input: &DataContract, geometry: &GeometryColumnContract, nullable: bool) -> Field {
    let field = input
        .schema
        .field_with_name(&geometry.name)
        .expect("contratto validato: il campo geometria esiste");
    Field::new(geometry.name.clone(), DataType::Binary, nullable)
        .with_metadata(field.metadata().clone())
}

/// Nuovo campo geometria con metadati di estensione `geoarrow.wkb` + `geo.crs`.
fn new_geometry_field(name: &str, crs: &ResolvedCrs, nullable: bool) -> Result<Field> {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    metadata.insert(GEO_METADATA_KEY.to_owned(), geo_metadata_json(crs.definition())?);
    Ok(Field::new(name, DataType::Binary, nullable).with_metadata(metadata))
}

/// Aggiorna il metadato `geo.crs` del campo geometria (solo `reproject`).
fn set_geometry_crs(fields: &mut [Field], geometry: &GeometryColumnContract, crs: &ResolvedCrs) -> Result<()> {
    for field in fields.iter_mut() {
        if field.name() == &geometry.name {
            let mut metadata = field.metadata().clone();
            metadata.insert(GEO_METADATA_KEY.to_owned(), geo_metadata_json(crs.definition())?);
            *field = field.clone().with_metadata(metadata);
            return Ok(());
        }
    }
    Err(PlenoraError::Schema(format!(
        "colonna geometria `{}` assente dallo schema",
        geometry.name
    )))
}

/// Risoluzione CRS in analisi: riuso del CRS di piano se la definizione
/// coincide, altrimenti backend (fail-closed senza `proj-backend`).
fn resolve_definition(definition: &str, plan_crs: Option<&ResolvedCrs>) -> Result<ResolvedCrs> {
    required_definition(Some(definition), "crs")?;
    if let Some(plan) = plan_crs {
        if plan.definition() == definition {
            return Ok(plan.clone());
        }
    }
    resolve_crs_backend(definition)
}

#[cfg(feature = "proj-backend")]
fn resolve_crs_backend(definition: &str) -> Result<ResolvedCrs> {
    crate::crs::resolve_crs(definition, "crs").map_err(PlenoraError::from)
}

#[cfg(not(feature = "proj-backend"))]
fn resolve_crs_backend(definition: &str) -> Result<ResolvedCrs> {
    plenora_core::crs::resolve_crs(definition, "crs").map_err(PlenoraError::from)
}

// ---------------------------------------------------------------------------
// Inferenza per forma di risultato.
// ---------------------------------------------------------------------------

/// Aggiunge una colonna scalare in coda allo schema (geometria preservata).
fn analyze_add_column(
    op: &str,
    input: &DataContract,
    name: &str,
    data_type: DataType,
) -> Result<DataContract> {
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(Field::new(name, data_type, true));
    rebuild(input, fields, input.properties.clone())
}

/// Espansione 1:N (`explode`, `delaunay`, `split`): schema invariato piu'
/// `__parent_index`; `sorted_by` preservato (espansione stabile), `row_count`
/// eliminato.
fn analyze_expand(op: &str, input: &DataContract) -> Result<DataContract> {
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, PARENT_INDEX_COLUMN)?;
    fields.push(Field::new(PARENT_INDEX_COLUMN, DataType::UInt64, false));
    let mut properties = input.properties.clone();
    properties.row_count = None;
    rebuild(input, fields, properties)
}

/// Aggregazione a sole geometrie (`dissolve`, builder, `polygonize`,
/// `line_merge`, `overlay`): le colonne attributo non sono propagate; la
/// geometria aggregata e' nullable (input vuoto -> geometria null).
fn analyze_geometry_only(
    input: &DataContract,
    geometry: &GeometryColumnContract,
    extra: &[(String, DataType, bool)],
) -> Result<DataContract> {
    let mut fields = vec![geometry_field(input, geometry, true)];
    for (name, data_type, nullable) in extra {
        fields.push(Field::new(name.clone(), data_type.clone(), *nullable));
    }
    let active = input
        .active_geometry
        .filter(|active| *active == geometry.field_id);
    let aggregated = GeometryColumnContract {
        nullable: true,
        ..geometry.clone()
    };
    DataContract::new(
        Arc::new(Schema::new(fields)),
        vec![aggregated],
        active,
        ContractProperties::default(),
    )
}

/// `bounds_extractor`: quattro colonne `{geometria}_minx/miny/maxx/maxy`.
fn analyze_bounds(op: &str, input: &DataContract, geometry: &GeometryColumnContract) -> Result<DataContract> {
    let mut fields = output_fields(input);
    for suffix in ["minx", "miny", "maxx", "maxy"] {
        let name = format!("{}_{suffix}", geometry.name);
        ensure_name_free(op, &fields, &name)?;
        fields.push(Field::new(name, DataType::Float64, true));
    }
    rebuild(input, fields, input.properties.clone())
}

/// `geometry_diagnostics`: la colonna geometria e' sostituita dalle 10
/// colonne diagnostiche; il contratto diventa non-geografico.
fn analyze_diagnostics(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
) -> Result<DataContract> {
    let mut fields = output_fields(input);
    let position = fields
        .iter()
        .position(|field| field.name() == &geometry.name)
        .ok_or_else(|| {
            PlenoraError::Schema(format!(
                "colonna geometria `{}` assente dallo schema",
                geometry.name
            ))
        })?;
    for (name, _) in &DIAGNOSTIC_COLUMNS {
        if fields
            .iter()
            .enumerate()
            .any(|(index, field)| index != position && field.name() == name)
        {
            return Err(PlenoraError::Schema(format!(
                "{op}: la colonna diagnostica `{name}` esiste gia' nello schema"
            )));
        }
    }
    let diagnostic_fields: Vec<Field> = DIAGNOSTIC_COLUMNS
        .iter()
        .map(|(name, data_type)| Field::new(*name, data_type.clone(), true))
        .collect();
    fields
        .splice(position..position + 1, diagnostic_fields)
        .for_each(drop);
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        Vec::new(),
        None,
        input.properties.clone(),
    )
}

/// `reproject`: schema invariato, CRS del contratto e metadato `geo.crs`
/// aggiornati al target risolto.
fn analyze_reproject(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
) -> Result<DataContract> {
    let parsed: ReprojectConfig = parse_config(op, config)?;
    let target = resolve_definition(&parsed.target_crs, plan_crs)?;
    validate_requirement(CrsRequirement::Reprojection, &[&geometry.crs, &target])?;
    let mut fields = output_fields(input);
    set_geometry_crs(&mut fields, geometry, &target)?;
    let reprojected = GeometryColumnContract {
        crs: target,
        ..geometry.clone()
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![reprojected],
        input.active_geometry,
        input.properties.clone(),
    )
}

/// `from_coords`: nessuna geometria in input; due colonne numeriche
/// (`Float64`/`Int64`) producono la colonna geometria (nuovo `FieldId`,
/// `nullable=false`), CRS da config o di piano.
fn analyze_from_coords(
    op: &str,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    requirement: CrsRequirement,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: FromCoordsConfig = parse_config(op, config)?;
    if !input.geometries.is_empty() {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input ha gia' una colonna geometria"
        )));
    }
    let x_column = parsed.x_column.as_deref().unwrap_or(DEFAULT_X_COLUMN);
    let y_column = parsed.y_column.as_deref().unwrap_or(DEFAULT_Y_COLUMN);
    let name = parsed
        .geometry_column
        .as_deref()
        .unwrap_or(DEFAULT_GEOMETRY_COLUMN);
    for (param, column) in [("x_column", x_column), ("y_column", y_column)] {
        if !ensure_name(column) {
            return Err(invalid_param(op, param, "non deve essere vuoto"));
        }
        let field = input.schema.field_with_name(column).map_err(|_| {
            PlenoraError::Schema(format!("{op}: colonna `{column}` assente dallo schema"))
        })?;
        if !matches!(field.data_type(), DataType::Float64 | DataType::Int64) {
            return Err(PlenoraError::Schema(format!(
                "{op}: colonna `{column}` di tipo {}, attesa Float64 o Int64",
                field.data_type()
            )));
        }
    }
    if !ensure_name(name) {
        return Err(invalid_param(op, "geometry_column", "non deve essere vuoto"));
    }
    let crs = match &parsed.crs {
        Some(definition) => resolve_definition(definition, plan_crs)?,
        None => plan_crs.cloned().ok_or_else(|| {
            PlenoraError::Crs(format!(
                "{op}: CRS obbligatorio (config `crs` o CRS di piano)"
            ))
        })?,
    };
    validate_requirement(requirement, &[&crs])?;
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(new_geometry_field(name, &crs, false)?);
    let field_id = fields_allocator.alloc();
    let geometry = GeometryColumnContract {
        field_id,
        name: name.to_owned(),
        crs,
        dimensions: GeometryDimensions::Xy,
        nullable: false,
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![geometry],
        Some(field_id),
        input.properties.clone(),
    )
}

/// `from_wkt`: nessuna geometria in input; una colonna `Utf8` WKT produce la
/// colonna geometria (nuovo `FieldId`, **nullable**: celle null o invalide
/// con `on_error: null` danno geometria null). CRS da config `crs` o di
/// piano; requisito del catalogo (`Known`).
fn analyze_from_wkt(
    op: &str,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    requirement: CrsRequirement,
    fields_allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let parsed: FromWktConfig = parse_config(op, config)?;
    let _ = &parsed.on_error;
    if !input.geometries.is_empty() {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input ha gia' una colonna geometria"
        )));
    }
    if !ensure_name(&parsed.wkt_column) {
        return Err(invalid_param(op, "wkt_column", "non deve essere vuoto"));
    }
    let wkt_field = input
        .schema
        .field_with_name(&parsed.wkt_column)
        .map_err(|_| {
            PlenoraError::Schema(format!(
                "{op}: colonna `{}` assente dallo schema",
                parsed.wkt_column
            ))
        })?;
    if wkt_field.data_type() != &DataType::Utf8 {
        return Err(PlenoraError::Schema(format!(
            "{op}: colonna `{}` di tipo {}, attesa Utf8",
            parsed.wkt_column,
            wkt_field.data_type()
        )));
    }
    let name = parsed
        .output_column
        .as_deref()
        .unwrap_or(DEFAULT_GEOMETRY_COLUMN);
    if !ensure_name(name) {
        return Err(invalid_param(op, "output_column", "non deve essere vuoto"));
    }
    let crs = match &parsed.crs {
        Some(definition) => resolve_definition(definition, plan_crs)?,
        None => plan_crs.cloned().ok_or_else(|| {
            PlenoraError::Crs(format!("{op}: CRS obbligatorio (config `crs` o CRS di piano)"))
        })?,
    };
    validate_requirement(requirement, &[&crs])?;
    let mut fields = output_fields(input);
    ensure_name_free(op, &fields, name)?;
    fields.push(new_geometry_field(name, &crs, true)?);
    let field_id = fields_allocator.alloc();
    let geometry = GeometryColumnContract {
        field_id,
        name: name.to_owned(),
        crs,
        dimensions: GeometryDimensions::Xy,
        nullable: true,
    };
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        vec![geometry],
        Some(field_id),
        input.properties.clone(),
    )
}

/// `geometry_accessors`: aggiunge le colonne richieste (default tutte) con
/// prefisso opzionale; 1:1 sulle righe, proprieta' preservate.
fn analyze_geometry_accessors(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: GeometryAccessorsConfig = parse_config(op, config)?;
    let mut selected: Vec<usize> = match &parsed.fields {
        None => (0..ACCESSOR_COLUMNS.len()).collect(),
        Some(fields) => {
            if fields.is_empty() {
                return Err(invalid_param(op, "fields", "non deve essere vuoto"));
            }
            let mut indexes: Vec<usize> = fields.iter().map(|field| field.column_index()).collect();
            indexes.sort_unstable();
            indexes.dedup();
            if indexes.len() != fields.len() {
                return Err(invalid_param(op, "fields", "campi duplicati"));
            }
            indexes
        }
    };
    selected.sort_unstable();
    let prefix = parsed.output_prefix.as_deref().unwrap_or("");
    let mut fields = output_fields(input);
    for index in selected {
        let (name, data_type) = &ACCESSOR_COLUMNS[index];
        let name = format!("{prefix}{name}");
        if !ensure_name(&name) {
            return Err(invalid_param(op, "output_prefix", "produce un nome vuoto"));
        }
        ensure_name_free(op, &fields, &name)?;
        fields.push(Field::new(name, data_type.clone(), true));
    }
    rebuild(input, fields, input.properties.clone())
}

/// `collect`: aggregazione per gruppo a sole geometrie piu' le colonne
/// chiave; gli altri attributi non sono propagati (come `dissolve`).
fn analyze_collect(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: CollectConfig = parse_config(op, config)?;
    if parsed.group_by.is_empty() {
        return Err(invalid_param(op, "group_by", "non deve essere vuoto"));
    }
    let mut extra: Vec<(String, DataType, bool)> = Vec::with_capacity(parsed.group_by.len());
    for name in &parsed.group_by {
        if !ensure_name(name) {
            return Err(invalid_param(op, "group_by", "nomi colonna non vuoti"));
        }
        if name == &geometry.name {
            return Err(invalid_param(
                op,
                "group_by",
                "la colonna geometria non puo' essere chiave di gruppo",
            ));
        }
        let field = input.schema.field_with_name(name).map_err(|_| {
            PlenoraError::Schema(format!("{op}: colonna `{name}` assente dallo schema"))
        })?;
        if extra.iter().any(|(seen, _, _)| seen == name) {
            return Err(invalid_param(op, "group_by", "colonne duplicate"));
        }
        extra.push((
            name.clone(),
            field.data_type().clone(),
            field.is_nullable(),
        ));
    }
    analyze_geometry_only(input, geometry, &extra)
}

/// `line_locate_point`: punto da config (`point_wkb` hex, D16) validato
/// strutturalmente e per tipo (deve essere un Point); aggiunge `fraction`.
fn analyze_line_locate_point(
    op: &str,
    input: &DataContract,
    config: &Value,
) -> Result<DataContract> {
    let parsed: LineLocatePointConfig = parse_config(op, config)?;
    let bytes = validate_wkb_hex(op, "point_wkb", &parsed.point_wkb)?;
    let point = crate::geometry_from_wkb(&bytes)
        .map_err(|_| invalid_param(op, "point_wkb", "WKB non decodificabile"))?;
    if !matches!(point, geo::Geometry::Point(_)) {
        return Err(invalid_param(op, "point_wkb", "deve essere un Point"));
    }
    let name = output_name(op, parsed.output_column.as_deref(), FRACTION_COLUMN)?;
    analyze_add_column(op, input, name, DataType::Float64)
}

// ---------------------------------------------------------------------------
// Validazione parametri per gruppo di operazioni.
// ---------------------------------------------------------------------------

/// Trasformazioni 1:1 in place con parametri: domini identici a
/// `TransformArrowSchema::validate_parameters` (senza i limiti di trasporto).
fn validate_transform_params(op: &str, config: &Value) -> Result<()> {
    match op {
        "geo.centroid"
        | "geo.convex_hull"
        | "geo.envelope"
        | "geo.boundary"
        | "geo.point_on_surface"
        | "geo.make_valid" => {
            let _: EmptyConfig = parse_config(op, config)?;
        }
        "geo.buffer" => {
            let parsed: BufferConfig = parse_config(op, config)?;
            ensure_finite(op, "distance", parsed.distance)?;
            let _ = &parsed.cap;
        }
        "geo.simplify" => {
            let parsed: SimplifyConfig = parse_config(op, config)?;
            ensure_non_negative(op, "tolerance", parsed.tolerance)?;
            let _ = &parsed.policy;
        }
        "geo.affine_transform" => {
            let parsed: AffineTransformConfig = parse_config(op, config)?;
            if parsed.coefficients.len() != 6 {
                return Err(invalid_param(op, "coefficients", "devono essere esattamente 6 coefficienti"));
            }
            if parsed.coefficients.iter().any(|value| !value.is_finite()) {
                return Err(invalid_param(op, "coefficients", "devono essere finiti"));
            }
        }
        "geo.translate" => {
            let parsed: TranslateConfig = parse_config(op, config)?;
            ensure_finite(op, "x_offset", parsed.x_offset)?;
            ensure_finite(op, "y_offset", parsed.y_offset)?;
        }
        "geo.scale" => {
            let parsed: ScaleConfig = parse_config(op, config)?;
            ensure_finite(op, "x_factor", parsed.x_factor)?;
            ensure_finite(op, "y_factor", parsed.y_factor)?;
            for (name, value) in [("x_origin", parsed.x_origin), ("y_origin", parsed.y_origin)] {
                if let Some(value) = value {
                    ensure_finite(op, name, value)?;
                }
            }
        }
        "geo.rotate" => {
            let parsed: RotateConfig = parse_config(op, config)?;
            ensure_finite(op, "degrees", parsed.degrees)?;
            for (name, value) in [("x_origin", parsed.x_origin), ("y_origin", parsed.y_origin)] {
                if let Some(value) = value {
                    ensure_finite(op, name, value)?;
                }
            }
        }
        "geo.concave_hull" => {
            let parsed: ConcaveHullConfig = parse_config(op, config)?;
            ensure_positive(op, "concavity", parsed.concavity)?;
            if let Some(value) = parsed.length_threshold {
                ensure_non_negative(op, "length_threshold", value)?;
            }
        }
        "geo.densify" => {
            let parsed: DensifyConfig = parse_config(op, config)?;
            ensure_positive(op, "max_segment_length", parsed.max_segment_length)?;
        }
        "geo.snap_to_grid" => {
            let parsed: SnapToGridConfig = parse_config(op, config)?;
            ensure_positive(op, "grid_size", parsed.grid_size)?;
        }
        "geo.line_substring" => {
            let parsed: LineSubstringConfig = parse_config(op, config)?;
            ensure_ratio(op, "start_ratio", parsed.start_ratio)?;
            ensure_ratio(op, "end_ratio", parsed.end_ratio)?;
            if parsed.start_ratio > parsed.end_ratio {
                return Err(invalid_param(
                    op,
                    "start_ratio/end_ratio",
                    "start_ratio non puo superare end_ratio",
                ));
            }
        }
        "geo.line_interpolate_point" => {
            let parsed: LineInterpolatePointConfig = parse_config(op, config)?;
            ensure_ratio(op, "ratio", parsed.ratio)?;
        }
        _ => unreachable!("validate_transform_params: op non una trasformazione"),
    }
    Ok(())
}

/// Misure con colonna scalare aggiunta (`area`, `length`, ...).
fn analyze_measure(op: &str, input: &DataContract, config: &Value) -> Result<DataContract> {
    let parsed: OutputColumnConfig = parse_config(op, config)?;
    let name = output_name(op, parsed.output_column.as_deref(), short_id(op))?;
    analyze_add_column(op, input, name, DataType::Float64)
}

/// Predicati e distanze "unari" con secondo operando da config (`other_wkb`).
fn analyze_unary_pair(op: &str, input: &DataContract, config: &Value, data_type: DataType) -> Result<DataContract> {
    let parsed: OtherWkbConfig = parse_config(op, config)?;
    validate_other_wkb(op, &parsed.other_wkb)?;
    let name = output_name(op, parsed.output_column.as_deref(), short_id(op))?;
    analyze_add_column(op, input, name, data_type)
}

/// Inferenza per le operazioni unarie (tutto tranne `from_coords` e le
/// binarie, gestite altrove).
fn analyze_unary(
    descriptor: &OperationDescriptor,
    input: &DataContract,
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
) -> Result<DataContract> {
    let op = descriptor.id;
    let geometry = single_geometry(op, input)?;
    let requirement = descriptor.crs_requirement.ok_or_else(|| {
        PlenoraError::Contract(format!("{op}: crs_requirement assente nel catalogo"))
    })?;
    match op {
        // Trasformazioni 1:1 in place: schema e FieldId invariati.
        "geo.centroid" | "geo.convex_hull" | "geo.envelope" | "geo.boundary"
        | "geo.point_on_surface" | "geo.make_valid" | "geo.buffer" | "geo.simplify"
        | "geo.affine_transform" | "geo.translate" | "geo.scale" | "geo.rotate"
        | "geo.concave_hull" | "geo.densify" | "geo.snap_to_grid" | "geo.line_substring"
        | "geo.line_interpolate_point" => {
            validate_transform_params(op, config)?;
            validate_requirement(requirement, &[&geometry.crs])?;
            Ok(input.clone())
        }
        "geo.reproject" => analyze_reproject(op, input, geometry, config, plan_crs),
        "geo.area" | "geo.length" | "geo.perimeter" | "geo.geodesic_line_length"
        | "geo.geodesic_area" => {
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_measure(op, input, config)
        }
        "geo.vertex_count" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[&geometry.crs])?;
            let name = output_name(op, parsed.output_column.as_deref(), short_id(op))?;
            analyze_add_column(op, input, name, DataType::UInt64)
        }
        "geo.to_wkt" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[&geometry.crs])?;
            let name = output_name(op, parsed.output_column.as_deref(), WKT_COLUMN)?;
            analyze_add_column(op, input, name, DataType::Utf8)
        }
        "geo.bounds_extractor" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_bounds(op, input, geometry)
        }
        "geo.geometry_diagnostics" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_diagnostics(op, input, geometry)
        }
        "geo.explode" | "geo.delaunay" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_expand(op, input)
        }
        "geo.split" => {
            let parsed: SplitConfig = parse_config(op, config)?;
            validate_other_wkb(op, &parsed.other_wkb)?;
            if let Some(tolerance) = parsed.tolerance {
                ensure_non_negative(op, "tolerance", tolerance)?;
            }
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_expand(op, input)
        }
        "geo.voronoi" => {
            let parsed: VoronoiConfig = parse_config(op, config)?;
            if let Some(max_points) = parsed.max_points {
                if max_points < 2 {
                    return Err(invalid_param(op, "max_points", "deve essere almeno 2"));
                }
            }
            validate_requirement(requirement, &[&geometry.crs])?;
            Ok(input.clone())
        }
        "geo.clean_topology" => {
            let parsed: CleanTopologyConfig = parse_config(op, config)?;
            ensure_non_negative(op, "snap_tolerance", parsed.snap_tolerance)?;
            let _ = (&parsed.remove_overlaps, &parsed.fill_gaps);
            validate_requirement(requirement, &[&geometry.crs])?;
            Ok(input.clone())
        }
        "geo.dissolve" | "geo.line_builder" | "geo.polygon_builder" | "geo.line_merge" => {
            let _: EmptyConfig = parse_config(op, config)?;
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_geometry_only(input, geometry, &[])
        }
        "geo.collect" => {
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_collect(op, input, geometry, config)
        }
        "geo.geometry_accessors" => {
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_geometry_accessors(op, input, config)
        }
        "geo.line_locate_point" => {
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_line_locate_point(op, input, config)
        }
        "geo.polygonize" => {
            let parsed: PolygonizeConfig = parse_config(op, config)?;
            let _ = (&parsed.node_input, &parsed.require_complete);
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_geometry_only(input, geometry, &[(CLASS_COLUMN.to_owned(), DataType::Utf8, false)])
        }
        "geo.distance" | "geo.hausdorff_distance" | "geo.frechet_distance"
        | "geo.haversine_distance" | "geo.geodesic_distance" | "geo.bearing" => {
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_unary_pair(op, input, config, DataType::Float64)
        }
        _ if op.starts_with("geo.predicate_") => {
            validate_requirement(requirement, &[&geometry.crs])?;
            analyze_unary_pair(op, input, config, DataType::Boolean)
        }
        _ => Err(PlenoraError::Unsupported(format!(
            "{op}: analyze_contract non implementata"
        ))),
    }
}

/// Inferenza per le operazioni binarie ordinate (left, right).
fn analyze_binary(
    descriptor: &OperationDescriptor,
    inputs: &[DataContract],
    config: &Value,
) -> Result<DataContract> {
    let op = descriptor.id;
    let left = &inputs[0];
    let right = &inputs[1];
    let left_geometry = single_geometry(op, left)?;
    let right_geometry = single_geometry(op, right)?;
    let requirement = descriptor.crs_requirement.ok_or_else(|| {
        PlenoraError::Contract(format!("{op}: crs_requirement assente nel catalogo"))
    })?;
    validate_requirement(requirement, &[&left_geometry.crs, &right_geometry.crs])?;
    match op {
        // Schema left invariato, geometria sostituita in place; righe
        // allineate a left nel protocollo legacy: proprieta' preservate.
        "geo.clip" | "geo.intersection" | "geo.union" | "geo.difference"
        | "geo.symmetric_difference" => {
            let _: EmptyConfig = parse_config(op, config)?;
            Ok(left.clone())
        }
        "geo.within" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            let name = output_name(op, parsed.output_column.as_deref(), WITHIN_COLUMN)?;
            analyze_add_column(op, left, name, DataType::Boolean)
        }
        "geo.count_points_in_polygons" => {
            let parsed: OutputColumnConfig = parse_config(op, config)?;
            let name = output_name(op, parsed.output_column.as_deref(), COUNT_COLUMN)?;
            analyze_add_column(op, left, name, DataType::UInt64)
        }
        // Join con lineage: righe moltiplicate, proprieta' eliminate.
        "geo.sjoin" => {
            let parsed: SJoinConfig = parse_config(op, config)?;
            let _ = &parsed.predicate;
            let mut fields = output_fields(left);
            ensure_name_free(op, &fields, RIGHT_INDEX_COLUMN)?;
            fields.push(Field::new(RIGHT_INDEX_COLUMN, DataType::UInt64, false));
            rebuild(left, fields, ContractProperties::default())
        }
        "geo.nearest" => {
            let parsed: NearestConfig = parse_config(op, config)?;
            if let Some(max_distance) = parsed.max_distance {
                ensure_non_negative(op, "max_distance", max_distance)?;
            }
            let mut fields = output_fields(left);
            for (name, data_type) in [
                (RIGHT_INDEX_COLUMN, DataType::UInt64),
                (DISTANCE_COLUMN, DataType::Float64),
            ] {
                ensure_name_free(op, &fields, name)?;
                fields.push(Field::new(name, data_type, true));
            }
            rebuild(left, fields, ContractProperties::default())
        }
        "geo.overlay" => {
            let parsed: OverlayConfig = parse_config(op, config)?;
            let _ = &parsed.mode;
            analyze_geometry_only(
                left,
                left_geometry,
                &[
                    (LEFT_INDEX_COLUMN.to_owned(), DataType::UInt64, true),
                    (RIGHT_INDEX_COLUMN.to_owned(), DataType::UInt64, true),
                ],
            )
        }
        _ => Err(PlenoraError::Unsupported(format!(
            "{op}: analyze_contract non implementata"
        ))),
    }
}

/// `analyze_contract` del catalogo per le operazioni `geo.*`
/// (Architetture.md par. 4.3): inferenza a secco del contratto di output.
///
/// `plan_crs` e' il CRS di piano gia' risolto dal planner (usato da
/// `from_coords` e per il riuso in `reproject`); `fields` alloca i `FieldId`
/// delle nuove colonne geometriche nel namespace globale del grafo.
///
/// # Errors
///
/// Fallisce (fail-closed, in validazione) se: l'op non e' nel catalogo o non
/// e' geo; l'arieta' non e' rispettata; un input non ha esattamente una
/// colonna geometria attiva (v1); il `crs_requirement` non e' soddisfatto;
/// la config non supera deserializzazione stretta o domini dei parametri;
/// una colonna prodotta collide con una esistente; il CRS di output non e'
/// risolvibile.
pub fn analyze_geo_contract(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    plan_crs: Option<&ResolvedCrs>,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let descriptor = find_operation(op).ok_or_else(|| {
        PlenoraError::Unsupported(format!("operazione `{op}` assente dal catalogo"))
    })?;
    if descriptor.family != Family::Geo {
        return Err(PlenoraError::Unsupported(format!(
            "`{op}` non e' un'operazione geo"
        )));
    }
    let expected_arity = match descriptor.arity {
        Arity::Unary => 1,
        Arity::BinaryOrdered => 2,
        Arity::NAry => {
            return Err(PlenoraError::Unsupported(format!(
                "{op}: arieta' N-aria non supportata in v1"
            )))
        }
    };
    if inputs.len() != expected_arity {
        return Err(PlenoraError::Contract(format!(
            "{op}: attesi {expected_arity} input, ricevuti {}",
            inputs.len()
        )));
    }
    if descriptor.id == "geo.from_coords" {
        let requirement = descriptor.crs_requirement.ok_or_else(|| {
            PlenoraError::Contract(format!("{op}: crs_requirement assente nel catalogo"))
        })?;
        return analyze_from_coords(descriptor.id, &inputs[0], config, plan_crs, requirement, fields);
    }
    if descriptor.id == "geo.from_wkt" {
        let op = descriptor.id;
        let requirement = descriptor.crs_requirement.ok_or_else(|| {
            PlenoraError::Contract(format!("{op}: crs_requirement assente nel catalogo"))
        })?;
        return analyze_from_wkt(op, &inputs[0], config, plan_crs, requirement, fields);
    }
    match descriptor.arity {
        Arity::Unary => analyze_unary(descriptor, &inputs[0], config, plan_crs),
        Arity::BinaryOrdered => analyze_binary(descriptor, inputs, config),
        Arity::NAry => unreachable!("arieta' N-aria gia' rifiutata"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use geo::{Geometry, Point};
    use geozero::{CoordDimensions, ToWkb};
    use plenora_core::catalog::{CATALOG, CrsRequirement, Family};
    use plenora_core::contract::{ContractProperty, FieldId, PropertyConfidence, PropertyScope};
    use plenora_core::crs::CrsKind;
    use serde_json::{json, Value};

    use super::*;

    fn projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    fn other_projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:3857".to_owned(),
            json!({"type": "ProjectedCRS", "name": "WGS 84 / Pseudo-Mercator"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    fn geographic_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:4326".to_owned(),
            json!({"type": "GeographicCRS", "name": "WGS 84"}),
            CrsKind::Geographic,
            None,
        )
    }

    fn geometry_arrow_field() -> Field {
        let mut metadata = HashMap::new();
        metadata.insert(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        );
        metadata.insert(
            GEO_METADATA_KEY.to_owned(),
            geo_metadata_json("EPSG:32632").expect("geo metadata"),
        );
        Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(metadata)
    }

    fn geo_contract(crs: ResolvedCrs) -> DataContract {
        DataContract::new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("label", DataType::Utf8, true),
                geometry_arrow_field(),
            ])),
            vec![GeometryColumnContract {
                field_id: FieldId(2),
                name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
                crs,
                dimensions: GeometryDimensions::Xy,
                nullable: true,
            }],
            Some(FieldId(2)),
            ContractProperties::default(),
        )
        .expect("contratto geometrico valido")
    }

    fn tabular_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new(DEFAULT_X_COLUMN, DataType::Float64, true),
            Field::new(DEFAULT_Y_COLUMN, DataType::Float64, true),
        ])))
    }

    fn wkt_tabular_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new(WKT_COLUMN, DataType::Utf8, true),
        ])))
    }

    fn point_wkb_hex() -> String {
        let wkb = Geometry::Point(Point::new(1.0, 2.0))
            .to_wkb(CoordDimensions::xy())
            .expect("encode punto");
        wkb.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn other_wkb_config() -> Value {
        json!({ "other_wkb": point_wkb_hex() })
    }

    // -----------------------------------------------------------------------
    // Tabella dei 69 casi: config minima valida + contratto atteso per op.
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    enum Expect {
        /// Schema identico all'input (geometria in place, stesso FieldId).
        Unchanged,
        /// Colonne dell'input piu' queste in coda (nome, tipo, nullable).
        Appended(Vec<(&'static str, DataType, bool)>),
        /// Solo geometria (nullable) piu' eventuali colonne extra.
        GeometryOnly(Vec<(&'static str, DataType, bool)>),
        /// Le 10 colonne diagnostiche al posto della geometria.
        Diagnostics,
        /// Input tabellare + colonna geometria non-null con nuovo FieldId.
        FromCoords,
        /// Input tabellare WKT + colonna geometria nullable con nuovo FieldId.
        FromWkt,
        /// Schema invariato, CRS del contratto aggiornato al target.
        Reprojected,
    }

    struct Case {
        op: &'static str,
        config: Value,
        binary: bool,
        expected: Expect,
    }

    fn float_column(name: &'static str) -> (&'static str, DataType, bool) {
        (name, DataType::Float64, true)
    }

    fn cases() -> Vec<Case> {
        let unary = |op: &'static str, config: Value, expected: Expect| Case {
            op,
            config,
            binary: false,
            expected,
        };
        let binary = |op: &'static str, config: Value, expected: Expect| Case {
            op,
            config,
            binary: true,
            expected,
        };
        let unchanged = |op: &'static str, config: Value| unary(op, config, Expect::Unchanged);
        let float_measure = |op: &'static str| {
            unary(op, json!({}), Expect::Appended(vec![float_column(short_id(op))]))
        };
        let float_pair = |op: &'static str| {
            unary(
                op,
                other_wkb_config(),
                Expect::Appended(vec![float_column(short_id(op))]),
            )
        };
        vec![
            // --- Trasformazioni 1:1 in place (19) ---------------------------
            unchanged("geo.centroid", json!({})),
            unchanged("geo.convex_hull", json!({})),
            unchanged("geo.envelope", json!({})),
            unchanged("geo.boundary", json!({})),
            unchanged("geo.point_on_surface", json!({})),
            unchanged("geo.make_valid", json!({})),
            unchanged("geo.buffer", json!({"distance": 100.0})),
            unchanged("geo.simplify", json!({"tolerance": 0.5})),
            unchanged("geo.affine_transform", json!({"coefficients": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]})),
            unchanged("geo.translate", json!({"x_offset": 1.0, "y_offset": 2.0})),
            unchanged("geo.scale", json!({"x_factor": 2.0, "y_factor": 2.0})),
            unchanged("geo.rotate", json!({"degrees": 90.0})),
            unchanged("geo.concave_hull", json!({"concavity": 2.0})),
            unchanged("geo.densify", json!({"max_segment_length": 10.0})),
            unchanged("geo.snap_to_grid", json!({"grid_size": 1.0})),
            unchanged("geo.line_substring", json!({"start_ratio": 0.1, "end_ratio": 0.9})),
            unchanged("geo.line_interpolate_point", json!({"ratio": 0.5})),
            unchanged("geo.voronoi", json!({})),
            unchanged("geo.clean_topology", json!({"snap_tolerance": 0.01})),
            // --- Misure e rappresentazioni ----------------------------------
            float_measure("geo.area"),
            float_measure("geo.length"),
            float_measure("geo.perimeter"),
            float_measure("geo.geodesic_line_length"),
            float_measure("geo.geodesic_area"),
            unary(
                "geo.vertex_count",
                json!({}),
                Expect::Appended(vec![("vertex_count", DataType::UInt64, true)]),
            ),
            unary(
                "geo.to_wkt",
                json!({}),
                Expect::Appended(vec![("wkt", DataType::Utf8, true)]),
            ),
            unary(
                "geo.bounds_extractor",
                json!({}),
                Expect::Appended(vec![
                    float_column("geometry_minx"),
                    float_column("geometry_miny"),
                    float_column("geometry_maxx"),
                    float_column("geometry_maxy"),
                ]),
            ),
            unary("geo.geometry_diagnostics", json!({}), Expect::Diagnostics),
            unary(
                "geo.geometry_accessors",
                json!({}),
                Expect::Appended(vec![
                    ("geometry_type", DataType::Utf8, true),
                    ("num_geometries", DataType::UInt64, true),
                    ("num_interior_rings", DataType::UInt64, true),
                    ("start_point", DataType::Utf8, true),
                    ("end_point", DataType::Utf8, true),
                    ("is_closed", DataType::Boolean, true),
                ]),
            ),
            unary(
                "geo.line_locate_point",
                json!({"point_wkb": point_wkb_hex()}),
                Expect::Appended(vec![float_column(FRACTION_COLUMN)]),
            ),
            // --- Espansioni 1:N ---------------------------------------------
            unary(
                "geo.explode",
                json!({}),
                Expect::Appended(vec![(PARENT_INDEX_COLUMN, DataType::UInt64, false)]),
            ),
            unary(
                "geo.delaunay",
                json!({}),
                Expect::Appended(vec![(PARENT_INDEX_COLUMN, DataType::UInt64, false)]),
            ),
            unary(
                "geo.split",
                other_wkb_config(),
                Expect::Appended(vec![(PARENT_INDEX_COLUMN, DataType::UInt64, false)]),
            ),
            // --- Aggregazioni a sole geometrie ------------------------------
            unary("geo.dissolve", json!({}), Expect::GeometryOnly(vec![])),
            unary("geo.line_builder", json!({}), Expect::GeometryOnly(vec![])),
            unary("geo.polygon_builder", json!({}), Expect::GeometryOnly(vec![])),
            unary("geo.line_merge", json!({}), Expect::GeometryOnly(vec![])),
            unary(
                "geo.collect",
                json!({"group_by": ["id"]}),
                Expect::GeometryOnly(vec![("id", DataType::Int64, false)]),
            ),
            unary(
                "geo.polygonize",
                json!({}),
                Expect::GeometryOnly(vec![(CLASS_COLUMN, DataType::Utf8, false)]),
            ),
            // --- Costruzione e riproiezione ---------------------------------
            unary("geo.from_coords", json!({}), Expect::FromCoords),
            unary(
                "geo.from_wkt",
                json!({"wkt_column": "wkt"}),
                Expect::FromWkt,
            ),
            unary(
                "geo.reproject",
                json!({"target_crs": "EPSG:32632"}),
                Expect::Reprojected,
            ),
            // --- Distanze e predicati "unari" (other_wkb) -------------------
            float_pair("geo.distance"),
            float_pair("geo.hausdorff_distance"),
            float_pair("geo.frechet_distance"),
            float_pair("geo.haversine_distance"),
            float_pair("geo.geodesic_distance"),
            float_pair("geo.bearing"),
            unary(
                "geo.predicate_intersects",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_intersects", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_disjoint",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_disjoint", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_contains",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_contains", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_within",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_within", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_equals_topo",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_equals_topo", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_covers",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_covers", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_covered_by",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_covered_by", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_contains_properly",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_contains_properly", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_touches",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_touches", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_crosses",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_crosses", DataType::Boolean, true)]),
            ),
            unary(
                "geo.predicate_overlaps",
                other_wkb_config(),
                Expect::Appended(vec![("predicate_overlaps", DataType::Boolean, true)]),
            ),
            // --- Binarie -----------------------------------------------------
            binary("geo.clip", json!({}), Expect::Unchanged),
            binary("geo.intersection", json!({}), Expect::Unchanged),
            binary("geo.union", json!({}), Expect::Unchanged),
            binary("geo.difference", json!({}), Expect::Unchanged),
            binary("geo.symmetric_difference", json!({}), Expect::Unchanged),
            binary(
                "geo.within",
                json!({}),
                Expect::Appended(vec![(WITHIN_COLUMN, DataType::Boolean, true)]),
            ),
            binary(
                "geo.count_points_in_polygons",
                json!({}),
                Expect::Appended(vec![(COUNT_COLUMN, DataType::UInt64, true)]),
            ),
            binary(
                "geo.sjoin",
                json!({"predicate": "intersects"}),
                Expect::Appended(vec![(RIGHT_INDEX_COLUMN, DataType::UInt64, false)]),
            ),
            binary(
                "geo.nearest",
                json!({}),
                Expect::Appended(vec![
                    (RIGHT_INDEX_COLUMN, DataType::UInt64, true),
                    float_column(DISTANCE_COLUMN),
                ]),
            ),
            binary(
                "geo.overlay",
                json!({"mode": "intersection"}),
                Expect::GeometryOnly(vec![
                    (LEFT_INDEX_COLUMN, DataType::UInt64, true),
                    (RIGHT_INDEX_COLUMN, DataType::UInt64, true),
                ]),
            ),
        ]
    }

    // -----------------------------------------------------------------------
    // Harness di verifica del contratto atteso.
    // -----------------------------------------------------------------------

    fn input_crs_for(op: &str) -> ResolvedCrs {
        let descriptor = find_operation(op).expect("op in catalogo");
        match descriptor.crs_requirement {
            Some(CrsRequirement::Geographic) => geographic_crs(),
            _ if op == "geo.reproject" => geographic_crs(),
            _ => projected_crs(),
        }
    }

    /// Esegue l'analisi del caso e restituisce (output, left input, allocatore).
    fn run_case(case: &Case) -> (DataContract, DataContract, FieldAllocator) {
        let input = if case.op == "geo.from_coords" {
            tabular_contract()
        } else if case.op == "geo.from_wkt" {
            wkt_tabular_contract()
        } else {
            geo_contract(input_crs_for(case.op))
        };
        let mut inputs = vec![input.clone()];
        if case.binary {
            inputs.push(geo_contract(projected_crs()));
        }
        let plan_crs = projected_crs();
        let mut allocator = FieldAllocator::new(100);
        let output = analyze_geo_contract(case.op, &inputs, &case.config, Some(&plan_crs), &mut allocator)
            .unwrap_or_else(|error| panic!("{}: {error}", case.op));
        (output, input, allocator)
    }

    fn field_signature(field: &Field) -> (&str, DataType, bool) {
        (
            field.name().as_str(),
            field.data_type().clone(),
            field.is_nullable(),
        )
    }

    fn signatures(contract: &DataContract) -> Vec<(&str, DataType, bool)> {
        contract
            .schema
            .fields()
            .iter()
            .map(|field| field_signature(field))
            .collect()
    }

    fn assert_appended(output: &DataContract, input: &DataContract, extra: &[(&str, DataType, bool)]) {
        let output_signatures = signatures(output);
        let input_signatures = signatures(input);
        assert_eq!(
            output_signatures.len(),
            input_signatures.len() + extra.len(),
            "numero colonne"
        );
        assert_eq!(
            &output_signatures[..input_signatures.len()],
            input_signatures.as_slice(),
            "le colonne di input passano invariate"
        );
        assert_eq!(
            &output_signatures[input_signatures.len()..],
            extra,
            "colonne aggiunte"
        );
    }

    fn assert_geometry_preserved(output: &DataContract, input: &DataContract) {
        let input_geometry = &input.geometries[0];
        let output_geometry = output
            .active_geometry_column()
            .expect("geometria attiva in output");
        assert_eq!(output_geometry.field_id, input_geometry.field_id, "FieldId preservato");
        assert_eq!(output_geometry.name, input_geometry.name);
    }

    #[test]
    fn table_covers_all_and_only_the_69_catalog_geo_ops() {
        let catalog_ops: HashSet<&str> = CATALOG
            .iter()
            .filter(|op| op.family == Family::Geo)
            .map(|op| op.id)
            .collect();
        assert_eq!(catalog_ops.len(), 69);
        let case_ops: HashSet<&str> = cases().iter().map(|case| case.op).collect();
        assert_eq!(case_ops.len(), 69, "casi duplicati nella tabella");
        assert_eq!(catalog_ops, case_ops);
    }

    #[test]
    fn every_geo_op_produces_the_expected_contract() {
        for case in cases() {
            let (output, input, allocator) = run_case(&case);
            output
                .validate()
                .unwrap_or_else(|error| panic!("{}: contratto non valido: {error}", case.op));
            match &case.expected {
                Expect::Unchanged => {
                    assert_eq!(signatures(&output), signatures(&input), "{}: schema", case.op);
                    assert_geometry_preserved(&output, &input);
                    assert_eq!(
                        output.active_geometry_column().unwrap().crs.definition(),
                        input.geometries[0].crs.definition(),
                        "{}: CRS preservato",
                        case.op
                    );
                }
                Expect::Appended(extra) => {
                    assert_appended(&output, &input, extra);
                    assert_geometry_preserved(&output, &input);
                }
                Expect::GeometryOnly(extra) => {
                    let mut expected = vec![(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true)];
                    expected.extend(extra.iter().cloned());
                    assert_eq!(signatures(&output), expected, "{}: schema", case.op);
                    let geometry = output.active_geometry_column().expect("geometria in output");
                    assert_eq!(geometry.field_id, FieldId(2), "{}: FieldId preservato", case.op);
                    assert!(geometry.nullable, "{}: geometria aggregata nullable", case.op);
                }
                Expect::Diagnostics => {
                    assert!(output.geometries.is_empty(), "{}: niente geometria", case.op);
                    assert_eq!(output.active_geometry, None);
                    let expected: Vec<(&str, DataType, bool)> = [
                        vec![
                            ("id", DataType::Int64, false),
                            ("label", DataType::Utf8, true),
                        ],
                        DIAGNOSTIC_COLUMNS
                            .iter()
                            .map(|(name, data_type)| (*name, data_type.clone(), true))
                            .collect(),
                    ]
                    .concat();
                    assert_eq!(signatures(&output), expected, "{}: schema", case.op);
                }
                Expect::FromCoords => {
                    let mut expected = signatures(&input);
                    expected.push((DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false));
                    assert_eq!(signatures(&output), expected, "{}: schema", case.op);
                    let geometry = output.active_geometry_column().expect("geometria creata");
                    assert_eq!(geometry.field_id, FieldId(100), "{}: FieldId allocato", case.op);
                    assert!(!geometry.nullable, "{}: geometria non null", case.op);
                    assert_eq!(geometry.crs.definition(), "EPSG:32632");
                    assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
                    let field = output
                        .schema
                        .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                        .expect("campo geometria");
                    assert_eq!(
                        field.metadata().get(GEOARROW_EXTENSION_KEY).map(String::as_str),
                        Some(GEOARROW_WKB_EXTENSION)
                    );
                    let geo: Value = serde_json::from_str(
                        field.metadata().get(GEO_METADATA_KEY).expect("geo metadata"),
                    )
                    .expect("geo JSON");
                    assert_eq!(geo.get("crs").and_then(Value::as_str), Some("EPSG:32632"));
                }
                Expect::FromWkt => {
                    let mut expected = signatures(&input);
                    expected.push((DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true));
                    assert_eq!(signatures(&output), expected, "{}: schema", case.op);
                    let geometry = output.active_geometry_column().expect("geometria creata");
                    assert_eq!(geometry.field_id, FieldId(100), "{}: FieldId allocato", case.op);
                    assert!(geometry.nullable, "{}: geometria nullable", case.op);
                    assert_eq!(geometry.crs.definition(), "EPSG:32632");
                    assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
                    let field = output
                        .schema
                        .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                        .expect("campo geometria");
                    assert_eq!(
                        field.metadata().get(GEOARROW_EXTENSION_KEY).map(String::as_str),
                        Some(GEOARROW_WKB_EXTENSION)
                    );
                    let geo: Value = serde_json::from_str(
                        field.metadata().get(GEO_METADATA_KEY).expect("geo metadata"),
                    )
                    .expect("geo JSON");
                    assert_eq!(geo.get("crs").and_then(Value::as_str), Some("EPSG:32632"));
                }
                Expect::Reprojected => {
                    assert_eq!(signatures(&output), signatures(&input), "{}: schema", case.op);
                    let geometry = output.active_geometry_column().expect("geometria in output");
                    assert_eq!(geometry.field_id, FieldId(2), "{}: FieldId preservato", case.op);
                    assert_eq!(geometry.crs.definition(), "EPSG:32632", "{}: CRS target", case.op);
                    let field = output
                        .schema
                        .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                        .expect("campo geometria");
                    let geo: Value = serde_json::from_str(
                        field.metadata().get(GEO_METADATA_KEY).expect("geo metadata"),
                    )
                    .expect("geo JSON");
                    assert_eq!(
                        geo.get("crs").and_then(Value::as_str),
                        Some("EPSG:32632"),
                        "{}: metadato geo.crs aggiornato",
                        case.op
                    );
                }
            }
            // L'allocatore non viene consumato dalle op che non creano geometrie.
            if case.op != "geo.from_coords" && case.op != "geo.from_wkt" {
                assert_eq!(allocator.peek(), FieldId(100), "{}: allocatore intatto", case.op);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Verifiche fail-closed: input non geometrico, arieta', catalogo.
    // -----------------------------------------------------------------------

    #[test]
    fn every_geo_op_rejects_an_input_without_geometry() {
        for case in cases() {
            let mut allocator = FieldAllocator::new(0);
            let result = if case.op == "geo.from_coords" || case.op == "geo.from_wkt" {
                // from_coords e from_wkt sono le uniche che richiedono zero
                // geometrie: un input gia' geometrico deve fallire.
                analyze_geo_contract(
                    case.op,
                    &[geo_contract(projected_crs())],
                    &case.config,
                    None,
                    &mut allocator,
                )
            } else {
                let mut inputs = vec![tabular_contract()];
                if case.binary {
                    inputs.push(geo_contract(projected_crs()));
                }
                analyze_geo_contract(case.op, &inputs, &case.config, None, &mut allocator)
            };
            assert!(result.is_err(), "{}: input non geometrico accettato", case.op);
        }
    }

    #[test]
    fn binary_ops_reject_a_second_input_without_geometry() {
        for op in ["geo.sjoin", "geo.clip", "geo.overlay", "geo.nearest", "geo.within"] {
            let config = match op {
                "geo.sjoin" => json!({"predicate": "intersects"}),
                "geo.overlay" => json!({"mode": "union"}),
                _ => json!({}),
            };
            let inputs = [geo_contract(projected_crs()), tabular_contract()];
            let result = analyze_geo_contract(op, &inputs, &config, None, &mut FieldAllocator::new(0));
            assert!(result.is_err(), "{op}: secondo input non geometrico accettato");
        }
    }

    #[test]
    fn arity_is_enforced() {
        let one = [geo_contract(projected_crs())];
        let two = [geo_contract(projected_crs()), geo_contract(projected_crs())];
        assert!(analyze_geo_contract(
            "geo.buffer",
            &two,
            &json!({"distance": 1.0}),
            None,
            &mut FieldAllocator::new(0)
        )
        .is_err());
        assert!(analyze_geo_contract(
            "geo.sjoin",
            &one,
            &json!({"predicate": "intersects"}),
            None,
            &mut FieldAllocator::new(0)
        )
        .is_err());
    }

    #[test]
    fn unknown_or_non_geo_ops_are_unsupported() {
        let inputs = [geo_contract(projected_crs())];
        for op in ["geo.nope", "table.filter", "nonsense"] {
            let result = analyze_geo_contract(op, &inputs, &json!({}), None, &mut FieldAllocator::new(0));
            assert!(
                matches!(result, Err(PlenoraError::Unsupported(_))),
                "{op}: atteso Unsupported"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Copertura dei 5 CrsRequirement.
    // -----------------------------------------------------------------------

    fn analyze_one(op: &str, inputs: &[DataContract], config: &Value, plan_crs: Option<&ResolvedCrs>) -> Result<DataContract> {
        analyze_geo_contract(op, inputs, config, plan_crs, &mut FieldAllocator::new(0))
    }

    #[test]
    fn known_requirement_accepts_any_resolved_crs() {
        for op in ["geo.explode", "geo.to_wkt", "geo.vertex_count", "geo.geometry_diagnostics", "geo.make_valid"] {
            let inputs = [geo_contract(geographic_crs())];
            analyze_one(op, &inputs, &json!({}), None)
                .unwrap_or_else(|error| panic!("{op} su CRS geografico: {error}"));
        }
    }

    #[test]
    fn projected_requirement_rejects_geographic_input() {
        for op in ["geo.buffer", "geo.area", "geo.simplify", "geo.voronoi", "geo.dissolve"] {
            let config = match op {
                "geo.buffer" => json!({"distance": 1.0}),
                "geo.simplify" => json!({"tolerance": 1.0}),
                _ => json!({}),
            };
            let inputs = [geo_contract(geographic_crs())];
            let result = analyze_one(op, &inputs, &config, None);
            assert!(matches!(result, Err(PlenoraError::Crs(_))), "{op}: CRS geografico accettato");
        }
    }

    #[test]
    fn geographic_requirement_rejects_projected_input() {
        for op in ["geo.geodesic_area", "geo.geodesic_line_length"] {
            let inputs = [geo_contract(projected_crs())];
            let result = analyze_one(op, &inputs, &json!({}), None);
            assert!(matches!(result, Err(PlenoraError::Crs(_))), "{op}: CRS proiettato accettato");
        }
        // Anche le distanze geodetiche "unary" con other_wkb.
        let inputs = [geo_contract(projected_crs())];
        let result = analyze_one("geo.haversine_distance", &inputs, &other_wkb_config(), None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))));
    }

    #[test]
    fn same_projected_requires_same_projected_crs_on_both_inputs() {
        let config = json!({"predicate": "intersects"});
        let matching = [geo_contract(projected_crs()), geo_contract(projected_crs())];
        analyze_one("geo.sjoin", &matching, &config, None).expect("stesso CRS proiettato");

        let different = [geo_contract(projected_crs()), geo_contract(other_projected_crs())];
        let result = analyze_one("geo.sjoin", &different, &config, None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "CRS diversi accettati");

        let geographic_right = [geo_contract(projected_crs()), geo_contract(geographic_crs())];
        let result = analyze_one("geo.sjoin", &geographic_right, &config, None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "right geografico accettato");

        // Variante unaria (other_wkb): il CRS dell'input deve essere proiettato.
        let inputs = [geo_contract(geographic_crs())];
        let result = analyze_one("geo.distance", &inputs, &other_wkb_config(), None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))));
    }

    #[test]
    fn reprojection_uses_plan_crs_when_definitions_match() {
        let inputs = [geo_contract(geographic_crs())];
        let plan = projected_crs();
        let output = analyze_one(
            "geo.reproject",
            &inputs,
            &json!({"target_crs": "EPSG:32632"}),
            Some(&plan),
        )
        .expect("target = CRS di piano");
        assert_eq!(
            output.active_geometry_column().unwrap().crs.definition(),
            "EPSG:32632"
        );
        // Senza plan_crs ne' backend PROJ: fail-closed.
        #[cfg(not(feature = "proj-backend"))]
        {
            let result = analyze_one(
                "geo.reproject",
                &inputs,
                &json!({"target_crs": "EPSG:32632"}),
                None,
            );
            assert!(result.is_err(), "target non risolvibile senza piano/backend");
        }
    }

    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn reprojection_without_backend_fails_closed_on_new_definitions() {
        let inputs = [geo_contract(geographic_crs())];
        let plan = projected_crs();
        let result = analyze_one(
            "geo.reproject",
            &inputs,
            &json!({"target_crs": "EPSG:3857"}),
            Some(&plan),
        );
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "definizione non verificata accettata");
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn reprojection_with_backend_resolves_new_definitions() {
        let inputs = [geo_contract(geographic_crs())];
        let output = analyze_one(
            "geo.reproject",
            &inputs,
            &json!({"target_crs": "EPSG:3857"}),
            None,
        )
        .expect("risoluzione PROJ del target");
        assert_eq!(
            output.active_geometry_column().unwrap().crs.kind(),
            CrsKind::Projected
        );
    }

    #[test]
    fn from_coords_uses_config_or_plan_crs_and_validates_its_requirement() {
        // from_coords richiede un CRS proiettato (catalogo): il CRS di piano
        // geografico e' rifiutato.
        let inputs = [tabular_contract()];
        let geographic_plan = geographic_crs();
        let result = analyze_one("geo.from_coords", &inputs, &json!({}), Some(&geographic_plan));
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "CRS geografico accettato");

        // Senza config `crs` ne' CRS di piano: obbligatorio.
        let result = analyze_one("geo.from_coords", &inputs, &json!({}), None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))));

        // Config `crs` che coincide col piano: riuso senza backend.
        let plan = projected_crs();
        let output = analyze_one(
            "geo.from_coords",
            &inputs,
            &json!({"crs": "EPSG:32632"}),
            Some(&plan),
        )
        .expect("crs da config = piano");
        assert_eq!(
            output.active_geometry_column().unwrap().crs.definition(),
            "EPSG:32632"
        );
    }

    // -----------------------------------------------------------------------
    // Validazione config fail-closed.
    // -----------------------------------------------------------------------

    #[test]
    fn configs_are_strictly_validated() {
        let inputs = [geo_contract(projected_crs())];
        let bad_configs: [(&str, Value); 22] = [
            ("geo.buffer", json!({})),                                  // distance mancante
            ("geo.buffer", json!({"distance": 1.0, "bogus": 1})),       // campo sconosciuto
            ("geo.buffer", json!({"distance": "molto"})),               // tipo errato
            ("geo.simplify", json!({"tolerance": -1.0})),
            ("geo.affine_transform", json!({"coefficients": [1.0, 2.0]})),
            ("geo.translate", json!({"x_offset": 1.0})),                // y_offset mancante
            ("geo.concave_hull", json!({"concavity": 0.0})),
            ("geo.densify", json!({"max_segment_length": 0.0})),
            ("geo.snap_to_grid", json!({"grid_size": -1.0})),
            ("geo.line_substring", json!({"start_ratio": 0.9, "end_ratio": 0.1})),
            ("geo.line_interpolate_point", json!({"ratio": 1.5})),
            ("geo.clean_topology", json!({})),                          // snap_tolerance mancante
            ("geo.voronoi", json!({"max_points": 1})),
            ("geo.distance", json!({"other_wkb": "zz"})),               // hex non valido
            ("geo.geometry_accessors", json!({"fields": []})),          // selezione vuota
            ("geo.geometry_accessors", json!({"fields": ["geometry_type", "geometry_type"]})),
            ("geo.geometry_accessors", json!({"fields": ["bogus"]})),   // campo sconosciuto
            ("geo.collect", json!({"group_by": []})),                   // nessuna chiave
            ("geo.collect", json!({"group_by": ["assente"]})),          // chiave non in schema
            ("geo.collect", json!({"group_by": ["geometry"]})),         // geometria come chiave
            ("geo.line_locate_point", json!({})),                       // point_wkb mancante
            ("geo.line_locate_point", json!({"point_wkb": "zz"})),      // hex non valido
        ];
        for (op, config) in bad_configs {
            let result = analyze_one(op, &inputs, &config, None);
            assert!(result.is_err(), "{op} con config {config}: accettata");
        }

        // other_wkb esadecimale ma con byte residui dopo la geometria.
        let mut trailing = point_wkb_hex();
        trailing.push_str("00");
        let result = analyze_one("geo.distance", &inputs, &json!({"other_wkb": trailing}), None);
        assert!(result.is_err(), "WKB con byte residui accettato");

        // Config non oggetto.
        let result = analyze_one("geo.centroid", &inputs, &json!("centroid"), None);
        assert!(result.is_err());
    }

    #[test]
    fn binary_configs_are_strictly_validated() {
        let inputs = [geo_contract(projected_crs()), geo_contract(projected_crs())];
        let bad_configs: [(&str, Value); 5] = [
            ("geo.sjoin", json!({})),                          // predicate mancante
            ("geo.sjoin", json!({"predicate": "nope"})),       // predicato sconosciuto
            ("geo.overlay", json!({})),                        // mode mancante
            ("geo.overlay", json!({"mode": "intersection", "x": 1})),
            ("geo.nearest", json!({"max_distance": -1.0})),
        ];
        for (op, config) in bad_configs {
            let result = analyze_one(op, &inputs, &config, None);
            assert!(result.is_err(), "{op} con config {config}: accettata");
        }
    }

    #[test]
    fn from_coords_validates_coordinate_columns_and_output_name() {
        // Colonna x non numerica.
        let wrong_type = DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new(DEFAULT_X_COLUMN, DataType::Utf8, true),
            Field::new(DEFAULT_Y_COLUMN, DataType::Float64, true),
        ])));
        let plan = projected_crs();
        assert!(analyze_one("geo.from_coords", &[wrong_type], &json!({}), Some(&plan)).is_err());

        // Colonne coordinate assenti con i nomi di config.
        let inputs = [tabular_contract()];
        assert!(analyze_one(
            "geo.from_coords",
            &inputs,
            &json!({"x_column": "lon", "y_column": "lat"}),
            Some(&plan)
        )
        .is_err());

        // Nome geometria gia' presente.
        assert!(analyze_one(
            "geo.from_coords",
            &inputs,
            &json!({"geometry_column": "id"}),
            Some(&plan)
        )
        .is_err());

        // Nomi vuoti rifiutati.
        assert!(analyze_one(
            "geo.from_coords",
            &inputs,
            &json!({"x_column": "  "}),
            Some(&plan)
        )
        .is_err());
    }

    #[test]
    fn from_wkt_validates_column_output_name_and_crs() {
        let plan = projected_crs();
        let inputs = [wkt_tabular_contract()];

        // Colonna WKT assente dallo schema o di tipo non-Utf8.
        assert!(analyze_one("geo.from_wkt", &inputs, &json!({"wkt_column": "geom_text"}), Some(&plan)).is_err());
        let numeric = [tabular_contract()];
        assert!(analyze_one("geo.from_wkt", &numeric, &json!({"wkt_column": "x"}), Some(&plan)).is_err());

        // Nome di output di default e override; collisione con colonna esistente.
        let output = analyze_one(
            "geo.from_wkt",
            &inputs,
            &json!({"wkt_column": "wkt", "output_column": "geom", "on_error": "fail"}),
            Some(&plan),
        )
        .expect("override nome colonna");
        assert_eq!(output.geometries[0].name, "geom");
        assert!(analyze_one(
            "geo.from_wkt",
            &inputs,
            &json!({"wkt_column": "wkt", "output_column": "id"}),
            Some(&plan)
        )
        .is_err());
        assert!(analyze_one(
            "geo.from_wkt",
            &inputs,
            &json!({"wkt_column": "wkt", "on_error": "bogus"}),
            Some(&plan)
        )
        .is_err());

        // CRS obbligatorio: senza config `crs` ne' CRS di piano fallisce.
        let result = analyze_one("geo.from_wkt", &inputs, &json!({"wkt_column": "wkt"}), None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "CRS mancante accettato");
        // Config `crs` coincidente col piano: riuso senza backend.
        let output = analyze_one(
            "geo.from_wkt",
            &inputs,
            &json!({"wkt_column": "wkt", "crs": "EPSG:32632"}),
            Some(&plan),
        )
        .expect("crs da config = piano");
        assert_eq!(output.geometries[0].crs.definition(), "EPSG:32632");
        assert!(output.geometries[0].nullable, "geometria da WKT nullable");
    }

    #[test]
    fn geometry_accessors_supports_field_selection_prefixes_and_collision_checks() {
        let inputs = [geo_contract(projected_crs())];

        // Selezione con ordine libero: l'output segue l'ordine canonico.
        let output = analyze_one(
            "geo.geometry_accessors",
            &inputs,
            &json!({"fields": ["is_closed", "geometry_type"]}),
            None,
        )
        .expect("subset di campi");
        let names: Vec<&str> = output
            .schema
            .fields()
            .iter()
            .skip(3)
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["geometry_type", "is_closed"]);

        // Prefisso applicato a tutte le colonne.
        let output = analyze_one(
            "geo.geometry_accessors",
            &inputs,
            &json!({"output_prefix": "acc_"}),
            None,
        )
        .expect("prefisso");
        assert_eq!(
            output.schema.fields().last().expect("ultima colonna").name(),
            "acc_is_closed"
        );

        // Collisione con una colonna esistente: fail-closed.
        let with_accessor_column = DataContract::new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("geometry_type", DataType::Utf8, true),
                geometry_arrow_field(),
            ])),
            vec![GeometryColumnContract {
                field_id: FieldId(2),
                name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
                crs: projected_crs(),
                dimensions: GeometryDimensions::Xy,
                nullable: true,
            }],
            Some(FieldId(2)),
            ContractProperties::default(),
        )
        .expect("contratto valido");
        assert!(analyze_one("geo.geometry_accessors", &[with_accessor_column], &json!({}), None).is_err());

        // 1:1 sulle righe: proprieta' preservate.
        let inputs = [contract_with_properties()];
        let output = analyze_one("geo.geometry_accessors", &inputs, &json!({}), None)
            .expect("accessors su contratto con proprieta'");
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_some());
    }

    #[test]
    fn collect_outputs_group_keys_and_drops_properties() {
        let inputs = [contract_with_properties()];
        let output = analyze_one(
            "geo.collect",
            &inputs,
            &json!({"group_by": ["id", "label"]}),
            None,
        )
        .expect("collect con due chiavi");
        let expected: Vec<(&str, DataType, bool)> = vec![
            (DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true),
            ("id", DataType::Int64, false),
            ("label", DataType::Utf8, true),
        ];
        assert_eq!(signatures(&output), expected);
        assert!(output.geometries[0].nullable, "collezione nullable");
        assert!(output.properties.sorted_by.is_none(), "aggregazione: sorted_by declassato");
        assert!(output.properties.row_count.is_none(), "aggregazione: row_count declassato");

        // Chiavi duplicate rifiutate.
        assert!(analyze_one(
            "geo.collect",
            &inputs,
            &json!({"group_by": ["id", "id"]}),
            None
        )
        .is_err());
    }

    #[test]
    fn line_locate_point_requires_a_point_and_names_the_output() {
        let inputs = [contract_with_properties()];

        // point_wkb valido ma non Point: rifiutato in analisi.
        let line = Geometry::LineString(geo::LineString::new(vec![
            (0.0, 0.0).into(),
            (1.0, 1.0).into(),
        ]))
        .to_wkb(CoordDimensions::xy())
        .expect("encode linea");
        let line_hex: String = line.iter().map(|byte| format!("{byte:02x}")).collect();
        let result = analyze_one(
            "geo.line_locate_point",
            &inputs,
            &json!({"point_wkb": line_hex}),
            None,
        );
        assert!(matches!(result, Err(PlenoraError::Contract(_))), "LineString accettata");

        // Override del nome colonna; proprieta' preservate (1:1 streaming).
        let output = analyze_one(
            "geo.line_locate_point",
            &inputs,
            &json!({"point_wkb": point_wkb_hex(), "output_column": "frac"}),
            None,
        )
        .expect("override nome colonna");
        assert_eq!(
            output.schema.fields().last().expect("ultima colonna").name(),
            "frac"
        );
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_some());
    }

    #[test]
    fn added_columns_must_not_collide_with_existing_fields() {
        let inputs = [geo_contract(projected_crs())];
        // Nome di output esplicito che collide.
        let result = analyze_one("geo.area", &inputs, &json!({"output_column": "id"}), None);
        assert!(matches!(result, Err(PlenoraError::Schema(_))));

        // Default `wkt` che collide con una colonna esistente.
        let mut fields = vec![
            Field::new("id", DataType::Int64, false),
            Field::new(WKT_COLUMN, DataType::Utf8, true),
            geometry_arrow_field(),
        ];
        let with_wkt = DataContract::new(
            Arc::new(Schema::new(std::mem::take(&mut fields))),
            vec![GeometryColumnContract {
                field_id: FieldId(2),
                name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
                crs: projected_crs(),
                dimensions: GeometryDimensions::Xy,
                nullable: true,
            }],
            Some(FieldId(2)),
            ContractProperties::default(),
        )
        .expect("contratto valido");
        let result = analyze_one("geo.to_wkt", &[with_wkt], &json!({}), None);
        assert!(matches!(result, Err(PlenoraError::Schema(_))));

        // L'override del nome evita la collisione.
        let inputs = [geo_contract(projected_crs())];
        let output = analyze_one(
            "geo.to_wkt",
            &inputs,
            &json!({"output_column": "geom_wkt"}),
            None,
        )
        .expect("override del nome colonna");
        assert_eq!(
            output.schema.fields().last().expect("ultima colonna").name(),
            "geom_wkt"
        );
    }

    // -----------------------------------------------------------------------
    // Proprieta' del contratto, FieldId, capability.
    // -----------------------------------------------------------------------

    fn contract_with_properties() -> DataContract {
        let mut contract = geo_contract(projected_crs());
        contract.properties = ContractProperties {
            sorted_by: Some(ContractProperty::new(
                PropertyConfidence::Proven(vec![FieldId(0)]),
                PropertyScope::Stream,
            )),
            row_count: Some(ContractProperty::new(
                PropertyConfidence::Estimated(1_000),
                PropertyScope::Dataset,
            )),
        };
        contract
    }

    #[test]
    fn row_aligned_ops_preserve_properties() {
        for (op, config) in [
            ("geo.buffer", json!({"distance": 1.0})),
            ("geo.area", json!({})),
            ("geo.reproject", json!({"target_crs": "EPSG:32632"})),
            ("geo.clean_topology", json!({"snap_tolerance": 0.1})),
        ] {
            let inputs = [contract_with_properties()];
            let plan = projected_crs();
            let output = analyze_one(op, &inputs, &config, Some(&plan))
                .unwrap_or_else(|error| panic!("{op}: {error}"));
            assert!(output.properties.sorted_by.is_some(), "{op}: sorted_by perso");
            assert!(output.properties.row_count.is_some(), "{op}: row_count perso");
        }
    }

    #[test]
    fn expand_preserves_sort_but_drops_row_count() {
        let inputs = [contract_with_properties()];
        let output = analyze_one("geo.explode", &inputs, &json!({}), None).expect("explode");
        assert!(output.properties.sorted_by.is_some(), "espansione stabile preserva l'ordine");
        assert!(output.properties.row_count.is_none(), "righe in uscita non note a secco");
    }

    #[test]
    fn joins_and_aggregations_drop_properties() {
        let plan = projected_crs();
        let inputs = [contract_with_properties()];
        let output = analyze_one("geo.dissolve", &inputs, &json!({}), None).expect("dissolve");
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());

        let pair = [contract_with_properties(), geo_contract(projected_crs())];
        let output = analyze_one(
            "geo.sjoin",
            &pair,
            &json!({"predicate": "intersects"}),
            Some(&plan),
        )
        .expect("sjoin");
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());
    }

    #[test]
    fn from_coords_allocates_fresh_field_ids() {
        let inputs = [tabular_contract()];
        let plan = projected_crs();
        let mut allocator = FieldAllocator::new(41);
        let first = analyze_geo_contract(
            "geo.from_coords",
            &inputs,
            &json!({}),
            Some(&plan),
            &mut allocator,
        )
        .expect("prima from_coords");
        let second = analyze_geo_contract(
            "geo.from_coords",
            &inputs,
            &json!({"geometry_column": "geom2"}),
            Some(&plan),
            &mut allocator,
        )
        .expect("seconda from_coords");
        assert_eq!(first.geometries[0].field_id, FieldId(41));
        assert_eq!(second.geometries[0].field_id, FieldId(42));
        assert_eq!(allocator.peek(), FieldId(43));
    }

    #[test]
    fn capabilities_are_declared_in_catalog_not_verified_in_analysis() {
        // Le op backend-pending dichiarano la capability nel descriptor; la
        // verifica sui backend compilati e' del planner (par. 6.1 passo 5),
        // quindi l'analisi succeede anche senza backend.
        let expected: [(&str, &str); 4] = [
            ("geo.make_valid", "geos"),
            ("geo.reproject", "proj"),
            ("geo.polygonize", "geos"),
            ("geo.split", "geos"),
        ];
        for (op, capability) in expected {
            let descriptor = find_operation(op).expect("op in catalogo");
            assert!(
                descriptor.required_capabilities.contains(&capability),
                "{op}: capability {capability} non registrata"
            );
        }
        let inputs = [geo_contract(projected_crs())];
        analyze_one("geo.make_valid", &inputs, &json!({}), None).expect("make_valid senza geos");
        analyze_one("geo.polygonize", &inputs, &json!({}), None).expect("polygonize senza geos");
        analyze_one("geo.split", &inputs, &other_wkb_config(), None).expect("split senza geos");
    }
}
