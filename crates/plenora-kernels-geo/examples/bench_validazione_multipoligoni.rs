//! Scalatura della validazione OGC col numero di parti di un `MultiPolygon`
//! e di buchi di un `Polygon` (non un gate).
//!
//! Tre forme, mediana di `RUNS` corse, taglie 100, 1k, 10k, 20k:
//!
//! - `multi_reticolo`: quadrati di lato 1 su un reticolo di passo 2, parti
//!   disgiunte con rettangoli disgiunti (l'uscita tipica di `dissolve`);
//! - `multi_scacchiera`: quadrati di lato 1 a scacchiera, che si toccano
//!   negli spigoli: ogni parte ha quattro vicini con rettangoli che si
//!   toccano, e `relate` si esegue davvero su quelle coppie;
//! - `buchi_reticolo`: un quadrato con i buchi sullo stesso reticolo;
//! - `multi_colonna`: quadrati di lato 1 in una colonna di passo 2, parti
//!   disgiunte che condividono tutte la stessa proiezione su `x` (caso
//!   sfavorevole per una scansione su `x`).
//!
//! Piu' `dissolve_validated` sul reticolo disgiunto, dove quasi tutto il
//! tempo e' la validazione dell'uscita.
//!
//! Uso: `cargo run --release --example bench_validazione_multipoligoni` —
//! una riga JSON per misura su stdout.

use std::hint::black_box;
use std::time::Instant;

use geo::{Geometry, LineString, MultiPolygon, Polygon};
use plenora_kernels_geo::check_geometry_valid;
use plenora_kernels_geo::topology::dissolve_validated;

/// Precisione dichiarata: 1 cm con coordinate in metri (docs/limiti.md, «Limiti dichiarati»).
fn precisione() -> plenora_kernels_geo::rust_backend::precision::Precision {
    plenora_kernels_geo::rust_backend::precision::Precision::new(0.01).expect("precisione valida")
}

const RUNS: usize = 5;
const TAGLIE: [usize; 4] = [100, 1_000, 10_000, 20_000];

fn quadrato(x: f64, y: f64, lato: f64) -> LineString<f64> {
    LineString::from(vec![
        (x, y),
        (x + lato, y),
        (x + lato, y + lato),
        (x, y + lato),
        (x, y),
    ])
}

/// Le celle `(colonna, riga)` di un reticolo quasi quadrato con `quante`
/// celle.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn celle(quante: usize) -> Vec<(f64, f64)> {
    let lato = (quante as f64).sqrt().ceil() as usize;
    (0..quante)
        .map(|indice| ((indice % lato) as f64, (indice / lato) as f64))
        .collect()
}

fn reticolo(quante: usize) -> Vec<Polygon<f64>> {
    celle(quante)
        .into_iter()
        .map(|(x, y)| Polygon::new(quadrato(2.0 * x, 2.0 * y, 1.0), vec![]))
        .collect()
}

/// Colonna: tutte le parti alla stessa `x`, distanti 1 in verticale.
#[allow(clippy::cast_precision_loss)]
fn colonna(quante: usize) -> Vec<Polygon<f64>> {
    (0..quante)
        .map(|indice| Polygon::new(quadrato(0.0, 2.0 * indice as f64, 1.0), vec![]))
        .collect()
}

/// Scacchiera: sulla riga `r` le celle stanno a `x = 2c + (r mod 2)`, e le
/// righe distano 1, quindi ogni quadrato tocca i diagonali in un vertice.
fn scacchiera(quante: usize) -> Vec<Polygon<f64>> {
    celle(quante)
        .into_iter()
        .map(|(x, y)| {
            let sfasamento = y % 2.0;
            Polygon::new(quadrato(2.0f64.mul_add(x, sfasamento), y, 1.0), vec![])
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn con_buchi(quanti: usize) -> Polygon<f64> {
    let buchi: Vec<LineString<f64>> = celle(quanti)
        .into_iter()
        .map(|(x, y)| quadrato(2.0f64.mul_add(x, 1.0), 2.0f64.mul_add(y, 1.0), 1.0))
        .collect();
    let estensione = 2.0f64.mul_add((quanti as f64).sqrt().ceil(), 2.0);
    Polygon::new(quadrato(0.0, 0.0, estensione), buchi)
}

fn mediana(mut campioni: Vec<f64>) -> f64 {
    campioni.sort_by(f64::total_cmp);
    campioni[campioni.len() / 2]
}

fn misura(forma: &str, taglia: usize, mut corsa: impl FnMut() -> bool) {
    let mut campioni = Vec::with_capacity(RUNS);
    let mut esito = true;
    for _ in 0..RUNS {
        let inizio = Instant::now();
        esito &= black_box(corsa());
        campioni.push(inizio.elapsed().as_secs_f64());
    }
    println!(
        "{{\"forma\":\"{forma}\",\"taglia\":{taglia},\"mediana_ms\":{:.3},\"ok\":{esito}}}",
        mediana(campioni) * 1e3
    );
}

fn main() {
    let solo: Option<String> = std::env::args().nth(1);
    let vuole = |forma: &str| solo.as_deref().is_none_or(|filtro| forma.contains(filtro));
    for taglia in TAGLIE {
        if vuole("multi_reticolo") {
            let geometria = Geometry::MultiPolygon(MultiPolygon(reticolo(taglia)));
            misura("multi_reticolo", taglia, || {
                check_geometry_valid(&geometria).is_ok()
            });
        }
        if vuole("multi_scacchiera") {
            let geometria = Geometry::MultiPolygon(MultiPolygon(scacchiera(taglia)));
            misura("multi_scacchiera", taglia, || {
                check_geometry_valid(&geometria).is_ok()
            });
        }
        if vuole("multi_colonna") {
            let geometria = Geometry::MultiPolygon(MultiPolygon(colonna(taglia)));
            misura("multi_colonna", taglia, || {
                check_geometry_valid(&geometria).is_ok()
            });
        }
        if vuole("buchi_reticolo") {
            let geometria = Geometry::Polygon(con_buchi(taglia));
            misura("buchi_reticolo", taglia, || {
                check_geometry_valid(&geometria).is_ok()
            });
        }
        if vuole("dissolve_reticolo") {
            let celle: Vec<Geometry<f64>> = reticolo(taglia)
                .into_iter()
                .map(Geometry::Polygon)
                .collect();
            misura("dissolve_reticolo", taglia, || {
                dissolve_validated(&celle, precisione()).is_ok()
            });
        }
    }
}
