//! Asserzioni condivise: l'atteso viene dalla fixture, mai dal risultato.

// Ogni modulo di prove include lo stesso file, anche quando usa un solo aiuto.
#![allow(unused_macros)]

/// Rifiuto con categoria e fase dichiarate dal chiamante.
macro_rules! assert_rifiuto {
    ($risultato:expr, $categoria:expr, $fase:expr $(,)?) => {{
        let Err(errore) = $risultato else {
            panic!("lo scenario deve essere rifiutato");
        };
        assert_eq!(errore.category(), $categoria, "categoria del rifiuto");
        assert_eq!(errore.phase(), $fase, "fase del rifiuto");
        errore
    }};
}

/// Successo con valore completo atteso, inclusi i metadati del tipo confrontato.
macro_rules! assert_successo {
    ($risultato:expr, $atteso:expr $(,)?) => {{
        let ottenuto = $risultato.expect("lo scenario deve riuscire");
        assert_eq!(ottenuto, $atteso, "valore atteso dello scenario");
        ottenuto
    }};
}
