//! Confine di lettura: il minimo perché un file malformato diventi un
//! errore esplicito invece di un panico o di un risultato sbagliato
//! (README, «Confine di lettura»):
//!
//! - **barriera anti-panico** ([`barriera`]) attorno a ogni chiamata ad
//!   `arrow-ipc`, `parquet` e `concat_batches` sui byte del file: le
//!   dipendenze vanno in panico su input malformati (`fb_to_schema`,
//!   `unwrap` sui campi opzionali del footer);
//! - **limiti economici** ([`LimitiLettura`]): metadati IPC e footer
//!   Parquet entro [`LimitiLettura::max_byte_metadati`], metadati di schema e
//!   di campo entro [`LimitiLettura::max_byte_metadati_custom`], blocchi IPC
//!   e row group entro [`LimitiLettura::max_blocchi`].
//!
//! Non è una difesa da file costruiti apposta: un'allocazione impossibile
//! dentro `parquet` o `arrow-ipc` è un aborto del processo, che la barriera
//! non ferma (limite dichiarato nel README).
//!
//! Gli errori non riportano byte del file né valori: solo che cosa non va e
//! quale limite.

use std::panic::AssertUnwindSafe;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::panic_policy::{barriera_di_dipendenza, forma_payload};
use plenora_core::{PlenoraError, Result};

/// Limiti del confine di lettura.
///
/// I valori predefiniti stanno larghi sui file reali (README, «Confine di
/// lettura»); chi ne ha di più grandi li alza con
/// [`crate::leggi_tabella_con_limiti`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitiLettura {
    /// Byte massimi di un messaggio di metadati: ogni messaggio IPC
    /// (schema, dizionario, blocco), il footer IPC e il footer Parquet.
    /// Vale anche il budget residuo, se è più stretto.
    pub max_byte_metadati: u64,
    /// Byte massimi dei metadati chiave-valore dello schema letto (di
    /// schema e di tutti i campi, a ogni profondità: chiavi più valori).
    pub max_byte_metadati_custom: u64,
    /// Blocchi IPC (record batch) o row group Parquet massimi per file.
    pub max_blocchi: u64,
}

/// 16 MiB: il footer di un Parquet con migliaia di colonne e decine di row
/// group resta sotto qualche MiB.
pub const MAX_BYTE_METADATI: u64 = 16 * 1024 * 1024;

/// 4 MiB: metadati `pandas`, `geo` con PROJJSON completi, `ARROW:schema`
/// di tabelle larghe stanno sotto qualche centinaio di KiB.
pub const MAX_BYTE_METADATI_CUSTOM: u64 = 4 * 1024 * 1024;

/// 100 000: una tabella di 10 milioni di righe scritta a blocchi di 64 Ki
/// righe ne ha circa 150.
pub const MAX_BLOCCHI: u64 = 100_000;

impl Default for LimitiLettura {
    fn default() -> Self {
        Self {
            max_byte_metadati: MAX_BYTE_METADATI,
            max_byte_metadati_custom: MAX_BYTE_METADATI_CUSTOM,
            max_blocchi: MAX_BLOCCHI,
        }
    }
}

impl LimitiLettura {
    /// Il tetto dei metadati con il budget residuo: anche i metadati sono
    /// un'allocazione, precedente a qualunque dato.
    #[must_use]
    pub fn metadati_entro(&self, residuo: u64) -> u64 {
        self.max_byte_metadati.min(residuo)
    }
}

/// Un file malformato: `DataMapping` con il solo motivo.
#[must_use]
pub fn malformato(formato: &str, motivo: &str) -> PlenoraError {
    PlenoraError::DataMapping(format!("{formato} malformato: {motivo}"))
}

/// Un limite del confine superato: `ResourceLimit`.
#[must_use]
pub fn oltre_il_limite(cosa: &str, dichiarati: u64, limite: u64) -> PlenoraError {
    PlenoraError::ResourceLimit(format!(
        "{cosa}: {dichiarati} oltre il limite di lettura di {limite}"
    ))
}

/// Esegue una chiamata a una dipendenza (`arrow-ipc`, `parquet`) sui byte di
/// un file dentro la barriera di dipendenza: un panico diventa
/// `DataMapping` con la sola forma del payload.
///
/// Il lavoro contiene solo la chiamata alla dipendenza
/// ([`barriera_di_dipendenza`]).
///
/// # Errors
///
/// Quelli del lavoro; `DataMapping` se la dipendenza va in panico.
pub fn barriera<T>(dipendenza: &str, lavoro: impl FnOnce() -> Result<T>) -> Result<T> {
    barriera_di_dipendenza(AssertUnwindSafe(lavoro)).unwrap_or_else(|payload| {
        Err(PlenoraError::DataMapping(format!(
            "{dipendenza} in panico su un file malformato: {}",
            forma_payload(payload.as_ref())
        )))
    })
}

/// I metadati chiave-valore dello schema e dei campi, più `altri`, stanno
/// in `limite` byte.
///
/// Si contano a ogni profondità; `altri` sono byte già contati (per Parquet
/// i metadati chiave-valore del file, che la conversione dello schema in
/// parte toglie).
///
/// # Errors
///
/// `ResourceLimit` oltre il limite.
pub fn verifica_metadati_custom(schema: &Schema, altri: u64, limite: u64) -> Result<()> {
    let mut totale = byte_mappa(schema.metadata()).saturating_add(altri);
    let mut da_visitare: Vec<&Field> = schema.fields().iter().map(AsRef::as_ref).collect();
    while let Some(campo) = da_visitare.pop() {
        totale = totale.saturating_add(byte_mappa(campo.metadata()));
        if totale > limite {
            break;
        }
        figli(campo.data_type(), &mut da_visitare);
    }
    if totale > limite {
        return Err(oltre_il_limite(
            "metadati di schema e di campo (byte)",
            totale,
            limite,
        ));
    }
    Ok(())
}

fn byte_mappa(mappa: &std::collections::HashMap<String, String>) -> u64 {
    mappa.iter().fold(0_u64, |totale, (chiave, valore)| {
        totale
            .saturating_add(u64::try_from(chiave.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(valore.len()).unwrap_or(u64::MAX))
    })
}

/// I campi figli di un tipo annidato.
fn figli<'a>(tipo: &'a DataType, uscita: &mut Vec<&'a Field>) {
    match tipo {
        DataType::List(figlio)
        | DataType::LargeList(figlio)
        | DataType::ListView(figlio)
        | DataType::LargeListView(figlio)
        | DataType::FixedSizeList(figlio, _)
        | DataType::Map(figlio, _) => uscita.push(figlio),
        DataType::Struct(campi) => uscita.extend(campi.iter().map(AsRef::as_ref)),
        DataType::Union(campi, _) => uscita.extend(campi.iter().map(|(_, campo)| campo.as_ref())),
        DataType::RunEndEncoded(estremi, valori) => {
            uscita.push(estremi);
            uscita.push(valori);
        }
        DataType::Dictionary(_, valori) => figli(valori, uscita),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use plenora_core::arrow::schema::{DataType, Field, Schema};
    use plenora_core::ErrorCategory;

    use super::{barriera, verifica_metadati_custom};

    #[test]
    fn la_barriera_rende_un_errore_senza_payload() {
        let esito: plenora_core::Result<()> =
            barriera("dipendenza", || std::panic::panic_any("segreto".to_owned()));
        let errore = esito.expect_err("panico");
        assert_eq!(errore.category(), ErrorCategory::DataMapping);
        assert!(!errore.to_string().contains("segreto"), "{errore}");
    }

    #[test]
    fn i_metadati_annidati_contano() {
        let metadati = HashMap::from([("k".to_owned(), "v".repeat(100))]);
        let foglia = Field::new("x", DataType::Int32, true).with_metadata(metadati);
        let lista = Field::new_list("l", foglia, true);
        let schema = Schema::new(vec![lista]);
        assert!(verifica_metadati_custom(&schema, 0, 101).is_ok());
        assert!(verifica_metadati_custom(&schema, 1, 101).is_err());
        let errore = verifica_metadati_custom(&schema, 0, 100).expect_err("oltre");
        assert_eq!(errore.category(), ErrorCategory::ResourceLimit);
    }
}
