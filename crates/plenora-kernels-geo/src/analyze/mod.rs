//! Inferenza a secco dei `DataContract` per le operazioni `geo.*`
//! (architettura.md#planner-ed-executor).
//!
//! [`analyze_geo_contract`] ricava il contratto dell'arco in uscita da id
//! dell'operazione, contratti di input, config JSON e CRS di piano, oppure
//! fallisce **in validazione** (fail-closed), mai a runtime.
//! `required_capabilities` non si verifica qui: il controllo sui backend
//! compilati spetta al planner.
//!
//! # Config
//!
//! Le struct serde sono locali: stessi nomi e domini di `validate_parameters`
//! delle config di `plenora-engine::geo_transport`, senza i limiti di
//! trasporto, che non appartengono all'analisi semantica. Gli enum semantici
//! di `kernels-geo` sono riusati.
//!
//! # Forme di output
//!
//! Le trasformazioni 1:1 riscrivono la geometria in place con lo stesso
//! `FieldId`; quelle che cambiano tipo dichiarano i tipi dell'output e
//! sostituiscono le chiavi canoniche ereditate (piano-v5.md#contratti-di-input,
//! decisione 8). Misure e predicati aggiungono una colonna, le espansioni 1:N
//! aggiungono `__parent_index`, le aggregazioni tengono le sole geometrie, i
//! join aggiungono `__right_index`, i produttori creano una colonna geometria
//! con nuovo `FieldId`. Il dettaglio per operazione sta sulle funzioni di
//! inferenza.
//!
//! Il catalogo marca `Unary` predicati, distanze a due colonne e `split`, ma
//! un input ha una sola colonna geometria (D16): il secondo operando arriva
//! dalla config come WKB hex (`other_wkb`), validato in analisi, con CRS
//! assunto uguale a quello dell'input e coordinate nel suo dominio di
//! validita' (come `point_wkb`, `reference_wkb` e l'`extent` di
//! `generate_grid`).
//!
//! # CRS, dimensionalita', encoding
//!
//! Una definizione in config testualmente uguale al CRS di piano lo riusa
//! senza backend; altrimenti `resolve_crs`, che risolve solo la tabella dei
//! CRS integrati e fallisce chiuso sul resto (`CRS_NOT_BUILTIN`,
//! `CRS_BACKEND_UNAVAILABLE`). Ogni kernel che consuma una geometria
//! la decodifica in XY, quindi un input con `dimensions != Xy` si rifiuta a
//! compile-plan (R3.4); produttori e output ricodificati dichiarano `Xy`, e
//! le dimensionalita' estese passano solo per le op tabellari.
//!
//! La chiave `encoding` dei metadati `geo` di output si scrive solo se il
//! contratto la dichiara. EWKB senza flag Z/M e senza SRID e' byte-identico a
//! WKB ISO e passa come `xy`; il flag SRID EWKB e' sempre rifiutato dal
//! validatore celle dell'esecutore.
//!
//! # Proprieta' del contratto
//!
//! Le op 1:1 preservano `sorted_by`/`row_count`; le espansioni 1:N
//! preservano `sorted_by` ma eliminano `row_count`; join e aggregazioni
//! eliminano entrambe (declassamento obbligatorio, par. 4.3).

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
use self::helpers::crs_requirement;
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
/// (architettura.md): inferenza a secco del contratto di output.
///
/// `plan_crs` e' il CRS di piano gia' risolto dal planner (usato dai
/// produttori `from_coords`, `from_wkt`, `generate_grid`); `fields` alloca i `FieldId`
/// delle nuove colonne geometriche nel namespace globale del grafo.
///
/// # Errors
///
/// Fallisce (fail-closed, in validazione) se: l'op non e' nel catalogo o non
/// e' geo; l'arieta' non e' rispettata; un input non ha esattamente una
/// colonna geometria attiva; la colonna geometria non e' identificabile
/// dal trasporto (ne' estensione `geoarrow.wkb` ne' chiavi canoniche,
/// piano-v5.md#contratti-di-input decisione 8); la geometria di input non e' `Xy` per un kernel
/// che la elabora; il `crs_requirement` non e' soddisfatto;
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
    // I produttori sono unari: l'arieta' e' gia' verificata sopra. Il
    // rifiuto dell'N-aria resta prima del controllo sul numero di input,
    // quindi qui l'arieta' e' solo 1 o 2.
    match (descriptor.id, expected_arity) {
        ("geo.from_coords", _) => {
            // Il messaggio nomina l'op come richiesta (anche un alias).
            let requirement = crs_requirement(op, descriptor)?;
            analyze_from_coords(
                descriptor.id,
                &inputs[0],
                config,
                plan_crs,
                requirement,
                fields,
            )
        }
        ("geo.from_wkt", _) => {
            let op = descriptor.id;
            let requirement = crs_requirement(op, descriptor)?;
            analyze_from_wkt(op, &inputs[0], config, plan_crs, requirement, fields)
        }
        ("geo.generate_grid", _) => {
            let op = descriptor.id;
            let requirement = crs_requirement(op, descriptor)?;
            analyze_generate_grid(op, &inputs[0], config, plan_crs, requirement, fields)
        }
        (_, 2) => analyze_binary(descriptor, inputs, config),
        _ => analyze_unary(descriptor, &inputs[0], config, fields),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use geo::{Geometry, Point};
    use geozero::{CoordDimensions, ToWkb};
    use plenora_core::arrow::{DataType, Field, Schema};
    use plenora_core::catalog::{find_operation, CrsRequirement, Family, Maturity, CATALOG};
    use plenora_core::contract::{
        ContractCrs, ContractProperties, ContractProperty, DataContract, FieldAllocator, FieldId,
        GeometryColumnContract, GeometryDimensions, GeometryType, GeometryTypesProperty,
        PropertyConfidence, PropertyScope, TypesDeclaration,
    };
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use plenora_core::esadecimale::esadecimale;
    use plenora_core::{ErrorCategory, PlenoraError, Result};
    use serde_json::{json, Value};

    use super::helpers::short_id;
    use super::*;
    use crate::arrow_adapter::{
        geo_metadata_json_with_dimensions, DEFAULT_GEOMETRY_COLUMN, GEOARROW_EXTENSION_KEY,
        GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY, PLENORA_GEOMETRY_DIMENSIONS_KEY,
        PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
        PLENORA_GEOMETRY_TYPES_KEY,
    };

    /// Il CRS risolto di un contratto di colonna (i test di analyze lavorano
    /// su CRS risolti; il gate R4.6.3 per `Missing` ha test dedicati).
    fn resolved_crs_of(geometry: &GeometryColumnContract) -> &ResolvedCrs {
        geometry.crs.as_resolved().expect("CRS risolto")
    }

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

    /// Contratto con la colonna geometria `FieldId(2)`, XY, nullable e senza
    /// tipi dichiarati; i campi e il CRS sono del chiamante.
    fn contract_with_geometry(fields: Vec<Field>, crs: ContractCrs) -> DataContract {
        DataContract::new(
            Arc::new(Schema::new(fields)),
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

    fn geo_contract(crs: ResolvedCrs) -> DataContract {
        contract_with_geometry(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("label", DataType::Utf8, true),
                geometry_arrow_field(),
            ],
            ContractCrs::Resolved(crs),
        )
    }

    /// Il contratto con i metadati del campo geometria modificati da
    /// `modifica`, gli altri campi invariati. `conserva_metadati_schema`
    /// dice se lo schema ricostruito tiene i metadati di schema o nasce
    /// senza: i chiamanti fanno l'una e l'altra cosa, e la scelta resta loro.
    fn with_geometry_field_metadata(
        mut contract: DataContract,
        conserva_metadati_schema: bool,
        modifica: impl Fn(&mut HashMap<String, String>),
    ) -> DataContract {
        let fields: Vec<Field> = contract
            .schema
            .fields()
            .iter()
            .map(|field| {
                if field.name() == DEFAULT_GEOMETRY_COLUMN {
                    let mut metadata = field.metadata().clone();
                    modifica(&mut metadata);
                    field.as_ref().clone().with_metadata(metadata)
                } else {
                    field.as_ref().clone()
                }
            })
            .collect();
        contract.schema = Arc::new(if conserva_metadati_schema {
            Schema::new_with_metadata(fields, contract.schema.metadata().clone())
        } else {
            Schema::new(fields)
        });
        contract
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
        esadecimale(&wkb)
    }

    /// Replay deterministico dell'invariante di `fuzz_targets/analyze_geo.rs`
    /// («mai panic») sui soli parametri WKB esadecimali.
    ///
    /// Non esplora: ripete un elenco scritto a mano sul percorso
    /// `analyze_geo_contract` -> `validate_wkb_hex`, per ogni operazione che
    /// accetta un WKB da configurazione, dove la campagna libFuzzer non e'
    /// eseguibile (release.md#fuzzing). Ogni caso deve arrivare davvero a
    /// `validate_wkb_hex`: il controllo con un WKB valido prova che il resto
    /// della config e l'input passano, e l'errore atteso e' quello del
    /// decoder, non di un controllo precedente.
    #[test]
    fn nessun_wkb_di_config_ostile_manda_in_panic_l_analisi() {
        // Lunghezza pari in byte con taglio dentro un carattere, lunghezza
        // dispari, cifre non esadecimali, vuoto: rifiutati dalla decodifica.
        let non_esadecimali = [
            "a\u{e9}b",
            "\u{e9}\u{e9}",
            "0\u{e9}0",
            "\u{1F642}",
            "\u{1F642}\u{1F642}",
            "ab\u{e9}",
            "",
            "0",
            "abc",
            "zz",
            "0g",
            "\u{0}\u{0}",
        ];
        // Esadecimale ben formato ma non un WKB valido: rifiutato dalla
        // validazione strutturale.
        let non_wkb = ["00", "ffffffff"];
        // Ogni operazione che legge un WKB dalla configurazione, con gli
        // altri parametri obbligatori validi.
        let mut parametri: Vec<(&str, &str, Value)> = vec![
            ("geo.distance", "other_wkb", json!({})),
            ("geo.hausdorff_distance", "other_wkb", json!({})),
            ("geo.frechet_distance", "other_wkb", json!({})),
            ("geo.haversine_distance", "other_wkb", json!({})),
            ("geo.geodesic_distance", "other_wkb", json!({})),
            ("geo.bearing", "other_wkb", json!({})),
            ("geo.split", "other_wkb", json!({})),
            ("geo.line_locate_point", "point_wkb", json!({})),
            ("geo.snap", "reference_wkb", json!({"tolerance": 0.5})),
        ];
        for predicato in [
            "geo.predicate_intersects",
            "geo.predicate_disjoint",
            "geo.predicate_contains",
            "geo.predicate_within",
            "geo.predicate_equals_topo",
            "geo.predicate_covers",
            "geo.predicate_covered_by",
            "geo.predicate_contains_properly",
            "geo.predicate_touches",
            "geo.predicate_crosses",
            "geo.predicate_overlaps",
        ] {
            parametri.push((predicato, "other_wkb", json!({})));
        }

        let analizza = |op: &str, parametro: &str, base: &Value, wkb: &str| {
            let mut config = base.as_object().expect("config oggetto").clone();
            config.insert(parametro.to_owned(), Value::String(wkb.to_owned()));
            analyze_geo_contract(
                op,
                &[geo_contract(input_crs_for(op))],
                &Value::Object(config),
                None,
                &mut FieldAllocator::new(100),
            )
        };

        for (op, parametro, base) in &parametri {
            analizza(op, parametro, base, &point_wkb_hex())
                .unwrap_or_else(|errore| panic!("{op}: controllo con WKB valido: {errore}"));

            let atteso =
                format!("{op}: parametro `{parametro}` non valido: WKB esadecimale non valido");
            for ostile in non_esadecimali {
                match analizza(op, parametro, base, ostile) {
                    Err(PlenoraError::InvalidPlan(messaggio)) if messaggio == atteso => {}
                    esito => panic!("{op} con {ostile:?}: atteso «{atteso}», ottenuto {esito:?}"),
                }
            }
            for ostile in non_wkb {
                match analizza(op, parametro, base, ostile) {
                    Err(PlenoraError::InvalidPlan(messaggio))
                        if messaggio.starts_with("struttura WKB non valida: ") => {}
                    esito => panic!(
                        "{op} con {ostile:?}: atteso un rifiuto strutturale, ottenuto {esito:?}"
                    ),
                }
            }
        }
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
        /// Op di copertura (WholeToMany): schema nuovo completo (tutto
        /// non null), geometria con nuovo `FieldId` e CRS dell'input.
        CoverageRows(Vec<(&'static str, DataType, bool)>),
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

    // Tabella di fixture: la lunghezza e' data dall'elenco dei casi
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
            unary(
                op,
                json!({}),
                Expect::Appended(vec![float_column(short_id(op))]),
            )
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
            unchanged(
                "geo.affine_transform",
                json!({"coefficients": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]}),
            ),
            unchanged("geo.translate", json!({"x_offset": 1.0, "y_offset": 2.0})),
            unchanged("geo.scale", json!({"x_factor": 2.0, "y_factor": 2.0})),
            unchanged("geo.rotate", json!({"degrees": 90.0})),
            unchanged("geo.concave_hull", json!({"concavity": 2.0})),
            unchanged("geo.densify", json!({"max_segment_length": 10.0})),
            unchanged("geo.snap_to_grid", json!({"grid_size": 1.0})),
            unchanged(
                "geo.line_substring",
                json!({"start_ratio": 0.1, "end_ratio": 0.9}),
            ),
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
            // --- Coperture (WholeToMany, schema nuovo) -----------------------
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
            // --- Clustering (Blocking, output allineato alle righe) ----------
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
            unary(
                "geo.polygon_builder",
                json!({}),
                Expect::GeometryOnly(vec![]),
            ),
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
            // --- Costruzione ------------------------------------------------
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
                Expect::Appended(vec![(
                    "predicate_contains_properly",
                    DataType::Boolean,
                    true,
                )]),
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
        let output = analyze_geo_contract(
            case.op,
            &inputs,
            &case.config,
            Some(&plan_crs),
            &mut allocator,
        )
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

    fn assert_appended(
        output: &DataContract,
        input: &DataContract,
        extra: &[(&str, DataType, bool)],
    ) {
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
        assert_eq!(
            output_geometry.field_id, input_geometry.field_id,
            "FieldId preservato"
        );
        assert_eq!(output_geometry.name, input_geometry.name);
    }

    #[test]
    fn table_covers_all_and_only_the_74_catalog_geo_ops() {
        let catalog_ops: HashSet<&str> = CATALOG
            .iter()
            .filter(|op| op.family == Family::Geo)
            .map(|op| op.id)
            .collect();
        assert_eq!(catalog_ops.len(), 74);
        let case_ops: HashSet<&str> = cases().iter().map(|case| case.op).collect();
        assert_eq!(case_ops.len(), 74, "casi duplicati nella tabella");
        assert_eq!(catalog_ops, case_ops);
    }

    /// Come `geo_contract`, con la dimensionalita' dichiarata.
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
        // (a) per OGNI op geo del catalogo, un input con dimensionalita'
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
                // Il produttore ricodifica WKB ISO XY — nessun encoding
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
                let mut inputs = vec![geo_contract_with_dimensions(
                    input_crs_for(case.op),
                    dimensions,
                )];
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
                        panic!(
                            "{}: atteso Unsupported con {dimensions}, trovato {other:?}",
                            case.op
                        )
                    }
                    Ok(_) => panic!(
                        "{}: input {dimensions} accettato: la dimensionalita' non \
                         xy passa in silenzio come xy",
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
                    assert_eq!(
                        signatures(&output),
                        signatures(&input),
                        "{}: schema",
                        case.op
                    );
                    assert_geometry_preserved(&output, &input);
                    assert_eq!(
                        resolved_crs_of(output.active_geometry_column().unwrap()).definition(),
                        resolved_crs_of(&input.geometries[0]).definition(),
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
                    let geometry = output
                        .active_geometry_column()
                        .expect("geometria in output");
                    assert_eq!(
                        geometry.field_id,
                        FieldId(2),
                        "{}: FieldId preservato",
                        case.op
                    );
                    assert!(
                        geometry.nullable,
                        "{}: geometria aggregata nullable",
                        case.op
                    );
                }
                Expect::Diagnostics => {
                    assert!(
                        output.geometries.is_empty(),
                        "{}: niente geometria",
                        case.op
                    );
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
                    assert_eq!(
                        geometry.field_id,
                        FieldId(100),
                        "{}: FieldId allocato",
                        case.op
                    );
                    assert!(!geometry.nullable, "{}: geometria non null", case.op);
                    assert_eq!(resolved_crs_of(geometry).definition(), "EPSG:32632");
                    assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
                    let field = output
                        .schema
                        .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                        .expect("campo geometria");
                    assert_eq!(
                        field
                            .metadata()
                            .get(GEOARROW_EXTENSION_KEY)
                            .map(String::as_str),
                        Some(GEOARROW_WKB_EXTENSION)
                    );
                    let geo: Value = serde_json::from_str(
                        field
                            .metadata()
                            .get(GEO_METADATA_KEY)
                            .expect("geo metadata"),
                    )
                    .expect("geo JSON");
                    assert_eq!(geo.get("crs").and_then(Value::as_str), Some("EPSG:32632"));
                }
                Expect::FromWkt => {
                    let mut expected = signatures(&input);
                    expected.push((DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true));
                    assert_eq!(signatures(&output), expected, "{}: schema", case.op);
                    let geometry = output.active_geometry_column().expect("geometria creata");
                    assert_eq!(
                        geometry.field_id,
                        FieldId(100),
                        "{}: FieldId allocato",
                        case.op
                    );
                    assert!(geometry.nullable, "{}: geometria nullable", case.op);
                    assert_eq!(resolved_crs_of(geometry).definition(), "EPSG:32632");
                    assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
                    let field = output
                        .schema
                        .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                        .expect("campo geometria");
                    assert_eq!(
                        field
                            .metadata()
                            .get(GEOARROW_EXTENSION_KEY)
                            .map(String::as_str),
                        Some(GEOARROW_WKB_EXTENSION)
                    );
                    let geo: Value = serde_json::from_str(
                        field
                            .metadata()
                            .get(GEO_METADATA_KEY)
                            .expect("geo metadata"),
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
                    assert_eq!(
                        geometry.field_id,
                        FieldId(100),
                        "{}: FieldId allocato",
                        case.op
                    );
                    assert!(
                        !geometry.nullable,
                        "{}: geometria di griglia non null",
                        case.op
                    );
                    assert_eq!(resolved_crs_of(geometry).definition(), "EPSG:32632");
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
                    assert_eq!(
                        geometry.field_id,
                        FieldId(100),
                        "{}: FieldId allocato",
                        case.op
                    );
                    assert!(!geometry.nullable, "{}: geometria non null", case.op);
                    assert_eq!(geometry.dimensions, GeometryDimensions::Xy);
                    assert_eq!(
                        resolved_crs_of(geometry).definition(),
                        resolved_crs_of(&input.geometries[0]).definition(),
                        "{}: CRS dell'input",
                        case.op
                    );
                    // L'azzeramento delle proprieta' non si prova qui:
                    // l'input della tabella non ne dichiara. Lo prova
                    // `coverage_ops_allocate_a_fresh_geometry_and_require_projected_crs`
                    // su un input che le dichiara.
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
                assert_eq!(
                    allocator.peek(),
                    FieldId(100),
                    "{}: allocatore intatto",
                    case.op
                );
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
            let result = if matches!(
                case.op,
                "geo.from_coords" | "geo.from_wkt" | "geo.generate_grid"
            ) {
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
            assert!(
                result.is_err(),
                "{}: input non geometrico accettato",
                case.op
            );
        }
    }

    #[test]
    fn binary_ops_reject_a_second_input_without_geometry() {
        for op in [
            "geo.sjoin",
            "geo.clip",
            "geo.overlay",
            "geo.nearest",
            "geo.within",
        ] {
            let config = match op {
                "geo.sjoin" => json!({"predicate": "intersects"}),
                "geo.overlay" => json!({"mode": "union"}),
                _ => json!({}),
            };
            let inputs = [geo_contract(projected_crs()), tabular_contract()];
            let result =
                analyze_geo_contract(op, &inputs, &config, None, &mut FieldAllocator::new(0));
            assert!(
                result.is_err(),
                "{op}: secondo input non geometrico accettato"
            );
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
            let result =
                analyze_geo_contract(op, &inputs, &json!({}), None, &mut FieldAllocator::new(0));
            assert!(
                matches!(result, Err(PlenoraError::Unsupported(_))),
                "{op}: atteso Unsupported"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Copertura dei 5 CrsRequirement.
    // -----------------------------------------------------------------------

    fn analyze_one(
        op: &str,
        inputs: &[DataContract],
        config: &Value,
        plan_crs: Option<&ResolvedCrs>,
    ) -> Result<DataContract> {
        analyze_geo_contract(op, inputs, config, plan_crs, &mut FieldAllocator::new(0))
    }

    #[test]
    fn known_requirement_accepts_any_resolved_crs() {
        for op in [
            "geo.explode",
            "geo.to_wkt",
            "geo.vertex_count",
            "geo.geometry_diagnostics",
            "geo.make_valid",
        ] {
            let inputs = [geo_contract(geographic_crs())];
            analyze_one(op, &inputs, &json!({}), None)
                .unwrap_or_else(|error| panic!("{op} su CRS geografico: {error}"));
        }
    }

    #[test]
    fn projected_requirement_rejects_geographic_input() {
        for op in [
            "geo.buffer",
            "geo.area",
            "geo.simplify",
            "geo.voronoi",
            "geo.dissolve",
        ] {
            let config = match op {
                "geo.buffer" => json!({"distance": 1.0}),
                "geo.simplify" => json!({"tolerance": 1.0}),
                _ => json!({}),
            };
            let inputs = [geo_contract(geographic_crs())];
            let result = analyze_one(op, &inputs, &config, None);
            assert!(
                matches!(result, Err(PlenoraError::Crs(_))),
                "{op}: CRS geografico accettato"
            );
        }
    }

    #[test]
    fn geographic_requirement_rejects_projected_input() {
        for op in ["geo.geodesic_area", "geo.geodesic_line_length"] {
            let inputs = [geo_contract(projected_crs())];
            let result = analyze_one(op, &inputs, &json!({}), None);
            assert!(
                matches!(result, Err(PlenoraError::Crs(_))),
                "{op}: CRS proiettato accettato"
            );
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

        let different = [
            geo_contract(projected_crs()),
            geo_contract(other_projected_crs()),
        ];
        let result = analyze_one("geo.sjoin", &different, &config, None);
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "CRS diversi accettati"
        );

        let geographic_right = [
            geo_contract(projected_crs()),
            geo_contract(geographic_crs()),
        ];
        let result = analyze_one("geo.sjoin", &geographic_right, &config, None);
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "right geografico accettato"
        );

        // Variante unaria (other_wkb): il CRS dell'input deve essere proiettato.
        let inputs = [geo_contract(geographic_crs())];
        let result = analyze_one("geo.distance", &inputs, &other_wkb_config(), None);
        assert!(matches!(result, Err(PlenoraError::Crs(_))));
    }

    #[test]
    fn from_coords_uses_config_or_plan_crs_and_validates_its_requirement() {
        // from_coords richiede un CRS proiettato (catalogo): il CRS di piano
        // geografico e' rifiutato.
        let inputs = [tabular_contract()];
        let geographic_plan = geographic_crs();
        let result = analyze_one(
            "geo.from_coords",
            &inputs,
            &json!({}),
            Some(&geographic_plan),
        );
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "CRS geografico accettato"
        );

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
            resolved_crs_of(output.active_geometry_column().unwrap()).definition(),
            "EPSG:32632"
        );
    }

    // -----------------------------------------------------------------------
    // Validazione config fail-closed.
    // -----------------------------------------------------------------------

    /// Ogni config invalida si ferma al SUO controllo: categoria attesa e
    /// frammento che identifica la guardia (`{op}: ...`), non un errore
    /// qualunque. I produttori (`generate_grid`) ricevono un input tabellare:
    /// con una geometria in ingresso si fermerebbero prima, al rifiuto
    /// dell'input, e i controlli sulla config non sarebbero raggiunti.
    #[test]
    // Tabella di fixture: la lunghezza e' data dall'elenco delle config
    // invalide con il loro esito atteso, non da logica da spezzare.
    #[allow(clippy::too_many_lines)]
    fn configs_are_strictly_validated() {
        let geometrico = [geo_contract(projected_crs())];
        let tabellare = [tabular_contract()];
        let bad_configs: [(&str, Value, ErrorCategory, &str); 40] = [
            (
                "geo.buffer",
                json!({}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `distance`",
            ),
            (
                "geo.buffer",
                json!({"distance": 1.0, "bogus": 1}),
                ErrorCategory::InvalidPlan,
                "config non valida: unknown field `bogus`",
            ),
            (
                "geo.buffer",
                json!({"distance": "molto"}),
                ErrorCategory::InvalidPlan,
                "config non valida: invalid type",
            ),
            (
                "geo.simplify",
                json!({"tolerance": -1.0}),
                ErrorCategory::InvalidPlan,
                "parametro `tolerance` non valido: deve essere non negativo",
            ),
            (
                "geo.affine_transform",
                json!({"coefficients": [1.0, 2.0]}),
                ErrorCategory::InvalidPlan,
                "parametro `coefficients` non valido: devono essere esattamente 6 coefficienti",
            ),
            (
                "geo.translate",
                json!({"x_offset": 1.0}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `y_offset`",
            ),
            (
                "geo.concave_hull",
                json!({"concavity": 0.0}),
                ErrorCategory::InvalidPlan,
                "parametro `concavity` non valido: deve essere maggiore di zero",
            ),
            (
                "geo.densify",
                json!({"max_segment_length": 0.0}),
                ErrorCategory::InvalidPlan,
                "parametro `max_segment_length` non valido: deve essere maggiore di zero",
            ),
            (
                "geo.snap_to_grid",
                json!({"grid_size": -1.0}),
                ErrorCategory::InvalidPlan,
                "parametro `grid_size` non valido: deve essere maggiore di zero",
            ),
            (
                "geo.line_substring",
                json!({"start_ratio": 0.9, "end_ratio": 0.1}),
                ErrorCategory::InvalidPlan,
                "parametro `start_ratio/end_ratio` non valido: start_ratio non puo superare end_ratio",
            ),
            (
                "geo.line_interpolate_point",
                json!({"ratio": 1.5}),
                ErrorCategory::InvalidPlan,
                "parametro `ratio` non valido: deve essere finito e compreso tra zero e uno",
            ),
            (
                "geo.clean_topology",
                json!({}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `snap_tolerance`",
            ),
            (
                "geo.voronoi",
                json!({"max_points": 1}),
                ErrorCategory::InvalidPlan,
                "parametro `max_points` non valido: deve essere almeno 2",
            ),
            (
                "geo.distance",
                json!({"other_wkb": "zz"}),
                ErrorCategory::InvalidPlan,
                "parametro `other_wkb` non valido: WKB esadecimale non valido",
            ),
            (
                "geo.geometry_accessors",
                json!({"fields": []}),
                ErrorCategory::InvalidPlan,
                "parametro `fields` non valido: non deve essere vuoto",
            ),
            (
                "geo.geometry_accessors",
                json!({"fields": ["geometry_type", "geometry_type"]}),
                ErrorCategory::InvalidPlan,
                "parametro `fields` non valido: campi duplicati",
            ),
            (
                "geo.geometry_accessors",
                json!({"fields": ["bogus"]}),
                ErrorCategory::InvalidPlan,
                "config non valida: unknown variant `bogus`",
            ),
            (
                "geo.collect",
                json!({"group_by": []}),
                ErrorCategory::InvalidPlan,
                "parametro `group_by` non valido: non deve essere vuoto",
            ),
            (
                "geo.collect",
                json!({"group_by": ["assente"]}),
                ErrorCategory::Schema,
                "colonna `assente` assente dallo schema",
            ),
            (
                "geo.collect",
                json!({"group_by": ["geometry"]}),
                ErrorCategory::InvalidPlan,
                "parametro `group_by` non valido: la colonna geometria non puo' essere chiave di gruppo",
            ),
            (
                "geo.line_locate_point",
                json!({}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `point_wkb`",
            ),
            (
                "geo.line_locate_point",
                json!({"point_wkb": "zz"}),
                ErrorCategory::InvalidPlan,
                "parametro `point_wkb` non valido: WKB esadecimale non valido",
            ),
            (
                "geo.generate_grid",
                json!({}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `extent`",
            ),
            (
                "geo.generate_grid",
                json!({"extent": {"xmin": 5.0, "ymin": 0.0, "xmax": 5.0, "ymax": 1.0}, "cell_size": 1.0}),
                ErrorCategory::InvalidPlan,
                "parametro extent non valido: xmax deve essere maggiore di xmin",
            ),
            (
                "geo.generate_grid",
                json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1.0, "ymax": 1.0}, "cell_size": 0.0}),
                ErrorCategory::InvalidPlan,
                "parametro cell_size non valido: deve essere finito e maggiore di zero",
            ),
            (
                "geo.generate_grid",
                json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1.0, "ymax": 1.0}, "cell_size": 1.0, "shape": "triangle"}),
                ErrorCategory::InvalidPlan,
                "config non valida: unknown variant `triangle`",
            ),
            (
                "geo.generate_grid",
                json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1.0, "ymax": 1.0}, "cell_size": 1.0, "bogus": 1}),
                ErrorCategory::InvalidPlan,
                "config non valida: unknown field `bogus`",
            ),
            (
                "geo.subdivide",
                json!({}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `max_vertices`",
            ),
            (
                "geo.subdivide",
                json!({"max_vertices": 3}),
                ErrorCategory::InvalidPlan,
                "parametro `max_vertices` non valido: deve essere almeno 4",
            ),
            (
                "geo.snap",
                json!({"tolerance": 0.5}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `reference_wkb`",
            ),
            (
                "geo.snap",
                json!({"reference_wkb": point_wkb_hex(), "tolerance": -1.0}),
                ErrorCategory::InvalidPlan,
                "parametro `tolerance` non valido: deve essere non negativo",
            ),
            (
                "geo.coverage_validate",
                json!({"tolerance": -1.0}),
                ErrorCategory::InvalidPlan,
                "parametro `tolerance` non valido: deve essere non negativo",
            ),
            (
                "geo.coverage_validate",
                json!({"max_issues": 0}),
                ErrorCategory::InvalidPlan,
                "parametro `max_issues` non valido: deve essere maggiore di zero",
            ),
            (
                "geo.coverage_validate",
                json!({"bogus": 1}),
                ErrorCategory::InvalidPlan,
                "config non valida: unknown field `bogus`",
            ),
            (
                "geo.shared_paths",
                json!({"min_length": -1.0}),
                ErrorCategory::InvalidPlan,
                "parametro `min_length` non valido: deve essere non negativo",
            ),
            (
                "geo.shared_paths",
                json!({"tolerance": 1.0, "bogus": true}),
                ErrorCategory::InvalidPlan,
                "config non valida: unknown field `bogus`",
            ),
            (
                "geo.cluster_dbscan",
                json!({"min_points": 3}),
                ErrorCategory::InvalidPlan,
                "config non valida: missing field `eps`",
            ),
            (
                "geo.cluster_dbscan",
                json!({"eps": 0.0, "min_points": 3}),
                ErrorCategory::InvalidPlan,
                "parametro `eps` non valido: deve essere maggiore di zero",
            ),
            (
                "geo.cluster_dbscan",
                json!({"eps": 1.0, "min_points": 0}),
                ErrorCategory::InvalidPlan,
                "parametro `min_points` non valido: deve essere almeno 1",
            ),
            (
                "geo.cluster_dbscan",
                json!({"eps": 1.0, "min_points": 3, "bogus": 1}),
                ErrorCategory::InvalidPlan,
                "config non valida: unknown field `bogus`",
            ),
        ];
        for (op, config, categoria, frammento) in bad_configs {
            let inputs: &[DataContract] = if op == "geo.generate_grid" {
                &tabellare
            } else {
                &geometrico
            };
            let errore = analyze_one(op, inputs, &config, None)
                .expect_err(&format!("{op} con config {config}: accettata"));
            let atteso = format!("{op}: {frammento}");
            assert!(
                errore.category() == categoria && errore.to_string().contains(&atteso),
                "{op} con config {config}: atteso {categoria:?} con «{atteso}», ottenuto {errore:?}"
            );
        }

        // other_wkb esadecimale ma con byte residui dopo la geometria.
        let mut trailing = point_wkb_hex();
        trailing.push_str("00");
        let result = analyze_one(
            "geo.distance",
            &geometrico,
            &json!({"other_wkb": trailing}),
            None,
        );
        assert!(
            matches!(
                &result,
                Err(PlenoraError::InvalidPlan(messaggio))
                    if messaggio == "struttura WKB non valida: byte residui dopo la geometria"
            ),
            "WKB con byte residui: {result:?}"
        );

        // Config non oggetto.
        let result = analyze_one("geo.centroid", &geometrico, &json!("centroid"), None);
        assert!(
            matches!(
                &result,
                Err(PlenoraError::InvalidPlan(messaggio))
                    if messaggio.starts_with("geo.centroid: config non valida: ")
            ),
            "config non oggetto: {result:?}"
        );
    }

    #[test]
    fn binary_configs_are_strictly_validated() {
        let inputs = [geo_contract(projected_crs()), geo_contract(projected_crs())];
        // Come per le unarie: ogni config si ferma al proprio controllo.
        let bad_configs: [(&str, Value, &str); 5] = [
            (
                "geo.sjoin",
                json!({}),
                "config non valida: missing field `predicate`",
            ),
            (
                "geo.sjoin",
                json!({"predicate": "nope"}),
                "config non valida: unknown variant `nope`",
            ),
            (
                "geo.overlay",
                json!({}),
                "config non valida: missing field `mode`",
            ),
            (
                "geo.overlay",
                json!({"mode": "intersection", "x": 1}),
                "config non valida: unknown field `x`",
            ),
            (
                "geo.nearest",
                json!({"max_distance": -1.0}),
                "parametro `max_distance` non valido: deve essere non negativo",
            ),
        ];
        for (op, config, frammento) in bad_configs {
            let result = analyze_one(op, &inputs, &config, None);
            let atteso = format!("{op}: {frammento}");
            assert!(
                matches!(&result, Err(PlenoraError::InvalidPlan(messaggio)) if messaggio.starts_with(&atteso)),
                "{op} con config {config}: atteso «{atteso}», ottenuto {result:?}"
            );
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
        assert!(analyze_one(
            "geo.from_wkt",
            &inputs,
            &json!({"wkt_column": "geom_text"}),
            Some(&plan)
        )
        .is_err());
        let numeric = [tabular_contract()];
        assert!(analyze_one(
            "geo.from_wkt",
            &numeric,
            &json!({"wkt_column": "x"}),
            Some(&plan)
        )
        .is_err());

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
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "CRS mancante accettato"
        );
        // Config `crs` coincidente col piano: riuso senza backend.
        let output = analyze_one(
            "geo.from_wkt",
            &inputs,
            &json!({"wkt_column": "wkt", "crs": "EPSG:32632"}),
            Some(&plan),
        )
        .expect("crs da config = piano");
        assert_eq!(
            resolved_crs_of(&output.geometries[0]).definition(),
            "EPSG:32632"
        );
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
            output
                .schema
                .fields()
                .last()
                .expect("ultima colonna")
                .name(),
            "acc_is_closed"
        );

        // Collisione con una colonna esistente: fail-closed.
        let with_accessor_column = contract_with_geometry(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("geometry_type", DataType::Utf8, true),
                geometry_arrow_field(),
            ],
            ContractCrs::Resolved(projected_crs()),
        );
        assert!(analyze_one(
            "geo.geometry_accessors",
            &[with_accessor_column],
            &json!({}),
            None
        )
        .is_err());

        // 1:1 sulle righe: proprieta' preservate.
        let inputs = [contract_with_properties()];
        let output = analyze_one("geo.geometry_accessors", &inputs, &json!({}), None)
            .expect("accessors su contratto con proprieta'");
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_some());
    }

    #[test]
    fn geometry_accessors_tutti_i_campi_seguono_l_ordine_canonico() {
        // Ogni campo di `AccessorFieldParam` e' selezionato in ordine sparso:
        // l'indice di `column_index` deve allinearsi ad ACCESSOR_COLUMNS
        // (nome E tipo), qualunque sia l'ordine della richiesta.
        let inputs = [geo_contract(projected_crs())];
        let output = analyze_one(
            "geo.geometry_accessors",
            &inputs,
            &json!({"fields": [
                "end_point", "num_interior_rings", "is_closed",
                "start_point", "num_geometries", "geometry_type"
            ]}),
            None,
        )
        .expect("tutti i campi accessor");
        let selected: Vec<(&str, &DataType)> = output
            .schema
            .fields()
            .iter()
            .skip(3)
            .map(|field| (field.name().as_str(), field.data_type()))
            .collect();
        let expected: Vec<(&str, &DataType)> = ACCESSOR_COLUMNS
            .iter()
            .map(|(name, data_type)| (*name, data_type))
            .collect();
        assert_eq!(selected, expected);
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
        assert!(
            output.properties.sorted_by.is_none(),
            "aggregazione: sorted_by declassato"
        );
        assert!(
            output.properties.row_count.is_none(),
            "aggregazione: row_count declassato"
        );

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
        let line_hex = esadecimale(&line);
        let result = analyze_one(
            "geo.line_locate_point",
            &inputs,
            &json!({"point_wkb": line_hex}),
            None,
        );
        assert!(
            matches!(result, Err(PlenoraError::InvalidPlan(_))),
            "LineString accettata"
        );

        // Override del nome colonna; proprieta' preservate (1:1 streaming).
        let output = analyze_one(
            "geo.line_locate_point",
            &inputs,
            &json!({"point_wkb": point_wkb_hex(), "output_column": "frac"}),
            None,
        )
        .expect("override nome colonna");
        assert_eq!(
            output
                .schema
                .fields()
                .last()
                .expect("ultima colonna")
                .name(),
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
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "CRS mancante accettato"
        );

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
        assert_eq!(
            resolved_crs_of(&output.geometries[0]).definition(),
            "EPSG:32632"
        );

        // Limite celle: extent enorme con celle piccole fallisce in analisi.
        let over_limit = json!({"extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1e6, "ymax": 1e6}, "cell_size": 1.0});
        let result = analyze_one("geo.generate_grid", &inputs, &over_limit, Some(&plan));
        assert!(
            matches!(result, Err(PlenoraError::InvalidPlan(_))),
            "limite celle non applicato"
        );

        // Extent con span che overflowa il conteggio celle (coordinate finite
        // ma prodotto colonne x righe non rappresentabile).
        let nan_extent = json!({"extent": {"xmin": -1e308, "ymin": 0.0, "xmax": 1e308, "ymax": 1.0}, "cell_size": 1.0});
        assert!(analyze_one("geo.generate_grid", &inputs, &nan_extent, Some(&plan)).is_err());
    }

    #[test]
    fn subdivide_expands_like_explode_and_can_rename_the_geometry() {
        let inputs = [contract_with_properties()];
        let output = analyze_one("geo.subdivide", &inputs, &json!({"max_vertices": 16}), None)
            .expect("subdivide");
        // Espansione stabile: sorted_by preservato, row_count eliminato.
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_none());
        assert_eq!(
            output
                .schema
                .fields()
                .last()
                .expect("ultima colonna")
                .name(),
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
        assert!(
            matches!(result, Err(PlenoraError::Crs(_))),
            "input geografico accettato"
        );
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
        assert_eq!(
            output.geometries[0].field_id,
            FieldId(0),
            "allocatore da zero"
        );
        assert!(!output.geometries[0].nullable);
        assert_eq!(
            resolved_crs_of(&output.geometries[0]).definition(),
            "EPSG:32632"
        );
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());
        let field = output
            .schema
            .field_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("campo geometria");
        assert_eq!(
            field
                .metadata()
                .get(GEOARROW_EXTENSION_KEY)
                .map(String::as_str),
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
        // Anche qui l'input dichiara ordinamento e conteggio: non valgono
        // per le righe nuove.
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());

        // SameProjected: input geografico rifiutato da entrambe.
        let geographic = [geo_contract(geographic_crs())];
        for op in ["geo.coverage_validate", "geo.shared_paths"] {
            let result = analyze_one(op, &geographic, &json!({}), None);
            assert!(
                matches!(result, Err(PlenoraError::Crs(_))),
                "{op}: CRS geografico accettato"
            );
        }
    }

    #[test]
    fn added_columns_must_not_collide_with_existing_fields() {
        let inputs = [geo_contract(projected_crs())];
        // Nome di output esplicito che collide.
        let result = analyze_one("geo.area", &inputs, &json!({"output_column": "id"}), None);
        assert!(matches!(result, Err(PlenoraError::Schema(_))));

        // Default `wkt` che collide con una colonna esistente.
        let with_wkt = contract_with_geometry(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new(WKT_COLUMN, DataType::Utf8, true),
                geometry_arrow_field(),
            ],
            ContractCrs::Resolved(projected_crs()),
        );
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
            output
                .schema
                .fields()
                .last()
                .expect("ultima colonna")
                .name(),
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
            ("geo.clean_topology", json!({"snap_tolerance": 0.1})),
        ] {
            let inputs = [contract_with_properties()];
            let plan = projected_crs();
            let output = analyze_one(op, &inputs, &config, Some(&plan))
                .unwrap_or_else(|error| panic!("{op}: {error}"));
            assert!(
                output.properties.sorted_by.is_some(),
                "{op}: sorted_by perso"
            );
            assert!(
                output.properties.row_count.is_some(),
                "{op}: row_count perso"
            );
        }
    }

    #[test]
    fn expand_preserves_sort_but_drops_row_count() {
        let inputs = [contract_with_properties()];
        let output = analyze_one("geo.explode", &inputs, &json!({}), None).expect("explode");
        assert!(
            output.properties.sorted_by.is_some(),
            "espansione stabile preserva l'ordine"
        );
        assert!(
            output.properties.row_count.is_none(),
            "righe in uscita non note a secco"
        );
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
    fn reproject_resta_assente_e_nessuna_operazione_chiede_un_backend_nativo() {
        // Rust puro: `reproject` (PROJ) non esiste nel catalogo, e l'analisi
        // la rifiuta come operazione sconosciuta, mai con un'inferenza che
        // poi nessun kernel potrebbe onorare. `make_valid`, `polygonize` e
        // `split`, che a 190c493 dichiaravano la capability `geos`, sono
        // tornate col backend Rust (`crate::rust_backend`) e non dichiarano
        // piu' alcuna capability: l'analisi le accetta come prima.
        let inputs = [geo_contract(projected_crs())];
        assert!(find_operation("geo.reproject").is_none());
        assert!(
            matches!(
                analyze_one("geo.reproject", &inputs, &json!({}), None),
                Err(PlenoraError::Unsupported(_))
            ),
            "reproject: atteso rifiuto Unsupported"
        );
        for op in ["geo.make_valid", "geo.polygonize", "geo.split"] {
            let descriptor = find_operation(op).expect("op in catalogo");
            assert!(descriptor.required_capabilities.is_empty(), "{op}");
            assert_eq!(descriptor.maturity, Maturity::KernelValidated, "{op}");
        }
        analyze_one("geo.make_valid", &inputs, &json!({}), None).expect("make_valid");
        analyze_one("geo.polygonize", &inputs, &json!({}), None).expect("polygonize");
        analyze_one("geo.split", &inputs, &other_wkb_config(), None).expect("split");
        assert!(
            CATALOG
                .iter()
                .all(|descriptor| descriptor.required_capabilities.is_empty()),
            "nessuna operazione del catalogo richiede un backend nativo"
        );
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
        let left =
            attach_schema_metadata(&geo_contract(projected_crs()), &[("shared.key", "alpha")]);
        let right =
            attach_schema_metadata(&geo_contract(projected_crs()), &[("shared.key", "omega")]);
        let result = analyze_one(
            "geo.overlay",
            &[left, right],
            &json!({"mode": "union"}),
            None,
        );
        match result {
            Err(PlenoraError::InvalidPlan(message)) => {
                assert!(
                    message.contains("shared.key"),
                    "l'errore nomina la chiave: {message}"
                );
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
        // Schema ricostruito senza i metadati di schema.
        let contract =
            with_geometry_field_metadata(geo_contract(projected_crs()), false, |metadata| {
                metadata.insert("plenora.geometry.encoding".to_owned(), "wkb".to_owned());
                metadata.insert("driver.native".to_owned(), "kept".to_owned());
            });
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

    // -------------------------------------------------------------------
    // R4.6.3: il requisito di CRS risolvibile e' condizionato alle op che
    // lo usano — il gate vive qui, in analyze (compile-plan).
    // -------------------------------------------------------------------

    /// Campo geometria con la sola estensione `geoarrow.wkb` (colonna
    /// identificabile dal trasporto, piano-v5.md#contratti-di-input decisione 8) e nessun
    /// metadato `geo`: la forma delle fixture senza CRS dichiarato.
    fn extension_only_geometry_field() -> Field {
        Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(HashMap::from([
            (
                GEOARROW_EXTENSION_KEY.to_owned(),
                GEOARROW_WKB_EXTENSION.to_owned(),
            ),
        ]))
    }

    /// Contratto con geometria SENZA CRS dichiarato (`ContractCrs::Missing`).
    fn geo_contract_missing_crs() -> DataContract {
        contract_with_geometry(
            vec![
                Field::new("id", DataType::Int64, false),
                extension_only_geometry_field(),
            ],
            ContractCrs::Missing,
        )
    }

    #[test]
    fn every_geo_op_declares_a_crs_requirement() {
        // Perimetro verificato dal catalogo: NESSUNA op geo e' senza
        // `CrsRequirement` (il gate R4.6.3 ha sempre un requisito da
        // applicare); le op senza requisito sono solo le table.*.
        for descriptor in CATALOG {
            if descriptor.family == Family::Geo {
                assert!(
                    descriptor.crs_requirement.is_some(),
                    "{}: crs_requirement assente",
                    descriptor.id
                );
            }
        }
    }

    #[test]
    fn missing_crs_stops_geo_ops_in_analyze_with_the_declared_cause() {
        // R4.6.3: un contratto con CRS `Missing` ferma OGNI op che dichiara
        // un `CrsRequirement` nel punto in cui tocca la colonna — categoria
        // `Crs` (come ogni requisito non soddisfatto) e messaggio che
        // dichiara la causa, non l'ultimo tentativo di lettura fallito.
        let input = geo_contract_missing_crs();
        let cases: [(&str, Value); 4] = [
            ("geo.buffer", json!({"distance": 1.0})),
            ("geo.area", json!({})),
            ("geo.centroid", json!({})),
            ("geo.explode", json!({})),
        ];
        for (op, config) in &cases {
            let result = analyze_one(op, std::slice::from_ref(&input), config, None);
            match result {
                Err(PlenoraError::Crs(message)) => {
                    assert!(
                        message
                            .contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
                        "{op}: {message}"
                    );
                }
                other => panic!("{op}: atteso errore Crs per CRS mancante, ottenuto {other:?}"),
            }
        }

        // Binaria: il gate scatta sul primo operando senza CRS risolto.
        let result = analyze_one("geo.union", &[input.clone(), input], &json!({}), None);
        match result {
            Err(PlenoraError::Crs(message)) => {
                assert!(
                    message.contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
                    "{message}"
                );
            }
            other => panic!("geo.union: atteso errore Crs per CRS mancante, ottenuto {other:?}"),
        }
    }

    /// Contratto con geometria a CRS dichiarato non risolto
    /// (`ContractCrs::DeclaredUnresolved`, R4.6.3).
    fn geo_contract_declared_unresolved_crs() -> DataContract {
        contract_with_geometry(
            vec![
                Field::new("id", DataType::Int64, false),
                extension_only_geometry_field(),
            ],
            ContractCrs::DeclaredUnresolved {
                crs_id: Some("EPSG:99999".to_owned()),
                definition: None,
                definition_format: None,
            },
        )
    }

    #[test]
    fn declared_unresolved_crs_stops_geo_ops_with_a_distinct_cause() {
        // R4.6.3: come `Missing`, lo stato `DeclaredUnresolved` ferma le op
        // con `CrsRequirement` in analyze con categoria `Crs` — ma il
        // messaggio e' DISTINTO: la colonna DICHIARA un'incoerenza, non
        // un'assenza, e la risoluzione richiede una decisione esplicita nel
        // piano (mai una scelta silenziosa del centro).
        let input = geo_contract_declared_unresolved_crs();
        let cases: [(&str, Value); 2] = [
            ("geo.buffer", json!({"distance": 1.0})),
            ("geo.area", json!({})),
        ];
        for (op, config) in &cases {
            let result = analyze_one(op, std::slice::from_ref(&input), config, None);
            match result {
                Err(PlenoraError::Crs(message)) => {
                    assert!(
                        message.contains("declared_unresolved")
                            && message.contains("decisione esplicita nel piano"),
                        "{op}: {message}"
                    );
                    assert!(
                        !message.contains("nessun CRS dichiarato"),
                        "{op}: il messaggio di `missing` non si applica: {message}"
                    );
                }
                other => panic!(
                    "{op}: atteso errore Crs per CRS declared_unresolved, ottenuto {other:?}"
                ),
            }
        }
    }

    // -----------------------------------------------------------------------
    // piano-v5.md#contratti-di-input decisione 8: le op che riscrivono un fatto canonico (tipo
    // geometrico, CRS) dichiarano il fatto dell'OUTPUT; le chiavi ereditate
    // sono sostituite. Identificabilita' della colonna a compile-plan.
    // -----------------------------------------------------------------------

    /// (op, dichiarazione attesa, lista canonica attesa) per le operazioni
    /// che CAMBIANO il tipo geometrico: i tipi dichiarati sono quelli
    /// dell'OUTPUT, verificati contro i kernel (`transform_output_types`).
    const TYPE_CHANGERS: [(&str, TypesDeclaration, &str); 19] = [
        (
            "geo.from_wkt",
            TypesDeclaration::Mixed,
            "point,linestring,polygon,multipoint,multilinestring,multipolygon,geometrycollection",
        ),
        ("geo.generate_grid", TypesDeclaration::Exact, "polygon"),
        ("geo.centroid", TypesDeclaration::Exact, "point"),
        ("geo.point_on_surface", TypesDeclaration::Exact, "point"),
        (
            "geo.line_interpolate_point",
            TypesDeclaration::Exact,
            "point",
        ),
        ("geo.convex_hull", TypesDeclaration::Exact, "polygon"),
        ("geo.concave_hull", TypesDeclaration::Exact, "polygon"),
        (
            "geo.envelope",
            TypesDeclaration::Exact,
            "point,linestring,polygon",
        ),
        (
            "geo.line_substring",
            TypesDeclaration::Exact,
            "point,linestring",
        ),
        ("geo.buffer", TypesDeclaration::Exact, "multipolygon"),
        (
            "geo.boundary",
            TypesDeclaration::Exact,
            "multipoint,multilinestring,geometrycollection",
        ),
        ("geo.make_valid", TypesDeclaration::Mixed, ""),
        ("geo.voronoi", TypesDeclaration::Exact, "polygon"),
        (
            "geo.clean_topology",
            TypesDeclaration::Exact,
            "polygon,multipolygon",
        ),
        ("geo.clip", TypesDeclaration::Exact, "multipolygon"),
        ("geo.intersection", TypesDeclaration::Exact, "multipolygon"),
        ("geo.union", TypesDeclaration::Exact, "multipolygon"),
        ("geo.difference", TypesDeclaration::Exact, "multipolygon"),
        (
            "geo.symmetric_difference",
            TypesDeclaration::Exact,
            "multipolygon",
        ),
    ];

    fn expected_types_of(op: &str) -> Option<(TypesDeclaration, &'static str)> {
        TYPE_CHANGERS
            .iter()
            .find(|(id, _, _)| *id == op)
            .map(|(_, declaration, list)| (*declaration, *list))
    }

    /// Sweep su OGNI op del catalogo (tabella `cases()`): le op che
    /// cambiano il tipo dichiarano i tipi dell'output, tutte le altre non
    /// inventano una dichiarazione (l'input della fixture e' undeclared).
    #[test]
    fn output_types_declaration_for_every_geo_op() {
        for case in cases() {
            let (output, _, _) = run_case(&case);
            let Some(geometry) = output.active_geometry_column() else {
                continue; // output non geometrico (diagnostiche)
            };
            match expected_types_of(case.op) {
                Some((declaration, list)) => {
                    let types = geometry
                        .types
                        .value()
                        .unwrap_or_else(|| panic!("{}: tipi dell'output non dichiarati", case.op));
                    assert_eq!(
                        types.declaration(),
                        declaration,
                        "{}: dichiarazione",
                        case.op
                    );
                    assert_eq!(
                        types.to_canonical_list(),
                        list,
                        "{}: lista canonica",
                        case.op
                    );
                }
                None => {
                    assert!(
                        geometry.types.value().is_none(),
                        "{}: op a tipo preservato non deve inventare una dichiarazione",
                        case.op
                    );
                }
            }
        }
    }

    /// Contratto con tipi dichiarati (`exact`/`polygon`) e chiavi canoniche
    /// `types`/`types_declaration` sul campo, come prodotto dalla discovery.
    fn geo_contract_with_declared_types() -> DataContract {
        let mut contract = geo_contract(projected_crs());
        contract.geometries[0].types = ContractProperty::new(
            PropertyConfidence::Declared(
                GeometryTypesProperty::new(TypesDeclaration::Exact, vec![GeometryType::Polygon])
                    .expect("coerenza R3.4.1"),
            ),
            PropertyScope::Schema,
        );
        with_geometry_field_metadata(contract, true, |metadata| {
            metadata.insert(
                PLENORA_GEOMETRY_TYPES_DECLARATION_KEY.to_owned(),
                "exact".to_owned(),
            );
            metadata.insert(PLENORA_GEOMETRY_TYPES_KEY.to_owned(), "polygon".to_owned());
        })
    }

    #[test]
    fn type_changers_replace_preexisting_types_type_preservers_keep_it() {
        // Type-changer: la dichiarazione di input (`polygon`) e' SOSTITUITA
        // da quella dell'output e le chiavi ereditate sono rimosse (mai un
        // conflitto R2.6 a valle).
        for case in cases() {
            let Some((declaration, list)) = expected_types_of(case.op) else {
                continue;
            };
            // I produttori accettano per contratto input senza geometria: la
            // riscrittura dei tipi riguarda il loro output, non autorizza un
            // input geometrico che il kernel deve rifiutare.
            let mut inputs = match case.op {
                "geo.from_wkt" => vec![wkt_tabular_contract()],
                "geo.generate_grid" => vec![tabular_contract()],
                _ => vec![geo_contract_with_declared_types()],
            };
            if case.binary {
                inputs.push(geo_contract(projected_crs()));
            }
            let output = analyze_geo_contract(
                case.op,
                &inputs,
                &case.config,
                Some(&projected_crs()),
                &mut FieldAllocator::new(100),
            )
            .unwrap_or_else(|error| panic!("{}: {error}", case.op));
            let geometry = output
                .active_geometry_column()
                .expect("geometria in output");
            let types = geometry.types.value().expect("tipi riscritti");
            assert_eq!(
                types.declaration(),
                declaration,
                "{}: dichiarazione",
                case.op
            );
            assert_eq!(
                types.to_canonical_list(),
                list,
                "{}: la dichiarazione di input non sopravvive",
                case.op
            );
            let field = output
                .schema
                .field_with_name(&geometry.name)
                .expect("campo geometria");
            assert!(
                !field.metadata().contains_key(PLENORA_GEOMETRY_TYPES_KEY)
                    && !field
                        .metadata()
                        .contains_key(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY),
                "{}: chiavi types ereditate rimosse (sostituzione)",
                case.op
            );
        }
        // Tipo preservato: la dichiarazione di input attraversa invariata,
        // chiavi del campo comprese (identity-preserving).
        for op in [
            "geo.simplify",
            "geo.affine_transform",
            "geo.translate",
            "geo.scale",
            "geo.rotate",
            "geo.densify",
            "geo.snap_to_grid",
            "geo.snap",
        ] {
            let case = cases()
                .into_iter()
                .find(|case| case.op == op)
                .unwrap_or_else(|| panic!("{op}: caso in tabella"));
            let output = analyze_geo_contract(
                op,
                &[geo_contract_with_declared_types()],
                &case.config,
                Some(&projected_crs()),
                &mut FieldAllocator::new(100),
            )
            .unwrap_or_else(|error| panic!("{op}: {error}"));
            let geometry = output
                .active_geometry_column()
                .expect("geometria in output");
            let types = geometry.types.value().expect("tipi preservati");
            assert_eq!(
                types.declaration(),
                TypesDeclaration::Exact,
                "{op}: dichiarazione"
            );
            assert_eq!(
                types.to_canonical_list(),
                "polygon",
                "{op}: dichiarazione di input preservata"
            );
            let field = output
                .schema
                .field_with_name(&geometry.name)
                .expect("campo geometria");
            assert_eq!(
                field
                    .metadata()
                    .get(PLENORA_GEOMETRY_TYPES_KEY)
                    .map(String::as_str),
                Some("polygon"),
                "{op}: chiavi ereditate intatte"
            );
        }
    }

    #[test]
    fn unidentifiable_geometry_column_is_rejected_in_analysis() {
        // Minore 2 / architettura.md#geometrie: una colonna geometria che il trasporto non
        // saprebbe identificare (ne' estensione `geoarrow.wkb` ne' chiavi
        // canoniche) e' rifiutata QUI, in analisi del piano — mai scoperta
        // a meta' esecuzione.
        let mut contract = geo_contract(projected_crs());
        contract.schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true),
        ]));
        let result = analyze_one(
            "geo.centroid",
            std::slice::from_ref(&contract),
            &json!({}),
            None,
        );
        match result {
            Err(PlenoraError::Schema(message)) => {
                assert!(
                    message.contains("non identificabile"),
                    "geo.centroid: {message}"
                );
            }
            other => panic!("geo.centroid: atteso rifiuto in analisi, ottenuto {other:?}"),
        }
        // Binaria: il gate scatta anche sul secondo operando.
        let identifiable = geo_contract(projected_crs());
        let result = analyze_one("geo.union", &[identifiable, contract], &json!({}), None);
        assert!(
            matches!(result, Err(PlenoraError::Schema(_))),
            "geo.union: atteso rifiuto in analisi, ottenuto {result:?}"
        );
    }

    #[test]
    fn canonical_only_geometry_column_is_accepted_in_analysis() {
        // Minore 1: la forma a sole chiavi canoniche (estensione ammessa,
        // non richiesta) identifica la colonna.
        let mut contract = geo_contract(projected_crs());
        contract.schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(
                HashMap::from([
                    (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
                    (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
                ]),
            ),
        ]));
        analyze_one(
            "geo.centroid",
            std::slice::from_ref(&contract),
            &json!({}),
            None,
        )
        .expect("forma canonica-only accettata");
    }
}
