//! Advanced pure-Rust kernels whose output cardinality differs from the input.

use geo::{BoundingRect, Geometry, Intersects, MultiPoint, Point, Polygon, Rect, Voronoi};
use rstar::{RTree, RTreeObject, AABB};
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
    voronoi_cells_con(geometries, max_points, associa_celle)
}

/// Firma dell'associazione punto -> cella: separata perche' l'oracolo dei
/// test possa far girare la stessa pipeline con la `find` lineare.
type Associazione = fn(&[Polygon<f64>], &[Point<f64>]) -> Result<Vec<Geometry<f64>>, AdvancedError>;

fn voronoi_cells_con(
    geometries: &[Geometry<f64>],
    max_points: usize,
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

    let multipunto = MultiPoint::new(points.clone());
    let cells = protetto(|| multipunto.voronoi_cells())?
        .map_err(|error| AdvancedError::Voronoi(error.to_string()))?;
    for cell in &cells {
        cell.validazione_protetta().map_err(classifica_cella)?;
    }
    associa(&cells, &points)
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
        let riferimento = voronoi_cells_con(&geometrie, usize::MAX, associa_lineare);
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
}
