//! Descrizione di un tipo Arrow per i messaggi d'errore, senza metadati.
//!
//! `Display` e `Debug` di `DataType` in Arrow 60 stampano i campi figli
//! interi: nomi, nullabilità e **metadati** (un `Struct` con un figlio che
//! porta `{"chiave": "valore"}` lo riporta nel testo). I metadati di un file
//! sono dati del file, e non entrano nei messaggi («errori senza dati»).
//! [`descrivi_tipo`] scende nei tipi composti e ne scrive solo le varianti:
//! `List<Binary>`, `Struct<3 campi>`, `Map<Struct<2 campi>>`, mai nomi o
//! metadati dei figli.

use crate::arrow::schema::DataType;

/// Il tipo come testo per un messaggio d'errore: varianti, ricorsivamente,
/// senza nomi né metadati dei campi figli.
///
/// I tipi senza figli si scrivono come li scrive Arrow (`Int64`,
/// `Timestamp(Microsecond, Some("UTC"))`, `Decimal128(10, 2)`): i loro
/// parametri sono parte del tipo, non metadati. I composti si scrivono con
/// il tipo dei figli (`List<Utf8>`, `FixedSizeList<Float64; 3>`,
/// `Dictionary<Int32, Utf8>`) o con il loro numero (`Struct<2 campi>`,
/// `Union<3 campi>`).
#[must_use]
pub fn descrivi_tipo(tipo: &DataType) -> String {
    match tipo {
        DataType::List(figlio) => format!("List<{}>", descrivi_tipo(figlio.data_type())),
        DataType::LargeList(figlio) => {
            format!("LargeList<{}>", descrivi_tipo(figlio.data_type()))
        }
        DataType::ListView(figlio) => format!("ListView<{}>", descrivi_tipo(figlio.data_type())),
        DataType::LargeListView(figlio) => {
            format!("LargeListView<{}>", descrivi_tipo(figlio.data_type()))
        }
        DataType::FixedSizeList(figlio, lunghezza) => format!(
            "FixedSizeList<{}; {lunghezza}>",
            descrivi_tipo(figlio.data_type())
        ),
        DataType::Map(voci, _) => format!("Map<{}>", descrivi_tipo(voci.data_type())),
        DataType::Struct(figli) => format!("Struct<{} campi>", figli.len()),
        DataType::Union(figli, _) => format!("Union<{} campi>", figli.len()),
        DataType::Dictionary(chiave, valore) => format!(
            "Dictionary<{}, {}>",
            descrivi_tipo(chiave),
            descrivi_tipo(valore)
        ),
        DataType::RunEndEncoded(fini, valori) => format!(
            "RunEndEncoded<{}, {}>",
            descrivi_tipo(fini.data_type()),
            descrivi_tipo(valori.data_type())
        ),
        // Nessun campo figlio: niente nomi né metadati da tacere.
        foglia => foglia.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::descrivi_tipo;
    use crate::arrow::schema::{DataType, Field, Fields, UnionFields, UnionMode};

    fn con_segreto(nome: &str, tipo: DataType) -> Field {
        Field::new(nome, tipo, true).with_metadata(HashMap::from([(
            "plenora.segreto".to_owned(),
            "SEGRETO".to_owned(),
        )]))
    }

    #[test]
    fn i_metadati_e_i_nomi_dei_figli_non_compaiono() {
        let figlio = con_segreto("nome_figlio", DataType::Binary);
        let struttura = DataType::Struct(Fields::from(vec![
            figlio.clone(),
            con_segreto("altro", DataType::Utf8),
        ]));
        let casi = [
            (DataType::List(Arc::new(figlio.clone())), "List<Binary>"),
            (
                DataType::LargeList(Arc::new(figlio.clone())),
                "LargeList<Binary>",
            ),
            (
                DataType::FixedSizeList(Arc::new(figlio.clone()), 3),
                "FixedSizeList<Binary; 3>",
            ),
            (struttura.clone(), "Struct<2 campi>"),
            (
                DataType::List(Arc::new(con_segreto("s", struttura.clone()))),
                "List<Struct<2 campi>>",
            ),
            (
                DataType::Map(Arc::new(Field::new("voci", struttura, false)), false),
                "Map<Struct<2 campi>>",
            ),
            (
                DataType::Union(
                    UnionFields::try_new(
                        vec![0, 1],
                        vec![figlio, con_segreto("u", DataType::Int64)],
                    )
                    .expect("union"),
                    UnionMode::Sparse,
                ),
                "Union<2 campi>",
            ),
            (
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                "Dictionary<Int32, Utf8>",
            ),
            (
                DataType::RunEndEncoded(
                    Arc::new(Field::new("run_ends", DataType::Int32, false)),
                    Arc::new(con_segreto("values", DataType::Utf8)),
                ),
                "RunEndEncoded<Int32, Utf8>",
            ),
            (DataType::Int64, "Int64"),
        ];
        for (tipo, atteso) in casi {
            let testo = descrivi_tipo(&tipo);
            assert_eq!(testo, atteso);
            assert!(!testo.contains("SEGRETO"), "{testo}");
            assert!(!testo.contains("nome_figlio"), "{testo}");
            // Il difetto che la funzione evita: Arrow stampa i metadati.
            if atteso != "Int64" && atteso != "Dictionary<Int32, Utf8>" {
                assert!(
                    format!("{tipo}").contains("SEGRETO")
                        || format!("{tipo:?}").contains("SEGRETO"),
                    "Arrow non stampa piu' i metadati dei figli: {tipo}"
                );
            }
        }
    }
}
