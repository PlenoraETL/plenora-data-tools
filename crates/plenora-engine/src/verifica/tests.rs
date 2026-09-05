//! Prove del verificatore dell'artefatto.
//!
//! Ogni caso di rifiuto e' scritto **negativo**: si costruisce un artefatto
//! che sbaglia una cosa sola e si pretende che venga respinto. Un test che
//! verificasse solo il caso nominale passerebbe anche con un verificatore che
//! non verifica niente.

use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, DictionaryArray, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::ipc::writer::FileWriter;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::DataContract;
use sha2::{Digest, Sha256};

use super::{verifica_artefatto, AtteseVerifica, ALGORITMO_DIGEST};
use crate::commit_footer::scrivi_commit_token;
use crate::commit_token::CommitToken;
use crate::geo_transport::ipc::IpcLimits;

const UNO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DUE: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

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
            Arc::new(StringArray::from(vec![
                Some("segreto-alfa"),
                None,
                Some("segreto-beta"),
            ])),
        ],
    )
    .expect("batch valido")
}

/// Schema con due colonne dictionary-encoded: due blocchi di dizionario nel
/// footer, che e' cio' che il tetto cumulativo governa.
fn schema_con_dizionari() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(
            "citta",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
        Field::new(
            "regione",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
    ]))
}

fn batch_con_dizionari() -> RecordBatch {
    let citta: DictionaryArray<Int32Type> =
        vec!["milano", "torino", "milano"].into_iter().collect();
    let regione: DictionaryArray<Int32Type> = vec!["lombardia", "piemonte", "lombardia"]
        .into_iter()
        .collect();
    RecordBatch::try_new(
        schema_con_dizionari(),
        vec![Arc::new(citta), Arc::new(regione)],
    )
    .expect("batch con dizionari")
}

/// Scrive un artefatto con `batch_count` batch identici e il token dato.
fn artefatto(
    schema: &SchemaRef,
    batch: &RecordBatch,
    quanti: usize,
    tok: Option<&CommitToken>,
) -> Vec<u8> {
    let mut byte = Vec::new();
    {
        let mut scrittore = FileWriter::try_new(&mut byte, schema).expect("writer");
        for _ in 0..quanti {
            scrittore.write(batch).expect("batch scritto");
        }
        scrivi_commit_token(&mut scrittore, tok);
        scrittore.finish().expect("finish");
    }
    byte
}

/// Artefatto con una coppia arbitraria nei custom metadata del footer.
fn artefatto_con_metadata(coppie: &[(&str, &str)]) -> Vec<u8> {
    let mut byte = Vec::new();
    {
        let mut scrittore = FileWriter::try_new(&mut byte, &schema()).expect("writer");
        scrittore.write(&batch()).expect("batch scritto");
        for (chiave, valore) in coppie {
            scrittore.write_metadata(*chiave, *valore);
        }
        scrittore.finish().expect("finish");
    }
    byte
}

fn digest_reale(byte: &[u8]) -> String {
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

/// Esegue il verificatore su byte scritti in un file temporaneo.
struct Prova {
    _dir: tempfile::TempDir,
    percorso: std::path::PathBuf,
}

impl Prova {
    fn nuova(byte: &[u8]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let percorso = dir.path().join("artefatto.arrow");
        std::fs::write(&percorso, byte).expect("scrittura");
        Self {
            _dir: dir,
            percorso,
        }
    }
}

struct Dichiarati {
    algoritmo: String,
    digest: String,
    righe: u64,
    batch: u64,
    token: CommitToken,
    contratto: DataContract,
    tetto_dizionari: u64,
}

impl Dichiarati {
    /// Tutto coerente con un artefatto di tre righe e un batch, senza
    /// dizionari.
    fn coerenti(byte: &[u8]) -> Self {
        Self {
            algoritmo: ALGORITMO_DIGEST.to_owned(),
            digest: digest_reale(byte),
            righe: 3,
            batch: 1,
            token: token(UNO),
            contratto: DataContract::tabular(schema()),
            tetto_dizionari: 1024 * 1024,
        }
    }
}

fn esegui(byte: &[u8], dichiarati: &Dichiarati) -> Result<(), String> {
    let prova = Prova::nuova(byte);
    let digest = crate::protocollo::messaggi::DigestArtefatto {
        algoritmo: dichiarati.algoritmo.clone(),
        valore: dichiarati.digest.clone(),
    };
    let attese = AtteseVerifica {
        contratto: &dichiarati.contratto,
        digest: &digest,
        conteggi: crate::protocollo::messaggi::ConteggiDichiarati {
            righe: dichiarati.righe,
            batch: dichiarati.batch,
        },
        commit_token: &dichiarati.token,
    };
    let limiti = IpcLimits {
        max_retained_dictionary_body_bytes: dichiarati.tetto_dizionari,
        ..IpcLimits::default()
    };
    verifica_artefatto(
        &prova.percorso,
        &attese,
        plenora_core::crs::resolve_crs,
        &limiti,
    )
    .map_err(|errore| errore.to_string())
}

// ---------------------------------------------------------------------------
// Il caso nominale
// ---------------------------------------------------------------------------

#[test]
fn un_artefatto_coerente_e_accettato() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    assert_eq!(esegui(&byte, &Dichiarati::coerenti(&byte)), Ok(()));
}

// ---------------------------------------------------------------------------
// Passo 3: presenza
// ---------------------------------------------------------------------------

#[test]
fn un_artefatto_che_non_esiste_e_un_errore_di_io() {
    let dir = tempfile::tempdir().expect("tempdir");
    let digest = crate::protocollo::messaggi::DigestArtefatto {
        algoritmo: ALGORITMO_DIGEST.to_owned(),
        valore: UNO.to_owned(),
    };
    let contratto = DataContract::tabular(schema());
    let tok = token(UNO);
    let attese = AtteseVerifica {
        contratto: &contratto,
        digest: &digest,
        conteggi: crate::protocollo::messaggi::ConteggiDichiarati { righe: 0, batch: 0 },
        commit_token: &tok,
    };
    let esito = verifica_artefatto(
        &dir.path().join("non-esiste.arrow"),
        &attese,
        plenora_core::crs::resolve_crs,
        &IpcLimits {
            max_retained_dictionary_body_bytes: 0,
            ..IpcLimits::default()
        },
    );
    assert!(matches!(
        esito,
        Err(plenora_core::error::PlenoraError::Tagged { .. }
            | plenora_core::error::PlenoraError::Io(_))
    ));
}

// ---------------------------------------------------------------------------
// Passi 4 e 5: sigillo e framing
// ---------------------------------------------------------------------------

#[test]
fn un_artefatto_troncato_e_respinto() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let troncato = &byte[..byte.len() / 2];
    let mut dichiarati = Dichiarati::coerenti(troncato);
    dichiarati.righe = 3;
    dichiarati.batch = 1;
    assert!(esegui(troncato, &dichiarati).is_err());
}

#[test]
fn byte_arbitrari_non_sono_un_artefatto() {
    let spazzatura = vec![0x41_u8; 512];
    assert!(esegui(&spazzatura, &Dichiarati::coerenti(&spazzatura)).is_err());
}

// ---------------------------------------------------------------------------
// Passo 5-bis: integrita'
// ---------------------------------------------------------------------------

#[test]
fn un_digest_diverso_e_respinto() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.digest = DUE.to_owned();
    let errore = esegui(&byte, &dichiarati).expect_err("deve fallire");
    assert!(errore.contains("digest"), "{errore}");
}

#[test]
fn un_algoritmo_non_ammesso_e_respinto_e_non_compare_nell_errore() {
    // Il nome e' una **sentinella non fidata**: arriva dall'`Esito`, cioe' da
    // un altro processo, ed e' testo che chi lo scrive controlla. Ripeterlo
    // nell'errore metterebbe in un log una stringa arbitraria di quel processo.
    const SENTINELLA: &str = "algoritmo-non-fidato-9f3c";

    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    // Il valore del digest resta quello giusto: e' l'algoritmo dichiarato a non
    // essere ammesso, e il verificatore non deve accettarlo «perche' tanto il
    // numero combacia».
    dichiarati.algoritmo = SENTINELLA.to_owned();
    let errore = esegui(&byte, &dichiarati).expect_err("deve fallire");
    assert!(errore.contains("algoritmo"), "{errore}");
    assert!(
        !errore.contains(SENTINELLA),
        "il nome dichiarato non deve comparire: {errore}"
    );
}

#[test]
fn un_digest_non_canonico_e_respinto() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    // Stesso valore, grafia maiuscola: due grafie dello stesso numero sono due
    // digest diversi, e il confronto e' su una forma canonica sola.
    dichiarati.digest = digest_reale(&byte).to_uppercase();
    let errore = esegui(&byte, &dichiarati).expect_err("deve fallire");
    assert!(errore.contains("canonico"), "{errore}");
}

// ---------------------------------------------------------------------------
// Passo 7: contratto
// ---------------------------------------------------------------------------

#[test]
fn un_contratto_diverso_e_respinto() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.contratto = DataContract::tabular(Arc::new(Schema::new(vec![Field::new(
        "altro",
        DataType::Int32,
        true,
    )])));
    let errore = esegui(&byte, &dichiarati).expect_err("deve fallire");
    assert!(errore.contains("contratto"), "{errore}");
}

// ---------------------------------------------------------------------------
// Passo 8: completezza
// ---------------------------------------------------------------------------

#[test]
fn conteggi_dichiarati_in_eccesso_sono_respinti() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.righe = 4;
    assert!(esegui(&byte, &dichiarati).is_err());
}

#[test]
fn conteggi_dichiarati_in_difetto_sono_respinti() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.righe = 2;
    assert!(esegui(&byte, &dichiarati).is_err());
}

#[test]
fn un_numero_di_batch_sbagliato_e_respinto() {
    let byte = artefatto(&schema(), &batch(), 2, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.righe = 6;
    dichiarati.batch = 1;
    assert!(esegui(&byte, &dichiarati).is_err());
}

/// **Il digest non sostituisce i conteggi.**
///
/// L'artefatto e' integro e il suo digest e' quello giusto: un verificatore
/// che si fermasse al digest lo accetterebbe. I conteggi dicono che manca
/// meta' del lavoro.
#[test]
fn un_digest_corretto_non_salva_conteggi_falsi() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    assert_eq!(dichiarati.digest, digest_reale(&byte));
    dichiarati.righe = 6;
    dichiarati.batch = 2;
    let errore = esegui(&byte, &dichiarati).expect_err("deve fallire");
    assert!(errore.contains("conteggi"), "{errore}");
}

// ---------------------------------------------------------------------------
// Passo 8-bis: identita' del tentativo
// ---------------------------------------------------------------------------

#[test]
fn un_artefatto_senza_token_non_e_di_questo_tentativo() {
    let byte = artefatto(&schema(), &batch(), 1, None);
    let errore = esegui(&byte, &Dichiarati::coerenti(&byte)).expect_err("deve fallire");
    assert!(errore.contains("commit token"), "{errore}");
}

#[test]
fn un_token_diverso_e_respinto() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(DUE)));
    // Le attese portano UNO.
    let errore = esegui(&byte, &Dichiarati::coerenti(&byte)).expect_err("deve fallire");
    assert!(errore.contains("commit token"), "{errore}");
}

#[test]
fn un_token_non_canonico_nel_footer_e_respinto() {
    let byte = artefatto_con_metadata(&[(
        crate::commit_token::CHIAVE_FOOTER_COMMIT_TOKEN,
        "NON-CANONICO",
    )]);
    let errore = esegui(&byte, &Dichiarati::coerenti(&byte)).expect_err("deve fallire");
    assert!(errore.contains("canonico"), "{errore}");
    assert!(
        !errore.contains("NON-CANONICO"),
        "il valore non deve comparire: {errore}"
    );
}

// ---------------------------------------------------------------------------
// Il tetto cumulativo sui dizionari
// ---------------------------------------------------------------------------

#[test]
fn un_artefatto_senza_dizionari_non_ne_trattiene_nessun_byte() {
    // Tetto a zero: se il verificatore contasse qualcosa che non e' un
    // dizionario, questo caso fallirebbe.
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.tetto_dizionari = 0;
    assert_eq!(esegui(&byte, &dichiarati), Ok(()));
}

#[test]
fn i_dizionari_sotto_il_tetto_sono_verificati() {
    let byte = artefatto(
        &schema_con_dizionari(),
        &batch_con_dizionari(),
        1,
        Some(&token(UNO)),
    );
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.contratto = DataContract::tabular(schema_con_dizionari());
    dichiarati.tetto_dizionari = 1024 * 1024;
    assert_eq!(esegui(&byte, &dichiarati), Ok(()));
}

#[test]
fn i_dizionari_oltre_il_tetto_sono_respinti_prima_di_arrow() {
    let byte = artefatto(
        &schema_con_dizionari(),
        &batch_con_dizionari(),
        1,
        Some(&token(UNO)),
    );
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.contratto = DataContract::tabular(schema_con_dizionari());
    // I due dizionari stanno comodamente sotto `max_body_bytes`, che resta
    // quello di default: e' il tetto cumulativo a fermarli.
    dichiarati.tetto_dizionari = 1;
    let errore = esegui(&byte, &dichiarati).expect_err("deve fallire");
    assert!(errore.contains("dizionari"), "{errore}");
}

// ---------------------------------------------------------------------------
// Streaming: nessun batch precedente resta vivo
// ---------------------------------------------------------------------------

/// Il conteggio incrementale regge a `N` e a `10N` batch, e **discrimina** a
/// entrambe le scale.
///
/// # Che cosa prova, e che cosa no
///
/// Prova che il verificatore attraversa **tutti** i batch e li somma
/// esattamente: un lettore che si fermasse al primo, o che ne contasse uno di
/// troppo, fallirebbe qui — ed e' la ragione per cui ogni scala include anche
/// un conteggio sbagliato di uno, che deve essere respinto. Senza quella
/// seconda meta' il caso passerebbe anche con un verificatore che non
/// verifica.
///
/// **Non prova** che nessun batch precedente resti vivo. Quella proprieta' e'
/// **strutturale** — la funzione di conteggio non tiene collezioni — e
/// in-process non esiste un tetto imposto che la renda osservabile: e'
/// esattamente cio' che manca finche' il profilo isolato non esiste. Scrivere
/// che due esecuzioni riuscite la dimostrano sarebbe un falso oracolo.
#[test]
fn il_conteggio_incrementale_discrimina_a_n_e_a_dieci_n_batch() {
    for (quanti, righe) in [(5_usize, 15_u64), (50, 150)] {
        let byte = artefatto(&schema(), &batch(), quanti, Some(&token(UNO)));
        let batch_attesi = u64::try_from(quanti).expect("entro u64");

        let mut giusti = Dichiarati::coerenti(&byte);
        giusti.righe = righe;
        giusti.batch = batch_attesi;
        assert_eq!(
            esegui(&byte, &giusti),
            Ok(()),
            "conteggi giusti, {quanti} batch"
        );

        let mut una_riga_di_meno = Dichiarati::coerenti(&byte);
        con_conteggi(&mut una_riga_di_meno, righe - 1, batch_attesi);
        assert!(
            esegui(&byte, &una_riga_di_meno).is_err(),
            "una riga di meno deve fallire, {quanti} batch"
        );

        let mut un_batch_di_meno = Dichiarati::coerenti(&byte);
        con_conteggi(&mut un_batch_di_meno, righe, batch_attesi - 1);
        assert!(
            esegui(&byte, &un_batch_di_meno).is_err(),
            "un batch di meno deve fallire, {quanti} batch"
        );
    }
}

fn con_conteggi(dichiarati: &mut Dichiarati, righe: u64, batch: u64) {
    dichiarati.righe = righe;
    dichiarati.batch = batch;
}

// ---------------------------------------------------------------------------
// Errori senza dati
// ---------------------------------------------------------------------------

/// Nessun messaggio di rifiuto porta valori delle celle.
///
/// I valori scritti nel batch sono riconoscibili di proposito: se un errore li
/// citasse, questo test lo direbbe.
#[test]
fn nessun_errore_porta_valori_dell_artefatto() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(DUE)));
    let mut casi: Vec<Dichiarati> = Vec::new();
    casi.push({
        let mut d = Dichiarati::coerenti(&byte);
        d.digest = DUE.to_owned();
        d
    });
    casi.push({
        let mut d = Dichiarati::coerenti(&byte);
        d.righe = 99;
        d
    });
    casi.push(Dichiarati::coerenti(&byte));
    for dichiarati in &casi {
        let errore = esegui(&byte, dichiarati).expect_err("deve fallire");
        for segreto in ["segreto-alfa", "segreto-beta"] {
            assert!(
                !errore.contains(segreto),
                "l'errore cita un valore di cella: {errore}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Non-ritenzione: il batch precedente e' rilasciato prima del successivo
// ---------------------------------------------------------------------------

/// Il conteggio **non trattiene** il batch precedente.
///
/// La prova non ha bisogno di un limite di processo, e infatti non ne usa uno.
/// L'iteratore consegna batch costruiti su un array di cui conserva soltanto
/// una [`Weak`](std::sync::Weak): prima di produrre il successivo pretende che
/// quella `Weak` non si possa piu' promuovere, cioe' che l'ultimo riferimento
/// forte sia caduto. Se il consumatore accumulasse — un `Vec`, una `HashMap`,
/// perfino una sola variabile viva — il riferimento resterebbe e il test
/// fallirebbe qui, non in un profiler.
///
/// E' il criterio di uscita normativo «nessun record batch precedente resta
/// vivo», reso osservabile.
#[test]
fn nessun_batch_precedente_resta_vivo_durante_il_conteggio() {
    use std::sync::Weak;

    use plenora_core::arrow::array::{Array, ArrayRef};

    struct BatchSorvegliati {
        rimasti: usize,
        precedente: Option<Weak<dyn Array>>,
    }

    impl Iterator for BatchSorvegliati {
        type Item = plenora_core::error::Result<RecordBatch>;

        fn next(&mut self) -> Option<Self::Item> {
            if let Some(precedente) = self.precedente.take() {
                assert!(
                    precedente.upgrade().is_none(),
                    "il batch precedente e' ancora vivo: il conteggio lo trattiene"
                );
            }
            if self.rimasti == 0 {
                return None;
            }
            self.rimasti -= 1;
            let colonna: ArrayRef = Arc::new(UInt64Array::from(vec![1_u64, 2, 3]));
            // La `Weak` sopravvive al batch; il riferimento forte no.
            self.precedente = Some(Arc::downgrade(&colonna));
            let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::UInt64, false)]));
            let batch = RecordBatch::try_new(schema, vec![colonna]).expect("batch valido");
            Some(Ok(batch))
        }
    }

    let sorvegliati = BatchSorvegliati {
        rimasti: 32,
        precedente: None,
    };
    let conteggi = super::conta_in_streaming(sorvegliati).expect("conteggio riuscito");
    assert_eq!(conteggi.batch, 32);
    assert_eq!(conteggi.righe, 96);
}

/// Il sorvegliante **funziona**: un consumatore che accumula lo fa fallire.
///
/// Senza questa meta', il test qui sopra passerebbe anche con una `Weak` che
/// non si promuove mai per un'altra ragione — e sarebbe di nuovo un falso
/// oracolo.
#[test]
#[should_panic(expected = "il batch precedente e' ancora vivo")]
fn il_sorvegliante_accusa_un_consumatore_che_accumula() {
    use std::sync::Weak;

    use plenora_core::arrow::array::{Array, ArrayRef};

    struct BatchSorvegliati {
        rimasti: usize,
        precedente: Option<Weak<dyn Array>>,
    }

    impl Iterator for BatchSorvegliati {
        type Item = RecordBatch;

        fn next(&mut self) -> Option<Self::Item> {
            if let Some(precedente) = self.precedente.take() {
                assert!(
                    precedente.upgrade().is_none(),
                    "il batch precedente e' ancora vivo: il conteggio lo trattiene"
                );
            }
            if self.rimasti == 0 {
                return None;
            }
            self.rimasti -= 1;
            let colonna: ArrayRef = Arc::new(UInt64Array::from(vec![1_u64]));
            self.precedente = Some(Arc::downgrade(&colonna));
            let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::UInt64, false)]));
            Some(RecordBatch::try_new(schema, vec![colonna]).expect("batch valido"))
        }
    }

    // Un consumatore che accumula: e' cio' che il sorvegliante deve accusare.
    // Il `Vec` e' il punto — tiene vivi i batch — quindi non si sostituisce con
    // un conteggio, che li lascerebbe cadere e non proverebbe niente.
    let mut accumulati: Vec<RecordBatch> = Vec::new();
    for prodotto in (BatchSorvegliati {
        rimasti: 4,
        precedente: None,
    }) {
        accumulati.push(prodotto);
    }
    assert_eq!(accumulati.len(), 4);
}

// ---------------------------------------------------------------------------
// Passo 6: schema
// ---------------------------------------------------------------------------

/// Uno schema con metadati incoerenti fa fallire il passo 6 **da solo**.
///
/// Il caso e' distinto dal passo 7: qui il contratto non si **ricostruisce**,
/// mentre li' si ricostruisce e non e' quello atteso. Senza un caso proprio la
/// suite salterebbe dal 5-bis al 7, e un passo 6 rimosso passerebbe inosservato
/// — l'errore arriverebbe comunque, dal confronto successivo, per la ragione
/// sbagliata.
#[test]
fn uno_schema_incoerente_fa_fallire_la_ricostruzione_del_contratto() {
    // Metadato `geo` senza l'estensione geoarrow: `contract_from_arrow_schema`
    // lo rifiuta come incoerente.
    let campo = Field::new("forma", DataType::Binary, true).with_metadata(
        std::iter::once((
            plenora_core::contract::arrow_metadata::GEO_METADATA_KEY.to_owned(),
            r#"{"crs":"EPSG:4326"}"#.to_owned(),
        ))
        .collect(),
    );
    let schema_rotto: SchemaRef = Arc::new(Schema::new(vec![campo]));
    let batch_rotto = RecordBatch::try_new(
        Arc::clone(&schema_rotto),
        vec![Arc::new(plenora_core::arrow::array::BinaryArray::from(
            Vec::<&[u8]>::new(),
        ))],
    )
    .expect("batch valido");

    let byte = artefatto(&schema_rotto, &batch_rotto, 1, Some(&token(UNO)));
    let mut dichiarati = Dichiarati::coerenti(&byte);
    dichiarati.righe = 0;
    dichiarati.contratto = DataContract::tabular(schema_rotto);

    let errore = esegui(&byte, &dichiarati).expect_err("deve fallire");
    // Non e' il messaggio del passo 7: il contratto non e' arrivato a esistere.
    assert!(
        !errore.contains("non e' quello atteso dal piano"),
        "deve fallire alla ricostruzione, non al confronto: {errore}"
    );
}

// ---------------------------------------------------------------------------
// L'harness del fuzz raggiunge davvero i passi profondi
// ---------------------------------------------------------------------------

/// L'harness del fuzz **accetta** un artefatto coerente con le proprie attese.
///
/// Il corpus del fuzzer non e' versionato (`/fuzz/corpus` e' ignorato), quindi
/// non esiste un seed committato che garantisca al target di produrre un Arrow
/// IPC ben formato con lo schema, i conteggi e il token che l'harness fissa.
/// Questo caso e' il sostituto, e pretende `Ok(true)`: un `Ok(false)` direbbe
/// che l'harness si e' fermato prima, e nessuno se ne accorgerebbe.
///
/// L'artefatto viene da `interni::artefatto_di_prova`, cioe' dalla **stessa**
/// definizione da cui l'harness ricava le attese: costruirlo qui a mano
/// avrebbe creato due fixture libere di divergere, e la divergenza avrebbe
/// spento la prova senza rompere il test.
#[test]
fn l_harness_del_fuzz_accetta_un_artefatto_coerente_con_le_sue_attese() {
    let byte = crate::interni::artefatto_di_prova();
    assert_eq!(
        crate::interni::verifica_artefatto_ostile(&byte),
        Ok(true),
        "l'harness deve poter concludere con un'accettazione"
    );
}

/// E **rifiuta** un artefatto che con quelle attese non c'entra.
///
/// Senza questa meta', il caso qui sopra passerebbe anche con un harness che
/// rende `Ok(true)` sempre.
#[test]
fn l_harness_del_fuzz_rifiuta_un_artefatto_estraneo() {
    let byte = artefatto(&schema(), &batch(), 1, Some(&token(DUE)));
    assert_eq!(crate::interni::verifica_artefatto_ostile(&byte), Ok(false));
}

/// Il workspace dell'harness e' **riusato** e **troncato** a ogni scrittura.
///
/// La sequenza e' valido → corto → valido, e prova due cose distinte:
///
/// - **riuso**: il percorso reso e' sempre lo stesso, quindi il workspace nasce
///   una volta per thread e non per iterazione. Senza, una campagna da milioni
///   di casi creerebbe altrettante directory, e il traffico sul filesystem
///   crescerebbe con gli ingressi invece di restare costante;
/// - **nessun byte residuo**: dopo l'ingresso corto il file sul disco e'
///   **esattamente** quello corto. Senza troncamento conserverebbe la coda del
///   valido che lo precede, e il verificatore leggerebbe un artefatto che il
///   fuzzer non ha mai prodotto — irriproducibile per costruzione.
///
/// La lettura del file e' il punto: asserire sul solo esito non
/// distinguerebbe un troncamento riuscito da un residuo che viene comunque
/// rifiutato.
#[test]
fn il_workspace_dell_harness_e_riusato_e_troncato() {
    let lungo = crate::interni::artefatto_di_prova();
    assert!(lungo.len() > 16, "l'artefatto di prova deve essere lungo");
    let corto: &[u8] = b"ARROW1";

    let primo = crate::interni::scrivi_nel_workspace(&lungo).expect("prima scrittura");
    assert_eq!(std::fs::read(&primo).expect("rilettura"), lungo);

    let secondo = crate::interni::scrivi_nel_workspace(corto).expect("seconda scrittura");
    assert_eq!(secondo, primo, "il workspace non e' riusato");
    assert_eq!(
        std::fs::read(&secondo).expect("rilettura"),
        corto,
        "il file conserva la coda della scrittura precedente"
    );

    let terzo = crate::interni::scrivi_nel_workspace(&lungo).expect("terza scrittura");
    assert_eq!(terzo, primo, "il workspace non e' riusato");
    assert_eq!(std::fs::read(&terzo).expect("rilettura"), lungo);
}

/// E il giro completo regge sulla stessa sequenza.
///
/// **Non e' l'oracolo del troncamento**, e dirlo sarebbe sbagliato: senza
/// troncamento l'ingresso corto lascerebbe in coda i byte del valido che lo
/// precede, e il risultato verrebbe respinto lo stesso — l'esito non
/// cambierebbe. L'oracolo discriminante e' quello qui sopra, che **rilegge il
/// file** e pretende che sia esattamente l'ingresso corto.
///
/// Qui si prova un'altra cosa, che quel caso non copre: che il verificatore
/// riapra ogni volta il percorso riusato e ne renda il verdetto giusto —
/// accettato, rifiutato, accettato di nuovo — cioe' che il riuso del workspace
/// non lasci il giro in uno stato da cui non si torna.
#[test]
fn valido_corto_valido_rende_gli_esiti_attesi() {
    let valido = crate::interni::artefatto_di_prova();
    assert_eq!(crate::interni::verifica_artefatto_ostile(&valido), Ok(true));
    assert_eq!(
        crate::interni::verifica_artefatto_ostile(b"ARROW1"),
        Ok(false)
    );
    assert_eq!(crate::interni::verifica_artefatto_ostile(&valido), Ok(true));
}

// ---------------------------------------------------------------------------
// La classificazione dell'esito nell'harness del fuzz
// ---------------------------------------------------------------------------

/// Un rifiuto ordinario del verificatore e' `Ok(false)`, non un errore.
#[test]
fn i_rifiuti_ordinari_sono_verdetti_non_guasti() {
    use plenora_core::error::PlenoraError;

    for errore in [
        PlenoraError::DataMapping("artefatto rifiutato".to_owned()),
        PlenoraError::ResourceLimit("tetto superato".to_owned()),
    ] {
        assert_eq!(crate::interni::classifica_esito(Err(errore)), Ok(false));
    }
    assert_eq!(crate::interni::classifica_esito(Ok(())), Ok(true));
}

/// Un errore di **I/O** e' un guasto dell'harness, non un rifiuto.
///
/// E' il caso che `is_ok()` appiattirebbe: un disco che non risponde
/// diventerebbe un ingresso respinto, e il fuzzer conterebbe come esercitato un
/// percorso che non attraversa.
#[test]
fn un_errore_di_io_e_un_guasto_dell_harness() {
    use plenora_core::error::PlenoraError;

    let esito = crate::interni::classifica_esito(Err(PlenoraError::Io(std::io::Error::other(
        "disco assente",
    ))));
    let rottura = esito.expect_err("l'I/O non e' un rifiuto");
    assert!(rottura.contains("I/O"), "{rottura}");
}

/// Una categoria che il contratto del verificatore **non dichiara** e' un
/// guasto, e il messaggio la nomina.
///
/// Senza questo ramo una categoria nuova verrebbe letta come rifiuto ordinario
/// e nessuno se ne accorgerebbe: il target continuerebbe a passare mentre il
/// contratto e' cambiato sotto di lui.
#[test]
fn una_categoria_non_dichiarata_e_un_guasto() {
    use plenora_core::error::PlenoraError;

    let esito = crate::interni::classifica_esito(Err(PlenoraError::InvalidPlan(
        "categoria fuori contratto".to_owned(),
    )));
    let rottura = esito.expect_err("una categoria non dichiarata non e' un rifiuto");
    assert!(rottura.contains("non dichiarata"), "{rottura}");
    // Il nome della categoria e' una stringa stabile del nostro canone: puo'
    // comparire, e serve a chi legge per capire che cosa e' cambiato.
    assert!(rottura.contains("invalid_plan"), "{rottura}");
}
