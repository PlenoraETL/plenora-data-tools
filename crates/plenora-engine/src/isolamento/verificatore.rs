//! Il verificatore: che cosa fa appena nasce, prima di dire qualunque cosa.
//!
//! # Mirror di `worker`, non una sua variante
//!
//! Stessa disciplina, stesso confine fra «prima che il canale esista» e
//! «dopo l'accordo», e per la stessa ragione: fino all'accordo non c'e'
//! nessuno a cui dire niente, e da li' in poi ogni fallimento diventa un
//! esito dichiarato invece di un rifiuto muto — ma non lo **stesso** tipo di
//! esito del worker: vedi sotto.
//!
//! Cio' che distingue il verificatore dal worker non e' il canale — le due
//! pipe si riaprono e si accertano esattamente allo stesso modo, e infatti
//! questo modulo **riusa** [`super::worker::accerta_gli_estremi`] invece di
//! duplicarlo — ma:
//!
//! - un **terzo** descrittore ereditato, l'artefatto in sola lettura, che il
//!   worker non riceve mai;
//! - l'incarico che riceve dopo l'accordo e' [`IncaricoVerifica`], non
//!   [`Incarico`](crate::protocollo::messaggi::Incarico): non un piano da
//!   eseguire, ma un contratto atteso, un digest atteso, conteggi attesi e un
//!   budget di memoria da cui derivare i tetti del confine ostile Arrow IPC;
//! - l'esito che dichiara e' [`crate::protocollo::messaggi::EsitoVerificaSulFilo`]
//!   dentro `Corpo::EsitoVerifica`, non [`crate::protocollo::messaggi::EsitoWorkerSulFilo`]
//!   dentro `Corpo::Esito`: stessa forma (successo con digest/conteggi
//!   riconfermati, errore tipizzato, panic), tipo distinto sul filo — un
//!   `EsitoVerificaSulFilo::Successo` dice «ho riconfermato», non «ho
//!   eseguito». `isolamento::macchina::Ruolo::Verificatore` e' cio' che
//!   dice alla macchina a stati del coordinatore di aspettarsi questo corpo
//!   e non l'altro;
//! - **nessuna capability offerta**: il verificatore non esegue nessun
//!   kernel, quindi non ha niente da dichiarare di piu' della sola identita'
//!   (artefatto, resolver, ambiente) — e il supervisore, dal proprio lato
//!   (`isolamento::prova::supervisore_per`), non ne richiede nessuna;
//! - **nessuna capability di pubblicazione**: questo processo non riceve mai
//!   la destinazione finale, ne' un percorso ne' un handle di scrittura su
//!   di essa. Il solo descrittore che riceve sull'artefatto e' aperto in
//!   sola lettura dal coordinatore, e lo resta — lo stesso principio di
//!   `GA-5` per il worker, qui applicato a un processo che non ha nemmeno il
//!   piano da cui la destinazione potrebbe dedursi.

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
/// # Perche' separati
///
/// Stessa ragione di [`super::worker::numeri_da`]: l'ambiente e' globale al
/// processo, e casi che lo scrivessero si darebbero fastidio a vicenda
/// girando in parallelo. Isolato, il giudizio e' una funzione pura, e le
/// forme storte si scrivono invece di produrle mutando l'ambiente vero.
///
/// # Perche' la forma si giudica con `descrittore_canonico`
///
/// Perche' e' la stessa funzione che rilegge gli argomenti dello spawner —
/// la forma che il coordinatore scrive con `i32::to_string()` e' una sola, e
/// un secondo giudizio scritto qui potrebbe divergere dal primo.
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
/// # L'ordine
///
/// 1. le due pipe, con [`super::worker::accerta_gli_estremi`] — che accerta
///    gia' da solo il monothread, i due numeri, la riapertura di ciascuna e
///    che non siano la stessa pipe;
/// 2. il terzo descrittore, **dopo**: se il canale non regge non c'e' modo
///    di dire al coordinatore che l'artefatto e' inaccessibile, quindi non
///    ha senso accertarlo per primo.
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
/// Stessa sequenza di [`super::worker`], con una sola differenza dichiarata:
/// la descrizione locale non offre **nessuna** capability — il verificatore
/// non attraversa nessun kernel, e dichiararne una direbbe una capacita' che
/// questo processo non esercita.
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
/// # Perche' l'errore della verifica non esce di qui
///
/// Perche' esce **sul filo**, esattamente come per il worker: un artefatto
/// che non supera i passi 3-8-bis non e' un fallimento del *verificatore* —
/// e' l'esito che il coordinatore deve leggere per non pubblicare. Cio' che
/// invece esce di qui e' il fallimento del **canale**: se l'esito non parte,
/// il coordinatore non ha niente da leggere.
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
/// # Perche' rende gia' l'`EsitoVerificaSulFilo`
///
/// Perche' il successo non porta la prova opaca che
/// `verifica::verifica_artefatto_handle` costruisce (`ArtefattoVerificato`):
/// quella prova vale nel processo che l'ha costruita, e non attraversa un
/// confine — lo stesso principio per cui `isolamento::prova::riverifica` la
/// lascia cadere. Cio' che il coordinatore ha bisogno di sapere e' digest e
/// conteggi **riconfermati**, e questa funzione li rende esattamente uguali
/// a quelli **attesi**: se la verifica e' riuscita, `attese.digest` e
/// `attese.conteggi` sono per costruzione i valori che l'artefatto ha
/// dimostrato di avere — passo 5-bis e passo 8 lo pretendono prima di
/// concludere — quindi non c'e' un secondo valore «osservato» da leggere
/// fuori dalla prova opaca.
///
/// # Perche' non rende un `Result`
///
/// Perche' non c'e' mai un errore da risalire: gli errori del confine
/// diventano `EsitoVerificaSulFilo::Errore`, dentro il valore stesso che
/// questa funzione rende sempre. Il canale del filo porta **un** esito, non
/// un esito e un errore separato — un `Result<EsitoVerificaSulFilo>` che non
/// e' mai `Err` direbbe una cosa falsa a chi legge la firma.
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
