//! Politica unica delle dichiarazioni geometriche, indipendente dal trasporto.
pub mod report;
pub mod types;

use crate::arrow::Metadata;
use crate::PlenoraError;

/// Significato di una chiave geometrica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Significato delle coordinate.
    Crs,
    /// Riepilogo dei valori.
    Derived,
    /// Provenienza o rappresentazione.
    Representation,
    /// Tipi e dimensioni.
    Types,
}
/// Effetto sulla singola colonna geometrica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Celle e righe invariate.
    Unchanged,
    /// Celle invariate, righe selezionate o riordinate.
    Rows,
    /// Coordinate o geometrie modificate.
    Values,
    /// Valori e dichiarazioni dei tipi ricostruiti.
    Types,
    /// Riferimento delle coordinate cambiato.
    Crs,
    /// Colonna risultante da piu' sorgenti.
    Merge,
}
/// Elenco esaustivo delle chiavi conosciute; gli altri membri geometrici sono derivati.
pub const KEYS: &[(&str, Class)] = &[
    ("plenora.postgres.native_type", Class::Derived),
    ("plenora.postgres.native_declaration", Class::Derived),
    ("plenora.postgres.type_kind", Class::Derived),
    ("plenora.postgres.enum_labels", Class::Derived),
    ("plenora.postgres.domain_base_type", Class::Derived),
    ("plenora.postgres.domain_constraints", Class::Derived),
    ("plenora.postgres.collation", Class::Derived),
    ("crs", Class::Crs),
    ("epoch", Class::Crs),
    ("edges", Class::Crs),
    ("crs_type", Class::Crs),
    ("axis_order", Class::Crs),
    ("plenora.geometry.crs_resolution", Class::Crs),
    ("plenora.geometry.crs_id", Class::Crs),
    ("plenora.geometry.crs_definition", Class::Crs),
    ("plenora.geometry.crs_definition_format", Class::Crs),
    ("plenora.geometry.axis_order", Class::Crs),
    ("plenora.geometry.srid", Class::Crs),
    ("plenora.geometry.spatial_semantics", Class::Crs),
    ("plenora.geometry.native.geoparquet.epoch", Class::Crs),
    ("plenora.geometry.native.geoparquet.edges", Class::Crs),
    ("bbox", Class::Derived),
    ("covering", Class::Derived),
    ("geometry_types", Class::Derived),
    ("orientation", Class::Representation),
    ("precision", Class::Derived),
    ("plenora.geometry.native.geoparquet.column", Class::Derived),
    ("plenora.geometry.native.geoparquet.bbox", Class::Derived),
    (
        "plenora.geometry.native.geoparquet.orientation",
        Class::Representation,
    ),
    ("encoding", Class::Representation),
    ("ARROW:extension:name", Class::Representation),
    ("plenora.geometry.encoding", Class::Representation),
    ("plenora.geometry.precision", Class::Representation),
    (
        "plenora.geometry.native.geoparquet.primary_column",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.geoparquet.version",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.geoparquet.compatibilita",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.geoparquet.altre_colonne",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.gpkg.rtree_index",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.shp.measure_no_data",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.filegdb.geometry_field_count",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.dxf.geometry_model",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.dxf.z_inference",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.csv.geometry_encoding",
        Class::Representation,
    ),
    (
        "plenora.geometry.native.xlsx.geometry_encoding",
        Class::Representation,
    ),
    ("dimensions", Class::Types),
    ("types", Class::Types),
    ("types_declaration", Class::Types),
    ("plenora.geometry.dimensions", Class::Types),
    ("plenora.geometry.types", Class::Types),
    ("plenora.geometry.types_declaration", Class::Types),
    (
        "plenora.geometry.native.gpkg.geometry_type_name",
        Class::Types,
    ),
    ("plenora.geometry.native.gpkg.z", Class::Types),
    ("plenora.geometry.native.gpkg.m", Class::Types),
    ("plenora.geometry.native.shp.shape_type", Class::Types),
    (
        "plenora.geometry.native.filegdb.ogr_geometry_type",
        Class::Types,
    ),
];
/// Nomi strutturali dei contenitori: non sono proprieta' di una geometria.
/// `columns` contiene nomi di campi, non ulteriori chiavi di proprieta'.
pub const STRUCTURAL_MEMBERS: &[&str] = &[
    "plenora.primary_geometry",
    "geo",
    "ARROW:extension:metadata",
    "version",
    "primary_column",
    "columns",
];

/// Classe di una chiave nel namespace geometrico o in un contenitore geometrico.
#[must_use]
pub fn class(key: &str) -> Class {
    KEYS.iter()
        .find(|(name, _)| *name == key)
        .map_or(Class::Derived, |(_, class)| *class)
}
/// Chiavi di primo livello governate da questa politica.
#[must_use]
pub fn governed(key: &str) -> bool {
    key.starts_with("plenora.geometry.")
        || key.starts_with("plenora.postgres.")
        || KEYS.iter().any(|(name, _)| *name == key)
}
/// Decisione unica per classe ed effetto; bbox e' un contenitore dei valori superstiti.
#[must_use]
pub fn keep(key: &str, effect: Effect, equal: bool) -> bool {
    // Dichiarazioni sul valore corrente, anche future: nessuna provenienza
    // nota in questo namespace. Non estende l'enum pubblico Class della 4.1.
    if key.starts_with("plenora.postgres.") {
        return matches!(effect, Effect::Unchanged | Effect::Rows);
    }
    match (class(key), effect) {
        (_, Effect::Unchanged)
        | (Class::Crs | Class::Representation | Class::Types, Effect::Rows)
        | (Class::Crs, Effect::Values | Effect::Types | Effect::Merge)
        | (Class::Types, Effect::Values | Effect::Crs) => true,
        (Class::Derived, Effect::Rows) => {
            matches!(key, "bbox" | "plenora.geometry.native.geoparquet.bbox")
        }
        (Class::Representation | Class::Types, Effect::Merge) => equal,
        _ => false,
    }
}
/// I membri tipizzati legacy rimangono al codec, salvo il confronto di fusione.
#[must_use]
pub fn keep_member(key: &str, effect: Effect, equal: bool) -> bool {
    (effect != Effect::Merge && matches!(key, "dimensions" | "encoding"))
        || keep(key, effect, equal)
}

/// Applica la politica ai membri opachi, lasciando i campi tipizzati al codec.
/// Il codec riscrive encoding, precisione, tipi e dimensioni dall'uscita effettiva.
pub fn apply(metadata: &mut Metadata, effect: Effect) {
    if effect == Effect::Unchanged {
        return;
    }
    if matches!(effect, Effect::Values | Effect::Types | Effect::Crs)
        && metadata
            .get("plenora.geometry.precision")
            .is_some_and(|v| v != "float64")
    {
        metadata.remove("plenora.geometry.precision");
    }
    metadata.retain(|key, value| {
        (!governed(key)
            || (typed(key) && !(effect == Effect::Crs && class(key) == Class::Crs))
            || keep(key, effect, true))
            && (effect != Effect::Rows
                || !matches!(
                    key.as_str(),
                    "bbox" | "plenora.geometry.native.geoparquet.bbox"
                )
                || valid_bbox(value))
    });
    for container in ["geo", "ARROW:extension:metadata"] {
        let Some(raw) = metadata.get(container) else {
            continue;
        };
        // Un documento malformato rimane visibile al validatore.
        if crate::json::ensure_no_duplicate_keys(raw).is_err()
            || crate::json::ensure_numbers_exact(raw).is_err()
        {
            continue;
        }
        if let Ok(serde_json::Value::Object(mut object)) = serde_json::from_str(raw) {
            let original = object.clone();
            if object
                .get("crs")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| crate::json::ensure_no_duplicate_keys(text).is_err())
            {
                continue;
            }
            object.retain(|key, value| {
                keep_member(key, effect, true)
                    && (effect != Effect::Rows || key != "bbox" || valid_bbox(&value.to_string()))
            });
            if object == original {
                continue;
            }
            if object.is_empty() {
                metadata.remove(container);
            } else {
                metadata.insert(container, serde_json::Value::Object(object).to_string());
            }
        }
    }
}
fn typed(key: &str) -> bool {
    key == "ARROW:extension:name"
        || (key.starts_with("plenora.geometry.")
            && !key.starts_with("plenora.geometry.native.")
            && KEYS.iter().any(|(name, _)| *name == key))
}
/// Fondi dichiarazioni senza ereditare riepiloghi del primo ramo.
///
/// # Errors
/// Rifiuta proprieta' CRS diverse o presenti da un solo lato.
pub fn merge(left: &Metadata, right: &Metadata) -> Result<Metadata, PlenoraError> {
    let mut result = left.clone();
    for key in left.keys().chain(right.keys()) {
        if !governed(key) {
            continue;
        }
        let equal = left.get(key) == right.get(key);
        if class(key) == Class::Crs && !typed(key) && key != "crs" && !equal {
            return Err(PlenoraError::Schema(
                "fusione con proprieta' del CRS diverse".into(),
            ));
        }
        if !keep(key, Effect::Merge, equal) {
            result.remove(key);
        }
    }
    if left.get("plenora.geometry.types") != right.get("plenora.geometry.types")
        || left.get("plenora.geometry.types_declaration")
            != right.get("plenora.geometry.types_declaration")
    {
        result.remove("plenora.geometry.types");
        result.insert("plenora.geometry.types_declaration", "unresolved");
    }
    for container in ["geo", "ARROW:extension:metadata"] {
        let parse =
            |md: &Metadata| -> Result<serde_json::Map<String, serde_json::Value>, PlenoraError> {
                let Some(raw) = md.get(container) else {
                    return Ok(serde_json::Map::new());
                };
                crate::json::ensure_no_duplicate_keys(raw)
                    .map_err(|_| PlenoraError::Schema("metadato geometrico non valido".into()))?;
                crate::json::ensure_numbers_exact(raw)
                    .map_err(|_| PlenoraError::Schema("numero geometrico non esatto".into()))?;
                match serde_json::from_str(raw) {
                    Ok(serde_json::Value::Object(object)) => Ok(object),
                    _ => Err(PlenoraError::Schema(
                        "metadato geometrico non oggetto".into(),
                    )),
                }
            };
        let a = parse(left)?;
        let b = parse(right)?;
        let mut merged = a.clone();
        for key in a.keys().chain(b.keys()) {
            let equal = a.get(key) == b.get(key);
            // Il codec confronta le rappresentazioni CRS equivalenti.
            if class(key) == Class::Crs && !matches!(key.as_str(), "crs" | "crs_type") && !equal {
                return Err(PlenoraError::Schema(
                    "fusione con proprieta' del CRS diverse".into(),
                ));
            }
            if !keep(key, Effect::Merge, equal) {
                merged.remove(key);
            }
        }
        if merged.is_empty() {
            result.remove(container);
        } else if left.contains_key(container) {
            result.insert(container, serde_json::Value::Object(merged).to_string());
        }
    }
    Ok(result)
}

/// Uso geometrico di ciascun ingresso; per N-aria il ruolo si ripete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandUse {
    /// Trasporto senza interpretare coordinate.
    Transport,
    /// Selezione o riordino delle celle.
    Rows,
    /// Lettura della geometria per calcolare, anche quando viene conservata.
    Read,
    /// Produzione da attributi o configurazione, senza operando geometrico.
    Produce,
}

/// Inventario chiuso: il gate confronta tutte le righe con il catalogo.
pub const OPERATION_POLICY: &[(&str, Effect, &[OperandUse])] = &[
    (
        "table.add_row_number",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.aggregate", Effect::Rows, &[OperandUse::Rows]),
    ("table.bin", Effect::Unchanged, &[OperandUse::Transport]),
    ("table.concat", Effect::Merge, &[OperandUse::Rows]),
    (
        "table.concat_columns",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.conditional",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.cross_join",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    (
        "table.date_extract",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.dedup_advanced", Effect::Rows, &[OperandUse::Rows]),
    ("table.distinct", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.drop_columns",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.fill_na", Effect::Unchanged, &[OperandUse::Transport]),
    ("table.filter", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.flatten_json",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.formula", Effect::Unchanged, &[OperandUse::Transport]),
    (
        "table.join",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    ("table.lookup", Effect::Unchanged, &[OperandUse::Transport]),
    ("table.melt", Effect::Rows, &[OperandUse::Rows]),
    ("table.pivot", Effect::Rows, &[OperandUse::Rows]),
    ("table.rename", Effect::Unchanged, &[OperandUse::Transport]),
    (
        "table.reorder_columns",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.replace", Effect::Unchanged, &[OperandUse::Transport]),
    ("table.sample", Effect::Rows, &[OperandUse::Rows]),
    ("table.sort", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.split_column",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.statistics", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.string_extract",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.string_length",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.string_pad",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.table_diff",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    (
        "table.text_normalize",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.transpose", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.type_cast",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.uuid_generator",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.window_function", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.mask_data",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.md5_hash",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.anti_join",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    (
        "table.asof_join",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    (
        "table.assert_not_null",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.assert_range",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.assert_regex",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.assert_schema",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.assert_unique",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.coalesce",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.date_add",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.date_diff",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.date_format",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.except",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    ("table.explode", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.intersect",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    ("table.rolling_window", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.semi_join",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    (
        "table.sha256_hash",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.timezone_convert",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.union_distinct",
        Effect::Merge,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    ("table.unnest", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.expression",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.assert_cardinality",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.assert_metadata",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.assert_foreign_key",
        Effect::Unchanged,
        &[OperandUse::Transport, OperandUse::Transport],
    ),
    (
        "table.reconcile",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
    ("geo.centroid", Effect::Types, &[OperandUse::Read]),
    ("geo.convex_hull", Effect::Types, &[OperandUse::Read]),
    ("geo.envelope", Effect::Types, &[OperandUse::Read]),
    (
        "geo.sjoin",
        Effect::Rows,
        &[OperandUse::Read, OperandUse::Read],
    ),
    ("geo.area", Effect::Unchanged, &[OperandUse::Read]),
    ("geo.boundary", Effect::Types, &[OperandUse::Read]),
    (
        "geo.bounds_extractor",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    ("geo.buffer", Effect::Types, &[OperandUse::Read]),
    ("geo.clean_topology", Effect::Types, &[OperandUse::Read]),
    (
        "geo.clip",
        Effect::Types,
        &[OperandUse::Read, OperandUse::Read],
    ),
    (
        "geo.count_points_in_polygons",
        Effect::Unchanged,
        &[OperandUse::Read, OperandUse::Read],
    ),
    (
        "geo.difference",
        Effect::Types,
        &[OperandUse::Read, OperandUse::Read],
    ),
    ("geo.dissolve", Effect::Types, &[OperandUse::Read]),
    ("geo.distance", Effect::Unchanged, &[OperandUse::Read]),
    ("geo.explode", Effect::Types, &[OperandUse::Read]),
    ("geo.from_coords", Effect::Values, &[OperandUse::Produce]),
    (
        "geo.intersection",
        Effect::Types,
        &[OperandUse::Read, OperandUse::Read],
    ),
    ("geo.length", Effect::Unchanged, &[OperandUse::Read]),
    ("geo.line_builder", Effect::Types, &[OperandUse::Read]),
    (
        "geo.nearest",
        Effect::Rows,
        &[OperandUse::Read, OperandUse::Read],
    ),
    (
        "geo.overlay",
        Effect::Types,
        &[OperandUse::Read, OperandUse::Read],
    ),
    ("geo.perimeter", Effect::Unchanged, &[OperandUse::Read]),
    ("geo.point_on_surface", Effect::Types, &[OperandUse::Read]),
    ("geo.polygon_builder", Effect::Types, &[OperandUse::Read]),
    ("geo.simplify", Effect::Values, &[OperandUse::Read]),
    (
        "geo.symmetric_difference",
        Effect::Types,
        &[OperandUse::Read, OperandUse::Read],
    ),
    ("geo.to_wkt", Effect::Unchanged, &[OperandUse::Read]),
    (
        "geo.union",
        Effect::Types,
        &[OperandUse::Read, OperandUse::Read],
    ),
    ("geo.vertex_count", Effect::Unchanged, &[OperandUse::Read]),
    ("geo.voronoi", Effect::Types, &[OperandUse::Read]),
    (
        "geo.within",
        Effect::Unchanged,
        &[OperandUse::Read, OperandUse::Read],
    ),
    ("geo.make_valid", Effect::Types, &[OperandUse::Read]),
    ("geo.reproject", Effect::Crs, &[OperandUse::Read]),
    (
        "geo.predicate_intersects",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_disjoint",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_contains",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_within",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_equals_topo",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_covers",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_covered_by",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_contains_properly",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_touches",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_crosses",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.predicate_overlaps",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    ("geo.affine_transform", Effect::Values, &[OperandUse::Read]),
    ("geo.translate", Effect::Values, &[OperandUse::Read]),
    ("geo.scale", Effect::Values, &[OperandUse::Read]),
    ("geo.rotate", Effect::Values, &[OperandUse::Read]),
    ("geo.concave_hull", Effect::Types, &[OperandUse::Read]),
    (
        "geo.hausdorff_distance",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.haversine_distance",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.geodesic_distance",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    (
        "geo.geodesic_line_length",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    ("geo.densify", Effect::Values, &[OperandUse::Read]),
    ("geo.snap_to_grid", Effect::Values, &[OperandUse::Read]),
    ("geo.delaunay", Effect::Types, &[OperandUse::Read]),
    ("geo.polygonize", Effect::Types, &[OperandUse::Read]),
    ("geo.line_merge", Effect::Types, &[OperandUse::Read]),
    ("geo.split", Effect::Types, &[OperandUse::Read]),
    ("geo.line_substring", Effect::Types, &[OperandUse::Read]),
    (
        "geo.line_interpolate_point",
        Effect::Types,
        &[OperandUse::Read],
    ),
    (
        "geo.frechet_distance",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    ("geo.bearing", Effect::Unchanged, &[OperandUse::Read]),
    ("geo.geodesic_area", Effect::Unchanged, &[OperandUse::Read]),
    (
        "geo.geometry_diagnostics",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    ("geo.from_wkt", Effect::Values, &[OperandUse::Produce]),
    (
        "geo.geometry_accessors",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    ("geo.collect", Effect::Types, &[OperandUse::Read]),
    (
        "geo.line_locate_point",
        Effect::Unchanged,
        &[OperandUse::Read],
    ),
    ("geo.generate_grid", Effect::Values, &[OperandUse::Produce]),
    ("geo.subdivide", Effect::Types, &[OperandUse::Read]),
    ("geo.snap", Effect::Values, &[OperandUse::Read]),
    ("geo.coverage_validate", Effect::Types, &[OperandUse::Read]),
    ("geo.shared_paths", Effect::Types, &[OperandUse::Read]),
    ("geo.cluster_dbscan", Effect::Unchanged, &[OperandUse::Read]),
    ("table.limit", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.select_columns",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    (
        "table.stable_fingerprint",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.top_n", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.align_schema",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.concat_by_name", Effect::Merge, &[OperandUse::Rows]),
    (
        "table.hmac_sha256",
        Effect::Unchanged,
        &[OperandUse::Transport],
    ),
    ("table.validate_rules", Effect::Rows, &[OperandUse::Rows]),
    (
        "table.fuzzy_join",
        Effect::Rows,
        &[OperandUse::Rows, OperandUse::Rows],
    ),
];

/// Ruoli degli ingressi dichiarati, senza default per operazioni sconosciute.
///
/// # Errors
/// Internal se il catalogo non ha una classificazione.
pub fn operand_uses(id: &str) -> Result<&'static [OperandUse], PlenoraError> {
    OPERATION_POLICY
        .iter()
        .find(|row| row.0 == id)
        .map(|row| row.2)
        .ok_or_else(|| PlenoraError::Internal("operazione senza politica geometrica".into()))
}

/// Effetto sul campo geometrico ereditato. Il gate vieta voci senza politica.
#[must_use]
pub fn operation_effect(op: &crate::catalog::OperationDescriptor) -> Effect {
    OPERATION_POLICY
        .iter()
        .find(|row| row.0 == op.id)
        .map_or(Effect::Merge, |row| row.1)
}
/// Effetto per ruolo sulle geometrie d'uscita, senza consultare la primaria.
///
/// Le colonne nuove hanno dichiarazioni fresche; i produttori ereditano solo C1.
/// Gli operandi sono quelli trasformati sopravvissuti, rimappati per identita' dal chiamante.
///
/// # Errors
/// Internal se una trasformazione perde operandi con righe non conservate,
/// fuori dai produttori. Le misure possono eliminare il solo operando letto.
pub fn column_effects(
    descriptor: &crate::catalog::OperationDescriptor,
    output_schema: &crate::arrow::Schema,
    operands: &[Option<&super::GeometryColumnContract>],
) -> Result<Vec<(usize, Effect)>, PlenoraError> {
    use crate::catalog::{Family, SourceRowProvenance};
    let effect = if descriptor.id == "table.table_diff" {
        Effect::Merge
    } else {
        operation_effect(descriptor)
    };
    let passthrough = if descriptor.source_row_provenance() == SourceRowProvenance::Preserved {
        Effect::Unchanged
    } else {
        Effect::Rows
    };
    let selected = operands.iter().copied().flatten().collect::<Vec<_>>();
    let present = |operand: &&super::GeometryColumnContract| {
        output_schema.fields().iter().any(|field| {
            super::arrow_metadata::field_declares_wkb_geometry(field)
                && operand.name == *field.name()
        })
    };
    let survives = selected.iter().any(present);
    let missing = selected.iter().any(|operand| !present(operand));
    let producer = matches!(
        descriptor.id,
        "geo.coverage_validate" | "geo.shared_paths" | "geo.generate_grid"
    );
    if descriptor.family == Family::Geo
        && !matches!(effect, Effect::Unchanged | Effect::Rows)
        && passthrough == Effect::Rows
        && (!survives || missing)
        && !producer
    {
        return Err(PlenoraError::Internal(
            "politica metadati: operando assente dall'uscita con righe non conservate".into(),
        ));
    }
    Ok(output_schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, field)| super::arrow_metadata::field_declares_wkb_geometry(field))
        .map(|(position, field)| {
            let selected = selected.iter().any(|operand| operand.name == *field.name());
            let effect = if descriptor.family != Family::Geo || selected {
                effect
            } else if producer && !survives {
                Effect::Unchanged
            } else {
                passthrough
            };
            (position, effect)
        })
        .collect())
}

/// Applica l'effetto alle colonne geometriche di uno schema.
///
/// # Errors
/// Proprieta' CRS incompatibili in una fusione.
pub fn schema_effect(
    schema: &crate::arrow::Schema,
    inputs: &[&crate::arrow::Schema],
    effect: Effect,
) -> Result<crate::arrow::Schema, PlenoraError> {
    let fields = schema
        .fields()
        .iter()
        .map(|field| {
            let mut md = field.metadata().clone();
            if super::arrow_metadata::field_declares_wkb_geometry(field) {
                if effect == Effect::Merge {
                    for input in inputs {
                        if let Ok(other) = input.field_with_name(field.name()) {
                            md = merge(&md, other.metadata())?;
                        }
                    }
                    // La fusione grezza elimina C3 discordante. Lo schema di
                    // esecuzione riemette il default del codec WKB, come la
                    // pubblicazione canonica del contratto in validazione.
                    if !md.contains_key("plenora.geometry.precision")
                        && inputs.iter().any(|input| {
                            input.field_with_name(field.name()).is_ok_and(|source| {
                                source.metadata().contains_key("plenora.geometry.precision")
                            })
                        })
                    {
                        md.insert("plenora.geometry.precision", "float64");
                    }
                } else {
                    apply(&mut md, effect);
                }
            }
            Ok(field.as_ref().clone().with_metadata(md))
        })
        .collect::<Result<Vec<_>, PlenoraError>>()?;
    let mut md = schema.metadata().clone();
    if effect != Effect::Unchanged {
        apply(&mut md, effect);
        // Il geo di schema e' un riepilogo dell'intero dataset, rigenerato dal writer.
        md.remove("geo");
    }
    Ok(crate::arrow::Schema::new_with_metadata(fields, md))
}

/// Rimuove dichiarazioni covering che non referenziano lo schema pubblicato.
/// I contenitori nativi rimangono opachi: nessun percorso e' un requisito di input.
///
/// # Errors
/// JSON con chiavi duplicate o numeri non esatti.
pub fn sanitize_covering(
    schema: &crate::arrow::Schema,
) -> Result<crate::arrow::Schema, PlenoraError> {
    fn valid(value: &serde_json::Value, schema: &crate::arrow::Schema) -> bool {
        match value {
            serde_json::Value::Object(object) => {
                !object.is_empty() && object.values().all(|v| valid(v, schema))
            }
            serde_json::Value::Array(path) => {
                let mut names = path.iter();
                let Some(name) = names.next().and_then(serde_json::Value::as_str) else {
                    return false;
                };
                let Ok(mut field) = schema.field_with_name(name) else {
                    return false;
                };
                for name in names {
                    let Some(name) = name.as_str() else {
                        return false;
                    };
                    let crate::arrow::DataType::Struct(fields) = field.data_type() else {
                        return false;
                    };
                    let Some(child) = fields.iter().find(|f| f.name() == name) else {
                        return false;
                    };
                    field = child;
                }
                true
            }
            _ => false,
        }
    }
    fn prune(value: &mut serde_json::Value, schema: &crate::arrow::Schema) {
        if let serde_json::Value::Object(object) = value {
            if object.get("covering").is_some_and(|v| !valid(v, schema)) {
                object.remove("covering");
            }
            for child in object.values_mut() {
                prune(child, schema);
            }
        }
    }
    let clean = |source: &Metadata| -> Result<Metadata, PlenoraError> {
        let mut md = source.clone();
        for key in [
            "geo",
            "covering",
            "ARROW:extension:metadata",
            "plenora.geometry.native.geoparquet.column",
        ] {
            let Some(raw) = source.get(key) else { continue };
            let Ok(mut value) = serde_json::from_str(raw) else {
                continue;
            };
            crate::json::ensure_no_duplicate_keys(raw).map_err(|_| {
                PlenoraError::Schema("metadato geometrico con chiavi duplicate".into())
            })?;
            if key == "covering" {
                if !valid(&value, schema) {
                    md.remove(key);
                }
            } else {
                let original = value.clone();
                prune(&mut value, schema);
                if value != original {
                    crate::json::ensure_numbers_exact(raw).map_err(|_| {
                        PlenoraError::Schema("metadato geometrico con numero non esatto".into())
                    })?;
                    md.insert(key, value.to_string());
                }
            }
        }
        Ok(md)
    };
    let fields = schema
        .fields()
        .iter()
        .map(|field| {
            Ok(field
                .as_ref()
                .clone()
                .with_metadata(clean(field.metadata())?))
        })
        .collect::<Result<Vec<_>, PlenoraError>>()?;
    Ok(crate::arrow::Schema::new_with_metadata(
        fields,
        clean(schema.metadata())?,
    ))
}

fn valid_bbox(raw: &str) -> bool {
    let wrapped;
    let raw = if raw.starts_with('[') {
        raw
    } else {
        wrapped = format!("[{raw}]");
        &wrapped
    };
    if crate::json::ensure_numbers_exact(raw).is_err() {
        return false;
    }
    let Ok(serde_json::Value::Array(values)) = serde_json::from_str(raw) else {
        return false;
    };
    if !matches!(values.len(), 4 | 6) {
        return false;
    }
    let half = values.len() / 2;
    values.iter().take(half).zip(values.iter().skip(half)).all(|(lo, hi)| {
        matches!((lo.as_f64(), hi.as_f64()), (Some(lo), Some(hi)) if lo.is_finite() && hi.is_finite() && lo <= hi)
    })
}
/// Metadati del dataset dopo un effetto su righe o geometrie.
#[must_use]
pub fn schema_metadata(metadata: &Metadata, effect: Effect) -> Metadata {
    let mut output = metadata.clone();
    apply(&mut output, effect);
    if effect != Effect::Unchanged {
        output.remove("geo");
    }
    output
}

/// Fusione dei metadati di dataset: stessa politica per le chiavi geometriche,
/// conflitto esplicito per le chiavi applicative.
///
/// # Errors
/// Dichiarazioni CRS diverse o metadati applicativi incompatibili.
pub fn merge_schema(sources: &[&Metadata]) -> Result<Metadata, PlenoraError> {
    let Some(first) = sources.first() else {
        return Ok(Metadata::new());
    };
    let mut output = (*first).clone();
    output.remove("geo");
    for source in sources.iter().skip(1) {
        let mut source = (*source).clone();
        source.remove("geo");
        for key in output.keys().chain(source.keys()) {
            if governed(key) && class(key) == Class::Crs && output.get(key) != source.get(key) {
                return Err(PlenoraError::Schema(
                    "proprieta' CRS di schema in conflitto".into(),
                ));
            }
        }
        let mut next = merge(&output, &source)?;
        for (key, value) in &source {
            if governed(key) {
                continue;
            }
            if output.get(key).is_some_and(|old| old != value) {
                return Err(PlenoraError::Schema(format!(
                    "metadato applicativo di schema in conflitto: {key}"
                )));
            }
            next.insert(key.clone(), value.clone());
        }
        output = next;
    }
    apply(&mut output, Effect::Merge);
    Ok(output)
}

/// Proprieta' CRS opache, coerenti fra le rappresentazioni e trasportabili.
///
/// # Errors
/// CRS per epoca non numerica, lati non riconosciuti o dichiarazioni discordanti.
pub fn crs_properties(
    campo: &crate::arrow::Field,
) -> Result<serde_json::Map<String, serde_json::Value>, PlenoraError> {
    use serde_json::{Map, Value};
    let invalido = |message: &str| PlenoraError::Crs(message.to_owned());
    let mut output = Map::new();
    for (member, native) in [
        ("epoch", "plenora.geometry.native.geoparquet.epoch"),
        ("edges", "plenora.geometry.native.geoparquet.edges"),
    ] {
        let mut candidates = Vec::new();
        if let Some(raw) = campo.metadata().get(native) {
            if member == "epoch" {
                crate::json::ensure_numbers_exact(raw).map_err(|_| invalido("epoch non esatta"))?;
                candidates.push(
                    serde_json::from_str::<Value>(raw)
                        .map_err(|_| invalido("epoch non numerica"))?,
                );
            } else {
                candidates.push(Value::String(raw.clone()));
            }
        }
        for container in [
            super::arrow_metadata::GEO_METADATA_KEY,
            super::arrow_schema::GEOARROW_EXTENSION_METADATA_KEY,
        ] {
            if let Some(raw) = campo.metadata().get(container).filter(|raw| {
                container != super::arrow_schema::GEOARROW_EXTENSION_METADATA_KEY
                    || !raw.trim().is_empty()
            }) {
                crate::json::ensure_no_duplicate_keys(raw)
                    .map_err(|_| invalido("metadato geometrico duplicato"))?;
                crate::json::ensure_numbers_exact(raw)
                    .map_err(|_| invalido("metadato geometrico non esatto"))?;
                let parsed: Value = serde_json::from_str(raw)
                    .map_err(|_| invalido("metadato geometrico non JSON"))?;
                if let Some(value) = parsed.get(member) {
                    candidates.push(value.clone());
                }
            }
        }
        if let Some(value) = candidates.first() {
            if candidates.iter().any(|other| other != value) {
                return Err(invalido("proprieta' CRS discordanti"));
            }
            if member == "epoch" && !value.as_f64().is_some_and(f64::is_finite) {
                return Err(invalido("epoch non numerica"));
            }
            if member == "edges"
                && !matches!(
                    value.as_str(),
                    Some("planar" | "spherical" | "vincenty" | "thomas" | "andoyer" | "karney")
                )
            {
                return Err(invalido("edges non riconosciuto"));
            }
            output.insert(member.into(), value.clone());
        }
    }
    Ok(output)
}

/// Capacita' del calcolo sul solo campo usato; il trasporto non la richiede.
///
/// # Errors
/// Unsupported per geography o lati non planari, CRS per dichiarazioni incoerenti.
pub fn require_planar(field: &crate::arrow::Field) -> Result<(), PlenoraError> {
    if super::arrow_metadata::canonical_geometry_spatial_semantics(field)?
        == Some(super::SpatialSemantics::Geography)
        || crs_properties(field)?
            .get("edges")
            .is_some_and(|value| value != "planar")
    {
        return Err(PlenoraError::Unsupported(
            "calcolo su semantica non planare non supportato".into(),
        ));
    }
    Ok(())
}

/// Prepara il campo per una dichiarazione dei tipi prodotta dal codec.
pub fn rewrite_types(metadata: &mut Metadata) {
    apply(metadata, Effect::Types);
    metadata.remove("plenora.geometry.types");
    metadata.remove("plenora.geometry.types_declaration");
}

fn object(raw: &str) -> Result<serde_json::Map<String, serde_json::Value>, PlenoraError> {
    crate::json::ensure_no_duplicate_keys(raw)
        .map_err(|_| PlenoraError::Schema("metadato geometrico duplicato".into()))?;
    crate::json::ensure_numbers_exact(raw)
        .map_err(|_| PlenoraError::Schema("metadato geometrico non esatto".into()))?;
    match serde_json::from_str(raw) {
        Ok(serde_json::Value::Object(value)) => Ok(value),
        _ => Err(PlenoraError::Schema(
            "metadato geometrico non oggetto".into(),
        )),
    }
}

/// Riscrive il blocco tipizzato legacy conservando i membri gia' filtrati.
///
/// # Errors
/// Contenitori geometrici malformati.
pub fn rewrite_geo(metadata: &mut Metadata, replacement: &str) -> Result<(), PlenoraError> {
    let mut retained = metadata
        .get("geo")
        .map_or_else(|| Ok(serde_json::Map::new()), |raw| object(raw))?;
    retained.extend(object(replacement)?);
    metadata.insert("geo", serde_json::Value::Object(retained).to_string());
    Ok(())
}

/// Una geometria nuova nello stesso riferimento eredita le sole proprieta' C1.
///
/// # Errors
/// Contenitori geometrici malformati.
pub fn inherit_crs(
    output: &crate::arrow::Field,
    source: &crate::arrow::Field,
) -> Result<crate::arrow::Field, PlenoraError> {
    let mut md = output.metadata().clone();
    for (key, value) in source.metadata() {
        if governed(key) && class(key) == Class::Crs {
            md.insert(key.clone(), value.clone());
        }
    }
    for container in ["geo", "ARROW:extension:metadata"] {
        if let Some(raw) = source.metadata().get(container) {
            if container == "ARROW:extension:metadata" && raw.trim().is_empty() {
                continue;
            }
            let inherited = object(raw)?;
            let mut target = md
                .get(container)
                .map_or_else(|| Ok(serde_json::Map::new()), |raw| object(raw))?;
            target.extend(
                inherited
                    .into_iter()
                    .filter(|(key, _)| class(key) == Class::Crs),
            );
            if !target.is_empty() {
                md.insert(container, serde_json::Value::Object(target).to_string());
            }
        }
    }
    Ok(output.clone().with_metadata(md))
}

/// Aggiorna il contratto C4 di una fusione insieme alle sue dichiarazioni Arrow.
pub fn merge_types(
    output: &mut crate::contract::DataContract,
    inputs: &[crate::contract::DataContract],
) {
    let mut schema_fields = output
        .schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect::<Vec<_>>();
    for geometry in &mut output.geometries {
        let declarations = inputs
            .iter()
            .flat_map(|input| &input.geometries)
            .filter(|other| other.name == geometry.name)
            .collect::<Vec<_>>();
        if declarations
            .windows(2)
            .any(|pair| pair[0].types != pair[1].types)
        {
            geometry.types = crate::contract::GeometryColumnContract::undeclared_types();
            if let Some(field) = schema_fields
                .iter_mut()
                .find(|field| field.name() == &geometry.name)
            {
                let mut metadata = field.metadata().clone();
                metadata.remove("plenora.geometry.types");
                metadata.insert("plenora.geometry.types_declaration", "unresolved");
                *field = field.clone().with_metadata(metadata);
            }
        }
    }
    output.schema = std::sync::Arc::new(crate::arrow::Schema::new_with_metadata(
        schema_fields,
        output.schema.metadata().clone(),
    ));
}

/// Confronta le proprieta' opache; il codec confronta separatamente il CRS tipizzato.
///
/// # Errors
/// Proprieta' CRS native discordanti.
pub fn require_same_reference_metadata(
    left: &Metadata,
    right: &Metadata,
) -> Result<(), PlenoraError> {
    let native = |source: &Metadata| {
        let mut metadata = source.clone();
        metadata.retain(|key, _| {
            !super::arrow_metadata::CRS_KEYS_REPLACED_BY_DECISION.contains(&key.as_str())
                && key != "crs"
        });
        metadata
    };
    merge(&native(left), &native(right)).map(|_| ())
}

/// Effetti delle colonne dopo la rimappatura delle identita' dell'analisi.
/// Le binarie geometriche trasformano la sinistra; la destra e' letta.
///
/// # Errors
/// Gli errori di coerenza di [`column_effects`].
pub fn contract_column_effects(
    descriptor: &crate::catalog::OperationDescriptor,
    output: &super::DataContract,
    operands: &[Option<&super::GeometryColumnContract>],
) -> Result<Vec<(usize, Effect)>, PlenoraError> {
    let resolved = operands
        .iter()
        .take(1)
        .map(|operand| {
            operand.and_then(|operand| {
                output
                    .geometries
                    .iter()
                    .find(|geometry| geometry.field_id == operand.field_id)
            })
        })
        .collect::<Vec<_>>();
    column_effects(descriptor, &output.schema, &resolved)
}
