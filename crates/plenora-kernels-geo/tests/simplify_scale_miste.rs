use geo::{
    Geometry, GeometryCollection, LineString, MultiLineString, MultiPolygon, Polygon, Simplify,
};
use plenora_kernels_geo::construction::with_crs::geometry_from_wkt;
use plenora_kernels_geo::crs::CrsContext;
use plenora_kernels_geo::operations;

fn linee_a_scale_diverse() -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString(vec![
        LineString::from(vec![(0.0, 0.0), (0.0, 1e-200), (1e-200, 0.0)]),
        LineString::from(vec![(1.0, 1.0), (2.0, 2.0)]),
    ]))
}

fn rifiuto_numerico(geometry: &Geometry<f64>, tolerance: f64, vertici: u64) {
    // Il contatore pubblico esegue ensure_valid prima di contare.
    assert_eq!(
        (operations::with_crs::vertex_count(
            CrsContext::new(&[
                &plenora_core::crs::resolve_crs("EPSG:3857", "crs").expect("CRS fixture metrico")
            ]),
            geometry
        )
        .map_err(kernel_error_fixture))
        .expect("successo atteso"),
        vertici
    );
    assert!(matches!(
        operations::with_crs::simplify(
            CrsContext::new(&[
                &plenora_core::crs::resolve_crs("EPSG:3857", "crs").expect("CRS fixture metrico")
            ]),
            geometry,
            tolerance
        )
        .map_err(kernel_error_fixture),
        Err(operations::OperationError::Internal(
            "semplificazione: distanza non rappresentabile"
        ))
    ));
}

#[test]
fn simplify_segmento_subnormale_con_coordinate_finite() {
    rifiuto_numerico(&linee_a_scale_diverse(), 1e-220, 5);
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "Controprova isolata del vendor: in debug il panico è obbligatorio, in release si confronta il risultato completo. Il prodotto ha prove protette distinte."
)]
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
    rifiuto_numerico(&geometry, 1e-220, 9);
}

#[test]
fn simplify_wkt_finito_attraversa_parser_e_validazione() {
    let geometry = geometry_from_wkt(
        CrsContext::new(&[
            &plenora_core::crs::resolve_crs("EPSG:3857", "crs").expect("CRS fixture metrico")
        ]),
        "MULTILINESTRING((0 0,0 1e-200,1e-200 0),(1 1,2 2))",
    )
    .map_err(kernel_error_fixture)
    .unwrap();
    rifiuto_numerico(&geometry, 1.0, 5);
}

#[test]
fn simplify_collezione_non_restituisce_componenti_parziali() {
    let geometry = Geometry::GeometryCollection(GeometryCollection(vec![
        Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 1.0)])),
        linee_a_scale_diverse(),
    ]));
    rifiuto_numerico(&geometry, 0.1, 7);
}

#[test]
fn simplify_tolleranza_zero_non_calcola_distanze() {
    let geometry = linee_a_scale_diverse();
    assert_eq!(
        operations::with_crs::simplify(
            CrsContext::new(&[
                &plenora_core::crs::resolve_crs("EPSG:3857", "crs").expect("CRS fixture metrico")
            ]),
            &geometry,
            0.0
        )
        .map_err(kernel_error_fixture)
        .unwrap(),
        geometry
    );
}

#[test]
fn simplify_componente_minuscola_normalizzata_resta_ammessa() {
    let geometry = Geometry::LineString(LineString::from(vec![
        (0.0, 0.0),
        (0.0, 1e-200),
        (1e-200, 0.0),
    ]));
    assert_eq!(
        (operations::with_crs::simplify(
            CrsContext::new(&[
                &plenora_core::crs::resolve_crs("EPSG:3857", "crs").expect("CRS fixture metrico")
            ]),
            &geometry,
            1e-220
        )
        .map_err(kernel_error_fixture))
        .expect("successo atteso"),
        geometry
    );
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
                operations::with_crs::simplify(
                    CrsContext::new(&[&plenora_core::crs::resolve_crs("EPSG:3857", "crs")
                        .expect("CRS fixture metrico")]),
                    &Geometry::LineString(line.clone()),
                    tolerance
                )
                .map_err(kernel_error_fixture)
                .unwrap(),
                Geometry::LineString(line.simplify(tolerance)),
            );
        }
    }
}

// Un rifiuto CRS inatteso deve far fallire la fixture, senza mascherare l'oracolo del kernel.
fn kernel_error_fixture<E>(error: plenora_kernels_geo::crs::DirectError<E>) -> E {
    match error {
        plenora_kernels_geo::crs::DirectError::Kernel(error) => error,
        plenora_kernels_geo::crs::DirectError::Crs(_) => panic!("CRS della fixture rifiutato"),
    }
}
