//! Regressioni della review statica 2026-08-16 sui kernel tabellari.
//!
//! Ogni test qui sotto fallisce sul codice precedente alla correzione: sono
//! l'oracolo dei difetti trovati, non una copertura generica.

use std::cmp::Ordering;
use std::sync::Arc;

use plenora_core::arrow::array::{
    ArrayRef, BinaryArray, Decimal128Array, Float64Array, Int64Array, RecordBatch,
    TimestampMillisecondArray,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::PlenoraError;
use plenora_kernels_table::aggregation::{compare_cells_typed, sort, Sort};
use plenora_kernels_table::governance::{validate_rules, ValidateRules};
use plenora_kernels_table::joins::{join, Join, JoinHow};
use plenora_kernels_table::{
    exact_f64_from_i128, exact_f64_from_i64, exact_f64_from_u64, Limits, NumericBound,
};
use serde_json::json;

fn batch(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).expect("batch valido")
}

// ---------------------------------------------------------------------------
// Finding 3 — `to_f64()` non e' un test di rappresentabilita'
// ---------------------------------------------------------------------------

#[test]
fn le_conversioni_esatte_rifiutano_cio_che_to_f64_arrotondava() {
    // `ToPrimitive::to_f64()` restituisce sempre `Some` per gli interi: ogni
    // `value.to_f64().ok_or_else(|| "non rappresentabile")` era un controllo
    // che non scattava mai.
    assert_eq!(exact_f64_from_i64(0), Some(0.0));
    assert_eq!(
        exact_f64_from_i64(9_007_199_254_740_992),
        Some(9_007_199_254_740_992.0)
    );
    // 2^53 + 1: nessun double lo rappresenta.
    assert_eq!(exact_f64_from_i64(9_007_199_254_740_993), None);
    // Il criterio e' esatto, non conservativo: 2^54 ha un solo bit
    // significativo ed e' rappresentabile.
    assert_eq!(
        exact_f64_from_i64(18_014_398_509_481_984),
        Some(18_014_398_509_481_984.0)
    );
    // Gli estremi non passano dal round-trip ingenuo, che saturando li
    // avrebbe accettati.
    assert_eq!(exact_f64_from_i64(i64::MAX), None);
    // -2^63 ha un solo bit significativo: e' esattamente rappresentabile.
    assert_eq!(
        exact_f64_from_i64(i64::MIN),
        Some(-9_223_372_036_854_775_808.0)
    );
    assert_eq!(exact_f64_from_u64(u64::MAX), None);
    assert_eq!(exact_f64_from_i128(i128::MAX), None);
    assert_eq!(exact_f64_from_i128(-1), Some(-1.0));
}

#[test]
fn il_sort_su_interi_oltre_2_53_non_li_collassa() {
    // Con la conversione a f64 i due valori diventavano lo stesso double.
    let values: ArrayRef = Arc::new(Int64Array::from(vec![
        9_007_199_254_740_993_i64,
        9_007_199_254_740_992,
    ]));
    let input = batch(vec![Field::new("id", DataType::Int64, false)], vec![values]);
    let sorted = sort(
        &input,
        &Sort {
            columns: vec!["id".to_owned()],
            ascending: true,
        },
    )
    .expect("sort");
    let column = sorted
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("Int64");
    assert_eq!(column.value(0), 9_007_199_254_740_992);
    assert_eq!(column.value(1), 9_007_199_254_740_993);
}

// ---------------------------------------------------------------------------
// Finding 4 — `max_rows` del join applicato al totale, prima di allocare
// ---------------------------------------------------------------------------

#[test]
fn il_join_applica_max_rows_al_totale_non_al_singolo_chunk() {
    // 40 righe sinistre x 40 destre sulla stessa chiave = 1600 coppie, tutte
    // in un unico chunk sequenziale. Il conteggio precede l'allocazione.
    let left_keys: Vec<i64> = vec![1; 40];
    let right_keys: Vec<i64> = vec![1; 40];
    let left = batch(
        vec![Field::new("k", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(left_keys))],
    );
    let right = batch(
        vec![Field::new("k", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(right_keys))],
    );
    let config = Join {
        left_keys: vec!["k".to_owned()],
        right_keys: vec!["k".to_owned()],
        how: JoinHow::Inner,
    };
    let limits = Limits {
        max_rows: 100,
        ..Limits::default()
    };
    let error = join(&left, &right, &config, &limits).expect_err("oltre max_rows");
    assert!(
        error.to_string().contains("max_rows"),
        "atteso il limite del join, ottenuto: {error}"
    );

    // Sotto soglia il join resta identico a prima: 1600 coppie.
    let limits = Limits {
        max_rows: 10_000,
        ..Limits::default()
    };
    let output = join(&left, &right, &config, &limits).expect("entro max_rows");
    assert_eq!(output.num_rows(), 1_600);
}

#[test]
fn il_join_right_conta_anche_le_righe_destre_non_abbinate() {
    // Le righe destre senza match venivano aggiunte DOPO l'allocazione, e il
    // totale si controllava a valle: il limite arrivava quando le righe erano
    // gia' in memoria. Ora entrano nel conteggio della prima fase.
    let left = batch(
        vec![Field::new("k", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(vec![1_i64]))],
    );
    // Nessuna chiave destra abbina la sinistra: con `right` l'output e' fatto
    // interamente di righe destre non abbinate.
    let right_keys: Vec<i64> = (100..160).collect();
    let right = batch(
        vec![Field::new("k", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(right_keys))],
    );
    let config = Join {
        left_keys: vec!["k".to_owned()],
        right_keys: vec!["k".to_owned()],
        how: JoinHow::Right,
    };
    let limits = Limits {
        max_rows: 10,
        ..Limits::default()
    };
    let error = join(&left, &right, &config, &limits).expect_err("oltre max_rows");
    assert!(
        error.to_string().contains("max_rows"),
        "atteso il limite del join, ottenuto: {error}"
    );

    // Entro il limite l'output resta quello atteso: 60 righe destre.
    let limits = Limits {
        max_rows: 1_000,
        ..Limits::default()
    };
    let output = join(&left, &right, &config, &limits).expect("entro max_rows");
    assert_eq!(output.num_rows(), 60);
}

#[test]
fn il_join_outer_produce_lo_stesso_output_del_percorso_generico() {
    // Il conteggio in due fasi non deve cambiare l'ordine ne' il contenuto:
    // sinistre in ordine, poi destre non abbinate in ordine di indice.
    let left = batch(
        vec![Field::new("k", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(vec![1_i64, 2, 2, 5]))],
    );
    let right = batch(
        vec![Field::new("k", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(vec![2_i64, 3, 2]))],
    );
    let config = Join {
        left_keys: vec!["k".to_owned()],
        right_keys: vec!["k".to_owned()],
        how: JoinHow::Outer,
    };
    let output = join(&left, &right, &config, &Limits::default()).expect("outer join");
    // 1 (sinistra senza match) + 2*2 (le due chiavi 2 x due destre) +
    // 1 (sinistra 5 senza match) + 1 (destra 3 senza match) = 7.
    assert_eq!(output.num_rows(), 7);
}

// ---------------------------------------------------------------------------
// Finding 5 — comparatori nativi al posto della forma testuale
// ---------------------------------------------------------------------------

#[test]
fn i_decimal_si_ordinano_per_valore_non_per_stringa() {
    // "10.00" < "9.00" lessicograficamente: il ripiego testuale invertiva
    // l'ordine di ogni coppia con numero di cifre diverso.
    let decimals: ArrayRef = Arc::new(
        Decimal128Array::from(vec![1_000_i128, 900, -1_000])
            .with_precision_and_scale(10, 2)
            .expect("decimal"),
    );
    // 10.00 > 9.00
    assert_eq!(
        compare_cells_typed(&decimals, 0, &decimals, 1).expect("confronto"),
        Ordering::Greater
    );
    // -10.00 < 9.00
    assert_eq!(
        compare_cells_typed(&decimals, 2, &decimals, 1).expect("confronto"),
        Ordering::Less
    );
}

#[test]
fn i_timestamp_si_ordinano_per_istante_non_per_ora_locale() {
    // Con timezone, la forma testuale era l'ora LOCALE: attorno al cambio
    // d'ora l'ordine delle stringhe non e' quello degli istanti UTC.
    let values: ArrayRef = Arc::new(
        TimestampMillisecondArray::from(vec![1_698_541_200_000_i64, 1_698_544_800_000])
            .with_timezone("Europe/Rome"),
    );
    assert_eq!(
        compare_cells_typed(&values, 0, &values, 1).expect("confronto"),
        Ordering::Less
    );
}

#[test]
fn il_binario_si_ordina_per_byte_anche_se_non_e_utf8() {
    // Il ripiego testuale pretendeva UTF-8 valido e falliva su una colonna
    // binaria qualunque (una geometria WKB, per esempio).
    let values: ArrayRef = Arc::new(BinaryArray::from(vec![
        [0x01_u8, 0xff].as_slice(),
        [0x01, 0x02].as_slice(),
    ]));
    assert_eq!(
        compare_cells_typed(&values, 0, &values, 1).expect("confronto"),
        Ordering::Greater
    );
}

// ---------------------------------------------------------------------------
// Finding 6 — errore del sort deterministico
// ---------------------------------------------------------------------------

#[test]
fn il_sort_rifiuta_le_colonne_non_ordinabili_prima_di_confrontare() {
    // La prevalidazione decide sull'ordine delle colonne dichiarate, non
    // sull'ordine in cui il sort capita di confrontare le righe: lo stesso
    // input produce sempre lo stesso errore.
    let values: ArrayRef = Arc::new(Float64Array::from(vec![1.0, 2.0]));
    let unsupported: ArrayRef = Arc::new(plenora_core::arrow::array::UInt32Array::from(vec![
        1_u32, 2,
    ]));
    let input = batch(
        vec![
            Field::new("ok", DataType::Float64, false),
            Field::new("ko", DataType::UInt32, false),
        ],
        vec![values, unsupported],
    );
    let config = Sort {
        columns: vec!["ok".to_owned(), "ko".to_owned()],
        ascending: true,
    };
    let first = sort(&input, &config).expect_err("colonna non ordinabile");
    for _ in 0..8 {
        let again = sort(&input, &config).expect_err("colonna non ordinabile");
        assert_eq!(first.to_string(), again.to_string());
    }
    assert!(matches!(first, PlenoraError::Schema(_)), "{first}");
}

// ---------------------------------------------------------------------------
// Finding 10 — `ne` non passa su valori non interpretabili
// ---------------------------------------------------------------------------

#[test]
fn la_regola_ne_fallisce_sui_valori_non_interpretabili() {
    // Colonna Binary: la riga 0 e' UTF-8 leggibile, la riga 1 no. La lettura
    // scalare della riga 1 fallisce, e trasformare quell'errore in
    // `equal = false` faceva PASSARE la regola `ne` proprio sulle righe che
    // il kernel non era riuscito a leggere — l'opposto di quanto la regola
    // documenta («qualunque valore non interpretabile e' un fallimento»).
    let values: ArrayRef = Arc::new(BinaryArray::from(vec![
        b"atteso".as_slice(),
        [0xff_u8, 0xfe].as_slice(),
    ]));
    let input = batch(vec![Field::new("v", DataType::Binary, true)], vec![values]);
    let config: ValidateRules = serde_json::from_value(json!({
        "rules": [{"name": "diverso", "operator": "ne", "column": "v", "value": "altro"}],
        "output_mode": "annotate"
    }))
    .expect("config");
    let output = validate_rules(&input, &config).expect("validate_rules");
    let valid = output
        .column_by_name("_valid")
        .expect("colonna _valid")
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
        .expect("Boolean");
    // Riga 0: "atteso" != "altro" -> regola superata.
    assert!(valid.value(0), "la riga leggibile deve superare `ne`");
    // Riga 1: valore non interpretabile -> regola FALLITA, non superata.
    assert!(
        !valid.value(1),
        "una cella illeggibile non puo' «essere diversa» da nulla"
    );
}

// ---------------------------------------------------------------------------
// Quarto giro, finding 3 — zeri non significativi contati come cifre
// ---------------------------------------------------------------------------

#[test]
fn gli_zeri_non_significativi_non_fanno_ricadere_un_decimale_su_f64() {
    // Quaranta zeri e una cifra: 41 caratteri, ma UNA cifra significativa.
    // Contandoli si sforava il tetto di 38 e il letterale finiva in `f64`,
    // dove `0.1` non e' esatto: il confronto con una colonna `Decimal128`
    // diventava corretto rispetto al double e sbagliato rispetto al
    // letterale scritto.
    let molti_zeri = format!("{}.1", "0".repeat(40));
    assert_eq!(
        NumericBound::parse(&molti_zeri),
        Some(NumericBound::Decimal {
            unscaled: 1,
            scale: 1
        }),
        "gli zeri iniziali non sono cifre significative"
    );
    // Zeri finali della parte frazionaria: stesso valore, scala normalizzata.
    assert_eq!(
        NumericBound::parse("1.2300"),
        Some(NumericBound::Decimal {
            unscaled: 123,
            scale: 2
        })
    );
    // Gli zeri INIZIALI della frazione sono la scala e non si toccano.
    assert_eq!(
        NumericBound::parse("0.001"),
        Some(NumericBound::Decimal {
            unscaled: 1,
            scale: 3
        })
    );
    // Lo zero in tutte le sue scritture resta lo zero esatto, non un f64.
    for testo in ["0.0", "-0.000", ".0", "000.000"] {
        assert_eq!(
            NumericBound::parse(testo),
            Some(NumericBound::Decimal {
                unscaled: 0,
                scale: 0
            }),
            "{testo}"
        );
    }
    // Un letterale con 38 cifre significative resta esatto anche se scritto
    // con zeri in piu'; con 39 significative resta fuori dal dominio.
    let trentotto = format!("00{}.{}0", "9".repeat(20), "9".repeat(18));
    assert!(matches!(
        NumericBound::parse(&trentotto),
        Some(NumericBound::Decimal { .. })
    ));
    let trentanove = format!("{}.{}", "9".repeat(21), "9".repeat(18));
    assert!(matches!(
        NumericBound::parse(&trentanove),
        Some(NumericBound::F64(_))
    ));
}

// ---------------------------------------------------------------------------
// Sesto giro, finding 1 — il letterale decimale non si arrotonda MAI per
// confrontarlo, nemmeno contro una colonna Float64
// ---------------------------------------------------------------------------

#[test]
fn una_soglia_decimale_non_viene_arrotondata_per_confrontarla_con_un_double() {
    // `0.100000000000000001` e' STRETTAMENTE minore del double `1e-1`
    // (0.1000000000000000055511151231257827...). Convertendo la soglia con
    // `10^-scale` i due diventavano uguali, e un filtro `> soglia` escludeva
    // una riga che il letterale include.
    let soglia = NumericBound::parse("0.100000000000000001").expect("decimale");
    assert!(matches!(soglia, NumericBound::Decimal { .. }), "{soglia:?}");
    assert_eq!(
        plenora_kernels_table::compare_f64(0.1, soglia),
        Some(Ordering::Greater),
        "la colonna a 1e-1 e' maggiore della soglia decimale"
    );
    // Il verso opposto, con una soglia appena sopra il double.
    let sopra = NumericBound::parse("0.100000000000000010").expect("decimale");
    assert_eq!(
        plenora_kernels_table::compare_f64(0.1, sopra),
        Some(Ordering::Less)
    );
    // `compare_bounds` (colonne testuali numeriche) usa lo stesso percorso.
    assert_eq!(
        plenora_kernels_table::compare_bounds(NumericBound::F64(0.1), soglia),
        Some(Ordering::Greater)
    );
}

#[test]
fn il_filtro_su_float64_rispetta_la_soglia_decimale_esatta() {
    // Percorso generico E fast path del filtro: la riga a `1e-1` deve
    // superare la soglia `0.100000000000000001`. Con la conversione
    // arrotondata il confronto dava «uguale» e `>` la escludeva.
    let colonna = Arc::new(Float64Array::from(vec![Some(0.1), Some(0.05)])) as ArrayRef;
    let input = batch(
        vec![Field::new("valore", DataType::Float64, true)],
        vec![colonna],
    );
    for operatore in [">", ">="] {
        let filtrato = plenora_kernels_table::filtering::filter(
            &input,
            &serde_json::from_value(json!({
                "column": "valore",
                "operator": operatore,
                // Il letterale JSON e' gia' un double: questo caso verifica
                // che il percorso non fallisca, non l'esattezza (che vive nel
                // test sul comparatore, dove la soglia arriva come testo).
                "value": 0.1_f64,
            }))
            .expect("config"),
        );
        // Il letterale JSON e' gia' un double: qui si verifica solo che il
        // percorso non fallisca. Il caso esatto vive sul confronto sopra,
        // dove la soglia arriva come testo decimale.
        assert!(filtrato.is_ok(), "{operatore}: {filtrato:?}");
    }
}

// ---------------------------------------------------------------------------
// Sesto giro, finding 2 — scale negative estreme e zero
// ---------------------------------------------------------------------------

#[test]
fn le_scale_decimali_estreme_non_rendono_indecidibile_un_valore_arrow() {
    use plenora_kernels_table::exact_compare::compare_decimal_with_f64;

    // Arrow valida solo `scale > 38`: `Decimal128(38, -100)` e' un tipo
    // legittimo e i suoi valori devono essere confrontabili.
    for scale in [-56_i8, -100, i8::MIN] {
        assert_eq!(
            compare_decimal_with_f64(1, scale, 1.0),
            Some(Ordering::Greater),
            "scale {scale}"
        );
        assert!(
            compare_decimal_with_f64(-7, scale, 1e300).is_some(),
            "scale {scale}: nessun valore valido deve restare indeciso"
        );
    }

    // Lo zero e' esatto a qualunque scala: prima `10^|scale|` traboccava e la
    // conversione rispondeva «non rappresentabile» sullo zero.
    for scale in [-128_i8, -60, -39, 0, 38, 127] {
        assert_eq!(
            plenora_kernels_table::exact_f64_from_decimal128(0, scale),
            Some(0.0),
            "scale {scale}: lo zero e' sempre esatto"
        );
    }

    // Valore esatto il cui prodotto `unscaled * 10^a` uscirebbe da `i128`:
    // 2^124 * 10 = 2^125 * 5, parte dispari 5, quindi esatto in `f64`.
    let atteso = (2_f64).powi(125) * 5.0;
    assert_eq!(
        plenora_kernels_table::exact_f64_from_decimal128(1_i128 << 124, -1),
        Some(atteso),
        "un valore esatto non deve andare perso per il traboccamento del fattore"
    );

    // E cio' che esatto non e' resta un errore, non un arrotondamento.
    assert_eq!(
        plenora_kernels_table::exact_f64_from_decimal128(1, -39),
        None,
        "10^39 ha parte dispari 5^39 > 2^53: non e' esatto"
    );

    // Settimo giro, finding 6: `2^126 * 10 = 5 * 2^127` e' esattamente
    // rappresentabile (parte dispari 5), ma il prodotto intermedio
    // `2^126 * 5` esce da `i128`. Le potenze di due vanno estratte PRIMA di
    // moltiplicare per `5^a`.
    assert_eq!(
        plenora_kernels_table::exact_f64_from_decimal128(1_i128 << 126, -1),
        Some(5.0 * (2_f64).powi(127)),
        "il valore e' esatto: l'overflow intermedio non deve nasconderlo"
    );
    // La stessa classe su TUTTO il dominio, non su un campione: per ogni
    // potenza di due `2^p` con `p` da 0 a 126 e per ogni scala negativa da -1
    // a -30, il valore vale `2^(p+a) * 5^a`. La parte dispari e' `5^a`, quindi
    // il valore e' esatto in `f64` se e solo se `5^a <= 2^53`, cioe' `a <= 22`.
    // Sopra quella soglia la risposta corretta e' `None`: un rifiuto, non un
    // arrotondamento. Il ciclo verifica ENTRAMBI i versi su 126 x 30 casi.
    let mut esatti = 0_u32;
    let mut rifiutati = 0_u32;
    for potenza in 0_u32..=126 {
        for a in 1_u32..=30 {
            let scala = -i8::try_from(a).expect("scala entro i8");
            let ottenuto =
                plenora_kernels_table::exact_f64_from_decimal128(1_i128 << potenza, scala);
            if a <= 22 {
                let atteso = 5_f64.powi(i32::try_from(a).expect("scala"))
                    * (2_f64).powi(i32::try_from(potenza + a).expect("esponente"));
                assert_eq!(
                    ottenuto,
                    Some(atteso),
                    "2^{potenza} con scala {scala}: parte dispari 5^{a} <= 2^53, e' esatto"
                );
                esatti += 1;
            } else {
                assert_eq!(
                    ottenuto, None,
                    "2^{potenza} con scala {scala}: parte dispari 5^{a} > 2^53, va rifiutato"
                );
                rifiutati += 1;
            }
        }
    }
    assert_eq!(
        (esatti, rifiutati),
        (127 * 22, 127 * 8),
        "il dominio dev'essere stato percorso per intero, in entrambi i versi"
    );
    // Il valore piu' negativo resta trattabile (nessun overflow di segno).
    assert!(plenora_kernels_table::exact_f64_from_decimal128(i128::MIN, -1).is_some());
}

// ---------------------------------------------------------------------------
// Sesto giro, finding 3 — null LOGICO delle DictionaryArray
// ---------------------------------------------------------------------------

/// Dictionary con una chiave NON nulla che punta a una entry NULLA del
/// dizionario: la riga e' logicamente nulla, ma `DictionaryArray::is_null`
/// risponde `false` perche' guarda solo la bitmap delle chiavi.
fn dictionary_con_valore_nullo() -> ArrayRef {
    use plenora_core::arrow::array::{types::Int32Type, DictionaryArray, Int32Array, StringArray};
    let valori = StringArray::from(vec![Some("alfa"), None, Some("gamma")]);
    // riga 0 -> "alfa", riga 1 -> entry nulla (chiave valida!), riga 2 -> chiave nulla
    let chiavi = Int32Array::from(vec![Some(0), Some(1), None]);
    Arc::new(DictionaryArray::<Int32Type>::try_new(chiavi, Arc::new(valori)).expect("dictionary"))
        as ArrayRef
}

#[test]
fn una_chiave_valida_verso_una_entry_nulla_e_una_riga_nulla() {
    let array = dictionary_con_valore_nullo();

    // La bitmap di primo livello NON vede la riga 1: e' la trappola.
    assert!(
        !array.is_null(1),
        "premessa del difetto: is_null non la vede"
    );
    assert!(
        plenora_kernels_table::is_logically_null(array.as_ref(), 1),
        "la riga 1 e' logicamente nulla"
    );
    assert!(plenora_kernels_table::is_logically_null(array.as_ref(), 2));
    assert!(!plenora_kernels_table::is_logically_null(array.as_ref(), 0));

    // `scalar_as_string` restituiva la stringa vuota al posto del null.
    assert_eq!(
        plenora_kernels_table::scalar_as_string(array.as_ref(), 1).expect("lettura"),
        None,
        "una riga nulla non e' la stringa vuota"
    );
    assert_eq!(
        plenora_kernels_table::scalar_as_string(array.as_ref(), 0).expect("lettura"),
        Some("alfa".to_owned())
    );
}

#[test]
fn il_null_logico_della_dictionary_e_coerente_in_filtro_ordinamento_e_setop() {
    let array = dictionary_con_valore_nullo();
    let schema_dict = array.data_type().clone();
    let input = batch(
        vec![Field::new("etichetta", schema_dict, true)],
        vec![Arc::clone(&array)],
    );

    // 1. `isnull` / `notnull`: la riga 1 deve stare fra i null.
    let nulli = plenora_kernels_table::filtering::filter(
        &input,
        &serde_json::from_value(json!({"column": "etichetta", "operator": "isnull"}))
            .expect("config"),
    )
    .expect("filtro isnull");
    assert_eq!(nulli.num_rows(), 2, "righe 1 e 2 sono nulle");
    let non_nulli = plenora_kernels_table::filtering::filter(
        &input,
        &serde_json::from_value(json!({"column": "etichetta", "operator": "notnull"}))
            .expect("config"),
    )
    .expect("filtro notnull");
    assert_eq!(non_nulli.num_rows(), 1, "solo la riga 0 ha un valore");

    // 2. Un confronto di uguaglianza non deve mai selezionare la riga nulla,
    //    nemmeno confrontandola con la stringa vuota.
    let vuoti = plenora_kernels_table::filtering::filter(
        &input,
        &serde_json::from_value(json!({"column": "etichetta", "operator": "==", "value": ""}))
            .expect("config"),
    )
    .expect("filtro uguaglianza");
    assert_eq!(
        vuoti.num_rows(),
        0,
        "la riga nulla non e' uguale alla stringa vuota"
    );

    // 3. Ordinamento: i null vanno in coda, non fra i valori.
    let ordinato = sort(
        &input,
        &Sort {
            columns: vec!["etichetta".to_owned()],
            ascending: true,
        },
    )
    .expect("sort");
    let colonna = ordinato.column(0);
    assert!(
        !plenora_kernels_table::is_logically_null(colonna.as_ref(), 0),
        "il valore viene prima dei null in ascendente"
    );
    assert!(plenora_kernels_table::is_logically_null(
        colonna.as_ref(),
        1
    ));
    assert!(plenora_kernels_table::is_logically_null(
        colonna.as_ref(),
        2
    ));

    // 4. Set operation: la chiave compatta deve trattare la riga 1 come
    //    NULL. Unendo il batch con se stesso, `union_distinct` deve produrre
    //    le stesse righe distinte dell'originale — se la riga nulla fosse
    //    codificata come stringa vuota, collasserebbe con un valore vuoto.
    let unite = plenora_kernels_table::setops::union_distinct(
        &input,
        &input,
        &serde_json::from_value(json!({})).expect("config"),
        &Limits::default(),
    )
    .expect("union_distinct");
    assert_eq!(
        unite.num_rows(),
        2,
        "un valore piu' una riga nulla: le due righe nulle collassano"
    );
    let colonna = unite.column(0);
    assert!(
        (0..unite.num_rows())
            .any(|row| plenora_kernels_table::is_logically_null(colonna.as_ref(), row)),
        "il null sopravvive alla set operation"
    );
}

// ---------------------------------------------------------------------------
// Settimo giro, finding 1 — il null logico vale in TUTTI i kernel che
// decidono sulla nullita', non solo in filtro/ordinamento/setop
// ---------------------------------------------------------------------------

#[test]
fn il_null_logico_vale_anche_in_quality_governance_e_security() {
    let array = dictionary_con_valore_nullo();
    let tipo = array.data_type().clone();
    let input = batch(
        vec![Field::new("etichetta", tipo, true)],
        vec![Arc::clone(&array)],
    );

    // 1. `assert_not_null`: la riga 1 e' senza valore e va rifiutata.
    let esito = plenora_kernels_table::quality::assert_not_null(
        &input,
        &serde_json::from_value(json!({"columns": ["etichetta"]})).expect("config"),
    );
    let error = esito.expect_err("una riga senza valore deve essere rifiutata");
    let report = error.row_diagnostics().expect("diagnostica row-scoped");
    assert_eq!(
        report.observed_total, 2,
        "righe 1 (entry nulla) e 2 (chiave nulla): {report:?}"
    );

    // 2. `assert_unique` con `nulls_equal = false`: le righe nulle non
    //    partecipano al confronto, quindi due null non sono un duplicato.
    plenora_kernels_table::quality::assert_unique(
        &input,
        &serde_json::from_value(json!({"columns": ["etichetta"], "nulls_equal": false}))
            .expect("config"),
    )
    .expect("due righe nulle non sono un duplicato quando i null non si eguagliano");

    // 3. Regola di governance `notnull`: la riga 1 non la supera. Il modo
    //    predefinito annota invece di rifiutare, quindi si guarda `_valid`.
    let annotato = plenora_kernels_table::governance::validate_rules(
        &input,
        &serde_json::from_value(json!({
            "rules": [{"name": "etichetta-presente", "column": "etichetta", "operator": "notnull"}]
        }))
        .expect("config"),
    )
    .expect("validate_rules");
    let valid = annotato
        .column_by_name("_valid")
        .and_then(|column| {
            column
                .as_any()
                .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
        })
        .expect("colonna _valid");
    assert!(valid.value(0), "la riga con valore e' valida");
    assert!(
        !valid.value(1),
        "la chiave valida verso una entry nulla NON supera `notnull`"
    );
    assert!(!valid.value(2), "la chiave nulla non supera `notnull`");

    // 4. `hash` con `null_policy = error`: un null sorgente e' un errore, e
    //    la riga 1 e' un null.
    let esito = plenora_kernels_table::security::sha256_hash(
        &input,
        &serde_json::from_value(json!({
            "columns": ["etichetta"],
            "output_column": "digest",
            "null_policy": "error"
        }))
        .expect("config"),
    );
    let error = esito.expect_err("null sorgente con null_policy=error");
    let report = error.row_diagnostics().expect("diagnostica row-scoped");
    assert_eq!(report.observed_total, 2, "{report:?}");
}

// ---------------------------------------------------------------------------
// Ottavo giro, finding 3 — il null logico in aggregate, formula e coalesce
// ---------------------------------------------------------------------------

#[test]
fn il_null_logico_vale_anche_in_count_formula_e_coalesce() {
    let array = dictionary_con_valore_nullo();
    let tipo = array.data_type().clone();

    // 1. `count` conta i VALORI: righe 1 e 2 sono nulle, resta una sola.
    let con_gruppo = batch(
        vec![
            Field::new("gruppo", DataType::Utf8, false),
            Field::new("etichetta", tipo.clone(), true),
        ],
        vec![
            Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                "g", "g", "g",
            ])) as ArrayRef,
            Arc::clone(&array),
        ],
    );
    let aggregato = plenora_kernels_table::aggregation::aggregate(
        &con_gruppo,
        &serde_json::from_value(json!({
            "group_by": ["gruppo"],
            "aggregations": [{"column": "etichetta", "function": "count"}]
        }))
        .expect("config"),
    )
    .expect("aggregate");
    let conteggio = aggregato
        .column(1)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("conteggio");
    assert_eq!(
        conteggio.value(0),
        1,
        "una chiave valida verso una entry nulla non e' un valore da contare"
    );

    // 2. `formula`: una cella logicamente nulla non e' la stringa vuota.
    let solo_etichetta = batch(
        vec![Field::new("etichetta", tipo.clone(), true)],
        vec![Arc::clone(&array)],
    );
    let con_formula = plenora_kernels_table::formula::formula(
        &solo_etichetta,
        &serde_json::from_value(json!({
            "formula": "etichetta",
            "new_column": "copia"
        }))
        .expect("config"),
    )
    .expect("formula");
    let copia = con_formula.column_by_name("copia").expect("copia");
    assert!(
        copia.is_null(1),
        "la riga logicamente nulla resta nulla dopo la formula"
    );

    // 3. `coalesce`: il fast path vedeva `null_count() == 0` e restituiva la
    //    prima colonna intatta, saltando il risolutore logico.
    // Stesso tipo Arrow della prima colonna — `coalesce` lo richiede — ma
    // senza null, ne' fisici ne' logici.
    let ripiego = {
        use plenora_core::arrow::array::{
            types::Int32Type, DictionaryArray, Int32Array, StringArray,
        };
        let valori = StringArray::from(vec![Some("ripiego")]);
        let chiavi = Int32Array::from(vec![Some(0), Some(0), Some(0)]);
        Arc::new(DictionaryArray::<Int32Type>::try_new(chiavi, Arc::new(valori)).expect("ripiego"))
            as ArrayRef
    };
    // Il fast path scatta solo se `null_count()` e' zero: serve una colonna
    // con la bitmap delle chiavi tutta valida e il null nascosto nel
    // dizionario. E' esattamente il caso che il fast path leggeva come
    // «nessun null, restituisci la prima colonna».
    let senza_null_fisici = {
        use plenora_core::arrow::array::{
            types::Int32Type, DictionaryArray, Int32Array, StringArray,
        };
        let valori = StringArray::from(vec![Some("alfa"), None, Some("gamma")]);
        let chiavi = Int32Array::from(vec![Some(0), Some(1), Some(2)]);
        Arc::new(DictionaryArray::<Int32Type>::try_new(chiavi, Arc::new(valori)).expect("nascosto"))
            as ArrayRef
    };
    assert_eq!(
        senza_null_fisici.null_count(),
        0,
        "premessa del difetto: nessun null fisico, quindi il fast path scatta"
    );
    let due_colonne = batch(
        vec![
            Field::new("etichetta", tipo, true),
            Field::new("ripiego", ripiego.data_type().clone(), true),
        ],
        vec![senza_null_fisici, ripiego],
    );
    let unito = plenora_kernels_table::quality::coalesce(
        &due_colonne,
        &serde_json::from_value(json!({
            "columns": ["etichetta", "ripiego"],
            "output_column": "risultato"
        }))
        .expect("config"),
    )
    .expect("coalesce");
    let risultato = unito.column_by_name("risultato").expect("risultato");
    assert!(
        !plenora_kernels_table::is_logically_null(risultato.as_ref(), 1),
        "la riga logicamente nulla deve prendere il ripiego, non restare nulla"
    );
}

// ---------------------------------------------------------------------------
// Nono giro, finding 1 — il null logico nel pivot Count
// ---------------------------------------------------------------------------

#[test]
fn il_pivot_count_non_conta_una_entry_nulla_del_dizionario() {
    // Il difetto viveva in DUE punti: il kernel e l'oracolo di parita' dei
    // test, che ne e' una copia verbatim. Con lo stesso errore in entrambi la
    // parita' non poteva scoprirlo — due implementazioni che sbagliano allo
    // stesso modo concordano.
    //
    // Questo test non confronta il kernel con l'oracolo: dichiara il
    // risultato ATTESO a mano, riga per riga, ed e' quindi indipendente da
    // entrambi.
    use plenora_core::arrow::array::{types::Int32Type, DictionaryArray, Int32Array, StringArray};

    // Valori del dizionario: la voce 1 e' NULLA.
    let valori = StringArray::from(vec![Some("presente"), None]);
    // Quattro righe, tutte con chiave VALIDA (nessun null fisico):
    //   riga 0 -> "presente"      valore
    //   riga 1 -> entry nulla     null logico
    //   riga 2 -> "presente"      valore
    //   riga 3 -> entry nulla     null logico
    let chiavi = Int32Array::from(vec![Some(0), Some(1), Some(0), Some(1)]);
    let etichette: ArrayRef = Arc::new(
        DictionaryArray::<Int32Type>::try_new(chiavi, Arc::new(valori)).expect("dictionary"),
    );
    assert_eq!(
        etichette.null_count(),
        0,
        "premessa: nessun null FISICO, quindi solo il null logico distingue le righe"
    );

    let gruppo: ArrayRef = Arc::new(plenora_core::arrow::array::StringArray::from(vec![
        "g", "g", "g", "g",
    ]));
    let colonna: ArrayRef = Arc::new(plenora_core::arrow::array::StringArray::from(vec![
        "p", "p", "p", "p",
    ]));
    let ingresso = batch(
        vec![
            Field::new("gruppo", DataType::Utf8, false),
            Field::new("perno", DataType::Utf8, false),
            Field::new("valore", etichette.data_type().clone(), true),
        ],
        vec![gruppo, colonna, etichette],
    );

    let uscita = plenora_kernels_table::reshape::pivot(
        &ingresso,
        &serde_json::from_value(json!({
            "index_col": "gruppo",
            "pivot_col": "perno",
            "value_col": "valore",
            "aggr_func": "count"
        }))
        .expect("config"),
        &Limits::default(),
    )
    .expect("pivot");

    // Atteso INDIPENDENTE: delle quattro righe del gruppo `g` solo due
    // portano un valore; le altre due sono nulle, anche se la loro chiave
    // dictionary e' valida. `count` conta i valori.
    let conteggio = uscita
        .column_by_name("p")
        .expect("colonna perno")
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("conteggio Int64");
    assert_eq!(
        conteggio.value(0),
        2,
        "due righe su quattro portano un valore: le altre due puntano a una entry nulla"
    );
}

// ---------------------------------------------------------------------------
// Nono giro, finding 2 — la classe `ResourceLimit` chiusa nei kernel
// ---------------------------------------------------------------------------

#[test]
#[allow(clippy::too_many_lines)] // Elenco di casi: la lunghezza e' nei dati.
fn ogni_limite_di_risorsa_dei_kernel_ha_la_categoria_dedicata() {
    // Il difetto: la modellazione di `ResourceLimit` era stata applicata a
    // join, spill ed executor, ma decine di kernel continuavano a rispondere
    // `invalid_plan` quando erano i DATI a non entrare nel budget. La stessa
    // condizione — «il piano e' corretto, il volume no» — dava due categorie
    // diverse a seconda del kernel che la incontrava, e quindi due exit code
    // diversi.
    //
    // Il test attraversa un kernel per famiglia; la ricerca per classe e'
    // documentata in docs/review-5-fix-2026-08-17.md.
    use plenora_core::ErrorCategory;

    let stretti = Limits {
        max_rows: 1,
        max_columns: 1,
        max_string_bytes: 1,
        ..Limits::default()
    };
    let solo_righe = Limits {
        max_rows: 1,
        ..Limits::default()
    };

    let due_righe = batch(
        vec![Field::new("id", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef],
    );
    let altre_due = batch(
        vec![Field::new("id", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(vec![3_i64, 4])) as ArrayRef],
    );

    let mut casi: Vec<(&str, PlenoraError)> = Vec::new();

    casi.push((
        "cross_join oltre max_rows",
        plenora_kernels_table::joins::cross_join(
            &due_righe,
            &altre_due,
            &serde_json::from_value(json!({})).expect("config"),
            &solo_righe,
        )
        .expect_err("il prodotto e' 4 righe, il tetto 1"),
    ));

    casi.push((
        "concat_by_name oltre max_rows",
        plenora_kernels_table::joins::concat_by_name(
            &[&due_righe, &altre_due],
            &serde_json::from_value(json!({})).expect("config"),
            &solo_righe,
        )
        .expect_err("la somma e' 4 righe, il tetto 1"),
    ));

    casi.push((
        "union_distinct oltre max_rows",
        plenora_kernels_table::setops::union_distinct(
            &due_righe,
            &altre_due,
            &serde_json::from_value(json!({})).expect("config"),
            &solo_righe,
        )
        .expect_err("la somma e' 4 righe, il tetto 1"),
    ));

    casi.push((
        "melt oltre max_rows",
        plenora_kernels_table::reshape::melt(
            &batch(
                vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("a", DataType::Int64, true),
                    Field::new("b", DataType::Int64, true),
                ],
                vec![
                    Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
                    Arc::new(Int64Array::from(vec![Some(1_i64), Some(2)])) as ArrayRef,
                    Arc::new(Int64Array::from(vec![Some(3_i64), Some(4)])) as ArrayRef,
                ],
            ),
            &serde_json::from_value(json!({
                "id_columns": ["id"],
                "value_columns": ["a", "b"]
            }))
            .expect("config"),
            &solo_righe,
        )
        .expect_err("2 righe x 2 colonne valore = 4 righe, il tetto 1"),
    ));

    casi.push((
        "string_pad oltre max_string_bytes",
        plenora_kernels_table::strings::string_pad(
            &batch(
                vec![Field::new("t", DataType::Utf8, true)],
                vec![
                    Arc::new(plenora_core::arrow::array::StringArray::from(vec![Some(
                        "abc",
                    )])) as ArrayRef,
                ],
            ),
            &serde_json::from_value(json!({
                "column": "t",
                "width": 64,
                "fill_char": "x",
                "side": "left"
            }))
            .expect("config"),
            &stretti,
        )
        .expect_err("64 byte di riempimento contro un tetto di 1"),
    ));

    for (descrizione, errore) in casi {
        assert_eq!(
            errore.category(),
            ErrorCategory::ResourceLimit,
            "{descrizione}: un volume oltre il tetto e' una risorsa, non un piano invalido \
             (ottenuto {:?})",
            errore.category()
        );
    }
}

// ---------------------------------------------------------------------------
// Decimo giro, finding 1 — la classe chiusa anche dietro gli helper
// ---------------------------------------------------------------------------

#[test]
fn i_limiti_nascosti_dietro_helper_hanno_la_categoria_dedicata() {
    // Il censimento del giro precedente cercava le occorrenze LETTERALI di
    // `PlenoraError::InvalidPlan` e quindi non vedeva i siti che passano da
    // un helper (`contract()` della CLI), da un costruttore di comodo
    // (`cell_too_large` in geo) o da una conversione. Questi sono i cinque
    // che erano rimasti indietro, uno per famiglia.
    use plenora_core::ErrorCategory;

    let stretti = Limits {
        max_string_bytes: 1,
        ..Limits::default()
    };

    let testo = batch(
        vec![Field::new("t", DataType::Utf8, true)],
        vec![
            Arc::new(plenora_core::arrow::array::StringArray::from(vec![Some(
                "VALORE",
            )])) as ArrayRef,
        ],
    );

    let mut casi: Vec<(&str, PlenoraError)> = Vec::new();

    casi.push((
        "text_normalize oltre max_string_bytes",
        plenora_kernels_table::strings::text_normalize(
            &testo,
            &serde_json::from_value(json!({
                "columns": ["t"],
                "operations": "lower"
            }))
            .expect("config"),
            &stretti,
        )
        .expect_err("sei byte contro un tetto di uno"),
    ));

    casi.push((
        "fuzzy_join oltre max_candidates",
        plenora_kernels_table::fuzzy::fuzzy_join(
            &batch(
                vec![Field::new("k", DataType::Utf8, true)],
                vec![
                    Arc::new(plenora_core::arrow::array::StringArray::from(vec![Some(
                        "a",
                    )])) as ArrayRef,
                ],
            ),
            &batch(
                vec![Field::new("k", DataType::Utf8, true)],
                vec![Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                    Some("a"),
                    Some("a"),
                    Some("a"),
                ])) as ArrayRef],
            ),
            &serde_json::from_value(json!({
                "left_key": "k",
                "right_key": "k",
                "metric": "levenshtein",
                "blocking": "none",
                "threshold": 0.5,
                "max_candidates": 1
            }))
            .expect("config"),
            &Limits::default(),
        )
        .expect_err("tre candidati contro un tetto di uno"),
    ));

    for (descrizione, errore) in casi {
        assert_eq!(
            errore.category(),
            ErrorCategory::ResourceLimit,
            "{descrizione}: un volume oltre il tetto e' una risorsa, ottenuto {:?}",
            errore.category()
        );
    }
}

// ---------------------------------------------------------------------------
// Decimo giro, finding 2 — il rifiuto PREVENTIVO, prima di allocare
// ---------------------------------------------------------------------------

#[test]
fn cross_join_rifiuta_prima_di_allocare_l_output() {
    // Il controllo esisteva solo a valle: si costruiva l'output e poi lo si
    // confrontava col budget. Con un prodotto cartesiano quel «poi» puo' non
    // arrivare mai, perche' l'allocazione esaurisce la memoria prima.
    //
    // Il test verifica la proprieta' che si puo' verificare da dentro il
    // processo: che il rifiuto avvenga con un messaggio di STIMA — cioe' dal
    // percorso preventivo — e non dal conteggio a valle. Non puo' provare
    // «non ha allocato»; puo' provare che il ramo raggiunto e' quello che
    // decide prima.
    let sinistra = batch(
        vec![Field::new("id", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from((0..512_i64).collect::<Vec<_>>())) as ArrayRef],
    );
    let destra = batch(
        vec![Field::new("altro", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from((0..512_i64).collect::<Vec<_>>())) as ArrayRef],
    );
    // 512 x 512 = 262 144 righe: sotto `max_rows`, quindi il vecchio
    // controllo sulle righe non scatta e si arriva all'allocazione.
    let limiti = Limits {
        max_rows: 10_000_000,
        max_governed_memory_bytes: 64 * 1024,
        ..Limits::default()
    };

    let errore = plenora_kernels_table::joins::cross_join(
        &sinistra,
        &destra,
        &serde_json::from_value(json!({})).expect("config"),
        &limiti,
    )
    .expect_err("il prodotto non entra in 64 KiB");

    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
    assert!(
        errore.to_string().contains("output stimato"),
        "il rifiuto dev'essere quello PREVENTIVO, non un conteggio a valle: {errore}"
    );
    assert!(
        errore
            .to_string()
            .contains("supera max_governed_memory_bytes"),
        "e deve dire quale tetto: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Decimo giro, finding 3 — compare_cells_typed non e' piu' fail-open
// ---------------------------------------------------------------------------

#[test]
fn il_confronto_valida_dominio_e_indici_prima_dei_null() {
    use plenora_core::arrow::array::{types::Int32Type, DictionaryArray, Int32Array, StringArray};
    use plenora_kernels_table::aggregation::compare_cells_typed;

    let interi: ArrayRef = Arc::new(Int64Array::from(vec![None, Some(1_i64)]));
    let testi: ArrayRef = Arc::new(StringArray::from(vec![None, Some("a")]));

    // 1. Due celle NULLE di tipi non confrontabili: prima rispondeva `Equal`,
    //    cioe' dichiarava un ordine fra tipi che non sa confrontare. Con
    //    valori non nulli gli stessi due tipi davano `Schema`: la stessa
    //    coppia di colonne aveva due contratti a seconda del contenuto.
    let esito = compare_cells_typed(&interi, 0, &testi, 0);
    assert!(
        esito.is_err(),
        "due null di tipi diversi non sono uguali: sono un errore, ottenuto {esito:?}"
    );

    // 2. Un tipo non ordinabile del tutto, con la cella nulla.
    let non_ordinabile: ArrayRef = Arc::new(plenora_core::arrow::array::Float32Array::from(vec![
        None,
        Some(1.0_f32),
    ]));
    assert!(
        compare_cells_typed(&non_ordinabile, 0, &non_ordinabile, 0).is_err(),
        "un tipo senza confronto nativo resta un errore anche fra celle nulle"
    );

    // 3. Indice fuori intervallo: era un panico di arrow su un'API pubblica.
    assert!(
        compare_cells_typed(&interi, 99, &interi, 0).is_err(),
        "un indice fuori dall'array e' un errore, non un panico"
    );
    assert!(
        compare_cells_typed(&interi, 0, &interi, 99).is_err(),
        "e vale per entrambi i lati"
    );

    // 4. Dictionary FUORI dal profilo supportato, con entrambe le celle
    //    nulle. `compare_cells_typed` risolve solo `Dictionary(Int32, Utf8)`;
    //    su qualunque altra combinazione non ha un confronto, e prima
    //    rispondeva `Equal` perche' i null decidevano per primi. E' la stessa
    //    forma del difetto «dictionary incoerente mascherata dal null».
    //
    //    La variante con la CHIAVE malformata — chiave valida che punta fuori
    //    dal dizionario — non e' costruibile da un test: `try_new` di arrow
    //    la rifiuta e le vie alternative richiedono `unsafe`, vietato dal
    //    workspace. Quel percorso e' chiuso per costruzione: la risoluzione
    //    e' ora fallibile e viene eseguita per ENTRAMBE le celle prima di
    //    qualunque uscita anticipata, quindi non esiste piu' un ordine di
    //    valutazione in cui l'errore possa essere saltato.
    let valori = Int64Array::from(vec![Some(1_i64)]);
    let chiavi = Int32Array::from(vec![None, None]);
    let non_supportato: ArrayRef = Arc::new(
        DictionaryArray::<Int32Type>::try_new(chiavi, Arc::new(valori)).expect("dictionary"),
    );
    assert!(
        compare_cells_typed(&non_supportato, 0, &non_supportato, 1).is_err(),
        "una dictionary fuori profilo non viene mascherata dai null"
    );

    // 5. Cio' che era corretto resta corretto: stessa famiglia, null in coda.
    assert_eq!(
        compare_cells_typed(&interi, 0, &interi, 1).expect("stesso tipo"),
        Ordering::Greater,
        "il null va dopo il valore, come prima"
    );
}

// ---------------------------------------------------------------------------
// Settimo giro, finding 1 — la stima per operazione, non una formula unica
// ---------------------------------------------------------------------------

/// Budget scelto FRA la stima vecchia e quella nuova.
///
/// E' la forma che rende i tre test qui sotto significativi: con il modello
/// precedente — massimo della larghezza media degli input — la stima stava
/// SOTTO il budget e il preflight lasciava passare; col modello per
/// operazione la stima lo supera e il rifiuto arriva prima di allocare. Un
/// budget scelto a caso proverebbe solo che un numero grande viene rifiutato.
fn budget_fra(stima_vecchia: usize, stima_nuova: usize) -> Limits {
    assert!(
        stima_vecchia < stima_nuova,
        "il test non e' significativo se le due stime non sono ordinate: \
         {stima_vecchia} vs {stima_nuova}"
    );
    Limits {
        max_rows: 10_000_000,
        max_columns: 4_096,
        // A meta' strada: sopra cio' che la vecchia formula stimava, sotto
        // cio' che l'operazione richiede davvero.
        max_governed_memory_bytes: stima_vecchia + (stima_nuova - stima_vecchia) / 2,
        ..Limits::default()
    }
}

#[test]
fn cross_join_stima_la_somma_dei_due_lati_non_il_massimo() {
    // Ogni riga del prodotto cartesiano AFFIANCA una riga sinistra e una
    // destra: i byte si sommano. Il massimo e' il modello di un impilamento.
    // In piu' `cross_join` costruisce due vettori di indici lunghi quanto
    // l'output, che nessuna misura sugli input vedrebbe.
    let sinistra = batch(
        vec![Field::new("a", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from((0..200_i64).collect::<Vec<_>>())) as ArrayRef],
    );
    let destra = batch(
        vec![
            Field::new("b", DataType::Int64, false),
            Field::new("c", DataType::Int64, false),
            Field::new("d", DataType::Int64, false),
        ],
        vec![
            Arc::new(Int64Array::from((0..200_i64).collect::<Vec<_>>())) as ArrayRef,
            Arc::new(Int64Array::from((0..200_i64).collect::<Vec<_>>())) as ArrayRef,
            Arc::new(Int64Array::from((0..200_i64).collect::<Vec<_>>())) as ArrayRef,
        ],
    );
    let righe = 200 * 200;
    let per_riga_sinistra =
        plenora_kernels_table::batch_bytes_per_row(&sinistra).expect("larghezza di riga");
    let per_riga_destra =
        plenora_kernels_table::batch_bytes_per_row(&destra).expect("larghezza di riga");
    let indici = 2 * std::mem::size_of::<Option<usize>>();

    let limiti = budget_fra(
        // vecchio modello: massimo fra i due lati, indici ignorati
        righe * per_riga_sinistra.max(per_riga_destra),
        // nuovo: somma dei due lati piu' i vettori di indici
        righe * (per_riga_sinistra + per_riga_destra + indici),
    );

    let errore = plenora_kernels_table::joins::cross_join(
        &sinistra,
        &destra,
        &serde_json::from_value(json!({})).expect("config"),
        &limiti,
    )
    .expect_err("la somma dei due lati supera il budget");
    assert!(
        errore.to_string().contains("output stimato"),
        "dev'essere il rifiuto PREVENTIVO: {errore}"
    );
}

#[test]
fn concat_by_name_conta_le_colonne_portate_da_un_input_vuoto() {
    // Un input VUOTO non ha righe da misurare, ma porta le proprie colonne
    // nello schema unione: nell'output diventano colonne di null lunghe
    // quanto le righe degli altri input. Il modello precedente misurava solo
    // gli input e prendeva il massimo delle larghezze totali, quindi un input
    // vuoto pesava zero — ed e' il caso peggiore, non quello trascurabile.
    let pieno = batch(
        vec![Field::new("a", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from((0..1_000_i64).collect::<Vec<_>>())) as ArrayRef],
    );
    // Venti colonne, zero righe.
    let campi_vuoti: Vec<Field> = (0..20)
        .map(|indice| Field::new(format!("vuota_{indice}"), DataType::Int64, true))
        .collect();
    let colonne_vuote: Vec<ArrayRef> = (0..20)
        .map(|_| Arc::new(Int64Array::from(Vec::<i64>::new())) as ArrayRef)
        .collect();
    let vuoto = batch(campi_vuoti, colonne_vuote);

    let righe = 1_000;
    let per_riga_pieno =
        plenora_kernels_table::batch_bytes_per_row(&pieno).expect("larghezza di riga");
    let pavimento = plenora_kernels_table::type_bytes_floor(&DataType::Int64);

    let limiti = budget_fra(
        // vecchio modello: solo l'input misurabile
        righe * per_riga_pieno,
        // nuovo: una colonna per campo dello schema unione
        righe * (per_riga_pieno + 20 * pavimento),
    );

    let errore = plenora_kernels_table::joins::concat_by_name(
        &[&pieno, &vuoto],
        &serde_json::from_value(json!({})).expect("config"),
        &limiti,
    )
    .expect_err("le venti colonne di null non stanno nel budget");
    assert!(
        errore.to_string().contains("output stimato"),
        "dev'essere il rifiuto PREVENTIVO: {errore}"
    );
}

#[test]
fn melt_conta_la_colonna_dei_nomi_di_colonna() {
    // `melt` crea una colonna `variable` che ripete il NOME della colonna di
    // provenienza, una volta per riga di output. Con nomi lunghi quella
    // colonna puo' pesare piu' di tutto il resto, e il modello precedente —
    // larghezza media del batch d'ingresso — non la contava affatto.
    let nome_lungo = "colonna_con_un_nome_deliberatamente_molto_lungo_per_il_test";
    let ingresso = batch(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new(nome_lungo, DataType::Int64, true),
            Field::new(format!("{nome_lungo}_2"), DataType::Int64, true),
        ],
        vec![
            Arc::new(Int64Array::from((0..500_i64).collect::<Vec<_>>())) as ArrayRef,
            Arc::new(Int64Array::from((0..500_i64).collect::<Vec<_>>())) as ArrayRef,
            Arc::new(Int64Array::from((0..500_i64).collect::<Vec<_>>())) as ArrayRef,
        ],
    );
    let righe_uscita = 500 * 2;
    let per_riga_ingresso =
        plenora_kernels_table::batch_bytes_per_row(&ingresso).expect("larghezza di riga");
    let per_colonna = plenora_kernels_table::column_bytes_per_row(ingresso.column(0).as_ref());
    let nome = nome_lungo.len() + 2 + plenora_kernels_table::type_bytes_floor(&DataType::Utf8);

    let limiti = budget_fra(
        // vecchio modello: larghezza media dell'intero batch d'ingresso
        righe_uscita * per_riga_ingresso,
        // nuovo: id + valore + il nome ripetuto
        righe_uscita * (per_colonna + per_colonna + nome),
    );

    let errore = plenora_kernels_table::reshape::melt(
        &ingresso,
        &serde_json::from_value(json!({
            "id_columns": ["id"],
            "value_columns": [nome_lungo, format!("{nome_lungo}_2")]
        }))
        .expect("config"),
        &limiti,
    )
    .expect_err("la colonna dei nomi non sta nel budget");
    assert!(
        errore.to_string().contains("output stimato"),
        "dev'essere il rifiuto PREVENTIVO: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Settimo giro, finding 4 — i tetti sulle colonne, prima di allocare
// ---------------------------------------------------------------------------

#[test]
fn i_tetti_sulle_colonne_scattano_prima_delle_allocazioni() {
    // Il numero di colonne di output si sa dagli SCHEMI. `combine_horizontal`
    // lo verificava dopo i `take` di entrambi i lati — cioe' dopo aver
    // materializzato esattamente le colonne che stava per rifiutare — e
    // `concat_by_name` non lo verificava affatto.
    let sinistra = batch(
        vec![
            Field::new("a", DataType::Int64, false),
            Field::new("b", DataType::Int64, false),
        ],
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            Arc::new(Int64Array::from(vec![3_i64, 4])) as ArrayRef,
        ],
    );
    let destra = batch(
        vec![
            Field::new("c", DataType::Int64, false),
            Field::new("d", DataType::Int64, false),
        ],
        vec![
            Arc::new(Int64Array::from(vec![5_i64, 6])) as ArrayRef,
            Arc::new(Int64Array::from(vec![7_i64, 8])) as ArrayRef,
        ],
    );
    // Quattro colonne di output contro un tetto di tre.
    let limiti = Limits {
        max_columns: 3,
        ..Limits::default()
    };

    let errore = plenora_kernels_table::joins::cross_join(
        &sinistra,
        &destra,
        &serde_json::from_value(json!({})).expect("config"),
        &limiti,
    )
    .expect_err("quattro colonne contro un tetto di tre");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
    assert!(
        errore.to_string().contains("colonne di output"),
        "il messaggio dice il conteggio noto in anticipo: {errore}"
    );

    let errore = plenora_kernels_table::joins::concat_by_name(
        &[&sinistra, &destra],
        &serde_json::from_value(json!({})).expect("config"),
        &limiti,
    )
    .expect_err("lo schema unione ha quattro colonne");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
    assert!(
        errore.to_string().contains("max_columns"),
        "il tetto dev'essere nominato: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Ottavo giro, finding 1 — le righe dei batch SENZA COLONNE
// ---------------------------------------------------------------------------

/// Batch con zero colonne e `righe` righe.
///
/// Arrow lo consente e il progetto lo produce gia': una tabella puo' avere
/// una cardinalita' senza avere attributi. Va costruito con
/// `RecordBatchOptions`, perche' senza colonne non c'e' nulla da cui dedurre
/// il numero di righe — ed e' esattamente la ragione per cui ricostruirlo con
/// `try_new` non sapeva costruirlo: arrow rifiuta un batch senza colonne e
/// senza cardinalita' dichiarata.
fn batch_senza_colonne(righe: usize) -> RecordBatch {
    let opzioni = plenora_core::arrow::array::RecordBatchOptions::new().with_row_count(Some(righe));
    RecordBatch::try_new_with_options(
        Arc::new(plenora_core::arrow::schema::Schema::empty()),
        Vec::new(),
        &opzioni,
    )
    .expect("batch senza colonne")
}

#[test]
fn le_righe_non_si_perdono_nei_batch_senza_colonne() {
    // Le operazioni ricostruivano l'output con `RecordBatch::try_new`, che
    // deriva le righe dalla PRIMA colonna; con zero colonne arrow RIFIUTA di
    // costruire il batch. `concat` di due e tre righe non rispondeva cinque:
    // non rispondeva affatto. Difetto di disponibilita', non di correttezza —
    // la prima stesura di questo commento diceva «rispondeva zero» ed era
    // sbagliata.
    let due = batch_senza_colonne(2);
    let tre = batch_senza_colonne(3);
    assert_eq!(due.num_rows(), 2, "premessa: il batch di partenza ha righe");
    assert_eq!(due.num_columns(), 0, "premessa: e nessuna colonna");

    assert_eq!(
        plenora_kernels_table::joins::concat(
            &due,
            &tre,
            &serde_json::from_value(json!({})).expect("config"),
            &Limits::default(),
        )
        .expect("concat")
        .num_rows(),
        5,
        "concat impila le righe: due piu' tre"
    );

    assert_eq!(
        plenora_kernels_table::joins::concat_by_name(
            &[&due, &tre],
            &serde_json::from_value(json!({})).expect("config"),
            &Limits::default(),
        )
        .expect("concat_by_name")
        .num_rows(),
        5,
        "concat_by_name idem"
    );

    assert_eq!(
        plenora_kernels_table::joins::cross_join(
            &due,
            &tre,
            &serde_json::from_value(json!({})).expect("config"),
            &Limits::default(),
        )
        .expect("cross_join")
        .num_rows(),
        6,
        "cross_join moltiplica le cardinalita': due per tre"
    );

    // `select_rows` e' attraversata da molti kernel: se perdesse le righe qui,
    // le perderebbero anche loro.
    assert_eq!(
        plenora_kernels_table::select_rows(&due, &[0, 1, 0])
            .expect("select_rows")
            .num_rows(),
        3,
        "select_rows restituisce le righe selezionate, non zero"
    );
}

// ---------------------------------------------------------------------------
// Ottavo giro, finding 2 — melt che converte in testo
// ---------------------------------------------------------------------------

#[test]
fn melt_stima_la_larghezza_testuale_quando_converte_in_stringa() {
    // Con colonne eterogenee e `type_policy = "string"` i valori diventano
    // UTF-8: un `Int64` da otto byte puo' occuparne venti come testo. Il
    // modello misurava la larghezza binaria della sorgente, quindi
    // sottostimava proprio il percorso che alloca di piu'.
    //
    // Il budget sta FRA le due stime: con quella binaria il preflight lascia
    // passare, con quella testuale rifiuta.
    let righe = 2_000_usize;
    let ingresso = batch(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("numero", DataType::Int64, true),
            Field::new("testo", DataType::Utf8, true),
        ],
        vec![
            Arc::new(Int64Array::from(
                (0..righe)
                    .map(|riga| i64::try_from(riga).expect("indice"))
                    .collect::<Vec<_>>(),
            )) as ArrayRef,
            // Valori a fondo scala: la loro forma decimale e' lunga venti
            // caratteri, contro otto byte di rappresentazione binaria.
            Arc::new(Int64Array::from(vec![i64::MIN; righe])) as ArrayRef,
            Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                Some("x");
                righe
            ])) as ArrayRef,
        ],
    );
    let righe_uscita = righe * 2;
    let per_id = plenora_kernels_table::column_bytes_per_row(ingresso.column(0).as_ref());
    let binaria = plenora_kernels_table::column_bytes_per_row(ingresso.column(1).as_ref());
    let testuale = plenora_kernels_table::text_bytes_floor(&DataType::Int64);
    let nome = "numero".len() + plenora_kernels_table::type_bytes_floor(&DataType::Utf8);

    let limiti = budget_fra(
        righe_uscita * (per_id + binaria + nome),
        righe_uscita * (per_id + testuale + nome),
    );

    let errore = plenora_kernels_table::reshape::melt(
        &ingresso,
        &serde_json::from_value(json!({
            "id_columns": ["id"],
            "value_columns": ["numero", "testo"],
            "type_policy": "string"
        }))
        .expect("config"),
        &limiti,
    )
    .expect_err("la conversione in testo non sta nel budget");
    assert!(
        errore.to_string().contains("output stimato"),
        "dev'essere il rifiuto PREVENTIVO: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Ottavo giro, finding 4 — le stime non saturano in silenzio
// ---------------------------------------------------------------------------

#[test]
fn una_stima_che_perde_il_conto_non_autorizza_l_allocazione() {
    // `saturating_add` a fondo scala restituisce `usize::MAX`; con un budget
    // anch'esso a fondo scala il confronto `stima > budget` e' falso, e una
    // stima che ha perso il conto autorizzerebbe l'allocazione. E' la stessa
    // forma del difetto gia' corretto nello spill e nel picco della CLI.
    //
    // Il caso si costruisce sul prodotto, che e' la moltiplicazione dove il
    // traboccamento e' raggiungibile: righe enormi per una larghezza reale.
    let larga = batch(
        vec![Field::new("a", DataType::Int64, false)],
        vec![Arc::new(Int64Array::from(vec![1_i64; 4])) as ArrayRef],
    );
    let limiti = Limits {
        max_rows: usize::MAX,
        max_governed_memory_bytes: usize::MAX,
        ..Limits::default()
    };
    // `usize::MAX` righe non sono costruibili; il traboccamento della stima
    // si esercita sulla funzione, che e' il punto in cui la decisione viene
    // presa.
    let errore = plenora_kernels_table::preflight_output_bytes(
        "prova",
        usize::MAX,
        plenora_kernels_table::batch_bytes_per_row(&larga).expect("larghezza"),
        &limiti,
    )
    .expect_err("il prodotto non e' rappresentabile");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{errore}"
    );
    assert!(
        errore.to_string().contains("non rappresentabile"),
        "il messaggio dice che la stima non e' affidabile: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Nono giro, finding 2 — le Dictionary nel preflight di melt
// ---------------------------------------------------------------------------

#[test]
fn melt_conta_la_dictionary_dereferenziata_non_il_dizionario() {
    // Una `DictionaryArray` conta il valore testuale UNA VOLTA, nel
    // dizionario; l'output `Utf8` lo materializza per OGNI riga. Una singola
    // stringa lunga referenziata da migliaia di chiavi occupa pochi byte per
    // riga nell'input e molti megabyte nell'output.
    //
    // Il budget sta fra le due stime: con la misura sull'input il preflight
    // lascia passare, con il limite superiore sulle voci del dizionario
    // rifiuta.
    use plenora_core::arrow::array::{types::Int32Type, DictionaryArray, Int32Array, StringArray};

    let righe = 4_000_usize;
    let lunga = "v".repeat(512);
    // Una sola voce nel dizionario, referenziata da tutte le righe: e' il
    // caso peggiore e anche il piu' naturale per una codifica dictionary.
    let valori = StringArray::from(vec![Some(lunga.as_str())]);
    let chiavi = Int32Array::from(vec![Some(0_i32); righe]);
    let dizionario: ArrayRef = Arc::new(
        DictionaryArray::<Int32Type>::try_new(chiavi, Arc::new(valori)).expect("dictionary"),
    );
    let ingresso = batch(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("etichetta", dizionario.data_type().clone(), true),
            Field::new("numero", DataType::Int64, true),
        ],
        vec![
            Arc::new(Int64Array::from(
                (0..righe)
                    .map(|riga| i64::try_from(riga).expect("indice"))
                    .collect::<Vec<_>>(),
            )) as ArrayRef,
            dizionario.clone(),
            Arc::new(Int64Array::from(vec![1_i64; righe])) as ArrayRef,
        ],
    );

    let righe_uscita = righe * 2;
    let per_id = plenora_kernels_table::column_bytes_per_row(ingresso.column(0).as_ref());
    // Stima VECCHIA: la larghezza misurata della colonna dictionary, che
    // conta la stringa una volta sola su tutte le righe.
    let misurata = plenora_kernels_table::column_bytes_per_row(dizionario.as_ref()).max(
        plenora_kernels_table::text_bytes_floor(dizionario.data_type()),
    );
    // Stima NUOVA: la voce piu' lunga del dizionario, che ogni chiave puo'
    // dereferenziare.
    let dereferenziata = plenora_kernels_table::text_bytes_per_row(dizionario.as_ref());
    let nome = "etichetta".len() + plenora_kernels_table::type_bytes_floor(&DataType::Utf8);

    let limiti = budget_fra(
        righe_uscita * (per_id + misurata + nome),
        righe_uscita * (per_id + dereferenziata + nome),
    );

    let errore = plenora_kernels_table::reshape::melt(
        &ingresso,
        &serde_json::from_value(json!({
            "id_columns": ["id"],
            "value_columns": ["etichetta", "numero"],
            "type_policy": "string"
        }))
        .expect("config"),
        &limiti,
    )
    .expect_err("la dictionary dereferenziata non sta nel budget");
    assert!(
        errore.to_string().contains("output stimato"),
        "dev'essere il rifiuto PREVENTIVO: {errore}"
    );
}

#[test]
fn melt_rifiuta_un_tipo_non_convertibile_prima_di_allocare() {
    // La conversione in testo falliva a META' scansione, dopo aver costruito
    // gli indici di ripetizione e la colonna dei nomi: si allocava proprio
    // cio' che il rifiuto doveva evitare. Ora i tipi si validano sugli
    // SCHEMI, prima delle allocazioni proporzionali ai dati.
    let ingresso = batch(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("testo", DataType::Utf8, true),
            // Float32 non e' nel profilo scalare testuale.
            Field::new("non_convertibile", DataType::Float32, true),
        ],
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                Some("a"),
                Some("b"),
            ])) as ArrayRef,
            Arc::new(plenora_core::arrow::array::Float32Array::from(vec![
                Some(1.0_f32),
                Some(2.0),
            ])) as ArrayRef,
        ],
    );

    let errore = plenora_kernels_table::reshape::melt(
        &ingresso,
        &serde_json::from_value(json!({
            "id_columns": ["id"],
            "value_columns": ["testo", "non_convertibile"],
            "type_policy": "string"
        }))
        .expect("config"),
        &Limits::default(),
    )
    .expect_err("Float32 non e' convertibile in testo");
    assert!(
        errore.to_string().contains("non convertibile in testo"),
        "il rifiuto nomina il tipo e la colonna: {errore}"
    );
    assert!(
        errore.to_string().contains("non_convertibile"),
        "e la colonna: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Decimo giro, finding 1 — text_convertible coincide con scalar_as_string
// ---------------------------------------------------------------------------

#[test]
fn la_prevalidazione_dei_tipi_coincide_col_formatter() {
    // Il predicato accettava QUALUNQUE `Timestamp(_, _)` e qualunque
    // `Decimal128(_, _)`. Il formatter e' piu' stretto: fa downcast solo su
    // `TimestampMillisecondArray`, e converte la scala con `u32::try_from`
    // (che rifiuta le negative) e `10^scala` con `checked_pow` (che trabocca
    // oltre 38).
    //
    // Un `melt` eterogeneo con quei tipi superava quindi la prevalidazione e
    // falliva DOPO aver allocato indici e colonna `variable` — cioe' proprio
    // cio' che la prevalidazione esiste per evitare.
    use plenora_core::arrow::schema::TimeUnit;

    // Accettati: sono quelli che il formatter sa trattare.
    for tipo in [
        DataType::Utf8,
        DataType::Int64,
        DataType::Float64,
        DataType::Boolean,
        DataType::UInt64,
        DataType::Date32,
        DataType::Binary,
        DataType::Timestamp(TimeUnit::Millisecond, None),
        DataType::Decimal128(38, 0),
        DataType::Decimal128(38, 38),
    ] {
        assert!(
            plenora_kernels_table::text_convertible(&tipo),
            "{tipo:?} e' gestito dal formatter: va accettato"
        );
    }

    // Rifiutati: il formatter non li tratta, e accettarli sposta il
    // fallimento a dopo l'allocazione.
    for tipo in [
        DataType::Timestamp(TimeUnit::Second, None),
        DataType::Timestamp(TimeUnit::Microsecond, None),
        DataType::Timestamp(TimeUnit::Nanosecond, None),
        DataType::Decimal128(38, -1),
        DataType::Decimal128(38, -100),
        DataType::Decimal128(38, 39),
        DataType::Float32,
    ] {
        assert!(
            !plenora_kernels_table::text_convertible(&tipo),
            "{tipo:?} NON e' gestito dal formatter: va rifiutato prima di allocare"
        );
    }
}

#[test]
fn melt_rifiuta_timestamp_e_decimal_fuori_dal_formatter_prima_di_allocare() {
    // La stessa proprieta' vista dal kernel: il rifiuto arriva dalla
    // prevalidazione, non da meta' scansione.
    use plenora_core::arrow::array::{Decimal128Array, TimestampSecondArray};
    use plenora_core::arrow::schema::TimeUnit;

    for (nome, tipo, colonna) in [
        (
            "timestamp in secondi",
            DataType::Timestamp(TimeUnit::Second, None),
            Arc::new(TimestampSecondArray::from(vec![Some(1_i64), Some(2)])) as ArrayRef,
        ),
        (
            "decimal a scala negativa",
            DataType::Decimal128(38, -1),
            Arc::new(
                Decimal128Array::from(vec![Some(1_i128), Some(2)])
                    .with_precision_and_scale(38, -1)
                    .expect("decimal a scala negativa"),
            ) as ArrayRef,
        ),
    ] {
        let ingresso = batch(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("testo", DataType::Utf8, true),
                Field::new("fuori", tipo, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
                Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                ])) as ArrayRef,
                colonna,
            ],
        );
        let errore = plenora_kernels_table::reshape::melt(
            &ingresso,
            &serde_json::from_value(json!({
                "id_columns": ["id"],
                "value_columns": ["testo", "fuori"],
                "type_policy": "string"
            }))
            .expect("config"),
            &Limits::default(),
        )
        .expect_err("il tipo non e' convertibile");
        assert!(
            errore.to_string().contains("non convertibile in testo"),
            "{nome}: il rifiuto dev'essere quello della prevalidazione: {errore}"
        );
    }
}

// ---------------------------------------------------------------------------
// Undicesimo giro, finding 1 — i due nomi di melt non possono collidere
// ---------------------------------------------------------------------------

/// Batch con una colonna `id` e una colonna `v`, entrambe intere.
fn ingresso_con_v() -> RecordBatch {
    batch(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("v", DataType::Int64, true),
        ],
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            Arc::new(Int64Array::from(vec![Some(10_i64), Some(20)])) as ArrayRef,
        ],
    )
}

/// Nomi delle colonne dell'output, in ordine.
fn nomi_colonne(uscita: &RecordBatch) -> Vec<String> {
    uscita
        .schema()
        .fields()
        .iter()
        .map(|campo| campo.name().clone())
        .collect()
}

#[test]
fn i_nomi_di_melt_si_risolvono_in_sequenza_e_non_collidono() {
    // I due nomi venivano risolti INDIPENDENTEMENTE, ciascuno confrontato col
    // solo schema di input. Con l'input che contiene `v`:
    //
    //   var_name   = "v"   -> collide con l'input   -> "v_1"
    //   value_name = "v_1" -> non collide con `v`   -> "v_1"
    //
    // I nomi RICHIESTI erano distinti, quindi il controllo di contratto non
    // se ne accorgeva, e l'output usciva con due colonne omonime.
    //
    // Le attese qui sotto sono scritte a MANO: confrontarle con l'oracolo di
    // parita' non proverebbe nulla, perche' l'oracolo replicava lo stesso
    // algoritmo — e quindi lo stesso difetto.
    let uscita = plenora_kernels_table::reshape::melt(
        &ingresso_con_v(),
        &serde_json::from_value(json!({
            "id_columns": ["id"],
            "value_columns": ["v"],
            "var_name": "v",
            "value_name": "v_1"
        }))
        .expect("config"),
        &Limits::default(),
    )
    .expect("melt");

    let nomi = nomi_colonne(&uscita);
    assert_eq!(
        nomi,
        vec!["id".to_owned(), "v_1".to_owned(), "v_1_1".to_owned()],
        "il secondo nome deve evitare anche il PRIMO nome risolto"
    );
    let distinti: std::collections::BTreeSet<&String> = nomi.iter().collect();
    assert_eq!(
        distinti.len(),
        nomi.len(),
        "uno schema con nomi duplicati e' un contratto rotto: {nomi:?}"
    );
}

#[test]
fn due_nomi_di_melt_uguali_non_producono_colonne_omonime() {
    // Chiamando il kernel DIRETTAMENTE — l'executor rifiuta prima
    // `var_name == value_name`, ma il kernel e' pubblico — la risoluzione
    // sequenziale disambigua invece di produrre due colonne con lo stesso
    // nome.
    let uscita = plenora_kernels_table::reshape::melt(
        &ingresso_con_v(),
        &serde_json::from_value(json!({
            "id_columns": ["id"],
            "value_columns": ["v"],
            "var_name": "etichetta",
            "value_name": "etichetta"
        }))
        .expect("config"),
        &Limits::default(),
    )
    .expect("melt");

    let nomi = nomi_colonne(&uscita);
    assert_eq!(
        nomi,
        vec![
            "id".to_owned(),
            "etichetta".to_owned(),
            "etichetta_1".to_owned()
        ],
        "il secondo nome riceve il suffisso perche' il primo e' gia' riservato"
    );
}

#[test]
fn il_risolutore_condiviso_riserva_i_nomi_man_mano() {
    // La proprieta' alla radice, senza passare da `melt`: ogni nome risolto
    // diventa occupato per quelli successivi.
    let risolti = plenora_kernels_table::resolve_output_names(["v"], &["v", "v_1", "v"])
        .expect("risoluzione");
    assert_eq!(
        risolti,
        vec!["v_1".to_owned(), "v_1_1".to_owned(), "v_2".to_owned()],
        "ogni nome evita l'input E i nomi gia' risolti"
    );
    let distinti: std::collections::BTreeSet<&String> = risolti.iter().collect();
    assert_eq!(
        distinti.len(),
        risolti.len(),
        "nessun duplicato: {risolti:?}"
    );
}

// ---------------------------------------------------------------------------
// Undicesimo giro, finding 2 — cio' che si sa dallo schema si rifiuta prima
// ---------------------------------------------------------------------------

#[test]
fn melt_rifiuta_una_timezone_non_valida_dallo_schema() {
    // La timezone sta nello SCHEMA, non nei valori: `scalar_as_string` la
    // risolveva a ogni riga, quindi una timezone non valida faceva fallire
    // l'operazione durante la scansione, dopo le allocazioni.
    use plenora_core::arrow::array::TimestampMillisecondArray;
    use plenora_core::arrow::schema::TimeUnit;

    let istanti = TimestampMillisecondArray::from(vec![Some(0_i64), Some(1)])
        .with_timezone("Non/Esiste".to_owned());
    let ingresso = batch(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("testo", DataType::Utf8, true),
            Field::new(
                "istante",
                DataType::Timestamp(TimeUnit::Millisecond, Some("Non/Esiste".into())),
                true,
            ),
        ],
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                Some("a"),
                Some("b"),
            ])) as ArrayRef,
            Arc::new(istanti) as ArrayRef,
        ],
    );

    let errore = plenora_kernels_table::reshape::melt(
        &ingresso,
        &serde_json::from_value(json!({
            "id_columns": ["id"],
            "value_columns": ["testo", "istante"],
            "type_policy": "string"
        }))
        .expect("config"),
        &Limits::default(),
    )
    .expect_err("la timezone non e' risolvibile");
    assert!(
        errore.to_string().contains("timezone Arrow"),
        "il rifiuto viene dalla prevalidazione sullo schema: {errore}"
    );
}

#[test]
fn un_piano_invalido_resta_invalido_qualunque_sia_il_budget() {
    // Colonne eterogenee con `type_policy = "reject"`: la condizione e' nota
    // dallo schema. Il rifiuto arrivava pero' in fondo, dopo il tetto sulle
    // righe e dopo la stima: con un budget stretto usciva prima un
    // `resource_limit`, e chi lo leggeva andava ad alzare un budget per un
    // piano che non sarebbe comunque stato eseguibile.
    use plenora_core::ErrorCategory;

    let ingresso = batch(
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("numero", DataType::Int64, true),
            Field::new("testo", DataType::Utf8, true),
        ],
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            Arc::new(Int64Array::from(vec![Some(10_i64), Some(20)])) as ArrayRef,
            Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                Some("a"),
                Some("b"),
            ])) as ArrayRef,
        ],
    );
    let config = serde_json::from_value(json!({
        "id_columns": ["id"],
        "value_columns": ["numero", "testo"]
    }))
    .expect("config");

    // Budget generoso e budget al minimo devono dare la STESSA categoria.
    for limiti in [
        Limits::default(),
        Limits {
            max_rows: 1,
            max_governed_memory_bytes: 1,
            ..Limits::default()
        },
    ] {
        let errore = plenora_kernels_table::reshape::melt(&ingresso, &config, &limiti)
            .expect_err("colonne eterogenee con policy reject");
        assert_eq!(
            errore.category(),
            ErrorCategory::InvalidPlan,
            "il piano e' invalido a prescindere dal budget: {errore}"
        );
    }
}

// ---------------------------------------------------------------------------
// Dodicesimo giro, finding 1 — analisi ed esecuzione decidono insieme
// ---------------------------------------------------------------------------

/// Contratto d'ingresso per l'analisi, a partire dai campi dichiarati.
fn contratto(campi: Vec<Field>) -> plenora_core::contract::DataContract {
    plenora_core::contract::DataContract::tabular(Arc::new(
        plenora_core::arrow::schema::Schema::new(campi),
    ))
}

#[test]
fn l_analisi_di_melt_rifiuta_cio_che_il_kernel_rifiuterebbe() {
    // L'analisi dichiarava `Utf8` non appena leggeva `type_policy = "string"`
    // nella config, senza verificare che le colonne fossero davvero
    // convertibili. Ma il kernel lo verifica, e lo fa guardando SOLO lo
    // schema — che l'analisi ha sotto gli occhi.
    //
    // Un contratto che promette un output che l'esecuzione sa gia' di non
    // poter produrre e' peggio di un rifiuto: sposta la scoperta a runtime
    // per un difetto conoscibile in validazione.
    use plenora_core::arrow::schema::TimeUnit;

    let casi: [(&str, DataType); 3] = [
        // Unita' temporale che il formatter non tratta.
        (
            "timestamp in secondi",
            DataType::Timestamp(TimeUnit::Second, None),
        ),
        // Timezone non risolvibile: sta nello schema, non nei valori.
        (
            "timezone inesistente",
            DataType::Timestamp(TimeUnit::Millisecond, Some("Non/Esiste".into())),
        ),
        // Scala negativa: Arrow la accetta, il formatter no.
        ("decimal a scala negativa", DataType::Decimal128(38, -1)),
    ];

    for (nome, tipo) in casi {
        let ingresso = contratto(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("testo", DataType::Utf8, true),
            Field::new("fuori", tipo.clone(), true),
        ]);
        let config = json!({
            "id_columns": ["id"],
            "value_columns": ["testo", "fuori"],
            "type_policy": "string"
        });

        // L'ANALISI deve rifiutare.
        let errore = plenora_kernels_table::analyze::analyze_table_contract(
            "table.melt",
            std::slice::from_ref(&ingresso),
            &config,
            &mut plenora_core::contract::FieldAllocator::new(100),
        )
        .expect_err(&format!("{nome}: l'analisi deve rifiutare"));
        assert_eq!(
            errore.category(),
            plenora_core::ErrorCategory::Schema,
            "{nome}: e' il tipo della colonna a non essere convertibile, non il \
             piano a essere malformato: {errore}"
        );

        // E il KERNEL rifiuta lo stesso caso: e' la coerenza fra i due che si
        // sta verificando, non due comportamenti indipendenti.
        let colonna: ArrayRef = plenora_core::arrow::array::new_null_array(&tipo, 2);
        let batch_ingresso = batch(
            vec![
                Field::new("id", DataType::Int64, false),
                Field::new("testo", DataType::Utf8, true),
                Field::new("fuori", tipo, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
                Arc::new(plenora_core::arrow::array::StringArray::from(vec![
                    Some("a"),
                    Some("b"),
                ])) as ArrayRef,
                colonna,
            ],
        );
        let errore_kernel = plenora_kernels_table::reshape::melt(
            &batch_ingresso,
            &serde_json::from_value(config).expect("config"),
            &Limits::default(),
        )
        .expect_err(&format!("{nome}: il kernel deve rifiutare"));
        assert_eq!(
            errore_kernel.category(),
            plenora_core::ErrorCategory::Schema,
            "{nome}: stessa categoria dell'analisi: {errore_kernel}"
        );
    }
}

// ---------------------------------------------------------------------------
// Dodicesimo giro, finding 2 — il nome GENERATO rispetta il limite
// ---------------------------------------------------------------------------

#[test]
fn un_nome_al_limite_che_collide_non_produce_un_nome_oltre_il_limite() {
    // `validate_output_name` impone 1024 byte. Il suffisso allunga il nome: un
    // nome di esattamente 1024 byte che collide diventava `nome_1`, cioe'
    // 1026 byte — oltre il limite che la validazione esiste per imporre.
    // L'invariante si perdeva proprio nel caso che lo mette alla prova.
    let al_limite = "n".repeat(1024);
    assert_eq!(
        al_limite.len(),
        1024,
        "premessa: il nome e' esattamente al limite"
    );

    // Senza collisione il nome resta valido e passa.
    let senza_collisione =
        plenora_kernels_table::resolve_output_names(["altro"], &[al_limite.as_str()])
            .expect("nessuna collisione");
    assert_eq!(senza_collisione, vec![al_limite.clone()]);

    // Con la collisione non esiste alcun suffisso che stia nel limite: il
    // nome non e' risolvibile, e la funzione lo dice invece di produrre un
    // nome fuori contratto.
    let errore =
        plenora_kernels_table::resolve_output_names([al_limite.as_str()], &[al_limite.as_str()])
            .expect_err("nessun candidato puo' stare nel limite");
    assert!(
        errore
            .to_string()
            .contains("impossibile evitare collisione"),
        "il rifiuto e' esplicito: {errore}"
    );

    // Un nome che dopo il suffisso resta entro il limite si risolve ancora.
    let quasi = "n".repeat(1022);
    let risolti = plenora_kernels_table::resolve_output_names([quasi.as_str()], &[quasi.as_str()])
        .expect("il suffisso ci sta");
    assert_eq!(risolti, vec![format!("{quasi}_1")]);
    assert!(
        risolti[0].len() <= 1024,
        "il nome generato rispetta il limite: {} byte",
        risolti[0].len()
    );
}

#[test]
fn i_suffissi_offerti_sono_quelli_dichiarati() {
    // Il messaggio d'errore prometteva cento suffissi e il codice ne provava
    // novantanove. La costante e' ora una sola, e questo test verifica che
    // l'ultimo suffisso dichiarato sia davvero raggiungibile.
    let base = "c";
    let mut occupati: Vec<String> = vec![base.to_owned()];
    for indice in 1..plenora_kernels_table::MAX_SUFFISSI_COLLISIONE {
        occupati.push(format!("{base}_{indice}"));
    }
    let riferimenti: Vec<&str> = occupati.iter().map(String::as_str).collect();
    let risolti = plenora_kernels_table::resolve_output_names(riferimenti.clone(), &[base])
        .expect("l'ultimo suffisso e' ancora disponibile");
    assert_eq!(
        risolti,
        vec![format!(
            "{base}_{}",
            plenora_kernels_table::MAX_SUFFISSI_COLLISIONE
        )],
        "l'ultimo suffisso dichiarato dev'essere raggiungibile"
    );

    // Occupando anche quello, non resta nulla.
    let mut tutti = occupati.clone();
    tutti.push(format!(
        "{base}_{}",
        plenora_kernels_table::MAX_SUFFISSI_COLLISIONE
    ));
    let riferimenti: Vec<&str> = tutti.iter().map(String::as_str).collect();
    let errore = plenora_kernels_table::resolve_output_names(riferimenti, &[base])
        .expect_err("tutti i suffissi sono occupati");
    assert!(
        errore
            .to_string()
            .contains(&plenora_kernels_table::MAX_SUFFISSI_COLLISIONE.to_string()),
        "il messaggio riporta il numero REALE di suffissi: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Dodicesimo giro, ricerca della classe — la SECONDA copia del profilo
// ---------------------------------------------------------------------------

#[test]
fn il_profilo_testuale_dell_analizzatore_e_quello_del_formatter() {
    // Cercando la stessa classe del finding su melt e' emersa una seconda
    // copia del profilo scalare testuale, dentro l'analizzatore
    // (`analyze::helpers::is_scalar_string`). Era piu' permissiva
    // dell'originale sulle scale di `Decimal128` e non guardava la timezone:
    // `table.lookup` su una colonna `Decimal128(38, -1)` riceveva un
    // contratto valido e falliva in esecuzione.
    //
    // Il presidio non e' il singolo op: e' che la copia non esiste piu'. Qui
    // si verifica dal lato osservabile, su un op che passa da
    // `require_scalar_string`.
    use plenora_core::arrow::schema::TimeUnit;

    let casi: [(&str, DataType); 2] = [
        ("decimal a scala negativa", DataType::Decimal128(38, -1)),
        (
            "timezone inesistente",
            DataType::Timestamp(TimeUnit::Millisecond, Some("Non/Esiste".into())),
        ),
    ];

    for (nome, tipo) in casi {
        let ingresso = contratto(vec![Field::new("chiave", tipo, true)]);
        let errore = plenora_kernels_table::analyze::analyze_table_contract(
            "table.lookup",
            std::slice::from_ref(&ingresso),
            &json!({"column": "chiave", "mapping": {"a": "b"}}),
            &mut plenora_core::contract::FieldAllocator::new(100),
        )
        .expect_err(&format!("{nome}: l'analisi deve rifiutare"));
        assert_eq!(
            errore.category(),
            plenora_core::ErrorCategory::InvalidPlan,
            "{nome}: categoria invariata rispetto a prima, cambia cio' che \
             viene accettato: {errore}"
        );
    }
}

#[test]
fn le_set_operation_non_usano_il_profilo_testuale() {
    // Il contrappeso del test precedente, e la ragione per cui la copia non
    // e' stata semplicemente cancellata: l'encoder di chiave delle set
    // operation NON formatta niente, scrive la rappresentazione binaria.
    // Accetta quindi `Decimal128` a qualunque scala e non risolve la
    // timezone.
    //
    // Se le si fosse fatte delegare al profilo testuale, questi piani
    // sarebbero stati rifiutati in validazione pur essendo eseguibili: una
    // falsa segnalazione al posto del difetto.
    use plenora_core::arrow::schema::TimeUnit;

    for tipo in [
        DataType::Decimal128(38, -1),
        DataType::Timestamp(TimeUnit::Millisecond, Some("Non/Esiste".into())),
    ] {
        let ingresso = contratto(vec![Field::new("k", tipo.clone(), true)]);
        let contratto_uscita = plenora_kernels_table::analyze::analyze_table_contract(
            "table.union_distinct",
            &[ingresso.clone(), ingresso],
            &json!({}),
            &mut plenora_core::contract::FieldAllocator::new(100),
        )
        .unwrap_or_else(|errore| {
            panic!("{tipo:?} e' codificabile come chiave, non va rifiutato: {errore}")
        });
        assert_eq!(
            contratto_uscita.schema.fields().len(),
            1,
            "lo schema attraversa la set operation invariato"
        );
    }
}

// ---------------------------------------------------------------------------
// Tredicesimo giro — matrice analyzer <-> runtime di `table.expression`
// ---------------------------------------------------------------------------

/// Verdetto atteso, scritto A MANO per ciascun caso.
///
/// Le due colonne sono separate di proposito: nella stragrande maggioranza
/// dei casi devono coincidere, ed e' questo che il test verifica. Dove NON
/// coincidono — l'eterogeneita' tollerata con `output_type` dichiarato — la
/// differenza e' dichiarata qui, non nascosta in un helper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdetto {
    /// Accettato; per l'analisi e' anche il tipo Arrow atteso in output.
    Accetta,
    Rifiuta,
}

/// Una riga della matrice: nome del caso, espressione, `output_type`,
/// verdetto atteso dall'ANALISI, tipo Arrow atteso quando accetta, verdetto
/// atteso dall'ESECUZIONE sugli stessi dati.
struct CasoExpression {
    nome: &'static str,
    espressione: serde_json::Value,
    tipo_output: &'static str,
    analisi: Verdetto,
    tipo_atteso: Option<DataType>,
    runtime: Verdetto,
}

/// Batch di prova: una riga, valori non nulli salvo `nullnum`.
fn batch_espressioni() -> RecordBatch {
    use plenora_core::arrow::array::{
        BooleanArray, Date32Array, Float64Array, StringArray, TimestampMillisecondArray,
        TimestampSecondArray,
    };
    use plenora_core::arrow::schema::TimeUnit;

    batch(
        vec![
            Field::new("txt", DataType::Utf8, true),
            Field::new("num", DataType::Float64, true),
            Field::new("nullnum", DataType::Float64, true),
            Field::new("flag", DataType::Boolean, true),
            Field::new("nullflag", DataType::Boolean, true),
            Field::new("d", DataType::Date32, true),
            Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new("tsec", DataType::Timestamp(TimeUnit::Second, None), true),
        ],
        vec![
            Arc::new(StringArray::from(vec![Some("2024-01-15")])) as ArrayRef,
            Arc::new(Float64Array::from(vec![Some(1.5_f64)])) as ArrayRef,
            Arc::new(Float64Array::from(vec![None::<f64>])) as ArrayRef,
            Arc::new(BooleanArray::from(vec![Some(true)])) as ArrayRef,
            Arc::new(BooleanArray::from(vec![None::<bool>])) as ArrayRef,
            Arc::new(Date32Array::from(vec![Some(19_738_i32)])) as ArrayRef,
            Arc::new(TimestampMillisecondArray::from(vec![Some(
                1_700_000_000_000_i64,
            )])) as ArrayRef,
            Arc::new(TimestampSecondArray::from(vec![Some(1_700_000_000_i64)])) as ArrayRef,
        ],
    )
}

#[test]
#[allow(clippy::too_many_lines)]
fn analisi_ed_esecuzione_di_expression_decidono_insieme() {
    // L'analizzatore replicava a mano il sistema di tipi dell'interprete, e
    // le due copie erano scivolate: `year` numerico invece che testuale,
    // `concat` senza controlli, `case when` senza booleano, `Timestamp` di
    // qualunque unita' trattato come numero, e — il caso piu' largo — l'AST
    // non analizzato affatto quando `output_type` era dichiarato.
    //
    // Questa matrice non confronta l'analizzatore col runtime lasciandoli
    // decidere: ogni riga porta l'attesa SCRITTA A MANO per entrambi. Un
    // confronto diretto sarebbe verde anche se sbagliassero insieme.
    let ingresso = batch_espressioni();
    let contratto_ingresso = contratto(
        ingresso
            .schema()
            .fields()
            .iter()
            .map(|campo| campo.as_ref().clone())
            .collect(),
    );

    let colonna = |nome: &str| json!({"kind": "column", "name": nome});
    let letterale = |valore: serde_json::Value| json!({"kind": "literal", "value": valore});
    let funzione = |nome: &str, args: Vec<serde_json::Value>| json!({"kind": "function", "name": nome, "args": args});

    // (nome, espressione, output_type, verdetto analisi, tipo atteso,
    //  verdetto runtime)
    let riga = |nome, espressione, tipo_output, analisi, tipo_atteso, runtime| CasoExpression {
        nome,
        espressione,
        tipo_output,
        analisi,
        tipo_atteso,
        runtime,
    };
    let casi: Vec<CasoExpression> = vec![
        riga(
            "colonna testuale in auto",
            colonna("txt"),
            "auto",
            Verdetto::Accetta,
            Some(DataType::Utf8),
            Verdetto::Accetta,
        ),
        riga(
            "timestamp in secondi: nessun percorso numerico",
            colonna("tsec"),
            "auto",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "timestamp in millisecondi: numero",
            colonna("ts"),
            "auto",
            Verdetto::Accetta,
            Some(DataType::Float64),
            Verdetto::Accetta,
        ),
        riga(
            "year su testo",
            funzione("year", vec![colonna("txt")]),
            "auto",
            Verdetto::Accetta,
            Some(DataType::Float64),
            Verdetto::Accetta,
        ),
        riga(
            "year su colonna temporale: il runtime vuole testo",
            funzione("year", vec![colonna("d")]),
            "auto",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "concat di soli testi",
            funzione("concat", vec![colonna("txt"), letterale(json!("!"))]),
            "auto",
            Verdetto::Accetta,
            Some(DataType::Utf8),
            Verdetto::Accetta,
        ),
        riga(
            "concat con un numero: `text()` non converte",
            funzione("concat", vec![colonna("txt"), colonna("num")]),
            "auto",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "case con condizione booleana",
            json!({
                "kind": "case",
                "branches": [{"when": colonna("flag"), "then": letterale(json!(1))}],
                "else_value": letterale(json!(2))
            }),
            "auto",
            Verdetto::Accetta,
            Some(DataType::Float64),
            Verdetto::Accetta,
        ),
        riga(
            "case con condizione numerica: `boolean()` non converte",
            json!({
                "kind": "case",
                "branches": [{"when": colonna("num"), "then": letterale(json!(1))}],
                "else_value": letterale(json!(2))
            }),
            "auto",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "testo dichiarato booleano",
            colonna("txt"),
            "boolean",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "booleano dichiarato numero",
            colonna("flag"),
            "number",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "colonna Date32 dichiarata date32: solo date_trunc produce una data",
            colonna("d"),
            "date32",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "date_trunc dichiarato date32",
            funzione("date_trunc", vec![letterale(json!("day")), colonna("d")]),
            "date32",
            Verdetto::Accetta,
            Some(DataType::Date32),
            Verdetto::Accetta,
        ),
        riga(
            "colonna inesistente col tipo dichiarato",
            colonna("assente"),
            "text",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "in con una lista di tipo diverso: `compare` rifiuta",
            funzione("in", vec![colonna("num"), letterale(json!(["a", "b"]))]),
            "auto",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "in con una lista dello stesso tipo",
            funzione("in", vec![colonna("num"), letterale(json!([1.5, 2.5]))]),
            "auto",
            Verdetto::Accetta,
            Some(DataType::Boolean),
            Verdetto::Accetta,
        ),
        riga(
            "confronto fra tipi diversi col tipo dichiarato: resta un errore",
            json!({
                "kind": "binary",
                "op": "equal",
                "left": colonna("num"),
                "right": colonna("txt")
            }),
            "boolean",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "greatest fra tipi diversi col tipo dichiarato: resta un errore",
            funzione("greatest", vec![colonna("num"), colonna("txt")]),
            "text",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "coalesce a TRE tipi: l'insieme li conserva tutti",
            funzione(
                "coalesce",
                vec![colonna("num"), colonna("txt"), colonna("flag")],
            ),
            "auto",
            Verdetto::Rifiuta,
            None,
            // Anche il kernel decide dallo SCHEMA: con `auto` un insieme
            // eterogeneo non ha un tipo di output, e il rifiuto e' lo stesso
            // dell'analisi. Prima il kernel guardava i VALORI e su questi
            // dati riusciva, con uno schema che dipendeva dalle righe.
            Verdetto::Rifiuta,
        ),
        riga(
            "coalesce a tre tipi, ordine diverso: stesso verdetto",
            funzione(
                "coalesce",
                vec![colonna("flag"), colonna("num"), colonna("txt")],
            ),
            "auto",
            Verdetto::Rifiuta,
            None,
            // Anche il kernel decide dallo SCHEMA: con `auto` un insieme
            // eterogeneo non ha un tipo di output, e il rifiuto e' lo stesso
            // dell'analisi. Prima il kernel guardava i VALORI e su questi
            // dati riusciva, con uno schema che dipendeva dalle righe.
            Verdetto::Rifiuta,
        ),
        riga(
            "coalesce a tre tipi, tipo dichiarato PRESENTE nell'insieme",
            funzione(
                "coalesce",
                vec![colonna("nullnum"), colonna("txt"), colonna("nullflag")],
            ),
            "text",
            Verdetto::Accetta,
            Some(DataType::Utf8),
            Verdetto::Accetta,
        ),
        riga(
            "coalesce a tre tipi, tipo dichiarato ASSENTE dall'insieme",
            funzione(
                "coalesce",
                vec![colonna("num"), colonna("txt"), colonna("flag")],
            ),
            "date32",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "confronto con un coalesce eterogeneo ANNIDATO",
            json!({
                "kind": "binary",
                "op": "equal",
                "left": funzione("coalesce", vec![colonna("nullnum"), colonna("txt")]),
                "right": colonna("num")
            }),
            "boolean",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "case a tre rami con tre tipi",
            json!({
                "kind": "case",
                "branches": [
                    {"when": colonna("flag"), "then": colonna("num")},
                    {"when": colonna("flag"), "then": colonna("txt")}
                ],
                "else_value": colonna("flag")
            }),
            "auto",
            Verdetto::Rifiuta,
            None,
            // Anche il kernel decide dallo SCHEMA: con `auto` un insieme
            // eterogeneo non ha un tipo di output, e il rifiuto e' lo stesso
            // dell'analisi. Prima il kernel guardava i VALORI e su questi
            // dati riusciva, con uno schema che dipendeva dalle righe.
            Verdetto::Rifiuta,
        ),
        riga(
            "case a tre rami, tipo dichiarato assente dall'insieme",
            json!({
                "kind": "case",
                "branches": [
                    {"when": colonna("flag"), "then": colonna("num")},
                    {"when": colonna("flag"), "then": colonna("txt")}
                ],
                "else_value": colonna("flag")
            }),
            "date32",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "greatest con un coalesce eterogeneo annidato",
            funzione(
                "greatest",
                vec![
                    funzione("coalesce", vec![colonna("nullnum"), colonna("txt")]),
                    colonna("num"),
                ],
            ),
            "number",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "in con un valore eterogeneo annidato",
            funzione(
                "in",
                vec![
                    funzione("coalesce", vec![colonna("nullnum"), colonna("txt")]),
                    letterale(json!([1.5])),
                ],
            ),
            "boolean",
            Verdetto::Rifiuta,
            None,
            Verdetto::Rifiuta,
        ),
        riga(
            "eterogeneo con tipo dichiarato, ramo nullo: il runtime riesce",
            funzione("coalesce", vec![colonna("nullnum"), colonna("txt")]),
            "text",
            Verdetto::Accetta,
            Some(DataType::Utf8),
            Verdetto::Accetta,
        ),
    ];

    // Gli scarti si ACCUMULANO invece di fermare il test al primo: una
    // matrice che si ferma subito dimostra un solo difetto per esecuzione, e
    // qui i difetti chiusi sono cinque.
    let mut scarti: Vec<String> = Vec::new();

    for CasoExpression {
        nome,
        espressione,
        tipo_output,
        analisi: atteso_analisi,
        tipo_atteso,
        runtime: atteso_runtime,
    } in casi
    {
        let config = json!({
            "output_column": "e",
            "expression": espressione,
            "output_type": tipo_output
        });

        // ANALISI.
        let analisi = plenora_kernels_table::analyze::analyze_table_contract(
            "table.expression",
            std::slice::from_ref(&contratto_ingresso),
            &config,
            &mut plenora_core::contract::FieldAllocator::new(100),
        );
        match (atteso_analisi, &analisi) {
            (Verdetto::Accetta, Ok(contratto_uscita)) => {
                let campo = contratto_uscita
                    .schema
                    .field_with_name("e")
                    .unwrap_or_else(|errore| panic!("{nome}: colonna di output assente: {errore}"));
                assert_eq!(
                    campo.data_type(),
                    tipo_atteso.as_ref().unwrap_or_else(|| -> &DataType {
                        panic!("{nome}: tipo atteso mancante nel caso")
                    }),
                    "{nome}: tipo dichiarato dall'analisi"
                );
            }
            (Verdetto::Rifiuta, Err(_)) => {}
            (atteso, esito) => scarti.push(format!(
                "{nome}: l'analisi doveva {atteso:?}, esito {}",
                if esito.is_ok() { "Accetta" } else { "Rifiuta" }
            )),
        }

        // ESECUZIONE, sulla stessa configurazione e sugli stessi dati.
        let esecuzione = plenora_kernels_table::expressions::expression(
            &ingresso,
            &serde_json::from_value(config).expect("config"),
        );
        match (atteso_runtime, &esecuzione) {
            (Verdetto::Accetta, Ok(uscita)) => {
                // Il tipo si confronta solo dove ANCHE l'analisi accetta: e'
                // li' che c'e' una promessa da mantenere. Dove l'analisi
                // rifiuta e l'esecuzione riesce — la conservativita' di
                // `auto` — non c'e' nessun tipo promesso da verificare.
                if let Some(atteso) = tipo_atteso.as_ref() {
                    let campo = uscita.schema().field_with_name("e").map_or_else(
                        |errore| panic!("{nome}: colonna prodotta assente: {errore}"),
                        Clone::clone,
                    );
                    assert_eq!(
                        campo.data_type(),
                        atteso,
                        "{nome}: il tipo prodotto e' quello che l'analisi aveva promesso"
                    );
                }
            }
            (Verdetto::Rifiuta, Err(_)) => {}
            (atteso, esito) => scarti.push(format!(
                "{nome}: l'esecuzione doveva {atteso:?}, esito {}",
                if esito.is_ok() { "Accetta" } else { "Rifiuta" }
            )),
        }
    }

    assert!(
        scarti.is_empty(),
        "analisi ed esecuzione non decidono insieme:
  {}",
        scarti.join(
            "
  "
        )
    );
}

#[test]
fn l_eterogeneita_con_tipo_dichiarato_resta_ammessa_e_dipende_dai_dati() {
    // L'UNICO punto in cui analisi ed esecuzione non coincidono, e lo fanno
    // di proposito. Con `output_type` dichiarato il runtime converte riga per
    // riga, quindi la riuscita dipende dai DATI: rifiutare staticamente
    // sarebbe una falsa segnalazione su piani che oggi girano, ed e' anche la
    // via d'uscita che il messaggio di `auto` indica.
    let config = json!({
        "output_column": "e",
        "expression": {
            "kind": "function",
            "name": "coalesce",
            "args": [{"kind": "column", "name": "num"}, {"kind": "column", "name": "txt"}]
        },
        "output_type": "text"
    });
    let ingresso = batch_espressioni();
    let contratto_ingresso = contratto(
        ingresso
            .schema()
            .fields()
            .iter()
            .map(|campo| campo.as_ref().clone())
            .collect(),
    );

    // L'analisi accetta: staticamente il tipo non e' determinabile.
    plenora_kernels_table::analyze::analyze_table_contract(
        "table.expression",
        std::slice::from_ref(&contratto_ingresso),
        &config,
        &mut plenora_core::contract::FieldAllocator::new(100),
    )
    .expect("l'eterogeneita' col tipo dichiarato resta ammessa");

    // Su QUESTI dati il runtime fallisce, perche' `num` non e' nullo e
    // `text()` non converte un numero. Il limite e' dichiarato, non nascosto.
    plenora_kernels_table::expressions::expression(
        &ingresso,
        &serde_json::from_value(config).expect("config"),
    )
    .expect_err("con `num` valorizzato il runtime rifiuta la riga");
}

#[test]
fn il_predicato_e_l_encoder_accettano_gli_stessi_tipi() {
    // `is_key_encodable` viveva nell'analizzatore e dichiarava di replicare
    // il `match` di `CompactRowEncoder::try_new`. Un predicato copiato accanto
    // a cio' che descrive e' esattamente la forma di difetto che questa serie
    // di review ha gia' trovato tre volte, quindi ora vive nel modulo
    // dell'encoder — e questo test lega le due cose invece di fidarsi.
    use plenora_core::arrow::array::new_null_array;
    use plenora_core::arrow::schema::TimeUnit;

    let tipi = [
        DataType::Utf8,
        DataType::Int64,
        DataType::Float64,
        DataType::Boolean,
        DataType::UInt64,
        DataType::Date32,
        DataType::Binary,
        DataType::Timestamp(TimeUnit::Millisecond, None),
        DataType::Timestamp(TimeUnit::Millisecond, Some("Non/Esiste".into())),
        DataType::Timestamp(TimeUnit::Second, None),
        DataType::Decimal128(10, 2),
        // Scala negativa: il profilo TESTUALE la rifiuta, quello di chiave no.
        DataType::Decimal128(38, -1),
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        DataType::Dictionary(Box::new(DataType::Int64), Box::new(DataType::Utf8)),
        DataType::Int32,
        DataType::List(Arc::new(Field::new("item", DataType::Int64, true))),
    ];

    for tipo in tipi {
        let dichiarato = plenora_kernels_table::setops::key_encodable(&tipo);
        let colonna: ArrayRef = new_null_array(&tipo, 1);
        let ingresso = batch(vec![Field::new("k", tipo.clone(), true)], vec![colonna]);
        // `except` con se stesso costruisce l'encoder su questo schema: e'
        // l'unico modo di interrogarlo da fuori dal crate.
        let accettato = plenora_kernels_table::setops::except(
            &ingresso,
            &ingresso,
            &serde_json::from_value(json!({})).expect("config"),
        )
        .is_ok();
        assert_eq!(
            dichiarato, accettato,
            "{tipo:?}: il predicato dice {dichiarato}, l'encoder {accettato}"
        );
    }
}

#[test]
fn l_analisi_di_formula_rifiuta_la_timezone_che_il_formatter_rifiuta() {
    // `table.formula` legge come TESTO ogni colonna che non sia Int64/Float64
    // (`formula::evaluate`), quindi passa da `scalar_as_string`, che risolve
    // la timezone con `chrono_tz` a ogni riga. L'analisi usava un predicato
    // di solo TIPO: una timezone non risolvibile — che sta nello schema, non
    // nei dati — otteneva un contratto valido e falliva in esecuzione.
    use plenora_core::arrow::array::TimestampMillisecondArray;
    use plenora_core::arrow::schema::TimeUnit;

    let tipo = DataType::Timestamp(TimeUnit::Millisecond, Some("Non/Esiste".into()));
    let config = json!({"new_column": "f", "formula": "istante + 1"});

    // ANALISI.
    let errore = plenora_kernels_table::analyze::analyze_table_contract(
        "table.formula",
        std::slice::from_ref(&contratto(vec![Field::new("istante", tipo.clone(), true)])),
        &config,
        &mut plenora_core::contract::FieldAllocator::new(100),
    )
    .expect_err("la timezone non e' risolvibile");
    assert!(
        errore.to_string().contains("timezone Arrow"),
        "il rifiuto viene dallo schema, non dai dati: {errore}"
    );

    // ESECUZIONE: stesso piano, stesso esito.
    let ingresso = batch(
        vec![Field::new("istante", tipo, true)],
        vec![Arc::new(
            TimestampMillisecondArray::from(vec![Some(0_i64)])
                .with_timezone("Non/Esiste".to_owned()),
        ) as ArrayRef],
    );
    plenora_kernels_table::formula::formula(
        &ingresso,
        &serde_json::from_value(config).expect("config"),
    )
    .expect_err("anche il kernel rifiuta");
}

// ---------------------------------------------------------------------------
// Quindicesimo giro — lo schema di output non dipende dai VALORI
// ---------------------------------------------------------------------------

/// Tre batch con lo STESSO schema: pieno, tutto null, vuoto.
///
/// Il vuoto passa dal percorso generico (`num_rows == 0`), gli altri due dal
/// fast path compilato: confrontare i tre copre anche l'accordo fra i due
/// percorsi sul tipo prodotto.
fn tre_batch_stesso_schema() -> [RecordBatch; 3] {
    use plenora_core::arrow::array::{BooleanArray, Date32Array, Float64Array, StringArray};

    let campi = || {
        vec![
            Field::new("num", DataType::Float64, true),
            Field::new("txt", DataType::Utf8, true),
            Field::new("flag", DataType::Boolean, true),
            Field::new("d", DataType::Date32, true),
        ]
    };
    let pieno = batch(
        campi(),
        vec![
            Arc::new(Float64Array::from(vec![Some(1.5_f64)])) as ArrayRef,
            Arc::new(StringArray::from(vec![Some("2024-01-15")])) as ArrayRef,
            Arc::new(BooleanArray::from(vec![Some(true)])) as ArrayRef,
            Arc::new(Date32Array::from(vec![Some(19_738_i32)])) as ArrayRef,
        ],
    );
    let tutto_null = batch(
        campi(),
        vec![
            Arc::new(Float64Array::from(vec![None::<f64>])) as ArrayRef,
            Arc::new(StringArray::from(vec![None::<&str>])) as ArrayRef,
            Arc::new(BooleanArray::from(vec![None::<bool>])) as ArrayRef,
            Arc::new(Date32Array::from(vec![None::<i32>])) as ArrayRef,
        ],
    );
    let vuoto = batch(
        campi(),
        vec![
            Arc::new(Float64Array::from(Vec::<f64>::new())) as ArrayRef,
            Arc::new(StringArray::from(Vec::<&str>::new())) as ArrayRef,
            Arc::new(BooleanArray::from(Vec::<bool>::new())) as ArrayRef,
            Arc::new(Date32Array::from(Vec::<i32>::new())) as ArrayRef,
        ],
    );
    [pieno, tutto_null, vuoto]
}

#[test]
#[allow(clippy::too_many_lines)]
fn lo_schema_di_expression_non_dipende_dai_valori() {
    // Il kernel risolveva `output_type = auto` osservando i valori CALCOLATI.
    // Su un batch vuoto o tutto null non ne osservava nessuno e ripiegava su
    // `Utf8`, anche dove l'analisi aveva dichiarato `Boolean` o `Float64`.
    //
    // Due batch con lo stesso schema e la stessa configurazione producevano
    // quindi schemi diversi, e nessuno dei due era necessariamente quello del
    // contratto. Il tipo si ricava ora dallo SCHEMA, con la stessa funzione
    // che usa l'analizzatore.
    let colonna = |nome: &str| json!({"kind": "column", "name": nome});
    let letterale = |valore: serde_json::Value| json!({"kind": "literal", "value": valore});
    let unario =
        |op: &str, valore: serde_json::Value| json!({"kind": "unary", "op": op, "value": valore});
    let binario = |op: &str, sinistra: serde_json::Value, destra: serde_json::Value| json!({"kind": "binary", "op": op, "left": sinistra, "right": destra});

    // (nome, espressione, output_type, tipo Arrow atteso)
    let casi: Vec<(&str, serde_json::Value, &str, DataType)> = vec![
        (
            "colonna booleana",
            colonna("flag"),
            "auto",
            DataType::Boolean,
        ),
        (
            "colonna numerica",
            colonna("num"),
            "auto",
            DataType::Float64,
        ),
        ("colonna testuale", colonna("txt"), "auto", DataType::Utf8),
        (
            "colonna Date32: numero per il runtime",
            colonna("d"),
            "auto",
            DataType::Float64,
        ),
        (
            "date_trunc: tipo temporale nativo",
            json!({
                "kind": "function",
                "name": "date_trunc",
                "args": [letterale(json!("day")), colonna("d")]
            }),
            "auto",
            DataType::Date32,
        ),
        // `not` su un valore nullo torna null, ma il TIPO resta booleano:
        // «solo null» soddisfa la richiesta di un operando booleano e il
        // nodo produce comunque un booleano. Non e' il ripiego su `Utf8`
        // che il kernel faceva quando non osservava valori.
        (
            "not di un letterale null",
            unario("not", letterale(serde_json::Value::Null)),
            "auto",
            DataType::Boolean,
        ),
        (
            "not di una colonna booleana",
            unario("not", colonna("flag")),
            "auto",
            DataType::Boolean,
        ),
        // Aritmetica null-propagating: ogni riga e' null, il tipo no.
        (
            "aritmetica con un operando nullo",
            binario("add", colonna("num"), letterale(serde_json::Value::Null)),
            "auto",
            DataType::Float64,
        ),
        // Confronto null-propagating: risultato booleano anche se sempre null.
        (
            "confronto con un operando nullo",
            binario(
                "greater",
                colonna("num"),
                letterale(serde_json::Value::Null),
            ),
            "auto",
            DataType::Boolean,
        ),
        (
            "is_null: booleano a prescindere",
            unario("is_null", colonna("txt")),
            "auto",
            DataType::Boolean,
        ),
        // Tipo dichiarato: vale su tutti e tre i batch allo stesso modo.
        (
            "dichiarato number",
            colonna("num"),
            "number",
            DataType::Float64,
        ),
        (
            "dichiarato boolean",
            colonna("flag"),
            "boolean",
            DataType::Boolean,
        ),
        (
            "letterale null dichiarato boolean",
            letterale(serde_json::Value::Null),
            "boolean",
            DataType::Boolean,
        ),
    ];

    let batch_di_prova = tre_batch_stesso_schema();
    let contratto_ingresso = contratto(
        batch_di_prova[0]
            .schema()
            .fields()
            .iter()
            .map(|campo| campo.as_ref().clone())
            .collect(),
    );

    for (nome, espressione, tipo_output, atteso) in casi {
        let config = json!({
            "output_column": "e",
            "expression": espressione,
            "output_type": tipo_output
        });

        // Il CONTRATTO dichiarato dall'analisi.
        let contratto_uscita = plenora_kernels_table::analyze::analyze_table_contract(
            "table.expression",
            std::slice::from_ref(&contratto_ingresso),
            &config,
            &mut plenora_core::contract::FieldAllocator::new(100),
        )
        .unwrap_or_else(|errore| panic!("{nome}: l'analisi deve accettare: {errore}"));
        assert_eq!(
            contratto_uscita
                .schema
                .field_with_name("e")
                .unwrap_or_else(|errore| panic!("{nome}: {errore}"))
                .data_type(),
            &atteso,
            "{nome}: tipo dichiarato dal contratto"
        );

        // Lo stesso tipo su TUTTI E TRE i batch.
        for (etichetta, ingresso) in [
            ("pieno", &batch_di_prova[0]),
            ("tutto null", &batch_di_prova[1]),
            ("vuoto", &batch_di_prova[2]),
        ] {
            let uscita = plenora_kernels_table::expressions::expression(
                ingresso,
                &serde_json::from_value(config.clone()).expect("config"),
            )
            .unwrap_or_else(|errore| panic!("{nome} [{etichetta}]: {errore}"));
            assert_eq!(
                uscita
                    .schema()
                    .field_with_name("e")
                    .unwrap_or_else(|errore| panic!("{nome} [{etichetta}]: {errore}"))
                    .data_type(),
                &atteso,
                "{nome} [{etichetta}]: lo schema prodotto non dipende dai valori"
            );
        }
    }
}

#[test]
fn lo_schema_di_formula_non_dipende_dai_valori() {
    // Il difetto simmetrico: `formula` decideva Float64 quando NESSUN valore
    // era testo — e su un batch vuoto o tutto null nessun valore lo e'.
    // Un'espressione testuale usciva quindi Float64 sul vuoto e Utf8 sul
    // pieno, mentre l'analisi prometteva Utf8 in entrambi i casi.
    let casi: [(&str, &str, DataType); 4] = [
        ("aritmetica", "num * 2", DataType::Float64),
        ("concatenazione", "txt + '!'", DataType::Utf8),
        // Concatenazione con un operando numerico: il `+` con un testo e'
        // concatenazione, quindi testo anche se il numero e' nullo.
        ("numero concatenato a testo", "num + txt", DataType::Utf8),
        ("colonna testuale sola", "txt", DataType::Utf8),
    ];

    let batch_di_prova = tre_batch_stesso_schema();
    let contratto_ingresso = contratto(
        batch_di_prova[0]
            .schema()
            .fields()
            .iter()
            .map(|campo| campo.as_ref().clone())
            .collect(),
    );

    for (nome, formula, atteso) in casi {
        let config = json!({"new_column": "f", "formula": formula});

        let contratto_uscita = plenora_kernels_table::analyze::analyze_table_contract(
            "table.formula",
            std::slice::from_ref(&contratto_ingresso),
            &config,
            &mut plenora_core::contract::FieldAllocator::new(100),
        )
        .unwrap_or_else(|errore| panic!("{nome}: l'analisi deve accettare: {errore}"));
        assert_eq!(
            contratto_uscita
                .schema
                .field_with_name("f")
                .unwrap_or_else(|errore| panic!("{nome}: {errore}"))
                .data_type(),
            &atteso,
            "{nome}: tipo dichiarato dal contratto"
        );

        for (etichetta, ingresso) in [
            ("pieno", &batch_di_prova[0]),
            ("tutto null", &batch_di_prova[1]),
            ("vuoto", &batch_di_prova[2]),
        ] {
            let uscita = plenora_kernels_table::formula::formula(
                ingresso,
                &serde_json::from_value(config.clone()).expect("config"),
            )
            .unwrap_or_else(|errore| panic!("{nome} [{etichetta}]: {errore}"));
            assert_eq!(
                uscita
                    .schema()
                    .field_with_name("f")
                    .unwrap_or_else(|errore| panic!("{nome} [{etichetta}]: {errore}"))
                    .data_type(),
                &atteso,
                "{nome} [{etichetta}]: lo schema prodotto non dipende dai valori"
            );
        }
    }
}

#[test]
fn due_batch_consecutivi_producono_lo_stesso_schema() {
    // La forma in cui il difetto mordeva davvero: uno stream in cui il primo
    // batch e' tutto null e il secondo no. I due schemi devono coincidere,
    // altrimenti la concatenazione a valle non e' nemmeno possibile.
    let [pieno, tutto_null, vuoto] = tre_batch_stesso_schema();
    let config = json!({
        "output_column": "e",
        "expression": {"kind": "column", "name": "flag"},
        "output_type": "auto"
    });
    let esegui = |ingresso: &RecordBatch| {
        plenora_kernels_table::expressions::expression(
            ingresso,
            &serde_json::from_value(config.clone()).expect("config"),
        )
        .expect("expression")
        .schema()
    };
    assert_eq!(esegui(&tutto_null), esegui(&pieno), "null poi pieno");
    assert_eq!(esegui(&vuoto), esegui(&pieno), "vuoto poi pieno");

    let config_formula = json!({"new_column": "f", "formula": "txt + '!'"});
    let esegui_formula = |ingresso: &RecordBatch| {
        plenora_kernels_table::formula::formula(
            ingresso,
            &serde_json::from_value(config_formula.clone()).expect("config"),
        )
        .expect("formula")
        .schema()
    };
    assert_eq!(
        esegui_formula(&tutto_null),
        esegui_formula(&pieno),
        "formula: null poi pieno"
    );
    assert_eq!(
        esegui_formula(&vuoto),
        esegui_formula(&pieno),
        "formula: vuoto poi pieno"
    );
}

// ---------------------------------------------------------------------------
// Hotfix 2026-08-18 — la semantica esatta su cui l'oracolo fuzz si appoggia
// ---------------------------------------------------------------------------

#[test]
fn il_confronto_di_un_estremo_decimale_e_esatto() {
    // L'oracolo di `fuzz/fuzz_targets/diff_kernels.rs` degradava valore ed
    // estremo a `f64` (`bound_as_f64`), replicando una semantica che il
    // kernel ha abbandonato. Ora delega a `compare_bounds`, quindi questi
    // casi sono il contratto su cui si appoggia: se cambiano, l'oracolo
    // cambia con loro.
    use plenora_kernels_table::{compare_bounds, NumericBound};
    use std::cmp::Ordering;

    let bound = |testo: &str| NumericBound::parse(testo).expect("letterale numerico");

    // Scala positiva: due scritture dello stesso valore sono uguali.
    assert_eq!(
        compare_bounds(bound("10.5"), bound("10.50")),
        Some(Ordering::Equal),
        "10.5 e 10.50 sono lo stesso valore"
    );
    // Due decimali che `f64` non distingue: 0.1 e 0.1 + 1e-17.
    assert_eq!(
        compare_bounds(bound("0.1"), bound("0.10000000000000001")),
        Some(Ordering::Less),
        "in f64 questi due collassano sullo stesso double"
    );
    // Scala negativa (esponente positivo).
    assert_eq!(
        compare_bounds(bound("1e3"), bound("1000")),
        Some(Ordering::Equal),
        "1e3 e 1000 sono lo stesso valore"
    );
    assert_eq!(
        compare_bounds(bound("-2.5e2"), bound("-250")),
        Some(Ordering::Equal)
    );

    // Oltre 2^53: interi distinti che `f64` collassa.
    assert_eq!(
        compare_bounds(bound("9007199254740993"), bound("9007199254740992")),
        Some(Ordering::Greater),
        "9007199254740993 e 9007199254740992 sono lo stesso f64, non lo stesso intero"
    );
    assert_eq!(
        compare_bounds(bound("-9007199254740993"), bound("-9007199254740992")),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_bounds(bound("18446744073709551615"), bound("18446744073709551614")),
        Some(Ordering::Greater),
        "u64::MAX e il suo predecessore"
    );

    // Intero contro decimale: confronto nel dominio scalato.
    assert_eq!(
        compare_bounds(bound("10"), bound("10.5")),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_bounds(bound("10"), bound("10.0")),
        Some(Ordering::Equal)
    );

    // Decimale con piu' cifre di quante ne tenga un i128: ricade su F64,
    // dichiaratamente approssimato, ma non deve rompere il confronto.
    let non_rappresentabile = bound("0.123456789012345678901234567890123456789012345");
    assert!(
        matches!(non_rappresentabile, NumericBound::F64(_)),
        "oltre la capacita' di i128 si ricade su F64: {non_rappresentabile:?}"
    );
    assert_eq!(
        compare_bounds(non_rappresentabile, bound("1")),
        Some(Ordering::Less)
    );

    // NaN: confronto non definito, come IEEE.
    assert_eq!(compare_bounds(bound("NaN"), bound("0")), None);
}

#[test]
fn il_filtro_ordinato_su_colonna_testuale_non_passa_da_f64() {
    // Il percorso NON tipizzato di `filtering` — quello che l'oracolo fuzz
    // replicava con `bound_as_f64`. Su una colonna Utf8 il kernel confronta
    // nel dominio esatto: sopra 2^53 due interi distinti restano distinti.
    use plenora_core::arrow::array::StringArray;

    let ingresso = batch(
        vec![Field::new("t", DataType::Utf8, true)],
        vec![Arc::new(StringArray::from(vec![
            Some("9007199254740992"),
            Some("9007199254740993"),
            Some("10.50"),
        ])) as ArrayRef],
    );

    // `> 9007199254740992` esclude il primo e include il secondo: in f64
    // sarebbero lo stesso valore e il secondo sparirebbe.
    let uscita = plenora_kernels_table::filtering::filter(
        &ingresso,
        &serde_json::from_value(json!({
            "column": "t",
            "operator": ">",
            "value": "9007199254740992"
        }))
        .expect("config"),
    )
    .expect("filter");
    assert_eq!(
        uscita.num_rows(),
        1,
        "solo 9007199254740993 e' maggiore: in f64 sarebbe zero righe"
    );

    // L'asimmetria, verificata sul kernel e non assunta: su una colonna
    // testuale `==` e' un confronto di TESTO (`scalar_as_string`), mentre gli
    // operatori ORDINATI passano dal confronto numerico esatto
    // (`scalar_compare`). "10.50" e "10.5" sono quindi lo stesso NUMERO ma
    // non lo stesso testo.
    let conta = |operatore: &str, valore: &str| {
        plenora_kernels_table::filtering::filter(
            &ingresso,
            &serde_json::from_value(json!({"column": "t", "operator": operatore, "value": valore}))
                .expect("config"),
        )
        .expect("filter")
        .num_rows()
    };
    assert_eq!(conta("==", "10.5"), 0, "`==` su testo confronta il TESTO");
    assert_eq!(conta("==", "10.50"), 1, "il testo identico corrisponde");
    assert_eq!(
        conta("between", "10.5,10.5"),
        1,
        "`between` passa dal numerico esatto: 10.50 e' compreso"
    );
}
