//! Oracolo di `table_diff`: la funzione com'era prima dell'arena unica,
//! del confronto nativo dei valori e dei buffer d'uscita diretti, copiata
//! alla lettera con i suoi aiuti (rinominati `_precedente`), e i confronti
//! degli esiti completi (batch per byte e per bit, errori per categoria e
//! messaggio).

use std::sync::Arc;

use plenora_core::arrow::array::{
    BinaryArray, Date32Array, Decimal128Array, TimestampMillisecondArray,
};
use proptest::prelude::*;

use super::*;
use crate::test_support::{assert_same_outcome_bits, nullable_batch};

// Copia letterale del percorso precedente.
struct DiffRowPrecedente {
    old_row: Option<usize>,
    new_row: Option<usize>,
    status: &'static str,
    changed: Option<String>,
    old_values: Option<String>,
}

/// Chiavi di un lato di `table_diff`, una per riga: l'indice di chiave nel
/// [`KeyInterner`] coincide con la riga, perche' una chiave ripetuta e' un
/// errore prima di diventare un indice.
///
/// Codifica binaria di [`BinaryKeyEncoder`]: stessa identita' dei byte di
/// `composite_key` sulle colonne di un lato, stessi errori alla stessa riga.
///
/// # Errors
///
/// - `InvalidPlan` con `duplicata` alla prima chiave ripetuta;
/// - gli errori di conversione dell'encoder;
/// - `Internal` se l'indice di una chiave nuova non coincide con la riga.
fn diff_side_keys_precedente(
    batch: &RecordBatch,
    indices: &[usize],
    duplicata: &'static str,
) -> Result<KeyInterner> {
    let encoder = BinaryKeyEncoder::new(batch, indices);
    let mut keys = KeyInterner::with_capacity(batch.num_rows());
    let mut key = Vec::new();
    for row in 0..batch.num_rows() {
        encoder.encode_into(row, &mut key)?;
        let (indice, nuova) = keys.inserisci(&key);
        if !nuova {
            return Err(PlenoraError::InvalidPlan(duplicata.into()));
        }
        if indice != row {
            return Err(PlenoraError::Internal(
                "table_diff: indice di chiave diverso dalla riga".into(),
            ));
        }
    }
    Ok(keys)
}

/// Riga di `other` con la chiave `indice` di `keys`, se le chiavi dei due
/// lati sono confrontabili.
fn diff_lookup_precedente(
    keys: &KeyInterner,
    indice: usize,
    other: &KeyInterner,
    confrontabili: bool,
) -> Result<Option<usize>> {
    if !confrontabili {
        return Ok(None);
    }
    let key = keys
        .chiave(indice)
        .ok_or_else(|| PlenoraError::Internal("table_diff: chiave di riga assente".into()))?;
    Ok(other.cerca(key))
}

fn diff_values_precedente(
    left: &ArrayRef,
    right: &ArrayRef,
    rows: &[DiffRowPrecedente],
) -> Result<ArrayRef> {
    if left.data_type() != right.data_type() {
        return Err(PlenoraError::Schema(format!(
            "table_diff richiede tipi Arrow identici, trovati {} e {}",
            left.data_type(),
            right.data_type()
        )));
    }
    let combined = plenora_core::arrow::select::concat::concat(&[left.as_ref(), right.as_ref()])?;
    let indices = rows
        .iter()
        .map(|row| {
            let index = if let Some(new_row) = row.new_row {
                left.len().saturating_add(new_row)
            } else {
                row.old_row.ok_or_else(|| {
                    PlenoraError::InvalidPlan("riga table_diff senza sorgente".into())
                })?
            };
            u32::try_from(index)
                .map(Some)
                .map_err(|_| PlenoraError::ResourceLimit("indice table_diff oltre u32".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(plenora_core::arrow::select::take::take(
        combined.as_ref(),
        &UInt32Array::from(indices),
        None,
    )?)
}

#[allow(clippy::too_many_lines)] // Le fasi del diff restano vicine per essere verificabili.
fn table_diff_precedente(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &TableDiff,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
        return Err(PlenoraError::InvalidPlan(
            "chiavi table_diff non valide".into(),
        ));
    }
    let left_keys = config
        .left_keys
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_keys = config
        .right_keys
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    let compare = if config.compare_columns.is_empty() {
        left.schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .filter(|name| {
                !config.left_keys.contains(name) && right.schema().index_of(name).is_ok()
            })
            .collect::<Vec<_>>()
    } else {
        config.compare_columns.clone()
    };
    // Regola condivisa sulla config, non oracolata.
    config.verifica_separatore(compare.len())?;
    let left_compare = compare
        .iter()
        .map(|name| column_index(left, name))
        .collect::<Result<Vec<_>>>()?;
    let right_compare = compare
        .iter()
        .map(|name| column_index(right, name))
        .collect::<Result<Vec<_>>>()?;
    // Chiavi binarie (`BinaryKeyEncoder`, stessa identita' dei byte di
    // `composite_key`) codificate una volta per riga in un'arena per lato;
    // confronto valori via `TextColumn` senza `scalar_as_string` per cella.
    // Stesso ordine righe in output (sorgente sinistra, poi righe solo a
    // destra), stessi errori.
    //
    // La chiave testuale porta il tipo Arrow di ogni colonna: chiavi di
    // colonne con tipi di nome diverso non coincidono mai, anche a parita'
    // di valore. La codifica binaria non porta il tipo (un Int64 e un UInt64
    // di pari valore hanno gli stessi byte), quindi il confronto fra i lati
    // e' ammesso solo se i nomi dei tipi coincidono colonna per colonna;
    // altrimenti nessuna chiave combacia, come nel percorso testuale.
    let confrontabili = left_keys
        .iter()
        .zip(&right_keys)
        .all(|(left_index, right_index)| {
            left.column(*left_index).data_type().to_string()
                == right.column(*right_index).data_type().to_string()
        });
    let left_text_columns = left_compare
        .iter()
        .map(|index| TextColumn::new(left.column(*index)))
        .collect::<Vec<_>>();
    let right_text_columns = right_compare
        .iter()
        .map(|index| TextColumn::new(right.column(*index)))
        .collect::<Vec<_>>();
    let old =
        diff_side_keys_precedente(left, &left_keys, "chiavi duplicate nella tabella sinistra")?;
    let new =
        diff_side_keys_precedente(right, &right_keys, "chiavi duplicate nella tabella destra")?;
    // Ordine delle sorgenti: prima le righe vecchie, poi quelle solo nuove.
    // Ordinare la chiave codificata metterebbe i null in testa e
    // riordinerebbe dati altrimenti stabili.
    let mut matched = Vec::with_capacity(old.len().saturating_add(new.len()));
    for row in 0..left.num_rows() {
        matched.push((
            Some(row),
            diff_lookup_precedente(&old, row, &new, confrontabili)?,
        ));
    }
    for row in 0..right.num_rows() {
        if diff_lookup_precedente(&new, row, &old, confrontabili)?.is_none() {
            matched.push((None, Some(row)));
        }
    }
    let mut rows = Vec::new();
    let mut before = String::new();
    let mut after = String::new();
    for (old_row, new_row) in matched {
        let (status, changed, old_values) = match (old_row, new_row) {
            (None, Some(_)) => ("ADDED", None, None),
            (Some(_), None) => ("DELETED", None, None),
            (Some(old_row), Some(new_row)) => {
                let mut changed = Vec::new();
                let mut old_values = Vec::new();
                for ((name, left_column), right_column) in compare
                    .iter()
                    .zip(&left_text_columns)
                    .zip(&right_text_columns)
                {
                    before.clear();
                    after.clear();
                    let has_before = left_column.write_value(old_row, &mut before)?;
                    let has_after = right_column.write_value(new_row, &mut after)?;
                    if has_before != has_after || (has_before && before != after) {
                        changed.push(name.clone());
                        old_values.push(before.clone());
                    }
                }
                if changed.is_empty() {
                    ("UNCHANGED", None, None)
                } else {
                    (
                        "MODIFIED",
                        Some(changed.join(config.separatore())),
                        Some(old_values.join(config.separatore())),
                    )
                }
            }
            (None, None) => {
                // `matched` e' costruito solo con una sorgente `Some` a
                // sinistra o a destra: la coppia (None, None) non e'
                // producibile; invariante interna, errore esplicito invece
                // di un panico.
                return Err(PlenoraError::Internal(
                    "table_diff: riga senza sorgente in nessuno dei due batch".into(),
                ));
            }
        };
        if status != "UNCHANGED" || config.include_unchanged == IncludeUnchanged::Yes {
            rows.push(DiffRowPrecedente {
                old_row,
                new_row,
                status,
                changed,
                old_values,
            });
        }
    }
    if rows.len() > limits.max_rows {
        return Err(PlenoraError::ResourceLimit(
            "table_diff supera max_rows".into(),
        ));
    }
    let output_count = config
        .right_keys
        .len()
        .saturating_add(compare.len())
        .saturating_add(3);
    if output_count > limits.max_columns {
        return Err(PlenoraError::ResourceLimit(
            "table_diff supera max_columns".into(),
        ));
    }
    let mut fields = Vec::new();
    let mut columns: Vec<Arc<dyn plenora_core::arrow::array::Array>> = Vec::new();
    for (position, name) in config.left_keys.iter().enumerate() {
        let left_column = left.column(left_keys[position]);
        let right_column = right.column(right_keys[position]);
        fields.push(
            left.schema()
                .field(left_keys[position])
                .as_ref()
                .clone()
                .with_name(name)
                .with_nullable(true),
        );
        columns.push(diff_values_precedente(left_column, right_column, &rows)?);
    }
    for (position, name) in compare.iter().enumerate() {
        let left_column = left.column(left_compare[position]);
        let right_column = right.column(right_compare[position]);
        fields.push(
            right
                .schema()
                .field(right_compare[position])
                .as_ref()
                .clone()
                .with_name(name)
                .with_nullable(true),
        );
        columns.push(diff_values_precedente(left_column, right_column, &rows)?);
    }
    for (name, selector) in [
        ("_diff_status", 0_usize),
        ("_diff_columns", 1),
        ("_diff_old_values", 2),
    ] {
        fields.push(Field::new(
            name,
            DataType::Utf8,
            selector == 1 || selector == 2,
        ));
        // StringBuilder diretto: nessun clone di String per riga.
        let mut builder = StringBuilder::with_capacity(
            rows.len(),
            rows.len().saturating_mul(8).min(64 * 1024 * 1024),
        );
        for row in &rows {
            let value = match selector {
                0 => Some(row.status),
                1 => row.changed.as_deref(),
                _ => row.old_values.as_deref(),
            };
            match value {
                Some(value) => builder.append_value(value),
                None => builder.append_null(),
            }
        }
        columns.push(Arc::new(builder.finish()));
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

fn config(chiavi: &[&str], confronto: &[&str], unchanged: &str, separator: &str) -> TableDiff {
    TableDiff {
        left_keys: chiavi.iter().map(|nome| (*nome).to_owned()).collect(),
        right_keys: chiavi.iter().map(|nome| (*nome).to_owned()).collect(),
        compare_columns: confronto.iter().map(|nome| (*nome).to_owned()).collect(),
        include_unchanged: IncludeUnchanged::try_from(unchanged.to_owned())
            .unwrap_or(IncludeUnchanged::No),
        // Il default `#` vale assente: la regola su `separator` scritto con
        // una sola colonna confrontata non toglie quei casi all'oracolo.
        separator: (separator != "#").then(|| separator.into()),
    }
}

fn confronta(left: &RecordBatch, right: &RecordBatch, config: &TableDiff, limits: &Limits) {
    assert_same_outcome_bits(
        table_diff(left, right, config, limits),
        table_diff_precedente(left, right, config, limits),
    );
}

/// Double che il testo accomuna o distingue: NaN di payload e segni
/// diversi (stesso testo), zeri con segno (testi diversi), infiniti.
const DOUBLE: [Option<f64>; 8] = [
    Some(f64::NAN),
    Some(-0.0),
    Some(0.0),
    None,
    Some(f64::INFINITY),
    Some(1.5),
    Some(f64::NEG_INFINITY),
    Some(0.1),
];

fn double_alternativi(indice: usize) -> Option<f64> {
    match indice % 8 {
        0 => Some(f64::from_bits(0xfff8_0000_0000_0001)),
        1 => Some(0.0),
        2 => Some(-1.0),
        3 => Some(2.0),
        4 => None,
        5 => Some(1.5),
        6 => Some(f64::NEG_INFINITY),
        _ => Some(0.1 + 1e-17),
    }
}

/// Lato di un diff con una colonna per tipo confrontato; `variante` cambia
/// i valori in modo diverso per colonna.
// Una colonna per tipo, elencata per esteso: spezzarla nasconderebbe i valori.
#[allow(clippy::too_many_lines)]
fn lato(chiavi: &[i64], variante: usize) -> RecordBatch {
    let righe = chiavi.len();
    let valore = |riga: usize| (riga + variante) % 8;
    nullable_batch(vec![
        (
            "id",
            Arc::new(Int64Array::from(
                chiavi
                    .iter()
                    .map(|chiave| Some(*chiave))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "tag",
            Arc::new(StringArray::from(
                chiavi
                    .iter()
                    .map(|chiave| Some(format!("k{}", chiave % 3)))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "f",
            Arc::new(Float64Array::from(
                (0..righe)
                    .map(|riga| {
                        if variante.is_multiple_of(2) {
                            DOUBLE[valore(riga)]
                        } else {
                            double_alternativi(valore(riga))
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "i",
            Arc::new(Int64Array::from(
                (0..righe)
                    .map(|riga| {
                        (valore(riga) != 3).then(|| i64::try_from(valore(riga) / 2).unwrap_or(0))
                    })
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "u",
            Arc::new(UInt64Array::from(
                (0..righe)
                    .map(|riga| {
                        (valore(riga) != 5).then_some(u64::MAX - 1 + u64::from(riga % 2 == 0))
                    })
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "s",
            Arc::new(StringArray::from(
                (0..righe)
                    .map(|riga| match valore(riga) {
                        0 => None,
                        1 => Some(String::new()),
                        altro => Some(format!("t{}", altro / 3)),
                    })
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "b",
            Arc::new(BooleanArray::from(
                (0..righe)
                    .map(|riga| (valore(riga) != 2).then_some(valore(riga) % 3 == 0))
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "d",
            Arc::new(Date32Array::from(
                (0..righe)
                    .map(|riga| {
                        (valore(riga) != 4).then(|| i32::try_from(valore(riga) / 3).unwrap_or(0))
                    })
                    .collect::<Vec<_>>(),
            )),
        ),
        (
            "m",
            Arc::new(
                Decimal128Array::from(
                    (0..righe)
                        .map(|riga| {
                            (valore(riga) != 6)
                                .then(|| i128::try_from(valore(riga) / 2).unwrap_or(0) * 10)
                        })
                        .collect::<Vec<_>>(),
                )
                .with_precision_and_scale(20, 1)
                .expect("decimal"),
            ),
        ),
    ])
}

const CONFRONTI: [&[&str]; 6] = [
    &[],
    &["f"],
    &["i", "u", "s", "b"],
    &["d", "m"],
    &["f", "s", "d", "i"],
    &["assente"],
];

#[test]
fn table_diff_come_il_riferimento_su_ogni_tipo_e_forma() {
    let limits = Limits::default();
    let prima = [0_i64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    let dopo = [3_i64, 1, 20, 0, 5, 4, 11, 21, 7, 9, 8, 22, 6];
    for (sinistra, destra) in [(0, 0), (0, 1), (1, 2), (2, 3), (3, 3)] {
        let left = lato(&prima, sinistra);
        let right = lato(&dopo, destra);
        for confronto in CONFRONTI {
            for unchanged in ["yes", "no", "YES"] {
                for separator in ["#", ", ", ""] {
                    for chiavi in [&["id"][..], &["tag", "id"]] {
                        let config = config(chiavi, confronto, unchanged, separator);
                        confronta(&left, &right, &config, &limits);
                        confronta(&right, &left, &config, &limits);
                    }
                }
            }
        }
        // Lati vuoti.
        let vuoto = lato(&[], 0);
        let config = config(&["id"], &[], "yes", "#");
        confronta(&vuoto, &right, &config, &limits);
        confronta(&left, &vuoto, &config, &limits);
        confronta(&vuoto, &vuoto, &config, &limits);
    }
}

// Un caso per riga, in sequenza: spezzarlo disperderebbe l'elenco dei casi.
#[allow(clippy::too_many_lines)]
#[test]
fn table_diff_come_il_riferimento_su_chiavi_speciali_ed_errori() {
    let limits = Limits::default();
    let left = lato(&[0, 1, 2, 3], 0);
    let right = lato(&[3, 2, 1, 7], 1);
    // Chiave Float64: NaN di payload diversi sono la stessa chiave, -0 e 0
    // no; chiave con null.
    let chiavi_double = |valori: Vec<Option<f64>>, batch: &RecordBatch| {
        let mut colonne = batch
            .schema()
            .fields()
            .iter()
            .map(|campo| campo.name().clone())
            .zip(batch.columns().iter().cloned())
            .collect::<Vec<_>>();
        colonne.push((
            "kf".into(),
            Arc::new(Float64Array::from(valori)) as ArrayRef,
        ));
        nullable_batch(
            colonne
                .iter()
                .map(|(nome, colonna)| (nome.as_str(), colonna.clone()))
                .collect(),
        )
    };
    let left_f = chiavi_double(vec![Some(f64::NAN), Some(-0.0), None, Some(1.0)], &left);
    let right_f = chiavi_double(
        vec![
            Some(0.0),
            Some(f64::from_bits(0xfff8_0000_0000_0003)),
            Some(1.0),
            None,
        ],
        &right,
    );
    confronta(
        &left_f,
        &right_f,
        &config(&["kf"], &[], "yes", "#"),
        &limits,
    );
    // Chiavi ripetute a sinistra, a destra, in entrambi.
    let ripetuta = lato(&[0, 1, 1, 2], 0);
    for (a, b) in [
        (&ripetuta, &right),
        (&left, &ripetuta),
        (&ripetuta, &ripetuta),
    ] {
        confronta(a, b, &config(&["id"], &[], "yes", "#"), &limits);
        confronta(a, b, &config(&["tag"], &[], "yes", "#"), &limits);
    }
    // Una chiave destra uguale a una sinistra ripetuta a destra.
    let doppia_destra = lato(&[3, 3], 0);
    confronta(
        &left,
        &doppia_destra,
        &config(&["id"], &[], "yes", "#"),
        &limits,
    );
    // Tipi di chiave con nome diverso: nessun abbinamento.
    let mut altra = config(&["id"], &[], "yes", "#");
    altra.right_keys = vec!["u".into()];
    confronta(&left, &right, &altra, &limits);
    // Chiavi non convertibili in testo, a sinistra e a destra.
    let binaria = |valori: Vec<Option<&[u8]>>| {
        nullable_batch(vec![
            ("x", Arc::new(BinaryArray::from(valori)) as ArrayRef),
            (
                "v",
                Arc::new(Int64Array::from(vec![Some(1), Some(2), Some(3)])),
            ),
        ])
    };
    let buona = binaria(vec![Some(b"a"), Some(b"b"), None]);
    let rotta = binaria(vec![Some(b"a"), Some(b"\xff"), None]);
    for (a, b) in [(&buona, &rotta), (&rotta, &buona), (&buona, &buona)] {
        confronta(a, b, &config(&["x"], &["v"], "yes", "#"), &limits);
    }
    // Timestamp fuori intervallo come colonna confrontata (percorso
    // testuale) e colonne di tipo diverso fra i lati.
    let tempo = |valori: Vec<Option<i64>>| {
        nullable_batch(vec![
            (
                "id",
                Arc::new(Int64Array::from(vec![Some(1), Some(2)])) as ArrayRef,
            ),
            ("t", Arc::new(TimestampMillisecondArray::from(valori))),
            (
                "f",
                Arc::new(Float64Array::from(vec![Some(1.0), Some(2.0)])),
            ),
        ])
    };
    confronta(
        &tempo(vec![Some(0), Some(i64::MAX)]),
        &tempo(vec![Some(1), Some(0)]),
        &config(&["id"], &["t"], "yes", "#"),
        &limits,
    );
    let testo = nullable_batch(vec![
        (
            "id",
            Arc::new(Int64Array::from(vec![Some(1), Some(2)])) as ArrayRef,
        ),
        ("f", Arc::new(StringArray::from(vec![Some("1"), Some("x")]))),
    ]);
    confronta(
        &tempo(vec![Some(0), Some(1)]),
        &testo,
        &config(&["id"], &["f"], "yes", "#"),
        &limits,
    );
    // Configurazioni e limiti.
    confronta(&left, &right, &config(&[], &[], "yes", "#"), &limits);
    let mut sbilanciata = config(&["id"], &[], "yes", "#");
    sbilanciata.right_keys.push("tag".into());
    confronta(&left, &right, &sbilanciata, &limits);
    for max_rows in 0..6 {
        let stretti = Limits {
            max_rows,
            ..Limits::default()
        };
        confronta(&left, &right, &config(&["id"], &[], "yes", "#"), &stretti);
    }
    for max_columns in 0..14 {
        let stretti = Limits {
            max_columns,
            ..Limits::default()
        };
        confronta(&left, &right, &config(&["id"], &[], "no", "#"), &stretti);
    }
}

/// Colonne Float64 abbinate riga per riga: NaN di segni e payload diversi
/// hanno lo stesso testo (nessuna modifica), gli zeri con segno no.
#[test]
fn table_diff_come_il_riferimento_sui_double_abbinati() {
    let valori_prima = [
        Some(f64::NAN),
        Some(f64::from_bits(0x7ff8_0000_0000_0001)),
        Some(f64::from_bits(0xfff8_0000_0000_0000)),
        Some(0.0),
        Some(-0.0),
        Some(f64::INFINITY),
        None,
        Some(f64::NAN),
        Some(1.0),
    ];
    let valori_dopo = [
        Some(f64::from_bits(0xfff8_0000_0000_0001)),
        Some(f64::NAN),
        Some(f64::from_bits(0x7ff0_0000_0000_0001)),
        Some(-0.0),
        Some(0.0),
        Some(f64::INFINITY),
        None,
        None,
        Some(1.0 + f64::EPSILON),
    ];
    let lato_double = |valori: &[Option<f64>]| {
        nullable_batch(vec![
            (
                "id",
                Arc::new(Int64Array::from((0..9).map(Some).collect::<Vec<_>>())) as ArrayRef,
            ),
            ("f", Arc::new(Float64Array::from(valori.to_vec()))),
        ])
    };
    let (left, right) = (lato_double(&valori_prima), lato_double(&valori_dopo));
    for unchanged in ["yes", "no"] {
        confronta(
            &left,
            &right,
            &config(&["id"], &["f"], unchanged, "#"),
            &Limits::default(),
        );
    }
    let diff = table_diff(
        &left,
        &right,
        &config(&["id"], &["f"], "no", "#"),
        &Limits::default(),
    )
    .expect("diff");
    // Cambiano solo gli zeri con segno, il valore contro null e 1 contro
    // il suo successivo.
    assert_eq!(diff.num_rows(), 4);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn table_diff_come_il_riferimento_su_input_casuali(
        sinistra in prop::collection::vec((0_i64..12, 0_usize..8), 0..14),
        destra in prop::collection::vec((0_i64..12, 0_usize..8), 0..14),
        unchanged in any::<bool>(),
        tutte in any::<bool>(),
    ) {
        let costruisci = |righe: &[(i64, usize)]| {
            let chiavi = righe.iter().map(|riga| riga.0).collect::<Vec<_>>();
            let base = lato(&chiavi, 0);
            let valori = righe
                .iter()
                .map(|riga| if riga.1 < 4 { DOUBLE[riga.1 * 2] } else { double_alternativi((riga.1 - 4) * 2) })
                .collect::<Vec<_>>();
            let testi = righe.iter().map(|riga| (riga.1 != 3).then(|| format!("v{}", riga.1 / 2))).collect::<Vec<_>>();
            nullable_batch(vec![
                ("id", base.column(0).clone()),
                ("f", Arc::new(Float64Array::from(valori)) as ArrayRef),
                ("s", Arc::new(StringArray::from(testi))),
                ("d", base.column(7).clone()),
            ])
        };
        let (left, right) = (costruisci(&sinistra), costruisci(&destra));
        let confronto: &[&str] = if tutte { &[] } else { &["f", "s"] };
        let config = config(&["id"], confronto, if unchanged { "yes" } else { "no" }, "|");
        confronta(&left, &right, &config, &Limits::default());
    }
}
