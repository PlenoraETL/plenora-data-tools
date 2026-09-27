//! Il `commit_token` nel footer di un artefatto Arrow IPC.
//!
//! Il token e' l'**identita' del tentativo**: correla artefatto, `Saluto` e
//! verifica (`isolamento.md`, passo 8-bis). **Non e' una credenziale**: niente
//! firma, MAC o chiave, chiunque scriva un file Arrow IPC puo' metterci il
//! token che vuole. Dice quale tentativo *dichiara* di aver prodotto il file.
//!
//! Da non confondere con il marcatore durevole del footer (il file e' finito)
//! e con il digest dell'artefatto, trasmesso nell'`Esito`, l'unico dei tre che
//! dica qualcosa sul contenuto.
//!
//! Senza token non si scrive nulla, nemmeno una chiave vuota. La lettura passa
//! dalla traversata rinforzata
//! ([`valida_file_ed_estrai`](crate::geo_transport::ipc::valida_file_ed_estrai)),
//! mai da `FileReader::custom_metadata`, che ne salterebbe i controlli. Un
//! token assente e' legittimo; presente ma non canonico si rifiuta sempre.
//! Che sia obbligatorio e' una proprieta' del percorso isolato, non di questo
//! modulo.

use std::io::Write;

use plenora_core::arrow::ipc::writer::FileWriter;

use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
// Ogni import porta il `cfg` di chi lo usa, o la build ordinaria segnala un
// import inutilizzato — e un warning tollerato e' un warning che smette di
// essere letto.
//
// `ArrowTransportError` serve a `interpreta_commit_token`, che e' di
// produzione da quando `pubblicazione::risolvi_commit` la chiama: l'import non
// porta `cfg` perche' la sua condizione non vale piu'.
use crate::geo_transport::error::ArrowTransportError;
// Questi soltanto a `leggi_commit_token`.
#[cfg(test)]
use crate::geo_transport::ipc::{valida_file_ed_estrai, IpcLimits, IpcSource};

/// Interpreta il testo trovato sotto la chiave del token.
///
/// Un punto solo per la regola (presente ma non canonico si rifiuta) condivisa
/// dai due chiamanti: la traversata del solo token e il verificatore.
///
/// # Errors
///
/// [`ArrowTransportError::IpcMetadataInvalid`] se il testo non e' canonico. Il
/// messaggio e' un `&'static str`, quindi il tipo non consente di portare il
/// valore.
// Senza `cfg`: `pubblicazione::risolvi_commit` la chiama da codice di
// produzione. Registro:
// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
pub fn interpreta_commit_token(testo: &str) -> Result<CommitToken, ArrowTransportError> {
    CommitToken::da_esadecimale(testo).map_err(|_| {
        ArrowTransportError::IpcMetadataInvalid(
            "commit token del footer non canonico: atteso esadecimale minuscolo di 64 caratteri",
        )
    })
}

/// Scrive il `commit_token` nel footer, se c'e'.
///
/// Va chiamata **prima** di `FileWriter::finish`: dopo, la chiamata non avrebbe
/// effetto, in silenzio. Non rende un `Result` perche' `write_metadata`
/// accumula in una mappa e non puo' fallire.
pub fn scrivi_commit_token<W: Write>(scrittore: &mut FileWriter<W>, token: Option<&CommitToken>) {
    let Some(token) = token else {
        // Nessuna chiave, nessun valore, nessun byte: e' cio' che rende gli
        // artefatti senza token identici a quelli di prima.
        return;
    };
    scrittore.write_metadata(CHIAVE_FOOTER_COMMIT_TOKEN, token.in_esadecimale());
}

/// Legge il `commit_token` dal footer di un artefatto.
///
/// Rende `Ok(None)` se il token non c'e': e' il caso ordinario, non un
/// difetto.
///
/// # Errors
///
/// - gli errori del framing, perche' la lettura passa dalla convalida;
/// - [`ArrowTransportError::IpcMetadataInvalid`] se il token c'e' ma non e'
///   canonico (il messaggio e' un `&'static str` e non porta il valore).
///
/// Dietro `cfg(test)`: il verificatore non la usa perche' deve riferire
/// framing, token e digest a un solo handle, senza una seconda traversata.
/// Cio' che condividono sta in [`interpreta_commit_token`]. Registro:
/// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
#[cfg(test)]
pub fn leggi_commit_token<S: IpcSource + ?Sized>(
    sorgente: &mut S,
    limiti: &IpcLimits,
) -> Result<Option<CommitToken>, ArrowTransportError> {
    let trovato = valida_file_ed_estrai(sorgente, limiti, Some(CHIAVE_FOOTER_COMMIT_TOKEN))?;
    let Some(testo) = trovato else {
        return Ok(None);
    };
    interpreta_commit_token(&testo).map(Some)
}

#[cfg(test)]
mod tests;
