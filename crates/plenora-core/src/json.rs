//! Lettura fail-closed del JSON di controllo.
//!
//! `serde_json` risolve le chiavi duplicate di un oggetto con la regola
//! «vince l'ultima»: `{"a": 1, "a": 2}` diventa `{"a": 2}` senza che nulla lo
//! segnali. Per i dati e' una tolleranza ragionevole; per il JSON di
//! CONTROLLO — piani, config dei nodi, metadati contrattuali — non lo e':
//!
//! - la chiave scartata puo' essere quella che l'autore intende, e il piano
//!   eseguito non e' quello scritto;
//! - la risoluzione avviene PRIMA della validazione e prima del `plan_hash`,
//!   quindi due testi diversi producono lo stesso piano canonico e lo stesso
//!   hash: la firma non distingue piu' l'input che l'ha prodotta.
//!
//! [`ensure_no_duplicate_keys`] rifiuta il documento invece di scegliere.

use std::collections::HashSet;
use std::fmt;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};

use crate::error::{PlenoraError, Result};

/// Verifica che nessun oggetto del documento JSON abbia chiavi ripetute.
///
/// La visita non costruisce nulla: attraversa il documento e tiene per ogni
/// oggetto il solo insieme delle sue chiavi.
///
/// Un documento sintatticamente INVALIDO non e' un errore per questa
/// funzione: risponde a una sola domanda — ci sono chiavi ripetute? — e
/// lascia che sia la deserializzazione vera a segnalare la sintassi, che ha
/// il contesto per classificarla nella categoria giusta.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se una chiave e' ripetuta nello stesso
/// oggetto, nominando la chiave.
pub fn ensure_no_duplicate_keys(json_text: &str) -> Result<()> {
    let mut deserializer = serde_json::Deserializer::from_str(json_text);
    let esito = serde::de::DeserializeSeed::deserialize(UniqueKeys, &mut deserializer);
    // `Category::Data` e' la classe degli errori prodotti dal visitatore, cioe'
    // esattamente la chiave duplicata; sintassi ed EOF spettano al parse vero e
    // qui non sono un errore.
    if let Err(error) = esito {
        if error.classify() == serde_json::error::Category::Data {
            return Err(PlenoraError::InvalidPlan(error.to_string()));
        }
    }
    Ok(())
}

/// Seme di deserializzazione che non produce valore: verifica soltanto.
struct UniqueKeys;

impl<'de> serde::de::DeserializeSeed<'de> for UniqueKeys {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueKeysVisitor)
    }
}

struct UniqueKeysVisitor;

impl<'de> Visitor<'de> for UniqueKeysVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("un documento JSON senza chiavi duplicate")
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            // Il valore si visita comunque: gli oggetti annidati hanno le
            // proprie chiavi da controllare.
            map.next_value_seed(UniqueKeys)?;
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "chiave JSON duplicata `{key}`: il documento e' ambiguo e non viene risolto \
                     con «vince l'ultima»"
                )));
            }
        }
        Ok(())
    }

    fn visit_seq<A>(self, mut seq: A) -> std::result::Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        while seq.next_element_seed(UniqueKeys)?.is_some() {}
        Ok(())
    }

    // Gli scalari non hanno chiavi: si accettano senza materializzarli.
    fn visit_bool<E>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_unit<E>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_none<E>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_some<D>(self, deserializer: D) -> std::result::Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i_documenti_senza_duplicati_passano() {
        for text in [
            r#"{"a": 1, "b": {"a": 2}}"#,
            r#"[{"a": 1}, {"a": 2}]"#,
            r#"{"nested": [[{"x": null}]], "n": 1.5}"#,
            "42",
            r#""testo""#,
            "null",
        ] {
            ensure_no_duplicate_keys(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        }
    }

    #[test]
    fn le_chiavi_duplicate_sono_rifiutate_a_ogni_profondita() {
        for text in [
            r#"{"a": 1, "a": 2}"#,
            r#"{"outer": {"a": 1, "a": 2}}"#,
            r#"[{"a": 1, "a": 2}]"#,
            r#"{"list": [1, {"k": 1, "k": 2}]}"#,
        ] {
            let error = ensure_no_duplicate_keys(text).expect_err(text);
            assert!(error.to_string().contains("duplicata"), "{text}: {error}");
        }
    }

    #[test]
    fn il_json_malformato_non_e_affare_di_questa_funzione() {
        // La sintassi la segnala la deserializzazione vera, che la classifica
        // come errore di mappatura dei dati: anticiparla qui cambierebbe la
        // categoria dell'errore visto dal chiamante.
        assert!(ensure_no_duplicate_keys("{").is_ok());
        assert!(ensure_no_duplicate_keys("{} {}").is_ok());
        assert!(ensure_no_duplicate_keys("non json").is_ok());
    }
}
