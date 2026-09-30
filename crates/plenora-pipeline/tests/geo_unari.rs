//! Operazioni geo 1:1 nel runner: ogni operazione uguale alla chiamata
//! diretta del kernel, validazione allineata all'analisi, CRS e dominio,
//! catene con le tabellari, budget.

mod comune_geo;

use std::sync::Arc;

use geo::{Geometry, LineString, Point, Polygon};
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::arrow_schema::arrow_schema_from_contract;
use plenora_core::crs::resolve_crs;
use plenora_core::{ErrorCategory, PlenoraError};
use plenora_kernels_geo::extended_algorithms::GeometryDiagnostics;
use plenora_kernels_geo::operations::{BufferCapStyle, SimplifyPolicy};
use plenora_kernels_geo::predicates::SpatialPredicate;
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::{
    extended, extended_algorithms, extensions, operations, predicates, Operation,
};
use serde_json::{json, Value};

use comune_geo::{
    analisi, attese_geo, attesi_f64, con_budget, esadecimale_di, esegui, geometrie, linee,
    linee_lonlat, multipunti, passo, piano, poligoni, poligoni_lonlat, punti, punti_lonlat,
    quadrato, stesso_errore, tabella, un_passo, un_passo_con_crs, wkb, LONLAT, UTM, X0, Y0,
};

fn centimetro_utm() -> Precision {
    Precision::from_crs(&resolve_crs(UTM, "crs").expect("crs")).expect("precisione")
}

fn colonna<'a>(tabella: &'a RecordBatch, nome: &str) -> &'a ArrayRef {
    tabella
        .column_by_name(nome)
        .unwrap_or_else(|| panic!("colonna `{nome}` assente"))
}

fn linea(g: &Geometry<f64>) -> &LineString<f64> {
    match g {
        Geometry::LineString(linea) => linea,
        _ => panic!("fixture non lineare"),
    }
}

fn punto(g: &Geometry<f64>) -> Point<f64> {
    match g {
        Geometry::Point(punto) => *punto,
        _ => panic!("fixture non puntuale"),
    }
}

/// Le trasformazioni in place: la colonna geometria del runner e' quella
/// del kernel riga per riga, gli attributi restano quelli dell'ingresso.
#[test]
#[allow(clippy::too_many_lines)] // Un caso per operazione, in un solo elenco.
fn le_trasformazioni_sono_i_kernel() {
    type Oracolo = Box<dyn Fn(&Geometry<f64>) -> Option<Geometry<f64>>>;
    let casi: Vec<(&str, Value, RecordBatch, Oracolo)> = vec![
        (
            "geo.centroid",
            json!({}),
            poligoni(),
            Box::new(|g| {
                Some(plenora_kernels_geo::transform_geometry(Operation::Centroid, g).unwrap())
            }),
        ),
        (
            "geo.convex_hull",
            json!({}),
            multipunti(),
            Box::new(|g| {
                Some(plenora_kernels_geo::transform_geometry(Operation::ConvexHull, g).unwrap())
            }),
        ),
        (
            "geo.envelope",
            json!({}),
            linee(),
            Box::new(|g| {
                Some(plenora_kernels_geo::transform_geometry(Operation::Envelope, g).unwrap())
            }),
        ),
        (
            "geo.boundary",
            json!({}),
            poligoni(),
            Box::new(|g| Some(operations::boundary(g).unwrap())),
        ),
        (
            "geo.point_on_surface",
            json!({}),
            poligoni(),
            Box::new(|g| operations::point_on_surface(g).unwrap()),
        ),
        (
            "geo.buffer",
            json!({"distance": 5.0, "cap": "flat"}),
            linee(),
            Box::new(|g| {
                Some(
                    operations::buffer_with_cap(g, 5.0, BufferCapStyle::Flat, centimetro_utm())
                        .unwrap(),
                )
            }),
        ),
        (
            "geo.buffer",
            json!({"distance": 3.0}),
            poligoni(),
            Box::new(|g| {
                Some(
                    operations::buffer_with_cap(g, 3.0, BufferCapStyle::Round, centimetro_utm())
                        .unwrap(),
                )
            }),
        ),
        (
            "geo.simplify",
            json!({"tolerance": 10.0, "policy": "preserve_topology"}),
            linee(),
            Box::new(|g| {
                Some(
                    operations::simplify_with_policy(g, 10.0, SimplifyPolicy::PreserveTopology)
                        .unwrap(),
                )
            }),
        ),
        (
            "geo.affine_transform",
            json!({"coefficients": [1.0, 0.0, 0.0, 1.0, 5.0, -5.0]}),
            poligoni(),
            Box::new(|g| {
                Some(extended::affine_transform(g, [1.0, 0.0, 0.0, 1.0, 5.0, -5.0]).unwrap())
            }),
        ),
        (
            "geo.translate",
            json!({"x_offset": 10.0, "y_offset": -3.0}),
            punti(),
            Box::new(|g| Some(extended::translate(g, 10.0, -3.0).unwrap())),
        ),
        (
            "geo.scale",
            json!({"x_factor": 1.5, "y_factor": 1.0, "x_origin": X0, "y_origin": Y0}),
            poligoni(),
            Box::new(|g| Some(extended::scale_about(g, 1.5, 1.0, Point::new(X0, Y0)).unwrap())),
        ),
        (
            "geo.rotate",
            json!({"degrees": 30.0, "x_origin": X0, "y_origin": Y0}),
            linee(),
            Box::new(|g| Some(extended::rotate_about(g, 30.0, Point::new(X0, Y0)).unwrap())),
        ),
        (
            "geo.concave_hull",
            json!({"concavity": 2.0}),
            multipunti(),
            Box::new(|g| Some(extended::concave_hull(g, 2.0, 0.0, u64::MAX).unwrap())),
        ),
        (
            "geo.densify",
            json!({"max_segment_length": 25.0}),
            linee(),
            Box::new(|g| Some(extended_algorithms::densify(g, 25.0, u64::MAX).unwrap())),
        ),
        (
            "geo.snap_to_grid",
            json!({"grid_size": 4.0}),
            linee(),
            Box::new(|g| Some(extended_algorithms::snap_to_grid(g, 4.0).unwrap())),
        ),
        (
            "geo.line_substring",
            json!({"start_ratio": 0.25, "end_ratio": 0.75}),
            linee(),
            Box::new(|g| extended_algorithms::line_substring(linea(g), 0.25, 0.75).unwrap()),
        ),
        (
            "geo.line_interpolate_point",
            json!({"ratio": 0.4}),
            linee(),
            Box::new(|g| {
                extended_algorithms::line_interpolate_point(linea(g), 0.4)
                    .unwrap()
                    .map(Geometry::Point)
            }),
        ),
    ];
    for (op, config, ingresso, oracolo) in casi {
        let uscita = un_passo(op, config, std::slice::from_ref(&ingresso))
            .unwrap_or_else(|errore| panic!("{op}: {errore}"));
        assert_eq!(uscita.num_rows(), ingresso.num_rows(), "{op}");
        assert_eq!(
            colonna(&uscita, "geometry").as_ref(),
            attese_geo(&ingresso, oracolo).as_ref(),
            "{op}"
        );
        for nome in ["id", "label"] {
            assert_eq!(colonna(&uscita, nome), colonna(&ingresso, nome), "{op}");
        }
    }
}

/// Misure, accessori, distanze e predicati: una colonna in coda, uguale al
/// kernel riga per riga.
#[test]
#[allow(clippy::too_many_lines)] // Un caso per operazione.
fn le_misure_sono_i_kernel() {
    type Reale = Box<dyn Fn(&Geometry<f64>) -> Option<f64>>;
    let altra_linea = Geometry::LineString(LineString::from(vec![
        (X0, Y0 + 20.0),
        (X0 + 150.0, Y0 + 30.0),
    ]));
    let altro_poligono = Geometry::Polygon(quadrato(X0 + 20.0, Y0 + 20.0, 60.0));
    let milano = Geometry::Point(Point::new(9.19, 45.46));
    let altra = altra_linea.clone();
    let riferimento = Point::new(X0 + 120.0, Y0 + 40.0);
    let casi: Vec<(&str, Value, RecordBatch, &str, Reale)> = vec![
        (
            "geo.area",
            json!({}),
            poligoni(),
            "area",
            Box::new(|g| Some(operations::area(g).unwrap())),
        ),
        (
            "geo.length",
            json!({"output_column": "lunghezza"}),
            linee(),
            "lunghezza",
            Box::new(|g| Some(operations::length(g).unwrap())),
        ),
        (
            "geo.perimeter",
            json!({}),
            poligoni(),
            "perimeter",
            Box::new(|g| Some(operations::perimeter(g).unwrap())),
        ),
        (
            "geo.geodesic_line_length",
            json!({}),
            linee_lonlat(),
            "geodesic_line_length",
            Box::new(|g| Some(extended::geodesic_line_length_m(linea(g)).unwrap())),
        ),
        (
            "geo.geodesic_area",
            json!({}),
            poligoni_lonlat(),
            "geodesic_area",
            Box::new(|g| Some(extended_algorithms::geodesic_area_m2(g).unwrap())),
        ),
        (
            "geo.line_locate_point",
            json!({"point_wkb": esadecimale_di(&Geometry::Point(riferimento))}),
            linee(),
            "fraction",
            Box::new(move |g| extensions::line_locate_point(g, &riferimento).unwrap()),
        ),
        (
            "geo.distance",
            json!({"other_wkb": esadecimale_di(&altra_linea)}),
            poligoni(),
            "distance",
            Box::new({
                let altra = altra.clone();
                move |g| operations::distance(g, &altra).unwrap()
            }),
        ),
        (
            "geo.hausdorff_distance",
            json!({"other_wkb": esadecimale_di(&altra_linea)}),
            linee(),
            "hausdorff_distance",
            Box::new({
                let altra = altra.clone();
                move |g| extended::hausdorff_distance(g, &altra, u64::MAX).unwrap()
            }),
        ),
        (
            "geo.frechet_distance",
            json!({"other_wkb": esadecimale_di(&altra_linea), "output_column": "frechet"}),
            linee(),
            "frechet",
            Box::new(move |g| {
                extended_algorithms::frechet_distance(linea(g), linea(&altra), u64::MAX).unwrap()
            }),
        ),
        (
            "geo.haversine_distance",
            json!({"other_wkb": esadecimale_di(&milano)}),
            punti_lonlat(),
            "haversine_distance",
            Box::new({
                let milano = milano.clone();
                move |g| Some(extended::haversine_distance_m(punto(g), punto(&milano)).unwrap())
            }),
        ),
        (
            "geo.geodesic_distance",
            json!({"other_wkb": esadecimale_di(&milano)}),
            punti_lonlat(),
            "geodesic_distance",
            Box::new({
                let milano = milano.clone();
                move |g| Some(extended::geodesic_distance_m(punto(g), punto(&milano)).unwrap())
            }),
        ),
        (
            "geo.bearing",
            json!({"other_wkb": esadecimale_di(&milano)}),
            punti_lonlat(),
            "bearing",
            Box::new(move |g| {
                Some(
                    extended_algorithms::geodesic_bearing_degrees(punto(g), punto(&milano))
                        .unwrap(),
                )
            }),
        ),
    ];
    for (op, config, ingresso, nome, oracolo) in casi {
        let uscita = un_passo(op, config, std::slice::from_ref(&ingresso))
            .unwrap_or_else(|errore| panic!("{op}: {errore}"));
        assert_eq!(uscita.num_columns(), ingresso.num_columns() + 1, "{op}");
        assert_eq!(
            colonna(&uscita, nome).as_ref(),
            attesi_f64(&ingresso, oracolo).as_ref(),
            "{op}"
        );
        assert_eq!(
            colonna(&uscita, "geometry"),
            colonna(&ingresso, "geometry"),
            "{op}"
        );
    }

    // Conteggi, testo, limiti.
    let ingresso = poligoni();
    let geometrie_in = geometrie(&ingresso, "geometry");
    let uscita = un_passo(
        "geo.vertex_count",
        json!({}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let attesi: UInt64Array = geometrie_in
        .iter()
        .map(|g| g.as_ref().map(|g| operations::vertex_count(g).unwrap()))
        .collect();
    assert_eq!(
        colonna(&uscita, "vertex_count").as_ref(),
        &attesi as &dyn Array
    );
    let uscita = un_passo("geo.to_wkt", json!({}), std::slice::from_ref(&ingresso)).unwrap();
    let attesi: StringArray = geometrie_in
        .iter()
        .map(|g| g.as_ref().map(|g| operations::to_wkt(g).unwrap()))
        .collect();
    assert_eq!(colonna(&uscita, "wkt").as_ref(), &attesi as &dyn Array);
    let uscita = un_passo(
        "geo.bounds_extractor",
        json!({}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    for (asse, nome) in ["minx", "miny", "maxx", "maxy"].into_iter().enumerate() {
        let attesi: Float64Array = geometrie_in
            .iter()
            .map(|g| {
                g.as_ref()
                    .and_then(|g| operations::bounds(g).unwrap())
                    .map(|b| b[asse])
            })
            .collect();
        assert_eq!(
            colonna(&uscita, &format!("geometry_{nome}")).as_ref(),
            &attesi as &dyn Array
        );
    }

    // Predicati: uno per ciascuno, contro un poligono della config.
    for (op, predicato) in [
        ("geo.predicate_intersects", SpatialPredicate::Intersects),
        ("geo.predicate_disjoint", SpatialPredicate::Disjoint),
        ("geo.predicate_contains", SpatialPredicate::Contains),
        ("geo.predicate_within", SpatialPredicate::Within),
        ("geo.predicate_equals_topo", SpatialPredicate::EqualsTopo),
        ("geo.predicate_covers", SpatialPredicate::Covers),
        ("geo.predicate_covered_by", SpatialPredicate::CoveredBy),
        (
            "geo.predicate_contains_properly",
            SpatialPredicate::ContainsProperly,
        ),
        ("geo.predicate_touches", SpatialPredicate::Touches),
        ("geo.predicate_crosses", SpatialPredicate::Crosses),
        ("geo.predicate_overlaps", SpatialPredicate::Overlaps),
    ] {
        let uscita = un_passo(
            op,
            json!({"other_wkb": esadecimale_di(&altro_poligono)}),
            std::slice::from_ref(&ingresso),
        )
        .unwrap_or_else(|errore| panic!("{op}: {errore}"));
        let attesi: BooleanArray = geometrie_in
            .iter()
            .map(|g| {
                g.as_ref()
                    .map(|g| predicates::evaluate(g, &altro_poligono, predicato).unwrap())
            })
            .collect();
        let nome = op.strip_prefix("geo.").unwrap();
        assert_eq!(
            colonna(&uscita, nome).as_ref(),
            &attesi as &dyn Array,
            "{op}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Un caso per operazione, in un solo elenco.
fn accessori_diagnostica_snap_e_riproiezione_sono_i_kernel() {
    let ingresso = linee();
    let geometrie_in = geometrie(&ingresso, "geometry");

    let uscita = un_passo(
        "geo.geometry_accessors",
        json!({"fields": ["is_closed", "geometry_type"], "output_prefix": "a_"}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let nomi: Vec<String> = uscita
        .schema()
        .fields()
        .iter()
        .map(|campo| campo.name().clone())
        .collect();
    assert_eq!(
        nomi,
        ["id", "label", "geometry", "a_geometry_type", "a_is_closed"]
    );
    let accessori: Vec<_> = geometrie_in
        .iter()
        .map(|g| {
            g.as_ref()
                .map(|g| extensions::geometry_accessors(g).unwrap())
        })
        .collect();
    let tipi: StringArray = accessori
        .iter()
        .map(|a| a.as_ref().map(|a| a.geometry_type))
        .collect();
    assert_eq!(
        colonna(&uscita, "a_geometry_type").as_ref(),
        &tipi as &dyn Array
    );
    let chiuse: BooleanArray = accessori
        .iter()
        .map(|a| a.as_ref().map(|a| a.is_closed))
        .collect();
    assert_eq!(
        colonna(&uscita, "a_is_closed").as_ref(),
        &chiuse as &dyn Array
    );

    // Diagnostica: le dieci colonne al posto della geometria, anche su una
    // geometria OGC non valida (il dato che la diagnostica riporta).
    let farfalla = Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (X0, Y0),
            (X0 + 4.0, Y0 + 4.0),
            (X0 + 4.0, Y0),
            (X0, Y0 + 4.0),
            (X0, Y0),
        ]),
        vec![],
    ));
    let con_invalida = tabella(UTM, &[Some(farfalla.clone()), None]);
    let uscita = un_passo("geo.geometry_diagnostics", json!({}), &[con_invalida]).unwrap();
    assert!(uscita.column_by_name("geometry").is_none());
    let diagnostica: GeometryDiagnostics =
        extended_algorithms::geometry_diagnostics(&farfalla).unwrap();
    let validita = colonna(&uscita, "is_valid")
        .as_any()
        .downcast_ref::<BooleanArray>()
        .unwrap();
    assert_eq!(validita.value(0), diagnostica.is_valid);
    assert!(!diagnostica.is_valid);
    assert!(validita.is_null(1));
    let motivo = colonna(&uscita, "validity_reason")
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(
        Some(motivo.value(0)),
        diagnostica.validity_reason.as_deref()
    );

    // Snap: l'adapter di colonna dei kernel.
    let riferimento = Geometry::Point(Point::new(X0 + 199.0, Y0 + 1.0));
    let uscita = un_passo(
        "geo.snap",
        json!({"reference_wkb": esadecimale_di(&riferimento), "tolerance": 2.0}),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let celle = ingresso
        .column_by_name("geometry")
        .unwrap()
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap()
        .clone();
    let attese = extensions2_snap(&celle, &riferimento);
    assert_eq!(colonna(&uscita, "geometry").as_ref(), attese.as_ref());

    // Riproiezione: l'adapter dei kernel, stesso CRS d'uscita del contratto.
    let config = json!({"target_crs": "EPSG:4326"});
    let uscita = un_passo(
        "geo.reproject",
        config.clone(),
        std::slice::from_ref(&ingresso),
    )
    .unwrap();
    let sorgente = resolve_crs(UTM, "crs").unwrap();
    let parametri = plenora_kernels_geo::riproiezione::ReprojectParams::da_config(
        "geo.reproject",
        &config,
        &sorgente,
    )
    .unwrap();
    let (_, attese) = plenora_kernels_geo::riproiezione::reproject_batches(
        &ingresso.schema(),
        std::slice::from_ref(&ingresso),
        "geometry",
        &sorgente,
        &parametri,
    )
    .unwrap();
    assert_eq!(
        colonna(&uscita, "geometry").as_ref(),
        attese[0].column(2).as_ref()
    );
    let contratto = comune_geo::contratto(&uscita);
    assert_eq!(
        contratto.geometries[0]
            .crs
            .as_resolved()
            .unwrap()
            .definition(),
        "EPSG:4326"
    );
}

fn extensions2_snap(celle: &BinaryArray, riferimento: &Geometry<f64>) -> ArrayRef {
    let attese = plenora_kernels_geo::extensions2::snap_column(celle, riferimento, 2.0).unwrap();
    Arc::new(attese.iter().map(Option::as_deref).collect::<BinaryArray>())
}

/// I produttori: `from_coords` e `from_wkt` con il CRS di piano.
#[test]
fn i_produttori_sono_i_kernel() {
    let coordinate = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Int64, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![X0, X0 + 1.5])),
            Arc::new(Int64Array::from(vec![5_000_000, 5_000_010])),
        ],
    )
    .unwrap();
    let uscita = un_passo_con_crs("geo.from_coords", json!({}), coordinate, UTM).unwrap();
    let attese: BinaryArray = [
        wkb(&Geometry::Point(Point::new(X0, 5_000_000.0))),
        wkb(&Geometry::Point(Point::new(X0 + 1.5, 5_000_010.0))),
    ]
    .iter()
    .map(|c| Some(c.as_slice()))
    .collect();
    assert_eq!(colonna(&uscita, "geometry").as_ref(), &attese as &dyn Array);
    assert!(!uscita
        .schema()
        .field_with_name("geometry")
        .unwrap()
        .is_nullable());

    let testi = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("wkt", DataType::Utf8, true)])),
        vec![Arc::new(StringArray::from(vec![
            Some("POINT (500000 5000000)"),
            None,
            Some("LINESTRING (500000 5000000, 500010 5000010)"),
        ]))],
    )
    .unwrap();
    let uscita = un_passo_con_crs(
        "geo.from_wkt",
        json!({"wkt_column": "wkt"}),
        testi.clone(),
        UTM,
    )
    .unwrap();
    let attese = extensions::from_wkt_column(
        testi
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap(),
        extensions::OnWktError::Null,
    )
    .unwrap();
    let attese: BinaryArray = attese.iter().map(Option::as_deref).collect();
    assert_eq!(colonna(&uscita, "geometry").as_ref(), &attese as &dyn Array);

    // Una coordinata null in una colonna geometria non nullable: errore
    // esplicito, non una cella null.
    let con_null = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, true),
            Field::new("y", DataType::Float64, true),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![Some(X0), None])),
            Arc::new(Float64Array::from(vec![Some(Y0), Some(Y0)])),
        ],
    )
    .unwrap();
    let errore = un_passo_con_crs("geo.from_coords", json!({}), con_null, UTM).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
}

/// Ogni configurazione che l'analisi rifiuta la rifiuta la validazione, con
/// lo stesso errore; ogni configurazione che accetta la accetta, con lo
/// stesso contratto d'uscita.
#[test]
#[allow(clippy::too_many_lines)] // Un caso per operazione, in un solo elenco.
fn la_validazione_rifiuta_esattamente_cio_che_l_analisi_rifiuta() {
    let farfalla = Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (X0, Y0),
            (X0 + 4.0, Y0 + 4.0),
            (X0 + 4.0, Y0),
            (X0, Y0 + 4.0),
            (X0, Y0),
        ]),
        vec![],
    ));
    let fuori = Geometry::Point(Point::new(5.0e7, 4.0e6));
    let punto_utm = Geometry::Point(Point::new(X0, Y0));
    let casi: Vec<(&str, Value, RecordBatch)> = vec![
        ("geo.buffer", json!({"distance": 1.0}), poligoni()),
        ("geo.buffer", json!({}), poligoni()),
        (
            "geo.buffer",
            json!({"distance": 1.0, "extra": 1}),
            poligoni(),
        ),
        ("geo.simplify", json!({"tolerance": -1.0}), linee()),
        (
            "geo.affine_transform",
            json!({"coefficients": [1.0]}),
            linee(),
        ),
        (
            "geo.line_substring",
            json!({"start_ratio": 0.8, "end_ratio": 0.2}),
            linee(),
        ),
        ("geo.area", json!({"output_column": "id"}), poligoni()),
        ("geo.area", json!({"output_column": ""}), poligoni()),
        ("geo.geodesic_area", json!({}), poligoni()),
        ("geo.distance", json!({"other_wkb": "zz"}), poligoni()),
        (
            "geo.distance",
            json!({"other_wkb": esadecimale_di(&farfalla)}),
            poligoni(),
        ),
        (
            "geo.distance",
            json!({"other_wkb": esadecimale_di(&fuori)}),
            poligoni(),
        ),
        (
            "geo.predicate_within",
            json!({"other_wkb": esadecimale_di(&punto_utm)}),
            poligoni(),
        ),
        (
            "geo.line_locate_point",
            json!({"point_wkb": esadecimale_di(&farfalla)}),
            linee(),
        ),
        (
            "geo.snap",
            json!({"reference_wkb": esadecimale_di(&punto_utm), "tolerance": -1.0}),
            linee(),
        ),
        (
            "geo.reproject",
            json!({"target_crs": "EPSG:99999"}),
            linee(),
        ),
        (
            "geo.reproject",
            json!({"target_crs": "EPSG:23032"}),
            linee(),
        ),
        ("geo.reproject", json!({"target_crs": "EPSG:3857"}), linee()),
        ("geo.geometry_accessors", json!({"fields": []}), linee()),
        (
            "geo.geometry_accessors",
            json!({"fields": ["is_closed", "is_closed"]}),
            linee(),
        ),
        ("geo.centroid", json!({}), punti_lonlat()),
        ("geo.to_wkt", json!({"output_column": "label"}), linee()),
    ];
    for (op, config, ingresso) in casi {
        let atteso = analisi(op, std::slice::from_ref(&ingresso), &config, None);
        let pipeline = piano(&["t"], vec![passo("x", op, &["t"], config.clone())], &["x"]);
        let validata = pipeline.validate(&[("t", ingresso.schema())]);
        match (&validata, &atteso) {
            (Ok(validata), Ok(contratto)) => {
                // Il runner porta ogni contratto nello schema che emette
                // (blocco canonico delle geometrie).
                let ottenuto = validata.contratto("x").expect("contratto del passo");
                let emesso = arrow_schema_from_contract(contratto).expect("schema emesso");
                assert_eq!(ottenuto.schema, emesso, "{op} {config}");
            }
            (Err(runner), Err(analisi)) => {
                assert!(
                    stesso_errore(runner, analisi),
                    "{op} {config}: {runner} / {analisi}"
                );
            }
            _ => panic!(
                "{op} {config}: validazione {:?}, analisi {:?}",
                validata.as_ref().err(),
                atteso.as_ref().err()
            ),
        }
    }
}

/// Coordinate fuori dal dominio di validita' del CRS: `Crs` prima del
/// kernel, per ogni forma di operazione.
#[test]
fn le_coordinate_fuori_dominio_si_rifiutano() {
    let fuori = tabella(UTM, &[Some(Geometry::Point(Point::new(5.0e6, 4.0e6)))]);
    for (op, config) in [
        ("geo.area", json!({})),
        ("geo.buffer", json!({"distance": 1.0})),
        ("geo.geometry_diagnostics", json!({})),
        ("geo.reproject", json!({"target_crs": "EPSG:4326"})),
        (
            "geo.snap",
            json!({"reference_wkb": esadecimale_di(&Geometry::Point(Point::new(X0, Y0))), "tolerance": 1.0}),
        ),
    ] {
        let errore = un_passo(op, config, std::slice::from_ref(&fuori)).unwrap_err();
        assert_eq!(errore.category(), ErrorCategory::Crs, "{op}: {errore}");
    }
    // Un produttore rifiuta le geometrie prodotte fuori dominio.
    let coordinate = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("x", DataType::Float64, false),
            Field::new("y", DataType::Float64, false),
        ])),
        vec![
            Arc::new(Float64Array::from(vec![200.0])),
            Arc::new(Float64Array::from(vec![45.0])),
        ],
    )
    .unwrap();
    let errore = un_passo_con_crs("geo.from_coords", json!({}), coordinate, LONLAT).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::Crs, "{errore}");
}

/// Una colonna che dichiara i tipi geometrici si verifica: un tipo non
/// dichiarato e' un errore di schema, non un risultato.
#[test]
fn i_tipi_dichiarati_di_un_ingresso_si_verificano() {
    let ingresso = punti();
    let mut campi: Vec<Field> = ingresso
        .schema()
        .fields()
        .iter()
        .map(|campo| campo.as_ref().clone())
        .collect();
    let mut metadati = campi[2].metadata().clone();
    metadati.insert("plenora.geometry.types".to_owned(), "polygon".to_owned());
    metadati.insert(
        "plenora.geometry.types_declaration".to_owned(),
        "exact".to_owned(),
    );
    campi[2] = campi[2].clone().with_metadata(metadati);
    let dichiarata = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            campi,
            plenora_core::contract::arrow_metadata::canonical_schema_version_metadata(),
        )),
        ingresso.columns().to_vec(),
    )
    .unwrap();
    let errore = un_passo("geo.area", json!({}), &[dichiarata]).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::Schema, "{errore}");
}

/// Il CRS attraversa la catena: `reproject` lo cambia, `buffer` lavora
/// nella precisione del nuovo CRS e lo conserva.
#[test]
fn il_crs_attraversa_la_catena() {
    let pipeline = piano(
        &["t"],
        vec![
            passo("utm", "geo.reproject", &["t"], json!({"target_crs": UTM})),
            passo("fasce", "geo.buffer", &["utm"], json!({"distance": 50.0})),
            passo("aree", "geo.area", &["fasce"], json!({})),
        ],
        &["aree"],
    );
    let ingresso = punti_lonlat();
    let esito = esegui(&pipeline, &[("t", ingresso.clone())]).unwrap();
    let uscita = &esito.outputs[0].1;
    let contratto = comune_geo::contratto(uscita);
    assert_eq!(
        contratto.geometries[0]
            .crs
            .as_resolved()
            .unwrap()
            .definition(),
        UTM
    );
    // Stesso risultato dei kernel in sequenza.
    let sorgente = resolve_crs(LONLAT, "crs").unwrap();
    let parametri = plenora_kernels_geo::riproiezione::ReprojectParams::da_config(
        "geo.reproject",
        &json!({"target_crs": UTM}),
        &sorgente,
    )
    .unwrap();
    let (_, riproiettate) = plenora_kernels_geo::riproiezione::reproject_batches(
        &ingresso.schema(),
        std::slice::from_ref(&ingresso),
        "geometry",
        &sorgente,
        &parametri,
    )
    .unwrap();
    let aree: Float64Array = geometrie(&riproiettate[0], "geometry")
        .iter()
        .map(|g| {
            g.as_ref().map(|g| {
                let fascia =
                    operations::buffer_with_cap(g, 50.0, BufferCapStyle::Round, centimetro_utm())
                        .unwrap();
                operations::area(&fascia).unwrap()
            })
        })
        .collect();
    assert_eq!(colonna(uscita, "area").as_ref(), &aree as &dyn Array);
}

/// Tabellari e geo nella stessa catena, in entrambi i versi.
#[test]
fn tabellari_e_geo_si_concatenano() {
    let pipeline = piano(
        &["t"],
        vec![
            passo(
                "pari",
                "table.filter",
                &["t"],
                json!({"column": "label", "operator": "==", "value": "pari"}),
            ),
            passo("fasce", "geo.buffer", &["pari"], json!({"distance": 2.0})),
            passo(
                "scelte",
                "table.select_columns",
                &["fasce"],
                json!({"columns": ["geometry", "id"]}),
            ),
            passo("aree", "geo.area", &["scelte"], json!({})),
            passo(
                "ordinate",
                "table.sort",
                &["aree"],
                json!({"columns": ["area"], "ascending": false}),
            ),
        ],
        &["ordinate"],
    );
    let ingresso = poligoni();
    let esito = esegui(&pipeline, &[("t", ingresso.clone())]).unwrap();
    let uscita = &esito.outputs[0].1;
    let nomi: Vec<String> = uscita
        .schema()
        .fields()
        .iter()
        .map(|campo| campo.name().clone())
        .collect();
    assert_eq!(nomi, ["geometry", "id", "area"]);
    // Righe pari: 0 e 2 (la 1 e' null ma dispari), aree decrescenti.
    let aree = colonna(uscita, "area")
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let attese: Vec<f64> = geometrie(&ingresso, "geometry")
        .iter()
        .enumerate()
        .filter(|(riga, _)| riga % 2 == 0)
        .map(|(_, g)| {
            let fascia = operations::buffer_with_cap(
                g.as_ref().unwrap(),
                2.0,
                BufferCapStyle::Round,
                centimetro_utm(),
            )
            .unwrap();
            operations::area(&fascia).unwrap()
        })
        .collect();
    let mut attese = attese;
    attese.sort_by(|a, b| b.total_cmp(a));
    assert_eq!(aree.values().to_vec(), attese);
}

/// Un passo geo che non sta nel budget si rifiuta prima di girare.
#[test]
fn un_passo_geo_oltre_il_budget_si_rifiuta_prima_di_eseguire() {
    let grandi: Vec<Option<Geometry<f64>>> = (0..2000_u32)
        .map(|i| {
            Some(Geometry::Polygon(quadrato(
                f64::from(i).mul_add(10.0, X0),
                Y0,
                5.0,
            )))
        })
        .collect();
    let ingresso = tabella(UTM, &grandi);
    let pipeline = con_budget(
        piano(
            &["t"],
            vec![passo("x", "geo.buffer", &["t"], json!({"distance": 1.0}))],
            &["x"],
        ),
        6 * 1024 * 1024,
    );
    let errore = esegui(&pipeline, &[("t", ingresso.clone())]).unwrap_err();
    let PlenoraError::ResourceLimit(messaggio) = &errore else {
        panic!("atteso ResourceLimit: {errore}");
    };
    assert!(messaggio.contains("geo.buffer"), "{messaggio}");
    assert!(messaggio.contains("previsti"), "{messaggio}");
    // Con il budget di default lo stesso passo gira.
    let senza_limite = piano(
        &["t"],
        vec![passo("x", "geo.buffer", &["t"], json!({"distance": 1.0}))],
        &["x"],
    );
    let esito = esegui(&senza_limite, &[("t", ingresso)]).unwrap();
    assert!(esito.report.passi[0].byte_previsti > 6 * 1024 * 1024);
}

/// Il modello delle geo e' generato dalle misure nel repository.
#[test]
fn il_modello_geo_viene_dalle_misure_registrate() {
    use sha2::{Digest, Sha256};
    let percorso = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/misure/catalogo-memoria-v4.json");
    let impronta =
        plenora_core::esadecimale::esadecimale(&Sha256::digest(std::fs::read(percorso).unwrap()));
    assert_eq!(impronta, plenora_pipeline::costi_geo::SHA256_MISURE);
    // Ogni operazione geo del catalogo ha il suo modello.
    for descrittore in plenora_core::catalog::CATALOG
        .iter()
        .filter(|d| d.family == plenora_core::catalog::Family::Geo)
    {
        assert!(
            plenora_pipeline::budget::costo_di(descrittore.id).is_some(),
            "{}",
            descrittore.id
        );
    }
}

/// Una geometria vuota in una colonna non nullable: errore esplicito del
/// passo, mai una cella null in una colonna che non la ammette.
#[test]
fn un_risultato_vuoto_in_una_colonna_non_nullable_e_un_errore() {
    let vuota = Geometry::MultiPolygon(geo::MultiPolygon::new(vec![]));
    let ingresso = tabella(UTM, &[Some(vuota)]);
    let mut campi: Vec<Field> = ingresso
        .schema()
        .fields()
        .iter()
        .map(|campo| campo.as_ref().clone())
        .collect();
    campi[2] = campi[2].clone().with_nullable(false);
    let non_nullable =
        RecordBatch::try_new(Arc::new(Schema::new(campi)), ingresso.columns().to_vec()).unwrap();
    let errore = un_passo("geo.point_on_surface", json!({}), &[non_nullable]).unwrap_err();
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
    assert!(errore.to_string().contains("non ammette null"), "{errore}");
}

/// Il tipo della geometria `other_wkb` si decide dalla config: si rifiuta
/// in validazione (analisi e runner insieme), anche su una tabella vuota o
/// tutta null, che in esecuzione non arriverebbe mai al kernel.
#[test]
fn il_tipo_di_other_wkb_si_rifiuta_in_validazione() {
    let punto_utm = Geometry::Point(Point::new(X0, Y0));
    let linea_lonlat = Geometry::LineString(LineString::from(vec![(9.0, 45.0), (9.5, 45.5)]));
    let poligono_lonlat = Geometry::Polygon(quadrato(9.0, 45.0, 0.1));
    let vuota_utm = tabella(UTM, &[]);
    let nulla_lonlat = tabella(LONLAT, &[None, None]);
    let casi = [
        ("geo.frechet_distance", punto_utm.clone(), linee()),
        ("geo.frechet_distance", punto_utm, vuota_utm),
        (
            "geo.haversine_distance",
            linea_lonlat.clone(),
            punti_lonlat(),
        ),
        (
            "geo.geodesic_distance",
            poligono_lonlat.clone(),
            nulla_lonlat.clone(),
        ),
        ("geo.bearing", linea_lonlat, nulla_lonlat),
        ("geo.bearing", poligono_lonlat, tabella(LONLAT, &[])),
    ];
    for (op, altra, ingresso) in casi {
        let config = json!({"other_wkb": esadecimale_di(&altra)});
        let analizzata = analisi(op, std::slice::from_ref(&ingresso), &config, None)
            .expect_err("l'analisi rifiuta il tipo");
        let errore = un_passo(op, config, &[ingresso]).expect_err("la validazione rifiuta");
        assert!(
            stesso_errore(&errore, &analizzata),
            "{op}: {errore} / {analizzata}"
        );
        assert_eq!(
            errore.category(),
            ErrorCategory::InvalidPlan,
            "{op}: {errore}"
        );
        assert!(errore.to_string().contains("other_wkb"), "{op}: {errore}");
    }
    // Il tipo giusto passa, anche su una tabella vuota.
    let linea = Geometry::LineString(LineString::from(vec![(X0, Y0), (X0 + 1.0, Y0)]));
    let uscita = un_passo(
        "geo.frechet_distance",
        json!({"other_wkb": esadecimale_di(&linea)}),
        &[tabella(UTM, &[])],
    )
    .unwrap();
    assert_eq!(uscita.num_rows(), 0);
}

/// `table.pivot` con `mapping` dopo un passo geo e su un ingresso con
/// geometria: il contratto validato (schema canonico delle tabelle geo) e'
/// lo schema eseguito, con le colonne del mapping nell'ordine delle chiavi.
#[test]
fn pivot_con_mapping_sullo_schema_canonico() {
    let dopo_area = || {
        piano(
            &["t"],
            vec![
                passo("a", "geo.area", &["t"], json!({})),
                passo(
                    "p",
                    "table.pivot",
                    &["a"],
                    json!({"index_col": "label", "pivot_col": "id", "value_col": "area",
                           "aggr_func": "sum",
                           "mapping": {"0": "zero", "3": "tre", "9": "nove"}}),
                ),
            ],
            &["p"],
        )
    };
    let diretto = piano(
        &["t"],
        vec![passo(
            "p",
            "table.pivot",
            &["t"],
            json!({"index_col": "label", "pivot_col": "id", "value_col": "id",
                   "aggr_func": "count", "mapping": {"1": "uno", "2": "due"}}),
        )],
        &["p"],
    );
    for (pipeline, attesi) in [
        (dopo_area(), vec!["label", "zero", "tre", "nove"]),
        (diretto, vec!["label", "uno", "due"]),
    ] {
        let tabella = poligoni();
        let validata = pipeline
            .validate(&[("t", tabella.schema())])
            .expect("pivot con mapping validato");
        let atteso = validata.contratto("p").expect("contratto").schema.clone();
        let esito = validata
            .run(vec![("t".to_owned(), tabella)])
            .expect("pivot con mapping eseguito");
        let uscita = &esito.outputs[0].1;
        assert_eq!(uscita.schema(), atteso);
        let nomi: Vec<&str> = atteso
            .fields()
            .iter()
            .map(|campo| campo.name().as_str())
            .collect();
        assert_eq!(nomi, attesi);
        assert_eq!(uscita.num_rows(), 2, "una riga per etichetta");
    }
    // I valori dopo `geo.area`: riga 0 (pari, 100 x 100) in `zero`, riga 3
    // (dispari, 30 x 30) in `tre`, `nove` assente dai dati e tutta null.
    let esito = esegui(&dopo_area(), &[("t", poligoni())]).unwrap();
    let uscita = &esito.outputs[0].1;
    let etichette = uscita
        .column_by_name("label")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let f64_di = |nome: &str| {
        uscita
            .column_by_name(nome)
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .clone()
    };
    let (zero, tre, nove) = (f64_di("zero"), f64_di("tre"), f64_di("nove"));
    for riga in 0..uscita.num_rows() {
        match etichette.value(riga) {
            "pari" => {
                assert!((zero.value(riga) - 10_000.0).abs() < 1e-6);
                assert!(tre.is_null(riga));
            }
            "dispari" => {
                assert!(zero.is_null(riga));
                assert!((tre.value(riga) - 900.0).abs() < 1e-6);
            }
            altra => panic!("etichetta inattesa `{altra}`"),
        }
        assert!(nove.is_null(riga));
    }
}

/// Run-end e union non arrivano ai kernel per nessuna via d'ingresso del
/// runner: uno schema che li contiene si rifiuta in validazione anche
/// accanto a una geometria (prima del contratto canonico), e una tabella
/// che li porta al posto dello schema validato si rifiuta in `run`, prima
/// del primo passo.
#[test]
fn run_end_e_union_rifiutati_in_validazione_e_in_esecuzione() {
    use plenora_core::arrow::array::types::Int32Type;
    use plenora_core::arrow::array::{Int32Array, RunArray, UnionArray};
    use plenora_core::arrow::schema::UnionFields;

    let base = poligoni();
    let righe = base.num_rows();
    let run_end: ArrayRef = Arc::new(
        RunArray::<Int32Type>::try_new(
            &Int32Array::from(vec![2, i32::try_from(righe).unwrap()]),
            &StringArray::from(vec![Some("pari"), None]),
        )
        .unwrap(),
    );
    let campi_union: UnionFields =
        std::iter::once((0_i8, Arc::new(Field::new("s", DataType::Utf8, true)))).collect();
    let union: ArrayRef = Arc::new(
        UnionArray::try_new(
            campi_union,
            vec![0_i8; righe].into(),
            None,
            vec![Arc::new(StringArray::from(vec![Some("x"); righe])) as ArrayRef],
        )
        .unwrap(),
    );
    let pipeline = piano(
        &["t"],
        vec![
            passo("a", "geo.area", &["t"], json!({})),
            passo("f", "table.limit", &["a"], json!({"n": 10})),
        ],
        &["f"],
    );
    for colonna in [run_end, union] {
        // `label` sostituita: stesso nome, tipo non supportato.
        let campi: Vec<Field> = base
            .schema()
            .fields()
            .iter()
            .map(|campo| {
                if campo.name() == "label" {
                    Field::new("label", colonna.data_type().clone(), true)
                } else {
                    campo.as_ref().clone()
                }
            })
            .collect();
        let mut colonne = base.columns().to_vec();
        colonne[1] = colonna.clone();
        let codificata =
            RecordBatch::try_new(Arc::new(Schema::new(campi)), colonne).expect("tabella");
        // In validazione, sullo schema che li porta.
        let errore = pipeline
            .validate(&[("t", codificata.schema())])
            .expect_err("tipo non supportato in validazione");
        assert!(
            matches!(&errore, PlenoraError::Unsupported(m) if m.contains("RunEndEncoded")),
            "{errore}"
        );
        // In esecuzione, dopo una validazione sullo schema senza codifica.
        let validata = pipeline
            .validate(&[("t", base.schema())])
            .expect("schema senza codifica");
        let errore = validata
            .run(vec![("t".to_owned(), codificata)])
            .expect_err("tabella diversa dallo schema validato");
        assert!(matches!(errore, PlenoraError::Schema(_)), "{errore}");
    }
}

/// `table.pivot` con la colonna geometria come indice: la geometria resta
/// geometria (tipo, metadati di campo con il CRS, contratto con CRS e
/// geometria attiva), lo schema validato e' quello eseguito e ogni riga
/// porta nella colonna della sua etichetta l'`id` della riga d'ingresso con
/// la stessa geometria.
#[test]
#[allow(clippy::too_many_lines)] // Schema, contratto e valori in un caso.
fn pivot_con_indice_geometria_conserva_la_geometria() {
    let ingresso = poligoni();
    let pipeline = piano(
        &["t"],
        vec![passo(
            "p",
            "table.pivot",
            &["t"],
            json!({"index_col": "geometry", "pivot_col": "label", "value_col": "id",
                   "aggr_func": "first", "mapping": {"pari": "p", "dispari": "d"}}),
        )],
        &["p"],
    );
    let validata = pipeline
        .validate(&[("t", ingresso.schema())])
        .expect("pivot sulla geometria validato");
    let contratto_validato = validata.contratto("p").expect("contratto").clone();
    let esito = validata
        .run(vec![("t".to_owned(), ingresso.clone())])
        .expect("pivot sulla geometria eseguito");
    let uscita = &esito.outputs[0].1;
    assert_eq!(uscita.schema(), contratto_validato.schema);
    let nomi: Vec<&str> = uscita
        .schema_ref()
        .fields()
        .iter()
        .map(|campo| campo.name().as_str())
        .collect();
    // Chiavi del mapping in ordine di byte: "dispari" prima di "pari".
    assert_eq!(nomi, ["geometry", "d", "p"]);
    // Il campo geometria e' quello d'ingresso nella forma canonica del
    // runner: stesso tipo, metadati GeoArrow (CRS) d'ingresso conservati,
    // CRS canonico dichiarato.
    let schema_ingresso = ingresso.schema();
    let campo_ingresso = schema_ingresso.field_with_name("geometry").unwrap();
    let campo_uscita = uscita.schema_ref().field_with_name("geometry").unwrap();
    assert_eq!(campo_uscita.data_type(), campo_ingresso.data_type());
    for (chiave, valore) in campo_ingresso.metadata() {
        assert_eq!(
            campo_uscita.metadata().get(chiave),
            Some(valore),
            "metadato `{chiave}`"
        );
    }
    assert_eq!(
        campo_uscita
            .metadata()
            .get("plenora.geometry.crs_id")
            .map(String::as_str),
        Some(UTM)
    );
    // Contratto validato e contratto letto dall'uscita: stessa geometria,
    // stesso CRS, geometria attiva.
    for contratto in [contratto_validato, comune_geo::contratto(uscita)] {
        assert_eq!(contratto.geometries.len(), 1);
        let geometria = &contratto.geometries[0];
        assert_eq!(geometria.name, "geometry");
        assert_eq!(
            geometria.crs.as_resolved().unwrap().definition(),
            UTM,
            "CRS conservato"
        );
        assert_eq!(contratto.active_geometry, Some(geometria.field_id));
    }
    // I valori: quattro geometrie distinte (una nulla), una riga ciascuna.
    assert_eq!(uscita.num_rows(), ingresso.num_rows());
    let celle_in = colonna(&ingresso, "geometry")
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    let id_in = colonna(&ingresso, "id")
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let etichette_in = colonna(&ingresso, "label")
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let celle = colonna(uscita, "geometry")
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    let interi = |nome: &str| {
        colonna(uscita, nome)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .clone()
    };
    let (d, p) = (interi("d"), interi("p"));
    // La chiave nulla per prima (ordine delle chiavi di `aggregate`).
    assert!(celle.is_null(0));
    let mut viste = Vec::new();
    for riga in 0..uscita.num_rows() {
        let sorgente = (0..ingresso.num_rows())
            .find(|&i| {
                celle_in.is_null(i) == celle.is_null(riga)
                    && (celle.is_null(riga) || celle_in.value(i) == celle.value(riga))
            })
            .expect("geometria dell'ingresso");
        viste.push(sorgente);
        let (piena, vuota) = match etichette_in.value(sorgente) {
            "pari" => (&p, &d),
            "dispari" => (&d, &p),
            altra => panic!("etichetta inattesa `{altra}`"),
        };
        assert!(!piena.is_null(riga), "riga {riga}");
        assert_eq!(piena.value(riga), id_in.value(sorgente), "riga {riga}");
        assert!(vuota.is_null(riga), "riga {riga}");
    }
    viste.sort_unstable();
    assert_eq!(viste, [0, 1, 2, 3], "ogni geometria una volta");
}

/// Due piani in catena: l'uscita del primo (`geo.area`) e' l'ingresso del
/// secondo (`table.pivot` con `mapping`). Il secondo valida sullo schema
/// eseguito dal primo ed esegue con i valori attesi; la stessa uscita con
/// una colonna run-end o union si rifiuta al confine del secondo piano, in
/// validazione e in esecuzione.
#[test]
#[allow(clippy::too_many_lines)] // Catena, valori e i due rifiuti in un caso.
fn due_piani_in_catena_pivot_e_rifiuto_di_run_end_e_union() {
    use plenora_core::arrow::array::types::Int32Type;
    use plenora_core::arrow::array::{Int32Array, RunArray, UnionArray};
    use plenora_core::arrow::schema::UnionFields;

    let primo = piano(
        &["t"],
        vec![passo("a", "geo.area", &["t"], json!({}))],
        &["a"],
    );
    let intermedia = esegui(&primo, &[("t", poligoni())])
        .expect("primo piano")
        .outputs
        .remove(0)
        .1;
    let secondo = piano(
        &["a"],
        vec![passo(
            "p",
            "table.pivot",
            &["a"],
            json!({"index_col": "label", "pivot_col": "id", "value_col": "area",
                   "aggr_func": "sum", "mapping": {"0": "zero", "3": "tre", "9": "nove"}}),
        )],
        &["p"],
    );
    let validata = secondo
        .validate(&[("a", intermedia.schema())])
        .expect("secondo piano validato sull'uscita del primo");
    let atteso = validata.contratto("p").expect("contratto").schema.clone();
    let esito = validata
        .run(vec![("a".to_owned(), intermedia.clone())])
        .expect("secondo piano eseguito");
    let uscita = &esito.outputs[0].1;
    assert_eq!(uscita.schema(), atteso);
    let etichette = colonna(uscita, "label")
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let f64_di = |nome: &str| {
        colonna(uscita, nome)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .clone()
    };
    let (zero, tre, nove) = (f64_di("zero"), f64_di("tre"), f64_di("nove"));
    assert_eq!(uscita.num_rows(), 2);
    for riga in 0..uscita.num_rows() {
        match etichette.value(riga) {
            "pari" => {
                assert!((zero.value(riga) - 10_000.0).abs() < 1e-6);
                assert!(tre.is_null(riga));
            }
            "dispari" => {
                assert!(zero.is_null(riga));
                assert!((tre.value(riga) - 900.0).abs() < 1e-6);
            }
            altra => panic!("etichetta inattesa `{altra}`"),
        }
        assert!(nove.is_null(riga));
    }

    // L'uscita del primo piano con `label` run-end o union.
    let righe = intermedia.num_rows();
    let run_end: ArrayRef = Arc::new(
        RunArray::<Int32Type>::try_new(
            &Int32Array::from(vec![2, i32::try_from(righe).unwrap()]),
            &StringArray::from(vec![Some("pari"), None]),
        )
        .unwrap(),
    );
    let campi_union: UnionFields =
        std::iter::once((0_i8, Arc::new(Field::new("s", DataType::Utf8, true)))).collect();
    let union: ArrayRef = Arc::new(
        UnionArray::try_new(
            campi_union,
            vec![0_i8; righe].into(),
            None,
            vec![Arc::new(StringArray::from(vec![Some("x"); righe])) as ArrayRef],
        )
        .unwrap(),
    );
    let schema_intermedio = intermedia.schema();
    let posizione = schema_intermedio.index_of("label").unwrap();
    for colonna_codificata in [run_end, union] {
        let campi: Vec<Field> = schema_intermedio
            .fields()
            .iter()
            .enumerate()
            .map(|(i, campo)| {
                if i == posizione {
                    Field::new("label", colonna_codificata.data_type().clone(), true)
                } else {
                    campo.as_ref().clone()
                }
            })
            .collect();
        let mut colonne = intermedia.columns().to_vec();
        colonne[posizione] = colonna_codificata.clone();
        let codificata = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                campi,
                schema_intermedio.metadata().clone(),
            )),
            colonne,
        )
        .expect("tabella");
        let errore = secondo
            .validate(&[("a", codificata.schema())])
            .expect_err("tipo non supportato in validazione");
        assert!(
            matches!(&errore, PlenoraError::Unsupported(m) if m.contains("RunEndEncoded")),
            "{errore}"
        );
        let errore = secondo
            .validate(&[("a", schema_intermedio.clone())])
            .expect("schema senza codifica")
            .run(vec![("a".to_owned(), codificata)])
            .expect_err("tabella diversa dallo schema validato");
        assert!(matches!(errore, PlenoraError::Schema(_)), "{errore}");
    }
}

/// L'ordine dei rifiuti di `pivot` in validazione (scheda, «Errori»): una
/// colonna assente e' `InvalidPlan` anche senza `mapping`; senza `mapping`
/// ogni altro difetto (`index_col` vuoto o ripetuto) e' `Unsupported`; con
/// `mapping` lo stesso difetto e' `InvalidPlan`.
#[test]
fn pivot_ordine_dei_rifiuti_in_validazione() {
    let schema = poligoni().schema();
    let valida = |config: Value| {
        piano(
            &["t"],
            vec![passo("p", "table.pivot", &["t"], config)],
            &["p"],
        )
        .validate(&[("t", schema.clone())])
        .expect_err("rifiuto")
    };
    let mapping = json!({"pari": "p"});
    for index_col in ["", " , ", "id,id"] {
        let senza = valida(json!({"index_col": index_col, "pivot_col": "label",
                                  "value_col": "id"}));
        assert!(
            matches!(senza, PlenoraError::Unsupported(_)),
            "`{index_col}` senza mapping: {senza}"
        );
        let con = valida(json!({"index_col": index_col, "pivot_col": "label",
                                "value_col": "id", "mapping": mapping}));
        assert!(
            matches!(con, PlenoraError::InvalidPlan(_)),
            "`{index_col}` con mapping: {con}"
        );
    }
    for (index_col, value_col) in [("manca", "id"), ("id", "manca")] {
        let senza = valida(json!({"index_col": index_col, "pivot_col": "label",
                                  "value_col": value_col}));
        assert!(matches!(senza, PlenoraError::InvalidPlan(_)), "{senza}");
    }
}
