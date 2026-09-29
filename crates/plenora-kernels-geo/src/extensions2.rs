//! Kernel di `geo.generate_grid`, `geo.subdivide` e `geo.snap`.
//!
//! Kernel puri su `geo::Geometry<f64>` che rendono [`ExtensionError`], piu'
//! gli adapter di colonna/righe (`snap_column`, `generate_grid_rows`,
//! `subdivide_wkb`) che lo mappano su [`PlenoraError`] preservando i messaggi.
//!
//! - `generate_grid` produce una cella poligonale per riga. Celle `square`:
//!   tiling esatto dell'extent, celle di bordo tagliate sull'extent, indici
//!   `cell_i`/`cell_j` da 0 in `(xmin, ymin)`. Celle `hex`: esagoni flat-top
//!   a lato `cell_size`, emessi solo se interamente contenuti nell'extent.
//!   Il limite [`MAX_GRID_CELLS`] e' verificato prima dell'allocazione.
//! - `subdivide` taglia ricorsivamente i poligoni (somma delle aree
//!   preservata entro la precisione, profondita' limitata da
//!   [`MAX_SUBDIVIDE_DEPTH`]) e spezza a blocchi linee e `MultiPoint`; sotto
//!   soglia la geometria passa invariata.
//! - `snap` e' nativo (R-tree, niente GEOS): a parita' di distanza la scelta
//!   e' deterministica ma non specificata; l'output e' validato.

use geo::{
    Area, BooleanOps, BoundingRect, Coord, CoordsIter, Geometry, LineString, MapCoords, MultiPoint,
    MultiPolygon, Polygon, Rect,
};
use plenora_core::arrow::array::BinaryArray;
use plenora_core::PlenoraError;
use rstar::RTree;
use serde::{Deserialize, Serialize};

use crate::arrow_adapter::{decode_geometry_cell, encode_geometry, map_nullable};
use crate::extensions::{
    check_tolerance, ensure_valid, invalid_parameter, protetto, u64_len, validate_output,
    ExtensionError,
};
use crate::rust_backend::griglia;
use crate::rust_backend::precision::Precision;
use crate::ValidazioneProtetta as _;

/// Numero minimo di vertici ammesso per `max_vertices` (un anello chiuso ne
/// richiede almeno 4).
pub const MIN_SUBDIVIDE_VERTICES: usize = 4;
/// Limite di ricorsione del taglio poligonale di `subdivide` (fail-closed).
pub const MAX_SUBDIVIDE_DEPTH: u32 = 32;
/// Limite di celle prodotte da `generate_grid` (verificato in analisi e nel
/// kernel, prima dell'allocazione).
pub const MAX_GRID_CELLS: u64 = 1_000_000;

// ---------------------------------------------------------------------------
// geo.generate_grid
// ---------------------------------------------------------------------------

/// Forma delle celle della griglia (default `square`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GridShape {
    /// `square`: quadrati di lato `cell_size` che coprono l'extent, tagliati
    /// sul suo bordo.
    Square,
    /// `hex`: esagoni con un lato in alto, di lato `cell_size`, solo se
    /// interamente contenuti nell'extent.
    Hex,
}

/// Extent della griglia: finito, non degenere (`xmax > xmin`, `ymax > ymin`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridExtent {
    /// Ascissa minima.
    pub xmin: f64,
    /// Ordinata minima.
    pub ymin: f64,
    /// Ascissa massima.
    pub xmax: f64,
    /// Ordinata massima.
    pub ymax: f64,
}

impl GridExtent {
    /// Crea un extent validato (finito e non degenere).
    ///
    /// # Errors
    ///
    /// `ExtensionError::InvalidParameter` se una coordinata non e'
    /// finita, oppure se `xmax <= xmin` o `ymax <= ymin` (extent
    /// degenere).
    pub fn new(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Result<Self, ExtensionError> {
        for (name, value) in [
            ("extent.xmin", xmin),
            ("extent.ymin", ymin),
            ("extent.xmax", xmax),
            ("extent.ymax", ymax),
        ] {
            if !value.is_finite() {
                return Err(invalid_parameter(name, "deve essere finito"));
            }
        }
        if xmax <= xmin {
            return Err(invalid_parameter(
                "extent",
                "xmax deve essere maggiore di xmin",
            ));
        }
        if ymax <= ymin {
            return Err(invalid_parameter(
                "extent",
                "ymax deve essere maggiore di ymin",
            ));
        }
        Ok(Self {
            xmin,
            ymin,
            xmax,
            ymax,
        })
    }

    fn width(self) -> f64 {
        self.xmax - self.xmin
    }

    fn height(self) -> f64 {
        self.ymax - self.ymin
    }
}

/// Una riga della griglia: poligono della cella, indici e centro (il centro
/// e' il centroide geometrico della cella; per le celle esagonali il centro
/// dell'esagono).
#[derive(Clone, Debug, PartialEq)]
pub struct GridCell {
    /// Indice di colonna, da 0 a partire da `xmin`.
    pub cell_i: u64,
    /// Indice di riga, da 0 a partire da `ymin`.
    pub cell_j: u64,
    /// Il poligono della cella.
    pub geometry: Geometry<f64>,
    /// Ascissa del centro.
    pub centroid_x: f64,
    /// Ordinata del centro.
    pub centroid_y: f64,
}

fn check_cell_size(cell_size: f64) -> Result<(), ExtensionError> {
    if !cell_size.is_finite() || cell_size <= 0.0 {
        return Err(invalid_parameter(
            "cell_size",
            "deve essere finito e maggiore di zero",
        ));
    }
    Ok(())
}

fn checked_axis_cells(span: f64, cell_size: f64) -> Result<u64, ExtensionError> {
    let cells = (span / cell_size).ceil();
    // Soglia 2^64: esatta in f64 e uguale a `u64::MAX as f64`, che
    // arrotonda per eccesso.
    if !cells.is_finite() || cells > 18_446_744_073_709_551_616.0 {
        return Err(ExtensionError::IndexOverflow);
    }
    // Guardia sopra: cells finito e <= 2^64; uno span negativo satura a 0
    // (extent degenere -> griglia vuota).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cells_u64 = cells as u64;
    Ok(cells_u64)
}

/// Dimensioni della griglia quadrata `(colonne, righe)`, con il limite di
/// celle applicato al prodotto.
fn square_dimensions(extent: &GridExtent, cell_size: f64) -> Result<(u64, u64), ExtensionError> {
    let columns = checked_axis_cells(extent.width(), cell_size)?;
    let rows = checked_axis_cells(extent.height(), cell_size)?;
    let total = columns.checked_mul(rows).ok_or(ExtensionError::CellLimit {
        actual: u64::MAX,
        limit: MAX_GRID_CELLS,
    })?;
    if total > MAX_GRID_CELLS {
        return Err(ExtensionError::CellLimit {
            actual: total,
            limit: MAX_GRID_CELLS,
        });
    }
    Ok((columns, rows))
}

/// Centri `(cell_i, cell_j, cx, cy)` degli esagoni interamente contenuti
/// nell'extent. Le colonne dispari sono sfalsate di meta' passo verticale;
/// il limite di celle e' applicato sia alle colonne sia al totale.
fn hex_centers(
    extent: &GridExtent,
    cell_size: f64,
) -> Result<Vec<(u64, u64, f64, f64)>, ExtensionError> {
    let vertical_step = cell_size * 3.0_f64.sqrt();
    let column_step = 1.5 * cell_size;
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE, e i
    // bit della forma non fusa sono il contratto numerico.
    #[allow(clippy::suboptimal_flops)]
    let columns_f = ((extent.width() - 2.0 * cell_size) / column_step).floor();
    let columns = if extent.width() >= 2.0 * cell_size
        && columns_f.is_finite()
        // Soglia 2^64, esatta in f64 (uguale a `u64::MAX as f64`).
        && columns_f < 18_446_744_073_709_551_616.0
    {
        // Guardia nella condizione: columns_f finito, >= 0 (questo ramo
        // richiede width >= 2*cell_size) e < 2^64; cast saturante esatto.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let columns_u64 = columns_f as u64;
        columns_u64 + 1
    } else if extent.width() < 2.0 * cell_size {
        0
    } else {
        return Err(ExtensionError::CellLimit {
            actual: u64::MAX,
            limit: MAX_GRID_CELLS,
        });
    };
    if columns > MAX_GRID_CELLS {
        return Err(ExtensionError::CellLimit {
            actual: columns,
            limit: MAX_GRID_CELLS,
        });
    }
    let mut centers = Vec::new();
    for cell_i in 0..columns {
        // cell_i < columns <= MAX_GRID_CELLS (verificato sopra): ben sotto
        // 2^52, la conversione in f64 e' esatta. Forma non fusa come sopra.
        #[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
        let cx = extent.xmin + cell_size + cell_i as f64 * column_step;
        let first_cy = if cell_i % 2 == 0 {
            extent.ymin + vertical_step / 2.0
        } else {
            extent.ymin + vertical_step
        };
        let mut cell_j = 0_u64;
        let mut cy = first_cy;
        // Accumulo voluto: `first_cy + cell_j * vertical_step` cambierebbe
        // l'arrotondamento delle coordinate di griglia (contratto numerico).
        // Il loop e' limitato dal contatore `cell_j` e da `MAX_GRID_CELLS`,
        // mai dalla sola aritmetica float.
        #[allow(clippy::while_float)]
        while cy + vertical_step / 2.0 <= extent.ymax {
            centers.push((cell_i, cell_j, cx, cy));
            if u64_len(centers.len())? > MAX_GRID_CELLS {
                return Err(ExtensionError::CellLimit {
                    actual: u64_len(centers.len())?,
                    limit: MAX_GRID_CELLS,
                });
            }
            cell_j += 1;
            cy += vertical_step;
        }
    }
    Ok(centers)
}

/// Numero di celle che [`generate_grid`] produrrebbe con gli stessi
/// parametri (usato dall'analisi per il contratto e il limite).
///
/// # Errors
///
/// - `ExtensionError::InvalidParameter`: `cell_size` non finito o
///   minore o uguale a zero.
/// - `ExtensionError::CellLimit`: le celle (totale per le griglie
///   quadrate, colonne o totale per quelle esagonali) superano
///   [`MAX_GRID_CELLS`].
/// - `ExtensionError::IndexOverflow`: il numero di celle non e'
///   rappresentabile come `u64`.
pub fn grid_cell_count(
    extent: &GridExtent,
    cell_size: f64,
    shape: GridShape,
) -> Result<u64, ExtensionError> {
    check_cell_size(cell_size)?;
    match shape {
        GridShape::Square => {
            let (columns, rows) = square_dimensions(extent, cell_size)?;
            Ok(columns * rows)
        }
        GridShape::Hex => u64_len(hex_centers(extent, cell_size)?.len()),
    }
}

fn square_cell(extent: &GridExtent, cell_size: f64, cell_i: u64, cell_j: u64) -> GridCell {
    // cell_i e cell_j sono minori delle dimensioni della griglia, il cui
    // prodotto e' <= MAX_GRID_CELLS (verificato in square_dimensions):
    // ben sotto 2^52, le conversioni in f64 sono esatte.
    // Niente mul_add/FMA: la fusione cambia l'arrotondamento IEEE, e i
    // bit della forma non fusa sono il contratto numerico.
    #[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
    let x0 = extent.xmin + cell_i as f64 * cell_size;
    let x1 = (x0 + cell_size).min(extent.xmax);
    // Stessa forma non fusa, per lo stesso motivo.
    #[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
    let y0 = extent.ymin + cell_j as f64 * cell_size;
    let y1 = (y0 + cell_size).min(extent.ymax);
    let ring = LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]);
    GridCell {
        cell_i,
        cell_j,
        geometry: Geometry::Polygon(Polygon::new(ring, Vec::new())),
        centroid_x: f64::midpoint(x0, x1),
        centroid_y: f64::midpoint(y0, y1),
    }
}

fn hex_cell(cell_i: u64, cell_j: u64, cx: f64, cy: f64, cell_size: f64) -> GridCell {
    let half_height = cell_size * 3.0_f64.sqrt() / 2.0;
    let half_side = cell_size / 2.0;
    // Flat-top, vertice a est, anello antiorario.
    let ring = LineString::from(vec![
        (cx + cell_size, cy),
        (cx + half_side, cy + half_height),
        (cx - half_side, cy + half_height),
        (cx - cell_size, cy),
        (cx - half_side, cy - half_height),
        (cx + half_side, cy - half_height),
        (cx + cell_size, cy),
    ]);
    GridCell {
        cell_i,
        cell_j,
        geometry: Geometry::Polygon(Polygon::new(ring, Vec::new())),
        centroid_x: cx,
        centroid_y: cy,
    }
}

/// Genera la griglia di celle sull'extent (una riga per cella, ordine
/// deterministico per `(cell_j, cell_i)` crescente nelle griglie quadrate e
/// per colonna poi riga in quelle esagonali).
///
/// # Errors
///
/// - `ExtensionError::InvalidParameter`: `cell_size` non finito o
///   minore o uguale a zero.
/// - `ExtensionError::CellLimit`: le celle superano [`MAX_GRID_CELLS`]
///   (verificato prima dell'allocazione).
/// - `ExtensionError::IndexOverflow`: il numero di celle per asse non
///   e' rappresentabile come `u64`.
pub fn generate_grid(
    extent: &GridExtent,
    cell_size: f64,
    shape: GridShape,
) -> Result<Vec<GridCell>, ExtensionError> {
    check_cell_size(cell_size)?;
    match shape {
        GridShape::Square => {
            let (columns, rows) = square_dimensions(extent, cell_size)?;
            // columns * rows <= MAX_GRID_CELLS (verificato in
            // square_dimensions): entra in usize anche su target a 32 bit.
            #[allow(clippy::cast_possible_truncation)]
            let cell_count = (columns * rows) as usize;
            let mut cells = Vec::with_capacity(cell_count);
            for cell_j in 0..rows {
                for cell_i in 0..columns {
                    cells.push(square_cell(extent, cell_size, cell_i, cell_j));
                }
            }
            Ok(cells)
        }
        GridShape::Hex => Ok(hex_centers(extent, cell_size)?
            .into_iter()
            .map(|(cell_i, cell_j, cx, cy)| hex_cell(cell_i, cell_j, cx, cy, cell_size))
            .collect()),
    }
}

/// Riga della griglia con la geometria codificata WKB (adapter per il
/// canone GeoArrow-WKB).
#[derive(Clone, Debug, PartialEq)]
pub struct GridRow {
    /// Indice di colonna, come in [`GridCell`].
    pub cell_i: u64,
    /// Indice di riga, come in [`GridCell`].
    pub cell_j: u64,
    /// Il poligono della cella in WKB XY.
    pub wkb: Vec<u8>,
    /// Ascissa del centro.
    pub centroid_x: f64,
    /// Ordinata del centro.
    pub centroid_y: f64,
}

fn grid_error(error: &ExtensionError) -> PlenoraError {
    error.del_passo("geo.generate_grid")
}

/// Adapter righe per `geo.generate_grid`: le celle sono gia' valide per
/// costruzione; ogni geometria e' codificata WKB entro il limite per cella.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per gli errori di [`generate_grid`] (messaggio
/// con prefisso `geo.generate_grid:`) o per una serializzazione WKB fallita;
/// `PlenoraError::ResourceLimit` per una cella oltre il limite di byte per
/// cella.
pub fn generate_grid_rows(
    extent: &GridExtent,
    cell_size: f64,
    shape: GridShape,
) -> Result<Vec<GridRow>, PlenoraError> {
    let cells = generate_grid(extent, cell_size, shape).map_err(|error| grid_error(&error))?;
    cells
        .iter()
        .map(|cell| {
            Ok(GridRow {
                cell_i: cell.cell_i,
                cell_j: cell.cell_j,
                wkb: encode_geometry(&cell.geometry)?,
                centroid_x: cell.centroid_x,
                centroid_y: cell.centroid_y,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// geo.subdivide
// ---------------------------------------------------------------------------

const fn check_max_vertices(max_vertices: usize) -> Result<(), ExtensionError> {
    if max_vertices < MIN_SUBDIVIDE_VERTICES {
        return Err(invalid_parameter(
            "max_vertices",
            "deve essere almeno 4 (anello chiuso minimo)",
        ));
    }
    Ok(())
}

/// Chunking di una `LineString` in parti con vertice condiviso.
///
/// Ogni parte ha al piu' `max_vertices` vertici e l'ultimo vertice di una
/// parte e' il primo della successiva. Precondizione: `line.0.len() >
/// max_vertices`.
fn chunk_line_string(line: &LineString<f64>, max_vertices: usize) -> Vec<Geometry<f64>> {
    let count = line.0.len();
    let mut parts = Vec::new();
    let mut start = 0_usize;
    while count - start > max_vertices {
        parts.push(Geometry::LineString(LineString::new(
            line.0[start..start + max_vertices].to_vec(),
        )));
        start += max_vertices - 1;
    }
    // Il ciclo garantisce almeno due vertici residui: si ferma quando
    // `count - start > max_vertices` smette di valere.
    parts.push(Geometry::LineString(LineString::new(
        line.0[start..].to_vec(),
    )));
    parts
}

/// Taglio ricorsivo di un poligono di partenza.
fn subdivide_polygon_root(
    polygon: &Polygon<f64>,
    max_vertices: usize,
    precision: Precision,
    parts: &mut Vec<Geometry<f64>>,
) -> Result<(), ExtensionError> {
    subdivide_polygon(polygon, max_vertices, 0, precision, parts)
}

/// Taglio ricorsivo di un poligono per bisezione dell'envelope.
///
/// L'envelope e' tagliato a meta' sull'asse lungo e il poligono e'
/// intersecato nativamente (`BooleanOps`) con le due meta'; la linea di
/// taglio appartiene a entrambe le meta' (sovrapposizione di area nulla).
///
/// Ogni intersezione passa dalla griglia di `i_overlay`, con il controllo a
/// priori sull'ingombro del pezzo (la meta' vi sta dentro). I tagli sono in
/// catena, un livello sul risultato del precedente: ognuno ha la sua parte
/// di [`MAX_SUBDIVIDE_DEPTH`] del bilancio (`griglia::controlla_overlay_in_catena`),
/// e la foglia piu' profonda resta entro meta' della precisione. Nessun
/// controllo a posteriori delle foglie (README, «Limiti dichiarati»).
fn subdivide_polygon(
    polygon: &Polygon<f64>,
    max_vertices: usize,
    depth: u32,
    precision: Precision,
    parts: &mut Vec<Geometry<f64>>,
) -> Result<(), ExtensionError> {
    if polygon.coords_count() <= max_vertices {
        parts.push(Geometry::Polygon(polygon.clone()));
        return Ok(());
    }
    if depth >= MAX_SUBDIVIDE_DEPTH {
        return Err(ExtensionError::SubdivideDepth {
            limit: MAX_SUBDIVIDE_DEPTH,
        });
    }
    let rect = polygon
        .bounding_rect()
        .ok_or_else(|| ExtensionError::InvalidInput("poligono senza envelope".to_owned()))?;
    let min = rect.min();
    let max = rect.max();
    let halves = if max.x - min.x >= max.y - min.y {
        let mid_x = f64::midpoint(min.x, max.x);
        [
            Rect::new(min, Coord { x: mid_x, y: max.y }),
            Rect::new(Coord { x: mid_x, y: min.y }, max),
        ]
    } else {
        let mid_y = f64::midpoint(min.y, max.y);
        [
            Rect::new(min, Coord { x: max.x, y: mid_y }),
            Rect::new(Coord { x: min.x, y: mid_y }, max),
        ]
    };
    griglia::controlla_overlay_in_catena(Some(rect), precision, MAX_SUBDIVIDE_DEPTH)?;
    let pezzo = MultiPolygon::new(vec![polygon.clone()]);
    for half in halves {
        let meta = MultiPolygon::new(vec![half.to_polygon()]);
        let intersection = protetto(|| pezzo.intersection(&meta))?;
        for part in intersection.0 {
            // Scarti di area nulla lungo la linea di taglio.
            if part.coords_count() == 0 || protetto(|| part.unsigned_area())? == 0.0 {
                continue;
            }
            subdivide_polygon(&part, max_vertices, depth + 1, precision, parts)?;
        }
    }
    Ok(())
}

fn subdivide_validated(
    geometry: &Geometry<f64>,
    max_vertices: usize,
    precision: Precision,
) -> Result<Vec<Geometry<f64>>, ExtensionError> {
    if geometry.coords_count() <= max_vertices {
        return Ok(vec![geometry.clone()]);
    }
    let mut parts = Vec::new();
    match geometry {
        Geometry::Point(_) | Geometry::Line(_) => {
            return Err(ExtensionError::Internal(
                "punto/linea hanno al piu' 2 vertici <= max_vertices",
            ));
        }
        Geometry::LineString(line) => parts = chunk_line_string(line, max_vertices),
        Geometry::MultiLineString(lines) => {
            for line in &lines.0 {
                if line.0.len() <= max_vertices {
                    parts.push(Geometry::LineString(line.clone()));
                } else {
                    parts.extend(chunk_line_string(line, max_vertices));
                }
            }
        }
        Geometry::MultiPoint(points) => {
            for chunk in points.0.chunks(max_vertices) {
                parts.push(Geometry::MultiPoint(MultiPoint::new(chunk.to_vec())));
            }
        }
        Geometry::Polygon(polygon) => {
            subdivide_polygon_root(polygon, max_vertices, precision, &mut parts)?;
        }
        Geometry::MultiPolygon(polygons) => {
            for polygon in &polygons.0 {
                subdivide_polygon_root(polygon, max_vertices, precision, &mut parts)?;
            }
        }
        Geometry::GeometryCollection(collection) => {
            for child in &collection.0 {
                parts.extend(subdivide_validated(child, max_vertices, precision)?);
            }
        }
        Geometry::Rect(rect) => {
            subdivide_polygon_root(&rect.to_polygon(), max_vertices, precision, &mut parts)?;
        }
        Geometry::Triangle(triangle) => {
            subdivide_polygon_root(&triangle.to_polygon(), max_vertices, precision, &mut parts)?;
        }
    }
    for part in &parts {
        part.validazione_protetta().map_err(|esito| {
            esito.separa(
                |ragione| ExtensionError::InvalidOutput(ragione.to_string()),
                ExtensionError::ValidazioneNonConclusa,
            )
        })?;
    }
    Ok(parts)
}

/// Spezza una geometria con piu' di `max_vertices` vertici.
///
/// Le parti hanno al piu' `max_vertices` vertici ciascuna; le geometrie
/// sotto soglia passano invariate (una sola parte, tipo e identita'
/// geometrica preservati).
///
/// # Errors
///
/// - `ExtensionError::InvalidInput`: la geometria in ingresso non supera
///   la validazione OGC.
/// - `ExtensionError::InvalidParameter`: `max_vertices` e' minore di
///   [`MIN_SUBDIVIDE_VERTICES`].
/// - `ExtensionError::SubdivideDepth`: il taglio ricorsivo non converge
///   entro [`MAX_SUBDIVIDE_DEPTH`] livelli (geometrie degenere).
/// - `ExtensionError::InvalidOutput`: una parte prodotta non supera la
///   validazione OGC.
/// - `ExtensionError::Internal`: invariante interna violata (mai atteso:
///   punti e linee hanno al piu' 2 vertici).
/// - `ExtensionError::PrecisionInsufficient`: il taglio dei poligoni passa
///   dalla griglia di `i_overlay`, che sposterebbe una parte oltre la
///   precisione dichiarata (`precision`, nelle unita' delle
///   coordinate: `Precision::from_crs` con un CRS, altrimenti esplicita).
/// - `ExtensionError::ValidazioneNonConclusa`,
///   `ExtensionError::CalcoloNonConcluso`: una validazione OGC che non
///   conclude o un panico di `geo`/`i_overlay` (interni).
pub fn subdivide(
    geometry: &Geometry<f64>,
    max_vertices: usize,
    precision: Precision,
) -> Result<Vec<Geometry<f64>>, ExtensionError> {
    ensure_valid(geometry)?;
    check_max_vertices(max_vertices)?;
    subdivide_validated(geometry, max_vertices, precision)
}

fn subdivide_error(error: &ExtensionError) -> PlenoraError {
    error.del_passo("geo.subdivide")
}

/// Helper WKB per `geo.subdivide`: decodifica una cella, la spezza e
/// codifica ogni parte (espansione 1:N, l'indice di riga madre e'
/// responsabilita' del chiamante).
///
/// # Errors
///
/// Con prefisso `geo.subdivide:` (tradotti da [`ExtensionError::del_passo`]):
/// `PlenoraError::InvalidPlan` per `max_vertices` non valido, taglio che non
/// converge o parte invalida; `PlenoraError::Unsupported` per
/// `PrecisionInsufficient`; `PlenoraError::Internal` per un panico o una
/// validazione che non conclude. Dal decode della cella
/// (`decode_geometry_cell`) e dalla codifica delle parti (`encode_geometry`):
/// `InvalidPlan` per un WKB malformato o OGC-invalido, `Unsupported` per
/// dimensioni Z/M o SRID, `ResourceLimit` per una cella oltre il limite di
/// byte.
pub fn subdivide_wkb(
    payload: &[u8],
    max_vertices: usize,
    precision: Precision,
) -> Result<Vec<Vec<u8>>, PlenoraError> {
    check_max_vertices(max_vertices).map_err(|error| subdivide_error(&error))?;
    let geometry = decode_geometry_cell(payload)?;
    let parts = subdivide_validated(&geometry, max_vertices, precision)
        .map_err(|error| subdivide_error(&error))?;
    parts.iter().map(encode_geometry).collect()
}

// ---------------------------------------------------------------------------
// geo.snap
// ---------------------------------------------------------------------------

fn snap_with_tree(
    geometry: &Geometry<f64>,
    tree: &RTree<[f64; 2]>,
    tolerance: f64,
) -> Result<Geometry<f64>, ExtensionError> {
    // Rect/Triangle non esistono nel trasporto WKB; snappati come poligoni
    // per non violare l'invariante min <= max di Rect.
    let working = match geometry {
        Geometry::Rect(rect) => Geometry::Polygon(rect.to_polygon()),
        Geometry::Triangle(triangle) => Geometry::Polygon(triangle.to_polygon()),
        other => other.clone(),
    };
    let snapped = protetto(|| {
        working.map_coords(|coordinate| {
            match tree.nearest_neighbor(&[coordinate.x, coordinate.y]) {
                Some(&[x, y]) if (coordinate.x - x).hypot(coordinate.y - y) <= tolerance => {
                    Coord { x, y }
                }
                _ => coordinate,
            }
        })
    })?;
    validate_output(snapped)
}

fn snap_validated(
    geometry: &Geometry<f64>,
    reference: &Geometry<f64>,
    tolerance: f64,
) -> Result<Geometry<f64>, ExtensionError> {
    let reference_vertices: Vec<[f64; 2]> = reference
        .coords_iter()
        .map(|coordinate| [coordinate.x, coordinate.y])
        .collect();
    if reference_vertices.is_empty() {
        return Ok(geometry.clone());
    }
    let tree = protetto(|| RTree::bulk_load(reference_vertices))?;
    snap_with_tree(geometry, &tree, tolerance)
}

/// Aggancia ogni vertice della geometria al vertice del riferimento piu'
/// vicino entro `tolerance`.
///
/// Implementazione nativa con R-tree (niente GEOS); i vertici oltre
/// tolleranza restano invariati. Tipo preservato.
///
/// # Errors
///
/// - `ExtensionError::InvalidInput`: la geometria in ingresso o il
///   riferimento non supera la validazione OGC.
/// - `ExtensionError::InvalidParameter`: `tolerance` non finita o
///   negativa.
/// - `ExtensionError::InvalidOutput`: la geometria snappata non supera
///   la validazione OGC (lo snap puo' collassare anelli).
/// - `ExtensionError::ValidazioneNonConclusa`,
///   `ExtensionError::CalcoloNonConcluso`: una validazione OGC che non
///   conclude o un panico nell'R-tree o in `geo` (interni).
pub fn snap(
    geometry: &Geometry<f64>,
    reference: &Geometry<f64>,
    tolerance: f64,
) -> Result<Geometry<f64>, ExtensionError> {
    ensure_valid(geometry)?;
    ensure_valid(reference)?;
    check_tolerance(tolerance)?;
    snap_validated(geometry, reference, tolerance)
}

fn snap_error(error: &ExtensionError) -> PlenoraError {
    error.del_passo("geo.snap")
}

/// Adapter di colonna per `geo.snap`.
///
/// Decodifica ogni cella WKB non-null, applica lo snap verso il
/// riferimento (gia' validato in analisi) e ricodifica preservando i
/// null. Righe indipendenti: iterazione parallela con collect indicizzato
/// (ordine deterministico).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il riferimento non supera la validazione
/// OGC o `tolerance` non e' finita o e' negativa (messaggio con prefisso
/// `geo.snap:`), per un WKB di cella malformato o OGC-invalido, o se lo snap
/// di una cella produce una geometria non valida (prefisso `geo.snap:`);
/// `PlenoraError::Unsupported` per una cella con dimensioni Z/M o SRID;
/// `PlenoraError::ResourceLimit` per una cella oltre il limite di byte;
/// `PlenoraError::Internal` per un panico o una validazione che non
/// conclude. Vince la prima cella che fallisce in ordine di riga.
pub fn snap_column(
    cells: &BinaryArray,
    reference: &Geometry<f64>,
    tolerance: f64,
) -> Result<Vec<Option<Vec<u8>>>, PlenoraError> {
    ensure_valid(reference).map_err(|error| snap_error(&error))?;
    check_tolerance(tolerance).map_err(|error| snap_error(&error))?;
    // L'R-tree del riferimento e' costruito una volta per colonna, non per
    // cella: il riferimento non cambia tra le righe.
    let reference_vertices: Vec<[f64; 2]> = reference
        .coords_iter()
        .map(|coordinate| [coordinate.x, coordinate.y])
        .collect();
    if reference_vertices.is_empty() {
        // Come `snap_validated` con riferimento vuoto: geometria invariata
        // (decode + encode, nessuna validazione OGC aggiuntiva).
        return map_nullable(cells, |payload| {
            let geometry = decode_geometry_cell(payload)?;
            encode_geometry(&geometry).map(Some)
        });
    }
    let tree =
        protetto(|| RTree::bulk_load(reference_vertices)).map_err(|error| snap_error(&error))?;
    map_nullable(cells, |payload| {
        let geometry = decode_geometry_cell(payload)?;
        let snapped =
            snap_with_tree(&geometry, &tree, tolerance).map_err(|error| snap_error(&error))?;
        encode_geometry(&snapped).map(Some)
    })
}

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    /// Precisione dei test: coordinate astratte, un milionesimo di unita'.
    fn precisione() -> Precision {
        Precision::new(1e-6).unwrap()
    }
    use crate::test_support::{assert_close, rect};
    use geo::{line_string, GeometryCollection, MultiLineString, Point};
    use geozero::{CoordDimensions, ToWkb};

    /// Gli adapter v2 non accusano il piano di cio' che non ha concluso.
    #[test]
    fn gli_adapter_v2_separano_l_interno_dal_piano() {
        use plenora_core::ErrorCategory;

        for adapter in [grid_error, subdivide_error, snap_error] {
            for interno in [
                ExtensionError::Internal("forma"),
                ExtensionError::ValidazioneNonConclusa("forma"),
            ] {
                let errore = adapter(&interno);
                assert_eq!(errore.category(), ErrorCategory::Internal, "{errore}");
            }
            let del_piano = adapter(&ExtensionError::InvalidInput("anello aperto".to_owned()));
            assert_eq!(
                del_piano.category(),
                ErrorCategory::InvalidPlan,
                "{del_piano}"
            );
        }
    }

    fn extent() -> GridExtent {
        GridExtent::new(0.0, 0.0, 10.0, 10.0).expect("extent valido")
    }

    fn coords(geometry: &Geometry<f64>) -> Vec<(f64, f64)> {
        geometry
            .coords_iter()
            .map(|coordinate| (coordinate.x, coordinate.y))
            .collect()
    }

    // --- geo.generate_grid ---------------------------------------------------

    #[test]
    fn square_grid_tiles_the_extent_with_exact_coordinates() {
        let cells = generate_grid(&extent(), 5.0, GridShape::Square).expect("griglia");
        assert_eq!(cells.len(), 4);
        let first = &cells[0];
        assert_eq!((first.cell_i, first.cell_j), (0, 0));
        assert_eq!(
            coords(&first.geometry),
            vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0), (0.0, 0.0)]
        );
        assert_eq!((first.centroid_x, first.centroid_y), (2.5, 2.5));
        assert_eq!((cells[3].cell_i, cells[3].cell_j), (1, 1));
        assert_eq!((cells[3].centroid_x, cells[3].centroid_y), (7.5, 7.5));
        for cell in &cells {
            cell.geometry.validazione_protetta().expect("cella valida");
        }
    }

    #[test]
    fn square_grid_clips_edge_cells_to_the_extent() {
        let cells = generate_grid(&extent(), 3.0, GridShape::Square).expect("griglia");
        // ceil(10/3) = 4 celle per lato; l'ultima e' larga/alta 1.
        assert_eq!(cells.len(), 16);
        let last_column: Vec<&GridCell> = cells.iter().filter(|cell| cell.cell_i == 3).collect();
        assert_eq!(last_column.len(), 4);
        let rect = last_column[0].geometry.bounding_rect().expect("envelope");
        assert_eq!(rect.min().x, 9.0);
        assert_eq!(rect.max().x, 10.0);
        assert_close(rect.max().x - rect.min().x, 1.0);
        assert_close(last_column[0].centroid_x, 9.5);
        assert_eq!(
            grid_cell_count(&extent(), 3.0, GridShape::Square).unwrap(),
            16
        );
    }

    #[test]
    fn hex_grid_is_geometrically_correct_and_inside_the_extent() {
        let extent = GridExtent::new(0.0, 0.0, 10.0, 3.5).expect("extent");
        let cells = generate_grid(&extent, 1.0, GridShape::Hex).expect("griglia");
        let half_height = 3.0_f64.sqrt() / 2.0;
        // 6 colonne (cx = 1, 2.5, ..., 8.5); colonne pari 2 righe, dispari 1.
        assert_eq!(cells.len(), 9);
        assert_eq!(
            grid_cell_count(&extent, 1.0, GridShape::Hex).unwrap(),
            9,
            "conteggio coerente con la generazione"
        );
        let first = &cells[0];
        assert_eq!((first.cell_i, first.cell_j), (0, 0));
        assert_close(first.centroid_x, 1.0);
        assert_close(first.centroid_y, half_height);
        let expected = [
            (2.0, half_height),
            (1.5, 2.0 * half_height),
            (0.5, 2.0 * half_height),
            (0.0, half_height),
            (0.5, 0.0),
            (1.5, 0.0),
            (2.0, half_height),
        ];
        for ((ax, ay), (ex, ey)) in coords(&first.geometry).iter().zip(expected.iter()) {
            assert_close(*ax, *ex);
            assert_close(*ay, *ey);
        }
        // Colonna dispari: sfalsata di meta' passo verticale.
        let odd = cells
            .iter()
            .find(|cell| cell.cell_i == 1)
            .expect("colonna dispari");
        assert_close(odd.centroid_x, 2.5);
        assert_close(odd.centroid_y, 2.0 * half_height);
        // Ogni esagono e' interamente contenuto nell'extent.
        for cell in &cells {
            let rect = cell.geometry.bounding_rect().expect("envelope");
            assert!(rect.min().x >= extent.xmin && rect.max().x <= extent.xmax);
            assert!(rect.min().y >= extent.ymin && rect.max().y <= extent.ymax);
            cell.geometry.validazione_protetta().expect("cella valida");
        }
    }

    #[test]
    fn hex_grid_too_small_for_one_hexagon_produces_zero_cells() {
        let extent = GridExtent::new(0.0, 0.0, 1.0, 1.0).expect("extent");
        assert!(generate_grid(&extent, 1.0, GridShape::Hex)
            .expect("griglia")
            .is_empty());
        assert_eq!(grid_cell_count(&extent, 1.0, GridShape::Hex).unwrap(), 0);
        // Quadrata: una sola cella tagliata sull'extent.
        assert_eq!(
            generate_grid(&extent, 2.0, GridShape::Square)
                .expect("griglia")
                .len(),
            1
        );
    }

    #[test]
    fn generate_grid_rejects_degenerate_extents_and_bad_cell_sizes() {
        assert!(GridExtent::new(5.0, 0.0, 5.0, 10.0).is_err());
        assert!(GridExtent::new(0.0, 0.0, 10.0, -1.0).is_err());
        assert!(GridExtent::new(0.0, f64::NAN, 10.0, 10.0).is_err());
        assert!(GridExtent::new(0.0, 0.0, f64::INFINITY, 10.0).is_err());
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(generate_grid(&extent(), bad, GridShape::Square).is_err());
            assert!(grid_cell_count(&extent(), bad, GridShape::Hex).is_err());
        }
    }

    #[test]
    fn generate_grid_enforces_the_cell_limit_before_allocating() {
        let huge = GridExtent::new(0.0, 0.0, 1e6, 1e6).expect("extent");
        let result = generate_grid(&huge, 1.0, GridShape::Square);
        assert!(
            matches!(
                result,
                Err(ExtensionError::CellLimit {
                    limit: MAX_GRID_CELLS,
                    ..
                })
            ),
            "{result:?}"
        );
        // Anche per gli esagoni (colonne oltre il limite).
        let wide = GridExtent::new(0.0, 0.0, 1e9, 0.5).expect("extent");
        assert!(matches!(
            generate_grid(&wide, 1.0, GridShape::Hex),
            Err(ExtensionError::CellLimit { .. })
        ));
    }

    #[test]
    fn generate_grid_rows_encodes_decodable_wkb() {
        let rows = generate_grid_rows(&extent(), 5.0, GridShape::Square).expect("righe");
        assert_eq!(rows.len(), 4);
        for row in &rows {
            let geometry = crate::geometry_from_wkb(&row.wkb).expect("decode");
            assert!(matches!(geometry, Geometry::Polygon(_)));
        }
        assert!(generate_grid_rows(&extent(), 0.0, GridShape::Square).is_err());
    }

    // --- geo.subdivide -------------------------------------------------------

    #[test]
    fn subdivide_passes_small_geometries_through_unchanged() {
        let square = rect(0.0, 0.0, 4.0, 4.0);
        assert_eq!(
            subdivide(&square, 8, precisione()).unwrap(),
            vec![square.clone()]
        );
        // Soglia esatta: vertici == max_vertices, nessuna divisione.
        assert_eq!(subdivide(&square, 5, precisione()).unwrap(), vec![square]);
        let point = Geometry::Point(Point::new(1.0, 2.0));
        assert_eq!(subdivide(&point, 4, precisione()).unwrap(), vec![point]);
    }

    #[test]
    fn subdivide_chunks_linestrings_with_a_shared_vertex() {
        let line = Geometry::LineString(LineString::new(
            (0..10)
                .map(|index| Coord {
                    x: f64::from(index),
                    y: 0.0,
                })
                .collect(),
        ));
        let parts = subdivide(&line, 4, precisione()).expect("parti");
        assert_eq!(parts.len(), 3);
        for (part, expected) in parts.iter().zip([4_usize, 4, 4]) {
            assert_eq!(part.coords_count(), expected);
        }
        // Continuita': l'ultimo vertice di una parte e' il primo della successiva.
        let ends: Vec<(f64, f64)> = parts
            .iter()
            .map(|part| *coords(part).last().expect("fine"))
            .collect();
        let starts: Vec<(f64, f64)> = parts.iter().map(|part| coords(part)[0]).collect();
        assert_eq!(ends[0], starts[1]);
        assert_eq!(ends[1], starts[2]);
        assert_eq!(starts[0], (0.0, 0.0));
        assert_eq!(ends[2], (9.0, 0.0));

        // n = 2*(m-1)+1: nessuna parte da un solo vertice.
        let seven = Geometry::LineString(LineString::new(
            (0..7)
                .map(|index| Coord {
                    x: f64::from(index),
                    y: 0.0,
                })
                .collect(),
        ));
        let parts = subdivide(&seven, 4, precisione()).expect("parti");
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|part| part.coords_count() >= 2));
    }

    #[test]
    fn subdivide_splits_dense_polygons_preserving_area_and_validity() {
        // Poligono a stella con 41 vertici, sopra la soglia 8.
        let mut ring: Vec<(f64, f64)> = Vec::new();
        for index in 0..40 {
            let angle = f64::from(index) * std::f64::consts::TAU / 40.0;
            let radius = if index % 2 == 0 { 10.0 } else { 6.0 };
            ring.push((radius * angle.cos(), radius * angle.sin()));
        }
        ring.push(ring[0]);
        let star = Geometry::Polygon(Polygon::new(LineString::from(ring), Vec::new()));
        let original_area = star.unsigned_area();
        let parts = subdivide(&star, 8, precisione()).expect("parti");
        assert!(parts.len() > 1);
        let mut total_area = 0.0;
        for part in &parts {
            assert!(
                part.coords_count() <= 8,
                "parte con {} vertici",
                part.coords_count()
            );
            part.validazione_protetta().expect("parte valida");
            assert!(matches!(part, Geometry::Polygon(_)));
            total_area += part.unsigned_area();
        }
        assert!(
            (total_area - original_area).abs() < 1e-9 * original_area.max(1.0),
            "area non preservata: {total_area} vs {original_area}"
        );
    }

    #[test]
    fn subdivide_handles_polygons_with_interior_rings() {
        // Corona quadrata con anello interno densificato (piu' di 8 vertici).
        let exterior: Vec<(f64, f64)> = (0..=20)
            .map(|index| (f64::from(index), 0.0))
            .chain((1..=20).map(|index| (20.0, f64::from(index))))
            .chain((1..=20).map(|index| (20.0 - f64::from(index), 20.0)))
            .chain((1..=20).map(|index| (0.0, 20.0 - f64::from(index))))
            .collect();
        let hole = vec![
            (5.0, 5.0),
            (15.0, 5.0),
            (15.0, 15.0),
            (5.0, 15.0),
            (5.0, 5.0),
        ];
        let donut = Geometry::Polygon(Polygon::new(
            LineString::from(exterior),
            vec![LineString::from(hole)],
        ));
        let original_area = donut.unsigned_area();
        let parts = subdivide(&donut, 8, precisione()).expect("parti");
        assert!(parts.len() > 1);
        let total_area: f64 = parts.iter().map(geo::Area::unsigned_area).sum();
        for part in &parts {
            assert!(part.coords_count() <= 8);
            part.validazione_protetta().expect("parte valida");
        }
        assert!(
            (total_area - original_area).abs() < 1e-9 * original_area,
            "area con buco non preservata: {total_area} vs {original_area}"
        );
    }

    #[test]
    fn subdivide_chunks_multipoints_and_explodes_multi_lines_and_collections() {
        let points = Geometry::MultiPoint(MultiPoint::new(
            (0..10)
                .map(|index| Point::new(f64::from(index), 0.0))
                .collect(),
        ));
        let parts = subdivide(&points, 4, precisione()).expect("parti");
        assert_eq!(parts.len(), 3);
        for (part, expected) in parts.iter().zip([4_usize, 4, 2]) {
            match part {
                Geometry::MultiPoint(chunk) => assert_eq!(chunk.0.len(), expected),
                other => panic!("atteso MultiPoint, ottenuto {other:?}"),
            }
        }

        let lines = Geometry::MultiLineString(MultiLineString::new(vec![
            line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
            line_string![(x: 2.0, y: 0.0), (x: 3.0, y: 0.0)],
            LineString::new(
                (0..10)
                    .map(|index| Coord {
                        x: f64::from(index),
                        y: 1.0,
                    })
                    .collect(),
            ),
        ]));
        let parts = subdivide(&lines, 4, precisione()).expect("parti");
        // 2 linee corte passate come parti singole + 3 chunk della lunga.
        assert_eq!(parts.len(), 5);
        assert!(parts
            .iter()
            .all(|part| matches!(part, Geometry::LineString(_))));

        let collection = Geometry::GeometryCollection(GeometryCollection(vec![
            Geometry::Point(Point::new(0.0, 0.0)),
            lines,
        ]));
        let parts = subdivide(&collection, 4, precisione()).expect("parti");
        assert_eq!(parts.len(), 6);
        assert!(matches!(parts[0], Geometry::Point(_)));
    }

    #[test]
    fn subdivide_rejects_too_low_max_vertices_and_fails_closed_on_degenerates() {
        let line = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]);
        assert!(matches!(
            subdivide(&line, 3, precisione()),
            Err(ExtensionError::InvalidParameter {
                name: "max_vertices",
                ..
            })
        ));

        // Anello degenere (tutti i vertici coincidenti): envelope nullo, il
        // taglio non progredisce; il kernel fallisce chiuso (input rifiutato
        // dal validatore o limite di ricorsione).
        let degenerate = Geometry::Polygon(Polygon::new(
            LineString::from(vec![(1.0, 1.0); 10]),
            Vec::new(),
        ));
        assert!(subdivide(&degenerate, 4, precisione()).is_err());
    }

    #[test]
    fn subdivide_wkb_roundtrips_decodable_parts() {
        let line = Geometry::LineString(LineString::new(
            (0..10)
                .map(|index| Coord {
                    x: f64::from(index),
                    y: 0.0,
                })
                .collect(),
        ));
        let payload = line.to_wkb(CoordDimensions::xy()).expect("encode");
        let parts = subdivide_wkb(&payload, 4, precisione()).expect("parti");
        assert_eq!(parts.len(), 3);
        for part in &parts {
            crate::geometry_from_wkb(part).expect("decode parte");
        }
        assert!(subdivide_wkb(&payload, 2, precisione()).is_err());
    }

    // --- geo.snap ------------------------------------------------------------

    fn reference_line() -> Geometry<f64> {
        Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 10.0, y: 0.0)])
    }

    #[test]
    fn snap_moves_vertices_within_tolerance_and_keeps_the_others() {
        let input = Geometry::LineString(line_string![
            (x: 0.1, y: 0.3),   // distanza ~0.316 dal vertice (0,0): snappato
            (x: 5.0, y: 3.0),   // oltre tolleranza: invariato
            (x: 9.9, y: 0.4),   // distanza ~0.412 da (10,0): snappato
        ]);
        let snapped = snap(&input, &reference_line(), 0.5).expect("snap");
        assert_eq!(coords(&snapped), vec![(0.0, 0.0), (5.0, 3.0), (10.0, 0.0)]);
        assert!(
            matches!(snapped, Geometry::LineString(_)),
            "tipo preservato"
        );
    }

    #[test]
    fn snap_includes_the_tolerance_boundary_and_zero_means_exact_only() {
        // Distanza esattamente uguale alla tolleranza: snappato (<=).
        let input = Geometry::Point(Point::new(0.3, 0.4));
        let reference = Geometry::Point(Point::new(0.0, 0.0));
        let snapped = snap(&input, &reference, 0.5).expect("snap");
        assert_eq!(coords(&snapped), vec![(0.0, 0.0)]);

        // Tolleranza zero: solo coincidenze esatte.
        let near = Geometry::Point(Point::new(1e-9, 0.0));
        assert_eq!(snap(&near, &reference, 0.0).expect("snap"), near);
        let exact = snap(&input_at_origin(), &reference, 0.0).expect("snap");
        assert_eq!(coords(&exact), vec![(0.0, 0.0)]);
    }

    fn input_at_origin() -> Geometry<f64> {
        Geometry::Point(Point::new(0.0, 0.0))
    }

    #[test]
    fn snap_beyond_tolerance_leaves_the_geometry_unchanged() {
        let input = Geometry::LineString(line_string![(x: 5.0, y: 5.0), (x: 6.0, y: 6.0)]);
        let snapped = snap(&input, &reference_line(), 0.5).expect("snap");
        assert_eq!(snapped, input);
    }

    #[test]
    fn snap_with_empty_reference_or_duplicate_vertices_is_stable() {
        let input = Geometry::LineString(line_string![(x: 0.1, y: 0.0), (x: 5.0, y: 5.0)]);
        let empty = Geometry::MultiPoint(MultiPoint::new(Vec::new()));
        assert_eq!(snap(&input, &empty, 1.0).expect("snap"), input);

        // Riferimento con vertici duplicati: lo snap resta deterministico.
        let duplicated = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
        ]));
        let snapped = snap(&input, &duplicated, 0.5).expect("snap");
        assert_eq!(coords(&snapped), vec![(0.0, 0.0), (5.0, 5.0)]);

        // Input con vertici duplicati: ogni occorrenza e' snappata allo
        // stesso modo.
        let repeated = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(0.1, 0.0),
            Point::new(0.1, 0.0),
        ]));
        let snapped = snap(&repeated, &reference_line(), 0.5).expect("snap");
        assert_eq!(coords(&snapped), vec![(0.0, 0.0), (0.0, 0.0)]);
    }

    #[test]
    fn snap_snaps_polygon_rings_and_preserves_multipart_types() {
        // Quadrato leggermente sbilanciato verso il riferimento.
        let input = rect(0.05, 0.05, 10.05, 10.0);
        let reference = rect(0.0, 0.0, 10.0, 10.0);
        let snapped = snap(&input, &reference, 0.2).expect("snap");
        assert!(matches!(snapped, Geometry::Polygon(_)));
        assert_eq!(
            coords(&snapped),
            coords(&reference),
            "anello agganciato esattamente al riferimento"
        );

        let multi = Geometry::MultiPoint(MultiPoint::new(vec![
            Point::new(0.05, 0.05),
            Point::new(100.0, 100.0),
        ]));
        let snapped = snap(&multi, &reference, 0.2).expect("snap");
        assert!(matches!(snapped, Geometry::MultiPoint(_)));
        assert_eq!(coords(&snapped), vec![(0.0, 0.0), (100.0, 100.0)]);
    }

    #[test]
    fn snap_rejects_bad_tolerance_and_invalid_inputs() {
        let input = Geometry::Point(Point::new(0.0, 0.0));
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                snap(&input, &reference_line(), bad),
                Err(ExtensionError::InvalidParameter {
                    name: "tolerance",
                    ..
                })
            ));
        }
        assert!(snap_column(
            &BinaryArray::from_iter([Some(
                Geometry::Point(Point::new(0.0, 0.0))
                    .to_wkb(CoordDimensions::xy())
                    .expect("encode")
                    .as_slice()
            )]),
            &reference_line(),
            -1.0,
        )
        .is_err());
    }

    #[test]
    fn snap_column_maps_rows_preserving_nulls() {
        let near = Geometry::Point(Point::new(0.1, 0.2))
            .to_wkb(CoordDimensions::xy())
            .expect("encode");
        let far = Geometry::Point(Point::new(50.0, 50.0))
            .to_wkb(CoordDimensions::xy())
            .expect("encode");
        let cells = BinaryArray::from_iter([Some(near.as_slice()), None, Some(far.as_slice())]);
        let snapped = snap_column(&cells, &reference_line(), 0.5).expect("colonna");
        assert_eq!(snapped.len(), 3);
        assert!(snapped[1].is_none());
        assert_eq!(
            crate::geometry_from_wkb(snapped[0].as_deref().expect("wkb")).expect("decode"),
            Geometry::Point(Point::new(0.0, 0.0))
        );
        assert_eq!(
            crate::geometry_from_wkb(snapped[2].as_deref().expect("wkb")).expect("decode"),
            Geometry::Point(Point::new(50.0, 50.0))
        );
    }
}
