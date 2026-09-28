//! L'impalcatura comune dei benchmark di questo crate.
//!
//! Qui c'e' **solo** cio' che non distingue un benchmark dall'altro: leggere
//! il picco di memoria, cronometrare N ripetizioni, emettere la riga JSON.
//! Cio' che distingue — la fixture, gli scenari, i loro nomi, cosa si misura
//! — resta in ciascun file, dove si legge accanto al numero che produce.
//!
//! # Il confine del cronometro non e' negoziabile
//!
//! Warm-up fuori dalla misura, un `Instant::now()` per ripetizione, mediana
//! all'indice `len / 2`, `black_box` sull'uscita: sono le convenzioni che
//! rendono confrontabili le righe di baseline gia' raccolte. Stanno in
//! [`cronometra`] perche' sono le stesse in tutti i file: il modulo le
//! raccoglie, non le uniforma.
//!
//! # Tre uscite, non una
//!
//! [`run_scenario`], [`measure`] e [`measure_record`] condividono il ciclo e
//! **non** l'uscita: la prima non ha `note`, la terza rende una
//! [`Measurement`] a chi riepiloga. Fonderle costringerebbe chi non riepiloga
//! a costruire un valore da buttare e a leggere `/proc` una seconda volta —
//! lavoro in piu' dentro un programma che esiste per misurare.
//!
//! Non attraversa i confini di crate: i benchmark geo hanno le loro copie, e
//! una copia residua non giustifica una dipendenza nuova.

// Ogni benchmark usa un sottoinsieme di questa impalcatura: cio' che non usa
// non e' codice morto, e' codice di un altro benchmark.
#![allow(dead_code)]

pub mod fixture;
pub mod lcg;
pub mod rng;

use std::hint::black_box;
use std::time::Instant;

use plenora_core::arrow::array::RecordBatch;
use serde_json::json;

/// Picco di memoria residente del processo, in KiB.
///
/// `None` dove `/proc` non c'e': un benchmark che gira comunque vale piu' di
/// un benchmark che rifiuta di partire fuori da Linux.
pub fn peak_rss_kib() -> Option<u64> {
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

/// Mediana, righe al secondo e righe in uscita di `repetitions` ripetizioni.
///
/// Il warm-up e' fuori dalla misura; dentro il ciclo c'e' un `Instant` per
/// ripetizione e nient'altro.
fn cronometra(
    rows: usize,
    repetitions: usize,
    mut operation: impl FnMut() -> RecordBatch,
) -> (f64, f64, usize) {
    black_box(operation());
    let mut durations = Vec::with_capacity(repetitions);
    let mut output_rows = 0;
    for _ in 0..repetitions {
        let start = Instant::now();
        let output = operation();
        durations.push(start.elapsed().as_secs_f64());
        output_rows = output.num_rows();
        black_box(output);
    }
    durations.sort_by(f64::total_cmp);
    let median = durations[durations.len() / 2];
    #[allow(clippy::cast_precision_loss)]
    let rows_per_second = rows as f64 / median;
    (median, rows_per_second, output_rows)
}

/// Cronometra uno scenario ed emette la riga JSON, senza `note`.
///
/// `operation` e' `FnMut` e non prende argomenti: chi misura una sola
/// trasformazione passa `|| trasforma(&input)`, chi ha bisogno di stato fra
/// una ripetizione e l'altra lo cattura.
pub fn run_scenario(
    name: &str,
    rows: usize,
    repetitions: usize,
    operation: impl FnMut() -> RecordBatch,
) {
    let (median, rows_per_second, output_rows) = cronometra(rows, repetitions, operation);
    println!(
        "{}",
        serde_json::to_string(&json!({
            "scenario": name,
            "rows": rows,
            "repetitions": repetitions,
            "median_seconds": median,
            "rows_per_second": rows_per_second,
            "output_rows": output_rows,
            "peak_rss_kib": peak_rss_kib(),
        }))
        .expect("JSON")
    );
}

/// Come [`run_scenario`], con una `note` nel JSON.
pub fn measure(
    op: &'static str,
    rows: usize,
    repetitions: usize,
    note: &str,
    execute: impl Fn() -> RecordBatch,
) {
    let (median, rows_per_second, output_rows) = cronometra(rows, repetitions, execute);
    let record = json!({
        "scenario": op,
        "rows": rows,
        "repetitions": repetitions,
        "median_seconds": median,
        "rows_per_second": rows_per_second,
        "output_rows": output_rows,
        "peak_rss_kib": peak_rss_kib(),
        "note": note,
    });
    println!("{}", serde_json::to_string(&record).expect("JSON"));
}

/// Una misura, per i benchmark che ne raccolgono molte e le riepilogano.
#[derive(Debug)]
pub struct Measurement {
    pub op: &'static str,
    pub rows: usize,
    pub repetitions: usize,
    pub median_seconds: f64,
    pub rows_per_second: f64,
    pub output_rows: usize,
    pub peak_rss_kib: Option<u64>,
    pub note: String,
}

/// Come [`measure`], e rende la misura a chi la riepiloga.
///
/// Le due letture di `/proc` — una per il JSON, una per il valore reso —
/// sono quelle su cui poggiano le righe di baseline: `VmHWM` e' monotono,
/// quindi la seconda non puo' essere minore, e ridurle a una sarebbe un
/// cambio di comportamento travestito da riordino.
pub fn measure_record(
    op: &'static str,
    rows: usize,
    repetitions: usize,
    note: &str,
    execute: impl Fn() -> RecordBatch,
) -> Measurement {
    let (median, rows_per_second, output_rows) = cronometra(rows, repetitions, execute);
    let record = json!({
        "scenario": op,
        "rows": rows,
        "repetitions": repetitions,
        "median_seconds": median,
        "rows_per_second": rows_per_second,
        "output_rows": output_rows,
        "peak_rss_kib": peak_rss_kib(),
        "note": note,
    });
    println!("{}", serde_json::to_string(&record).expect("JSON"));
    Measurement {
        op,
        rows,
        repetitions,
        median_seconds: median,
        rows_per_second,
        output_rows,
        peak_rss_kib: peak_rss_kib(),
        note: note.to_owned(),
    }
}
