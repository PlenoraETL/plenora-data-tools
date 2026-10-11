//! L'inventario dei campi interi dei tipi Thrift che il fork `parquet`
//! (`vendor/parquet-60.0.0-eof`) decodifica dal footer e dagli header di
//! pagina, ciascuno con la sua regola.
//!
//! Un campo intero letto dal file diventa spesso un conteggio, una
//! dimensione o un offset: negativo, una conversione `as` lo faceva enorme.
//! La prova estrae dai sorgenti del fork ogni campo `i8`, `i16`, `i32` o
//! `i64` delle definizioni Thrift (le `thrift_struct!` e le definizioni nei
//! commenti dei decoder scritti a mano) e pretende che ciascuno abbia una
//! regola qui sotto, e che la tabella non nomini campi che non esistono più.
//! Un campo nuovo, o rinominato, fa fallire la prova finché non è
//! classificato.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// La regola di un campo intero.
#[derive(Debug, Clone, Copy)]
enum Regola {
    /// Un conteggio, una dimensione o un offset: negativo è un errore, dove il
    /// fork lo decodifica (il testo dice dove).
    NonNegativo(&'static str),
    /// Un valore che non è una dimensione: il segno non conta.
    Valore(&'static str),
    /// Un valore il cui dominio dipende dal contesto, controllato dove serve.
    Contestuale(&'static str),
    /// Una struttura che il fork non legge: la lettura è un errore esplicito.
    NonLetto(&'static str),
}

const PIEDE: &str = "lettura del footer (`file/metadata/thrift/mod.rs`)";
const PAGINA: &str = "header di pagina (`file/serialized_reader.rs`, `decode_page_con_larghezza`)";
const STATISTICHE: &str = "`convert_stats` e `from_thrift_page_stats_con_larghezza`";
const INDICE: &str = "indice di pagina non qualificato (`PAGE_INDEX_NOT_QUALIFIED`)";

/// Ogni campo intero, con la sua regola.
const CAMPI: &[(&str, &str, Regola)] = &[
    ("DecimalType", "scale", Regola::Contestuale("DECIMAL: il costruttore dello schema rifiuta una scala negativa o maggiore della precisione")),
    ("DecimalType", "precision", Regola::Contestuale("DECIMAL: il costruttore dello schema rifiuta una precisione sotto 1 o oltre il tipo")),
    ("IntType", "bit_width", Regola::Contestuale("8, 16, 32 o 64, controllata dalla conversione dello schema")),
    ("VariantType", "specification_version", Regola::Valore("versione della specifica, non una dimensione")),
    ("BloomFilterHeader", "num_bytes", Regola::NonLetto("bloom filter non qualificato (`BLOOM_FILTER_NOT_QUALIFIED`)")),
    ("PageEncodingStats", "count", Regola::NonNegativo(PIEDE)),
    ("SortingColumn", "column_idx", Regola::NonNegativo(PIEDE)),
    ("SchemaElement", "type_length", Regola::Contestuale("assente vale -1; una colonna FLBA rifiuta una larghezza negativa (`schema/types.rs`)")),
    ("SchemaElement", "num_children", Regola::NonNegativo("conversione dello schema (`usize::try_from`)")),
    ("SchemaElement", "scale", Regola::Contestuale("come `DecimalType.scale`; assente vale -1")),
    ("SchemaElement", "precision", Regola::Contestuale("come `DecimalType.precision`; assente vale -1")),
    ("SchemaElement", "field_id", Regola::Valore("identificativo del campo, non un indice")),
    ("Statistics", "null_count", Regola::NonNegativo(STATISTICHE)),
    ("Statistics", "distinct_count", Regola::NonNegativo(STATISTICHE)),
    ("Statistics", "nan_count", Regola::NonNegativo(STATISTICHE)),
    ("PageStatistics", "null_count", Regola::NonNegativo(STATISTICHE)),
    ("PageStatistics", "distinct_count", Regola::NonNegativo(STATISTICHE)),
    ("PageStatistics", "nan_count", Regola::NonNegativo(STATISTICHE)),
    ("GeospatialStatistics", "geospatial_types", Regola::Valore("codici dei tipi WKB, conservati come valori, non usati come dimensione o indice")),
    ("SizeStatistics", "unencoded_byte_array_data_bytes", Regola::NonNegativo(PIEDE)),
    ("SizeStatistics", "repetition_level_histogram", Regola::NonNegativo(PIEDE)),
    ("SizeStatistics", "definition_level_histogram", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "num_values", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "total_uncompressed_size", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "total_compressed_size", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "data_page_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "index_page_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "dictionary_page_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "bloom_filter_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", "bloom_filter_length", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", "file_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", "offset_index_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", "offset_index_length", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", "column_index_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", "column_index_length", Regola::NonNegativo(PIEDE)),
    ("RowGroup", "total_byte_size", Regola::NonNegativo(PIEDE)),
    ("RowGroup", "num_rows", Regola::NonNegativo(PIEDE)),
    ("RowGroup", "file_offset", Regola::NonNegativo(PIEDE)),
    ("RowGroup", "total_compressed_size", Regola::NonNegativo(PIEDE)),
    ("RowGroup", "ordinal", Regola::NonNegativo(PIEDE)),
    ("FileMetaData", "version", Regola::Valore("versione del formato, non una dimensione")),
    ("FileMetaData", "num_rows", Regola::NonNegativo(PIEDE)),
    ("DictionaryPageHeader", "num_values", Regola::NonNegativo(PAGINA)),
    ("DataPageHeader", "num_values", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", "num_values", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", "num_nulls", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", "num_rows", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", "definition_levels_byte_length", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", "repetition_levels_byte_length", Regola::NonNegativo(PAGINA)),
    ("PageHeader", "uncompressed_page_size", Regola::NonNegativo(PAGINA)),
    ("PageHeader", "compressed_page_size", Regola::NonNegativo(PAGINA)),
    ("PageHeader", "crc", Regola::Valore("CRC32, uno schema di bit confrontato con il calcolato")),
    ("ThriftColumnIndex", "null_counts", Regola::NonLetto(INDICE)),
    ("ThriftColumnIndex", "repetition_level_histograms", Regola::NonLetto(INDICE)),
    ("ThriftColumnIndex", "definition_level_histograms", Regola::NonLetto(INDICE)),
    ("ThriftColumnIndex", "nan_counts", Regola::NonLetto(INDICE)),
    ("PageLocation", "offset", Regola::NonLetto(INDICE)),
    ("PageLocation", "compressed_page_size", Regola::NonLetto(INDICE)),
    ("PageLocation", "first_row_index", Regola::NonLetto(INDICE)),
    ("OffsetIndexMetaData", "unencoded_byte_array_data_bytes", Regola::NonLetto(INDICE)),
];

const INTERI: [&str; 4] = ["i8", "i16", "i32", "i64"];

fn sorgenti(cartella: &Path, file: &mut Vec<PathBuf>) {
    for voce in std::fs::read_dir(cartella).unwrap() {
        let percorso = voce.unwrap().path();
        if percorso.is_dir() {
            sorgenti(&percorso, file);
        } else if percorso.extension().is_some_and(|e| e == "rs") {
            file.push(percorso);
        }
    }
}

/// Il campo intero di una riga di definizione Thrift (`5: required i64
/// num_values`, anche in un commento `//`), se c'è.
fn campo_intero(riga: &str) -> Option<String> {
    let riga = riga.trim().trim_start_matches("//").trim();
    let (id, resto) = riga.split_once(':')?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut parti = resto.split_whitespace();
    let presenza = parti.next()?;
    if presenza != "required" && presenza != "optional" {
        return None;
    }
    let tipo = parti.next()?;
    let interno = tipo
        .strip_prefix("list<")
        .map_or(tipo, |t| t.trim_end_matches('>'));
    if !INTERI.contains(&interno) {
        return None;
    }
    let nome = parti.next()?;
    let nome: String = nome
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!nome.is_empty()).then_some(nome)
}

/// Tutti i campi interi delle definizioni Thrift dei sorgenti del fork.
fn campi_dei_sorgenti() -> BTreeSet<(String, String)> {
    let radice = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/parquet-60.0.0-eof/src");
    let mut file = Vec::new();
    sorgenti(&radice, &mut file);
    let mut campi = BTreeSet::new();
    for percorso in file {
        let testo = std::fs::read_to_string(&percorso).unwrap();
        let mut struttura: Option<String> = None;
        let mut profondita = 0_i32;
        for riga in testo.lines() {
            let pulita = riga.trim().trim_start_matches("//").trim();
            if struttura.is_none() {
                // Una definizione: `struct Nome {` (con `pub`, o in un
                // commento dei decoder scritti a mano).
                let senza_pub = pulita
                    .trim_start_matches("pub(crate) ")
                    .trim_start_matches("pub(super) ")
                    .trim_start_matches("pub ");
                if let Some(resto) = senza_pub.strip_prefix("struct ") {
                    if pulita.ends_with('{') {
                        let nome: String = resto
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .collect();
                        struttura = Some(nome);
                        profondita = 1;
                    }
                }
                continue;
            }
            if let Some(nome) = campo_intero(riga) {
                campi.insert((struttura.clone().unwrap(), nome));
            }
            profondita += i32::try_from(pulita.matches('{').count()).unwrap();
            profondita -= i32::try_from(pulita.matches('}').count()).unwrap();
            if profondita <= 0 {
                struttura = None;
            }
        }
    }
    campi
}

#[test]
fn ogni_campo_intero_del_footer_ha_una_regola() {
    let estratti = campi_dei_sorgenti();
    assert!(
        estratti.len() > 40,
        "estrazione troppo povera ({} campi): il parser della prova è rotto",
        estratti.len()
    );
    let classificati: BTreeSet<(String, String)> = CAMPI
        .iter()
        .map(|(s, c, _)| ((*s).to_owned(), (*c).to_owned()))
        .collect();
    let senza_regola: Vec<_> = estratti.difference(&classificati).collect();
    assert!(
        senza_regola.is_empty(),
        "campi interi senza una regola in `CAMPI`: {senza_regola:?}"
    );
    let superflui: Vec<_> = classificati.difference(&estratti).collect();
    assert!(
        superflui.is_empty(),
        "regole di campi che i sorgenti non hanno più: {superflui:?}"
    );
    // Ogni regola dice dove o perché.
    for (struttura, campo, regola) in CAMPI {
        let testo = match regola {
            Regola::NonNegativo(t)
            | Regola::Valore(t)
            | Regola::Contestuale(t)
            | Regola::NonLetto(t) => t,
        };
        assert!(!testo.is_empty(), "{struttura}.{campo}");
    }
}
