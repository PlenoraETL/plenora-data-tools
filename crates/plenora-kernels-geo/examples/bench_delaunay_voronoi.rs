//! Misura di `geo.delaunay` (`extended_algorithms::delaunay`) e `geo.voronoi`
//! (`advanced::voronoi_cells`): `n` punti in coordinate UTM al centimetro
//! (casuali) o su una griglia di passo 1 m (cocircolari a quattro a quattro),
//! mediana di `runs` esecuzioni. Stampa un'impronta FNV-1a dell'uscita
//! completa (bit delle coordinate, in ordine) per confrontare due binari
//! sullo stesso input.
//!
//! Uso: `bench_delaunay_voronoi <n> <casuali|griglia> <delaunay|voronoi> [runs]`.

use std::hint::black_box;
use std::time::Instant;

use geo::{CoordsIter, Geometry, MultiPoint, Point};
use plenora_kernels_geo::advanced::voronoi_cells;
use plenora_kernels_geo::extended_algorithms::delaunay;

struct Rng(u64);

impl Rng {
    const fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
}

#[allow(clippy::cast_precision_loss)] // valori < 2^53: esatti in f64.
const fn reale(valore: u64) -> f64 {
    valore as f64
}

fn punti(n: usize, griglia: bool) -> Vec<Point<f64>> {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let lato = (n as f64).sqrt().ceil() as u64;
    if griglia {
        return (0..lato)
            .flat_map(|i| (0..lato).map(move |j| (i, j)))
            .take(n)
            .map(|(i, j)| Point::new(500_000.0 + reale(i), 4_500_000.0 + reale(j)))
            .collect();
    }
    // Densita' costante: circa un punto ogni 100 m^2, coordinate al cm.
    let centimetri = lato * 1_000;
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    (0..n)
        .map(|_| {
            let x = reale((rng.next() >> 11) % centimetri) / 100.0;
            let y = reale((rng.next() >> 11) % centimetri) / 100.0;
            Point::new(500_000.0 + x, 4_500_000.0 + y)
        })
        .collect()
}

fn fnv(impronta: &mut u64, valore: u64) {
    for byte in valore.to_le_bytes() {
        *impronta ^= u64::from(byte);
        *impronta = impronta.wrapping_mul(0x0000_0100_0000_01B3);
    }
}

fn main() {
    let argomenti: Vec<String> = std::env::args().collect();
    let n: usize = argomenti
        .get(1)
        .and_then(|valore| valore.parse().ok())
        .unwrap_or(10_000);
    let griglia = argomenti.get(2).is_some_and(|valore| valore == "griglia");
    let voronoi = argomenti.get(3).is_some_and(|valore| valore == "voronoi");
    let runs: usize = argomenti
        .get(4)
        .and_then(|valore| valore.parse().ok())
        .unwrap_or(5);
    let siti = punti(n, griglia);
    let geometrie: Vec<Geometry<f64>> = siti.iter().copied().map(Geometry::Point).collect();
    let multipunto = Geometry::MultiPoint(MultiPoint::new(siti));
    let mut tempi = Vec::with_capacity(runs);
    let mut impronta = 0xCBF2_9CE4_8422_2325_u64;
    let mut quanti = 0_usize;
    for giro in 0..runs {
        let inizio = Instant::now();
        let uscita: Vec<Geometry<f64>> = if voronoi {
            voronoi_cells(
                black_box(&geometrie),
                usize::MAX,
                plenora_kernels_geo::rust_backend::precision::Precision::new(0.01)
                    .expect("precisione"),
            )
            .expect("voronoi")
        } else {
            delaunay(black_box(&multipunto), u64::MAX, u64::MAX)
                .expect("delaunay")
                .into_iter()
                .map(Geometry::Polygon)
                .collect()
        };
        tempi.push(inizio.elapsed().as_secs_f64());
        if giro == 0 {
            quanti = uscita.len();
            for geometria in &uscita {
                for coordinata in geometria.coords_iter() {
                    fnv(&mut impronta, coordinata.x.to_bits());
                    fnv(&mut impronta, coordinata.y.to_bits());
                }
            }
        }
    }
    tempi.sort_by(f64::total_cmp);
    let mediana = tempi[tempi.len() / 2];
    println!(
        "n={n} {} {} runs={runs} mediana={:.1} ms uscite={quanti} impronta={impronta:016x}",
        if griglia { "griglia" } else { "casuali" },
        if voronoi { "voronoi" } else { "delaunay" },
        mediana * 1000.0,
    );
}
