//! La precisione dichiarata (1 cm a terra, [`super::precision`]) applicata
//! alle operazioni che passano dalla griglia intera di `i_overlay`: le
//! booleane di `geo` (`BooleanOps`, `unary_union`) e il buffer
//! ([`super::buffer`], costruito con pezzi e un'unione).
//!
//! # La griglia
//!
//! `i_overlay` 4.5.2 porta ogni coordinata su interi `i32` con
//! `i_float::adapter::FloatPointAdapter::new` (`i_float` 1.16.0): il
//! rettangolo d'ingombro degli operandi di **quella** chiamata, `h` la meta'
//! della sua dimensione maggiore, scala `2^(29 - round(log2(h)))` (con
//! `round` che arrotonda la meta' lontano da zero, `FloatNumber::to_i32`).
//! Il passo della griglia e' quindi `g = 2^(round(log2(h)) - 29)`
//! ([`passo_griglia`]).
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
//! [`controlla_overlay`] rifiuta prima del calcolo se `(1 + sqrt(2)) g +
//! 4 ulp(M)` supera `p / 2` (la tolleranza del controllo a posteriori,
//! sotto), o se le coordinate sono troppo rade
//! ([`super::precision::coordinate_abbastanza_fitte`]). In metri con 1 cm
//! il passo massimo ammesso e' `2^-9` m: un'estensione fino a circa 2.950
//! km passa (l'Italia, 1.300 km, ha `g = 2^-10`), 20.000 km no (`g = 2^-6`).
//!
//! # Lo spostamento a posteriori: due controlli contro gli ingressi originali
//!
//! Gli agganci successivi al primo hanno un raggio che cresce a ogni giro, e
//! il numero di giri dipende dai dati: nessun limite a priori li copre. Dopo
//! l'overlay, con gli ingressi **originali** dell'operazione pubblica
//! ([`Operandi`]), non i risultati intermedi, cosi' gli overlay in catena
//! non sommano gli spostamenti:
//!
//! - **nessun bordo fuori posto** ([`Operandi::bordo_entro`]): il bordo
//!   esatto di un'intersezione, unione, differenza o differenza simmetrica
//!   di poligoni validi sta sui bordi degli operandi. Ogni lato del risultato
//!   deve stare **per intero** entro `p / 2` da quei bordi. La tolleranza e'
//!   `p / 2`, non `p`: un aggancio puo' portare un vertice da un bordo a un
//!   altro bordo d'ingresso vicino, e i lati che vi arrivano stanno allora
//!   entro `delta / 2` da uno dei due; con `p / 2` passa solo `delta <= p`.
//!   L'insieme dei punti entro `r` da un segmento e' convesso (uno
//!   «stadio»), la sua traccia su un lato e' un intervallo, e gli intervalli
//!   dei segmenti vicini devono ricoprire il lato;
//! - **nessun bordo mancante** ([`Operandi::completo`]): ogni tratto di
//!   bordo d'ingresso a piu' di `p / 2` dai bordi degli altri operandi sta
//!   tutto dentro o tutto fuori da ciascuno di essi, e la regola
//!   dell'operazione (unione, intersezione, differenza...) dice, da chi lo
//!   contiene (`Contains` esatto su un punto del tratto), se appartiene al
//!   bordo del risultato esatto. Se si', deve stare entro `p / 2` dal bordo
//!   del risultato. Una faccia omessa o cancellata piu' larga di `p` lascia
//!   scoperto il suo bordo: errore.
//!
//! Sotto la precisione restano le differenze dichiarate: parti piu' sottili
//! di `p` fuse o sparite.

use std::f64::consts::SQRT_2;

use geo::{
    BoundingRect, Contains, Coord, CoordsIter, Line, LineString, MultiPolygon, Point, Polygon, Rect,
};
use rstar::{RTree, RTreeObject, AABB};

use super::precision::{coordinate_abbastanza_fitte, modulo_massimo, Precision};

/// Lo spostamento che la griglia introdurrebbe supera la precisione: ogni
/// modulo lo traduce nella propria variante `PrecisionInsufficient`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrecisioneInsufficiente;

/// Esponente di `i_float`: la scala e' `2^(29 - round(log2(h)))`.
const ESPONENTE_GRIGLIA: i32 = 29;

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
    /// L'indice della linework dei multipoligoni, ognuno con la sua
    /// etichetta.
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
                    if !self.lato_entro(lato, extra, &filtro, r, &mut intervalli) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Il lato sta per intero entro `r` dai segmenti dell'indice la cui
    /// etichetta passa `filtro`, o da uno di `extra`?
    fn lato_entro(
        &self,
        lato: Line<f64>,
        extra: &[Line<f64>],
        filtro: &impl Fn(usize) -> bool,
        r: f64,
        intervalli: &mut Vec<(f64, f64)>,
    ) -> bool {
        intervalli.clear();
        let candidati = self
            .albero
            .locate_in_envelope_intersecting(&busta(lato, r))
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
        ricopre(intervalli)
    }
}

/// Il rettangolo del lato allargato di `r`.
fn busta(lato: Line<f64>, r: f64) -> AABB<[f64; 2]> {
    AABB::from_corners(
        [
            lato.start.x.min(lato.end.x) - r,
            lato.start.y.min(lato.end.y) - r,
        ],
        [
            lato.start.x.max(lato.end.x) + r,
            lato.start.y.max(lato.end.y) + r,
        ],
    )
}

/// L'ingombro di un operando, per trovare chi contiene un punto.
struct Ingombro {
    etichetta: usize,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for Ingombro {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// Gli ingressi originali di un'operazione poligonale, ognuno con la sua
/// etichetta (la posizione), per i due controlli a posteriori (vedi il
/// modulo).
pub struct Operandi<'a> {
    poligoni: Vec<&'a MultiPolygon<f64>>,
    bordi: IndiceLinework,
    ingombri: RTree<Ingombro>,
}

impl<'a> Operandi<'a> {
    /// Gli operandi, etichettati con la loro posizione.
    #[must_use]
    pub fn nuovi(poligoni: Vec<&'a MultiPolygon<f64>>) -> Self {
        let bordi = IndiceLinework::da_multipoligoni(poligoni.iter().copied().enumerate());
        let ingombri = RTree::bulk_load(
            poligoni
                .iter()
                .enumerate()
                .filter_map(|(etichetta, polygons)| {
                    polygons.bounding_rect().map(|rect| Ingombro {
                        etichetta,
                        envelope: AABB::from_corners(
                            [rect.min().x, rect.min().y],
                            [rect.max().x, rect.max().y],
                        ),
                    })
                })
                .collect(),
        );
        Self {
            poligoni,
            bordi,
            ingombri,
        }
    }

    /// Le etichette degli operandi il cui ingombro interseca `rect`, in
    /// ordine.
    #[must_use]
    pub fn vicini(&self, rect: Rect<f64>) -> Vec<usize> {
        let mut vicini: Vec<usize> = self
            .ingombri
            .locate_in_envelope_intersecting(&AABB::from_corners(
                [rect.min().x, rect.min().y],
                [rect.max().x, rect.max().y],
            ))
            .map(|ingombro| ingombro.etichetta)
            .collect();
        vicini.sort_unstable();
        vicini
    }

    /// Il primo controllo (vedi il modulo): ogni lato di `output` entro `p /
    /// 2` dai bordi degli operandi accettati da `rilevante`.
    pub fn bordo_entro(
        &self,
        output: &MultiPolygon<f64>,
        rilevante: impl Fn(usize) -> bool,
        precision: Precision,
    ) -> bool {
        self.bordi.bordo_entro(output, &[], rilevante, precision)
    }

    /// Il secondo controllo (vedi il modulo): ogni tratto di bordo degli
    /// operandi accettati da `rilevante` (dentro `regione`, se c'e'), a piu'
    /// di `p / 2` dai bordi degli altri, che secondo `regola` sta sul bordo
    /// del risultato esatto, e' entro `p / 2` dal bordo di `output`.
    pub fn completo(
        &self,
        output: &MultiPolygon<f64>,
        rilevante: impl Fn(usize) -> bool,
        regione: Option<Rect<f64>>,
        regola: Regola,
        precision: Precision,
    ) -> bool {
        let r = precision.value() * FRAZIONE_BORDO;
        let uscita = IndiceLinework::da_multipoligoni([(0, output)]);
        let segmenti: Vec<Segmento> = regione.map_or_else(
            || self.bordi.albero.iter().copied().collect(),
            |rect| {
                self.bordi
                    .albero
                    .locate_in_envelope_intersecting(&busta(Line::new(rect.min(), rect.max()), r))
                    .copied()
                    .collect()
            },
        );
        let mut vicini = Vec::new();
        let mut coperti = Vec::new();
        for segmento in segmenti
            .iter()
            .filter(|segmento| rilevante(segmento.etichetta))
        {
            let lato = segmento.linea;
            vicini.clear();
            for altro in self
                .bordi
                .albero
                .locate_in_envelope_intersecting(&busta(lato, r))
            {
                if altro.etichetta != segmento.etichetta && rilevante(altro.etichetta) {
                    if let Some(intervallo) = intervallo_entro(
                        lato.start,
                        lato.end,
                        altro.linea.start,
                        altro.linea.end,
                        r,
                    ) {
                        vicini.push(intervallo);
                    }
                }
            }
            for (t0, t1) in liberi(&mut vicini) {
                let medio = punto_a(lato, f64::midpoint(t0, t1));
                let proprietario = segmento.etichetta;
                let contiene = |etichetta: usize| {
                    self.poligoni
                        .get(etichetta)
                        .is_some_and(|polygons| polygons.contains(&Point::from(medio)))
                };
                // Un altro operando rilevante, diverso da `escluso` e dal
                // proprietario, contiene il punto? (si ferma al primo)
                let altro = |escluso: usize| {
                    self.ingombri
                        .locate_in_envelope_intersecting(&AABB::from_point([medio.x, medio.y]))
                        .any(|ingombro| {
                            let etichetta = ingombro.etichetta;
                            etichetta != proprietario
                                && etichetta != escluso
                                && rilevante(etichetta)
                                && contiene(etichetta)
                        })
                };
                let atteso = match regola {
                    Regola::Unione => !altro(proprietario),
                    Regola::Intersezione => altro(proprietario),
                    Regola::DifferenzaSimmetrica => true,
                    Regola::Differenza(soggetto) if proprietario == soggetto => !altro(soggetto),
                    Regola::IntersezioneConUnione(soggetto) if proprietario == soggetto => {
                        altro(soggetto)
                    }
                    Regola::Differenza(soggetto) | Regola::IntersezioneConUnione(soggetto) => {
                        rilevante(soggetto) && contiene(soggetto) && !altro(soggetto)
                    }
                };
                if atteso {
                    let tratto = Line::new(punto_a(lato, t0), punto_a(lato, t1));
                    if !uscita.lato_entro(tratto, &[], &|_| true, r, &mut coperti) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// I due controlli insieme.
    ///
    /// # Errors
    ///
    /// [`PrecisioneInsufficiente`] se uno dei due fallisce.
    pub fn verifica(
        &self,
        output: &MultiPolygon<f64>,
        rilevante: impl Fn(usize) -> bool,
        regione: Option<Rect<f64>>,
        regola: Regola,
        precision: Precision,
    ) -> Result<(), PrecisioneInsufficiente> {
        if self.bordo_entro(output, &rilevante, precision)
            && self.completo(output, &rilevante, regione, regola, precision)
        {
            Ok(())
        } else {
            Err(PrecisioneInsufficiente)
        }
    }
}

/// La regola dell'operazione per [`Operandi::completo`]: quando un tratto
/// di bordo di un operando (il proprietario) sta sul bordo del risultato
/// esatto, secondo quali altri operandi rilevanti lo contengono.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Regola {
    /// Unione di tutti: nessun altro lo contiene.
    Unione,
    /// Intersezione di due: l'altro lo contiene.
    Intersezione,
    /// Differenza simmetrica di due: sempre.
    DifferenzaSimmetrica,
    /// `soggetto` meno l'unione degli altri: il bordo del soggetto che
    /// nessun altro contiene; il bordo di un altro dentro il soggetto e
    /// dentro nessun terzo.
    Differenza(usize),
    /// `soggetto` intersecato con l'unione degli altri: il bordo del
    /// soggetto dentro almeno un altro; il bordo di un altro dentro il
    /// soggetto e dentro nessun terzo.
    IntersezioneConUnione(usize),
}

/// Il punto del lato al parametro `t`.
fn punto_a(lato: Line<f64>, t: f64) -> Coord<f64> {
    Coord {
        x: t.mul_add(lato.end.x - lato.start.x, lato.start.x),
        y: t.mul_add(lato.end.y - lato.start.y, lato.start.y),
    }
}

/// I tratti di `[0, 1]` non coperti dagli intervalli, di lunghezza positiva.
fn liberi(intervalli: &mut [(f64, f64)]) -> Vec<(f64, f64)> {
    intervalli.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
    let mut liberi = Vec::new();
    let mut coperto = 0.0_f64;
    for &(t0, t1) in intervalli.iter() {
        if t0 > coperto {
            liberi.push((coperto, t0.min(1.0)));
        }
        coperto = coperto.max(t1);
        if coperto >= 1.0 {
            return liberi;
        }
    }
    liberi.push((coperto, 1.0));
    liberi.retain(|(t0, t1)| t1 > t0);
    liberi
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
        let ingresso = MultiPolygon::new(vec![polygon![
            (x: 0.0, y: 0.0), (x: 100.0, y: 0.0), (x: 100.0, y: 100.0), (x: 0.0, y: 100.0)
        ]]);
        let indice = IndiceLinework::da_multipoligoni([(0, &ingresso)]);
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

    /// Revisione: un vertice agganciato a un bordo d'ingresso parallelo a
    /// 1,8 cm stava entro 0,9 cm da uno dei due bordi lungo i lati obliqui, e
    /// passava con tolleranza `p`. Con `p / 2` no; a 0,8 cm si'.
    #[test]
    fn il_vertice_agganciato_a_un_altro_bordo_e_visto() {
        let centimetro = Precision::new(0.01).unwrap();
        let sotto = MultiPolygon::new(vec![polygon![
            (x: 0.0, y: 0.0), (x: 200.0, y: 0.0), (x: 200.0, y: 50.0), (x: 0.0, y: 50.0)
        ]]);
        let spostato = |dy: f64| {
            let sopra = MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 50.0 + dy), (x: 200.0, y: 50.0 + dy),
                (x: 200.0, y: 100.0), (x: 0.0, y: 100.0)
            ]]);
            let indice = IndiceLinework::da_multipoligoni([(0, &sotto), (1, &sopra)]);
            let uscita = MultiPolygon::new(vec![polygon![
                (x: 0.0, y: 0.0), (x: 200.0, y: 0.0), (x: 200.0, y: 50.0),
                (x: 100.0, y: 50.0 + dy), (x: 0.0, y: 50.0)
            ]]);
            indice.bordo_entro(&uscita, &[], |_| true, centimetro)
        };
        assert!(!spostato(0.018));
        assert!(spostato(0.008));
    }

    /// Il secondo controllo: una faccia cancellata dal risultato lascia
    /// scoperto il suo bordo; la stessa faccia contenuta da un altro
    /// operando no.
    #[test]
    fn il_bordo_mancante_e_visto() {
        let centimetro = Precision::new(0.01).unwrap();
        let grande = MultiPolygon::new(vec![polygon![
            (x: 0.0, y: 0.0), (x: 100.0, y: 0.0), (x: 100.0, y: 100.0), (x: 0.0, y: 100.0)
        ]]);
        let staccato = MultiPolygon::new(vec![polygon![
            (x: 200.0, y: 0.0), (x: 201.0, y: 0.0), (x: 201.0, y: 1.0), (x: 200.0, y: 1.0)
        ]]);
        let dentro = MultiPolygon::new(vec![polygon![
            (x: 10.0, y: 10.0), (x: 20.0, y: 10.0), (x: 20.0, y: 20.0), (x: 10.0, y: 20.0)
        ]]);
        let operandi = Operandi::nuovi(vec![&grande, &staccato, &dentro]);
        // Unione esatta: il grande e lo staccato; il bordo del terzo e'
        // dentro il primo.
        let esatta = MultiPolygon::new(vec![grande.0[0].clone(), staccato.0[0].clone()]);
        assert!(operandi.completo(&esatta, |_| true, None, Regola::Unione, centimetro));
        assert!(operandi.bordo_entro(&esatta, |_| true, centimetro));
        // Senza lo staccato: il suo bordo manca.
        assert!(!operandi.completo(&grande, |_| true, None, Regola::Unione, centimetro));
        // Differenza grande - dentro: il bordo del terzo deve esserci.
        let rilevante = |e: usize| e != 1;
        assert!(!operandi.completo(&grande, rilevante, None, Regola::Differenza(0), centimetro));
        let bucato = MultiPolygon::new(vec![Polygon::new(
            grande.0[0].exterior().clone(),
            vec![dentro.0[0].exterior().clone()],
        )]);
        assert!(operandi.completo(&bucato, rilevante, None, Regola::Differenza(0), centimetro));
    }

    #[test]
    fn i_tratti_liberi_sono_il_complemento() {
        assert_eq!(liberi(&mut []), vec![(0.0, 1.0)]);
        assert_eq!(liberi(&mut [(-1.0, 2.0)]), Vec::<(f64, f64)>::new());
        assert_eq!(
            liberi(&mut [(0.5, 0.6), (-0.1, 0.2)]),
            vec![(0.2, 0.5), (0.6, 1.0)]
        );
    }
}
