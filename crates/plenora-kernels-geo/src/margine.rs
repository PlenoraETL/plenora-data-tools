//! Il margine di memoria dei kernel geo: quanti byte un kernel puo' ancora
//! far crescere con i suoi risultati e i suoi insiemi candidati prima di
//! fermarsi con un errore esplicito.
//!
//! **Perche'.** Il runner prevede il picco di un passo dal modello di costo
//! (`plenora-pipeline`, «Budget di memoria»), ma alcuni profili crescono
//! con una grandezza che a secco non si conosce: le coppie candidate di un
//! join spaziale in cui ogni riga ne tocca molte, i pezzi di un overlay di
//! poligoni che si sovrappongono, le sovrapposizioni di una copertura, i
//! vicini equidistanti. Senza margine il kernel li alloca tutti, e il
//! runner se ne accorge solo dopo il passo, o mai se la memoria finisce
//! prima. Con il margine il kernel conta, nei punti in cui quei vettori
//! crescono, i byte che vi ha messo (piu' quelli che il chiamante spende
//! per ogni risultato nell'uscita, [`MargineMemoria::con_uscita_per_risultato`])
//! e si ferma con un `ResourceLimit` prima di superarlo.
//!
//! **Che cosa conta e che cosa no.** Si contano i risultati che il kernel
//! trattiene (coppie, pezzi, problemi, vicini, uscite del buffer) con stime
//! per eccesso della loro rappresentazione in memoria
//! ([`crate::memory_estimate`] per le geometrie), non il transitorio dentro
//! una chiamata di `geo` o `i_overlay`, che non si osserva; il controllo e'
//! deterministico (conteggi nell'ordine delle righe, o un totale che non
//! dipende dai thread). Nei kernel per riga (`buffer`) il margine vale per
//! ogni riga: righe in parallelo possono trattenere insieme fino a tanti
//! margini quanti sono i thread (limite dichiarato nel README).

use std::mem::size_of;

use geo::{Coord, Geometry, LineString, MultiPolygon, Point, Polygon};
use thiserror::Error;

/// Il margine di memoria passato a un kernel geo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MargineMemoria {
    byte: u64,
    uscita_per_risultato: u64,
}

impl MargineMemoria {
    /// Nessun margine: i kernel chiamati senza runner, come prima.
    pub const ILLIMITATO: Self = Self {
        byte: u64::MAX,
        uscita_per_risultato: 0,
    };

    /// Un margine di `byte` byte.
    #[must_use]
    pub const fn byte(byte: u64) -> Self {
        Self {
            byte,
            uscita_per_risultato: 0,
        }
    }

    /// Lo stesso margine, con `byte` byte che il chiamante spendera' per
    /// ogni risultato nell'uscita (per esempio la riga sinistra ripetuta di
    /// un join): il kernel li conta insieme ai propri.
    #[must_use]
    pub const fn con_uscita_per_risultato(self, byte: u64) -> Self {
        Self {
            byte: self.byte,
            uscita_per_risultato: byte,
        }
    }

    /// Lo stesso margine (con la stessa uscita per risultato) ridotto o
    /// portato a `byte` byte.
    #[must_use]
    pub const fn con_byte(self, byte: u64) -> Self {
        Self {
            byte,
            uscita_per_risultato: self.uscita_per_risultato,
        }
    }

    /// I byte del margine.
    #[must_use]
    pub const fn byte_disponibili(self) -> u64 {
        self.byte
    }

    /// I byte di un risultato: quelli del kernel piu' quelli dell'uscita.
    #[must_use]
    pub const fn byte_per_risultato(self, nel_kernel: u64) -> u64 {
        nel_kernel.saturating_add(self.uscita_per_risultato)
    }

    /// Quanti risultati da `nel_kernel` byte (piu' l'uscita) entrano nel
    /// margine; nessun tetto ([`u64::MAX`]) senza margine.
    #[must_use]
    pub const fn risultati_massimi(self, nel_kernel: u64) -> u64 {
        let per_risultato = self.byte_per_risultato(nel_kernel);
        if per_risultato == 0 || self.byte == u64::MAX {
            u64::MAX
        } else {
            self.byte / per_risultato
        }
    }

    /// `Ok` se `previsti` byte stanno nel margine.
    ///
    /// # Errors
    ///
    /// [`MargineSuperato`] se non ci stanno.
    pub const fn verifica(self, previsti: u64) -> Result<(), MargineSuperato> {
        if previsti <= self.byte {
            Ok(())
        } else {
            Err(MargineSuperato {
                previsti,
                margine: self.byte,
            })
        }
    }
}

/// Un kernel geo si e' fermato prima di superare il margine di memoria.
/// Solo conteggi di byte, nessun valore dei dati.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error(
    "memoria prevista {previsti} byte oltre il margine di {margine} byte \
     (max_governed_memory_bytes)"
)]
pub struct MargineSuperato {
    /// I byte che il kernel avrebbe trattenuto (almeno), oltre il margine.
    pub previsti: u64,
    /// Il margine.
    pub margine: u64,
}

/// Il tetto dei risultati di un kernel che ha gia' un limite di conteggio
/// (`max_pairs`, `max_results`): il minore fra quel limite e i risultati
/// che entrano nel margine.
#[derive(Clone, Copy, Debug)]
pub struct TettoRisultati {
    limite: u64,
    tetto_memoria: u64,
    byte_per_risultato: u64,
    margine: u64,
}

impl TettoRisultati {
    /// Il tetto per `limite` risultati da `nel_kernel` byte nel `margine`.
    #[must_use]
    pub const fn nuovo(limite: u64, margine: MargineMemoria, nel_kernel: u64) -> Self {
        Self {
            limite,
            tetto_memoria: margine.risultati_massimi(nel_kernel),
            byte_per_risultato: margine.byte_per_risultato(nel_kernel),
            margine: margine.byte_disponibili(),
        }
    }

    /// Quanti risultati si possono trattenere.
    #[must_use]
    pub const fn tetto(&self) -> u64 {
        if self.limite < self.tetto_memoria {
            self.limite
        } else {
            self.tetto_memoria
        }
    }

    /// Superato il tetto: `None` se a fermare e' il limite di conteggio
    /// (l'errore e' quello del kernel), il margine superato altrimenti.
    #[must_use]
    pub const fn superato_dalla_memoria(&self) -> Option<MargineSuperato> {
        if self.limite <= self.tetto_memoria {
            None
        } else {
            Some(MargineSuperato {
                previsti: self
                    .tetto_memoria
                    .saturating_add(1)
                    .saturating_mul(self.byte_per_risultato),
                margine: self.margine,
            })
        }
    }
}

/// Un contatore dei byte trattenuti, in un kernel sequenziale.
#[derive(Clone, Copy, Debug)]
pub struct ContatoreMargine {
    margine: MargineMemoria,
    usati: u64,
}

impl ContatoreMargine {
    /// Un contatore vuoto sul margine.
    #[must_use]
    pub const fn nuovo(margine: MargineMemoria) -> Self {
        Self { margine, usati: 0 }
    }

    /// Aggiunge un risultato da `nel_kernel` byte (piu' l'uscita).
    ///
    /// # Errors
    ///
    /// [`MargineSuperato`] se il totale supera il margine.
    pub fn aggiungi(&mut self, nel_kernel: u64) -> Result<(), MargineSuperato> {
        let totale = self
            .usati
            .saturating_add(self.margine.byte_per_risultato(nel_kernel));
        self.margine.verifica(totale)?;
        self.usati = totale;
        Ok(())
    }

    /// Un contatore sul margine con `usati` byte gia' contati.
    #[must_use]
    pub const fn con_usati(margine: MargineMemoria, usati: u64) -> Self {
        Self { margine, usati }
    }

    /// Aggiunge `byte` byte del kernel senza l'uscita per risultato.
    ///
    /// # Errors
    ///
    /// [`MargineSuperato`] se il totale supera il margine.
    pub fn aggiungi_senza_uscita(&mut self, byte: u64) -> Result<(), MargineSuperato> {
        let totale = self.usati.saturating_add(byte);
        self.margine.verifica(totale)?;
        self.usati = totale;
        Ok(())
    }

    /// I byte contati finora.
    #[must_use]
    pub const fn usati(&self) -> u64 {
        self.usati
    }
}

/// I byte di heap che una geometria trattiene.
///
/// La **capacita'** di ogni `Vec` (coordinate, anelli, parti, figli) per la dimensione del suo
/// elemento. E' il conto esatto delle allocazioni della geometria, escluso
/// l'overhead dell'allocatore; la struttura della geometria stessa la conta
/// il contenitore che la tiene.
#[must_use]
pub fn byte_heap_geometria(geometria: &Geometry<f64>) -> u64 {
    match geometria {
        Geometry::Point(_) | Geometry::Line(_) | Geometry::Rect(_) | Geometry::Triangle(_) => 0,
        Geometry::LineString(linea) => byte_linea(linea),
        Geometry::Polygon(poligono) => byte_poligono(poligono),
        Geometry::MultiPoint(punti) => byte_vec(punti.0.capacity(), size_of::<Point<f64>>()),
        Geometry::MultiLineString(linee) => linee.0.iter().fold(
            byte_vec(linee.0.capacity(), size_of::<LineString<f64>>()),
            |totale, linea| totale.saturating_add(byte_linea(linea)),
        ),
        Geometry::MultiPolygon(poligoni) => byte_multipoligono(poligoni),
        Geometry::GeometryCollection(collezione) => collezione.0.iter().fold(
            byte_vec(collezione.0.capacity(), size_of::<Geometry<f64>>()),
            |totale, figlia| totale.saturating_add(byte_heap_geometria(figlia)),
        ),
    }
}

/// [`byte_heap_geometria`] di un `MultiPolygon`, senza costruire la
/// `Geometry`.
#[must_use]
pub fn byte_multipoligono(poligoni: &MultiPolygon<f64>) -> u64 {
    poligoni.0.iter().fold(
        byte_vec(poligoni.0.capacity(), size_of::<Polygon<f64>>()),
        |totale, poligono| totale.saturating_add(byte_poligono(poligono)),
    )
}

fn byte_vec(capacita: usize, elemento: usize) -> u64 {
    u64::try_from(capacita.saturating_mul(elemento)).unwrap_or(u64::MAX)
}

fn byte_linea(linea: &LineString<f64>) -> u64 {
    byte_vec(linea.0.capacity(), size_of::<Coord<f64>>())
}

fn byte_poligono(poligono: &Polygon<f64>) -> u64 {
    poligono.interiors().iter().fold(
        byte_linea(poligono.exterior()).saturating_add(byte_vec(
            poligono.interiors().len(),
            size_of::<LineString<f64>>(),
        )),
        |totale, anello| totale.saturating_add(byte_linea(anello)),
    )
}

/// La lunghezza esatta del WKB 2D di una geometria (`Rect` e `Triangle`
/// come i poligoni che `geo` ne fa, per eccesso).
#[must_use]
pub fn byte_wkb(geometria: &Geometry<f64>) -> u64 {
    // Ordine dei byte e tipo, poi un conteggio di 4 byte dove serve.
    const TESTA: u64 = 5;
    const CONTEGGIO: u64 = 4;
    const PUNTO: u64 = 16;
    let n = |quanti: usize| u64::try_from(quanti).unwrap_or(u64::MAX);
    let anello = |quanti: usize| CONTEGGIO.saturating_add(n(quanti).saturating_mul(PUNTO));
    let poligono = |p: &Polygon<f64>| {
        p.interiors().iter().fold(
            (TESTA + CONTEGGIO).saturating_add(anello(p.exterior().0.len())),
            |totale, interno| totale.saturating_add(anello(interno.0.len())),
        )
    };
    match geometria {
        Geometry::Point(_) => TESTA + PUNTO,
        Geometry::Line(_) => TESTA + anello(2),
        Geometry::LineString(linea) => TESTA.saturating_add(anello(linea.0.len())),
        Geometry::Rect(_) => TESTA + CONTEGGIO + anello(5),
        Geometry::Triangle(_) => TESTA + CONTEGGIO + anello(4),
        Geometry::Polygon(p) => poligono(p),
        Geometry::MultiPoint(punti) => {
            (TESTA + CONTEGGIO).saturating_add(n(punti.0.len()).saturating_mul(TESTA + PUNTO))
        }
        Geometry::MultiLineString(linee) => linee.0.iter().fold(TESTA + CONTEGGIO, |t, l| {
            t.saturating_add(TESTA.saturating_add(anello(l.0.len())))
        }),
        Geometry::MultiPolygon(parti) => parti
            .0
            .iter()
            .fold(TESTA + CONTEGGIO, |t, p| t.saturating_add(poligono(p))),
        Geometry::GeometryCollection(collezione) => collezione
            .0
            .iter()
            .fold(TESTA + CONTEGGIO, |t, g| t.saturating_add(byte_wkb(g))),
    }
}

/// Un maggiorante dei byte che la codifica WKB di una geometria alloca.
///
/// Il `Vec` di `to_wkb` cresce raddoppiando, quindi la sua capacita' finale e'
/// al piu' il doppio della lunghezza ([`byte_wkb`]) piu' la capacita'
/// minima.
#[must_use]
pub fn byte_codifica(geometria: &Geometry<f64>) -> u64 {
    byte_wkb(geometria).saturating_mul(2).saturating_add(64)
}

/// I byte di un `Vec` di `elemento` byte cresciuto con `push` fino a
/// `lunghezza` elementi: la capacita' e' al piu' il doppio della lunghezza,
/// e almeno 4 elementi.
#[must_use]
pub fn byte_vec_cresciuto(lunghezza: usize, elemento: usize) -> u64 {
    byte_vec(lunghezza.saturating_mul(2).max(4), elemento)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margine_e_contatore() {
        let margine = MargineMemoria::byte(100).con_uscita_per_risultato(10);
        assert_eq!(margine.byte_per_risultato(15), 25);
        assert_eq!(margine.risultati_massimi(15), 4);
        assert_eq!(MargineMemoria::byte(7).risultati_massimi(0), u64::MAX);
        assert!(margine.verifica(100).is_ok());
        assert_eq!(
            margine.verifica(101),
            Err(MargineSuperato {
                previsti: 101,
                margine: 100
            })
        );
        let mut contatore = ContatoreMargine::nuovo(margine);
        for _ in 0..4 {
            contatore.aggiungi(15).unwrap();
        }
        assert_eq!(contatore.usati(), 100);
        assert_eq!(
            contatore.aggiungi(0),
            Err(MargineSuperato {
                previsti: 110,
                margine: 100
            })
        );
        assert_eq!(contatore.usati(), 100);
        assert_eq!(MargineMemoria::ILLIMITATO.risultati_massimi(64), u64::MAX);
    }

    #[test]
    fn la_lunghezza_del_wkb_e_esatta_e_la_codifica_la_maggiora() {
        use geo::{line_string, point, polygon, MultiLineString};
        let quadrato: Polygon<f64> = polygon![
            exterior: [(x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 4.0), (x: 0.0, y: 4.0), (x: 0.0, y: 0.0)],
            interiors: [[(x: 1.0, y: 1.0), (x: 1.0, y: 2.0), (x: 2.0, y: 2.0), (x: 1.0, y: 1.0)]],
        ];
        let casi = [
            Geometry::Point(point!(x: 1.0, y: 2.0)),
            Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]),
            Geometry::Polygon(quadrato.clone()),
            Geometry::MultiPolygon(MultiPolygon::new(vec![quadrato.clone(), quadrato])),
            Geometry::MultiLineString(MultiLineString::new(vec![
                line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)],
                line_string![(x: 2.0, y: 0.0), (x: 3.0, y: 1.0), (x: 4.0, y: 0.0)],
            ])),
        ];
        for caso in &casi {
            let wkb = crate::arrow_adapter::encode_geometry(caso).unwrap();
            assert_eq!(byte_wkb(caso), wkb.len() as u64);
            assert!(byte_codifica(caso) >= wkb.capacity() as u64);
        }
    }
}
