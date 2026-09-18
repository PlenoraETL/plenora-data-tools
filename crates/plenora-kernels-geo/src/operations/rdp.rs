//! Precondizione numerica del percorso Douglas-Peucker di geo 0.33.1.
//!
//! La finitezza delle coordinate non implica quella delle distanze interne.
//! La traversata segue gli stessi segmenti e lo stesso spareggio (`>=`) di
//! geo, senza cambiare i vertici o la tolleranza. Ogni distanza viene
//! controllata, non soltanto il massimo: un NaN parziale sparirebbe dal fold.
//! Il costo e' una seconda traversata RDP, con stack esplicito O(n).
//! Ambito e condizione di rientro: docs/errori-e-limiti.md.

use super::OperationError;
use geo::{Coord, Distance, Euclidean, Geometry, Line, LineString};

const DISTANZA_NON_RAPPRESENTABILE: OperationError =
    OperationError::Internal("semplificazione: distanza non rappresentabile");

pub(super) fn accerta_distanze(
    geometry: &Geometry<f64>,
    tolerance: f64,
) -> Result<(), OperationError> {
    if tolerance <= 0.0 {
        return Ok(());
    }
    match geometry {
        Geometry::LineString(line) => accerta_linea(line, tolerance),
        Geometry::MultiLineString(lines) => lines
            .iter()
            .try_for_each(|line| accerta_linea(line, tolerance)),
        Geometry::Polygon(polygon) => std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .try_for_each(|line| accerta_linea(line, tolerance)),
        Geometry::MultiPolygon(polygons) => polygons.iter().try_for_each(|polygon| {
            std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .try_for_each(|line| accerta_linea(line, tolerance))
        }),
        // Le collezioni passano ricorsivamente da simplify_with_policy;
        // gli altri tipi non invocano RDP nel dispatch del chiamante.
        _ => Ok(()),
    }
}

fn accerta_linea(line: &LineString<f64>, tolerance: f64) -> Result<(), OperationError> {
    let mut pending: Vec<&[Coord<f64>]> = vec![&line.0];
    while let Some(coords) = pending.pop() {
        let [first, _, .., last] = coords else {
            continue;
        };
        let segment = Line::new(*first, *last);
        if first != last {
            let dx = last.x - first.x;
            let dy = last.y - first.y;
            // Deve coincidere con geo-types::private_utils::line_segment_distance:
            // mul_add cambierebbe gli arrotondamenti del denominatore verificato.
            #[expect(
                clippy::suboptimal_flops,
                reason = "stessi arrotondamenti separati di geo-types"
            )]
            let squared = dx * dx + dy * dy;
            if squared == 0.0 || !squared.is_finite() {
                return Err(DISTANZA_NON_RAPPRESENTABILE);
            }
        }
        let mut farthest = 0;
        let mut maximum = 0.0;
        for (index, coord) in coords.iter().enumerate().take(coords.len() - 1).skip(1) {
            let distance = Euclidean.distance(*coord, &segment);
            if !distance.is_finite() {
                return Err(DISTANZA_NON_RAPPRESENTABILE);
            }
            if distance >= maximum {
                farthest = index;
                maximum = distance;
            }
        }
        if maximum > tolerance {
            // Gli estremi restano inclusi nei due segmenti, come in geo.
            // Gli slice sono validi: farthest proviene dall'enumerazione.
            pending.push(&coords[farthest..]);
            pending.push(&coords[..=farthest]);
        }
    }
    Ok(())
}
