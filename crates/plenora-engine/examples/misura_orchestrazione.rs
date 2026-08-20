//! Fase di MISURA dell'orchestrazione — nessuna ottimizzazione, nessuna
//! modifica di comportamento.
//!
//! Stabilisce la linea di base contro cui misurare qualunque lavoro
//! sull'orchestratore: senza numeri di partenza, «piu' veloce» e' un'opinione.
//!
//! # Come e' costruita la misura
//!
//! Ogni scelta qui sotto corregge un difetto della prima versione, che
//! produceva numeri non utilizzabili per decidere:
//!
//! - **un processo isolato per carico**. Il genitore rilancia se stesso con
//!   `--carico <nome>` e raccoglie un JSON per figlio. Serve al picco di
//!   memoria: `VmHWM` e' il massimo di TUTTA la vita del processo, quindi in
//!   un processo unico il picco di un carico contaminava tutti gli altri;
//! - **RSS per carico, con baseline sottratta**, letto nel figlio subito dopo
//!   la fase cronometrata e prima di determinismo e prove sotto pressione;
//! - **metriche di OGNI ripetizione**, non solo dell'ultima. La prima versione
//!   sovrascriveva le metriche a ogni giro e poi le chiamava «cumulate su 7
//!   ripetizioni»: erano un campione solo, e non identificato. Qui si
//!   conservano tutte e si riportano mediana e intervallo, anche per nodo e
//!   per il tetto di parallelizzazione;
//! - **determinismo sui BYTE**: si confrontano direttamente i byte della
//!   serializzazione IPC, non un hash. Un FNV a 64 bit puo' collidere, e due
//!   output vuoti davano entrambi zero — cioe' «identici» senza guardarli;
//! - **ripetizioni a BLOCCHI** finche' il wall cronometrato raggiunge davvero
//!   [`SOGLIA_CUMULATIVA`], non finche' una stima fatta una volta dice che
//!   dovrebbe bastare; il JSON dichiara `soglia_raggiunta` e `max_raggiunto`;
//! - **timing e memoria separati**. La memoria ha un processo dedicato, UNA
//!   sola esecuzione misurata e `VmHWM` azzerato via `/proc/self/clear_refs`
//!   con verifica dell'azzeramento: cosi' RSS e governor descrivono lo stesso
//!   evento. Senza, il picco includeva warm-up, decine di esecuzioni
//!   consecutive e la retention dell'allocatore;
//! - **tre campagne temporali indipendenti** per carico: il fattore di
//!   parallelismo si riporta come mediana e intervallo fra processi. La
//!   quantizzazione del contatore (`risoluzione_tick_pct`) NON e'
//!   l'incertezza della misura, ed e' nominata per quello che e';
//! - **CPU e wall delimitati sullo STESSO intervallo**. Gli `Inputs` di tutte
//!   le ripetizioni sono costruiti PRIMA, cosi' i due contatori partono e si
//!   fermano sugli stessi confini;
//! - **`CLK_TCK` letto, non assunto**: da `getconf`. Se non e' leggibile, il
//!   parallelismo effettivo e' dichiarato non misurabile invece di essere
//!   calcolato su una costante sperata;
//! - **un solo run** produce sia il JSON grezzo sia il testo: documento e dati
//!   non possono divergere.
//!
//! # Che cosa NON misura
//!
//! Il parallelismo fra nodi del DAG, perche' non esiste: l'esecuzione e'
//! seriale per costruzione (`SerialFused`). I carichi con rami servono a
//! quantificare il TETTO di quel guadagno mancante.
//!
//! Uso: `cargo run --release --example misura_orchestrazione [-- --json FILE]`

// I cast a `f64` sono TUTTI per la stampa di misure diagnostiche
// (millisecondi, MiB, rapporti). I cast di `usize` verso `i64` generano
// l'input sintetico da costanti note.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::similar_names
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use plenora_core::arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::DataContract;
use plenora_engine::executor::{execute, Input, Inputs};
use plenora_engine::planner::validate;
use plenora_engine::prepare::RuntimeContext;
use serde_json::{json, Value};

/// Input sintetico: 24 batch x 8192 righe.
const RIGHE_PER_BATCH: usize = 8_192;
const BATCH: usize = 24;

/// Lavoro cronometrato minimo per carico. Sotto questa soglia un tick del
/// contatore di CPU (~10 ms) e' una frazione non trascurabile della misura, e
/// il rapporto CPU/wall non e' affidabile.
const SOGLIA_CUMULATIVA: Duration = Duration::from_millis(1500);
/// Tetto delle ripetizioni: evita che un carico microscopico faccia girare
/// l'harness all'infinito. Se viene raggiunto, il JSON lo dichiara.
const RIPETIZIONI_MAX: usize = 2_000;
/// Esecuzioni scartate prima di cronometrare (cache, allocatore, pool).
const WARMUP: usize = 2;
/// Esecuzioni confrontate byte a byte per il determinismo.
const RIPETIZIONI_DETERMINISMO: usize = 5;
/// Ripetizioni per blocco del ciclo adattivo: si continua a blocchi finche' il
/// wall CRONOMETRATO raggiunge la soglia, invece di stimare una volta sola
/// quante ne servono e fermarsi li'.
const BLOCCO: usize = 8;
/// Campagne temporali indipendenti per carico, ciascuna in un processo suo.
/// Una sola campagna da' un numero; tre danno un intervallo, che e' l'unica
/// forma in cui un fattore di parallelismo e' dichiarabile stabile.
const PROCESSI_TEMPO: usize = 3;

// ---------------------------------------------------------------------------
// Osservazione del processo
// ---------------------------------------------------------------------------

/// Tick del contatore di CPU al secondo, letto da `getconf`.
///
/// `None` se non e' leggibile: il parallelismo effettivo viene allora
/// dichiarato non misurabile. Assumere 100 e' vero su Linux corrente ma resta
/// un'assunzione non verificata, e una misura che poggia su un'assunzione
/// taciuta non e' una misura.
fn clk_tck() -> Option<f64> {
    let uscita = Command::new("getconf").arg("CLK_TCK").output().ok()?;
    if !uscita.status.success() {
        return None;
    }
    String::from_utf8(uscita.stdout).ok()?.trim().parse().ok()
}

/// Tempo di CPU del processo (utente + sistema), in tick.
fn cpu_tick() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Il nome del comando puo' contenere spazi ed e' fra parentesi: si taglia
    // dall'ultima parentesi chiusa, poi utime e stime sono i campi 12 e 13.
    let coda = stat.rsplit_once(')').map(|(_, coda)| coda)?;
    let campi: Vec<&str> = coda.split_whitespace().collect();
    let utime: u64 = campi.get(11)?.parse().ok()?;
    let stime: u64 = campi.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// Un campo di `/proc/self/status`, in byte.
fn campo_status(nome: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let riga = status.lines().find(|r| r.starts_with(nome))?;
    let kb: u64 = riga.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

// ---------------------------------------------------------------------------
// Dati sintetici
// ---------------------------------------------------------------------------

fn schema_sintetico() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("gruppo", DataType::Int64, false),
        Field::new("valore", DataType::Float64, true),
        Field::new("etichetta", DataType::Utf8, true),
    ]))
}

fn batch_sintetico(indice: usize) -> RecordBatch {
    let base = (indice * RIGHE_PER_BATCH) as i64;
    let id: Vec<i64> = (0..RIGHE_PER_BATCH as i64).map(|r| base + r).collect();
    let gruppo: Vec<i64> = id.iter().map(|v| v % 64).collect();
    let valore: Vec<Option<f64>> = id
        .iter()
        .map(|v| {
            if v % 17 == 0 {
                None
            } else {
                Some((*v as f64) * 0.5)
            }
        })
        .collect();
    let etichetta: Vec<Option<String>> = id
        .iter()
        .map(|v| Some(format!("riga-{:08}", v % 1000)))
        .collect();
    RecordBatch::try_new(
        schema_sintetico(),
        vec![
            Arc::new(Int64Array::from(id)),
            Arc::new(Int64Array::from(gruppo)),
            Arc::new(Float64Array::from(valore)),
            Arc::new(StringArray::from(etichetta)),
        ],
    )
    .expect("batch sintetico")
}

fn ingresso() -> Vec<RecordBatch> {
    (0..BATCH).map(batch_sintetico).collect()
}

fn byte_ingresso(batches: &[RecordBatch]) -> u64 {
    batches
        .iter()
        .map(|b| {
            b.columns()
                .iter()
                .map(|c| c.get_array_memory_size() as u64)
                .sum::<u64>()
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Carichi
// ---------------------------------------------------------------------------

/// Raggruppamento dei nodi per il tetto di parallelizzazione: due rami
/// indipendenti e i nodi di convergenza.
type Rami = (
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

struct Carico {
    nome: &'static str,
    perche: &'static str,
    piano: Value,
    rami: Option<Rami>,
}

fn carichi() -> Vec<Carico> {
    vec![
        Carico {
            nome: "streaming_lineare",
            perche: "catena di kernel streaming: nessuna materializzazione",
            piano: json!({
                "schema_version": 4,
                "inputs": ["main"],
                // `formula` PRIMA di `filter`: dopo un nodo che cambia
                // cardinalita' l'arco non porta provenance row-level e il
                // planner la rifiuta. Vincolo di contratto, non del carico.
                "nodes": [
                    {"id": "c", "op": "table.formula", "in": ["main"],
                     "config": {"new_column": "doppio", "formula": "valore * 2"}},
                    {"id": "s", "op": "table.string_pad", "in": ["c"],
                     "config": {"column": "etichetta", "width": 20, "side": "left", "fill_char": "0"}},
                    {"id": "f", "op": "table.filter", "in": ["s"],
                     "config": {"column": "valore", "operator": ">", "value": "10"}}
                ],
                "output": "f"
            }),
            rami: None,
        },
        Carico {
            nome: "blocking_sort",
            perche: "materializza l'intero input e ordina",
            piano: json!({
                "schema_version": 4,
                "inputs": ["main"],
                "nodes": [
                    {"id": "o", "op": "table.sort", "in": ["main"],
                     "config": {"columns": ["valore"], "ascending": true}}
                ],
                "output": "o"
            }),
            rami: None,
        },
        Carico {
            nome: "blocking_aggregate",
            perche: "raggruppa e aggrega: materializzazione piu' hashing",
            piano: json!({
                "schema_version": 4,
                "inputs": ["main"],
                "nodes": [
                    {"id": "a", "op": "table.aggregate", "in": ["main"],
                     "config": {"group_by": ["gruppo"],
                                "aggregations": [{"column": "valore", "function": "sum"},
                                                 {"column": "id", "function": "count"}]}}
                ],
                "output": "a"
            }),
            rami: None,
        },
        Carico {
            nome: "fan_out_tee",
            perche: "un input, due consumatori: costo del tee (D9/V10)",
            piano: json!({
                "schema_version": 4,
                "inputs": ["main"],
                "nodes": [
                    {"id": "ramo_a", "op": "table.filter", "in": ["main"],
                     "config": {"column": "gruppo", "operator": "<", "value": "32"}},
                    {"id": "ramo_b", "op": "table.filter", "in": ["main"],
                     "config": {"column": "gruppo", "operator": ">=", "value": "32"}},
                    {"id": "unione", "op": "table.concat", "in": ["ramo_a", "ramo_b"],
                     "config": {}}
                ],
                "output": "unione"
            }),
            rami: Some((&["ramo_a"], &["ramo_b"], &["unione"])),
        },
        Carico {
            nome: "rami_indipendenti",
            perche: "due catene che non si toccano fino alla fine: il TETTO del \
                     guadagno che il parallelismo fra nodi potrebbe dare",
            piano: json!({
                "schema_version": 4,
                "inputs": ["main"],
                "nodes": [
                    {"id": "a1", "op": "table.sort", "in": ["main"],
                     "config": {"columns": ["valore"], "ascending": true}},
                    {"id": "a2", "op": "table.distinct", "in": ["a1"], "config": {}},
                    // Raggruppato su `id`: con 64 gruppi il join superava
                    // `max_expansion_factor`, e un carico che non gira non
                    // misura niente.
                    {"id": "b1", "op": "table.aggregate", "in": ["main"],
                     "config": {"group_by": ["id"],
                                "aggregations": [{"column": "valore", "function": "mean"}]}},
                    {"id": "b2", "op": "table.sort", "in": ["b1"],
                     "config": {"columns": ["id"], "ascending": false}},
                    {"id": "fine", "op": "table.join", "in": ["a2", "b2"],
                     "config": {"left_keys": ["id"], "right_keys": ["id"], "how": "inner"}}
                ],
                "output": "fine"
            }),
            rami: Some((&["a1", "a2"], &["b1", "b2"], &["fine"])),
        },
    ]
}

// ---------------------------------------------------------------------------
// Statistica
// ---------------------------------------------------------------------------

/// Mediana, minimo e massimo di una serie non vuota.
fn statistica(valori: &[u64]) -> (u64, u64, u64) {
    assert!(!valori.is_empty(), "statistica su una serie vuota");
    let mut ordinati = valori.to_vec();
    ordinati.sort_unstable();
    (
        ordinati[ordinati.len() / 2],
        ordinati[0],
        ordinati[ordinati.len() - 1],
    )
}

fn mediana_f64(valori: &[f64]) -> (f64, f64, f64) {
    assert!(!valori.is_empty(), "statistica su una serie vuota");
    let mut ordinati = valori.to_vec();
    ordinati.sort_by(f64::total_cmp);
    (
        ordinati[ordinati.len() / 2],
        ordinati[0],
        ordinati[ordinati.len() - 1],
    )
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1_000_000.0
}

fn mib(byte: u64) -> f64 {
    byte as f64 / (1024.0 * 1024.0)
}

// ---------------------------------------------------------------------------
// Ambiente di esecuzione condiviso dalle fasi
// ---------------------------------------------------------------------------

struct Banco {
    batches: Vec<RecordBatch>,
    contratto: DataContract,
    grafo: plenora_engine::planner::ValidatedGraph,
    contesto: RuntimeContext,
}

impl Banco {
    fn nuovo(carico: &Carico) -> Self {
        let contratto = DataContract::tabular(schema_sintetico());
        let contratti = [("main".to_owned(), contratto.clone())];
        let grafo = validate(&carico.piano.to_string(), &contratti).expect("piano valido");
        Self {
            batches: ingresso(),
            contratto,
            grafo,
            contesto: RuntimeContext::default(),
        }
    }

    fn inputs(&self) -> Inputs {
        Inputs::strict()
            .with_contract(
                "main",
                Input::from_batches(self.batches.clone()).expect("input"),
                self.contratto.clone(),
            )
            .expect("inputs")
    }

    fn esegui(&self) -> (Vec<RecordBatch>, plenora_engine::ExecutionMetrics) {
        let uscita =
            execute(&self.grafo, self.inputs(), self.contesto.clone()).expect("esecuzione");
        uscita.collect_batches().expect("raccolta")
    }
}

/// Serializza i batch in IPC: e' il confronto di determinismo, byte a byte.
fn byte_ipc(batches: &[RecordBatch]) -> Vec<u8> {
    let Some(primo) = batches.first() else {
        return Vec::new();
    };
    let mut buffer = Vec::new();
    {
        let mut scrittore =
            plenora_core::arrow::ipc::writer::StreamWriter::try_new(&mut buffer, &primo.schema())
                .expect("writer IPC");
        for batch in batches {
            scrittore.write(batch).expect("scrittura IPC");
        }
        scrittore.finish().expect("chiusura IPC");
    }
    buffer
}

// ---------------------------------------------------------------------------
// Fase TEMPO: ripetizioni a blocchi fino alla soglia effettiva
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_lines)] // una misura per volta, in ordine di esecuzione
fn fase_tempo(carico: &Carico) -> Value {
    let banco = Banco::nuovo(carico);

    // Warm-up: scartato, scalda cache e allocatore.
    for _ in 0..WARMUP {
        drop(banco.esegui());
    }

    let mut tempi: Vec<u64> = Vec::new();
    let mut per_nodo: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut picchi: Vec<u64> = Vec::new();
    let mut ultima = None;

    // Ciclo ADATTIVO a blocchi: si continua finche' il wall CRONOMETRATO
    // raggiunge davvero la soglia, non finche' una stima fatta una volta dice
    // che dovrebbe bastare. La versione precedente stimava le ripetizioni da
    // una sonda e si fermava li': quattro carichi su cinque restavano sotto
    // la soglia che il documento dichiarava.
    let mut wall_totale = Duration::ZERO;
    let mut cpu_totale_tick: u64 = 0;
    let mut soglia_raggiunta = false;
    let mut max_raggiunto = false;

    while !soglia_raggiunta && !max_raggiunto {
        let rimanenti = RIPETIZIONI_MAX - tempi.len();
        let blocco = BLOCCO.min(rimanenti);
        if blocco == 0 {
            max_raggiunto = true;
            break;
        }
        // Gli `Inputs` del blocco sono costruiti PRIMA: CPU e wall coprono lo
        // stesso intervallo, e nessuno dei due include la preparazione.
        let preparati: Vec<Inputs> = (0..blocco).map(|_| banco.inputs()).collect();

        let cpu_prima = cpu_tick();
        let wall_prima = Instant::now();
        for inputs in preparati {
            let inizio = Instant::now();
            let uscita = execute(&banco.grafo, inputs, banco.contesto.clone()).expect("esecuzione");
            let (prodotti, metriche) = uscita.collect_batches().expect("raccolta");
            tempi.push(u64::try_from(inizio.elapsed().as_nanos()).unwrap_or(u64::MAX));
            for (id, nodo) in &metriche.nodes {
                per_nodo
                    .entry(id.clone())
                    .or_default()
                    .push(u64::try_from(nodo.wall_time.as_nanos()).unwrap_or(u64::MAX));
            }
            picchi.push(metriche.memory.peak_reserved_bytes);
            ultima = Some((prodotti, metriche));
        }
        wall_totale += wall_prima.elapsed();
        if let (Some(a), Some(b)) = (cpu_prima, cpu_tick()) {
            cpu_totale_tick += b.saturating_sub(a);
        }

        soglia_raggiunta = wall_totale >= SOGLIA_CUMULATIVA;
        max_raggiunto = tempi.len() >= RIPETIZIONI_MAX;
    }

    let (prodotti, metriche) = ultima.expect("almeno una ripetizione");

    // Fail-closed: ogni nodo deve avere ESATTAMENTE un campione per
    // ripetizione. Un nodo con meno campioni renderebbe il tetto dei rami una
    // somma su indici disallineati, e nessuno se ne accorgerebbe.
    for (id, serie) in &per_nodo {
        assert_eq!(
            serie.len(),
            tempi.len(),
            "nodo {id}: {} campioni per {} ripetizioni",
            serie.len(),
            tempi.len()
        );
    }

    // Determinismo: byte IPC identici, confrontati direttamente.
    let riferimento = byte_ipc(&prodotti);
    let mut identici = true;
    for _ in 0..RIPETIZIONI_DETERMINISMO {
        let (altri, _) = banco.esegui();
        identici &= byte_ipc(&altri) == riferimento;
    }

    // Sotto pressione: budget pari alla meta' del picco governato mediano.
    let budget = (statistica(&picchi).0 / 2).max(1_048_576);
    let mut stretto = carico.piano.clone();
    stretto["limits"] = json!({"max_memory_bytes": budget});
    let contratti = [("main".to_owned(), banco.contratto.clone())];
    let pressione = match validate(&stretto.to_string(), &contratti) {
        Err(errore) => {
            json!({"esito": "rifiutato in validazione", "dettaglio": errore.to_string()})
        }
        Ok(grafo_stretto) => match execute(&grafo_stretto, banco.inputs(), banco.contesto.clone())
            .and_then(plenora_engine::Output::collect_batches)
        {
            Ok((_, m)) => json!({
                "esito": "riuscito",
                "picco_governato": m.memory.peak_reserved_bytes,
                "spill_file": m.spill.files,
                "spill_scritti": m.spill.bytes_written,
            }),
            Err(errore) => json!({
                "esito": format!("{:?}", errore.category()),
                "dettaglio": errore.to_string(),
            }),
        },
    };

    let (mediana_t, min_t, max_t) = statistica(&tempi);
    let nodi: serde_json::Map<String, Value> = per_nodo
        .iter()
        .map(|(id, serie)| {
            let (mediana, minimo, massimo) = statistica(serie);
            (
                id.clone(),
                json!({
                    "operazione": metriche.nodes.get(id).map_or("", |n| n.operation.as_str()),
                    "mediana_ns": mediana, "min_ns": minimo, "max_ns": massimo,
                    "campioni": serie.len(),
                    "righe_in": metriche.nodes.get(id).map_or(0, |n| n.rows_in),
                    "righe_out": metriche.nodes.get(id).map_or(0, |n| n.rows_out),
                }),
            )
        })
        .collect();

    // Tetto di parallelizzazione su OGNI ripetizione. Le somme usano lookup
    // FAIL-CLOSED: un id mancante e' un errore, non un addendo saltato in
    // silenzio.
    let tetto = carico.rami.map(|(a, b, convergenza)| {
        let somma = |gruppo: &[&str], indice: usize| -> u64 {
            gruppo
                .iter()
                .map(|id| {
                    let serie = per_nodo
                        .get(*id)
                        .unwrap_or_else(|| panic!("nodo {id} assente dalle metriche"));
                    *serie
                        .get(indice)
                        .unwrap_or_else(|| panic!("nodo {id}: campione {indice} mancante"))
                })
                .sum()
        };
        let guadagni: Vec<u64> = (0..tempi.len())
            .map(|i| {
                let (ramo_a, ramo_b, conv) = (somma(a, i), somma(b, i), somma(convergenza, i));
                let seriale = ramo_a + ramo_b + conv;
                assert!(seriale > 0, "ripetizione {i}: tempo di ramo nullo");
                let ideale = ramo_a.max(ramo_b) + conv;
                10_000 - (ideale * 10_000) / seriale
            })
            .collect();
        let (mediana, minimo, massimo) = statistica(&guadagni);
        json!({
            "guadagno_mediano_pct": mediana as f64 / 100.0,
            "guadagno_min_pct": minimo as f64 / 100.0,
            "guadagno_max_pct": massimo as f64 / 100.0,
            "campioni": guadagni.len(),
        })
    });

    let parallelismo = match clk_tck() {
        Some(tck) if tck > 0.0 && cpu_totale_tick > 0 => {
            let cpu_s = cpu_totale_tick as f64 / tck;
            json!({
                "misurabile": true,
                "fattore": cpu_s / wall_totale.as_secs_f64().max(f64::EPSILON),
                "cpu_ms": cpu_s * 1000.0,
                "wall_ms": wall_totale.as_secs_f64() * 1000.0,
                "clk_tck": tck,
                "tick_consumati": cpu_totale_tick,
                // NON e' l'incertezza della misura: e' la sola quantizzazione
                // del contatore di CPU su questo intervallo. La ripetibilita'
                // si legge dal range fra processi indipendenti.
                "risoluzione_tick_pct": (1.0 / cpu_totale_tick as f64) * 100.0,
            })
        }
        _ => json!({"misurabile": false}),
    };

    json!({
        "carico": carico.nome,
        "ripetizioni": tempi.len(),
        "soglia_raggiunta": soglia_raggiunta,
        "max_raggiunto": max_raggiunto,
        "tempo": {"mediana_ns": mediana_t, "min_ns": min_t, "max_ns": max_t},
        "wall_cumulato_ms": wall_totale.as_secs_f64() * 1000.0,
        "parallelismo": parallelismo,
        "byte": {
            "input": byte_ingresso(&banco.batches),
            "attraversati_dai_nodi": metriche.nodes.values().map(|n| n.bytes_in).sum::<u64>(),
        },
        "picco_governato_mediano": statistica(&picchi).0,
        "spill": {"file": metriche.spill.files, "scritti": metriche.spill.bytes_written},
        "righe": {
            "uscita": prodotti.iter().map(|b| b.num_rows() as u64).sum::<u64>(),
            "processate": metriche.total_rows_processed,
        },
        "nodi": nodi,
        "tetto_parallelizzazione": tetto,
        "determinismo": {
            "byte_identici": identici,
            "byte_confrontati": riferimento.len(),
            "esecuzioni": RIPETIZIONI_DETERMINISMO + 1,
        },
        "sotto_pressione": pressione,
        "contatori_saturati": metriche.counters_saturated,
    })
}

// ---------------------------------------------------------------------------
// Fase MEMORIA: processo dedicato, UNA esecuzione misurata
// ---------------------------------------------------------------------------

/// Azzera il picco di RSS del processo e verifica che sia avvenuto.
///
/// `/proc/self/clear_refs` con valore 5 azzera `VmHWM`. Se la scrittura non e'
/// possibile — kernel senza `CONFIG_PROC_PAGE_MONITOR`, permessi, filesystem
/// in sola lettura — oppure se `VmHWM` resta sopra `VmRSS`, la misura viene
/// dichiarata **non disponibile**: un picco non azzerato include tutto cio'
/// che il processo ha fatto prima, e confrontarlo col governor di una singola
/// esecuzione sarebbe la stessa contaminazione di prima con un nome nuovo.
fn azzera_picco_rss() -> Result<(), String> {
    // Si legge lo stato PRIMA della scrittura: `clear_refs` porta VmHWM
    // all'occupazione dell'istante in cui viene scritto, non a quella
    // dell'istante in cui la si rilegge. Se fra i due momenti l'allocatore
    // restituisce pagine al sistema, VmRSS scende sotto VmHWM pur essendo
    // l'azzeramento perfettamente riuscito: confrontare solo con il VmRSS
    // successivo produce un falso negativo.
    let picco_prima = campo_status("VmHWM:").ok_or("VmHWM non leggibile")?;
    let rss_prima = campo_status("VmRSS:").ok_or("VmRSS non leggibile")?;
    std::fs::write("/proc/self/clear_refs", "5")
        .map_err(|errore| format!("clear_refs non scrivibile: {errore}"))?;
    let picco_dopo = campo_status("VmHWM:").ok_or("VmHWM non leggibile")?;
    let rss_dopo = campo_status("VmRSS:").ok_or("VmRSS non leggibile")?;
    // L'azzeramento e' riuscito se il picco residuo non supera l'occupazione
    // osservata a cavallo della scrittura, a meno di una pagina di
    // granularita'. Se `clear_refs` non ha avuto effetto, `picco_dopo` resta
    // il massimo di tutta la vita del processo e la disuguaglianza fallisce.
    let riferimento = rss_prima.max(rss_dopo);
    if picco_dopo > riferimento.saturating_add(1024 * 1024) {
        return Err(format!(
            "azzeramento non riuscito: VmHWM {picco_prima} -> {picco_dopo}              resta sopra VmRSS {rss_prima} -> {rss_dopo}"
        ));
    }
    Ok(())
}

/// Una singola esecuzione misurata, con `VmHWM` azzerato immediatamente
/// prima: RSS e governor descrivono cosi' lo STESSO evento.
fn misura_una(banco: &Banco) -> Result<Value, String> {
    azzera_picco_rss()?;
    let baseline = campo_status("VmRSS:").ok_or("VmRSS non leggibile")?;
    let (_, metriche) = banco.esegui();
    let picco = campo_status("VmHWM:").ok_or("VmHWM non leggibile")?;
    let governato = metriche.memory.peak_reserved_bytes;
    let incremento = picco.saturating_sub(baseline);
    Ok(json!({
        "rss_baseline": baseline,
        "rss_picco": picco,
        "rss_incremento": incremento,
        "picco_governato": governato,
        "rapporto": incremento as f64 / governato.max(1) as f64,
    }))
}

/// Memoria: processo dedicato, UNA esecuzione misurata per regime.
///
/// # Perche' DUE regimi e non uno
///
/// Con il warm-up prima della misura l'allocatore ha gia' le pagine che
/// serviranno, quindi l'incremento di RSS della singola esecuzione tende a
/// zero: e' un **limite inferiore** del fabbisogno, non il fabbisogno. Senza
/// warm-up l'unica esecuzione paga anche l'inizializzazione irripetibile ed e'
/// un **limite superiore**.
///
/// Nessuno dei due, da solo, e' «il picco». Si riportano entrambi e il
/// fabbisogno reale sta in mezzo: dirlo e' piu' utile che sceglierne uno e
/// spacciarlo per la misura.
fn fase_memoria(carico: &Carico) -> Value {
    let banco = Banco::nuovo(carico);

    // A FREDDO: nessun warm-up prima. Sovrastima.
    let a_freddo = misura_una(&banco);

    // A CALDO: dopo il warm-up. Sottostima.
    for _ in 0..WARMUP {
        drop(banco.esegui());
    }
    let a_caldo = misura_una(&banco);

    match (a_freddo, a_caldo) {
        (Ok(freddo), Ok(caldo)) => json!({
            "carico": carico.nome,
            "disponibile": true,
            "esecuzioni_misurate_per_regime": 1,
            "a_freddo": freddo,
            "a_caldo": caldo,
        }),
        (Err(motivo), _) | (_, Err(motivo)) => json!({
            "carico": carico.nome,
            "disponibile": false,
            "motivo": motivo,
        }),
    }
}

// ---------------------------------------------------------------------------
// Genitore
// ---------------------------------------------------------------------------

fn figlio(eseguibile: &std::path::Path, carico: &str, fase: &str) -> Value {
    let uscita = Command::new(eseguibile)
        .args(["--carico", carico, "--fase", fase])
        .output()
        .expect("avvio del figlio");
    assert!(
        uscita.status.success(),
        "carico {carico} fase {fase} fallita: {}",
        String::from_utf8_lossy(&uscita.stderr)
    );
    serde_json::from_slice(&uscita.stdout).expect("json valido")
}

/// Tabella Markdown generata dal programma: il documento non puo' divergere
/// dai dati perche' non c'e' trascrizione a mano in mezzo.
fn tabella_markdown(risultati: &[Value]) -> String {
    let mut testo = String::new();
    testo.push_str("| carico | tempo mediano | ripetizioni | soglia 1,5 s | parallelismo (mediana su 3 processi) | range |\n");
    testo.push_str("|---|---|---|---|---|---|\n");
    for r in risultati {
        let p = &r["parallelismo_fra_processi"];
        let _ = writeln!(
            testo,
            "| `{}` | {:.2} ms | {} | {} | **{:.2}x** | {:.2}–{:.2} |",
            r["carico"].as_str().unwrap_or("?"),
            ms(r["tempo"]["mediana_ns"].as_u64().unwrap_or(0)),
            r["ripetizioni"].as_u64().unwrap_or(0),
            if r["soglia_raggiunta"].as_bool().unwrap_or(false) {
                "si"
            } else {
                "**NO**"
            },
            p["mediana"].as_f64().unwrap_or(0.0),
            p["min"].as_f64().unwrap_or(0.0),
            p["max"].as_f64().unwrap_or(0.0),
        );
    }
    testo.push_str(
        "\n| carico | picco governato | RSS a freddo (max) | RSS a caldo (min) | rapporto a freddo |\n|---|---|---|---|---|\n",
    );
    for r in risultati {
        let m = &r["memoria"];
        if m["disponibile"].as_bool().unwrap_or(false) {
            let _ = writeln!(
                testo,
                "| `{}` | {:.2} MiB | {:.2} MiB | {:.2} MiB | {:.2}x |",
                r["carico"].as_str().unwrap_or("?"),
                mib(m["a_freddo"]["picco_governato"].as_u64().unwrap_or(0)),
                mib(m["a_freddo"]["rss_incremento"].as_u64().unwrap_or(0)),
                mib(m["a_caldo"]["rss_incremento"].as_u64().unwrap_or(0)),
                m["a_freddo"]["rapporto"].as_f64().unwrap_or(0.0),
            );
        } else {
            let _ = writeln!(
                testo,
                "| `{}` | — | — | — | **non disponibile**: {} |",
                r["carico"].as_str().unwrap_or("?"),
                m["motivo"].as_str().unwrap_or("?")
            );
        }
    }
    testo
}

#[allow(clippy::too_many_lines)] // riepilogo lineare: la lunghezza e' nel numero di voci
fn main() {
    let argomenti: Vec<String> = std::env::args().collect();
    let valore = |chiave: &str| -> Option<String> {
        argomenti
            .iter()
            .position(|a| a == chiave)
            .and_then(|i| argomenti.get(i + 1))
            .cloned()
    };

    // Figlio: un carico, una fase, un JSON su stdout.
    if let Some(nome) = valore("--carico") {
        let elenco = carichi();
        let carico = elenco
            .iter()
            .find(|c| c.nome == nome)
            .unwrap_or_else(|| panic!("carico sconosciuto: {nome}"));
        let fase = valore("--fase").unwrap_or_else(|| "tempo".to_owned());
        let risultato = match fase.as_str() {
            "tempo" => fase_tempo(carico),
            "memoria" => fase_memoria(carico),
            altro => panic!("fase sconosciuta: {altro}"),
        };
        println!("{risultato}");
        return;
    }

    // Genitore: per ogni carico, PROCESSI_TEMPO campagne temporali isolate
    // piu' una campagna di memoria dedicata.
    let eseguibile = std::env::current_exe().expect("eseguibile");
    let mut risultati = Vec::new();
    for carico in carichi() {
        let campagne: Vec<Value> = (0..PROCESSI_TEMPO)
            .map(|_| figlio(&eseguibile, carico.nome, "tempo"))
            .collect();
        let fattori: Vec<f64> = campagne
            .iter()
            .filter_map(|c| c["parallelismo"]["fattore"].as_f64())
            .collect();
        let (mediana_p, min_p, max_p) = if fattori.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            mediana_f64(&fattori)
        };
        // La campagna di riferimento e' quella con il wall mediano: una sola,
        // identificata, non un miscuglio.
        let mut ordinate: Vec<&Value> = campagne.iter().collect();
        ordinate.sort_by_key(|c| c["tempo"]["mediana_ns"].as_u64().unwrap_or(0));
        let riferimento = ordinate[ordinate.len() / 2].clone();

        let mut voce = riferimento;
        voce["perche"] = json!(carico.perche);
        voce["campagne_tempo"] = json!(PROCESSI_TEMPO);
        voce["parallelismo_fra_processi"] = json!({
            "mediana": mediana_p, "min": min_p, "max": max_p, "campioni": fattori.len(),
        });
        voce["memoria"] = figlio(&eseguibile, carico.nome, "memoria");
        risultati.push(voce);
    }

    let tabella = tabella_markdown(&risultati);
    println!("# Misura dell'orchestrazione — linea di base\n");
    println!(
        "input: {BATCH} batch x {RIGHE_PER_BATCH} righe = {} righe, {:.2} MiB Arrow; \
         {} core logici",
        BATCH * RIGHE_PER_BATCH,
        mib(risultati[0]["byte"]["input"].as_u64().unwrap_or(0)),
        std::thread::available_parallelism().map_or(0, std::num::NonZeroUsize::get)
    );
    println!(
        "{PROCESSI_TEMPO} campagne temporali isolate per carico, piu' un processo dedicato \
         alla memoria con VmHWM azzerato\n"
    );
    println!("{tabella}");

    for r in &risultati {
        println!("\n## {}", r["carico"].as_str().unwrap_or("?"));
        println!("   {}", r["perche"].as_str().unwrap_or(""));
        let d = &r["determinismo"];
        println!(
            "   determinismo: {} su {} esecuzioni, {} byte IPC",
            if d["byte_identici"].as_bool().unwrap_or(false) {
                "IDENTICI byte a byte"
            } else {
                "DIVERGENTI"
            },
            d["esecuzioni"].as_u64().unwrap_or(0),
            d["byte_confrontati"].as_u64().unwrap_or(0)
        );
        println!(
            "   sotto pressione: {}",
            r["sotto_pressione"]["esito"].as_str().unwrap_or("?")
        );
        let (bi, bn) = (
            r["byte"]["input"].as_u64().unwrap_or(1),
            r["byte"]["attraversati_dai_nodi"].as_u64().unwrap_or(0),
        );
        println!(
            "   byte attraversati dai nodi: {:.2}x l'input ({:.2} MiB)",
            bn as f64 / bi as f64,
            mib(bn)
        );
        println!(
            "   risoluzione del tick sulla campagna di riferimento: {:.2}%",
            r["parallelismo"]["risoluzione_tick_pct"]
                .as_f64()
                .unwrap_or(0.0)
        );
        if let Some(t) = r["tetto_parallelizzazione"].as_object() {
            println!(
                "   tetto parallelizzazione rami: mediana {:.1}%  [{:.1}..{:.1}]  su {} ripetizioni",
                t["guadagno_mediano_pct"].as_f64().unwrap_or(0.0),
                t["guadagno_min_pct"].as_f64().unwrap_or(0.0),
                t["guadagno_max_pct"].as_f64().unwrap_or(0.0),
                t["campioni"].as_u64().unwrap_or(0)
            );
        }
        println!("   profilo per nodo (mediana):");
        if let Some(nodi) = r["nodi"].as_object() {
            let mut righe: Vec<(&String, &Value)> = nodi.iter().collect();
            righe.sort_by_key(|(_, v)| std::cmp::Reverse(v["mediana_ns"].as_u64().unwrap_or(0)));
            for (id, v) in righe {
                println!(
                    "     {:>10}  {:>8.3} ms  [{:>8.3}..{:>8.3}]  {:>8} -> {:<8}  {}",
                    id,
                    ms(v["mediana_ns"].as_u64().unwrap_or(0)),
                    ms(v["min_ns"].as_u64().unwrap_or(0)),
                    ms(v["max_ns"].as_u64().unwrap_or(0)),
                    v["righe_in"].as_u64().unwrap_or(0),
                    v["righe_out"].as_u64().unwrap_or(0),
                    v["operazione"].as_str().unwrap_or("")
                );
            }
        }
    }

    if let Some(percorso) = valore("--json") {
        let documento = json!({
            "input": {"batch": BATCH, "righe_per_batch": RIGHE_PER_BATCH},
            "core_logici": std::thread::available_parallelism()
                .map_or(0, std::num::NonZeroUsize::get),
            "soglia_cumulativa_ms": u64::try_from(SOGLIA_CUMULATIVA.as_millis())
                .unwrap_or(u64::MAX),
            "warmup": WARMUP,
            "processi_tempo_per_carico": PROCESSI_TEMPO,
            "tabella_markdown": tabella,
            "carichi": risultati,
        });
        std::fs::write(
            &percorso,
            serde_json::to_string_pretty(&documento).expect("json"),
        )
        .expect("scrittura json");
        println!("\nJSON grezzo (con la tabella Markdown dentro) scritto in {percorso}");
    }
}
