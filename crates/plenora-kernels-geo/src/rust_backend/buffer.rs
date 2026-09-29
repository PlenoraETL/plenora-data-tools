//! Buffer planare entro la precisione dichiarata: il `Buffer` di `geo`
//! (`i_overlay::mesh`, contorni e tratti) con gli archi scelti dalla
//! precisione, e controllato a posteriori contro la **definizione esatta**.
//!
//! **Archi.** `geo` espone `LineJoin::Round(a)` e `LineCap::Round(a)` di
//! `i_overlay` 4.5.2, con `a` il passo angolare richiesto; `i_overlay` lo
//! porta in `[0.01 pi, 0.25 pi]` (`mesh/style.rs`) e divide un arco di
//! ampiezza `A` in `round(A / a)` corde: il passo effettivo resta sotto
//! `1.5 a` (il cerchio di un punto, `ceil(2 pi / a)` corde, e le estremita',
//! `floor(pi / a)`, stanno sotto). Per una freccia al piu' `f` si chiede
//! `a = (4/3) acos(1 - f / |d|)`, portato nello stesso intervallo
//! ([`angolo_degli_archi`]; un passo piu' fine rispetta la freccia a
//! maggior ragione). `f = max(p / 2, 0.001 |d|)` ([`freccia_degli_archi`]):
//! fino a `|d| = 500 p` (5 m con 1 cm) freccia piu' griglia restano entro
//! `p`; oltre, la freccia e' lo 0,1% della distanza, **deviazione
//! dichiarata** del solo buffer (README «Limiti dichiarati»). Con il passo
//! minimo `0.01 pi` la freccia e' al piu' `2.8e-4 |d|`, quindi sempre entro
//! la tolleranza.
//!
//! **Griglia.** Il buffer passa dalla griglia di `i_overlay` piu' volte
//! (offset arrotondati, centri degli archi da vertici gia' arrotondati,
//! overlay dei contorni e finale): il primo passaggio sposta un punto di al
//! piu' `(2 + 2 sqrt(2)) g`, con `g` il passo sull'ingombro allargato di
//! `3 |d|` (il margine massimo di `i_overlay::mesh`). Oltre `p`, errore
//! prima del calcolo: e' un filtro grossolano, la garanzia (entro `p / 2`
//! dalla definizione) e' il controllo a posteriori.
//!
//! **Componenti sotto la griglia.** `i_overlay` salta senza errore un
//! anello d'area intera nulla e una linea i cui punti cadono sullo stesso
//! punto della griglia: il loro buffer, spesso `2 |d|`, sparirebbe. Prima
//! del calcolo una linea piu' corta di `2 g` diventa il suo primo punto, un
//! poligono d'area sotto `4 g^2` o piu' sottile di `2 g` il suo anello
//! esterno (e poi, se corto, un punto): lo scarto e' sotto la griglia.
//!
//! **Controllo a posteriori contro la definizione**, non contro cio' che il
//! calcolo ha prodotto ([`verifica_contro_la_definizione`]). La definizione
//! esatta con `d > 0` e' l'unione di: rettangoli dei lati (allungati di
//! `d` agli estremi liberi solo con estremita' quadrate), settori dei coni
//! normali ai vertici interni (giunzioni tonde), dischi (tonde) o quadrati
//! (quadrate) agli estremi liberi e ai punti, parti areali; con `d < 0`,
//! i punti delle parti areali a distanza almeno `|d|` dai loro anelli. Tre
//! prove, tutte su indici `rstar`:
//!
//! - ogni vertice dell'uscita sta nella definizione allargata di `p / 2`;
//! - ogni vertice dell'uscita e' vicino al bordo esatto: non sta nella
//!   definizione ristretta di `f + p / 2` (una faccia bucata o un vertice
//!   che affonda nel buffer e' un errore);
//! - punti campione ben dentro la definizione (a `|d| - f - p / 2` dai
//!   lati, lungo le bisettrici dei giunti e davanti agli estremi, dentro le
//!   parti areali) stanno dentro l'uscita: una parte persa e' un errore.
//!
//! Un lato dell'uscita fra due vertici corretti non e' controllato punto per
//! punto: e' una corda di un arco o un tratto parallelo all'ingresso.

use std::f64::consts::PI;

use geo::algorithm::bool_ops::unary_union;
use geo::algorithm::buffer::{BufferStyle, LineCap, LineJoin};
use geo::orient::{Direction, Orient};
use geo::{
    Area, BoundingRect, Buffer, Coord, CoordsIter, Geometry, LineString, MultiLineString,
    MultiPolygon, Point, Polygon,
};

use super::griglia::{
    self, ErroreVerifica, IndiceLinework, Operandi, PrecisioneInsufficiente, Regola,
};
use super::precision::Precision;

/// Le estremita' delle linee nel buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Estremita {
    Tonde,
    Piatte,
    Quadrate,
}

/// Perche' il buffer non si calcola.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErroreBuffer {
    /// La griglia supererebbe la precisione, o un controllo a posteriori
    /// ha fallito.
    PrecisioneInsufficiente,
    /// Un calcolo di `geo` o `i_overlay` e' andato in panico dentro
    /// [`crate::calcolo_protetto`]: la forma del payload, mai il contenuto.
    CalcoloNonConcluso(&'static str),
}

impl From<PrecisioneInsufficiente> for ErroreBuffer {
    fn from(_: PrecisioneInsufficiente) -> Self {
        Self::PrecisioneInsufficiente
    }
}

impl From<ErroreVerifica> for ErroreBuffer {
    fn from(errore: ErroreVerifica) -> Self {
        match errore {
            ErroreVerifica::PrecisioneInsufficiente => Self::PrecisioneInsufficiente,
            ErroreVerifica::CalcoloNonConcluso(forma) => Self::CalcoloNonConcluso(forma),
        }
    }
}

/// Un calcolo di `geo` dietro la barriera dei panici.
fn protetto<T>(calcolo: impl FnOnce() -> T) -> Result<T, ErroreBuffer> {
    crate::calcolo_protetto(calcolo).map_err(ErroreBuffer::CalcoloNonConcluso)
}

/// La freccia relativa massima ammessa per gli archi del buffer: lo 0,1%
/// della distanza (deviazione dichiarata, vedi il modulo).
pub const FRECCIA_RELATIVA_MASSIMA: f64 = 0.001;

/// La freccia degli archi di un buffer di distanza `distance` con
/// precisione `precision`: `max(p / 2, 0.001 |d|)` (vedi il modulo).
#[must_use]
pub fn freccia_degli_archi(distance: f64, precision: Precision) -> f64 {
    (precision.value() * griglia::FRAZIONE_BORDO).max(FRECCIA_RELATIVA_MASSIMA * distance.abs())
}

/// Il passo angolare minimo e massimo che `i_overlay` accetta.
const ANGOLO_MINIMO: f64 = 0.01 * PI;
const ANGOLO_MASSIMO: f64 = 0.25 * PI;

/// Il passo angolare da chiedere a `i_overlay` perche' la freccia degli
/// archi di raggio `raggio` resti entro `freccia` (vedi il modulo).
#[must_use]
pub fn angolo_degli_archi(raggio: f64, freccia: f64) -> f64 {
    let rapporto = freccia / raggio.abs();
    if !(rapporto.is_finite() && rapporto > 0.0) {
        return ANGOLO_MINIMO;
    }
    let angolo = if rapporto >= 1.0 {
        ANGOLO_MASSIMO
    } else {
        (4.0 / 3.0) * (1.0 - rapporto).acos() * (1.0 - 1e-9)
    };
    angolo.clamp(ANGOLO_MINIMO, ANGOLO_MASSIMO)
}

/// Il primo passaggio del buffer sposta un punto di al piu' `(2 + 2
/// sqrt(2)) g` (vedi il modulo).
const FATTORE_BUFFER: f64 = 2.0 + 2.0 * std::f64::consts::SQRT_2;

/// Margine d'ingombro del buffer, in multipli di `|d|`: il massimo fra i
/// margini di `i_overlay::mesh` (giunzioni `1.1`, estremita' quadrate `2`,
/// tonde `3`).
const MARGINE_IN_DISTANZE: f64 = 3.0;

/// L'ingombro dell'ingresso allargato di `margine` per lato.
fn ingombro_allargato(geometry: &Geometry<f64>, margine: f64) -> Option<geo::Rect<f64>> {
    griglia::rettangolo_coordinate(geometry.coords_iter()).map(|rect| {
        geo::Rect::new(
            Coord {
                x: rect.min().x - margine,
                y: rect.min().y - margine,
            },
            Coord {
                x: rect.max().x + margine,
                y: rect.max().y + margine,
            },
        )
    })
}

/// La normale sinistra.
const fn sinistra(u: Coord<f64>) -> Coord<f64> {
    Coord { x: -u.y, y: u.x }
}

/// `a + k u`.
const fn somma(a: Coord<f64>, b: Coord<f64>, k: f64) -> Coord<f64> {
    Coord {
        x: k.mul_add(b.x, a.x),
        y: k.mul_add(b.y, a.y),
    }
}

fn direzione(a: Coord<f64>, b: Coord<f64>) -> Option<(Coord<f64>, f64)> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let lunghezza = dx.hypot(dy);
    (lunghezza > 0.0 && lunghezza.is_finite()).then(|| {
        (
            Coord {
                x: dx / lunghezza,
                y: dy / lunghezza,
            },
            lunghezza,
        )
    })
}

/// Le parti areali di una geometria areale, orientate.
fn areale_di(geometry: &Geometry<f64>) -> Option<MultiPolygon<f64>> {
    let polygons = match geometry {
        Geometry::Polygon(polygon) => MultiPolygon::new(vec![polygon.clone()]),
        Geometry::MultiPolygon(polygons) => polygons.clone(),
        Geometry::Rect(rect) => MultiPolygon::new(vec![rect.to_polygon()]),
        Geometry::Triangle(triangle) => MultiPolygon::new(vec![triangle.to_polygon()]),
        _ => return None,
    };
    let polygons = MultiPolygon::new(
        polygons
            .0
            .into_iter()
            .filter(|polygon| polygon.exterior().0.len() > 1)
            .collect(),
    );
    Some(polygons.orient(Direction::Default))
}

/// Le parti areali di primo livello (e delle collezioni).
fn areali(geometry: &Geometry<f64>, out: &mut Vec<MultiPolygon<f64>>) {
    match geometry {
        Geometry::GeometryCollection(collection) => {
            for child in collection {
                areali(child, out);
            }
        }
        other => {
            if let Some(polygons) = areale_di(other) {
                if !polygons.0.is_empty() {
                    out.push(polygons);
                }
            }
        }
    }
}

/// L'ingresso con le componenti sotto la griglia `g` sostituite (vedi il
/// modulo): una linea corta dal suo primo punto, un poligono minuscolo o
/// sottile dal suo anello esterno.
fn senza_componenti_sotto_griglia(geometry: &Geometry<f64>, g: f64) -> Geometry<f64> {
    let corta = |coordinate: &[Coord<f64>]| {
        griglia::rettangolo_coordinate(coordinate.iter().copied())
            .is_some_and(|rect| rect.width().max(rect.height()) <= 2.0 * g)
    };
    let da_linea = |line: &LineString<f64>| -> Geometry<f64> {
        match line.0.first() {
            Some(primo) if corta(&line.0) => Geometry::Point(Point(*primo)),
            _ => Geometry::LineString(line.clone()),
        }
    };
    let sottile = |polygon: &Polygon<f64>| {
        polygon.unsigned_area() <= 4.0 * g * g
            || polygon
                .bounding_rect()
                .is_some_and(|rect| rect.width().min(rect.height()) <= 2.0 * g)
    };
    match geometry {
        Geometry::Line(line) => da_linea(&LineString::new(vec![line.start, line.end])),
        Geometry::LineString(line) => da_linea(line),
        Geometry::MultiLineString(lines) => {
            let parti: Vec<Geometry<f64>> = lines.iter().map(da_linea).collect();
            if parti.iter().all(|p| matches!(p, Geometry::LineString(_))) {
                geometry.clone()
            } else {
                Geometry::GeometryCollection(parti.into())
            }
        }
        Geometry::Polygon(polygon) if sottile(polygon) => da_linea(polygon.exterior()),
        Geometry::MultiPolygon(polygons) if polygons.iter().any(sottile) => {
            let mut parti: Vec<Geometry<f64>> = Vec::new();
            let mut pieni = Vec::new();
            for polygon in polygons {
                if sottile(polygon) {
                    parti.push(da_linea(polygon.exterior()));
                } else {
                    pieni.push(polygon.clone());
                }
            }
            if !pieni.is_empty() {
                parti.push(Geometry::MultiPolygon(MultiPolygon::new(pieni)));
            }
            Geometry::GeometryCollection(parti.into())
        }
        Geometry::GeometryCollection(collection) => Geometry::GeometryCollection(
            collection
                .iter()
                .map(|child| senza_componenti_sotto_griglia(child, g))
                .collect::<Vec<_>>()
                .into(),
        ),
        other => other.clone(),
    }
}

/// Il buffer e' vuoto per definizione, senza calcolo: distanza negativa
/// senza parti areali, o soli punti con estremita' piatte (come `geo`).
fn nulla_da_bufferizzare(geometry: &Geometry<f64>, distance: f64, estremita: Estremita) -> bool {
    fn soli_punti(geometry: &Geometry<f64>) -> bool {
        match geometry {
            Geometry::Point(_) | Geometry::MultiPoint(_) => true,
            Geometry::GeometryCollection(collection) => collection.iter().all(soli_punti),
            _ => false,
        }
    }
    if distance < 0.0 {
        let mut parti = Vec::new();
        areali(geometry, &mut parti);
        return parti.is_empty();
    }
    estremita == Estremita::Piatte && soli_punti(geometry)
}

/// L'unione controllata delle parti areali (buffer a distanza nulla).
fn unione(
    operandi: &[MultiPolygon<f64>],
    precision: Precision,
) -> Result<MultiPolygon<f64>, ErroreBuffer> {
    if operandi.is_empty() {
        return Ok(MultiPolygon::new(Vec::new()));
    }
    griglia::controlla_overlay(griglia::rettangolo_multipoligoni(operandi), precision)?;
    let risultato = protetto(|| unary_union(operandi))?;
    let controllo = Operandi::nuovi(operandi.iter().collect())?;
    controllo.verifica(&risultato, |_| true, None, Regola::Unione, precision)?;
    Ok(risultato)
}

/// Una forma della definizione esatta del buffer, con il suo ingombro.
#[derive(Clone, Copy, Debug)]
enum Forma {
    /// I punti che si proiettano sul lato `a + t u`, `t` in `[-prima, l +
    /// dopo]`, a distanza al piu' `r` dalla sua retta.
    Rettangolo {
        a: Coord<f64>,
        u: Coord<f64>,
        l: f64,
        prima: f64,
        dopo: f64,
    },
    /// Il disco di raggio `r`.
    Disco(Coord<f64>),
    /// La giunzione tonda al vertice `c` fra un lato entrante di direzione
    /// `u` e uno uscente di direzione `w`: i punti del disco di raggio `r`
    /// nel cono normale `e . u >= 0`, `e . w <= 0` (nulla se i lati sono
    /// allineati, il mezzo disco davanti se si invertono).
    Giunto {
        c: Coord<f64>,
        u: Coord<f64>,
        w: Coord<f64>,
    },
    /// Il quadrato di mezzo lato `r` (estremita' quadrate di un punto).
    Quadrato(Coord<f64>),
}

#[derive(Clone, Copy, Debug)]
struct FormaIndicizzata {
    forma: Forma,
    envelope: rstar::AABB<[f64; 2]>,
}

impl rstar::RTreeObject for FormaIndicizzata {
    type Envelope = rstar::AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// La definizione esatta del buffer di raggio `raggio` (vedi il modulo),
/// **indipendente da cio' che il calcolo ha prodotto**, su un indice
/// `rstar`; le parti areali su indici dei loro anelli (parita').
///
/// `rientro` accorcia di tanto i rettangoli agli estremi liberi con
/// estremita' piatte: serve alla definizione ristretta (i vertici
/// dell'uscita non devono affondarvi).
struct Definizione {
    raggio: f64,
    albero: rstar::RTree<FormaIndicizzata>,
    areali: Vec<IndiceLinework>,
}

/// Una spezzata senza vertici ripetuti (e senza la chiusura, se chiusa).
fn vertici_distinti(coordinate: &[Coord<f64>], chiusa: bool) -> Vec<Coord<f64>> {
    let mut punti: Vec<Coord<f64>> = Vec::with_capacity(coordinate.len());
    for c in coordinate {
        if punti.last() != Some(c) {
            punti.push(*c);
        }
    }
    if chiusa && punti.len() > 1 && punti.first() == punti.last() {
        punti.pop();
    }
    punti
}

impl Definizione {
    fn nuova(geometry: &Geometry<f64>, raggio: f64, estremita: Estremita, rientro: f64) -> Self {
        let mut forme = Vec::new();
        let mut areali = Vec::new();
        Self::raccogli(
            geometry,
            raggio,
            estremita,
            rientro,
            &mut forme,
            &mut areali,
        );
        Self {
            raggio,
            albero: rstar::RTree::bulk_load(forme),
            areali: areali
                .iter()
                .map(|parte| IndiceLinework::da_multipoligoni([(0, parte)]))
                .collect(),
        }
    }

    fn con_ingombro(forma: Forma, raggio: f64) -> FormaIndicizzata {
        let (minimo, massimo) = match forma {
            Forma::Rettangolo {
                a,
                u,
                l,
                prima,
                dopo,
            } => {
                let inizio = somma(a, u, -prima);
                let fine = somma(a, u, l + dopo);
                (
                    [inizio.x.min(fine.x) - raggio, inizio.y.min(fine.y) - raggio],
                    [inizio.x.max(fine.x) + raggio, inizio.y.max(fine.y) + raggio],
                )
            }
            Forma::Disco(c) | Forma::Quadrato(c) | Forma::Giunto { c, .. } => {
                ([c.x - raggio, c.y - raggio], [c.x + raggio, c.y + raggio])
            }
        };
        FormaIndicizzata {
            forma,
            envelope: rstar::AABB::from_corners(minimo, massimo),
        }
    }

    fn raccogli(
        geometry: &Geometry<f64>,
        raggio: f64,
        estremita: Estremita,
        rientro: f64,
        forme: &mut Vec<FormaIndicizzata>,
        areali: &mut Vec<MultiPolygon<f64>>,
    ) {
        let forma_del_punto = |c: Coord<f64>, forme: &mut Vec<FormaIndicizzata>| match estremita {
            Estremita::Tonde => forme.push(Self::con_ingombro(Forma::Disco(c), raggio)),
            Estremita::Quadrate => forme.push(Self::con_ingombro(Forma::Quadrato(c), raggio)),
            Estremita::Piatte => {}
        };
        let spezzata = |coordinate: &[Coord<f64>],
                        chiusa: bool,
                        forme: &mut Vec<FormaIndicizzata>| {
            let punti = vertici_distinti(coordinate, chiusa);
            if punti.len() == 1 {
                forma_del_punto(punti[0], forme);
                return;
            }
            let lati = if chiusa {
                punti.len()
            } else {
                punti.len().saturating_sub(1)
            };
            let libero = match estremita {
                Estremita::Quadrate => raggio,
                Estremita::Piatte => -rientro,
                Estremita::Tonde => 0.0,
            };
            for indice in 0..lati {
                let a = punti[indice];
                let b = punti[(indice + 1) % punti.len()];
                if let Some((u, l)) = direzione(a, b) {
                    let prima = if !chiusa && indice == 0 { libero } else { 0.0 };
                    let dopo = if !chiusa && indice + 1 == lati {
                        libero
                    } else {
                        0.0
                    };
                    forme.push(Self::con_ingombro(
                        Forma::Rettangolo {
                            a,
                            u,
                            l,
                            prima,
                            dopo,
                        },
                        raggio,
                    ));
                }
            }
            for (indice, c) in punti.iter().enumerate() {
                let estremo = !chiusa && (indice == 0 || indice + 1 == punti.len());
                if estremo {
                    if estremita == Estremita::Tonde {
                        forme.push(Self::con_ingombro(Forma::Disco(*c), raggio));
                    }
                    continue;
                }
                let n = punti.len();
                let prima = punti[(indice + n - 1) % n];
                let dopo = punti[(indice + 1) % n];
                if let (Some((u, _)), Some((w, _))) = (direzione(prima, *c), direzione(*c, dopo)) {
                    forme.push(Self::con_ingombro(Forma::Giunto { c: *c, u, w }, raggio));
                }
            }
        };
        match geometry {
            Geometry::Point(p) => forma_del_punto(p.0, forme),
            Geometry::MultiPoint(points) => {
                for p in points {
                    forma_del_punto(p.0, forme);
                }
            }
            Geometry::Line(line) => spezzata(&[line.start, line.end], false, forme),
            Geometry::LineString(line) => spezzata(&line.0, false, forme),
            Geometry::MultiLineString(lines) => {
                for line in lines {
                    spezzata(&line.0, false, forme);
                }
            }
            Geometry::GeometryCollection(collection) => {
                for child in collection {
                    Self::raccogli(child, raggio, estremita, rientro, forme, areali);
                }
            }
            altro => {
                if let Some(polygons) = areale_di(altro) {
                    for polygon in &polygons {
                        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                            spezzata(&ring.0, true, forme);
                        }
                    }
                    areali.push(polygons);
                }
            }
        }
    }

    /// Il tratto `t` in `[0, 1]` del lato `s + t w` dentro la forma
    /// allargata di `tolleranza` (una forma e' convessa: la traccia e' un
    /// intervallo).
    fn traccia(
        forma: Forma,
        raggio: f64,
        lato: geo::Line<f64>,
        tolleranza: f64,
    ) -> Option<(f64, f64)> {
        use griglia::{dentro_disco, entro_lineare};
        let s = lato.start;
        let wx = lato.end.x - s.x;
        let wy = lato.end.y - s.y;
        let r = raggio + tolleranza;
        let interseca = |a: Option<(f64, f64)>, b: Option<(f64, f64)>| match (a, b) {
            (Some((a0, a1)), Some((b0, b1))) => {
                let (t0, t1) = (a0.max(b0), a1.min(b1));
                (t0 <= t1).then_some((t0, t1))
            }
            _ => None,
        };
        match forma {
            Forma::Rettangolo {
                a,
                u,
                l,
                prima,
                dopo,
            } => {
                let dx = s.x - a.x;
                let dy = s.y - a.y;
                let lungo = entro_lineare(
                    dx.mul_add(u.x, dy * u.y),
                    wx.mul_add(u.x, wy * u.y),
                    -prima - tolleranza,
                    l + dopo + tolleranza,
                );
                let traverso = entro_lineare(
                    (-dx).mul_add(u.y, dy * u.x),
                    (-wx).mul_add(u.y, wy * u.x),
                    -r,
                    r,
                );
                interseca(lungo, traverso)
            }
            Forma::Disco(c) => dentro_disco(s.x - c.x, s.y - c.y, wx, wy, r),
            Forma::Quadrato(c) => interseca(
                entro_lineare(s.x - c.x, wx, -r, r),
                entro_lineare(s.y - c.y, wy, -r, r),
            ),
            Forma::Giunto { c, u, w } => {
                let ex = s.x - c.x;
                let ey = s.y - c.y;
                let disco = dentro_disco(ex, ey, wx, wy, r);
                let davanti = entro_lineare(
                    ex.mul_add(u.x, ey * u.y),
                    wx.mul_add(u.x, wy * u.y),
                    -tolleranza,
                    f64::INFINITY,
                );
                let dietro = entro_lineare(
                    ex.mul_add(w.x, ey * w.y),
                    wx.mul_add(w.x, wy * w.y),
                    f64::NEG_INFINITY,
                    tolleranza,
                );
                interseca(interseca(disco, davanti), dietro)
            }
        }
    }

    /// Il lato sta per intero nella definizione allargata di `tolleranza`
    /// (parti areali escluse: un lato dell'uscita non le attraversa)?
    fn copre_lato(&self, lato: geo::Line<f64>, tolleranza: f64) -> bool {
        let busta = rstar::AABB::from_corners(
            [
                lato.start.x.min(lato.end.x) - tolleranza,
                lato.start.y.min(lato.end.y) - tolleranza,
            ],
            [
                lato.start.x.max(lato.end.x) + tolleranza,
                lato.start.y.max(lato.end.y) + tolleranza,
            ],
        );
        let mut intervalli: Vec<(f64, f64)> = self
            .albero
            .locate_in_envelope_intersecting(&busta)
            .filter_map(|indicizzata| {
                Self::traccia(indicizzata.forma, self.raggio, lato, tolleranza)
            })
            .map(|(t0, t1)| (t0.max(0.0), t1.min(1.0)))
            .filter(|(t0, t1)| t0 <= t1)
            .collect();
        griglia::ricopre(&mut intervalli)
    }

    /// Il lato tocca la definizione (ristretta di `margine`)?
    /// I tratti del lato dentro la definizione (ristretta di `margine`).
    fn tracce(&self, lato: geo::Line<f64>, margine: f64) -> Vec<(f64, f64)> {
        let busta = rstar::AABB::from_corners(
            [lato.start.x.min(lato.end.x), lato.start.y.min(lato.end.y)],
            [lato.start.x.max(lato.end.x), lato.start.y.max(lato.end.y)],
        );
        self.albero
            .locate_in_envelope_intersecting(&busta)
            .filter_map(|indicizzata| Self::traccia(indicizzata.forma, self.raggio, lato, -margine))
            .map(|(t0, t1)| (t0.max(0.0), t1.min(1.0)))
            .filter(|(t0, t1)| t0 <= t1)
            .collect()
    }

    /// Il punto sta nella definizione allargata di `tolleranza` (negativa:
    /// ristretta)?
    fn contiene(&self, v: Coord<f64>, tolleranza: f64) -> bool {
        let r = self.raggio + tolleranza;
        let busta = rstar::AABB::from_corners(
            [v.x - tolleranza.abs(), v.y - tolleranza.abs()],
            [v.x + tolleranza.abs(), v.y + tolleranza.abs()],
        );
        self.albero
            .locate_in_envelope_intersecting(&busta)
            .any(|indicizzata| match indicizzata.forma {
                Forma::Rettangolo {
                    a,
                    u,
                    l,
                    prima,
                    dopo,
                } => {
                    let dx = v.x - a.x;
                    let dy = v.y - a.y;
                    let lungo = dx.mul_add(u.x, dy * u.y);
                    let traverso = (-dx).mul_add(u.y, dy * u.x).abs();
                    lungo >= -prima - tolleranza && lungo <= l + dopo + tolleranza && traverso <= r
                }
                Forma::Disco(c) => (v.x - c.x).hypot(v.y - c.y) <= r,
                Forma::Giunto { c, u, w } => {
                    let ex = v.x - c.x;
                    let ey = v.y - c.y;
                    ex.hypot(ey) <= r
                        && ex.mul_add(u.x, ey * u.y) >= -tolleranza
                        && ex.mul_add(w.x, ey * w.y) <= tolleranza
                }
                Forma::Quadrato(c) => (v.x - c.x).abs() <= r && (v.y - c.y).abs() <= r,
            })
            || self.areali.iter().any(|parte| parte.dentro(v))
    }
}

/// I punti campione ben dentro la definizione di raggio `raggio` con
/// `d > 0` (vedi il modulo): ai due lati del punto medio di ogni lato,
/// lungo la bisettrice del cono di ogni giunto, davanti agli estremi tondi,
/// attorno ai punti. Con estremita' piatte i lati estremi piu' corti di
/// `2 rientro` non danno campioni (il campione cadrebbe vicino al bordo
/// dell'estremita').
fn campioni(
    geometry: &Geometry<f64>,
    raggio: f64,
    estremita: Estremita,
    rientro: f64,
    out: &mut Vec<Coord<f64>>,
) {
    let attorno = |c: Coord<f64>, out: &mut Vec<Coord<f64>>| {
        if estremita != Estremita::Piatte {
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                out.push(Coord {
                    x: raggio.mul_add(dx, c.x),
                    y: raggio.mul_add(dy, c.y),
                });
            }
        }
    };
    let spezzata = |coordinate: &[Coord<f64>], chiusa: bool, out: &mut Vec<Coord<f64>>| {
        let punti = vertici_distinti(coordinate, chiusa);
        if punti.len() == 1 {
            attorno(punti[0], out);
            return;
        }
        let lati = if chiusa {
            punti.len()
        } else {
            punti.len().saturating_sub(1)
        };
        let mut direzioni = Vec::with_capacity(lati);
        for indice in 0..lati {
            let inizio = punti[indice];
            let fine = punti[(indice + 1) % punti.len()];
            let Some((u, lunghezza)) = direzione(inizio, fine) else {
                continue;
            };
            direzioni.push(Some(u));
            let estremo = !chiusa && (indice == 0 || indice + 1 == lati);
            if estremo && estremita == Estremita::Piatte && lunghezza < 2.0 * rientro {
                continue;
            }
            let medio = Coord {
                x: f64::midpoint(inizio.x, fine.x),
                y: f64::midpoint(inizio.y, fine.y),
            };
            let n = sinistra(u);
            out.push(somma(medio, n, raggio));
            out.push(somma(medio, n, -raggio));
        }
        if direzioni.len() != lati {
            return;
        }
        let direzioni: Vec<Coord<f64>> = direzioni.into_iter().flatten().collect();
        let giunti = if chiusa { 0..lati } else { 1..lati };
        for indice in giunti {
            let u = direzioni[(indice + lati - 1) % lati];
            let w = direzioni[indice % lati];
            let bisettrice = Coord {
                x: u.x - w.x,
                y: u.y - w.y,
            };
            let norma = bisettrice.x.hypot(bisettrice.y);
            if norma > 1e-6 {
                out.push(somma(punti[indice], bisettrice, raggio / norma));
            }
        }
        if !chiusa && estremita == Estremita::Tonde {
            out.push(somma(punti[0], direzioni[0], -raggio));
            out.push(somma(punti[punti.len() - 1], direzioni[lati - 1], raggio));
        }
    };
    match geometry {
        Geometry::Point(p) => attorno(p.0, out),
        Geometry::MultiPoint(points) => {
            for p in points {
                attorno(p.0, out);
            }
        }
        Geometry::Line(line) => spezzata(&[line.start, line.end], false, out),
        Geometry::LineString(line) => spezzata(&line.0, false, out),
        Geometry::MultiLineString(lines) => {
            for line in lines {
                spezzata(&line.0, false, out);
            }
        }
        Geometry::GeometryCollection(collection) => {
            for child in collection {
                campioni(child, raggio, estremita, rientro, out);
            }
        }
        altro => {
            if let Some(polygons) = areale_di(altro) {
                for polygon in &polygons {
                    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                        spezzata(&ring.0, true, out);
                    }
                }
            }
        }
    }
}

/// I tratti chiusi di `[0, 1]` fuori dagli intervalli: un tratto libero
/// comprende i suoi estremi, cosi' i tratti ammessi di parti diverse si
/// toccano.
fn complemento(intervalli: &mut [(f64, f64)]) -> Vec<(f64, f64)> {
    intervalli.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
    let mut liberi = Vec::new();
    let mut coperto = 0.0_f64;
    for &(t0, t1) in intervalli.iter() {
        if t0 > coperto {
            liberi.push((coperto, t0));
        }
        coperto = coperto.max(t1);
    }
    if intervalli.is_empty() || coperto < 1.0 {
        liberi.push((coperto, 1.0));
    }
    liberi.retain(|(t0, t1)| t1 > t0);
    liberi
}

/// Gli anelli di una parte areale come linee (per le fasce dell'erosione).
fn anelli_di(parte: &MultiPolygon<f64>) -> Geometry<f64> {
    Geometry::MultiLineString(MultiLineString::new(
        parte
            .iter()
            .flat_map(|polygon| {
                std::iter::once(polygon.exterior().clone())
                    .chain(polygon.interiors().iter().cloned())
            })
            .collect(),
    ))
}

/// Il controllo a posteriori contro la **definizione** del buffer (vedi il
/// modulo): vertici dentro la definizione allargata, vertici non affondati
/// nella definizione ristretta, campioni profondi dentro l'uscita. La
/// tolleranza e' `p / 2` (la griglia) piu' un margine d'arrotondamento; la
/// freccia `f` si aggiunge verso l'interno (gli archi sono inscritti).
// Due rami (d > 0, d < 0) con tre prove ciascuno: lunghezza intrinseca.
#[allow(clippy::too_many_lines)]
fn verifica_contro_la_definizione(
    geometry: &Geometry<f64>,
    distance: f64,
    estremita: Estremita,
    freccia: f64,
    output: &MultiPolygon<f64>,
    precision: Precision,
) -> Result<(), ErroreBuffer> {
    let d = distance.abs();
    let margine = |v: Coord<f64>| 64.0 * f64::EPSILON * (v.x.abs() + v.y.abs() + d);
    let tolleranza = precision.value() * griglia::FRAZIONE_BORDO;
    let profondita = freccia + tolleranza;
    let esito = protetto(|| {
        let uscita = IndiceLinework::da_multipoligoni([(0, output)]);
        if distance > 0.0 {
            let definizione = Definizione::nuova(geometry, d, estremita, 0.0);
            let interno = d - profondita;
            let ristretta = (interno > 0.0)
                .then(|| Definizione::nuova(geometry, interno, estremita, profondita));
            // Ogni lato per intero dentro la definizione allargata (le
            // parti areali: un lato del buffer positivo non le attraversa;
            // se l'ingresso e' solo areale, i vertici dentro la parte
            // restano ammessi dal controllo dei vertici qui sotto).
            let lati_ok = output.iter().all(|polygon| {
                std::iter::once(polygon.exterior())
                    .chain(polygon.interiors())
                    .all(|ring| {
                        ring.lines().all(|lato| {
                            definizione.copre_lato(lato, tolleranza + margine(lato.start))
                        })
                    })
            });
            let vertici_ok = lati_ok
                && output.coords_iter().all(|v| {
                    ristretta
                        .as_ref()
                        .is_none_or(|ristretta| !ristretta.contiene(v, -margine(v)))
                });
            if !vertici_ok {
                return false;
            }
            if interno <= 0.0 {
                return true;
            }
            let mut punti = Vec::new();
            campioni(geometry, interno, estremita, profondita, &mut punti);
            punti.into_iter().all(|q| uscita.dentro(q))
        } else {
            let mut parti = Vec::new();
            areali(geometry, &mut parti);
            let fasce: Vec<(IndiceLinework, Definizione, Definizione, Definizione)> = parti
                .iter()
                .map(|parte| {
                    let anelli = anelli_di(parte);
                    (
                        IndiceLinework::da_multipoligoni([(0, parte)]),
                        Definizione::nuova(&anelli, 0.0, Estremita::Tonde, 0.0),
                        Definizione::nuova(
                            &anelli,
                            (d - profondita).max(0.0),
                            Estremita::Tonde,
                            0.0,
                        ),
                        Definizione::nuova(&anelli, d + tolleranza, Estremita::Tonde, 0.0),
                    )
                })
                .collect();
            // Ogni vertice dentro una parte e vicino al bordo esatto; ogni
            // lato fuori dalla fascia stretta della parte (quindi non
            // attraversa i suoi anelli).
            let vertici_ok = output.coords_iter().all(|v| {
                fasce.iter().any(|(parte, bordo, stretta, larga)| {
                    let dentro = parte.dentro(v) || bordo.contiene(v, tolleranza + margine(v));
                    dentro
                        && (stretta.raggio <= 0.0 || !stretta.contiene(v, -margine(v)))
                        && larga.contiene(v, margine(v))
                })
            }) && output.iter().all(|polygon| {
                std::iter::once(polygon.exterior())
                    .chain(polygon.interiors())
                    .all(|ring| {
                        ring.lines().all(|lato| {
                            // I tratti ammessi da ciascuna parte (fuori dalla
                            // sua fascia stretta, quindi tutti dentro o tutti
                            // fuori dalla parte: si prova il punto medio),
                            // riuniti su tutte le parti, devono ricoprire il
                            // lato: le erosioni di parti sovrapposte si
                            // uniscono (revisione Codex, ultimo giro).
                            let mut ammessi: Vec<(f64, f64)> = Vec::new();
                            for (parte, _, stretta, _) in &fasce {
                                let mut vietati = if stretta.raggio > 0.0 {
                                    stretta.tracce(lato, margine(lato.start))
                                } else {
                                    Vec::new()
                                };
                                for (t0, t1) in complemento(&mut vietati) {
                                    let t = f64::midpoint(t0, t1);
                                    let medio = Coord {
                                        x: t.mul_add(lato.end.x - lato.start.x, lato.start.x),
                                        y: t.mul_add(lato.end.y - lato.start.y, lato.start.y),
                                    };
                                    if parte.dentro(medio) {
                                        ammessi.push((t0, t1));
                                    }
                                }
                            }
                            griglia::ricopre(&mut ammessi)
                        })
                    })
            });
            if !vertici_ok {
                return false;
            }
            // Campioni profondi dell'erosione: dal punto medio di ogni lato,
            // verso l'interno (a sinistra negli anelli orientati), a `|d| +
            // 2 tolleranza`; contano quelli dentro la parte e fuori dalla
            // fascia larga.
            parti
                .iter()
                .zip(&fasce)
                .all(|(parte, (indice, _, _, larga))| {
                    parte.iter().all(|polygon| {
                        std::iter::once(polygon.exterior())
                            .chain(polygon.interiors())
                            .all(|ring| {
                                ring.lines().all(|lato| {
                                    let Some((u, _)) = direzione(lato.start, lato.end) else {
                                        return true;
                                    };
                                    let medio = Coord {
                                        x: f64::midpoint(lato.start.x, lato.end.x),
                                        y: f64::midpoint(lato.start.y, lato.end.y),
                                    };
                                    let q =
                                        somma(medio, sinistra(u), 2.0f64.mul_add(tolleranza, d));
                                    !indice.dentro(q) || larga.contiene(q, 0.0) || uscita.dentro(q)
                                })
                            })
                    })
                })
        }
    })?;
    if esito {
        Ok(())
    } else {
        Err(ErroreBuffer::PrecisioneInsufficiente)
    }
}

/// Il buffer di `geometry` a distanza `distance`, entro la precisione
/// (vedi il modulo): freccia degli archi [`freccia_degli_archi`], griglia
/// entro `p / 2`.
///
/// # Errors
///
/// [`ErroreBuffer::PrecisioneInsufficiente`] se la griglia supererebbe la
/// precisione o un controllo a posteriori fallisce;
/// [`ErroreBuffer::CalcoloNonConcluso`] se `geo` va in panico.
pub fn buffer_controllato(
    geometry: &Geometry<f64>,
    distance: f64,
    estremita: Estremita,
    precision: Precision,
) -> Result<MultiPolygon<f64>, ErroreBuffer> {
    buffer_con_freccia(
        geometry,
        distance,
        estremita,
        freccia_degli_archi(distance, precision),
        precision,
    )
}

/// Come [`buffer_controllato`], con la freccia degli archi `freccia`
/// scelta dal chiamante e la griglia entro `precision / 2`: per le
/// operazioni che compongono piu' passaggi nello stesso bilancio.
///
/// # Errors
///
/// Come [`buffer_controllato`].
pub fn buffer_con_freccia(
    geometry: &Geometry<f64>,
    distance: f64,
    estremita: Estremita,
    freccia: f64,
    precision: Precision,
) -> Result<MultiPolygon<f64>, ErroreBuffer> {
    if !distance.is_finite() {
        return Err(ErroreBuffer::PrecisioneInsufficiente);
    }
    if distance == 0.0 {
        let mut parti = Vec::new();
        protetto(|| areali(geometry, &mut parti))?;
        return unione(&parti, precision);
    }
    if nulla_da_bufferizzare(geometry, distance, estremita) {
        return Ok(MultiPolygon::new(Vec::new()));
    }
    let Some(ingombro) = ingombro_allargato(geometry, MARGINE_IN_DISTANZE * distance.abs()) else {
        return Ok(MultiPolygon::new(Vec::new()));
    };
    // Filtro grossolano: una griglia che da sola sposterebbe oltre `p` non
    // si prova nemmeno. La garanzia e' il controllo a posteriori.
    griglia::controlla_griglia(Some(ingombro), precision, FATTORE_BUFFER, precision.value())?;
    let passo = griglia::passo_griglia(ingombro).ok_or(ErroreBuffer::PrecisioneInsufficiente)?;
    let lavoro = senza_componenti_sotto_griglia(geometry, passo);
    let angolo = angolo_degli_archi(distance, freccia);
    let estremita_geo = match estremita {
        Estremita::Tonde => LineCap::Round(angolo),
        Estremita::Piatte => LineCap::Butt,
        Estremita::Quadrate => LineCap::Square,
    };
    let stile = BufferStyle::new(distance)
        .line_join(LineJoin::Round(angolo))
        .line_cap(estremita_geo);
    let risultato = protetto(|| lavoro.buffer_with_style(stile))?;
    verifica_contro_la_definizione(
        geometry, distance, estremita, freccia, &risultato, precision,
    )?;
    Ok(risultato)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::Area;

    fn centimetro() -> Precision {
        Precision::new(0.01).unwrap()
    }

    #[test]
    fn la_freccia_e_mezzo_centimetro_o_lo_zero_virgola_uno_per_cento() {
        assert!((freccia_degli_archi(1.0, centimetro()) - 0.005).abs() < 1e-15);
        assert!((freccia_degli_archi(-5.0, centimetro()) - 0.005).abs() < 1e-15);
        assert!((freccia_degli_archi(100.0, centimetro()) - 0.1).abs() < 1e-15);
        // Il passo chiesto a `i_overlay`, nell'intervallo che accetta, e
        // la freccia del passo effettivo (fino a 1,5 volte) entro la
        // tolleranza per ogni distanza.
        for d in [0.001, 0.1, 1.0, 5.0, 10.0, 100.0, 1000.0, 1e6] {
            let f = freccia_degli_archi(d, centimetro());
            let a = angolo_degli_archi(d, f);
            assert!((ANGOLO_MINIMO..=ANGOLO_MASSIMO).contains(&a), "{d}");
            let freccia_effettiva = d * (1.0 - (0.75 * a).cos());
            assert!(
                freccia_effettiva <= f * (1.0 + 1e-6),
                "{d}: {freccia_effettiva} > {f}"
            );
        }
    }

    /// Il cerchio di un punto: fino a `|d| = 500 p` ogni punto del bordo
    /// sta entro `p` dal cerchio vero; oltre, entro lo 0,1% della distanza
    /// piu' la griglia (deviazione dichiarata).
    #[test]
    fn il_cerchio_di_un_punto_sta_entro_la_freccia_dichiarata() {
        let centro = Coord {
            x: 500_000.0,
            y: 4_000_000.0,
        };
        for (raggio, scarto) in [(4.0, 0.01), (10.0, 0.015), (100.0, 0.105), (1000.0, 1.005)] {
            let buffer = buffer_controllato(
                &Geometry::Point(Point(centro)),
                raggio,
                Estremita::Tonde,
                centimetro(),
            )
            .unwrap();
            let vertici: Vec<Coord<f64>> = buffer.coords_iter().collect();
            for coppia in vertici.windows(2) {
                let medio = Coord {
                    x: f64::midpoint(coppia[0].x, coppia[1].x),
                    y: f64::midpoint(coppia[0].y, coppia[1].y),
                };
                let distanza = (medio.x - centro.x).hypot(medio.y - centro.y);
                assert!((distanza - raggio).abs() <= scarto, "{raggio}: {distanza}");
            }
        }
    }

    /// Revisione: una componente che la griglia ridurrebbe a un punto (0,4
    /// mm accanto a una linea di 1.300 km) ha il suo buffer.
    #[test]
    fn la_componente_minuscola_ha_il_suo_buffer() {
        use geo::Contains as _;
        let linee = Geometry::MultiLineString(MultiLineString::new(vec![
            LineString::from(vec![(0.0, 0.0), (1_300_000.0, 0.0)]),
            LineString::from(vec![(650_000.0, 9.0), (650_000.000_4, 9.0)]),
        ]));
        let buffer = buffer_controllato(&linee, 10.0, Estremita::Tonde, centimetro()).unwrap();
        assert!(buffer.contains(&Point::new(650_000.0, 18.9)));
        assert!(!buffer.contains(&Point::new(650_000.0, 19.1)));
    }

    /// Il buffer negativo di una collezione considera solo le parti areali.
    #[test]
    fn il_buffer_negativo_ignora_punti_e_linee() {
        let collezione = Geometry::GeometryCollection(
            vec![
                Geometry::Polygon(geo::Rect::new((0.0, 0.0), (100.0, 100.0)).to_polygon()),
                Geometry::Point(Point::new(10.0, 10.0)),
                Geometry::LineString(LineString::from(vec![(0.0, 0.0), (5.0, 5.0)])),
            ]
            .into(),
        );
        let buffer =
            buffer_controllato(&collezione, -10.0, Estremita::Tonde, centimetro()).unwrap();
        assert!((buffer.unsigned_area() - 6400.0).abs() < 1e-6);
    }

    #[test]
    fn estremita_come_in_geo() {
        let linea = Geometry::LineString(LineString::from(vec![(0.0, 0.0), (2.0, 0.0)]));
        let area = |estremita| {
            buffer_controllato(&linea, 1.0, estremita, centimetro())
                .unwrap()
                .unsigned_area()
        };
        assert!((area(Estremita::Piatte) - 4.0).abs() < 1e-9);
        assert!((area(Estremita::Quadrate) - 8.0).abs() < 1e-9);
        assert!((area(Estremita::Tonde) - (4.0 + PI)).abs() < 2.0 * PI * 0.005);
        let punto = Geometry::Point(Point::new(0.0, 0.0));
        assert!(
            buffer_controllato(&punto, 1.0, Estremita::Piatte, centimetro())
                .unwrap()
                .0
                .is_empty()
        );
    }

    #[test]
    fn apertura_e_chiusura_entro_la_precisione() {
        let quadrato = Geometry::Polygon(geo::Rect::new((0.0, 0.0), (100.0, 100.0)).to_polygon());
        let erosa = buffer_controllato(&quadrato, -10.0, Estremita::Tonde, centimetro()).unwrap();
        assert!((erosa.unsigned_area() - 6400.0).abs() < 1e-6);
        let quarto = Precision::new(0.0025).unwrap();
        let espansa = buffer_controllato(&quadrato, 10.0, Estremita::Tonde, quarto).unwrap();
        let chiusa = buffer_controllato(
            &Geometry::MultiPolygon(espansa),
            -10.0,
            Estremita::Tonde,
            quarto,
        )
        .unwrap();
        for c in chiusa.coords_iter() {
            let dentro_x = (-0.01..=100.01).contains(&c.x);
            let dentro_y = (-0.01..=100.01).contains(&c.y);
            assert!(dentro_x && dentro_y, "{c:?}");
        }
        assert!((chiusa.unsigned_area() - 10_000.0).abs() < 400.0 * 0.01);
    }

    /// Revisione (Codex, secondo giro): con estremita' piatte l'uscita non
    /// esce dagli estremi, e il controllo contro la definizione rifiuta
    /// un'uscita che sporge.
    #[test]
    fn le_estremita_piatte_restano_agli_estremi() {
        let linea =
            Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]));
        for estremita in [Estremita::Piatte, Estremita::Quadrate] {
            let buffer = buffer_controllato(&linea, 1000.0, estremita, centimetro()).unwrap();
            let (minimo, massimo) = if estremita == Estremita::Piatte {
                (0.0, 2.0)
            } else {
                (-1000.0, 1002.0)
            };
            for c in buffer.coords_iter() {
                assert!(
                    c.x >= minimo - 0.01 && c.x <= massimo + 0.01,
                    "{estremita:?}: {c:?}"
                );
            }
        }
        let rettangolo = |x0: f64, x1: f64| {
            MultiPolygon::new(vec![geo::Rect::new(
                Coord { x: x0, y: -1000.0 },
                Coord { x: x1, y: 1000.0 },
            )
            .to_polygon()])
        };
        assert_eq!(
            verifica_contro_la_definizione(
                &linea,
                1000.0,
                Estremita::Piatte,
                1.0,
                &rettangolo(-3.0, 5.0),
                centimetro()
            ),
            Err(ErroreBuffer::PrecisioneInsufficiente)
        );
        assert!(verifica_contro_la_definizione(
            &linea,
            1000.0,
            Estremita::Piatte,
            1.0,
            &rettangolo(0.0, 2.0),
            centimetro()
        )
        .is_ok());
    }

    /// Revisione (Codex, terzo giro): un lato dell'uscita fuori dal buffer
    /// con i vertici dentro (qui attraverso il vuoto fra due linee) e'
    /// rifiutato; un'uscita vuota o troncata anche.
    #[test]
    fn lati_fuori_e_parti_mancanti_sono_visti() {
        let linee = Geometry::MultiLineString(MultiLineString::new(vec![
            LineString::from(vec![(0.0, 0.0), (10.0, 0.0)]),
            LineString::from(vec![(0.0, 5.0), (10.0, 5.0)]),
        ]));
        let rettangolo = |y0: f64, y1: f64| {
            geo::Rect::new(Coord { x: 0.0, y: y0 }, Coord { x: 10.0, y: y1 }).to_polygon()
        };
        let giusta = MultiPolygon::new(vec![rettangolo(-1.0, 1.0), rettangolo(4.0, 6.0)]);
        let verifica = |uscita: &MultiPolygon<f64>| {
            verifica_contro_la_definizione(
                &linee,
                1.0,
                Estremita::Piatte,
                0.005,
                uscita,
                centimetro(),
            )
        };
        assert!(verifica(&giusta).is_ok());
        // Un solo poligono sui vertici giusti: i lati verticali attraversano
        // il vuoto fra le due fasce.
        let ponte = MultiPolygon::new(vec![rettangolo(-1.0, 6.0)]);
        assert!(verifica(&ponte).is_err());
        assert!(verifica(&MultiPolygon::new(Vec::new())).is_err());
        assert!(verifica(&MultiPolygon::new(vec![rettangolo(-1.0, 1.0)])).is_err());
    }

    /// Revisione (Codex, ultimo giro): l'erosione di parti sovrapposte si
    /// unisce, e un lato dell'uscita ammesso a tratti da parti diverse non
    /// e' rifiutato; un'uscita che sporge da entrambe resta rifiutata.
    #[test]
    fn erosione_di_parti_sovrapposte() {
        let collezione = Geometry::GeometryCollection(
            vec![
                Geometry::Polygon(geo::Rect::new((0.0, 0.0), (10.0, 10.0)).to_polygon()),
                Geometry::Polygon(geo::Rect::new((5.0, 0.0), (15.0, 10.0)).to_polygon()),
            ]
            .into(),
        );
        let buffer = buffer_controllato(&collezione, -1.0, Estremita::Tonde, centimetro()).unwrap();
        assert!((buffer.unsigned_area() - 104.0).abs() < 1e-6);
        let sporgente = MultiPolygon::new(vec![geo::Rect::new(
            Coord { x: 0.5, y: 1.0 },
            Coord { x: 14.0, y: 9.0 },
        )
        .to_polygon()]);
        assert!(verifica_contro_la_definizione(
            &collezione,
            -1.0,
            Estremita::Tonde,
            0.005,
            &sporgente,
            centimetro()
        )
        .is_err());
    }

    #[test]
    fn il_complemento_degli_intervalli() {
        assert_eq!(complemento(&mut []), vec![(0.0, 1.0)]);
        assert_eq!(complemento(&mut [(0.0, 1.0)]), Vec::<(f64, f64)>::new());
        assert_eq!(
            complemento(&mut [(0.2, 0.3), (0.6, 0.7)]),
            vec![(0.0, 0.2), (0.3, 0.6), (0.7, 1.0)]
        );
    }
}
