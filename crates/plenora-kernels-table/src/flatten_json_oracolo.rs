//! Oracolo di `flatten_json` in una passata.
//!
//! Il riferimento e' il percorso precedente: una pre-validazione di ogni
//! riga con `from_str::<Value>` (copiata alla lettera), poi l'appiattimento
//! dell'oracolo `oracle_flatten_json` sull'albero `Value`. Si confronta
//! l'esito intero: batch byte per byte, oppure categoria, messaggio e
//! diagnostica di riga (cause, conteggi, esempi).
//!
//! Il corpus mette ogni forma difficile (numeri oltre `f64`, interi
//! enormi, esponenti, surrogati isolati, escape invalidi, caratteri di
//! controllo, chiavi duplicate, annidamento attorno al limite di 128) in
//! ogni posizione che la passata unica attraversa in modo diverso: foglia
//! emessa, oggetto oltre `max_level`, chiave fuori da `output_columns`,
//! array emesso, array non emesso, radice array. Nelle posizioni saltate
//! `IgnoredAny` di `serde_json` accetterebbe numeri fuori intervallo e
//! surrogati isolati che `Value` rifiuta.

use std::collections::BTreeMap;
use std::sync::Arc;

use plenora_core::arrow::array::{Array, BinaryArray, RecordBatch, StringArray};
use plenora_core::arrow::schema::DataType;
use plenora_core::diagnostics::{
    RowDiagnosticExample, RowDiagnosticScope, RowDiagnostics, RowDiagnosticsCompleteness,
    ROW_DIAGNOSTICS_CONTRACT, ROW_DIAGNOSTICS_INDEX_BASIS,
};
use plenora_core::{ErrorPhase, PlenoraError, Result};
use proptest::prelude::*;
use serde_json::Value;

use super::{flatten_json, FlattenJson};
use crate::test_support::{assert_same_outcome_bits, single_column_batch};
use crate::{column_index, scalar_as_string, Limits};

/// Il percorso precedente: pre-validazione con `Value`, copiata alla
/// lettera, poi l'oracolo ad albero.
fn flatten_json_prima(
    batch: &RecordBatch,
    config: &FlattenJson,
    limits: &Limits,
) -> Result<RecordBatch> {
    const EXAMPLES_LIMIT: u64 = 10;
    if config.max_level > 5 {
        return Err(PlenoraError::InvalidPlan("max_level oltre 5".into()));
    }
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    let mut counts = BTreeMap::new();
    let mut examples = Vec::new();
    let mut observed_total = 0_u64;
    for row in 0..batch.num_rows() {
        let Some(text) = scalar_as_string(source.as_ref(), row)? else {
            continue;
        };
        let cause = match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(_)) => continue,
            Ok(_) => "json.root_not_object",
            Err(_) => "json.invalid_syntax",
        };
        observed_total = observed_total.checked_add(1).ok_or_else(|| {
            PlenoraError::Internal("overflow del conteggio diagnostico JSON".into())
        })?;
        let count = counts.entry(cause.to_owned()).or_insert(0_u64);
        *count = count
            .checked_add(1)
            .ok_or_else(|| PlenoraError::Internal("overflow del conteggio causa JSON".into()))?;
        let source_index = u64::try_from(row)
            .map_err(|_| PlenoraError::Internal("indice sorgente non rappresentabile".into()))?;
        if u64::try_from(examples.len())
            .map_err(|_| PlenoraError::Internal("troppi esempi JSON".into()))?
            < EXAMPLES_LIMIT
        {
            examples.push(RowDiagnosticExample {
                source_index,
                cause: cause.to_owned(),
                column: Some(config.column.clone()),
                key: None,
                write_state: None,
            });
        }
    }
    if observed_total > 0 {
        let report = RowDiagnostics {
            contract: ROW_DIAGNOSTICS_CONTRACT.to_owned(),
            scope: RowDiagnosticScope::Read,
            index_basis: ROW_DIAGNOSTICS_INDEX_BASIS.to_owned(),
            completeness: RowDiagnosticsCompleteness::Complete,
            knowledge_limits: None,
            observed_total,
            total: Some(observed_total),
            input_total: None,
            counts,
            examples_limit: EXAMPLES_LIMIT,
            examples_truncated: observed_total > EXAMPLES_LIMIT,
            examples,
            diagnostic_state_counts: None,
            write_outcome: None,
        };
        return Err(PlenoraError::DataMapping(
            "documenti JSON rifiutati; consultare row_diagnostics".into(),
        )
        .with_phase(ErrorPhase::Read)
        .with_row_diagnostics(report));
    }
    super::tests::oracle_flatten_json(batch, config, limits)
}

fn docs(docs: &[Option<String>]) -> RecordBatch {
    single_column_batch(
        "doc",
        Arc::new(StringArray::from(
            docs.iter().map(Option::as_deref).collect::<Vec<_>>(),
        )),
        DataType::Utf8,
        true,
    )
}

fn configs() -> Vec<FlattenJson> {
    let config = |max_level: usize, output_columns: &[&str]| FlattenJson {
        column: "doc".into(),
        prefix: String::new(),
        max_level,
        output_columns: output_columns.iter().map(|c| (*c).to_owned()).collect(),
    };
    vec![
        config(0, &[]),
        config(1, &[]),
        config(2, &[]),
        config(5, &[]),
        config(1, &["doc_k"]),
        config(3, &["doc_o.k", "doc_a"]),
        config(0, &["doc_k", "doc_o"]),
    ]
}

fn confronta(batch: &RecordBatch) {
    for config in configs() {
        assert_same_outcome_bits(
            flatten_json(batch, &config, &Limits::default()),
            flatten_json_prima(batch, &config, &Limits::default()),
        );
    }
}

/// Valori JSON (anche invalidi) che la validazione deve trattare come
/// `Value`.
fn valori_difficili() -> Vec<String> {
    let mut valori = [
        // Numeri.
        "0",
        "-0",
        "0.0",
        "-0.0",
        "1e5",
        "1E+5",
        "1e-5",
        "2.5e-3",
        "1e308",
        "1.7976931348623157e308",
        "1e309",
        "-1e309",
        "1e400",
        "1e-400",
        "4.9e-324",
        "1e-330",
        "9007199254740993",
        "18446744073709551615",
        "18446744073709551616",
        "-9223372036854775808",
        "-9223372036854775809",
        "123456789012345678901234567890",
        "0.1",
        "01",
        "1.",
        ".5",
        "-",
        "+1",
        "1e",
        "1e+",
        "NaN",
        "Infinity",
        "0x10",
        // Stringhe ed escape.
        r#""a""#,
        r#""""#,
        r#""\ud83d\ude00""#,
        r#""\ud800""#,
        r#""\udc00""#,
        r#""\ud800\u0041""#,
        r#""\ud83d""#,
        r#""\u00e8""#,
        r#""\u00""#,
        r#""\u00zz""#,
        r#""\x41""#,
        r#""\/\b\f\n\r\t\"\\""#,
        "\"\u{1}\"",
        "\"\t\"",
        "\"a\u{7f}\"",
        r#""\u0000""#,
        "\"non chiusa",
        // Letterali.
        "true",
        "false",
        "null",
        "tru",
        "nul",
        "True",
        // Contenitori.
        "[]",
        "{}",
        "[1,]",
        "[,1]",
        "{\"a\":1,}",
        "{\"a\"}",
        "{ 1:2}", // chiave non stringa
        "[1 2]",
        r#"{"k":1,"k":2}"#,
        r#"[{"k":1e400}]"#,
        r#"{"k":[1,{"x":"\ud800"}]}"#,
    ]
    .iter()
    .map(|v| (*v).to_owned())
    .collect::<Vec<_>>();
    for depth in [126_usize, 127, 128, 129] {
        valori.push(format!("{}1{}", "[".repeat(depth), "]".repeat(depth)));
        valori.push(format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth)));
    }
    valori
}

/// Ogni valore in ogni posizione della passata.
fn contesti(valore: &str) -> Vec<String> {
    vec![
        // Foglia emessa (livello 1) e radice.
        format!("{{\"k\":{valore}}}"),
        valore.to_owned(),
        // Dentro un oggetto oltre max_level 0/1/2.
        format!("{{\"o\":{{\"k\":{valore}}}}}"),
        format!("{{\"o\":{{\"p\":{{\"q\":{{\"k\":{valore}}}}}}}}}"),
        // Chiave fuori da output_columns (saltata nel parsing selettivo).
        format!("{{\"z\":{valore},\"k\":1}}"),
        // Array emesso e array saltato.
        format!("{{\"a\":[1,{valore}]}}"),
        format!("{{\"z\":[{valore}],\"k\":1}}"),
        // Radice array.
        format!("[{valore}]"),
        // Come chiave, se e' una stringa.
        format!("{{{valore}:1}}"),
        // Chiave con punto (ricaduta sull'albero) accanto al valore.
        format!("{{\"a.b\":1,\"k\":{valore}}}"),
        // Chiave ripetuta: l'ultima occorrenza sostituisce il sotto-albero.
        format!("{{\"k\":{{\"x\":1}},\"k\":{valore}}}"),
        format!("{{\"k\":{valore},\"k\":{{\"x\":1}}}}"),
        format!("{{\"o\":{{\"k\":{valore}}},\"o\":{{\"k\":2}}}}"),
        // Spazi e coda.
        format!(" {{ \"k\" : {valore} }} "),
        format!("{{\"k\":{valore}}} x"),
    ]
}

#[test]
fn corpus_difficile_documento_per_documento() {
    // Un documento per batch: accettati e rifiutati si confrontano uno a
    // uno, senza che un rifiuto nasconda l'output degli altri.
    let mut rifiutati = 0_usize;
    let mut accettati = 0_usize;
    for valore in valori_difficili() {
        for documento in contesti(&valore) {
            let batch = docs(&[Some(documento.clone())]);
            confronta(&batch);
            if serde_json::from_str::<Value>(&documento).is_ok_and(|v| v.is_object()) {
                accettati += 1;
            } else {
                rifiutati += 1;
            }
        }
    }
    // Il corpus esercita davvero entrambi gli esiti.
    assert!(
        accettati > 200 && rifiutati > 200,
        "{accettati} {rifiutati}"
    );
}

#[test]
fn la_chiave_ripetuta_sostituisce_il_sotto_albero() {
    // Difetto del percorso precedente: le celle della prima occorrenza
    // restavano. `{"z":1,"z":{}}` dava `doc_z = "1"`, `{"z":{"x":1},"z":2}`
    // dava anche `doc_z.x = "1"`; `Value` tiene solo l'ultima.
    let batch = docs(&[
        Some(r#"{"z":1,"z":{}}"#.into()),
        Some(r#"{"z":{"x":1},"z":2}"#.into()),
        Some(r#"{"z":[1],"z":{"y":3}}"#.into()),
    ]);
    let config = FlattenJson {
        column: "doc".into(),
        prefix: String::new(),
        max_level: 2,
        output_columns: Vec::new(),
    };
    let output = flatten_json(&batch, &config, &Limits::default()).expect("flatten");
    let colonna = |nome: &str| {
        output
            .column_by_name(nome)
            .unwrap_or_else(|| panic!("colonna {nome}"))
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8")
            .iter()
            .map(|v| v.map(ToOwned::to_owned))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        output
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect::<Vec<_>>(),
        ["doc", "doc_z", "doc_z.y"]
    );
    assert_eq!(colonna("doc_z"), [None, Some("2".into()), None]);
    assert_eq!(colonna("doc_z.y"), [None, None, Some("3".into())]);
    confronta(&batch);
}

#[test]
fn corpus_difficile_in_un_solo_batch() {
    // Tutti insieme, con null in mezzo: conteggi, cause ed esempi (primi
    // dieci) dell'errore, oppure l'output se il batch e' tutto valido.
    let mut tutti = Vec::new();
    let mut validi = Vec::new();
    for valore in valori_difficili() {
        for documento in contesti(&valore) {
            if serde_json::from_str::<Value>(&documento).is_ok_and(|v| v.is_object()) {
                validi.push(Some(documento.clone()));
            }
            tutti.push(Some(documento));
            tutti.push(None);
        }
    }
    confronta(&docs(&tutti));
    confronta(&docs(&validi));
    // Il primo rifiuto dopo molte righe valide, e nessun rifiuto dopo.
    let mut tardi = validi.clone();
    tardi.push(Some("{\"k\":1e400}".into()));
    tardi.extend(validi.iter().cloned());
    confronta(&docs(&tardi));
}

#[test]
fn sorgente_non_utf8_stesso_errore_alla_stessa_riga() {
    // Binary: il testo viene da `scalar_as_string`, che rifiuta i byte non
    // UTF-8 alla stessa riga in entrambi i percorsi, anche dopo un rifiuto
    // JSON.
    for valori in [
        vec![
            Some(&b"{\"k\":1}"[..]),
            Some(&[0xff][..]),
            Some(&b"[1]"[..]),
        ],
        vec![Some(&b"[1]"[..]), Some(&[0xff][..])],
        vec![
            Some(&b"{\"k\":1}"[..]),
            None,
            Some(&b"{\"k\":\"\\ud800\"}"[..]),
        ],
    ] {
        let batch = single_column_batch(
            "doc",
            Arc::new(BinaryArray::from(valori)),
            DataType::Binary,
            true,
        );
        assert!(batch.column(0).len() > 1);
        confronta(&batch);
    }
}

/// Frammenti con cui il generatore compone documenti quasi-JSON.
const FRAMMENTI: [&str; 32] = [
    "{",
    "}",
    "[",
    "]",
    ":",
    ",",
    "\"k\"",
    "\"o\"",
    "\"a\"",
    "\"a.b\"",
    "\"\"",
    "1",
    "-0",
    "1e400",
    "1e-400",
    "18446744073709551616",
    "2.5",
    "\"s\"",
    "\"\\ud800\"",
    "\"\\ud83d\\ude00\"",
    "\"\\u00e8\"",
    "\"\\x\"",
    "true",
    "null",
    " ",
    "\"k\":",
    "\"o\":{",
    "{\"k\":",
    "[1,",
    "01",
    "\"\u{1}\"",
    "}}",
];

fn documento() -> impl Strategy<Value = String> {
    prop::collection::vec(0..FRAMMENTI.len(), 0..24)
        .prop_map(|indici| indici.into_iter().map(|i| FRAMMENTI[i]).collect::<String>())
}

/// Documento ben formato: oggetto con valori annidati dai frammenti
/// scalari, cosi' una buona parte dei casi e' accettata.
fn oggetto() -> impl Strategy<Value = String> {
    let scalare = prop::sample::select(vec![
        "1",
        "-0",
        "1e400",
        "1e-400",
        "18446744073709551616",
        "2.5",
        "\"s\"",
        "\"\\ud800\"",
        "\"\\ud83d\\ude00\"",
        "true",
        "null",
        "[]",
        "[1,2]",
        "{}",
    ]);
    let chiave = prop::sample::select(vec!["k", "o", "a", "a.b", "", "z"]);
    let foglia = (chiave.clone(), scalare).prop_map(|(k, v)| format!("\"{k}\":{v}"));
    let livello = prop::collection::vec(foglia, 0..4).prop_map(|campi| campi.join(","));
    (livello.clone(), chiave, livello).prop_map(|(esterno, k, interno)| {
        let sep = if esterno.is_empty() { "" } else { "," };
        format!("{{{esterno}{sep}\"{k}\":{{{interno}}}}}")
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn documenti_generati_come_il_percorso_precedente(
        grezzi in prop::collection::vec(documento(), 1..6),
        buoni in prop::collection::vec(oggetto(), 1..6),
    ) {
        for documento in grezzi.iter().chain(&buoni) {
            confronta(&docs(&[Some(documento.clone())]));
        }
        confronta(&docs(&buoni.iter().cloned().map(Some).collect::<Vec<_>>()));
        confronta(&docs(
            &grezzi.iter().chain(&buoni).cloned().map(Some).collect::<Vec<_>>(),
        ));
    }
}
