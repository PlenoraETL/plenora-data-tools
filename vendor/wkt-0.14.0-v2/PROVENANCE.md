# Provenienza — `wkt` 0.14.0, v2

**Adottato**: è il `wkt` del workspace e di chi dipende dai crate di
data-tools: pacchetto `plenora-wkt`, dipendenza per percorso con la chiave
`wkt` (`Cargo.toml`, «Copie vendorizzate»). Il `wkt` 0.14.0 di crates.io
resta nel grafo solo come dipendenza di `geozero`, per il suo lettore WKT
che nessun crate usa (`clippy.toml`).

- Pacchetto: `wkt-0.14.0.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto: `efb2b923ccc882312e559ffaa832a055ba9d1ac0cc8e86b3e25453247e4b81d7`
  (verificato contro i byte scaricati, nel progetto d'origine).
- Patch applicata, da `patches/wkt-v2.patch`:
  conserva i componenti vuoti e i ruoli degli anelli; rifiuta gli interni
  orfani con `InteriorWithoutExterior` (`src/error.rs`); espone
  `try_wkt_string()` fallibile (`src/geo_types_to_wkt.rs`,
  `src/to_wkt/geo_trait_impl.rs`, `src/to_wkt/mod.rs`). La vecchia
  `wkt_string()` resta nella firma pubblica, infallibile, e puo' ancora
  panicare sullo stato orfano — non e' rimossa da questa patch.
- Nome del pacchetto, da `patches/wkt-nome-proprio.patch`: `plenora-wkt`
  (la libreria resta `wkt`), perché una `[patch.crates-io]` non vale per
  chi dipende dai crate di data-tools.
- Licenza: `LICENSE-APACHE`/`LICENSE-MIT` sono quelle spedite nel pacchetto
  pubblicato, invariate dalle patch (`src/` e il nome in `Cargo.toml`).

Una bozza precedente della stessa correzione (v1, su
`write_multi_polygon`) non e' applicata: si sovrapporrebbe a `wkt-v2.patch`.

La ricostruzione end-to-end la facevano gli script di verifica di
`plenora-data-tools@190c493`, non portati qui.

**Chiamanti.** `geo.to_wkt` usa `try_wkt_string()`
(`crates/plenora-kernels-geo/src/operations.rs`); l'unica chiamata a
`wkt_string()` resta su un `Point` (`extensions.rs`), che non ha anelli e
non puo' trovarsi nello stato orfano.
