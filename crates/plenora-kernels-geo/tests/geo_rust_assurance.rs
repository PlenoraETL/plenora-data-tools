//! La campagna di assurance indipendente da GEOS dei tre kernel Rust.
//!
//! Portata da `plenora-memory-lab/operations/geo_rust/assurance/src/main.rs`
//! (1.097 controlli, verdi su Windows e Linux `x86_64` nel laboratorio). Il
//! verdetto non usa GEOS ne' l'overlay: le attese vengono da costruzioni con
//! topologia nota (griglie, split rettangolari, bow-tie), area shoelace,
//! chiusura e finitudine degli anelli e un controllo indipendente delle
//! auto-intersezioni. Le trasformazioni usano potenze di due, quindi le
//! attese restano esatte.
//!
//! Stesse categorie, stessi casi, stesso generatore (xorshift con lo stesso
//! seme), stessi conteggi. Differenze dal binario del laboratorio: una
//! funzione di test per categoria invece di un `main`, conversioni numeriche
//! scritte con `From`/`try_from` dove il laboratorio usava `as` (stessi
//! valori: interi piccoli, esatti in `f64`), e i kernel chiamati dal modulo
//! `rust_backend` invece dei crate candidati.

// Stessa aritmetica del laboratorio: un `mul_add` cambierebbe
// l'arrotondamento delle trasformazioni e degli oracoli d'area, i confronti
// esatti fra `f64` sono voluti (attese esatte), e le funzioni restano quelle
// della sorgente, lunghe come la' e con gli stessi nomi.
#![allow(
    clippy::suboptimal_flops,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::too_many_lines
)]

use geo::{
    line_string, polygon, Coord, CoordsIter, Geometry, GeometryCollection, LineString, MapCoords,
    MultiLineString, Point, Polygon,
};
use geozero::{wkb::Wkb, CoordDimensions, ToGeo, ToWkb};
use plenora_kernels_geo::rust_backend::make_valid::{
    make_valid_geometry_rust_bounded, make_valid_geometry_rust_with_limits, MakeValidError,
    MakeValidLimits, RepairMethod,
};
use plenora_kernels_geo::rust_backend::polygonize::{
    polygonize_linework_rust, polygonize_linework_rust_bounded, PolygonizeError, PolygonizeLimits,
    PolygonizeOptions, PolygonizeResult,
};
use plenora_kernels_geo::rust_backend::split::{
    split_polygon_by_linework_rust, split_polygon_by_linework_rust_bounded, SplitError, SplitLimits,
};

#[derive(Debug)]
struct AssuranceError(String);

impl std::fmt::Display for AssuranceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

type AssuranceResult<T> = Result<T, AssuranceError>;

#[derive(Clone, Copy)]
struct Affine {
    xx: f64,
    xy: f64,
    yx: f64,
    yy: f64,
    tx: f64,
    ty: f64,
}

impl Affine {
    fn determinant(self) -> f64 {
        self.xx * self.yy - self.xy * self.yx
    }

    fn apply(self, geometry: &Geometry<f64>) -> Geometry<f64> {
        geometry.map_coords(|coordinate| Coord {
            x: self.xx * coordinate.x + self.xy * coordinate.y + self.tx,
            y: self.yx * coordinate.x + self.yy * coordinate.y + self.ty,
        })
    }
}

fn require(condition: bool, message: impl Into<String>) -> AssuranceResult<()> {
    if condition {
        Ok(())
    } else {
        Err(AssuranceError(message.into()))
    }
}

fn close(left: f64, right: f64, relative: f64) -> bool {
    if left == right {
        return true;
    }
    let scale = left.abs().max(right.abs());
    let tolerance = (scale * relative).max(f64::MIN_POSITIVE);
    (left - right).abs() <= tolerance
}

fn orientation(first: Coord<f64>, second: Coord<f64>, third: Coord<f64>) -> f64 {
    (second.x - first.x) * (third.y - first.y) - (second.y - first.y) * (third.x - first.x)
}

fn on_segment(start: Coord<f64>, point: Coord<f64>, end: Coord<f64>) -> bool {
    orientation(start, point, end) == 0.0
        && point.x >= start.x.min(end.x)
        && point.x <= start.x.max(end.x)
        && point.y >= start.y.min(end.y)
        && point.y <= start.y.max(end.y)
}

fn segments_intersect(
    left_start: Coord<f64>,
    left_end: Coord<f64>,
    right_start: Coord<f64>,
    right_end: Coord<f64>,
) -> bool {
    let first = orientation(left_start, left_end, right_start);
    let second = orientation(left_start, left_end, right_end);
    let third = orientation(right_start, right_end, left_start);
    let fourth = orientation(right_start, right_end, left_end);
    if ((first > 0.0 && second < 0.0) || (first < 0.0 && second > 0.0))
        && ((third > 0.0 && fourth < 0.0) || (third < 0.0 && fourth > 0.0))
    {
        return true;
    }
    (first == 0.0 && on_segment(left_start, right_start, left_end))
        || (second == 0.0 && on_segment(left_start, right_end, left_end))
        || (third == 0.0 && on_segment(right_start, left_start, right_end))
        || (fourth == 0.0 && on_segment(right_start, left_end, right_end))
}

fn require_simple_ring(ring: &LineString<f64>) -> AssuranceResult<()> {
    let segment_count = ring.0.len().saturating_sub(1);
    for left_index in 0..segment_count {
        for right_index in left_index.saturating_add(1)..segment_count {
            let adjacent = right_index == left_index.saturating_add(1)
                || (left_index == 0 && right_index.saturating_add(1) == segment_count);
            if adjacent {
                continue;
            }
            let left_start = ring.0[left_index];
            let left_end = ring.0[left_index + 1];
            let right_start = ring.0[right_index];
            let right_end = ring.0[right_index + 1];
            require(
                !segments_intersect(left_start, left_end, right_start, right_end),
                "anello output auto-intersecante secondo l'oracolo indipendente",
            )?;
        }
    }
    Ok(())
}

fn signed_ring_area(ring: &LineString<f64>) -> AssuranceResult<f64> {
    require(
        ring.0.len() >= 4,
        "anello output con meno di quattro coordinate",
    )?;
    let first = ring
        .0
        .first()
        .copied()
        .ok_or_else(|| AssuranceError("anello output vuoto".to_owned()))?;
    let last = ring
        .0
        .last()
        .copied()
        .ok_or_else(|| AssuranceError("anello output vuoto".to_owned()))?;
    require(first == last, "anello output non chiuso")?;
    require(
        ring.0
            .iter()
            .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite()),
        "anello output con coordinate non finite",
    )?;
    require_simple_ring(ring)?;
    let twice_area = ring.0.windows(2).try_fold(0.0, |total, pair| {
        let [left, right] = pair else {
            return Err(AssuranceError(
                "finestra anello senza due coordinate".to_owned(),
            ));
        };
        Ok(total + left.x * right.y - right.x * left.y)
    })?;
    Ok(twice_area * 0.5)
}

fn polygon_area(polygon: &Polygon<f64>) -> AssuranceResult<f64> {
    let shell = signed_ring_area(polygon.exterior())?.abs();
    let holes = polygon.interiors().iter().try_fold(0.0, |total, ring| {
        Ok::<f64, AssuranceError>(total + signed_ring_area(ring)?.abs())
    })?;
    let area = shell - holes;
    require(area >= 0.0, "area poligonale indipendente negativa")?;
    Ok(area)
}

fn polygons_area(polygons: &[Polygon<f64>]) -> AssuranceResult<f64> {
    polygons
        .iter()
        .try_fold(0.0, |total, polygon| Ok(total + polygon_area(polygon)?))
}

fn geometry_area(geometry: &Geometry<f64>) -> AssuranceResult<f64> {
    match geometry {
        Geometry::Polygon(polygon) => polygon_area(polygon),
        Geometry::MultiPolygon(polygons) => polygons_area(&polygons.0),
        Geometry::GeometryCollection(collection) => collection
            .0
            .iter()
            .try_fold(0.0, |total, child| Ok(total + geometry_area(child)?)),
        _ => Ok(0.0),
    }
}

fn geometry_dimensions(geometry: &Geometry<f64>) -> (usize, usize, usize) {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => (0, 0, 1),
        Geometry::Line(_) | Geometry::LineString(_) | Geometry::MultiLineString(_) => (0, 1, 0),
        Geometry::Polygon(_)
        | Geometry::MultiPolygon(_)
        | Geometry::Rect(_)
        | Geometry::Triangle(_) => (1, 0, 0),
        Geometry::GeometryCollection(collection) => {
            collection
                .0
                .iter()
                .fold((0, 0, 0), |(areas, lines, points), child| {
                    let child_dimensions = geometry_dimensions(child);
                    (
                        areas + child_dimensions.0,
                        lines + child_dimensions.1,
                        points + child_dimensions.2,
                    )
                })
        }
    }
}

fn rectangle(width: f64, height: f64) -> Geometry<f64> {
    Geometry::Polygon(polygon![
        (x: 0.0, y: 0.0),
        (x: width, y: 0.0),
        (x: width, y: height),
        (x: 0.0, y: height),
        (x: 0.0, y: 0.0)
    ])
}

fn rectangular_grid(columns: u32, rows: u32) -> Geometry<f64> {
    let mut lines = Vec::new();
    for column in 0..=columns {
        let x = f64::from(column);
        lines.push(line_string![(x: x, y: 0.0), (x: x, y: f64::from(rows))]);
    }
    for row in 0..=rows {
        let y = f64::from(row);
        lines.push(line_string![(x: 0.0, y: y), (x: f64::from(columns), y: y)]);
    }
    Geometry::MultiLineString(MultiLineString::new(lines))
}

fn vertical_splitters(width: u32, height: u32, parts: u32) -> Geometry<f64> {
    let mut lines = Vec::new();
    for index in 1..parts {
        let x = f64::from(width) * f64::from(index) / f64::from(parts);
        lines.push(line_string![(x: x, y: -1.0), (x: x, y: f64::from(height) + 1.0)]);
    }
    Geometry::MultiLineString(MultiLineString::new(lines))
}

/// Precisione dichiarata della campagna: le coordinate sono astratte, e i
/// kernel la vogliono esplicita. `2^-20` del lato minore del rettangolo
/// d'ingombro: molto sotto ogni feature delle costruzioni, anche con le
/// trasformazioni anisotrope, e sopra il bilancio degli overlay (con
/// `i_overlay` 4.5 due diagonali del passo `2^-30`, al piu' `2^-28` del lato
/// maggiore; con il motore `i64` di 9.0 `1 + sqrt(2)` diagonali di `span *
/// 2^-49 + 4 ulp(M)`, molto meno) finche' l'anisotropia resta sotto `2^8`.
fn precisione(geometry: &Geometry<f64>) -> f64 {
    use geo::BoundingRect;
    geometry.bounding_rect().map_or(1.0, |rect| {
        let (minore, maggiore) = (
            rect.width().min(rect.height()),
            rect.width().max(rect.height()),
        );
        (minore * 2_f64.powi(-20))
            .max(maggiore * 2_f64.powi(-28))
            .max(f64::MIN_POSITIVE)
    })
}

/// Precisione per i casi fissi dei limiti (estensione al piu' 20).
const PRECISIONE: f64 = 1e-6;

fn polygonize(geometry: &Geometry<f64>) -> AssuranceResult<PolygonizeResult> {
    polygonize_linework_rust(
        geometry,
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            limits: PolygonizeLimits::unlimited(),
            precision: precisione(geometry),
        },
    )
    .map_err(|error| AssuranceError(format!("polygonize: {error}")))
}

const fn unlimited_split_limits() -> SplitLimits {
    SplitLimits {
        max_input_coordinates: u64::MAX,
        max_noding_work: u64::MAX,
        max_output_parts: u64::MAX,
        max_output_coordinates: u64::MAX,
    }
}

const fn unlimited_make_valid_limits() -> MakeValidLimits {
    MakeValidLimits {
        max_input_coordinates: u64::MAX,
        max_noding_work: u64::MAX,
        max_output_geometries: u64::MAX,
        max_output_coordinates: u64::MAX,
    }
}

fn assert_polygonize_oracle(
    geometry: &Geometry<f64>,
    expected_faces: u32,
    expected_area: f64,
    label: &str,
) -> AssuranceResult<()> {
    let output = polygonize(geometry)?;
    require(
        output.polygons.len() == usize::try_from(expected_faces).unwrap_or(usize::MAX),
        format!(
            "{label}: facce={}, attese={expected_faces}",
            output.polygons.len()
        ),
    )?;
    require(
        output.cut_edges.is_empty()
            && output.dangles.is_empty()
            && output.invalid_ring_lines.is_empty(),
        format!("{label}: residui inattesi"),
    )?;
    let area = polygons_area(&output.polygons)?;
    require(
        close(area, expected_area, 1e-10),
        format!("{label}: area={area}, attesa={expected_area}"),
    )
}

fn assert_split_oracle(
    source: &Geometry<f64>,
    splitter: &Geometry<f64>,
    expected_parts: u32,
    expected_area: f64,
    label: &str,
) -> AssuranceResult<()> {
    let output = split_polygon_by_linework_rust(
        source,
        splitter,
        unlimited_split_limits(),
        precisione(source),
    )
    .map_err(|error| AssuranceError(format!("{label}: split: {error}")))?;
    require(
        output.len() == usize::try_from(expected_parts).unwrap_or(usize::MAX),
        format!("{label}: parti={}, attese={expected_parts}", output.len()),
    )?;
    let area = polygons_area(&output)?;
    require(
        close(area, expected_area, 1e-10),
        format!("{label}: area={area}, attesa={expected_area}"),
    )
}

fn assert_make_valid_oracle(
    geometry: &Geometry<f64>,
    method: RepairMethod,
    expected_area: f64,
    label: &str,
) -> AssuranceResult<()> {
    let output = make_valid_geometry_rust_with_limits(
        geometry,
        method,
        true,
        unlimited_make_valid_limits(),
        precisione(geometry),
    )
    .map_err(|error| AssuranceError(format!("{label}: make_valid: {error}")))?;
    let area = geometry_area(&output)?;
    require(
        close(area, expected_area, 1e-10),
        format!("{label}: area={area}, attesa={expected_area}"),
    )?;
    let dimensions = geometry_dimensions(&output);
    require(
        dimensions.0 > 0,
        format!("{label}: output senza componente areale"),
    )
}

fn run_independent_oracles() -> AssuranceResult<usize> {
    let grids = [(1_u32, 1_u32), (2, 3), (4, 5), (8, 7)];
    let mut checks = 0_usize;
    for (columns, rows) in grids {
        let grid = rectangular_grid(columns, rows);
        let faces = columns * rows;
        assert_polygonize_oracle(&grid, faces, f64::from(faces), "griglia intera")?;
        checks += 1;
    }

    for parts in [2_u32, 3, 5, 9] {
        let source = rectangle(18.0, 7.0);
        let splitter = vertical_splitters(18, 7, parts);
        assert_split_oracle(&source, &splitter, parts, 126.0, "split rettangolare")?;
        checks += 1;
    }

    for scale in [1.0_f64, 2.0, 8.0] {
        let bow_tie = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 2.0 * scale, y: 2.0 * scale),
            (x: 0.0, y: 2.0 * scale),
            (x: 2.0 * scale, y: 0.0),
            (x: 0.0, y: 0.0)
        ]);
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            assert_make_valid_oracle(&bow_tie, method, 2.0 * scale * scale, "bow-tie esatto")?;
            checks += 1;
        }
    }
    Ok(checks)
}

fn reverse_linework(geometry: &Geometry<f64>) -> AssuranceResult<Geometry<f64>> {
    let Geometry::MultiLineString(lines) = geometry else {
        return Err(AssuranceError(
            "reverse_linework richiede MultiLineString".to_owned(),
        ));
    };
    let mut reversed = lines.0.clone();
    reversed.reverse();
    for line in &mut reversed {
        line.0.reverse();
    }
    Ok(Geometry::MultiLineString(MultiLineString::new(reversed)))
}

fn reverse_polygon(geometry: &Geometry<f64>) -> AssuranceResult<Geometry<f64>> {
    let Geometry::Polygon(polygon) = geometry else {
        return Err(AssuranceError(
            "reverse_polygon richiede Polygon".to_owned(),
        ));
    };
    let mut shell = polygon.exterior().clone();
    shell.0.reverse();
    let mut holes = polygon.interiors().to_vec();
    holes.reverse();
    for hole in &mut holes {
        hole.0.reverse();
    }
    Ok(Geometry::Polygon(Polygon::new(shell, holes)))
}

fn run_metamorphic() -> AssuranceResult<usize> {
    let transforms = [
        Affine {
            xx: 1.0,
            xy: 0.0,
            yx: 0.0,
            yy: 1.0,
            tx: 0.0,
            ty: 0.0,
        },
        Affine {
            xx: 1.0,
            xy: 0.0,
            yx: 0.0,
            yy: 1.0,
            tx: 1_000_000.0,
            ty: -2_000_000.0,
        },
        Affine {
            xx: 8.0,
            xy: 0.0,
            yx: 0.0,
            yy: 0.125,
            tx: 0.0,
            ty: 0.0,
        },
        Affine {
            xx: -2.0,
            xy: 0.0,
            yx: 0.0,
            yy: 4.0,
            tx: 40.0,
            ty: -12.0,
        },
        Affine {
            xx: 0.0,
            xy: -1.0,
            yx: 1.0,
            yy: 0.0,
            tx: 20.0,
            ty: 30.0,
        },
        Affine {
            xx: 1_048_576.0,
            xy: 0.0,
            yx: 0.0,
            yy: 0.000_000_953_674_316_406_25,
            tx: 0.0,
            ty: 0.0,
        },
    ];
    let base_grid = rectangular_grid(3, 2);
    let base_source = rectangle(12.0, 5.0);
    let base_splitter = vertical_splitters(12, 5, 4);
    let base_bow_tie = Geometry::Polygon(polygon![
        (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
        (x: 0.0, y: 2.0), (x: 2.0, y: 0.0), (x: 0.0, y: 0.0)
    ]);
    let mut checks = 0_usize;
    for transform in transforms {
        let determinant = transform.determinant().abs();
        require(
            determinant.is_finite() && determinant > 0.0,
            "trasformazione singolare",
        )?;
        assert_polygonize_oracle(
            &transform.apply(&base_grid),
            6,
            6.0 * determinant,
            "polygonize metamorfico",
        )?;
        checks += 1;
        assert_split_oracle(
            &transform.apply(&base_source),
            &transform.apply(&base_splitter),
            4,
            60.0 * determinant,
            "split metamorfico",
        )?;
        checks += 1;
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            assert_make_valid_oracle(
                &transform.apply(&base_bow_tie),
                method,
                2.0 * determinant,
                "make_valid metamorfico",
            )?;
            checks += 1;
        }
    }

    assert_polygonize_oracle(&reverse_linework(&base_grid)?, 6, 6.0, "linework invertito")?;
    checks += 1;
    assert_split_oracle(
        &reverse_polygon(&base_source)?,
        &reverse_linework(&base_splitter)?,
        4,
        60.0,
        "split invertito",
    )?;
    checks += 1;
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        assert_make_valid_oracle(
            &reverse_polygon(&base_bow_tie)?,
            method,
            2.0,
            "make_valid invertito",
        )?;
        checks += 1;
    }
    Ok(checks)
}

struct ExactRng {
    state: u64,
}

impl ExactRng {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    const fn next(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }

    fn below(&mut self, upper: u32) -> AssuranceResult<u32> {
        require(upper > 0, "limite RNG nullo")?;
        u32::try_from(self.next() % u64::from(upper))
            .map_err(|_| AssuranceError("RNG oltre u32".to_owned()))
    }

    fn power_of_two(&mut self) -> AssuranceResult<f64> {
        let exponent = i32::try_from(self.below(25)?)
            .map_err(|_| AssuranceError("esponente RNG non rappresentabile".to_owned()))?
            - 12;
        let sign = if self.next() & 1 == 0 { 1.0 } else { -1.0 };
        Ok(sign * 2_f64.powi(exponent))
    }

    fn exact_affine(&mut self) -> AssuranceResult<Affine> {
        let xx = self.power_of_two()?;
        let yy = self.power_of_two()?;
        let tx_steps = i32::try_from(self.below(17)?)
            .map_err(|_| AssuranceError("traslazione X non rappresentabile".to_owned()))?
            - 8;
        let ty_steps = i32::try_from(self.below(17)?)
            .map_err(|_| AssuranceError("traslazione Y non rappresentabile".to_owned()))?
            - 8;
        Ok(Affine {
            xx,
            xy: 0.0,
            yx: 0.0,
            yy,
            tx: xx * f64::from(tx_steps),
            ty: yy * f64::from(ty_steps),
        })
    }
}

fn run_generated_exact_oracles() -> AssuranceResult<usize> {
    const CASES: usize = 256;

    let mut rng = ExactRng::new(0x9e37_79b9_7f4a_7c15);
    let mut checks = 0_usize;
    for case_index in 0..CASES {
        let columns = rng.below(8)? + 1;
        let rows = rng.below(8)? + 1;
        let transform = rng.exact_affine()?;
        let mut grid = rectangular_grid(columns, rows);
        if case_index & 1 == 1 {
            grid = reverse_linework(&grid)?;
        }
        let expected_faces = columns * rows;
        assert_polygonize_oracle(
            &transform.apply(&grid),
            expected_faces,
            f64::from(expected_faces) * transform.determinant().abs(),
            "polygonize generativo esatto",
        )?;
        checks += 1;

        let parts = rng.below(8)? + 2;
        let height = rng.below(8)? + 1;
        let width = parts
            .checked_mul(2)
            .ok_or_else(|| AssuranceError("larghezza split generata in overflow".to_owned()))?;
        let mut source = rectangle(f64::from(width), f64::from(height));
        let mut splitter = vertical_splitters(width, height, parts);
        if case_index & 2 == 2 {
            source = reverse_polygon(&source)?;
            splitter = reverse_linework(&splitter)?;
        }
        assert_split_oracle(
            &transform.apply(&source),
            &transform.apply(&splitter),
            parts,
            f64::from(width) * f64::from(height) * transform.determinant().abs(),
            "split generativo esatto",
        )?;
        checks += 1;

        let size = f64::from(rng.below(8)? + 1);
        let mut bow_tie = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 2.0 * size, y: 2.0 * size),
            (x: 0.0, y: 2.0 * size),
            (x: 2.0 * size, y: 0.0),
            (x: 0.0, y: 0.0)
        ]);
        if case_index & 4 == 4 {
            bow_tie = reverse_polygon(&bow_tie)?;
        }
        for method in [RepairMethod::Structure, RepairMethod::Linework] {
            assert_make_valid_oracle(
                &transform.apply(&bow_tie),
                method,
                2.0 * size * size * transform.determinant().abs(),
                "make_valid generativo esatto",
            )?;
            checks += 1;
        }
    }
    Ok(checks)
}

fn next_up(value: f64) -> f64 {
    if value == f64::INFINITY {
        value
    } else if value == -0.0 {
        f64::from_bits(1)
    } else if value >= 0.0 {
        f64::from_bits(value.to_bits() + 1)
    } else {
        f64::from_bits(value.to_bits() - 1)
    }
}

fn run_adversarial_numeric() -> AssuranceResult<usize> {
    let one_next = next_up(1.0);
    let thin = Geometry::MultiLineString(MultiLineString::new(vec![
        line_string![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0)],
        line_string![(x: 2.0, y: 0.0), (x: 2.0, y: 1.0)],
        line_string![(x: 2.0, y: 1.0), (x: 0.0, y: 1.0)],
        line_string![(x: 0.0, y: 1.0), (x: 0.0, y: 0.0)],
        line_string![(x: 1.0, y: 0.0), (x: 1.0, y: 1.0)],
        line_string![(x: one_next, y: 0.0), (x: one_next, y: 1.0)],
    ]));
    assert_polygonize_oracle(&thin, 3, 2.0, "faccia larga un ULP")?;

    let repeated = Geometry::MultiLineString(MultiLineString::new(vec![
        line_string![(x: 0.0, y: 0.0), (x: 0.0, y: 0.0), (x: 4.0, y: 0.0)],
        line_string![(x: 4.0, y: 0.0), (x: 4.0, y: 4.0), (x: 4.0, y: 4.0)],
        line_string![(x: 4.0, y: 4.0), (x: 0.0, y: 4.0)],
        line_string![(x: 0.0, y: 4.0), (x: 0.0, y: 0.0), (x: 0.0, y: 0.0)],
    ]));
    assert_polygonize_oracle(&repeated, 1, 16.0, "vertici ripetuti")?;

    let scale = Affine {
        xx: 2_f64.powi(180),
        xy: 0.0,
        yx: 0.0,
        yy: 2_f64.powi(-180),
        tx: 0.0,
        ty: 0.0,
    };
    assert_polygonize_oracle(
        &scale.apply(&rectangular_grid(2, 2)),
        4,
        4.0,
        "scala anisotropa estrema",
    )?;

    let source = Geometry::Polygon(polygon![
        (x: 1.0, y: 0.0),
        (x: 3.0, y: 0.0),
        (x: 3.0, y: 1.0),
        (x: 1.0, y: 1.0),
        (x: 1.0, y: 0.0)
    ]);
    let near_boundary = Geometry::LineString(line_string![
        (x: next_up(1.0), y: -1.0),
        (x: next_up(1.0), y: 2.0)
    ]);
    assert_split_oracle(&source, &near_boundary, 2, 2.0, "split a un ULP dal bordo")?;

    let on_boundary = Geometry::LineString(line_string![
        (x: 1.0, y: -1.0),
        (x: 1.0, y: 2.0)
    ]);
    assert_split_oracle(&source, &on_boundary, 1, 2.0, "split coincidente col bordo")?;
    let before_boundary = f64::from_bits(1.0_f64.to_bits() - 1);
    let outside = Geometry::LineString(line_string![
        (x: before_boundary, y: -1.0),
        (x: before_boundary, y: 2.0)
    ]);
    assert_split_oracle(&source, &outside, 1, 2.0, "split a un ULP fuori dal bordo")?;
    Ok(6)
}

fn wkb_round_trip(geometry: &Geometry<f64>, label: &str) -> AssuranceResult<Geometry<f64>> {
    let bytes = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| AssuranceError(format!("{label}: encoding WKB: {error}")))?;
    require(!bytes.is_empty(), format!("{label}: WKB vuoto"))?;
    let decoded = Wkb(&bytes)
        .to_geo()
        .map_err(|error| AssuranceError(format!("{label}: decoding WKB: {error}")))?;
    require(
        decoded == *geometry,
        format!("{label}: round-trip WKB non identico"),
    )?;
    Ok(decoded)
}

fn run_wkb_corpus() -> AssuranceResult<usize> {
    let grid = wkb_round_trip(&rectangular_grid(3, 3), "grid WKB")?;
    assert_polygonize_oracle(&grid, 9, 9.0, "grid WKB")?;

    let source = wkb_round_trip(&rectangle(10.0, 8.0), "source WKB")?;
    let splitter = wkb_round_trip(&vertical_splitters(10, 8, 5), "splitter WKB")?;
    assert_split_oracle(&source, &splitter, 5, 80.0, "split WKB")?;

    let bow_tie = wkb_round_trip(
        &Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 4.0, y: 4.0),
            (x: 0.0, y: 4.0), (x: 4.0, y: 0.0), (x: 0.0, y: 0.0)
        ]),
        "bow-tie WKB",
    )?;
    for method in [RepairMethod::Structure, RepairMethod::Linework] {
        assert_make_valid_oracle(&bow_tie, method, 8.0, "make_valid WKB")?;
    }

    let collection = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
        Geometry::Point(Point::new(1.0, 2.0)),
        rectangle(3.0, 2.0),
    ]));
    let decoded_collection = wkb_round_trip(&collection, "collection WKB")?;
    require(
        decoded_collection.coords_count() == collection.coords_count(),
        "collection WKB con coordinate diverse",
    )?;
    Ok(7)
}

fn run_limit_contracts() -> AssuranceResult<usize> {
    let grid = rectangular_grid(8, 8);
    let coordinate_limited = polygonize_linework_rust(
        &grid,
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            precision: PRECISIONE,
            limits: PolygonizeLimits {
                max_input_coordinates: 1,
                ..PolygonizeLimits::unlimited()
            },
        },
    );
    require(
        matches!(
            coordinate_limited,
            Err(PolygonizeError::CoordinateLimit { .. })
        ),
        "polygonize non fail-closed sul limite input",
    )?;
    let work_limited = polygonize_linework_rust(
        &grid,
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            precision: PRECISIONE,
            limits: PolygonizeLimits {
                max_noding_work: 10,
                ..PolygonizeLimits::unlimited()
            },
        },
    );
    require(
        matches!(work_limited, Err(PolygonizeError::WorkLimit { .. })),
        "polygonize non fail-closed sul lavoro",
    )?;
    for limits in [
        PolygonizeLimits {
            max_output_geometries: 1,
            ..PolygonizeLimits::unlimited()
        },
        PolygonizeLimits {
            max_output_coordinates: 4,
            ..PolygonizeLimits::unlimited()
        },
    ] {
        let limited = polygonize_linework_rust(
            &grid,
            PolygonizeOptions {
                node_input: true,
                require_complete: false,
                precision: PRECISIONE,
                limits,
            },
        );
        require(
            matches!(limited, Err(PolygonizeError::OutputLimit { .. })),
            "polygonize non fail-closed sul limite output",
        )?;
    }

    let incomplete_polygonize = polygonize_linework_rust_bounded(
        &grid,
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            precision: PRECISIONE,
            limits: PolygonizeLimits {
                max_input_coordinates: 36,
                ..PolygonizeLimits::unlimited()
            },
        },
    );
    require(
        matches!(
            incomplete_polygonize,
            Err(PolygonizeError::UnboundedLimitConfiguration)
        ),
        "polygonize bounded accetta un profilo incompleto",
    )?;
    let bounded_polygonize = polygonize_linework_rust_bounded(
        &grid,
        PolygonizeOptions {
            node_input: true,
            require_complete: false,
            precision: PRECISIONE,
            limits: PolygonizeLimits {
                max_input_coordinates: 36,
                max_noding_work: 1_000_000,
                max_output_geometries: 64,
                max_output_coordinates: 320,
            },
        },
    )
    .map_err(|error| AssuranceError(format!("polygonize bounded: {error}")))?;
    require(
        bounded_polygonize.polygons.len() == 64,
        "polygonize bounded non produce le 64 facce attese",
    )?;

    let source = rectangle(20.0, 10.0);
    let splitter = vertical_splitters(20, 10, 10);
    for limits in [
        SplitLimits {
            max_input_coordinates: 1,
            ..unlimited_split_limits()
        },
        SplitLimits {
            max_noding_work: 1,
            ..unlimited_split_limits()
        },
        SplitLimits {
            max_output_parts: 1,
            ..unlimited_split_limits()
        },
        SplitLimits {
            max_output_coordinates: 4,
            ..unlimited_split_limits()
        },
    ] {
        let limited = split_polygon_by_linework_rust(&source, &splitter, limits, PRECISIONE);
        require(
            matches!(
                limited,
                Err(SplitError::CoordinateLimit { .. }
                    | SplitError::WorkLimit { .. }
                    | SplitError::OutputLimit { .. })
            ),
            "split non fail-closed su un limite",
        )?;
    }

    let incomplete_split = split_polygon_by_linework_rust_bounded(
        &source,
        &splitter,
        SplitLimits {
            max_input_coordinates: 25,
            ..SplitLimits::unlimited()
        },
        PRECISIONE,
    );
    require(
        matches!(
            incomplete_split,
            Err(SplitError::UnboundedLimitConfiguration)
        ),
        "split bounded accetta un profilo incompleto",
    )?;
    let bounded_split = split_polygon_by_linework_rust_bounded(
        &source,
        &splitter,
        SplitLimits {
            max_input_coordinates: 25,
            max_noding_work: 1_000_000,
            max_output_parts: 32,
            max_output_coordinates: 160,
        },
        PRECISIONE,
    )
    .map_err(|error| AssuranceError(format!("split bounded: {error}")))?;
    require(
        bounded_split.len() == 10,
        "split bounded non produce le 10 parti attese",
    )?;

    let bow_tie = Geometry::Polygon(polygon![
        (x: 0.0, y: 0.0), (x: 2.0, y: 2.0),
        (x: 0.0, y: 2.0), (x: 2.0, y: 0.0), (x: 0.0, y: 0.0)
    ]);
    for limits in [
        MakeValidLimits {
            max_input_coordinates: 1,
            ..unlimited_make_valid_limits()
        },
        MakeValidLimits {
            max_noding_work: 1,
            ..unlimited_make_valid_limits()
        },
        MakeValidLimits {
            max_output_geometries: 1,
            ..unlimited_make_valid_limits()
        },
        MakeValidLimits {
            max_output_coordinates: 4,
            ..unlimited_make_valid_limits()
        },
    ] {
        let limited = make_valid_geometry_rust_with_limits(
            &bow_tie,
            RepairMethod::Linework,
            true,
            limits,
            PRECISIONE,
        );
        require(
            matches!(
                limited,
                Err(MakeValidError::CoordinateLimit { .. }
                    | MakeValidError::WorkLimit { .. }
                    | MakeValidError::OutputLimit { .. })
            ),
            "make_valid non fail-closed su un limite",
        )?;
    }

    let incomplete_make_valid = make_valid_geometry_rust_bounded(
        &bow_tie,
        RepairMethod::Linework,
        true,
        MakeValidLimits {
            max_input_coordinates: 5,
            ..MakeValidLimits::unlimited()
        },
        PRECISIONE,
    );
    require(
        matches!(
            incomplete_make_valid,
            Err(MakeValidError::UnboundedLimitConfiguration)
        ),
        "make_valid bounded accetta un profilo incompleto",
    )?;
    let bounded_make_valid = make_valid_geometry_rust_bounded(
        &bow_tie,
        RepairMethod::Linework,
        true,
        MakeValidLimits {
            max_input_coordinates: 5,
            max_noding_work: 1_000,
            max_output_geometries: 2,
            max_output_coordinates: 10,
        },
        PRECISIONE,
    )
    .map_err(|error| AssuranceError(format!("make_valid bounded: {error}")))?;
    require(
        (geometry_area(&bounded_make_valid)? - 2.0).abs() <= 1e-12,
        "make_valid bounded non conserva l'area attesa",
    )?;
    Ok(18)
}

/// Controlli per categoria, come nel CSV del laboratorio
/// (`results/geo-rust/assurance/summary.csv`).
const EXPECTED: [(&str, usize); 6] = [
    ("independent_integer_oracles", 14),
    ("metamorphic_affine_and_order", 28),
    ("generated_exact_oracles", 1_024),
    ("adversarial_numeric", 6),
    ("wkb_transport_corpus", 7),
    ("fail_closed_limits", 18),
];

fn expected(category: &str) -> usize {
    EXPECTED
        .iter()
        .find(|(name, _)| *name == category)
        .map_or(0, |(_, checks)| *checks)
}

#[test]
fn il_totale_dei_controlli_e_quello_del_laboratorio() {
    assert_eq!(
        EXPECTED.iter().map(|(_, checks)| checks).sum::<usize>(),
        1_097
    );
}

#[test]
fn independent_integer_oracles() -> AssuranceResult<()> {
    let checks = run_independent_oracles()?;
    require(
        checks == expected("independent_integer_oracles"),
        "conteggio",
    )
}

#[test]
fn metamorphic_affine_and_order() -> AssuranceResult<()> {
    let checks = run_metamorphic()?;
    require(
        checks == expected("metamorphic_affine_and_order"),
        "conteggio",
    )
}

#[test]
fn generated_exact_oracles() -> AssuranceResult<()> {
    let checks = run_generated_exact_oracles()?;
    require(checks == expected("generated_exact_oracles"), "conteggio")
}

#[test]
fn adversarial_numeric() -> AssuranceResult<()> {
    let checks = run_adversarial_numeric()?;
    require(checks == expected("adversarial_numeric"), "conteggio")
}

#[test]
fn wkb_transport_corpus() -> AssuranceResult<()> {
    let checks = run_wkb_corpus()?;
    require(checks == expected("wkb_transport_corpus"), "conteggio")
}

#[test]
fn fail_closed_limits() -> AssuranceResult<()> {
    let checks = run_limit_contracts()?;
    require(checks == expected("fail_closed_limits"), "conteggio")
}
