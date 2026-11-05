//! Regressioni e determinismo del backend Rust di `geo.make_valid`,
//! `geo.polygonize` e `geo.split`.
//!
//! - Il blocker del laboratorio (`seed=2147483647`, caso `227`): il caso e'
//!   rigenerato con il generatore della campagna differenziale
//!   (`polygonize_reference`, `fuzz_polygonize_matches_geos`), non copiato;
//!   l'attesa e' l'esito GEOS registrato in
//!   `results/geo-rust/FULL_FUZZ_BLOCKER.md` (8 poligoni, 0 cut edge, 3
//!   dangle).
//! - Determinismo: ogni operazione due volte, byte per byte.
//! - Forma canonica: input permutato o invertito dove il contratto la
//!   promette (coordinate e intersezioni esattamente rappresentabili, vedi
//!   `rust_backend::polygonize_linework`), byte per byte; altrove solo
//!   l'equivalenza per classe e area.

// Il generatore riproduce l'aritmetica della campagna del laboratorio: un
// `mul_add` cambierebbe l'arrotondamento e quindi il caso generato.
#![allow(clippy::suboptimal_flops)]

use std::sync::Arc;

use geo::{
    line_string, polygon, Area, Distance, Euclidean, Geometry, LineString, MultiLineString,
    Polygon, Relate,
};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{Array, BinaryArray, Int64Array, StringArray, UInt64Array};
use plenora_core::arrow::{DataType, Field, RecordBatch, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{geometry_output_field, DEFAULT_GEOMETRY_COLUMN};
use plenora_kernels_geo::rust_backend::arrow::{
    make_valid_batches, polygonize_batches, split_batches, PolygonizeParams,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::rust_backend::{
    make_valid_wkb, polygonize_linework, split_polygon_by_linework, PolygonizeResult, RepairMethod,
};

const LIMIT: u64 = 1_000_000;

/// Precisione dei test in unita' astratte (la griglia degli overlay resta
/// sotto su ogni estensione usata qui).
fn precisione() -> Precision {
    Precision::new(1e-6).expect("precisione")
}

fn polygonize(linework: &Geometry<f64>, node_input: bool) -> PolygonizeResult {
    polygonize_linework(
        linework,
        node_input,
        false,
        LIMIT,
        LIMIT,
        LIMIT,
        LIMIT,
        precisione(),
    )
    .expect("polygonize")
}

fn multi(lines: Vec<LineString<f64>>) -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString::new(lines))
}

/// Le linee in ordine inverso, ognuna percorsa al contrario.
fn reversed(lines: &[LineString<f64>]) -> Vec<LineString<f64>> {
    lines
        .iter()
        .rev()
        .map(|line| LineString::new(line.0.iter().rev().copied().collect()))
        .collect()
}

/// Una permutazione fissa che non e' ne' l'identita' ne' l'inversione.
fn interleaved(lines: &[LineString<f64>]) -> Vec<LineString<f64>> {
    let (even, odd): (Vec<_>, Vec<_>) = lines
        .iter()
        .cloned()
        .enumerate()
        .partition(|(i, _)| i % 2 == 0);
    odd.into_iter().chain(even).map(|(_, line)| line).collect()
}

// ---------------------------------------------------------------------------
// Il blocker `seed=2147483647`, caso 227.
// ---------------------------------------------------------------------------

/// Il generatore di `fuzz_polygonize_matches_geos`, riscritto senza `as`:
/// stessi valori (interi piccoli, esatti in `f64`).
struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    const fn next(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }

    fn index(&mut self, upper: usize) -> usize {
        let upper = u64::try_from(upper).expect("limite");
        usize::try_from(self.next() % upper).expect("indice")
    }

    fn integer(&mut self, minimum: i32, maximum: i32) -> f64 {
        let width = u64::try_from(i64::from(maximum) - i64::from(minimum) + 1).expect("ampiezza");
        let offset = i32::try_from(self.next() % width).expect("offset");
        f64::from(minimum + offset)
    }
}

fn fuzz_rectangle(rng: &mut FuzzRng) -> LineString<f64> {
    let minimum_x = rng.integer(-8, 6);
    let minimum_y = rng.integer(-8, 6);
    let maximum_x = minimum_x + rng.integer(1, 6);
    let maximum_y = minimum_y + rng.integer(1, 6);
    LineString::from(vec![
        (minimum_x, minimum_y),
        (maximum_x, minimum_y),
        (maximum_x, maximum_y),
        (minimum_x, maximum_y),
        (minimum_x, minimum_y),
    ])
}

fn transformed_line(
    line: &LineString<f64>,
    (scale_x, scale_y, offset_x, offset_y): (f64, f64, f64, f64),
    reverse: bool,
) -> LineString<f64> {
    let mut coordinates = line
        .0
        .iter()
        .map(|c| (c.x * scale_x + offset_x, c.y * scale_y + offset_y))
        .collect::<Vec<_>>();
    if reverse {
        coordinates.reverse();
    }
    LineString::from(coordinates)
}

/// Il caso `case` (con noding) del seme `seed`, consumando il generatore
/// esattamente come la campagna, compreso il ramo senza noding.
fn fuzz_case(seed: u64, case: usize) -> Vec<LineString<f64>> {
    let transforms = [
        (1.0, 1.0, 0.0, 0.0),
        (0.25, 2.0, 0.3, -0.7),
        (-2.0, 0.5, 13.0, -11.0),
        (1_000_000.0, 0.000_001, -2_000_000.0, 0.000_003),
    ];
    let mut rng = FuzzRng::new(seed);
    for case_index in 0..=case {
        let mut lines = Vec::new();
        let rectangle_count = rng.index(4) + 1;
        for _ in 0..rectangle_count {
            lines.push(fuzz_rectangle(&mut rng));
        }
        let cutter_count = rng.index(4);
        for _ in 0..cutter_count {
            let position = rng.integer(-10, 10);
            let line = match rng.index(3) {
                0 => LineString::from(vec![(position, -12.0), (position, 12.0)]),
                1 => LineString::from(vec![(-12.0, position), (12.0, position)]),
                _ => LineString::from(vec![(-12.0, position), (12.0, -position)]),
            };
            lines.push(line);
        }
        if rng.index(8) == 0 {
            if let Some(first) = lines.first().cloned() {
                lines.push(first);
            }
        }
        let transform = transforms[rng.index(transforms.len())];
        let reverse = rng.index(2) == 1;
        let mut transformed = lines
            .iter()
            .map(|line| transformed_line(line, transform, reverse))
            .collect::<Vec<_>>();
        if reverse {
            transformed.reverse();
        }
        if case_index == case {
            return transformed;
        }
        // Ramo senza noding della campagna: consuma il generatore.
        for _ in 0..rectangle_count {
            for (minimum, maximum) in [(-2, 2), (-8, 8), (1, 8), (1, 8)] {
                rng.integer(minimum, maximum);
            }
        }
    }
    Vec::new()
}

#[test]
fn blocker_seed_2147483647_caso_227_conserva_la_faccia() {
    let lines = fuzz_case(2_147_483_647, 227);
    assert!(!lines.is_empty(), "caso generato");
    let result = polygonize(&multi(lines.clone()), true);
    // Esito GEOS registrato nel dossier del blocker.
    assert_eq!(result.polygons.len(), 8, "poligoni");
    assert!(result.cut_edges.is_empty(), "cut edge");
    assert_eq!(result.dangles.len(), 3, "dangle");
    // La faccia persa dal vecchio noding aveva area 3,5.
    assert!(
        result
            .polygons
            .iter()
            .any(|polygon| (polygon.unsigned_area() - 3.5).abs() <= 3.5 * 1e-9),
        "faccia di area 3,5 assente"
    );
    // Anche permutato, invertito e dall'adapter Arrow: stesse classi.
    for variant in [reversed(&lines), interleaved(&lines)] {
        let other = polygonize(&multi(variant), true);
        assert_eq!(other.polygons.len(), 8);
        assert!(other.cut_edges.is_empty());
        assert_eq!(other.dangles.len(), 3);
    }
    let (schema, batch) = table(&wkb_lines(&lines));
    let (out_schema, batches) = polygonize_batches(
        &schema,
        &[batch],
        DEFAULT_GEOMETRY_COLUMN,
        PolygonizeParams::default(),
        LIMIT,
        precisione(),
    )
    .expect("polygonize Arrow");
    let classes = batches[0]
        .column(out_schema.index_of("__class").expect("classe"))
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::StringArray>()
        .expect("utf8");
    let classes: Vec<&str> = (0..classes.len()).map(|row| classes.value(row)).collect();
    let count = |class: &str| classes.iter().filter(|value| **value == class).count();
    assert_eq!(
        (count("polygon"), count("cut_edge"), count("dangle")),
        (8, 0, 3)
    );
    assert_eq!(classes.len(), 11, "nessun anello invalido");
}

/// Il test del laboratorio `preserves_near_endpoint_face_on_anisotropic_linework`
/// e' lo stesso caso: le sei linee coincidono a meno dell'ordine.
#[test]
fn il_test_unitario_del_laboratorio_e_il_caso_227() {
    let mut generated = fuzz_case(2_147_483_647, 227)
        .into_iter()
        .map(|line| format!("{:?}", line.0))
        .collect::<Vec<_>>();
    let mut unit = [
        line_string![(x: 6_000_000.0, y: 0.000_015), (x: 6_000_000.0, y: -0.000_009)],
        line_string![(x: 10_000_000.0, y: 0.000_007), (x: -14_000_000.0, y: -0.000_000_999_999_999_999_999_7)],
        line_string![(x: -3_000_000.0, y: 0.0), (x: -3_000_000.0, y: 0.000_002_000_000_000_000_000_3), (x: 0.0, y: 0.000_002_000_000_000_000_000_3), (x: 0.0, y: 0.0), (x: -3_000_000.0, y: 0.0)],
        line_string![(x: 1_000_000.0, y: -0.000_001_999_999_999_999_999_5), (x: 1_000_000.0, y: 0.000_004), (x: 6_000_000.0, y: 0.000_004), (x: 6_000_000.0, y: -0.000_001_999_999_999_999_999_5), (x: 1_000_000.0, y: -0.000_001_999_999_999_999_999_5)],
        line_string![(x: 4_000_000.0, y: 0.000_004_999_999_999_999_999_6), (x: 4_000_000.0, y: 0.000_007), (x: 10_000_000.0, y: 0.000_007), (x: 10_000_000.0, y: 0.000_004_999_999_999_999_999_6), (x: 4_000_000.0, y: 0.000_004_999_999_999_999_999_6)],
        line_string![(x: 4_000_000.0, y: -0.000_000_999_999_999_999_999_7), (x: 4_000_000.0, y: 0.0), (x: 5_000_000.0, y: 0.0), (x: 5_000_000.0, y: -0.000_000_999_999_999_999_999_7), (x: 4_000_000.0, y: -0.000_000_999_999_999_999_999_7)],
    ]
    .into_iter()
    .map(|line| format!("{:?}", line.0))
    .collect::<Vec<_>>();
    generated.sort();
    unit.sort();
    assert_eq!(generated, unit);
}

// ---------------------------------------------------------------------------
// Determinismo e forma canonica.
// ---------------------------------------------------------------------------

/// Griglia 4x3 piu' due diagonali: incroci su coordinate intere e a meta'
/// (esattamente rappresentabili), vertici di grado diverso, un dangle.
fn integer_network() -> Vec<LineString<f64>> {
    let mut lines = Vec::new();
    for x in 0..=4 {
        let x = f64::from(x);
        lines.push(line_string![(x: x, y: 0.0), (x: x, y: 3.0)]);
    }
    for y in 0..=3 {
        let y = f64::from(y);
        lines.push(line_string![(x: 0.0, y: y), (x: 4.0, y: y)]);
    }
    lines.push(line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 2.0)]);
    lines.push(line_string![(x: 0.0, y: 3.0), (x: 2.0, y: 1.0)]);
    lines.push(line_string![(x: 4.0, y: 3.0), (x: 6.0, y: 5.0)]);
    lines
}

// Facce ricavate dalle intersezioni razionali delle rette della fixture,
// indipendentemente dal kernel. Gli anelli fissano copertura, non solo area.
// Fixture esplicite: coordinate e metadati indipendenti dalla produzione.
#[allow(clippy::too_many_lines)]
fn integer_faces() -> Vec<Polygon<f64>> {
    vec![
        Polygon::new(
            LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (1.0, 0.5), (0.0, 0.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (1.0, 0.5),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.0, 0.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (0.0, 1.0),
                (1.0, 1.0),
                (1.0, 2.0),
                (0.0, 2.0),
                (0.0, 1.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(0.0, 2.0), (1.0, 2.0), (0.0, 3.0), (0.0, 2.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(0.0, 3.0), (1.0, 2.0), (1.0, 3.0), (0.0, 3.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (1.0, 0.0),
                (2.0, 0.0),
                (2.0, 1.0),
                (1.0, 0.5),
                (1.0, 0.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(1.0, 0.5), (2.0, 1.0), (1.0, 1.0), (1.0, 0.5)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(1.0, 1.0), (2.0, 1.0), (1.0, 2.0), (1.0, 1.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(1.0, 2.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (1.0, 2.0),
                (2.0, 2.0),
                (2.0, 3.0),
                (1.0, 3.0),
                (1.0, 2.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (2.0, 0.0),
                (3.0, 0.0),
                (3.0, 1.0),
                (2.0, 1.0),
                (2.0, 0.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(2.0, 1.0), (3.0, 1.0), (3.0, 1.5), (2.0, 1.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (2.0, 1.0),
                (3.0, 1.5),
                (3.0, 2.0),
                (2.0, 2.0),
                (2.0, 1.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (2.0, 2.0),
                (3.0, 2.0),
                (3.0, 3.0),
                (2.0, 3.0),
                (2.0, 2.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (3.0, 0.0),
                (4.0, 0.0),
                (4.0, 1.0),
                (3.0, 1.0),
                (3.0, 0.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (3.0, 1.0),
                (4.0, 1.0),
                (4.0, 2.0),
                (3.0, 1.5),
                (3.0, 1.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(3.0, 1.5), (4.0, 2.0), (3.0, 2.0), (3.0, 1.5)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (3.0, 2.0),
                (4.0, 2.0),
                (4.0, 3.0),
                (3.0, 3.0),
                (3.0, 2.0),
            ]),
            vec![],
        ),
    ]
}
fn split_faces() -> Vec<Polygon<f64>> {
    vec![
        Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (5.0, 0.0),
                (5.0, 2.0),
                (2.0, 2.0),
                (0.0, 0.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(0.0, 0.0), (2.0, 2.0), (0.0, 2.0), (0.0, 0.0)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (0.0, 2.0),
                (2.0, 2.0),
                (4.0, 4.0),
                (4.0, 6.0),
                (5.0, 6.0),
                (5.0, 10.0),
                (0.0, 10.0),
                (0.0, 2.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (2.0, 2.0),
                (5.0, 2.0),
                (5.0, 4.0),
                (4.0, 4.0),
                (2.0, 2.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (5.0, 0.0),
                (10.0, 0.0),
                (10.0, 2.0),
                (5.0, 2.0),
                (5.0, 0.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (5.0, 2.0),
                (10.0, 2.0),
                (10.0, 10.0),
                (6.0, 6.0),
                (6.0, 4.0),
                (5.0, 4.0),
                (5.0, 2.0),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (5.0, 6.0),
                (6.0, 6.0),
                (10.0, 10.0),
                (5.0, 10.0),
                (5.0, 6.0),
            ]),
            vec![],
        ),
    ]
}
fn nonrepresentable_faces() -> Vec<Polygon<f64>> {
    vec![
        Polygon::new(
            LineString::from(vec![(-1.2, -6.7), (-0.2, -6.7), (-0.2, -2.7), (-1.2, -6.7)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![(-1.2, -6.7), (-0.2, -2.7), (-1.2, -2.7), (-1.2, -6.7)]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (-1.2, -2.7),
                (-0.2, -2.7),
                (-0.2, 5.3),
                (-1.2, 5.3),
                (-1.2, -2.7),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (-0.2, -2.7),
                (1.3, -2.7),
                (1.55, -2.7),
                (2.55, -2.7),
                (2.55, 8.3),
                (-0.2, -2.7),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (0.3, 9.3),
                (1.05, 9.3),
                (1.05, 13.3),
                (0.3, 13.3),
                (0.3, 9.3),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (0.3, 13.3),
                (1.05, 13.3),
                (1.05, 17.3),
                (0.3, 17.3),
                (0.3, 13.3),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (1.05, 9.3),
                (1.8, 9.3),
                (1.8, 13.3),
                (1.05, 13.3),
                (1.05, 9.3),
            ]),
            vec![],
        ),
        Polygon::new(
            LineString::from(vec![
                (1.3, -10.7),
                (1.55, -10.7),
                (1.55, -2.7),
                (1.3, -2.7),
                (1.3, -10.7),
            ]),
            vec![],
        ),
    ]
}

fn assert_faces(actual: &[Polygon<f64>], expected: &[Polygon<f64>]) {
    assert_eq!(actual.len(), expected.len());
    let mut remaining = expected.to_vec();
    for polygon in actual {
        let position = remaining
            .iter()
            .position(|expected| polygon.relate(expected).is_equal_topo())
            .expect("faccia con copertura attesa");
        remaining.remove(position);
    }
}

fn assert_integer_network(result: &PolygonizeResult, node_input: bool) {
    if node_input {
        assert_faces(&result.polygons, &integer_faces());
        assert_eq!(result.dangles.len(), 1);
        assert!(result.dangles[0]
            .relate(&line_string![(x: 4.0, y: 3.0), (x: 6.0, y: 5.0)])
            .is_equal_topo());
    } else {
        // Senza noding solo i quattro lati esterni hanno estremi comuni.
        assert_faces(
            &result.polygons,
            &[polygon![(x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
            (x: 4.0, y: 3.0), (x: 0.0, y: 3.0), (x: 0.0, y: 0.0)]],
        );
        let expected = MultiLineString(vec![
            line_string![(x: 1.0, y: 0.0), (x: 1.0, y: 3.0)],
            line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 3.0)],
            line_string![(x: 3.0, y: 0.0), (x: 3.0, y: 3.0)],
            line_string![(x: 0.0, y: 1.0), (x: 4.0, y: 1.0)],
            line_string![(x: 0.0, y: 2.0), (x: 4.0, y: 2.0)],
            line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 2.0)],
            line_string![(x: 0.0, y: 3.0), (x: 2.0, y: 1.0)],
            line_string![(x: 4.0, y: 3.0), (x: 6.0, y: 5.0)],
        ]);
        assert!(MultiLineString(result.dangles.clone())
            .relate(&expected)
            .is_equal_topo());
    }
    assert!(result.cut_edges.is_empty());
    assert!(result.invalid_ring_lines.is_empty());
}

fn assert_nonrepresentable(result: &PolygonizeResult) {
    // docs/topologia.md: otto facce macroscopiche e la scheggia esatta che
    // GEOS scarta. Aree razionali: 2,2,8,121/8,3,3,3,2, totale 305/8.
    assert_eq!(counts(result), [9, 0, 4, 0]);
    // Le sei code esterne sono quattro componenti residue: le due code
    // incidenti in (2.55, -2.7) e quelle in (2.55, 8.3) si concatenano.
    // Gli estremi di innesto vengono dall'intersezione y = 4x - 1.9
    // con x = 2.55 e con i bordi dei rettangoli della fixture.
    let mut expected_dangles = vec![
        line_string![(x: 2.55, y: -24.7), (x: 2.55, y: -2.7), (x: 3.3, y: -2.7)],
        line_string![(x: 2.55, y: 23.3), (x: 2.55, y: 8.3), (x: 3.3, y: 11.3)],
        line_string![(x: -2.7, y: -2.7), (x: -1.2, y: -2.7)],
        line_string![(x: -2.7, y: -12.7), (x: -1.2, y: -6.7)],
    ];
    let length = |line: &LineString<f64>| {
        line.lines()
            .map(|segment| (segment.end.x - segment.start.x).hypot(segment.end.y - segment.start.y))
            .sum::<f64>()
    };
    for actual in &result.dangles {
        let position = expected_dangles
            .iter()
            .position(|expected| {
                (length(actual) - length(expected)).abs() < 1e-12
                    && line_close(actual, expected)
                    && line_close(expected, actual)
            })
            .expect("copertura e lunghezza del residuo atteso");
        expected_dangles.remove(position);
    }
    assert!(expected_dangles.is_empty());
    let mut remaining = nonrepresentable_faces();
    let mut slivers = 0;
    for polygon in &result.polygons {
        if polygon.unsigned_area() < 1e-30 {
            slivers += 1;
            assert!(polygon
                .exterior()
                .0
                .iter()
                .all(|c| (c.x + 0.2).abs() < 1e-12 && (c.y + 2.7).abs() < 1e-12));
            continue;
        }
        let position = remaining
            .iter()
            .position(|expected| {
                (polygon.unsigned_area() - expected.unsigned_area()).abs() < 1e-12
                    && boundary_close(polygon, expected)
                    && boundary_close(expected, polygon)
            })
            .expect("copertura della faccia razionale entro l'arrotondamento");
        remaining.remove(position);
    }
    assert_eq!(slivers, 1);
    assert!(remaining.is_empty());
    assert!((result.polygons.iter().map(Area::unsigned_area).sum::<f64>() - 38.125).abs() < 1e-12);
}

fn boundary_close(actual: &Polygon<f64>, expected: &Polygon<f64>) -> bool {
    actual.interiors().is_empty()
        && expected.interiors().is_empty()
        && line_close(actual.exterior(), expected.exterior())
}

fn line_close(actual: &LineString<f64>, expected: &LineString<f64>) -> bool {
    actual.lines().all(|line| {
        [
            line.start,
            line.end,
            geo::Coord {
                x: f64::midpoint(line.start.x, line.end.x),
                y: f64::midpoint(line.start.y, line.end.y),
            },
        ]
        .iter()
        .all(|c| Euclidean.distance(&geo::Point(*c), expected) < 1e-12)
    })
}

fn repaired_faces(bowtie: bool, method: RepairMethod) -> Vec<Polygon<f64>> {
    if bowtie {
        vec![
            polygon![(x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 2.0, y: 2.0), (x: 0.0, y: 0.0)],
            polygon![(x: 0.0, y: 4.0), (x: 2.0, y: 2.0), (x: 4.0, y: 4.0), (x: 0.0, y: 4.0)],
        ]
    } else {
        let mut faces = vec![
            polygon![(x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 1.0),
            (x: 3.0, y: 1.0), (x: 3.0, y: 3.0), (x: 4.0, y: 3.0), (x: 4.0, y: 4.0),
            (x: 0.0, y: 4.0), (x: 0.0, y: 0.0)],
        ];
        if matches!(method, RepairMethod::Linework) {
            faces.push(polygon![(x: 4.0, y: 1.0), (x: 5.0, y: 1.0), (x: 5.0, y: 3.0), (x: 4.0, y: 3.0), (x: 4.0, y: 1.0)]);
        }
        faces
    }
}

fn polygons(geometry: Geometry<f64>) -> Vec<Polygon<f64>> {
    match geometry {
        Geometry::Polygon(p) => vec![p],
        Geometry::MultiPolygon(p) => p.0,
        _ => panic!("atteso esito soltanto poligonale"),
    }
}

#[test]
fn polygonize_deterministico_e_canonico_su_input_permutato() {
    let lines = integer_network();
    for node_input in [true, false] {
        let base = polygonize(&multi(lines.clone()), node_input);
        assert_integer_network(&base, node_input);
        assert_eq!(
            base,
            polygonize(&multi(lines.clone()), node_input),
            "due volte"
        );
        if !node_input {
            continue;
        }
        assert!(!base.polygons.is_empty() && !base.dangles.is_empty());
        for variant in [reversed(&lines), interleaved(&lines)] {
            assert_eq!(
                base,
                polygonize(&multi(variant), true),
                "input permutato o invertito"
            );
        }
    }
}

/// Classe per classe: poligoni, cut edge, dangle, anelli invalidi.
const fn counts(result: &PolygonizeResult) -> [usize; 4] {
    [
        result.polygons.len(),
        result.cut_edges.len(),
        result.dangles.len(),
        result.invalid_ring_lines.len(),
    ]
}

/// Intersezioni proprie non rappresentabili (le linee del test
/// `iterated_noding_matches_geos_on_near_coincident_crossings`): il
/// contratto non promette byte identici su input permutato, ma lo stesso
/// esito per classe.
#[test]
fn polygonize_equivalente_per_classe_su_intersezioni_non_rappresentabili() {
    let lines = vec![
        line_string![(x: 2.55, y: 23.3), (x: 2.55, y: -24.7)],
        line_string![(x: 3.3, y: 11.3), (x: -2.7, y: -12.7)],
        line_string![(x: 3.3, y: -2.7), (x: -2.7, y: -2.7)],
        line_string![(x: 0.3, y: 9.3), (x: 0.3, y: 17.3), (x: 1.05, y: 17.3), (x: 1.05, y: 9.3), (x: 0.3, y: 9.3)],
        line_string![(x: 1.3, y: -10.7), (x: 1.3, y: -2.7), (x: 1.55, y: -2.7), (x: 1.55, y: -10.7), (x: 1.3, y: -10.7)],
        line_string![(x: -1.2, y: -6.7), (x: -1.2, y: 5.3), (x: -0.2, y: 5.3), (x: -0.2, y: -6.7), (x: -1.2, y: -6.7)],
        line_string![(x: 0.3, y: 9.3), (x: 0.3, y: 13.3), (x: 1.8, y: 13.3), (x: 1.8, y: 9.3), (x: 0.3, y: 9.3)],
    ];
    let base = polygonize(&multi(lines.clone()), true);
    assert_nonrepresentable(&base);
    assert_eq!(base, polygonize(&multi(lines.clone()), true), "due volte");
    for variant in [reversed(&lines), interleaved(&lines)] {
        let other = polygonize(&multi(variant), true);
        assert_nonrepresentable(&other);
        assert_eq!(counts(&base), counts(&other));
        let total = |result: &PolygonizeResult| {
            result.polygons.iter().map(Area::unsigned_area).sum::<f64>()
        };
        assert!((total(&base) - total(&other)).abs() <= 1e-12);
    }
}

fn holed_source() -> Geometry<f64> {
    Geometry::Polygon(Polygon::new(
        line_string![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0), (x: 10.0, y: 10.0),
            (x: 0.0, y: 10.0), (x: 0.0, y: 0.0)
        ],
        vec![line_string![
            (x: 4.0, y: 4.0), (x: 6.0, y: 4.0), (x: 6.0, y: 6.0),
            (x: 4.0, y: 6.0), (x: 4.0, y: 4.0)
        ]],
    ))
}

fn split_lines() -> Vec<LineString<f64>> {
    vec![
        line_string![(x: 5.0, y: -1.0), (x: 5.0, y: 11.0)],
        line_string![(x: -1.0, y: 2.0), (x: 11.0, y: 2.0)],
        line_string![(x: -1.0, y: -1.0), (x: 11.0, y: 11.0)],
    ]
}

fn split(source: &Geometry<f64>, lines: Vec<LineString<f64>>) -> Vec<Polygon<f64>> {
    split_polygon_by_linework(
        source,
        &multi(lines),
        LIMIT,
        LIMIT,
        LIMIT,
        LIMIT,
        precisione(),
    )
    .expect("split")
}

#[test]
fn split_deterministico_e_canonico_su_input_permutato() {
    let source = holed_source();
    let base = split(&source, split_lines());
    assert_faces(&base, &split_faces());
    assert_eq!(base, split(&source, split_lines()), "due volte");
    let Geometry::Polygon(polygon) = &source else {
        unreachable!("sorgente poligonale")
    };
    let reversed_source = Geometry::Polygon(Polygon::new(
        LineString::new(polygon.exterior().0.iter().rev().copied().collect()),
        polygon
            .interiors()
            .iter()
            .map(|ring| LineString::new(ring.0.iter().rev().copied().collect()))
            .collect(),
    ));
    for lines in [reversed(&split_lines()), interleaved(&split_lines())] {
        assert_eq!(base, split(&source, lines.clone()), "splitter permutato");
        assert_eq!(base, split(&reversed_source, lines), "anelli invertiti");
    }
}

fn bow_tie_wkb() -> Vec<u8> {
    Geometry::Polygon(polygon![
        (x: 0.0, y: 0.0), (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
        (x: 4.0, y: 0.0), (x: 0.0, y: 0.0)
    ])
    .to_wkb(CoordDimensions::xy())
    .expect("wkb")
}

/// `make_valid` non promette forma canonica su input riorientato: solo
/// stesso output sullo stesso input.
#[test]
fn make_valid_deterministico() {
    let crossing_hole = Geometry::Polygon(Polygon::new(
        line_string![
            (x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 4.0),
            (x: 0.0, y: 4.0), (x: 0.0, y: 0.0)
        ],
        vec![line_string![
            (x: 3.0, y: 1.0), (x: 5.0, y: 1.0), (x: 5.0, y: 3.0),
            (x: 3.0, y: 3.0), (x: 3.0, y: 1.0)
        ]],
    ))
    .to_wkb(CoordDimensions::xy())
    .expect("wkb");
    for (bowtie, payload) in [(true, bow_tie_wkb()), (false, crossing_hole)] {
        for method in [RepairMethod::Linework, RepairMethod::Structure] {
            for keep_collapsed in [false, true] {
                let first = make_valid_wkb(&payload, method, keep_collapsed, precisione())
                    .expect("riparata");
                assert_faces(
                    &polygons(plenora_kernels_geo::geometry_from_wkb(&first).expect("WKB valido")),
                    &repaired_faces(bowtie, method),
                );
                let second = make_valid_wkb(&payload, method, keep_collapsed, precisione())
                    .expect("riparata");
                assert_eq!(first, second, "{method:?} keep_collapsed={keep_collapsed}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter Arrow: due volte e righe permutate.
// ---------------------------------------------------------------------------

const CRS: &str = "EPSG:3857";

fn table(cells: &[Vec<u8>]) -> (SchemaRef, RecordBatch) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        {
            let campo = geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("campo");
            let mut metadata = campo.metadata().clone();
            metadata.insert(
                "plenora.geometry.axis_order".to_owned(),
                "easting_northing".to_owned(),
            );
            campo.with_metadata(metadata)
        },
    ]));
    let ids = (0..cells.len())
        .map(|row| i64::try_from(row).expect("id"))
        .collect::<Vec<_>>();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(plenora_core::arrow::array::Int64Array::from(ids)),
            Arc::new(
                cells
                    .iter()
                    .map(|cell| Some(cell.as_slice()))
                    .collect::<BinaryArray>(),
            ),
        ],
    )
    .expect("batch");
    (schema, batch)
}

fn wkb_lines(lines: &[LineString<f64>]) -> Vec<Vec<u8>> {
    lines
        .iter()
        .map(|line| {
            Geometry::LineString(line.clone())
                .to_wkb(CoordDimensions::xy())
                .expect("wkb")
        })
        .collect()
}

fn output_field(input: &Field, nullable: bool) -> Field {
    let mut metadata = input.metadata().clone();
    metadata.insert(
        "plenora.geometry.precision".to_owned(),
        "float64".to_owned(),
    );
    Field::new(input.name(), DataType::Binary, nullable).with_metadata(metadata)
}

#[test]
fn polygonize_arrow_deterministico_e_canonico_su_righe_permutate() {
    let lines = integer_network();
    let run = |cells: &[Vec<u8>]| {
        let (schema, batch) = table(cells);
        let (out_schema, output) = polygonize_batches(
            &schema,
            &[batch],
            DEFAULT_GEOMETRY_COLUMN,
            PolygonizeParams::default(),
            LIMIT,
            precisione(),
        )
        .expect("polygonize");
        assert_eq!(out_schema.fields().len(), 2);
        assert_eq!(out_schema.field(0), &output_field(schema.field(1), false));
        assert_eq!(
            out_schema.field(1),
            &Field::new("__class", DataType::Utf8, false)
        );
        assert_eq!(out_schema.metadata(), schema.metadata());
        assert_eq!(output.len(), 1);
        let geometries = output[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .expect("WKB");
        let classes = output[0]
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("classi");
        assert_eq!(geometries.null_count(), 0);
        assert_eq!(classes.null_count(), 0);
        let mut result = PolygonizeResult {
            polygons: vec![],
            cut_edges: vec![],
            dangles: vec![],
            invalid_ring_lines: vec![],
        };
        for row in 0..output[0].num_rows() {
            match (
                classes.value(row),
                plenora_kernels_geo::geometry_from_wkb(geometries.value(row))
                    .expect("geometria valida"),
            ) {
                ("polygon", Geometry::Polygon(p)) => result.polygons.push(p),
                ("dangle", Geometry::LineString(l)) => result.dangles.push(l),
                other => panic!("classe o forma inattesa: {other:?}"),
            }
        }
        assert_integer_network(&result, true);
        output
    };
    let base = run(&wkb_lines(&lines));
    assert_eq!(base, run(&wkb_lines(&lines)), "due volte");
    assert_eq!(base, run(&wkb_lines(&reversed(&lines))), "righe invertite");
    assert_eq!(
        base,
        run(&wkb_lines(&interleaved(&lines))),
        "righe permutate"
    );
}

#[test]
// Fixture esplicite: coordinate e metadati indipendenti dalla produzione.
#[allow(clippy::too_many_lines)]
fn make_valid_e_split_arrow_deterministici() {
    let square = polygon![
        (x: 0.0, y: 0.0), (x: 2.0, y: 0.0), (x: 2.0, y: 2.0), (x: 0.0, y: 2.0), (x: 0.0, y: 0.0)
    ];
    let cells = vec![
        bow_tie_wkb(),
        Geometry::Polygon(square)
            .to_wkb(CoordDimensions::xy())
            .expect("wkb"),
    ];
    let (schema, batch) = table(&cells);
    let first = make_valid_batches(
        &schema,
        std::slice::from_ref(&batch),
        DEFAULT_GEOMETRY_COLUMN,
        precisione(),
    )
    .expect("make_valid");
    let second = make_valid_batches(
        &schema,
        std::slice::from_ref(&batch),
        DEFAULT_GEOMETRY_COLUMN,
        precisione(),
    )
    .expect("make_valid");
    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].num_rows(), 2);
    assert_eq!(first[0].schema().field(0), schema.field(0));
    assert_eq!(
        first[0].schema().field(1),
        &output_field(schema.field(1), true)
    );
    assert_eq!(first[0].schema().metadata(), schema.metadata());
    assert_eq!(first[0].column(0), batch.column(0));
    let repaired = first[0]
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("WKB");
    assert_eq!(repaired.null_count(), 0);
    assert_faces(
        &polygons(plenora_kernels_geo::geometry_from_wkb(repaired.value(0)).expect("riparata")),
        &repaired_faces(true, RepairMethod::Linework),
    );
    assert_eq!(repaired.value(1), cells[1]);

    let source = holed_source().to_wkb(CoordDimensions::xy()).expect("wkb");
    let splitter = multi(split_lines())
        .to_wkb(CoordDimensions::xy())
        .expect("wkb");
    let (schema, batch) = table(&[source.clone(), source]);
    let splitters = [Some(splitter.as_slice()), Some(splitter.as_slice())]
        .into_iter()
        .collect::<BinaryArray>();
    let run = || {
        split_batches(
            &schema,
            std::slice::from_ref(&batch),
            DEFAULT_GEOMETRY_COLUMN,
            &splitters,
            None,
            LIMIT,
            precisione(),
        )
        .expect("split")
        .1
    };
    let base = run();
    assert_eq!(base, run());
    assert_eq!(base.len(), 1);
    assert_eq!(base[0].num_rows(), 14);
    assert_eq!(base[0].schema().fields().len(), 3);
    assert_eq!(base[0].schema().field(0), schema.field(0));
    assert_eq!(
        base[0].schema().field(1),
        &output_field(schema.field(1), false)
    );
    assert_eq!(
        base[0].schema().field(2),
        &Field::new("__parent_index", DataType::UInt64, false)
    );
    assert_eq!(base[0].schema().metadata(), schema.metadata());
    let parts = base[0]
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .expect("WKB");
    let ids = base[0]
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("id");
    let parents = base[0]
        .column(2)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("genitori");
    assert_eq!(parts.null_count(), 0);
    assert_eq!(ids.null_count(), 0);
    assert_eq!(parents.null_count(), 0);
    for (parent, start) in [(0, 0), (1, 7)] {
        let mut actual = Vec::new();
        for row in start..start + 7 {
            assert_eq!(ids.value(row), parent);
            assert_eq!(parents.value(row), u64::try_from(parent).expect("indice"));
            actual.extend(polygons(
                plenora_kernels_geo::geometry_from_wkb(parts.value(row)).expect("parte valida"),
            ));
        }
        assert_faces(&actual, &split_faces());
    }
}
