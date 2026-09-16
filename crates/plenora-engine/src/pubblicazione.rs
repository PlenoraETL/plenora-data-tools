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
//! I passi da 3 a 8-bis stanno in [`crate::verifica`]; il passo 9 — la
//! pubblicazione atomica no-clobber — sta qui, in [`pubblica`].
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
//! # I tre vincoli che questo modulo rispetta
//!
//! **Il passo 9 non accetta un percorso.** Un `&Path` non porta con se' la prova
//! di aver superato i passi da 3 a 8-bis: chiunque potrebbe costruirne uno e
//! chiedere di pubblicare un file che nessuno ha verificato. Cio' che il passo 9
//! consuma e' [`ArtefattoVerificato`], **opaco e prodotto soltanto dal
//! verificatore**, con i campi privati e nessun costruttore aperto — la stessa
//! forma di `isolamento::NumeriDelCanale`, dove «rivalidato» e' una proprieta'
//! del tipo e non una promessa nel commento di chi lo costruisce. E lo consuma
//! per valore: una prova che si potesse riusare direbbe che due pubblicazioni
//! diverse hanno la stessa verifica dietro.
//!
//! **Il residuo dice che cosa e dove, e sa di non sapere.** Nel ripiego di
//! `persist_noclobber` — `hard_link` seguito da `unlink`, con l'errore
//! dell'`unlink` ignorato — il temporaneo puo' restare al suo posto. I soli byte
//! non basterebbero: chi deve bonificare ha bisogno del percorso. E
//! l'accertamento stesso puo' fallire, **dopo** un commit gia' riuscito: percio'
//! `geo_transport::publish::PuliziaDelTemporaneo` sa dire «non l'ho potuto
//! accertare», che non e' ne' «niente da bonificare» ne' un fallimento della
//! pubblicazione. E' la stessa distinzione a tre stati della quiescenza di un
//! dominio, e per la stessa ragione: confondere «vuoto» con «non l'ho potuto
//! guardare» e' fail-open.
//!
//! **La durabilita' e il residuo restano due fatti.** Uno riguarda la
//! destinazione, l'altro il temporaneo; comprimerli in un enum solo
//! costringerebbe a inventare una variante per ogni combinazione. Stanno percio'
//! sui due assi di `geo_transport::publish::EsitoDellaPubblicazione`.

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

#[cfg(any(test, feature = "internals"))]
/// L'artefatto che ha superato i passi da 3 a 8-bis.
///
/// # Perche' un tipo, e non un percorso
///
/// Perche' un `&Path` non porta con se' nessuna prova: chiunque potrebbe
/// costruirne uno e chiedere di pubblicare un file che nessuno ha verificato. Un
/// tipo con i campi privati e nessun costruttore aperto rende quella pretesa
/// **irrappresentabile**: l'unico modo di averne uno e' che il verificatore lo
/// abbia prodotto, e produrlo significa aver attraversato la sequenza.
///
/// E' la stessa forma di `isolamento::NumeriDelCanale`, dove «rivalidato» e' cio'
/// che il tipo significa e non una promessa nel commento di chi lo costruisce.
///
/// # Perche' conserva l'handle, e non lo riapre
///
/// Perche' riaprire per percorso darebbe al passo 9 la possibilita' di trovare
/// un file **diverso** da quello verificato: fra la verifica e la
/// pubblicazione ci sarebbe una finestra, e la prova varrebbe per un file che
/// non e' piu' quello. L'handle e' lo stesso che ha letto il sigillo.
///
/// # Perche' non e' clonabile
///
/// Perche' una prova che si potesse duplicare direbbe che due pubblicazioni
/// diverse hanno la stessa verifica dietro. Il passo 9 la **consuma**.
///
/// # Perche' non deriva `Debug`
///
/// Perche' porterebbe il digest in ogni log che stampasse la prova, e il digest
/// e' l'identita' di un artefatto. Il tipo che la custodisce non la mostra.
pub(crate) struct ArtefattoVerificato {
    /// L'handle gia' aperto e convalidato: la sorgente dei byte da pubblicare.
    artefatto: ArtefattoConvalidato,
    /// Quanti byte la verifica ha misurato.
    byte_verificati: u64,
    /// Il digest che quei byte devono rendere.
    digest_atteso: Esadecimale32,
}

#[cfg(any(test, feature = "internals"))]
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

#[cfg(any(test, feature = "internals"))]
/// Passo 9: rende visibile l'artefatto verificato, **senza mai sostituire**.
///
/// # Perche' copia invece di spostare il file
///
/// Perche' il commit atomico no-clobber ha gia' un'autorita' qualificata in
/// questo crate — tempfile nella directory di destinazione, `sync_all`, retry
/// sui guasti transitori, `persist_noclobber`, `fsync` della directory secondo
/// il profilo — e quell'autorita' scrive **attraverso un writer**. Spostare il
/// file dov'e' vorrebbe dire una seconda implementazione del commit: su Unix
/// `renameat2(RENAME_NOREPLACE)` con ripiego, su Windows `MoveFileExW`, cioe'
/// codice per piattaforma e una dipendenza nuova, per riottenere garanzie che
/// gia' esistono e sono provate.
///
/// **Il costo e' dichiarato**: una lettura e una scrittura integrali in piu', e
/// le due copie coesistono fino al commit. E' lo stesso genere di costo gia'
/// accettato per il passo 5-bis, e per la stessa ragione: si paga una passata
/// per non fidarsi.
///
/// **Condizione di rientro**: una primitiva cross-platform qualificata che
/// committi direttamente un file esistente conservando no-clobber e
/// l'osservabilita' della pulizia.
///
/// # Che cosa si pretende durante la copia
///
/// Il **numero esatto** di byte verificati, preteso prima di leggere: la prova
/// e l'handle portano quel numero da due momenti diversi, e se si contraddicono
/// la prova non riguarda questo handle. Poi lo **SHA-256 ricalcolato sui byte
/// effettivamente copiati**, confrontato prima del commit. Non e' una ripetizione
/// del passo 5-bis: quello ha misurato i byte letti allora, questo misura i byte
/// che finiscono nella destinazione. Fra i due c'e' una copia, ed e' proprio la
/// copia a poter sbagliare.
///
/// Qualunque divergenza ferma la closure, e una closure che si ferma vuol dire
/// che il commit non avviene: la destinazione **non appare**.
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

#[cfg(any(test, feature = "internals"))]
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
    // Il conteggio, chiesto al descrittore **adesso**: e' una misura nuova, non
    // la copia di quella che la prova gia' porta. Confrontare `byte_verificati`
    // con `artefatto.byte_totali()` sarebbe tautologico — entrambi vengono
    // dall'apertura, e il duplicato porta con se' lo stesso valore — mentre
    // interrogare il descrittore scopre l'unica cosa che puo' essere successa
    // davvero: il file mutato **in place** dopo la verifica.
    //
    // Dopo il ciclo non ci sarebbe niente da chiedere: il ciclo legge a offset
    // esatti e `leggi_a` **fallisce** invece di consegnare corto, quindi un
    // confronto finale fra `copiati` e il numero che ha guidato il ciclo
    // direbbe solo che il ciclo ha girato.
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

#[cfg(any(test, feature = "internals"))]
/// Byte copiati per volta: memoria costante, come nel passo 5-bis.
const BLOCCO_COPIA: usize = 64 * 1024;

#[cfg(test)]
mod tests;
