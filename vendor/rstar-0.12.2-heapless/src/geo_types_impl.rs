// Integrazioni da geo-types 0.7.20 (MIT OR Apache-2.0); vedi PROVENANCE.md.
use geo_types::{coord, Coord, Point, Line, LineString, Polygon, MultiPolygon, Rect, Triangle};
impl<T> crate::Point for Coord<T>
where
    T: ::num_traits::Float + crate::RTreeNum,
{
    type Scalar = T;

    const DIMENSIONS: usize = 2;

    #[inline]
    fn generate(mut generator: impl FnMut(usize) -> Self::Scalar) -> Self {
        coord! {
            x: generator(0),
            y: generator(1),
        }
    }

    #[inline]
    fn nth(&self, index: usize) -> Self::Scalar {
        match index {
            0 => self.x,
            1 => self.y,
            _ => unreachable!(),
        }
    }

    #[inline]
    fn nth_mut(&mut self, index: usize) -> &mut Self::Scalar {
        match index {
            0 => &mut self.x,
            1 => &mut self.y,
            _ => unreachable!(),
        }
    }
}

impl<T> crate::Point for Point<T>
where
    T: ::num_traits::Float + crate::RTreeNum,
{
    type Scalar = T;

    const DIMENSIONS: usize = 2;

    fn generate(mut generator: impl FnMut(usize) -> Self::Scalar) -> Self {
        Point::new(generator(0), generator(1))
    }

    fn nth(&self, index: usize) -> Self::Scalar {
        match index {
            0 => self.0.x,
            1 => self.0.y,
            _ => unreachable!(),
        }
    }
    fn nth_mut(&mut self, index: usize) -> &mut Self::Scalar {
        match index {
            0 => &mut self.0.x,
            1 => &mut self.0.y,
            _ => unreachable!(),
        }
    }
}


        impl<T> crate::RTreeObject for Line<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            type Envelope = crate::AABB<Point<T>>;

            fn envelope(&self) -> Self::Envelope {
                crate::AABB::from_corners(self.start_point(), self.end_point())
            }
        }

        impl<T> crate::PointDistance for Line<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            fn distance_2(&self, point: &Point<T>) -> T {
                let d = crate::private_utils::point_line_euclidean_distance(*point, *self);
                d.powi(2)
            }
        }


        impl<T> crate::RTreeObject for LineString<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            type Envelope = crate::AABB<Point<T>>;

            fn envelope(&self) -> Self::Envelope {
                use num_traits::Bounded;
                let bounding_rect = crate::private_utils::line_string_bounding_rect(self);
                match bounding_rect {
                    None => crate::AABB::from_corners(
                        Point::new(Bounded::min_value(), Bounded::min_value()),
                        Point::new(Bounded::max_value(), Bounded::max_value()),
                    ),
                    Some(b) => crate::AABB::from_corners(
                        Point::new(b.min().x, b.min().y),
                        Point::new(b.max().x, b.max().y),
                    ),
                }
            }
        }

        impl<T> crate::PointDistance for LineString<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            fn distance_2(&self, point: &Point<T>) -> T {
                let d = crate::private_utils::point_line_string_euclidean_distance(*point, self);
                if d == T::zero() {
                    d
                } else {
                    d.powi(2)
                }
            }
        }


        impl<T> crate::RTreeObject for Polygon<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            type Envelope = crate::AABB<Point<T>>;

            fn envelope(&self) -> Self::Envelope {
                self.exterior().envelope()
            }
        }


        impl<T> crate::RTreeObject for MultiPolygon<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            type Envelope = crate::AABB<Point<T>>;
            fn envelope(&self) -> Self::Envelope {
                use crate::Envelope;
                self.iter()
                    .map(|p| p.envelope())
                    .fold(crate::AABB::new_empty(), |a, b| a.merged(&b))
            }
        }


        impl<T> crate::RTreeObject for Rect<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            type Envelope = crate::AABB<Point<T>>;

            fn envelope(&self) -> Self::Envelope {
                crate::AABB::from_corners(self.min().into(), self.max().into())
            }
        }

        impl<T> crate::PointDistance for Rect<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            fn distance_2(&self, point: &Point<T>) -> T {
                use crate::RTreeObject;
                self.envelope().distance_2(point)
            }
        }


        impl<T> crate::RTreeObject for Triangle<T>
        where
            T: ::num_traits::Float + crate::RTreeNum,
        {
            type Envelope = crate::AABB<Point<T>>;

            fn envelope(&self) -> Self::Envelope {
                let bounding_rect =
                    crate::private_utils::get_bounding_rect(self.to_array()).unwrap();
                crate::AABB::from_corners(bounding_rect.min().into(), bounding_rect.max().into())
            }
        }
