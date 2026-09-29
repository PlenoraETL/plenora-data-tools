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
  3. (dopo `orient2d-filtro-sperimentale.patch`, vedi
     `PROVENANCE-FILTRO-SPERIMENTALE.md`) `geo-i-overlay-9.patch` — porting a
     `i_overlay` 9.0.0 (sezione sotto).
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

## Porting a `i_overlay` 9.0.0 (`patches/geo-i-overlay-9.patch`)

`geo` 0.33.1 dichiara `i_overlay = "4.5.1, < 4.6.0"`; la patch lo pinna a
`=9.0.0` e porta l'integrazione alla nuova API, senza cambiare l'API
pubblica di `geo` usata dai kernel (`BooleanOps`, `unary_union`, `Buffer`,
`BufferStyle`, `LineJoin`, `LineCap`):

- `src/algorithm/bool_ops/i_overlay_integration.rs`: `FloatPointCompatible`
  ha il tipo associato `Scalar` (da `i_float` 5) al posto del parametro;
- `src/algorithm/bool_ops/mod.rs`: il motore intero delle booleane, di
  `unary_union` e del ritaglio delle linee (`clip`) e' `i64`
  (`OverlayI`, `FloatOverlay::<_, i64>::from_subj_and_clip_custom`,
  `from_subj_custom`, `clip_by_as::<i64>`), con `OverlayOptions::ogc()`;
- `src/algorithm/buffer.rs`: stili e offset da `i_overlay::mesh::float`
  (`mesh` e' diviso in `int`/`float` da 9.0), `LineCap<P>` con un solo
  parametro, contorni e tratti con `outline_as::<i64>` e `stroke_as::<i64>`,
  e `miter_min_turn = 1e-4` rad: i_overlay 9 smussa le giunzioni `Miter`
  con una svolta sotto 5 gradi, 4.5 solo sotto `|cross| < 1e-4` (tratti) e
  mai sui contorni; la soglia di 4.5 conserva la semantica pubblica di
  `LineJoin::Miter` (una svolta di 4 gradi a 100 m perdeva la punta di 12
  cm). Le giunzioni `Round` e `Bevel` la ignorano.

**Perche' `i64`.** Con `i32` la griglia di `i_float` 5 (adattatore
conservativo) ha passo `2^(ceil(log2 r) - 29)`, fino al doppio di quello di
4.5: a 1 cm in metri le estensioni oltre circa 1.500-3.000 km non starebbero
piu' nel bilancio. Con `i64` il passo e' `2^(ceil(log2 r) - 61)`, sotto la
spaziatura dei `f64` delle coordinate stesse; l'unico limite resta il
modulo delle coordinate (`coordinate_abbastanza_fitte`). Con `i64`
`OverlayOptions::ogc()` e i default dell'offset attivano `clean_result`
(pulizia dei vertici allineati dopo il ritorno in `f64`). Costo: vedi
README («Limiti dichiarati»).

**Motivazione dell'aggiornamento** (oltre alla griglia): 4.5 perde senza
errore regioni piene con il solver `Frag`, scelto da `Auto` oltre 16.000
segmenti (upstream #87, corretto in 9.0: `tests/fragment_tests.rs`), e
sbaglia l'estrazione OGC in casi di buchi (#78-#81, #91-#92).

Dipendenze nel lockfile, dal registro (checksum del `Cargo.lock`):

| crate | versione | checksum |
| --- | --- | --- |
| `i_overlay` | 9.0.0 | `a518dce2d7cf7e3758159747e7175c8b86ccdcd8d333dfcb7dbeb3eac2e324fc` |
| `i_float` | 5.0.0 | `9ecaff1ce29bcacd72cdf581ba6fce636ef4c4e0b2da7e7846dd0137d98141e2` |
| `i_shape` | 5.0.0 | `fa1878f91ef7921feaf1b2346253ebc5e661878b22d5715195c1d5dcf0f20ae5` |
| `i_tree` | 0.19.0 | `1e9d4a992a9fe83130f41ceacceac3bb116a4355dfc9c8d6ecea4f1e4b4c6caf` |
| `i_key_sort` | 0.11.0 | `7c6c58d0c60705e66264ce0f788a69a2f21472aeb8188559e7c8c619dbdc10fa` |

Nessuna versione 4.x di `i_overlay` ne' 1.x di `i_float`/`i_shape` resta nel
lockfile. `i_overlay` 9 (49 occorrenze di `unsafe`, accessi senza controllo
dei limiti in `build`, `core/extract*`, `split`, `string`, `vector`),
`i_tree` 0.19 (19) e `i_key_sort` 0.11 (19) contengono `unsafe` proprio,
come le versioni precedenti (49, 24, 19): e' codice della dipendenza, fuori
da `unsafe_code = "forbid"` del workspace.

**`vendor/i_shape-1.18.0-buffer` rimossa, con `patches/i_shape.patch`.** La
patch dava area zero a un percorso vuoto in `IntArea::unsafe_int_area`
(`src/float/int_area.rs`), dove l'offset di 4.5 accedeva all'ultimo vertice
di un anello vuoto. `i_shape` 5 non ha piu' quella funzione, e l'offset di
`i_overlay` 9 (`mesh/int/outline/build.rs`) salta i percorsi con meno di tre
punti prima di calcolarne l'area; i test dei componenti vuoti
(`operations.rs`, `buffer_su_*_con_*vuoto*`) restano e passano.

**Verifica della patch.** L'albero di `HEAD` prima del porting (`git
archive`) piu' `git apply -p1 patches/geo-i-overlay-9.patch` coincide con
questa cartella (`diff -r`, a meno dei fine riga). SHA-256 della patch:
`6a98fdcddb79013ba29d3a6b12d055a1c3dc8c76fa2dff62fcca0121a311b6a7`.
