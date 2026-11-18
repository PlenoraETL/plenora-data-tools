//! Certificazione dell'heap richiesto al `GlobalAlloc`, non RSS o pagine OS.
//! Il gruppo comprende solo la chiamata e il drop dell'output. Gli ingressi
//! presi in prestito sono costruiti prima: esclusione incrementale zero.
//! L'overhead misurabile dell'allocatore e' il picco della somma viva di
//! `wrapped_size - object_size` (header e padding del tracker), non una soglia
//! empirica. Metadati/pagine privati di System non sono osservabili da
//! `GlobalAlloc` e non fanno parte di questa misura. Realloc del tracker usa
//! alloc/copy/dealloc: il picco include entrambi i buffer, conservativamente.
//! Il minimo bilancio di successo, cercato fino al singolo byte, e' il picco
//! delle ricevute: tutte le riserve hanno lo stesso percorso deterministico.
//! Ogni misura verifica anche il ritorno dei byte vivi a zero dopo il drop.
#![deny(clippy::disallowed_methods)]

use geo::{Geometry, LineString, MultiLineString};
use plenora_kernels_geo::extended_algorithms::{polygonize_with_limits, split_line_with_limits};
use plenora_kernels_geo::predicates::{relate_matrix_with_limits, PredicateError, RelateLimits};
use std::alloc::System;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracking_allocator::{
    AllocationGroupId, AllocationGroupToken, AllocationRegistry, AllocationTracker,
};

#[global_allocator]
static ALLOCATOR: tracking_allocator::Allocator<System> = tracking_allocator::Allocator::system();
static GROUP: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static OVERHEAD: AtomicUsize = AtomicUsize::new(0);
static OVERHEAD_PEAK: AtomicUsize = AtomicUsize::new(0);

struct HeapProbe;
impl AllocationTracker for HeapProbe {
    fn allocated(&self, _: usize, object: usize, wrapped: usize, group: AllocationGroupId) {
        if group.as_usize().get() == GROUP.load(Ordering::SeqCst) {
            let live = LIVE.fetch_add(wrapped, Ordering::SeqCst) + wrapped;
            PEAK.fetch_max(live, Ordering::SeqCst);
            let overhead =
                OVERHEAD.fetch_add(wrapped - object, Ordering::SeqCst) + wrapped - object;
            OVERHEAD_PEAK.fetch_max(overhead, Ordering::SeqCst);
        }
    }
    fn deallocated(
        &self,
        _: usize,
        object: usize,
        wrapped: usize,
        source: AllocationGroupId,
        _: AllocationGroupId,
    ) {
        if source.as_usize().get() == GROUP.load(Ordering::SeqCst) {
            LIVE.fetch_sub(wrapped, Ordering::SeqCst);
            OVERHEAD.fetch_sub(wrapped - object, Ordering::SeqCst);
        }
    }
}

struct HeapSample {
    peak: u64,
    exclusion: u64,
}

fn measure<T>(
    call: impl FnOnce() -> Result<T, PredicateError>,
) -> (Result<(), PredicateError>, HeapSample) {
    assert_eq!(LIVE.load(Ordering::SeqCst), 0, "perdita precedente");
    assert_eq!(OVERHEAD.load(Ordering::SeqCst), 0);
    let mut token = AllocationGroupToken::register().expect("gruppo disponibile");
    // Inizializza lo stack TLS prima dell'intervallo misurato.
    drop(token.enter());
    GROUP.store(token.id().as_usize().get(), Ordering::SeqCst);
    PEAK.store(0, Ordering::SeqCst);
    OVERHEAD_PEAK.store(0, Ordering::SeqCst);
    let guard = token.enter();
    let result = call().map(drop);
    drop(guard);
    let sample = HeapSample {
        peak: u64::try_from(PEAK.load(Ordering::SeqCst)).expect("picco u64"),
        exclusion: u64::try_from(OVERHEAD_PEAK.load(Ordering::SeqCst)).expect("overhead u64"),
    };
    assert_eq!(LIVE.load(Ordering::SeqCst), 0, "perdita nella chiamata");
    assert_eq!(OVERHEAD.load(Ordering::SeqCst), 0);
    (result, sample)
}

fn certify<T>(
    case: &str,
    operation: &str,
    call: impl Fn(RelateLimits) -> Result<T, PredicateError>,
) {
    let limits = RelateLimits {
        max_reserved_bytes: u64::MAX,
        ..RelateLimits::default()
    };
    let (result, full) = measure(|| call(limits));
    result.expect("corpus valido con risorse sufficienti");
    assert!(full.peak > 0, "misuratore attivo");
    let mut low = 0;
    // Una prenotazione puo' essere conservativa: la ricerca non assume che
    // il picco reale maggiori quello contabile.
    let mut high = RelateLimits::default().max_reserved_bytes;
    call(RelateLimits {
        max_reserved_bytes: high,
        ..limits
    })
    .expect("estremo superiore ammesso");
    while low < high {
        let mid = low + (high - low) / 2;
        match call(RelateLimits {
            max_reserved_bytes: mid,
            ..limits
        }) {
            Ok(value) => {
                drop(value);
                high = mid;
            }
            Err(PredicateError::ReservationLimit) => low = mid + 1,
            Err(error) => panic!("errore diverso da risorsa nella ricerca della soglia: {error}"),
        }
    }
    let counted = high;
    let (exact, exact_heap) = measure(|| {
        call(RelateLimits {
            max_reserved_bytes: counted,
            ..limits
        })
    });
    exact.expect("soglia esatta");
    assert_eq!(full.peak, exact_heap.peak, "percorso deterministico");
    assert!(
        full.peak.saturating_sub(counted) <= full.exclusion,
        "{case}/{operation}: reale={} contato={counted} esclusione={}",
        full.peak,
        full.exclusion
    );
    let short = counted.checked_sub(1).expect("picco non nullo");
    let (failure, failed_heap) = measure(|| {
        call(RelateLimits {
            max_reserved_bytes: short,
            ..limits
        })
    });
    assert!(matches!(failure, Err(PredicateError::ReservationLimit)));
    assert!(
        failed_heap.peak.saturating_sub(short)
            <= RelateLimits::MAX_VALUE_BYTES + failed_heap.exclusion,
        "{case}/{operation}: picco rifiuto oltre il massimo per valore"
    );
    println!("{case}/{operation}: contato={counted}, reale={}, esclusione={}, bilancio_rifiuto={short}, reale_rifiuto={}, esclusione_rifiuto={}, vivi_finali=0", full.peak, full.exclusion, failed_heap.peak, failed_heap.exclusion);
}

fn segment(a: (f64, f64), b: (f64, f64)) -> LineString<f64> {
    LineString::from(vec![a, b])
}

fn extreme_linework(extent: f64) -> Vec<LineString<f64>> {
    vec![
        LineString::from(vec![
            (-extent, -extent),
            (extent, -extent),
            (extent, extent),
            (-extent, extent),
            (-extent, -extent),
        ]),
        segment((-extent, 0.0), (extent, 0.0)),
        segment((0.0, -extent), (0.0, extent)),
    ]
}

#[test]
#[deny(clippy::disallowed_methods)]
fn real_heap_certification() {
    AllocationRegistry::set_global_tracker(HeapProbe).expect("unico tracker");
    AllocationRegistry::enable_tracking();
    let bytes = std::mem::size_of::<RelateLimits>();
    let (probe, measured) = measure(|| Ok(std::hint::black_box(vec![0_u8; bytes])));
    probe.expect("allocazione nota");
    assert_eq!(
        measured.peak - measured.exclusion,
        u64::try_from(bytes).expect("byte u64")
    );
    let mut grid = Vec::new();
    for i in 0..4 {
        let x = f64::from(i);
        grid.push(segment((x, 0.0), (x, 3.0)));
        grid.push(segment((0.0, x), (3.0, x)));
    }
    let mut comb = vec![segment((0.0, 1.0), (33.0, 1.0))];
    for i in 1..33 {
        let x = f64::from(i);
        comb.push(segment((x, -1.0), (x, 1.0)));
    }
    let sparse = (0..256)
        .map(|i| {
            let x = f64::from(i).mul_add(4.0, 16.0);
            segment((x, 0.0), (x + 1.0, 1.0))
        })
        .collect();
    let tiny = f64::from_bits(1);
    for (name, source, lines) in [
        ("reticolo", segment((0.0, 1.5), (3.0, 1.5)), grid),
        ("pettine", segment((0.0, 0.0), (33.0, 0.0)), comb),
        ("sparso256", segment((0.0, 0.0), (1.0, 0.0)), sparse),
        (
            "subnormali",
            segment((-tiny, 0.0), (tiny, 0.0)),
            extreme_linework(tiny),
        ),
        (
            "esponenti_massimi",
            segment((-f64::MAX, 0.0), (f64::MAX, 0.0)),
            extreme_linework(f64::MAX),
        ),
    ] {
        let linework = Geometry::MultiLineString(MultiLineString(lines));
        let operand = Geometry::LineString(source.clone());
        certify(name, "relate", |limits| {
            relate_matrix_with_limits(&operand, &linework, limits, None)
        });
        certify(name, "split", |limits| {
            split_line_with_limits(&source, &linework, limits, None)
        });
        certify(name, "polygonize", |limits| {
            polygonize_with_limits(&linework, limits, None)
        });
    }
    // Carico sparso maggiore per le API che non classificano tutte le celle
    // DE-9IM: il corpus comune sopra resta identico per le tre operazioni.
    let source = segment((0.0, 0.0), (1.0, 0.0));
    let lines = (0..4096)
        .map(|i| {
            let x = f64::from(i).mul_add(4.0, 16.0);
            segment((x, 0.0), (x + 1.0, 1.0))
        })
        .collect();
    let linework = Geometry::MultiLineString(MultiLineString(lines));
    certify("sparso4096", "split", |limits| {
        split_line_with_limits(&source, &linework, limits, None)
    });
    certify("sparso4096", "polygonize", |limits| {
        polygonize_with_limits(&linework, limits, None)
    });
}
