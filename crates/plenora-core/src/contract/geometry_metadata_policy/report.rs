//! Effetti localizzati: proiezione della politica e dei contratti, senza valori.
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{keep, operation_effect, Effect};
use crate::contract::DataContract;
use crate::{PlenoraError, Result};

/// Contratto component-owned aggiuntivo del risultato di esecuzione.
pub const CONTRACT: &str = "plenora-data-localized-effects-v1";

/// Un effetto: indici zero-based nello schema del passo, mai indici di riga.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LocalizedEffect {
    /// Campo d'uscita; assente per metadati di schema o campo eliminato.
    pub field_index: Option<usize>,
    /// Ingresso del passo; assente per un campo interamente nuovo.
    pub input_index: Option<usize>,
    /// Campo sorgente; assente per metadati di schema.
    pub source_field_index: Option<usize>,
    /// JSON Pointer della chiave; assente per la derivazione del campo.
    pub key: Option<String>,
    /// `derived`, `removed`, `metadata_invalidated` oppure `metadata_reconstructed`.
    pub action: &'static str,
}

fn pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn properties(
    metadata: &crate::arrow::Metadata,
    present_after: &crate::arrow::Metadata,
    unchanged_peer: Option<&crate::arrow::Metadata>,
) -> Result<BTreeMap<String, (String, String)>> {
    let mut result = BTreeMap::new();
    for (key, value) in metadata {
        let path = format!("/{}", pointer(key));
        // La politica puo' eliminare un contenitore opaco senza leggerlo.
        // Il rapporto ne dichiara la rimozione per chiave, non lo interpreta.
        if present_after.contains_key(key)
            && unchanged_peer.is_none_or(|peer| peer.get(key) != Some(value))
            && matches!(key.as_str(), "ARROW:extension:metadata" | "geo")
            && !value.trim().is_empty()
        {
            crate::json::ensure_no_duplicate_keys(value)?;
            crate::json::ensure_numbers_exact(value)?;
            let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| {
                PlenoraError::Internal("metadati del rapporto non leggibili".into())
            })?;
            if let serde_json::Value::Object(members) = &parsed {
                if members.is_empty() {
                    flatten(&mut result, &path, key, &parsed);
                }
                for (name, member) in members {
                    let member_path = format!("{path}/{}", pointer(name));
                    if key == "geo" && name == "columns" {
                        if let serde_json::Value::Object(columns) = member {
                            for (column, properties) in columns {
                                if let serde_json::Value::Object(properties) = properties {
                                    for (property, value) in properties {
                                        flatten(
                                            &mut result,
                                            &format!(
                                                "{member_path}/{}/{}",
                                                pointer(column),
                                                pointer(property)
                                            ),
                                            property,
                                            value,
                                        );
                                    }
                                } else {
                                    flatten(
                                        &mut result,
                                        &format!("{member_path}/{}", pointer(column)),
                                        name,
                                        properties,
                                    );
                                }
                            }
                            continue;
                        }
                    }
                    flatten(&mut result, &member_path, name, member);
                }
            } else {
                flatten(&mut result, &path, key, &parsed);
            }
        } else {
            result.insert(path, (key.clone(), value.clone()));
        }
    }
    Ok(result)
}

fn flatten(
    result: &mut BTreeMap<String, (String, String)>,
    path: &str,
    key: &str,
    value: &serde_json::Value,
) {
    if let serde_json::Value::Object(object) = value {
        if !object.is_empty() {
            for (name, member) in object {
                flatten(result, &format!("{path}/{}", pointer(name)), key, member);
            }
            return;
        }
    }
    result.insert(path.to_owned(), (key.to_owned(), value.to_string()));
}

fn metadata_effects(
    result: &mut Vec<LocalizedEffect>,
    before: &crate::arrow::Metadata,
    after: &crate::arrow::Metadata,
    effect: Effect,
    location: &LocalizedEffect,
) -> Result<()> {
    // Un contenitore passato byte per byte non viene interpretato dalla politica.
    let same_before = (effect == Effect::Unchanged).then_some(after);
    let same_after = (effect == Effect::Unchanged).then_some(before);
    let before = properties(before, after, same_before)?;
    let after = properties(after, after, same_after)?;
    let keys: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    for key in keys {
        let action = match (before.get(key), after.get(key)) {
            (Some(_), None) => "metadata_invalidated",
            (None, Some(_)) => "metadata_reconstructed",
            (Some(old), Some(new))
                if old != new
                    || ((super::governed(&new.0)
                        || key.starts_with("/ARROW:extension:metadata/")
                        || key.starts_with("/geo/"))
                        && !keep(&new.0, effect, true)) =>
            {
                "metadata_reconstructed"
            }
            _ => continue,
        };
        result.push(LocalizedEffect {
            key: Some(key.clone()),
            action,
            ..location.clone()
        });
    }
    Ok(())
}

/// Confronta identita' e chiavi applicando gli effetti della stessa politica
/// usata dall'analisi. Nessun valore di metadato entra nel rapporto.
///
/// # Errors
/// Internal per contenitori geometrici interpretati non leggibili.
pub fn localized_effects(
    descriptor: &crate::catalog::OperationDescriptor,
    inputs: &[&DataContract],
    output: &DataContract,
    effects: &[(usize, Effect)],
) -> Result<Vec<LocalizedEffect>> {
    let mut result = Vec::new();
    let empty = crate::arrow::Metadata::new();
    for (position, field) in output.schema.fields().iter().enumerate() {
        let id = output.field_ids.get(field.name());
        let mut sources = Vec::new();
        for (input_index, input) in inputs.iter().enumerate() {
            for (source_index, source) in input.schema.fields().iter().enumerate() {
                if (id.is_some() && input.field_ids.get(source.name()) == id)
                    || (operation_effect(descriptor) == Effect::Merge
                        && source.name() == field.name()
                        && (descriptor.family == crate::catalog::Family::Table
                            || crate::contract::arrow_metadata::field_declares_wkb_geometry(field)))
                {
                    sources.push((input_index, source_index, source));
                }
            }
        }
        let fresh = sources.is_empty();
        // Un derivato che sostituisce un campo conserva la localizzazione della
        // dichiarazione invalidata; gli ID sono nuovi, i nomi da soli non
        // attestano conservazione dell'identita'.
        if fresh {
            if let Some(input) = inputs.first() {
                if let Ok(index) = input.schema.index_of(field.name()) {
                    sources.push((0, index, &input.schema.fields()[index]));
                }
            }
        }
        let effect = effects
            .iter()
            .find(|(index, _)| *index == position)
            .map_or(Effect::Unchanged, |(_, effect)| *effect);
        let base = LocalizedEffect {
            field_index: Some(position),
            input_index: None,
            source_field_index: None,
            key: None,
            action: "derived",
        };
        if fresh
            || (descriptor.family == crate::catalog::Family::Table
                && operation_effect(descriptor) == Effect::Merge)
            || matches!(
                effect,
                Effect::Values | Effect::Types | Effect::Crs | Effect::Merge
            )
        {
            if sources.is_empty() {
                result.push(base.clone());
            } else {
                for (input_index, source_index, _) in &sources {
                    result.push(LocalizedEffect {
                        input_index: Some(*input_index),
                        source_field_index: Some(*source_index),
                        ..base.clone()
                    });
                }
            }
        }
        if sources.is_empty() {
            metadata_effects(&mut result, &empty, field.metadata(), effect, &base)?;
        }
        for (input_index, source_index, source) in sources {
            metadata_effects(
                &mut result,
                source.metadata(),
                field.metadata(),
                effect,
                &LocalizedEffect {
                    input_index: Some(input_index),
                    source_field_index: Some(source_index),
                    ..base.clone()
                },
            )?;
        }
    }
    removed_and_schema_effects(&mut result, descriptor, inputs, output)?;
    Ok(result)
}

fn removed_and_schema_effects(
    result: &mut Vec<LocalizedEffect>,
    descriptor: &crate::catalog::OperationDescriptor,
    inputs: &[&DataContract],
    output: &DataContract,
) -> Result<()> {
    let empty = crate::arrow::Metadata::new();

    for (input_index, input) in inputs.iter().enumerate() {
        for (source_index, field) in input.schema.fields().iter().enumerate() {
            let id = input.field_ids.get(field.name());
            if !output.schema.fields().iter().any(|out| {
                (id.is_some() && output.field_ids.get(out.name()) == id)
                    || (operation_effect(descriptor) == Effect::Merge
                        && out.name() == field.name()
                        && (descriptor.family == crate::catalog::Family::Table
                            || crate::contract::arrow_metadata::field_declares_wkb_geometry(out)))
                    || (input_index == 0
                        && out.name() == field.name()
                        && !inputs.iter().any(|source| {
                            source.field_ids.values().any(|source_id| {
                                Some(source_id) == output.field_ids.get(out.name())
                            })
                        }))
            }) {
                result.push(LocalizedEffect {
                    field_index: None,
                    input_index: Some(input_index),
                    source_field_index: Some(source_index),
                    key: None,
                    action: "removed",
                });
                metadata_effects(
                    result,
                    field.metadata(),
                    &empty,
                    Effect::Unchanged,
                    &LocalizedEffect {
                        field_index: None,
                        input_index: Some(input_index),
                        source_field_index: Some(source_index),
                        key: None,
                        action: "removed",
                    },
                )?;
            }
        }
        metadata_effects(
            result,
            input.schema.metadata(),
            output.schema.metadata(),
            operation_effect(descriptor),
            &LocalizedEffect {
                field_index: None,
                input_index: Some(input_index),
                source_field_index: None,
                key: None,
                action: "derived",
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn discarded_container_is_reported_without_interpreting_its_value() {
        for value in ["obsoleto", r#"{"x":1,"x":2}"#, r#"{"x":1e400}"#] {
            let mut before = crate::arrow::Metadata::new();
            before.insert("geo", value);
            let after = crate::arrow::Metadata::new();
            let location = LocalizedEffect {
                field_index: None,
                input_index: Some(0),
                source_field_index: None,
                key: None,
                action: "derived",
            };
            let mut result = Vec::new();
            metadata_effects(&mut result, &before, &after, Effect::Values, &location).unwrap();
            assert_eq!(
                result,
                vec![LocalizedEffect {
                    key: Some("/geo".into()),
                    action: "metadata_invalidated",
                    ..location
                }]
            );
        }
    }

    #[test]
    fn unchanged_opaque_container_does_not_hide_other_metadata_changes() {
        let mut before = crate::arrow::Metadata::new();
        before.insert("geo", "obsoleto");
        before.insert("user.old", "private");
        let mut after = crate::arrow::Metadata::new();
        after.insert("geo", "obsoleto");
        after.insert("user.new", "private");
        let location = LocalizedEffect {
            field_index: None,
            input_index: Some(0),
            source_field_index: None,
            key: None,
            action: "derived",
        };
        let mut result = Vec::new();
        metadata_effects(&mut result, &before, &after, Effect::Unchanged, &location).unwrap();
        assert_eq!(
            result,
            vec![
                LocalizedEffect {
                    key: Some("/user.new".into()),
                    action: "metadata_reconstructed",
                    ..location.clone()
                },
                LocalizedEffect {
                    key: Some("/user.old".into()),
                    action: "metadata_invalidated",
                    ..location
                },
            ]
        );
    }

    #[test]
    fn nested_crs_is_preserved_and_unknown_pointer_is_localized() {
        let mut before = crate::arrow::Metadata::new();
        before.insert(
            "ARROW:extension:metadata",
            r#"{"crs":{"type":"ProjectedCRS","name":"private"},"future/key":{"~value":1}}"#,
        );
        let mut after = crate::arrow::Metadata::new();
        after.insert(
            "ARROW:extension:metadata",
            r#"{"crs":{"type":"ProjectedCRS","name":"private"}}"#,
        );
        let mut result = Vec::new();
        let location = LocalizedEffect {
            field_index: Some(1),
            input_index: Some(0),
            source_field_index: Some(2),
            key: None,
            action: "derived",
        };
        metadata_effects(&mut result, &before, &after, Effect::Values, &location).unwrap();
        assert_eq!(
            result,
            vec![LocalizedEffect {
                key: Some("/ARROW:extension:metadata/future~1key/~0value".into()),
                action: "metadata_invalidated",
                ..location
            }]
        );
    }
    fn contract(id: u32, future: bool) -> DataContract {
        let mut md = crate::arrow::Metadata::new();
        if future {
            md.insert("plenora.postgres.future", "secret");
        }
        let field =
            crate::arrow::Field::new("g", crate::arrow::DataType::Binary, true).with_metadata(md);
        let mut contract =
            DataContract::tabular(std::sync::Arc::new(crate::arrow::Schema::new(vec![field])));
        contract
            .field_ids
            .insert("g".into(), crate::contract::FieldId(id));
        contract
    }

    #[test]
    fn merge_reports_metadata_from_every_input() {
        let first = contract(1, false);
        let second = contract(2, false);
        let third = contract(3, true);
        let output = contract(1, false);
        let op = crate::catalog::find_operation("table.concat").unwrap();
        let effects = localized_effects(
            op,
            &[&first, &second, &third],
            &output,
            &[(0, Effect::Merge)],
        )
        .unwrap();
        assert!(effects.iter().any(|e| e.input_index == Some(2)
            && e.source_field_index == Some(0)
            && e.field_index == Some(0)
            && e.key.as_deref() == Some("/plenora.postgres.future")
            && e.action == "metadata_invalidated"));
    }

    #[test]
    fn same_name_is_not_same_identity() {
        let left = contract(1, false);
        let right = contract(2, false);
        let output = contract(2, false);
        let op = crate::catalog::find_operation("table.join").unwrap();
        let effects = localized_effects(op, &[&left, &right], &output, &[]).unwrap();
        assert!(effects.iter().any(|e| e.input_index == Some(0)
            && e.source_field_index == Some(0)
            && e.field_index.is_none()
            && e.action == "removed"));
    }
    #[test]
    fn union_distinct_combines_both_origins_but_intersection_does_not() {
        let left = contract(1, false);
        let right = contract(2, false);
        let output = contract(1, false);
        for (name, removed) in [
            ("table.union_distinct", false),
            ("table.intersect", true),
            ("table.except", true),
        ] {
            let op = crate::catalog::find_operation(name).unwrap();
            let effects = localized_effects(op, &[&left, &right], &output, &[]).unwrap();
            assert_eq!(
                effects
                    .iter()
                    .any(|e| e.input_index == Some(1) && e.action == "removed"),
                removed
            );
        }
    }
}
