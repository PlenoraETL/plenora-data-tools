//! Dispatch delle operazioni tabellari: la config tipizzata una volta in
//! validazione, il kernel chiamato con quella in esecuzione.
//!
//! Porting di `prepare_step`, `execute_step` ed `execute_binary` di
//! `plenora-engine/src/table_engine/executor.rs` a `190c493`, con la forma
//! allineata all'API corrente dei kernel di
//! `plenora-memory-lab/operations/table_catalog/src/dispatch.rs`. Il dispatch
//! e' per id canonico del catalogo: nessun alias, nessun nome legacy.

use plenora_core::{PlenoraError, Result};
use plenora_kernels_table::{
    aggregation, analysis, cleansing, columns, dates, expressions, filtering, formula, fuzzy,
    governance, joins, quality, reshape, security, setops, strings, utility,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Config tipizzata di un passo tabellare, una variante per operazione.
///
/// Le varianti sono `Box`ate perche' le config hanno dimensioni molto
/// diverse.
#[derive(Debug)]
// F1: le config preparate si leggono solo dall'esecuzione, che arriva in F2.
#[allow(dead_code)]
pub enum PassoPreparato {
    DropColumns(Box<columns::DropColumns>),
    Rename(Box<columns::Rename>),
    ReorderColumns(Box<columns::ReorderColumns>),
    SelectColumns(Box<columns::SelectColumns>),
    AlignSchema(Box<columns::AlignSchema>),
    ConcatColumns(Box<columns::ConcatColumns>),
    SplitColumn(Box<columns::SplitColumn>),
    StringPad(Box<strings::StringPad>),
    StringLength(Box<strings::StringLength>),
    TextNormalize(Box<strings::TextNormalize>),
    FillNa(Box<cleansing::FillNa>),
    Replace(Box<cleansing::Replace>),
    TypeCast(Box<cleansing::TypeCast>),
    Filter(Box<filtering::Filter>),
    Conditional(Box<filtering::Conditional>),
    StringExtract(Box<strings::StringExtract>),
    DateExtract(Box<utility::DateExtract>),
    UuidGenerator(Box<utility::UuidGenerator>),
    Limit(Box<utility::Limit>),
    Lookup(Box<analysis::Lookup>),
    FlattenJson(Box<analysis::FlattenJson>),
    MaskData(Box<security::MaskData>),
    Md5Hash(Box<security::Md5Hash>),
    AddRowNumber(Box<utility::AddRowNumber>),
    Bin(Box<analysis::Bin>),
    Sample(Box<analysis::Sample>),
    Statistics(Box<analysis::Statistics>),
    Sort(Box<aggregation::Sort>),
    TopN(Box<aggregation::TopN>),
    Distinct(Box<aggregation::Distinct>),
    DedupAdvanced(Box<aggregation::DedupAdvanced>),
    Aggregate(Box<aggregation::Aggregate>),
    WindowFunction(Box<aggregation::WindowFunction>),
    RollingWindow(Box<aggregation::RollingWindow>),
    Melt(Box<reshape::Melt>),
    Pivot(Box<reshape::Pivot>),
    Transpose(Box<reshape::Transpose>),
    Formula(Box<formula::Formula>),
    Expression(Box<expressions::ExpressionTransform>),
    AssertCardinality(Box<governance::AssertCardinality>),
    AssertMetadata(Box<governance::AssertMetadata>),
    AssertSchema(Box<quality::AssertSchema>),
    AssertNotNull(Box<quality::AssertNotNull>),
    AssertUnique(Box<quality::AssertUnique>),
    AssertRange(Box<quality::AssertRange>),
    AssertRegex(Box<quality::AssertRegex>),
    Coalesce(Box<quality::Coalesce>),
    DateFormat(Box<dates::DateFormat>),
    DateAdd(Box<dates::DateAdd>),
    DateDiff(Box<dates::DateDiff>),
    TimezoneConvert(Box<dates::TimezoneConvert>),
    Sha256Hash(Box<security::Sha256Hash>),
    StableFingerprint(Box<security::StableFingerprint>),
    HmacSha256(Box<security::HmacSha256>),
    ValidateRules(Box<governance::ValidateRules>),
    Explode(Box<reshape::Explode>),
    Unnest(Box<reshape::Unnest>),
    Join(Box<joins::Join>),
    Concat(Box<joins::Concat>),
    ConcatByName(Box<joins::ConcatByName>),
    CrossJoin(Box<joins::CrossJoin>),
    TableDiff(Box<reshape::TableDiff>),
    SemiJoin(Box<joins::MembershipJoin>),
    AntiJoin(Box<joins::MembershipJoin>),
    AsOfJoin(Box<joins::AsOfJoin>),
    FuzzyJoin(Box<fuzzy::FuzzyJoin>),
    UnionDistinct(Box<setops::SetOperation>),
    Intersect(Box<setops::SetOperation>),
    Except(Box<setops::SetOperation>),
    AssertForeignKey(Box<governance::ForeignKey>),
    Reconcile(Box<governance::Reconcile>),
}

fn decodifica<T: DeserializeOwned>(config: &Value) -> Result<T> {
    // La config e' gia' passata dall'analisi con lo stesso tipo: un rifiuto
    // qui e' comunque un errore di piano, mai un default.
    T::deserialize(config)
        .map_err(|errore| PlenoraError::InvalidPlan(format!("config non valida: {errore}")))
}

impl PassoPreparato {
    /// Deserializza la config del passo nella forma tipizzata.
    ///
    /// # Errors
    ///
    /// - `InvalidPlan`: config non deserializzabile nel tipo dell'operazione;
    /// - `Unsupported`: operazione senza dispatch nel runner.
    #[allow(clippy::too_many_lines)] // Un braccio per operazione, in un solo match verificabile.
    pub fn prepara(op: &str, config: &Value) -> Result<Self> {
        Ok(match op {
            "table.drop_columns" => Self::DropColumns(Box::new(decodifica(config)?)),
            "table.rename" => Self::Rename(Box::new(decodifica(config)?)),
            "table.reorder_columns" => Self::ReorderColumns(Box::new(decodifica(config)?)),
            "table.select_columns" => Self::SelectColumns(Box::new(decodifica(config)?)),
            "table.align_schema" => Self::AlignSchema(Box::new(decodifica(config)?)),
            "table.concat_columns" => Self::ConcatColumns(Box::new(decodifica(config)?)),
            "table.split_column" => Self::SplitColumn(Box::new(decodifica(config)?)),
            "table.string_pad" => Self::StringPad(Box::new(decodifica(config)?)),
            "table.string_length" => Self::StringLength(Box::new(decodifica(config)?)),
            "table.text_normalize" => Self::TextNormalize(Box::new(decodifica(config)?)),
            "table.fill_na" => Self::FillNa(Box::new(decodifica(config)?)),
            "table.replace" => Self::Replace(Box::new(decodifica(config)?)),
            "table.type_cast" => Self::TypeCast(Box::new(decodifica(config)?)),
            "table.filter" => Self::Filter(Box::new(decodifica(config)?)),
            "table.conditional" => Self::Conditional(Box::new(decodifica(config)?)),
            "table.string_extract" => Self::StringExtract(Box::new(decodifica(config)?)),
            "table.date_extract" => Self::DateExtract(Box::new(decodifica(config)?)),
            "table.uuid_generator" => Self::UuidGenerator(Box::new(decodifica(config)?)),
            "table.limit" => Self::Limit(Box::new(decodifica(config)?)),
            "table.lookup" => Self::Lookup(Box::new(decodifica(config)?)),
            "table.flatten_json" => Self::FlattenJson(Box::new(decodifica(config)?)),
            "table.mask_data" => Self::MaskData(Box::new(decodifica(config)?)),
            "table.md5_hash" => Self::Md5Hash(Box::new(decodifica(config)?)),
            "table.add_row_number" => Self::AddRowNumber(Box::new(decodifica(config)?)),
            "table.bin" => Self::Bin(Box::new(decodifica(config)?)),
            "table.sample" => Self::Sample(Box::new(decodifica(config)?)),
            "table.statistics" => Self::Statistics(Box::new(decodifica(config)?)),
            "table.sort" => Self::Sort(Box::new(decodifica(config)?)),
            "table.top_n" => Self::TopN(Box::new(decodifica(config)?)),
            "table.distinct" => Self::Distinct(Box::new(decodifica(config)?)),
            "table.dedup_advanced" => Self::DedupAdvanced(Box::new(decodifica(config)?)),
            "table.aggregate" => Self::Aggregate(Box::new(decodifica(config)?)),
            "table.window_function" => Self::WindowFunction(Box::new(decodifica(config)?)),
            "table.rolling_window" => Self::RollingWindow(Box::new(decodifica(config)?)),
            "table.melt" => Self::Melt(Box::new(decodifica(config)?)),
            "table.pivot" => Self::Pivot(Box::new(decodifica(config)?)),
            "table.transpose" => Self::Transpose(Box::new(decodifica(config)?)),
            "table.formula" => Self::Formula(Box::new(decodifica(config)?)),
            "table.expression" => Self::Expression(Box::new(decodifica(config)?)),
            "table.assert_cardinality" => Self::AssertCardinality(Box::new(decodifica(config)?)),
            "table.assert_metadata" => Self::AssertMetadata(Box::new(decodifica(config)?)),
            "table.assert_schema" => Self::AssertSchema(Box::new(decodifica(config)?)),
            "table.assert_not_null" => Self::AssertNotNull(Box::new(decodifica(config)?)),
            "table.assert_unique" => Self::AssertUnique(Box::new(decodifica(config)?)),
            "table.assert_range" => Self::AssertRange(Box::new(decodifica(config)?)),
            "table.assert_regex" => Self::AssertRegex(Box::new(decodifica(config)?)),
            "table.coalesce" => Self::Coalesce(Box::new(decodifica(config)?)),
            "table.date_format" => Self::DateFormat(Box::new(decodifica(config)?)),
            "table.date_add" => Self::DateAdd(Box::new(decodifica(config)?)),
            "table.date_diff" => Self::DateDiff(Box::new(decodifica(config)?)),
            "table.timezone_convert" => Self::TimezoneConvert(Box::new(decodifica(config)?)),
            "table.sha256_hash" => Self::Sha256Hash(Box::new(decodifica(config)?)),
            "table.stable_fingerprint" => Self::StableFingerprint(Box::new(decodifica(config)?)),
            "table.hmac_sha256" => Self::HmacSha256(Box::new(decodifica(config)?)),
            "table.validate_rules" => Self::ValidateRules(Box::new(decodifica(config)?)),
            "table.explode" => Self::Explode(Box::new(decodifica(config)?)),
            "table.unnest" => Self::Unnest(Box::new(decodifica(config)?)),
            "table.join" => Self::Join(Box::new(decodifica(config)?)),
            "table.concat" => Self::Concat(Box::new(decodifica(config)?)),
            "table.concat_by_name" => Self::ConcatByName(Box::new(decodifica(config)?)),
            "table.cross_join" => Self::CrossJoin(Box::new(decodifica(config)?)),
            "table.table_diff" => Self::TableDiff(Box::new(decodifica(config)?)),
            "table.semi_join" => Self::SemiJoin(Box::new(decodifica(config)?)),
            "table.anti_join" => Self::AntiJoin(Box::new(decodifica(config)?)),
            "table.asof_join" => Self::AsOfJoin(Box::new(decodifica(config)?)),
            "table.fuzzy_join" => Self::FuzzyJoin(Box::new(decodifica(config)?)),
            "table.union_distinct" => Self::UnionDistinct(Box::new(decodifica(config)?)),
            "table.intersect" => Self::Intersect(Box::new(decodifica(config)?)),
            "table.except" => Self::Except(Box::new(decodifica(config)?)),
            "table.assert_foreign_key" => Self::AssertForeignKey(Box::new(decodifica(config)?)),
            "table.reconcile" => Self::Reconcile(Box::new(decodifica(config)?)),
            altra => {
                return Err(PlenoraError::Unsupported(format!(
                    "{altra}: operazione senza dispatch nel runner"
                )))
            }
        })
    }
}
