//! Che cosa si vede su una destinazione, dopo che qualcuno ha provato a
//! pubblicarci; e il passo 9, la pubblicazione atomica no-clobber
//! ([`pubblica`]).
//!
//! Separato da [`crate::verifica`] perche' la verifica non tocca niente,
//! mentre il passo 9 rende visibile un output ed e' irreversibile.
//!
//! - Il passo 9 non accetta un percorso: consuma per valore
//!   [`ArtefattoVerificato`], opaco e prodotto solo dal verificatore.
//! - Il residuo del temporaneo dice che cosa e dove, e sa dire «non l'ho
//!   potuto accertare» (`geo_transport::publish::PuliziaDelTemporaneo`):
//!   confondere «vuoto» con «non guardato» e' fail-open.
//! - Durabilita' e residuo restano due assi di
//!   `geo_transport::publish::EsitoDellaPubblicazione`.

use std::io::{ErrorKind, Write};
use std::path::Path;

use plenora_core::error::{ErrorCategory, ErrorPhase, PlenoraError, Result};
use sha2::{Digest as _, Sha256};

use crate::commit_footer::interpreta_commit_token;
use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
use crate::esadecimale32::Esadecimale32;
use crate::geo_transport::error::ArrowTransportError;
use crate::geo_transport::publish::{
    publish_with_profile, EsitoDellaPubblicazione, PublishProfile,
};
use crate::ipc_boundary::{
    convalida_artefatto_con_causa, ArtefattoConvalidato, CausaDiApertura, IpcLimits,
};

/// Che cosa si vede guardando una destinazione, dato il token di un tentativo.
///
/// Osservazioni e non decisioni: chi chiama sa se il percorso e' suo e se
/// ritentare abbia senso. Non e' un `Result` perche' ognuna e'
/// un'osservazione riuscita, anche [`Self::InvalidOrUnreadable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsservazioneDelCommit {
    /// La destinazione esiste, porta **questo** token, e sigillo e struttura
    /// sono validi.
    ///
    /// La verifica non e' un di piu': una chiave uguale su un file troncato
    /// direbbe «riuscito» di un output che non lo e'.
    CommittedMatching,
    /// Esiste e porta un token, ma di un altro tentativo.
    OccupiedByOtherAttempt,
    /// Esiste e non porta alcun token.
    ///
    /// Quale delle due — questa o [`Self::OccupiedByOtherAttempt`] — lo dice il
    /// token trovato, non la storia del file: con `(token, destinazione)` non
    /// c'e' modo di sapere che cosa ci fosse prima.
    IdentityMissing,
    /// La destinazione non esiste.
    ///
    /// **Non significa «riprova pure».** Un file puo' mancare perche' il commit
    /// non e' avvenuto, ma anche perche' la durabilita' e' andata persa, perche'
    /// qualcuno l'ha rimosso, o perche' il filesystem e' tornato indietro.
    /// Riprovare puo' essere giusto, e la decisione e' di chi conosce quel
    /// percorso.
    Absent,
    /// Esiste, e non si puo' concludere: la ragione dice **perche'**.
    InvalidOrUnreadable(RagioneNonLeggibile),
}

/// Perche' una destinazione esistente non si e' potuta giudicare.
///
/// Strutturata, perche' le decisioni che ne seguono sono diverse e un testo
/// cambia con piattaforma e lingua. Ogni guasto che il codice sa distinguere
/// ha una voce sua; gli altri stanno in [`Self::GuastoDiLettura`], invece di
/// ricevere una diagnosi precisa e falsa.
///
/// Non porta byte dei dati ne' frammenti di percorso: dice di che genere e'
/// il guasto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RagioneNonLeggibile {
    /// Il file c'e' e non si apre: permessi.
    PermessoNegato,
    /// Il file c'e' e la lettura non riesce, per un guasto che non e' un
    /// permesso.
    ///
    /// Non e' una voce di scarto: e' la dichiarazione che il genere esatto non
    /// e' distinguibile qui, ed e' meglio dirlo che scegliere una voce piu'
    /// precisa e falsa.
    GuastoDiLettura,
    /// Un tetto del confine ostile e' stato superato leggendo la destinazione.
    ///
    /// Distinto da [`Self::FooterRifiutato`]: un tetto superato dice che il file
    /// e' **piu' grande** di quanto si ammetta, non che sia malformato.
    TettoSuperato,
    /// Non e' un contenitore Arrow IPC leggibile dal confine ostile.
    FramingNonValido,
    /// Il sigillo di fine file manca: l'artefatto e' troncato.
    SigilloAssente,
    /// Il sigillo c'e' e non corrisponde.
    SigilloNonCorrispondente,
    /// Il confine ostile ha rifiutato il footer: forma, o duplicati.
    FooterRifiutato,
    /// Il footer si legge, e i metadati che dovrebbero portare il token no.
    ///
    /// Distinto da [`Self::FooterRifiutato`] e da
    /// [`OsservazioneDelCommit::IdentityMissing`]: qui non si e' potuto
    /// **guardare** se un token ci sia, mentre `IdentityMissing` dice di aver
    /// guardato e non averlo trovato.
    MetadatiNonLeggibili,
}

/// Che cosa c'e' sulla destinazione, dato il token di un tentativo.
///
/// Non e' un metodo dell'oggetto d'esecuzione, che e' proprio cio' che puo'
/// mancare. Non rende un `Result`: ogni esito e' un'osservazione riuscita, e
/// [`RagioneNonLeggibile`] distingue gia' cio' che non si conclude. Applica i
/// tetti di default del confine: la destinazione e' un artefatto nostro, e chi
/// ha perso il processo non deve ricostruire il budget della scrittura.
#[must_use]
pub fn risolvi_commit(commit_token: &CommitToken, destinazione: &Path) -> OsservazioneDelCommit {
    // Una **sola** apertura, e da qui in poi si legge solo da quell'handle:
    // riaprire per rispondere alla seconda domanda darebbe due risposte su due
    // file che possono non essere lo stesso.
    let (trovato, artefatto) = match convalida_artefatto_con_causa(
        destinazione,
        &IpcLimits::default(),
        CHIAVE_FOOTER_COMMIT_TOKEN,
    ) {
        Ok(coppia) => coppia,
        Err(CausaDiApertura::Io(errore)) => {
            return match errore.kind() {
                // L'assenza non e' una ragione: e' una delle cinque
                // osservazioni, e dice qualcosa di diverso dal non poter
                // leggere.
                ErrorKind::NotFound => OsservazioneDelCommit::Absent,
                ErrorKind::PermissionDenied => non_leggibile(RagioneNonLeggibile::PermessoNegato),
                _ => non_leggibile(RagioneNonLeggibile::GuastoDiLettura),
            };
        }
        Err(CausaDiApertura::Confine(causa)) => return non_leggibile(ragione_di(&causa)),
    };

    // Sigillo e struttura reggono: da qui decide il token.
    let Some(testo) = trovato else {
        return OsservazioneDelCommit::IdentityMissing;
    };
    match interpreta_commit_token(&testo) {
        // Il token e' il nostro: solo adesso vale la pena leggere il resto.
        Ok(letto) if &letto == commit_token => match percorri_i_corpi(artefatto) {
            Ok(()) => OsservazioneDelCommit::CommittedMatching,
            Err(ragione) => non_leggibile(ragione),
        },
        Ok(_) => OsservazioneDelCommit::OccupiedByOtherAttempt,
        // Un valore che c'e' e non si interpreta non e' «nessun token»: si e'
        // trovato qualcosa e non lo si e' potuto leggere, che e' un'altra cosa.
        Err(_) => non_leggibile(RagioneNonLeggibile::MetadatiNonLeggibili),
    }
}

/// Legge l'artefatto fino in fondo, e butta via cio' che legge.
///
/// Il footer dice dove stanno i blocchi, non che cosa contengano, e il
/// formato Arrow file non ha un checksum complessivo: per sapere se i corpi si
/// leggono bisogna leggerli. Si fa solo quando il token e' il nostro.
///
/// **Non dimostra** che i byte siano quelli verificati: il digest vive
/// nell'`Esito`, che qui manca. `CommittedMatching` dice «di questo tentativo,
/// e leggibile per intero».
fn percorri_i_corpi(
    artefatto: ArtefattoConvalidato,
) -> std::result::Result<(), RagioneNonLeggibile> {
    let (_schema, batch) = artefatto
        .in_batches()
        .map_err(|errore| ragione_dal_confine(&errore))?;
    for prossimo in batch {
        // Il batch si lascia cadere subito: qui interessa che si legga, non che
        // cosa contenga, e tenerlo vivo costerebbe memoria per niente.
        drop(prossimo.map_err(|errore| ragione_dal_confine(&errore))?);
    }
    Ok(())
}

/// Da un errore del confine gia' tradotto, alla ragione che chi legge riceve.
///
/// La consegna ad arrow rende [`PlenoraError`], non piu' la variante di
/// trasporto: le distinzioni fini del sigillo qui non ci sono piu', e si dice
/// quello che resta invece di inventare una precisione perduta.
const fn ragione_dal_confine(errore: &PlenoraError) -> RagioneNonLeggibile {
    match errore.category() {
        ErrorCategory::ResourceLimit => RagioneNonLeggibile::TettoSuperato,
        // Framing, schema e mappatura: il contenitore non si lascia leggere.
        ErrorCategory::DataMapping | ErrorCategory::Schema => RagioneNonLeggibile::FramingNonValido,
        // `Io` cade qui insieme al resto, e non e' una svista: e' esattamente
        // cio' che `GuastoDiLettura` dichiara — un guasto che questa porta non
        // sa distinguere meglio. Dargli un ramo suo direbbe che lo distingue.
        _ => RagioneNonLeggibile::GuastoDiLettura,
    }
}

/// L'osservazione che porta una ragione, scritta in un posto solo.
const fn non_leggibile(ragione: RagioneNonLeggibile) -> OsservazioneDelCommit {
    OsservazioneDelCommit::InvalidOrUnreadable(ragione)
}

/// Da che cosa il confine ha rifiutato, a **perche'** chi legge non conclude.
///
/// Si elencano tutte le voci del confine di `ArrowTransportError`, perche' li'
/// una variante nuova deve costringere a scegliere; il ramo finale raccoglie
/// le voci dei kernel, che aprire e validare un file non puo' produrre.
fn ragione_di(causa: &ArrowTransportError) -> RagioneNonLeggibile {
    use ArrowTransportError as E;
    match causa {
        // Il file c'e' e la lettura non riesce: il genere lo dice `io::Error`.
        E::Io(errore) => match errore.kind() {
            ErrorKind::PermissionDenied => RagioneNonLeggibile::PermessoNegato,
            _ => RagioneNonLeggibile::GuastoDiLettura,
        },

        // Il sigillo dell'envelope: assente o mutilato da una parte, non
        // corrispondente dall'altra. Sono due diagnosi diverse — un file
        // troncato e un file alterato — e restano separate.
        E::InvalidMagic | E::InvalidTrailer => RagioneNonLeggibile::SigilloAssente,
        E::ChecksumMismatch => RagioneNonLeggibile::SigilloNonCorrispondente,

        // I tetti. Dicono che il file e' **piu' grande** di quanto si ammetta,
        // non che sia malformato: chiamarli framing manderebbe chi legge a
        // cercare una corruzione che non c'e'.
        E::StreamTooLarge
        | E::TooManyRows(_)
        | E::TooManyColumns(_)
        | E::TooManyBatches(_)
        | E::CellTooLarge(_)
        | E::CrsTooLarge
        | E::IpcMetadataTooLarge(_, _)
        | E::IpcBodyTooLarge { .. }
        | E::IpcRetainedDictionariesTooLarge { .. }
        | E::IpcTooManyMessages(_, _)
        | E::IpcSchemaTooComplex(_)
        | E::IpcTooManyRecordBatches(_, _)
        | E::IpcTooManyMetadataPairs(_, _)
        | E::IpcMetadataKeyTooLarge(_, _)
        | E::IpcMetadataValueTooLarge(_, _) => RagioneNonLeggibile::TettoSuperato,

        // Il footer del file format: blocchi fuori regione, sovrapposti, non
        // allineati.
        E::IpcFooterInvalid(_) => RagioneNonLeggibile::FooterRifiutato,

        // I custom metadata: ci sono, e non si lasciano leggere.
        E::IpcMetadataInvalid(_) => RagioneNonLeggibile::MetadatiNonLeggibili,

        // La struttura del contenitore: troncato, disallineato, con byte dopo
        // la fine, o con un costrutto che il confine non sa limitare.
        E::TrailingBytes
        | E::RowCountMismatch { .. }
        | E::PayloadLengthMismatch { .. }
        | E::UnsupportedSchemaVersion(_)
        | E::MissingGeometryColumn(_)
        | E::MissingGeoArrowMetadata(_)
        | E::GeometryColumnNotBinary { .. }
        | E::CrsRequired
        | E::IpcTruncated
        | E::IpcTrailingAfterEos
        | E::IpcUnsupportedFeature(_)
        | E::IpcSchemaInvalid(_) => RagioneNonLeggibile::FramingNonValido,

        // Tutto cio' che nasce **fuori** dal confine: parametri di
        // un'operazione, backend assenti, guasti dei kernel. Aprire e validare
        // un file non ne esegue nessuna, quindi qui non si raggiungono; se ci
        // arrivassero, il genere non sarebbe distinguibile da questa porta, ed
        // e' esattamente cio' che `GuastoDiLettura` dichiara.
        _ => RagioneNonLeggibile::GuastoDiLettura,
    }
}

/// L'artefatto che ha superato i passi da 3 a 8-bis.
///
/// Campi privati e nessun costruttore aperto: averne uno significa che il
/// verificatore l'ha prodotto. Conserva l'handle che ha letto il sigillo,
/// perche' riaprire per percorso aprirebbe una finestra verso un file diverso.
/// Non e' clonabile (il passo 9 lo consuma) e non deriva `Debug`, che
/// porterebbe il digest nei log.
pub(crate) struct ArtefattoVerificato {
    /// L'handle gia' aperto e convalidato: la sorgente dei byte da pubblicare.
    artefatto: ArtefattoConvalidato,
    /// Quanti byte la verifica ha misurato.
    byte_verificati: u64,
    /// Il digest che quei byte devono rendere.
    digest_atteso: Esadecimale32,
}

impl ArtefattoVerificato {
    /// Lo costruisce il verificatore, e nessun altro.
    ///
    /// `pub(crate)` e non `pub`: fuori dal crate la sola via per averne uno e'
    /// far girare la sequenza.
    pub(crate) const fn accertato(
        artefatto: ArtefattoConvalidato,
        byte_verificati: u64,
        digest_atteso: Esadecimale32,
    ) -> Self {
        Self {
            artefatto,
            byte_verificati,
            digest_atteso,
        }
    }
}

/// Passo 9: rende visibile l'artefatto verificato, **senza mai sostituire**.
///
/// Copia attraverso il commit atomico no-clobber gia' qualificato nel crate
/// invece di spostare il file, che richiederebbe una seconda implementazione
/// per piattaforma. Costo dichiarato: una lettura e una scrittura integrali in
/// piu'. Condizione di rientro: una primitiva cross-platform qualificata che
/// committi un file esistente conservando no-clobber e l'osservabilita' della
/// pulizia.
///
/// Durante la copia si pretendono il numero esatto di byte verificati e lo
/// SHA-256 ricalcolato sui byte copiati; qualunque divergenza ferma la closure
/// e la destinazione non appare.
///
/// # Errors
///
/// - [`PlenoraError::InvalidPlan`] se la destinazione esiste gia': e' il
///   no-clobber, e la prima esecuzione che pubblica vince;
/// - [`PlenoraError::Io`] per i guasti della copia e del commit;
/// - [`PlenoraError::DataMapping`] se l'artefatto non ha piu' i byte che la
///   prova gli ha misurato, o se i byte copiati non rendono il digest
///   verificato: in entrambi i casi il file e' cambiato dopo la verifica.
pub(crate) fn pubblica(
    verificato: ArtefattoVerificato,
    destinazione: &Path,
    profilo: PublishProfile,
) -> Result<EsitoDellaPubblicazione> {
    let ArtefattoVerificato {
        mut artefatto,
        byte_verificati,
        digest_atteso,
    } = verificato;

    let ((), esito) = publish_with_profile(destinazione, profilo, |uscita| {
        copia_accertando(&mut artefatto, byte_verificati, &digest_atteso, uscita)
    })?;
    Ok(esito)
}

/// Copia i byte verificati nel writer, contandoli e ricalcolandone il digest.
///
/// # Perche' i due controlli stanno qui e non dopo
///
/// Perche' dopo non servirebbero a niente: il commit sarebbe gia' avvenuto e la
/// destinazione gia' visibile. Fermarsi dentro la closure e' l'unico momento in
/// cui una divergenza puo' ancora impedire che l'output appaia.
fn copia_accertando(
    artefatto: &mut ArtefattoConvalidato,
    byte_verificati: u64,
    digest_atteso: &Esadecimale32,
    uscita: &mut dyn Write,
) -> Result<()> {
    // Il conteggio si chiede al descrittore adesso: confrontare con
    // `artefatto.byte_totali()` sarebbe tautologico, mentre il descrittore
    // scopre la mutazione in place dopo la verifica. Dopo il ciclo non serve
    // un confronto: `leggi_a` fallisce invece di consegnare corto.
    let misurati = artefatto.misura_ora()?;
    if misurati != byte_verificati {
        // Non e' un difetto interno: e' il file sotto di noi che e' cambiato.
        // `DataMapping` e' la stessa classe del passo 5-bis, che giudica lo
        // stesso genere di fatto sullo stesso artefatto.
        return Err(PlenoraError::DataMapping(format!(
            "pubblicazione: l'artefatto verificato aveva {byte_verificati} byte e ora ne ha \
             {misurati}: e' cambiato dopo la verifica"
        ))
        .with_phase(ErrorPhase::Write));
    }

    let mut hasher = Sha256::new();
    let mut buffer = Vec::with_capacity(BLOCCO_COPIA);
    let mut copiati = 0_u64;
    while copiati < byte_verificati {
        let restanti = byte_verificati.saturating_sub(copiati);
        let blocco = usize::try_from(restanti.min(BLOCCO_COPIA as u64)).unwrap_or(BLOCCO_COPIA);
        artefatto.leggi_a(copiati, blocco, &mut buffer)?;
        hasher.update(&buffer);
        uscita
            .write_all(&buffer)
            .map_err(|errore| PlenoraError::Io(errore).with_phase(ErrorPhase::Write))?;
        copiati = copiati.checked_add(blocco as u64).ok_or_else(|| {
            PlenoraError::Internal(
                "pubblicazione: il conteggio dei byte copiati e' andato oltre un u64".to_owned(),
            )
        })?;
    }

    // Il digest: sui byte usciti, non su quelli letti prima. Il valore non entra
    // nel messaggio — e' l'identita' di un artefatto, e i messaggi di questo
    // progetto non portano contenuto.
    let ricalcolato = Esadecimale32::dai_byte(hasher.finalize().into());
    if &ricalcolato != digest_atteso {
        // `DataMapping` e non `Internal`: un artefatto verificato non e'
        // immutabile, e chi lo altera sta fuori da questo processo. Dire
        // «difetto interno» accuserebbe il codice di una cosa che non ha
        // fatto, e manderebbe chi legge a cercarla dove non c'e'.
        return Err(PlenoraError::DataMapping(
            "pubblicazione: i byte copiati non rendono il digest verificato".to_owned(),
        )
        .with_phase(ErrorPhase::Write));
    }
    Ok(())
}

/// Byte copiati per volta: memoria costante, come nel passo 5-bis.
const BLOCCO_COPIA: usize = 64 * 1024;

#[cfg(test)]
mod tests;
