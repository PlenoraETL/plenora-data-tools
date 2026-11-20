//! O-M3/O-M4 e GOP-028: adapter 1:1, budget cumulativo e stop atomico.
use super::*;
use crate::noding::Work;
use crate::predicates::{PredicateError, RelateLimits};
use geo::CoordsIter;
use std::sync::atomic::AtomicBool;

fn fixture(copies: usize) -> VoronoiCells {
    // Anelli chiusi, buco e zeri con segno: la copia non normalizza i bit.
    let cell = Polygon::new(
        LineString::from(vec![(-0., 0.), (8., 0.), (8., 8.), (0., 8.), (-0., 0.)]),
        vec![LineString::from(vec![
            (2., 2.),
            (2., 3.),
            (3., 3.),
            (3., 2.),
            (2., 2.),
        ])],
    );
    VoronoiCells {
        cells: vec![cell],
        input_indices: vec![0; copies],
    }
}

fn bits(output: &[Geometry<f64>]) -> Vec<Vec<(u64, u64)>> {
    output
        .iter()
        .map(|g| {
            g.coords_iter()
                .map(|p| (p.x.to_bits(), p.y.to_bits()))
                .collect()
        })
        .collect()
}

#[test]
fn materialization_default_limits_scale_and_bits() {
    // Limiti fissati prima della misura: default comune; nessun rapporto
    // empirico byte/punto viene usato per ammettere una triangolazione.
    let mut previous = 0;
    for copies in [256, 512, 1024] {
        let cells = fixture(copies);
        let mut work = Work::new(RelateLimits::default(), None);
        let result = cells.materialize_in(&mut work).unwrap();
        assert_eq!(bits(&result), bits(&cells.materialize()));
        assert_eq!(work.nodes, u64::try_from(copies * 10).unwrap());
        assert!(work.peak_bytes() > previous);
        assert!(work.peak_bytes() < 2_000_000);
        assert!(work.steps < 100_000);
        previous = work.peak_bytes();
    }
}

#[test]
fn materialization_exact_budget_boundaries() {
    let cells = fixture(17);
    let mut measured = Work::new(RelateLimits::default(), None);
    drop(cells.materialize_in(&mut measured).unwrap());
    let steps = measured.steps;
    let bytes = measured.peak_bytes();
    let nodes = measured.nodes;
    for delta in [0, 1] {
        let limits = RelateLimits {
            max_steps: steps + delta,
            max_reserved_bytes: bytes + delta,
            max_nodes: nodes + delta,
            ..RelateLimits::default()
        };
        assert_eq!(
            bits(&cells.materialize_with_limits(limits, None).unwrap()),
            bits(&cells.materialize())
        );
    }
    assert!(matches!(
        cells.materialize_with_limits(
            RelateLimits {
                max_steps: steps - 1,
                ..RelateLimits::default()
            },
            None
        ),
        Err(PredicateError::WorkLimit)
    ));
    assert!(matches!(
        cells.materialize_with_limits(
            RelateLimits {
                max_reserved_bytes: bytes - 1,
                ..RelateLimits::default()
            },
            None
        ),
        Err(PredicateError::ReservationLimit)
    ));
    assert!(matches!(
        cells.materialize_with_limits(
            RelateLimits {
                max_nodes: nodes - 1,
                ..RelateLimits::default()
            },
            None
        ),
        Err(PredicateError::WorkLimit)
    ));
}

#[test]
fn materialization_cancel_and_timeout_after_real_work() {
    let cells = fixture(128);
    let mut measured = Work::new(RelateLimits::default(), None);
    drop(cells.materialize_in(&mut measured).unwrap());
    let midpoint = measured.steps / 2;
    for timeout in [false, true] {
        let flag = AtomicBool::new(false);
        let mut work = Work::new(RelateLimits::default(), Some(&flag));
        work.interrupt_at = Some((midpoint, timeout));
        let result = cells.materialize_in(&mut work);
        if timeout {
            assert!(matches!(result, Err(PredicateError::DeadlineExceeded)));
        } else {
            assert!(matches!(result, Err(PredicateError::Cancelled)));
        }
        assert_eq!(work.steps, midpoint);
        assert!(work.nodes > 0 && work.nodes < measured.nodes);
        assert!(work.peak_bytes() > 0);
        // Le celle condivise restano utilizzabili dopo lo scarto delle copie.
        assert_eq!(cells.materialize().len(), 128);
    }
}

#[test]
fn materialization_empty_checks_interruption() {
    let cells = fixture(0);
    let flag = AtomicBool::new(true);
    assert!(matches!(
        cells.materialize_with_limits(RelateLimits::default(), Some(&flag)),
        Err(PredicateError::Cancelled)
    ));
    assert!(matches!(
        cells.materialize_with_limits(
            RelateLimits {
                deadline: Some(std::time::Instant::now()),
                ..RelateLimits::default()
            },
            Some(&flag)
        ),
        Err(PredicateError::DeadlineExceeded)
    ));
}
