//! Prove del giudizio puro del verificatore: il numero del terzo
//! descrittore, senza toccare l'ambiente vero.

use super::numero_da;
use crate::isolamento::canale::VARIABILE_ARTEFATTO;

#[test]
fn la_variabile_assente_e_un_rifiuto_nominato() {
    let motivo = numero_da(None).expect_err("assente");
    assert!(
        motivo.contains("non c'e'"),
        "il rifiuto non nomina l'assenza: {motivo}"
    );
}

#[test]
fn la_forma_canonica_passa() {
    for numero in [3, 4, 1024] {
        assert_eq!(numero_da(Some(&numero.to_string())), Ok(numero));
    }
}

/// `-1` e' la forma con cui una richiesta per il worker dichiara «assente» —
/// ma qui, dove la variabile e' scritta per davvero, un `-1` letto e' un
/// numero canonico come un altro: e' `descrittore_canonico` a giudicarlo, e
/// il senso di «nessun artefatto» appartiene al protocollo fra spawner e
/// richiesta, non a questa lettura.
#[test]
fn un_negativo_ha_forma_canonica_ma_non_e_un_descrittore_ammissibile() {
    assert_eq!(numero_da(Some("-1")), Ok(-1));
}

#[test]
fn ogni_forma_non_canonica_e_un_rifiuto() {
    for grezzo in ["+3", "03", "-0", " 3", "3 ", "", "non-un-numero"] {
        assert!(
            numero_da(Some(grezzo)).is_err(),
            "«{grezzo}» non dovrebbe avere forma canonica"
        );
    }
}

// L'unico test di questo file che tocca l'ambiente per davvero: nessun altro
// test, in questo processo, legge o scrive questa variabile — stessa ragione
// per cui `numero_da` sopra e' provata pura, senza toccare l'ambiente
// affatto (`attivazione::tests::lettura_reale_della_politica_dell_host`
// segue lo stesso principio per la propria variabile).
#[test]
fn lettura_reale_del_numero_dell_artefatto() {
    let originale = std::env::var(VARIABILE_ARTEFATTO).ok();

    std::env::remove_var(VARIABILE_ARTEFATTO);
    let motivo = super::numero_artefatto_dall_ambiente()
        .expect_err("assente: deve rifiutare, mai un default indovinato");
    assert!(
        motivo.category() == plenora_core::error::ErrorCategory::IsolationUnavailable,
        "{motivo:?}"
    );

    std::env::set_var(VARIABILE_ARTEFATTO, "non-un-numero");
    assert!(super::numero_artefatto_dall_ambiente().is_err());

    std::env::set_var(VARIABILE_ARTEFATTO, "7");
    assert_eq!(
        super::numero_artefatto_dall_ambiente().expect("un numero canonico deve leggersi"),
        7
    );

    match originale {
        Some(valore) => std::env::set_var(VARIABILE_ARTEFATTO, valore),
        None => std::env::remove_var(VARIABILE_ARTEFATTO),
    }
}
