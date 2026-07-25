#![cfg(all(feature = "geos-backend", feature = "proj-backend"))]

use geo::{
    line_string, polygon, Coord, Geometry, GeometryCollection, Line, LineString, MultiLineString,
    MultiPoint, MultiPolygon, Point, Rect, Triangle,
};
use geozero::{CoordDimensions, ToWkb};
use plenora_kernels_geo::advanced::{voronoi_cells, AdvancedError};
use plenora_kernels_geo::analysis::{minimum_distances, nearest_matches, AnalysisError};
use plenora_kernels_geo::construction::{
    geometry_from_wkt, line_from_ordered_points, point_from_lon_lat, polygon_from_ordered_points,
    ConstructionError,
};
use plenora_kernels_geo::extended::{
    affine_transform, concave_hull, geodesic_line_length_m, hausdorff_distance,
    haversine_distance_m, rotate_about, ExtendedError,
};
use plenora_kernels_geo::extended_algorithms::{
    delaunay, densify, frechet_distance, geodesic_area_m2, geodesic_bearing_degrees,
    geometry_diagnostics, line_interpolate_point, line_merge, line_substring, snap_to_grid,
    split_line, ExtendedAlgorithmError,
};
use plenora_kernels_geo::geos_backend::{
    make_valid_wkb, polygonize_linework, split_polygon_by_linework, GeosBackendError, RepairMethod,
};
use plenora_kernels_geo::operations::{
    area, boundary, bounds, buffer_with_cap, distance, explode, length, perimeter,
    point_on_surface, simplify_with_policy, to_wkt, vertex_count, BufferCapStyle, OperationError,
    SimplifyPolicy,
};
use plenora_engine::geo_transport::pair_protocol::{read_pairs, write_pairs, PairProtocolError, MAX_PAIRS};
use plenora_kernels_geo::predicates::{evaluate, PredicateError, SpatialPredicate};
use plenora_kernels_geo::proj_backend::{reproject_geometry, ProjBackendError};
use plenora_engine::geo_transport::protocol::{
    FrameReader, FrameWriter, ProtocolError, MAX_GEOMETRY_BYTES, MAX_ROWS, PROTOCOL_MAGIC,
};
use plenora_kernels_geo::spatial_join::JoinPair;
use plenora_kernels_geo::topology::{
    boolean_operation, clean_valid_polygon_topology, clip_to_mask, dissolve, polygon_overlay,
    BooleanOperation, OverlayMode, TopologyError,
};
use plenora_core::PlenoraError;
use plenora_kernels_geo::{
    geometry_from_wkb, transform_geometry, transform_wkb, Operation, MAX_WKB_BYTES,
    MAX_WKB_COMPONENTS,
};

fn square(x: f64, y: f64, size: f64) -> Geometry<f64> {
    Geometry::Polygon(polygon![
        (x: x, y: y), (x: x + size, y: y),
        (x: x + size, y: y + size), (x: x, y: y + size),
        (x: x, y: y),
    ])
}

fn empty_line() -> Geometry<f64> {
    Geometry::LineString(LineString::new(Vec::new()))
}

fn nan_point() -> Geometry<f64> {
    Geometry::Point(Point::new(f64::NAN, 0.0))
}

#[test]
fn operations_cover_every_geometry_family_and_fail_closed() {
    let point = Geometry::Point(Point::new(1.0, 2.0));
    let line = Geometry::Line(Line::new((0.0, 0.0), (3.0, 4.0)));
    let open = Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)]);
    let closed = Geometry::LineString(line_string![
        (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 0.0, y: 0.0)
    ]);
    let polygon = square(0.0, 0.0, 2.0);
    let multi_point = Geometry::MultiPoint(MultiPoint::new(vec![
        Point::new(0.0, 0.0),
        Point::new(1.0, 1.0),
    ]));
    let multi_line = Geometry::MultiLineString(MultiLineString::new(vec![
        line_string![(x: 0.0, y: 0.0), (x: 1.0, y: 0.0)],
        line_string![(x: 1.0, y: 0.0), (x: 2.0, y: 0.0)],
    ]));
    let multi_polygon = Geometry::MultiPolygon(MultiPolygon::new(vec![
        match square(0.0, 0.0, 1.0) {
            Geometry::Polygon(value) => value,
            _ => unreachable!(),
        },
        match square(3.0, 0.0, 1.0) {
            Geometry::Polygon(value) => value,
            _ => unreachable!(),
        },
    ]));
    let rect = Geometry::Rect(Rect::new((0.0, 0.0), (2.0, 3.0)));
    let triangle = Geometry::Triangle(Triangle::new(
        Coord { x: 0.0, y: 0.0 },
        Coord { x: 2.0, y: 0.0 },
        Coord { x: 0.0, y: 2.0 },
    ));
    let collection = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
        point.clone(),
        open.clone(),
        polygon.clone(),
    ]));

    for geometry in [
        &point,
        &line,
        &open,
        &polygon,
        &multi_point,
        &multi_line,
        &multi_polygon,
        &rect,
        &triangle,
        &collection,
    ] {
        assert!(area(geometry).unwrap().is_finite());
        assert!(length(geometry).unwrap().is_finite());
        assert_eq!(perimeter(geometry).unwrap(), length(geometry).unwrap());
        assert!(bounds(geometry).unwrap().is_some());
        assert!(vertex_count(geometry).unwrap() > 0);
        assert!(!to_wkt(geometry).unwrap().is_empty());
        let _ = point_on_surface(geometry).unwrap();
        let _ = boundary(geometry).unwrap();
    }
    assert_eq!(distance(&empty_line(), &point).unwrap(), None);
    assert!(distance(&point, &polygon).unwrap().unwrap() >= 0.0);
    assert!(matches!(
        area(&nan_point()),
        Err(OperationError::InvalidInput(_))
    ));
    assert!(matches!(
        boundary(&nan_point()),
        Err(OperationError::InvalidInput(_))
    ));

    assert!(
        matches!(boundary(&closed).unwrap(), Geometry::MultiPoint(value) if value.0.is_empty())
    );
    assert_eq!(explode(&multi_point).unwrap().len(), 2);
    assert_eq!(explode(&multi_line).unwrap().len(), 2);
    assert_eq!(explode(&multi_polygon).unwrap().len(), 2);
    assert_eq!(explode(&collection).unwrap().len(), 3);
    assert_eq!(explode(&point).unwrap().len(), 1);

    for cap in [
        BufferCapStyle::Round,
        BufferCapStyle::Flat,
        BufferCapStyle::Square,
    ] {
        assert!(buffer_with_cap(&open, 0.5, cap).is_ok());
    }
    assert!(buffer_with_cap(&open, f64::INFINITY, BufferCapStyle::Round).is_err());

    for policy in [
        SimplifyPolicy::DouglasPeucker,
        SimplifyPolicy::PreserveTopology,
    ] {
        for geometry in [
            &open,
            &multi_line,
            &polygon,
            &multi_polygon,
            &collection,
            &point,
        ] {
            assert!(simplify_with_policy(geometry, 0.01, policy).is_ok());
        }
    }
    assert!(simplify_with_policy(&open, -1.0, SimplifyPolicy::DouglasPeucker).is_err());
}

#[test]
fn constructors_reject_malformed_and_dimensioned_inputs() {
    assert!(matches!(
        point_from_lon_lat(f64::INFINITY, 1.0),
        Err(ConstructionError::NonFiniteCoordinate { name: "lon" })
    ));
    assert!(matches!(
        point_from_lon_lat(1.0, f64::NEG_INFINITY),
        Err(ConstructionError::NonFiniteCoordinate { name: "lat" })
    ));
    for value in [
        "garbage",
        "POINT Z (1 2 3)",
        "POINT M (1 2 3)",
        "POINT ZM (1 2 3 4)",
        "SRID=4326;POINT(1 2)",
    ] {
        assert!(geometry_from_wkt(value).is_err(), "{value}");
    }
    let non_point = Some(Geometry::Rect(Rect::new((0.0, 0.0), (1.0, 1.0))));
    assert!(matches!(
        line_from_ordered_points(&[non_point.clone(), None]),
        Err(ConstructionError::ExpectedPoint { .. })
    ));
    assert!(matches!(
        polygon_from_ordered_points(&[non_point]),
        Err(ConstructionError::ExpectedPoint { .. })
    ));
}

#[test]
fn extended_operations_reject_all_invalid_parameters() {
    let line = Geometry::LineString(line_string![
        (x: 0.0, y: 0.0), (x: 1.0, y: 0.0), (x: 2.0, y: 1.0)
    ]);
    assert!(matches!(
        affine_transform(&line, [1.0, 0.0, f64::NAN, 0.0, 1.0, 0.0]),
        Err(ExtendedError::InvalidParameter { .. })
    ));
    assert!(matches!(
        rotate_about(&line, f64::INFINITY, Point::new(0.0, 0.0)),
        Err(ExtendedError::InvalidParameter { .. })
    ));
    for (concavity, threshold) in [(0.0, 0.0), (-1.0, 0.0), (1.0, -1.0), (f64::NAN, 0.0)] {
        assert!(concave_hull(&line, concavity, threshold, 100).is_err());
    }
    assert_eq!(hausdorff_distance(&empty_line(), &line, 100).unwrap(), None);
    assert!(hausdorff_distance(&line, &line, 1).is_err());
    assert!(haversine_distance_m(Point::new(181.0, 0.0), Point::new(0.0, 0.0)).is_err());
    assert!(geodesic_line_length_m(&line_string![(x: 0.0, y: 91.0), (x: 1.0, y: 0.0)]).is_err());
    assert!(affine_transform(&nan_point(), [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]).is_err());
}

#[test]
fn extended_algorithms_cover_empty_complex_and_wrong_types() {
    let line = line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 4.0)];
    let geometry = Geometry::LineString(line.clone());
    let collection = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
        geometry.clone(),
        Geometry::MultiLineString(MultiLineString::new(vec![line.clone()])),
    ]));
    assert!(densify(&collection, 0.5, 100).is_ok());
    assert!(densify(&geometry, 0.0, 100).is_err());
    assert!(densify(&geometry, 0.1, 2).is_err());
    assert!(densify(&Geometry::Rect(Rect::new((0.0, 0.0), (1.0, 1.0))), 1.0, 100).is_err());
    assert!(snap_to_grid(&geometry, -1.0).is_err());
    assert!(snap_to_grid(&nan_point(), 1.0).is_err());
    assert!(delaunay(&geometry, 1, 10).is_err());
    assert!(delaunay(&geometry, 100, 0).is_err());

    for ratio in [-0.1, 1.1, f64::NAN] {
        assert!(line_interpolate_point(&line, ratio).is_err());
    }
    assert!(line_substring(&line, 0.8, 0.2).is_err());
    assert!(line_substring(&LineString::new(Vec::new()), 0.0, 1.0)
        .unwrap()
        .is_none());
    assert_eq!(
        frechet_distance(&LineString::new(Vec::new()), &line, 100).unwrap(),
        None
    );
    assert!(frechet_distance(&line, &line, 1).is_err());
    assert!(geodesic_bearing_degrees(Point::new(0.0, 91.0), Point::new(0.0, 0.0)).is_err());
    assert!(geodesic_area_m2(&Geometry::Point(Point::new(0.0, 0.0))).is_err());
    assert!(geodesic_area_m2(&nan_point()).is_err());
    assert!(!geometry_diagnostics(&nan_point()).unwrap().is_valid);

    assert!(line_merge(&Geometry::Point(Point::new(0.0, 0.0)), 100, 10).is_err());
    assert!(line_merge(&collection, 1, 10).is_err());
    let splitter = Geometry::Point(Point::new(1.0, 0.0));
    assert!(split_line(&line, &splitter, -1.0, 100, 100, 10, 100).is_err());
    assert!(split_line(
        &line,
        &Geometry::Rect(Rect::new((0.0, 0.0), (1.0, 1.0))),
        0.0,
        100,
        100,
        10,
        100
    )
    .is_err());
    assert!(matches!(
        split_line(&line, &splitter, 0.0, 1, 100, 10, 100),
        Err(ExtendedAlgorithmError::CoordinateLimit { .. })
    ));
}

#[test]
fn topology_all_modes_empty_sides_and_limits() {
    let a = square(0.0, 0.0, 2.0);
    let b = square(1.0, 0.0, 2.0);
    for operation in [
        BooleanOperation::Intersection,
        BooleanOperation::Union,
        BooleanOperation::Difference,
        BooleanOperation::SymmetricDifference,
    ] {
        assert!(boolean_operation(&a, &b, operation).is_ok());
    }
    assert!(boolean_operation(
        &Geometry::Point(Point::new(0.0, 0.0)),
        &b,
        BooleanOperation::Union
    )
    .is_err());
    assert!(dissolve(&[a.clone(), b.clone()]).is_ok());
    assert_eq!(
        clip_to_mask(&[a.clone(), b.clone()], &[]).unwrap(),
        vec![None, None]
    );
    assert_eq!(
        clip_to_mask(std::slice::from_ref(&a), &[square(100.0, 100.0, 1.0)]).unwrap(),
        vec![None]
    );
    for mode in [
        OverlayMode::Intersection,
        OverlayMode::Union,
        OverlayMode::Identity,
        OverlayMode::SymmetricDifference,
    ] {
        assert!(polygon_overlay(
            std::slice::from_ref(&a),
            std::slice::from_ref(&b),
            mode,
            10,
            10
        )
        .is_ok());
    }
    assert!(
        !polygon_overlay(&[], std::slice::from_ref(&b), OverlayMode::Union, 10, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        !polygon_overlay(std::slice::from_ref(&a), &[], OverlayMode::Identity, 10, 10)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        polygon_overlay(
            std::slice::from_ref(&a),
            std::slice::from_ref(&b),
            OverlayMode::Union,
            0,
            10,
        ),
        Err(TopologyError::InvalidParameter { .. })
    ));
    assert!(matches!(
        polygon_overlay(
            std::slice::from_ref(&a),
            std::slice::from_ref(&b),
            OverlayMode::Union,
            10,
            1,
        ),
        Err(TopologyError::ResourceLimit { .. })
    ));
    assert!(
        clean_valid_polygon_topology(std::slice::from_ref(&a), f64::NAN, true, true, 10, 100,)
            .is_err()
    );
    assert!(
        clean_valid_polygon_topology(std::slice::from_ref(&a), 0.1, true, true, 0, 100,).is_err()
    );
    assert!(
        clean_valid_polygon_topology(std::slice::from_ref(&a), 0.1, true, true, 10, 1,).is_err()
    );
}

#[test]
fn analysis_predicates_and_voronoi_adversarial_errors() {
    let empty = Some(empty_line());
    let point = Some(Geometry::Point(Point::new(0.0, 0.0)));
    assert_eq!(
        minimum_distances(&[None, empty.clone()], &[None], 1).unwrap(),
        vec![None, None]
    );
    assert!(matches!(
        minimum_distances(
            std::slice::from_ref(&point),
            std::slice::from_ref(&point),
            0
        ),
        Err(AnalysisError::InvalidWorkLimit)
    ));
    assert!(minimum_distances(&[Some(nan_point())], std::slice::from_ref(&point), 10).is_err());
    assert!(nearest_matches(
        std::slice::from_ref(&point),
        std::slice::from_ref(&point),
        Some(-1.0),
        10,
        10,
    )
    .is_err());
    let duplicate_points = [point.clone(), point.clone()];
    assert!(nearest_matches(std::slice::from_ref(&point), &duplicate_points, None, 10, 1).is_err());
    assert!(nearest_matches(&[None, empty], &[None], None, 10, 10)
        .unwrap()
        .is_empty());

    let outer = square(0.0, 0.0, 4.0);
    let inner = square(1.0, 1.0, 1.0);
    let overlap = square(3.0, 0.0, 3.0);
    let far = square(20.0, 20.0, 1.0);
    let cases = [
        (SpatialPredicate::Intersects, &outer, &overlap),
        (SpatialPredicate::Disjoint, &outer, &far),
        (SpatialPredicate::Contains, &outer, &inner),
        (SpatialPredicate::Within, &inner, &outer),
        (SpatialPredicate::EqualsTopo, &outer, &outer),
        (SpatialPredicate::Covers, &outer, &inner),
        (SpatialPredicate::CoveredBy, &inner, &outer),
        (SpatialPredicate::ContainsProperly, &outer, &inner),
        (SpatialPredicate::Touches, &outer, &square(4.0, 0.0, 1.0)),
        (SpatialPredicate::Overlaps, &outer, &overlap),
    ];
    for (predicate, left, right) in cases {
        assert!(evaluate(left, right, predicate).unwrap());
    }
    assert!(matches!(
        evaluate(&nan_point(), &outer, SpatialPredicate::Intersects),
        Err(PredicateError::NonFiniteCoordinate { .. })
    ));

    assert!(matches!(
        voronoi_cells(&[], 1),
        Err(AdvancedError::InvalidPointLimit)
    ));
    assert!(matches!(
        voronoi_cells(&[], 2),
        Err(AdvancedError::InsufficientPoints)
    ));
    assert!(voronoi_cells(
        &[
            Geometry::Point(Point::new(f64::NAN, 0.0)),
            Geometry::Point(Point::new(1.0, 1.0))
        ],
        2
    )
    .is_err());
}

#[test]
fn protocols_reject_bad_headers_trailers_counts_and_frames() {
    assert!(matches!(
        FrameWriter::new(Vec::new(), MAX_ROWS + 1),
        Err(ProtocolError::TooManyRows(_))
    ));
    let writer = FrameWriter::new(Vec::new(), 1).unwrap();
    assert!(matches!(
        writer.finish(),
        Err(ProtocolError::MissingFrames { .. })
    ));
    let mut writer = FrameWriter::new(Vec::new(), 1).unwrap();
    writer.write_frame(None).unwrap();
    assert!(matches!(
        writer.write_frame(None),
        Err(ProtocolError::TooManyFrames)
    ));
    let stream = FrameWriter::new(Vec::new(), 0).unwrap().finish().unwrap().0;
    assert!(FrameReader::new(stream.as_slice(), 0)
        .unwrap()
        .next_frame()
        .unwrap()
        .is_none());

    let mut bad_magic = vec![0_u8; 16];
    assert!(matches!(
        FrameReader::new(bad_magic.as_slice(), 0),
        Err(ProtocolError::InvalidMagic)
    ));
    bad_magic[..8].copy_from_slice(PROTOCOL_MAGIC);
    bad_magic[8..].copy_from_slice(&(MAX_ROWS + 1).to_le_bytes());
    assert!(matches!(
        FrameReader::new(bad_magic.as_slice(), MAX_ROWS),
        Err(ProtocolError::TooManyRows(_))
    ));

    let mut oversized = Vec::new();
    oversized.extend_from_slice(PROTOCOL_MAGIC);
    oversized.extend_from_slice(&1_u64.to_le_bytes());
    oversized.extend_from_slice(&(MAX_GEOMETRY_BYTES + 1).to_le_bytes());
    let mut reader = FrameReader::new(oversized.as_slice(), 1).unwrap();
    assert!(matches!(
        reader.next_frame(),
        Err(ProtocolError::GeometryTooLarge(_))
    ));

    let mut invalid_trailer = Vec::new();
    invalid_trailer.extend_from_slice(PROTOCOL_MAGIC);
    invalid_trailer.extend_from_slice(&0_u64.to_le_bytes());
    invalid_trailer.extend_from_slice(b"BADTRAIL");
    invalid_trailer.extend_from_slice(&[0_u8; 32]);
    let mut reader = FrameReader::new(invalid_trailer.as_slice(), 0).unwrap();
    assert!(matches!(
        reader.next_frame(),
        Err(ProtocolError::InvalidTrailer)
    ));

    let encoded = write_pairs(Vec::new(), &[JoinPair { left: 1, right: 2 }])
        .unwrap()
        .0;
    let mut bad_pair_magic = encoded.clone();
    bad_pair_magic[0] ^= 1;
    assert!(matches!(
        read_pairs(bad_pair_magic.as_slice()),
        Err(PairProtocolError::InvalidMagic)
    ));
    let mut bad_pair_trailer = encoded.clone();
    let trailer_offset = 16 + 16;
    bad_pair_trailer[trailer_offset] ^= 1;
    assert!(matches!(
        read_pairs(bad_pair_trailer.as_slice()),
        Err(PairProtocolError::InvalidTrailer)
    ));
    let mut too_many = Vec::from(*b"PLNPAIR1");
    too_many.extend_from_slice(&(MAX_PAIRS + 1).to_le_bytes());
    assert!(matches!(
        read_pairs(too_many.as_slice()),
        Err(PairProtocolError::TooManyPairs(_))
    ));
}

#[test]
fn wkb_contract_rejects_malformed_nested_and_extreme_payloads() {
    for payload in [Vec::new(), vec![2], vec![1, 1, 0, 0], vec![9, 1, 0, 0, 0]] {
        assert!(geometry_from_wkb(&payload).is_err());
    }
    let fixture = square(0.0, 0.0, 2.0);
    let wkb = fixture.to_wkb(CoordDimensions::xy()).unwrap();
    for operation in [
        Operation::Centroid,
        Operation::ConvexHull,
        Operation::Envelope,
    ] {
        assert_eq!(
            operation.name(),
            match operation {
                Operation::Centroid => "centroid",
                Operation::ConvexHull => "convex_hull",
                Operation::Envelope => "envelope",
            }
        );
        assert!(transform_geometry(operation, &fixture).is_ok());
        assert!(transform_wkb(operation, &wkb).is_ok());
    }
    assert!(matches!(
        transform_geometry(Operation::Envelope, &empty_line()),
        Err(PlenoraError::Contract(message))
            if message == "geometria vuota non supportata da envelope"
    ));
    assert!(transform_geometry(Operation::Centroid, &nan_point()).is_err());

    let mut big_endian_point = vec![0_u8];
    big_endian_point.extend_from_slice(&1_u32.to_be_bytes());
    big_endian_point.extend_from_slice(&1.5_f64.to_be_bytes());
    big_endian_point.extend_from_slice(&(-2.5_f64).to_be_bytes());
    assert_eq!(
        geometry_from_wkb(&big_endian_point).unwrap(),
        Geometry::Point(Point::new(1.5, -2.5))
    );

    let mut dimensional = vec![1_u8];
    dimensional.extend_from_slice(&1001_u32.to_le_bytes());
    dimensional.extend_from_slice(&1.0_f64.to_le_bytes());
    dimensional.extend_from_slice(&2.0_f64.to_le_bytes());
    dimensional.extend_from_slice(&3.0_f64.to_le_bytes());
    assert!(matches!(
        geometry_from_wkb(&dimensional),
        Err(PlenoraError::Unsupported(message))
            if message == "WKB contiene dimensioni Z/M o SRID non preservabili nel protocollo 2D"
    ));

    let mut one_vertex_line = vec![1_u8];
    one_vertex_line.extend_from_slice(&2_u32.to_le_bytes());
    one_vertex_line.extend_from_slice(&1_u32.to_le_bytes());
    one_vertex_line.extend_from_slice(&0.0_f64.to_le_bytes());
    one_vertex_line.extend_from_slice(&0.0_f64.to_le_bytes());
    assert!(geometry_from_wkb(&one_vertex_line).is_err());

    let mut wrong_child = vec![1_u8];
    wrong_child.extend_from_slice(&4_u32.to_le_bytes());
    wrong_child.extend_from_slice(&1_u32.to_le_bytes());
    wrong_child.push(1);
    wrong_child.extend_from_slice(&2_u32.to_le_bytes());
    wrong_child.extend_from_slice(&0_u32.to_le_bytes());
    assert!(geometry_from_wkb(&wrong_child).is_err());

    let mut nested = vec![1_u8];
    nested.extend_from_slice(&7_u32.to_le_bytes());
    nested.extend_from_slice(&1_u32.to_le_bytes());
    for _ in 0..65 {
        nested.push(1);
        nested.extend_from_slice(&7_u32.to_le_bytes());
        nested.extend_from_slice(&1_u32.to_le_bytes());
    }
    nested.push(1);
    nested.extend_from_slice(&7_u32.to_le_bytes());
    nested.extend_from_slice(&0_u32.to_le_bytes());
    assert!(geometry_from_wkb(&nested).is_err());

    let mut many_components = vec![1_u8];
    many_components.extend_from_slice(&7_u32.to_le_bytes());
    many_components.extend_from_slice(&((MAX_WKB_COMPONENTS + 1) as u32).to_le_bytes());
    for _ in 0..=MAX_WKB_COMPONENTS {
        many_components.push(1);
        many_components.extend_from_slice(&2_u32.to_le_bytes());
        many_components.extend_from_slice(&0_u32.to_le_bytes());
    }
    assert!(geometry_from_wkb(&many_components).is_err());

    let oversized = vec![0_u8; MAX_WKB_BYTES + 1];
    assert!(geometry_from_wkb(&oversized).is_err());
}

#[test]
fn geos_and_proj_reject_hostile_inputs_and_handle_complex_topology() {
    assert!(make_valid_wkb(&[1, 2, 3], RepairMethod::Linework, false).is_err());
    let valid = square(0.0, 0.0, 4.0).to_wkb(CoordDimensions::xy()).unwrap();
    assert_eq!(
        make_valid_wkb(&valid, RepairMethod::Structure, false).unwrap(),
        valid
    );

    let wrong = Geometry::Point(Point::new(0.0, 0.0));
    assert!(matches!(
        polygonize_linework(&wrong, true, false, 10, 10, 10, 10),
        Err(GeosBackendError::UnsupportedGeometry { .. })
    ));
    let network = Geometry::GeometryCollection(GeometryCollection::new_from(vec![
        Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 0.0)]),
        Geometry::LineString(line_string![(x: 4.0, y: 0.0), (x: 4.0, y: 4.0)]),
        Geometry::LineString(line_string![(x: 4.0, y: 4.0), (x: 0.0, y: 4.0)]),
        Geometry::LineString(line_string![(x: 0.0, y: 4.0), (x: 0.0, y: 0.0)]),
        Geometry::LineString(line_string![(x: 0.0, y: 0.0), (x: 4.0, y: 4.0)]),
    ]));
    assert_eq!(
        polygonize_linework(&network, true, true, 100, 10_000, 100, 1_000)
            .unwrap()
            .polygons
            .len(),
        2
    );
    assert!(split_polygon_by_linework(&wrong, &network, 100, 10_000, 100, 1_000).is_err());
    assert!(
        split_polygon_by_linework(&square(0.0, 0.0, 4.0), &wrong, 100, 10_000, 100, 1_000).is_err()
    );

    for crs in ["", "\0EPSG:4326"] {
        assert!(matches!(
            reproject_geometry(&wrong, crs, "EPSG:3857", 10),
            Err(ProjBackendError::Crs(_))
        ));
    }
    assert!(reproject_geometry(&nan_point(), "EPSG:4326", "EPSG:3857", 10).is_err());
}
