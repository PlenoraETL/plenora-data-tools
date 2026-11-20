//! O-M3: memoria richiesta all'allocatore durante Voronoi condiviso e adapter.
//! Input preesistente escluso; include temporanei delle dipendenze e output.
//! Misura del picco, non un limite di ammissione o una misura RSS.
#![deny(clippy::disallowed_methods)]
use geo::{CoordsIter, Geometry, Point};
use plenora_kernels_geo::advanced::{voronoi_cells, voronoi_cells_distinct};
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

#[test]
fn duplicate_circle_heap_curve() {
    AllocationRegistry::set_global_tracker(HeapProbe).unwrap();
    AllocationRegistry::enable_tracking();
    let precision = Precision::new(0.01).unwrap();
    for n in [128_u32, 256, 512] {
        let mut inputs = vec![Geometry::Point(Point::new(0.0, 0.0)); usize::try_from(n).unwrap()];
        inputs.extend((0..n).map(|i| {
            let angle = f64::from(i) * std::f64::consts::TAU / f64::from(n);
            Geometry::Point(Point::new(100.0 * angle.cos(), 100.0 * angle.sin()))
        }));
        let shared = measure(|| voronoi_cells_distinct(&inputs, 1024, precision).unwrap());
        let materialized = measure(|| voronoi_cells(&inputs, 1024, precision).unwrap());
        assert!(
            shared < materialized,
            "n={n}: shared={shared}, expanded={materialized}"
        );
        // Soglie del corpus fissate prima della misura, non ammissione del kernel.
        assert!(shared < 8_000_000);
        assert!(materialized < 32_000_000);
        println!("n={n}: shared={shared}, materialized={materialized}, final_live=0");
    }
    for side in [16_u32, 32, 64] {
        let input = Geometry::MultiPoint(geo::MultiPoint::from(
            (0..side)
                .flat_map(|x| (0..side).map(move |y| (f64::from(x), f64::from(y))))
                .collect::<Vec<_>>(),
        ));
        let peak = measure(|| {
            plenora_kernels_geo::extended_algorithms::delaunay(&input, 4096, 8192).unwrap()
        });
        assert!(peak < 32_000_000);
        println!("delaunay side={side}: peak={peak}, final_live=0");
        let coordinates: Vec<_> = input.coords_iter().collect();
        let bounded = measure(|| {
            plenora_kernels_geo::extended_algorithms::delaunay_points_with_limits(
                &coordinates,
                8192,
                plenora_kernels_geo::predicates::RelateLimits::default(),
                None,
            )
            .unwrap()
        });
        assert!(bounded < 32_000_000);
        let rejected = measure(|| {
            let result = plenora_kernels_geo::extended_algorithms::delaunay_points_with_limits(
                &coordinates,
                8192,
                plenora_kernels_geo::predicates::RelateLimits {
                    max_reserved_bytes: 1024,
                    ..Default::default()
                },
                None,
            );
            assert!(matches!(
                result,
                Err(
                    plenora_kernels_geo::extended_algorithms::DelaunayBuildError::Resource(
                        plenora_kernels_geo::predicates::PredicateError::ReservationLimit
                    )
                )
            ));
        });
        assert!(rejected < 4096);
        println!("bounded delaunay side={side}: peak={bounded}, rejected={rejected}, final_live=0");
    }
    AllocationRegistry::disable_tracking();
}
