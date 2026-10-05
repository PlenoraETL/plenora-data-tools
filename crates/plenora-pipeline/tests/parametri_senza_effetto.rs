//! Parametri scritti e senza effetto: si rifiutano in validazione e nel
//! kernel, con la stessa regola (una funzione `verifica_*` condivisa).
//!
//! Per ogni caso: il piano con il parametro senza effetto si rifiuta in
//! `validate` con `InvalidPlan` e il frammento della regola; la chiamata
//! diretta del kernel sulle stesse tabelle fallisce con lo stesso
//! frammento; la config gemella, dove il parametro ha effetto (o manca),
//! si valida e gira.

mod comune;

use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::SchemaRef;
use plenora_core::PlenoraError;
use plenora_pipeline::{Passo, Pipeline};
use serde_json::{json, Value};

use comune::{chiamata_diretta, nomi_input, tabelle, Fixture};

fn piano(op: &str, ingressi: &[&str], config: Value) -> Pipeline {
    Pipeline {
        version: 1,
        inputs: ingressi.iter().map(|nome| (*nome).to_owned()).collect(),
        crs: None,
        limits: None,
        steps: vec![Passo {
            out: "uscita".to_owned(),
            op: op.to_owned(),
            inputs: ingressi.iter().map(|nome| (*nome).to_owned()).collect(),
            config,
        }],
        outputs: vec!["uscita".to_owned()],
    }
}

fn e_piano(errore: &PlenoraError) -> bool {
    match errore {
        PlenoraError::InvalidPlan(_) => true,
        PlenoraError::Tagged { source, .. } => e_piano(source),
        _ => false,
    }
}

/// Esito di validazione ed esecuzione del piano di un passo.
fn esegui(op: &str, fixture: Fixture, config: &Value) -> Result<(), PlenoraError> {
    let tavole = tabelle(fixture);
    let ingressi = nomi_input(tavole.len());
    let riferimenti: Vec<&str> = ingressi.iter().map(String::as_str).collect();
    let schemi: Vec<(&str, SchemaRef)> = riferimenti
        .iter()
        .copied()
        .zip(tavole.iter().map(RecordBatch::schema))
        .collect();
    let validata = piano(op, &riferimenti, config.clone()).validate(&schemi)?;
    validata
        .run(ingressi.into_iter().zip(tavole).collect())
        .map(|_| ())
}

struct Caso {
    op: &'static str,
    fixture: Fixture,
    rifiutata: Value,
    frammento: &'static str,
    /// La stessa config dove il parametro ha effetto, o senza il parametro.
    accettata: Value,
}

const fn caso(
    op: &'static str,
    fixture: Fixture,
    rifiutata: Value,
    frammento: &'static str,
    accettata: Value,
) -> Caso {
    Caso {
        op,
        fixture,
        rifiutata,
        frammento,
        accettata,
    }
}

#[allow(clippy::too_many_lines)] // Un caso per regola.
fn casi() -> Vec<Caso> {
    use Fixture::{Binary, Set, Wide};
    vec![
        caso(
            "table.concat",
            Set,
            json!({"ignore_index": true}),
            "ignore_index non ha effetto",
            json!({}),
        ),
        caso(
            "table.sample",
            Wide,
            json!({"n": 3, "fraction": 0.5}),
            "n e fraction insieme",
            json!({"fraction": 0.5}),
        ),
        caso(
            "table.sample",
            Wide,
            json!({"fraction": 0.0, "random_state": 7}),
            "random_state non ha effetto",
            json!({"fraction": 0.5, "random_state": 7}),
        ),
        caso(
            "table.md5_hash",
            Wide,
            json!({"columns": ["name"], "null_literal": "x"}),
            "null_literal ammesso solo",
            json!({"columns": ["name"], "null_policy": "literal", "null_literal": "x"}),
        ),
        caso(
            "table.sha256_hash",
            Wide,
            json!({"columns": ["name"], "null_policy": "error", "null_literal": "x"}),
            "null_literal ammesso solo",
            json!({"columns": ["name"], "null_policy": "error"}),
        ),
        caso(
            "table.date_format",
            Wide,
            json!({"column": "date", "input_format": "%Y-%m-%d", "output_column": "d", "invalid": "null"}),
            "invalid non ha effetto",
            json!({"column": "date", "input_format": "%Y-%m-%d", "output_column": "d"}),
        ),
        caso(
            "table.date_add",
            Wide,
            json!({"column": "date", "input_format": "%Y-%m-%d", "amount": 1, "unit": "days",
                    "output_column": "d", "invalid": "error"}),
            "invalid non ha effetto",
            json!({"column": "date", "input_format": "%Y-%m-%d", "amount": 1, "unit": "days",
                    "output_column": "d"}),
        ),
        caso(
            "table.date_diff",
            Wide,
            json!({"start_column": "date", "end_column": "date2", "input_format": "%Y-%m-%d",
                    "unit": "days", "output_column": "d", "invalid": "null"}),
            "invalid non ha effetto",
            json!({"start_column": "date", "end_column": "date2", "input_format": "%Y-%m-%d",
                    "unit": "days", "output_column": "d"}),
        ),
        caso(
            "table.timezone_convert",
            Wide,
            json!({"column": "date", "input_format": "%Y-%m-%d", "source_timezone": "UTC",
                    "target_timezone": "Europe/Rome", "output_column": "d", "ambiguous": "earliest"}),
            "ambiguous non ha effetto",
            json!({"column": "date", "input_format": "%Y-%m-%d", "source_timezone": "UTC",
                    "target_timezone": "Europe/Rome", "output_column": "d"}),
        ),
        caso(
            "table.date_extract",
            Wide,
            json!({"column": "date", "invalid": "error"}),
            "invalid non ha effetto",
            json!({"column": "date"}),
        ),
        caso(
            "table.date_extract",
            Wide,
            json!({"column": "date", "parts": ["year", "year"]}),
            "parte ripetuta",
            json!({"column": "date", "parts": ["year", "month"]}),
        ),
        caso(
            "table.filter",
            Wide,
            json!({"column": "id", "operator": "isnull", "value": 1}),
            "value non ha effetto",
            json!({"column": "id", "operator": "isnull"}),
        ),
        caso(
            "table.filter",
            Wide,
            json!({"column": "id", "operator": "notnull", "value": null}),
            "value non ha effetto",
            json!({"column": "id", "operator": "==", "value": 1}),
        ),
        caso(
            "table.conditional",
            Wide,
            json!({"column": "id", "conditions": [{"operator": "isnull", "value": 0, "result": 1}]}),
            "value non ha effetto",
            json!({"column": "id", "conditions": [{"operator": "isnull", "result": 1}]}),
        ),
        caso(
            "table.expression",
            Wide,
            json!({"output_column": "e", "expression": {"kind": "column", "name": "id", "type": "int"}}),
            "campo sconosciuto",
            json!({"output_column": "e", "expression": {"kind": "column", "name": "id"}}),
        ),
        caso(
            "table.type_cast",
            Wide,
            json!({"column": "id", "target_type": "str", "errors": "raise"}),
            "errors non ha effetto",
            json!({"column": "id", "target_type": "int", "errors": "raise"}),
        ),
        caso(
            "table.concat_columns",
            Wide,
            json!({"columns": ["name"], "separator": "-"}),
            "separator senza effetto",
            json!({"columns": ["name", "date"], "separator": "-"}),
        ),
        caso(
            "table.split_column",
            Wide,
            json!({"column": "date", "delimiter": "-", "new_columns": ["a"]}),
            "delimiter senza effetto",
            json!({"column": "date", "delimiter": "-", "new_columns": ["a", "b"]}),
        ),
        caso(
            "table.split_column",
            Wide,
            json!({"column": "date", "delimiter": "-", "new_columns": ["a", "b", "c"], "max_splits": 2}),
            "max_splits senza effetto",
            json!({"column": "date", "delimiter": "-", "new_columns": ["a", "b", "c"], "max_splits": 1}),
        ),
        caso(
            "table.string_pad",
            Wide,
            json!({"column": "name", "width": 0}),
            "width 0",
            json!({"column": "name", "width": 3}),
        ),
        caso(
            "table.table_diff",
            Set,
            json!({"left_keys": ["id"], "right_keys": ["id"], "compare_columns": ["name"],
                    "separator": "|"}),
            "separator senza effetto",
            json!({"left_keys": ["id"], "right_keys": ["id"], "compare_columns": ["name", "date"],
                    "separator": "|"}),
        ),
        caso(
            "table.assert_cardinality",
            Wide,
            json!({"min_rows": 0}),
            "min_rows 0",
            json!({"min_rows": 1}),
        ),
        caso(
            "table.assert_range",
            Wide,
            json!({"column": "value", "max": 1000, "inclusive_min": false}),
            "inclusive_min/inclusive_max",
            json!({"column": "value", "max": 1000, "inclusive_max": false}),
        ),
        caso(
            "table.top_n",
            Wide,
            json!({"columns": ["id"], "n": 0}),
            "n 0",
            json!({"columns": ["id"], "n": 1}),
        ),
        caso(
            "table.limit",
            Wide,
            json!({"n": 0, "offset": 1}),
            "offset senza effetto",
            json!({"n": 1, "offset": 1}),
        ),
        caso(
            "table.aggregate",
            Wide,
            json!({"group_by": ["name"], "aggregations": [{"column": "name", "function": "count"}]}),
            "uguale a una colonna di group_by",
            json!({"group_by": ["name"], "aggregations": [{"column": "name", "function": "count",
                                                             "alias": "quanti"}]}),
        ),
        caso(
            "table.aggregate",
            Wide,
            json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "mean"},
                                                            {"column": "value", "function": "avg"}]}),
            "ripetuto",
            json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "mean"},
                                                            {"column": "value", "function": "sum"}]}),
        ),
        caso(
            "table.statistics",
            Wide,
            json!({"column": "value", "stats": ["mean", "mean"]}),
            "statistica ripetuta",
            json!({"column": "value", "stats": ["mean", "sum"]}),
        ),
        caso(
            "table.mask_data",
            Wide,
            json!({"maskings": [{"column": "name", "mask_type": "email"},
                                 {"column": "name", "mask_type": "phone"}]}),
            "ripetuta in maskings",
            json!({"maskings": [{"column": "name", "mask_type": "email"},
                                 {"column": "name", "mask_type": "phone"}], "overwrite": true}),
        ),
        caso(
            "table.rename",
            Wide,
            json!({"renames": [{"old_name": "name", "new_name": "name"}]}),
            "su se stessa",
            json!({"renames": [{"old_name": "name", "new_name": "nome"}]}),
        ),
        caso(
            "table.rename",
            Wide,
            json!({"renames": [{"old_name": "assente", "new_name": "z"},
                               {"old_name": "name", "new_name": "z"}]}),
            "destinazione: colonna ripetuta",
            json!({"renames": [{"old_name": "assente", "new_name": "y"},
                               {"old_name": "name", "new_name": "z"}]}),
        ),
        caso(
            "table.reorder_columns",
            Wide,
            json!({}),
            "non sposta niente",
            json!({"alphabetical": true}),
        ),
        caso(
            "table.pivot",
            Wide,
            json!({"index_col": "id,,flag", "pivot_col": "name", "value_col": "value",
                    "mapping": {"a": "col_a"}}),
            "voce vuota in index_col",
            json!({"index_col": "id,flag", "pivot_col": "name", "value_col": "value",
                    "mapping": {"a": "col_a"}}),
        ),
        caso(
            "table.asof_join",
            Binary,
            json!({"left_on": "id", "right_on": "rid", "tolerance": 0, "allow_exact": false}),
            "non abbina mai",
            json!({"left_on": "id", "right_on": "rid", "tolerance": 0}),
        ),
        caso(
            "table.expression",
            Wide,
            json!({"output_column": "e", "on_division_by_zero": "null",
                    "expression": {"kind": "column", "name": "value"}}),
            "on_division_by_zero senza effetto",
            json!({"output_column": "e", "on_division_by_zero": "null",
                    "expression": {"kind": "binary", "op": "divide",
                                   "left": {"kind": "column", "name": "value"},
                                   "right": {"kind": "column", "name": "id"}}}),
        ),
        caso(
            "table.formula",
            Wide,
            json!({"new_column": "f", "formula": "value * 2", "on_division_by_zero": "error"}),
            "on_division_by_zero senza effetto",
            json!({"new_column": "f", "formula": "value / 2", "on_division_by_zero": "error"}),
        ),
    ]
}

#[test]
fn i_parametri_senza_effetto_si_rifiutano_in_validazione_e_nel_kernel() {
    let mut difetti = Vec::new();
    for caso in casi() {
        let tavole = tabelle(caso.fixture);
        match esegui(caso.op, caso.fixture, &caso.rifiutata) {
            Err(errore) if e_piano(&errore) && errore.to_string().contains(caso.frammento) => {}
            altro => difetti.push(format!(
                "{} {}: validazione, atteso `{}`, avuto {altro:?}",
                caso.op, caso.rifiutata, caso.frammento
            )),
        }
        match chiamata_diretta(caso.op, &caso.rifiutata, &tavole) {
            Err(errore) if e_piano(&errore) && errore.to_string().contains(caso.frammento) => {}
            altro => difetti.push(format!(
                "{} {}: kernel, atteso `{}`, avuto {:?}",
                caso.op,
                caso.rifiutata,
                caso.frammento,
                altro.map(|uscita| uscita.num_rows())
            )),
        }
        if let Err(errore) = esegui(caso.op, caso.fixture, &caso.accettata) {
            difetti.push(format!(
                "{} {}: gemella rifiutata: {errore}",
                caso.op, caso.accettata
            ));
        }
    }
    assert!(difetti.is_empty(), "{}", difetti.join("\n"));
}

/// I parametri che non hanno effetto solo con certi ingressi si accettano:
/// lo stesso piano deve girare su tabelle diverse,
/// e il rifiuto dipenderebbe dallo schema, non dalla config.
#[test]
fn i_parametri_senza_effetto_solo_con_certi_ingressi_si_accettano() {
    use Fixture::Wide;
    let tutte = json!([
        {"name": "id", "type": "Int64"}, {"name": "name", "type": "Utf8"},
        {"name": "value", "type": "Float64"}, {"name": "flag", "type": "Boolean"},
        {"name": "date", "type": "Utf8"}, {"name": "date2", "type": "Utf8"},
        {"name": "json", "type": "Utf8"}, {"name": "geom", "type": "Binary"}
    ]);
    let casi = [
        (
            "table.align_schema",
            json!({"columns": [{"name": "id", "type": "Int64", "default": 1}]}),
        ),
        (
            "table.align_schema",
            json!({"columns": tutte, "keep_extra": true}),
        ),
        ("table.drop_columns", json!({"columns": ["assente"]})),
        (
            "table.rename",
            json!({"renames": [{"old_name": "assente", "new_name": "x"}]}),
        ),
        (
            "table.reorder_columns",
            json!({"columns": ["id", "name", "value", "flag", "date", "date2", "json"],
                   "alphabetical": true}),
        ),
        (
            "table.melt",
            json!({"id_columns": ["id"], "value_columns": ["name", "date"],
                   "type_policy": "string"}),
        ),
    ];
    for (op, config) in casi {
        esegui(op, Wide, &config).unwrap_or_else(|errore| panic!("{op} {config}: {errore}"));
    }
}

/// Un parametro facoltativo scritto `null` non vale «assente»: si rifiuta
/// dalla config, altrimenti sfuggirebbe alle regole sui parametri scritti.
#[test]
fn un_parametro_facoltativo_null_si_rifiuta() {
    use Fixture::{Set, Wide};
    let casi = [
        ("table.concat", Set, json!({"ignore_index": null})),
        ("table.sample", Wide, json!({"n": null})),
        (
            "table.md5_hash",
            Wide,
            json!({"columns": ["name"], "null_literal": null}),
        ),
        (
            "table.date_format",
            Wide,
            json!({"column": "date", "input_format": "%Y-%m-%d", "output_column": "d",
                   "invalid": null}),
        ),
        (
            "table.concat_columns",
            Wide,
            json!({"columns": ["name"], "separator": null}),
        ),
        (
            "table.formula",
            Wide,
            json!({"new_column": "f", "formula": "value / 2", "on_division_by_zero": null}),
        ),
        (
            "table.expression",
            Wide,
            json!({"output_column": "e", "on_division_by_zero": null,
                   "expression": {"kind": "column", "name": "value"}}),
        ),
        (
            "table.aggregate",
            Wide,
            json!({"group_by": ["name"], "aggregations": [{"column": "value",
                                                           "function": "sum", "ddof": null}]}),
        ),
        (
            "table.type_cast",
            Wide,
            json!({"column": "id", "target_type": "int", "errors": null}),
        ),
        ("table.reorder_columns", Wide, json!({"alphabetical": null})),
    ];
    for (op, fixture, config) in casi {
        match esegui(op, fixture, &config) {
            Err(errore) if e_piano(&errore) && errore.to_string().contains("null non ammesso") => {}
            altro => panic!("{op} {config}: atteso `null non ammesso`, avuto {altro:?}"),
        }
    }
}
