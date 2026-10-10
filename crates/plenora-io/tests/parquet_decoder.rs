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

use std::sync::Arc;

use parquet::arrow::arrow_reader::statistics::StatisticsConverter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::column::reader::get_typed_column_reader;
use parquet::data_type::{ByteArrayType, DoubleType};
use parquet::file::reader::{FileReader, SerializedFileReader};
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
    /// `0` `REQUIRED`, `1` `OPTIONAL` (livelli di definizione nella pagina).
    ripetizione: i64,
}

const fn colonna(tipo: i64) -> Colonna {
    Colonna {
        tipo,
        larghezza: None,
        decimale: None,
        ripetizione: 0,
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
/// non passa dal decoder (il lettore salta la pagina): i salti delle prove
/// sono parziali. Un panico fa fallire la prova.
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

    let mut delta = delta_costante(&[0, 1]); // prefissi
    delta.extend(delta_costante(&[1, 1])); // suffissi
    delta.extend_from_slice(b"ab");
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        2,
        DELTA_BYTE_ARRAY,
        &delta,
        None,
        None,
    );
    assert_eq!(arrow(&byte), Ok(2), "\"a\", \"ab\"");

    let doppi: Vec<u8> = (0..8_u32)
        .flat_map(|i| f64::from(i).to_le_bytes())
        .collect();
    let byte = parquet(&colonna(DOUBLE), 8, PLAIN, &doppi, None, None);
    assert_eq!(arrow(&byte), Ok(8));
}

/// Punto 1: `DELTA_BYTE_ARRAY`: un prefisso più lungo del valore precedente.
/// `truncate` lo accettava in silenzio: «a» poi prefisso 5 e suffisso «b»
/// dava «ab», un valore che nessun writer ha scritto.
#[test]
fn un_prefisso_oltre_il_valore_precedente_e_un_errore() {
    let mut delta = delta_costante(&[0, 5]);
    delta.extend(delta_costante(&[1, 1]));
    delta.extend_from_slice(b"ab");
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        2,
        DELTA_BYTE_ARRAY,
        &delta,
        None,
        None,
    );
    contiene(arrow(&byte), "prefix length");
    rifiutato_dal_confine(&byte);
    // L'API per colonne lo rifiutava già.
    contiene(colonna_api::<ByteArrayType>(&byte, None), "prefix length");
}

/// Punto 2: `DELTA_BYTE_ARRAY`: una lunghezza di suffisso negativa, `-1 as usize`,
/// faceva traboccare la fine del suffisso (lettura e salto).
#[test]
fn un_suffisso_negativo_e_un_errore() {
    let mut delta = delta_costante(&[0]);
    delta.extend(delta_costante(&[-1]));
    delta.extend_from_slice(b"ab");
    let byte = parquet(
        &colonna(BYTE_ARRAY),
        1,
        DELTA_BYTE_ARRAY,
        &delta,
        None,
        None,
    );
    contiene(arrow(&byte), "suffix length");
    rifiutato_dal_confine(&byte);
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

/// Punto 4: `DELTA_LENGTH_BYTE_ARRAY` nell'API per colonne: una lunghezza
/// negativa o oltre i byte tagliava fuori dalla pagina; la somma in `i32` del
/// salto traboccava. Il lettore Arrow li rifiutava già: resta la prova.
#[test]
fn le_lunghezze_delta_negative_o_oltre_i_byte_sono_un_errore() {
    for lunghezze in [
        vec![-1, 0],
        vec![100, 101],
        vec![i64::from(i32::MAX), i64::from(i32::MAX)],
    ] {
        let mut contenuto = delta_costante(&lunghezze);
        contenuto.extend_from_slice(b"abc");
        let quanti = i64::try_from(lunghezze.len()).unwrap();
        let byte = parquet(
            &colonna(BYTE_ARRAY),
            quanti,
            DELTA_LENGTH_BYTE_ARRAY,
            &contenuto,
            None,
            None,
        );
        for salta in [None, Some(1)] {
            let esito = colonna_api::<ByteArrayType>(&byte, salta);
            assert!(esito.is_err(), "{lunghezze:?}, salta={salta:?}: {esito:?}");
        }
        assert!(arrow(&byte).is_err(), "{lunghezze:?}");
        rifiutato_dal_confine(&byte);
    }
}

/// Punto 5: `DELTA_BINARY_PACKED`: un `block_size` di 2^62 (multiplo di 128, un
/// miniblocco da 2^62 valori, multiplo di 32: passa i controlli
/// dell'header) faceva traboccare `larghezza * valori_per_miniblocco`.
#[test]
fn un_blocco_delta_enorme_e_un_errore() {
    let mut contenuto = Vec::new();
    varint(1 << 62, &mut contenuto);
    varint(1, &mut contenuto);
    varint(2, &mut contenuto);
    varint(zigzag(7), &mut contenuto);
    varint(zigzag(1), &mut contenuto); // min_delta del blocco
    contenuto.push(8); // larghezza dell'unico miniblocco
    contenuto.extend_from_slice(&[0; 8]);
    let byte = parquet(
        &colonna(INT32),
        2,
        DELTA_BINARY_PACKED,
        &contenuto,
        None,
        None,
    );
    contiene(arrow(&byte), "delta block size overflows");
    rifiutato_dal_confine(&byte);
}

/// Punto 6: `BYTE_STREAM_SPLIT` e `PLAIN` a larghezza fissa: un resto della
/// divisione per la larghezza era scartato in silenzio (65 byte di otto
/// `DOUBLE`: l'ultimo ignorato, il file si leggeva).
#[test]
fn un_resto_nella_pagina_e_un_errore() {
    let mut doppi: Vec<u8> = (0..8_u32)
        .flat_map(|i| f64::from(i).to_le_bytes())
        .collect();
    doppi.push(0xAB);
    let byte = parquet(&colonna(DOUBLE), 8, BYTE_STREAM_SPLIT, &doppi, None, None);
    contiene(arrow(&byte), "not a whole number");
    rifiutato_dal_confine(&byte);
    contiene(colonna_api::<DoubleType>(&byte, None), "not a whole number");

    let flba = Colonna {
        tipo: FLBA,
        larghezza: Some(2),
        ..colonna(FLBA)
    };
    for codifica in [PLAIN, BYTE_STREAM_SPLIT] {
        let byte = parquet(&flba, 2, codifica, b"abcde", None, None);
        contiene(arrow(&byte), "not a whole number");
        rifiutato_dal_confine(&byte);
    }
}

/// Punto 6, salto: `BYTE_STREAM_SPLIT` dichiarava saltati valori oltre i byte
/// della pagina (sedici dichiarati, otto nei 64 byte).
#[test]
fn un_salto_oltre_i_byte_della_pagina_e_un_errore() {
    let doppi: Vec<u8> = (0..8_u32)
        .flat_map(|i| f64::from(i).to_le_bytes())
        .collect();
    let byte = parquet(&colonna(DOUBLE), 16, BYTE_STREAM_SPLIT, &doppi, None, None);
    contiene(
        colonna_api::<DoubleType>(&byte, Some(10)),
        "values requested beyond",
    );
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
    // Il salto non aveva nessun controllo: una lunghezza oltre la pagina.
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
        "Not enough bytes",
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

/// Stessa classe, pagina v1: i livelli `BIT_PACKED` dichiarati (cento valori,
/// tredici byte) oltre i byte della pagina tagliavano fuori dai limiti.
#[test]
fn i_livelli_bit_packed_oltre_la_pagina_sono_un_errore() {
    let opzionale = Colonna {
        ripetizione: 1,
        ..colonna(INT32)
    };
    let contenuto = [0xFF_u8, 0xFF];
    let header = Struttura::default()
        .i32(1, 0)
        .i32(2, 2)
        .i32(3, 2)
        .struttura(
            5,
            Struttura::default()
                .i32(1, 100)
                .i32(2, PLAIN)
                .i32(3, 4) // BIT_PACKED
                .i32(4, RLE),
        )
        .fine();
    let byte = parquet_con_header(&opzionale, 100, PLAIN, &contenuto, &header, None, None);
    contiene(arrow(&byte), "not enough data to read levels");
    rifiutato_dal_confine(&byte);
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
    vec![
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
    ]
}

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
        rifiutato_dal_confine(byte);
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
