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
//!
//! # La chiave riservata di `serde_json`
//!
//! La stessa passata rifiuta una seconda ambiguita', della stessa specie e
//! peggiore. `serde_json` riserva la chiave `$serde_json::private::RawValue`
//! per trasportare JSON grezzo: quando e' la **prima** chiave di un oggetto,
//! `serde_json::from_str::<Value>` non la legge come una chiave.
//!
//! Se il valore non e' una stringa, **fallisce**:
//!
//! ```text
//! {"$serde_json::private::RawValue": 3}
//!   -> invalid type: integer `3`, expected raw value
//! ```
//!
//! Se il valore e' una stringa di JSON valido, e' peggio: **riesce**, e rende
//! un documento diverso da quello scritto.
//!
//! ```text
//! {"$serde_json::private::RawValue": "{\"schema_version\":4}"}
//!   -> {"schema_version": 4}
//! ```
//!
//! Il testo letterale e' un oggetto con una chiave e un valore stringa; il
//! documento che il lettore ottiene e' un altro. E' la stessa perdita della
//! chiave duplicata portata all'estremo — il piano eseguito non e' quello
//! scritto — con in piu' un effetto che il solo controllo dei duplicati non
//! chiude: **il contrabbando lo aggira**, perche' la passata vede un oggetto
//! con una chiave sola mentre il documento effettivo ne ha due uguali.
//!
//! # E' una restrizione del contratto, non una correzione a costo zero
//!
//! Il rifiuto **toglie qualcosa**: un documento con quella chiave in seconda
//! posizione, o annidata dove nessuna riscrittura la riordina, non e' ambiguo
//! per nessun lettore, e da qui in avanti e' respinto lo stesso. Chi lo scrive
//! riceve un errore al posto di un piano valido.
//!
//! Si restringe lo stesso, perche' la posizione non e' una proprieta' stabile
//! del documento: la canonicalizzazione riordina le chiavi e `$` precede ogni
//! lettera, quindi una chiave innocua diventa la prima del testo canonico. Un
//! rifiuto ristretto alla sola prima posizione ammetterebbe documenti che il
//! progetto stesso rende poi illeggibili — e lascerebbe aperto il
//! contrabbando, che vive proprio in prima posizione.
//!
//! **Perimetro**: la chiave, a ogni posizione e profondita', in ogni documento
//! JSON di controllo. Come **valore** stringa e' testo qualunque e passa.
//! **Condizione di rientro**: il giorno che `serde_json` smetta di riservare
//! quel nome, o offra un lettore che non lo reinterpreta, la restrizione non
//! ha piu' ragione.
//!
//! L'escape non e' una via d'uscita: `serde_json` decodifica `\u0024` prima di
//! consegnare la chiave, quindi `"\u0024serde_json..."` e' la stessa chiave e
//! riceve lo stesso rifiuto.

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
/// oggetto, o se un oggetto usa la chiave riservata di `serde_json`,
/// nominando in entrambi i casi la chiave.
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

/// La chiave che `serde_json` riserva a [`serde_json::value::RawValue`].
///
/// Scritta qui per intero e non composta: e' un letterale del formato, e
/// costruirla a pezzi renderebbe piu' difficile cercarla che leggerla.
const CHIAVE_RISERVATA_SERDE_JSON: &str = "$serde_json::private::RawValue";

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
            // Prima del duplicato, perche' un documento che porta questa
            // chiave non e' ambiguo: e' illeggibile o, peggio, ne nasconde un
            // altro. Il controllo e' sulla chiave a **qualunque** posizione e
            // profondita', non sulla sola prima di un oggetto: `serde_json`
            // reinterpreta la prima, e una riscrittura canonica riordina le
            // chiavi — `$` viene prima di ogni lettera, quindi una chiave
            // innocua in coda diventa la prima del canonico.
            if key == CHIAVE_RISERVATA_SERDE_JSON {
                return Err(serde::de::Error::custom(format!(
                    "chiave JSON riservata `{key}`: `serde_json` la usa per trasportare \
                     JSON grezzo e non la legge come una chiave, quindi il documento \
                     non e' quello che dichiara di essere"
                )));
            }
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

    /// **La chiave riservata di `serde_json` e' rifiutata a ogni posizione.**
    ///
    /// Non solo in prima posizione, che e' l'unica in cui
    /// `serde_json` la reinterpreta: una riscrittura canonica riordina le
    /// chiavi, e `$` viene prima di ogni lettera, quindi una chiave innocua in
    /// seconda posizione diventa la prima del testo canonico. Restringere il
    /// rifiuto alla sola prima posizione lascerebbe passare proprio il
    /// documento che la canonicalizzazione rende illeggibile.
    #[test]
    fn la_chiave_riservata_e_rifiutata_a_ogni_posizione() {
        for testo in [
            r#"{"$serde_json::private::RawValue": 3}"#,
            r#"{"a": 1, "$serde_json::private::RawValue": 3}"#,
            r#"{"esterno": {"$serde_json::private::RawValue": 3}}"#,
            r#"[{"$serde_json::private::RawValue": 3}]"#,
        ] {
            let errore = ensure_no_duplicate_keys(testo).expect_err(testo);
            assert!(
                errore.to_string().contains("riservata"),
                "{testo}: {errore}"
            );
        }
    }

    /// **L'escape Unicode non e' una via d'uscita.**
    ///
    /// `serde_json` decodifica gli escape prima di consegnare la chiave al
    /// visitatore, quindi il confronto avviene sul nome decodificato. Un
    /// rifiuto che cercasse la sequenza nel testo grezzo si farebbe aggirare
    /// scrivendo un solo carattere in `\u0024xxxx`, e non e' teoria: `serde_json`
    /// riconosce la chiave anche cosi', quindi il documento resterebbe
    /// illeggibile mentre il confine lo dichiara buono.
    #[test]
    fn l_escape_unicode_non_aggira_il_rifiuto() {
        for testo in [
            r#"{"\u0024serde_json::private::RawValue": 3}"#,
            r#"{"$serde_json::private::\u0052awValue": 3}"#,
            r#"{"\u0024serde_json::private::RawValue": "{\"a\":1,\"a\":2}"}"#,
        ] {
            let errore = ensure_no_duplicate_keys(testo).expect_err(testo);
            assert!(
                errore.to_string().contains("riservata"),
                "{testo}: {errore}"
            );
        }
    }

    /// **Il caso che il fuzzer ha trovato, ridotto.**
    ///
    /// `plan_v5_parse` lo ha prodotto in 511 byte; qui sta in una riga. Senza
    /// questo rifiuto la migrazione accetta il testo e ne rende uno che nessun
    /// lettore del progetto sa rileggere: l'oracolo di idempotenza del target
    /// segnala un difetto vero, non una propria imprecisione.
    #[test]
    fn il_riproduttore_ridotto_del_fuzzer_e_rifiutato() {
        let errore = ensure_no_duplicate_keys(
            r#"{"schema_version": 4, "$serde_json::private::RawValue": 3}"#,
        )
        .expect_err("il documento non e' rileggibile e va rifiutato");
        assert!(errore.to_string().contains("riservata"), "{errore}");
    }

    /// **Il contrabbando di un documento diverso e' rifiutato.**
    ///
    /// E' il verso peggiore, perche' non fallisce: `serde_json` **riesce** e
    /// rende il documento contenuto nella stringa. Il testo letterale ha una
    /// chiave sola, quindi il controllo dei duplicati non vede niente, mentre
    /// il documento effettivo ne ha due uguali e le risolve con «vince
    /// l'ultima». Senza questo rifiuto, il contrabbando aggira il controllo
    /// che esiste apposta per quell'ambiguita'.
    #[test]
    fn il_contrabbando_non_aggira_il_controllo_dei_duplicati() {
        let contrabbando =
            r#"{"$serde_json::private::RawValue": "{\"schema_version\":5,\"schema_version\":4}"}"#;

        // La premessa: senza il rifiuto, `serde_json` legge un ALTRO documento.
        let effettivo: serde_json::Value =
            serde_json::from_str(contrabbando).expect("serde_json lo accetta, ed e' il problema");
        assert_eq!(
            effettivo,
            serde_json::json!({"schema_version": 4}),
            "il documento effettivo non e' quello scritto, e i duplicati sono gia' risolti"
        );

        // La conclusione: il confine lo ferma prima.
        let errore = ensure_no_duplicate_keys(contrabbando).expect_err("il confine lo ferma");
        assert!(errore.to_string().contains("riservata"), "{errore}");
    }

    /// **Come valore stringa non e' una chiave, e passa.**
    ///
    /// Il rifiuto e' sulla chiave. Un documento che nomina quel testo dentro
    /// un valore non inganna nessun lettore, e rifiutarlo sarebbe togliere a
    /// chi scrive una stringa qualunque.
    #[test]
    fn come_valore_stringa_resta_testo_qualunque() {
        ensure_no_duplicate_keys(r#"{"nota": "$serde_json::private::RawValue"}"#)
            .expect("come valore non e' una chiave");
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
