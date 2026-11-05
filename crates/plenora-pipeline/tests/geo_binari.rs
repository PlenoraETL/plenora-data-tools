//! Operazioni geo su due tabelle nel runner: join, ritaglio, overlay e
//! booleane uguali ai kernel, con la semantica delle righe dichiarata nel
//! docs/runner.md («Operazioni geo»).

// Un caso per operazione; `_sx`/`_dx` sono coppie volute.
#![allow(clippy::too_many_lines, clippy::similar_names)]

mod comune_geo;

use geo::{CoordsIter, Geometry};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, UInt64Array,
};
use plenora_core::crs::resolve_crs;
use plenora_core::ErrorCategory;
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::spatial_join::JoinPredicate;
use plenora_kernels_geo::topology::{BooleanOperation, OverlayMode};
use plenora_kernels_geo::{analysis, spatial_join, topology};
use serde_json::json;

use comune_geo::{
    geometrie, poligoni, punti, punti_lonlat, quadrato, stessa_validazione, tabella, un_passo, wkb,
    UTM, X0, Y0,
};

fn centimetro() -> Precision {
    Precision::from_crs(&resolve_crs(UTM, "crs").expect("crs")).expect("precisione")
}

fn righe_massime() -> u64 {
    plenora_core::limits::Limits::default().rows.max_output_rows
}

fn colonna<'a>(tabella: &'a RecordBatch, nome: &str) -> &'a ArrayRef {
    tabella
        .column_by_name(nome)
        .unwrap_or_else(|| panic!("colonna `{nome}` assente"))
}

fn binaria(celle: &[Option<Vec<u8>>]) -> BinaryArray {
    celle.iter().map(Option::as_deref).collect()
}

/// Maschere/destra: due quadrati, uno sovrapposto ai poligoni di sinistra.
fn maschere() -> RecordBatch {
    tabella(
        UTM,
        &[
            Some(Geometry::Polygon(quadrato(X0 + 20.0, Y0 + 20.0, 60.0))),
            None,
            Some(Geometry::Polygon(quadrato(X0 + 510.0, Y0 + 10.0, 5.0))),
            Some(Geometry::Polygon(quadrato(X0 + 900.0, Y0 + 900.0, 5.0))),
        ],
    )
}

#[test]
fn i_join_sono_i_kernel() {
    let sinistra = poligoni();
    let destra = punti();
    let sx = geometrie(&sinistra, "geometry");
    let dx = geometrie(&destra, "geometry");

    let uscita = un_passo_verificato(
        "geo.sjoin",
        json!({"predicate": "intersects"}),
        &[sinistra.clone(), destra.clone()],
    )
    .unwrap();
    let coppie =
        spatial_join::spatial_join_nullable(&sx, &dx, JoinPredicate::Intersects, u64::MAX).unwrap();
    assert_eq!(
        coppie.iter().map(|c| (c.left, c.right)).collect::<Vec<_>>(),
        [(0, 0), (0, 1)]
    );
    let id: Int64Array = coppie
        .iter()
        .map(|c| Some(i64::try_from(c.left).unwrap()))
        .collect();
    assert_eq!(colonna(&uscita, "id").as_ref(), &id as &dyn Array);
    let destre: UInt64Array = coppie.iter().map(|c| Some(c.right)).collect();
    assert_eq!(
        colonna(&uscita, "__right_index").as_ref(),
        &destre as &dyn Array
    );

    let uscita = un_passo_verificato(
        "geo.nearest",
        json!({"max_distance": 1000.0}),
        &[sinistra.clone(), destra.clone()],
    )
    .unwrap();
    let trovati = analysis::nearest_matches(&sx, &dx, Some(1000.0), u64::MAX, u64::MAX).unwrap();
    assert_eq!(
        trovati
            .iter()
            .map(|m| (m.left, m.right))
            .collect::<Vec<_>>(),
        [(0, 0), (0, 1), (2, 1), (3, 4)]
    );
    let attese = [0.0, 0.0, 40.0_f64.hypot(47.0), 95.0_f64.hypot(22.0)];
    for (m, attesa) in trovati.iter().zip(attese) {
        assert!((m.distance - attesa).abs() < 1e-9);
    }
    let distanze: Float64Array = trovati.iter().map(|m| Some(m.distance)).collect();
    assert_eq!(
        colonna(&uscita, "distance").as_ref(),
        &distanze as &dyn Array
    );
    let destre: UInt64Array = trovati.iter().map(|m| Some(m.right)).collect();
    assert_eq!(
        colonna(&uscita, "__right_index").as_ref(),
        &destre as &dyn Array
    );

    // Allineate a left: `within` dei punti nei poligoni, conteggi dei punti.
    let uscita =
        un_passo_verificato("geo.within", json!({}), &[destra.clone(), sinistra.clone()]).unwrap();
    let dentro = analysis::within_indexes(&dx, &sx, u64::MAX).unwrap();
    assert_eq!(dentro, [1]);
    let attesi: BooleanArray = dx
        .iter()
        .enumerate()
        .map(|(riga, g)| {
            g.as_ref()
                .map(|_| dentro.contains(&u64::try_from(riga).unwrap()))
        })
        .collect();
    assert_eq!(colonna(&uscita, "within").as_ref(), &attesi as &dyn Array);
    assert_eq!(uscita.num_rows(), destra.num_rows());

    let uscita = un_passo_verificato(
        "geo.count_points_in_polygons",
        json!({"output_column": "n"}),
        &[sinistra, destra],
    )
    .unwrap();
    let conteggi = analysis::count_points_in_polygons(&sx, &dx, u64::MAX).unwrap();
    assert_eq!(conteggi, [1, 0, 0, 0]);
    let attesi: UInt64Array = sx
        .iter()
        .zip(&conteggi)
        .map(|(g, n)| g.as_ref().map(|_| *n))
        .collect();
    assert_eq!(colonna(&uscita, "n").as_ref(), &attesi as &dyn Array);
}

#[test]
fn ritaglio_overlay_e_booleane_sono_i_kernel() {
    let sinistra = poligoni();
    let destra = maschere();
    let sx = geometrie(&sinistra, "geometry");
    let dx = geometrie(&destra, "geometry");

    // `clip`: ogni riga di left contro l'unione di tutte le maschere.
    let uscita =
        un_passo_verificato("geo.clip", json!({}), &[sinistra.clone(), destra.clone()]).unwrap();
    let presenti: Vec<Geometry<f64>> = sx.iter().flatten().cloned().collect();
    let tutte_le_maschere: Vec<Geometry<f64>> = dx.iter().flatten().cloned().collect();
    let mut ritagliate = topology::clip_to_mask(&presenti, &tutte_le_maschere, centimetro())
        .unwrap()
        .into_iter();
    let attese: Vec<Option<Vec<u8>>> = sx
        .iter()
        .map(|g| {
            g.as_ref()
                .and_then(|_| ritagliate.next().flatten().map(|r| wkb(&r)))
        })
        .collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&attese) as &dyn Array
    );
    let ritagli = geometrie(&uscita, "geometry");
    for (riga, x, y, lato) in [
        (0, X0 + 20.0, Y0 + 20.0, 60.0),
        (2, X0 + 50.0, Y0 + 50.0, 30.0),
        (3, X0 + 510.0, Y0 + 10.0, 5.0),
    ] {
        comune_geo::assert_poligoni(
            ritagli[riga].as_ref().expect("ritaglio"),
            &Geometry::Polygon(quadrato(x, y, lato)),
        );
    }
    assert!(ritagli[1].is_none());
    assert_eq!(uscita.num_rows(), sinistra.num_rows());
    assert!(uscita
        .schema()
        .field_with_name("geometry")
        .unwrap()
        .is_nullable());

    // `overlay`: i pezzi, con le righe d'origine dei due lati.
    let uscita = un_passo_verificato(
        "geo.overlay",
        json!({"mode": "intersection"}),
        &[sinistra.clone(), destra.clone()],
    )
    .unwrap();
    let pezzi = topology::polygon_overlay(
        &presenti,
        &tutte_le_maschere,
        OverlayMode::Intersection,
        righe_massime(),
        righe_massime(),
        centimetro(),
    )
    .unwrap();
    let geometrie_pezzi: Vec<Option<Vec<u8>>> =
        pezzi.iter().map(|p| Some(wkb(&p.geometry))).collect();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        &binaria(&geometrie_pezzi) as &dyn Array
    );
    assert_eq!(
        pezzi.iter().map(|p| (p.left, p.right)).collect::<Vec<_>>(),
        [(Some(0), Some(0)), (Some(1), Some(0)), (Some(2), Some(1))]
    );
    for (pezzo, (x, y, lato)) in pezzi.iter().zip([
        (X0 + 20.0, Y0 + 20.0, 60.0),
        (X0 + 50.0, Y0 + 50.0, 30.0),
        (X0 + 510.0, Y0 + 10.0, 5.0),
    ]) {
        comune_geo::assert_poligoni(&pezzo.geometry, &Geometry::Polygon(quadrato(x, y, lato)));
    }
    // Le posizioni fra le righe non null riportate alle righe della tabella.
    let righe_sx: Vec<u64> = (0..sx.len())
        .filter(|riga| sx[*riga].is_some())
        .map(|riga| u64::try_from(riga).unwrap())
        .collect();
    let righe_dx: Vec<u64> = (0..dx.len())
        .filter(|riga| dx[*riga].is_some())
        .map(|riga| u64::try_from(riga).unwrap())
        .collect();
    let sinistre: UInt64Array = pezzi
        .iter()
        .map(|p| p.left.map(|i| righe_sx[usize::try_from(i).unwrap()]))
        .collect();
    let destre: UInt64Array = pezzi
        .iter()
        .map(|p| p.right.map(|i| righe_dx[usize::try_from(i).unwrap()]))
        .collect();
    assert_eq!(
        colonna(&uscita, "__left_index").as_ref(),
        &sinistre as &dyn Array
    );
    assert_eq!(
        colonna(&uscita, "__right_index").as_ref(),
        &destre as &dyn Array
    );

    // Booleane riga per riga: null dove uno dei due e' null o il
    // risultato e' vuoto.
    for (op, operazione) in [
        ("geo.intersection", BooleanOperation::Intersection),
        ("geo.union", BooleanOperation::Union),
        ("geo.difference", BooleanOperation::Difference),
        (
            "geo.symmetric_difference",
            BooleanOperation::SymmetricDifference,
        ),
    ] {
        let uscita =
            un_passo_verificato(op, json!({}), &[sinistra.clone(), destra.clone()]).unwrap();
        let attese: Vec<Option<Vec<u8>>> = sx
            .iter()
            .zip(&dx)
            .map(|coppia| match coppia {
                (Some(a), Some(b)) => {
                    let r = topology::boolean_operation(a, b, operazione, centimetro()).unwrap();
                    (r.coords_count() > 0).then(|| wkb(&r))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            colonna(&uscita, "geometry").as_ref(),
            &binaria(&attese) as &dyn Array,
            "{op}"
        );
        let obtenues = geometrie(&uscita, "geometry");
        for riga in [0_usize, 2, 3] {
            let Geometry::Polygon(a) = sx[riga].as_ref().unwrap() else {
                panic!("fixture")
            };
            let Geometry::Polygon(b) = dx[riga].as_ref().unwrap() else {
                panic!("fixture")
            };
            let attesa = match (op, riga) {
                ("geo.intersection", 0) => Some(Geometry::Polygon(b.clone())),
                ("geo.intersection", _) => None,
                ("geo.union", 0) | ("geo.difference", 2 | 3) => Some(Geometry::Polygon(a.clone())),
                ("geo.difference" | "geo.symmetric_difference", 0) => Some(Geometry::Polygon(
                    geo::Polygon::new(a.exterior().clone(), vec![b.exterior().clone()]),
                )),
                _ => Some(Geometry::MultiPolygon(geo::MultiPolygon(vec![
                    a.clone(),
                    b.clone(),
                ]))),
            };
            match (&obtenues[riga], attesa) {
                (Some(g), Some(a)) => comune_geo::assert_poligoni(g, &a),
                (None, None) => {}
                _ => panic!("null geometrico inatteso"),
            }
        }
        assert!(obtenues[1].is_none());
        assert_eq!(colonna(&uscita, "id"), colonna(&sinistra, "id"), "{op}");
    }
    // Un'intersezione vuota e' null (la geometria dell'uscita e' nullable).
    let uscita = un_passo_verificato("geo.intersection", json!({}), &[sinistra, destra]).unwrap();
    let celle = colonna(&uscita, "geometry");
    assert!(celle.is_null(3), "riga 3: quadrati disgiunti");
}

#[test]
fn le_booleane_chiedono_le_stesse_righe() {
    let errore = un_passo(
        "geo.intersection",
        json!({}),
        &[poligoni(), tabella(UTM, &[None])],
    )
    .unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
    assert!(errore.to_string().contains("stesse righe"), "{errore}");
}

#[test]
fn la_validazione_rifiuta_esattamente_cio_che_l_analisi_rifiuta() {
    use ErrorCategory::{Crs, InvalidPlan, Schema};
    let casi = vec![
        (
            "geo.sjoin",
            json!({"predicate": "within"}),
            vec![poligoni(), punti()],
            None,
        ),
        ("geo.sjoin", json!({}), vec![poligoni(), punti()], None),
        (
            "geo.sjoin",
            json!({"predicate": "equals"}),
            vec![poligoni(), punti()],
            None,
        ),
        (
            "geo.nearest",
            json!({"max_distance": -1.0}),
            vec![poligoni(), punti()],
            None,
        ),
        (
            "geo.within",
            json!({"output_column": "id"}),
            vec![punti(), poligoni()],
            None,
        ),
        (
            "geo.clip",
            json!({"extra": true}),
            vec![poligoni(), maschere()],
            None,
        ),
        (
            "geo.overlay",
            json!({"mode": "union"}),
            vec![poligoni(), maschere()],
            None,
        ),
        ("geo.overlay", json!({}), vec![poligoni(), maschere()], None),
        // Requisito CRS del catalogo: stesso CRS proiettato sui due lati.
        (
            "geo.union",
            json!({}),
            vec![poligoni(), punti_lonlat()],
            None,
        ),
        (
            "geo.count_points_in_polygons",
            json!({}),
            vec![poligoni(), punti()],
            None,
        ),
    ];
    stessa_validazione(
        &casi,
        &[
            None,
            Some(InvalidPlan),
            Some(InvalidPlan),
            Some(InvalidPlan),
            Some(Schema),
            Some(InvalidPlan),
            None,
            Some(InvalidPlan),
            Some(Crs),
            None,
        ],
    );
}

#[test]
fn un_join_si_concatena_con_le_tabellari() {
    let pipeline = comune_geo::piano(
        &["t", "u"],
        vec![
            comune_geo::passo(
                "coppie",
                "geo.sjoin",
                &["t", "u"],
                json!({"predicate": "intersects"}),
            ),
            comune_geo::passo(
                "per_poligono",
                "table.aggregate",
                &["coppie"],
                json!({"group_by": ["id"],
                       "aggregations": [{"column": "__right_index", "function": "count"}]}),
            ),
        ],
        &["per_poligono"],
    );
    let esito = comune_geo::esegui(&pipeline, &[("t", poligoni()), ("u", punti())]).unwrap();
    assert!(esito.outputs[0].1.num_rows() > 0);
}

/// Stesso piano, con schema atteso dichiarato indipendentemente dai kernel.
fn un_passo_verificato(
    op: &str,
    config: serde_json::Value,
    tabelle: &[RecordBatch],
) -> plenora_core::Result<RecordBatch> {
    let atteso = comune_geo::schema_geo_indipendente(op, &tabelle[0], &config, None);
    let risultato = un_passo(op, config, tabelle);
    if let Ok(output) = &risultato {
        assert_eq!(output.schema(), atteso, "schema indipendente {op}");
        let righe: Vec<u32> = match op {
            "geo.sjoin" => vec![0, 0],
            "geo.nearest" => vec![0, 0, 2, 3],
            _ => (0..u32::try_from(tabelle[0].num_rows()).expect("righe")).collect(),
        };
        if op != "geo.overlay" {
            comune_geo::assert_attributi(
                output,
                &tabelle[0],
                &righe,
                matches!(
                    op,
                    "geo.sjoin" | "geo.nearest" | "geo.within" | "geo.count_points_in_polygons"
                ),
            );
        }
    }
    risultato
}
