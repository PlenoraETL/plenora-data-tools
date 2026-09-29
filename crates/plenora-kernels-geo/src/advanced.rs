//! Advanced pure-Rust kernels whose output cardinality differs from the input.

use geo::algorithm::triangulate_delaunay::TriangulationError;
use geo::{
    BooleanOps, BoundingRect, Contains, Coord, Geometry, Intersects, LineString, Point, Polygon,
    Rect, Vector2DOps, VoronoiError,
};
use rstar::{RTree, RTreeObject, AABB};
use spade::handles::VoronoiVertex::{Inner, Outer};
use spade::Triangulation as _;
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
    /// Un calcolo di `geo` o `rstar` e' andato in panico dentro
    /// [`crate::calcolo_protetto`]: non accusa l'ingresso, porta la *forma*
    /// del payload, mai il contenuto.
    #[error("calcolo Voronoi non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
}

/// Un calcolo di `geo` o `rstar` dietro la barriera dei panici.
fn protetto<T>(calcolo: impl FnOnce() -> T) -> Result<T, AdvancedError> {
    crate::calcolo_protetto(calcolo).map_err(AdvancedError::CalcoloNonConcluso)
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
    voronoi_cells_con(geometries, max_points, costruisci_celle, associa_celle)
}

/// Firma dell'associazione punto -> cella: separata perche' l'oracolo dei
/// test possa far girare la stessa pipeline con la `find` lineare.
type Associazione = fn(&[Polygon<f64>], &[Point<f64>]) -> Result<Vec<Geometry<f64>>, AdvancedError>;

/// Firma della costruzione delle celle: separata perche' l'oracolo dei test
/// possa far girare la stessa pipeline con `voronoi_cells` di `geo`
/// (inserimento incrementale).
type Costruzione = fn(&[Point<f64>]) -> Result<Vec<Polygon<f64>>, AdvancedError>;

fn voronoi_cells_con(
    geometries: &[Geometry<f64>],
    max_points: usize,
    costruisci: Costruzione,
    associa: Associazione,
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

    let cells = costruisci(&points)?;
    for cell in &cells {
        cell.validazione_protetta().map_err(classifica_cella)?;
    }
    associa(&cells, &points)
}

/// Le celle di `voronoi_cells` di `geo` 0.33.1 (parametri predefiniti:
/// tolleranza zero, ritaglio `Padded`), sulla triangolazione caricata in
/// blocco di [`crate::triangolazione`] invece che sull'inserimento
/// incrementale.
///
/// Il corpo e' `build_raw_voronoi_cells` e `voronoi_cells_with_params` di
/// `geo` 0.33.1 ricopiati: stessi circocentri (`circumcenter` di `spade`
/// sulla faccia), stessi raggi, stesso ordinamento angolare, stesso
/// ritaglio con `intersection` di `geo`. Cambiano solo due cose, che
/// riportano l'uscita a quella dell'incrementale:
/// - le celle escono in ordine di rango (prima comparsa del sito), che e'
///   l'ordine dei vertici, e quindi delle celle, dell'incrementale;
/// - il rettangolo d'ingombro dei siti si accumula in ordine di rango sui
///   bit che l'incrementale lascia ai vertici.
///
/// Il circocentro di una faccia dipende dal vertice da cui `spade` la
/// parte: se l'incrementale la parte da un altro vertice, un vertice
/// Voronoi puo' differire di qualche `ulp`; sugli ingressi degeneri le
/// facce stesse sono diverse (README, «geo.delaunay e geo.voronoi»).
fn costruisci_celle(points: &[Point<f64>]) -> Result<Vec<Polygon<f64>>, AdvancedError> {
    let coordinate: Vec<Coord<f64>> = points.iter().map(|point| point.0).collect();
    protetto(|| celle_da_spade(&coordinate))?.map_err(AdvancedError::Voronoi)
}

// Il corpo ricopia `build_raw_voronoi_cells` di `geo` riga per riga: spezzarlo
// renderebbe piu' difficile il confronto con il sorgente.
#[allow(clippy::too_many_lines)]
fn celle_da_spade(coordinate: &[Coord<f64>]) -> Result<Vec<Polygon<f64>>, String> {
    let costruita =
        crate::triangolazione::triangola(coordinate).map_err(|errore| match errore {
            crate::triangolazione::ErroreTriangolazione::Inserimento(inserimento) => {
                VoronoiError::Triangulation(TriangulationError::SpadeError(inserimento)).to_string()
            }
            crate::triangolazione::ErroreTriangolazione::VerticiInattesi => {
                "vertici della triangolazione diversi dai punti distinti".to_owned()
            }
        })?;
    let triangolazione = &costruita.triangolazione;

    let num_vertices = costruita.siti.len();
    if num_vertices < 2 {
        return Err(VoronoiError::InsufficientVertices.to_string());
    }

    let base_bounds = compute_bounds_from_vertices(costruita.siti.iter().copied());

    // Use padded bounds for extension distance calculation
    let padded = padded_bounds(base_bounds, 0.5);
    let extension = (padded.width() + padded.height()) * 2.0;

    // Una cella per rango al piu': l'ordine di `voronoi_faces` e' quello
    // dei vertici caricati in blocco, non quello dell'incrementale.
    let mut per_rango: Vec<Option<Polygon<f64>>> = vec![None; num_vertices];

    for face in triangolazione.voronoi_faces() {
        let edges: Vec<_> = face.adjacent_edges().collect();
        if edges.is_empty() {
            continue;
        }

        let sito = *face.as_delaunay_vertex().data();
        let site_coord = sito.coordinata;

        // Collect circumcenters and ray info
        let mut circumcenters: Vec<Coord<f64>> = Vec::new();
        let mut rays: Vec<(Coord<f64>, Coord<f64>)> = Vec::new(); // (origin, direction)

        for edge in &edges {
            let from_vertex = edge.from();
            let to_vertex = edge.to();

            if let Inner(inner_face) = &from_vertex {
                let cc = inner_face.circumcenter();
                let coord = Coord { x: cc.x, y: cc.y };
                if !circumcenters.contains(&coord) {
                    circumcenters.push(coord);
                }
            }
            if let Inner(inner_face) = &to_vertex {
                let cc = inner_face.circumcenter();
                let coord = Coord { x: cc.x, y: cc.y };
                if !circumcenters.contains(&coord) {
                    circumcenters.push(coord);
                }
            }

            // Collect ray information
            if let (Inner(inner_face), Outer(outer_edge)) = (&from_vertex, &to_vertex) {
                let ref_pt = inner_face.circumcenter();
                let dir = outer_edge.direction_vector();
                rays.push((
                    Coord {
                        x: ref_pt.x,
                        y: ref_pt.y,
                    },
                    Coord { x: dir.x, y: dir.y },
                ));
            }

            if let (Outer(outer_edge), Inner(inner_face)) = (&from_vertex, &to_vertex) {
                let ref_pt = inner_face.circumcenter();
                let dir = outer_edge.direction_vector();
                rays.push((
                    Coord {
                        x: ref_pt.x,
                        y: ref_pt.y,
                    },
                    Coord { x: dir.x, y: dir.y },
                ));
            }
        }

        // Build cell vertices
        let mut vertices: Vec<Coord<f64>> = circumcenters.clone();

        if rays.is_empty() {
            // Interior cell: just circumcenters
            if vertices.len() < 3 {
                continue;
            }
        } else {
            // Boundary cell: extend rays far beyond bbox
            for (origin, direction) in &rays {
                // Normalise direction to unit vector so all rays extend the same distance.
                // Skip degenerate zero-length or non-finite directions.
                let Some(unit_dir) = direction.try_normalize() else {
                    continue;
                };

                // Add a point far beyond the bbox in the ray direction
                let extended = *origin + unit_dir * extension;
                vertices.push(extended);
            }
        }

        if vertices.len() < 3 {
            continue;
        }

        // Sort vertices by angle around the site
        vertices.sort_by(|a, b| {
            let angle_a = f64::atan2(a.y - site_coord.y, a.x - site_coord.x);
            let angle_b = f64::atan2(b.y - site_coord.y, b.x - site_coord.x);
            angle_a.total_cmp(&angle_b)
        });

        let Some(&primo) = vertices.first() else {
            continue;
        };
        vertices.push(primo);
        let poly = Polygon::new(LineString::new(vertices), vec![]);

        if let Some(posto) = per_rango.get_mut(sito.rango) {
            *posto = Some(poly);
        }
    }

    let raw_cells: Vec<Polygon<f64>> = per_rango.into_iter().flatten().collect();

    // Collinear input produces no cells (only perpendicular bisector lines).
    // Return an error rather than silently returning an empty result.
    if raw_cells.is_empty() {
        return Err(VoronoiError::CollinearInput.to_string());
    }

    // Ritaglio `VoronoiClip::Padded`, come `voronoi_cells_with_params`.
    let clip_poly: Polygon<f64> = padded_bounds(base_bounds, 0.5).to_polygon();
    let clip_rect = clip_poly.bounding_rect();

    Ok(raw_cells
        .into_iter()
        .flat_map(|cell| {
            // Skip intersection if cell is entirely within clip bounds
            let contained_by_clip = clip_rect
                .as_ref()
                .zip(cell.bounding_rect())
                .is_some_and(|(cr, cell_rect)| cr.contains(&cell_rect));

            if contained_by_clip {
                vec![cell]
            } else {
                cell.intersection(&clip_poly).0
            }
        })
        .collect())
}

/// `compute_bounds_from_vertices` di `geo` 0.33.1.
fn compute_bounds_from_vertices(vertices: impl Iterator<Item = Coord<f64>>) -> Rect<f64> {
    let (min_x, min_y, max_x, max_y) = vertices.fold(
        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
        |(min_x, min_y, max_x, max_y), p| {
            (
                f64::min(min_x, p.x),
                f64::min(min_y, p.y),
                f64::max(max_x, p.x),
                f64::max(max_y, p.y),
            )
        },
    );
    Rect::new((min_x, min_y), (max_x, max_y))
}

/// `padded_bounds` di `geo` 0.33.1.
fn padded_bounds(base: Rect<f64>, padding_factor: f64) -> Rect<f64> {
    let padding = f64::max(base.width(), base.height()) * padding_factor;
    Rect::new(
        (base.min().x - padding, base.min().y - padding),
        (base.max().x + padding, base.max().y + padding),
    )
}

/// Associa a ogni punto, in ordine d'ingresso, la prima cella (per indice
/// nell'uscita di `geo`) che lo interseca; `UnmatchedPoint` se nessuna.
fn associa_celle(
    cells: &[Polygon<f64>],
    points: &[Point<f64>],
) -> Result<Vec<Geometry<f64>>, AdvancedError> {
    // Pre-filtro per bounding rect: il rect (min/max esatti, bordo incluso)
    // copre ogni punto della cella, quindi scartare le celle il cui rect non
    // interseca il punto non cambia l'esito di `Intersects` ed evita il
    // predicato costoso sulle celle lontane. `None` = cella vuota, che non
    // interseca alcun punto.
    let cell_bounds: Vec<Option<Rect<f64>>> =
        cells.iter().map(BoundingRect::bounding_rect).collect();

    // Indice R-tree sugli stessi rect: sostituisce la scansione lineare di
    // tutte le celle per ogni punto (quadratica: 17,5 s a 100k punti).
    // L'esito resta quello della `find` lineare per costruzione:
    // - l'envelope di ogni cella e' il suo rect con gli stessi `f64`, senza
    //   restringimenti, e rstar confronta a intervalli chiusi (`<=`/`>=`)
    //   come `Rect::intersects(Point)`; i nodi interni sono min/max esatti
    //   dei figli. L'interrogazione restituisce quindi *esattamente* le celle
    //   il cui rect interseca il punto: un sovrainsieme di quelle che il
    //   predicato accetta;
    // - sui candidati si applica lo stesso predicato di prima
    //   ([`cella_accetta`]) in ordine crescente d'indice, e il primo che lo
    //   accetta e' l'indice minimo tra quelli accettati, cioe' quello che la
    //   `find` lineare restituisce, anche a parita' (punto su un lato o un
    //   vertice condiviso da piu' celle);
    // - le celle senza rect restano fuori dall'indice come erano fuori dal
    //   predicato; l'ordine di visita di rstar non conta, perche' i
    //   candidati si riordinano per indice prima del predicato.
    // L'oracolo e' `tests::associa_lineare`, la `find` di prima ricopiata
    // alla lettera.
    let involucri: Vec<CellaIndicizzata> = cell_bounds
        .iter()
        .enumerate()
        .filter_map(|(indice, bounds)| {
            bounds.map(|rect| CellaIndicizzata {
                indice,
                envelope: AABB::from_corners(
                    [rect.min().x, rect.min().y],
                    [rect.max().x, rect.max().y],
                ),
            })
        })
        .collect();
    let indice = protetto(|| RTree::bulk_load(involucri))?;

    let mut candidati: Vec<usize> = Vec::new();
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            candidati.clear();
            protetto(|| {
                candidati.extend(
                    indice
                        .locate_in_envelope_intersecting(&AABB::from_point([point.x(), point.y()]))
                        .map(|cella| cella.indice),
                );
            })?;
            candidati.sort_unstable();
            for &candidato in &candidati {
                let Some((cell, bounds)) = cells.get(candidato).zip(cell_bounds.get(candidato))
                else {
                    continue;
                };
                if protetto(|| cella_accetta(cell, bounds.as_ref(), point))? {
                    return Ok(Geometry::Polygon(cell.clone()));
                }
            }
            Err(AdvancedError::UnmatchedPoint(index))
        })
        .collect()
}

/// Envelope di una cella Voronoi con il suo indice nell'uscita di `geo`.
struct CellaIndicizzata {
    indice: usize,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for CellaIndicizzata {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// Il predicato di associazione punto -> cella: rect presente e che
/// interseca il punto, poi `Intersects` esatto sulla cella.
fn cella_accetta(cell: &Polygon<f64>, bounds: Option<&Rect<f64>>, point: &Point<f64>) -> bool {
    bounds.is_some_and(|bounds| bounds.intersects(point)) && cell.intersects(point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Area, HausdorffDistance, MultiPoint, Voronoi};

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

    // --- Oracolo dell'associazione punto -> cella con R-tree ---

    /// L'associazione di prima dell'R-tree, ricopiata alla lettera: scansione
    /// lineare di tutte le celle con pre-filtro per rect. E' il riferimento
    /// contro cui `associa_celle` deve dare la stessa uscita, errori inclusi.
    fn associa_lineare(
        cells: &[Polygon<f64>],
        points: &[Point<f64>],
    ) -> Result<Vec<Geometry<f64>>, AdvancedError> {
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

    /// Impronta esatta di un'uscita: `Debug` di `f64` e' la rappresentazione
    /// piu' corta che torna agli stessi bit (e distingue `-0.0`), quindi due
    /// impronte uguali sono due uscite identiche bit per bit, errore e suo
    /// indice compresi.
    fn impronta(uscita: &Result<Vec<Geometry<f64>>, AdvancedError>) -> String {
        format!("{uscita:?}")
    }

    /// Stessa pipeline completa, una volta con l'R-tree e una con la `find`
    /// lineare: le uscite devono coincidere.
    fn confronta_pipeline(punti: &[(f64, f64)]) -> Result<Vec<Geometry<f64>>, AdvancedError> {
        let geometrie: Vec<Geometry<f64>> = punti
            .iter()
            .map(|&(x, y)| Geometry::Point(Point::new(x, y)))
            .collect();
        let veloce = voronoi_cells(&geometrie, usize::MAX);
        let riferimento =
            voronoi_cells_con(&geometrie, usize::MAX, costruisci_celle, associa_lineare);
        assert_eq!(
            impronta(&veloce),
            impronta(&riferimento),
            "punti: {punti:?}"
        );
        veloce
    }

    /// Associazione diretta su celle e punti arbitrari (anche celle che si
    /// sovrappongono, vuote o degeneri), fuori dalla costruzione Voronoi.
    /// Oltre al lotto intero confronta ogni punto da solo: il primo punto
    /// senza cella trasforma il lotto in un errore e nasconderebbe le scelte
    /// fatte sugli altri.
    fn confronta_associazione(cells: &[Polygon<f64>], points: &[Point<f64>]) {
        assert_eq!(
            impronta(&associa_celle(cells, points)),
            impronta(&associa_lineare(cells, points)),
        );
        for punto in points.chunks(1) {
            assert_eq!(
                impronta(&associa_celle(cells, punto)),
                impronta(&associa_lineare(cells, punto)),
                "punto: {punto:?}"
            );
        }
    }

    /// Xorshift64 deterministico: stesso seme, stessa sequenza.
    struct Xorshift(u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn sotto(&mut self, limite: u64) -> u64 {
            self.next() % limite
        }

        fn unitario(&mut self) -> f64 {
            intero(self.next() >> 11) / intero(1_u64 << 53)
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn intero(valore: u64) -> f64 {
        valore as f64
    }

    // Niente mul_add: la fusione cambia l'arrotondamento e i punti di prova
    // devono restare quelli scritti (architettura.md#determinismo).
    #[allow(clippy::suboptimal_flops)]
    fn griglia(lato: u64, passo: f64, origine: (f64, f64)) -> Vec<(f64, f64)> {
        (0..lato)
            .flat_map(|i| {
                (0..lato)
                    .map(move |j| (origine.0 + intero(i) * passo, origine.1 + intero(j) * passo))
            })
            .collect()
    }

    fn punti(coppie: Vec<(f64, f64)>) -> Vec<Point<f64>> {
        coppie.into_iter().map(Point::from).collect()
    }

    /// Griglia regolare: siti cocircolari a quattro a quattro, vertici
    /// Voronoi condivisi da quattro celle, lati condivisi ovunque.
    #[test]
    fn oracolo_voronoi_griglia_con_parita() {
        for lato in [2, 3, 5, 8, 13] {
            confronta_pipeline(&griglia(lato, 1.0, (0.0, 0.0))).unwrap();
        }
    }

    /// Punti che cadono esattamente su lati e vertici condivisi delle celle
    /// di una griglia: ogni punto e' accettato da due o quattro celle e vince
    /// l'indice minimo, come nella `find` lineare.
    #[test]
    fn oracolo_associazione_punti_sui_lati_e_vertici_condivisi() {
        let siti = punti(griglia(6, 1.0, (0.0, 0.0)));
        let celle = MultiPoint::new(siti.clone()).voronoi_cells().unwrap();
        let mut interrogazioni = punti(griglia(13, 0.5, (-0.5, -0.5)));
        interrogazioni.extend(siti);
        // Fuori da ogni cella: l'errore e il suo indice devono coincidere.
        interrogazioni.push(Point::new(1.0e6, 1.0e6));
        confronta_associazione(&celle, &interrogazioni);
        confronta_associazione(&celle, &interrogazioni[..interrogazioni.len() - 1]);
        assert!(associa_celle(&celle, &interrogazioni[..interrogazioni.len() - 1]).is_ok());
    }

    /// Celle sintetiche che si sovrappongono, duplicate, degeneri e vuote:
    /// la parita' tra celle diverse e le celle senza rect (`None`) seguono la
    /// stessa regola della `find` lineare.
    #[test]
    fn oracolo_associazione_celle_sovrapposte_duplicate_e_vuote() {
        let quadrato = |x0: f64, y0: f64, lato: f64| {
            geo::Rect::new((x0, y0), (x0 + lato, y0 + lato)).to_polygon()
        };
        let vuota = Polygon::new(geo::LineString::new(Vec::new()), Vec::new());
        let degenere = Polygon::new(
            geo::LineString::from(vec![(0.0, 0.0), (2.0, 2.0), (4.0, 4.0), (0.0, 0.0)]),
            Vec::new(),
        );
        let celle = vec![
            vuota.clone(),
            quadrato(2.0, 2.0, 2.0),
            quadrato(0.0, 0.0, 4.0),
            quadrato(2.0, 2.0, 2.0),
            degenere,
            vuota,
            quadrato(-0.0, -0.0, 1.0),
            quadrato(3.0, 0.0, 1.0),
        ];
        let mut interrogazioni = punti(griglia(11, 0.5, (-0.5, -0.5)));
        interrogazioni.push(Point::new(-0.0, -0.0));
        interrogazioni.push(Point::new(1.0, 1.0));
        confronta_associazione(&celle, &interrogazioni);
        confronta_associazione(&celle, &interrogazioni[20..]);
        confronta_associazione(&celle[3..], &interrogazioni[..30]);
        confronta_associazione(&[], &interrogazioni[..3]);
    }

    /// Celle il cui rect ha larghezza o altezza nulla (segmenti verticali e
    /// orizzontali, un punto ripetuto, un segmento obliquo di lunghezza un
    /// `ulp`), accanto a celle ordinarie che le contengono o le toccano:
    /// l'envelope dell'R-tree e' un rect degenere con gli stessi `f64`, e
    /// l'intervallo chiuso di rstar deve accettare esattamente i punti che
    /// `Rect::intersects` accetta. Interrogazioni sui segmenti, sugli
    /// estremi, a un `ulp` di distanza e nei due versi di `-0.0`.
    #[test]
    fn oracolo_associazione_rect_di_larghezza_o_altezza_nulla() {
        let anello = |coordinate: &[(f64, f64)]| {
            Polygon::new(geo::LineString::from(coordinate.to_vec()), Vec::new())
        };
        let dopo = |x: f64| f64::from_bits(x.to_bits() + 1);
        let verticale = anello(&[(1.0, 0.0), (1.0, 2.0), (1.0, 4.0), (1.0, 0.0)]);
        let orizzontale = anello(&[(0.0, 3.0), (2.0, 3.0), (4.0, 3.0), (0.0, 3.0)]);
        let puntiforme = anello(&[(2.0, 2.0), (2.0, 2.0), (2.0, 2.0), (2.0, 2.0)]);
        let zero_negativo = anello(&[(-0.0, 0.0), (-0.0, 1.0), (0.0, 2.0), (-0.0, 0.0)]);
        let obliquo_minimo = anello(&[(3.0, 3.0), (dopo(3.0), dopo(3.0)), (3.0, 3.0)]);
        let quadrato = geo::Rect::new((0.0, 0.0), (4.0, 4.0)).to_polygon();
        let celle_degeneri = vec![
            verticale,
            orizzontale,
            puntiforme,
            zero_negativo,
            obliquo_minimo.clone(),
        ];
        for cella in &celle_degeneri {
            let rect = cella.bounding_rect().expect("rect");
            assert!(
                rect.width() == 0.0 || rect.height() == 0.0 || cella == &obliquo_minimo,
                "fixture: rect non degenere"
            );
        }
        let mut interrogazioni = punti(vec![
            (1.0, 0.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (1.0, 5.0),
            (dopo(1.0), 1.0),
            (0.999_999_999_999_999_9, 1.0),
            (0.0, 3.0),
            (3.0, 3.0),
            (4.0, 3.0),
            (2.0, dopo(3.0)),
            (2.0, 2.0),
            (dopo(2.0), 2.0),
            (2.0, dopo(2.0)),
            (-0.0, 0.5),
            (0.0, 0.5),
            (-0.0, -0.0),
            (dopo(3.0), dopo(3.0)),
            (dopo(3.0), 3.0),
            (5.0, 5.0),
        ]);
        interrogazioni.push(Point::new(f64::from_bits(1), 1.0));
        interrogazioni.push(Point::new(-f64::from_bits(1), 1.0));
        // Da sole, prima e dopo una cella ordinaria che le contiene: l'indice
        // minimo vince in entrambi gli ordini.
        confronta_associazione(&celle_degeneri, &interrogazioni);
        let mut prima = vec![quadrato.clone()];
        prima.extend(celle_degeneri.iter().cloned());
        confronta_associazione(&prima, &interrogazioni);
        let mut dopo_il_quadrato = celle_degeneri.clone();
        dopo_il_quadrato.push(quadrato);
        confronta_associazione(&dopo_il_quadrato, &interrogazioni);
        for cella in &celle_degeneri {
            confronta_associazione(std::slice::from_ref(cella), &interrogazioni);
        }
    }

    /// Rect con estremi subnormali: celle e punti con coordinate sotto
    /// `f64::MIN_POSITIVE`, compreso il minimo positivo `2^-1074`, lo zero
    /// con segno e rect di larghezza di un solo subnormale.
    #[test]
    fn oracolo_associazione_rect_con_estremi_subnormali() {
        let sub = |k: u64| f64::from_bits(k);
        let quadrato =
            |x0: f64, y0: f64, x1: f64, y1: f64| geo::Rect::new((x0, y0), (x1, y1)).to_polygon();
        let celle = vec![
            quadrato(0.0, 0.0, sub(1), sub(1)),
            quadrato(sub(1), 0.0, sub(3), sub(2)),
            quadrato(-sub(2), -sub(2), 0.0, 0.0),
            quadrato(-0.0, -0.0, sub(4), sub(4)),
            quadrato(sub(5), sub(5), f64::MIN_POSITIVE, f64::MIN_POSITIVE),
            quadrato(
                f64::MIN_POSITIVE / 4.0,
                0.0,
                f64::MIN_POSITIVE / 2.0,
                sub(7),
            ),
            // Larghezza nulla a un subnormale.
            Polygon::new(
                geo::LineString::from(vec![(sub(9), 0.0), (sub(9), sub(9)), (sub(9), 0.0)]),
                Vec::new(),
            ),
        ];
        let mut interrogazioni = Vec::new();
        for k in [0_u64, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
            for j in [0_u64, 1, 2, 5, 9] {
                interrogazioni.push(Point::new(sub(k), sub(j)));
                interrogazioni.push(Point::new(-sub(k), sub(j)));
                interrogazioni.push(Point::new(sub(k), -sub(j)));
            }
        }
        interrogazioni.extend(punti(vec![
            (-0.0, -0.0),
            (f64::MIN_POSITIVE, f64::MIN_POSITIVE),
            (f64::MIN_POSITIVE / 2.0, sub(3)),
            (f64::MIN_POSITIVE / 4.0, 0.0),
            (f64::MIN_POSITIVE / 3.0, sub(8)),
            (1.0, 1.0),
        ]));
        confronta_associazione(&celle, &interrogazioni);
        let mut rovesciate = celle;
        rovesciate.reverse();
        confronta_associazione(&rovesciate, &interrogazioni);
    }

    /// Pipeline intera con siti a coordinate subnormali: le celle di `geo`
    /// hanno allora rect con estremi subnormali (o la costruzione fallisce),
    /// e le due associazioni devono dare la stessa uscita, errori compresi.
    #[test]
    fn oracolo_voronoi_siti_subnormali() {
        let sub = |k: u64| f64::from_bits(k);
        for (passo, origine) in [
            (sub(1), (0.0, 0.0)),
            (sub(3), (-sub(4), sub(2))),
            (f64::MIN_POSITIVE / 8.0, (0.0, 0.0)),
            (f64::MIN_POSITIVE / 2.0, (-f64::MIN_POSITIVE, 0.0)),
        ] {
            let _ = confronta_pipeline(&griglia(3, passo, origine));
            let _ = confronta_pipeline(&griglia(4, passo, origine));
        }
        let _ = confronta_pipeline(&[(0.0, 0.0), (sub(1), 0.0), (0.0, sub(1)), (sub(1), sub(1))]);
        let _ = confronta_pipeline(&[(-0.0, 0.0), (sub(2), sub(1)), (sub(1), sub(3))]);
    }

    /// Duplicati, quasi-collineari, collineari (errore di costruzione) e
    /// scale estreme: coordinate minuscole, enormi e lontane dall'origine.
    #[test]
    fn oracolo_voronoi_casi_degeneri_e_scale_estreme() {
        confronta_pipeline(&[(0.0, 0.0), (0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]).unwrap();
        let _ = confronta_pipeline(&[(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (1.0, 1.0), (1.0, 1.0)]);
        let mut duplicati = griglia(4, 1.0, (0.0, 0.0));
        duplicati.extend(griglia(4, 1.0, (0.0, 0.0)));
        confronta_pipeline(&duplicati).unwrap();
        // Collineari: `geo` rifiuta la costruzione, stesso errore da entrambe.
        let collineari = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)];
        assert!(confronta_pipeline(&collineari).is_err());
        let _ = confronta_pipeline(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 1.0e-9)]);
        confronta_pipeline(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (1.0, 1.0), (1.0, -1.0)]).unwrap();
        for (passo, origine) in [
            (1.0e-300, (0.0, 0.0)),
            (1.0e-9, (1.0e9, -1.0e9)),
            (1.0e-3, (1.0e15, 1.0e15)),
            (1.0e150, (0.0, 0.0)),
            (1.0e300, (-1.0e300, -1.0e300)),
            (f64::MIN_POSITIVE, (0.0, 0.0)),
        ] {
            let _ = confronta_pipeline(&griglia(4, passo, origine));
        }
    }

    /// Confronti casuali ma deterministici: coordinate su un reticolo piccolo
    /// (molti duplicati e cocircolari) e coordinate continue, pipeline intera.
    #[test]
    fn oracolo_voronoi_casuale_su_reticolo_e_continuo() {
        let mut generatore = Xorshift(0x9E37_79B9_7F4A_7C15);
        for giro in 0..60_u64 {
            let quanti = 2 + generatore.sotto(250);
            let coppie: Vec<(f64, f64)> = (0..quanti)
                .map(|_| {
                    if giro % 2 == 0 {
                        let lato = 3 + generatore.sotto(20);
                        (
                            intero(generatore.sotto(lato)),
                            intero(generatore.sotto(lato)),
                        )
                    } else {
                        (generatore.unitario() * 100.0, generatore.unitario() * 100.0)
                    }
                })
                .collect();
            let _ = confronta_pipeline(&coppie);
        }
    }

    /// Confronti casuali sull'associazione nuda: celle Voronoi di siti su
    /// reticolo interrogate con i loro vertici (parita' fra tre o piu' celle)
    /// e con punti casuali, anche fuori da ogni cella.
    #[test]
    // Niente mul_add: la fusione cambia l'arrotondamento e i punti di prova
    // devono restare quelli scritti (architettura.md#determinismo).
    #[allow(clippy::suboptimal_flops)]
    fn oracolo_associazione_casuale_sui_vertici_delle_celle() {
        let mut generatore = Xorshift(0xD1B5_4A32_D192_ED03);
        for _ in 0..30 {
            let quanti = 3 + generatore.sotto(120);
            let siti: Vec<Point<f64>> = (0..quanti)
                .map(|_| Point::new(intero(generatore.sotto(12)), intero(generatore.sotto(12))))
                .collect();
            let Ok(celle) = MultiPoint::new(siti.clone()).voronoi_cells() else {
                continue;
            };
            let mut interrogazioni: Vec<Point<f64>> = celle
                .iter()
                .flat_map(|cella| cella.exterior().points().collect::<Vec<_>>())
                .collect();
            interrogazioni.extend(siti);
            interrogazioni.extend((0..50).map(|_| {
                Point::new(
                    generatore.unitario() * 40.0 - 14.0,
                    generatore.unitario() * 40.0 - 14.0,
                )
            }));
            confronta_associazione(&celle, &interrogazioni);
        }
    }

    // --- Oracolo della costruzione con il caricamento in blocco di `spade` ---

    /// La costruzione di prima, ricopiata alla lettera: `voronoi_cells` di
    /// `geo` 0.33.1, che inserisce i siti in `spade` uno alla volta.
    fn costruisci_con_geo(points: &[Point<f64>]) -> Result<Vec<Polygon<f64>>, AdvancedError> {
        let multipunto = MultiPoint::new(points.to_vec());
        let cells = protetto(|| multipunto.voronoi_cells())?
            .map_err(|error| AdvancedError::Voronoi(error.to_string()))?;
        Ok(cells)
    }

    /// Quanto l'uscita nuova si scosta da quella di prima.
    #[derive(Debug, Default)]
    struct Scarti {
        celle: usize,
        identiche: usize,
        hausdorff: f64,
    }

    fn perimetro(poligono: &Polygon<f64>) -> f64 {
        use geo::{Euclidean, Length};
        Euclidean.length(poligono.exterior())
    }

    /// Pipeline intera, costruzione nuova contro costruzione di `geo`, con la
    /// stessa associazione: stesso errore (stessa variante, stesso
    /// messaggio, stesso indice); altrimenti una cella per punto, nello
    /// stesso ordine, identica bit per bit oppure entro `tolleranza`
    /// (Hausdorff fra i vertici, e area entro `tolleranza` per il perimetro).
    fn confronta_costruzione(punti: &[(f64, f64)], tolleranza: f64) -> Scarti {
        let geometrie: Vec<Geometry<f64>> = punti
            .iter()
            .map(|&(x, y)| Geometry::Point(Point::new(x, y)))
            .collect();
        let nuova = voronoi_cells(&geometrie, usize::MAX);
        let vecchia = voronoi_cells_con(&geometrie, usize::MAX, costruisci_con_geo, associa_celle);
        let mut scarti = Scarti::default();
        match (nuova, vecchia) {
            (Err(nuova), Err(vecchia)) => {
                assert_eq!(
                    format!("{nuova:?}"),
                    format!("{vecchia:?}"),
                    "punti: {punti:?}"
                );
            }
            (Ok(nuova), Ok(vecchia)) => {
                assert_eq!(nuova.len(), vecchia.len(), "punti: {punti:?}");
                for (cella, riferimento) in nuova.iter().zip(&vecchia) {
                    scarti.celle += 1;
                    if format!("{cella:?}") == format!("{riferimento:?}") {
                        scarti.identiche += 1;
                        continue;
                    }
                    let (Geometry::Polygon(cella), Geometry::Polygon(riferimento)) =
                        (cella, riferimento)
                    else {
                        panic!("cella non poligonale");
                    };
                    let distanza = cella.hausdorff_distance(riferimento);
                    let area = (cella.unsigned_area() - riferimento.unsigned_area()).abs();
                    assert!(
                        distanza <= tolleranza && area <= tolleranza * perimetro(riferimento),
                        "scarto {distanza:e}, area {area:e}, oltre {tolleranza:e}: {punti:?}"
                    );
                    scarti.hausdorff = scarti.hausdorff.max(distanza);
                }
            }
            (nuova, vecchia) => panic!("esiti diversi: {nuova:?} contro {vecchia:?}"),
        }
        scarti
    }

    /// Punti interi su una circonferenza di raggio `raggio` (terne
    /// pitagoriche), piu' il centro: tutti cocircolari, esatti in `f64`.
    fn cocircolari(raggio: i64, centro: (f64, f64)) -> Vec<(f64, f64)> {
        let mut punti = vec![centro];
        for x in -raggio..=raggio {
            for y in -raggio..=raggio {
                if x * x + y * y == raggio * raggio {
                    #[allow(clippy::cast_precision_loss)]
                    punti.push((centro.0 + x as f64, centro.1 + y as f64));
                }
            }
        }
        punti
    }

    /// Siti casuali continui e in coordinate UTM al centimetro: nessuna
    /// quaterna cocircolare in pratica, le facce sono quelle
    /// dell'incrementale e ogni cella deve essere identica bit per bit o, se
    /// `spade` parte la faccia da un altro vertice, a qualche `ulp`.
    #[test]
    fn oracolo_costruzione_voronoi_casuale() {
        let mut generatore = Xorshift(0x5DEE_CE66_D1CE_4E5B);
        let mut totale = Scarti::default();
        for giro in 0..40_u64 {
            let quanti = 3 + generatore.sotto(400);
            let coppie: Vec<(f64, f64)> = (0..quanti)
                .map(|_| {
                    if giro % 2 == 0 {
                        (generatore.unitario() * 100.0, generatore.unitario() * 100.0)
                    } else {
                        (
                            500_000.0 + intero(generatore.sotto(100_000)) / 100.0,
                            4_500_000.0 + intero(generatore.sotto(100_000)) / 100.0,
                        )
                    }
                })
                .collect();
            let scarti = confronta_costruzione(&coppie, 1.0e-6);
            totale.celle += scarti.celle;
            totale.identiche += scarti.identiche;
            totale.hausdorff = totale.hausdorff.max(scarti.hausdorff);
        }
        eprintln!("voronoi casuale: {totale:?}");
        assert!(totale.celle > 5_000);
    }

    /// Griglie (quattro siti cocircolari per ogni vertice Voronoi), anche
    /// lontane dall'origine, e siti interi cocircolari: le due
    /// triangolazioni possono scegliere diagonali diverse, il diagramma e'
    /// lo stesso entro la precisione.
    #[test]
    fn oracolo_costruzione_voronoi_griglie_e_cocircolari() {
        for lato in [2_u64, 3, 4, 5, 8, 13, 21, 30] {
            confronta_costruzione(&griglia(lato, 1.0, (0.0, 0.0)), 1.0e-9);
            confronta_costruzione(&griglia(lato, 1.0, (500_000.0, 4_500_000.0)), 1.0e-6);
            confronta_costruzione(&griglia(lato, 0.25, (-3.0, 7.0)), 1.0e-9);
        }
        for raggio in [5, 25, 65, 325] {
            confronta_costruzione(&cocircolari(raggio, (0.0, 0.0)), 1.0e-6);
            confronta_costruzione(&cocircolari(raggio, (500_000.0, 4_500_000.0)), 1.0e-6);
            let mut senza_centro = cocircolari(raggio, (10.0, -10.0));
            senza_centro.remove(0);
            confronta_costruzione(&senza_centro, 1.0e-6);
        }
    }

    /// Duplicati (anche `-0.0` contro `0.0`, prima e dopo che i siti smettano
    /// di essere collineari), collineari, pochi punti: stessi errori e stesse
    /// celle, bit compresi.
    #[test]
    fn oracolo_costruzione_voronoi_duplicati_collineari_pochi_punti() {
        let esatti: &[&[(f64, f64)]] = &[
            &[(0.0, 0.0), (1.0, 0.0)],
            &[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (-0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 0.0), (-0.0, -0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (-0.0, 0.0), (1.0, 1.0)],
            &[
                (0.0, -0.0),
                (0.0, 0.0),
                (0.0, 3.0),
                (0.0, -0.0),
                (-2.0, 1.0),
            ],
            &[
                (0.0, 0.0),
                (1.0, 0.0),
                (-0.0, -0.0),
                (0.0, 1.0),
                (0.0, -0.0),
            ],
            &[
                (-0.0, 0.0),
                (2.0, 0.0),
                (0.0, 2.0),
                (0.0, -0.0),
                (-0.0, -0.0),
            ],
            &[(1.0, -0.0), (1.0, 0.0), (0.0, 1.0), (3.0, 3.0), (1.0, -0.0)],
            &[
                (0.0, 0.0),
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 0.0),
                (0.5, 1.0),
                (0.5, 1.0),
            ],
        ];
        for punti in esatti {
            let scarti = confronta_costruzione(punti, 0.0);
            assert_eq!(scarti.celle, scarti.identiche, "punti: {punti:?}");
        }
        let errori: &[&[(f64, f64)]] = &[
            &[(0.0, 0.0), (0.0, 0.0)],
            &[(1.0, 1.0), (1.0, 1.0), (1.0, 1.0)],
            &[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)],
            &[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (1.0, 0.0), (-0.0, 0.0)],
            &[(0.0, 5.0), (0.0, -5.0), (0.0, 0.0)],
        ];
        for punti in errori {
            let geometrie: Vec<Geometry<f64>> = punti
                .iter()
                .map(|&(x, y)| Geometry::Point(Point::new(x, y)))
                .collect();
            assert!(
                matches!(
                    voronoi_cells(&geometrie, usize::MAX),
                    Err(AdvancedError::Voronoi(_))
                ),
                "punti: {punti:?}"
            );
            confronta_costruzione(punti, 0.0);
        }
        let mut duplicati = griglia(6, 1.0, (0.0, 0.0));
        duplicati.extend(griglia(6, 1.0, (0.0, 0.0)));
        duplicati.reverse();
        confronta_costruzione(&duplicati, 1.0e-9);
        let _ = confronta_pipeline(&duplicati);
    }

    /// Coordinate ai bordi del dominio di `spade` (`[2^-142, 2^201]` in
    /// modulo, o zero) e fuori: dentro, stesse celle; fuori, lo stesso
    /// `InsertionError` di `geo`, per il primo punto fuori in ordine
    /// d'ingresso.
    #[test]
    fn oracolo_costruzione_voronoi_scale_estreme() {
        let minimo = 2.0_f64.powi(-142);
        let massimo = 2.0_f64.powi(201);
        let dentro = [
            (minimo, (0.0, 0.0), 1.0e-9 * minimo),
            (minimo * 8.0, (minimo, -minimo), 1.0e-9 * minimo),
            (2.0_f64.powi(190), (0.0, 0.0), 1.0e-9 * 2.0_f64.powi(190)),
            (2.0_f64.powi(196), (-massimo / 2.0, 0.0), 1.0e-9 * massimo),
            (1.0e-9, (1.0e9, -1.0e9), 1.0e-3),
            (1.0e-3, (1.0e15, 1.0e15), 1.0),
        ];
        for (passo, origine, tolleranza) in dentro {
            for lato in [2, 3, 4, 7] {
                confronta_costruzione(&griglia(lato, passo, origine), tolleranza);
            }
        }
        let fuori: &[&[(f64, f64)]] = &[
            &[(0.0, 0.0), (1.0e-300, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 0.0), (0.0, minimo / 2.0)],
            &[(0.0, 0.0), (massimo * 2.0, 0.0), (0.0, 1.0)],
            &[(0.0, 0.0), (1.0, 1.0e61), (1.0e-300, 1.0)],
            &[(1.0e-300, 1.0e61), (0.0, 0.0), (1.0, 1.0)],
            &[(f64::MIN_POSITIVE, 0.0), (1.0, 0.0), (0.0, 1.0)],
            &[(f64::from_bits(1), 0.0), (1.0, 0.0), (0.0, 1.0)],
        ];
        for punti in fuori {
            confronta_costruzione(punti, 0.0);
        }
        for (passo, origine) in [
            (1.0e-300, (0.0, 0.0)),
            (1.0e150, (0.0, 0.0)),
            (1.0e300, (-1.0e300, -1.0e300)),
            (f64::MIN_POSITIVE, (0.0, 0.0)),
        ] {
            confronta_costruzione(&griglia(4, passo, origine), 0.0);
        }
    }

    /// Siti su un reticolo piccolo: molti duplicati e molte quaterne
    /// cocircolari insieme.
    #[test]
    fn oracolo_costruzione_voronoi_reticolo_con_duplicati() {
        let mut generatore = Xorshift(0x2545_F491_4F6C_DD1D);
        for _ in 0..40 {
            let quanti = 2 + generatore.sotto(300);
            let lato = 2 + generatore.sotto(25);
            let coppie: Vec<(f64, f64)> = (0..quanti)
                .map(|_| {
                    (
                        intero(generatore.sotto(lato)),
                        intero(generatore.sotto(lato)),
                    )
                })
                .collect();
            confronta_costruzione(&coppie, 1.0e-9);
        }
    }

    /// Stesso ingresso, stessa uscita: due chiamate danno gli stessi bit,
    /// anche sugli ingressi degeneri dove la triangolazione non e' unica.
    #[test]
    fn voronoi_deterministico() {
        let mut griglia_duplicata = griglia(15, 1.0, (0.0, 0.0));
        griglia_duplicata.extend(griglia(9, 1.0, (0.5, 0.5)));
        for punti in [griglia_duplicata, cocircolari(65, (3.0, 4.0))] {
            let geometrie: Vec<Geometry<f64>> = punti
                .iter()
                .map(|&(x, y)| Geometry::Point(Point::new(x, y)))
                .collect();
            let prima = impronta(&voronoi_cells(&geometrie, usize::MAX));
            for _ in 0..3 {
                assert_eq!(impronta(&voronoi_cells(&geometrie, usize::MAX)), prima);
            }
        }
    }
}
