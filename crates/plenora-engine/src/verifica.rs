//! Verifica in streaming dell'artefatto prodotto: i passi da 3 a 8-bis della
//! sequenza di `isolamento.md`, in-process.
//!
//! # Che cosa verifica, e in quale ordine
//!
//! L'ordine e' vincolante, e ogni passo puo' solo fermare la sequenza:
//!
//! | # | passo | fallisce se |
//! |---|---|---|
//! | 3 | presenza | l'artefatto non esiste o non si apre |
//! | 4 | sigillo | magic o marcatore di coda mancanti: il file non e' finito |
//! | 5 | framing | il footer o i suoi blocchi non reggono il confine ostile |
//! | 5-bis | integrita' | lo SHA-256 dell'intero file non e' quello dichiarato |
//! | 6 | schema | il contratto non si ricostruisce dallo schema |
//! | 7 | contratto | il contratto letto non e' quello atteso |
//! | 8 | completezza | righe o batch osservati non sono quelli dichiarati |
//! | 8-bis | identita' | il `commit_token` non e' quello atteso |
//!
//! I passi 1 e 2 — stato terminale del worker ed esito dichiarato — non sono
//! qui: presuppongono un processo da osservare, e appartengono al supervisore.
//! Il passo 9, la pubblicazione, e' di chi pubblica.
//!
//! **I passi 4 e 5 sono due decisioni logiche su una sola traversata.** Il
//! confine ostile osserva magic, coda, footer e blocchi in un passaggio, e
//! separarli in due letture non aggiungerebbe una garanzia: aggiungerebbe una
//! finestra fra le due.
//!
//! # Un handle solo
//!
//! Framing, estrazione del token, digest e consegna ad arrow riferiscono **lo
//! stesso `File`, aperto una volta**. Riaprire per percorso fra un passo e
//! l'altro darebbe a ogni riapertura la possibilita' di trovare un file
//! diverso, e la verifica direbbe cose vere su file diversi.
//!
//! Resta la non-garanzia gia' dichiarata: tenere un handle aperto difende
//! dalla **sostituzione** del percorso, non dalla **mutazione in place** dei
//! byte, che un altro processo puo' fare attraverso il proprio descrittore.
//!
//! # La memoria trattenuta
//!
//! Non e' funzione del solo schema e del batch corrente, e dichiararlo
//! sarebbe falso. `FileReader` decodifica **tutti** i dizionari dentro
//! `try_new` e li trattiene per l'intera scansione, e tiene un indice di 24
//! byte per record batch. Il limite conservativo e':
//!
//! ```text
//!     schema limitato
//!   + custom metadata limitati        (tetti del confine: coppie, chiave, valore)
//!   + indice dei blocchi              (24 B x max_record_batches)
//!   + body dei dizionari trattenuti   (<= IpcLimits::max_retained_dictionary_body_bytes)
//!   + un solo record batch corrente   (<= max_body_bytes)
//!   + overhead strutturale limitato
//! ```
//!
//! Ogni componente ha un tetto **imposto prima della decodifica**; nessun
//! record batch precedente resta vivo; e il picco non puo' superare quella
//! formula. L'ultima riga e' l'unica non misurabile in byte di IPC: e'
//! l'overhead delle strutture di arrow, limitato da schema, metadati e numero
//! massimo di messaggi. Non si promette uguaglianza fra i `bodyLength` del
//! footer e l'heap esatto di arrow — sono due grandezze diverse, e prometterlo
//! sarebbe una precisione inventata.
//!
//! Il tetto sui dizionari e' l'unico cumulativo del confine: il tetto per
//! singolo body non li governa, perche' e' la loro **somma** a restare viva.
//! Vive in [`IpcLimits`] e non qui, cosi' lo applicano tutti i lettori dello
//! stesso formato e non il solo verificatore — un tetto che protegge un
//! percorso soltanto lascia aperta la classe del difetto.
//!
//! I dizionari **delta** sono rifiutati in prevalidazione, e la formula
//! dipende da quel rifiuto: su un delta arrow concatena il dizionario
//! precedente con il nuovo in un buffer ulteriore mentre entrambi gli
//! originali sono ancora vivi, quindi il picco si avvicina al **doppio** della
//! somma dei body e la riga qui sopra sarebbe falsa. Il nostro `FileWriter`
//! non ne produce.

use std::path::Path;

use plenora_core::contract::arrow_schema::{contract_from_arrow_schema, CrsResolver};
use plenora_core::contract::DataContract;
use plenora_core::error::{ErrorPhase, PlenoraError, Result};
use sha2::{Digest, Sha256};

use crate::commit_footer::interpreta_commit_token;
use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
use crate::esadecimale32::Esadecimale32;
use crate::geo_transport::ipc::IpcLimits;
use crate::ipc_boundary::{convalida_artefatto, ArtefattoConvalidato};
use crate::planner::contract_fingerprint;
use crate::protocollo::digest::ALGORITMO_DIGEST;
use crate::protocollo::messaggi::{ConteggiDichiarati, DigestArtefatto};

/// Byte letti per volta nel calcolo del digest.
///
/// Costante e piccola: e' cio' che rende il passo 5-bis a memoria costante
/// invece che proporzionale alla dimensione dell'artefatto.
const BLOCCO_DIGEST: usize = 64 * 1024;

/// Cio' che l'artefatto deve risultare, e che il verificatore **riceve**
/// invece di dedurre.
///
/// Nessuno di questi campi si ricava dall'artefatto: dedurli dal file che si
/// sta verificando significherebbe confrontarlo con se stesso.
#[derive(Debug, Clone, Copy)]
pub struct AtteseVerifica<'a> {
    /// Il contratto che il piano validato prevede.
    pub contratto: &'a DataContract,
    /// Il digest dichiarato dal produttore dell'artefatto.
    pub digest: &'a DigestArtefatto,
    /// Righe e batch dichiarati.
    pub conteggi: ConteggiDichiarati,
    /// Il token del tentativo.
    ///
    /// **Obbligatorio.** Un lettore generico del footer puo' non trovare alcun
    /// token e non e' un difetto; qui l'assenza fa fallire il passo 8-bis,
    /// perche' un artefatto senza token non e' attribuibile a questo
    /// tentativo.
    pub commit_token: &'a CommitToken,
}

/// Esegue i passi da 3 a 8-bis.
///
/// # Errors
///
/// - [`PlenoraError::Io`] se l'artefatto non esiste o non si legge (passo 3);
/// - [`PlenoraError::ResourceLimit`] se un tetto del confine o quello sui
///   dizionari e' superato (passi 4-5);
/// - [`PlenoraError::DataMapping`] per sigillo, framing, digest, schema,
///   contratto, conteggi e token (passi 4, 5, 5-bis, 6, 7, 8, 8-bis).
///
/// Nessun errore porta valori dell'artefatto: i messaggi nominano il passo e
/// le grandezze, mai il contenuto.
pub fn verifica_artefatto(
    percorso: &Path,
    attese: &AtteseVerifica<'_>,
    resolver: CrsResolver,
    limiti: &IpcLimits,
) -> Result<()> {
    // --- passi 3, 4 e 5: presenza, sigillo e framing, in una traversata ----
    //
    // Una chiamata sola apre, convalida ed estrae il token. Rileggerlo dopo, da
    // un'altra porta, sarebbe la `HashMap` di arrow — che comprime i duplicati
    // con «vince l'ultima» e non applica nessuno dei tetti — e riaprire per
    // percorso darebbe a ogni passo la possibilita' di trovare un file diverso.
    let (token_grezzo, mut artefatto) =
        convalida_artefatto(percorso, limiti, CHIAVE_FOOTER_COMMIT_TOKEN)?;

    // --- passo 5-bis: integrita' ------------------------------------------
    verifica_digest(&mut artefatto, attese.digest)?;

    // --- passo 6: schema ---------------------------------------------------
    let (schema, batch) = artefatto.in_batches()?;
    let contratto = contract_from_arrow_schema(schema, resolver)?;

    // --- passo 7: contratto ------------------------------------------------
    //
    // Il confronto passa dal **fingerprint**, che e' l'autorita' gia' in uso:
    // e' con quello che il planner verifica un contratto contro quello atteso
    // dal grafo validato. Confrontare i campi a mano avrebbe introdotto una
    // seconda nozione di uguaglianza fra contratti, libera di divergere dalla
    // prima appena uno dei due elenchi cambia.
    if contract_fingerprint(&contratto)? != contract_fingerprint(attese.contratto)? {
        return Err(PlenoraError::DataMapping(
            "verifica dell'artefatto: il contratto letto non e' quello atteso dal piano".to_owned(),
        )
        .with_phase(ErrorPhase::Read));
    }

    // --- passo 8: completezza ---------------------------------------------
    let osservati = conta_in_streaming(batch)?;
    if osservati != attese.conteggi {
        return Err(PlenoraError::DataMapping(format!(
            "verifica dell'artefatto: conteggi osservati (righe {}, batch {}) diversi da quelli \
             dichiarati (righe {}, batch {})",
            osservati.righe, osservati.batch, attese.conteggi.righe, attese.conteggi.batch
        ))
        .with_phase(ErrorPhase::Read));
    }

    // --- passo 8-bis: identita' del tentativo -----------------------------
    //
    // Dopo il passo 8 e non prima: l'ordine della sequenza e' vincolante, e un
    // artefatto incompleto va respinto come incompleto anche se il token
    // combacia.
    verifica_token(token_grezzo.as_deref(), attese.commit_token)
}

/// Passo 5-bis: SHA-256 dell'intero file finalizzato, footer compreso.
///
/// Legge a blocchi di dimensione costante: memoria costante, **una passata
/// sequenziale in piu'** sull'artefatto. Il costo in I/O e' dichiarato e non
/// si evita: il digest copre i byte, e i byte vanno letti.
fn verifica_digest(
    artefatto: &mut ArtefattoConvalidato,
    dichiarato: &DigestArtefatto,
) -> Result<()> {
    if dichiarato.algoritmo != ALGORITMO_DIGEST {
        // Il nome dichiarato **non si ripete nell'errore**: arriva dall'`Esito`
        // del worker, cioe' da fuori, ed e' testo che chi lo scrive controlla.
        // Rimandarlo in un messaggio metterebbe in un log una stringa
        // arbitraria di un altro processo. Si dice quale algoritmo e' ammesso,
        // che e' l'unica informazione che serve a chi legge.
        return Err(PlenoraError::DataMapping(format!(
            "verifica dell'artefatto: algoritmo di digest non ammesso (atteso `{ALGORITMO_DIGEST}`)"
        ))
        .with_phase(ErrorPhase::Read));
    }
    // Il valore dichiarato si interpreta con lo stesso tipo che impone la
    // forma canonica al resto del protocollo: 64 esadecimali minuscoli. Un
    // confronto fra testi avrebbe accettato `AB..` accanto ad `ab..` come due
    // digest diversi dello stesso valore.
    let atteso = Esadecimale32::da_esadecimale(&dichiarato.valore).map_err(|_| {
        PlenoraError::DataMapping(
            "verifica dell'artefatto: digest dichiarato non canonico (atteso esadecimale \
             minuscolo di 64 caratteri)"
                .to_owned(),
        )
        .with_phase(ErrorPhase::Read)
    })?;

    let byte_totali = artefatto.byte_totali();
    let mut hasher = Sha256::new();
    let mut buffer = Vec::with_capacity(BLOCCO_DIGEST);
    let mut letti = 0_u64;
    while letti < byte_totali {
        let restanti = byte_totali.saturating_sub(letti);
        let blocco = usize::try_from(restanti.min(BLOCCO_DIGEST as u64)).unwrap_or(BLOCCO_DIGEST);
        artefatto.leggi_a(letti, blocco, &mut buffer)?;
        hasher.update(&buffer);
        letti = letti.checked_add(blocco as u64).ok_or_else(|| {
            PlenoraError::Internal(
                "verifica dell'artefatto: offset del digest fuori intervallo".to_owned(),
            )
        })?;
    }
    let calcolato = Esadecimale32::dai_byte(hasher.finalize().into());

    if calcolato != atteso {
        // Nessuno dei due valori entra nel messaggio. Il digest e' derivato
        // dai byte dell'artefatto, quindi mostrarlo sarebbe far uscire una
        // funzione del contenuto da un errore.
        return Err(PlenoraError::DataMapping(
            "verifica dell'artefatto: digest calcolato diverso da quello dichiarato".to_owned(),
        )
        .with_phase(ErrorPhase::Read));
    }
    Ok(())
}

/// Passo 8: righe e batch, contati mentre scorrono.
///
/// Nessun batch precedente resta vivo: ogni `RecordBatch` e' rilasciato prima
/// che il successivo sia letto, e la funzione non tiene alcuna collezione.
///
/// L'aritmetica e' **controllata**. Un `wrapping` farebbe combaciare i
/// conteggi di un artefatto che ne ha 2^64 di troppo, e un `saturating`
/// direbbe `u64::MAX` per due artefatti diversi.
pub fn conta_in_streaming(
    batch: impl Iterator<Item = Result<plenora_core::arrow::array::RecordBatch>>,
) -> Result<ConteggiDichiarati> {
    let mut righe = 0_u64;
    let mut conteggio = 0_u64;
    for prossimo in batch {
        let prossimo = prossimo?;
        let di_questo = u64::try_from(prossimo.num_rows()).map_err(|_| {
            PlenoraError::DataMapping(
                "verifica dell'artefatto: numero di righe di un batch fuori intervallo".to_owned(),
            )
            .with_phase(ErrorPhase::Read)
        })?;
        righe = righe.checked_add(di_questo).ok_or_else(|| {
            PlenoraError::DataMapping(
                "verifica dell'artefatto: somma delle righe fuori intervallo".to_owned(),
            )
            .with_phase(ErrorPhase::Read)
        })?;
        conteggio = conteggio.checked_add(1).ok_or_else(|| {
            PlenoraError::DataMapping(
                "verifica dell'artefatto: numero di batch fuori intervallo".to_owned(),
            )
            .with_phase(ErrorPhase::Read)
        })?;
        // Esplicito, anche se il `for` lo farebbe comunque: e' la riga che
        // dice al lettore che qui non si accumula.
        drop(prossimo);
    }
    Ok(ConteggiDichiarati {
        righe,
        batch: conteggio,
    })
}

/// Passo 8-bis: il token del footer e' quello del tentativo.
///
/// Tre esiti, e due sono un rifiuto: assente, non canonico, diverso.
fn verifica_token(trovato: Option<&str>, atteso: &CommitToken) -> Result<()> {
    let Some(testo) = trovato else {
        return Err(PlenoraError::DataMapping(
            "verifica dell'artefatto: il footer non porta un commit token, quindi l'artefatto \
             non e' attribuibile a questo tentativo"
                .to_owned(),
        )
        .with_phase(ErrorPhase::Read));
    };
    // L'interpretazione e' condivisa con il lettore del footer: la regola «non
    // canonico si rifiuta» ha un posto solo. Il valore non entra mai
    // nell'errore, ed e' il tipo del messaggio a impedirlo.
    let letto = interpreta_commit_token(testo).map_err(crate::ipc_boundary::read_error)?;
    if &letto != atteso {
        return Err(PlenoraError::DataMapping(
            "verifica dell'artefatto: il commit token del footer non e' quello di questo tentativo"
                .to_owned(),
        )
        .with_phase(ErrorPhase::Read));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
