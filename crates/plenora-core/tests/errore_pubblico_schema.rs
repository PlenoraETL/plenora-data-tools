//! La proiezione pubblica di `PlenoraError` e la diagnostica per riga contro
//! gli schemi JSON del contratto (`plenora-contracts`).
//!
//! Gli schemi sono copiati in `tests/fixtures/contratti/` da
//! `plenora-contracts` al commit `ade868cf89c6652cffe20019e7194b383384ee78`
//! (`schemas/error-v1.schema.json`, `schemas/row-diagnostics-v1.schema.json`):
//! il test ne verifica lo SHA-256, così una copia cambiata a mano non passa.
//!
//! Il validatore è minimo e scritto qui: copre le sole parole chiave che i
//! due schemi usano, e una parola chiave che non conosce fa fallire il test
//! invece di essere ignorata. Niente crate di validazione JSON Schema (una
//! dipendenza nuova con un albero intero per due schemi); `regex` e `sha2`
//! sono già nel lockfile.

use std::time::Duration;

use plenora_core::diagnostics::RowDiagnostics;
use plenora_core::error::{PublicError, CODE_DETAILS_NOT_PUBLISHABLE};
use plenora_core::esadecimale::esadecimale;
use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SCHEMA_ERRORE: &str = include_str!("fixtures/contratti/error-v1.schema.json");
const SCHEMA_DIAGNOSTICA: &str = include_str!("fixtures/contratti/row-diagnostics-v1.schema.json");
const SHA256_ERRORE: &str = "15795d92adea7a6df15dd25a045699c43ad1c79949702c70158f4725f4fed4c0";
const SHA256_DIAGNOSTICA: &str = "b8765b1c5cc87325a8107ca355da51a6b0a6ba5a18b0e4b9674305e4dcb2992a";

// ---------------------------------------------------------------------------
// Validatore JSON Schema minimo (draft 2020-12, sottoinsieme degli schemi).
// ---------------------------------------------------------------------------

/// Annotazioni e definizioni: non vincolano l'istanza.
const ANNOTAZIONI: &[&str] = &["$schema", "$id", "title", "$defs", "then", "else"];

/// Prefisso di un errore dello schema (non dell'istanza): attraversa
/// `oneOf`, `allOf`, `not` e `if`, che altrimenti lo leggerebbero come
/// «ramo non valido» (e `not` come documento accettato).
const STRUTTURALE: &str = "schema: ";

/// Esito di un sotto-schema dentro un combinatore: `Ok(valido)`, o l'errore
/// dello schema che lo rende inutilizzabile.
fn ramo(esito: Result<(), String>) -> Result<bool, String> {
    match esito {
        Ok(()) => Ok(true),
        Err(motivo) if motivo.starts_with(STRUTTURALE) => Err(motivo),
        Err(_) => Ok(false),
    }
}

// Un ramo per parola chiave, in un punto solo: si rilegge accanto agli schemi.
#[allow(clippy::too_many_lines)]
fn valida(radice: &Value, schema: &Value, istanza: &Value, dove: &str) -> Result<(), String> {
    let strutturale = |motivo: &str| format!("{STRUTTURALE}{dove}: {motivo}");
    let Some(oggetto) = schema.as_object() else {
        return Err(strutturale("schema non oggetto"));
    };
    let errore = |motivo: &str| Err(format!("{dove}: {motivo}"));
    for (parola, valore) in oggetto {
        match parola.as_str() {
            parola if ANNOTAZIONI.contains(&parola) => {}
            "$ref" => {
                let nome = valore
                    .as_str()
                    .and_then(|riferimento| riferimento.strip_prefix("#/$defs/"))
                    .ok_or_else(|| strutturale("$ref non locale"))?;
                let definizione = &radice["$defs"][nome];
                if definizione.is_null() {
                    return Err(strutturale("$ref senza definizione"));
                }
                valida(radice, definizione, istanza, dove)?;
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
                            valida(radice, sotto, figlio, &format!("{dove}/{nome}"))?;
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
                            sotto => valida(radice, sotto, figlio, &format!("{dove}/{nome}"))?,
                        }
                    }
                }
            }
            "propertyNames" => {
                if let Some(oggetto) = istanza.as_object() {
                    for nome in oggetto.keys() {
                        valida(radice, valore, &json!(nome), &format!("{dove}/<{nome}>"))?;
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
                    valida(radice, valore, elemento, &format!("{dove}/{indice}"))?;
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
            "oneOf" | "allOf" => {
                let sotto = valore.as_array().map_or(&[][..], Vec::as_slice);
                let mut valide = 0;
                for sotto in sotto {
                    valide += usize::from(ramo(valida(radice, sotto, istanza, dove))?);
                }
                if (parola == "oneOf" && valide != 1)
                    || (parola == "allOf" && valide != sotto.len())
                {
                    return errore(&format!("{parola}: {valide} di {} valide", sotto.len()));
                }
            }
            "not" => {
                if ramo(valida(radice, valore, istanza, dove))? {
                    return errore("not soddisfatto");
                }
            }
            "if" => {
                let seguito = if ramo(valida(radice, valore, istanza, dove))? {
                    schema.get("then")
                } else {
                    schema.get("else")
                };
                if let Some(seguito) = seguito {
                    valida(radice, seguito, istanza, dove)?;
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

fn schema(testo: &str) -> Value {
    serde_json::from_str(testo).expect("schema JSON")
}

fn valida_errore(documento: &Value) -> Result<(), String> {
    let radice = schema(SCHEMA_ERRORE);
    valida(&radice, &radice, documento, "")?;
    // `details.row_diagnostics` e' `type: object` nello schema d'errore: il
    // documento si valida contro il proprio schema.
    if let Some(diagnostica) = documento.pointer("/details/row_diagnostics") {
        valida_diagnostica(diagnostica)?;
    }
    Ok(())
}

fn valida_diagnostica(documento: &Value) -> Result<(), String> {
    let radice = schema(SCHEMA_DIAGNOSTICA);
    valida(&radice, &radice, documento, "")
}

fn proiezione(errore: &PlenoraError) -> (PublicError, Value) {
    let pubblico = errore.public_projection();
    let documento = serde_json::to_value(&pubblico).expect("proiezione serializzabile");
    (pubblico, documento)
}

// ---------------------------------------------------------------------------
// Provenienza delle fixture.
// ---------------------------------------------------------------------------

#[test]
fn le_fixture_sono_quelle_del_contratto() {
    for (testo, atteso) in [
        (SCHEMA_ERRORE, SHA256_ERRORE),
        (SCHEMA_DIAGNOSTICA, SHA256_DIAGNOSTICA),
    ] {
        let impronta = esadecimale(&Sha256::digest(testo.as_bytes()));
        assert_eq!(impronta, atteso);
    }
}

// ---------------------------------------------------------------------------
// Il validatore rifiuta: senza questo, un validatore che accetta tutto
// farebbe passare ogni test sotto.
// ---------------------------------------------------------------------------

#[test]
fn il_validatore_rifiuta_i_documenti_fuori_schema() {
    // Il payload del vettore `runtime-v1/data-run-timeout-error.json`.
    let vettore = json!({
        "category": "timeout",
        "phase": "finalize",
        "remote_effect": "none",
        "retry": {"kind": "safe"},
        "code": "EXECUTION_DEADLINE_EXCEEDED",
        "execution_id": "vector-execution-2",
        "message": "The execution deadline elapsed before a result was produced."
    });
    assert_eq!(valida_errore(&vettore), Ok(()));
    let modifiche: [(&str, Value); 9] = [
        ("/category", json!("timeout_locale")),
        ("/phase", json!("execute")),
        ("/retry", json!({"kind": "after"})),
        ("/retry", json!({"kind": "safe", "delay_ms": 5})),
        ("/code", json!("execution_deadline")),
        ("/message", json!("")),
        ("/message", json!("x".repeat(2_049))),
        ("/remote_effect", json!("unknown")),
        ("/extra", json!(1)),
    ];
    for (puntatore, valore) in modifiche {
        let mut documento = vettore.clone();
        if puntatore == "/extra" {
            documento["extra"] = valore;
        } else {
            *documento.pointer_mut(puntatore).expect("campo") = valore;
        }
        assert!(
            valida_errore(&documento).is_err(),
            "{puntatore} accettato: {documento}"
        );
    }
    let senza_messaggio = {
        let mut documento = vettore;
        documento
            .as_object_mut()
            .expect("oggetto")
            .remove("message");
        documento
    };
    assert!(valida_errore(&senza_messaggio).is_err());

    let diagnostica = json!({
        "contract": "plenora-row-diagnostics-v1",
        "scope": "read",
        "index_basis": "source_row_zero_based",
        "completeness": "complete",
        "observed_total": 1,
        "total": 1,
        "counts": {"conversion.invalid_date": 1},
        "examples_limit": 10,
        "examples_truncated": false,
        "examples": [{"source_index": 7, "cause": "conversion.invalid_date", "column": "d"}]
    });
    assert_eq!(valida_diagnostica(&diagnostica), Ok(()));
    for (puntatore, valore) in [
        ("/index_basis", json!("step_input_row_zero_based")),
        ("/counts", json!({"conversion": 1})),
        ("/examples/0/source_index", json!(-1)),
        ("/examples/0/cause", json!("Conversion.x")),
        ("/scope", json!("execute")),
    ] {
        let mut documento = diagnostica.clone();
        *documento.pointer_mut(puntatore).expect("campo") = valore;
        assert!(
            valida_diagnostica(&documento).is_err(),
            "{puntatore} accettato"
        );
    }
    // Chiave `redacted` con un valore: vietato dall'if/then dello schema.
    let mut chiave = diagnostica;
    chiave["examples"][0]["key"] = json!({"field": "id", "state": "redacted", "value": 3});
    assert!(valida_diagnostica(&chiave).is_err());
    chiave["examples"][0]["key"] = json!({"field": "id", "state": "redacted"});
    assert_eq!(valida_diagnostica(&chiave), Ok(()));
}

#[test]
fn una_parola_chiave_sconosciuta_fa_fallire_il_validatore() {
    // Anche dentro i combinatori: sotto `not` un errore dello schema non
    // diventa un documento accettato.
    for schema in [
        json!({"type": "object", "minProperties": 1}),
        json!({"not": {"minProperties": 1}}),
        json!({"oneOf": [{"type": "object"}, {"minProperties": 1}]}),
        json!({"if": {"minProperties": 1}, "then": {}}),
    ] {
        let esito = valida(&schema, &schema, &json!({"a": 1}), "");
        assert!(
            esito
                .as_ref()
                .is_err_and(|motivo| motivo.starts_with(STRUTTURALE)),
            "{schema}: {esito:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Le proiezioni del workspace.
// ---------------------------------------------------------------------------

/// Diagnostica valida di lettura, con `esempi` esempi su `osservate` righe.
fn diagnostica(osservate: u64, esempi: u64) -> RowDiagnostics {
    serde_json::from_value(json!({
        "contract": "plenora-row-diagnostics-v1",
        "scope": "read",
        "index_basis": "source_row_zero_based",
        "completeness": "complete",
        "observed_total": osservate,
        "total": osservate,
        "counts": {"evaluation.division_by_zero": osservate},
        "examples_limit": esempi.max(1),
        "examples_truncated": osservate > esempi,
        "examples": (0..esempi).map(|indice| json!({
            "source_index": indice * 3,
            "cause": "evaluation.division_by_zero",
            "column": "d"
        })).collect::<Vec<_>>()
    }))
    .expect("diagnostica valida")
}

/// Un errore per variante, piu' i wrapper.
fn campioni() -> Vec<PlenoraError> {
    vec![
        PlenoraError::InvalidPlan("passo `x`: operazione sconosciuta".into()),
        PlenoraError::Unsupported("geo.buffer: parametro non trattato".into()),
        PlenoraError::Schema("colonna `id` assente".into()),
        PlenoraError::DataMapping("arrow error: cast".into()),
        PlenoraError::Execution {
            node: "x".into(),
            operation: "table.filter".into(),
            execution_id: "e-1".into(),
            reason: "fallito".into(),
        },
        PlenoraError::Crs("geo.buffer: crs obbligatorio".into()),
        PlenoraError::from(plenora_core::crs::CrsError::NotBuiltin).con_contesto("geo.buffer"),
        PlenoraError::Io(std::io::Error::other("disco"))
            .with_remote_effect(plenora_core::RemoteEffect::Partial),
        PlenoraError::Cancelled("esecuzione annullata prima del passo `x` (table.sort)".into()),
        PlenoraError::ResourceLimit("budget superato".into()),
        PlenoraError::Io(std::io::Error::other("disco")),
        PlenoraError::Protocol("cornice troncata".into()),
        PlenoraError::Timeout("scadenza dell'esecuzione superata prima del passo `x`".into()),
        PlenoraError::Conflict("destinazione esistente".into()),
        PlenoraError::InvalidConfiguration("griglia non valida".into()),
        PlenoraError::Internal("stato impossibile".into()),
        PlenoraError::DataMapping("rifiutate".into()).with_row_diagnostics(diagnostica(25, 10)),
        PlenoraError::DataMapping("rifiutate".into())
            .with_row_diagnostics(
                diagnostica(4, 4)
                    .senza_attribuzione()
                    .expect("senza attribuzione"),
            )
            .with_phase(ErrorPhase::Read),
        PlenoraError::DataMapping(String::new()),
        PlenoraError::Internal("é".repeat(3_000)),
    ]
}

#[test]
fn ogni_proiezione_e_valida_contro_lo_schema() {
    for errore in campioni() {
        let (pubblico, documento) = proiezione(&errore);
        assert_eq!(valida_errore(&documento), Ok(()), "{documento}");
        // La proiezione non cambia gli assi (nessun errore qui ha effetto
        // ignoto) e non aggiunge testo: il messaggio e' il `Display`,
        // eventualmente troncato.
        assert_eq!(pubblico.category(), errore.category());
        assert_eq!(pubblico.phase(), errore.phase());
        assert_eq!(pubblico.retry(), errore.retry_disposition());
        let testo = errore.to_string();
        assert!(
            testo.starts_with(pubblico.message()) || testo.is_empty(),
            "{documento}"
        );
        assert_eq!(
            documento.pointer("/details/row_diagnostics").is_some(),
            errore.row_diagnostics().is_some()
        );
    }
}

#[test]
fn la_diagnostica_senza_attribuzione_e_valida_contro_lo_schema() {
    let ridotta = diagnostica(12, 10)
        .senza_attribuzione()
        .expect("senza attribuzione");
    let documento = serde_json::to_value(&ridotta).expect("serializzabile");
    assert_eq!(valida_diagnostica(&documento), Ok(()));
    assert_eq!(documento["completeness"], "partial");
    assert_eq!(
        documento["knowledge_limits"],
        json!(["read.row_attribution_unavailable"])
    );
    assert_eq!(documento["examples"], json!([]));
}

#[test]
fn la_diagnostica_di_scrittura_e_valida_contro_lo_schema() {
    // Lo scope `write` non lo produce nessun codice del workspace, ma il
    // tipo lo rappresenta: la sua forma serializzata e' quella del contratto.
    let documento = json!({
        "contract": "plenora-row-diagnostics-v1",
        "scope": "write",
        "index_basis": "source_row_zero_based",
        "completeness": "complete",
        "observed_total": 1,
        "total": 1,
        "input_total": 5200,
        "counts": {"database.constraint_violation": 1},
        "examples_limit": 10,
        "examples_truncated": false,
        "examples": [{
            "source_index": 4999,
            "cause": "database.constraint_violation",
            "column": "area_m2",
            "key": {"field": "parcel_id", "state": "redacted"},
            "write_state": "certainly_rejected"
        }],
        "diagnostic_state_counts": {
            "certainly_rejected": 1,
            "certainly_not_attempted": 0,
            "certainly_rolled_back": 0,
            "effect_unknown": 0
        },
        "write_outcome": {
            "certainly_rejected": {"state": "known", "value": 1},
            "certainly_not_attempted": {"state": "known", "value": 200},
            "certainly_rolled_back": {"state": "known", "value": 4999},
            "effect_unknown": {"state": "unknown"}
        }
    });
    let letta: RowDiagnostics = serde_json::from_value(documento.clone()).expect("valida");
    let riscritta = serde_json::to_value(&letta).expect("serializzabile");
    assert_eq!(riscritta, documento);
    assert_eq!(valida_diagnostica(&riscritta), Ok(()));
}

#[test]
fn la_proiezione_sostitutiva_e_valida_contro_lo_schema() {
    // 129 esempi: l'array supera i 128 elementi di ERR-012.
    let errore =
        PlenoraError::DataMapping("rifiutate".into()).with_row_diagnostics(diagnostica(200, 129));
    assert!(errore.row_diagnostics().is_some());
    let (pubblico, documento) = proiezione(&errore);
    assert_eq!(valida_errore(&documento), Ok(()), "{documento}");
    assert_eq!(pubblico.category(), ErrorCategory::Internal);
    assert_eq!(pubblico.code(), Some(CODE_DETAILS_NOT_PUBLISHABLE));
    assert!(documento.get("details").is_none());
}

#[test]
fn ritardo_e_quarantena_sono_validi_contro_lo_schema() {
    use plenora_core::RetryDisposition;
    let base = proiezione(&PlenoraError::Io(std::io::Error::other("disco"))).1;
    for retry in [
        RetryDisposition::After(Duration::from_millis(250)),
        RetryDisposition::Quarantine,
        RetryDisposition::RequiresRecovery,
        RetryDisposition::RequiresIdempotencyKey,
    ] {
        let mut documento = base.clone();
        documento["retry"] = serde_json::to_value(retry).expect("serializzabile");
        assert_eq!(valida_errore(&documento), Ok(()), "{documento}");
    }
}
