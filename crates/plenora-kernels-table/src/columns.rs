use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::array::{Array, ArrayRef, RecordBatch, RecordBatchOptions, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde::Deserialize;

use crate::Limits;
use plenora_core::{PlenoraError, Result};

use super::{replace_or_append, utf8_column, validate_output_name};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropColumns {
    pub columns: Vec<String>,
}

pub fn drop_columns(batch: &RecordBatch, config: &DropColumns) -> Result<RecordBatch> {
    let removed: HashSet<&str> = config.columns.iter().map(String::as_str).collect();
    let mut fields = Vec::new();
    let mut columns = Vec::new();
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        if !removed.contains(field.name().as_str()) {
            fields.push(field.as_ref().clone());
            columns.push(Arc::clone(column));
        }
    }
    let schema = Arc::new(Schema::new_with_metadata(
        fields,
        batch.schema().metadata().clone(),
    ));
    let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
    Ok(RecordBatch::try_new_with_options(
        schema, columns, &options,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenamePair {
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
    pub renames: Vec<RenamePair>,
}

pub fn rename(batch: &RecordBatch, config: &Rename) -> Result<RecordBatch> {
    let mapping: HashMap<&str, &str> = config
        .renames
        .iter()
        .map(|item| (item.old_name.as_str(), item.new_name.as_str()))
        .collect();
    let mut names = HashSet::new();
    let mut fields = Vec::with_capacity(batch.num_columns());
    for field in batch.schema().fields() {
        let name = mapping
            .get(field.name().as_str())
            .copied()
            .unwrap_or(field.name());
        validate_output_name(name)?;
        if !names.insert(name.to_owned()) {
            return Err(PlenoraError::Schema(format!(
                "rename produce il nome duplicato: {name}"
            )));
        }
        fields.push(field.as_ref().clone().with_name(name));
    }
    let schema = Schema::new_with_metadata(fields, batch.schema().metadata().clone());
    Ok(RecordBatch::try_new(
        Arc::new(schema),
        batch.columns().to_vec(),
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderColumns {
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default, alias = "sort_alphabetical")]
    pub alphabetical: bool,
}

pub fn reorder_columns(batch: &RecordBatch, config: &ReorderColumns) -> Result<RecordBatch> {
    let schema = batch.schema();
    let mut selected = Vec::with_capacity(batch.num_columns());
    let mut seen = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return Err(PlenoraError::Contract(format!(
                "colonna ripetuta nel riordino: {name}"
            )));
        }
        let index = schema
            .index_of(name)
            .map_err(|_| PlenoraError::Schema(format!("colonna non trovata: {name}")))?;
        selected.push(index);
    }
    let mut remaining: Vec<usize> = (0..batch.num_columns())
        .filter(|index| !selected.contains(index))
        .collect();
    if config.alphabetical {
        remaining.sort_by_key(|index| schema.field(*index).name().to_lowercase());
    }
    selected.extend(remaining);
    let fields: Vec<Field> = selected
        .iter()
        .map(|index| schema.field(*index).clone())
        .collect();
    let columns: Vec<ArrayRef> = selected
        .iter()
        .map(|index| Arc::clone(batch.column(*index)))
        .collect();
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
    )?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcatColumns {
    pub columns: Vec<String>,
    #[serde(default = "default_concat_output")]
    pub output_column: String,
    #[serde(default = "default_separator")]
    pub separator: String,
    #[serde(default = "default_true")]
    pub skip_null: bool,
}

fn default_concat_output() -> String {
    "concatenated".into()
}
fn default_separator() -> String {
    " ".into()
}
const fn default_true() -> bool {
    true
}

pub fn concat_columns(
    batch: &RecordBatch,
    config: &ConcatColumns,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.columns.is_empty() {
        return Err(PlenoraError::Contract(
            "concat_columns richiede almeno una colonna".into(),
        ));
    }
    validate_output_name(&config.output_column)?;
    let arrays = config
        .columns
        .iter()
        .map(|name| utf8_column(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let mut output = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let mut parts = Vec::with_capacity(arrays.len());
        for array in &arrays {
            if array.is_null(row) {
                if !config.skip_null {
                    parts.push("");
                }
            } else {
                parts.push(array.value(row));
            }
        }
        if config.skip_null && parts.is_empty() {
            output.push(None);
        } else {
            let value = parts.join(&config.separator);
            if value.len() > limits.max_string_bytes {
                return Err(PlenoraError::Contract(
                    "concat_columns supera max_string_bytes".into(),
                ));
            }
            output.push(Some(value));
        }
    }
    replace_or_append(
        batch,
        &config.output_column,
        DataType::Utf8,
        true,
        Arc::new(StringArray::from(output)),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitColumn {
    pub column: String,
    #[serde(default = "default_delimiter")]
    pub delimiter: String,
    pub new_columns: Vec<String>,
    #[serde(default = "default_max_splits")]
    pub max_splits: i64,
}

fn default_delimiter() -> String {
    ",".into()
}
const fn default_max_splits() -> i64 {
    -1
}

pub fn split_column(
    batch: &RecordBatch,
    config: &SplitColumn,
    limits: &Limits,
) -> Result<RecordBatch> {
    if config.delimiter.is_empty() {
        return Err(PlenoraError::Contract("delimiter vuoto".into()));
    }
    if config.new_columns.is_empty() {
        return Err(PlenoraError::Contract(
            "new_columns e' obbligatorio nel percorso streaming".into(),
        ));
    }
    if config.new_columns.len() > limits.max_split_columns {
        return Err(PlenoraError::Contract(
            "split_column supera max_split_columns".into(),
        ));
    }
    let unique: HashSet<_> = config.new_columns.iter().collect();
    if unique.len() != config.new_columns.len() {
        return Err(PlenoraError::Schema(
            "split_column contiene nomi output duplicati".into(),
        ));
    }
    for name in &config.new_columns {
        validate_output_name(name)?;
    }
    let input = utf8_column(batch, &config.column)?;
    let requested_parts = config.new_columns.len();
    let split_limit = if config.max_splits > 0 {
        usize::try_from(config.max_splits)
            .unwrap_or(usize::MAX)
            .saturating_add(1)
            .min(requested_parts)
    } else {
        requested_parts
    };
    let mut outputs = vec![Vec::<Option<String>>::with_capacity(batch.num_rows()); requested_parts];
    for row in 0..batch.num_rows() {
        if input.is_null(row) {
            for output in &mut outputs {
                output.push(None);
            }
            continue;
        }
        let parts: Vec<&str> = input
            .value(row)
            .splitn(split_limit, &config.delimiter)
            .collect();
        for (index, output) in outputs.iter_mut().enumerate() {
            output.push(parts.get(index).map(|value| (*value).to_owned()));
        }
    }
    let mut result = batch.clone();
    for (name, output) in config.new_columns.iter().zip(outputs) {
        result = replace_or_append(
            &result,
            name,
            DataType::Utf8,
            true,
            Arc::new(StringArray::from(output)),
        )?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("z", DataType::Utf8, true),
                Field::new("a", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("x,y,z"), None])),
                Arc::new(StringArray::from(vec![Some("1"), Some("2")])),
            ],
        )
        .expect("fixture")
    }

    #[test]
    fn defaults_and_non_destructive_paths_work() {
        let input = batch();
        let dropped = drop_columns(
            &input,
            &DropColumns {
                columns: vec!["missing".into()],
            },
        )
        .expect("unknown drop is no-op");
        assert_eq!(dropped.num_columns(), 2);

        let renamed = rename(&input, &Rename { renames: vec![] }).expect("empty rename");
        assert_eq!(renamed.schema(), input.schema());

        let reorder: ReorderColumns = serde_json::from_value(json!({})).expect("defaults");
        assert!(!reorder.alphabetical);
        assert_eq!(
            reorder_columns(&input, &reorder)
                .expect("no-op")
                .num_columns(),
            2
        );

        let concat: ConcatColumns =
            serde_json::from_value(json!({"columns": ["a"]})).expect("defaults");
        assert_eq!(concat.output_column, "concatenated");
        assert_eq!(concat.separator, " ");
        assert!(concat.skip_null);

        let split: SplitColumn =
            serde_json::from_value(json!({"column": "z", "new_columns": ["one", "two", "three"]}))
                .expect("defaults");
        assert_eq!(split.delimiter, ",");
        assert_eq!(split.max_splits, -1);
        let output = split_column(&input, &split, &Limits::default()).expect("unbounded split");
        assert_eq!(output.num_columns(), 5);
    }

    #[test]
    fn runtime_guards_remain_defensive() {
        let input = batch();
        let limits = Limits::default();
        assert!(concat_columns(
            &input,
            &ConcatColumns {
                columns: vec![],
                output_column: "x".into(),
                separator: String::new(),
                skip_null: true,
            },
            &limits,
        )
        .is_err());
        assert!(reorder_columns(
            &input,
            &ReorderColumns {
                columns: vec!["a".into(), "a".into()],
                alphabetical: false,
            },
        )
        .is_err());

        let base = SplitColumn {
            column: "z".into(),
            delimiter: ",".into(),
            new_columns: vec!["x".into()],
            max_splits: -1,
        };
        let empty_delimiter = SplitColumn {
            delimiter: String::new(),
            ..base
        };
        assert!(split_column(&input, &empty_delimiter, &limits).is_err());
        let empty_outputs = SplitColumn {
            column: "z".into(),
            delimiter: ",".into(),
            new_columns: vec![],
            max_splits: -1,
        };
        assert!(split_column(&input, &empty_outputs, &limits).is_err());
        let duplicates = SplitColumn {
            new_columns: vec!["x".into(), "x".into()],
            ..empty_outputs
        };
        assert!(split_column(&input, &duplicates, &limits).is_err());
        let one_output = Limits {
            max_split_columns: 1,
            ..Limits::default()
        };
        let too_many = SplitColumn {
            column: "z".into(),
            delimiter: ",".into(),
            new_columns: vec!["x".into(), "y".into()],
            max_splits: -1,
        };
        assert!(split_column(&input, &too_many, &one_output).is_err());
    }

    #[test]
    fn concat_null_modes_and_alphabetical_reorder_are_exact() {
        let input = batch();
        let output = concat_columns(
            &input,
            &ConcatColumns {
                columns: vec!["z".into(), "a".into()],
                output_column: "joined".into(),
                separator: "|".into(),
                skip_null: false,
            },
            &Limits::default(),
        )
        .expect("concat");
        let joined = output
            .column_by_name("joined")
            .expect("joined")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("utf8");
        assert_eq!(joined.value(1), "|2");

        let reordered = reorder_columns(
            &input,
            &ReorderColumns {
                columns: vec![],
                alphabetical: true,
            },
        )
        .expect("alphabetical");
        assert_eq!(reordered.schema().field(0).name(), "a");
    }
}
