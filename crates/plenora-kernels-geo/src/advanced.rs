//! Kernel di `geo.voronoi`: una cella di Voronoi per ogni punto d'ingresso,
//! costruita sulla triangolazione caricata in blocco di
//! `crate::triangolazione`.

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
/// blocco di [`crate::triangolazione`] invece che sull'inserimento
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
    let costruita =
        crate::triangolazione::triangola(coordinate).map_err(|errore| match errore {
            crate::triangolazione::ErroreTriangolazione::Inserimento(inserimento) => {
                AdvancedError::Voronoi(
                    VoronoiError::Triangulation(TriangulationError::SpadeError(inserimento))
                        .to_string(),
                )
            }
            crate::triangolazione::ErroreTriangolazione::VerticiInattesi => AdvancedError::Voronoi(
                "vertici della triangolazione diversi dai punti distinti".to_owned(),
            ),
        })?;
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
    a: crate::triangolazione::Sito,
    b: crate::triangolazione::Sito,
    c: crate::triangolazione::Sito,
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

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Area, HausdorffDistance, MultiPoint, Voronoi};

    /// Precisione delle prove su coordinate unitarie: fitta abbastanza da
    /// non rifiutare nulla fino a coordinate di circa `2^30`.
    ///
    /// `10^-4` su estensioni fino a 100: un rapporto di `10^6`, come 10 km
    /// al centimetro.
    fn prova() -> Precision {
        Precision::new(1.0e-4).expect("precisione")
    }

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
        let cells = voronoi_cells(&sites, 10, prova()).unwrap();
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
            voronoi_cells(&sites, 2, prova()),
            Err(AdvancedError::PointLimitExceeded { .. })
        ));
        let non_points = vec![
            sites[0].clone(),
            geo::Rect::new((0.0, 0.0), (1.0, 1.0)).into(),
        ];
        assert!(matches!(
            voronoi_cells(&non_points, 10, prova()),
            Err(AdvancedError::ExpectedPoint { index: 1, .. })
        ));
        assert!(matches!(
            voronoi_cells(&[], 1, prova()),
            Err(AdvancedError::InvalidPointLimit)
        ));
        assert!(matches!(
            voronoi_cells(&[], 2, prova()),
            Err(AdvancedError::InsufficientPoints)
        ));
        let invalid = vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            Geometry::Point(Point::new(f64::NAN, 1.0)),
        ];
        assert!(matches!(
            voronoi_cells(&invalid, 2, prova()),
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
                voronoi_cells(&[sites[0].clone(), variant], 2, prova()),
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
        let veloce = voronoi_cells(&geometrie, usize::MAX, prova());
        let riferimento = voronoi_cells_con(
            &geometrie,
            usize::MAX,
            prova(),
            costruisci_celle,
            associa_lineare,
        );
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
    // devono restare quelli scritti.
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
    // devono restare quelli scritti.
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
    fn costruisci_con_geo(
        points: &[Point<f64>],
        _precision: Precision,
    ) -> Result<Vec<Polygon<f64>>, AdvancedError> {
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
        /// Ingressi su cui la costruzione nuova rifiuta per precisione e
        /// quella di `geo` risponde.
        rifiutati: usize,
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
    ///
    /// La precisione e' `tolleranza`, e non meno di `10^-4`; un rifiuto per
    /// precisione dove `geo` risponde fa fallire la prova.
    fn confronta_costruzione(punti: &[(f64, f64)], tolleranza: f64) -> Scarti {
        let scarti = confronta_o_rifiuta(punti, tolleranza);
        assert_eq!(scarti.rifiutati, 0, "rifiuto per precisione: {punti:?}");
        scarti
    }

    /// Come [`confronta_costruzione`], ma un rifiuto per precisione
    /// (`PrecisionInsufficient`, `VerticeMalCondizionato`) si conta.
    fn confronta_o_rifiuta(punti: &[(f64, f64)], tolleranza: f64) -> Scarti {
        let geometrie: Vec<Geometry<f64>> = punti
            .iter()
            .map(|&(x, y)| Geometry::Point(Point::new(x, y)))
            .collect();
        let precisione = Precision::new(tolleranza.max(1.0e-4)).expect("precisione");
        let nuova = voronoi_cells(&geometrie, usize::MAX, precisione);
        let vecchia = voronoi_cells_con(
            &geometrie,
            usize::MAX,
            precisione,
            costruisci_con_geo,
            associa_celle,
        );
        let mut scarti = Scarti::default();
        match (nuova, vecchia) {
            (
                Err(AdvancedError::PrecisionInsufficient | AdvancedError::VerticeMalCondizionato),
                Ok(_),
            ) => scarti.rifiutati += 1,
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
        // Non piu' bit per bit: il circocentro ha l'origine nel vertice di
        // rango minimo, non in quello da cui `spade` parte la faccia.
        for punti in esatti {
            confronta_costruzione(punti, 1.0e-12);
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
                    voronoi_cells(&geometrie, usize::MAX, prova()),
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
            (1.0e-3, (1.0e11, 1.0e11), 1.0),
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
            let prima = impronta(&voronoi_cells(&geometrie, usize::MAX, prova()));
            for _ in 0..3 {
                assert_eq!(
                    impronta(&voronoi_cells(&geometrie, usize::MAX, prova())),
                    prima
                );
            }
        }
    }

    /// Il reperto della revisione: siti a `(10^15, 10^15)` piu' pochi metri,
    /// distanti almeno 11 m. Con l'origine nel vertice da cui `spade` parte
    /// la faccia, incrementale e caricamento in blocco davano circocentri a
    /// 12,5 cm l'uno dall'altro. A `10^15` la spaziatura dei `f64` e'
    /// 0,125: la precisione di 1 cm non si puo' garantire e l'operazione si
    /// rifiuta; con i siti in coordinate UTM le celle ci sono.
    #[test]
    fn coordinate_troppo_grandi_per_la_precisione_si_rifiutano() {
        let siti = [(12.0, 19.125), (2.375, 2.625), (0.875, 19.125)];
        let centimetro = Precision::new(0.01).expect("precisione");
        let lontani: Vec<Geometry<f64>> = siti
            .iter()
            .map(|&(x, y)| Geometry::Point(Point::new(1.0e15 + x, 1.0e15 + y)))
            .collect();
        assert!(matches!(
            voronoi_cells(&lontani, 10, centimetro),
            Err(AdvancedError::PrecisionInsufficient)
        ));
        assert_eq!(
            AdvancedError::PrecisionInsufficient.to_string(),
            "geometria troppo estesa per la precisione dichiarata"
        );
        let vicini: Vec<Geometry<f64>> = siti
            .iter()
            .map(|&(x, y)| Geometry::Point(Point::new(500_000.0 + x, 4_500_000.0 + y)))
            .collect();
        assert_eq!(
            voronoi_cells(&vicini, 10, centimetro).expect("celle").len(),
            3
        );
        // Al limite: attorno a 2^38 m passa (spaziatura 2^-14 m), a 2^41 m no.
        for (origine, passa) in [(2.0_f64.powi(38), true), (2.0_f64.powi(41), false)] {
            let quadrato: Vec<Geometry<f64>> = griglia(3, 10.0, (origine, origine))
                .into_iter()
                .map(|(x, y)| Geometry::Point(Point::new(x, y)))
                .collect();
            let esito = voronoi_cells(&quadrato, 100, centimetro);
            assert_eq!(esito.is_ok(), passa, "{esito:?}");
        }
    }

    const fn sito(x: f64, y: f64, rango: usize) -> crate::triangolazione::Sito {
        crate::triangolazione::Sito {
            coordinata: Coord { x, y },
            rango,
        }
    }

    /// Il circocentro non dipende dalla rotazione della faccia (stessi bit),
    /// e il maggiorante copre lo scarto dal circocentro calcolato con le
    /// altre due origini, che hanno ciascuna il proprio maggiorante.
    #[test]
    // Niente mul_add: i punti di prova devono restare quelli scritti.
    #[allow(clippy::suboptimal_flops)]
    fn circocentro_indipendente_dalla_rotazione_e_maggiorato() {
        let mut generatore = Xorshift(0x0DDB_A11C_AFE0_F00D);
        let mut casi = 0;
        for giro in 0..20_000_u64 {
            let scala = [1.0, 1.0e3, 1.0e6][usize::try_from(giro % 3).unwrap_or(0)];
            let base = (500_000.0, 4_500_000.0);
            let a = (
                base.0 + generatore.unitario() * scala,
                base.1 + generatore.unitario() * scala,
            );
            let b = (
                base.0 + generatore.unitario() * scala,
                base.1 + generatore.unitario() * scala,
            );
            // Il terzo vicino alla retta per a e b: triangoli sottili.
            let t = generatore.unitario() * 3.0 - 1.0;
            let scosta = (generatore.unitario() - 0.5) * scala * 1.0e-6;
            let c = (
                a.0 + t * (b.0 - a.0) - scosta,
                a.1 + t * (b.1 - a.1) + scosta,
            );
            let orientazione = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            if orientazione.abs() < 1.0e-9 * scala * scala {
                continue;
            }
            let (a, b) = if orientazione > 0.0 { (a, b) } else { (b, a) };
            let [sa, sb, sc] = [sito(a.0, a.1, 0), sito(b.0, b.1, 1), sito(c.0, c.1, 2)];
            let (centro, errore) = circocentro_maggiorato(sa, sb, sc);
            for altra in [
                circocentro_maggiorato(sb, sc, sa),
                circocentro_maggiorato(sc, sa, sb),
            ] {
                assert_eq!(
                    (altra.0.x.to_bits(), altra.0.y.to_bits(), altra.1.to_bits()),
                    (centro.x.to_bits(), centro.y.to_bits(), errore.to_bits()),
                );
            }
            if !errore.is_finite() {
                continue;
            }
            // Le altre due origini: ranghi riassegnati, la formula parte da
            // b e poi da c.
            for [v0, v1, v2] in [[sb, sc, sa], [sc, sa, sb]] {
                let (altro, errore_altro) = circocentro_maggiorato(
                    sito(v0.coordinata.x, v0.coordinata.y, 0),
                    sito(v1.coordinata.x, v1.coordinata.y, 1),
                    sito(v2.coordinata.x, v2.coordinata.y, 2),
                );
                let scarto = (altro.x - centro.x).abs() + (altro.y - centro.y).abs();
                assert!(
                    scarto <= errore + errore_altro,
                    "scarto {scarto:e} oltre {:e}",
                    errore + errore_altro
                );
            }
            casi += 1;
        }
        assert!(casi > 15_000);
    }

    /// Il dominio realistico: siti UTM al centimetro (o continui) su 1, 10 e
    /// 30 km con 1 cm di precisione non si rifiutano (maggiorante misurato
    /// al piu' `3,4e-4 p`, contro il limite `p / 4`).
    #[test]
    fn dominio_utm_realistico_non_si_rifiuta() {
        let centimetro = Precision::new(0.01).expect("precisione");
        let mut generatore = Xorshift(0x1234_5678_9ABC_DEF1);
        for (quanti, lato_cm) in [
            (2_000_u64, 100_000_u64),
            (5_000, 1_000_000),
            (5_000, 3_000_000),
        ] {
            for continui in [false, true] {
                let siti: Vec<Geometry<f64>> = (0..quanti)
                    .map(|_| {
                        let (x, y) = if continui {
                            (
                                generatore.unitario() * intero(lato_cm) / 100.0,
                                generatore.unitario() * intero(lato_cm) / 100.0,
                            )
                        } else {
                            (
                                intero(generatore.sotto(lato_cm)) / 100.0,
                                intero(generatore.sotto(lato_cm)) / 100.0,
                            )
                        };
                        Geometry::Point(Point::new(500_000.0 + x, 4_500_000.0 + y))
                    })
                    .collect();
                assert_eq!(
                    voronoi_cells(&siti, usize::MAX, centimetro)
                        .expect("celle")
                        .len(),
                    siti.len()
                );
            }
        }
    }
}
