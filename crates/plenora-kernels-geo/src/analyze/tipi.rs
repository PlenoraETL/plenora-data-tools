//! Tipi geometrici dell'output delle operazioni che spezzano, raccolgono o
//! ricostruiscono le geometrie senza essere trasformazioni 1:1
//! (piano-v5.md#contratti-di-input, decisione 8).
//!
//! Queste operazioni conservavano la dichiarazione `types` dell'ingresso,
//! che per loro non vale: `explode` di `[MultiPolygon]` produce `Polygon`,
//! `dissolve` produce sempre `MultiPolygon`, `polygonize` di linee produce
//! poligoni e residui lineari. Il contratto pubblicava una dichiarazione
//! falsa senza errore. Qui ogni operazione dichiara i tipi che il suo kernel
//! puo' produrre, verificati contro i kernel dall'oracolo
//! `analyze::tests::kernel_crosscheck`:
//!
//! - **insieme fisso**: `polygonize` (`LineString`, `Polygon`), `dissolve`
//!   (`MultiPolygon`), `overlay` (`Polygon`, `MultiPolygon`), `delaunay`
//!   (`Polygon`), `line_merge` (`LineString`);
//! - **per tipo d'ingresso**: `explode`, `subdivide`, `split` e `collect`
//!   mappano ogni tipo dichiarato nei tipi che il kernel ne ricava. Con una
//!   dichiarazione `exact` l'uscita e' `exact`; `mixed` con elenco resta
//!   `mixed`; `unresolved` e l'assenza di dichiarazione restano tali (per
//!   `split`, che produce solo `LineString` e `Polygon` qualunque sia
//!   l'ingresso, l'insieme fisso);
//! - **nessuna dichiarazione**: `line_builder` e `polygon_builder` non hanno
//!   un kernel in questo workspace che la verifichi; la dichiarazione
//!   ereditata si toglie, invece di tenerne una falsa.

use plenora_core::contract::{
    ContractProperty, GeometryColumnContract, GeometryType, GeometryTypesProperty,
    PropertyConfidence, PropertyScope, TypesDeclaration,
};
use plenora_core::{PlenoraError, Result};

/// I sette tipi che i kernel XY decodificano.
const TUTTI: &[GeometryType] = &[
    GeometryType::Point,
    GeometryType::LineString,
    GeometryType::Polygon,
    GeometryType::MultiPoint,
    GeometryType::MultiLineString,
    GeometryType::MultiPolygon,
    GeometryType::GeometryCollection,
];

fn proprieta(
    declaration: TypesDeclaration,
    types: Vec<GeometryType>,
) -> Result<ContractProperty<GeometryTypesProperty>> {
    let types = GeometryTypesProperty::new(declaration, types).map_err(|error| {
        PlenoraError::Internal(format!("mappa tipi di output incoerente: {error}"))
    })?;
    Ok(ContractProperty::new(
        PropertyConfidence::Declared(types),
        PropertyScope::Schema,
    ))
}

fn fissi(types: &[GeometryType]) -> Result<ContractProperty<GeometryTypesProperty>> {
    proprieta(TypesDeclaration::Exact, types.to_vec())
}

/// `explode`: un livello di multi-parte o collezione.
const fn di_explode(tipo: GeometryType) -> &'static [GeometryType] {
    match tipo {
        GeometryType::Point | GeometryType::MultiPoint => &[GeometryType::Point],
        GeometryType::LineString | GeometryType::MultiLineString => &[GeometryType::LineString],
        GeometryType::Polygon | GeometryType::MultiPolygon => &[GeometryType::Polygon],
        _ => TUTTI,
    }
}

/// `subdivide`: sotto la soglia di vertici la geometria passa invariata,
/// sopra si spezza (poligoni in `Polygon`, linee in `LineString`, punti in
/// `MultiPoint` a blocchi, collezioni nei membri).
const fn di_subdivide(tipo: GeometryType) -> &'static [GeometryType] {
    match tipo {
        GeometryType::Point => &[GeometryType::Point],
        GeometryType::LineString => &[GeometryType::LineString],
        GeometryType::Polygon => &[GeometryType::Polygon],
        GeometryType::MultiPoint => &[GeometryType::MultiPoint],
        GeometryType::MultiLineString => &[GeometryType::LineString, GeometryType::MultiLineString],
        GeometryType::MultiPolygon => &[GeometryType::Polygon, GeometryType::MultiPolygon],
        _ => TUTTI,
    }
}

/// `split`: le sorgenti lineari danno `LineString`, le poligonali
/// `Polygon`; gli altri tipi sono rifiutati dal kernel e non producono
/// righe.
const fn di_split(tipo: GeometryType) -> &'static [GeometryType] {
    match tipo {
        GeometryType::LineString => &[GeometryType::LineString],
        GeometryType::Polygon | GeometryType::MultiPolygon => &[GeometryType::Polygon],
        _ => &[],
    }
}

/// `collect`: il gruppo di una sola geometria resta quella; un gruppo
/// omogeneo di `Point`/`LineString`/`Polygon` diventa il multi
/// corrispondente; ogni altro gruppo una `GeometryCollection`.
fn di_collect(tipi: &[GeometryType]) -> Vec<GeometryType> {
    let mut uscita = tipi.to_vec();
    for tipo in tipi {
        match tipo {
            GeometryType::Point => uscita.push(GeometryType::MultiPoint),
            GeometryType::LineString => uscita.push(GeometryType::MultiLineString),
            GeometryType::Polygon => uscita.push(GeometryType::MultiPolygon),
            _ => {}
        }
    }
    let omogeneo_semplice = matches!(
        tipi,
        [GeometryType::Point | GeometryType::LineString | GeometryType::Polygon]
    );
    if !omogeneo_semplice {
        uscita.push(GeometryType::GeometryCollection);
    }
    uscita
}

/// La dichiarazione dell'ingresso portata sull'uscita con `mappa`: `exact`
/// e `mixed` con elenco mappano l'elenco; il resto resta com'e'.
fn per_tipo(
    ingresso: &ContractProperty<GeometryTypesProperty>,
    mappa: impl Fn(&[GeometryType]) -> Vec<GeometryType>,
) -> Result<ContractProperty<GeometryTypesProperty>> {
    let Some(dichiarati) = ingresso.value() else {
        return Ok(GeometryColumnContract::undeclared_types());
    };
    match dichiarati.declaration() {
        TypesDeclaration::Unresolved => Ok(ingresso.clone()),
        declaration if dichiarati.types().is_empty() => proprieta(declaration, Vec::new()),
        declaration => proprieta(declaration, mappa(dichiarati.types())),
    }
}

fn unione(
    tipi: &[GeometryType],
    mappa: fn(GeometryType) -> &'static [GeometryType],
) -> Vec<GeometryType> {
    tipi.iter()
        .flat_map(|tipo| mappa(*tipo).iter().copied())
        .collect()
}

/// La dichiarazione dei tipi dell'uscita di `op`, dato l'ingresso; `None`
/// per le operazioni che conservano i tipi (nessuna ridichiarazione).
///
/// # Errors
///
/// `Internal` se una mappa produce una dichiarazione incoerente (mai atteso:
/// le mappe sono statiche).
pub(in crate::analyze) fn tipi_di_uscita(
    op: &str,
    ingresso: &ContractProperty<GeometryTypesProperty>,
) -> Result<Option<ContractProperty<GeometryTypesProperty>>> {
    let tipi = match op {
        "geo.polygonize" => fissi(&[GeometryType::LineString, GeometryType::Polygon])?,
        "geo.dissolve" => fissi(&[GeometryType::MultiPolygon])?,
        "geo.overlay" => fissi(&[GeometryType::Polygon, GeometryType::MultiPolygon])?,
        "geo.delaunay" => fissi(&[GeometryType::Polygon])?,
        "geo.line_merge" => fissi(&[GeometryType::LineString])?,
        "geo.explode" => per_tipo(ingresso, |tipi| unione(tipi, di_explode))?,
        "geo.subdivide" => per_tipo(ingresso, |tipi| unione(tipi, di_subdivide))?,
        "geo.collect" => per_tipo(ingresso, di_collect)?,
        "geo.split" => {
            let tutti = [GeometryType::LineString, GeometryType::Polygon];
            match ingresso.value() {
                Some(dichiarati) if dichiarati.declaration() == TypesDeclaration::Exact => {
                    let mappati = unione(dichiarati.types(), di_split);
                    if mappati.is_empty() {
                        fissi(&tutti)?
                    } else {
                        proprieta(TypesDeclaration::Exact, mappati)?
                    }
                }
                _ => fissi(&tutti)?,
            }
        }
        "geo.line_builder" | "geo.polygon_builder" => GeometryColumnContract::undeclared_types(),
        _ => return Ok(None),
    };
    Ok(Some(tipi))
}
