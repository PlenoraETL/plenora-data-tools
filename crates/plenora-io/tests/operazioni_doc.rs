//! Il riferimento delle operazioni: `docs/operazioni.md`, generato dalle
//! schede `docs/schede/<id>.md` e dal catalogo.
//!
//! Vive in `plenora-io` perché è il crate in cima al grafo: vede il runner, i
//! kernel tabellari e (fra le dev-dependency) quelli geografici, e un esempio
//! si esegue come lo eseguirebbe un piano vero.
//!
//! Tre prove:
//!
//! - **complete**: una scheda per ogni operazione del catalogo e nessuna in
//!   più; le sezioni sono tutte, nell'ordine fissato; i collegamenti alle
//!   guide di `docs/` e fra le schede puntano a titoli che esistono;
//! - **esempi**: l'esempio di ogni scheda gira dal runner come passo unico e
//!   l'uscita è quella scritta, colonna per colonna, tipo per tipo, cella per
//!   cella. `table.pivot` senza `mapping` e `table.transpose`, che il
//!   runner rifiuta perché il loro schema dipende dai dati, girano dal
//!   kernel. Un'operazione geo che il runner rifiutasse con `Unsupported` si
//!   verificherebbe sul contratto: config accettata dall'analisi, colonne e
//!   tipi d'uscita quelli scritti, valori non eseguiti, e il documento lo
//!   dichiarerebbe. Oggi il runner le esegue tutte e la prova confronta i
//!   valori;
//! - **aggiornato**: `docs/operazioni.md` è byte per byte quello che le
//!   schede e il catalogo generano. Si rigenera con
//!
//!   ```sh
//!   PLENORA_RIGENERA_DOC=1 cargo test -p plenora-io --test operazioni_doc
//!   ```
//!
//! La sezione «Memoria» di ogni operazione è un segnaposto fisso
//! (`Memoria: da misura v4.`), generato qui e non scritto nelle schede: lo
//! sostituirà il catalogo delle misure v4.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Nomi paralleli voluti (`scritta`/`scritte`, `riga`/`righe`, `tipo`/`tipi`):
// la coppia singolare/plurale è il modo più chiaro di dirli in italiano.
#![allow(clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{NaiveDate, NaiveDateTime};
use geo::Geometry;
use plenora_core::arrow::array::builder::{
    make_builder, ArrayBuilder, BinaryBuilder, BooleanBuilder, Date32Builder, Decimal128Builder,
    Float32Builder, Float64Builder, Int16Builder, Int32Builder, Int64Builder, Int8Builder,
    LargeStringBuilder, ListBuilder, StringBuilder, StringDictionaryBuilder, StructBuilder,
    TimestampMicrosecondBuilder, TimestampMillisecondBuilder, TimestampNanosecondBuilder,
    TimestampSecondBuilder, UInt16Builder, UInt32Builder, UInt64Builder, UInt8Builder,
};
use plenora_core::arrow::array::cast::AsArray;
use plenora_core::arrow::array::types::{
    Date32Type, Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, Int8Type,
    TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
    TimestampSecondType, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
};
use plenora_core::arrow::array::{Array, ArrayRef, RecordBatch};
use plenora_core::arrow::schema::{Fields, TimeUnit};
use plenora_core::arrow::{DataType, Field, Schema, SchemaRef};
use plenora_core::catalog::{
    find_operation, Arity, CancellationBehavior, CrsRequirement, DeterminismPolicy, ExecutionClass,
    ExpansionConstraint, Family, GeoFusion, Maturity, OperationDescriptor, Origin, ResultShape,
    SourceRowProvenance, ALIASES, CATALOG,
};
use plenora_core::contract::arrow_metadata::{
    geometry_output_field, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
};
use plenora_core::contract::arrow_schema::{
    arrow_schema_from_contract, contract_from_arrow_schema,
};
use plenora_core::contract::FieldAllocator;
use plenora_core::crs::resolve_crs;
use plenora_core::PlenoraError;
use plenora_pipeline::Pipeline;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

/// Nome della tabella d'uscita del passo di ogni esempio.
const USCITA: &str = "risultato";
/// Segnaposto della memoria misurata, finché non c'è il catalogo v4.
const SEGNAPOSTO_MEMORIA: &str = "Memoria: da misura v4.";
/// Variabile d'ambiente che rigenera il documento invece di confrontarlo.
const RIGENERA: &str = "PLENORA_RIGENERA_DOC";
/// Variabile d'ambiente con il file dell'anteprima delle schede presenti.
const ANTEPRIMA: &str = "PLENORA_ANTEPRIMA_DOC";
/// Variabile della chiave di `table.hmac_sha256` negli esempi: il runner ne
/// controlla l'esistenza in validazione.
const CHIAVE_HMAC: &str = "PLENORA_ESEMPIO_CHIAVE_HMAC";

/// Sezioni di ogni scheda, nell'ordine. `Precisione` solo per le geo.
const SEZIONI: [&str; 10] = [
    "Che cosa fa",
    "Parametri",
    "Schema",
    "Righe",
    "Ordine",
    "Errori",
    "Limiti e deviazioni",
    "Precisione",
    "Complessità",
    "Esempio",
];

fn radice() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cartella_schede() -> PathBuf {
    radice().join("docs").join("schede")
}

fn percorso_documento() -> PathBuf {
    radice().join("docs").join("operazioni.md")
}

/// Il testo con i fine riga normalizzati: il documento si confronta per
/// contenuto, non per la convenzione di fine riga del checkout.
fn leggi(percorso: &Path) -> String {
    std::fs::read_to_string(percorso)
        .unwrap_or_else(|errore| panic!("{}: {errore}", percorso.display()))
        .replace("\r\n", "\n")
}

// ---------------------------------------------------------------------------
// Le schede
// ---------------------------------------------------------------------------

/// Una scheda letta: le sezioni nell'ordine di [`SEZIONI`] e l'esempio.
struct Scheda {
    operazione: &'static OperationDescriptor,
    /// `(titolo, corpo)`, senza `Esempio`.
    sezioni: Vec<(&'static str, String)>,
    /// Testo dell'esempio prima del blocco JSON (può essere vuoto).
    premessa_esempio: String,
    esempio: Esempio,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Esempio {
    /// La config del passo, testo originale: il documento la riporta come è
    /// scritta, senza riordinare le chiavi.
    config: Box<RawValue>,
    ingressi: Vec<TabellaSpec>,
    uscita: UscitaSpec,
    /// CRS di piano, per i produttori geo.
    #[serde(default)]
    crs: Option<String>,
    /// `limits` del piano.
    #[serde(default)]
    limits: Option<Box<RawValue>>,
    /// `false` solo per le operazioni di [`USCITA_CASUALE`] (e per loro è
    /// obbligatorio): si confrontano schema e righe, non i valori.
    #[serde(default = "vero")]
    valori_confrontati: bool,
}

const fn vero() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TabellaSpec {
    nome: String,
    colonne: Vec<ColonnaSpec>,
    /// Metadati di schema dell'ingresso (`table.assert_metadata`).
    #[serde(default)]
    metadati: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UscitaSpec {
    colonne: Vec<ColonnaSpec>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColonnaSpec {
    nome: String,
    tipo: String,
    /// Testo originale di ogni valore: un `float64` si legge dal suo testo
    /// con `str::parse`, esatto, perché `serde_json` senza `float_roundtrip`
    /// può sbagliare l'ultima cifra.
    valori: Vec<Box<RawValue>>,
    /// CRS della colonna `geometry`; assente vale `EPSG:4326`.
    #[serde(default)]
    crs: Option<String>,
}

fn leggi_scheda(operazione: &'static OperationDescriptor) -> Result<Scheda, String> {
    let percorso = cartella_schede().join(format!("{}.md", operazione.id));
    if !percorso.exists() {
        return Err("scheda assente".into());
    }
    let testo = leggi(&percorso);
    let geo = operazione.family == Family::Geo;
    let attese: Vec<&'static str> = SEZIONI
        .iter()
        .copied()
        .filter(|sezione| geo || *sezione != "Precisione")
        .collect();
    let mut trovate: Vec<(String, String)> = Vec::new();
    for riga in testo.lines() {
        if let Some(titolo) = riga.strip_prefix("### ") {
            trovate.push((titolo.trim().to_owned(), String::new()));
        } else if riga.starts_with("# ") || riga.starts_with("## ") {
            return Err(format!("titolo di livello non ammesso: `{riga}`"));
        } else if let Some((_, corpo)) = trovate.last_mut() {
            corpo.push_str(riga);
            corpo.push('\n');
        } else if !riga.trim().is_empty() && !riga.starts_with("<!--") {
            return Err(format!("testo prima della prima sezione: `{riga}`"));
        }
    }
    let titoli: Vec<&str> = trovate.iter().map(|(titolo, _)| titolo.as_str()).collect();
    if titoli != attese {
        return Err(format!("sezioni {titoli:?}, attese {attese:?}"));
    }
    let mut sezioni = Vec::new();
    let mut blocco_esempio = None;
    for ((_, corpo), titolo) in trovate.into_iter().zip(attese) {
        let corpo = corpo.trim().to_owned();
        if corpo.is_empty() {
            return Err(format!("sezione `{titolo}` vuota"));
        }
        for vietato in ["TODO", "FIXME", "XXX", "da misura v4"] {
            if corpo.contains(vietato) {
                return Err(format!("sezione `{titolo}`: contiene `{vietato}`"));
            }
        }
        if titolo == "Esempio" {
            blocco_esempio = Some(corpo);
        } else {
            sezioni.push((titolo, corpo));
        }
    }
    let blocco = blocco_esempio.ok_or("sezione Esempio assente")?;
    let (premessa, resto) = blocco
        .split_once("```json\n")
        .ok_or("l'esempio non ha un blocco ```json")?;
    let (json, dopo) = resto
        .split_once("\n```")
        .ok_or("blocco ```json dell'esempio non chiuso")?;
    if !dopo.trim().is_empty() {
        return Err("testo dopo il blocco JSON dell'esempio".into());
    }
    plenora_core::json::ensure_no_duplicate_keys(json).map_err(|e| e.to_string())?;
    let esempio: Esempio = serde_json::from_str(json).map_err(|errore| {
        // Posizione e categoria, non il messaggio di serde, che può
        // citare il valore letto.
        format!(
            "esempio non leggibile (riga {}, colonna {}, {:?})",
            errore.line(),
            errore.column(),
            errore.classify()
        )
    })?;
    Ok(Scheda {
        operazione,
        sezioni,
        premessa_esempio: premessa.trim().to_owned(),
        esempio,
    })
}

/// Le operazioni del catalogo nell'ordine del documento: tabellari poi geo,
/// ciascuna famiglia per id.
fn operazioni_ordinate() -> Vec<&'static OperationDescriptor> {
    let mut operazioni: Vec<&'static OperationDescriptor> = CATALOG.iter().collect();
    operazioni.sort_by_key(|operazione| (operazione.family == Family::Geo, operazione.id));
    operazioni
}

fn leggi_schede() -> (Vec<Scheda>, Vec<String>) {
    let mut schede = Vec::new();
    let mut errori = Vec::new();
    for operazione in operazioni_ordinate() {
        match leggi_scheda(operazione) {
            Ok(scheda) => schede.push(scheda),
            Err(errore) => errori.push(format!("{}: {errore}", operazione.id)),
        }
    }
    let note: BTreeSet<String> = CATALOG
        .iter()
        .map(|operazione| format!("{}.md", operazione.id))
        .collect();
    // Una cartella illeggibile è un errore, non un controllo saltato; i nomi
    // si ordinano perché la diagnostica non dipenda dall'ordine del file
    // system.
    let cartella = cartella_schede();
    let voci = std::fs::read_dir(&cartella)
        .unwrap_or_else(|errore| panic!("{}: {errore}", cartella.display()));
    let mut nomi: Vec<String> = voci
        .map(|voce| {
            voce.unwrap_or_else(|errore| panic!("{}: {errore}", cartella.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    nomi.sort();
    for nome in nomi {
        if !note.contains(&nome) {
            errori.push(format!("{nome}: scheda senza operazione nel catalogo"));
        }
    }
    (schede, errori)
}

// ---------------------------------------------------------------------------
// Tipi e valori degli esempi
// ---------------------------------------------------------------------------

/// Tipo di una colonna d'esempio: `geometry` è `Binary` GeoArrow-WKB.
fn tipo_arrow(tipo: &str) -> Result<(DataType, bool), String> {
    if tipo == "geometry" {
        return Ok((DataType::Binary, true));
    }
    let mut lettore = LettoreTipo {
        testo: tipo,
        posizione: 0,
    };
    let tipo_letto = lettore.tipo()?;
    lettore.spazi();
    if lettore.posizione != tipo.len() {
        return Err(format!("tipo `{tipo}`: testo in coda"));
    }
    Ok((tipo_letto, false))
}

struct LettoreTipo<'a> {
    testo: &'a str,
    posizione: usize,
}

impl LettoreTipo<'_> {
    fn spazi(&mut self) {
        while self.testo[self.posizione..].starts_with(' ') {
            self.posizione += 1;
        }
    }

    fn parola(&mut self) -> &str {
        self.spazi();
        let inizio = self.posizione;
        while let Some(carattere) = self.testo[self.posizione..].chars().next() {
            if carattere.is_ascii_alphanumeric() || carattere == '_' {
                self.posizione += carattere.len_utf8();
            } else {
                break;
            }
        }
        &self.testo[inizio..self.posizione]
    }

    fn atteso(&mut self, simbolo: char) -> Result<(), String> {
        self.spazi();
        if self.testo[self.posizione..].starts_with(simbolo) {
            self.posizione += simbolo.len_utf8();
            Ok(())
        } else {
            Err(format!("tipo `{}`: atteso `{simbolo}`", self.testo))
        }
    }

    fn prossimo(&mut self, simbolo: char) -> bool {
        self.spazi();
        if self.testo[self.posizione..].starts_with(simbolo) {
            self.posizione += simbolo.len_utf8();
            true
        } else {
            false
        }
    }

    fn intero(&mut self) -> Result<i64, String> {
        let negativo = self.prossimo('-');
        let cifre = self.parola().to_owned();
        let valore: i64 = cifre
            .parse()
            .map_err(|_| format!("tipo `{}`: intero atteso", self.testo))?;
        Ok(if negativo { -valore } else { valore })
    }

    fn tipo(&mut self) -> Result<DataType, String> {
        let nome = self.parola().to_owned();
        let tipo = match nome.as_str() {
            "int8" => DataType::Int8,
            "int16" => DataType::Int16,
            "int32" => DataType::Int32,
            "int64" => DataType::Int64,
            "uint8" => DataType::UInt8,
            "uint16" => DataType::UInt16,
            "uint32" => DataType::UInt32,
            "uint64" => DataType::UInt64,
            "float32" => DataType::Float32,
            "float64" => DataType::Float64,
            "utf8" => DataType::Utf8,
            "large_utf8" => DataType::LargeUtf8,
            "bool" => DataType::Boolean,
            "binary" => DataType::Binary,
            "date32" => DataType::Date32,
            "decimal128" => {
                self.atteso('(')?;
                let precisione = u8::try_from(self.intero()?).map_err(|e| e.to_string())?;
                self.atteso(',')?;
                let scala = i8::try_from(self.intero()?).map_err(|e| e.to_string())?;
                self.atteso(')')?;
                DataType::Decimal128(precisione, scala)
            }
            "timestamp" => {
                self.atteso('(')?;
                let unita = match self.parola() {
                    "s" => TimeUnit::Second,
                    "ms" => TimeUnit::Millisecond,
                    "us" => TimeUnit::Microsecond,
                    "ns" => TimeUnit::Nanosecond,
                    altra => return Err(format!("unità di timestamp `{altra}`")),
                };
                let fuso = if self.prossimo(',') {
                    // Il fuso va fino alla parentesi: `find` rende un indice
                    // di byte su un confine di carattere, e un fuso non ASCII
                    // (che nessun nome IANA è) si rifiuta invece di finire
                    // in Arrow.
                    let resto = &self.testo[self.posizione..];
                    let fine = resto
                        .find(')')
                        .ok_or_else(|| format!("tipo `{}`: atteso `)`", self.testo))?;
                    let fuso = resto[..fine].trim();
                    if fuso.is_empty() || !fuso.is_ascii() {
                        return Err(format!("tipo `{}`: fuso vuoto o non ASCII", self.testo));
                    }
                    self.posizione += fine;
                    Some(Arc::from(fuso))
                } else {
                    None
                };
                self.atteso(')')?;
                DataType::Timestamp(unita, fuso)
            }
            "list" => {
                self.atteso('<')?;
                let figlio = self.tipo()?;
                self.atteso('>')?;
                DataType::List(Arc::new(Field::new("item", figlio, true)))
            }
            "dictionary" => {
                self.atteso('<')?;
                let valore = self.tipo()?;
                self.atteso('>')?;
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(valore))
            }
            "struct" => {
                self.atteso('<')?;
                let mut campi = Vec::new();
                loop {
                    let nome = self.parola().to_owned();
                    self.atteso(':')?;
                    let figlio = self.tipo()?;
                    campi.push(Field::new(nome, figlio, true));
                    if !self.prossimo(',') {
                        break;
                    }
                }
                self.atteso('>')?;
                DataType::Struct(Fields::from(campi))
            }
            altro => return Err(format!("tipo `{altro}` non previsto dagli esempi")),
        };
        Ok(tipo)
    }
}

/// Il nome con cui un tipo compare nell'intestazione delle tabelle, nella
/// stessa grammatica di [`tipo_arrow`].
fn nome_tipo(tipo: &DataType) -> String {
    match tipo {
        DataType::Int8 => "int8".into(),
        DataType::Int16 => "int16".into(),
        DataType::Int32 => "int32".into(),
        DataType::Int64 => "int64".into(),
        DataType::UInt8 => "uint8".into(),
        DataType::UInt16 => "uint16".into(),
        DataType::UInt32 => "uint32".into(),
        DataType::UInt64 => "uint64".into(),
        DataType::Float32 => "float32".into(),
        DataType::Float64 => "float64".into(),
        DataType::Utf8 => "utf8".into(),
        DataType::LargeUtf8 => "large_utf8".into(),
        DataType::Boolean => "bool".into(),
        DataType::Binary => "binary".into(),
        DataType::Date32 => "date32".into(),
        DataType::Decimal128(precisione, scala) => format!("decimal128({precisione}, {scala})"),
        DataType::Timestamp(unita, fuso) => {
            let unita = match unita {
                TimeUnit::Second => "s",
                TimeUnit::Millisecond => "ms",
                TimeUnit::Microsecond => "us",
                TimeUnit::Nanosecond => "ns",
            };
            fuso.as_ref().map_or_else(
                || format!("timestamp({unita})"),
                |fuso| format!("timestamp({unita}, {fuso})"),
            )
        }
        DataType::List(figlio) => format!("list<{}>", nome_tipo(figlio.data_type())),
        DataType::Dictionary(_, valore) => format!("dictionary<{}>", nome_tipo(valore)),
        DataType::Struct(campi) => {
            let campi: Vec<String> = campi
                .iter()
                .map(|campo| format!("{}: {}", campo.name(), nome_tipo(campo.data_type())))
                .collect();
            format!("struct<{}>", campi.join(", "))
        }
        altro => format!("{altro:?}"),
    }
}

fn e_geometria(campo: &Field) -> bool {
    campo
        .metadata()
        .get(GEOARROW_EXTENSION_KEY)
        .is_some_and(|estensione| estensione == GEOARROW_WKB_EXTENSION)
}

fn istante(testo: &str) -> Result<NaiveDateTime, String> {
    let testo = testo.replace('T', " ");
    for formato in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M"] {
        if let Ok(valore) = NaiveDateTime::parse_from_str(&testo, formato) {
            return Ok(valore);
        }
    }
    Err("istante non leggibile (AAAA-MM-GG HH:MM:SS[.f])".to_owned())
}

fn decimale(testo: &str, scala: i8) -> Result<i128, String> {
    let (negativo, testo) = testo
        .strip_prefix('-')
        .map_or((false, testo), |resto| (true, resto));
    let (intera, frazione) = testo.split_once('.').unwrap_or((testo, ""));
    let scala = usize::try_from(scala).map_err(|_| "scala negativa".to_owned())?;
    if frazione.len() > scala {
        return Err("decimale con più cifre della scala".to_owned());
    }
    let cifre = format!("{intera}{frazione:0<scala$}");
    let valore: i128 = cifre
        .parse()
        .map_err(|_| "decimale non leggibile".to_owned())?;
    Ok(if negativo { -valore } else { valore })
}

fn scarica<T: 'static>(builder: &mut dyn ArrayBuilder) -> &mut T {
    builder
        .as_any_mut()
        .downcast_mut::<T>()
        .expect("builder del tipo dichiarato")
}

fn numero<T: TryFrom<i64> + TryFrom<u64>>(valore: &Value) -> Result<T, String> {
    if let Some(intero) = valore.as_i64() {
        return T::try_from(intero).map_err(|_| "intero fuori dal tipo".to_owned());
    }
    if let Some(intero) = valore.as_u64() {
        return T::try_from(intero).map_err(|_| "intero fuori dal tipo".to_owned());
    }
    Err("intero atteso".to_owned())
}

fn virgola_mobile(valore: &Value) -> Result<f64, String> {
    match valore {
        Value::Number(numero) => numero
            .as_f64()
            .ok_or_else(|| "numero non rappresentabile".to_owned()),
        Value::String(testo) => match testo.as_str() {
            "NaN" => Ok(f64::NAN),
            "inf" => Ok(f64::INFINITY),
            "-inf" => Ok(f64::NEG_INFINITY),
            _ => Err("numero atteso".to_owned()),
        },
        _ => Err("numero atteso".to_owned()),
    }
}

fn testo_di(valore: &Value) -> Result<&str, String> {
    valore.as_str().ok_or_else(|| "testo atteso".to_owned())
}

/// Aggiunge un valore JSON al builder del tipo dato; `null` è sempre null.
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)] // Un braccio per tipo.
fn aggiungi(builder: &mut dyn ArrayBuilder, tipo: &DataType, valore: &Value) -> Result<(), String> {
    macro_rules! primitivo {
        ($builder:ty, $conversione:expr) => {{
            let builder = scarica::<$builder>(builder);
            if valore.is_null() {
                builder.append_null();
            } else {
                builder.append_value($conversione?);
            }
        }};
    }
    match tipo {
        DataType::Int8 => primitivo!(Int8Builder, numero::<i8>(valore)),
        DataType::Int16 => primitivo!(Int16Builder, numero::<i16>(valore)),
        DataType::Int32 => primitivo!(Int32Builder, numero::<i32>(valore)),
        DataType::Int64 => primitivo!(Int64Builder, numero::<i64>(valore)),
        DataType::UInt8 => primitivo!(UInt8Builder, numero::<u8>(valore)),
        DataType::UInt16 => primitivo!(UInt16Builder, numero::<u16>(valore)),
        DataType::UInt32 => primitivo!(UInt32Builder, numero::<u32>(valore)),
        DataType::UInt64 => primitivo!(UInt64Builder, numero::<u64>(valore)),
        DataType::Float32 => {
            primitivo!(Float32Builder, virgola_mobile(valore).map(|v| v as f32));
        }
        DataType::Float64 => primitivo!(Float64Builder, virgola_mobile(valore)),
        DataType::Boolean => primitivo!(
            BooleanBuilder,
            valore.as_bool().ok_or_else(|| "booleano atteso".to_owned())
        ),
        DataType::Utf8 => primitivo!(StringBuilder, testo_di(valore)),
        DataType::LargeUtf8 => primitivo!(LargeStringBuilder, testo_di(valore)),
        DataType::Binary => primitivo!(
            BinaryBuilder,
            testo_di(valore).and_then(|esadecimale| {
                plenora_kernels_geo::wkb_hex_to_bytes(esadecimale)
                    .ok_or_else(|| "esadecimale atteso".to_owned())
            })
        ),
        DataType::Date32 => primitivo!(
            Date32Builder,
            testo_di(valore).and_then(|testo| {
                let data = NaiveDate::parse_from_str(testo, "%Y-%m-%d")
                    .map_err(|_| "data non leggibile (AAAA-MM-GG)".to_owned())?;
                let epoca = NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoca");
                i32::try_from((data - epoca).num_days()).map_err(|e| e.to_string())
            })
        ),
        DataType::Timestamp(unita, _) => {
            let valore_epoca = || -> Result<i64, String> {
                let istante = istante(testo_di(valore)?)?.and_utc();
                Ok(match unita {
                    TimeUnit::Second => istante.timestamp(),
                    TimeUnit::Millisecond => istante.timestamp_millis(),
                    TimeUnit::Microsecond => istante.timestamp_micros(),
                    TimeUnit::Nanosecond => istante
                        .timestamp_nanos_opt()
                        .ok_or("istante fuori dai nanosecondi")?,
                })
            };
            match unita {
                TimeUnit::Second => primitivo!(TimestampSecondBuilder, valore_epoca()),
                TimeUnit::Millisecond => primitivo!(TimestampMillisecondBuilder, valore_epoca()),
                TimeUnit::Microsecond => primitivo!(TimestampMicrosecondBuilder, valore_epoca()),
                TimeUnit::Nanosecond => primitivo!(TimestampNanosecondBuilder, valore_epoca()),
            }
        }
        DataType::Decimal128(_, scala) => primitivo!(
            Decimal128Builder,
            testo_di(valore).and_then(|testo| decimale(testo, *scala))
        ),
        DataType::Dictionary(_, valore_tipo) if **valore_tipo == DataType::Utf8 => {
            primitivo!(StringDictionaryBuilder<Int32Type>, testo_di(valore));
        }
        DataType::List(figlio) => {
            let builder = scarica::<ListBuilder<Box<dyn ArrayBuilder>>>(builder);
            if valore.is_null() {
                builder.append_null();
            } else {
                let elementi = valore.as_array().ok_or_else(|| "lista attesa".to_owned())?;
                for elemento in elementi {
                    aggiungi(builder.values().as_mut(), figlio.data_type(), elemento)?;
                }
                builder.append(true);
            }
        }
        DataType::Struct(campi) => {
            let builder = scarica::<StructBuilder>(builder);
            let oggetto = valore.as_object();
            if !valore.is_null() && oggetto.is_none() {
                return Err("oggetto atteso".to_owned());
            }
            for (indice, campo) in campi.iter().enumerate() {
                let figlio = oggetto
                    .and_then(|oggetto| oggetto.get(campo.name()))
                    .unwrap_or(&Value::Null);
                aggiungi(
                    builder.field_builders_mut()[indice].as_mut(),
                    campo.data_type(),
                    figlio,
                )?;
            }
            builder.append(!valore.is_null());
        }
        altro => return Err(format!("tipo {altro} non costruibile dagli esempi")),
    }
    Ok(())
}

/// Un valore d'esempio come `float64`, letto dal testo: `null`, un numero
/// JSON o le stringhe `"NaN"`, `"inf"`, `"-inf"`.
fn float_dal_testo(testo: &str) -> Result<Option<f64>, String> {
    match testo.trim() {
        "null" => Ok(None),
        "\"NaN\"" => Ok(Some(f64::NAN)),
        "\"inf\"" => Ok(Some(f64::INFINITY)),
        "\"-inf\"" => Ok(Some(f64::NEG_INFINITY)),
        numero => numero
            .parse()
            .map(Some)
            .map_err(|_| "numero atteso".to_owned()),
    }
}

/// Le diagnostiche degli esempi nominano colonna e riga, mai il valore
/// (AGENTS.md, «Errori senza dati», vale anche per i test).
fn colonna(spec: &ColonnaSpec) -> Result<(Field, ArrayRef), String> {
    colonna_senza_contesto(spec).map_err(|errore| format!("colonna `{}`: {errore}", spec.nome))
}

fn in_riga<T>(riga: usize, esito: Result<T, String>) -> Result<T, String> {
    esito.map_err(|errore| format!("riga {riga}: {errore}"))
}

fn colonna_senza_contesto(spec: &ColonnaSpec) -> Result<(Field, ArrayRef), String> {
    let (tipo, geometria) = tipo_arrow(&spec.tipo)?;
    if tipo == DataType::Float64 && !geometria {
        let valori = spec
            .valori
            .iter()
            .enumerate()
            .map(|(riga, valore)| in_riga(riga, float_dal_testo(valore.get())))
            .collect::<Result<Vec<_>, _>>()?;
        let array = plenora_core::arrow::array::Float64Array::from(valori);
        return Ok((Field::new(&spec.nome, tipo, true), Arc::new(array)));
    }
    let valori = spec
        .valori
        .iter()
        .enumerate()
        .map(|(riga, valore)| {
            plenora_core::json::ensure_no_duplicate_keys(valore.get())
                .map_err(|e| e.to_string())?;
            in_riga(
                riga,
                serde_json::from_str::<Value>(valore.get())
                    .map_err(|_| "JSON non leggibile".to_owned()),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if geometria {
        let crs = spec.crs.as_deref().unwrap_or("EPSG:4326");
        let campo = geometry_output_field(&spec.nome, crs).map_err(|e| e.to_string())?;
        // Le schede dichiarano WKT nella convenzione GIS del CRS.
        let mut metadata = campo.metadata().clone();
        let resolved = plenora_core::crs::resolve_crs(crs, "crs").map_err(|e| e.to_string())?;
        metadata.insert(
            "plenora.geometry.axis_order".to_owned(),
            resolved.normalized_gis_axis_order().as_str().to_owned(),
        );
        let campo = campo.with_metadata(metadata);
        let mut builder = BinaryBuilder::new();
        for (riga, valore) in valori.iter().enumerate() {
            if valore.is_null() {
                builder.append_null();
                continue;
            }
            let testo = in_riga(riga, testo_di(valore))?;
            let geometria: Geometry<f64> = in_riga(
                riga,
                wkt::TryFromWkt::try_from_wkt_str(testo)
                    .map_err(|_| "WKT non leggibile".to_owned()),
            )?;
            let wkb = in_riga(
                riga,
                plenora_kernels_geo::arrow_adapter::encode_geometry(&geometria)
                    .map_err(|e| e.to_string()),
            )?;
            builder.append_value(wkb);
        }
        return Ok((campo, Arc::new(builder.finish())));
    }
    if spec.crs.is_some() {
        return Err(format!(
            "colonna `{}`: `crs` solo per `geometry`",
            spec.nome
        ));
    }
    let mut builder = make_builder(&tipo, valori.len());
    for (riga, valore) in valori.iter().enumerate() {
        in_riga(riga, aggiungi(builder.as_mut(), &tipo, valore))?;
    }
    let array = builder.finish();
    Ok((Field::new(&spec.nome, tipo, true), array))
}

fn tabella(colonne: &[ColonnaSpec]) -> Result<RecordBatch, String> {
    tabella_con_metadati(colonne, &BTreeMap::new())
}

fn tabella_con_metadati(
    colonne: &[ColonnaSpec],
    metadati: &BTreeMap<String, String>,
) -> Result<RecordBatch, String> {
    let mut campi = Vec::new();
    let mut array = Vec::new();
    for spec in colonne {
        let (campo, valori) = colonna(spec)?;
        campi.push(campo);
        array.push(valori);
    }
    let righe = colonne.first().map_or(0, |spec| spec.valori.len());
    let metadati = metadati
        .iter()
        .map(|(chiave, valore)| (chiave.clone(), valore.clone()))
        .collect::<plenora_core::arrow::Metadata>();
    let mut metadati = metadati;
    if campi
        .iter()
        .any(|campo| campo.metadata().contains_key("plenora.geometry.axis_order"))
    {
        metadati.insert("plenora.contract.version".to_owned(), "1".to_owned());
    }
    let schema = Schema::new_with_metadata(campi, metadati);
    plenora_core::batch_with_rows(Arc::new(schema), array, righe).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Resa delle celle
// ---------------------------------------------------------------------------

/// Una cella come testo: la stessa resa per l'uscita del kernel e per
/// quella scritta nella scheda, così il confronto è sul testo che il lettore
/// vede.
#[allow(clippy::too_many_lines)] // Un braccio per tipo.
fn cella(array: &dyn Array, riga: usize, geometria: bool, annidata: bool) -> String {
    if array.is_null(riga) {
        return "null".into();
    }
    // Dentro liste e struct il testo è sempre fra virgolette; fuori, solo
    // dove si confonderebbe con il nullo o con la stringa vuota.
    let testo = |valore: &str| {
        if annidata || valore.is_empty() || valore == "null" || valore.starts_with('"') {
            serde_json::to_string(valore).expect("stringa")
        } else {
            valore.to_owned()
        }
    };
    match array.data_type() {
        DataType::Int8 => array.as_primitive::<Int8Type>().value(riga).to_string(),
        DataType::Int16 => array.as_primitive::<Int16Type>().value(riga).to_string(),
        DataType::Int32 => array.as_primitive::<Int32Type>().value(riga).to_string(),
        DataType::Int64 => array.as_primitive::<Int64Type>().value(riga).to_string(),
        DataType::UInt8 => array.as_primitive::<UInt8Type>().value(riga).to_string(),
        DataType::UInt16 => array.as_primitive::<UInt16Type>().value(riga).to_string(),
        DataType::UInt32 => array.as_primitive::<UInt32Type>().value(riga).to_string(),
        DataType::UInt64 => array.as_primitive::<UInt64Type>().value(riga).to_string(),
        DataType::Float32 => format!("{:?}", array.as_primitive::<Float32Type>().value(riga)),
        DataType::Float64 => format!("{:?}", array.as_primitive::<Float64Type>().value(riga)),
        DataType::Boolean => array.as_boolean().value(riga).to_string(),
        DataType::Utf8 => testo(array.as_string::<i32>().value(riga)),
        DataType::LargeUtf8 => testo(array.as_string::<i64>().value(riga)),
        DataType::Binary => {
            let byte = array.as_binary::<i32>().value(riga);
            if geometria {
                plenora_kernels_geo::wkb_decoder::decode_validated(byte).map_or_else(
                    |_| "WKB non decodificabile".into(),
                    |geometria| wkt::ToWkt::to_wkt(&geometria).to_string(),
                )
            } else {
                byte.iter().fold(String::new(), |mut uscita, b| {
                    let _ = write!(uscita, "{b:02x}");
                    uscita
                })
            }
        }
        DataType::Date32 => {
            let giorni = array.as_primitive::<Date32Type>().value(riga);
            (NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoca")
                + chrono::Duration::days(i64::from(giorni)))
            .to_string()
        }
        DataType::Timestamp(unita, _) => {
            let istante = match unita {
                TimeUnit::Second => chrono::DateTime::from_timestamp(
                    array.as_primitive::<TimestampSecondType>().value(riga),
                    0,
                ),
                TimeUnit::Millisecond => chrono::DateTime::from_timestamp_millis(
                    array.as_primitive::<TimestampMillisecondType>().value(riga),
                ),
                TimeUnit::Microsecond => chrono::DateTime::from_timestamp_micros(
                    array.as_primitive::<TimestampMicrosecondType>().value(riga),
                ),
                TimeUnit::Nanosecond => Some(chrono::DateTime::from_timestamp_nanos(
                    array.as_primitive::<TimestampNanosecondType>().value(riga),
                )),
            };
            istante.map_or_else(
                || "fuori intervallo".into(),
                |istante| istante.naive_utc().to_string(),
            )
        }
        DataType::Decimal128(_, scala) => {
            let valore = array
                .as_primitive::<plenora_core::arrow::array::types::Decimal128Type>()
                .value(riga);
            let scala = usize::try_from(*scala).unwrap_or(0);
            let cifre = valore.unsigned_abs().to_string();
            let cifre = format!("{cifre:0>width$}", width = scala + 1);
            let (intera, frazione) = cifre.split_at(cifre.len() - scala);
            let segno = if valore < 0 { "-" } else { "" };
            if scala == 0 {
                format!("{segno}{intera}")
            } else {
                format!("{segno}{intera}.{frazione}")
            }
        }
        DataType::Dictionary(_, _) => {
            let dizionario = array.as_any_dictionary();
            let chiave = dizionario.normalized_keys()[riga];
            cella(dizionario.values().as_ref(), chiave, false, annidata)
        }
        DataType::List(_) => {
            let lista = array.as_list::<i32>().value(riga);
            let elementi: Vec<String> = (0..lista.len())
                .map(|indice| cella(lista.as_ref(), indice, false, true))
                .collect();
            format!("[{}]", elementi.join(", "))
        }
        DataType::Struct(campi) => {
            let struttura = array.as_struct();
            let elementi: Vec<String> = campi
                .iter()
                .enumerate()
                .map(|(indice, campo)| {
                    format!(
                        "{}: {}",
                        campo.name(),
                        cella(struttura.column(indice).as_ref(), riga, false, true)
                    )
                })
                .collect();
            format!("{{{}}}", elementi.join(", "))
        }
        altro => format!("<{altro}>"),
    }
}

struct Resa {
    intestazioni: Vec<String>,
    tipi: Vec<DataType>,
    /// Le celle come il documento le mostra.
    righe: Vec<Vec<String>>,
    /// Le celle per il confronto: `None` il nullo, il testo delle stringhe
    /// di primo livello senza virgolette, il resto come nella resa. Il tipo
    /// si confronta a parte ([`Resa::tipi`]), quindi `"1"` in `utf8` e `1` in
    /// `int64` non si confondono.
    valori: Vec<Vec<Option<String>>>,
}

/// Il valore di una cella per il confronto: nullità e valore separati.
fn valore_cella(array: &dyn Array, riga: usize, geometria: bool) -> Option<String> {
    if nullo_logico(array, riga) {
        return None;
    }
    Some(match array.data_type() {
        DataType::Utf8 => array.as_string::<i32>().value(riga).to_owned(),
        DataType::LargeUtf8 => array.as_string::<i64>().value(riga).to_owned(),
        DataType::Dictionary(_, _) => {
            let dizionario = array.as_any_dictionary();
            let chiave = dizionario.normalized_keys()[riga];
            return valore_cella(dizionario.values().as_ref(), chiave, false);
        }
        _ => cella(array, riga, geometria, false),
    })
}

/// Nullità logica: per un dizionario anche la voce nulla puntata da una
/// chiave valida.
fn nullo_logico(array: &dyn Array, riga: usize) -> bool {
    array
        .logical_nulls()
        .is_some_and(|nulli| nulli.is_null(riga))
}

fn rendi(batch: &RecordBatch) -> Resa {
    let schema = batch.schema();
    let geometrie: Vec<bool> = schema.fields().iter().map(|c| e_geometria(c)).collect();
    let intestazioni = schema
        .fields()
        .iter()
        .zip(&geometrie)
        .map(|(campo, geometria)| {
            let tipo = if *geometria {
                "geometry".to_owned()
            } else {
                nome_tipo(campo.data_type())
            };
            format!("{}: {tipo}", campo.name())
        })
        .collect();
    let righe = (0..batch.num_rows())
        .map(|riga| {
            batch
                .columns()
                .iter()
                .zip(&geometrie)
                .map(|(colonna, geometria)| cella(colonna.as_ref(), riga, *geometria, false))
                .collect()
        })
        .collect();
    let valori = (0..batch.num_rows())
        .map(|riga| {
            batch
                .columns()
                .iter()
                .zip(&geometrie)
                .map(|(colonna, geometria)| valore_cella(colonna.as_ref(), riga, *geometria))
                .collect()
        })
        .collect();
    Resa {
        intestazioni,
        tipi: schema
            .fields()
            .iter()
            .map(|c| c.data_type().clone())
            .collect(),
        righe,
        valori,
    }
}

fn tabella_markdown(resa: &Resa) -> String {
    let escape = |testo: &str| testo.replace('|', "\\|");
    let mut uscita = String::new();
    let intestazioni: Vec<String> = resa
        .intestazioni
        .iter()
        .map(|testo| format!("`{}`", escape(testo)))
        .collect();
    if intestazioni.is_empty() {
        let _ = writeln!(uscita, "(nessuna colonna, {} righe)", resa.righe.len());
        return uscita;
    }
    let _ = writeln!(uscita, "| {} |", intestazioni.join(" | "));
    let _ = writeln!(uscita, "|{}", " --- |".repeat(intestazioni.len()));
    for riga in &resa.righe {
        let celle: Vec<String> = riga.iter().map(|c| escape(c)).collect();
        let _ = writeln!(uscita, "| {} |", celle.join(" | "));
    }
    if resa.righe.is_empty() {
        uscita.push_str("\n(nessuna riga)\n");
    }
    uscita
}

// ---------------------------------------------------------------------------
// Esecuzione degli esempi
// ---------------------------------------------------------------------------

/// Come è stato verificato un esempio.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verifica {
    /// Il runner l'ha eseguito e l'uscita è quella scritta.
    Runner,
    /// Il kernel l'ha eseguito (il runner rifiuta l'operazione in
    /// validazione: schema d'uscita dipendente dai dati).
    Kernel,
    /// Il runner l'ha eseguito; schema e righe confrontati, valori no
    /// (uscita non deterministica per contratto).
    RunnerSenzaValori,
    /// Il runner non esegue l'operazione geo: l'analisi accetta la config
    /// e dichiara colonne e tipi scritti; i valori non sono eseguiti.
    Contratto,
}

impl Verifica {
    const fn testo(self) -> &'static str {
        match self {
            Self::Runner => "eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.",
            Self::Kernel => "eseguito dal kernel (il runner rifiuta questa config in validazione, perché lo schema d'uscita dipende dai dati); l'uscita è confrontata cella per cella.",
            Self::RunnerSenzaValori => "eseguito dal runner come passo unico; schema e numero di righe confrontati, valori no (sono casuali per contratto).",
            Self::Contratto => "**contratto verificato, valori non eseguiti**: il runner non esegue l'operazione; l'analisi accetta la config e dichiara le colonne e i tipi dell'uscita, i valori sono scritti a mano.",
        }
    }
}

fn testo_piano(scheda: &Scheda) -> String {
    let esempio = &scheda.esempio;
    let nomi: Vec<String> = esempio
        .ingressi
        .iter()
        .map(|t| serde_json::to_string(&t.nome).expect("nome"))
        .collect();
    let mut piano = format!("{{\"version\": 1, \"inputs\": [{}]", nomi.join(", "));
    if let Some(crs) = &esempio.crs {
        let _ = write!(
            piano,
            ", \"crs\": {}",
            serde_json::to_string(crs).expect("crs")
        );
    }
    if let Some(limits) = &esempio.limits {
        let _ = write!(piano, ", \"limits\": {}", limits.get());
    }
    let _ = write!(
        piano,
        ", \"steps\": [{{\"out\": \"{USCITA}\", \"op\": \"{}\", \"in\": [{}], \"config\": {}}}], \"outputs\": [\"{USCITA}\"]}}",
        scheda.operazione.id,
        nomi.join(", "),
        esempio.config.get()
    );
    piano
}

fn confronta(scheda: &Scheda, uscita: &RecordBatch, valori: bool) -> Result<(), String> {
    let attesa = tabella(&scheda.esempio.uscita.colonne)?;
    let reale = rendi(uscita);
    let scritta = rendi(&attesa);
    if reale.intestazioni != scritta.intestazioni || reale.tipi != scritta.tipi {
        return Err(format!(
            "colonne d'uscita {:?}, scritte {:?}",
            reale.intestazioni, scritta.intestazioni
        ));
    }
    if reale.valori.len() != scritta.valori.len() {
        return Err(format!(
            "{} righe d'uscita, scritte {}",
            reale.valori.len(),
            scritta.valori.len()
        ));
    }
    if valori {
        // Solo le posizioni: i valori delle celle non entrano nei messaggi.
        let diverse = celle_diverse(&reale, &scritta);
        if !diverse.is_empty() {
            return Err(format!(
                "celle d'uscita diverse da quelle scritte (riga, colonna): {diverse:?}"
            ));
        }
    }
    Ok(())
}

/// Le posizioni `(riga, colonna)` in cui due rese hanno valori diversi, al
/// più 20.
fn celle_diverse(una: &Resa, altra: &Resa) -> Vec<(usize, String)> {
    let mut diverse = Vec::new();
    for (riga, (prima, seconda)) in una.valori.iter().zip(&altra.valori).enumerate() {
        for ((a, b), nome) in prima.iter().zip(seconda).zip(&una.intestazioni) {
            if a != b && diverse.len() < 20 {
                diverse.push((riga, nome.clone()));
            }
        }
    }
    diverse
}

/// Verifica sul contratto, per le operazioni geo che il runner non esegue:
/// l'analisi accetta la config e l'uscita ha le colonne e i tipi scritti.
fn verifica_contratto(scheda: &Scheda, ingressi: &[RecordBatch]) -> Result<(), String> {
    plenora_core::json::ensure_no_duplicate_keys(scheda.esempio.config.get())
        .map_err(|e| e.to_string())?;
    let config: Value = serde_json::from_str(scheda.esempio.config.get()).expect("config JSON");
    let contratti = ingressi
        .iter()
        .map(|batch| contract_from_arrow_schema(batch.schema(), resolve_crs))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("contratto d'ingresso: {e}"))?;
    let crs_piano = scheda
        .esempio
        .crs
        .as_deref()
        .map(|crs| resolve_crs(crs, "crs"))
        .transpose()
        .map_err(|e| format!("CRS di piano: {e}"))?;
    let mut campi = FieldAllocator::new(1000);
    let contratto = plenora_kernels_geo::analyze::analyze_geo_contract(
        scheda.operazione.id,
        &contratti,
        &config,
        crs_piano.as_ref(),
        &mut campi,
    )
    .map_err(|e| format!("analisi: {e}"))?;
    let schema = arrow_schema_from_contract(&contratto).map_err(|e| e.to_string())?;
    let attesa = tabella(&scheda.esempio.uscita.colonne)?;
    let vuota = RecordBatch::new_empty(schema);
    let reale = rendi(&vuota);
    let scritta = rendi(&attesa);
    if reale.intestazioni != scritta.intestazioni || reale.tipi != scritta.tipi {
        return Err(format!(
            "colonne del contratto {:?}, scritte {:?}",
            reale.intestazioni, scritta.intestazioni
        ));
    }
    Ok(())
}

fn kernel_diretto(scheda: &Scheda, ingresso: &RecordBatch) -> Result<RecordBatch, String> {
    use plenora_kernels_table::reshape;
    let limiti = plenora_kernels_table::Limits::default();
    let config = scheda.esempio.config.get();
    plenora_core::json::ensure_no_duplicate_keys(config).map_err(|e| e.to_string())?;
    match scheda.operazione.id {
        "table.pivot" => {
            let config: reshape::Pivot = serde_json::from_str(config).map_err(|e| e.to_string())?;
            reshape::pivot(ingresso, &config, &limiti).map_err(|e| e.to_string())
        }
        "table.transpose" => {
            let config: reshape::Transpose =
                serde_json::from_str(config).map_err(|e| e.to_string())?;
            reshape::transpose(ingresso, &config, &limiti).map_err(|e| e.to_string())
        }
        altra => Err(format!("{altra}: nessuna chiamata diretta prevista")),
    }
}

/// Le sole operazioni la cui uscita è casuale per contratto: per loro, e
/// solo per loro, un esempio confronta schema e righe ma non i valori. Il
/// catalogo non lo dice (`uuid_generator` vi dichiara un ordine definito,
/// che riguarda le righe, non i valori), quindi l'elenco è qui, esplicito.
const USCITA_CASUALE: [&str; 1] = ["table.uuid_generator"];

#[expect(
    clippy::disallowed_methods,
    reason = "Selezione delle operazioni senza implementazione eseguibile dopo la validazione; i casi eseguibili hanno un atteso indipendente."
)]
fn esegui_esempio(scheda: &Scheda) -> Result<Verifica, String> {
    let esempio = &scheda.esempio;
    let casuale = USCITA_CASUALE.contains(&scheda.operazione.id);
    if esempio.valori_confrontati == casuale {
        return Err(format!(
            "`valori_confrontati` deve essere {} per questa operazione",
            !casuale
        ));
    }
    let ingressi = esempio
        .ingressi
        .iter()
        .map(|spec| tabella_con_metadati(&spec.colonne, &spec.metadati))
        .collect::<Result<Vec<_>, _>>()?;
    let righe_in: Vec<usize> = ingressi.iter().map(RecordBatch::num_rows).collect();
    let piano = Pipeline::from_json(&testo_piano(scheda)).map_err(|e| format!("piano: {e}"))?;
    let schemi: Vec<(&str, SchemaRef)> = esempio
        .ingressi
        .iter()
        .zip(&ingressi)
        .map(|(spec, batch)| (spec.nome.as_str(), batch.schema()))
        .collect();
    // L'elenco delle operazioni che il runner non esegue mai
    // (`plenora_pipeline::disponibilita`, pubblicato da `data.catalog`) deve
    // dire il vero in entrambe le direzioni: un'operazione elencata non passa
    // la validazione, e una non elencata che non la passa e' solo `pivot`
    // senza `mapping` (rifiuto che dipende dalla config).
    let non_eseguibile =
        plenora_pipeline::disponibilita::motivo_non_eseguibile(scheda.operazione.id).is_some();
    let validata = piano.validate(&schemi);
    if non_eseguibile && validata.is_ok() {
        return Err("dichiarata non eseguibile dal runner, ma la validazione la accetta".into());
    }
    match validata {
        Ok(validata) => {
            let tabelle = esempio
                .ingressi
                .iter()
                .zip(ingressi)
                .map(|(spec, batch)| (spec.nome.clone(), batch))
                .collect();
            let esito = validata
                .run(tabelle)
                .map_err(|e| format!("esecuzione: {e}"))?;
            let (_, uscita) = esito.outputs.into_iter().next().expect("un'uscita");
            verifica_forma(scheda.operazione, &righe_in, uscita.num_rows(), true)?;
            confronta(scheda, &uscita, esempio.valori_confrontati)?;
            Ok(if esempio.valori_confrontati {
                Verifica::Runner
            } else {
                Verifica::RunnerSenzaValori
            })
        }
        Err(PlenoraError::Unsupported(_)) if scheda.operazione.family == Family::Geo => {
            verifica_contratto(scheda, &ingressi)?;
            Ok(Verifica::Contratto)
        }
        // Dal kernel solo `pivot` (senza `mapping` nell'esempio) e le
        // operazioni che il runner non esegue mai: un'altra rifiutata
        // (`transpose` tolta dall'elenco compresa) e' un errore sotto.
        Err(errore) if scheda.operazione.id == "table.pivot" || non_eseguibile => {
            let uscita = kernel_diretto(scheda, &ingressi[0])
                .map_err(|e| format!("kernel: {e} (il runner: {errore})"))?;
            verifica_forma(scheda.operazione, &righe_in, uscita.num_rows(), true)?;
            confronta(scheda, &uscita, true)?;
            Ok(Verifica::Kernel)
        }
        Err(errore) => Err(format!("validazione: {errore}")),
    }
}

/// La forma delle righe che il catalogo dichiara, contro le righe che il
/// runner ha reso su un ingresso: la classe di difetti dei descrittori più
/// larghi o più stretti del kernel, provata su ogni operazione.
///
/// - `source_row_provenance` `Preserved` (tabellari e geo): tante righe
///   quante il primo ingresso (identità e ordine li prova l'oracolo di
///   `plenora-pipeline`, `diagnostica_righe.rs`);
/// - forma geo 1:1, produttore o diagnostica: tante righe quante il primo
///   ingresso, anche per le binarie (la destra non aggiunge righe);
/// - forma N:1: al più una riga per riga d'ingresso, e al più una da un
///   ingresso vuoto;
/// - con `testimone`, cioè sull'esempio della scheda: una forma geo 1:N,
///   N:1 o «da tutto l'ingresso» ha un numero di righe diverso dal primo
///   ingresso, così l'esempio mostra la forma che il catalogo dichiara e una
///   forma più larga del kernel non passa inosservata.
fn verifica_forma(
    operazione: &OperationDescriptor,
    righe_in: &[usize],
    righe_out: usize,
    testimone: bool,
) -> Result<(), String> {
    let prima = righe_in.first().copied().unwrap_or_default();
    let conserva = operazione.source_row_provenance() == SourceRowProvenance::Preserved;
    let dichiarata = operazione.result_shape;
    let uno_a_uno = matches!(
        dichiarata,
        Some(ResultShape::OneToOne | ResultShape::FromCoords | ResultShape::Diagnostic)
    );
    if (conserva || uno_a_uno) && righe_out != prima {
        return Err(format!(
            "forma: {righe_out} righe da {righe_in:?}, il catalogo dichiara una riga per riga \
             del primo ingresso"
        ));
    }
    if dichiarata == Some(ResultShape::ManyToOne) && righe_out > prima.max(1) {
        return Err(format!(
            "forma: {righe_out} righe da {righe_in:?}, il catalogo dichiara N:1"
        ));
    }
    let larga = matches!(
        dichiarata,
        Some(ResultShape::OneToMany | ResultShape::ManyToOne | ResultShape::WholeToMany)
    );
    if testimone && larga && righe_out == prima {
        return Err(format!(
            "forma: l'esempio non mostra la forma {} ({righe_out} righe da {righe_in:?})",
            forma(dichiarata)
        ));
    }
    Ok(())
}

fn prepara_ambiente() {
    // Solo questo binario di test legge la variabile, e la scrive sempre
    // con lo stesso valore: l'ordine fra i test non conta.
    std::env::set_var(CHIAVE_HMAC, "chiave-di-esempio");
}

// ---------------------------------------------------------------------------
// Il documento
// ---------------------------------------------------------------------------

const fn famiglia(operazione: &OperationDescriptor) -> &'static str {
    match (operazione.family, operazione.origin) {
        (Family::Table, Origin::ManipolaCompat) => "tabellare, compatibile Manipola",
        (Family::Table, Origin::Extension) => "tabellare, estensione",
        (Family::Geo, Origin::ManipolaCompat) => "geo, compatibile Manipola",
        (Family::Geo, Origin::Extension) => "geo, estensione",
    }
}

const fn arieta(arity: Arity) -> &'static str {
    match arity {
        Arity::Unary => "unaria",
        Arity::BinaryOrdered => "binaria ordinata (sinistra, destra)",
        Arity::NAry => "N-aria",
    }
}

const fn esecuzione(classe: ExecutionClass) -> &'static str {
    match classe {
        ExecutionClass::Streaming => "streaming (1:1, batch per batch)",
        ExecutionClass::Blocking => "bloccante (tutto l'ingresso)",
        ExecutionClass::BinaryBlocking => "bloccante su due ingressi",
    }
}

const fn cancellazione(comportamento: CancellationBehavior) -> &'static str {
    match comportamento {
        CancellationBehavior::Cooperative => "cooperativa",
        CancellationBehavior::BoundaryOnly => "solo ai confini",
        CancellationBehavior::NonInterruptible => "non interrompibile",
    }
}

const fn forma(shape: Option<ResultShape>) -> &'static str {
    match shape {
        None => "non dichiarata (tabellare)",
        Some(ResultShape::OneToOne) => "1:1",
        Some(ResultShape::OneToMany) => "1:N",
        Some(ResultShape::ManyToOne) => "N:1",
        Some(ResultShape::Collective) => "collettiva",
        Some(ResultShape::WholeToMany) => "da tutto l'ingresso a molte righe",
        Some(ResultShape::FromCoords) => "produttore 1:1",
        Some(ResultShape::Diagnostic) => "diagnostica",
    }
}

const fn requisito_crs(requisito: Option<CrsRequirement>) -> &'static str {
    match requisito {
        None => "nessuno",
        Some(CrsRequirement::Known) => "CRS noto",
        Some(CrsRequirement::Projected) => "CRS proiettato",
        Some(CrsRequirement::Geographic) => "CRS geografico",
        Some(CrsRequirement::SameProjected) => "stesso CRS proiettato sui due ingressi",
        Some(CrsRequirement::Reprojection) => "CRS d'origine e di destinazione risolti",
    }
}

const fn determinismo(politica: DeterminismPolicy) -> &'static str {
    match politica {
        DeterminismPolicy::DefinedOrder => "ordine definito dall'operazione",
        DeterminismPolicy::InputOrder => "ordine d'arrivo degli ingressi",
        DeterminismPolicy::StableKeyOrder => "ordinamento stabile sulla chiave dichiarata",
        DeterminismPolicy::CanonicalOrder => "ordine canonico dei valori",
    }
}

fn espansione(operazione: &OperationDescriptor) -> String {
    if operazione.expansion_factor_exempt {
        return "esente da `max_expansion_factor` (restano i limiti di righe)".into();
    }
    // Per le unarie il runner misura sempre sull'unico ingresso: il vincolo
    // dichiarato vale solo per chi ha due ingressi.
    if operazione.arity == Arity::Unary {
        return "uscita / ingresso".into();
    }
    match operazione.expansion_constraint {
        ExpansionConstraint::SumRelative => "uscita / (sinistra + destra)".into(),
        ExpansionConstraint::LeftRelative => "uscita / sinistra".into(),
        ExpansionConstraint::RightRelative => "uscita / destra".into(),
        ExpansionConstraint::MaxRelative => "max(uscita / sinistra, uscita / destra)".into(),
        ExpansionConstraint::Custom(fattore) => {
            format!("uscita / (sinistra + destra), soglia propria {fattore}")
        }
    }
}

const fn maturita(maturity: Maturity) -> &'static str {
    match maturity {
        Maturity::Planned => "pianificata",
        Maturity::BackendPending => "in attesa del backend",
        Maturity::KernelValidated => "kernel validato",
        Maturity::PublicProtocol => "protocollo pubblico",
    }
}

const fn fusione(geo_fusion: GeoFusion) -> &'static str {
    match geo_fusion {
        GeoFusion::NotFusible => "non fondibile",
        GeoFusion::TransformInPlace => "trasformazione sul posto",
        GeoFusion::TerminalMeasure => "misura terminale",
    }
}

fn scheda_catalogo(operazione: &OperationDescriptor) -> String {
    let mut uscita = String::new();
    let alias: Vec<String> = ALIASES
        .iter()
        .filter(|(_, _, canonico)| *canonico == operazione.id)
        .map(|(versione, alias, _)| format!("`{alias}` (schema {versione})"))
        .collect();
    let provenienza = match operazione.source_row_provenance() {
        SourceRowProvenance::Preserved => "conservata",
        SourceRowProvenance::Unavailable => "non disponibile",
    };
    let requisiti = if operazione.required_capabilities.is_empty() {
        "nessuna".to_owned()
    } else {
        operazione
            .required_capabilities
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let righe: Vec<(&str, String)> = vec![
        ("famiglia", famiglia(operazione).into()),
        (
            "geometrie multiple",
            if operazione.supports_multiple_geometries() {
                "supportate per colonna"
            } else {
                "rifiutate: limite transitorio"
            }
            .into(),
        ),
        (
            "alias legacy",
            if alias.is_empty() {
                "nessuno".into()
            } else {
                format!(
                    "{} (risolti da `find_operation`; il runner li rifiuta)",
                    alias.join(", ")
                )
            },
        ),
        ("arietà", arieta(operazione.arity).into()),
        (
            "esecuzione",
            format!(
                "{}; cancellazione {}",
                esecuzione(operazione.execution_class),
                cancellazione(operazione.cancellation_behavior)
            ),
        ),
        ("forma del risultato", forma(operazione.result_shape).into()),
        ("determinismo", determinismo(operazione.determinism).into()),
        ("indice della riga sorgente", provenienza.into()),
        (
            "requisito CRS",
            requisito_crs(operazione.crs_requirement).into(),
        ),
        ("capability richieste", requisiti),
        ("vincolo di espansione", espansione(operazione)),
        ("fusione geo", fusione(operazione.geo_fusion).into()),
        ("maturità", maturita(operazione.maturity).into()),
        (
            "versioni",
            format!(
                "semantica {}, config {}, analisi {}, kernel {}",
                operazione.semantic_version,
                operazione.config_schema_version,
                operazione.contract_analysis_version,
                operazione.kernel_version
            ),
        ),
    ];
    uscita.push_str("| dal catalogo | |\n| --- | --- |\n");
    for (chiave, valore) in righe {
        let _ = writeln!(uscita, "| {chiave} | {valore} |");
    }
    uscita
}

fn sezione_esempio(scheda: &Scheda, verifica: Verifica) -> String {
    let esempio = &scheda.esempio;
    let mut uscita = String::new();
    if !scheda.premessa_esempio.is_empty() {
        let _ = writeln!(uscita, "{}\n", scheda.premessa_esempio);
    }
    let nomi: Vec<String> = esempio
        .ingressi
        .iter()
        .map(|t| serde_json::to_string(&t.nome).expect("nome"))
        .collect();
    uscita.push_str("Passo del piano");
    if let Some(crs) = &esempio.crs {
        let _ = write!(uscita, " (con `\"crs\": \"{crs}\"` di piano)");
    }
    if let Some(limits) = &esempio.limits {
        let _ = write!(uscita, " (con `\"limits\": {}` di piano)", limits.get());
    }
    uscita.push_str(":\n\n```json\n");
    let _ = writeln!(
        uscita,
        "{{\"out\": \"{USCITA}\", \"op\": \"{}\", \"in\": [{}],\n \"config\": {}}}",
        scheda.operazione.id,
        nomi.join(", "),
        esempio.config.get()
    );
    uscita.push_str("```\n");
    for spec in &esempio.ingressi {
        let batch =
            tabella_con_metadati(&spec.colonne, &spec.metadati).expect("ingresso già verificato");
        let crs: Vec<String> = spec
            .colonne
            .iter()
            .filter(|c| c.tipo == "geometry")
            .map(|c| {
                format!(
                    "`{}` in {}",
                    c.nome,
                    c.crs.as_deref().unwrap_or("EPSG:4326")
                )
            })
            .collect();
        let crs = if crs.is_empty() {
            String::new()
        } else {
            format!(" (geometrie {})", crs.join(", "))
        };
        let metadati = if spec.metadati.is_empty() {
            String::new()
        } else {
            format!(
                " (metadati di schema `{}`)",
                serde_json::to_string(&spec.metadati).expect("metadati")
            )
        };
        let _ = writeln!(uscita, "\nIngresso `{}`{crs}{metadati}:\n", spec.nome);
        uscita.push_str(&tabella_markdown(&rendi(&batch)));
    }
    let attesa = tabella(&esempio.uscita.colonne).expect("uscita già verificata");
    let _ = writeln!(uscita, "\nUscita `{USCITA}`:\n");
    uscita.push_str(&tabella_markdown(&rendi(&attesa)));
    let _ = writeln!(uscita, "\nVerifica: {}", verifica.testo());
    uscita
}

fn documento(schede: &[Scheda], verifiche: &[Verifica]) -> String {
    let mut uscita = String::new();
    uscita.push_str(INTESTAZIONE);
    for (famiglia, titolo) in [(Family::Table, "Tabellari"), (Family::Geo, "Geografiche")] {
        let ids: Vec<String> = schede
            .iter()
            .filter(|s| s.operazione.family == famiglia)
            .map(|s| {
                format!(
                    "[`{}`](#{})",
                    s.operazione.id,
                    slug(&format!("`{}`", s.operazione.id))
                )
            })
            .collect();
        let _ = writeln!(uscita, "- **{titolo}** ({}): {}", ids.len(), ids.join(", "));
    }
    let contratto = verifiche
        .iter()
        .filter(|v| **v == Verifica::Contratto)
        .count();
    let _ = writeln!(
        uscita,
        "\nEsempi: {} eseguiti con l'uscita confrontata, {} verificati solo sul contratto.\n",
        verifiche.len() - contratto,
        contratto
    );
    let mut famiglia_corrente = None;
    for (scheda, verifica) in schede.iter().zip(verifiche) {
        if famiglia_corrente != Some(scheda.operazione.family) {
            famiglia_corrente = Some(scheda.operazione.family);
            let titolo = match scheda.operazione.family {
                Family::Table => "Operazioni tabellari",
                Family::Geo => "Operazioni geografiche",
            };
            let _ = writeln!(uscita, "## {titolo}\n");
        }
        let _ = writeln!(uscita, "### `{}`\n", scheda.operazione.id);
        uscita.push_str(&scheda_catalogo(scheda.operazione));
        for (titolo, corpo) in &scheda.sezioni {
            let _ = writeln!(uscita, "\n#### {titolo}\n\n{corpo}");
        }
        let _ = writeln!(uscita, "\n#### Memoria\n\n{SEGNAPOSTO_MEMORIA}");
        let _ = write!(
            uscita,
            "\n#### Esempio\n\n{}",
            sezione_esempio(scheda, *verifica)
        );
        uscita.push('\n');
    }
    // Una sola riga vuota in coda.
    while uscita.ends_with("\n\n") {
        uscita.pop();
    }
    uscita
}

const INTESTAZIONE: &str = "\
# Operazioni

<!-- Generato da crates/plenora-io/tests/operazioni_doc.rs dalle schede in
docs/schede/ e dal catalogo: non si modifica a mano. Rigenerare con
PLENORA_RIGENERA_DOC=1 cargo test -p plenora-io --test operazioni_doc -->

Una scheda per operazione del catalogo. La tabella «dal catalogo» di ogni
scheda è letta da `plenora_core::catalog` (`CATALOG`, `ALIASES`), il resto
dalla scheda in `docs/schede/<id>.md`; un test confronta questo documento con
quello che schede e catalogo generano, e un esempio che non gira o la cui
uscita non è quella scritta fa fallire lo stesso test.

Regole comuni, che le schede non ripetono:

- **Politiche e limiti** stanno nelle [guide](README.md): precisione
  geografica di 1 cm ([«Precisione delle operazioni geografiche»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)),
  validazione e budget del [runner](runner.md#runner), limiti dei
  [file](file.md#limiti-dichiarati) e della
  [riproiezione](riproiezione.md#limiti-dichiarati-della-riproiezione). Le
  schede li collegano, non li ricopiano.
- **Config**: ogni config si legge con `deny_unknown_fields`; un campo
  sconosciuto, un tipo sbagliato o un valore fuori dominio è `InvalidPlan`
  in validazione. Un parametro scritto che l'operazione ignorerebbe si
  rifiuta ([Runner, «Validazione»](runner.md#validazione)).
- **Errori senza dati**: i messaggi nominano colonne, parametri e limiti,
  mai i valori delle celle.
- **Limiti di risorsa** (`max_rows_per_edge`, `max_output_rows`,
  `max_expansion_factor`, `max_columns`, `max_string_bytes`, …) valgono per
  ogni operazione e sono `ResourceLimit`; le schede nominano solo quelli
  propri dell'operazione.
- **Tipi negli esempi**: `utf8`, `int64`, `float64`, `bool`, `date32`,
  `timestamp(unità[, fuso])`, `decimal128(p, s)`, `list<…>`, `struct<…>`,
  `dictionary<…>` sono i tipi Arrow; `geometry` è una colonna `Binary`
  GeoArrow-WKB, scritta in WKT. `null` è il valore nullo, `\"\"` la stringa
  vuota; `float64` si scrive nella forma più corta che torna allo stesso
  valore (`1.0`, `0.1`).
- **Memoria**: il picco misurato per operazione verrà dal catalogo delle
  misure v4; finché non c'è, ogni scheda porta il segnaposto.

";

// ---------------------------------------------------------------------------
// Collegamenti
// ---------------------------------------------------------------------------

/// L'ancora che GitHub dà a un titolo: minuscole, via tutto ciò che non è
/// lettera, cifra, spazio, `-` o `_`, spazi in `-`.
fn slug(titolo: &str) -> String {
    titolo
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// Le ancore dei titoli di un documento Markdown, con i suffissi `-1`, `-2`
/// dei titoli ripetuti; i blocchi di codice non contano.
fn ancore(testo: &str) -> BTreeSet<String> {
    let mut contatori: BTreeMap<String, usize> = BTreeMap::new();
    let mut ancore = BTreeSet::new();
    let mut in_codice = false;
    for riga in testo.lines() {
        if riga.starts_with("```") {
            in_codice = !in_codice;
            continue;
        }
        if in_codice || !riga.starts_with('#') {
            continue;
        }
        let titolo = riga.trim_start_matches('#');
        if !titolo.starts_with(' ') {
            continue;
        }
        let base = slug(titolo);
        let volte = contatori.entry(base.clone()).or_insert(0);
        let ancora = if *volte == 0 {
            base.clone()
        } else {
            format!("{base}-{volte}")
        };
        *volte += 1;
        ancore.insert(ancora);
    }
    ancore
}

/// I collegamenti `](destinazione)` di un testo.
fn collegamenti(testo: &str) -> Vec<String> {
    let mut uscita = Vec::new();
    let mut resto = testo;
    while let Some(inizio) = resto.find("](") {
        resto = &resto[inizio + 2..];
        if let Some(fine) = resto.find(')') {
            uscita.push(resto[..fine].to_owned());
            resto = &resto[fine..];
        }
    }
    uscita
}

/// I collegamenti del documento generato che non portano da nessuna parte.
/// Un percorso si risolve da `docs/`, dove il documento vive; un'ancora si
/// cerca fra i titoli del documento di destinazione, o fra i propri.
fn collegamenti_rotti(documento: &str) -> Vec<String> {
    let docs = radice().join("docs");
    let proprie = ancore(documento);
    let mut rotti = Vec::new();
    for destinazione in collegamenti(documento) {
        let (file, ancora) = match destinazione.split_once('#') {
            Some((file, ancora)) => (file, Some(ancora)),
            None => (destinazione.as_str(), None),
        };
        let valido = if destinazione.starts_with("https://") {
            true
        } else if file.is_empty() {
            ancora.is_some_and(|ancora| proprie.contains(ancora))
        } else {
            let percorso = docs.join(file);
            ancora.map_or_else(
                || percorso.exists(),
                |ancora| {
                    percorso.extension().is_some_and(|e| e == "md")
                        && percorso.is_file()
                        && ancore(&leggi(&percorso)).contains(ancora)
                },
            )
        };
        if !valido {
            rotti.push(destinazione);
        }
    }
    rotti
}

// ---------------------------------------------------------------------------
// Le prove
// ---------------------------------------------------------------------------

#[test]
fn ogni_operazione_ha_una_scheda_completa() {
    let (_, errori) = leggi_schede();
    assert!(errori.is_empty(), "schede:\n{}", errori.join("\n"));
    assert_eq!(
        find_operation("table.filter").map(|o| o.id),
        Some("table.filter")
    );
}

fn verifiche(schede: &[Scheda]) -> (Vec<Verifica>, Vec<String>) {
    prepara_ambiente();
    let mut verifiche = Vec::new();
    let mut errori = Vec::new();
    for scheda in schede {
        match esegui_esempio(scheda) {
            Ok(verifica) => verifiche.push(verifica),
            Err(errore) => {
                errori.push(format!("{}: {errore}", scheda.operazione.id));
                verifiche.push(Verifica::Contratto);
            }
        }
    }
    (verifiche, errori)
}

/// Controlla le schede presenti, anche senza tutte le altre: chi scrive una
/// scheda la prova da sola. Con `PLENORA_ANTEPRIMA_DOC=<file>` scrive in
/// `<file>` il documento delle sole schede presenti.
#[test]
fn gli_esempi_delle_schede_girano() {
    let (schede, errori) = leggi_schede();
    let errori: Vec<String> = errori
        .into_iter()
        .filter(|errore| !errore.ends_with(": scheda assente"))
        .collect();
    assert!(errori.is_empty(), "schede:\n{}", errori.join("\n"));
    let (verifiche, errori) = verifiche(&schede);
    assert!(errori.is_empty(), "esempi:\n{}", errori.join("\n\n"));
    if let Some(percorso) = std::env::var_os(ANTEPRIMA) {
        let generato = documento(&schede, &verifiche);
        let rotti = collegamenti_rotti(&generato);
        assert!(rotti.is_empty(), "collegamenti rotti: {rotti:?}");
        std::fs::write(percorso, generato).expect("scrittura dell'anteprima");
    }
}

/// L'esempio di ogni scheda sugli stessi ingressi senza righe: il piano
/// gira dal runner (dal kernel per `pivot` e `transpose`) e l'uscita
/// rispetta la forma del catalogo ([`verifica_forma`], senza testimone).
/// Un rifiuto è ammesso solo per le operazioni di [`RIFIUTANO_IL_VUOTO`].
#[test]
fn gli_esempi_su_ingressi_vuoti_rispettano_la_forma() {
    prepara_ambiente();
    let (schede, _) = leggi_schede();
    let mut errori = Vec::new();
    for scheda in &schede {
        let id = scheda.operazione.id;
        if RIFIUTANO_IL_VUOTO.contains(&id) {
            let ingressi: Vec<_> = scheda
                .esempio
                .ingressi
                .iter()
                .map(|spec| {
                    tabella_con_metadati(&spec.colonne, &spec.metadati)
                        .expect("fixture della scheda")
                        .slice(0, 0)
                })
                .collect();
            // Le due schede dichiarano un difetto del piano: min_rows non
            // rispettato e meno di due punti. La fase canonica di InvalidPlan
            // resta Validate anche quando il controllo avviene in esecuzione.
            let errore = esegui_su_tipizzato(scheda, &ingressi)
                .expect_err("lo scenario vuoto deve essere rifiutato");
            verifica_rifiuto(
                &errore,
                plenora_core::ErrorCategory::InvalidPlan,
                plenora_core::ErrorPhase::Validate,
            );
        } else if let Err(errore) = esegui_su_vuoti(scheda) {
            errori.push(format!("{id}: {errore}"));
        }
    }
    assert!(errori.is_empty(), "ingressi vuoti:\n{}", errori.join("\n"));
}

/// Le operazioni che, con la config del loro esempio, rifiutano un ingresso
/// vuoto per contratto: `assert_cardinality` con `min_rows` (la scheda,
/// «Errori»), `voronoi` con meno di due punti non nulli (`InsufficientPoints`, la
/// scheda, «Errori»).
const RIFIUTANO_IL_VUOTO: &[&str] = &["table.assert_cardinality", "geo.voronoi"];

fn esegui_su_vuoti(scheda: &Scheda) -> Result<usize, String> {
    let ingressi = scheda
        .esempio
        .ingressi
        .iter()
        .map(|spec| tabella_con_metadati(&spec.colonne, &spec.metadati).map(|t| t.slice(0, 0)))
        .collect::<Result<Vec<_>, _>>()?;
    let attesa = if scheda.operazione.family == Family::Geo {
        attesa_geo_vuota(scheda, &ingressi, true)
    } else {
        attesa_tabella_vuota(scheda, &ingressi)
    };
    let uscita = if matches!(scheda.operazione.id, "table.pivot" | "table.transpose") {
        kernel_diretto(scheda, &ingressi[0])?
    } else {
        esegui_su_tipizzato(scheda, &ingressi).map_err(|e| format!("esecuzione: {e}"))?
    };
    if attesa.schema() != uscita.schema() {
        return Err(format!(
            "schema atteso {:?}, ottenuto {:?}",
            attesa.schema(),
            uscita.schema()
        ));
    }
    if !stessi_valori(&attesa, &uscita) {
        return Err("valori o numero di righe diversi dall'oracolo vuoto".into());
    }
    let righe_out = uscita.num_rows();
    verifica_forma(
        scheda.operazione,
        &vec![0; ingressi.len()],
        righe_out,
        false,
    )?;
    Ok(righe_out)
}

/// Gli esempi tabellari vuoti conservano le colonne scritte nelle schede,
/// tranne i due reshape dipendenti dai dati. Reconcile emette cinque zeri.
/// Nullabilita' e metadati seguono le sezioni Schema delle stesse schede;
/// non vengono interrogati kernel, analisi o risultati dell'esecuzione.
fn attesa_tabella_vuota(scheda: &Scheda, ingressi: &[RecordBatch]) -> RecordBatch {
    let op = scheda.operazione.id;
    if op == "table.transpose" {
        return ingressi[0].clone();
    }
    if op == "table.pivot" {
        // Nessun valore pivot: resta la sola chiave negozio, senza pubblicazione.
        return ingressi[0].project(&[0]).unwrap();
    }
    let mut colonne = scheda.esempio.uscita.colonne.clone();
    for colonna in &mut colonne {
        if op == "table.reconcile" {
            if colonna.nome == "value" {
                colonna.valori = (0..5)
                    .map(|_| RawValue::from_string("0".into()).unwrap())
                    .collect();
            }
        } else {
            colonna.valori.clear();
        }
    }
    let valori = tabella(&colonne).unwrap();
    let campi: Vec<_> = valori
        .schema()
        .fields()
        .iter()
        .map(|campo| {
            let non_nullable = matches!(
                (op, campo.name().as_str()),
                ("table.add_row_number" | "table.aggregate", "n")
                    | ("table.align_schema", "stato")
                    | ("table.md5_hash", "md5_hash")
                    | ("table.sha256_hash", "sha256_hash")
                    | ("table.stable_fingerprint", "fingerprint")
                    | ("table.melt", "trimestre")
                    | ("table.table_diff", "_diff_status")
                    | ("table.uuid_generator", "chiave")
                    | ("table.validate_rules", "_valid" | "_errors" | "_warnings")
            ) || op == "table.reconcile";
            campo.as_ref().clone().with_nullable(!non_nullable)
        })
        .collect();
    let schema = schema_ruolo_catalogo(op, "", &ingressi[0], campi);
    RecordBatch::try_new(Arc::new(schema), valori.columns().to_vec()).unwrap()
}

#[test]
fn il_documento_e_aggiornato() {
    let (schede, errori) = leggi_schede();
    assert!(errori.is_empty(), "schede:\n{}", errori.join("\n"));
    let (verifiche, errori) = verifiche(&schede);
    assert!(errori.is_empty(), "esempi:\n{}", errori.join("\n\n"));
    let generato = documento(&schede, &verifiche);
    let rotti = collegamenti_rotti(&generato);
    assert!(rotti.is_empty(), "collegamenti rotti: {rotti:?}");
    let percorso = percorso_documento();
    if std::env::var(RIGENERA).as_deref() == Ok("1") {
        std::fs::write(&percorso, &generato).expect("scrittura del documento");
        return;
    }
    let attuale = if percorso.exists() {
        leggi(&percorso)
    } else {
        String::new()
    };
    assert!(
        attuale == generato,
        "docs/operazioni.md non è aggiornato: rigenerarlo con \
         {RIGENERA}=1 cargo test -p plenora-io --test operazioni_doc"
    );
}

/// Il confronto degli esempi separa nullità e valore: il nullo, la stringa
/// `"null"`, la stringa vuota e la stringa `""` (due virgolette) sono
/// quattro valori diversi, anche nella resa del documento.
#[test]
fn il_confronto_distingue_nullo_e_testi_simili() {
    let spec: ColonnaSpec = serde_json::from_str(
        r#"{"nome": "t", "tipo": "utf8", "valori": [null, "null", "", "\"\""]}"#,
    )
    .expect("spec");
    let batch = tabella(std::slice::from_ref(&spec)).expect("tabella");
    let resa = rendi(&batch);
    let valori: Vec<&Option<String>> = resa.valori.iter().map(|riga| &riga[0]).collect();
    let celle: Vec<&String> = resa.righe.iter().map(|riga| &riga[0]).collect();
    for i in 0..4 {
        for j in (i + 1)..4 {
            assert_ne!(valori[i], valori[j], "valori {i} e {j} confusi");
            assert_ne!(celle[i], celle[j], "celle {i} e {j} confuse");
        }
    }
    // Stesse celle in un'altra riga dello stesso batch: nessuna differenza;
    // una riga in cui il nullo diventa "null": una differenza, per posizione.
    let altra: ColonnaSpec = serde_json::from_str(
        r#"{"nome": "t", "tipo": "utf8", "valori": ["null", "null", "", "\"\""]}"#,
    )
    .expect("spec");
    let altra = rendi(&tabella(std::slice::from_ref(&altra)).expect("tabella"));
    assert!(celle_diverse(&resa, &resa).is_empty());
    assert_eq!(
        celle_diverse(&resa, &altra),
        vec![(0, "t: utf8".to_owned())]
    );
}

/// Un fuso non ASCII nel tipo è un errore esplicito, non un panico.
#[test]
fn il_tipo_rifiuta_un_fuso_non_ascii() {
    assert_eq!(
        tipo_arrow("timestamp(ms, è)"),
        Err("tipo `timestamp(ms, è)`: fuso vuoto o non ASCII".to_owned())
    );
    assert_eq!(
        tipo_arrow("timestamp(ms, UTC"),
        Err("tipo `timestamp(ms, UTC`: atteso `)`".to_owned())
    );
    assert_eq!(
        tipo_arrow("timestamp(ms, Europe/Rome)").map(|(tipo, _)| tipo),
        Ok(DataType::Timestamp(
            TimeUnit::Millisecond,
            Some(Arc::from("Europe/Rome"))
        ))
    );
}

// ---------------------------------------------------------------------------
// Nullabilita' dichiarata delle geo contro quella emessa
// ---------------------------------------------------------------------------

/// Le colonne che una geo dichiara nullable anche con ingressi senza null:
/// il kernel rende null per un valore presente. `(operazione, colonna,
/// motivo)`; ogni altra colonna nullable con ingressi senza null e' una
/// dichiarazione piu' larga dell'uscita (il difetto di `geo.nearest`, che
/// dichiarava `__right_index` e `distance` nullable senza mai emetterne).
const NULL_CON_INGRESSI_PRESENTI: &[(&str, &str, &str)] = &[
    (
        "geo.bounds_extractor",
        "geometry_minx",
        "geometria vuota: nessun limite",
    ),
    (
        "geo.bounds_extractor",
        "geometry_miny",
        "geometria vuota: nessun limite",
    ),
    (
        "geo.bounds_extractor",
        "geometry_maxx",
        "geometria vuota: nessun limite",
    ),
    (
        "geo.bounds_extractor",
        "geometry_maxy",
        "geometria vuota: nessun limite",
    ),
    (
        "geo.clean_topology",
        "geometry",
        "riga assorbita dalle precedenti",
    ),
    ("geo.clip", "geometry", "ritaglio vuoto"),
    ("geo.intersection", "geometry", "risultato vuoto"),
    ("geo.union", "geometry", "risultato vuoto"),
    ("geo.difference", "geometry", "risultato vuoto"),
    ("geo.symmetric_difference", "geometry", "risultato vuoto"),
    ("geo.cluster_dbscan", "cluster_id", "rumore"),
    ("geo.dissolve", "geometry", "tabella vuota: una riga null"),
    (
        "geo.line_builder",
        "geometry",
        "meno di due punti, tabella vuota",
    ),
    (
        "geo.polygon_builder",
        "geometry",
        "punti insufficienti, tabella vuota",
    ),
    (
        "geo.distance",
        "distance",
        "geometria vuota: nessuna distanza",
    ),
    (
        "geo.hausdorff_distance",
        "hausdorff_distance",
        "geometria vuota: nessuna distanza",
    ),
    (
        "geo.frechet_distance",
        "frechet_distance",
        "linea vuota: nessuna distanza",
    ),
    (
        "geo.line_locate_point",
        "fraction",
        "non LineString o linea vuota",
    ),
    (
        "geo.geometry_accessors",
        "start_point",
        "non una linea aperta",
    ),
    (
        "geo.geometry_accessors",
        "end_point",
        "non una linea aperta",
    ),
    (
        "geo.geometry_diagnostics",
        "validity_reason",
        "geometria valida",
    ),
    ("geo.geometry_diagnostics", "bounds_minx", "geometria vuota"),
    ("geo.geometry_diagnostics", "bounds_miny", "geometria vuota"),
    ("geo.geometry_diagnostics", "bounds_maxx", "geometria vuota"),
    ("geo.geometry_diagnostics", "bounds_maxy", "geometria vuota"),
    ("geo.overlay", "__left_index", "resto della destra"),
    ("geo.overlay", "__right_index", "resto della sinistra"),
];

/// Le colonne d'uscita nullable dove lo e' una colonna d'ingresso di nome
/// diverso: `(operazione, colonna d'uscita, colonna d'ingresso)`.
const NULL_DA_UNA_COLONNA_D_INGRESSO: &[(&str, &str, &str)] =
    &[("geo.from_wkt", "geometry", "wkt")];

/// Le geo allineate riga a riga su due tabelle: la riga in piu' della
/// sinistra ha una compagna nella destra (la prima riga ripetuta).
const ALLINEATE: [&str; 4] = [
    "geo.intersection",
    "geo.union",
    "geo.difference",
    "geo.symmetric_difference",
];

/// La tabella con ogni colonna senza null dichiarata non nullable.
fn senza_null_dichiarati(batch: &RecordBatch) -> RecordBatch {
    let campi: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .zip(batch.columns())
        .map(|(campo, colonna)| {
            campo
                .as_ref()
                .clone()
                .with_nullable(colonna.null_count() > 0)
        })
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            campi,
            batch.schema().metadata().clone(),
        )),
        batch.columns().to_vec(),
    )
    .expect("stessa tabella, campi non nullable")
}

/// La tabella con una riga in coda, tutta null (geometria compresa: una
/// chiave null fa un gruppo a se'); ogni campo nullable. `None` senza
/// geometria.
fn con_una_geometria_null(batch: &RecordBatch) -> Option<RecordBatch> {
    let schema = batch.schema();
    schema
        .fields()
        .iter()
        .position(|campo| e_geometria(campo))?;
    con_una_riga_in_coda(batch, true)
}

/// La tabella con una riga in coda: tutta null, o la prima ripetuta; ogni
/// campo nullable. `None` senza righe da ripetere.
fn con_una_riga_in_coda(batch: &RecordBatch, null: bool) -> Option<RecordBatch> {
    if !null && batch.num_rows() == 0 {
        return None;
    }
    let schema = batch.schema();
    let colonne = batch
        .columns()
        .iter()
        .map(|colonna| {
            let coda = if null {
                plenora_core::arrow::array::new_null_array(colonna.data_type(), 1)
            } else {
                colonna.slice(0, 1)
            };
            plenora_core::arrow::select::concat::concat(&[colonna.as_ref(), coda.as_ref()])
                .expect("concatenazione")
        })
        .collect();
    let campi: Vec<Field> = schema
        .fields()
        .iter()
        .map(|campo| campo.as_ref().clone().with_nullable(true))
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(campi, schema.metadata().clone())),
        colonne,
    )
    .ok()
}

/// Un rifiuto deve corrispondere a entrambi gli assi attesi.
#[track_caller]
fn verifica_rifiuto(
    errore: &PlenoraError,
    categoria: plenora_core::ErrorCategory,
    fase: plenora_core::ErrorPhase,
) {
    assert_eq!((errore.category(), errore.phase()), (categoria, fase));
}

fn esegui_su_tipizzato(
    scheda: &Scheda,
    ingressi: &[RecordBatch],
) -> Result<RecordBatch, PlenoraError> {
    let piano = Pipeline::from_json(&testo_piano(scheda)).expect("piano");
    let schemi: Vec<_> = scheda
        .esempio
        .ingressi
        .iter()
        .zip(ingressi)
        .map(|(spec, batch)| (spec.nome.as_str(), batch.schema()))
        .collect();
    let validata = piano.validate(&schemi)?;
    let tabelle = scheda
        .esempio
        .ingressi
        .iter()
        .zip(ingressi)
        .map(|(spec, batch)| (spec.nome.clone(), batch.clone()))
        .collect();
    validata
        .run(tabelle)
        .map(|esito| esito.outputs.into_iter().next().expect("un'uscita").1)
}

/// L'esempio della scheda su ingressi dati; `None` se il runner non esegue
/// l'operazione (verifica sul solo contratto).
fn esegui_su(scheda: &Scheda, ingressi: &[RecordBatch]) -> Option<Result<RecordBatch, String>> {
    let piano = Pipeline::from_json(&testo_piano(scheda)).expect("piano");
    let schemi: Vec<(&str, SchemaRef)> = scheda
        .esempio
        .ingressi
        .iter()
        .zip(ingressi)
        .map(|(spec, batch)| (spec.nome.as_str(), batch.schema()))
        .collect();
    let validata = match piano.validate(&schemi) {
        Ok(validata) => validata,
        Err(PlenoraError::Unsupported(_)) => return None,
        Err(errore) => return Some(Err(format!("validazione: {errore}"))),
    };
    let tabelle = scheda
        .esempio
        .ingressi
        .iter()
        .zip(ingressi)
        .map(|(spec, batch)| (spec.nome.clone(), batch.clone()))
        .collect();
    Some(
        validata
            .run(tabelle)
            .map(|esito| esito.outputs.into_iter().next().expect("un'uscita").1)
            .map_err(|errore| format!("esecuzione: {errore}")),
    )
}

/// Ogni geo dichiara nullable esattamente le colonne che possono essere
/// null, sugli esempi delle schede:
///
/// - con ingressi senza null (campi non nullable), una colonna d'uscita
///   nullable deve essere una colonna d'ingresso che ha null, o stare in
///   [`NULL_CON_INGRESSI_PRESENTI`] con il motivo;
/// - con una riga in piu' a geometria null (primo ingresso), una colonna
///   che diventa nullable per quella geometria deve emettere davvero un
///   null, e una colonna che resta non nullable non ne ha (Arrow lo
///   rifiuterebbe).
#[test]
#[allow(clippy::too_many_lines)] // I due confronti, un esempio per volta.
fn le_geo_dichiarano_null_solo_dove_ne_emettono() {
    prepara_ambiente();
    let (schede, _) = leggi_schede();
    let mut difetti = Vec::new();
    for scheda in schede
        .iter()
        .filter(|scheda| scheda.operazione.family == Family::Geo)
    {
        let op = scheda.operazione.id;
        let ingressi = scheda
            .esempio
            .ingressi
            .iter()
            .map(|spec| tabella_con_metadati(&spec.colonne, &spec.metadati))
            .collect::<Result<Vec<_>, _>>()
            .expect("ingressi dell'esempio");
        let senza: Vec<RecordBatch> = ingressi.iter().map(senza_null_dichiarati).collect();
        let Some(uscita) = esegui_su(scheda, &senza) else {
            continue;
        };
        let uscita = match uscita {
            Ok(uscita) => uscita,
            Err(errore) => {
                difetti.push(format!("{op}, ingressi senza null: {errore}"));
                continue;
            }
        };
        let nullable_in_ingresso = |nome: &str| {
            senza.iter().any(|batch| {
                batch
                    .schema()
                    .field_with_name(nome)
                    .is_ok_and(Field::is_nullable)
            })
        };
        let nullable_senza: BTreeSet<String> = uscita
            .schema()
            .fields()
            .iter()
            .filter(|campo| campo.is_nullable())
            .map(|campo| campo.name().clone())
            .collect();
        for nome in &nullable_senza {
            let ammessa = NULL_CON_INGRESSI_PRESENTI
                .iter()
                .any(|(o, colonna, _)| *o == op && colonna == nome);
            let derivata = NULL_DA_UNA_COLONNA_D_INGRESSO
                .iter()
                .any(|(o, colonna, sorgente)| {
                    *o == op && colonna == nome && nullable_in_ingresso(sorgente)
                });
            if !ammessa && !derivata && !nullable_in_ingresso(nome) {
                difetti.push(format!(
                    "{op}: `{nome}` dichiarata nullable con ingressi senza null"
                ));
            }
        }

        let mut con_null = ingressi.clone();
        let Some(prima) = ingressi.first().and_then(con_una_geometria_null) else {
            continue;
        };
        con_null[0] = prima;
        if ALLINEATE.contains(&op) {
            con_null[1] = con_una_riga_in_coda(&ingressi[1], false).expect("destra con righe");
        }
        let Some(uscita) = esegui_su(scheda, &con_null) else {
            continue;
        };
        let uscita = match uscita {
            Ok(uscita) => uscita,
            Err(errore) => {
                difetti.push(format!("{op}, una geometria null: {errore}"));
                continue;
            }
        };
        let ingressi_con_null: BTreeSet<String> = con_null
            .iter()
            .flat_map(|batch| {
                batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|campo| campo.name().clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        for (campo, colonna) in uscita.schema().fields().iter().zip(uscita.columns()) {
            if !campo.is_nullable() {
                if colonna.null_count() > 0 {
                    difetti.push(format!("{op}: `{}` non nullable con null", campo.name()));
                }
                continue;
            }
            // Nullable solo per la geometria null (con ingressi senza null
            // era non nullable) e non una colonna d'ingresso passata: la
            // riga null deve dare un null.
            let per_la_geometria = !nullable_senza.contains(campo.name())
                && (!ingressi_con_null.contains(campo.name())
                    || senza.iter().any(|batch| {
                        batch
                            .schema()
                            .field_with_name(campo.name())
                            .is_ok_and(e_geometria)
                    }));
            if per_la_geometria && colonna.null_count() == 0 {
                difetti.push(format!(
                    "{op}: `{}` dichiarata nullable per la geometria null, ma nessun null \
                     emesso",
                    campo.name()
                ));
            }
        }
    }
    assert!(difetti.is_empty(), "nullabilita':\n{}", difetti.join("\n"));
}

/// Le geo 1:1 che per una geometria vuota non hanno un risultato e, con la
/// colonna d'uscita non nullable (quella d'ingresso), rifiutano il passo
/// con un errore esplicito del contratto, come le schede dichiarano
/// («Righe»): la nullabilita' non si allarga per un caso limite.
const VUOTA_RIFIUTATA: &[&str] = &[
    "geo.line_interpolate_point",
    "geo.line_substring",
    "geo.point_on_surface",
];

/// La geometria vuota del tipo di una geometria (`None` per un punto, che
/// in WKB vuoto avrebbe coordinate NaN).
fn vuota_dello_stesso_tipo(geometria: &Geometry<f64>) -> Option<Geometry<f64>> {
    use geo::{GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Polygon};
    Some(match geometria {
        Geometry::LineString(_) => Geometry::LineString(LineString::new(Vec::new())),
        Geometry::Polygon(_) => {
            Geometry::Polygon(Polygon::new(LineString::new(Vec::new()), Vec::new()))
        }
        Geometry::MultiPoint(_) => Geometry::MultiPoint(MultiPoint::new(Vec::new())),
        Geometry::MultiLineString(_) => Geometry::MultiLineString(MultiLineString::new(Vec::new())),
        Geometry::MultiPolygon(_) => Geometry::MultiPolygon(MultiPolygon::new(Vec::new())),
        Geometry::GeometryCollection(_) => {
            Geometry::GeometryCollection(GeometryCollection::new_from(Vec::new()))
        }
        _ => return None,
    })
}

/// Il primo ingresso con la prima geometria sostituita dalla vuota del suo
/// tipo; `None` se non ce n'e' una.
fn con_una_geometria_vuota(batch: &RecordBatch) -> Option<RecordBatch> {
    let schema = batch.schema();
    let indice = schema
        .fields()
        .iter()
        .position(|campo| e_geometria(campo))?;
    let celle = batch.column(indice).as_binary::<i32>();
    if celle.is_empty() || celle.is_null(0) {
        return None;
    }
    let prima = plenora_kernels_geo::geometry_from_wkb(celle.value(0)).ok()?;
    let vuota =
        plenora_kernels_geo::arrow_adapter::encode_geometry(&vuota_dello_stesso_tipo(&prima)?)
            .ok()?;
    let mut builder = BinaryBuilder::new();
    builder.append_value(vuota);
    for riga in 1..celle.len() {
        if celle.is_null(riga) {
            builder.append_null();
        } else {
            builder.append_value(celle.value(riga));
        }
    }
    let mut colonne = batch.columns().to_vec();
    colonne[indice] = Arc::new(builder.finish());
    RecordBatch::try_new(schema, colonne).ok()
}

/// Casi limite della nullabilita' dichiarata: ingressi senza righe e una
/// geometria vuota al posto della prima. Le schede determinano l'unico
/// esito ammesso; i successi hanno schema, metadati e celle attesi indipendenti.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "Aiuto tabellare: confronta categoria e fase con latteso indipendente della scheda, poi verifica i valori dei successi."
)]
fn le_geo_rispettano_la_nullabilita_su_ingressi_vuoti_e_geometrie_vuote() {
    prepara_ambiente();
    let (schede, _) = leggi_schede();
    let mut difetti = Vec::new();
    for scheda in schede
        .iter()
        .filter(|scheda| scheda.operazione.family == Family::Geo)
    {
        let op = scheda.operazione.id;
        let ingressi = scheda
            .esempio
            .ingressi
            .iter()
            .map(|spec| tabella_con_metadati(&spec.colonne, &spec.metadati))
            .collect::<Result<Vec<_>, _>>()
            .expect("ingressi dell'esempio");
        let senza: Vec<RecordBatch> = ingressi.iter().map(senza_null_dichiarati).collect();
        let vuoti: Vec<RecordBatch> = senza.iter().map(|batch| batch.slice(0, 0)).collect();
        let mut varianti = vec![("ingressi vuoti", vuoti)];
        if let Some(prima) = senza.first().and_then(con_una_geometria_vuota) {
            let mut con_vuota = senza.clone();
            con_vuota[0] = prima;
            varianti.push(("una geometria vuota", con_vuota));
        }
        for (nome, variante) in varianti {
            let rifiuto = if nome == "ingressi vuoti" {
                // Voronoi richiede almeno due punti anche su una tabella vuota.
                (op == "geo.voronoi").then_some(plenora_core::ErrorCategory::InvalidPlan)
            } else {
                // Le schede dichiarano il rifiuto dell'assenza di centroide,
                // inviluppo o geometria non nullable, anche per WKB vuoto valido.
                (VUOTA_RIFIUTATA.contains(&op) || matches!(op, "geo.centroid" | "geo.envelope"))
                    .then_some(plenora_core::ErrorCategory::InvalidPlan)
            };
            let esito = esegui_su_tipizzato(scheda, &variante);
            let atteso = rifiuto.map(|c| (c, plenora_core::ErrorPhase::Validate));
            let effettivo = esito.as_ref().err().map(|e| (e.category(), e.phase()));
            if effettivo != atteso {
                difetti.push(format!(
                    "{op}, {nome}: atteso {atteso:?}, ottenuto {effettivo:?}"
                ));
                continue;
            }
            if let Ok(uscita) = esito {
                let attesa = attesa_geo_vuota(scheda, &senza, nome == "ingressi vuoti");
                if !stessi_valori(&attesa, &uscita) {
                    difetti.push(format!(
                        "{op}, {nome}: valori, tipi, nomi o numero di righe diversi"
                    ));
                }
                if attesa.schema() != uscita.schema() {
                    difetti.push(format!(
                        "{op}, {nome}: schema, nullabilita' o metadati diversi"
                    ));
                }
            }
        }
    }
    assert!(difetti.is_empty(), "nullabilita':\n{}", difetti.join("\n"));
}

// La sorgente degli esempi resta docs/schede, indicizzata esclusivamente dal
// CATALOG: una nuova operazione senza esempio fa fallire anche questa prova.
// Ogni colonna viene provata come geometria, oltre alla geometria passante:
// limitarsi ad aggiungere una colonna non eserciterebbe melt/diff/aggregate.
fn ingresso_geometrico(batch: &RecordBatch, posizione: usize, proiettato: bool) -> RecordBatch {
    let crs = if proiettato {
        "EPSG:32632"
    } else {
        "EPSG:4326"
    };
    let axis = if proiettato {
        "easting_northing"
    } else {
        "lat_lon"
    };
    let mut campi: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut colonne = batch.columns().to_vec();
    // Una sola geometria dichiarata: le geometrie originarie non selezionate
    // rimangono byte anonimi, ma non fanno parte dell'oracolo delle sorgenti.
    for campo in &mut campi {
        if plenora_core::contract::arrow_metadata::field_declares_wkb_geometry(campo) {
            *campo = Field::new(campo.name(), campo.data_type().clone(), campo.is_nullable());
        }
    }
    let nome = campi
        .get(posizione)
        .map_or("__geometria_oracolo", |f| f.name().as_str());
    let campo = geometry_output_field(nome, crs).unwrap();
    let mut metadata = campo.metadata().clone();
    metadata.insert("plenora.geometry.axis_order", axis);
    metadata.insert("plenora.geometry.crs_id", crs);
    metadata.insert("plenora.geometry.encoding", "wkb");
    metadata.insert("plenora.geometry.precision", "float64");
    let campo = campo.with_metadata(metadata);
    let mut builder = BinaryBuilder::new();
    for riga in 0..batch.num_rows() {
        // WKB scritto indipendentemente dal codec di produzione, XY asimmetriche.
        let offset = f64::from(u32::try_from(riga).unwrap());
        let (x, y): (f64, f64) = if proiettato {
            (500_123.0 + offset, 4_982_345.0)
        } else {
            (45.0 + offset / 100.0, 12.0)
        };
        let mut bytes = vec![1, 1, 0, 0, 0];
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        builder.append_value(bytes);
    }
    let array: ArrayRef = Arc::new(builder.finish());
    if posizione < campi.len() {
        campi[posizione] = campo;
        colonne[posizione] = array;
    } else {
        campi.push(campo);
        colonne.push(array);
    }
    let mut metadata = batch.schema().metadata().clone();
    metadata.insert("plenora.contract.version", "1");
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(campi, metadata)),
        colonne,
    )
    .unwrap()
}

fn byte_delle_celle(array: &dyn Array) -> Vec<&[u8]> {
    use plenora_core::arrow::array::{
        BinaryArray, BinaryViewArray, FixedSizeBinaryArray, LargeBinaryArray,
    };
    macro_rules! leggi {
        ($tipo:ty) => {
            if let Some(a) = array.as_any().downcast_ref::<$tipo>() {
                return a.iter().flatten().collect();
            }
        };
    }
    leggi!(BinaryArray);
    leggi!(LargeBinaryArray);
    leggi!(BinaryViewArray);
    leggi!(FixedSizeBinaryArray);
    // Anche un campo testuale puo' contenere gli stessi byte WKB: alcune
    // coordinate hanno una codifica composta soltanto da byte UTF-8 validi.
    macro_rules! leggi_testo {
        ($tipo:ty) => {
            if let Some(a) = array.as_any().downcast_ref::<$tipo>() {
                return a.iter().flatten().map(str::as_bytes).collect();
            }
        };
    }
    leggi_testo!(plenora_core::arrow::array::StringArray);
    leggi_testo!(plenora_core::arrow::array::LargeStringArray);
    leggi_testo!(plenora_core::arrow::array::StringViewArray);
    Vec::new()
}

// Non usa require_same_geometry_reference: l'oracolo confronta le tre
// dichiarazioni direttamente, anche quando l'uscita e' Binary anonimo.
fn verifica_byte_geometrici(ingressi: &[RecordBatch], uscita: &RecordBatch) -> bool {
    uscita
        .schema()
        .fields()
        .iter()
        .zip(uscita.columns())
        .all(|(campo, array)| verifica_campo_geometrico(ingressi, campo, array.as_ref()))
}

fn verifica_campo_geometrico(ingressi: &[RecordBatch], campo: &Field, array: &dyn Array) -> bool {
    let bytes = byte_delle_celle(array);
    for ingresso in ingressi {
        for (origine, valori) in ingresso.schema().fields().iter().zip(ingresso.columns()) {
            if !plenora_core::contract::arrow_metadata::field_declares_wkb_geometry(origine) {
                continue;
            }
            let originali = byte_delle_celle(valori.as_ref());
            if bytes.iter().any(|b| originali.contains(b)) {
                for chiave in [
                    "plenora.geometry.crs_id",
                    "plenora.geometry.axis_order",
                    "plenora.geometry.encoding",
                ] {
                    if campo.metadata().get(chiave) != origine.metadata().get(chiave) {
                        return false;
                    }
                }
            }
        }
    }

    // Cammina anche list, struct, map, union e dictionary: un contenitore
    // non rende innocua la perdita dei metadati del suo campo binario.
    let data = array.to_data();
    for (indice, figlio) in data.child_data().iter().enumerate() {
        let campo_figlio = match campo.data_type() {
            DataType::List(f)
            | DataType::LargeList(f)
            | DataType::FixedSizeList(f, _)
            | DataType::Map(f, _) => f.as_ref().clone(),
            DataType::Struct(fields) => fields[indice].as_ref().clone(),
            DataType::Union(fields, _) => fields.iter().nth(indice).unwrap().1.as_ref().clone(),
            DataType::Dictionary(_, _) => {
                Field::new(campo.name(), figlio.data_type().clone(), true)
                    .with_metadata(campo.metadata().clone())
            }
            _ => Field::new("figlio", figlio.data_type().clone(), true),
        };
        let array_figlio = plenora_core::arrow::array::make_array(figlio.clone());
        if !verifica_campo_geometrico(ingressi, &campo_figlio, array_figlio.as_ref()) {
            return false;
        }
    }
    true
}

#[test]
#[allow(clippy::too_many_lines)] // La matrice e le asserzioni restano visibili nello stesso test.
#[expect(
    clippy::disallowed_methods,
    reason = "Aiuto tabellare: confronta categoria e fase con latteso indipendente della scheda, poi verifica i valori dei successi."
)]
fn catalogo_conserva_i_riferimenti_dei_byte_geometrici() {
    prepara_ambiente();
    let mut violazioni = Vec::new();
    let mut casi = 0;
    for operazione in CATALOG {
        let scheda =
            leggi_scheda(operazione).expect("ogni operazione deve avere un esempio valido");
        esegui_esempio(&scheda)
            .unwrap_or_else(|_| panic!("{}: esempio di base non valido", operazione.id));
        let originali: Vec<_> = scheda
            .esempio
            .ingressi
            .iter()
            .map(|s| tabella_con_metadati(&s.colonne, &s.metadati).unwrap())
            .collect();
        assert_eq!(
            originali.len() > 1,
            operazione.arity != Arity::Unary,
            "{}: copertura arieta'",
            operazione.id
        );
        let larghezza = originali
            .iter()
            .map(RecordBatch::num_columns)
            .max()
            .unwrap();
        for posizione in 0..=larghezza {
            for proiettato in [false, true] {
                // Ogni ingresso viene provato anche divergente dagli altri.
                for divergente in 0..=if originali.len() > 1 {
                    originali.len() * 3
                } else {
                    0
                } {
                    let ingressi: Vec<_> = originali
                        .iter()
                        .enumerate()
                        .map(|(i, b)| {
                            let cambia = divergente > 0 && (divergente - 1) % originali.len() == i;
                            let variante = divergente.saturating_sub(1) / originali.len();
                            let mut batch = ingresso_geometrico(
                                b,
                                posizione.min(b.num_columns()),
                                proiettato ^ (cambia && variante == 0),
                            );
                            if cambia && variante > 0 {
                                let mut campi: Vec<Field> = batch
                                    .schema()
                                    .fields()
                                    .iter()
                                    .map(|f| f.as_ref().clone())
                                    .collect();
                                let campo = &mut campi[posizione.min(b.num_columns())];
                                let mut metadata = campo.metadata().clone();
                                if variante == 1 {
                                    metadata.insert(
                                        "plenora.geometry.axis_order",
                                        if proiettato {
                                            "northing_easting"
                                        } else {
                                            "lon_lat"
                                        },
                                    );
                                } else {
                                    metadata.insert("plenora.geometry.encoding", "ewkb");
                                }
                                *campo = campo.clone().with_metadata(metadata);
                                // Riferimenti diversi hanno byte sentinella distinti: la provenienza
                                // non si puo' dedurre da celle identiche di due sorgenti diverse.
                                let index = posizione.min(b.num_columns());
                                let array = batch
                                    .column(index)
                                    .as_any()
                                    .downcast_ref::<plenora_core::arrow::array::BinaryArray>()
                                    .unwrap();
                                let mut builder = BinaryBuilder::new();
                                for bytes in array.iter().flatten() {
                                    let mut bytes = bytes.to_vec();
                                    let y =
                                        f64::from_le_bytes(bytes[13..21].try_into().unwrap()) + 1.0;
                                    bytes[13..21].copy_from_slice(&y.to_le_bytes());
                                    builder.append_value(bytes);
                                }
                                let mut columns = batch.columns().to_vec();
                                columns[index] = Arc::new(builder.finish());
                                batch = RecordBatch::try_new(
                                    Arc::new(Schema::new_with_metadata(
                                        campi,
                                        batch.schema().metadata().clone(),
                                    )),
                                    columns,
                                )
                                .unwrap();
                            }
                            contract_from_arrow_schema(batch.schema(), resolve_crs).expect(
                                "fixture geometrica valida indipendentemente dall'operazione",
                            );
                            batch
                        })
                        .collect();
                    // Gli ingressi vuoti esercitano ADDED/DELETED senza conversione
                    // testuale dei valori cambiati; i riferimenti restano obbligatori.
                    for vuoto in 0..=if ingressi.len() > 1 {
                        ingressi.len()
                    } else {
                        0
                    } {
                        let ingressi: Vec<_> = ingressi
                            .iter()
                            .enumerate()
                            .map(|(i, b)| {
                                if vuoto == i + 1 {
                                    b.slice(0, 0)
                                } else {
                                    b.clone()
                                }
                            })
                            .collect();
                        let piano = Pipeline::from_json(&testo_piano(&scheda)).unwrap();
                        let schemi: Vec<_> = scheda
                            .esempio
                            .ingressi
                            .iter()
                            .zip(&ingressi)
                            .map(|(s, b)| (s.nome.as_str(), b.schema()))
                            .collect();
                        let tabelle = scheda
                            .esempio
                            .ingressi
                            .iter()
                            .zip(&ingressi)
                            .map(|(s, b)| (s.nome.clone(), b.clone()))
                            .collect();
                        let esito = piano.validate(&schemi).and_then(|v| v.run(tabelle));
                        casi += 1;
                        let atteso = rifiuto_byte_catalogo(
                            operazione.id,
                            posizione,
                            proiettato,
                            divergente,
                            vuoto,
                        );
                        let effettivo = esito.as_ref().err().map(|e| (e.category(), e.phase()));
                        if effettivo != atteso {
                            violazioni.push(format!("{}: posizione {posizione}, proiettato {proiettato}, divergente {divergente}, vuoto {vuoto}: atteso {atteso:?}, ottenuto {effettivo:?}", operazione.id));
                        }
                        if let Ok(esito) = esito {
                            if esito
                                .outputs
                                .iter()
                                .any(|(_, b)| !verifica_byte_geometrici(&ingressi, b))
                            {
                                violazioni.push(format!("{}: posizione {posizione}, proiettato {proiettato}, divergente {divergente}, vuoto {vuoto}: riferimenti persi", operazione.id));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        violazioni.is_empty(),
        "violazioni del catalogo: {}",
        violazioni.join("\n")
    );
    eprintln!(
        "catalogo: {} operazioni, {casi} casi geometrici",
        CATALOG.len()
    );
}

#[test]
fn oracolo_geometrico_rileva_byte_anonimi_e_metadati_mutati() {
    use plenora_core::arrow::array::{Int64Array, StructArray};
    let base =
        RecordBatch::try_from_iter(vec![("g", Arc::new(Int64Array::from(vec![1])) as ArrayRef)])
            .unwrap();
    for proiettato in [false, true] {
        let sorgente = ingresso_geometrico(&base, 0, proiettato);
        assert!(verifica_byte_geometrici(
            std::slice::from_ref(&sorgente),
            &sorgente
        ));
        for chiave in [
            "plenora.geometry.crs_id",
            "plenora.geometry.axis_order",
            "plenora.geometry.encoding",
        ] {
            let mut metadata = sorgente.schema().field(0).metadata().clone();
            metadata.remove(chiave);
            let campo = sorgente.schema().field(0).clone().with_metadata(metadata);
            let mutata = RecordBatch::try_new(
                Arc::new(Schema::new(vec![campo])),
                sorgente.columns().to_vec(),
            )
            .unwrap();
            assert!(!verifica_byte_geometrici(
                std::slice::from_ref(&sorgente),
                &mutata
            ));
        }
        let bytes = byte_delle_celle(sorgente.column(0).as_ref());
        if let Ok(testo) = std::str::from_utf8(bytes[0]) {
            let array: ArrayRef =
                Arc::new(plenora_core::arrow::array::StringArray::from(vec![testo]));
            let anonima = RecordBatch::try_from_iter(vec![("anonimo", array)]).unwrap();
            assert!(!verifica_byte_geometrici(
                std::slice::from_ref(&sorgente),
                &anonima
            ));
        }
        let varianti: Vec<ArrayRef> = vec![
            Arc::new(plenora_core::arrow::array::BinaryViewArray::from(
                bytes.clone(),
            )),
            Arc::new(
                plenora_core::arrow::array::FixedSizeBinaryArray::try_from_iter(bytes.into_iter())
                    .unwrap(),
            ),
        ];
        for array in varianti {
            let anonima = RecordBatch::try_from_iter(vec![("anonimo", array)]).unwrap();
            assert!(!verifica_byte_geometrici(
                std::slice::from_ref(&sorgente),
                &anonima
            ));
        }
        let campo = Field::new("anonimo", DataType::Binary, true);
        let anonima = RecordBatch::try_new(
            Arc::new(Schema::new(vec![campo.clone()])),
            sorgente.columns().to_vec(),
        )
        .unwrap();
        assert!(!verifica_byte_geometrici(
            std::slice::from_ref(&sorgente),
            &anonima
        ));
        let struttura = StructArray::new(vec![campo].into(), sorgente.columns().to_vec(), None);
        let annidata =
            RecordBatch::try_from_iter(vec![("contenitore", Arc::new(struttura) as ArrayRef)])
                .unwrap();
        assert!(!verifica_byte_geometrici(
            std::slice::from_ref(&sorgente),
            &annidata
        ));
    }
}

/// Oracolo esplicito, indipendente dagli analyzer: per ogni campo dell'esempio
/// indica [ingresso, nome sorgente], oppure null per un campo nuovo/ricostruito.
/// Le trasformazioni geometriche in place mantengono la colonna logica;
/// le fusioni verticali adottano il primo ramo che contiene la colonna.
#[test]
#[allow(clippy::too_many_lines)]
fn catalogo_identita_seguono_la_provenienza() {
    use plenora_core::contract::{DataContract, FieldId};
    use std::collections::BTreeMap;
    type Origini = BTreeMap<String, Option<(usize, String)>>;
    let oracle: BTreeMap<String, Origini> =
        serde_json::from_str(include_str!("riferimenti/identita_catalogo.json")).unwrap();
    assert_eq!(
        oracle.len(),
        CATALOG.len(),
        "ogni operazione richiede un oracolo"
    );
    prepara_ambiente();
    let mut errors = Vec::new();
    for op in CATALOG {
        let scheda = leggi_scheda(op).unwrap();
        let output_schema = tabella(&scheda.esempio.uscita.colonne).unwrap().schema();
        let mut variants = vec![None];
        for field in output_schema.fields() {
            if oracle[op.id].get(field.name()) == Some(&None) {
                for branch in 0..scheda.esempio.ingressi.len() {
                    variants.push(Some((branch, field.name().clone())));
                }
            }
        }
        for variant in variants {
            let mut allocator = FieldAllocator::default();
            let mut inputs: Vec<DataContract> = scheda
                .esempio
                .ingressi
                .iter()
                .map(|spec| {
                    let batch = tabella_con_metadati(&spec.colonne, &spec.metadati).unwrap();
                    let mut input =
                        contract_from_arrow_schema(batch.schema(), resolve_crs).unwrap();
                    for field in input.schema.fields() {
                        let id = allocator.alloc().unwrap();
                        input.field_ids.insert(field.name().clone(), id);
                        for geometry in &mut input.geometries {
                            if geometry.name == *field.name() {
                                geometry.field_id = id;
                                input.active_geometry = Some(id);
                            }
                        }
                    }
                    input.validate().unwrap();
                    input
                })
                .collect();
            if let Some((branch, name)) = &variant {
                let input = &mut inputs[*branch];
                if input
                    .schema
                    .fields()
                    .iter()
                    .all(|campo| campo.name() != name)
                {
                    let mut fields = input.schema.fields().to_vec();
                    fields.push(Arc::new(Field::new(
                        name,
                        output_schema
                            .field_with_name(name)
                            .unwrap()
                            .data_type()
                            .clone(),
                        true,
                    )));
                    input.schema = Arc::new(Schema::new_with_metadata(
                        fields,
                        input.schema.metadata().clone(),
                    ));
                    input
                        .field_ids
                        .insert(name.clone(), allocator.alloc().unwrap());
                }
            }
            // Avvelena anche i nomi d'uscita: nessun campo nuovo puo' adottarli.
            let mut estranei = Vec::new();
            for name in inputs
                .iter()
                .flat_map(|input| input.field_ids.keys())
                .chain(oracle[op.id].keys())
            {
                estranei.push(allocator.derive(name).unwrap());
            }
            let mut config: Value = serde_json::from_str(scheda.esempio.config.get()).unwrap();
            if op.id == "table.pivot" {
                config["mapping"] = serde_json::json!({"feb":"feb", "gen":"gen"});
            }
            let result = if op.family == Family::Table {
                plenora_kernels_table::analyze::analyze_table_contract(
                    op.id,
                    &inputs,
                    &config,
                    &mut allocator,
                    &plenora_kernels_table::Limits::default(),
                )
            } else {
                let crs = scheda
                    .esempio
                    .crs
                    .as_deref()
                    .map(|s| resolve_crs(s, "crs").unwrap());
                plenora_kernels_geo::analyze::analyze_geo_contract(
                    op.id,
                    &inputs,
                    &config,
                    crs.as_ref(),
                    &mut allocator,
                )
            };
            if op.id == "table.transpose" {
                assert!(matches!(result, Err(PlenoraError::Unsupported(_))));
                continue;
            }
            // Le op che appendono senza sovrascrivere rifiutano l'omonimo sul
            // ramo conservato. Un errore inatteso non conta come prova superata.
            let rejects_collision = variant.as_ref().is_some_and(|(branch, _)| {
                *branch == 0
                    && (op.id == "table.unnest"
                        || matches!(
                            op.id,
                            "geo.area"
                                | "geo.bounds_extractor"
                                | "geo.count_points_in_polygons"
                                | "geo.distance"
                                | "geo.explode"
                                | "geo.from_coords"
                                | "geo.from_wkt"
                                | "geo.length"
                                | "geo.nearest"
                                | "geo.perimeter"
                                | "geo.to_wkt"
                                | "geo.vertex_count"
                                | "geo.within"
                                | "geo.sjoin"
                                | "geo.hausdorff_distance"
                                | "geo.haversine_distance"
                                | "geo.geodesic_distance"
                                | "geo.geodesic_line_length"
                                | "geo.delaunay"
                                | "geo.split"
                                | "geo.frechet_distance"
                                | "geo.bearing"
                                | "geo.geodesic_area"
                                | "geo.geometry_diagnostics"
                                | "geo.geometry_accessors"
                                | "geo.line_locate_point"
                                | "geo.subdivide"
                                | "geo.cluster_dbscan"
                        )
                        || op.id.starts_with("geo.predicate_"))
            });
            if rejects_collision {
                // L'unnest rifiuta il piano; i produttori geo rifiutano lo schema.
                let categoria = if op.id == "table.unnest" {
                    plenora_core::ErrorCategory::InvalidPlan
                } else {
                    plenora_core::ErrorCategory::Schema
                };
                verifica_rifiuto(
                    &result.expect_err(op.id),
                    categoria,
                    plenora_core::ErrorPhase::Validate,
                );
                continue;
            }
            let output = result.unwrap_or_else(|e| panic!("{}: {variant:?}: {e}", op.id));
            let mut expected = oracle[op.id].clone();
            if let Some((branch, name)) = &variant {
                match op.id {
                    "table.align_schema" => {
                        expected.insert(name.clone(), Some((*branch, name.clone())));
                    }
                    "table.melt" => {
                        let source = expected.remove(name).unwrap();
                        expected.insert(format!("{name}_1"), source);
                    }
                    "table.fuzzy_join" => {
                        let suffix = if *branch == 0 { "L" } else { "R" };
                        expected.insert(format!("{name}_{suffix}"), Some((*branch, name.clone())));
                    }
                    _ => {}
                }
            }
            let old: Vec<FieldId> = inputs
                .iter()
                .flat_map(|i| i.field_ids.values().copied())
                .collect();
            assert_eq!(
                output.field_ids.len(),
                output.schema.fields().len(),
                "{}",
                op.id
            );
            assert_eq!(
                expected.len(),
                output.field_ids.len(),
                "{}: {variant:?}",
                op.id
            );
            for (name, source) in &expected {
                let actual = output.field_ids[name];
                assert!(
                    !estranei.contains(&actual),
                    "{}: ID estraneo per {name}",
                    op.id
                );
                let correct = match source {
                    Some((branch, origin)) => actual == inputs[*branch].field_ids[origin],
                    None => !old.contains(&actual),
                };
                if !correct {
                    errors.push(format!(
                        "{}: {name}, origine {source:?}, variante {variant:?}",
                        op.id
                    ));
                }
            }
            // La stessa mappa deve sopravvivere a una continuazione tabellare.
            let continued = plenora_kernels_table::analyze::analyze_table_contract(
                "table.limit",
                std::slice::from_ref(&output),
                &serde_json::json!({"n": 1}),
                &mut allocator,
                &plenora_kernels_table::Limits::default(),
            )
            .unwrap();
            assert_eq!(
                continued.field_ids, output.field_ids,
                "{}: continuazione",
                op.id
            );
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn diagnostica_omonima_non_resuscita_dopo_un_passo_tabellare() {
    let op = CATALOG
        .iter()
        .find(|op| op.id == "geo.geometry_diagnostics")
        .unwrap();
    let scheda = leggi_scheda(op).unwrap();
    let spec = &scheda.esempio.ingressi[0];
    let batch = tabella_con_metadati(&spec.colonne, &spec.metadati).unwrap();
    let input = contract_from_arrow_schema(batch.schema(), resolve_crs).unwrap();
    let mut allocator = FieldAllocator::default();
    let limits = plenora_kernels_table::Limits::default();
    let renamed = plenora_kernels_table::analyze::analyze_table_contract(
        "table.rename",
        &[input],
        &serde_json::json!({"renames":[{"old_name":"geometry", "new_name":"geometry_type"}]}),
        &mut allocator,
        &limits,
    )
    .unwrap();
    let geometry_id = renamed.field_ids["geometry_type"];
    let diagnostics = plenora_kernels_geo::analyze::analyze_geo_contract(
        op.id,
        &[renamed],
        &serde_json::json!({}),
        None,
        &mut allocator,
    )
    .unwrap();
    let continued = plenora_kernels_table::analyze::analyze_table_contract(
        "table.limit",
        &[diagnostics],
        &serde_json::json!({"n":1}),
        &mut allocator,
        &limits,
    )
    .unwrap();
    assert_ne!(continued.field_ids["geometry_type"], geometry_id);
    assert!(!continued.field_ids.values().any(|id| *id == geometry_id));
}

/// Una colonna privata di un ramo deve seguire quel ramo, anche quando la
/// dichiarazione geometrica esiste solo nel contratto Rust e non in Arrow.
#[test]
#[allow(clippy::too_many_lines)]
fn catalogo_colonne_presenti_in_un_solo_ingresso() {
    use plenora_core::catalog::Arity;
    use plenora_core::contract::arrow_schema::arrow_schema_from_contract;
    const NAME: &str = "__geometria_oracolo";
    prepara_ambiente();
    for op in CATALOG.iter().filter(|op| op.arity != Arity::Unary) {
        let scheda = leggi_scheda(op).unwrap();
        assert_eq!(scheda.esempio.ingressi.len(), 2, "{}", op.id);
        for branch in 0..2 {
            for geometry in [false, true] {
                for contract_only in [false, true] {
                    let mut allocator = FieldAllocator::default();
                    let mut inputs = Vec::new();
                    for (i, spec) in scheda.esempio.ingressi.iter().enumerate() {
                        let mut batch =
                            tabella_con_metadati(&spec.colonne, &spec.metadati).unwrap();
                        if i == branch && op.family == Family::Table {
                            batch = ingresso_geometrico(&batch, batch.num_columns(), false);
                        }
                        let mut input =
                            contract_from_arrow_schema(batch.schema(), resolve_crs).unwrap();
                        if op.family == Family::Geo && geometry {
                            // Ogni binaria geo richiede la geometria di entrambi i
                            // lati: l'assenza su un lato deve essere un errore.
                            if i != branch {
                                let name = input.geometries[0].name.clone();
                                let fields: Vec<_> = input
                                    .schema
                                    .fields()
                                    .iter()
                                    .filter(|f| f.name() != &name)
                                    .cloned()
                                    .collect();
                                input.schema = Arc::new(Schema::new_with_metadata(
                                    fields,
                                    input.schema.metadata().clone(),
                                ));
                                input.field_ids.remove(&name);
                                input.geometries.clear();
                                input.active_geometry = None;
                            }
                        } else if i == branch {
                            let mut fields = input.schema.fields().to_vec();
                            if op.family == Family::Geo {
                                fields.push(Arc::new(Field::new(NAME, DataType::Binary, false)));
                            }
                            let field = fields.iter_mut().find(|f| f.name() == NAME).unwrap();
                            let mut metadata = if geometry && !contract_only {
                                field.metadata().clone()
                            } else {
                                plenora_core::arrow::Metadata::default()
                            };
                            metadata.insert("audit.source", "private");
                            *field = Arc::new(
                                field
                                    .as_ref()
                                    .clone()
                                    .with_nullable(false)
                                    .with_metadata(metadata),
                            );
                            input.schema = Arc::new(Schema::new_with_metadata(
                                fields,
                                input.schema.metadata().clone(),
                            ));
                            if !geometry && op.family == Family::Table {
                                input.geometries.clear();
                                input.active_geometry = None;
                            }
                            for g in &mut input.geometries {
                                if g.name == NAME {
                                    g.nullable = false;
                                }
                            }
                        }
                        for field in input.schema.fields() {
                            let id = allocator.alloc().unwrap();
                            input.field_ids.insert(field.name().clone(), id);
                            for g in &mut input.geometries {
                                if g.name == *field.name() {
                                    g.field_id = id;
                                    input.active_geometry = Some(id);
                                }
                            }
                        }
                        input.validate().unwrap();
                        inputs.push(input);
                    }
                    let mut config: Value =
                        serde_json::from_str(scheda.esempio.config.get()).unwrap();
                    if op.id == "table.concat_by_name" {
                        config["strict"] = Value::Bool(false);
                    }
                    let result = if op.family == Family::Table {
                        plenora_kernels_table::analyze::analyze_table_contract(
                            op.id,
                            &inputs,
                            &config,
                            &mut allocator,
                            &plenora_kernels_table::Limits::default(),
                        )
                    } else {
                        plenora_kernels_geo::analyze::analyze_geo_contract(
                            op.id,
                            &inputs,
                            &config,
                            None,
                            &mut allocator,
                        )
                    };
                    let context = format!(
                        "{}: ramo {branch}, geometria {geometry}, solo contratto {contract_only}",
                        op.id
                    );
                    let reject = matches!(
                        op.id,
                        "table.concat"
                            | "table.union_distinct"
                            | "table.except"
                            | "table.intersect"
                    ) || (op.family == Family::Geo && geometry);
                    if reject {
                        // Le operazioni insiemistiche richiedono schemi della stessa
                        // larghezza; le binarie geo richiedono l'operando su ogni lato.
                        let categoria = if op.family == Family::Geo {
                            plenora_core::ErrorCategory::Schema
                        } else {
                            plenora_core::ErrorCategory::InvalidPlan
                        };
                        verifica_rifiuto(
                            &result.expect_err(&context),
                            categoria,
                            plenora_core::ErrorPhase::Validate,
                        );
                        continue;
                    }
                    let output = result.unwrap_or_else(|e| panic!("{context}: {e}"));
                    // Oracolo semantico chiuso: un'operazione nuova richiede una
                    // decisione esplicita, non eredita un successo per default.
                    let retained = match op.id {
                        "table.concat_by_name"
                        | "table.join"
                        | "table.cross_join"
                        | "table.asof_join"
                        | "table.fuzzy_join" => true,
                        "table.anti_join"
                        | "table.semi_join"
                        | "table.assert_foreign_key"
                        | "geo.clip"
                        | "geo.difference"
                        | "geo.intersection"
                        | "geo.union"
                        | "geo.symmetric_difference"
                        | "geo.within"
                        | "geo.count_points_in_polygons"
                        | "geo.sjoin"
                        | "geo.nearest" => branch == 0,
                        "table.reconcile" | "table.table_diff" | "geo.overlay" => false,
                        _ => panic!("operazione senza oracolo: {}", op.id),
                    };
                    let name = if matches!(op.id, "table.join" | "table.fuzzy_join") {
                        format!("{NAME}_{}", if branch == 0 { "L" } else { "R" })
                    } else {
                        NAME.to_owned()
                    };
                    assert_eq!(
                        output.schema.fields().iter().any(|f| f.name() == &name),
                        retained,
                        "{context}"
                    );
                    if !retained {
                        continue;
                    }
                    let field = output.schema.field_with_name(&name).unwrap();
                    let source = &inputs[branch];
                    assert_eq!(output.field_ids[&name], source.field_ids[NAME], "{context}");
                    assert_eq!(
                        field.metadata(),
                        source.schema.field_with_name(NAME).unwrap().metadata(),
                        "{context}"
                    );
                    if geometry {
                        let mut expected = source.geometries[0].clone();
                        expected.name = name.clone();
                        expected.nullable = field.is_nullable();
                        assert_eq!(
                            format!("{:?}", output.geometries),
                            format!("{:?}", vec![expected]),
                            "{context}"
                        );
                        if matches!(
                            op.id,
                            "table.concat_by_name"
                                | "table.join"
                                | "table.asof_join"
                                | "table.semi_join"
                                | "table.anti_join"
                                | "table.assert_foreign_key"
                        ) {
                            assert_eq!(
                                output.active_geometry,
                                if op.id == "table.concat_by_name" {
                                    None
                                } else {
                                    Some(source.field_ids[NAME])
                                },
                                "{context}"
                            );
                        }
                        let emitted = arrow_schema_from_contract(&output).unwrap();
                        let reread = contract_from_arrow_schema(emitted, resolve_crs).unwrap();
                        assert_eq!(reread.geometries.len(), 1, "{context}");
                        assert_eq!(
                            format!("{:?}", reread.geometries[0].crs),
                            format!("{:?}", source.geometries[0].crs),
                            "{context}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn concat_sorgente_effettiva_emissione_e_conflitti() {
    use plenora_core::contract::arrow_schema::arrow_schema_from_contract;
    use plenora_core::contract::{
        ContractProperty, NullPlacement, PropertyConfidence, PropertyScope, SortDirection,
        SortOrder,
    };
    use plenora_kernels_table::{analyze::analyze_table_contract, joins, Limits};
    let base = RecordBatch::try_from_iter(vec![(
        "id",
        Arc::new(plenora_core::arrow::array::Int64Array::from(vec![1])) as ArrayRef,
    )])
    .unwrap();
    let geo = ingresso_geometrico(&base, 1, false);
    let mut fields = geo.schema().fields().to_vec();
    let mut metadata = fields[1].metadata().clone();
    metadata.insert("plenora.geometry.types", "point");
    metadata.insert("plenora.geometry.types_declaration", "exact");
    fields[1] = Arc::new(
        fields[1]
            .as_ref()
            .clone()
            .with_nullable(false)
            .with_metadata(metadata),
    );
    let geo = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            geo.schema().metadata().clone(),
        )),
        geo.columns().to_vec(),
    )
    .unwrap();
    let limits = Limits::default();
    for branch in 0..3 {
        for repeated in [false, true] {
            let batches: Vec<_> = (0..3)
                .map(|i| {
                    if i == branch || (repeated && i == (branch + 1) % 3) {
                        geo.clone()
                    } else {
                        base.clone()
                    }
                })
                .collect();
            let mut alloc = FieldAllocator::default();
            let inputs: Vec<_> = batches
                .iter()
                .map(|b| {
                    let mut input = contract_from_arrow_schema(b.schema(), resolve_crs).unwrap();
                    for field in input.schema.fields() {
                        let id = alloc.alloc().unwrap();
                        input.field_ids.insert(field.name().clone(), id);
                        for g in &mut input.geometries {
                            if g.name == *field.name() {
                                g.field_id = id;
                                input.active_geometry = Some(id);
                            }
                        }
                    }
                    // Le chiavi rappresentate dal contratto non restano in Arrow.
                    // L'ordine assi vive solo nel campo, quindi resta li'.
                    input.schema = Arc::new(Schema::new(
                        input
                            .schema
                            .fields()
                            .iter()
                            .map(|f| {
                                Field::new(f.name(), f.data_type().clone(), f.is_nullable())
                                    .with_metadata(
                                        f.metadata()
                                            .iter()
                                            .filter(|(k, _)| {
                                                k.as_str() == "plenora.geometry.axis_order"
                                            })
                                            .map(|(k, v)| (k.clone(), v.clone()))
                                            .collect::<plenora_core::arrow::Metadata>(),
                                    )
                            })
                            .collect::<Vec<_>>(),
                    ));
                    let mut metadata = input.schema.metadata().clone();
                    metadata.insert(
                        format!("audit.source.{}", input.field_ids["id"].0),
                        "retained",
                    );
                    input.schema = Arc::new(Schema::new_with_metadata(
                        input.schema.fields().clone(),
                        metadata,
                    ));
                    input.properties.row_count = Some(ContractProperty::new(
                        PropertyConfidence::Proven(1),
                        PropertyScope::Dataset,
                    ));
                    input.properties.sorted_by = Some(ContractProperty::new(
                        PropertyConfidence::Proven(SortOrder {
                            keys: vec![input.field_ids["id"]],
                            direction: SortDirection::Ascending,
                            nulls: NullPlacement::Last,
                        }),
                        PropertyScope::Dataset,
                    ));
                    input
                })
                .collect();
            let output = analyze_table_contract(
                "table.concat_by_name",
                &inputs,
                &serde_json::json!({"strict":false}),
                &mut alloc,
                &limits,
            )
            .unwrap();
            let source = inputs.iter().find(|i| !i.geometries.is_empty()).unwrap();
            assert_eq!(output.geometries.len(), 1);
            assert_eq!(output.geometries[0].field_id, source.geometries[0].field_id);
            assert!(output.geometries[0].nullable);
            assert_eq!(output.geometries[0].types, source.geometries[0].types);
            assert_eq!(output.active_geometry, None);
            assert_eq!(
                output.properties.row_count,
                Some(ContractProperty::new(
                    PropertyConfidence::Proven(3),
                    PropertyScope::Dataset
                ))
            );
            assert!(output.properties.sorted_by.is_none());
            for input in &inputs {
                for (key, value) in input.schema.metadata() {
                    assert_eq!(output.schema.metadata().get(key), Some(value));
                }
            }
            let emitted = arrow_schema_from_contract(&output).unwrap();
            let direct = joins::concat_by_name(
                &batches.iter().collect::<Vec<_>>(),
                &joins::ConcatByName { strict: false },
                &limits,
            )
            .unwrap();
            let rebuilt = RecordBatch::try_new(emitted, direct.columns().to_vec()).unwrap();
            assert!(verifica_byte_geometrici(&batches, &rebuilt));
            assert_eq!(rebuilt.column(1).null_count(), if repeated { 1 } else { 2 });
            let continued = analyze_table_contract(
                "table.limit",
                &[output],
                &serde_json::json!({"n":1}),
                &mut alloc,
                &limits,
            )
            .unwrap();
            assert_eq!(continued.geometries.len(), 1);
            assert_eq!(
                continued.geometries[0].field_id,
                source.geometries[0].field_id
            );
        }
    }
    let plain = contract_from_arrow_schema(base.schema(), resolve_crs).unwrap();
    let geometry = contract_from_arrow_schema(geo.schema(), resolve_crs).unwrap();
    for divergence in 0..3 {
        let mut other = geometry.clone();
        // Il terzo ingresso non deve essere confrontato con il primo,
        // che non ha il campo, ma con il secondo che ne e' la sorgente.
        other.schema = Arc::new(Schema::new(
            other
                .schema
                .fields()
                .iter()
                .map(|f| {
                    Field::new(f.name(), f.data_type().clone(), f.is_nullable()).with_metadata(
                        f.metadata()
                            .iter()
                            .filter(|(k, _)| k.as_str() == "plenora.geometry.axis_order")
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<plenora_core::arrow::Metadata>(),
                    )
                })
                .collect::<Vec<_>>(),
        ));
        match divergence {
            0 => {
                other.geometries[0].encoding = Some(plenora_core::contract::GeometryEncoding::Ewkb);
            }
            1 => {
                other.geometries.clear();
                other.active_geometry = None;
            }
            _ => {
                let mut fields = other.schema.fields().to_vec();
                fields[1] = Arc::new(fields[1].as_ref().clone().with_name("other_geometry"));
                other.schema = Arc::new(Schema::new(fields));
                let id = other.field_ids.remove("__geometria_oracolo").unwrap();
                other.field_ids.insert("other_geometry".into(), id);
                other.geometries[0].name = "other_geometry".into();
            }
        }
        let result = analyze_table_contract(
            "table.concat_by_name",
            &[plain.clone(), geometry.clone(), other],
            &serde_json::json!({"strict":false}),
            &mut FieldAllocator::default(),
            &limits,
        );
        match divergence {
            0 => assert!(matches!(result, Err(PlenoraError::Schema(_)))),
            1 => assert!(matches!(result, Err(PlenoraError::Crs(_)))),
            _ => {
                let output = result.unwrap();
                assert_eq!(output.geometries.len(), 2);
                assert!(output.active_geometry.is_none());
            }
        }
    }
    // Allineare aggiunge un Binary anonimo: non puo' attribuirgli la
    // dichiarazione geometrica dell'altro ramo, neppure se e' tutto null.
    let aligned = analyze_table_contract("table.align_schema", &[plain], &serde_json::json!({"columns":[{"name":"id","type":"Int64"},{"name":"__geometria_oracolo","type":"Binary"}]}), &mut FieldAllocator::default(), &limits).unwrap();
    for op in ["table.union_distinct", "table.except", "table.intersect"] {
        for inputs in [
            [aligned.clone(), geometry.clone()],
            [geometry.clone(), aligned.clone()],
        ] {
            assert!(matches!(
                analyze_table_contract(
                    op,
                    &inputs,
                    &serde_json::json!({}),
                    &mut FieldAllocator::default(),
                    &limits
                ),
                Err(PlenoraError::Crs(_))
            ));
        }
    }
}

/// Ogni operazione tabellare del catalogo attraversa due e tre geometrie indipendenti.
#[test]
// La stessa matrice copre tutte le operazioni, senza accettare rifiuti generici.
#[allow(clippy::too_many_lines)]
fn catalogo_tabellare_multigeometria() {
    use plenora_core::arrow::array::BinaryArray;
    prepara_ambiente();
    for op in CATALOG.iter().filter(|op| op.family == Family::Table) {
        let mut scheda = leggi_scheda(op).unwrap();
        if op.id == "table.assert_schema" {
            let mut config: Value = serde_json::from_str(scheda.esempio.config.get()).unwrap();
            config["allow_extra"] = Value::Bool(true);
            scheda.esempio.config = serde_json::value::to_raw_value(&config).unwrap();
        }
        for count in [2, 3] {
            for primary in [None, Some(0), Some(count - 1)] {
                let inputs = scheda
                    .esempio
                    .ingressi
                    .iter()
                    .map(|spec| {
                        let batch = tabella_con_metadati(&spec.colonne, &spec.metadati).unwrap();
                        let mut fields = batch
                            .schema()
                            .fields()
                            .iter()
                            .map(|f| f.as_ref().clone())
                            .collect::<Vec<_>>();
                        let mut columns = batch.columns().to_vec();
                        for i in 0..count {
                            let mut field = geometry_output_field(
                                &format!("geom_{i}"),
                                ["EPSG:32632", "EPSG:3857", "EPSG:32633"][i],
                            )
                            .unwrap();
                            let mut md = field.metadata().clone();
                            md.insert("plenora.field_id", (1000 + i).to_string());
                            field = field.with_metadata(md);
                            fields.push(field);
                            let mut wkb = vec![1, 1, 0, 0, 0];
                            wkb.extend_from_slice(
                                &f64::from(u32::try_from(i).unwrap()).to_le_bytes(),
                            );
                            wkb.extend_from_slice(&0_f64.to_le_bytes());
                            let values = (0..batch.num_rows())
                                .map(|row| {
                                    if (row + i) % 3 == 0 {
                                        None
                                    } else {
                                        Some(wkb.as_slice())
                                    }
                                })
                                .collect::<Vec<_>>();
                            columns.push(Arc::new(BinaryArray::from(values)));
                        }
                        let mut md = batch.schema().metadata().clone();
                        md.insert("plenora.contract.version", "1");
                        if let Some(primary) = primary {
                            md.insert("plenora.primary_geometry", (1000 + primary).to_string());
                        }
                        RecordBatch::try_new(
                            Arc::new(Schema::new_with_metadata(fields, md)),
                            columns,
                        )
                        .unwrap()
                    })
                    .collect::<Vec<_>>();
                let context = format!("{} geometrie={count} primaria={primary:?}", op.id);
                if op.id == "table.transpose" {
                    // Una trasposizione fonde tutte le sorgenti: CRS diversi devono essere rifiutati.
                    let config = plenora_kernels_table::reshape::Transpose {
                        id_column: None,
                        output_columns: vec![],
                        type_policy: None,
                    };
                    let indices = (inputs[0].num_columns() - count..inputs[0].num_columns())
                        .collect::<Vec<_>>();
                    let input = inputs[0].project(&indices).unwrap();
                    let error = plenora_kernels_table::reshape::transpose(
                        &input,
                        &config,
                        &plenora_kernels_table::Limits::default(),
                    )
                    .expect_err(&context);
                    assert_eq!(
                        error.category(),
                        plenora_core::ErrorCategory::Crs,
                        "{context}"
                    );
                    assert_eq!(
                        error.phase(),
                        plenora_core::ErrorPhase::Validate,
                        "{context}"
                    );
                    continue;
                }
                let execute = || {
                    if op.id == "table.pivot" {
                        kernel_diretto(&scheda, &inputs[0])
                    } else {
                        esegui_su(&scheda, &inputs)
                            .unwrap_or_else(|| panic!("supporto mancante: {context}"))
                    }
                };
                let output = execute().unwrap_or_else(|error| panic!("{context}: {error}"));
                let repeated = execute().unwrap_or_else(|error| panic!("{context}: {error}"));
                assert_eq!(output.schema(), repeated.schema(), "{context}");
                assert_eq!(output.num_rows(), repeated.num_rows(), "{context}");
                let contract = contract_from_arrow_schema(output.schema(), resolve_crs)
                    .unwrap_or_else(|e| panic!("{context}: {e}"));
                let expected_count = match op.id {
                    "table.join" | "table.cross_join" | "table.asof_join" | "table.fuzzy_join" => {
                        2 * count
                    }
                    "table.aggregate"
                    | "table.melt"
                    | "table.pivot"
                    | "table.select_columns"
                    | "table.reconcile"
                    | "table.align_schema" => 0,
                    _ => count,
                };
                assert_eq!(
                    contract.geometries.len(),
                    expected_count,
                    "{context}: presenza delle secondarie"
                );
                if !USCITA_CASUALE.contains(&op.id) {
                    assert_eq!(output, repeated, "{context}: determinismo");
                }
                for geometry in &contract.geometries {
                    let index = output.schema().index_of(&geometry.name).unwrap();
                    assert_eq!(output.column(index), repeated.column(index), "{context}");
                    let source = inputs
                        .iter()
                        .flat_map(|b| b.schema().fields().to_vec())
                        .find(|f| {
                            geometry.name == *f.name()
                                || geometry.name.starts_with(&format!("{}_", f.name()))
                        })
                        .unwrap();
                    for (key, value) in source.metadata() {
                        if key != "plenora.field_id" {
                            assert_eq!(
                                output.schema().field(index).metadata().get(key),
                                Some(value),
                                "{context}: {key}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Il catalogo enumera tutti i kernel: l'operando conserva l'esito storico,
/// le geometrie passanti hanno CRS, assi e precisione indipendenti.
#[test]
#[allow(clippy::too_many_lines)] // Matrice completa e oracolo nello stesso punto.
fn catalogo_geo_multigeometria_operandi_e_passanti() {
    use plenora_core::contract::arrow_schema::primary_geometry_position;
    prepara_ambiente();
    for op in CATALOG.iter().filter(|op| op.family == Family::Geo) {
        let mut scheda = leggi_scheda(op).unwrap();
        let originali: Vec<_> = scheda
            .esempio
            .ingressi
            .iter()
            .map(|spec| tabella_con_metadati(&spec.colonne, &spec.metadati).unwrap())
            .collect();
        let config: Value = serde_json::from_str(scheda.esempio.config.get()).unwrap();
        let base = esegui_su(&scheda, &originali)
            .expect("kernel presente")
            .expect(op.id);
        let produttore = matches!(
            op.id,
            "geo.from_coords" | "geo.from_wkt" | "geo.generate_grid"
        );
        // Su una geometria il selettore esplicito riproduce il default storico.
        let mut esplicita = config.clone();
        for (side, batch) in originali.iter().enumerate() {
            if let Some(index) = primary_geometry_position(&batch.schema()).unwrap() {
                let key = if originali.len() == 2 {
                    if side == 0 {
                        "left_geometry"
                    } else {
                        "right_geometry"
                    }
                } else {
                    "input_geometry"
                };
                esplicita[key] = Value::String(batch.schema().field(index).name().clone());
            }
        }
        scheda.esempio.config = RawValue::from_string(esplicita.to_string()).unwrap();
        let esplicito = esegui_su(&scheda, &originali).unwrap().unwrap();
        assert_eq!(esplicito, base, "{} default storico e selettore", op.id);
        // Tipi invalidi e nomi non geometrici hanno categorie precise e non citano dati.
        let key = if originali.len() == 2 {
            "left_geometry"
        } else {
            "input_geometry"
        };
        for value in [
            serde_json::json!(null),
            serde_json::json!(7),
            serde_json::json!(""),
            serde_json::json!("SEGRETO_ASSENTE"),
        ] {
            let mut invalida = config.clone();
            invalida[key] = value.clone();
            scheda.esempio.config = RawValue::from_string(invalida.to_string()).unwrap();
            let piano = Pipeline::from_json(&testo_piano(&scheda)).unwrap();
            let schemi: Vec<_> = scheda
                .esempio
                .ingressi
                .iter()
                .zip(&originali)
                .map(|(s, b)| (s.nome.as_str(), b.schema()))
                .collect();
            let Err(errore) = piano.validate(&schemi) else {
                panic!("{}: selettore invalido accettato in validazione", op.id);
            };
            assert!(
                !errore.to_string().contains("SEGRETO"),
                "{}: {errore}",
                op.id
            );
            let categoria = if value == serde_json::json!("SEGRETO_ASSENTE") && !produttore {
                plenora_core::ErrorCategory::Schema
            } else {
                plenora_core::ErrorCategory::InvalidPlan
            };
            assert_eq!(errore.category(), categoria, "{} categoria", op.id);
            assert_eq!(
                errore.phase(),
                plenora_core::ErrorPhase::Validate,
                "{} fase",
                op.id
            );
        }

        let ridotto = matches!(
            op.id,
            "geo.generate_grid"
                | "geo.collect"
                | "geo.dissolve"
                | "geo.line_builder"
                | "geo.polygon_builder"
                | "geo.line_merge"
                | "geo.polygonize"
                | "geo.overlay"
                | "geo.coverage_validate"
                | "geo.shared_paths"
        );
        for numero in [2, 3] {
            // La primaria puo' precedere o seguire l'operando, oppure mancare.
            for primaria in [Some(0), Some(numero - 1), None] {
                let mut selezionata = config.clone();
                let ingressi: Vec<_> = originali
                    .iter()
                    .enumerate()
                    .map(|(lato, batch)| {
                        let mut campi: Vec<Field> = batch
                            .schema()
                            .fields()
                            .iter()
                            .map(|f| f.as_ref().clone())
                            .collect();
                        let mut colonne = batch.columns().to_vec();
                        let operando = primary_geometry_position(&batch.schema()).unwrap();
                        if let Some(index) = operando {
                            let key = if originali.len() == 2 {
                                if lato == 0 {
                                    "left_geometry"
                                } else {
                                    "right_geometry"
                                }
                            } else {
                                "input_geometry"
                            };
                            selezionata[key] = Value::String(campi[index].name().clone());
                        }
                        campi.push(Field::new("__lineage_test", DataType::UInt64, false));
                        colonne.push(Arc::new(plenora_core::arrow::array::UInt64Array::from(
                            (0..batch.num_rows())
                                .map(|row| u64::try_from(row).unwrap())
                                .collect::<Vec<_>>(),
                        )));
                        let aggiunte = numero - usize::from(operando.is_some());
                        for extra in 0..aggiunte {
                            let nome = format!("__passante_{extra}");
                            let crs = if extra == 0 { "EPSG:4326" } else { "EPSG:3857" };
                            let mut campo = geometry_output_field(&nome, crs).unwrap();
                            let mut metadata = campo.metadata().clone();
                            metadata.insert(
                                "plenora.geometry.axis_order",
                                if extra == 0 {
                                    "lat_lon"
                                } else {
                                    "northing_easting"
                                },
                            );
                            metadata.insert("plenora.geometry.precision", "float32");
                            metadata.insert("plenora.geometry.crs_id", crs);
                            metadata.insert("plenora.geometry.crs_resolution", "resolved");
                            metadata.insert(
                                "plenora.geometry.srid",
                                if extra == 0 { "4326" } else { "3857" },
                            );
                            metadata.insert("plenora.geometry.dimensions", "xy");
                            metadata.insert("plenora.geometry.encoding", "wkb");
                            metadata.insert("plenora.geometry.spatial_semantics", "geometry");
                            metadata.insert("plenora.geometry.types_declaration", "unresolved");
                            campo = campo.with_metadata(metadata);
                            campo = campo.with_nullable(true);
                            let mut builder = BinaryBuilder::new();
                            for row in 0..batch.num_rows() {
                                if row % 2 == 1 {
                                    builder.append_null();
                                    continue;
                                }
                                let mut bytes = vec![1, 1, 0, 0, 0];
                                bytes.extend_from_slice(
                                    &(45.0 + f64::from(u32::try_from(row).unwrap()) / 100.0)
                                        .to_le_bytes(),
                                );
                                bytes.extend_from_slice(&12.0_f64.to_le_bytes());
                                builder.append_value(&bytes);
                            }
                            // La seconda passante precede anche fisicamente l'operando.
                            if extra == 1 {
                                campi.insert(0, campo);
                                colonne.insert(0, Arc::new(builder.finish()));
                            } else {
                                campi.push(campo);
                                colonne.push(Arc::new(builder.finish()));
                            }
                        }
                        let mut geometrie = Vec::new();
                        for (index, campo) in campi.iter_mut().enumerate() {
                            let mut metadata = campo.metadata().clone();
                            metadata.insert("plenora.field_id", index.to_string());
                            *campo = campo.clone().with_metadata(metadata);
                            if plenora_core::contract::arrow_metadata::field_declares_wkb_geometry(
                                campo,
                            ) {
                                geometrie.push(index);
                            }
                        }
                        let mut metadata = batch.schema().metadata().clone();
                        metadata.insert("plenora.contract.version", "1");
                        metadata.insert(
                            "plenora.primary_geometry",
                            primaria.map_or_else(|| "none".into(), |p| geometrie[p].to_string()),
                        );
                        RecordBatch::try_new(
                            Arc::new(Schema::new_with_metadata(campi, metadata)),
                            colonne,
                        )
                        .unwrap()
                    })
                    .collect();
                scheda.esempio.config = RawValue::from_string(selezionata.to_string()).unwrap();
                let uscita = esegui_su(&scheda, &ingressi)
                    .expect("multigeometria supportata")
                    .unwrap_or_else(|e| panic!("{} n={numero} primaria={primaria:?}: {e}", op.id));
                assert_eq!(uscita.num_rows(), base.num_rows(), "{}", op.id);
                for (campo, valori) in base.schema().fields().iter().zip(base.columns()) {
                    let index = uscita.schema().index_of(campo.name()).unwrap();
                    assert_eq!(
                        uscita.column(index).to_data(),
                        valori.to_data(),
                        "{} {}",
                        op.id,
                        campo.name()
                    );
                    let mut attesi = campo.metadata().clone();
                    attesi.remove("plenora.field_id");
                    let mut effettivi = uscita.schema().field(index).metadata().clone();
                    effettivi.remove("plenora.field_id");
                    assert_eq!(effettivi, attesi, "{} {} metadati", op.id, campo.name());
                }
                for campo in ingressi[0]
                    .schema()
                    .fields()
                    .iter()
                    .filter(|f| f.name().starts_with("__passante_"))
                {
                    let index = uscita.schema().index_of(campo.name());
                    if ridotto {
                        assert!(
                            uscita
                                .schema()
                                .fields()
                                .iter()
                                .all(|f| f.name() != campo.name()),
                            "{} schema ridotto",
                            op.id
                        );
                        continue;
                    }
                    let index = index.unwrap_or_else(|_| panic!("{}: passante persa", op.id));
                    let mut attesi = campo.metadata().clone();
                    attesi.remove("plenora.field_id");
                    let mut effettivi = uscita.schema().field(index).metadata().clone();
                    effettivi.remove("plenora.field_id");
                    let effect = matrice_metadati::expected_effects(op, &ingressi, &uscita)
                        .into_iter()
                        .find(|(position, _)| *position == index)
                        .unwrap()
                        .1;
                    plenora_core::contract::geometry_metadata_policy::apply(&mut attesi, effect);
                    assert_eq!(effettivi, attesi, "{} passante metadati", op.id);
                    let originali = ingressi[0]
                        .column(ingressi[0].schema().index_of(campo.name()).unwrap())
                        .as_any()
                        .downcast_ref::<plenora_core::arrow::array::BinaryArray>()
                        .unwrap();
                    let effettivi = uscita
                        .column(index)
                        .as_any()
                        .downcast_ref::<plenora_core::arrow::array::BinaryArray>()
                        .unwrap();
                    let lineage = uscita
                        .column(uscita.schema().index_of("__lineage_test").unwrap())
                        .as_any()
                        .downcast_ref::<plenora_core::arrow::array::UInt64Array>()
                        .unwrap();
                    for row in 0..uscita.num_rows() {
                        let source = usize::try_from(lineage.value(row)).unwrap();
                        assert_eq!(
                            effettivi.is_null(row),
                            originali.is_null(source),
                            "{} passante null",
                            op.id
                        );
                        if !originali.is_null(source) {
                            assert_eq!(
                                effettivi.value(row),
                                originali.value(source),
                                "{} passante byte e lineage",
                                op.id
                            );
                        }
                    }
                }
                let primaria_ingresso = primary_geometry_position(&ingressi[0].schema())
                    .unwrap()
                    .map(|p| ingressi[0].schema().field(p).name().clone());
                let primaria_attesa = if op.id == "geo.generate_grid" {
                    Some("geometry".to_owned())
                } else if matches!(op.id, "geo.coverage_validate" | "geo.shared_paths") {
                    primaria_ingresso
                        .filter(|name| !name.starts_with("__passante_"))
                        .map(|_| "geometry".to_owned())
                } else {
                    primaria_ingresso
                        .filter(|name| uscita.schema().fields().iter().any(|f| f.name() == name))
                };
                let primaria_effettiva = primary_geometry_position(&uscita.schema())
                    .unwrap()
                    .map(|p| uscita.schema().field(p).name().clone());
                assert_eq!(primaria_effettiva, primaria_attesa, "{} primaria", op.id);
                if !produttore {
                    // Default sulla stessa colonna, anche fisicamente non iniziale.
                    let impliciti: Vec<_> = ingressi
                        .iter()
                        .enumerate()
                        .map(|(side, batch)| {
                            let key = if ingressi.len() == 2 {
                                if side == 0 {
                                    "left_geometry"
                                } else {
                                    "right_geometry"
                                }
                            } else {
                                "input_geometry"
                            };
                            let name = selezionata[key].as_str().unwrap();
                            let schema = batch.schema();
                            let field = schema.field_with_name(name).unwrap();
                            let mut metadata = schema.metadata().clone();
                            metadata.insert(
                                "plenora.primary_geometry",
                                field.metadata().get("plenora.field_id").unwrap().clone(),
                            );
                            RecordBatch::try_new(
                                Arc::new(Schema::new_with_metadata(
                                    schema.fields().clone(),
                                    metadata,
                                )),
                                batch.columns().to_vec(),
                            )
                            .unwrap()
                        })
                        .collect();
                    scheda.esempio.config = RawValue::from_string(config.to_string()).unwrap();
                    let implicito = esegui_su(&scheda, &impliciti)
                        .unwrap()
                        .unwrap_or_else(|e| panic!("{} default: {e}", op.id));
                    assert_eq!(implicito.columns(), uscita.columns(), "{} implicito", op.id);
                    // La passante con assi invertiti diventa un errore solo se consumata.
                    if plenora_kernels_geo::analyze::richiede_ordine_assi(op.id) {
                        let mut assi = selezionata.clone();
                        assi[if ingressi.len() == 2 {
                            "left_geometry"
                        } else {
                            "input_geometry"
                        }] = Value::String("__passante_0".into());
                        scheda.esempio.config = RawValue::from_string(assi.to_string()).unwrap();
                        let errore = esegui_su_tipizzato(&scheda, &ingressi).expect_err(op.id);
                        verifica_rifiuto(
                            &errore,
                            plenora_core::ErrorCategory::Crs,
                            plenora_core::ErrorPhase::Validate,
                        );
                    }
                }
                // Nessun selettore e nessuna primaria: rifiuto, mai la prima colonna.
                if !produttore && primaria.is_none() {
                    scheda.esempio.config = RawValue::from_string(config.to_string()).unwrap();
                    let errore = esegui_su_tipizzato(&scheda, &ingressi).expect_err(op.id);
                    verifica_rifiuto(
                        &errore,
                        plenora_core::ErrorCategory::Schema,
                        plenora_core::ErrorPhase::Validate,
                    );
                }
            }
        }
    }
}
#[path = "comune/matrice_metadati.rs"]
mod matrice_metadati;

// Ogni ruolo nominato dall'esempio viene isolato: una lista [a,b] diventa
// [a] e [b], così le fusioni non si fermano solo sul tipo eterogeneo.
fn ruoli_geometrici(config: &Value, nomi: &BTreeSet<String>) -> Vec<(Value, String)> {
    let mut varianti = Vec::new();
    match config {
        Value::String(nome) if nomi.contains(nome) => {
            varianti.push((config.clone(), nome.clone()));
        }
        Value::Array(lista) => {
            if lista
                .iter()
                .all(|v| v.as_str().is_some_and(|n| nomi.contains(n)))
            {
                for nome in lista.iter().filter_map(Value::as_str) {
                    varianti.push((serde_json::json!([nome]), nome.to_owned()));
                }
            } else {
                for (indice, valore) in lista.iter().enumerate() {
                    for (variante, nome) in ruoli_geometrici(valore, nomi) {
                        let mut copia = lista.clone();
                        copia[indice] = variante;
                        varianti.push((Value::Array(copia), nome));
                    }
                }
            }
        }
        Value::Object(mappa) => {
            for (chiave, valore) in mappa {
                for (variante, nome) in ruoli_geometrici(valore, nomi) {
                    let mut copia = mappa.clone();
                    copia.insert(chiave.clone(), variante);
                    varianti.push((Value::Object(copia), nome));
                }
            }
        }
        _ => {}
    }
    varianti
}

#[expect(
    clippy::disallowed_methods,
    reason = "Classificazione indipendente dei byte della fixture, non risultato del prodotto."
)]
fn geometria_per_ruolo(
    batch: &RecordBatch,
    nome: &str,
    utf8: bool,
    multipla: bool,
    stato: usize,
) -> RecordBatch {
    let Ok(posizione) = batch.schema().index_of(nome) else {
        return batch.clone();
    };
    let mut batch = ingresso_geometrico(batch, posizione, false);
    if multipla {
        // La seconda geometria conserva la prima, anziché sostituirla.
        let aggiunta = ingresso_geometrico(&batch, batch.num_columns(), false);
        let mut campi = aggiunta.schema().fields().to_vec();
        campi[posizione] = batch.schema().field(posizione).clone().into();
        batch = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                campi,
                batch.schema().metadata().clone(),
            )),
            aggiunta.columns().to_vec(),
        )
        .unwrap();
    }
    let mut colonne = batch.columns().to_vec();
    for (indice, campo) in batch.schema().fields().iter().enumerate() {
        if !plenora_core::contract::arrow_metadata::field_declares_wkb_geometry(campo) {
            continue;
        }
        let (x, y): (f64, f64) = if utf8 { (2.0, 4.0) } else { (12.5, 45.1) };
        let mut bytes = vec![1, 1, 0, 0, 0];
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        assert_eq!(std::str::from_utf8(&bytes).is_ok(), utf8);
        let mut builder = BinaryBuilder::new();
        for _ in 0..batch.num_rows() {
            if stato == 2 {
                builder.append_null();
            } else {
                builder.append_value(&bytes);
            }
        }
        colonne[indice] = Arc::new(builder.finish());
    }
    let batch = RecordBatch::try_new(batch.schema(), colonne).unwrap();
    if stato == 1 {
        batch.slice(0, 0)
    } else {
        batch
    }
}

fn esito_ruolo_geometrico(
    scheda: &Scheda,
    config: &Value,
    ingressi: &[RecordBatch],
) -> (bool, Option<plenora_core::error::ErrorCategory>) {
    let mut testo: Value = serde_json::from_str(&testo_piano(scheda)).unwrap();
    testo["steps"][0]["config"] = config.clone();
    let piano = Pipeline::from_json(&testo.to_string()).unwrap();
    let schemi: Vec<_> = scheda
        .esempio
        .ingressi
        .iter()
        .zip(ingressi)
        .map(|(s, b)| (s.nome.as_str(), b.schema()))
        .collect();
    let validato = match piano.validate(&schemi) {
        Ok(v) => v,
        Err(e) => {
            assert!(
                e.category() != plenora_core::ErrorCategory::Internal,
                "{}: validazione Internal",
                scheda.operazione.id
            );
            return (false, Some(e.category()));
        }
    };
    let atteso = validato.schema_uscita(USCITA).unwrap().clone();
    let contratto = arrow_schema_from_contract(validato.contratto(USCITA).unwrap()).unwrap();
    let tabelle = scheda
        .esempio
        .ingressi
        .iter()
        .zip(ingressi)
        .map(|(s, b)| (s.nome.clone(), b.clone()))
        .collect();
    match validato.run(tabelle) {
        Ok(esito) => {
            assert_eq!(esito.outputs.len(), 1);
            for (nome, batch) in esito.outputs {
                assert_eq!(nome, USCITA);
                assert_eq!(
                    batch.schema(),
                    atteso,
                    "{}: schema pubblicato",
                    scheda.operazione.id
                );
                assert_eq!(batch.num_columns(), contratto.fields().len());
                for (reale, inferito) in batch.schema().fields().iter().zip(contratto.fields()) {
                    let mut metadata = reale.metadata().clone();
                    metadata.remove("plenora.field_id");
                    let mut previsto = inferito.metadata().clone();
                    previsto.remove("plenora.field_id");
                    assert_eq!(
                        metadata, previsto,
                        "{}: metadati inferiti",
                        scheda.operazione.id
                    );
                }
                assert!(
                    verifica_byte_geometrici(ingressi, &batch),
                    "{}: byte senza riferimento",
                    scheda.operazione.id
                );
            }
            (true, None)
        }
        Err(e) => {
            assert!(
                e.category() != plenora_core::ErrorCategory::Internal,
                "{}: esecuzione Internal",
                scheda.operazione.id
            );
            (true, Some(e.category()))
        }
    }
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "Aiuto tabellare: confronta categoria e fase con latteso indipendente della scheda, poi verifica i valori dei successi."
)]
fn catalogo_geometria_in_ogni_ruolo_non_legge_wkb_come_testo() {
    prepara_ambiente();
    let mut casi = 0;
    let mut difetti = Vec::new();
    for operazione in CATALOG {
        let mut scheda = leggi_scheda(operazione).unwrap();
        let originali: Vec<_> = scheda
            .esempio
            .ingressi
            .iter()
            .map(|s| tabella_con_metadati(&s.colonne, &s.metadati).unwrap())
            .collect();
        let nomi = originali
            .iter()
            .flat_map(|b| {
                b.schema()
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut config: Value = serde_json::from_str(scheda.esempio.config.get()).unwrap();
        if operazione.id == "table.pivot" {
            config["mapping"] = serde_json::json!({"gen":"gen","feb":"feb"});
        }
        for (config, nome) in ruoli_geometrici(&config, &nomi) {
            for multipla in [false, true] {
                for stato in 0..3 {
                    let mut esiti = Vec::new();
                    for utf8 in [false, true] {
                        let ingressi: Vec<_> = originali
                            .iter()
                            .map(|b| geometria_per_ruolo(b, &nome, utf8, multipla, stato))
                            .collect();
                        scheda.esempio.config = RawValue::from_string(config.to_string()).unwrap();
                        let atteso = rifiuto_ruolo_catalogo(operazione.id, &nome, stato, multipla);
                        let esito = esegui_su_tipizzato(&scheda, &ingressi);
                        let effettivo = esito.as_ref().err().map(|e| (e.category(), e.phase()));
                        let scenario = format!(
                            "{}: ruolo {nome}, multipla {multipla}, stato {stato}, utf8 {utf8}",
                            operazione.id
                        );
                        if atteso != effettivo {
                            difetti.push(format!(
                                "{scenario}: atteso {atteso:?}, ottenuto {effettivo:?}"
                            ));
                        } else if let Ok(out) = &esito {
                            let previsto =
                                valori_ruolo_catalogo(&scheda, &config, &nome, stato, &ingressi);
                            if !stessi_valori(&previsto, out) || previsto.schema() != out.schema() {
                                difetti.push(format!(
                                    "{scenario}: valori o schema diversi dall'oracolo"
                                ));
                            }
                            if !verifica_byte_geometrici(&ingressi, out) {
                                difetti.push(format!("{scenario}: riferimenti persi"));
                            }
                        }
                        esiti.push(effettivo);
                        casi += 1;
                    }
                    assert_eq!(
                        esiti[0], esiti[1],
                        "{}: ruolo {nome}, multipla {multipla}, stato {stato}",
                        operazione.id
                    );
                }
            }
        }
    }
    assert!(casi > CATALOG.len());
    assert!(difetti.is_empty(), "ruoli:\n{}", difetti.join("\n"));
    eprintln!(
        "catalogo: {} operazioni, {casi} varianti di ruolo geometriche",
        CATALOG.len()
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Oracolo unico: ingresso, due ordini delle uscite e confronto delle identita.
fn catalogo_identita_nuove_non_correlano_uscite_diverse() {
    prepara_ambiente();
    let mut provate = 0;
    for operazione in CATALOG {
        let scheda = leggi_scheda(operazione).unwrap();
        let mut prossimo = 0_u64;
        let schemi: Vec<_> = scheda
            .esempio
            .ingressi
            .iter()
            .map(|s| {
                let batch = tabella_con_metadati(&s.colonne, &s.metadati).unwrap();
                let campi: Vec<_> = batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| {
                        let mut metadata = f.metadata().clone();
                        metadata.insert("plenora.field_id", prossimo.to_string());
                        prossimo += 1;
                        f.as_ref().clone().with_metadata(metadata)
                    })
                    .collect();
                let mut metadata = batch.schema().metadata().clone();
                metadata.insert("plenora.contract.version", "1");
                (
                    s.nome.as_str(),
                    Arc::new(Schema::new_with_metadata(campi, metadata)),
                )
            })
            .collect();
        let mut testo: Value = serde_json::from_str(&testo_piano(&scheda)).unwrap();
        let piano = Pipeline::from_json(&testo.to_string()).unwrap();
        // Sono i soli esempi con schema dipendente dai dati: transpose
        // sempre, pivot senza mapping come nella sua scheda. L'elenco e'
        // indipendente dall'esito della validazione e dal catalogo runtime.
        if matches!(operazione.id, "table.transpose" | "table.pivot") {
            let Err(errore) = piano.validate(&schemi) else {
                panic!("schema dipendente dai dati")
            };
            verifica_rifiuto(
                &errore,
                plenora_core::ErrorCategory::Unsupported,
                plenora_core::ErrorPhase::Validate,
            );
            continue;
        }
        let iniziale = piano.validate(&schemi).unwrap_or_else(|errore| {
            panic!(
                "{}: esempio deve essere validabile: {errore}",
                operazione.id
            )
        });
        verifica_origini_catalogo(
            operazione,
            &schemi,
            &testo["steps"][0]["config"],
            iniziale.crs_piano(),
        );
        let nomi: Vec<_> = iniziale
            .schema_uscita(USCITA)
            .unwrap()
            .fields()
            .iter()
            .map(|f| f.name())
            .collect();
        testo["steps"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "out": "copia", "op": "table.select_columns", "in": [USCITA], "config": {"columns": nomi}
            }));
        let mut uscite = vec![USCITA.to_owned(), "copia".to_owned()];
        uscite.extend(scheda.esempio.ingressi.iter().map(|s| s.nome.clone()));
        for inverso in [false, true] {
            if inverso {
                uscite.reverse();
            }
            testo["outputs"] = serde_json::json!(uscite);
            let piano = Pipeline::from_json(&testo.to_string()).unwrap();
            let validato = piano.validate(&schemi).unwrap();
            let ripetuto = piano.validate(&schemi).unwrap();
            let mut assegnati = BTreeSet::new();
            for uscita in &uscite {
                let schema = validato.schema_uscita(uscita).unwrap();
                assert_eq!(
                    Some(schema),
                    ripetuto.schema_uscita(uscita),
                    "{}: determinismo ID",
                    operazione.id
                );
                let contratto = validato.contratto(uscita).unwrap();
                for campo in schema.fields() {
                    let id: u64 = campo
                        .metadata()
                        .get("plenora.field_id")
                        .unwrap()
                        .parse()
                        .unwrap();
                    if id >= prossimo {
                        assert!(assegnati.insert(id), "{}: ID nuovo riusato", operazione.id);
                    } else {
                        // Un ID conservato deve gia' essere presente nel contratto,
                        // prima dell'assegnazione delle identita' pubblicate.
                        let fonte: u64 = contratto
                            .schema
                            .field_with_name(campo.name())
                            .unwrap()
                            .metadata()
                            .get("plenora.field_id")
                            .unwrap()
                            .parse()
                            .unwrap();
                        assert_eq!(
                            id, fonte,
                            "{}: ID conservato da un'altra origine",
                            operazione.id
                        );
                    }
                }
            }
            provate += 1;
        }
    }
    assert_eq!(provate, 2 * (CATALOG.len() - 2));
}

#[test]
fn geometrie_derivate_melt_pivot_coalesce_hanno_tipi_irrisolti() {
    prepara_ambiente();
    let casi = [
        (
            "table.melt",
            "q1",
            serde_json::json!({"id_columns":["id"],"value_columns":["q1"],"value_name":"valore"}),
            "valore",
        ),
        (
            "table.coalesce",
            "cellulare",
            serde_json::json!({"columns":["cellulare"],"output_column":"cellulare"}),
            "cellulare",
        ),
        (
            "table.coalesce",
            "cellulare",
            serde_json::json!({"columns":["cellulare"],"output_column":"nuova"}),
            "nuova",
        ),
        (
            "table.pivot",
            "vendite",
            serde_json::json!({"index_col":"negozio","pivot_col":"mese","value_col":"vendite","aggr_func":"first","mapping":{"gen":"gen"}}),
            "gen",
        ),
        (
            "table.pivot",
            "vendite",
            serde_json::json!({"index_col":"negozio","pivot_col":"mese","value_col":"vendite","aggr_func":"last","mapping":{"gen":"gen","feb":"feb"}}),
            "gen",
        ),
    ];
    for (op, nome, config, derivata) in casi {
        let scheda = leggi_scheda(find_operation(op).unwrap()).unwrap();
        let originale = tabella_con_metadati(
            &scheda.esempio.ingressi[0].colonne,
            &scheda.esempio.ingressi[0].metadati,
        )
        .unwrap();
        for multipla in [false, true] {
            for stato in 0..3 {
                for utf8 in [false, true] {
                    let batch = geometria_per_ruolo(&originale, nome, utf8, multipla, stato);
                    assert_eq!(
                        esito_ruolo_geometrico(&scheda, &config, std::slice::from_ref(&batch)),
                        (true, None),
                        "{op}: geometria derivata supportata"
                    );
                    let mut testo: Value = serde_json::from_str(&testo_piano(&scheda)).unwrap();
                    testo["steps"][0]["config"] = config.clone();
                    let piano = Pipeline::from_json(&testo.to_string()).unwrap();
                    let validato = piano
                        .validate(&[(&scheda.esempio.ingressi[0].nome, batch.schema())])
                        .unwrap();
                    let schema = validato.schema_uscita(USCITA).unwrap();
                    let campo = schema.field_with_name(derivata).unwrap();
                    assert_eq!(
                        campo
                            .metadata()
                            .get("plenora.geometry.types_declaration")
                            .map(String::as_str),
                        Some("unresolved")
                    );
                    assert!(!campo.metadata().contains_key("plenora.geometry.types"));
                }
            }
        }
    }
}

#[test]
fn transpose_diretto_conserva_geometrie_derivate() {
    let scheda = leggi_scheda(find_operation("table.coalesce").unwrap()).unwrap();
    let originale = tabella_con_metadati(
        &scheda.esempio.ingressi[0].colonne,
        &scheda.esempio.ingressi[0].metadati,
    )
    .unwrap();
    for utf8 in [false, true] {
        let batch = geometria_per_ruolo(&originale, "cellulare", utf8, true, 0);
        let batch = batch.project(&[1, 3]).unwrap();
        let config = serde_json::from_value(serde_json::json!({})).unwrap();
        let out = plenora_kernels_table::reshape::transpose(
            &batch,
            &config,
            &plenora_kernels_table::Limits::default(),
        )
        .unwrap();
        assert!(verifica_byte_geometrici(std::slice::from_ref(&batch), &out));
        for campo in out.schema().fields().iter().skip(1) {
            assert_eq!(
                campo
                    .metadata()
                    .get("plenora.geometry.types_declaration")
                    .map(String::as_str),
                Some("unresolved")
            );
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)] // La tabella dei consumatori testuali e' l'oracolo indipendente.
fn valori_geometrici_rifiutati_prima_delle_celle_anche_api_diretta() {
    prepara_ambiente();
    let casi = [
        ("table.pivot", "vendite"),
        ("table.pivot", "mese"),
        ("table.aggregate", "importo"),
        ("table.lookup", "stato"),
        ("table.mask_data", "email"),
        ("table.date_format", "data"),
        ("table.date_add", "scadenza"),
        ("table.date_diff", "inizio"),
        ("table.date_diff", "fine"),
        ("table.date_extract", "quando"),
        ("table.timezone_convert", "ora"),
        ("table.md5_hash", "nome"),
        ("table.sha256_hash", "codice"),
        ("table.hmac_sha256", "cliente"),
        ("table.stable_fingerprint", "nome"),
        ("table.flatten_json", "doc"),
        ("table.filter", "importo"),
        ("table.conditional", "classe"),
        ("table.formula", "prezzo"),
        ("table.expression", "importo"),
        ("table.validate_rules", "importo"),
    ];
    for (op, nome) in casi {
        let scheda = leggi_scheda(find_operation(op).unwrap()).unwrap();
        let originale = tabella_con_metadati(
            &scheda.esempio.ingressi[0].colonne,
            &scheda.esempio.ingressi[0].metadati,
        )
        .unwrap();
        let mut config: Value = serde_json::from_str(scheda.esempio.config.get()).unwrap();
        match op {
            "table.pivot" => {
                config["mapping"] = serde_json::json!({"gen":"gen"});
                config["aggr_func"] = serde_json::json!("concat");
            }
            "table.aggregate" => {
                config["aggregations"] =
                    serde_json::json!([{"column":"importo","function":"concat"}]);
            }
            "table.lookup" => {
                config.as_object_mut().unwrap().remove("output_column");
            }
            "table.stable_fingerprint" => {
                config.as_object_mut().unwrap().remove("columns");
            }
            "table.filter" => {
                config["operator"] = serde_json::json!("contains");
            }
            "table.formula" => {
                config["formula"] = serde_json::json!("prezzo");
            }
            "table.expression" => {
                config["expression"] = serde_json::json!({"kind":"column","name":"importo"});
            }
            "table.validate_rules" => {
                config["rules"] = serde_json::json!([{"name":"confronto","operator":"eq","column":"importo","value":"testo"}]);
            }
            _ => {}
        }
        for stato in 0..3 {
            for utf8 in [false, true] {
                let batch = geometria_per_ruolo(&originale, nome, utf8, true, stato);
                assert_eq!(
                    esito_ruolo_geometrico(&scheda, &config, std::slice::from_ref(&batch)),
                    (false, Some(plenora_core::ErrorCategory::Crs)),
                    "{op}: rifiuto in validazione"
                );
                let errore =
                    esegui_testo_diretto(op, &config, &batch).expect_err("rifiuto API diretta");
                assert_eq!(
                    errore.category(),
                    plenora_core::ErrorCategory::Crs,
                    "{op}: rifiuto API diretta"
                );
            }
        }
    }
}

fn esegui_testo_diretto(
    op: &str,
    config: &Value,
    batch: &RecordBatch,
) -> plenora_core::Result<RecordBatch> {
    use plenora_kernels_table::{
        aggregation, analysis, dates, expressions, filtering, formula, governance, quality,
        reshape, security, utility,
    };
    let limits = plenora_kernels_table::Limits::default();
    macro_rules! chiama {
        ($funzione:path) => {{
            let config = serde_json::from_value(config.clone()).unwrap();
            $funzione(batch, &config)
        }};
        ($funzione:path, limiti) => {{
            let config = serde_json::from_value(config.clone()).unwrap();
            $funzione(batch, &config, &limits)
        }};
    }
    match op {
        "table.melt" => chiama!(reshape::melt, limiti),
        "table.coalesce" => chiama!(quality::coalesce),
        "table.pivot" => chiama!(reshape::pivot, limiti),
        "table.aggregate" => chiama!(aggregation::aggregate_con_limiti, limiti),
        "table.lookup" => chiama!(analysis::lookup),
        "table.mask_data" => chiama!(security::mask_data_con_limiti, limiti),
        "table.date_format" => chiama!(dates::date_format),
        "table.date_add" => chiama!(dates::date_add),
        "table.date_diff" => chiama!(dates::date_diff),
        "table.date_extract" => chiama!(utility::date_extract),
        "table.timezone_convert" => chiama!(dates::timezone_convert),
        "table.md5_hash" => chiama!(security::md5_hash),
        "table.sha256_hash" => chiama!(security::sha256_hash),
        "table.hmac_sha256" => chiama!(security::hmac_sha256),
        "table.stable_fingerprint" => chiama!(security::stable_fingerprint),
        "table.flatten_json" => chiama!(analysis::flatten_json, limiti),
        "table.filter" => chiama!(filtering::filter),
        "table.conditional" => chiama!(filtering::conditional),
        "table.formula" => chiama!(formula::formula_con_effetti, limiti).map(|(batch, _)| batch),
        "table.expression" => {
            chiama!(expressions::expression_con_effetti, limiti).map(|(batch, _)| batch)
        }
        "table.validate_rules" => chiama!(governance::validate_rules),
        _ => panic!("consumatore testuale senza oracolo diretto"),
    }
}

#[test]
fn fusione_geometrica_con_crs_deciso_e_crs_del_piano() {
    use plenora_core::contract::ContractCrs;
    let scheda = leggi_scheda(find_operation("table.coalesce").unwrap()).unwrap();
    let originale = tabella_con_metadati(
        &scheda.esempio.ingressi[0].colonne,
        &scheda.esempio.ingressi[0].metadati,
    )
    .unwrap();
    let batch = geometria_per_ruolo(&originale, "cellulare", true, false, 0);
    let mut deciso = contract_from_arrow_schema(batch.schema(), resolve_crs).unwrap();
    deciso.geometries[0].crs =
        ContractCrs::ResolvedByDecision(resolve_crs("EPSG:4326", "crs").unwrap());
    let canonico = arrow_schema_from_contract(&deciso).unwrap();
    let batch = RecordBatch::try_new(canonico, batch.columns().to_vec()).unwrap();
    for (op, config, colonna) in [
        (
            "table.melt",
            serde_json::json!({"id_columns":["id"],"value_columns":["cellulare"],"value_name":"valore"}),
            "valore",
        ),
        (
            "table.coalesce",
            serde_json::json!({"columns":["cellulare"],"output_column":"valore"}),
            "valore",
        ),
        (
            "table.pivot",
            serde_json::json!({"index_col":"id","pivot_col":"fisso","value_col":"cellulare","aggr_func":"first","mapping":{"02 9":"valore"}}),
            "valore",
        ),
    ] {
        let contratto = plenora_kernels_table::analyze::analyze_table_contract(
            op,
            std::slice::from_ref(&deciso),
            &config,
            &mut FieldAllocator::default(),
            &plenora_kernels_table::Limits::default(),
        )
        .unwrap();
        let atteso = arrow_schema_from_contract(&contratto).unwrap();
        let uscita = esegui_testo_diretto(op, &config, &batch).unwrap();
        assert_eq!(
            uscita.schema().field_with_name(colonna).unwrap().metadata(),
            atteso.field_with_name(colonna).unwrap().metadata(),
            "{op}: decisione CRS"
        );
    }
    let scheda = leggi_scheda(find_operation("geo.from_coords").unwrap()).unwrap();
    let input = tabella_con_metadati(
        &scheda.esempio.ingressi[0].colonne,
        &scheda.esempio.ingressi[0].metadati,
    )
    .unwrap();
    let piano = Pipeline::from_json(&serde_json::json!({
        "version":1,"inputs":["t"],"crs":"EPSG:3857",
        "steps":[
            {"out":"g","op":"geo.from_coords","in":["t"],"config":{}},
            {"out":"m","op":"table.melt","in":["g"],"config":{"id_columns":["id"],"value_columns":["geometry"],"value_name":"valore"}}
        ],"outputs":["m"]
    }).to_string()).unwrap();
    let validato = piano.validate(&[("t", input.schema())]).unwrap();
    let atteso = validato.schema_uscita("m").unwrap().clone();
    let esito = validato
        .run(vec![("t".to_owned(), input)].into_iter().collect())
        .unwrap();
    assert_eq!(esito.outputs[0].1.schema(), atteso);
    let campo = atteso.field_with_name("valore").unwrap();
    assert_eq!(
        campo
            .metadata()
            .get("plenora.geometry.crs_id")
            .map(String::as_str),
        Some("EPSG:3857")
    );
    assert_eq!(
        campo
            .metadata()
            .get("plenora.geometry.types_declaration")
            .map(String::as_str),
        Some("unresolved")
    );
}

fn verifica_origini_catalogo(
    op: &OperationDescriptor,
    schemi: &[(&str, SchemaRef)],
    config: &Value,
    crs: Option<&plenora_core::crs::ResolvedCrs>,
) {
    use plenora_core::contract::FieldId;
    let mut ingressi = Vec::new();
    let mut originali = BTreeSet::new();
    let mut allocatore = FieldAllocator::default();
    for (_, schema) in schemi {
        let mut contratto = contract_from_arrow_schema(schema.clone(), resolve_crs).unwrap();
        let primaria = contratto
            .geometries
            .iter()
            .find(|g| Some(g.field_id) == contratto.active_geometry)
            .map(|g| g.name.clone());
        for campo in schema.fields() {
            let id = FieldId(
                campo
                    .metadata()
                    .get("plenora.field_id")
                    .unwrap()
                    .parse()
                    .unwrap(),
            );
            contratto.field_ids.insert(campo.name().clone(), id);
            originali.insert(id);
            allocatore
                .record_public_identity(id, campo.metadata().get("plenora.field_id").cloned());
        }
        for g in &mut contratto.geometries {
            g.field_id = contratto.field_ids[&g.name];
        }
        contratto.active_geometry = primaria.map(|nome| contratto.field_ids[&nome]);
        ingressi.push(contratto);
    }
    let uscita = if op.family == Family::Geo {
        plenora_kernels_geo::analyze::analyze_geo_contract(
            op.id,
            &ingressi,
            config,
            crs,
            &mut allocatore,
        )
    } else {
        plenora_kernels_table::analyze::analyze_table_contract(
            op.id,
            &ingressi,
            config,
            &mut allocatore,
            &plenora_kernels_table::Limits::default(),
        )
    }
    .unwrap();
    for campo in uscita.schema.fields() {
        let interno = uscita.field_ids[campo.name()];
        if !originali.contains(&interno) {
            // Una riscrittura sul posto puo' cambiare l'identita' interna
            // conservando esplicitamente quella pubblica della sorgente.
            let ereditata = allocatore.public_identity(interno).and_then(Option::as_ref);
            assert_eq!(
                campo.metadata().get("plenora.field_id"),
                ereditata,
                "{}: origine nuova senza lineage pubblica dichiarata",
                op.id
            );
            if let Some(id) = ereditata {
                assert!(originali.contains(&FieldId(id.parse().unwrap())));
            }
        }
    }
}

/// Attese delle fixture di riferimenti: le posizioni sono quelle delle schede,
/// prima di eseguire il piano. Un nuovo id richiede una classificazione esplicita.
// Bracci separati per la regola documentata (tipo, testo, fusione, CRS).
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn rifiuto_byte_catalogo(
    op: &str,
    posizione: usize,
    proiettato: bool,
    divergente: usize,
    vuoto: usize,
) -> Option<(plenora_core::ErrorCategory, plenora_core::ErrorPhase)> {
    use plenora_core::{ErrorCategory as C, ErrorPhase as F};
    let categoria = match op {
        "table.pivot" => Some(C::Unsupported),
        // Id geometrico oppure dati eterogenei da convertire in testo.
        "table.transpose" => Some(C::Crs),
        "table.add_row_number"
        | "table.assert_cardinality"
        | "table.assert_metadata"
        | "table.assert_not_null"
        | "table.assert_unique"
        | "table.cross_join"
        | "table.dedup_advanced"
        | "table.distinct"
        | "table.drop_columns"
        | "table.limit"
        | "table.rename"
        | "table.reorder_columns"
        | "table.sample"
        | "table.select_columns"
        | "table.sort"
        | "table.top_n"
        | "table.uuid_generator" => None,
        "table.assert_schema" => Some(C::InvalidPlan),
        "table.align_schema" => (posizione == 1).then_some(C::InvalidPlan),
        "table.aggregate" => (posizione == 1).then_some(C::InvalidPlan),
        "table.assert_range"
        | "table.bin"
        | "table.rolling_window"
        | "table.fill_na"
        | "table.assert_regex"
        | "table.filter"
        | "table.statistics"
        | "table.window_function" => {
            (posizione == usize::from(op != "table.bin")).then_some(C::InvalidPlan)
        }
        "table.lookup" | "table.type_cast" => (posizione == 1).then_some(C::Crs),
        "table.explode" | "table.unnest" => (posizione == 1).then_some(C::Crs),
        "table.coalesce" => matches!(posizione, 1 | 2).then_some(C::InvalidPlan),
        "table.melt" => matches!(posizione, 1 | 2).then_some(C::Crs),
        "table.concat_columns" => (posizione < 3).then_some(C::InvalidPlan),
        "table.replace"
        | "table.split_column"
        | "table.string_extract"
        | "table.string_length"
        | "table.string_pad"
        | "table.text_normalize" => (posizione == 0).then_some(C::InvalidPlan),
        "table.conditional"
        | "table.date_add"
        | "table.date_extract"
        | "table.date_format"
        | "table.expression"
        | "table.flatten_json"
        | "table.hmac_sha256"
        | "table.sha256_hash"
        | "table.timezone_convert" => (posizione == 0).then_some(C::Crs),
        "table.date_diff"
        | "table.formula"
        | "table.mask_data"
        | "table.md5_hash"
        | "table.stable_fingerprint" => (posizione < 2).then_some(C::Crs),
        "table.validate_rules" => (posizione < 2).then_some(C::InvalidPlan),
        "table.asof_join" => (posizione == 0).then_some(C::InvalidPlan),
        "table.fuzzy_join" => (posizione < 2).then_some(C::InvalidPlan),
        "table.join" | "table.anti_join" | "table.semi_join" | "table.assert_foreign_key" => {
            // Le chiavi occupano posizioni diverse: si fonderebbe WKB con uno scalare.
            if position_cle_incompatible(op, posizione) {
                Some(C::Crs)
            } else if op == "table.assert_foreign_key" && vuoto == 2 {
                return Some((C::DataMapping, F::Write));
            } else {
                None
            }
        }
        "table.reconcile" => (posizione == 0 && divergente != 0).then_some(C::Crs),
        "table.concat" | "table.except" | "table.intersect" | "table.union_distinct" => {
            (divergente != 0).then_some(C::Crs)
        }
        "table.concat_by_name" => {
            if posizione < 2 {
                Some(C::Crs)
            } else {
                (divergente != 0).then_some(C::Crs)
            }
        }
        "table.table_diff" => {
            if divergente != 0 {
                Some(C::Crs)
            } else {
                None
            }
        }
        "geo.from_coords" => matches!(posizione, 1 | 2).then_some(C::Schema),
        "geo.from_wkt" => (posizione == 1).then_some(C::Schema),
        "geo.generate_grid" => None,
        "geo.to_wkt"
        | "geo.vertex_count"
        | "geo.explode"
        | "geo.geometry_accessors"
        | "geo.geometry_diagnostics" => None,
        "geo.boundary" | "geo.bounds_extractor" => (!proiettato).then_some(C::Crs),
        "geo.collect" => (posizione == 0).then_some(C::InvalidPlan),
        "geo.affine_transform"
        | "geo.area"
        | "geo.bearing"
        | "geo.buffer"
        | "geo.centroid"
        | "geo.clean_topology"
        | "geo.clip"
        | "geo.cluster_dbscan"
        | "geo.concave_hull"
        | "geo.convex_hull"
        | "geo.count_points_in_polygons"
        | "geo.coverage_validate"
        | "geo.delaunay"
        | "geo.densify"
        | "geo.difference"
        | "geo.dissolve"
        | "geo.distance"
        | "geo.envelope"
        | "geo.frechet_distance"
        | "geo.geodesic_area"
        | "geo.geodesic_distance"
        | "geo.geodesic_line_length"
        | "geo.hausdorff_distance"
        | "geo.haversine_distance"
        | "geo.intersection"
        | "geo.length"
        | "geo.line_builder"
        | "geo.line_interpolate_point"
        | "geo.line_locate_point"
        | "geo.line_merge"
        | "geo.line_substring"
        | "geo.make_valid"
        | "geo.nearest"
        | "geo.overlay"
        | "geo.perimeter"
        | "geo.point_on_surface"
        | "geo.polygonize"
        | "geo.polygon_builder"
        | "geo.predicate_contains"
        | "geo.predicate_contains_properly"
        | "geo.predicate_covered_by"
        | "geo.predicate_covers"
        | "geo.predicate_crosses"
        | "geo.predicate_disjoint"
        | "geo.predicate_equals_topo"
        | "geo.predicate_intersects"
        | "geo.predicate_overlaps"
        | "geo.predicate_touches"
        | "geo.predicate_within"
        | "geo.reproject"
        | "geo.rotate"
        | "geo.scale"
        | "geo.shared_paths"
        | "geo.simplify"
        | "geo.sjoin"
        | "geo.snap"
        | "geo.snap_to_grid"
        | "geo.split"
        | "geo.subdivide"
        | "geo.symmetric_difference"
        | "geo.translate"
        | "geo.union"
        | "geo.voronoi"
        | "geo.within" => {
            // Gli assi lat_lon della fixture sono incompatibili con l'ordine GIS.
            // Sui due ingressi, ogni divergenza impedisce l'operazione comune.
            // La lama di split contiene una ordinata negativa, fuori dal dominio
            // UTM EPSG:32632 della fixture: Crs anche con assi proiettati corretti.
            if !proiettato
                || divergente != 0
                || matches!(
                    op,
                    "geo.bearing"
                        | "geo.geodesic_area"
                        | "geo.geodesic_distance"
                        | "geo.geodesic_line_length"
                        | "geo.haversine_distance"
                        | "geo.split"
                )
            {
                Some(C::Crs)
            } else if matches!(
                op,
                "geo.clean_topology"
                    | "geo.clip"
                    | "geo.coverage_validate"
                    | "geo.difference"
                    | "geo.dissolve"
                    | "geo.frechet_distance"
                    | "geo.intersection"
                    | "geo.line_merge"
                    | "geo.overlay"
                    | "geo.polygonize"
                    | "geo.shared_paths"
                    | "geo.symmetric_difference"
                    | "geo.union"
                    | "geo.convex_hull"
                    | "geo.concave_hull"
                    | "geo.polygon_builder"
                    | "geo.voronoi"
                    | "geo.line_substring"
                    | "geo.line_interpolate_point"
            ) && (op != "geo.clip" || vuoto != 2)
            {
                Some(C::InvalidPlan)
            } else {
                None
            }
        }
        _ => panic!("{op}: operazione senza attesa dei riferimenti"),
    };
    categoria.map(|c| (c, F::Validate))
}

fn position_cle_incompatible(op: &str, posizione: usize) -> bool {
    match op {
        "table.join" | "table.assert_foreign_key" | "table.anti_join" | "table.semi_join" => {
            posizione < 2
        }
        _ => panic!("operazione senza chiavi classificate"),
    }
}

/// Il controllo non stampa mai celle, neppure in caso di differenza.
fn stessi_valori(atteso: &RecordBatch, reale: &RecordBatch) -> bool {
    atteso.num_rows() == reale.num_rows()
        && atteso.num_columns() == reale.num_columns()
        && atteso
            .schema()
            .fields()
            .iter()
            .zip(reale.schema().fields())
            .all(|(a, b)| a.name() == b.name() && a.data_type() == b.data_type())
        && atteso.columns() == reale.columns()
}

#[allow(clippy::too_many_lines)]
fn rifiuto_ruolo_catalogo(
    op: &str,
    nome: &str,
    stato: usize,
    multipla: bool,
) -> Option<(plenora_core::ErrorCategory, plenora_core::ErrorPhase)> {
    use plenora_core::{ErrorCategory as C, ErrorPhase as F};
    let categoria = match op {
        "table.assert_not_null" if stato == 2 => return Some((C::DataMapping, F::Write)),
        "table.assert_unique" if stato == 0 => return Some((C::DataMapping, F::Write)),
        "table.add_row_number"
        | "table.coalesce"
        | "table.dedup_advanced"
        | "table.distinct"
        | "table.drop_columns"
        | "table.melt"
        | "table.reconcile"
        | "table.rename"
        | "table.reorder_columns"
        | "table.select_columns"
        | "table.sort"
        | "table.top_n"
        | "table.assert_not_null"
        | "table.assert_unique" => None,

        "table.aggregate" | "table.statistics" => (nome == "importo").then_some(C::InvalidPlan),
        "table.window_function" => (nome == "punti").then_some(C::InvalidPlan),
        "table.pivot" => match nome {
            "negozio" => None,
            "mese" => Some(C::Crs),
            "vendite" => Some(C::InvalidPlan),
            _ => panic!("ruolo pivot non classificato"),
        },
        "table.table_diff" => (stato != 1).then_some(C::InvalidPlan),
        "table.join"
        | "table.align_schema"
        | "table.asof_join"
        | "table.assert_range"
        | "table.assert_regex"
        | "table.assert_schema"
        | "table.bin"
        | "table.concat_columns"
        | "table.fill_na"
        | "table.filter"
        | "table.fuzzy_join"
        | "table.replace"
        | "table.rolling_window"
        | "table.split_column"
        | "table.string_extract"
        | "table.string_length"
        | "table.string_pad"
        | "table.text_normalize"
        | "table.validate_rules" => Some(C::InvalidPlan),
        "table.anti_join"
        | "table.semi_join"
        | "table.assert_foreign_key"
        | "table.conditional"
        | "table.date_add"
        | "table.date_diff"
        | "table.date_extract"
        | "table.date_format"
        | "table.explode"
        | "table.expression"
        | "table.flatten_json"
        | "table.hmac_sha256"
        | "table.lookup"
        | "table.mask_data"
        | "table.md5_hash"
        | "table.sha256_hash"
        | "table.stable_fingerprint"
        | "table.timezone_convert"
        | "table.transpose"
        | "table.type_cast"
        | "table.unnest" => Some(C::Crs),
        "geo.collect" => Some(if multipla { C::Schema } else { C::InvalidPlan }),
        "geo.from_wkt" => Some(C::Schema),
        _ => panic!("{op}: ruolo senza attesa indipendente"),
    };
    categoria.map(|c| (c, F::Validate))
}

/// Selezione di righe dell'oracolo: solo indici scritti nella prova, nessuna
/// decisione presa dall'ordinamento, dal join o dal raggruppamento di produzione.
fn righe_oracolo(array: &ArrayRef, righe: &[Option<u32>]) -> ArrayRef {
    let indici = plenora_core::arrow::array::UInt32Array::from(righe.to_vec());
    plenora_core::arrow::select::take::take(array.as_ref(), &indici, None).unwrap()
}

fn colonna_oracolo(nome: &str, tipo: &str, valori: Value) -> (Field, ArrayRef) {
    let mut spec = serde_json::json!({"nome":nome,"tipo":tipo});
    spec["valori"] = valori;
    let spec: ColonnaSpec = serde_json::from_value(spec).unwrap();
    colonna(&spec).unwrap()
}

#[allow(clippy::too_many_lines)]
fn valori_ruolo_catalogo(
    scheda: &Scheda,
    config: &Value,
    nome: &str,
    stato: usize,
    ingressi: &[RecordBatch],
) -> RecordBatch {
    use serde_json::json;
    let op = scheda.operazione.id;
    let input = &ingressi[0];
    let tutti: Vec<_> = (0..input.num_rows())
        .map(|i| Some(u32::try_from(i).unwrap()))
        .collect();
    let mut campi: Vec<(Field, ArrayRef)> = input
        .schema()
        .fields()
        .iter()
        .zip(input.columns())
        .map(|(f, a)| (f.as_ref().clone(), a.clone()))
        .collect();
    let seleziona = |righe: &[Option<u32>]| -> Vec<(Field, ArrayRef)> {
        input
            .schema()
            .fields()
            .iter()
            .zip(input.columns())
            .map(|(f, a)| {
                (
                    f.as_ref().clone().with_nullable(true),
                    righe_oracolo(a, righe),
                )
            })
            .collect()
    };
    match op {
        "table.assert_not_null" | "table.assert_unique" | "table.sort" => {}
        "table.add_row_number" => campi.push(colonna_oracolo(
            "n",
            "int64",
            json!((1..=input.num_rows()).collect::<Vec<_>>()),
        )),
        "table.coalesce" => {
            let i = input.schema().index_of(nome).unwrap();
            campi.push((
                input.schema().field(i).clone().with_name("telefono"),
                input.column(i).clone(),
            ));
        }
        "table.dedup_advanced" => {
            campi = seleziona(if stato == 1 {
                &[]
            } else if nome == "cliente" {
                &[Some(2)]
            } else {
                &[Some(0), Some(1)]
            });
        }
        "table.distinct" => {
            campi = seleziona(&tutti.last().copied().into_iter().collect::<Vec<_>>());
        }
        "table.drop_columns" => campi.retain(|(f, _)| f.name() != "note"),
        "table.select_columns" => campi.retain(|(f, _)| f.name() == nome),
        "table.top_n" => campi = seleziona(if stato == 1 { &[] } else { &[Some(0), Some(1)] }),
        "table.rename" => {
            for (f, _) in &mut campi {
                let nuovo = match f.name().as_str() {
                    "a" => "b",
                    "b" => "a",
                    "cod" => "codice",
                    _ => continue,
                };
                *f = f.clone().with_name(nuovo);
            }
        }
        "table.reorder_columns" => {
            campi.sort_by_key(|(f, _)| (f.name() != "id", f.name().to_lowercase()));
        }
        "table.aggregate" => {
            let righe = if stato == 1 { vec![] } else { vec![Some(0)] };
            campi = vec![
                (
                    input.schema().field(0).clone(),
                    righe_oracolo(input.column(0), &righe),
                ),
                colonna_oracolo(
                    "importo_sum",
                    "float64",
                    if stato == 1 { json!([]) } else { json!([17.5]) },
                ),
                colonna_oracolo(
                    "n",
                    "int64",
                    if stato == 1 { json!([]) } else { json!([3]) },
                ),
            ];
        }
        "table.statistics" => {
            for (suffix, value) in [("count", 3.0), ("mean", 35.0 / 3.0), ("max", 20.0)] {
                campi.push(colonna_oracolo(
                    &format!("importo_{suffix}"),
                    "float64",
                    json!(vec![value; input.num_rows()]),
                ));
            }
        }
        "table.window_function" => campi.push(colonna_oracolo(
            "punti_rank",
            "float64",
            if stato == 1 {
                json!([])
            } else {
                json!([2.5, 4.0, 1.0, 2.5])
            },
        )),
        "table.pivot" => {
            let righe = if stato == 1 { vec![] } else { vec![Some(0)] };
            campi = vec![
                (
                    input.schema().field(0).clone(),
                    righe_oracolo(input.column(0), &righe),
                ),
                colonna_oracolo(
                    "feb",
                    "float64",
                    if stato == 1 { json!([]) } else { json!([5.0]) },
                ),
                colonna_oracolo(
                    "gen",
                    "float64",
                    if stato == 1 { json!([]) } else { json!([18.0]) },
                ),
            ];
        }
        "table.melt" => {
            let valori: Vec<_> = config["value_columns"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let righe: Vec<_> = valori.iter().flat_map(|_| tutti.clone()).collect();
            let id = input.schema().index_of("id").unwrap();
            let variabili: Vec<_> = valori
                .iter()
                .flat_map(|n| vec![*n; input.num_rows()])
                .collect();
            let arrays: Vec<_> = valori
                .iter()
                .map(|n| input.column(input.schema().index_of(n).unwrap()).as_ref())
                .collect();
            let array = plenora_core::arrow::select::concat::concat(&arrays).unwrap();
            let campo = input
                .schema()
                .field_with_name(valori[0])
                .unwrap()
                .clone()
                .with_name("vendite");
            campi = vec![
                (
                    input.schema().field(id).clone(),
                    righe_oracolo(input.column(id), &righe),
                ),
                colonna_oracolo("trimestre", "utf8", json!(variabili)),
                (campo, array),
            ];
        }
        "table.reconcile" => {
            campi = vec![
                colonna_oracolo(
                    "metric",
                    "utf8",
                    json!([
                        "matched_rows",
                        "left_only_rows",
                        "right_only_rows",
                        "left_duplicate_rows",
                        "right_duplicate_rows"
                    ]),
                ),
                colonna_oracolo(
                    "value",
                    "uint64",
                    match stato {
                        0 | 2 => json!([4, 0, 0, 3, 3]),
                        1 => json!([0, 0, 0, 0, 0]),
                        _ => unreachable!(),
                    },
                ),
            ];
        }
        "table.table_diff" => {
            assert_eq!(stato, 1);
            campi.truncate(2);
            for n in ["_diff_status", "_diff_columns", "_diff_old_values"] {
                campi.push(colonna_oracolo(n, "utf8", json!([])));
            }
            if input.num_columns() == 3 {
                campi.insert(
                    2,
                    (input.schema().field(2).clone(), input.column(2).clone()),
                );
            }
        }
        _ => panic!("{op}: successo senza valori attesi"),
    }
    for (campo, _) in &mut campi {
        let non_nullable = matches!(
            (op, campo.name().as_str()),
            ("table.add_row_number" | "table.aggregate", "n")
                | ("table.table_diff", "_diff_status")
                | ("table.melt", "trimestre")
        ) || op == "table.reconcile";
        *campo = campo.clone().with_nullable(!non_nullable);
    }
    let (fields, arrays): (Vec<_>, Vec<_>) = campi.into_iter().unzip();
    let schema = schema_ruolo_catalogo(op, nome, input, fields);
    RecordBatch::try_new(Arc::new(schema), arrays).unwrap()
}

/// I valori non modificati provengono dall'esempio scritto; quelli influenzati
/// dall'assenza della prima geometria sono espliciti, non calcolati dal kernel.
#[allow(clippy::too_many_lines)]
fn attesa_geo_vuota(scheda: &Scheda, ingressi: &[RecordBatch], senza_righe: bool) -> RecordBatch {
    use serde_json::json;
    let op = scheda.operazione.id;
    let mut colonne = scheda.esempio.uscita.colonne.clone();
    let prima = |colonne: &mut Vec<ColonnaSpec>, nome: &str, valore: Value| {
        let c = colonne.iter_mut().find(|c| c.nome == nome).unwrap();
        c.valori[0] = RawValue::from_string(valore.to_string()).unwrap();
    };
    let tutte = |colonne: &mut Vec<ColonnaSpec>, nome: &str, valori: Value| {
        let c = colonne.iter_mut().find(|c| c.nome == nome).unwrap();
        c.valori = valori
            .as_array()
            .unwrap()
            .iter()
            .map(|v| RawValue::from_string(v.to_string()).unwrap())
            .collect();
    };
    let azzera = |colonne: &mut Vec<ColonnaSpec>| {
        for c in colonne {
            c.valori.clear();
        }
    };
    if senza_righe {
        if op != "geo.generate_grid" {
            azzera(&mut colonne);
            if matches!(
                op,
                "geo.dissolve" | "geo.line_builder" | "geo.polygon_builder"
            ) {
                tutte(&mut colonne, "geometry", json!([null]));
            }
        }
    } else {
        let spec = scheda.esempio.ingressi[0]
            .colonne
            .iter()
            .find(|c| c.tipo == "geometry")
            .unwrap();
        let wkt: String = serde_json::from_str(spec.valori[0].get()).unwrap();
        let tipo = wkt.split('(').next().unwrap().trim();
        let vuota = format!("{tipo} EMPTY");
        match op {
            "geo.affine_transform"
            | "geo.densify"
            | "geo.scale"
            | "geo.simplify"
            | "geo.snap"
            | "geo.make_valid" => prima(&mut colonne, "geometry", json!(vuota)),
            "geo.area"
            | "geo.length"
            | "geo.perimeter"
            | "geo.geodesic_area"
            | "geo.geodesic_line_length"
            | "geo.vertex_count"
            | "geo.count_points_in_polygons" => {
                prima(&mut colonne, "geometry", json!(vuota));
                let nome = colonne.last().unwrap().nome.clone();
                // La riduzione orientata di Karney restituisce -0 per l'anello
                // esterno vuoto; le altre misure hanno lo zero positivo.
                prima(
                    &mut colonne,
                    &nome,
                    if op == "geo.geodesic_area" {
                        json!(-0.0)
                    } else {
                        json!(0)
                    },
                );
            }
            "geo.bounds_extractor"
            | "geo.frechet_distance"
            | "geo.hausdorff_distance"
            | "geo.line_locate_point" => {
                prima(&mut colonne, "geometry", json!(vuota));
                for c in colonne.iter_mut().skip(2) {
                    c.valori[0] = RawValue::from_string("null".into()).unwrap();
                }
            }
            "geo.boundary" => prima(&mut colonne, "geometry", json!("MULTILINESTRING(EMPTY)")),
            "geo.buffer" => prima(&mut colonne, "geometry", json!("MULTIPOLYGON EMPTY")),
            "geo.convex_hull" | "geo.concave_hull" => {
                prima(&mut colonne, "geometry", json!("POLYGON EMPTY"));
            }
            "geo.clean_topology" => tutte(
                &mut colonne,
                "geometry",
                json!([
                    "POLYGON EMPTY",
                    "POLYGON((1 0,3 0,3 2,1 2,1 0))",
                    "MULTIPOLYGON(((0.5 1.5,0.5 0.5,1 0.5,1 1.5,0.5 1.5)))"
                ]),
            ),
            "geo.clip" | "geo.difference" | "geo.intersection" => {
                prima(&mut colonne, "geometry", Value::Null);
            }
            "geo.union" | "geo.symmetric_difference" => prima(
                &mut colonne,
                "geometry",
                json!("MULTIPOLYGON(((1 2,1 0,3 0,3 2,1 2)))"),
            ),
            "geo.dissolve" => prima(
                &mut colonne,
                "geometry",
                json!("MULTIPOLYGON(((1 2,1 0,3 0,3 2,1 2)),((10 11,10 10,11 10,11 11,10 11)))"),
            ),
            "geo.coverage_validate"
            | "geo.delaunay"
            | "geo.line_merge"
            | "geo.shared_paths"
            | "geo.split" => azzera(&mut colonne),
            "geo.subdivide" => {
                for c in &mut colonne {
                    c.valori.remove(1);
                }
                prima(&mut colonne, "geometry", json!("LINESTRING EMPTY"));
            }
            "geo.explode" => {
                // Le due parti della prima geometria spariscono; la seconda resta.
                for c in &mut colonne {
                    c.valori = c.valori[2..].to_vec();
                }
            }
            "geo.geometry_accessors" => {
                prima(&mut colonne, "geometry", json!(vuota));
                prima(&mut colonne, "start_point", Value::Null);
                prima(&mut colonne, "is_closed", json!(true));
            }
            "geo.geometry_diagnostics" => {
                prima(&mut colonne, "coordinate_count", json!(0));
                prima(&mut colonne, "is_empty", json!(true));
                for n in ["bounds_minx", "bounds_miny", "bounds_maxx", "bounds_maxy"] {
                    prima(&mut colonne, n, Value::Null);
                }
            }
            "geo.predicate_contains"
            | "geo.predicate_contains_properly"
            | "geo.predicate_covers"
            | "geo.predicate_crosses"
            | "geo.predicate_equals_topo"
            | "geo.predicate_overlaps" => {
                prima(&mut colonne, "geometry", json!(vuota));
                prima(&mut colonne, op.strip_prefix("geo.").unwrap(), json!(false));
            }
            "geo.overlay" => {
                tutte(
                    &mut colonne,
                    "geometry",
                    json!(["MULTIPOLYGON(((1 2,1 0,3 0,3 2,1 2)))"]),
                );
                tutte(&mut colonne, "__left_index", json!([null]));
                tutte(&mut colonne, "__right_index", json!([0]));
            }
            "geo.polygonize" => {
                tutte(
                    &mut colonne,
                    "geometry",
                    json!([
                        "LINESTRING(0 0,0 10,10 10)",
                        "LINESTRING(10 0,10 10)",
                        "LINESTRING(10 10,15 10)"
                    ]),
                );
                tutte(
                    &mut colonne,
                    "__class",
                    json!(["dangle", "dangle", "dangle"]),
                );
            }
            _ => panic!("{op}: geometria vuota senza valori attesi"),
        }
    }
    let valori = tabella(&colonne).unwrap();
    let campi: Vec<_> = valori
        .schema()
        .fields()
        .iter()
        .map(|campo| {
            let mut campo = campo.as_ref().clone();
            let nullable = nullabilita_geo_vuota(op, campo.name(), &ingressi[0].schema());
            campo = campo.with_nullable(nullable);
            campo
        })
        .collect();
    let schema = schema_geo_vuota(op, ingressi, campi);
    RecordBatch::try_new(Arc::new(schema), valori.columns().to_vec()).unwrap()
}

fn nullabilita_geo_vuota(op: &str, nome: &str, ingresso: &Schema) -> bool {
    if matches!(
        op,
        "geo.coverage_validate"
            | "geo.shared_paths"
            | "geo.generate_grid"
            | "geo.polygonize"
            | "geo.line_merge"
    ) {
        return false;
    }
    if matches!(nome, "__parent_index" | "__right_index") && op != "geo.overlay" {
        return false;
    }
    if op == "geo.overlay" {
        return nome != "geometry";
    }
    if nome == "geometry" {
        if matches!(
            op,
            "geo.dissolve"
                | "geo.line_builder"
                | "geo.polygon_builder"
                | "geo.clip"
                | "geo.intersection"
                | "geo.union"
                | "geo.difference"
                | "geo.symmetric_difference"
                | "geo.clean_topology"
        ) {
            return true;
        }
        if matches!(
            op,
            "geo.delaunay"
                | "geo.explode"
                | "geo.nearest"
                | "geo.sjoin"
                | "geo.split"
                | "geo.from_coords"
        ) {
            return false;
        }
        if op == "geo.from_wkt" {
            return ingresso.field_with_name("wkt").unwrap().is_nullable();
        }
    }
    if let Ok(campo) = ingresso.field_with_name(nome) {
        return campo.is_nullable();
    }
    if matches!(
        op,
        "geo.bounds_extractor"
            | "geo.distance"
            | "geo.frechet_distance"
            | "geo.hausdorff_distance"
            | "geo.line_locate_point"
            | "geo.cluster_dbscan"
    ) || matches!(
        nome,
        "start_point"
            | "validity_reason"
            | "bounds_minx"
            | "bounds_miny"
            | "bounds_maxx"
            | "bounds_maxy"
    ) {
        return true;
    }
    if op == "geo.nearest" {
        return false;
    }
    ingresso.field_with_name("geometry").unwrap().is_nullable()
}

/// Politica delle schede: elenco chiuso anche per le trasformazioni che non
/// cambiano la dichiarazione. Non consulta l'analisi o le policy di produzione.
#[allow(clippy::too_many_lines)]
fn tipi_geo_vuota(op: &str) -> (&'static str, Option<&'static str>) {
    match op {
        "geo.boundary" => ("exact",Some("MultiPoint,MultiLineString,GeometryCollection")),
        "geo.buffer" | "geo.clip" | "geo.difference" | "geo.dissolve" | "geo.intersection"
        | "geo.symmetric_difference" | "geo.union" => ("exact",Some("MultiPolygon")),
        "geo.centroid" | "geo.line_interpolate_point" | "geo.point_on_surface" => ("exact",Some("Point")),
        "geo.clean_topology" | "geo.overlay" => ("exact",Some("Polygon,MultiPolygon")),
        "geo.concave_hull" | "geo.convex_hull" | "geo.delaunay" | "geo.generate_grid" | "geo.voronoi" => ("exact",Some("Polygon")),
        "geo.envelope" => ("exact",Some("Point,LineString,Polygon")),
        "geo.line_merge" => ("exact",Some("LineString")),
        "geo.line_substring" => ("exact",Some("Point,LineString")),
        "geo.polygonize" | "geo.split" => ("exact",Some("LineString,Polygon")),
        "geo.make_valid" => ("mixed",None),
        "geo.from_wkt" => ("mixed",Some("Point,LineString,Polygon,MultiPoint,MultiLineString,MultiPolygon,GeometryCollection")),
        "geo.affine_transform" | "geo.area" | "geo.bearing" | "geo.bounds_extractor"
        | "geo.cluster_dbscan" | "geo.collect" | "geo.count_points_in_polygons"
        | "geo.coverage_validate" | "geo.densify" | "geo.distance" | "geo.explode"
        | "geo.frechet_distance" | "geo.from_coords" | "geo.geodesic_area"
        | "geo.geodesic_distance" | "geo.geodesic_line_length" | "geo.geometry_accessors"
        | "geo.geometry_diagnostics" | "geo.hausdorff_distance" | "geo.haversine_distance"
        | "geo.length" | "geo.line_builder" | "geo.line_locate_point" | "geo.nearest"
        | "geo.perimeter" | "geo.polygon_builder" | "geo.predicate_contains"
        | "geo.predicate_contains_properly" | "geo.predicate_covered_by" | "geo.predicate_covers"
        | "geo.predicate_crosses" | "geo.predicate_disjoint" | "geo.predicate_equals_topo"
        | "geo.predicate_intersects" | "geo.predicate_overlaps" | "geo.predicate_touches"
        | "geo.predicate_within" | "geo.reproject" | "geo.rotate" | "geo.scale"
        | "geo.shared_paths" | "geo.simplify" | "geo.sjoin" | "geo.snap" | "geo.snap_to_grid"
        | "geo.subdivide" | "geo.to_wkt" | "geo.translate" | "geo.vertex_count" | "geo.within" => ("unresolved",None),
        _ => panic!("{op}: schema geografico senza oracolo"),
    }
}

fn schema_geo_vuota(op: &str, ingressi: &[RecordBatch], mut campi: Vec<Field>) -> Schema {
    let (dichiarazione, tipi) = tipi_geo_vuota(op);
    let mut metadata = ingressi[0].schema().metadata().clone();
    metadata.insert("plenora.contract.version", "1");
    // Una sola geometria primaria (o nessuna geometria): chiave omessa,
    // come il profilo di emissione in docs/metadati-arrow.md.
    for (i, campo) in campi.iter_mut().enumerate() {
        let mut md = campo.metadata().clone();
        md.insert("plenora.field_id", i.to_string());
        if e_geometria(campo) {
            let geo: Value = serde_json::from_str(md.get("geo").unwrap()).unwrap();
            let crs = geo["crs"].as_str().unwrap();
            md.insert("plenora.geometry.crs_id", crs);
            if op == "geo.reproject" {
                md.insert(
                    "geo",
                    serde_json::json!({"crs":crs,"dimensions":"xy","encoding":"wkb"}).to_string(),
                );
            }
            md.insert("plenora.geometry.crs_resolution", "resolved");
            // Identificatore EPSG: nessuna definizione WKT/PROJJSON.
            md.insert("plenora.geometry.srid", crs.strip_prefix("EPSG:").unwrap());
            md.insert("plenora.geometry.encoding", "wkb");
            md.insert("plenora.geometry.dimensions", "xy");
            md.insert("plenora.geometry.spatial_semantics", "geometry");
            md.insert("plenora.geometry.precision", "float64");
            md.insert("plenora.geometry.types_declaration", dichiarazione);
            if let Some(tipi) = tipi {
                md.insert("plenora.geometry.types", tipi.to_ascii_lowercase());
            }
        }
        *campo = campo.clone().with_metadata(md);
    }
    Schema::new_with_metadata(campi, metadata)
}

/// Pubblicazione attesa delle fixture senza ID iniziali. Le geometrie derivate
/// conservano i riferimenti, ma non diventano primarie (metadati-arrow.md).
fn schema_ruolo_catalogo(
    op: &str,
    ruolo: &str,
    input: &RecordBatch,
    mut campi: Vec<Field>,
) -> Schema {
    let geometrie_input = input
        .schema()
        .fields()
        .iter()
        .filter(|f| e_geometria(f))
        .count();
    let geometrie_output: Vec<_> = campi
        .iter()
        .enumerate()
        .filter(|(_, f)| e_geometria(f))
        .map(|(i, _)| i)
        .collect();
    let primaria = geometrie_input == 1
        && !matches!(op, "table.drop_columns" | "table.reconcile")
        && (op != "table.melt" || ruolo == "id");
    let mut metadata = input.schema().metadata().clone();
    metadata.insert("plenora.contract.version", "1");
    if geometrie_output.len() == 1 && !primaria {
        metadata.insert("plenora.primary_geometry", "none");
    } else if geometrie_output.len() > 1 && primaria {
        metadata.insert("plenora.primary_geometry", geometrie_output[0].to_string());
    }
    for (i, f) in campi.iter_mut().enumerate() {
        let mut md = f.metadata().clone();
        md.insert("plenora.field_id", i.to_string());
        if e_geometria(f) {
            md.insert("plenora.geometry.crs_resolution", "resolved");
            md.insert("plenora.geometry.srid", "4326");
            md.insert("plenora.geometry.dimensions", "xy");
            md.insert("plenora.geometry.spatial_semantics", "geometry");
            md.insert("plenora.geometry.types_declaration", "unresolved");
        }
        *f = f.clone().with_metadata(md);
    }
    Schema::new_with_metadata(campi, metadata)
}
