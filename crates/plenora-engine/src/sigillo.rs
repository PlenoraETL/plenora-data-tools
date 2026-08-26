//! Il `commit_token` nel footer di un artefatto Arrow IPC.
//!
//! # Due sigilli distinti, e non vanno confusi
//!
//! - il **marcatore durevole** del footer, scritto da `FileWriter::finish`, e'
//!   cio' che il framing rinforzato verifica: dice che il file e' finito;
//! - il `commit_token` dice **chi** ha autorizzato quel file, e viaggia qui.
//!
//! Il digest dell'artefatto e' una terza cosa ancora, calcolata sull'intero
//! file finalizzato e trasmessa nell'`Esito`: scriverla dentro il file che
//! copre sarebbe autoreferenziale.
//!
//! # Scrittura: prima di `finish`, e solo se c'e' un token
//!
//! Senza token **non si scrive nulla**, nemmeno una chiave vuota. E' la
//! ragione per cui gli artefatti prodotti in-process restano byte per byte
//! quelli di prima: la presenza del token e' l'unica differenza, e quando non
//! c'e' non c'e' proprio.
//!
//! # Lettura: dalla traversata rinforzata, mai da `FileReader`
//!
//! `FileReader::custom_metadata` sarebbe una terza strada nel footer, e di
//! tutti i controlli che `PR-0` ha messo in quella traversata non ne farebbe
//! nessuno. Qui si passa da
//! [`valida_file_ed_estrai`](crate::geo_transport::ipc::valida_file_ed_estrai),
//! che convalida **e** estrae nello stesso passaggio.
//!
//! # Assente, canonico, non canonico
//!
//! - **assente**: legittimo. Un artefatto ordinario non ha un token, e
//!   pretenderlo renderebbe illeggibile tutto cio' che esiste gia';
//! - **canonico**: accettato;
//! - **presente ma non canonico**: rifiutato sempre, in ogni percorso. Non si
//!   normalizza e non si ignora — un token che non e' quello che diciamo di
//!   scrivere e' un artefatto di cui non sappiamo dire chi l'ha autorizzato.
//!
//! Che il token sia **obbligatorio** e' una proprieta' del percorso isolato,
//! non di questa funzione: qui si dice cosa c'e', non se doveva esserci.

use std::io::Write;

use plenora_core::arrow::ipc::writer::FileWriter;

use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
use crate::geo_transport::error::ArrowTransportError;
use crate::geo_transport::ipc::{valida_file_ed_estrai, IpcLimits, IpcSource};

/// Scrive il `commit_token` nel footer, se c'e'.
///
/// Va chiamata **prima** di `FileWriter::finish`: dopo, il footer e' gia'
/// stato emesso e la chiamata non avrebbe effetto — silenziosamente, che e' il
/// modo peggiore di non funzionare.
///
/// Non rende un `Result`, e non per brevita': `write_metadata` accumula in una
/// mappa e non puo' fallire. Un `Result` che non porta mai un errore invita a
/// scrivere una gestione che non serve, e a credere che il fallimento sia
/// stato considerato.
pub fn sigilla<W: Write>(scrittore: &mut FileWriter<W>, token: Option<&CommitToken>) {
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
///   canonico. Il messaggio e' un `&'static str`, quindi **non puo'** portare
///   il valore: non e' una disciplina da ricordare, e' il tipo che non lo
///   consente.
///
/// # Perche' e' dietro un `cfg`
///
/// Il suo primo chiamante reale e' la **verifica dell'artefatto**, che e'
/// `PR-6`. Qui il token si sa scrivere e si sa rileggere, e le quattro forme
/// del footer sono provate; ma chi rilegge per decidere qualcosa non esiste
/// ancora. Il `cfg` lo dice invece di lasciare che un `dead_code` lo dica
/// peggio, e sparisce con `PR-6`. Registrato in
/// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
#[cfg(any(test, feature = "internals"))]
pub fn leggi_commit_token<S: IpcSource + ?Sized>(
    sorgente: &mut S,
    limiti: &IpcLimits,
) -> Result<Option<CommitToken>, ArrowTransportError> {
    let trovato = valida_file_ed_estrai(sorgente, limiti, Some(CHIAVE_FOOTER_COMMIT_TOKEN))?;
    let Some(testo) = trovato else {
        return Ok(None);
    };
    CommitToken::da_esadecimale(&testo).map(Some).map_err(|_| {
        ArrowTransportError::IpcMetadataInvalid(
            "commit token del footer non canonico: atteso esadecimale minuscolo di 64 caratteri",
        )
    })
}

#[cfg(test)]
mod tests;
