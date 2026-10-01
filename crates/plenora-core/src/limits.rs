//! I limiti, in tre famiglie.
//!
//! - [`RowLimits`]: righe e fattore di espansione;
//! - [`PlanLimits`]: complessità del piano, applicati in lettura e in
//!   validazione;
//! - [`Limits`]: contenitore unico (righe, piano, memoria, stringhe,
//!   geometrie).
//!
//! Il runner applica solo una parte dei campi e rende dichiarabili nel piano
//! solo quelli (README, «Il piano»); i campi che nessun codice applica lo
//! dicono nel proprio rustdoc.

use serde::Deserialize;

use crate::error::{PlenoraError, Result};

/// Limiti di righe, semanticamente distinti.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowLimits {
    /// Righe di ciascun input del piano. Default 10 milioni.
    pub max_input_rows: u64,
    /// Righe di ciascun output del piano. Default 10 milioni.
    pub max_output_rows: u64,
    /// Righe di ogni tabella intermedia (uscita di un passo). Default 10
    /// milioni.
    pub max_rows_per_edge: u64,
    /// Fattore di espansione massimo, righe d'uscita su righe d'ingresso: la
    /// base è l'unico ingresso per le unarie, il vincolo del catalogo
    /// ([`crate::catalog::ExpansionConstraint`]) per le operazioni a due
    /// ingressi. Finito e positivo; default 100.
    ///
    /// Che cosa protegge: il runner lo controlla dopo il passo, sulle righe
    /// dell'uscita gia' costruita, quindi non difende la memoria (la
    /// difendono il budget prima e dopo il passo, i preflight dei kernel e
    /// `max_rows_per_edge`). E' una guardia logica contro un'espansione dei
    /// dati che nessun piano sensato chiede: un join molti-a-molti su una
    /// chiave sbagliata, un prodotto cartesiano involontario, liste esplose
    /// piu' lunghe del previsto. Non si applica alle uscite con righe fissate
    /// dalla config (esenzioni del catalogo, `table.melt`). Con la base
    /// `SumRelative` dei join un abbinamento con la chiave unica su un lato
    /// vale al piu' 1, quindi 100 non rifiuta un arricchimento 1:N o N:1
    /// legittimo (README, «Fattore di espansione»).
    pub max_expansion_factor: f64,
}

impl Default for RowLimits {
    fn default() -> Self {
        Self {
            max_input_rows: 10_000_000,
            max_output_rows: 10_000_000,
            max_rows_per_edge: 10_000_000,
            max_expansion_factor: 100.0,
        }
    }
}

/// Budget di memoria governato applicato quando il piano non lo dichiara.
///
/// 512 MiB. È pubblica perché chi scrive un piano deve poter sapere quale
/// budget si applica quando non lo dichiara.
///
/// È una costante sola perché serve anche ai kernel tabellari: due letterali
/// divergerebbero in silenzio, con budget diversi applicati allo stesso piano.
pub const DEFAULT_MAX_GOVERNED_MEMORY_BYTES: u64 = DEFAULT_MAX_GOVERNED_MEMORY_BYTES_USIZE as u64;

/// Lo stesso default per chi lo tiene in `usize`.
///
/// Il letterale sta qui e non nella forma `u64`: `as usize` troncherebbe su un
/// target a 32 bit, che applicherebbe un budget diverso da quello pubblicato.
///
/// La conversione verso `u64` e' esatta perche' **questo valore** e'
/// rappresentabile in entrambe le forme, non perche' `usize as u64` lo sia in
/// generale: chi cambia il valore rifa la verifica. Sui target dove il
/// letterale non entra in `usize` la valutazione costante va in overflow, e
/// la compilazione fallisce.
pub const DEFAULT_MAX_GOVERNED_MEMORY_BYTES_USIZE: usize = 512 * 1024 * 1024;

/// Byte massimi di un testo, applicati quando il piano non li dichiara.
///
/// 16 MiB, per i testi di config e per quelli prodotti dai kernel. Costante
/// sola, come [`DEFAULT_MAX_GOVERNED_MEMORY_BYTES`]: la usano anche i kernel
/// tabellari.
pub const DEFAULT_MAX_STRING_BYTES: usize = 16 * 1024 * 1024;

/// Byte massimi di un'espressione regolare, applicati quando il piano non li
/// dichiara.
///
/// 64 KiB. Costante sola: prima i kernel tabellari ne avevano una loro
/// (4096), e chi li chiamava direttamente con i limiti di default applicava
/// un tetto diverso da quello del piano.
pub const DEFAULT_MAX_REGEX_BYTES: usize = 64 * 1024;

/// Limiti alla complessità del piano, controllati prima di qualunque dato.
///
/// Il runner applica sempre `PlanLimits::default()`: nel piano non sono
/// dichiarabili.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanLimits {
    /// Byte del testo JSON del piano, controllati prima della lettura.
    /// Default 16 MiB.
    pub max_plan_json_bytes: usize,
    /// Passi del piano. Default 1024.
    pub max_plan_nodes: usize,
    /// Archi (ingressi dei passi). Default 4096.
    pub max_plan_edges: usize,
    /// Profondità del grafo dei passi. Default 256.
    pub max_plan_depth: usize,
    /// Consumatori di una stessa tabella. Default 64.
    pub max_fan_out: usize,
    /// Input del piano. Default 16.
    pub max_inputs: usize,
    /// Byte della config di un passo. Default 1 MiB.
    pub max_config_bytes_per_node: usize,
    /// Byte di un nome (input, passo, output). Default 256.
    pub max_identifier_bytes: usize,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_plan_json_bytes: 16 * 1024 * 1024,
            max_plan_nodes: 1_024,
            max_plan_edges: 4_096,
            max_plan_depth: 256,
            max_fan_out: 64,
            max_inputs: 16,
            max_config_bytes_per_node: 1024 * 1024,
            max_identifier_bytes: 256,
        }
    }
}

/// Contenitore unico dei limiti.
///
/// Si costruisce con `Limits::default()` e si controlla con
/// [`Limits::validate`]; il runner parte dal default e sostituisce i soli
/// campi dichiarati nel piano.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Limiti di righe (in serde, campi al primo livello).
    #[serde(flatten)]
    pub rows: RowLimits,
    /// Limiti di complessità del piano.
    #[serde(default)]
    pub plan: PlanLimits,
    /// Budget di memoria del runner, in byte (README, «Budget di memoria»).
    /// Default [`DEFAULT_MAX_GOVERNED_MEMORY_BYTES`].
    pub max_governed_memory_bytes: u64,
    /// Grado massimo di parallelismo; `0` significa «numero di core logici».
    ///
    /// Nessun codice di questo repository lo applica: nel progetto
    /// d'origine l'engine dimensionava il pool Rayon del processo.
    pub max_parallelism: u32,
    /// Byte di una cella WKB. Default 64 MiB. Nessun codice di questo
    /// repository lo applica.
    pub max_wkb_cell_bytes: u64,
    /// Byte complessivi letti. Default 16 GiB. Nessun codice di questo
    /// repository lo applica.
    pub max_payload_bytes: u64,
    /// Batch per arco. Default 65 536. Nessun codice di questo repository lo
    /// applica: il runner ha un batch per tabella.
    pub max_batches: u64,
    /// Profondità d'annidamento delle geometrie (collezioni dentro
    /// collezioni). Default 64, lo stesso valore delle costanti con cui
    /// `plenora-io` e i kernel geo controllano il WKB; il campo in sé non è
    /// letto da nessun codice.
    pub max_geometry_depth: u32,
    /// Byte di un testo: parametri testuali delle config tabellari e
    /// stringhe prodotte da alcuni kernel (per esempio `string_pad`).
    /// Default 16 MiB.
    pub max_string_bytes: usize,
    /// Byte di un pattern regex. Default 64 KiB.
    pub max_regex_bytes: usize,
}

impl Limits {
    /// Validazione dei limiti effettivi, in un punto solo.
    ///
    /// Un limite fuori dominio si rifiuta, non si corregge: una correzione
    /// silenziosa (per esempio `max_input_rows.max(1)`) eseguirebbe il piano
    /// con limiti diversi da quelli dichiarati. Un limite a zero rende il
    /// componente incapace di fare alcunche', e va detto subito.
    /// `max_expansion_factor` dev'essere finito e positivo: un `NaN` rende
    /// falso ogni confronto e aprirebbe il limite.
    ///
    /// # Errors
    ///
    /// `PlenoraError::InvalidPlan` alla prima violazione, nominando il limite.
    pub fn validate(&self) -> Result<()> {
        let nullo = |nome: &str| {
            Err(PlenoraError::InvalidPlan(format!(
                "{nome} a zero: nessuna esecuzione sarebbe possibile"
            )))
        };
        if self.rows.max_input_rows == 0 {
            return nullo("max_input_rows");
        }
        if self.rows.max_output_rows == 0 {
            return nullo("max_output_rows");
        }
        if self.rows.max_rows_per_edge == 0 {
            return nullo("max_rows_per_edge");
        }
        if !self.rows.max_expansion_factor.is_finite() || self.rows.max_expansion_factor <= 0.0 {
            return Err(PlenoraError::InvalidPlan(
                "max_expansion_factor deve essere finito e positivo: un valore non ordinabile \
                 renderebbe vero ogni confronto di espansione"
                    .to_owned(),
            ));
        }
        if self.max_governed_memory_bytes == 0 {
            return nullo("max_governed_memory_bytes");
        }
        if self.max_payload_bytes == 0 {
            return nullo("max_payload_bytes");
        }
        if self.max_batches == 0 {
            return nullo("max_batches");
        }
        if self.max_wkb_cell_bytes == 0 {
            return nullo("max_wkb_cell_bytes");
        }
        if self.max_geometry_depth == 0 {
            return nullo("max_geometry_depth");
        }
        if self.max_string_bytes == 0 {
            return nullo("max_string_bytes");
        }
        if self.max_regex_bytes == 0 {
            return nullo("max_regex_bytes");
        }
        // Limiti di piano: si rifiuta lo zero solo dove nessun documento
        // valido potrebbe rispettarlo. I tetti sui nodi (nodi, archi,
        // profondita', fan-out, byte di config) valgono legittimamente zero
        // per un piano senza passi: rifiutarli qui darebbe un verdetto
        // discorde da quello del piano. Un piano invece ha sempre byte,
        // almeno un input e identificatori non vuoti.
        if self.plan.max_plan_json_bytes == 0 {
            return nullo("plan.max_plan_json_bytes");
        }
        if self.plan.max_inputs == 0 {
            return nullo("plan.max_inputs");
        }
        if self.plan.max_identifier_bytes == 0 {
            return nullo("plan.max_identifier_bytes");
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            rows: RowLimits::default(),
            plan: PlanLimits::default(),
            max_governed_memory_bytes: DEFAULT_MAX_GOVERNED_MEMORY_BYTES,
            max_parallelism: 0, // 0 = numero di core logici
            max_wkb_cell_bytes: 64 * 1024 * 1024,
            max_payload_bytes: 16 * 1024 * 1024 * 1024,
            max_batches: 65_536,
            max_geometry_depth: 64,
            max_string_bytes: DEFAULT_MAX_STRING_BYTES,
            max_regex_bytes: DEFAULT_MAX_REGEX_BYTES,
        }
    }
}

/// `true` se `output_rows` supera `base_rows * factor`.
///
/// Nessun conteggio passa per `f64`: il fattore (un `f64` per contratto) si
/// decompone in `mantissa * 2^esponente` e il confronto resta fra interi:
///
/// ```text
///   output_rows  >  base_rows * mantissa * 2^esponente
/// ```
///
/// Una soglia `(base_rows as f64) * factor` arrotonderebbe i conteggi oltre
/// `2^53` e lascerebbe passare un'espansione oltre la soglia.
///
/// Un fattore non finito o non positivo e' **fail-closed** (risponde `true`):
/// [`Limits::validate`] lo esclude a monte, ma una soglia non ordinabile non
/// puo' dire che il limite sia rispettato.
///
/// Su tutta la base `u64` la funzione è totale (`base * mantissa <= 2^117`).
/// La variante interna `expansion_exceeded_wide` prende una base a 128 bit e
/// non è pubblica, perché fuori dal suo dominio il prodotto può traboccare.
#[must_use]
pub fn expansion_exceeded(output_rows: u64, base_rows: u64, factor: f64) -> bool {
    expansion_exceeded_wide(output_rows, u128::from(base_rows), factor)
}

/// Come [`expansion_exceeded`], con base a 128 bit.
///
/// **Dominio:** `base_rows < 2^66`, cioe' la somma di due conteggi a 64 bit —
/// l'unica cosa che i vincoli binari costruiscono. Li' `base * mantissa <
/// 2^119` e il prodotto non trabocca mai.
pub(crate) fn expansion_exceeded_wide(output_rows: u64, base_rows: u128, factor: f64) -> bool {
    if !factor.is_finite() || factor <= 0.0 {
        // Fail-closed: soglia non ordinabile, nessuna espansione dichiarabile
        // accettabile. Con `output_rows == 0` non c'e' alcuna espansione da
        // rifiutare e la risposta resta `false`.
        return output_rows > 0;
    }
    let bits = factor.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    let (mantissa, exponent) = if raw_exponent == 0 {
        (fraction, -1074_i32)
    } else {
        (fraction | (1_u64 << 52), raw_exponent - 1075)
    };
    let Some(scaled) = base_rows.checked_mul(u128::from(mantissa)) else {
        // Irraggiungibile sul dominio dichiarato. Se lo diventasse, con
        // esponente negativo la soglia potrebbe essere PICCOLA e rispondere
        // «non superato» sarebbe aprire il limite: si risponde fail-closed.
        return true;
    };
    let output = u128::from(output_rows);
    if scaled == 0 {
        // Soglia NULLA: la base e' vuota. La metrica e' allora infinita se
        // l'output non lo e' (espansione da niente) e zero altrimenti — la
        // stessa convenzione di `JoinExpansion::compute`. Va deciso qui,
        // prima degli spostamenti: con un fattore enorme l'esponente esce
        // dai 128 bit e la guardia sullo shift risponderebbe «non superato»
        // anche con la soglia a zero.
        return output_rows > 0;
    }
    if exponent >= 0 {
        let shift = u32::try_from(exponent).unwrap_or(u32::MAX);
        scaled
            .checked_shl(shift)
            .filter(|_| shift < 128 && scaled.leading_zeros() >= shift)
            .is_some_and(|threshold| output > threshold)
    } else {
        let shift = u32::try_from(-exponent).unwrap_or(u32::MAX);
        // `output << shift  >  scaled`: se lo spostamento esce da u128 il
        // lato sinistro e' certamente maggiore.
        output
            .checked_shl(shift)
            .filter(|_| shift < 128 && output.leading_zeros() >= shift)
            .map_or(output != 0, |shifted| shifted > scaled)
    }
}

#[cfg(test)]
mod tests {
    use super::{expansion_exceeded, Limits, PlanLimits, DEFAULT_MAX_GOVERNED_MEMORY_BYTES};

    #[test]
    fn il_default_governato_viene_dall_autorita_ed_e_quello_pubblicato() {
        // Il valore è PUBBLICO: è il budget di ogni piano che non lo
        // dichiara. Cambiarlo è una modifica osservabile, non un dettaglio,
        // e questo test lo dice a chi ci prova.
        assert_eq!(
            DEFAULT_MAX_GOVERNED_MEMORY_BYTES, 536_870_912,
            "512 MiB, ed e' il numero che la documentazione pubblica"
        );
        // Il default della struttura viene DA li', non da un letterale
        // uguale per coincidenza.
        assert_eq!(
            Limits::default().max_governed_memory_bytes,
            DEFAULT_MAX_GOVERNED_MEMORY_BYTES
        );
    }

    #[test]
    fn un_limite_di_piano_a_zero_e_rifiutato() {
        // Gli otto tetti strutturali passano da `validate`: se non ci
        // passassero, un limite a zero entrerebbe senza che nessuno lo dica.
        fn azzerato(muta: impl FnOnce(&mut PlanLimits)) -> Limits {
            let mut limits = Limits::default();
            muta(&mut limits.plan);
            limits
        }
        // Incompatibili con qualunque documento valido: rifiutati.
        let rifiutati = [
            (
                "plan.max_plan_json_bytes",
                azzerato(|p| p.max_plan_json_bytes = 0),
            ),
            ("plan.max_inputs", azzerato(|p| p.max_inputs = 0)),
            (
                "plan.max_identifier_bytes",
                azzerato(|p| p.max_identifier_bytes = 0),
            ),
        ];
        for (nome, limits) in rifiutati {
            let errore = limits
                .validate()
                .expect_err("un limite di piano a zero deve essere rifiutato");
            let testo = errore.to_string();
            assert!(testo.contains(nome), "{nome}: {testo}");
        }
        // Tetti sui NODI: zero è legittimo, lo rispetta un piano senza passi.
        let ammessi = [
            azzerato(|p| p.max_plan_nodes = 0),
            azzerato(|p| p.max_plan_edges = 0),
            azzerato(|p| p.max_plan_depth = 0),
            azzerato(|p| p.max_fan_out = 0),
            azzerato(|p| p.max_config_bytes_per_node = 0),
        ];
        for limits in ammessi {
            assert!(
                limits.validate().is_ok(),
                "un tetto sui nodi a zero e' rispettabile da un pass-through"
            );
        }
        // I default restano validi: la regola nuova non ne rifiuta nessuno.
        assert!(Limits::default().validate().is_ok());
    }

    #[test]
    fn il_predicato_e_totale_su_tutto_il_dominio_dei_conteggi() {
        /// Ultimo intero con un `f64` esatto: oltre, il rapporto arrotonda.
        const DUE_53: u64 = 1 << 53;

        // Base e output sono conteggi di righe: `u64`. Su quel dominio
        // `base * mantissa <= 2^117` non trabocca mai, quindi non esistono
        // combinazioni in cui la funzione debba «arrendersi».
        assert!(!expansion_exceeded(u64::MAX, u64::MAX, 1.0));
        assert!(!expansion_exceeded(0, 0, 1.0));
        // Da input vuoto qualunque output e' espansione infinita.
        assert!(expansion_exceeded(1, 0, f64::MAX));
        // Fattori minuscoli: la soglia e' piccola e il superamento va visto.
        assert!(expansion_exceeded(1, u64::MAX, f64::MIN_POSITIVE));
        assert!(expansion_exceeded(2, 1, 1.999_999_999_999_999_8));
        assert!(!expansion_exceeded(2, 1, 2.0));
        // Fattori enormi: nessun conteggio a 64 bit li raggiunge.
        assert!(!expansion_exceeded(u64::MAX, 1, f64::MAX));
        assert!(!expansion_exceeded(u64::MAX, 1, 1.9e19));
        // Ma la soglia resta esatta anche a ridosso del fondo scala.
        assert!(expansion_exceeded(u64::MAX, 1, 1.8e19));
        // Esattezza sopra 2^53, il motivo per cui non si passa da `f64`.
        assert!(expansion_exceeded(DUE_53 + 1, DUE_53, 1.0));
        assert!(!expansion_exceeded(DUE_53, DUE_53, 1.0));
        // Un fattore non ordinabile e' FAIL-CLOSED: non si dichiara
        // rispettato un limite che non si sa confrontare.
        assert!(expansion_exceeded(u64::MAX, 1, f64::NAN));
        assert!(expansion_exceeded(u64::MAX, 1, 0.0));
        assert!(expansion_exceeded(1, 1, -1.0));
        // Senza output non c'e' espansione da rifiutare, nemmeno qui.
        assert!(!expansion_exceeded(0, 1, f64::NAN));
    }
}
