//! Test del lettore di confine Arrow IPC.

use std::fs;
use std::io::Write as _;
use std::sync::Arc;

use plenora_core::arrow::array::{
    types::Int32Type, Array, DictionaryArray, Int32Array, Int64Array, RecordBatch, StringArray,
};
use plenora_core::arrow::ipc::writer::{FileWriter, StreamWriter};
use plenora_core::arrow::schema::{DataType, Field, Schema};

use super::*;

fn batch() -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
    RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![1_i64, 2, 3]))])
        .expect("batch valido")
}

fn write_file_format(path: &std::path::Path) {
    let batch = batch();
    let file = fs::File::create(path).expect("create");
    let mut writer = FileWriter::try_new(file, &batch.schema()).expect("writer");
    writer.write(&batch).expect("write");
    writer.finish().expect("finish");
}

fn write_stream_format(path: &std::path::Path) {
    let batch = batch();
    let file = fs::File::create(path).expect("create");
    let mut writer = StreamWriter::try_new(file, &batch.schema()).expect("writer");
    writer.write(&batch).expect("write");
    writer.finish().expect("finish");
}

/// Batch con una colonna dictionary: il file format emette allora messaggi
/// `DictionaryBatch` intercalati ai `RecordBatch`, ed e' proprio il caso in
/// cui una scansione sequenziale del framing puo' sbagliare a contare i
/// messaggi.
/// Il file format ammette UN solo dizionario per campo su tutti i batch:
/// i due batch condividono il dizionario e differiscono per le sole chiavi.
fn dictionary_batch(keys: &[i32]) -> RecordBatch {
    let dictionary = StringArray::from(vec!["alfa", "beta", "gamma"]);
    let array = DictionaryArray::<Int32Type>::try_new(
        Int32Array::from(keys.to_vec()),
        Arc::new(dictionary),
    )
    .expect("dictionary valido");
    let schema = Arc::new(Schema::new(vec![Field::new(
        "etichetta",
        array.data_type().clone(),
        false,
    )]));
    RecordBatch::try_new(schema, vec![Arc::new(array)]).expect("batch valido")
}

#[test]
fn il_confine_accetta_file_multi_batch_con_dizionari() {
    // Piu' messaggi in sequenza, con i `DictionaryBatch` che il file format
    // intercala: la camminata sul framing deve coprirli tutti e fermarsi
    // esattamente al footer, senza rifiutare un file legittimo.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("dizionari.arrow");
    let primo = dictionary_batch(&[0, 1]);
    let secondo = dictionary_batch(&[2, 0]);
    let file = fs::File::create(&path).expect("create");
    let mut writer = FileWriter::try_new(file, &primo.schema()).expect("writer");
    writer.write(&primo).expect("write");
    writer.write(&secondo).expect("write");
    writer.finish().expect("finish");

    let (schema, batches) = open(&path, &IpcLimits::default()).expect("apertura multi-batch");
    assert_eq!(schema.fields().len(), 1);
    let letti: Vec<_> = batches.map(|batch| batch.expect("batch")).collect();
    assert_eq!(letti.len(), 2);
    assert_eq!(letti.iter().map(RecordBatch::num_rows).sum::<usize>(), 4);
}

#[test]
fn i_file_prodotti_da_arrow_passano_il_confine_in_entrambi_i_formati() {
    // La pre-validazione del framing non deve rifiutare l'output legittimo
    // dei writer di arrow: e' il controllo di non-regressione del confine.
    let directory = tempfile::tempdir().expect("tempdir");
    let file_path = directory.path().join("in.arrow");
    write_file_format(&file_path);
    assert_eq!(sniff_format(&file_path).expect("sniff"), IpcFormat::File);
    let (schema, batches) = open(&file_path, &IpcLimits::default()).expect("apertura file format");
    assert_eq!(schema.fields().len(), 1);
    let rows: usize = batches.map(|batch| batch.expect("batch").num_rows()).sum();
    assert_eq!(rows, 3);

    let stream_path = directory.path().join("in.stream");
    write_stream_format(&stream_path);
    assert_eq!(
        sniff_format(&stream_path).expect("sniff"),
        IpcFormat::Stream
    );
    let (schema, batches) =
        open(&stream_path, &IpcLimits::default()).expect("apertura stream format");
    assert_eq!(schema.fields().len(), 1);
    let rows: usize = batches.map(|batch| batch.expect("batch").num_rows()).sum();
    assert_eq!(rows, 3);
}

#[test]
fn il_confine_rifiuta_i_metadati_oltre_il_tetto_prima_di_allocare() {
    // Messaggio stream con metadata_len dichiarato enorme: senza la
    // pre-validazione arrow tenterebbe l'allocazione. Il confine lo rifiuta
    // leggendo solo gli 8 byte del prefisso.
    //
    // Ottavo giro: le due diagnosi vanno distinte. Una lunghezza che il file
    // NON contiene descrive un file troncato (`data_mapping`); una lunghezza
    // che il file contiene davvero ma sfora il tetto e' un limite di risorsa
    // (`resource_limit`). Entrambe rifiutano prima di allocare — e' la
    // proprieta' che questo test difende — ma dicono al chiamante due cose
    // diverse, e prima dicevano la stessa.
    let directory = tempfile::tempdir().expect("tempdir");

    // (a) dichiarazione impossibile in 40 byte di file: TRONCATO.
    let troncato = directory.path().join("ostile.stream");
    let mut file = fs::File::create(&troncato).expect("create");
    file.write_all(&0xFFFF_FFFF_u32.to_le_bytes())
        .expect("marker");
    file.write_all(&u32::MAX.to_le_bytes()).expect("len");
    file.write_all(&[0_u8; 32]).expect("coda");
    drop(file);
    let error = open(&troncato, &IpcLimits::default()).expect_err("dichiarazione impossibile");
    assert!(
        error.to_string().contains("troncato"),
        "una lunghezza che il file non contiene e' un troncamento: {error}"
    );

    // (b) dichiarazione CONTENUTA nel file ma oltre il tetto: RISORSA.
    let oltre = directory.path().join("oltre-il-tetto.stream");
    let mut file = fs::File::create(&oltre).expect("create");
    file.write_all(&0xFFFF_FFFF_u32.to_le_bytes())
        .expect("marker");
    file.write_all(&64_u32.to_le_bytes()).expect("len");
    file.write_all(&[0_u8; 64])
        .expect("metadati dichiarati e presenti");
    drop(file);
    let limits = IpcLimits {
        max_metadata_bytes: 16,
        ..IpcLimits::default()
    };
    let error = open(&oltre, &limits).expect_err("metadati oltre il tetto");
    assert!(
        error.to_string().contains("oltre il limite"),
        "atteso il superamento del tetto sui metadati, ottenuto: {error}"
    );
}

#[test]
fn il_confine_rifiuta_un_file_format_con_footer_incoerente() {
    // Magic corretto in testa e in coda ma footer_len che sfonda la regione
    // dei messaggi: rifiutato prima che arrow legga il footer.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("footer.arrow");
    let mut file = fs::File::create(&path).expect("create");
    file.write_all(b"ARROW1\0\0").expect("magic");
    file.write_all(&[0_u8; 16]).expect("corpo");
    file.write_all(&1_000_000_i32.to_le_bytes())
        .expect("footer_len");
    file.write_all(b"ARROW1").expect("magic finale");
    drop(file);

    let error = open_with_format(&path, IpcFormat::File, &IpcLimits::default())
        .expect_err("footer incoerente");
    assert!(
        error.to_string().contains("troncato"),
        "atteso stream troncato, ottenuto: {error}"
    );
}

/// Posizione, dentro il file, del campo `offset` del primo `Block` del
/// footer.
///
/// I `Block` sono struct flatbuffer inline da 24 byte (offset i64,
/// `metaDataLength` i32 + padding, `bodyLength` i64): si riconoscono per i
/// vincoli che tutte e tre le lunghezze devono soddisfare insieme, il che
/// rende praticamente impossibile scambiare per un blocco dei byte a caso.
fn footer_start_of(bytes: &[u8]) -> u64 {
    let trailer = bytes.len() - 10;
    let footer_len = u64::try_from(i32::from_le_bytes(
        bytes[trailer..trailer + 4].try_into().expect("footer_len"),
    ))
    .expect("footer_len non negativo");
    u64::try_from(trailer).expect("dimensione del file") - footer_len
}

fn find_footer_block(bytes: &[u8]) -> usize {
    let footer_start = footer_start_of(bytes);
    let inizio = usize::try_from(footer_start).expect("footer_start");
    for index in inizio..(bytes.len() - 10 - 24) {
        let offset = i64::from_le_bytes(bytes[index..index + 8].try_into().expect("offset"));
        let metadata = i32::from_le_bytes(bytes[index + 8..index + 12].try_into().expect("meta"));
        let body = i64::from_le_bytes(bytes[index + 16..index + 24].try_into().expect("body"));
        let (Ok(offset), Ok(metadata), Ok(body)) = (
            u64::try_from(offset),
            u64::try_from(metadata),
            u64::try_from(body),
        ) else {
            continue;
        };
        let plausibile = offset >= 8
            && offset % 8 == 0
            && offset < footer_start
            && metadata > 0
            && metadata <= 4096
            && offset + metadata + body <= footer_start;
        if plausibile {
            return index;
        }
    }
    panic!("nessun Block riconosciuto nel footer");
}

#[test]
fn il_confine_rifiuta_un_footer_che_punta_fuori_dalla_regione_dati() {
    // E' l'aggiramento che una scansione sequenziale non vedeva: i messaggi
    // fisici restano validi, ma `FileReader` non li percorre — salta agli
    // `offset` dei blocchi del footer. Spostando un blocco fuori dalla
    // regione dati, arrow leggerebbe byte che nessuno ha validato.
    let directory = tempfile::tempdir().expect("tempdir");
    let valido = directory.path().join("valido.arrow");
    write_file_format(&valido);
    let mut bytes = fs::read(&valido).expect("read");
    let block = find_footer_block(&bytes);
    let footer_start = footer_start_of(&bytes);
    bytes[block..block + 8].copy_from_slice(&footer_start.to_le_bytes());
    let ostile = directory.path().join("footer-fuori.arrow");
    fs::write(&ostile, &bytes).expect("write");

    let error = open_with_format(&ostile, IpcFormat::File, &IpcLimits::default())
        .expect_err("blocco fuori");
    assert!(
        error.to_string().contains("footer IPC incoerente"),
        "atteso il rifiuto del blocco fuori regione, ottenuto: {error}"
    );
}

#[test]
fn il_confine_rifiuta_un_footer_con_blocchi_sovrapposti() {
    // Due blocchi che descrivono la stessa regione: arrow la leggerebbe con
    // due interpretazioni diverse, e la validazione ne avrebbe vista una sola.
    let directory = tempfile::tempdir().expect("tempdir");
    let valido = directory.path().join("due-batch.arrow");
    let batch = batch();
    let file = fs::File::create(&valido).expect("create");
    let mut writer = FileWriter::try_new(file, &batch.schema()).expect("writer");
    writer.write(&batch).expect("write");
    writer.write(&batch).expect("write");
    writer.finish().expect("finish");

    let mut bytes = fs::read(&valido).expect("read");
    let primo = find_footer_block(&bytes);
    // Il secondo blocco segue immediatamente il primo nel vettore.
    let secondo = primo + 24;
    let offset_primo: [u8; 8] = bytes[primo..primo + 8].try_into().expect("offset");
    bytes[secondo..secondo + 8].copy_from_slice(&offset_primo);
    let ostile = directory.path().join("sovrapposti.arrow");
    fs::write(&ostile, &bytes).expect("write");

    let error = open_with_format(&ostile, IpcFormat::File, &IpcLimits::default())
        .expect_err("blocchi sovrapposti");
    assert!(
        error.to_string().contains("footer IPC incoerente"),
        "atteso il rifiuto dei blocchi sovrapposti, ottenuto: {error}"
    );
}

#[test]
fn il_confine_applica_i_tetti_su_body_e_numero_di_messaggi() {
    // I tetti si applicano alle lunghezze DICHIARATE nei blocchi, cioe' prima
    // che arrow legga un byte del corpo: `max_batch_bytes` dell'executor
    // misura invece un `RecordBatch` gia' costruito.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("limiti.arrow");
    let batch = batch();
    let file = fs::File::create(&path).expect("create");
    let mut writer = FileWriter::try_new(file, &batch.schema()).expect("writer");
    writer.write(&batch).expect("write");
    writer.write(&batch).expect("write");
    writer.finish().expect("finish");

    let body_stretto = IpcLimits {
        max_body_bytes: 1,
        ..IpcLimits::default()
    };
    let error = open_with_format(&path, IpcFormat::File, &body_stretto).expect_err("body oltre");
    assert!(
        error.to_string().contains("body del messaggio IPC"),
        "atteso il tetto sul body, ottenuto: {error}"
    );

    let pochi_messaggi = IpcLimits {
        max_messages: 1,
        ..IpcLimits::default()
    };
    let error =
        open_with_format(&path, IpcFormat::File, &pochi_messaggi).expect_err("messaggi oltre");
    assert!(
        error.to_string().contains("messaggi IPC"),
        "atteso il tetto sul numero di messaggi, ottenuto: {error}"
    );

    // Con i tetti di default lo stesso file resta leggibile.
    assert!(open_with_format(&path, IpcFormat::File, &IpcLimits::default()).is_ok());
}

#[test]
fn il_confine_esige_che_i_blocchi_dichiarino_le_lunghezze_esatte() {
    // Terzo giro, finding 1: i blocchi del footer venivano controllati per
    // CONTENIMENTO (il messaggio ci sta dentro la regione dichiarata), non per
    // uguaglianza. Un `bodyLength` piu' grande del vero lascia arrow
    // interpretare come corpo del batch dei byte che la validazione ha
    // attribuito al messaggio successivo — cioe' contenuto mai controllato,
    // pur restando tutto dentro la regione dati.
    let directory = tempfile::tempdir().expect("tempdir");
    let valido = directory.path().join("valido.arrow");
    write_file_format(&valido);
    let originale = fs::read(&valido).expect("read");
    let block = find_footer_block(&originale);

    // `bodyLength` gonfiato di 8 byte: resta dentro la regione dati (il
    // footer segue), ma non e' piu' la lunghezza del messaggio.
    let mut bytes = originale.clone();
    let body = i64::from_le_bytes(bytes[block + 16..block + 24].try_into().expect("body"));
    bytes[block + 16..block + 24].copy_from_slice(&(body + 8).to_le_bytes());
    let ostile = directory.path().join("body-gonfiato.arrow");
    fs::write(&ostile, &bytes).expect("write");
    let error = open_with_format(&ostile, IpcFormat::File, &IpcLimits::default())
        .expect_err("body diverso");
    assert!(
        error.to_string().contains("footer IPC incoerente"),
        "atteso il rifiuto del bodyLength non esatto, ottenuto: {error}"
    );

    // Stesso discorso per `metaDataLength`: un valore piu' grande sposta
    // l'inizio del corpo rispetto a dove il messaggio dice che sia.
    let mut bytes = originale;
    let metadata = i32::from_le_bytes(bytes[block + 8..block + 12].try_into().expect("meta"));
    bytes[block + 8..block + 12].copy_from_slice(&(metadata + 8).to_le_bytes());
    let ostile = directory.path().join("metadata-gonfiato.arrow");
    fs::write(&ostile, &bytes).expect("write");
    let error = open_with_format(&ostile, IpcFormat::File, &IpcLimits::default())
        .expect_err("metadata diverso");
    assert!(
        error.to_string().contains("footer IPC incoerente"),
        "atteso il rifiuto del metaDataLength non esatto, ottenuto: {error}"
    );
}

#[test]
fn il_confine_rifiuta_i_byte_dopo_la_fine_dello_stream() {
    // Terzo giro, finding 9: il marcatore di fine stream chiudeva la
    // validazione con un `Ok` incondizionato, quindi tutto cio' che seguiva
    // non veniva mai guardato. Un reader che non si fermasse li' — o un
    // consumatore diverso dello stesso file — leggerebbe byte non validati.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("dopo-eos.arrow");
    write_stream_format(&path);
    let mut bytes = fs::read(&path).expect("read");
    bytes.extend_from_slice(&[0xff; 32]);
    fs::write(&path, &bytes).expect("write");

    let error = open_with_format(&path, IpcFormat::Stream, &IpcLimits::default())
        .expect_err("byte dopo EOS");
    assert!(
        error.to_string().contains("fine stream"),
        "atteso il rifiuto dei byte dopo EOS, ottenuto: {error}"
    );
}

#[test]
fn il_confine_applica_il_tetto_sui_record_batch_anche_al_file_format() {
    // Quarto giro: negli stream i record batch erano contati per tipo di
    // header, ma nel file format i blocchi di dizionari e di record batch
    // finiscono in un vettore solo e si controllava il solo `max_messages`.
    // Un file con piu' batch di quanti il piano ne ammetta superava il
    // confine e veniva fermato solo dopo averne materializzato un altro.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("tre-batch.arrow");
    let batch = batch();
    let file = fs::File::create(&path).expect("create");
    let mut writer = FileWriter::try_new(file, &batch.schema()).expect("writer");
    for _ in 0..3 {
        writer.write(&batch).expect("write");
    }
    writer.finish().expect("finish");

    let stretto = IpcLimits {
        max_record_batches: 2,
        ..IpcLimits::default()
    };
    let error = open_with_format(&path, IpcFormat::File, &stretto).expect_err("record batch oltre");
    assert!(
        error.to_string().contains("record batch IPC"),
        "atteso il tetto sui record batch, ottenuto: {error}"
    );

    // Il tetto e' sui RECORD BATCH, non sui messaggi: lo schema non conta.
    let esatto = IpcLimits {
        max_record_batches: 3,
        ..IpcLimits::default()
    };
    assert!(
        open_with_format(&path, IpcFormat::File, &esatto).is_ok(),
        "tre batch con tetto tre devono passare: lo schema non e' un record batch"
    );

    // E i dizionari non consumano il tetto: un file con dizionari e due
    // batch passa con `max_record_batches = 2` benche' i messaggi siano di
    // piu'.
    let con_dizionari = directory.path().join("dizionari-due.arrow");
    let primo = dictionary_batch(&[0, 1]);
    let secondo = dictionary_batch(&[2, 0]);
    let file = fs::File::create(&con_dizionari).expect("create");
    let mut writer = FileWriter::try_new(file, &primo.schema()).expect("writer");
    writer.write(&primo).expect("write");
    writer.write(&secondo).expect("write");
    writer.finish().expect("finish");
    let due = IpcLimits {
        max_record_batches: 2,
        ..IpcLimits::default()
    };
    assert!(
        open_with_format(&con_dizionari, IpcFormat::File, &due).is_ok(),
        "i DictionaryBatch non devono consumare il tetto sui record batch"
    );
}

#[test]
fn il_confine_rifiuta_un_file_troppo_corto_per_il_trailer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("corto.arrow");
    fs::write(&path, b"ARROW1\0\0").expect("write");
    let error = open_with_format(&path, IpcFormat::File, &IpcLimits::default())
        .expect_err("file troppo corto");
    assert!(
        error.to_string().contains("troncato"),
        "atteso stream troncato, ottenuto: {error}"
    );
}

#[test]
fn i_tetti_del_piano_limitano_anche_i_metadati() {
    // Sesto giro, finding 4: `max_metadata_bytes` restava al default (64 MiB)
    // anche con un budget di memoria di pochi byte. I metadati sono
    // un'allocazione come il body, e arrow li alloca PRIMA di qualunque
    // batch: un tetto che non li copre non e' un tetto.
    let stretti = limits_from_plan(
        &plenora_core::limits::Limits {
            max_governed_memory_bytes: 1_024,
            ..plenora_core::limits::Limits::default()
        },
        64 * 1024 * 1024,
    );
    assert_eq!(
        stretti.max_metadata_bytes, 1_024,
        "i metadati devono seguire il budget di memoria"
    );
    assert_eq!(stretti.max_body_bytes, 1_024);

    // Con un budget largo resta il default: il tetto e' il PIU' STRETTO fra i
    // due, non il budget in assoluto.
    let larghi = limits_from_plan(
        &plenora_core::limits::Limits {
            max_governed_memory_bytes: u64::MAX,
            max_payload_bytes: u64::MAX,
            ..plenora_core::limits::Limits::default()
        },
        64 * 1024 * 1024,
    );
    assert_eq!(
        larghi.max_metadata_bytes,
        IpcLimits::default().max_metadata_bytes
    );

    // E un file i cui metadati eccedono il budget viene rifiutato prima di
    // allocarli. Il messaggio di schema di un file minimo supera comunque i
    // 64 byte, quindi il budget qui e' volutamente minuscolo.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("metadati.arrow");
    write_file_format(&path);
    let minuscoli = limits_from_plan(
        &plenora_core::limits::Limits {
            max_governed_memory_bytes: 64,
            ..plenora_core::limits::Limits::default()
        },
        64 * 1024 * 1024,
    );
    assert_eq!(minuscoli.max_metadata_bytes, 64);
    let error =
        open_with_format(&path, IpcFormat::File, &minuscoli).expect_err("metadati oltre il budget");
    assert!(
        error.to_string().contains("oltre il limite"),
        "atteso il rifiuto dei metadati, ottenuto: {error}"
    );
}

#[test]
fn il_budget_di_memoria_limita_il_confine_anche_senza_piano_v4() {
    // Sesto giro, finding 5: il percorso legacy costruiva `IpcLimits::default()`
    // e quindi ammetteva 64 MiB per messaggio qualunque fosse
    // `max_governed_memory_bytes` del piano.
    let stretti = limits_from_memory_budget(2_048);
    assert_eq!(stretti.max_body_bytes, 2_048);
    assert_eq!(stretti.max_metadata_bytes, 2_048);

    // Un budget enorme non alza i default: il confine resta il piu' stretto.
    let larghi = limits_from_memory_budget(usize::MAX);
    assert_eq!(larghi.max_body_bytes, IpcLimits::default().max_body_bytes);
    assert_eq!(
        larghi.max_metadata_bytes,
        IpcLimits::default().max_metadata_bytes
    );
}

/// I tre tetti sui custom metadata sono limiti, non file rotti.
///
/// La distinzione non e' accademica: `DataMapping` dice al chiamante che il
/// file e' corrotto, e lo manda a cercare un difetto che non c'e'. La forma
/// non ammessa resta `DataMapping`, perche' li' il file lo e' davvero.
///
/// Ogni caso verifica **categoria e fase** insieme: il tag del confine deve
/// vincere sulla derivazione per variante, e questi errori nascono leggendo.
#[test]
fn i_tetti_dei_custom_metadata_sono_limiti_di_risorse() {
    use plenora_core::error::{ErrorCategory, ErrorPhase};

    let limiti = [
        ArrowTransportError::IpcTooManyMetadataPairs(300, 256),
        ArrowTransportError::IpcMetadataKeyTooLarge(200, 128),
        ArrowTransportError::IpcMetadataValueTooLarge(70_000, 65_536),
    ];
    for errore in limiti {
        let tradotto = read_error(errore);
        assert_eq!(
            tradotto.category(),
            ErrorCategory::ResourceLimit,
            "atteso ResourceLimit"
        );
        assert_eq!(
            tradotto.phase(),
            ErrorPhase::Read,
            "atteso ErrorPhase::Read"
        );
    }

    let forme = [
        ArrowTransportError::IpcMetadataInvalid("chiave assente"),
        ArrowTransportError::IpcSchemaInvalid("schema senza il campo fields"),
    ];
    for errore in forme {
        let tradotto = read_error(errore);
        assert_eq!(
            tradotto.category(),
            ErrorCategory::DataMapping,
            "una forma non ammessa e' un file rotto, non un tetto"
        );
        assert_eq!(tradotto.phase(), ErrorPhase::Read);
    }
}

/// Un errore del filesystem non e' un file corrotto.
///
/// La versione precedente di `read_error` classificava `Io` come
/// `DataMapping` e ne teneva il solo testo: un disco che non risponde durante
/// `read_at` o `rewind` diventava «il file e' rotto», e mandava chi legge a
/// cercare un difetto nei dati che non c'era.
#[test]
fn un_errore_di_io_resta_io_e_conserva_la_causa() {
    use plenora_core::error::{ErrorCategory, ErrorPhase};
    use std::io::ErrorKind;

    let sorgente = std::io::Error::new(ErrorKind::PermissionDenied, "permesso negato");
    let tradotto = read_error(ArrowTransportError::Io(sorgente));

    assert_eq!(
        tradotto.category(),
        ErrorCategory::Io,
        "atteso ErrorCategory::Io"
    );
    assert_eq!(
        tradotto.phase(),
        ErrorPhase::Read,
        "la fase del confine resta Read"
    );
    // `with_phase` AVVOLGE in `Tagged` invece di marcare in posto, quindi la
    // causa sta un livello sotto. La prima stesura di questo test guardava
    // l'involucro e falliva: e' il genere di assunzione sull'API che solo un
    // test negativo scopre, perche' `category()` ricorre sulla causa e da
    // sola sembrava confermare.
    let dentro = match &tradotto {
        PlenoraError::Tagged { source, .. } => source.as_ref(),
        altro => altro,
    };
    // La causa e' preservata, non stampata: e' la ragione per cui la funzione
    // consuma l'errore invece di prenderlo a prestito.
    assert!(
        matches!(dentro, PlenoraError::Io(io) if io.kind() == ErrorKind::PermissionDenied),
        "lo std::io::Error originale non e' stato conservato"
    );
}

/// La diagnostica di riga non cambia la categoria di cio' che avvolge.
#[test]
fn la_diagnostica_di_riga_non_maschera_un_limite() {
    use plenora_core::error::{ErrorCategory, ErrorPhase};

    let avvolto = ArrowTransportError::IpcTooManyMetadataPairs(300, 256);
    let tradotto = read_error(avvolto);
    assert_eq!(tradotto.category(), ErrorCategory::ResourceLimit);
    assert_eq!(tradotto.phase(), ErrorPhase::Read);
}
