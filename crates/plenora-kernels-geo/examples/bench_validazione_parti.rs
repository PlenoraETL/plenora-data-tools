//! Validazione OGC delle uscite con molte parti e degli anelli radiali (non
//! un gate).
//!
//! Scenari, mediana di `RUNS` corse:
//!
//! - `intersezione_stelle_5k`: `boolean_operation` (intersezione) di due
//!   stelle da 5 000 vertici con i centri a distanza pari al raggio, fine a
//!   fine: l'uscita e' un `MultiPolygon` con migliaia di parti, e prima
//!   della preparazione delle parti quasi tutto il tempo era la sua
//!   validazione (`relate` su ogni coppia a rettangoli sovrapposti);
//! - `valida_uscita_stelle_5k`: la sola validazione di quell'uscita;
//! - `from_wkb_stella_{2k,10k}`: `geometry_from_wkb` di una stella, i cui
//!   segmenti radiali hanno rettangoli sovrapposti su entrambi gli assi
//!   (limite dichiarato della ricerca delle auto-intersezioni).
//!
//! Uso: `cargo run --release --example bench_validazione_parti` — una riga
//! JSON per misura su stdout.

#[path = "comune/argomenti.rs"]
mod argomenti;

use std::hint::black_box;
use std::time::Instant;

use geo::{Geometry, LineString, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_kernels_geo::topology::{boolean_operation, BooleanOperation};
use plenora_kernels_geo::{check_geometry_valid, geometry_from_wkb};

const RUNS: usize = 5;

/// Precisione dichiarata: 1 cm con coordinate in metri (docs/limiti.md, «Limiti dichiarati»).
fn precisione() -> plenora_kernels_geo::rust_backend::precision::Precision {
    plenora_kernels_geo::rust_backend::precision::Precision::new(0.01).expect("precisione valida")
}

struct Rng(u64);

impl Rng {
    const fn seme(seme: u64) -> Self {
        Self(seme)
    }

    const fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0
    }

    #[allow(clippy::cast_precision_loss)] // 2^53 esatta in f64.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }
}

/// Stella: `vertici` punti a raggio `raggio * [0.75, 1.25)` su angoli
/// equispaziati (stessa costruzione di `bench_geo_perfcheck`).
fn stella(rng: &mut Rng, raggio: f64, vertici: usize) -> Polygon<f64> {
    let mut anello = Vec::with_capacity(vertici + 1);
    for indice in 0..vertici {
        #[allow(clippy::cast_precision_loss)] // indice < 2^53: esatto.
        let angolo = indice as f64 * 2.0 * std::f64::consts::PI / vertici as f64;
        // Niente mul_add/FMA: stesso risultato su ogni piattaforma (AGENTS.md, determinismo).
        #[allow(clippy::suboptimal_flops)]
        let fattore = 0.75 + 0.5 * rng.unit();
        #[allow(clippy::suboptimal_flops)]
        anello.push((
            raggio * fattore * angolo.cos(),
            raggio * fattore * angolo.sin(),
        ));
    }
    if let Some(primo) = anello.first().copied() {
        anello.push(primo);
    }
    Polygon::new(LineString::from(anello), vec![])
}

fn mediana(mut campioni: Vec<f64>) -> f64 {
    campioni.sort_by(f64::total_cmp);
    campioni[campioni.len() / 2]
}

fn misura(scenario: &str, ripetizioni: usize, mut corsa: impl FnMut() -> bool) {
    let mut campioni = Vec::with_capacity(ripetizioni);
    let mut valida = true;
    for _ in 0..ripetizioni {
        let inizio = Instant::now();
        valida &= black_box(corsa());
        campioni.push(inizio.elapsed().as_secs_f64());
    }
    println!(
        "{{\"scenario\":\"{scenario}\",\"mediana_ms\":{:.3},\"valida\":{valida}}}",
        mediana(campioni) * 1e3
    );
}

fn main() {
    // Assente vale `RUNS`; con 0 la mediana non esiste, e un valore
    // illeggibile non deve diventare in silenzio quello predefinito.
    let corse = argomenti::intero_positivo_env("CORSE", RUNS);

    let mut rng = Rng::seme(0x2545_F491_4F6C_DD1D);
    let sinistra = Geometry::Polygon(stella(&mut rng, 100.0, 5_000));
    // Centri a distanza pari al raggio: il nucleo comune e' una parte
    // grande, attorno migliaia di parti piccole (7 922) i cui rettangoli si
    // sovrappongono al suo.
    let destra = Geometry::Polygon(geo::Translate::translate(
        &stella(&mut rng, 100.0, 5_000),
        100.0,
        0.0,
    ));
    let uscita = boolean_operation(
        &sinistra,
        &destra,
        BooleanOperation::Intersection,
        precisione(),
    )
    .expect("intersezione valida");
    if let Geometry::MultiPolygon(parti) = &uscita {
        eprintln!("parti dell'uscita: {}", parti.0.len());
    }
    misura("valida_uscita_stelle_5k", corse, || {
        check_geometry_valid(&uscita).is_ok()
    });
    misura("intersezione_stelle_5k", corse, || {
        boolean_operation(
            &sinistra,
            &destra,
            BooleanOperation::Intersection,
            precisione(),
        )
        .is_ok()
    });

    for vertici in [2_000, 10_000] {
        let wkb = Geometry::Polygon(stella(&mut rng, 100.0, vertici))
            .to_wkb(CoordDimensions::xy())
            .expect("codifica");
        misura(
            &format!("from_wkb_stella_{}k", vertici / 1_000),
            corse.max(21),
            || geometry_from_wkb(&wkb).is_ok(),
        );
    }
}
