# Change impact analysis — row diagnostics sugli output Data Tools

Data: 2026-08-03.

## Baseline e fonte normativa

- baseline funzionale: `af812aaf30f7a189dbdafe78429ef628636bec70`;
- Contracts 2.0-rc17, branch read-only `row-diagnostics-v1`;
- regole applicate: R9.9-R9.14 e schema `plenora-row-diagnostics-v1`.

Manifesti e metadati release congelati non sono modificati.

## Invariante applicato

- un output pubblicato contiene soltanto righe conformi al contratto del nodo;
- un difetto attribuibile a riga/cella non viene convertito in null, corretto,
  omesso o degradato a warning;
- il rifiuto e' fail-closed e porta cause machine-readable, indici sorgente
  zero-based, conteggi esatti e esempi bounded privi di valori;
- gli indici restano indici dell'input originale soltanto lungo nodi che
  preservano cardinalita' e ordine; altrimenti il planner rifiuta il piano;
- `accepted` e `rejected` potranno essere prodotti soltanto da un futuro nodo
  esplicito a due output. Nessun nodo attuale simula una quarantena.

## Superfici e sibling paths auditati

- `table.type_cast`: tutti i target fallibili rifiutano ogni conversione
  invalida anche con la policy legacy `coerce`;
- `table.date_extract`, `date_format`, `date_add`, `date_diff` e
  `timezone_convert`: nessun null sintetico o scelta DST implicita;
- `table.flatten_json`: sintassi JSON invalida e root non-object sono cause
  distinte; nessuna riga difettosa viene accettata;
- `table.assert_not_null`, `assert_unique`, `assert_range`, `assert_regex` e
  `assert_foreign_key`: tutte le righe osservabili sono conteggiate;
- `table.md5_hash` e `sha256_hash` con `null_policy=error`: rifiuto strutturato;
- `geo.from_wkt`: entrambe le policy legacy falliscono chiuse e il nome della
  colonna sorgente e' conservato senza il valore;
- `filter`, `sample`, `explode`, join, aggregate, sort, melt, pivot, transpose,
  top-n, distinct/dedup, window, concat, table-diff e sibling geo generativi o
  binari invalidano la provenance originale. Anche un solo modo non
  conservativo rende la classificazione del descrittore `Unavailable`;
- `explode.empty_policy=drop` e' rifiutato: il drop non e' una partizione
  accepted/rejected.

## Bump di versione ADR-0004 (elenco effettivo)

Baseline `af812aa` → delta row-diagnostics; lo snapshot canonico del
catalogo rende ogni cambio esplicito nel fingerprint per-op:

- `table.formula`: semantic 1→2, kernel 2→3 (nuovo `reject_rows`);
- `table.expression`: semantic 2→3, kernel 3→4 (nuovo `reject_rows`;
  2/2/2/3 era il bump preesistente expression-v2);
- `table.type_cast`: kernel 2→3 (nuova implementazione diagnostica;
  semantic gia' 2 nel delta);
- gia' bumpate nel delta (semantic 1→2 + kernel): `table.date_extract`,
  `flatten_json`, `date_add`, `date_diff`, `date_format`,
  `timezone_convert`, `assert_not_null`, `assert_range`, `assert_regex`,
  `assert_unique`, `assert_foreign_key`, `md5_hash`, `sha256_hash`,
  `explode` e `geo.from_wkt` (semantic 1→2, kernel 1→2);
- semantic 1→2 con kernel invariato (la raccolta row-scoped e' nel
  trasporto/executor, non nel kernel) per le 27 op geo
  `diag-transport`/`diag-coords`: `affine_transform`, `area`, `boundary`,
  `bounds_extractor`, `buffer`, `centroid`, `concave_hull`, `convex_hull`,
  `densify`, `envelope`, `from_coords`, `geodesic_area`,
  `geodesic_line_length`, `length`, `line_interpolate_point`,
  `line_substring`, `make_valid`, `perimeter`, `point_on_surface`,
  `reproject`, `rotate`, `scale`, `simplify`, `snap_to_grid`, `to_wkt`,
  `translate`, `vertex_count`.

Nessun bump di `config_schema_version` o `contract_analysis_version`:
forma/significato delle config e inferenza dei contratti sono invariati.

## Breaking/behavioral: gate legacy → DAG v4 per le op diagnostiche

Comportamento voluto, dichiarato breaking: un piano legacy
(`schema_version < 4`) che contiene un'operazione che puo' emettere
diagnostica row-scoped e' rifiutato dalla CLI con `Unsupported`
("richiede piano DAG v4") — prima del delta veniva eseguito. Il perimetro
segue l'autorita' unica config-sensitive
`OperationDescriptor::emits_row_diagnostics`:

- table incondizionate: `flatten_json`, `date_extract`, `date_format`,
  `date_add`, `date_diff`, `timezone_convert`, `formula`, `expression`,
  `assert_not_null`, `assert_unique`, `assert_range`, `assert_regex`,
  `assert_foreign_key`;
- table config-sensitive: `type_cast` solo con `target_type` a
  conversione fallibile row-scoped (`int`, `float`, `bool`, `uint64`,
  `date`, `datetime`, `date32`, `timestamp_millis`, `decimal128`) e
  `errors` assente/`coerce`/`raise`; `md5_hash`/`sha256_hash` solo con
  `null_policy=error`; `hmac_sha256` mai;
- geo (24 op dispatchate nel DAG v4): `from_wkt`, `centroid`,
  `convex_hull`, `envelope`, `buffer`, `simplify`, `boundary`,
  `point_on_surface`, `make_valid`, `reproject`, `affine_transform`,
  `translate`, `scale`, `rotate`, `concave_hull`, `densify`,
  `snap_to_grid`, `line_substring`, `line_interpolate_point`, `area`,
  `length`, `perimeter`, `vertex_count`, `to_wkt`.

Behavioral anche nel DAG v4: il gate provenance del planner rifiuta
(`InvalidPlan`) le stesse op a valle di nodi che non preservano
cardinalita'/ordine (`filter`, `sort`, join, `aggregate`, `explode`,
`sample`, `melt`, `pivot`, `transpose`, `top_n`, `distinct`/dedup,
`window`, `concat`, `table_diff`, sibling geo generativi/binari —
classificazione `source_row_provenance` del catalogo): piani prima validi
diventano invalidi. Le op solo-trasporto (`from_coords`,
`bounds_extractor`, `geodesic_area`, `geodesic_line_length`) non
attraversano i gate: nel trasporto CLI la provenance e' la posizione nel
batch di input aggregata con offset checked.

## Quota `max_temp_bytes`: tre domini separati (design intenzionale)

`max_temp_bytes` e' applicata PER DOMINIO di scrittura, non come quota
globale su disco: lo staging dell'input del gate WKB, lo staging degli
output accettati dei segmenti row-diagnostics e lo spill degli operatori
misurano ciascuno la propria scrittura contro la quota; il picco su disco
puo' arrivare alla somma dei domini concorrenti (fino a ~3x la quota).
Ne' ADR-0002 ne' ADR-0006 dichiarano una quota temp globale (ADR-0002 la
cita solo per lo spill): la separazione e' design intenzionale,
registrata come DER-004 in `docs/deroghe.md`, e non va letta come "quota
globale".

## Correzioni di classificazione e preservazione (post-review 2026-08-03)

- CLI `transform-arrow`: un rifiuto row-scoped del trasporto legacy e' ora
  classificato `data_mapping`/`read` (remote_effect none, retry never) con
  la diagnostica allegata — prima veniva riclassificato
  `invalid_plan`/`validate`. Gli errori non row-scoped mantengono la
  classificazione storica.
- Trasporto 1:1 e `from_coords`: un errore tardivo non row-scoped dopo
  diagnostica gia' osservata propaga l'errore reale con il report
  accumulato declassato a `Partial` (knowledge limit
  `data_tools.processing_interrupted` o `data_tools.diagnostic_merge_failed`,
  `total` sconosciuto, zero accepted) — prima la diagnostica accumulata
  andava persa silenziosamente.

## Evidenza TDD e qualificazione

I test nuovi coprono report completi e deterministici per cast, JSON, date/DST,
assert, hash e WKT, merge cross-batch, soppressione downstream, envelope CLI e
provenance conservata/non disponibile. La prova RED osservata per
`flatten_json` pubblicava ancora null sintetici prima della prevalidazione.

I risultati effettivi dei gate del current-tree sono riportati nel report di
handoff; un comando non eseguito o impedito dall'ambiente non costituisce
evidenza positiva.

## Limiti residui

- non esiste ancora un nodo pubblico di quarantena a due output; questa patch
  mantiene pertanto il comportamento fail-closed dell'intera operazione;
- non esiste ancora un sidecar lineage per attraversare nodi che cambiano
  cardinalita' o ordine; quei piani sono rifiutati;
- le regole R9.9-R9.14 sono ancora marcate `proposta` nella fonte rc17;
- la review indipendente richiesta per logica safety-critical resta un gate
  esterno: il vincolo di writer unico impedisce di rappresentarla come svolta.

Nessun tag, release, commit, push o claim di conformita' cross-component e'
autorizzato da questo record.
