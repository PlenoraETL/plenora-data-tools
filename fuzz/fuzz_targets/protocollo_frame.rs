#![no_main]

//! Decoder del protocollo supervisore/worker contro byte ostili
//! (isolamento.md#4-protocollo-interno,
//! errori-e-limiti.md#protocollo-del-worker-i-tetti-sono-del-profilo-isolato).
//!
//! E' il codice che legge byte scritti da un **altro processo**: e' il punto
//! del sistema con la superficie d'attacco piu' diretta, e l'unico in cui un
//! payload arbitrario arriva a un parser senza passare da nessuna validazione
//! precedente.
//!
//! Invarianti pretese, per qualunque ingresso:
//!
//! - mai un panico, mai un blocco: ogni ingresso rende `Ok` o un errore
//!   tipizzato;
//! - un frame accettato si **ricodifica sempre**, e la sua forma canonica
//!   sta sotto il tetto;
//! - il prefisso prodotto dichiara esattamente i byte del payload;
//! - rileggere la forma canonica rende la **stessa** struttura, e
//!   ricodificarla rende gli **stessi** byte.
//!
//! Le ultime due sono la difesa contro la deriva silenziosa fra i due versi:
//! un decoder che accettasse qualcosa che il codificatore non sa riprodurre
//! romperebbe il giro senza che nessun test nominale se ne accorga.
//!
//! # I due ingressi
//!
//! Il payload grezzo esercita il **framing**: prefisso troncato, lunghezze
//! incoerenti, byte in eccesso, lunghezze oltre il tetto.
//!
//! Il payload **inquadrato** — gli stessi byte con davanti il prefisso giusto
//! — esercita quello che c'e' dopo: UTF-8, JSON, chiavi duplicate, campi
//! ignoti, enum fuori dominio, tetti per campo. Senza il secondo ingresso il
//! fuzzer spenderebbe quasi tutto il suo tempo a sbagliare i quattro byte
//! iniziali, e il parser vero resterebbe di fatto fuori dalla campagna.

use libfuzzer_sys::fuzz_target;
use plenora_engine::interni::verifica_giro_del_frame;

/// Le invarianti stanno **dentro** il crate, in `interni`: qui si applicano e
/// si abortisce, ma non si decide che cosa significhino.
///
/// Scritte qui sarebbero compilate solo dalla toolchain nightly. Scritte li'
/// le compila e le controlla ogni build normale, e la suite ordinaria le
/// esercita senza aspettare la campagna.
fn controlla(byte: &[u8]) {
    if let Err(motivo) = verifica_giro_del_frame(byte) {
        panic!("invariante del protocollo rotta: {motivo}");
    }
}

fuzz_target!(|payload: &[u8]| {
    // 1. I byte come arrivano: e' il framing a essere sotto esame.
    controlla(payload);

    // 2. Gli stessi byte con la cornice giusta: e' tutto il resto.
    if let Ok(lunghezza) = u32::try_from(payload.len()) {
        let mut inquadrato = Vec::with_capacity(4 + payload.len());
        inquadrato.extend_from_slice(&lunghezza.to_be_bytes());
        inquadrato.extend_from_slice(payload);
        controlla(&inquadrato);
    }
});
