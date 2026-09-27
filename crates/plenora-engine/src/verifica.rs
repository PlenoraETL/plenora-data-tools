//! Verifica in streaming dell'artefatto prodotto: i passi da 3 a 8-bis della
//! sequenza di `isolamento.md`, in-process.
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
//! I passi 1-2 appartengono al supervisore, il 9 a chi pubblica. I passi 4 e 5
//! sono due decisioni su una sola traversata. Framing, token, digest e
//! consegna ad arrow usano **lo stesso `File`, aperto una volta**: difende
//! dalla sostituzione del percorso, non dalla mutazione in place dei byte.
//!
//! Memoria trattenuta: schema, custom metadata e indice dei blocchi limitati,
//! piu' i body dei dizionari (`FileReader` li trattiene tutti; tetto cumulativo
//! `IpcLimits::max_retained_dictionary_body_bytes`), piu' un solo record
//! batch (`max_body_bytes`), piu' overhead strutturale limitato. Ogni tetto si
//! impone prima della decodifica. La formula dipende dal rifiuto dei
//! dizionari **delta** in prevalidazione: con un delta arrow concatena e il
//! picco si avvicina al doppio.

use std::path::Path;

use plenora_core::contract::arrow_schema::{contract_from_arrow_schema, CrsResolver};
use plenora_core::error::{ErrorPhase, PlenoraError, Result};
use sha2::{Digest, Sha256};

use crate::commit_footer::interpreta_commit_token;
use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
use crate::esadecimale32::Esadecimale32;
use crate::geo_transport::ipc::IpcLimits;
use crate::ipc_boundary::{convalida_artefatto, convalida_handle_artefatto, ArtefattoConvalidato};
use crate::planner::contract_fingerprint;
use crate::protocollo::digest::{DigestSha256, ALGORITMO_DIGEST};
use crate::protocollo::messaggi::{ConteggiDichiarati, DigestArtefatto};
use crate::pubblicazione::ArtefattoVerificato;

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
    /// Il **fingerprint** del contratto che il piano validato prevede — non
    /// il contratto stesso.
    ///
    /// Il passo 7 confronta solo le impronte, e il verificatore isolato
    /// (`isolamento.md#2-quater-topologia-chi-osserva-chi`) riceve solo
    /// l'impronta. E' un `DigestSha256`, la forma sul filo, cosi' passa qui
    /// senza conversione; da un `ContractFingerprint` si ottiene con
    /// `DigestSha256::da_esadecimale(&impronta.to_hex())`.
    pub contratto_fingerprint_atteso: DigestSha256,
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
/// Le categorie sono quelle della matrice di `isolamento.md#10-matrice-degli-esiti`:
///
/// - [`PlenoraError::Io`] se l'artefatto non esiste o non si legge (passo 3);
/// - [`PlenoraError::Internal`] per sigillo e framing (passi 4-5), anche
///   quando e' un tetto del confine a fermarli (riga 12, vedi
///   [`artefatto_troncato`]);
/// - [`PlenoraError::Schema`] se il contratto letto non e' quello atteso
///   (passo 7, riga 13);
/// - [`PlenoraError::DataMapping`] per digest, schema, conteggi e token
///   (passi 5-bis, 6, 8, 8-bis: riga 14, «secondo il passo»).
///
/// Nessun errore porta valori dell'artefatto: i messaggi nominano il passo e
/// le grandezze, mai il contenuto.
pub fn verifica_artefatto(
    percorso: &Path,
    attese: &AtteseVerifica<'_>,
    resolver: CrsResolver,
    limiti: &IpcLimits,
) -> Result<ArtefattoVerificato> {
    // --- passi 3, 4 e 5: presenza, sigillo e framing, in una traversata ----
    //
    // Una chiamata sola apre, convalida ed estrae il token. Rileggerlo dopo, da
    // un'altra porta, sarebbe la `HashMap` di arrow — che comprime i duplicati
    // con «vince l'ultima» e non applica nessuno dei tetti — e riaprire per
    // percorso darebbe a ogni passo la possibilita' di trovare un file diverso.
    let aperto = convalida_artefatto(percorso, limiti, CHIAVE_FOOTER_COMMIT_TOKEN)
        .map_err(artefatto_troncato)?;
    verifica_artefatto_aperto(aperto, attese, resolver)
}

/// Un errore dei passi 4-5 — sigillo, framing, tetti del confine — nella
/// categoria della riga 12 della matrice: `Internal`.
///
/// L'artefatto l'ha scritto il nostro worker: un sigillo rotto dice che
/// qualcosa nel sistema non va, non che l'ingresso sia sbagliato
/// (`DataMapping`), e un tetto superato e' un'incoerenza fra i due lati, non
/// un budget (`ResourceLimit` e' dell'evidenza del dominio, `F4-2`). L'I/O
/// resta I/O, ritentabile.
fn artefatto_troncato(errore: PlenoraError) -> PlenoraError {
    match errore.category() {
        plenora_core::ErrorCategory::Io => errore,
        _ => PlenoraError::Internal(format!("{PREFISSO_PASSI_4_5}: {errore}"))
            .with_phase(ErrorPhase::Read),
    }
}

/// L'inizio del messaggio di ogni errore di [`artefatto_troncato`].
///
/// Serve a chi deve distinguere questo `Internal` — un rifiuto ordinario di
/// byte che non sono un artefatto — da un `Internal` che dice un difetto: e'
/// testo nostro e costante, mai dell'ingresso.
pub const PREFISSO_PASSI_4_5: &str = "verifica dell'artefatto, sigillo o framing (passi 4-5)";

/// Come [`verifica_artefatto`], ma da un `File` **gia' aperto** invece che da
/// un percorso.
///
/// Il verificatore isolato riceve l'artefatto come descrittore aperto in
/// sola lettura, mai come percorso (`GA-5`): non deve poter scoprire la
/// destinazione finale.
///
/// # Errors
///
/// Come [`verifica_artefatto`], meno gli errori di apertura per percorso.
pub fn verifica_artefatto_handle(
    handle: std::fs::File,
    attese: &AtteseVerifica<'_>,
    resolver: CrsResolver,
    limiti: &IpcLimits,
) -> Result<ArtefattoVerificato> {
    let aperto = convalida_handle_artefatto(handle, limiti, CHIAVE_FOOTER_COMMIT_TOKEN)
        .map_err(artefatto_troncato)?;
    verifica_artefatto_aperto(aperto, attese, resolver)
}

/// Il corpo condiviso fra [`verifica_artefatto`] e [`verifica_artefatto_handle`]:
/// tutto cio' che viene **dopo** l'apertura, che e' l'unica cosa che le
/// distingue.
///
/// # Errors
///
/// Come [`verifica_artefatto`].
fn verifica_artefatto_aperto(
    aperto: (Option<String>, ArtefattoConvalidato),
    attese: &AtteseVerifica<'_>,
    resolver: CrsResolver,
) -> Result<ArtefattoVerificato> {
    let (token_grezzo, mut artefatto) = aperto;

    // Il duplicato si prende **adesso**, prima che il passo 6 consumi
    // l'artefatto: e' lo stesso descrittore, non una seconda apertura, quindi
    // fra la verifica e la pubblicazione non c'e' istante in cui il percorso
    // possa risolvere a un file diverso.
    let per_la_pubblicazione = artefatto.duplica()?;
    let byte_verificati = artefatto.byte_totali();

    // --- passo 5-bis: integrita' ------------------------------------------
    let digest_verificato = verifica_digest(&mut artefatto, attese.digest)?;

    // --- passo 6: schema ---------------------------------------------------
    let (schema, batch) = artefatto.in_batches()?;
    let contratto = contract_from_arrow_schema(schema, resolver)?;

    // --- passo 7: contratto ------------------------------------------------
    //
    // Il confronto passa dal fingerprint, la stessa autorita' del planner:
    // una seconda nozione di uguaglianza fra contratti potrebbe divergere.
    if contract_fingerprint(&contratto)?.to_hex()
        != attese.contratto_fingerprint_atteso.in_esadecimale()
    {
        // `Schema`, riga 13 della matrice: l'artefatto e' integro ma dice una
        // forma diversa da quella che il piano ha promesso.
        return Err(PlenoraError::Schema(
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
    verifica_token(token_grezzo.as_deref(), attese.commit_token)?;

    // La prova esiste solo qui: chi la riceve ha attraversato tutti i passi.
    Ok(ArtefattoVerificato::accertato(
        per_la_pubblicazione,
        byte_verificati,
        digest_verificato,
    ))
}

/// Passo 5-bis: SHA-256 dell'intero file finalizzato, footer compreso.
///
/// Legge a blocchi di dimensione costante: memoria costante, **una passata
/// sequenziale in piu'** sull'artefatto. Il costo in I/O e' dichiarato e non
/// si evita: il digest copre i byte, e i byte vanno letti.
fn verifica_digest(
    artefatto: &mut ArtefattoConvalidato,
    dichiarato: &DigestArtefatto,
) -> Result<Esadecimale32> {
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
    // Si rende il valore **accertato**, non il testo dichiarato: chi pubblica
    // lo riusa per confrontare i byte copiati, e reinterpretare il testo una
    // seconda volta darebbe due letture dello stesso campo.
    Ok(calcolato)
}

/// Passo 8: righe e batch, contati mentre scorrono.
///
/// Nessun batch precedente resta vivo. L'aritmetica e' controllata: un
/// `wrapping` farebbe combaciare conteggi sbagliati di 2^64, un `saturating`
/// darebbe `u64::MAX` per due artefatti diversi.
///
/// # Errors
///
/// Propaga l'errore che l'iteratore rende leggendo un batch; e
/// [`PlenoraError::ResourceLimit`] se righe o batch non stanno in un `u64`.
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
