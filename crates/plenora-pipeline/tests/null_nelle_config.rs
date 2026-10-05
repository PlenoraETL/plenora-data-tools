//! `null` non è l'assenza nelle config dei passi: per ogni operazione che
//! leggeva un `null` scritto come un parametro omesso, la config senza il
//! campo si valida e la stessa config con il campo `null` si rifiuta con
//! `InvalidPlan` e il messaggio fisso di `plenora_core::json::mai_null`.
//!
//! I campi dove `null` ha un significato proprio (`value` di `filter` e
//! `fill_na`, `default_value` e `result` di `conditional`, `default` di
//! `align_schema`) restano ammessi e dichiarati nelle schede; il censimento
//! di ogni campo tabellare è in `censimento_parametri.rs`, quello di ogni
//! campo geo nei test di `plenora_kernels_geo::analyze::config`.

mod comune_geo;

use std::sync::Arc;

use geo::{Coord, Geometry, LineString, Point};
use plenora_core::arrow::array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::json::MESSAGGIO_NULL_NON_AMMESSO;
use plenora_core::{ErrorCategory, PlenoraError};
use serde_json::{json, Value};

use comune_geo::{esadecimale_di, linee, passo, piano, poligoni, punti, UTM, X0, Y0};

/// Valida il piano di un passo `op` sulle tabelle `t` (e `u`).
fn valida(
    op: &str,
    config: &Value,
    tabelle: &[RecordBatch],
    crs: Option<&str>,
) -> Result<(), PlenoraError> {
    let nomi: Vec<&str> = ["t", "u"].into_iter().take(tabelle.len()).collect();
    let mut pipeline = piano(&nomi, vec![passo("x", op, &nomi, config.clone())], &["x"]);
    pipeline.crs = crs.map(str::to_owned);
    let schemi: Vec<(&str, SchemaRef)> = nomi
        .iter()
        .copied()
        .zip(tabelle.iter().map(RecordBatch::schema))
        .collect();
    pipeline.validate(&schemi).map(|_| ())
}

/// La config `base` con `null` al percorso JSON `percorso` (`/a/0/b`).
fn con_null(base: &Value, percorso: &str) -> Value {
    let (genitore, campo) = percorso.rsplit_once('/').expect("percorso");
    let mut config = base.clone();
    config
        .pointer_mut(genitore)
        .and_then(Value::as_object_mut)
        .expect("oggetto")
        .insert(campo.to_owned(), Value::Null);
    config
}

struct Caso {
    op: &'static str,
    tabelle: Vec<RecordBatch>,
    crs: Option<&'static str>,
    /// Config valida senza i campi provati.
    base: Value,
    /// Percorsi JSON dei campi facoltativi da scrivere `null`.
    campi: &'static [&'static str],
}

/// Il frammento atteso nel rifiuto di un `null`: il messaggio di
/// `mai_null`, salvo `geo.reproject`, che non riporta il motivo serde e
/// rifiuta ogni config illeggibile con un messaggio fisso.
fn frammento(op: &str) -> &'static str {
    if op == "geo.reproject" {
        "config non valida per geo.reproject"
    } else {
        MESSAGGIO_NULL_NON_AMMESSO
    }
}

fn verifica(casi: &[Caso]) {
    let mut difetti = Vec::new();
    for caso in casi {
        if let Err(errore) = valida(caso.op, &caso.base, &caso.tabelle, caso.crs) {
            difetti.push(format!("{} omesso: {errore}", caso.op));
            continue;
        }
        for percorso in caso.campi {
            let config = con_null(&caso.base, percorso);
            match valida(caso.op, &config, &caso.tabelle, caso.crs) {
                Err(errore)
                    if errore.category() == ErrorCategory::InvalidPlan
                        && errore.to_string().contains(frammento(caso.op)) => {}
                altro => difetti.push(format!("{} {percorso} null: {altro:?}", caso.op)),
            }
        }
    }
    assert!(difetti.is_empty(), "{}", difetti.join("\n"));
}

fn colonna_testo(valori: &[&str]) -> ArrayRef {
    Arc::new(StringArray::from(valori.to_vec()))
}

/// Tabella senza geometria: `id` ordinato, `name` e `date` di testo.
fn tabellare() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("date", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            colonna_testo(&["a", "b", "a"]),
            colonna_testo(&["2024-01-01", "2024-02-01", "2024-03-01"]),
        ],
    )
    .expect("tabella")
}

#[test]
fn le_config_tabellari_distinguono_null_e_assente() {
    let t = || vec![tabellare()];
    verifica(&[
        Caso {
            op: "table.add_row_number",
            tabelle: t(),
            crs: None,
            base: json!({}),
            campi: &["/partition_column", "/order_column", "/ascending"],
        },
        Caso {
            op: "table.fill_na",
            tabelle: t(),
            crs: None,
            base: json!({"method": "value", "value": 1}),
            campi: &["/column"],
        },
        Caso {
            op: "table.date_extract",
            tabelle: t(),
            crs: None,
            base: json!({"column": "date"}),
            campi: &["/date_format"],
        },
        Caso {
            op: "table.string_extract",
            tabelle: t(),
            crs: None,
            base: json!({"column": "name", "pattern": "a"}),
            campi: &["/output_column"],
        },
        Caso {
            op: "table.string_length",
            tabelle: t(),
            crs: None,
            base: json!({"column": "name"}),
            campi: &["/output_column"],
        },
        Caso {
            op: "table.string_pad",
            tabelle: t(),
            crs: None,
            base: json!({"column": "name", "width": 5}),
            campi: &["/output_column"],
        },
        Caso {
            op: "table.asof_join",
            tabelle: vec![tabellare(), tabellare()],
            crs: None,
            base: json!({"left_on": "id", "right_on": "id"}),
            campi: &["/tolerance"],
        },
        Caso {
            op: "table.assert_schema",
            tabelle: t(),
            crs: None,
            base: json!({"fields": [
                {"name": "id", "data_type": "int64"},
                {"name": "name", "data_type": "utf8"},
                {"name": "date", "data_type": "utf8"},
            ]}),
            campi: &["/fields/0/nullable"],
        },
        Caso {
            op: "table.validate_rules",
            tabelle: t(),
            crs: None,
            base: json!({"rules": [{"name": "r", "operator": "isnull", "column": "id"}]}),
            campi: &["/rules/0/value"],
        },
        Caso {
            op: "table.lookup",
            tabelle: t(),
            crs: None,
            base: json!({"column": "name", "mapping": {"a": "A"}}),
            campi: &["/default"],
        },
    ]);
    // `column` di una regola: omessa la regola si rifiuta comunque, quindi
    // qui si prova solo che il `null` si rifiuta per il `null`.
    let errore = valida(
        "table.validate_rules",
        &json!({"rules": [{"name": "r", "operator": "isnull", "column": null}]}),
        &[tabellare()],
        None,
    )
    .expect_err("null non e' l'assenza");
    assert!(
        errore.to_string().contains(MESSAGGIO_NULL_NON_AMMESSO),
        "{errore}"
    );
}

/// Tabella con le coordinate di `geo.from_coords` e il WKT di
/// `geo.from_wkt`.
fn coordinate() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
            Field::new("wkt", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![X0, X0 + 1.0])),
            Arc::new(Float64Array::from(vec![Y0, Y0 + 1.0])),
            colonna_testo(&["POINT (500000 5000000)", "POINT (500001 5000001)"]),
        ],
    )
    .expect("tabella")
}

#[test]
#[allow(clippy::too_many_lines)] // Un caso per configurazione geo.
fn le_config_geo_distinguono_null_e_assente() {
    let punto = esadecimale_di(&Geometry::Point(Point::new(X0, Y0)));
    let lama = esadecimale_di(&Geometry::LineString(LineString(vec![
        Coord {
            x: X0 - 10.0,
            y: Y0 + 0.5,
        },
        Coord {
            x: X0 + 10.0,
            y: Y0 + 0.5,
        },
    ])));
    let estensione = json!({"xmin": X0, "ymin": Y0, "xmax": X0 + 10.0, "ymax": Y0 + 10.0});
    verifica(&[
        Caso {
            op: "geo.area",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({}),
            campi: &["/output_column"],
        },
        Caso {
            op: "geo.buffer",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"distance": 1.0}),
            campi: &["/cap"],
        },
        Caso {
            op: "geo.simplify",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"tolerance": 1.0}),
            campi: &["/policy", "/min_area"],
        },
        Caso {
            op: "geo.scale",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"x_factor": 2.0, "y_factor": 2.0}),
            campi: &["/x_origin", "/y_origin"],
        },
        Caso {
            op: "geo.rotate",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"degrees": 10.0}),
            campi: &["/x_origin", "/y_origin"],
        },
        Caso {
            op: "geo.concave_hull",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"concavity": 2.0}),
            campi: &["/length_threshold"],
        },
        Caso {
            op: "geo.voronoi",
            tabelle: vec![punti()],
            crs: None,
            base: json!({}),
            campi: &["/max_points"],
        },
        Caso {
            op: "geo.polygonize",
            tabelle: vec![linee()],
            crs: None,
            base: json!({}),
            campi: &["/node_input", "/require_complete"],
        },
        Caso {
            op: "geo.from_coords",
            tabelle: vec![coordinate()],
            crs: Some(UTM),
            base: json!({}),
            campi: &["/x_column", "/y_column", "/geometry_column", "/crs"],
        },
        Caso {
            op: "geo.from_wkt",
            tabelle: vec![coordinate()],
            crs: Some(UTM),
            base: json!({"wkt_column": "wkt"}),
            campi: &["/output_column", "/on_error", "/crs"],
        },
        Caso {
            op: "geo.distance",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"other_wkb": punto}),
            campi: &["/output_column"],
        },
        Caso {
            op: "geo.split",
            tabelle: vec![linee()],
            crs: None,
            base: json!({"other_wkb": lama}),
            campi: &["/tolerance"],
        },
        Caso {
            op: "geo.nearest",
            tabelle: vec![punti(), punti()],
            crs: None,
            base: json!({}),
            campi: &["/max_distance"],
        },
        Caso {
            op: "geo.geometry_accessors",
            tabelle: vec![linee()],
            crs: None,
            base: json!({}),
            campi: &["/fields", "/output_prefix"],
        },
        Caso {
            op: "geo.line_locate_point",
            tabelle: vec![linee()],
            crs: None,
            base: json!({"point_wkb": punto}),
            campi: &["/output_column"],
        },
        Caso {
            op: "geo.generate_grid",
            tabelle: vec![tabellare()],
            crs: Some(UTM),
            base: json!({"extent": estensione, "cell_size": 5.0}),
            campi: &["/shape", "/crs", "/include_centroid"],
        },
        Caso {
            op: "geo.subdivide",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"max_vertices": 8}),
            campi: &["/output_column"],
        },
        Caso {
            op: "geo.coverage_validate",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({}),
            campi: &["/tolerance", "/max_issues"],
        },
        Caso {
            op: "geo.shared_paths",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({}),
            campi: &["/tolerance", "/min_length"],
        },
        Caso {
            op: "geo.cluster_dbscan",
            tabelle: vec![punti()],
            crs: None,
            base: json!({"eps": 1.0, "min_points": 1}),
            campi: &["/output_column"],
        },
        // La regola dei parametri senza effetto di `geo.reproject`: un
        // parametro scritto `null` non la aggira.
        Caso {
            op: "geo.reproject",
            tabelle: vec![poligoni()],
            crs: None,
            base: json!({"target_crs": "EPSG:4326"}),
            campi: &[
                "/accuratezza_accettata_m",
                "/trasformazioni",
                "/convenzione_wgs84_etrs89",
            ],
        },
    ]);
}
