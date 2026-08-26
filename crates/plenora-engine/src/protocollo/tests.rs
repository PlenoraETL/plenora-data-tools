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

use super::codifica::{
    codifica, decodifica, ScrittoreLimitato, BYTE_PREFISSO, MAX_PROTOCOL_FRAME_BYTES,
};
use super::limiti::{
    MAX_BACKEND_DINAMICI, MAX_CAPABILITY, MAX_CHIAVE_CONTEGGIO_BYTES, MAX_CONTEGGI_DIAGNOSTICA,
    MAX_DIGEST_BYTES, MAX_ESEMPIO_BYTES, MAX_ESEMPI_DIAGNOSTICA, MAX_IDENTIFICATORE_BYTES,
    MAX_INGRESSI, MAX_MESSAGGIO_BYTES, MAX_MOTIVO_BYTES, MAX_PERCORSO_BYTES,
    MAX_PIANO_CANONICO_BYTES, MAX_RISORSE, MAX_VERSIONE_BYTES,
};
use super::messaggi::{
    Ambiente, Annulla, BackendDinamico, CategoriaSulFilo, Corpo, DescrittoreIngresso,
    DiagnosticaSulFilo, DigestArtefatto, EffettoSulFilo, ErroreSulFilo, EsempioDiagnostica,
    EsitoWorkerSulFilo, FaseSulFilo, FormaPanicSulFilo, FormatoIngresso, Frame, IdentitaArtefatto,
    IdentitaResolver, Incarico, LimitiDichiarati, Progresso, RetrySulFilo, RisorsaRisolta,
    Risposta, Saluto, TipoMessaggio, VERSIONE_PROTOCOLLO,
};

// ---------------------------------------------------------------------------
// I vettori scritti a mano
// ---------------------------------------------------------------------------

const CORPO_SALUTO: &str = concat!(
    r#"{"artefatto":{"digest":"aa","versione":"1.0"},"#,
    r#""resolver":{"identita":"proj","versione":"9.4"},"#,
    r#""ambiente":{"digest_insieme":"bb","acquisizione_dinamica":false,"#,
    r#""risorse":[{"nome":"grid","versione":"1","percorso":"/r/g"}],"#,
    r#""backend_dinamici":[]},"#,
    r#""commit_token":"cc","#,
    r#""limiti":{"max_frame_bytes":1,"max_piano_canonico_bytes":2,"#,
    r#""max_messaggi_verso_worker":3,"max_messaggi_verso_supervisore":4}}"#,
);
const PREFISSO_SALUTO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x01, 0xB2];

const CORPO_INCARICO: &str = concat!(
    r#"{"piano_canonico":{"schema_version":6},"#,
    r#""plan_hash_atteso":"dd","#,
    r#""ingressi":[{"nome":"in","percorso":"/d/a.arrow","formato":"file","#,
    r#""contract_fingerprint_atteso":"ee"}],"#,
    r#""artefatto_temporaneo":"/t/out.arrow"}"#,
);
const PREFISSO_INCARICO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0xFD];

const CORPO_ANNULLA: &str = r#"{"motivo":"timeout"}"#;
const PREFISSO_ANNULLA: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0x44];

const CORPO_RISPOSTA: &str = concat!(
    r#"{"artefatto":{"digest":"aa","versione":"1.0"},"#,
    r#""resolver":{"identita":"proj","versione":"9.4"},"#,
    r#""ambiente":{"digest_insieme":"bb","acquisizione_dinamica":false,"#,
    r#""risorse":[],"#,
    r#""backend_dinamici":[{"nome":"gdal","versione":"3.8","percorso":"/l/libgdal.so"}]},"#,
    r#""capability":["arrow_ipc"]}"#,
);
const PREFISSO_RISPOSTA: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x01, 0x49];

const CORPO_PROGRESSO: &str = r#"{"righe":10,"batch":2,"nodi_completati":3}"#;
const PREFISSO_PROGRESSO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0x5C];

const CORPO_ESITO_SUCCESSO: &str =
    r#"{"esito":"successo","digest_artefatto":{"algoritmo":"sha256","valore":"ff"}}"#;
const PREFISSO_ESITO_SUCCESSO: [u8; BYTE_PREFISSO] = [0x00, 0x00, 0x00, 0x7A];

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

fn artefatto() -> IdentitaArtefatto {
    IdentitaArtefatto {
        digest: "aa".to_owned(),
        versione: "1.0".to_owned(),
    }
}

fn resolver() -> IdentitaResolver {
    IdentitaResolver {
        identita: "proj".to_owned(),
        versione: "9.4".to_owned(),
    }
}

fn saluto() -> Frame {
    Frame {
        protocol_version: VERSIONE_PROTOCOLLO,
        tipo: TipoMessaggio::Saluto,
        corpo: Corpo::Saluto(Box::new(Saluto {
            artefatto: artefatto(),
            resolver: resolver(),
            ambiente: Ambiente {
                digest_insieme: "bb".to_owned(),
                acquisizione_dinamica: false,
                risorse: vec![RisorsaRisolta {
                    nome: "grid".to_owned(),
                    versione: "1".to_owned(),
                    percorso: "/r/g".to_owned(),
                }],
                backend_dinamici: Vec::new(),
            },
            commit_token: "cc".to_owned(),
            limiti: LimitiDichiarati {
                max_frame_bytes: 1,
                max_piano_canonico_bytes: 2,
                max_messaggi_verso_worker: 3,
                max_messaggi_verso_supervisore: 4,
            },
        })),
    }
}

fn incarico() -> Frame {
    Frame {
        protocol_version: VERSIONE_PROTOCOLLO,
        tipo: TipoMessaggio::Incarico,
        corpo: Corpo::Incarico(Box::new(Incarico {
            piano_canonico: grezzo(r#"{"schema_version":6}"#),
            plan_hash_atteso: "dd".to_owned(),
            ingressi: vec![DescrittoreIngresso {
                nome: "in".to_owned(),
                percorso: "/d/a.arrow".to_owned(),
                formato: FormatoIngresso::File,
                contract_fingerprint_atteso: "ee".to_owned(),
            }],
            artefatto_temporaneo: "/t/out.arrow".to_owned(),
        })),
    }
}

fn annulla_con(motivo: String) -> Frame {
    Frame {
        protocol_version: VERSIONE_PROTOCOLLO,
        tipo: TipoMessaggio::Annulla,
        corpo: Corpo::Annulla(Annulla { motivo }),
    }
}

fn annulla() -> Frame {
    annulla_con("timeout".to_owned())
}

fn incarico_con(piano: Box<RawValue>, ingressi: Vec<DescrittoreIngresso>) -> Frame {
    Frame {
        protocol_version: VERSIONE_PROTOCOLLO,
        tipo: TipoMessaggio::Incarico,
        corpo: Corpo::Incarico(Box::new(Incarico {
            piano_canonico: piano,
            plan_hash_atteso: "dd".to_owned(),
            ingressi,
            artefatto_temporaneo: "/t/out.arrow".to_owned(),
        })),
    }
}

fn risposta() -> Frame {
    Frame {
        protocol_version: VERSIONE_PROTOCOLLO,
        tipo: TipoMessaggio::Risposta,
        corpo: Corpo::Risposta(Box::new(Risposta {
            artefatto: artefatto(),
            resolver: resolver(),
            ambiente: Ambiente {
                digest_insieme: "bb".to_owned(),
                acquisizione_dinamica: false,
                risorse: Vec::new(),
                backend_dinamici: vec![BackendDinamico {
                    nome: "gdal".to_owned(),
                    versione: "3.8".to_owned(),
                    percorso: "/l/libgdal.so".to_owned(),
                }],
            },
            capability: vec!["arrow_ipc".to_owned()],
        })),
    }
}

fn progresso() -> Frame {
    Frame {
        protocol_version: VERSIONE_PROTOCOLLO,
        tipo: TipoMessaggio::Progresso,
        corpo: Corpo::Progresso(Progresso {
            righe: 10,
            batch: 2,
            nodi_completati: 3,
        }),
    }
}

fn esito(dentro: EsitoWorkerSulFilo) -> Frame {
    Frame {
        protocol_version: VERSIONE_PROTOCOLLO,
        tipo: TipoMessaggio::Esito,
        corpo: Corpo::Esito(Box::new(dentro)),
    }
}

fn esito_successo() -> Frame {
    esito(EsitoWorkerSulFilo::Successo {
        digest_artefatto: DigestArtefatto {
            algoritmo: "sha256".to_owned(),
            valore: "ff".to_owned(),
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

    assert_eq!(
        lunghezza_dichiarata(&[0x00, 0x00, 0x00, 0x00]).ok(),
        Some(0)
    );
    let al_tetto = u32::try_from(MAX_PROTOCOL_FRAME_BYTES).expect("sta in u32");
    assert_eq!(
        lunghezza_dichiarata(&al_tetto.to_be_bytes()).ok(),
        Some(MAX_PROTOCOL_FRAME_BYTES),
        "MAX deve passare: il tetto e' inclusivo"
    );
    let oltre = al_tetto + 1;
    assert!(
        lunghezza_dichiarata(&oltre.to_be_bytes()).is_err(),
        "MAX + 1 deve fallire"
    );
    assert!(lunghezza_dichiarata(&[0xFF, 0xFF, 0xFF, 0xFF]).is_err());

    // E il decoder rifiuta con **quello stesso** motivo, cioe' non ha una
    // copia propria del confronto.
    let suo = lunghezza_dichiarata(&oltre.to_be_bytes())
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
    assert!(rifiuto(&incornicia(involucro)).contains("duplicate"));

    // Anche in fondo, dentro il corpo.
    let corpo = r#"{"protocol_version":1,"tipo":"annulla","corpo":{"motivo":"x","motivo":"y"}}"#;
    assert!(rifiuto(&incornicia(corpo)).contains("duplicate"));

    // E dentro il piano, che il protocollo non interpreta ma non per questo
    // lascia ambiguo.
    let piano = concat!(
        r#"{"protocol_version":1,"tipo":"incarico","corpo":{"#,
        r#""piano_canonico":{"a":1,"a":2},"plan_hash_atteso":"dd","#,
        r#""ingressi":[],"artefatto_temporaneo":"/t/o"}}"#,
    );
    assert!(rifiuto(&incornicia(piano)).contains("duplicate"));
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
            r#""piano_canonico":{},"plan_hash_atteso":"dd","#,
            r#""ingressi":[{"nome":"i","percorso":"/p","formato":"file","#,
            r#""contract_fingerprint_atteso":"e","extra":1}],"#,
            r#""artefatto_temporaneo":"/t/o"}}"#,
        ),
    ];
    for testo in casi {
        let messaggio = rifiuto(&incornicia(testo));
        assert!(
            messaggio.contains("unknown field"),
            "campo ignoto accettato in «{testo}»: {messaggio}"
        );
    }
}

#[test]
fn un_campo_mancante_e_un_errore() {
    let testo = r#"{"protocol_version":1,"tipo":"progresso","corpo":{"righe":1,"batch":2}}"#;
    let messaggio = rifiuto(&incornicia(testo));
    assert!(
        messaggio.contains("missing field"),
        "campo mancante accettato: {messaggio}"
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
        messaggio.contains("unknown variant"),
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
                r#"{"piano_canonico":{},"plan_hash_atteso":"d","#,
                r#""ingressi":[{"nome":"i","percorso":"/p","formato":"socket","#,
                r#""contract_fingerprint_atteso":"e"}],"artefatto_temporaneo":"/t"}"#,
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
            messaggio.contains("unknown variant"),
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
    let casi = [
        // `esito`, variante con campi
        r#"{"esito":"panic","forma":"statico","extra":1}"#,
        r#"{"esito":"successo","digest_artefatto":{"algoritmo":"sha256","valore":"f"},"extra":1}"#,
    ];
    for corpo in casi {
        let messaggio = rifiuto(&incornicia(&payload("esito", corpo)));
        assert!(
            messaggio.contains("unknown field"),
            "campo ignoto accettato in «{corpo}»: {messaggio}"
        );
    }

    // `retry`, tutte e cinque le disposizioni.
    for disposizione in [
        "never",
        "safe",
        "requires_idempotency_key",
        "requires_recovery",
    ] {
        let corpo = format!(
            concat!(
                r#"{{"esito":"errore","errore":{{"categoria":"schema","fase":"read","#,
                r#""effetto":"none","retry":{{"kind":"{}","estraneo":1}},"messaggio":"m","#,
                r#""nodo":null,"operazione":null,"execution_id":null,"diagnostica":null}}}}"#,
            ),
            disposizione
        );
        let messaggio = rifiuto(&incornicia(&payload("esito", &corpo)));
        assert!(
            messaggio.contains("unknown field"),
            "campo ignoto accettato su `{disposizione}`: {messaggio}"
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
        messaggio.contains("unknown field"),
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

/// Un ingresso minimo, per i casi in cui a essere sotto esame e' un altro
/// campo.
fn ingresso_minimo() -> DescrittoreIngresso {
    DescrittoreIngresso {
        nome: "i".to_owned(),
        percorso: "/p".to_owned(),
        formato: FormatoIngresso::File,
        contract_fingerprint_atteso: "e".to_owned(),
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
        digest_insieme: ripeti(MAX_DIGEST_BYTES),
        acquisizione_dinamica: true,
        risorse: vec![
            RisorsaRisolta {
                nome: ripeti(MAX_IDENTIFICATORE_BYTES),
                versione: ripeti(MAX_VERSIONE_BYTES),
                percorso: ripeti(MAX_PERCORSO_BYTES),
            };
            MAX_RISORSE
        ],
        backend_dinamici: vec![
            BackendDinamico {
                nome: ripeti(MAX_IDENTIFICATORE_BYTES),
                versione: ripeti(MAX_VERSIONE_BYTES),
                percorso: ripeti(MAX_PERCORSO_BYTES),
            };
            MAX_BACKEND_DINAMICI
        ],
    }
}

fn piano_al_tetto() -> Box<RawValue> {
    grezzo(&piano_di(MAX_PIANO_CANONICO_BYTES))
}

/// I sei massimi **veri**, prodotti dal codificatore.
fn massimi() -> Vec<(&'static str, Frame)> {
    let artefatto = IdentitaArtefatto {
        digest: ripeti(MAX_DIGEST_BYTES),
        versione: ripeti(MAX_VERSIONE_BYTES),
    };
    let resolver = IdentitaResolver {
        identita: ripeti(MAX_IDENTIFICATORE_BYTES),
        versione: ripeti(MAX_VERSIONE_BYTES),
    };
    vec![
        (
            "saluto",
            Frame {
                protocol_version: VERSIONE_PROTOCOLLO,
                tipo: TipoMessaggio::Saluto,
                corpo: Corpo::Saluto(Box::new(Saluto {
                    artefatto: artefatto.clone(),
                    resolver: resolver.clone(),
                    ambiente: ambiente_massimo(),
                    commit_token: ripeti(MAX_DIGEST_BYTES),
                    limiti: LimitiDichiarati {
                        max_frame_bytes: u64::MAX,
                        max_piano_canonico_bytes: u64::MAX,
                        max_messaggi_verso_worker: u64::MAX,
                        max_messaggi_verso_supervisore: u64::MAX,
                    },
                })),
            },
        ),
        (
            "incarico",
            Frame {
                protocol_version: VERSIONE_PROTOCOLLO,
                tipo: TipoMessaggio::Incarico,
                corpo: Corpo::Incarico(Box::new(Incarico {
                    piano_canonico: piano_al_tetto(),
                    plan_hash_atteso: ripeti(MAX_DIGEST_BYTES),
                    ingressi: vec![
                        DescrittoreIngresso {
                            nome: ripeti(MAX_IDENTIFICATORE_BYTES),
                            percorso: ripeti(MAX_PERCORSO_BYTES),
                            formato: FormatoIngresso::Stream,
                            contract_fingerprint_atteso: ripeti(MAX_DIGEST_BYTES),
                        };
                        MAX_INGRESSI
                    ],
                    artefatto_temporaneo: ripeti(MAX_PERCORSO_BYTES),
                })),
            },
        ),
        (
            "annulla",
            Frame {
                protocol_version: VERSIONE_PROTOCOLLO,
                tipo: TipoMessaggio::Annulla,
                corpo: Corpo::Annulla(Annulla {
                    motivo: ripeti(MAX_MOTIVO_BYTES),
                }),
            },
        ),
        (
            "risposta",
            Frame {
                protocol_version: VERSIONE_PROTOCOLLO,
                tipo: TipoMessaggio::Risposta,
                corpo: Corpo::Risposta(Box::new(Risposta {
                    artefatto,
                    resolver,
                    ambiente: ambiente_massimo(),
                    capability: vec![ripeti(MAX_IDENTIFICATORE_BYTES); MAX_CAPABILITY],
                })),
            },
        ),
        (
            "progresso",
            Frame {
                protocol_version: VERSIONE_PROTOCOLLO,
                tipo: TipoMessaggio::Progresso,
                corpo: Corpo::Progresso(Progresso {
                    righe: u64::MAX,
                    batch: u64::MAX,
                    nodi_completati: u64::MAX,
                }),
            },
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
            messaggio: ripeti(MAX_MESSAGGIO_BYTES),
            nodo: Some(ripeti(MAX_IDENTIFICATORE_BYTES)),
            operazione: Some(ripeti(MAX_IDENTIFICATORE_BYTES)),
            execution_id: Some(ripeti(MAX_IDENTIFICATORE_BYTES)),
            diagnostica: Some(DiagnosticaSulFilo {
                contract: ripeti(MAX_IDENTIFICATORE_BYTES),
                scope: ripeti(MAX_IDENTIFICATORE_BYTES),
                completeness: ripeti(MAX_IDENTIFICATORE_BYTES),
                observed_total: u64::MAX,
                conteggi: vec![
                    (ripeti(MAX_CHIAVE_CONTEGGIO_BYTES), u64::MAX);
                    MAX_CONTEGGI_DIAGNOSTICA
                ],
                esempi: vec![
                    EsempioDiagnostica {
                        indice: u64::MAX,
                        codice: ripeti(MAX_ESEMPIO_BYTES),
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

/// Le invarianti che il target fuzz pretende, verificate qui sulla suite
/// normale.
///
/// Il target `protocollo_frame` gira solo su nightly, e un'invariante
/// sbagliata li' non si manifesterebbe come un difetto del protocollo ma come
/// una campagna che fallisce alle 3 del mattino. Provarle qui costa poco e
/// dice **subito** se sono vere.
///
/// I casi sono scelti **non canonici** di proposito: chiavi in ordine
/// diverso, spazi, `Esito` per cui il decoder accetta piu' forme dello stesso
/// messaggio. E' li' che un'invariante di idempotenza si rompe.
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
                r#"{"piano_canonico":{ "a" : [ 1 , 2 ] },"plan_hash_atteso":"d","#,
                r#""ingressi":[],"artefatto_temporaneo":"/t"}"#,
            ),
        ),
    ];
    for testo in non_canonici {
        let byte = incornicia(&testo);
        let frame = decodifica(&byte).unwrap_or_else(|errore| {
            panic!("il caso non canonico «{testo}» va accettato: {errore}")
        });

        let canonico = codifica(&frame).expect("un frame decodificato si ricodifica sempre");
        let corpo = canonico.len() - BYTE_PREFISSO;
        assert!(corpo <= MAX_PROTOCOL_FRAME_BYTES);
        let dichiarata =
            u32::from_be_bytes([canonico[0], canonico[1], canonico[2], canonico[3]]) as usize;
        assert_eq!(dichiarata, corpo, "prefisso incoerente per «{testo}»");

        let riletto = decodifica(&canonico).expect("la forma canonica si rilegge");
        assert_eq!(riletto, frame, "il giro non e' idempotente per «{testo}»");
        assert_eq!(
            codifica(&riletto).expect("ricodifica"),
            canonico,
            "codifica non deterministica per «{testo}»"
        );
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
            messaggio.contains("unknown variant"),
            "forma di panico «{candidato}» accettata: {messaggio}"
        );
    }
}

/// La sentinella non compare nemmeno negli errori del **writer**.
#[test]
fn il_contenuto_non_entra_negli_errori_del_writer() {
    let mut frame = annulla();
    let Corpo::Annulla(dentro) = &mut frame.corpo else {
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

/// La direzione e' una **proprieta' del tipo**, non un campo sul filo.
///
/// Trasmetterla avrebbe permesso a un capo di dichiarare la direzione
/// sbagliata; qui non c'e' nulla da dichiarare.
#[test]
fn ogni_tipo_ha_una_sola_direzione() {
    use super::messaggi::Direzione;

    let verso_worker = [
        TipoMessaggio::Saluto,
        TipoMessaggio::Incarico,
        TipoMessaggio::Annulla,
    ];
    let verso_supervisore = [
        TipoMessaggio::Risposta,
        TipoMessaggio::Progresso,
        TipoMessaggio::Esito,
    ];
    for tipo in verso_worker {
        assert_eq!(tipo.direzione(), Direzione::VersoWorker, "{tipo:?}");
    }
    for tipo in verso_supervisore {
        assert_eq!(tipo.direzione(), Direzione::VersoSupervisore, "{tipo:?}");
    }
    // I sei tipi sono coperti: se ne comparisse un settimo, questa somma
    // smetterebbe di tornare.
    assert_eq!(verso_worker.len() + verso_supervisore.len(), 6);
}
