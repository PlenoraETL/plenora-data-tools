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
//!   più; le sezioni sono tutte, nell'ordine fissato; i collegamenti al
//!   README e fra le schede puntano a titoli che esistono;
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

#[derive(Deserialize)]
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
        .collect();
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
    match piano.validate(&schemi) {
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
        Err(errore) if matches!(scheda.operazione.id, "table.pivot" | "table.transpose") => {
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

- **Politiche e limiti** stanno nel [README](../README.md): precisione
  geografica di 1 cm ([«Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)),
  validazione e budget del [runner](../README.md#runner), limiti dei file e
  della riproiezione. Le schede li collegano, non li ricopiano.
- **Config**: ogni config si legge con `deny_unknown_fields`; un campo
  sconosciuto, un tipo sbagliato o un valore fuori dominio è `InvalidPlan`
  in validazione. Un parametro scritto che l'operazione ignorerebbe si
  rifiuta ([README, «Validazione»](../README.md#validazione)).
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

fn collegamenti_rotti(documento: &str) -> Vec<String> {
    let readme = ancore(&leggi(&radice().join("README.md")));
    let proprie = ancore(documento);
    let mut rotti = Vec::new();
    for destinazione in collegamenti(documento) {
        let readme_ancora = destinazione.strip_prefix("../README.md#");
        let propria_ancora = destinazione.strip_prefix('#');
        let valido = match (readme_ancora, propria_ancora) {
            (Some(ancora), _) => readme.contains(ancora),
            (None, Some(ancora)) => proprie.contains(ancora),
            (None, None) => {
                destinazione == "../README.md"
                    || destinazione.starts_with("https://")
                    || radice().join("docs").join(&destinazione).exists()
            }
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
        match (esegui_su_vuoti(scheda), RIFIUTANO_IL_VUOTO.contains(&id)) {
            (Ok(righe), true) => errori.push(format!(
                "{id}: in RIFIUTANO_IL_VUOTO, ma gira ({righe} righe)"
            )),
            (Err(errore), ammesso) if !ammesso || errore.starts_with("forma:") => {
                errori.push(format!("{id}: {errore}"));
            }
            _ => {}
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
    let esempio = &scheda.esempio;
    let ingressi = esempio
        .ingressi
        .iter()
        .map(|spec| tabella_con_metadati(&spec.colonne, &spec.metadati).map(|t| t.slice(0, 0)))
        .collect::<Result<Vec<_>, _>>()?;
    let righe_in = vec![0; ingressi.len()];
    let righe_out = if matches!(scheda.operazione.id, "table.pivot" | "table.transpose") {
        kernel_diretto(scheda, &ingressi[0])?.num_rows()
    } else {
        let piano = Pipeline::from_json(&testo_piano(scheda)).map_err(|e| format!("piano: {e}"))?;
        let schemi: Vec<(&str, SchemaRef)> = esempio
            .ingressi
            .iter()
            .zip(&ingressi)
            .map(|(spec, batch)| (spec.nome.as_str(), batch.schema()))
            .collect();
        let tabelle = esempio
            .ingressi
            .iter()
            .zip(ingressi)
            .map(|(spec, batch)| (spec.nome.clone(), batch))
            .collect();
        let esito = piano
            .validate(&schemi)
            .and_then(|validata| validata.run(tabelle))
            .map_err(|e| format!("esecuzione: {e}"))?;
        let (_, uscita) = esito.outputs.into_iter().next().expect("un'uscita");
        uscita.num_rows()
    };
    verifica_forma(scheda.operazione, &righe_in, righe_out, false)?;
    Ok(righe_out)
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
    assert!(tipo_arrow("timestamp(ms, è)").is_err());
    assert!(tipo_arrow("timestamp(ms, UTC").is_err());
    assert_eq!(
        tipo_arrow("timestamp(ms, Europe/Rome)").map(|(tipo, _)| tipo),
        Ok(DataType::Timestamp(
            TimeUnit::Millisecond,
            Some(Arc::from("Europe/Rome"))
        ))
    );
}
