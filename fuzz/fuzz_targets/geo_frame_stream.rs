#![no_main]

//! Lettore dello stream geometrico `PLNGEO2` contro byte ostili
//! (errori-e-limiti.md#lo-spill-dimensiona-un-buffer-su-una-lunghezza-dichiarata).
//!
//! E' il secondo dei due confini che leggono da una sorgente non fidata — il
//! primo e' l'envelope `PLNGEO3`, che ha gia' il suo target — e senza questo
//! nessuna campagna lo raggiunge: `protocollo_frame` esercita il protocollo
//! supervisore/worker, che e' un altro codice e un altro formato.
//!
//! Invarianti pretese, per qualunque ingresso:
//!
//! - mai un panico, mai un blocco: ogni ingresso rende `Ok` o un errore
//!   tipizzato;
//! - **nessuna richiesta a `Read::read` supera la finestra fissa di 16 KiB**,
//!   su tutti i percorsi compresi quelli d'errore;
//! - un frame accettato non porta piu' byte di quanti la sorgente ne
//!   contenesse, e la somma dei frame non li supera;
//! - dopo la fine il lettore non torna a rendere frame.
//!
//! La seconda e' la difesa sull'**amplificazione**, ed e' l'unica che la
//! sorveglia: la lunghezza di un frame arriva dallo stream e puo' dichiarare
//! fino a 64 MiB, ma la fetta che il lettore chiede resta il buffer fisso. Un
//! lettore che dimensionasse sul dichiarato la romperebbe anche quando poi
//! fallisce, perche' la richiesta precede l'EOF che la delude.
//!
//! La terza e' un'altra promessa, e non copre la prima: un frame accettato non
//! materializza byte che la sorgente non ha consegnato. Protegge dalla
//! duplicazione, non dall'allocazione anticipata.
//!
//! # I due ingressi
//!
//! I byte grezzi, con un `schema_rows` preso dall'ingresso stesso, esercitano
//! l'header: magic, contatore fuori tetto, disaccordo fra schema e stream.
//!
//! Gli stessi byte con **un header giusto davanti** esercitano quello che c'e'
//! dopo: lunghezze dei frame, frame nulli, tetti, trailer, checksum, byte
//! residui. Senza il secondo ingresso il fuzzer spenderebbe quasi tutto il suo
//! tempo a sbagliare i sedici byte iniziali, e il lettore dei frame resterebbe
//! di fatto fuori dalla campagna.

use libfuzzer_sys::fuzz_target;
use plenora_engine::interni::verifica_lettore_frame_geo;

/// Le invarianti stanno **dentro** il crate, in `interni`: qui si applicano e
/// si abortisce, ma non si decide che cosa significhino.
///
/// Scritte qui sarebbero compilate solo dalla toolchain nightly. Scritte li'
/// le compila e le controlla ogni build normale, e la suite ordinaria le
/// esercita senza aspettare la campagna.
fn controlla(byte: &[u8], righe: u64) {
    if let Err(motivo) = verifica_lettore_frame_geo(byte, righe) {
        panic!("invariante del lettore rotta: {motivo}");
    }
}

/// Quante righe far dichiarare all'header sintetico.
///
/// Poche: ogni riga costa al fuzzer quattro byte di lunghezza da indovinare
/// prima di arrivare al trailer, e un contatore grande renderebbe la chiusura
/// dello stream irraggiungibile in pratica. Il ramo dei contatori enormi resta
/// coperto dal primo ingresso, che li prende dai byte grezzi.
const RIGHE_MASSIME_SINTETICHE: u64 = 8;

fuzz_target!(|byte: &[u8]| {
    // Il contatore atteso viene dall'ingresso: cosi' il fuzzer controlla anche
    // il lato del chiamante, non solo quello dello stream.
    let dai_byte = byte
        .first()
        .map_or(0, |primo| u64::from(*primo) * 0x0001_0000_0000);
    controlla(byte, dai_byte);

    // Gli stessi byte dietro un header valido e concorde.
    let righe = u64::from(byte.first().copied().unwrap_or(0)) % (RIGHE_MASSIME_SINTETICHE + 1);
    let mut inquadrato = Vec::with_capacity(16 + byte.len());
    inquadrato.extend_from_slice(b"PLNGEO2\0");
    inquadrato.extend_from_slice(&righe.to_le_bytes());
    inquadrato.extend_from_slice(byte);
    controlla(&inquadrato, righe);
});
