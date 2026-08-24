//! Metriche di esecuzione: osservabilita' per nodo.
//!
//! Tre livelli — nodo, segmento fuso, esecuzione — e le somme sature che li
//! aggregano. La saturazione e' dichiarata (`saturated`), non silenziosa: un
//! contatore che sfonda `u64` deve dirlo, altrimenti la metrica smette di
//! essere un numero e diventa un'opinione.

use std::collections::BTreeMap;
use std::time::Duration;

use plenora_kernels_table::spill::SpillMetrics;

use crate::governor::MemoryMetrics;
use crate::prepare::SegmentMode;

/// Metriche di un nodo logico (presenti anche dentro ai segmenti fusi, osservabilita' per nodo).
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct NodeMetrics {
    /// Id canonico dell'operazione del nodo.
    pub operation: String,
    /// Righe in ingresso al nodo.
    pub rows_in: u64,
    /// Righe prodotte dal nodo.
    pub rows_out: u64,
    /// Batch in ingresso al nodo.
    pub batches_in: u64,
    /// Batch prodotti dal nodo.
    pub batches_out: u64,
    /// Byte Arrow in ingresso al nodo (metadati dei buffer, osservabilita' per nodo; architettura.md#memoria).
    pub bytes_in: u64,
    /// Byte Arrow prodotti dal nodo (metadati dei buffer, osservabilita' per nodo; architettura.md#memoria).
    pub bytes_out: u64,
    /// Wall time cumulato del kernel.
    pub wall_time: Duration,
}

/// Metriche di un segmento fisico.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SegmentMetrics {
    /// Modalita' fisica del segmento.
    pub mode: SegmentMode,
    /// Righe in ingresso al segmento.
    pub rows_in: u64,
    /// Righe prodotte dal segmento.
    pub rows_out: u64,
    /// Batch in ingresso al segmento.
    pub batches_in: u64,
    /// Batch prodotti dal segmento.
    pub batches_out: u64,
    /// Wall time cumulato del segmento.
    pub wall_time: Duration,
}

/// Metriche di un'esecuzione: per nodo logico, per segmento e sull'output.
///
/// Prefilled per tutti i nodi/segmenti del piano all'avvio: un nodo che non
/// ha ancora visto batch compare con contatori a zero. In caso di errore a
/// meta' stream le metriche restano consultabili fino al punto di
/// fallimento.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ExecutionMetrics {
    /// Per id nodo.
    pub nodes: BTreeMap<String, NodeMetrics>,
    /// Per id segmento.
    pub segments: BTreeMap<String, SegmentMetrics>,
    /// Righe pubblicate sull'output del piano.
    pub output_rows: u64,
    /// Batch pubblicati sull'output del piano.
    pub output_batches: u64,
    /// Righe processate complessivamente: somma delle righe in ingresso a
    /// ogni nodo del piano (errori-e-limiti.md: metrica obbligatoria, **non** limite v1 —
    /// dipende dal piano fisico, es. segmenti fusi).
    pub total_rows_processed: u64,
    /// Osservabilita' dei lease (architettura.md#memoria): snapshot del governor al momento
    /// della lettura delle metriche — lease vivi, byte trattenuti attuali e
    /// di picco, eta' del lease piu' vecchio.
    pub memory: MemoryMetrics,
    /// Metriche di spill aggregate sull'esecuzione (architettura.md#memoria, Fase 2B, spill generalizzato):
    /// byte scritti/letti sui file temporanei e numero di file
    /// materializzati dai percorsi `*_spilled` di sort/distinct/aggregate.
    /// Tutte zero se nessun nodo ha spillato.
    pub spill: SpillMetrics,
    /// Batch per cui il runner fuso geo e' ricaduto sul percorso non fuso
    /// per reservation governor fallita (architettura.md#geometrie D12.7): mai silenzioso —
    /// una pressione ricorrente e' osservabile qui invece di manifestarsi
    /// come rallentamento inspiegabile. Nessun errore nuovo: il risultato e'
    /// identico, cambia solo la scelta fisica.
    pub geo_fusion_fallbacks: u64,
    /// `true` se almeno un contatore ha raggiunto il proprio fondo scala e i
    /// valori qui sopra sono quindi LIMITI INFERIORI, non conteggi.
    ///
    /// I limiti di riga sono configurabili fino a `u64::MAX`, quindi la somma
    /// e' rappresentabile solo finche' i conteggi restano piccoli rispetto a
    /// quel fondo scala. Le tre uscite possibili erano: avvolgere (un
    /// contatore che riparte da zero e mente), andare in panico
    /// (`overflow-checks` e' attivo anche in release: una metrica che abbatte
    /// l'esecuzione) o saturare. Si satura, ma senza tacerlo: questo flag e'
    /// la differenza fra una degradazione dichiarata e una silenziosa.
    pub counters_saturated: bool,
}

/// Accumula una metrica saturando e DICHIARANDO la saturazione.
pub(super) const fn accumulate(counter: &mut u64, delta: u64, saturated: &mut bool) {
    if let Some(value) = counter.checked_add(delta) {
        *counter = value;
    } else {
        *counter = u64::MAX;
        *saturated = true;
    }
}

/// Come [`accumulate`], per i tempi cumulati.
pub(super) const fn accumulate_time(counter: &mut Duration, delta: Duration, saturated: &mut bool) {
    if let Some(value) = counter.checked_add(delta) {
        *counter = value;
    } else {
        *counter = Duration::MAX;
        *saturated = true;
    }
}

/// Somma di due conteggi di riga per una metrica, saturante e dichiarata.
pub(super) const fn sum_rows(left: u64, right: u64, saturated: &mut bool) -> u64 {
    if let Some(value) = left.checked_add(right) {
        return value;
    }
    *saturated = true;
    u64::MAX
}
