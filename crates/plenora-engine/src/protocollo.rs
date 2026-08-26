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
//! # Perche' e' privato
//!
//! Il protocollo e' interno. Renderlo pubblico significherebbe promettere di
//! non cambiarlo, e non c'e' ragione di promettere a nessuno la forma di un
//! canale fra due processi che spediamo insieme.

// I sei messaggi esistono come **forma** e non hanno ancora un chiamante: chi
// li costruisce e' l'handshake, che per perimetro esplicito appartiene alla PR
// successiva. L'alternativa sarebbe stata portare qui un pezzo di supervisore
// solo per far usare i tipi, cioe' allargare il perimetro per zittire un
// avviso. Questo `allow` **scade con PR-5**: quando l'handshake esiste, i tipi
// hanno un chiamante vero e va tolto.
#![allow(dead_code)]

pub mod codifica;
pub mod limiti;
pub mod messaggi;

#[cfg(test)]
mod tests;
