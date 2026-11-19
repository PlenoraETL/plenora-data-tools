//! C1: GOP-004/005/007/028 e G1/G2, come registrati in GEO-DENSIFY.md.
#![allow(clippy::float_cmp, reason = "Conservazione esatta degli originali")]
use super::*;
use geo::{Geometry, MultiLineString, MultiPolygon, Point, Polygon};
use plenora_core::crs::resolve_crs;

fn limits() -> GeodesicGeometryDensifyLimits {
    GeodesicGeometryDensifyLimits {
        output: GeodesicDensifyLimits {
            max_points: 100_000,
            max_reserved_bytes: 20_000_000,
            deadline: None,
        },
        max_validation_work: 2_000_000,
    }
}

fn run(input: &Geometry, maximum: f64) -> Result<Geometry, GeodesicDensifyError> {
    geodesic_densify_geometry_with_limits(
        input,
        &resolve_crs("EPSG:4326", "crs").unwrap(),
        maximum,
        limits(),
        None,
    )
}

fn rectangle(west: f64, south: f64, east: f64, north: f64) -> LineString {
    LineString::from(vec![
        (west, south),
        (east, south),
        (east, north),
        (west, north),
        (west, south),
    ])
}

fn check_ring(input: &LineString, output: &LineString, maximum: f64) {
    // GOP-007: originali come sottosequenza ordinata e chiusura identica.
    let mut remaining = output.0.iter();
    for point in &input.0 {
        assert!(remaining.any(|candidate| candidate == point));
    }
    assert_eq!(input.0.first(), output.0.first());
    assert_eq!(input.0.last(), output.0.last());
    let ellipsoid = crate::geodetica::wgs84_di_prova();
    for pair in output.0.windows(2) {
        // G1 inverse di Karney: verifica indipendente del massimo GOP-007.
        let distance: f64 = ellipsoid
            .geodetica()
            .inverse(pair[0].y, pair[0].x, pair[1].y, pair[1].x);
        assert!(distance <= maximum);
    }
    // Oracolo differenziale: stessa primitiva del ramo percorsi, anello per anello.
    let expected = geodesic_densify_with_limits(
        std::slice::from_ref(input),
        &resolve_crs("EPSG:4326", "crs").unwrap(),
        maximum,
        limits().output,
        None,
    )
    .unwrap();
    assert_eq!(output, &expected[0]);
}

fn check_polygon(input: &Polygon, output: &Polygon, maximum: f64) {
    check_ring(input.exterior(), output.exterior(), maximum);
    assert_eq!(input.interiors().len(), output.interiors().len());
    for (a, b) in input.interiors().iter().zip(output.interiors()) {
        check_ring(a, b, maximum);
    }
}

#[test]
fn polygon_holes_and_multipart_keep_roles_and_order() {
    // GOP-005: rettangoli piccoli disgiunti, buchi strettamente interni.
    let polygon = Polygon::new(
        rectangle(-4.0, -4.0, 4.0, 4.0),
        vec![
            rectangle(-3.0, -1.0, -1.0, 1.0),
            rectangle(1.0, -1.0, 3.0, 1.0),
        ],
    );
    let maximum = 100_000.0;
    let Geometry::Polygon(output) = run(&Geometry::Polygon(polygon.clone()), maximum).unwrap()
    else {
        panic!("tipo di uscita errato");
    };
    check_polygon(&polygon, &output, maximum);
    let parts = MultiPolygon(vec![
        Polygon::new(rectangle(20.0, 10.0, 22.0, 12.0), vec![]),
        Polygon::empty(),
        polygon,
    ]);
    let Geometry::MultiPolygon(output) =
        run(&Geometry::MultiPolygon(parts.clone()), maximum).unwrap()
    else {
        panic!("tipo di uscita errato");
    };
    // GOP-007: ordine delle parti, incluso il poligono vuoto intermedio.
    assert_eq!(parts.0.len(), output.0.len());
    for (a, b) in parts.0.iter().zip(&output.0) {
        check_polygon(a, b, maximum);
    }
}

#[test]
fn seam_polar_caps_and_traversal_select_the_minor_region() {
    // GOP-004/005: seam continuo; per simmetria i quattro archi a 80 N
    // racchiudono il polo e il corrispondente anello a 85 N ne e' un buco.
    let cap = |latitude| {
        LineString::from(vec![
            (0.0, latitude),
            (90.0, latitude),
            (180.0, latitude),
            (-90.0, latitude),
            (0.0, latitude),
        ])
    };
    for polygon in [
        Polygon::new(
            rectangle(178.0, -3.0, -178.0, 3.0),
            vec![rectangle(179.0, -1.0, -179.0, 1.0)],
        ),
        Polygon::new(cap(80.0), vec![cap(85.0)]),
    ] {
        for reverse_shell in [false, true] {
            for reverse_holes in [false, true] {
                let mut shell = polygon.exterior().clone();
                if reverse_shell {
                    shell.0.reverse();
                }
                let holes = polygon
                    .interiors()
                    .iter()
                    .map(|ring| {
                        let mut ring = ring.clone();
                        if reverse_holes {
                            ring.0.reverse();
                        }
                        ring
                    })
                    .collect();
                let input = Polygon::new(shell, holes);
                let Geometry::Polygon(output) =
                    run(&Geometry::Polygon(input.clone()), 200_000.0).unwrap()
                else {
                    panic!("tipo di uscita errato");
                };
                // GOP-005: il verso cambia il segno, non la regione minore.
                check_polygon(&input, &output, 200_000.0);
            }
        }
    }
}

#[test]
fn ambiguous_hemisphere_crossings_and_invalid_holes_are_typed() {
    // G1 simmetria dell'ellissoide: l'equatore delimita due emisferi uguali.
    // Anche una perturbazione sotto il margine certificato resta ambigua.
    for latitude in [0.0, 1e-14, -1e-14] {
        let ring = LineString::from(vec![
            (0.0, latitude),
            (90.0, latitude),
            (180.0, latitude),
            (-90.0, latitude),
            (0.0, latitude),
        ]);
        assert_eq!(
            run(&Geometry::Polygon(Polygon::new(ring, vec![])), 1e6),
            Err(GeodesicDensifyError::UnsupportedSurface)
        );
    }
    // GOP-005: incrocio trasverso, contatto e buchi annidati esclusi.
    for polygon in [
        Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (3.0, 0.0),
                (0.0, 0.0),
            ]),
            vec![],
        ),
        Polygon::new(
            rectangle(0.0, 0.0, 4.0, 4.0),
            vec![rectangle(0.0, 1.0, 2.0, 2.0)],
        ),
    ] {
        assert_eq!(
            run(&Geometry::Polygon(polygon), 1e6),
            Err(GeodesicDensifyError::UnsupportedSurface)
        );
    }
    // GOP-002/005: un buco fuori dal guscio e' geometria malformata.
    let outside = Polygon::new(
        rectangle(0.0, 0.0, 2.0, 2.0),
        vec![rectangle(4.0, 4.0, 5.0, 5.0)],
    );
    assert_eq!(
        run(&Geometry::Polygon(outside), 1e6),
        Err(GeodesicDensifyError::InvalidGeometry)
    );
    let no_shell = Polygon::new(LineString::new(vec![]), vec![rectangle(0.0, 0.0, 1.0, 1.0)]);
    assert_eq!(
        run(&Geometry::Polygon(no_shell), 1e6),
        Err(GeodesicDensifyError::InvalidGeometry)
    );
}

#[test]
fn geometry_empty_linear_and_unsupported_domains() {
    // GOP-007: vuoti tipizzati, parti vuote e dominio dei quattro tipi.
    for input in [
        Geometry::LineString(LineString::new(vec![])),
        Geometry::MultiLineString(MultiLineString(vec![])),
        Geometry::Polygon(Polygon::empty()),
        Geometry::MultiPolygon(MultiPolygon(vec![])),
    ] {
        assert_eq!(run(&input, 1.0).unwrap(), input);
    }
    let lines = MultiLineString(vec![LineString::new(vec![]), rectangle(0.0, 0.0, 1.0, 1.0)]);
    let Geometry::MultiLineString(output) =
        run(&Geometry::MultiLineString(lines.clone()), 100_000.0).unwrap()
    else {
        panic!("tipo di uscita errato");
    };
    for (a, b) in lines.0.iter().zip(&output.0) {
        check_ring(a, b, 100_000.0);
    }
    for input in [
        Geometry::Point(Point::new(0.0, 0.0)),
        Geometry::GeometryCollection(geo::GeometryCollection::empty()),
        Geometry::MultiPoint(geo::MultiPoint::new(vec![])),
        Geometry::Line(geo::Line::new((0.0, 0.0), (1.0, 1.0))),
        Geometry::Rect(geo::Rect::new((0.0, 0.0), (1.0, 1.0))),
        Geometry::Triangle(geo::Triangle::new(
            (0.0, 0.0).into(),
            (1.0, 0.0).into(),
            (0.0, 1.0).into(),
        )),
    ] {
        assert_eq!(
            run(&input, 1.0),
            Err(GeodesicDensifyError::UnsupportedGeometry)
        );
    }
}

#[test]
fn geometry_limits_and_interruption_precedence() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let input = Geometry::Polygon(Polygon::new(rectangle(0.0, 0.0, 1.0, 1.0), vec![]));
    let invoke =
        |budget, flag| geodesic_densify_geometry_with_limits(&input, &crs, 1e6, budget, flag);
    let mut budget = limits();
    // GOP-026: cinque coordinate incluse chiusura, soglia esatta accettata.
    budget.output.max_points = 5;
    assert_eq!(invoke(budget, None).unwrap(), input);
    budget.output.max_points = 4;
    assert_eq!(invoke(budget, None), Err(GeodesicDensifyError::PointLimit));
    budget = limits();
    budget.max_validation_work = 0;
    assert_eq!(invoke(budget, None), Err(GeodesicDensifyError::WorkLimit));
    budget = limits();
    budget.output.max_reserved_bytes = 0;
    assert_eq!(invoke(budget, None), Err(GeodesicDensifyError::MemoryLimit));
    // GOP-028: deadline prevale sul flag al medesimo checkpoint.
    let flag = AtomicBool::new(true);
    assert_eq!(
        invoke(limits(), Some(&flag)),
        Err(GeodesicDensifyError::Cancelled)
    );
    budget = limits();
    budget.output.deadline = Some(Instant::now());
    assert_eq!(
        invoke(budget, None),
        Err(GeodesicDensifyError::DeadlineExceeded)
    );
    assert_eq!(
        invoke(budget, Some(&flag)),
        Err(GeodesicDensifyError::DeadlineExceeded)
    );
    // G1: soglia equatoriale analitica, identica alla primitiva dei percorsi.
    let length = 6_378_137.0 * 1.0_f64.to_radians();
    let line = Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 0.0)]));
    for (maximum, points) in [(length, 3), (length + 2e-6, 2), (length / 2.0, 4)] {
        let Geometry::LineString(output) = run(&line, maximum).unwrap() else {
            panic!("tipo di uscita errato");
        };
        assert_eq!(output.0.len(), points);
    }
}

#[test]
fn black_box_interrupts_during_large_emission() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let input = Geometry::LineString(LineString::from(vec![(0.0, 0.0), (20.0, 0.0)]));
    let mut budget = limits();
    budget.output.max_points = 3_000_000;
    budget.output.max_reserved_bytes = 64_000_000;
    let flag = AtomicBool::new(false);
    // GOP-028: flag modificato dall'esterno, senza callback o sonde del kernel.
    // Il timer non e' un limite di latenza: l'attesa accerta solo il rifiuto.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(5));
            flag.store(true, Ordering::Relaxed);
        });
        assert_eq!(
            geodesic_densify_geometry_with_limits(&input, &crs, 10.0, budget, Some(&flag)),
            Err(GeodesicDensifyError::Cancelled)
        );
    });
    budget.output.deadline = Some(Instant::now() + std::time::Duration::from_millis(5));
    assert_eq!(
        geodesic_densify_geometry_with_limits(&input, &crs, 10.0, budget, None),
        Err(GeodesicDensifyError::DeadlineExceeded)
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn independent_reader_small_surfaces_and_steps() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    // t_dens2.py: rettangoli circa 79x111 m, 4x5.5 m e 0.8x1.1 m.
    for (width, maximum) in [
        (0.001, 0.02),
        (0.001, 0.003),
        (0.00005, 0.5),
        (0.00001, 10.0),
    ] {
        let ring = rectangle(10.0, 45.0, 10.0 + width, 45.0 + width);
        let input = Geometry::Polygon(Polygon::new(ring.clone(), vec![]));
        let budget = GeodesicGeometryDensifyLimits {
            output: GeodesicDensifyLimits {
                max_points: 200_000,
                max_reserved_bytes: 200_000_000,
                deadline: None,
            },
            max_validation_work: 100_000_000,
        };
        let Geometry::Polygon(output) =
            geodesic_densify_geometry_with_limits(&input, &crs, maximum, budget, None).unwrap()
        else {
            panic!("tipo inatteso")
        };
        assert_eq!(output.exterior().0.first(), ring.0.first());
        assert_eq!(output.exterior().0.last(), ring.0.last());
        let expected =
            geodesic_densify_with_limits(&[ring], &crs, maximum, budget.output, None).unwrap();
        assert_eq!(output.exterior(), &expected[0]);
    }
}

#[test]
#[deny(clippy::disallowed_methods)]
fn nested_holes_are_invalid_geometry() {
    let polygon = Polygon::new(
        rectangle(0.0, 0.0, 10.0, 10.0),
        vec![rectangle(2.0, 2.0, 8.0, 8.0), rectangle(4.0, 4.0, 6.0, 6.0)],
    );
    assert_eq!(
        run(&polygon.into(), 1e6),
        Err(GeodesicDensifyError::InvalidGeometry)
    );
}

#[test]
#[deny(clippy::disallowed_methods)]
fn close_edges_and_uncertifiable_area_have_distinct_categories() {
    // Lati opposti a circa 2 mm: il GAP fisso non e' un limite geometrico.
    let close = Polygon::new(rectangle(10.0, 45.0, 10.000_000_25, 45.000_000_018), vec![]);
    let output = run(&close.clone().into(), 10.0).unwrap();
    assert_eq!(output, close.into());
    // Sotto il certificato delle primitive non si dichiara un incrocio.
    let tiny = Polygon::new(
        rectangle(10.0, 45.0, 10.000_000_001, 45.000_000_001),
        vec![],
    );
    assert_eq!(
        run(&tiny.into(), 10.0),
        Err(GeodesicDensifyError::AccuracyUnproven)
    );
    let touch = Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (2.0, 0.0),
            (0.0, 4.0),
            (0.0, 0.0),
        ]),
        vec![],
    );
    assert_eq!(
        run(&touch.into(), 1e6),
        Err(GeodesicDensifyError::UnsupportedSurface)
    );
    // t_surf.py: bowtie simmetrico, area nulla ma incrocio dimostrato.
    let bowtie = Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (2.0, 2.0),
            (2.0, 0.0),
            (0.0, 2.0),
            (0.0, 0.0),
        ]),
        vec![],
    );
    assert_eq!(
        run(&bowtie.into(), 1e6),
        Err(GeodesicDensifyError::UnsupportedSurface)
    );
}

#[test]
fn resource_prefixes_preserve_polygon_vertices_and_roles() {
    let crs = resolve_crs("EPSG:4326", "crs").unwrap();
    let input = Geometry::Polygon(Polygon::new(rectangle(0.0, 0.0, 2.0, 2.0), vec![]));
    // GOP-007: ogni lato e' minore di 1000 km, quindi nessun vertice
    // aggiunto; GOP-005 conserva guscio, chiusura e verso.
    for memory in [false, true] {
        let mut succeeded = false;
        for budget in (0..128).chain((7..=23).map(|power| 1_u64 << power)) {
            let mut bounded = limits();
            if memory {
                bounded.output.max_reserved_bytes = budget;
            } else {
                bounded.max_validation_work = budget;
            }
            match geodesic_densify_geometry_with_limits(&input, &crs, 1_000_000.0, bounded, None) {
                Ok(actual) => {
                    assert_eq!(actual, input);
                    succeeded = true;
                }
                Err(error) => {
                    // GOP-026: include anche la validazione dell'output.
                    assert!(!succeeded);
                    assert_eq!(
                        error,
                        if memory {
                            GeodesicDensifyError::MemoryLimit
                        } else {
                            GeodesicDensifyError::WorkLimit
                        }
                    );
                }
            }
        }
        assert!(succeeded);
    }
}
