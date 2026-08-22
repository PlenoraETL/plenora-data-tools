//! Benchmark di baseline in-process per i kernel geografici di `plenora-geo`,
//! complementare agli esempi del progetto di origine (`profile_arrow`,
//! `benchmark_spatial_join`, `benchmark_extended_topology`) che coprono il
//! trasporto Arrow v3 e lo spatial join ma non i singoli kernel su WKB.
//!
//! Fixture: box a 5 vertici con generatore LCG deterministico (seed 42),
//! stessa famiglia della fixture di `spike_arrow.py` (box casuali, coordinate
//! in [0, 10000), lati in [0.1, 50)).
//!
//! Uso: `geo-kernel-bench <benchmark> <rows> [repetitions]`
//! benchmark: wkb_decode | wkb_encode | centroid | convex_hull | envelope |
//!            buffer | simplify | area | intersects | chain_wkb | chain_fused
//!
//! Ogni benchmark a geometria singola applica il modello di costo per nodo del
//! motore oggi: decode WKB (con validazione contratto + OGC) -> kernel ->
//! encode WKB. `chain_wkb` replica una catena buffer -> simplify -> centroid ->
//! area con round-trip WKB tra i nodi; `chain_fused` esegue la stessa catena
//! con un solo decode iniziale (riferimento per il vincolo decode/encode geo minimizzato).

use std::hint::black_box;
use std::time::Instant;

use geo::{polygon, Area, Geometry};
use geozero::{CoordDimensions, ToWkb};
use plenora_geo::operations::{buffer_with_cap, simplify, BufferCapStyle};
use plenora_geo::predicates::{evaluate, SpatialPredicate};
use plenora_geo::{geometry_from_wkb, transform_wkb, Operation};

const SEED: u64 = 42;
const BUFFER_DISTANCE: f64 = 10.0;
const SIMPLIFY_TOLERANCE: f64 = 1.0;

/// LCG a 64 bit (Numerical Recipes), deterministico e sufficiente per una
/// fixture sintetica; niente dipendenze extra.
struct Lcg(u64);

impl Lcg {
    fn uniform(&mut self, low: f64, high: f64) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let unit = (self.0 >> 11) as f64 / (1u64 << 53) as f64;
        low + (high - low) * unit
    }
}

fn make_boxes(rows: usize) -> Vec<Geometry<f64>> {
    let mut rng = Lcg(SEED);
    (0..rows)
        .map(|_| {
            let cx = rng.uniform(0.0, 10_000.0);
            let cy = rng.uniform(0.0, 10_000.0);
            let w = rng.uniform(0.1, 50.0);
            let h = rng.uniform(0.1, 50.0);
            Geometry::Polygon(polygon![
                (x: cx, y: cy),
                (x: cx + w, y: cy),
                (x: cx + w, y: cy + h),
                (x: cx, y: cy + h),
                (x: cx, y: cy),
            ])
        })
        .collect()
}

fn encode(geometry: &Geometry<f64>) -> Vec<u8> {
    geometry
        .to_wkb(CoordDimensions::xy())
        .expect("encode fixture")
}

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

/// Esegue `work` una volta di warmup e poi `repetitions` volte, restituendo la
/// mediana dei tempi.
fn measure(repetitions: usize, mut work: impl FnMut() -> u64) -> (f64, u64) {
    black_box(work());
    let mut durations = Vec::with_capacity(repetitions);
    let mut sink = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        sink = work();
        durations.push(start.elapsed().as_secs_f64());
    }
    durations.sort_by(f64::total_cmp);
    (durations[durations.len() / 2], sink)
}

#[allow(clippy::too_many_lines)]
fn main() {
    let benchmark = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "centroid".into());
    let rows: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let repetitions: usize = std::env::args()
        .nth(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    assert!(rows > 0 && repetitions > 0);

    let geometries = make_boxes(rows);
    let payloads: Vec<Vec<u8>> = geometries.iter().map(encode).collect();
    let input_bytes: usize = payloads.iter().map(Vec::len).sum();

    // Geometrie decodificate una sola volta fuori dal tempo per i benchmark che
    // non misurano il decode.
    let decoded: Vec<Geometry<f64>> = match benchmark.as_str() {
        "wkb_encode" | "intersects" | "chain_fused" => geometries,
        _ => Vec::new(),
    };

    let (median, sink) = match benchmark.as_str() {
        "wkb_decode" => measure(repetitions, || {
            let mut coords = 0u64;
            for payload in &payloads {
                let geometry = geometry_from_wkb(payload).expect("decode");
                coords += geometry_coord_hint(&geometry);
                black_box(geometry);
            }
            coords
        }),
        "wkb_encode" => measure(repetitions, || {
            let mut bytes = 0u64;
            for geometry in &decoded {
                let payload = encode(geometry);
                bytes += payload.len() as u64;
                black_box(payload);
            }
            bytes
        }),
        "centroid" | "convex_hull" | "envelope" => {
            let operation = match benchmark.as_str() {
                "centroid" => Operation::Centroid,
                "convex_hull" => Operation::ConvexHull,
                _ => Operation::Envelope,
            };
            measure(repetitions, || {
                let mut bytes = 0u64;
                for payload in &payloads {
                    let output = transform_wkb(operation, payload).expect("transform");
                    bytes += output.len() as u64;
                    black_box(output);
                }
                bytes
            })
        }
        "buffer" => measure(repetitions, || {
            let mut bytes = 0u64;
            for payload in &payloads {
                let geometry = geometry_from_wkb(payload).expect("decode");
                let output =
                    buffer_with_cap(&geometry, BUFFER_DISTANCE, BufferCapStyle::Round)
                        .expect("buffer");
                let encoded = encode(&output);
                bytes += encoded.len() as u64;
                black_box(encoded);
            }
            bytes
        }),
        "simplify" => measure(repetitions, || {
            let mut bytes = 0u64;
            for payload in &payloads {
                let geometry = geometry_from_wkb(payload).expect("decode");
                let output = simplify(&geometry, SIMPLIFY_TOLERANCE).expect("simplify");
                let encoded = encode(&output);
                bytes += encoded.len() as u64;
                black_box(encoded);
            }
            bytes
        }),
        "area" => measure(repetitions, || {
            let mut total = 0u64;
            for payload in &payloads {
                let geometry = geometry_from_wkb(payload).expect("decode");
                total += black_box(geometry.unsigned_area()).to_bits();
            }
            total
        }),
        // Coppie deterministiche: la meta' pari si sovrappone, la dispari no.
        "intersects" => measure(repetitions, || {
            let mut hits = 0u64;
            for (index, left) in decoded.iter().enumerate() {
                let right = &decoded[(index * 7 + 3) % decoded.len()];
                if evaluate(left, right, SpatialPredicate::Intersects).expect("predicate") {
                    hits += 1;
                }
            }
            hits
        }),
        "chain_wkb" => measure(repetitions, || {
            let mut total = 0u64;
            for payload in &payloads {
                let node1 = geometry_from_wkb(payload).expect("decode 1");
                let node1 = buffer_with_cap(&node1, BUFFER_DISTANCE, BufferCapStyle::Round)
                    .expect("buffer");
                let node2 = geometry_from_wkb(&encode(&node1)).expect("decode 2");
                let node2 = simplify(&node2, SIMPLIFY_TOLERANCE).expect("simplify");
                let node3 = geometry_from_wkb(&encode(&node2)).expect("decode 3");
                let node3 = transform_wkb(Operation::Centroid, &encode(&node3))
                    .expect("centroid node");
                let node4 = geometry_from_wkb(&node3).expect("decode 4");
                total += black_box(node4.unsigned_area()).to_bits();
            }
            total
        }),
        "chain_fused" => measure(repetitions, || {
            let mut total = 0u64;
            for geometry in &decoded {
                let node1 = buffer_with_cap(geometry, BUFFER_DISTANCE, BufferCapStyle::Round)
                    .expect("buffer");
                let node2 = simplify(&node1, SIMPLIFY_TOLERANCE).expect("simplify");
                let node3 = node2.centroid_hint().expect("centroid");
                total += black_box(node3.unsigned_area()).to_bits();
            }
            total
        }),
        other => panic!("benchmark sconosciuto: {other}"),
    };

    println!(
        "{{\"benchmark\":\"{benchmark}\",\"rows\":{rows},\"repetitions\":{repetitions},\
\"median_seconds\":{median:.6},\"geometries_per_second\":{:.1},\
\"input_wkb_bytes\":{input_bytes},\"peak_rss_kib\":{},\"sink\":{sink}}}",
        rows as f64 / median,
        peak_rss_kib().map_or("null".into(), |value| value.to_string()),
    );
}

fn geometry_coord_hint(geometry: &Geometry<f64>) -> u64 {
    use geo::CoordsIter;
    geometry.coords_count() as u64
}

trait CentroidHint {
    fn centroid_hint(&self) -> Option<Geometry<f64>>;
}

impl CentroidHint for Geometry<f64> {
    fn centroid_hint(&self) -> Option<Geometry<f64>> {
        use geo::Centroid;
        self.centroid().map(Geometry::Point)
    }
}
