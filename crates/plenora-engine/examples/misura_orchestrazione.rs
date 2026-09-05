//! Sonda di MISURA dell'orchestrazione: non ottimizza nulla e non cambia
//! comportamento.
//!
//! Stabilisce la linea di base contro cui misurare qualunque lavoro
//! sull'orchestratore: senza numeri di partenza, «piu' veloce» e' un'opinione.
//!
//! # Come e' costruita la misura
//!
//! Ogni scelta qui sotto chiude un modo di produrre numeri non
//! utilizzabili per decidere:
//!
//! - **un processo isolato per carico**. Il genitore rilancia se stesso con
//!   `--carico <nome>` e raccoglie un JSON per figlio. Serve al picco di
//!   memoria: `VmHWM` e' il massimo di TUTTA la vita del processo, quindi in
//!   un processo unico il picco di un carico contaminerebbe tutti gli altri;
//! - **RSS per carico, con baseline sottratta**, letto nel figlio subito dopo
//!   la fase cronometrata e prima di determinismo e prove sotto pressione;
//! - **metriche di OGNI ripetizione**, non solo dell'ultima. Sovrascriverle
//!   a ogni giro e chiamarle poi «cumulate su 7 ripetizioni» pubblica un
//!   campione solo, e non identificato. Qui si conservano tutte e si
//!   riportano mediana e intervallo, anche per nodo e per il tetto di
//!   parallelizzazione;
//! - **determinismo sui BYTE**: si confrontano direttamente i byte della
//!   serializzazione IPC, non un hash. Un FNV a 64 bit puo' collidere, e due
//!   output vuoti danno entrambi zero — cioe' «identici» senza guardarli;
//! - **ripetizioni a BLOCCHI** finche' il wall cronometrato raggiunge davvero
//!   [`SOGLIA_CUMULATIVA`], non finche' una stima fatta una volta dice che
//!   dovrebbe bastare; il JSON dichiara `soglia_raggiunta` e `max_raggiunto`;
//! - **timing e memoria separati**. La memoria ha un processo dedicato, UNA
//!   sola esecuzione misurata e `VmHWM` azzerato via `/proc/self/clear_refs`
//!   con verifica dell'azzeramento: cosi' RSS e governor descrivono lo stesso
//!   evento. Senza, il picco comprenderebbe warm-up, decine di esecuzioni
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

use std::collections::{BTreeMap, BTreeSet};
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

fn batch_sintetico_di(indice: usize, righe: usize) -> RecordBatch {
    let base = (indice * righe) as i64;
    let id: Vec<i64> = (0..righe as i64).map(|r| base + r).collect();
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

/// Nanosecondi di una durata, saturando invece di troncare in silenzio.
/// Il fondo scala di `u64` e' ~584 anni: la saturazione non puo' accadere in
/// questo banco, ma un troncamento tacito non e' comunque un modo accettabile
/// di produrre una misura.
fn ns(durata: Duration) -> u64 {
    u64::try_from(durata.as_nanos()).unwrap_or(u64::MAX)
}

/// Ingresso di forma arbitraria a **righe totali costanti**: serve alla
/// decomposizione, dove si varia il numero di batch tenendo fermo il lavoro
/// per riga. Cio' che cresce col numero di batch e' costo per batch, non
/// costo dei kernel.
fn ingresso_di(n_batch: usize, righe: usize) -> Vec<RecordBatch> {
    (0..n_batch)
        .map(|indice| batch_sintetico_di(indice, righe))
        .collect()
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
                "schema_version": 5,
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
                "schema_version": 5,
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
                "schema_version": 5,
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
            perche: "un input, due consumatori: costo del tee, rilascio al last consumer",
            piano: json!({
                "schema_version": 5,
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
                "schema_version": 5,
                "inputs": ["main"],
                "nodes": [
                    {"id": "a1", "op": "table.sort", "in": ["main"],
                     "config": {"columns": ["valore"], "ascending": true}},
                    {"id": "a2", "op": "table.distinct", "in": ["a1"], "config": {}},
                    // Raggruppato su `id`: con 64 gruppi il join
                    // supererebbe `max_expansion_factor`, e un carico che
                    // non gira non misura niente.
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
        Self::nuovo_con(carico, BATCH, RIGHE_PER_BATCH, true)
    }

    /// Banco di forma arbitraria, con la raccolta delle metriche per
    /// nodo/segmento accendibile o spegnibile.
    ///
    /// Spegnerle NON cambia la semantica — `MetricsConfig` e' una manopola
    /// pubblica di `RuntimeContext` — ma toglie dal percorso caldo
    /// l'accumulo per nodo e per segmento e il conteggio dei byte ai confini
    /// interni (`get_array_memory_size` per batch per nodo). La differenza
    /// fra acceso e spento e' quindi il costo dell'osservabilita' stessa.
    fn nuovo_con(carico: &Carico, n_batch: usize, righe: usize, metriche: bool) -> Self {
        let contratto = DataContract::tabular(schema_sintetico());
        let contratti = [("main".to_owned(), contratto.clone())];
        let grafo = validate(&carico.piano.to_string(), &contratti).expect("piano valido");
        let mut contesto = RuntimeContext::default();
        contesto.metrics.per_node = metriche;
        contesto.metrics.per_segment = metriche;
        Self {
            batches: ingresso_di(n_batch, righe),
            contratto,
            grafo,
            contesto,
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

    /// Come [`Banco::esegui`], ma cronometrando separatamente le due meta'
    /// dell'API pubblica. La somma dei due tempi **e'** il wall dell'
    /// esecuzione: non c'e' doppio conteggio, e' una partizione.
    ///
    /// - `execute` costruisce il piano fisico e lo stream, non tira batch;
    /// - `collect_batches` drena lo stream: qui dentro sta tutto il resto.
    fn esegui_diviso(&self, inputs: Inputs) -> Spesa {
        let inizio = Instant::now();
        let uscita = execute(&self.grafo, inputs, self.contesto.clone()).expect("esecuzione");
        let costruzione = inizio.elapsed();
        let inizio = Instant::now();
        let (batches, metriche) = uscita.collect_batches().expect("raccolta");
        let drenaggio = inizio.elapsed();
        // La distruzione dei batch di uscita sta FUORI dal cronometro: e'
        // lavoro del consumatore, non dell'esecuzione, e la fase temporale
        // non la conta. Legandoli a `_` verrebbero distrutti prima di
        // `elapsed()` e il residuo risulterebbe gonfiato di tutto il costo di
        // deallocazione dell'output.
        drop(batches);
        Spesa {
            costruzione,
            drenaggio,
            kernel: metriche.nodes.values().map(|n| n.wall_time).sum(),
            segmenti: metriche.segments.values().map(|s| s.wall_time).sum(),
        }
    }
}

/// Una esecuzione, divisa nelle parti che l'API pubblica permette di
/// cronometrare separatamente.
#[derive(Clone, Copy)]
struct Spesa {
    /// `execute()`: costruzione del piano fisico e dello stream.
    costruzione: Duration,
    /// `collect_batches()`: drenaggio dello stream.
    drenaggio: Duration,
    /// Somma dei `wall_time` per nodo — zero a metriche spente.
    kernel: Duration,
    /// Somma dei `wall_time` per segmento — zero a metriche spente.
    segmenti: Duration,
}

impl Spesa {
    fn wall(self) -> Duration {
        self.costruzione + self.drenaggio
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

/// Gli id dei nodi **dichiarati dal piano**, letti dal piano stesso.
///
/// E' il riferimento contro cui confrontare i nodi che hanno prodotto
/// metriche: senza, l'unico insieme disponibile sarebbe quello osservato, che
/// non puo' rivelare la propria incompletezza.
fn nodi_dichiarati(carico: &Carico) -> BTreeSet<String> {
    let nodi = carico.piano["nodes"]
        .as_array()
        .expect("il piano deve dichiarare `nodes`");
    let insieme: BTreeSet<String> = nodi
        .iter()
        .map(|n| {
            n["id"]
                .as_str()
                .expect("ogni nodo del piano deve avere un `id`")
                .to_owned()
        })
        .collect();
    assert_eq!(
        insieme.len(),
        nodi.len(),
        "il piano dichiara id di nodo duplicati"
    );
    insieme
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
    // che dovrebbe bastare. Stimare le ripetizioni da una sonda e fermarsi
    // li' lascia la maggior parte dei carichi sotto la soglia che il
    // documento dichiara.
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

    // Fail-closed, in due tempi.
    //
    // Primo: l'INSIEME dei nodi osservati deve essere quello dichiarato dal
    // piano. Percorrere solo i nodi presenti nella mappa lascia passare il
    // caso peggiore — un nodo che non compare in NESSUNA ripetizione non ha
    // una voce da controllare, e sparisce in silenzio dal profilo e dai
    // rami.
    let dichiarati = nodi_dichiarati(carico);
    let osservati: BTreeSet<String> = per_nodo.keys().cloned().collect();
    assert_eq!(
        osservati,
        dichiarati,
        "i nodi con metriche non sono quelli del piano: mancano {:?}, in piu' {:?}",
        dichiarati.difference(&osservati).collect::<Vec<_>>(),
        osservati.difference(&dichiarati).collect::<Vec<_>>(),
    );

    // Secondo: ogni nodo deve avere ESATTAMENTE un campione per ripetizione.
    // Un nodo con meno campioni renderebbe il tetto dei rami una somma su
    // indici disallineati, e nessuno se ne accorgerebbe.
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
    stretto["limits"] = json!({"max_governed_memory_bytes": budget});
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
// Fase DECOMPOSIZIONE: attribuire il wall che i tempi per nodo non coprono
// ---------------------------------------------------------------------------

/// Forme dell'input a **righe totali costanti** (196 608). Variare il numero
/// di batch tenendo fermo il lavoro per riga separa il costo fisso per
/// esecuzione da quello che si paga a ogni batch: e' la distinzione che
/// decide se un orchestratore parallelo possa toccare quel tempo.
const FORME: [(usize, usize); 5] = [
    (6, 32_768),
    (12, 16_384),
    (24, 8_192),
    (48, 4_096),
    (96, 2_048),
];

/// Secondo asse: **numero di batch costante**, righe totali variabili.
///
/// Serve perche' il primo asse da solo non basta. Tenendo le righe totali
/// ferme, tutto cio' che e' proporzionale alle righe si comporta come un
/// costo fisso e i due casi non sono distinguibili — e la differenza conta:
/// un costo per riga e' lavoro sui dati, un costo fisso per esecuzione e'
/// preparazione che nessun parallelismo riduce.
const FORME_RIGHE: [(usize, usize); 5] = [
    (24, 1_024),
    (24, 2_048),
    (24, 4_096),
    (24, 8_192),
    (24, 16_384),
];

/// Soglia cumulata per ogni singola cella (forma x metriche): dieci celle per
/// carico, quindi piu' bassa di quella della fase temporale.
const SOGLIA_DECOMPOSIZIONE: Duration = Duration::from_millis(700);

/// Ripetizioni minime per cella, **oltre** alla soglia di tempo.
///
/// Senza questo minimo la soglia cumulata si soddisfa anche con UNA sola
/// ripetizione, purche' abbastanza lenta — ed e' proprio la ripetizione
/// contaminata a essere lenta. Una mediana su un campione non e' una
/// mediana: una cella di `streaming_lineare` misurata su un solo campione
/// da 989 ms contro i ~20 attesi porta la regressione sull'asse delle righe
/// da R² 0,99 a 0,002.
const RIPETIZIONI_MIN: usize = 5;

/// Mediana delle parti su una campagna a soglia.
fn campagna_appaiata(acceso: &Banco, spento: &Banco) -> (Vec<Spesa>, Vec<Spesa>) {
    for _ in 0..WARMUP {
        drop(acceso.esegui());
        drop(spento.esegui());
    }
    let mut con: Vec<Spesa> = Vec::new();
    let mut senza: Vec<Spesa> = Vec::new();
    let mut cumulato = Duration::ZERO;
    // Si esce quando ENTRAMBE le condizioni sono soddisfatte: tempo cumulato
    // sopra la soglia E almeno RIPETIZIONI_MIN campioni. La sola soglia si
    // accontenta di un campione lento, che e' esattamente il campione da non
    // credere.
    while (cumulato < SOGLIA_DECOMPOSIZIONE || con.len() < RIPETIZIONI_MIN)
        && con.len() < RIPETIZIONI_MAX
    {
        // Le due configurazioni si alternano DENTRO lo stesso ciclo, e
        // l'ordine si inverte a ogni giro. Misurarle in due campagne separate
        // le espone a due stati diversi dell'host: il primo tentativo dava
        // differenze di segno casuale, cioe' sotto il rumore. Appaiare e'
        // l'unico modo per cui la differenza significhi qualcosa.
        //
        // Gli `Inputs` si costruiscono FUORI dalla finestra cronometrata:
        // clonare i batch e' lavoro del banco, non dell'engine.
        let (a, b) = if con.len().is_multiple_of(2) {
            let ia = acceso.inputs();
            let ib = spento.inputs();
            let a = acceso.esegui_diviso(ia);
            let b = spento.esegui_diviso(ib);
            (a, b)
        } else {
            let ib = spento.inputs();
            let ia = acceso.inputs();
            let b = spento.esegui_diviso(ib);
            let a = acceso.esegui_diviso(ia);
            (a, b)
        };
        cumulato += a.wall() + b.wall();
        con.push(a);
        senza.push(b);
    }
    (con, senza)
}

/// Mediana di una serie di durate estratte da una serie di spese.
fn mediana_di(spese: &[Spesa], estrai: fn(&Spesa) -> Duration) -> Duration {
    let valori: Vec<u64> = spese.iter().map(|s| ns(estrai(s))).collect();
    Duration::from_nanos(statistica(&valori).0)
}

/// Mediana di una serie di differenze **con segno**, in nanosecondi.
///
/// Il segno si conserva: una differenza negativa e' l'informazione che
/// l'effetto cercato e' sotto il rumore, e azzerarla la nasconderebbe.
fn mediana_con_segno(valori: &[i64]) -> i64 {
    assert!(!valori.is_empty(), "statistica su una serie vuota");
    let mut ordinati = valori.to_vec();
    ordinati.sort_unstable();
    ordinati[ordinati.len() / 2]
}

/// Retta ai minimi quadrati `y = intercetta + pendenza * x`.
///
/// Serve a leggere il residuo come «tanto fisso piu' tanto per batch». Con
/// meno di due punti distinti non e' definita e si restituisce `None`: mai un
/// coefficiente inventato.
fn retta(punti: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = punti.len() as f64;
    if punti.len() < 2 {
        return None;
    }
    let media_x = punti.iter().map(|p| p.0).sum::<f64>() / n;
    let media_y = punti.iter().map(|p| p.1).sum::<f64>() / n;
    let numeratore: f64 = punti
        .iter()
        .map(|p| (p.0 - media_x) * (p.1 - media_y))
        .sum();
    let denominatore: f64 = punti.iter().map(|p| (p.0 - media_x).powi(2)).sum();
    if denominatore == 0.0 {
        return None;
    }
    let pendenza = numeratore / denominatore;
    Some(((-pendenza).mul_add(media_x, media_y), pendenza))
}

/// Coefficiente di determinazione della retta: dice se «fisso + per batch»
/// e' una lettura onesta del residuo o una forzatura.
fn r_quadro(punti: &[(f64, f64)], intercetta: f64, pendenza: f64) -> f64 {
    let media_y = punti.iter().map(|p| p.1).sum::<f64>() / punti.len() as f64;
    let totale: f64 = punti.iter().map(|p| (p.1 - media_y).powi(2)).sum();
    let residua: f64 = punti
        .iter()
        .map(|p| (p.1 - pendenza.mul_add(p.0, intercetta)).powi(2))
        .sum();
    if totale == 0.0 {
        return 1.0;
    }
    1.0 - residua / totale
}

/// Terzo asse: catena di `k` nodi identici, input fisso.
///
/// `string_pad` a larghezza 20 su una colonna gia' lunga 20 e' idempotente
/// dopo il primo nodo: righe, schema e dimensione dei dati restano gli stessi
/// lungo tutta la catena. Cio' che cresce con `k` e' **solo** il numero di
/// attraversamenti di confine fra nodi. Se il residuo cresce con `k`, e'
/// lavoro ai confini; se resta piatto, e' gestione di ingresso e uscita.
fn carico_catena(k: usize) -> Carico {
    let nodi: Vec<Value> = (0..k)
        .map(|i| {
            let ingresso = if i == 0 {
                "main".to_owned()
            } else {
                format!("n{}", i - 1)
            };
            json!({
                "id": format!("n{i}"),
                "op": "table.string_pad",
                "in": [ingresso],
                "config": {
                    "column": "etichetta", "width": 20,
                    "side": "left", "fill_char": "0"
                }
            })
        })
        .collect();
    Carico {
        nome: "catena",
        perche: "catena di nodi identici: isola il costo di attraversamento",
        piano: json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": nodi,
            "output": format!("n{}", k - 1),
            "limits": {"max_governed_memory_bytes": 512 * 1024 * 1024},
        }),
        rami: None,
    }
}

/// Catena di `k` nodi `formula`, ognuno con una colonna nuova.
///
/// Se il residuo cresce proporzionalmente a `k`, e' costo **per nodo
/// formula**; se resta costante, e' costo per esecuzione che la presenza di
/// un solo nodo formula fa comparire.
fn carico_catena_formula(k: usize) -> Carico {
    let nodi: Vec<Value> = (0..k)
        .map(|i| {
            let ingresso = if i == 0 {
                "main".to_owned()
            } else {
                format!("n{}", i - 1)
            };
            json!({
                "id": format!("n{i}"),
                "op": "table.formula",
                "in": [ingresso],
                "config": {"new_column": format!("doppio{i}"), "formula": "valore * 2"}
            })
        })
        .collect();
    Carico {
        nome: "catena_formula",
        perche: "catena di nodi formula: il residuo scala col numero di nodi?",
        piano: json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": nodi,
            "output": format!("n{}", k - 1),
            "limits": {"max_governed_memory_bytes": 512 * 1024 * 1024},
        }),
        rami: None,
    }
}

/// Piano a nodo singolo, per attribuire il residuo alla singola operazione.
fn carico_singolo(nome: &'static str, nodo: &Value) -> Carico {
    Carico {
        nome,
        perche: "nodo singolo: attribuisce il residuo all'operazione",
        piano: json!({
            "schema_version": 5,
            "inputs": ["main"],
            "nodes": [nodo],
            "output": "n0",
            "limits": {"max_governed_memory_bytes": 512 * 1024 * 1024},
        }),
        rami: None,
    }
}

/// I tre nodi di `streaming_lineare`, ciascuno da solo, piu' la catena
/// completa: il confronto dice se il residuo e' dell'attraversamento o di
/// un'operazione precisa.
fn carichi_per_operazione() -> Vec<(&'static str, Carico)> {
    let formula = json!({
        "id": "n0", "op": "table.formula", "in": ["main"],
        "config": {"new_column": "doppio", "formula": "valore * 2"}
    });
    let pad = json!({
        "id": "n0", "op": "table.string_pad", "in": ["main"],
        "config": {"column": "etichetta", "width": 20, "side": "left", "fill_char": "0"}
    });
    let filtro = json!({
        "id": "n0", "op": "table.filter", "in": ["main"],
        "config": {"column": "valore", "operator": ">", "value": "10"}
    });
    // Due varianti di formula per separare il costo del CALCOLO da quello
    // strutturale della presenza del nodo: una costante non legge colonne.
    let costante = json!({
        "id": "n0", "op": "table.formula", "in": ["main"],
        "config": {"new_column": "uno", "formula": "1"}
    });
    let intero = json!({
        "id": "n0", "op": "table.formula", "in": ["main"],
        "config": {"new_column": "doppio_id", "formula": "id * 2"}
    });
    vec![
        ("solo_formula", carico_singolo("solo_formula", &formula)),
        (
            "solo_formula_costante",
            carico_singolo("solo_formula_costante", &costante),
        ),
        (
            "solo_formula_intero",
            carico_singolo("solo_formula_intero", &intero),
        ),
        ("solo_string_pad", carico_singolo("solo_string_pad", &pad)),
        ("solo_filter", carico_singolo("solo_filter", &filtro)),
    ]
}

/// Residuo di una ripetizione: `wall - (costruzione + kernel)`.
///
/// Unico punto in cui la sottrazione della partizione viene fatta, e con
/// `checked_sub`: se i tempi per nodo superassero il drenaggio la partizione
/// non sarebbe piu' una partizione, e saturare a zero lo nasconderebbe
/// producendo un residuo nullo perfettamente credibile. Meglio fermarsi.
fn residuo_di(s: &Spesa) -> Duration {
    let parti = s.costruzione + s.kernel;
    s.wall().checked_sub(parti).unwrap_or_else(|| {
        panic!(
            "partizione violata: kernel {:?} + costruzione {:?} superano il wall {:?} \
             (drenaggio {:?}). I tempi per nodo non possono eccedere il drenaggio: \
             o l'executor ha cambiato dove registra `elapsed`, o la misura e' rotta.",
            s.kernel,
            s.costruzione,
            s.wall(),
            s.drenaggio,
        )
    })
}

/// Metriche complete di UNA esecuzione, per capire dove il tempo NON e'
/// registrato: nodi e segmenti con i loro contatori, cosi' come l'engine li
/// espone. Serve alla diagnosi, non alla statistica.
fn diagnostica(banco: &Banco) -> Value {
    let (_, m) = banco.esegui();
    let nodi: Vec<Value> = m
        .nodes
        .iter()
        .map(|(id, n)| {
            json!({
                "id": id, "op": n.operation,
                "batch_in": n.batches_in, "batch_out": n.batches_out,
                "righe_in": n.rows_in, "righe_out": n.rows_out,
                "byte_in": n.bytes_in, "byte_out": n.bytes_out,
                "wall_ns": ns(n.wall_time),
            })
        })
        .collect();
    let segmenti: Vec<Value> = m
        .segments
        .iter()
        .map(|(id, s)| {
            json!({
                "id": id, "modo": format!("{:?}", s.mode),
                "batch_in": s.batches_in, "batch_out": s.batches_out,
                "righe_in": s.rows_in, "righe_out": s.rows_out,
                "wall_ns": ns(s.wall_time),
            })
        })
        .collect();
    json!({
        "nodi": nodi,
        "segmenti": segmenti,
        "righe_processate": m.total_rows_processed,
        "batch_uscita": m.output_batches,
        "righe_uscita": m.output_rows,
        "contatori_saturati": m.counters_saturated,
    })
}

/// Una cella della decomposizione: una forma dell'input, misurata appaiata.
struct Cella {
    valore: Value,
    /// Residuo mediano in nanosecondi, per le regressioni.
    residuo_ns: f64,
    /// Differenza appaiata mediana dell'osservabilita', in nanosecondi.
    osservabilita_ns: f64,
}

/// Misura una forma: campagna appaiata, partizione esatta, sottodivisione.
fn cella(carico: &Carico, n_batch: usize, righe: usize) -> Cella {
    let acceso = Banco::nuovo_con(carico, n_batch, righe, true);
    let spento = Banco::nuovo_con(carico, n_batch, righe, false);
    let (con, senza) = campagna_appaiata(&acceso, &spento);

    // A metriche spente i wall_time per nodo non esistono: e' atteso, ed e'
    // la ragione per cui il tempo dei kernel si prende dalle ripetizioni
    // accese. Se comparissero, l'assunzione sarebbe sbagliata.
    assert!(
        senza.iter().all(|s| s.kernel.is_zero()),
        "a metriche spente i tempi per nodo devono essere assenti"
    );
    // I segmenti accumulano lo STESSO `elapsed` dei nodi (executor.rs:
    // 3175/3188, 4693/4702, 4920/4929): non aggiungono copertura. Il
    // documento dichiara UGUAGLIANZA, quindi qui si verifica l'uguaglianza —
    // `<=` sarebbe passato anche se i segmenti avessero smesso di registrare,
    // cioe' proprio nel caso in cui la conclusione andrebbe rivista.
    //
    // Sono somme delle stesse `Duration` nello stesso ordine: l'uguaglianza
    // e' esatta, non approssimata.
    for s in &con {
        assert_eq!(
            s.segmenti, s.kernel,
            "somma per segmento e somma per nodo devono coincidere: \
             l'executor accumula lo stesso `elapsed` in entrambe. \
             Se divergono, o un nodo non e' registrato (vedi il controllo \
             sull'insieme dei nodi) o l'executor e' cambiato."
        );
    }

    // PARTIZIONE ESATTA, calcolata PER RIPETIZIONE:
    //   wall = costruzione + kernel + residuo
    // Le quote si mediano una per una: mediare i termini e poi dividere
    // darebbe una somma diversa da 100 senza che nulla sia sbagliato, e
    // sarebbe solo confondente.
    let residui: Vec<u64> = con.iter().map(|s| ns(residuo_di(s))).collect();
    let quote = |estrai: fn(&Spesa) -> Duration| -> f64 {
        let mut v: Vec<f64> = con.iter().map(|s| quota(estrai(s), s.wall())).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let mut quote_residuo: Vec<f64> = con.iter().map(|s| quota(residuo_di(s), s.wall())).collect();
    quote_residuo.sort_by(f64::total_cmp);

    let costruzione = mediana_di(&con, |s| s.costruzione);
    let kernel = mediana_di(&con, |s| s.kernel);
    let residuo = Duration::from_nanos(statistica(&residui).0);
    let wall = mediana_di(&con, |s: &Spesa| s.wall());

    // SOTTODIVISIONE del residuo, appaiata: quanto ne e' costo
    // dell'osservabilita' stessa. Differenza per COPPIA, poi mediana — non
    // differenza di mediane, che mescolerebbe momenti diversi dell'host.
    let coppie: Vec<i64> = con
        .iter()
        .zip(&senza)
        .map(|(a, b)| {
            i64::try_from(ns(a.drenaggio)).unwrap_or(i64::MAX)
                - i64::try_from(ns(b.drenaggio)).unwrap_or(i64::MAX)
        })
        .collect();
    let osservabilita = mediana_con_segno(&coppie);
    let positive = coppie.iter().filter(|d| **d > 0).count();

    Cella {
        valore: json!({
            "batch": n_batch,
            "righe_per_batch": righe,
            "righe_totali": n_batch * righe,
            "ripetizioni": con.len(),
            "wall_ns": ns(wall),
            "partizione_ns": {
                "costruzione": ns(costruzione),
                "kernel": ns(kernel),
                "residuo": ns(residuo),
            },
            "quote_pct": {
                "costruzione": quote(|s| s.costruzione),
                "kernel": quote(|s| s.kernel),
                "residuo": quote_residuo[quote_residuo.len() / 2],
            },
            // Sottodivisione del residuo, non un quarto termine.
            "osservabilita": {
                "mediana_appaiata_ns": osservabilita,
                "coppie": coppie.len(),
                "coppie_positive": positive,
                // Senza una maggioranza netta di differenze positive
                // l'effetto non e' distinguibile dal rumore, e va detto
                // invece di essere riportato come se fosse una misura.
                "risolta": positive * 4 >= coppie.len() * 3,
                "quota_del_residuo_pct": if residuo.is_zero() {
                    0.0
                } else {
                    100.0 * osservabilita as f64 / residuo.as_nanos() as f64
                },
            },
            "segmenti_ns": ns(mediana_di(&con, |s| s.segmenti)),
        }),
        residuo_ns: residuo.as_nanos() as f64,
        osservabilita_ns: osservabilita as f64,
    }
}

/// Regressione di una serie di punti, con il nome dell'ascissa.
fn regressione(punti: &[(f64, f64)], per_unita: &str) -> Value {
    retta(punti).map_or_else(
        || json!({"disponibile": false}),
        |(intercetta, pendenza)| {
            json!({
                "disponibile": true,
                "intercetta_ns": intercetta,
                per_unita: pendenza,
                "r_quadro": r_quadro(punti, intercetta, pendenza),
                "punti": punti.len(),
            })
        },
    )
}

fn fase_decomposizione(carico: &Carico) -> Value {
    // ASSE 1 — righe totali costanti, numero di batch variabile: isola cio'
    // che si paga a ogni batch.
    let mut celle_batch = Vec::new();
    let mut punti_residuo_batch = Vec::new();
    let mut punti_osservabilita = Vec::new();
    for (n_batch, righe) in FORME {
        let c = cella(carico, n_batch, righe);
        punti_residuo_batch.push((n_batch as f64, c.residuo_ns));
        punti_osservabilita.push((n_batch as f64, c.osservabilita_ns));
        celle_batch.push(c.valore);
    }

    // ASSE 2 — numero di batch costante, righe totali variabili: separa il
    // costo per riga da quello fisso per esecuzione, che sul primo asse sono
    // indistinguibili.
    let mut celle_righe = Vec::new();
    let mut punti_residuo_righe = Vec::new();
    for (n_batch, righe) in FORME_RIGHE {
        let c = cella(carico, n_batch, righe);
        punti_residuo_righe.push(((n_batch * righe) as f64, c.residuo_ns));
        celle_righe.push(c.valore);
    }

    json!({
        "carico": carico.nome,
        // Modi fisici dei segmenti: dicono quali nodi passano dal percorso
        // bloccante, dove la materializzazione precede il cronometro.
        "diagnostica": diagnostica(&Banco::nuovo(carico)),
        "asse_batch": {
            "descrizione": "righe totali costanti, numero di batch variabile",
            "forme": celle_batch,
            "residuo": regressione(&punti_residuo_batch, "per_batch_ns"),
            "osservabilita": regressione(&punti_osservabilita, "per_batch_ns"),
        },
        "asse_righe": {
            "descrizione": "numero di batch costante, righe totali variabili",
            "forme": celle_righe,
            "residuo": regressione(&punti_residuo_righe, "per_riga_ns"),
        },
    })
}

/// Fase CATENA: input costante, lunghezza della catena variabile.
///
/// Non dipende dal carico — costruisce piani propri — quindi e' una
/// misura a
/// se': ripeterla per ogni carico misurerebbe cinque volte la stessa cosa.
fn fase_catena() -> Value {
    let mut celle = Vec::new();
    let mut punti_residuo = Vec::new();
    let mut punti_kernel = Vec::new();
    for k in 1..=4_usize {
        let catena = carico_catena(k);
        let c = cella(&catena, BATCH, RIGHE_PER_BATCH);
        punti_residuo.push((k as f64, c.residuo_ns));
        punti_kernel.push((
            k as f64,
            c.valore["partizione_ns"]["kernel"].as_f64().unwrap_or(0.0),
        ));
        celle.push(c.valore);
    }
    // Ogni operazione della catena streaming, da sola: se il residuo si
    // concentra su una, non e' l'attraversamento a costare.
    let per_operazione: Vec<Value> = carichi_per_operazione()
        .into_iter()
        .map(|(nome, carico)| {
            let c = cella(&carico, BATCH, RIGHE_PER_BATCH);
            let banco = Banco::nuovo_con(&carico, BATCH, RIGHE_PER_BATCH, true);
            json!({
                "operazione": nome,
                "misura": c.valore,
                "diagnostica": diagnostica(&banco),
            })
        })
        .collect();

    // Stessa catena, ma di nodi `formula`: il confronto fra le due pendenze
    // dice se il residuo e' dell'attraversamento o dell'operazione.
    let mut celle_formula = Vec::new();
    let mut punti_residuo_formula = Vec::new();
    let mut punti_kernel_formula = Vec::new();
    for k in 1..=4_usize {
        let c = cella(&carico_catena_formula(k), BATCH, RIGHE_PER_BATCH);
        punti_residuo_formula.push((k as f64, c.residuo_ns));
        punti_kernel_formula.push((
            k as f64,
            c.valore["partizione_ns"]["kernel"].as_f64().unwrap_or(0.0),
        ));
        celle_formula.push(c.valore);
    }

    json!({
        "carico": "catena",
        "descrizione": "input costante (24 x 8192), catena di k nodi identici",
        "catena_string_pad": {
            "forme": celle,
            "residuo": regressione(&punti_residuo, "per_nodo_ns"),
            "kernel": regressione(&punti_kernel, "per_nodo_ns"),
        },
        "catena_formula": {
            "forme": celle_formula,
            "residuo": regressione(&punti_residuo_formula, "per_nodo_ns"),
            "kernel": regressione(&punti_kernel_formula, "per_nodo_ns"),
        },
        "per_operazione": per_operazione,
    })
}

/// Quota percentuale di una parte sul totale, zero se il totale e' nullo.
fn quota(parte: Duration, totale: Duration) -> f64 {
    if totale.is_zero() {
        return 0.0;
    }
    100.0 * parte.as_nanos() as f64 / totale.as_nanos() as f64
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
        // Se il parallelismo non e' disponibile su TUTTE le campagne, la
        // tabella lo dice: uno zero al posto di un fattore verrebbe letto
        // come una misura.
        let (fattore, intervallo) = if p["disponibile"].as_bool().unwrap_or(false) {
            (
                format!("**{:.2}x**", p["mediana"].as_f64().unwrap_or(0.0)),
                format!(
                    "{:.2}–{:.2}",
                    p["min"].as_f64().unwrap_or(0.0),
                    p["max"].as_f64().unwrap_or(0.0)
                ),
            )
        } else {
            (
                "**non disponibile**".to_owned(),
                format!(
                    "{} campagne su {}",
                    p["campioni"].as_u64().unwrap_or(0),
                    p["campagne"].as_u64().unwrap_or(0)
                ),
            )
        };
        let _ = writeln!(
            testo,
            "| `{}` | {:.2} ms | {} | {} | {fattore} | {intervallo} |",
            r["carico"].as_str().unwrap_or("?"),
            ms(r["tempo"]["mediana_ns"].as_u64().unwrap_or(0)),
            r["ripetizioni"].as_u64().unwrap_or(0),
            if r["soglia_raggiunta"].as_bool().unwrap_or(false) {
                "si"
            } else {
                "**NO**"
            },
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
            "decomposizione" => fase_decomposizione(carico),
            "catena" => fase_catena(),
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
        // Fail-closed sul parallelismo: o tutte e PROCESSI_TEMPO le campagne
        // hanno prodotto un fattore, o il dato e' dichiarato NON DISPONIBILE.
        // Con `filter_map` una campagna che dichiara il parallelismo non
        // misurabile (CLK_TCK illeggibile) sparirebbe, e si pubblicherebbe
        // un «range su 3 processi» calcolato su uno o due: la colonna
        // direbbe una cosa che non e' vera.
        let fattori: Vec<Option<f64>> = campagne
            .iter()
            .map(|c| c["parallelismo"]["fattore"].as_f64())
            .collect();
        let completi: Vec<f64> = fattori.iter().filter_map(|f| *f).collect();
        let parallelismo = if completi.len() == PROCESSI_TEMPO {
            let (mediana_p, min_p, max_p) = mediana_f64(&completi);
            json!({
                "disponibile": true,
                "mediana": mediana_p, "min": min_p, "max": max_p,
                "campioni": completi.len(), "campagne": PROCESSI_TEMPO,
            })
        } else {
            json!({
                "disponibile": false,
                "campioni": completi.len(), "campagne": PROCESSI_TEMPO,
                "motivo": format!(
                    "{} campagne su {PROCESSI_TEMPO} non hanno misurato il parallelismo",
                    PROCESSI_TEMPO - completi.len()
                ),
            })
        };
        // La campagna di riferimento e' quella con il wall mediano: una sola,
        // identificata, non un miscuglio.
        let mut ordinate: Vec<&Value> = campagne.iter().collect();
        ordinate.sort_by_key(|c| c["tempo"]["mediana_ns"].as_u64().unwrap_or(0));
        let riferimento = ordinate[ordinate.len() / 2].clone();

        let mut voce = riferimento;
        voce["perche"] = json!(carico.perche);
        voce["campagne_tempo"] = json!(PROCESSI_TEMPO);
        voce["parallelismo_fra_processi"] = parallelismo;
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
