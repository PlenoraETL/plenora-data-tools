//! Oracolo delle chiavi di raggruppamento e di appartenenza: i kernel che
//! raggruppano o deduplicano su chiavi binarie danno lo stesso output del
//! percorso testuale su cui erano costruiti.
//!
//! Il riferimento e' scritto qui, indipendente dal kernel: la chiave di una
//! riga e' la sequenza dei testi di `scalar_as_string` (null distinto dal
//! testo vuoto), l'ordine dei gruppi e' quello lessicografico delle chiavi
//! testuali di `row_key` (prefisso di tipo, `0` per il null, `1{len}:{testo}`
//! per il valore). Da li' seguono gruppi e righe di `aggregate`, righe di
//! `distinct`, esito di `assert_unique`, stati di `table_diff` e righe delle
//! set operation, in memoria e nelle varianti spilled.
//!
//! I dati mescolano cio' che le chiavi binarie devono trattare come il
//! testo: NaN con payload e segni diversi (un solo `NaN`), `-0.0` distinto
//! da `0.0`, null distinto da stringa vuota, ("ab","c") distinto da
//! ("a","bc"), testi che imitano i delimitatori delle due chiavi e il
//! testo "null", testi prefissi l'uno dell'altro, Decimal128 negativi,
//! input vuoti.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, BooleanArray, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_kernels_table::aggregation::{self, Aggregate, Distinct};
use plenora_kernels_table::quality::{assert_unique, AssertUnique};
use plenora_kernels_table::reshape::{table_diff, IncludeUnchanged, TableDiff};
use plenora_kernels_table::setops::{
    except, intersect, union_distinct, SetOperation, SetOperationKind,
};
use plenora_kernels_table::spill::{aggregate_spilled, distinct_spilled, execute_set_operation};
use plenora_kernels_table::{scalar_as_string, select_rows, Limits};
use proptest::prelude::*;
use serde_json::json;

const FLOATS: [Option<f64>; 9] = [
    Some(f64::NAN),
    Some(-f64::NAN),
    Some(-0.0),
    Some(0.0),
    Some(1.5),
    Some(f64::INFINITY),
    Some(1e21),
    Some(0.1),
    None,
];

const TEXTS: [Option<&str>; 22] = [
    Some("a"),
    Some("ab"),
    Some("abc"),
    Some("b"),
    Some("bc"),
    Some("c"),
    Some(""),
    Some("\u{e4}"),
    // Testi che imitano i delimitatori della chiave testuale (`\u{1e}`,
    // `\u{1f}`, il marcatore `0` del null, il tag `{len}:`) e della chiave
    // binaria (byte 0 e 1 del marcatore, una lunghezza big-endian a 8 byte),
    // il testo "null" e testi prefissi l'uno dell'altro.
    Some("null"),
    Some("0"),
    Some("1"),
    Some("1:a"),
    Some("0\u{1f}"),
    Some("\u{1e}"),
    Some("\u{1f}"),
    Some("a\u{1f}Utf8\u{1e}1"),
    Some("\u{0}"),
    Some("\u{1}"),
    Some("\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{1}a"),
    Some("NaN"),
    Some("abcd"),
    None,
];

/// Una riga generata: pochi valori distinti per colonna (molti pareggi).
/// Il quinto campo sceglie il payload dei NaN.
type Riga = (Option<i64>, usize, usize, Option<bool>, u64, Option<i128>);

fn riga() -> impl Strategy<Value = Riga> {
    (
        proptest::option::weighted(0.85, -3_i64..4),
        0..FLOATS.len(),
        0..TEXTS.len(),
        proptest::option::weighted(0.8, any::<bool>()),
        any::<u64>(),
        proptest::option::weighted(0.8, -3_i128..4),
    )
}

/// Il NaN della riga con un payload scelto dal generatore: bit diversi,
/// stesso testo `NaN`.
fn float(riga: &Riga) -> Option<f64> {
    FLOATS[riga.1].map(|value| {
        if value.is_nan() && riga.4.is_multiple_of(2) {
            let segno = value.to_bits() & (1 << 63);
            f64::from_bits(segno | 0x7ff8_0000_0000_0000 | (riga.4 >> 13))
        } else {
            value
        }
    })
}

/// Batch con le colonne di chiave `i f s b dec` e un `id` di riga.
fn batch(righe: &[Riga]) -> RecordBatch {
    let ids = (0..righe.len())
        .map(|row| i64::try_from(row).expect("id"))
        .collect::<Vec<_>>();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("i", DataType::Int64, true),
            Field::new("f", DataType::Float64, true),
            Field::new("s", DataType::Utf8, true),
            Field::new("b", DataType::Boolean, true),
            Field::new("dec", DataType::Decimal128(10, 2), true),
            Field::new("id", DataType::Int64, false),
        ])),
        vec![
            Arc::new(Int64Array::from(
                righe.iter().map(|riga| riga.0).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                righe.iter().map(float).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                righe.iter().map(|riga| TEXTS[riga.2]).collect::<Vec<_>>(),
            )),
            Arc::new(BooleanArray::from(
                righe.iter().map(|riga| riga.3).collect::<Vec<_>>(),
            )),
            Arc::new(
                Decimal128Array::from(righe.iter().map(|riga| riga.5).collect::<Vec<_>>())
                    .with_precision_and_scale(10, 2)
                    .expect("decimal"),
            ),
            Arc::new(Int64Array::from(ids)),
        ],
    )
    .expect("batch generato")
}

fn indici(batch: &RecordBatch, nomi: &[&str]) -> Vec<usize> {
    nomi.iter()
        .map(|nome| batch.schema().index_of(nome).expect("colonna"))
        .collect()
}

/// Chiave testuale di riferimento, con gli stessi byte di `row_key`: il suo
/// ordine lessicografico e' l'ordine canonico dei gruppi.
fn chiave_testuale(batch: &RecordBatch, indici: &[usize], row: usize) -> String {
    let mut chiave = String::new();
    for indice in indici {
        let colonna = batch.column(*indice);
        chiave.push_str(&colonna.data_type().to_string());
        chiave.push('\u{1e}');
        match scalar_as_string(colonna.as_ref(), row).expect("testo di riferimento") {
            Some(testo) => {
                chiave.push('1');
                chiave.push_str(&testo.len().to_string());
                chiave.push(':');
                chiave.push_str(&testo);
            }
            None => chiave.push('0'),
        }
        chiave.push('\u{1f}');
    }
    chiave
}

fn chiavi(batch: &RecordBatch, nomi: &[&str]) -> Vec<String> {
    let indici = indici(batch, nomi);
    (0..batch.num_rows())
        .map(|row| chiave_testuale(batch, &indici, row))
        .collect()
}

fn limiti_ampi(spill_partitions: usize) -> Limits {
    Limits {
        max_governed_memory_bytes: 1 << 30,
        spill_partitions,
        ..Limits::default()
    }
}

fn errore(contesto: &str) -> impl Fn(plenora_core::PlenoraError) -> TestCaseError + '_ {
    move |error| TestCaseError::fail(format!("{contesto}: {error}"))
}

fn testi(batch: &RecordBatch, nome: &str) -> Vec<Option<String>> {
    let colonna = batch.column(batch.schema().index_of(nome).expect("colonna"));
    (0..colonna.len())
        .map(|row| scalar_as_string(colonna.as_ref(), row).expect("testo"))
        .collect()
}

/// `aggregate` atteso: gruppi nell'ordine delle chiavi testuali, colonne di
/// gruppo dal rappresentante (prima riga), righe del gruppo in ordine.
fn aggregate_atteso(input: &RecordBatch, subset: &[&str]) -> (RecordBatch, Vec<String>) {
    let mut gruppi: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (row, chiave) in chiavi(input, subset).into_iter().enumerate() {
        gruppi.entry(chiave).or_default().push(row);
    }
    let rappresentanti = gruppi.values().map(|righe| righe[0]).collect::<Vec<_>>();
    let proiezione = input.project(&indici(input, subset)).expect("proiezione");
    let chiavi_attese = select_rows(&proiezione, &rappresentanti).expect("rappresentanti");
    let righe = gruppi
        .values()
        .map(|righe| {
            righe
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    (chiavi_attese, righe)
}

fn verifica_aggregate(
    uscita: &RecordBatch,
    input: &RecordBatch,
    subset: &[&str],
) -> Result<(), TestCaseError> {
    let (chiavi_attese, righe_attese) = aggregate_atteso(input, subset);
    let colonne = (0..subset.len()).collect::<Vec<_>>();
    let chiavi_uscita = uscita.project(&colonne).expect("colonne di gruppo");
    prop_assert_eq!(chiavi_uscita.columns(), chiavi_attese.columns());
    let righe = testi(uscita, "righe")
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect::<Vec<_>>();
    prop_assert_eq!(righe, righe_attese);
    Ok(())
}

/// `distinct` atteso: righe crescenti, secondo `keep`.
fn distinct_atteso(input: &RecordBatch, subset: &[&str], keep: &str) -> Vec<usize> {
    let chiavi = chiavi(input, subset);
    let mut conteggi: HashMap<&String, usize> = HashMap::new();
    let mut ultima: HashMap<&String, usize> = HashMap::new();
    for (row, chiave) in chiavi.iter().enumerate() {
        *conteggi.entry(chiave).or_default() += 1;
        ultima.insert(chiave, row);
    }
    let mut viste = HashSet::new();
    chiavi
        .iter()
        .enumerate()
        .filter(|(row, chiave)| match keep {
            "first" => viste.insert(*chiave),
            "last" => ultima[chiave] == *row,
            _ => conteggi[chiave] == 1,
        })
        .map(|(row, _)| row)
        .collect()
}

/// Set operation attesa sulle chiavi testuali di riga intera.
fn set_operation_attesa(
    operazione: SetOperationKind,
    left: &RecordBatch,
    right: &RecordBatch,
) -> RecordBatch {
    let nomi = ["i", "f", "s", "b", "dec"];
    let sinistra = chiavi(left, &nomi);
    let destra = chiavi(right, &nomi);
    let insieme_destro = destra.iter().collect::<HashSet<_>>();
    let mut emesse = HashSet::new();
    match operazione {
        SetOperationKind::UnionDistinct => {
            let righe_sinistre = (0..sinistra.len())
                .filter(|row| emesse.insert(&sinistra[*row]))
                .collect::<Vec<_>>();
            let righe_destre = (0..destra.len())
                .filter(|row| emesse.insert(&destra[*row]))
                .collect::<Vec<_>>();
            let a = select_rows(left, &righe_sinistre).expect("sinistra");
            let b = select_rows(right, &righe_destre).expect("destra");
            plenora_kernels_table::setops::concat_compatible(&a, &b, &Limits::default())
                .expect("concat")
        }
        SetOperationKind::Intersect => {
            let selezione = (0..sinistra.len())
                .filter(|row| {
                    insieme_destro.contains(&sinistra[*row]) && emesse.insert(&sinistra[*row])
                })
                .collect::<Vec<_>>();
            select_rows(left, &selezione).expect("intersect")
        }
        SetOperationKind::Except => {
            let selezione = (0..sinistra.len())
                .filter(|row| {
                    !insieme_destro.contains(&sinistra[*row]) && emesse.insert(&sinistra[*row])
                })
                .collect::<Vec<_>>();
            select_rows(left, &selezione).expect("except")
        }
    }
}

/// Stati attesi di `table_diff` con `include_unchanged = yes`, oppure
/// `None` se un lato ha chiavi duplicate (l'errore del kernel).
fn stati_attesi(
    left: &RecordBatch,
    right: &RecordBatch,
    chiave: &[&str],
    confronto: &str,
) -> Option<Vec<&'static str>> {
    let sinistra = chiavi(left, chiave);
    let destra = chiavi(right, chiave);
    let mappa = |chiavi: &[String]| {
        let mut mappa = HashMap::new();
        for (row, chiave) in chiavi.iter().enumerate() {
            if mappa.insert(chiave.clone(), row).is_some() {
                return None;
            }
        }
        Some(mappa)
    };
    let vecchie = mappa(&sinistra)?;
    let nuove = mappa(&destra)?;
    let valori_sinistri = testi(left, confronto);
    let valori_destri = testi(right, confronto);
    let mut stati = Vec::new();
    for (row, chiave) in sinistra.iter().enumerate() {
        stati.push(match nuove.get(chiave) {
            None => "DELETED",
            Some(abbinata) if valori_sinistri[row] == valori_destri[*abbinata] => "UNCHANGED",
            Some(_) => "MODIFIED",
        });
    }
    for chiave in &destra {
        if !vecchie.contains_key(chiave) {
            stati.push("ADDED");
        }
    }
    Some(stati)
}

fn sottoinsieme() -> impl Strategy<Value = Vec<&'static str>> {
    proptest::sample::subsequence(vec!["i", "f", "s", "b", "dec"], 1..=5).prop_shuffle()
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
    #![proptest_config(ProptestConfig::with_cases(casi(24, 128)))]

    #[test]
    fn aggregate_e_distinct_hanno_l_identita_e_l_ordine_delle_chiavi_testuali(
        righe in proptest::collection::vec(riga(), 0..300),
        subset in sottoinsieme(),
        keep in proptest::sample::select(vec!["first", "last", "false"]),
        spill_partitions in 2_usize..9,
    ) {
        let input = batch(&righe);
        let limits = limiti_ampi(spill_partitions);
        let config: Aggregate = serde_json::from_value(json!({
            "group_by": subset,
            "aggregations": [
                {"column": "id", "function": "concat", "separator": "|", "alias": "righe"},
            ],
        }))
        .expect("config aggregate");
        let in_memoria = aggregation::aggregate(&input, &config)
            .map_err(errore("aggregate in memoria"))?;
        verifica_aggregate(&in_memoria, &input, &subset)?;
        if input.num_rows() > 0 {
            let (spilled, _) = aggregate_spilled(&input, &config, &limits)
                .map_err(errore("aggregate spilled"))?;
            verifica_aggregate(&spilled, &input, &subset)?;
        }

        let config: Distinct = serde_json::from_value(json!({"subset": subset, "keep": keep}))
            .expect("config distinct");
        let atteso = select_rows(&input, &distinct_atteso(&input, &subset, keep))
            .expect("distinct atteso");
        let in_memoria = aggregation::distinct(&input, &config)
            .map_err(errore("distinct in memoria"))?;
        prop_assert_eq!(&in_memoria, &atteso);
        let (spilled, _) = distinct_spilled(&input, &config, &limits)
            .map_err(errore("distinct spilled"))?;
        prop_assert_eq!(&spilled, &atteso);
    }

    #[test]
    fn assert_unique_e_table_diff_hanno_l_identita_delle_chiavi_testuali(
        sinistra in proptest::collection::vec(riga(), 0..40),
        destra in proptest::collection::vec(riga(), 0..40),
        subset in sottoinsieme(),
        nulls_equal in any::<bool>(),
        con_id in proptest::bool::weighted(0.8),
    ) {
        let left = batch(&sinistra);
        let right = batch(&destra);

        let config = AssertUnique {
            columns: subset.iter().map(|nome| (*nome).to_owned()).collect(),
            nulls_equal,
        };
        let indici_chiave = indici(&left, &subset);
        let mut viste = HashSet::new();
        let unica = (0..left.num_rows())
            .filter(|row| {
                nulls_equal
                    || indici_chiave
                        .iter()
                        .all(|indice| !left.column(*indice).is_null(*row))
            })
            .all(|row| viste.insert(chiave_testuale(&left, &indici_chiave, row)));
        prop_assert_eq!(assert_unique(&left, &config).is_ok(), unica);

        // Chiave su `subset` e, di solito, `id` della riga: con la sola
        // chiave generata i duplicati renderebbero quasi ogni caso un errore
        // (il ramo resta esercitato nei casi senza `id`); con `id` una parte
        // delle righe si abbina fra i due lati (stesso indice di riga).
        let mut chiave = subset.clone();
        if con_id {
            chiave.push("id");
        }
        let config = TableDiff {
            left_keys: chiave.iter().map(|nome| (*nome).to_owned()).collect(),
            right_keys: chiave.iter().map(|nome| (*nome).to_owned()).collect(),
            compare_columns: vec![if subset.contains(&"s") { "f".into() } else { "s".into() }],
            include_unchanged: IncludeUnchanged::Yes,
            separator: "#".into(),
        };
        let confronto = config.compare_columns[0].clone();
        let atteso = stati_attesi(&left, &right, &chiave, &confronto);
        let uscita = table_diff(&left, &right, &config, &Limits::default());
        match atteso {
            None => prop_assert!(uscita.is_err()),
            Some(stati) => {
                let uscita = uscita.map_err(errore("table_diff"))?;
                let stati_uscita = testi(&uscita, "_diff_status")
                    .into_iter()
                    .map(Option::unwrap_or_default)
                    .collect::<Vec<_>>();
                prop_assert_eq!(stati_uscita, stati);
            }
        }
    }

    #[test]
    fn set_operation_hanno_l_identita_delle_chiavi_testuali(
        sinistra in proptest::collection::vec(riga(), 0..200),
        destra in proptest::collection::vec(riga(), 0..200),
        spill_partitions in 2_usize..9,
    ) {
        // Righe intere senza `id`, che renderebbe ogni riga unica.
        let senza_id = |righe: &[Riga]| batch(righe).project(&[0, 1, 2, 3, 4]).expect("proiezione");
        let left = senza_id(&sinistra);
        let right = senza_id(&destra);
        let config = SetOperation {};
        let limits = limiti_ampi(spill_partitions);
        for (operazione, nome) in [
            (SetOperationKind::UnionDistinct, "union_distinct"),
            (SetOperationKind::Intersect, "intersect"),
            (SetOperationKind::Except, "except"),
        ] {
            let atteso = set_operation_attesa(operazione, &left, &right);
            let in_memoria = match operazione {
                SetOperationKind::UnionDistinct => {
                    union_distinct(&left, &right, &config, &Limits::default())
                }
                SetOperationKind::Intersect => intersect(&left, &right, &config),
                SetOperationKind::Except => except(&left, &right, &config),
            }
            .map_err(errore(nome))?;
            prop_assert_eq!(&in_memoria, &atteso, "{}", nome);
            let spilled = execute_set_operation(operazione, &left, &right, &limits)
                .map_err(errore(nome))?;
            prop_assert_eq!(&spilled, &atteso, "{} spilled", nome);
        }
    }
}
