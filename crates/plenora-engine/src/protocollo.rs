//! Il protocollo fra supervisore e worker: **solo la forma serializzata**.
//!
//! Questo modulo definisce cosa viaggia sul filo e come si scrive e si legge.
//! Non definisce **quando**: la costruzione dell'handshake e la verifica
//! semantica dei suoi campi appartengono alla PR successiva, e tenerle fuori
//! e' deliberato — un modulo che sa serializzare e anche decidere e' un
//! modulo in cui un errore di decisione si nasconde dietro un errore di
//! formato.
//!
//! Niente qui apre un pipe, avvia un processo o parla con un worker.
//!
//! # Perche' e' privato, e come lo raggiunge chi sta fuori
//!
//! Il protocollo e' interno. Renderlo pubblico significherebbe promettere di
//! non cambiarlo, e non c'e' ragione di promettere a nessuno la forma di un
//! canale fra due processi che spediamo insieme.
//!
//! Il crate `fuzz/` e la sonda di calibrazione stanno fuori dal crate e non
//! possono entrare qui: passano da `crate::interni`, che rende un verdetto e
//! una costante e **non** i tipi di questo modulo.

pub mod codifica;
pub mod handshake;
pub mod lettore;
pub mod limiti;
pub mod messaggi;

#[cfg(test)]
mod tests;
