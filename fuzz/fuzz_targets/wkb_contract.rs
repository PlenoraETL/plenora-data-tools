#![no_main]

use libfuzzer_sys::fuzz_target;
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, Operation};

fuzz_target!(|payload: &[u8]| {
    if geometry_from_wkb(payload).is_ok() {
        for operation in Operation::ALL {
            if let Ok(output) = transform_wkb(operation, payload) {
                assert!(geometry_from_wkb(&output).is_ok());
            }
        }
    }
});
