//! Come questa build **descrive se stessa** all'altro lato del filo.
//!
//! Ciascun lato si misura da solo (artefatto, resolver, ambiente): un worker
//! che rispecchiasse la descrizione ricevuta renderebbe il confronto
//! dell'handshake verde per costruzione.
//!
//! Con `proj-backend` la descrizione **rifiuta**: l'ambiente PROJ non ha una
//! radice esclusiva, immutabile e inventariabile da cui ricavare l'insieme
//! delle risorse. Perimetro e rientro in `errori-e-limiti.md`.

#[cfg(unix)]
use std::io::Read as _;

use plenora_core::{PlenoraError, Result};
use sha2::{Digest as _, Sha256};

use crate::esadecimale32::Esadecimale32;
use crate::risolutore::{Risolutore, VERSIONE};

use super::digest::DigestSha256;
#[cfg(unix)]
use super::handshake::{Descrizione, DescrizioneLocale};
#[cfg(unix)]
use super::messaggi::IdentitaArtefatto;
use super::messaggi::{Ambiente, IdentitaResolver};

/// Il percorso dell'immagine in esecuzione.
///
/// Non il nome di invocazione, che una `rename` puo' sostituire fra il
/// controllo e la lettura: questo collegamento il kernel lo tiene legato
/// all'immagine **di questo processo**.
#[cfg(target_os = "linux")]
const IMMAGINE: &str = "/proc/self/exe";

/// Quanto si legge per volta, digerendo l'immagine.
///
/// Il buffer **e'** il limite di memoria: si legge a blocchi, quindi non serve
/// un tetto totale sulla dimensione dell'immagine. Il tempo lo governa la
/// scadenza dell'handshake.
const BLOCCO: usize = 64 * 1024;

/// Il dominio del digest dell'insieme delle risorse.
///
/// Il prefisso separa **questo** digest da ogni altro digest del programma:
/// l'insieme vuoto e' l'hash di questa stringa, non di niente. La versione sta
/// nel dominio perche' la regola di calcolo puo' cambiare, e due regole
/// diverse non devono poter coincidere.
const DOMINIO_INSIEME: &str = "plenora:insieme-risorse:v1";

/// La descrizione di questa build, misurata adesso.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se l'immagine non si legge o non e'
/// quella che dichiara di essere; [`PlenoraError::InvalidConfiguration`] se
/// questa build non sa descrivere il proprio ambiente.
#[cfg(target_os = "linux")]
pub fn di_questa_build(capability: Vec<String>) -> Result<DescrizioneLocale> {
    let scelto = Risolutore::di_questa_build();
    Ok(DescrizioneLocale {
        comune: Descrizione {
            artefatto: artefatto()?,
            resolver: resolver(scelto),
            ambiente: ambiente(scelto)?,
        },
        capability,
    })
}

/// L'identita' dell'immagine in esecuzione.
///
/// Due controlli: **file regolare**, perche' cio' che non e' un'immagine non
/// descrive un programma; e **dimensione stabile**, confrontando la taglia del
/// descrittore aperto con i byte digeriti. Se divergono l'immagine e' stata
/// riscritta durante la lettura, e il digest descriverebbe una cucitura.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] con la condizione che manca.
#[cfg(target_os = "linux")]
fn artefatto() -> Result<IdentitaArtefatto> {
    let mut immagine = std::fs::File::open(IMMAGINE)
        .map_err(|causa| non_leggibile(&format!("non si apre: {causa}")))?;
    let dati = immagine
        .metadata()
        .map_err(|causa| non_leggibile(&format!("non si interroga: {causa}")))?;
    if !dati.is_file() {
        return Err(non_leggibile(
            "non e' un file regolare: cio' che non e' un'immagine non si digerisce",
        ));
    }
    let dichiarata = dati.len();

    let mut digestore = Sha256::new();
    let mut blocco = vec![0_u8; BLOCCO];
    let mut letti: u64 = 0;
    loop {
        let quanti = immagine
            .read(&mut blocco)
            .map_err(|causa| non_leggibile(&format!("non si legge: {causa}")))?;
        if quanti == 0 {
            break;
        }
        digestore.update(&blocco[..quanti]);
        // Non trabocca su un file reale, ma `checked_add` lo controlla invece
        // di presumerlo.
        letti = letti
            .checked_add(quanti as u64)
            .ok_or_else(|| non_leggibile("la somma dei byte letti trabocca"))?;
    }
    if letti != dichiarata {
        return Err(non_leggibile(&format!(
            "dichiarava {dichiarata} byte e ne ha resi {letti}: l'immagine e' cambiata durante \
             la lettura, e il digest non descriverebbe nessun programma"
        )));
    }

    let digest = DigestSha256::da_esadecimale(&in_esadecimale(digestore))
        .map_err(|forma| non_leggibile(&format!("il digest non e' in forma canonica: {forma}")))?;
    Ok(IdentitaArtefatto {
        digest,
        versione: VERSIONE.to_owned(),
    })
}

/// L'identita' del resolver, dal selettore che sceglie anche la funzione.
fn resolver(scelto: Risolutore) -> IdentitaResolver {
    IdentitaResolver {
        identita: scelto.identita().to_owned(),
        versione: VERSIONE.to_owned(),
    }
}

/// L'ambiente di questa build, oppure il rifiuto di descriverlo.
///
/// L'insieme vuoto e' una descrizione verificabile: nessun backend scarica
/// risorse a esecuzione in corso. Con PROJ i percorsi si **aggiungono** a
/// quelli esistenti e la cache delle griglie e' attiva per default: un digest
/// del solo searchpath direbbe «stesso ambiente» su macchine con contenuti
/// diversi.
///
/// # Errors
///
/// [`PlenoraError::InvalidConfiguration`] quando l'ambiente non e'
/// inventariabile: e' una condizione della **build**, non dell'esecuzione, e
/// chi la legge deve poter cambiare build.
fn ambiente(scelto: Risolutore) -> Result<Ambiente> {
    if !scelto.ambiente_inventariabile() {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "il profilo isolato non e' disponibile con il resolver «{}»: non esiste una radice \
             esclusiva, immutabile e inventariabile da cui ricavare l'insieme delle risorse, e un \
             digest inventato sarebbe una falsa garanzia. Le condizioni di rientro stanno in \
             errori-e-limiti.md",
            scelto.identita()
        )));
    }
    let mut digestore = Sha256::new();
    digestore.update(DOMINIO_INSIEME.as_bytes());
    // Nessuna risorsa: **niente** dopo il dominio; un segnaposto farebbe un
    // insieme di un elemento.
    let digest = DigestSha256::da_esadecimale(&in_esadecimale(digestore)).map_err(|forma| {
        PlenoraError::InvalidConfiguration(format!(
            "il digest dell'insieme vuoto non e' in forma canonica: {forma}"
        ))
    })?;
    Ok(Ambiente {
        digest_insieme: digest,
        acquisizione_dinamica: false,
        risorse: Vec::new(),
        backend_dinamici: Vec::new(),
    })
}

/// Lo SHA-256 concluso, nella forma che [`DigestSha256`] accetta.
///
/// Passa da [`Esadecimale32`], l'unica autorita' sulla grafia esadecimale.
fn in_esadecimale(digestore: Sha256) -> String {
    Esadecimale32::dai_byte(digestore.finalize().into()).in_esadecimale()
}

/// Il rifiuto sull'immagine, col percorso che lo riguarda.
#[cfg(target_os = "linux")]
fn non_leggibile(motivo: &str) -> PlenoraError {
    PlenoraError::IsolationUnavailable(format!("artefatto: {IMMAGINE}: {motivo}"))
}

#[cfg(test)]
mod tests;
