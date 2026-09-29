//! Dispatch delle operazioni tabellari: la config tipizzata una volta in
//! validazione, il kernel chiamato con quella in esecuzione.
//!
//! Porting di `prepare_step`, `execute_step` ed `execute_binary` di
//! `plenora-engine/src/table_engine/executor.rs` a `190c493`, con la forma
//! allineata all'API corrente dei kernel di
//! `plenora-memory-lab/operations/table_catalog/src/dispatch.rs`. Il dispatch
//! e' per id canonico del catalogo: nessun alias, nessun nome legacy.

use plenora_core::arrow::array::RecordBatch;
use plenora_core::{PlenoraError, Result};
use plenora_kernels_table::{
    aggregation, analysis, cleansing, columns, dates, expressions, filtering, formula, fuzzy,
    governance, joins, quality, reshape, security, setops, spill, strings, utility, Limits,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Config tipizzata di un passo tabellare, una variante per operazione.
///
/// Le varianti sono `Box`ate perche' le config hanno dimensioni molto
/// diverse.
#[derive(Debug)]
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

/// Variante del kernel scelta dal runner per un passo.
///
/// La sceglie il budget ([`crate::esecuzione`]): la variante spilled solo
/// dove il kernel la ha (`sort`, `distinct`, `aggregate`, set operation) e
/// solo quando quella in memoria non sta nel budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variante {
    /// Il kernel in memoria.
    InMemoria,
    /// Il kernel con spill su disco.
    Spill,
}

impl PassoPreparato {
    /// `true` se il passo ha una variante spilled.
    pub const fn ha_spill(&self) -> bool {
        matches!(self, Self::Sort(_) | Self::Distinct(_) | Self::Aggregate(_))
            || self.tipo_set_operation().is_some()
    }

    /// Operazione di set per `spill::execute_set_operation`.
    const fn tipo_set_operation(&self) -> Option<setops::SetOperationKind> {
        match self {
            Self::UnionDistinct(_) => Some(setops::SetOperationKind::UnionDistinct),
            Self::Intersect(_) => Some(setops::SetOperationKind::Intersect),
            Self::Except(_) => Some(setops::SetOperationKind::Except),
            _ => None,
        }
    }

    /// Esegue un passo unario con la config già tipizzata.
    ///
    /// # Errors
    ///
    /// Gli errori del kernel; `Internal` se il passo non è unario (la
    /// validazione lo esclude).
    #[allow(clippy::too_many_lines)] // Un braccio per operazione, in un solo match verificabile.
    pub fn esegui_unario(
        &self,
        batch: &RecordBatch,
        limits: &Limits,
        variante: Variante,
    ) -> Result<RecordBatch> {
        let spill = variante == Variante::Spill;
        if spill && !self.ha_spill() {
            return Err(PlenoraError::Internal(
                "variante spilled chiesta per un passo che non la ha".to_owned(),
            ));
        }
        match self {
            Self::DropColumns(config) => columns::drop_columns(batch, config),
            Self::Rename(config) => columns::rename(batch, config),
            Self::ReorderColumns(config) => columns::reorder_columns(batch, config),
            Self::SelectColumns(config) => columns::select_columns(batch, config),
            Self::AlignSchema(config) => columns::align_schema(batch, config),
            Self::ConcatColumns(config) => columns::concat_columns(batch, config, limits),
            Self::SplitColumn(config) => columns::split_column(batch, config, limits),
            Self::StringPad(config) => strings::string_pad(batch, config, limits),
            Self::StringLength(config) => strings::string_length(batch, config),
            Self::TextNormalize(config) => strings::text_normalize(batch, config, limits),
            Self::FillNa(config) => cleansing::fill_na(batch, config),
            Self::Replace(config) => cleansing::replace(batch, config),
            Self::TypeCast(config) => cleansing::type_cast(batch, config),
            Self::Filter(config) => filtering::filter(batch, config),
            Self::Conditional(config) => filtering::conditional(batch, config),
            Self::StringExtract(config) => strings::string_extract(batch, config, limits),
            Self::DateExtract(config) => utility::date_extract(batch, config),
            Self::UuidGenerator(config) => utility::uuid_generator(batch, config),
            Self::Limit(config) => utility::limit(batch, config),
            Self::Lookup(config) => analysis::lookup(batch, config),
            Self::FlattenJson(config) => analysis::flatten_json(batch, config, limits),
            Self::MaskData(config) => security::mask_data(batch, config),
            Self::Md5Hash(config) => security::md5_hash(batch, config),
            Self::AddRowNumber(config) => utility::add_row_number(batch, config),
            Self::Bin(config) => analysis::bin(batch, config),
            Self::Sample(config) => analysis::sample(batch, config),
            Self::Statistics(config) => analysis::statistics(batch, config),
            Self::Sort(config) if spill => {
                let mut area = spill::RowSpillWorkspace::new(limits.max_temp_bytes)?;
                spill::sort_spilled_in(batch, config, limits, &mut area).map(|(uscita, _)| uscita)
            }
            Self::Sort(config) => aggregation::sort(batch, config),
            Self::TopN(config) => aggregation::top_n(batch, config),
            Self::Distinct(config) if spill => {
                let mut area = spill::RowSpillWorkspace::new(limits.max_temp_bytes)?;
                spill::distinct_spilled_in(batch, config, limits, &mut area)
                    .map(|(uscita, _)| uscita)
            }
            Self::Distinct(config) => aggregation::distinct(batch, config),
            Self::DedupAdvanced(config) => aggregation::dedup_advanced(batch, config),
            Self::Aggregate(config) if spill => {
                let mut area = spill::RowSpillWorkspace::new(limits.max_temp_bytes)?;
                spill::aggregate_spilled_in(batch, config, limits, &mut area)
                    .map(|(uscita, _)| uscita)
            }
            Self::Aggregate(config) => aggregation::aggregate(batch, config),
            Self::WindowFunction(config) => aggregation::window_function(batch, config),
            Self::RollingWindow(config) => aggregation::rolling_window(batch, config),
            Self::Melt(config) => reshape::melt(batch, config, limits),
            Self::Pivot(config) => reshape::pivot(batch, config, limits),
            Self::Transpose(config) => reshape::transpose(batch, config, limits),
            Self::Formula(config) => formula::formula(batch, config),
            Self::Expression(config) => expressions::expression(batch, config),
            Self::AssertCardinality(config) => governance::assert_cardinality(batch, config),
            Self::AssertMetadata(config) => governance::assert_metadata(batch, config),
            Self::AssertSchema(config) => quality::assert_schema(batch, config),
            Self::AssertNotNull(config) => quality::assert_not_null(batch, config),
            Self::AssertUnique(config) => quality::assert_unique(batch, config),
            Self::AssertRange(config) => quality::assert_range(batch, config),
            Self::AssertRegex(config) => quality::assert_regex(batch, config),
            Self::Coalesce(config) => quality::coalesce(batch, config),
            Self::DateFormat(config) => dates::date_format(batch, config),
            Self::DateAdd(config) => dates::date_add(batch, config),
            Self::DateDiff(config) => dates::date_diff(batch, config),
            Self::TimezoneConvert(config) => dates::timezone_convert(batch, config),
            Self::Sha256Hash(config) => security::sha256_hash(batch, config),
            Self::StableFingerprint(config) => security::stable_fingerprint(batch, config),
            Self::HmacSha256(config) => security::hmac_sha256(batch, config),
            Self::ValidateRules(config) => governance::validate_rules(batch, config),
            Self::Explode(config) => reshape::explode(batch, config, limits),
            Self::Unnest(config) => reshape::unnest(batch, config, limits),
            Self::Join(_)
            | Self::Concat(_)
            | Self::ConcatByName(_)
            | Self::CrossJoin(_)
            | Self::TableDiff(_)
            | Self::SemiJoin(_)
            | Self::AntiJoin(_)
            | Self::AsOfJoin(_)
            | Self::FuzzyJoin(_)
            | Self::UnionDistinct(_)
            | Self::Intersect(_)
            | Self::Except(_)
            | Self::AssertForeignKey(_)
            | Self::Reconcile(_) => Err(PlenoraError::Internal(
                "passo binario sul percorso unario".to_owned(),
            )),
        }
    }

    /// Esegue un passo a due input (left, right) con la config già tipizzata.
    ///
    /// # Errors
    ///
    /// Gli errori del kernel; `Internal` se il passo non è binario (la
    /// validazione lo esclude).
    pub fn esegui_binario(
        &self,
        left: &RecordBatch,
        right: &RecordBatch,
        limits: &Limits,
        variante: Variante,
    ) -> Result<RecordBatch> {
        if variante == Variante::Spill {
            let Some(tipo) = self.tipo_set_operation() else {
                return Err(PlenoraError::Internal(
                    "variante spilled chiesta per un passo che non la ha".to_owned(),
                ));
            };
            return spill::execute_set_operation(tipo, left, right, limits);
        }
        match self {
            Self::Join(config) => joins::join(left, right, config, limits),
            Self::Concat(config) => joins::concat(left, right, config, limits),
            Self::ConcatByName(config) => joins::concat_by_name(&[left, right], config, limits),
            Self::CrossJoin(config) => joins::cross_join(left, right, config, limits),
            Self::TableDiff(config) => reshape::table_diff(left, right, config, limits),
            Self::SemiJoin(config) => joins::semi_join(left, right, config),
            Self::AntiJoin(config) => joins::anti_join(left, right, config),
            Self::AsOfJoin(config) => joins::asof_join(left, right, config, limits),
            Self::FuzzyJoin(config) => fuzzy::fuzzy_join(left, right, config, limits),
            Self::UnionDistinct(config) => setops::union_distinct(left, right, config, limits),
            Self::Intersect(config) => setops::intersect(left, right, config),
            Self::Except(config) => setops::except(left, right, config),
            Self::AssertForeignKey(config) => {
                governance::assert_foreign_key(left, right, config, limits)
            }
            Self::Reconcile(config) => governance::reconcile(left, right, config, limits),
            _ => Err(PlenoraError::Internal(
                "passo unario sul percorso binario".to_owned(),
            )),
        }
    }
}
