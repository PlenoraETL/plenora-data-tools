//! Misura di `geo.delaunay` (`extended_algorithms::delaunay`) e `geo.voronoi`
//! (`advanced::voronoi_cells`): `n` punti in coordinate UTM al centimetro
//! (casuali) o su una griglia di passo 1 m (cocircolari a quattro a quattro),
//! mediana di `runs` esecuzioni. Stampa un'impronta FNV-1a dell'uscita
//! completa (bit delle coordinate, in ordine) per confrontare due binari
//! sullo stesso input.
//!
//! Uso: `bench_delaunay_voronoi <n> <casuali|griglia> <delaunay|voronoi> [runs]`.
//! Un argomento assente vale il predefinito (10 000, `casuali`, `delaunay`,
//! 5); uno non valido esce con codice 2.

#[path = "comune/argomenti.rs"]
mod argomenti;

use plenora_kernels_geo::crs::CrsContext;
use std::hint::black_box;
use std::time::Instant;

use geo::{CoordsIter, Geometry, MultiPoint, Point};
use plenora_kernels_geo::advanced::with_crs::voronoi_cells;
use plenora_kernels_geo::extended_algorithms::with_crs::delaunay;

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
    let n = argomenti::intero_positivo_arg(1, "n", 10_000);
    let griglia =
        argomenti::scelta_arg(2, "disposizione", &["casuali", "griglia"], "casuali") == "griglia";
    let voronoi =
        argomenti::scelta_arg(3, "costruzione", &["delaunay", "voronoi"], "delaunay") == "voronoi";
    let runs = argomenti::intero_positivo_arg(4, "runs", 5);
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
                CrsContext::new(&[&crs_fixture_precision(
                    plenora_kernels_geo::rust_backend::precision::Precision::new(0.01)
                        .expect("precisione"),
                )]),
                black_box(&geometrie),
                usize::MAX,
            )
            .map_err(kernel_error_fixture)
            .expect("voronoi")
        } else {
            delaunay(
                CrsContext::new(&[&plenora_core::crs::resolve_crs("EPSG:3857", "crs")
                    .expect("CRS fixture metrico")]),
                black_box(&multipunto),
                u64::MAX,
                u64::MAX,
            )
            .map_err(kernel_error_fixture)
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

// Un rifiuto CRS inatteso deve far fallire la fixture, senza mascherare l'oracolo del kernel.
fn kernel_error_fixture<E>(error: plenora_kernels_geo::crs::DirectError<E>) -> E {
    match error {
        plenora_kernels_geo::crs::DirectError::Kernel(error) => error,
        plenora_kernels_geo::crs::DirectError::Crs(_) => panic!("CRS della fixture rifiutato"),
    }
}

// Sistema cartesiano della fixture: la sua unita' rende esplicita la precisione attesa.
fn crs_fixture_precision(
    precision: plenora_kernels_geo::rust_backend::precision::Precision,
) -> plenora_core::crs::ResolvedCrs {
    let crs = plenora_core::crs::ResolvedCrs::from_resolved_parts(
        "fixture".to_owned(),
        serde_json::json!({"type": "ProjectedCRS", "name": "fixture"}),
        plenora_core::crs::CrsKind::Projected,
        Some(0.01 / precision.value()),
    );
    assert_eq!(
        crs.precisione_coordinate().map(f64::to_bits),
        Some(precision.value().to_bits()),
        "precisione della fixture"
    );
    crs
}
