//! Buffer planare entro la precisione dichiarata: il `Buffer` di `geo`
//! (`i_overlay::mesh`, contorni e tratti) con gli archi scelti dalla
//! precisione e la griglia controllata a priori. Nessun controllo a
//! posteriori contro la definizione esatta: la garanzia poggia sui limiti a
//! priori e sulla correttezza di `i_overlay` (README, «Limiti dichiarati»).
//!
//! **Archi.** `geo` espone `LineJoin::Round(a)` e `LineCap::Round(a)` di
//! `i_overlay` 9.0.0, con `a` il passo angolare richiesto; `i_overlay` lo
//! porta in `[0.01 pi, 0.25 pi]` (`mesh/float/style.rs`), lo converte in
//! unita' angolari intere (`2^32` per giro, al piu' vicino) e costruisce
//! ogni arco (giunzioni ed estremita') con rotazioni intere
//! (`mesh/int/arc`): **ogni intervallo angolare fra due direzioni
//! consecutive e' al piu' `a`**, errori delle rotazioni compresi (in 4.5
//! l'arco era diviso in `round(A / a)` corde e il passo effettivo arrivava
//! a `1.5 a`). Il cerchio di un punto lo costruisce `geo` in `f64` con
//! `ceil(2 pi / a)` corde uguali, anch'esse entro `a`. Una corda che
//! sottende al piu' `a` ha freccia `R (1 - cos(a / 2))`: per una freccia al
//! piu' `f` si chiede `a = 2 acos(1 - f / |d|)`, ridotto di una parte su un
//! milione (piu' dell'arrotondamento all'unita' angolare, `7.3e-10` rad,
//! anche al passo minimo) e portato nello stesso intervallo
//! ([`angolo_degli_archi`]; un passo piu' fine rispetta la freccia a
//! maggior ragione). `f = max(p / 2, 0.001 |d|)` ([`freccia_degli_archi`]):
//! fino a `|d| = 500 p` (5 m con 1 cm) freccia piu' griglia restano entro
//! `p`; oltre, la freccia e' lo 0,1% della distanza, **deviazione
//! dichiarata** del solo buffer (README «Limiti dichiarati»). Con il passo
//! minimo `0.01 pi` la freccia e' al piu' `1.24e-4 |d|`, quindi sempre
//! entro la tolleranza. Il raggio `R` e' la distanza arrotondata sulla
//! griglia (al piu' `g / 2` da `|d|`) e le direzioni sono vettori unitari
//! interi a `2^-62`: lo scarto dall'arco di raggio `|d|` sta nel termine
//! della griglia, sotto. Le giunzioni `Round` non usano la soglia
//! `miter_min_turn` di `i_overlay` 9 (svolte sotto 5 gradi smussate: vale
//! solo per `Miter`): un arco sotto `a` e' gia' la sua corda.
//!
//! **Griglia.** Il buffer passa dalla griglia intera (`i64`) di
//! `i_overlay` piu' volte: vertici arrotondati (`sqrt(2) / 2 g`), distanza
//! arrotondata (`g / 2`), punti degli offset e degli archi arrotondati
//! (`sqrt(2) / 2 g`), direzioni unitarie intere (al piu' `g` sul raggio),
//! overlay dei contorni e finale (incrocio `sqrt(2) / 2 g` e primo aggancio
//! `g` ciascuno), pulizia del risultato (`sqrt(2) / 2 g`): al piu' `(3.5 +
//! 2.5 sqrt(2)) g`, piu' gli arrotondamenti dei `f64` (`12 ulp(M)`, vedi
//! [`super::griglia`]), con `g` il passo sull'ingombro allargato di `3 |d|`
//! (piu' del margine di `i_overlay::mesh` 9, `1.1 (|esterno| + |interno|) =
//! 2.2 |d|` per i contorni e `|d| max(1.1, 2)` per i tratti: un ingombro
//! piu' grande da' un passo uguale o maggiore). Le collezioni (e i punti
//! degeneri delle linee) sono bufferizzate per parti e poi unite: un secondo
//! passaggio in catena. Prima del calcolo i due passaggi devono restare
//! entro `p / 2` ([`griglia::controlla_griglia`]); con `i64` il limite non
//! scatta prima della guardia di spaziatura. Il buffer resta quindi entro
//! `p / 2` dal buffer esatto verso l'esterno ed entro `f + p / 2` verso
//! l'interno lungo gli archi.
//!
//! **Componenti sotto la griglia.** `i_overlay` salta senza errore un
//! anello d'area intera nulla e una linea i cui punti cadono sullo stesso
//! punto della griglia: il loro buffer, spesso `2 |d|`, sparirebbe. Prima
//! del calcolo una linea piu' corta di `2 g` diventa il suo primo punto, un
//! poligono d'area sotto `4 g^2` o piu' sottile di `2 g` il suo anello
//! esterno (e poi, se corto, un punto): lo scarto e' sotto la griglia.

use std::f64::consts::PI;

use geo::algorithm::bool_ops::unary_union;
use geo::algorithm::buffer::{BufferStyle, LineCap, LineJoin};
use geo::orient::{Direction, Orient};
use geo::{
    Area, BoundingRect, Buffer, Coord, CoordsIter, Geometry, LineString, MultiPolygon, Point,
    Polygon,
};

use super::griglia::{self, PrecisioneInsufficiente};
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
    /// La griglia supererebbe la precisione.
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
        2.0 * (1.0 - rapporto).acos() * (1.0 - RIDUZIONE_ANGOLO)
    };
    angolo.clamp(ANGOLO_MINIMO, ANGOLO_MASSIMO)
}

/// La riduzione relativa del passo angolare chiesto: copre l'arrotondamento
/// di `i_overlay` all'unita' angolare intera (`pi / 2^32` rad al piu',
/// `2.3e-8` del passo minimo) e quello di `acos` (vedi il modulo).
const RIDUZIONE_ANGOLO: f64 = 1e-6;

/// Il buffer sposta un punto di al piu' `(3.5 + 2.5 sqrt(2)) g` piu' gli
/// arrotondamenti dei `f64` (vedi il modulo).
const FATTORE_BUFFER: f64 = 3.5 + 2.5 * std::f64::consts::SQRT_2;

/// I passaggi in catena del buffer: il `Buffer` di `geo` e l'unione delle
/// parti (collezioni, punti degeneri delle linee).
const PASSI_BUFFER: u32 = 2;

/// Margine d'ingombro del buffer, in multipli di `|d|`: almeno il margine
/// di `i_overlay::mesh` 9 (contorni `1.1 (|esterno| + |interno|) = 2.2 |d|`,
/// tratti `|d| max(1.1, 2)`), con larghezza per gli arrotondamenti del
/// calcolo del margine (vedi il modulo).
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

/// L'unione delle parti areali (buffer a distanza nulla), con il
/// controllo a priori della griglia.
fn unione(
    operandi: &[MultiPolygon<f64>],
    precision: Precision,
) -> Result<MultiPolygon<f64>, ErroreBuffer> {
    if operandi.is_empty() {
        return Ok(MultiPolygon::new(Vec::new()));
    }
    griglia::controlla_overlay(griglia::rettangolo_multipoligoni(operandi), precision)?;
    protetto(|| unary_union(operandi))
}

/// Il buffer di `geometry` a distanza `distance`, entro la precisione
/// (vedi il modulo): freccia degli archi [`freccia_degli_archi`], griglia
/// entro `p / 2`.
///
/// # Errors
///
/// [`ErroreBuffer::PrecisioneInsufficiente`] se la griglia supererebbe la
/// precisione;
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
    // I due passaggi in catena entro `p / 2` (vedi il modulo).
    griglia::controlla_griglia(
        Some(ingombro),
        precision,
        FATTORE_BUFFER,
        PASSI_BUFFER,
        precision.value() * griglia::FRAZIONE_BORDO,
    )?;
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
    protetto(|| lavoro.buffer_with_style(stile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Area, MultiLineString};

    fn centimetro() -> Precision {
        Precision::new(0.01).unwrap()
    }

    #[test]
    fn la_freccia_e_mezzo_centimetro_o_lo_zero_virgola_uno_per_cento() {
        assert!((freccia_degli_archi(1.0, centimetro()) - 0.005).abs() < 1e-15);
        assert!((freccia_degli_archi(-5.0, centimetro()) - 0.005).abs() < 1e-15);
        assert!((freccia_degli_archi(100.0, centimetro()) - 0.1).abs() < 1e-15);
        // Il passo chiesto a `i_overlay`, nell'intervallo che accetta, e
        // la freccia di una corda che sottende il passo (il massimo che
        // `i_overlay` 9 garantisce) entro la tolleranza per ogni distanza.
        for d in [0.001, 0.1, 1.0, 5.0, 10.0, 100.0, 1000.0, 1e6] {
            let f = freccia_degli_archi(d, centimetro());
            let a = angolo_degli_archi(d, f);
            assert!((ANGOLO_MINIMO..=ANGOLO_MASSIMO).contains(&a), "{d}");
            let freccia_effettiva = d * (1.0 - (0.5 * a).cos());
            assert!(freccia_effettiva <= f, "{d}: {freccia_effettiva} > {f}");
        }
    }

    /// Oracolo della garanzia di `i_overlay` 9 sugli archi, sull'uscita
    /// vera di `geo`: attorno a ogni spigolo di un quadrato (giunzioni) e
    /// attorno agli estremi di una linea (estremita' tonde) ogni coppia di
    /// vertici consecutivi dell'arco sottende al piu' il passo chiesto, e il
    /// punto medio della corda sta entro la freccia (piu' la griglia) dal
    /// cerchio. Con il passo di 4.5 (fino a `1.5 a`) fallirebbe.
    #[test]
    fn gli_archi_di_i_overlay_hanno_il_passo_chiesto() {
        for d in [1.0, 10.0, 100.0, 1000.0] {
            let f = freccia_degli_archi(d, centimetro());
            let a = angolo_degli_archi(d, f);
            let stile = || {
                BufferStyle::new(d)
                    .line_join(LineJoin::Round(a))
                    .line_cap(LineCap::Round(a))
            };
            let lato = 5_000.0;
            let quadrato =
                geo::Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: lato, y: lato }).to_polygon();
            let linea = LineString::from(vec![(0.0, 0.0), (lato, 0.0)]);
            let casi = [
                (
                    quadrato.buffer_with_style(stile()),
                    vec![(0.0, 0.0), (lato, 0.0), (lato, lato), (0.0, lato)],
                    (lato * 0.5, lato * 0.5),
                ),
                (
                    linea.buffer_with_style(stile()),
                    vec![(0.0, 0.0), (lato, 0.0)],
                    (lato * 0.5, 0.0),
                ),
            ];
            for (uscita, centri, (mx, my)) in casi {
                for (cx, cy) in centri {
                    // I vertici dell'arco attorno al centro: a distanza `d`
                    // (entro la griglia). Angoli misurati dalla bisettrice
                    // esterna, cosi' l'arco non attraversa il taglio di
                    // `atan2`.
                    let (bx, by) = (cx - mx, cy - my);
                    let mut angoli: Vec<f64> = uscita
                        .exterior_coords_iter()
                        .filter(|c| ((c.x - cx).hypot(c.y - cy) - d).abs() < 1e-6 * d)
                        .map(|c| {
                            let (vx, vy) = (c.x - cx, c.y - cy);
                            bx.mul_add(vy, -(by * vx)).atan2(bx.mul_add(vx, by * vy))
                        })
                        .collect();
                    angoli.sort_by(f64::total_cmp);
                    angoli.dedup();
                    assert!(angoli.len() >= 3, "{d}: arco di {} vertici", angoli.len());
                    for coppia in angoli.windows(2) {
                        let ampiezza = coppia[1] - coppia[0];
                        assert!(ampiezza <= a * (1.0 + 1e-9), "{d}: {ampiezza} > {a}");
                        let freccia = (-d).mul_add((0.5 * ampiezza).cos(), d);
                        assert!(freccia <= 1e-9f64.mul_add(d, f), "{d}: {freccia} > {f}");
                    }
                }
            }
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
    /// esce dagli estremi.
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
    }

    /// Revisione (Codex, ultimo giro): l'erosione di parti sovrapposte si
    /// unisce (unione delle erosioni, come `geo`).
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
    }
}
