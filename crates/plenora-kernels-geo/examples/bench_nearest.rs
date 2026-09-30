//! Misura di `geo.nearest` (`nearest_matches_validated`): `n` left contro
//! `n` right, punti uniformi o quadrati piccoli, mediana di `runs`
//! esecuzioni. Stampa anche un'impronta FNV-1a dell'output completo
//! (`left`, `right`, bit della distanza) per confrontare byte per byte due
//! binari diversi sullo stesso input.
//!
//! Uso: `bench_nearest <n> <punti|quadrati> [runs]`. Un argomento assente
//! vale il predefinito (1000, `punti`, 5); uno non valido esce con codice 2.

#[path = "comune/argomenti.rs"]
mod argomenti;

use std::hint::black_box;
use std::time::Instant;

use geo::{Geometry, LineString, Point, Polygon};
use plenora_kernels_geo::analysis::nearest_matches_validated;

struct Rng(u64);

impl Rng {
    const fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// Coordinata su una griglia intera di passo 1 in `[0, lato)`: molti pari
    /// esatti, come nei dati reali arrotondati.
    #[allow(clippy::cast_precision_loss)] // lato <= 2^53: esatto in f64.
    const fn coordinata(&mut self, lato: u64) -> f64 {
        ((self.next() >> 11) % lato) as f64
    }
}

fn colonna(rng: &mut Rng, n: usize, lato: u64, quadrati: bool) -> Vec<Option<Geometry<f64>>> {
    (0..n)
        .map(|_| {
            let (x, y) = (rng.coordinata(lato), rng.coordinata(lato));
            Some(if quadrati {
                Geometry::Polygon(Polygon::new(
                    LineString::from(vec![
                        (x, y),
                        (x + 0.5, y),
                        (x + 0.5, y + 0.5),
                        (x, y + 0.5),
                        (x, y),
                    ]),
                    vec![],
                ))
            } else {
                Geometry::Point(Point::new(x, y))
            })
        })
        .collect()
}

fn main() {
    let n = argomenti::intero_positivo_arg(1, "n", 1000);
    let quadrati = argomenti::scelta_arg(2, "forma", &["punti", "quadrati"], "punti") == "quadrati";
    let runs = argomenti::intero_positivo_arg(3, "runs", 5);
    // Densita' costante: una geometria ogni 25 celle unitarie circa, con
    // coordinate intere: pari frequenti ma non dominanti.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let lato = ((n as f64 * 25.0).sqrt() as u64).max(2);
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let left = colonna(&mut rng, n, lato, quadrati);
    let right = colonna(&mut rng, n, lato, quadrati);
    let mut tempi = Vec::with_capacity(runs);
    let mut impronta = 0_u64;
    let mut righe_emesse = 0_usize;
    for _ in 0..runs {
        let inizio = Instant::now();
        let esito = nearest_matches_validated(&left, &right, None, u64::MAX, u64::MAX)
            .unwrap_or_else(|errore| panic!("nearest fallita: {errore}"));
        tempi.push(inizio.elapsed().as_secs_f64());
        let mut hash = 0xCBF2_9CE4_8422_2325_u64;
        for riga in &esito {
            for valore in [riga.left, riga.right, riga.distance.to_bits()] {
                for byte in valore.to_le_bytes() {
                    hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3);
                }
            }
        }
        impronta = hash;
        righe_emesse = esito.len();
        black_box(esito);
    }
    tempi.sort_by(f64::total_cmp);
    let mediana = tempi[tempi.len() / 2];
    println!(
        "{{\"n\":{n},\"tipo\":\"{}\",\"runs\":{runs},\"mediana_s\":{mediana:.4},\"righe\":{righe_emesse},\"impronta\":\"{impronta:016x}\"}}",
        if quadrati { "quadrati" } else { "punti" }
    );
}
