//! Diagnostica isolata (non un gate, non un benchmark applicativo): separa
//! `buffer`, `simplify` e `centroid` su poligoni a molti vertici, a
//! dimensioni crescenti, per localizzare dove va il tempo quando il
//! candidato esatto (diff 1, orientamento esatto) rallenta drammaticamente
//! `executor::tests::geo_fusion_falls_back_when_the_governor_rejects_the_reservation`
//! (quel test resta intatto: qui la stessa fixture — poligono-cerchio — e'
//! ricostruita a parte, a dimensioni piu' piccole e crescenti).
//!
//! Ogni chiamata gira su un thread separato con un tetto per-taglia: se una
//! taglia non risponde entro il budget, la taglia e' segnata "incompleto" e
//! la diagnostica si ferma (le taglie piu' grandi non risponderebbero
//! comunque prima) invece di restare bloccata come nella corsa alla cieca.
//!
//! Uso: `cargo run --release --example diag_geo_scaling` (release: la stessa
//! build usata per il benchmark dei join, cosi' i numeri sono confrontabili;
//! debug e' comunque supportato per il confronto di profilo).

use geo::algorithm::validation::Validation;
use geo::{Geometry, LineString, Polygon};
use plenora_kernels_geo::operations::{self, SimplifyPolicy};
use plenora_kernels_geo::{transform_geometry, Operation};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Stessa costruzione di `circle_polygon_wkb` in
/// `crates/plenora-engine/src/executor/tests.rs` (non importata da li': quella
/// e' privata al modulo di test dell'altro crate) — poligono-cerchio con
/// `coords` vertici, chiusura esatta sull'angolo 0.
#[allow(clippy::cast_precision_loss)]
fn circle_polygon(coords: usize) -> Geometry<f64> {
    let mut ring: Vec<(f64, f64)> = (0..coords - 1)
        .map(|index| {
            let angle = index as f64 / (coords - 1) as f64 * std::f64::consts::TAU;
            (1_000.0 * angle.cos(), 1_000.0 * angle.sin())
        })
        .collect();
    ring.push((1_000.0, 0.0));
    Geometry::Polygon(Polygon::new(LineString::from(ring), Vec::new()))
}

/// Esegue `f` su un thread a parte; ritorna `None` se non risponde entro
/// `timeout` (il thread resta a girare in background, ma il processo intero
/// termina alla fine di `main`, quindi non e' una perdita di risorse
/// osservabile fuori da questa singola invocazione diagnostica).
fn con_tetto<F, R>(f: F, timeout: Duration) -> Option<(R, Duration)>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let inizio = Instant::now();
        let esito = f();
        let _ = tx.send((esito, inizio.elapsed()));
    });
    rx.recv_timeout(timeout).ok()
}

fn main() {
    let budget_per_taglia = Duration::from_secs(
        std::env::var("DIAG_BUDGET_SECS_PER_TAGLIA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
    );
    let taglie: Vec<usize> = vec![10, 25, 50, 100, 200, 400, 600, 800, 1200, 1600, 2000, 2400];

    println!(
        "{{\"diagnostica\":\"buffer_simplify_centroid_isolati\",\"budget_secs_per_taglia\":{}}}",
        budget_per_taglia.as_secs()
    );

    for n in taglie {
        let poligono = circle_polygon(n);

        // --- SOLA validazione OGC (`check_validation`, la stessa chiamata
        // dietro `ensure_valid`/`valida_ogc`), senza alcun kernel a valle:
        // isola il costo del gate d'ingresso comune da quello di ciascuna
        // operazione, per capire se il costo condiviso da buffer/simplify/
        // centroid sta li' o nell'algoritmo proprio di ciascuna. ---
        let validazione_esito = con_tetto(
            move || circle_polygon(n).check_validation().is_ok(),
            budget_per_taglia,
        );
        let (validazione_secs, validazione_ok) = match &validazione_esito {
            Some((risultato, durata)) => (Some(durata.as_secs_f64()), Some(*risultato)),
            None => (None, None),
        };
        stampa(n, "validazione_sola", validazione_secs, validazione_ok);
        if validazione_esito.is_none() {
            println!(
                "{{\"n\":{n},\"operazione\":\"validazione_sola\",\"stato\":\"incompleto\",\"motivo\":\"oltre il budget di {}s\"}}",
                budget_per_taglia.as_secs()
            );
        }

        // --- buffer da solo, sul cerchio grezzo (stessa distanza del test
        // originale: 5.0) ---
        let buffer_esito = con_tetto(
            move || operations::buffer(&circle_polygon(n), 5.0),
            budget_per_taglia,
        );
        let (buffer_secs, buffer_ok) = match &buffer_esito {
            Some((risultato, durata)) => (Some(durata.as_secs_f64()), Some(risultato.is_ok())),
            None => (None, None),
        };
        stampa(n, "buffer", buffer_secs, buffer_ok);
        if buffer_esito.is_none() {
            println!(
                "{{\"n\":{n},\"operazione\":\"buffer\",\"stato\":\"incompleto\",\"motivo\":\"oltre il budget di {}s, taglie successive non tentate\"}}",
                budget_per_taglia.as_secs()
            );
            break;
        }

        // --- simplify da solo, sullo stesso cerchio grezzo (non sull'esito
        // del buffer: isola il costo di simplify da quello del buffer che lo
        // precede nella pipeline reale) ---
        let simplify_esito = con_tetto(
            move || {
                operations::simplify_with_policy(
                    &circle_polygon(n),
                    0.01,
                    SimplifyPolicy::DouglasPeucker,
                )
            },
            budget_per_taglia,
        );
        let (simplify_secs, simplify_ok) = match &simplify_esito {
            Some((risultato, durata)) => (Some(durata.as_secs_f64()), Some(risultato.is_ok())),
            None => (None, None),
        };
        stampa(n, "simplify", simplify_secs, simplify_ok);

        // --- centroid da solo ---
        let centroid_esito = con_tetto(
            move || transform_geometry(Operation::Centroid, &circle_polygon(n)),
            budget_per_taglia,
        );
        let (centroid_secs, centroid_ok) = match &centroid_esito {
            Some((risultato, durata)) => (Some(durata.as_secs_f64()), Some(risultato.is_ok())),
            None => (None, None),
        };
        stampa(n, "centroid", centroid_secs, centroid_ok);

        let _ = poligono; // solo per tenere viva la costruzione sopra nel log implicito
    }
}

fn stampa(n: usize, operazione: &str, secs: Option<f64>, ok: Option<bool>) {
    use std::io::Write;
    match (secs, ok) {
        (Some(s), Some(o)) => println!(
            "{{\"n\":{n},\"operazione\":\"{operazione}\",\"stato\":\"misurato\",\"secs\":{s:.6},\"ok\":{o}}}"
        ),
        _ => println!("{{\"n\":{n},\"operazione\":\"{operazione}\",\"stato\":\"incompleto\"}}"),
    }
    std::io::stdout().flush().ok();
}
