//! Offset planare XY orientato, profilo GOP-017 di GEOMETRY-OPERATIONS-1.0.
//!
//! Distanza positiva a destra, estremi butt, nessuna semplificazione implicita.
//! CRS, assi e unita' sono quelli dell'ingresso, risolti dal chiamante. Nessuna
//! conversione in metri. La tolleranza riguarda la discretizzazione degli archi;
//! il certificato separa l'errore di pubblicazione GOP-015A. I valori costruiti
//! sono razionali o racchiusi da intervalli razionali certificati: entrambi gli
//! estremi devono arrotondarsi allo stesso binary64, con la funzione condivisa.
//! La topologia deve essere dimostrabile prima e dopo la pubblicazione.
//!
//! Work e prenotazioni sono quelli di `RelateLimits`, incluse le esclusioni degli
//! scratch aritmetici e dell'allocatore. Ogni ciclo ha checkpoint; fra due check
//! si esegue un numero costante di operazioni su interi limitati da noding.
//! Cancellazione precede deadline, come nelle altre API con `RelateLimits`.
//! La raffinazione ha al massimo 2560 bisezioni per radice e tentativo; un caso
//! non certificabile restituisce `AccuracyUnproven`, mai coordinate approssimate
//! senza certificato. Nessun limite di risorse cambia la tolleranza.

use crate::noding::{CountedVec, Edge, Work, P, Q};
use crate::predicates::arrangement_indexed::line_boolean::{
    ordinate, verify_publication, LineBooleanError, Originals, PublishedEdge,
};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Coord, Geometry, LineString};
use std::sync::atomic::AtomicBool;

mod enclosure;
use enclosure::{EnclosedPoint, Enclosure};

/// Raccordo esterno. I raccordi interni intersecano sempre le rette di supporto.
#[derive(Clone, Copy, Debug, Default)]
pub enum OffsetJoin {
    /// Arco circolare, approssimato entro la tolleranza richiesta.
    #[default]
    Rounded,
    /// Segmento fra i due estremi traslati.
    Bevelled,
    /// Intersezione delle parallele; oltre il rapporto limite usa bevel.
    Mitered {
        /// Rapporto finito >= 1 fra lunghezza del mitre e distanza assoluta.
        limit: f64,
    },
}

/// Parametri geometrici in unita' delle coordinate del modello planare.
#[derive(Clone, Copy, Debug)]
pub struct OffsetOptions {
    /// Distanza con segno; zero conserva i bit e l'ordine dell'ingresso valido.
    pub distance: f64,
    /// Raccordo esterno, rounded per default tramite `OffsetJoin::default`.
    pub join: OffsetJoin,
    /// Limite Hausdorff positivo finito per le corde degli archi.
    /// Obbligatorio anche quando il percorso non produce archi.
    pub arc_tolerance: f64,
}

impl OffsetOptions {
    /// Parametri con raccordo rounded predefinito; valori verificati dall'API.
    #[must_use]
    pub const fn new(distance: f64, arc_tolerance: f64) -> Self {
        Self {
            distance,
            join: OffsetJoin::Rounded,
            arc_tolerance,
        }
    }
}

/// Certificato GOP-006 nel modello planare e nelle unita' dell'ingresso.
#[derive(Clone, Copy, Debug)]
pub struct OffsetAccuracy {
    /// Bound Hausdorff simmetrico della sola discretizzazione; zero senza archi.
    pub discretization_bound: f64,
    /// Bound assoluto della pubblicazione, separato dalla discretizzazione.
    /// Il bound totale e' la somma matematica dei due campi; relativo zero.
    pub rounding_bound: f64,
}

/// Polilinea orientata con certificato; puo' essere non semplice, senza repair.
#[derive(Debug)]
pub struct OffsetResult {
    /// Vertici in ordine di percorrenza, compresi eventuali incroci non nodati.
    pub line: LineString<f64>,
    /// Modello planare, nessuna trasformazione; incertezza del datum esclusa.
    pub accuracy: OffsetAccuracy,
}

/// Errori tipizzati e senza dati; nessun prefisso di successo viene restituito.
#[derive(Debug, thiserror::Error)]
pub enum OffsetError {
    /// Parametri non finiti, tolleranza non positiva o limite mitre < 1.
    #[error("parametri di offset non validi")]
    InvalidConfiguration,
    /// Coordinate non finite o struttura priva di un segmento.
    #[error("struttura della polilinea non valida")]
    InvalidGeometry,
    /// Tipo escluso, linea chiusa/non semplice, lati nulli, reversal o collasso.
    #[error("costruzione di offset non supportata")]
    Unsupported,
    /// Impossibile certificare arrotondamento o topologia entro la raffinazione.
    #[error("GEOMETRY_ACCURACY_UNPROVEN")]
    AccuracyUnproven,
    /// La pubblicazione cambia contatti, incroci, ordine o orientamento.
    #[error("topologia non conservata dalla pubblicazione binary64")]
    InvalidRoundedTopology,
    /// Coordinata costruita non pubblicabile nel dominio binary64 finito.
    #[error("coordinata costruita fuori dal dominio binary64 finito")]
    CoordinateOutOfRange,
    /// Limiti, allocazione, aritmetica, cancellazione e deadline condivisi.
    #[error(transparent)]
    Predicate(#[from] PredicateError),
}
impl From<LineBooleanError> for OffsetError {
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

/// Offset di una singola `LineString` aperta semplice, senza opzioni di servizio.
///
/// I vertici collineari intermedi possono essere omessi se la distanza non e' zero.
/// I limiti includono validazione, tentativi di raffinazione, archi, certificati
/// e uscita; il numero di vertici degli archi e' addebitato prima della crescita.
///
/// # Errors
/// Configurazione o geometria invalida, dominio escluso, precisione/topologia
/// non certificabile, coordinate fuori dominio, risorse, cancellazione o deadline.
pub fn offset_with_limits(
    input: &Geometry<f64>,
    options: OffsetOptions,
    limits: RelateLimits,
    cancelled: Option<&AtomicBool>,
) -> Result<OffsetResult, OffsetError> {
    execute(input, options, &mut Work::new(limits, cancelled))
}

fn rational(value: f64, work: &Work<'_>) -> Result<Q, OffsetError> {
    Ok(P::of_in(Coord { x: value, y: 0.0 }, work)?.x)
}
fn dot(a: &P, b: &P) -> Result<Q, OffsetError> {
    Ok(a.x.try_mul(&b.x)?.try_add(&a.y.try_mul(&b.y)?)?)
}

fn validate(input: &Geometry<f64>, work: &mut Work<'_>) -> Result<CountedVec<P>, OffsetError> {
    let Geometry::LineString(line) = input else {
        return Err(OffsetError::Unsupported);
    };
    if line.0.len() < 2 {
        return Err(OffsetError::InvalidGeometry);
    }
    let mut points = CountedVec::new();
    for coord in &line.0 {
        work.nodes(1)?;
        if !coord.x.is_finite() || !coord.y.is_finite() {
            return Err(OffsetError::InvalidGeometry);
        }
        let point = P::of_in(*coord, work)?;
        if points.last() == Some(&point) {
            return Err(OffsetError::Unsupported);
        }
        work.push(&mut points, point)?;
    }
    if points.first() == points.last() {
        return Err(OffsetError::Unsupported);
    }
    let mut edges = CountedVec::new();
    for pair in points.windows(2) {
        work.nodes(2)?;
        work.push(
            &mut edges,
            Edge {
                a: pair[0].try_clone()?,
                b: pair[1].try_clone()?,
            },
        )?;
    }
    let mut refs = CountedVec::new();
    for edge in &edges {
        work.push(&mut refs, edge)?;
    }
    let mut invalid = false;
    crate::noding::visit(&refs, work, |i, j, hits, _| {
        if hits[1].is_some() || (hits[0].is_some() && i.abs_diff(j) != 1) {
            invalid = true;
        }
        Ok(())
    })?;
    if invalid {
        return Err(OffsetError::Unsupported);
    }
    let mut reduced = CountedVec::<P>::new();
    for point in points {
        work.step()?;
        if reduced.len() >= 2 {
            let last = reduced.len() - 1;
            let a = reduced[last].try_sub(&reduced[last - 1])?;
            let b = point.try_sub(&reduced[last])?;
            if a.try_cross(&b)?.sign().is_eq() {
                if !dot(&a, &b)?.sign().is_gt() {
                    return Err(OffsetError::Unsupported);
                }
                reduced.pop();
            }
        }
        work.push(&mut reduced, point)?;
    }
    Ok(reduced)
}

fn execute(
    input: &Geometry<f64>,
    options: OffsetOptions,
    work: &mut Work<'_>,
) -> Result<OffsetResult, OffsetError> {
    work.step()?;
    if !options.distance.is_finite()
        || matches!(options.join, OffsetJoin::Mitered { limit } if !limit.is_finite() || limit < 1.0)
    {
        return Err(OffsetError::InvalidConfiguration);
    }
    let points = validate(input, work)?;
    if options.distance.to_bits() << 1 == 0 {
        let Geometry::LineString(line) = input else {
            return Err(OffsetError::Unsupported);
        };
        let mut copy = CountedVec::new();
        for coord in &line.0 {
            work.nodes(1)?;
            work.push(&mut copy, *coord)?;
        }
        let line = LineString(copy.export(work)?);
        work.step()?;
        return Ok(OffsetResult {
            line,
            accuracy: OffsetAccuracy {
                discretization_bound: 0.0,
                rounding_bound: 0.0,
            },
        });
    }
    for precision in [80, 160, 320, 640, 1280, 2560] {
        work.step()?;
        match construct(&points, options, precision, work) {
            Err(OffsetError::AccuracyUnproven) => {}
            result => return result,
        }
    }
    Err(OffsetError::AccuracyUnproven)
}

struct Parallel {
    direction: P,
    shift: EnclosedPoint,
    length: Enclosure,
    square: Q,
}
fn parallels(
    points: &[P],
    distance: f64,
    precision: u32,
    work: &mut Work<'_>,
) -> Result<CountedVec<Parallel>, OffsetError> {
    let distance = Enclosure::exact(rational(distance, work)?)?;
    let mut result = CountedVec::new();
    for pair in points.windows(2) {
        work.nodes(1)?;
        let direction = pair[1].try_sub(&pair[0])?;
        let length = enclosure::length(&direction, precision, work)?;
        let normal = P {
            x: direction.y.try_clone()?,
            y: work.integer(0)?.try_sub(&direction.x)?,
        };
        let shift = EnclosedPoint::exact(&normal)?.scale(&distance.div(&length, work)?)?;
        let square = dot(&direction, &direction)?;
        work.push(
            &mut result,
            Parallel {
                direction,
                shift,
                length,
                square,
            },
        )?;
    }
    Ok(result)
}

fn append(
    points: &mut CountedVec<EnclosedPoint>,
    p: EnclosedPoint,
    work: &mut Work<'_>,
) -> Result<(), OffsetError> {
    work.nodes(1)?;
    work.push(points, p)?;
    Ok(())
}

// Sistema delle due normali: x=d*(dx1*L2-dx2*L1)/cross,
// y=d*(dy1*L2-dy2*L1)/cross. Radici proporzionali si combinano prima
// di racchiuderle: le cancellazioni esatte restano zeri esatti.
fn mitre(
    a: &Parallel,
    b: &Parallel,
    distance: f64,
    work: &Work<'_>,
) -> Result<EnclosedPoint, OffsetError> {
    let denominator = Enclosure::exact(a.direction.try_cross(&b.direction)?)?;
    let factor = Enclosure::exact(rational(distance, work)?)?.div(&denominator, work)?;
    let ratio = a.square.try_div(&b.square)?.try_sqrt_exact()?;
    let component = |x: &Q, y: &Q| -> Result<Enclosure, OffsetError> {
        let value = if let Some(ratio) = &ratio {
            Enclosure::exact(x.try_sub(&y.try_mul(ratio)?)?)?.mul(&b.length)?
        } else {
            Enclosure::exact(x.try_clone()?)?
                .mul(&b.length)?
                .sub(&Enclosure::exact(y.try_clone()?)?.mul(&a.length)?)?
        };
        value.mul(&factor)
    };
    Ok(EnclosedPoint {
        x: component(&a.direction.x, &b.direction.x)?,
        y: component(&a.direction.y, &b.direction.y)?,
    })
}

fn arc(
    output: &mut CountedVec<EnclosedPoint>,
    center: &P,
    a: &Parallel,
    b: &Parallel,
    options: OffsetOptions,
    work: &mut Work<'_>,
) -> Result<(), OffsetError> {
    let radius = rational(options.distance.abs(), work)?;
    let product = a.square.try_mul(&b.square)?;
    let lengths = if let Some(root) = product.try_sqrt_exact()? {
        Enclosure::exact(root)?
    } else {
        a.length.mul(&b.length)?
    };
    let normal = EnclosedPoint::exact(&P {
        x: a.direction.y.try_clone()?,
        y: work.integer(0)?.try_sub(&a.direction.x)?,
    })?;
    let scale = Enclosure::exact(rational(options.distance, work)?)?.div(&a.length, work)?;
    let one = Enclosure::exact(work.integer(1)?)?;
    let two = Enclosure::exact(work.integer(2)?)?;
    // t = tan(theta/2); theta e' il giro esterno minore, mai una inversione.
    let end = Enclosure::exact(a.direction.try_cross(&b.direction)?)?.div(
        &lengths.add(&Enclosure::exact(dot(&a.direction, &b.direction)?)?)?,
        work,
    )?;
    let bound = end.mul(&end)?.high;
    let tolerance = rational(options.arc_tolerance, work)?;
    let numerator = radius.try_mul(&bound)?;
    let mut count = 1_u64;
    let mut count_q = work.integer(1)?;
    while numerator
        .try_cmp(
            &tolerance
                .try_mul(&work.integer(2)?)?
                .try_mul(&count_q.try_mul(&count_q)?)?,
        )?
        .is_gt()
    {
        work.step()?;
        count = count.checked_mul(2).ok_or(PredicateError::WorkLimit)?;
        count_q = count_q.try_mul(&work.integer(2)?)?;
    }
    // |d theta / dt| <= 2; freccia <= r*(delta theta)^2/8.
    // Il conteggio precede tutte le costruzioni dei vertici intermedi.
    work.nodes(count - 1)?;
    let center = EnclosedPoint::exact(center)?;
    let mut index_q = work.integer(1)?;
    for _ in 1..count {
        work.step()?;
        let fraction = Enclosure::exact(index_q.try_div(&count_q)?)?;
        let t = end.mul(&fraction)?;
        let t2 = t.mul(&t)?;
        let denominator = one.add(&t2)?;
        let cosine = one.sub(&t2)?.div(&denominator, work)?;
        let sine = two.mul(&t)?.div(&denominator, work)?;
        let rotated = EnclosedPoint {
            x: normal.x.mul(&cosine)?.sub(&normal.y.mul(&sine)?)?,
            y: normal.x.mul(&sine)?.add(&normal.y.mul(&cosine)?)?,
        };
        work.push(output, center.add(&rotated.scale(&scale)?)?)?;
        index_q = index_q.try_add(&work.integer(1)?)?;
    }
    append(output, center.add(&b.shift)?, work)
}

fn forward(start: &EnclosedPoint, end: &EnclosedPoint, direction: &P) -> Result<(), OffsetError> {
    if !end
        .sub(start)?
        .dot(&EnclosedPoint::exact(direction)?)?
        .sign()?
        .is_gt()
    {
        return Err(OffsetError::Unsupported);
    }
    Ok(())
}

fn construct(
    points: &[P],
    options: OffsetOptions,
    precision: u32,
    work: &mut Work<'_>,
) -> Result<OffsetResult, OffsetError> {
    let parallel = parallels(points, options.distance, precision, work)?;
    let mut output = CountedVec::new();
    let mut start = EnclosedPoint::exact(&points[0])?.add(&parallel[0].shift)?;
    append(&mut output, start.copy()?, work)?;
    let mut arcs = CountedVec::new();
    for i in 1..points.len() - 1 {
        work.step()?;
        let (a, b) = (&parallel[i - 1], &parallel[i]);
        let turn = a.direction.try_cross(&b.direction)?.sign();
        let outer = turn.is_gt() == (options.distance > 0.0);
        let intersection = mitre(a, b, options.distance, work)?;
        let center = EnclosedPoint::exact(&points[i])?;
        let bevel = if outer {
            match options.join {
                OffsetJoin::Mitered { limit } => {
                    let threshold =
                        rational(limit, work)?.try_mul(&rational(options.distance, work)?)?;
                    intersection
                        .dot(&intersection)?
                        .sub(&Enclosure::exact(threshold.try_mul(&threshold)?)?)?
                        .sign()?
                        .is_gt()
                }
                _ => true,
            }
        } else {
            false
        };
        if bevel {
            let end = center.add(&a.shift)?;
            forward(&start, &end, &a.direction)?;
            append(&mut output, end, work)?;
            start = center.add(&b.shift)?;
            if matches!(options.join, OffsetJoin::Rounded) {
                if !options.arc_tolerance.is_finite() || options.arc_tolerance <= 0.0 {
                    return Err(OffsetError::InvalidConfiguration);
                }
                let first = output.len() - 1;
                arc(&mut output, &points[i], a, b, options, work)?;
                work.push(
                    &mut arcs,
                    ArcRegion {
                        first,
                        last: output.len() - 1,
                        center: &points[i],
                    },
                )?;
            } else {
                append(&mut output, start.copy()?, work)?;
            }
        } else {
            let end = center.add(&intersection)?;
            forward(&start, &end, &a.direction)?;
            append(&mut output, end.copy()?, work)?;
            start = end;
        }
    }
    let last = parallel.len() - 1;
    let end = EnclosedPoint::exact(&points[points.len() - 1])?.add(&parallel[last].shift)?;
    forward(&start, &end, &parallel[last].direction)?;
    append(&mut output, end, work)?;
    certify_arcs(&output, &arcs, options, work)?;
    publish(
        &output,
        if arcs.is_empty() {
            0.0
        } else {
            options.arc_tolerance
        },
        work,
    )
}

// Gli inviluppi separati certificano l'assenza di incidenza senza orientazioni.
// Negli altri casi si certificano i segni necessari sull'intero intervallo.
#[allow(clippy::too_many_lines)]
fn certify_topology(points: &[EnclosedPoint], work: &mut Work<'_>) -> Result<(), OffsetError> {
    let mut parameters = CountedVec::new();
    for _ in points.windows(2) {
        let mut row = CountedVec::new();
        work.push(&mut row, Enclosure::exact(work.integer(0)?)?)?;
        work.push(&mut row, Enclosure::exact(work.integer(1)?)?)?;
        work.push(&mut parameters, row)?;
    }
    for i in 0..points.len() - 1 {
        for j in i + 1..points.len() - 1 {
            work.pair()?;
            let mut separated = false;
            for axis in [true, false] {
                let coord = |k: usize| if axis { &points[k].x } else { &points[k].y };
                for (a, b) in [(i, j), (j, i)] {
                    if coord(a).high.try_cmp(&coord(b).low)?.is_lt()
                        && coord(a).high.try_cmp(&coord(b + 1).low)?.is_lt()
                        && coord(a + 1).high.try_cmp(&coord(b).low)?.is_lt()
                        && coord(a + 1).high.try_cmp(&coord(b + 1).low)?.is_lt()
                    {
                        separated = true;
                    }
                }
            }
            if separated {
                continue;
            }
            let mut signs = [Some(std::cmp::Ordering::Equal); 4];
            for (position, [a, b, c]) in [
                [i, i + 1, j],
                [i, i + 1, j + 1],
                [j, j + 1, i],
                [j, j + 1, i + 1],
            ]
            .into_iter()
            .enumerate()
            {
                work.step()?;
                if a != c && b != c {
                    signs[position] = match points[b]
                        .sub(&points[a])?
                        .cross(&points[c].sub(&points[a])?)?
                        .sign()
                    {
                        Ok(sign) => Some(sign),
                        Err(OffsetError::AccuracyUnproven) => None,
                        Err(error) => return Err(error),
                    };
                }
                if position % 2 == 1 {
                    if let (Some(a), Some(b)) = (signs[position - 1], signs[position]) {
                        if a == b && !a.is_eq() {
                            separated = true;
                            break;
                        }
                    }
                }
            }
            if separated {
                continue;
            }
            let [Some(first_start), Some(first_end), Some(second_start), Some(second_end)] = signs
            else {
                return Err(OffsetError::AccuracyUnproven);
            };
            let signs = [first_start, first_end, second_start, second_end];
            if j == i + 1 {
                continue;
            }
            let straddles =
                |a: std::cmp::Ordering, b: std::cmp::Ordering| a.is_eq() || b.is_eq() || a != b;
            if straddles(signs[0], signs[1]) && straddles(signs[2], signs[3]) {
                let r = points[i + 1].sub(&points[i])?;
                let s = points[j + 1].sub(&points[j])?;
                let denominator = r.cross(&s)?;
                if denominator.sign()?.is_eq() {
                    // Gli overlap esatti sono gestiti dal certificato razionale;
                    // con intervalli non puntuali la costruzione e' ambigua.
                    for p in [&points[i], &points[i + 1], &points[j], &points[j + 1]] {
                        if p.x.low != p.x.high || p.y.low != p.y.high {
                            return Err(OffsetError::AccuracyUnproven);
                        }
                    }
                    // Inserire i parametri di tutti gli estremi collineari;
                    // anche gli esterni ordinano correttamente i siti in [0,1].
                    for (edge, other) in [(i, j), (j, i)] {
                        let direction = points[edge + 1].sub(&points[edge])?;
                        let axis_x = !direction.x.sign()?.is_eq();
                        for p in [&points[other], &points[other + 1]] {
                            let delta = p.sub(&points[edge])?;
                            let t = if axis_x {
                                delta.x.div(&direction.x, work)?
                            } else {
                                delta.y.div(&direction.y, work)?
                            };
                            work.push(&mut parameters[edge], t)?;
                        }
                    }
                } else {
                    let delta = points[j].sub(&points[i])?;
                    let t = delta.cross(&s)?.div(&denominator, work)?;
                    let u = delta.cross(&r)?.div(&denominator, work)?;
                    work.nodes(2)?;
                    work.push(&mut parameters[i], t)?;
                    work.push(&mut parameters[j], u)?;
                }
            }
        }
    }
    for row in &parameters {
        for (i, a) in row.iter().enumerate() {
            for b in &row[i + 1..] {
                work.step()?;
                a.sub(b)?.sign()?;
            }
        }
    }
    Ok(())
}

struct ArcRegion<'a> {
    first: usize,
    last: usize,
    center: &'a P,
}

// Certificato conservativo: ogni pezzo estraneo deve restare fuori dal disco
// dell'arco, maggiorato della tolleranza degli altri archi. Le due tangenti
// incidenti sono escluse: il raccordo minore non puo' incontrarle altrove.
// Cosi' la lente fra corda e arco non puo' creare o cancellare un contatto.
fn certify_arcs(
    points: &[EnclosedPoint],
    arcs: &[ArcRegion<'_>],
    options: OffsetOptions,
    work: &mut Work<'_>,
) -> Result<(), OffsetError> {
    if arcs.is_empty() {
        return Ok(());
    }
    let radius =
        rational(options.distance.abs(), work)?.try_add(&rational(options.arc_tolerance, work)?)?;
    let square = Enclosure::exact(radius.try_mul(&radius)?)?;
    for arc in arcs {
        let center = EnclosedPoint::exact(arc.center)?;
        for (i, pair) in points.windows(2).enumerate() {
            work.step()?;
            if i + 1 >= arc.first && i <= arc.last {
                continue;
            }
            work.pair()?;
            let a = pair[0].sub(&center)?;
            let b = pair[1].sub(&center)?;
            let direction = b.sub(&a)?;
            let from = a.dot(&direction)?;
            let to = b.dot(&direction)?;
            let distance_square = if from.sign()?.is_ge() {
                a.dot(&a)?
            } else if to.sign()?.is_le() {
                b.dot(&b)?
            } else {
                let cross = a.cross(&direction)?;
                cross.mul(&cross)?.div(&direction.dot(&direction)?, work)?
            };
            if !distance_square.sub(&square)?.sign()?.is_gt() {
                return Err(OffsetError::AccuracyUnproven);
            }
        }
    }
    Ok(())
}

fn absolute(q: Q, work: &Work<'_>) -> Result<Q, OffsetError> {
    Ok(if q.sign().is_lt() {
        work.integer(0)?.try_sub(&q)?
    } else {
        q
    })
}
fn error_bound(value: f64, interval: &Enclosure, work: &Work<'_>) -> Result<Q, OffsetError> {
    let value = rational(value, work)?;
    let low = absolute(value.try_sub(&interval.low)?, work)?;
    let high = absolute(value.try_sub(&interval.high)?, work)?;
    Ok(if low.try_cmp(&high)?.is_gt() {
        low
    } else {
        high
    })
}
fn publish(
    points: &[EnclosedPoint],
    arc_bound: f64,
    work: &mut Work<'_>,
) -> Result<OffsetResult, OffsetError> {
    certify_topology(points, work)?;
    let mut exact = CountedVec::new();
    let mut coords = CountedVec::new();
    let mut error = work.integer(0)?;
    for point in points {
        work.nodes(2)?;
        let coord = Coord {
            x: point.x.rounded(work)?,
            y: point.y.rounded(work)?,
        };
        let bound = error_bound(coord.x, &point.x, work)?
            .try_add(&error_bound(coord.y, &point.y, work)?)?;
        if bound.try_cmp(&error)?.is_gt() {
            error = bound;
        }
        work.push(&mut coords, coord)?;
        work.push(&mut exact, point.middle()?)?;
    }
    let mut edges = CountedVec::new();
    for pair in exact.windows(2) {
        work.nodes(2)?;
        work.push(
            &mut edges,
            Edge {
                a: pair[0].try_clone()?,
                b: pair[1].try_clone()?,
            },
        )?;
    }
    let empty = Geometry::LineString(LineString(Vec::new()));
    let originals = Originals::new(&empty, &empty, work)?;
    let mut certificates = CountedVec::new();
    for (edge, pair) in edges.iter().zip(coords.windows(2)) {
        let certificate = PublishedEdge::new(edge, [pair[0], pair[1]], work)?;
        work.push(&mut certificates, certificate)?;
    }
    verify_publication(&certificates, &originals, true, None, work)?;
    let mut rounding_bound = ordinate(&error, work)?;
    if rational(rounding_bound, work)?.try_cmp(&error)?.is_lt() {
        rounding_bound = rounding_bound.next_up();
    }
    if !rounding_bound.is_finite() {
        return Err(OffsetError::CoordinateOutOfRange);
    }
    let line = LineString(coords.export(work)?);
    work.step()?;
    Ok(OffsetResult {
        line,
        accuracy: OffsetAccuracy {
            discretization_bound: arc_bound,
            rounding_bound,
        },
    })
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
