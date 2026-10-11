//! La classe dei decoder di `parquet` che si fidano dei valori letti dal
//! file (fork `vendor/parquet-60.0.0-eof`, `patches/parquet-decoder.patch`).
//!
//! Ogni prova costruisce a mano un Parquet minimo e **valido nella forma**:
//! footer e header di pagina in Thrift compatto scritti campo per campo, una
//! colonna `REQUIRED` `x`, una pagina di dati v1 senza livelli e, se serve,
//! una pagina di dizionario. Cambia solo il contenuto codificato della
//! pagina, cioè proprio ciò che il decoder legge. Ogni caso pretende un
//! errore del decoder (non il panico che la barriera di lettura convertirebbe
//! in `DataMapping`, «parquet in panico»), o, dove il difetto era
//! un'accettazione silenziosa, il rifiuto di un file che prima si leggeva.
//!
//! Senza la patch: panici (`attempt to add with overflow`, `index out of
//! bounds`, `range end index`, `attempt to multiply with overflow`) e due
//! letture riuscite che non dovevano riuscire (il prefisso oltre il valore
//! precedente, il resto di `BYTE_STREAM_SPLIT`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod comune;

use std::fmt::Write as _;
use std::sync::Arc;

use parquet::arrow::arrow_reader::statistics::StatisticsConverter;
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::column::reader::get_typed_column_reader;
use parquet::data_type::{ByteArrayType, DoubleType, Int32Type};
use parquet::file::reader::{FileReader, SerializedFileReader};
use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::{Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::ErrorCategory;
use plenora_io::{leggi_tabella_con_limiti, Formato, LimitiLettura};

use comune::cartella;

// --- Thrift compatto --------------------------------------------------------

const I32: u8 = 5;
const I64: u8 = 6;
const BINARIO: u8 = 8;
const ELENCO: u8 = 9;
const STRUCT: u8 = 12;

fn varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let sette = u8::try_from(n & 0x7F).unwrap();
        n >>= 7;
        if n == 0 {
            out.push(sette);
            return;
        }
        out.push(sette | 0x80);
    }
}

const fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)).cast_unsigned()
}

/// Una struct Thrift: i campi in ordine crescente di identificatore.
#[derive(Default)]
struct Struttura {
    byte: Vec<u8>,
    ultimo: u8,
}

impl Struttura {
    fn campo(&mut self, id: u8, tipo: u8) {
        let delta = id - self.ultimo;
        assert!(delta > 0 && delta < 16);
        self.byte.push((delta << 4) | tipo);
        self.ultimo = id;
    }
    fn intero(mut self, id: u8, valore: i64, tipo: u8) -> Self {
        self.campo(id, tipo);
        varint(zigzag(valore), &mut self.byte);
        self
    }
    fn i32(self, id: u8, valore: i64) -> Self {
        self.intero(id, valore, I32)
    }
    fn i64(self, id: u8, valore: i64) -> Self {
        self.intero(id, valore, I64)
    }
    fn binario(mut self, id: u8, valore: &[u8]) -> Self {
        self.campo(id, BINARIO);
        varint(valore.len() as u64, &mut self.byte);
        self.byte.extend_from_slice(valore);
        self
    }
    fn testo(self, id: u8, valore: &str) -> Self {
        self.binario(id, valore.as_bytes())
    }
    fn struttura(mut self, id: u8, interna: Self) -> Self {
        self.campo(id, STRUCT);
        self.byte.extend(interna.fine());
        self
    }
    fn elenco(mut self, id: u8, tipo: u8, elementi: Vec<Vec<u8>>) -> Self {
        self.campo(id, ELENCO);
        let quanti = elementi.len();
        assert!(quanti < 15);
        self.byte.push((u8::try_from(quanti).unwrap() << 4) | tipo);
        for elemento in elementi {
            self.byte.extend(elemento);
        }
        self
    }
    fn fine(mut self) -> Vec<u8> {
        self.byte.push(0);
        self.byte
    }
}

fn intero_di_elenco(valore: i64) -> Vec<u8> {
    let mut out = Vec::new();
    varint(zigzag(valore), &mut out);
    out
}

fn testo_di_elenco(valore: &str) -> Vec<u8> {
    let mut out = Vec::new();
    varint(valore.len() as u64, &mut out);
    out.extend_from_slice(valore.as_bytes());
    out
}

// --- Il file ----------------------------------------------------------------

/// Tipi fisici e codifiche (numeri di `parquet.thrift`).
const BYTE_ARRAY: i64 = 6;
const DOUBLE: i64 = 5;
const INT32: i64 = 1;
const FLBA: i64 = 7;
const PLAIN: i64 = 0;
const RLE: i64 = 3;
const DELTA_BINARY_PACKED: i64 = 5;
const DELTA_LENGTH_BYTE_ARRAY: i64 = 6;
const DELTA_BYTE_ARRAY: i64 = 7;
const RLE_DICTIONARY: i64 = 8;
const BYTE_STREAM_SPLIT: i64 = 9;

/// Una colonna `x` `REQUIRED`: tipo fisico, larghezza (per FLBA), tipo
/// convertito con scala e precisione (per i decimali).
struct Colonna {
    tipo: i64,
    larghezza: Option<i64>,
    decimale: Option<(i64, i64)>,
    /// `0` `REQUIRED`, `1` `OPTIONAL` (livelli di definizione nella pagina),
    /// `2` `REPEATED` (livelli di ripetizione e di definizione).
    ripetizione: i64,
    /// Tipo convertito (`converted_type`), per esempio `INT_8`.
    convertito: Option<i64>,
}

const fn colonna(tipo: i64) -> Colonna {
    Colonna {
        tipo,
        larghezza: None,
        decimale: None,
        ripetizione: 0,
        convertito: None,
    }
}

/// Un Parquet di una pagina di dati (`valori` dichiarati, `codifica`,
/// `contenuto`), con un dizionario PLAIN se dato, e statistiche min/max
/// (`min_value`, `max_value`) se date.
fn parquet(
    colonna: &Colonna,
    valori: i64,
    codifica: i64,
    contenuto: &[u8],
    dizionario: Option<(i64, &[u8])>,
    statistiche: Option<(&[u8], &[u8])>,
) -> Vec<u8> {
    let lunghezza = i64::try_from(contenuto.len()).unwrap();
    let header = Struttura::default()
        .i32(1, 0) // DATA_PAGE
        .i32(2, lunghezza)
        .i32(3, lunghezza)
        .struttura(
            5,
            Struttura::default()
                .i32(1, valori)
                .i32(2, codifica)
                .i32(3, RLE)
                .i32(4, RLE),
        )
        .fine();
    parquet_con_header(
        colonna,
        valori,
        codifica,
        contenuto,
        &header,
        dizionario,
        statistiche,
    )
}

/// Come [`parquet`], con l'header della pagina di dati dato.
fn parquet_con_header(
    colonna: &Colonna,
    valori: i64,
    codifica: i64,
    contenuto: &[u8],
    header_dati: &[u8],
    dizionario: Option<(i64, &[u8])>,
    statistiche: Option<(&[u8], &[u8])>,
) -> Vec<u8> {
    let mut file = b"PAR1".to_vec();
    let inizio = i64::try_from(file.len()).unwrap();
    let mut offset_dizionario = None;
    let mut codifiche = vec![intero_di_elenco(codifica)];
    if let Some((quanti, byte)) = dizionario {
        offset_dizionario = Some(i64::try_from(file.len()).unwrap());
        let lunghezza = i64::try_from(byte.len()).unwrap();
        let header = Struttura::default()
            .i32(1, 2) // DICTIONARY_PAGE
            .i32(2, lunghezza)
            .i32(3, lunghezza)
            .struttura(7, Struttura::default().i32(1, quanti).i32(2, PLAIN))
            .fine();
        file.extend(header);
        file.extend_from_slice(byte);
        codifiche.push(intero_di_elenco(PLAIN));
    }
    let offset_dati = i64::try_from(file.len()).unwrap();
    file.extend_from_slice(header_dati);
    file.extend_from_slice(contenuto);
    let pagine = i64::try_from(file.len()).unwrap() - inizio;

    let mut metadati = Struttura::default()
        .i32(1, colonna.tipo)
        .elenco(2, I32, codifiche)
        .elenco(3, BINARIO, vec![testo_di_elenco("x")])
        .i32(4, 0) // UNCOMPRESSED
        .i64(5, valori)
        .i64(6, pagine)
        .i64(7, pagine)
        .i64(9, offset_dati);
    if let Some(offset) = offset_dizionario {
        metadati = metadati.i64(11, offset);
    }
    if let Some((minimo, massimo)) = statistiche {
        metadati = metadati.struttura(
            12,
            Struttura::default().binario(5, massimo).binario(6, minimo),
        );
    }
    let chunk = Struttura::default()
        .i64(2, inizio)
        .struttura(3, metadati)
        .fine();
    let gruppo = Struttura::default()
        .elenco(1, STRUCT, vec![chunk])
        .i64(2, pagine)
        .i64(3, valori)
        .fine();
    let radice = Struttura::default().testo(4, "schema").i32(5, 1).fine();
    let mut foglia = Struttura::default().i32(1, colonna.tipo);
    if let Some(larghezza) = colonna.larghezza {
        foglia = foglia.i32(2, larghezza);
    }
    foglia = foglia.i32(3, colonna.ripetizione).testo(4, "x");
    if let Some((scala, precisione)) = colonna.decimale {
        foglia = foglia.i32(6, 5).i32(7, scala).i32(8, precisione); // DECIMAL
    } else if let Some(convertito) = colonna.convertito {
        foglia = foglia.i32(6, convertito);
    }
    let footer = Struttura::default()
        .i32(1, 1)
        .elenco(2, STRUCT, vec![radice, foglia.fine()])
        .i64(3, valori)
        .elenco(4, STRUCT, vec![gruppo])
        .fine();
    let lunghezza_footer = u32::try_from(footer.len()).unwrap();
    file.extend(footer);
    file.extend_from_slice(&lunghezza_footer.to_le_bytes());
    file.extend_from_slice(b"PAR1");
    file
}

/// `DELTA_BINARY_PACKED` di valori a differenza costante: un blocco da 128
/// valori in 4 miniblocchi, larghezze 0 (ogni delta vale il minimo).
fn delta_costante(valori: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(128, &mut out);
    varint(4, &mut out);
    varint(valori.len() as u64, &mut out);
    varint(zigzag(valori.first().copied().unwrap_or(0)), &mut out);
    if valori.len() > 1 {
        let delta = valori[1] - valori[0];
        assert!(valori.windows(2).all(|w| w[1] - w[0] == delta));
        varint(zigzag(delta), &mut out);
        out.extend_from_slice(&[0, 0, 0, 0]);
    }
    out
}

// --- Le letture -------------------------------------------------------------

fn su_file<T>(byte: &[u8], f: impl FnOnce(std::fs::File) -> T) -> T {
    let dir = cartella();
    let percorso = dir.path().join("f.parquet");
    std::fs::write(&percorso, byte).unwrap();
    f(std::fs::File::open(&percorso).unwrap())
}

/// Il testo del panico, se c'è.
fn panico(carico: &(dyn std::any::Any + Send)) -> String {
    carico
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| carico.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "(payload non testuale)".to_owned())
}

/// Il lettore Arrow: righe lette o l'errore; un panico fa fallire la prova.
fn arrow(byte: &[u8]) -> Result<usize, String> {
    su_file(byte, |file| {
        std::panic::catch_unwind(move || {
            let lettore = ParquetRecordBatchReaderBuilder::try_new(file)
                .map_err(|e| e.to_string())?
                .build()
                .map_err(|e| e.to_string())?;
            let mut righe = 0;
            for batch in lettore {
                righe += batch.map_err(|e| e.to_string())?.num_rows();
            }
            Ok(righe)
        })
        .unwrap_or_else(|carico| panic!("il lettore Arrow va in panico: {}", panico(&*carico)))
    })
}

/// Il confine di questo crate: l'errore deve essere del decoder, non della
/// barriera (che converte un panico in `DataMapping`, «in panico»).
fn rifiutato_dal_confine(byte: &[u8]) {
    let dir = cartella();
    let percorso = dir.path().join("c.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let errore = leggi_tabella_con_limiti(
        &percorso,
        Some(Formato::Parquet),
        u64::MAX,
        &LimitiLettura::default(),
    )
    .expect_err("il file doveva essere rifiutato");
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
    assert!(!errore.to_string().contains("panico"), "{errore}");
}

/// L'API per colonne (decoder di `encodings::decoding`): legge tutti i
/// valori della colonna, o ne salta `salta`. Un salto di una pagina intera
/// non passa dal decoder (il lettore salta la pagina, controllandone solo la
/// codifica dall'header). Un panico fa fallire la prova.
fn colonna_api<T: parquet::data_type::DataType>(
    byte: &[u8],
    salta: Option<usize>,
) -> Result<usize, String>
where
    T::T: Default + Clone,
{
    su_file(byte, |file| {
        std::panic::catch_unwind(move || {
            let lettore = SerializedFileReader::new(file).map_err(|e| e.to_string())?;
            let gruppo = lettore.get_row_group(0).map_err(|e| e.to_string())?;
            let colonna = gruppo.get_column_reader(0).map_err(|e| e.to_string())?;
            let mut tipizzata = get_typed_column_reader::<T>(colonna);
            if let Some(quanti) = salta {
                return tipizzata.skip_records(quanti).map_err(|e| e.to_string());
            }
            let mut valori = Vec::new();
            tipizzata
                .read_records(64, None, None, &mut valori)
                .map(|(record, _, _)| record)
                .map_err(|e| e.to_string())
        })
        .unwrap_or_else(|carico| panic!("l'API per colonne va in panico: {}", panico(&*carico)))
    })
}

/// Il rifiuto di una codifica non qualificata: dal lettore Arrow del fork,
/// con il testo fisso, e dal confine, `Unsupported`.
fn non_qualificata(byte: &[u8]) {
    contiene(arrow(byte), parquet::basic::ENCODING_NOT_QUALIFIED);
    let dir = cartella();
    let percorso = dir.path().join("q.parquet");
    std::fs::write(&percorso, byte).unwrap();
    let errore = leggi_tabella_con_limiti(
        &percorso,
        Some(Formato::Parquet),
        u64::MAX,
        &LimitiLettura::default(),
    )
    .expect_err("codifica non qualificata");
    assert_eq!(errore.category(), ErrorCategory::Unsupported, "{errore}");
}

fn contiene(esito: Result<usize, String>, atteso: &str) {
    match esito {
        Ok(righe) => panic!("letto senza errore ({righe} righe), atteso «{atteso}»"),
        Err(testo) => assert!(testo.contains(atteso), "atteso «{atteso}»: {testo}"),
    }
}

// --- Le prove ---------------------------------------------------------------

/// Controfattuale: il costruttore di file produce Parquet che si leggono.
#[test]
fn i_file_costruiti_a_mano_si_leggono() {
    let mut testo = Vec::new();
    for valore in [b"ab".as_slice(), b"c"] {
        testo.extend_from_slice(&u32::try_from(valore.len()).unwrap().to_le_bytes());
        testo.extend_from_slice(valore);
    }
    let byte = parquet(&colonna(BYTE_ARRAY), 2, PLAIN, &testo, None, None);
    assert_eq!(arrow(&byte), Ok(2));
    assert_eq!(colonna_api::<ByteArrayType>(&byte, None), Ok(2));

    let doppi: Vec<u8> = (0..8_u32)
        .flat_map(|i| f64::from(i).to_le_bytes())
        .collect();
    let byte = parquet(&colonna(DOUBLE), 8, PLAIN, &doppi, None, None);
    assert_eq!(arrow(&byte), Ok(8));
}

/// Punto 3: `RLE_DICTIONARY` su FLBA: un indice oltre il dizionario tagliava una
/// slice fuori dai limiti.
#[test]
fn un_indice_oltre_il_dizionario_flba_e_un_errore() {
    let flba = Colonna {
        tipo: FLBA,
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    // Larghezza dei bit 3, una corsa RLE di un valore: l'indice 5.
    let indici = [3_u8, 2, 5];
    let byte = parquet(&flba, 1, RLE_DICTIONARY, &indici, Some((1, b"zz")), None);
    contiene(arrow(&byte), "dictionary index out of bounds");
    rifiutato_dal_confine(&byte);
}

/// Stessa classe, `BYTE_ARRAY`: un indice a 32 bit `0xFFFFFFFF` è `-1` in
/// `i32`, `usize::MAX` convertito, e `index + 1` traboccava.
#[test]
fn un_indice_negativo_del_dizionario_e_un_errore() {
    let mut dizionario = 1_u32.to_le_bytes().to_vec();
    dizionario.push(b'a');
    let indici = [32_u8, 2, 0xFF, 0xFF, 0xFF, 0xFF];
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &indici,
        Some((1, &dizionario)),
        None,
    );
    contiene(arrow(&byte), "dictionary");
    rifiutato_dal_confine(&byte);
}

/// `PLAIN` a larghezza fissa: un resto della divisione per la larghezza era
/// scartato in silenzio (5 byte di valori da 2: l'ultimo ignorato).
#[test]
fn un_resto_nella_pagina_e_un_errore() {
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let byte = parquet(&flba, 2, PLAIN, b"abcde", None, None);
    contiene(arrow(&byte), "not a whole number");
    rifiutato_dal_confine(&byte);
}

/// Punto 7: Statistiche di un decimale FLBA vuote o oltre 16 byte:
/// `sign_extend_be::<16>` andava in panico. Ora la statistica manca.
#[test]
fn una_statistica_decimale_malformata_manca_e_non_va_in_panico() {
    let decimale = Colonna {
        tipo: FLBA,
        larghezza: Some(4),
        decimale: Some((0, 9)),
        ..colonna(FLBA)
    };
    let valori = [0_u8, 0, 0, 1, 0, 0, 0, 2];
    for (minimo, massimo) in [
        (&[][..], &[0_u8, 0, 0, 2][..]),
        (&[1_u8; 17][..], &[0, 0, 0, 2][..]),
    ] {
        let byte = parquet(&decimale, 2, PLAIN, &valori, None, Some((minimo, massimo)));
        su_file(&byte, |file| {
            let costruttore = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
            let convertitore = StatisticsConverter::try_new(
                "x",
                costruttore.schema(),
                costruttore.parquet_schema(),
            )
            .unwrap();
            let gruppi = costruttore.metadata().row_groups();
            let minimi = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                convertitore.row_group_mins(gruppi).unwrap()
            }))
            .unwrap_or_else(|carico| panic!("statistica in panico: {}", panico(&*carico)));
            assert_eq!(minimi.null_count(), 1, "{minimo:?}");
            let massimi = convertitore.row_group_maxes(gruppi).unwrap();
            assert_eq!(massimi.null_count(), 0);
        });
    }
}

/// Stessa classe nei dati: un decimale `BYTE_ARRAY` di 17 byte (o vuoto)
/// faceva andare in panico `sign_extend_be::<16>`.
#[test]
fn un_decimale_byte_array_troppo_largo_e_un_errore() {
    let decimale = Colonna {
        tipo: BYTE_ARRAY,
        larghezza: None,
        decimale: Some((0, 10)),
        ..colonna(BYTE_ARRAY)
    };
    for valore in [vec![1_u8; 17], Vec::new()] {
        let mut contenuto = u32::try_from(valore.len()).unwrap().to_le_bytes().to_vec();
        contenuto.extend_from_slice(&valore);
        let byte = parquet(&decimale, 1, PLAIN, &contenuto, None, None);
        contiene(arrow(&byte), "decimal value");
        rifiutato_dal_confine(&byte);
    }
}

/// Stessa classe: la pagina `PLAIN` di `BYTE_ARRAY` che finisce prima dei
/// valori dichiarati. Il lettore Arrow rispondeva «letti tutti» con un
/// valore solo; l'API per colonne leggeva la lunghezza da meno di 4 byte.
#[test]
fn una_pagina_plain_piu_corta_dei_valori_e_un_errore() {
    let mut contenuto = 1_u32.to_le_bytes().to_vec();
    contenuto.push(b'a');
    let byte = parquet(&colonna(BYTE_ARRAY), 3, PLAIN, &contenuto, None, None);
    assert!(arrow(&byte).is_err());
    rifiutato_dal_confine(&byte);
    let corto = parquet(&colonna(BYTE_ARRAY), 1, PLAIN, &[1, 0], None, None);
    contiene(
        colonna_api::<ByteArrayType>(&corto, None),
        "Not enough bytes",
    );
    // Il salto dentro una pagina non aveva nessun controllo (una lunghezza
    // oltre la pagina): ora non è qualificato.
    let oltre = parquet(
        &colonna(BYTE_ARRAY),
        2,
        PLAIN,
        &[100, 0, 0, 0, b'a'],
        None,
        None,
    );
    contiene(
        colonna_api::<ByteArrayType>(&oltre, Some(1)),
        parquet::basic::SKIP_NOT_QUALIFIED,
    );
}

/// Stessa classe: una corsa RLE oltre `u32` si troncava in silenzio.
#[test]
fn una_corsa_rle_oltre_u32_e_un_errore() {
    let mut dizionario = 1_u32.to_le_bytes().to_vec();
    dizionario.push(b'a');
    let mut indici = vec![1_u8];
    varint(1 << 33, &mut indici); // corsa RLE di 2^32 valori
    indici.push(0);
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &indici,
        Some((1, &dizionario)),
        None,
    );
    contiene(arrow(&byte), "RLE run length out of range");
    rifiutato_dal_confine(&byte);
}

/// La pagina di dizionario è condivisa fra i casi: lo stesso costruttore
/// con valori validi si legge (controfattuale dei casi a dizionario).
#[test]
fn un_dizionario_valido_si_legge() {
    let mut dizionario = 1_u32.to_le_bytes().to_vec();
    dizionario.push(b'a');
    let indici = [1_u8, 2, 0];
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &indici,
        Some((1, &dizionario)),
        None,
    );
    assert_eq!(arrow(&byte), Ok(1));
    let flba = Colonna {
        tipo: FLBA,
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let byte = parquet(&flba, 1, RLE_DICTIONARY, &[1, 2, 0], Some((1, b"zz")), None);
    assert_eq!(arrow(&byte), Ok(1));
    let _ = Arc::new(());
}

/// Stessa classe, pagina v2: la somma delle lunghezze dei livelli in `i32`
/// traboccava (un panico), e con il codec `UNCOMPRESSED` i livelli erano
/// confrontati con la dimensione dichiarata, non con i byte letti.
#[test]
fn i_livelli_di_una_pagina_v2_oltre_i_byte_sono_un_errore() {
    let contenuto = 7_i32.to_le_bytes();
    for (definizione, ripetizione, dichiarata) in [(i64::from(i32::MAX), 1, 4), (0, 3, 100)] {
        let header = Struttura::default()
            .i32(1, 3) // DATA_PAGE_V2
            .i32(2, dichiarata)
            .i32(3, 4)
            .struttura(
                8,
                Struttura::default()
                    .i32(1, 1)
                    .i32(2, 0)
                    .i32(3, 1)
                    .i32(4, PLAIN)
                    .i32(5, definizione)
                    .i32(6, ripetizione),
            )
            .fine();
        let byte = parquet_con_header(&colonna(INT32), 1, PLAIN, &contenuto, &header, None, None);
        assert!(arrow(&byte).is_err(), "{definizione} + {ripetizione}");
        rifiutato_dal_confine(&byte);
    }
}

/// Una colonna `OPTIONAL` `INT32` con i livelli di definizione `BIT_PACKED`
/// (pagina v1: i livelli senza lunghezza, poi i valori).
fn livelli_bit_packed(valori: i64, contenuto: &[u8]) -> Vec<u8> {
    let opzionale = Colonna {
        ripetizione: 1,
        ..colonna(INT32)
    };
    let lunghezza = i64::try_from(contenuto.len()).unwrap();
    let header = Struttura::default()
        .i32(1, 0)
        .i32(2, lunghezza)
        .i32(3, lunghezza)
        .struttura(
            5,
            Struttura::default()
                .i32(1, valori)
                .i32(2, PLAIN)
                .i32(3, 4) // BIT_PACKED
                .i32(4, RLE),
        )
        .fine();
    parquet_con_header(&opzionale, valori, PLAIN, contenuto, &header, None, None)
}

/// I livelli `BIT_PACKED` si impacchettano dal bit più significativo, l'RLE
/// ibrido dal meno significativo; i due decoder del fork li leggevano
/// nell'ordine RLE. Un payload `0x80` su 8 righe (il valore 42 nella prima)
/// dava 42 nell'ultima riga e null nella prima, in silenzio. Non qualificati:
/// `Unsupported`, anche con livelli oltre la pagina (cento valori, due byte),
/// e nell'API per colonne.
#[test]
fn i_livelli_bit_packed_non_sono_qualificati() {
    let mut contenuto = vec![0x80_u8];
    contenuto.extend_from_slice(&42_i32.to_le_bytes());
    let byte = livelli_bit_packed(8, &contenuto);
    non_qualificata(&byte);
    contiene(
        colonna_api::<Int32Type>(&byte, None),
        parquet::basic::ENCODING_NOT_QUALIFIED,
    );
    non_qualificata(&livelli_bit_packed(100, &[0xFF, 0xFF]));
    // Controfattuale: gli stessi livelli in RLE (un gruppo bit-packed da 8,
    // il primo bit acceso) danno 42 nella prima riga.
    let byte = opzionale_con_livelli(8, &[(1 << 1) | 1, 0x01], &42_i32.to_le_bytes());
    let valori = su_file(&byte, |file| {
        let lettore = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let batch = lettore.into_iter().next().unwrap().unwrap();
        let colonna = batch.column(0).clone();
        (0..colonna.len())
            .map(|riga| {
                if colonna.is_null(riga) {
                    "null".to_owned()
                } else {
                    testo_della_cella(colonna.as_ref(), riga)
                }
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        valori,
        ["42", "null", "null", "null", "null", "null", "null", "null"]
    );
}

/// Stessa classe: una pagina `RLE_DICTIONARY` senza pagina di dizionario
/// nel column chunk mandava in panico il decoder generico («Decoder for dict
/// should have been set»), quello dei tipi primitivi.
#[test]
fn una_pagina_a_dizionario_senza_dizionario_e_un_errore() {
    let byte = parquet(&colonna(INT32), 1, RLE_DICTIONARY, &[1, 2, 0], None, None);
    contiene(arrow(&byte), "missing dictionary page");
    rifiutato_dal_confine(&byte);
}

// --- Semi del fuzz ----------------------------------------------------------

/// I file malformati delle prove sopra, uno per caso: sono anche semi del
/// target di fuzz `lettura_parquet` (`scripts/genera_corpus_fuzz.py` li
/// prende da `tests/dati/fuzz-decoder/`). Con `PLENORA_RIGENERA_SEMI=1` la
/// prova li riscrive; senza, pretende che i file versionati siano questi.
#[allow(clippy::too_many_lines)] // Un elenco di casi, uno per voce.
fn semi() -> Vec<(&'static str, Vec<u8>)> {
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let mut dizionario = 1_u32.to_le_bytes().to_vec();
    dizionario.push(b'a');
    let mut prefisso = delta_costante(&[0, 5]);
    prefisso.extend(delta_costante(&[1, 1]));
    prefisso.extend_from_slice(b"ab");
    let mut suffisso = delta_costante(&[0]);
    suffisso.extend(delta_costante(&[-1]));
    suffisso.extend_from_slice(b"ab");
    let mut lunghezze = delta_costante(&[-1, 0]);
    lunghezze.extend_from_slice(b"abc");
    let mut blocco = Vec::new();
    varint(1 << 62, &mut blocco);
    varint(1, &mut blocco);
    varint(2, &mut blocco);
    varint(zigzag(7), &mut blocco);
    varint(zigzag(1), &mut blocco);
    blocco.push(8);
    blocco.extend_from_slice(&[0; 8]);
    let mut resto: Vec<u8> = (0..8_u32)
        .flat_map(|i| f64::from(i).to_le_bytes())
        .collect();
    resto.push(0xAB);
    let mut corsa = vec![1_u8];
    varint(1 << 33, &mut corsa);
    corsa.push(0);
    let decimale = Colonna {
        decimale: Some((0, 10)),
        ..colonna(BYTE_ARRAY)
    };
    let mut largo = 17_u32.to_le_bytes().to_vec();
    largo.extend_from_slice(&[1; 17]);
    let testo = colonna(BYTE_ARRAY);
    let mut semi = vec![
        (
            "prefisso-oltre-il-valore",
            parquet(&testo, 2, DELTA_BYTE_ARRAY, &prefisso, None, None),
        ),
        (
            "suffisso-negativo",
            parquet(&testo, 1, DELTA_BYTE_ARRAY, &suffisso, None, None),
        ),
        (
            "indice-flba-oltre-il-dizionario",
            parquet(&flba, 1, RLE_DICTIONARY, &[3, 2, 5], Some((1, b"zz")), None),
        ),
        (
            "indice-negativo",
            parquet(
                &testo,
                1,
                RLE_DICTIONARY,
                &[32, 2, 0xFF, 0xFF, 0xFF, 0xFF],
                Some((1, &dizionario)),
                None,
            ),
        ),
        (
            "lunghezza-delta-negativa",
            parquet(&testo, 2, DELTA_LENGTH_BYTE_ARRAY, &lunghezze, None, None),
        ),
        (
            "blocco-delta-enorme",
            parquet(&colonna(INT32), 2, DELTA_BINARY_PACKED, &blocco, None, None),
        ),
        (
            "resto-byte-stream-split",
            parquet(&colonna(DOUBLE), 8, BYTE_STREAM_SPLIT, &resto, None, None),
        ),
        (
            "resto-flba-plain",
            parquet(&flba, 2, PLAIN, b"abcde", None, None),
        ),
        (
            "decimale-troppo-largo",
            parquet(&decimale, 1, PLAIN, &largo, None, None),
        ),
        (
            "plain-piu-corta",
            parquet(&testo, 3, PLAIN, &[1, 0, 0, 0, b'a'], None, None),
        ),
        (
            "corsa-rle-oltre-u32",
            parquet(
                &testo,
                1,
                RLE_DICTIONARY,
                &corsa,
                Some((1, &dizionario)),
                None,
            ),
        ),
        (
            "dizionario-mancante",
            parquet(&colonna(INT32), 1, RLE_DICTIONARY, &[1, 2, 0], None, None),
        ),
    ];
    semi.extend(semi_del_secondo_giro());
    semi
}

/// I file malformati del secondo giro (letti con lo schema dedotto).
#[allow(clippy::too_many_lines)] // Un elenco di casi, uno per voce.
fn semi_del_secondo_giro() -> Vec<(&'static str, Vec<u8>)> {
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let mut suffissi = delta_costante(&[0, 1]);
    suffissi.extend(delta_costante(&[1]));
    suffissi.extend_from_slice(b"a");
    let mut troncato = Vec::new();
    varint(128, &mut troncato);
    varint(4, &mut troncato);
    varint(2, &mut troncato);
    varint(zigzag(0), &mut troncato);
    varint(zigzag(0), &mut troncato);
    troncato.extend_from_slice(&[8, 0, 0, 0, 1]);
    let mut lungo = vec![1_u8];
    lungo.extend_from_slice(&[0xFF; 10]);
    lungo.push(0x01);
    let mut oltre = Vec::new();
    varint((100 << 1) | 1, &mut oltre);
    let dati = [1_u8, 0, 0, 0, 2, 0, 0, 0];
    vec![
        (
            "flba-senza-dizionario",
            parquet(&flba, 1, RLE_DICTIONARY, &[1, 2, 0], None, None),
        ),
        (
            "indice-oltre-le-voci",
            parquet(
                &flba,
                1,
                RLE_DICTIONARY,
                &[1, 2, 1],
                Some((1, b"zzyy")),
                None,
            ),
        ),
        (
            "suffissi-mancanti",
            parquet(
                &colonna(BYTE_ARRAY),
                2,
                DELTA_BYTE_ARRAY,
                &suffissi,
                None,
                None,
            ),
        ),
        (
            "blocco-delta-troncato",
            parquet(
                &colonna(BYTE_ARRAY),
                2,
                DELTA_BYTE_ARRAY,
                &troncato,
                None,
                None,
            ),
        ),
        (
            "varint-di-11-byte",
            parquet(
                &colonna(BYTE_ARRAY),
                1,
                RLE_DICTIONARY,
                &lungo,
                Some((1, &testo_plain(&[b"a"]))),
                None,
            ),
        ),
        (
            "livelli-bit-packed-oltre-i-dati",
            opzionale_con_livelli(2, &oltre, &dati),
        ),
        (
            "livello-rle-di-valore-2",
            opzionale_con_livelli(2, &[2 << 1, 2], &dati),
        ),
        (
            "livelli-bit-packed",
            livelli_bit_packed(8, &[0x80, 42, 0, 0, 0]),
        ),
        (
            "dizionario-vuoto-con-byte",
            parquet(
                &colonna(BYTE_ARRAY),
                1,
                RLE_DICTIONARY,
                &[1, 2, 0],
                Some((0, &testo_plain(&[b"a"]))),
                None,
            ),
        ),
        (
            "dizionario-piu-corto",
            parquet(
                &colonna(BYTE_ARRAY),
                1,
                RLE_DICTIONARY,
                &[1, 2, 0],
                Some((2, &testo_plain(&[b"a"]))),
                None,
            ),
        ),
        (
            "v2-livelli-contro-valori",
            v2_livelli_contro_valori(&[5, 0]),
        ),
        (
            "valori-plain-in-piu",
            parquet(
                &colonna(INT32),
                1,
                PLAIN,
                &[7, 0, 0, 0, 8, 0, 0, 0],
                None,
                None,
            ),
        ),
        (
            "livelli-con-una-corsa-in-piu",
            opzionale_con_livelli(2, &[2 << 1, 1, 2 << 1, 1], &[1, 0, 0, 0, 2, 0, 0, 0]),
        ),
        (
            "dichiarata-plain-usata-delta",
            parquet_con_header(
                &colonna(INT32),
                1,
                PLAIN,
                &delta_costante(&[7]),
                &header_v1(1, DELTA_BINARY_PACKED, delta_costante(&[7]).len()),
                None,
                None,
            ),
        ),
    ]
}

/// I semi che usano una codifica non qualificata: si rifiutano come
/// `Unsupported` prima di arrivare al decoder (le correzioni dei decoder di
/// quelle codifiche restano nel fork, non raggiungibili).
const NON_QUALIFICATI: [&str; 9] = [
    "prefisso-oltre-il-valore",
    "suffisso-negativo",
    "lunghezza-delta-negativa",
    "blocco-delta-enorme",
    "resto-byte-stream-split",
    "suffissi-mancanti",
    "blocco-delta-troncato",
    "dichiarata-plain-usata-delta",
    "livelli-bit-packed",
];

#[test]
fn i_semi_del_fuzz_sono_quelli_delle_prove_e_si_rifiutano() {
    let cartella = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("dati")
        .join("fuzz-decoder");
    let rigenera = std::env::var_os("PLENORA_RIGENERA_SEMI").is_some();
    if rigenera {
        std::fs::create_dir_all(&cartella).unwrap();
    }
    let semi = semi();
    for (nome, byte) in &semi {
        if NON_QUALIFICATI.contains(nome) {
            non_qualificata(byte);
        } else {
            rifiutato_dal_confine(byte);
        }
        let percorso = cartella.join(format!("{nome}.parquet"));
        if rigenera {
            std::fs::write(&percorso, byte).unwrap();
        } else {
            let versionato = std::fs::read(&percorso).unwrap_or_else(|_| {
                panic!("{nome}: seme assente, rigenerare con PLENORA_RIGENERA_SEMI=1")
            });
            assert_eq!(&versionato, byte, "{nome}: seme diverso dalla prova");
        }
    }
    let versionati = std::fs::read_dir(&cartella).unwrap().count();
    assert_eq!(versionati, semi.len(), "semi in eccesso nella cartella");
}

// --- Secondo giro (`patches/parquet-decoder-2.patch`) ------------------------

/// Il lettore Arrow con uno schema Arrow dato (`with_schema`): i batch letti
/// o l'errore; un panico fa fallire la prova.
fn arrow_con_schema(byte: &[u8], schema: Schema) -> Result<Vec<RecordBatch>, String> {
    su_file(byte, |file| {
        std::panic::catch_unwind(move || {
            let opzioni = ArrowReaderOptions::new().with_schema(Arc::new(schema));
            let lettore = ParquetRecordBatchReaderBuilder::try_new_with_options(file, opzioni)
                .map_err(|e| e.to_string())?
                .build()
                .map_err(|e| e.to_string())?;
            lettore
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        })
        .unwrap_or_else(|carico| panic!("il lettore Arrow va in panico: {}", panico(&*carico)))
    })
}

/// I valori letti dal lettore Arrow, come testo di debug della colonna.
fn valori_arrow(byte: &[u8]) -> Vec<String> {
    su_file(byte, |file| {
        let lettore = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let mut valori = Vec::new();
        for batch in lettore {
            let colonna = batch.unwrap().column(0).clone();
            for riga in 0..colonna.len() {
                valori.push(testo_della_cella(colonna.as_ref(), riga));
            }
        }
        valori
    })
}

/// Una cella come testo: i tipi delle prove (`Utf8`/`Binary` come testo,
/// `Int32` in decimale, `FixedSizeBinary` in esadecimale).
fn testo_della_cella(colonna: &dyn Array, riga: usize) -> String {
    match colonna.data_type() {
        DataType::Utf8 => colonna.as_string::<i32>().value(riga).to_owned(),
        DataType::Binary => {
            String::from_utf8(colonna.as_binary::<i32>().value(riga).to_vec()).unwrap()
        }
        DataType::Int32 => colonna
            .as_primitive::<plenora_core::arrow::array::types::Int32Type>()
            .value(riga)
            .to_string(),
        DataType::Int8 => colonna
            .as_primitive::<plenora_core::arrow::array::types::Int8Type>()
            .value(riga)
            .to_string(),
        DataType::UInt8 => colonna
            .as_primitive::<plenora_core::arrow::array::types::UInt8Type>()
            .value(riga)
            .to_string(),
        DataType::Int16 => colonna
            .as_primitive::<plenora_core::arrow::array::types::Int16Type>()
            .value(riga)
            .to_string(),
        DataType::UInt16 => colonna
            .as_primitive::<plenora_core::arrow::array::types::UInt16Type>()
            .value(riga)
            .to_string(),
        DataType::Dictionary(_, _) => {
            let dizionario = colonna.as_any_dictionary();
            testo_della_cella(
                dizionario.values().as_ref(),
                dizionario.normalized_keys()[riga],
            )
        }
        DataType::FixedSizeBinary(_) => colonna.as_fixed_size_binary().value(riga).iter().fold(
            String::new(),
            |mut testo, byte| {
                write!(testo, "{byte:02x}").unwrap();
                testo
            },
        ),
        altro => panic!("tipo non previsto dalle prove: {altro}"),
    }
}

fn testo_plain(valori: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for valore in valori {
        out.extend_from_slice(&u32::try_from(valore.len()).unwrap().to_le_bytes());
        out.extend_from_slice(valore);
    }
    out
}

/// I controfattuali verificano i valori, non solo il numero di righe.
#[test]
fn i_file_validi_danno_i_valori_scritti() {
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        2,
        PLAIN,
        &testo_plain(&[b"ab", b"c"]),
        None,
        None,
    );
    assert_eq!(valori_arrow(&byte), ["ab", "c"]);

    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let byte = parquet(&flba, 1, RLE_DICTIONARY, &[1, 2, 0], Some((1, b"zz")), None);
    assert_eq!(valori_arrow(&byte), ["7a7a"]);

    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &[1, 2, 0],
        Some((1, &testo_plain(&[b"a"]))),
        None,
    );
    assert_eq!(valori_arrow(&byte), ["a"]);
}

/// Secondo giro, punto 1: FLBA `RLE_DICTIONARY` senza pagina di dizionario: un
/// `unwrap` (panico).
#[test]
fn una_pagina_flba_a_dizionario_senza_dizionario_e_un_errore() {
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let byte = parquet(&flba, 1, RLE_DICTIONARY, &[1, 2, 0], None, None);
    contiene(arrow(&byte), "missing dictionary page");
    rifiutato_dal_confine(&byte);
}

/// Secondo giro, punto 2: il dizionario FLBA dichiara una voce ma la pagina ne
/// porta i byte di due. L'indice 1 era confrontato con i byte, non con le
/// voci dichiarate, e leggeva «yy» in silenzio. Ora la pagina di dizionario
/// stessa, con byte oltre le voci dichiarate, è un errore.
#[test]
fn un_indice_oltre_le_voci_dichiarate_e_un_errore() {
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let byte = parquet(
        &flba,
        1,
        RLE_DICTIONARY,
        &[1, 2, 1],
        Some((1, b"zzyy")),
        None,
    );
    contiene(arrow(&byte), parquet::basic::DICTIONARY_NOT_AS_DECLARED);
    rifiutato_dal_confine(&byte);
}

/// Secondo giro, punto 5: un varint di 11 byte (panico) e uno di 10 con bit oltre
/// i 64 (troncati in silenzio), nell'indicatore RLE degli indici.
#[test]
fn un_varint_malformato_e_un_errore() {
    let dizionario = testo_plain(&[b"a"]);
    let mut lungo = vec![1_u8];
    lungo.extend_from_slice(&[0xFF; 10]);
    lungo.push(0x01);
    let mut largo = vec![1_u8];
    largo.extend_from_slice(&[0x80; 9]);
    largo.push(0x7F);
    for (nome, indici) in [("11 byte", lungo), ("oltre 64 bit", largo)] {
        let byte = parquet(
            &colonna(BYTE_ARRAY),
            1,
            RLE_DICTIONARY,
            &indici,
            Some((1, &dizionario)),
            None,
        );
        assert!(arrow(&byte).is_err(), "{nome}");
        rifiutato_dal_confine(&byte);
    }
}

/// Un Parquet di una colonna `OPTIONAL` `INT32` con livelli di definizione
/// RLE dati (pagina v1: lunghezza in 4 byte, poi i livelli) e i valori.
fn opzionale_con_livelli(valori: i64, livelli: &[u8], dati: &[u8]) -> Vec<u8> {
    let mut contenuto = u32::try_from(livelli.len()).unwrap().to_le_bytes().to_vec();
    contenuto.extend_from_slice(livelli);
    contenuto.extend_from_slice(dati);
    let opzionale = Colonna {
        ripetizione: 1,
        ..colonna(INT32)
    };
    parquet(&opzionale, valori, PLAIN, &contenuto, None, None)
}

/// Secondo giro, punto 6: il decoder ottimizzato dei livelli Arrow (larghezza 1)
/// non aveva le correzioni del decoder RLE: una corsa bit-packed oltre il
/// payload leggeva bit fuori dai dati, una corsa di 2^32 era accettata, e un
/// valore RLE diverso da 0 e 1 era letto come 1. Una corsa bit-packed finale
/// più corta dei suoi gruppi è lecita (come nel decoder generico e in C++):
/// si legge fino ai suoi byte, e se i livelli non bastano è un errore.
#[test]
fn i_livelli_rle_malformati_sono_un_errore() {
    let mut oltre = Vec::new();
    varint((100 << 1) | 1, &mut oltre); // 100 gruppi bit-packed, nessun byte
    let mut enorme = Vec::new();
    varint(1 << 33, &mut enorme); // corsa RLE di 2^32
    enorme.push(1);
    let valore_due = [2_u8 << 1, 2]; // corsa di 2, valore 2
    for (nome, livelli) in [
        ("bit-packed oltre i dati", oltre),
        ("corsa di 2^32", enorme),
        ("valore 2", valore_due.to_vec()),
    ] {
        let byte = opzionale_con_livelli(2, &livelli, &[1, 0, 0, 0, 2, 0, 0, 0]);
        assert!(arrow(&byte).is_err(), "{nome}");
        rifiutato_dal_confine(&byte);
    }
    // Controfattuali: una corsa RLE di 2 valori presenti, e una corsa
    // bit-packed finale dichiarata di 3 gruppi con il solo byte che serve.
    let byte = opzionale_con_livelli(2, &[2 << 1, 1], &[1, 0, 0, 0, 2, 0, 0, 0]);
    assert_eq!(valori_arrow(&byte), ["1", "2"]);
    let byte = opzionale_con_livelli(2, &[(3 << 1) | 1, 0b11], &[1, 0, 0, 0, 2, 0, 0, 0]);
    assert_eq!(valori_arrow(&byte), ["1", "2"]);
}

/// Secondo giro, punto 7: con una chiave Arrow stretta (`Int8`) l'indice 256 si
/// troncava a 0 con `as` prima di ogni controllo, e leggeva «a» in silenzio.
#[test]
fn un_indice_oltre_la_chiave_stretta_e_un_errore() {
    let mut indici = vec![9_u8]; // larghezza 9 bit
    indici.push(2); // corsa RLE di 1
    indici.extend_from_slice(&256_u16.to_le_bytes());
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &indici,
        Some((1, &testo_plain(&[b"a"]))),
        None,
    );
    let schema = Schema::new(vec![Field::new(
        "x",
        DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Binary)),
        false,
    )]);
    contiene(
        arrow_con_schema(&byte, schema.clone()).map(|batch| batch.len()),
        "out of range for the key type",
    );
    // Controfattuale: l'indice 0 con la stessa chiave si legge.
    let mut zero = vec![9_u8, 2];
    zero.extend_from_slice(&0_u16.to_le_bytes());
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &zero,
        Some((1, &testo_plain(&[b"a"]))),
        None,
    );
    let batch = arrow_con_schema(&byte, schema).unwrap();
    assert_eq!(testo_della_cella(batch[0].column(0).as_ref(), 0), "a");
}

/// Una colonna FLBA letta come dizionario Arrow
/// (`Dictionary(Int32, FixedSizeBinary(2))`) passava dal lettore dei byte
/// array variabili, che legge il dizionario con il prefisso di lunghezza:
/// un dizionario FLBA valido (`zz`) si rifiutava, e uno costruito con i
/// prefissi (`a`, `bcd`) diventava `["ab", "cd"]` in silenzio. Ora la
/// combinazione è `Unsupported`, esplicita; lo stesso dizionario valido si
/// legge come `FixedSizeBinary`.
#[test]
fn una_colonna_flba_come_dizionario_arrow_e_non_supportata() {
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let schema = Schema::new(vec![Field::new(
        "x",
        DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::FixedSizeBinary(2)),
        ),
        false,
    )]);
    for dizionario in [b"zz".to_vec(), testo_plain(&[b"a", b"bcd"])] {
        let voci = if dizionario.len() == 2 { 1 } else { 2 };
        let byte = parquet(
            &flba,
            1,
            RLE_DICTIONARY,
            &[1, 2, 0],
            Some((voci, &dizionario)),
            None,
        );
        contiene(
            arrow_con_schema(&byte, schema.clone()).map(|batch| batch.len()),
            "FIXED_LEN_BYTE_ARRAY read as an Arrow dictionary",
        );
    }
    let valido = parquet(&flba, 1, RLE_DICTIONARY, &[1, 2, 0], Some((1, b"zz")), None);
    assert_eq!(valori_arrow(&valido), ["7a7a"]);
}

/// Livelli v2 oltre i byte letti davvero (non solo oltre la dimensione
/// dichiarata): dichiarati 20 byte non compressi (entro il column chunk), 4
/// letti, 8 di livelli.
#[test]
fn i_livelli_v2_oltre_i_byte_letti_sono_un_errore() {
    let contenuto = 7_i32.to_le_bytes();
    let header = Struttura::default()
        .i32(1, 3)
        .i32(2, 20)
        .i32(3, 4)
        .struttura(
            8,
            Struttura::default()
                .i32(1, 1)
                .i32(2, 0)
                .i32(3, 1)
                .i32(4, PLAIN)
                .i32(5, 8)
                .i32(6, 0),
        )
        .fine();
    let byte = parquet_con_header(&colonna(INT32), 1, PLAIN, &contenuto, &header, None, None);
    contiene(arrow(&byte), "implausible");
    rifiutato_dal_confine(&byte);
}

// --- Codifiche lette --------------------------------------------------------

/// Una pagina valida per ciascuna codifica non qualificata: tipo della
/// colonna, valori, contenuto.
fn pagine_non_qualificate() -> Vec<(&'static str, i64, Colonna, i64, Vec<u8>)> {
    let mut lunghezze = delta_costante(&[2]);
    lunghezze.extend_from_slice(b"ab");
    let mut delta = delta_costante(&[0]);
    delta.extend(delta_costante(&[2]));
    delta.extend_from_slice(b"ab");
    vec![
        (
            "DELTA_BINARY_PACKED",
            DELTA_BINARY_PACKED,
            colonna(INT32),
            1,
            delta_costante(&[7]),
        ),
        (
            "DELTA_LENGTH_BYTE_ARRAY",
            DELTA_LENGTH_BYTE_ARRAY,
            colonna(BYTE_ARRAY),
            1,
            lunghezze,
        ),
        (
            "DELTA_BYTE_ARRAY",
            DELTA_BYTE_ARRAY,
            colonna(BYTE_ARRAY),
            1,
            delta,
        ),
        (
            "BYTE_STREAM_SPLIT",
            BYTE_STREAM_SPLIT,
            colonna(DOUBLE),
            1,
            2.5_f64.to_le_bytes().to_vec(),
        ),
    ]
}

/// L'header di una pagina di dati v1 con la codifica data.
fn header_v1(valori: i64, codifica: i64, lunghezza: usize) -> Vec<u8> {
    let lunghezza = i64::try_from(lunghezza).unwrap();
    Struttura::default()
        .i32(1, 0)
        .i32(2, lunghezza)
        .i32(3, lunghezza)
        .struttura(
            5,
            Struttura::default()
                .i32(1, valori)
                .i32(2, codifica)
                .i32(3, RLE)
                .i32(4, RLE),
        )
        .fine()
}

/// Ogni codifica non qualificata si rifiuta (`Unsupported`, testo fisso),
/// in tre forme: dichiarata nel footer e usata nella pagina; dichiarata
/// `PLAIN` e usata nella pagina (la controlla il fork, pagina per pagina);
/// dichiarata nel footer e la pagina in `PLAIN` (la controlla il confine sul
/// column chunk). Senza la riduzione le pagine, valide, si leggevano.
#[test]
fn le_codifiche_non_qualificate_si_rifiutano_in_ogni_forma() {
    for (nome, codifica, colonna, valori, contenuto) in pagine_non_qualificate() {
        let usata = parquet(&colonna, valori, codifica, &contenuto, None, None);
        non_qualificata(&usata);

        let header = header_v1(valori, codifica, contenuto.len());
        let nascosta = parquet_con_header(&colonna, valori, PLAIN, &contenuto, &header, None, None);
        non_qualificata(&nascosta);

        // Dichiarata nel footer, pagina PLAIN: il lettore del fork la legge
        // (non controlla il footer), il confine la rifiuta.
        let plain: Vec<u8> = match colonna.tipo {
            INT32 => 7_i32.to_le_bytes().to_vec(),
            DOUBLE => 2.5_f64.to_le_bytes().to_vec(),
            _ => testo_plain(&[b"ab"]),
        };
        let header = header_v1(valori, PLAIN, plain.len());
        let dichiarata =
            parquet_con_header(&colonna, valori, codifica, &plain, &header, None, None);
        assert_eq!(arrow(&dichiarata), Ok(1), "{nome}");
        let dir = cartella();
        let percorso = dir.path().join("d.parquet");
        std::fs::write(&percorso, &dichiarata).unwrap();
        let errore = leggi_tabella_con_limiti(
            &percorso,
            Some(Formato::Parquet),
            u64::MAX,
            &LimitiLettura::default(),
        )
        .expect_err(nome);
        assert_eq!(
            errore.category(),
            ErrorCategory::Unsupported,
            "{nome}: {errore}"
        );
    }
}

/// L'API per colonne del fork rifiuta le stesse codifiche, anche quando
/// salta la pagina intera (prima la saltava senza guardarla).
#[test]
fn l_api_per_colonne_rifiuta_le_codifiche_non_qualificate() {
    for (nome, codifica, colonna, valori, contenuto) in pagine_non_qualificate() {
        let byte = parquet(&colonna, valori, codifica, &contenuto, None, None);
        let tutte = Some(usize::try_from(valori).unwrap());
        for salta in [None, tutte] {
            let esito = match colonna.tipo {
                INT32 => colonna_api::<Int32Type>(&byte, salta),
                DOUBLE => colonna_api::<DoubleType>(&byte, salta),
                _ => colonna_api::<ByteArrayType>(&byte, salta),
            };
            match esito {
                Err(testo) => assert!(
                    testo.contains(parquet::basic::ENCODING_NOT_QUALIFIED),
                    "{nome} ({salta:?}): {testo}"
                ),
                Ok(righe) => panic!("{nome} ({salta:?}): letto ({righe} righe)"),
            }
        }
    }
    // Controfattuale: la stessa pagina intera in PLAIN si salta.
    let byte = parquet(&colonna(INT32), 1, PLAIN, &7_i32.to_le_bytes(), None, None);
    assert_eq!(colonna_api::<Int32Type>(&byte, Some(1)), Ok(1));
}

// --- Livelli, interi stretti, UTF-8, varint ----------------------------------

/// Una colonna `REPEATED` `INT32` (lista): livelli di ripetizione e di
/// definizione RLE, ciascuno con la sua lunghezza in 4 byte, poi i valori.
/// Massimi: ripetizione 1, definizione 1; i livelli passano dal decoder
/// generico, non da quello ottimizzato.
fn ripetuta(valori: i64, ripetizione: &[u8], definizione: &[u8], dati: &[u8]) -> Vec<u8> {
    let mut contenuto = u32::try_from(ripetizione.len())
        .unwrap()
        .to_le_bytes()
        .to_vec();
    contenuto.extend_from_slice(ripetizione);
    contenuto.extend_from_slice(&u32::try_from(definizione.len()).unwrap().to_le_bytes());
    contenuto.extend_from_slice(definizione);
    contenuto.extend_from_slice(dati);
    let lista = Colonna {
        ripetizione: 2,
        ..colonna(INT32)
    };
    parquet(&lista, valori, PLAIN, &contenuto, None, None)
}

/// Punto 1: il decoder RLE condiviso dei livelli non verificava il valore
/// (un livello 2 con massimo 1, contato come assente) né il payload
/// bit-packed (100 gruppi con un byte). Le stesse regole del decoder
/// ottimizzato, nel decoder generico dei livelli e negli indici.
#[test]
fn i_livelli_del_decoder_generico_si_controllano() {
    let dati = [1_u8, 0, 0, 0, 2, 0, 0, 0];
    // Controfattuale: una lista [1, 2].
    let valida = ripetuta(2, &[1 << 1, 0, 1 << 1, 1], &[2 << 1, 1], &dati);
    assert_eq!(arrow(&valida), Ok(1));
    let mut bit_packed = Vec::new();
    varint((100 << 1) | 1, &mut bit_packed);
    for (nome, ripetizione, definizione) in [
        (
            "definizione 2 su massimo 1",
            vec![1 << 1, 0, 1 << 1, 1],
            vec![2 << 1, 2],
        ),
        (
            "ripetizione 2 su massimo 1",
            vec![1 << 1, 0, 1 << 1, 2],
            vec![2 << 1, 1],
        ),
        (
            "definizione bit-packed oltre i dati",
            vec![1 << 1, 0, 1 << 1, 1],
            bit_packed.clone(),
        ),
    ] {
        let byte = ripetuta(2, &ripetizione, &definizione, &dati);
        assert!(arrow(&byte).is_err(), "{nome}");
        rifiutato_dal_confine(&byte);
    }
    // Indici di dizionario: un valore RLE oltre la larghezza dichiarata (1 bit).
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &[1, 2, 2],
        Some((1, &testo_plain(&[b"a"]))),
        None,
    );
    contiene(arrow(&byte), "RLE value wider than the bit width");
    rifiutato_dal_confine(&byte);
}

/// Punto 3: un `INT32` annotato `INT_8` (o `UINT_8`, `INT_16`, `UINT_16`)
/// fuori dalla larghezza diventava un altro numero con `as` (256 → 0).
#[test]
fn un_intero_stretto_fuori_dalla_larghezza_e_un_errore() {
    for (convertito, fuori, dentro, atteso) in [
        (15, 256_i32, -7_i32, "-7"),   // INT_8
        (11, 256, 200, "200"),         // UINT_8
        (16, 40_000, -300, "-300"),    // INT_16
        (12, 70_000, 60_000, "60000"), // UINT_16
    ] {
        let stretta = Colonna {
            convertito: Some(convertito),
            ..colonna(INT32)
        };
        let byte = parquet(&stretta, 1, PLAIN, &fuori.to_le_bytes(), None, None);
        contiene(arrow(&byte), "outside its annotated integer width");
        rifiutato_dal_confine(&byte);
        let byte = parquet(&stretta, 1, PLAIN, &dentro.to_le_bytes(), None, None);
        su_file(&byte, |file| {
            let mut lettore = ParquetRecordBatchReaderBuilder::try_new(file)
                .unwrap()
                .build()
                .unwrap();
            let batch = lettore.next().unwrap().unwrap();
            let colonna = testo_della_cella(batch.column(0).as_ref(), 0);
            assert_eq!(colonna, atteso, "tipo convertito {convertito}");
        });
    }
}

/// Punto 4: la validazione UTF-8 di `PLAIN` (anche per `Utf8View` e per il
/// dizionario) non accetta un carattere spezzato fra due valori.
#[test]
fn un_carattere_utf8_spezzato_fra_due_valori_si_rifiuta() {
    // Colonne annotate `UTF8` (tipo convertito 0).
    let testo_annotato = Colonna {
        convertito: Some(0),
        ..colonna(BYTE_ARRAY)
    };
    let spezzato = testo_plain(&[&[0xC3], &[0xA9]]);
    let intero = testo_plain(&["é".as_bytes()]);
    for tipo in [DataType::Utf8, DataType::Utf8View] {
        let schema = Schema::new(vec![Field::new("x", tipo.clone(), false)]);
        let byte = parquet(&testo_annotato, 2, PLAIN, &spezzato, None, None);
        assert!(arrow_con_schema(&byte, schema.clone()).is_err(), "{tipo}");
        let byte = parquet(&testo_annotato, 1, PLAIN, &intero, None, None);
        assert_eq!(
            arrow_con_schema(&byte, schema).map(|batch| batch[0].num_rows()),
            Ok(1),
            "{tipo}"
        );
    }
    let schema = Schema::new(vec![Field::new(
        "x",
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        false,
    )]);
    // Indici [0, 1]: un gruppo bit-packed da un bit.
    let byte = parquet(
        &testo_annotato,
        2,
        RLE_DICTIONARY,
        &[1, 3, 0x02],
        Some((2, &spezzato)),
        None,
    );
    contiene(
        arrow_con_schema(&byte, schema.clone()).map(|batch| batch.len()),
        "non UTF-8",
    );
    let byte = parquet(
        &testo_annotato,
        1,
        RLE_DICTIONARY,
        &[1, 2, 0],
        Some((1, &intero)),
        None,
    );
    let batch = arrow_con_schema(&byte, schema).unwrap();
    assert_eq!(testo_della_cella(batch[0].column(0).as_ref(), 0), "é");
}

/// Non bloccante: varint, ogni caso da solo. Dieci byte di continuazione
/// senza terminatore erano la fine dei dati (`Ok(None)`), undici un
/// `assert!`, dieci con bit oltre i 64 un troncamento.
#[test]
fn ogni_varint_malformato_ha_il_suo_errore() {
    let dizionario = testo_plain(&[b"a"]);
    let mut senza_fine = vec![1_u8];
    senza_fine.extend_from_slice(&[0x80; 10]);
    let mut undici = vec![1_u8];
    undici.extend_from_slice(&[0x80; 10]);
    undici.push(0x01);
    let mut oltre = vec![1_u8];
    oltre.extend_from_slice(&[0x80; 9]);
    oltre.push(0x7F);
    let mut troncato = vec![1_u8];
    troncato.extend_from_slice(&[0x80; 3]);
    for (indici, atteso) in [
        (senza_fine, "varint truncated"),
        (undici, "varint longer than"),
        (oltre, "varint beyond 64 bits"),
        (troncato, "varint truncated"),
    ] {
        let byte = parquet(
            &colonna(BYTE_ARRAY),
            1,
            RLE_DICTIONARY,
            &indici,
            Some((1, &dizionario)),
            None,
        );
        contiene(arrow(&byte), atteso);
        rifiutato_dal_confine(&byte);
    }
}

/// Un tipo Arrow testo (dato o incorporato) su una colonna di byte senza
/// annotazione di testo: i decoder validano l'UTF-8 dall'annotazione, e i
/// byte spezzati diventavano una stringa non valida (un panico nelle build di
/// debug, UTF-8 non valido in release). Ora `Unsupported`, esplicito; la
/// stessa colonna si legge come `Binary`.
#[test]
fn un_tipo_testo_su_byte_non_annotati_e_non_supportato() {
    let spezzato = testo_plain(&[&[0xC3], &[0xA9]]);
    let byte = parquet(&colonna(BYTE_ARRAY), 2, PLAIN, &spezzato, None, None);
    for tipo in [
        DataType::Utf8,
        DataType::Utf8View,
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
    ] {
        let schema = Schema::new(vec![Field::new("x", tipo.clone(), false)]);
        contiene(
            arrow_con_schema(&byte, schema).map(|batch| batch.len()),
            parquet::basic::STRING_WITHOUT_ANNOTATION,
        );
    }
    assert_eq!(arrow(&byte), Ok(2));
}

/// Il rovescio della lettura: un dizionario di `FixedSizeBinary` non si
/// scrive (`ArrowWriter` lo scriverebbe in una forma che la lettura rifiuta),
/// né da solo né dentro una lista; il file non nasce.
#[test]
fn un_dizionario_di_binari_fissi_non_si_scrive() {
    use plenora_core::arrow::array::builder::{FixedSizeBinaryDictionaryBuilder, ListBuilder};
    use plenora_core::arrow::array::types::Int32Type as Chiave;
    use plenora_core::arrow::array::{ArrayRef, DictionaryArray, FixedSizeBinaryArray, Int32Array};
    let valori =
        FixedSizeBinaryArray::try_from_iter(vec![vec![1_u8, 2], vec![3, 4]].into_iter()).unwrap();
    let dizionario: ArrayRef = Arc::new(
        DictionaryArray::<Chiave>::try_new(Int32Array::from(vec![0, 1, 0]), Arc::new(valori))
            .unwrap(),
    );
    let mut costruttore = ListBuilder::new(FixedSizeBinaryDictionaryBuilder::<Chiave>::new(2));
    costruttore.values().append([1_u8, 2]).unwrap();
    costruttore.values().append([3_u8, 4]).unwrap();
    costruttore.append(true);
    let lista: ArrayRef = Arc::new(costruttore.finish());
    for (nome, colonna) in [("da solo", dizionario), ("in una lista", lista)] {
        let schema = Schema::new(vec![Field::new("x", colonna.data_type().clone(), false)]);
        let tabella = RecordBatch::try_new(Arc::new(schema), vec![colonna]).unwrap();
        let dir = cartella();
        let percorso = dir.path().join("fsb.parquet");
        let errore = plenora_io::scrivi_tabella(
            &tabella,
            &percorso,
            &plenora_io::OpzioniScrittura::default(),
        )
        .expect_err(nome);
        assert_eq!(
            errore.category(),
            ErrorCategory::Unsupported,
            "{nome}: {errore}"
        );
        assert!(!percorso.exists(), "{nome}");
    }
}

// --- Dizionari, pagine v2, valori di testo ------------------------------------

/// Una pagina di dizionario che non tiene le voci dichiarate. `BYTE_ARRAY`:
/// zero voci dichiarate con dei byte dividevano per zero (panico); meno voci
/// delle dichiarate, o byte dopo l'ultima, si accettavano (il conteggio letto
/// era ignorato). Lo stesso nei tre lettori Arrow (byte, dizionario, viste),
/// nel decoder generico (`INT32`) e per FLBA.
#[test]
fn un_dizionario_diverso_dal_dichiarato_e_un_errore() {
    let un_valore = testo_plain(&[b"a"]);
    let due_valori = testo_plain(&[b"a", b"b"]);
    let dizionario_binario =
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary));
    for (nome, voci, dizionario) in [
        ("zero voci con byte", 0, un_valore.clone()),
        ("due voci, una presente", 2, un_valore),
        ("una voce, byte di due", 1, due_valori),
    ] {
        let byte = parquet(
            &colonna(BYTE_ARRAY),
            1,
            RLE_DICTIONARY,
            &[1, 2, 0],
            Some((voci, &dizionario)),
            None,
        );
        contiene(arrow(&byte), parquet::basic::DICTIONARY_NOT_AS_DECLARED);
        rifiutato_dal_confine(&byte);
        // Il lettore dei dizionari Arrow e quello delle viste: un errore (le
        // viste riconoscono la fine dei dati prima, con il loro testo).
        for tipo in [dizionario_binario.clone(), DataType::BinaryView] {
            let schema = Schema::new(vec![Field::new("x", tipo.clone(), false)]);
            assert!(arrow_con_schema(&byte, schema).is_err(), "{nome} {tipo}");
        }
        assert!(colonna_api::<ByteArrayType>(&byte, None).is_err(), "{nome}");
    }
    // Il decoder generico: due voci dichiarate e i byte di una, o una voce e
    // i byte di due.
    for (voci, byte_dizionario) in [(2, 4), (1, 8)] {
        let dizionario: Vec<u8> = (0..byte_dizionario).collect();
        let byte = parquet(
            &colonna(INT32),
            1,
            RLE_DICTIONARY,
            &[1, 2, 0],
            Some((voci, &dizionario)),
            None,
        );
        if voci == 1 {
            contiene(arrow(&byte), parquet::basic::DICTIONARY_NOT_AS_DECLARED);
            contiene(
                colonna_api::<Int32Type>(&byte, None),
                parquet::basic::DICTIONARY_NOT_AS_DECLARED,
            );
        } else {
            // Meno byte delle voci: l'errore di fine dati del decoder.
            assert!(arrow(&byte).is_err());
            assert!(colonna_api::<Int32Type>(&byte, None).is_err());
        }
        rifiutato_dal_confine(&byte);
    }
    // FLBA: una voce dichiarata, un byte solo.
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let byte = parquet(&flba, 1, RLE_DICTIONARY, &[1, 2, 0], Some((1, b"z")), None);
    contiene(arrow(&byte), parquet::basic::DICTIONARY_NOT_AS_DECLARED);
    // Controfattuali: le voci dichiarate, esatte.
    let byte = parquet(
        &colonna(INT32),
        1,
        RLE_DICTIONARY,
        &[1, 2, 0],
        Some((1, &7_i32.to_le_bytes())),
        None,
    );
    assert_eq!(valori_arrow(&byte), ["7"]);
    assert_eq!(colonna_api::<Int32Type>(&byte, None), Ok(1));
}

/// Una pagina di dizionario in una codifica esclusa ha il testo di ogni altro
/// rifiuto (`Unsupported` al confine), non quello generico del dizionario.
#[test]
fn un_dizionario_in_una_codifica_esclusa_non_e_qualificato() {
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    for (colonna, dizionario) in [
        (colonna(BYTE_ARRAY), testo_plain(&[b"a"])),
        (colonna(INT32), 7_i32.to_le_bytes().to_vec()),
        (flba, b"zz".to_vec()),
    ] {
        let mut byte = parquet(
            &colonna,
            1,
            RLE_DICTIONARY,
            &[1, 2, 0],
            Some((1, &dizionario)),
            None,
        );
        // L'header del dizionario: `num_values` 1 e `encoding` PLAIN (0), il
        // primo campo della struct annidata; diventa DELTA_BINARY_PACKED.
        let codifica = byte
            .windows(4)
            .position(|finestra| finestra == [0x15, 0x02, 0x15, 0x00])
            .unwrap()
            + 3;
        byte[codifica] = u8::try_from(zigzag(DELTA_BINARY_PACKED)).unwrap();
        non_qualificata(&byte);
    }
}

/// Una pagina v2 `OPTIONAL` `INT32`: 8 righe, `num_nulls` dato, i livelli RLE
/// dati e 8 valori `PLAIN`.
fn v2_livelli_contro_valori(livelli: &[u8]) -> Vec<u8> {
    v2_opzionale(8, 0, livelli)
}

fn v2_opzionale(righe: i64, nulli: i64, livelli: &[u8]) -> Vec<u8> {
    let opzionale = Colonna {
        ripetizione: 1,
        ..colonna(INT32)
    };
    let mut contenuto = livelli.to_vec();
    for valore in 0..8_i32 {
        contenuto.extend_from_slice(&valore.to_le_bytes());
    }
    let lunghezza = i64::try_from(contenuto.len()).unwrap();
    let header = Struttura::default()
        .i32(1, 3) // DATA_PAGE_V2
        .i32(2, lunghezza)
        .i32(3, lunghezza)
        .struttura(
            8,
            Struttura::default()
                .i32(1, righe)
                .i32(2, nulli)
                .i32(3, righe)
                .i32(4, PLAIN)
                .i32(5, i64::try_from(livelli.len()).unwrap())
                .i32(6, 0),
        )
        .fine();
    parquet_con_header(&opzionale, righe, PLAIN, &contenuto, &header, None, None)
}

/// Una pagina v2 dichiara i suoi valori (`num_values - num_nulls`); i livelli
/// decidono quanti se ne leggono. Livelli `05 00` (una corsa bit-packed di
/// due gruppi con un byte solo, tutti zero) su 8 righe senza null
/// dichiarati: 8 null in silenzio, con 8 valori nella pagina. Ora la fine
/// della pagina confronta i valori letti con quelli dichiarati.
#[test]
fn una_pagina_v2_con_livelli_contro_i_valori_e_un_errore() {
    let byte = v2_livelli_contro_valori(&[5, 0]);
    contiene(arrow(&byte), "data page V2 declares");
    rifiutato_dal_confine(&byte);
    // Controfattuale: una corsa RLE di 8 livelli 1, gli 8 valori.
    let byte = v2_livelli_contro_valori(&[8 << 1, 1]);
    assert_eq!(
        valori_arrow(&byte),
        ["0", "1", "2", "3", "4", "5", "6", "7"]
    );
}

/// Il tipo dei valori di un dizionario di testo seguiva solo `UTF8`: una
/// colonna `JSON` o `ENUM` letta come `Dictionary(Int32, Binary)` andava in
/// panico, e come `Dictionary(Int32, Utf8)` teneva valori binari. Ora il
/// tipo segue la stessa annotazione della validazione UTF-8 (`UTF8`, `JSON`,
/// `ENUM`), e un dizionario binario su testo è un errore: `Unsupported` dal
/// lettore, esplicito (prima, su `ENUM`, i valori binari passavano perché il
/// tipo interno non seguiva l'annotazione).
#[test]
fn un_dizionario_di_testo_segue_l_annotazione() {
    for (nome, convertito) in [("UTF8", 0), ("ENUM", 4), ("JSON", 19)] {
        let annotata = Colonna {
            convertito: Some(convertito),
            ..colonna(BYTE_ARRAY)
        };
        let byte = parquet(
            &annotata,
            1,
            RLE_DICTIONARY,
            &[1, 2, 0],
            Some((1, &testo_plain(&[b"a"]))),
            None,
        );
        for valori in [
            DataType::Binary,
            DataType::LargeBinary,
            DataType::BinaryView,
            DataType::FixedSizeBinary(1),
        ] {
            // `ENUM` si deduce `Binary`: lo schema dato passa il confronto e
            // arriva al lettore, che lo rifiuta. `UTF8` e `JSON` si deducono
            // `Utf8`, e lo schema si rifiuta già al confronto (anche
            // `FixedSizeBinary` su `ENUM`).
            let al_lettore = convertito == 4 && !matches!(valori, DataType::FixedSizeBinary(_));
            let tipo = DataType::Dictionary(Box::new(DataType::Int32), Box::new(valori));
            let schema = Schema::new(vec![Field::new("x", tipo.clone(), false)]);
            let esito = arrow_con_schema(&byte, schema).map(|batch| batch.len());
            if al_lettore {
                contiene(esito, parquet::basic::BINARY_DICTIONARY_OVER_TEXT);
            } else {
                assert!(esito.is_err(), "{nome} {tipo}");
            }
        }
        for valori in [DataType::Utf8, DataType::LargeUtf8, DataType::Utf8View] {
            let tipo = DataType::Dictionary(Box::new(DataType::Int32), Box::new(valori.clone()));
            let schema = Schema::new(vec![Field::new("x", tipo, false)]);
            let batch = arrow_con_schema(&byte, schema)
                .unwrap_or_else(|errore| panic!("{nome} {valori}: {errore}"));
            let colonna = batch[0].column(0);
            let dizionario = colonna.as_any_dictionary();
            let testo = match dizionario.values().data_type() {
                DataType::Utf8 => dizionario.values().as_string::<i32>().value(0).to_owned(),
                DataType::LargeUtf8 => dizionario.values().as_string::<i64>().value(0).to_owned(),
                DataType::Utf8View => dizionario.values().as_string_view().value(0).to_owned(),
                altro => panic!("{nome}: valori {altro}"),
            };
            assert_eq!(testo, "a", "{nome} {valori}");
        }
    }
}

// --- Salti e fine della pagina -------------------------------------------------

/// Un salto dentro una pagina non decodificava ciò che saltava: una pagina
/// v2 chiusa da un salto sfuggiva al confronto dei nulli, livelli di
/// ripetizione troncati facevano girare il salto senza avanzare, un indice
/// di dizionario fuori dal dizionario passava. Ora il salto dentro una
/// pagina non è qualificato (`SKIP_NOT_QUALIFIED`, prima di leggere un
/// livello); il salto di pagine intere, dall'header, resta.
#[test]
fn un_salto_dentro_una_pagina_non_e_qualificato() {
    let dati = [1_u8, 0, 0, 0, 2, 0, 0, 0];
    let mut troncati = Vec::new();
    varint((100 << 1) | 1, &mut troncati); // ripetizione bit-packed senza byte
    let lista_troncata = ripetuta(2, &troncati, &[2 << 1, 1], &dati);
    let fuori_dizionario = parquet(
        &colonna(BYTE_ARRAY),
        2,
        RLE_DICTIONARY,
        &[2, 4, 3], // larghezza 2, corsa RLE di 2 dell'indice 3
        Some((1, &testo_plain(&[b"a"]))),
        None,
    );
    let v2 = v2_livelli_contro_valori(&[5, 0]);
    let due = parquet(&colonna(INT32), 2, PLAIN, &dati, None, None);
    contiene(
        colonna_api::<Int32Type>(&lista_troncata, Some(1)),
        parquet::basic::SKIP_NOT_QUALIFIED,
    );
    contiene(
        colonna_api::<ByteArrayType>(&fuori_dizionario, Some(1)),
        parquet::basic::SKIP_NOT_QUALIFIED,
    );
    contiene(
        colonna_api::<Int32Type>(&v2, Some(4)),
        parquet::basic::SKIP_NOT_QUALIFIED,
    );
    contiene(
        colonna_api::<Int32Type>(&due, Some(1)),
        parquet::basic::SKIP_NOT_QUALIFIED,
    );
    // Controfattuale: la pagina intera si salta.
    assert_eq!(colonna_api::<Int32Type>(&due, Some(2)), Ok(2));
}

/// Una pagina deve finire dove finiscono i suoi valori e i suoi livelli:
/// valori `PLAIN` in più (`INT32`, `BYTE_ARRAY`, anche come viste, FLBA),
/// una corsa di livelli in più, indici di dizionario in più, un byte dopo
/// lo stream dei booleani `RLE`. Prima si leggevano i valori dichiarati e il
/// resto si ignorava, in silenzio.
#[test]
#[allow(clippy::too_many_lines)] // Un caso per decoder.
fn una_pagina_con_byte_o_valori_in_piu_e_un_errore() {
    let atteso = parquet::basic::PAGE_NOT_AS_DECLARED;
    // INT32: un valore dichiarato, due nella pagina.
    let byte = parquet(
        &colonna(INT32),
        1,
        PLAIN,
        &[7, 0, 0, 0, 8, 0, 0, 0],
        None,
        None,
    );
    contiene(arrow(&byte), atteso);
    contiene(colonna_api::<Int32Type>(&byte, None), atteso);
    rifiutato_dal_confine(&byte);
    // BYTE_ARRAY, anche come viste.
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        PLAIN,
        &testo_plain(&[b"a", b"b"]),
        None,
        None,
    );
    contiene(arrow(&byte), atteso);
    contiene(colonna_api::<ByteArrayType>(&byte, None), atteso);
    rifiutato_dal_confine(&byte);
    let schema = Schema::new(vec![Field::new("x", DataType::BinaryView, false)]);
    contiene(
        arrow_con_schema(&byte, schema).map(|batch| batch.len()),
        atteso,
    );
    // FLBA: due valori di larghezza 2, uno dichiarato.
    let flba = Colonna {
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    let byte = parquet(&flba, 1, PLAIN, b"zzyy", None, None);
    contiene(arrow(&byte), atteso);
    rifiutato_dal_confine(&byte);
    // Livelli di definizione: due corse di 2 per 2 righe (decoder
    // ottimizzato), e lo stesso nei livelli di ripetizione (decoder generico).
    let dati = [1_u8, 0, 0, 0, 2, 0, 0, 0];
    let byte = opzionale_con_livelli(2, &[2 << 1, 1, 2 << 1, 1], &dati);
    contiene(arrow(&byte), atteso);
    rifiutato_dal_confine(&byte);
    let byte = ripetuta(2, &[1 << 1, 0, 1 << 1, 1, 2 << 1, 1], &[2 << 1, 1], &dati);
    contiene(arrow(&byte), atteso);
    // Indici di dizionario: una corsa in più dopo quella del valore.
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        RLE_DICTIONARY,
        &[1, 2, 0, 2, 0],
        Some((1, &testo_plain(&[b"a"]))),
        None,
    );
    contiene(arrow(&byte), atteso);
    contiene(colonna_api::<ByteArrayType>(&byte, None), atteso);
    rifiutato_dal_confine(&byte);
    // Booleani RLE: lunghezza 2, una corsa di un `true`, poi un byte.
    let booleana = colonna(0);
    let byte = parquet(&booleana, 1, RLE, &[2, 0, 0, 0, 2, 1, 0], None, None);
    contiene(arrow(&byte), atteso);
    contiene(
        colonna_api::<parquet::data_type::BoolType>(&byte, None),
        atteso,
    );
    rifiutato_dal_confine(&byte);
    // Controfattuali: le stesse pagine esatte si leggono.
    let byte = parquet(&booleana, 1, RLE, &[2, 0, 0, 0, 2, 1], None, None);
    assert_eq!(arrow(&byte), Ok(1));
    let byte = parquet(&flba, 2, PLAIN, b"zzyy", None, None);
    assert_eq!(valori_arrow(&byte), ["7a7a", "7979"]);
    let byte = opzionale_con_livelli(2, &[2 << 1, 1], &dati);
    assert_eq!(valori_arrow(&byte), ["1", "2"]);
    let byte = parquet(&colonna(INT32), 2, PLAIN, &dati, None, None);
    assert_eq!(colonna_api::<Int32Type>(&byte, None), Ok(2));
}
