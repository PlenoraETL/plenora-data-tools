//! Supporto dei test della CLI: il binario come sottoprocesso, i file dei
//! contratti copiati (con lo SHA-256 verificato) e un validatore JSON Schema
//! minimo.
//!
//! Il validatore è quello di `plenora-core/tests/errore_pubblico_schema.rs`
//! con due aggiunte che gli schemi della CLI chiedono: `$ref` verso un altro
//! schema per `$id` (l'inviluppo rimanda a `error-v1`) e `minItems`. Come
//! l'originale, una parola chiave che non conosce fa fallire la validazione
//! invece di essere ignorata. Niente crate di validazione: la verifica
//! indipendente con `jsonschema` è `scripts/verifica_cli_contratti.py`.

#![allow(dead_code)] // Ogni file di test usa una parte del supporto.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use plenora_core::esadecimale::esadecimale;
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Il binario.
// ---------------------------------------------------------------------------

/// Esito di un'invocazione: codice, documento JSON, testo di stdout.
pub struct Esito {
    pub codice: i32,
    pub documento: Value,
    pub stdout: String,
}

pub fn binario() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_plenora-data"))
}

pub fn processo(argomenti: &[&str]) -> Output {
    Command::new(binario())
        .args(argomenti)
        .output()
        .expect("avvio del binario")
}

/// Invoca il binario e verifica le regole di flusso di CLI 2.0 (sezione 4):
/// stderr vuoto, un solo documento JSON seguito da un a capo, exit 0 se e
/// solo se `status` è `ok`. Il documento si valida contro lo schema
/// dell'inviluppo.
pub fn invoca(argomenti: &[&str]) -> Esito {
    esito(&processo(argomenti))
}

pub fn esito(uscita: &Output) -> Esito {
    assert!(
        uscita.stderr.is_empty(),
        "stderr non vuoto: {}",
        String::from_utf8_lossy(&uscita.stderr)
    );
    let stdout = String::from_utf8(uscita.stdout.clone()).expect("stdout UTF-8");
    assert!(stdout.ends_with('\n'), "manca l'a capo finale");
    assert_eq!(
        stdout.matches('\n').count(),
        1,
        "un solo documento: {stdout}"
    );
    let documento: Value = serde_json::from_str(stdout.trim_end()).expect("stdout JSON");
    let codice = uscita.status.code().expect("codice d'uscita");
    let ok = documento["status"] == "ok";
    assert_eq!(codice == 0, ok, "exit 0 se e solo se ok: {codice} {stdout}");
    valida_inviluppo(&documento).unwrap_or_else(|motivo| panic!("inviluppo: {motivo}\n{stdout}"));
    if !ok {
        verifica_limiti_errore(&documento["error"]);
    }
    Esito {
        codice,
        documento,
        stdout,
    }
}

// ---------------------------------------------------------------------------
// I file dei contratti.
// ---------------------------------------------------------------------------

fn cartella_contratti() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/contratti")
}

/// Un file dei contratti, dopo averne verificato lo SHA-256 contro
/// `provenienza.json`.
pub fn contratto(nome: &str) -> Value {
    let cartella = cartella_contratti();
    let provenienza: Value = serde_json::from_slice(
        &std::fs::read(cartella.join("provenienza.json")).expect("provenienza.json"),
    )
    .expect("provenienza JSON");
    let atteso = provenienza["file"][nome][1]
        .as_str()
        .unwrap_or_else(|| panic!("{nome} senza provenienza"));
    let byte = std::fs::read(cartella.join(nome)).expect("file dei contratti");
    assert_eq!(
        esadecimale(&Sha256::digest(&byte)),
        atteso,
        "{nome}: SHA-256 diverso dal commit dei contratti"
    );
    serde_json::from_slice(&byte).expect("JSON dei contratti")
}

/// Ogni file della cartella è in `provenienza.json` e viceversa.
pub fn provenienza_completa() {
    let cartella = cartella_contratti();
    let provenienza: Value = serde_json::from_slice(
        &std::fs::read(cartella.join("provenienza.json")).expect("provenienza.json"),
    )
    .expect("provenienza JSON");
    let dichiarati: Vec<String> = provenienza["file"]
        .as_object()
        .expect("file")
        .keys()
        .cloned()
        .collect();
    let mut presenti: Vec<String> = std::fs::read_dir(&cartella)
        .expect("cartella")
        .map(|voce| {
            voce.expect("voce")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|nome| nome != "provenienza.json")
        .collect();
    presenti.sort();
    assert_eq!(presenti, dichiarati);
    for nome in &dichiarati {
        let _ = contratto(nome);
    }
}

// ---------------------------------------------------------------------------
// Validatore JSON Schema minimo (draft 2020-12, sottoinsieme degli schemi).
// ---------------------------------------------------------------------------

const ANNOTAZIONI: &[&str] = &[
    "$schema", "$id", "title", "$defs", "then", "else", "$comment",
];
const STRUTTURALE: &str = "schema: ";

/// Gli schemi per `$id`, per i `$ref` fra schemi.
pub struct Registro(BTreeMap<String, Value>);

impl Registro {
    pub fn dei_contratti() -> Self {
        let mut per_id = BTreeMap::new();
        for nome in [
            "cli-envelope-v2.schema.json",
            "error-v1.schema.json",
            "capabilities-v2.schema.json",
            "row-diagnostics-v1.schema.json",
            "operation-registry-v1.schema.json",
            "surface-bindings-v1.schema.json",
        ] {
            let schema = contratto(nome);
            let id = schema["$id"].as_str().expect("$id").to_owned();
            per_id.insert(id, schema);
        }
        Self(per_id)
    }

    /// Valida `istanza` contro lo schema con `$id` che finisce con `nome`.
    pub fn valida(&self, nome: &str, istanza: &Value) -> Result<(), String> {
        let radice = self
            .0
            .iter()
            .find(|(id, _)| id.ends_with(nome))
            .map(|(_, schema)| schema)
            .ok_or_else(|| format!("schema {nome} assente"))?;
        valida(self, radice, radice, istanza, "")
    }
}

fn ramo(esito: Result<(), String>) -> Result<bool, String> {
    match esito {
        Ok(()) => Ok(true),
        Err(motivo) if motivo.starts_with(STRUTTURALE) => Err(motivo),
        Err(_) => Ok(false),
    }
}

#[allow(clippy::too_many_lines)]
fn valida(
    registro: &Registro,
    radice: &Value,
    schema: &Value,
    istanza: &Value,
    dove: &str,
) -> Result<(), String> {
    let strutturale = |motivo: &str| format!("{STRUTTURALE}{dove}: {motivo}");
    let Some(oggetto) = schema.as_object() else {
        return Err(strutturale("schema non oggetto"));
    };
    let errore = |motivo: &str| Err(format!("{dove}: {motivo}"));
    for (parola, valore) in oggetto {
        match parola.as_str() {
            parola if ANNOTAZIONI.contains(&parola) => {}
            "$ref" => {
                let riferimento = valore.as_str().ok_or_else(|| strutturale("$ref"))?;
                if let Some(nome) = riferimento.strip_prefix("#/$defs/") {
                    let definizione = &radice["$defs"][nome];
                    if definizione.is_null() {
                        return Err(strutturale("$ref senza definizione"));
                    }
                    valida(registro, radice, definizione, istanza, dove)?;
                } else {
                    let altro = registro
                        .0
                        .get(riferimento)
                        .ok_or_else(|| strutturale("$ref verso uno schema assente"))?;
                    valida(registro, altro, altro, istanza, dove)?;
                }
            }
            "type" => {
                let tipi: Vec<&str> = match valore {
                    Value::String(tipo) => vec![tipo.as_str()],
                    Value::Array(tipi) => tipi.iter().filter_map(Value::as_str).collect(),
                    _ => return Err(strutturale("type non valido")),
                };
                if !tipi.iter().any(|tipo| ha_tipo(istanza, tipo)) {
                    return errore(&format!("tipo diverso da {tipi:?}"));
                }
            }
            "enum" => {
                if !valore
                    .as_array()
                    .is_some_and(|ammessi| ammessi.contains(istanza))
                {
                    return errore("valore fuori da enum");
                }
            }
            "const" => {
                if valore != istanza {
                    return errore("valore diverso da const");
                }
            }
            "required" => {
                if let Some(oggetto) = istanza.as_object() {
                    for nome in valore.as_array().into_iter().flatten() {
                        let nome = nome.as_str().unwrap_or_default();
                        if !oggetto.contains_key(nome) {
                            return errore(&format!("manca `{nome}`"));
                        }
                    }
                }
            }
            "properties" => {
                if let (Some(oggetto), Some(proprieta)) = (istanza.as_object(), valore.as_object())
                {
                    for (nome, figlio) in oggetto {
                        if let Some(sotto) = proprieta.get(nome) {
                            valida(registro, radice, sotto, figlio, &format!("{dove}/{nome}"))?;
                        }
                    }
                }
            }
            "additionalProperties" => {
                if let Some(oggetto) = istanza.as_object() {
                    let dichiarate = schema.get("properties").and_then(Value::as_object);
                    for (nome, figlio) in oggetto {
                        if dichiarate.is_some_and(|dichiarate| dichiarate.contains_key(nome)) {
                            continue;
                        }
                        match valore {
                            Value::Bool(false) => {
                                return errore(&format!("proprieta' non ammessa `{nome}`"))
                            }
                            Value::Bool(true) => {}
                            sotto => {
                                valida(registro, radice, sotto, figlio, &format!("{dove}/{nome}"))?;
                            }
                        }
                    }
                }
            }
            "propertyNames" => {
                if let Some(oggetto) = istanza.as_object() {
                    for nome in oggetto.keys() {
                        valida(
                            registro,
                            radice,
                            valore,
                            &json!(nome),
                            &format!("{dove}/<{nome}>"),
                        )?;
                    }
                }
            }
            "pattern" => {
                if let Some(testo) = istanza.as_str() {
                    let espressione = Regex::new(valore.as_str().unwrap_or_default())
                        .map_err(|_| strutturale("pattern non compilabile"))?;
                    if !espressione.is_match(testo) {
                        return errore("pattern non soddisfatto");
                    }
                }
            }
            "minLength" | "maxLength" => {
                if let Some(testo) = istanza.as_str() {
                    let caratteri = u64::try_from(testo.chars().count()).unwrap_or(u64::MAX);
                    let limite = valore.as_u64().unwrap_or_default();
                    if (parola == "minLength" && caratteri < limite)
                        || (parola == "maxLength" && caratteri > limite)
                    {
                        return errore(parola);
                    }
                }
            }
            "minItems" | "maxItems" => {
                if let Some(elementi) = istanza.as_array() {
                    let numero = u64::try_from(elementi.len()).unwrap_or(u64::MAX);
                    let limite = valore.as_u64().unwrap_or_default();
                    if (parola == "minItems" && numero < limite)
                        || (parola == "maxItems" && numero > limite)
                    {
                        return errore(parola);
                    }
                }
            }
            "minimum" | "maximum" => {
                if let (Some(numero), Some(limite)) = (istanza.as_f64(), valore.as_f64()) {
                    if (parola == "minimum" && numero < limite)
                        || (parola == "maximum" && numero > limite)
                    {
                        return errore(parola);
                    }
                }
            }
            "items" => {
                for (indice, elemento) in istanza.as_array().into_iter().flatten().enumerate() {
                    valida(
                        registro,
                        radice,
                        valore,
                        elemento,
                        &format!("{dove}/{indice}"),
                    )?;
                }
            }
            "uniqueItems" => {
                if let (Some(elementi), Some(true)) = (istanza.as_array(), valore.as_bool()) {
                    for (indice, elemento) in elementi.iter().enumerate() {
                        if elementi[..indice].contains(elemento) {
                            return errore("elementi ripetuti");
                        }
                    }
                }
            }
            "oneOf" | "allOf" | "anyOf" => {
                let sotto = valore.as_array().map_or(&[][..], Vec::as_slice);
                let mut valide = 0;
                for sotto in sotto {
                    valide += usize::from(ramo(valida(registro, radice, sotto, istanza, dove))?);
                }
                if (parola == "oneOf" && valide != 1)
                    || (parola == "allOf" && valide != sotto.len())
                    || (parola == "anyOf" && valide == 0)
                {
                    return errore(&format!("{parola}: {valide} di {} valide", sotto.len()));
                }
            }
            "not" => {
                if ramo(valida(registro, radice, valore, istanza, dove))? {
                    return errore("not soddisfatto");
                }
            }
            "if" => {
                let seguito = if ramo(valida(registro, radice, valore, istanza, dove))? {
                    schema.get("then")
                } else {
                    schema.get("else")
                };
                if let Some(seguito) = seguito {
                    valida(registro, radice, seguito, istanza, dove)?;
                }
            }
            sconosciuta => {
                return Err(strutturale(&format!(
                    "parola chiave non supportata `{sconosciuta}`"
                )))
            }
        }
    }
    Ok(())
}

fn ha_tipo(istanza: &Value, tipo: &str) -> bool {
    match tipo {
        "object" => istanza.is_object(),
        "array" => istanza.is_array(),
        "string" => istanza.is_string(),
        "boolean" => istanza.is_boolean(),
        "null" => istanza.is_null(),
        "number" => istanza.is_number(),
        "integer" => istanza.is_i64() || istanza.is_u64(),
        _ => false,
    }
}

pub fn valida_inviluppo(documento: &Value) -> Result<(), String> {
    let registro = Registro::dei_contratti();
    registro.valida("cli-envelope-v2.schema.json", documento)?;
    if let Some(diagnostica) = documento.pointer("/error/details/row_diagnostics") {
        registro.valida("row-diagnostics-v1.schema.json", diagnostica)?;
    }
    Ok(())
}

/// ERR-011 ed ERR-012, che lo schema non esprime: byte dell'errore e di
/// `details`, profondità, ventaglio, stringhe e nodi.
pub fn verifica_limiti_errore(errore: &Value) {
    assert!(errore.to_string().len() <= 524_288, "ERR-011: errore");
    let Some(details) = errore.get("details") else {
        return;
    };
    assert!(details.to_string().len() <= 262_144, "ERR-011: details");
    let mut nodi = 0_usize;
    let mut pila = vec![(details, 1_usize)];
    while let Some((valore, profondita)) = pila.pop() {
        nodi += 1;
        assert!(profondita <= 8, "ERR-012: profondita'");
        match valore {
            Value::Object(campi) => {
                assert!(campi.len() <= 128, "ERR-012: proprieta'");
                pila.extend(campi.values().map(|figlio| (figlio, profondita + 1)));
            }
            Value::Array(elementi) => {
                assert!(elementi.len() <= 128, "ERR-012: elementi");
                pila.extend(elementi.iter().map(|figlio| (figlio, profondita + 1)));
            }
            Value::String(testo) => assert!(testo.len() <= 4_096, "ERR-012: stringa"),
            _ => {}
        }
    }
    assert!(nodi <= 2_048, "ERR-012: nodi");
}
