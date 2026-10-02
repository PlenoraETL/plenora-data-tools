//! L'inviluppo CLI 2.0 (`schemas/cli-envelope-v2.schema.json`) e il codice
//! d'uscita.
//!
//! Un'invocazione produce **un** documento JSON seguito da un a capo su
//! stdout, e niente su stderr (CLI 2.0, sezione 4). Le chiavi escono in
//! ordine (la `Map` di `serde_json` senza `preserve_order` è una
//! `BTreeMap`): stesso esito, stessi byte.
//!
//! L'errore è sempre la proiezione pubblica di `PlenoraError`
//! (`plenora-error-v1`, `PlenoraError::public_projection`): questo modulo
//! non costruisce errori propri.

use plenora_core::{ErrorCategory, PlenoraError, RemoteEffect};
use serde_json::{json, Value};

use crate::{COMPONENTE, PROTOCOLLO_CLI, VERSIONE_COMPONENTE};

/// Contratto e comando dell'inviluppo quando il comando non si conosce
/// (argomenti che non nominano un comando).
pub const CONTRATTO_ERRORE: &str = "plenora-error-v1";
/// Comando dell'inviluppo quando il comando non si conosce.
pub const COMANDO_SCONOSCIUTO: &str = "unknown";

/// Identità dell'inviluppo: comando canonico e contratto del risultato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identita {
    pub comando: &'static str,
    pub contratto: &'static str,
}

impl Identita {
    /// L'identità di un'invocazione il cui comando non si è potuto leggere.
    pub const SCONOSCIUTA: Self = Self {
        comando: COMANDO_SCONOSCIUTO,
        contratto: CONTRATTO_ERRORE,
    };
}

/// Il codice d'uscita di una categoria (CLI 2.0, sezione 8). Il `match` è
/// esaustivo: una categoria nuova non compila finché non ha la sua riga.
#[must_use]
pub const fn codice_uscita(categoria: ErrorCategory) -> u8 {
    match categoria {
        ErrorCategory::InvalidPlan | ErrorCategory::InvalidConfiguration => 2,
        ErrorCategory::Schema
        | ErrorCategory::DataMapping
        | ErrorCategory::Crs
        | ErrorCategory::Unsupported => 3,
        ErrorCategory::ResourceLimit => 4,
        ErrorCategory::Io
        | ErrorCategory::NotFound
        | ErrorCategory::Conflict
        | ErrorCategory::ConcurrentModification
        | ErrorCategory::Protocol
        | ErrorCategory::Authentication
        | ErrorCategory::Authorization
        | ErrorCategory::Timeout
        | ErrorCategory::Transient => 5,
        ErrorCategory::Execution => 6,
        ErrorCategory::Internal => 70,
        ErrorCategory::Cancelled => 130,
    }
}

fn identita(stato: &str, identita: Identita) -> serde_json::Map<String, Value> {
    let mut campi = serde_json::Map::new();
    campi.insert("status".to_owned(), json!(stato));
    campi.insert("protocol_version".to_owned(), json!(PROTOCOLLO_CLI));
    campi.insert("component".to_owned(), json!(COMPONENTE));
    campi.insert("component_version".to_owned(), json!(VERSIONE_COMPONENTE));
    campi.insert("contract".to_owned(), json!(identita.contratto));
    campi.insert("command".to_owned(), json!(identita.comando));
    campi
}

/// Un inviluppo pronto: il testo per stdout (con l'a capo finale) e il
/// codice d'uscita.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uscita {
    pub stdout: String,
    pub codice: u8,
}

/// Inviluppo di successo, exit 0.
#[must_use]
pub fn successo(identita_comando: Identita, risultato: Value) -> Uscita {
    let mut campi = identita("ok", identita_comando);
    campi.insert("result".to_owned(), risultato);
    Uscita {
        stdout: testo(&Value::Object(campi)),
        codice: 0,
    }
}

/// Inviluppo d'errore, con il codice d'uscita della categoria.
#[must_use]
pub fn errore(identita_comando: Identita, errore: &PlenoraError) -> Uscita {
    let pubblico = errore.public_projection();
    let codice = codice_uscita(pubblico.category());
    let mut campi = identita("error", identita_comando);
    // La proiezione è serializzabile per costruzione (campi chiusi, details
    // già verificato contro ERR-011/ERR-012): un fallimento qui è
    // un'invariante violata, e diventa l'errore interno di riserva.
    let Ok(documento) = serde_json::to_value(&pubblico) else {
        return riserva(identita_comando);
    };
    campi.insert("error".to_owned(), documento);
    Uscita {
        stdout: testo(&Value::Object(campi)),
        codice,
    }
}

/// L'inviluppo di un panico: errore `internal` senza il testo del payload
/// (CLI 2.0, sezione 6), exit 70.
///
/// `con_effetti`: il comando scrive file (`run`). Un panico può arrivare
/// dopo che un output è stato scritto, e nessuno sa quanti: l'effetto è
/// `unknown` (ERR-004), mai `none`.
#[must_use]
pub fn panico(identita_comando: Identita, con_effetti: bool) -> Uscita {
    let interno = PlenoraError::Internal(
        "panico interno intercettato; il contenuto non viene pubblicato".to_owned(),
    );
    let interno = if con_effetti {
        interno.override_remote_effect(RemoteEffect::Unknown)
    } else {
        interno
    };
    errore(identita_comando, &interno)
}

/// Ultima riserva, se nemmeno un errore si serializza: testo costante, con
/// l'effetto prudente (`unknown`: non si sa a che punto si era).
fn riserva(identita_comando: Identita) -> Uscita {
    let mut campi = identita("error", identita_comando);
    campi.insert(
        "error".to_owned(),
        json!({
            "category": "internal",
            "phase": "finalize",
            "remote_effect": "unknown",
            "retry": {"kind": "never"},
            "message": "errore non serializzabile",
        }),
    );
    Uscita {
        stdout: testo(&Value::Object(campi)),
        codice: codice_uscita(ErrorCategory::Internal),
    }
}

/// JSON compatto con l'a capo finale. La serializzazione di un `Value` non
/// fallisce (chiavi stringa, numeri finiti per costruzione).
fn testo(documento: &Value) -> String {
    let mut testo = documento.to_string();
    testo.push('\n');
    testo
}

#[cfg(test)]
mod tests {
    use plenora_core::ErrorCategory;

    use super::codice_uscita;

    /// La tabella della sezione 8 di CLI 2.0, riga per riga, per nome
    /// stabile: ogni categoria dello schema ha la sua riga, e nessuna
    /// proietta su 0.
    #[test]
    fn codici_d_uscita_della_sezione_8() {
        let tabella: &[(u8, &[&str])] = &[
            (2, &["invalid_plan", "invalid_configuration"]),
            (3, &["schema", "data_mapping", "crs", "unsupported"]),
            (4, &["resource_limit"]),
            (
                5,
                &[
                    "io",
                    "not_found",
                    "conflict",
                    "concurrent_modification",
                    "protocol",
                    "authentication",
                    "authorization",
                    "timeout",
                    "transient",
                ],
            ),
            (6, &["execution"]),
            (70, &["internal"]),
            (130, &["cancelled"]),
        ];
        let mut viste = 0;
        for (codice, categorie) in tabella {
            for nome in *categorie {
                let categoria = ErrorCategory::from_stable_name(nome).expect("categoria");
                assert_eq!(codice_uscita(categoria), *codice, "{nome}");
                viste += 1;
            }
        }
        assert_eq!(viste, ErrorCategory::ALL.len());
        assert!(ErrorCategory::ALL.iter().all(|c| codice_uscita(*c) != 0));
    }
}
