//! L'inventario dei campi interi dei tipi Thrift che il fork `parquet`
//! (`vendor/parquet-60.0.0-eof`) decodifica dal footer e dagli header di
//! pagina, ciascuno con la sua regola.
//!
//! Un campo intero letto dal file diventa spesso un conteggio, una
//! dimensione o un offset: negativo, una conversione `as` lo faceva enorme.
//! La prova estrae dal codice del fork ogni campo `i8`, `i16`, `i32` o `i64`,
//! senza dipendere dagli a capo:
//!
//! - dalle definizioni `thrift_struct!` (il contenuto della macro, con
//!   identificatore, presenza, tipo e nome del campo anche su più righe);
//! - dai decoder scritti a mano, dal `match` sugli identificatori dei campi:
//!   un ramo che legge un intero (`i64::read_thrift`, `read_i32()`, una
//!   lista di interi) è un campo intero della struttura del decoder.
//!
//! I commenti non contano. Ogni campo deve avere una regola in [`CAMPI`], e
//! la tabella non deve nominare campi che il codice non ha più. Un campo
//! nuovo, rinominato o letto da un decoder nuovo fa fallire la prova finché
//! non è classificato.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
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
const PAGINA: &str =
    "header di pagina (`file/serialized_reader.rs`, `decode_page_con_larghezza`, `LimitiPagina`)";
const STATISTICHE: &str = "`convert_stats` e `from_thrift_page_stats_con_larghezza`";
const INDICE: &str = "indice di pagina non qualificato (`PAGE_INDEX_NOT_QUALIFIED`)";

/// Ogni campo intero: struttura, identificatore, nome, regola.
const CAMPI: &[(&str, u32, &str, Regola)] = &[
    ("DecimalType", 1, "scale", Regola::Contestuale("DECIMAL: il costruttore dello schema rifiuta una scala negativa o maggiore della precisione")),
    ("DecimalType", 2, "precision", Regola::Contestuale("DECIMAL: il costruttore dello schema rifiuta una precisione sotto 1 o oltre il tipo")),
    ("IntType", 1, "bit_width", Regola::Contestuale("8, 16, 32 o 64, controllata dalla conversione dello schema")),
    ("VariantType", 1, "specification_version", Regola::Valore("versione della specifica, non una dimensione")),
    ("BloomFilterHeader", 1, "num_bytes", Regola::NonLetto("bloom filter non qualificato (`BLOOM_FILTER_NOT_QUALIFIED`)")),
    ("PageEncodingStats", 3, "count", Regola::NonNegativo(PIEDE)),
    ("SortingColumn", 1, "column_idx", Regola::NonNegativo(PIEDE)),
    ("SchemaElement", 2, "type_length", Regola::Contestuale("assente vale -1; una colonna FLBA rifiuta una larghezza negativa (`schema/types.rs`)")),
    ("SchemaElement", 5, "num_children", Regola::NonNegativo("conversione dello schema (`usize::try_from`)")),
    ("SchemaElement", 7, "scale", Regola::Contestuale("come `DecimalType.scale`; assente vale -1")),
    ("SchemaElement", 8, "precision", Regola::Contestuale("come `DecimalType.precision`; assente vale -1")),
    ("SchemaElement", 9, "field_id", Regola::Valore("identificativo del campo, non un indice")),
    ("Statistics", 3, "null_count", Regola::NonNegativo(STATISTICHE)),
    ("Statistics", 4, "distinct_count", Regola::NonNegativo(STATISTICHE)),
    ("Statistics", 9, "nan_count", Regola::NonNegativo(STATISTICHE)),
    ("PageStatistics", 3, "null_count", Regola::NonNegativo(STATISTICHE)),
    ("PageStatistics", 4, "distinct_count", Regola::NonNegativo(STATISTICHE)),
    ("PageStatistics", 9, "nan_count", Regola::NonNegativo(STATISTICHE)),
    ("GeospatialStatistics", 2, "geospatial_types", Regola::Valore("codici dei tipi WKB, conservati come valori, non usati come dimensione o indice")),
    ("SizeStatistics", 1, "unencoded_byte_array_data_bytes", Regola::NonNegativo(PIEDE)),
    ("SizeStatistics", 2, "repetition_level_histogram", Regola::NonNegativo(PIEDE)),
    ("SizeStatistics", 3, "definition_level_histogram", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 5, "num_values", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 6, "total_uncompressed_size", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 7, "total_compressed_size", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 9, "data_page_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 10, "index_page_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 11, "dictionary_page_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 14, "bloom_filter_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnMetaData", 15, "bloom_filter_length", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", 2, "file_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", 4, "offset_index_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", 5, "offset_index_length", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", 6, "column_index_offset", Regola::NonNegativo(PIEDE)),
    ("ColumnChunk", 7, "column_index_length", Regola::NonNegativo(PIEDE)),
    ("RowGroup", 2, "total_byte_size", Regola::NonNegativo(PIEDE)),
    ("RowGroup", 3, "num_rows", Regola::NonNegativo(PIEDE)),
    ("RowGroup", 5, "file_offset", Regola::NonNegativo(PIEDE)),
    ("RowGroup", 6, "total_compressed_size", Regola::NonNegativo(PIEDE)),
    ("RowGroup", 7, "ordinal", Regola::NonNegativo(PIEDE)),
    ("FileMetaData", 1, "version", Regola::Valore("versione del formato, non una dimensione")),
    ("FileMetaData", 3, "num_rows", Regola::NonNegativo(PIEDE)),
    ("DictionaryPageHeader", 1, "num_values", Regola::NonNegativo(PAGINA)),
    ("DataPageHeader", 1, "num_values", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", 1, "num_values", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", 2, "num_nulls", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", 3, "num_rows", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", 5, "definition_levels_byte_length", Regola::NonNegativo(PAGINA)),
    ("DataPageHeaderV2", 6, "repetition_levels_byte_length", Regola::NonNegativo(PAGINA)),
    ("PageHeader", 2, "uncompressed_page_size", Regola::NonNegativo(PAGINA)),
    ("PageHeader", 3, "compressed_page_size", Regola::NonNegativo(PAGINA)),
    ("PageHeader", 4, "crc", Regola::Valore("CRC32, uno schema di bit confrontato con il calcolato")),
    ("ThriftColumnIndex", 5, "null_counts", Regola::NonLetto(INDICE)),
    ("ThriftColumnIndex", 6, "repetition_level_histograms", Regola::NonLetto(INDICE)),
    ("ThriftColumnIndex", 7, "definition_level_histograms", Regola::NonLetto(INDICE)),
    ("ThriftColumnIndex", 8, "nan_counts", Regola::NonLetto(INDICE)),
    ("PageLocation", 1, "offset", Regola::NonLetto(INDICE)),
    ("PageLocation", 2, "compressed_page_size", Regola::NonLetto(INDICE)),
    ("PageLocation", 3, "first_row_index", Regola::NonLetto(INDICE)),
    ("OffsetIndexMetaData", 2, "unencoded_byte_array_data_bytes", Regola::NonLetto(INDICE)),
];

/// La struttura di un decoder scritto a mano che non è in un `impl ... for
/// Struttura`: la funzione che lo contiene.
const DECODER_A_MANO: &[(&str, &str)] = &[
    ("read_column_metadata", "ColumnMetaData"),
    ("read_column_chunk", "ColumnChunk"),
    ("read_row_group", "RowGroup"),
    ("parquet_metadata_from_bytes", "FileMetaData"),
    ("parquet_schema_from_bytes", "FileMetaData"),
];

const INTERI: [&str; 4] = ["i8", "i16", "i32", "i64"];

/// Un campo intero estratto: struttura, identificatore, nome (vuoto per un
/// decoder scritto a mano, che non lo nomina).
type Campo = (String, u32, String);

/// Il testo senza commenti (`//` fino a fine riga, `/* */`): le definizioni
/// nei commenti non sono codice.
fn senza_commenti(testo: &str) -> String {
    let mut fuori = String::with_capacity(testo.len());
    let mut caratteri = testo.chars().peekable();
    let mut in_stringa = false;
    while let Some(c) = caratteri.next() {
        if in_stringa {
            fuori.push(c);
            if c == '\\' {
                if let Some(d) = caratteri.next() {
                    fuori.push(d);
                }
            } else if c == '"' {
                in_stringa = false;
            }
            continue;
        }
        match (c, caratteri.peek()) {
            ('"', _) => {
                in_stringa = true;
                fuori.push(c);
            }
            ('/', Some('/')) => {
                for d in caratteri.by_ref() {
                    if d == '\n' {
                        fuori.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                caratteri.next();
                let mut prima = ' ';
                for d in caratteri.by_ref() {
                    if prima == '*' && d == '/' {
                        break;
                    }
                    prima = d;
                }
                fuori.push(' ');
            }
            _ => fuori.push(c),
        }
    }
    fuori
}

/// Le parole e i segni del testo, senza spazi né a capo.
fn parole(testo: &str) -> Vec<String> {
    let mut parole = Vec::new();
    let mut corrente = String::new();
    for c in testo.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '\'' {
            corrente.push(c);
        } else {
            if !corrente.is_empty() {
                parole.push(std::mem::take(&mut corrente));
            }
            if !c.is_whitespace() {
                parole.push(c.to_string());
            }
        }
    }
    if !corrente.is_empty() {
        parole.push(corrente);
    }
    parole
}

/// La parentesi che chiude quella aperta in `inizio`.
fn chiusura(parole: &[String], inizio: usize, apre: &str, chiude: &str) -> usize {
    let mut profondita = 0;
    for (i, parola) in parole.iter().enumerate().skip(inizio) {
        if parola == apre {
            profondita += 1;
        } else if parola == chiude {
            profondita -= 1;
            if profondita == 0 {
                return i;
            }
        }
    }
    parole.len()
}

fn numero(parola: &str) -> bool {
    !parola.is_empty() && parola.chars().all(|c| c.is_ascii_digit())
}

/// I campi interi delle definizioni `thrift_struct!` del testo.
fn campi_delle_macro(parole: &[String]) -> Vec<Campo> {
    let mut campi = Vec::new();
    let mut i = 0;
    while i + 2 < parole.len() {
        if !(parole[i] == "thrift_struct" && parole[i + 1] == "!" && parole[i + 2] == "(") {
            i += 1;
            continue;
        }
        let fine = chiusura(parole, i + 2, "(", ")");
        let corpo = &parole[i + 3..fine];
        if let Some(s) = corpo.iter().position(|p| p == "struct") {
            let nome = corpo[s + 1].clone();
            let mut j = s + 2;
            while j + 3 < corpo.len() {
                // `id : presenza tipo <'lt>? <elemento>? nome`.
                if numero(&corpo[j])
                    && corpo[j + 1] == ":"
                    && (corpo[j + 2] == "required" || corpo[j + 2] == "optional")
                {
                    let id: u32 = corpo[j].parse().unwrap();
                    let tipo = corpo[j + 3].clone();
                    let mut k = j + 4;
                    let mut elementi = Vec::new();
                    while corpo.get(k).is_some_and(|p| p == "<") {
                        elementi.push(corpo[k + 1].clone());
                        k += 3;
                    }
                    let nome_campo = corpo[k].clone();
                    let intero = INTERI.contains(&tipo.as_str())
                        || ((tipo == "list" || tipo == "set")
                            && elementi.iter().any(|e| INTERI.contains(&e.as_str())));
                    if intero {
                        campi.push((nome.clone(), id, nome_campo));
                    }
                    j = k + 1;
                } else {
                    j += 1;
                }
            }
        }
        i = fine;
    }
    campi
}

/// Il nome della struttura di un decoder scritto a mano: la funzione che lo
/// contiene, nella tabella, o il tipo dell'`impl` che la contiene.
fn struttura_del_decoder(parole: &[String], posizione: usize) -> Option<String> {
    let funzione = (0..posizione)
        .rev()
        .find(|&i| parole[i] == "fn")
        .map(|i| parole[i + 1].clone())?;
    if let Some((_, struttura)) = DECODER_A_MANO.iter().find(|(f, _)| *f == funzione) {
        return Some((*struttura).to_owned());
    }
    let impl_ = (0..posizione).rev().find(|&i| parole[i] == "impl")?;
    let graffa = impl_ + parole[impl_..].iter().position(|p| p == "{")?;
    // La funzione deve stare dentro l'`impl`.
    if chiusura(parole, graffa, "{", "}") < posizione {
        return None;
    }
    // `impl ... for Tipo {`, o `impl Tipo {` (i metodi propri del tipo).
    let intestazione = &parole[impl_..graffa];
    intestazione.iter().position(|p| p == "for").map_or_else(
        || intestazione.get(1).cloned(),
        |f| intestazione.get(f + 1).cloned(),
    )
}

/// Se un ramo di un decoder scritto a mano legge un intero.
fn legge_un_intero(corpo: &[String]) -> bool {
    corpo.windows(4).any(|f| {
        INTERI.contains(&f[0].as_str()) && f[1] == ":" && f[2] == ":" && f[3] == "read_thrift"
    }) || corpo
        .iter()
        .any(|p| INTERI.iter().any(|t| *p == format!("read_{t}")))
        || corpo
            .windows(5)
            .any(|f| f[0] == "read_thrift_vec" && f[3] == "<" && INTERI.contains(&f[4].as_str()))
}

/// I campi interi letti dai decoder scritti a mano (`match field_ident.id`),
/// e i `match` di cui non si trova la struttura.
fn campi_dei_decoder(parole: &[String]) -> (Vec<Campo>, usize) {
    let mut campi = Vec::new();
    let mut non_mappati = 0;
    let mut i = 0;
    while i + 4 < parole.len() {
        let inizio_match = parole[i] == "match"
            && parole[i + 1] == "field_ident"
            && parole[i + 2] == "."
            && parole[i + 3] == "id"
            && parole[i + 4] == "{";
        if !inizio_match {
            i += 1;
            continue;
        }
        let fine = chiusura(parole, i + 4, "{", "}");
        let Some(struttura) = struttura_del_decoder(parole, i) else {
            non_mappati += 1;
            i = fine;
            continue;
        };
        // I rami: `id (if ...)? =>` al primo livello del `match`.
        let mut rami: Vec<(u32, usize)> = Vec::new();
        let mut profondita = 0;
        for j in i + 5..fine {
            match parole[j].as_str() {
                "{" | "(" | "[" => profondita += 1,
                "}" | ")" | "]" => profondita -= 1,
                _ => {}
            }
            if profondita == 0
                && numero(&parole[j])
                && (j == i + 5 || matches!(parole[j - 1].as_str(), "}" | "," | "|"))
            {
                rami.push((parole[j].parse().unwrap(), j));
            }
        }
        for (n, (id, inizio)) in rami.iter().enumerate() {
            let termine = rami.get(n + 1).map_or(fine, |(_, p)| *p);
            if legge_un_intero(&parole[*inizio..termine]) {
                campi.push((struttura.clone(), *id, String::new()));
            }
        }
        i = fine;
    }
    (campi, non_mappati)
}

/// I campi interi di un sorgente (le prove `#[cfg(test)] mod tests` escluse),
/// e i decoder scritti a mano di cui non si trova la struttura.
fn campi_del_sorgente(testo: &str) -> (Vec<Campo>, usize) {
    let testo = senza_commenti(testo);
    let codice = testo
        .split("#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(&testo);
    let parole = parole(codice);
    let mut campi = campi_delle_macro(&parole);
    let (dai_decoder, non_mappati) = campi_dei_decoder(&parole);
    campi.extend(dai_decoder);
    (campi, non_mappati)
}

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

#[test]
fn ogni_campo_intero_del_footer_ha_una_regola() {
    let radice = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/parquet-60.0.0-eof/src");
    let mut file = Vec::new();
    sorgenti(&radice, &mut file);
    // Per (struttura, id): il nome dalla macro, se c'è.
    let mut estratti: BTreeMap<(String, u32), String> = BTreeMap::new();
    for percorso in file {
        // La definizione della macro ha un `match field_ident.id` generico.
        if percorso.ends_with("parquet_macros.rs") {
            continue;
        }
        let testo = std::fs::read_to_string(&percorso).unwrap();
        let (campi, non_mappati) = campi_del_sorgente(&testo);
        assert_eq!(
            non_mappati,
            0,
            "{}: decoder scritti a mano senza struttura in `DECODER_A_MANO`",
            percorso.display()
        );
        for (struttura, id, nome) in campi {
            let voce = estratti.entry((struttura, id)).or_default();
            if voce.is_empty() {
                *voce = nome;
            }
        }
    }
    let classificati: BTreeMap<(String, u32), &str> = CAMPI
        .iter()
        .map(|(s, id, nome, _)| (((*s).to_owned(), *id), *nome))
        .collect();
    let chiavi_estratte: BTreeSet<_> = estratti.keys().cloned().collect();
    let chiavi_classificate: BTreeSet<_> = classificati.keys().cloned().collect();
    let senza_regola: Vec<_> = chiavi_estratte.difference(&chiavi_classificate).collect();
    assert!(
        senza_regola.is_empty(),
        "campi interi senza una regola in `CAMPI`: {senza_regola:?}"
    );
    let superflui: Vec<_> = chiavi_classificate.difference(&chiavi_estratte).collect();
    assert!(
        superflui.is_empty(),
        "regole di campi che il codice non ha più: {superflui:?}"
    );
    assert_eq!(estratti.len(), 60, "il numero dei campi interi del fork");
    // Un campo rinominato nella macro cambia la sua regola.
    for (chiave, nome) in &estratti {
        if !nome.is_empty() {
            assert_eq!(classificati[chiave], nome.as_str(), "{chiave:?}");
        }
    }
    for (struttura, id, campo, regola) in CAMPI {
        let testo = match regola {
            Regola::NonNegativo(t)
            | Regola::Valore(t)
            | Regola::Contestuale(t)
            | Regola::NonLetto(t) => t,
        };
        assert!(!testo.is_empty(), "{struttura}.{id} {campo}");
    }
}

/// L'estrazione non dipende dagli a capo: un campo spezzato su più righe,
/// una struttura su una riga sola, un ramo di decoder su più righe e un
/// decoder in un `impl` si vedono; una definizione in un commento no; un
/// decoder di cui non si trova la struttura è un errore, non un campo perso.
#[test]
fn l_estrazione_non_dipende_dagli_a_capo() {
    let sintetico = r"
thrift_struct!(struct Riga { 1: required i64 conteggio; 2: optional binary<'a> testo; 3: optional list<i32> elenco });
thrift_struct!(
pub struct Spezzata {
  1:
    required
      i32
        larghezza
  2: optional list
     <i64>
     istogramma
}
);
// thrift_struct!(struct Commentata { 1: required i64 nascosto })
/* thrift_struct!(struct AncheCommentata { 1: required i64 nascosto }) */
fn read_column_chunk() {
    match field_ident.id { 7 =>
        { let v =
            i64
            ::read_thrift(&mut *prot)?; }
        8 => { altro()?; }
        _ => {}
    }
}
impl Decodifica for Intestazione {
    fn read_thrift() { match field_ident.id { 4 => { prot.read_i32()?; } _ => {} } }
}
impl Propria {
    fn read_thrift_without_stats() { match field_ident.id { 2 => { let v = i16::read_thrift(p)?; } } }
}
";
    let (campi, non_mappati) = campi_del_sorgente(sintetico);
    assert_eq!(non_mappati, 0);
    for atteso in [
        ("Riga", 1, "conteggio"),
        ("Riga", 3, "elenco"),
        ("Spezzata", 1, "larghezza"),
        ("Spezzata", 2, "istogramma"),
        ("ColumnChunk", 7, ""),
        ("Intestazione", 4, ""),
        ("Propria", 2, ""),
    ] {
        let atteso: Campo = (atteso.0.into(), atteso.1, atteso.2.into());
        assert!(campi.contains(&atteso), "{atteso:?} non visto: {campi:?}");
    }
    assert!(
        !campi
            .iter()
            .any(|(s, _, _)| s == "Commentata" || s == "AncheCommentata"),
        "{campi:?}"
    );
    assert!(
        !campi
            .iter()
            .any(|(s, id, _)| s == "ColumnChunk" && *id == 8),
        "{campi:?}"
    );
    assert!(
        !campi.iter().any(|(s, id, _)| s == "Riga" && *id == 2),
        "{campi:?}"
    );
    let (_, non_mappati) = campi_del_sorgente(
        "fn read_qualcosa() { match field_ident.id { 7 => { i64::read_thrift(p)?; } } }",
    );
    assert_eq!(non_mappati, 1);
}
