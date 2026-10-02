//! Contratti dati: che cosa una tabella promette prima di vedere i dati.
//!
//! I tipi che descrivono le tabelle fra un passo e l'altro: il
//! [`DataContract`] (schema, colonna geometria, proprietà), l'identità
//! stabile delle colonne ([`FieldId`]), provenienza e ambito delle
//! proprietà, e due tipi del progetto d'origine che qui nessun codice usa
//! ([`RuntimeStatistic`], [`BatchSequence`]). L'analisi dei kernel
//! (`analyze_table_contract`, l'analisi geo) calcola il contratto d'uscita
//! di un passo da quelli d'ingresso e dalla config; [`arrow_schema`] e
//! [`arrow_metadata`] lo leggono da uno schema Arrow e lo riscrivono nei
//! metadati.
//!
//! Struttura aperta, comportamento chiuso: il modello ammette più colonne
//! geometriche, ma [`DataContract::validate`] ne rifiuta più di una. Le
//! combinazioni confidence/scope prive di senso non sono escluse dai tipi:
//! la rappresentazione è la coppia `ContractProperty<T> { confidence, scope
//! }`.
//!
//! I valori testuali dei metadati `plenora.geometry.*` (tipi, dimensioni,
//! encoding, CRS) seguono il contratto d'interfaccia del progetto
//! d'origine: forme chiuse, minuscole, mai un default silenzioso per un
//! valore non riconosciuto.

pub mod arrow_metadata;
pub mod arrow_schema;

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::arrow::schema::DataType;
use crate::arrow::SchemaRef;
use crate::crs::ResolvedCrs;
use crate::error::{PlenoraError, Result};

/// Genera un enum a forma testuale chiusa: l'enum con i suoi attributi
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

            /// Forma testuale canonica. Coincide con la serializzazione serde.
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

/// Identità logica stabile di una colonna nel piano.
///
/// Namespace unico per piano: la validazione del runner assegna gli ID con
/// un solo [`FieldAllocator`] (e rimappa le geometrie degli input), così due
/// input non possono collidere. Una rinomina preserva il `FieldId`; una
/// colonna calcolata o derivata ne riceve uno nuovo; un join eredita i
/// `FieldId` dei rispettivi lati.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldId(pub u32);

impl fmt::Display for FieldId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "field#{}", self.0)
    }
}

enum_icd! {
    /// Dimensionalità delle geometrie di una colonna (chiave
    /// `plenora.geometry.dimensions`).
    ///
    /// Il contratto rappresenta e propaga la dimensionalità, non la elabora.
    ///
    /// `Unknown` significa «byte preservati, dimensionalità non risolta» e non
    /// va mai mappato a [`GeometryDimensions::Xy`]: nasconderebbe geometrie
    /// Z/M dietro un contratto 2D.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    GeometryDimensions,
    errore UnknownGeometryDimensions = "dimensionalita' geometria non riconosciuta",
    "ammesse" {
        /// Due coordinate.
        Xy => "xy",
        /// Con quota Z.
        Xyz => "xyz",
        /// Con misura M.
        Xym => "xym",
        /// Con quota Z e misura M.
        Xyzm => "xyzm",
        /// Byte preservati, dimensionalità non risolta: mai mappare a `Xy`.
        Unknown => "unknown",
    }
}

impl GeometryDimensions {
    /// Byte per coordinata interleaved (`f64`), se garantiti: `Xy` = 16,
    /// `Xyz`/`Xym` = 24, `Xyzm` = 32.
    ///
    /// `Unknown` non garantisce alcuno stride (i byte sono preservati ma la
    /// dimensionalità non è risolta) e restituisce `None`: nessun
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
    /// Framing binario delle celle geometria (chiave
    /// `plenora.geometry.encoding`): enum chiuso.
    ///
    /// Solo WKB ISO ed EWKB (`PostGIS`, con SRID/flag Z/M): altri framing
    /// (`GeoPackage`, TWKB, …) la lettura del contratto li rifiuta, mai
    /// mappati a un encoding noto.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    GeometryEncoding,
    errore UnknownGeometryEncoding = "encoding geometria non riconosciuto",
    "ammessi" {
        /// WKB ISO.
        Wkb => "wkb",
        /// EWKB di `PostGIS`, con SRID e flag Z/M.
        Ewkb => "ewkb",
    }
}

enum_icd! {
    /// Tipo geometrico canonico di una colonna: i quindici tipi del WKB ISO
    /// più `Unknown`.
    ///
    /// Serializzati in minuscolo senza separatore (`linestring`), come i sistemi
    /// esterni (`PostGIS`, `GeoPackage`, WKT): ai confini non serve traduzione.
    /// Un componente può supportarne un sottoinsieme, ma rifiuta
    /// esplicitamente gli altri.
    ///
    /// INVARIANTE: l'ordine delle varianti è l'ordine canonico. `Ord` ne
    /// deriva e la serializzazione canonica delle liste di tipi
    /// ([`GeometryTypesProperty`]) ne dipende: non riordinare le varianti.
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
        /// Tipo non risolto: mai degradato a un tipo noto.
        Unknown => "unknown",
    }
}

impl GeometryType {
    /// Mappa il type code WKB base ISO (senza la serie dimensionale 1000+
    /// ne' i flag EWKB, gia' estratti dal chiamante) al tipo canonico.
    ///
    /// Restituisce `None` per 13 e 14 (`curve`/`surface`, astratti, mai sul
    /// filo) e per ogni codice sconosciuto: il rifiuto spetta al chiamante.
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
    /// Stato di dichiarazione dei tipi geometrici di una colonna (chiave
    /// canonica `plenora.geometry.types_declaration`).
    ///
    /// `Mixed` significa tipi diversi **per dichiarazione** (per esempio una
    /// colonna `PostGIS` `geometry` senza vincolo): è informazione, non
    /// ignoranza. `Unresolved` significa byte non ispezionati e nessuna
    /// dichiarazione.
    ///
    /// Le conversioni `mixed` ↔ `unresolved` sono vietate: direbbero una cosa
    /// diversa da quella dichiarata. Un input senza le chiavi
    /// `types`/`types_declaration` non è `Unresolved` ma «proprietà non
    /// dichiarata» ([`GeometryColumnContract::undeclared_types`]).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    TypesDeclaration,
    errore UnknownTypesDeclaration = "types_declaration non riconosciuta",
    "ammesse" {
        /// Esattamente i tipi dell'elenco, non vuoto.
        Exact => "exact",
        /// Tipi diversi per dichiarazione; l'elenco, se c'è, li nomina.
        Mixed => "mixed",
        /// Nessuna dichiarazione e byte non ispezionati; nessun elenco.
        Unresolved => "unresolved",
    }
}

/// Errore di costruzione o parsing di [`GeometryTypesProperty`].
///
/// I messaggi descrivono la violazione senza riportare mai il contenuto
/// dell'elenco (regola «errori senza dati» di `plenora-core`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryTypesPropertyError {
    /// `exact` senza elenco o con elenco vuoto.
    ExactWithoutTypes,
    /// `unresolved` con elenco presente.
    UnresolvedWithTypes,
    /// Valore non canonico nell'elenco testuale (maiuscole, `snake_case`,
    /// spazi, token vuoti o nomi ignoti).
    UnknownTypeInList,
    /// Duplicato nell'elenco testuale: la forma canonica richiede valori
    /// unici.
    DuplicateTypeInList,
    /// Elenco testuale fuori dall'ordine canonico di [`GeometryType`].
    NonCanonicalOrder,
}

impl fmt::Display for GeometryTypesPropertyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ExactWithoutTypes => {
                "types_declaration `exact` richiede un elenco di tipi presente e non vuoto"
            }
            Self::UnresolvedWithTypes => {
                "types_declaration `unresolved` non ammette un elenco di tipi"
            }
            Self::UnknownTypeInList => {
                "elenco tipi con valore non canonico (ammessi i 16 tipi di `GeometryType`, minuscoli senza separatore, separati da `,` senza spazi)"
            }
            Self::DuplicateTypeInList => {
                "elenco tipi con duplicati: la forma canonica richiede valori unici"
            }
            Self::NonCanonicalOrder => {
                "elenco tipi fuori ordine canonico (l'ordine di `GeometryType`)"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GeometryTypesPropertyError {}

/// Coppia coerente (`types_declaration`, `types`) delle chiavi canoniche
/// `plenora.geometry.*` per una colonna geometrica.
///
/// Le coerenze sono imposte per costruzione (campi privati, unico ingresso
/// [`GeometryTypesProperty::new`]): `Exact` richiede un elenco non vuoto,
/// `Unresolved` lo vieta, `Mixed` lo ammette (vuoto conta come assente).
///
/// `new` normalizza l'elenco, cosi' una dichiarazione ha una sola
/// serializzazione; il parsing ([`GeometryTypesProperty::from_canonical_list`])
/// e' invece fail-closed su spazi, duplicati e ordine non canonico.
///
/// Niente serde derivato: nei metadati c'è la coppia di chiavi, e una
/// `Deserialize` derivata aggirerebbe il validatore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeometryTypesProperty {
    declaration: TypesDeclaration,
    types: Box<[GeometryType]>,
}

impl GeometryTypesProperty {
    /// Costruisce la proprietà validando le coerenze e normalizzando
    /// l'elenco (dedup + ordine canonico).
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
        // Normalizzazione: valori unici in ordine canonico, così una stessa
        // dichiarazione ha una sola serializzazione.
        types.sort_unstable();
        types.dedup();
        Self::check_coherence(declaration, types.len())?;
        Ok(Self {
            declaration,
            types: types.into_boxed_slice(),
        })
    }

    /// Parsing fail-closed dalla forma canonica dei metadati: valori unici
    /// in ordine canonico separati da `,` senza spazi. La stringa vuota
    /// modella l'elenco assente (chiave `types` non emessa).
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

    /// La dichiarazione (`types_declaration`).
    #[must_use]
    pub const fn declaration(&self) -> TypesDeclaration {
        self.declaration
    }

    /// L'elenco normalizzato dei tipi (unici, ordine canonico); vuoto quando
    /// la dichiarazione non porta elenco.
    #[must_use]
    pub fn types(&self) -> &[GeometryType] {
        &self.types
    }

    /// Serializzazione canonica della chiave `plenora.geometry.types`:
    /// valori unici in ordine canonico separati da `,` senza spazi. Stringa
    /// vuota quando l'elenco è assente.
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
    /// Ordine degli assi del CRS (chiave canonica
    /// `plenora.geometry.axis_order`). Forma testuale minuscola con `_`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    AxisOrder,
    errore UnknownAxisOrder = "ordine assi non riconosciuto",
    "ammessi" {
        /// Longitudine, latitudine.
        LonLat => "lon_lat",
        /// Latitudine, longitudine.
        LatLon => "lat_lon",
        /// Est, nord.
        EastingNorthing => "easting_northing",
        /// Nord, est.
        NorthingEasting => "northing_easting",
        /// Un altro ordine.
        Other => "other",
        /// Ordine non noto.
        Unknown => "unknown",
    }
}

enum_icd! {
    /// Stato di risoluzione del CRS (chiave canonica
    /// `plenora.geometry.crs_resolution`). Forma testuale minuscola con `_`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    CrsResolution,
    errore UnknownCrsResolution = "risoluzione CRS non riconosciuta",
    "ammesse" {
        /// CRS risolto.
        Resolved => "resolved",
        /// CRS dichiarato ma non risolto (o dichiarazioni in conflitto).
        DeclaredUnresolved => "declared_unresolved",
        /// Nessun CRS dichiarato.
        Missing => "missing",
    }
}

enum_icd! {
    /// Formato testuale della definizione CRS (chiave canonica
    /// `plenora.geometry.crs_definition_format`). Forma testuale minuscola.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    CrsDefinitionFormat,
    errore UnknownCrsDefinitionFormat = "formato definizione CRS non riconosciuto",
    "ammessi" {
        /// WKT1.
        Wkt => "wkt",
        /// WKT2.
        Wkt2 => "wkt2",
        /// PROJJSON.
        Projjson => "projjson",
    }
}

enum_icd! {
    /// Semantica spaziale della colonna (chiave canonica
    /// `plenora.geometry.spatial_semantics`). Forma testuale minuscola.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    SpatialSemantics,
    errore UnknownSpatialSemantics = "semantica spaziale non riconosciuta",
    "ammesse" {
        /// Geometria planare.
        Geometry => "geometry",
        /// Geografia sulla sfera o sull'ellissoide.
        Geography => "geography",
    }
}

enum_icd! {
    /// Precisione delle coordinate (chiave canonica
    /// `plenora.geometry.precision`). Forma testuale minuscola.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    GeometryPrecision,
    errore UnknownGeometryPrecision = "precisione geometria non riconosciuta",
    "ammesse" {
        /// `f64`.
        Float64 => "float64",
        /// `f32`.
        Float32 => "float32",
        /// Quella nativa della sorgente.
        Native => "native",
    }
}

/// CRS di una colonna geometrica nel contratto. Gli stati di risoluzione
/// non si collassano l'uno nell'altro, e un CRS non si inventa mai.
///
/// - [`ContractCrs::Resolved`]: definizione risolta dal risolutore alla
///   lettura del contratto (qui la tabella dei CRS integrati, o un
///   risolutore del chiamante);
/// - [`ContractCrs::ResolvedByDecision`]: risolta allo stesso modo, ma per
///   una decisione esplicita del piano su uno stato `DeclaredUnresolved`.
///   Conta per l'emissione, dove il CRS deciso sostituisce le dichiarazioni
///   in conflitto della sorgente; altrove è un CRS risolto a tutti gli
///   effetti. Nessun codice di questo repository la costruisce: le
///   decisioni CRS del piano (`crs_decisions`) erano del progetto
///   d'origine;
/// - [`ContractCrs::DeclaredUnresolved`]: il CRS c'è ma non si risolve
///   (dichiarato così, o dichiarazioni in conflitto). L'incoerenza si
///   propaga fino alla scrittura con le dichiarazioni originali: per questo
///   la variante porta `crs_id` e `definition` col suo formato. Lo `srid`
///   viaggia come lineage nei metadati. La lettura la costruisce solo con
///   almeno una rappresentazione dichiarata fra `crs_id`, `definition` e
///   `srid`: col solo `srid` i due campi sono assenti;
/// - [`ContractCrs::Missing`]: nessun CRS dichiarato. Si propaga negli
///   output (`plenora.geometry.crs_resolution = missing`) e ferma solo le
///   operazioni con un `CrsRequirement`, nell'analisi del contratto.
///
/// `DeclaredUnresolved` ferma le stesse op nello stesso punto e con la
/// stessa categoria (`Crs`) di `Missing`, ma con un messaggio distinto: la
/// colonna dichiara un'incoerenza, non un'assenza.
#[derive(Clone, Debug)]
pub enum ContractCrs {
    /// CRS risolto.
    Resolved(ResolvedCrs),
    /// Risolto per decisione esplicita del piano: stesso
    /// comportamento di [`ContractCrs::Resolved`] per i consumatori del
    /// CRS; l'emissione sostituisce le dichiarazioni della sorgente con il
    /// CRS deciso.
    ResolvedByDecision(ResolvedCrs),
    /// Incoerenza dichiarata non risolta: le rappresentazioni originali, per
    /// la ri-emissione fedele.
    DeclaredUnresolved {
        /// Identificatore di autorita' dichiarato (`plenora.geometry.crs_id`).
        crs_id: Option<String>,
        /// Definizione testuale dichiarata (`plenora.geometry.crs_definition`).
        definition: Option<String>,
        /// Formato della definizione (`plenora.geometry.crs_definition_format`),
        /// se dichiarato.
        definition_format: Option<CrsDefinitionFormat>,
    },
    /// Nessun CRS dichiarato.
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
    /// `plenora.geometry.crs_resolution`.
    #[must_use]
    pub const fn resolution(&self) -> CrsResolution {
        match self {
            Self::Resolved(_) | Self::ResolvedByDecision(_) => CrsResolution::Resolved,
            Self::DeclaredUnresolved { .. } => CrsResolution::DeclaredUnresolved,
            Self::Missing => CrsResolution::Missing,
        }
    }
}

/// Contratto di una colonna geometrica.
#[derive(Clone, Debug)]
pub struct GeometryColumnContract {
    /// Identità logica stabile nel grafo: le rinomine cambiano `name`,
    /// non `field_id`.
    pub field_id: FieldId,
    /// Nome visibile della colonna nello schema Arrow.
    pub name: String,
    /// Stato del CRS (solo `geo.reproject` modifica un CRS risolto; ogni
    /// altro passo lo preserva, compreso lo stato `Missing`).
    pub crs: ContractCrs,
    /// Dimensionalità delle geometrie.
    pub dimensions: GeometryDimensions,
    /// Framing binario delle celle, se dichiarato dai metadati: `None`
    /// quando la sorgente non dichiara un `encoding`, mai un default
    /// silenzioso. I framing fuori dall'enum chiuso non sono
    /// rappresentabili: la lettura del contratto li rifiuta con errore
    /// esplicito.
    pub encoding: Option<GeometryEncoding>,
    /// Se la colonna ammette null (uguale al campo dello schema).
    pub nullable: bool,
    /// Dichiarazione dei tipi geometrici della colonna (chiavi canoniche
    /// `plenora.geometry.types` + `plenora.geometry.types_declaration`).
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
    /// Non equivale a `Declared(TypesDeclaration::Unresolved)`.
    #[must_use]
    pub const fn undeclared_types() -> ContractProperty<GeometryTypesProperty> {
        ContractProperty::new(PropertyConfidence::Unknown, PropertyScope::Schema)
    }
}

/// Assegnatore di [`FieldId`] nel namespace del piano.
///
/// Dentro un piano l'allocatore è uno solo, condiviso dalla validazione del
/// runner e dalle analisi dei kernel: due allocatori non coordinati
/// darebbero lo stesso ID a colonne diverse.
///
/// La validazione rimappa le geometrie di input con
/// [`FieldAllocator::alloc`] (senza legare i nomi: gli input possono avere
/// colonne omonime); [`FieldAllocator::observe`] registra gli ID già
/// presenti negli input di un passo. Le colonne propagate tengono l'ID, le
/// derivate ([`FieldAllocator::derive`]) ne ricevono uno nuovo, le rinomine
/// lo spostano ([`FieldAllocator::rename`]); [`FieldAllocator::intern`]
/// rende stabile l'ID di una colonna per nome (chiavi `sorted_by`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldAllocator {
    /// Il prossimo ID libero, in `u64` perche' deve poter valere
    /// `u32::MAX + 1` (spazio esaurito) senza saturare: un cursore `u32`
    /// saturato a `u32::MAX` riassegnerebbe un ID gia' osservato.
    next: u64,
    by_name: HashMap<String, FieldId>,
}

impl FieldAllocator {
    /// Allocatore che parte da `next` (per esempio il primo ID libero dopo
    /// quelli degli input).
    #[must_use]
    pub fn new(next: u32) -> Self {
        Self {
            next: u64::from(next),
            by_name: HashMap::new(),
        }
    }

    /// Assegna un nuovo `FieldId` garantito fresco e avanza il cursore.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` quando lo spazio degli identificatori e'
    /// esaurito: un incremento saturante renderebbe lo stesso id due volte, e
    /// due colonne condividerebbero l'identità.
    pub fn alloc(&mut self) -> Result<FieldId> {
        // Il controllo e' sull'ID da consegnare, non sul successivo: anche
        // `u32::MAX` si assegna, e solo la richiesta dopo fallisce.
        let id = u32::try_from(self.next).map_err(|_| {
            PlenoraError::InvalidPlan(
                "spazio dei FieldId esaurito: nessun identificatore fresco disponibile".to_owned(),
            )
        })?;
        self.next = u64::from(id) + 1;
        Ok(FieldId(id))
    }

    /// Il prossimo ID che verrà assegnato (ispezione, non consuma).
    ///
    /// A spazio esaurito rende `u32::MAX`, che e' gia' assegnato: e'
    /// un'ispezione, e solo [`FieldAllocator::alloc`] decide (con errore).
    #[must_use]
    pub fn peek(&self) -> FieldId {
        FieldId(u32::try_from(self.next).unwrap_or(u32::MAX))
    }

    /// Registra un ID già assegnato (contratti di input) per evitare
    /// collisioni con i futuri [`FieldAllocator::alloc`].
    pub fn observe(&mut self, id: FieldId) {
        // Niente saturazione: osservato `u32::MAX`, il cursore passa oltre
        // e il prossimo `alloc` fallisce invece di riconsegnarlo.
        self.next = self.next.max(u64::from(id.0) + 1);
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

    /// Rinomina: il `FieldId` segue la colonna.
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(id) = self.by_name.remove(old) {
            self.by_name.insert(new.to_owned(), id);
        }
    }

    /// Colonna derivata: riceve un `FieldId` nuovo, sostituendo l'eventuale
    /// identità precedente associata al nome.
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

/// Provenienza di una proprietà del contratto.
///
/// Come precondizione semantica vale solo una proprietà `Proven`; le
/// `Estimated` guidano esclusivamente scelte prestazionali correggibili in
/// esecuzione. Una `Declared` può diventare `Proven` con una verifica sui
/// dati, una `Estimated` può essere aggiornata in esecuzione.
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

/// Ambito di validità di una proprietà.
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

/// Una proprietà tipizzata con provenienza e ambito.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractProperty<T> {
    /// Valore e livello di fiducia.
    pub confidence: PropertyConfidence<T>,
    /// Ambito in cui la proprietà vale.
    pub scope: PropertyScope,
}

impl<T> ContractProperty<T> {
    /// Proprietà con la fiducia e l'ambito dati.
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

/// Verso di un ordinamento.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortDirection {
    /// Dal minore al maggiore.
    Ascending,
    /// Dal maggiore al minore.
    Descending,
}

/// Dove stanno i null rispetto ai valori, nell'ordine dell'arco.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NullPlacement {
    /// Null prima di ogni valore.
    First,
    /// Null dopo ogni valore.
    Last,
}

/// Ordinamento di un arco: chiavi nell'ordine di precedenza, verso comune a
/// tutte le chiavi e posizione dei null.
///
/// Le chiavi senza verso non bastano: "ordinato su `x`" in discendente non è
/// "ordinato su `x`" in ascendente, e un consumatore che li confondesse
/// leggerebbe l'ordine al contrario senza errore. Un produttore che non sa
/// dichiarare verso e null non dichiara l'ordinamento.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortOrder {
    /// Chiavi, come `FieldId` nel namespace globale del grafo.
    pub keys: Vec<FieldId>,
    /// Verso, lo stesso per tutte le chiavi.
    pub direction: SortDirection,
    /// Posizione dei null su ciascuna chiave.
    pub nulls: NullPlacement,
}

/// Proprietà tipizzate del contratto: non un framework generico, le nuove
/// proprietà si aggiungono come campi tipizzati.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContractProperties {
    /// Ordinamento dichiarato/dimostrato: chiavi, verso e posizione dei null.
    pub sorted_by: Option<ContractProperty<SortOrder>>,
    /// Cardinalità nota o stimata della tabella: mai `Proven` in
    /// validazione, perché senza i dati non è dimostrabile.
    pub row_count: Option<ContractProperty<u64>>,
}

/// Contratto di una tabella: ingresso o uscita di un passo.
///
/// Ogni tabella è conforme al suo contratto, inferito senza i dati
/// dall'analisi dell'operazione che la produce; il runner verifica dopo
/// ogni passo che l'uscita del kernel abbia nomi, tipi e metadati del
/// contratto.
#[derive(Clone, Debug)]
pub struct DataContract {
    /// Schema Arrow, metadati compresi.
    pub schema: SchemaRef,
    /// Colonne geometriche: al massimo una ([`DataContract::validate`]).
    pub geometries: Vec<GeometryColumnContract>,
    /// La colonna geometrica attiva, se dichiarata.
    pub active_geometry: Option<FieldId>,
    /// Proprietà (ordinamento, cardinalità).
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
    /// - al massimo una colonna geometrica;
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
                "contratto con {} colonne geometriche: la v1 ne ammette al massimo una",
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
                    "colonna geometrica `{}`: tipo fisico {}, atteso Binary (framing WKB/EWKB)",
                    geometry.name,
                    crate::tipo_arrow::descrivi_tipo(field.data_type())
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

/// Chiave canonica dell'elenco dei tipi geometrici.
///
/// Vive qui, nel livello contratto, perche' e' proprio la chiave con cui il
/// contratto e i metadati Arrow devono concordare; `plenora-kernels-geo` la
/// riespone per il lato emissione, cosi' esiste una sola definizione.
pub const PLENORA_GEOMETRY_TYPES_KEY: &str = "plenora.geometry.types";

/// Chiave canonica dello stato di dichiarazione dei tipi.
pub const PLENORA_GEOMETRY_TYPES_DECLARATION_KEY: &str = "plenora.geometry.types_declaration";

/// Chiave canonica della dimensionalità.
pub const PLENORA_GEOMETRY_DIMENSIONS_KEY: &str = "plenora.geometry.dimensions";

/// Chiave canonica del framing binario delle celle.
pub const PLENORA_GEOMETRY_ENCODING_KEY: &str = "plenora.geometry.encoding";

/// Chiave canonica dello stato di risoluzione del CRS.
pub const PLENORA_GEOMETRY_CRS_RESOLUTION_KEY: &str = "plenora.geometry.crs_resolution";

/// Coerenza fra la proprieta' tipizzata `types` del contratto e i metadati
/// canonici della colonna.
///
/// 1. Ogni chiave canonica presente dev'essere leggibile, qualunque cosa
///    dichiari il contratto: «presente ma malformata» non e' «assente».
/// 2. Le due fonti si confrontano solo quando entrambe dichiarano qualcosa
///    («non dichiarato» è uno stato legittimo); se si
///    contraddicono, il contratto e' rifiutato.
fn validate_declared_types(
    geometry: &GeometryColumnContract,
    metadata: &crate::arrow::Metadata,
) -> Result<()> {
    // Ogni chiave canonica PRESENTE dev'essere sintatticamente valida, anche
    // quando il lato tipizzato del contratto tace. «Assente» e «presente ma
    // malformata» sono stati diversi: saltare il controllo quando il
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
            // Si nominano la chiave e la violazione, mai i valori («errori
            // senza dati»).
            return Err(PlenoraError::Schema(format!(
                "colonna geometrica `{}`: dimensions del contratto diversa dai metadati canonici",
                geometry.name
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
        // `None` nel contratto significa «non dichiarato», uno stato
        // legittimo: la DISCREPANZA si controlla solo quando entrambi
        // parlano, ma la validita' sintattica sopra vale comunque.
        if geometry.encoding.is_some_and(|encoding| encoding != parsed) {
            return Err(PlenoraError::Schema(format!(
                "colonna geometrica `{}`: encoding del contratto diverso dai metadati canonici",
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
    // lettura declassa un `resolved` con chiavi in conflitto; una decisione
    // di piano risolverebbe un `declared_unresolved`). La coerenza del CRS
    // la decidono lettura e risoluzione per precedenza.
    //
    // Tipi geometrici: il confronto scatta solo con entrambi i lati presenti,
    // perche' «non dichiarato» e' legittimo su ciascun lato; la
    // contraddizione e' sempre un errore.
    //
    // `unresolved` nei metadati non dichiara nulla: è la forma che
    // l'emissione scrive per una proprietà non dichiarata (il vocabolario
    // Arrow vuole la chiave sempre), e la lettura la riporta a «non
    // dichiarato». Un contratto che ne sa di più (tipi letti da `GeoParquet`,
    // dichiarati da un'operazione) non la contraddice: l'emissione la
    // sostituisce.
    let Some(declared) = geometry.types.value() else {
        return Ok(());
    };
    if parsed_declaration == TypesDeclaration::Unresolved {
        return Ok(());
    }
    if parsed_declaration != declared.declaration() {
        return Err(PlenoraError::Schema(format!(
            "colonna geometrica `{}`: types_declaration del contratto diversa dai metadati canonici",
            geometry.name
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

/// Statistica di runtime, dal planner del progetto d'origine.
///
/// Regola: un piano è valido anche con statistiche completamente assenti
/// (`Unknown` → scelta conservativa); le statistiche `Known`/`Estimated`
/// possono solo migliorare scelte fisiche correggibili; nessuna scelta
/// semantica può dipendere da una statistica. Nessun codice di questo
/// repository la usa.
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

/// Sequenza logica di un batch, dall'executor a stream del progetto
/// d'origine.
///
/// Le operazioni parallele ricompongono l'output secondo l'ordine logico
/// assegnato dal piano, mai secondo l'ordine temporale di completamento.
/// Nessun codice di questo repository la usa: il runner lavora su tabelle
/// intere, un batch per tabella.
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

    /// Regressione: l'ultimo ID rappresentabile si assegna, e osservarlo
    /// esaurisce lo spazio invece di farlo riconsegnare (il cursore `u32`
    /// saturava e `alloc` rendeva un ID gia' osservato).
    #[test]
    fn field_allocator_arriva_a_u32_max_e_non_riconsegna_l_osservato() {
        let mut allocator = FieldAllocator::new(u32::MAX);
        assert_eq!(allocator.alloc().expect("ultimo id"), FieldId(u32::MAX));
        assert!(allocator.alloc().is_err());

        let mut allocator = FieldAllocator::default();
        allocator.observe(FieldId(u32::MAX));
        assert!(allocator.alloc().is_err(), "u32::MAX e' gia' in uso");
    }

    #[test]
    fn field_allocator_intern_rename_and_derive_follow_column_identity() {
        let mut allocator = FieldAllocator::default();
        // Interning: stesso nome -> stesso id, nomi diversi -> id diversi.
        let id = allocator.intern("geom").expect("id");
        assert_eq!(allocator.intern("geom").expect("id"), id);
        assert_ne!(allocator.intern("other").expect("id"), id);

        // Rinomina: l'id segue la colonna.
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
        // Mai default silenziosi: neppure maiuscole o vuoto.
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
        // Unknown: nessuno stride garantito.
        assert_eq!(GeometryDimensions::Unknown.coordinate_stride(), None);
    }

    #[test]
    fn geometry_encoding_from_str_rejects_unrecognized_values() {
        // Enum chiuso: altri framing (GeoPackage, TWKB) sono rifiutati.
        for value in ["WKB", "gpkg", "twkb", "", "iso-wkb"] {
            assert_eq!(
                value.parse::<GeometryEncoding>(),
                Err(UnknownGeometryEncoding)
            );
        }
    }

    /// I sedici valori canonici (quindici tipi più `unknown`), in ordine
    /// canonico.
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
        // L'ordine di dichiarazione delle varianti E' l'ordine canonico:
        // `Ord` (deriva dall'ordine di dichiarazione) e la serializzazione
        // delle liste di tipi dipendono da questa invariante.
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
        // `ALL` segue l'ordine canonico, oracolo scritto a parte.
        assert_eq!(GeometryType::ALL.len(), CANONICAL_TYPES.len());
        for (&geometry_type, (expected, text)) in GeometryType::ALL.iter().zip(CANONICAL_TYPES) {
            assert_eq!(geometry_type, expected);
            assert_eq!(geometry_type.as_str(), text);
        }
    }

    #[test]
    fn geometry_type_from_str_rejects_non_canonical_forms() {
        // Fail-closed: maiuscole, snake_case, spazi, vuoto e nomi ignoti
        // sono rifiutati, mai normalizzati in silenzio.
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
        // None: il rifiuto esplicito spetta al chiamante.
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
        // Una stessa dichiarazione ha una sola serializzazione.
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

        // Le coerenze valgono anche per la forma testuale.
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
        // Un ingresso privo delle chiavi significa «proprieta'
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
