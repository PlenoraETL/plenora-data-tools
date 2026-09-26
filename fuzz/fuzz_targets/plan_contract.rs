#![no_main]

use libfuzzer_sys::fuzz_target;
use plenora_engine::table_engine::Plan;

#[path = "comune/aggancio.rs"]
mod aggancio;

fuzz_target!(init: aggancio::installa(), |payload: &[u8]| {
    if let Ok(plan) = serde_json::from_slice::<Plan>(payload) {
        let _ = plan.validate();
    }
});
