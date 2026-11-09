//! Superficie aggiunta da geo-types 0.7.20 ai tipi accettati dai kernel.

use geo::{Geometry, Point, Rect};
use rstar::{PointDistance, RTreeObject};

#[test]
fn nome_statico_e_indice_spaziale_dei_rettangoli() {
    let point = Point::new(0.0_f64, 0.0);
    assert_eq!(Geometry::Point(point).static_name(), "Point");
    let rectangle = Rect::new((0.0_f64, 2.0), (1.0, 3.0));
    assert_eq!(rectangle.envelope().lower(), Point::new(0.0, 2.0));
    assert_eq!(rectangle.distance_2(&point).to_bits(), 4.0_f64.to_bits());
    assert_eq!(
        rectangle.distance_2(&Point::new(0.5, 2.5)).to_bits(),
        0.0_f64.to_bits()
    );
}
