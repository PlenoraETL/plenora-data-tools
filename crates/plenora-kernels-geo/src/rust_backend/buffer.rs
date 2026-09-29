//! Buffer planare entro la precisione dichiarata, costruito per pezzi.
//!
//! Il `Buffer` di `geo` (`i_overlay::mesh`) approssima gli archi con un
//! passo angolare fisso (0,2 rad: una freccia fino all'1,1% della distanza,
//! 4,8 cm per il cerchio di 10 m di un punto) e passa dalla griglia di
//! `i_overlay` piu' volte, saltando senza errore le parti che la griglia
//! riduce a un punto. Qui il buffer e' la definizione di Minkowski scritta
//! per pezzi, e il solo calcolo approssimato e' un'unione controllata:
//!
//! - per `d > 0` l'unione di: le parti areali dell'ingresso; per ogni lato
//!   di una linea il rettangolo largo `2d` attorno al lato (allungato di `d`
//!   all'estremo libero con estremita' quadrate), per ogni lato di un
//!   anello la sola meta' esterna (quella interna sta nel poligono, e un
//!   punto esterno ha il punto piu' vicino dal lato esterno); per ogni vertice il
//!   settore di raggio `d` del suo cono normale, sul lato convesso della
//!   svolta (un punto entro `d` dalla spezzata ha un punto piu' vicino,
//!   interno a un lato o vertice: sta nel rettangolo o nel settore); il
//!   mezzo disco agli estremi liberi con estremita' tonde; per i punti il
//!   disco (tonde) o il quadrato di lato `2d` (quadrate), niente con
//!   estremita' piatte, come `geo`;
//! - per `d < 0` ogni parte areale meno l'unione dei pezzi interni dei suoi
//!   anelli con raggio `|d|` (l'erosione e' la parte a distanza almeno
//!   `|d|` dal bordo); punti e linee non contribuiscono, come in `geo`;
//! - per `d = 0` l'unione delle parti areali.
//!
//! **Archi.** Dischi e settori sono poligoni inscritti con freccia al piu'
//! `f = max(p / 2, 0.001 |d|)` ([`freccia_degli_archi`]): `n = ceil(2 pi /
//! (2 acos(1 - f / |d|)))` lati per il cerchio intero
//! ([`lati_per_cerchio`]). Fino a `|d| = 500 p` (5 m con 1 cm) la freccia
//! e' `p / 2` e freccia piu' griglia restano entro `p`; oltre, la freccia e'
//! lo 0,1% della distanza, **deviazione dichiarata** del solo buffer
//! (decisione dell'utente, README «Limiti dichiarati»): gli archi del
//! buffer si scostano dal cerchio esatto fino a `0.001 |d|` (10 cm a 100
//! m), sempre verso l'interno (poligoni inscritti), senza errore. La
//! griglia e i controlli dell'unione contro i pezzi restano a `p / 2`:
//! la deviazione riguarda solo la discretizzazione degli archi. Oltre [`MAX_LATI_CERCHIO`] lati per cerchio, o
//! [`MAX_VERTICI_BUFFER`] vertici in tutto, errore esplicito.

use std::f64::consts::{PI, TAU};

use geo::algorithm::bool_ops::unary_union;
use geo::orient::{Direction, Orient};
use geo::{BooleanOps, BoundingRect, Coord, Geometry, LineString, MultiPolygon, Polygon};

use super::griglia::{self, ErroreVerifica, Operandi, PrecisioneInsufficiente, Regola};
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
    /// La griglia o gli archi supererebbero la precisione, o un controllo a
    /// posteriori ha fallito.
    PrecisioneInsufficiente,
    /// I pezzi supererebbero [`MAX_VERTICI_BUFFER`] vertici.
    TroppiVertici { actual: u64, limit: u64 },
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

/// Lati massimi di un cerchio: oltre, la precisione non e' raggiungibile
/// per quella distanza (circa `|d| = 3,5e7 p`).
pub const MAX_LATI_CERCHIO: u64 = 1 << 20;

/// La freccia relativa massima ammessa per gli archi del buffer: lo 0,1%
/// della distanza (deviazione dichiarata, vedi il modulo).
pub const FRECCIA_RELATIVA_MASSIMA: f64 = 0.001;

/// La freccia degli archi di un buffer di distanza `distance` con
/// precisione `precision`: `max(p / 2, 0.001 |d|)` (vedi il modulo).
#[must_use]
pub fn freccia_degli_archi(distance: f64, precision: Precision) -> f64 {
    (precision.value() * griglia::FRAZIONE_BORDO).max(FRECCIA_RELATIVA_MASSIMA * distance.abs())
}

/// Vertici massimi dei pezzi di un buffer.
///
/// Tutti i pezzi stanno nel buffer esatto, salvo la sovrapposizione ai
/// giunti (vedi `Pezzi::nuovi`), oltre `|d|` di un centesimo della freccia.
pub const MAX_VERTICI_BUFFER: u64 = 50_000_000;

/// Il numero di lati del poligono inscritto nel cerchio di raggio `raggio`
/// con freccia al piu' `freccia`: `None` se oltre [`MAX_LATI_CERCHIO`] o
/// se i valori non sono finiti e positivi. Almeno 4.
#[must_use]
pub fn lati_per_cerchio(raggio: f64, freccia: f64) -> Option<u64> {
    if !(raggio.is_finite() && freccia.is_finite() && raggio > 0.0 && freccia > 0.0) {
        return None;
    }
    let rapporto = freccia / raggio;
    if rapporto >= 1.0 {
        return Some(4);
    }
    // Passo con freccia esatta `freccia`, ristretto di un margine relativo
    // per l'arrotondamento di `acos`.
    let passo = 2.0 * (1.0 - rapporto).acos() * (1.0 - 1e-9);
    let lati = (TAU / passo).ceil();
    #[allow(clippy::cast_precision_loss)]
    let massimo = MAX_LATI_CERCHIO as f64;
    if !(lati.is_finite() && lati <= massimo) {
        return None;
    }
    // `lati` e' un intero in [1, 2^20]: la conversione e' esatta.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lati = lati as u64;
    Some(lati.max(4))
}

/// Da quale lato di una spezzata servono i pezzi. Gli anelli sono
/// orientati (esterno antiorario, buchi orari): l'esterno del poligono e'
/// sempre a destra.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lato {
    Entrambi,
    /// L'esterno di un anello: il buffer positivo di un poligono (l'interno
    /// e' il poligono stesso).
    Destra,
    /// L'interno di un anello: la fascia dell'erosione.
    Sinistra,
}

/// Costruttore dei pezzi di raggio `raggio`.
struct Pezzi {
    raggio: f64,
    giunto: f64,
    lato: Lato,
    /// Passo angolare massimo degli archi.
    passo: f64,
    lati: u64,
    prodotti: Vec<MultiPolygon<f64>>,
    vertici: u64,
}

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

/// Il vettore opposto.
const fn opposto(u: Coord<f64>) -> Coord<f64> {
    Coord { x: -u.x, y: -u.y }
}

/// La normale sinistra.
const fn sinistra(u: Coord<f64>) -> Coord<f64> {
    Coord { x: -u.y, y: u.x }
}

impl Pezzi {
    fn nuovi(raggio: f64, freccia: f64) -> Result<Self, ErroreBuffer> {
        // I rettangoli di due lati consecutivi si sovrappongono di `giunto`
        // invece di toccarsi lungo un lato: un contatto quasi collineare
        // farebbe agganciare `i_overlay` a raggi crescenti e lascerebbe
        // schegge. La parte in piu' dista dal vertice al piu' `sqrt(r^2 +
        // giunto^2)`, cioe' oltre `r` di `giunto^2 / (2r)`: un centesimo
        // della freccia.
        let giunto = (2.0 * raggio * freccia * 0.01).sqrt();
        let lati =
            lati_per_cerchio(raggio, freccia).ok_or(ErroreBuffer::PrecisioneInsufficiente)?;
        #[allow(clippy::cast_precision_loss)]
        let passo = TAU / lati as f64;
        Ok(Self {
            raggio,
            giunto,
            lato: Lato::Entrambi,
            passo,
            lati,
            prodotti: Vec::new(),
            vertici: 0,
        })
    }

    fn aggiungi(&mut self, anello: Vec<Coord<f64>>) -> Result<(), ErroreBuffer> {
        let vertici = u64::try_from(anello.len()).unwrap_or(u64::MAX);
        self.vertici = self.vertici.saturating_add(vertici);
        if self.vertici > MAX_VERTICI_BUFFER {
            return Err(ErroreBuffer::TroppiVertici {
                actual: self.vertici,
                limit: MAX_VERTICI_BUFFER,
            });
        }
        let polygon = Polygon::new(LineString::new(anello), Vec::new()).orient(Direction::Default);
        self.prodotti.push(MultiPolygon::new(vec![polygon]));
        Ok(())
    }

    fn disco(&mut self, centro: Coord<f64>) -> Result<(), ErroreBuffer> {
        let lati = self.lati;
        let mut anello = Vec::with_capacity(usize::try_from(lati).unwrap_or(0) + 1);
        for indice in 0..lati {
            #[allow(clippy::cast_precision_loss)]
            let angolo = TAU * indice as f64 / lati as f64;
            anello.push(Coord {
                x: self.raggio.mul_add(angolo.cos(), centro.x),
                y: self.raggio.mul_add(angolo.sin(), centro.y),
            });
        }
        self.aggiungi(anello)
    }

    fn quadrato(&mut self, centro: Coord<f64>) -> Result<(), ErroreBuffer> {
        let r = self.raggio;
        self.aggiungi(vec![
            Coord {
                x: centro.x - r,
                y: centro.y - r,
            },
            Coord {
                x: centro.x + r,
                y: centro.y - r,
            },
            Coord {
                x: centro.x + r,
                y: centro.y + r,
            },
            Coord {
                x: centro.x - r,
                y: centro.y + r,
            },
        ])
    }

    /// Il rettangolo attorno al lato `a -> b` (direzione `u`), allungato
    /// delle lunghezze date all'inizio e alla fine.
    fn rettangolo(
        &mut self,
        da: Coord<f64>,
        verso: Coord<f64>,
        u: Coord<f64>,
        allunga_inizio: f64,
        allunga_fine: f64,
    ) -> Result<(), ErroreBuffer> {
        let raggio = self.raggio;
        let inizio = somma(da, u, -allunga_inizio);
        let fine = somma(verso, u, allunga_fine);
        let normale = sinistra(u);
        match self.lato {
            Lato::Entrambi => self.aggiungi(vec![
                somma(inizio, normale, -raggio),
                somma(fine, normale, -raggio),
                somma(fine, normale, raggio),
                somma(inizio, normale, raggio),
            ]),
            // Le meta' entrano di `giunto` dall'altra parte del lato: il loro
            // bordo interno attraversa gli altri pezzi invece di coincidere
            // con il lato dell'anello (schegge, vedi `giunto`); la parte in
            // piu' sta entro `giunto < r` dal lato, quindi nel buffer (o,
            // per l'erosione, fuori dal poligono).
            Lato::Destra => self.aggiungi(vec![
                somma(inizio, normale, self.giunto),
                somma(inizio, normale, -raggio),
                somma(fine, normale, -raggio),
                somma(fine, normale, self.giunto),
            ]),
            Lato::Sinistra => self.aggiungi(vec![
                somma(inizio, normale, -self.giunto),
                somma(fine, normale, -self.giunto),
                somma(fine, normale, raggio),
                somma(inizio, normale, raggio),
            ]),
        }
    }

    /// Il settore di centro `v` fra i punti `v + r n0` e `v + r n1`,
    /// ruotando di `delta` radianti (con segno) da `n0`. Gli estremi
    /// dell'arco sono gli stessi `f64` degli angoli dei rettangoli.
    fn settore(
        &mut self,
        v: Coord<f64>,
        n0: Coord<f64>,
        n1: Coord<f64>,
        delta: f64,
    ) -> Result<(), ErroreBuffer> {
        let r = self.raggio;
        let passi = (delta.abs() / self.passo).ceil().max(1.0);
        let inizio = n0.y.atan2(n0.x);
        // Il vertice interno del settore e' arretrato di `r / 2` dalla parte
        // opposta al cono: i lati del settore attraversano i rettangoli
        // invece di coincidere con i loro lati (vedi `giunto`). Il poligono
        // resta nel disco di centro `v`, quindi nel buffer.
        let medio = 0.5_f64.mul_add(delta, inizio);
        let arretrato = Coord {
            x: (-0.5 * r).mul_add(medio.cos(), v.x),
            y: (-0.5 * r).mul_add(medio.sin(), v.y),
        };
        let mut anello = vec![arretrato, somma(v, n0, r)];
        // `passi` e' un intero piccolo e positivo: conversione esatta.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let conteggio = passi as u64;
        for indice in 1..conteggio {
            #[allow(clippy::cast_precision_loss)]
            let angolo = delta.mul_add(indice as f64 / passi, inizio);
            anello.push(Coord {
                x: r.mul_add(angolo.cos(), v.x),
                y: r.mul_add(angolo.sin(), v.y),
            });
        }
        anello.push(somma(v, n1, r));
        self.aggiungi(anello)
    }

    /// Il pezzo di vertice fra un lato entrante (direzione `u`) e uno
    /// uscente (`w`): il settore del cono normale, cioe' delle direzioni `e`
    /// con `e . u >= 0` e `e . w <= 0`, sul lato convesso della svolta.
    ///
    /// Basta: un punto a distanza al piu' `r` dalla spezzata ha un punto
    /// piu' vicino; se e' interno a un lato il punto sta nel rettangolo del
    /// lato, se e' un vertice sta nel cono normale di quel vertice. Nessun
    /// disco intero, qualunque sia la lunghezza dei lati.
    fn vertice(&mut self, v: Coord<f64>, u: Coord<f64>, w: Coord<f64>) -> Result<(), ErroreBuffer> {
        let prodotto = u.x.mul_add(w.y, -(u.y * w.x));
        let scalare = u.x.mul_add(w.x, u.y * w.y);
        let (n0, n1) = (sinistra(u), sinistra(w));
        if prodotto == 0.0 {
            if scalare > 0.0 {
                // Lati allineati nello stesso verso: i rettangoli combaciano.
                return Ok(());
            }
            // Inversione: il mezzo disco davanti a `u`.
            return self.settore(v, n0, opposto(n0), -PI);
        }
        let delta = prodotto.atan2(scalare);
        if delta > 0.0 {
            // Svolta a sinistra: il cono e' a destra.
            if self.lato == Lato::Sinistra {
                return Ok(());
            }
            self.settore(v, opposto(n0), opposto(n1), delta)
        } else {
            if self.lato == Lato::Destra {
                return Ok(());
            }
            self.settore(v, n0, n1, delta)
        }
    }

    /// Una spezzata (anello chiuso o linea aperta).
    fn spezzata(
        &mut self,
        coordinate: &[Coord<f64>],
        chiusa: bool,
        estremita: Estremita,
    ) -> Result<(), ErroreBuffer> {
        let mut punti: Vec<Coord<f64>> = Vec::with_capacity(coordinate.len());
        for c in coordinate {
            if punti.last() != Some(c) {
                punti.push(*c);
            }
        }
        if chiusa && punti.len() > 1 && punti.first() == punti.last() {
            punti.pop();
        }
        match punti.len() {
            0 => return Ok(()),
            1 => return self.punto(punti[0], estremita),
            _ => {}
        }
        let lati = if chiusa { punti.len() } else { punti.len() - 1 };
        let mut direzioni = Vec::with_capacity(lati);
        for indice in 0..lati {
            let a = punti[indice];
            let b = punti[(indice + 1) % punti.len()];
            let (u, _) = direzione(a, b).ok_or(ErroreBuffer::PrecisioneInsufficiente)?;
            direzioni.push(u);
        }
        let quadrate = !chiusa && estremita == Estremita::Quadrate;
        // Agli estremi liberi: `r` con estremita' quadrate, niente
        // altrimenti; ai giunti la sovrapposizione `giunto`.
        let libero = if quadrate { self.raggio } else { 0.0 };
        for (indice, &u) in direzioni.iter().enumerate() {
            let a = punti[indice];
            let b = punti[(indice + 1) % punti.len()];
            let inizio = if chiusa || indice > 0 {
                self.giunto
            } else {
                libero
            };
            let fine = if chiusa || indice + 1 < lati {
                self.giunto
            } else {
                libero
            };
            self.rettangolo(a, b, u, inizio, fine)?;
        }
        if chiusa {
            for indice in 0..lati {
                let entrante = direzioni[(indice + lati - 1) % lati];
                self.vertice(punti[indice], entrante, direzioni[indice])?;
            }
        } else {
            for indice in 1..lati {
                self.vertice(punti[indice], direzioni[indice - 1], direzioni[indice])?;
            }
            if estremita == Estremita::Tonde {
                // I mezzi dischi dietro l'inizio e davanti alla fine.
                let n = sinistra(direzioni[0]);
                self.settore(punti[0], n, opposto(n), PI)?;
                let n = sinistra(direzioni[lati - 1]);
                self.settore(punti[punti.len() - 1], opposto(n), n, PI)?;
            }
        }
        Ok(())
    }

    fn punto(&mut self, c: Coord<f64>, estremita: Estremita) -> Result<(), ErroreBuffer> {
        match estremita {
            Estremita::Tonde => self.disco(c),
            Estremita::Quadrate => self.quadrato(c),
            Estremita::Piatte => Ok(()),
        }
    }

    /// I pezzi degli anelli di un poligono orientato, dal lato `lato`.
    fn anelli(&mut self, polygon: &Polygon<f64>, lato: Lato) -> Result<(), ErroreBuffer> {
        self.lato = lato;
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            self.spezzata(&ring.0, true, Estremita::Tonde)?;
        }
        self.lato = Lato::Entrambi;
        Ok(())
    }

    /// I pezzi di tutta la linework e dei punti di `geometry`; le parti
    /// areali in `areali`.
    fn geometria(
        &mut self,
        geometry: &Geometry<f64>,
        estremita: Estremita,
        areali: &mut Vec<MultiPolygon<f64>>,
    ) -> Result<(), ErroreBuffer> {
        match geometry {
            Geometry::Point(point) => self.punto(point.0, estremita),
            Geometry::MultiPoint(points) => {
                for point in points {
                    self.punto(point.0, estremita)?;
                }
                Ok(())
            }
            Geometry::Line(line) => self.spezzata(&[line.start, line.end], false, estremita),
            Geometry::LineString(line) => self.spezzata(&line.0, false, estremita),
            Geometry::MultiLineString(lines) => {
                for line in lines {
                    self.spezzata(&line.0, false, estremita)?;
                }
                Ok(())
            }
            Geometry::GeometryCollection(collection) => {
                for child in collection {
                    self.geometria(child, estremita, areali)?;
                }
                Ok(())
            }
            altro => {
                if let Some(polygons) = areale_di(altro) {
                    for polygon in &polygons {
                        self.anelli(polygon, Lato::Destra)?;
                    }
                    areali.push(polygons);
                }
                Ok(())
            }
        }
    }
}

/// Le parti areali di una geometria areale, orientate (anello esterno
/// antiorario): `unary_union` sceglie la regola di riempimento dal verso
/// del primo anello.
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

/// L'unione controllata degli operandi (regola dell'unione).
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

/// L'erosione controllata di una parte areale: la parte meno l'unione dei
/// pezzi dei suoi anelli.
fn erosione(
    parte: &MultiPolygon<f64>,
    raggio: f64,
    freccia: f64,
    precision: Precision,
) -> Result<MultiPolygon<f64>, ErroreBuffer> {
    let mut pezzi = Pezzi::nuovi(raggio, freccia)?;
    protetto(|| -> Result<(), ErroreBuffer> {
        for polygon in parte {
            pezzi.anelli(polygon, Lato::Sinistra)?;
        }
        Ok(())
    })??;
    let mut operandi = vec![parte.clone()];
    operandi.append(&mut pezzi.prodotti);
    griglia::controlla_overlay(griglia::rettangolo_multipoligoni(&operandi), precision)?;
    let risultato = protetto(|| parte.difference(&unary_union(&operandi[1..])))?;
    let controllo = Operandi::nuovi(operandi.iter().collect())?;
    controllo.verifica(
        &risultato,
        |_| true,
        parte.bounding_rect(),
        Regola::Differenza(0),
        precision,
    )?;
    Ok(risultato)
}

/// Il buffer di `geometry` a distanza `distance`, entro la precisione
/// (vedi il modulo): freccia degli archi [`freccia_degli_archi`], griglia
/// entro `p / 2`.
///
/// # Errors
///
/// [`ErroreBuffer::PrecisioneInsufficiente`] se la griglia o gli archi
/// supererebbero la precisione o un controllo a posteriori fallisce;
/// [`ErroreBuffer::TroppiVertici`] oltre [`MAX_VERTICI_BUFFER`].
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
    if distance > 0.0 {
        let mut pezzi = Pezzi::nuovi(distance, freccia)?;
        let mut operandi = Vec::new();
        protetto(|| pezzi.geometria(geometry, estremita, &mut operandi))??;
        operandi.append(&mut pezzi.prodotti);
        return unione(&operandi, precision);
    }
    let mut parti = Vec::new();
    protetto(|| areali(geometry, &mut parti))?;
    if distance == 0.0 {
        return unione(&parti, precision);
    }
    // Piu' parti: erosioni e unione finale si dividono il bilancio.
    let (passo, freccia) = if parti.len() > 1 {
        (
            Precision::new(precision.value() * 0.5)
                .map_err(|_| ErroreBuffer::PrecisioneInsufficiente)?,
            freccia * 0.5,
        )
    } else {
        (precision, freccia)
    };
    let mut erose = Vec::with_capacity(parti.len());
    for parte in &parti {
        let risultato = erosione(parte, -distance, freccia, passo)?;
        if !risultato.0.is_empty() {
            erose.push(protetto(|| risultato.orient(Direction::Default))?);
        }
    }
    match erose.len() {
        0 => Ok(MultiPolygon::new(Vec::new())),
        1 => Ok(erose.remove(0)),
        _ => unione(&erose, passo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Area, CoordsIter, Point};

    fn centimetro() -> Precision {
        Precision::new(0.01).unwrap()
    }

    /// Lati del cerchio per 1 cm (freccia 5 mm) a 1, 10, 100, 1000 m.
    #[test]
    fn i_lati_dipendono_da_distanza_e_precisione() {
        let lati: Vec<u64> = [1.0, 10.0, 100.0, 1000.0]
            .iter()
            .map(|d| lati_per_cerchio(*d, 0.005).unwrap())
            .collect();
        assert_eq!(lati, vec![32, 100, 315, 994]);
        for (d, n) in [1.0, 10.0, 100.0, 1000.0].iter().zip(&lati) {
            #[allow(clippy::cast_precision_loss)]
            let freccia = d * (1.0 - (PI / *n as f64).cos());
            assert!(freccia <= 0.005, "{d}: {freccia}");
        }
        assert_eq!(lati_per_cerchio(0.001, 0.005), Some(4));
        assert_eq!(lati_per_cerchio(1e12, 0.005), None);
        assert_eq!(lati_per_cerchio(f64::NAN, 0.005), None);
    }

    /// Il cerchio di un punto: fino a `|d| = 500 p` ogni punto del bordo
    /// sta entro `p` dal cerchio vero (il cerchio di `geo` a 10 m, 32 lati,
    /// aveva 4,8 cm di freccia); oltre, entro lo 0,1% della distanza piu' la
    /// griglia (deviazione dichiarata).
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

    #[test]
    fn la_freccia_e_mezzo_centimetro_o_lo_zero_virgola_uno_per_cento() {
        assert!((freccia_degli_archi(1.0, centimetro()) - 0.005).abs() < 1e-15);
        assert!((freccia_degli_archi(-5.0, centimetro()) - 0.005).abs() < 1e-15);
        assert!((freccia_degli_archi(100.0, centimetro()) - 0.1).abs() < 1e-15);
        let lati: Vec<u64> = [1.0, 10.0, 100.0, 1000.0]
            .iter()
            .map(|d| lati_per_cerchio(*d, freccia_degli_archi(*d, centimetro())).unwrap())
            .collect();
        assert_eq!(lati, vec![32, 71, 71, 71]);
    }

    /// Revisione: una componente che la griglia ridurrebbe a un punto (0,4
    /// mm accanto a una linea di 1.300 km) ha il suo buffer.
    #[test]
    fn la_componente_minuscola_ha_il_suo_buffer() {
        use geo::Contains as _;
        let linee = Geometry::MultiLineString(geo::MultiLineString::new(vec![
            LineString::from(vec![(0.0, 0.0), (1_300_000.0, 0.0)]),
            LineString::from(vec![(650_000.0, 9.0), (650_000.000_4, 9.0)]),
        ]));
        let buffer = buffer_controllato(&linee, 10.0, Estremita::Tonde, centimetro()).unwrap();
        assert!(buffer.contains(&Point::new(650_000.0, 18.9)));
        assert!(!buffer.contains(&Point::new(650_000.0, 19.1)));
    }

    /// Revisione: il buffer negativo di una collezione considera solo le
    /// parti areali (il punto non conta, come in `geo`).
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

    /// Estremita' piatte e quadrate su una linea, e un punto con estremita'
    /// piatte (nessun buffer, come in `geo`).
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
        // Freccia al piu' 5 mm per un perimetro di 2 pi.
        assert!((area(Estremita::Tonde) - (4.0 + PI)).abs() < 2.0 * PI * 0.005);
        let punto = Geometry::Point(Point::new(0.0, 0.0));
        assert!(
            buffer_controllato(&punto, 1.0, Estremita::Piatte, centimetro())
                .unwrap()
                .0
                .is_empty()
        );
    }

    /// L'erosione di un quadrato di 100 m di 10 m e la chiusura (+10, -10):
    /// gli angoli tornano entro la precisione.
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
}
