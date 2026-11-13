//! Anche gli adapter diretti controllano la semantica senza leggere celle.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use plenora_core::arrow::Schema;
use plenora_core::contract::arrow_metadata::geometry_output_field;
use plenora_kernels_geo::rust_backend::{arrow::make_valid_batches, precision::Precision};
use std::sync::Arc;

#[test]
fn adapter_diretto_rifiuta_geography_e_spherical_anche_senza_batch() {
    let crs = plenora_core::crs::resolve_crs("EPSG:32632", "crs").unwrap();
    for (key, value) in [
        ("plenora.geometry.spatial_semantics", "geography"),
        ("plenora.geometry.native.geoparquet.edges", "spherical"),
    ] {
        let field = geometry_output_field("g", "EPSG:32632").unwrap();
        let mut md = field.metadata().clone();
        md.insert(key, value);
        let schema = Arc::new(Schema::new(vec![field.with_metadata(md)]));
        let error = make_valid_batches(&schema, &[], "g", Precision::from_crs(&crs).unwrap())
            .expect_err("rifiuto");
        assert_eq!(error.category(), plenora_core::ErrorCategory::Unsupported);
        assert_eq!(error.remote_effect(), plenora_core::RemoteEffect::None);
        assert_eq!(
            error.retry_disposition(),
            plenora_core::RetryDisposition::Never
        );
    }
}
