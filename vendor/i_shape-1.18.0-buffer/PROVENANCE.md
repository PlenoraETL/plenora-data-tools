# Provenienza — `i_shape` 1.18.0, buffer

**Non adottato.** Presente per revisione (vedi la nota in cima a `Cargo.toml`).

- Pacchetto: `i_shape-1.18.0.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto: `bfa9eac533d7509a8ab87672b60ac610c17240f9ea4851d26227689fdfe349c8`
  (verificato contro i byte scaricati — vedi `scripts/verifica_vendor_provenienza.py`).
- Patch applicata, da `patches/i_shape.patch`:
  definisce area zero per un percorso vuoto prima dell'accesso all'ultimo
  vertice (`src/float/int_area.rs`). Nessun filtro dei componenti in
  ingresso — il panico sull'ingresso vuoto e' riprodotto anche sulla base,
  indipendente dal predicato esatto di `geo` (diff 1/2).
- Dipendenza **transitiva**: non compare in `[workspace.dependencies]` ne'
  in alcun `Cargo.toml` del prodotto — arriva via l'algoritmo di buffer di
  `geo`. Il `[patch.crates-io]` la raggiunge comunque; la verifica di
  risoluzione (`scripts/verifica_risoluzione_vendor.py`) la tratta allo
  stesso modo delle dipendenze dirette, non la salta perche' transitiva.
- Licenza: `LICENSE` e' quella spedita nel pacchetto pubblicato, invariata
  dalla patch (che tocca solo `src/float/int_area.rs`).

Ricostruzione end-to-end: `python scripts/verifica_vendor_provenienza.py`.
