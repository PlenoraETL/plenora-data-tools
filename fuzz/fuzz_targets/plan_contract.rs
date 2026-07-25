#![no_main]

use libfuzzer_sys::fuzz_target;
use plenora_engine::table_engine::Plan;

fuzz_target!(|payload: &[u8]| {
    if let Ok(plan) = serde_json::from_slice::<Plan>(payload) {
        let _ = plan.validate();
    }
});
