//! Le tre famiglie di limiti (decisione D19, errori-e-limiti.md).
//!
//! - [`RowLimits`]: espansioni logiche dei dati;
//! - [`PlanLimits`]: complessità del piano, applicati durante il parsing;
//! - [`Limits`]: contenitore unico (dati/runtime, piano, stringhe).

use serde::Deserialize;

use crate::error::{PlenoraError, Result};

/// Limiti di righe, semanticamente distinti (errori-e-limiti.md).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowLimits {
    /// Righe lette da ciascuna sorgente di input.
    pub max_input_rows: u64,
    /// Righe dell'output finale.
    pub max_output_rows: u64,
    /// Righe su qualunque arco intermedio del DAG.
    pub max_rows_per_edge: u64,
    /// Fattore di espansione output/input; la base è per classe di operazione.
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

/// Budget di memoria governato applicato quando il piano non lo dichiara:
/// **536 870 912 byte**, cioe' 512 MiB.
///
/// # Perche' e' pubblica
///
/// Il contratto `Plan Budget 1.0` (`PLAN-013`) obbliga il componente a
/// **pubblicare** il default che applica. Senza, la regola per cui il tetto
/// del dominio deve essere `>=` al budget governato **effettivo** non e'
/// verificabile da chi invia un piano prima di inviarlo: il vincolo si
/// scoprirebbe solo al rifiuto.
///
/// # Perche' e' una costante e non un letterale ripetuto
///
/// Il valore era scritto due volte in codice di produzione — qui e nei
/// default dei kernel tabellari — e le due copie potevano divergere senza che
/// nulla lo notasse. Non e' un rischio ipotetico introdotto da una versione
/// futura del formato: e' gia' possibile oggi, e sarebbe emerso come due
/// componenti dello stesso processo che applicano budget diversi allo stesso
/// piano.
///
/// Chi aggiunge un terzo sito deve puntare qui.
pub const DEFAULT_MAX_GOVERNED_MEMORY_BYTES: u64 = DEFAULT_MAX_GOVERNED_MEMORY_BYTES_USIZE as u64;

/// Lo stesso default per chi lo tiene in `usize`.
///
/// # Perche' il letterale sta QUI e non nella forma `u64`
///
/// La derivazione va nel verso che non puo' perdere nulla. Partire dal `u64`
/// e convertire con `as usize` troncherebbe su un target a 32 bit, e un
/// ripiego a [`usize::MAX`] o a qualunque altro valore sarebbe peggio del
/// troncamento: quel target applicherebbe un budget **diverso** da quello
/// pubblicato, cioe' proprio la divergenza che questa costante chiude.
///
/// Nel verso scelto la conversione e' un allargamento e non ha casi
/// degeneri, quindi non serve nessun presidio: `usize as u64` puo' perdere
/// qualcosa solo dove `usize` supera i 64 bit, e li' questo valore ci sta
/// comunque.
///
/// # Chi esclude i target che non lo rappresentano
///
/// Il compilatore, da solo. Il valore entra in un `usize` a 32 bit —
/// 536 870 912 sta sotto 4 294 967 295 — quindi restano fuori solo i target
/// piu' stretti, dove **il letterale stesso** va in overflow in valutazione
/// costante, che e' un errore di compilazione. L'esclusione e' esplicita e
/// non ha bisogno che la scriva io.
///
/// Una prima stesura aggiungeva qui un controllo di compilazione che
/// confrontava le due forme. Era una **tautologia**: la costante `u64` e'
/// definita come la conversione di questa, quindi il confronto era vero per
/// costruzione e non poteva fallire. La sua documentazione sosteneva il
/// contrario, ed e' esattamente il tipo di presidio che sembra verificare e
/// non verifica.
pub const DEFAULT_MAX_GOVERNED_MEMORY_BYTES_USIZE: usize = 512 * 1024 * 1024;

/// Quota di spill su disco applicata quando il piano non la dichiara:
/// **8 GiB**.
///
/// Stessa classe di [`DEFAULT_MAX_GOVERNED_MEMORY_BYTES`]: era scritta due
/// volte in codice di produzione, qui e nei default dei kernel tabellari, e
/// il percorso legacy porta la seconda copia negli override del piano. Due
/// letterali uguali per convenzione, non per costruzione.
pub const DEFAULT_MAX_TEMP_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Partizioni di spill applicate quando il piano non le dichiara: **64**.
///
/// Stessa classe delle due precedenti. Vive in `u32` perche' e' cosi' che il
/// piano la dichiara; i kernel la tengono in `usize` e la conversione al
/// confine e' gia' totale e verificata (`legacy_limits_override`).
pub const DEFAULT_SPILL_PARTITIONS: u32 = 64;

/// Limiti alla complessità del piano, applicati durante il parsing (errori-e-limiti.md).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanLimits {
    pub max_plan_json_bytes: usize,
    pub max_plan_nodes: usize,
    pub max_plan_edges: usize,
    pub max_plan_depth: usize,
    pub max_fan_out: usize,
    pub max_inputs: usize,
    pub max_config_bytes_per_node: usize,
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

/// Contenitore unico dei limiti (architettura.md).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(flatten)]
    pub rows: RowLimits,
    #[serde(default)]
    pub plan: PlanLimits,
    /// Budget memoria globale del piano (perimetro: architettura.md#memoria).
    pub max_governed_memory_bytes: u64,
    /// Quota spill su disco.
    pub max_temp_bytes: u64,
    /// Partizioni di spill.
    pub spill_partitions: u32,
    /// Grado massimo di parallelismo (risorsa, non proprietà dei nodi).
    ///
    /// `0` significa «numero di core logici». Il tetto e' applicato
    /// dimensionando il pool Rayon del processo
    /// (`plenora_engine::parallelism::configure`), che e' l'unica leva che
    /// copre insieme tutti i percorsi paralleli dei kernel; e' quindi una
    /// configurazione **di processo**, fatta dal binario prima
    /// dell'esecuzione, non una proprieta' per-piano.
    pub max_parallelism: u32,
    /// Limite per cella WKB (da geo-tools-arrow).
    pub max_wkb_cell_bytes: u64,
    /// Limite payload complessivo in lettura.
    pub max_payload_bytes: u64,
    /// Numero massimo di batch per arco.
    pub max_batches: u64,
    /// Profondità geometrica massima (componenti annidate).
    pub max_geometry_depth: u32,
    /// Limite per singola stringa.
    pub max_string_bytes: usize,
    /// Limite per pattern regex.
    pub max_regex_bytes: usize,
}

impl Limits {
    /// Numero minimo di partizioni di spill: con una sola partizione il
    /// merge non ha nulla da fondere e lo spill non riduce la memoria.
    pub const MIN_SPILL_PARTITIONS: u32 = 2;

    /// Numero massimo di partizioni di spill: oltre, i file aperti e i buffer
    /// per partizione costano piu' della memoria che lo spill libera.
    pub const MAX_SPILL_PARTITIONS: u32 = 4_096;

    /// Validazione dei limiti effettivi, in un punto solo.
    ///
    /// Un limite fuori dominio va RIFIUTATO, non corretto. Il preparer
    /// applicava `spill_partitions.max(2)` in silenzio: un piano che
    /// dichiarava `spill_partitions: 1` veniva eseguito con 2, cioe' con
    /// limiti diversi da quelli dichiarati e senza che nulla lo segnalasse.
    /// Per un componente fail-closed la correzione silenziosa di un input e'
    /// il verso sbagliato, e sparpagliata sui punti d'uso non copriva i piani
    /// che quel percorso non attraversano (i piani solo-geo).
    ///
    /// Le regole ricalcano quelle del motore tabellare legacy — un limite a
    /// zero rende il componente incapace di fare alcunche', e va detto subito
    /// invece che scoperto al primo batch — e vi aggiungono
    /// `max_expansion_factor`, che dev'essere finito e positivo: un `NaN`
    /// costruito via API Rust rende falso ogni confronto successivo e apre il
    /// limite invece di chiuderlo.
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
        if self.max_temp_bytes == 0 {
            return nullo("max_temp_bytes");
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
        // Limiti di piano: si rifiuta lo zero SOLO dove nessun documento
        // valido potrebbe rispettarlo.
        //
        // La distinzione non e' pedanteria. I tetti sui NODI — nodi, archi,
        // profondita', fan-out, byte di config — valgono legittimamente zero
        // per un piano **pass-through** (`nodes: []`, `output` che riferisce
        // un input), che questo formato documenta e testa come valido: una
        // policy che ammette solo pass-through e' una policy sensata.
        // Rifiutarli in blocco la rendeva impossibile, e faceva di peggio —
        // il parse accettava il piano (zero nodi non superano un tetto di
        // zero) e la validazione dei limiti lo rifiutava dopo: due verdetti
        // discordi sullo stesso documento.
        //
        // Restano incompatibili con qualunque documento: un piano ha dei
        // byte, ha almeno un input da cui leggere, e ha identificatori non
        // vuoti.
        if self.plan.max_plan_json_bytes == 0 {
            return nullo("plan.max_plan_json_bytes");
        }
        if self.plan.max_inputs == 0 {
            return nullo("plan.max_inputs");
        }
        if self.plan.max_identifier_bytes == 0 {
            return nullo("plan.max_identifier_bytes");
        }
        if self.spill_partitions < Self::MIN_SPILL_PARTITIONS
            || self.spill_partitions > Self::MAX_SPILL_PARTITIONS
        {
            return Err(PlenoraError::InvalidPlan(format!(
                "spill_partitions {} fuori dall'intervallo {}..={}",
                self.spill_partitions,
                Self::MIN_SPILL_PARTITIONS,
                Self::MAX_SPILL_PARTITIONS
            )));
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
            max_temp_bytes: DEFAULT_MAX_TEMP_BYTES,
            spill_partitions: DEFAULT_SPILL_PARTITIONS,
            max_parallelism: 0, // 0 = numero di core logici
            max_wkb_cell_bytes: 64 * 1024 * 1024,
            max_payload_bytes: 16 * 1024 * 1024 * 1024,
            max_batches: 65_536,
            max_geometry_depth: 64,
            max_string_bytes: 16 * 1024 * 1024,
            max_regex_bytes: 64 * 1024,
        }
    }
}

/// `true` se `output_rows` supera `base_rows * factor`.
///
/// NESSUN conteggio passa per `f64`. Il fattore e' un double per contratto
/// (errori-e-limiti.md), quindi lo si decompone in `mantissa * 2^esponente` e il confronto
/// resta fra interi:
///
/// ```text
///   output_rows  >  base_rows * mantissa * 2^esponente
/// ```
///
/// Calcolare la soglia come `(base_rows as f64) * factor` arrotonda i
/// CONTEGGI: con `base_rows = output_rows = 2^53+1` e fattore `1` una
/// cardinalita' valida veniva rifiutata, e — nel verso opposto, quello che
/// conta per un limite — `base = 2^53`, `output = 2^53+1` e fattore `1`
/// collassavano sullo stesso double, lasciando passare un'espansione oltre
/// la soglia.
///
/// Un fattore non finito o non positivo e' **fail-closed**: la funzione
/// risponde `true`, cioe' «limite superato».
///
/// [`Limits::validate`] esclude quei fattori a monte, quindi il caso non
/// dovrebbe presentarsi; se si presenta, la soglia non e' ordinabile e non si
/// puo' dire che il limite sia rispettato. Rispondere `false` — «tutto bene»
/// — su una soglia che non si sa confrontare e' esattamente il modo in cui un
/// limite smette di limitare: un `NaN` in configurazione avrebbe aperto ogni
/// espansione invece di chiuderla.
///
/// La base e' un `u64` — un conteggio di righe — e su tutto quel dominio la
/// funzione e' totale: `base * mantissa <= 2^64 * 2^53 = 2^117` non trabocca
/// mai. La variante interna [`expansion_exceeded_wide`] prende una base a
/// 128 bit per la somma dei due lati di un'operazione binaria; non e'
/// pubblica proprio perche' fuori da quel dominio ristretto il prodotto puo'
/// traboccare, e una funzione pubblica con una precondizione taciuta e' una
/// trappola.
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
        // dai 128 bit e la guardia sullo shift rispondeva «non superato»
        // anche se la soglia era zero.
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
    use super::{
        expansion_exceeded, Limits, PlanLimits, DEFAULT_MAX_GOVERNED_MEMORY_BYTES,
        DEFAULT_MAX_GOVERNED_MEMORY_BYTES_USIZE, DEFAULT_MAX_TEMP_BYTES, DEFAULT_SPILL_PARTITIONS,
    };

    #[test]
    fn il_default_governato_viene_dall_autorita_ed_e_quello_pubblicato() {
        // Il valore e' PUBBLICO: `Plan Budget 1.0` (`PLAN-013`) obbliga a
        // pubblicarlo, perche' senza di esso chi invia un piano non puo'
        // verificare prima di inviare che il tetto del dominio sia `>=` al
        // budget effettivo. Cambiarlo e' una modifica osservabile, non un
        // dettaglio, e questo test lo dice a chi ci prova.
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
        // Gli altri due default del gruppo memoria sono pubblicati nella
        // stessa tabella di `piano-v5.md`, e valgono le stesse due cose:
        // il numero e' quello documentato, e la struttura lo prende da qui.
        assert_eq!(DEFAULT_MAX_TEMP_BYTES, 8_589_934_592, "8 GiB");
        assert_eq!(DEFAULT_SPILL_PARTITIONS, 64);
        let predefiniti = Limits::default();
        assert_eq!(predefiniti.max_temp_bytes, DEFAULT_MAX_TEMP_BYTES);
        assert_eq!(predefiniti.spill_partitions, DEFAULT_SPILL_PARTITIONS);
    }

    #[test]
    fn la_conversione_a_usize_e_esatta() {
        // Nessuna saturazione e nessun ripiego: se `usize` non
        // rappresentasse il valore, `_CONVERSIONE_ESATTA` non compilerebbe e
        // questo test non esisterebbe. Qui si verifica che, dove compila, le
        // due forme dicano lo stesso numero.
        assert_eq!(
            DEFAULT_MAX_GOVERNED_MEMORY_BYTES_USIZE as u64, DEFAULT_MAX_GOVERNED_MEMORY_BYTES,
            "la conversione non perde nulla"
        );
    }

    #[test]
    fn un_limite_di_piano_a_zero_e_rifiutato() {
        // Gli otto tetti strutturali restavano fuori da `validate`: un piano
        // poteva dichiararne uno a zero, entrare nella forma canonica e
        // quindi nel `plan_hash`, e nessuno lo diceva.
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
        // Tetti sui NODI: zero e' legittimo, lo rispetta un pass-through.
        // Rifiutarli qui darebbe un verdetto discorde da quello del parse,
        // che un piano senza nodi lo accetta.
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
