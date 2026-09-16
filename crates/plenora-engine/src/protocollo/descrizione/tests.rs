//! I casi della descrizione locale.

use plenora_core::PlenoraError;
use sha2::{Digest as _, Sha256};

use crate::risolutore::Risolutore;

use crate::esadecimale32::Esadecimale32;

use super::{ambiente, resolver, DOMINIO_INSIEME};

/// Lo SHA-256 di questi byte, nella grafia canonica.
///
/// I casi passano dalla stessa autorita' del codice: un formattatore scritto
/// qui potrebbe concordare con l'implementazione per costruzione e non
/// accorgersi di niente.
fn digest_di(byte: &[u8]) -> String {
    Esadecimale32::dai_byte(Sha256::digest(byte).into()).in_esadecimale()
}

/// **Il digest dell'insieme vuoto e' quello del dominio, non quello del nulla.**
///
/// # Che cosa esclude
///
/// Che due digest diversi coincidano per caso. Senza il prefisso, l'insieme
/// vuoto sarebbe l'hash di zero byte — un valore che qualunque altro punto del
/// programma potrebbe produrre digerendo niente, e che l'handshake
/// confronterebbe come se significasse «stesso ambiente».
///
/// Il caso confronta con **entrambe** le cose sbagliate: l'hash del nulla, e
/// l'hash del dominio con un segnaposto dopo. Il secondo perche' l'insieme
/// vuoto e' l'assenza di elementi, non l'insieme di un elemento chiamato
/// «niente».
#[test]
fn il_digest_dell_insieme_vuoto_e_dominato() {
    let Ok(vuoto) = ambiente(Risolutore::SenzaBackend) else {
        panic!("senza backend l'ambiente si descrive");
    };
    let detto = vuoto.digest_insieme.in_esadecimale();

    let del_dominio = digest_di(DOMINIO_INSIEME.as_bytes());
    assert_eq!(detto, del_dominio, "il digest e' quello del dominio");

    let del_nulla = digest_di(b"");
    assert_ne!(detto, del_nulla, "senza dominio sarebbe l'hash del nulla");

    let mut con_segnaposto = Sha256::new();
    con_segnaposto.update(DOMINIO_INSIEME.as_bytes());
    con_segnaposto.update(b"");
    // Aggiungere zero byte non cambia un hash: e' proprio per questo che il
    // segnaposto non deve essere una stringa vuota ma **niente**.
    assert_eq!(
        detto,
        Esadecimale32::dai_byte(con_segnaposto.finalize().into()).in_esadecimale()
    );

    let mut con_zero = Sha256::new();
    con_zero.update(DOMINIO_INSIEME.as_bytes());
    con_zero.update([0_u8]);
    assert_ne!(
        detto,
        Esadecimale32::dai_byte(con_zero.finalize().into()).in_esadecimale(),
        "un byte di segnaposto renderebbe l'insieme vuoto un insieme di un elemento"
    );
}

/// L'ambiente senza backend dichiara **tutte e quattro** le cose, non solo il
/// digest.
///
/// `acquisizione_dinamica` e' dichiarata e non assunta: un backend che
/// scaricasse una griglia a esecuzione in corso renderebbe il digest una
/// fotografia scaduta, e il campo esiste per escluderlo.
#[test]
fn l_ambiente_senza_backend_e_dichiarato_per_intero() {
    let Ok(vuoto) = ambiente(Risolutore::SenzaBackend) else {
        panic!("senza backend l'ambiente si descrive");
    };
    assert!(!vuoto.acquisizione_dinamica);
    assert!(vuoto.risorse.is_empty());
    assert!(vuoto.backend_dinamici.is_empty());
}

/// **Con PROJ l'ambiente si rifiuta, e il rifiuto dice perche'.**
///
/// Non e' un errore d'esecuzione ma una condizione della **build**: chi lo
/// legge deve poter cambiare build, e per questo la variante e'
/// `InvalidConfiguration` e non `IsolationUnavailable`.
#[test]
fn con_proj_l_ambiente_si_rifiuta() {
    let esito = ambiente(Risolutore::Proj);
    let Err(PlenoraError::InvalidConfiguration(motivo)) = esito else {
        panic!("con PROJ l'ambiente non e' descrivibile: {esito:?}");
    };
    assert!(motivo.contains("proj"), "{motivo}");
    assert!(
        motivo.contains("inventariabile") && motivo.contains("errori-e-limiti"),
        "il rifiuto deve dire perche' e dove sta il rientro: {motivo}"
    );
}

/// L'identita' del resolver viene dal selettore, non da una costante sua.
///
/// E' la meta' che tiene insieme descrizione e comportamento: se venisse da
/// altrove, il worker potrebbe dichiarare un resolver e usarne un altro.
#[test]
fn l_identita_del_resolver_viene_dal_selettore() {
    for scelto in [Risolutore::SenzaBackend, Risolutore::Proj] {
        let detta = resolver(scelto);
        assert_eq!(detta.identita, scelto.identita());
        assert!(!detta.versione.is_empty());
    }
}

/// **Il digest dell'immagine e' stabile e in forma canonica.**
///
/// Stabile perche' l'immagine non cambia fra due letture consecutive: due
/// valori diversi direbbero che qualcosa la sta riscrivendo, ed e' proprio la
/// condizione che la funzione rifiuta.
#[test]
#[cfg(target_os = "linux")]
fn il_digest_dell_immagine_e_stabile() {
    let Ok(prima) = super::artefatto() else {
        panic!("l'immagine di questo processo si legge");
    };
    let Ok(dopo) = super::artefatto() else {
        panic!("e si legge due volte");
    };
    assert_eq!(prima.digest, dopo.digest, "due letture, due digest");
    assert_eq!(prima.digest.in_esadecimale().len(), 64);
    assert!(!prima.versione.is_empty());
}

/// **La descrizione intera si compone, oppure rifiuta per l'ambiente.**
///
/// Su una build senza backend arriva fino in fondo; con PROJ cade
/// sull'ambiente. In entrambi i casi l'artefatto e' stato misurato — e' il
/// primo dei tre — quindi il caso prova anche che l'ordine non nasconda un
/// rifiuto dietro l'altro.
#[test]
#[cfg(target_os = "linux")]
fn la_descrizione_intera_segue_il_selettore() {
    let esito = super::di_questa_build(vec!["prova".to_owned()]);
    if Risolutore::di_questa_build().ambiente_inventariabile() {
        let Ok(descritta) = esito else {
            panic!("senza backend la descrizione si compone");
        };
        assert_eq!(descritta.capability, vec!["prova".to_owned()]);
        assert_eq!(
            descritta.comune.resolver.identita,
            Risolutore::di_questa_build().identita()
        );
    } else {
        assert!(
            matches!(esito, Err(PlenoraError::InvalidConfiguration(_))),
            "con PROJ la descrizione rifiuta sull'ambiente"
        );
    }
}
