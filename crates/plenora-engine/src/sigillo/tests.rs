//! Prove del sigillo.
//!
//! Le quattro forme del footer — assente, canonico, non canonico, duplicato —
//! e la proprieta' che le rende utili: **senza token i byte non cambiano**.
//! Quella non discende dalle altre. Un writer che scrivesse una chiave vuota
//! quando il token manca supererebbe tutti i test sul contenuto e cambierebbe
//! ogni artefatto gia' prodotto.

use std::sync::Arc;

use plenora_core::arrow::array::{RecordBatch, StringArray, UInt64Array};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema};

use super::{leggi_commit_token, sigilla};
use crate::commit_token::{CommitToken, CHIAVE_FOOTER_COMMIT_TOKEN};
use crate::geo_transport::ipc::IpcLimits;

const UNO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DUE: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn token(testo: &str) -> CommitToken {
    CommitToken::da_esadecimale(testo).expect("canonico")
}

fn schema() -> Arc<Schema> {
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
            Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])),
        ],
    )
    .expect("batch valido")
}

/// Scrive un artefatto completo, con o senza token.
fn artefatto(token: Option<&CommitToken>) -> Vec<u8> {
    let mut byte = Vec::new();
    let mut scrittore = FileWriter::try_new(&mut byte, &schema()).expect("writer");
    scrittore.write(&batch()).expect("batch scritto");
    sigilla(&mut scrittore, token);
    scrittore.finish().expect("finish");
    drop(scrittore);
    byte
}

/// Scrive un artefatto con una coppia di metadata arbitraria.
fn artefatto_con_metadata(coppie: &[(&str, &str)]) -> Vec<u8> {
    let mut byte = Vec::new();
    let mut scrittore = FileWriter::try_new(&mut byte, &schema()).expect("writer");
    scrittore.write(&batch()).expect("batch scritto");
    for (chiave, valore) in coppie {
        scrittore.write_metadata(*chiave, *valore);
    }
    scrittore.finish().expect("finish");
    drop(scrittore);
    byte
}

fn letto(byte: &[u8]) -> Result<Option<CommitToken>, String> {
    let mut sorgente = byte;
    leggi_commit_token(&mut sorgente, &IpcLimits::default()).map_err(|errore| errore.to_string())
}

// ---------------------------------------------------------------------------
// Le quattro forme del footer
// ---------------------------------------------------------------------------

/// **Assente**: legittimo, ed e' il caso di ogni artefatto ordinario.
#[test]
fn un_footer_senza_token_e_legittimo() {
    let byte = artefatto(None);
    assert_eq!(letto(&byte).expect("il file e' valido"), None);
}

/// **Canonico**: si rilegge identico.
#[test]
fn un_token_canonico_si_rilegge() {
    for testo in [UNO, DUE] {
        let byte = artefatto(Some(&token(testo)));
        let riletto = letto(&byte)
            .expect("il file e' valido")
            .expect("il token c'e'");
        assert_eq!(riletto.in_esadecimale(), testo);
    }
}

/// **Non canonico**: rifiutato, sempre, e senza citare il valore.
///
/// Non si normalizza: un token che non e' quello che diciamo di scrivere e' un
/// artefatto di cui non sappiamo dire chi l'ha autorizzato.
#[test]
fn un_token_non_canonico_e_rifiutato_e_non_compare_nel_messaggio() {
    let sospetti = [
        UNO.to_uppercase(),
        UNO[..63].to_owned(),
        format!("{UNO}0"),
        "non un token".to_owned(),
        String::new(),
    ];
    for valore in sospetti {
        let byte = artefatto_con_metadata(&[(CHIAVE_FOOTER_COMMIT_TOKEN, &valore)]);
        let messaggio = letto(&byte).expect_err("token non canonico");
        assert!(
            messaggio.contains("non canonico"),
            "motivo inatteso per «{valore}»: {messaggio}"
        );
        // Il messaggio e' un `&'static str`: non puo' portare il valore, e
        // qui si verifica che quella garanzia arrivi fino in fondo.
        assert!(
            !messaggio.to_lowercase().contains(&UNO[..16]),
            "il messaggio ha copiato il valore: {messaggio}"
        );
        assert!(
            !messaggio.contains(&valore) || valore.is_empty(),
            "il messaggio ha copiato il valore: {messaggio}"
        );
    }
}

/// **Duplicato**: il nostro writer non puo' produrlo, e va detto perche'.
///
/// `FileWriter::write_metadata` tiene le coppie in una mappa, quindi due
/// scritture della stessa chiave **collassano** in una. Un test che se lo
/// aspettasse rifiutato costruirebbe un caso impossibile e proverebbe una cosa
/// falsa: qui si fissa il fatto vero, cioe' che da questo lato il duplicato
/// non nasce.
///
/// Il duplicato puo' arrivare solo da un produttore estraneo, e li' lo rifiuta
/// la traversata rinforzata — provato in `ipc.rs`, `caso_10`, `caso_11` e,
/// attraverso la variante che estrae, `caso_14`.
#[test]
fn il_nostro_writer_non_puo_emettere_un_token_duplicato() {
    let byte = artefatto_con_metadata(&[
        (CHIAVE_FOOTER_COMMIT_TOKEN, UNO),
        (CHIAVE_FOOTER_COMMIT_TOKEN, DUE),
    ]);
    // Nessun errore: la seconda scrittura ha sostituito la prima.
    let riletto = letto(&byte)
        .expect("il footer non contiene un duplicato")
        .expect("il token c'e'");
    assert_eq!(
        riletto.in_esadecimale(),
        DUE,
        "la seconda scrittura non ha sostituito la prima"
    );
}

/// Le chiavi altrui restano ammesse: il confine valida la forma, non il
/// vocabolario.
#[test]
fn le_chiavi_di_altri_produttori_non_disturbano() {
    let byte = artefatto_con_metadata(&[
        ("org.altro.qualcosa", "valore"),
        (CHIAVE_FOOTER_COMMIT_TOKEN, UNO),
        ("ARROW:extension:name", "x"),
    ]);
    let riletto = letto(&byte)
        .expect("il file e' valido")
        .expect("il token c'e'");
    assert_eq!(riletto.in_esadecimale(), UNO);
}

// ---------------------------------------------------------------------------
// I byte
// ---------------------------------------------------------------------------

/// **Senza token i byte restano identici.**
///
/// E' la promessa che rende innocuo aggiungere il sigillo al percorso
/// in-process, che passa sempre `None`. Un writer che scrivesse una chiave
/// vuota supererebbe ogni test sul contenuto e cambierebbe ogni artefatto gia'
/// prodotto.
#[test]
fn senza_token_i_byte_non_cambiano() {
    let mut senza_sigillo = Vec::new();
    let mut scrittore = FileWriter::try_new(&mut senza_sigillo, &schema()).expect("writer");
    scrittore.write(&batch()).expect("batch");
    scrittore.finish().expect("finish");
    drop(scrittore);

    assert_eq!(
        artefatto(None),
        senza_sigillo,
        "`sigilla(None)` ha cambiato i byte dell'artefatto"
    );
}

/// Stesso token, stessi byte: il sigillo non introduce non determinismo.
#[test]
fn lo_stesso_token_rende_gli_stessi_byte() {
    assert_eq!(artefatto(Some(&token(UNO))), artefatto(Some(&token(UNO))));
}

/// Token diverso, byte diversi — ma lo **schema** resta lo stesso.
///
/// E' la distinzione che conta: il token cambia l'artefatto, non i dati.
/// Se cambiasse anche lo schema, cambierebbe il `DataContract` e con esso il
/// fingerprint, e due esecuzioni identiche del medesimo piano risulterebbero
/// incompatibili solo per chi le ha autorizzate.
#[test]
fn un_token_diverso_cambia_i_byte_ma_non_lo_schema() {
    use plenora_core::arrow::ipc::reader::FileReader;

    let uno = artefatto(Some(&token(UNO)));
    let due = artefatto(Some(&token(DUE)));
    assert_ne!(uno, due, "due token diversi hanno prodotto gli stessi byte");
    assert_eq!(
        uno.len(),
        due.len(),
        "i due token hanno la stessa lunghezza"
    );

    let leggi = |byte: &[u8]| {
        let lettore =
            FileReader::try_new(std::io::Cursor::new(byte.to_vec()), None).expect("lettore");
        let schema = lettore.schema();
        let batch: Vec<RecordBatch> = lettore.map(|b| b.expect("batch")).collect();
        (schema, batch)
    };
    let (schema_uno, batch_uno) = leggi(&uno);
    let (schema_due, batch_due) = leggi(&due);
    assert_eq!(schema_uno, schema_due, "il token ha toccato lo schema");
    assert_eq!(batch_uno, batch_due, "il token ha toccato i dati");

    // E lo schema e' quello di partenza, non uno arricchito dal sigillo.
    assert_eq!(schema_uno.as_ref(), schema().as_ref());
}

/// La lettura passa dalla convalida: un file rotto non rende un token.
///
/// E' la conseguenza di aver messo l'estrazione dentro la traversata
/// rinforzata invece che accanto: non esiste un modo di ottenere il token
/// saltando i controlli.
#[test]
fn un_artefatto_rotto_non_rende_un_token() {
    let mut byte = artefatto(Some(&token(UNO)));
    // Guasta il magic finale: il file non e' piu' un file Arrow.
    let ultimo = byte.len() - 1;
    byte[ultimo] ^= 0xFF;
    assert!(
        letto(&byte).is_err(),
        "un file rotto ha reso comunque un token"
    );

    // E troncato.
    let mut byte = artefatto(Some(&token(UNO)));
    byte.truncate(byte.len() / 2);
    assert!(letto(&byte).is_err(), "un file troncato ha reso un token");
}
