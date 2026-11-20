//! O-M3 / memoria 14: allocazioni reali e curva 10k/20k/40k/85k.
//! Input escluso, output incluso; byte richiesti totali e picco vivo del tracker.
#![deny(clippy::disallowed_methods)]
use geo::{Coord, Geometry, LineString, Polygon};
use plenora_kernels_geo::equality::{
    geo_equals_with_tolerance, normalize_geometry, GeometryComparison,
};
use std::alloc::System;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tracking_allocator::{
    AllocationGroupId, AllocationGroupToken, AllocationRegistry, AllocationTracker,
};

#[global_allocator]
static ALLOCATOR: tracking_allocator::Allocator<System> = tracking_allocator::Allocator::system();
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static GROUP: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static OVERHEAD: AtomicUsize = AtomicUsize::new(0);
static OVERHEAD_PEAK: AtomicUsize = AtomicUsize::new(0);

struct HeapProbe;
impl AllocationTracker for HeapProbe {
    fn allocated(&self, _: usize, object: usize, wrapped: usize, group: AllocationGroupId) {
        if group.as_usize().get() == GROUP.load(Ordering::SeqCst) {
            ALLOCATED.fetch_add(object, Ordering::SeqCst);
            CALLS.fetch_add(1, Ordering::SeqCst);
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

/// O-M3: nessuna soglia temporale dipendente dall'host; il gate verifica
/// allocazioni e byte lineari. Cinque ripetizioni riportano la mediana.
#[test]
fn linear_allocations_and_scale() {
    AllocationRegistry::set_global_tracker(HeapProbe).expect("unico tracker");
    AllocationRegistry::enable_tracking();
    for n in [10_000_usize, 20_000, 40_000, 85_000] {
        let mut coords = vec![Coord { x: 0.0, y: 0.0 }; n];
        coords[n - 1].x = 1.0;
        let input = Geometry::Polygon(Polygon::new(LineString(coords), vec![]));
        for comparison in [false, true] {
            let mut times = Vec::new();
            for _ in 0..5 {
                let mut token = AllocationGroupToken::register().expect("gruppo disponibile");
                drop(token.enter());
                GROUP.store(token.id().as_usize().get(), Ordering::SeqCst);
                PEAK.store(0, Ordering::SeqCst);
                OVERHEAD_PEAK.store(0, Ordering::SeqCst);
                ALLOCATED.store(0, Ordering::SeqCst);
                CALLS.store(0, Ordering::SeqCst);
                let guard = token.enter();
                let start = Instant::now();
                if comparison {
                    assert!(std::hint::black_box(geo_equals_with_tolerance(
                        std::hint::black_box(&input),
                        &input,
                        GeometryComparison::new(0.0).with_normalization(true)
                    )));
                } else {
                    let result = normalize_geometry(std::hint::black_box(&input));
                    std::hint::black_box(&result);
                    drop(result);
                }
                let elapsed = start.elapsed();
                drop(guard);
                assert_eq!(LIVE.load(Ordering::SeqCst), 0);
                assert_eq!(OVERHEAD.load(Ordering::SeqCst), 0);
                assert!(
                    ALLOCATED.load(Ordering::SeqCst)
                        <= (if comparison { 192 } else { 96 }) * n + 1024
                );
                assert!(
                    PEAK.load(Ordering::SeqCst) <= (if comparison { 192 } else { 96 }) * n + 1024
                );
                assert!(CALLS.load(Ordering::SeqCst) <= (if comparison { 16 } else { 8 }));
                times.push(elapsed.as_nanos());
            }
            times.sort_unstable();
            println!("comparison={comparison} n={n} median_ns={} allocated_bytes={} peak_wrapped_bytes={} allocations={} live=0", times[2],ALLOCATED.load(Ordering::SeqCst),PEAK.load(Ordering::SeqCst),CALLS.load(Ordering::SeqCst));
        }
    }
}
