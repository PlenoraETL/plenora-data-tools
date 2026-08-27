//! Prove della scrittura e della rilettura del `commit_token` nel footer.
//!
//! Le quattro forme del footer — assente, canonico, non canonico, duplicato —
//! e la proprieta' che le rende utili: **senza token i byte non cambiano**.
//! Quella non discende dalle altre. Un writer che scrivesse una chiave vuota
//! quando il token manca supererebbe tutti i test sul contenuto e cambierebbe
//! ogni artefatto gia' prodotto.

use std::sync::Arc;

use plenora_core::arrow::array::{RecordBatch, StringArray, UInt64Array};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;

use crate::planner::contract_fingerprint;

use super::{leggi_commit_token, scrivi_commit_token};
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
    scrivi_commit_token(&mut scrittore, token);
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
/// artefatto di cui non sappiamo dire a quale tentativo appartenga.
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
/// E' la promessa che rende innocuo aggiungere il token al percorso
/// in-process, che passa sempre `None`. Un writer che scrivesse una chiave
/// vuota supererebbe ogni test sul contenuto e cambierebbe ogni artefatto gia'
/// prodotto.
#[test]
fn senza_token_i_byte_non_cambiano() {
    let mut senza_token = Vec::new();
    let mut scrittore = FileWriter::try_new(&mut senza_token, &schema()).expect("writer");
    scrittore.write(&batch()).expect("batch");
    scrittore.finish().expect("finish");
    drop(scrittore);

    assert_eq!(
        artefatto(None),
        senza_token,
        "`scrivi_commit_token(None)` ha cambiato i byte dell'artefatto"
    );
}

/// Stesso token, stessi byte: scrivere il token non introduce non determinismo.
#[test]
fn lo_stesso_token_rende_gli_stessi_byte() {
    assert_eq!(artefatto(Some(&token(UNO))), artefatto(Some(&token(UNO))));
}

/// Token diverso, byte diversi — ma lo **schema** resta lo stesso.
///
/// E' la distinzione che conta: il token cambia l'artefatto, non i dati.
/// Se cambiasse anche lo schema, cambierebbe il `DataContract` e con esso il
/// fingerprint, e due esecuzioni identiche del medesimo piano risulterebbero
/// incompatibili solo per il tentativo che le ha prodotte.
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

    // E lo schema e' quello di partenza, non uno arricchito dal token.
    assert_eq!(schema_uno.as_ref(), schema().as_ref());

    // --- E il `DataContract` col suo fingerprint.
    //
    // Lo schema uguale **non implica** il contratto uguale: il contratto si
    // ricava dallo schema piu' i metadati canonici, e un token che ne
    // toccasse uno cambierebbe il fingerprint senza cambiare i campi. Sarebbe
    // il difetto peggiore di questa PR — due esecuzioni identiche dello stesso
    // piano diventerebbero incompatibili solo per il tentativo che le ha
    // prodotte.
    //
    // Va detto che cosa sorveglia davvero, perche' oggi **nessuna mutazione
    // del codice di produzione lo fa fallire**: il token finisce nel footer,
    // che non entra nello schema, quindi il fingerprint non puo' divergere
    // per costruzione. Questa asserzione e' una guardia contro un cambiamento
    // che oggi non esiste — scrivere il token nei metadati dello schema
    // invece che nel footer — e quel giorno sarebbe l'unica cosa a
    // fermarlo. E' una guardia dichiarata, non una prova verificata: chiamarla
    // «coperta dalle mutazioni» sarebbe falso.
    let impronta = |schema: SchemaRef| {
        let contratto = contract_from_arrow_schema(schema, risolvi_crs)
            .expect("il contratto si ricava dallo schema");
        let impronta = contract_fingerprint(&contratto).expect("fingerprint");
        (contratto, impronta)
    };
    let (contratto_uno, impronta_uno) = impronta(schema_uno);
    let (contratto_due, impronta_due) = impronta(schema_due);
    // Il confronto e' sul **fingerprint** e non sul `DataContract`: il
    // fingerprint *e'* l'identita' canonica del contratto — l'hash della sua
    // forma canonica — quindi confrontarlo dice piu' di un'uguaglianza
    // struttura per struttura, che fra l'altro il tipo non offre.
    assert_eq!(
        impronta_uno, impronta_due,
        "il token ha cambiato il contract_fingerprint"
    );

    // E le **proprieta' strutturali**, una per una.
    //
    // Non un confronto di `Debug`: quello e' testo, cambia con la
    // formattazione e non e' un'autorita' su niente. Questi sono i campi che
    // il contratto dichiara, e sono anche cio' che un fingerprint uguale
    // dovrebbe implicare — verificarli separatamente e' quello che rende il
    // fingerprint una prova invece che una tautologia su due contratti vuoti.
    assert_eq!(
        contratto_uno.schema, contratto_due.schema,
        "il token ha cambiato lo schema del contratto"
    );
    assert_eq!(
        contratto_uno.geometries.len(),
        contratto_due.geometries.len(),
        "il token ha cambiato le colonne geometriche"
    );
    assert_eq!(
        contratto_uno.active_geometry, contratto_due.active_geometry,
        "il token ha cambiato la geometria attiva"
    );
    assert_eq!(
        contratto_uno.properties, contratto_due.properties,
        "il token ha cambiato le proprieta' del contratto"
    );
    // E il contratto non e' vuoto: due contratti senza colonne avrebbero lo
    // stesso fingerprint e la prova non direbbe nulla.
    assert_eq!(
        contratto_uno.schema.fields().len(),
        2,
        "le fixture hanno due colonne"
    );
}

/// Un CRS non serve a queste fixture: lo schema non ha colonne geometriche.
fn risolvi_crs(
    _identificatore: &str,
    _contesto: &'static str,
) -> Result<plenora_core::crs::ResolvedCrs, plenora_core::crs::CrsError> {
    unreachable!("nessuna colonna geometrica nelle fixture del commit_footer")
}

/// Il `plan_hash` non dipende dal `commit_token`, e il legame e' **causale**.
///
/// La stesura precedente validava due volte lo stesso piano con lo stesso
/// contratto, costruito da uno schema di comodo: due `plan_hash` uguali erano
/// garantiti dal determinismo di `validate`, non dal fatto che il token non
/// conti. Provava il determinismo e lo spacciava per indipendenza.
///
/// Qui la catena e' completa e ogni anello parte dall'artefatto vero:
///
/// 1. si producono **due artefatti** con token diversi;
/// 2. si rilegge lo `Schema` di **ciascuno**;
/// 3. si costruisce il `DataContract` da **ciascuno** schema;
/// 4. si valida **lo stesso piano** con ciascun contratto;
/// 5. si confrontano i `plan_hash`.
///
/// Cosi' un token che toccasse lo schema — l'unico modo in cui potrebbe
/// arrivare al piano — farebbe divergere i due hash, e il test lo direbbe.
#[test]
fn il_plan_hash_non_dipende_dal_token_lungo_tutta_la_catena() {
    use plenora_core::arrow::ipc::reader::FileReader;

    use crate::planner::validate;

    const PIANO: &str = r#"{
        "schema_version": 5,
        "inputs": ["ingresso"],
        "output": "n0",
        "nodes": [
            {"id": "n0", "op": "table.filter", "in": ["ingresso"],
             "config": {"column": "id", "operator": ">", "value": 0}}
        ]
    }"#;

    // 1-4: dall'artefatto al `plan_hash`, senza scorciatoie.
    let hash_di = |token: &CommitToken| {
        let byte = artefatto(Some(token));
        let lettore = FileReader::try_new(std::io::Cursor::new(byte), None).expect("lettore");
        let schema = lettore.schema();
        let contratto =
            contract_from_arrow_schema(schema, risolvi_crs).expect("contratto dallo schema");
        let ingressi = vec![("ingresso".to_owned(), contratto)];
        validate(PIANO, &ingressi)
            .expect("piano valido")
            .plan_hash()
    };

    // 5: due token diversi, lo stesso `plan_hash`.
    assert_eq!(
        hash_di(&token(UNO)),
        hash_di(&token(DUE)),
        "il `commit_token` e' arrivato fino al `plan_hash`"
    );

    // E l'assenza del token non cambia nulla nemmeno lei: un artefatto
    // ordinario e uno col token descrivono lo stesso contratto.
    let senza = {
        let byte = artefatto(None);
        let lettore = FileReader::try_new(std::io::Cursor::new(byte), None).expect("lettore");
        let contratto = contract_from_arrow_schema(lettore.schema(), risolvi_crs)
            .expect("contratto dallo schema");
        let ingressi = vec![("ingresso".to_owned(), contratto)];
        validate(PIANO, &ingressi)
            .expect("piano valido")
            .plan_hash()
    };
    assert_eq!(
        hash_di(&token(UNO)),
        senza,
        "la presenza del token cambia il `plan_hash`"
    );
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
