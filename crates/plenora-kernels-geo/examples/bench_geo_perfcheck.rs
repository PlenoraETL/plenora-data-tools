//! Verifica prestazionale mirata dei percorsi toccati dai commit perf del
//! 2026-07-28 (misura richiesta da Prestazioni.md §8): la full `geo_sweep`
//! non e' eseguibile su questo host (stallo noto WSL2 `__vma_start_write`,
//! vedi `benchmarks/sweep/geo_sweep.md`), quindi fixture COMPATTE a bassa
//! pressione di allocazione, mediana di 5, stessa classe di carico:
//!
//! - `ref.decode_points`: ancora (percorso invariato per costruzione);
//! - `op.centroid_polys`: per-cella con decode+op+encode via `map_nullable`
//!   (misura il costo del Vec di determinismo ADR-0001);
//! - `op.snap_reference_2k`: R-tree del riferimento condiviso vs per-cella;
//! - `op.voronoi_2k`: pre-filtro `bounding_rect`;
//! - `op.from_wkt`: parsing senza uppercase integrale.
//!
//! Uso: `bench_geo_perfcheck` — una riga JSON per scenario su stdout.

use std::hint::black_box;
use std::time::Instant;

use geo::{Centroid, CoordsIter, Geometry, LineString, Point, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::BinaryArray;
use plenora_kernels_geo::advanced::voronoi_cells;
use plenora_kernels_geo::arrow_adapter::{encode_geometry, map_nullable};
use plenora_kernels_geo::extensions2::snap_column;
use plenora_kernels_geo::geometry_from_wkb;

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
        // violerebbe il determinismo bit-esatto (ADR-0001).
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
        // Niente mul_add/FMA (ADR-0010): forma non fusa come il contratto.
        #[allow(clippy::suboptimal_flops)]
        let jitter = 0.75 + 0.5 * rng.unit();
        // Niente mul_add/FMA (ADR-0010): forma non fusa come il contratto.
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
        .map(|_| Geometry::Point(Point::new(rng.range(0.0, 10_000.0), rng.range(0.0, 10_000.0))))
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
}
