use std::sync::Arc;

use plenora_core::arrow::array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_engine::{execute_batch, execute_binary, Limits};
use serde_json::json;

// Ogni file usa una parte delle fixture: il resto serve agli altri.
#[allow(dead_code)]
mod fixture_table;
use fixture_table::{i64s, piano, plan, plan_with_limits, utf8};

fn unary_fixture() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("group", DataType::Utf8, true),
            Field::new("kind", DataType::Utf8, false),
            Field::new("num", DataType::Float64, true),
            Field::new("text", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                None,
            ])),
            Arc::new(StringArray::from(vec![
                Some("a"),
                Some("a"),
                Some("b"),
                Some("b"),
                None,
            ])),
            Arc::new(StringArray::from(vec!["x", "y", "x", "y", "x"])),
            Arc::new(Float64Array::from(vec![
                Some(1.0),
                Some(2.0),
                Some(3.0),
                Some(4.0),
                None,
            ])),
            Arc::new(StringArray::from(vec![
                Some("u"),
                Some("v"),
                Some("w"),
                None,
                Some("z"),
            ])),
        ],
    )
    .expect("fixture")
}

fn table(ids: Vec<Option<i64>>, values: Vec<Option<&str>>, suffix: &str) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("value", DataType::Utf8, true),
            Field::new(suffix, DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(StringArray::from(values)),
            Arc::new(StringArray::from(vec![suffix; 4])),
        ],
    )
    .expect("table")
}

fn left() -> RecordBatch {
    table(
        vec![Some(1), Some(2), Some(3), None],
        vec![Some("old"), Some("same"), Some("gone"), Some("null-old")],
        "left",
    )
}

fn right() -> RecordBatch {
    table(
        vec![Some(1), Some(2), Some(4), None],
        vec![Some("new"), Some("same"), Some("added"), Some("null-new")],
        "right",
    )
}

#[test]
fn melt_transpose_and_all_pivot_aggregations_are_bounded() {
    let input = unary_fixture();
    let melt = execute_batch(input.clone(), &plan("melt", json!({"id_columns":["group"],"value_columns":["num","text"],"var_name":"variable","value_name":"value","type_policy":"string"}))).expect("melt");
    assert_eq!(melt.num_rows(), 10);
    assert_eq!(utf8(&melt, "variable").value(0), "num");
    let inferred = execute_batch(input.clone(), &plan("melt", json!({"id_columns":["id","group","kind","num"],"value_columns":[],"var_name":"kind","value_name":"value"}))).expect("melt collision");
    assert!(inferred.column_by_name("kind_1").is_some());
    assert!(piano(
        "melt",
        json!({"id_columns":[],"value_columns":["num"],"var_name":"x","value_name":"x"}),
        Limits::default()
    )
    .validate()
    .is_err());

    let transpose = execute_batch(
        input.clone(),
        &plan(
            "transpose",
            json!({"id_column":"group","output_columns":["r0","r1","r2","r3","r4"],"type_policy":"string"}),
        ),
    )
    .expect("transpose");
    assert_eq!(transpose.num_rows(), 4);
    assert_eq!(transpose.num_columns(), 6);
    assert_eq!(utf8(&transpose, "r0").value(0), "1");

    for aggregation in [
        "first", "last", "max", "min", "sum", "mean", "count", "concat",
    ] {
        let pivot = execute_batch(input.clone(), &plan("pivot", json!({"index_col":"group","pivot_col":"kind","value_col":"num","aggr_func":aggregation,"mapping":{"x":"X","y":"Y"}}))).unwrap_or_else(|error| panic!("{aggregation}: {error}"));
        assert_eq!(pivot.num_rows(), 3);
        assert_eq!(pivot.num_columns(), 3);
    }
    // Il tetto d'uscita del pivot. Le righe d'uscita sono le chiavi
    // distinte, mai piu' delle righe d'ingresso: un `max_rows` che le
    // fermasse fermerebbe prima l'ingresso. Il tetto raggiungibile e' sulle
    // colonne, una per valore distinto di `pivot_col`: tre colonne
    // d'ingresso stanno in `max_columns: 4`, le cinque d'uscita (indice piu'
    // quattro valori) no.
    let wide = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("group", DataType::Utf8, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("num", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["a", "a", "b", "b"])),
            Arc::new(StringArray::from(vec!["k1", "k2", "k3", "k4"])),
            Arc::new(Float64Array::from(vec![1.0, 2.0, 3.0, 4.0])),
        ],
    )
    .expect("pivot fixture");
    let tight = Limits {
        max_columns: 4,
        ..Limits::default()
    };
    let error = execute_batch(
        wide,
        &plan_with_limits(
            "pivot",
            json!({"index_col":"group","pivot_col":"kind","value_col":"num","aggr_func":"sum","mapping":{}}),
            tight,
        ),
    )
    .expect_err("pivot oltre max_columns in uscita");
    assert_eq!(
        error.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("pivot supera i limiti di output"),
        "il rifiuto deve venire dal tetto d'uscita del pivot, non dall'ingresso: {error}"
    );
}

#[test]
fn melt_and_transpose_preserve_homogeneous_arrow_types() {
    let input = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Int64, true),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["r0", "r1"])),
            Arc::new(Int64Array::from(vec![Some(1), None])),
            Arc::new(Int64Array::from(vec![Some(3), Some(4)])),
        ],
    )
    .expect("typed reshape fixture");
    let melted = execute_batch(
        input.clone(),
        &plan(
            "melt",
            json!({"id_columns":["id"],"value_columns":["a","b"]}),
        ),
    )
    .expect("typed melt");
    assert_eq!(
        melted.column_by_name("value").unwrap().data_type(),
        &DataType::Int64
    );
    assert_eq!(i64s(&melted, "value").value(0), 1);
    assert!(i64s(&melted, "value").is_null(1));

    let transposed = execute_batch(
        input.clone(),
        &plan(
            "transpose",
            json!({"id_column":"id","output_columns":["r0","r1"]}),
        ),
    )
    .expect("typed transpose");
    assert_eq!(
        transposed.column_by_name("r0").unwrap().data_type(),
        &DataType::Int64
    );
    assert_eq!(i64s(&transposed, "r0").values(), &[1, 3]);
    assert!(i64s(&transposed, "r1").is_null(0));

    let short_strings = Limits {
        max_string_bytes: 1,
        ..Limits::default()
    };
    assert!(execute_batch(
        input.clone(),
        &plan_with_limits(
            "melt",
            json!({"id_columns":["b"],"value_columns":["id","a"],"type_policy":"string"}),
            short_strings.clone(),
        ),
    )
    .is_err());
    assert!(execute_batch(
        input,
        &plan_with_limits(
            "transpose",
            json!({"id_column":null,"type_policy":"string"}),
            short_strings,
        ),
    )
    .is_err());

    let heterogeneous = unary_fixture();
    assert!(execute_batch(
        heterogeneous.clone(),
        &plan(
            "melt",
            json!({"id_columns":["group"],"value_columns":["num","text"]}),
        ),
    )
    .is_err());
    assert!(execute_batch(
        heterogeneous,
        &plan("transpose", json!({"id_column":"group"})),
    )
    .is_err());
}

#[test]
fn joins_cover_all_modes_null_semantics_and_key_coalescing() {
    let left = left();
    let right = right();
    for (how, rows) in [("inner", 2), ("left", 4), ("right", 4), ("outer", 6)] {
        let output = execute_binary(
            &left,
            &right,
            &plan(
                "join",
                json!({"left_keys":["id"],"right_keys":["id"],"how":how}),
            ),
        )
        .unwrap_or_else(|error| panic!("{how}: {error}"));
        assert_eq!(output.num_rows(), rows, "{how}");
        assert_eq!(output.num_columns(), 5, "{how}");
        assert!(output.column_by_name("value_L").is_some(), "{how}");
        assert!(output.column_by_name("value_R").is_some(), "{how}");
        if how == "right" || how == "outer" {
            assert!(i64s(&output, "id").iter().flatten().any(|value| value == 4));
        }
    }
    let wrong_type = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)])),
        vec![Arc::new(StringArray::from(vec!["1"]))],
    )
    .expect("wrong type");
    assert!(execute_binary(
        &left,
        &wrong_type,
        &plan(
            "join",
            json!({"left_keys":["id"],"right_keys":["id"],"how":"inner"})
        )
    )
    .is_err());
    // Il tetto d'uscita del join: quattro righe per lato stanno in
    // `max_rows: 4`; l'outer ne produce sei (due corrispondenze, 3 e null a
    // sinistra, 4 e null a destra: il null non corrisponde).
    let tight = Limits {
        max_rows: 4,
        ..Limits::default()
    };
    let error = execute_binary(
        &left,
        &right,
        &plan_with_limits(
            "join",
            json!({"left_keys":["id"],"right_keys":["id"],"how":"outer"}),
            tight,
        ),
    )
    .expect_err("outer oltre max_rows in uscita");
    assert_eq!(
        error.category(),
        plenora_core::ErrorCategory::ResourceLimit,
        "{error}"
    );
    assert!(
        error.to_string().contains("join supera max_rows"),
        "il rifiuto deve venire dal tetto d'uscita del join, non dall'ingresso: {error}"
    );
}

#[test]
fn concat_and_cross_join_enforce_schema_and_cardinality() {
    let left = left();
    let concatenated = execute_binary(&left, &left, &plan("concat", json!({"ignore_index":false})))
        .expect("concat");
    assert_eq!(concatenated.num_rows(), 8);
    assert_eq!(utf8(&concatenated, "value").value(4), "old");
    assert!(execute_binary(
        &left,
        &right(),
        &plan("concat", json!({"ignore_index":true}))
    )
    .is_err());

    let cross =
        execute_binary(&left, &right(), &plan("cross_join", json!({}))).expect("cross join");
    assert_eq!(cross.num_rows(), 16);
    assert_eq!(cross.num_columns(), 6);
    assert!(cross.column_by_name("id_x").is_some());
    assert!(cross.column_by_name("id_y").is_some());
    assert!(cross.column_by_name("value_x").is_some());
    assert!(cross.column_by_name("value_y").is_some());
    assert!(cross.column_by_name("left").is_some());
    assert!(cross.column_by_name("right").is_some());
    let tight = Limits {
        max_rows: 15,
        ..Limits::default()
    };
    assert!(execute_binary(
        &left,
        &right(),
        &plan_with_limits("cross_join", json!({}), tight)
    )
    .is_err());
}

#[test]
fn table_diff_reports_every_status_and_rejects_ambiguous_keys() {
    let output = execute_binary(&left(), &right(), &plan("table_diff", json!({"left_keys":["id"],"right_keys":["id"],"compare_columns":["value"],"include_unchanged":"yes","separator":"|"}))).expect("diff");
    assert_eq!(output.num_rows(), 5);
    let statuses = utf8(&output, "_diff_status")
        .iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(
        output.column_by_name("id").unwrap().data_type(),
        &DataType::Int64
    );
    assert_eq!(
        output.column_by_name("value").unwrap().data_type(),
        &DataType::Utf8
    );
    let unchanged = statuses
        .iter()
        .position(|status| *status == "UNCHANGED")
        .expect("unchanged row");
    assert!(utf8(&output, "_diff_columns").is_null(unchanged));
    assert!(utf8(&output, "_diff_old_values").is_null(unchanged));
    for expected in ["ADDED", "DELETED", "MODIFIED", "UNCHANGED"] {
        assert!(statuses.contains(&expected), "{expected}");
    }
    let changed = statuses
        .iter()
        .filter(|status| **status == "MODIFIED")
        .count();
    assert_eq!(changed, 2);
    let without_unchanged = execute_binary(&left(), &right(), &plan("table_diff", json!({"left_keys":["id"],"right_keys":["id"],"compare_columns":[],"include_unchanged":"no","separator":"#"}))).expect("diff inferred");
    assert!(without_unchanged.num_rows() < output.num_rows());

    let duplicate = table(
        vec![Some(1), Some(1), Some(2), Some(3)],
        vec![Some("a"), Some("b"), Some("c"), Some("d")],
        "left",
    );
    assert!(execute_binary(&duplicate, &right(), &plan("table_diff", json!({"left_keys":["id"],"right_keys":["id"],"compare_columns":["value"],"include_unchanged":"yes","separator":"#"}))).is_err());
    let tight = Limits {
        max_columns: 3,
        ..Limits::default()
    };
    assert!(execute_binary(&left(), &right(), &plan_with_limits("table_diff", json!({"left_keys":["id"],"right_keys":["id"],"compare_columns":["value"],"include_unchanged":"yes","separator":"#"}), tight)).is_err());
}
