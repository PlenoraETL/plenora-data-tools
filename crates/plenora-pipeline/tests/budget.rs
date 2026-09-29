//! Budget di memoria del runner: modello di costo, sfratto e rilettura,
//! variante spilled, rifiuto prima di eseguire, catene generate.

mod comune;

use std::sync::Arc;

use plenora_core::arrow::array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::{PlenoraError, Result};
use plenora_pipeline::budget::{costo_di, riserva_spill, Ingresso};
use plenora_pipeline::costi_operazioni::{COSTI, SHA256_CATALOGO};
use plenora_pipeline::{byte_vivi, Esito, LimitiParziali, Passo, Pipeline, Variante};
use proptest::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Budget di default: nessuno sfratto sulle tabelle di questi test.
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

/// Lo stesso piano con il budget di default: il riferimento senza sfratti
/// e senza spill.
fn riferimento(pipeline: &Pipeline, tabelle: &[(&str, RecordBatch)]) -> Esito {
    let mut ampio = pipeline.clone();
    ampio.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(BUDGET_AMPIO),
        ..LimitiParziali::default()
    });
    let esito = esegui(&ampio, tabelle).expect("riferimento");
    for passo in &esito.report.passi {
        assert!(passo.sfrattati.is_empty() && passo.variante == Variante::InMemoria);
    }
    esito
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

fn picco(op: &str, righe: u64, byte_in: u64, spill: bool) -> u64 {
    let costo = costo_di(op).expect("modello");
    let ingresso = Ingresso {
        righe,
        byte: byte_in,
        coppie: 0,
    };
    if spill {
        // Partizioni di default (64).
        costo.spill.expect("variante spilled").picco(ingresso) + riserva_spill(byte_in, 64)
    } else {
        costo.in_memoria.picco(ingresso)
    }
}

fn righe(tabella: &RecordBatch) -> u64 {
    u64::try_from(tabella.num_rows()).expect("righe")
}

#[test]
fn il_catalogo_nel_repository_e_quello_del_modello() {
    let percorso = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/misure/catalogo-memoria-tabellare-v3.json"
    );
    let testo = std::fs::read(percorso).expect("catalogo");
    let impronta = plenora_core::esadecimale::esadecimale(&Sha256::digest(&testo));
    assert_eq!(impronta, SHA256_CATALOGO);
}

#[test]
fn ogni_operazione_del_runner_ha_un_modello_e_lo_spill_dove_il_kernel_lo_ha() {
    for caso in comune::CASI {
        assert!(costo_di(caso.op).is_some(), "{} senza modello", caso.op);
    }
    let con_spill: Vec<&str> = COSTI
        .iter()
        .filter(|voce| voce.spill.is_some())
        .map(|voce| voce.op)
        .collect();
    assert_eq!(
        con_spill,
        [
            "table.aggregate",
            "table.distinct",
            "table.except",
            "table.intersect",
            "table.sort",
            "table.union_distinct"
        ]
    );
}

#[test]
fn le_tabelle_fredde_si_sfrattano_e_si_rileggono_con_lo_stesso_output() {
    let caldo = tabella(60_000, 8, 1);
    let freddo = tabella(20_000, 100, 2);
    // Il sort di `caldo` sta nel budget solo senza `freddo` residente.
    let budget = byte(&caldo) + picco("table.sort", righe(&caldo), byte(&caldo), false) + 1;
    assert!(byte(&freddo) > 1);
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
                "filtrato",
                "table.filter",
                &["ordinato"],
                json!({"column": "v", "operator": ">", "value": 10}),
            ),
            passo(
                "stretto",
                "table.select_columns",
                &["freddo"],
                json!({"columns": ["k", "s"]}),
            ),
        ],
        &["filtrato", "stretto"],
        budget,
    );
    let tabelle = [("caldo", caldo), ("freddo", freddo)];
    let esito = esegui(&pipeline, &tabelle).expect("nel budget con lo sfratto");
    let passi = &esito.report.passi;
    assert_eq!(passi[0].sfrattati, ["freddo"]);
    assert_eq!(passi[2].ricaricati, ["freddo"]);
    for passo in passi {
        assert!(passo.byte_vivi_con_uscita <= budget, "{passo:?}");
        assert!(passo.byte_vivi <= budget, "{passo:?}");
    }
    assert!(esito.report.byte_su_disco_massimi > 0);
    assert_eq!(esito.outputs, riferimento(&pipeline, &tabelle).outputs);
}

#[test]
fn gli_output_sfrattati_si_rileggono_alla_fine() {
    let primo = tabella(20_000, 64, 3);
    let secondo = tabella(60_000, 8, 4);
    // `copia` e' un output del piano senza altri consumatori: il primo
    // candidato allo sfratto (prossimo uso piu' lontano).
    let budget = byte(&secondo) + picco("table.sort", righe(&secondo), byte(&secondo), false) + 1;
    let pipeline = piano(
        &["primo", "secondo"],
        vec![
            passo(
                "copia",
                "table.select_columns",
                &["primo"],
                json!({"columns": ["k", "s"]}),
            ),
            passo(
                "ordinato",
                "table.sort",
                &["secondo"],
                json!({"columns": ["k"]}),
            ),
        ],
        &["copia", "ordinato"],
        budget,
    );
    let tabelle = [("primo", primo), ("secondo", secondo)];
    let esito = esegui(&pipeline, &tabelle).expect("nel budget");
    assert_eq!(esito.report.passi[1].sfrattati, ["copia"]);
    assert_eq!(esito.report.ricaricati_alla_fine, ["copia"]);
    assert_eq!(esito.outputs, riferimento(&pipeline, &tabelle).outputs);
}

#[test]
fn una_set_operation_oltre_il_budget_passa_alla_variante_spilled() {
    let sinistra = tabella(20_000, 16, 5);
    let destra = tabella(20_000, 16, 6);
    let insieme = byte_vivi([&sinistra, &destra]).expect("byte");
    let righe_in = righe(&sinistra) + righe(&destra);
    let spill = picco("table.intersect", righe_in, insieme, true);
    let memoria = picco("table.intersect", righe_in, insieme, false);
    assert!(spill < memoria);
    let budget = insieme + spill + 1;
    for op in ["table.intersect", "table.except", "table.union_distinct"] {
        let pipeline = piano(
            &["a", "b"],
            vec![passo("x", op, &["a", "b"], json!({}))],
            &["x"],
            budget + picco(op, righe_in, insieme, true) - spill,
        );
        let tabelle = [("a", sinistra.clone()), ("b", destra.clone())];
        let esito = esegui(&pipeline, &tabelle).unwrap_or_else(|errore| panic!("{op}: {errore}"));
        assert_eq!(esito.report.passi[0].variante, Variante::Spill, "{op}");
        assert_eq!(
            esito.outputs,
            riferimento(&pipeline, &tabelle).outputs,
            "{op}"
        );
    }
}

#[test]
fn un_sort_che_non_sta_in_memoria_passa_alla_variante_spilled() {
    // Righe larghe: il termine per byte decide, e quello della variante
    // spilled e' piu' basso.
    let larga = tabella(20_000, 400, 7);
    let byte_in = byte(&larga);
    let memoria = picco("table.sort", righe(&larga), byte_in, false);
    let spill = picco("table.sort", righe(&larga), byte_in, true);
    assert!(spill < memoria, "{spill} >= {memoria}");
    let budget = byte_in + spill + 1;
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
    let esito = esegui(&pipeline, &tabelle).expect("variante spilled");
    let passo = &esito.report.passi[0];
    assert_eq!(passo.variante, Variante::Spill);
    assert!(passo.byte_vivi_con_uscita <= budget);
    assert_eq!(esito.outputs, riferimento(&pipeline, &tabelle).outputs);

    // Sotto la somma di input e picco spilled: rifiuto prima di eseguire.
    let mut stretto = pipeline;
    stretto.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(budget - 2),
        ..LimitiParziali::default()
    });
    let errore = esegui(&stretto, &tabelle).expect_err("oltre il budget");
    assert!(
        matches!(&errore, PlenoraError::ResourceLimit(messaggio)
            if messaggio.contains("passo `x`") && messaggio.contains("table.sort")),
        "{errore}"
    );
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
        assert!(passo.sfrattati.is_empty());
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

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// A ogni confine di passo i byte vivi stanno nel budget, il picco
    /// previsto copre i byte nuovi dell'output e l'output e' quello del
    /// percorso senza sfratti; oppure il piano si rifiuta con
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
        // costringere a sfratti e spill.
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

/// Dizionario con metadati di campo, non nullo.
fn dizionario() -> RecordBatch {
    use plenora_core::arrow::array::types::Int32Type;
    use plenora_core::arrow::array::{DictionaryArray, Int32Array};
    let valori: ArrayRef = Arc::new(StringArray::from(vec!["uno", "due", "tre"]));
    let chiavi = Int32Array::from((0..300).map(|riga| riga % 3).collect::<Vec<i32>>());
    let colonna: ArrayRef =
        Arc::new(DictionaryArray::<Int32Type>::try_new(chiavi, valori).expect("dizionario"));
    let campo = Field::new("d", colonna.data_type().clone(), false)
        .with_metadata([("unita".to_owned(), "nessuna".to_owned())].into());
    RecordBatch::try_new(Arc::new(Schema::new(vec![campo])), vec![colonna]).expect("tabella")
}

/// Tabelle fredde di tipi diversi (metadati di schema e di campo, null,
/// liste, struct, dizionari), output del piano senza consumatori: il sort
/// le sfratta tutte e la fine le rilegge identiche.
fn piano_con_fredde(max_temp_bytes: Option<u64>) -> (Pipeline, Vec<(&'static str, RecordBatch)>) {
    let caldo = tabella(60_000, 8, 13);
    let fredde = [
        ("larga", comune::wide(true)),
        ("annidata", comune::nested()),
        ("dizionario", dizionario()),
    ];
    let byte_fredde = byte_vivi(fredde.iter().map(|(_, tabella)| tabella)).expect("byte");
    let budget = byte(&caldo) + picco("table.sort", righe(&caldo), byte(&caldo), false) + 1;
    assert!(byte_fredde > 1);
    let mut pipeline = piano(
        &["caldo", "larga", "annidata", "dizionario"],
        vec![passo(
            "ordinato",
            "table.sort",
            &["caldo"],
            json!({"columns": ["k", "s"]}),
        )],
        &["ordinato", "larga", "annidata", "dizionario"],
        budget,
    );
    pipeline.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(budget),
        max_temp_bytes,
        ..LimitiParziali::default()
    });
    let mut tabelle = vec![("caldo", caldo)];
    tabelle.extend(fredde);
    (pipeline, tabelle)
}

#[test]
fn lo_sfratto_conserva_tipi_annidati_dizionari_e_metadati() {
    let (pipeline, tabelle) = piano_con_fredde(None);
    let esito = esegui(&pipeline, &tabelle).expect("nel budget con lo sfratto");
    assert_eq!(
        esito.report.passi[0].sfrattati.len() + esito.report.ricaricati_alla_fine.len(),
        6,
        "{:?}",
        esito.report
    );
    for (nome, tabella) in &esito.outputs {
        if let Some((_, originale)) = tabelle.iter().find(|(input, _)| input == nome) {
            assert_eq!(tabella, originale, "{nome}");
            assert_eq!(tabella.schema(), originale.schema(), "{nome}");
        }
    }
    assert_eq!(esito.outputs, riferimento(&pipeline, &tabelle).outputs);
}

#[test]
fn senza_quota_su_disco_non_si_sfratta_e_il_passo_si_rifiuta_prima() {
    // Quota di un byte: nessuna tabella fredda ci sta, e il rifiuto arriva
    // dalla scelta, prima di scrivere, non da uno sfratto a meta'.
    let (pipeline, tabelle) = piano_con_fredde(Some(1));
    let errore = esegui(&pipeline, &tabelle).expect_err("oltre il budget");
    assert!(
        matches!(&errore, PlenoraError::ResourceLimit(messaggio)
            if messaggio.contains("passo `ordinato`") && messaggio.contains("previsti")),
        "{errore}"
    );
}

/// Una colonna dizionario con pochi indici e valori grandi: i valori non si
/// affettano con le righe, e la scrittura IPC li codifica interi.
fn dizionario_grande(valori: usize) -> RecordBatch {
    use plenora_core::arrow::array::types::Int32Type;
    use plenora_core::arrow::array::{DictionaryArray, Int32Array};
    let testi: ArrayRef = Arc::new(StringArray::from_iter_values(
        (0..valori).map(|indice| format!("{indice:0100}")),
    ));
    let chiavi = Int32Array::from(vec![0, 1, 2]);
    let colonna: ArrayRef =
        Arc::new(DictionaryArray::<Int32Type>::try_new(chiavi, testi).expect("dizionario"));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "d",
            colonna.data_type().clone(),
            false,
        )])),
        vec![colonna],
    )
    .expect("tabella")
}

#[test]
fn il_transitorio_dello_sfratto_di_un_dizionario_conta_nel_budget() {
    // `freddo` va sfrattato perche' il sort stia, ma la sua scrittura
    // codifica i valori del dizionario interi (circa due volte, per la
    // crescita del vettore): con `freddo` ancora residente non ci stanno, e
    // il passo si rifiuta prima di scrivere invece di sforare.
    let caldo = tabella(60_000, 8, 14);
    let picco_sort = picco("table.sort", righe(&caldo), byte(&caldo), false);
    // Valori per circa meta' del picco: gli input iniziali stanno nel budget.
    let freddo = dizionario_grande(usize::try_from(picco_sort / 220).expect("valori"));
    assert!(3 * byte(&freddo) > picco_sort);
    let budget = byte(&caldo) + picco_sort + 1;
    assert!(byte(&caldo) + byte(&freddo) <= budget);
    assert!(byte(&caldo) + byte(&freddo) + picco_sort > budget);
    let pipeline = piano(
        &["caldo", "freddo"],
        vec![passo(
            "ordinato",
            "table.sort",
            &["caldo"],
            json!({"columns": ["k"]}),
        )],
        &["ordinato", "freddo"],
        budget,
    );
    let errore = esegui(&pipeline, &[("caldo", caldo), ("freddo", freddo)])
        .expect_err("transitorio oltre il budget");
    assert!(
        matches!(&errore, PlenoraError::ResourceLimit(messaggio)
            if messaggio.contains("passo `ordinato`") && messaggio.contains("previsti")),
        "{errore}"
    );
}

#[test]
fn uno_sfratto_che_non_libera_nulla_non_blocca_la_quota_su_disco() {
    // `alias` e' `x` rinominata: stesse allocazioni, sfrattarla non libera
    // nulla, ma e' la prima in ordine di Belady (output senza altri usi).
    // Basta sfrattare `freddo`, e la quota non basta per entrambe.
    let x = tabella(60_000, 8, 15);
    let freddo = tabella(20_000, 100, 16);
    let budget = byte(&x) + picco("table.sort", righe(&x), byte(&x), false) + 1;
    assert!(byte(&x) + byte(&freddo) + picco("table.sort", righe(&x), byte(&x), false) > budget);
    let quota = byte(&freddo) + byte(&freddo) / 2;
    assert!(quota < byte(&freddo) + byte(&x));
    let mut pipeline = piano(
        &["x", "freddo"],
        vec![
            passo(
                "alias",
                "table.rename",
                &["x"],
                json!({"renames": [{"old_name": "k", "new_name": "k2"}]}),
            ),
            passo("ordinato", "table.sort", &["x"], json!({"columns": ["k"]})),
            passo(
                "stretto",
                "table.select_columns",
                &["freddo"],
                json!({"columns": ["k"]}),
            ),
        ],
        &["alias", "ordinato", "stretto"],
        budget,
    );
    pipeline.limits = Some(LimitiParziali {
        max_governed_memory_bytes: Some(budget),
        max_temp_bytes: Some(quota),
        ..LimitiParziali::default()
    });
    let tabelle = [("x", x), ("freddo", freddo)];
    let esito = esegui(&pipeline, &tabelle).expect("sfrattando solo `freddo`");
    assert_eq!(esito.report.passi[1].sfrattati, ["freddo"]);
    assert_eq!(esito.report.passi[1].variante, Variante::InMemoria);
    assert_eq!(esito.outputs, riferimento(&pipeline, &tabelle).outputs);
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
