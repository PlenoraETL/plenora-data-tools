//! Che cosa si vede su una destinazione, dopo che qualcuno ha provato a
//! pubblicarci.
//!
//! # Che cosa vive qui
//!
//! La **superficie pubblica** della domanda «com'e' andata?»: le cinque
//! osservazioni che si possono fare su una destinazione dato il token di un
//! tentativo, e le ragioni per cui una destinazione esistente puo' restare
//! non giudicabile.
//!
//! # Perche' un modulo separato da `verifica`
//!
//! Perche' i due hanno confini diversi. `verifica` **non tocca niente**: apre,
//! legge, confronta, e qualunque cosa concluda lascia il mondo come lo trova. Il
//! passo 9 invece e' l'unico che rende visibile un output, ed e' irreversibile:
//! dopo, nessun evento successivo puo' renderlo non riuscito.
//!
//! Tenerli nello stesso modulo direbbe che sono la stessa specie di cosa, e la
//! specie e' proprio cio' che li distingue: uno si puo' rifare, l'altro no.
//!
//! # Che cosa non c'e' ancora
//!
//! Il **passo 9**. `risolvi_commit` guarda una destinazione e non la tocca: e'
//! la meta' di questo modulo che non ha bisogno di nient'altro, e ha un
//! chiamante che sta per definizione fuori — chi non riceve risposta dal
//! processo incaricato di pubblicare. Il passo 9 arriva insieme al verificatore
//! che gli fornisce la prova, perche' senza quella prova sarebbe una funzione
//! che accetta un percorso e si fida.

use std::io::ErrorKind;
use std::path::Path;

use plenora_core::error::{ErrorCategory, PlenoraError};

use crate::commit_footer::interpreta_commit_token;
use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
use crate::geo_transport::error::ArrowTransportError;
use crate::ipc_boundary::{
    convalida_artefatto_con_causa, ArtefattoConvalidato, CausaDiApertura, IpcLimits,
};

/// Che cosa si vede guardando una destinazione, dato il token di un tentativo.
///
/// # Perche' osservazioni e non decisioni
///
/// Perche' chi chiama sa cose che questo codice non sa: se quel percorso e'
/// suo, se qualcun altro ci scrive, se ritentare abbia senso. Rendere una
/// decisione — «riprova», «rinuncia» — vorrebbe dire prenderla al posto suo con
/// meno informazioni delle sue.
///
/// # Perche' non e' un `Result`
///
/// Perche' nessuna di queste cinque e' un guasto di questa funzione: sono tutte
/// osservazioni riuscite. Anche [`Self::InvalidOrUnreadable`] dice che si e'
/// guardato e che cosa si e' trovato, non che l'osservazione sia fallita.
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
/// # Perche' strutturata invece che un testo
///
/// Perche' le decisioni che ne seguono sono diverse, e un testo obbligherebbe
/// chi legge a distinguerle confrontando stringhe che cambiano con la
/// piattaforma e con la lingua del sistema. Un permesso negato si risolve con i
/// permessi; un sigillo che non corrisponde e' un file da non toccare.
///
/// # Perche' otto e non cinque
///
/// Perche' cinque sono le **osservazioni**, non le ragioni. Un elenco che
/// coprisse solo permesso, framing, sigillo e footer costringerebbe a
/// classificare come «framing non valido» un disco che risponde male, o come
/// «footer rifiutato» un tetto del confine superato: due diagnosi false che
/// mandano chi legge dalla parte sbagliata. Ogni guasto che il codice sa
/// distinguere ha una voce sua; quelli che non sa distinguere stanno in
/// [`Self::GuastoDiLettura`], che dice esattamente quello.
///
/// # Che cosa non porta
///
/// Nessun byte dei dati e nessun frammento di percorso oltre quello che il
/// chiamante ha gia' passato: dice **di che genere** di guasto si tratta, non
/// che cosa il file contiene. E' la stessa regola di ogni altro errore del
/// progetto, e vale anche qui perche' questa e' superficie pubblica.
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
/// Non e' un metodo dell'oggetto d'esecuzione: quell'oggetto puo' mancare, ed e'
/// proprio la situazione in cui la domanda si pone.
///
/// # Perche' non rende un `Result`
///
/// Perche' ogni esito qui e' un'**osservazione riuscita**, compreso quello che
/// dice di non poter concludere. Chi chiama non riceve risposta dal processo
/// incaricato di pubblicare, e cio' che gli serve e' sapere cosa c'e' sul
/// disco: un errore lo costringerebbe a distinguere «non ho potuto guardare» da
/// «ho guardato e non si capisce», che e' proprio la distinzione che
/// [`RagioneNonLeggibile`] gli da' gia'.
///
/// # Quali tetti applica
///
/// Quelli di default del confine. La destinazione e' un artefatto che questo
/// programma ha prodotto, non un ingresso di terzi, e la firma non porta limiti
/// perche' chiederli a chi ha perso il processo vorrebbe dire chiedergli di
/// ricostruire anche il budget della scrittura.
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
/// # Perche' non basta il footer
///
/// Perche' il footer descrive **dove** stanno i blocchi, non che cosa
/// contengano: un file con envelope e footer intatti e corpi illeggibili passa
/// ogni controllo di struttura. Il formato Arrow file non ha un checksum
/// sull'intero contenuto, quindi l'unico modo di sapere se i corpi si leggono
/// e' leggerli.
///
/// Si fa **solo** quando il token e' il nostro. Un tentativo altrui e un token
/// assente non affermano niente sulla leggibilita', quindi pagarne la lettura
/// sarebbe spesa senza risposta.
///
/// # Che cosa questo **non** dimostra
///
/// Che i byte siano quelli verificati. Il digest dichiarato vive nell'`Esito`
/// del worker, e chi arriva qui l'`Esito` non ce l'ha — e' precisamente la
/// situazione in cui la domanda si pone. `CommittedMatching` dice «di questo
/// tentativo, e leggibile per intero», non «identico a cio' che fu verificato».
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
/// # Perche' le voci del confine si elencano, e il resto no
///
/// Perche' `ArrowTransportError` copre due domini: il confine — apertura,
/// sigillo, framing, tetti, footer — e l'esecuzione dei kernel. Elencarle tutte
/// obbligherebbe a decidere qui che ragione dare a un guasto di `polygonize`,
/// che questa porta non puo' produrre: aprire e validare un file non esegue
/// nessuna operazione.
///
/// Si elencano quindi **tutte** le voci del confine, una per una, perche' li'
/// una variante nuova deve costringere a scegliere. Il ramo finale raccoglie il
/// resto e lo dichiara per quello che e': un guasto che questa porta non sa
/// distinguere, ed e' meglio dirlo che dargli una voce piu' precisa e falsa.
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

#[cfg(test)]
mod tests;
