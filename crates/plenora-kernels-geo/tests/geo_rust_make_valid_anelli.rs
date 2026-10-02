//! `geo.make_valid` su anelli che girano dentro se stessi o si toccano, su
//! parti collassate e su incroci arrotondati: attese dall'output di GEOS
//! 3.14 registrato dalla sonda della campagna differenziale del laboratorio
//! (qui GEOS non gira), in metri con 1 cm.
//!
//! - `STRUCTURE` ripara un anello come `GeometryFixer::fixRing` di GEOS, il
//!   buffer nullo nei due versi: le regioni con avvolgimento diverso da zero.
//!   Il kernel del laboratorio prendeva tutte le facce del polygonize, e
//!   l'isola di una cornice che gira dentro se stessa (avvolgimento 0)
//!   finiva piena: da buco sottraeva 36 m^2 di troppo, senza errore.
//! - Le parti collassate di un `MultiPolygon` (`keep_collapsed`) si uniscono
//!   come in GEOS: linee nodate meno cio' che l'area copre, punti non
//!   coperti. Il laboratorio polygonizzava le linee e ne faceva area.
//! - `LINEWORK` rifiuta un incrocio arrotondato a meno di 1 cm da un'altra
//!   feature: li' la topologia e' decisa da un ULP.

use geo::{Area, Geometry};
use plenora_kernels_geo::rust_backend::make_valid::{
    make_valid_geometry_rust, MakeValidError, RepairMethod,
};
use wkt::TryFromWkt;

const CENTIMETRO: f64 = 0.01;

fn wkt(text: &str) -> Geometry<f64> {
    Geometry::try_from_wkt_str(text).expect("WKT del test")
}

fn polygonal_area(geometry: &Geometry<f64>) -> f64 {
    match geometry {
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => geometry.unsigned_area(),
        Geometry::GeometryCollection(collection) => collection.0.iter().map(polygonal_area).sum(),
        _ => 0.0,
    }
}

fn line_length(geometry: &Geometry<f64>) -> f64 {
    use geo::{Euclidean, Length};
    match geometry {
        Geometry::LineString(line) => Euclidean.length(line),
        Geometry::MultiLineString(lines) => lines.0.iter().map(|line| Euclidean.length(line)).sum(),
        Geometry::GeometryCollection(collection) => collection.0.iter().map(line_length).sum(),
        _ => 0.0,
    }
}

fn points(geometry: &Geometry<f64>) -> usize {
    match geometry {
        Geometry::Point(_) => 1,
        Geometry::MultiPoint(points) => points.0.len(),
        Geometry::GeometryCollection(collection) => collection.0.iter().map(points).sum(),
        _ => 0,
    }
}

/// Anelli come shell e come buco: area dell'output GEOS.
#[test]
fn structure_fills_rings_by_nonzero_winding_like_geos() {
    let cases = [
        // Controesempio: cornice che gira dentro se
        // stessa come buco. GEOS: la cornice esterna e l'isola, 80 m^2.
        (
            "POLYGON((-1 -1,11 -1,11 11,-1 11,-1 -1),(0 0,10 0,10 10,0 10,0 0,2 2,2 8,8 8,8 2,2 2,0 0))",
            80.0,
        ),
        // La stessa cornice come shell: l'isola (avvolgimento 0) resta vuota.
        (
            "POLYGON((0 0,10 0,10 10,0 10,0 0,2 2,2 8,8 8,8 2,2 2,0 0))",
            64.0,
        ),
        // Isola percorsa nello stesso verso: avvolgimento 2, piena.
        (
            "POLYGON((0 0,10 0,10 10,0 10,0 0,2 2,8 2,8 8,2 8,2 2,0 0))",
            100.0,
        ),
        // Due livelli: avvolgimento 1, 0, 1.
        (
            "POLYGON((0 0,10 0,10 10,0 10,0 0,2 2,2 8,8 8,8 2,2 2,4 4,6 4,6 6,4 6,4 4,2 2,0 0))",
            68.0,
        ),
        // Due livelli nello stesso verso: 1, 2, 3.
        (
            "POLYGON((0 0,10 0,10 10,0 10,0 0,2 2,8 2,8 8,2 8,2 2,4 4,6 4,6 6,4 6,4 4,2 2,0 0))",
            100.0,
        ),
        // Due livelli come buco.
        (
            "POLYGON((-1 -1,11 -1,11 11,-1 11,-1 -1),(0 0,10 0,10 10,0 10,0 0,2 2,2 8,8 8,8 2,2 2,4 4,6 4,6 6,4 6,4 4,2 2,0 0))",
            76.0,
        ),
        // Triangolo interno che tocca la shell in un vertice.
        (
            "POLYGON((0 0,10 0,10 10,0 10,0 0,5 5,8 2,8 8,5 5,0 0))",
            100.0,
        ),
        // Otto: due lobi che si toccano in un vertice, come shell e come buco.
        ("POLYGON((0 0,4 0,4 4,6 4,6 8,4 8,4 4,0 4,0 0))", 24.0),
        (
            "POLYGON((-1 -1,9 -1,9 9,-1 9,-1 -1),(0 0,4 0,4 4,6 4,6 8,4 8,4 4,0 4,0 0))",
            76.0,
        ),
        // Spirale che rientra toccando il proprio lato.
        (
            "POLYGON((0 0,10 0,10 10,0 10,0 1,9 1,9 9,1 9,1 0.5,0 0.5,0 0))",
            99.5,
        ),
    ];
    for (input, geos) in cases {
        let output =
            make_valid_geometry_rust(&wkt(input), RepairMethod::Structure, false, CENTIMETRO)
                .unwrap_or_else(|errore| panic!("{input}: {errore}"));
        assert!(
            (polygonal_area(&output) - geos).abs() < 1e-9,
            "{input}: area {}, GEOS {geos}",
            polygonal_area(&output)
        );
    }
}

/// Parti collassate di un `MultiPolygon` con `keep_collapsed`: area, lunghezza
/// delle linee e numero di punti dell'output GEOS.
#[test]
fn collapsed_parts_are_united_like_geos() {
    let cases = [
        // Tre poligoni collassati che formano un triangolo: linee, non area.
        (
            "MULTIPOLYGON(((0 0,2 0,4 0,0 0)),((4 0,4 2,4 4,4 0)),((4 4,2 2,0 0,4 4)))",
            0.0,
            8.0 + 32_f64.sqrt(),
            0,
        ),
        (
            "MULTIPOLYGON(((0 0,2 0,4 0,0 0)),((4 0,4 2,4 4,4 0)),((4 4,2 2,0 0,4 4)),((10 10,11 10,11 11,10 10)))",
            0.5,
            8.0 + 32_f64.sqrt(),
            0,
        ),
        // Linea dentro l'area: coperta.
        (
            "MULTIPOLYGON(((0 0,10 0,10 10,0 10,0 0)),((2 2,4 2,6 2,2 2)))",
            100.0,
            0.0,
            0,
        ),
        // Linea che esce dall'area: resta la parte fuori.
        (
            "MULTIPOLYGON(((0 0,10 0,10 10,0 10,0 0)),((5 5,10 5,15 5,5 5)))",
            100.0,
            5.0,
            0,
        ),
        // Linea sul bordo dell'area: coperta.
        (
            "MULTIPOLYGON(((0 0,10 0,10 10,0 10,0 0)),((10 2,10 4,10 6,10 2)))",
            100.0,
            0.0,
            0,
        ),
        // Anello collassato in un punto fuori dall'area: il punto resta.
        (
            "MULTIPOLYGON(((0 0,10 0,10 10,0 10,0 0)),((20 20,20 20,20 20,20 20)))",
            100.0,
            0.0,
            1,
        ),
        // ...e dentro: coperto.
        (
            "MULTIPOLYGON(((0 0,10 0,10 10,0 10,0 0)),((5 5,5 5,5 5,5 5)))",
            100.0,
            0.0,
            0,
        ),
    ];
    for (input, area, length, point_count) in cases {
        let output =
            make_valid_geometry_rust(&wkt(input), RepairMethod::Structure, true, CENTIMETRO)
                .unwrap_or_else(|errore| panic!("{input}: {errore}"));
        assert!(
            (polygonal_area(&output) - area).abs() < 1e-9,
            "{input}: {output:?}"
        );
        assert!(
            (line_length(&output) - length).abs() < 1e-9,
            "{input}: {output:?}"
        );
        assert_eq!(points(&output), point_count, "{input}: {output:?}");
    }
}

/// Generatore di anelli annidati della campagna, seme 1 caso 109: il ponte
/// della shell passa (in aritmetica esatta) per il vertice `(0.55, 1.3)` del
/// buco, in `f64` a un ULP; l'incrocio arrotondato cade a `3e-16` da quel
/// vertice. Il kernel senza controllo dava 36 m^2, GEOS 88: errore esplicito.
#[test]
fn linework_rejects_a_rounded_crossing_next_to_another_feature() {
    let input = wkt(
        "POLYGON((0.3 -0.7,1.3 7.3,2.8 7.3,2.8 19.3,1.3 19.3,1.3 7.3,0.3 -0.7,0.3 27.3,3.8 27.3,3.8 -0.7,0.3 -0.7),\
         (0.55 1.3,1.3 7.3,1.55 9.3,2.55 9.3,2.55 17.3,1.55 17.3,1.55 9.3,1.3 7.3,1.3 19.3,2.8 19.3,2.8 7.3,1.3 7.3,\
         0.55 1.3,0.55 25.3,3.55 25.3,3.55 1.3,0.55 1.3))",
    );
    for keep_collapsed in [false, true] {
        assert!(matches!(
            make_valid_geometry_rust(&input, RepairMethod::Linework, keep_collapsed, CENTIMETRO),
            Err(MakeValidError::PrecisionInsufficient)
        ));
    }
    // Buchi sovrapposti per `1e-15` (scala di epsilon del laboratorio): gli
    // incroci sono esatti, e il risultato resta quello di GEOS.
    let exact = wkt("POLYGON((0 0,10 0,10 10,0 10,0 0),(1 1,5 1,5 5,1 5,1 1),\
         (4.999999999999999 3,9 3,9 8,4.999999999999999 8,4.999999999999999 3))");
    assert!(make_valid_geometry_rust(&exact, RepairMethod::Linework, false, CENTIMETRO).is_ok());
}

/// Il tratto ripercorso `(0 0) -> (5 5) -> (0 0)` sparisce dal
/// polygonize, e il campione interno del quadrato `(5, 5)` sta su di esso.
/// L'avvolgimento per perturbazione simbolica e' quello della faccia (1):
/// il quadrato, come GEOS, come shell e come buco.
#[test]
fn structure_winding_ignores_retraced_segments_under_the_sample() {
    let cases = [
        ("POLYGON((0 0,10 0,10 10,0 10,0 0,5 5,0 0))", 100.0),
        ("POLYGON((0 0,10 0,10 10,0 10,0 0,5 5,5 8,5 5,0 0))", 100.0),
        (
            "POLYGON((-1 -1,11 -1,11 11,-1 11,-1 -1),(0 0,10 0,10 10,0 10,0 0,5 5,0 0))",
            44.0,
        ),
    ];
    for (input, geos) in cases {
        let output =
            make_valid_geometry_rust(&wkt(input), RepairMethod::Structure, false, CENTIMETRO)
                .unwrap_or_else(|errore| panic!("{input}: {errore}"));
        assert!(
            (polygonal_area(&output) - geos).abs() < 1e-9,
            "{input}: {output:?}"
        );
    }
}
