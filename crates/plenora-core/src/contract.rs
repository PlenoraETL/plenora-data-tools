//! Contratti dati del grafo (architettura.md, decisioni D6, D16,
//! D25; architettura.md#determinismo, architettura.md#planner-ed-executor).
//!
//! Fondamenta della Fase 2A: tipi NUOVI (non trasloco) che descrivono ciò che
//! scorre sugli archi del DAG (`DataContract`), l'identità logica stabile delle
//! colonne (`FieldId`), la provenienza e lo scope delle proprietà
//! (`PropertyConfidence`/`PropertyScope`/`ContractProperty`), le statistiche di
//! runtime (`RuntimeStatistic`) e la sequenza logica dei batch
//! (`BatchSequence`).
//!
//! Struttura aperta, comportamento chiuso: il modello ammette più colonne
//! geometriche e una geometria attiva, ma in v1 [`DataContract::validate`]
//! rifiuta contratti con più di una geometria (decisione D16).
//!
//! La type-safety avanzata sulle combinazioni confidence/scope prive di senso
//! (es. `Proven` + scope `Schema` per proprietà dataset-wide, architettura.md#planner-ed-executor) è
//! deliberatamente rimandata: la rappresentazione v1 è la coppia semplice
//! `ContractProperty<T> { confidence, scope }`.

pub mod arrow_metadata;

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::arrow::schema::DataType;
use crate::arrow::SchemaRef;
use crate::crs::ResolvedCrs;
use crate::error::{PlenoraError, Result};

/// Identità logica stabile di una colonna nel grafo (decisione D16).
///
/// Namespace globale del grafo: gli ID sono assegnati dal planner dopo aver
/// letto tutti gli input (oppure rimappati all'ingresso), così due input non
/// possono collidere. Una rinomina preserva il `FieldId`; una colonna
/// calcolata o derivata ne riceve uno nuovo; un join eredita i `FieldId`
/// globali dei rispettivi rami.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldId(pub u32);

impl fmt::Display for FieldId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "field#{}", self.0)
    }
}

/// Dimensionalità delle geometrie di una colonna (ICD §3.3, milestone B1.1).
///
/// Il contratto rappresenta e propaga la dimensionalità, NON la elabora.
/// Serializzazione ICD: `"xy"`, `"xyz"`, `"xym"`, `"xyzm"`, `"unknown"`
/// (minuscola).
///
/// `Unknown` significa «byte preservati, dimensionalità non risolta» e NON
/// deve mai essere mappato a [`GeometryDimensions::Xy`] (regola R3.4): chi
/// legge metadati privi di dimensionalità ottiene `Unknown`, mai un default
/// silenzioso. Trattare `Unknown` come `Xy` nasconderebbe geometrie Z/M
/// dietro un contratto 2D: errore di semantica, non semplificazione.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeometryDimensions {
    Xy,
    Xyz,
    Xym,
    Xyzm,
    /// Byte preservati, dimensionalità non risolta: mai mappare a `Xy` (R3.4).
    Unknown,
}

impl GeometryDimensions {
    /// Forma testuale ICD (minuscola): `"xy"`, `"xyz"`, `"xym"`, `"xyzm"`,
    /// `"unknown"`. Coincide con la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Xy => "xy",
            Self::Xyz => "xyz",
            Self::Xym => "xym",
            Self::Xyzm => "xyzm",
            Self::Unknown => "unknown",
        }
    }

    /// Byte per coordinata interleaved (`f64`), se garantiti: `Xy` = 16,
    /// `Xyz`/`Xym` = 24, `Xyzm` = 32.
    ///
    /// `Unknown` non garantisce alcuno stride (R3.4: i byte sono preservati
    /// ma la dimensionalità non è risolta) e restituisce `None`: nessun
    /// consumatore può assumere un layout di coordinate per `Unknown`.
    #[must_use]
    pub const fn coordinate_stride(self) -> Option<usize> {
        match self {
            Self::Xy => Some(16),
            Self::Xyz | Self::Xym => Some(24),
            Self::Xyzm => Some(32),
            Self::Unknown => None,
        }
    }
}

impl fmt::Display for GeometryDimensions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`GeometryDimensions`]: valore non riconosciuto.
///
/// Il messaggio elenca i valori ammessi e non riporta l'input (regola
/// «errori senza dati» di `plenora-core`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownGeometryDimensions;

impl fmt::Display for UnknownGeometryDimensions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "dimensionalita' geometria non riconosciuta (ammesse: xy, xyz, xym, xyzm, unknown)",
        )
    }
}

impl std::error::Error for UnknownGeometryDimensions {}

impl std::str::FromStr for GeometryDimensions {
    type Err = UnknownGeometryDimensions;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "xy" => Ok(Self::Xy),
            "xyz" => Ok(Self::Xyz),
            "xym" => Ok(Self::Xym),
            "xyzm" => Ok(Self::Xyzm),
            "unknown" => Ok(Self::Unknown),
            _ => Err(UnknownGeometryDimensions),
        }
    }
}

/// Framing binario delle celle geometria (ICD §3.3, regola R3.5: enum
/// chiuso).
///
/// Solo WKB ISO ed EWKB (estensione `PostGIS` con SRID/flag Z/M) sono
/// rappresentabili. Altri framing — header `GeoPackage`, TWKB, … — NON sono
/// rappresentabili: la discovery (milestone B1.3) deve rifiutarli con
/// errore esplicito, mai mapparli a un encoding noto.
///
/// Serializzazione ICD: `"wkb"`, `"ewkb"` (minuscola).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeometryEncoding {
    Wkb,
    Ewkb,
}

impl GeometryEncoding {
    /// Forma testuale ICD (minuscola): `"wkb"`, `"ewkb"`. Coincide con la
    /// serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wkb => "wkb",
            Self::Ewkb => "ewkb",
        }
    }
}

impl fmt::Display for GeometryEncoding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`GeometryEncoding`]: valore non riconosciuto.
///
/// Il messaggio elenca i valori ammessi e non riporta l'input (regola
/// «errori senza dati» di `plenora-core`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownGeometryEncoding;

impl fmt::Display for UnknownGeometryEncoding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("encoding geometria non riconosciuto (ammessi: wkb, ewkb)")
    }
}

impl std::error::Error for UnknownGeometryEncoding {}

impl std::str::FromStr for GeometryEncoding {
    type Err = UnknownGeometryEncoding;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "wkb" => Ok(Self::Wkb),
            "ewkb" => Ok(Self::Ewkb),
            _ => Err(UnknownGeometryEncoding),
        }
    }
}

/// Tipo geometrico canonico di una colonna (ICD §3.1, regola R3.1).
///
/// Enum chiuso dei sedici tipi canonici, serializzati in minuscolo SENZA
/// separatore (`point`, `linestring`, …): la forma coincide con quella dei
/// sistemi esterni (`PostGIS`, `GeoPackage`, OGC WKT usano `LINESTRING`,
/// `MULTIPOLYGON`), cosi' nessuna traduzione — e nessun punto di perdita di
/// informazione — e' richiesta ai confini.
///
/// R3.2: un componente PUO' supportare un sottoinsieme dei tipi, ma DEVE
/// rifiutare esplicitamente quelli che non supporta — mai degradarli,
/// approssimarli o ignorarli in silenzio.
///
/// INVARIANTE: l'ordine di dichiarazione delle varianti E' l'ordine
/// canonico di §3.1. `Ord` deriva da tale ordine e la serializzazione
/// canonica delle liste di tipi (R3.4.1, vedi [`GeometryTypesProperty`])
/// dipende da esso: non riordinare le varianti.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeometryType {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
    CircularString,
    CompoundCurve,
    CurvePolygon,
    MultiCurve,
    MultiSurface,
    PolyhedralSurface,
    Tin,
    Triangle,
    /// Tipo non risolto (R3.1): mai degradato a un tipo noto.
    Unknown,
}

impl GeometryType {
    /// Forma testuale ICD (minuscola senza separatore, R3.1). Coincide con
    /// la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::LineString => "linestring",
            Self::Polygon => "polygon",
            Self::MultiPoint => "multipoint",
            Self::MultiLineString => "multilinestring",
            Self::MultiPolygon => "multipolygon",
            Self::GeometryCollection => "geometrycollection",
            Self::CircularString => "circularstring",
            Self::CompoundCurve => "compoundcurve",
            Self::CurvePolygon => "curvepolygon",
            Self::MultiCurve => "multicurve",
            Self::MultiSurface => "multisurface",
            Self::PolyhedralSurface => "polyhedralsurface",
            Self::Tin => "tin",
            Self::Triangle => "triangle",
            Self::Unknown => "unknown",
        }
    }

    /// Mappa il type code WKB base ISO (senza la serie dimensionale 1000+
    /// ne' i flag EWKB, gia' estratti dal chiamante) al tipo canonico.
    ///
    /// Codici: 1..=7 i sette tipi base, 8 `circularstring`,
    /// 9 `compoundcurve`, 10 `curvepolygon`, 11 `multicurve`,
    /// 12 `multisurface`, 15 `polyhedralsurface`, 16 `tin`, 17 `triangle`.
    ///
    /// Restituisce `None` per 13 e 14 (`curve`/`surface`, tipi ASTRATTI
    /// non istanziabili: non compaiono mai come type code di una geometria
    /// concreta sul filo) e per qualunque altro codice (0, 18+, code con
    /// serie dimensionale non estratta). La scelta del rifiuto spetta al
    /// chiamante (R3.2: rifiuto esplicito, mai degradazione) — coerente
    /// con `parse_wkb_type_code` di `plenora-kernels-geo`, che oggi
    /// supporta solo 1..=7 e rifiuta esplicitamente il resto.
    #[must_use]
    pub const fn from_wkb_base_type(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::Point),
            2 => Some(Self::LineString),
            3 => Some(Self::Polygon),
            4 => Some(Self::MultiPoint),
            5 => Some(Self::MultiLineString),
            6 => Some(Self::MultiPolygon),
            7 => Some(Self::GeometryCollection),
            8 => Some(Self::CircularString),
            9 => Some(Self::CompoundCurve),
            10 => Some(Self::CurvePolygon),
            11 => Some(Self::MultiCurve),
            12 => Some(Self::MultiSurface),
            15 => Some(Self::PolyhedralSurface),
            16 => Some(Self::Tin),
            17 => Some(Self::Triangle),
            _ => None,
        }
    }
}

impl fmt::Display for GeometryType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`GeometryType`]: valore non riconosciuto.
///
/// Il messaggio elenca i valori ammessi e non riporta l'input (regola
/// «errori senza dati» di `plenora-core`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownGeometryType;

impl fmt::Display for UnknownGeometryType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "tipo geometrico non riconosciuto (ammessi: point, linestring, polygon, multipoint, multilinestring, multipolygon, geometrycollection, circularstring, compoundcurve, curvepolygon, multicurve, multisurface, polyhedralsurface, tin, triangle, unknown)",
        )
    }
}

impl std::error::Error for UnknownGeometryType {}

impl std::str::FromStr for GeometryType {
    type Err = UnknownGeometryType;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "point" => Ok(Self::Point),
            "linestring" => Ok(Self::LineString),
            "polygon" => Ok(Self::Polygon),
            "multipoint" => Ok(Self::MultiPoint),
            "multilinestring" => Ok(Self::MultiLineString),
            "multipolygon" => Ok(Self::MultiPolygon),
            "geometrycollection" => Ok(Self::GeometryCollection),
            "circularstring" => Ok(Self::CircularString),
            "compoundcurve" => Ok(Self::CompoundCurve),
            "curvepolygon" => Ok(Self::CurvePolygon),
            "multicurve" => Ok(Self::MultiCurve),
            "multisurface" => Ok(Self::MultiSurface),
            "polyhedralsurface" => Ok(Self::PolyhedralSurface),
            "tin" => Ok(Self::Tin),
            "triangle" => Ok(Self::Triangle),
            "unknown" => Ok(Self::Unknown),
            _ => Err(UnknownGeometryType),
        }
    }
}

/// Stato di dichiarazione dei tipi geometrici di una colonna (ICD R3.4.1,
/// chiave canonica `plenora.geometry.types_declaration`).
///
/// Tre stati che la 1.x confondeva in `unknown`:
///
/// - `Exact`: l'insieme dei tipi presenti e' noto ed e' quello elencato;
/// - `Mixed`: la colonna ammette tipi diversi PER DICHIARAZIONE (es. una
///   colonna `PostGIS` `geometry` senza vincolo): e' informazione, non
///   ignoranza;
/// - `Unresolved`: i byte non sono stati ispezionati e nessuna
///   dichiarazione e' disponibile.
///
/// R3.4.1 vieta le conversioni `mixed` ↔ `unresolved`. Un input legacy
/// privo di entrambe le chiavi `types`/`types_declaration` NON e'
/// `Unresolved`: significa «proprieta' non dichiarata» e va preservato o
/// normalizzato con un `LossReport`, mai interpretato come `unresolved`.
///
/// Serializzazione ICD: `"exact"`, `"mixed"`, `"unresolved"` (minuscola).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypesDeclaration {
    Exact,
    Mixed,
    Unresolved,
}

impl TypesDeclaration {
    /// Forma testuale ICD (minuscola): `"exact"`, `"mixed"`,
    /// `"unresolved"`. Coincide con la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Mixed => "mixed",
            Self::Unresolved => "unresolved",
        }
    }
}

impl fmt::Display for TypesDeclaration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`TypesDeclaration`]: valore non riconosciuto.
///
/// Il messaggio elenca i valori ammessi e non riporta l'input (regola
/// «errori senza dati» di `plenora-core`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownTypesDeclaration;

impl fmt::Display for UnknownTypesDeclaration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("types_declaration non riconosciuta (ammesse: exact, mixed, unresolved)")
    }
}

impl std::error::Error for UnknownTypesDeclaration {}

impl std::str::FromStr for TypesDeclaration {
    type Err = UnknownTypesDeclaration;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "exact" => Ok(Self::Exact),
            "mixed" => Ok(Self::Mixed),
            "unresolved" => Ok(Self::Unresolved),
            _ => Err(UnknownTypesDeclaration),
        }
    }
}

/// Errore di costruzione o parsing di [`GeometryTypesProperty`].
///
/// I messaggi descrivono la violazione senza riportare mai il contenuto
/// dell'elenco (regola «errori senza dati» di `plenora-core`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryTypesPropertyError {
    /// `exact` senza elenco o con elenco vuoto (R3.4.1).
    ExactWithoutTypes,
    /// `unresolved` con elenco presente (R3.4.1).
    UnresolvedWithTypes,
    /// Valore non canonico nell'elenco testuale (maiuscole, `snake_case`,
    /// spazi, token vuoti o nomi ignoti).
    UnknownTypeInList,
    /// Duplicato nell'elenco testuale: la forma canonica richiede valori
    /// unici (R3.4.1).
    DuplicateTypeInList,
    /// Elenco testuale fuori dall'ordine canonico di §3.1 (R3.4.1).
    NonCanonicalOrder,
}

impl fmt::Display for GeometryTypesPropertyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ExactWithoutTypes => {
                "types_declaration `exact` richiede un elenco di tipi presente e non vuoto (R3.4.1)"
            }
            Self::UnresolvedWithTypes => {
                "types_declaration `unresolved` non ammette un elenco di tipi (R3.4.1)"
            }
            Self::UnknownTypeInList => {
                "elenco tipi con valore non canonico (ammessi i 16 tipi di R3.1, minuscoli senza separatore, separati da `,` senza spazi)"
            }
            Self::DuplicateTypeInList => {
                "elenco tipi con duplicati: la forma canonica richiede valori unici (R3.4.1)"
            }
            Self::NonCanonicalOrder => {
                "elenco tipi fuori ordine canonico (R3.4.1, ordine di §3.1)"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GeometryTypesPropertyError {}

/// Coppia coerente (`types_declaration`, `types`) delle chiavi canoniche
/// R2.2/R3.4.1 per una colonna geometrica.
///
/// Le coerenze di R3.4.1 sono imposte per costruzione — campi privati,
/// unico ingresso [`GeometryTypesProperty::new`]:
///
/// - `Exact` richiede un elenco presente e non vuoto;
/// - `Unresolved` vieta l'elenco (errore se presente);
/// - `Mixed` ammette elenco assente o non vuoto (un elenco vuoto NON e'
///   distinguibile dall'assenza e conta come assente).
///
/// `new` normalizza l'elenco (valori unici, ordine canonico §3.1) cosi'
/// una stessa dichiarazione ha una sola serializzazione
/// ([`GeometryTypesProperty::to_canonical_list`]). Il parsing testuale
/// ([`GeometryTypesProperty::from_canonical_list`]) e' invece fail-closed:
/// spazi, duplicati e ordine non canonico sono errori, mai correzioni
/// silenziose — un produttore conforme non li emette.
///
/// Nessuna derivazione serde: la forma sul filo e' la coppia di chiavi di
/// metadati R2.2 (`types_declaration` + lista canonica), non una
/// serializzazione di struct, e una `Deserialize` derivata bypasserebbe il
/// validatore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeometryTypesProperty {
    declaration: TypesDeclaration,
    types: Box<[GeometryType]>,
}

impl GeometryTypesProperty {
    /// Costruisce la proprieta' validando le coerenze R3.4.1 e
    /// normalizzando l'elenco (dedup + ordine canonico §3.1).
    ///
    /// # Errors
    ///
    /// - [`GeometryTypesPropertyError::ExactWithoutTypes`] se `exact` con
    ///   elenco vuoto;
    /// - [`GeometryTypesPropertyError::UnresolvedWithTypes`] se
    ///   `unresolved` con elenco non vuoto.
    pub fn new(
        declaration: TypesDeclaration,
        mut types: Vec<GeometryType>,
    ) -> std::result::Result<Self, GeometryTypesPropertyError> {
        // Normalizzazione R3.4.1: valori unici in ordine canonico §3.1 —
        // una stessa dichiarazione ha una sola serializzazione.
        types.sort_unstable();
        types.dedup();
        Self::check_coherence(declaration, types.len())?;
        Ok(Self {
            declaration,
            types: types.into_boxed_slice(),
        })
    }

    /// Parsing fail-closed dalla forma canonica sul filo: valori unici in
    /// ordine §3.1 separati da `,` senza spazi. La stringa vuota modella
    /// l'elenco assente (chiave `types` non emessa).
    ///
    /// # Errors
    ///
    /// Oltre alle coerenze di [`GeometryTypesProperty::new`]:
    /// [`GeometryTypesPropertyError::UnknownTypeInList`],
    /// [`GeometryTypesPropertyError::DuplicateTypeInList`],
    /// [`GeometryTypesPropertyError::NonCanonicalOrder`].
    pub fn from_canonical_list(
        declaration: TypesDeclaration,
        list: &str,
    ) -> std::result::Result<Self, GeometryTypesPropertyError> {
        if list.is_empty() {
            Self::check_coherence(declaration, 0)?;
            return Ok(Self {
                declaration,
                types: Vec::new().into_boxed_slice(),
            });
        }
        let mut parsed: Vec<GeometryType> = Vec::new();
        for token in list.split(',') {
            // Fail-closed: maiuscole, snake_case, spazi, token vuoti e nomi
            // ignoti sono rifiutati da `FromStr`; mai correggere.
            let geometry_type = token
                .parse::<GeometryType>()
                .map_err(|_| GeometryTypesPropertyError::UnknownTypeInList)?;
            if let Some(&last) = parsed.last() {
                if last == geometry_type {
                    return Err(GeometryTypesPropertyError::DuplicateTypeInList);
                }
                if last > geometry_type {
                    return Err(GeometryTypesPropertyError::NonCanonicalOrder);
                }
            }
            parsed.push(geometry_type);
        }
        Self::check_coherence(declaration, parsed.len())?;
        Ok(Self {
            declaration,
            types: parsed.into_boxed_slice(),
        })
    }

    const fn check_coherence(
        declaration: TypesDeclaration,
        len: usize,
    ) -> std::result::Result<(), GeometryTypesPropertyError> {
        match declaration {
            TypesDeclaration::Exact if len == 0 => {
                Err(GeometryTypesPropertyError::ExactWithoutTypes)
            }
            TypesDeclaration::Unresolved if len > 0 => {
                Err(GeometryTypesPropertyError::UnresolvedWithTypes)
            }
            _ => Ok(()),
        }
    }

    /// La dichiarazione R3.4.1.
    #[must_use]
    pub const fn declaration(&self) -> TypesDeclaration {
        self.declaration
    }

    /// L'elenco normalizzato dei tipi (unici, ordine canonico §3.1);
    /// vuoto quando la dichiarazione non porta elenco.
    #[must_use]
    pub fn types(&self) -> &[GeometryType] {
        &self.types
    }

    /// Serializzazione canonica della chiave `plenora.geometry.types`
    /// (R2.2): valori unici in ordine §3.1 separati da `,` senza spazi.
    /// Stringa vuota quando l'elenco e' assente.
    #[must_use]
    pub fn to_canonical_list(&self) -> String {
        let mut list = String::new();
        for (index, geometry_type) in self.types.iter().enumerate() {
            if index > 0 {
                list.push(',');
            }
            list.push_str(geometry_type.as_str());
        }
        list
    }
}

/// Ordine degli assi del CRS (chiave canonica `plenora.geometry.axis_order`,
/// tabella R2.2). Serializzazione ICD minuscola con `_`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisOrder {
    LonLat,
    LatLon,
    EastingNorthing,
    NorthingEasting,
    Other,
    Unknown,
}

impl AxisOrder {
    /// Forma testuale ICD. Coincide con la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LonLat => "lon_lat",
            Self::LatLon => "lat_lon",
            Self::EastingNorthing => "easting_northing",
            Self::NorthingEasting => "northing_easting",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for AxisOrder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`AxisOrder`]: valore non riconosciuto (nessun dato
/// nel messaggio, regola «errori senza dati»).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownAxisOrder;

impl fmt::Display for UnknownAxisOrder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "ordine assi non riconosciuto (ammessi: lon_lat, lat_lon, easting_northing, northing_easting, other, unknown)",
        )
    }
}

impl std::error::Error for UnknownAxisOrder {}

impl std::str::FromStr for AxisOrder {
    type Err = UnknownAxisOrder;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "lon_lat" => Ok(Self::LonLat),
            "lat_lon" => Ok(Self::LatLon),
            "easting_northing" => Ok(Self::EastingNorthing),
            "northing_easting" => Ok(Self::NorthingEasting),
            "other" => Ok(Self::Other),
            "unknown" => Ok(Self::Unknown),
            _ => Err(UnknownAxisOrder),
        }
    }
}

/// Stato di risoluzione del CRS (chiave canonica
/// `plenora.geometry.crs_resolution`, tabella R2.2). Serializzazione ICD
/// minuscola con `_`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrsResolution {
    Resolved,
    DeclaredUnresolved,
    Missing,
}

impl CrsResolution {
    /// Forma testuale ICD. Coincide con la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::DeclaredUnresolved => "declared_unresolved",
            Self::Missing => "missing",
        }
    }
}

impl fmt::Display for CrsResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`CrsResolution`]: valore non riconosciuto (nessun
/// dato nel messaggio, regola «errori senza dati»).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownCrsResolution;

impl fmt::Display for UnknownCrsResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "risoluzione CRS non riconosciuta (ammesse: resolved, declared_unresolved, missing)",
        )
    }
}

impl std::error::Error for UnknownCrsResolution {}

impl std::str::FromStr for CrsResolution {
    type Err = UnknownCrsResolution;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "resolved" => Ok(Self::Resolved),
            "declared_unresolved" => Ok(Self::DeclaredUnresolved),
            "missing" => Ok(Self::Missing),
            _ => Err(UnknownCrsResolution),
        }
    }
}

/// Formato testuale della definizione CRS (chiave canonica
/// `plenora.geometry.crs_definition_format`, tabella R2.2).
/// Serializzazione ICD minuscola.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrsDefinitionFormat {
    Wkt,
    Wkt2,
    Projjson,
}

impl CrsDefinitionFormat {
    /// Forma testuale ICD. Coincide con la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wkt => "wkt",
            Self::Wkt2 => "wkt2",
            Self::Projjson => "projjson",
        }
    }
}

impl fmt::Display for CrsDefinitionFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`CrsDefinitionFormat`]: valore non riconosciuto
/// (nessun dato nel messaggio, regola «errori senza dati»).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownCrsDefinitionFormat;

impl fmt::Display for UnknownCrsDefinitionFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("formato definizione CRS non riconosciuto (ammessi: wkt, wkt2, projjson)")
    }
}

impl std::error::Error for UnknownCrsDefinitionFormat {}

impl std::str::FromStr for CrsDefinitionFormat {
    type Err = UnknownCrsDefinitionFormat;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "wkt" => Ok(Self::Wkt),
            "wkt2" => Ok(Self::Wkt2),
            "projjson" => Ok(Self::Projjson),
            _ => Err(UnknownCrsDefinitionFormat),
        }
    }
}

/// Semantica spaziale della colonna (chiave canonica
/// `plenora.geometry.spatial_semantics`, tabella R2.2). Serializzazione ICD
/// minuscola.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpatialSemantics {
    Geometry,
    Geography,
}

impl SpatialSemantics {
    /// Forma testuale ICD. Coincide con la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Geometry => "geometry",
            Self::Geography => "geography",
        }
    }
}

impl fmt::Display for SpatialSemantics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`SpatialSemantics`]: valore non riconosciuto
/// (nessun dato nel messaggio, regola «errori senza dati»).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownSpatialSemantics;

impl fmt::Display for UnknownSpatialSemantics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("semantica spaziale non riconosciuta (ammesse: geometry, geography)")
    }
}

impl std::error::Error for UnknownSpatialSemantics {}

impl std::str::FromStr for SpatialSemantics {
    type Err = UnknownSpatialSemantics;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "geometry" => Ok(Self::Geometry),
            "geography" => Ok(Self::Geography),
            _ => Err(UnknownSpatialSemantics),
        }
    }
}

/// Precisione delle coordinate (chiave canonica
/// `plenora.geometry.precision`, tabella R2.2). Serializzazione ICD
/// minuscola.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeometryPrecision {
    Float64,
    Float32,
    Native,
}

impl GeometryPrecision {
    /// Forma testuale ICD. Coincide con la serializzazione serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Float64 => "float64",
            Self::Float32 => "float32",
            Self::Native => "native",
        }
    }
}

impl fmt::Display for GeometryPrecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Errore di parsing di [`GeometryPrecision`]: valore non riconosciuto
/// (nessun dato nel messaggio, regola «errori senza dati»).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownGeometryPrecision;

impl fmt::Display for UnknownGeometryPrecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("precisione geometria non riconosciuta (ammesse: float64, float32, native)")
    }
}

impl std::error::Error for UnknownGeometryPrecision {}

impl std::str::FromStr for GeometryPrecision {
    type Err = UnknownGeometryPrecision;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "float64" => Ok(Self::Float64),
            "float32" => Ok(Self::Float32),
            "native" => Ok(Self::Native),
            _ => Err(UnknownGeometryPrecision),
        }
    }
}

/// CRS di una colonna geometrica nel contratto (R4.1: gli stati di
/// risoluzione non si collassano nel modello interno; R4.4: mai un CRS
/// inventato — `Missing` non porta dati).
///
/// - [`ContractCrs::Resolved`]: definizione risolta contro il database PROJ
///   dalla discovery (solo una risoluzione produce questo valore);
/// - [`ContractCrs::ResolvedByDecision`]: come `Resolved` (definizione
///   risolta contro PROJ, gli stessi consumatori), ma la risoluzione e'
///   l'effetto di una DECISIONE ESPLICITA DEL PIANO (R4.6.3, campo v4
///   `crs_decisions`) su uno stato `DeclaredUnresolved` — non di una
///   dichiarazione della sorgente. La distinzione serve all'emissione: le
///   dichiarazioni in conflitto della sorgente (chiavi canoniche CRS e
///   `geo.crs` legacy nei metadati di campo) sono sostituite dal CRS
///   deciso, non propagate accanto ad esso; ovunque altro (gate delle op,
///   fingerprint, riepiloghi) il valore e' un CRS risolto a tutti gli
///   effetti;
/// - [`ContractCrs::DeclaredUnresolved`]: il CRS c'e' ma non si risolve —
///   il produttore dichiara `declared_unresolved`, oppure le
///   rappresentazioni dichiarate sono in conflitto decidibile. R4.6.3: in
///   assenza di una decisione esplicita nel piano il centro PROPAGA
///   l'incoerenza senza risolverla (mai una scelta silenziosa) e senza
///   pretendere un CRS risolvibile per operazioni che non lo richiedono;
///   R4.6.4: l'incoerenza arriva al bordo di scrittura con le dichiarazioni
///   ORIGINALI ri-emesse invariate (mai una persa, mai una inventata) —
///   per questo la variante porta `crs_id` e `definition` con il suo
///   formato (R4.3: una definizione senza discriminatore non e'
///   interpretabile). Lo `srid` non entra nella variante: non e' una
///   definizione risolvibile dal centro e viaggia come chiave di lineage
///   nei metadati di campo (R2.4). Invariante di costruzione (imposta
///   dalla discovery): almeno uno fra `crs_id` e `definition` e' presente
///   — `declared_unresolved` senza rappresentazioni e' una contraddizione
///   R4.1 e resta un errore;
/// - [`ContractCrs::Missing`]: nessun CRS dichiarato in alcuna
///   rappresentazione accettata. R4.6.3: la discovery non puo' pretendere un
///   CRS risolvibile per operazioni che non lo richiedono — lo stato entra
///   nel contratto, si propaga invariato negli output (R4.6.4, emesso come
///   `plenora.geometry.crs_resolution = missing`) e ferma solo le op che
///   dichiarano un `CrsRequirement`, in analyze (mai a meta' stream).
///
/// `DeclaredUnresolved` ferma le op con `CrsRequirement` come `Missing`,
/// nello stesso punto (analyze, compile-plan) e con la stessa categoria
/// (`Crs`), ma con un messaggio distinto: la colonna DICHIARA
/// un'incoerenza, non un'assenza.
#[derive(Clone, Debug)]
pub enum ContractCrs {
    Resolved(ResolvedCrs),
    /// Risolto per decisione esplicita del piano (R4.6.3): stesso
    /// comportamento di [`ContractCrs::Resolved`] per i consumatori del
    /// CRS; l'emissione sostituisce le dichiarazioni della sorgente con il
    /// CRS deciso.
    ResolvedByDecision(ResolvedCrs),
    /// Incoerenza dichiarata non risolta (R4.6.3): le rappresentazioni
    /// originali, per la ri-emissione fedele (R2.4/R4.6.4).
    DeclaredUnresolved {
        /// Identificatore di autorita' dichiarato (`plenora.geometry.crs_id`).
        crs_id: Option<String>,
        /// Definizione testuale dichiarata (`plenora.geometry.crs_definition`).
        definition: Option<String>,
        /// Formato della definizione (`plenora.geometry.crs_definition_format`,
        /// R4.3), se dichiarato.
        definition_format: Option<CrsDefinitionFormat>,
    },
    Missing,
}

impl ContractCrs {
    /// Il CRS risolto, se lo stato e' `Resolved` o `ResolvedByDecision`.
    #[must_use]
    pub const fn as_resolved(&self) -> Option<&ResolvedCrs> {
        match self {
            Self::Resolved(crs) | Self::ResolvedByDecision(crs) => Some(crs),
            Self::DeclaredUnresolved { .. } | Self::Missing => None,
        }
    }

    /// Stato di risoluzione per la chiave canonica
    /// `plenora.geometry.crs_resolution` (R2.2).
    #[must_use]
    pub const fn resolution(&self) -> CrsResolution {
        match self {
            Self::Resolved(_) | Self::ResolvedByDecision(_) => CrsResolution::Resolved,
            Self::DeclaredUnresolved { .. } => CrsResolution::DeclaredUnresolved,
            Self::Missing => CrsResolution::Missing,
        }
    }
}

/// Contratto di una colonna geometrica (architettura.md).
#[derive(Clone, Debug)]
pub struct GeometryColumnContract {
    /// Identità logica stabile nel grafo: le rinomine cambiano `name`,
    /// non `field_id`.
    pub field_id: FieldId,
    /// Nome visibile della colonna nello schema Arrow.
    pub name: String,
    /// Stato del CRS (solo `geo.reproject` modifica un CRS risolto; ogni
    /// altro step lo preserva, incluso lo stato `Missing` — R4.6.4).
    pub crs: ContractCrs,
    pub dimensions: GeometryDimensions,
    /// Framing binario delle celle (ICD §3.3, regola R3.5), se dichiarato
    /// dai metadati: `None` quando la sorgente non dichiara un `encoding`
    /// (input pre-B1.3) — mai un default silenzioso. I framing fuori
    /// dall'enum chiuso non sono rappresentabili: la discovery li rifiuta
    /// con errore esplicito prima di costruire il contratto.
    pub encoding: Option<GeometryEncoding>,
    pub nullable: bool,
    /// Dichiarazione dei tipi geometrici della colonna (chiavi canoniche
    /// `plenora.geometry.types` + `plenora.geometry.types_declaration`,
    /// R2.2/R3.4.1).
    ///
    /// DEFAULT per i costruttori esistenti:
    /// [`GeometryColumnContract::undeclared_types`] — confidence `Unknown`,
    /// cioe' «proprieta' non dichiarata». Attenzione alla distinzione di
    /// R3.4.1: un input legacy privo di entrambe le chiavi NON e'
    /// `TypesDeclaration::Unresolved`. `Unresolved` e' una dichiarazione
    /// esplicita del produttore («byte non ispezionati»); il legacy
    /// assente va preservato o normalizzato con un `LossReport`, mai
    /// interpretato come `unresolved` (ne' convertito in/da `mixed`).
    pub types: ContractProperty<GeometryTypesProperty>,
}

impl GeometryColumnContract {
    /// Default del campo `types` per i contratti costruiti senza ispezione
    /// dei tipi: proprieta' NON dichiarata (confidence `Unknown`, scope
    /// `Schema`).
    ///
    /// NON equivale a `Declared(TypesDeclaration::Unresolved)`: per
    /// R3.4.1 l'assenza delle chiavi in un ingresso legacy significa
    /// «proprieta' non dichiarata», uno stato distinto da `unresolved` che
    /// va preservato o normalizzato con un `LossReport`.
    #[must_use]
    pub const fn undeclared_types() -> ContractProperty<GeometryTypesProperty> {
        ContractProperty::new(PropertyConfidence::Unknown, PropertyScope::Schema)
    }
}

/// Assegnatore di [`FieldId`] nel namespace globale del grafo (decisione D16).
///
/// Unico allocatore condiviso dal planner e dai moduli `analyze_contract`
/// delle famiglie di kernel (unificazione Fase 2A-3: in precedenza
/// `plenora-kernels-table` ne definiva uno locale, doppione di questo).
/// Il planner crea un unico allocatore per l'analisi dell'intero grafo e
/// rimappa i `FieldId` delle geometrie di input con [`FieldAllocator::alloc`]
/// (senza legare i nomi: due input possono avere colonne omonime);
/// [`FieldAllocator::observe`] e' chiamato dai moduli `analyze_contract` per
/// registrare gli ID gia' presenti nei contratti di input di ciascun nodo ed
/// evitare collisioni con i futuri [`FieldAllocator::alloc`]; le colonne
/// propagate mantengono l'ID di input; quelle derivate
/// ([`FieldAllocator::derive`]) ne ricevono uno nuovo; le rinomine spostano
/// l'identità ([`FieldAllocator::rename`]). L'interning per nome
/// ([`FieldAllocator::intern`]) rende stabile l'ID di una colonna visibile
/// tra nodi analizzati con lo stesso allocatore (usato per le chiavi
/// `sorted_by`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldAllocator {
    next: u32,
    by_name: HashMap<String, FieldId>,
}

impl FieldAllocator {
    /// Allocatore che parte da `next` (il planner usa il primo ID libero dopo
    /// gli input del grafo).
    #[must_use]
    pub fn new(next: u32) -> Self {
        Self {
            next,
            by_name: HashMap::new(),
        }
    }

    /// Assegna un nuovo `FieldId` garantito fresco e avanza il cursore.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` quando lo spazio degli identificatori e'
    /// esaurito. L'incremento era saturante: arrivato a `u32::MAX`
    /// l'allocatore restituiva ripetutamente LO STESSO id, e la garanzia di
    /// freschezza — su cui poggia l'identita' delle colonne nel grafo (D16) —
    /// cadeva in silenzio, con due colonne diverse che condividevano
    /// identita'. Meglio un piano rifiutato che un grafo con identita'
    /// ambigue.
    pub fn alloc(&mut self) -> Result<FieldId> {
        let id = FieldId(self.next);
        self.next = self.next.checked_add(1).ok_or_else(|| {
            PlenoraError::InvalidPlan(
                "spazio dei FieldId esaurito: nessun identificatore fresco disponibile".to_owned(),
            )
        })?;
        Ok(id)
    }

    /// Il prossimo ID che verrà assegnato (ispezione, non consuma).
    #[must_use]
    pub const fn peek(&self) -> FieldId {
        FieldId(self.next)
    }

    /// Registra un ID già assegnato (contratti di input) per evitare
    /// collisioni con i futuri [`FieldAllocator::alloc`].
    pub fn observe(&mut self, id: FieldId) {
        self.next = self.next.max(id.0.saturating_add(1));
    }

    /// ID stabile di una colonna propagata: stesso nome → stesso ID.
    ///
    /// # Errors
    ///
    /// Come [`FieldAllocator::alloc`], e solo al primo incontro del nome: un
    /// nome gia' interned non consuma identificatori.
    pub fn intern(&mut self, name: &str) -> Result<FieldId> {
        if let Some(id) = self.by_name.get(name) {
            return Ok(*id);
        }
        let id = self.alloc()?;
        self.by_name.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Rinomina: il `FieldId` segue la colonna (D16).
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(id) = self.by_name.remove(old) {
            self.by_name.insert(new.to_owned(), id);
        }
    }

    /// Colonna derivata: riceve un `FieldId` nuovo, sostituendo l'eventuale
    /// identità precedente associata al nome (D16).
    ///
    /// # Errors
    ///
    /// Come [`FieldAllocator::alloc`].
    pub fn derive(&mut self, name: &str) -> Result<FieldId> {
        let id = self.alloc()?;
        self.by_name.insert(name.to_owned(), id);
        Ok(id)
    }
}

/// Provenienza di una proprietà del contratto (decisione D25).
///
/// Il planner usa come precondizioni semantiche solo proprietà `Proven`;
/// le `Estimated` guidano esclusivamente scelte prestazionali correggibili a
/// runtime. Le proprietà possono cambiare livello nel tempo: una `Declared`
/// può diventare `Proven` tramite validazione dinamica, una `Estimated` può
/// essere aggiornata dal runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyConfidence<T> {
    /// Dichiarata da una fonte esterna (piano, utente), non verificata.
    Declared(T),
    /// Dimostrata: usabile come precondizione semantica.
    Proven(T),
    /// Stimata: solo scelte prestazionali correggibili a runtime.
    Estimated(T),
    /// Assente.
    Unknown,
}

impl<T> PropertyConfidence<T> {
    /// Il valore, se presente a qualunque livello di fiducia.
    pub const fn value(&self) -> Option<&T> {
        match self {
            Self::Declared(value) | Self::Proven(value) | Self::Estimated(value) => Some(value),
            Self::Unknown => None,
        }
    }

    /// Il valore solo se `Proven` (unica precondizione semantica ammessa).
    pub const fn proven_value(&self) -> Option<&T> {
        match self {
            Self::Proven(value) => Some(value),
            _ => None,
        }
    }

    /// `true` solo per `Proven`.
    pub const fn is_proven(&self) -> bool {
        matches!(self, Self::Proven(_))
    }
}

/// Ambito di validità di una proprietà (decisione D25).
///
/// Lo scope conta: "ogni batch è ordinato" non implica "lo stream è
/// ordinato". Una proprietà dataset-wide diventa `Proven(Dataset)` solo al
/// completamento dell'intero input — mai inferenze premature a metà stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropertyScope {
    /// Deducibile dallo schema (statica, indipendente dai dati).
    Schema,
    /// Vale per il singolo batch.
    Batch,
    /// Vale lungo lo stream di batch di un arco.
    Stream,
    /// Vale sull'intero dataset.
    Dataset,
}

/// Una proprietà tipizzata con provenienza e scope (architettura.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractProperty<T> {
    pub confidence: PropertyConfidence<T>,
    pub scope: PropertyScope,
}

impl<T> ContractProperty<T> {
    pub const fn new(confidence: PropertyConfidence<T>, scope: PropertyScope) -> Self {
        Self { confidence, scope }
    }

    /// `true` solo se la proprietà è `Proven` (precondizione semantica).
    pub const fn is_proven(&self) -> bool {
        self.confidence.is_proven()
    }

    /// Il valore, se presente a qualunque livello di fiducia.
    pub const fn value(&self) -> Option<&T> {
        self.confidence.value()
    }
}

/// Proprietà tipizzate del contratto, v1 minimale (non un framework generico:
/// nuove proprietà si aggiungono come campi tipizzati).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContractProperties {
    /// Chiavi di ordinamento dichiarate/dimostrate, come `FieldId` nel
    /// namespace globale del grafo.
    pub sorted_by: Option<ContractProperty<Vec<FieldId>>>,
    /// Cardinalità nota o stimata dell'arco (mai `Proven` in fase 1: non è
    /// dimostrabile dagli header, architettura.md).
    pub row_count: Option<ContractProperty<u64>>,
}

/// Contratto di un arco del DAG (decisioni D6, D16).
///
/// Ogni arco trasporta `RecordBatch` conformi a questo contratto, inferito a
/// secco dal planner (`analyze_contract` di ogni operazione, Fase 2A-2).
#[derive(Clone, Debug)]
pub struct DataContract {
    pub schema: SchemaRef,
    /// v1: al massimo una colonna geometrica (validato, decisione D16).
    pub geometries: Vec<GeometryColumnContract>,
    pub active_geometry: Option<FieldId>,
    pub properties: ContractProperties,
}

impl DataContract {
    /// Contratto tabellare: nessuna colonna geometrica.
    #[must_use]
    pub fn tabular(schema: SchemaRef) -> Self {
        Self {
            schema,
            geometries: Vec::new(),
            active_geometry: None,
            properties: ContractProperties::default(),
        }
    }

    /// Costruisce e valida subito il contratto (fail-closed).
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::Schema` se il contratto viola le regole di
    /// [`DataContract::validate`].
    pub fn new(
        schema: SchemaRef,
        geometries: Vec<GeometryColumnContract>,
        active_geometry: Option<FieldId>,
        properties: ContractProperties,
    ) -> Result<Self> {
        let contract = Self {
            schema,
            geometries,
            active_geometry,
            properties,
        };
        contract.validate()?;
        Ok(contract)
    }

    /// Validazione strutturale del contratto.
    ///
    /// Regole v1:
    ///
    /// - i nomi dei campi dello schema sono univoci — senza questo controllo
    ///   `field_with_name` risolve al primo omonimo e il contratto puo'
    ///   descrivere una colonna diversa da quella che verra' letta;
    /// - al massimo una colonna geometrica (D16);
    /// - ogni colonna geometrica esiste nello schema con lo stesso nome, la
    ///   stessa nullability e **tipo fisico `Binary`** — il framing WKB/EWKB
    ///   e' un vettore di byte, e ogni lettore della geometria fa `downcast`
    ///   a `BinaryArray`: un contratto che dichiara geometrica una colonna di
    ///   tipo diverso passava il dry-run e falliva a runtime;
    /// - se sia il contratto sia i metadati canonici della colonna dichiarano
    ///   i tipi geometrici, le due dichiarazioni devono coincidere;
    /// - `active_geometry`, se presente, riferisce una delle colonne
    ///   geometriche dichiarate.
    ///
    /// # Errors
    ///
    /// Restituisce `PlenoraError::Schema` descrivendo la prima violazione.
    pub fn validate(&self) -> Result<()> {
        let mut seen: HashSet<&str> = HashSet::with_capacity(self.schema.fields().len());
        for field in self.schema.fields() {
            if !seen.insert(field.name().as_str()) {
                return Err(PlenoraError::Schema(format!(
                    "schema con nome di campo ripetuto `{}`: l'identita' delle colonne del contratto non sarebbe univoca",
                    field.name()
                )));
            }
        }
        if self.geometries.len() > 1 {
            return Err(PlenoraError::Schema(format!(
                "contratto con {} colonne geometriche: la v1 ne ammette al massimo una (D16)",
                self.geometries.len()
            )));
        }
        for geometry in &self.geometries {
            let field = self.schema.field_with_name(&geometry.name).map_err(|_| {
                PlenoraError::Schema(format!(
                    "colonna geometrica `{}` ({}) assente dallo schema",
                    geometry.name, geometry.field_id
                ))
            })?;
            if field.is_nullable() != geometry.nullable {
                return Err(PlenoraError::Schema(format!(
                    "colonna geometrica `{}`: nullability del contratto ({}) diversa dallo schema ({})",
                    geometry.name,
                    geometry.nullable,
                    field.is_nullable()
                )));
            }
            if field.data_type() != &DataType::Binary {
                return Err(PlenoraError::Schema(format!(
                    "colonna geometrica `{}`: tipo fisico {:?}, atteso Binary (framing WKB/EWKB)",
                    geometry.name,
                    field.data_type()
                )));
            }
            validate_declared_types(geometry, field.metadata())?;
        }
        if let Some(active) = self.active_geometry {
            if !self.geometries.iter().any(|g| g.field_id == active) {
                return Err(PlenoraError::Schema(format!(
                    "active_geometry {active} non e' tra le colonne geometriche dichiarate"
                )));
            }
        }
        Ok(())
    }

    /// La colonna geometrica attiva, se presente: `active_geometry` se
    /// dichiarata, altrimenti l'unica geometria del contratto.
    ///
    #[must_use]
    pub fn active_geometry_column(&self) -> Option<&GeometryColumnContract> {
        self.active_geometry.map_or_else(
            || self.geometries.first(),
            |active| self.geometries.iter().find(|g| g.field_id == active),
        )
    }
}

/// Chiave canonica dell'elenco dei tipi geometrici (R2.2).
///
/// Vive qui, nel livello contratto, perche' e' proprio la chiave con cui il
/// contratto e i metadati Arrow devono concordare; `plenora-kernels-geo` la
/// riespone per il lato emissione, cosi' esiste una sola definizione.
pub const PLENORA_GEOMETRY_TYPES_KEY: &str = "plenora.geometry.types";

/// Chiave canonica dello stato di dichiarazione dei tipi (R2.2/R3.4.1).
pub const PLENORA_GEOMETRY_TYPES_DECLARATION_KEY: &str = "plenora.geometry.types_declaration";

/// Chiave canonica della dimensionalita' (R2.1/R2.2).
pub const PLENORA_GEOMETRY_DIMENSIONS_KEY: &str = "plenora.geometry.dimensions";

/// Chiave canonica del framing binario delle celle (R3.5).
pub const PLENORA_GEOMETRY_ENCODING_KEY: &str = "plenora.geometry.encoding";

/// Chiave canonica dello stato di risoluzione del CRS (R2.2).
pub const PLENORA_GEOMETRY_CRS_RESOLUTION_KEY: &str = "plenora.geometry.crs_resolution";

/// Coerenza fra la proprieta' tipizzata `types` del contratto e i metadati
/// canonici della colonna.
///
/// Due controlli distinti, che non vanno confusi:
///
/// 1. **Validita' sintattica di ogni chiave presente.** Una chiave canonica
///    che c'e' dev'essere leggibile, indipendentemente da cosa dichiari il
///    contratto: «assente» e «presente ma malformata» sono stati diversi, e
///    saltare il parsing quando il lato tipizzato tace lasciava passare
///    `encoding = "twkb"` o un elenco di tipi fuori ordine.
/// 2. **Coerenza fra le due fonti.** Qui il confronto scatta solo quando
///    ENTRAMBE dichiarano qualcosa: un contratto senza dichiarazione resta
///    legittimo (R3.4.1: «non dichiarato» e' uno stato, non un'assenza da
///    colmare) e cosi' pure una colonna senza chiavi canoniche. Quando
///    entrambe parlano e si contraddicono, il contratto e' incoerente e va
///    rifiutato subito: e' il caso in cui il dry-run accettava un contratto
///    che a runtime avrebbe letto altro.
fn validate_declared_types(
    geometry: &GeometryColumnContract,
    metadata: &HashMap<String, String>,
) -> Result<()> {
    // Ogni chiave canonica PRESENTE dev'essere sintatticamente valida, anche
    // quando il lato tipizzato del contratto tace. «Assente» e «presente ma
    // malformata» sono stati diversi (R5.1): saltare il controllo quando il
    // contratto non dichiara nulla lasciava passare `encoding = "twkb"` o
    // `dimensions = "2d"` senza che nessuno li leggesse.
    if let Some(declared) = metadata.get(PLENORA_GEOMETRY_DIMENSIONS_KEY) {
        let parsed: GeometryDimensions = declared.parse().map_err(|_| {
            PlenoraError::Schema(format!(
                "colonna geometrica `{}`: dimensions canonica non riconosciuta",
                geometry.name
            ))
        })?;
        // Il contratto la dichiara sempre (`Unknown` e' un valore canonico,
        // non un'assenza), quindi il confronto scatta sempre.
        if parsed != geometry.dimensions {
            return Err(PlenoraError::Schema(format!(
                "colonna geometrica `{}`: dimensions del contratto ({}) diversa dai metadati canonici ({declared})",
                geometry.name,
                geometry.dimensions.as_str()
            )));
        }
    }
    if let Some(declared) = metadata.get(PLENORA_GEOMETRY_ENCODING_KEY) {
        let parsed: GeometryEncoding = declared.parse().map_err(|_| {
            PlenoraError::Schema(format!(
                "colonna geometrica `{}`: encoding canonico non riconosciuto",
                geometry.name
            ))
        })?;
        // `None` nel contratto significa «non dichiarato» (R5.2), uno stato
        // legittimo: la DISCREPANZA si controlla solo quando entrambi
        // parlano, ma la validita' sintattica sopra vale comunque.
        if geometry.encoding.is_some_and(|encoding| encoding != parsed) {
            return Err(PlenoraError::Schema(format!(
                "colonna geometrica `{}`: encoding del contratto diverso dai metadati canonici ({declared})",
                geometry.name
            )));
        }
    }
    // Lo stato di risoluzione del CRS non e' confrontabile (vedi sotto), ma
    // dev'essere comunque un valore canonico: una chiave presente e
    // illeggibile non e' una chiave assente.
    if let Some(declared) = metadata.get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY) {
        declared.parse::<CrsResolution>().map_err(|_| {
            PlenoraError::Schema(format!(
                "colonna geometrica `{}`: crs_resolution canonica non riconosciuta",
                geometry.name
            ))
        })?;
    }
    // La coppia types/types_declaration si valida per intero: la
    // dichiarazione dev'essere canonica, l'elenco dev'essere parsabile con
    // quella dichiarazione (ordine, unicita', coerenza cardinalita'-stato), e
    // `types` senza `types_declaration` e' una coppia incompleta.
    let metadata_types = metadata.get(PLENORA_GEOMETRY_TYPES_KEY);
    let Some(metadata_declaration) = metadata.get(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY) else {
        if metadata_types.is_some() {
            return Err(PlenoraError::Schema(format!(
                "colonna geometrica `{}`: chiave types senza types_declaration",
                geometry.name
            )));
        }
        return Ok(());
    };
    let metadata_types = metadata_types.map_or("", String::as_str);
    let parsed_declaration: TypesDeclaration = metadata_declaration.parse().map_err(|_| {
        PlenoraError::Schema(format!(
            "colonna geometrica `{}`: types_declaration canonica non riconosciuta",
            geometry.name
        ))
    })?;
    let parsed_types =
        GeometryTypesProperty::from_canonical_list(parsed_declaration, metadata_types).map_err(
            |error| {
                PlenoraError::Schema(format!(
                    "colonna geometrica `{}`: elenco dei tipi canonico non valido: {error}",
                    geometry.name
                ))
            },
        )?;
    // Lo stato di risoluzione del CRS NON e' confrontabile qui, ed e' una
    // scelta, non una dimenticanza: il contratto puo' divergere dai metadati
    // in ENTRAMBE le direzioni, legittimamente.
    //
    // Piu' conservativo: la discovery declassa a `declared_unresolved` una
    // colonna che dichiara `resolved` con chiavi in conflitto (`crs_id` e
    // `srid` che non concordano) — e' il comportamento fail-closed voluto,
    // e il contratto e' la vista CORRETTA, non una copia dei metadati.
    //
    // Meno conservativo: una decisione di piano (`crs_decisions`, R4.6.3)
    // risolve un CRS che i metadati dichiarano `missing`; la decisione vive
    // nel piano ed entra nel `plan_hash`, quindi e' tracciata altrove.
    //
    // Non esistendo una direzione sempre valida, un confronto qui
    // rifiuterebbe contratti corretti. La coerenza del CRS resta dove ha il
    // contesto per deciderla: la discovery e la risoluzione per precedenza.
    //
    // Tipi geometrici: qui il controllo resta a DUE lati presenti, e non e'
    // una scorciatoia. Per R3.4.1 «non dichiarato» e' uno stato legittimo su
    // entrambi i lati: un contratto puo' dichiarare tipi che i metadati non
    // portano ancora (dopo un'op che li cambia, prima dell'emissione), e una
    // colonna puo' portarli senza che il contratto li abbia letti. Rendere
    // errore l'asimmetria rifiuterebbe contratti validi; la contraddizione,
    // invece, e' sempre un errore.
    let Some(declared) = geometry.types.value() else {
        return Ok(());
    };
    if parsed_declaration != declared.declaration() {
        return Err(PlenoraError::Schema(format!(
            "colonna geometrica `{}`: types_declaration del contratto ({}) diversa dai metadati canonici ({metadata_declaration})",
            geometry.name,
            declared.declaration().as_str()
        )));
    }
    let declared_types = declared.to_canonical_list();
    if parsed_types.to_canonical_list() != declared_types {
        return Err(PlenoraError::Schema(format!(
            "colonna geometrica `{}`: elenco dei tipi del contratto diverso dai metadati canonici",
            geometry.name
        )));
    }
    Ok(())
}

/// Statistica di runtime (architettura.md#planner-ed-executor).
///
/// Regola: `prepare` produce sempre un piano valido anche con statistiche
/// completamente assenti (`Unknown` → scelta conservativa); le statistiche
/// `Known`/`Estimated` possono solo migliorare scelte fisiche correggibili;
/// nessuna scelta semantica può dipendere da una statistica.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeStatistic<T> {
    /// Misurata (es. numero di righe da header Arrow IPC file format).
    Known(T),
    /// Stimata: solo scelte migliorative.
    Estimated(T),
    /// Assente: scelta conservativa obbligatoria.
    #[default]
    Unknown,
}

impl<T> RuntimeStatistic<T> {
    /// Il valore, se noto o stimato.
    pub const fn value(&self) -> Option<&T> {
        match self {
            Self::Known(value) | Self::Estimated(value) => Some(value),
            Self::Unknown => None,
        }
    }

    /// Il valore solo se misurato (`Known`).
    pub const fn known_value(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            _ => None,
        }
    }

    /// `true` solo per `Known`.
    pub const fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }
}

/// Sequenza logica di un batch (architettura.md#determinismo).
///
/// Le operazioni parallele ricompongono l'output secondo l'ordine logico
/// assegnato dal piano, mai secondo l'ordine temporale di completamento: la
/// politica `InputOrder` del catalogo significa "ordine logico delle
/// `BatchSequence` in ingresso".
///
/// `source_node` usa l'id testuale del nodo del piano (formato v4);
/// l'eventuale interning in indici compatti è una decisione fisica del
/// planner e non appartiene a questo contratto.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BatchSequence {
    /// Nodo del DAG che ha prodotto il batch.
    pub source_node: String,
    /// Partizione di input (rami paralleli della stessa sorgente).
    pub input_partition: u32,
    /// Numero di sequenza all'interno della partizione.
    pub sequence_number: u64,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::arrow::schema::{DataType, Field, Schema};
    use crate::crs::CrsKind;

    fn schema(fields: Vec<Field>) -> SchemaRef {
        Arc::new(Schema::new(fields))
    }

    fn projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    fn geometry(field_id: u32, name: &str, nullable: bool) -> GeometryColumnContract {
        GeometryColumnContract {
            field_id: FieldId(field_id),
            name: name.to_owned(),
            crs: ContractCrs::Resolved(projected_crs()),
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable,
            types: GeometryColumnContract::undeclared_types(),
        }
    }

    #[test]
    fn field_allocator_assigns_monotonic_ids_without_consuming_on_peek() {
        let mut allocator = FieldAllocator::new(41);
        assert_eq!(allocator.peek(), FieldId(41));
        assert_eq!(allocator.alloc().expect("id fresco"), FieldId(41));
        assert_eq!(allocator.alloc().expect("id fresco"), FieldId(42));
        assert_eq!(allocator.peek(), FieldId(43));
        assert_eq!(FieldAllocator::default().peek(), FieldId(0));
    }

    #[test]
    fn field_allocator_observe_avoids_collisions_with_assigned_ids() {
        let mut allocator = FieldAllocator::default();
        allocator.observe(FieldId(7));
        assert_eq!(allocator.peek(), FieldId(8));
        assert_eq!(allocator.alloc().expect("id fresco"), FieldId(8));
        // Osservare un id gia' coperto non fa arretrare il cursore.
        allocator.observe(FieldId(3));
        assert_eq!(allocator.peek(), FieldId(9));
    }

    #[test]
    fn field_allocator_intern_rename_and_derive_follow_column_identity() {
        let mut allocator = FieldAllocator::default();
        // Interning: stesso nome -> stesso id, nomi diversi -> id diversi.
        let id = allocator.intern("geom").expect("id");
        assert_eq!(allocator.intern("geom").expect("id"), id);
        assert_ne!(allocator.intern("other").expect("id"), id);

        // Rinomina: l'id segue la colonna (D16).
        allocator.rename("geom", "geometry");
        assert_eq!(allocator.intern("geometry").expect("id"), id);

        // Derivazione: nuovo id, sostituisce il binding del nome.
        let derived = allocator.derive("geometry").expect("id");
        assert_ne!(derived, id);
        assert_eq!(allocator.intern("geometry").expect("id"), derived);
        // Gli id internati non collidono con quelli osservati.
        allocator.observe(FieldId(100));
        assert!(allocator.intern("fresh").expect("id").0 > 100);
    }

    #[test]
    fn tabular_contract_is_valid_without_geometries() {
        let contract = DataContract::tabular(schema(vec![Field::new("a", DataType::Int64, false)]));
        contract.validate().unwrap();
        assert!(contract.active_geometry_column().is_none());
        assert!(contract.properties.sorted_by.is_none());
    }

    #[test]
    fn single_geometry_contract_is_valid() {
        let contract = DataContract::new(
            schema(vec![
                Field::new("a", DataType::Int64, false),
                Field::new("geom", DataType::Binary, true),
            ]),
            vec![geometry(7, "geom", true)],
            None,
            ContractProperties::default(),
        )
        .unwrap();
        let active = contract.active_geometry_column().unwrap();
        assert_eq!(active.field_id, FieldId(7));
        assert_eq!(active.name, "geom");
    }

    #[test]
    fn more_than_one_geometry_is_rejected_in_v1() {
        let result = DataContract::new(
            schema(vec![
                Field::new("g1", DataType::Binary, true),
                Field::new("g2", DataType::Binary, true),
            ]),
            vec![geometry(1, "g1", true), geometry(2, "g2", true)],
            None,
            ContractProperties::default(),
        );
        assert!(matches!(result, Err(PlenoraError::Schema(_))));
    }

    #[test]
    fn geometry_column_must_exist_with_matching_nullability() {
        let missing = DataContract::new(
            schema(vec![Field::new("a", DataType::Int64, false)]),
            vec![geometry(1, "geom", true)],
            None,
            ContractProperties::default(),
        );
        assert!(matches!(missing, Err(PlenoraError::Schema(_))));

        let nullability_mismatch = DataContract::new(
            schema(vec![Field::new("geom", DataType::Binary, false)]),
            vec![geometry(1, "geom", true)],
            None,
            ContractProperties::default(),
        );
        assert!(matches!(nullability_mismatch, Err(PlenoraError::Schema(_))));
    }

    #[test]
    fn active_geometry_must_reference_a_declared_geometry() {
        let result = DataContract::new(
            schema(vec![Field::new("geom", DataType::Binary, true)]),
            vec![geometry(1, "geom", true)],
            Some(FieldId(99)),
            ContractProperties::default(),
        );
        assert!(matches!(result, Err(PlenoraError::Schema(_))));

        let ok = DataContract::new(
            schema(vec![Field::new("geom", DataType::Binary, true)]),
            vec![geometry(1, "geom", true)],
            Some(FieldId(1)),
            ContractProperties::default(),
        )
        .unwrap();
        assert_eq!(ok.active_geometry_column().unwrap().field_id, FieldId(1));
    }

    #[test]
    fn property_confidence_exposes_proven_values_only_for_proven() {
        let declared: PropertyConfidence<u64> = PropertyConfidence::Declared(10);
        let proven = PropertyConfidence::Proven(10);
        let estimated = PropertyConfidence::Estimated(10);
        let unknown: PropertyConfidence<u64> = PropertyConfidence::Unknown;

        assert_eq!(declared.value(), Some(&10));
        assert!(!declared.is_proven());
        assert_eq!(declared.proven_value(), None);
        assert!(proven.is_proven());
        assert_eq!(proven.proven_value(), Some(&10));
        assert_eq!(estimated.proven_value(), None);
        assert_eq!(unknown.value(), None);
    }

    #[test]
    fn contract_property_combines_confidence_and_scope() {
        let property = ContractProperty::new(
            PropertyConfidence::Proven(vec![FieldId(0), FieldId(3)]),
            PropertyScope::Stream,
        );
        assert!(property.is_proven());
        assert_eq!(property.scope, PropertyScope::Stream);
        assert_eq!(property.value().unwrap().len(), 2);

        let unknown: ContractProperty<Vec<FieldId>> =
            ContractProperty::new(PropertyConfidence::Unknown, PropertyScope::Dataset);
        assert!(!unknown.is_proven());
        assert_eq!(unknown.value(), None);
    }

    #[test]
    fn runtime_statistic_distinguishes_known_estimated_unknown() {
        let known = RuntimeStatistic::Known(42_u64);
        let estimated = RuntimeStatistic::Estimated(42_u64);
        let unknown: RuntimeStatistic<u64> = RuntimeStatistic::Unknown;

        assert!(known.is_known());
        assert_eq!(known.known_value(), Some(&42));
        assert!(!estimated.is_known());
        assert_eq!(estimated.value(), Some(&42));
        assert_eq!(estimated.known_value(), None);
        assert_eq!(unknown.value(), None);
        assert_eq!(
            RuntimeStatistic::<u64>::default(),
            RuntimeStatistic::Unknown
        );
    }

    #[test]
    fn geometry_dimensions_serde_roundtrip_icd_lowercase() {
        let cases = [
            (GeometryDimensions::Xy, "xy"),
            (GeometryDimensions::Xyz, "xyz"),
            (GeometryDimensions::Xym, "xym"),
            (GeometryDimensions::Xyzm, "xyzm"),
            (GeometryDimensions::Unknown, "unknown"),
        ];
        for (dimensions, text) in cases {
            assert_eq!(dimensions.as_str(), text);
            assert_eq!(dimensions.to_string(), text);
            let serialized = serde_json::to_string(&dimensions).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: GeometryDimensions = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, dimensions);
            assert_eq!(text.parse::<GeometryDimensions>(), Ok(dimensions));
        }
    }

    #[test]
    fn geometry_dimensions_from_str_rejects_unrecognized_values() {
        // Mai default silenziosi: neppure maiuscole o vuoto (R3.4).
        for value in ["XY", "XYZ ", "", "2d", "xyzm "] {
            assert_eq!(
                value.parse::<GeometryDimensions>(),
                Err(UnknownGeometryDimensions)
            );
        }
    }

    #[test]
    fn geometry_dimensions_stride_only_when_resolved() {
        assert_eq!(GeometryDimensions::Xy.coordinate_stride(), Some(16));
        assert_eq!(GeometryDimensions::Xyz.coordinate_stride(), Some(24));
        assert_eq!(GeometryDimensions::Xym.coordinate_stride(), Some(24));
        assert_eq!(GeometryDimensions::Xyzm.coordinate_stride(), Some(32));
        // Unknown: nessuno stride garantito (R3.4).
        assert_eq!(GeometryDimensions::Unknown.coordinate_stride(), None);
    }

    #[test]
    fn geometry_encoding_serde_roundtrip_icd_lowercase() {
        let cases = [
            (GeometryEncoding::Wkb, "wkb"),
            (GeometryEncoding::Ewkb, "ewkb"),
        ];
        for (encoding, text) in cases {
            assert_eq!(encoding.as_str(), text);
            assert_eq!(encoding.to_string(), text);
            let serialized = serde_json::to_string(&encoding).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: GeometryEncoding = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, encoding);
            assert_eq!(text.parse::<GeometryEncoding>(), Ok(encoding));
        }
    }

    #[test]
    fn geometry_encoding_from_str_rejects_unrecognized_values() {
        // R3.5: enum chiuso — altri framing (GeoPackage, TWKB) sono rifiutati.
        for value in ["WKB", "gpkg", "twkb", "", "iso-wkb"] {
            assert_eq!(
                value.parse::<GeometryEncoding>(),
                Err(UnknownGeometryEncoding)
            );
        }
    }

    /// I sedici tipi canonici di §3.1, in ordine canonico (R3.1).
    const CANONICAL_TYPES: [(GeometryType, &str); 16] = [
        (GeometryType::Point, "point"),
        (GeometryType::LineString, "linestring"),
        (GeometryType::Polygon, "polygon"),
        (GeometryType::MultiPoint, "multipoint"),
        (GeometryType::MultiLineString, "multilinestring"),
        (GeometryType::MultiPolygon, "multipolygon"),
        (GeometryType::GeometryCollection, "geometrycollection"),
        (GeometryType::CircularString, "circularstring"),
        (GeometryType::CompoundCurve, "compoundcurve"),
        (GeometryType::CurvePolygon, "curvepolygon"),
        (GeometryType::MultiCurve, "multicurve"),
        (GeometryType::MultiSurface, "multisurface"),
        (GeometryType::PolyhedralSurface, "polyhedralsurface"),
        (GeometryType::Tin, "tin"),
        (GeometryType::Triangle, "triangle"),
        (GeometryType::Unknown, "unknown"),
    ];

    #[test]
    fn geometry_type_serde_roundtrip_icd_lowercase_no_separator() {
        for (geometry_type, text) in CANONICAL_TYPES {
            assert_eq!(geometry_type.as_str(), text);
            assert_eq!(geometry_type.to_string(), text);
            let serialized = serde_json::to_string(&geometry_type).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: GeometryType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, geometry_type);
            assert_eq!(text.parse::<GeometryType>(), Ok(geometry_type));
        }
    }

    #[test]
    fn geometry_type_ord_matches_canonical_r31_declaration_order() {
        // L'ordine di dichiarazione delle varianti E' l'ordine canonico di
        // §3.1: `Ord` (deriva dall'ordine di dichiarazione) e la
        // serializzazione delle liste R3.4.1 dipendono da questa invariante.
        let shuffled = [
            GeometryType::Unknown,
            GeometryType::Tin,
            GeometryType::MultiPolygon,
            GeometryType::Point,
        ];
        let mut sorted = shuffled;
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            [
                GeometryType::Point,
                GeometryType::MultiPolygon,
                GeometryType::Tin,
                GeometryType::Unknown,
            ]
        );
        for pair in CANONICAL_TYPES.windows(2) {
            assert!(pair[0].0 < pair[1].0);
        }
    }

    #[test]
    fn geometry_type_from_str_rejects_non_canonical_forms() {
        // Fail-closed: maiuscole, snake_case, spazi, vuoto e nomi ignoti
        // sono rifiutati, mai normalizzati in silenzio (R3.1/R3.2).
        for value in [
            "Point",
            "LINESTRING",
            "line_string",
            "multi_polygon",
            " point",
            "point ",
            "",
            "geomcollection",
        ] {
            assert_eq!(value.parse::<GeometryType>(), Err(UnknownGeometryType));
            assert!(serde_json::from_str::<GeometryType>(&format!("\"{value}\"")).is_err());
        }
    }

    #[test]
    fn geometry_type_from_wkb_base_type_maps_iso_codes() {
        let concrete = [
            (1, GeometryType::Point),
            (2, GeometryType::LineString),
            (3, GeometryType::Polygon),
            (4, GeometryType::MultiPoint),
            (5, GeometryType::MultiLineString),
            (6, GeometryType::MultiPolygon),
            (7, GeometryType::GeometryCollection),
            (8, GeometryType::CircularString),
            (9, GeometryType::CompoundCurve),
            (10, GeometryType::CurvePolygon),
            (11, GeometryType::MultiCurve),
            (12, GeometryType::MultiSurface),
            (15, GeometryType::PolyhedralSurface),
            (16, GeometryType::Tin),
            (17, GeometryType::Triangle),
        ];
        for (code, expected) in concrete {
            assert_eq!(GeometryType::from_wkb_base_type(code), Some(expected));
        }
        // 13/14 (curve/surface ASTRATTI, non istanziabili) e tutto il resto
        // — incluso un code con serie dimensionale non estratta (1001) — ->
        // None: il rifiuto esplicito spetta al chiamante (R3.2).
        for code in [0, 13, 14, 18, 99, 1001] {
            assert_eq!(GeometryType::from_wkb_base_type(code), None);
        }
    }

    #[test]
    fn types_declaration_serde_roundtrip_icd_lowercase() {
        let cases = [
            (TypesDeclaration::Exact, "exact"),
            (TypesDeclaration::Mixed, "mixed"),
            (TypesDeclaration::Unresolved, "unresolved"),
        ];
        for (declaration, text) in cases {
            assert_eq!(declaration.as_str(), text);
            assert_eq!(declaration.to_string(), text);
            let serialized = serde_json::to_string(&declaration).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: TypesDeclaration = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, declaration);
            assert_eq!(text.parse::<TypesDeclaration>(), Ok(declaration));
        }
        for value in ["Exact", "EXACT", "un_resolved", "", "unknown"] {
            assert_eq!(
                value.parse::<TypesDeclaration>(),
                Err(UnknownTypesDeclaration)
            );
        }
    }

    #[test]
    fn geometry_types_property_enforces_r341_coherences() {
        // exact richiede un elenco presente e non vuoto.
        assert_eq!(
            GeometryTypesProperty::new(TypesDeclaration::Exact, Vec::new()),
            Err(GeometryTypesPropertyError::ExactWithoutTypes)
        );
        // unresolved vieta l'elenco.
        assert_eq!(
            GeometryTypesProperty::new(TypesDeclaration::Unresolved, vec![GeometryType::Point]),
            Err(GeometryTypesPropertyError::UnresolvedWithTypes)
        );
        // mixed ammette l'elenco assente...
        let mixed = GeometryTypesProperty::new(TypesDeclaration::Mixed, Vec::new()).unwrap();
        assert_eq!(mixed.declaration(), TypesDeclaration::Mixed);
        assert!(mixed.types().is_empty());
        assert_eq!(mixed.to_canonical_list(), "");
        // ... e quello non vuoto.
        let mixed_with_types =
            GeometryTypesProperty::new(TypesDeclaration::Mixed, vec![GeometryType::Point]).unwrap();
        assert_eq!(mixed_with_types.to_canonical_list(), "point");
        // exact con elenco non vuoto: ok.
        let exact =
            GeometryTypesProperty::new(TypesDeclaration::Exact, vec![GeometryType::Point]).unwrap();
        assert_eq!(exact.declaration(), TypesDeclaration::Exact);
        // unresolved senza elenco: ok.
        assert!(GeometryTypesProperty::new(TypesDeclaration::Unresolved, Vec::new()).is_ok());
    }

    #[test]
    fn geometry_types_property_normalizes_unique_canonical_order() {
        let property = GeometryTypesProperty::new(
            TypesDeclaration::Exact,
            vec![
                GeometryType::MultiPolygon,
                GeometryType::Point,
                GeometryType::MultiPolygon,
                GeometryType::LineString,
            ],
        )
        .unwrap();
        assert_eq!(
            property.types(),
            &[
                GeometryType::Point,
                GeometryType::LineString,
                GeometryType::MultiPolygon,
            ]
        );
        assert_eq!(
            property.to_canonical_list(),
            "point,linestring,multipolygon"
        );
        // Una stessa dichiarazione ha una sola serializzazione (R3.4.1).
        let reordered = GeometryTypesProperty::new(
            TypesDeclaration::Exact,
            vec![
                GeometryType::LineString,
                GeometryType::MultiPolygon,
                GeometryType::Point,
            ],
        )
        .unwrap();
        assert_eq!(property, reordered);
    }

    #[test]
    fn geometry_types_property_from_canonical_list_is_fail_closed() {
        let parsed = GeometryTypesProperty::from_canonical_list(
            TypesDeclaration::Exact,
            "point,linestring,multipolygon",
        )
        .unwrap();
        assert_eq!(parsed.to_canonical_list(), "point,linestring,multipolygon");
        assert_eq!(parsed.declaration(), TypesDeclaration::Exact);

        // Stringa vuota = elenco assente (chiave non emessa): ok per mixed.
        let mixed =
            GeometryTypesProperty::from_canonical_list(TypesDeclaration::Mixed, "").unwrap();
        assert!(mixed.types().is_empty());

        // Le coerenze R3.4.1 valgono anche per la forma testuale.
        assert_eq!(
            GeometryTypesProperty::from_canonical_list(TypesDeclaration::Exact, ""),
            Err(GeometryTypesPropertyError::ExactWithoutTypes)
        );
        assert_eq!(
            GeometryTypesProperty::from_canonical_list(TypesDeclaration::Unresolved, "point"),
            Err(GeometryTypesPropertyError::UnresolvedWithTypes)
        );

        // Fail-closed: spazi, maiuscole, snake_case, token vuoti, duplicati
        // e ordine non canonico sono errori, mai correzioni silenziose.
        for list in [
            "point, polygon",
            "Point",
            "line_string",
            "point,,polygon",
            "point,",
        ] {
            assert_eq!(
                GeometryTypesProperty::from_canonical_list(TypesDeclaration::Exact, list),
                Err(GeometryTypesPropertyError::UnknownTypeInList)
            );
        }
        assert_eq!(
            GeometryTypesProperty::from_canonical_list(TypesDeclaration::Exact, "point,point"),
            Err(GeometryTypesPropertyError::DuplicateTypeInList)
        );
        assert_eq!(
            GeometryTypesProperty::from_canonical_list(TypesDeclaration::Exact, "polygon,point"),
            Err(GeometryTypesPropertyError::NonCanonicalOrder)
        );
    }

    #[test]
    fn axis_order_serde_roundtrip_icd() {
        let cases = [
            (AxisOrder::LonLat, "lon_lat"),
            (AxisOrder::LatLon, "lat_lon"),
            (AxisOrder::EastingNorthing, "easting_northing"),
            (AxisOrder::NorthingEasting, "northing_easting"),
            (AxisOrder::Other, "other"),
            (AxisOrder::Unknown, "unknown"),
        ];
        for (axis_order, text) in cases {
            assert_eq!(axis_order.as_str(), text);
            assert_eq!(axis_order.to_string(), text);
            let serialized = serde_json::to_string(&axis_order).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: AxisOrder = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, axis_order);
            assert_eq!(text.parse::<AxisOrder>(), Ok(axis_order));
        }
        for value in ["lonlat", "LON_LAT", "lon lat", "", "xy"] {
            assert_eq!(value.parse::<AxisOrder>(), Err(UnknownAxisOrder));
        }
    }

    #[test]
    fn crs_resolution_serde_roundtrip_icd() {
        let cases = [
            (CrsResolution::Resolved, "resolved"),
            (CrsResolution::DeclaredUnresolved, "declared_unresolved"),
            (CrsResolution::Missing, "missing"),
        ];
        for (resolution, text) in cases {
            assert_eq!(resolution.as_str(), text);
            assert_eq!(resolution.to_string(), text);
            let serialized = serde_json::to_string(&resolution).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: CrsResolution = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, resolution);
            assert_eq!(text.parse::<CrsResolution>(), Ok(resolution));
        }
        for value in [
            "declaredunresolved",
            "DECLARED_UNRESOLVED",
            "",
            "unresolved",
        ] {
            assert_eq!(value.parse::<CrsResolution>(), Err(UnknownCrsResolution));
        }
    }

    #[test]
    fn crs_definition_format_serde_roundtrip_icd() {
        let cases = [
            (CrsDefinitionFormat::Wkt, "wkt"),
            (CrsDefinitionFormat::Wkt2, "wkt2"),
            (CrsDefinitionFormat::Projjson, "projjson"),
        ];
        for (format, text) in cases {
            assert_eq!(format.as_str(), text);
            assert_eq!(format.to_string(), text);
            let serialized = serde_json::to_string(&format).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: CrsDefinitionFormat = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, format);
            assert_eq!(text.parse::<CrsDefinitionFormat>(), Ok(format));
        }
        for value in ["WKT", "wkt1", "proj_json", "", "wkt 2"] {
            assert_eq!(
                value.parse::<CrsDefinitionFormat>(),
                Err(UnknownCrsDefinitionFormat)
            );
        }
    }

    #[test]
    fn spatial_semantics_serde_roundtrip_icd() {
        let cases = [
            (SpatialSemantics::Geometry, "geometry"),
            (SpatialSemantics::Geography, "geography"),
        ];
        for (semantics, text) in cases {
            assert_eq!(semantics.as_str(), text);
            assert_eq!(semantics.to_string(), text);
            let serialized = serde_json::to_string(&semantics).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: SpatialSemantics = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, semantics);
            assert_eq!(text.parse::<SpatialSemantics>(), Ok(semantics));
        }
        for value in ["Geometry", "GEOGRAPHY", "geo", ""] {
            assert_eq!(
                value.parse::<SpatialSemantics>(),
                Err(UnknownSpatialSemantics)
            );
        }
    }

    #[test]
    fn geometry_precision_serde_roundtrip_icd() {
        let cases = [
            (GeometryPrecision::Float64, "float64"),
            (GeometryPrecision::Float32, "float32"),
            (GeometryPrecision::Native, "native"),
        ];
        for (precision, text) in cases {
            assert_eq!(precision.as_str(), text);
            assert_eq!(precision.to_string(), text);
            let serialized = serde_json::to_string(&precision).unwrap();
            assert_eq!(serialized, format!("\"{text}\""));
            let parsed: GeometryPrecision = serde_json::from_str(&serialized).unwrap();
            assert_eq!(parsed, precision);
            assert_eq!(text.parse::<GeometryPrecision>(), Ok(precision));
        }
        for value in ["f64", "FLOAT64", "float_64", "double", ""] {
            assert_eq!(
                value.parse::<GeometryPrecision>(),
                Err(UnknownGeometryPrecision)
            );
        }
    }

    #[test]
    fn geometry_column_types_default_is_undeclared_not_unresolved() {
        // R3.4.1: un ingresso legacy privo delle chiavi significa «proprieta'
        // non dichiarata» (confidence Unknown), MAI `unresolved`.
        let default = GeometryColumnContract::undeclared_types();
        assert_eq!(default.value(), None);
        assert!(!default.is_proven());
        assert_eq!(default.scope, PropertyScope::Schema);
        // Il default e' quello usato dai costruttori esistenti.
        let column = geometry(1, "geom", true);
        assert!(column.types.value().is_none());
        // Un contratto col default resta valido (comportamento invariato).
        let contract = DataContract::new(
            schema(vec![Field::new("geom", DataType::Binary, true)]),
            vec![column],
            None,
            ContractProperties::default(),
        )
        .unwrap();
        assert!(contract
            .active_geometry_column()
            .unwrap()
            .types
            .value()
            .is_none());
    }

    #[test]
    fn batch_sequence_carries_logical_order() {
        let sequence = BatchSequence {
            source_node: "n0".to_owned(),
            input_partition: 2,
            sequence_number: 17,
        };
        assert_eq!(sequence.source_node, "n0");
        assert_eq!(sequence.input_partition, 2);
        assert_eq!(sequence.sequence_number, 17);
    }
}
