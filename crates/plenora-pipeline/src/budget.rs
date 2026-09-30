//! Budget di memoria del runner: il modello di costo delle operazioni e la
//! previsione del picco di un passo.
//!
//! Il modello è generato dalle misure v4 da
//! `scripts/genera_costi_operazioni.py` in [`crate::costi_operazioni`]
//! (formula nell'intestazione di quel file) per le operazioni tabellari, e
//! da `scripts/genera_costi_geo.py` in [`crate::costi_geo`] per le geo, con
//! le regole di `scripts/modello_costi.py`.
//! Qui ci sono i tipi e l'aritmetica: intera, controllata, per eccesso.
//! Un valore non rappresentabile satura a `u64::MAX`, che non sta in nessun
//! budget e diventa un `ResourceLimit` esplicito, mai un passo ammesso.

use crate::costi_geo::COSTI_GEO;
use crate::costi_operazioni::{BUDGET_SPILL_MISURATO, COSTI, FATTORE_SICUREZZA};

/// Coefficienti del modello di una variante: `a` in byte, gli altri in
/// millesimi di byte per unità. Il picco senza fattore di sicurezza è
///
/// ```text
/// a + max(r*R + c*B, r_s*R, c_l*B) + k*K + p*P
/// ```
///
/// con `R` righe, `B` byte, `K` celle d'uscita e `P` coppie di [`Ingresso`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Costo {
    /// Costo fisso, non oltre il picco più piccolo misurato.
    pub a: u64,
    /// Piano: per riga in ingresso (somma dei due lati per le binarie).
    pub r_millesimi: u64,
    /// Piano: per byte in ingresso (allocazioni Arrow, una volta ciascuna).
    pub c_millesimi: u64,
    /// Ramo per righe più strette di quelle misurate: per riga, dal profilo
    /// con le righe più strette.
    pub r_stretta_millesimi: u64,
    /// Ramo per righe più larghe di quelle misurate: per byte, dal profilo
    /// con le righe più larghe.
    pub c_larga_millesimi: u64,
    /// Per cella d'uscita prevista (righe in ingresso per colonne
    /// d'uscita): solo le operazioni la cui larghezza d'uscita la fa la
    /// config (`pivot`, che oggi il runner rifiuta in validazione: il
    /// termine vale per un contratto con le colonne esatte del kernel).
    pub k_millesimi: u64,
    /// Per coppia di righe (sinistra per destra): solo le superlineari.
    pub p_millesimi: u64,
}

/// Modello di un'operazione misurata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CostoOperazione {
    pub op: &'static str,
    pub in_memoria: Costo,
    /// Variante spilled, dove il kernel la ha ed è stata misurata.
    pub spill: Option<Costo>,
    /// Output e transitorio dipendono dal contenuto: l'output lo limitano i
    /// preflight dei kernel e il controllo esatto dopo il passo.
    pub dipende_dai_dati: bool,
    /// Profili misurati dell'operazione.
    pub profili: &'static [&'static str],
    /// Profili misurati ma esclusi dal modello: la loro memoria cresce con
    /// una grandezza che il runner non conosce prima del passo (README,
    /// «Limiti dichiarati del runner», voce «Modelli di costo geo»).
    pub esclusi: &'static [&'static str],
}

/// Grandezze dell'ingresso di un passo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ingresso {
    /// Righe di tutti gli input.
    pub righe: u64,
    /// Byte Arrow degli input, ogni allocazione una volta.
    pub byte: u64,
    /// Righe sinistra per righe destra (zero per le unarie).
    pub coppie: u64,
    /// Righe per colonne dell'uscita (colonne note a secco dal contratto).
    pub celle: u64,
}

/// Il modello di un'operazione, se misurata: le tabellari da
/// [`COSTI`], le geo da [`COSTI_GEO`].
#[must_use]
pub fn costo_di(op: &str) -> Option<&'static CostoOperazione> {
    let tabella = if op.starts_with("geo.") {
        COSTI_GEO
    } else {
        COSTI
    };
    tabella
        .binary_search_by(|voce| voce.op.cmp(op))
        .ok()
        .and_then(|indice| tabella.get(indice))
}

/// Memoria da riservare, oltre al picco del modello, al kernel spilled.
///
/// Il kernel tiene in memoria fino al proprio `max_governed_memory_bytes`
/// di batch riletti da una partizione, e `concat_batches` li copia: la
/// riserva è due volte il margine che gli si passa, cioè due volte due
/// partizioni medie (margine per lo sbilanciamento delle chiavi), al più
/// [`BUDGET_SPILL_MISURATO`]. Il picco misurato non la copre: le varianti
/// spilled sono state misurate con la stima dei batch IPC gonfiata, cioè
/// con partizioni rilette più piccole (README, «Budget di memoria»).
#[must_use]
pub fn riserva_spill(byte_in: u64, partizioni: u32) -> u64 {
    let media = byte_in.div_ceil(u64::from(partizioni.max(1)));
    media
        .saturating_mul(2)
        .min(BUDGET_SPILL_MISURATO)
        .saturating_mul(2)
}

/// `ceil(millesimi * unita / 1000)` in `u128`: non trabocca per operandi
/// `u64`.
fn per_unita(millesimi: u64, unita: u64) -> u128 {
    (u128::from(millesimi) * u128::from(unita)).div_ceil(1000)
}

impl Costo {
    /// Picco del modello senza fattore di sicurezza, per eccesso termine
    /// per termine: `a + max(r*R + c*B, r_s*R, c_l*B) + k*K + p*P`. È il
    /// valore che l'oracolo confronta con le misure.
    #[must_use]
    pub fn base(&self, ingresso: Ingresso) -> u128 {
        let piano = per_unita(self.r_millesimi, ingresso.righe)
            + per_unita(self.c_millesimi, ingresso.byte);
        let variabile = piano
            .max(per_unita(self.r_stretta_millesimi, ingresso.righe))
            .max(per_unita(self.c_larga_millesimi, ingresso.byte));
        u128::from(self.a)
            + variabile
            + per_unita(self.k_millesimi, ingresso.celle)
            + per_unita(self.p_millesimi, ingresso.coppie)
    }

    /// Picco previsto oltre le tabelle residenti, con il fattore di
    /// sicurezza `S` sulla [`base`](Self::base), per eccesso; `u64::MAX` se
    /// non rappresentabile.
    #[must_use]
    pub fn picco(&self, ingresso: Ingresso) -> u64 {
        let (numeratore, denominatore) = FATTORE_SICUREZZA;
        let scalato = self
            .base(ingresso)
            .saturating_mul(u128::from(numeratore))
            .div_ceil(u128::from(denominatore));
        u64::try_from(scalato).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::{costo_di, Costo, Ingresso};
    use crate::costi_geo::COSTI_GEO;
    use crate::costi_operazioni::{COSTI, FATTORE_SICUREZZA};

    #[test]
    fn i_costi_sono_ordinati_e_unici() {
        assert!(COSTI.windows(2).all(|coppia| coppia[0].op < coppia[1].op));
        assert!(COSTI_GEO
            .windows(2)
            .all(|coppia| coppia[0].op < coppia[1].op));
        assert!(costo_di("table.sort").is_some());
        assert!(costo_di("geo.buffer").is_some());
        assert!(costo_di("table.inesistente").is_none());
        assert!(costo_di("geo.inesistente").is_none());
    }

    #[test]
    fn il_picco_arrotonda_per_eccesso_e_satura() {
        assert_eq!(FATTORE_SICUREZZA, (3, 2));
        let costo = Costo {
            a: 1,
            r_millesimi: 1,
            ..Costo::default()
        };
        // a + ceil(1/1000) = 2, per 3/2 = 3.
        assert_eq!(
            costo.picco(Ingresso {
                righe: 1,
                ..Ingresso::default()
            }),
            3
        );
        let enorme = Costo {
            a: u64::MAX,
            r_millesimi: u64::MAX,
            c_millesimi: u64::MAX,
            r_stretta_millesimi: u64::MAX,
            c_larga_millesimi: u64::MAX,
            k_millesimi: u64::MAX,
            p_millesimi: u64::MAX,
        };
        let ingresso = Ingresso {
            righe: u64::MAX,
            byte: u64::MAX,
            coppie: u64::MAX,
            celle: u64::MAX,
        };
        assert_eq!(enorme.picco(ingresso), u64::MAX);
    }

    #[test]
    fn piano_rami_e_termini_additivi() {
        let costo = Costo {
            a: 7,
            r_millesimi: 2000,
            c_millesimi: 500,
            r_stretta_millesimi: 3000,
            c_larga_millesimi: 800,
            k_millesimi: 3000,
            p_millesimi: 0,
        };
        // Righe larghe: piano 2*10 + 0.5*100 = 70 contro i rami 3*10 = 30 e
        // 0.8*100 = 80, decide il ramo per byte; piu' 3*5 = 15 per cella.
        let larghe = Ingresso {
            righe: 10,
            byte: 100,
            coppie: 0,
            celle: 5,
        };
        assert_eq!(costo.base(larghe), 7 + 80 + 15);
        // Righe strette: piano 250, decide il ramo per riga (300).
        let strette = Ingresso {
            righe: 100,
            ..larghe
        };
        assert_eq!(costo.base(strette), 7 + 300 + 15);
        // In mezzo: il piano (40 contro 30 e 32).
        let medie = Ingresso { byte: 40, ..larghe };
        assert_eq!(costo.base(medie), 7 + 40 + 15);
        // Per 3/2, per eccesso: 62 * 3 / 2 = 93.
        assert_eq!(costo.picco(medie), 93);
    }
}
