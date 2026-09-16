//! Advanced pure-Rust kernels whose output cardinality differs from the input.

use geo::{BoundingRect, Geometry, Intersects, MultiPoint, Point, Rect, Voronoi};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdvancedError {
    #[error("max_points deve essere almeno 2")]
    InvalidPointLimit,
    #[error("Voronoi richiede almeno due punti")]
    InsufficientPoints,
    #[error("Voronoi: {actual} punti oltre il limite di {limit}")]
    PointLimitExceeded { actual: usize, limit: usize },
    #[error("Voronoi accetta solo Point; riga {index}: {geometry_type}")]
    ExpectedPoint {
        index: usize,
        geometry_type: &'static str,
    },
    #[error("punto non valido alla riga {index}: {reason}")]
    InvalidPoint { index: usize, reason: String },
    #[error("costruzione Voronoi fallita: {0}")]
    Voronoi(String),
    #[error("nessuna cella Voronoi associabile alla riga {0}")]
    UnmatchedPoint(usize),
    #[error("cella Voronoi non valida: {0}")]
    InvalidOutput(String),
    /// La validazione OGC non ha concluso: `geo` si e' interrotta.
    ///
    /// **Non** e' una geometria invalida. Nessuno ha dimostrato che l'ingresso
    /// sia sbagliato, e accusarlo manderebbe chi legge a correggere un errore
    /// che non ha commesso. Porta la *forma* del payload, mai il contenuto.
    #[error("validazione OGC non conclusa: {0} (contenuto non pubblicato)")]
    ValidazioneNonConclusa(&'static str),
}

use crate::geometry_type_name as geometry_name;
use crate::ValidazioneProtetta as _;

/// Mappa l'esito della barriera sull'errore proprio di questo modulo, per un
/// punto d'ingresso alla riga `index`. Estratta a parte perche' e' la
/// conversione che una prova sintetica deve esercitare per intero: stessa
/// motivazione di `predicates::classifica_lato`.
fn classifica_punto(esito: crate::EsitoValidazione, index: usize) -> AdvancedError {
    esito.separa(
        |ragione| AdvancedError::InvalidPoint {
            index,
            reason: ragione.to_string(),
        },
        AdvancedError::ValidazioneNonConclusa,
    )
}

/// Come [`classifica_punto`], per una cella Voronoi in uscita: nessun indice
/// di riga da nominare, il contratto e' quello dell'output.
fn classifica_cella(esito: crate::EsitoValidazione) -> AdvancedError {
    esito.separa(
        |ragione| AdvancedError::InvalidOutput(ragione.to_string()),
        AdvancedError::ValidazioneNonConclusa,
    )
}

/// One bounded Voronoi polygon for every input point, retaining input order.
///
/// Duplicate points receive the same cell. Nulls are intentionally not
/// accepted because Manipola's current `MultiPoint` construction rejects them.
///
/// # Errors
///
/// - `InvalidPointLimit`: `max_points` is below 2.
/// - `InsufficientPoints`: fewer than 2 input geometries.
/// - `PointLimitExceeded`: more than `max_points` input geometries.
/// - `InvalidPoint`: an input geometry fails OGC validation (e.g. NaN
///   coordinates).
/// - `ExpectedPoint`: an input geometry is not a `Point`.
/// - `Voronoi`: the Voronoi construction itself failed.
/// - `InvalidOutput`: a produced cell fails OGC validation.
/// - `UnmatchedPoint`: no produced cell intersects an input point.
pub fn voronoi_cells(
    geometries: &[Geometry<f64>],
    max_points: usize,
) -> Result<Vec<Geometry<f64>>, AdvancedError> {
    if max_points < 2 {
        return Err(AdvancedError::InvalidPointLimit);
    }
    if geometries.len() < 2 {
        return Err(AdvancedError::InsufficientPoints);
    }
    if geometries.len() > max_points {
        return Err(AdvancedError::PointLimitExceeded {
            actual: geometries.len(),
            limit: max_points,
        });
    }

    let points: Vec<Point<f64>> = geometries
        .iter()
        .enumerate()
        .map(|(index, geometry)| {
            geometry
                .validazione_protetta()
                .map_err(|esito| classifica_punto(esito, index))?;
            match geometry {
                Geometry::Point(point) => Ok(*point),
                value => Err(AdvancedError::ExpectedPoint {
                    index,
                    geometry_type: geometry_name(value),
                }),
            }
        })
        .collect::<Result<_, _>>()?;

    let cells = MultiPoint::new(points.clone())
        .voronoi_cells()
        .map_err(|error| AdvancedError::Voronoi(error.to_string()))?;
    for cell in &cells {
        cell.validazione_protetta().map_err(classifica_cella)?;
    }

    // Pre-filtro per bounding rect: il bounding rect di una cella copre per
    // costruzione (min/max esatti delle coordinate, bordo incluso) ogni
    // punto della cella, quindi un punto che interseca la cella interseca
    // sempre anche il suo bounding rect. Scartare le celle il cui rect non
    // interseca il punto non puo' cambiare l'esito di `Intersects`, ma
    // evita il predicato geometrico costoso sulle celle lontane. Il rect
    // e' calcolato una sola volta per cella, fuori dal loop sui punti.
    // `bounding_rect` e' `Option` (None per cella vuota, che non puo'
    // intersecare alcun punto: esito coerente col predicato geometrico).
    let cell_bounds: Vec<Option<Rect<f64>>> =
        cells.iter().map(BoundingRect::bounding_rect).collect();

    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            cells
                .iter()
                .zip(&cell_bounds)
                .find(|(cell, bounds)| {
                    bounds
                        .as_ref()
                        .is_some_and(|bounds| bounds.intersects(point))
                        && cell.intersects(point)
                })
                .map(|(cell, _)| cell.clone())
                .map(Geometry::Polygon)
                .ok_or(AdvancedError::UnmatchedPoint(index))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Area, Contains};

    /// Sintetico attraverso la conversione reale (stessa motivazione di
    /// `predicates::classifica_lato_non_appiattisce_l_interruzione`): nessun
    /// reperto reale interrompe piu' `geo` col candidato esatto, quindi
    /// l'innesco e' un `EsitoValidazione::NonConclusa` costruito a mano, ma
    /// la funzione chiamata e' quella vera di `voronoi_cells` per un punto
    /// d'ingresso.
    #[test]
    fn classifica_punto_non_appiattisce_l_interruzione() {
        let esito = crate::EsitoValidazione::NonConclusa("forma di prova");
        let errore = classifica_punto(esito, 3);
        assert!(
            matches!(
                errore,
                AdvancedError::ValidazioneNonConclusa("forma di prova")
            ),
            "atteso ValidazioneNonConclusa, ottenuto: {errore:?}"
        );
        assert_eq!(
            errore.to_string(),
            "validazione OGC non conclusa: forma di prova (contenuto non pubblicato)"
        );
    }

    /// Controprova: un esito concluso produce l'altra variante, col suo
    /// indice — mai la stessa variante dell'interruzione.
    #[test]
    fn classifica_punto_su_esito_concluso_resta_invalidpoint_con_indice() {
        let esito = crate::EsitoValidazione::NonValida(crate::RagioneNonValida::AutoIntersezione);
        let errore = classifica_punto(esito, 3);
        assert!(
            matches!(errore, AdvancedError::InvalidPoint { index: 3, .. }),
            "atteso InvalidPoint con indice 3, ottenuto: {errore:?}"
        );
        assert_eq!(
            errore.to_string(),
            "punto non valido alla riga 3: anello con auto-intersezione"
        );
    }

    /// Stessa coppia per la cella d'uscita: nessun indice da nominare, ma la
    /// stessa distinzione a due vie.
    #[test]
    fn classifica_cella_non_appiattisce_l_interruzione() {
        let esito = crate::EsitoValidazione::NonConclusa("forma di prova");
        let errore = classifica_cella(esito);
        assert!(
            matches!(
                errore,
                AdvancedError::ValidazioneNonConclusa("forma di prova")
            ),
            "atteso ValidazioneNonConclusa, ottenuto: {errore:?}"
        );
    }

    #[test]
    fn classifica_cella_su_esito_concluso_resta_invalidoutput() {
        let esito = crate::EsitoValidazione::NonValida(crate::RagioneNonValida::AutoIntersezione);
        let errore = classifica_cella(esito);
        assert!(
            matches!(errore, AdvancedError::InvalidOutput(_)),
            "atteso InvalidOutput, ottenuto: {errore:?}"
        );
    }

    #[test]
    fn voronoi_preserves_input_order_and_contains_each_site() {
        let sites = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(2.0, 0.0)),
            Geometry::Point(Point::new(1.0, 2.0)),
        ];
        let cells = voronoi_cells(&sites, 10).unwrap();
        assert_eq!(cells.len(), sites.len());
        for (cell, site) in cells.iter().zip(&sites) {
            assert!(cell.contains(site) || cell.intersects(site));
            assert!(cell.unsigned_area() > 0.0);
        }
    }

    #[test]
    fn voronoi_rejects_invalid_shape_and_resource_limit() {
        let sites = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(2.0, 0.0)),
            Geometry::Point(Point::new(1.0, 2.0)),
        ];
        assert!(matches!(
            voronoi_cells(&sites, 2),
            Err(AdvancedError::PointLimitExceeded { .. })
        ));
        let non_points = vec![
            sites[0].clone(),
            geo::Rect::new((0.0, 0.0), (1.0, 1.0)).into(),
        ];
        assert!(matches!(
            voronoi_cells(&non_points, 10),
            Err(AdvancedError::ExpectedPoint { index: 1, .. })
        ));
        assert!(matches!(
            voronoi_cells(&[], 1),
            Err(AdvancedError::InvalidPointLimit)
        ));
        assert!(matches!(
            voronoi_cells(&[], 2),
            Err(AdvancedError::InsufficientPoints)
        ));
        let invalid = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(f64::NAN, 1.0)),
        ];
        assert!(matches!(
            voronoi_cells(&invalid, 2),
            Err(AdvancedError::InvalidPoint { .. })
        ));
        let variants = vec![
            Geometry::Line(geo::Line::new((0.0, 0.0), (1.0, 1.0))),
            Geometry::LineString(geo::LineString::from(vec![(0.0, 0.0), (1.0, 1.0)])),
            Geometry::Polygon(geo::Rect::new((0.0, 0.0), (1.0, 1.0)).to_polygon()),
            Geometry::MultiPoint(vec![Point::new(0.0, 0.0)].into()),
            Geometry::MultiLineString(geo::MultiLineString::new(Vec::new())),
            Geometry::MultiPolygon(geo::MultiPolygon::new(Vec::new())),
            Geometry::GeometryCollection(Vec::<Geometry<f64>>::new().into()),
            Geometry::Triangle(geo::Triangle::new(
                geo::Coord { x: 0.0, y: 0.0 },
                geo::Coord { x: 1.0, y: 0.0 },
                geo::Coord { x: 0.0, y: 1.0 },
            )),
        ];
        for variant in variants {
            assert!(matches!(
                voronoi_cells(&[sites[0].clone(), variant], 2),
                Err(AdvancedError::ExpectedPoint { .. })
            ));
        }
    }
}
