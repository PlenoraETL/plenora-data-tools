//! I casi della pubblicazione: le cinque osservazioni, e il passo 9.
//!
//! Le destinazioni si costruiscono **a mano**, byte per byte: un caso che
//! passasse dalla pubblicazione per preparare cio' che poi osserva proverebbe
//! che le due funzioni concordano, non che ciascuna dica il vero.

use std::sync::Arc;

use plenora_core::arrow::array::{RecordBatch, StringArray, UInt64Array};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::DataContract;
use sha2::{Digest as _, Sha256};

use super::{pubblica, risolvi_commit, OsservazioneDelCommit, PublishProfile, RagioneNonLeggibile};
use crate::commit_footer::scrivi_commit_token;
use crate::commit_token::CommitToken;
use crate::geo_transport::publish::{PublishOutcome, PuliziaDelTemporaneo};
use crate::protocollo::digest::ALGORITMO_DIGEST;
use crate::protocollo::messaggi::{ConteggiDichiarati, DigestArtefatto};
use crate::verifica::{verifica_artefatto, AtteseVerifica};

const NOSTRO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ALTRUI: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn token(testo: &str) -> CommitToken {
    CommitToken::da_esadecimale(testo).expect("canonico")
}

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::UInt64, false),
        Field::new("nome", DataType::Utf8, true),
    ]))
}

fn batch() -> RecordBatch {
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(UInt64Array::from(vec![1_u64, 2, 3])),
            Arc::new(StringArray::from(vec![Some("alfa"), None, Some("beta")])),
        ],
    )
    .expect("batch valido")
}

/// Un artefatto valido, col token dato — o senza, se `None`.
fn artefatto(tok: Option<&CommitToken>) -> Vec<u8> {
    let mut byte = Vec::new();
    {
        let mut scrittore = FileWriter::try_new(&mut byte, &schema()).expect("writer");
        scrittore.write(&batch()).expect("batch scritto");
        scrivi_commit_token(&mut scrittore, tok);
        scrittore.finish().expect("finish");
    }
    byte
}

fn digest_di(byte: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(byte);
    let esito: [u8; 32] = hasher.finalize().into();
    let mut testo = String::with_capacity(64);
    for grezzo in esito {
        let _ = write!(testo, "{grezzo:02x}");
    }
    testo
}

/// Una stanza temporanea, con un file scritto dentro.
struct Stanza {
    _dir: tempfile::TempDir,
    percorso: std::path::PathBuf,
}

impl Stanza {
    fn con(nome: &str, byte: &[u8]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let percorso = dir.path().join(nome);
        std::fs::write(&percorso, byte).expect("scrittura");
        Self {
            _dir: dir,
            percorso,
        }
    }

    fn vuota() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let percorso = dir.path().join("assente.arrow");
        Self {
            _dir: dir,
            percorso,
        }
    }

    fn accanto(&self, nome: &str) -> std::path::PathBuf {
        self.percorso
            .parent()
            .expect("la stanza ha un padre")
            .join(nome)
    }
}

// ---------------------------------------------------------------------------
// Le cinque osservazioni
// ---------------------------------------------------------------------------

/// **Una destinazione che non c'e' e' assente**, e non «non leggibile».
///
/// # Che cosa esclude
///
/// Che l'assenza finisca fra le ragioni. Sono due cose diverse: chi legge
/// `Absent` sa che non c'e' niente, chi legge una ragione sa che c'e' qualcosa
/// che non si giudica.
#[test]
fn una_destinazione_che_non_esiste_e_assente() {
    let stanza = Stanza::vuota();
    assert_eq!(
        risolvi_commit(&token(NOSTRO), &stanza.percorso),
        OsservazioneDelCommit::Absent
    );
}

/// **Il nostro token su un artefatto valido e' un commit che combacia.**
#[test]
fn il_nostro_token_su_un_artefatto_valido_combacia() {
    let nostro = token(NOSTRO);
    let stanza = Stanza::con("uscita.arrow", &artefatto(Some(&nostro)));
    assert_eq!(
        risolvi_commit(&nostro, &stanza.percorso),
        OsservazioneDelCommit::CommittedMatching
    );
}

/// **Un token diverso dice che la destinazione e' di un altro tentativo.**
///
/// # Che cosa esclude
///
/// Che il confronto guardi la sola presenza del token. Un artefatto valido con
/// una chiave altrui e' occupato, non riuscito: concluderne «riuscito»
/// attribuirebbe a questo tentativo il lavoro di un altro.
#[test]
fn un_token_altrui_dice_che_la_destinazione_e_occupata() {
    let altrui = token(ALTRUI);
    let stanza = Stanza::con("uscita.arrow", &artefatto(Some(&altrui)));
    assert_eq!(
        risolvi_commit(&token(NOSTRO), &stanza.percorso),
        OsservazioneDelCommit::OccupiedByOtherAttempt
    );
}

/// **Un artefatto valido senza token non ha identita'.**
#[test]
fn un_artefatto_senza_token_non_ha_identita() {
    let stanza = Stanza::con("uscita.arrow", &artefatto(None));
    assert_eq!(
        risolvi_commit(&token(NOSTRO), &stanza.percorso),
        OsservazioneDelCommit::IdentityMissing
    );
}

/// **Un file troncato non si giudica, e la ragione lo dice.**
///
/// # Che cosa esclude
///
/// Che una chiave uguale basti. Qui il file e' tagliato, quindi non c'e'
/// nemmeno un footer da leggere: se `CommittedMatching` non pretendesse la
/// struttura, un artefatto mutilato passerebbe per riuscito.
#[test]
fn un_artefatto_troncato_non_si_giudica() {
    let intero = artefatto(Some(&token(NOSTRO)));
    let tagliato = &intero[..intero.len() / 2];
    let stanza = Stanza::con("uscita.arrow", tagliato);

    let visto = risolvi_commit(&token(NOSTRO), &stanza.percorso);
    assert!(
        matches!(visto, OsservazioneDelCommit::InvalidOrUnreadable(_)),
        "un file tagliato a meta' non e' giudicabile: {visto:?}"
    );
}

/// **Un valore che c'e' e non si interpreta non e' «nessun token».**
///
/// # Che cosa esclude
///
/// Che un token non canonico venga letto come assenza. Sono due diagnosi
/// diverse: l'assenza dice che nessuno ha firmato, questo dice che la firma
/// c'e' e non si legge.
#[test]
fn un_token_non_canonico_e_un_metadato_non_leggibile() {
    let mut byte = Vec::new();
    {
        let mut scrittore = FileWriter::try_new(&mut byte, &schema()).expect("writer");
        scrittore.write(&batch()).expect("batch scritto");
        scrittore.write_metadata(
            crate::commit_token::CHIAVE_FOOTER_COMMIT_TOKEN,
            "non-esadecimale",
        );
        scrittore.finish().expect("finish");
    }
    let stanza = Stanza::con("uscita.arrow", &byte);
    assert_eq!(
        risolvi_commit(&token(NOSTRO), &stanza.percorso),
        OsservazioneDelCommit::InvalidOrUnreadable(RagioneNonLeggibile::MetadatiNonLeggibili)
    );
}

// ---------------------------------------------------------------------------
// Il passo 9
// ---------------------------------------------------------------------------

/// La prova, ottenuta come la ottiene la produzione: facendo girare i passi.
fn verificato(
    percorso: &std::path::Path,
    byte: &[u8],
    tok: &CommitToken,
) -> super::ArtefattoVerificato {
    let digest = DigestArtefatto {
        algoritmo: ALGORITMO_DIGEST.to_owned(),
        valore: digest_di(byte),
    };
    let contratto = DataContract::tabular(schema());
    let attese = AtteseVerifica {
        contratto: &contratto,
        digest: &digest,
        conteggi: ConteggiDichiarati { righe: 3, batch: 1 },
        commit_token: tok,
    };
    verifica_artefatto(
        percorso,
        &attese,
        plenora_core::crs::resolve_crs,
        &crate::ipc_boundary::IpcLimits::default(),
    )
    .expect("l'artefatto di prova supera la sequenza")
}

/// **Il passo 9 rende visibile l'artefatto, e non lascia residui.**
///
/// # Che cosa prova
///
/// Che i byte pubblicati siano **gli stessi** verificati — il confronto e' sul
/// digest, non sulla dimensione — e che il commit atomico non lasci il proprio
/// temporaneo in giro.
#[test]
fn il_passo_nove_pubblica_gli_stessi_byte_verificati() {
    let nostro = token(NOSTRO);
    let byte = artefatto(Some(&nostro));
    let stanza = Stanza::con("temporaneo.arrow", &byte);
    let destinazione = stanza.accanto("uscita.arrow");

    let prova = verificato(&stanza.percorso, &byte, &nostro);
    let esito = pubblica(prova, &destinazione, PublishProfile::Atomic).expect("pubblicazione");

    assert_eq!(esito.durabilita, PublishOutcome::Published);
    assert_eq!(
        esito.pulizia,
        PuliziaDelTemporaneo::Rimosso,
        "il commit atomico non lascia il proprio temporaneo"
    );
    assert_eq!(
        std::fs::read(&destinazione).expect("la destinazione c'e'"),
        byte,
        "i byte pubblicati sono quelli verificati"
    );
    // E la destinazione pubblicata si riconosce come **nostra**: e' la stessa
    // domanda che si porrebbe chi non avesse ricevuto risposta.
    assert_eq!(
        risolvi_commit(&nostro, &destinazione),
        OsservazioneDelCommit::CommittedMatching
    );
}

/// **Una destinazione gia' occupata ferma il passo 9, e non la sostituisce.**
///
/// # Che cosa esclude
///
/// La sovrascrittura. Due esecuzioni sulla stessa destinazione non si
/// sostituiscono: la prima che pubblica vince, e la seconda deve trovare
/// intatto cio' che ha trovato.
#[test]
fn una_destinazione_occupata_non_viene_sostituita() {
    let nostro = token(NOSTRO);
    let byte = artefatto(Some(&nostro));
    let stanza = Stanza::con("temporaneo.arrow", &byte);
    let destinazione = stanza.accanto("uscita.arrow");
    std::fs::write(&destinazione, b"cio' che c'era prima").expect("la destinazione si occupa");

    let prova = verificato(&stanza.percorso, &byte, &nostro);
    let esito = pubblica(prova, &destinazione, PublishProfile::Atomic);

    assert!(esito.is_err(), "una destinazione occupata ferma il passo 9");
    assert_eq!(
        std::fs::read(&destinazione).expect("la destinazione c'e' ancora"),
        b"cio' che c'era prima",
        "cio' che c'era non e' stato toccato"
    );
}

/// **Il profilo durabile chiede di piu', e lo dichiara.**
#[test]
fn il_profilo_durabile_dichiara_il_proprio_esito() {
    let nostro = token(NOSTRO);
    let byte = artefatto(Some(&nostro));
    let stanza = Stanza::con("temporaneo.arrow", &byte);
    let destinazione = stanza.accanto("uscita.arrow");

    let prova = verificato(&stanza.percorso, &byte, &nostro);
    let esito =
        pubblica(prova, &destinazione, PublishProfile::DurableAtomic).expect("pubblicazione");

    // Su una piattaforma che non sincronizza le directory l'esito e' «non
    // confermata», e resta un successo: il caso non pretende quale delle due,
    // pretende che sia una delle due e che l'output ci sia.
    assert!(matches!(
        esito.durabilita,
        PublishOutcome::Published | PublishOutcome::PublishedButDurabilityUnconfirmed
    ));
    assert!(destinazione.is_file(), "l'output e' visibile in ogni caso");
}

// --- i due accertamenti della copia -----------------------------------------
//
// I casi qui sotto passano dalla **porta di produzione**: verificano, poi
// alterano il file, poi chiamano il passo 9. Costruire a mano una discordanza
// che quella porta non puo' produrre proverebbe che la riga esiste, non che
// serva a qualcosa; e un controllo che nessun percorso reale puo' far fallire
// non e' un controllo.
//
// Alterare il file dopo la verifica e' precisamente cio' che puo' succedere:
// un handle aperto difende dalla **sostituzione** del percorso, non dalla
// **mutazione in place** dei byte, ed e' una non-garanzia gia' dichiarata.

/// **Un artefatto accorciato dopo la verifica non si pubblica.**
///
/// E' il caso che rende non vacua la misura ripresa dal descrittore: la prova
/// porta il numero di byte visto alla verifica, il descrittore quello di
/// adesso, e i due vengono da momenti diversi.
#[test]
fn un_artefatto_accorciato_dopo_la_verifica_non_si_pubblica() {
    let nostro = token(NOSTRO);
    let byte = artefatto(Some(&nostro));
    let stanza = Stanza::con("temporaneo.arrow", &byte);
    let destinazione = stanza.accanto("uscita.arrow");

    let prova = verificato(&stanza.percorso, &byte, &nostro);

    // Fra la verifica e il passo 9 il file perde un byte. Il percorso non e'
    // stato sostituito: e' lo stesso file, piu' corto.
    std::fs::write(&stanza.percorso, &byte[..byte.len() - 1]).expect("il file si accorcia");

    let esito = pubblica(prova, &destinazione, PublishProfile::Atomic);

    let errore = esito.expect_err("un artefatto cambiato non si pubblica");
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::DataMapping,
        "il file e' cambiato sotto di noi, non e' un difetto interno"
    );
    assert!(
        !destinazione.exists(),
        "e la destinazione non appare, nemmeno vuota"
    );
}

/// **Un artefatto alterato senza cambiare lunghezza non si pubblica.**
///
/// Qui la misura combacia — stessi byte in numero — e a fermarlo e' il solo
/// digest ricalcolato sui byte copiati. E' il caso che distingue i due
/// accertamenti: se il digest non ci fosse, questo passerebbe.
#[test]
fn un_artefatto_alterato_a_pari_lunghezza_non_si_pubblica() {
    let nostro = token(NOSTRO);
    let byte = artefatto(Some(&nostro));
    let stanza = Stanza::con("temporaneo.arrow", &byte);
    let destinazione = stanza.accanto("uscita.arrow");

    let prova = verificato(&stanza.percorso, &byte, &nostro);

    // Un byte del corpo cambia; la lunghezza no. Si tocca il mezzo del file,
    // lontano da header e footer, perche' il passo 9 non li rilegge: cio' che
    // deve accorgersene e' il digest, non il framing.
    let mut alterato = byte;
    let meta = alterato.len() / 2;
    alterato[meta] ^= 0xff;
    std::fs::write(&stanza.percorso, &alterato).expect("il file si altera");

    let esito = pubblica(prova, &destinazione, PublishProfile::Atomic);

    let errore = esito.expect_err("byte alterati non si pubblicano");
    assert_eq!(errore.category(), plenora_core::ErrorCategory::DataMapping);
    assert!(
        !destinazione.exists(),
        "il commit non avviene, quindi non c'e' niente da vedere"
    );
}

/// **Un corpo illeggibile non e' un commit riuscito, anche se l'involucro regge.**
///
/// E' il caso che distingue «struttura valida» da «leggibile». Il footer dice
/// **dove** stanno i blocchi e con che intestazione, non che cosa contengano, e
/// il formato Arrow file non ha un checksum sull'intero contenuto: senza
/// percorrere i corpi, una destinazione col nostro token e i dati illeggibili
/// direbbe `CommittedMatching` — cioe' «riuscito» di un output inutilizzabile.
///
/// # Perche' questo caso non puo' diventare vacuo
///
/// Perche' pretende la propria premessa: **prima** afferma che la convalida di
/// struttura passa, e solo dopo che l'osservazione rifiuta. Se un domani quella
/// posizione diventasse strutturalmente invalida, a fallire sarebbe la prima
/// affermazione — rumorosamente — invece di passare per la ragione sbagliata.
///
/// La posizione non e' scelta a caso: su questa fissatura sono 101 su 1210 i
/// byte che solo la traversata vede, e questo e' il primo.
#[test]
fn un_corpo_illeggibile_col_nostro_token_non_e_un_commit_riuscito() {
    /// Un byte del corpo che la convalida di struttura non guarda.
    const DENTRO_AL_CORPO: usize = 272;

    let nostro = token(NOSTRO);
    let byte = artefatto(Some(&nostro));
    let mut corrotto = byte;
    corrotto[DENTRO_AL_CORPO] ^= 0xff;
    let stanza = Stanza::con("uscita.arrow", &corrotto);

    // La premessa: involucro, footer e token reggono ancora.
    let (trovato, _artefatto) = crate::ipc_boundary::convalida_artefatto_con_causa(
        &stanza.percorso,
        &crate::ipc_boundary::IpcLimits::default(),
        crate::commit_token::CHIAVE_FOOTER_COMMIT_TOKEN,
    )
    .unwrap_or_else(|_| panic!("la struttura deve reggere, o il caso giudica un'altra cosa"));
    assert_eq!(
        trovato.as_deref(),
        Some(NOSTRO),
        "e il token e' ancora il nostro"
    );

    // La conclusione: chi osserva non dichiara riuscito cio' che non si legge.
    let osservazione = risolvi_commit(&nostro, &stanza.percorso);
    assert!(
        matches!(osservazione, OsservazioneDelCommit::InvalidOrUnreadable(_)),
        "struttura valida e corpo illeggibile: si e' guardato senza poter \
         concludere, non si e' concluso «riuscito» — {osservazione:?}"
    );
}
