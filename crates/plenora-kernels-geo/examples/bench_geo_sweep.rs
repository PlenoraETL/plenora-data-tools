//! Sweep prestazionale di TUTTI i kernel geografici `geo.*` del catalogo
//! (75 op: 33 Manipola-compat, 11 predicati DE-9IM, 21 estensioni storiche,
//! 10 estensioni v1.1-v1.3), su fixture realistiche deterministiche (seed
//! logico 42 via xorshift, stesso schema di `bench_sweep` tabellare).
//!
//! Pipeline misurata per le op per-cella: decode WKB (`decode_geometry_cell`)
//! + kernel + encode WKB (`encode_geometry`) quando l'output e' una
//! geometria — lo stesso percorso dell'adapter Arrow del trasporto. Per le
//! op blocking/collettive (join, dissolve, overlay, coverage, dbscan, ...)
//! la misura include il decode dell'intera tabella e l'encode degli output.
//! I riferimenti `_ref.wkb_decode.*` / `_ref.wkb_encode.*` misurano il solo
//! adapter e permettono di classificare ogni op come decode-bound o
//! compute-bound (campo `bound_class`).
//!
//! Scala: 1M celle dove una run singola resta entro `TARGET_REP_SECONDS`
//! (calibrazione su 20k celle), altrimenti 100k/10k/1k con nota. Mediana di
//! 3 run. Peak RSS da `VmHWM` di `/proc/self/status` (cumulativo di
//! processo; `rss_delta_kib` e' il delta rispetto alla misura precedente).
//!
//! Op BackendPending (feature `geos`/`proj` non abilitate di default):
//! `geo.make_valid`, `geo.reproject`, `geo.polygonize`, `geo.split` sono
//! riportate come `skipped`.
//!
//! Uso: `bench_geo_sweep` — scrive `benchmarks/sweep/geo_sweep.json` e
//! `benchmarks/sweep/geo_sweep.md` (relativi alla cwd, /work in Docker) e
//! stampa le stesse righe JSON su stdout. Ogni misura e' anche accodata in
//! streaming a `benchmarks/sweep/geo_sweep.jsonl` (sopravvive a uno stallo
//! del container prima della scrittura finale).
//!
//! Note operative:
//! - kernel WSL2 6.18: sotto carico di allocazioni intensive il processo puo'
//!   stallare in stato D su `brk`/`__vma_start_write`; mitigato con
//!   `MALLOC_ARENA_MAX=4 MALLOC_MMAP_THRESHOLD_=32768` (vedi geo_sweep.md);
//! - `GEO_SWEEP_SKIP_PREFIX=1`: salta gli scenari fino a `geo.collect`
//!   escluso (gia' misurati) e appende al JSONL esistente (resume manuale).

#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::missing_panics_doc)]

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use geo::{
    Coord, Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};
use plenora_kernels_geo::advanced::voronoi_cells;
use plenora_kernels_geo::analysis::{count_points_in_polygons, nearest_matches, within_indexes};
use plenora_kernels_geo::arrow_adapter::{decode_geometry_cell, encode_geometry};
use plenora_kernels_geo::cluster::dbscan_nullable;
use plenora_kernels_geo::construction::{
    geometry_from_wkt, line_from_ordered_points, point_from_lon_lat, polygon_from_ordered_points,
};
use plenora_kernels_geo::extended::{
    affine_transform, concave_hull, geodesic_distance_m, geodesic_line_length_m,
    hausdorff_distance, haversine_distance_m, rotate_about, scale_about, translate,
};
use plenora_kernels_geo::extended_algorithms::{
    delaunay, densify, frechet_distance, geodesic_area_m2, geodesic_bearing_degrees,
    geometry_diagnostics, line_interpolate_point, line_merge, line_substring, snap_to_grid,
};
use plenora_kernels_geo::extensions::{
    collect_geometries, geometry_accessors, line_locate_point,
};
use plenora_kernels_geo::extensions2::{
    generate_grid_rows, snap, subdivide_wkb, GridExtent, GridShape,
};
use plenora_kernels_geo::extensions3::{coverage_validate_nullable, shared_paths_nullable};
use plenora_kernels_geo::operations::{
    area, boundary, bounds, buffer_with_cap, distance, explode, length, perimeter,
    point_on_surface, simplify_with_policy, to_wkt, vertex_count, BufferCapStyle, SimplifyPolicy,
};
use plenora_kernels_geo::predicates::{evaluate as evaluate_predicate, SpatialPredicate};
use plenora_kernels_geo::spatial_join::{spatial_join_nullable, JoinPredicate};
use plenora_kernels_geo::topology::{
    boolean_operation, clean_valid_polygon_topology, clip_to_mask, dissolve, polygon_overlay,
    BooleanOperation, OverlayMode,
};
use plenora_kernels_geo::{transform_wkb, Operation};
use rayon::prelude::*;
use serde_json::{json, Value};

/// Budget di una singola ripetizione: oltre si riduce la scala.
const TARGET_REP_SECONDS: f64 = 4.0;
/// Celle usate per la calibrazione del costo unitario.
const CALIBRATION_CELLS: usize = 20_000;
/// Limiti di lavoro "larghi" per le op collettive (stile trasporto).
const MAX_WORK: u64 = 1_000_000_000_000;

// ---------------------------------------------------------------------------
// RNG deterministico (xorshift64*, come bench_sweep)
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn seeded() -> Self {
        Self(42)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniforme in [0, 1).
    fn unit(&mut self) -> f64 {
        // 53 bit di mantissa / 2^53.
        (self.next() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniforme in [min, max).
    fn range(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.unit()
    }
}

// ---------------------------------------------------------------------------
// Generatori di geometrie realistiche
// ---------------------------------------------------------------------------

/// Poligono semplice a stella (semplice per costruzione): raggi jitterati
/// attorno al centro, angoli ordinati.
fn star_polygon(rng: &mut Rng, cx: f64, cy: f64, radius: f64, vertices: usize) -> Polygon<f64> {
    let mut ring = Vec::with_capacity(vertices + 1);
    for index in 0..vertices {
        let angle = std::f64::consts::TAU * index as f64 / vertices as f64;
        let r = radius * rng.range(0.7, 1.3);
        ring.push(Coord {
            x: cx + r * angle.cos(),
            y: cy + r * angle.sin(),
        });
    }
    ring.push(ring[0]);
    Polygon::new(LineString::new(ring), Vec::new())
}

/// Linea random-walk con `vertices` vertici e passo medio `step`.
fn random_walk(rng: &mut Rng, x0: f64, y0: f64, step: f64, vertices: usize) -> LineString<f64> {
    let mut coords = Vec::with_capacity(vertices);
    let (mut x, mut y) = (x0, y0);
    for _ in 0..vertices {
        coords.push(Coord { x, y });
        let angle = rng.range(0.0, std::f64::consts::TAU);
        x += step * angle.cos();
        y += step * angle.sin();
    }
    LineString::new(coords)
}

fn enc_cell(geometry: &Geometry<f64>) -> Vec<u8> {
    encode_geometry(geometry).expect("encode fixture")
}

// ---------------------------------------------------------------------------
// Fixture WKB (costruite lazy, seed 42)
// ---------------------------------------------------------------------------

type WkbCells = Vec<Vec<u8>>;

fn build_fixture(count: usize, make: impl Fn(&mut Rng, usize) -> Geometry<f64>) -> WkbCells {
    let mut rng = Rng::seeded();
    let mut cells = Vec::with_capacity(count);
    for index in 0..count {
        cells.push(enc_cell(&make(&mut rng, index)));
    }
    cells
}

/// Punti uniformi in [0, 10000)^2 (EPSG:3857-like).
fn points_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(1_000_000, |rng, _| {
            Geometry::Point(Point::new(rng.range(0.0, 10_000.0), rng.range(0.0, 10_000.0)))
        })
    })
}

/// Punti densi in [0, 1000)^2 per DBSCAN (eps=10).
fn dbscan_points_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(1_000_000, |rng, _| {
            Geometry::Point(Point::new(rng.range(0.0, 1_000.0), rng.range(0.0, 1_000.0)))
        })
    })
}

/// Punti ordinati su cerchi rumorosi da 1000 punti (per line/polygon_builder:
/// anelli validi per costruzione).
fn polygroup_points_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        let mut rng = Rng::seeded();
        let mut cells = Vec::with_capacity(1_000_000);
        for group in 0..1_000_usize {
            let cx = rng.range(1_000.0, 9_000.0);
            let cy = rng.range(1_000.0, 9_000.0);
            let radius = rng.range(50.0, 200.0);
            for index in 0..1_000_usize {
                let angle = std::f64::consts::TAU * index as f64 / 1_000.0;
                let r = radius * rng.range(0.9, 1.1);
                cells.push(enc_cell(&Geometry::Point(Point::new(
                    cx + r * angle.cos(),
                    cy + r * angle.sin(),
                ))));
            }
            black_box(group);
        }
        cells
    })
}

/// Linee random-walk ~50 vertici, passo 20, in [0, 10000)^2.
fn lines_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(200_000, |rng, _| {
            let x0 = rng.range(1_000.0, 9_000.0);
            let y0 = rng.range(1_000.0, 9_000.0);
            Geometry::LineString(random_walk(rng, x0, y0, 20.0, 50))
        })
    })
}

/// Poligoni semplici ~100 vertici, raggio ~50, in [0, 10000)^2.
fn polys_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(200_000, |rng, _| {
            let cx = rng.range(500.0, 9_500.0);
            let cy = rng.range(500.0, 9_500.0);
            Geometry::Polygon(star_polygon(rng, cx, cy, 50.0, 100))
        })
    })
}

/// Poligoni complessi ~2000 vertici, raggio ~500.
fn polys_complex_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(20_000, |rng, _| {
            let cx = rng.range(1_000.0, 9_000.0);
            let cy = rng.range(1_000.0, 9_000.0);
            Geometry::Polygon(star_polygon(rng, cx, cy, 500.0, 2_000))
        })
    })
}

/// MultiPoligoni: 4 componenti da ~100 vertici.
fn multipolys_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(50_000, |rng, _| {
            let cx = rng.range(500.0, 9_500.0);
            let cy = rng.range(500.0, 9_500.0);
            let parts = (0..4)
                .map(|part| {
                    star_polygon(
                        rng,
                        cx + 150.0 * part as f64,
                        cy + 40.0 * part as f64,
                        30.0,
                        100,
                    )
                })
                .collect();
            Geometry::MultiPolygon(MultiPolygon::new(parts))
        })
    })
}

/// WKB eterogeneo: point/line/polygon/multipolygon a rotazione.
fn hetero_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        let mut rng = Rng::seeded();
        (0..200_000)
            .map(|index| {
                let geometry = match index % 4 {
                    0 => Geometry::Point(Point::new(
                        rng.range(0.0, 10_000.0),
                        rng.range(0.0, 10_000.0),
                    )),
                    1 => {
                        let x0 = rng.range(1_000.0, 9_000.0);
                        let y0 = rng.range(1_000.0, 9_000.0);
                        Geometry::LineString(random_walk(&mut rng, x0, y0, 20.0, 50))
                    }
                    2 => {
                        let cx = rng.range(500.0, 9_500.0);
                        let cy = rng.range(500.0, 9_500.0);
                        Geometry::Polygon(star_polygon(&mut rng, cx, cy, 50.0, 100))
                    }
                    _ => {
                        let cx = rng.range(500.0, 9_500.0);
                        let cy = rng.range(500.0, 9_500.0);
                        Geometry::MultiPolygon(MultiPolygon::new(vec![
                            star_polygon(&mut rng, cx, cy, 30.0, 100),
                            star_polygon(&mut rng, cx + 150.0, cy + 40.0, 30.0, 100),
                        ]))
                    }
                };
                enc_cell(&geometry)
            })
            .collect()
    })
}

/// Punti geografici (lon [-180,180), lat [-85,85)).
fn geo_points_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(1_000_000, |rng, _| {
            Geometry::Point(Point::new(rng.range(-180.0, 180.0), rng.range(-85.0, 85.0)))
        })
    })
}

/// Linee geografiche ~50 vertici (passo 0.01 gradi).
fn geo_lines_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(100_000, |rng, _| {
            let x0 = rng.range(-170.0, 170.0);
            let y0 = rng.range(-80.0, 80.0);
            Geometry::LineString(random_walk(rng, x0, y0, 0.01, 50))
        })
    })
}

/// Poligoni geografici ~100 vertici (raggio ~0.05 gradi).
fn geo_polys_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(50_000, |rng, _| {
            let cx = rng.range(-170.0, 170.0);
            let cy = rng.range(-80.0, 80.0);
            Geometry::Polygon(star_polygon(rng, cx, cy, 0.05, 100))
        })
    })
}

/// MultiPoint da 50 punti (per delaunay/concave hull).
fn multipoint50_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        build_fixture(100_000, |rng, _| {
            let cx = rng.range(500.0, 9_500.0);
            let cy = rng.range(500.0, 9_500.0);
            let points = (0..50)
                .map(|_| Point::new(cx + rng.range(-50.0, 50.0), cy + rng.range(-50.0, 50.0)))
                .collect();
            Geometry::MultiPoint(MultiPoint::new(points))
        })
    })
}

/// Coppie di poligoni semplici sovrapposti (stesso centro, offset 25).
fn pairs_wkb() -> (&'static WkbCells, &'static WkbCells) {
    static LEFT: OnceLock<WkbCells> = OnceLock::new();
    static RIGHT: OnceLock<WkbCells> = OnceLock::new();
    let left = LEFT.get_or_init(|| {
        let mut rng = Rng::seeded();
        (0..20_000)
            .map(|_| {
                let cx = rng.range(500.0, 9_500.0);
                let cy = rng.range(500.0, 9_500.0);
                enc_cell(&Geometry::Polygon(star_polygon(&mut rng, cx, cy, 50.0, 100)))
            })
            .collect()
    });
    let right = RIGHT.get_or_init(|| {
        // Stessi centri della sinistra: rigenera lo stream e trasla di 25.
        let mut rng = Rng::seeded();
        (0..20_000)
            .map(|_| {
                let cx = rng.range(500.0, 9_500.0);
                let cy = rng.range(500.0, 9_500.0);
                enc_cell(&Geometry::Polygon(star_polygon(
                    &mut rng,
                    cx + 25.0,
                    cy + 25.0,
                    50.0,
                    100,
                )))
            })
            .collect()
    });
    (left, right)
}

/// Rettangolo di griglia [i*100, (i+1)*100) x [j*100, (j+1)*100).
fn grid_rect(i: usize, j: usize, grow: f64, shift: f64) -> Geometry<f64> {
    let x0 = i as f64 * 100.0 - grow + shift;
    let y0 = j as f64 * 100.0 - grow + shift;
    let x1 = (i + 1) as f64 * 100.0 + grow + shift;
    let y1 = (j + 1) as f64 * 100.0 + grow + shift;
    Geometry::Polygon(Polygon::new(
        LineString::new(vec![
            Coord { x: x0, y: y0 },
            Coord { x: x1, y: y0 },
            Coord { x: x1, y: y1 },
            Coord { x: x0, y: y1 },
            Coord { x: x0, y: y0 },
        ]),
        Vec::new(),
    ))
}

fn grid_fixture(grow: f64, shift: f64) -> WkbCells {
    let mut cells = Vec::with_capacity(10_000);
    for j in 0..100 {
        for i in 0..100 {
            cells.push(enc_cell(&grid_rect(i, j, grow, shift)));
        }
    }
    cells
}

/// Griglia 100x100 esatta (tiling, bordi condivisi) su [0,10000)^2.
fn grid_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| grid_fixture(0.0, 0.0))
}

/// Griglia 100x100 con crescita casuale 0..5 (overlap/gap tra vicini).
fn grid_jitter_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        let mut rng = Rng::seeded();
        let mut cells = Vec::with_capacity(10_000);
        for j in 0..100 {
            for i in 0..100 {
                cells.push(enc_cell(&grid_rect(i, j, rng.range(0.0, 5.0), 0.0)));
            }
        }
        cells
    })
}

/// Griglia 100x100 shiftata di (50,50) con jitter (per overlay).
fn grid_shift_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        let mut rng = Rng::seeded();
        let mut cells = Vec::with_capacity(10_000);
        for j in 0..100 {
            for i in 0..100 {
                cells.push(enc_cell(&grid_rect(i, j, rng.range(0.0, 5.0), 50.0)));
            }
        }
        cells
    })
}

/// Poligoni stellati ~100v su griglia spaziata 100 con raggio <= 26:
/// disgiunti e non tangenti per costruzione (per geo.collect: la validazione
/// MultiPolygon respinge overlap E contatti lungo un segmento).
fn collect_polys_wkb() -> &'static WkbCells {
    static CELLS: OnceLock<WkbCells> = OnceLock::new();
    CELLS.get_or_init(|| {
        let mut rng = Rng::seeded();
        let mut cells = Vec::with_capacity(100_000);
        for index in 0..100_000_usize {
            let i = index % 317;
            let j = index / 317;
            let cx = 50.0 + i as f64 * 100.0;
            let cy = 50.0 + j as f64 * 100.0;
            cells.push(enc_cell(&Geometry::Polygon(star_polygon(
                &mut rng, cx, cy, 20.0, 100,
            ))));
        }
        cells
    })
}

/// WKT di poligoni semplici (per geo.from_wkt).
fn wkt_polys() -> &'static Vec<String> {
    static CELLS: OnceLock<Vec<String>> = OnceLock::new();
    CELLS.get_or_init(|| {
        polys_wkb()[..100_000]
            .iter()
            .map(|payload| {
                let geometry = decode_geometry_cell(payload).expect("decode fixture");
                to_wkt(&geometry).expect("wkt fixture")
            })
            .collect()
    })
}

/// Coppie (x, y) per geo.from_coords.
fn coords_xy() -> &'static Vec<(f64, f64)> {
    static CELLS: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    CELLS.get_or_init(|| {
        let mut rng = Rng::seeded();
        (0..1_000_000)
            .map(|_| (rng.range(0.0, 10_000.0), rng.range(0.0, 10_000.0)))
            .collect()
    })
}

/// LineString decodificate (per line_merge: catene di 10 segmenti).
fn chain_lines() -> &'static Vec<LineString<f64>> {
    static LINES: OnceLock<Vec<LineString<f64>>> = OnceLock::new();
    LINES.get_or_init(|| {
        let mut rng = Rng::seeded();
        let mut lines = Vec::with_capacity(100_000);
        for _ in 0..10_000 {
            let mut x = rng.range(1_000.0, 9_000.0);
            let mut y = rng.range(1_000.0, 9_000.0);
            for _ in 0..10 {
                let nx = x + rng.range(-20.0, 20.0);
                let ny = y + rng.range(-20.0, 20.0);
                lines.push(LineString::new(vec![
                    Coord { x, y },
                    Coord { x: nx, y: ny },
                ]));
                x = nx;
                y = ny;
            }
        }
        lines
    })
}

// ---------------------------------------------------------------------------
// Harness di misura
// ---------------------------------------------------------------------------

fn peak_rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

/// Tempi unitari (s/cella) dei riferimenti decode/encode per fixture,
/// riempiti dagli scenari `_ref.*` prima delle op vere e propri.
struct RefTimes {
    decode: Option<f64>,
    encode: Option<f64>,
}

static REF_TIMES: Mutex<Option<HashMap<&'static str, RefTimes>>> = Mutex::new(None);

fn ref_times_insert(fixture: &'static str, decode: Option<f64>, encode: Option<f64>) {
    let mut guard = REF_TIMES.lock().expect("ref times");
    guard
        .get_or_insert_with(HashMap::new)
        .entry(fixture)
        .and_modify(|entry| {
            if let Some(value) = decode {
                entry.decode = Some(value);
            }
            if let Some(value) = encode {
                entry.encode = Some(value);
            }
        })
        .or_insert(RefTimes { decode, encode });
}

fn ref_times_get(fixture: &str) -> Option<(Option<f64>, Option<f64>)> {
    let guard = REF_TIMES.lock().expect("ref times");
    guard
        .as_ref()?
        .get(fixture)
        .map(|entry| (entry.decode, entry.encode))
}

struct Measurement {
    op: &'static str,
    kind: &'static str,
    fixture: &'static str,
    cells: usize,
    repetitions: usize,
    median_seconds: f64,
    geoms_per_second: f64,
    output_units: usize,
    /// Quota del tempo per-cella attribuibile a decode (+ encode) WKB.
    decode_share: Option<f64>,
    bound_class: &'static str,
    peak_rss_kib: Option<u64>,
    rss_delta_kib: Option<i64>,
    note: String,
}

fn classify(share: Option<f64>) -> &'static str {
    match share {
        None => "n/a",
        Some(value) if value >= 0.5 => "decode-bound",
        Some(value) if value >= 0.25 => "mixed",
        Some(_) => "compute-bound",
    }
}

/// Quota decode(+encode) per un'op per-cella su una fixture con riferimento.
fn decode_share(fixture: &str, geom_output: bool, per_cell_seconds: f64) -> Option<f64> {
    if per_cell_seconds <= 0.0 {
        return None;
    }
    let (decode, encode) = ref_times_get(fixture)?;
    let mut adapter = decode?;
    if geom_output {
        adapter += encode?;
    }
    Some(adapter / per_cell_seconds)
}

#[allow(clippy::too_many_arguments)]
fn record(
    results: &mut Vec<Measurement>,
    op: &'static str,
    kind: &'static str,
    fixture: &'static str,
    cells: usize,
    repetitions: usize,
    median_seconds: f64,
    output_units: usize,
    share: Option<f64>,
    rss_before: Option<u64>,
    note: String,
) {
    let geoms_per_second = if median_seconds > 0.0 {
        cells as f64 / median_seconds
    } else {
        0.0
    };
    let peak = peak_rss_kib();
    let entry = json!({
        "op": op,
        "kind": kind,
        "fixture": fixture,
        "cells": cells,
        "repetitions": repetitions,
        "median_seconds": median_seconds,
        "geoms_per_second": geoms_per_second,
        "output_units": output_units,
        "decode_share": share,
        "bound_class": classify(share),
        "peak_rss_kib": peak,
        "note": note,
    });
    println!("{}", serde_json::to_string(&entry).expect("JSON"));
    // Streaming: ogni misura e' anche accodata a geo_sweep.jsonl (sopravvive
    // a un eventuale stallo del container prima della scrittura finale).
    let directory = std::path::Path::new("benchmarks/sweep");
    std::fs::create_dir_all(directory).expect("mkdir benchmarks/sweep");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("geo_sweep.jsonl"))
    {
        use std::io::Write as _;
        let _ = writeln!(file, "{}", serde_json::to_string(&entry).expect("JSON"));
    }
    results.push(Measurement {
        op,
        kind,
        fixture,
        cells,
        repetitions,
        median_seconds,
        geoms_per_second,
        output_units,
        decode_share: share,
        bound_class: classify(share),
        peak_rss_kib: peak,
        rss_delta_kib: match (peak, rss_before) {
            (Some(after), Some(before)) => Some(after as i64 - before as i64),
            _ => None,
        },
        note,
    });
}

/// Misura un'op per-cella (parallela rayon, come `map_nullable`): calibra su
/// `CALIBRATION_CELLS`, sceglie la scala (1M/100k/10k/1k, limitata alla
/// fixture) entro `TARGET_REP_SECONDS` e prende la mediana di 3 run.
fn sweep_cells<T: Sync>(
    results: &mut Vec<Measurement>,
    op: &'static str,
    kind: &'static str,
    fixture: &'static str,
    cells: &[T],
    geom_output: bool,
    is_adapter_ref: bool,
    extra_note: &str,
    f: &(dyn Fn(&T) -> Result<usize, String> + Sync),
) {
    let rss_before = peak_rss_kib();
    let len = cells.len();
    let calibration = len.min(CALIBRATION_CELLS);
    let start = Instant::now();
    let digest = cells[..calibration]
        .par_iter()
        .map(f)
        .collect::<Result<Vec<_>, _>>()
        .expect(op);
    black_box(&digest);
    let per_cell = start.elapsed().as_secs_f64() / calibration as f64;

    let chosen = [1_000_000_usize, 100_000, 10_000, 1_000]
        .into_iter()
        .filter(|&n| n <= len)
        .find(|&n| per_cell * n as f64 <= TARGET_REP_SECONDS)
        .unwrap_or_else(|| len.min(1_000));
    let mut note = extra_note.to_owned();
    if chosen < 1_000_000 {
        note.push_str(&format!(
            "; scala ridotta a {chosen} (stima > {TARGET_REP_SECONDS}s/rep a 1M o fixture limitata)"
        ));
    }

    let mut durations = Vec::with_capacity(3);
    let mut output_units = 0_usize;
    for _ in 0..3 {
        let start = Instant::now();
        let digest = cells[..chosen]
            .par_iter()
            .map(f)
            .collect::<Result<Vec<_>, _>>()
            .expect(op);
        durations.push(start.elapsed().as_secs_f64());
        output_units = digest.iter().sum();
        black_box(digest);
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    let per_cell_median = median / chosen as f64;
    if is_adapter_ref {
        if kind == "ref_decode" {
            ref_times_insert(fixture, Some(per_cell_median), None);
        } else {
            ref_times_insert(fixture, None, Some(per_cell_median));
        }
    }
    let share = if is_adapter_ref {
        None
    } else {
        decode_share(fixture, geom_output, per_cell_median)
    };
    record(
        results,
        op,
        kind,
        fixture,
        chosen,
        3,
        median,
        output_units,
        share,
        rss_before,
        note,
    );
}

/// Misura un'op collettiva/pairwise: `f(n)` esegue la pipeline completa su
/// `n` geometrie (decode incluso). Calibrazione sulla scala minima con
/// estrapolazione `exponent` (2 per op quadratiche tipo nearest).
fn sweep_collective(
    results: &mut Vec<Measurement>,
    op: &'static str,
    fixture: &'static str,
    sizes: &[usize],
    exponent: f64,
    extra_note: &str,
    f: &(dyn Fn(usize) -> Result<usize, String> + Sync),
) {
    let rss_before = peak_rss_kib();
    let base = sizes[0];
    let start = Instant::now();
    let digest = f(base).expect(op);
    black_box(digest);
    let base_seconds = start.elapsed().as_secs_f64();

    let chosen = sizes
        .iter()
        .copied()
        .filter(|&n| base_seconds * (n as f64 / base as f64).powf(exponent) <= TARGET_REP_SECONDS)
        .last()
        .unwrap_or(base);
    let mut note = extra_note.to_owned();
    if chosen < *sizes.last().unwrap_or(&base) {
        note.push_str(&format!(
            "; scala ridotta a {chosen} (stima > {TARGET_REP_SECONDS}s/rep a scala piena, esponente {exponent})"
        ));
    }

    let mut durations = Vec::with_capacity(3);
    let mut output_units = 0_usize;
    for _ in 0..3 {
        let start = Instant::now();
        let digest = f(chosen).expect(op);
        durations.push(start.elapsed().as_secs_f64());
        output_units = digest;
        black_box(digest);
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    record(
        results,
        op,
        "collective",
        fixture,
        chosen,
        3,
        median,
        output_units,
        None,
        rss_before,
        note,
    );
}

/// Scenario saltato (backend geos/proj non abilitato).
fn sweep_skipped(results: &mut Vec<Measurement>, op: &'static str, note: &str) {
    record(
        results,
        op,
        "skipped",
        "-",
        0,
        0,
        0.0,
        0,
        None,
        peak_rss_kib(),
        note.to_owned(),
    );
}

// ---------------------------------------------------------------------------
// Helper per le closure per-cella
// ---------------------------------------------------------------------------

fn dec(payload: &[u8]) -> Result<Geometry<f64>, String> {
    decode_geometry_cell(payload).map_err(|error| error.to_string())
}

fn enc(geometry: &Geometry<f64>) -> Result<usize, String> {
    encode_geometry(geometry)
        .map(|wkb| wkb.len())
        .map_err(|error| error.to_string())
}

fn enc_opt(geometry: &Option<Geometry<f64>>) -> Result<usize, String> {
    geometry.as_ref().map_or(Ok(0), enc)
}

fn enc_many(geometries: &[Geometry<f64>]) -> Result<usize, String> {
    geometries.iter().try_fold(0_usize, |total, geometry| {
        enc(geometry).map(|len| total + len)
    })
}

fn as_line(geometry: &Geometry<f64>) -> Result<&LineString<f64>, String> {
    match geometry {
        Geometry::LineString(line) => Ok(line),
        _ => Err("attesa LineString".to_owned()),
    }
}

fn as_point(geometry: &Geometry<f64>) -> Result<Point<f64>, String> {
    match geometry {
        Geometry::Point(point) => Ok(*point),
        _ => Err("atteso Point".to_owned()),
    }
}

/// Decode parallelo di `n` celle WKB (parte della pipeline collettiva).
fn decode_prefix(cells: &[Vec<u8>], n: usize) -> Result<Vec<Geometry<f64>>, String> {
    cells[..n].par_iter().map(|payload| dec(payload)).collect()
}

fn as_options(geometries: Vec<Geometry<f64>>) -> Vec<Option<Geometry<f64>>> {
    geometries.into_iter().map(Some).collect()
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn write_outputs(results: &[Measurement]) {
    let directory = std::path::Path::new("benchmarks/sweep");
    std::fs::create_dir_all(directory).expect("mkdir benchmarks/sweep");

    let json_records: Vec<Value> = results
        .iter()
        .map(|entry| {
            json!({
                "op": entry.op,
                "kind": entry.kind,
                "fixture": entry.fixture,
                "cells": entry.cells,
                "repetitions": entry.repetitions,
                "median_seconds": entry.median_seconds,
                "geoms_per_second": entry.geoms_per_second,
                "output_units": entry.output_units,
                "decode_share": entry.decode_share,
                "bound_class": entry.bound_class,
                "peak_rss_kib": entry.peak_rss_kib,
                "rss_delta_kib": entry.rss_delta_kib,
                "note": entry.note,
            })
        })
        .collect();
    let cpus = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let document = json!({
        "benchmark": "bench_geo_sweep",
        "seed": 42,
        "container": "docker run --cpus=4 --memory=10g rust:1.92 (cargo run --release)",
        "logical_cpus": cpus,
        "target_rep_seconds": TARGET_REP_SECONDS,
        "calibration_cells": CALIBRATION_CELLS,
        "results": json_records,
    });
    std::fs::write(
        directory.join("geo_sweep.json"),
        serde_json::to_string_pretty(&document).expect("JSON"),
    )
    .expect("write geo_sweep.json");

    let mut sorted: Vec<&Measurement> = results.iter().collect();
    sorted.sort_by(|left, right| {
        let left_skip = left.kind == "skipped";
        let right_skip = right.kind == "skipped";
        left_skip
            .cmp(&right_skip)
            .then(left.geoms_per_second.total_cmp(&right.geoms_per_second))
    });
    let mut markdown = String::new();
    markdown.push_str(
        "# Sweep kernel geografici (bench_geo_sweep)\n\n\
         Fixture deterministiche (seed 42), mediana di 3 run, container Docker\n\
         `--cpus=4 --memory=10g`, release. Le op per-cella misurano la pipeline\n\
         decode WKB + kernel + encode WKB (come l'adapter Arrow); quelle\n\
         collettive includono il decode dell'intera tabella. `decode_share` e'\n\
         la quota del tempo per-cella attribuita a decode(+encode) WKB dai\n\
         riferimenti `_ref.*`; `bound_class` deriva da soglie 0.5/0.25.\n\
         Classifica ordinata per lentezza (geom/s crescenti, skipped in coda).\n\
         Il peak RSS e' il `VmHWM` cumulativo di processo (le fixture lazy\n\
         restano in memoria); `delta RSS` e' l'incremento rispetto alla misura\n\
         precedente.\n\n\
         | # | op | tipo | fixture | celle | mediana (s) | geom/s | decode share | classe | peak RSS (MiB) | delta RSS (MiB) | note |\n\
         |---|----|------|---------|-------|-------------|--------|--------------|--------|----------------|-----------------|------|\n",
    );
    for (position, entry) in sorted.iter().enumerate() {
        markdown.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {:.4} | {:.0} | {} | {} | {} | {} | {} |\n",
            position + 1,
            entry.op,
            entry.kind,
            entry.fixture,
            entry.cells,
            entry.median_seconds,
            entry.geoms_per_second,
            entry
                .decode_share
                .map(|share| format!("{share:.2}"))
                .unwrap_or_else(|| "-".into()),
            entry.bound_class,
            entry
                .peak_rss_kib
                .map(|kib| format!("{}", kib / 1024))
                .unwrap_or_else(|| "n/d".into()),
            entry
                .rss_delta_kib
                .map(|kib| format!("{}", kib / 1024))
                .unwrap_or_else(|| "-".into()),
            entry.note,
        ));
    }
    std::fs::write(directory.join("geo_sweep.md"), markdown).expect("write geo_sweep.md");
}

// ---------------------------------------------------------------------------
// Scenari
// ---------------------------------------------------------------------------

fn main() {
    let mut results: Vec<Measurement> = Vec::new();
    let directory = std::path::Path::new("benchmarks/sweep");
    std::fs::create_dir_all(directory).expect("mkdir benchmarks/sweep");
    // GEO_SWEEP_SKIP_PREFIX=1: salta gli scenari fino a geo.collect escluso
    // (gia' misurati) e appende al JSONL esistente; usato per riprendere una
    // run interrotta senza ripetere la parte per-cella.
    let skip_prefix = std::env::var_os("GEO_SWEEP_SKIP_PREFIX").is_some();
    if !skip_prefix {
        let _ = std::fs::remove_file(directory.join("geo_sweep.jsonl"));
    }
    if !skip_prefix {

    // --- Riferimenti adapter (decode/encode puri) ---------------------------
    let decode_op = |payload: &Vec<u8>| -> Result<usize, String> {
        dec(payload).map(|geometry| black_box(geometry).coords_count())
    };
    use geo::CoordsIter;
    sweep_cells(&mut results, "_ref.wkb_decode.points", "ref_decode", "points", points_wkb(), false, true, "solo decode WKB", &decode_op);
    sweep_cells(&mut results, "_ref.wkb_decode.lines50", "ref_decode", "lines50", lines_wkb(), false, true, "solo decode WKB", &decode_op);
    sweep_cells(&mut results, "_ref.wkb_decode.poly_simple", "ref_decode", "poly_simple", polys_wkb(), false, true, "solo decode WKB", &decode_op);
    sweep_cells(&mut results, "_ref.wkb_decode.poly_complex", "ref_decode", "poly_complex", polys_complex_wkb(), false, true, "solo decode WKB", &decode_op);
    sweep_cells(&mut results, "_ref.wkb_decode.multipoly", "ref_decode", "multipoly", multipolys_wkb(), false, true, "solo decode WKB", &decode_op);
    sweep_cells(&mut results, "_ref.wkb_decode.hetero", "ref_decode", "hetero", hetero_wkb(), false, true, "solo decode WKB eterogeneo", &decode_op);
    sweep_cells(&mut results, "_ref.wkb_decode.geo_points", "ref_decode", "geo_points", geo_points_wkb(), false, true, "solo decode WKB", &decode_op);

    let encode_op = |payload: &Vec<u8>| -> Result<usize, String> {
        dec(payload).and_then(|geometry| enc(&geometry))
    };
    let encode_fixtures: [(&str, &str, &WkbCells); 5] = [
        ("points", "_ref.wkb_encode.points", points_wkb()),
        ("lines50", "_ref.wkb_encode.lines50", lines_wkb()),
        ("poly_simple", "_ref.wkb_encode.poly_simple", polys_wkb()),
        ("poly_complex", "_ref.wkb_encode.poly_complex", polys_complex_wkb()),
        ("multipoly", "_ref.wkb_encode.multipoly", multipolys_wkb()),
    ];
    for &(fixture, op_name, cells) in &encode_fixtures {
        sweep_cells(&mut results, op_name, "ref_encode", fixture, cells, true, true, "decode + encode WKB (quota encode = misura - ref decode)", &encode_op);
    }
    // Registra encode = (decode+encode) - decode per le fixture di riferimento.
    for &(fixture, op_name, _) in &encode_fixtures {
        if let Some(entry) = results.iter().find(|entry| entry.op == op_name) {
            let combined = entry.median_seconds / entry.cells as f64;
            if let Some((Some(decode), _)) = ref_times_get(fixture) {
                ref_times_insert(fixture, None, Some((combined - decode).max(0.0)));
            }
        }
    }

    // --- Riferimenti geometrici decodificati una volta (config) -------------
    let ref_poly = dec(&polys_wkb()[0]).expect("ref polygon");
    let ref_poly = &ref_poly;
    let ref_line = as_line(&dec(&lines_wkb()[0]).expect("ref line"))
        .expect("line")
        .clone();
    let ref_line_geometry = Geometry::LineString(ref_line.clone());
    let ref_complex = dec(&polys_complex_wkb()[0]).expect("ref complex");
    let ref_geo_point = as_point(&dec(&geo_points_wkb()[1]).expect("ref geo point")).expect("pt");

    // --- Manipola-compat per-cella ------------------------------------------
    let op_centroid = |payload: &Vec<u8>| transform_wkb(Operation::Centroid, payload).map(|w| w.len()).map_err(|e| e.to_string());
    sweep_cells(&mut results, "geo.centroid", "per_cell", "poly_simple", polys_wkb(), true, false, "", &op_centroid);
    let op_convex = |payload: &Vec<u8>| transform_wkb(Operation::ConvexHull, payload).map(|w| w.len()).map_err(|e| e.to_string());
    sweep_cells(&mut results, "geo.convex_hull", "per_cell", "poly_simple", polys_wkb(), true, false, "", &op_convex);
    let op_envelope = |payload: &Vec<u8>| transform_wkb(Operation::Envelope, payload).map(|w| w.len()).map_err(|e| e.to_string());
    sweep_cells(&mut results, "geo.envelope", "per_cell", "poly_simple", polys_wkb(), true, false, "", &op_envelope);
    let op_area = |payload: &Vec<u8>| dec(payload).and_then(|g| area(&g).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.area", "per_cell", "poly_simple", polys_wkb(), false, false, "", &op_area);
    let op_boundary = |payload: &Vec<u8>| dec(payload).and_then(|g| boundary(&g).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.boundary", "per_cell", "poly_simple", polys_wkb(), true, false, "", &op_boundary);
    let op_bounds = |payload: &Vec<u8>| dec(payload).and_then(|g| bounds(&g).map(|v| black_box(v).is_some() as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.bounds_extractor", "per_cell", "poly_simple", polys_wkb(), false, false, "", &op_bounds);
    let op_buffer = |payload: &Vec<u8>| dec(payload).and_then(|g| buffer_with_cap(&g, 10.0, BufferCapStyle::Round).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.buffer", "per_cell", "poly_simple", polys_wkb(), true, false, "d=10, cap round", &op_buffer);
    let op_distance = |payload: &Vec<u8>| dec(payload).and_then(|g| distance(&g, ref_poly).map(|v| black_box(v).unwrap_or(0.0) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.distance", "per_cell", "poly_simple", polys_wkb(), false, false, "vs poligono di riferimento", &op_distance);
    let op_explode = |payload: &Vec<u8>| dec(payload).and_then(|g| explode(&g).map_err(|e| e.to_string()).and_then(|parts| enc_many(&parts)));
    sweep_cells(&mut results, "geo.explode", "per_cell", "multipoly", multipolys_wkb(), true, false, "1:N, encode di ogni parte", &op_explode);
    let op_from_coords = |xy: &(f64, f64)| point_from_lon_lat(xy.0, xy.1).map_err(|e| e.to_string()).and_then(|g| enc(&g));
    sweep_cells(&mut results, "geo.from_coords", "per_cell", "coords_xy", coords_xy(), true, false, "da colonne x/y (niente decode)", &op_from_coords);
    let op_length = |payload: &Vec<u8>| dec(payload).and_then(|g| length(&g).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.length", "per_cell", "lines50", lines_wkb(), false, false, "", &op_length);
    let op_perimeter = |payload: &Vec<u8>| dec(payload).and_then(|g| perimeter(&g).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.perimeter", "per_cell", "poly_simple", polys_wkb(), false, false, "", &op_perimeter);
    let op_pos = |payload: &Vec<u8>| dec(payload).and_then(|g| point_on_surface(&g).map_err(|e| e.to_string()).and_then(|r| enc_opt(&r)));
    sweep_cells(&mut results, "geo.point_on_surface", "per_cell", "poly_simple", polys_wkb(), true, false, "", &op_pos);
    let op_simplify = |payload: &Vec<u8>| dec(payload).and_then(|g| simplify_with_policy(&g, 1.0, SimplifyPolicy::DouglasPeucker).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.simplify", "per_cell", "poly_simple", polys_wkb(), true, false, "tol=1, douglas_peucker", &op_simplify);
    let op_simplify_vw = |payload: &Vec<u8>| dec(payload).and_then(|g| simplify_with_policy(&g, 1.0, SimplifyPolicy::PreserveTopology).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.simplify[preserve_topology]", "per_cell", "poly_simple", polys_wkb(), true, false, "tol=1, variante VW preserve", &op_simplify_vw);
    let op_to_wkt = |payload: &Vec<u8>| dec(payload).and_then(|g| to_wkt(&g).map(|s| s.len()).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.to_wkt", "per_cell", "poly_simple", polys_wkb(), false, false, "output WKT (niente encode WKB)", &op_to_wkt);
    let op_vcount = |payload: &Vec<u8>| dec(payload).and_then(|g| vertex_count(&g).map(|v| v as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.vertex_count", "per_cell", "poly_simple", polys_wkb(), false, false, "", &op_vcount);

    // --- Predicati DE-9IM (11), punti vs poligono di riferimento ------------
    for (name, predicate) in [
        ("geo.predicate_intersects", SpatialPredicate::Intersects),
        ("geo.predicate_disjoint", SpatialPredicate::Disjoint),
        ("geo.predicate_contains", SpatialPredicate::Contains),
        ("geo.predicate_within", SpatialPredicate::Within),
        ("geo.predicate_equals_topo", SpatialPredicate::EqualsTopo),
        ("geo.predicate_covers", SpatialPredicate::Covers),
        ("geo.predicate_covered_by", SpatialPredicate::CoveredBy),
        ("geo.predicate_contains_properly", SpatialPredicate::ContainsProperly),
        ("geo.predicate_touches", SpatialPredicate::Touches),
        ("geo.predicate_crosses", SpatialPredicate::Crosses),
        ("geo.predicate_overlaps", SpatialPredicate::Overlaps),
    ] {
        let op = move |payload: &Vec<u8>| {
            dec(payload).and_then(|g| {
                evaluate_predicate(&g, ref_poly, predicate)
                    .map(|v| black_box(v) as usize)
                    .map_err(|e| e.to_string())
            })
        };
        let leaked: &(dyn Fn(&Vec<u8>) -> Result<usize, String> + Sync) = Box::leak(Box::new(op));
        sweep_cells(&mut results, name, "per_cell", "points", points_wkb(), false, false, "punti vs poligono 100v di riferimento", leaked);
    }

    // --- Estensioni storiche per-cella --------------------------------------
    let op_affine = |payload: &Vec<u8>| dec(payload).and_then(|g| affine_transform(&g, [1.1, 0.05, 3.0, -0.02, 0.9, 7.0]).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.affine_transform", "per_cell", "poly_simple", polys_wkb(), true, false, "[1.1,0.05,3,-0.02,0.9,7]", &op_affine);
    let op_translate = |payload: &Vec<u8>| dec(payload).and_then(|g| translate(&g, 10.0, 20.0).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.translate", "per_cell", "poly_simple", polys_wkb(), true, false, "dx=10 dy=20", &op_translate);
    let op_scale = |payload: &Vec<u8>| dec(payload).and_then(|g| scale_about(&g, 1.5, 0.75, Point::new(0.0, 0.0)).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.scale", "per_cell", "poly_simple", polys_wkb(), true, false, "1.5x0.75 su origine", &op_scale);
    let op_rotate = |payload: &Vec<u8>| dec(payload).and_then(|g| rotate_about(&g, 30.0, Point::new(5_000.0, 5_000.0)).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.rotate", "per_cell", "poly_simple", polys_wkb(), true, false, "30 gradi", &op_rotate);
    let op_concave = |payload: &Vec<u8>| dec(payload).and_then(|g| concave_hull(&g, 2.0, 0.0, 10_000_000).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.concave_hull", "per_cell", "poly_simple", polys_wkb(), true, false, "concavity=2", &op_concave);
    let op_hausdorff = |payload: &Vec<u8>| dec(payload).and_then(|g| hausdorff_distance(&g, &ref_line_geometry, 1_000_000_000).map(|v| black_box(v).unwrap_or(0.0) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.hausdorff_distance", "per_cell", "lines50", lines_wkb(), false, false, "vs linea 50v di riferimento", &op_hausdorff);
    let op_haversine = |payload: &Vec<u8>| dec(payload).and_then(|g| haversine_distance_m(as_point(&g)?, ref_geo_point).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.haversine_distance", "per_cell", "geo_points", geo_points_wkb(), false, false, "vs punto di riferimento", &op_haversine);
    let op_geodesic = |payload: &Vec<u8>| dec(payload).and_then(|g| geodesic_distance_m(as_point(&g)?, ref_geo_point).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.geodesic_distance", "per_cell", "geo_points", geo_points_wkb(), false, false, "vs punto di riferimento", &op_geodesic);
    let op_geolen = |payload: &Vec<u8>| dec(payload).and_then(|g| geodesic_line_length_m(as_line(&g)?).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.geodesic_line_length", "per_cell", "geo_lines", geo_lines_wkb(), false, false, "", &op_geolen);
    let op_densify = |payload: &Vec<u8>| dec(payload).and_then(|g| densify(&g, 5.0, 100_000_000).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.densify", "per_cell", "lines50", lines_wkb(), true, false, "max_segment_length=5", &op_densify);
    let op_snapgrid = |payload: &Vec<u8>| dec(payload).and_then(|g| snap_to_grid(&g, 1.0).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.snap_to_grid", "per_cell", "poly_simple", polys_wkb(), true, false, "grid=1", &op_snapgrid);
    let op_delaunay = |payload: &Vec<u8>| {
        dec(payload).and_then(|g| {
            delaunay(&g, 1_000_000, 10_000_000)
                .map_err(|e| e.to_string())
                .and_then(|triangles| {
                    triangles
                        .iter()
                        .try_fold(0_usize, |total, triangle| {
                            enc(&Geometry::Polygon(triangle.clone())).map(|len| total + len)
                        })
                })
        })
    };
    sweep_cells(&mut results, "geo.delaunay", "per_cell", "multipoint50", multipoint50_wkb(), true, false, "multipoint 50 punti, encode triangoli", &op_delaunay);
    let op_linesub = |payload: &Vec<u8>| dec(payload).and_then(|g| line_substring(as_line(&g)?, 0.2, 0.8).map_err(|e| e.to_string()).and_then(|r| enc_opt(&r)));
    sweep_cells(&mut results, "geo.line_substring", "per_cell", "lines50", lines_wkb(), true, false, "[0.2, 0.8]", &op_linesub);
    let op_lineinterp = |payload: &Vec<u8>| {
        dec(payload).and_then(|g| {
            line_interpolate_point(as_line(&g)?, 0.5)
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_or(Ok(0), |p| enc(&Geometry::Point(p))))
        })
    };
    sweep_cells(&mut results, "geo.line_interpolate_point", "per_cell", "lines50", lines_wkb(), true, false, "ratio=0.5", &op_lineinterp);
    let op_frechet = |payload: &Vec<u8>| dec(payload).and_then(|g| frechet_distance(as_line(&g)?, &ref_line, 1_000_000_000).map(|v| black_box(v).unwrap_or(0.0) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.frechet_distance", "per_cell", "lines50", lines_wkb(), false, false, "vs linea 50v di riferimento", &op_frechet);
    let op_bearing = |payload: &Vec<u8>| dec(payload).and_then(|g| geodesic_bearing_degrees(ref_geo_point, as_point(&g)?).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.bearing", "per_cell", "geo_points", geo_points_wkb(), false, false, "vs punto di riferimento", &op_bearing);
    let op_geoarea = |payload: &Vec<u8>| dec(payload).and_then(|g| geodesic_area_m2(&g).map(|v| black_box(v) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.geodesic_area", "per_cell", "geo_polys", geo_polys_wkb(), false, false, "", &op_geoarea);
    let op_diag = |payload: &Vec<u8>| dec(payload).and_then(|g| geometry_diagnostics(&g).map(|d| black_box(d.coordinate_count) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.geometry_diagnostics", "per_cell", "poly_simple", polys_wkb(), false, false, "struct diagnostico", &op_diag);

    // --- Estensioni v1.1 -----------------------------------------------------
    let op_from_wkt = |wkt: &String| geometry_from_wkt(wkt).map_err(|e| e.to_string()).and_then(|g| enc(&g));
    sweep_cells(&mut results, "geo.from_wkt", "per_cell", "wkt_polys", wkt_polys(), true, false, "parse WKT + encode WKB (niente decode)", &op_from_wkt);
    let op_accessors = |payload: &Vec<u8>| dec(payload).and_then(|g| geometry_accessors(&g).map(|a| black_box(a.num_geometries) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.geometry_accessors", "per_cell", "hetero", hetero_wkb(), false, false, "WKB eterogeneo", &op_accessors);
    let op_locate = |payload: &Vec<u8>| dec(payload).and_then(|g| line_locate_point(&g, &Point::new(5_000.0, 5_000.0)).map(|v| (black_box(v).unwrap_or(0.0) * 1e6) as usize).map_err(|e| e.to_string()));
    sweep_cells(&mut results, "geo.line_locate_point", "per_cell", "lines50", lines_wkb(), false, false, "vs punto (5000,5000)", &op_locate);

    // --- Estensioni v1.2 -----------------------------------------------------
    let op_subdivide = |payload: &Vec<u8>| subdivide_wkb(payload, 500).map(|parts| parts.iter().map(Vec::len).sum()).map_err(|e| e.to_string());
    sweep_cells(&mut results, "geo.subdivide", "per_cell", "poly_complex", polys_complex_wkb(), true, false, "max_vertices=500 (helper WKB: decode+encode inclusi)", &op_subdivide);
    let op_snap = |payload: &Vec<u8>| dec(payload).and_then(|g| snap(&g, &ref_complex, 0.5).map_err(|e| e.to_string()).and_then(|r| enc(&r)));
    sweep_cells(&mut results, "geo.snap", "per_cell", "poly_simple", polys_wkb(), true, false, "tol=0.5, riferimento poligono 2000v (R-tree ricostruito per cella, come snap_column)", &op_snap);

    // --- geo.generate_grid (generativa): extent 1000x1000, cell_size=1 ------
    {
        let rss_before = peak_rss_kib();
        let extent = GridExtent::new(0.0, 0.0, 1_000.0, 1_000.0).expect("extent");
        let mut durations = Vec::with_capacity(3);
        let mut output_units = 0_usize;
        for _ in 0..3 {
            let start = Instant::now();
            let rows = generate_grid_rows(&extent, 1.0, GridShape::Square)
                .map_err(|e| e.to_string())
                .expect("geo.generate_grid");
            durations.push(start.elapsed().as_secs_f64());
            output_units = rows.iter().map(|row| row.wkb.len()).sum();
            black_box(rows);
        }
        durations.sort_by(f64::total_cmp);
        let median = durations[durations.len() / 2];
        record(
            &mut results,
            "geo.generate_grid",
            "generative",
            "extent 1000x1000",
            1_000_000,
            3,
            median,
            output_units,
            None,
            rss_before,
            "cell_size=1, shape square, encode WKB incluso".to_owned(),
        );
    }

    // --- Booleane pairwise (intersection/union/difference/sym_diff) ---------
    let (pairs_left, pairs_right) = pairs_wkb();
    for (name, boolean) in [
        ("geo.intersection", BooleanOperation::Intersection),
        ("geo.union", BooleanOperation::Union),
        ("geo.difference", BooleanOperation::Difference),
        ("geo.symmetric_difference", BooleanOperation::SymmetricDifference),
    ] {
        let run = move |n: usize| -> Result<usize, String> {
            let left = decode_prefix(pairs_left, n)?;
            let right = decode_prefix(pairs_right, n)?;
            left.par_iter()
                .zip(right.par_iter())
                .map(|(l, r)| {
                    boolean_operation(l, r, boolean)
                        .map_err(|e| e.to_string())
                        .and_then(|out| enc(&out))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|v| v.iter().sum())
        };
        let leaked: &(dyn Fn(usize) -> Result<usize, String> + Sync) = Box::leak(Box::new(run));
        sweep_collective(
            &mut results,
            name,
            "poly_simple pairs (overlap)",
            &[1_000, 10_000, 20_000],
            1.5,
            "coppie di poligoni 100v sovrapposti",
            leaked,
        );
    }

    // --- Collettive Manipola-compat ------------------------------------------
    sweep_collective(
        &mut results,
        "geo.sjoin",
        "points x grid100",
        &[10_000, 100_000, 1_000_000],
        1.2,
        "punti vs griglia 10k rettangoli, predicato intersects (R-tree)",
        &|n| {
            let left = as_options(decode_prefix(points_wkb(), n)?);
            let right = as_options(decode_prefix(grid_wkb(), 10_000)?);
            spatial_join_nullable(&left, &right, JoinPredicate::Intersects, MAX_WORK)
                .map(|pairs| pairs.len())
                .map_err(|e| e.to_string())
        },
    );
    sweep_collective(
        &mut results,
        "geo.within",
        "points x grid100",
        &[10_000, 100_000, 1_000_000],
        1.2,
        "punti vs griglia 10k rettangoli (within)",
        &|n| {
            let left = as_options(decode_prefix(points_wkb(), n)?);
            let right = as_options(decode_prefix(grid_wkb(), 10_000)?);
            within_indexes(&left, &right, MAX_WORK)
                .map(|v| v.len())
                .map_err(|e| e.to_string())
        },
    );
    sweep_collective(
        &mut results,
        "geo.count_points_in_polygons",
        "points x grid100",
        &[10_000, 100_000, 1_000_000],
        1.2,
        "punti vs griglia 10k rettangoli",
        &|n| {
            let polys = as_options(decode_prefix(grid_wkb(), 10_000)?);
            let points = as_options(decode_prefix(points_wkb(), n)?);
            count_points_in_polygons(&polys, &points, MAX_WORK)
                .map(|v| v.len())
                .map_err(|e| e.to_string())
        },
    );
    sweep_collective(
        &mut results,
        "geo.nearest",
        "points x points",
        &[1_000, 10_000],
        2.0,
        "O(n*m) esatto: n punti vs 10k punti, pareggi multipli inclusi",
        &|n| {
            let left = as_options(decode_prefix(points_wkb(), n)?);
            let right = as_options(decode_prefix(points_wkb(), 10_000)?);
            nearest_matches(&left, &right, None, MAX_WORK, MAX_WORK)
                .map(|v| v.len())
                .map_err(|e| e.to_string())
        },
    );
    sweep_collective(
        &mut results,
        "geo.dissolve",
        "grid100 jitter",
        &[1_000, 10_000],
        1.5,
        "unary union di rettangoli con overlap jitterati",
        &|n| {
            let geoms = decode_prefix(grid_jitter_wkb(), n)?;
            dissolve(&geoms)
                .map_err(|e| e.to_string())
                .and_then(|out| enc(&out))
        },
    );
    sweep_collective(
        &mut results,
        "geo.clip",
        "poly_simple x mask",
        &[1_000, 10_000],
        1.5,
        "poligoni 100v vs maschera (dissolve di una striscia centrale della griglia)",
        &|n| {
            let geoms = decode_prefix(polys_wkb(), n)?;
            let masks = decode_prefix(&grid_jitter_wkb()[4_500..], 100)?;
            clip_to_mask(&geoms, &masks)
                .map_err(|e| e.to_string())
                .and_then(|outs| {
                    outs.iter().try_fold(0_usize, |total, out| {
                        enc_opt(out).map(|len| total + len)
                    })
                })
        },
    );
    sweep_collective(
        &mut results,
        "geo.overlay",
        "grid100 x grid100 shift",
        &[1_000, 10_000],
        1.5,
        "mode intersection, griglie sfasate di (50,50)",
        &|n| {
            let left = decode_prefix(grid_wkb(), n)?;
            let right = decode_prefix(grid_shift_wkb(), n)?;
            polygon_overlay(&left, &right, OverlayMode::Intersection, MAX_WORK, MAX_WORK)
                .map_err(|e| e.to_string())
                .and_then(|pieces| {
                    pieces.iter().try_fold(0_usize, |total, piece| {
                        enc(&piece.geometry).map(|len| total + len)
                    })
                })
        },
    );
    sweep_collective(
        &mut results,
        "geo.clean_topology",
        "grid100 jitter",
        &[1_000, 10_000],
        1.5,
        "snap_tolerance=1, remove_overlaps+fill_gaps",
        &|n| {
            let geoms = decode_prefix(grid_jitter_wkb(), n)?;
            clean_valid_polygon_topology(&geoms, 1.0, true, true, MAX_WORK, MAX_WORK)
                .map_err(|e| e.to_string())
                .and_then(|outs| {
                    outs.iter().try_fold(0_usize, |total, out| {
                        enc_opt(out).map(|len| total + len)
                    })
                })
        },
    );
    sweep_collective(
        &mut results,
        "geo.voronoi",
        "points",
        &[10_000, 100_000],
        1.2,
        "celle Voronoi da n punti, encode di ogni cella",
        &|n| {
            let geoms = decode_prefix(points_wkb(), n)?;
            voronoi_cells(&geoms, 100_000)
                .map_err(|e| e.to_string())
                .and_then(|cells| enc_many(&cells))
        },
    );
    sweep_collective(
        &mut results,
        "geo.line_merge",
        "chain lines 10x10k",
        &[10_000, 100_000],
        1.2,
        "un MultiLineString di n segmenti in catene da 10",
        &|n| {
            let mls = Geometry::MultiLineString(MultiLineString::new(
                chain_lines()[..n].to_vec(),
            ));
            line_merge(&mls, MAX_WORK, MAX_WORK)
                .map_err(|e| e.to_string())
                .and_then(|lines| {
                    lines.iter().try_fold(0_usize, |total, line| {
                        enc(&Geometry::LineString(line.clone())).map(|len| total + len)
                    })
                })
        },
    );
    } // fine prefisso saltabile (GEO_SWEEP_SKIP_PREFIX)

    sweep_collective(
        &mut results,
        "geo.collect",
        "disjoint polys groups of 10",
        &[10_000, 100_000],
        1.0,
        "raggruppamento senza unione, gruppi di 10 poligoni disgiunti (MultiPolygon valido)",
        &|n| {
            let geoms = decode_prefix(collect_polys_wkb(), n)?;
            let options = as_options(geoms);
            options
                .par_chunks(10)
                .map(|group| {
                    collect_geometries(group)
                        .map_err(|e| e.to_string())
                        .and_then(|out| enc_opt(&out))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|v| v.iter().sum())
        },
    );
    sweep_collective(
        &mut results,
        "geo.line_builder",
        "circle points groups of 1000",
        &[100_000, 1_000_000],
        1.0,
        "gruppi ordinati di 1000 punti -> linea",
        &|n| {
            let geoms = decode_prefix(polygroup_points_wkb(), n)?;
            let options = as_options(geoms);
            options
                .par_chunks(1_000)
                .map(|group| {
                    line_from_ordered_points(group)
                        .map_err(|e| e.to_string())
                        .and_then(|out| enc_opt(&out))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|v| v.iter().sum())
        },
    );
    sweep_collective(
        &mut results,
        "geo.polygon_builder",
        "circle points groups of 1000",
        &[100_000, 1_000_000],
        1.0,
        "gruppi ordinati di 1000 punti -> poligono (anelli validi)",
        &|n| {
            let geoms = decode_prefix(polygroup_points_wkb(), n)?;
            let options = as_options(geoms);
            options
                .par_chunks(1_000)
                .map(|group| {
                    polygon_from_ordered_points(group)
                        .map_err(|e| e.to_string())
                        .and_then(|out| enc_opt(&out))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|v| v.iter().sum())
        },
    );

    // --- Estensioni v1.3 ------------------------------------------------------
    sweep_collective(
        &mut results,
        "geo.coverage_validate",
        "grid100 jitter",
        &[1_000, 10_000],
        1.5,
        "griglia 100x100 con overlap jitterati, tolerance=0",
        &|n| {
            let geoms = as_options(decode_prefix(grid_jitter_wkb(), n)?);
            coverage_validate_nullable(&geoms, 0.0, 1_000_000)
                .map_err(|e| e.to_string())
                .and_then(|issues| {
                    issues.iter().try_fold(0_usize, |total, issue| {
                        enc(&issue.geometry).map(|len| total + len)
                    })
                })
        },
    );
    sweep_collective(
        &mut results,
        "geo.shared_paths",
        "grid100",
        &[1_000, 10_000],
        1.5,
        "griglia 100x100 esatta (bordi condivisi), min_length=1",
        &|n| {
            let geoms = as_options(decode_prefix(grid_wkb(), n)?);
            shared_paths_nullable(&geoms, 0.001, 1.0)
                .map_err(|e| e.to_string())
                .and_then(|paths| {
                    paths.iter().try_fold(0_usize, |total, path| {
                        enc(&path.geometry).map(|len| total + len)
                    })
                })
        },
    );
    sweep_collective(
        &mut results,
        "geo.cluster_dbscan",
        "points dense [0,1000)^2",
        &[10_000, 100_000, 1_000_000],
        1.3,
        "eps=10, min_points=5",
        &|n| {
            let geoms = as_options(decode_prefix(dbscan_points_wkb(), n)?);
            dbscan_nullable(&geoms, 10.0, 5)
                .map(|labels| labels.len())
                .map_err(|e| e.to_string())
        },
    );

    // --- BackendPending (feature geos/proj non abilitate) ---------------------
    sweep_skipped(&mut results, "geo.make_valid", "backend `geos` non abilitato (BackendPending)");
    sweep_skipped(&mut results, "geo.reproject", "backend `proj` non abilitato (BackendPending)");
    sweep_skipped(&mut results, "geo.polygonize", "backend `geos` non abilitato (BackendPending)");
    sweep_skipped(&mut results, "geo.split", "backend `geos` non abilitato (BackendPending)");

    write_outputs(&results);
    eprintln!("scritti benchmarks/sweep/geo_sweep.json e geo_sweep.md ({} scenari)", results.len());
}
