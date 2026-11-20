//! O-M3: memoria richiesta all'allocatore durante Voronoi condiviso e adapter.
//! Input preesistente escluso; include temporanei delle dipendenze e output.
//! Misura del picco, non un limite di ammissione o una misura RSS.
#![deny(clippy::disallowed_methods)]
use geo::{Geometry, Point};
use plenora_kernels_geo::advanced::voronoi_cells_distinct;
use plenora_kernels_geo::rust_backend::precision::Precision;
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

fn measure<T>(call: impl FnOnce() -> T) -> usize {
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    let mut token = AllocationGroupToken::register().unwrap();
    drop(token.enter());
    GROUP.store(token.id().as_usize().get(), Ordering::SeqCst);
    PEAK.store(0, Ordering::SeqCst);
    OVERHEAD_PEAK.store(0, Ordering::SeqCst);
    let guard = token.enter();
    drop(call());
    drop(guard);
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    PEAK.load(Ordering::SeqCst)
}

/// O-M3: picco fisico dell'adapter con limiti predefiniti e rilascio su errore.
#[test]
fn bounded_materialization_heap_curve() {
    AllocationRegistry::set_global_tracker(HeapProbe).unwrap();
    AllocationRegistry::enable_tracking();
    for n in [128_u32, 256, 512] {
        let mut inputs = vec![Geometry::Point(Point::new(0., 0.)); usize::try_from(n).unwrap()];
        inputs.extend((0..n).map(|i| {
            let angle = f64::from(i) * std::f64::consts::TAU / f64::from(n);
            Geometry::Point(Point::new(100. * angle.cos(), 100. * angle.sin()))
        }));
        let cells = voronoi_cells_distinct(&inputs, 1024, Precision::new(0.01).unwrap()).unwrap();
        let peak = measure(|| {
            cells
                .materialize_with_limits(
                    plenora_kernels_geo::predicates::RelateLimits::default(),
                    None,
                )
                .unwrap()
        });
        assert!(peak < 32_000_000);
        let refused = measure(|| {
            assert!(matches!(
                cells.materialize_with_limits(
                    plenora_kernels_geo::predicates::RelateLimits {
                        max_reserved_bytes: 32_768,
                        ..Default::default()
                    },
                    None
                ),
                Err(plenora_kernels_geo::predicates::PredicateError::ReservationLimit)
            ));
        });
        // Il tracker include header e padding delle allocazioni; il limite
        // fisico del corpus e' fissato separatamente dal budget governato.
        assert!(refused < 64_000);
        println!("n={n}: adapter_peak={peak}, refused_peak={refused}, final_live=0");
    }
    AllocationRegistry::disable_tracking();
}
