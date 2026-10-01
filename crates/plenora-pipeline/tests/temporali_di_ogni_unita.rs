//! Colonne temporali di ogni unita' fuori dalle operazioni su date: chiavi,
//! raggruppamenti, ordinamenti, confronti, insiemi, celle scelte e
//! `date_trunc` su `Timestamp` in secondi, millisecondi, microsecondi e
//! nanosecondi, con e senza fuso, e su `Date64`.
//!
//! Ogni caso passa dal runner (validazione, poi esecuzione) e porta l'attesa
//! scritta a mano, non letta dal codice sotto prova. I valori delle colonne
//! in micro e nanosecondi sono **distinti nella loro unita' e uguali in
//! millisecondi**: una conversione verso i millisecondi li fonderebbe, e il
//! risultato sarebbe sbagliato in silenzio (piu' corrispondenze in un join,
//! meno gruppi in un raggruppamento).

mod comune_geo;

use std::collections::BTreeMap;
use std::sync::Arc;

use comune_geo::{esegui, passo, piano};
use plenora_core::arrow::array::{
    Array, ArrayRef, Date64Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use plenora_core::ErrorCategory;
use serde_json::{json, Value};

const UNITA: [TimeUnit; 4] = [
    TimeUnit::Second,
    TimeUnit::Millisecond,
    TimeUnit::Microsecond,
    TimeUnit::Nanosecond,
];

const FUSI: [Option<&str>; 2] = [None, Some("Europe/Rome")];

/// Nanosecondi per unita'.
const fn per_unita(unita: TimeUnit) -> i128 {
    match unita {
        TimeUnit::Second => 1_000_000_000,
        TimeUnit::Millisecond => 1_000_000,
        TimeUnit::Microsecond => 1_000,
        TimeUnit::Nanosecond => 1,
    }
}

/// Base allineata al millisecondo (2024-01-31T10:20:30.123Z) e passo del
/// terzo valore: in micro e nanosecondi `b`, `b + 1` e `b + d` cadono nello
/// stesso millisecondo.
const fn base_e_passo(unita: TimeUnit) -> (i64, i64) {
    match unita {
        TimeUnit::Second => (1_706_696_430, 2),
        TimeUnit::Millisecond => (1_706_696_430_123, 2),
        TimeUnit::Microsecond => (1_706_696_430_123_000, 999),
        TimeUnit::Nanosecond => (1_706_696_430_123_000_000, 999_999),
    }
}

/// `[b, b+1, b, null, b+1, b+d]`.
fn valori(unita: TimeUnit) -> Vec<Option<i64>> {
    let (b, d) = base_e_passo(unita);
    vec![
        Some(b),
        Some(b + 1),
        Some(b),
        None,
        Some(b + 1),
        Some(b + d),
    ]
}

fn colonna_temporale(unita: TimeUnit, fuso: Option<&str>, valori: Vec<Option<i64>>) -> ArrayRef {
    let fuso: Option<Arc<str>> = fuso.map(Into::into);
    match unita {
        TimeUnit::Second => Arc::new(TimestampSecondArray::from(valori).with_timezone_opt(fuso)),
        TimeUnit::Millisecond => {
            Arc::new(TimestampMillisecondArray::from(valori).with_timezone_opt(fuso))
        }
        TimeUnit::Microsecond => {
            Arc::new(TimestampMicrosecondArray::from(valori).with_timezone_opt(fuso))
        }
        TimeUnit::Nanosecond => {
            Arc::new(TimestampNanosecondArray::from(valori).with_timezone_opt(fuso))
        }
    }
}

/// `t` temporale, `i` numero di riga, `k` chiave testuale.
fn tabella(colonna: ArrayRef) -> RecordBatch {
    let tipo = colonna.data_type().clone();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("t", tipo, true),
            Field::new("i", DataType::Int64, false),
            Field::new("k", DataType::Utf8, false),
        ])),
        vec![
            colonna,
            Arc::new(Int64Array::from(vec![0_i64, 1, 2, 3, 4, 5])),
            Arc::new(StringArray::from(vec!["a", "a", "b", "b", "a", "b"])),
        ],
    )
    .expect("tabella")
}

/// I valori nativi di una colonna temporale a 64 bit.
fn nativi(colonna: &ArrayRef) -> Vec<Option<i64>> {
    let leggi = |valore: &dyn Fn(usize) -> i64| -> Vec<Option<i64>> {
        (0..colonna.len())
            .map(|riga| (!colonna.is_null(riga)).then(|| valore(riga)))
            .collect()
    };
    let any = colonna.as_any();
    if let Some(v) = any.downcast_ref::<TimestampSecondArray>() {
        return leggi(&|riga| v.value(riga));
    }
    if let Some(v) = any.downcast_ref::<TimestampMillisecondArray>() {
        return leggi(&|riga| v.value(riga));
    }
    if let Some(v) = any.downcast_ref::<TimestampMicrosecondArray>() {
        return leggi(&|riga| v.value(riga));
    }
    if let Some(v) = any.downcast_ref::<TimestampNanosecondArray>() {
        return leggi(&|riga| v.value(riga));
    }
    if let Some(v) = any.downcast_ref::<Date64Array>() {
        return leggi(&|riga| v.value(riga));
    }
    panic!("colonna non temporale: {:?}", colonna.data_type())
}

fn interi(colonna: &ArrayRef) -> Vec<Option<i64>> {
    let v = colonna
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("int64");
    (0..v.len())
        .map(|riga| (!v.is_null(riga)).then(|| v.value(riga)))
        .collect()
}

/// Valida ed esegue un passo; l'uscita deve avere esattamente lo schema che
/// la validazione aveva promesso (parita' validazione-esecuzione).
// Config per valore: i casi la scrivono inline con `json!`.
#[allow(clippy::needless_pass_by_value)]
fn passo_validato(op: &str, config: Value, tabelle: &[RecordBatch]) -> RecordBatch {
    let nomi: Vec<&str> = ["t", "u"].into_iter().take(tabelle.len()).collect();
    let pipeline = piano(&nomi, vec![passo("x", op, &nomi, config.clone())], &["x"]);
    let schemi: Vec<(&str, SchemaRef)> = nomi
        .iter()
        .copied()
        .zip(tabelle.iter().map(RecordBatch::schema))
        .collect();
    let validata = pipeline
        .validate(&schemi)
        .unwrap_or_else(|errore| panic!("{op} {config}: validazione rifiutata: {errore}"));
    let promesso = validata.contratto("x").expect("contratto").schema.clone();
    let coppie: Vec<(&str, RecordBatch)> =
        nomi.iter().copied().zip(tabelle.iter().cloned()).collect();
    let mut esito = esegui(&pipeline, &coppie)
        .unwrap_or_else(|errore| panic!("{op} {config}: esecuzione rifiutata: {errore}"));
    let uscita = esito.outputs.remove(0).1;
    let tipi = |schema: &Schema| -> Vec<(String, DataType)> {
        schema
            .fields()
            .iter()
            .map(|campo| (campo.name().clone(), campo.data_type().clone()))
            .collect()
    };
    assert_eq!(
        tipi(&uscita.schema()),
        tipi(&promesso),
        "{op} {config}: schema emesso diverso da quello validato"
    );
    uscita
}

fn ogni_colonna(mut caso: impl FnMut(&str, TimeUnit, Option<&str>, RecordBatch)) {
    for unita in UNITA {
        for fuso in FUSI {
            let nome = format!("{unita:?}/{fuso:?}");
            caso(
                &nome,
                unita,
                fuso,
                tabella(colonna_temporale(unita, fuso, valori(unita))),
            );
        }
    }
}

#[test]
fn le_chiavi_di_gruppo_restano_distinte_nella_loro_unita() {
    ogni_colonna(|nome, unita, _, ingresso| {
        let (b, d) = base_e_passo(unita);
        // aggregate per `t`: quattro gruppi (b, b+1, b+d, null), mai fusi.
        let uscita = passo_validato(
            "table.aggregate",
            json!({"group_by": ["t"], "aggregations": [{"column": "i", "function": "count"}]}),
            std::slice::from_ref(&ingresso),
        );
        let gruppi: BTreeMap<Option<i64>, i64> = nativi(uscita.column(0))
            .into_iter()
            .zip(interi(uscita.column(1)))
            .map(|(chiave, conteggio)| (chiave, conteggio.expect("conteggio")))
            .collect();
        let attesi = BTreeMap::from([(None, 1), (Some(b), 2), (Some(b + 1), 2), (Some(b + d), 1)]);
        assert_eq!(gruppi, attesi, "{nome}: aggregate");
        assert_eq!(uscita.num_rows(), 4, "{nome}: aggregate");

        // distinct e statistics: le stesse quattro chiavi.
        let distinte = passo_validato(
            "table.distinct",
            json!({"subset": ["t"]}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(distinte.num_rows(), 4, "{nome}: distinct");
        let statistiche = passo_validato(
            "table.statistics",
            json!({"column": "i", "group_by": "t", "stats": ["count"]}),
            std::slice::from_ref(&ingresso),
        );
        // Ogni riga riceve il conteggio del proprio gruppo: due righe per b
        // e per b+1, una per b+d e per il null.
        let conteggi = statistiche
            .column(statistiche.num_columns() - 1)
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::Float64Array>()
            .expect("count Float64")
            .values()
            .to_vec();
        assert_eq!(
            conteggi,
            vec![2.0, 2.0, 2.0, 1.0, 2.0, 1.0],
            "{nome}: statistics"
        );

        // pivot con indice `t`: una riga per chiave distinta.
        let pivot = passo_validato(
            "table.pivot",
            json!({"index_col": "t", "pivot_col": "k", "value_col": "i",
                   "aggr_func": "first", "mapping": {"a": "va", "b": "vb"}}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(pivot.num_rows(), 4, "{nome}: pivot");
    });
}

#[test]
fn le_celle_scelte_restano_nel_tipo_e_nel_valore_nativi() {
    ogni_colonna(|nome, unita, _, ingresso| {
        let (b, d) = base_e_passo(unita);
        // `first`/`last`/`min`/`max` per `k`: a = righe 0,1,4; b = righe 2,3,5.
        let uscita = passo_validato(
            "table.aggregate",
            json!({"group_by": ["k"], "aggregations": [
                {"column": "t", "function": "first", "alias": "primo"},
                {"column": "t", "function": "last", "alias": "ultimo"},
                {"column": "t", "function": "min", "alias": "minimo"},
                {"column": "t", "function": "max", "alias": "massimo"}]}),
            std::slice::from_ref(&ingresso),
        );
        let tipo = ingresso.schema().field(0).data_type().clone();
        for colonna in ["primo", "ultimo", "minimo", "massimo"] {
            assert_eq!(
                uscita
                    .schema()
                    .field_with_name(colonna)
                    .expect("campo")
                    .data_type(),
                &tipo,
                "{nome}: {colonna} nel tipo d'ingresso, fuso compreso"
            );
        }
        let chiavi = uscita
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("k");
        let mut righe: BTreeMap<String, Vec<Option<i64>>> = BTreeMap::new();
        for riga in 0..uscita.num_rows() {
            let valori = ["primo", "ultimo", "minimo", "massimo"]
                .iter()
                .map(|colonna| nativi(uscita.column_by_name(colonna).expect("colonna"))[riga])
                .collect();
            righe.insert(chiavi.value(riga).to_owned(), valori);
        }
        assert_eq!(
            righe,
            BTreeMap::from([
                (
                    "a".to_owned(),
                    vec![Some(b), Some(b + 1), Some(b), Some(b + 1)]
                ),
                (
                    "b".to_owned(),
                    vec![Some(b), Some(b + d), Some(b), Some(b + d)]
                ),
            ]),
            "{nome}: celle scelte"
        );

        // `lag`/`lead` spostano la cella com'e'.
        let spostate = passo_validato(
            "table.window_function",
            json!({"column": "t", "function": "lag", "order_column": "i", "output_column": "p"}),
            std::slice::from_ref(&ingresso),
        );
        let p = spostate.column_by_name("p").expect("p");
        assert_eq!(p.data_type(), &tipo, "{nome}: lag nel tipo d'ingresso");
        assert_eq!(
            nativi(p),
            vec![None, Some(b), Some(b + 1), Some(b), None, Some(b + 1)],
            "{nome}: lag"
        );
    });
}

#[test]
fn ordinamenti_confronti_e_join_decidono_sul_valore_nativo() {
    ogni_colonna(|nome, unita, _, ingresso| {
        let (b, d) = base_e_passo(unita);
        // sort: valori in ordine, null in coda (in testa in discendente:
        // l'ordine si rovescia tutto).
        let ordinata = passo_validato(
            "table.sort",
            json!({"columns": ["t"]}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(
            nativi(ordinata.column(0)),
            vec![
                Some(b),
                Some(b),
                Some(b + 1),
                Some(b + 1),
                Some(b + d),
                None
            ],
            "{nome}: sort"
        );
        let discendente = passo_validato(
            "table.top_n",
            json!({"columns": ["t"], "n": 2, "descending": true}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(
            nativi(discendente.column(0)),
            vec![None, Some(b + d)],
            "{nome}: top_n"
        );

        // filter con un estremo numerico: il valore nativo nell'unita' della
        // colonna. `> b` tiene le righe di b+1 (due) e b+d.
        let filtrate = passo_validato(
            "table.filter",
            json!({"column": "t", "operator": ">", "value": b}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(
            interi(filtrate.column(1)),
            vec![Some(1), Some(4), Some(5)],
            "{nome}: filter"
        );
        let tra = passo_validato(
            "table.filter",
            json!({"column": "t", "operator": "between", "value": format!("{},{}", b + 1, b + d)}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(
            interi(tra.column(1)),
            vec![Some(1), Some(4), Some(5)],
            "{nome}: between"
        );

        // join della tabella con se stessa su `t`: 2x2 + 2x2 + 1 = 9 coppie.
        // Con i valori fusi in millisecondi sarebbero 5x5 = 25.
        let destra = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("rt", ingresso.schema().field(0).data_type().clone(), true),
                Field::new("ri", DataType::Int64, false),
                Field::new("rk", DataType::Utf8, false),
            ])),
            ingresso.columns().to_vec(),
        )
        .expect("destra");
        let unite = passo_validato(
            "table.join",
            json!({"left_keys": ["t"], "right_keys": ["rt"], "how": "inner"}),
            &[ingresso.clone(), destra.clone()],
        );
        assert_eq!(unite.num_rows(), 9, "{nome}: join");
        let semi = passo_validato(
            "table.semi_join",
            json!({"left_keys": ["t"], "right_keys": ["rt"]}),
            &[ingresso.clone(), destra.clone()],
        );
        assert_eq!(semi.num_rows(), 5, "{nome}: semi_join");
        // outer: la chiave fusa tiene tipo e fuso.
        let esterne = passo_validato(
            "table.join",
            json!({"left_keys": ["t"], "right_keys": ["rt"], "how": "outer"}),
            &[ingresso.clone(), destra],
        );
        assert_eq!(
            esterne
                .schema()
                .field_with_name("t")
                .expect("t")
                .data_type(),
            ingresso.schema().field(0).data_type(),
            "{nome}: chiave fusa"
        );
        assert_eq!(
            esterne.num_rows(),
            11,
            "{nome}: outer join, null mai in coppia"
        );
    });
}

#[test]
fn le_operazioni_su_insiemi_confrontano_il_valore_nativo() {
    ogni_colonna(|nome, unita, fuso, _| {
        let (b, d) = base_e_passo(unita);
        let sola = |valori: Vec<Option<i64>>| {
            let colonna = colonna_temporale(unita, fuso, valori);
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![Field::new(
                    "t",
                    colonna.data_type().clone(),
                    true,
                )])),
                vec![colonna],
            )
            .expect("tabella")
        };
        let sinistra = sola(valori(unita));
        let destra = sola(vec![Some(b + 1), None]);
        let unione = passo_validato(
            "table.union_distinct",
            json!({}),
            &[sinistra.clone(), destra.clone()],
        );
        assert_eq!(unione.num_rows(), 4, "{nome}: union_distinct");
        let comuni = passo_validato(
            "table.intersect",
            json!({}),
            &[sinistra.clone(), destra.clone()],
        );
        let mut comuni = nativi(comuni.column(0));
        comuni.sort_unstable();
        assert_eq!(comuni, vec![None, Some(b + 1)], "{nome}: intersect");
        let differenza = passo_validato("table.except", json!({}), &[sinistra, destra]);
        let mut differenza = nativi(differenza.column(0));
        differenza.sort_unstable();
        assert_eq!(differenza, vec![Some(b), Some(b + d)], "{nome}: except");
    });
}

#[test]
fn date_trunc_tronca_ogni_unita_al_millisecondo_esatto() {
    ogni_colonna(|nome, unita, fuso, ingresso| {
        let config = json!({"output_column": "s", "expression": {"kind": "function",
            "name": "date_trunc", "args": [{"kind": "literal", "value": "second"},
            {"kind": "column", "name": "t"}]}});
        if fuso.is_some() {
            // Con fuso il troncamento resta rifiutato in validazione.
            let pipeline = piano(
                &["t"],
                vec![passo("x", "table.expression", &["t"], config)],
                &["x"],
            );
            let errore = pipeline
                .validate(&[("t", ingresso.schema())])
                .expect_err("fuso rifiutato");
            assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{nome}");
            return;
        }
        let uscita = passo_validato("table.expression", config, std::slice::from_ref(&ingresso));
        let s = uscita.column_by_name("s").expect("s");
        assert_eq!(
            s.data_type(),
            &DataType::Timestamp(TimeUnit::Millisecond, None)
        );
        // Atteso: il secondo per difetto del valore nativo, in millisecondi.
        let attesi: Vec<Option<i64>> = valori(unita)
            .into_iter()
            .map(|valore| {
                valore.map(|v| {
                    let ns = i128::from(v) * per_unita(unita);
                    i64::try_from(ns.div_euclid(1_000_000_000) * 1_000).expect("ms")
                })
            })
            .collect();
        assert_eq!(nativi(s), attesi, "{nome}: date_trunc");
    });
    // Prima dell'epoca il troncamento va verso meno infinito.
    let prima = tabella(colonna_temporale(
        TimeUnit::Microsecond,
        None,
        vec![
            Some(-1),
            Some(-1_000_001),
            Some(0),
            None,
            Some(1),
            Some(999_999),
        ],
    ));
    let uscita = passo_validato(
        "table.expression",
        json!({"output_column": "s", "expression": {"kind": "function",
            "name": "date_trunc", "args": [{"kind": "literal", "value": "second"},
            {"kind": "column", "name": "t"}]}}),
        std::slice::from_ref(&prima),
    );
    assert_eq!(
        nativi(uscita.column_by_name("s").expect("s")),
        vec![Some(-1_000), Some(-2_000), Some(0), None, Some(0), Some(0)]
    );
}

#[test]
fn date_trunc_in_secondi_oltre_i_millisecondi_e_un_errore() {
    let ingresso = tabella(colonna_temporale(
        TimeUnit::Second,
        None,
        vec![Some(1), Some(i64::MAX), Some(1), None, Some(1), Some(1)],
    ));
    let pipeline = piano(
        &["t"],
        vec![passo(
            "x",
            "table.expression",
            &["t"],
            json!({"output_column": "s", "expression": {"kind": "function",
                "name": "date_trunc", "args": [{"kind": "literal", "value": "day"},
                {"kind": "column", "name": "t"}]}}),
        )],
        &["x"],
    );
    let errore = esegui(&pipeline, &[("t", ingresso)]).expect_err("fuori gamma");
    assert!(errore.to_string().contains("fuori range"), "{errore}");
    assert!(
        !errore.to_string().contains(&i64::MAX.to_string()),
        "errore senza dati"
    );
}

#[test]
fn date64_allineato_si_legge_come_data_e_disallineato_si_rifiuta() {
    const GIORNO: i64 = 86_400_000;
    let colonna: ArrayRef = Arc::new(Date64Array::from(vec![
        Some(19_753 * GIORNO),
        Some(19_754 * GIORNO),
        Some(19_753 * GIORNO),
        None,
        Some(19_754 * GIORNO),
        Some(19_755 * GIORNO),
    ]));
    let ingresso = tabella(colonna);
    let gruppi = passo_validato(
        "table.aggregate",
        json!({"group_by": ["t"], "aggregations": [{"column": "i", "function": "count"}]}),
        std::slice::from_ref(&ingresso),
    );
    assert_eq!(gruppi.num_rows(), 4);
    let celle = passo_validato(
        "table.aggregate",
        json!({"group_by": ["k"], "aggregations": [
            {"column": "t", "function": "first", "alias": "primo"},
            {"column": "t", "function": "max", "alias": "massimo"}]}),
        std::slice::from_ref(&ingresso),
    );
    assert_eq!(celle.schema().field(1).data_type(), &DataType::Date64);
    assert_eq!(celle.schema().field(2).data_type(), &DataType::Date64);
    assert_eq!(
        nativi(celle.column(2)),
        vec![Some(19_754 * GIORNO), Some(19_755 * GIORNO)]
    );
    let ordinata = passo_validato(
        "table.sort",
        json!({"columns": ["t"], "ascending": false}),
        std::slice::from_ref(&ingresso),
    );
    assert_eq!(
        nativi(ordinata.column(0)),
        vec![
            None,
            Some(19_755 * GIORNO),
            Some(19_754 * GIORNO),
            Some(19_754 * GIORNO),
            Some(19_753 * GIORNO),
            Some(19_753 * GIORNO)
        ]
    );

    // Le operazioni su date leggono un Date64 come il suo testo AAAA-MM-GG.
    let parti = passo_validato(
        "table.date_extract",
        json!({"column": "t", "parts": ["year", "month", "day"], "prefix": "p_"}),
        std::slice::from_ref(&ingresso),
    );
    // 19 753 giorni dall'epoca: 2024-01-31.
    assert_eq!(
        interi(parti.column_by_name("p_year").expect("anno"))[0],
        Some(2024)
    );
    assert_eq!(
        interi(parti.column_by_name("p_month").expect("mese"))[0],
        Some(1)
    );
    assert_eq!(
        interi(parti.column_by_name("p_day").expect("giorno"))[0],
        Some(31)
    );

    // Un Date64 con un'ora nascosta non e' una data: la chiave lo rifiuta
    // invece di scriverlo come il suo giorno (due valori diversi avrebbero
    // la stessa chiave). Il messaggio non porta il valore.
    let disallineato = tabella(Arc::new(Date64Array::from(vec![
        Some(19_753 * GIORNO),
        Some(19_753 * GIORNO + 1),
        Some(1),
        None,
        Some(2),
        Some(3),
    ])));
    let pipeline = piano(
        &["t"],
        vec![passo(
            "x",
            "table.aggregate",
            &["t"],
            json!({"group_by": ["t"], "aggregations": [{"column": "i", "function": "count"}]}),
        )],
        &["x"],
    );
    let errore = esegui(&pipeline, &[("t", disallineato)]).expect_err("disallineato");
    assert_eq!(errore.category(), ErrorCategory::Schema, "{errore}");
    assert!(
        !errore
            .to_string()
            .contains(&(19_753 * GIORNO + 1).to_string()),
        "errore senza dati: {errore}"
    );
}

#[test]
fn le_somme_di_istanti_restano_rifiutate_in_ogni_unita() {
    let mut tipi: Vec<DataType> = UNITA
        .iter()
        .map(|unita| DataType::Timestamp(*unita, None))
        .collect();
    tipi.push(DataType::Date64);
    for tipo in tipi {
        let colonna = plenora_core::arrow::array::new_null_array(&tipo, 6);
        let pipeline = piano(
            &["t"],
            vec![passo(
                "x",
                "table.aggregate",
                &["t"],
                json!({"group_by": ["k"], "aggregations": [{"column": "t", "function": "sum"}]}),
            )],
            &["x"],
        );
        let errore = pipeline
            .validate(&[("t", tabella(colonna).schema())])
            .expect_err("somma di istanti");
        assert_eq!(
            errore.category(),
            ErrorCategory::InvalidPlan,
            "{tipo:?}: {errore}"
        );
    }
}

/// La catena reale che ha trovato il difetto: `first` su un
/// `Timestamp(Microsecond, None)` letto da Parquet si rifiutava in
/// validazione come Â«non leggibile come scalare testualeÂ», benche' scegliere
/// una cella non legga il testo.
#[test]
fn first_su_microsecondi_senza_fuso_si_valida_e_si_esegue() {
    let ingresso = tabella(colonna_temporale(
        TimeUnit::Microsecond,
        None,
        valori(TimeUnit::Microsecond),
    ));
    let uscita = passo_validato(
        "table.aggregate",
        json!({"group_by": ["k"], "aggregations": [{"column": "t", "function": "first"}]}),
        std::slice::from_ref(&ingresso),
    );
    assert_eq!(
        uscita.schema().field(1).data_type(),
        &DataType::Timestamp(TimeUnit::Microsecond, None)
    );
    assert_eq!(uscita.num_rows(), 2);
}

/// Due colonne temporali di unita' diverse lette come numero nella stessa
/// espressione: il confronto sugli interi nativi sarebbe sbagliato senza
/// errore (1 secondo contro 1000 microsecondi), quindi la validazione
/// rifiuta il piano, e il kernel con la stessa regola.
#[test]
fn un_espressione_con_unita_miste_si_rifiuta_in_validazione() {
    let secondi = colonna_temporale(TimeUnit::Second, None, valori(TimeUnit::Second));
    let micro = colonna_temporale(TimeUnit::Microsecond, None, valori(TimeUnit::Microsecond));
    let ingresso = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("a", secondi.data_type().clone(), true),
            Field::new("b", micro.data_type().clone(), true),
        ])),
        vec![secondi, micro],
    )
    .expect("tabella");
    let config = json!({"output_column": "c", "expression": {"kind": "binary", "op": "greater",
        "left": {"kind": "column", "name": "a"}, "right": {"kind": "column", "name": "b"}}});
    let pipeline = piano(
        &["t"],
        vec![passo("x", "table.expression", &["t"], config.clone())],
        &["x"],
    );
    let errore = pipeline
        .validate(&[("t", ingresso.schema())])
        .expect_err("unita' miste");
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan, "{errore}");
    let kernel = plenora_kernels_table::expressions::expression(
        &ingresso,
        &serde_json::from_value(config).expect("config"),
    )
    .expect_err("unita' miste nel kernel");
    assert_eq!(kernel.category(), ErrorCategory::InvalidPlan, "{kernel}");
}

/// Due istanti a 24 secondi, `1900-08-20T21:59:35Z` (offset LMT di
/// `America/Anchorage`, -09:59:36) e `1900-08-20T21:59:59Z` (offset
/// -10:00:00): chrono li scrive entrambi `1900-08-20T11:59:59-10:00`,
/// perche' arrotonda l'offset al minuto. Nella stessa colonna, in ogni unita'.
fn coppia_lmt(unita: TimeUnit) -> ArrayRef {
    let per = match unita {
        TimeUnit::Second => 1,
        TimeUnit::Millisecond => 1_000,
        TimeUnit::Microsecond => 1_000_000,
        TimeUnit::Nanosecond => 1_000_000_000,
    };
    colonna_temporale(
        unita,
        Some("America/Anchorage"),
        vec![
            Some(-2_188_951_225 * per),
            Some(-2_188_951_201 * per),
            Some(-2_188_951_225 * per),
            None,
            Some(-2_188_951_201 * per),
            Some(-2_188_951_201 * per),
        ],
    )
}

/// Chiavi d'identita' sull'istante, non sul testo: i due istanti dell'ora
/// media locale restano distinti in group-by, distinct, statistics,
/// `add_row_number` e join.
#[test]
fn l_ora_media_locale_non_fonde_le_chiavi() {
    for unita in UNITA {
        let ingresso = tabella(coppia_lmt(unita));
        let nome = format!("{unita:?}");
        let gruppi = passo_validato(
            "table.aggregate",
            json!({"group_by": ["t"], "aggregations": [{"column": "i", "function": "count"}]}),
            std::slice::from_ref(&ingresso),
        );
        // Ordine cronologico delle chiavi: null, poi i due istanti.
        assert_eq!(
            nativi(gruppi.column(0)),
            vec![None, valori_lmt(unita, 0), valori_lmt(unita, 1)],
            "{nome}: aggregate"
        );
        assert_eq!(
            interi(gruppi.column(1)),
            vec![Some(1), Some(2), Some(3)],
            "{nome}"
        );
        let distinte = passo_validato(
            "table.distinct",
            json!({"subset": ["t"]}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(distinte.num_rows(), 3, "{nome}: distinct");
        let numerate = passo_validato(
            "table.add_row_number",
            json!({"partition_column": "t", "output_column": "n"}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(numerate.num_rows(), 6, "{nome}: add_row_number");
        let destra = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("rt", ingresso.schema().field(0).data_type().clone(), true),
                Field::new("ri", DataType::Int64, false),
                Field::new("rk", DataType::Utf8, false),
            ])),
            ingresso.columns().to_vec(),
        )
        .expect("destra");
        // 2x2 + 3x3 = 13 coppie; con i due istanti fusi 5x5 = 25.
        let unite = passo_validato(
            "table.join",
            json!({"left_keys": ["t"], "right_keys": ["rt"], "how": "inner"}),
            &[ingresso.clone(), destra],
        );
        assert_eq!(unite.num_rows(), 13, "{nome}: join");
        let pivot = passo_validato(
            "table.pivot",
            json!({"index_col": "t", "pivot_col": "k", "value_col": "i",
                   "aggr_func": "first", "mapping": {"a": "va", "b": "vb"}}),
            std::slice::from_ref(&ingresso),
        );
        assert_eq!(pivot.num_rows(), 3, "{nome}: pivot");
    }
}

fn valori_lmt(unita: TimeUnit, quale: usize) -> Option<i64> {
    nativi(&coppia_lmt(unita))[quale]
}

/// Il testo RFC 3339 di un istante con un offset ai secondi non esiste:
/// errore esplicito, mai l'offset arrotondato. Lo stesso per un
/// `output_format` che scrive l'offset in ore e minuti; `%::z` lo scrive
/// coi secondi ed e' esatto.
#[test]
fn l_ora_media_locale_non_ha_testo_rfc_3339() {
    for unita in UNITA {
        let ingresso = tabella(coppia_lmt(unita));
        for (op, config) in [
            (
                "table.type_cast",
                json!({"column": "t", "target_type": "str"}),
            ),
            (
                "table.melt",
                json!({"id_columns": ["i"], "value_columns": ["t", "k"], "type_policy": "string"}),
            ),
            (
                "table.md5_hash",
                json!({"columns": ["t"], "output_column": "h"}),
            ),
            (
                "table.timezone_convert",
                json!({"column": "t", "output_format": "%Y-%m-%d %H:%M:%S %z",
                       "source_timezone": "America/Anchorage",
                       "target_timezone": "America/Anchorage", "output_column": "f"}),
            ),
        ] {
            let pipeline = piano(&["t"], vec![passo("x", op, &["t"], config)], &["x"]);
            // La validazione accetta: l'offset e' una proprieta' della cella.
            pipeline
                .validate(&[("t", ingresso.schema())])
                .unwrap_or_else(|errore| panic!("{unita:?} {op}: {errore}"));
            let errore =
                esegui(&pipeline, &[("t", ingresso.clone())]).expect_err("offset ai secondi");
            if op == "table.timezone_convert" {
                // Per riga: le due celle LMT (righe 0 e 2), non le tre a
                // -10:00:00.
                let diagnostica = errore
                    .row_diagnostics()
                    .unwrap_or_else(|| panic!("{unita:?} {op}: {errore}"));
                assert_eq!(
                    diagnostica.counts.get("conversion.offset_precision"),
                    Some(&2),
                    "{unita:?}"
                );
                assert_eq!(
                    diagnostica
                        .examples
                        .iter()
                        .map(|esempio| esempio.source_index)
                        .collect::<Vec<_>>(),
                    vec![0, 2],
                    "{unita:?}"
                );
            } else {
                assert!(
                    errore.to_string().contains("offset del fuso"),
                    "{unita:?} {op}: {errore}"
                );
            }
        }
        let esatto = passo_validato(
            "table.timezone_convert",
            json!({"column": "t", "output_format": "%Y-%m-%dT%H:%M:%S%::z",
                   "source_timezone": "America/Anchorage",
                   "target_timezone": "America/Anchorage", "output_column": "f"}),
            std::slice::from_ref(&ingresso),
        );
        let testi = esatto
            .column_by_name("f")
            .expect("f")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("testo")
            .clone();
        assert_eq!(testi.value(0), "1900-08-20T11:59:59-09:59:36", "{unita:?}");
        assert_eq!(testi.value(1), "1900-08-20T11:59:59-10:00:00", "{unita:?}");
    }
    // Un offset a minuti interi resta scritto com'e'.
    let oggi = tabella(colonna_temporale(
        TimeUnit::Microsecond,
        Some("America/Anchorage"),
        valori(TimeUnit::Microsecond),
    ));
    passo_validato(
        "table.type_cast",
        json!({"column": "t", "target_type": "str"}),
        std::slice::from_ref(&oggi),
    );
}
