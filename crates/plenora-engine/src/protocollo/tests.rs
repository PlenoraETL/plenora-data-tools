//! Prove della **forma sul filo**.
//!
//! # Perche' i vettori sono scritti a mano
//!
//! Un round-trip prova che il codificatore e il decodificatore concordano.
//! Non prova che concordino su qualcosa di giusto: se il codificatore
//! emettesse `"tipo":"saluto "` con uno spazio, il decodificatore lo
//! riaccetterebbe e il test resterebbe verde per sempre.
//!
//! Per questo l'oracolo e' un **testo scritto a mano**, byte per byte,
//! prefisso compreso — e il codificatore e il decodificatore ci vengono
//! confrontati **separatamente**. Il round-trip resta, ma come prova
//! secondaria: dice solo che i due versi non hanno derive reciproche.

use serde_json::value::RawValue;

use crate::commit_token::CommitToken;

use super::codifica::{
    codifica, decodifica, ScrittoreLimitato, BYTE_PREFISSO, MAX_PROTOCOL_FRAME_BYTES,
};
use super::digest::DigestSha256;
use super::limiti::{
    MAX_BACKEND_DINAMICI, MAX_CAPABILITY, MAX_CHIAVE_CONTEGGIO_BYTES, MAX_CONTEGGI_DIAGNOSTICA,
    MAX_DIGEST_BYTES, MAX_ESEMPIO_BYTES, MAX_ESEMPI_DIAGNOSTICA, MAX_IDENTIFICATORE_BYTES,
    MAX_INGRESSI, MAX_MESSAGGIO_BYTES, MAX_MOTIVO_BYTES, MAX_PERCORSO_BYTES,
    MAX_PIANO_CANONICO_BYTES, MAX_RISORSE, MAX_VERSIONE_BYTES,
};
use super::messaggi::{
    Ambiente, Annulla, BackendDinamico, CategoriaSulFilo, ConteggiDichiarati, Corpo,
    DescrittoreIngresso, DiagnosticaSulFilo, DigestArtefatto, EffettoSulFilo, ErroreSulFilo,
    EsempioDiagnostica, EsitoWorkerSulFilo, FaseSulFilo, FormaPanicSulFilo, FormatoIngresso, Frame,
    IdentitaArtefatto, IdentitaResolver, Incarico, LimitiDichiarati, Progresso, RetrySulFilo,
    RisorsaRisolta, Risposta, Saluto, TipoMessaggio, VERSIONE_PROTOCOLLO,
};

// ---------------------------------------------------------------------------
// I vettori scritti a mano
// ---------------------------------------------------------------------------

const CORPO_SALUTO: &str = concat!(
    r#"{"artefatto":{"digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","versione":"1.0"},"#,
    r#""resolver":{"identita":"proj","versione":"9.4"},"#,
    r#""ambiente":{"digest_insieme":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","acquisizione_dinamica":false,"#,
    r#""risorse":[{"nome":"grid","versione":"1","percorso":"/r/g"}],"#,
    r#""backend_dinamici":[]},"#,
    r#""commit_token":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","#,
    r#""limiti":{"max_frame_bytes":1,"max_piano_canonico_bytes":2,"#,
    r#""max_messaggi_verso_worker":3,"max_messaggi_verso_supervisore":4}}"#,
);
const PREFISSO_SALUTO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x02, 0x6C];

const CORPO_INCARICO: &str = concat!(
    r#"{"piano_canonico":{"schema_version":6},"#,
    r#""plan_hash_atteso":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","#,
    r#""ingressi":[{"nome":"in","percorso":"/d/a.arrow","formato":"file","#,
    r#""contract_fingerprint_atteso":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}],"#,
    r#""artefatto_temporaneo":"/t/out.arrow"}"#,
);
const PREFISSO_INCARICO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x01, 0x79];

const CORPO_ANNULLA: &str = r#"{"motivo":"timeout"}"#;
const PREFISSO_ANNULLA: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0x44];

const CORPO_RISPOSTA: &str = concat!(
    r#"{"artefatto":{"digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","versione":"1.0"},"#,
    r#""resolver":{"identita":"proj","versione":"9.4"},"#,
    r#""ambiente":{"digest_insieme":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","acquisizione_dinamica":false,"#,
    r#""risorse":[],"#,
    r#""backend_dinamici":[{"nome":"gdal","versione":"3.8","percorso":"/l/libgdal.so"}]},"#,
    r#""capability":["arrow_ipc"]}"#,
);
const PREFISSO_RISPOSTA: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x01, 0xC5];

const CORPO_PROGRESSO: &str = r#"{"righe":10,"batch":2,"nodi_completati":3}"#;
const PREFISSO_PROGRESSO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0x5C];

const CORPO_ESITO_SUCCESSO: &str = concat!(
    r#"{"esito":"successo","digest_artefatto":{"algoritmo":"sha256","valore":"ff"},"#,
    r#""conteggi":{"righe":10,"batch":2}}"#,
);
const PREFISSO_ESITO_SUCCESSO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0x9C];

const CORPO_ESITO_ERRORE: &str = concat!(
    r#"{"esito":"errore","errore":{"categoria":"schema","fase":"read","effetto":"none","#,
    r#""retry":{"kind":"after","delay_ms":250},"#,
    r#""messaggio":"colonna mancante","nodo":"n1","operazione":"table.filter","#,
    r#""execution_id":null,"#,
    r#""diagnostica":{"contract":"c","scope":"s","completeness":"exact","#,
    r#""observed_total":5,"conteggi":[["null_non_ammesso",5]],"#,
    r#""esempi":[{"indice":0,"codice":"E_NULL"}],"esempi_troncati":false}}}"#,
);
const PREFISSO_ESITO_ERRORE: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x01, 0xBD];

const CORPO_ESITO_PANIC: &str = r#"{"esito":"panic","forma":"statico"}"#;
const PREFISSO_ESITO_PANIC: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0x51];

/// Il testo completo del payload, involucro compreso.
fn payload(tipo: &str, corpo: &str) -> String {
    format!(r#"{{"protocol_version":1,"tipo":"{tipo}","corpo":{corpo}}}"#)
}

/// I byte completi del frame: prefisso scritto a mano + payload.
fn vettore(prefisso: [u8; BYTE_PREFISSO], tipo: &str, corpo: &str) -> Vec<u8> {
    let testo = payload(tipo, corpo);
    let mut byte = prefisso.to_vec();
    byte.extend_from_slice(testo.as_bytes());
    byte
}

// ---------------------------------------------------------------------------
// Le strutture corrispondenti, costruite a mano
// ---------------------------------------------------------------------------

fn grezzo(testo: &str) -> Box<RawValue> {
    RawValue::from_string(testo.to_owned()).expect("JSON di prova valido")
}

/// Il `commit_token` dei vettori: 64 esadecimali minuscoli, scritti a mano.
fn token_di_prova() -> CommitToken {
    CommitToken::da_esadecimale("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        .expect("canonico")
}

/// Un digest dalla forma canonica, per le fixture.
///
/// I test condividono **il costruttore**, non un valore: ciascuno sceglie il
/// proprio testo, e a garantirne la forma e' il tipo del filo.
///
/// Nei casi «massimi» un digest non puo' essere fatto di caratteri che JSON
/// espande — e' ASCII esadecimale — quindi il tetto del frame resta su questi
/// campi un maggiorante conservativo, ed e' giusto che lo sia.
fn digest(testo: &str) -> DigestSha256 {
    DigestSha256::da_esadecimale(testo).expect("canonico")
}

fn artefatto() -> IdentitaArtefatto {
    IdentitaArtefatto {
        digest: digest("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        versione: "1.0".to_owned(),
    }
}

fn resolver() -> IdentitaResolver {
    IdentitaResolver {
        identita: "proj".to_owned(),
        versione: "9.4".to_owned(),
    }
}

/// L'ambiente dei vettori: digest e bandiera sono gli stessi nei due messaggi
/// che lo portano, a distinguerli e' il **contenuto**, che resta del
/// chiamante. I vettori scritti a mano restano due e indipendenti: qui c'e'
/// l'ingresso, l'atteso e' li'.
fn ambiente_di_prova(
    risorse: Vec<RisorsaRisolta>,
    backend_dinamici: Vec<BackendDinamico>,
) -> Ambiente {
    Ambiente {
        digest_insieme: digest("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        acquisizione_dinamica: false,
        risorse,
        backend_dinamici,
    }
}

fn saluto() -> Frame {
    Frame::nuovo(Corpo::Saluto(Box::new(Saluto {
        artefatto: artefatto(),
        resolver: resolver(),
        ambiente: ambiente_di_prova(
            vec![RisorsaRisolta {
                nome: "grid".to_owned(),
                versione: "1".to_owned(),
                percorso: "/r/g".to_owned(),
            }],
            Vec::new(),
        ),
        commit_token: token_di_prova(),
        limiti: LimitiDichiarati {
            max_frame_bytes: 1,
            max_piano_canonico_bytes: 2,
            max_messaggi_verso_worker: 3,
            max_messaggi_verso_supervisore: 4,
        },
    })))
}

fn incarico() -> Frame {
    incarico_con(
        grezzo(r#"{"schema_version":6}"#),
        vec![DescrittoreIngresso {
            nome: "in".to_owned(),
            percorso: "/d/a.arrow".to_owned(),
            formato: FormatoIngresso::File,
            contract_fingerprint_atteso: digest(
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            ),
        }],
    )
}

fn annulla_con(motivo: String) -> Frame {
    Frame::nuovo(Corpo::Annulla(Annulla { motivo }))
}

fn annulla() -> Frame {
    annulla_con("timeout".to_owned())
}

fn incarico_con(piano: Box<RawValue>, ingressi: Vec<DescrittoreIngresso>) -> Frame {
    Frame::nuovo(Corpo::Incarico(Box::new(Incarico {
        piano_canonico: piano,
        plan_hash_atteso: digest(
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        ),
        ingressi,
        artefatto_temporaneo: "/t/out.arrow".to_owned(),
    })))
}

fn risposta() -> Frame {
    Frame::nuovo(Corpo::Risposta(Box::new(Risposta {
        artefatto: artefatto(),
        resolver: resolver(),
        ambiente: ambiente_di_prova(
            Vec::new(),
            vec![BackendDinamico {
                nome: "gdal".to_owned(),
                versione: "3.8".to_owned(),
                percorso: "/l/libgdal.so".to_owned(),
            }],
        ),
        capability: vec!["arrow_ipc".to_owned()],
    })))
}

fn progresso() -> Frame {
    Frame::nuovo(Corpo::Progresso(Progresso {
        righe: 10,
        batch: 2,
        nodi_completati: 3,
    }))
}

fn esito(dentro: EsitoWorkerSulFilo) -> Frame {
    Frame::nuovo(Corpo::Esito(Box::new(dentro)))
}

fn esito_successo() -> Frame {
    esito(EsitoWorkerSulFilo::Successo {
        digest_artefatto: DigestArtefatto {
            algoritmo: "sha256".to_owned(),
            valore: "ff".to_owned(),
        },
        conteggi: ConteggiDichiarati {
            righe: 10,
            batch: 2,
        },
    })
}

fn esito_errore() -> Frame {
    esito(EsitoWorkerSulFilo::Errore {
        errore: Box::new(ErroreSulFilo {
            categoria: CategoriaSulFilo::Schema,
            fase: FaseSulFilo::Read,
            effetto: EffettoSulFilo::None,
            retry: RetrySulFilo::After { delay_ms: 250 },
            messaggio: "colonna mancante".to_owned(),
            nodo: Some("n1".to_owned()),
            operazione: Some("table.filter".to_owned()),
            execution_id: None,
            diagnostica: Some(DiagnosticaSulFilo {
                contract: "c".to_owned(),
                scope: "s".to_owned(),
                completeness: "exact".to_owned(),
                observed_total: 5,
                conteggi: vec![("null_non_ammesso".to_owned(), 5)],
                esempi: vec![EsempioDiagnostica {
                    indice: 0,
                    codice: "E_NULL".to_owned(),
                }],
                esempi_troncati: false,
            }),
        }),
    })
}

fn esito_panic() -> Frame {
    esito(EsitoWorkerSulFilo::Panic {
        forma: FormaPanicSulFilo::Statico,
    })
}

/// Gli otto casi: i sei tipi, e per l'`Esito` le tre forme.
fn casi() -> Vec<(&'static str, Frame, Vec<u8>)> {
    vec![
        (
            "saluto",
            saluto(),
            vettore(PREFISSO_SALUTO, "saluto", CORPO_SALUTO),
        ),
        (
            "incarico",
            incarico(),
            vettore(PREFISSO_INCARICO, "incarico", CORPO_INCARICO),
        ),
        (
            "annulla",
            annulla(),
            vettore(PREFISSO_ANNULLA, "annulla", CORPO_ANNULLA),
        ),
        (
            "risposta",
            risposta(),
            vettore(PREFISSO_RISPOSTA, "risposta", CORPO_RISPOSTA),
        ),
        (
            "progresso",
            progresso(),
            vettore(PREFISSO_PROGRESSO, "progresso", CORPO_PROGRESSO),
        ),
        (
            "esito/successo",
            esito_successo(),
            vettore(PREFISSO_ESITO_SUCCESSO, "esito", CORPO_ESITO_SUCCESSO),
        ),
        (
            "esito/errore",
            esito_errore(),
            vettore(PREFISSO_ESITO_ERRORE, "esito", CORPO_ESITO_ERRORE),
        ),
        (
            "esito/panic",
            esito_panic(),
            vettore(PREFISSO_ESITO_PANIC, "esito", CORPO_ESITO_PANIC),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Codificatore e decodificatore, confrontati SEPARATAMENTE
// ---------------------------------------------------------------------------

#[test]
fn il_codificatore_produce_i_vettori_scritti_a_mano() {
    for (nome, frame, atteso) in casi() {
        let prodotto = codifica(&frame).expect("il caso nominale si codifica");
        assert_eq!(
            String::from_utf8_lossy(&prodotto[BYTE_PREFISSO..]),
            String::from_utf8_lossy(&atteso[BYTE_PREFISSO..]),
            "payload divergente per «{nome}»"
        );
        assert_eq!(
            prodotto[..BYTE_PREFISSO],
            atteso[..BYTE_PREFISSO],
            "prefisso divergente per «{nome}»"
        );
    }
}

#[test]
fn il_decodificatore_ricostruisce_dai_vettori_scritti_a_mano() {
    for (nome, atteso, byte) in casi() {
        let letto = decodifica(&byte).expect("il vettore nominale si decodifica");
        assert_eq!(letto, atteso, "struttura divergente per «{nome}»");
    }
}

/// Prova **secondaria**: dice che i due versi non hanno derive reciproche, non
/// che siano giusti. Quello lo dicono i due test qui sopra.
#[test]
fn il_giro_completo_torna_al_punto_di_partenza() {
    for (nome, frame, _) in casi() {
        let byte = codifica(&frame).expect("codifica");
        let letto = decodifica(&byte).expect("decodifica");
        assert_eq!(letto, frame, "giro non chiuso per «{nome}»");
    }
}

#[test]
fn la_codifica_e_deterministica_byte_per_byte() {
    for (nome, frame, _) in casi() {
        let una = codifica(&frame).expect("codifica");
        let altra = codifica(&frame).expect("codifica");
        assert_eq!(una, altra, "codifica non deterministica per «{nome}»");
    }
}

#[test]
fn il_prefisso_e_big_endian_e_conta_solo_il_payload() {
    let byte = codifica(&annulla()).expect("codifica");
    let dichiarata = u32::from_be_bytes([byte[0], byte[1], byte[2], byte[3]]) as usize;
    assert_eq!(dichiarata, byte.len() - BYTE_PREFISSO);
    // Big-endian e non little: il byte piu' significativo e' il primo.
    assert_eq!(byte[0], 0x00);
    assert_eq!(byte[3], 0x44);
}

// ---------------------------------------------------------------------------
// Rifiuti
// ---------------------------------------------------------------------------

/// Il messaggio d'errore, per poterci fare asserzioni sopra.
fn rifiuto(byte: &[u8]) -> String {
    match decodifica(byte) {
        Ok(_) => panic!("accettato un frame che andava rifiutato"),
        Err(errore) => errore.to_string(),
    }
}

#[test]
fn il_prefisso_troncato_e_un_errore() {
    for lunghezza in 0..BYTE_PREFISSO {
        let byte = vec![0x00; lunghezza];
        assert!(
            rifiuto(&byte).contains("prefisso troncato"),
            "accettato un prefisso di {lunghezza} byte"
        );
    }
}

#[test]
fn il_payload_troncato_e_un_errore() {
    let mut byte = codifica(&annulla()).expect("codifica");
    byte.pop();
    assert!(rifiuto(&byte).contains("payload troncato"));
}

#[test]
fn i_byte_in_eccesso_dopo_il_frame_sono_un_errore() {
    let mut byte = codifica(&annulla()).expect("codifica");
    byte.push(b' ');
    assert!(rifiuto(&byte).contains("byte in eccesso"));
}

#[test]
fn una_lunghezza_incoerente_e_un_errore() {
    // Il payload e' intatto; e' il prefisso a mentire, in entrambi i versi.
    let originale = codifica(&annulla()).expect("codifica");
    let vera = u32::from_be_bytes([originale[0], originale[1], originale[2], originale[3]]);

    let mut corta = originale.clone();
    corta[..BYTE_PREFISSO].copy_from_slice(&(vera - 1).to_be_bytes());
    assert!(rifiuto(&corta).contains("byte in eccesso"));

    let mut lunga = originale;
    lunga[..BYTE_PREFISSO].copy_from_slice(&(vera + 1).to_be_bytes());
    assert!(rifiuto(&lunga).contains("payload troncato"));
}

/// Il rifiuto del frame sovradimensionato **non tocca il payload**.
///
/// La prova non e' che l'errore arrivi: e' che arrivi con quattro byte in
/// mano. Se il decoder allocasse prima di controllare, questo test dovrebbe
/// riservare mezzo gigabyte per fallire — e invece ne passa quattro.
#[test]
fn un_frame_oltre_il_tetto_e_rifiutato_senza_leggere_il_payload() {
    let dichiarata = u32::try_from(MAX_PROTOCOL_FRAME_BYTES + 1).expect("sta in u32");
    let solo_prefisso = dichiarata.to_be_bytes().to_vec();
    assert_eq!(solo_prefisso.len(), BYTE_PREFISSO);

    let messaggio = rifiuto(&solo_prefisso);
    assert!(
        messaggio.contains("oltre il tetto"),
        "motivo inatteso: {messaggio}"
    );
    assert!(
        messaggio.contains("senza leggere il payload"),
        "motivo inatteso: {messaggio}"
    );
    // Il tetto e' battuto sul *troncamento*: con un payload assente, un
    // decoder che controllasse la lunghezza dopo direbbe «troncato».
    assert!(
        !messaggio.contains("troncato"),
        "il tetto non ha preceduto la lettura: {messaggio}"
    );
}

/// Il predicato che il lettore usera' e' **lo stesso** che usa il decoder.
///
/// E' la parte della difesa che conta davvero contro un pipe: quattro byte in
/// mano, la decisione, e solo dopo la lettura. Se qui e in `decodifica` ci
/// fossero due confronti, potrebbero divergere — e divergerebbero proprio nel
/// caso in cui uno dei due e' sbagliato.
#[test]
fn la_lunghezza_dichiarata_decide_sui_soli_quattro_byte() {
    use super::codifica::lunghezza_dichiarata;

    assert_eq!(lunghezza_dichiarata([0x00, 0x00, 0x00, 0x00]).ok(), Some(0));
    let al_tetto = u32::try_from(MAX_PROTOCOL_FRAME_BYTES).expect("sta in u32");
    assert_eq!(
        lunghezza_dichiarata(al_tetto.to_be_bytes()).ok(),
        Some(MAX_PROTOCOL_FRAME_BYTES),
        "MAX deve passare: il tetto e' inclusivo"
    );
    let oltre = al_tetto + 1;
    assert!(
        lunghezza_dichiarata(oltre.to_be_bytes()).is_err(),
        "MAX + 1 deve fallire"
    );
    assert!(lunghezza_dichiarata([0xFF, 0xFF, 0xFF, 0xFF]).is_err());

    // E il decoder rifiuta con **quello stesso** motivo, cioe' non ha una
    // copia propria del confronto.
    let suo = lunghezza_dichiarata(oltre.to_be_bytes())
        .expect_err("MAX + 1")
        .to_string();
    assert_eq!(rifiuto(&oltre.to_be_bytes()), suo);
}

/// Il tetto e' esclusivo: `MAX` passa il controllo di lunghezza, `MAX + 1` no.
#[test]
fn il_confine_del_tetto_cade_fra_max_e_max_piu_uno() {
    let al_tetto = u32::try_from(MAX_PROTOCOL_FRAME_BYTES).expect("sta in u32");
    let messaggio = rifiuto(&al_tetto.to_be_bytes());
    // Al tetto esatto il rifiuto arriva, ma per un motivo diverso: il payload
    // manca davvero. E' la prova che `MAX` non e' respinto dal tetto.
    assert!(
        messaggio.contains("payload troncato"),
        "MAX rifiutato dal tetto invece che dal troncamento: {messaggio}"
    );
}

#[test]
fn lo_scrittore_limitato_si_ferma_a_max_piu_uno() {
    // Il writer si prova sul proprio confine, perche' e' il confine che
    // esiste: nessun frame *strutturalmente valido* raggiunge
    // `MAX_PROTOCOL_FRAME_BYTES`, che e' un maggiorante e non una misura.
    use std::io::Write;

    let mut esatto = ScrittoreLimitato::nuovo(8);
    assert!(esatto.write_all(&[b'x'; 8]).is_ok());
    assert_eq!(esatto.in_byte().len(), 8);

    let mut oltre = ScrittoreLimitato::nuovo(8);
    let esito = oltre.write_all(&[b'x'; 9]);
    assert!(esito.is_err(), "accettati 9 byte con un tetto di 8");
    // Il byte oltre il tetto non e' stato scritto: il writer si ferma, non
    // misura dopo.
    assert!(oltre.in_byte().len() <= 8);
}

#[test]
fn lo_scrittore_limitato_si_ferma_a_meta_flusso() {
    use std::io::Write;

    let mut scrittore = ScrittoreLimitato::nuovo(4);
    assert!(scrittore.write_all(b"ab").is_ok());
    assert!(scrittore.write_all(b"cde").is_err());
    // I due byte gia' accettati restano; i tre rifiutati non entrano affatto.
    assert_eq!(scrittore.in_byte(), b"ab".to_vec());
}

#[test]
fn un_payload_non_utf8_e_un_errore() {
    let corpo = [0xFF_u8, 0xFE];
    let mut byte = u32::try_from(corpo.len())
        .expect("corto")
        .to_be_bytes()
        .to_vec();
    byte.extend_from_slice(&corpo);
    assert!(rifiuto(&byte).contains("non UTF-8"));
}

#[test]
fn un_json_malformato_e_un_errore() {
    let testo = r#"{"protocol_version":1,"tipo":"annulla","corpo":{"motivo":"x""#;
    assert!(rifiuto(&incornicia(testo)).contains("malformato"));
}

/// Impacchetta un testo con il prefisso corretto: serve per i casi in cui a
/// essere sbagliato e' il **contenuto**, non la cornice.
fn incornicia(testo: &str) -> Vec<u8> {
    let mut byte = u32::try_from(testo.len())
        .expect("i casi di prova stanno in u32")
        .to_be_bytes()
        .to_vec();
    byte.extend_from_slice(testo.as_bytes());
    byte
}

#[test]
fn le_chiavi_duplicate_sono_un_errore() {
    // Senza il controllo, «vince l'ultima»: due frame diversi diventerebbero
    // lo stesso messaggio, e nessuno dei due capi saprebbe quale.
    let involucro =
        r#"{"protocol_version":1,"protocol_version":1,"tipo":"annulla","corpo":{"motivo":"x"}}"#;
    assert!(rifiuto(&incornicia(involucro)).contains("chiave JSON duplicata"));

    // Anche in fondo, dentro il corpo.
    let corpo = r#"{"protocol_version":1,"tipo":"annulla","corpo":{"motivo":"x","motivo":"y"}}"#;
    assert!(rifiuto(&incornicia(corpo)).contains("chiave JSON duplicata"));

    // E dentro il piano, che il protocollo non interpreta ma non per questo
    // lascia ambiguo.
    let piano = concat!(
        r#"{"protocol_version":1,"tipo":"incarico","corpo":{"#,
        r#""piano_canonico":{"a":1,"a":2},"plan_hash_atteso":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","#,
        r#""ingressi":[],"artefatto_temporaneo":"/t/o"}}"#,
    );
    assert!(rifiuto(&incornicia(piano)).contains("chiave JSON duplicata"));
}

#[test]
fn un_campo_sconosciuto_e_un_errore_a_ogni_livello() {
    let casi = [
        // nell'involucro
        r#"{"protocol_version":1,"tipo":"annulla","corpo":{"motivo":"x"},"extra":1}"#,
        // nel corpo
        r#"{"protocol_version":1,"tipo":"annulla","corpo":{"motivo":"x","extra":1}}"#,
        // annidato
        concat!(
            r#"{"protocol_version":1,"tipo":"incarico","corpo":{"#,
            r#""piano_canonico":{},"plan_hash_atteso":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","#,
            r#""ingressi":[{"nome":"i","percorso":"/p","formato":"file","#,
            r#""contract_fingerprint_atteso":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","extra":1}],"#,
            r#""artefatto_temporaneo":"/t/o"}}"#,
        ),
    ];
    for testo in casi {
        let messaggio = rifiuto(&incornicia(testo));
        assert!(
            messaggio.contains("forma o tipo non conformi"),
            "campo ignoto accettato in «{testo}»: {messaggio}"
        );
    }
}

/// I conteggi del `Successo` sono **obbligatori**, e ogni loro campo lo e'.
///
/// E' il termine di paragone del passo 8 della verifica: senza, quel passo
/// confronterebbe cio' che osserva con niente. Renderli facoltativi renderebbe
/// facoltativo il passo, e lascerebbe la sequenza normativa a citare un dato
/// che non viaggia.
#[test]
fn i_conteggi_del_successo_sono_obbligatori() {
    let digest = r#""digest_artefatto":{"algoritmo":"sha256","valore":"f"}"#;
    let mancanti = [
        // Nessun blocco `conteggi`.
        format!(r#"{{"esito":"successo",{digest}}}"#),
        // Blocco presente ma vuoto.
        format!(r#"{{"esito":"successo",{digest},"conteggi":{{}}}}"#),
        // Solo le righe.
        format!(r#"{{"esito":"successo",{digest},"conteggi":{{"righe":1}}}}"#),
        // Solo i batch.
        format!(r#"{{"esito":"successo",{digest},"conteggi":{{"batch":1}}}}"#),
    ];
    for corpo in mancanti {
        let messaggio = rifiuto(&incornicia(&payload("esito", &corpo)));
        assert!(
            messaggio.contains("forma o tipo non conformi"),
            "conteggi incompleti accettati in «{corpo}»: {messaggio}"
        );
    }

    // Un campo estraneo dentro i conteggi: la struttura e' chiusa anche li'.
    let corpo =
        format!(r#"{{"esito":"successo",{digest},"conteggi":{{"righe":1,"batch":1,"nodi":1}}}}"#);
    let messaggio = rifiuto(&incornicia(&payload("esito", &corpo)));
    assert!(
        messaggio.contains("forma o tipo non conformi"),
        "campo estraneo accettato nei conteggi: {messaggio}"
    );

    // E i valori fuori dominio: sono `u64`.
    for valore in ["-1", "1.5", "18446744073709551616", "null", r#""molte""#] {
        let corpo =
            format!(r#"{{"esito":"successo",{digest},"conteggi":{{"righe":{valore},"batch":0}}}}"#);
        let messaggio = rifiuto(&incornicia(&payload("esito", &corpo)));
        assert!(
            messaggio.contains("malformato"),
            "valore `{valore}` accettato come conteggio: {messaggio}"
        );
    }
}

/// `nodi_completati` **non** entra nei conteggi del `Successo`.
///
/// `Progresso` lo porta, e la simmetria inviterebbe ad aggiungerlo anche qui.
/// Non si puo': rileggendo un file Arrow IPC si osservano righe e batch, non
/// quanti nodi del piano li hanno prodotti. Un numero che il verificatore non
/// puo' confrontare gli chiederebbe di crederci, ed e' cio' che il passo 8
/// esiste per non fare.
#[test]
fn i_conteggi_del_successo_non_portano_i_nodi() {
    let corpo = concat!(
        r#"{"esito":"successo","digest_artefatto":{"algoritmo":"sha256","valore":"f"},"#,
        r#""conteggi":{"righe":1,"batch":1,"nodi_completati":1}}"#,
    );
    let messaggio = rifiuto(&incornicia(&payload("esito", corpo)));
    assert!(
        messaggio.contains("forma o tipo non conformi"),
        "`nodi_completati` accettato fra i conteggi del successo: {messaggio}"
    );
}

#[test]
fn un_campo_mancante_e_un_errore() {
    let testo = r#"{"protocol_version":1,"tipo":"progresso","corpo":{"righe":1,"batch":2}}"#;
    let messaggio = rifiuto(&incornicia(testo));
    assert!(
        messaggio.contains("forma o tipo non conformi"),
        "campo mancante accettato: {messaggio}"
    );
}

/// La versione ricevuta non finisce nel messaggio.
///
/// E' un `u16` che sceglie chi scrive il frame: un numero arbitrario nel
/// rifiuto e' un numero arbitrario nel log di chi indaga. Cio' che il rifiuto
/// puo' dire e' **la nostra** versione, che e' una costante di questo binario.
///
/// La sentinella e' valida come `u16` e sconosciuta come versione: sta nel
/// dominio del tipo, quindi arriva fino al confronto invece di essere respinta
/// prima come numero malformato.
#[test]
fn la_versione_ricevuta_non_compare_nel_rifiuto() {
    const SENTINELLA: u16 = 51_423;

    let testo =
        format!(r#"{{"protocol_version":{SENTINELLA},"tipo":"annulla","corpo":{{"motivo":"x"}}}}"#);
    let messaggio = rifiuto(&incornicia(&testo));
    assert!(
        !messaggio.contains(&SENTINELLA.to_string()),
        "il rifiuto ha copiato la versione ricevuta: {messaggio}"
    );
    assert!(
        messaggio.contains("non riconosciuta"),
        "rifiuto inatteso: {messaggio}"
    );
}

#[test]
fn una_versione_sconosciuta_e_un_errore() {
    for versione in [0_u16, 2, u16::MAX] {
        let testo = format!(
            r#"{{"protocol_version":{versione},"tipo":"annulla","corpo":{{"motivo":"x"}}}}"#
        );
        let messaggio = rifiuto(&incornicia(&testo));
        assert!(
            messaggio.contains("non riconosciuta"),
            "versione {versione} accettata: {messaggio}"
        );
    }
}

#[test]
fn un_tipo_sconosciuto_e_un_errore() {
    let testo = r#"{"protocol_version":1,"tipo":"saluta","corpo":{"motivo":"x"}}"#;
    let messaggio = rifiuto(&incornicia(testo));
    assert!(
        messaggio.contains("forma o tipo non conformi"),
        "tipo ignoto accettato: {messaggio}"
    );
}

/// Il tipo dichiarato **governa** la lettura del corpo.
///
/// Non «si legge il corpo e poi si confronta»: il corpo si legge *in quel
/// tipo*, quindi un `Progresso` etichettato `saluto` non ha nemmeno una
/// struttura in cui atterrare.
#[test]
fn un_corpo_che_non_corrisponde_al_tipo_e_un_errore() {
    let coppie = [
        ("saluto", CORPO_PROGRESSO),
        ("progresso", CORPO_ANNULLA),
        ("annulla", CORPO_ESITO_PANIC),
        ("risposta", CORPO_SALUTO),
        ("incarico", CORPO_RISPOSTA),
        ("esito", CORPO_INCARICO),
    ];
    for (tipo, corpo) in coppie {
        let testo = payload(tipo, corpo);
        let messaggio = rifiuto(&incornicia(&testo));
        assert!(
            messaggio.contains(&format!("corpo di `{tipo}` malformato")),
            "corpo estraneo accettato sotto `{tipo}`: {messaggio}"
        );
    }
}

#[test]
fn un_enum_fuori_dominio_e_un_errore() {
    let casi = [
        // categoria inesistente
        (
            "esito",
            concat!(
                r#"{"esito":"errore","errore":{"categoria":"boh","fase":"read","#,
                r#""effetto":"none","retry":{"kind":"never"},"messaggio":"m","#,
                r#""nodo":null,"operazione":null,"execution_id":null,"diagnostica":null}}"#,
            ),
        ),
        // formato d'ingresso inesistente
        (
            "incarico",
            concat!(
                r#"{"piano_canonico":{},"plan_hash_atteso":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","#,
                r#""ingressi":[{"nome":"i","percorso":"/p","formato":"socket","#,
                r#""contract_fingerprint_atteso":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}],"artefatto_temporaneo":"/t"}"#,
            ),
        ),
        // forma di panico inesistente
        ("esito", r#"{"esito":"panic","forma":"testuale"}"#),
        // esito inesistente
        ("esito", r#"{"esito":"forse","forma":"statico"}"#),
    ];
    for (tipo, corpo) in casi {
        let messaggio = rifiuto(&incornicia(&payload(tipo, corpo)));
        assert!(
            messaggio.contains("forma o tipo non conformi"),
            "valore fuori dominio accettato in «{corpo}»: {messaggio}"
        );
    }
}

/// Anche gli enum **con tag interno** rifiutano i campi ignoti.
///
/// Vale la pena provarlo a parte: `deny_unknown_fields` su un enum con tag
/// interno non copre le varianti *unitarie*, che `serde` riconosce dal tag e
/// poi accetta con qualunque contorno. Entrambi gli enum con tag del
/// protocollo — `retry` ed `esito` — sono scritti con sole varianti di
/// struttura proprio per questo, e qui si verifica che la scrittura tenga.
#[test]
fn gli_enum_con_tag_interno_rifiutano_i_campi_ignoti() {
    // --- `esito`: un caso per ogni nome **generato**.
    //
    // La tabella qui sotto e' scritta a mano, ma la sua completezza non lo
    // e': si itera su `NOMI`, e un nome senza caso fa fallire il test. Una
    // variante nuova non puo' quindi restare fuori dalle prove.
    let corpi_esito = [
        ("panic", r#"{"esito":"panic","forma":"statico","extra":1}"#),
        (
            "successo",
            concat!(
                r#"{"esito":"successo","digest_artefatto":{"algoritmo":"sha256","valore":"f"},"#,
                r#""conteggi":{"righe":0,"batch":0},"extra":1}"#,
            ),
        ),
        (
            "errore",
            concat!(
                r#"{"esito":"errore","errore":{"categoria":"schema","fase":"read","#,
                r#""effetto":"none","retry":{"kind":"never"},"messaggio":"m","#,
                r#""nodo":null,"operazione":null,"execution_id":null,"diagnostica":null},"#,
                r#""extra":1}"#,
            ),
        ),
    ];
    for nome in EsitoWorkerSulFilo::NOMI {
        let corpo = corpi_esito
            .iter()
            .find(|(variante, _)| variante == nome)
            .unwrap_or_else(|| panic!("nessun caso per la variante `{nome}` di `esito`"))
            .1;
        let messaggio = rifiuto(&incornicia(&payload("esito", corpo)));
        assert!(
            messaggio.contains("forma o tipo non conformi"),
            "campo ignoto accettato in «{corpo}»: {messaggio}"
        );
    }

    // --- `retry`: i casi si **derivano** dai campioni generati.
    //
    // Non c'e' nessuna tabella da tenere allineata: il campione lo costruisce
    // la macro insieme alla variante, e il caso ostile e' quel campione con
    // un campo in piu'. Una disposizione nuova entra in `TUTTE` da sola.
    for (disposizione, nome) in RetrySulFilo::TUTTE {
        let base = serde_json::to_string(disposizione).expect("campione serializzabile");
        assert!(
            base.contains(&format!(r#""kind":"{nome}""#)),
            "`{nome}` non compare nel proprio campione: {base}"
        );
        let ostile = format!(
            "{},{}",
            base.strip_suffix('}')
                .expect("un oggetto JSON finisce con }"),
            r#""estraneo":1}"#
        );
        let corpo = format!(
            concat!(
                r#"{{"esito":"errore","errore":{{"categoria":"schema","fase":"read","#,
                r#""effetto":"none","retry":{},"messaggio":"m","#,
                r#""nodo":null,"operazione":null,"execution_id":null,"diagnostica":null}}}}"#,
            ),
            ostile
        );
        let messaggio = rifiuto(&incornicia(&payload("esito", &corpo)));
        assert!(
            messaggio.contains("forma o tipo non conformi"),
            "campo ignoto accettato su «{ostile}»: {messaggio}"
        );
    }
}

/// Le quattro disposizioni senza ritardo restano **senza campi** sul filo.
///
/// Scriverle `Never {}` invece di `Never` cambia la deserializzazione, non la
/// forma: se cambiasse anche la forma sarebbe una modifica del protocollo
/// travestita da correzione.
#[test]
fn le_disposizioni_senza_ritardo_non_hanno_campi_sul_filo() {
    let attese = [
        (RetrySulFilo::Never {}, r#"{"kind":"never"}"#),
        (RetrySulFilo::Safe {}, r#"{"kind":"safe"}"#),
        (
            RetrySulFilo::RequiresIdempotencyKey {},
            r#"{"kind":"requires_idempotency_key"}"#,
        ),
        (
            RetrySulFilo::RequiresRecovery {},
            r#"{"kind":"requires_recovery"}"#,
        ),
        (
            RetrySulFilo::After { delay_ms: 250 },
            r#"{"kind":"after","delay_ms":250}"#,
        ),
    ];
    for (valore, atteso) in attese {
        let prodotto = serde_json::to_string(&valore).expect("serializzabile");
        assert_eq!(prodotto, atteso, "forma cambiata per {valore:?}");
    }
}

/// `delay_ms` esiste **solo** con `after`.
///
/// Un ritardo su `never` direbbe al chiamante di riprovare piu' tardi senza
/// che nessuna disposizione glielo abbia concesso.
#[test]
fn un_ritardo_su_una_disposizione_che_non_lo_prevede_e_un_errore() {
    let corpo = concat!(
        r#"{"esito":"errore","errore":{"categoria":"schema","fase":"read","#,
        r#""effetto":"none","retry":{"kind":"never","delay_ms":10},"messaggio":"m","#,
        r#""nodo":null,"operazione":null,"execution_id":null,"diagnostica":null}}"#,
    );
    let messaggio = rifiuto(&incornicia(&payload("esito", corpo)));
    assert!(
        messaggio.contains("forma o tipo non conformi"),
        "ritardo accettato su `never`: {messaggio}"
    );
}

#[test]
fn un_numero_fuori_dominio_e_un_errore() {
    let casi = [
        // negativo dove il tipo e' senza segno
        r#"{"righe":-1,"batch":0,"nodi_completati":0}"#,
        // oltre u64
        r#"{"righe":18446744073709551616,"batch":0,"nodi_completati":0}"#,
        // frazionario dove il tipo e' intero
        r#"{"righe":1.5,"batch":0,"nodi_completati":0}"#,
    ];
    for corpo in casi {
        let messaggio = rifiuto(&incornicia(&payload("progresso", corpo)));
        assert!(
            messaggio.contains("malformato"),
            "numero fuori dominio accettato in «{corpo}»: {messaggio}"
        );
    }

    // E la versione, che e' `u16`.
    let testo = r#"{"protocol_version":65536,"tipo":"annulla","corpo":{"motivo":"x"}}"#;
    assert!(rifiuto(&incornicia(testo)).contains("malformato"));
}

// ---------------------------------------------------------------------------
// I tetti per campo, in entrambi i versi
// ---------------------------------------------------------------------------

fn ripeti(n: usize) -> String {
    "a".repeat(n)
}

/// Il carattere che JSON **deve** espandere: `U+0001`, un byte decodificato,
/// sei byte codificati.
///
/// Serve alle fixture massime. Con `"a"` il frame massimo resterebbe a un
/// sesto della taglia che il tetto gli concede, quindi la prova dei massimi
/// non toccherebbe mai `ESPANSIONE_ESCAPE`: una mutazione a `1` la lascerebbe
/// verde pur rendendo non codificabile un frame **strutturalmente ammesso**
/// che portasse caratteri di controllo — e i campi del protocollo ammettono
/// qualunque stringa.
const ESPANDIBILE: &str = "\u{1}";

/// `n` byte decodificati che ne diventano `n * ESPANSIONE_ESCAPE` codificati.
fn ripeti_espandendo(n: usize) -> String {
    ESPANDIBILE.repeat(n)
}

/// Un ingresso minimo, per i casi in cui a essere sotto esame e' un altro
/// campo.
fn ingresso_minimo() -> DescrittoreIngresso {
    DescrittoreIngresso {
        nome: "i".to_owned(),
        percorso: "/p".to_owned(),
        formato: FormatoIngresso::File,
        contract_fingerprint_atteso: digest(&"e".repeat(MAX_DIGEST_BYTES)),
    }
}

/// Un piano finto di **esattamente** `byte` byte.
///
/// Una stringa JSON, non un array: l'array ha una granularita' di due byte e
/// non permette di stare sul confine esatto, che qui e' tutto cio' che conta.
fn piano_di(byte: usize) -> String {
    let mut testo = String::with_capacity(byte);
    testo.push('"');
    testo.push_str(&ripeti(byte - 2));
    testo.push('"');
    testo
}

/// Un tetto che valesse da un lato solo renderebbe il protocollo asimmetrico
/// proprio dove i due capi devono concordare: un mittente potrebbe emettere
/// cio' che il destinatario rifiuta.
#[test]
fn i_tetti_valgono_in_scrittura_e_in_lettura() {
    let al_tetto = codifica(&annulla_con(ripeti(MAX_MOTIVO_BYTES))).expect("al tetto si codifica");
    decodifica(&al_tetto).expect("al tetto si decodifica");

    let scritto = codifica(&annulla_con(ripeti(MAX_MOTIVO_BYTES + 1)));
    assert!(
        scritto.is_err(),
        "il writer ha emesso un motivo oltre il tetto"
    );

    // E il decoder lo rifiuta anche se qualcun altro riesce a produrlo.
    let testo = payload(
        "annulla",
        &format!(r#"{{"motivo":"{}"}}"#, ripeti(MAX_MOTIVO_BYTES + 1)),
    );
    let messaggio = rifiuto(&incornicia(&testo));
    assert!(
        messaggio.contains("oltre il tetto"),
        "motivo oltre il tetto accettato in lettura: {messaggio}"
    );
}

#[test]
fn il_piano_oltre_il_tetto_e_rifiutato_in_entrambi_i_versi() {
    let uno = ingresso_minimo();
    let al_tetto = piano_di(MAX_PIANO_CANONICO_BYTES);
    assert_eq!(al_tetto.len(), MAX_PIANO_CANONICO_BYTES);
    codifica(&incarico_con(grezzo(&al_tetto), vec![uno.clone()]))
        .expect("un piano al tetto si codifica");

    let oltre = piano_di(MAX_PIANO_CANONICO_BYTES + 1);
    let messaggio = codifica(&incarico_con(grezzo(&oltre), vec![uno]))
        .expect_err("un piano oltre il tetto non si codifica")
        .to_string();
    assert!(
        messaggio.contains("`piano_canonico` oltre il tetto"),
        "motivo inatteso: {messaggio}"
    );
}

/// Un campo limitato: come portarlo a una lunghezza e quale tetto ha.
type CasoTetto = (&'static str, usize, Box<dyn Fn(usize) -> Frame>);

/// I tre costruttori di caso, uno per corpo che ne ha bisogno.
///
/// Ognuno ripete lo stesso gesto — parti dal frame di prova, entra nel corpo,
/// porta **un** campo a lunghezza `n` — e cio' che cambia e' la sola riga che
/// nomina il campo. Il resto e' impalcatura: tenerne una copia per caso
/// significa che una svista nell'impalcatura si legge tredici volte.
///
/// L'`if let` che non trova la variante attesa lascia il frame intatto, e la
/// riga fallisce a `tetto + 1` perche' il frame resta valido. E' il controllo
/// sulla tabella descritto in `ogni_campo_limitato_e_provato_sul_proprio_confine`.
fn caso_su_saluto(applica: impl Fn(&mut Saluto, usize) + 'static) -> Box<dyn Fn(usize) -> Frame> {
    Box::new(move |n| {
        let mut frame = saluto();
        if let Corpo::Saluto(dentro) = frame.corpo_mutabile() {
            applica(dentro, n);
        }
        frame
    })
}

fn caso_su_risposta(
    applica: impl Fn(&mut Risposta, usize) + 'static,
) -> Box<dyn Fn(usize) -> Frame> {
    Box::new(move |n| {
        let mut frame = risposta();
        if let Corpo::Risposta(dentro) = frame.corpo_mutabile() {
            applica(dentro, n);
        }
        frame
    })
}

fn caso_su_incarico(
    applica: impl Fn(&mut Incarico, usize) + 'static,
) -> Box<dyn Fn(usize) -> Frame> {
    Box::new(move |n| {
        let mut frame = incarico();
        if let Corpo::Incarico(dentro) = frame.corpo_mutabile() {
            applica(dentro, n);
        }
        frame
    })
}

fn casi_saluto() -> Vec<CasoTetto> {
    vec![
        (
            "artefatto.versione",
            MAX_VERSIONE_BYTES,
            caso_su_saluto(|s, n| s.artefatto.versione = ripeti(n)),
        ),
        (
            "resolver.identita",
            MAX_IDENTIFICATORE_BYTES,
            caso_su_saluto(|s, n| s.resolver.identita = ripeti(n)),
        ),
        (
            "resolver.versione",
            MAX_VERSIONE_BYTES,
            caso_su_saluto(|s, n| s.resolver.versione = ripeti(n)),
        ),
    ]
}

fn casi_risposta() -> Vec<CasoTetto> {
    vec![
        (
            "risorsa.nome",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| risposta_con_risorsa(ripeti(n), "x".to_owned(), "x".to_owned())),
        ),
        (
            "risorsa.versione",
            MAX_VERSIONE_BYTES,
            Box::new(|n| risposta_con_risorsa("x".to_owned(), ripeti(n), "x".to_owned())),
        ),
        (
            "risorsa.percorso",
            MAX_PERCORSO_BYTES,
            Box::new(|n| risposta_con_risorsa("x".to_owned(), "x".to_owned(), ripeti(n))),
        ),
        (
            "backend.nome",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| risposta_con_backend(ripeti(n), "x".to_owned(), "x".to_owned())),
        ),
        (
            "backend.versione",
            MAX_VERSIONE_BYTES,
            Box::new(|n| risposta_con_backend("x".to_owned(), ripeti(n), "x".to_owned())),
        ),
        (
            "backend.percorso",
            MAX_PERCORSO_BYTES,
            Box::new(|n| risposta_con_backend("x".to_owned(), "x".to_owned(), ripeti(n))),
        ),
        (
            "risorse",
            MAX_RISORSE,
            caso_su_risposta(|r, n| {
                r.ambiente.risorse = vec![
                    RisorsaRisolta {
                        nome: "r".to_owned(),
                        versione: "1".to_owned(),
                        percorso: "/r".to_owned(),
                    };
                    n
                ];
            }),
        ),
        (
            "backend_dinamici",
            MAX_BACKEND_DINAMICI,
            caso_su_risposta(|r, n| {
                r.ambiente.backend_dinamici = vec![
                    BackendDinamico {
                        nome: "b".to_owned(),
                        versione: "1".to_owned(),
                        percorso: "/b".to_owned(),
                    };
                    n
                ];
            }),
        ),
        (
            "capability",
            MAX_CAPABILITY,
            caso_su_risposta(|r, n| r.capability = vec!["c".to_owned(); n]),
        ),
        (
            "capability (elemento)",
            MAX_IDENTIFICATORE_BYTES,
            caso_su_risposta(|r, n| r.capability = vec![ripeti(n)]),
        ),
    ]
}

fn risposta_con_risorsa(nome: String, versione: String, percorso: String) -> Frame {
    let mut f = risposta();
    if let Corpo::Risposta(r) = f.corpo_mutabile() {
        r.ambiente.risorse = vec![RisorsaRisolta {
            nome,
            versione,
            percorso,
        }];
    }
    f
}

fn risposta_con_backend(nome: String, versione: String, percorso: String) -> Frame {
    let mut f = risposta();
    if let Corpo::Risposta(r) = f.corpo_mutabile() {
        r.ambiente.backend_dinamici = vec![BackendDinamico {
            nome,
            versione,
            percorso,
        }];
    }
    f
}

fn casi_incarico() -> Vec<CasoTetto> {
    vec![
        (
            "artefatto_temporaneo",
            MAX_PERCORSO_BYTES,
            caso_su_incarico(|i, n| i.artefatto_temporaneo = ripeti(n)),
        ),
        (
            "ingresso.nome",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| ingresso_con(ripeti(n), "/x".to_owned(), digest_massimo())),
        ),
        (
            "ingresso.percorso",
            MAX_PERCORSO_BYTES,
            Box::new(|n| ingresso_con("x".to_owned(), ripeti(n), digest_massimo())),
        ),
    ]
}

fn ingresso_con(nome: String, percorso: String, impronta: DigestSha256) -> Frame {
    incarico_con(
        grezzo(r#"{"schema_version":6}"#),
        vec![DescrittoreIngresso {
            nome,
            percorso,
            formato: FormatoIngresso::File,
            contract_fingerprint_atteso: impronta,
        }],
    )
}

fn casi_esito() -> Vec<CasoTetto> {
    vec![
        (
            "digest_artefatto.algoritmo",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| {
                esito(EsitoWorkerSulFilo::Successo {
                    digest_artefatto: DigestArtefatto {
                        algoritmo: ripeti(n),
                        valore: "f".to_owned(),
                    },
                    conteggi: ConteggiDichiarati { righe: 0, batch: 0 },
                })
            }),
        ),
        (
            "digest_artefatto.valore",
            MAX_DIGEST_BYTES,
            Box::new(|n| {
                esito(EsitoWorkerSulFilo::Successo {
                    digest_artefatto: DigestArtefatto {
                        algoritmo: "sha256".to_owned(),
                        valore: ripeti(n),
                    },
                    conteggi: ConteggiDichiarati { righe: 0, batch: 0 },
                })
            }),
        ),
        (
            "messaggio",
            MAX_MESSAGGIO_BYTES,
            Box::new(|n| errore_con(|e| e.messaggio = ripeti(n))),
        ),
        (
            "nodo",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| errore_con(|e| e.nodo = Some(ripeti(n)))),
        ),
        (
            "operazione",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| errore_con(|e| e.operazione = Some(ripeti(n)))),
        ),
        (
            "execution_id",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| errore_con(|e| e.execution_id = Some(ripeti(n)))),
        ),
        (
            "diagnostica.contract",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| diagnostica_con(|d| d.contract = ripeti(n))),
        ),
        (
            "diagnostica.scope",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| diagnostica_con(|d| d.scope = ripeti(n))),
        ),
        (
            "diagnostica.completeness",
            MAX_IDENTIFICATORE_BYTES,
            Box::new(|n| diagnostica_con(|d| d.completeness = ripeti(n))),
        ),
        (
            "diagnostica.conteggi",
            MAX_CONTEGGI_DIAGNOSTICA,
            Box::new(|n| diagnostica_con(|d| d.conteggi = vec![("k".to_owned(), 1); n])),
        ),
        (
            "chiave di conteggio",
            MAX_CHIAVE_CONTEGGIO_BYTES,
            Box::new(|n| diagnostica_con(|d| d.conteggi = vec![(ripeti(n), 1)])),
        ),
        (
            "diagnostica.esempi",
            MAX_ESEMPI_DIAGNOSTICA,
            Box::new(|n| {
                diagnostica_con(|d| {
                    d.esempi = vec![
                        EsempioDiagnostica {
                            indice: 0,
                            codice: "E".to_owned(),
                        };
                        n
                    ];
                })
            }),
        ),
        (
            "esempio.codice",
            MAX_ESEMPIO_BYTES,
            Box::new(|n| {
                diagnostica_con(|d| {
                    d.esempi = vec![EsempioDiagnostica {
                        indice: 0,
                        codice: ripeti(n),
                    }];
                })
            }),
        ),
    ]
}

fn errore_minimo() -> ErroreSulFilo {
    ErroreSulFilo {
        categoria: CategoriaSulFilo::Schema,
        fase: FaseSulFilo::Read,
        effetto: EffettoSulFilo::None,
        retry: RetrySulFilo::Never {},
        messaggio: "m".to_owned(),
        nodo: None,
        operazione: None,
        execution_id: None,
        diagnostica: None,
    }
}

fn errore_con(applica: impl FnOnce(&mut ErroreSulFilo)) -> Frame {
    let mut dentro = errore_minimo();
    applica(&mut dentro);
    esito(EsitoWorkerSulFilo::Errore {
        errore: Box::new(dentro),
    })
}

fn diagnostica_con(applica: impl FnOnce(&mut DiagnosticaSulFilo)) -> Frame {
    let mut diagnostica = DiagnosticaSulFilo {
        contract: "c".to_owned(),
        scope: "s".to_owned(),
        completeness: "exact".to_owned(),
        observed_total: 0,
        conteggi: Vec::new(),
        esempi: Vec::new(),
        esempi_troncati: false,
    };
    applica(&mut diagnostica);
    errore_con(|e| e.diagnostica = Some(diagnostica))
}

/// I quattro **digest** non compaiono in questa tabella, e non per
/// dimenticanza: non hanno un tetto da provare. Un digest ha una **forma** —
/// 64 esadecimali minuscoli — e la forma e' del tipo, quindi «al tetto» e
/// «oltre il tetto» non sono stati che si possano costruire. Le forme
/// rifiutate stanno nelle prove del tipo, una per una.
///
/// **Ogni** campo limitato, provato sul proprio confine.
///
/// Senza questo, i tetti sarebbero applicati ma quasi mai esercitati: una
/// costante sbagliata in uno dei `limita` — il tetto del percorso su un
/// identificatore, per dire — passerebbe inosservata, perché nessun test
/// arriverebbe a quel confine.
///
/// Il caso a `tetto + 1` fa anche da controllo sulla tabella stessa: se un
/// costruttore non modificasse davvero il campo che dichiara, il frame
/// resterebbe valido e la riga fallirebbe.
#[test]
fn ogni_campo_limitato_e_provato_sul_proprio_confine() {
    let tutti = casi_saluto()
        .into_iter()
        .chain(casi_risposta())
        .chain(casi_incarico())
        .chain(casi_esito());
    let mut provati = 0_usize;
    for (nome, tetto, costruisci) in tutti {
        let al_tetto = costruisci(tetto);
        let byte = codifica(&al_tetto)
            .unwrap_or_else(|errore| panic!("«{nome}» al tetto non si codifica: {errore}"));
        decodifica(&byte)
            .unwrap_or_else(|errore| panic!("«{nome}» al tetto non si rilegge: {errore}"));

        let messaggio = codifica(&costruisci(tetto + 1))
            .map_or_else(|errore| errore.to_string(), |_| String::from("ACCETTATO"));
        // Due forme di rifiuto, non una: sui campi ordinari decide il tetto,
        // sui digest decide la **forma canonica**, che e' piu' stretta — 64
        // caratteri esatti, non «al piu' 64». Cio' che il test pretende in
        // entrambi i casi e' che il rifiuto nomini il campo: un tetto
        // applicato al campo sbagliato passerebbe altrimenti inosservato.
        assert!(
            messaggio.contains("oltre il tetto") && messaggio.contains(nome),
            "«{nome}» a tetto+1 non e' stato respinto sul proprio nome: {messaggio}"
        );
        provati += 1;
    }
    // Se qualcuno aggiunge un `limita` senza aggiungere la riga, questo numero
    // resta indietro ed e' il segnale che manca una prova. Sono quattro meno
    // di prima perche' i quattro digest hanno smesso di avere un tetto: hanno
    // un tipo.
    assert_eq!(provati, 29, "la tabella dei confini non copre piu' tutto");
}

#[test]
fn i_tetti_di_cardinalita_valgono_in_scrittura() {
    let piano = grezzo(r#"{"schema_version":6}"#);
    let al_tetto = vec![ingresso_minimo(); MAX_INGRESSI];
    codifica(&incarico_con(piano.clone(), al_tetto)).expect("al tetto si codifica");

    let oltre = vec![ingresso_minimo(); MAX_INGRESSI + 1];
    assert!(
        codifica(&incarico_con(piano, oltre)).is_err(),
        "ingressi oltre il tetto emessi"
    );
}

// ---------------------------------------------------------------------------
// Il tetto del frame, provato sui sei massimi veri
// ---------------------------------------------------------------------------

fn ambiente_massimo() -> Ambiente {
    Ambiente {
        digest_insieme: digest_massimo(),
        acquisizione_dinamica: true,
        risorse: vec![
            RisorsaRisolta {
                nome: ripeti_espandendo(MAX_IDENTIFICATORE_BYTES),
                versione: ripeti_espandendo(MAX_VERSIONE_BYTES),
                percorso: ripeti_espandendo(MAX_PERCORSO_BYTES),
            };
            MAX_RISORSE
        ],
        backend_dinamici: vec![
            BackendDinamico {
                nome: ripeti_espandendo(MAX_IDENTIFICATORE_BYTES),
                versione: ripeti_espandendo(MAX_VERSIONE_BYTES),
                percorso: ripeti_espandendo(MAX_PERCORSO_BYTES),
            };
            MAX_BACKEND_DINAMICI
        ],
    }
}

/// Il digest piu' lungo che la forma canonica ammette: 64 esadecimali.
fn digest_massimo() -> DigestSha256 {
    digest(&"f".repeat(MAX_DIGEST_BYTES))
}

fn piano_al_tetto() -> Box<RawValue> {
    grezzo(&piano_di(MAX_PIANO_CANONICO_BYTES))
}

/// I sei massimi **veri**, prodotti dal codificatore.
fn massimi() -> Vec<(&'static str, Frame)> {
    let artefatto = IdentitaArtefatto {
        digest: digest_massimo(),
        versione: ripeti_espandendo(MAX_VERSIONE_BYTES),
    };
    let resolver = IdentitaResolver {
        identita: ripeti_espandendo(MAX_IDENTIFICATORE_BYTES),
        versione: ripeti_espandendo(MAX_VERSIONE_BYTES),
    };
    vec![
        (
            "saluto",
            Frame::nuovo(Corpo::Saluto(Box::new(Saluto {
                artefatto: artefatto.clone(),
                resolver: resolver.clone(),
                ambiente: ambiente_massimo(),
                commit_token: token_di_prova(),
                limiti: LimitiDichiarati {
                    max_frame_bytes: u64::MAX,
                    max_piano_canonico_bytes: u64::MAX,
                    max_messaggi_verso_worker: u64::MAX,
                    max_messaggi_verso_supervisore: u64::MAX,
                },
            }))),
        ),
        (
            "incarico",
            Frame::nuovo(Corpo::Incarico(Box::new(Incarico {
                piano_canonico: piano_al_tetto(),
                plan_hash_atteso: digest_massimo(),
                ingressi: vec![
                    DescrittoreIngresso {
                        nome: ripeti_espandendo(MAX_IDENTIFICATORE_BYTES),
                        percorso: ripeti_espandendo(MAX_PERCORSO_BYTES),
                        formato: FormatoIngresso::Stream,
                        contract_fingerprint_atteso: digest_massimo(),
                    };
                    MAX_INGRESSI
                ],
                artefatto_temporaneo: ripeti_espandendo(MAX_PERCORSO_BYTES),
            }))),
        ),
        (
            "annulla",
            Frame::nuovo(Corpo::Annulla(Annulla {
                motivo: ripeti_espandendo(MAX_MOTIVO_BYTES),
            })),
        ),
        (
            "risposta",
            Frame::nuovo(Corpo::Risposta(Box::new(Risposta {
                artefatto,
                resolver,
                ambiente: ambiente_massimo(),
                capability: vec![ripeti_espandendo(MAX_IDENTIFICATORE_BYTES); MAX_CAPABILITY],
            }))),
        ),
        (
            "progresso",
            Frame::nuovo(Corpo::Progresso(Progresso {
                righe: u64::MAX,
                batch: u64::MAX,
                nodi_completati: u64::MAX,
            })),
        ),
        ("esito", esito_massimo()),
    ]
}

/// L'esito piu' grande: ogni campo al proprio tetto.
fn esito_massimo() -> Frame {
    esito(EsitoWorkerSulFilo::Errore {
        errore: Box::new(ErroreSulFilo {
            categoria: CategoriaSulFilo::UnattributedMemoryPressure,
            fase: FaseSulFilo::Finalize,
            effetto: EffettoSulFilo::Unknown,
            retry: RetrySulFilo::After { delay_ms: u64::MAX },
            messaggio: ripeti_espandendo(MAX_MESSAGGIO_BYTES),
            nodo: Some(ripeti_espandendo(MAX_IDENTIFICATORE_BYTES)),
            operazione: Some(ripeti_espandendo(MAX_IDENTIFICATORE_BYTES)),
            execution_id: Some(ripeti_espandendo(MAX_IDENTIFICATORE_BYTES)),
            diagnostica: Some(DiagnosticaSulFilo {
                contract: ripeti_espandendo(MAX_IDENTIFICATORE_BYTES),
                scope: ripeti_espandendo(MAX_IDENTIFICATORE_BYTES),
                completeness: ripeti_espandendo(MAX_IDENTIFICATORE_BYTES),
                observed_total: u64::MAX,
                conteggi: vec![
                    (ripeti_espandendo(MAX_CHIAVE_CONTEGGIO_BYTES), u64::MAX);
                    MAX_CONTEGGI_DIAGNOSTICA
                ],
                esempi: vec![
                    EsempioDiagnostica {
                        indice: u64::MAX,
                        codice: ripeti_espandendo(MAX_ESEMPIO_BYTES),
                    };
                    MAX_ESEMPI_DIAGNOSTICA
                ],
                esempi_troncati: true,
            }),
        }),
    })
}

/// Il tetto del frame **contiene** i sei massimi, e l'`Incarico` e' il piu'
/// grande.
///
/// «Il piu' grande» non e' un'intuizione da confermare: se lo fosse un altro,
/// la derivazione del tetto starebbe misurando il messaggio sbagliato.
#[test]
fn i_sei_massimi_stanno_sotto_il_tetto_e_l_incarico_e_il_maggiore() {
    let mut misure: Vec<(&'static str, usize)> = Vec::new();
    for (nome, frame) in massimi() {
        let byte = codifica(&frame)
            .unwrap_or_else(|errore| panic!("il massimo di «{nome}» non si codifica: {errore}"));
        let payload = byte.len() - BYTE_PREFISSO;
        assert!(
            payload <= MAX_PROTOCOL_FRAME_BYTES,
            "il massimo di «{nome}» ({payload} byte) supera il tetto \
             ({MAX_PROTOCOL_FRAME_BYTES})"
        );
        // E si rilegge: un massimo che il decoder rifiuta sarebbe un tetto
        // dichiarato da un lato solo.
        decodifica(&byte)
            .unwrap_or_else(|errore| panic!("il massimo di «{nome}» non si rilegge: {errore}"));
        misure.push((nome, payload));
    }

    let Some(&(maggiore, _)) = misure.iter().max_by_key(|(_, byte)| *byte) else {
        panic!("nessuna misura");
    };
    assert_eq!(
        maggiore, "incarico",
        "il messaggio piu' grande e' «{maggiore}», non l'incarico: la \
         derivazione del tetto misura il messaggio sbagliato ({misure:?})"
    );
}

/// Le invarianti che il target fuzz pretende, applicate qui dalla suite
/// ordinaria.
///
/// Non sono riscritte: chiamano `interni::verifica_giro_del_frame`, la stessa
/// funzione che invoca il target. Riscriverle creerebbe due definizioni della
/// stessa promessa, e la copia nei test resterebbe verde mentre quella del
/// fuzzer sbaglia.
///
/// I casi sono **non canonici** di proposito — chiavi in ordine diverso,
/// spazi, un piano grezzo con la sua formattazione — perche' e' li' che
/// un'invariante di idempotenza si rompe.
#[test]
fn le_invarianti_del_target_fuzz_reggono() {
    let non_canonici = [
        // ordine delle chiavi invertito nell'involucro
        r#"{"corpo":{"motivo":"x"},"tipo":"annulla","protocol_version":1}"#.to_owned(),
        // spazi ovunque
        r#" { "protocol_version" : 1 , "tipo" : "annulla" , "corpo" : { "motivo" : "x" } } "#
            .to_owned(),
        // ordine invertito nel corpo
        payload("progresso", r#"{"nodi_completati":3,"batch":2,"righe":10}"#),
        // il piano grezzo conserva la sua forma, spazi compresi
        payload(
            "incarico",
            concat!(
                r#"{"piano_canonico":{ "a" : [ 1 , 2 ] },"plan_hash_atteso":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","#,
                r#""ingressi":[],"artefatto_temporaneo":"/t"}"#,
            ),
        ),
    ];
    for testo in non_canonici {
        let byte = incornicia(&testo);
        // Prima che valga come prova, il caso deve essere accettato: se fosse
        // rifiutato, il verdetto sarebbe `Ok` per il motivo sbagliato.
        decodifica(&byte)
            .unwrap_or_else(|errore| panic!("il caso «{testo}» va accettato: {errore}"));
        crate::interni::verifica_giro_del_frame(&byte)
            .unwrap_or_else(|motivo| panic!("invariante rotta su «{testo}»: {motivo}"));
    }

    // E su byte che non sono un frame il verdetto e' `Ok`: non essere un
    // frame non e' un guasto, ed e' la gran parte di cio' che produce un
    // fuzzer.
    for spazzatura in [
        b"".as_slice(),
        &[0x00],
        &[0xFF, 0xFF, 0xFF, 0xFF],
        b"non un frame",
    ] {
        crate::interni::verifica_giro_del_frame(spazzatura)
            .expect("byte che non sono un frame non sono un guasto");
    }
}

// ---------------------------------------------------------------------------
// Sentinelle: cosa NON deve comparire
// ---------------------------------------------------------------------------

const SENTINELLA: &str = "VALORE_DI_CELLA_RISERVATO";

/// Il contenuto del payload non entra negli errori del decoder.
///
/// Un errore che cita i byte che l'ha causato e' un errore che copia dati in
/// un log: qui la cornice deve fallire **senza citare**.
#[test]
fn il_contenuto_non_entra_negli_errori_del_decoder() {
    let sospetti = [
        // JSON malformato con la sentinella dentro
        format!(r#"{{"protocol_version":1,"tipo":"annulla","corpo":{{"motivo":"{SENTINELLA}""#),
        // campo sconosciuto il cui *valore* e' la sentinella
        payload(
            "annulla",
            &format!(r#"{{"motivo":"x","extra":"{SENTINELLA}"}}"#),
        ),
        // tipo ignoto con la sentinella nel corpo
        payload(
            "annulla",
            &format!(r#"{{"motivo":"{SENTINELLA}","altro":1}}"#),
        ),
        // valore oltre il tetto, il cui contenuto e' la sentinella ripetuta
        payload(
            "annulla",
            &format!(r#"{{"motivo":"{}"}}"#, SENTINELLA.repeat(200)),
        ),
    ];
    for testo in sospetti {
        let messaggio = rifiuto(&incornicia(&testo));
        assert!(
            !messaggio.contains(SENTINELLA),
            "il decoder ha copiato il contenuto nell'errore: {messaggio}"
        );
    }
}

/// Il contenuto di un panico non ha un posto dove entrare.
///
/// Non e' una sanificazione da ricordarsi di applicare: [`FormaPanicSulFilo`]
/// e' un enum chiuso, quindi non c'e' nessun campo in cui un messaggio di
/// panico possa finire. Il test lo verifica dal lato del filo.
#[test]
fn il_contenuto_di_un_panico_non_ha_dove_entrare() {
    let byte = codifica(&esito_panic()).expect("codifica");
    let testo = String::from_utf8(byte).expect("il frame e' UTF-8");
    assert!(testo.contains(r#""forma":"statico""#));
    assert!(!testo.contains(SENTINELLA));

    // E dal filo non si puo' introdurre: la forma e' chiusa.
    for candidato in [SENTINELLA, "", "Statico", "statico ", "dinamico\u{0}"] {
        let corpo = format!(
            r#"{{"esito":"panic","forma":{}}}"#,
            serde_json::to_string(candidato).expect("stringa serializzabile")
        );
        let messaggio = rifiuto(&incornicia(&payload("esito", &corpo)));
        assert!(
            messaggio.contains("forma o tipo non conformi"),
            "forma di panico «{candidato}» accettata: {messaggio}"
        );
    }
}

/// La sentinella non compare nemmeno negli errori del **writer**.
#[test]
fn il_contenuto_non_entra_negli_errori_del_writer() {
    let mut frame = annulla();
    let Corpo::Annulla(dentro) = frame.corpo_mutabile() else {
        panic!("costruito il caso sbagliato");
    };
    dentro.motivo = SENTINELLA.repeat(MAX_MOTIVO_BYTES);
    let messaggio = codifica(&frame)
        .expect_err("oltre il tetto non si codifica")
        .to_string();
    assert!(
        !messaggio.contains(SENTINELLA),
        "il writer ha copiato il contenuto nell'errore: {messaggio}"
    );
}

// ---------------------------------------------------------------------------
// Direzione
// ---------------------------------------------------------------------------

/// L'involucro e' derivato assumendo che il nome di tipo piu' lungo sia di
/// nove caratteri: qui si verifica che l'assunzione regga.
///
/// E' il genere di premessa che marcisce in silenzio. Un settimo tipo con un
/// nome piu' lungo non romperebbe niente di visibile — renderebbe soltanto la
/// derivazione del tetto un po' meno vera di quanto dichiara.
#[test]
fn nessun_nome_di_tipo_supera_i_nove_caratteri() {
    let mut piu_lungo = 0;
    for (_, nome) in TipoMessaggio::TUTTE {
        assert!(
            nome.len() <= 9,
            "`{nome}` e' lungo {} caratteri, non nove",
            nome.len()
        );
        piu_lungo = piu_lungo.max(nome.len());
    }
    assert_eq!(
        piu_lungo, 9,
        "nessun tipo arriva a nove: la derivazione dell'involucro e' \
         piu' larga del necessario, e vale la pena saperlo"
    );
}

/// Il tipo non puo' contraddire il corpo, perche' non e' un campo.
///
/// Come campo, il codificatore potrebbe emettere `tipo: "saluto"` con dentro
/// un `Annulla` — un frame che il decoder rifiuta. Non esiste uno stato in
/// cui i due divergano: il tipo e' una funzione del corpo.
#[test]
fn il_tipo_e_sempre_quello_del_corpo() {
    for (nome, frame, _) in casi() {
        let atteso = match frame.corpo() {
            Corpo::Saluto(_) => TipoMessaggio::Saluto,
            Corpo::Incarico(_) => TipoMessaggio::Incarico,
            Corpo::Annulla(_) => TipoMessaggio::Annulla,
            Corpo::Risposta(_) => TipoMessaggio::Risposta,
            Corpo::Progresso(_) => TipoMessaggio::Progresso,
            Corpo::Esito(_) => TipoMessaggio::Esito,
        };
        assert_eq!(frame.tipo(), atteso, "tipo dedotto male per «{nome}»");

        // E il filo porta sempre la versione corrente, che nessuno imposta.
        let byte = codifica(&frame).expect("codifica");
        let testo = String::from_utf8(byte[BYTE_PREFISSO..].to_vec()).expect("UTF-8");
        assert!(
            testo.starts_with(&format!(r#"{{"protocol_version":{VERSIONE_PROTOCOLLO},"#)),
            "versione assente o diversa per «{nome}»: {testo:.60}"
        );
    }
}

fn nome_sul_filo<T: serde::Serialize>(valore: &T) -> String {
    serde_json::to_string(valore)
        .expect("serializzabile")
        .trim_matches('"')
        .to_owned()
}

/// Le due proprieta' del vocabolario che la macro **non** garantisce.
///
/// L'esaustivita' non si prova qui, ed e' il punto: `TUTTE` nasce dalla
/// stessa lista che genera le varianti, quindi una variante nuova ci entra da
/// sola. Una tabella scritta a mano nel test enumererebbe se stessa: una
/// variante aggiunta all'enum la lascerebbe invariata, e il test verde.
///
/// Restano due cose che la macro non puo' garantire, ed entrambe sono difetti
/// veri:
///
/// - due varianti col **medesimo** nome sul filo. La macro le accetta e
///   `serde` pure: la seconda diventa irraggiungibile in lettura;
/// - un nome che rilegge la variante **di un'altra**, che e' come il caso
///   precedente si manifesta dal lato del decoder.
fn vocabolario_biunivoco<T>(tutte: &[(T, &'static str)], etichetta: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + core::fmt::Debug + Copy,
{
    let mut visti: std::collections::BTreeSet<&'static str> = std::collections::BTreeSet::new();
    for (valore, nome) in tutte {
        assert_eq!(
            &nome_sul_filo(valore),
            nome,
            "{etichetta}: {valore:?} non si serializza come `{nome}`"
        );
        let indietro: T = serde_json::from_str(&format!("\"{nome}\""))
            .unwrap_or_else(|errore| panic!("{etichetta}: `{nome}` non si rilegge: {errore}"));
        assert_eq!(
            &indietro, valore,
            "{etichetta}: `{nome}` rilegge una variante diversa da quella che lo dichiara"
        );
        assert!(
            visti.insert(nome),
            "{etichetta}: `{nome}` e' dichiarato da due varianti"
        );
    }
    assert!(!visti.is_empty(), "{etichetta}: nessuna variante");
}

/// I nomi di un enum **con tag interno** devono essere distinti.
///
/// Vale lo stesso motivo degli enum unitari, con una conseguenza piu' brutta:
/// due varianti con lo stesso valore di tag compilano, e la seconda diventa
/// irraggiungibile in lettura senza che nulla lo dica. La macro genera i
/// nomi ma non puo' impedirlo.
fn nomi_distinti(nomi: &[&'static str], etichetta: &str) {
    let mut visti: std::collections::BTreeSet<&'static str> = std::collections::BTreeSet::new();
    for nome in nomi {
        assert!(
            visti.insert(nome),
            "{etichetta}: `{nome}` e' dichiarato da due varianti"
        );
    }
    assert!(!visti.is_empty(), "{etichetta}: nessuna variante");
}

/// Il vocabolario sul filo delle enumerazioni chiuse.
#[test]
fn il_vocabolario_sul_filo_e_biunivoco() {
    vocabolario_biunivoco(TipoMessaggio::TUTTE, "TipoMessaggio");
    vocabolario_biunivoco(CategoriaSulFilo::TUTTE, "CategoriaSulFilo");
    vocabolario_biunivoco(FaseSulFilo::TUTTE, "FaseSulFilo");
    vocabolario_biunivoco(EffettoSulFilo::TUTTE, "EffettoSulFilo");
    vocabolario_biunivoco(FormaPanicSulFilo::TUTTE, "FormaPanicSulFilo");
    vocabolario_biunivoco(FormatoIngresso::TUTTE, "FormatoIngresso");

    // Gli enum con tag: la biunivocita' non si puo' provare allo stesso modo
    // (le varianti hanno campi, e il nome non e' l'intero valore), ma
    // l'unicita' si', ed e' la meta' che fa il danno.
    nomi_distinti(RetrySulFilo::NOMI, "RetrySulFilo");
    nomi_distinti(EsitoWorkerSulFilo::NOMI, "EsitoWorkerSulFilo");

    // I due insiemi generati dalla stessa lista devono avere la stessa
    // taglia: se divergessero, sarebbe la macro a espandere male, e i test
    // che iterano `TUTTE` starebbero saltando varianti in silenzio.
    assert_eq!(
        RetrySulFilo::TUTTE.len(),
        RetrySulFilo::NOMI.len(),
        "`TUTTE` e `NOMI` di `RetrySulFilo` non coprono le stesse varianti"
    );
    for (indice, (_, nome)) in RetrySulFilo::TUTTE.iter().enumerate() {
        assert_eq!(
            RetrySulFilo::NOMI.get(indice),
            Some(nome),
            "`TUTTE` e `NOMI` di `RetrySulFilo` divergono in posizione {indice}"
        );
    }
}

/// I tetti della sequenza, e la relazione fra loro.
///
/// Il protocollo non li usa — il contatore e' del supervisore — ma il budget
/// e' dichiarato qui, e la relazione fra i due numeri e' cio' che si puo'
/// sbagliare senza accorgersene.
#[test]
fn i_tetti_della_sequenza_sono_coerenti() {
    use super::limiti::{MAX_MESSAGGI_VERSO_SUPERVISORE, MAX_MESSAGGI_VERSO_WORKER, MAX_PROGRESSO};

    // `Saluto`, `Incarico`, e al piu' un `Annulla`.
    assert_eq!(MAX_MESSAGGI_VERSO_WORKER, 3);
    // `Risposta` + `Esito` + la quota di progresso. E' l'unica delle due che
    // dice qualcosa: un `assert!(MAX_PROGRESSO > 0)` sarebbe stato un
    // confronto fra due costanti note al compilatore, cioe' una guardia
    // tautologica travestita da prova.
    assert_eq!(MAX_MESSAGGI_VERSO_SUPERVISORE, 2 + MAX_PROGRESSO);
}

/// La direzione e' una **proprieta' del tipo**, non un campo sul filo.
///
/// Trasmetterla avrebbe permesso a un capo di dichiarare la direzione
/// sbagliata; qui non c'e' nulla da dichiarare.
#[test]
fn ogni_tipo_ha_una_sola_direzione() {
    use super::messaggi::Direzione;

    // L'oracolo e' un `match` **esaustivo** su `TUTTE`: un tipo nuovo non
    // compila finche' non gli si assegna una direzione qui, e non puo'
    // sfuggire all'iterazione perche' `TUTTE` lo contiene per costruzione.
    for (tipo, nome) in TipoMessaggio::TUTTE {
        let atteso = match tipo {
            TipoMessaggio::Saluto | TipoMessaggio::Incarico | TipoMessaggio::Annulla => {
                Direzione::VersoWorker
            }
            TipoMessaggio::Risposta | TipoMessaggio::Progresso | TipoMessaggio::Esito => {
                Direzione::VersoSupervisore
            }
        };
        assert_eq!(tipo.direzione(), atteso, "direzione sbagliata per `{nome}`");
    }
}

/// Nessun errore del decoder riporta cio' che ha letto.
///
/// E' l'asse che il rifiuto per forma lascia aperto: `serde` costruisce i
/// propri messaggi **dal valore incontrato** — «invalid type: string "…"»,
/// «unknown field `…`» — e su questo canale quel valore lo sceglie l'altro
/// capo. Un frame malformato ad arte sarebbe un modo di far scrivere testo
/// arbitrario nel log di chi indaga; e poiche' il `Saluto` porta il
/// `commit_token`, il testo arbitrario potrebbe essere il token.
///
/// Cio' che resta e' dove, non che cosa: categoria, riga e colonna. Chi indaga
/// ha il frame in mano.
#[test]
fn nessun_errore_del_decoder_riporta_cio_che_ha_letto() {
    const SENTINELLA: &str = "SEGRETO-8675309124816324";
    const NUMERICA: &str = "8675309124816324";

    let casi = [
        // Valore di tipo sbagliato: il valore finirebbe in `Unexpected::Str`.
        format!(
            r#"{{"protocol_version":"{SENTINELLA}","tipo":"annulla","corpo":{{"motivo":"x"}}}}"#
        ),
        // Campo ignoto: il **nome** lo sceglie chi scrive il frame.
        format!(
            r#"{{"protocol_version":1,"tipo":"annulla","corpo":{{"motivo":"x"}},"{SENTINELLA}":1}}"#
        ),
        // Valore numerico di tipo sbagliato, dentro il corpo.
        format!(r#"{{"protocol_version":1,"tipo":"annulla","corpo":{{"motivo":{NUMERICA}}}}}"#),
        // Chiave duplicata: anche il nome della chiave arriva dal filo.
        format!(
            r#"{{"protocol_version":1,"tipo":"annulla","corpo":{{"motivo":"x"}},"{SENTINELLA}":1,"{SENTINELLA}":2}}"#
        ),
        // Il caso che conta di piu': un `Saluto` col token accanto a un campo
        // malformato. Il token e' canonico, quindi non e' lui a far fallire.
        format!(
            r#"{{"protocol_version":1,"tipo":"saluto","corpo":{{"commit_token":"{}","limiti":"{SENTINELLA}"}}}}"#,
            "a".repeat(64)
        ),
    ];

    for testo in casi {
        let messaggio = rifiuto(&incornicia(&testo));
        assert!(
            !messaggio.contains(SENTINELLA) && !messaggio.contains(NUMERICA),
            "l'errore ha copiato cio' che ha letto: {messaggio}"
        );
        // E non per caso: il rifiuto dice dove, con la categoria e la
        // posizione. Se un giorno tornasse a dire «che cosa», questa riga
        // resterebbe vera mentre quella sopra cadrebbe — sono due asserzioni,
        // non una.
        assert!(
            messaggio.contains("riga ") || messaggio.contains("chiave JSON duplicata"),
            "rifiuto senza posizione ne' causa strutturale: {messaggio}"
        );
    }
}
