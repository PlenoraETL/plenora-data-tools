//! Facciata **minima** per i due consumatori fuori dal crate.
//!
//! # Superficie pubblica instabile e non-production
//!
//! Questo modulo esiste solo con la feature `internals`, che non e' nel
//! `default` e che **nessun consumatore di produzione** abilita: gli unici a
//! farlo sono il crate `fuzz/` e la sonda di calibrazione, che sono strumenti
//! di verifica di questo repository. Non ha garanzie di stabilita' e non e'
//! pensato per essere usato in produzione: puo' cambiare o sparire senza
//! preavviso.
//!
//! # Perche' una facciata e non il modulo
//!
//! Esporre `pub mod protocollo` sotto la stessa feature lo renderebbe API
//! pubblica a tutti gli effetti ogni volta che la feature e' attiva — e il
//! crate `fuzz/` e' precisamente un consumatore che la attiva. «Privato
//! tranne che per chi lo usa» non e' privato.
//!
//! Qui invece escono due cose sole, e nessun DTO del protocollo:
//!
//! - [`verifica_giro_del_frame`], che esercita codificatore e decodificatore
//!   **dall'interno** e restituisce un verdetto, non una struttura;
//! - [`MAX_PIANO_CANONICO_BYTES`], l'unico limite che la sonda di
//!   calibrazione deve leggere.
//!
//! Il vantaggio non e' solo di superficie: scritte qui, le invarianti del
//! fuzzer stanno **dentro** il crate, quindi le compila e le controlla la
//! build normale invece della sola toolchain nightly.

use crate::protocollo::codifica::{codifica, decodifica, BYTE_PREFISSO, MAX_PROTOCOL_FRAME_BYTES};

/// Il tetto della forma canonica di un piano nel profilo isolato.
///
/// Ri-esportato e non ridefinito: due copie dello stesso numero sono due
/// numeri che possono divergere.
pub const MAX_PIANO_CANONICO_BYTES: usize = crate::protocollo::limiti::MAX_PIANO_CANONICO_BYTES;

/// Esercita il giro completo su byte arbitrari e rende un verdetto.
///
/// Un ingresso che non e' un frame **non e' un guasto**: la gran parte di
/// cio' che produce un fuzzer non lo e', e restituisce `Ok(())`. Sono guasti
/// solo le rotture d'invariante.
///
/// # Errors
///
/// Descrive quale invariante e' saltata:
///
/// - un frame decodificato che non si ricodifica;
/// - una forma canonica oltre [`MAX_PROTOCOL_FRAME_BYTES`];
/// - un prefisso che non dichiara i byte del payload;
/// - una forma canonica che non si rilegge, o che rilegge un'altra struttura;
/// - una seconda codifica diversa dalla prima.
///
/// Le ultime due sono la difesa contro la deriva silenziosa fra i due versi:
/// un decoder che accettasse qualcosa che il codificatore non sa riprodurre
/// romperebbe il giro senza che nessun test nominale se ne accorga.
pub fn verifica_giro_del_frame(byte: &[u8]) -> Result<(), String> {
    let Ok(frame) = decodifica(byte) else {
        return Ok(());
    };

    let canonico = codifica(&frame)
        .map_err(|origine| format!("un frame decodificato non si ricodifica: {origine}"))?;
    let payload = canonico
        .len()
        .checked_sub(BYTE_PREFISSO)
        .ok_or_else(|| "il frame codificato non contiene il prefisso".to_owned())?;
    if payload > MAX_PROTOCOL_FRAME_BYTES {
        return Err(format!(
            "la forma canonica supera il tetto: {payload} byte > {MAX_PROTOCOL_FRAME_BYTES}"
        ));
    }

    let dichiarata =
        u32::from_be_bytes([canonico[0], canonico[1], canonico[2], canonico[3]]) as usize;
    if dichiarata != payload {
        return Err(format!(
            "il prefisso dichiara {dichiarata} byte, il payload ne ha {payload}"
        ));
    }

    let riletto = decodifica(&canonico)
        .map_err(|origine| format!("la forma canonica non si rilegge: {origine}"))?;
    if riletto != frame {
        return Err("il giro non rende la stessa struttura".to_owned());
    }
    let di_nuovo =
        codifica(&riletto).map_err(|origine| format!("la ricodifica fallisce: {origine}"))?;
    if di_nuovo != canonico {
        return Err("codifica non deterministica".to_owned());
    }
    Ok(())
}
