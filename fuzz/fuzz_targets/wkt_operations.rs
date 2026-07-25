#![no_main]

use libfuzzer_sys::fuzz_target;
use plenora_kernels_geo::construction::geometry_from_wkt;
use plenora_kernels_geo::operations::{
    area, boundary, bounds, buffer, length, point_on_surface, simplify, to_wkt, vertex_count,
};
use plenora_kernels_geo::{transform_geometry, Operation};

fuzz_target!(|payload: &[u8]| {
    let Ok(text) = std::str::from_utf8(payload) else {
        return;
    };
    let Ok(geometry) = geometry_from_wkt(text) else {
        return;
    };
    for operation in Operation::ALL {
        let _ = transform_geometry(operation, &geometry);
    }
    let _ = area(&geometry);
    let _ = length(&geometry);
    let _ = bounds(&geometry);
    let _ = vertex_count(&geometry);
    let _ = point_on_surface(&geometry);
    let _ = to_wkt(&geometry);
    let _ = boundary(&geometry);
    for value in [-1.0, 0.0, 1.0] {
        let _ = buffer(&geometry, value);
        let _ = simplify(&geometry, value);
    }
});
