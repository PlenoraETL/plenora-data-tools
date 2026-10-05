//! Censimento dei parametri delle config tabellari: ogni campo di ogni
//! config, letto da serde (un campo sconosciuto fa elencare quelli
//! ammessi), ha una voce qui con la sua regola sui parametri senza effetto.
//!
//! Un campo nuovo fa fallire il test finche' non entra nel censimento: chi lo
//! aggiunge decide se ha effetto con ogni combinazione, o in quali
//! combinazioni non ne ha e con quale regola (in analisi e nel kernel,
//! `verifica_*`) si rifiuta (docs/runner.md, «Parametri ignorati: censimento»).

use std::collections::{BTreeMap, BTreeSet};

use plenora_core::catalog::{Family, CATALOG};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_kernels_table::analyze::analyze_table_contract;
use plenora_kernels_table::Limits;
use serde_json::json;

/// Campi elencati da serde per la config di `op`: la config
/// `{"zz_campo_sconosciuto": 0}` si rifiuta con `expected one of ...`.
fn campi_serde(op: &str) -> BTreeSet<String> {
    let schema = std::sync::Arc::new(plenora_core::arrow::schema::Schema::empty());
    let ingressi: Vec<DataContract> = (0..2)
        .map(|_| DataContract::tabular(schema.clone()))
        .collect();
    let arieta = match plenora_core::catalog::find_operation(op).map(|d| d.arity) {
        Some(plenora_core::catalog::Arity::Unary) => 1,
        _ => 2,
    };
    let errore = analyze_table_contract(
        op,
        &ingressi[..arieta],
        &json!({"zz_campo_sconosciuto": 0}),
        &mut FieldAllocator::default(),
        &Limits::default(),
    )
    .expect_err("un campo sconosciuto si rifiuta")
    .to_string();
    let Some((_, elenco)) = errore.split_once(", expected ") else {
        // `there are no fields`: una config senza campi.
        assert!(errore.contains("there are no fields"), "{op}: {errore}");
        return BTreeSet::new();
    };
    elenco
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Il censimento: per operazione, i campi e la regola che rifiuta le
/// combinazioni senza effetto (`-` quando il campo ha effetto con ogni
/// combinazione ammessa).
#[allow(clippy::too_many_lines)] // Una voce per operazione.
fn censimento() -> BTreeMap<&'static str, Vec<(&'static str, &'static str)>> {
    const NULLI: &str = "limite dichiarato: senza effetto su colonne non nullable";
    let voci: &[(&str, &[(&str, &str)])] = &[
        ("table.add_row_number", &[
            ("output_column", "-"),
            ("start", "-"),
            ("partition_column", "-"),
            ("order_column", "rifiutato se scritto (ordinamento non supportato)"),
            ("ascending", "verifica_ascending: senza order_column"),
        ]),
        ("table.aggregate", &[
            ("group_by", "-"),
            ("aggregations", "Aggregate::nomi_uscita (nomi ripetuti o uguali a una chiave); Aggregation::verifica_parametri"),
        ]),
        ("table.align_schema", &[
            ("columns", "dipende dallo schema: default su una colonna che esiste accettato"),
            ("keep_extra", "dipende dallo schema: accettato; null rifiutato"),
        ]),
        ("table.anti_join", &[("left_keys", "-"), ("right_keys", "-")]),
        ("table.asof_join", &[
            ("left_on", "-"), ("right_on", "-"), ("left_by", "-"), ("right_by", "-"),
            ("direction", "-"),
            ("tolerance", "AsOfJoin::verifica_parametri: 0 con allow_exact=false non abbina mai"),
            ("allow_exact", "AsOfJoin::verifica_parametri"),
        ]),
        ("table.assert_cardinality", &[
            ("exact_rows", "AssertCardinality::verifica_parametri: esclude min_rows e max_rows"),
            ("min_rows", "AssertCardinality::verifica_parametri: 0 non vincola"),
            ("max_rows", "-"),
        ]),
        ("table.assert_foreign_key", &[("left_keys", "-"), ("right_keys", "-"), ("allow_null", NULLI)]),
        ("table.assert_metadata", &[("expected", "vuoto: asserzione vacua"), ("allow_extra", "-")]),
        ("table.assert_not_null", &[("columns", "vuoto: asserzione vacua")]),
        ("table.assert_range", &[
            ("column", "-"),
            ("min", "AssertRange::verifica_parametri: almeno un estremo"),
            ("max", "AssertRange::verifica_parametri: almeno un estremo"),
            ("inclusive_min", "AssertRange::verifica_parametri: senza min"),
            ("inclusive_max", "AssertRange::verifica_parametri: senza max"),
            ("allow_null", NULLI),
        ]),
        ("table.assert_regex", &[("column", "-"), ("pattern", "-"), ("allow_null", NULLI)]),
        ("table.assert_schema", &[("fields", "vuoto: asserzione vacua"), ("allow_extra", "-"), ("ordered", "-")]),
        ("table.assert_unique", &[("columns", "vuoto: asserzione vacua"), ("nulls_equal", NULLI)]),
        ("table.bin", &[("column", "-"), ("bins", "-"), ("labels", "-"), ("output_column", "-")]),
        ("table.coalesce", &[("columns", "-"), ("output_column", "-")]),
        ("table.concat", &[("ignore_index", "Concat::verifica_parametri: senza effetto con ogni valore")]),
        ("table.concat_by_name", &[("strict", "-")]),
        ("table.concat_columns", &[
            ("columns", "-"),
            ("output_column", "-"),
            ("separator", "ConcatColumns::verifica_parametri: una sola colonna"),
            ("skip_null", "-"),
        ]),
        ("table.conditional", &[
            ("column", "-"),
            ("conditions", "filtering::verifica_valore: value con isnull/notnull"),
            ("default_value", "-"),
            ("output_column", "-"),
        ]),
        ("table.date_add", &[
            ("column", "-"), ("input_format", "-"), ("output_format", "-"),
            ("amount", "dates::verifica_amount"),
            ("unit", "limite dichiarato: senza effetto con amount 0"),
            ("output_column", "-"),
            ("invalid", "dates::verifica_politiche: senza effetto con ogni valore"),
        ]),
        ("table.date_diff", &[
            ("start_column", "-"), ("end_column", "-"), ("input_format", "-"), ("unit", "-"),
            ("output_column", "-"),
            ("invalid", "dates::verifica_politiche: senza effetto con ogni valore"),
        ]),
        ("table.date_extract", &[
            ("column", "-"),
            ("parts", "DateExtract::verifica_parti: vuoto o ripetute"),
            ("prefix", "-"),
            ("date_format", "-"),
            ("invalid", "dates::verifica_politiche: senza effetto con ogni valore"),
        ]),
        ("table.date_format", &[
            ("column", "-"), ("input_format", "-"), ("output_format", "-"), ("output_column", "-"),
            ("invalid", "dates::verifica_politiche: senza effetto con ogni valore"),
        ]),
        ("table.dedup_advanced", &[
            ("subset", "-"), ("keep", "-"), ("order_column", "-"),
            ("ascending", "verifica_ascending: senza order_column"),
        ]),
        ("table.distinct", &[("subset", "-"), ("keep", "-")]),
        ("table.drop_columns", &[("columns", "DropColumns::verifica_parametri: vuoto o ripetuto; assente accettato (schema)")]),
        ("table.explode", &[("column", "-"), ("empty_policy", "drop rifiutato"), ("output_column", "-")]),
        ("table.expression", &[
            ("output_column", "-"),
            ("expression", "campi sconosciuti dei nodi rifiutati; divisore letterale zero"),
            ("output_type", "-"),
            ("on_division_by_zero", "ExpressionTransform::verifica_parametri: senza divisioni"),
        ]),
        ("table.fill_na", &[
            ("column", "-"), ("method", "-"),
            ("value", "FillNa::verifica_parametri: con ffill/bfill; senza value vale null (limite dichiarato)"),
        ]),
        ("table.filter", &[
            ("column", "-"), ("operator", "-"),
            ("value", "filtering::verifica_valore: con isnull/notnull"),
        ]),
        ("table.flatten_json", &[("column", "-"), ("max_level", "-"), ("output_columns", "-"), ("prefix", "-")]),
        ("table.formula", &[
            ("new_column", "-"), ("formula", "divisore letterale zero"),
            ("on_division_by_zero", "formula::validate: senza /"),
        ]),
        ("table.fuzzy_join", &[
            ("left_key", "-"), ("right_key", "-"), ("metric", "-"), ("threshold", "-"),
            ("how", "-"), ("blocking", "-"), ("blocking_param", "-"),
            ("case_sensitive", "-"), ("max_candidates", "-"), ("score_column", "-"),
        ]),
        ("table.hmac_sha256", &[("columns", "-"), ("key_env", "-"), ("output_column", "-"), ("null_policy", NULLI)]),
        ("table.join", &[("left_keys", "-"), ("right_keys", "-"), ("how", "-")]),
        ("table.limit", &[("n", "-"), ("offset", "Limit::verifica_parametri: con n 0")]),
        ("table.lookup", &[("column", "-"), ("mapping", "-"), ("default", "-"), ("output_column", "-")]),
        ("table.mask_data", &[
            ("maskings", "Masking::verifica_parametri; MaskData::verifica_colonne: colonna ripetuta senza overwrite"),
            ("overwrite", "-"),
        ]),
        ("table.md5_hash", &[
            ("columns", "-"), ("output_column", "-"), ("normalize", "-"),
            ("null_policy", NULLI),
            ("null_literal", "security::verifica_null_literal: senza null_policy=literal"),
        ]),
        ("table.melt", &[
            ("id_columns", "-"), ("value_columns", "-"), ("var_name", "-"), ("value_name", "-"),
            ("type_policy", "dipende dallo schema: accettato; null rifiutato"),
        ]),
        ("table.pivot", &[
            ("index_col", "Pivot::verifica_mapping: voce vuota"),
            ("pivot_col", "-"), ("value_col", "-"), ("aggr_func", "-"), ("mapping", "-"),
        ]),
        ("table.reconcile", &[("left_keys", "-"), ("right_keys", "-"), ("nulls_equal", NULLI)]),
        ("table.rename", &[("renames", "Rename::verifica_parametri: vuoto, su se stessa, ripetuta; assente accettato (schema)")]),
        ("table.reorder_columns", &[
            ("columns", "ReorderColumns::verifica_parametri: vuoto senza alphabetical"),
            ("alphabetical", "dipende dallo schema: accettato; null rifiutato"),
            ("sort_alphabetical", "alias di alphabetical"),
        ]),
        ("table.replace", &[("column", "-"), ("old_value", "-"), ("new_value", "-"), ("regex", "-")]),
        ("table.rolling_window", &[
            ("column", "-"), ("function", "-"), ("window", "-"), ("min_periods", "-"),
            ("group_by", "-"), ("order_column", "-"), ("output_column", "-"),
            ("ddof", "RollingWindow: fuori da stddev"),
        ]),
        ("table.sample", &[
            ("n", "Sample::verifica_parametri: con fraction"),
            ("fraction", "-"),
            ("random_state", "Sample::verifica_parametri: campione sempre vuoto"),
            ("stratify_column", "-"),
        ]),
        ("table.select_columns", &[("columns", "-")]),
        ("table.semi_join", &[("left_keys", "-"), ("right_keys", "-")]),
        ("table.sha256_hash", &[
            ("columns", "vuoto: impronta vacua"), ("output_column", "-"), ("normalize", "-"),
            ("null_policy", NULLI),
            ("null_literal", "security::verifica_null_literal: senza null_policy=literal"),
        ]),
        ("table.sort", &[("columns", "-"), ("ascending", "-")]),
        ("table.split_column", &[
            ("column", "-"),
            ("delimiter", "SplitColumn::verifica_parametri: una sola colonna d'uscita"),
            ("new_columns", "-"),
            ("max_splits", "SplitColumn::verifica_parametri: non riduce le parti"),
        ]),
        ("table.stable_fingerprint", &[("columns", "-"), ("output_column", "-"), ("algorithm", "-")]),
        ("table.statistics", &[
            ("column", "-"), ("group_by", "-"),
            ("stats", "Statistics::verifica_parametri: vuoto o ripetute"),
            ("output_prefix", "-"),
        ]),
        ("table.string_extract", &[
            ("column", "-"), ("pattern", "-"),
            ("output_column", "verifica_gruppi_con_nome"),
            ("extract_all", "verifica_gruppi_con_nome"),
        ]),
        ("table.string_length", &[("column", "-"), ("output_column", "-")]),
        ("table.string_pad", &[
            ("column", "-"),
            ("width", "StringPad::verifica_parametri: 0 non allunga"),
            ("side", "-"), ("fill_char", "-"), ("output_column", "-"),
        ]),
        ("table.table_diff", &[
            ("left_keys", "-"), ("right_keys", "-"), ("compare_columns", "-"), ("include_unchanged", "-"),
            ("separator", "TableDiff::verifica_separatore: una sola colonna in compare_columns"),
        ]),
        ("table.text_normalize", &[("columns", "-"), ("operations", "-"), ("overwrite", "-")]),
        ("table.timezone_convert", &[
            ("column", "-"), ("input_format", "-"), ("output_format", "-"),
            ("source_timezone", "-"), ("target_timezone", "-"), ("output_column", "-"),
            ("invalid", "dates::verifica_politiche: senza effetto con ogni valore"),
            ("ambiguous", "dates::verifica_politiche: senza effetto con ogni valore"),
        ]),
        ("table.top_n", &[("columns", "-"), ("n", "TopN::verifica_parametri: 0 da' sempre vuoto"), ("descending", "-")]),
        ("table.transpose", &[
            ("id_column", "-"), ("output_columns", "-"),
            ("type_policy", "dipende dallo schema: accettato; null rifiutato"),
        ]),
        ("table.type_cast", &[
            ("column", "-"), ("target_type", "-"),
            ("date_format", "TypeCast::verifica_parametri: target senza date"),
            ("errors", "TypeCast::verifica_parametri: target che non fallisce"),
            ("precision", "TypeCast::verifica_parametri: solo decimal128"),
            ("scale", "TypeCast::verifica_parametri: solo decimal128"),
            ("timezone", "TypeCast::verifica_parametri: solo timestamp_millis"),
        ]),
        ("table.unnest", &[("column", "-"), ("prefix", "-"), ("drop_source", "-")]),
        ("table.uuid_generator", &[("output_column", "-")]),
        ("table.validate_rules", &[("rules", "-"), ("output_mode", "-")]),
        ("table.window_function", &[
            ("column", "-"), ("function", "-"), ("group_by", "-"), ("order_column", "-"),
            ("output_column", "-"), ("buckets", "rifiutato fuori da ntile"),
            ("offset", "WindowFunction::verifica_offset: fuori da lag/lead"),
        ]),
    ];
    voci.iter()
        .map(|(op, campi)| (*op, campi.to_vec()))
        .collect()
}

#[test]
fn ogni_campo_di_ogni_config_tabellare_e_censito() {
    let censimento = censimento();
    let mut mancanti = Vec::new();
    for operazione in CATALOG.iter().filter(|op| op.family == Family::Table) {
        let serde = campi_serde(operazione.id);
        let censiti: BTreeSet<String> = censimento
            .get(operazione.id)
            .map(|campi| campi.iter().map(|(campo, _)| (*campo).to_owned()).collect())
            .unwrap_or_default();
        if serde != censiti {
            mancanti.push(format!(
                "{}: serde {serde:?}, censiti {censiti:?}",
                operazione.id
            ));
        }
    }
    assert!(mancanti.is_empty(), "{}", mancanti.join("\n"));
}

/// `true` se la config `{campo: null}` si rifiuta per il `null` (la
/// deserializzazione visita i campi prima di cercare quelli mancanti).
fn rifiuta_null(op: &str, config: &serde_json::Value) -> Result<(), String> {
    let schema = std::sync::Arc::new(plenora_core::arrow::schema::Schema::empty());
    let ingressi: Vec<DataContract> = (0..2)
        .map(|_| DataContract::tabular(schema.clone()))
        .collect();
    let arieta = match plenora_core::catalog::find_operation(op).map(|d| d.arity) {
        Some(plenora_core::catalog::Arity::Unary) => 1,
        _ => 2,
    };
    // `transpose` si rifiuta come `Unsupported` prima della config: la sua
    // config si legge direttamente.
    if op == "table.transpose" {
        return match serde_json::from_value::<plenora_kernels_table::reshape::Transpose>(
            config.clone(),
        ) {
            Err(errore) if !errore.to_string().contains("missing field") => Ok(()),
            altro => Err(format!("{:?}", altro.map(|_| ()))),
        };
    }
    match analyze_table_contract(
        op,
        &ingressi[..arieta],
        config,
        &mut FieldAllocator::default(),
        &Limits::default(),
    ) {
        // Un errore di deserializzazione che non e' il campo mancante: il
        // `null` non si e' letto (anche un enum senza tag lo rifiuta cosi').
        Err(errore)
            if errore.to_string().contains("config non valida")
                && !errore.to_string().contains("missing field") =>
        {
            Ok(())
        }
        altro => Err(format!("{:?}", altro.map(|_| ()))),
    }
}

/// Un `null` esplicito non vale «assente» per nessun campo di nessuna config
/// tabellare, di primo livello o annidato, salvo quelli dove la scheda lo
/// ammette e ha un significato proprio (elenco qui sotto, con il motivo):
/// senza, un parametro scritto `null` sfuggirebbe alle regole sui parametri
/// scritti.
#[test]
#[allow(clippy::too_many_lines)] // Elenco dei campi ammessi e dei casi annidati.
fn nessun_campo_accetta_null_salvo_quelli_dichiarati() {
    let ammessi: &[(&str, &str, &str)] = &[
        (
            "table.filter",
            "value",
            "`null` scritto e' il testo vuoto, e conta come scritto",
        ),
        (
            "table.conditional",
            "default_value",
            "`null` e' un valore d'uscita",
        ),
        (
            "table.fill_na",
            "value",
            "`null` scritto conta come scritto (valore_scritto)",
        ),
    ];
    let mut difetti = Vec::new();
    for operazione in CATALOG.iter().filter(|op| op.family == Family::Table) {
        for campo in campi_serde(operazione.id) {
            if ammessi
                .iter()
                .any(|(op, nome, _)| *op == operazione.id && *nome == campo)
            {
                continue;
            }
            if let Err(avuto) = rifiuta_null(operazione.id, &json!({ campo.clone(): null })) {
                difetti.push(format!("{} `{campo}`: {avuto}", operazione.id));
            }
        }
    }
    // Campi annidati, uno per struttura.
    let annidati: &[(&str, serde_json::Value)] = &[
        (
            "table.aggregate",
            json!({"group_by": ["a"], "aggregations": [{"column": "a", "alias": null}]}),
        ),
        (
            "table.aggregate",
            json!({"group_by": ["a"], "aggregations": [{"column": "a", "separator": null}]}),
        ),
        (
            "table.aggregate",
            json!({"group_by": ["a"], "aggregations": [{"column": "a", "quantile": null}]}),
        ),
        (
            "table.mask_data",
            json!({"maskings": [{"column": "a", "chars_start": null}]}),
        ),
        (
            "table.mask_data",
            json!({"maskings": [{"column": "a", "mask_char": null}]}),
        ),
        (
            "table.conditional",
            json!({"column": "a", "conditions": [{"operator": null}]}),
        ),
        // Ammessi e dichiarati nella scheda: `result` di `conditional`
        // (`null` e' un valore d'uscita), `default` di `align_schema`
        // (`null`: colonna di null).
        (
            "table.validate_rules",
            json!({"rules": [{"name": "r", "operator": "gt", "severity": null}]}),
        ),
        (
            "table.validate_rules",
            json!({"rules": [{"name": "r", "operator": "gt", "column": null}]}),
        ),
        (
            "table.validate_rules",
            json!({"rules": [{"name": "r", "operator": "isnull", "value": null}]}),
        ),
        (
            "table.assert_schema",
            json!({"fields": [{"name": "a", "data_type": "utf8", "nullable": null}]}),
        ),
        (
            "table.rename",
            json!({"renames": [{"old_name": null, "new_name": "b"}]}),
        ),
    ];
    for (op, config) in annidati {
        if let Err(avuto) = rifiuta_null(op, config) {
            difetti.push(format!("{op} {config}: {avuto}"));
        }
    }
    assert!(difetti.is_empty(), "{}", difetti.join("\n"));
}
