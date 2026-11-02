use super::{CoordNum, Kernel, Orientation};
use crate::Coord;

use num_traits::{Float, NumCast};

#[path = "exact_orientation.rs"]
mod exact_orientation;

#[path = "orient2d_filtered.rs"]
mod orient2d_filtered;

/// Robust kernel that uses [fast robust
/// predicates](//www.cs.cmu.edu/~quake/robust.html) to
/// provide robust floating point predicates. Should only be
/// used with types that can _always_ be casted to `f64`
/// _without loss in precision_.
#[derive(Default, Debug)]
pub struct RobustKernel;

impl<T> Kernel<T> for RobustKernel
where
    T: CoordNum + Float,
{
    fn orient2d(p: Coord<T>, q: Coord<T>, r: Coord<T>) -> Orientation {
        use robust::{Coord, orient2d};

        let bits = [p.x, p.y, q.x, q.y, r.x, r.y]
            .map(|x| <f64 as NumCast>::from(x).unwrap().to_bits());
        if let Some(sign) = orient2d_filtered::orient2d_sign_bits_filtered(bits) {
            return match sign {
                -1 => Orientation::Clockwise,
                0 => Orientation::Collinear,
                1 => Orientation::CounterClockwise,
                _ => unreachable!("exact sign is -1, 0 or 1"),
            };
        }

        let orientation = orient2d(
            Coord {
                x: <f64 as NumCast>::from(p.x).unwrap(),
                y: <f64 as NumCast>::from(p.y).unwrap(),
            },
            Coord {
                x: <f64 as NumCast>::from(q.x).unwrap(),
                y: <f64 as NumCast>::from(q.y).unwrap(),
            },
            Coord {
                x: <f64 as NumCast>::from(r.x).unwrap(),
                y: <f64 as NumCast>::from(r.y).unwrap(),
            },
        );

        if orientation < 0. {
            Orientation::Clockwise
        } else if orientation > 0. {
            Orientation::CounterClockwise
        } else {
            Orientation::Collinear
        }
    }
}
