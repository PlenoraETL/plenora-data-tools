//! La precisione dichiarata (1 cm a terra, [`super::precision`]) applicata
//! alle operazioni che passano dalla griglia intera di `i_overlay`: le
//! booleane di `geo` (`BooleanOps`, `unary_union`) e il buffer
//! ([`super::buffer`], costruito con pezzi e un'unione).
//!
//! # La griglia
//!
//! `geo` (vendorizzato, porting a `i_overlay` 9.0.0) chiama ogni overlay e
//! ogni buffer con il motore intero `i64`, e `i_overlay` porta ogni
//! coordinata su interi con `FloatPointAdapter::with_iter_conservative` di
//! `i_float` 5.0.0: sul rettangolo d'ingombro degli operandi di **quella**
//! chiamata, centro `c = (min + max) * 0.5` per asse e raggio `r` la
//! distanza massima di un lato del rettangolo dal centro (in `f64`, con le
//! operazioni di `i_float`), scala `2^(61 - ceil(log2(r)))` (`61 = 64 - 3`
//! bit conservativi), esponente limitato a `1023` per rettangoli minuscoli.
//! Il passo della griglia e' quindi `g = 2^max(ceil(log2(r)) - 61, -1023)`
//! ([`passo_griglia`]). Con `i32` sarebbe `2^(ceil(log2(r)) - 29)`: il
//! motore `i64` e' `2^32` volte piu' fine, sotto la spaziatura dei `f64`
//! delle coordinate stesse (`g <= ulp(r) / 256`).
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
//! - `g * sqrt(2) / 2` per la pulizia del risultato (`clean_result`, attiva
//!   con `i64`: il risultato torna sulla griglia per togliere i vertici
//!   allineati, e poi in `f64`);
//! - gli arrotondamenti dei `f64` nei due passaggi `f64 -> i64 -> f64`
//!   (ingresso e pulizia): per asse la sottrazione del centro (al piu'
//!   `ulp(M)`), la conversione dell'intero in `f64` (al piu' `ulp(M)`) e la
//!   somma del centro (al piu' `ulp(M) / 2`), meno di `3 ulp(M)` a
//!   passaggio e `6 ulp(M)` in tutto: `6 sqrt(2) ulp(M) < 9 ulp(M)` come
//!   vettore.
//!
//! [`controlla_overlay`] rifiuta prima del calcolo se lo spostamento a
//! priori `(1 + 3 sqrt(2) / 2) g + 12 ulp(M)` supera `p / 2`, o se le
//! coordinate sono troppo rade
//! ([`super::precision::coordinate_abbastanza_fitte`], `ulp(M) <= p / 64`).
//! Con la guardia di spaziatura soddisfatta il limite non scatta:
//! `12 ulp(M) <= 0.19 p` e `g < ulp(M) / 64` (`r <= 2 M`). In metri con 1 cm
//! un'estensione di 20.000 km ha `g = 2^-37` m: nessun limite d'estensione,
//! resta il solo modulo delle coordinate (circa `2^39` m).
//!
//! # Overlay in catena
//!
//! Un'operazione che passa il risultato di un overlay a un altro (maschera
//! dissolta e poi intersecata, unione dei vicini e poi differenza, tagli
//! ricorsivi di `subdivide`) somma gli spostamenti:
//! [`controlla_overlay_in_catena`] da' a ognuno degli `n` passi `p / (2 n)`,
//! e la catena resta entro `p / 2`.
//!
//! # Nessun controllo a posteriori
//!
//! Il risultato non e' confrontato con gli ingressi dopo il calcolo: la
//! garanzia di 1 cm poggia sui limiti a priori (griglia, arrotondamenti,
//! catene) e sulla correttezza di `i_overlay`. Due spostamenti non hanno un
//! limite a priori e restano dichiarati (docs/limiti.md, «Limiti dichiarati»): gli
//! agganci dopo il primo giro (raggio `2^(k/2) g` al giro `k`: per arrivare
//! a `p / 2` servono circa `2 log2(p / g)` giri, 26 al limite della guardia
//! di spaziatura e 60 a 20.000 km con 1 cm) e un difetto di `i_overlay`
//! (una faccia persa o in piu'), che nessun controllo vedrebbe. Sotto la
//! precisione restano le differenze dichiarate: parti piu' sottili di `p`
//! fuse o sparite.

use std::f64::consts::SQRT_2;

use geo::{Coord, CoordsIter, MultiPolygon, Rect};

use super::precision::{coordinate_abbastanza_fitte, modulo_massimo, Precision};

/// Lo spostamento che la griglia introdurrebbe supera la precisione: ogni
/// modulo lo traduce nella propria variante `PrecisionInsufficient`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrecisioneInsufficiente;

/// I bit conservativi delle coordinate del motore `i64` in `i_float` 5.0.0
/// (`FloatPointAdapter::CONSERVATIVE_COORDINATE_BITS = I::BITS - 3`): la
/// scala e' `2^(61 - ceil(log2(r)))`.
const BIT_COORDINATE: i32 = 61;

/// L'esponente massimo della scala (`f64::MAX_EXP - 1`): `i_float` limita
/// la scala dei rettangoli minuscoli a `2^1023`.
const ESPONENTE_SCALA_MASSIMO: i32 = 1023;

/// Le unita' in ultima posizione del modulo massimo nello spostamento a
/// priori (vedi il modulo: meno di `9 ulp(M)` per un overlay).
const ULP_A_PRIORI: f64 = 12.0;

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

/// Il passo della griglia di `i_overlay` (motore `i64`) per operandi di
/// ingombro `rect`; `Some(0.0)` se il rettangolo e' un punto (tutte le
/// coordinate coincidono e tornano esatte), `None` se non e' finito.
///
/// Centro e raggio con le stesse operazioni `f64` di
/// `FloatPointAdapter::center_and_radius`: il raggio e' lo stesso numero.
/// `i_float` ricava `ceil(log2(r))` troncando `libm::log2(r)` e
/// confrontando la potenza di due con `r`, che da' il tetto esatto anche
/// quando `log2` sbaglia di un'unita' in ultima posizione vicino a un
/// intero; qui il tetto si legge esatto dai bit ([`tetto_log2`]).
#[must_use]
pub fn passo_griglia(rect: Rect<f64>) -> Option<f64> {
    let (min, max) = (rect.min(), rect.max());
    // Le stesse operazioni di `i_float`, non `f64::midpoint`: il centro deve
    // essere lo stesso numero.
    #[allow(clippy::manual_midpoint)]
    let x = (min.x + max.x) * 0.5;
    #[allow(clippy::manual_midpoint)]
    let y = (min.y + max.y) * 0.5;
    let raggio = (x - min.x).max(max.x - x).max(y - min.y).max(max.y - y);
    if !raggio.is_finite() || raggio < 0.0 {
        return None;
    }
    if raggio == 0.0 {
        return Some(0.0);
    }
    let scala = (BIT_COORDINATE - tetto_log2(raggio)).min(ESPONENTE_SCALA_MASSIMO);
    Some(2_f64.powi(-scala))
}

/// `ceil(log2(x))` esatto per `x` finito e positivo, letto dai bit.
fn tetto_log2(x: f64) -> i32 {
    const MANTISSA: u64 = (1 << 52) - 1;
    let bit = x.to_bits();
    let mantissa = bit & MANTISSA;
    // L'esponente con bias di un positivo finito sta in [0, 2046]: la
    // conversione e' esatta.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let con_bias = (bit >> 52) as i32;
    if con_bias == 0 {
        // Subnormale: x = mantissa * 2^-1074, mantissa in [1, 2^52).
        // `significativi` sta in [1, 52]: la conversione e' esatta.
        #[allow(clippy::cast_possible_wrap)]
        let significativi = (u64::BITS - mantissa.leading_zeros()) as i32;
        return -1074 + significativi - i32::from(mantissa.is_power_of_two());
    }
    // Normale: x = (1 + mantissa / 2^52) * 2^(con_bias - 1023).
    con_bias - 1023 + i32::from(mantissa != 0)
}

/// La spaziatura dei `f64` al modulo `magnitude`.
pub fn ulp(magnitude: f64) -> f64 {
    let magnitude = magnitude.abs();
    f64::from_bits(magnitude.to_bits().saturating_add(1)) - magnitude
}

/// Il primo giro di un overlay booleano, con la pulizia del risultato,
/// sposta un punto di al piu' `(1 + 3 sqrt(2) / 2) g`, piu' gli
/// arrotondamenti dei `f64` (vedi il modulo).
pub const FATTORE_OVERLAY: f64 = 1.0 + 1.5 * SQRT_2;

/// Il limite a priori dello spostamento di un overlay (o di una catena di
/// overlay), in frazioni della precisione: `p / 2`, l'altra meta' per gli
/// agganci successivi al primo giro, che nessun limite a priori copre (vedi
/// il modulo). E' anche la freccia minima degli archi del buffer.
pub const FRAZIONE_BORDO: f64 = 0.5;

/// Lo spostamento a priori della griglia: `fattore * g + 12 ulp(M)`.
#[must_use]
pub fn spostamento_a_priori(rect: Rect<f64>, fattore: f64) -> Option<f64> {
    let g = passo_griglia(rect)?;
    let magnitude = modulo_massimo([rect.min(), rect.max()]);
    let spostamento = fattore.mul_add(g, ULP_A_PRIORI * ulp(magnitude));
    spostamento.is_finite().then_some(spostamento)
}

/// Guardia di spaziatura e spostamento a priori di `passi` giri in catena,
/// ognuno al piu' [`spostamento_a_priori`], entro `limite`.
pub fn controlla_griglia(
    rect: Option<Rect<f64>>,
    precision: Precision,
    fattore: f64,
    passi: u32,
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
        Some(spostamento) if f64::from(passi) * spostamento <= limite => Ok(()),
        _ => Err(PrecisioneInsufficiente),
    }
}

/// Il controllo prima di un overlay booleano i cui operandi hanno ingombro
/// `rect` (`None`: nessuna coordinata, nessun calcolo da controllare):
/// `(1 + 3 sqrt(2) / 2) g + 12 ulp(M) <= p / 2`.
///
/// # Errors
///
/// [`PrecisioneInsufficiente`] se le coordinate sono troppo rade per la
/// precisione, o se lo spostamento a priori della griglia supera `p / 2`.
pub fn controlla_overlay(
    rect: Option<Rect<f64>>,
    precision: Precision,
) -> Result<(), PrecisioneInsufficiente> {
    controlla_overlay_in_catena(rect, precision, 1)
}

/// Il controllo di un overlay che e' uno di `passi` overlay in catena (il
/// risultato di uno e' un operando del successivo): ognuno entro `p / (2
/// passi)`, la catena entro `p / 2` (vedi il modulo). `rect` e' l'ingombro
/// degli operandi di **questo** passo.
///
/// # Errors
///
/// Come [`controlla_overlay`].
pub fn controlla_overlay_in_catena(
    rect: Option<Rect<f64>>,
    precision: Precision,
    passi: u32,
) -> Result<(), PrecisioneInsufficiente> {
    controlla_griglia(
        rect,
        precision,
        FATTORE_OVERLAY,
        passi,
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

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use geo::{LineString, Polygon};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect<f64> {
        Rect::new(Coord { x: x0, y: y0 }, Coord { x: x1, y: y1 })
    }

    /// Il passo riproduce `FloatPointAdapter::with_iter_conservative` di
    /// `i_float` 5.0.0 con il motore `i64`: `2^max(ceil(log2(r)) - 61,
    /// -1023)`, `r` il raggio dal centro arrotondato.
    #[test]
    fn il_passo_e_quello_di_i_float() {
        // r = 1: ceil(log2) = 0, passo 2^-61.
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 2.0, 1.0)),
            Some(2_f64.powi(-61))
        );
        // r = 650 km: log2 = 19.31, ceil 20, passo 2^-41.
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 1_300_000.0, 10.0)),
            Some(2_f64.powi(-41))
        );
        // r = 10.000 km: log2 = 23.25, ceil 24, passo 2^-37.
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 20_000_000.0, 10.0)),
            Some(2_f64.powi(-37))
        );
        // Una potenza di due esatta resta tale; un'unita' in ultima
        // posizione sopra passa all'esponente successivo (centro 1 + 2^-52,
        // raggio 1 + 2^-52).
        assert_eq!(
            passo_griglia(rect(-4.0, -1.0, 4.0, 1.0)),
            Some(2_f64.powi(-59))
        );
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 2.0 + 2_f64.powi(-51), 0.0)),
            Some(2_f64.powi(-60))
        );
        // Rettangolo minuscolo: la scala si ferma a 2^1023.
        assert_eq!(
            passo_griglia(rect(0.0, 0.0, 2_f64.powi(-1000), 0.0)),
            Some(2_f64.powi(-1023))
        );
        assert_eq!(passo_griglia(rect(5.0, 5.0, 5.0, 5.0)), Some(0.0));
        assert_eq!(passo_griglia(rect(0.0, 0.0, f64::INFINITY, 0.0)), None);
    }

    #[test]
    fn il_tetto_del_logaritmo_e_esatto() {
        for (x, atteso) in [
            (1.0, 0),
            (1.5, 1),
            (0.75, 0),
            (0.5, -1),
            (0.5 + f64::EPSILON, 0),
            (1_300_000.0, 21),
            (f64::MAX, 1024),
            (f64::MIN_POSITIVE, -1022),
            (f64::MIN_POSITIVE * 0.75, -1022),
            (5e-324, -1074),
            (3.0 * 5e-324, -1072),
            (4.0 * 5e-324, -1072),
        ] {
            assert_eq!(tetto_log2(x), atteso, "{x:e}");
        }
    }

    /// Controprova diretta sulla dipendenza: un vertice a `1.3 g` dal
    /// centro del rettangolo (l'origine, dove i `f64` sono piu' fitti della
    /// griglia) torna a `1 g`. Con un passo doppio o dimezzato tornerebbe a
    /// `2 g` o a `1.5 g` (`2.6 -> 3` meta' passi).
    #[test]
    fn il_passo_coincide_con_l_adapter_della_dipendenza() {
        use geo::algorithm::bool_ops::BooleanOps as _;
        for lato in [1.0, 1_300_000.0, 20_000_000.0, 0.001_953_125] {
            let g = passo_griglia(rect(-lato, -lato, lato, lato)).unwrap();
            let anello = |x0: f64, y0: f64, x1: f64, y1: f64| {
                Polygon::new(
                    LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
                    vec![],
                )
            };
            let quadrato = anello(-lato, -lato, lato, lato);
            let interno = anello(1.3 * g, lato * -0.5, lato * 0.5, lato * 0.5);
            let uscita = quadrato.difference(&interno);
            assert!(
                uscita.coords_iter().any(|c| (c.x - g).abs() < g * 1e-3),
                "lato {lato}"
            );
        }
    }

    /// Con il motore `i64` il limite a priori non scatta prima della
    /// guardia di spaziatura: 20.000 km passano con meno di un micron di
    /// spostamento a priori, i moduli oltre circa `2^40` m no.
    #[test]
    fn il_controllo_a_priori_segue_l_estensione() {
        let centimetro = Precision::new(0.01).unwrap();
        assert!(
            controlla_overlay(Some(rect(0.0, 0.0, 1_300_000.0, 1_000_000.0)), centimetro).is_ok()
        );
        assert!(controlla_overlay(Some(rect(0.0, 0.0, 2_900_000.0, 10.0)), centimetro).is_ok());
        assert!(controlla_overlay(Some(rect(0.0, 0.0, 5_000_000.0, 10.0)), centimetro).is_ok());
        assert!(controlla_overlay(Some(rect(0.0, 0.0, 20_000_000.0, 10.0)), centimetro).is_ok());
        assert!(
            spostamento_a_priori(rect(0.0, 0.0, 20_000_000.0, 10.0), FATTORE_OVERLAY).unwrap()
                < 1e-6
        );
        assert!(controlla_overlay(Some(rect(0.0, 0.0, 1e12, 10.0)), centimetro).is_ok());
        assert_eq!(
            controlla_overlay(Some(rect(0.0, 0.0, 2e12, 10.0)), centimetro),
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
}
