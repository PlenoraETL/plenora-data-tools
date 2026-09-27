//! Il protocollo fra supervisore e worker: la forma sul filo, la sua
//! scrittura e lettura, e la verifica pura dell'handshake.
//!
//! Niente qui apre un pipe, avvia un processo o parla con un worker.
//!
//! # Perche' e' privato
//!
//! Renderlo pubblico significherebbe promettere di non cambiare un canale fra
//! due processi che spediamo insieme. Il crate `fuzz/` e la sonda di
//! calibrazione passano da `crate::interni`, che rende un verdetto e una
//! costante e **non** i tipi di questo modulo.

pub mod assi;
pub mod codifica;
// Come questa build descrive se stessa: il primo chiamante e' il worker, che
// deve dire all'altro lato chi e'.
pub mod descrizione;
/// Il digest SHA-256 sul filo.
///
/// `pub` perche' i messaggi lo espongono in campi `pub`, e un tipo meno
/// visibile del campo non si puo' nominare; fuori dal crate non esce, perche'
/// `protocollo` e' privato.
pub mod digest;
pub mod handshake;
pub mod lettore;
pub mod limiti;
pub mod messaggi;

#[cfg(test)]
mod tests;
