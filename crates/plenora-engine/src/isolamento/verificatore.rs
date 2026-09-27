//! Il verificatore: che cosa fa appena nasce, prima di dire qualunque cosa.
//!
//! Mirror di `worker`: stesso confine fra «prima che il canale esista» e «dopo
//! l'accordo», e le due pipe si accertano con
//! [`super::worker::accerta_gli_estremi`]. Le differenze:
//!
//! - un **terzo** descrittore ereditato, l'artefatto in sola lettura;
//! - riceve [`IncaricoVerifica`], non
//!   [`Incarico`](crate::protocollo::messaggi::Incarico): contratto, digest e
//!   conteggi attesi, e il budget da cui derivare i tetti del confine ostile
//!   Arrow IPC;
//! - dichiara [`crate::protocollo::messaggi::EsitoVerificaSulFilo`] in
//!   `Corpo::EsitoVerifica`, non
//!   [`crate::protocollo::messaggi::EsitoWorkerSulFilo`]: stessa forma, tipo
//!   distinto («ho riconfermato», non «ho eseguito»), e
//!   `isolamento::macchina::Ruolo::Verificatore` dice alla macchina quale
//!   aspettarsi;
//! - **nessuna capability offerta**, perche' non esegue kernel, e il
//!   supervisore (`isolamento::prova::supervisore_per`) non ne richiede;
//! - **nessuna capability di pubblicazione**: non riceve la destinazione ne'
//!   un handle di scrittura, come `GA-5` per il worker.

use plenora_core::error::PlenoraError;

use super::canale;
use super::DalConfine;
use super::{non_disponibile, Result};
use crate::protocollo::{
    assi::{errore_dichiarabile, forma_sul_filo},
    codifica::codifica,
    descrizione,
    handshake::{WorkerAccordato, WorkerInAttesa},
    lettore::leggi_frame,
    messaggi::{Corpo, EsitoVerificaSulFilo, Frame, IncaricoVerifica},
};

/// I tre estremi del verificatore, riaperti e verificati: le due pipe del
/// canale — identiche a quelle del worker — piu' l'artefatto in sola
/// lettura.
struct Estremi {
    legge: std::fs::File,
    scrive: std::fs::File,
    /// L'artefatto. Non si apre finche' non serve (dopo l'incarico): tenerlo
    /// qui invece che riaprirlo a quel punto renderebbe pero' rappresentabile
    /// un verificatore accordato senza un artefatto da rileggere, che e' uno
    /// stato che non deve esistere — quindi si riapre e si accerta **qui**,
    /// insieme alle pipe, e non dopo.
    artefatto: std::fs::File,
}

/// Il numero del terzo descrittore, letto dall'ambiente e **non ancora
/// creduto**.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], con la ragione nominata.
fn numero_artefatto_dall_ambiente() -> Result<i32> {
    let quale = canale::VARIABILE_ARTEFATTO;
    numero_da(std::env::var(quale).ok().as_deref()).map_err(|motivo| {
        non_disponibile("artefatto", &format!("la variabile «{quale}»: {motivo}"))
    })
}

/// Il giudizio sul valore, separato dalla lettura dell'ambiente.
///
/// Come [`super::worker::numeri_da`]: l'ambiente e' globale al processo, e il
/// giudizio puro si prova senza mutarlo. La forma si giudica con
/// `descrittore_canonico`, la stessa funzione degli argomenti dello spawner.
///
/// # Errors
///
/// Il motivo, in forma di frase: «la variabile non c'e'» se `grezzo` e'
/// `None`, altrimenti quello di [`super::descrittore_canonico`].
fn numero_da(grezzo: Option<&str>) -> std::result::Result<i32, String> {
    let Some(grezzo) = grezzo else {
        return Err("non c'e': il verificatore non sa da dove rileggere l'artefatto".to_owned());
    };
    super::descrittore_canonico(grezzo)
}

/// Riapre i tre estremi e **accerta** che siano quelli.
///
/// Prima le due pipe ([`super::worker::accerta_gli_estremi`]), poi il terzo
/// descrittore: senza canale non si puo' dire al coordinatore che l'artefatto
/// e' inaccessibile.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`], con la ragione precisa.
fn accerta_gli_estremi() -> Result<Estremi> {
    let super::worker::Estremi { legge, scrive } = super::worker::accerta_gli_estremi()?;
    let numero = numero_artefatto_dall_ambiente()?;
    let artefatto = canale::riapri_accertato_artefatto(numero)?;
    Ok(Estremi {
        legge,
        scrive,
        artefatto,
    })
}

/// Conclude l'accordo con il coordinatore.
///
/// Stessa sequenza di [`super::worker`], ma la descrizione locale non offre
/// **nessuna** capability: il verificatore non esegue kernel.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se il canale non regge; quello che
/// rende l'handshake se le due descrizioni non concordano.
fn accordati(estremi: &mut Estremi) -> Result<WorkerAccordato> {
    let locale = descrizione::di_questa_build(Vec::new())?;
    let attesa = WorkerInAttesa::nuovo(locale)?;

    let Some(frame) = leggi_frame(&mut estremi.legge)? else {
        return Err(non_disponibile(
            "accordo",
            "il canale e' finito prima del saluto: il coordinatore non ha detto niente",
        ));
    };
    let (risposta, accordato) = attesa.ricevi(frame)?;

    manda(&mut estremi.scrive, Corpo::Risposta(Box::new(risposta)))?;
    Ok(accordato)
}

/// Scrive tutti i byte, e si assicura che partano.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se la scrittura non riesce.
fn scrivi_tutto(dove: &mut std::fs::File, byte: &[u8]) -> Result<()> {
    use std::io::Write as _;
    dove.write_all(byte)
        .map_err(|causa| non_disponibile("accordo", &format!("la risposta non parte: {causa}")))?;
    dove.flush()
        .map_err(|causa| non_disponibile("accordo", &format!("la risposta non arriva: {causa}")))
}

/// Manda un corpo, e si assicura che parta.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se la codifica o la scrittura non
/// riescono.
fn manda(dove: &mut std::fs::File, corpo: Corpo) -> Result<()> {
    let byte = codifica(&Frame::nuovo(corpo))?;
    scrivi_tutto(dove, &byte)
}

/// Il verificatore, dal confine: la sequenza intera.
///
/// Stessa struttura di [`super::worker::dal_confine`]: prima del canale un
/// rifiuto muto, dopo l'accordo ogni fallimento e' un `Esito` dichiarato.
#[cfg(target_os = "linux")]
pub(super) fn dal_confine() -> DalConfine {
    let mut estremi = match accerta_gli_estremi() {
        Ok(estremi) => estremi,
        Err(errore) => return DalConfine::Fallita(errore),
    };
    let accordato = match accordati(&mut estremi) {
        Ok(accordato) => accordato,
        Err(errore) => return DalConfine::Fallita(errore),
    };
    match lavora(estremi, accordato) {
        Ok(()) => DalConfine::Conclusa,
        Err(errore) => DalConfine::Fallita(errore),
    }
}

/// Riceve l'incarico di verifica, rilegge l'artefatto, e dichiara com'e'
/// andata.
///
/// L'errore della verifica esce **sul filo**: e' l'esito che il coordinatore
/// legge per non pubblicare. Di qui esce solo il fallimento del canale.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se l'incarico non arriva o l'esito
/// non parte; gli errori del protocollo se il frame non e' un
/// `IncaricoVerifica`.
fn lavora(estremi: Estremi, accordato: WorkerAccordato) -> Result<()> {
    let Estremi {
        mut legge,
        mut scrive,
        artefatto,
    } = estremi;

    let Some(frame) = leggi_frame(&mut legge)? else {
        let errore = non_disponibile(
            "incarico",
            "il canale e' finito prima dell'incarico di verifica: l'accordo c'e', il lavoro no",
        );
        return manda(
            &mut scrive,
            Corpo::EsitoVerifica(Box::new(esito_di_errore(&errore))),
        );
    };
    let (incarico, token) = match accordato.ricevi_incarico_verifica(frame) {
        Ok(coppia) => coppia,
        Err(causa) => {
            return manda(
                &mut scrive,
                Corpo::EsitoVerifica(Box::new(esito_di_errore(&causa))),
            )
        }
    };

    // Il lavoro vero — mai eseguito prima dell'incarico, per lo stesso
    // motivo per cui il worker non tocca dati prima dell'accordo — e' avvolto
    // nella stessa barriera anti-panico del worker: un panico dentro
    // `verifica::verifica_artefatto_handle` non deve uscire da questo
    // processo senza che il coordinatore riceva niente.
    let esito = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        verifica(&incarico, &token, artefatto)
    }));

    let corpo = match esito {
        Ok(esito_riuscito) => esito_riuscito,
        Err(payload) => EsitoVerificaSulFilo::Panic {
            forma: forma_sul_filo(payload.as_ref()),
        },
    };
    manda(&mut scrive, Corpo::EsitoVerifica(Box::new(corpo)))
}

/// Il corpo di un `EsitoVerifica` di errore, dichiarato.
///
/// Non `EsitoWorkerSulFilo`: e' un tipo distinto, con lo stesso nome sul filo
/// («esito») ma sotto un tag di messaggio diverso — vedi
/// [`EsitoVerificaSulFilo`]. Il verificatore non manda mai un `Corpo::Esito`.
fn esito_di_errore(causa: &PlenoraError) -> EsitoVerificaSulFilo {
    EsitoVerificaSulFilo::Errore {
        errore: Box::new(errore_dichiarabile(causa)),
    }
}

/// I passi da 3 a 8-bis, sull'artefatto ereditato — e nient'altro.
///
/// Rende gia' l'`EsitoVerificaSulFilo`: la prova opaca `ArtefattoVerificato`
/// non attraversa un confine. Sul successo digest e conteggi riconfermati
/// sono per costruzione quelli **attesi**, che i passi 5-bis e 8 pretendono.
/// Non rende un `Result`: gli errori del confine diventano
/// `EsitoVerificaSulFilo::Errore`, e un `Result` mai `Err` mentirebbe.
fn verifica(
    incarico: &IncaricoVerifica,
    token: &crate::commit_token::CommitToken,
    artefatto: std::fs::File,
) -> EsitoVerificaSulFilo {
    let limiti = crate::ipc_boundary::limits_from_memory_budget(
        usize::try_from(incarico.budget_memoria_governata_bytes).unwrap_or(usize::MAX),
    );
    let attese = crate::verifica::AtteseVerifica {
        contratto_fingerprint_atteso: incarico.contract_fingerprint_atteso,
        digest: &incarico.digest_atteso,
        conteggi: incarico.conteggi_attesi,
        commit_token: token,
    };
    match crate::verifica::verifica_artefatto_handle(
        artefatto,
        &attese,
        crate::risolutore::risolvi,
        &limiti,
    ) {
        // La prova si lascia cadere: e' opaca, non attraversa il confine, e
        // il coordinatore non la riceve — riceve i valori attesi, riconfermati.
        Ok(_prova) => EsitoVerificaSulFilo::Successo {
            digest_artefatto: incarico.digest_atteso.clone(),
            conteggi: incarico.conteggi_attesi,
        },
        Err(causa) => esito_di_errore(&causa),
    }
}

// La cancellazione non ha un ruolo qui: il verificatore non ascolta un
// `Annulla` mentre rilegge, perche' i suoi passi sono in streaming a memoria
// costante e a tempo limitato dal tetto del confine ostile — non da un
// lavoro che possa girare indefinitamente e su cui valga la pena
// interrompersi a meta'. La cancellazione dell'intero tentativo resta quella
// del coordinatore, che smette di aspettare e termina il dominio del
// verificatore come termina quello del worker (`isolamento::esecuzione_isolata`).

#[cfg(test)]
mod tests;
