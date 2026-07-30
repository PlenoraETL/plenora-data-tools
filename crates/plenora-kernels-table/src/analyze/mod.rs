//! Inferenza a secco del `DataContract` di output per le 71 operazioni
//! `table.*` del catalogo (Fase 2A-2, Architetture.md par. 4.3 e 6.1, ADR 5).
//!
//! [`analyze_table_contract`] deserializza la config tipizzata dell'operazione
//! (fail-closed: config non valida -> errore `InvalidPlan` puntuale), replica le
//! validazioni statiche del kernel (esistenza colonne, vincoli di tipo,
//! parametri) e inferisce il contratto di output: schema Arrow, propagazione
//! della colonna geometrica (D16) e proprietà (`sorted_by`, `row_count`) con
//! provenienza e scope (D25).
//!
//! Regole di propagazione (D16): una rinomina preserva il `FieldId`, una
//! colonna derivata ne riceve uno nuovo dal [`FieldAllocator`]. La colonna
//! geometrica sopravvive solo se la colonna e' propagata inalterata (stesso
//! tipo, valori passthrough): una sovrascrittura in place (semantica
//! `replace_or_append`) produce una colonna derivata senza metadati
//! `geoarrow.wkb`, quindi il contratto diventa tabellare.
//!
//! Proprieta': v1 deliberatamente conservativa. `sorted_by` e' `Proven` in
//! output solo per le op blocking che riordinano l'intero stream (`sort`,
//! `dedup_advanced`/`rolling_window`/`window_function` con `order_column`);
//! le op che preservano l'ordine delle righe propagano la proprieta' di
//! input inalterata; tutte le altre la eliminano. `row_count` e' propagato
//! solo quando il numero di righe e' esatto (op 1:1, `concat`, `cross_join`,
//! `melt`, `asof_join`, `reconcile`), mai inventato.
//!
//! Op con schema dipendente dai dati (non inferibile a secco):
//! `table.pivot`, `table.transpose` e `table.flatten_json` senza
//! `output_columns` esplicito falliscono con `Unsupported` esplicito —
//! meglio fallire in validazione che indovinare uno schema sbagliato.
//!
//! Lineage dei metadata Arrow (R2.4, plenora-contracts v2.0-rc10 par. 2): i
//! metadata dello schema di input attraversano sempre lo schema di output;
//! per le op a due sorgenti (join e varianti, `union_distinct`, `table_diff`)
//! vale la merge-policy — chiave su una sola sorgente o con valore identico
//! copiata, valori diversi -> errore `InvalidPlan` (mai precedenza implicita).
//! I metadata di campo seguono le classi R2.4: identity/type-preserving ->
//! copiati dal campo sorgente, colonne derivate -> non ereditati. Le chiavi
//! canoniche `plenora.*` non sono mai emesse qui (emissione centralizzata
//! in `executor.rs::canonical_output_schema`): il compito e' solo non
//! perdere quelle esistenti.

// Firma uniforme degli analyzer per-op: il dispatch passa l'allocatore di
// FieldId a ogni operazione, anche a quelle (assert, gate, join) che non
// derivano colonne e quindi non lo usano.
#![allow(clippy::needless_pass_by_ref_mut)]

use plenora_core::catalog::{find_operation, Arity, Family};
use plenora_core::contract::{DataContract, FieldAllocator};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

pub(crate) mod analysis;
pub(crate) mod cleansing;
pub(crate) mod columns;
pub(crate) mod dates;
pub(crate) mod filter;
pub(crate) mod formula;
pub(crate) mod generators;
pub(crate) mod helpers;
pub(crate) mod joins;
pub(crate) mod quality;
pub(crate) mod reshape;
pub(crate) mod sort_agg;
pub(crate) mod strings;

use self::analysis::{
    analyze_bin, analyze_flatten_json, analyze_lookup, analyze_sample, analyze_statistics,
};
use self::cleansing::{analyze_fill_na, analyze_replace, analyze_type_cast};
use self::columns::{
    analyze_align_schema, analyze_concat_columns, analyze_drop_columns, analyze_rename,
    analyze_reorder_columns, analyze_select_columns, analyze_split_column,
};
use self::dates::{
    analyze_date_add, analyze_date_diff, analyze_date_format, analyze_timezone_convert,
};
use self::filter::{analyze_conditional, analyze_filter};
use self::formula::{analyze_expression, analyze_formula};
use self::generators::{
    analyze_add_row_number, analyze_date_extract, analyze_limit, analyze_uuid_generator,
};
use self::joins::{
    analyze_asof_join, analyze_concat, analyze_concat_by_name, analyze_cross_join,
    analyze_fuzzy_join, analyze_join, analyze_membership_join, analyze_set_operation, SetOp,
};
use self::quality::{
    analyze_assert_cardinality, analyze_assert_foreign_key, analyze_assert_metadata,
    analyze_assert_not_null, analyze_assert_range, analyze_assert_regex, analyze_assert_schema,
    analyze_assert_unique, analyze_coalesce, analyze_reconcile, analyze_validate_rules,
};
use self::reshape::{
    analyze_explode, analyze_melt, analyze_pivot, analyze_table_diff, analyze_transpose,
    analyze_unnest,
};
use self::sort_agg::{
    analyze_aggregate, analyze_dedup_advanced, analyze_distinct, analyze_rolling_window,
    analyze_sort, analyze_top_n, analyze_window_function,
};
use self::strings::{
    analyze_hmac_sha256, analyze_mask_data, analyze_md5_hash, analyze_sha256_hash,
    analyze_stable_fingerprint, analyze_string_extract, analyze_string_length, analyze_string_pad,
    analyze_text_normalize,
};

/// Inferisce il `DataContract` di output di un'operazione `table.*` a secco
/// (Fase 1 `validate`, Architetture.md par. 6.1 passo 6).
///
/// `op` accetta id canonici e alias legacy (risolti via catalogo);
/// `inputs` deve rispettare l'arieta' dichiarata dal catalogo (unaria,
/// binaria ordinata, N-aria per `table.concat`).
///
/// # Errors
///
/// - `Unsupported`: operazione sconosciuta, non `table.*`, oppure schema di
///   output non inferibile a secco (dipende dai dati);
/// - `InvalidPlan`: config non valida, arieta' errata, colonne mancanti,
///   vincoli di tipo o parametri violati, collisioni di naming;
/// - `Schema`: il contratto inferito viola le regole strutturali v1 (D16).
// Un braccio per operazione: la lunghezza e' intrinseca al dispatch su 71 op.
#[allow(clippy::too_many_lines)]
pub fn analyze_table_contract(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let descriptor = find_operation(op)
        .ok_or_else(|| PlenoraError::Unsupported(format!("{op}: operazione sconosciuta")))?;
    if descriptor.family != Family::Table {
        return Err(PlenoraError::Unsupported(format!(
            "{op}: non e' un'operazione table.*"
        )));
    }
    let id = descriptor.id;
    match descriptor.arity {
        Arity::Unary if inputs.len() != 1 => {
            return Err(PlenoraError::InvalidPlan(format!(
                "{id}: atteso 1 input, ricevuti {}",
                inputs.len()
            )));
        }
        Arity::BinaryOrdered if inputs.len() != 2 => {
            return Err(PlenoraError::InvalidPlan(format!(
                "{id}: attesi 2 input (left, right), ricevuti {}",
                inputs.len()
            )));
        }
        Arity::NAry if inputs.len() < 2 => {
            return Err(PlenoraError::InvalidPlan(format!(
                "{id}: attesi almeno 2 input, ricevuti {}",
                inputs.len()
            )));
        }
        _ => {}
    }
    for input in inputs {
        for geometry in &input.geometries {
            fields.observe(geometry.field_id);
        }
        if let Some(active) = input.active_geometry {
            fields.observe(active);
        }
        if let Some(sorted) = &input.properties.sorted_by {
            if let Some(keys) = sorted.confidence.value() {
                for key in keys {
                    fields.observe(*key);
                }
            }
        }
    }
    match id {
        "table.add_row_number" => analyze_add_row_number(id, inputs, config, fields),
        "table.aggregate" => analyze_aggregate(id, inputs, config, fields),
        "table.bin" => analyze_bin(id, inputs, config, fields),
        "table.concat" => analyze_concat(id, inputs, config, fields),
        "table.concat_columns" => analyze_concat_columns(id, inputs, config, fields),
        "table.conditional" => analyze_conditional(id, inputs, config, fields),
        "table.cross_join" => analyze_cross_join(id, inputs, config, fields),
        "table.date_extract" => analyze_date_extract(id, inputs, config, fields),
        "table.dedup_advanced" => analyze_dedup_advanced(id, inputs, config, fields),
        "table.distinct" => analyze_distinct(id, inputs, config, fields),
        "table.drop_columns" => analyze_drop_columns(id, inputs, config, fields),
        "table.fill_na" => analyze_fill_na(id, inputs, config, fields),
        "table.filter" => analyze_filter(id, inputs, config, fields),
        "table.flatten_json" => analyze_flatten_json(id, inputs, config, fields),
        "table.formula" => analyze_formula(id, inputs, config, fields),
        "table.join" => analyze_join(id, inputs, config, fields),
        "table.lookup" => analyze_lookup(id, inputs, config, fields),
        "table.melt" => analyze_melt(id, inputs, config, fields),
        "table.pivot" => analyze_pivot(id, inputs, config, fields),
        "table.rename" => analyze_rename(id, inputs, config, fields),
        "table.reorder_columns" => analyze_reorder_columns(id, inputs, config, fields),
        "table.replace" => analyze_replace(id, inputs, config, fields),
        "table.sample" => analyze_sample(id, inputs, config, fields),
        "table.sort" => analyze_sort(id, inputs, config, fields),
        "table.split_column" => analyze_split_column(id, inputs, config, fields),
        "table.statistics" => analyze_statistics(id, inputs, config, fields),
        "table.string_extract" => analyze_string_extract(id, inputs, config, fields),
        "table.string_length" => analyze_string_length(id, inputs, config, fields),
        "table.string_pad" => analyze_string_pad(id, inputs, config, fields),
        "table.table_diff" => analyze_table_diff(id, inputs, config, fields),
        "table.text_normalize" => analyze_text_normalize(id, inputs, config, fields),
        "table.transpose" => analyze_transpose(id, inputs, config, fields),
        "table.type_cast" => analyze_type_cast(id, inputs, config, fields),
        "table.uuid_generator" => analyze_uuid_generator(id, inputs, config, fields),
        "table.window_function" => analyze_window_function(id, inputs, config, fields),
        "table.mask_data" => analyze_mask_data(id, inputs, config, fields),
        "table.md5_hash" => analyze_md5_hash(id, inputs, config, fields),
        "table.anti_join" | "table.semi_join" => {
            analyze_membership_join(id, inputs, config, fields)
        }
        "table.asof_join" => analyze_asof_join(id, inputs, config, fields),
        "table.assert_not_null" => analyze_assert_not_null(id, inputs, config, fields),
        "table.assert_range" => analyze_assert_range(id, inputs, config, fields),
        "table.assert_regex" => analyze_assert_regex(id, inputs, config, fields),
        "table.assert_schema" => analyze_assert_schema(id, inputs, config, fields),
        "table.assert_unique" => analyze_assert_unique(id, inputs, config, fields),
        "table.coalesce" => analyze_coalesce(id, inputs, config, fields),
        "table.date_add" => analyze_date_add(id, inputs, config, fields),
        "table.date_diff" => analyze_date_diff(id, inputs, config, fields),
        "table.date_format" => analyze_date_format(id, inputs, config, fields),
        "table.except" => analyze_set_operation(id, inputs, config, fields, SetOp::Except),
        "table.explode" => analyze_explode(id, inputs, config, fields),
        "table.intersect" => analyze_set_operation(id, inputs, config, fields, SetOp::Intersect),
        "table.rolling_window" => analyze_rolling_window(id, inputs, config, fields),
        "table.sha256_hash" => analyze_sha256_hash(id, inputs, config, fields),
        "table.timezone_convert" => analyze_timezone_convert(id, inputs, config, fields),
        "table.union_distinct" => {
            analyze_set_operation(id, inputs, config, fields, SetOp::UnionDistinct)
        }
        "table.unnest" => analyze_unnest(id, inputs, config, fields),
        "table.expression" => analyze_expression(id, inputs, config, fields),
        "table.assert_cardinality" => analyze_assert_cardinality(id, inputs, config, fields),
        "table.assert_metadata" => analyze_assert_metadata(id, inputs, config, fields),
        "table.assert_foreign_key" => analyze_assert_foreign_key(id, inputs, config, fields),
        "table.reconcile" => analyze_reconcile(id, inputs, config, fields),
        "table.select_columns" => analyze_select_columns(id, inputs, config, fields),
        "table.limit" => analyze_limit(id, inputs, config, fields),
        "table.top_n" => analyze_top_n(id, inputs, config, fields),
        "table.stable_fingerprint" => analyze_stable_fingerprint(id, inputs, config, fields),
        "table.align_schema" => analyze_align_schema(id, inputs, config, fields),
        "table.concat_by_name" => analyze_concat_by_name(id, inputs, config, fields),
        "table.validate_rules" => analyze_validate_rules(id, inputs, config, fields),
        "table.hmac_sha256" => analyze_hmac_sha256(id, inputs, config, fields),
        "table.fuzzy_join" => analyze_fuzzy_join(id, inputs, config, fields),
        _ => Err(PlenoraError::Unsupported(format!(
            "{id}: analyze_contract non disponibile"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use plenora_core::catalog::{Family, CATALOG};
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use serde_json::json;

    use std::collections::HashMap;

    use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
    use plenora_core::contract::{
        ContractCrs, ContractProperties, ContractProperty, DataContract, FieldAllocator, FieldId,
        GeometryColumnContract, PropertyConfidence, PropertyScope,
    };
    use plenora_core::{PlenoraError, Result};
    use serde_json::Value;

    use super::analyze_table_contract;
    use crate::{
        aggregation, analysis, cleansing, columns, dates, expressions, filtering, formula,
        governance, joins, quality, reshape, security, setops, strings, utility, Limits,
    };

    fn projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            serde_json::json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    fn base_fields() -> Vec<Field> {
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("value", DataType::Float64, true),
            Field::new("flag", DataType::Boolean, true),
            Field::new("d", DataType::Date32, true),
            Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new("dc", DataType::Decimal128(10, 2), true),
            Field::new("u", DataType::UInt64, true),
            Field::new("lst", DataType::List(Arc::new(Field::new("item", DataType::Int64, true))), true),
            Field::new(
                "st",
                DataType::Struct(vec![
                    Field::new("a", DataType::Int64, true),
                    Field::new("b", DataType::Utf8, true),
                ]
                .into()),
                true,
            ),
            Field::new("geom", DataType::Binary, true),
        ]
    }

    fn geometry(field_id: u32, name: &str, nullable: bool) -> GeometryColumnContract {
        GeometryColumnContract {
            field_id: FieldId(field_id),
            name: name.to_owned(),
            crs: ContractCrs::Resolved(projected_crs()),
            dimensions: plenora_core::contract::GeometryDimensions::Xy,
            encoding: None,
            nullable,
            types: GeometryColumnContract::undeclared_types(),
        }
    }

    /// Contratto con geometria attiva `geom` (FieldId(7)) e nessuna proprieta'.
    fn geo_contract() -> DataContract {
        DataContract::new(
            Arc::new(Schema::new(base_fields())),
            vec![geometry(7, "geom", true)],
            Some(FieldId(7)),
            ContractProperties::default(),
        )
        .unwrap()
    }

    /// Come `geo_contract`, con `sorted_by = Proven([FieldId(0)], Stream)` e
    /// `row_count = Proven(100, Dataset)`.
    fn proven_contract() -> DataContract {
        DataContract::new(
            Arc::new(Schema::new(base_fields())),
            vec![geometry(7, "geom", true)],
            Some(FieldId(7)),
            ContractProperties {
                sorted_by: Some(ContractProperty::new(
                    PropertyConfidence::Proven(vec![FieldId(0)]),
                    PropertyScope::Stream,
                )),
                row_count: Some(ContractProperty::new(
                    PropertyConfidence::Proven(100),
                    PropertyScope::Dataset,
                )),
            },
        )
        .unwrap()
    }

    fn tabular_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(base_fields())))
    }

    /// Contratto right per le op binarie: chiave `rid` Int64, colonne
    /// condivise `name`/`value` e colonna propria `rname`.
    fn right_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("rid", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("value", DataType::Float64, true),
            Field::new("rname", DataType::Utf8, true),
        ])))
    }

    /// Coppia di contratti semplici per concat/setops (niente List/Struct).
    fn simple_pair() -> (DataContract, DataContract) {
        let make = |nullable: bool, rows: u64| {
            DataContract::new(
                Arc::new(Schema::new(vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("name", DataType::Utf8, nullable),
                ])),
                Vec::new(),
                None,
                ContractProperties {
                    sorted_by: None,
                    row_count: Some(ContractProperty::new(
                        PropertyConfidence::Proven(rows),
                        PropertyScope::Dataset,
                    )),
                },
            )
            .unwrap()
        };
        (make(false, 100), make(true, 50))
    }

    // Config posseduta per ergonomia dei test (json! inline).
    #[allow(clippy::needless_pass_by_value)]
    fn ok(op: &str, inputs: &[DataContract], config: Value) -> DataContract {
        analyze_table_contract(op, inputs, &config, &mut FieldAllocator::default())
            .unwrap_or_else(|error| panic!("{op} con config valida fallisce: {error}"))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn err(op: &str, inputs: &[DataContract], config: Value) -> PlenoraError {
        analyze_table_contract(op, inputs, &config, &mut FieldAllocator::default())
            .expect_err(&format!("{op} con config invalida deve fallire"))
    }

    fn assert_field(contract: &DataContract, name: &str, data_type: &DataType, nullable: bool) {
        let field = contract
            .schema
            .field_with_name(name)
            .unwrap_or_else(|_| panic!("colonna {name} assente dallo schema di output"));
        assert_eq!(
            field.data_type(),
            data_type,
            "tipo della colonna {name}"
        );
        assert_eq!(field.is_nullable(), nullable, "nullability di {name}");
    }

    fn proven_sorted_keys(contract: &DataContract) -> &Vec<FieldId> {
        let property = contract
            .properties
            .sorted_by
            .as_ref()
            .expect("sorted_by assente");
        assert!(property.is_proven(), "sorted_by non Proven");
        assert_eq!(property.scope, PropertyScope::Stream);
        property.confidence.proven_value().unwrap()
    }

    fn proven_rows(contract: &DataContract) -> u64 {
        let property = contract
            .properties
            .row_count
            .as_ref()
            .expect("row_count assente");
        *property.confidence.proven_value().expect("row_count non Proven")
    }

    // -- Completezza del dispatch -------------------------------------------

    #[test]
    fn passthrough_preserves_geometry_dimensions_and_encoding() {
        use plenora_core::contract::{GeometryDimensions, GeometryEncoding};

        // B1.3: le op tabellari sono passthrough byte-preserving — la
        // dimensionalita' (anche `Unknown`, R3.4) e l'encoding del contratto
        // di input attraversano invariati filtri e rinomine; MAI un xy
        // silenzioso.
        for dimensions in [
            GeometryDimensions::Xyz,
            GeometryDimensions::Xyzm,
            GeometryDimensions::Unknown,
        ] {
            let mut contract = geo_contract();
            contract.geometries[0].dimensions = dimensions;
            contract.geometries[0].encoding = Some(GeometryEncoding::Ewkb);

            let filtered = ok(
                "table.filter",
                &[contract.clone()],
                json!({"column": "id", "operator": ">", "value": 0}),
            );
            let geometry = filtered
                .active_geometry_column()
                .expect("geometria preservata da filter");
            assert_eq!(geometry.dimensions, dimensions, "filter: dimensions");
            assert_eq!(
                geometry.encoding,
                Some(GeometryEncoding::Ewkb),
                "filter: encoding"
            );

            let renamed = ok(
                "table.rename",
                &[contract],
                json!({"renames": [{"old_name": "geom", "new_name": "geometry"}]}),
            );
            let geometry = renamed
                .active_geometry_column()
                .expect("geometria preservata da rename");
            assert_eq!(geometry.name, "geometry");
            assert_eq!(geometry.dimensions, dimensions, "rename: dimensions");
            assert_eq!(
                geometry.encoding,
                Some(GeometryEncoding::Ewkb),
                "rename: encoding"
            );
        }
    }

    #[test]
    fn every_table_op_has_an_analysis_arm() {
        let table_ops: Vec<_> = CATALOG
            .iter()
            .filter(|op| op.family == Family::Table)
            .collect();
        assert_eq!(table_ops.len(), 71);
        for descriptor in table_ops {
            let inputs = vec![tabular_contract(), right_contract()];
            let result =
                analyze_table_contract(descriptor.id, &inputs, &json!({}), &mut FieldAllocator::default());
            if let Err(PlenoraError::Unsupported(message)) = &result {
                assert!(
                    !message.contains("analyze_contract non disponibile"),
                    "{} senza braccio di analisi",
                    descriptor.id
                );
            }
        }
    }

    #[test]
    fn unknown_and_geo_ops_are_unsupported() {
        let inputs = vec![tabular_contract()];
        assert!(matches!(
            analyze_table_contract("table.nonexistent", &inputs, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::Unsupported(_))
        ));
        assert!(matches!(
            analyze_table_contract("geo.buffer", &inputs, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::Unsupported(_))
        ));
    }

    #[test]
    fn arity_is_enforced() {
        let one = vec![tabular_contract()];
        let three = [tabular_contract(), tabular_contract(), tabular_contract()];
        assert!(matches!(
            analyze_table_contract("table.join", &one, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::InvalidPlan(_))
        ));
        assert!(matches!(
            analyze_table_contract("table.filter", &three[..2], &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::InvalidPlan(_))
        ));
        assert!(matches!(
            analyze_table_contract("table.concat", &one, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::InvalidPlan(_))
        ));
        // concat N-aria: 3 input ammessi.
        let (a, b) = simple_pair();
        let (_, c) = simple_pair();
        assert!(analyze_table_contract("table.concat", &[a, b, c], &json!({}), &mut FieldAllocator::default()).is_ok());
    }

    // -- utility / strings / security / dates --------------------------------

    #[test]
    fn add_row_number_appends_int64_and_preserves_row_count() {
        let output = ok("table.add_row_number", &[proven_contract()], json!({}));
        assert_field(&output, "row_number", &DataType::Int64, false);
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(proven_rows(&output), 100);
        assert!(output.properties.sorted_by.is_some());
        assert!(matches!(
            err("table.add_row_number", &[tabular_contract()], json!({"order_column": "id"})),
            PlenoraError::InvalidPlan(_)
        ));
    }

    #[test]
    fn date_extract_appends_one_int64_per_part() {
        let output = ok(
            "table.date_extract",
            &[tabular_contract()],
            json!({"column": "name", "parts": ["year", "week"]}),
        );
        assert_field(&output, "name_year", &DataType::Int64, true);
        assert_field(&output, "name_week", &DataType::Int64, true);
        assert!(err("table.date_extract", &[tabular_contract()], json!({"column": "missing"})).to_string().contains("missing"));
    }

    #[test]
    fn uuid_generator_appends_non_nullable_utf8() {
        let output = ok("table.uuid_generator", &[tabular_contract()], json!({}));
        assert_field(&output, "uuid", &DataType::Utf8, false);
        assert!(err("table.uuid_generator", &[tabular_contract()], json!({"output_column": " "})).to_string().contains("nome"));
    }

    #[test]
    fn string_ops_validate_utf8_and_produce_expected_types() {
        let padded = ok("table.string_pad", &[tabular_contract()], json!({"column": "name"}));
        assert_field(&padded, "name", &DataType::Utf8, true);
        assert!(err("table.string_pad", &[tabular_contract()], json!({"column": "value"})).to_string().contains("Utf8"));
        assert!(err("table.string_pad", &[tabular_contract()], json!({"column": "name", "fill_char": "ab"})).to_string().contains("fill_char"));

        let length = ok("table.string_length", &[tabular_contract()], json!({"column": "name"}));
        assert_field(&length, "name_length", &DataType::Int64, true);
        assert!(err("table.string_length", &[tabular_contract()], json!({"column": "flag"})).to_string().contains("Utf8"));

        let extracted = ok(
            "table.string_extract",
            &[tabular_contract()],
            json!({"column": "name", "pattern": "(?P<year>\\d{4})-(?P<month>\\d{2})"}),
        );
        assert_field(&extracted, "year", &DataType::Utf8, true);
        assert_field(&extracted, "month", &DataType::Utf8, true);
        let plain = ok(
            "table.string_extract",
            &[tabular_contract()],
            json!({"column": "name", "pattern": "\\d+"}),
        );
        assert_field(&plain, "name_extracted", &DataType::Utf8, true);
        assert!(err("table.string_extract", &[tabular_contract()], json!({"column": "name", "pattern": "("})).to_string().contains("regex"));

        let normalized = ok("table.text_normalize", &[tabular_contract()], json!({"columns": ["name"]}));
        assert_field(&normalized, "name", &DataType::Utf8, true);
        assert!(err("table.text_normalize", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));
    }

    #[test]
    fn hash_and_mask_ops_append_utf8() {
        let md5 = ok("table.md5_hash", &[tabular_contract()], json!({"columns": ["name"]}));
        assert_field(&md5, "md5_hash", &DataType::Utf8, false);
        assert!(err("table.md5_hash", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));

        let sha = ok("table.sha256_hash", &[tabular_contract()], json!({"columns": ["name"]}));
        assert_field(&sha, "sha256_hash", &DataType::Utf8, false);
        assert!(err("table.sha256_hash", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));

        let masked = ok(
            "table.mask_data",
            &[tabular_contract()],
            json!({"maskings": [{"column": "name"}]}),
        );
        assert_field(&masked, "name_masked", &DataType::Utf8, true);
        let overwritten = ok(
            "table.mask_data",
            &[tabular_contract()],
            json!({"maskings": [{"column": "name"}], "overwrite": true}),
        );
        assert!(overwritten.schema.field_with_name("name_masked").is_err());
        assert_field(&overwritten, "name", &DataType::Utf8, true);
        assert!(err("table.mask_data", &[tabular_contract()], json!({"maskings": []})).to_string().contains("maskings"));
    }

    #[test]
    fn v1_1_extensions_analyze_contracts() {
        // select_columns: proiezione nell'ordine dato, geometria propagata se
        // selezionata inalterata.
        let output = ok(
            "table.select_columns",
            &[geo_contract()],
            json!({"columns": ["id", "geom"]}),
        );
        assert_eq!(output.schema.fields().len(), 2);
        assert_eq!(output.schema.field(0).name(), "id");
        assert_eq!(output.schema.field(1).name(), "geom");
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        // Geometria non selezionata: contratto tabellare.
        let output = ok(
            "table.select_columns",
            &[geo_contract()],
            json!({"columns": ["name", "id"]}),
        );
        assert!(output.geometries.is_empty());
        assert!(output.active_geometry.is_none());
        // Una colonna rimossa puo' essere chiave: sorted_by eliminato,
        // row_count invariato (1:1 sulle righe).
        let output = ok(
            "table.select_columns",
            &[proven_contract()],
            json!({"columns": ["id"]}),
        );
        assert!(output.properties.sorted_by.is_none());
        assert_eq!(proven_rows(&output), 100);
        assert!(err("table.select_columns", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));
        assert!(err("table.select_columns", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));
        assert!(err("table.select_columns", &[tabular_contract()], json!({"columns": ["id", "id"]})).to_string().contains("ripetuta"));

        // limit: schema invariato, ordine preservato, row_count non esatto.
        let output = ok("table.limit", &[proven_contract()], json!({"n": 10}));
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_none());
        assert!(err("table.limit", &[tabular_contract()], json!({})).to_string().contains("config"));

        // top_n: sorted_by Proven sulle chiavi, row_count = min(n, righe).
        let output = ok(
            "table.top_n",
            &[proven_contract()],
            json!({"columns": ["value"], "n": 10}),
        );
        assert_eq!(proven_sorted_keys(&output).len(), 1);
        assert_eq!(proven_rows(&output), 10);
        let output = ok(
            "table.top_n",
            &[proven_contract()],
            json!({"columns": ["value"], "n": 10_000}),
        );
        assert_eq!(proven_rows(&output), 100);
        assert!(err("table.top_n", &[tabular_contract()], json!({"columns": [], "n": 1})).to_string().contains("columns"));
        assert!(err("table.top_n", &[tabular_contract()], json!({"columns": ["missing"], "n": 1})).to_string().contains("missing"));

        // stable_fingerprint: append Utf8 non nullable; colonne validate.
        let output = ok(
            "table.stable_fingerprint",
            &[tabular_contract()],
            json!({"columns": ["id", "name"]}),
        );
        assert_field(&output, "fingerprint", &DataType::Utf8, false);
        assert!(err("table.stable_fingerprint", &[tabular_contract()], json!({"columns": ["lst"]})).to_string().contains("scalare"));
        assert!(err("table.stable_fingerprint", &[tabular_contract()], json!({"columns": ["id", "id"]})).to_string().contains("ripetuta"));
        // Default (tutte le colonne): lo schema di test contiene List/Struct,
        // non leggibili via profilo scalare -> fail-closed in validazione.
        assert!(err("table.stable_fingerprint", &[tabular_contract()], json!({})).to_string().contains("scalare"));
    }

    #[test]
    // Molte fixture in sequenza lineare: lunghezza intrinseca ai casi coperti.
    #[allow(clippy::too_many_lines)]
    fn v1_2_extensions_analyze_contracts() {
        // align_schema: riordino/proiezione + colonna aggiunta di null (o
        // default non nullable); mismatch di tipo -> errore (mai cast).
        let output = ok(
            "table.align_schema",
            &[proven_contract()],
            json!({"columns": [
                {"name": "name", "type": "Utf8"},
                {"name": "id", "type": "Int64"},
                {"name": "note", "type": "Utf8"},
                {"name": "score", "type": "Float64", "default": 0}
            ]}),
        );
        let names: Vec<_> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["name", "id", "note", "score"]);
        assert_field(&output, "note", &DataType::Utf8, true);
        assert_field(&output, "score", &DataType::Float64, false);
        // Colonne rimosse/aggiunte: sorted_by eliminato, row_count invariato.
        assert!(output.properties.sorted_by.is_none());
        assert_eq!(proven_rows(&output), 100);
        // Permutazione pura con keep_extra: sorted_by preservato.
        let output = ok(
            "table.align_schema",
            &[proven_contract()],
            json!({"columns": [
                {"name": "name", "type": "Utf8"},
                {"name": "id", "type": "Int64"}
            ], "keep_extra": true}),
        );
        assert!(output.properties.sorted_by.is_some());
        // Geometria dichiarata col tipo giusto (Binary) e' propagata.
        let output = ok(
            "table.align_schema",
            &[geo_contract()],
            json!({"columns": [{"name": "geom", "type": "Binary"}, {"name": "id", "type": "Int64"}]}),
        );
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        // Errori: tipo diverso, default non convertibile, duplicati, vuoto.
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": [{"name": "id", "type": "Utf8"}]})).to_string().contains("cast implicito"));
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": [{"name": "x", "type": "Int64", "default": "abc"}]})).to_string().contains("default"));
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": [{"name": "id", "type": "Int64"}, {"name": "id", "type": "Int64"}]})).to_string().contains("ripetuta"));
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));

        // concat_by_name: unione per nome su schemi diversi.
        let (a, b) = simple_pair();
        let third = DataContract::new(
            Arc::new(Schema::new(vec![
                Field::new("name", DataType::Utf8, true),
                Field::new("extra", DataType::Int64, false),
            ])),
            Vec::new(),
            None,
            ContractProperties {
                sorted_by: None,
                row_count: Some(ContractProperty::new(
                    PropertyConfidence::Proven(25),
                    PropertyScope::Dataset,
                )),
            },
        )
        .unwrap();
        let output = ok("table.concat_by_name", &[a, b, third], json!({}));
        let names: Vec<_> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["id", "name", "extra"]);
        // id manca nel terzo input -> nullable; row_count = somma esatta.
        assert_field(&output, "id", &DataType::Int64, true);
        assert_eq!(proven_rows(&output), 175);
        // Tipi incompatibili per nome: errore.
        let incompatible = DataContract::tabular(Arc::new(Schema::new(vec![Field::new(
            "id",
            DataType::Utf8,
            true,
        )])));
        let (a, b) = simple_pair();
        assert!(err("table.concat_by_name", &[a, incompatible, b], json!({})).to_string().contains("incompatibili"));
        // Strict: schemi permutati -> errore.
        let permuted = DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("id", DataType::Int64, false),
        ])));
        let (a, _) = simple_pair();
        assert!(err("table.concat_by_name", &[a, permuted], json!({"strict": true})).to_string().contains("schemi"));

        // validate_rules annotate: tre colonne non nullable, row_count 1:1.
        let output = ok(
            "table.validate_rules",
            &[proven_contract()],
            json!({"rules": [
                {"name": "r1", "operator": "gt", "column": "value", "value": 0},
                {"name": "r2", "operator": "regex", "column": "name", "value": "^[a-z]+$", "severity": "warning"}
            ]}),
        );
        assert_field(&output, "_valid", &DataType::Boolean, false);
        assert_field(&output, "_errors", &DataType::Utf8, false);
        assert_field(&output, "_warnings", &DataType::Utf8, false);
        assert_eq!(proven_rows(&output), 100);
        // Summary: dataset nuovo (name, errors, warnings).
        let output = ok(
            "table.validate_rules",
            &[tabular_contract()],
            json!({"output_mode": "summary", "rules": [
                {"name": "r1", "operator": "isnull", "column": "name"}
            ]}),
        );
        assert_eq!(output.schema.fields().len(), 3);
        assert_field(&output, "name", &DataType::Utf8, false);
        assert_field(&output, "errors", &DataType::Int64, false);
        assert!(output.geometries.is_empty());
        // Errori di validazione: regex invalida, ordinato su Utf8, regex su
        // numerica, value mancante/non ammesso, regola duplicata.
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "regex", "column": "name", "value": "("}]})).to_string().contains("regex"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "gt", "column": "name", "value": 1}]})).to_string().contains("numerica"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "regex", "column": "id", "value": "1"}]})).to_string().contains("Utf8"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "eq", "column": "id"}]})).to_string().contains("value"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "isnull", "column": "id", "value": 1}]})).to_string().contains("value"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [
            {"name": "r", "operator": "isnull", "column": "id"},
            {"name": "r", "operator": "notnull", "column": "id"}
        ]})).to_string().contains("ripetuta"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": []})).to_string().contains("rules"));

        // hmac_sha256: append Utf8; nullable solo con null_policy "null".
        let output = ok(
            "table.hmac_sha256",
            &[tabular_contract()],
            json!({"columns": ["id", "name"], "key_env": "PLENORA_HMAC"}),
        );
        assert_field(&output, "hmac", &DataType::Utf8, false);
        let output = ok(
            "table.hmac_sha256",
            &[tabular_contract()],
            json!({"columns": ["id"], "key_env": "PLENORA_HMAC", "null_policy": "null"}),
        );
        assert_field(&output, "hmac", &DataType::Utf8, true);
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": [], "key_env": "K"})).to_string().contains("columns"));
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": ["id"], "key_env": " "})).to_string().contains("key_env"));
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": ["lst"], "key_env": "K"})).to_string().contains("scalare"));
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": ["id", "id"], "key_env": "K"})).to_string().contains("ripetuta"));
    }

    #[test]
    fn v1_3_extensions_analyze_contracts() {
        // fuzzy_join: naming Manipola con chiave destra INCLUSA (_R) e score
        // Float64 in coda (non nullable in inner); proprieta' declassate.
        let inputs = [proven_contract(), right_contract()];
        let output = ok(
            "table.fuzzy_join",
            &inputs,
            json!({"left_key": "name", "right_key": "rname", "metric": "jaro_winkler", "threshold": 0.9, "blocking": "prefix"}),
        );
        let names: Vec<_> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(
            names.last(),
            Some(&"score"),
            "score in coda: {names:?}"
        );
        assert!(names.contains(&"rname_R"), "chiave destra inclusa: {names:?}");
        assert!(names.contains(&"name"), "chiave sinistra conserva il nome: {names:?}");
        assert_field(&output, "score", &DataType::Float64, false);
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());
        // how=left: score nullable; nome personalizzato.
        let output = ok(
            "table.fuzzy_join",
            &[tabular_contract(), right_contract()],
            json!({"left_key": "name", "right_key": "rname", "metric": "levenshtein", "threshold": 0.5, "blocking": "soundex", "how": "left", "score_column": "similarity"}),
        );
        assert_field(&output, "similarity", &DataType::Float64, true);
        // Soglia validata in analisi: 0 escluso, oltre 1 e NaN rifiutati.
        for bad in [json!(0.0), json!(-0.1), json!(1.5), json!(null)] {
            let mut config = json!({"left_key": "name", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "none"});
            config["threshold"] = bad;
            assert!(
                err("table.fuzzy_join", &[tabular_contract(), right_contract()], config)
                    .to_string()
                    .contains("fuzzy_join")
            );
        }
        // Chiavi mancanti o non Utf8, blocking_param fuori posto, collisione score.
        assert!(err("table.fuzzy_join", &[tabular_contract(), right_contract()], json!({"left_key": "missing", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "prefix"})).to_string().contains("missing"));
        assert!(err("table.fuzzy_join", &[tabular_contract(), right_contract()], json!({"left_key": "id", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "prefix"})).to_string().contains("Utf8"));
        assert!(err("table.fuzzy_join", &[tabular_contract(), right_contract()], json!({"left_key": "name", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "soundex", "blocking_param": 3})).to_string().contains("blocking_param"));
        // Collisione score: l'unica possibile e' una chiave sinistra chiamata
        // come la colonna score (la chiave conserva il nome, le altre colonne
        // prendono suffisso _L/_R).
        let key_named_score = DataContract::tabular(Arc::new(Schema::new(vec![Field::new(
            "score",
            DataType::Utf8,
            true,
        )])));
        assert!(err("table.fuzzy_join", &[key_named_score, right_contract()], json!({"left_key": "score", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "prefix"})).to_string().contains("collisione"));
    }

    #[test]
    fn date_ops_produce_expected_types() {
        let formatted = ok(
            "table.date_format",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y-%m-%d", "output_column": "df"}),
        );
        assert_field(&formatted, "df", &DataType::Utf8, true);

        let added = ok(
            "table.date_add",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y-%m-%d", "amount": 3, "unit": "days", "output_column": "da"}),
        );
        assert_field(&added, "da", &DataType::Utf8, true);
        assert!(err(
            "table.date_add",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y", "amount": 1, "output_column": "da"})
        )
        .to_string()
        .contains("unit"));

        let diffed = ok(
            "table.date_diff",
            &[tabular_contract()],
            json!({"start_column": "name", "end_column": "name", "input_format": "%Y", "unit": "days", "output_column": "dd"}),
        );
        assert_field(&diffed, "dd", &DataType::Float64, true);

        let converted = ok(
            "table.timezone_convert",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Europe/Rome", "output_column": "tc"}),
        );
        assert_field(&converted, "tc", &DataType::Utf8, true);
        assert!(err(
            "table.timezone_convert",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Marte/Olympus", "output_column": "tc"})
        )
        .to_string()
        .contains("timezone"));
    }

    // -- filtering / analysis -------------------------------------------------

    #[test]
    fn filter_preserves_schema_and_sorted_by_but_not_row_count() {
        let output = ok(
            "table.filter",
            &[proven_contract()],
            json!({"column": "value", "operator": ">", "value": 3}),
        );
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert_eq!(output.geometries.len(), 1, "filter preserva la geometria");
        assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
        assert!(output.properties.row_count.is_none());
        assert!(err(
            "table.filter",
            &[tabular_contract()],
            json!({"column": "value", "operator": ">", "value": "abc"})
        )
        .to_string()
        .contains("numeric"));
        assert!(err(
            "table.filter",
            &[tabular_contract()],
            json!({"column": "missing", "operator": "==", "value": 1})
        )
        .to_string()
        .contains("missing"));
    }

    #[test]
    fn conditional_type_comes_from_config_literals() {
        let numeric = ok(
            "table.conditional",
            &[tabular_contract()],
            json!({"column": "value", "conditions": [{"operator": ">", "value": 3, "result": 1}], "default_value": 0}),
        );
        assert_field(&numeric, "result", &DataType::Float64, true);
        let textual = ok(
            "table.conditional",
            &[tabular_contract()],
            json!({"column": "value", "conditions": [{"operator": ">", "value": 3, "result": "high"}], "default_value": "low"}),
        );
        assert_field(&textual, "result", &DataType::Utf8, false);
        assert!(err(
            "table.conditional",
            &[tabular_contract()],
            json!({"column": "missing", "conditions": []})
        )
        .to_string()
        .contains("missing"));
    }

    #[test]
    fn lookup_bin_statistics_sample_behave_as_kernels() {
        let lookup = ok(
            "table.lookup",
            &[tabular_contract()],
            json!({"column": "name", "mapping": {"a": "A"}}),
        );
        assert_field(&lookup, "name", &DataType::Utf8, true);
        assert!(err("table.lookup", &[tabular_contract()], json!({"column": "missing", "mapping": {}})).to_string().contains("missing"));

        let binned = ok("table.bin", &[tabular_contract()], json!({"column": "value", "bins": 5}));
        assert_field(&binned, "value_bin", &DataType::Utf8, true);
        assert!(err("table.bin", &[tabular_contract()], json!({"column": "value", "bins": 1})).to_string().contains("2..=100"));
        assert!(err(
            "table.bin",
            &[tabular_contract()],
            json!({"column": "value", "bins": [0.0, 10.0, 5.0]})
        )
        .to_string()
        .contains("crescenti"));

        let stats = ok("table.statistics", &[tabular_contract()], json!({"column": "value"}));
        for name in ["value_count", "value_min", "value_max", "value_mean", "value_median", "value_std"] {
            assert_field(&stats, name, &DataType::Float64, true);
        }
        assert!(err("table.statistics", &[tabular_contract()], json!({"column": "flag"})).to_string().contains("numero"));

        let sampled = ok("table.sample", &[proven_contract()], json!({"n": 10}));
        assert_eq!(sampled.schema.fields().len(), base_fields().len());
        assert_eq!(proven_rows(&sampled), 10, "sample senza stratify: min(n, righe)");
        assert!(sampled.properties.sorted_by.is_none(), "sample mescola le righe");
        assert!(err("table.sample", &[tabular_contract()], json!({"fraction": 1.5})).to_string().contains("fraction"));
    }

    #[test]
    fn flatten_json_requires_explicit_output_columns() {
        let output = ok(
            "table.flatten_json",
            &[tabular_contract()],
            json!({"column": "name", "output_columns": ["name_a", "name_b"]}),
        );
        assert_field(&output, "name_a", &DataType::Utf8, true);
        assert_field(&output, "name_b", &DataType::Utf8, true);
        assert!(matches!(
            err("table.flatten_json", &[tabular_contract()], json!({"column": "name"})),
            PlenoraError::Unsupported(_)
        ));
        assert!(err(
            "table.flatten_json",
            &[tabular_contract()],
            json!({"column": "name", "output_columns": ["other"]})
        )
        .to_string()
        .contains("prefix"));
    }

    // -- aggregation ----------------------------------------------------------

    #[test]
    fn sort_produces_proven_stream_sorted_by_and_keeps_rows() {
        let output = ok("table.sort", &[proven_contract()], json!({"columns": ["id", "name"]}));
        let keys = proven_sorted_keys(&output);
        assert_eq!(keys.len(), 2);
        assert_eq!(proven_rows(&output), 100);
        assert_eq!(output.geometries.len(), 1);
        assert!(err("table.sort", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));
        assert!(err("table.sort", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));
    }

    #[test]
    fn distinct_and_dedup_keep_schema_and_drop_row_count() {
        let distinct = ok("table.distinct", &[proven_contract()], json!({"subset": ["name"]}));
        assert_eq!(distinct.schema.fields().len(), base_fields().len());
        assert!(distinct.properties.row_count.is_none());
        assert_eq!(proven_sorted_keys(&distinct), &[FieldId(0)]);
        assert!(err("table.distinct", &[tabular_contract()], json!({"subset": ["missing"]})).to_string().contains("missing"));

        let dedup = ok(
            "table.dedup_advanced",
            &[tabular_contract()],
            json!({"subset": ["name"], "order_column": "id"}),
        );
        assert_eq!(proven_sorted_keys(&dedup).len(), 1);
        assert!(matches!(
            err("table.dedup_advanced", &[tabular_contract()], json!({"subset": ["name"], "keep": "false"})),
            PlenoraError::InvalidPlan(_)
        ));
    }

    #[test]
    fn aggregate_builds_group_and_aggregation_columns() {
        let output = ok(
            "table.aggregate",
            &[tabular_contract()],
            json!({
                "group_by": ["name"],
                "aggregations": [
                    {"column": "value", "function": "sum"},
                    {"column": "value", "function": "mean"},
                    {"column": "id", "function": "count"}
                ]
            }),
        );
        assert_eq!(output.schema.fields().len(), 4);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_field(&output, "value_sum", &DataType::Float64, true);
        assert_field(&output, "value_mean", &DataType::Float64, true);
        assert_field(&output, "id", &DataType::Int64, false);
        assert!(output.geometries.is_empty(), "geometria non in group_by: tabellare");
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());

        let count_only = ok(
            "table.aggregate",
            &[tabular_contract()],
            json!({"group_by": ["name"]}),
        );
        assert_field(&count_only, "count", &DataType::Int64, false);

        assert!(err("table.aggregate", &[tabular_contract()], json!({"group_by": []})).to_string().contains("group_by"));
        assert!(err(
            "table.aggregate",
            &[tabular_contract()],
            json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "quantile"}]})
        )
        .to_string()
        .contains("quantile"));
    }

    #[test]
    fn aggregate_preserves_geometry_grouped_by() {
        let output = ok(
            "table.aggregate",
            &[geo_contract()],
            json!({"group_by": ["geom"], "aggregations": [{"column": "id", "function": "count"}]}),
        );
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.geometries[0].name, "geom");
        assert_eq!(output.geometries[0].field_id, FieldId(7));
    }

    #[test]
    fn rolling_and_window_append_float64_and_track_order() {
        let rolling = ok(
            "table.rolling_window",
            &[tabular_contract()],
            json!({"column": "value", "function": "sum", "window": 3, "order_column": "id", "output_column": "rw"}),
        );
        assert_field(&rolling, "rw", &DataType::Float64, true);
        assert_eq!(proven_sorted_keys(&rolling).len(), 1);
        assert!(err(
            "table.rolling_window",
            &[tabular_contract()],
            json!({"column": "value", "function": "sum", "window": 0, "output_column": "rw"})
        )
        .to_string()
        .contains("window"));

        let window = ok(
            "table.window_function",
            &[tabular_contract()],
            json!({"column": "value", "function": "rank"}),
        );
        assert_field(&window, "value_rank", &DataType::Float64, true);
        assert!(err(
            "table.window_function",
            &[tabular_contract()],
            json!({"column": "value", "function": "ntile"})
        )
        .to_string()
        .contains("buckets"));
        assert!(err(
            "table.window_function",
            &[tabular_contract()],
            json!({"column": "value", "function": "rank", "buckets": 4})
        )
        .to_string()
        .contains("buckets"));
    }

    // -- columns / cleansing ---------------------------------------------------

    #[test]
    fn drop_columns_removes_geometry_and_becomes_tabular() {
        let output = ok("table.drop_columns", &[geo_contract()], json!({"columns": ["geom"]}));
        assert!(output.schema.field_with_name("geom").is_err());
        assert!(output.geometries.is_empty(), "drop della geometria -> tabellare");
        assert!(output.active_geometry.is_none());
        // Nomi inesistenti ignorati silenziosamente (comportamento del kernel).
        let unchanged = ok("table.drop_columns", &[geo_contract()], json!({"columns": ["nope"]}));
        assert_eq!(unchanged.geometries.len(), 1);
        assert!(err("table.drop_columns", &[tabular_contract()], json!({"columns": "x"})).to_string().contains("config"));
    }

    #[test]
    fn rename_preserves_field_id_and_changes_geometry_name() {
        let output = ok(
            "table.rename",
            &[geo_contract()],
            json!({"renames": [{"old_name": "geom", "new_name": "geometry"}]}),
        );
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.geometries[0].name, "geometry");
        assert_eq!(output.geometries[0].field_id, FieldId(7), "rinomina preserva il FieldId");
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        assert!(err(
            "table.rename",
            &[tabular_contract()],
            json!({"renames": [{"old_name": "id", "new_name": "name"}]})
        )
        .to_string()
        .contains("duplicato"));
    }

    #[test]
    fn reorder_columns_validates_and_reorders() {
        let output = ok(
            "table.reorder_columns",
            &[tabular_contract()],
            json!({"columns": ["name", "id"]}),
        );
        assert_eq!(output.schema.field(0).name(), "name");
        assert_eq!(output.schema.field(1).name(), "id");
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert!(err(
            "table.reorder_columns",
            &[tabular_contract()],
            json!({"columns": ["id", "id"]})
        )
        .to_string()
        .contains("ripetuta"));
        assert!(err(
            "table.reorder_columns",
            &[tabular_contract()],
            json!({"columns": ["missing"]})
        )
        .to_string()
        .contains("missing"));
    }

    #[test]
    fn concat_and_split_columns_validate_utf8() {
        let concatenated = ok(
            "table.concat_columns",
            &[tabular_contract()],
            json!({"columns": ["name"]}),
        );
        assert_field(&concatenated, "concatenated", &DataType::Utf8, true);
        assert!(err(
            "table.concat_columns",
            &[tabular_contract()],
            json!({"columns": ["name", "value"]})
        )
        .to_string()
        .contains("Utf8"));

        let split = ok(
            "table.split_column",
            &[tabular_contract()],
            json!({"column": "name", "new_columns": ["first", "second"]}),
        );
        assert_field(&split, "first", &DataType::Utf8, true);
        assert_field(&split, "second", &DataType::Utf8, true);
        assert_field(&split, "name", &DataType::Utf8, true);
        assert!(err(
            "table.split_column",
            &[tabular_contract()],
            json!({"column": "name", "new_columns": []})
        )
        .to_string()
        .contains("new_columns"));
    }

    #[test]
    fn fill_na_checks_types_and_value_coherence() {
        let output = ok(
            "table.fill_na",
            &[tabular_contract()],
            json!({"column": "value", "value": 0}),
        );
        assert_field(&output, "value", &DataType::Float64, true);
        assert!(err(
            "table.fill_na",
            &[tabular_contract()],
            json!({"column": "value", "value": "abc"})
        )
        .to_string()
        .contains("fill"));
        // Senza column il target e' l'intero schema: Binary (geometria) non supportato.
        assert!(err("table.fill_na", &[geo_contract()], json!({}))
            .to_string()
            .contains("fill_na non supporta"));
    }

    #[test]
    fn replace_requires_utf8_and_valid_regex() {
        let output = ok(
            "table.replace",
            &[tabular_contract()],
            json!({"column": "name", "old_value": "a", "new_value": "b"}),
        );
        assert_field(&output, "name", &DataType::Utf8, true);
        assert!(err(
            "table.replace",
            &[tabular_contract()],
            json!({"column": "value", "old_value": "1", "new_value": "2"})
        )
        .to_string()
        .contains("Utf8"));
        assert!(err(
            "table.replace",
            &[tabular_contract()],
            json!({"column": "name", "old_value": "(", "new_value": "b", "regex": true})
        )
        .to_string()
        .contains("regex"));
    }

    #[test]
    fn type_cast_maps_target_types_and_drops_geometry_metadata() {
        let output = ok(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "date32"}),
        );
        assert_field(&output, "name", &DataType::Date32, true);

        let decimal = ok(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "decimal128", "precision": 12, "scale": 3}),
        );
        assert_field(&decimal, "name", &DataType::Decimal128(12, 3), true);

        let timestamp = ok(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "timestamp_millis", "timezone": "UTC"}),
        );
        assert_field(
            &timestamp,
            "name",
            &DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
            true,
        );

        // Cast della colonna geometrica: metadati geoarrow persi -> tabellare.
        let casted_geometry = ok(
            "table.type_cast",
            &[geo_contract()],
            json!({"column": "geom", "target_type": "str"}),
        );
        assert!(casted_geometry.geometries.is_empty());

        assert!(err(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "decimal128"})
        )
        .to_string()
        .contains("precision"));
        assert!(err(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "timestamp_millis", "timezone": "Marte/Olympus"})
        )
        .to_string()
        .contains("timezone"));
    }

    // -- formula / expression ---------------------------------------------------

    #[test]
    fn formula_infers_static_type() {
        let numeric = ok(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "value * 2 + id"}),
        );
        assert_field(&numeric, "f", &DataType::Float64, true);
        let textual = ok(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "name + ' suffix'"}),
        );
        assert_field(&textual, "f", &DataType::Utf8, true);
        assert!(err(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "value +"})
        )
        .to_string()
        .contains("formula"));
        assert!(err(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "missing + 1"})
        )
        .to_string()
        .contains("missing"));
        assert!(err(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "name - 'x'"})
        )
        .to_string()
        .contains("testo"));
    }

    #[test]
    fn expression_uses_declared_or_inferred_output_type() {
        let declared = ok(
            "table.expression",
            &[tabular_contract()],
            json!({
                "output_column": "e",
                "expression": {"kind": "column", "name": "name"},
                "output_type": "boolean"
            }),
        );
        assert_field(&declared, "e", &DataType::Boolean, true);

        let inferred = ok(
            "table.expression",
            &[tabular_contract()],
            json!({
                "output_column": "e",
                "expression": {
                    "kind": "binary",
                    "op": "add",
                    "left": {"kind": "column", "name": "value"},
                    "right": {"kind": "literal", "value": 1}
                }
            }),
        );
        assert_field(&inferred, "e", &DataType::Float64, true);

        let heterogeneous = err(
            "table.expression",
            &[tabular_contract()],
            json!({
                "output_column": "e",
                "expression": {
                    "kind": "function",
                    "name": "coalesce",
                    "args": [{"kind": "column", "name": "value"}, {"kind": "column", "name": "name"}]
                }
            }),
        );
        assert!(heterogeneous.to_string().contains("eterogenei"), "{heterogeneous}");
        assert!(err(
            "table.expression",
            &[tabular_contract()],
            json!({"output_column": "e", "expression": {"kind": "column", "name": "missing"}})
        )
        .to_string()
        .contains("missing"));
    }

    /// Contratto con colonne temporali native per `date_trunc`.
    fn temporal_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("d", DataType::Date32, true),
            Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new(
                "tstz",
                DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                true,
            ),
            Field::new("name", DataType::Utf8, true),
        ])))
    }

    fn date_trunc(unit: &str, column: &str) -> Value {
        json!({
            "kind": "function",
            "name": "date_trunc",
            "args": [{"kind": "literal", "value": unit}, {"kind": "column", "name": column}]
        })
    }

    #[test]
    // Molte fixture in sequenza lineare: lunghezza intrinseca ai casi coperti.
    #[allow(clippy::too_many_lines)]
    fn expression_date_trunc_native_temporal_types() {
        // Auto: il tipo discende dalla colonna di input (mai Utf8).
        let date = ok(
            "table.expression",
            &[temporal_contract()],
            json!({"output_column": "e", "expression": date_trunc("month", "d")}),
        );
        assert_field(&date, "e", &DataType::Date32, true);
        let ts = ok(
            "table.expression",
            &[temporal_contract()],
            json!({"output_column": "e", "expression": date_trunc("hour", "ts")}),
        );
        assert_field(
            &ts,
            "e",
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        );
        // output_type esplicito date32/timestamp_ms.
        let explicit = ok(
            "table.expression",
            &[temporal_contract()],
            json!({
                "output_column": "e",
                "expression": date_trunc("day", "ts"),
                "output_type": "timestamp_ms"
            }),
        );
        assert_field(
            &explicit,
            "e",
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        );
        // Annidamento: il tipo discende dalla colonna radice.
        let nested = ok(
            "table.expression",
            &[temporal_contract()],
            json!({
                "output_column": "e",
                "expression": {
                    "kind": "function",
                    "name": "date_trunc",
                    "args": [{"kind": "literal", "value": "year"}, date_trunc("month", "ts")]
                }
            }),
        );
        assert_field(
            &nested,
            "e",
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        );
        // Unita' invalida, non letterale, sub-day su Date32.
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("week", "ts")})
            )
            .to_string()
            .contains("unita' non valida")
        );
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({
                    "output_column": "e",
                    "expression": {
                        "kind": "function",
                        "name": "date_trunc",
                        "args": [{"kind": "column", "name": "name"}, {"kind": "column", "name": "ts"}]
                    }
                })
            )
            .to_string()
            .contains("letterale")
        );
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("hour", "d")})
            )
            .to_string()
            .contains("sub-day")
        );
        // Input testuale (nessun parsing) e timezone-aware: errori in validazione.
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("day", "name")})
            )
            .to_string()
            .contains("Date32 o Timestamp")
        );
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("day", "tstz")})
            )
            .to_string()
            .contains("timezone-aware")
        );
        // Valore letterale null: tipo Any -> Utf8 in auto, come il kernel.
        let null_source = ok(
            "table.expression",
            &[temporal_contract()],
            json!({
                "output_column": "e",
                "expression": {
                    "kind": "function",
                    "name": "date_trunc",
                    "args": [{"kind": "literal", "value": "day"}, {"kind": "literal", "value": Value::Null}]
                }
            }),
        );
        assert_field(&null_source, "e", &DataType::Utf8, true);
        // Valore non temporale (letterale numerico): rifiutato a secco.
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({
                    "output_column": "e",
                    "expression": {
                        "kind": "function",
                        "name": "date_trunc",
                        "args": [{"kind": "literal", "value": "day"}, {"kind": "literal", "value": 5}]
                    }
                })
            )
            .to_string()
            .contains("colonna temporale")
        );
        // Annidamento: sub-day su un date_trunc che produce Date32.
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({
                    "output_column": "e",
                    "expression": {
                        "kind": "function",
                        "name": "date_trunc",
                        "args": [{"kind": "literal", "value": "hour"}, date_trunc("month", "d")]
                    }
                })
            )
            .to_string()
            .contains("sub-day")
        );
    }

    #[test]
    fn formula_rifiuta_colonne_con_tipo_non_valutabile() {
        // Il lookup di tipo dell'analisi rifiuta le colonne fuori dal
        // profilo scalare (qui List), prima di qualunque dato.
        let error = err(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "lst + 1"}),
        );
        assert!(error.to_string().contains("non valutabile"), "{error}");
    }

    // Batteria lineare: un caso per ramo dell'inferenza statica dell'AST.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn expression_inferenza_statica_per_tutti_i_rami_dell_ast() {
        let infer = |expression: Value| {
            ok(
                "table.expression",
                &[tabular_contract()],
                json!({"output_column": "e", "expression": expression}),
            )
        };
        let infer_err = |expression: Value| {
            err(
                "table.expression",
                &[tabular_contract()],
                json!({"output_column": "e", "expression": expression}),
            )
        };
        let col = |name: &str| json!({"kind": "column", "name": name});
        let lit = |value: Value| json!({"kind": "literal", "value": value});
        let func = |name: &str, args: Vec<Value>| {
            json!({"kind": "function", "name": name, "args": args})
        };

        // Colonne: Boolean resta Boolean; i tipi fuori profilo sono errori.
        assert_field(&infer(col("flag")), "e", &DataType::Boolean, true);
        assert!(infer_err(col("lst")).to_string().contains("non valutabile"));
        assert!(infer_err(col("st")).to_string().contains("non valutabile"));

        // Letterali scalari: null -> Any (Utf8 in auto), bool, numero, testo.
        assert_field(&infer(lit(Value::Null)), "e", &DataType::Utf8, true);
        assert_field(&infer(lit(json!(true))), "e", &DataType::Boolean, true);
        assert_field(&infer(lit(json!(42))), "e", &DataType::Float64, true);
        assert_field(&infer(lit(json!("x"))), "e", &DataType::Utf8, true);

        // Unari: not/negate richiedono Boolean/Number; is_null e' Boolean.
        let unary = |op: &str, value: Value| json!({"kind": "unary", "op": op, "value": value});
        assert_field(&infer(unary("not", col("flag"))), "e", &DataType::Boolean, true);
        assert_field(&infer(unary("negate", col("value"))), "e", &DataType::Float64, true);
        assert_field(&infer(unary("is_null", col("name"))), "e", &DataType::Boolean, true);
        assert_field(&infer(unary("is_not_null", col("value"))), "e", &DataType::Boolean, true);
        assert!(infer_err(unary("not", col("value"))).to_string().contains("operando Boolean"));
        assert!(infer_err(unary("negate", col("name"))).to_string().contains("operando Number"));

        // Binari: logici su Boolean, confronti su tipi omogenei -> Boolean.
        let binary = |op: &str, left: Value, right: Value| {
            json!({"kind": "binary", "op": op, "left": left, "right": right})
        };
        assert_field(
            &infer(binary("and", col("flag"), lit(json!(true)))),
            "e",
            &DataType::Boolean,
            true,
        );
        assert_field(
            &infer(binary("or", col("flag"), unary("not", col("flag")))),
            "e",
            &DataType::Boolean,
            true,
        );
        assert!(infer_err(binary("and", col("value"), lit(json!(1)))).to_string().contains("operando Boolean"));
        assert!(infer_err(binary("or", col("flag"), col("name"))).to_string().contains("operando Boolean"));
        for op in ["equal", "not_equal", "greater", "greater_equal", "less", "less_equal"] {
            assert_field(
                &infer(binary(op, col("value"), lit(json!(1)))),
                "e",
                &DataType::Boolean,
                true,
            );
        }

        // `in`: membership su lista di letterali -> Boolean.
        assert_field(
            &infer(func("in", vec![col("value"), lit(json!([1, 2, 3]))])),
            "e",
            &DataType::Boolean,
            true,
        );

        // Funzioni testuali: argomenti Text obbligatori.
        assert_field(&infer(func("lower", vec![col("name")])), "e", &DataType::Utf8, true);
        assert_field(&infer(func("upper", vec![col("name")])), "e", &DataType::Utf8, true);
        assert_field(&infer(func("trim", vec![col("name")])), "e", &DataType::Utf8, true);
        assert!(infer_err(func("lower", vec![col("value")])).to_string().contains("operando Text"));
        assert_field(
            &infer(func("concat", vec![col("name"), lit(json!("-")), col("name")])),
            "e",
            &DataType::Utf8,
            true,
        );
        assert_field(&infer(func("length", vec![col("name")])), "e", &DataType::Float64, true);
        assert!(infer_err(func("length", vec![col("value")])).to_string().contains("operando Text"));
        for name in ["contains", "starts_with", "ends_with"] {
            assert_field(
                &infer(func(name, vec![col("name"), lit(json!("a"))])),
                "e",
                &DataType::Boolean,
                true,
            );
        }
        assert!(infer_err(func("contains", vec![col("name"), lit(json!(1))])).to_string().contains("operando Text"));

        // Funzioni numeriche: argomenti Number obbligatori.
        for name in ["abs", "round", "floor", "ceil"] {
            assert_field(
                &infer(func(name, vec![col("value")])),
                "e",
                &DataType::Float64,
                true,
            );
        }
        // `year` legge le colonne temporali come Number (come il kernel).
        assert_field(&infer(func("year", vec![col("d")])), "e", &DataType::Float64, true);
        assert_field(
            &infer(func("power", vec![col("value"), lit(json!(2))])),
            "e",
            &DataType::Float64,
            true,
        );
        assert!(infer_err(func("abs", vec![col("name")])).to_string().contains("operando Number"));

        // substring: (testo, numero, numero?) -> testo.
        assert_field(
            &infer(func("substring", vec![col("name"), lit(json!(0)), lit(json!(2))])),
            "e",
            &DataType::Utf8,
            true,
        );
        assert_field(
            &infer(func("substring", vec![col("name"), lit(json!(1))])),
            "e",
            &DataType::Utf8,
            true,
        );
        // Senza argomenti non c'e' nulla da controllare: il tipo resta Text.
        assert_field(&infer(func("substring", vec![])), "e", &DataType::Utf8, true);
        assert!(infer_err(func("substring", vec![col("value"), lit(json!(0))])).to_string().contains("operando Text"));
        assert!(infer_err(func("substring", vec![col("name"), col("name")])).to_string().contains("operando Number"));

        // regex_replace: tutti gli argomenti testuali -> testo.
        assert_field(
            &infer(func("regex_replace", vec![col("name"), lit(json!("a")), lit(json!("b"))])),
            "e",
            &DataType::Utf8,
            true,
        );
        assert!(infer_err(func("regex_replace", vec![col("name"), lit(json!(1)), lit(json!("b"))])).to_string().contains("operando Text"));

        // between: omogeneita' come i confronti -> Boolean.
        assert_field(
            &infer(func("between", vec![col("value"), lit(json!(0)), lit(json!(1))])),
            "e",
            &DataType::Boolean,
            true,
        );
        assert!(infer_err(func("between", vec![col("value"), lit(json!(0)), col("name")])).to_string().contains("eterogenei"));

        // greatest/least: tipo comune degli argomenti.
        assert_field(
            &infer(func("greatest", vec![col("value"), col("id")])),
            "e",
            &DataType::Float64,
            true,
        );
        assert_field(
            &infer(func("least", vec![col("value"), lit(json!(1))])),
            "e",
            &DataType::Float64,
            true,
        );

        // case: tipo comune dei rami e dell'else; eterogeneo -> errore.
        let case = |branches: Vec<Value>, else_value: Value| {
            json!({"kind": "case", "branches": branches, "else_value": else_value})
        };
        assert_field(
            &infer(case(
                vec![json!({"when": col("flag"), "then": lit(json!(1))})],
                lit(json!(2)),
            )),
            "e",
            &DataType::Float64,
            true,
        );
        assert!(infer_err(case(
            vec![json!({"when": col("flag"), "then": lit(json!(1))})],
            col("name"),
        ))
        .to_string()
        .contains("eterogenei"));

        // output_type dichiarato: nessuna inferenza, tipo rispettato.
        let declared_number = ok(
            "table.expression",
            &[tabular_contract()],
            json!({"output_column": "e", "expression": col("flag"), "output_type": "number"}),
        );
        assert_field(&declared_number, "e", &DataType::Float64, true);
        let declared_date = ok(
            "table.expression",
            &[tabular_contract()],
            json!({"output_column": "e", "expression": col("d"), "output_type": "date32"}),
        );
        assert_field(&declared_date, "e", &DataType::Date32, true);
    }

    // -- quality / governance ----------------------------------------------------

    #[test]
    fn assert_ops_validate_and_pass_through() {
        let input = proven_contract();
        let output = ok(
            "table.assert_schema",
            std::slice::from_ref(&input),
            json!({"fields": [
                {"name": "id", "data_type": "int64", "nullable": false},
                {"name": "name", "data_type": "utf8"}
            ], "allow_extra": true}),
        );
        assert_eq!(proven_rows(&output), 100);
        assert!(err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": [{"name": "id", "data_type": "utf8"}], "allow_extra": true})
        )
        .to_string()
        .contains("tipo errato"));
        assert!(err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": [{"name": "id", "data_type": "wat"}], "allow_extra": true})
        )
        .to_string()
        .contains("non supportato"));

        assert!(ok("table.assert_not_null", &[tabular_contract()], json!({"columns": ["id"]})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_not_null", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));

        assert!(ok("table.assert_unique", &[tabular_contract()], json!({"columns": ["id"]})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_unique", &[tabular_contract()], json!({"columns": ["st"]})).to_string().contains("scalare"));

        assert!(ok("table.assert_range", &[tabular_contract()], json!({"column": "value", "min": 0})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_range", &[tabular_contract()], json!({"column": "flag"})).to_string().contains("numero"));

        assert!(ok("table.assert_regex", &[tabular_contract()], json!({"column": "name", "pattern": "^a+$"})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_regex", &[tabular_contract()], json!({"column": "name", "pattern": "("})).to_string().contains("regex"));
        assert!(err("table.assert_regex", &[tabular_contract()], json!({"column": "value", "pattern": "a"})).to_string().contains("Utf8"));

        let coalesced = ok(
            "table.coalesce",
            &[tabular_contract()],
            json!({"columns": ["value"], "output_column": "c"}),
        );
        assert_field(&coalesced, "c", &DataType::Float64, true);
        assert!(err(
            "table.coalesce",
            &[tabular_contract()],
            json!({"columns": ["value", "id"], "output_column": "c"})
        )
        .to_string()
        .contains("identici"));
        assert!(err(
            "table.coalesce",
            &[tabular_contract()],
            json!({"columns": [], "output_column": "c"})
        )
        .to_string()
        .contains("almeno una"));
    }

    #[test]
    fn assert_schema_verifica_conteggio_posizione_nome_e_nullability() {
        // Le 11 colonne della fixture, in ordine: pass-through completo
        // (copre anche le famiglie list/struct/timestamp/decimal128).
        let expectations: Vec<Value> = [
            ("id", "int64"),
            ("name", "utf8"),
            ("value", "float64"),
            ("flag", "boolean"),
            ("d", "date32"),
            ("ts", "timestamp_millis"),
            ("dc", "decimal128"),
            ("u", "uint64"),
            ("lst", "list"),
            ("st", "struct"),
            ("geom", "binary"),
        ]
        .iter()
        .map(|(name, data_type)| json!({"name": name, "data_type": data_type}))
        .collect();
        let passed = ok(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": expectations, "allow_extra": false}),
        );
        assert_eq!(passed.schema.fields().len(), base_fields().len());

        // Conteggio diverso senza allow_extra.
        let error = err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": [{"name": "id", "data_type": "int64"}]}),
        );
        assert!(error.to_string().contains("attese 1 colonne"), "{error}");

        // Ordered + allow_extra: la posizione oltre lo schema e' un errore.
        let mut beyond = expectations.clone();
        beyond.push(json!({"name": "extra", "data_type": "utf8"}));
        let error = err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": beyond, "allow_extra": true}),
        );
        assert!(
            error.to_string().contains("colonna mancante in posizione 11"),
            "{error}"
        );

        // Nome diverso in posizione: la colonna attesa non coincide.
        let mut renamed = expectations;
        renamed[0] = json!({"name": "wrong", "data_type": "int64"});
        let error = err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": renamed, "allow_extra": true}),
        );
        assert!(
            error.to_string().contains("attesa wrong in posizione 0"),
            "{error}"
        );

        // Unordered: colonna assente per nome.
        let error = err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": [{"name": "missing", "data_type": "utf8"}], "ordered": false, "allow_extra": true}),
        );
        assert!(error.to_string().contains("colonna mancante"), "{error}");

        // Nullability dichiarata diversa dalla colonna (unordered: il
        // confronto per posizione sarebbe respinto prima del nome).
        let error = err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": [{"name": "name", "data_type": "utf8", "nullable": false}], "ordered": false, "allow_extra": true}),
        );
        assert!(error.to_string().contains("nullability errata"), "{error}");

        // dictionary_utf8: tipo parametrico con confronto esatto.
        let dict_contract = DataContract::tabular(Arc::new(Schema::new(vec![Field::new(
            "dict",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )])));
        let passed = ok(
            "table.assert_schema",
            &[dict_contract],
            json!({"fields": [{"name": "dict", "data_type": "dictionary_utf8"}]}),
        );
        assert_eq!(passed.schema.fields().len(), 1);
    }

    #[test]
    fn validate_rules_rifiuta_regole_malformate_prima_dei_dati() {
        let rule_err = |rule: Value| {
            err(
                "table.validate_rules",
                &[tabular_contract()],
                json!({"rules": [rule]}),
            )
        };
        // Nome vuoto (solo spazi).
        assert!(rule_err(json!({"name": "  ", "operator": "isnull", "column": "id"}))
            .to_string()
            .contains("nome regola vuoto"));
        // Regola senza column.
        assert!(rule_err(json!({"name": "r", "operator": "isnull"}))
            .to_string()
            .contains("senza column"));
        // eq/ne su tipo non confrontabile (List).
        assert!(rule_err(json!({"name": "r", "operator": "eq", "column": "lst", "value": 1}))
            .to_string()
            .contains("non confrontabile"));
        // eq numerico con valore non parsabile.
        assert!(rule_err(json!({"name": "r", "operator": "eq", "column": "id", "value": "abc"}))
            .to_string()
            .contains("non numerico"));
        // Confronto ordinato con valore non numerico.
        assert!(rule_err(json!({"name": "r", "operator": "ge", "column": "value", "value": "abc"}))
            .to_string()
            .contains("valore numerico"));
        // range su colonna non numerica.
        assert!(rule_err(json!({"name": "r", "operator": "range", "column": "name", "value": "1,2"}))
            .to_string()
            .contains("range richiede colonna numerica"));
        // range senza la coppia min,max.
        assert!(rule_err(json!({"name": "r", "operator": "range", "column": "id", "value": "1"}))
            .to_string()
            .contains("min,max"));
        // range con estremi non numerici.
        assert!(rule_err(json!({"name": "r", "operator": "range", "column": "id", "value": "a,b"}))
            .to_string()
            .contains("estremi range non numerici"));
        // regex oltre il limite di byte.
        let long_pattern = "a".repeat(Limits::default().max_regex_bytes + 1);
        assert!(rule_err(json!({"name": "r", "operator": "regex", "column": "name", "value": long_pattern}))
            .to_string()
            .contains("max_regex_bytes"));
    }

    #[test]
    fn assert_cardinality_uses_proven_row_count() {
        assert!(ok(
            "table.assert_cardinality",
            &[proven_contract()],
            json!({"min_rows": 50, "max_rows": 100})
        )
        .schema
        .fields()
        .len()
            == base_fields().len());
        assert!(matches!(
            err("table.assert_cardinality", &[proven_contract()], json!({"exact_rows": 5})),
            PlenoraError::InvalidPlan(_)
        ));
    }

    #[test]
    fn assert_metadata_checks_schema_metadata() {
        let mut input = tabular_contract();
        input.schema = Arc::new(Schema::new_with_metadata(
            base_fields(),
            std::collections::HashMap::from([("source".to_owned(), "test".to_owned())]),
        ));
        assert!(ok(
            "table.assert_metadata",
            std::slice::from_ref(&input),
            json!({"expected": {"source": "test"}})
        )
        .schema
        .metadata()
        .contains_key("source"));
        assert!(err(
            "table.assert_metadata",
            &[input],
            json!({"expected": {"source": "other"}})
        )
        .to_string()
        .contains("metadata"));
        // Senza allow_extra i metadata oltre `expected` sono rifiutati.
        let mut input = tabular_contract();
        input.schema = Arc::new(Schema::new_with_metadata(
            base_fields(),
            std::collections::HashMap::from([("source".to_owned(), "test".to_owned())]),
        ));
        assert!(err(
            "table.assert_metadata",
            &[input],
            json!({"expected": {}, "allow_extra": false})
        )
        .to_string()
        .contains("metadata extra"));
    }

    #[test]
    fn assert_foreign_key_and_reconcile_are_binary() {
        let left = tabular_contract();
        let right = right_contract();
        let output = ok(
            "table.assert_foreign_key",
            &[left.clone(), right.clone()],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert!(err(
            "table.assert_foreign_key",
            &[left.clone(), right.clone()],
            json!({"left_keys": ["id"], "right_keys": ["rname"]})
        )
        .to_string()
        .contains("tipi"));

        let report = ok(
            "table.reconcile",
            &[left, right],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert_eq!(report.schema.fields().len(), 2);
        assert_field(&report, "metric", &DataType::Utf8, false);
        assert_field(&report, "value", &DataType::UInt64, false);
        assert_eq!(proven_rows(&report), 5);
        assert!(report.geometries.is_empty());
    }

    // -- reshape ------------------------------------------------------------------

    #[test]
    fn melt_builds_id_var_value_columns() {
        assert!(err(
            "table.melt",
            &[tabular_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value", "dc"]})
        )
        .to_string()
        .contains("eterogenee"));
        let homogeneous = ok(
            "table.melt",
            &[proven_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value"]}),
        );
        let fields = &homogeneous.schema;
        assert_eq!(fields.field(0).name(), "id");
        assert_field(&homogeneous, "variable", &DataType::Utf8, false);
        // collision_free del kernel: "value" esiste gia' nell'input -> "value_1".
        assert_field(&homogeneous, "value_1", &DataType::Float64, true);
        assert_eq!(proven_rows(&homogeneous), 100);

        let as_string = ok(
            "table.melt",
            &[tabular_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value", "dc"], "type_policy": "string"}),
        );
        assert_field(&as_string, "value_1", &DataType::Utf8, true);

        // Collisione col nome di una colonna esistente: suffisso automatico.
        let collision = ok(
            "table.melt",
            &[tabular_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value"], "var_name": "name"}),
        );
        assert_field(&collision, "name_1", &DataType::Utf8, false);

        // Geometria come colonna id: preservata.
        let with_geometry = ok(
            "table.melt",
            &[geo_contract()],
            json!({"id_columns": ["geom"], "value_columns": ["value"]}),
        );
        assert_eq!(with_geometry.geometries.len(), 1);
        assert_eq!(with_geometry.geometries[0].field_id, FieldId(7));
    }

    #[test]
    fn pivot_and_transpose_are_explicitly_unsupported() {
        let pivot = err(
            "table.pivot",
            &[tabular_contract()],
            json!({"index_col": "id", "pivot_col": "name", "value_col": "value"}),
        );
        assert!(matches!(pivot, PlenoraError::Unsupported(_)), "{pivot}");
        let transpose = err(
            "table.transpose",
            &[tabular_contract()],
            json!({"id_column": "id"}),
        );
        assert!(matches!(transpose, PlenoraError::Unsupported(_)), "{transpose}");
        // Config invalida fallisce prima come Contract.
        assert!(matches!(
            err("table.pivot", &[tabular_contract()], json!({"index_col": 1})),
            PlenoraError::InvalidPlan(_)
        ));
    }

    #[test]
    fn explode_replaces_list_with_element_type() {
        let output = ok("table.explode", &[geo_contract()], json!({"column": "lst"}));
        assert_field(&output, "lst", &DataType::Int64, true);
        assert_eq!(output.geometries.len(), 1);
        assert!(output.properties.row_count.is_none());
        let renamed = ok(
            "table.explode",
            &[tabular_contract()],
            json!({"column": "lst", "output_column": "element"}),
        );
        assert_field(&renamed, "element", &DataType::Int64, true);
        assert_field(&renamed, "lst", &DataType::List(Arc::new(Field::new("item", DataType::Int64, true))), true);
        assert!(err("table.explode", &[tabular_contract()], json!({"column": "id"})).to_string().contains("List"));
    }

    #[test]
    fn unnest_expands_struct_fields() {
        let output = ok("table.unnest", &[geo_contract()], json!({"column": "st"}));
        assert!(output.schema.field_with_name("st").is_err(), "drop_source di default");
        assert_field(&output, "a", &DataType::Int64, true);
        assert_field(&output, "b", &DataType::Utf8, true);
        assert_eq!(output.geometries.len(), 1);
        let prefixed = ok(
            "table.unnest",
            &[tabular_contract()],
            json!({"column": "st", "prefix": "s_", "drop_source": false}),
        );
        assert_field(&prefixed, "s_a", &DataType::Int64, true);
        assert_field(&prefixed, "st", &DataType::Struct(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Utf8, true),
        ].into()), true);
        assert!(err("table.unnest", &[tabular_contract()], json!({"column": "id"})).to_string().contains("Struct"));
        // Nome derivato che collide con una colonna esistente: errore.
        let colliding = DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int64, true),
            Field::new(
                "st",
                DataType::Struct(vec![Field::new("a", DataType::Int64, true)].into()),
                true,
            ),
        ])));
        assert!(err("table.unnest", &[colliding], json!({"column": "st"}))
            .to_string()
            .contains("collisione"));
    }

    #[test]
    fn table_diff_builds_key_compare_and_diff_columns() {
        let output = ok(
            "table.table_diff",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        // compare di default: colonne left non chiave presenti in right.
        assert_field(&output, "id", &DataType::Int64, true);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_field(&output, "value", &DataType::Float64, true);
        assert_field(&output, "_diff_status", &DataType::Utf8, false);
        assert_field(&output, "_diff_columns", &DataType::Utf8, true);
        assert_field(&output, "_diff_old_values", &DataType::Utf8, true);
        assert_eq!(output.schema.fields().len(), 6);
        assert!(err(
            "table.table_diff",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": [], "right_keys": []})
        )
        .to_string()
        .contains("chiavi"));
        assert!(err(
            "table.table_diff",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rid"], "compare_columns": ["nope"]})
        )
        .to_string()
        .contains("nope"));
    }

    // -- joins / setops ------------------------------------------------------------

    #[test]
    fn join_suffixes_columns_and_forces_nullable() {
        let output = ok(
            "table.join",
            &[geo_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert_field(&output, "id", &DataType::Int64, true);
        assert_field(&output, "name_L", &DataType::Utf8, true);
        assert_field(&output, "value_L", &DataType::Float64, true);
        assert_field(&output, "name_R", &DataType::Utf8, true);
        assert_field(&output, "rname_R", &DataType::Utf8, true);
        assert!(output.schema.field_with_name("rid").is_err(), "chiave right omessa");
        // Geometria left non chiave: rinominata _L, FieldId preservato.
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.geometries[0].name, "geom_L");
        assert_eq!(output.geometries[0].field_id, FieldId(7));
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        for field in output.schema.fields() {
            assert!(field.is_nullable(), "join forza nullable=true su {}", field.name());
        }
        assert!(err(
            "table.join",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rname"]})
        )
        .to_string()
        .contains("tipi"));
        assert!(err(
            "table.join",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": []})
        )
        .to_string()
        .contains("chiavi"));
    }

    #[test]
    fn join_with_two_geometries_fails_closed() {
        let mut right = right_contract();
        let mut fields: Vec<Field> = right
            .schema
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect();
        fields.push(Field::new("rgeom", DataType::Binary, true));
        right = DataContract::new(
            Arc::new(Schema::new(fields)),
            vec![geometry(70, "rgeom", true)],
            Some(FieldId(70)),
            ContractProperties::default(),
        )
        .unwrap();
        assert!(matches!(
            err(
                "table.join",
                &[geo_contract(), right],
                json!({"left_keys": ["id"], "right_keys": ["rid"]})
            ),
            PlenoraError::InvalidPlan(_)
        ));
    }

    #[test]
    fn cross_join_uses_pandas_suffixes() {
        let output = ok(
            "table.cross_join",
            &[tabular_contract(), right_contract()],
            json!({}),
        );
        assert_field(&output, "name_x", &DataType::Utf8, true);
        assert_field(&output, "name_y", &DataType::Utf8, true);
        assert_field(&output, "value_x", &DataType::Float64, true);
        assert_field(&output, "value_y", &DataType::Float64, true);
        assert_field(&output, "rid", &DataType::Int64, true);
        assert_field(&output, "rname", &DataType::Utf8, true);
        // Collisione residua dopo il rename: errore.
        let mut colliding_fields = base_fields();
        colliding_fields.push(Field::new("name_x", DataType::Utf8, true));
        let colliding = DataContract::tabular(Arc::new(Schema::new(colliding_fields)));
        assert!(err("table.cross_join", &[colliding, right_contract()], json!({}))
            .to_string()
            .contains("collisione"));
    }

    #[test]
    fn membership_joins_return_left_unchanged() {
        for op in ["table.semi_join", "table.anti_join"] {
            let output = ok(
                op,
                &[proven_contract(), right_contract()],
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
            );
            assert_eq!(output.schema.fields().len(), base_fields().len());
            assert_eq!(output.geometries.len(), 1);
            assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
            assert!(output.properties.row_count.is_none());
            assert!(err(op, &[tabular_contract(), right_contract()], json!({"left_keys": [], "right_keys": []}))
                .to_string()
                .contains("chiavi"));
        }
    }

    #[test]
    fn asof_join_keeps_left_rows_and_appends_right_columns() {
        let output = ok(
            "table.asof_join",
            &[proven_contract(), right_contract()],
            json!({"left_on": "id", "right_on": "rid"}),
        );
        assert_field(&output, "id", &DataType::Int64, true);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_field(&output, "name_R", &DataType::Utf8, true);
        assert_field(&output, "value_R", &DataType::Float64, true);
        assert_field(&output, "rname", &DataType::Utf8, true);
        assert!(output.schema.field_with_name("rid").is_err());
        assert_eq!(proven_rows(&output), 100, "una riga per riga left");
        assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
        assert!(err(
            "table.asof_join",
            &[tabular_contract(), right_contract()],
            json!({"left_on": "name", "right_on": "rname"})
        )
        .to_string()
        .contains("Int64 o Float64"));
        assert!(err(
            "table.asof_join",
            &[tabular_contract(), right_contract()],
            json!({"left_on": "id", "right_on": "rid", "tolerance": -1.0})
        )
        .to_string()
        .contains("tolerance"));
    }

    #[test]
    fn concat_merges_nullability_and_sums_row_count() {
        let inputs: [DataContract; 2] = simple_pair().into();
        let output = ok("table.concat", &inputs, json!({}));
        assert_field(&output, "id", &DataType::Int64, false);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_eq!(proven_rows(&output), 150);
        assert!(output.properties.sorted_by.is_none());
        assert!(err(
            "table.concat",
            &[simple_pair().0, right_contract()],
            json!({})
        )
        .to_string()
        .contains("schemi"));
    }

    #[test]
    fn set_operations_validate_schema_and_encoder_types() {
        let (left, right) = simple_pair();
        let union = ok("table.union_distinct", &[left.clone(), right.clone()], json!({}));
        assert_field(&union, "name", &DataType::Utf8, true);
        assert!(union.properties.row_count.is_none());

        for op in ["table.intersect", "table.except"] {
            let output = ok(op, &[left.clone(), right.clone()], json!({}));
            assert_eq!(output.schema.fields().len(), 2);
            assert!(output.properties.row_count.is_none());
            assert!(output.properties.sorted_by.is_none());
        }
        // Schema con List/Struct: l'encoder delle chiavi non li supporta.
        assert!(err(
            "table.union_distinct",
            &[tabular_contract(), tabular_contract()],
            json!({})
        )
        .to_string()
        .contains("non supportato"));
        assert!(err(
            "table.intersect",
            &[simple_pair().0, right_contract()],
            json!({})
        )
        .to_string()
        .contains("schemi"));
    }

    // -- Lineage dei metadata Arrow (R2.4) --------------------------------------

    /// Clona il contratto sostituendo i metadata dello schema Arrow.
    fn with_schema_metadata(contract: &DataContract, entries: &[(&str, &str)]) -> DataContract {
        let metadata: HashMap<String, String> = entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let mut out = contract.clone();
        out.schema = Arc::new(Schema::new_with_metadata(
            contract.schema.fields().clone(),
            metadata,
        ));
        out
    }

    /// Clona il contratto aggiungendo metadata al campo `name`.
    fn with_field_metadata(
        contract: &DataContract,
        name: &str,
        entries: &[(&str, &str)],
    ) -> DataContract {
        let metadata: HashMap<String, String> = entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let patched: Vec<Field> = contract
            .schema
            .fields()
            .iter()
            .map(|field| {
                if field.name().as_str() == name {
                    field.as_ref().clone().with_metadata(metadata.clone())
                } else {
                    field.as_ref().clone()
                }
            })
            .collect();
        let mut out = contract.clone();
        out.schema = Arc::new(Schema::new_with_metadata(
            patched,
            contract.schema.metadata().clone(),
        ));
        out
    }

    #[test]
    fn schema_metadata_survive_aggregate_and_melt() {
        let input = with_schema_metadata(&tabular_contract(), &[("source", "driver-x")]);
        let aggregated = ok(
            "table.aggregate",
            std::slice::from_ref(&input),
            json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "sum"}]}),
        );
        assert_eq!(
            aggregated.schema.metadata().get("source"),
            Some(&"driver-x".to_owned()),
            "aggregate: metadata di schema conservati (R2.4)"
        );
        let melted = ok(
            "table.melt",
            &[input],
            json!({"id_columns": ["id"], "value_columns": ["value"]}),
        );
        assert_eq!(
            melted.schema.metadata().get("source"),
            Some(&"driver-x".to_owned()),
            "melt: metadata di schema conservati (R2.4)"
        );
    }

    #[test]
    fn join_merges_schema_metadata_and_fails_on_conflict() {
        let left = with_schema_metadata(
            &tabular_contract(),
            &[("left_only", "a"), ("shared", "same")],
        );
        let right = with_schema_metadata(
            &right_contract(),
            &[("right_only", "b"), ("shared", "same")],
        );
        let merged = ok(
            "table.join",
            &[left, right],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        let metadata = merged.schema.metadata();
        assert_eq!(metadata.get("left_only"), Some(&"a".to_owned()));
        assert_eq!(metadata.get("right_only"), Some(&"b".to_owned()));
        assert_eq!(metadata.get("shared"), Some(&"same".to_owned()));

        // Stessa chiave con valori diversi: errore Contract che nomina la
        // chiave e non riporta mai i valori (errori senza dati).
        let conflicting = with_schema_metadata(&right_contract(), &[("shared", "other")]);
        let error = err(
            "table.join",
            &[
                with_schema_metadata(&tabular_contract(), &[("shared", "same")]),
                conflicting,
            ],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert!(matches!(error, PlenoraError::InvalidPlan(_)), "{error}");
        let message = error.to_string();
        assert!(message.contains("shared"), "chiave in conflitto nominata: {message}");
        assert!(!message.contains("same"), "mai valori negli errori: {message}");
        assert!(!message.contains("other"), "mai valori negli errori: {message}");
    }

    #[test]
    fn union_distinct_merges_schema_metadata_and_fails_on_conflict() {
        let (left, right) = simple_pair();
        let left = with_schema_metadata(&left, &[("shared", "one")]);
        let right = with_schema_metadata(&right, &[("extra", "two")]);
        let merged = ok("table.union_distinct", &[left.clone(), right], json!({}));
        let metadata = merged.schema.metadata();
        assert_eq!(metadata.get("shared"), Some(&"one".to_owned()));
        assert_eq!(metadata.get("extra"), Some(&"two".to_owned()));

        let conflicting = with_schema_metadata(&simple_pair().1, &[("shared", "other")]);
        assert!(matches!(
            err("table.union_distinct", &[left, conflicting], json!({})),
            PlenoraError::InvalidPlan(_)
        ));
    }

    #[test]
    fn field_metadata_survive_type_preserving_ops() {
        let expected: HashMap<String, String> =
            HashMap::from([("unit".to_owned(), "m/s".to_owned())]);
        let filled = ok(
            "table.fill_na",
            &[with_field_metadata(&tabular_contract(), "value", &[("unit", "m/s")])],
            json!({"column": "value", "value": 0}),
        );
        let field = filled.schema.field_with_name("value").unwrap();
        assert_eq!(field.metadata(), &expected, "fill_na: metadata di campo (R2.4)");

        let replaced = ok(
            "table.replace",
            &[with_field_metadata(&tabular_contract(), "name", &[("unit", "m/s")])],
            json!({"column": "name", "old_value": "a", "new_value": "b"}),
        );
        let field = replaced.schema.field_with_name("name").unwrap();
        assert_eq!(field.metadata(), &expected, "replace: metadata di campo (R2.4)");

        // Join identity-preserving: la rinomina `_L` conserva i metadata.
        let joined = ok(
            "table.join",
            &[
                with_field_metadata(&tabular_contract(), "name", &[("unit", "m/s")]),
                right_contract(),
            ],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        let field = joined.schema.field_with_name("name_L").unwrap();
        assert_eq!(field.metadata(), &expected, "join: metadata di campo (R2.4)");
    }

    #[test]
    fn table_diff_merges_schema_metadata_and_drops_field_metadata() {
        let left = with_schema_metadata(&tabular_contract(), &[("shared", "v")]);
        let left = with_field_metadata(&left, "id", &[("origin", "left")]);
        let right = with_schema_metadata(&right_contract(), &[("shared", "v"), ("r_only", "1")]);
        let output = ok(
            "table.table_diff",
            &[left, right],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert_eq!(output.schema.metadata().get("r_only"), Some(&"1".to_owned()));
        assert_eq!(output.schema.metadata().get("shared"), Some(&"v".to_owned()));
        // Campi ricostruiti = colonne derivate (R2.4): nessun metadata ereditato.
        let field = output.schema.field_with_name("id").unwrap();
        assert!(
            field.metadata().is_empty(),
            "table_diff: campo derivato senza metadata ereditati"
        );
    }

    // -- Proprieta' e geometria ----------------------------------------------------

    #[test]
    fn append_ops_preserve_row_count_and_sorted_by() {
        let output = ok("table.uuid_generator", &[proven_contract()], json!({}));
        assert_eq!(proven_rows(&output), 100);
        assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
        assert_eq!(output.geometries.len(), 1);
    }

    #[test]
    fn overwriting_a_column_drops_sorted_by() {
        // lookup di default sovrascrive la colonna sorgente: possibile chiave.
        let output = ok(
            "table.lookup",
            &[proven_contract()],
            json!({"column": "name", "mapping": {"a": "A"}}),
        );
        assert!(output.properties.sorted_by.is_none());
        assert_eq!(proven_rows(&output), 100);
    }

    #[test]
    fn field_allocator_avoids_collisions_with_input_ids() {
        let mut allocator = FieldAllocator::default();
        let fresh = allocator.alloc();
        assert_eq!(fresh, FieldId(0));
        // L'allocatore osserva gli id degli input (geometria FieldId(7),
        // chiavi FieldId(0)) prima di internare.
        let _ = ok_with_allocator(&mut allocator);
        let id = allocator.alloc();
        assert!(id.0 > 7, "alloc dopo observe deve superare gli id di input: {id}");
    }

    fn ok_with_allocator(allocator: &mut FieldAllocator) -> DataContract {
        analyze_table_contract(
            "table.sort",
            &[proven_contract()],
            &json!({"columns": ["id"]}),
            allocator,
        )
        .unwrap()
    }

    #[test]
    fn sample_and_sort_keep_geometry() {
        let sampled = ok("table.sample", &[geo_contract()], json!({"n": 5}));
        assert_eq!(sampled.geometries.len(), 1);
        let sorted = ok("table.sort", &[geo_contract()], json!({"columns": ["id"]}));
        assert_eq!(sorted.geometries.len(), 1);
        assert_eq!(sorted.geometries[0].field_id, FieldId(7));
    }

    #[test]
    fn config_with_unknown_fields_fails_closed() {
        assert!(matches!(
            err(
                "table.filter",
                &[tabular_contract()],
                json!({"column": "value", "operator": "==", "value": 1, "bogus": true})
            ),
            PlenoraError::InvalidPlan(_)
        ));
    }

    // -- Cross-check analyze vs kernel su batch reali --------------------------
    //
    // Esegue il kernel vero su un piccolo RecordBatch e confronta lo schema
    // del batch prodotto con quello inferito a secco da analyze_table_contract
    // (nomi, tipi, nullability). Copre tutte le op con schema deterministico.

    mod kernel_crosscheck {
        use plenora_core::arrow::array::{
            types::Int64Type, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array,
            ListArray, RecordBatch, StringArray, StructArray,
        };

        use super::*;

        fn signature(schema: &Schema) -> Vec<(String, DataType, bool)> {
            schema
                .fields()
                .iter()
                .map(|field| {
                    (
                        field.name().clone(),
                        field.data_type().clone(),
                        field.is_nullable(),
                    )
                })
                .collect()
        }

        fn simple_batch() -> RecordBatch {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("name", DataType::Utf8, true),
                    Field::new("value", DataType::Float64, true),
                    Field::new("flag", DataType::Boolean, true),
                    Field::new("geom", DataType::Binary, true),
                ])),
                vec![
                    Arc::new(Int64Array::from(vec![3, 1, 2])),
                    Arc::new(StringArray::from(vec![Some("b"), Some("a"), None])),
                    Arc::new(Float64Array::from(vec![Some(3.5), Some(1.5), None])),
                    Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])),
                    Arc::new(BinaryArray::from(vec![
                        Some(b"wkb-a".as_slice()),
                        Some(b"wkb-b".as_slice()),
                        None,
                    ])),
                ],
            )
            .unwrap()
        }

        fn nested_batch() -> RecordBatch {
            let list: ArrayRef = Arc::new(ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
                Some(vec![Some(1), Some(2)]),
                Some(vec![]),
                Some(vec![Some(3)]),
            ]));
            let structure: ArrayRef = Arc::new(StructArray::from(vec![
                (
                    Arc::new(Field::new("a", DataType::Int64, true)),
                    Arc::new(Int64Array::from(vec![Some(1), Some(2), None])) as ArrayRef,
                ),
                (
                    Arc::new(Field::new("b", DataType::Utf8, true)),
                    Arc::new(StringArray::from(vec![Some("x"), None, Some("z")])) as ArrayRef,
                ),
            ]));
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("lst", list.data_type().clone(), true),
                    Field::new("st", structure.data_type().clone(), true),
                ])),
                vec![
                    Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
                    list,
                    structure,
                ],
            )
            .unwrap()
        }

        fn right_batch() -> RecordBatch {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("rid", DataType::Int64, false),
                    Field::new("name", DataType::Utf8, true),
                    Field::new("value", DataType::Float64, true),
                    Field::new("rname", DataType::Utf8, true),
                ])),
                vec![
                    Arc::new(Int64Array::from(vec![1, 2, 3])),
                    Arc::new(StringArray::from(vec![Some("a"), Some("b"), None])),
                    Arc::new(Float64Array::from(vec![Some(10.0), Some(20.0), None])),
                    Arc::new(StringArray::from(vec![Some("x"), Some("y"), None])),
                ],
            )
            .unwrap()
        }

        fn geo_input(batch: &RecordBatch) -> DataContract {
            DataContract::new(
                batch.schema(),
                vec![geometry(1, "geom", true)],
                Some(FieldId(1)),
                ContractProperties::default(),
            )
            .unwrap()
        }

        #[allow(clippy::needless_pass_by_value)]
        fn check_unary(
            op: &str,
            batch: &RecordBatch,
            config: Value,
            kernel: impl FnOnce(&RecordBatch) -> Result<RecordBatch>,
        ) -> DataContract {
            let expected = kernel(batch).unwrap_or_else(|e| panic!("kernel {op}: {e}"));
            let input = geo_input(batch);
            let analyzed = analyze_table_contract(op, &[input], &config, &mut FieldAllocator::default())
                .unwrap_or_else(|e| panic!("analyze {op}: {e}"));
            assert_eq!(
                signature(&analyzed.schema),
                signature(&expected.schema()),
                "schema diverso per {op}"
            );
            analyzed
        }

        #[allow(clippy::needless_pass_by_value)]
        fn check_unary_plain(
            op: &str,
            batch: &RecordBatch,
            config: Value,
            kernel: impl FnOnce(&RecordBatch) -> Result<RecordBatch>,
        ) {
            let expected = kernel(batch).unwrap_or_else(|e| panic!("kernel {op}: {e}"));
            let input = DataContract::tabular(batch.schema());
            let analyzed = analyze_table_contract(op, &[input], &config, &mut FieldAllocator::default())
                .unwrap_or_else(|e| panic!("analyze {op}: {e}"));
            assert_eq!(
                signature(&analyzed.schema),
                signature(&expected.schema()),
                "schema diverso per {op}"
            );
        }

        #[allow(clippy::needless_pass_by_value)]
        fn check_binary(
            op: &str,
            left: &RecordBatch,
            right: &RecordBatch,
            config: Value,
            kernel: impl FnOnce(&RecordBatch, &RecordBatch) -> Result<RecordBatch>,
        ) {
            let expected = kernel(left, right).unwrap_or_else(|e| panic!("kernel {op}: {e}"));
            let inputs = [geo_input(left), DataContract::tabular(right.schema())];
            let analyzed =
                analyze_table_contract(op, &inputs, &config, &mut FieldAllocator::default())
                    .unwrap_or_else(|e| panic!("analyze {op}: {e}"));
            assert_eq!(
                signature(&analyzed.schema),
                signature(&expected.schema()),
                "schema diverso per {op}"
            );
        }

        fn cfg<T: serde::de::DeserializeOwned>(config: &Value) -> T {
            serde_json::from_value(config.clone()).unwrap()
        }

        #[test]
        // Un cross-check per op: lunghezza intrinseca.
        #[allow(clippy::too_many_lines)]
        fn unary_ops_match_kernel_schemas() {
            let batch = simple_batch();
            let limits = Limits::default();

            check_unary("table.add_row_number", &batch, json!({}), |b| {
                utility::add_row_number(b, &cfg(&json!({})))
            });
            check_unary("table.sort", &batch, json!({"columns": ["id"]}), |b| {
                aggregation::sort(b, &cfg(&json!({"columns": ["id"]})))
            });
            check_unary("table.distinct", &batch, json!({}), |b| {
                aggregation::distinct(b, &cfg(&json!({})))
            });
            check_unary(
                "table.dedup_advanced",
                &batch,
                json!({"subset": ["name"]}),
                |b| aggregation::dedup_advanced(b, &cfg(&json!({"subset": ["name"]}))),
            );
            let filtered = check_unary(
                "table.filter",
                &batch,
                json!({"column": "value", "operator": ">", "value": 2}),
                |b| filtering::filter(b, &cfg(&json!({"column": "value", "operator": ">", "value": 2}))),
            );
            assert_eq!(filtered.geometries.len(), 1);
            check_unary("table.sample", &batch, json!({"n": 2}), |b| {
                analysis::sample(b, &cfg(&json!({"n": 2})))
            });
            check_unary(
                "table.rename",
                &batch,
                json!({"renames": [{"old_name": "name", "new_name": "label"}]}),
                |b| {
                    columns::rename(
                        b,
                        &cfg(&json!({"renames": [{"old_name": "name", "new_name": "label"}]})),
                    )
                },
            );
            check_unary("table.drop_columns", &batch, json!({"columns": ["flag"]}), |b| {
                columns::drop_columns(b, &cfg(&json!({"columns": ["flag"]})))
            });
            check_unary(
                "table.reorder_columns",
                &batch,
                json!({"columns": ["name"]}),
                |b| columns::reorder_columns(b, &cfg(&json!({"columns": ["name"]}))),
            );
            check_unary("table.concat_columns", &batch, json!({"columns": ["name"]}), |b| {
                columns::concat_columns(b, &cfg(&json!({"columns": ["name"]})), &limits)
            });
            check_unary(
                "table.split_column",
                &batch,
                json!({"column": "name", "new_columns": ["first"]}),
                |b| {
                    columns::split_column(
                        b,
                        &cfg(&json!({"column": "name", "new_columns": ["first"]})),
                        &limits,
                    )
                },
            );
            check_unary(
                "table.fill_na",
                &batch,
                json!({"column": "name", "value": "?"}),
                |b| cleansing::fill_na(b, &cfg(&json!({"column": "name", "value": "?"}))),
            );
            check_unary(
                "table.replace",
                &batch,
                json!({"column": "name", "old_value": "a", "new_value": "z"}),
                |b| {
                    cleansing::replace(
                        b,
                        &cfg(&json!({"column": "name", "old_value": "a", "new_value": "z"})),
                    )
                },
            );
            check_unary(
                "table.type_cast",
                &batch,
                json!({"column": "id", "target_type": "str"}),
                |b| cleansing::type_cast(b, &cfg(&json!({"column": "id", "target_type": "str"}))),
            );
            check_unary(
                "table.conditional",
                &batch,
                json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": 1}], "default_value": 0}),
                |b| {
                    filtering::conditional(
                        b,
                        &cfg(&json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": 1}], "default_value": 0})),
                    )
                },
            );
            check_unary(
                "table.conditional",
                &batch,
                json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": "hi"}], "default_value": "lo"}),
                |b| {
                    filtering::conditional(
                        b,
                        &cfg(&json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": "hi"}], "default_value": "lo"})),
                    )
                },
            );
            check_unary(
                "table.lookup",
                &batch,
                json!({"column": "name", "mapping": {"a": "A"}}),
                |b| analysis::lookup(b, &cfg(&json!({"column": "name", "mapping": {"a": "A"}}))),
            );
            check_unary("table.bin", &batch, json!({"column": "value", "bins": 2}), |b| {
                analysis::bin(b, &cfg(&json!({"column": "value", "bins": 2})))
            });
            check_unary("table.statistics", &batch, json!({"column": "value"}), |b| {
                analysis::statistics(b, &cfg(&json!({"column": "value"})))
            });
            check_unary(
                "table.date_extract",
                &batch,
                json!({"column": "name", "parts": ["year", "month"]}),
                |b| {
                    utility::date_extract(
                        b,
                        &cfg(&json!({"column": "name", "parts": ["year", "month"]})),
                    )
                },
            );
            check_unary("table.string_pad", &batch, json!({"column": "name"}), |b| {
                strings::string_pad(b, &cfg(&json!({"column": "name"})), &limits)
            });
            check_unary("table.string_length", &batch, json!({"column": "name"}), |b| {
                strings::string_length(b, &cfg(&json!({"column": "name"})))
            });
            check_unary(
                "table.string_extract",
                &batch,
                json!({"column": "name", "pattern": "(?P<x>a)"}),
                |b| {
                    strings::string_extract(
                        b,
                        &cfg(&json!({"column": "name", "pattern": "(?P<x>a)"})),
                        &limits,
                    )
                },
            );
            check_unary("table.text_normalize", &batch, json!({"columns": ["name"]}), |b| {
                strings::text_normalize(b, &cfg(&json!({"columns": ["name"]})), &limits)
            });
            check_unary("table.md5_hash", &batch, json!({"columns": ["name"]}), |b| {
                security::md5_hash(b, &cfg(&json!({"columns": ["name"]})))
            });
            check_unary("table.sha256_hash", &batch, json!({"columns": ["name"]}), |b| {
                security::sha256_hash(b, &cfg(&json!({"columns": ["name"]})))
            });
            check_unary(
                "table.mask_data",
                &batch,
                json!({"maskings": [{"column": "name"}]}),
                |b| security::mask_data(b, &cfg(&json!({"maskings": [{"column": "name"}]}))),
            );
            check_unary("table.uuid_generator", &batch, json!({}), |b| {
                utility::uuid_generator(b, &cfg(&json!({})))
            });
            check_unary(
                "table.date_format",
                &batch,
                json!({"column": "name", "input_format": "%Y", "output_column": "df"}),
                |b| {
                    dates::date_format(
                        b,
                        &cfg(&json!({"column": "name", "input_format": "%Y", "output_column": "df"})),
                    )
                },
            );
            check_unary(
                "table.date_add",
                &batch,
                json!({"column": "name", "input_format": "%Y", "amount": 1, "unit": "days", "output_column": "da"}),
                |b| {
                    dates::date_add(
                        b,
                        &cfg(&json!({"column": "name", "input_format": "%Y", "amount": 1, "unit": "days", "output_column": "da"})),
                    )
                },
            );
            check_unary(
                "table.date_diff",
                &batch,
                json!({"start_column": "name", "end_column": "name", "input_format": "%Y", "unit": "days", "output_column": "dd"}),
                |b| {
                    dates::date_diff(
                        b,
                        &cfg(&json!({"start_column": "name", "end_column": "name", "input_format": "%Y", "unit": "days", "output_column": "dd"})),
                    )
                },
            );
            check_unary(
                "table.timezone_convert",
                &batch,
                json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Europe/Rome", "output_column": "tc"}),
                |b| {
                    dates::timezone_convert(
                        b,
                        &cfg(&json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Europe/Rome", "output_column": "tc"})),
                    )
                },
            );
            check_unary(
                "table.aggregate",
                &batch,
                json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "sum"}, {"column": "id", "function": "count"}]}),
                |b| {
                    aggregation::aggregate(
                        b,
                        &cfg(&json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "sum"}, {"column": "id", "function": "count"}]})),
                    )
                },
            );
            check_unary(
                "table.rolling_window",
                &batch,
                json!({"column": "value", "function": "sum", "window": 2, "output_column": "rw"}),
                |b| {
                    aggregation::rolling_window(
                        b,
                        &cfg(&json!({"column": "value", "function": "sum", "window": 2, "output_column": "rw"})),
                    )
                },
            );
            check_unary(
                "table.window_function",
                &batch,
                json!({"column": "value", "function": "rank"}),
                |b| aggregation::window_function(b, &cfg(&json!({"column": "value", "function": "rank"}))),
            );
            check_unary(
                "table.coalesce",
                &batch,
                json!({"columns": ["value"], "output_column": "c"}),
                |b| quality::coalesce(b, &cfg(&json!({"columns": ["value"], "output_column": "c"}))),
            );
            check_unary(
                "table.formula",
                &batch,
                json!({"new_column": "f", "formula": "value * 2"}),
                |b| formula::formula(b, &cfg(&json!({"new_column": "f", "formula": "value * 2"}))),
            );
            check_unary(
                "table.formula",
                &batch,
                json!({"new_column": "f", "formula": "name + 'x'"}),
                |b| formula::formula(b, &cfg(&json!({"new_column": "f", "formula": "name + 'x'"}))),
            );
            check_unary(
                "table.expression",
                &batch,
                json!({"output_column": "e", "expression": {"kind": "binary", "op": "add", "left": {"kind": "column", "name": "value"}, "right": {"kind": "literal", "value": 1}}}),
                |b| {
                    expressions::expression(
                        b,
                        &cfg(&json!({"output_column": "e", "expression": {"kind": "binary", "op": "add", "left": {"kind": "column", "name": "value"}, "right": {"kind": "literal", "value": 1}}})),
                    )
                },
            );
            check_unary(
                "table.expression",
                &batch,
                json!({"output_column": "e", "expression": {"kind": "column", "name": "name"}, "output_type": "text"}),
                |b| {
                    expressions::expression(
                        b,
                        &cfg(&json!({"output_column": "e", "expression": {"kind": "column", "name": "name"}, "output_type": "text"})),
                    )
                },
            );
            check_unary(
                "table.flatten_json",
                &batch,
                json!({"column": "name", "output_columns": ["name_a"]}),
                |b| {
                    analysis::flatten_json(
                        b,
                        &cfg(&json!({"column": "name", "output_columns": ["name_a"]})),
                        &limits,
                    )
                },
            );
            check_unary(
                "table.melt",
                &batch,
                json!({"id_columns": ["id"], "value_columns": ["value"]}),
                |b| {
                    reshape::melt(
                        b,
                        &cfg(&json!({"id_columns": ["id"], "value_columns": ["value"]})),
                        &limits,
                    )
                },
            );
            check_unary(
                "table.assert_schema",
                &batch,
                json!({"fields": [{"name": "id", "data_type": "int64", "nullable": false}], "allow_extra": true}),
                |b| {
                    quality::assert_schema(
                        b,
                        &cfg(&json!({"fields": [{"name": "id", "data_type": "int64", "nullable": false}], "allow_extra": true})),
                    )
                },
            );
            check_unary("table.assert_not_null", &batch, json!({"columns": ["id"]}), |b| {
                quality::assert_not_null(b, &cfg(&json!({"columns": ["id"]})))
            });
            check_unary("table.assert_unique", &batch, json!({"columns": ["id"]}), |b| {
                quality::assert_unique(b, &cfg(&json!({"columns": ["id"]})))
            });
            check_unary(
                "table.assert_range",
                &batch,
                json!({"column": "value", "min": 0, "allow_null": true}),
                |b| {
                    quality::assert_range(
                        b,
                        &cfg(&json!({"column": "value", "min": 0, "allow_null": true})),
                    )
                },
            );
            check_unary(
                "table.assert_regex",
                &batch,
                json!({"column": "name", "pattern": "^[ab]$", "allow_null": true}),
                |b| {
                    quality::assert_regex(
                        b,
                        &cfg(&json!({"column": "name", "pattern": "^[ab]$", "allow_null": true})),
                    )
                },
            );
            check_unary("table.assert_cardinality", &batch, json!({"min_rows": 1}), |b| {
                governance::assert_cardinality(b, &cfg(&json!({"min_rows": 1})))
            });
            check_unary("table.assert_metadata", &batch, json!({"expected": {}}), |b| {
                governance::assert_metadata(b, &cfg(&json!({"expected": {}})))
            });

            let nested = nested_batch();
            check_unary_plain("table.explode", &nested, json!({"column": "lst"}), |b| {
                reshape::explode(b, &cfg(&json!({"column": "lst"})), &limits)
            });
            check_unary_plain("table.unnest", &nested, json!({"column": "st"}), |b| {
                reshape::unnest(b, &cfg(&json!({"column": "st"})), &limits)
            });
        }

        #[test]
        #[allow(clippy::too_many_lines)]
        fn binary_ops_match_kernel_schemas() {
            let batch = simple_batch();
            let right = right_batch();
            let limits = Limits::default();

            check_binary(
                "table.join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    joins::join(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
            check_binary(
                "table.join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"], "how": "outer"}),
                |l, r| {
                    joins::join(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"], "how": "outer"})),
                        &limits,
                    )
                },
            );
            check_binary("table.cross_join", &batch, &right, json!({}), |l, r| {
                joins::cross_join(l, r, &cfg(&json!({})), &limits)
            });
            check_binary(
                "table.semi_join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| joins::semi_join(l, r, &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]}))),
            );
            check_binary(
                "table.anti_join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| joins::anti_join(l, r, &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]}))),
            );
            check_binary(
                "table.asof_join",
                &batch,
                &right,
                json!({"left_on": "id", "right_on": "rid"}),
                |l, r| {
                    joins::asof_join(
                        l,
                        r,
                        &cfg(&json!({"left_on": "id", "right_on": "rid"})),
                        &limits,
                    )
                },
            );
            check_binary("table.concat", &batch, &batch, json!({}), |l, r| {
                joins::concat(l, r, &cfg(&json!({})), &limits)
            });
            check_binary("table.union_distinct", &batch, &batch, json!({}), |l, r| {
                setops::union_distinct(l, r, &cfg(&json!({})), &limits)
            });
            check_binary("table.intersect", &batch, &batch, json!({}), |l, r| {
                setops::intersect(l, r, &cfg(&json!({})))
            });
            check_binary("table.except", &batch, &batch, json!({}), |l, r| {
                setops::except(l, r, &cfg(&json!({})))
            });
            check_binary(
                "table.table_diff",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    reshape::table_diff(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
            check_binary(
                "table.reconcile",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    governance::reconcile(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
            check_binary(
                "table.assert_foreign_key",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    governance::assert_foreign_key(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
        }
    }
}
