//! plenora-kernels-geo — kernel geografici su `geo::Geometry<f64>` e adapter
//! Arrow per il canone GeoArrow-WKB (architettura.md#geometrie).
//!
//! Contiene il validatore WKB strutturale, i kernel puri (`operations`, `analysis`, `topology`,
//! `predicates`, `construction`, `equality`, `extended`,
//! `extended_algorithms`, `advanced`, `spatial_join`, `extensions`,
//! `extensions2`, `extensions3`, `cluster`),
//! backend opzionali (`geos_backend`,
//! `proj_backend`) e l'adapter Arrow di rappresentazione (`arrow_adapter`).
//!
//! L'adapter è progettato per ammettere la cache di decode per segmento
//! (architettura.md: decode/encode geo minimizzato, WKB come confine)
//! senza modifiche ai contratti.
//!
//! - [`arrow_adapter`](crate::arrow_adapter) per la rappresentazione
//!   GeoArrow-WKB e [`analyze`] per l'inferenza a secco dei contratti
//!   (`analyze_contract` del catalogo).
//! - [`memory_estimate`](crate::memory_estimate) per la STIMA dichiarata
//!   della memoria nativa delle geometrie decodificate
//!   (architettura.md#memoria): mai un conteggio preciso.
//! - [`geometry_contract`](crate::geometry_contract) per il contratto sulle
//!   geometrie decodificate (architettura.md#geometrie): dimensione esatta del WKB ISO XY e
//!   validazione strutturale su `Geometry`.
//!
//! Errori: il sorgente usava `GeoEngineError`; qui le stesse condizioni sono
//! mappate su [`plenora_core::PlenoraError`] preservando i messaggi:
//! - `InvalidWkb` / `EmptyGeometry` / `WkbSerialization` / `NonFiniteCoordinate`
//!   / `InvalidWkbStructure` / `InvalidGeometry` → `PlenoraError::InvalidPlan`;
//! - `UnsupportedWkbDimension` → `PlenoraError::Unsupported`.
//!
//! Feature: `geos-backend`, `proj-backend`, `full-backends`.

pub mod advanced;
pub mod analysis;
pub mod analyze;
pub mod arrow_adapter;
pub mod cluster;
pub mod construction;
pub mod crs;
pub mod decoded_size;
pub mod equality;
pub mod extended;
pub mod extended_algorithms;
pub mod extensions;
pub mod extensions2;
pub mod extensions3;
pub mod geometry_contract;
#[cfg(feature = "geos-backend")]
pub mod geos_backend;
pub mod memory_estimate;
pub mod operations;
pub mod predicates;
#[cfg(feature = "proj-backend")]
pub mod proj_backend;
pub mod spatial_join;
pub mod topology;
pub mod wkb_decoder;

use geo::algorithm::validation::Validation;
use geo::{
    BoundingRect, Centroid, ConvexHull, Coord, CoordsIter, Geometry, LineString, MapCoords, Point,
};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::contract::{GeometryDimensions, GeometryEncoding};
use plenora_core::PlenoraError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Centroid,
    ConvexHull,
    Envelope,
}

impl Operation {
    pub const ALL: [Self; 3] = [Self::Centroid, Self::ConvexHull, Self::Envelope];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Centroid => "centroid",
            Self::ConvexHull => "convex_hull",
            Self::Envelope => "envelope",
        }
    }
}

/// Mappatura delle varianti `GeoEngineError` del sorgente su `PlenoraError`
/// (messaggi invariati).
fn empty_geometry(operation: &'static str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("geometria vuota non supportata da {operation}"))
}

fn wkb_serialization(error: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("serializzazione WKB fallita: {error}"))
}

fn unsupported_wkb_dimension() -> PlenoraError {
    PlenoraError::Unsupported(
        "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D".to_owned(),
    )
}

/// Errore dedicato di coerenza: il type code dichiara una
/// dimensionalita' diversa da quella attesa dal contratto. Mai un
/// passthrough silenzioso: la divergenza e' sempre un errore esplicito.
fn wkb_dimension_mismatch() -> PlenoraError {
    PlenoraError::InvalidPlan(
        "dimensionalita' WKB incoerente con la dimensionalita' attesa dal contratto".to_owned(),
    )
}

pub(crate) fn non_finite_coordinate() -> PlenoraError {
    PlenoraError::InvalidPlan("WKB contiene coordinate NaN o infinite".to_owned())
}

pub(crate) fn invalid_wkb_structure(reason: &'static str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("struttura WKB non valida: {reason}"))
}

pub(crate) const fn geometry_type_name(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
        Geometry::Point(_) => "Point",
        Geometry::Line(_) => "Line",
        Geometry::LineString(_) => "LineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
        Geometry::Rect(_) => "Rect",
        Geometry::Triangle(_) => "Triangle",
    }
}

fn invalid_geometry(error: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("geometria OGC non valida: {error}"))
}

pub const MAX_WKB_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_WKB_COMPONENTS: u64 = 100_000;
/// Profondita' massima di annidamento (multi-geometrie) di default.
pub const MAX_WKB_DEPTH: usize = 64;

/// Flag EWKB (estensione PostGIS): ordinata Z presente.
const EWKB_Z_FLAG: u32 = 0x8000_0000;
/// Flag EWKB (estensione PostGIS): ordinata M presente.
const EWKB_M_FLAG: u32 = 0x4000_0000;
/// Flag EWKB (estensione PostGIS): SRID presente. Mai ammesso: lo SRID non
/// e' preservabile dal validatore strutturale.
const EWKB_SRID_FLAG: u32 = 0x2000_0000;
/// Maschera del tipo base nella forma EWKB (16 bit bassi).
const EWKB_TYPE_MASK: u32 = 0x0000_FFFF;
/// Bit alti riservati nella forma EWKB: se attivi, il type code non e' ne'
/// ISO ne' EWKB valido e va rifiutato.
const EWKB_RESERVED_MASK: u32 = 0x1FFF_0000;

/// Decodifica una geometria dal canone GeoArrow-WKB.
///
/// Una sola passata (architettura.md#geometrie): il decoder validante
/// ([`wkb_decoder::decode_validated`]) esegue validazione strutturale e
/// costruzione nella stessa camminata sui byte; poi la validazione OGC
/// della geometria risultante.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il payload viola il contratto WKB (struttura
/// non valida, coordinate NaN o infinite, geometria OGC non valida) o se il
/// decode fallisce; `PlenoraError::Unsupported` se il payload porta
/// dimensioni Z/M o SRID non preservabili nel protocollo 2D.
pub fn geometry_from_wkb(payload: &[u8]) -> Result<Geometry<f64>, PlenoraError> {
    let geometry = wkb_decoder::decode_validated(payload)?;
    geometry
        .check_validation()
        .map_err(|error| invalid_geometry(error.to_string()))?;
    Ok(geometry)
}

pub(crate) struct WkbCursor<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> WkbCursor<'a> {
    const fn new(payload: &'a [u8]) -> Self {
        Self { payload, offset: 0 }
    }

    const fn remaining(&self) -> usize {
        self.payload.len().saturating_sub(self.offset)
    }

    fn read_u8(&mut self) -> Result<u8, PlenoraError> {
        let value = *self
            .payload
            .get(self.offset)
            .ok_or_else(|| invalid_wkb_structure("byte mancante"))?;
        self.offset += 1;
        Ok(value)
    }

    fn read_u32(&mut self, little_endian: bool) -> Result<u32, PlenoraError> {
        let bytes: [u8; 4] = *self
            .payload
            .get(self.offset..)
            .and_then(|tail| tail.first_chunk::<4>())
            .ok_or_else(|| invalid_wkb_structure("uint32 troncato"))?;
        self.offset += 4;
        Ok(if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn read_f64(&mut self, little_endian: bool) -> Result<f64, PlenoraError> {
        let bytes: [u8; 8] = *self
            .payload
            .get(self.offset..)
            .and_then(|tail| tail.first_chunk::<8>())
            .ok_or_else(|| invalid_wkb_structure("float64 troncato"))?;
        self.offset += 8;
        Ok(if little_endian {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }

    /// Salta `bytes` byte senza leggerli: usato per le ordinate extra (Z/M),
    /// che il validatore stride-aware non decodifica ne' reinterpreta.
    fn skip(&mut self, bytes: usize) -> Result<(), PlenoraError> {
        if self.remaining() < bytes {
            return Err(invalid_wkb_structure("coordinata troncata"));
        }
        self.offset += bytes;
        Ok(())
    }

    /// Legge una coordinata con lo `stride` dichiarato (byte per coordinata
    /// interleaved, vedi [`GeometryDimensions::coordinate_stride`]): X e Y
    /// sono decodificate e validate (NaN/inf vietati, architettura.md#determinismo), le ordinate
    /// extra (Z/M) sono saltate via stride e mai lette.
    fn read_coordinate(
        &mut self,
        little_endian: bool,
        stride: usize,
    ) -> Result<(f64, f64), PlenoraError> {
        let x = self.read_f64(little_endian)?;
        let y = self.read_f64(little_endian)?;
        if !x.is_finite() || !y.is_finite() {
            return Err(non_finite_coordinate());
        }
        self.skip(stride - 16)?;
        Ok((x, y))
    }
}

pub(crate) fn checked_count(
    value: u32,
    remaining: usize,
    minimum_item_bytes: usize,
) -> Result<usize, PlenoraError> {
    let count = value as usize;
    if count > remaining / minimum_item_bytes.max(1) {
        return Err(invalid_wkb_structure(
            "conteggio elementi oltre i byte disponibili",
        ));
    }
    Ok(count)
}

/// Interpreta il type code WKB e ne deriva tipo base e stride coordinata,
/// verificando la coerenza con la dimensionalita' attesa.
///
/// Forme ammesse:
/// - ISO: `tipo + 1000 * dimensione` con dimensione 0..=3 (XY, Z, M, ZM);
/// - EWKB: flag Z ([`EWKB_Z_FLAG`]) e/o M ([`EWKB_M_FLAG`]) e tipo base nei
///   16 bit bassi, senza altri bit alti attivi.
///
/// Il flag SRID EWKB e' sempre rifiutato, per qualunque dimensionalita'
/// attesa: lo SRID non e' preservabile. I codici dimensione ISO oltre 3
/// mantengono l'errore storico ([`unsupported_wkb_dimension`]), cosi' i
/// chiamanti non osservano un cambio di variante d'errore su input
/// malformati.
///
/// Comportamento dichiarato: un payload EWKB SENZA flag Z/M e
/// senza SRID ha type code byte-identici a WKB ISO — le due forme sono
/// indistinguibili sul filo e un input dichiarato `encoding: ewkb` puro-XY
/// passa i gate come `xy`. L'encoding dichiarato nei metadati non cambia la
/// validazione strutturale: il gate resta sui type code, non sulla chiave
/// `encoding` del metadato `geo`.
///
/// Coerenza con la dimensionalita' attesa:
/// - `Xy`: ogni marcatore dimensionale e' rifiutato con l'errore storico,
///   come fa il validatore a sola XY;
/// - `Unknown` (R3.4: byte preservati, dimensionalita' dal type code):
///   qualunque forma valida e' accettata e lo stride e' derivato dal type
///   code stesso, geometria per geometria;
/// - `Xyz`/`Xym`/`Xyzm`: la divergenza dal type code produce l'errore
///   dedicato [`wkb_dimension_mismatch`], mai un passthrough.
pub(crate) fn parse_wkb_type_code(
    raw_type: u32,
    expected: GeometryDimensions,
) -> Result<(u32, usize), PlenoraError> {
    if raw_type & EWKB_SRID_FLAG != 0 {
        return Err(unsupported_wkb_dimension());
    }
    let (geometry_type, actual) = if raw_type & (EWKB_Z_FLAG | EWKB_M_FLAG) != 0 {
        if raw_type & EWKB_RESERVED_MASK != 0 {
            return Err(invalid_wkb_structure("tipo geometria non supportato"));
        }
        let has_z = raw_type & EWKB_Z_FLAG != 0;
        let has_m = raw_type & EWKB_M_FLAG != 0;
        // Almeno uno dei due flag e' attivo (guardia sopra): XY non occorre.
        let dimensions = match (has_z, has_m) {
            (true, false) => GeometryDimensions::Xyz,
            (false, true) => GeometryDimensions::Xym,
            (true, true) => GeometryDimensions::Xyzm,
            (false, false) => GeometryDimensions::Xy,
        };
        (raw_type & EWKB_TYPE_MASK, dimensions)
    } else {
        let dimensions = match raw_type / 1000 {
            0 => GeometryDimensions::Xy,
            1 => GeometryDimensions::Xyz,
            2 => GeometryDimensions::Xym,
            3 => GeometryDimensions::Xyzm,
            _ => return Err(unsupported_wkb_dimension()),
        };
        (raw_type % 1000, dimensions)
    };
    let coherent = match expected {
        GeometryDimensions::Unknown => actual,
        GeometryDimensions::Xy if actual != GeometryDimensions::Xy => {
            return Err(unsupported_wkb_dimension());
        }
        expected if expected == actual => actual,
        _ => return Err(wkb_dimension_mismatch()),
    };
    // `coherent` non e' mai `Unknown` (o e' `actual`, o e' `expected`
    // uguale ad `actual`): lo stride garantito esiste sempre.
    let stride = coherent
        .coordinate_stride()
        .ok_or_else(|| invalid_wkb_structure("tipo geometria non supportato"))?;
    Ok((geometry_type, stride))
}

/// Wrapper a dimensionalita' attesa `Xy`: serie ISO 1000+ e flag EWKB
/// Z/M/SRID rifiutati, come il validatore storico. Usato dal percorso
/// pubblico storico; la
/// variante stride-aware e' [`validate_wkb_geometry_with_dimensions`].
fn validate_wkb_geometry(
    cursor: &mut WkbCursor<'_>,
    depth: usize,
    max_depth: usize,
    components: &mut u64,
) -> Result<u32, PlenoraError> {
    validate_wkb_geometry_with_dimensions(
        cursor,
        depth,
        max_depth,
        components,
        GeometryDimensions::Xy,
        EmbeddedSridPolicy::Reject,
    )
}

#[derive(Clone, Copy)]
enum EmbeddedSridPolicy {
    Reject,
    Match(Option<u32>),
}

fn type_code_without_embedded_srid(
    cursor: &mut WkbCursor<'_>,
    little_endian: bool,
    raw_type: u32,
    policy: EmbeddedSridPolicy,
) -> Result<u32, PlenoraError> {
    if raw_type & EWKB_SRID_FLAG == 0 {
        return Ok(raw_type);
    }
    let EmbeddedSridPolicy::Match(expected_srid) = policy else {
        return Err(unsupported_wkb_dimension());
    };
    let embedded_srid = cursor.read_u32(little_endian)?;
    let expected_srid = expected_srid.ok_or_else(|| {
        PlenoraError::Crs("SRID EWKB embedded senza autorita' CRS governata".to_owned())
    })?;
    if embedded_srid != expected_srid {
        return Err(PlenoraError::Crs(
            "SRID EWKB embedded incoerente con lo SRID dichiarato".to_owned(),
        ));
    }
    Ok(raw_type & !EWKB_SRID_FLAG)
}

/// Validatore strutturale stride-aware: annidamento, conteggi, bound
/// sui byte e finitezza di X/Y sono verificati con lo stride della
/// dimensionalita' attesa (o derivato dal type code, se `Unknown`).
///
/// Le ordinate extra (Z/M) sono saltate via stride: mai lette, mai
/// reinterpretate, mai validate. In particolare la finitezza di Z/M NON e'
/// controllata — scelta deliberata: i byte sono preservati senza elaborare
/// le ordinate extra, quindi un NaN in Z/M non e' un dato elaborato dal kernel
/// — e la chiusura degli anelli e' valutata sulle sole X/Y.
fn validate_wkb_geometry_with_dimensions(
    cursor: &mut WkbCursor<'_>,
    depth: usize,
    max_depth: usize,
    components: &mut u64,
    expected: GeometryDimensions,
    srid_policy: EmbeddedSridPolicy,
) -> Result<u32, PlenoraError> {
    if depth > max_depth {
        return Err(invalid_wkb_structure(
            "annidamento geometrie oltre il limite",
        ));
    }
    *components = components
        .checked_add(1)
        .ok_or_else(|| invalid_wkb_structure("conteggio componenti oltre il limite"))?;
    if *components > MAX_WKB_COMPONENTS {
        return Err(invalid_wkb_structure(
            "conteggio componenti oltre il limite",
        ));
    }
    let byte_order = cursor.read_u8()?;
    let little_endian = match byte_order {
        0 => false,
        1 => true,
        _ => return Err(invalid_wkb_structure("byte order non valido")),
    };
    let raw_type = cursor.read_u32(little_endian)?;
    let raw_type = type_code_without_embedded_srid(cursor, little_endian, raw_type, srid_policy)?;
    let (geometry_type, stride) = parse_wkb_type_code(raw_type, expected)?;
    match geometry_type {
        1 => {
            cursor.read_coordinate(little_endian, stride)?;
        }
        2 => {
            let count = cursor.read_u32(little_endian)?;
            let count = checked_count(count, cursor.remaining(), stride)?;
            if count == 1 {
                return Err(invalid_wkb_structure(
                    "LineString deve essere vuota o avere almeno due coordinate",
                ));
            }
            for _ in 0..count {
                cursor.read_coordinate(little_endian, stride)?;
            }
        }
        3 => {
            let rings = cursor.read_u32(little_endian)?;
            let rings = checked_count(rings, cursor.remaining(), 4)?;
            for _ in 0..rings {
                let count = cursor.read_u32(little_endian)?;
                let count = checked_count(count, cursor.remaining(), stride)?;
                if count < 4 {
                    return Err(invalid_wkb_structure(
                        "anello poligonale con meno di quattro coordinate",
                    ));
                }
                let first = cursor.read_coordinate(little_endian, stride)?;
                let mut last = first;
                for _ in 1..count {
                    last = cursor.read_coordinate(little_endian, stride)?;
                }
                if first != last {
                    return Err(invalid_wkb_structure("anello poligonale non chiuso"));
                }
            }
        }
        4..=7 => {
            let children = cursor.read_u32(little_endian)?;
            let children = checked_count(children, cursor.remaining(), 5)?;
            for _ in 0..children {
                let child_type = validate_wkb_geometry_with_dimensions(
                    cursor,
                    depth + 1,
                    max_depth,
                    components,
                    expected,
                    srid_policy,
                )?;
                let valid_child = match geometry_type {
                    4 => child_type == 1,
                    5 => child_type == 2,
                    6 => child_type == 3,
                    7 => true,
                    _ => {
                        return Err(invalid_wkb_structure("tipo geometria non supportato"));
                    }
                };
                if !valid_child {
                    return Err(invalid_wkb_structure(
                        "tipo figlio incompatibile con multi-geometria",
                    ));
                }
            }
        }
        _ => {
            return Err(invalid_wkb_structure("tipo geometria non supportato"));
        }
    }
    Ok(geometry_type)
}

/// Byte di un WKB scritto in esadecimale; `None` se la stringa non lo e'.
///
/// # Perche' sui byte e non su `&str`
///
/// Affettare la stringa per indici di byte (`&hex[index..index + 2]`) dopo
/// aver controllato che la LUNGHEZZA IN BYTE sia pari non basta: le due
/// cose non si implicano. `"a\u{e9}b"` e' lungo quattro
/// byte — pari — ma l'indice 2 cade in mezzo alla codifica UTF-8 di
/// `\u{e9}`, e affettare fuori da un confine di carattere e' un **panic**,
/// non un errore. L'input arriva dalla configurazione di un piano, quindi da
/// fuori.
///
/// Trovato dalla campagna fuzz notturna (`analyze_geo`, artefatto
/// `crash-fd1eba39798feba74d4fc8837358f35c82a4a34a`). Il lint anti-panic R6
/// non copre questa classe: non c'e' nessun `unwrap`/`expect`/`panic!` — il
/// panic e' dentro l'indicizzazione.
///
/// Un esadecimale valido e' per definizione ASCII, quindi ogni byte fuori da
/// quell'insieme e' gia' un input non valido, non un carattere da
/// interpretare: la decodifica lavora sui byte e non affetta mai la stringa.
///
/// Vive QUI, accanto al validatore che la accompagna sempre, e non in due
/// copie: due copie della stessa decodifica sono due copie dello stesso
/// difetto.
///
/// `None` per stringa vuota, di lunghezza dispari, o con un byte che non e'
/// una cifra esadecimale ASCII. Il chiamante lo traduce nel proprio errore.
#[must_use]
pub fn wkb_hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    const fn cifra(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let cifre = hex.as_bytes();
    if cifre.is_empty() || !cifre.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(cifre.len() / 2);
    // `as_chunks::<2>()` invece di `chunks_exact(2)`: il tipo diventa
    // `[u8; 2]`, quindi l'indicizzazione e' totale per costruzione invece che
    // per convenzione. Il resto (un'eventuale cifra spaiata) e' gia' escluso
    // dal controllo di parita' qui sopra.
    for coppia in cifre.as_chunks::<2>().0 {
        let (Some(alto), Some(basso)) = (cifra(coppia[0]), cifra(coppia[1])) else {
            return None;
        };
        bytes.push((alto << 4) | basso);
    }
    Some(bytes)
}

/// Valida un payload WKB contro il contratto strutturale.
///
/// Verifica limiti di byte, annidamento, conteggi e finitezza delle
/// coordinate, con dimensionalita' attesa `Xy` e profondita' massima
/// [`MAX_WKB_DEPTH`].
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la struttura WKB non e' valida (byte o
/// conteggi oltre i limiti, anelli non chiusi, coordinate NaN o infinite,
/// byte residui); `PlenoraError::Unsupported` se il payload porta
/// dimensioni Z/M o SRID non preservabili nel protocollo 2D.
pub fn validate_wkb_contract(payload: &[u8]) -> Result<(), PlenoraError> {
    validate_wkb_contract_with_depth(payload, MAX_WKB_DEPTH)
}

/// Variante con profondita' di annidamento configurabile.
///
/// Il limite arriva dai `Limits` effettivi del piano
/// (`max_geometry_depth`); il default di [`validate_wkb_contract`] resta
/// [`MAX_WKB_DEPTH`].
///
/// # Errors
///
/// Come [`validate_wkb_contract`]; in piu' `PlenoraError::InvalidPlan` se
/// l'annidamento delle geometrie supera `max_depth`.
pub fn validate_wkb_contract_with_depth(
    payload: &[u8],
    max_depth: usize,
) -> Result<(), PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    validate_wkb_geometry(&mut cursor, 0, max_depth, &mut components)?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(())
}

/// Variante stride-aware, con dimensionalita' attesa esplicita.
///
/// La validazione strutturale usa lo stride della dimensionalita'
/// dichiarata e rifiuta ogni type code incoerente con essa
/// ([`wkb_dimension_mismatch`]). Con [`GeometryDimensions::Unknown`] i byte
/// sono preservati e la dimensionalita' e' derivata dal type code di ogni
/// geometria (R3.4); il flag SRID EWKB resta rifiutato in ogni caso. Le
/// ordinate extra (Z/M) non sono lette: vedi
/// [`validate_wkb_geometry_with_dimensions`].
///
/// Nessun chiamante passa ancora la dimensionalita' del contratto di
/// colonna: usano tutti [`validate_wkb_contract`], cioe' `Xy`.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la struttura WKB non e' valida o se il type
/// code e' incoerente con la dimensionalita' attesa
/// ([`wkb_dimension_mismatch`]); `PlenoraError::Unsupported` se il payload
/// porta dimensioni Z/M non dichiarate o il flag SRID EWKB.
pub fn validate_wkb_contract_for_dimensions(
    payload: &[u8],
    dimensions: GeometryDimensions,
) -> Result<(), PlenoraError> {
    validate_wkb_contract_for_dimensions_with_depth(payload, dimensions, MAX_WKB_DEPTH)
}

/// Come [`validate_wkb_contract_for_dimensions`], con profondita' di
/// annidamento configurabile (come [`validate_wkb_contract_with_depth`]).
///
/// # Errors
///
/// Come [`validate_wkb_contract_for_dimensions`]; in piu'
/// `PlenoraError::InvalidPlan` se l'annidamento delle geometrie supera
/// `max_depth`.
pub fn validate_wkb_contract_for_dimensions_with_depth(
    payload: &[u8],
    dimensions: GeometryDimensions,
    max_depth: usize,
) -> Result<(), PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    validate_wkb_geometry_with_dimensions(
        &mut cursor,
        0,
        max_depth,
        &mut components,
        dimensions,
        EmbeddedSridPolicy::Reject,
    )?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(())
}

/// Valida WKB/EWKB al confine di trasporto senza riscrivere le celle.
///
/// A differenza dei decoder dei kernel geometrici, questo gate ammette il
/// flag SRID soltanto per encoding EWKB dichiarato e verifica ogni SRID
/// embedded contro l'autorita' governata. I decoder elaboranti continuano a rifiutare
/// lo SRID embedded finche' non possono preservarlo.
///
/// # Errors
///
/// `PlenoraError::Crs` se uno SRID embedded diverge da `expected_srid`; le
/// altre varianti coincidono con
/// [`validate_wkb_contract_for_dimensions_with_depth`].
pub fn validate_wkb_transport_for_dimensions_with_depth(
    payload: &[u8],
    dimensions: GeometryDimensions,
    encoding: GeometryEncoding,
    expected_srid: Option<u32>,
    max_depth: usize,
) -> Result<(), PlenoraError> {
    if payload.len() > MAX_WKB_BYTES {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    let mut cursor = WkbCursor::new(payload);
    let mut components = 0_u64;
    let srid_policy = match encoding {
        GeometryEncoding::Wkb => EmbeddedSridPolicy::Reject,
        GeometryEncoding::Ewkb => EmbeddedSridPolicy::Match(expected_srid),
    };
    validate_wkb_geometry_with_dimensions(
        &mut cursor,
        0,
        max_depth,
        &mut components,
        dimensions,
        srid_policy,
    )?;
    if cursor.remaining() != 0 {
        return Err(invalid_wkb_structure("byte residui dopo la geometria"));
    }
    Ok(())
}

// float_cmp: i confronti esatti min/max individuano gli envelope degeneri
// (larghezza o altezza nulle) per costruzione — min e max provengono dalle
// stesse coordinate, quindi l'uguaglianza esatta e' il criterio voluto e un
// margine epsilon cambierebbe la geometria prodotta (determinismo architettura.md#determinismo).
#[allow(clippy::float_cmp)]
fn envelope(geometry: &Geometry<f64>) -> Result<Geometry<f64>, PlenoraError> {
    let rect = geometry
        .bounding_rect()
        .ok_or_else(|| empty_geometry("envelope"))?;
    let min = rect.min();
    let max = rect.max();

    if min.x == max.x && min.y == max.y {
        return Ok(Geometry::Point(Point::new(min.x, min.y)));
    }
    if min.x == max.x || min.y == max.y {
        return Ok(Geometry::LineString(LineString::from(vec![min, max])));
    }
    Ok(Geometry::Polygon(rect.to_polygon()))
}

fn robust_convex_hull(geometry: &Geometry<f64>) -> Geometry<f64> {
    // `geo`'s orientation math can overflow for otherwise finite coordinates
    // near f64 limits. Uniform normalization preserves hull topology while
    // keeping every intermediate determinant in a safe numeric range.
    let scale = geometry.coords_iter().fold(0.0_f64, |maximum, coordinate| {
        maximum.max(coordinate.x.abs()).max(coordinate.y.abs())
    });
    if scale == 0.0 {
        return Geometry::Polygon(geometry.convex_hull());
    }
    let normalized = geometry.map_coords(|coordinate| Coord {
        x: coordinate.x / scale,
        y: coordinate.y / scale,
    });
    Geometry::Polygon(normalized.convex_hull().map_coords(|coordinate| Coord {
        x: coordinate.x * scale,
        y: coordinate.y * scale,
    }))
}

/// Applica l'operazione (`Operation::Centroid`, `ConvexHull`, `Envelope`)
/// a una geometria gia' decodificata, validandola in ingresso e in uscita.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la geometria in ingresso o quella prodotta
/// non supera la validazione OGC, o se l'operazione non e' definita su una
/// geometria vuota (es. centroide o envelope di una geometria vuota).
pub fn transform_geometry(
    operation: Operation,
    geometry: &Geometry<f64>,
) -> Result<Geometry<f64>, PlenoraError> {
    geometry
        .check_validation()
        .map_err(|error| invalid_geometry(error.to_string()))?;
    transform_geometry_validated(operation, geometry)
}

fn transform_geometry_validated(
    operation: Operation,
    geometry: &Geometry<f64>,
) -> Result<Geometry<f64>, PlenoraError> {
    let output = match operation {
        Operation::Centroid => geometry
            .centroid()
            .map(Geometry::Point)
            .ok_or_else(|| empty_geometry("centroid")),
        Operation::ConvexHull => Ok(robust_convex_hull(geometry)),
        Operation::Envelope => envelope(geometry),
    }?;
    output
        .check_validation()
        .map_err(|error| invalid_geometry(error.to_string()))?;
    Ok(output)
}

/// Applica l'operazione direttamente su un payload WKB.
///
/// Decode, trasforma ([`transform_geometry`]), ri-encode nel canone XY e
/// ri-validazione del risultato contro il contratto
/// ([`validate_wkb_contract`]).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per gli errori di decode, trasformazione,
/// serializzazione WKB o validazione del risultato;
/// `PlenoraError::Unsupported` se il payload in ingresso porta dimensioni
/// Z/M o SRID non preservabili nel protocollo 2D.
pub fn transform_wkb(operation: Operation, payload: &[u8]) -> Result<Vec<u8>, PlenoraError> {
    let geometry = geometry_from_wkb(payload)?;
    let transformed = transform_geometry_validated(operation, &geometry)?;
    let output = transformed
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| wkb_serialization(error.to_string()))?;
    validate_wkb_contract(&output)?;
    Ok(output)
}

/// Validazione OGC di una geometria decodificata (architettura.md#geometrie D12.4).
///
/// La STESSA chiamata e lo STESSO messaggio del check in coda a
/// [`geometry_from_wkb`]. Esposta per il runner fuso, che riproduce tra i
/// passi del gruppo il controllo che il percorso non fuso esegue al decode
/// del nodo successivo.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la geometria non supera la validazione OGC.
pub fn check_geometry_valid(geometry: &Geometry<f64>) -> Result<(), PlenoraError> {
    geometry
        .check_validation()
        .map_err(|error| invalid_geometry(error.to_string()))
}

/// Pipeline di [`transform_wkb`] su una geometria gia' decodificata
/// (architettura.md#geometrie D12.4, profilo A).
///
/// Per `centroid`/`convex_hull`/`envelope`: stesso ordine e stessi messaggi
/// — kernel con validazione OGC dell'output
/// ([`transform_geometry_validated`]), limite di 64 MiB sul WKB equivalente
/// ([`geometry_contract::wkb_size_xy`], esatto per costruzione), validazione
/// strutturale ([`geometry_contract::validate_geometry_structural`], parita'
/// con `validate_wkb_contract` dimostrata dai test di `geometry_contract`).
/// Manca solo la serializzazione, demandata al chiamante: `to_wkb` su `Vec`
/// non ha casi di fallimento raggiungibili da una geometria che ha superato
/// questi controlli.
///
/// # Errors
///
/// Come [`transform_wkb`] per la parte successiva al decode.
pub fn transform_geometry_canonical(
    operation: Operation,
    geometry: &Geometry<f64>,
) -> Result<Geometry<f64>, PlenoraError> {
    let output = transform_geometry_validated(operation, geometry)?;
    if geometry_contract::wkb_size_xy(&output) > MAX_WKB_BYTES as u64 {
        return Err(invalid_wkb_structure("WKB oltre il limite di 64 MiB"));
    }
    geometry_contract::validate_geometry_structural(&output, MAX_WKB_DEPTH, MAX_WKB_COMPONENTS)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon, Area};
    use proptest::prelude::*;

    fn round_trip(operation: Operation, geometry: &Geometry<f64>) -> Geometry<f64> {
        let payload = geometry
            .to_wkb(CoordDimensions::xy())
            .expect("encode fixture");
        geometry_from_wkb(&transform_wkb(operation, &payload).expect("transform"))
            .expect("decode result")
    }

    /// Equivalente dei match sulle varianti `GeoEngineError` del sorgente:
    /// la condizione e' identificata dal messaggio, preservato verbatim.
    fn is_contract_error(result: &Result<Geometry<f64>, PlenoraError>, message: &str) -> bool {
        matches!(result, Err(PlenoraError::InvalidPlan(reason)) if reason == message)
    }

    /// Il crash della campagna fuzz notturna (`analyze_geo`, artefatto
    /// `crash-fd1eba39798feba74d4fc8837358f35c82a4a34a`).
    ///
    /// Affettare la stringa per indici di byte controllando la sola
    /// PARITA' della lunghezza non basta: `"a\u{e9}b"` e'
    /// lungo quattro byte — pari — ma l'indice 2 cade in mezzo alla codifica
    /// UTF-8 di `\u{e9}`, e affettare fuori da un confine di carattere e' un
    /// panic. L'input viene dalla configurazione di un piano.
    #[test]
    fn il_wkb_esadecimale_non_va_mai_in_panic_su_input_ostile() {
        // Il caso del crash: lunghezza pari in byte, confine di carattere no.
        assert_eq!(wkb_hex_to_bytes("a\u{e9}b"), None, "il caso del crash");

        // La stessa forma con altri caratteri multi-byte, di lunghezza pari.
        for ostile in [
            "\u{e9}\u{e9}",       // 4 byte, nessun indice pari e' un confine oltre lo 0
            "0\u{e9}0",           // 4 byte, indice 2 dentro la codifica
            "\u{1F642}",          // 4 byte, un solo carattere
            "\u{1F642}\u{1F642}", // 8 byte
            "ab\u{e9}",           // 4 byte, il taglio cade dentro l'ultimo
        ] {
            assert_eq!(wkb_hex_to_bytes(ostile), None, "input ostile: {ostile:?}");
        }

        // Lunghezza dispari e cifre non esadecimali: errore, non panic.
        for invalido in ["", "0", "abc", "zz", "0g", "00 ", " 00", "-1", "0\u{0}"] {
            assert_eq!(wkb_hex_to_bytes(invalido), None, "invalido: {invalido:?}");
        }

        // Gli esadecimali VALIDI continuano a decodificare, maiuscoli inclusi.
        assert_eq!(wkb_hex_to_bytes("00"), Some(vec![0x00]));
        assert_eq!(wkb_hex_to_bytes("ff"), Some(vec![0xff]));
        assert_eq!(wkb_hex_to_bytes("FF"), Some(vec![0xff]));
        assert_eq!(wkb_hex_to_bytes("0aFf"), Some(vec![0x0a, 0xff]));
        assert_eq!(
            wkb_hex_to_bytes("0101000000000000000000f03f0000000000000040"),
            Some(vec![
                0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
            ]),
            "un POINT(1 2) reale attraversa la decodifica"
        );
    }

    #[test]
    fn geometry_type_name_covers_every_geo_variant() {
        let variants = [
            (Geometry::Point(Point::new(0.0, 0.0)), "Point"),
            (
                Geometry::Line(geo::Line::new((0.0, 0.0), (1.0, 1.0))),
                "Line",
            ),
            (
                Geometry::LineString(LineString::new(Vec::new())),
                "LineString",
            ),
            (
                Geometry::Polygon(geo::Polygon::new(LineString::new(Vec::new()), Vec::new())),
                "Polygon",
            ),
            (
                Geometry::MultiPoint(geo::MultiPoint::new(Vec::new())),
                "MultiPoint",
            ),
            (
                Geometry::MultiLineString(geo::MultiLineString::new(Vec::new())),
                "MultiLineString",
            ),
            (
                Geometry::MultiPolygon(geo::MultiPolygon::new(Vec::new())),
                "MultiPolygon",
            ),
            (
                Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into()),
                "GeometryCollection",
            ),
            (
                Geometry::Rect(geo::Rect::new((0.0, 0.0), (1.0, 1.0))),
                "Rect",
            ),
            (
                Geometry::Triangle(geo::Triangle::new(
                    geo::Coord { x: 0.0, y: 0.0 },
                    geo::Coord { x: 1.0, y: 0.0 },
                    geo::Coord { x: 0.0, y: 1.0 },
                )),
                "Triangle",
            ),
        ];

        for (geometry, expected) in variants {
            assert_eq!(geometry_type_name(&geometry), expected);
        }
    }

    #[test]
    fn centroid_transforms_polygon_to_expected_point() {
        let input = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0),
            (x: 4.0, y: 0.0),
            (x: 4.0, y: 2.0),
            (x: 0.0, y: 2.0),
            (x: 0.0, y: 0.0),
        ]);
        let result = round_trip(Operation::Centroid, &input);
        assert_eq!(result, Geometry::Point(Point::new(2.0, 1.0)));
    }

    #[test]
    fn envelope_preserves_degenerate_dimension() {
        let point = Geometry::Point(Point::new(2.0, 3.0));
        assert_eq!(round_trip(Operation::Envelope, &point), point);

        let line = Geometry::LineString(line_string![(x: 1.0, y: 4.0), (x: 5.0, y: 4.0)]);
        assert_eq!(round_trip(Operation::Envelope, &line), line);
    }

    #[test]
    fn convex_hull_contains_input_area() {
        let input = Geometry::LineString(line_string![
            (x: 0.0, y: 0.0),
            (x: 4.0, y: 0.0),
            (x: 2.0, y: 3.0),
            (x: 2.0, y: 1.0),
        ]);
        let result = round_trip(Operation::ConvexHull, &input);
        assert!(result.unsigned_area() > 0.0);
    }

    #[test]
    fn convex_hull_normalizes_extreme_finite_coordinates_without_panicking() {
        let input = Geometry::LineString(LineString::from(vec![
            (-3.477_300_121_932_381e-164, 2.781_342_323_781_663e-309),
            (1.344_974_619_049_452e-284, 6.354_280_840_450_530_5e-183),
            (2.639_614_224_254_873e-309, 3.236_069_361_538_085e-111),
            (-5.488_802_840_312_24e303, -6.971_241_357_778_827e182),
            (-5.486_124_068_793_689e303, 7.064_166_183_585_296e-304),
        ]));
        let hull = transform_geometry(Operation::ConvexHull, &input).unwrap();
        assert!(hull.check_validation().is_ok());
        assert!(hull
            .coords_iter()
            .all(|coordinate| coordinate.x.is_finite() && coordinate.y.is_finite()));
        let payload = input.to_wkb(CoordDimensions::xy()).unwrap();
        assert!(transform_wkb(Operation::ConvexHull, &payload).is_ok());
    }

    #[test]
    fn rejects_non_finite_and_dimensional_wkb() {
        let mut nan_point = vec![1_u8, 1, 0, 0, 0];
        nan_point.extend_from_slice(&f64::NAN.to_le_bytes());
        nan_point.extend_from_slice(&1.0_f64.to_le_bytes());
        assert!(is_contract_error(
            &geometry_from_wkb(&nan_point),
            "WKB contiene coordinate NaN o infinite"
        ));

        let mut z_point = vec![1_u8];
        z_point.extend_from_slice(&1001_u32.to_le_bytes());
        z_point.extend_from_slice(&1.0_f64.to_le_bytes());
        z_point.extend_from_slice(&2.0_f64.to_le_bytes());
        z_point.extend_from_slice(&3.0_f64.to_le_bytes());
        assert!(matches!(
            geometry_from_wkb(&z_point),
            Err(PlenoraError::Unsupported(message))
            if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
    }

    #[test]
    fn rejects_dimensional_wkb_hidden_in_collection() {
        let mut collection = vec![1_u8];
        collection.extend_from_slice(&7_u32.to_le_bytes());
        collection.extend_from_slice(&1_u32.to_le_bytes());
        collection.push(1_u8);
        collection.extend_from_slice(&1001_u32.to_le_bytes());
        collection.extend_from_slice(&1.0_f64.to_le_bytes());
        collection.extend_from_slice(&2.0_f64.to_le_bytes());
        collection.extend_from_slice(&3.0_f64.to_le_bytes());
        assert!(matches!(
            geometry_from_wkb(&collection),
            Err(PlenoraError::Unsupported(message))
            if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
    }

    #[test]
    fn rejects_unclosed_or_too_short_polygon_rings() {
        let mut polygon = vec![1_u8];
        polygon.extend_from_slice(&3_u32.to_le_bytes());
        polygon.extend_from_slice(&1_u32.to_le_bytes());
        polygon.extend_from_slice(&4_u32.to_le_bytes());
        for (x, y) in [(0.0_f64, 0.0_f64), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            polygon.extend_from_slice(&x.to_le_bytes());
            polygon.extend_from_slice(&y.to_le_bytes());
        }
        assert!(is_contract_error(
            &geometry_from_wkb(&polygon),
            "struttura WKB non valida: anello poligonale non chiuso"
        ));

        let mut short = vec![1_u8];
        short.extend_from_slice(&3_u32.to_le_bytes());
        short.extend_from_slice(&1_u32.to_le_bytes());
        short.extend_from_slice(&3_u32.to_le_bytes());
        for _ in 0..3 {
            short.extend_from_slice(&0.0_f64.to_le_bytes());
            short.extend_from_slice(&0.0_f64.to_le_bytes());
        }
        assert!(is_contract_error(
            &geometry_from_wkb(&short),
            "struttura WKB non valida: anello poligonale con meno di quattro coordinate"
        ));
    }

    #[test]
    fn wkb_validator_covers_endianness_truncation_counts_and_trailing_bytes() {
        let mut big_endian_point = vec![0_u8];
        big_endian_point.extend_from_slice(&1_u32.to_be_bytes());
        big_endian_point.extend_from_slice(&2.0_f64.to_be_bytes());
        big_endian_point.extend_from_slice(&3.0_f64.to_be_bytes());
        assert_eq!(
            geometry_from_wkb(&big_endian_point).unwrap(),
            Geometry::Point(Point::new(2.0, 3.0))
        );

        for malformed in [
            vec![],
            vec![2],
            vec![1, 1, 0],
            vec![1, 1, 0, 0, 0, 0],
            vec![1, 2, 0, 0, 0, 1, 0, 0, 0],
            vec![1, 2, 0, 0, 0, 2, 0, 0, 0],
        ] {
            assert!(geometry_from_wkb(&malformed).is_err(), "{malformed:?}");
        }

        let mut trailing = big_endian_point.clone();
        trailing.push(0xff);
        assert!(is_contract_error(
            &geometry_from_wkb(&trailing),
            "struttura WKB non valida: byte residui dopo la geometria"
        ));

        let mut unsupported = vec![1_u8];
        unsupported.extend_from_slice(&99_u32.to_le_bytes());
        assert!(is_contract_error(
            &geometry_from_wkb(&unsupported),
            "struttura WKB non valida: tipo geometria non supportato"
        ));
    }

    #[test]
    fn wkb_validator_accepts_valid_multi_types_and_rejects_wrong_children() {
        for geometry in [
            Geometry::MultiPoint(geo::MultiPoint::new(vec![Point::new(1.0, 2.0)])),
            Geometry::MultiLineString(geo::MultiLineString::new(vec![line_string![
                (x: 0.0, y: 0.0), (x: 1.0, y: 1.0)
            ]])),
            Geometry::MultiPolygon(geo::MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 0.0, y: 1.0), (x: 0.0, y: 0.0)
            ]])),
            Geometry::GeometryCollection(geo::GeometryCollection(vec![Geometry::Point(
                Point::new(1.0, 2.0),
            )])),
        ] {
            let payload = geometry.to_wkb(CoordDimensions::xy()).unwrap();
            assert_eq!(geometry_from_wkb(&payload).unwrap(), geometry);
        }

        for parent_type in [4_u32, 5, 6] {
            let mut payload = vec![1_u8];
            payload.extend_from_slice(&parent_type.to_le_bytes());
            payload.extend_from_slice(&1_u32.to_le_bytes());
            let wrong_child = if parent_type == 4 {
                Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)])
            } else {
                Geometry::Point(Point::new(0.0, 0.0))
            };
            payload.extend_from_slice(&wrong_child.to_wkb(CoordDimensions::xy()).unwrap());
            assert!(is_contract_error(
                &geometry_from_wkb(&payload),
                "struttura WKB non valida: tipo figlio incompatibile con multi-geometria"
            ));
        }
    }

    #[test]
    fn validate_wkb_contract_with_depth_enforces_the_configurable_limit() {
        // GC(GC(Point)): il punto e' a profondita' 2.
        let nested = Geometry::GeometryCollection(geo::GeometryCollection(vec![
            Geometry::GeometryCollection(geo::GeometryCollection(vec![Geometry::Point(
                Point::new(1.0, 2.0),
            )])),
        ]));
        let payload = nested.to_wkb(CoordDimensions::xy()).unwrap();
        assert!(validate_wkb_contract_with_depth(&payload, MAX_WKB_DEPTH).is_ok());
        assert!(validate_wkb_contract_with_depth(&payload, 2).is_ok());
        assert!(matches!(
            validate_wkb_contract_with_depth(&payload, 1),
            Err(PlenoraError::InvalidPlan(reason))
                if reason == "struttura WKB non valida: annidamento geometrie oltre il limite"
        ));
        // Il default resta 64 (comportamento invariato di validate_wkb_contract).
        assert!(validate_wkb_contract(&payload).is_ok());
    }

    // ---- Fixture e test stride-aware ----

    /// Header little-endian di una geometria con il type code dato.
    fn push_header(payload: &mut Vec<u8>, raw_type: u32) {
        payload.push(1_u8);
        payload.extend_from_slice(&raw_type.to_le_bytes());
    }

    /// Coordinata con X, Y e le ordinate extra (Z e/o M, nell'ordine del
    /// type code): lo stride e' 16 + 8 * `extra.len()`.
    fn push_coordinate(payload: &mut Vec<u8>, x: f64, y: f64, extra: &[f64]) {
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        for value in extra {
            payload.extend_from_slice(&value.to_le_bytes());
        }
    }

    #[test]
    fn dimensional_wkb_validates_with_matching_expected_dimensions() {
        // Punto ISO ed EWKB nelle tre dimensionalita' estese.
        for (iso_type, ewkb_type, expected, extra) in [
            (
                1001_u32,
                0x8000_0001,
                GeometryDimensions::Xyz,
                &[7.0_f64][..],
            ),
            (2001, 0x4000_0001, GeometryDimensions::Xym, &[8.0][..]),
            (3001, 0xC000_0001, GeometryDimensions::Xyzm, &[7.0, 8.0][..]),
        ] {
            for raw_type in [iso_type, ewkb_type] {
                let mut payload = Vec::new();
                push_header(&mut payload, raw_type);
                push_coordinate(&mut payload, 1.0, 2.0, extra);
                assert!(
                    validate_wkb_contract_for_dimensions(&payload, expected).is_ok(),
                    "type code {raw_type:#x} con {expected}"
                );
                // Unknown: byte preservati, dimensionalita' dal type code.
                assert!(validate_wkb_contract_for_dimensions(
                    &payload,
                    GeometryDimensions::Unknown
                )
                .is_ok());
            }
        }
        // Big-endian ZM: lo stride non dipende dall'endianness.
        let mut big_endian = vec![0_u8];
        big_endian.extend_from_slice(&3001_u32.to_be_bytes());
        for value in [1.0_f64, 2.0, 3.0, 4.0] {
            big_endian.extend_from_slice(&value.to_be_bytes());
        }
        assert!(
            validate_wkb_contract_for_dimensions(&big_endian, GeometryDimensions::Xyzm).is_ok()
        );
    }

    #[test]
    fn dimensional_linestring_polygon_and_nested_collection_validate_with_stride() {
        // LineString ZM con due coordinate.
        let mut line = Vec::new();
        push_header(&mut line, 3002);
        line.extend_from_slice(&2_u32.to_le_bytes());
        push_coordinate(&mut line, 0.0, 0.0, &[5.0, 9.0]);
        push_coordinate(&mut line, 1.0, 1.0, &[6.0, 10.0]);
        assert!(validate_wkb_contract_for_dimensions(&line, GeometryDimensions::Xyzm).is_ok());

        // Poligono Z con anello esterno e un buco.
        let mut polygon = Vec::new();
        push_header(&mut polygon, 1003);
        polygon.extend_from_slice(&2_u32.to_le_bytes());
        polygon.extend_from_slice(&4_u32.to_le_bytes());
        for (x, y) in [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 0.0)] {
            push_coordinate(&mut polygon, x, y, &[1.0]);
        }
        polygon.extend_from_slice(&4_u32.to_le_bytes());
        for (x, y) in [(1.0, 1.0), (2.0, 1.0), (1.0, 2.0), (1.0, 1.0)] {
            push_coordinate(&mut polygon, x, y, &[2.0]);
        }
        assert!(validate_wkb_contract_for_dimensions(&polygon, GeometryDimensions::Xyz).is_ok());

        // Collection annidata ZM: GC(MultiPoint ZM, Point ZM).
        let mut collection = Vec::new();
        push_header(&mut collection, 3007);
        collection.extend_from_slice(&2_u32.to_le_bytes());
        push_header(&mut collection, 3004);
        collection.extend_from_slice(&1_u32.to_le_bytes());
        push_header(&mut collection, 3001);
        push_coordinate(&mut collection, 3.0, 4.0, &[5.0, 6.0]);
        push_header(&mut collection, 3001);
        push_coordinate(&mut collection, 7.0, 8.0, &[9.0, 10.0]);
        assert!(
            validate_wkb_contract_for_dimensions(&collection, GeometryDimensions::Xyzm).is_ok()
        );
    }

    #[test]
    fn truncation_inside_extra_ordinates_is_rejected() {
        let mut point = Vec::new();
        push_header(&mut point, 3001);
        push_coordinate(&mut point, 1.0, 2.0, &[3.0, 4.0]);
        assert!(validate_wkb_contract_for_dimensions(&point, GeometryDimensions::Xyzm).is_ok());
        // Troncato a meta' dell'ordinata M (ultimi 4 byte).
        assert!(validate_wkb_contract_for_dimensions(
            &point[..point.len() - 4],
            GeometryDimensions::Xyzm
        )
        .is_err());
        // Troncato a meta' dell'ordinata Z.
        assert!(validate_wkb_contract_for_dimensions(
            &point[..point.len() - 12],
            GeometryDimensions::Xyzm
        )
        .is_err());
        // Troncato esattamente dopo Y: le ordinate extra mancano del tutto.
        assert!(validate_wkb_contract_for_dimensions(
            &point[..point.len() - 16],
            GeometryDimensions::Xyzm
        )
        .is_err());
    }

    #[test]
    fn type_code_incoherent_with_expected_dimensions_gets_dedicated_error() {
        // Dichiara Xy ma il type code e' Z (ISO): rifiuto storico del wrapper.
        let mut z_point = Vec::new();
        push_header(&mut z_point, 1001);
        push_coordinate(&mut z_point, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract(&z_point),
            Err(PlenoraError::Unsupported(message))
                if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
        ));
        // Dichiara Xyz/Xym/Xyzm ma il type code e' XY: errore dedicato.
        let mut xy_point = Vec::new();
        push_header(&mut xy_point, 1);
        push_coordinate(&mut xy_point, 1.0, 2.0, &[]);
        for expected in [
            GeometryDimensions::Xyz,
            GeometryDimensions::Xym,
            GeometryDimensions::Xyzm,
        ] {
            assert!(matches!(
                validate_wkb_contract_for_dimensions(&xy_point, expected),
                Err(PlenoraError::InvalidPlan(message))
                    if message == "dimensionalita' WKB incoerente con la dimensionalita' attesa dal contratto"
            ));
        }
        // Dichiara Xym ma il type code porta Z (ISO 1001): stesso errore.
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&z_point, GeometryDimensions::Xym),
            Err(PlenoraError::InvalidPlan(message))
                if message == "dimensionalita' WKB incoerente con la dimensionalita' attesa dal contratto"
        ));
    }

    #[test]
    fn point_count_is_checked_against_the_real_stride() {
        // LineString Z che dichiara 3 coordinate ma ne contiene 2 (48 byte):
        // con stride 16 basterebbero, con lo stride reale 24 no. Il
        // validatore non deve desincronizzarsi: rifiuta.
        let mut line = Vec::new();
        push_header(&mut line, 1002);
        line.extend_from_slice(&3_u32.to_le_bytes());
        push_coordinate(&mut line, 0.0, 0.0, &[1.0]);
        push_coordinate(&mut line, 1.0, 1.0, &[2.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&line, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: conteggio elementi oltre i byte disponibili"
        ));
        // Conteggio ostile con stride ZM: sempre rifiutato, mai desync.
        let mut hostile = Vec::new();
        push_header(&mut hostile, 3002);
        hostile.extend_from_slice(&u32::MAX.to_le_bytes());
        push_coordinate(&mut hostile, 0.0, 0.0, &[1.0, 2.0]);
        assert!(validate_wkb_contract_for_dimensions(&hostile, GeometryDimensions::Xyzm).is_err());
    }

    #[test]
    fn ewkb_srid_flag_is_rejected_for_every_expected_dimension() {
        // Punto EWKB con SRID (flag 0x2000_0000) + valore SRID + XY.
        let mut payload = Vec::new();
        push_header(&mut payload, 0x2000_0001);
        payload.extend_from_slice(&4326_u32.to_le_bytes());
        push_coordinate(&mut payload, 1.0, 2.0, &[]);
        for expected in [
            GeometryDimensions::Xy,
            GeometryDimensions::Xyz,
            GeometryDimensions::Xym,
            GeometryDimensions::Xyzm,
            GeometryDimensions::Unknown,
        ] {
            assert!(matches!(
                validate_wkb_contract_for_dimensions(&payload, expected),
                Err(PlenoraError::Unsupported(_))
            ));
        }
        // SRID combinato con Z: sempre rifiutato.
        let mut payload_z = Vec::new();
        push_header(&mut payload_z, 0xA000_0001);
        payload_z.extend_from_slice(&4326_u32.to_le_bytes());
        push_coordinate(&mut payload_z, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload_z, GeometryDimensions::Unknown),
            Err(PlenoraError::Unsupported(_))
        ));
    }

    #[test]
    fn transport_gate_matches_big_endian_srid_without_weakening_kernel_gate() {
        let mut payload = vec![0_u8];
        payload.extend_from_slice(&0x2000_0001_u32.to_be_bytes());
        payload.extend_from_slice(&4326_u32.to_be_bytes());
        payload.extend_from_slice(&1.0_f64.to_be_bytes());
        payload.extend_from_slice(&2.0_f64.to_be_bytes());

        assert!(validate_wkb_transport_for_dimensions_with_depth(
            &payload,
            GeometryDimensions::Xy,
            GeometryEncoding::Ewkb,
            Some(4326),
            MAX_WKB_DEPTH,
        )
        .is_ok());
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload,
                GeometryDimensions::Xy,
                GeometryEncoding::Wkb,
                Some(4326),
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::Unsupported(_))
        ));
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload,
                GeometryDimensions::Xy,
                GeometryEncoding::Ewkb,
                None,
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::Crs(_))
        ));
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload,
                GeometryDimensions::Xy,
                GeometryEncoding::Ewkb,
                Some(32632),
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::Crs(_))
        ));
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xy),
            Err(PlenoraError::Unsupported(_))
        ));
        assert!(matches!(
            validate_wkb_transport_for_dimensions_with_depth(
                &payload[..5],
                GeometryDimensions::Xy,
                GeometryEncoding::Ewkb,
                Some(4326),
                MAX_WKB_DEPTH,
            ),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }

    #[test]
    fn non_finite_xy_is_rejected_with_z_present_but_z_is_never_read() {
        // NaN in X con Z presente: rifiutato (architettura.md#determinismo).
        let mut payload = Vec::new();
        push_header(&mut payload, 1001);
        push_coordinate(&mut payload, f64::NAN, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "WKB contiene coordinate NaN o infinite"
        ));
        // Inf in Y con Z presente: rifiutato.
        let mut payload = Vec::new();
        push_header(&mut payload, 1001);
        push_coordinate(&mut payload, 1.0, f64::INFINITY, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "WKB contiene coordinate NaN o infinite"
        ));
        // NaN nell'ordinata Z: accettato, perche' Z non e' mai letta ne'
        // reinterpretata: i byte sono preservati senza elaborare le ordinate
        // extra; vedi il doc-comment di validate_wkb_geometry_with_dimensions).
        let mut payload = Vec::new();
        push_header(&mut payload, 1001);
        push_coordinate(&mut payload, 1.0, 2.0, &[f64::NAN]);
        assert!(validate_wkb_contract_for_dimensions(&payload, GeometryDimensions::Xyz).is_ok());
    }

    #[test]
    fn depth_limit_applies_to_dimensional_collections() {
        // GC(GC(Point ZM)): il punto e' a profondita' 2.
        let mut outer = Vec::new();
        push_header(&mut outer, 3007);
        outer.extend_from_slice(&1_u32.to_le_bytes());
        push_header(&mut outer, 3007);
        outer.extend_from_slice(&1_u32.to_le_bytes());
        push_header(&mut outer, 3001);
        push_coordinate(&mut outer, 1.0, 2.0, &[3.0, 4.0]);
        assert!(validate_wkb_contract_for_dimensions_with_depth(
            &outer,
            GeometryDimensions::Xyzm,
            2
        )
        .is_ok());
        assert!(matches!(
            validate_wkb_contract_for_dimensions_with_depth(&outer, GeometryDimensions::Xyzm, 1),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: annidamento geometrie oltre il limite"
        ));
        // Il default resta 64 anche per la variante stride-aware.
        assert!(validate_wkb_contract_for_dimensions(&outer, GeometryDimensions::Xyzm).is_ok());
    }

    #[test]
    fn unknown_dimensions_preserve_bytes_and_derive_stride_per_geometry() {
        // Unknown (R3.4): byte preservati, dimensionalita' dal type code di
        // ogni geometria; una collection puo' mescolare dimensionalita'.
        let mut collection = Vec::new();
        push_header(&mut collection, 7);
        collection.extend_from_slice(&3_u32.to_le_bytes());
        push_header(&mut collection, 1);
        push_coordinate(&mut collection, 1.0, 2.0, &[]);
        push_header(&mut collection, 1001);
        push_coordinate(&mut collection, 3.0, 4.0, &[5.0]);
        push_header(&mut collection, 0xC000_0001);
        push_coordinate(&mut collection, 6.0, 7.0, &[8.0, 9.0]);
        assert!(
            validate_wkb_contract_for_dimensions(&collection, GeometryDimensions::Unknown).is_ok()
        );
        // Unknown non accetta strutture sbagliate: troncamento rifiutato.
        assert!(validate_wkb_contract_for_dimensions(
            &collection[..collection.len() - 8],
            GeometryDimensions::Unknown
        )
        .is_err());
        // Type code con bit alti riservati (non ISO, non EWKB): rifiutato.
        let mut garbage = Vec::new();
        push_header(&mut garbage, 0x8001_0001);
        push_coordinate(&mut garbage, 1.0, 2.0, &[3.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&garbage, GeometryDimensions::Unknown),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: tipo geometria non supportato"
        ));
    }

    #[test]
    fn ring_closure_is_checked_on_xy_only() {
        // Anello chiuso in X/Y con Z diverse agli estremi: accettato, perche'
        // Z non e' letta: le ordinate extra non sono elaborate.
        let mut polygon = Vec::new();
        push_header(&mut polygon, 1003);
        polygon.extend_from_slice(&1_u32.to_le_bytes());
        polygon.extend_from_slice(&4_u32.to_le_bytes());
        push_coordinate(&mut polygon, 0.0, 0.0, &[1.0]);
        push_coordinate(&mut polygon, 4.0, 0.0, &[2.0]);
        push_coordinate(&mut polygon, 4.0, 4.0, &[3.0]);
        push_coordinate(&mut polygon, 0.0, 0.0, &[99.0]);
        assert!(validate_wkb_contract_for_dimensions(&polygon, GeometryDimensions::Xyz).is_ok());
        // Anello non chiuso in X/Y: rifiutato anche con Z coerenti.
        let mut open = Vec::new();
        push_header(&mut open, 1003);
        open.extend_from_slice(&1_u32.to_le_bytes());
        open.extend_from_slice(&4_u32.to_le_bytes());
        push_coordinate(&mut open, 0.0, 0.0, &[1.0]);
        push_coordinate(&mut open, 4.0, 0.0, &[1.0]);
        push_coordinate(&mut open, 4.0, 4.0, &[1.0]);
        push_coordinate(&mut open, 0.0, 4.0, &[1.0]);
        assert!(matches!(
            validate_wkb_contract_for_dimensions(&open, GeometryDimensions::Xyz),
            Err(PlenoraError::InvalidPlan(message))
                if message == "struttura WKB non valida: anello poligonale non chiuso"
        ));
    }

    #[test]
    fn xy_wrapper_still_rejects_ewkb_flags_like_before() {
        // Il wrapper a sola XY rifiuta i flag EWKB con l'errore storico.
        for raw_type in [0x8000_0001_u32, 0x4000_0001, 0xC000_0001, 0x2000_0001] {
            let mut payload = Vec::new();
            push_header(&mut payload, raw_type);
            payload.extend_from_slice(&0_u32.to_le_bytes()); // eventuale SRID
            push_coordinate(&mut payload, 1.0, 2.0, &[3.0, 4.0]);
            assert!(matches!(
                validate_wkb_contract(&payload),
                Err(PlenoraError::Unsupported(message))
                    if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
            ));
        }
    }

    proptest! {
        #[test]
        fn arbitrary_wkb_bytes_never_panic_and_successes_remain_roundtrippable(
            payload in proptest::collection::vec(any::<u8>(), 0..4096)
        ) {
            if let Ok(geometry) = geometry_from_wkb(&payload) {
                prop_assert!(geometry.check_validation().is_ok());
                for operation in Operation::ALL {
                    if let Ok(output) = transform_wkb(operation, &payload) {
                        prop_assert!(geometry_from_wkb(&output).is_ok());
                    }
                }
            }
        }

        #[test]
        fn single_byte_mutations_of_valid_wkb_never_escape_the_contract(
            index in any::<usize>(), replacement in any::<u8>()
        ) {
            let mut payload = Geometry::Polygon(polygon![
                (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
                (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
                (x: 0.0, y: 0.0),
            ]).to_wkb(CoordDimensions::xy()).unwrap();
            let position = index % payload.len();
            payload[position] = replacement;
            if let Ok(geometry) = geometry_from_wkb(&payload) {
                prop_assert!(geometry.check_validation().is_ok());
                let encoded = geometry.to_wkb(CoordDimensions::xy()).unwrap();
                prop_assert!(validate_wkb_contract(&encoded).is_ok());
            }
        }
    }
}
