//! Delaunay da coordinate: contesto comune fino alla consegna dei poligoni.
use crate::noding::{sorted, CountedVec, Work};
use crate::predicates::{PredicateError, RelateLimits};
use geo::{Coord, GeoNum, Kernel, LineString, Orientation, Polygon};
use spade::Triangulation as _;

/// Esiti della costruzione Delaunay da coordinate con limiti.
#[derive(Debug, thiserror::Error)]
pub enum DelaunayBuildError {
    /// Budget, cancellazione o scadenza del contesto condiviso.
    #[error(transparent)]
    Resource(#[from] PredicateError),
    /// Dominio di Spade o verifica dei ranghi fallita; nessuna coordinata.
    #[error("costruzione della triangolazione fallita")]
    Triangulation(crate::triangolazione::ErroreTriangolazione),
    /// Barriera dei panici esterni, senza il contenuto del payload.
    #[error("costruzione della triangolazione non conclusa: {0}")]
    Calculation(&'static str),
    /// Numero effettivo delle facce oltre il limite richiesto.
    #[error("numero di triangoli oltre il limite")]
    TriangleLimit,
    /// Una faccia non e' un triangolo antiorario non degenere.
    #[error("faccia della triangolazione non valida")]
    InvalidFace,
}

/// Costruisce Delaunay da siti XY, con gli stessi bit e l'ordine di [`super::delaunay`].
///
/// L'ingresso e' una sequenza di coordinate, equivalente a un `MultiPoint`;
/// non richiede la validazione di anelli o superfici. Duplicati, zeri con
/// segno e siti cocircolari seguono il percorso storico. Pochi siti o siti
/// collineari producono zero triangoli. L'orientamento delle facce viene
/// verificato con il predicato esatto prima di costruire i poligoni chiusi.
///
/// Un solo `RelateLimits` governa confronti, scansioni, copie, capacita'
/// locali e output. I buffer trasferiti conservano la ricevuta fino alla
/// consegna, anche quando il loro consumatore li ha gia' liberati: il budget
/// e' conservativo. L'ingresso in prestito e' escluso; `max_nodes` conta i
/// siti distinti e le quattro coordinate di ogni triangolo pubblicato.
///
/// Spade 2.15.1 `bulk_load` e' una chiamata esterna indivisibile (opzione A):
/// le sue allocazioni private e il lavoro interno NON sono governati da
/// `RelateLimits`. Ci sono checkpoint immediatamente prima e dopo la chiamata,
/// non una garanzia di latenza o heap globale. Il vettore consegnato a Spade
/// e' invece prenotato. Nessuna stima byte/punto ammette l'allocazione esterna.
/// Il chiamante che necessita un tetto fisico deve usare l'isolamento.
///
/// # Errors
/// [`DelaunayBuildError`]: dominio di Spade, facce inattese, limiti,
/// allocazione fallita, deadline o annullamento. Nessuna uscita parziale.
pub fn delaunay_points_with_limits(
    coordinates: &[Coord<f64>],
    max_triangles: u64,
    limits: RelateLimits,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<Polygon<f64>>, DelaunayBuildError> {
    build(
        coordinates,
        max_triangles,
        &mut Work::new(limits, cancelled),
    )
}

fn build(
    coordinates: &[Coord<f64>],
    max_triangles: u64,
    work: &mut Work<'_>,
) -> Result<Vec<Polygon<f64>>, DelaunayBuildError> {
    let built = crate::triangolazione::bounded::build(coordinates, work)?;
    let mut faces = CountedVec::new();
    for face in built.triangolazione.inner_faces() {
        work.step()?;
        if u64::try_from(faces.len()).map_err(|_| DelaunayBuildError::TriangleLimit)?
            >= max_triangles
        {
            return Err(DelaunayBuildError::TriangleLimit);
        }
        let [a, b, c] = face.vertices().map(|v| v.data().rango);
        let canonical = if b < a && b < c {
            [b, c, a]
        } else if c < a && c < b {
            [c, a, b]
        } else {
            [a, b, c]
        };
        work.push(&mut faces, canonical)?;
    }
    sorted(&mut faces, |a, b| Ok(a.cmp(b)), work)?;
    let sites = built.siti;
    drop(built.triangolazione);
    let mut output = CountedVec::new();
    for &[a, b, c] in &faces {
        work.step()?;
        let [a, b, c] = [sites[a], sites[b], sites[c]];
        if <f64 as GeoNum>::Ker::orient2d(a, b, c) != Orientation::CounterClockwise {
            return Err(DelaunayBuildError::InvalidFace);
        }
        // Triangle::new della baseline puo' riordinare usando il cross
        // floating point: conservarlo anche quando differisce dal segno esatto.
        let [a, b, c] = geo::Triangle::new(a, b, c).to_array();
        let mut ring = CountedVec::new();
        for coordinate in [a, b, c, a] {
            work.nodes(1)?;
            work.push(&mut ring, coordinate)?;
        }
        let polygon = Polygon::new(LineString::new(ring.export(work)?), Vec::new());
        work.push(&mut output, polygon)?;
    }
    let output = output.export(work)?;
    work.step()?;
    Ok(output)
}

#[cfg(test)]
#[deny(clippy::disallowed_methods)]
mod tests;
