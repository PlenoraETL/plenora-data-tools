//! `geo.make_valid`: l'esito non dipende dall'ordine di anelli e poligoni, e
//! `LINEWORK` e' quello di GEOS.
//!
//! - Due controesempi (un buco che condivide un lato con
//!   la shell e ne sporge, le parti sovrapposte di un `MultiPolygon`) in ogni
//!   ordine: stessa geometria, e l'area e le linee che GEOS 3.14 da' su
//!   quegli input (`MakeValid` `LINEWORK`, sonda della campagna
//!   differenziale del laboratorio, registrata: qui GEOS non gira). Non e' l'area pari-dispari sugli anelli: GEOS
//!   costruisce l'area a giri (`BuildArea`, XOR, lati meno il bordo), e sui
//!   due controesempi da' 112 e 96 dove il pari-dispari darebbe 52 e 56.
//! - Otto casi della classe B della campagna differenziale (`p5`, seme e caso
//!   nel nome), con l'output GEOS registrato come attesa: area, lati delle
//!   linee, punti.
//! - Una proprieta': anelli e parti permutati (e per `LINEWORK` anche
//!   ruotati e invertiti) danno la stessa geometria, per entrambi i metodi e
//!   entrambi i valori di `keep_collapsed`.

use std::collections::BTreeMap;

use geo::{Area, Coord, Geometry, LineString, MultiPolygon, Polygon};
use plenora_kernels_geo::rust_backend::make_valid::{make_valid_geometry_rust, RepairMethod};
use proptest::prelude::*;
use wkt::TryFromWkt;

/// Coordinate astratte fino a qualche decina di unita'.
const PRECISION: f64 = 1e-6;

fn wkt(text: &str) -> Geometry<f64> {
    Geometry::try_from_wkt_str(text).expect("WKT del test")
}

fn repaired(input: &Geometry<f64>, method: RepairMethod) -> Geometry<f64> {
    make_valid_geometry_rust(input, method, false, PRECISION).expect("riparazione")
}

fn polygonal_area(geometry: &Geometry<f64>) -> f64 {
    match geometry {
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => geometry.unsigned_area(),
        Geometry::GeometryCollection(collection) => collection.0.iter().map(polygonal_area).sum(),
        _ => 0.0,
    }
}

type SegmentKey = ((u64, u64), (u64, u64));

fn key(coordinate: Coord<f64>) -> (u64, u64) {
    (
        (coordinate.x + 0.0).to_bits(),
        (coordinate.y + 0.0).to_bits(),
    )
}

/// I lati delle linee (non degli anelli), come multinsieme non orientato:
/// il confronto della campagna differenziale.
fn line_segments(geometry: &Geometry<f64>, output: &mut BTreeMap<SegmentKey, usize>) {
    let mut add = |line: &LineString<f64>| {
        for segment in line.lines() {
            let (start, end) = (key(segment.start), key(segment.end));
            let entry = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            *output.entry(entry).or_default() += 1;
        }
    };
    match geometry {
        Geometry::LineString(line) => add(line),
        Geometry::MultiLineString(lines) => lines.0.iter().for_each(add),
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                line_segments(child, output);
            }
        }
        _ => {}
    }
}

fn segments_of(geometry: &Geometry<f64>) -> BTreeMap<SegmentKey, usize> {
    let mut output = BTreeMap::new();
    line_segments(geometry, &mut output);
    output
}

/// Stessa area e stessi lati delle linee dell'output GEOS.
fn assert_matches_geos(name: &str, output: &Geometry<f64>, geos: &Geometry<f64>) {
    let expected = polygonal_area(geos);
    let actual = polygonal_area(output);
    assert!(
        (actual - expected).abs() <= 1e-9 * expected.max(1.0),
        "{name}: area {actual}, GEOS {expected}"
    );
    assert_eq!(segments_of(output), segments_of(geos), "{name}: linee");
}

fn square(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> LineString<f64> {
    LineString::from(vec![
        (min_x, min_y),
        (max_x, min_y),
        (max_x, max_y),
        (min_x, max_y),
        (min_x, min_y),
    ])
}

fn permutations(items: &[usize]) -> Vec<Vec<usize>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut output = Vec::new();
    for (index, first) in items.iter().enumerate() {
        let mut rest = items.to_vec();
        rest.remove(index);
        for mut tail in permutations(&rest) {
            tail.insert(0, *first);
            output.push(tail);
        }
    }
    output
}

/// `S = [0, 10]^2`, `A = [9, 11] x [2, 4]`, `B = [2, 12] x [0, 8]`: `B`
/// condivide con la shell il lato `y = 0` fra 2 e 10 e ne sporge. Il
/// kernel precedente dava 114 con i buchi in ordine `A, B` e 112 in ordine
/// `B, A`. GEOS: 112, cioe' `(S U B) \ A`, e le linee interne rimaste.
#[test]
fn hole_sharing_an_edge_with_the_shell_in_every_order() {
    let geos = wkt(
        "GEOMETRYCOLLECTION(POLYGON((10 10,10 8,12 8,12 0,10 0,2 0,0 0,0 10,10 10),\
         (10 2,11 2,11 4,10 4,9 4,9 2,10 2)),\
         MULTILINESTRING((10 0,10 2),(10 2,10 4),(10 4,10 8),(10 8,2 8,2 0)))",
    );
    assert!((polygonal_area(&geos) - 112.0).abs() < 1e-12);
    let holes = [square(9.0, 2.0, 11.0, 4.0), square(2.0, 0.0, 12.0, 8.0)];
    let mut outputs = Vec::new();
    for order in permutations(&[0, 1]) {
        let input = Geometry::Polygon(Polygon::new(
            square(0.0, 0.0, 10.0, 10.0),
            order.iter().map(|index| holes[*index].clone()).collect(),
        ));
        let output = repaired(&input, RepairMethod::Linework);
        assert_matches_geos(&format!("buchi {order:?}"), &output, &geos);
        outputs.push(output);
    }
    assert!(outputs.windows(2).all(|pair| pair[0] == pair[1]));
}

/// `S = [0, 10]^2`, `A = [3, 5]^2`, `B = [2, 8] x [0, 8]` come parti di un
/// `MultiPolygon`: il kernel precedente dava 100 in ordine `S, A, B` e 96 in
/// ordine `S, B, A`. GEOS: 96, `S \ A`, e il contorno interno di `B` come
/// linea. Le sei permutazioni danno la stessa geometria.
#[test]
fn overlapping_multipolygon_parts_in_every_order() {
    let geos = wkt(
        "GEOMETRYCOLLECTION(POLYGON((0 0,0 10,10 10,10 0,8 0,2 0,0 0),(5 5,3 5,3 3,5 3,5 5)),\
         LINESTRING(8 0,8 8,2 8,2 0))",
    );
    assert!((polygonal_area(&geos) - 96.0).abs() < 1e-12);
    let parts = [
        square(0.0, 0.0, 10.0, 10.0),
        square(3.0, 3.0, 5.0, 5.0),
        square(2.0, 0.0, 8.0, 8.0),
    ];
    for method in [RepairMethod::Linework, RepairMethod::Structure] {
        let mut outputs = Vec::new();
        for order in permutations(&[0, 1, 2]) {
            let input = Geometry::MultiPolygon(MultiPolygon::new(
                order
                    .iter()
                    .map(|index| Polygon::new(parts[*index].clone(), Vec::new()))
                    .collect(),
            ));
            let output = repaired(&input, method);
            if method == RepairMethod::Linework {
                assert_matches_geos(&format!("parti {order:?}"), &output, &geos);
            } else {
                // `GeometryFixer`: l'unione delle parti.
                assert!((polygonal_area(&output) - 100.0).abs() < 1e-9);
            }
            outputs.push(output);
        }
        assert!(
            outputs.windows(2).all(|pair| pair[0] == pair[1]),
            "{method:?}: l'ordine delle parti cambia l'esito"
        );
    }
}

/// Casi della classe B della campagna `p5` (standard), `LINEWORK`: input e
/// output GEOS come registrati dalla sonda della campagna. In tutti un buco
/// condivide un lato con la shell e ne interseca un altro.
#[test]
fn campaign_class_b_cases_match_geos() {
    let cases = [
        (
            "seed=1 case=68",
            "POLYGON((0 0,0 18,9 18,9 0,0 0),(1 1,1 13,4 13,4 1,1 1),(1.5 6,1.5 18,4.5 18,4.5 6,1.5 6))",
            "GEOMETRYCOLLECTION(POLYGON((0 0,0 18,1.5 18,4.5 18,9 18,9 0,0 0),(1 1,4 1,4 6,4 13,1.5 13,1 13,1 1)),\
             MULTILINESTRING((1.5 6,1.5 13),(1.5 13,1.5 18),(4.5 18,4.5 6,4 6),(4 6,1.5 6)))",
        ),
        (
            "seed=1 case=84",
            "POLYGON((13 -11,-23 -11,-23 -3,13 -3,13 -11),(11 -10.5,-13 -10.5,-13 -5.5,11 -5.5,11 -10.5),\
             (1 -8.5,-23 -8.5,-23 -3.5,1 -3.5,1 -8.5))",
            "GEOMETRYCOLLECTION(POLYGON((-23 -3,13 -3,13 -11,-23 -11,-23 -8.5,-23 -3.5,-23 -3),\
             (11 -10.5,11 -5.5,1 -5.5,-13 -5.5,-13 -8.5,-13 -10.5,11 -10.5)),\
             MULTILINESTRING((1 -8.5,-13 -8.5),(-13 -8.5,-23 -8.5),(-23 -3.5,1 -3.5,1 -5.5),(1 -5.5,1 -8.5)))",
        ),
        (
            "seed=1 case=107",
            "POLYGON((0.3 -0.7,0.3 35.3,3.55 35.3,3.55 -0.7,0.3 -0.7),(0.55 1.3,0.55 25.3,2.3 25.3,2.3 1.3,0.55 1.3),\
             (1.175 11.3,1.175 35.3,2.925 35.3,2.925 11.3,1.175 11.3))",
            "GEOMETRYCOLLECTION(POLYGON((0.3 -0.7,0.3 35.3,1.175 35.3,2.925 35.3,3.55 35.3,3.55 -0.7,0.3 -0.7),\
             (0.55 1.3,2.3 1.3,2.3 11.3,2.3 25.3,1.175 25.3,0.55 25.3,0.55 1.3)),\
             MULTILINESTRING((1.175 11.3,1.175 25.3),(1.175 25.3,1.175 35.3),(2.925 35.3,2.925 11.3,2.3 11.3),(2.3 11.3,1.175 11.3)))",
        ),
        (
            "seed=17 case=23",
            "POLYGON((13 -11,13 -2,-5 -2,-5 -11,13 -11),(11 -10.5,11 -4.5,5 -4.5,5 -10.5,11 -10.5),\
             (10 -8,10 -2,4 -2,4 -8,10 -8))",
            "GEOMETRYCOLLECTION(POLYGON((-5 -11,-5 -2,4 -2,10 -2,13 -2,13 -11,-5 -11),\
             (10 -4.5,5 -4.5,5 -8,5 -10.5,11 -10.5,11 -4.5,10 -4.5)),\
             MULTILINESTRING((10 -8,10 -4.5),(10 -4.5,10 -2),(4 -2,4 -8,5 -8),(5 -8,10 -8)))",
        ),
        (
            "seed=257 case=8",
            "POLYGON((0 0,18 0,18 17,0 17,0 0),(1 1,13 1,13 12,1 12,1 1),(6 5.5,18 5.5,18 16.5,6 16.5,6 5.5))",
            "GEOMETRYCOLLECTION(POLYGON((0 17,18 17,18 16.5,18 5.5,18 0,0 0,0 17),\
             (13 5.5,13 12,6 12,1 12,1 1,13 1,13 5.5)),\
             MULTILINESTRING((6 5.5,13 5.5),(13 5.5,18 5.5),(18 16.5,6 16.5,6 12),(6 12,6 5.5)))",
        ),
        (
            "seed=65537 case=12",
            "POLYGON((13 -11,13 -6.5,-23 -6.5,-23 -11,13 -11),(11 -10.5,11 -9,-13 -9,-13 -10.5,11 -10.5),\
             (1 -10.25,1 -8.75,-23 -8.75,-23 -10.25,1 -10.25))",
            "GEOMETRYCOLLECTION(POLYGON((13 -11,-23 -11,-23 -10.25,-23 -8.75,-23 -6.5,13 -6.5,13 -11),\
             (1 -9,-13 -9,-13 -10.25,-13 -10.5,11 -10.5,11 -9,1 -9)),\
             MULTILINESTRING((1 -10.25,1 -9),(1 -9,1 -8.75,-23 -8.75),(-23 -10.25,-13 -10.25),(-13 -10.25,1 -10.25)))",
        ),
        (
            "seed=2147483647 case=124",
            "POLYGON((0.3 -0.7,0.3 35.3,2.55 35.3,2.55 -0.7,0.3 -0.7),(0.55 1.3,0.55 25.3,1.3 25.3,1.3 1.3,0.55 1.3),\
             (0.675 11.3,0.675 35.3,1.425 35.3,1.425 11.3,0.675 11.3))",
            "GEOMETRYCOLLECTION(POLYGON((0.3 -0.7,0.3 35.3,0.675 35.3,1.425 35.3,2.55 35.3,2.55 -0.7,0.3 -0.7),\
             (0.55 1.3,1.3 1.3,1.3 11.3,1.3 25.3,0.675 25.3,0.55 25.3,0.55 1.3)),\
             MULTILINESTRING((0.675 11.3,0.675 25.3),(0.675 25.3,0.675 35.3),(1.425 35.3,1.425 11.3,1.3 11.3),(1.3 11.3,0.675 11.3)))",
        ),
        (
            "seed=32416190071 case=110",
            "POLYGON((0.3 -0.7,0.3 19.3,4.8 19.3,4.8 -0.7,0.3 -0.7),(0.55 1.3,0.55 9.3,3.55 9.3,3.55 1.3,0.55 1.3),\
             (1.8 3.3,1.8 11.3,4.8 11.3,4.8 3.3,1.8 3.3))",
            "GEOMETRYCOLLECTION(POLYGON((0.3 19.3,4.8 19.3,4.8 11.3,4.8 3.3,4.8 -0.7,0.3 -0.7,0.3 19.3),\
             (0.55 1.3,3.55 1.3,3.55 3.3,3.55 9.3,1.8 9.3,0.55 9.3,0.55 1.3)),\
             MULTILINESTRING((1.8 3.3,1.8 9.3),(1.8 9.3,1.8 11.3,4.8 11.3),(4.8 3.3,3.55 3.3),(3.55 3.3,1.8 3.3)))",
        ),
    ];
    for (name, input, geos) in cases {
        let input = wkt(input);
        let geos = wkt(geos);
        let output = repaired(&input, RepairMethod::Linework);
        assert_matches_geos(name, &output, &geos);
        // Buchi in ordine inverso: stessa geometria.
        let Geometry::Polygon(polygon) = &input else {
            panic!("{name}: poligono atteso");
        };
        let mut holes = polygon.interiors().to_vec();
        holes.reverse();
        let reversed = Geometry::Polygon(Polygon::new(polygon.exterior().clone(), holes));
        assert_eq!(
            repaired(&reversed, RepairMethod::Linework),
            output,
            "{name}"
        );
    }
}

// ---- proprieta': permutazioni su input casuali

fn ring_strategy() -> impl Strategy<Value = LineString<f64>> {
    prop_oneof![
        (0_i32..12, 0_i32..12, 1_i32..8, 1_i32..8).prop_map(|(x, y, w, h)| square(
            f64::from(x),
            f64::from(y),
            f64::from(x + w),
            f64::from(y + h)
        )),
        (
            (0_i32..12, 0_i32..12),
            (0_i32..12, 0_i32..12),
            (0_i32..12, 0_i32..12)
        )
            .prop_map(|(a, b, c)| {
                let point = |(x, y): (i32, i32)| (f64::from(x), f64::from(y));
                LineString::from(vec![point(a), point(b), point(c), point(a)])
            }),
        // Bow-tie: un anello che si auto-interseca.
        (0_i32..10, 0_i32..10, 1_i32..5).prop_map(|(x, y, s)| {
            let (x, y, s) = (f64::from(x), f64::from(y), f64::from(s));
            LineString::from(vec![(x, y), (x + s, y + s), (x, y + s), (x + s, y), (x, y)])
        }),
    ]
}

fn polygon_strategy() -> impl Strategy<Value = Polygon<f64>> {
    (
        ring_strategy(),
        prop::collection::vec(ring_strategy(), 0..4),
    )
        .prop_map(|(shell, holes)| Polygon::new(shell, holes))
}

/// Mescola con un generatore lineare congruenziale: la permutazione e'
/// una funzione del seme.
fn shuffle<T>(items: &mut [T], seed: &mut u64) {
    for index in (1..items.len()).rev() {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let bound = u64::try_from(index + 1).expect("indice");
        let other = usize::try_from((*seed >> 33) % bound).expect("indice");
        items.swap(index, other);
    }
}

/// Anello ruotato di `shift` vertici e, se richiesto, invertito: stesso
/// insieme di lati.
fn rotated(ring: &LineString<f64>, shift: usize, reverse: bool) -> LineString<f64> {
    let open = &ring.0[..ring.0.len().saturating_sub(1)];
    if open.is_empty() {
        return ring.clone();
    }
    let mut coordinates = Vec::with_capacity(ring.0.len());
    for index in 0..open.len() {
        coordinates.push(open[(index + shift) % open.len()]);
    }
    if reverse {
        coordinates.reverse();
    }
    coordinates.push(coordinates[0]);
    LineString::new(coordinates)
}

fn permuted(polygons: &[Polygon<f64>], seed: u64, rotate: bool) -> Vec<Polygon<f64>> {
    let mut state = seed;
    let mut output = polygons
        .iter()
        .map(|polygon| {
            let mut holes = polygon.interiors().to_vec();
            shuffle(&mut holes, &mut state);
            if rotate {
                state = state
                    .wrapping_mul(2_862_933_555_777_941_757)
                    .wrapping_add(3);
                let shift = usize::try_from(state >> 60).expect("indice");
                let reverse = state & 1 == 1;
                Polygon::new(
                    rotated(polygon.exterior(), shift, reverse),
                    holes
                        .iter()
                        .map(|hole| rotated(hole, shift + 1, !reverse))
                        .collect(),
                )
            } else {
                Polygon::new(polygon.exterior().clone(), holes)
            }
        })
        .collect::<Vec<_>>();
    shuffle(&mut output, &mut state);
    output
}

fn outcome(
    input: &Geometry<f64>,
    method: RepairMethod,
    keep_collapsed: bool,
) -> Result<Geometry<f64>, String> {
    make_valid_geometry_rust(input, method, keep_collapsed, PRECISION)
        .map_err(|error| error.to_string())
}

fn as_geometry(polygons: Vec<Polygon<f64>>) -> Geometry<f64> {
    if polygons.len() == 1 {
        Geometry::Polygon(polygons.into_iter().next().expect("un poligono"))
    } else {
        Geometry::MultiPolygon(MultiPolygon::new(polygons))
    }
}

/// `pieni` con `PLENORA_TEST_LUNGHI=1` (suite lunga, README «Suite lunga»),
/// `ridotti` altrimenti; un valore diverso da `0` e `1` ferma il test. Copia
/// di `casi` di `test_support`, che un test d'integrazione non raggiunge.
fn casi(ridotti: u32, pieni: u32) -> u32 {
    match std::env::var("PLENORA_TEST_LUNGHI") {
        Err(std::env::VarError::NotPresent) => ridotti,
        Ok(valore) if valore == "0" => ridotti,
        Ok(valore) if valore == "1" => pieni,
        _ => panic!("PLENORA_TEST_LUNGHI vale 1 (suite lunga) o 0"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(casi(32, 256)))]

    /// Buchi e parti permutati: stessa geometria (o stesso errore), salvo
    /// l'input gia' valido, che passa invariato e quindi nell'ordine suo.
    /// Per `LINEWORK` anche gli anelli ruotati e invertiti.
    #[test]
    fn repair_does_not_depend_on_ring_or_part_order(
        polygons in prop::collection::vec(polygon_strategy(), 1..4),
        seed in any::<u64>(),
    ) {
        let original = as_geometry(polygons.clone());
        for method in [RepairMethod::Linework, RepairMethod::Structure] {
            let variant = as_geometry(permuted(&polygons, seed, method == RepairMethod::Linework));
            for keep_collapsed in [false, true] {
                let first = outcome(&original, method, keep_collapsed);
                let second = outcome(&variant, method, keep_collapsed);
                if first.as_ref() == Ok(&original) {
                    // Passthrough dell'input valido.
                    prop_assert_eq!(second, Ok(variant.clone()));
                } else {
                    prop_assert_eq!(
                        &first, &second,
                        "{:?} keep={}: input {:?}", method, keep_collapsed, original
                    );
                }
            }
        }
    }
}

/// Buchi che si toccano in due vertici: il bordo dei
/// figli della faccia esce dal polygonize in catene aperte, che il
/// laboratorio scartava, e la faccia perdeva il buco centrale coprendone la
/// faccia figlia. GEOS: la shell con i tre buchi, piu' il bow-tie diviso,
/// area `100 - 4 - 4 - 2 + 2 = 92`, nessuna linea.
#[test]
fn holes_touching_at_two_vertices_stay_holes() {
    let input = wkt(
        "MULTIPOLYGON(((0 0,10 0,10 10,0 10,0 0),(1 4,3 4,3 6,1 6,1 4),(3 6,5 6,5 8,3 8,3 6),\
         (5 8,7 8,7 9,5 9,5 8)),((20 0,22 2,22 0,20 2,20 0)))",
    );
    let geos = wkt(
        "MULTIPOLYGON(((20 2,21 1,20 0,20 2)),((21 1,22 2,22 0,21 1)),((10 0,0 0,0 10,10 10,10 0),\
         (1 6,1 4,3 4,3 6,1 6),(5 6,5 8,3 8,3 6,5 6),(7 8,7 9,5 9,5 8,7 8)))",
    );
    let output = repaired(&input, RepairMethod::Linework);
    assert_matches_geos("buchi pizzicati", &output, &geos);
    assert!((polygonal_area(&output) - 92.0).abs() < 1e-12);
}
