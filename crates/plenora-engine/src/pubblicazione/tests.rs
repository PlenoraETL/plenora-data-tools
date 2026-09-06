//! I casi della pubblicazione: le cinque osservazioni, e il passo 9.
//!
//! Le destinazioni si costruiscono **a mano**, byte per byte: un caso che
//! passasse dalla pubblicazione per preparare cio' che poi osserva proverebbe
//! che le due funzioni concordano, non che ciascuna dica il vero.

use std::sync::Arc;

use plenora_core::arrow::array::{RecordBatch, StringArray, UInt64Array};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};

use super::{risolvi_commit, OsservazioneDelCommit, RagioneNonLeggibile};
use crate::commit_footer::scrivi_commit_token;
use crate::commit_token::CommitToken;

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
