//! Oracolo del modello di costo contro le misure: per ogni punto osservato
//! di `data/misure/catalogo-memoria-v4.json`, il picco del modello senza
//! fattore di sicurezza non è sotto il picco misurato (l'invariante di
//! conservatività; con `S` la previsione è almeno una volta e mezza la
//! misura). Solo i profili dichiarati in `esclusi` restano fuori, e il test
//! ne riporta il rapporto. I profili delle varianti spilled che il catalogo
//! conserva (`execution` diverso da `direct`) non sono nel modello: il
//! runner esegue solo i kernel in memoria.
//!
//! Le unità di un punto sono quelle che il runner calcola per un passo
//! (`scripts/modello_costi.py`, `unita_del_punto`): righe di tutti gli
//! ingressi (per `geo.generate_grid` le celle d'uscita, note a secco), byte
//! Arrow degli ingressi, righe sinistra per righe destra, righe per colonne
//! d'uscita.
//!
//! La distribuzione dei rapporti previsto/misurato (con `S`, sui punti di
//! almeno 1 MiB) si legge con
//!
//! ```sh
//! cargo test -p plenora-pipeline --test oracolo_costi -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use plenora_pipeline::budget::{costo_di, Ingresso};
use serde_json::Value;

const MIB: u64 = 1024 * 1024;

fn misure() -> Value {
    let percorso = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/misure/catalogo-memoria-v4.json"
    );
    serde_json::from_slice(&std::fs::read(percorso).expect("misure")).expect("json")
}

fn intero(punto: &Value, campo: &str) -> u64 {
    punto[campo]
        .as_u64()
        .unwrap_or_else(|| panic!("campo {campo} non intero non negativo"))
}

/// Picco da coprire: stima di budget, mai meno dei byte nuovi dell'output,
/// mai negativo.
fn picco_misurato(punto: &Value) -> u64 {
    let stima = punto["budget_estimate_bytes"].as_i64().expect("stima");
    let uscita = intero(punto, "output_new_buffer_bytes");
    u64::try_from(stima.max(0)).expect("stima").max(uscita)
}

fn ingresso(operazione: &str, punto: &Value) -> Ingresso {
    let (primarie, secondarie) = if punto.get("rows").is_some() {
        (intero(punto, "rows"), intero(punto, "secondary_rows"))
    } else {
        (
            intero(punto, "features"),
            intero(punto, "secondary_features"),
        )
    };
    let mut righe = primarie + secondarie;
    if operazione == "geo.generate_grid" {
        righe = righe.max(intero(punto, "output_rows"));
    }
    Ingresso {
        righe,
        byte: intero(punto, "input_buffer_bytes"),
        coppie: primarie * secondarie,
        celle: righe * intero(punto, "output_columns"),
    }
}

/// Mediana, novantesimo percentile e massimo di valori ordinati.
fn distribuzione(valori: &mut [f64]) -> (f64, f64, f64) {
    valori.sort_by(f64::total_cmp);
    let n = valori.len();
    assert!(n > 0);
    let quantile = |q: f64| {
        // Indice per difetto del quantile, come `numpy` con `lower`.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let indice = (q * (n - 1) as f64).floor() as usize;
        valori[indice]
    };
    (quantile(0.5), quantile(0.9), valori[n - 1])
}

#[test]
fn il_modello_copre_ogni_punto_misurato_non_escluso() {
    let dati = misure();
    let mut sui_coperti = Vec::new();
    let mut esclusi_visti = BTreeSet::new();
    let mut sotto_esclusi = Vec::new();
    let mut coperti = 0_usize;
    for profilo in dati["profiles"].as_array().expect("profili") {
        if profilo["status"] != "measured" {
            continue;
        }
        let operazione = profilo["operation_id"].as_str().expect("id");
        let id = profilo["profile_id"].as_str().expect("profilo");
        let voce = costo_di(operazione).unwrap_or_else(|| panic!("{operazione} senza modello"));
        let spill = !(profilo["family"] == "geo" || profilo["execution"] == "direct");
        assert_eq!(
            voce.profili.contains(&id),
            !spill,
            "{operazione}: profilo {id} misurato in memoria e non elencato, o spilled ed elencato"
        );
        if spill {
            continue;
        }
        let costo = voce.in_memoria;
        let escluso = voce.esclusi.contains(&id);
        if escluso {
            esclusi_visti.insert((operazione, id));
        }
        for punto in profilo["points"].as_array().expect("punti") {
            if punto["status"] != "observed" {
                continue;
            }
            let misurato = picco_misurato(punto);
            let unita = ingresso(operazione, punto);
            let base = costo.base(unita);
            #[allow(clippy::cast_precision_loss)]
            let rapporto = costo.picco(unita) as f64 / misurato.max(1) as f64;
            if escluso {
                if base < u128::from(misurato) {
                    sotto_esclusi.push((operazione, id, rapporto));
                }
                continue;
            }
            assert!(
                base >= u128::from(misurato),
                "{operazione} {id}: modello {base} sotto il picco misurato {misurato} \
                 (righe {}, byte {})",
                unita.righe,
                unita.byte
            );
            coperti += 1;
            if misurato >= MIB {
                sui_coperti.push(rapporto);
            }
        }
    }
    // Ogni esclusione dichiarata esiste nelle misure.
    for voce in plenora_pipeline::costi_geo::COSTI_GEO
        .iter()
        .chain(plenora_pipeline::costi_operazioni::COSTI)
    {
        for id in voce.esclusi {
            assert!(
                esclusi_visti.contains(&(voce.op, *id)),
                "{}: escluso {id} non misurato",
                voce.op
            );
        }
    }
    assert!(coperti > 1_000, "{coperti} punti coperti");
    let (mediana, p90, massimo) = distribuzione(&mut sui_coperti);
    eprintln!(
        "previsto/misurato (con S, punti >= 1 MiB): {} punti, mediana {mediana:.2}, \
         p90 {p90:.2}, massimo {massimo:.2}",
        sui_coperti.len()
    );
    assert!(mediana >= 1.5);
    sotto_esclusi.sort_by(|a, b| a.2.total_cmp(&b.2));
    for (operazione, id, rapporto) in &sotto_esclusi {
        eprintln!("escluso sotto la misura: {operazione} {id}: {rapporto:.3}");
    }
}
