//! Kernel di `geo.voronoi`: una cella di Voronoi per ogni punto d'ingresso,
//! costruita sulla triangolazione caricata in blocco di
//! `crate::triangolazione::baseline_610`.

use geo::algorithm::triangulate_delaunay::TriangulationError;
use geo::{
    BooleanOps, BoundingRect, Contains, Coord, Geometry, Intersects, LineString, Point, Polygon,
    Rect, Vector2DOps, VoronoiError,
};
use rstar::{RTree, RTreeObject, AABB};
use spade::handles::VoronoiVertex::{Inner, Outer};
use spade::Triangulation as _;

use crate::rust_backend::griglia;
use crate::rust_backend::precision::{coordinate_abbastanza_fitte, modulo_massimo, Precision};
use thiserror::Error;

/// Errori di [`voronoi_cells`]. I messaggi nominano al piu' l'indice della
/// geometria d'ingresso, mai le coordinate.
#[derive(Debug, Error)]
pub enum AdvancedError {
    /// `max_points` minore di 2.
    #[error("max_points deve essere almeno 2")]
    InvalidPointLimit,
    /// Meno di due geometrie d'ingresso.
    #[error("Voronoi richiede almeno due punti")]
    InsufficientPoints,
    /// Piu' geometrie d'ingresso (`actual`) di `max_points` (`limit`).
    #[error("Voronoi: {actual} punti oltre il limite di {limit}")]
    PointLimitExceeded { actual: usize, limit: usize },
    /// La geometria all'indice `index` e' valida ma non e' un `Point`.
    // Il tipo della cella non entra nel messaggio («errori senza dati»).
    #[error("Voronoi accetta solo Point; riga {index}: geometria di un altro tipo")]
    ExpectedPoint {
        /// Indice della geometria nell'ingresso.
        index: usize,
        /// Tipo `geo` della geometria.
        geometry_type: &'static str,
    },
    /// La geometria all'indice `index` non supera la validazione OGC (per
    /// un punto: coordinate NaN o infinite); `reason` e' la ragione
    /// classificata, senza coordinate.
    #[error("punto non valido alla riga {index}: {reason}")]
    InvalidPoint { index: usize, reason: String },
    /// La costruzione non e' riuscita, con il messaggio di `geo` 0.33.1:
    /// coordinata fuori dal dominio di `spade` (zero o modulo in
    /// `[2^-142, 2^201]`), meno di due siti distinti, siti tutti collineari
    /// (nessuna cella), vertici persi o fusi dal caricamento in blocco.
    #[error("costruzione Voronoi fallita: {0}")]
    Voronoi(String),
    /// Nessuna cella interseca il punto all'indice dato.
    #[error("nessuna cella Voronoi associabile alla riga {0}")]
    UnmatchedPoint(usize),
    /// Una cella prodotta non supera la validazione OGC.
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
    /// `crate::calcolo_protetto`: non accusa l'ingresso, porta la *forma*
    /// del payload, mai il contenuto.
    #[error("calcolo Voronoi non concluso: {0} (contenuto non pubblicato)")]
    CalcoloNonConcluso(&'static str),
    /// Le coordinate (siti e punti lontani dei raggi, o vertici delle celle
    /// prima del ritaglio) sono troppo grandi per la precisione dichiarata:
    /// la spaziatura dei `f64` supera `p / 64` (`rust_backend::precision`);
    /// oppure la griglia di `i_overlay` del ritaglio di una cella di bordo
    /// supererebbe `p / 2`.
    #[error("geometria troppo estesa per la precisione dichiarata")]
    PrecisionInsufficient,
    /// Un vertice Voronoi (circocentro di un triangolo quasi degenere) ha un
    /// errore d'arrotondamento maggiorato oltre un quarto della precisione.
    #[error("vertice Voronoi troppo mal condizionato per la precisione dichiarata")]
    VerticeMalCondizionato,
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

/// Una cella di Voronoi limitata per ogni punto d'ingresso, nell'ordine
/// d'ingresso (l'ordine e' parte del contratto, `DefinedOrder` nel
/// catalogo).
///
/// Le celle sono quelle di `voronoi_cells` di `geo` 0.33.1 (tolleranza
/// zero, ritaglio `Padded`): le celle di bordo, infinite, si chiudono con
/// raggi prolungati e si ritagliano sul rettangolo d'ingombro dei siti
/// allargato per lato della meta' del suo lato maggiore. A ogni punto va la
/// prima cella, in ordine di sito, che lo interseca: i punti duplicati
/// ricevono la stessa cella. L'ingresso non ha null: una riga nulla va
/// trattata dal chiamante prima (Manipola, con cui l'operazione e'
/// compatibile, rifiuta i null nella costruzione del `MultiPoint`).
///
/// `precision` e' la precisione dichiarata nelle unita' delle coordinate
/// (docs/limiti.md, «Precisione delle operazioni geografiche: 1 cm a terra»): i
/// rifiuti per precisione sono descritti in docs/limiti.md, «geo.delaunay e
/// geo.voronoi».
///
/// # Errors
///
/// Nell'ordine in cui si controllano:
///
/// - `InvalidPointLimit`: `max_points` minore di 2;
/// - `InsufficientPoints`: meno di due geometrie;
/// - `PointLimitExceeded`: piu' di `max_points` geometrie;
/// - `InvalidPoint`: una geometria non supera la validazione OGC (per
///   esempio coordinate NaN); `ValidazioneNonConclusa` se la validazione
///   non conclude;
/// - `ExpectedPoint`: una geometria valida non e' un `Point`;
/// - `Voronoi`: la costruzione e' fallita (dominio di `spade`, siti
///   collineari o meno di due distinti);
/// - `PrecisionInsufficient`: siti, punti lontani dei raggi o vertici
///   delle celle dove la spaziatura dei `f64` supera `precision / 64`,
///   oppure griglia del ritaglio oltre `precision / 2`;
/// - `VerticeMalCondizionato`: un vertice Voronoi (circocentro di un
///   triangolo quasi degenere) con un errore d'arrotondamento maggiorato
///   oltre `precision / 4`;
/// - `CalcoloNonConcluso`: un calcolo di `geo`, `spade` o `rstar` e' andato
///   in panico;
/// - `InvalidOutput`: una cella prodotta non supera la validazione OGC;
/// - `UnmatchedPoint`: nessuna cella interseca un punto d'ingresso.
pub fn voronoi_cells(
    geometries: &[Geometry<f64>],
    max_points: usize,
    precision: Precision,
) -> Result<Vec<Geometry<f64>>, AdvancedError> {
    voronoi_cells_con(
        geometries,
        max_points,
        precision,
        costruisci_celle,
        associa_celle,
    )
}

/// Firma dell'associazione punto -> cella: separata perche' l'oracolo dei
/// test possa far girare la stessa pipeline con la `find` lineare.
type Associazione = fn(&[Polygon<f64>], &[Point<f64>]) -> Result<Vec<Geometry<f64>>, AdvancedError>;

/// Firma della costruzione delle celle: separata perche' l'oracolo dei test
/// possa far girare la stessa pipeline con `voronoi_cells` di `geo`
/// (inserimento incrementale).
type Costruzione = fn(&[Point<f64>], Precision) -> Result<Vec<Polygon<f64>>, AdvancedError>;

fn voronoi_cells_con(
    geometries: &[Geometry<f64>],
    max_points: usize,
    precision: Precision,
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

    let cells = costruisci(&points, precision)?;
    for cell in &cells {
        cell.validazione_protetta().map_err(classifica_cella)?;
    }
    associa(&cells, &points)
}

/// Le celle di `voronoi_cells` di `geo` 0.33.1 (parametri predefiniti:
/// tolleranza zero, ritaglio `Padded`), sulla triangolazione caricata in
/// blocco di [`crate::triangolazione::baseline_610`] invece che sull'inserimento
/// incrementale.
///
/// Il corpo e' `build_raw_voronoi_cells` e `voronoi_cells_with_params` di
/// `geo` 0.33.1 ricopiati: stessi raggi, stesso ordinamento angolare,
/// stesso ritaglio con `intersection` di `geo`. Due cambiamenti riportano
/// l'uscita a quella dell'incrementale:
/// - le celle escono in ordine di rango (prima comparsa del sito), che e'
///   l'ordine dei vertici, e quindi delle celle, dell'incrementale;
/// - il rettangolo d'ingombro dei siti si accumula in ordine di rango sui
///   bit che l'incrementale lascia ai vertici.
///
/// Una differenza voluta dal corpo di `geo`: il circocentro non e'
/// `circumcenter` di `spade`, che dipende dal vertice da cui `spade` parte la
/// faccia, ma [`circocentro_maggiorato`], con l'origine nel vertice di rango
/// minimo: stesso risultato qualunque sia la rotazione della faccia, e un
/// errore maggiorato che si confronta con la precisione. Rispetto
/// all'incrementale un vertice Voronoi puo' quindi differire di qualche
/// `ulp`; sugli ingressi degeneri le facce stesse sono diverse (docs/limiti.md,
/// «geo.delaunay e geo.voronoi»).
///
/// Tre controlli di precisione, tutti errori espliciti:
/// - spaziatura (`coordinate_abbastanza_fitte`) sul modulo massimo dei siti
///   piu' la distanza a cui si prolungano i raggi, e sul modulo massimo di
///   ogni vertice delle celle grezze;
/// - errore maggiorato di ogni circocentro entro `p / 4`;
/// - griglia del ritaglio delle celle di bordo entro `p / 2`, a priori
///   (`griglia::controlla_overlay`, come le booleane).
fn costruisci_celle(
    points: &[Point<f64>],
    precision: Precision,
) -> Result<Vec<Polygon<f64>>, AdvancedError> {
    let coordinate: Vec<Coord<f64>> = points.iter().map(|point| point.0).collect();
    protetto(|| celle_da_spade(&coordinate, precision))?
}

// Il corpo ricopia `build_raw_voronoi_cells` di `geo` riga per riga: spezzarlo
// renderebbe piu' difficile il confronto con il sorgente.
#[allow(clippy::too_many_lines)]
fn celle_da_spade(
    coordinate: &[Coord<f64>],
    precision: Precision,
) -> Result<Vec<Polygon<f64>>, AdvancedError> {
    let costruita = crate::triangolazione::baseline_610::triangola(coordinate).map_err(
        |errore| match errore {
            crate::triangolazione::baseline_610::ErroreTriangolazione::Inserimento(inserimento) => {
                AdvancedError::Voronoi(
                    VoronoiError::Triangulation(TriangulationError::SpadeError(inserimento))
                        .to_string(),
                )
            }
            crate::triangolazione::baseline_610::ErroreTriangolazione::VerticiInattesi => {
                AdvancedError::Voronoi(
                    "vertici della triangolazione diversi dai punti distinti".to_owned(),
                )
            }
        },
    )?;
    let triangolazione = &costruita.triangolazione;

    let num_vertices = costruita.siti.len();
    if num_vertices < 2 {
        return Err(AdvancedError::Voronoi(
            VoronoiError::InsufficientVertices.to_string(),
        ));
    }

    let base_bounds = compute_bounds_from_vertices(costruita.siti.iter().copied());

    // Il rettangolo allargato da' la distanza a cui si prolungano i raggi.
    let padded = padded_bounds(base_bounds, 0.5);
    let extension = (padded.width() + padded.height()) * 2.0;

    // Spaziatura: siti e punti lontani dei raggi (a `extension` da un
    // circocentro dentro o vicino al rettangolo) devono stare dove i `f64`
    // sono fitti per la precisione.
    let p = precision.value();
    if !coordinate_abbastanza_fitte(
        modulo_massimo(costruita.siti.iter().copied()) + extension,
        p,
    ) {
        return Err(AdvancedError::PrecisionInsufficient);
    }

    // Circocentri di tutte le facce, una volta, con l'errore maggiorato.
    let mut circocentri: Vec<Coord<f64>> =
        vec![Coord { x: 0.0, y: 0.0 }; triangolazione.num_all_faces()];
    for faccia in triangolazione.inner_faces() {
        let [a, b, c] = faccia.vertices().map(|vertice| *vertice.data());
        let (centro, errore) = circocentro_maggiorato(a, b, c);
        // `errore` puo' essere NaN o infinito: si rifiuta anche allora.
        if !matches!(
            errore.partial_cmp(&(p / 4.0)),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ) {
            return Err(AdvancedError::VerticeMalCondizionato);
        }
        if let Some(posto) = circocentri.get_mut(faccia.fix().index()) {
            *posto = centro;
        }
    }
    let centro_della_faccia = |indice: usize| {
        circocentri.get(indice).copied().unwrap_or(Coord {
            x: f64::NAN,
            y: f64::NAN,
        })
    };

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

        // Circocentri e raggi della cella.
        let mut circumcenters: Vec<Coord<f64>> = Vec::new();
        let mut rays: Vec<(Coord<f64>, Coord<f64>)> = Vec::new(); // (origine, direzione)

        for edge in &edges {
            let from_vertex = edge.from();
            let to_vertex = edge.to();

            if let Inner(inner_face) = &from_vertex {
                let coord = centro_della_faccia(inner_face.fix().index());
                if !circumcenters.contains(&coord) {
                    circumcenters.push(coord);
                }
            }
            if let Inner(inner_face) = &to_vertex {
                let coord = centro_della_faccia(inner_face.fix().index());
                if !circumcenters.contains(&coord) {
                    circumcenters.push(coord);
                }
            }

            // Un lato verso l'esterno e' un raggio dal circocentro.
            if let (Inner(inner_face), Outer(outer_edge)) = (&from_vertex, &to_vertex) {
                let ref_pt = centro_della_faccia(inner_face.fix().index());
                let dir = outer_edge.direction_vector();
                rays.push((ref_pt, Coord { x: dir.x, y: dir.y }));
            }

            if let (Outer(outer_edge), Inner(inner_face)) = (&from_vertex, &to_vertex) {
                let ref_pt = centro_della_faccia(inner_face.fix().index());
                let dir = outer_edge.direction_vector();
                rays.push((ref_pt, Coord { x: dir.x, y: dir.y }));
            }
        }

        // I vertici della cella.
        let mut vertices: Vec<Coord<f64>> = circumcenters.clone();

        if rays.is_empty() {
            // Cella interna: solo circocentri.
            if vertices.len() < 3 {
                continue;
            }
        } else {
            // Cella di bordo: i raggi si prolungano ben oltre il rettangolo.
            for (origin, direction) in &rays {
                // Direzione unitaria, cosi' ogni raggio si prolunga della
                // stessa distanza; si saltano le direzioni nulle o non
                // finite.
                let Some(unit_dir) = direction.try_normalize() else {
                    continue;
                };

                // Un punto lontano oltre il rettangolo, lungo il raggio.
                let extended = *origin + unit_dir * extension;
                vertices.push(extended);
            }
        }

        if vertices.len() < 3 {
            continue;
        }

        // Vertici in ordine d'angolo attorno al sito.
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

    // Siti collineari non danno celle (solo assi paralleli): errore, non
    // un risultato vuoto in silenzio.
    if raw_cells.is_empty() {
        return Err(AdvancedError::Voronoi(
            VoronoiError::CollinearInput.to_string(),
        ));
    }
    let vertici_grezzi = raw_cells
        .iter()
        .flat_map(|cella| cella.exterior().0.iter().copied());
    if !coordinate_abbastanza_fitte(modulo_massimo(vertici_grezzi), p) {
        return Err(AdvancedError::PrecisionInsufficient);
    }

    // Ritaglio `VoronoiClip::Padded`, come `voronoi_cells_with_params`.
    let clip_poly: Polygon<f64> = padded_bounds(base_bounds, 0.5).to_polygon();
    let clip_rect = clip_poly.bounding_rect();

    let mut celle = Vec::with_capacity(raw_cells.len());
    for cell in raw_cells {
        // Una cella tutta dentro il rettangolo di ritaglio non si interseca.
        let cell_rect = cell.bounding_rect();
        let contained_by_clip = clip_rect
            .as_ref()
            .zip(cell_rect)
            .is_some_and(|(cr, cell_rect)| cr.contains(&cell_rect));

        if contained_by_clip {
            celle.push(cell);
        } else {
            // Il ritaglio passa dalla griglia di `i_overlay`: lo stesso
            // controllo a priori delle booleane (`p / 2`, che con i vertici
            // entro `p / 4` resta nella precisione).
            griglia::controlla_overlay(griglia::unisci(clip_rect, cell_rect), precision)
                .map_err(|_| AdvancedError::PrecisionInsufficient)?;
            celle.extend(cell.intersection(&clip_poly).0);
        }
    }
    Ok(celle)
}

/// Il circocentro del triangolo `a, b, c` e un maggiorante del suo errore
/// assoluto (norma 1), indipendenti dalla rotazione della terna.
///
/// La terna si ruota a partire dal vertice di rango minimo (il verso resta
/// quello di `spade`), poi si calcola come `math::circumcenter` di `spade`
/// 2.15.1 con l'origine in quel vertice: `b = v1 - v0`, `c = v2 - v0`,
/// `d = 2 (bx cy - cx by)`, `x = (|b|^2 cy - |c|^2 by) / d`,
/// `y = (|c|^2 bx - |b|^2 cx) / d`, piu' `v0`.
///
/// Il maggiorante segue ogni operazione al primo ordine in `u = 2^-53`
/// (differenze: `u`; quadrati e somme: `4 u`; `d`: `8 u (|bx cy| + |cx by|)`;
/// numeratori: `7 u (|.| + |.|)`; quoziente: errore relativo del
/// denominatore `r`, infinito se `r >= 1/2`; somma finale: `u |x + v0|`) e
/// si raddoppia per coprire i termini di ordine superiore. Infinito (o NaN)
/// se il denominatore non e' separato dallo zero.
// I nomi corti sono quelli della formula di `spade` (`math::circumcenter`);
// niente `mul_add`: la fusione cambia l'arrotondamento su cui e' scritto il
// maggiorante.
#[allow(clippy::many_single_char_names, clippy::suboptimal_flops)]
fn circocentro_maggiorato(
    a: crate::triangolazione::baseline_610::Sito,
    b: crate::triangolazione::baseline_610::Sito,
    c: crate::triangolazione::baseline_610::Sito,
) -> (Coord<f64>, f64) {
    const U: f64 = f64::EPSILON / 2.0;
    let [v0, v1, v2] = if b.rango < a.rango && b.rango < c.rango {
        [b, c, a]
    } else if c.rango < a.rango && c.rango < b.rango {
        [c, a, b]
    } else {
        [a, b, c]
    }
    .map(|sito| sito.coordinata);
    let (bx, by) = (v1.x - v0.x, v1.y - v0.y);
    let (cx, cy) = (v2.x - v0.x, v2.y - v0.y);
    let d = 2.0 * (bx * cy - cx * by);
    let len_b = bx * bx + by * by;
    let len_c = cx * cx + cy * cy;
    let d_inv = 1.0 / d;
    let nx = len_b * cy - len_c * by;
    let ny = -len_b * cx + len_c * bx;
    let x = nx * d_inv;
    let y = ny * d_inv;
    let centro = Coord {
        x: x + v0.x,
        y: y + v0.y,
    };

    let errore_d = 16.0 * U * ((bx * cy).abs() + (cx * by).abs());
    let r = errore_d / d.abs();
    let errore = if r < 0.5 {
        let numeratore_x = 7.0 * U * ((len_b * cy).abs() + (len_c * by).abs());
        let numeratore_y = 7.0 * U * ((len_b * cx).abs() + (len_c * bx).abs());
        let ex = (numeratore_x / d.abs() + x.abs() * r) / (1.0 - r) + 2.0 * U * x.abs();
        let ey = (numeratore_y / d.abs() + y.abs() * r) / (1.0 - r) + 2.0 * U * y.abs();
        2.0 * (ex + ey + U * (centro.x.abs() + centro.y.abs()))
    } else {
        f64::INFINITY
    };
    (centro, errore)
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
