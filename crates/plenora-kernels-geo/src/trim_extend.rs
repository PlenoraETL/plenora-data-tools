//! Editing planare XY GOP-018: taglio sinistro o estensione terminale esatta.
//!
//! CRS, assi e unita' risolti dal chiamante; nessuna riproiezione, Z/M o curve.
//! La guida rappresenta la retta infinita, anche fuori dai propri estremi.
//! GOP-015A: nearest-even condiviso, vertici originali invariati, verifica unica
//! della topologia dopo pubblicazione. Nessuna riparazione o tolleranza implicita.
//! Risorse ed esclusioni contabili di `RelateLimits`; prenotazioni prima della
//! crescita. Ogni ciclo controlla deadline e cancellazione, in questo ordine.
//! Fra checkpoint: al massimo 256 primitive Q (conversione, copia, confronto,
//! segno o aritmetica), con P espanse; interi limitati dal tetto di noding.
//! Allocazioni e drop esclusi da questa misura; nessuna garanzia wall-clock.
//! Nessuna serializzazione; dettagli del bound nella guida trim-extend.

use crate::noding::{CountedVec, Edge, Work, P, Q};
use crate::predicates::arrangement_indexed::line_boolean::{
    verify_publication, LineBooleanError, Originals, PublishedEdge,
};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Geometry, LineString};
use std::sync::atomic::AtomicBool;

/// Estremi abilitati per l'estensione; non modifica la selezione del taglio.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrimExtendEnds {
    /// Solo il raggio opposto al primo lato.
    Start,
    /// Solo il raggio lungo l'ultimo lato.
    End,
    /// Entrambi i raggi terminali.
    #[default]
    Both,
    /// Nessuna estensione.
    Neither,
}

/// Ricostruzione dell'unico estremo esteso.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrimExtendMode {
    /// Conserva l'estremo originale aggiungendo il punto costruito.
    #[default]
    AddVertex,
    /// Sostituisce l'estremo originale con il punto costruito.
    MoveTerminalVertex,
}

/// Opzioni semantiche GOP-018, prive di flag di servizio.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrimExtendOptions {
    /// Default: entrambi gli estremi.
    pub ends: TrimExtendEnds,
    /// Default: aggiunta di un vertice.
    pub mode: TrimExtendMode,
}

/// Errori tipizzati senza coordinate o valori dell'ingresso.
#[derive(Debug, thiserror::Error)]
pub enum TrimExtendError {
    /// Coordinate non finite o struttura senza segmenti.
    #[error("struttura della polilinea non valida")]
    InvalidGeometry,
    /// Dominio escluso o costruzione ambigua/non semplice di GOP-018.
    #[error("costruzione trim extend non supportata")]
    Unsupported,
    /// La pubblicazione modifica la topologia esatta.
    #[error("topologia non conservata dalla pubblicazione binary64")]
    InvalidRoundedTopology,
    /// Punto costruito fuori dal dominio binary64 finito.
    #[error("coordinata costruita fuori dal dominio binary64 finito")]
    CoordinateOutOfRange,
    /// Risorse, aritmetica, cancellazione e deadline condivise.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}
impl From<LineBooleanError> for TrimExtendError {
    fn from(error: LineBooleanError) -> Self {
        match error {
            LineBooleanError::InvalidRoundedTopology => Self::InvalidRoundedTopology,
            LineBooleanError::CoordinateOutOfRange => Self::CoordinateOutOfRange,
            LineBooleanError::Predicate(error) => Self::Predicate(error),
            LineBooleanError::UnsupportedOperands | LineBooleanError::InvalidOutputDimension => {
                Self::Unsupported
            }
        }
    }
}

/// Taglia una linea aperta semplice a sinistra della guida diretta rettilinea.
///
/// Senza tagli, estende l'unico raggio abilitato con colpo a distanza positiva;
/// zero colpi conserva tutti i bit e l'ordine del target. La guida puo' avere
/// vertici intermedi collineari, senza inversioni. Le opzioni non sono bit flag.
///
/// Certificato GOP-015A: costruzione razionale esatta, errore per ordinata <=
/// mezza ulp, nearest-even, nessuna discretizzazione o trasformazione. Il modello
/// e le unita' sono quelli planari dell'ingresso; incertezza del datum esclusa.
/// Limiti cumulativi di validazione, costruzione e verifica come `RelateLimits`.
/// GOP-028: deadline scaduta precede cancellazione al medesimo checkpoint.
///
/// # Errors
/// Geometria malformata, dominio escluso GOP-018, topologia alterata dal rounding,
/// coordinate fuori dominio, risorse, cancellazione o deadline.
pub(crate) fn trim_extend_with_limits(
    target: &Geometry<f64>,
    guide: &Geometry<f64>,
    options: TrimExtendOptions,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<LineString<f64>, TrimExtendError> {
    execute(target, guide, options, &mut Work::new(limits, cancelled))
}

fn read(input: &Geometry<f64>, work: &mut Work<'_>) -> Result<CountedVec<P>, TrimExtendError> {
    read_path(input, false, work)
}

pub(crate) fn read_path(
    input: &Geometry<f64>,
    allow_closed: bool,
    work: &mut Work<'_>,
) -> Result<CountedVec<P>, TrimExtendError> {
    let Geometry::LineString(line) = input else {
        return Err(TrimExtendError::Unsupported);
    };
    if line.0.len() < 2 {
        return Err(TrimExtendError::InvalidGeometry);
    }
    let mut points = CountedVec::new();
    for coord in &line.0 {
        work.nodes(1)?;
        if !coord.x.is_finite() || !coord.y.is_finite() {
            return Err(TrimExtendError::InvalidGeometry);
        }
        work.push(&mut points, P::of_in(*coord, work)?)?;
    }
    simple_path(&points, allow_closed, work)?;
    Ok(points)
}

pub(crate) fn edges(
    points: &[P],
    work: &mut Work<'_>,
) -> Result<CountedVec<Edge>, TrimExtendError> {
    let mut result = CountedVec::new();
    for pair in points.windows(2) {
        work.nodes(2)?;
        if pair[0] == pair[1] {
            return Err(TrimExtendError::Unsupported);
        }
        work.push(
            &mut result,
            Edge {
                a: pair[0].try_clone()?,
                b: pair[1].try_clone()?,
            },
        )?;
    }
    Ok(result)
}

pub(crate) fn simple(points: &[P], work: &mut Work<'_>) -> Result<(), TrimExtendError> {
    simple_path(points, false, work)
}

fn simple_path(
    points: &[P],
    allow_closed: bool,
    work: &mut Work<'_>,
) -> Result<(), TrimExtendError> {
    work.step()?;
    let closed = points.first() == points.last();
    if points.len() < 2 || (closed && !allow_closed) {
        return Err(TrimExtendError::Unsupported);
    }
    let edges = edges(points, work)?;
    let mut refs = CountedVec::new();
    for edge in &edges {
        work.push(&mut refs, edge)?;
    }
    let mut invalid = false;
    let scan = crate::noding::visit(&refs, work, |i, j, hits, _| {
        let adjacent = i.abs_diff(j) == 1 || (closed && i.abs_diff(j) == edges.len() - 1);
        if hits[1].is_some() || (hits[0].is_some() && !adjacent) {
            invalid = true;
        }
        Ok(())
    });
    if invalid {
        return Err(TrimExtendError::Unsupported);
    }
    scan?;
    Ok(())
}

fn side(point: &P, origin: &P, direction: &P) -> Result<Q, PredicateError> {
    direction.try_cross(&point.try_sub(origin)?)
}

fn intersection(a: &P, b: &P, sa: &Q, sb: &Q) -> Result<P, PredicateError> {
    let t = sa.try_div(&sa.try_sub(sb)?)?;
    let delta = b.try_sub(a)?;
    a.try_add(&P {
        x: delta.x.try_mul(&t)?,
        y: delta.y.try_mul(&t)?,
    })
}

struct Cut {
    // Il punto puo' essere un vertice interno originale oppure un nuovo nodo.
    point: P,
    before: usize,
    after: usize,
}

fn cuts(points: &[P], sides: &[Q], work: &mut Work<'_>) -> Result<Option<Cut>, TrimExtendError> {
    let mut cut = None;
    for i in 0..points.len() {
        work.step()?;
        let sign = sides[i].sign();
        let candidate = if sign.is_eq() {
            if i == 0
                || i + 1 == points.len()
                || sides[i - 1].sign().is_eq()
                || sides[i + 1].sign().is_eq()
                || sides[i - 1].sign() == sides[i + 1].sign()
            {
                return Err(TrimExtendError::Unsupported);
            }
            work.nodes(1)?;
            Some(Cut {
                point: points[i].try_clone()?,
                before: i - 1,
                after: i + 1,
            })
        } else if i + 1 < points.len()
            && !sides[i + 1].sign().is_eq()
            && sign != sides[i + 1].sign()
        {
            work.nodes(1)?;
            Some(Cut {
                point: intersection(&points[i], &points[i + 1], &sides[i], &sides[i + 1])?,
                before: i,
                after: i + 1,
            })
        } else {
            None
        };
        if candidate.is_some() {
            if cut.is_some() {
                return Err(TrimExtendError::Unsupported);
            }
            cut = candidate;
        }
    }
    Ok(cut)
}

pub(crate) fn append(
    output: &mut CountedVec<P>,
    point: &P,
    work: &mut Work<'_>,
) -> Result<(), TrimExtendError> {
    work.nodes(1)?;
    work.push(output, point.try_clone()?)?;
    Ok(())
}

fn execute(
    target: &Geometry<f64>,
    guide: &Geometry<f64>,
    options: TrimExtendOptions,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, TrimExtendError> {
    work.step()?;
    let points = read(target, work)?;
    if let Geometry::LineString(line) = guide {
        if line.0.windows(2).all(|pair| pair[0] == pair[1]) {
            return Err(TrimExtendError::InvalidGeometry);
        }
    }
    let guide_points = read(guide, work)?;
    let origin = &guide_points[0];
    let direction = guide_points[1].try_sub(origin)?;
    for point in &guide_points[2..] {
        work.pair()?;
        if !side(point, origin, &direction)?.sign().is_eq() {
            return Err(TrimExtendError::Unsupported);
        }
    }
    let mut sides = CountedVec::new();
    for point in &points {
        work.pair()?;
        work.push(&mut sides, side(point, origin, &direction)?)?;
    }
    let mut output = CountedVec::new();
    if let Some(cut) = cuts(&points, &sides, work)? {
        if sides[cut.before].sign().is_gt() {
            for point in &points[..=cut.before] {
                append(&mut output, point, work)?;
            }
            append(&mut output, &cut.point, work)?;
        } else {
            append(&mut output, &cut.point, work)?;
            for point in &points[cut.after..] {
                append(&mut output, point, work)?;
            }
        }
    } else {
        let last = points.len() - 1;
        let mut hit = None;
        for (enabled, terminal, neighbor) in [
            (
                matches!(options.ends, TrimExtendEnds::Start | TrimExtendEnds::Both),
                0,
                1,
            ),
            (
                matches!(options.ends, TrimExtendEnds::End | TrimExtendEnds::Both),
                last,
                last - 1,
            ),
        ] {
            work.step()?;
            if !enabled {
                continue;
            }
            work.pair()?;
            let denominator = sides[terminal].try_sub(&sides[neighbor])?;
            if denominator.sign().is_eq() {
                continue;
            }
            // P(t)=terminale+t*(terminale-vicino), t>0.
            if sides[terminal].sign() == denominator.sign() {
                continue;
            }
            work.nodes(1)?;
            let point = intersection(
                &points[terminal],
                &points[neighbor],
                &sides[terminal],
                &sides[neighbor],
            )?;
            if hit.is_some() {
                return Err(TrimExtendError::Unsupported);
            }
            hit = Some((terminal, point));
        }
        if let Some((terminal, point)) = hit {
            if terminal == 0 {
                append(&mut output, &point, work)?;
            }
            for (i, original) in points.iter().enumerate() {
                work.step()?;
                if i != terminal || options.mode == TrimExtendMode::AddVertex {
                    append(&mut output, original, work)?;
                }
            }
            if terminal == last {
                append(&mut output, &point, work)?;
            }
        } else {
            // La copia tramite provenienza conserva anche gli zeri con segno.
            output = points;
        }
    }
    simple(&output, work)?;
    publish(&output, target, work)
}

fn publish(
    points: &[P],
    target: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, TrimExtendError> {
    let empty = Geometry::LineString(LineString(Vec::new()));
    publish_paths(points, target, &empty, work)
}

pub(crate) fn publish_paths(
    points: &[P],
    target: &Geometry<f64>,
    reshaper: &Geometry<f64>,
    work: &mut Work<'_>,
) -> Result<LineString<f64>, TrimExtendError> {
    let originals = Originals::new(target, reshaper, work)?;
    let mut coords = CountedVec::new();
    for point in points {
        work.nodes(1)?;
        let coord = originals.coordinate(point, work)?;
        work.push(&mut coords, coord)?;
    }
    let edges = edges(points, work)?;
    let mut certificates = CountedVec::new();
    for (edge, pair) in edges.iter().zip(coords.windows(2)) {
        let certificate = PublishedEdge::new(edge, [pair[0], pair[1]], work)?;
        work.push(&mut certificates, certificate)?;
    }
    verify_publication(&certificates, &originals, false, None, work)?;
    let result = LineString(coords.export(work)?);
    work.step()?;
    Ok(result)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;

/// Ingressi con CRS verificato e consenso planare esplicito.
pub mod with_crs {
    use super::{
        AtomicBool, Geometry, LineString, RelateLimits, TrimExtendError, TrimExtendOptions,
    };

    crate::crs::checked_kernel!(trim_extend_with_limits, "geo.split", 2,
        (target: &Geometry<f64>, guide: &Geometry<f64>, options: TrimExtendOptions, limits: RelateLimits, cancelled: Option<&AtomicBool>) -> (LineString<f64>, TrimExtendError),
        |context| {  super::trim_extend_with_limits(target, guide, options, limits, cancelled) });
}
