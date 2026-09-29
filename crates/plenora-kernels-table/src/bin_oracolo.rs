//! Oracolo di `table.bin`: la funzione com'era prima della ricerca binaria
//! sui bordi e delle etichette calcolate una volta per classe, copiata alla
//! lettera, e i confronti degli esiti completi (batch per byte e per bit,
//! errori per categoria e messaggio).

use std::sync::Arc;

use plenora_core::arrow::array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Int64Array, TimestampMillisecondArray,
    UInt64Array,
};
use proptest::prelude::*;

use super::*;
use crate::test_support::{assert_same_outcome_bits, nullable_batch};

// Copia letterale del percorso precedente: scansione di ogni classe e una
// `format!` per riga.
fn bin_riferimento(batch: &RecordBatch, config: &Bin) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    let source = batch.column(index);
    // Il double calcola i bordi a larghezza uguale; la classe la decide il
    // valore esatto, o un Int64 oltre 2^53 cadrebbe nella classe accanto.
    let celle = (0..batch.num_rows())
        .map(|row| scalar_as_numero(source.as_ref(), row))
        .collect::<Result<Vec<_>>>()?;
    let numeric = celle
        .iter()
        .map(|numero| numero.map(|(valore, _)| valore))
        .collect::<Vec<_>>();
    let edges = match &config.bins {
        Bins::Count(count) => equal_width_edges(&numeric, *count)?,
        Bins::Edges(edges) => {
            if edges.len() < 3
                || edges.len() > 101
                || edges
                    .windows(2)
                    .any(|v| !matches!(v[0].partial_cmp(&v[1]), Some(std::cmp::Ordering::Less)))
            {
                return Err(PlenoraError::InvalidPlan(
                    "bordi bin non strettamente crescenti".into(),
                ));
            }
            edges.clone()
        }
    };
    let count = edges.len() - 1;
    if config
        .labels
        .as_ref()
        .is_some_and(|labels| labels.len() != count)
    {
        return Err(PlenoraError::InvalidPlan(
            "numero labels diverso dai bin".into(),
        ));
    }
    // Con `Count` i bordi esterni sono il minimo e il massimo dei dati, presi
    // sul double: un valore esatto che vi arrotonda (2^53 + 1 sul bordo 2^53)
    // e' il minimo o il massimo, e resta nella classe esterna. Con `Edges` i
    // bordi sono del piano e valgono come scritti.
    let esterni_aperti = matches!(config.bins, Bins::Count(_));
    let values = celle
        .into_iter()
        .map(|numero| {
            numero.and_then(|(_, esatto)| {
                let rispetto = |bordo: f64| compare_bounds(esatto, NumericBound::F64(bordo));
                (0..count)
                    .find(|index| {
                        let primo = *index == 0;
                        let ultimo = *index + 1 == count;
                        let sopra = match rispetto(edges[*index]) {
                            Some(Ordering::Greater) => true,
                            Some(Ordering::Equal) => primo,
                            Some(Ordering::Less) => primo && esterni_aperti,
                            None => false,
                        };
                        let sotto = match rispetto(edges[*index + 1]) {
                            Some(Ordering::Less | Ordering::Equal) => true,
                            Some(Ordering::Greater) => ultimo && esterni_aperti,
                            None => false,
                        };
                        sopra && sotto
                    })
                    .map(|index| {
                        config.labels.as_ref().map_or_else(
                            || format!("({}, {}]", edges[index], edges[index + 1]),
                            |labels| labels[index].clone(),
                        )
                    })
            })
        })
        .collect::<Vec<_>>();
    let output = config
        .output_column
        .clone()
        .unwrap_or_else(|| format!("{}_bin", config.column));
    replace_or_append(
        batch,
        &output,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(values)),
    )
}

fn batch_di(colonna: ArrayRef) -> RecordBatch {
    nullable_batch(vec![("n", colonna)])
}

fn confronta(batch: &RecordBatch, bins: Bins, labels: Option<Vec<String>>) {
    let config = Bin {
        column: "n".into(),
        bins,
        labels,
        output_column: None,
    };
    assert_same_outcome_bits(bin(batch, &config), bin_riferimento(batch, &config));
}

/// Tutte le forme di bordi del catalogo: numero di classi (anche fuori
/// intervallo), bordi espliciti crescenti, ripetuti, decrescenti o NaN, con
/// e senza etichette (anche in numero sbagliato).
fn confronta_ogni_bordo(batch: &RecordBatch) {
    for count in [0, 1, 2, 3, 7, 20, 100, 101] {
        confronta(batch, Bins::Count(count), None);
        let etichette = (0..count).map(|indice| format!("c{indice}")).collect();
        confronta(batch, Bins::Count(count), Some(etichette));
    }
    let bordi: Vec<Vec<f64>> = vec![
        vec![0.0, 1.0, 2.0],
        vec![-1.0, -0.0, 0.5, 1.0, 1.5],
        vec![-2.5, 0.0, 1e-300, 1.5, 1e300],
        vec![0.0, 1.0],
        vec![0.0, 1.0, 1.0],
        vec![2.0, 1.0, 0.0],
        vec![0.0, f64::NAN, 2.0],
        vec![f64::MIN, 0.0, f64::MAX],
        vec![
            9_007_199_254_740_992.0,
            9_007_199_254_740_994.0,
            9_007_199_254_741_000.0,
        ],
        (0..=100).map(f64::from).collect(),
        (0..=101).map(f64::from).collect(),
    ];
    for bordi in bordi {
        let classi = bordi.len().saturating_sub(1);
        confronta(batch, Bins::Edges(bordi.clone()), None);
        let etichette = (0..classi).map(|indice| format!("e{indice}")).collect();
        confronta(batch, Bins::Edges(bordi.clone()), Some(etichette));
        confronta(batch, Bins::Edges(bordi), Some(vec!["una".into()]));
    }
}

/// Double sui bordi, appena sopra e sotto, fuori intervallo, NaN, infiniti.
fn double_avversari() -> Vec<Option<f64>> {
    let mut valori = vec![
        None,
        Some(f64::NAN),
        Some(f64::from_bits(0xfff8_0000_0000_0001)),
        Some(f64::INFINITY),
        Some(f64::NEG_INFINITY),
        Some(-0.0),
        Some(0.0),
        Some(1e-300),
        Some(f64::MIN_POSITIVE / 4.0),
        Some(f64::MAX),
        Some(f64::MIN),
    ];
    for bordo in [-2.5, -1.0, 0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 99.0, 100.0] {
        valori.push(Some(bordo));
        valori.push(Some(f64::next_up(bordo)));
        valori.push(Some(f64::next_down(bordo)));
    }
    valori
}

#[test]
fn bin_come_il_riferimento_sui_double_avversari() {
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(double_avversari()))));
    // Solo valori sui bordi del proprio intervallo, e con un solo valore.
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(vec![
        Some(0.0),
        Some(2.0),
        Some(1.0),
    ]))));
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(vec![Some(3.0); 4]))));
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(vec![Some(0.0); 2]))));
}

#[test]
fn bin_come_il_riferimento_con_bordi_calcolati_degeneri() {
    // Ampiezza che trabocca (bordi NaN) e ampiezza sotto l'ulp (bordi
    // ripetuti per arrotondamento): li' decide la scansione.
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(vec![
        Some(-1e308),
        Some(0.0),
        Some(1e308),
    ]))));
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(vec![
        Some(1e16),
        Some(1e16 + 2.0),
        Some(1e16),
    ]))));
    // Nessun valore numerico.
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(vec![
        None,
        Some(f64::NAN),
    ]))));
    confronta_ogni_bordo(&batch_di(Arc::new(Float64Array::from(
        Vec::<Option<f64>>::new(),
    ))));
}

#[test]
fn bin_come_il_riferimento_su_ogni_tipo() {
    let base = 1_i64 << 53;
    let interi = vec![
        Some(base),
        Some(base + 1),
        Some(base - 1),
        Some(-(base + 1)),
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(i64::MAX),
        Some(i64::MIN),
    ];
    let colonne: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(interi.clone())),
        Arc::new(UInt64Array::from(
            interi
                .iter()
                .map(|valore| valore.map(i64::unsigned_abs))
                .collect::<Vec<_>>(),
        )),
        Arc::new(Date32Array::from(vec![Some(0), Some(1), None, Some(-3)])),
        Arc::new(TimestampMillisecondArray::from(interi.clone())),
        Arc::new(
            Decimal128Array::from(vec![Some(150), Some(-5), None, Some(100), Some(99_999)])
                .with_precision_and_scale(20, 2)
                .expect("decimal"),
        ),
        Arc::new(StringArray::from(vec![
            Some(" 1,5"),
            Some("2"),
            None,
            Some("inf"),
            Some("NaN"),
            Some("9007199254740993"),
        ])),
        Arc::new(StringArray::from(vec![Some("1"), Some("parola")])),
        Arc::new(BooleanArray::from(vec![Some(true), Some(false)])),
    ];
    for colonna in colonne {
        confronta_ogni_bordo(&batch_di(colonna));
    }
}

#[test]
fn bin_come_il_riferimento_su_colonna_assente_e_nome_di_output() {
    let batch = batch_di(Arc::new(Float64Array::from(vec![Some(1.0), Some(2.0)])));
    let config = Bin {
        column: "assente".into(),
        bins: Bins::Count(2),
        labels: None,
        output_column: None,
    };
    assert_same_outcome_bits(bin(&batch, &config), bin_riferimento(&batch, &config));
    let config = Bin {
        column: "n".into(),
        bins: Bins::Count(2),
        labels: None,
        output_column: Some("n".into()),
    };
    assert_same_outcome_bits(bin(&batch, &config), bin_riferimento(&batch, &config));
}

/// Valori da un insieme piccolo che contiene i bordi generati.
fn valore() -> impl Strategy<Value = Option<f64>> {
    prop_oneof![
        1 => Just(None),
        1 => Just(Some(f64::NAN)),
        1 => Just(Some(f64::INFINITY)),
        1 => Just(Some(f64::NEG_INFINITY)),
        1 => Just(Some(-0.0)),
        8 => (-12_i8..13).prop_map(|valore| Some(f64::from(valore) / 4.0)),
        2 => any::<f64>().prop_map(Some),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn bin_come_il_riferimento_su_input_casuali(
        valori in prop::collection::vec(valore(), 0..40),
        bordi in prop::collection::vec(-12_i8..13, 2..8),
        count in 1_usize..12,
        espliciti in any::<bool>(),
        con_etichette in any::<bool>(),
        interi in any::<bool>(),
    ) {
        let colonna: ArrayRef = if interi {
            Arc::new(Int64Array::from(
                valori
                    .iter()
                    .map(|valore| valore.filter(|valore| valore.is_finite()).map(|valore| {
                        // Valori finiti e piccoli: il troncamento e' voluto.
                        #[allow(clippy::cast_possible_truncation)]
                        let intero = (valore * 4.0) as i64;
                        intero
                    }))
                    .collect::<Vec<_>>(),
            ))
        } else {
            Arc::new(Float64Array::from(valori))
        };
        let mut bordi = bordi.into_iter().map(|bordo| f64::from(bordo) / 2.0).collect::<Vec<_>>();
        bordi.sort_by(f64::total_cmp);
        let bins = if espliciti { Bins::Edges(bordi.clone()) } else { Bins::Count(count) };
        let classi = if espliciti { bordi.len().saturating_sub(1) } else { count };
        let labels = con_etichette.then(|| (0..classi).map(|indice| format!("l{indice}")).collect());
        let batch = batch_di(colonna);
        let config = Bin { column: "n".into(), bins, labels, output_column: Some("c".into()) };
        assert_same_outcome_bits(bin(&batch, &config), bin_riferimento(&batch, &config));
    }
}
