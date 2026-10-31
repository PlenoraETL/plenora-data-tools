//! Verifica dei valori Arrow Geometry Semantics prima dei kernel.
//!
//! Adattato dal lettore Rust di plenora-database-tools (`geometry_value.rs`):
//! revisione 5e32e8851bbfc3ceaa518d3b9f896c722cb49ce3 (stesso progetto Plenora).
//! Buona forma, SRID, tipi e dimensioni, nell'ordine GEO-011/REJ-002.
//! Nessun dato del valore entra nei messaggi.

use plenora_core::arrow::array::{Array, BinaryArray, RecordBatch};
use plenora_core::contract::{arrow_metadata::read_geometry_contract_keys, GeometryEncoding};
use plenora_core::{ErrorCategory, ErrorPhase, PlenoraError, Result};

const FLAG_Z: u32 = 0x8000_0000;
const FLAG_M: u32 = 0x4000_0000;
const FLAG_SRID: u32 = 0x2000_0000;
/// GEO-011: al piu 32 livelli di annidamento sotto la radice.
const MAX_NESTING: u64 = 32;

/// Ciò che il campo dichiara dei propri valori.
#[derive(Debug, Clone, Copy)]
struct ValueContract<'a> {
    /// `Some(true)` per `ewkb`, `Some(false)` per `wkb`; `None` su un ingresso
    /// legacy senza `plenora.geometry.encoding`, dove lo SRID del valore si
    /// confronta come sotto `ewkb`.
    pub ewkb: Option<bool>,
    pub srid: Option<u32>,
    pub types: Option<&'a str>,
    pub dimensions: Option<&'a str>,
}

/// Verifica un valore e ne restituisce l'ispezione.
///
/// `phase` e la fase in cui il chiamante incontra il valore (REJ-003): `Read`
/// per chi lo legge dal proprio ingresso, `Write` per chi lo scrive.
///
/// # Errors
///
/// `DataMapping` o `Crs` secondo l'ordine del modulo; `ResourceLimit` oltre
/// `max_components` o `max_depth`. Nessun messaggio riporta byte del valore.
fn verify_value(
    contract: ValueContract<'_>,
    bytes: &[u8],
    max_components: u64,
    max_depth: u64,
    phase: ErrorPhase,
) -> Result<Header> {
    let inspection = decode(bytes, max_components, max_depth, phase)?;
    if let Some(srid) = inspection.srid {
        if contract.ewkb == Some(false) {
            return Err(error(
                ErrorCategory::DataMapping,
                phase,
                "SRID incorporato in un valore dichiarato wkb",
            ));
        }
        // Un ingresso legacy senza encoding ne SRID dichiarati non ha un
        // campo con cui confrontare: resta com'era, lo SRID del valore.
        let legacy_without_srid = contract.ewkb.is_none() && contract.srid.is_none();
        if !legacy_without_srid && contract.srid != Some(srid) {
            return Err(error(
                ErrorCategory::Crs,
                phase,
                "SRID EWKB diverso da quello del campo, o su un campo che non ne dichiara",
            ));
        }
    }
    let type_name = TYPE_NAMES
        .iter()
        .find(|(code, _)| *code == inspection.base_type)
        .map(|(_, name)| *name)
        .ok_or_else(|| error(ErrorCategory::DataMapping, phase, "tipo geometrico ignoto"))?;
    if let Some(types) = contract.types {
        let admitted = types.split(',').any(|item| {
            item.eq_ignore_ascii_case("unknown") || item.eq_ignore_ascii_case(type_name)
        });
        if !admitted {
            return Err(error(
                ErrorCategory::DataMapping,
                phase,
                "tipo del valore fuori dalla lista dichiarata",
            ));
        }
    }
    if contract
        .dimensions
        .is_some_and(|declared| declared != "unknown" && declared != inspection.dimensions_label())
    {
        return Err(error(
            ErrorCategory::DataMapping,
            phase,
            "dimensioni del valore diverse da quelle dichiarate",
        ));
    }
    Ok(inspection)
}

const TYPE_NAMES: [(u32, &str); 15] = [
    (1, "point"),
    (2, "linestring"),
    (3, "polygon"),
    (4, "multipoint"),
    (5, "multilinestring"),
    (6, "multipolygon"),
    (7, "geometrycollection"),
    (8, "circularstring"),
    (9, "compoundcurve"),
    (10, "curvepolygon"),
    (11, "multicurve"),
    (12, "multisurface"),
    (15, "polyhedralsurface"),
    (16, "tin"),
    (17, "triangle"),
];

/// I membri che un contenitore ammette; `None` ammette ogni geometria.
const fn admitted_members(container: u32) -> Option<&'static [u32]> {
    match container {
        4 => Some(&[1]),
        5 => Some(&[2]),
        6 | 15 => Some(&[3]),
        9 => Some(&[2, 8]),
        10 | 11 => Some(&[2, 8, 9]),
        12 => Some(&[3, 10]),
        16 => Some(&[17]),
        _ => None,
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    components: u64,
    max_components: u64,
    max_depth: u64,
    observed_depth: u64,
    has_any_z: bool,
    has_any_m: bool,
    phase: ErrorPhase,
}

struct Header {
    base_type: u32,
    little_endian: bool,
    has_z: bool,
    has_m: bool,
    srid: Option<u32>,
}

impl Reader<'_> {
    fn malformed(&self, message: &'static str) -> PlenoraError {
        error(ErrorCategory::DataMapping, self.phase, message)
    }

    fn take(&mut self, amount: usize) -> Result<&[u8]> {
        let end = self
            .offset
            .checked_add(amount)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| self.malformed("valore WKB troncato"))?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    fn u32(&mut self, little_endian: bool) -> Result<u32> {
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| self.malformed("valore WKB troncato"))?;
        Ok(if little_endian {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    }

    fn add_components(&mut self, amount: u64) -> Result<()> {
        self.components = self.components.checked_add(amount).ok_or_else(|| {
            PlenoraError::ResourceLimit("overflow nel conteggio componenti WKB".to_owned())
        })?;
        if self.components > self.max_components {
            return Err(PlenoraError::ResourceLimit(
                "geometria WKB oltre il limite di componenti".to_owned(),
            ));
        }
        Ok(())
    }

    fn header(&mut self, outer: bool) -> Result<Header> {
        let little_endian = match self.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err(self.malformed("byte order WKB diverso da 0 e 1")),
        };
        let code = self.u32(little_endian)?;
        let mut has_z = code & FLAG_Z != 0;
        let mut has_m = code & FLAG_M != 0;
        let has_srid = code & FLAG_SRID != 0;
        let base = code & 0x1fff_ffff;
        let (thousands, kind) = (base / 1000, base % 1000);
        if thousands > 3 {
            return Err(self.malformed("codice di tipo WKB ignoto"));
        }
        if thousands > 0 && (has_z || has_m || has_srid) {
            return Err(self.malformed("codice di tipo ISO con flag della forma estesa"));
        }
        if thousands > 0 {
            has_z = thousands == 1 || thousands == 3;
            has_m = thousands >= 2;
        }
        if !TYPE_NAMES.iter().any(|(candidate, _)| *candidate == kind) {
            return Err(self.malformed("codice di tipo WKB ignoto"));
        }
        let srid = if has_srid {
            if !outer {
                return Err(self.malformed("SRID su un membro di una geometria"));
            }
            Some(self.u32(little_endian)?)
        } else {
            None
        };
        Ok(Header {
            base_type: kind,
            little_endian,
            has_z,
            has_m,
            srid,
        })
    }

    /// Prima i byte, poi il budget: un conteggio che i byte non contengono e
    /// un valore troncato (`data_mapping`), non un limite di risorse.
    fn points(&mut self, count: u32, width: usize) -> Result<&[u8]> {
        let bytes = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(width))
            .filter(|bytes| *bytes <= self.remaining())
            .ok_or_else(|| self.malformed("valore WKB troncato"))?;
        self.add_components(u64::from(count))?;
        self.take(bytes)
    }

    fn ring(&mut self, little_endian: bool, width: usize) -> Result<()> {
        let count = self.u32(little_endian)?;
        let points = self.points(count, width)?;
        if count < 4 || points[..width] != points[points.len() - width..] {
            return Err(self.malformed("anello non chiuso o di meno di quattro punti"));
        }
        Ok(())
    }

    /// Una geometria e i suoi membri. La ricorsione e limitata a
    /// [`MAX_NESTING`] livelli, quindi lo stack resta costante.
    fn geometry(&mut self, depth: u64, outer: bool) -> Result<Header> {
        if depth > MAX_NESTING {
            return Err(self.malformed("geometria WKB oltre 32 livelli di annidamento"));
        }
        if depth >= self.max_depth {
            return Err(PlenoraError::ResourceLimit(
                "geometria WKB oltre il limite di profondita".to_owned(),
            ));
        }
        self.observed_depth = self.observed_depth.max(depth + 1);
        self.add_components(1)?;
        let header = self.header(outer)?;
        self.has_any_z |= header.has_z;
        self.has_any_m |= header.has_m;
        let width = 8 * (2 + usize::from(header.has_z) + usize::from(header.has_m));
        let little_endian = header.little_endian;
        match header.base_type {
            1 => {
                self.points(1, width)?;
            }
            2 => {
                let count = self.u32(little_endian)?;
                self.points(count, width)?;
                if count == 1 {
                    return Err(self.malformed("linestring di un solo punto"));
                }
            }
            8 => {
                let count = self.u32(little_endian)?;
                self.points(count, width)?;
                if count != 0 && (count < 3 || count % 2 == 0) {
                    return Err(self
                        .malformed("circular string senza un numero dispari di almeno tre punti"));
                }
            }
            3 | 17 => {
                let rings = self.u32(little_endian)?;
                if usize::try_from(rings)
                    .ok()
                    .and_then(|rings| rings.checked_mul(4))
                    .is_none_or(|bytes| bytes > self.remaining())
                {
                    return Err(self.malformed("valore WKB troncato"));
                }
                if header.base_type == 17 && rings > 1 {
                    return Err(self.malformed("triangolo con piu di un anello"));
                }
                self.add_components(u64::from(rings))?;
                for _ in 0..rings {
                    self.ring(little_endian, width)?;
                }
            }
            container => {
                let members = self.u32(little_endian)?;
                // Ogni membro ha almeno un'intestazione: il ciclo e limitato
                // dai byte, non dal conteggio dichiarato.
                if usize::try_from(members)
                    .ok()
                    .and_then(|members| members.checked_mul(5))
                    .is_none_or(|bytes| bytes > self.remaining())
                {
                    return Err(self.malformed("valore WKB troncato"));
                }
                let admitted = admitted_members(container);
                for _ in 0..members {
                    let member = self.geometry(depth + 1, false)?;
                    if admitted.is_some_and(|types| !types.contains(&member.base_type)) {
                        return Err(
                            self.malformed("membro di un tipo che il contenitore non ammette")
                        );
                    }
                    if (member.has_z, member.has_m) != (header.has_z, header.has_m) {
                        return Err(self.malformed("membro con dimensioni diverse dal contenitore"));
                    }
                }
            }
        }
        Ok(header)
    }
}

fn decode(bytes: &[u8], max_components: u64, max_depth: u64, phase: ErrorPhase) -> Result<Header> {
    let mut reader = Reader {
        bytes,
        offset: 0,
        components: 0,
        max_components,
        max_depth,
        observed_depth: 0,
        has_any_z: false,
        has_any_m: false,
        phase,
    };
    let root = reader.geometry(0, true)?;
    if reader.remaining() != 0 {
        return Err(reader.malformed("byte dopo la geometria WKB"));
    }
    Ok(root)
}

fn error(category: ErrorCategory, phase: ErrorPhase, message: &'static str) -> PlenoraError {
    match category {
        ErrorCategory::Crs => PlenoraError::Crs(message.to_owned()),
        _ => PlenoraError::DataMapping(message.to_owned()),
    }
    .with_phase(phase)
}

impl Header {
    const fn dimensions_label(&self) -> &'static str {
        match (self.has_z, self.has_m) {
            (false, false) => "xy",
            (true, false) => "xyz",
            (false, true) => "xym",
            (true, true) => "xyzm",
        }
    }
}

/// Verifica i valori geometrici prima del kernel, in ordine di riga e campo.
/// Il passaggio tabellare non decodifica i byte e non chiama questa funzione.
///
/// # Errors
/// `DataMapping` per byte, tipi o dimensioni; `Crs` per SRID incoerenti.
pub fn verifica_tabella(tabella: &RecordBatch) -> Result<()> {
    let schema = tabella.schema();
    let mut colonne = Vec::new();
    for (i, field) in schema.fields().iter().enumerate() {
        if field
            .metadata()
            .get("ARROW:extension:name")
            .is_none_or(|v| v != "geoarrow.wkb")
        {
            continue;
        }
        let keys = read_geometry_contract_keys(field)?;
        let cells = tabella
            .column(i)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .ok_or_else(|| PlenoraError::Schema("storage geometrico non Binary".to_owned()))?;
        let contract = ValueContract {
            ewkb: keys.encoding.map(|e| e == GeometryEncoding::Ewkb),
            srid: keys.srid.map(|v| u32::from_ne_bytes(v.to_ne_bytes())),
            types: field
                .metadata()
                .get("plenora.geometry.types")
                .map(String::as_str),
            dimensions: field
                .metadata()
                .get("plenora.geometry.dimensions")
                .map(String::as_str),
        };
        colonne.push((cells, contract));
    }
    for row in 0..tabella.num_rows() {
        for (cells, contract) in &colonne {
            if !cells.is_null(row) {
                verify_value(*contract, cells.value(row), u64::MAX, 33, ErrorPhase::Read)?;
            }
        }
    }
    Ok(())
}
