//! Budget di memoria del runner: modello di costo, rifiuto prima di
//! eseguire (niente va su disco: un passo che non sta non ha ripiego),
//! vivibilità nelle catene lunghe, catene generate.

mod comune;

use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::{PlenoraError, Result};
use plenora_pipeline::budget::{costo_di, Ingresso};
use plenora_pipeline::costi_operazioni::{COSTI, SHA256_MISURE};
use plenora_pipeline::{byte_vivi, Esito, LimitiParziali, Passo, Pipeline};
use proptest::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Budget di default: ogni passo di questi test ci sta.
const BUDGET_AMPIO: u64 = 512 * 1024 * 1024;

fn passo(out: &str, op: &str, inputs: &[&str], config: Value) -> Passo {
    Passo {
        out: out.to_owned(),
        op: op.to_owned(),
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        config,
    }
}

fn piano(inputs: &[&str], steps: Vec<Passo>, outputs: &[&str], budget: u64) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: inputs.iter().map(|nome| (*nome).to_owned()).collect(),
        crs: None,
        limits: Some(LimitiParziali {
            max_governed_memory_bytes: Some(budget),
            ..LimitiParziali::default()
        }),
        steps,
        outputs: outputs.iter().map(|nome| (*nome).to_owned()).collect(),
    }
}

fn esegui(pipeline: &Pipeline, tabelle: &[(&str, RecordBatch)]) -> Result<Esito> {
    let schemi: Vec<(&str, SchemaRef)> = tabelle
        .iter()
        .map(|(nome, tabella)| (*nome, tabella.schema()))
        .collect();
    pipeline.validate(&schemi)?.run(
        tabelle
            .iter()
            .map(|(nome, tabella)| ((*nome).to_owned(), tabella.clone()))
            .collect(),
    )
}

/// Lo stesso piano con il budget di default: il riferimento.
fn riferimento(pipeline: &Pipeline, tabelle: &[(&str, RecordBatch)]) -> Esito {
    let mut ampio = pipeline.clone();
    ampio.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(BUDGET_AMPIO),
        ..LimitiParziali::default()
    });
    esegui(&ampio, tabelle).expect("riferimento")
}

/// `ResourceLimit` prima di eseguire, con il passo e l'operazione nominati.
fn rifiutato_prima(errore: &PlenoraError, out: &str, op: &str) -> bool {
    matches!(errore, PlenoraError::ResourceLimit(messaggio)
        if messaggio.contains(&format!("passo `{out}`"))
            && messaggio.contains(op)
            && messaggio.contains("previsti"))
}

/// `righe` righe: `k` intero, `v` reale, `s` testo di `larghezza` byte.
fn tabella(righe: usize, larghezza: usize, seme: i64) -> RecordBatch {
    let chiavi: Vec<i64> = (0..righe)
        .map(|riga| (i64::try_from(riga).expect("riga") * 7919 + seme) % 1009)
        .collect();
    let colonne: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(chiavi.clone())),
        Arc::new(Float64Array::from_iter_values(chiavi.iter().map(
            |chiave| f64::from(i32::try_from(*chiave).expect("chiave")) / 3.0,
        ))),
        Arc::new(StringArray::from_iter_values(chiavi.iter().map(|chiave| {
            let base = format!("{chiave:08}");
            base.repeat(larghezza.div_ceil(8).max(1))[..larghezza].to_owned()
        }))),
    ];
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("k", DataType::Int64, false),
            Field::new("v", DataType::Float64, false),
            Field::new("s", DataType::Utf8, false),
        ])),
        colonne,
    )
    .expect("tabella")
}

fn byte(tabella: &RecordBatch) -> u64 {
    byte_vivi(std::iter::once(tabella)).expect("byte")
}

fn picco(op: &str, righe: u64, byte_in: u64) -> u64 {
    costo_di(op).expect("modello").in_memoria.picco(Ingresso {
        righe,
        byte: byte_in,
        ..Ingresso::default()
    })
}

fn righe(tabella: &RecordBatch) -> u64 {
    u64::try_from(tabella.num_rows()).expect("righe")
}

#[test]
fn le_misure_nel_repository_sono_quelle_del_modello() {
    let percorso = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/misure/catalogo-memoria-v4.json"
    );
    let testo = std::fs::read(percorso).expect("misure");
    let impronta = plenora_core::esadecimale::esadecimale(&Sha256::digest(&testo));
    assert_eq!(impronta, SHA256_MISURE);
    assert_eq!(SHA256_MISURE, plenora_pipeline::costi_geo::SHA256_MISURE);
}

#[test]
fn ogni_operazione_del_runner_ha_un_modello() {
    for caso in comune::CASI {
        assert!(costo_di(caso.op).is_some(), "{} senza modello", caso.op);
    }
    // Nessun profilo spilled nel modello: il runner esegue solo i kernel in
    // memoria.
    for voce in COSTI {
        assert!(
            voce.profili.iter().all(|id| !id.starts_with("spilled")),
            "{}: {:?}",
            voce.op,
            voce.profili
        );
    }
}

#[test]
fn una_tabella_fredda_non_va_su_disco_e_il_passo_si_rifiuta_prima() {
    // Il sort di `caldo` sta nel budget solo senza `freddo` residente.
    // Prima `freddo` si sfrattava su disco e il piano riusciva: ora il
    // passo si rifiuta prima di eseguire, ed e' il primo, quindi nessun
    // kernel ha girato.
    let caldo = tabella(60_000, 8, 1);
    let freddo = tabella(8_000, 100, 2);
    let budget = byte(&caldo) + picco("table.sort", righe(&caldo), byte(&caldo)) + 1;
    assert!(byte(&caldo) + byte(&freddo) <= budget);
    let pipeline = piano(
        &["caldo", "freddo"],
        vec![
            passo(
                "ordinato",
                "table.sort",
                &["caldo"],
                json!({"columns": ["k"]}),
            ),
            passo(
                "stretto",
                "table.select_columns",
                &["freddo"],
                json!({"columns": ["k", "s"]}),
            ),
        ],
        &["ordinato", "stretto"],
        budget,
    );
    let tabelle = [("caldo", caldo), ("freddo", freddo.clone())];
    let errore = esegui(&pipeline, &tabelle).expect_err("oltre il budget");
    assert!(
        rifiutato_prima(&errore, "ordinato", "table.sort"),
        "{errore}"
    );
    // Con il posto per `freddo` lo stesso piano gira, con l'uscita del
    // riferimento: il rifiuto e' solo il budget.
    let mut largo = pipeline;
    largo.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(budget + byte(&freddo)),
        ..LimitiParziali::default()
    });
    let esito = esegui(&largo, &tabelle).expect("nel budget");
    for passo in &esito.report.passi {
        assert!(
            passo.byte_vivi_con_uscita <= budget + byte(&freddo),
            "{passo:?}"
        );
    }
    assert_eq!(esito.outputs, riferimento(&largo, &tabelle).outputs);
}

#[test]
fn una_set_operation_oltre_il_budget_si_rifiuta_senza_ripiego() {
    // Prima una set operation oltre il budget passava alla variante
    // spilled; ora si rifiuta prima di eseguire.
    let sinistra = tabella(20_000, 16, 5);
    let destra = tabella(20_000, 16, 6);
    let insieme = byte_vivi([&sinistra, &destra]).expect("byte");
    let righe_in = righe(&sinistra) + righe(&destra);
    for op in ["table.intersect", "table.except", "table.union_distinct"] {
        // Sotto i byte vivi piu' il picco: il runner usa byte in ingresso
        // mai sotto i byte vivi, quindi la previsione e' almeno questa.
        let budget = insieme + picco(op, righe_in, insieme) - 1;
        let pipeline = piano(
            &["a", "b"],
            vec![passo("x", op, &["a", "b"], json!({}))],
            &["x"],
            budget,
        );
        let tabelle = [("a", sinistra.clone()), ("b", destra.clone())];
        let errore = esegui(&pipeline, &tabelle).expect_err("oltre il budget");
        assert!(rifiutato_prima(&errore, "x", op), "{op}: {errore}");
    }
}

#[test]
fn un_sort_che_non_sta_si_rifiuta_prima_di_eseguire() {
    let larga = tabella(20_000, 400, 7);
    let byte_in = byte(&larga);
    let memoria = picco("table.sort", righe(&larga), byte_in);
    let budget = byte_in + memoria + 1;
    let pipeline = piano(
        &["t"],
        vec![passo(
            "x",
            "table.sort",
            &["t"],
            json!({"columns": ["s", "k"], "ascending": false}),
        )],
        &["x"],
        budget,
    );
    let tabelle = [("t", larga)];
    let esito = esegui(&pipeline, &tabelle).expect("in memoria");
    let passo = &esito.report.passi[0];
    assert!(passo.byte_vivi_con_uscita <= budget);
    assert_eq!(passo.margine_kernel, budget - byte_in);
    assert_eq!(esito.outputs, riferimento(&pipeline, &tabelle).outputs);

    // Sotto la somma di input e picco: rifiuto prima di eseguire.
    let mut stretto = pipeline;
    stretto.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(budget - 2),
        ..LimitiParziali::default()
    });
    let errore = esegui(&stretto, &tabelle).expect_err("oltre il budget");
    assert!(rifiutato_prima(&errore, "x", "table.sort"), "{errore}");
}

/// `pivot` con `mapping` nel runner: il modello conta le celle d'uscita,
/// righe in ingresso per colonne del contratto validato (indice piu' una
/// colonna per voce), quindi la previsione cresce con il `mapping` e copre
/// i byte nuovi dell'uscita.
#[test]
fn il_pivot_con_mapping_conta_le_celle_del_contratto() {
    // Formato EAV: chiave testuale (una ogni quattro righe), nome
    // dell'attributo fra 32, valore.
    let n = 20_000_usize;
    let t = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("chiave", DataType::Utf8, false),
            Field::new("nome", DataType::Utf8, false),
            Field::new("v", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from_iter_values(
                (0..n).map(|riga| format!("k{:06}", riga / 4)),
            )),
            Arc::new(StringArray::from_iter_values(
                (0..n).map(|riga| format!("a{:02}", riga % 32)),
            )),
            Arc::new(Float64Array::from_iter_values((0..n).map(|riga| {
                f64::from(u32::try_from(riga % 97).expect("valore"))
            }))),
        ],
    )
    .expect("tabella");
    let costo = costo_di("table.pivot").expect("modello").in_memoria;
    assert!(costo.k_millesimi > 0);
    let mut precedente = 0;
    for voci in [2_usize, 8, 32] {
        let mapping: serde_json::Map<String, Value> = (0..voci)
            .map(|voce| (format!("a{voce:02}"), json!(format!("c{voce}"))))
            .collect();
        let pipeline = piano(
            &["t"],
            vec![passo(
                "p",
                "table.pivot",
                &["t"],
                json!({"index_col": "chiave", "pivot_col": "nome", "value_col": "v",
                       "aggr_func": "sum", "mapping": mapping}),
            )],
            &["p"],
            BUDGET_AMPIO,
        );
        let esito = esegui(&pipeline, &[("t", t.clone())]).expect("pivot");
        let passo = &esito.report.passi[0];
        let colonne = u64::try_from(voci + 1).expect("colonne");
        assert_eq!(
            output_colonne(&esito, "p"),
            voci + 1,
            "indice piu' una colonna per voce"
        );
        let senza_celle = Ingresso {
            righe: righe(&t),
            byte: byte(&t),
            ..Ingresso::default()
        };
        let con_celle = Ingresso {
            celle: righe(&t) * colonne,
            ..senza_celle
        };
        // Il runner usa byte in ingresso mai sotto i byte vivi.
        assert!(passo.byte_previsti >= costo.picco(con_celle), "{passo:?}");
        assert!(costo.picco(con_celle) > costo.picco(senza_celle));
        assert!(
            passo.byte_previsti >= passo.byte_output_esclusivi,
            "{passo:?}"
        );
        assert!(passo.byte_previsti > precedente, "{voci} voci");
        precedente = passo.byte_previsti;
    }
}

fn output_colonne(esito: &Esito, nome: &str) -> usize {
    esito
        .outputs
        .iter()
        .find(|(uscita, _)| uscita == nome)
        .map(|(_, tabella)| tabella.num_columns())
        .expect("uscita")
}

#[test]
fn un_cross_join_oltre_il_budget_si_rifiuta_con_passo_e_operazione() {
    let a = tabella(2_000, 8, 8);
    let b = tabella(2_000, 8, 9);
    let pipeline = piano(
        &["a", "b"],
        vec![
            passo(
                "b2",
                "table.rename",
                &["b"],
                json!({"renames": [
                    {"old_name": "k", "new_name": "k2"},
                    {"old_name": "v", "new_name": "v2"},
                    {"old_name": "s", "new_name": "s2"}
                ]}),
            ),
            passo("prodotto", "table.cross_join", &["a", "b2"], json!({})),
        ],
        &["prodotto"],
        64 * 1024 * 1024,
    );
    let errore = esegui(&pipeline, &[("a", a), ("b", b)]).expect_err("oltre il budget");
    let PlenoraError::ResourceLimit(messaggio) = &errore else {
        panic!("atteso ResourceLimit: {errore}");
    };
    assert!(messaggio.contains("passo `prodotto`"), "{messaggio}");
    assert!(messaggio.contains("table.cross_join"), "{messaggio}");
}

#[test]
fn una_catena_lunga_resta_nel_budget_grazie_alla_vivibilita() {
    // 40 passi in fila: ogni intermedio muore al passo dopo, i byte vivi
    // restano quelli di due tabelle.
    let base = tabella(20_000, 32, 10);
    let byte_base = byte(&base);
    let mut passi = Vec::new();
    let mut precedente = "t".to_owned();
    for indice in 0..40 {
        let out = format!("p{indice:02}");
        let (op, config) = match indice % 4 {
            0 => ("table.sort", json!({"columns": ["k", "s"]})),
            1 => (
                "table.filter",
                json!({"column": "v", "operator": ">=", "value": 0}),
            ),
            2 => ("table.select_columns", json!({"columns": ["k", "v", "s"]})),
            _ => ("table.distinct", json!({"subset": ["k", "v", "s"]})),
        };
        passi.push(passo(&out, op, &[&precedente], config));
        precedente = out;
    }
    let budget = 64 * 1024 * 1024;
    let pipeline = piano(&["t"], passi, &[&precedente], budget);
    let esito = esegui(&pipeline, &[("t", base)]).expect("catena");
    assert_eq!(esito.report.passi.len(), 40);
    for passo in &esito.report.passi {
        assert!(passo.byte_vivi_con_uscita <= 3 * byte_base, "{passo:?}");
        assert!(passo.byte_vivi <= 2 * byte_base, "{passo:?}");
    }
}

/// Operazioni della proptest: output prevedibile dalle righe e dai byte
/// dell'input (nessuna dipende dai dati nel modello).
fn operazione(scelta: u8, soglia: i64, n: usize) -> (&'static str, Value) {
    match scelta % 6 {
        0 => ("table.sort", json!({"columns": ["k", "v"]})),
        1 => (
            "table.filter",
            json!({"column": "k", "operator": ">", "value": soglia}),
        ),
        2 => ("table.select_columns", json!({"columns": ["k", "v", "s"]})),
        3 => ("table.distinct", json!({"subset": ["k"]})),
        4 => ("table.limit", json!({"n": n})),
        _ => (
            "table.top_n",
            json!({"columns": ["v"], "n": n, "descending": true}),
        ),
    }
}

/// `pieni` con `PLENORA_TEST_LUNGHI=1` (suite lunga, README «Suite lunga»),
/// `ridotti` altrimenti; un valore diverso da `0` e `1` ferma il test. Copia
/// di `casi` di `test_support`, che un test d'integrazione non raggiunge.
fn casi(ridotti: u32, pieni: u32) -> u32 {
    match std::env::var("PLENORA_TEST_LUNGHI") {
        Err(std::env::VarError::NotPresent) => ridotti,
        Ok(valore) if valore == "0" => ridotti,
        Ok(valore) if valore == "1" => pieni,
        _ => panic!("PLENORA_TEST_LUNGHI vale 1 (suite lunga) o 0"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: casi(24, 96),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// A ogni confine di passo i byte vivi stanno nel budget, il picco
    /// previsto copre i byte nuovi dell'output e l'output e' quello del
    /// riferimento con il budget ampio; oppure il piano si rifiuta con
    /// `ResourceLimit`.
    #[test]
    fn le_catene_generate_restano_nel_budget(
        righe_a in 1_usize..40_000,
        righe_b in 1_usize..40_000,
        larghezza in 1_usize..200,
        budget_mib in 1_u64..16,
        passi in prop::collection::vec((any::<u8>(), 0_usize..8, -5_i64..1_100, 1_usize..3_000), 1..10),
        ultimi in 1_usize..3,
    ) {
        let tabelle = [
            ("a", tabella(righe_a, larghezza, 11)),
            ("b", tabella(righe_b, larghezza / 2 + 1, 12)),
        ];
        let mut nomi: Vec<String> = vec!["a".into(), "b".into()];
        let mut steps = Vec::new();
        for (indice, (scelta, sorgente, soglia, n)) in passi.iter().enumerate() {
            let (op, config) = operazione(*scelta, *soglia, *n);
            let ingresso = nomi[sorgente % nomi.len()].clone();
            let out = format!("p{indice}");
            steps.push(passo(&out, op, &[&ingresso], config));
            nomi.push(out);
        }
        let uscite: Vec<&str> = nomi.iter().rev().take(ultimi).map(String::as_str).collect();
        // Oltre gli input, da uno a sedici MiB: abbastanza vicino da
        // costringere a rifiuti.
        let budget = byte_vivi(tabelle.iter().map(|(_, tabella)| tabella)).expect("byte")
            + budget_mib * 1024 * 1024;
        let pipeline = piano(&["a", "b"], steps, &uscite, budget);
        match esegui(&pipeline, &tabelle) {
            Ok(esito) => {
                for passo in &esito.report.passi {
                    prop_assert!(passo.byte_vivi_con_uscita <= budget, "{:?}", passo);
                    prop_assert!(passo.byte_vivi <= budget, "{:?}", passo);
                    let modello = costo_di(passo.op).expect("modello");
                    if !modello.dipende_dai_dati {
                        prop_assert!(
                            passo.byte_previsti >= passo.byte_output_esclusivi,
                            "{:?}", passo
                        );
                    }
                }
                prop_assert_eq!(esito.outputs, riferimento(&pipeline, &tabelle).outputs);
            }
            Err(PlenoraError::ResourceLimit(_)) => {}
            Err(altro) => prop_assert!(false, "errore inatteso: {altro}"),
        }
    }
}

#[test]
fn input_oltre_il_budget_si_rifiutano_anche_senza_passi() {
    let t = tabella(20_000, 32, 17);
    let pipeline = piano(&["t"], Vec::new(), &["t"], byte(&t) - 1);
    let errore = esegui(&pipeline, &[("t", t)]).expect_err("oltre il budget");
    assert!(
        matches!(&errore, PlenoraError::ResourceLimit(messaggio)
            if messaggio.contains("byte vivi iniziali")),
        "{errore}"
    );
}
