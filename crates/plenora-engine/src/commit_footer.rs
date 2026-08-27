//! Il `commit_token` nel footer di un artefatto Arrow IPC.
//!
//! # Che cosa e', e soprattutto che cosa non e'
//!
//! Il token e' l'**identita' del tentativo**: dice di quale esecuzione questo
//! file e' il prodotto, e serve a correlare artefatto, `Saluto` e verifica
//! (`isolamento.md`, passo 8-bis). E' un ingresso dell'esecuzione, scelto dal
//! chiamante prima dell'invocazione come il percorso di destinazione.
//!
//! **Non e' una credenziale e non prova nulla sull'autenticita' del file.**
//! Non c'e' firma, non c'e' MAC, non c'e' chiave: chiunque sappia scrivere un
//! file Arrow IPC puo' metterci dentro il token che vuole. Chi legge il token
//! impara **quale tentativo dichiara** di aver prodotto il file, non che quel
//! tentativo lo abbia prodotto davvero, e nemmeno chi fosse autorizzato a
//! farlo. Trattarlo come una prova sarebbe il difetto peggiore che possa
//! nascere qui: una guardia che sembra proteggere e non protegge.
//!
//! # Tre cose diverse nello stesso file, da non confondere
//!
//! - il **marcatore durevole** del footer, scritto da `FileWriter::finish`, e'
//!   cio' che il framing rinforzato verifica: dice che il file e' finito;
//! - il `commit_token`, che dice di quale tentativo il file e' il prodotto;
//! - il **digest dell'artefatto**, calcolato sull'intero file finalizzato e
//!   trasmesso nell'`Esito`: scriverlo dentro il file che copre sarebbe
//!   autoreferenziale. E' l'unico dei tre che dica qualcosa sul **contenuto**.
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
//! `FileReader::custom_metadata` sarebbe una terza strada nel footer, e dei
//! controlli della traversata rinforzata — allocazione limitata, chiavi e
//! valori presenti, duplicati rifiutati — non ne farebbe nessuno. Qui si passa da
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
//!   scrivere e' un artefatto di cui non sappiamo dire a quale tentativo
//!   appartenga — e un token che non e' canonico non e' un token.
//!
//! Che il token sia **obbligatorio** e' una proprieta' del percorso isolato,
//! non di questa funzione: qui si dice cosa c'e', non se doveva esserci.

use std::io::Write;

use plenora_core::arrow::ipc::writer::FileWriter;

use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
// Servono soltanto a `leggi_commit_token`, che e' dietro un `cfg`: portano lo
// stesso, o la build ordinaria della lib segnala tre import inutilizzati — e un
// warning tollerato e' un warning che smette di essere letto.
#[cfg(test)]
use crate::geo_transport::error::ArrowTransportError;
#[cfg(test)]
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
///   canonico. Il messaggio e' un `&'static str`, quindi **non puo'** portare
///   il valore: non e' una disciplina da ricordare, e' il tipo che non lo
///   consente.
///
/// # Perche' e' dietro un `cfg`
///
/// Non ha ancora un chiamante di produzione: il token si sa scrivere e si sa
/// rileggere, e le quattro forme del footer sono provate, ma chi rilegge per
/// **decidere** qualcosa non esiste. Il `cfg` dichiara quella condizione
/// invece di lasciare che un `dead_code` la dica peggio. Regola, perimetro e
/// condizione di rientro stanno in
/// errori-e-limiti.md#moduli-compilati-solo-sotto-test-e-internals.
///
/// **`test` e non `any(test, internals)`**, a differenza di `protocollo`:
/// questo modulo e' `pub(crate)`, quindi la facciata `interni` non lo
/// raggiunge e la feature non gli porterebbe nessun chiamante. Le porterebbe
/// un `dead_code` nella build che la abilita, cioe' quella del fuzzer.
#[cfg(test)]
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
