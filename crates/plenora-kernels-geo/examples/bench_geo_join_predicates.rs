//! Benchmark mirato sul join spaziale reale — Overlaps/Crosses/Touches,
//! casi positivi e negativi, due densita' di candidati — separato da
//! `bench_geo_sweep` apposta: quello rifà tutte le 88 misure a ogni run e
//! adatta la scala ai tempi; qui la scala è FISSA (stessi N nei due bracci
//! del confronto candidato-esatto/originale) e lo scenario è quello del
//! join spaziale vero (`spatial_join_nullable_validated`, la stessa API di
//! `geo_transport::pair`), non predicati punto-contro-poligono per cella.
//!
//! # Budget interno vs esterno
//!
//! `PLENORA_BENCH_BUDGET_SECS` (default 300 = 5 minuti) limita il tempo
//! totale di ESECUZIONE (non di compilazione, che cargo misura a parte) fra
//! uno scenario e il successivo: allo scadere, gli scenari non ancora
//! misurati sono segnati `"stato":"incompleto"` invece di essere eseguiti.
//! Questo controllo interno **non** basta se un singolo scenario resta
//! bloccato (non c'e' punto di controllo dentro `spatial_join_nullable_validated`
//! stessa): il limite esterno e' il chiamante — `timeout <secondi>
//! ./bench_geo_join_predicates`, un processo diverso da quello misurato, che
//! lo termina comunque scaduto il tempo.
//!
//! # Uso
//!
//! ```text
//! cargo build --release --example bench_geo_join_predicates   # compilazione, misurata a parte
//! timeout <max_secondi> ./target/release/examples/bench_geo_join_predicates
//! ```
//!
//! Scrive `benchmarks/join/geo_join_predicates.jsonl` (una riga per
//! scenario) nella cwd — nell'albero in cui gira, che per il confronto sono
//! due alberi distinti con due cwd distinte: nessuna sovrascrittura
//! incrociata.
//!
//! # Ripetizioni
//!
//! Un riscaldamento (scartato) piu' [`RIPETIZIONI`] misure per scenario;
//! l'output porta la mediana E l'elenco completo dei tempi grezzi, non solo
//! la mediana — un caso patologico (una ripetizione anomala) resta visibile
//! invece di sparire dentro un aggregato.

use geo::{line_string, polygon, BoundingRect, Geometry, LineString, Polygon};
use plenora_kernels_geo::spatial_join::{spatial_join_nullable_validated, JoinPair, JoinPredicate};
use rstar::{RTree, RTreeObject, AABB};
use std::time::{Duration, Instant};

/// Fissato nei due bracci del confronto: cambiare qui cambia entrambe le
/// misure allo stesso modo, non una sola. A N=300 l'esecuzione totale
/// sarebbe 20-130ms — sotto la risoluzione utile di un confronto fra due
/// binari diversi.
const N: usize = 4000;
const MAX_PAIRS: u64 = 10_000_000;
const RIPETIZIONI: usize = 7;

fn square(cx: f64, cy: f64, half: f64) -> Polygon<f64> {
    polygon![
        (x: cx - half, y: cy - half),
        (x: cx + half, y: cy - half),
        (x: cx + half, y: cy + half),
        (x: cx - half, y: cy + half),
        (x: cx - half, y: cy - half),
    ]
}

fn diagonal_line(cx: f64, cy: f64, half: f64) -> LineString<f64> {
    line_string![
        (x: half.mul_add(-2.0, cx), y: half.mul_add(-2.0, cy)),
        (x: half.mul_add(2.0, cx), y: half.mul_add(2.0, cy)),
    ]
}

/// Frazione di candidati che raggiungono `relate`: NON tramite il passo fra
/// gruppi (spaziare due gruppi isolati piu' vicini, senza far toccare i loro
/// bbox, non cambia affatto quanti candidati l'R-tree produce — assumere il
/// contrario sarebbe un errore, vedi sotto).
/// "Densa" aggiunge invece, per ogni feature sinistra, uno o due
/// "decoy" a destra — bbox candidati veri (l'R-tree li restituisce, `relate`
/// viene chiamato) ma con una relazione nota che NON soddisfa il predicato
/// testato. "Sparsa" ha solo il candidato vero. Il gruppo i resta isolato
/// dal gruppo i+1 in entrambe le densita' (passo largo, fisso): la densita'
/// cambia quanti candidati esaminati per gruppo, non se i gruppi si
/// confondono fra loro.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Densita {
    Sparsa,
    Densa,
}

impl Densita {
    const fn nome(self) -> &'static str {
        match self {
            Self::Sparsa => "sparsa",
            Self::Densa => "densa",
        }
    }
}

const PASSO_GRUPPI: f64 = 20.0;

struct Scenario {
    nome: &'static str,
    predicate: JoinPredicate,
    densita: Densita,
    /// Numero di coppie attese (verificato, non solo loggato): la
    /// costruzione e' deterministica, quindi il conteggio atteso e' noto a
    /// priori indipendentemente da quale `geo` gira sotto.
    attese: usize,
    left: Vec<Geometry<f64>>,
    right: Vec<Geometry<f64>>,
}

const SIZE: f64 = 1.0;

const fn build(
    nome: &'static str,
    predicate: JoinPredicate,
    densita: Densita,
    attese: usize,
    left: Vec<Geometry<f64>>,
    right: Vec<Geometry<f64>>,
) -> Scenario {
    Scenario {
        nome,
        predicate,
        densita,
        attese,
        left,
        right,
    }
}

/// Un candidato bbox in piu' per feature, con una relazione nota che NON
/// soddisfa `NESSUno` dei predicati testati in questo file: un quadrato
/// "gemello" del sinistro, appena rimpicciolito e centrato nello stesso
/// punto — completamente contenuto (`Within`), mai `Overlaps`/`Touches`, e
/// il suo bbox e' comunque quello di `left[i]` quindi l'R-tree lo restituisce
/// come candidato per costruzione.
fn decoy_within(cx: f64, cy: f64, half: f64) -> Polygon<f64> {
    square(cx, cy, half * 0.3)
}

/// Decoy per gli scenari `Crosses` linea-poligono: un segmento tutto
/// interno al quadrato di riferimento — bbox candidato, ma `Within`
/// (entrambi gli estremi dentro, nessun punto fuori), mai `Crosses`.
fn decoy_line_within(cx: f64, cy: f64, half: f64) -> LineString<f64> {
    line_string![
        (x: half.mul_add(-0.3, cx), y: half.mul_add(-0.3, cy)),
        (x: half.mul_add(0.3, cx), y: half.mul_add(0.3, cy)),
    ]
}

/// Decoy per gli scenari `Crosses` linea-linea: un segmento corto,
/// orizzontale, in un angolo del bbox del gruppo lontano dalla diagonale a
/// 45 gradi usata come linea sinistra. Un decoy collineare con la
/// diagonale la intersecherebbe invece di non incrociarsi — scoperto dallo smoke test
/// (attese=0, trovate=299), non assunto. La diagonale passa per (cx+t,cy+t):
/// il decoy sta in basso a destra (y molto negativo, x positivo), dove la
/// diagonale passerebbe per x negativo — nessuna intersezione per
/// costruzione, non solo per posizione approssimata.
fn decoy_line_lontano_dalla_diagonale(cx: f64, cy: f64, half: f64) -> LineString<f64> {
    line_string![
        (x: half.mul_add(1.5, cx), y: half.mul_add(-1.9, cy)),
        (x: half.mul_add(1.8, cx), y: half.mul_add(-1.9, cy)),
    ]
}

/// `i` resta sempre `< N = 4000`: entro il range esatto di `f64` (2^53), la
/// conversione non perde precisione per costruzione di questo file.
#[allow(clippy::cast_precision_loss)]
fn cx_di(i: usize) -> f64 {
    i as f64 * PASSO_GRUPPI
}

/// 8 scenari (Overlaps/Touches/Crosses x2, positivo/negativo) x 2 densita':
/// una funzione dati piu' lunga della soglia pedantica, non piu' complessa —
/// spezzarla sposterebbe righe senza ridurre nulla.
#[allow(clippy::too_many_lines)]
fn scenari() -> Vec<Scenario> {
    let mut out = Vec::new();
    let half = SIZE / 2.0;

    for densita in [Densita::Sparsa, Densita::Densa] {
        let denso = densita == Densita::Densa;

        // --- Overlaps: positivo (sovrapposizione parziale reale) --------
        // Destra spostata di meta' lato in diagonale: interiors si
        // sovrappongono, nessuno contiene l'altro, i bordi si incrociano —
        // Overlaps DE-9IM vero, non Contains/Within/Touches. Decoy (denso):
        // un quadrato piu' piccolo interno a sinistra[i] — bbox candidato,
        // `Within` non `Overlaps`.
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i) + half, half, half)))
                .collect();
            if denso {
                right.extend((0..N).map(|i| Geometry::Polygon(decoy_within(cx(i), 0.0, half))));
            }
            out.push(build(
                "overlaps_positivo",
                JoinPredicate::Overlaps,
                densita,
                N,
                left,
                right,
            ));
        }
        // --- Overlaps: negativo (contatto di solo bordo, non overlap) ---
        // Destra adiacente esatta a sinistra: Touches, non Overlaps —
        // relate() viene chiamato e deve concludere falso. Decoy (denso):
        // stesso quadrato interno, ancora `Within` non `Overlaps`.
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i) + SIZE, 0.0, half)))
                .collect();
            if denso {
                right.extend((0..N).map(|i| Geometry::Polygon(decoy_within(cx(i), 0.0, half))));
            }
            out.push(build(
                "overlaps_negativo",
                JoinPredicate::Overlaps,
                densita,
                0,
                left,
                right,
            ));
        }

        // --- Touches: positivo (bordo condiviso, nessun overlap interno) -
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i) + SIZE, 0.0, half)))
                .collect();
            if denso {
                right.extend((0..N).map(|i| Geometry::Polygon(decoy_within(cx(i), 0.0, half))));
            }
            out.push(build(
                "touches_positivo",
                JoinPredicate::Touches,
                densita,
                N,
                left,
                right,
            ));
        }
        // --- Touches: negativo (overlap reale, non solo contatto) -------
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i) + half, half, half)))
                .collect();
            if denso {
                right.extend((0..N).map(|i| Geometry::Polygon(decoy_within(cx(i), 0.0, half))));
            }
            out.push(build(
                "touches_negativo",
                JoinPredicate::Touches,
                densita,
                0,
                left,
                right,
            ));
        }

        // --- Crosses: positivo (linea che attraversa un poligono) -------
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| Geometry::LineString(diagonal_line(cx(i), 0.0, half)))
                .collect();
            if denso {
                right.extend(
                    (0..N).map(|i| Geometry::LineString(decoy_line_within(cx(i), 0.0, half))),
                );
            }
            out.push(build(
                "crosses_poligono_positivo",
                JoinPredicate::Crosses,
                densita,
                N,
                left,
                right,
            ));
        }
        // --- Crosses: negativo (linea tangente al bordo, mai attraversa) -
        // Segmento appoggiato su un lato del quadrato (collineare col
        // bordo superiore): tocca, non attraversa — bbox candidato reale.
        // Un segmento tutto fuori dal bbox del quadrato non sarebbe
        // nemmeno un candidato.
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::Polygon(square(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| {
                    Geometry::LineString(line_string![
                        (x: cx(i) - half, y: half),
                        (x: cx(i) + half, y: half),
                    ])
                })
                .collect();
            if denso {
                right.extend(
                    (0..N).map(|i| Geometry::LineString(decoy_line_within(cx(i), 0.0, half))),
                );
            }
            out.push(build(
                "crosses_poligono_negativo",
                JoinPredicate::Crosses,
                densita,
                0,
                left,
                right,
            ));
        }
        // --- Crosses: positivo (linea contro linea, vera X) -------------
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::LineString(diagonal_line(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| {
                    Geometry::LineString(line_string![
                        (x: cx(i) - half * 2.0, y: half * 2.0),
                        (x: cx(i) + half * 2.0, y: -half * 2.0),
                    ])
                })
                .collect();
            if denso {
                // Decoy: segmento breve confinato in un solo quadrante
                // della X (da un estremo al centro) — bbox sovrapposto,
                // ma non attraversa la diagonale opposta.
                right.extend((0..N).map(|i| {
                    Geometry::LineString(line_string![
                        (x: cx(i) - half * 2.0, y: half * 2.0),
                        (x: cx(i) - half * 0.1, y: half * 0.1),
                    ])
                }));
            }
            out.push(build(
                "crosses_linea_positivo",
                JoinPredicate::Crosses,
                densita,
                N,
                left,
                right,
            ));
        }
        // --- Crosses: negativo (linea-linea parallele, mai si incrociano) -
        {
            let cx = cx_di;
            let left: Vec<_> = (0..N)
                .map(|i| Geometry::LineString(diagonal_line(cx(i), 0.0, half)))
                .collect();
            let mut right: Vec<_> = (0..N)
                .map(|i| {
                    Geometry::LineString(line_string![
                        (x: cx(i) - half * 2.0 + half * 0.2, y: -half * 2.0),
                        (x: cx(i) + half * 2.0 + half * 0.2, y: half * 2.0),
                    ])
                })
                .collect();
            if denso {
                right.extend((0..N).map(|i| {
                    Geometry::LineString(decoy_line_lontano_dalla_diagonale(cx(i), 0.0, half))
                }));
            }
            out.push(build(
                "crosses_linea_negativo",
                JoinPredicate::Crosses,
                densita,
                0,
                left,
                right,
            ));
        }
    }

    out
}

fn pairs_sorted(pairs: Vec<JoinPair>) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = pairs.into_iter().map(|p| (p.left, p.right)).collect();
    out.sort_unstable();
    out
}

struct Indicizzato {
    aabb: AABB<[f64; 2]>,
}

impl RTreeObject for Indicizzato {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.aabb
    }
}

fn aabb_di(geometry: &Geometry<f64>) -> AABB<[f64; 2]> {
    let rect = geometry.bounding_rect().expect("fixture con bbox definito");
    AABB::from_corners([rect.min().x, rect.min().y], [rect.max().x, rect.max().y])
}

/// Candidati EFFETTIVI: stessa strategia di `spatial_join.rs` (R-tree sul
/// lato destro, query per ogni elemento sinistro), non un conteggio
/// approssimato per densita' dichiarata. Il numero che conta per il
/// confronto e' quanti candidati l'R-tree produce DAVVERO con queste
/// fixture, non quanti la densita' avrebbe dovuto produrre in teoria.
fn conta_candidati_effettivi(left: &[Geometry<f64>], right: &[Geometry<f64>]) -> usize {
    let indicizzati: Vec<Indicizzato> = right
        .iter()
        .map(|g| Indicizzato { aabb: aabb_di(g) })
        .collect();
    let albero = RTree::bulk_load(indicizzati);
    left.iter()
        .map(|g| albero.locate_in_envelope_intersecting(&aabb_di(g)).count())
        .sum()
}

fn mediana(valori: &mut [f64]) -> f64 {
    valori.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let meta = valori.len() / 2;
    if valori.len().is_multiple_of(2) {
        f64::midpoint(valori[meta - 1], valori[meta])
    } else {
        valori[meta]
    }
}

/// Oracolo analitico, non un conteggio: per costruzione di [`scenari`] la
/// coppia vera e' sempre (i, i) per i positivi, nessuna per i negativi — i
/// decoy (indici >= N) non soddisfano mai il predicato testato. Confrontare
/// l'INSIEME, non la sola lunghezza, e' cio' che distingue "coppie giuste"
/// da "tante coppie quante attese, ma sbagliate" — vedi la controprova
/// dedicata nel referto.
fn coppie_attese_insieme(attese: usize) -> Vec<(u64, u64)> {
    if attese == 0 {
        Vec::new()
    } else {
        (0..attese as u64).map(|i| (i, i)).collect()
    }
}

/// Solo per la controprova sull'escalation del timeout esterno: mai
/// raggiunto da una corsa normale (nessuna variabile d'ambiente impostata).
fn simula_blocco_se_richiesto() {
    if std::env::var("PLENORA_BENCH_SIMULA_BLOCCO").is_ok() {
        eprintln!("simulazione-blocco-avviata");
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

enum EsitoScenario {
    Misurato {
        candidati: usize,
        tempi: Vec<f64>,
        coppie: Vec<(u64, u64)>,
    },
    Errore(String),
    Mismatch {
        ripetizione: usize,
        attese_len: usize,
        trovate: Vec<(u64, u64)>,
    },
}

fn esegui_scenario(scenario: &Scenario) -> EsitoScenario {
    let candidati = conta_candidati_effettivi(&scenario.left, &scenario.right);
    let left_opt: Vec<_> = scenario.left.iter().cloned().map(Some).collect();
    let right_opt: Vec<_> = scenario.right.iter().cloned().map(Some).collect();
    let atteso = coppie_attese_insieme(scenario.attese);

    // Riscaldamento: scartato dai tempi, ma verificato come ogni altra
    // ripetizione — un errore o un esito sbagliato qui non deve sparire
    // solo perche' non e' cronometrato.
    let mut tempi = Vec::with_capacity(RIPETIZIONI);
    let mut ultima_verificata = Vec::new();
    for ripetizione in 0..=RIPETIZIONI {
        let e_riscaldamento = ripetizione == 0;
        let inizio = Instant::now();
        let esito =
            spatial_join_nullable_validated(&left_opt, &right_opt, scenario.predicate, MAX_PAIRS);
        let durata = inizio.elapsed();
        let pairs = match esito {
            Err(errore) => return EsitoScenario::Errore(errore.to_string()),
            Ok(p) => p,
        };
        // Confronto dell'insieme COMPLETO, FUORI dalla sezione cronometrata
        // sopra: l'ordinamento e il confronto non contano nel tempo del
        // join, ma contano per ogni singola ripetizione, non solo per
        // l'ultima.
        let ordinate = pairs_sorted(pairs);
        if ordinate != atteso {
            return EsitoScenario::Mismatch {
                ripetizione,
                attese_len: atteso.len(),
                trovate: ordinate,
            };
        }
        if !e_riscaldamento {
            tempi.push(durata.as_secs_f64());
        }
        ultima_verificata = ordinate;
    }

    EsitoScenario::Misurato {
        candidati,
        tempi,
        coppie: ultima_verificata,
    }
}

fn main() {
    simula_blocco_se_richiesto();

    let budget = std::env::var("PLENORA_BENCH_BUDGET_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(Duration::from_secs(300), Duration::from_secs);

    std::fs::create_dir_all("benchmarks/join").expect("mkdir benchmarks/join");
    let mut righe: Vec<serde_json::Value> = Vec::new();

    let inizio_totale = Instant::now();
    let scenari = scenari();
    let mut interrotto = false;

    for scenario in &scenari {
        if interrotto || inizio_totale.elapsed() > budget {
            interrotto = true;
            righe.push(serde_json::json!({
                "scenario": scenario.nome,
                "densita": scenario.densita.nome(),
                "stato": "incompleto",
                "motivo": "budget di esecuzione esaurito prima di raggiungere questo scenario",
            }));
            continue;
        }

        match esegui_scenario(scenario) {
            EsitoScenario::Errore(errore) => {
                righe.push(serde_json::json!({
                    "scenario": scenario.nome,
                    "densita": scenario.densita.nome(),
                    "stato": "errore",
                    "errore": errore,
                }));
            }
            EsitoScenario::Mismatch {
                ripetizione,
                attese_len,
                trovate,
            } => {
                righe.push(serde_json::json!({
                    "scenario": scenario.nome,
                    "densita": scenario.densita.nome(),
                    "stato": "mismatch",
                    "ripetizione_fallita": ripetizione,
                    "coppie_attese_conteggio": attese_len,
                    "coppie_trovate_conteggio": trovate.len(),
                    "coppie_trovate": trovate,
                }));
            }
            EsitoScenario::Misurato {
                candidati,
                tempi,
                coppie,
            } => {
                righe.push(serde_json::json!({
                    "scenario": scenario.nome,
                    "densita": scenario.densita.nome(),
                    "predicate": format!("{:?}", scenario.predicate),
                    "n": N,
                    "stato": "misurato",
                    "candidati_effettivi": candidati,
                    "coppie_verificate_a_ogni_ripetizione": true,
                    "ripetizioni": RIPETIZIONI,
                    "elapsed_secs_mediana": mediana(&mut tempi.clone()),
                    "elapsed_secs_tutte_le_ripetizioni": tempi,
                    "coppie": coppie,
                }));
            }
        }
    }

    let percorso = "benchmarks/join/geo_join_predicates.jsonl";
    let contenuto = righe
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(percorso, contenuto + "\n").expect("scrittura output");

    let misurati = righe.iter().filter(|r| r["stato"] == "misurato").count();
    let incompleti = righe.iter().filter(|r| r["stato"] == "incompleto").count();
    let errori = righe.iter().filter(|r| r["stato"] == "errore").count();
    let mismatch = righe.iter().filter(|r| r["stato"] == "mismatch").count();
    println!(
        "{}",
        serde_json::json!({
            "totale_scenari": righe.len(),
            "misurati": misurati,
            "incompleti": incompleti,
            "errori": errori,
            "mismatch": mismatch,
            "budget_secs": budget.as_secs(),
            "tempo_totale_secs": inizio_totale.elapsed().as_secs_f64(),
            "output": percorso,
        })
    );
    // Codici distinti: un chiamante automatico deve poter distinguere "il
    // join ha fallito" da "un risultato e' sbagliato" da "non ho finito in
    // tempo" senza riaprire il JSON.
    if errori > 0 {
        std::process::exit(3);
    }
    if mismatch > 0 {
        std::process::exit(4);
    }
    if incompleti > 0 {
        std::process::exit(2);
    }
}
