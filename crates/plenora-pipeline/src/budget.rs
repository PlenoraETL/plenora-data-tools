//! Budget di memoria del runner: il modello di costo delle operazioni e la
//! previsione del picco di un passo.
//!
//! Il modello è generato da `scripts/genera_costi_operazioni.py` in
//! [`crate::costi_operazioni`] (formula nell'intestazione di quel file).
//! Qui ci sono i tipi e l'aritmetica: intera, controllata, per eccesso.
//! Un valore non rappresentabile satura a `u64::MAX`, che non sta in nessun
//! budget e diventa un `ResourceLimit` esplicito, mai un passo ammesso.

use crate::costi_operazioni::{BUDGET_SPILL_MISURATO, COSTI, FATTORE_SICUREZZA};

/// Coefficienti del modello di una variante: `a` in byte, gli altri in
/// millesimi di byte per unità.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Costo {
    /// Costo al campione più piccolo misurato.
    pub a: u64,
    /// Per riga in ingresso (somma dei due lati per le binarie).
    pub r_millesimi: u64,
    /// Per byte in ingresso (allocazioni Arrow, una volta ciascuna).
    pub c_millesimi: u64,
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
    /// Profili del catalogo da cui viene il modello.
    pub profili: &'static [&'static str],
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
}

/// Il modello di un'operazione, se misurata.
#[must_use]
pub fn costo_di(op: &str) -> Option<&'static CostoOperazione> {
    COSTI
        .binary_search_by(|voce| voce.op.cmp(op))
        .ok()
        .and_then(|indice| COSTI.get(indice))
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
    /// Picco previsto oltre le tabelle residenti, con il fattore di
    /// sicurezza: `S * (a + max(r * righe, c * byte, p * coppie))`, per
    /// eccesso; `u64::MAX` se non rappresentabile.
    #[must_use]
    pub fn picco(&self, ingresso: Ingresso) -> u64 {
        let variabile = per_unita(self.r_millesimi, ingresso.righe)
            .max(per_unita(self.c_millesimi, ingresso.byte))
            .max(per_unita(self.p_millesimi, ingresso.coppie));
        let (numeratore, denominatore) = FATTORE_SICUREZZA;
        let base = u128::from(self.a) + variabile;
        let scalato = (base * u128::from(numeratore)).div_ceil(u128::from(denominatore));
        u64::try_from(scalato).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::{costo_di, Costo, Ingresso};
    use crate::costi_operazioni::{COSTI, FATTORE_SICUREZZA};

    #[test]
    fn i_costi_sono_ordinati_e_unici() {
        assert!(COSTI.windows(2).all(|coppia| coppia[0].op < coppia[1].op));
        assert!(costo_di("table.sort").is_some());
        assert!(costo_di("table.inesistente").is_none());
    }

    #[test]
    fn il_picco_arrotonda_per_eccesso_e_satura() {
        assert_eq!(FATTORE_SICUREZZA, (3, 2));
        let costo = Costo {
            a: 1,
            r_millesimi: 1,
            c_millesimi: 0,
            p_millesimi: 0,
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
            p_millesimi: u64::MAX,
        };
        let ingresso = Ingresso {
            righe: u64::MAX,
            byte: u64::MAX,
            coppie: u64::MAX,
        };
        assert_eq!(enorme.picco(ingresso), u64::MAX);
    }

    #[test]
    fn il_termine_maggiore_decide() {
        let costo = Costo {
            a: 0,
            r_millesimi: 2000,
            c_millesimi: 500,
            p_millesimi: 0,
        };
        // max(2 * 10, 0.5 * 100) = 50, per 3/2 = 75.
        let ingresso = Ingresso {
            righe: 10,
            byte: 100,
            coppie: 0,
        };
        assert_eq!(costo.picco(ingresso), 75);
    }
}
