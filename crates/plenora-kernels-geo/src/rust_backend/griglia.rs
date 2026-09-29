//! La precisione dichiarata (1 cm a terra, [`super::precision`]) applicata
//! alle operazioni che passano dalla griglia intera di `i_overlay`: le
//! booleane di `geo` (`BooleanOps`, `unary_union`) e il `Buffer` di `geo`.
//!
//! # La griglia
//!
//! `i_overlay` 4.5.2 porta ogni coordinata su interi `i32` con
//! `i_float::adapter::FloatPointAdapter::new` (`i_float` 1.16.0): il
//! rettangolo d'ingombro degli operandi di **quella** chiamata, `h` la meta'
//! della sua dimensione maggiore, scala `2^(29 - round(log2(h)))` (con
//! `round` che arrotonda la meta' lontano da zero, `FloatNumber::to_i32`).
//! Il passo della griglia e' quindi `g = 2^(round(log2(h)) - 29)`
//! ([`passo_griglia`]). Nel buffer (`i_overlay::mesh`, contorni e tratti) il
//! rettangolo e' quello dell'ingresso allargato di un margine: `1.1 |d|` per
//! le giunzioni, fino a `3 |d|` per un'estremita' arrotondata (il rettangolo
//! dei punti del mezzo cerchio, largo `|d|` e alto `2 |d|`). Qui si usa
//! sempre `3 |d|`: un `h` piu' grande da' un passo uguale o doppio, mai
//! minore, quindi il controllo resta dal lato del rifiuto.
//!
//! # Lo spostamento a priori
//!
//! Un giro dell'overlay sposta un punto al piu' di:
//!
//! - `g * sqrt(2) / 2` per l'arrotondamento all'intero di ogni vertice
//!   (`float_to_int` arrotonda ogni asse al piu' vicino);
//! - `g * sqrt(2) / 2` per l'arrotondamento di un incrocio calcolato;
//! - `g` per il primo aggancio (`Solver::AUTO` usa `Precision::HIGH`: raggio
//!   quadro `1` al primo giro, raddoppiato a ogni giro successivo);
//! - pochi `ulp` delle coordinate nel passaggio `f64 -> i32 -> f64`, sotto
//!   `p / 64` ciascuno per il controllo di spaziatura.
//!
//! Il buffer passa dalla griglia piu' volte: i punti dell'offset si
//! arrotondano, il centro di un arco viene da un vertice gia' arrotondato,
//! poi l'overlay di ogni anello e quello finale. Il suo primo passaggio vale
//! `(2 + 2 sqrt(2)) g`.
//!
//! [`controlla_overlay`] rifiuta prima del calcolo se `(1 + sqrt(2)) g +
//! 4 ulp(M)` supera `p / 2` (la tolleranza del controllo a posteriori delle
//! booleane, sotto), [`controlla_buffer`] se `(2 + 2 sqrt(2)) g + 4 ulp(M)`
//! supera `p`; entrambi se le coordinate sono troppo rade
//! ([`super::precision::coordinate_abbastanza_fitte`]). In metri con 1 cm
//! il passo massimo ammesso e' `2^-9` m: un'estensione fino a circa 2.950
//! km passa (l'Italia, 1.300 km, ha `g = 2^-10`), 20.000 km no (`g = 2^-6`).
//!
//! # Lo spostamento a posteriori
//!
//! Gli agganci successivi al primo hanno un raggio che cresce a ogni giro, e
//! il numero di giri dipende dai dati: nessun limite a priori li copre. Per
//! questo, dopo l'overlay, un controllo sul risultato:
//!
//! - **booleane** ([`IndiceLinework::bordo_entro`]): il bordo esatto di
//!   un'intersezione, unione, differenza o differenza simmetrica di poligoni
//!   validi sta sui bordi degli operandi. Ogni lato del risultato deve stare
//!   **per intero** entro `p / 2` dai bordi degli operandi originali
//!   dell'operazione pubblica (non dei risultati intermedi, cosi' gli
//!   spostamenti di overlay in catena non si sommano). La tolleranza e' `p /
//!   2`, non `p`: un aggancio puo' portare un vertice da un bordo a un altro
//!   bordo d'ingresso vicino, e i lati che vi arrivano stanno allora entro
//!   `delta / 2` da uno dei due bordi, con `delta` lo spostamento; con `p /
//!   2` passa solo `delta <= p`. Il test e' esatto a meno di un margine
//!   d'arrotondamento che stringe il raggio: l'insieme dei punti entro `r`
//!   da un segmento e' convesso (uno «stadio»), la sua traccia su un lato e'
//!   un intervallo, e gli intervalli dei segmenti vicini devono ricoprire il
//!   lato;
//! - **buffer** ([`verifica_buffer`]): il bordo del buffer esatto sta alla
//!   distanza `|d|` dall'ingresso. Ogni vertice dell'uscita deve stare fra
//!   `|d| - s - p` e `k |d| + p` dall'ingresso, con `s` la freccia massima
//!   degli archi approssimati da corde ([`freccia_relativa_archi`]) e `k =
//!   sqrt(2)` per le estremita' quadrate di linee e punti (gli angoli del
//!   quadrato), `1` altrimenti; su linee con estremita' piatte o quadrate
//!   solo il limite superiore. Con `d > 0` ogni coordinata dell'ingresso
//!   deve stare dentro l'uscita o entro `p` dal suo bordo: `i_overlay` salta
//!   senza errore un anello o una linea che la griglia riduce a un punto, e
//!   il buffer di quella parte, spesso `2 |d|`, sparirebbe.
//!
//! Il controllo delle booleane e' **unilaterale**: dice che il bordo del
//! risultato sta vicino ai bordi degli ingressi, non che sia il bordo
//! giusto. Una faccia intera omessa, il cui bordo coincide con quello degli
//! ingressi, e un lato intero spostato su un bordo d'ingresso parallelo
//! (entrambi gli estremi agganciati, aggancio oltre `p / 2`) non sono visti
//! qui.

use std::f64::consts::SQRT_2;

use geo::{Coord, CoordsIter, Geometry, Line, LineString, MultiPolygon, Polygon, Rect};
use rstar::{RTree, RTreeObject, AABB};

use super::precision::{coordinate_abbastanza_fitte, modulo_massimo, Precision};

/// Lo spostamento che la griglia introdurrebbe supera la precisione: ogni
/// modulo lo traduce nella propria variante `PrecisionInsufficient`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrecisioneInsufficiente;

/// Esponente di `i_float`: la scala e' `2^(29 - round(log2(h)))`.
const ESPONENTE_GRIGLIA: i32 = 29;

/// Margine d'ingombro del buffer, in multipli di `|d|`: il massimo fra i
/// margini di `i_overlay::mesh` (giunzioni `1.1`, estremita' quadrate `2`,
/// arrotondate `3`).
pub const MARGINE_BUFFER_IN_DISTANZE: f64 = 3.0;

/// Passo angolare massimo delle corde degli archi del buffer, in radianti.
///
/// `geo` chiede archi con passo `0.2` rad (`BufferStyle`); `i_overlay`
/// divide un arco di ampiezza `a` in `round(a / 0.2)` corde, e un'ampiezza
/// sotto `0.2` diventa una sola corda: il passo resta sotto `1.5 * 0.2`. Il
/// cerchio di un punto (32 corde) e le estremita' arrotondate (15 corde su
/// mezzo giro) stanno sotto. La freccia relativa e' `1 - cos(passo / 2)`
/// ([`freccia_relativa_archi`]).
pub const PASSO_MASSIMO_ARCHI: f64 = 0.3;

/// La freccia massima degli archi del buffer, relativa alla distanza:
/// `1 - cos(PASSO_MASSIMO_ARCHI / 2)`, circa `0.0112`. La differenza fra
/// corda e arco e' l'approssimazione del buffer (README, «Limiti
/// dichiarati»), non uno spostamento della griglia.
#[must_use]
pub fn freccia_relativa_archi() -> f64 {
    1.0 - (PASSO_MASSIMO_ARCHI * 0.5).cos()
}

/// Il rettangolo d'ingombro delle coordinate; `None` senza coordinate. Una
/// coordinata non finita rende il rettangolo non finito (e il controllo di
/// spaziatura rifiuta).
pub fn rettangolo_coordinate(
    coordinate: impl IntoIterator<Item = Coord<f64>>,
) -> Option<Rect<f64>> {
    let mut iter = coordinate.into_iter();
    let first = iter.next()?;
    let (mut min, mut max) = (first, first);
    let mut non_numero = first.x.is_nan() || first.y.is_nan();
    for c in iter {
        non_numero |= c.x.is_nan() || c.y.is_nan();
        min.x = min.x.min(c.x);
        min.y = min.y.min(c.y);
        max.x = max.x.max(c.x);
        max.y = max.y.max(c.y);
    }
    if non_numero {
        let nan = Coord {
            x: f64::NAN,
            y: f64::NAN,
        };
        return Some(Rect::new(nan, nan));
    }
    Some(Rect::new(min, max))
}

/// L'unione di due rettangoli opzionali.
#[must_use]
pub fn unisci(a: Option<Rect<f64>>, b: Option<Rect<f64>>) -> Option<Rect<f64>> {
    match (a, b) {
        (Some(a), Some(b)) => rettangolo_coordinate([a.min(), a.max(), b.min(), b.max()]),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Il rettangolo allargato di `margine` per lato.
#[must_use]
pub fn allarga(rect: Rect<f64>, margine: f64) -> Rect<f64> {
    Rect::new(
        Coord {
            x: rect.min().x - margine,
            y: rect.min().y - margine,
        },
        Coord {
            x: rect.max().x + margine,
            y: rect.max().y + margine,
        },
    )
}

/// Il passo della griglia di `i_overlay` per operandi di ingombro `rect`;
/// `Some(0.0)` se il rettangolo e' un punto (tutte le coordinate coincidono
/// e tornano esatte), `None` se non e' finito.
///
/// Vicino a un pareggio `log2(h) = k + 0.5` il `log2` di `libm` usato da
/// `i_float` e quello della libreria standard possono differire di un'unita'
/// in ultima posizione e arrotondare in versi opposti: li' si prende
/// l'esponente maggiore, il passo piu' grosso.
#[must_use]
pub fn passo_griglia(rect: Rect<f64>) -> Option<f64> {
    let half = (rect.width() * 0.5).max(rect.height() * 0.5);
    if !half.is_finite() || half < 0.0 {
        return None;
    }
    if half == 0.0 {
        return Some(0.0);
    }
    let log2 = half.log2();
    let floor = log2.floor();
    let frazione = log2 - floor;
    let esponente = if (frazione - 0.5).abs() < 1e-9 || frazione > 0.5 {
        floor + 1.0
    } else {
        floor
    };
    // `esponente` e' un intero esatto in [-1075, 1024]: la conversione e'
    // esatta.
    #[allow(clippy::cast_possible_truncation)]
    let esponente = esponente as i32;
    Some(2_f64.powi(esponente - ESPONENTE_GRIGLIA))
}

/// La spaziatura dei `f64` al modulo `magnitude`.
fn ulp(magnitude: f64) -> f64 {
    let magnitude = magnitude.abs();
    f64::from_bits(magnitude.to_bits().saturating_add(1)) - magnitude
}

/// Il primo giro di un overlay booleano sposta un punto di al piu' `(1 +
/// sqrt(2)) g` (vedi il modulo).
pub const FATTORE_OVERLAY: f64 = 1.0 + SQRT_2;

/// Il primo passaggio del buffer sposta un punto di al piu' `(2 + 2
/// sqrt(2)) g` (vedi il modulo).
pub const FATTORE_BUFFER: f64 = 2.0 + 2.0 * SQRT_2;

/// La tolleranza del controllo a posteriori delle booleane, in frazioni
/// della precisione (vedi il modulo): anche il limite a priori delle
/// booleane, perche' uno spostamento legittimo non la superi.
pub const FRAZIONE_BORDO: f64 = 0.5;

/// Lo spostamento a priori della griglia: `fattore * g + 4 ulp(M)`.
#[must_use]
pub fn spostamento_a_priori(rect: Rect<f64>, fattore: f64) -> Option<f64> {
    let g = passo_griglia(rect)?;
    let magnitude = modulo_massimo([rect.min(), rect.max()]);
    let spostamento = fattore.mul_add(g, 4.0 * ulp(magnitude));
    spostamento.is_finite().then_some(spostamento)
}

/// Guardia di spaziatura e spostamento a priori entro `limite`.
fn controlla_griglia(
    rect: Option<Rect<f64>>,
    precision: Precision,
    fattore: f64,
    limite: f64,
) -> Result<(), PrecisioneInsufficiente> {
    let Some(rect) = rect else {
        return Ok(());
    };
    let magnitude = modulo_massimo([rect.min(), rect.max()]);
    if !coordinate_abbastanza_fitte(magnitude, precision.value()) {
        return Err(PrecisioneInsufficiente);
    }
    match spostamento_a_priori(rect, fattore) {
        Some(spostamento) if spostamento <= limite => Ok(()),
        _ => Err(PrecisioneInsufficiente),
    }
}

/// Il controllo prima di un overlay booleano i cui operandi hanno ingombro
/// `rect` (`None`: nessuna coordinata, nessun calcolo da controllare):
/// `(1 + sqrt(2)) g + 4 ulp(M) <= p / 2`.
///
/// # Errors
///
/// [`PrecisioneInsufficiente`] se le coordinate sono troppo rade per la
/// precisione, o se lo spostamento a priori della griglia supera `p / 2`.
pub fn controlla_overlay(
    rect: Option<Rect<f64>>,
    precision: Precision,
) -> Result<(), PrecisioneInsufficiente> {
    controlla_griglia(
        rect,
        precision,
        FATTORE_OVERLAY,
        precision.value() * FRAZIONE_BORDO,
    )
}

/// L'ingombro delle coordinate di un insieme di multipoligoni.
pub fn rettangolo_multipoligoni<'a>(
    geometrie: impl IntoIterator<Item = &'a MultiPolygon<f64>>,
) -> Option<Rect<f64>> {
    geometrie
        .into_iter()
        .map(|geometry| rettangolo_coordinate(geometry.coords_iter()))
        .fold(None, unisci)
}

/// Un segmento della linework di un ingresso, con l'etichetta del suo
/// operando (riga, lato, maschera: la sceglie il chiamante).
#[derive(Clone, Copy, Debug)]
struct Segmento {
    linea: Line<f64>,
    etichetta: usize,
}

impl RTreeObject for Segmento {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.linea.start.x, self.linea.start.y],
            [self.linea.end.x, self.linea.end.y],
        )
    }
}

/// I segmenti della linework di una geometria: gli anelli dei poligoni, le
/// linee, e i punti come segmenti degeneri.
fn raccogli_segmenti(geometry: &Geometry<f64>, etichetta: usize, out: &mut Vec<Segmento>) {
    let mut aggiungi = |linea: Line<f64>| out.push(Segmento { linea, etichetta });
    match geometry {
        Geometry::Point(point) => aggiungi(Line::new(point.0, point.0)),
        Geometry::MultiPoint(points) => {
            for point in points {
                aggiungi(Line::new(point.0, point.0));
            }
        }
        Geometry::Line(line) => aggiungi(*line),
        Geometry::GeometryCollection(collection) => {
            for child in collection {
                raccogli_segmenti(child, etichetta, out);
            }
        }
        Geometry::LineString(line) => linea_spezzata(line, &mut aggiungi),
        Geometry::MultiLineString(lines) => {
            for line in lines {
                linea_spezzata(line, &mut aggiungi);
            }
        }
        Geometry::Polygon(polygon) => anelli(polygon, &mut aggiungi),
        Geometry::MultiPolygon(polygons) => {
            for polygon in polygons {
                anelli(polygon, &mut aggiungi);
            }
        }
        Geometry::Rect(rect) => anelli(&rect.to_polygon(), &mut aggiungi),
        Geometry::Triangle(triangle) => anelli(&triangle.to_polygon(), &mut aggiungi),
    }
}

/// I lati di una spezzata; una spezzata di un solo punto e' il punto.
fn linea_spezzata(line: &LineString<f64>, linea: &mut impl FnMut(Line<f64>)) {
    if let [solo] = line.0.as_slice() {
        linea(Line::new(*solo, *solo));
        return;
    }
    for segment in line.lines() {
        linea(segment);
    }
}

fn anelli(polygon: &Polygon<f64>, linea: &mut impl FnMut(Line<f64>)) {
    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        linea_spezzata(ring, linea);
    }
}

/// La linework degli ingressi di un'operazione, indicizzata.
pub struct IndiceLinework {
    albero: RTree<Segmento>,
}

impl IndiceLinework {
    /// L'indice della linework di `geometrie`, ognuna con la sua etichetta.
    pub fn nuovo<'a>(geometrie: impl IntoIterator<Item = (usize, &'a Geometry<f64>)>) -> Self {
        let mut segmenti = Vec::new();
        for (etichetta, geometry) in geometrie {
            raccogli_segmenti(geometry, etichetta, &mut segmenti);
        }
        Self {
            albero: RTree::bulk_load(segmenti),
        }
    }

    /// Come [`Self::nuovo`], da multipoligoni.
    pub fn da_multipoligoni<'a>(
        geometrie: impl IntoIterator<Item = (usize, &'a MultiPolygon<f64>)>,
    ) -> Self {
        let mut segmenti = Vec::new();
        for (etichetta, polygons) in geometrie {
            let mut linea = |linea: Line<f64>| segmenti.push(Segmento { linea, etichetta });
            for polygon in polygons {
                anelli(polygon, &mut linea);
            }
        }
        Self {
            albero: RTree::bulk_load(segmenti),
        }
    }

    /// Ogni lato di `output` sta per intero entro `p / 2`
    /// ([`FRAZIONE_BORDO`]) dai segmenti dell'indice la cui etichetta passa
    /// `filtro`, o da uno di `extra`?
    ///
    /// Vero per il risultato esatto di una booleana (il suo bordo sta sui
    /// bordi degli operandi); falso se la griglia ha spostato un vertice, o
    /// piegato un lato, oltre `p / 2`, o agganciato un vertice a un altro
    /// bordo d'ingresso a piu' di `p` (vedi il modulo).
    pub fn bordo_entro(
        &self,
        output: &MultiPolygon<f64>,
        extra: &[Line<f64>],
        filtro: impl Fn(usize) -> bool,
        precision: Precision,
    ) -> bool {
        let r = precision.value() * FRAZIONE_BORDO;
        let mut intervalli = Vec::new();
        for polygon in output {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for lato in ring.lines() {
                    intervalli.clear();
                    let busta = AABB::from_corners(
                        [
                            lato.start.x.min(lato.end.x) - r,
                            lato.start.y.min(lato.end.y) - r,
                        ],
                        [
                            lato.start.x.max(lato.end.x) + r,
                            lato.start.y.max(lato.end.y) + r,
                        ],
                    );
                    let candidati = self
                        .albero
                        .locate_in_envelope_intersecting(&busta)
                        .filter(|segmento| filtro(segmento.etichetta))
                        .map(|segmento| segmento.linea)
                        .chain(extra.iter().copied());
                    for segmento in candidati {
                        if let Some(intervallo) =
                            intervallo_entro(lato.start, lato.end, segmento.start, segmento.end, r)
                        {
                            intervalli.push(intervallo);
                        }
                    }
                    if !ricopre(&mut intervalli) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Il punto sta dentro i poligoni i cui anelli sono nell'indice, o
    /// entro `raggio` dal loro bordo?
    ///
    /// Prima la vicinanza al bordo; poi, per un punto a piu' di `raggio` dal
    /// bordo, la parita' degli attraversamenti del raggio orizzontale verso
    /// `+x` (regola semiaperta sugli estremi): l'ascissa d'incrocio dista dal
    /// punto almeno quanto il bordo, cioe' piu' di `raggio`, e il suo errore
    /// d'arrotondamento non cambia il verso del confronto.
    pub fn copre(&self, punto: Coord<f64>, raggio: f64) -> bool {
        let busta = AABB::from_corners(
            [punto.x - raggio, punto.y - raggio],
            [punto.x + raggio, punto.y + raggio],
        );
        let margine = 16.0 * f64::EPSILON * (punto.x.abs() + punto.y.abs() + raggio);
        if self
            .albero
            .locate_in_envelope_intersecting(&busta)
            .any(|segmento| distanza_da_segmento(punto, segmento.linea) + margine <= raggio)
        {
            return true;
        }
        let semiretta = AABB::from_corners([punto.x, punto.y], [f64::MAX, punto.y]);
        let attraversamenti = self
            .albero
            .locate_in_envelope_intersecting(&semiretta)
            .filter(|segmento| {
                let (a, b) = (segmento.linea.start, segmento.linea.end);
                if (a.y > punto.y) == (b.y > punto.y) {
                    return false;
                }
                let x = (punto.y - a.y).mul_add((b.x - a.x) / (b.y - a.y), a.x);
                x > punto.x
            })
            .count();
        attraversamenti % 2 == 1
    }

    /// Ogni vertice di `output` dista dai segmenti dell'indice almeno
    /// `minima` e al piu' `massima`?
    pub fn vertici_alla_distanza(
        &self,
        output: &MultiPolygon<f64>,
        minima: f64,
        massima: f64,
    ) -> bool {
        if !(minima.is_finite() && massima.is_finite()) {
            return false;
        }
        output.coords_iter().all(|vertice| {
            let busta = AABB::from_corners(
                [vertice.x - massima, vertice.y - massima],
                [vertice.x + massima, vertice.y + massima],
            );
            let distanza = self
                .albero
                .locate_in_envelope_intersecting(&busta)
                .map(|segmento| distanza_da_segmento(vertice, segmento.linea))
                .fold(f64::INFINITY, f64::min);
            // Margine d'arrotondamento della distanza, sempre dal lato del
            // rifiuto.
            let margine = 16.0 * f64::EPSILON * (vertice.x.abs() + vertice.y.abs() + massima);
            distanza.is_finite()
                && (minima <= 0.0 || distanza - margine >= minima)
                && distanza + margine <= massima
        })
    }
}

/// La distanza del punto dal segmento, calcolata sulle differenze
/// dall'origine del segmento.
fn distanza_da_segmento(point: Coord<f64>, segment: Line<f64>) -> f64 {
    let px = point.x - segment.start.x;
    let py = point.y - segment.start.y;
    let dx = segment.end.x - segment.start.x;
    let dy = segment.end.y - segment.start.y;
    let lunghezza2 = dx.mul_add(dx, dy * dy);
    let t = if lunghezza2 > 0.0 {
        (px.mul_add(dx, py * dy) / lunghezza2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    t.mul_add(-dx, px).hypot(t.mul_add(-dy, py))
}

/// L'intervallo `[t0, t1]`, dentro `[0, 1]`, dei parametri per cui il punto
/// `inizio + t (fine - inizio)` dista dal segmento `da -> verso` al piu'
/// `raggio`; `None` se vuoto.
///
/// L'insieme dei punti entro `r` dal segmento e' lo stadio, convesso:
/// rettangolo lungo il segmento unito ai due dischi degli estremi. La
/// traccia di un segmento su un convesso e' un intervallo, e coincide con
/// l'inviluppo delle tracce sui tre pezzi. Il raggio e' ristretto di un
/// margine d'arrotondamento relativo alle grandezze in gioco: il test puo'
/// rifiutare un punto al limite, mai accettarne uno oltre.
fn intervallo_entro(
    inizio: Coord<f64>,
    fine: Coord<f64>,
    da: Coord<f64>,
    verso: Coord<f64>,
    raggio: f64,
) -> Option<(f64, f64)> {
    // Tutto relativo a `da`.
    let ux = inizio.x - da.x;
    let uy = inizio.y - da.y;
    let wx = fine.x - inizio.x;
    let wy = fine.y - inizio.y;
    let bx = verso.x - da.x;
    let by = verso.y - da.y;
    let scala = ux.abs() + uy.abs() + wx.abs() + wy.abs() + bx.abs() + by.abs();
    let r = 64.0f64.mul_add(-f64::EPSILON * scala, raggio);
    if r.is_nan() || r <= 0.0 {
        return None;
    }
    let mut inviluppo: Option<(f64, f64)> = None;
    let mut aggiungi = |intervallo: Option<(f64, f64)>| {
        if let Some((t0, t1)) = intervallo {
            let t0 = t0.max(0.0);
            let t1 = t1.min(1.0);
            if t0 <= t1 {
                inviluppo = Some(match inviluppo {
                    Some((s0, s1)) => (s0.min(t0), s1.max(t1)),
                    None => (t0, t1),
                });
            }
        }
    };
    aggiungi(dentro_disco(ux, uy, wx, wy, r));
    aggiungi(dentro_disco(ux - bx, uy - by, wx, wy, r));
    let lunghezza2 = bx.mul_add(bx, by * by);
    if lunghezza2 > 0.0 {
        let lunghezza = lunghezza2.sqrt();
        let (dx, dy) = (bx / lunghezza, by / lunghezza);
        // Lungo il segmento: 0 <= (u + t w) . d <= L.
        let lungo = entro_lineare(
            ux.mul_add(dx, uy * dy),
            wx.mul_add(dx, wy * dy),
            0.0,
            lunghezza,
        );
        // Di traverso: -r <= (u + t w) . n <= r, n = (-dy, dx).
        let traverso = entro_lineare(
            (-ux).mul_add(dy, uy * dx),
            (-wx).mul_add(dy, wy * dx),
            -r,
            r,
        );
        if let (Some(lungo), Some(traverso)) = (lungo, traverso) {
            aggiungi(Some((lungo.0.max(traverso.0), lungo.1.min(traverso.1))));
        }
    }
    inviluppo
}

/// I `t` per cui `lo <= c0 + c1 t <= hi`.
fn entro_lineare(c0: f64, c1: f64, lo: f64, hi: f64) -> Option<(f64, f64)> {
    if c1 == 0.0 {
        return (lo <= c0 && c0 <= hi).then_some((f64::NEG_INFINITY, f64::INFINITY));
    }
    let t_lo = (lo - c0) / c1;
    let t_hi = (hi - c0) / c1;
    let (t0, t1) = if t_lo <= t_hi {
        (t_lo, t_hi)
    } else {
        (t_hi, t_lo)
    };
    (t0 <= t1).then_some((t0, t1))
}

/// I `t` per cui `|(ux, uy) + t (wx, wy)| <= r`.
///
/// Dal piede della perpendicolare, non dal discriminante dell'equazione di
/// secondo grado: con lati lunghi centinaia di km `|u|^2` e `(u . w)^2`
/// valgono `1e22` e la loro differenza perderebbe il quadrato del raggio
/// (`1e-4` m^2). Qui la distanza dalla retta viene da un prodotto vettore
/// diviso per `|w|` (errore dell'ordine di `EPSILON |u|`, dentro il margine
/// tolto al raggio) e la semiampiezza da `(r - h)(r + h)`.
fn dentro_disco(ux: f64, uy: f64, wx: f64, wy: f64, r: f64) -> Option<(f64, f64)> {
    let a = wx.mul_add(wx, wy * wy);
    if a == 0.0 {
        return (ux.hypot(uy) <= r).then_some((f64::NEG_INFINITY, f64::INFINITY));
    }
    let lunghezza = a.sqrt();
    let piede = -ux.mul_add(wx, uy * wy) / a;
    let distanza = ux.mul_add(wy, -(uy * wx)).abs() / lunghezza;
    if distanza.is_nan() || distanza > r {
        return None;
    }
    let semiampiezza = ((r - distanza) * (r + distanza)).sqrt() / lunghezza;
    Some((piede - semiampiezza, piede + semiampiezza))
}

/// Gli intervalli ricoprono `[0, 1]`?
fn ricopre(intervalli: &mut [(f64, f64)]) -> bool {
    intervalli.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
    let mut coperto = 0.0_f64;
    for &(t0, t1) in intervalli.iter() {
        if t0 > coperto {
            return false;
        }
        coperto = coperto.max(t1);
        if coperto >= 1.0 {
            return true;
        }
    }
    false
}

/// Il controllo a priori di un buffer di distanza `distance` su `geometry`:
/// l'ingombro dell'ingresso allargato di [`MARGINE_BUFFER_IN_DISTANZE`]
/// `|d|` per lato.
///
/// # Errors
///
/// Come [`controlla_overlay`].
pub fn controlla_buffer(
    geometry: &Geometry<f64>,
    distance: f64,
    precision: Precision,
) -> Result<(), PrecisioneInsufficiente> {
    let rect = rettangolo_coordinate(geometry.coords_iter())
        .map(|rect| allarga(rect, MARGINE_BUFFER_IN_DISTANZE * distance.abs()));
    controlla_griglia(rect, precision, FATTORE_BUFFER, precision.value())
}

/// Le estremita' delle linee nel buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Estremita {
    Tonde,
    Piatte,
    Quadrate,
}

/// La geometria ha linee (con estremita' libere)?
fn ha_linee(geometry: &Geometry<f64>) -> bool {
    match geometry {
        Geometry::Line(_) | Geometry::LineString(_) | Geometry::MultiLineString(_) => true,
        Geometry::GeometryCollection(collection) => collection.iter().any(ha_linee),
        Geometry::Point(_)
        | Geometry::MultiPoint(_)
        | Geometry::Polygon(_)
        | Geometry::MultiPolygon(_)
        | Geometry::Rect(_)
        | Geometry::Triangle(_) => false,
    }
}

/// La geometria ha linee o punti (le sole parti a cui `geo` applica le
/// estremita': i poligoni le ignorano)?
fn ha_estremita(geometry: &Geometry<f64>) -> bool {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => true,
        Geometry::GeometryCollection(collection) => collection.iter().any(ha_estremita),
        other => ha_linee(other),
    }
}

/// Il controllo a posteriori di un buffer: ogni vertice dell'uscita sta fra
/// `|d| - s - p` e `k |d| + p` dalla linework dell'ingresso (vedi il
/// modulo); `k = sqrt(2)` con estremita' quadrate su linee o punti (gli
/// angoli del quadrato), `1` altrimenti. Con `d > 0` ogni coordinata
/// dell'ingresso sta dentro l'uscita o entro `p` dal suo bordo: nessuna
/// parte dell'ingresso ha perso il suo buffer.
///
/// Con estremita' piatte o quadrate su linee il limite inferiore non vale:
/// il bordo di un'estremita' piatta passa a distanza fra `0` e `|d|`
/// dall'estremo della linea (lo attraversa), e un vertice dell'unione puo'
/// cadervi sopra. Li' resta il solo limite superiore: il controllo vede lo
/// spostamento verso l'esterno, non quello verso l'interno.
///
/// # Errors
///
/// [`PrecisioneInsufficiente`] se un vertice esce dalla fascia.
pub fn verifica_buffer(
    geometry: &Geometry<f64>,
    distance: f64,
    estremita: Estremita,
    output: &MultiPolygon<f64>,
    precision: Precision,
) -> Result<(), PrecisioneInsufficiente> {
    let da_coprire = if distance > 0.0 {
        coordinate_da_coprire(geometry, estremita)
    } else {
        Vec::new()
    };
    if output.0.is_empty() && da_coprire.is_empty() {
        return Ok(());
    }
    let d = distance.abs();
    let p = precision.value();
    let minima = if estremita != Estremita::Tonde && ha_linee(geometry) {
        0.0
    } else {
        (freccia_relativa_archi().mul_add(-d, d) - p).max(0.0)
    };
    let massima = if estremita == Estremita::Quadrate && ha_estremita(geometry) {
        SQRT_2 * d
    } else {
        d
    } + p;
    let indice = IndiceLinework::nuovo([(0, geometry)]);
    if !indice.vertici_alla_distanza(output, minima, massima) {
        return Err(PrecisioneInsufficiente);
    }
    if !da_coprire.is_empty() {
        let uscita = IndiceLinework::da_multipoligoni([(0, output)]);
        if !da_coprire
            .iter()
            .all(|coordinata| uscita.copre(*coordinata, p))
        {
            return Err(PrecisioneInsufficiente);
        }
    }
    Ok(())
}

/// Le coordinate dell'ingresso che un buffer positivo deve coprire: tutte,
/// salvo punti e linee con estremita' piatte. Un punto con estremita'
/// piatte non ha buffer per definizione (`geo`), e una linea piu' corta
/// della griglia avrebbe un buffer piatto piu' sottile della griglia: che
/// sparisca e' dichiarato (una geometria piu' sottile della precisione).
fn coordinate_da_coprire(geometry: &Geometry<f64>, estremita: Estremita) -> Vec<Coord<f64>> {
    fn raccogli(geometry: &Geometry<f64>, estremita: Estremita, out: &mut Vec<Coord<f64>>) {
        match geometry {
            Geometry::GeometryCollection(collection) => {
                for child in collection {
                    raccogli(child, estremita, out);
                }
            }
            Geometry::Point(_)
            | Geometry::MultiPoint(_)
            | Geometry::Line(_)
            | Geometry::LineString(_)
            | Geometry::MultiLineString(_)
                if estremita == Estremita::Piatte => {}
            other => out.extend(other.coords_iter()),
        }
    }
    let mut coordinate = Vec::new();
    raccogli(geometry, estremita, &mut coordinate);
    coordinate
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use geo::polygon;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect<f64> {
        Rect::new(Coord { x: x0, y: y0 }, Coord { x: x1, y: y1 })
    }

    /// Il passo riproduce `FloatPointAdapter::new` di `i_float` 1.16.0.
    #[test]
    fn il_passo_e_quello_di_i_float() {
        // h = 1: log2 = 0, passo 2^-29.
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 2.0, 1.0)),
            Some(2_f64.powi(-29))
        );
        // h = 650 km: log2 = 19.31, round 19, passo 2^-10.
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 1_300_000.0, 10.0)),
            Some(2_f64.powi(-10))
        );
        // h = 10.000 km: log2 = 23.25, round 23, passo 2^-6.
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 20_000_000.0, 10.0)),
            Some(2_f64.powi(-6))
        );
        // Pareggio: h = 2^0.5, arrotonda lontano da zero (esponente 1).
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 2.0 * SQRT_2, 0.0)),
            Some(2_f64.powi(-28))
        );
        assert_eq!(passo_griglia(rect(5.0, 5.0, 5.0, 5.0)), Some(0.0));
        assert_eq!(passo_griglia(rect(0.0, 0.0, f64::INFINITY, 0.0)), None);
    }

    /// Controprova diretta sulla dipendenza: un vertice a `1.3 g` da un
    /// punto della griglia torna a `1 g`. Con un passo doppio o dimezzato
    /// tornerebbe a `0` o `2 g` (per `g / 2`: `2.6 -> 3` meta' passi).
    #[test]
    fn il_passo_coincide_con_l_adapter_della_dipendenza() {
        use geo::algorithm::bool_ops::BooleanOps as _;
        for lato in [1.0, 1_300_000.0, 20_000_000.0, 0.001_953_125] {
            let g = passo_griglia(rect(0.0, 0.0, lato, lato)).unwrap();
            let anello = |x0: f64, y0: f64, x1: f64, y1: f64| {
                Polygon::new(
                    LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
                    vec![],
                )
            };
            let quadrato = anello(0.0, 0.0, lato, lato);
            let spostato = lato.mul_add(0.25, 1.3 * g);
            let interno = anello(spostato, lato * 0.25, lato * 0.75, lato * 0.75);
            let uscita = quadrato.difference(&interno);
            let atteso = lato.mul_add(0.25, g);
            assert!(
                uscita
                    .coords_iter()
                    .any(|c| (c.x - atteso).abs() < g * 1e-3),
                "lato {lato}"
            );
        }
    }

    #[test]
    fn il_controllo_a_priori_segue_l_estensione() {
        let centimetro = Precision::new(0.01).unwrap();
        assert!(
            controlla_overlay(Some(rect(0.0, 0.0, 1_300_000.0, 1_000_000.0)), centimetro).is_ok()
        );
        assert!(controlla_overlay(Some(rect(0.0, 0.0, 2_900_000.0, 10.0)), centimetro).is_ok());
        assert_eq!(
            controlla_overlay(Some(rect(0.0, 0.0, 5_000_000.0, 10.0)), centimetro),
            Err(PrecisioneInsufficiente)
        );
        assert_eq!(
            controlla_overlay(Some(rect(0.0, 0.0, 20_000_000.0, 10.0)), centimetro),
            Err(PrecisioneInsufficiente)
        );
        // Coordinate oltre la guardia di modulo, estensione minuscola.
        let lontano = 2_f64.powi(45);
        assert_eq!(
            controlla_overlay(
                Some(rect(lontano, lontano, lontano + 1.0, lontano + 1.0)),
                centimetro
            ),
            Err(PrecisioneInsufficiente)
        );
        assert_eq!(
            controlla_overlay(
                rettangolo_coordinate([
                    Coord {
                        x: f64::NAN,
                        y: 0.0
                    },
                    Coord { x: 1.0, y: 1.0 }
                ]),
                centimetro
            ),
            Err(PrecisioneInsufficiente)
        );
        assert!(controlla_overlay(None, centimetro).is_ok());
    }

    #[test]
    fn l_intervallo_e_la_traccia_dello_stadio() {
        let c = |x: f64, y: f64| Coord { x, y };
        // Lato parallelo a 0.5 dal segmento [0,10]: dentro con r = 1.
        let (t0, t1) =
            intervallo_entro(c(0.0, 0.5), c(10.0, 0.5), c(0.0, 0.0), c(10.0, 0.0), 1.0).unwrap();
        assert!(t0 <= 0.0 + 1e-12 && t1 >= 1.0 - 1e-12);
        // A 2 unita': fuori.
        assert!(
            intervallo_entro(c(0.0, 2.0), c(10.0, 2.0), c(0.0, 0.0), c(10.0, 0.0), 1.0).is_none()
        );
        // Lato che esce oltre l'estremo: coperto fino a 11 (disco in b).
        let (t0, t1) =
            intervallo_entro(c(0.0, 0.0), c(20.0, 0.0), c(0.0, 0.0), c(10.0, 0.0), 1.0).unwrap();
        assert!(t0 <= 1e-12);
        assert!((t1 - 11.0 / 20.0).abs() < 1e-9);
        // Due segmenti collineari consecutivi ricoprono il lato intero.
        let mut intervalli = vec![
            intervallo_entro(c(0.0, 0.0), c(20.0, 0.0), c(0.0, 0.0), c(10.0, 0.0), 0.01).unwrap(),
            intervallo_entro(c(0.0, 0.0), c(20.0, 0.0), c(10.0, 0.0), c(20.0, 0.0), 0.01).unwrap(),
        ];
        assert!(ricopre(&mut intervalli));
        // Un buco fra i due: non ricoperto.
        let mut intervalli = vec![
            intervallo_entro(c(0.0, 0.0), c(20.0, 0.0), c(0.0, 0.0), c(9.0, 0.0), 0.01).unwrap(),
            intervallo_entro(c(0.0, 0.0), c(20.0, 0.0), c(10.0, 0.0), c(20.0, 0.0), 0.01).unwrap(),
        ];
        assert!(!ricopre(&mut intervalli));
    }

    /// Lati di centinaia di km: il disco dell'estremo copre la coda di un
    /// lato che supera il segmento di pochi millimetri (l'equazione di
    /// secondo grado perdeva il raggio nel rumore di `|u|^2`).
    #[test]
    fn i_lati_lunghi_non_perdono_il_disco() {
        let c = |x: f64, y: f64| Coord { x, y };
        let a = c(-203_556.482_421_875, 4_134_927.552_734_375);
        let b = c(-858_488.960_937_5, 4_000_000.0);
        let oltre = c(b.x - 0.003, b.y - 0.0005);
        let (t0, t1) = intervallo_entro(a, oltre, b, c(b.x + 1.0, b.y - 50_000.0), 0.01).unwrap();
        assert!(t0 < 1.0 && t1 >= 1.0, "{t0} {t1}");
        let (t0, t1) =
            dentro_disco(a.x - b.x, a.y - b.y, oltre.x - a.x, oltre.y - a.y, 0.01).unwrap();
        assert!(t0 < 1.0 && t1 >= 1.0, "{t0} {t1}");
    }

    /// Un lato piegato di 6 mm verso l'esterno e' rifiutato (tolleranza `p / 2`); di
    /// 4 mm no.
    #[test]
    fn il_bordo_spostato_oltre_la_precisione_e_visto() {
        let centimetro = Precision::new(0.01).unwrap();
        let ingresso: Geometry<f64> =
            polygon![(x: 0.0, y: 0.0), (x: 100.0, y: 0.0), (x: 100.0, y: 100.0), (x: 0.0, y: 100.0)]
                .into();
        let indice = IndiceLinework::nuovo([(0, &ingresso)]);
        let piegato = |dy: f64| {
            MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 0.0), (x: 50.0, y: -dy), (x: 100.0, y: 0.0),
                (x: 100.0, y: 100.0), (x: 0.0, y: 100.0)
            ]])
        };
        assert!(indice.bordo_entro(&piegato(0.004), &[], |_| true, centimetro));
        assert!(!indice.bordo_entro(&piegato(0.02), &[], |_| true, centimetro));
        assert!(!indice.bordo_entro(&piegato(0.006), &[], |_| true, centimetro));
        // L'etichetta filtrata non conta.
        assert!(!indice.bordo_entro(&piegato(0.0), &[], |_| false, centimetro));
    }

    /// Il buffer di un punto spostato di 2 cm e' rifiutato; quello vero no.
    #[test]
    fn il_buffer_spostato_oltre_la_precisione_e_visto() {
        use geo::Buffer as _;
        let centimetro = Precision::new(0.01).unwrap();
        let punto = Geometry::Point(geo::Point::new(500_000.0, 4_000_000.0));
        let buffer = punto.buffer(100.0);
        assert!(verifica_buffer(&punto, 100.0, Estremita::Tonde, &buffer, centimetro).is_ok());
        let spostato = geo::MapCoords::map_coords(&buffer, |c| Coord {
            x: c.x + 0.02,
            y: c.y,
        });
        assert_eq!(
            verifica_buffer(&punto, 100.0, Estremita::Tonde, &spostato, centimetro),
            Err(PrecisioneInsufficiente)
        );
    }

    /// Revisione: un vertice agganciato a un bordo d'ingresso parallelo a
    /// 1,8 cm stava entro 0,9 cm da uno dei due bordi lungo i lati obliqui, e
    /// passava con tolleranza `p`. Con `p / 2` no; a 0,8 cm si'.
    #[test]
    fn il_vertice_agganciato_a_un_altro_bordo_e_visto() {
        let centimetro = Precision::new(0.01).unwrap();
        let sotto: Geometry<f64> =
            polygon![(x: 0.0, y: 0.0), (x: 200.0, y: 0.0), (x: 200.0, y: 50.0), (x: 0.0, y: 50.0)]
                .into();
        let spostato = |dy: f64| {
            let sopra: Geometry<f64> = polygon![
                (x: 0.0, y: 50.0 + dy), (x: 200.0, y: 50.0 + dy),
                (x: 200.0, y: 100.0), (x: 0.0, y: 100.0)
            ]
            .into();
            let indice = IndiceLinework::nuovo([(0, &sotto), (1, &sopra)]);
            let uscita = MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 0.0), (x: 200.0, y: 0.0), (x: 200.0, y: 50.0),
                (x: 100.0, y: 50.0 + dy), (x: 0.0, y: 50.0)
            ]]);
            indice.bordo_entro(&uscita, &[], |_| true, centimetro)
        };
        assert!(!spostato(0.018));
        assert!(spostato(0.008));
    }

    /// Revisione: estremita' quadrate su un poligono (che `geo` ignora):
    /// un'uscita gonfiata di 3 m su 10 m non passa piu' con `k = sqrt(2)`.
    #[test]
    fn le_estremita_quadrate_non_allargano_il_buffer_di_un_poligono() {
        use geo::Buffer as _;
        let centimetro = Precision::new(0.01).unwrap();
        let quadrato: Geometry<f64> =
            polygon![(x: 0.0, y: 0.0), (x: 100.0, y: 0.0), (x: 100.0, y: 100.0), (x: 0.0, y: 100.0)]
                .into();
        let buffer = quadrato.buffer(10.0);
        assert!(verifica_buffer(&quadrato, 10.0, Estremita::Quadrate, &buffer, centimetro).is_ok());
        let gonfiato = quadrato.buffer(13.0);
        assert_eq!(
            verifica_buffer(&quadrato, 10.0, Estremita::Quadrate, &gonfiato, centimetro),
            Err(PrecisioneInsufficiente)
        );
    }

    /// Revisione: la parte dell'ingresso che la griglia riduce a un punto
    /// perde il buffer senza errore di `i_overlay`; il controllo lo vede.
    #[test]
    fn la_parte_senza_buffer_e_vista() {
        use geo::Buffer as _;
        let centimetro = Precision::new(0.01).unwrap();
        let due = Geometry::MultiPoint(geo::MultiPoint::from(vec![
            (500_000.0, 4_000_000.0),
            (500_100.0, 4_000_000.0),
        ]));
        let solo_uno = Geometry::Point(geo::Point::new(500_000.0, 4_000_000.0)).buffer(10.0);
        assert_eq!(
            verifica_buffer(&due, 10.0, Estremita::Tonde, &solo_uno, centimetro),
            Err(PrecisioneInsufficiente)
        );
        assert!(
            verifica_buffer(&due, 10.0, Estremita::Tonde, &due.buffer(10.0), centimetro).is_ok()
        );
        // Uscita vuota con distanza positiva: errore.
        assert_eq!(
            verifica_buffer(
                &due,
                10.0,
                Estremita::Tonde,
                &MultiPolygon::new(vec![]),
                centimetro
            ),
            Err(PrecisioneInsufficiente)
        );
    }

    /// La copertura per parita' distingue dentro, fuori e il buco.
    #[test]
    fn la_copertura_conta_i_buchi() {
        let centimetro = 0.01;
        let ciambella = MultiPolygon::new(vec![Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (0.0, 0.0),
            ]),
            vec![LineString::from(vec![
                (4.0, 4.0),
                (6.0, 4.0),
                (6.0, 6.0),
                (4.0, 6.0),
                (4.0, 4.0),
            ])],
        )]);
        let indice = IndiceLinework::da_multipoligoni([(0, &ciambella)]);
        let c = |x: f64, y: f64| Coord { x, y };
        assert!(indice.copre(c(2.0, 2.0), centimetro));
        assert!(!indice.copre(c(5.0, 5.0), centimetro));
        assert!(!indice.copre(c(12.0, 5.0), centimetro));
        assert!(indice.copre(c(10.005, 5.0), centimetro));
        // Sulla quota di un vertice (regola semiaperta).
        assert!(indice.copre(c(2.0, 4.0), centimetro));
        assert!(!indice.copre(c(5.0, 4.5), centimetro));
    }
}
