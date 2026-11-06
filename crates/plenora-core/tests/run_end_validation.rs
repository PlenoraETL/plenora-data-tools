//! RUSTSEC-2026-0442: la copertura riguarda offset e lunghezza del padre.
//! Non legge mai un valore fuori limite, neppure con la dipendenza difettosa.

use plenora_core::arrow::array::types::Int32Type;
use plenora_core::arrow::array::{Array, Int32Array, RunArray};
use plenora_core::arrow::ArrowError;

#[test]
fn run_end_rifiuta_lunghezza_e_offset_non_coperti() {
    let array = RunArray::<Int32Type>::try_new(
        &Int32Array::from(vec![2, 3]),
        &Int32Array::from(vec![7, 9]),
    )
    .expect("run-end valido");
    for (offset, len, valido) in [(0, 3, true), (1, 2, true), (0, 5, false), (2, 2, false)] {
        let risultato = array
            .to_data()
            .into_builder()
            .offset(offset)
            .len(len)
            .build();
        match (valido, risultato) {
            (true, Ok(dati)) => {
                assert_eq!(dati.offset(), offset, "offset del padre");
                assert_eq!(dati.len(), len, "lunghezza del padre");
            }
            (false, Err(ArrowError::InvalidArgumentError(_))) => {}
            (atteso, ottenuto) => panic!(
                "copertura del padre: atteso valido={atteso}, ottenuto {}",
                ottenuto.map_or("un rifiuto di altra variante", |_| "un successo")
            ),
        }
    }
}
