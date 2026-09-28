use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;

pub trait IntArea<P: FloatPointCompatible<T>, T: FloatNumber> {
    /// The area of the `Path`. An empty path has zero area.
    /// - Returns: A positive double area if path is clockwise and negative double area otherwise.
    fn unsafe_int_area(&self, adapter: &FloatPointAdapter<P, T>) -> i64;
}

impl<P: FloatPointCompatible<T>, T: FloatNumber> IntArea<P, T> for [P] {
    fn unsafe_int_area(&self, adapter: &FloatPointAdapter<P, T>) -> i64 {
        let n = self.len();
        // The empty boundary contributes no terms to the area sum.
        if n == 0 {
            return 0;
        }
        let mut p0 = adapter.float_to_int(&self[n - 1]);
        let mut area: i64 = 0;

        for pi in self.iter() {
            let p1 = adapter.float_to_int(pi);
            let a = (p1.x as i64).wrapping_mul(p0.y as i64);
            let b = (p1.y as i64).wrapping_mul(p0.x as i64);
            area = area.wrapping_add(a).wrapping_sub(b);
            p0 = p1;
        }

        area
    }
}

#[cfg(test)]
mod tests {
    use crate::float::int_area::IntArea;
    use crate::path;
    use i_float::adapter::FloatPointAdapter;

    #[test]
    fn empty_path_has_zero_area() {
        let bounds = [[-1f64, -1f64], [1f64, 1f64]];
        let adapter = FloatPointAdapter::with_iter(bounds.iter());
        let empty: [[f64; 2]; 0] = [];
        assert_eq!(empty.unsafe_int_area(&adapter), 0);
    }

    #[test]
    fn test_0() {
        let square = path![[-1f32, -1f32], [1f32, -1f32], [1f32, 1f32], [-1f32, 1f32],];
        let adapter = FloatPointAdapter::with_iter(square.iter());

        let area = square.unsafe_int_area(&adapter);
        assert!(area < 0);
    }
}
