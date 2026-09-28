//! Contratti dati del grafo (architettura.md, decisioni D6, D16,
//! D25; architettura.md#determinismo, architettura.md#planner-ed-executor).
//!
//! I tipi che descrivono cio' che scorre sugli archi del DAG: il
//! `DataContract`, l'identita' stabile delle colonne (`FieldId`), provenienza
//! e scope delle proprieta', le statistiche di runtime e la sequenza logica
//! dei batch (`BatchSequence`).
//!
//! Struttura aperta, comportamento chiuso: il modello ammette piu' colonne
//! geometriche, ma [`DataContract::validate`] ne rifiuta piu' di una (D16).
//! Le combinazioni confidence/scope prive di senso non sono escluse dai
//! tipi: la rappresentazione e' la coppia `ContractProperty<T> { confidence,
//! scope }`.

pub mod arrow_metadata;
pub mod arrow_schema;

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::arrow::schema::DataType;
use crate::arrow::SchemaRef;
use crate::crs::ResolvedCrs;
use crate::error::{PlenoraError, Result};

/// Genera un enum ICD a forma testuale chiusa: l'enum con i suoi attributi
/// (derive e `serde` compresi, passati cosi' come sono), `as_str`,
/// `Display`, l'elenco `ALL`, l'errore di parsing e `FromStr`.
///
/// Varianti, forme testuali ed elenco «ammessi» del messaggio d'errore
/// nascono dalla stessa lista: un valore nuovo non puo' mancare dal
/// parsing ne' dal messaggio. La concordanza di `as_str` con `rename_all`
/// di `serde` non si ricava dalla macro: la verificano i test per ogni
/// variante di `ALL`.
///
/// Il messaggio d'errore e' `"<descrizione> (<ammessi>: <forme>)"`:
/// descrizione e parola «ammessi» sono parametri perche' l'accordo di
/// genere dipende dal nome. Il messaggio non riporta mai l'input (regola
/// «errori senza dati» di `plenora-core`).
macro_rules! enum_icd {
    (
        $(#[$meta_enum:meta])*
        $nome_enum:ident,
        errore $nome_errore:ident = $descrizione:literal,
        $ammessi:literal {
            $(
                $(#[$attributo:meta])*
                $variante:ident => $forma:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$meta_enum])*
        pub enum $nome_enum {
            $(
                $(#[$attributo])*
                $variante,
            )+
        }

        impl $nome_enum {
            /// Tutte le varianti, in ordine di dichiarazione.
            pub const ALL: &'static [Self] = &[$(Self::$variante),+];

            /// Forma testuale ICD. Coincide con la serializzazione serde.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variante => $forma,)+
                }
            }
        }

        impl fmt::Display for $nome_enum {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        #[doc = concat!(
            "Errore di parsing di [`",
            stringify!($nome_enum),
            "`]: valore non riconosciuto.",
        )]
        ///
        /// Il messaggio elenca i valori ammessi e non riporta l'input (regola
        /// «errori senza dati» di `plenora-core`).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $nome_errore;

        impl fmt::Display for $nome_errore {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(enum_icd!(@messaggio $descrizione, $ammessi; $($forma),+))
            }
        }

        impl std::error::Error for $nome_errore {}

        impl std::str::FromStr for $nome_enum {
            type Err = $nome_errore;

            fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
                match value {
                    $($forma => Ok(Self::$variante),)+
                    _ => Err($nome_errore),
                }
            }
        }
    };
    (@messaggio $descrizione:literal, $ammessi:literal; $prima:literal $(, $altra:literal)*) => {
        concat!($descrizione, " (", $ammessi, ": ", $prima, $(", ", $altra,)* ")")
    };
}

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

enum_icd! {
    /// Dimensionalità delle geometrie di una colonna (ICD §3.3).
    ///
    /// Il contratto rappresenta e propaga la dimensionalita', non la elabora.
    ///
    /// `Unknown` significa «byte preservati, dimensionalita' non risolta» e non
    /// va mai mappato a [`GeometryDimensions::Xy`] (R3.4): nasconderebbe
    /// geometrie Z/M dietro un contratto 2D.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    GeometryDimensions,
    errore UnknownGeometryDimensions = "dimensionalita' geometria non riconosciuta",
    "ammesse" {
        Xy => "xy",
        Xyz => "xyz",
        Xym => "xym",
        Xyzm => "xyzm",
        /// Byte preservati, dimensionalità non risolta: mai mappare a `Xy` (R3.4).
        Unknown => "unknown",
    }
}

impl GeometryDimensions {
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

enum_icd! {
    /// Framing binario delle celle geometria (ICD §3.3, regola R3.5: enum
    /// chiuso).
    ///
    /// Solo WKB ISO ed EWKB (`PostGIS`, con SRID/flag Z/M): altri framing
    /// (`GeoPackage`, TWKB, …) la discovery li rifiuta, mai mappati a un encoding
    /// noto.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    GeometryEncoding,
    errore UnknownGeometryEncoding = "encoding geometria non riconosciuto",
    "ammessi" {
        Wkb => "wkb",
        Ewkb => "ewkb",
    }
}

enum_icd! {
    /// Tipo geometrico canonico di una colonna (ICD §3.1, regola R3.1).
    ///
    /// Serializzati in minuscolo senza separatore (`linestring`), come i sistemi
    /// esterni (`PostGIS`, `GeoPackage`, WKT): ai confini non serve traduzione.
    /// Un componente puo' supportarne un sottoinsieme, ma rifiuta esplicitamente
    /// gli altri (R3.2).
    ///
    /// INVARIANTE: l'ordine delle varianti e' l'ordine canonico di §3.1. `Ord`
    /// ne deriva e la serializzazione canonica delle liste di tipi (R3.4.1,
    /// [`GeometryTypesProperty`]) ne dipende: non riordinare le varianti.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    GeometryType,
    errore UnknownGeometryType = "tipo geometrico non riconosciuto",
    "ammessi" {
        Point => "point",
        LineString => "linestring",
        Polygon => "polygon",
        MultiPoint => "multipoint",
        MultiLineString => "multilinestring",
        MultiPolygon => "multipolygon",
        GeometryCollection => "geometrycollection",
        CircularString => "circularstring",
        CompoundCurve => "compoundcurve",
        CurvePolygon => "curvepolygon",
        MultiCurve => "multicurve",
        MultiSurface => "multisurface",
        PolyhedralSurface => "polyhedralsurface",
        Tin => "tin",
        Triangle => "triangle",
        /// Tipo non risolto (R3.1): mai degradato a un tipo noto.
        Unknown => "unknown",
    }
}

impl GeometryType {
    /// Mappa il type code WKB base ISO (senza la serie dimensionale 1000+
    /// ne' i flag EWKB, gia' estratti dal chiamante) al tipo canonico.
    ///
    /// Restituisce `None` per 13 e 14 (`curve`/`surface`, astratti, mai sul
    /// filo) e per ogni codice sconosciuto: il rifiuto spetta al chiamante
    /// (R3.2).
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

enum_icd! {
    /// Stato di dichiarazione dei tipi geometrici di una colonna (ICD R3.4.1,
    /// chiave canonica `plenora.geometry.types_declaration`).
    ///
    /// `Mixed` significa tipi diversi **per dichiarazione** (per esempio una
    /// colonna `PostGIS` `geometry` senza vincolo): e' informazione, non
    /// ignoranza. `Unresolved` significa byte non ispezionati e nessuna
    /// dichiarazione.
    ///
    /// R3.4.1 vieta le conversioni `mixed` ↔ `unresolved`. Un input legacy senza
    /// le chiavi `types`/`types_declaration` non e' `Unresolved` ma «proprieta'
    /// non dichiarata»: si preserva o si normalizza con un `LossReport`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    TypesDeclaration,
    errore UnknownTypesDeclaration = "types_declaration non riconosciuta",
    "ammesse" {
        Exact => "exact",
        Mixed => "mixed",
        Unresolved => "unresolved",
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
/// Le coerenze di R3.4.1 sono imposte per costruzione (campi privati, unico
/// ingresso [`GeometryTypesProperty::new`]): `Exact` richiede un elenco non
/// vuoto, `Unresolved` lo vieta, `Mixed` lo ammette (vuoto conta come
/// assente).
///
/// `new` normalizza l'elenco, cosi' una dichiarazione ha una sola
/// serializzazione; il parsing ([`GeometryTypesProperty::from_canonical_list`])
/// e' invece fail-closed su spazi, duplicati e ordine non canonico.
///
/// Niente serde derivato: sul filo c'e' la coppia di chiavi R2.2, e una
/// `Deserialize` derivata aggirerebbe il validatore.
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

enum_icd! {
    /// Ordine degli assi del CRS (chiave canonica `plenora.geometry.axis_order`,
    /// tabella R2.2). Serializzazione ICD minuscola con `_`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    AxisOrder,
    errore UnknownAxisOrder = "ordine assi non riconosciuto",
    "ammessi" {
        LonLat => "lon_lat",
        LatLon => "lat_lon",
        EastingNorthing => "easting_northing",
        NorthingEasting => "northing_easting",
        Other => "other",
        Unknown => "unknown",
    }
}

enum_icd! {
    /// Stato di risoluzione del CRS (chiave canonica
    /// `plenora.geometry.crs_resolution`, tabella R2.2). Serializzazione ICD
    /// minuscola con `_`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    CrsResolution,
    errore UnknownCrsResolution = "risoluzione CRS non riconosciuta",
    "ammesse" {
        Resolved => "resolved",
        DeclaredUnresolved => "declared_unresolved",
        Missing => "missing",
    }
}

enum_icd! {
    /// Formato testuale della definizione CRS (chiave canonica
    /// `plenora.geometry.crs_definition_format`, tabella R2.2).
    /// Serializzazione ICD minuscola.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    CrsDefinitionFormat,
    errore UnknownCrsDefinitionFormat = "formato definizione CRS non riconosciuto",
    "ammessi" {
        Wkt => "wkt",
        Wkt2 => "wkt2",
        Projjson => "projjson",
    }
}

enum_icd! {
    /// Semantica spaziale della colonna (chiave canonica
    /// `plenora.geometry.spatial_semantics`, tabella R2.2). Serializzazione ICD
    /// minuscola.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    SpatialSemantics,
    errore UnknownSpatialSemantics = "semantica spaziale non riconosciuta",
    "ammesse" {
        Geometry => "geometry",
        Geography => "geography",
    }
}

enum_icd! {
    /// Precisione delle coordinate (chiave canonica
    /// `plenora.geometry.precision`, tabella R2.2). Serializzazione ICD
    /// minuscola.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    GeometryPrecision,
    errore UnknownGeometryPrecision = "precisione geometria non riconosciuta",
    "ammesse" {
        Float64 => "float64",
        Float32 => "float32",
        Native => "native",
    }
}

/// CRS di una colonna geometrica nel contratto (R4.1: gli stati di
/// risoluzione non si collassano; R4.4: mai un CRS inventato).
///
/// - [`ContractCrs::Resolved`]: definizione risolta contro PROJ dalla
///   discovery;
/// - [`ContractCrs::ResolvedByDecision`]: risolta allo stesso modo, ma per
///   una decisione esplicita del piano (R4.6.3, `crs_decisions`) su uno stato
///   `DeclaredUnresolved`. Conta per l'emissione, dove il CRS deciso
///   sostituisce le dichiarazioni in conflitto della sorgente; altrove e' un
///   CRS risolto a tutti gli effetti;
/// - [`ContractCrs::DeclaredUnresolved`]: il CRS c'e' ma non si risolve
///   (dichiarato cosi', o dichiarazioni in conflitto). Senza decisione del
///   piano l'incoerenza si propaga (R4.6.3) e arriva al bordo di scrittura con
///   le dichiarazioni originali (R4.6.4): per questo la variante porta
///   `crs_id` e `definition` col suo formato (R4.3). Lo `srid` viaggia come
///   lineage nei metadati (R2.4). La discovery la costruisce solo con almeno
///   una rappresentazione dichiarata fra `crs_id`, `definition` e `srid`
///   (R4.1, R4.3.1): col solo `srid` i due campi sono assenti;
/// - [`ContractCrs::Missing`]: nessun CRS dichiarato. Si propaga negli
///   output (`plenora.geometry.crs_resolution = missing`) e ferma solo le op
///   con un `CrsRequirement`, in analyze.
///
/// `DeclaredUnresolved` ferma le stesse op nello stesso punto e con la
/// stessa categoria (`Crs`) di `Missing`, ma con un messaggio distinto: la
/// colonna dichiara un'incoerenza, non un'assenza.
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
    /// dai metadati: `None` quando la sorgente non dichiara un `encoding` —
    /// mai un default silenzioso. I framing fuori
    /// dall'enum chiuso non sono rappresentabili: la discovery li rifiuta
    /// con errore esplicito prima di costruire il contratto.
    pub encoding: Option<GeometryEncoding>,
    pub nullable: bool,
    /// Dichiarazione dei tipi geometrici della colonna (chiavi canoniche
    /// `plenora.geometry.types` + `plenora.geometry.types_declaration`,
    /// R2.2/R3.4.1).
    ///
    /// Default: [`GeometryColumnContract::undeclared_types`], cioe'
    /// «proprieta' non dichiarata», distinta da `TypesDeclaration::Unresolved`
    /// (vedi [`TypesDeclaration`]).
    pub types: ContractProperty<GeometryTypesProperty>,
}

impl GeometryColumnContract {
    /// Default del campo `types` per i contratti costruiti senza ispezione
    /// dei tipi: proprieta' non dichiarata (confidence `Unknown`, scope
    /// `Schema`).
    ///
    /// Non equivale a `Declared(TypesDeclaration::Unresolved)` (R3.4.1).
    #[must_use]
    pub const fn undeclared_types() -> ContractProperty<GeometryTypesProperty> {
        ContractProperty::new(PropertyConfidence::Unknown, PropertyScope::Schema)
    }
}

/// Assegnatore di [`FieldId`] nel namespace globale del grafo (decisione D16).
///
/// Dentro un grafo l'allocatore e' uno solo, condiviso da planner e moduli
/// `analyze_contract`: due allocatori non coordinati darebbero lo stesso ID a
/// colonne diverse.
///
/// Il planner rimappa le geometrie di input con [`FieldAllocator::alloc`]
/// (senza legare i nomi: gli input possono avere colonne omonime);
/// [`FieldAllocator::observe`] registra gli ID gia' presenti negli input di un
/// nodo. Le colonne propagate tengono l'ID, le derivate
/// ([`FieldAllocator::derive`]) ne ricevono uno nuovo, le rinomine lo spostano
/// ([`FieldAllocator::rename`]); [`FieldAllocator::intern`] rende stabile
/// l'ID di una colonna per nome (chiavi `sorted_by`).
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
    /// esaurito: un incremento saturante renderebbe lo stesso id due volte, e
    /// due colonne condividerebbero l'identita' (D16).
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

/// Proprietà tipizzate del contratto: non un framework generico, le nuove
/// proprietà si aggiungono come campi tipizzati.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContractProperties {
    /// Chiavi di ordinamento dichiarate/dimostrate, come `FieldId` nel
    /// namespace globale del grafo.
    pub sorted_by: Option<ContractProperty<Vec<FieldId>>>,
    /// Cardinalità nota o stimata dell'arco: mai `Proven` nella validazione
    /// statica, perché non è dimostrabile dagli header (D8, architettura.md).
    pub row_count: Option<ContractProperty<u64>>,
}

/// Contratto di un arco del DAG (decisioni D6, D16).
///
/// Ogni arco trasporta `RecordBatch` conformi a questo contratto, inferito a
/// secco dal planner (`analyze_contract` di ogni operazione).
#[derive(Clone, Debug)]
pub struct DataContract {
    pub schema: SchemaRef,
    /// Al massimo una colonna geometrica (validato, decisione D16).
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
    /// - nomi dei campi univoci (altrimenti `field_with_name` risolve al primo
    ///   omonimo);
    /// - al massimo una colonna geometrica (D16);
    /// - ogni colonna geometrica esiste nello schema con lo stesso nome, la
    ///   stessa nullability e tipo fisico `Binary`, che ogni lettore della
    ///   geometria presuppone;
    /// - se contratto e metadati canonici dichiarano entrambi i tipi
    ///   geometrici, coincidono;
    /// - `active_geometry`, se presente, riferisce una colonna geometrica
    ///   dichiarata.
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
/// 1. Ogni chiave canonica presente dev'essere leggibile, qualunque cosa
///    dichiari il contratto: «presente ma malformata» non e' «assente».
/// 2. Le due fonti si confrontano solo quando entrambe dichiarano qualcosa
///    (R3.4.1: «non dichiarato» e' uno stato legittimo); se si
///    contraddicono, il contratto e' rifiutato.
fn validate_declared_types(
    geometry: &GeometryColumnContract,
    metadata: &HashMap<String, String>,
) -> Result<()> {
    // Ogni chiave canonica PRESENTE dev'essere sintatticamente valida, anche
    // quando il lato tipizzato del contratto tace. «Assente» e «presente ma
    // malformata» sono stati diversi (R5.1): saltare il controllo quando il
    // contratto non dichiara nulla lascerebbe passare `encoding = "twkb"` o
    // `dimensions = "2d"` senza che nessuno li legga.
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
    // Lo stato di risoluzione del CRS non si confronta qui: il contratto puo'
    // divergere legittimamente dai metadati in entrambe le direzioni (la
    // discovery declassa un `resolved` con chiavi in conflitto; una
    // decisione di piano, R4.6.3, risolve un `declared_unresolved`). La coerenza del CRS
    // la decidono discovery e risoluzione per precedenza.
    //
    // Tipi geometrici: il confronto scatta solo con entrambi i lati presenti,
    // perche' «non dichiarato» e' legittimo su ciascun lato (R3.4.1); la
    // contraddizione e' sempre un errore.
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

    /// Oracolo comune degli enum di `enum_icd!`: per ogni variante di `ALL`,
    /// `Display` e serializzazione serde (`rename_all`, scritta a parte dalla
    /// macro) coincidono con `as_str`, e serde e `FromStr` fanno roundtrip;
    /// ogni forma di `rifiutate` e' respinta da `FromStr` con `errore`. I
    /// messaggi nominano il tipo, perche' un solo test li percorre tutti.
    fn assert_forma_icd<T>(
        all: &[T],
        as_str: fn(T) -> &'static str,
        rifiutate: &[&str],
        errore: T::Err,
    ) where
        T: Copy
            + std::fmt::Debug
            + std::fmt::Display
            + PartialEq
            + Serialize
            + serde::de::DeserializeOwned
            + std::str::FromStr,
        T::Err: Copy + std::fmt::Debug + PartialEq,
    {
        let tipo = std::any::type_name::<T>();
        assert!(!all.is_empty(), "{tipo}: ALL vuoto");
        for &value in all {
            let text = as_str(value);
            assert_eq!(value.to_string(), text, "{tipo}: Display");
            let serialized = serde_json::to_string(&value)
                .unwrap_or_else(|errore| panic!("{tipo}: serializzazione: {errore}"));
            assert_eq!(serialized, format!("\"{text}\""), "{tipo}: serde");
            let parsed: T = serde_json::from_str(&serialized)
                .unwrap_or_else(|errore| panic!("{tipo}: deserializzazione: {errore}"));
            assert_eq!(parsed, value, "{tipo}: roundtrip serde");
            assert_eq!(text.parse::<T>().ok(), Some(value), "{tipo}: FromStr");
        }
        for &value in rifiutate {
            assert_eq!(value.parse::<T>(), Err(errore), "{tipo}: {value:?}");
        }
    }

    /// Nove enum di `enum_icd!` in una tabella. I rifiuti di
    /// `GeometryDimensions`, `GeometryEncoding` e `GeometryType` stanno nei
    /// test dedicati sotto, con le norme che li motivano.
    #[test]
    fn gli_enum_icd_fanno_roundtrip_e_rifiutano_le_forme_non_canoniche() {
        assert_forma_icd(
            GeometryDimensions::ALL,
            GeometryDimensions::as_str,
            &[],
            UnknownGeometryDimensions,
        );
        assert_forma_icd(
            GeometryEncoding::ALL,
            GeometryEncoding::as_str,
            &[],
            UnknownGeometryEncoding,
        );
        assert_forma_icd(
            GeometryType::ALL,
            GeometryType::as_str,
            &[],
            UnknownGeometryType,
        );
        assert_forma_icd(
            TypesDeclaration::ALL,
            TypesDeclaration::as_str,
            &["Exact", "EXACT", "un_resolved", "", "unknown"],
            UnknownTypesDeclaration,
        );
        assert_forma_icd(
            AxisOrder::ALL,
            AxisOrder::as_str,
            &["lonlat", "LON_LAT", "lon lat", "", "xy"],
            UnknownAxisOrder,
        );
        assert_forma_icd(
            CrsResolution::ALL,
            CrsResolution::as_str,
            &[
                "declaredunresolved",
                "DECLARED_UNRESOLVED",
                "",
                "unresolved",
            ],
            UnknownCrsResolution,
        );
        assert_forma_icd(
            CrsDefinitionFormat::ALL,
            CrsDefinitionFormat::as_str,
            &["WKT", "wkt1", "proj_json", "", "wkt 2"],
            UnknownCrsDefinitionFormat,
        );
        assert_forma_icd(
            SpatialSemantics::ALL,
            SpatialSemantics::as_str,
            &["Geometry", "GEOGRAPHY", "geo", ""],
            UnknownSpatialSemantics,
        );
        assert_forma_icd(
            GeometryPrecision::ALL,
            GeometryPrecision::as_str,
            &["f64", "FLOAT64", "float_64", "double", ""],
            UnknownGeometryPrecision,
        );
    }

    /// Elenco «ammessi» atteso: le forme di `ALL` separate da `, `.
    fn elenco_forme<T: Copy>(all: &[T], as_str: fn(T) -> &'static str) -> String {
        all.iter()
            .map(|&value| as_str(value))
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[test]
    fn enum_icd_error_messages_derive_the_admitted_forms() {
        // Testi scritti a mano, identici byte per byte a quelli precedenti
        // la macro: l'elenco e' derivato, il testo osservabile non cambia.
        let cases = [
            (
                UnknownGeometryDimensions.to_string(),
                "dimensionalita' geometria non riconosciuta (ammesse: xy, xyz, xym, xyzm, unknown)",
                elenco_forme(GeometryDimensions::ALL, GeometryDimensions::as_str),
            ),
            (
                UnknownGeometryEncoding.to_string(),
                "encoding geometria non riconosciuto (ammessi: wkb, ewkb)",
                elenco_forme(GeometryEncoding::ALL, GeometryEncoding::as_str),
            ),
            (
                UnknownGeometryType.to_string(),
                "tipo geometrico non riconosciuto (ammessi: point, linestring, polygon, multipoint, multilinestring, multipolygon, geometrycollection, circularstring, compoundcurve, curvepolygon, multicurve, multisurface, polyhedralsurface, tin, triangle, unknown)",
                elenco_forme(GeometryType::ALL, GeometryType::as_str),
            ),
            (
                UnknownTypesDeclaration.to_string(),
                "types_declaration non riconosciuta (ammesse: exact, mixed, unresolved)",
                elenco_forme(TypesDeclaration::ALL, TypesDeclaration::as_str),
            ),
            (
                UnknownAxisOrder.to_string(),
                "ordine assi non riconosciuto (ammessi: lon_lat, lat_lon, easting_northing, northing_easting, other, unknown)",
                elenco_forme(AxisOrder::ALL, AxisOrder::as_str),
            ),
            (
                UnknownCrsResolution.to_string(),
                "risoluzione CRS non riconosciuta (ammesse: resolved, declared_unresolved, missing)",
                elenco_forme(CrsResolution::ALL, CrsResolution::as_str),
            ),
            (
                UnknownCrsDefinitionFormat.to_string(),
                "formato definizione CRS non riconosciuto (ammessi: wkt, wkt2, projjson)",
                elenco_forme(CrsDefinitionFormat::ALL, CrsDefinitionFormat::as_str),
            ),
            (
                UnknownSpatialSemantics.to_string(),
                "semantica spaziale non riconosciuta (ammesse: geometry, geography)",
                elenco_forme(SpatialSemantics::ALL, SpatialSemantics::as_str),
            ),
            (
                UnknownGeometryPrecision.to_string(),
                "precisione geometria non riconosciuta (ammesse: float64, float32, native)",
                elenco_forme(GeometryPrecision::ALL, GeometryPrecision::as_str),
            ),
        ];
        for (message, expected, forms) in cases {
            assert_eq!(message, expected);
            assert!(message.ends_with(&format!(": {forms})")), "{message}");
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
        // `ALL` segue l'ordine canonico di §3.1, oracolo scritto a parte.
        assert_eq!(GeometryType::ALL.len(), CANONICAL_TYPES.len());
        for (&geometry_type, (expected, text)) in GeometryType::ALL.iter().zip(CANONICAL_TYPES) {
            assert_eq!(geometry_type, expected);
            assert_eq!(geometry_type.as_str(), text);
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
