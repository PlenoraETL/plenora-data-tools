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

/// Messaggio di un parametro facoltativo scritto `null` dove `null` non e'
/// l'assenza (`mai_null` dei kernel tabellari).
pub const MESSAGGIO_NULL_NON_AMMESSO: &str = "null non ammesso: un parametro facoltativo si omette";

/// Messaggio di un numero della config che `NumeroConfig` dei kernel
/// tabellari non rappresenta esattamente.
pub const MESSAGGIO_NUMERO_NON_ESATTO: &str =
    "numero non rappresentabile esattamente (oltre 38 cifre significative o scala)";

/// Messaggio di `include_unchanged` di `table.table_diff` fuori dominio.
pub const MESSAGGIO_INCLUDE_UNCHANGED: &str = "include_unchanged ammette solo \"yes\" o \"no\"";

/// I messaggi `custom` di serde scritti da noi, statici e senza valori: gli
/// unici che [`descrivi_errore_config`] riporta per intero. Un messaggio
/// custom nuovo che deve arrivare a chi scrive il piano si aggiunge qui.
const MESSAGGI_STATICI: &[&str] = &[
    MESSAGGIO_NULL_NON_AMMESSO,
    MESSAGGIO_NUMERO_NON_ESATTO,
    MESSAGGIO_INCLUDE_UNCHANGED,
];

/// Descrive un errore di `serde_json` su una config senza il testo di
/// terzi.
///
/// Il testo di serde cita il valore letto (``invalid type: integer `7`,
/// expected a string``, il nome scritto di `unknown variant` e `unknown
/// field`): un valore scritto nel piano non deve attraversare il messaggio
/// pubblico (AGENTS.md, «Errori senza dati»). Delle forme di serde 1.0.229
/// si tiene solo cio' che viene dal codice e non dal documento:
///
/// - `missing field` e `duplicate field`: il nome del campo, che serde
///   prende dalla struttura;
/// - `unknown field` e `unknown variant`: l'elenco dei nomi ammessi, mai il
///   nome scritto;
/// - i messaggi custom del registro `MESSAGGI_STATICI` (le costanti
///   `MESSAGGIO_*` di questo modulo), per intero.
///
/// Un nome si riporta solo se e' fatto dei caratteri dei nomi del codice
/// (`[A-Za-z0-9_$-]` e gli operatori `=<>!`, non vuoto). L'elenco si legge
/// dalla fine e si ferma al primo `, expected` da destra, che e' quello di
/// serde: il nome scritto, che sta prima, non entra mai nell'elenco; il
/// controllo dei caratteri e' una seconda difesa. Tutto il resto
/// (tipo, valore o lunghezza non validi, messaggi custom fuori registro)
/// diventa il solo genere di [`serde_json::error::Category`], con riga e
/// colonna quando il documento era testo (una config letta da un `Value`
/// non ha posizione).
///
/// Il contesto che la regola di `error.rs` ammette resta nei messaggi dei
/// chiamanti: il passo, l'operazione, la colonna per nome, il nome di una
/// regola, il motivo. Non e' un valore scritto: e' quello che serve per
/// trovare l'errore nel piano.
#[must_use]
pub fn descrivi_errore_config(errore: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let testo = errore.to_string();
    // `serde_json` aggiunge la posizione in coda quando la riga e' nota.
    let coda = format!(" at line {} column {}", errore.line(), errore.column());
    let messaggio = if errore.line() == 0 {
        testo.as_str()
    } else {
        testo.strip_suffix(coda.as_str()).unwrap_or(testo.as_str())
    };
    if errore.classify() == Category::Data {
        if let Some(descrizione) = descrivi_forma_nota(messaggio) {
            return descrizione;
        }
    }
    let genere = match errore.classify() {
        Category::Io => "lettura non riuscita",
        Category::Syntax => "sintassi JSON non valida",
        Category::Data => "tipo, valore o forma di un campo non validi",
        Category::Eof => "documento JSON incompleto",
    };
    if errore.line() == 0 {
        genere.to_owned()
    } else {
        format!(
            "{genere} alla riga {} colonna {}",
            errore.line(),
            errore.column()
        )
    }
}

/// Le forme di serde di cui si tiene la parte statica; `None` per tutte le
/// altre.
fn descrivi_forma_nota(messaggio: &str) -> Option<String> {
    if MESSAGGI_STATICI.contains(&messaggio) {
        return Some(messaggio.to_owned());
    }
    let nome_singolo = |prefisso: &str| {
        messaggio
            .strip_prefix(prefisso)
            .and_then(|resto| resto.strip_suffix('`'))
            .filter(|nome| identificatore(nome))
    };
    if let Some(nome) = nome_singolo("missing field `") {
        return Some(format!("campo obbligatorio assente: `{nome}`"));
    }
    if let Some(nome) = nome_singolo("duplicate field `") {
        return Some(format!("campo ripetuto: `{nome}`"));
    }
    for (prefisso, sconosciuto, nessuno, ammessi) in [
        (
            "unknown field `",
            "campo sconosciuto",
            ", there are no fields",
            "campi ammessi",
        ),
        (
            "unknown variant `",
            "valore sconosciuto",
            ", there are no variants",
            "valori ammessi",
        ),
    ] {
        if !messaggio.starts_with(prefisso) {
            continue;
        }
        if messaggio.ends_with(nessuno) {
            return Some(format!("{sconosciuto}: nessuno ammesso"));
        }
        return Some(nomi_attesi(messaggio).map_or_else(
            || sconosciuto.to_owned(),
            |nomi| format!("{sconosciuto}; {ammessi}: {}", nomi.join(", ")),
        ));
    }
    None
}

/// I nomi dell'elenco `expected ...` in coda a un `unknown field` o
/// `unknown variant`, letti dalla fine: il nome scritto, che precede, puo'
/// contenere qualunque testo. Le forme di `serde::de::OneOf` sono
/// `` `a` ``, `` `a` or `b` `` e `` one of `a`, `b`, `c` ``; ogni nome si
/// restituisce fra backtick, `None` se la coda non ha una di queste forme
/// esatte.
fn nomi_attesi(messaggio: &str) -> Option<Vec<String>> {
    let mut resto = messaggio;
    let mut elenco = Vec::new();
    let mut giunzioni: Vec<&str> = Vec::new();
    loop {
        let senza_chiusa = resto.strip_suffix('`')?;
        let apertura = senza_chiusa.rfind('`')?;
        let nome = &senza_chiusa[apertura + 1..];
        if !identificatore(nome) {
            return None;
        }
        elenco.push(format!("`{nome}`"));
        resto = &senza_chiusa[..apertura];
        if resto.ends_with(", expected one of ") {
            // Tre o piu' nomi, tutti separati da virgola.
            if elenco.len() < 3 || giunzioni.iter().any(|separatore| *separatore != ", ") {
                return None;
            }
            break;
        }
        if resto.ends_with(", expected ") {
            // Un nome, o due separati da `or`.
            let valido = match elenco.len() {
                1 => true,
                2 => giunzioni == [" or "],
                _ => false,
            };
            if !valido {
                return None;
            }
            break;
        }
        let separatore = [", ", " or "]
            .into_iter()
            .find(|separatore| resto.ends_with(separatore))?;
        giunzioni.push(separatore);
        resto = &resto[..resto.len() - separatore.len()];
    }
    elenco.reverse();
    Some(elenco)
}

/// Un nome che serde prende dal codice: non vuoto, solo `[A-Za-z0-9_$-]`
/// e i caratteri degli operatori rinominati (`==`, `!=`, `<=`, ...).
fn identificatore(nome: &str) -> bool {
    !nome.is_empty()
        && nome.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'$' | b'-' | b'=' | b'<' | b'>' | b'!')
        })
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
/// con un messaggio fisso e la posizione nel documento. La chiave ripetuta
/// non si nomina: e' testo scritto dall'utente, e in un documento di dati
/// (le chiavi di `mapping` di `table.lookup`) e' un valore.
pub fn ensure_no_duplicate_keys(json_text: &str) -> Result<()> {
    let mut deserializer = serde_json::Deserializer::from_str(json_text);
    let esito = serde::de::DeserializeSeed::deserialize(UniqueKeys, &mut deserializer);
    // `Category::Data` e' la classe degli errori prodotti dal visitatore, cioe'
    // esattamente la chiave duplicata; sintassi ed EOF spettano al parse vero e
    // qui non sono un errore.
    // Il messaggio si ricompone dalle costanti: il testo dell'errore serde
    // non attraversa il confine nemmeno quando e' nostro.
    if let Err(error) = esito {
        if error.classify() == serde_json::error::Category::Data {
            let motivo = if error.to_string().starts_with(MESSAGGIO_CHIAVE_RISERVATA) {
                MESSAGGIO_CHIAVE_RISERVATA
            } else {
                MESSAGGIO_CHIAVE_DUPLICATA
            };
            return Err(PlenoraError::InvalidPlan(format!(
                "{motivo} (riga {} colonna {})",
                error.line(),
                error.column()
            )));
        }
    }
    Ok(())
}

/// Rifiuto di una chiave ripetuta nello stesso oggetto: fisso, senza la
/// chiave.
const MESSAGGIO_CHIAVE_DUPLICATA: &str = "chiave JSON duplicata nello stesso oggetto: il \
     documento e' ambiguo e non viene risolto con «vince l'ultima»";

/// Rifiuto della chiave riservata di `serde_json`. Nomina la chiave perche'
/// e' una costante del formato, non un testo dell'utente.
const MESSAGGIO_CHIAVE_RISERVATA: &str = "chiave JSON riservata \
     `$serde_json::private::RawValue`: `serde_json` la usa per trasportare JSON grezzo e \
     non la legge come una chiave, quindi il documento non e' quello che dichiara di essere";

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
                return Err(serde::de::Error::custom(MESSAGGIO_CHIAVE_RISERVATA));
            }
            if !seen.insert(key) {
                return Err(serde::de::Error::custom(MESSAGGIO_CHIAVE_DUPLICATA));
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

    /// Una config nel tipo `T` letta da un `Value`, e l'errore descritto.
    fn descritto<T: serde::de::DeserializeOwned + std::fmt::Debug>(
        config: serde_json::Value,
    ) -> String {
        let errore = serde_json::from_value::<T>(config).expect_err("config rifiutata");
        descrivi_errore_config(&errore)
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)] // I campi servono solo alla forma della config.
    struct TreCampi {
        alfa: String,
        beta: Option<u32>,
        gamma: Option<Modo>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct DueCampi {
        alfa: u32,
        beta: Option<u32>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct UnCampo {
        alfa: u32,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Vuota {}

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Modo {
        Primo,
        Secondo,
        Terzo,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum DueModi {
        Primo,
        Secondo,
    }

    /// Il sentinella che nessun messaggio deve contenere.
    const SENTINELLA: &str = "VALORE_SEGRETO_42";

    /// **Il campo mancante si nomina: il nome viene dalla struttura.**
    #[test]
    fn il_campo_mancante_si_nomina() {
        assert_eq!(
            descritto::<TreCampi>(serde_json::json!({})),
            "campo obbligatorio assente: `alfa`"
        );
    }

    /// **Il campo ripetuto si nomina** (da una mappa che serde non
    /// deduplica: il testo).
    #[test]
    fn il_campo_ripetuto_si_nomina() {
        let errore = serde_json::from_str::<UnCampo>(r#"{"alfa": 1, "alfa": 2}"#)
            .expect_err("campo ripetuto");
        assert_eq!(descrivi_errore_config(&errore), "campo ripetuto: `alfa`");
    }

    /// **Il campo sconosciuto non si cita: si elencano gli ammessi**, in
    /// tutte le forme di `OneOf` (uno, due, tre o piu', nessuno).
    #[test]
    fn il_campo_sconosciuto_elenca_gli_ammessi_e_non_se_stesso() {
        let casi = [
            (
                descritto::<TreCampi>(serde_json::json!({ SENTINELLA: 1 })),
                "campo sconosciuto; campi ammessi: `alfa`, `beta`, `gamma`",
            ),
            (
                descritto::<DueCampi>(serde_json::json!({ SENTINELLA: 1 })),
                "campo sconosciuto; campi ammessi: `alfa`, `beta`",
            ),
            (
                descritto::<UnCampo>(serde_json::json!({ SENTINELLA: 1 })),
                "campo sconosciuto; campi ammessi: `alfa`",
            ),
            (
                descritto::<Vuota>(serde_json::json!({ SENTINELLA: 1 })),
                "campo sconosciuto: nessuno ammesso",
            ),
        ];
        for (avuto, atteso) in casi {
            assert_eq!(avuto, atteso);
            assert!(!avuto.contains(SENTINELLA), "{avuto}");
        }
    }

    /// **La variante sconosciuta non si cita: si elencano le ammesse.**
    #[test]
    fn la_variante_sconosciuta_elenca_le_ammesse_e_non_se_stessa() {
        let tre = descritto::<TreCampi>(serde_json::json!({"alfa": "a", "gamma": SENTINELLA}));
        assert_eq!(
            tre,
            "valore sconosciuto; valori ammessi: `primo`, `secondo`, `terzo`"
        );
        let due = serde_json::from_value::<DueModi>(serde_json::json!(SENTINELLA))
            .expect_err("variante sconosciuta");
        assert_eq!(
            descrivi_errore_config(&due),
            "valore sconosciuto; valori ammessi: `primo`, `secondo`"
        );
    }

    /// **Tipo e valore non validi: solo il genere.** Il testo di serde
    /// citerebbe l'intero `7` e la stringa scritta.
    #[test]
    fn tipo_e_valore_non_validi_non_citano_il_valore() {
        for (config, scritto) in [
            (serde_json::json!({"alfa": 7}), "7"),
            (
                serde_json::json!({"alfa": "a", "beta": SENTINELLA}),
                SENTINELLA,
            ),
            (serde_json::json!({"alfa": "a", "beta": -12345}), "12345"),
        ] {
            let avuto = descritto::<TreCampi>(config);
            assert_eq!(avuto, "tipo, valore o forma di un campo non validi");
            assert!(!avuto.contains(scritto), "{avuto}");
        }
    }

    /// **Una chiave che imita la coda di serde non passa nel messaggio.**
    ///
    /// Il nome scritto precede l'elenco e puo' contenere backtick e
    /// `, expected`: l'elenco si legge dalla fine, e solo identificatori.
    #[test]
    fn una_chiave_che_imita_serde_non_entra_nel_messaggio() {
        for chiave in [
            format!("{SENTINELLA}`, expected `x"),
            format!("x`, expected one of `{SENTINELLA} spazio`, `y"),
            format!("{SENTINELLA}`"),
            "`".to_owned(),
            format!("a` or `{SENTINELLA}"),
        ] {
            let avuto = descritto::<TreCampi>(serde_json::json!({ chiave.clone(): 1 }));
            assert!(
                avuto == "campo sconosciuto; campi ammessi: `alfa`, `beta`, `gamma`",
                "{chiave}: {avuto}"
            );
            assert!(!avuto.contains(SENTINELLA), "{chiave}: {avuto}");
        }
    }

    /// **Una coda che non e' una forma esatta di serde non da' elenco.**
    #[test]
    fn una_coda_irregolare_non_da_elenco() {
        assert_eq!(
            nomi_attesi("unknown field `x`, expected `a` or `b` or `c`"),
            None
        );
        assert_eq!(
            nomi_attesi("unknown field `x`, expected one of `a`, `b`"),
            None
        );
        assert_eq!(nomi_attesi("unknown field `x`, expected `a`, `b`"), None);
        assert_eq!(nomi_attesi("unknown field `x`, expected `a b`"), None);
        assert_eq!(
            nomi_attesi("unknown variant `x`, expected `==` or `!=`"),
            Some(vec!["`==`".to_owned(), "`!=`".to_owned()])
        );
        assert_eq!(nomi_attesi("unknown field `x`, expected ``"), None);
        assert_eq!(
            nomi_attesi("unknown field `x`, expected one of `a`, `b`, `c`"),
            Some(vec!["`a`".to_owned(), "`b`".to_owned(), "`c`".to_owned()])
        );
    }

    /// **I messaggi custom del registro passano interi, gli altri no.**
    #[test]
    fn solo_i_messaggi_custom_del_registro_passano() {
        use serde::de::Error as _;
        for statico in MESSAGGI_STATICI {
            let errore = serde_json::Error::custom(statico);
            assert_eq!(descrivi_errore_config(&errore), *statico);
        }
        let estraneo = serde_json::Error::custom(format!("pattern {SENTINELLA} non valido"));
        let avuto = descrivi_errore_config(&estraneo);
        assert_eq!(avuto, "tipo, valore o forma di un campo non validi");
    }

    /// **Dal testo: sintassi e fine inattesa con la sola posizione.**
    #[test]
    fn il_testo_porta_solo_genere_e_posizione() {
        let sintassi = serde_json::from_str::<UnCampo>("{\"alfa\": x}").expect_err("sintassi");
        assert_eq!(
            descrivi_errore_config(&sintassi),
            "sintassi JSON non valida alla riga 1 colonna 10"
        );
        let fine = serde_json::from_str::<UnCampo>("{\"alfa\": ").expect_err("fine");
        assert!(descrivi_errore_config(&fine).starts_with("documento JSON incompleto alla riga 1"));
        let tipo =
            serde_json::from_str::<TreCampi>(&format!("{{\"alfa\": 7, \"{SENTINELLA}\": 1}}"))
                .expect_err("tipo");
        let avuto = descrivi_errore_config(&tipo);
        assert!(
            avuto.starts_with("tipo, valore o forma di un campo non validi alla riga 1"),
            "{avuto}"
        );
        // Dal testo la forma nota resta riconosciuta: la posizione si toglie.
        let mancante = serde_json::from_str::<TreCampi>("{}").expect_err("mancante");
        assert_eq!(
            descrivi_errore_config(&mancante),
            "campo obbligatorio assente: `alfa`"
        );
    }

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
            // La chiave scritta non entra nel messaggio.
            assert!(!error.to_string().contains('`'), "{text}: {error}");
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
