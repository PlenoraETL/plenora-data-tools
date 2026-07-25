#![no_main]

use libfuzzer_sys::fuzz_target;
use plenora_engine::geo_transport::transport::{
    decode_ipc, encode_ipc, transform_batches, ArrowOperation, TransformArrowSchema,
};

// Envelope strutturalmente valido con payload mutato: decode + transform
// (centroid) + encode. Invarianti: mai panic; se la trasformazione riesce,
// l'output deve ri-decodificare senza errori.
fn params() -> TransformArrowSchema {
    TransformArrowSchema {
        schema_version: TransformArrowSchema::VERSION,
        operation: ArrowOperation::Centroid,
        row_count: 0,
        crs: Some("EPSG:3857".to_owned()),
        geometry_column: None,
        distance: None,
        cap: None,
        tolerance: None,
        simplify_policy: None,
        target_crs: None,
        max_output_rows: None,
        max_points: None,
        x_column: None,
        y_column: None,
        snap_tolerance: None,
        remove_overlaps: None,
        fill_gaps: None,
        coefficients: None,
        x_offset: None,
        y_offset: None,
        x_factor: None,
        y_factor: None,
        degrees: None,
        x_origin: None,
        y_origin: None,
        concavity: None,
        length_threshold: None,
        max_segment_length: None,
        grid_size: None,
        start_ratio: None,
        end_ratio: None,
        ratio: None,
        node_input: None,
        require_complete: None,
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok((schema, batches)) = decode_ipc(data) else {
        return;
    };
    let Ok((output_schema, output_batches)) = transform_batches(&schema, &batches, &params())
    else {
        return;
    };
    let Ok(payload) = encode_ipc(&output_schema, &output_batches) else {
        return;
    };
    decode_ipc(&payload).expect("l'output del trasporto deve ri-decodificare");
});
