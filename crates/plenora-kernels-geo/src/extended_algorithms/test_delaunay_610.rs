//! Baseline 6.1.0 congelata per confronto integrale.
use super::*;
pub fn delaunay(
    geometry: &Geometry<f64>,
    max_input_coordinates: u64,
    max_triangles: u64,
) -> Result<Vec<Polygon<f64>>, ExtendedAlgorithmError> {
    validate_input(geometry)?;
    let coordinates = coordinate_count(geometry)?;
    if coordinates > max_input_coordinates {
        return Err(ExtendedAlgorithmError::CoordinateLimit {
            actual: coordinates,
            limit: max_input_coordinates,
        });
    }
    let punti: Vec<Coord<f64>> = geometry.coords_iter().collect();
    let triangoli = protetto(|| {
        crate::triangolazione::baseline_610::triangola(&punti)
            .map(|costruita| triangoli_canonici(&costruita))
    })?
    .map_err(errore_di_triangolazione)?;
    let actual =
        u64::try_from(triangoli.len()).map_err(|_| ExtendedAlgorithmError::IndexOverflow)?;
    if actual > max_triangles {
        return Err(ExtendedAlgorithmError::OutputLimit {
            actual,
            limit: max_triangles,
        });
    }
    triangoli
        .into_iter()
        .map(|triangle| {
            let polygon = triangle.to_polygon();
            validate_output(Geometry::Polygon(polygon.clone()))?;
            Ok(polygon)
        })
        .collect()
}

/// Le facce interne come triangoli nell'ordine canonico di [`delaunay`]:
/// ogni faccia antioraria (come `spade` la da') ruotata a partire dal
/// vertice di rango minimo, le facce ordinate per i ranghi dei tre vertici.
/// Due facce distinte non hanno la stessa terna, quindi l'ordine e' totale.
fn triangoli_canonici(
    costruita: &crate::triangolazione::baseline_610::Triangolazione,
) -> Vec<Triangle<f64>> {
    let mut facce: Vec<[crate::triangolazione::baseline_610::Sito; 3]> = costruita
        .triangolazione
        .inner_faces()
        .map(|faccia| faccia.vertices().map(|vertice| *vertice.data()))
        .collect();
    for faccia in &mut facce {
        let [a, b, c] = *faccia;
        *faccia = if b.rango < a.rango && b.rango < c.rango {
            [b, c, a]
        } else if c.rango < a.rango && c.rango < b.rango {
            [c, a, b]
        } else {
            [a, b, c]
        };
    }
    facce.sort_unstable_by_key(|faccia| faccia.map(|sito| sito.rango));
    facce
        .into_iter()
        .map(|[a, b, c]| Triangle::new(a.coordinata, b.coordinata, c.coordinata))
        .collect()
}
fn errore_di_triangolazione(
    errore: crate::triangolazione::baseline_610::ErroreTriangolazione,
) -> ExtendedAlgorithmError {
    use crate::triangolazione::baseline_610::ErroreTriangolazione;
    use geo::algorithm::triangulate_delaunay::TriangulationError;
    ExtendedAlgorithmError::Triangulation(match errore {
        ErroreTriangolazione::Inserimento(inserimento) => {
            TriangulationError::SpadeError(inserimento).to_string()
        }
        ErroreTriangolazione::VerticiInattesi => {
            "vertici della triangolazione diversi dai punti distinti".to_owned()
        }
    })
}
