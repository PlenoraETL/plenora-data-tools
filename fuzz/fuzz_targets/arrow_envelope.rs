#![no_main]

use libfuzzer_sys::fuzz_target;
use plenora_engine::geo_transport::transport::EnvelopeReader;

// Byte arbitrari contro il lettore dell'envelope v3: header, payload_len,
// payload a chunk, trailer, checksum e limiti di risorse. Invarianti: mai
// panic, mai allocazioni oltre i limiti (il payload cresce solo con i byte
// che arrivano davvero), errori sempre tipizzati.
fuzz_target!(|data: &[u8]| {
    if let Ok(reader) = EnvelopeReader::new(data) {
        let _ = reader.read_payload();
    }
});
