# ADR 13 — Varianti `*_validated` dei kernel geo: la validazione OGC non si ripete su percorsi dimostrati

- **Stato**: accettato (attuazione 2026-07-31)
- **Decisioni collegate**: ADR 1 (determinismo), ADR 11 (decoder
  validante: `geometry_from_wkb` = una camminata strutturale +
  `check_validation`), ADR 12 (validazione inter-passo D12.4),
  ADR-0009/deroghe (principio R0.1: nessuna garanzia ceduta per inferenza
  sul produttore o sul chiamante)
- **Riferimenti**: regola 3 di AGENTS.md (ogni bug e' una classe);
  `plenora-contracts` R0.1 (evidenza riproducibile, non inferenza)

## Contesto

Lo scoping della fusione binari (2026-07-31) ha trovato la classe
«doppia validazione OGC»: ogni geometria che entra nel motore e' gia'
validata OGC al decode (`geometry_from_wkb`, ADR-0011), ma i kernel che
la ricevono rieseguono `check_validation` sull'ingresso. Siti nel
perimetro dello scoping: `spatial_join.rs` (`checked_envelope`),
`analysis.rs` (`validate_geometries`), `predicates.rs` (`validate`),
`extended.rs` (`validate_input`), `topology.rs` (`as_multi_polygon`).
Sullo stesso elenco si e' verificato che `extended.rs` (`validate_output`)
e `topology.rs` (`checked_result`) NON sono doppie validazioni: sono gate
di OUTPUT, cioe' la garanzia del produttore — restano.

Il vincolo delicato e' R0.1: la seconda validazione e' ridondante SOLO SE
il chiamante ha gia' validato, e «i chiamanti validano» non si puo'
assumere per inferenza — i kernel sono API pubblica del crate. Togliere
il gate dai kernel pubblici sarebbe esattamente il ragionamento vietato.

## Decisione

### D13.1 — Il gate dei kernel pubblici RESTA, invariato

Nessuna forma pubblica esistente cambia comportamento, errori o ordine di
valutazione: chi chiama `spatial_join`, `evaluate`, `boolean_operation`
ecc. con una geometria mai validata ottiene esattamente il gate di sempre.

### D13.2 — Varianti `*_validated` pubbliche con precondizione scritta

Per ogni kernel del perimetro esiste una variante `*_validated` che omette
il SOLO gate OGC di ingresso (e la scansione di finitezza, coperta dalla
stessa precondizione), con la precondizione dichiarata nel doc-comment:
input gia' validato (coordinate finite + validita' OGC) per costruzione —
da `geometry_from_wkb` al decode o da un kernel che valida il proprio
output — mai per inferenza sui chiamanti. Su input che viola la
precondizione il risultato e' indefinito e nessun errore dedicato e'
garantito: la variante e' un contratto del chiamante, non un flag
ottimistico. Restano SEMPRE attivi nelle varianti:

- i gate di OUTPUT (`validate_output`, `checked_result`): garanzia del
  produttore, e' cio' che autorizza i consumatori a valle (regola delle
  catene: un consumatore puo' omettere la validazione solo se il
  produttore garantisce l'output — niente catene di fiducia);
- i gate di TIPO (es. `UnsupportedGeometry` in `topology`): contratto del
  kernel, non validazione;
- limiti di lavoro, parametri e controlli di finitezza a costo nullo (es.
  il bounding box in `spatial_join`, gia' calcolato).

### D13.3 — Ricablaggio solo dei percorsi dimostrati per costruzione

Le varianti sono usate esclusivamente dove la mappa dei chiamanti dimostra
che OGNI input arriva da `geometry_from_wkb` (o da un kernel la cui
garanzia d'output e' equivalente):

- `plenora-engine/geo_transport/pair.rs`: tutti gli input da
  `decode_geometry_side` → `geometry_from_wkb` (sjoin, distance, nearest,
  clip, overlay, within, count_points, booleane pairwise, predicate,
  hausdorff);
- `plenora-engine/geo_transport/unary.rs`: `apply_transform_cell` (affine,
  translate, scale, rotate, concave_hull — decode per-cella
  `geometry_from_wkb`, ovvero decode iniziale + validazione inter-passo
  D12.4 nel runner fuso), `collect_batches` (dissolve) e
  `clean_topology_batches` (clean): stessa provenienza;
- `plenora-cli` `execute_spatial_join`: `read_geometry_stream` →
  `geometry_from_wkb`;
- benchmark `bench_geo_perfcheck` / `bench_geo_sweep`: gli scenari
  decodificano via `geometry_from_wkb`/`decode_geometry_cell` e ora
  rispecchiano il percorso di produzione.

Nelle catene intra-kernel (`polygon_overlay` → join candidati,
`clip_to_mask`/`polygon_overlay` → dissolve → booleana su maschera) la
garanzia si propaga solo perche' il produttore valida l'output
(`checked_result`): e' l'unica forma di «catena» ammessa. La morfologia
`buffer` in `clean_valid_polygon_topology` NON garantisce la validita'
dell'output: la sua geometria prodotta resta validata per intero in
entrambe le forme.

### D13.4 — Percorsi gated congelati

Le forme pubbliche gated mantengono il comportamento esatto precedente,
inclusa la ridondanza interna (es. `polygon_overlay` gated rivalida gli
input nel join candidati): eliminarla nel percorso gated cambierebbe il
comportamento su input invalidi (la scansione di finitezza del join non e'
sostituibile dal solo `check_validation` del ciclo a monte) e il percorso
gated non e' quello caldo di produzione. La ridondanza residua nei
percorsi gated e' dichiarata qui come scelta, non come dimenticanza.

### D13.5 — Percorsi NON dimostrati: nessuna modifica

- `geos_backend.rs` (`predicates::evaluate(source, point, Covers)` in
  `split_polygon_by_linework`): `source` e' validato da
  `checked_geos_input`, ma `point` proviene da `point_on_surface` su una
  geometria prodotta — la sua validita' sarebbe un'inferenza sul
  produttore (vietata da R0.1). Gate intatto.
- Kernel fuori dal perimetro dello scoping binari (`operations`,
  `extended_algorithms`, `extensions*`, `advanced`, `cluster`,
  `proj_backend`, `construction`): stessa classe di gate di ingresso, ma i
  loro percorsi di chiamata non sono stati mappati in questo scoping
  (frechet/line_split sul pair path, misure e trasformazioni sull'unary
  path). Nessuna modifica: l'estensione della stessa tecnica e' un
  follow-up con mappa dedicata, non un'assunzione.

## Conseguenze

- Una validazione OGC in meno per geometria sui percorsi binari e sui
  rami unari ricablati, senza alcuna cessione di garanzia: ogni byte che
  entra nel motore resta validato al decode, ogni geometria prodotta
  resta validata all'uscita del kernel che la produce, ogni chiamante di
  API pubblica non dimostrato resta dietro il gate.
- **Nessuna deroga.** Il contratto sul filo (archi WKB canonici,
  trasporti, publish) e' invariato; la garanzia «ogni kernel valida
  l'input» non e' un invariante di sistema ma una proprieta' della singola
  API, che resta tale nelle forme pubbliche gated. Le varianti aggiungono
  un contratto esplicito del chiamante, dichiarato in codice e qui — non
  spostano nessuna protezione su nessun percorso osservabile. Per questo
  non si apre una voce in `docs/deroghe.md`.
- Nuova API pubblica nel crate `plenora-kernels-geo` (13 funzioni
  `*_validated`): il catalogo operazioni non cambia (i kernel esposti al
  planner sono gli stessi; la variante e' una scelta fisica del
  trasporto), fingerprint invariato.

## Verifica

- **Test di parita'**: per OGNI variante, output identico al percorso
  gated su fixture valide (incluse le quattro booleane, i quattro
  `OverlayMode`, i sei `JoinPredicate`, gli undici `SpatialPredicate`,
  null/vuoti/limiti di lavoro).
- **Test di contratto**: su geometria OGC-invalida (bowtie) il percorso
  gated rifiuta e la variante validated la prende (precondizione violata
  ad arte, solo in test) — documentazione del contratto, NON un nuovo
  modo di accettare input invalidi in produzione; dove esiste, il gate di
  output continua a intercettare (es. `affine_transform_validated` su
  bowtie → `InvalidOutput`).
- **Suite esistente**: i test del trasporto (pair/unary/cli) e l'oracolo
  di fusione on/off coprono i percorsi ricablati con parita' byte-per-byte.
- **Benchmark**: `bench_geo_perfcheck` prima/dopo, container `rust:1.92`
  con `MALLOC_ARENA_MAX=4 MALLOC_MMAP_THRESHOLD_=32768` (numeri nello
  stato di attuazione).

## Stato di attuazione

**Attuato (2026-07-31).** Varianti `*_validated` in `spatial_join`
(`spatial_join_validated`, `spatial_join_nullable_validated`), `analysis`
(`minimum_distances_validated`, `nearest_matches_validated`,
`within_indexes_validated`, `count_points_in_polygons_validated`),
`predicates` (`evaluate_validated`), `extended`
(`affine_transform_validated`, `translate_validated`,
`scale_about_validated`, `rotate_about_validated`,
`concave_hull_validated`, `hausdorff_distance_validated`), `topology`
(`boolean_operation_validated`, `dissolve_validated`,
`clip_to_mask_validated`, `polygon_overlay_validated`,
`clean_valid_polygon_topology_validated`). Ricablati `pair.rs`,
`unary.rs`, `plenora-cli` e i due benchmark come da D13.3. Benchmark
(`bench_geo_perfcheck`, container `rust:1.92`, mitigazione allocatore
`MALLOC_ARENA_MAX=4 MALLOC_MMAP_THRESHOLD_=32768`, mediana di 5):

- `op.clip_inside_mask` (20k poligoni 100v, percorso
  decode→`clip_to_mask`→encode): 2,564 s -> 1,995 s (**−22,2%**);
- `op.overlay_union_unchanged` (5k rettangoli, union di griglie
  disgiunte): 10,91 ms -> 9,48 ms (**−13,1%**);
- ancore invariate entro la varianza dell'host (`ref.decode_points`,
  `ref.decode_polys`, `ref.encode_*`, `op.centroid_polys`): la misura e'
  attribuibile alla sola validazione OGC in meno per geometria (nello
  scenario clip: 20k rivalidazioni d'ingresso + la rivalidazione della
  maschera per riga, eliminata dalla catena `dissolve` -> booleana).

Verifiche: clippy workspace `--all-targets` 0/0 su Linux e sul target
`x86_64-pc-windows-msvc`, gate R6 su `plenora-kernels-geo` verde, suite
workspace verde (1073 test, 0 falliti, container `rust:1.92`).
