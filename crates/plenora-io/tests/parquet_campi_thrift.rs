//! L'inventario dei campi interi dei tipi Thrift che il fork `parquet`
//! (`vendor/parquet-60.0.0-eof`) decodifica dal footer e dagli header di
//! pagina, ciascuno con la sua regola.
//!
//! Un campo intero letto dal file diventa spesso un conteggio, una
//! dimensione o un offset: negativo, una conversione `as` lo faceva enorme.
//! La prova legge il codice del fork con il lexer di Rust di `proc-macro2`
//! (stringhe, raw string e commenti annidati come il compilatore; i commenti
//! non sono token) ed estrae ogni campo `i8`, `i16`, `i32` o `i64`:
//!
//! - dalle invocazioni di `thrift_struct!` e `thrift_union*!`, con
//!   qualunque delimitatore (identificatore, presenza, tipo e nome);
//! - dai decoder scritti a mano: un `match` i cui rami sono letterali interi
//!   (gli identificatori dei campi) o un ramo jolly, e che legge o salta
//!   valori (`read_*`, `skip*`). La forma decide, non il nome della
//!   variabile: un ramo che legge un intero (`i64::read_thrift`,
//!   `read_i32()`, una lista di interi) è un campo intero della struttura
//!   del decoder.
//!
//! Le voci `#[cfg(test)]` sono escluse. Una macro sconosciuta che contiene
//! un campo `required`/`optional` intero fa fallire la prova, come un
//! decoder di cui non si trova la struttura. Ogni campo deve avere una
//! regola in [`CAMPI`], e la tabella non deve nominare campi che il codice
//! non ha più.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

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

/// Le macro che definiscono tipi Thrift e di cui la prova legge i campi;
/// `thrift_enum!` definisce valori, non campi.
const MACRO_NOTE: &[&str] = &[
    "thrift_struct",
    "thrift_union",
    "thrift_union_all_empty",
    "thrift_union_with_unknown",
    "thrift_enum",
];

const INTERI: [&str; 4] = ["i8", "i16", "i32", "i64"];

/// Un campo intero estratto: struttura, identificatore, nome (vuoto per un
/// decoder scritto a mano, che non lo nomina).
type Campo = (String, u32, String);

/// Ciò che si estrae da un sorgente.
#[derive(Default, Debug)]
struct Estratto {
    campi: Vec<Campo>,
    /// Decoder scritti a mano di cui non si trova la struttura.
    non_mappati: usize,
    /// Macro sconosciute con un campo `required`/`optional` intero.
    macro_sconosciute: Vec<String>,
    /// Decoder scritti a mano trovati, con la loro struttura.
    decoder: Vec<String>,
}

/// La funzione e l'`impl` che contengono il punto visitato.
#[derive(Default, Clone)]
struct Contesto {
    funzione: Option<String>,
    tipo: Option<String>,
}

fn alberi(flusso: &TokenStream) -> Vec<TokenTree> {
    flusso.clone().into_iter().collect()
}

/// I token come testo, con le parentesi dei gruppi: per i confronti.
fn appiattisci(flusso: &TokenStream, fuori: &mut Vec<String>) {
    for albero in flusso.clone() {
        match albero {
            TokenTree::Group(gruppo) => {
                let (apre, chiude) = match gruppo.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::None => ("", ""),
                };
                if !apre.is_empty() {
                    fuori.push(apre.to_owned());
                }
                appiattisci(&gruppo.stream(), fuori);
                if !chiude.is_empty() {
                    fuori.push(chiude.to_owned());
                }
            }
            TokenTree::Ident(nome) => fuori.push(nome.to_string()),
            TokenTree::Punct(segno) => fuori.push(segno.as_char().to_string()),
            TokenTree::Literal(letterale) => fuori.push(letterale.to_string()),
        }
    }
}

fn piatto(alberi: &[TokenTree]) -> Vec<String> {
    let mut fuori = Vec::new();
    appiattisci(&alberi.iter().cloned().collect(), &mut fuori);
    fuori
}

fn identificatore(albero: Option<&TokenTree>) -> Option<String> {
    match albero {
        Some(TokenTree::Ident(nome)) => Some(nome.to_string()),
        _ => None,
    }
}

fn segno(albero: Option<&TokenTree>, carattere: char) -> bool {
    matches!(albero, Some(TokenTree::Punct(p)) if p.as_char() == carattere)
}

fn gruppo(albero: Option<&TokenTree>, delimitatore: Delimiter) -> Option<TokenStream> {
    match albero {
        Some(TokenTree::Group(g)) if g.delimiter() == delimitatore => Some(g.stream()),
        _ => None,
    }
}

/// Un letterale intero senza suffisso (l'identificatore di un campo).
fn intero(albero: Option<&TokenTree>) -> Option<u32> {
    match albero {
        Some(TokenTree::Literal(letterale)) => letterale.to_string().parse().ok(),
        _ => None,
    }
}

/// Se `alberi[i..]` comincia con un attributo `#[...]` (o `#![...]`): la
/// sua lunghezza e il suo contenuto.
fn attributo(alberi: &[TokenTree], i: usize) -> Option<(usize, Vec<String>)> {
    if !segno(alberi.get(i), '#') {
        return None;
    }
    let (salto, posizione) = if segno(alberi.get(i + 1), '!') {
        (3, i + 2)
    } else {
        (2, i + 1)
    };
    let contenuto = gruppo(alberi.get(posizione), Delimiter::Bracket)?;
    let mut testo = Vec::new();
    appiattisci(&contenuto, &mut testo);
    Some((salto, testo))
}

/// La fine della voce che comincia in `i` (dopo i suoi attributi): il primo
/// gruppo `{}` o il primo `;` al livello della voce, inclusi.
fn fine_della_voce(alberi: &[TokenTree], i: usize) -> usize {
    let mut j = i;
    while j < alberi.len() {
        if gruppo(alberi.get(j), Delimiter::Brace).is_some() || segno(alberi.get(j), ';') {
            return j + 1;
        }
        j += 1;
    }
    alberi.len()
}

/// Il primo gruppo `{}` da `i`, al livello corrente.
fn prossimo_blocco(alberi: &[TokenTree], i: usize) -> Option<usize> {
    (i..alberi.len()).find(|&j| gruppo(alberi.get(j), Delimiter::Brace).is_some())
}

/// Salta i parametri generici `<...>` che cominciano in `i`; `->` non chiude.
fn dopo_i_generici(alberi: &[TokenTree], mut i: usize) -> usize {
    if !segno(alberi.get(i), '<') {
        return i;
    }
    let mut profondita = 0_i32;
    while i < alberi.len() {
        if segno(alberi.get(i), '<') {
            profondita += 1;
        } else if segno(alberi.get(i), '>') && !freccia(alberi, i) {
            profondita -= 1;
            if profondita == 0 {
                return i + 1;
            }
        }
        i += 1;
    }
    i
}

fn freccia(alberi: &[TokenTree], i: usize) -> bool {
    i > 0
        && matches!(
            alberi.get(i - 1),
            Some(TokenTree::Punct(p)) if p.as_char() == '-' && p.spacing() == Spacing::Joint
        )
}

/// L'ultimo identificatore del percorso che comincia in `i` (`a::b::Tipo`).
fn tipo_del_percorso(alberi: &[TokenTree], mut i: usize) -> Option<String> {
    let mut ultimo = None;
    loop {
        if segno(alberi.get(i), ':') {
            i += 1;
            continue;
        }
        match identificatore(alberi.get(i)) {
            Some(nome) if nome != "for" && nome != "where" => {
                ultimo = Some(nome);
                i += 1;
            }
            _ => return ultimo,
        }
    }
}

/// Il tipo di un `impl` dalla sua intestazione: dopo `for`, o il primo
/// percorso (`impl<...> Tipo<...>`).
fn tipo_dell_impl(intestazione: &[TokenTree]) -> Option<String> {
    let inizio = dopo_i_generici(intestazione, 0);
    let mut profondita = 0_i32;
    for (j, albero) in intestazione.iter().enumerate().skip(inizio) {
        if segno(Some(albero), '<') {
            profondita += 1;
        } else if segno(Some(albero), '>') && !freccia(intestazione, j) {
            profondita -= 1;
        } else if profondita == 0 && identificatore(Some(albero)).as_deref() == Some("for") {
            return tipo_del_percorso(intestazione, j + 1);
        }
    }
    tipo_del_percorso(intestazione, inizio)
}

/// Il tipo di un campo e i tipi dei suoi elementi (`list<i64>`), dalla
/// posizione `i`; restituisce anche la posizione del nome. Le lifetime
/// (`binary<'a>`) non sono tipi.
fn tipo_del_campo(alberi: &[TokenTree], i: usize) -> Option<(String, Vec<String>, usize)> {
    let tipo = identificatore(alberi.get(i))?;
    let mut elementi = Vec::new();
    let mut j = i + 1;
    while segno(alberi.get(j), '<') {
        let fine = dopo_i_generici(alberi, j);
        for k in j..fine {
            if let Some(nome) = identificatore(alberi.get(k)) {
                if !segno(alberi.get(k.wrapping_sub(1)), '\'') {
                    elementi.push(nome);
                }
            }
        }
        j = fine;
    }
    Some((tipo, elementi, j))
}

fn e_intero(tipo: &str, elementi: &[String]) -> bool {
    INTERI.contains(&tipo)
        || ((tipo == "list" || tipo == "set")
            && elementi.iter().any(|e| INTERI.contains(&e.as_str())))
}

/// I campi interi di una definizione `thrift_struct!` o `thrift_union*!`:
/// `id : (required|optional)? tipo nome`, nel blocco dopo `struct Nome` o
/// `union Nome`.
fn campi_della_definizione(contenuto: &TokenStream) -> Vec<Campo> {
    let alberi = alberi(contenuto);
    let Some(parola) = alberi
        .iter()
        .position(|a| matches!(identificatore(Some(a)).as_deref(), Some("struct" | "union")))
    else {
        return Vec::new();
    };
    let Some(nome) = identificatore(alberi.get(parola + 1)) else {
        return Vec::new();
    };
    let Some(blocco) = prossimo_blocco(&alberi, parola + 2) else {
        return Vec::new();
    };
    let corpo = gruppo(alberi.get(blocco), Delimiter::Brace)
        .map(|g| self::alberi(&g))
        .unwrap();
    let mut campi = Vec::new();
    let mut i = 0;
    while i < corpo.len() {
        if let Some((salto, _)) = attributo(&corpo, i) {
            i += salto;
            continue;
        }
        let Some(id) = intero(corpo.get(i)) else {
            i += 1;
            continue;
        };
        if !segno(corpo.get(i + 1), ':') {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        if matches!(
            identificatore(corpo.get(j)).as_deref(),
            Some("required" | "optional")
        ) {
            j += 1;
        }
        let Some((tipo, elementi, dopo)) = tipo_del_campo(&corpo, j) else {
            i += 1;
            continue;
        };
        let nome_campo = identificatore(corpo.get(dopo)).unwrap_or_default();
        if e_intero(&tipo, &elementi) {
            campi.push((nome.clone(), id, nome_campo));
        }
        i = dopo + 1;
    }
    campi
}

/// Se il contenuto di una macro ha un campo `id : required|optional` intero.
fn ha_un_campo_intero(contenuto: &TokenStream) -> bool {
    let alberi = alberi(contenuto);
    let diretto = (0..alberi.len()).any(|i| {
        intero(alberi.get(i)).is_some()
            && segno(alberi.get(i + 1), ':')
            && matches!(
                identificatore(alberi.get(i + 2)).as_deref(),
                Some("required" | "optional")
            )
            && tipo_del_campo(&alberi, i + 3).is_some_and(|(t, e, _)| e_intero(&t, &e))
    });
    diretto
        || alberi.iter().any(|a| match a {
            TokenTree::Group(g) => ha_un_campo_intero(&g.stream()),
            _ => false,
        })
}

/// Il pattern di un ramo: gli identificatori interi (`1 | 2`), un jolly
/// (`_` o un nome), o altro. Una guardia `if` non conta.
enum Pattern {
    Interi(Vec<u32>),
    Jolly,
    Altro,
}

fn pattern(alberi: &[TokenTree]) -> Pattern {
    // Gli attributi del ramo (`#[cfg(feature = "encryption")] 8 => ...`).
    let mut inizio = 0;
    while let Some((salto, _)) = attributo(alberi, inizio) {
        inizio += salto;
    }
    let alberi = &alberi[inizio..];
    let fine = alberi
        .iter()
        .position(|a| identificatore(Some(a)).as_deref() == Some("if"))
        .unwrap_or(alberi.len());
    let alberi = &alberi[..fine];
    if alberi.len() == 1 && identificatore(alberi.first()).is_some() {
        return Pattern::Jolly;
    }
    let mut interi = Vec::new();
    for (k, albero) in alberi.iter().enumerate() {
        if k % 2 == 0 {
            match intero(Some(albero)) {
                Some(id) => interi.push(id),
                None => return Pattern::Altro,
            }
        } else if !segno(Some(albero), '|') {
            return Pattern::Altro;
        }
    }
    if interi.is_empty() {
        Pattern::Altro
    } else {
        Pattern::Interi(interi)
    }
}

/// I rami di un `match`: pattern e corpo.
fn rami(corpo: &[TokenTree]) -> Vec<(Vec<TokenTree>, Vec<TokenTree>)> {
    let mut rami = Vec::new();
    let mut i = 0;
    while i < corpo.len() {
        let Some(freccia) = (i..corpo.len()).find(|&j| {
            matches!(corpo.get(j), Some(TokenTree::Punct(p)) if p.as_char() == '=' && p.spacing() == Spacing::Joint)
                && segno(corpo.get(j + 1), '>')
        }) else {
            break;
        };
        let pattern = corpo[i..freccia].to_vec();
        let inizio = freccia + 2;
        let fine = if gruppo(corpo.get(inizio), Delimiter::Brace).is_some() {
            inizio + 1
        } else {
            (inizio..corpo.len())
                .find(|&j| segno(corpo.get(j), ','))
                .unwrap_or(corpo.len())
        };
        rami.push((pattern, corpo[inizio..fine].to_vec()));
        i = fine;
        if segno(corpo.get(i), ',') {
            i += 1;
        }
    }
    rami
}

/// Se un corpo legge o salta valori: una chiamata `read_*` o `skip*`.
fn legge_thrift(corpo: &[String]) -> bool {
    corpo
        .iter()
        .any(|p| p.starts_with("read_") || p.starts_with("skip"))
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

/// Un `match` sugli identificatori dei campi: i rami sono interi o un
/// jolly, almeno uno intero, e un ramo legge valori Thrift.
fn decoder_dei_campi(corpo: &[TokenTree], contesto: &Contesto, estratto: &mut Estratto) -> bool {
    let rami = rami(corpo);
    let mut con_interi = Vec::new();
    for (p, corpo) in &rami {
        match pattern(p) {
            Pattern::Interi(id) => con_interi.push((id, piatto(corpo))),
            Pattern::Jolly => {}
            Pattern::Altro => return false,
        }
    }
    if con_interi.is_empty() || !con_interi.iter().any(|(_, c)| legge_thrift(c)) {
        return false;
    }
    let funzione = contesto.funzione.as_deref().unwrap_or_default();
    let struttura = DECODER_A_MANO
        .iter()
        .find(|(f, _)| *f == funzione)
        .map(|(_, s)| (*s).to_owned())
        .or_else(|| contesto.tipo.clone());
    let Some(struttura) = struttura else {
        estratto.non_mappati += 1;
        return true;
    };
    estratto.decoder.push(struttura.clone());
    for (id, corpo) in con_interi {
        if legge_un_intero(&corpo) {
            for id in id {
                estratto.campi.push((struttura.clone(), id, String::new()));
            }
        }
    }
    true
}

fn visita(alberi: &[TokenTree], contesto: &Contesto, estratto: &mut Estratto) {
    let mut i = 0;
    while i < alberi.len() {
        // Le voci `#[cfg(test)]`: escluse fino alla fine della voce.
        if let Some((salto, testo)) = attributo(alberi, i) {
            if testo == ["cfg", "(", "test", ")"] {
                i = fine_della_voce(alberi, i + salto);
            } else {
                i += salto;
            }
            continue;
        }
        let nome = identificatore(alberi.get(i));
        // `macro_rules! nome { ... }`: la definizione, non un'invocazione.
        if nome.as_deref() == Some("macro_rules") && segno(alberi.get(i + 1), '!') {
            i += 4;
            continue;
        }
        if let (Some(macro_), true, Some(TokenTree::Group(g))) = (
            nome.as_deref(),
            segno(alberi.get(i + 1), '!'),
            alberi.get(i + 2),
        ) {
            if MACRO_NOTE.contains(&macro_) {
                if macro_ != "thrift_enum" {
                    estratto.campi.extend(campi_della_definizione(&g.stream()));
                }
            } else {
                if ha_un_campo_intero(&g.stream()) {
                    estratto.macro_sconosciute.push(macro_.to_owned());
                }
                visita(&self::alberi(&g.stream()), contesto, estratto);
            }
            i += 3;
            continue;
        }
        match nome.as_deref() {
            Some("impl") => {
                if let Some(blocco) = prossimo_blocco(alberi, i + 1) {
                    let interno = Contesto {
                        funzione: None,
                        tipo: tipo_dell_impl(&alberi[i + 1..blocco]),
                    };
                    let corpo = gruppo(alberi.get(blocco), Delimiter::Brace).unwrap();
                    visita(&self::alberi(&corpo), &interno, estratto);
                    i = blocco + 1;
                    continue;
                }
            }
            Some("fn") => {
                let funzione = identificatore(alberi.get(i + 1));
                let fine = fine_della_voce(alberi, i + 1);
                if let Some(corpo) = gruppo(alberi.get(fine - 1), Delimiter::Brace) {
                    let interno = Contesto {
                        funzione,
                        tipo: contesto.tipo.clone(),
                    };
                    visita(&self::alberi(&corpo), &interno, estratto);
                }
                i = fine;
                continue;
            }
            Some("match") => {
                if let Some(blocco) = prossimo_blocco(alberi, i + 1) {
                    let corpo =
                        self::alberi(&gruppo(alberi.get(blocco), Delimiter::Brace).unwrap());
                    decoder_dei_campi(&corpo, contesto, estratto);
                    visita(&alberi[i + 1..blocco], contesto, estratto);
                    visita(&corpo, contesto, estratto);
                    i = blocco + 1;
                    continue;
                }
            }
            _ => {}
        }
        if let Some(TokenTree::Group(g)) = alberi.get(i) {
            visita(&self::alberi(&g.stream()), contesto, estratto);
        }
        i += 1;
    }
}

/// I campi interi di un sorgente, con il lexer di Rust.
fn estrai(testo: &str) -> Estratto {
    let flusso = TokenStream::from_str(testo).expect("un sorgente Rust valido");
    let mut estratto = Estratto::default();
    visita(&alberi(&flusso), &Contesto::default(), &mut estratto);
    estratto
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
    file.sort();
    // Per (struttura, id): il nome dalla macro, se c'è.
    let mut estratti: BTreeMap<(String, u32), String> = BTreeMap::new();
    let mut decoder = Vec::new();
    for percorso in file {
        let testo = std::fs::read_to_string(&percorso).unwrap();
        let dal_file = estrai(&testo);
        assert_eq!(
            dal_file.non_mappati,
            0,
            "{}: decoder scritti a mano senza struttura in `DECODER_A_MANO`",
            percorso.display()
        );
        assert!(
            dal_file.macro_sconosciute.is_empty(),
            "{}: macro sconosciute con campi interi: {:?}",
            percorso.display(),
            dal_file.macro_sconosciute
        );
        decoder.extend(dal_file.decoder);
        for (struttura, id, nome) in dal_file.campi {
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
    // I decoder scritti a mano del footer e di `ColumnOrder`, trovati per
    // forma.
    decoder.sort();
    assert_eq!(
        decoder,
        [
            "ColumnChunk",
            "ColumnMetaData",
            "ColumnOrder",
            "FileMetaData",
            "FileMetaData",
            "RowGroup"
        ],
        "i decoder scritti a mano del fork"
    );
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

fn contiene(estratto: &Estratto, struttura: &str, id: u32, nome: &str) -> bool {
    estratto
        .campi
        .contains(&(struttura.to_owned(), id, nome.to_owned()))
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
impl<'a, R: Protocollo<'a>> Decodifica<'a, R> for Intestazione<'a> {
    fn read_thrift() { match field_ident.id { 4 => { prot.read_i32()?; } _ => {} } }
}
impl Propria {
    fn leggi() { match field_ident.id { 2 => { let v = i16::read_thrift(p)?; } _ => {} } }
}
";
    let estratto = estrai(sintetico);
    assert_eq!(estratto.non_mappati, 0);
    for (struttura, id, nome) in [
        ("Riga", 1, "conteggio"),
        ("Riga", 3, "elenco"),
        ("Spezzata", 1, "larghezza"),
        ("Spezzata", 2, "istogramma"),
        ("ColumnChunk", 7, ""),
        ("Intestazione", 4, ""),
        ("Propria", 2, ""),
    ] {
        assert!(
            contiene(&estratto, struttura, id, nome),
            "{struttura}.{id} non visto: {:?}",
            estratto.campi
        );
    }
    let strutture: BTreeSet<_> = estratto.campi.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(!strutture.contains("Commentata"), "{strutture:?}");
    assert!(!strutture.contains("AncheCommentata"), "{strutture:?}");
    assert!(!contiene(&estratto, "ColumnChunk", 8, ""));
    assert!(!contiene(&estratto, "Riga", 2, "testo"));
    let estratto = estrai(
        "fn read_qualcosa() { match field_ident.id { 7 => { i64::read_thrift(p)?; } _ => {} } }",
    );
    assert_eq!(estratto.non_mappati, 1);
}

/// I casi che un lexer scritto a mano sbagliava: una `}` in una stringa o in
/// una raw string chiudeva il `match`; un commento annidato finiva al primo
/// `*/`; `thrift_struct!{...}` e `thrift_struct![...]` si ignoravano;
/// `#[cfg(test)]` si escludeva solo seguito da un a capo e da `mod tests`; il
/// `match` si trovava solo sulla variabile `field_ident`.
#[test]
fn l_estrazione_regge_stringhe_commenti_delimitatori_e_nomi() {
    let sintetico = r##"
impl Decodifica for Stringhe {
    fn read_thrift() {
        match campo.id {
            1 => { let s = "}"; let r = r#"} { }"#; let c = '}'; let v = i64::read_thrift(p)?; }
            2 => { let v = i32::read_thrift(p)?; }
            _ => { prot.skip(tipo)?; }
        }
    }
}
/* esterno /* annidato */ thrift_struct!(struct Annidata { 1: required i64 nascosto }) */
thrift_struct!{ struct Graffe { 1: required i64 a } }
thrift_struct![ struct Quadre { 1: optional i32 b } ];
#[cfg(test)] mod prove { thrift_struct!(struct DiProva { 1: required i64 c }); }
#[cfg(test)]

fn prova_sola() { thrift_struct!(struct AncheDiProva { 1: required i64 d }); }
#[cfg(any(test, feature = "x"))]
thrift_struct!(struct Condizionata { 1: required i64 e });
thrift_union!(union Unione { 1: i64 numero 2: Altro altro });
"##;
    let estratto = estrai(sintetico);
    assert_eq!(estratto.non_mappati, 0);
    for (struttura, id, nome) in [
        ("Stringhe", 1, ""),
        ("Stringhe", 2, ""),
        ("Graffe", 1, "a"),
        ("Quadre", 1, "b"),
        ("Condizionata", 1, "e"),
        ("Unione", 1, "numero"),
    ] {
        assert!(
            contiene(&estratto, struttura, id, nome),
            "{struttura}.{id} non visto: {:?}",
            estratto.campi
        );
    }
    let strutture: BTreeSet<_> = estratto.campi.iter().map(|(s, _, _)| s.as_str()).collect();
    for esclusa in ["Annidata", "DiProva", "AncheDiProva"] {
        assert!(!strutture.contains(esclusa), "{esclusa}: {strutture:?}");
    }
    assert!(!contiene(&estratto, "Unione", 2, "altro"));
    // Una macro sconosciuta con un campo intero fa fallire la prova; una
    // senza campi interi no.
    let estratto = estrai(
        "nuova_macro!(struct Nuova { 1: required i64 conteggio }); \
         altra_macro!{ struct Testo { 1: optional binary nome } }",
    );
    assert_eq!(estratto.macro_sconosciute, ["nuova_macro"]);
    // Un `match` su interi che non legge valori Thrift non è un decoder.
    let estratto = estrai("fn larghezza(n: u8) -> u8 { match n { 1 => 8, 2 => 16, _ => 0 } }");
    assert!(estratto.decoder.is_empty() && estratto.non_mappati == 0);
}
