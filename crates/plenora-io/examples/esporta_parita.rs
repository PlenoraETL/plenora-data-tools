//! Corpus completo dalle schede, condiviso dalle prove delle cinque superfici.
#![allow(dead_code)] // Le altre funzioni del modulo restano prove/documentazione IO.
#[path = "../tests/operazioni_doc.rs"]
mod schede;
fn main() {
    let directory = std::env::args_os().nth(1).expect("cartella corpus");
    schede::esporta_casi_superfici(std::path::Path::new(&directory));
}
