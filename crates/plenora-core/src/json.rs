//! Lettura fail-closed del JSON di controllo.
//!
//! `serde_json` risolve le chiavi duplicate con «vince l'ultima». Per il JSON
//! di controllo (piani del runner, config dei passi, metadati `GeoParquet` e
//! contrattuali) è un pericolo: il piano eseguito può non essere quello
//! scritto, e due testi diversi diventano lo stesso piano.
//! [`ensure_no_duplicate_keys`] rifiuta il documento invece di scegliere.
//!
//! La stessa passata rifiuta la chiave riservata
//! `$serde_json::private::RawValue`, a ogni posizione e profondità e anche
//! scritta con escape: in prima posizione `serde_json` la reinterpreta come
//! JSON grezzo e può rendere un documento diverso da quello scritto,
//! aggirando il controllo dei duplicati. È una restrizione dichiarata: nel
//! JSON di controllo quella chiave non è ammessa, nemmeno dove sarebbe
//! innocua; come valore stringa resta ammessa.

use std::collections::HashSet;
use std::fmt;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};

use crate::error::{PlenoraError, Result};

/// Un campo facoltativo presente: `null` non è l'assenza.
///
/// Con `#[serde(default, deserialize_with = "plenora_core::json::presente")]`
/// un campo omesso resta `None` e un `null` scritto è un errore di forma,
/// come vuole uno schema che dichiara il campo di un tipo che non è `null`:
/// letto come assente, un `null` farebbe passare in silenzio un documento che
/// il contratto rifiuta (un'attesa non verificata, un CRS non dichiarato).
///
/// # Errors
///
/// Quelli di `T::deserialize`, anche per `null`.
pub fn presente<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Verifica che nessun oggetto del documento JSON abbia chiavi ripetute.
///
/// La visita non costruisce nulla: attraversa il documento e tiene per ogni
/// oggetto il solo insieme delle sue chiavi.
///
/// Un documento sintatticamente invalido non e' un errore qui: la sintassi
/// la segnala la deserializzazione vera, che sa classificarla.
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

/// Verifica che ogni numero del documento valga esattamente quello che
/// `serde_json` ne legge.
///
/// Senza la feature `arbitrary_precision` un intero oltre la gamma di `i64`
/// e `u64` e ogni numero con parte frazionaria o esponente diventano un
/// `f64`: `9007199254740993.0` arriva come `9007199254740992`, e un
/// vincolo della config (un estremo, un bordo, una soglia) sarebbe un
/// altro numero da quello scritto, senza errore. Il numero si accetta se il
/// suo valore decimale esatto e' quello del double letto (la forma piu'
/// corta che `serde_json` riscrive: `0.1`, `1e3`, `2.5` passano), altrimenti
/// il documento si rifiuta. Chi vuole il valore del double lo scrive come
/// il double lo rende; chi vuole un valore esatto oltre il double lo scrive
/// come stringa, dove l'operazione accetta stringhe.
///
/// Scansione lessicale: le stringhe si saltano (con i loro escape), ogni
/// altro numero si confronta. Un documento sintatticamente invalido non e'
/// un errore qui: lo segnala la deserializzazione vera.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per un numero non rappresentabile
/// esattamente, senza citarlo.
pub fn ensure_numbers_exact(json_text: &str) -> Result<()> {
    let byte = json_text.as_bytes();
    let mut indice = 0;
    while indice < byte.len() {
        match byte[indice] {
            b'"' => {
                indice += 1;
                while indice < byte.len() && byte[indice] != b'"' {
                    indice += if byte[indice] == b'\\' { 2 } else { 1 };
                }
                indice += 1;
            }
            b'-' | b'0'..=b'9' => {
                let inizio = indice;
                while indice < byte.len()
                    && matches!(byte[indice], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    indice += 1;
                }
                let token = &json_text[inizio..indice];
                if !numero_esatto(token) {
                    return Err(PlenoraError::InvalidPlan(
                        "un numero del documento non e' rappresentabile esattamente come \
                         viene letto (intero oltre i64/u64, o decimale oltre la precisione \
                         del double): scriverlo come il double che lo rappresenta, o come \
                         stringa dove ammessa"
                            .into(),
                    ));
                }
            }
            _ => indice += 1,
        }
    }
    Ok(())
}

/// Il numero `token` vale esattamente quello che `serde_json` ne legge.
/// Un token che non e' un numero JSON non e' giudicato qui.
fn numero_esatto(token: &str) -> bool {
    // Un intero in gamma resta intero; fuori gamma serde_json lo legge come
    // double, e si confronta come ogni altro numero.
    if !token.contains(['.', 'e', 'E'])
        && (token.parse::<i64>().is_ok() || token.parse::<u64>().is_ok())
    {
        return true;
    }
    let Ok(letto) = serde_json::from_str::<serde_json::Number>(token) else {
        return true;
    };
    if letto.is_i64() || letto.is_u64() {
        return true;
    }
    let Some(riscritto) = letto.as_f64().and_then(serde_json::Number::from_f64) else {
        return true;
    };
    match (
        decimale_canonico(token),
        decimale_canonico(&riscritto.to_string()),
    ) {
        (Some(scritto), Some(letto)) => scritto == letto,
        _ => false,
    }
}

/// Il valore esatto di un numero decimale in forma canonica: segno, cifre
/// significative senza zeri ai bordi, esponente della cifra meno
/// significativa. Lo zero e' uno solo.
fn decimale_canonico(testo: &str) -> Option<(bool, String, i64)> {
    let (negativo, resto) = testo
        .strip_prefix('-')
        .map_or((false, testo), |resto| (true, resto));
    let (mantissa, esponente) = match resto.find(['e', 'E']) {
        Some(posizione) => (
            &resto[..posizione],
            resto[posizione + 1..].parse::<i64>().ok()?,
        ),
        None => (resto, 0),
    };
    let (intera, frazione) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut cifre = format!("{intera}{frazione}");
    let mut esponente = esponente.checked_sub(i64::try_from(frazione.len()).ok()?)?;
    let senza_zeri_iniziali = cifre.trim_start_matches('0').to_owned();
    cifre = senza_zeri_iniziali;
    while cifre.ends_with('0') {
        cifre.pop();
        esponente = esponente.checked_add(1)?;
    }
    if cifre.is_empty() {
        return Some((false, String::new(), 0));
    }
    Some((negativo, cifre, esponente))
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
    /// La canonicalizzazione riordina le chiavi e `$` precede ogni lettera,
    /// quindi una chiave in seconda posizione diventa la prima del testo
    /// canonico, l'unica che `serde_json` reinterpreta.
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
    /// `serde_json` decodifica gli escape prima di consegnare la chiave, e
    /// riconosce la chiave riservata anche scritta come `\u0024...`: il
    /// confronto avviene quindi sul nome decodificato.
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
    /// Il fuzzer dei piani del progetto d'origine lo ha prodotto in 511
    /// byte; qui sta in una riga. Senza questo rifiuto il testo si accetta e
    /// una riscrittura canonica ne rende uno che nessun lettore sa rileggere.
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
    /// `serde_json` **riesce** e rende il documento contenuto nella stringa:
    /// il testo letterale ha una chiave sola, il documento effettivo due
    /// uguali, e il controllo dei duplicati verrebbe aggirato.
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

    /// Regressione: un numero che il double non tiene si
    /// rifiuta invece di diventare un altro numero; le forme che il double
    /// rende esattamente passano.
    #[test]
    fn i_numeri_del_documento_valgono_quello_che_si_legge() {
        for esatto in [
            r#"{"a": 1, "b": -7, "c": 18446744073709551615, "d": -9223372036854775808}"#,
            r#"{"a": 0.1, "b": 2.5, "c": 1e3, "d": 1.0, "e": -0.0, "f": 9007199254740992.0}"#,
            r#"{"s": "9007199254740993.0 dentro una stringa \" 1e400", "n": [1, 2.25]}"#,
        ] {
            assert!(ensure_numbers_exact(esatto).is_ok(), "{esatto}");
        }
        for inesatto in [
            r#"{"min": 9007199254740993.0}"#,
            r#"{"x": 18446744073709551616}"#,
            r#"{"x": 0.30000000000000001}"#,
            "[1, 2, 9007199254740993e0]",
        ] {
            assert!(
                matches!(
                    ensure_numbers_exact(inesatto),
                    Err(PlenoraError::InvalidPlan(_))
                ),
                "{inesatto}"
            );
        }
    }
}
