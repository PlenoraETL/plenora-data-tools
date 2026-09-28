//! Il formato di uscita, scelto dal flag globale `--format`.

use plenora_core::PlenoraError;

use crate::contract;

/// Formato dell'output dei comandi, scelto dal flag globale `--format`.
///
/// Stessa convenzione di `plenora-database-tools`: il flag e' globale, viene
/// tolto dagli argomenti PRIMA del dispatch e vale per il comando che segue.
/// `junit` non c'e': un formato senza un consumatore e' codice non provato, e
/// qui nessun gate lo legge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// JSON: il default, ed e' cio' che uno script deve poter assumere.
    Json,
    /// Markdown: leggibile da una persona, per i comandi che descrivono.
    Markdown,
}

static ACTIVE_FORMAT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

impl OutputFormat {
    fn set_active(self) {
        let value = match self {
            Self::Json => 0,
            Self::Markdown => 1,
        };
        ACTIVE_FORMAT.store(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn active() -> Self {
        if ACTIVE_FORMAT.load(std::sync::atomic::Ordering::Relaxed) == 1 {
            Self::Markdown
        } else {
            Self::Json
        }
    }

    /// Esige il formato JSON: i comandi che non hanno una resa leggibile
    /// rifiutano `--format markdown` invece di ignorarlo. Un flag accettato e
    /// disatteso e' peggio di un flag rifiutato.
    pub fn require_json(comando: &str) -> Result<(), PlenoraError> {
        if Self::active() == Self::Markdown {
            return Err(contract(format!(
                "`--format markdown` non e' disponibile per `{comando}`: \
                 formati supportati `json`"
            )));
        }
        Ok(())
    }
}

/// Toglie `--format VALORE` dagli argomenti e lo registra come formato
/// attivo. Il flag e' globale: puo' precedere o seguire il sottocomando.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il valore manca o non e' riconosciuto.
pub fn strip_output_format(args: Vec<String>) -> Result<Vec<String>, PlenoraError> {
    let mut rimanenti = Vec::with_capacity(args.len());
    let mut visto = false;
    let mut iteratore = args.into_iter();
    while let Some(argument) = iteratore.next() {
        if argument != "--format" {
            rimanenti.push(argument);
            continue;
        }
        let valore = iteratore
            .next()
            .ok_or_else(|| contract("valore mancante per --format (json|markdown)"))?;
        if visto {
            // Due `--format` con valori diversi sono due richieste diverse:
            // farne vincere una in silenzio significa eseguire quella che
            // l'utente non ha scritto.
            return Err(contract(
                "flag `--format` ripetuto: se ne accetta una sola occorrenza",
            ));
        }
        visto = true;
        match valore.as_str() {
            "json" => OutputFormat::Json.set_active(),
            "markdown" => OutputFormat::Markdown.set_active(),
            altro => {
                return Err(contract(format!(
                    "formato `{altro}` non riconosciuto: attesi `json` o `markdown`"
                )));
            }
        }
    }
    Ok(rimanenti)
}
