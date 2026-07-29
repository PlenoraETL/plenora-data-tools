//! Inferenza a secco dei `DataContract` per le 75 operazioni `geo.*`
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
//!   (`within` Boolean, `count` `UInt64`), allineate alle righe left;
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
//!   (`geometry_type` Utf8, `num_geometries`/`num_interior_rings` `UInt64`,
//!   `start_point`/`end_point` Utf8, `is_closed` Boolean, tutte nullable),
//!   filtrabili con `fields` e prefissabili con `output_prefix`;
//! - `collect`: aggregazione a sole geometrie come `dissolve`, piu' le
//!   colonne chiave di `group_by` (copiate dallo schema di input; gli altri
//!   attributi non sono propagati);
//! - `line_locate_point`: aggiunge `fraction` Float64 nullable (punto da
//!   config `point_wkb`, stessa convenzione D16 di `other_wkb`);
//! - `generate_grid` (v1.2, generativa): come `from_coords` richiede un input
//!   senza geometrie (l'input funge da trigger, le sue colonne non sono
//!   propagate: l'output e' una riga per cella). Schema nuovo: `geometry`
//!   (nuovo `FieldId`, non null), `cell_i`/`cell_j` `UInt64` non null, piu'
//!   `centroid_x`/`centroid_y` Float64 non null se `include_centroid`. CRS da
//!   config `crs` o di piano. Extent finito e non degenere, `cell_size > 0`,
//!   numero celle entro [`crate::extensions2::MAX_GRID_CELLS`]; il conteggio
//!   esatto e' esposto come `row_count` `Estimated` (la convenzione v1
//!   riserva `Proven` alle fonti dimostrabili dagli header);
//! - `subdivide` (v1.2): espansione 1:N come `explode` (`__parent_index`,
//!   `row_count` eliminato, `sorted_by` preservato); `max_vertices >= 4`;
//!   `output_column` rinomina la colonna geometria (stesso `FieldId`);
//! - `snap` (v1.2): 1:1 in place, schema invariato; `reference_wkb` (hex)
//!   validato strutturalmente E decodificato in analisi, `tolerance >= 0`;
//!   il riferimento e' assunto nello stesso CRS dell'input (D16), requisito
//!   `SameProjected` come le distanze "unarie";
//! - `geometry_diagnostics`: la colonna geometria e' **sostituita** dalle 10
//!   colonne diagnostiche [`DIAGNOSTIC_COLUMNS`] (il contratto diventa
//!   non-geografico), come nel kernel legacy.
//! - `coverage_validate`/`shared_paths` (v1.3, WholeToMany): consumano
//!   l'intera copertura (Blocking) e producono uno schema **nuovo** — una
//!   riga per issue/tratto condiviso: colonne diagnostiche non-null piu'
//!   geometria WKB non-null con **nuovo `FieldId`** e CRS dell'input
//!   (`SameProjected`: aree/lunghezze in unita' di mappa); le colonne
//!   attributo dell'input non sono propagate e le proprieta' sono azzerate.
//! - `cluster_dbscan` (v1.3): calcolo globale (Blocking) ma output allineato
//!   alle righe (OneToOne): aggiunge la colonna `cluster_id` `UInt64`
//!   **nullable** (noise → null), nome da `output_column`; `eps` finito e
//!   `> 0`, `min_points >= 1`; `Projected` (eps in unita' di mappa).
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
//! # Dimensionalita' (B1.3)
//!
//! Ogni kernel geo che consuma una geometria la decodifica in
//! `geo::Geometry<f64>` (XY): l'analisi rifiuta a compile-plan
//! ([`PlenoraError::Unsupported`], mai a meta' esecuzione) ogni contratto di
//! input con `dimensions != Xy` — Z/M dichiarate o `Unknown` (R3.4). I
//! produttori (`from_coords`, `from_wkt`, `generate_grid`) e gli output
//! ricodificati (`coverage_validate`, `shared_paths`) dichiarano `Xy`; i
//! metadati `geo` dei campi prodotti scrivono sempre la dimensionalita' del
//! contratto di output. Il trasporto byte-preserving delle dimensionalita'
//! estese resta affidato alle op tabellari (passthrough).
//!
//! # Encoding (B1.4)
//!
//! I writer dei metadati `geo` di output scrivono la chiave `encoding` solo
//! quando il contratto la dichiara (`Some`) e la omettono con `None`
//! (fingerprint e retrocompatibilita' invariati): un contratto con encoding
//! dichiarato che attraversa un kernel che riscrive il campo (`reproject`)
//! conserva la chiave nel metadato — coerenza contratto↔metadato. I
//! produttori e gli output ricodificati (WKB ISO XY) non dichiarano alcun
//! encoding. Nota sui type code: EWKB senza flag Z/M e senza SRID e'
//! byte-identico a WKB ISO, quindi un input `encoding: ewkb` puro-XY e'
//! indistinguibile da ISO e passa i gate come `xy` (comportamento
//! dichiarato); il flag SRID EWKB e' invece sempre rifiutato dal validatore
//! celle al gate di lettura dell'esecutore, per qualunque dimensionalita'
//! dichiarata.
//!
//! # Proprieta' del contratto
//!
//! Le op 1:1 allineate alle righe preservano `sorted_by`/`row_count`;
//! `explode`/`delaunay`/`split` preservano `sorted_by` (espansione stabile)
//! ma eliminano `row_count`; join e aggregazioni eliminano entrambe
//! (declassamento obbligatorio, par. 4.3).

mod config;
mod dispatch;
mod helpers;
mod measures;
mod producers;
mod quality;

use plenora_core::arrow::DataType;
use plenora_core::catalog::{find_operation, Arity, Family};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::crs::ResolvedCrs;
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use self::dispatch::{analyze_binary, analyze_unary};
use self::producers::{analyze_from_coords, analyze_from_wkt, analyze_generate_grid};

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
/// Colonna indice di colonna della cella di `generate_grid`.
pub const CELL_I_COLUMN: &str = "cell_i";
/// Colonna indice di riga della cella di `generate_grid`.
pub const CELL_J_COLUMN: &str = "cell_j";
/// Colonna X del centroide cella di `generate_grid` (`include_centroid`).
pub const CENTROID_X_COLUMN: &str = "centroid_x";
/// Colonna Y del centroide cella di `generate_grid` (`include_centroid`).
pub const CENTROID_Y_COLUMN: &str = "centroid_y";
/// Colonna tipo issue di `coverage_validate`.
pub const ISSUE_TYPE_COLUMN: &str = "issue_type";
/// Colonna primo indice di `coverage_validate`/`shared_paths`.
pub const INDEX_A_COLUMN: &str = "index_a";
/// Colonna secondo indice di `coverage_validate`/`shared_paths`.
pub const INDEX_B_COLUMN: &str = "index_b";
/// Colonna area dell'overlap di `coverage_validate`.
pub const ISSUE_AREA_COLUMN: &str = "area";
/// Colonna lunghezza condivisa di `shared_paths`.
pub const SHARED_LENGTH_COLUMN: &str = "shared_length";
/// Default della colonna etichetta di `cluster_dbscan`.
pub const CLUSTER_ID_COLUMN: &str = "cluster_id";

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
// Entry point del catalogo (`analyze_contract` delle operazioni `geo.*`).
// ---------------------------------------------------------------------------

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
/// colonna geometria attiva (v1); la geometria di input non e' `Xy` per un
/// kernel che la elabora (B1.3); il `crs_requirement` non e' soddisfatto;
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
        return Err(PlenoraError::InvalidPlan(format!(
            "{op}: attesi {expected_arity} input, ricevuti {}",
            inputs.len()
        )));
    }
    if descriptor.id == "geo.from_coords" {
        let requirement = descriptor.crs_requirement.ok_or_else(|| {
            PlenoraError::InvalidPlan(format!("{op}: crs_requirement assente nel catalogo"))
        })?;
        return analyze_from_coords(descriptor.id, &inputs[0], config, plan_crs, requirement, fields);
    }
    if descriptor.id == "geo.from_wkt" {
        let op = descriptor.id;
        let requirement = descriptor.crs_requirement.ok_or_else(|| {
            PlenoraError::InvalidPlan(format!("{op}: crs_requirement assente nel catalogo"))
        })?;
        return analyze_from_wkt(op, &inputs[0], config, plan_crs, requirement, fields);
    }
    if descriptor.id == "geo.generate_grid" {
        let op = descriptor.id;
        let requirement = descriptor.crs_requirement.ok_or_else(|| {
            PlenoraError::InvalidPlan(format!("{op}: crs_requirement assente nel catalogo"))
        })?;
        return analyze_generate_grid(op, &inputs[0], config, plan_crs, requirement, fields);
    }
    match descriptor.arity {
        Arity::Unary => analyze_unary(descriptor, &inputs[0], config, plan_crs, fields),
        Arity::BinaryOrdered => analyze_binary(descriptor, inputs, config),
        Arity::NAry => Err(PlenoraError::Unsupported(format!(
            "{op}: arieta' N-aria non supportata in v1"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fmt::Write as _;
    use std::sync::Arc;

    use geo::{Geometry, Point};
    use geozero::{CoordDimensions, ToWkb};
    use plenora_core::arrow::{DataType, Field, Schema};
    use plenora_core::catalog::{find_operation, CATALOG, CrsRequirement, Family};
    use plenora_core::contract::{
        ContractProperties, ContractProperty, DataContract, FieldAllocator, FieldId,
        GeometryColumnContract, GeometryDimensions, GeometryEncoding, PropertyConfidence,
        PropertyScope,
    };
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use plenora_core::{PlenoraError, Result};
    use serde_json::{json, Value};

    use super::helpers::short_id;
    use super::*;
    use crate::arrow_adapter::{
        geo_metadata_json_with_dimensions, DEFAULT_GEOMETRY_COLUMN, GEO_METADATA_KEY,
        GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    };

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
            geo_metadata_json_with_dimensions("EPSG:32632", GeometryDimensions::Xy)
                .expect("geo metadata"),
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
                encoding: None,
                nullable: true,
                types: GeometryColumnContract::undeclared_types(),
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
        // `write!` su `String` e' infallibile: l'`fmt::Result` non puo' essere Err.
        wkb.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
    }

    fn other_wkb_config() -> Value {
        json!({ "other_wkb": point_wkb_hex() })
    }

    // -----------------------------------------------------------------------
    // Tabella dei 69 casi: config minima valida + contratto atteso per op.
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    enum Expect {
        /// Schema identico all'input (geometria in place, stesso `FieldId`).
        Unchanged,
        /// Colonne dell'input piu' queste in coda (nome, tipo, nullable).
        Appended(Vec<(&'static str, DataType, bool)>),
        /// Solo geometria (nullable) piu' eventuali colonne extra.
        GeometryOnly(Vec<(&'static str, DataType, bool)>),
        /// Le 10 colonne diagnostiche al posto della geometria.
        Diagnostics,
        /// Input tabellare + colonna geometria non-null con nuovo `FieldId`.
        FromCoords,
        /// Input tabellare WKT + colonna geometria nullable con nuovo `FieldId`.
        FromWkt,
        /// Griglia generativa: schema nuovo (geometria non null nuovo `FieldId`,
        /// `cell_i/cell_j`, centroidi opzionali) + `row_count` esatto.
        Grid { centroid: bool },
        /// Op di copertura v1.3 (WholeToMany): schema nuovo completo (tutto
        /// non null), geometria con nuovo `FieldId` e CRS dell'input.
        CoverageRows(Vec<(&'static str, DataType, bool)>),
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

    // Tabella di fixture: la lunghezza e' data dall'elenco dei 69 casi
    // (config + contratto atteso per op), non da logica da spezzare.
    #[allow(clippy::too_many_lines)]
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
            unary(
                "geo.snap",
                json!({"reference_wkb": point_wkb_hex(), "tolerance": 0.5}),
                Expect::Unchanged,
            ),
            // --- Coperture v1.3 (WholeToMany, schema nuovo) ------------------
            unary(
                "geo.coverage_validate",
                json!({}),
                Expect::CoverageRows(vec![
                    (ISSUE_TYPE_COLUMN, DataType::Utf8, false),
                    (INDEX_A_COLUMN, DataType::UInt64, false),
                    (INDEX_B_COLUMN, DataType::UInt64, false),
                    (ISSUE_AREA_COLUMN, DataType::Float64, false),
                    (DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false),
                ]),
            ),
            unary(
                "geo.shared_paths",
                json!({}),
                Expect::CoverageRows(vec![
                    (INDEX_A_COLUMN, DataType::UInt64, false),
                    (INDEX_B_COLUMN, DataType::UInt64, false),
                    (SHARED_LENGTH_COLUMN, DataType::Float64, false),
                    (DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false),
                ]),
            ),
            // --- Clustering v1.3 (Blocking, output allineato alle righe) -----
            unary(
                "geo.cluster_dbscan",
                json!({"eps": 10.0, "min_points": 3}),
                Expect::Appended(vec![(CLUSTER_ID_COLUMN, DataType::UInt64, true)]),
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
            unary(
                "geo.subdivide",
                json!({"max_vertices": 8}),
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
                "geo.generate_grid",
                json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0}, "cell_size": 5.0}),
                Expect::Grid { centroid: false },
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
        } else if case.op == "geo.generate_grid" {
            tabular_contract()
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
    fn table_covers_all_and_only_the_75_catalog_geo_ops() {
        let catalog_ops: HashSet<&str> = CATALOG
            .iter()
            .filter(|op| op.family == Family::Geo)
            .map(|op| op.id)
            .collect();
        assert_eq!(catalog_ops.len(), 75);
        let case_ops: HashSet<&str> = cases().iter().map(|case| case.op).collect();
        assert_eq!(case_ops.len(), 75, "casi duplicati nella tabella");
        assert_eq!(catalog_ops, case_ops);
    }

    /// Come `geo_contract`, con la dimensionalita' dichiarata (fixture B1.3).
    fn geo_contract_with_dimensions(
        crs: ResolvedCrs,
        dimensions: GeometryDimensions,
    ) -> DataContract {
        let mut contract = geo_contract(crs);
        contract.geometries[0].dimensions = dimensions;
        contract
    }

    #[test]
    fn dimensions_propagation_table_for_all_catalog_geo_ops() {
        // (a) B1.3: per OGNI op geo del catalogo, un input con dimensionalita'
        // estesa o non risolta -> rifiuto esplicito a compile-plan (kernel
        // elaboranti) oppure Xy dichiarato dal produttore; MAI un xy
        // silenzioso. La tabella `cases()` copre tutte e sole le 75 op.
        const PRODUCERS: [&str; 3] = ["geo.from_coords", "geo.from_wkt", "geo.generate_grid"];
        for case in cases() {
            if PRODUCERS.contains(&case.op) {
                // Produttori: input non geometrico -> il contratto dichiara
                // Xy e il metadato del campo scrive la stessa dimensionalita'.
                let (output, _, _) = run_case(&case);
                let geometry = output
                    .active_geometry_column()
                    .unwrap_or_else(|| panic!("{}: geometria prodotta", case.op));
                assert_eq!(
                    geometry.dimensions,
                    GeometryDimensions::Xy,
                    "{}: il produttore dichiara Xy",
                    case.op
                );
                let field = output
                    .schema
                    .field_with_name(&geometry.name)
                    .unwrap_or_else(|_| panic!("{}: campo geometria", case.op));
                assert_eq!(
                    crate::arrow_adapter::geometry_dimensions_from_metadata(field),
                    GeometryDimensions::Xy,
                    "{}: metadato output coerente col contratto",
                    case.op
                );
                // B1.4: il produttore ricodifica WKB ISO XY — nessun encoding
                // dichiarato, chiave omessa dal metadato.
                assert_eq!(
                    crate::arrow_adapter::geometry_encoding_from_metadata(field),
                    None,
                    "{}: nessun encoding dichiarato dal produttore",
                    case.op
                );
                continue;
            }
            for dimensions in [
                GeometryDimensions::Xyz,
                GeometryDimensions::Xym,
                GeometryDimensions::Xyzm,
                GeometryDimensions::Unknown,
            ] {
                let mut inputs =
                    vec![geo_contract_with_dimensions(input_crs_for(case.op), dimensions)];
                if case.binary {
                    inputs.push(geo_contract_with_dimensions(projected_crs(), dimensions));
                }
                let mut allocator = FieldAllocator::new(100);
                let result = analyze_geo_contract(
                    case.op,
                    &inputs,
                    &case.config,
                    Some(&projected_crs()),
                    &mut allocator,
                );
                match result {
                    Err(PlenoraError::Unsupported(message)) => {
                        assert!(
                            message.contains(case.op),
                            "{}: l'errore cita l'operazione: {message}",
                            case.op
                        );
                        assert!(
                            message.contains(dimensions.as_str()),
                            "{}: l'errore cita la dimensionalita': {message}",
                            case.op
                        );
                    }
                    Err(other) => {
                        panic!("{}: atteso Unsupported con {dimensions}, trovato {other:?}", case.op)
                    }
                    Ok(_) => panic!(
                        "{}: input {dimensions} accettato: xy silenzioso (B1.3 violata)",
                        case.op
                    ),
                }
            }
        }
    }

    #[test]
    // Verifica sequenziale per variante di `Expect` su tutti i casi: la
    // lunghezza e' intrinseca alla tabella dei contratti attesi.
    #[allow(clippy::too_many_lines)]
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
                Expect::Grid { centroid } => {
                    let mut expected = vec![
                        (DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false),
                        (CELL_I_COLUMN, DataType::UInt64, false),
                        (CELL_J_COLUMN, DataType::UInt64, false),
                    ];
                    if *centroid {
                        expected.push((CENTROID_X_COLUMN, DataType::Float64, false));
                        expected.push((CENTROID_Y_COLUMN, DataType::Float64, false));
                    }
                    assert_eq!(signatures(&output), expected, "{}: schema", case.op);
                    let geometry = output.active_geometry_column().expect("geometria creata");
                    assert_eq!(geometry.field_id, FieldId(100), "{}: FieldId allocato", case.op);
                    assert!(!geometry.nullable, "{}: geometria di griglia non null", case.op);
                    assert_eq!(geometry.crs.definition(), "EPSG:32632");
                    assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
                    // Il numero di celle (2x2 con cell_size 5 su extent 10x10)
                    // e' noto a secco.
                    let row_count = output
                        .properties
                        .row_count
                        .as_ref()
                        .expect("row_count della griglia");
                    assert_eq!(row_count.value(), Some(&4), "{}: conteggio celle", case.op);
                }
                Expect::CoverageRows(expected) => {
                    assert_eq!(signatures(&output), *expected, "{}: schema", case.op);
                    let geometry = output.active_geometry_column().expect("geometria creata");
                    assert_eq!(geometry.field_id, FieldId(100), "{}: FieldId allocato", case.op);
                    assert!(!geometry.nullable, "{}: geometria non null", case.op);
                    assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
                    assert_eq!(
                        geometry.crs.definition(),
                        input.geometries[0].crs.definition(),
                        "{}: CRS dell'input",
                        case.op
                    );
                    assert!(output.properties.sorted_by.is_none(), "{}: proprieta' azzerate", case.op);
                    assert!(output.properties.row_count.is_none(), "{}: proprieta' azzerate", case.op);
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
            if !matches!(
                case.op,
                "geo.from_coords"
                    | "geo.from_wkt"
                    | "geo.generate_grid"
                    | "geo.coverage_validate"
                    | "geo.shared_paths"
            ) {
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
            let result = if matches!(case.op, "geo.from_coords" | "geo.from_wkt" | "geo.generate_grid") {
                // from_coords, from_wkt e generate_grid richiedono zero
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

    #[test]
    fn reprojection_preserves_a_declared_encoding_in_the_rewritten_metadata() {
        // B1.4: un contratto con encoding dichiarato (EWKB puro-XY: type
        // code byte-identici a ISO, quindi ammesso dal gate `xy`) che
        // attraversa `reproject` conserva la chiave `encoding` nel metadato
        // riscritto — coerenza contratto↔metadato. Prima di B1.4 il writer
        // riscriveva solo `dimensions` e la chiave andava persa.
        let mut input = geo_contract(geographic_crs());
        input.geometries[0].encoding = Some(GeometryEncoding::Ewkb);
        let plan = projected_crs();
        let output = analyze_one(
            "geo.reproject",
            &[input],
            &json!({"target_crs": "EPSG:32632"}),
            Some(&plan),
        )
        .expect("reproject con encoding dichiarato");
        let geometry = output.active_geometry_column().expect("geometria");
        assert_eq!(geometry.encoding, Some(GeometryEncoding::Ewkb));
        assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
        let field = output
            .schema
            .field_with_name(&geometry.name)
            .expect("campo geometria");
        assert_eq!(
            crate::arrow_adapter::geometry_encoding_from_metadata(field),
            Some(GeometryEncoding::Ewkb),
            "metadato riscritto coerente col contratto (B1.4)"
        );
        assert_eq!(
            crate::arrow_adapter::geometry_dimensions_from_metadata(field),
            GeometryDimensions::Xy
        );

        // Senza encoding dichiarato il metadato riscritto non ha la chiave
        // (fingerprint invariato, retrocompatibilita').
        let output = analyze_one(
            "geo.reproject",
            &[geo_contract(geographic_crs())],
            &json!({"target_crs": "EPSG:32632"}),
            Some(&plan),
        )
        .expect("reproject senza encoding");
        let geometry = output.active_geometry_column().expect("geometria");
        let field = output
            .schema
            .field_with_name(&geometry.name)
            .expect("campo geometria");
        assert_eq!(geometry.encoding, None);
        assert_eq!(
            crate::arrow_adapter::geometry_encoding_from_metadata(field),
            None,
            "None: chiave omessa dal metadato riscritto"
        );
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
        let bad_configs: [(&str, Value); 40] = [
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
            ("geo.generate_grid", json!({})),                           // extent mancante
            ("geo.generate_grid", json!({"extent": {"xmin": 5.0, "ymin": 0.0, "xmax": 5.0, "ymax": 1.0}, "cell_size": 1.0})), // extent degenere
            ("geo.generate_grid", json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1.0, "ymax": 1.0}, "cell_size": 0.0})), // cell_size nulla
            ("geo.generate_grid", json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1.0, "ymax": 1.0}, "cell_size": 1.0, "shape": "triangle"})), // forma sconosciuta
            ("geo.generate_grid", json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1.0, "ymax": 1.0}, "cell_size": 1.0, "bogus": 1})), // campo sconosciuto
            ("geo.subdivide", json!({})),                               // max_vertices mancante
            ("geo.subdivide", json!({"max_vertices": 3})),              // sotto il minimo 4
            ("geo.snap", json!({"tolerance": 0.5})),                    // reference_wkb mancante
            ("geo.snap", json!({"reference_wkb": point_wkb_hex(), "tolerance": -1.0})), // tolleranza negativa
            ("geo.coverage_validate", json!({"tolerance": -1.0})),      // tolleranza negativa
            ("geo.coverage_validate", json!({"max_issues": 0})),        // limite nullo
            ("geo.coverage_validate", json!({"bogus": 1})),             // campo sconosciuto
            ("geo.shared_paths", json!({"min_length": -1.0})),          // lunghezza negativa
            ("geo.shared_paths", json!({"tolerance": 1.0, "bogus": true})), // campo sconosciuto
            ("geo.cluster_dbscan", json!({"min_points": 3})),           // eps mancante
            ("geo.cluster_dbscan", json!({"eps": 0.0, "min_points": 3})), // eps nulla
            ("geo.cluster_dbscan", json!({"eps": 1.0, "min_points": 0})), // min_points nullo
            ("geo.cluster_dbscan", json!({"eps": 1.0, "min_points": 3, "bogus": 1})), // campo sconosciuto
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
                encoding: None,
                nullable: true,
                types: GeometryColumnContract::undeclared_types(),
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
        // `write!` su `String` e' infallibile: l'`fmt::Result` non puo' essere Err.
        let line_hex = line.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        let result = analyze_one(
            "geo.line_locate_point",
            &inputs,
            &json!({"point_wkb": line_hex}),
            None,
        );
        assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))), "LineString accettata");

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
    fn generate_grid_resolves_crs_centroids_and_the_cell_limit() {
        let inputs = [tabular_contract()];
        let plan = projected_crs();
        let extent = json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0}, "cell_size": 5.0});

        // CRS obbligatorio: senza config `crs` ne' CRS di piano fallisce.
        let result = analyze_one("geo.generate_grid", &inputs, &extent, None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "CRS mancante accettato");

        // include_centroid: due colonne Float64 non null in coda; shape hex.
        let mut config = extent;
        config["include_centroid"] = json!(true);
        config["shape"] = json!("hex");
        config["crs"] = json!("EPSG:32632");
        let output = analyze_one("geo.generate_grid", &inputs, &config, Some(&plan))
            .expect("griglia esagonale con centroidi");
        let expected: Vec<(&str, DataType, bool)> = vec![
            (DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false),
            (CELL_I_COLUMN, DataType::UInt64, false),
            (CELL_J_COLUMN, DataType::UInt64, false),
            (CENTROID_X_COLUMN, DataType::Float64, false),
            (CENTROID_Y_COLUMN, DataType::Float64, false),
        ];
        assert_eq!(signatures(&output), expected);
        assert_eq!(output.geometries[0].crs.definition(), "EPSG:32632");

        // Limite celle: extent enorme con celle piccole fallisce in analisi.
        let over_limit = json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1e6, "ymax": 1e6}, "cell_size": 1.0});
        let result = analyze_one("geo.generate_grid", &inputs, &over_limit, Some(&plan));
        assert!(matches!(result, Err(PlenoraError::InvalidPlan(_))), "limite celle non applicato");

        // Extent con span che overflowa il conteggio celle (coordinate finite
        // ma prodotto colonne x righe non rappresentabile).
        let nan_extent = json!({"extent": {"xmin": -1e308, "ymin": 0.0, "xmax": 1e308, "ymax": 1.0}, "cell_size": 1.0});
        assert!(analyze_one("geo.generate_grid", &inputs, &nan_extent, Some(&plan)).is_err());
    }

    #[test]
    fn subdivide_expands_like_explode_and_can_rename_the_geometry() {
        let inputs = [contract_with_properties()];
        let output = analyze_one(
            "geo.subdivide",
            &inputs,
            &json!({"max_vertices": 16}),
            None,
        )
        .expect("subdivide");
        // Espansione stabile: sorted_by preservato, row_count eliminato.
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_none());
        assert_eq!(
            output.schema.fields().last().expect("ultima colonna").name(),
            PARENT_INDEX_COLUMN
        );
        // FieldId preservato (geometria in place).
        assert_eq!(output.geometries[0].field_id, FieldId(2));

        // output_column rinomina la geometria (stesso FieldId); collisione
        // con una colonna esistente rifiutata.
        let output = analyze_one(
            "geo.subdivide",
            &inputs,
            &json!({"max_vertices": 16, "output_column": "parts"}),
            None,
        )
        .expect("rinomina geometria");
        assert_eq!(output.geometries[0].name, "parts");
        assert_eq!(output.geometries[0].field_id, FieldId(2));
        assert!(output.schema.field_with_name("parts").is_ok());
        assert!(analyze_one(
            "geo.subdivide",
            &inputs,
            &json!({"max_vertices": 16, "output_column": "id"}),
            None
        )
        .is_err());
        assert!(analyze_one(
            "geo.subdivide",
            &inputs,
            &json!({"max_vertices": 16, "output_column": "  "}),
            None
        )
        .is_err());
    }

    #[test]
    fn snap_validates_the_reference_and_requires_a_projected_input() {
        let inputs = [contract_with_properties()];
        let output = analyze_one(
            "geo.snap",
            &inputs,
            &json!({"reference_wkb": point_wkb_hex(), "tolerance": 0.5}),
            None,
        )
        .expect("snap");
        // 1:1 streaming: schema e proprieta' preservati.
        assert_eq!(signatures(&output), signatures(&inputs[0]));
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_some());

        // Hex valido ma non decodificabile (byte residui dopo la geometria).
        let mut trailing = point_wkb_hex();
        trailing.push_str("00");
        assert!(analyze_one(
            "geo.snap",
            &inputs,
            &json!({"reference_wkb": trailing, "tolerance": 0.5}),
            None
        )
        .is_err());

        // SameProjected: input geografico rifiutato (il riferimento da config
        // e' assunto nello stesso CRS dell'input).
        let geographic = [geo_contract(geographic_crs())];
        let result = analyze_one(
            "geo.snap",
            &geographic,
            &json!({"reference_wkb": point_wkb_hex(), "tolerance": 0.5}),
            None,
        );
        assert!(matches!(result, Err(PlenoraError::Crs(_))), "input geografico accettato");
    }

    #[test]
    fn coverage_ops_allocate_a_fresh_geometry_and_require_projected_crs() {
        let inputs = [contract_with_properties()];

        // coverage_validate: schema nuovo, nuovo FieldId, proprieta' azzerate.
        let output = analyze_one("geo.coverage_validate", &inputs, &json!({}), None)
            .expect("coverage_validate");
        let expected: Vec<(&str, DataType, bool)> = vec![
            (ISSUE_TYPE_COLUMN, DataType::Utf8, false),
            (INDEX_A_COLUMN, DataType::UInt64, false),
            (INDEX_B_COLUMN, DataType::UInt64, false),
            (ISSUE_AREA_COLUMN, DataType::Float64, false),
            (DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false),
        ];
        assert_eq!(signatures(&output), expected);
        assert_eq!(output.geometries[0].field_id, FieldId(0), "allocatore da zero");
        assert!(!output.geometries[0].nullable);
        assert_eq!(output.geometries[0].crs.definition(), "EPSG:32632");
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());
        let field = output
            .schema
            .field_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("campo geometria");
        assert_eq!(
            field.metadata().get(GEOARROW_EXTENSION_KEY).map(String::as_str),
            Some(GEOARROW_WKB_EXTENSION)
        );

        // shared_paths: config con parametri; l'allocatore avanza.
        let mut allocator = FieldAllocator::new(7);
        let output = analyze_geo_contract(
            "geo.shared_paths",
            &inputs,
            &json!({"tolerance": 1e-6, "min_length": 0.5}),
            None,
            &mut allocator,
        )
        .expect("shared_paths");
        let expected: Vec<(&str, DataType, bool)> = vec![
            (INDEX_A_COLUMN, DataType::UInt64, false),
            (INDEX_B_COLUMN, DataType::UInt64, false),
            (SHARED_LENGTH_COLUMN, DataType::Float64, false),
            (DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false),
        ];
        assert_eq!(signatures(&output), expected);
        assert_eq!(output.geometries[0].field_id, FieldId(7));
        assert_eq!(allocator.peek(), FieldId(8));

        // SameProjected: input geografico rifiutato da entrambe.
        let geographic = [geo_contract(geographic_crs())];
        for op in ["geo.coverage_validate", "geo.shared_paths"] {
            let result = analyze_one(op, &geographic, &json!({}), None);
            assert!(matches!(result, Err(PlenoraError::Crs(_))), "{op}: CRS geografico accettato");
        }
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
                encoding: None,
                nullable: true,
                types: GeometryColumnContract::undeclared_types(),
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

    // -----------------------------------------------------------------------
    // Lineage dei metadati Arrow (R2.4).
    // -----------------------------------------------------------------------

    /// Restituisce il contratto con i metadati di SCHEMA sostituiti dalle
    /// coppie date (campi, geometrie e proprieta' invariati).
    fn attach_schema_metadata(contract: &DataContract, pairs: &[(&str, &str)]) -> DataContract {
        let mut with_metadata = contract.clone();
        with_metadata.schema = Arc::new(Schema::new_with_metadata(
            contract
                .schema
                .fields()
                .iter()
                .map(|field| field.as_ref().clone())
                .collect::<Vec<_>>(),
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        ));
        with_metadata
    }

    #[test]
    fn schema_metadata_survive_schema_rebuilding_ops() {
        // Aggregazione a sole geometrie: le colonne attributo cadono, i
        // metadati di schema no.
        let input = attach_schema_metadata(
            &geo_contract(projected_crs()),
            &[("plenora.contract.version", "1"), ("driver.note", "x")],
        );
        let expected = input.schema.metadata().clone();
        let output = analyze_one("geo.dissolve", &[input], &json!({}), None).expect("dissolve");
        assert_eq!(output.schema.metadata(), &expected, "dissolve");

        // Op di copertura (schema nuovo): metadati di schema conservati.
        let input = attach_schema_metadata(
            &geo_contract(projected_crs()),
            &[("plenora.contract.version", "1")],
        );
        let expected = input.schema.metadata().clone();
        let output =
            analyze_one("geo.coverage_validate", &[input], &json!({}), None).expect("coverage");
        assert_eq!(output.schema.metadata(), &expected, "coverage_validate");

        // Generativa con input trigger tabellare: metadati conservati.
        let input = attach_schema_metadata(&tabular_contract(), &[("driver.note", "x")]);
        let expected = input.schema.metadata().clone();
        let plan = projected_crs();
        let output = analyze_one(
            "geo.generate_grid",
            &[input],
            &json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 10.0, "ymax": 10.0}, "cell_size": 5.0}),
            Some(&plan),
        )
        .expect("generate_grid");
        assert_eq!(output.schema.metadata(), &expected, "generate_grid");
    }

    #[test]
    fn binary_ops_merge_schema_metadata_and_reject_conflicts() {
        // Chiave in una sola sorgente -> copiata; in entrambe uguale -> una
        // sola copia, nessun errore.
        let left = attach_schema_metadata(
            &geo_contract(projected_crs()),
            &[("plenora.contract.version", "1"), ("left.only", "a")],
        );
        let right = attach_schema_metadata(
            &geo_contract(projected_crs()),
            &[("plenora.contract.version", "1"), ("right.only", "b")],
        );
        let output = analyze_one(
            "geo.sjoin",
            &[left, right],
            &json!({"predicate": "intersects"}),
            None,
        )
        .expect("sjoin");
        let metadata = output.schema.metadata();
        for key in ["plenora.contract.version", "left.only", "right.only"] {
            assert!(metadata.contains_key(key), "chiave `{key}` persa nel merge");
        }
        assert_eq!(metadata.len(), 3, "nessuna chiave duplicata o spuria");

        // Chiave in entrambe con valori diversi -> errore di contratto che
        // nomina la chiave e MAI i valori (errori senza dati).
        let left = attach_schema_metadata(&geo_contract(projected_crs()), &[("shared.key", "alpha")]);
        let right = attach_schema_metadata(&geo_contract(projected_crs()), &[("shared.key", "omega")]);
        let result = analyze_one("geo.overlay", &[left, right], &json!({"mode": "union"}), None);
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(message.contains("shared.key"), "l'errore nomina la chiave: {message}");
                assert!(
                    !message.contains("alpha") && !message.contains("omega"),
                    "l'errore non contiene mai i valori: {message}"
                );
            }
            other => panic!("atteso errore di contratto, trovato {other:?}"),
        }
    }

    #[test]
    fn geometry_field_metadata_survive_geometry_only_aggregation() {
        // R2.4 identity-preserving sul campo geometria che sopravvive
        // invariato: TUTTI i metadati del campo sorgente (chiave canonica
        // `plenora.*` gia' presente e chiave esterna) sono conservati.
        let mut contract = geo_contract(projected_crs());
        let fields: Vec<Field> = contract
            .schema
            .fields()
            .iter()
            .map(|field| {
                if field.name() == DEFAULT_GEOMETRY_COLUMN {
                    let mut metadata = field.metadata().clone();
                    metadata.insert("plenora.geometry.encoding".to_owned(), "wkb".to_owned());
                    metadata.insert("driver.native".to_owned(), "kept".to_owned());
                    Field::new(field.name(), field.data_type().clone(), field.is_nullable())
                        .with_metadata(metadata)
                } else {
                    field.as_ref().clone()
                }
            })
            .collect();
        contract.schema = Arc::new(Schema::new(fields));
        let expected = contract
            .schema
            .field_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("campo geometria")
            .metadata()
            .clone();
        let output = analyze_one("geo.dissolve", &[contract], &json!({}), None).expect("dissolve");
        let field = output
            .schema
            .field_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("campo geometria");
        assert_eq!(field.metadata(), &expected, "metadati del campo geometria");
    }
}
