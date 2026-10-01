//! Analyzer a secco di ordinamento, distinct e aggregazioni (kernel del
//! modulo `aggregation`).

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{ContractProperties, DataContract, FieldAllocator, FieldId};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use super::helpers::{
    analyze_append, check_name_list, check_output_name, check_rows, check_text_len, con_op,
    contract_error, field_of, finish, map_row_count, produce, propagate_geometry, proven_sorted,
    require_numeric, require_scalar_string, require_scalar_string_field, require_scalar_strings,
    sorted_only, typed,
};
use crate::{aggregation, Limits};

// ---------------------------------------------------------------------------
// aggregation
// ---------------------------------------------------------------------------

/// Le colonne di ordinamento esistono e hanno un confronto nativo.
///
/// Il runtime rifiuta i tipi senza confronto nativo (`validate_sortable`);
/// senza questo controllo il piano supererebbe la validazione e fallirebbe
/// solo in esecuzione, cioe' dopo aver aperto gli input e forse dopo aver
/// gia' prodotto lavoro. `is_sortable` e' lo stesso elenco di tipi del
/// comparatore, letto a secco dallo schema.
///
/// Vale per ogni operazione che passa una colonna della config al sort del
/// kernel: `sort` e `top_n` (`columns`), `dedup_advanced`, `rolling_window`
/// e `window_function` (`order_column`).
fn require_sortable(op: &str, input: &DataContract, columns: &[String]) -> Result<()> {
    for name in columns {
        let field = field_of(op, input, name)?;
        if !aggregation::is_sortable(field.data_type()) {
            return contract_error(
                op,
                format!(
                    "colonna {name} di tipo {:?} non ordinabile: nessun confronto nativo definito",
                    field.data_type()
                ),
            );
        }
    }
    Ok(())
}

pub(in crate::analyze) fn analyze_sort(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: aggregation::Sort = typed(op, config)?;
    let input = &inputs[0];
    check_name_list(op, &config.columns, limits.max_columns, "columns", false)?;
    require_sortable(op, input, &config.columns)?;
    let keys: Vec<FieldId> = config
        .columns
        .iter()
        .map(|name| fields.intern(name))
        .collect::<Result<_>>()?;
    let mut output = input.clone();
    // Sort blocking: l'intero stream di output e' ordinato sulle chiavi, nel
    // verso della config.
    output.properties = ContractProperties {
        sorted_by: Some(proven_sorted(keys, config.ascending)),
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}

pub(in crate::analyze) fn analyze_top_n(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: aggregation::TopN = typed(op, config)?;
    con_op(op, config.verifica_parametri())?;
    let input = &inputs[0];
    check_name_list(op, &config.columns, limits.max_columns, "columns", false)?;
    check_rows(op, config.n, limits.max_rows, "n")?;
    require_sortable(op, input, &config.columns)?;
    let keys: Vec<FieldId> = config
        .columns
        .iter()
        .map(|name| fields.intern(name))
        .collect::<Result<_>>()?;
    let mut output = input.clone();
    // Come sort, ma emesse esattamente min(n, righe) righe.
    output.properties = ContractProperties {
        sorted_by: Some(proven_sorted(keys, !config.descending)),
        row_count: map_row_count(input, |rows| rows.min(config.n)),
    };
    Ok(output)
}

pub(in crate::analyze) fn analyze_distinct(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: aggregation::Distinct = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    check_name_list(op, &config.subset, limits.max_columns, "subset", true)?;
    if config.subset.is_empty() {
        // Senza subset la chiave e' la riga intera, letta come testo.
        for field in input.schema.fields() {
            require_scalar_string_field(op, field)?;
        }
    } else {
        require_scalar_strings(op, input, &config.subset)?;
    }
    let mut output = input.clone();
    // Righe rimosse; l'ordine relativo delle occorrenze mantenute e' preservato.
    output.properties = sorted_only(input);
    Ok(output)
}

pub(in crate::analyze) fn analyze_dedup_advanced(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: aggregation::DedupAdvanced = typed(op, config)?;
    let input = &inputs[0];
    if matches!(config.keep, aggregation::Keep::False) {
        return contract_error(op, "keep=false non supportato");
    }
    check_name_list(op, &config.subset, limits.max_columns, "subset", false)?;
    con_op(op, aggregation::verifica_verso_dedup(&config))?;
    require_scalar_strings(op, input, &config.subset)?;
    let sorted_by = if let Some(order_column) = &config.order_column {
        require_sortable(op, input, std::slice::from_ref(order_column))?;
        // Sort interno su order_column, nel verso della config, prima della
        // deduplica; le righe tenute restano nell'ordine del sort.
        Some(proven_sorted(
            vec![fields.intern(order_column)?],
            config.ascending.unwrap_or(true),
        ))
    } else {
        input.properties.sorted_by.clone()
    };
    let mut output = input.clone();
    output.properties = ContractProperties {
        sorted_by,
        row_count: None,
    };
    Ok(output)
}

/// Parametri di una aggregazione indipendenti dallo schema: `separator` entro
/// `max_string_bytes`, e nessun parametro scritto che la funzione non usa
/// (`Aggregation::verifica_parametri`, la stessa regola del kernel).
fn check_aggregation_parameters(
    op: &str,
    aggregation: &aggregation::Aggregation,
    limits: &Limits,
) -> Result<()> {
    check_text_len(
        op,
        aggregation.separator(),
        limits.max_string_bytes,
        "separator",
    )?;
    con_op(op, aggregation.verifica_parametri())
}

pub(in crate::analyze) fn analyze_aggregate(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: aggregation::Aggregate = typed(op, config)?;
    let input = &inputs[0];
    if config.group_by.is_empty() {
        return contract_error(op, "aggregate richiede group_by");
    }
    check_name_list(op, &config.group_by, limits.max_columns, "group_by", false)?;
    if config.aggregations.len() > limits.max_columns {
        return contract_error(op, "aggregations oltre il limite di colonne");
    }
    // Il kernel legge le chiavi di gruppo come scalari testuali: stessi tipi
    // ammessi, rifiutati qui e non a esecuzione iniziata.
    let mut fields_out: Vec<Field> = config
        .group_by
        .iter()
        .map(|name| {
            let field = field_of(op, input, name)?;
            require_scalar_string_field(op, field)?;
            Ok(field.clone())
        })
        .collect::<Result<_>>()?;
    // Prima i parametri di ogni aggregazione, poi i nomi d'uscita: lo stesso
    // ordine del kernel.
    for aggregation in &config.aggregations {
        check_aggregation_parameters(op, aggregation, limits)?;
    }
    let nomi = con_op(op, config.nomi_uscita())?;
    for (aggregation, name) in config.aggregations.iter().zip(nomi) {
        let field = field_of(op, input, &aggregation.column)?;
        match aggregation.function {
            aggregation::AggFunction::Count => {}
            aggregation::AggFunction::Nunique | aggregation::AggFunction::Concat => {
                require_scalar_string_field(op, field)?;
            }
            // La cella com'e' (`take`): il tipo del profilo, senza il fuso,
            // con la stessa funzione del kernel.
            aggregation::AggFunction::First | aggregation::AggFunction::Last => {
                crate::validate_cella_prendibile(field.data_type(), field.name())
                    .map_err(|errore| PlenoraError::InvalidPlan(format!("{op}: {errore}")))?;
            }
            aggregation::AggFunction::Quantile => {
                if aggregation.quantile.is_none() {
                    return contract_error(op, "quantile richiede il parametro quantile");
                }
                // Come il kernel (`aggregate`): il range e' parte del
                // contratto; fuori [0, 1] l'indice nel gruppo ordinato
                // uscirebbe dai limiti, quindi si rifiuta in validazione.
                if aggregation
                    .quantile
                    .is_some_and(|quantile| !(0.0..=1.0).contains(&quantile))
                {
                    return contract_error(op, "quantile fuori dall'intervallo 0..=1");
                }
                require_numeric(op, input, &aggregation.column)?;
            }
            aggregation::AggFunction::Sum => {
                require_numeric(op, input, &aggregation.column)?;
                con_op(op, crate::float64_source::verifica_somma(field.data_type()))?;
            }
            _ => require_numeric(op, input, &aggregation.column)?,
        }
        // Il tipo del kernel (`tipo_uscita`): somme intere `Int64`, estremi
        // esatti e `first`/`last` nel tipo d'ingresso.
        let data_type = aggregation::tipo_uscita(aggregation.function, field.data_type());
        let nullable = !matches!(
            aggregation.function,
            aggregation::AggFunction::Count | aggregation::AggFunction::Nunique
        );
        produce(&mut fields_out, fields, &name, data_type, nullable)?;
    }
    if config.aggregations.is_empty() {
        produce(&mut fields_out, fields, "count", DataType::Int64, false)?;
    }
    // I metadata dello schema di input si conservano sempre (le chiavi
    // sconosciute non sono giudicabili qui; perderle rompe i round-trip).
    // Le colonne di gruppo tengono i metadata di campo; quelle aggregate
    // sono derivate e non ne ereditano. Un nome d'uscita ripetuto o uguale a
    // una chiave si rifiuta (`Aggregate::nomi_uscita`), come nel kernel.
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let preserved = input
        .geometries
        .first()
        .filter(|geometry| config.group_by.contains(&geometry.name))
        .map(|geometry| geometry.name.as_str());
    let geometry = propagate_geometry(input, &schema, preserved);
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(schema, geometry, active, ContractProperties::default())
}

pub(in crate::analyze) fn analyze_rolling_window(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: aggregation::RollingWindow = typed(op, config)?;
    let input = &inputs[0];
    con_op(op, config.verifica_parametri())?;
    if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
        return contract_error(op, "window/min_periods non validi");
    }
    if config.window > limits.max_rows {
        return contract_error(op, "window oltre max_rows");
    }
    check_output_name(op, &config.output_column)?;
    require_numeric(op, input, &config.column)?;
    // Le partizioni leggono `group_by` come scalare testuale
    // (`build_partitions`): stessi tipi ammessi del kernel.
    if let Some(group_by) = &config.group_by {
        require_scalar_string(op, input, group_by)?;
    }
    let sorted_by = if let Some(order_column) = &config.order_column {
        require_sortable(op, input, std::slice::from_ref(order_column))?;
        // Il kernel ordina sempre in ascendente su order_column.
        Some(proven_sorted(vec![fields.intern(order_column)?], true))
    } else {
        input.properties.sorted_by.clone()
    };
    if matches!(config.function, aggregation::RollingKind::Sum) {
        con_op(
            op,
            crate::float64_source::verifica_somma(field_of(op, input, &config.column)?.data_type()),
        )?;
    }
    // Il tipo del kernel (`tipo_uscita_rolling`).
    let tipo = aggregation::tipo_uscita_rolling(
        config.function,
        field_of(op, input, &config.column)?.data_type(),
    );
    let mut output = analyze_append(input, fields, &[(config.output_column, tipo, true)])?;
    output.properties = ContractProperties {
        sorted_by,
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}

pub(in crate::analyze) fn analyze_window_function(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    limits: &Limits,
) -> Result<DataContract> {
    let config: aggregation::WindowFunction = typed(op, config)?;
    let input = &inputs[0];
    con_op(op, config.verifica_offset())?;
    match (&config.function, config.buckets) {
        (aggregation::WindowKind::Ntile, Some(buckets)) if buckets > limits.max_rows => {
            return contract_error(op, "buckets oltre max_rows");
        }
        (aggregation::WindowKind::Ntile, Some(buckets)) if buckets > 0 => {}
        (aggregation::WindowKind::Ntile, _) => {
            return contract_error(op, "ntile richiede buckets > 0");
        }
        (_, Some(_)) => {
            return contract_error(op, "buckets ammesso solo con ntile");
        }
        _ => {}
    }
    require_numeric(op, input, &config.column)?;
    // Le funzioni di rango ordinano: il testo numerico non ha un ordine
    // esatto, e interpretarlo come double renderebbe a pari merito numeri
    // distinti. Rifiutato qui **e** nel kernel, con lo stesso confine.
    if aggregation::strategia(&config.function) == aggregation::Strategia::Rango
        && field_of(op, input, &config.column)?.data_type() == &DataType::Utf8
    {
        return contract_error(
            op,
            format!(
                "colonna {}: il testo numerico non ha un ordine esatto e le funzioni di rango non lo accettano",
                config.column
            ),
        );
    }
    // Partizioni su `group_by` come scalare testuale e sort su
    // `order_column`, come nel kernel.
    if let Some(group_by) = &config.group_by {
        require_scalar_string(op, input, group_by)?;
    }
    if let Some(order_column) = &config.order_column {
        require_sortable(op, input, std::slice::from_ref(order_column))?;
    }
    let name = config.output_column.clone().unwrap_or_else(|| {
        format!(
            "{}_{}",
            config.column,
            aggregation::suffisso(&config.function)
        )
    });
    check_output_name(op, &name)?;
    let sorted_by = match config.order_column.as_ref() {
        // Il kernel ordina sempre in ascendente su order_column.
        Some(order_column) => Some(proven_sorted(vec![fields.intern(order_column)?], true)),
        None => input.properties.sorted_by.clone(),
    };
    if matches!(config.function, aggregation::WindowKind::Cumsum) {
        con_op(
            op,
            crate::float64_source::verifica_somma(field_of(op, input, &config.column)?.data_type()),
        )?;
    }
    // Il tipo del kernel (`tipo_uscita_finestra`).
    let tipo = aggregation::tipo_uscita_finestra(
        &config.function,
        field_of(op, input, &config.column)?.data_type(),
    );
    let mut output = analyze_append(input, fields, &[(name, tipo, true)])?;
    output.properties = ContractProperties {
        sorted_by,
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}
