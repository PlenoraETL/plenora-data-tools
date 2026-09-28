# Provenienza — `geo` 0.33.1, candidato esatto

**Non adottato.** Presente per revisione e benchmark applicativo (vedi la
nota in cima a `Cargo.toml`).

- Pacchetto: `geo-0.33.1.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto: `30eb1fdc57c1e5cfd11826fe0caec4b9dc7901f3758263bb506228d88c8d9e9a`
  (lo stesso gia' registrato per `geo` nel `Cargo.lock` non patchato; verificato
  di nuovo contro i byte del file scaricato prima di estrarre — vedi
  `scripts/verifica_vendor_provenienza.py`).
- Patch applicate in ordine, da `patches/`:
  1. `geo-exact-orientation.patch` — orientamento esatto (`src/algorithm/kernels/robust.rs`,
     nuovo `src/algorithm/kernels/exact_orientation.rs`). Corregge il segno di
     `orient2d` con aritmetica intera esatta; **non** tocca il guardiano
     dell'asserzione in `edge_end_bundle_star.rs` — quella resta, per
     costruzione il candidato non dovrebbe piu' farla scattare sugli input
     noti.
  2. `logging.patch` — dodici emissioni `log` in `relate` diventano messaggi
     statici (`relate_operation.rs`, `edge_end_bundle_star.rs`,
     `geometry_graph.rs`, `node.rs`, `topology_position.rs`), a parita' di
     livello e decisione geometrica. Correzione distinta dal predicato — non
     tocca l'hook di panico, solo il canale `log` di `relate`.
- Digest dell'albero risultante: verificato da script, non fissato a mano qui
  (cambierebbe a ogni rigenerazione delle patch; il numero fissato e' il
  checksum del pacchetto sopra, quello e' la vera radice di fiducia).
- Licenza: `geo` non la spedisce nel pacchetto pubblicato su crates.io (vive
  alla radice del workspace upstream, non nella singola crate — verificato:
  il tarball estratto non la contiene). `LICENSE-APACHE`/`LICENSE-MIT` qui
  accanto sono lette con `git show 90a0469e2c692b87785b8d0d830852d61bbae142:LICENSE-*`
  da `georust/geo` (commit del 2017-08-14, invariate da allora) — dal blob
  Git, non da un checkout su disco: un checkout Windows con
  `core.autocrlf=true` le avrebbe silenziosamente riscritte in CRLF, cambiando
  il loro sha256 rispetto all'oggetto upstream vero. Verificate dallo script
  contro il digest del blob.

Ricostruzione end-to-end (pacchetto verificato → patch → digest confrontato
con questa cartella): `python scripts/verifica_vendor_provenienza.py`.
