//! Scalatura della validazione OGC col numero di vertici (non un gate).
//!
//! Due percorsi sulla stessa geometria, mediana di `RUNS` corse:
//!
//! - `geo.check_validation`: la validazione di `geo` vendorizzata, con la
//!   ricerca quadratica delle auto-intersezioni (il percorso generico);
//! - `plenora.check_geometry_valid`: la barriera del prodotto, cioe' cio' che
//!   `geometry_from_wkb` e i kernel eseguono davvero.
//!
//! Due forme: il cerchio (stessa costruzione di `diag_geo_scaling`) e un
//! pettine a denti orizzontali, dove i segmenti condividono quasi tutta la
//! stessa proiezione su `x` (caso sfavorevole per una scansione su `x`).
//!
//! Uso: `cargo run --release --example bench_validazione_ogc` — una riga
//! JSON per misura su stdout.

use std::hint::black_box;
use std::time::Instant;

use geo::algorithm::validation::Validation;
use geo::{Geometry, LineString, Polygon};
use plenora_kernels_geo::check_geometry_valid;

const RUNS: usize = 5;
const TAGLIE: [usize; 4] = [10, 100, 1_000, 10_000];

#[allow(clippy::cast_precision_loss)]
fn cerchio(vertici: usize) -> Geometry<f64> {
    let mut anello: Vec<(f64, f64)> = (0..vertici - 1)
        .map(|indice| {
            let angolo = indice as f64 / (vertici - 1) as f64 * std::f64::consts::TAU;
            (1_000.0 * angolo.cos(), 1_000.0 * angolo.sin())
        })
        .collect();
    anello.push((1_000.0, 0.0));
    Geometry::Polygon(Polygon::new(LineString::from(anello), Vec::new()))
}

/// Pettine: dorso verticale a `x = 0`, denti lunghi 1000 verso destra,
/// spessi 1 e distanti 1. Circa `vertici` vertici.
#[allow(clippy::cast_precision_loss)]
fn pettine(vertici: usize) -> Geometry<f64> {
    let denti = (vertici / 4).max(1);
    let mut anello: Vec<(f64, f64)> = Vec::with_capacity(4 * denti + 3);
    anello.push((0.0, 0.0));
    for dente in 0..denti {
        let base = 2.0 * dente as f64;
        anello.push((1_000.0, base));
        anello.push((1_000.0, base + 1.0));
        anello.push((1.0, base + 1.0));
        anello.push((1.0, base + 2.0));
    }
    anello.push((0.0, 2.0 * denti as f64));
    anello.push((0.0, 0.0));
    Geometry::Polygon(Polygon::new(LineString::from(anello), Vec::new()))
}

fn mediana(mut campioni: Vec<f64>) -> f64 {
    campioni.sort_by(f64::total_cmp);
    campioni[campioni.len() / 2]
}

fn misura(forma: &str, percorso: &str, vertici: usize, mut corsa: impl FnMut() -> bool) {
    // Ripetizioni per corsa: abbastanza da superare la risoluzione del clock
    // sulle taglie piccole, una sola sulle grandi.
    let ripetizioni = (20_000 / vertici).max(1);
    let mut campioni = Vec::with_capacity(RUNS);
    let mut valida = true;
    for _ in 0..RUNS {
        let inizio = Instant::now();
        for _ in 0..ripetizioni {
            valida &= black_box(corsa());
        }
        #[allow(clippy::cast_precision_loss)]
        campioni.push(inizio.elapsed().as_secs_f64() / ripetizioni as f64);
    }
    println!(
        "{{\"forma\":\"{forma}\",\"percorso\":\"{percorso}\",\"vertici\":{vertici},\
         \"mediana_us\":{:.3},\"valida\":{valida}}}",
        mediana(campioni) * 1e6
    );
}

fn main() {
    for (forma, costruttore) in [
        ("cerchio", cerchio as fn(usize) -> Geometry<f64>),
        ("pettine", pettine),
    ] {
        for vertici in TAGLIE {
            let geometria = costruttore(vertici);
            let conteggio = match &geometria {
                Geometry::Polygon(poligono) => poligono.exterior().0.len(),
                _ => 0,
            };
            misura(forma, "geo.check_validation", conteggio, || {
                geometria.check_validation().is_ok()
            });
            misura(forma, "plenora.check_geometry_valid", conteggio, || {
                check_geometry_valid(&geometria).is_ok()
            });
        }
    }
}
