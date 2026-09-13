# Provenienza — `wkt` 0.14.0, v2

**Non adottato.** Presente per revisione (vedi la nota in cima a `Cargo.toml`).

- Pacchetto: `wkt-0.14.0.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto: `efb2b923ccc882312e559ffaa832a055ba9d1ac0cc8e86b3e25453247e4b81d7`
  (verificato contro i byte scaricati — vedi `scripts/verifica_vendor_provenienza.py`).
- Patch applicata, da `patches/wkt-v2.patch`:
  conserva i componenti vuoti e i ruoli degli anelli; rifiuta gli interni
  orfani con `InteriorWithoutExterior` (`src/error.rs`); espone
  `try_wkt_string()` fallibile (`src/geo_types_to_wkt.rs`,
  `src/to_wkt/geo_trait_impl.rs`, `src/to_wkt/mod.rs`). La vecchia
  `wkt_string()` resta nella firma pubblica, infallibile, e puo' ancora
  panicare sullo stato orfano — non e' rimossa da questa patch.
- Licenza: `LICENSE-APACHE`/`LICENSE-MIT` sono quelle spedite nel pacchetto
  pubblicato, invariate dalla patch (che tocca solo `src/`).

**Non usata, deliberatamente**: `results/geo-ogc-panic/windows/logging-wkt-01/wkt.patch`
esiste nel congelamento accanto a `logging.patch`, ma e' una bozza v1 di
questa stessa correzione — tocca `write_multi_polygon` in
`src/to_wkt/geo_trait_impl.rs` con un approccio precedente, in
sovrapposizione con `wkt-v2.patch`. La tabella del manifesto lo dice
esplicitamente ("diff cumulativo, non da sovrapporre alla vecchia WKT v1"):
qui e' applicata solo `wkt-v2.patch`.

Ricostruzione end-to-end: `python scripts/verifica_vendor_provenienza.py`.

**Adozione lato prodotto (diff 4, migrazione `operations::to_wkt`) non e'
parte di questa vendorizzazione**: questa patch abilita l'API, non cambia chi
la chiama. Vedi la sezione sulla migrazione data-tools nel congelamento
`handoff-final-20260909/CONSEGNA-DATA-TOOLS.md` per cio' che resta da fare
(test engine su errore dopo riga valida, propagazione in executor blocking e
trasporto Arrow unary).
