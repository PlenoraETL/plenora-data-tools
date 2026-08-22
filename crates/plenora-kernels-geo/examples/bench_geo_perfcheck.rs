//! Verifica prestazionale mirata dei percorsi toccati dai commit perf del
//! 2026-07-28 (misura richiesta da architettura.md): la full `geo_sweep`
//! non e' eseguibile su questo host (stallo noto WSL2 `__vma_start_write`
//! sotto carico di allocazioni intensive), quindi fixture COMPATTE a bassa
//! pressione di allocazione, mediana di 5, stessa classe di carico:
//!
//! - `ref.decode_points`: ancora (percorso invariato per costruzione);
//! - `op.centroid_polys`: per-cella con decode+op+encode via `map_nullable`
//!   (misura il costo del Vec di determinismo architettura.md#determinismo);
//! - `op.snap_reference_2k`: R-tree del riferimento condiviso vs per-cella;
//! - `op.voronoi_2k`: pre-filtro `bounding_rect`;
//! - `op.from_wkt`: parsing senza uppercase integrale;
//! - `op.clip_inside_mask` / `op.overlay_union_unchanged`: scenari
//!   "passthrough" — output identico all'input per costruzione (clip con
//!   maschera a dominio intero, union di griglie disgiunte), misurano il
//!   costo decode+encode che un futuro passthrough WKB eliminerebbe;
//! - `ref.decode_polys` / `ref.encode_polys`: riferimenti decode/encode sui
//!   poligoni semplici (stesso stile di `ref.decode_points` /
//!   `ref.encode_points`);
//! - `ref.decode_rects` / `ref.encode_rects`: riferimenti sui rettangoli
//!   delle griglie di overlay (ancore di `op.overlay_union_unchanged`).
//!
//! Uso: `bench_geo_perfcheck` — una riga JSON per scenario su stdout.

// Gli scenari sono misure lineari in sequenza: spezzare `main` in fn
// aggiungerebbe solo indirezione (stesso allow di bench_geo_sweep).
#![allow(clippy::too_many_lines)]

use std::hint::black_box;
use std::time::Instant;

use geo::{Centroid, CoordsIter, Geometry, LineString, Point, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::BinaryArray;
use plenora_kernels_geo::advanced::voronoi_cells;
use plenora_kernels_geo::arrow_adapter::{encode_geometry, map_nullable};
use plenora_kernels_geo::extensions2::snap_column;
use plenora_kernels_geo::geometry_from_wkb;
use plenora_kernels_geo::topology::{
    clip_to_mask_validated, polygon_overlay_validated, OverlayMode,
};

const RUNS: usize = 5;

struct Rng(u64);

impl Rng {
    const fn seeded() -> Self {
        Self(0x2545_F491_4F6C_DD1D)
    }

    const fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0
    }

    #[allow(clippy::cast_precision_loss)] // 2^53 esatta in f64: schema uniforme standard.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    fn range(&mut self, min: f64, max: f64) -> f64 {
        // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
        // violerebbe il determinismo bit-esatto (architettura.md#determinismo).
        #[allow(clippy::suboptimal_flops)]
        let value = min + (max - min) * self.unit();
        value
    }
}

fn star_polygon(rng: &mut Rng, cx: f64, cy: f64, radius: f64, vertices: usize) -> Polygon<f64> {
    let mut outline = Vec::with_capacity(vertices + 1);
    for index in 0..vertices {
        #[allow(clippy::cast_precision_loss)] // index < 2^53: esatto.
        let angle = index as f64 * 2.0 * std::f64::consts::PI / vertices as f64;
        // Niente mul_add/FMA (architettura.md#determinismo): forma non fusa come il contratto.
        #[allow(clippy::suboptimal_flops)]
        let jitter = 0.75 + 0.5 * rng.unit();
        // Niente mul_add/FMA (architettura.md#determinismo): forma non fusa come il contratto.
        #[allow(clippy::suboptimal_flops)]
        let point = (
            cx + radius * jitter * angle.cos(),
            cy + radius * jitter * angle.sin(),
        );
        outline.push(point);
    }
    if let Some(first) = outline.first().copied() {
        outline.push(first);
    }
    Polygon::new(LineString::from(outline), vec![])
}

/// Griglia 50x50 di rettangoli 100x100 su [shift, shift+5000)^2
/// (deterministica, senza RNG).
fn grid_geometries(shift: f64) -> Vec<Geometry<f64>> {
    let mut geometries = Vec::with_capacity(2_500);
    for j in 0..50 {
        for i in 0..50 {
            // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE e
            // violerebbe il determinismo bit-esatto (architettura.md#determinismo).
            #[allow(clippy::suboptimal_flops)]
            let x0 = f64::from(i) * 100.0 + shift;
            #[allow(clippy::suboptimal_flops)]
            let y0 = f64::from(j) * 100.0 + shift;
            let x1 = x0 + 100.0;
            let y1 = y0 + 100.0;
            geometries.push(Geometry::Polygon(Polygon::new(
                LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
                vec![],
            )));
        }
    }
    geometries
}

fn cells_of(geometries: &[Geometry<f64>]) -> BinaryArray {
    let cells: Vec<Option<Vec<u8>>> = geometries
        .iter()
        .map(|geometry| Some(encode_geometry(geometry).expect("encode fixture")))
        .collect();
    cells
        .iter()
        .map(|cell| cell.as_deref())
        .collect::<BinaryArray>()
}

fn wkb_of(geometry: &Geometry<f64>) -> Vec<u8> {
    geometry.to_wkb(CoordDimensions::xy()).expect("wkb fixture")
}

fn measure(name: &str, units: usize, mut f: impl FnMut() -> usize) {
    let mut durations = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        black_box(f());
        durations.push(start.elapsed().as_secs_f64());
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    // units << 2^53: la conversione e' esatta per costruzione.
    #[allow(clippy::cast_precision_loss)]
    let rate = units as f64 / median;
    println!(
        "{{\"scenario\":\"{name}\",\"units\":{units},\"median_seconds\":{median},\"units_per_second\":{rate}}}"
    );
}

fn main() {
    let mut rng = Rng::seeded();

    // ref.decode_points: 200k punti (ancora; percorso invariato).
    let points: Vec<Geometry<f64>> = (0..200_000)
        .map(|_| {
            Geometry::Point(Point::new(
                rng.range(0.0, 10_000.0),
                rng.range(0.0, 10_000.0),
            ))
        })
        .collect();
    let point_cells = cells_of(&points);
    measure("ref.decode_points", 200_000, || {
        point_cells
            .iter()
            .map(|cell| {
                geometry_from_wkb(cell.expect("cella non null"))
                    .map(|geometry| black_box(geometry).coords_count())
                    .expect("decode")
            })
            .sum()
    });

    // op.centroid_polys: 20k poligoni da 100 vertici, percorso per-cella
    // completo (map_nullable: decode + kernel + encode).
    let polys: Vec<Geometry<f64>> = (0..20_000)
        .map(|_| {
            let cx = rng.range(500.0, 9_500.0);
            let cy = rng.range(500.0, 9_500.0);
            Geometry::Polygon(star_polygon(&mut rng, cx, cy, 100.0, 100))
        })
        .collect();
    let poly_cells = cells_of(&polys);
    measure("op.centroid_polys", 20_000, || {
        map_nullable(&poly_cells, |payload| {
            let geometry = geometry_from_wkb(payload)
                .map_err(|error| plenora_core::PlenoraError::InvalidPlan(error.to_string()))?;
            let centroid = geometry
                .centroid()
                .ok_or_else(|| plenora_core::PlenoraError::InvalidPlan("centroide vuoto".into()))?;
            encode_geometry(&Geometry::Point(centroid)).map(Some)
        })
        .expect("centroid column")
        .len()
    });

    // op.clip_inside_mask: scenario "passthrough" — maschera = rettangolo su
    // tutto il dominio [0,10000)^2 delle fixture: ogni output e' identico
    // all'input per costruzione, quindi il tempo misura quasi solo
    // decode+encode, cioe' il risparmio potenziale di un passthrough WKB
    // (riuso del payload di input invece di ri-encode). Solo misura, il
    // passthrough NON e' implementato.
    let full_domain_mask = Geometry::Polygon(Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (10_000.0, 0.0),
            (10_000.0, 10_000.0),
            (0.0, 10_000.0),
            (0.0, 0.0),
        ]),
        vec![],
    ));
    measure("op.clip_inside_mask", 20_000, || {
        let geometries: Vec<Geometry<f64>> = poly_cells
            .iter()
            .map(|cell| geometry_from_wkb(cell.expect("cella non null")).expect("decode"))
            .collect();
        // Variante `*_validated`: rispecchia il percorso di produzione
        // (engine `pair.rs`), dove la validazione OGC e' gia' avvenuta al
        // decode (`geometry_from_wkb`, righe sopra) e il kernel non
        // rivalida — precondizione dimostrata per costruzione (R0.1).
        clip_to_mask_validated(&geometries, std::slice::from_ref(&full_domain_mask))
            .expect("clip")
            .iter()
            .map(|out| {
                out.as_ref().map_or(0, |geometry| {
                    encode_geometry(geometry).expect("encode").len()
                })
            })
            .sum()
    });

    // op.overlay_union_unchanged: scenario "passthrough" — union di due
    // griglie 50x50 DISGIUNTE (la seconda traslata fuori dominio): nessuna
    // intersezione, tutti i pezzi invariati; il decode+encode di ogni pezzo
    // e' il costo che un passthrough WKB azzererebbe. Solo misura, il
    // passthrough NON e' implementato.
    let overlay_left_geoms = grid_geometries(0.0);
    let overlay_right_geoms = grid_geometries(100_000.0);
    let overlay_left = cells_of(&overlay_left_geoms);
    let overlay_right = cells_of(&overlay_right_geoms);
    measure("op.overlay_union_unchanged", 5_000, || {
        let decode = |cells: &BinaryArray| {
            cells
                .iter()
                .map(|cell| geometry_from_wkb(cell.expect("cella non null")).expect("decode"))
                .collect::<Vec<_>>()
        };
        let left = decode(&overlay_left);
        let right = decode(&overlay_right);
        // Variante `*_validated`: come sopra, rispecchia il percorso engine
        // (input validati al decode, nessuna rivalidazione nel kernel).
        polygon_overlay_validated(
            &left,
            &right,
            OverlayMode::Union,
            1_000_000_000_000,
            1_000_000_000_000,
        )
        .expect("overlay")
        .iter()
        .map(|piece| encode_geometry(&piece.geometry).expect("encode").len())
        .sum()
    });

    // op.snap_reference_2k: 10k punti snappati su riferimento da 2_000
    // vertici (HEAD costruisce l'R-tree una volta; il riferimento lo
    // ricostruisce per cella).
    let snap_cells: BinaryArray = point_cells.iter().take(10_000).collect();
    let cx = rng.range(4_000.0, 6_000.0);
    let cy = rng.range(4_000.0, 6_000.0);
    let reference = Geometry::Polygon(star_polygon(&mut rng, cx, cy, 100.0, 2_000));
    measure("op.snap_reference_2k", 10_000, || {
        snap_column(&snap_cells, &reference, 250.0)
            .expect("snap column")
            .len()
    });

    // op.voronoi_2k: 2_000 punti (pre-filtro bounding_rect in HEAD).
    let voronoi_geometries: Vec<Geometry<f64>> = (0..2_000)
        .map(|_| Geometry::Point(Point::new(rng.range(0.0, 1_000.0), rng.range(0.0, 1_000.0))))
        .collect();
    measure("op.voronoi_2k", 1, || {
        voronoi_cells(&voronoi_geometries, 100_000)
            .map(black_box)
            .expect("voronoi")
            .len()
    });

    // op.from_wkt: 10k celle WKT (parsing senza uppercase integrale).
    let wkts: Vec<Option<String>> = (0..10_000)
        .map(|index| Some(format!("POINT ({x} {x})", x = f64::from(index) * 0.5)))
        .collect();
    let wkt_column = plenora_core::arrow::array::StringArray::from(wkts);
    measure("op.from_wkt", 10_000, || {
        plenora_kernels_geo::extensions::from_wkt_column(
            &wkt_column,
            plenora_kernels_geo::extensions::OnWktError::Fail,
        )
        .expect("from_wkt")
        .len()
    });

    // Ancora finale: encode punti (percorso invariato).
    measure("ref.encode_points", 200_000, || {
        points
            .iter()
            .map(|geometry| black_box(wkb_of(geometry)).len())
            .sum()
    });

    // ref.decode_polys / ref.encode_polys: riferimenti decode/encode sui
    // poligoni semplici (ancore per il risparmio potenziale del passthrough,
    // cfr. op.clip_inside_mask).
    measure("ref.decode_polys", 20_000, || {
        poly_cells
            .iter()
            .map(|cell| {
                geometry_from_wkb(cell.expect("cella non null"))
                    .map(|geometry| black_box(geometry).coords_count())
                    .expect("decode")
            })
            .sum()
    });
    measure("ref.encode_polys", 20_000, || {
        polys
            .iter()
            .map(|geometry| black_box(wkb_of(geometry)).len())
            .sum()
    });

    // ref.decode_rects / ref.encode_rects: riferimenti decode/encode sui
    // rettangoli delle griglie di overlay (ancore per
    // op.overlay_union_unchanged: i pezzi della union disgiunta sono questi
    // stessi rettangoli, invariati).
    measure("ref.decode_rects", 5_000, || {
        overlay_left
            .iter()
            .chain(overlay_right.iter())
            .map(|cell| {
                geometry_from_wkb(cell.expect("cella non null"))
                    .map(|geometry| black_box(geometry).coords_count())
                    .expect("decode")
            })
            .sum()
    });
    measure("ref.encode_rects", 5_000, || {
        overlay_left_geoms
            .iter()
            .chain(overlay_right_geoms.iter())
            .map(|geometry| black_box(wkb_of(geometry)).len())
            .sum()
    });
}
