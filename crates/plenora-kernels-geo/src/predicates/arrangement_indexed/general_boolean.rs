//! Booleane generali planari XY, GOP-014/015/015A/027/028.
//!
//! Point, `MultiPoint`, `LineString`, `MultiLineString`, Polygon, `MultiPolygon` e
//! `GeometryCollection` anche annidate. CRS risolto e assi comuni sono a carico
//! del chiamante. Nessuna riparazione: `MultiPolygon` con interni sovrapposti o
//! confini comuni di dimensione 1 sono invalidi (GOP-002, SFA 6.1.14).
//! Contatti puntuali validi ammessi; le componenti di collezioni e dell'unione
//! molti-a-uno sono interpretate come unione insiemistica prima della selezione.
//! Un solo grafo razionale, nessun arrotondamento di risultati intermedi.
//!
//! Risultati multi omogenei, collezioni miste in ordine 0/1/2, componenti
//! coperte eliminate. Il filtro non estrae confini. Vuoto generale collezione,
//! vuoto filtrato multi tipizzato. Differenza con chiusura dei residui.
//! Ordine canonico e certificato binary64 come in `polygon_boolean`;
//! a siti coincidenti prevale il primo vertice nell'ordine di input.
//! Un unico Work e limiti cumulativi per tutta la chiamata, incluso l'array.
//! Byte vivi includono risultati precedenti ancora vivi e loro ricevute.
//! Esclusioni e massimo aritmetico per valore sono quelli di `RelateLimits`.
//! Ogni iterazione controlla deadline prima del token; intervallo massimo:
//! una visita di lato, sito o candidato, con primitive razionali limitate
//! da `RelateLimits::MAX_VALUE_BYTES`. Nessuna promessa di latenza reale.

use super::polygon_boolean;
use crate::noding::{CountedVec, Work};
use geo::Geometry;
use std::sync::atomic::AtomicBool;

pub use super::polygon_boolean::{
    PolygonBooleanError as GeneralBooleanError, PolygonBooleanLimits as GeneralBooleanLimits,
    PolygonBooleanOp as GeneralBooleanOp,
};

/// Booleana binaria, con tutte le coppie di dimensioni.
/// # Errors
/// Input invalido, tipo escluso, filtro invalido, pubblicazione non certificabile,
/// risorse esaurite, allocazione fallita, cancellazione o deadline.
pub fn boolean_with_limits(
    left: &Geometry<f64>,
    right: &Geometry<f64>,
    operation: GeneralBooleanOp,
    output_dimension: Option<u8>,
    limits: GeneralBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, GeneralBooleanError> {
    let mut work = Work::new(limits.work, cancelled);
    polygon_boolean::execute_many(
        std::slice::from_ref(left),
        std::slice::from_ref(right),
        operation,
        output_dimension,
        &limits,
        &mut work,
    )
}

/// Unione molti-a-uno, incluso l'array vuoto; ordine degli input conservato
/// per la provenienza dei vertici, non per l'ordine canonico delle componenti.
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`]; nessun successo parziale.
pub fn union_with_limits(
    inputs: &[Geometry<f64>],
    output_dimension: Option<u8>,
    limits: GeneralBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Geometry<f64>, GeneralBooleanError> {
    let mut work = Work::new(limits.work, cancelled);
    polygon_boolean::execute_many(
        inputs,
        &[],
        GeneralBooleanOp::Union,
        output_dimension,
        &limits,
        &mut work,
    )
}

fn array(
    inputs: &[Geometry<f64>],
    right: &Geometry<f64>,
    op: GeneralBooleanOp,
    filter: Option<u8>,
    limits: GeneralBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<Geometry<f64>>, GeneralBooleanError> {
    let mut work = Work::new(limits.work, cancelled);
    work.step()?;
    if filter.is_some_and(|d| d > 2) {
        return Err(GeneralBooleanError::InvalidOutputDimension);
    }
    // Anche un array vuoto valida l'operando comune.
    polygon_boolean::general_supported(right, &mut work)?;
    let empty = Geometry::MultiPoint(geo::MultiPoint::new(Vec::new()));
    super::relate(right, &empty, &mut work)?;
    let mut output = CountedVec::new();
    for left in inputs {
        let result = polygon_boolean::execute_many(
            std::slice::from_ref(left),
            std::slice::from_ref(right),
            op,
            filter,
            &limits,
            &mut work,
        )?;
        work.push(&mut output, result)?;
    }
    work.step()?;
    Ok(output.export(&mut work)?)
}

/// Intersezione atomica di ogni A con B, nell'ordine di A, inclusi vuoti e duplicati.
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`]; nessun prefisso pubblicato.
pub fn intersection_with_limits(
    inputs: &[Geometry<f64>],
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: GeneralBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<Geometry<f64>>, GeneralBooleanError> {
    array(
        inputs,
        right,
        GeneralBooleanOp::Intersection,
        output_dimension,
        limits,
        cancelled,
    )
}

/// Differenza atomica di ogni A meno B, nell'ordine di A, inclusi vuoti e duplicati.
/// # Errors
/// Gli stessi errori di [`boolean_with_limits`]; nessun prefisso pubblicato.
pub fn difference_with_limits(
    inputs: &[Geometry<f64>],
    right: &Geometry<f64>,
    output_dimension: Option<u8>,
    limits: GeneralBooleanLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<Geometry<f64>>, GeneralBooleanError> {
    array(
        inputs,
        right,
        GeneralBooleanOp::Difference,
        output_dimension,
        limits,
        cancelled,
    )
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
