//! Come questa build **descrive se stessa** all'altro lato del filo.
//!
//! # Perche' ciascun lato si descrive da solo
//!
//! Perche' l'handshake confronta due descrizioni, e un confronto ha senso solo
//! se le due arrivano da due misure indipendenti. Un worker che rispecchiasse
//! la descrizione ricevuta direbbe sempre «siamo d'accordo»: il confronto
//! resterebbe verde per costruzione, e la protezione contro un worker che non
//! e' quello atteso sparirebbe senza che niente lo dica.
//!
//! Le tre parti — artefatto, resolver, ambiente — si misurano quindi qui, dal
//! processo che le dichiara.
//!
//! # Che cosa questa build **non** sa descrivere
//!
//! L'ambiente PROJ. Con `proj-backend` la descrizione **rifiuta**, e il rifiuto
//! e' la cosa corretta da fare: non esiste una radice esclusiva, immutabile e
//! inventariabile da cui ricavare l'insieme delle risorse disponibili. La
//! ragione, il perimetro e le condizioni di rientro stanno in
//! `errori-e-limiti.md`.

use std::io::Read as _;

use plenora_core::{PlenoraError, Result};
use sha2::{Digest as _, Sha256};

use crate::esadecimale32::Esadecimale32;
use crate::risolutore::{Risolutore, VERSIONE};

use super::digest::DigestSha256;
use super::handshake::{Descrizione, DescrizioneLocale};
use super::messaggi::{Ambiente, IdentitaArtefatto, IdentitaResolver};

/// Il percorso dell'immagine in esecuzione.
///
/// Non e' il nome con cui il programma e' stato invocato: quello si puo'
/// sostituire fra il momento in cui lo si guarda e quello in cui lo si legge —
/// e' una `rename`, ed e' atomica. Questo collegamento il kernel lo tiene
/// legato all'immagine **di questo processo**, e non c'e' nessuna risoluzione
/// da rifare.
#[cfg(target_os = "linux")]
const IMMAGINE: &str = "/proc/self/exe";

/// Quanto si legge per volta, digerendo l'immagine.
///
/// # Perche' un buffer fisso, e perche' nessun tetto totale
///
/// Perche' il buffer **e'** il limite di memoria: si legge a blocchi e si
/// digerisce, quindi l'occupazione non cresce con la dimensione del file.
/// Aggiungere un tetto totale e presentarlo come limite di memoria direbbe una
/// cosa falsa — la memoria e' gia' limitata da questo numero — e ne
/// introdurrebbe uno arbitrario su cio' che l'immagine puo' essere grande.
///
/// Il tempo resta governato dall'handshake, che ha una scadenza sua: un'immagine
/// abbastanza grande da renderlo lento fa scadere quella, e il rifiuto arriva
/// da chi misura il tempo invece che da chi legge i byte.
const BLOCCO: usize = 64 * 1024;

/// Il dominio del digest dell'insieme delle risorse.
///
/// # Perche' un prefisso, e perche' versionato
///
/// Perche' un digest senza dominio e' un digest di byte, e gli stessi byte
/// possono voler dire due cose. Il prefisso separa **questo** digest da ogni
/// altro digest del programma: nessun contenuto casuale coincide con l'insieme
/// vuoto, perche' l'insieme vuoto non e' l'hash di niente — e' l'hash di questa
/// stringa.
///
/// La versione sta dentro il dominio perche' la **regola** di calcolo puo'
/// cambiare: il giorno che l'insieme si inventaria davvero, quel digest non
/// deve poter coincidere con uno calcolato con la regola di oggi. Due regole
/// diverse danno due domini diversi, e due lati che ne usassero due si
/// rifiuterebbero — che e' l'esito giusto.
const DOMINIO_INSIEME: &str = "plenora:insieme-risorse:v1";

/// La descrizione di questa build, misurata adesso.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] se l'immagine non si legge o non e'
/// quella che dichiara di essere; [`PlenoraError::InvalidConfiguration`] se
/// questa build non sa descrivere il proprio ambiente.
#[cfg(target_os = "linux")]
pub fn di_questa_build(capability: Vec<String>) -> Result<DescrizioneLocale> {
    let scelto = Risolutore::di_questa_build();
    Ok(DescrizioneLocale {
        comune: Descrizione {
            artefatto: artefatto()?,
            resolver: resolver(scelto),
            ambiente: ambiente(scelto)?,
        },
        capability,
    })
}

/// L'identita' dell'immagine in esecuzione.
///
/// # I due controlli, e perche' entrambi
///
/// **File regolare**: una directory, un socket o un dispositivo non sono
/// un'immagine, e digerirli darebbe un valore che non descrive un programma.
///
/// **La dimensione non cambia sotto la lettura**: si prende la taglia dai
/// metadati del descrittore **aperto**, si legge, e si confronta col numero di
/// byte digeriti. Se divergono, qualcuno riscrive l'immagine mentre la si
/// legge, e il digest non descrive ne' il prima ne' il dopo: descrive una
/// cucitura dei due, che non e' nessun programma. Un digest cosi' e' peggio
/// di nessun digest, perche' l'handshake lo confronterebbe come se significasse
/// qualcosa.
///
/// # Errors
///
/// [`PlenoraError::IsolationUnavailable`] con la condizione che manca.
#[cfg(target_os = "linux")]
fn artefatto() -> Result<IdentitaArtefatto> {
    let mut immagine = std::fs::File::open(IMMAGINE)
        .map_err(|causa| non_leggibile(&format!("non si apre: {causa}")))?;
    let dati = immagine
        .metadata()
        .map_err(|causa| non_leggibile(&format!("non si interroga: {causa}")))?;
    if !dati.is_file() {
        return Err(non_leggibile(
            "non e' un file regolare: cio' che non e' un'immagine non si digerisce",
        ));
    }
    let dichiarata = dati.len();

    let mut digestore = Sha256::new();
    let mut blocco = vec![0_u8; BLOCCO];
    let mut letti: u64 = 0;
    loop {
        let quanti = immagine
            .read(&mut blocco)
            .map_err(|causa| non_leggibile(&format!("non si legge: {causa}")))?;
        if quanti == 0 {
            break;
        }
        digestore.update(&blocco[..quanti]);
        // La somma non puo' traboccare su un file reale, ma «non puo'» non e'
        // un controllo: `checked_add` rende l'impossibilita' osservata invece
        // che presunta.
        letti = letti
            .checked_add(quanti as u64)
            .ok_or_else(|| non_leggibile("la somma dei byte letti trabocca"))?;
    }
    if letti != dichiarata {
        return Err(non_leggibile(&format!(
            "dichiarava {dichiarata} byte e ne ha resi {letti}: l'immagine e' cambiata durante \
             la lettura, e il digest non descriverebbe nessun programma"
        )));
    }

    let digest = DigestSha256::da_esadecimale(&in_esadecimale(digestore))
        .map_err(|forma| non_leggibile(&format!("il digest non e' in forma canonica: {forma}")))?;
    Ok(IdentitaArtefatto {
        digest,
        versione: VERSIONE.to_owned(),
    })
}

/// L'identita' del resolver, dal selettore che sceglie anche la funzione.
fn resolver(scelto: Risolutore) -> IdentitaResolver {
    IdentitaResolver {
        identita: scelto.identita().to_owned(),
        versione: VERSIONE.to_owned(),
    }
}

/// L'ambiente di questa build, oppure il rifiuto di descriverlo.
///
/// # Perche' l'insieme vuoto e' una descrizione e non un'assenza
///
/// Perche' «non ci sono risorse» e' un fatto, e si puo' dichiarare: l'insieme e'
/// vuoto, il suo digest e' quello dell'insieme vuoto in questo dominio, e nulla
/// puo' entrarci a esecuzione in corso perche' non c'e' nessun backend che
/// scarichi qualcosa. Due lati che ne convengono convengono su qualcosa di
/// verificabile.
///
/// Con PROJ non e' cosi', e la differenza non e' di grado. Non c'e' una radice
/// esclusiva da inventariare: i percorsi si **aggiungono** a quelli esistenti, e
/// la cache delle griglie e' attiva per default. Un digest ricavato dal solo
/// searchpath sarebbe una falsa garanzia — due macchine con lo stesso
/// searchpath e contenuti diversi lo condividerebbero, e l'handshake direbbe
/// «stesso ambiente» su due ambienti diversi.
///
/// # Errors
///
/// [`PlenoraError::InvalidConfiguration`] quando l'ambiente non e'
/// inventariabile: e' una condizione della **build**, non dell'esecuzione, e
/// chi la legge deve poter cambiare build.
fn ambiente(scelto: Risolutore) -> Result<Ambiente> {
    if !scelto.ambiente_inventariabile() {
        return Err(PlenoraError::InvalidConfiguration(format!(
            "il profilo isolato non e' disponibile con il resolver «{}»: non esiste una radice \
             esclusiva, immutabile e inventariabile da cui ricavare l'insieme delle risorse, e un \
             digest inventato sarebbe una falsa garanzia. Le condizioni di rientro stanno in \
             errori-e-limiti.md",
            scelto.identita()
        )));
    }
    let mut digestore = Sha256::new();
    digestore.update(DOMINIO_INSIEME.as_bytes());
    // Nessuna risorsa: **niente** dopo il dominio. Non uno zero, non una
    // stringa vuota — l'insieme vuoto e' l'assenza di elementi, e aggiungere un
    // segnaposto lo renderebbe l'insieme di un elemento chiamato «niente».
    let digest = DigestSha256::da_esadecimale(&in_esadecimale(digestore)).map_err(|forma| {
        PlenoraError::InvalidConfiguration(format!(
            "il digest dell'insieme vuoto non e' in forma canonica: {forma}"
        ))
    })?;
    Ok(Ambiente {
        digest_insieme: digest,
        acquisizione_dinamica: false,
        risorse: Vec::new(),
        backend_dinamici: Vec::new(),
    })
}

/// Lo SHA-256 concluso, nella forma che [`DigestSha256`] accetta.
///
/// # Perche' passa da [`Esadecimale32`]
///
/// Perche' la grafia esadecimale ha gia' un'autorita', ed e' quel tipo: tiene i
/// 32 byte, la forma canonica e il parsing. Formattarli qui a mano darebbe due
/// grafie possibili al posto di una — e le due sarebbero uguali finche' qualcuno
/// non tocca la seconda.
fn in_esadecimale(digestore: Sha256) -> String {
    Esadecimale32::dai_byte(digestore.finalize().into()).in_esadecimale()
}

/// Il rifiuto sull'immagine, col percorso che lo riguarda.
#[cfg(target_os = "linux")]
fn non_leggibile(motivo: &str) -> PlenoraError {
    PlenoraError::IsolationUnavailable(format!("artefatto: {IMMAGINE}: {motivo}"))
}

#[cfg(test)]
mod tests;
