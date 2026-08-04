# Coverage ledger — plenora-row-diagnostics-v1 (campagna 2026-08-03)

Classificazione delle 146 operazioni pubbliche del catalogo
(`crates/plenora-engine/tests/catalog_snapshot.snap`) rispetto al
contratto trasversale row-diagnostics: ogni failure row-level attribuibile
produce un record `plenora-row-diagnostics-v1` con indice riga e causa
machine-readable, oppure l'operazione fallisce chiusa. Versione
machine-readable: `row_diagnostics_coverage_ledger.json` (stessa cartella).

## Categorie

- `diag-kernel` (16 op) — **Diagnostica kernel table** — il kernel raccoglie i difetti per riga (conteggi completi, esempi bounded, nessun valore) e chiude con `reject_rows`; il planner impone il gate provenance e l'executor mergia con offset assoluti. Autorita' unica config-sensitive: `OperationDescriptor::emits_row_diagnostics` (catalogo) — stessa classificazione per gate planner, machinery executor e gate legacy CLI (re-review 2026-08-03: `table.hmac_sha256` riclassificata `failclosed-config` — null_policy legacy con output dichiarato, nessun rifiuto row-scoped; `md5_hash`/`sha256_hash` diagnostici solo con `null_policy=error`).
- `diag-transport` (26 op) — **Diagnostica trasporto geo 1:1** — trasformazioni e misure geo: `map_nullable` a raccolta completa + aggregazione multi-batch a offset assoluti (P0-2) + runner fuso ADR-0012 (parita' fusa/non fusa testata nel DAG); cause `geometry.*`. NOTA (P2 review): il gate planner NON esiste sul trasporto — vale solo per le op dispatchate nel DAG; per le op solo-trasporto (es. `geo.geodesic_*`) la provenance e' la posizione nel batch di input aggregata con offset checked.
- `diag-wkt` (1 op) — **Diagnostica geo.from_wkt** — raccolta kernel-geo (WIP preesistente), gate planner.
- `diag-coords` (1 op) — **Diagnostica geo.from_coords** — raccolta transport con indici assoluti multi-batch (questa campagna); op non dispatchable nel DAG v4 (solo trasporto/envelope CLI).
- `report-data` (3 op) — **Issue come dati** — il nodo riporta i problemi come righe/colonne di output dichiarate dal contratto (nessun drop silenzioso, semantica documentata).
- `gate-only` (12 op) — **Solo gate input WKB** — nel DAG l'unica failure per-cella possibile e' su archi di input del piano (gate WKB atomico con diagnostica completa); su input validato il kernel e' totale.
- `transport-collective` (9 op) — **Collettive solo-trasporto** — non dispatchable nel DAG v4; nel trasporto CLI il kernel e' totale su input gate-validato; una failure kernel collettiva rara e' fail-closed senza indice (residuo R1, non row-attribuibile per costruzione).
- `unsupported` (24 op) — **Nessun percorso esecutivo pubblico** — `Unsupported` fail-closed a prepare (DAG) e assente da `ArrowOperation` (trasporto): nessun accepted invalido possibile.
- `whole` (3 op) — **Failure whole/group-level** — overflow aggregato / asserzione di cardinalita' / totale: non row-attribuibile per definizione; provenance `Unavailable` dichiarata dal catalogo; fail-closed senza `source_index` conforme al contratto.
- `failclosed-config` (51 op) — **Solo errori di configurazione/schema** — rilevati ad analyze/prepare (tipi, parametri, mismatch schemi); nessun fallimento per-riga sul dato (kernel null-safe, semantica null dichiarata); audit 2026-08-03.

## Evidenze di test per categoria

- `diag-kernel`: kernel reject_rows (plenora-kernels-table) + gate planner + merge executor; test: kernels-table lib (286), planner::row_diagnostics_*, executor::*_collects_complete_row_diagnostics_*
- `diag-transport`: map_nullable a raccolta completa + aggregazione multi-batch a offset assoluti + attach executor (DAG) / rifiuto transport (CLI); test: unary::transform_cells_reports_complete_row_diagnostics, unary::fused_and_sequential_report_identical_row_diagnostics, unary::cell_too_large_is_attributed_to_the_producing_kernel, transport::one_to_one_reports_absolute_indices_and_complete_scan_across_batches (incl. Bounds), executor::geo_transform/geo_measure_reports_complete_row_diagnostics_across_input_batches
- `diag-wkt`: kernels-geo reject_rows (WIP preesistente); test: kernels-geo lib + planner gate
- `diag-coords`: raccolta transport from_coords con indici assoluti multi-batch (questa campagna); test: transport::from_coords_reports_row_diagnostics_with_absolute_indices, CLI roundtrip_smoke::transform_arrow_from_coords_reports_row_diagnostics
- `report-data`: le issue sono DATI di output dichiarati (schema di contratto), non rifiuti: nessun drop; test dedicati executor/transport
- `gate-only`: failure per-cella possibile solo su archi di input del piano: gate WKB atomico con diagnostica completa (test: executor::late_wkb_rejections_are_complete_absolute_and_publish_nothing); su input validato il kernel e' totale
- `transport-collective`: solo trasporto CLI (non dispatchable DAG): kernel totale su input gate-validato; failure kernel collettiva fail-closed senza indice (residuo R1, failure non row-attribuibile per costruzione)
- `unsupported`: nessun percorso esecutivo pubblico: Unsupported fail-closed a prepare (DAG) e assente da ArrowOperation (trasporto); nessun accepted possibile
- `whole`: failure group/whole-level (overflow aggregato, asserzione di cardinalita' intera, totale): non row-attribuibile per definizione, provenance Unavailable dichiarata dal catalogo; fail-closed senza source_index conforme
- `failclosed-config`: errori solo di configurazione/schema rilevati ad analyze/prepare (tipo colonne, parametri, mismatch schemi); nessun fallimento per-riga sul dato: verificato in audit 2026-08-03 sui kernel (null-safe, semantica null dichiarata)

## Tabella per operazione

| op | family | result_shape | execution_class | categoria |
|---|---|---|---|---|
| `geo.affine_transform` | geo | one_to_one | streaming | `diag-transport` |
| `geo.area` | geo | one_to_one | streaming | `diag-transport` |
| `geo.bearing` | geo | one_to_one | streaming | `unsupported` |
| `geo.boundary` | geo | one_to_one | streaming | `diag-transport` |
| `geo.bounds_extractor` | geo | one_to_one | streaming | `diag-transport` |
| `geo.buffer` | geo | one_to_one | streaming | `diag-transport` |
| `geo.centroid` | geo | one_to_one | streaming | `diag-transport` |
| `geo.clean_topology` | geo | one_to_many | blocking | `transport-collective` |
| `geo.clip` | geo | one_to_many | binary_blocking | `unsupported` |
| `geo.cluster_dbscan` | geo | one_to_one | blocking | `gate-only` |
| `geo.collect` | geo | many_to_one | blocking | `gate-only` |
| `geo.concave_hull` | geo | one_to_one | streaming | `diag-transport` |
| `geo.convex_hull` | geo | one_to_one | streaming | `diag-transport` |
| `geo.count_points_in_polygons` | geo | one_to_many | binary_blocking | `gate-only` |
| `geo.coverage_validate` | geo | whole_to_many | blocking | `report-data` |
| `geo.delaunay` | geo | one_to_many | blocking | `transport-collective` |
| `geo.densify` | geo | one_to_one | streaming | `diag-transport` |
| `geo.difference` | geo | one_to_many | binary_blocking | `unsupported` |
| `geo.dissolve` | geo | many_to_one | blocking | `transport-collective` |
| `geo.distance` | geo | one_to_one | streaming | `unsupported` |
| `geo.envelope` | geo | one_to_one | streaming | `diag-transport` |
| `geo.explode` | geo | one_to_many | streaming | `transport-collective` |
| `geo.frechet_distance` | geo | one_to_one | streaming | `unsupported` |
| `geo.from_coords` | geo | from_coords | streaming | `diag-coords` |
| `geo.from_wkt` | geo | from_coords | streaming | `diag-wkt` |
| `geo.generate_grid` | geo | whole_to_many | blocking | `gate-only` |
| `geo.geodesic_area` | geo | one_to_one | streaming | `diag-transport` |
| `geo.geodesic_distance` | geo | one_to_one | streaming | `unsupported` |
| `geo.geodesic_line_length` | geo | one_to_one | streaming | `diag-transport` |
| `geo.geometry_accessors` | geo | one_to_one | streaming | `gate-only` |
| `geo.geometry_diagnostics` | geo | diagnostic | streaming | `report-data` |
| `geo.hausdorff_distance` | geo | one_to_one | streaming | `unsupported` |
| `geo.haversine_distance` | geo | one_to_one | streaming | `unsupported` |
| `geo.intersection` | geo | one_to_many | binary_blocking | `unsupported` |
| `geo.length` | geo | one_to_one | streaming | `diag-transport` |
| `geo.line_builder` | geo | many_to_one | blocking | `transport-collective` |
| `geo.line_interpolate_point` | geo | one_to_one | streaming | `diag-transport` |
| `geo.line_locate_point` | geo | one_to_one | streaming | `gate-only` |
| `geo.line_merge` | geo | many_to_one | blocking | `transport-collective` |
| `geo.line_substring` | geo | one_to_one | streaming | `diag-transport` |
| `geo.make_valid` | geo | one_to_one | streaming | `diag-transport` |
| `geo.nearest` | geo | one_to_many | binary_blocking | `gate-only` |
| `geo.overlay` | geo | one_to_many | binary_blocking | `unsupported` |
| `geo.perimeter` | geo | one_to_one | streaming | `diag-transport` |
| `geo.point_on_surface` | geo | one_to_one | streaming | `diag-transport` |
| `geo.polygon_builder` | geo | many_to_one | blocking | `transport-collective` |
| `geo.polygonize` | geo | many_to_one | blocking | `transport-collective` |
| `geo.predicate_contains` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_contains_properly` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_covered_by` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_covers` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_crosses` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_disjoint` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_equals_topo` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_intersects` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_overlaps` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_touches` | geo | one_to_one | streaming | `unsupported` |
| `geo.predicate_within` | geo | one_to_one | streaming | `unsupported` |
| `geo.reproject` | geo | one_to_one | streaming | `diag-transport` |
| `geo.rotate` | geo | one_to_one | streaming | `diag-transport` |
| `geo.scale` | geo | one_to_one | streaming | `diag-transport` |
| `geo.shared_paths` | geo | whole_to_many | blocking | `gate-only` |
| `geo.simplify` | geo | one_to_one | streaming | `diag-transport` |
| `geo.sjoin` | geo | one_to_many | binary_blocking | `gate-only` |
| `geo.snap` | geo | one_to_one | streaming | `gate-only` |
| `geo.snap_to_grid` | geo | one_to_one | streaming | `diag-transport` |
| `geo.split` | geo | one_to_many | streaming | `unsupported` |
| `geo.subdivide` | geo | one_to_many | streaming | `gate-only` |
| `geo.symmetric_difference` | geo | one_to_many | binary_blocking | `unsupported` |
| `geo.to_wkt` | geo | one_to_one | streaming | `diag-transport` |
| `geo.translate` | geo | one_to_one | streaming | `diag-transport` |
| `geo.union` | geo | one_to_many | binary_blocking | `unsupported` |
| `geo.vertex_count` | geo | one_to_one | streaming | `diag-transport` |
| `geo.voronoi` | geo | one_to_many | blocking | `transport-collective` |
| `geo.within` | geo | one_to_many | binary_blocking | `gate-only` |
| `table.add_row_number` | table | - | blocking | `failclosed-config` |
| `table.aggregate` | table | - | blocking | `whole` |
| `table.align_schema` | table | - | streaming | `failclosed-config` |
| `table.anti_join` | table | - | binary_blocking | `failclosed-config` |
| `table.asof_join` | table | - | binary_blocking | `failclosed-config` |
| `table.assert_cardinality` | table | - | streaming | `whole` |
| `table.assert_foreign_key` | table | - | binary_blocking | `diag-kernel` |
| `table.assert_metadata` | table | - | streaming | `failclosed-config` |
| `table.assert_not_null` | table | - | streaming | `diag-kernel` |
| `table.assert_range` | table | - | streaming | `diag-kernel` |
| `table.assert_regex` | table | - | streaming | `diag-kernel` |
| `table.assert_schema` | table | - | streaming | `failclosed-config` |
| `table.assert_unique` | table | - | blocking | `diag-kernel` |
| `table.bin` | table | - | blocking | `failclosed-config` |
| `table.coalesce` | table | - | streaming | `failclosed-config` |
| `table.concat` | table | - | blocking | `failclosed-config` |
| `table.concat_by_name` | table | - | blocking | `failclosed-config` |
| `table.concat_columns` | table | - | streaming | `failclosed-config` |
| `table.conditional` | table | - | streaming | `failclosed-config` |
| `table.cross_join` | table | - | binary_blocking | `failclosed-config` |
| `table.date_add` | table | - | streaming | `diag-kernel` |
| `table.date_diff` | table | - | streaming | `diag-kernel` |
| `table.date_extract` | table | - | streaming | `diag-kernel` |
| `table.date_format` | table | - | streaming | `diag-kernel` |
| `table.dedup_advanced` | table | - | blocking | `failclosed-config` |
| `table.distinct` | table | - | blocking | `failclosed-config` |
| `table.drop_columns` | table | - | streaming | `failclosed-config` |
| `table.except` | table | - | binary_blocking | `failclosed-config` |
| `table.explode` | table | - | blocking | `failclosed-config` |
| `table.expression` | table | - | streaming | `diag-kernel` |
| `table.fill_na` | table | - | streaming | `failclosed-config` |
| `table.filter` | table | - | streaming | `failclosed-config` |
| `table.flatten_json` | table | - | streaming | `diag-kernel` |
| `table.formula` | table | - | streaming | `diag-kernel` |
| `table.fuzzy_join` | table | - | binary_blocking | `failclosed-config` |
| `table.hmac_sha256` | table | - | streaming | `failclosed-config` |
| `table.intersect` | table | - | binary_blocking | `failclosed-config` |
| `table.join` | table | - | binary_blocking | `failclosed-config` |
| `table.limit` | table | - | streaming | `failclosed-config` |
| `table.lookup` | table | - | streaming | `failclosed-config` |
| `table.mask_data` | table | - | streaming | `failclosed-config` |
| `table.md5_hash` | table | - | streaming | `diag-kernel` |
| `table.melt` | table | - | blocking | `failclosed-config` |
| `table.pivot` | table | - | blocking | `failclosed-config` |
| `table.reconcile` | table | - | binary_blocking | `failclosed-config` |
| `table.rename` | table | - | streaming | `failclosed-config` |
| `table.reorder_columns` | table | - | streaming | `failclosed-config` |
| `table.replace` | table | - | streaming | `failclosed-config` |
| `table.rolling_window` | table | - | blocking | `failclosed-config` |
| `table.sample` | table | - | blocking | `failclosed-config` |
| `table.select_columns` | table | - | streaming | `failclosed-config` |
| `table.semi_join` | table | - | binary_blocking | `failclosed-config` |
| `table.sha256_hash` | table | - | streaming | `diag-kernel` |
| `table.sort` | table | - | blocking | `failclosed-config` |
| `table.split_column` | table | - | streaming | `failclosed-config` |
| `table.stable_fingerprint` | table | - | streaming | `failclosed-config` |
| `table.statistics` | table | - | blocking | `whole` |
| `table.string_extract` | table | - | streaming | `failclosed-config` |
| `table.string_length` | table | - | streaming | `failclosed-config` |
| `table.string_pad` | table | - | streaming | `failclosed-config` |
| `table.table_diff` | table | - | binary_blocking | `failclosed-config` |
| `table.text_normalize` | table | - | streaming | `failclosed-config` |
| `table.timezone_convert` | table | - | streaming | `diag-kernel` |
| `table.top_n` | table | - | blocking | `failclosed-config` |
| `table.transpose` | table | - | blocking | `failclosed-config` |
| `table.type_cast` | table | - | streaming | `diag-kernel` |
| `table.union_distinct` | table | - | binary_blocking | `failclosed-config` |
| `table.unnest` | table | - | streaming | `failclosed-config` |
| `table.uuid_generator` | table | - | streaming | `failclosed-config` |
| `table.validate_rules` | table | - | streaming | `report-data` |
| `table.window_function` | table | - | blocking | `failclosed-config` |

## Residui dichiarati

- **R1 (transport-collective, 9 op)**: una failure del kernel GEOS/geo su input gia' validato dal gate (es. errore di robustness overlay in `clean_topology`) e' fail-closed senza `source_index`: nel trasporto le op collettive/one-to-many non raccolgono per riga. Motivo: l'attribuzione sarebbe a insiemi di righe (intera collezione o coppie), fuori dallo schema v1 single-row; il DAG v4 non le esegue (Unsupported).
- **R2 (whole, 3 op)**: overflow di aggregato e asserzioni whole-relation sono failure non row-attribuibili per definizione; il catalogo dichiara gia' provenance `Unavailable` per queste op, quindi nessun consumatore puo' richiedere `source_index` a valle (gate planner).
- **R3 (unsupported, 24 op)**: op di catalogo senza percorso esecutivo pubblico (Fase 2B/2C): restano fail-closed a prepare; quando verranno dispatchate, la classificazione andra' aggiornata PRIMA dell'abilitazione (binary overlay: attribuzione a coppia — serve estensione schema o rifiuto whole-level).
- **R4 (P2 review 2026-08-03, classificazione cause per messaggio)**: la mappa errore-kernel -> causa row-scoped usa costanti di messaggio condivise (`DIVISION_BY_ZERO_MESSAGE` ecc.), non un discriminatore tipizzato: un rename del messaggio declassificherebbe la causa a "non classificabile". Il comportamento e' comunque fail-closed — l'errore non classificabile propaga grezzo, senza causa ne' report inventati (test: `formula::errore_non_classificabile_propaga_senza_diagnostica_inventata`). La variante tipizzata richiederebbe un nuovo caso in `PlenoraError` (enum pubblico non `non_exhaustive`, plenora-core): API break rimandato alla prossima finestra di versione.
- **R5 (P2 review 2026-08-03, divisori costanti calcolati)**: il rifiuto configurazione-per-divisione-per-zero copre i divisori LETTERALI (anche negati); una sotto-espressione costante calcolata (es. `f / (2 - 2)`) resta valutata per riga e rifiutata row-scoped su tutte le righe (nessuna costante-piega implementata, per non duplicare la semantica di valutazione).
