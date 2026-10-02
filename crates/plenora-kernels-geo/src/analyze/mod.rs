//! Inferenza a secco dei `DataContract` per le operazioni `geo.*`.
//!
//! [`analyze_geo_contract`] ricava il contratto dell'arco in uscita da id
//! dell'operazione, contratti di input, config JSON e CRS di piano, oppure
//! fallisce **in validazione** (fail-closed), mai a runtime.
//! `required_capabilities` del catalogo non si verifica qui: l'analisi
//! decide solo il contratto.
//!
//! # Config
//!
//! Le struct serde sono locali all'analisi, con nomi e domini dei parametri
//! dei kernel e senza i limiti di trasporto, che non appartengono
//! all'analisi semantica. Gli enum semantici dei kernel sono riusati.
//!
//! # Forme di output
//!
//! Le trasformazioni 1:1 riscrivono la geometria in place con lo stesso
//! `FieldId`; quelle che cambiano tipo dichiarano i tipi dell'output e
//! sostituiscono le chiavi canoniche ereditate. Misure e predicati aggiungono una colonna, le espansioni 1:N
//! aggiungono `__parent_index`, le aggregazioni tengono le sole geometrie, i
//! join aggiungono `__right_index`, i produttori creano una colonna geometria
//! con nuovo `FieldId`. Il dettaglio per operazione sta sulle funzioni di
//! inferenza.
//!
//! Il catalogo marca `Unary` predicati, distanze a due colonne e `split`, ma
//! un input ha una sola colonna geometria: il secondo operando arriva
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
//! la decodifica in XY, quindi un input con `dimensions != Xy` si rifiuta
//! in analisi (una dimensionalita' `Unknown` non vale `Xy`); produttori e output ricodificati dichiarano `Xy`, e
//! le dimensionalita' estese passano solo per le op tabellari.
//!
//! La chiave `encoding` dei metadati `geo` di output si scrive solo se il
//! contratto la dichiara. EWKB senza flag Z/M e senza SRID e' byte-identico a
//! WKB ISO e passa come `xy`; il flag SRID EWKB e' sempre rifiutato dal
//! validatore WKB delle celle ([`crate::geometry_from_wkb`]).
//!
//! # Proprieta' del contratto
//!
//! Le op 1:1 preservano `sorted_by`/`row_count`; le espansioni 1:N
//! preservano `sorted_by` ma eliminano `row_count`; join e aggregazioni
//! eliminano entrambe: una proprieta' che l'operazione non conserva si
//! declassa, mai si eredita.

pub mod config;
mod dispatch;
mod helpers;
mod measures;
mod producers;
mod quality;
mod tipi;

use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Schema};
use plenora_core::catalog::{find_operation, Arity, Family};
use plenora_core::contract::arrow_metadata::PLENORA_GEOMETRY_PRECISION_KEY;
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
/// della colonna geometria che sostituiscono (come nel kernel).
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

/// Rifiuta una colonna geometria che dichiara la prima coordinata nord
/// (`axis_order` `lat_lon` o `northing_easting`).
///
/// Ogni kernel geo legge x come est/longitudine (l'ordine GIS normalizzato
/// del contratto): con gli assi scambiati misure geodetiche, azimut e
/// costruzioni darebbero un risultato sbagliato senza errore. Il contratto
/// Arrow ammette quegli ordini (il vettore `resolved-point` dichiara
/// `lat_lon` per `EPSG:4326`), e le operazioni tabellari li attraversano
/// intatti (ARROW-008); le operazioni geo li rifiutano, finché non ci sarà
/// uno scambio esplicito degli assi.
///
/// # Errors
///
/// `PlenoraError::Crs` per un ordine scambiato; l'errore di lettura della
/// chiave.
fn rifiuta_assi_scambiati(op: &str, inputs: &[DataContract]) -> Result<()> {
    for input in inputs {
        for geometry in &input.geometries {
            let Ok(field) = input.schema.field_with_name(&geometry.name) else {
                continue;
            };
            if matches!(
                plenora_core::contract::arrow_metadata::canonical_geometry_axis_order(field)?,
                Some(
                    plenora_core::contract::AxisOrder::LatLon
                        | plenora_core::contract::AxisOrder::NorthingEasting
                )
            ) {
                return Err(PlenoraError::Crs(format!(
                    "{op}: colonna geometria `{}`: `axis_order` dichiara la prima coordinata \
                     nord, ma i kernel geo leggono x come est/longitudine: l'ordine va \
                     normalizzato a monte, mai in silenzio qui",
                    geometry.name
                )));
            }
        }
    }
    Ok(())
}

/// `analyze_contract` del catalogo per le operazioni `geo.*`: inferenza a
/// secco del contratto di output.
///
/// `plan_crs` e' il CRS di piano gia' risolto dal chiamante (usato dai
/// produttori `from_coords`, `from_wkt`, `generate_grid`; `reproject` risolve
/// il target sempre dalla tabella integrata); `fields` alloca i `FieldId`
/// delle nuove colonne geometriche nel namespace globale del grafo.
///
/// # Errors
///
/// Fallisce (fail-closed, in validazione), senza toccare dati:
///
/// - `PlenoraError::Unsupported` se l'op non e' nel catalogo, non e' geo o e'
///   N-aria, o se la geometria di input non e' `Xy` per un kernel che la
///   elabora;
/// - `PlenoraError::InvalidPlan` se il numero di input non e' l'arieta'
///   dell'op, se la config non supera la deserializzazione stretta o i
///   domini dei parametri (anche un WKB di config non valido), o se lo
///   spazio dei `FieldId` di `fields` e' esaurito;
/// - `PlenoraError::Schema` se un input non ha esattamente una colonna
///   geometria attiva, se la colonna geometria non e' identificabile dal
///   trasporto (ne' estensione `geoarrow.wkb` ne' chiavi canoniche), se una
///   colonna richiesta manca o ha un tipo sbagliato, o se una colonna
///   prodotta collide con una esistente;
/// - `PlenoraError::Crs` se il `crs_requirement` non e' soddisfatto (CRS
///   `Missing` o dichiarato ma non risolto compresi), se il CRS di output non
///   e' risolvibile o la riproiezione non e' ammessa, se una geometria di
///   config esce dal dominio del CRS, o se una colonna geometria di input
///   dichiara gli assi scambiati (`lat_lon`, `northing_easting`);
/// - `PlenoraError::Internal` per un'incoerenza interna dell'analisi, o se
///   la validazione OGC di un WKB di config non conclude.
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
    // `geo.reproject` ha la verifica piu' stretta (ordine GIS normalizzato
    // del CRS sorgente, `richiedi_assi_normalizzati`).
    if descriptor.id != "geo.reproject" {
        rifiuta_assi_scambiati(descriptor.id, inputs)?;
    }
    // I produttori sono unari: l'arieta' e' gia' verificata sopra. Il
    // rifiuto dell'N-aria resta prima del controllo sul numero di input,
    // quindi qui l'arieta' e' solo 1 o 2.
    let output = match (descriptor.id, expected_arity) {
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
        ("geo.reproject", _) => {
            let op = descriptor.id;
            producers::analyze_reproject(op, &inputs[0], config)
        }
        (_, 2) => analyze_binary(descriptor, inputs, config),
        _ => analyze_unary(descriptor, &inputs[0], config, fields),
    }?;
    Ok(senza_precisione_ereditata(output))
}

/// Toglie `plenora.geometry.precision` ereditata dalle colonne geometriche
/// dell'uscita di un'operazione geo.
///
/// Un kernel geo decodifica e ricodifica le coordinate in `f64`: una
/// precisione `float32` o `native` dichiarata dall'ingresso direbbe il
/// falso sulle geometrie che escono. Senza la chiave, l'emissione del blocco
/// canonico dichiara `float64`, la precisione di ogni coordinata WKB. Le
/// operazioni tabellari non toccano le coordinate e la precisione
/// ereditata le attraversa intatta. Per semplicita' vale anche per le
/// operazioni geo che restituiscono la geometria com'era (misure,
/// predicati): l'informazione d'origine si perde, ma la dichiarazione
/// resta vera.
fn senza_precisione_ereditata(mut output: DataContract) -> DataContract {
    let geometrie: Vec<&str> = output
        .geometries
        .iter()
        .map(|geometria| geometria.name.as_str())
        .collect();
    // `float64` ereditata resta: e' gia' vera dopo la ricodifica.
    let da_togliere = |campo: &Field| {
        geometrie.contains(&campo.name().as_str())
            && campo
                .metadata()
                .get(PLENORA_GEOMETRY_PRECISION_KEY)
                .is_some_and(|precisione| precisione != "float64")
    };
    let eredita = output
        .schema
        .fields()
        .iter()
        .any(|campo| da_togliere(campo));
    if !eredita {
        return output;
    }
    let campi: Vec<Field> = output
        .schema
        .fields()
        .iter()
        .map(|campo| {
            if da_togliere(campo) {
                let mut metadati = campo.metadata().clone();
                metadati.remove(PLENORA_GEOMETRY_PRECISION_KEY);
                campo.as_ref().clone().with_metadata(metadati)
            } else {
                campo.as_ref().clone()
            }
        })
        .collect();
    output.schema = Arc::new(Schema::new_with_metadata(
        campi,
        output.schema.metadata().clone(),
    ));
    output
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
        NullPlacement, PropertyConfidence, PropertyScope, SortDirection, SortOrder,
        TypesDeclaration,
    };
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use plenora_core::esadecimale::esadecimale;
    use plenora_core::{ErrorCategory, PlenoraError, Result};
    use serde_json::{json, Value};

    use super::helpers::short_id;
    use super::producers::with_active_geometry_nullability;
    use super::*;
    use crate::arrow_adapter::{
        geo_metadata_json_with_dimensions, DEFAULT_GEOMETRY_COLUMN, GEOARROW_EXTENSION_KEY,
        GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY, PLENORA_GEOMETRY_DIMENSIONS_KEY,
        PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
        PLENORA_GEOMETRY_TYPES_KEY,
    };

    /// Il CRS risolto di un contratto di colonna (i test di analyze lavorano
    /// su CRS risolti; il rifiuto dei CRS `Missing` ha test dedicati).
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

    /// WGS 84 dalla tabella integrata: porta l'ellissoide che le misure
    /// geodetiche chiedono.
    fn geographic_crs() -> ResolvedCrs {
        plenora_core::crs::resolve_crs("EPSG:4326", "crs").expect("CRS integrato")
    }

    /// Un CRS geografico risolto dal chiamante: senza ellissoide.
    fn geographic_crs_without_ellipsoid() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:4326".to_owned(),
            json!({"type": "GeographicCRS", "name": "WGS 84"}),
            CrsKind::Geographic,
            None,
        )
    }

    /// Le misure geodetiche chiedono l'ellissoide del datum: un CRS
    /// geografico risolto dal chiamante, senza ellissoide, si rifiuta in
    /// validazione (`ELLIPSOID_REQUIRED`), mai con un ripiego su WGS 84.
    #[test]
    fn le_misure_geodetiche_rifiutano_un_crs_senza_ellissoide() {
        for (op, config) in [
            ("geo.geodesic_line_length", json!({})),
            ("geo.geodesic_area", json!({})),
            ("geo.geodesic_distance", other_wkb_config()),
            ("geo.haversine_distance", other_wkb_config()),
            ("geo.bearing", other_wkb_config()),
        ] {
            let senza = [geo_contract(geographic_crs_without_ellipsoid())];
            match analyze_one(op, &senza, &config, None) {
                Err(
                    PlenoraError::Crs(messaggio)
                    | PlenoraError::CrsCoded {
                        message: messaggio, ..
                    },
                ) => {
                    assert!(
                        messaggio.contains("ELLIPSOID_REQUIRED"),
                        "{op}: {messaggio}"
                    );
                }
                altro => panic!("{op}: {altro:?}"),
            }
            let con = [geo_contract(geographic_crs())];
            analyze_one(op, &con, &config, None).unwrap_or_else(|e| panic!("{op}: {e}"));
        }
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
        modifica: impl Fn(&mut plenora_core::arrow::Metadata),
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

    /// Una `LineString` valida, per il secondo operando di
    /// `frechet_distance`, che la chiede.
    fn line_wkb_hex() -> String {
        let wkb = Geometry::LineString(geo::LineString::from(vec![(1.0, 2.0), (3.0, 4.0)]))
            .to_wkb(CoordDimensions::xy())
            .expect("encode linea");
        esadecimale(&wkb)
    }

    /// Il secondo operando valido di `op`: una linea per
    /// `frechet_distance`, un punto per gli altri.
    fn other_wkb_hex_for(op: &str) -> String {
        if op == "geo.frechet_distance" {
            line_wkb_hex()
        } else {
            point_wkb_hex()
        }
    }

    fn point_wkb_hex() -> String {
        let wkb = Geometry::Point(Point::new(1.0, 2.0))
            .to_wkb(CoordDimensions::xy())
            .expect("encode punto");
        esadecimale(&wkb)
    }

    /// Replay deterministico dell'invariante «mai panic» del fuzz target
    /// dell'analisi del progetto d'origine (qui non portato), sui soli
    /// parametri WKB esadecimali.
    ///
    /// Non esplora: ripete un elenco scritto a mano sul percorso
    /// `analyze_geo_contract` -> `validate_wkb_hex`, per ogni operazione che
    /// accetta un WKB da configurazione. Ogni caso deve arrivare davvero a
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
            analizza(op, parametro, base, &other_wkb_hex_for(op))
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
    // Tabella dei casi, uno per op geo del catalogo: config minima valida +
    // contratto atteso.
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    enum Expect {
        /// Schema identico all'input (geometria in place, stesso `FieldId`).
        Unchanged,
        /// Colonne dell'input piu' queste in coda (nome, tipo, nullable).
        Appended(Vec<(&'static str, DataType, bool)>),
        /// Solo geometria (nullable) piu' eventuali colonne extra.
        GeometryOnly(Vec<(&'static str, DataType, bool)>),
        /// Come `GeometryOnly`, con la geometria non nullable: una riga per
        /// geometria prodotta (`line_merge`, `polygonize`, `overlay`).
        GeometryOnlyNotNull(Vec<(&'static str, DataType, bool)>),
        /// Come `Appended`, con la geometria non nullable: una riga per
        /// parte o per coppia, mai per una geometria null (espansioni,
        /// `sjoin`, `nearest`).
        AppendedGeometryNotNull(Vec<(&'static str, DataType, bool)>),
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
                json!({ "other_wkb": other_wkb_hex_for(op) }),
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
            unchanged(
                "geo.clean_topology",
                json!({"snap_tolerance": 0.01, "remove_overlaps": true, "fill_gaps": true}),
            ),
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
                Expect::AppendedGeometryNotNull(vec![(
                    PARENT_INDEX_COLUMN,
                    DataType::UInt64,
                    false,
                )]),
            ),
            unary(
                "geo.delaunay",
                json!({}),
                Expect::AppendedGeometryNotNull(vec![(
                    PARENT_INDEX_COLUMN,
                    DataType::UInt64,
                    false,
                )]),
            ),
            unary(
                "geo.split",
                other_wkb_config(),
                Expect::AppendedGeometryNotNull(vec![(
                    PARENT_INDEX_COLUMN,
                    DataType::UInt64,
                    false,
                )]),
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
            unary(
                "geo.line_merge",
                json!({}),
                Expect::GeometryOnlyNotNull(vec![]),
            ),
            unary(
                "geo.collect",
                json!({"group_by": ["id"]}),
                Expect::GeometryOnly(vec![("id", DataType::Int64, false)]),
            ),
            unary(
                "geo.polygonize",
                json!({}),
                Expect::GeometryOnlyNotNull(vec![(CLASS_COLUMN, DataType::Utf8, false)]),
            ),
            // --- Costruzione e riproiezione ---------------------------------
            unary("geo.from_coords", json!({}), Expect::FromCoords),
            unary(
                "geo.reproject",
                json!({"target_crs": "EPSG:32632"}),
                Expect::Reprojected,
            ),
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
                Expect::AppendedGeometryNotNull(vec![(
                    RIGHT_INDEX_COLUMN,
                    DataType::UInt64,
                    false,
                )]),
            ),
            binary(
                "geo.nearest",
                json!({}),
                // Una riga per coppia trovata: indice e distanza ci sono
                // sempre (prima si dichiaravano nullable).
                Expect::AppendedGeometryNotNull(vec![
                    (RIGHT_INDEX_COLUMN, DataType::UInt64, false),
                    (DISTANCE_COLUMN, DataType::Float64, false),
                ]),
            ),
            binary(
                "geo.overlay",
                json!({"mode": "intersection"}),
                Expect::GeometryOnlyNotNull(vec![
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
        if op == "geo.reproject" {
            // La riproiezione chiede un CRS della tabella integrata.
            return builtin_crs("EPSG:4326");
        }
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
        // estesa o non risolta -> rifiuto esplicito in analisi (kernel
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
                Expect::AppendedGeometryNotNull(extra) => {
                    assert_appended(
                        &output,
                        &with_active_geometry_nullability(&input, false)
                            .expect("geometria non nullable"),
                        extra,
                    );
                    assert_geometry_preserved(&output, &input);
                    assert!(
                        !output.active_geometry_column().expect("geometria").nullable,
                        "{}: geometria non nullable",
                        case.op
                    );
                }
                Expect::GeometryOnlyNotNull(extra) => {
                    let mut expected = vec![(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, false)];
                    expected.extend(extra.iter().cloned());
                    assert_eq!(signatures(&output), expected, "{}: schema", case.op);
                    assert!(
                        !output.active_geometry_column().expect("geometria").nullable,
                        "{}: geometria non nullable",
                        case.op
                    );
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
                Expect::Reprojected => {
                    assert_eq!(
                        signatures(&output),
                        signatures(&input),
                        "{}: schema",
                        case.op
                    );
                    let geometry = output
                        .active_geometry_column()
                        .expect("geometria in output");
                    assert_eq!(
                        geometry.field_id,
                        FieldId(2),
                        "{}: FieldId preservato",
                        case.op
                    );
                    assert_eq!(
                        resolved_crs_of(geometry).definition(),
                        "EPSG:32632",
                        "{}: CRS target",
                        case.op
                    );
                    let field = output
                        .schema
                        .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                        .expect("campo geometria");
                    let geo: Value = serde_json::from_str(
                        field
                            .metadata()
                            .get(GEO_METADATA_KEY)
                            .expect("geo metadata"),
                    )
                    .expect("geo JSON");
                    assert_eq!(
                        geo.get("crs").and_then(Value::as_str),
                        Some("EPSG:32632"),
                        "{}: metadato geo.crs aggiornato",
                        case.op
                    );
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
                matches!(
                    result,
                    Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
                ),
                "{op}: CRS geografico accettato"
            );
        }
    }

    /// `geo.clean_topology`: `remove_overlaps` e `fill_gaps` non hanno un
    /// valore predefinito (prima il runner usava `true`): un piano che non
    /// li scrive si rifiuta in validazione.
    #[test]
    fn clean_topology_chiede_remove_overlaps_e_fill_gaps() {
        let inputs = [geo_contract(projected_crs())];
        for (config, manca) in [
            (json!({"snap_tolerance": 0.1}), "remove_overlaps"),
            (
                json!({"snap_tolerance": 0.1, "fill_gaps": false}),
                "remove_overlaps",
            ),
            (
                json!({"snap_tolerance": 0.1, "remove_overlaps": false}),
                "fill_gaps",
            ),
        ] {
            match analyze_one("geo.clean_topology", &inputs, &config, None) {
                Err(PlenoraError::InvalidPlan(messaggio)) => {
                    assert!(
                        messaggio.contains(&format!("missing field `{manca}`")),
                        "{config}: {messaggio}"
                    );
                }
                altro => panic!("{config}: {altro:?}"),
            }
        }
        analyze_one(
            "geo.clean_topology",
            &inputs,
            &json!({"snap_tolerance": 0.1, "remove_overlaps": false, "fill_gaps": false}),
            None,
        )
        .expect("config completa");
    }

    /// `geo.simplify`: la soglia ha un nome per algoritmo. `tolerance` con
    /// `preserve_topology` (dove la soglia e' un'area) si rifiuta con un
    /// messaggio che nomina `min_area`, invece di reinterpretarla.
    #[test]
    fn simplify_rifiuta_la_soglia_dell_altro_algoritmo() {
        let inputs = [geo_contract(projected_crs())];
        for (config, atteso) in [
            (
                json!({"tolerance": 1.0, "policy": "preserve_topology"}),
                "`tolerance` non vale con `policy: preserve_topology`",
            ),
            (
                json!({"policy": "preserve_topology"}),
                "`min_area` obbligatorio",
            ),
            (json!({"min_area": 1.0}), "`min_area` vale solo"),
            (
                json!({"min_area": 1.0, "policy": "douglas_peucker"}),
                "`min_area` vale solo",
            ),
            (json!({}), "`tolerance` obbligatorio"),
            (
                json!({"min_area": f64::MAX, "policy": "preserve_topology", "tolerance": 1.0}),
                "`tolerance` non vale",
            ),
            (
                json!({"min_area": -1.0, "policy": "preserve_topology"}),
                "parametro `min_area` non valido",
            ),
        ] {
            match analyze_one("geo.simplify", &inputs, &config, None) {
                Err(PlenoraError::InvalidPlan(messaggio)) => {
                    assert!(messaggio.contains(atteso), "{config}: {messaggio}");
                }
                altro => panic!("{config}: {altro:?}"),
            }
        }
        for config in [
            json!({"tolerance": 1.0}),
            json!({"tolerance": 1.0, "policy": "douglas_peucker"}),
            json!({"min_area": 1.0, "policy": "preserve_topology"}),
        ] {
            analyze_one("geo.simplify", &inputs, &config, None)
                .unwrap_or_else(|e| panic!("{config}: {e}"));
        }
    }

    #[test]
    fn geographic_requirement_rejects_projected_input() {
        for op in ["geo.geodesic_area", "geo.geodesic_line_length"] {
            let inputs = [geo_contract(projected_crs())];
            let result = analyze_one(op, &inputs, &json!({}), None);
            assert!(
                matches!(
                    result,
                    Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
                ),
                "{op}: CRS proiettato accettato"
            );
        }
        // Anche le distanze geodetiche "unary" con other_wkb.
        let inputs = [geo_contract(projected_crs())];
        let result = analyze_one("geo.haversine_distance", &inputs, &other_wkb_config(), None);
        assert!(matches!(
            result,
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ));
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
            matches!(
                result,
                Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
            ),
            "CRS diversi accettati"
        );

        let geographic_right = [
            geo_contract(projected_crs()),
            geo_contract(geographic_crs()),
        ];
        let result = analyze_one("geo.sjoin", &geographic_right, &config, None);
        assert!(
            matches!(
                result,
                Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
            ),
            "right geografico accettato"
        );

        // Variante unaria (other_wkb): il CRS dell'input deve essere proiettato.
        let inputs = [geo_contract(geographic_crs())];
        let result = analyze_one("geo.distance", &inputs, &other_wkb_config(), None);
        assert!(matches!(
            result,
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ));
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
            matches!(
                result,
                Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
            ),
            "CRS geografico accettato"
        );

        // Senza config `crs` ne' CRS di piano: obbligatorio.
        let result = analyze_one("geo.from_coords", &inputs, &json!({}), None);
        assert!(matches!(
            result,
            Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
        ));

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
            matches!(
                result,
                Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
            ),
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
            matches!(
                result,
                Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
            ),
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
            matches!(
                result,
                Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
            ),
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
                matches!(
                    result,
                    Err(PlenoraError::Crs(_) | PlenoraError::CrsCoded { .. })
                ),
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
                PropertyConfidence::Proven(SortOrder {
                    keys: vec![FieldId(0)],
                    direction: SortDirection::Ascending,
                    nulls: NullPlacement::Last,
                }),
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
            (
                "geo.clean_topology",
                json!({"snap_tolerance": 0.1, "remove_overlaps": true, "fill_gaps": true}),
            ),
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

    fn builtin_crs(definizione: &str) -> ResolvedCrs {
        plenora_core::crs::resolve_crs(definizione, "crs").expect("CRS integrato")
    }

    #[test]
    fn nessuna_operazione_chiede_un_backend_nativo() {
        // Rust puro: `make_valid`, `polygonize` e `split` (nel progetto
        // d'origine dietro la capability `geos`) e `reproject` (dietro
        // `proj`) sono tornate con i backend Rust (`crate::rust_backend`,
        // `crate::riproiezione`) e non dichiarano alcuna capability.
        let inputs = [geo_contract(projected_crs())];
        for op in [
            "geo.make_valid",
            "geo.polygonize",
            "geo.split",
            "geo.reproject",
        ] {
            let descriptor = find_operation(op).expect("op in catalogo");
            assert!(descriptor.required_capabilities.is_empty(), "{op}");
            assert_eq!(descriptor.maturity, Maturity::KernelValidated, "{op}");
        }
        analyze_one("geo.make_valid", &inputs, &json!({}), None).expect("make_valid");
        analyze_one("geo.polygonize", &inputs, &json!({}), None).expect("polygonize");
        analyze_one("geo.split", &inputs, &other_wkb_config(), None).expect("split");
        let integrato = [geo_contract(builtin_crs("EPSG:32632"))];
        analyze_one(
            "geo.reproject",
            &integrato,
            &json!({"target_crs": "EPSG:3857"}),
            None,
        )
        .expect("reproject");
        assert!(
            CATALOG
                .iter()
                .all(|descriptor| descriptor.required_capabilities.is_empty()),
            "nessuna operazione del catalogo richiede un backend nativo"
        );
    }

    #[test]
    fn reproject_rifiuta_i_crs_fuori_tabella_e_le_config_sbagliate() {
        // Un CRS risolto dal chiamante (non della tabella) non si riproietta.
        let esterno = [geo_contract(projected_crs())];
        match analyze_one(
            "geo.reproject",
            &esterno,
            &json!({"target_crs": "EPSG:3857"}),
            None,
        ) {
            Err(PlenoraError::Crs(message) | PlenoraError::CrsCoded { message, .. }) => {
                assert!(message.contains("CRS_NOT_BUILTIN"), "{message}");
            }
            other => panic!("atteso CRS_NOT_BUILTIN: {other:?}"),
        }
        let inputs = [geo_contract(builtin_crs("EPSG:3003"))];
        // Regola dell'accuratezza: Gauss-Boaga -> RDN2008 senza accuratezza
        // accettata si rifiuta nell'analisi, con l'accuratezza del percorso.
        match analyze_one(
            "geo.reproject",
            &inputs,
            &json!({"target_crs": "EPSG:7791"}),
            None,
        ) {
            Err(PlenoraError::Crs(message) | PlenoraError::CrsCoded { message, .. }) => {
                assert!(
                    message.contains("REPROJECTION_ACCURACY_NOT_ACCEPTED")
                        && message.contains("4 m"),
                    "{message}"
                );
            }
            other => panic!("atteso il rifiuto dell'accuratezza: {other:?}"),
        }
        analyze_one(
            "geo.reproject",
            &inputs,
            &json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 4.0}),
            None,
        )
        .expect("accuratezza accettata");
        for config in [
            json!({}),
            json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 4.0, "extra": 1}),
            json!({"target_crs": "WGS 84"}),
            json!({"target_crs": "EPSG:4490", "accuratezza_accettata_m": 100.0}),
            json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 4.0,
                   "griglie": [{"trasformazione": 1659, "file": "x.gsb"}]}),
        ] {
            assert!(
                analyze_one("geo.reproject", &inputs, &config, None).is_err(),
                "{config}"
            );
        }
        // Le griglie si verificano nella forma; il file si legge in
        // esecuzione.
        analyze_one(
            "geo.reproject",
            &inputs,
            &json!({"target_crs": "EPSG:7791", "accuratezza_accettata_m": 0.1,
                    "griglie": [{"trasformazione": 9734, "file": "IGM/R40_F00.gsb"}]}),
            None,
        )
        .expect("griglia IGM dichiarata");
    }

    #[test]
    fn reproject_preserva_le_proprieta_riga_per_riga() {
        let mut contract = contract_with_properties();
        contract.geometries[0].crs = ContractCrs::Resolved(builtin_crs("EPSG:32632"));
        let output = analyze_one(
            "geo.reproject",
            &[contract.clone()],
            &json!({"target_crs": "EPSG:3857"}),
            None,
        )
        .expect("reproject");
        assert_eq!(output.properties, contract.properties);
    }

    // -----------------------------------------------------------------------
    // Lineage dei metadati Arrow: cio' che l'op non tocca resta invariato.
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
                .collect::<plenora_core::arrow::Metadata>(),
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
        // Lineage identity-preserving sul campo geometria che sopravvive
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
    // Il requisito di CRS risolvibile e' condizionato alle op che lo
    // usano: il rifiuto vive qui, in analisi.
    // -------------------------------------------------------------------

    /// Campo geometria con la sola estensione `geoarrow.wkb` (colonna
    /// identificabile dal trasporto) e nessun
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
        // `CrsRequirement` (il rifiuto dei CRS non risolti ha sempre un requisito da
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
        // Un contratto con CRS `Missing` ferma OGNI op che dichiara
        // un `CrsRequirement` nel punto in cui tocca la colonna — categoria
        // `Crs` (come ogni requisito non soddisfatto) e messaggio che
        // dichiara la causa, non l'ultimo tentativo di lettura fallito.
        let input = geo_contract_missing_crs();
        let cases: [(&str, Value); 5] = [
            ("geo.buffer", json!({"distance": 1.0})),
            ("geo.reproject", json!({"target_crs": "EPSG:3857"})),
            ("geo.area", json!({})),
            ("geo.centroid", json!({})),
            ("geo.explode", json!({})),
        ];
        for (op, config) in &cases {
            let result = analyze_one(op, std::slice::from_ref(&input), config, None);
            match result {
                Err(PlenoraError::Crs(message) | PlenoraError::CrsCoded { message, .. }) => {
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
            Err(PlenoraError::Crs(message) | PlenoraError::CrsCoded { message, .. }) => {
                assert!(
                    message.contains("nessun CRS dichiarato in alcuna rappresentazione accettata"),
                    "{message}"
                );
            }
            other => panic!("geo.union: atteso errore Crs per CRS mancante, ottenuto {other:?}"),
        }
    }

    /// Contratto con geometria a CRS dichiarato non risolto
    /// (`ContractCrs::DeclaredUnresolved`).
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
        // Come `Missing`, lo stato `DeclaredUnresolved` ferma le op
        // con `CrsRequirement` in analyze con categoria `Crs` — ma il
        // messaggio e' DISTINTO: la colonna DICHIARA un'incoerenza, non
        // un'assenza, e la risoluzione richiede una decisione esplicita nel
        // piano (mai una scelta silenziosa del centro).
        let input = geo_contract_declared_unresolved_crs();
        let cases: [(&str, Value); 3] = [
            ("geo.buffer", json!({"distance": 1.0})),
            ("geo.reproject", json!({"target_crs": "EPSG:3857"})),
            ("geo.area", json!({})),
        ];
        for (op, config) in &cases {
            let result = analyze_one(op, std::slice::from_ref(&input), config, None);
            match result {
                Err(PlenoraError::Crs(message) | PlenoraError::CrsCoded { message, .. }) => {
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
    // Le op che riscrivono un fatto canonico (tipo geometrico, CRS)
    // dichiarano il fatto dell'OUTPUT; le chiavi ereditate sono sostituite.
    // Identificabilita' della colonna in analisi.
    // -----------------------------------------------------------------------

    /// (op, dichiarazione attesa, lista canonica attesa) per le operazioni
    /// che CAMBIANO il tipo geometrico: i tipi dichiarati sono quelli
    /// dell'OUTPUT, verificati contro i kernel (`transform_output_types`).
    const TYPE_CHANGERS: [(&str, TypesDeclaration, &str); 25] = [
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
        // Aggregazioni e ricostruzioni (`analyze::tipi`), verificate contro
        // i kernel da `kernel_crosscheck`.
        (
            "geo.polygonize",
            TypesDeclaration::Exact,
            "linestring,polygon",
        ),
        ("geo.dissolve", TypesDeclaration::Exact, "multipolygon"),
        (
            "geo.overlay",
            TypesDeclaration::Exact,
            "polygon,multipolygon",
        ),
        ("geo.delaunay", TypesDeclaration::Exact, "polygon"),
        ("geo.line_merge", TypesDeclaration::Exact, "linestring"),
        // Con ingresso non dichiarato; con `[polygon]` dichiarato, `polygon`
        // (vedi sotto).
        ("geo.split", TypesDeclaration::Exact, "linestring,polygon"),
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
                    .expect("dichiarazione exact con tipi"),
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
    // Due tabelle (riscrittori e conservatori) in sequenza: lunghezza
    // intrinseca.
    #[allow(clippy::too_many_lines)]
    fn type_changers_replace_preexisting_types_type_preservers_keep_it() {
        // Type-changer: la dichiarazione di input (`polygon`) e' SOSTITUITA
        // da quella dell'output e le chiavi ereditate sono rimosse (mai un
        // conflitto con la chiave ereditata a valle).
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
            // `split` mappa i tipi dichiarati: da `[polygon]` solo poligoni.
            let list = if case.op == "geo.split" {
                "polygon"
            } else {
                list
            };
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
        // Una colonna geometria che il trasporto non
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
        // La forma a sole chiavi canoniche (estensione ammessa,
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

    // -----------------------------------------------------------------------
    // Oracolo analisi/kernel del catalogo geo (l'equivalente di
    // `kernel_crosscheck` di `plenora-kernels-table`).
    //
    // - Schema: per le operazioni con un kernel Arrow in questo workspace
    //   (`polygonize`, `split`) lo schema del contratto dedotto a secco
    //   contro quello del batch prodotto: nomi, tipi, nullability, metadati
    //   di campo e di schema; il contratto riletto dallo schema del kernel
    //   dichiara CRS, dimensioni ed encoding dell'analisi; nessun null dove il
    //   contratto dice non-null.
    // - Tipi: per le operazioni che spezzano, raccolgono o ricostruiscono le
    //   geometrie (`analyze::tipi`), ogni geometria prodotta dal kernel su
    //   ingressi dei tipi dichiarati ha un tipo che il contratto dichiara.
    // -----------------------------------------------------------------------

    mod kernel_crosscheck {
        use std::collections::{BTreeMap, BTreeSet};

        use geo::{LineString, MultiLineString, MultiPoint, MultiPolygon, Polygon};
        use plenora_core::arrow::array::{
            Array, BinaryArray, Int64Array, RecordBatch, StringArray,
        };
        use plenora_core::arrow::SchemaRef;
        use plenora_core::contract::arrow_schema::{
            arrow_schema_from_contract, contract_from_arrow_schema,
        };
        use plenora_core::contract::{ContractProperty, GeometryEncoding};

        use super::*;
        use crate::arrow_adapter::PLENORA_GEOMETRY_TYPES_DECLARATION_KEY;
        use crate::riproiezione::{reproject_batches, ReprojectParams};
        use crate::rust_backend::arrow::{
            make_valid_batches, polygonize_batches, split_batches, PolygonizeParams,
        };
        use crate::rust_backend::precision::Precision;

        type Firma = (String, DataType, bool, BTreeMap<String, String>);

        fn firma(schema: &Schema) -> (Vec<Firma>, BTreeMap<String, String>) {
            (
                schema
                    .fields()
                    .iter()
                    .map(|field| {
                        (
                            field.name().clone(),
                            field.data_type().clone(),
                            field.is_nullable(),
                            field
                                .metadata()
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        )
                    })
                    .collect(),
                schema
                    .metadata()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        }

        fn esatti(tipi: &[GeometryType]) -> ContractProperty<GeometryTypesProperty> {
            ContractProperty::new(
                PropertyConfidence::Declared(
                    GeometryTypesProperty::new(TypesDeclaration::Exact, tipi.to_vec())
                        .expect("tipi"),
                ),
                PropertyScope::Schema,
            )
        }

        /// Il contratto d'ingresso come lo restituisce la scoperta da uno
        /// schema Arrow:
        /// lo schema porta le chiavi canoniche emesse dal contratto (tipi
        /// compresi), piu' un metadato di lineage sul campo geometria e su
        /// un attributo, e un metadato di schema.
        fn ingresso(tipi: &[GeometryType], geometria_nullable: bool) -> DataContract {
            ingresso_con_crs(projected_crs(), tipi, geometria_nullable)
        }

        /// Come [`ingresso`], con il CRS dato (la riproiezione chiede un CRS
        /// della tabella integrata, che dichiara anche l'ordine degli assi).
        fn ingresso_con_crs(
            crs: ResolvedCrs,
            tipi: &[GeometryType],
            geometria_nullable: bool,
        ) -> DataContract {
            let mut contract = geo_contract(crs);
            contract.geometries[0].encoding = Some(GeometryEncoding::Wkb);
            contract.geometries[0].types = esatti(tipi);
            contract.geometries[0].nullable = geometria_nullable;
            let emesso = arrow_schema_from_contract(&contract).expect("schema emesso");
            let fields: Vec<Field> = emesso
                .fields()
                .iter()
                .map(|field| {
                    let mut metadata = field.metadata().clone();
                    let mut nullable = field.is_nullable();
                    if field.name() == DEFAULT_GEOMETRY_COLUMN {
                        nullable = geometria_nullable;
                        metadata.insert("descrizione".to_owned(), "lineage".to_owned());
                    }
                    if field.name() == "label" {
                        metadata.insert("descrizione".to_owned(), "lineage".to_owned());
                    }
                    Field::new(field.name(), field.data_type().clone(), nullable)
                        .with_metadata(metadata)
                })
                .collect();
            let mut schema_metadata = emesso.metadata().clone();
            schema_metadata.insert("dataset".to_owned(), "prova".to_owned());
            let schema = Arc::new(Schema::new_with_metadata(fields, schema_metadata));
            // La scoperta, come per un file d'ingresso vero.
            let scoperto = contract_from_arrow_schema(schema, plenora_core::crs::resolve_crs)
                .expect("scoperta dell'ingresso");
            assert!(
                scoperto
                    .schema
                    .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                    .unwrap()
                    .metadata()
                    .contains_key(PLENORA_GEOMETRY_TYPES_KEY),
                "l'ingresso dichiara i tipi"
            );
            assert_eq!(scoperto.geometries[0].nullable, geometria_nullable);
            scoperto
        }

        fn wkb(geometry: &Geometry<f64>) -> Vec<u8> {
            geometry.to_wkb(CoordDimensions::xy()).expect("wkb")
        }

        fn batch(schema: &SchemaRef, celle: &[Option<Vec<u8>>]) -> RecordBatch {
            let righe = celle.len();
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(
                        (0..righe)
                            .map(|i| i64::try_from(i).expect("riga"))
                            .collect::<Vec<_>>(),
                    )),
                    Arc::new(StringArray::from(
                        (0..righe).map(|_| Some("a")).collect::<Vec<_>>(),
                    )),
                    Arc::new(celle.iter().map(Option::as_deref).collect::<BinaryArray>()),
                ],
            )
            .expect("batch")
        }

        fn anello(punti: &[(f64, f64)]) -> LineString<f64> {
            LineString::from(punti.to_vec())
        }

        fn quadrato(x: f64, y: f64, lato: f64) -> Polygon<f64> {
            Polygon::new(
                anello(&[
                    (x, y),
                    (x + lato, y),
                    (x + lato, y + lato),
                    (x, y + lato),
                    (x, y),
                ]),
                vec![],
            )
        }

        /// Un esemplare per tipo, abbastanza denso da essere spezzato da
        /// `subdivide` con 8 vertici.
        fn esemplare(tipo: GeometryType) -> Geometry<f64> {
            let zigzag = |y: f64| {
                anello(
                    &(0..10_u8)
                        .map(|i| (f64::from(i), y + f64::from(i % 2) * 0.5))
                        .collect::<Vec<_>>(),
                )
            };
            let linea = zigzag(0.0);
            let poligono = Polygon::new(
                anello(&[
                    (0.0, 0.0),
                    (2.0, -0.5),
                    (4.0, 0.0),
                    (4.5, 2.0),
                    (4.0, 4.0),
                    (2.0, 4.5),
                    (0.0, 4.0),
                    (-0.5, 2.0),
                    (0.0, 0.0),
                ]),
                vec![],
            );
            match tipo {
                GeometryType::Point => Geometry::Point(Point::new(1.0, 2.0)),
                GeometryType::LineString => Geometry::LineString(linea),
                GeometryType::Polygon => Geometry::Polygon(poligono),
                GeometryType::MultiPoint => Geometry::MultiPoint(MultiPoint::from(
                    (0..10_u8)
                        .map(|i| (f64::from(i), f64::from(i * i % 7)))
                        .collect::<Vec<_>>(),
                )),
                GeometryType::MultiLineString => {
                    Geometry::MultiLineString(MultiLineString::new(vec![linea, zigzag(5.0)]))
                }
                GeometryType::MultiPolygon => Geometry::MultiPolygon(MultiPolygon::new(vec![
                    poligono,
                    quadrato(10.0, 10.0, 1.0),
                ])),
                GeometryType::GeometryCollection => Geometry::GeometryCollection(
                    vec![
                        Geometry::Point(Point::new(9.0, 9.0)),
                        Geometry::Polygon(quadrato(20.0, 20.0, 1.0)),
                    ]
                    .into(),
                ),
                other => panic!("tipo senza esemplare: {other:?}"),
            }
        }

        fn tipo_di(geometry: &Geometry<f64>) -> GeometryType {
            match geometry {
                Geometry::Point(_) => GeometryType::Point,
                Geometry::LineString(_) | Geometry::Line(_) => GeometryType::LineString,
                Geometry::Polygon(_) | Geometry::Rect(_) | Geometry::Triangle(_) => {
                    GeometryType::Polygon
                }
                Geometry::MultiPoint(_) => GeometryType::MultiPoint,
                Geometry::MultiLineString(_) => GeometryType::MultiLineString,
                Geometry::MultiPolygon(_) => GeometryType::MultiPolygon,
                Geometry::GeometryCollection(_) => GeometryType::GeometryCollection,
            }
        }

        fn centimetro() -> Precision {
            Precision::new(0.01).expect("precisione")
        }

        /// I tipi che l'analisi dichiara in uscita per un ingresso di tipi
        /// `tipi` (exact).
        fn dichiarati(op: &str, tipi: &[GeometryType], config: &Value) -> BTreeSet<GeometryType> {
            let mut contract = geo_contract(projected_crs());
            contract.geometries[0].types = esatti(tipi);
            let output = analyze_one(op, std::slice::from_ref(&contract), config, None)
                .unwrap_or_else(|errore| panic!("analisi {op}: {errore}"));
            let dichiarazione = output.geometries[0]
                .types
                .value()
                .unwrap_or_else(|| panic!("{op}: nessuna dichiarazione dei tipi"));
            assert_eq!(dichiarazione.declaration(), TypesDeclaration::Exact, "{op}");
            dichiarazione.types().iter().copied().collect()
        }

        fn contenuti(op: &str, tipi: &[GeometryType], prodotti: &[Geometry<f64>], config: &Value) {
            let ammessi = dichiarati(op, tipi, config);
            for prodotta in prodotti {
                assert!(
                    ammessi.contains(&tipo_di(prodotta)),
                    "{op} su {tipi:?}: prodotto {:?}, dichiarati {ammessi:?}",
                    tipo_di(prodotta)
                );
            }
        }

        /// Lo schema del kernel contro quello dell'analisi (vedi il blocco).
        fn confronta(op: &str, analizzato: &DataContract, kernel: &SchemaRef, batch: &RecordBatch) {
            assert_eq!(
                firma(&analizzato.schema),
                firma(kernel),
                "{op}: schema dell'analisi diverso da quello del kernel"
            );
            let riletto =
                contract_from_arrow_schema(kernel.clone(), plenora_core::crs::resolve_crs)
                    .expect("contratto riletto");
            assert_eq!(
                riletto.geometries.len(),
                analizzato.geometries.len(),
                "{op}"
            );
            for (letta, dedotta) in riletto.geometries.iter().zip(&analizzato.geometries) {
                assert_eq!(letta.name, dedotta.name, "{op}");
                assert_eq!(
                    letta.crs.as_resolved().map(ResolvedCrs::definition),
                    dedotta.crs.as_resolved().map(ResolvedCrs::definition),
                    "{op}: CRS"
                );
                assert_eq!(letta.dimensions, dedotta.dimensions, "{op}: dimensioni");
                assert_eq!(letta.encoding, dedotta.encoding, "{op}: encoding");
                assert_eq!(letta.nullable, dedotta.nullable, "{op}: nullable");
            }
            // Lo schema pubblicato (blocco canonico dal contratto) si
            // costruisce sui metadati del kernel senza chiavi in conflitto.
            let mut pubblicato = analizzato.clone();
            pubblicato.schema = kernel.clone();
            arrow_schema_from_contract(&pubblicato)
                .unwrap_or_else(|errore| panic!("{op}: schema pubblicato: {errore}"));
            for (indice, campo) in kernel.fields().iter().enumerate() {
                if !campo.is_nullable() {
                    assert_eq!(
                        batch.column(indice).null_count(),
                        0,
                        "{op}: {}",
                        campo.name()
                    );
                }
            }
        }

        fn geometrie_della_colonna(batch: &RecordBatch) -> Vec<Geometry<f64>> {
            let indice = batch.schema().index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
            let celle = batch
                .column(indice)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            (0..celle.len())
                .filter(|riga| !celle.is_null(*riga))
                .map(|riga| crate::geometry_from_wkb(celle.value(riga)).unwrap())
                .collect()
        }

        /// `make_valid` riscrive i tipi (`mixed`): il campo dell'uscita non
        /// porta piu' la dichiarazione dell'ingresso, il resto invariato.
        #[test]
        fn make_valid_come_l_analisi() {
            let contract = ingresso(&[GeometryType::Polygon], true);
            let analizzato = analyze_one(
                "geo.make_valid",
                std::slice::from_ref(&contract),
                &json!({}),
                None,
            )
            .expect("analisi");
            let farfalla = Geometry::Polygon(Polygon::new(
                anello(&[(0.0, 0.0), (4.0, 4.0), (4.0, 0.0), (0.0, 4.0), (0.0, 0.0)]),
                vec![],
            ));
            let input = batch(
                &contract.schema,
                &[
                    Some(wkb(&Geometry::Polygon(quadrato(0.0, 0.0, 4.0)))),
                    None,
                    Some(wkb(&farfalla)),
                ],
            );
            let batches = make_valid_batches(
                &contract.schema,
                &[input],
                DEFAULT_GEOMETRY_COLUMN,
                centimetro(),
            )
            .expect("kernel");
            let schema = batches[0].schema();
            confronta("geo.make_valid", &analizzato, &schema, &batches[0]);
        }

        /// `reproject` riscrive CRS e ordine degli assi del campo, conserva
        /// i tipi dichiarati, le lineage e i metadati di schema.
        #[test]
        fn reproject_come_l_analisi() {
            for tipi in [
                &[GeometryType::Polygon][..],
                &[GeometryType::LineString][..],
            ] {
                for target in ["EPSG:4326", "EPSG:3857", "EPSG:25832"] {
                    let contract = ingresso_con_crs(builtin_crs("EPSG:32632"), tipi, true);
                    // Verso ETRS89 nessuna accuratezza da accettare: WGS 84 ed
                    // ETRS89 sono equivalenti per convenzione.
                    let config = json!({"target_crs": target});
                    let analizzato = analyze_one(
                        "geo.reproject",
                        std::slice::from_ref(&contract),
                        &config,
                        None,
                    )
                    .expect("analisi");
                    let sorgente = contract.geometries[0]
                        .crs
                        .as_resolved()
                        .expect("CRS")
                        .clone();
                    let params = ReprojectParams::da_config("geo.reproject", &config, &sorgente)
                        .expect("config");
                    let esemplare = match tipi[0] {
                        GeometryType::Polygon => {
                            Geometry::Polygon(quadrato(500_000.0, 5_000_000.0, 1_000.0))
                        }
                        _ => Geometry::LineString(anello(&[
                            (400_000.0, 4_900_000.0),
                            (600_000.0, 5_100_000.0),
                        ])),
                    };
                    let input = batch(&contract.schema, &[Some(wkb(&esemplare)), None]);
                    let (schema, batches) = reproject_batches(
                        &contract.schema,
                        &[input],
                        DEFAULT_GEOMETRY_COLUMN,
                        &sorgente,
                        &params,
                    )
                    .expect("kernel");
                    confronta("geo.reproject", &analizzato, &schema, &batches[0]);
                    for prodotta in geometrie_della_colonna(&batches[0]) {
                        assert!(tipi.contains(&tipo_di(&prodotta)), "{target}: tipo");
                    }
                }
            }
        }

        #[test]
        fn polygonize_come_l_analisi() {
            let contract = ingresso(&[GeometryType::LineString], true);
            let analizzato = analyze_one(
                "geo.polygonize",
                std::slice::from_ref(&contract),
                &json!({}),
                None,
            )
            .expect("analisi");
            // Un quadrato chiuso e un dangle: un poligono e un residuo.
            let quadrato = Geometry::LineString(anello(&[
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ]));
            let coda = Geometry::LineString(anello(&[(4.0, 4.0), (6.0, 6.0)]));
            let input = batch(
                &contract.schema,
                &[Some(wkb(&quadrato)), None, Some(wkb(&coda))],
            );
            let (schema, batches) = polygonize_batches(
                &contract.schema,
                &[input],
                DEFAULT_GEOMETRY_COLUMN,
                PolygonizeParams::default(),
                100,
                centimetro(),
            )
            .expect("kernel");
            assert_eq!(batches[0].schema(), schema);
            confronta("geo.polygonize", &analizzato, &schema, &batches[0]);
            let prodotte = geometrie_della_colonna(&batches[0]);
            assert_eq!(prodotte.len(), 2);
            contenuti(
                "geo.polygonize",
                &[GeometryType::LineString],
                &prodotte,
                &json!({}),
            );
        }

        #[test]
        fn split_come_l_analisi() {
            // Nel dominio di EPSG:32632 (l'analisi controlla la lama).
            let nel_fuso = |g: Geometry<f64>| geo::Translate::translate(&g, 500_000.0, 4_000_000.0);
            let lama = nel_fuso(Geometry::LineString(anello(&[(2.0, -1.0), (2.0, 5.0)])));
            let config = json!({ "other_wkb": esadecimale(&wkb(&lama)) });
            let sorgenti = [
                Geometry::Polygon(quadrato(0.0, 0.0, 4.0)),
                Geometry::MultiPolygon(MultiPolygon::new(vec![
                    quadrato(0.0, 0.0, 1.0),
                    quadrato(3.0, 0.0, 1.0),
                ])),
                Geometry::LineString(anello(&[(0.0, 2.0), (4.0, 2.0)])),
            ]
            .map(nel_fuso);
            for nullable in [true, false] {
                let tipi = [
                    GeometryType::LineString,
                    GeometryType::Polygon,
                    GeometryType::MultiPolygon,
                ];
                let contract = ingresso(&tipi, nullable);
                let analizzato =
                    analyze_one("geo.split", std::slice::from_ref(&contract), &config, None)
                        .expect("analisi");
                let celle: Vec<Option<Vec<u8>>> = sorgenti.iter().map(|g| Some(wkb(g))).collect();
                let input = batch(&contract.schema, &celle);
                let taglienti: BinaryArray = celle
                    .iter()
                    .map(|_| Some(wkb(&lama)))
                    .collect::<Vec<_>>()
                    .iter()
                    .map(Option::as_deref)
                    .collect();
                let (schema, batches) = split_batches(
                    &contract.schema,
                    &[input],
                    DEFAULT_GEOMETRY_COLUMN,
                    &taglienti,
                    None,
                    100,
                    centimetro(),
                )
                .expect("kernel");
                assert_eq!(batches[0].schema(), schema);
                confronta("geo.split", &analizzato, &schema, &batches[0]);
                let prodotte = geometrie_della_colonna(&batches[0]);
                assert!(prodotte.len() >= 4, "{}", prodotte.len());
                contenuti("geo.split", &tipi, &prodotte, &config);
                // Solo poligoni dichiarati: solo `Polygon` in uscita.
                assert_eq!(
                    dichiarati("geo.split", &[GeometryType::MultiPolygon], &config),
                    BTreeSet::from([GeometryType::Polygon])
                );
            }
        }

        /// Il campo geometria dell'uscita non porta la dichiarazione dei
        /// tipi dell'ingresso: la riscrive il contratto.
        #[test]
        fn i_tipi_ereditati_non_sopravvivono_nel_campo() {
            let contract = ingresso(&[GeometryType::LineString], true);
            let analizzato = analyze_one(
                "geo.polygonize",
                std::slice::from_ref(&contract),
                &json!({}),
                None,
            )
            .expect("analisi");
            let campo = analizzato
                .schema
                .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                .unwrap();
            assert!(!campo.metadata().contains_key(PLENORA_GEOMETRY_TYPES_KEY));
            assert!(!campo
                .metadata()
                .contains_key(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY));
            let pubblicato = arrow_schema_from_contract(&analizzato).expect("pubblicato");
            assert_eq!(
                pubblicato
                    .field_with_name(DEFAULT_GEOMETRY_COLUMN)
                    .unwrap()
                    .metadata()
                    .get(PLENORA_GEOMETRY_TYPES_KEY)
                    .map(String::as_str),
                Some("linestring,polygon")
            );
        }

        const SEMPLICI_E_MULTI: [GeometryType; 7] = [
            GeometryType::Point,
            GeometryType::LineString,
            GeometryType::Polygon,
            GeometryType::MultiPoint,
            GeometryType::MultiLineString,
            GeometryType::MultiPolygon,
            GeometryType::GeometryCollection,
        ];

        #[test]
        fn explode_e_subdivide_dichiarano_i_tipi_prodotti() {
            for tipo in SEMPLICI_E_MULTI {
                let geometria = esemplare(tipo);
                let parti = crate::operations::explode(&geometria).expect("explode");
                contenuti("geo.explode", &[tipo], &parti, &json!({}));
                let parti =
                    crate::extensions2::subdivide(&geometria, 8, centimetro()).expect("subdivide");
                if !matches!(tipo, GeometryType::Point | GeometryType::GeometryCollection) {
                    assert!(parti.len() > 1, "{tipo:?}: nessun taglio");
                }
                contenuti(
                    "geo.subdivide",
                    &[tipo],
                    &parti,
                    &json!({"max_vertices": 8}),
                );
                // Sotto soglia la geometria passa invariata.
                let intera = crate::extensions2::subdivide(&geometria, 1_000, centimetro())
                    .expect("subdivide");
                contenuti(
                    "geo.subdivide",
                    &[tipo],
                    &intera,
                    &json!({"max_vertices": 1_000}),
                );
            }
        }

        #[test]
        fn collect_dichiara_i_tipi_prodotti() {
            for tipo in SEMPLICI_E_MULTI {
                let geometria = esemplare(tipo);
                let spostata = geo::Translate::translate(&geometria, 100.0, 100.0);
                for gruppo in [
                    vec![Some(geometria.clone())],
                    vec![Some(geometria.clone()), Some(spostata)],
                ] {
                    let raccolta = crate::extensions::collect_geometries(&gruppo)
                        .expect("collect")
                        .expect("gruppo non vuoto");
                    contenuti(
                        "geo.collect",
                        &[tipo],
                        &[raccolta],
                        &json!({"group_by": ["label"]}),
                    );
                }
            }
            // Gruppo misto: `GeometryCollection`.
            let misto = crate::extensions::collect_geometries(&[
                Some(esemplare(GeometryType::Point)),
                Some(esemplare(GeometryType::Polygon)),
            ])
            .expect("collect")
            .expect("gruppo");
            contenuti(
                "geo.collect",
                &[GeometryType::Point, GeometryType::Polygon],
                &[misto],
                &json!({"group_by": ["label"]}),
            );
        }

        #[test]
        fn aggregazioni_e_ricostruzioni_dichiarano_i_tipi_prodotti() {
            let poligoni = [GeometryType::Polygon, GeometryType::MultiPolygon];
            let ingressi: Vec<Geometry<f64>> =
                poligoni.iter().map(|tipo| esemplare(*tipo)).collect();
            let dissolta = crate::topology::dissolve(&ingressi, centimetro()).expect("dissolve");
            contenuti("geo.dissolve", &poligoni, &[dissolta], &json!({}));

            let triangoli = crate::extended_algorithms::delaunay(
                &esemplare(GeometryType::MultiPoint),
                1_000,
                1_000,
            )
            .expect("delaunay");
            let triangoli: Vec<Geometry<f64>> =
                triangoli.into_iter().map(Geometry::Polygon).collect();
            assert!(!triangoli.is_empty());
            contenuti(
                "geo.delaunay",
                &[GeometryType::MultiPoint],
                &triangoli,
                &json!({}),
            );

            let linee = [GeometryType::LineString, GeometryType::MultiLineString];
            for tipo in linee {
                let unite = crate::extended_algorithms::line_merge(&esemplare(tipo), 1_000, 1_000)
                    .expect("line_merge");
                let unite: Vec<Geometry<f64>> =
                    unite.into_iter().map(Geometry::LineString).collect();
                contenuti("geo.line_merge", &[tipo], &unite, &json!({}));
            }

            for (costruttore, tipi) in [
                ("geo.line_builder", [GeometryType::Point]),
                ("geo.polygon_builder", [GeometryType::LineString]),
            ] {
                let mut contract = geo_contract(projected_crs());
                contract.geometries[0].types = esatti(&tipi);
                let output =
                    analyze_one(costruttore, &[contract], &json!({}), None).expect("costruttore");
                assert!(
                    output.geometries[0].types.value().is_none(),
                    "{costruttore}: nessun kernel qui la verifica, nessuna dichiarazione"
                );
            }
        }

        #[test]
        fn overlay_dichiara_i_tipi_prodotti() {
            let sinistra = [esemplare(GeometryType::Polygon)];
            let destra = [Geometry::Polygon(quadrato(3.0, 3.0, 4.0))];
            let mut prodotti = Vec::new();
            for modo in [
                crate::topology::OverlayMode::Intersection,
                crate::topology::OverlayMode::Union,
                crate::topology::OverlayMode::Identity,
                crate::topology::OverlayMode::SymmetricDifference,
            ] {
                for (l, r) in [(&sinistra[..], &destra[..]), (&sinistra[..], &[][..])] {
                    prodotti.extend(
                        crate::topology::polygon_overlay(l, r, modo, 100, 100, centimetro())
                            .expect("overlay")
                            .into_iter()
                            .map(|pezzo| pezzo.geometry),
                    );
                }
            }
            let mut contratto_sinistro = geo_contract(projected_crs());
            contratto_sinistro.geometries[0].types = esatti(&[GeometryType::Polygon]);
            let output = analyze_one(
                "geo.overlay",
                &[contratto_sinistro, geo_contract(projected_crs())],
                &json!({"mode": "union"}),
                None,
            )
            .expect("overlay");
            let ammessi: BTreeSet<GeometryType> = output.geometries[0]
                .types
                .value()
                .expect("tipi")
                .types()
                .iter()
                .copied()
                .collect();
            for prodotto in &prodotti {
                assert!(
                    ammessi.contains(&tipo_di(prodotto)),
                    "{:?}",
                    tipo_di(prodotto)
                );
            }
        }
    }
}
