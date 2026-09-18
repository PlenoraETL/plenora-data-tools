use geo::{
    Geometry, GeometryCollection, LineString, MultiLineString, MultiPolygon, Polygon, Simplify,
};
use plenora_kernels_geo::{construction::geometry_from_wkt, operations};

fn linee_a_scale_diverse() -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString(vec![
        LineString::from(vec![(0.0, 0.0), (0.0, 1e-200), (1e-200, 0.0)]),
        LineString::from(vec![(1.0, 1.0), (2.0, 2.0)]),
    ]))
}

fn rifiuto_numerico(geometry: &Geometry<f64>, tolerance: f64) {
    // Il contatore pubblico esegue ensure_valid prima di contare.
    assert!(operations::vertex_count(geometry).is_ok());
    assert!(matches!(
        operations::simplify(geometry, tolerance),
        Err(operations::OperationError::Internal(
            "semplificazione: distanza non rappresentabile"
        ))
    ));
}

#[test]
fn simplify_segmento_subnormale_con_coordinate_finite() {
    rifiuto_numerico(&linee_a_scale_diverse(), 1e-220);
}

#[test]
fn controprova_vendor_non_presidiato_debug_e_release() {
    let Geometry::MultiLineString(lines) = linee_a_scale_diverse() else {
        unreachable!();
    };
    // Controllo isolato della dipendenza; le altre prove attraversano il
    // prodotto sullo stesso ingresso finito e validato.
    let result = std::panic::catch_unwind(|| lines.simplify(1e-220));
    if cfg!(debug_assertions) {
        assert!(result.is_err());
    } else {
        let simplified = result.unwrap();
        assert_eq!(
            simplified.0[0].0,
            vec![(0.0, 0.0).into(), (1e-200, 0.0).into()]
        );
        // Il vertice omesso dista 1e-200 dal segmento, oltre epsilon.
        assert_eq!(lines.0[0].0[1].y.to_bits(), 1e-200_f64.to_bits());
    }
}

#[test]
fn simplify_poligoni_a_scale_diverse_con_coordinate_finite() {
    let geometry = Geometry::MultiPolygon(MultiPolygon(vec![
        Polygon::new(
            LineString::from(vec![(0.0, 0.0), (0.0, 1e-200), (1e-200, 0.0), (0.0, 0.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (1.0, 1.0),
                (2.0, 1.0),
                (2.0, 2.0),
                (1.0, 2.0),
                (1.0, 1.0),
            ]),
            vec![],
        ),
    ]));
    rifiuto_numerico(&geometry, 1e-220);
}

#[test]
fn simplify_wkt_finito_attraversa_parser_e_validazione() {
    let geometry = geometry_from_wkt("MULTILINESTRING((0 0,0 1e-200,1e-200 0),(1 1,2 2))").unwrap();
    rifiuto_numerico(&geometry, 1.0);
}

#[test]
fn simplify_collezione_non_restituisce_componenti_parziali() {
    let geometry = Geometry::GeometryCollection(GeometryCollection(vec![
        Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 1.0)])),
        linee_a_scale_diverse(),
    ]));
    rifiuto_numerico(&geometry, 0.1);
}

#[test]
fn simplify_tolleranza_zero_non_calcola_distanze() {
    let geometry = linee_a_scale_diverse();
    assert_eq!(operations::simplify(&geometry, 0.0).unwrap(), geometry);
}

#[test]
fn simplify_componente_minuscola_normalizzata_resta_ammessa() {
    let geometry = Geometry::LineString(LineString::from(vec![
        (0.0, 0.0),
        (0.0, 1e-200),
        (1e-200, 0.0),
    ]));
    assert!(operations::simplify(&geometry, 1e-220).is_ok());
}

#[test]
fn simplify_ordinario_conserva_oracolo_geo_e_spareggi() {
    for count in 3..40 {
        let line = LineString::from(
            (0..count)
                .map(|index| (f64::from(index), f64::from((index * 7) % 5)))
                .collect::<Vec<_>>(),
        );
        for tolerance in [0.0, 0.1, 1.0, 2.0, 10.0] {
            assert_eq!(
                operations::simplify(&Geometry::LineString(line.clone()), tolerance).unwrap(),
                Geometry::LineString(line.simplify(tolerance)),
            );
        }
    }
}
