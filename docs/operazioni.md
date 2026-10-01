# Operazioni

<!-- Generato da crates/plenora-io/tests/operazioni_doc.rs dalle schede in
docs/schede/ e dal catalogo: non si modifica a mano. Rigenerare con
PLENORA_RIGENERA_DOC=1 cargo test -p plenora-io --test operazioni_doc -->

Una scheda per operazione del catalogo. La tabella «dal catalogo» di ogni
scheda è letta da `plenora_core::catalog` (`CATALOG`, `ALIASES`), il resto
dalla scheda in `docs/schede/<id>.md`; un test confronta questo documento con
quello che schede e catalogo generano, e un esempio che non gira o la cui
uscita non è quella scritta fa fallire lo stesso test.

Regole comuni, che le schede non ripetono:

- **Politiche e limiti** stanno nel [README](../README.md): precisione
  geografica di 1 cm ([«Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)),
  validazione e budget del [runner](../README.md#runner), limiti dei file e
  della riproiezione. Le schede li collegano, non li ricopiano.
- **Config**: ogni config si legge con `deny_unknown_fields`; un campo
  sconosciuto, un tipo sbagliato o un valore fuori dominio è `InvalidPlan`
  in validazione. Un parametro scritto che l'operazione ignorerebbe si
  rifiuta ([README, «Validazione»](../README.md#validazione)).
- **Errori senza dati**: i messaggi nominano colonne, parametri e limiti,
  mai i valori delle celle.
- **Limiti di risorsa** (`max_rows_per_edge`, `max_output_rows`,
  `max_expansion_factor`, `max_columns`, `max_string_bytes`, …) valgono per
  ogni operazione e sono `ResourceLimit`; le schede nominano solo quelli
  propri dell'operazione.
- **Tipi negli esempi**: `utf8`, `int64`, `float64`, `bool`, `date32`,
  `timestamp(unità[, fuso])`, `decimal128(p, s)`, `list<…>`, `struct<…>`,
  `dictionary<…>` sono i tipi Arrow; `geometry` è una colonna `Binary`
  GeoArrow-WKB, scritta in WKT. `null` è il valore nullo, `""` la stringa
  vuota; `float64` si scrive nella forma più corta che torna allo stesso
  valore (`1.0`, `0.1`).
- **Memoria**: il picco misurato per operazione verrà dal catalogo delle
  misure v4; finché non c'è, ogni scheda porta il segnaposto.

- **Tabellari** (71): [`table.add_row_number`](#tableadd_row_number), [`table.aggregate`](#tableaggregate), [`table.align_schema`](#tablealign_schema), [`table.anti_join`](#tableanti_join), [`table.asof_join`](#tableasof_join), [`table.assert_cardinality`](#tableassert_cardinality), [`table.assert_foreign_key`](#tableassert_foreign_key), [`table.assert_metadata`](#tableassert_metadata), [`table.assert_not_null`](#tableassert_not_null), [`table.assert_range`](#tableassert_range), [`table.assert_regex`](#tableassert_regex), [`table.assert_schema`](#tableassert_schema), [`table.assert_unique`](#tableassert_unique), [`table.bin`](#tablebin), [`table.coalesce`](#tablecoalesce), [`table.concat`](#tableconcat), [`table.concat_by_name`](#tableconcat_by_name), [`table.concat_columns`](#tableconcat_columns), [`table.conditional`](#tableconditional), [`table.cross_join`](#tablecross_join), [`table.date_add`](#tabledate_add), [`table.date_diff`](#tabledate_diff), [`table.date_extract`](#tabledate_extract), [`table.date_format`](#tabledate_format), [`table.dedup_advanced`](#tablededup_advanced), [`table.distinct`](#tabledistinct), [`table.drop_columns`](#tabledrop_columns), [`table.except`](#tableexcept), [`table.explode`](#tableexplode), [`table.expression`](#tableexpression), [`table.fill_na`](#tablefill_na), [`table.filter`](#tablefilter), [`table.flatten_json`](#tableflatten_json), [`table.formula`](#tableformula), [`table.fuzzy_join`](#tablefuzzy_join), [`table.hmac_sha256`](#tablehmac_sha256), [`table.intersect`](#tableintersect), [`table.join`](#tablejoin), [`table.limit`](#tablelimit), [`table.lookup`](#tablelookup), [`table.mask_data`](#tablemask_data), [`table.md5_hash`](#tablemd5_hash), [`table.melt`](#tablemelt), [`table.pivot`](#tablepivot), [`table.reconcile`](#tablereconcile), [`table.rename`](#tablerename), [`table.reorder_columns`](#tablereorder_columns), [`table.replace`](#tablereplace), [`table.rolling_window`](#tablerolling_window), [`table.sample`](#tablesample), [`table.select_columns`](#tableselect_columns), [`table.semi_join`](#tablesemi_join), [`table.sha256_hash`](#tablesha256_hash), [`table.sort`](#tablesort), [`table.split_column`](#tablesplit_column), [`table.stable_fingerprint`](#tablestable_fingerprint), [`table.statistics`](#tablestatistics), [`table.string_extract`](#tablestring_extract), [`table.string_length`](#tablestring_length), [`table.string_pad`](#tablestring_pad), [`table.table_diff`](#tabletable_diff), [`table.text_normalize`](#tabletext_normalize), [`table.timezone_convert`](#tabletimezone_convert), [`table.top_n`](#tabletop_n), [`table.transpose`](#tabletranspose), [`table.type_cast`](#tabletype_cast), [`table.union_distinct`](#tableunion_distinct), [`table.unnest`](#tableunnest), [`table.uuid_generator`](#tableuuid_generator), [`table.validate_rules`](#tablevalidate_rules), [`table.window_function`](#tablewindow_function)
- **Geografiche** (75): [`geo.affine_transform`](#geoaffine_transform), [`geo.area`](#geoarea), [`geo.bearing`](#geobearing), [`geo.boundary`](#geoboundary), [`geo.bounds_extractor`](#geobounds_extractor), [`geo.buffer`](#geobuffer), [`geo.centroid`](#geocentroid), [`geo.clean_topology`](#geoclean_topology), [`geo.clip`](#geoclip), [`geo.cluster_dbscan`](#geocluster_dbscan), [`geo.collect`](#geocollect), [`geo.concave_hull`](#geoconcave_hull), [`geo.convex_hull`](#geoconvex_hull), [`geo.count_points_in_polygons`](#geocount_points_in_polygons), [`geo.coverage_validate`](#geocoverage_validate), [`geo.delaunay`](#geodelaunay), [`geo.densify`](#geodensify), [`geo.difference`](#geodifference), [`geo.dissolve`](#geodissolve), [`geo.distance`](#geodistance), [`geo.envelope`](#geoenvelope), [`geo.explode`](#geoexplode), [`geo.frechet_distance`](#geofrechet_distance), [`geo.from_coords`](#geofrom_coords), [`geo.from_wkt`](#geofrom_wkt), [`geo.generate_grid`](#geogenerate_grid), [`geo.geodesic_area`](#geogeodesic_area), [`geo.geodesic_distance`](#geogeodesic_distance), [`geo.geodesic_line_length`](#geogeodesic_line_length), [`geo.geometry_accessors`](#geogeometry_accessors), [`geo.geometry_diagnostics`](#geogeometry_diagnostics), [`geo.hausdorff_distance`](#geohausdorff_distance), [`geo.haversine_distance`](#geohaversine_distance), [`geo.intersection`](#geointersection), [`geo.length`](#geolength), [`geo.line_builder`](#geoline_builder), [`geo.line_interpolate_point`](#geoline_interpolate_point), [`geo.line_locate_point`](#geoline_locate_point), [`geo.line_merge`](#geoline_merge), [`geo.line_substring`](#geoline_substring), [`geo.make_valid`](#geomake_valid), [`geo.nearest`](#geonearest), [`geo.overlay`](#geooverlay), [`geo.perimeter`](#geoperimeter), [`geo.point_on_surface`](#geopoint_on_surface), [`geo.polygon_builder`](#geopolygon_builder), [`geo.polygonize`](#geopolygonize), [`geo.predicate_contains`](#geopredicate_contains), [`geo.predicate_contains_properly`](#geopredicate_contains_properly), [`geo.predicate_covered_by`](#geopredicate_covered_by), [`geo.predicate_covers`](#geopredicate_covers), [`geo.predicate_crosses`](#geopredicate_crosses), [`geo.predicate_disjoint`](#geopredicate_disjoint), [`geo.predicate_equals_topo`](#geopredicate_equals_topo), [`geo.predicate_intersects`](#geopredicate_intersects), [`geo.predicate_overlaps`](#geopredicate_overlaps), [`geo.predicate_touches`](#geopredicate_touches), [`geo.predicate_within`](#geopredicate_within), [`geo.reproject`](#georeproject), [`geo.rotate`](#georotate), [`geo.scale`](#geoscale), [`geo.shared_paths`](#geoshared_paths), [`geo.simplify`](#geosimplify), [`geo.sjoin`](#geosjoin), [`geo.snap`](#geosnap), [`geo.snap_to_grid`](#geosnap_to_grid), [`geo.split`](#geosplit), [`geo.subdivide`](#geosubdivide), [`geo.symmetric_difference`](#geosymmetric_difference), [`geo.to_wkt`](#geoto_wkt), [`geo.translate`](#geotranslate), [`geo.union`](#geounion), [`geo.vertex_count`](#geovertex_count), [`geo.voronoi`](#geovoronoi), [`geo.within`](#geowithin)

Esempi: 146 eseguiti con l'uscita confrontata, 0 verificati solo sul contratto.

## Operazioni tabellari

### `table.add_row_number`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `add_row_number` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Aggiunge una colonna `int64` con il numero progressivo di ogni riga
nell'ordine d'ingresso, a partire da `start`. Con `partition_column` la
numerazione riparte da `start` per ogni valore distinto di quella colonna.
L'ordine è quello delle righe: per numerare secondo un ordinamento serve
prima un `table.sort`.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `"row_number"` | nome non vuoto, al più 1024 byte | colonna d'uscita |
| `start` | intero | `1` | intero a 64 bit | numero della prima riga (di ogni partizione) |
| `partition_column` | stringa o `null` | `null` | colonna dell'ingresso leggibile come testo | colonna le cui righe uguali formano una partizione |
| `order_column` | stringa o `null` | `null` | solo `null` | non supportato: scritto, si rifiuta |
| `ascending` | booleano o `null` | `null` | solo `null` | vale solo con `order_column`: scritto, si rifiuta |

Le partizioni si distinguono per il testo della cella (lo stesso di
[`table.type_cast`](#tabletype_cast) verso `str`); tutte le celle null
formano una partizione sola. Per un `float64` `0.0` e `-0.0` hanno testi
diversi e sono partizioni diverse.

#### Schema

La colonna `output_column`, `int64` non nullable: se esiste già si
sostituisce nella sua posizione (senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se `output_column` è una colonna nuova.

#### Righe

1:1: ogni riga riceve un numero.

#### Ordine

Righe nell'ordine d'ingresso; i numeri crescono di 1 nell'ordine delle
righe, dentro ogni partizione.

#### Errori

In validazione, `InvalidPlan`:

- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- `order_column` scritto (non nullo);
- `ascending` scritto;
- `partition_column` assente o di un tipo che non si legge come testo;
- config con campi sconosciuti.

In esecuzione:

- `ResourceLimit`: un numero oltre `i64::MAX` (con `start` vicino al
  massimo). Con e senza `partition_column` il numero vale `start` più la
  posizione della riga (nella partizione): `i64::MAX` si raggiunge, solo
  il numero successivo fallisce;
- `Schema`: una cella di `partition_column` che non si legge come testo.

#### Limiti e deviazioni

Nessuna numerazione ordinata: `order_column` e `ascending` restano nella
config per compatibilità, ma si rifiutano.

#### Complessità

Tempo O(n); memoria O(n) per la colonna d'uscita, più con
`partition_column` un contatore per partizione distinta con il testo della
sua chiave.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.add_row_number", "in": ["dipendenti"],
 "config": {"partition_column": "reparto", "start": 1, "output_column": "n"}}
```

Ingresso `dipendenti`:

| `reparto: utf8` |
| --- |
| A |
| B |
| A |
| null |
| A |

Uscita `risultato`:

| `reparto: utf8` | `n: int64` |
| --- | --- |
| A | 1 |
| B | 1 |
| A | 2 |
| null | 1 |
| A | 3 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.aggregate`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `aggregate` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 4, kernel 5 |

#### Che cosa fa

Raggruppa le righe per i valori delle colonne `group_by` e produce una riga
per gruppo: le colonne di gruppo, poi una colonna per ogni aggregazione di
`aggregations` (conteggi, somme, medie, estremi, varianze, quantili,
testi). Senza aggregazioni conta le righe di ogni gruppo.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `group_by` | lista di stringhe | obbligatorio | nomi di colonne leggibili come testo, almeno uno, senza ripetizioni | chiave di gruppo |
| `aggregations` | lista di oggetti | `[]` | al più `max_columns` voci, campi sotto | aggregazioni, nell'ordine delle colonne d'uscita |

Campi di ogni aggregazione:

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna dell'ingresso | colonna aggregata |
| `function` | stringa | `"count"` | `count`, `sum`, `avg`, `mean`, `min`, `max`, `first`, `last`, `concat`, `nunique`, `variance`, `stddev`, `quantile` | funzione (`avg` è `mean`) |
| `alias` | stringa | `""` | nome di colonna valido, o vuoto | nome della colonna d'uscita |
| `separator` | stringa | `", "` | entro `max_string_bytes`; solo con `concat` | separatore di `concat` |
| `distinct` | booleano | `false` | non con `count`, `nunique`, `first`, `last` | aggrega solo i valori distinti |
| `skip_null` | booleano | `true` | non con `count`, `first`, `last` | ignora i null (vedi sotto) |
| `quantile` | numero | nessuno | da `0` a `1`; obbligatorio con `quantile`, solo con `quantile` | quantile chiesto |
| `ddof` | intero | `1` | `0` o più; solo con `variance` e `stddev` | gradi di libertà sottratti al divisore |

Un parametro scritto per una funzione che non lo usa si rifiuta.
`separator`, `distinct`, `skip_null`, `quantile` e `ddof` non ammettono
`null` esplicito: un parametro facoltativo si omette.

Nome della colonna d'uscita: `alias` se non è vuoto; altrimenti
`<column>_<funzione>` (`avg` scrive `mean`) se la stessa `column` compare
in più aggregazioni; altrimenti `column`. Le chiavi di gruppo e i nomi
delle aggregazioni (o `count` senza aggregazioni) sono tutti distinti: due
aggregazioni con lo stesso nome, o un nome uguale a una chiave (anche una
colonna aggregata senza `alias` che è anche chiave), si rifiutano
([README, «Nomi delle colonne d'uscita»](../README.md#nomi-delle-colonne-duscita)).

Funzioni:

- `count`: `int64` non nullabile, le celle non nulle del gruppo; ammette
  una colonna di qualsiasi tipo;
- `nunique`: `int64` non nullabile, i testi distinti del gruppo, più uno
  se c'è un null e `skip_null` è `false`;
- `first`, `last`: la cella nella prima o nell'ultima riga del gruppo in
  ordine d'ingresso, null compreso, nel tipo della colonna (un
  `timestamp` di ogni unità resta `timestamp`, con la sua unità e il suo
  fuso, letto senza passare dal testo: il fuso non si verifica);
- `concat`: `utf8`, i testi delle celle in ordine d'ingresso uniti da
  `separator`; con `distinct` solo la prima occorrenza di ogni testo; un
  null si salta, o vale il testo vuoto con `skip_null: false`. Il testo di
  un gruppo non supera `max_string_bytes` byte;
- `sum`, `mean`, `min`, `max`, `variance`, `stddev`, `quantile`: con
  `skip_null: false` un null nel gruppo dà null; un gruppo senza valori dà
  null (anche `sum`: la somma di nessun valore non è zero). Sulle colonne
  intere (`int64`, `uint64`) `sum` è esatta ed esce `int64`, una somma
  oltre `int64` è un errore; `sum` su `date32`, `date64` o `timestamp` si rifiuta in
  validazione (una somma di date non è una data). Su interi, date e istanti
  `mean` parte dalla somma esatta e `variance`, `stddev` dagli scarti
  esatti (valori uguali danno zero). `min` e `max` sulle colonne intere
  (date e istanti compresi) e `decimal128` rendono la cella estrema nel tipo della colonna. Negli altri
  casi l'uscita è `float64` e la cella si legge come `f64` (vedi i
  limiti): `sum` somma in ordine d'ingresso; `min` e `max` ignorano i NaN
  salvo che il gruppo abbia solo NaN, la somma no; `variance` e `stddev`
  dividono per `valori - ddof` e danno null con `valori <= ddof`;
  `quantile` interpola linearmente fra i valori, ordinati come i
  `float64` di [`table.sort`](#tablesort), alla posizione
  `quantile * (valori - 1)`. Con `distinct` i valori si
  deduplicano sul valore esatto (su `float64` per bit: `-0.0` e `0.0`
  distinti) e si riducono in ordine crescente.

`nunique`, `concat` vogliono una colonna leggibile come testo (i tipi di
[`table.distinct`](#tabledistinct)); `first`, `last` gli stessi tipi, ma
la cella non passa dal testo (un fuso non valido non conta); le funzioni
numeriche una colonna `int64`, `uint64`, `float64`, `decimal128`, `date32`
(giorni), `date64` (millisecondi), `timestamp` di ogni unità (il valore
nell'unità della colonna, con o senza fuso) o `utf8` il cui testo è un
numero (spazi ai lati ignorati, virgola decimale ammessa).

#### Schema

Le colonne di `group_by` nel loro ordine, con tipo, nullabilità e metadati
di campo dell'ingresso; poi una colonna per aggregazione, nell'ordine di
`aggregations`, senza metadati di campo (tipi sopra); senza aggregazioni,
una colonna `count` `int64` non nullabile con le righe del gruppo, null
compresi. I metadati di schema si conservano. Una colonna geometrica resta
geometrica solo se è una chiave di gruppo. Il contratto non dichiara né
ordinamento né conteggio.

#### Righe

Aggregazione: una riga per chiave di gruppo distinta. L'uguaglianza delle
chiavi è quella di [`table.distinct`](#tabledistinct): un null è un gruppo
a sé, `-0.0` e `0.0` sono gruppi diversi, tutti i NaN un gruppo solo.

#### Ordine

I gruppi escono nell'ordine della loro chiave in testo, colonna per
colonna: il gruppo del null per primo, poi i valori nell'ordine dei byte
della stringa `<n>:<testo>`, con `n` la lunghezza in byte del testo scritta
in decimale. Quindi non è l'ordine dei valori: `"sud"` (`3:sud`) precede
`"nord"` (`4:nord`), `9` precede `-1`, `-1` precede `-2` e `10`, e un
testo di 10 byte precede uno di 1 byte (`10:` precede `1:`). Una chiave
`timestamp` non ha come testo la sua resa RFC 3339 ma il suo istante, a
larghezza fissa ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)):
i suoi gruppi escono in ordine cronologico, in ogni unità e fuso. L'ordine
è deterministico e non dipende dall'hash.

#### Errori

In validazione, `InvalidPlan`:

- `group_by` vuoto, con un nome ripetuto o non valido, o oltre il limite
  di colonne; `aggregations` oltre il limite di colonne;
- una colonna assente; una chiave di gruppo non leggibile come testo;
- una funzione su un tipo che non accetta (sopra);
- `quantile` assente con `function: "quantile"`, o fuori da `0..1`;
- un parametro scritto per una funzione che non lo usa; `separator`,
  `distinct`, `skip_null`, `quantile` o `ddof` `null` espliciti;
  `separator` oltre `max_string_bytes`; un nome d'uscita non valido (per
  esempio un `alias` di soli spazi) o ripetuto, anche uguale a una chiave
  di gruppo;
- funzione fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella `utf8` che non è un numero sotto una funzione
  numerica; una cella che non si converte in testo (date fuori
  intervallo, `binary` non UTF-8 sotto `first`, `last`, `concat`,
  `nunique`);
- `ResourceLimit`: più di `u32::MAX` righe; il testo `concat` di un gruppo
  oltre `max_string_bytes` byte (controllato prima di unire);
- `DataMapping`: `sum` su una colonna intera oltre la gamma di `int64`.

#### Limiti e deviazioni

Le funzioni a risultato `float64` calcolano in `f64`: un `decimal128` o un
testo con più cifre di quante un `f64` ne tenga si arrotondano senza errore
(con `distinct` i distinti si decidono comunque sul valore esatto); sulle
colonne intere media e dispersione arrotondano alla fine del calcolo
esatto, e
`quantile` interpola i valori arrotondati ([README, «Somme intere esatte e tipi delle riduzioni»](../README.md#somme-intere-esatte-e-tipi-delle-riduzioni)). Le strutture di chiavi e gruppi
non sono contabilizzate
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n) sulle righe per il raggruppamento, più O(g log g) per ordinare
i g gruppi e, per `quantile` e `distinct`, O(m log m) sugli m valori di
ogni gruppo; memoria O(n) per l'assegnazione delle righe ai gruppi più
l'uscita. Da 32.768 righe, con gruppi di almeno 8 righe in media, il
calcolo per gruppo va in parallelo, con lo stesso risultato.

#### Memoria

Memoria: da misura v4.

#### Esempio

`importo` compare due volte: la somma si chiama `importo_sum`, il conteggio
ha un alias. `"sud"` precede `"nord"` perché il testo è più corto.

Passo del piano:

```json
{"out": "risultato", "op": "table.aggregate", "in": ["vendite"],
 "config": {"group_by": ["regione"], "aggregations": [
    {"column": "importo", "function": "sum"},
    {"column": "importo", "function": "count", "alias": "n"}
  ]}}
```

Ingresso `vendite`:

| `regione: utf8` | `importo: float64` |
| --- | --- |
| nord | 10.0 |
| sud | 5.0 |
| nord | null |
| null | 2.5 |

Uscita `risultato`:

| `regione: utf8` | `importo_sum: float64` | `n: int64` |
| --- | --- | --- |
| null | 2.5 | 1 |
| sud | 5.0 | 1 |
| nord | 10.0 | 1 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.align_schema`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 2, analisi 3, kernel 3 |

#### Che cosa fa

Porta la tabella allo schema dichiarato in `columns`: le colonne escono
nell'ordine dichiarato; una colonna che esiste deve avere già il tipo
dichiarato (nessuna conversione implicita); una che manca si aggiunge,
tutta null oppure costante col `default`. Le colonne non dichiarate si
scartano, o con `keep_extra` si tengono in coda.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di oggetti | obbligatorio | da 1 a 4096 colonne, nomi senza ripetizioni | schema d'uscita, nell'ordine d'uscita |
| `columns[].name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte | nome della colonna |
| `columns[].type` | stringa | obbligatorio | `Utf8`, `Int64`, `UInt64`, `Float64`, `Boolean`, `Date32`, `Timestamp`, `Decimal128`, `Binary` | tipo della colonna (tabella sotto) |
| `columns[].default` | JSON | assente | valore convertibile nel tipo (sotto); `null` vale assente | valore di ogni cella di una colonna aggiunta |
| `keep_extra` | booleano | `false` | `true`, `false`; `null` non ammesso | tiene in coda, nell'ordine d'ingresso, le colonne non dichiarate |

I tipi: `Utf8` → `utf8`, `Int64` → `int64`, `UInt64` → `uint64`,
`Float64` → `float64`, `Boolean` → `bool`, `Date32` → `date32`,
`Timestamp` → `timestamp(ms)` senza fuso, `Decimal128` →
`decimal128(38, 10)`, `Binary` → `binary`. Il nome del tipo si scrive
esattamente così.

Il `default` si converte così, e ciò che non si converte si rifiuta:

- `Utf8`, `Binary`: solo una stringa JSON di al più `max_string_bytes`
  byte (per `Binary`, i suoi byte UTF-8);
- `Int64`, `UInt64`: un intero JSON nel dominio del tipo, o una stringa
  che lo è (spazi ai lati ignorati); `1.0` si rifiuta;
- `Float64`: un numero JSON, o una stringa con la virgola decimale
  ammessa (`"2,5"` vale 2,5);
- `Boolean`: `true`/`false` JSON, o le stringhe `"true"`/`"false"` in
  qualunque combinazione di maiuscole;
- `Date32`: una stringa `AAAA-MM-GG`;
- `Timestamp`: una stringa RFC 3339 con fuso (`"2026-07-25T00:00:00Z"`),
  convertita all'istante in millisecondi; una parte sotto il millisecondo,
  una frazione oltre il nanosecondo e un secondo intercalare si rifiutano
  (il lettore di [README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data));
- `Decimal128`: un numero JSON o una stringa, senza esponente, con un
  segno facoltativo (uno solo: `"--5"` e `"+-5"` si rifiutano) e con al
  più 10 cifre decimali (nessun arrotondamento).

Si accetta, perché l'effetto dipende dall'ingresso e lo stesso piano gira
su tabelle diverse: il `default` di una colonna che esiste già (non si
legge) e `keep_extra` quando ogni colonna d'ingresso è dichiarata (non
tiene niente). `keep_extra: null` esplicito si rifiuta: il parametro si
omette.

#### Schema

Le colonne dichiarate, nell'ordine di `columns`, poi, con `keep_extra`, le
altre colonne dell'ingresso nel loro ordine. Una colonna che esiste resta
identica (tipo, nullabilità, metadati di campo); una aggiunta senza
`default` è nullable e tutta null; una aggiunta con `default` non è
nullable. I metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna è stata aggiunta né scartata. Se la colonna
geometrica è scartata il contratto diventa tabellare.

#### Righe

1:1: stesse righe; le colonne esistenti hanno gli stessi valori.

#### Ordine

Righe nell'ordine d'ingresso; colonne come descritto sopra.

#### Errori

In validazione, `InvalidPlan`:

- `columns` assente, vuota o con più di 4096 colonne;
- un nome ripetuto, vuoto, di soli spazi o oltre 1024 byte;
- una colonna esistente di tipo diverso da quello dichiarato (anche un
  `timestamp(ms)` con fuso, o un decimale di precisione o scala diverse);
- un `default` non convertibile nel tipo, o un `default` `Utf8` o
  `Binary` oltre `max_string_bytes` byte;
- `keep_extra` `null` esplicito;
- un `type` fuori elenco, config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

- **Nessuna conversione implicita**: per cambiare il tipo di una colonna
  esistente serve [`table.type_cast`](#tabletype_cast) prima.

#### Complessità

Tempo O(c) sulle colonne più O(n) per ogni colonna aggiunta; memoria O(n)
per ogni colonna aggiunta, nessuna per quelle esistenti (array condivisi).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.align_schema", "in": ["ordini"],
 "config": {"columns": [
    {"name": "id", "type": "Int64"},
    {"name": "stato", "type": "Utf8", "default": "nuovo"},
    {"name": "sconto", "type": "Float64"}
  ]}}
```

Ingresso `ordini`:

| `note: utf8` | `id: int64` |
| --- | --- |
| urgente | 1 |
| null | 2 |

Uscita `risultato`:

| `id: int64` | `stato: utf8` | `sconto: float64` |
| --- | --- | --- |
| 1 | nuovo | null |
| 2 | nuovo | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.anti_join`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `anti_join` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Tiene le righe di sinistra la cui chiave non compare a destra, e scarta le
altre. Dalla destra non si prende nessuna colonna. È il complemento di
[`table.semi_join`](#tablesemi_join): ogni riga di sinistra finisce in
esattamente una delle due uscite.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | colonne della sinistra, almeno una, senza ripetizioni | colonne chiave del lato sinistro |
| `right_keys` | lista di stringhe | obbligatorio | colonne della destra, tante quante `left_keys`, senza ripetizioni | colonne chiave del lato destro, nello stesso ordine |

Tipi di chiave come in `table.semi_join`: stesso tipo Arrow nella coppia,
fra `utf8`, `int64`, `uint64`, `float64`, `bool`, `date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary` e
`dictionary<utf8>`.

#### Schema

Identico alla sinistra: stesse colonne, tipi, nullabilità, metadati e
colonna geometrica. Delle proprietà del contratto resta l'ordinamento
dichiarato (`sorted_by`) della sinistra; il conteggio delle righe non è più
noto.

#### Righe

Filtro della sinistra: ogni riga al più una volta. Le chiavi si
confrontano come in [`table.join`](#tablejoin). Una riga sinistra con una
colonna chiave nulla non ha mai corrispondenza e quindi **resta** (come
`NOT EXISTS` in SQL, non come `NOT IN`); le righe destre con una chiave
nulla non contano.

#### Ordine

Le righe tenute restano nell'ordine della sinistra.

#### Errori

In validazione, `InvalidPlan`:

- config con campi sconosciuti, `left_keys` o `right_keys` assenti;
- liste di chiavi vuote, di lunghezza diversa, con nomi ripetuti o oltre
  `max_columns`; colonna assente; tipi diversi nella coppia; tipo fuori
  dall'elenco sopra.

In esecuzione, `Schema`: una cella chiave `date32`, `date64` o `timestamp` fuori
dall'intervallo delle date rappresentabili, un `date64` non allineato al
  giorno, o un dizionario malformato.

#### Limiti e deviazioni

Nessuna conversione fra tipi di chiave, come in `table.join`. L'insieme
delle chiavi di destra usa un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n + m) atteso (insieme delle chiavi di destra, sonda della
sinistra, in parallelo da 65.536 righe sinistre); memoria O(m) per
l'insieme, più la copia delle righe tenute.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.anti_join", "in": ["ordini", "attivi"],
 "config": {"left_keys": ["cliente"], "right_keys": ["codice"]}}
```

Ingresso `ordini`:

| `id: int64` | `cliente: utf8` |
| --- | --- |
| 1 | a |
| 2 | b |
| 3 | null |
| 4 | a |

Ingresso `attivi`:

| `codice: utf8` |
| --- |
| a |
| a |
| null |

Uscita `risultato`:

| `id: int64` | `cliente: utf8` |
| --- | --- |
| 2 | b |
| 3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.asof_join`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `asof_join` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 4, kernel 4 |

#### Che cosa fa

Affianca a ogni riga di sinistra al più una riga di destra: quella con il
valore ordinato (`right_on`) più vicino al valore della riga
(`left_on`), nella direzione scelta, dentro lo stesso gruppo (`left_by` con
`right_by`) ed entro la tolleranza. Serve ad abbinare eventi a istanti non
coincidenti, per esempio a ogni ordine l'ultimo prezzo noto.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_on` | stringa | obbligatorio | colonna `int64` o `float64` della sinistra | valore ordinato della riga sinistra |
| `right_on` | stringa | obbligatorio | colonna della destra, dello stesso tipo di `left_on` | valore ordinato dei candidati |
| `left_by` | lista di stringhe | `[]` | colonne della sinistra, senza ripetizioni | gruppo: si abbinano solo righe con gli stessi valori |
| `right_by` | lista di stringhe | `[]` | colonne della destra, tante quante `left_by`, senza ripetizioni | colonne di gruppo del lato destro, nello stesso ordine |
| `direction` | stringa | `backward` | `backward`, `forward`, `nearest` | `backward`: il più grande `<=` del valore; `forward`: il più piccolo `>=`; `nearest`: il più vicino dei due |
| `tolerance` | numero o `null` | `null` | finito, `>= 0`; se intero, esatto in `f64`; non `0` con `allow_exact: false` | distanza massima fra i due valori; `null` nessun limite |
| `allow_exact` | booleano | `true` | `true`, `false` | `false`: un candidato con valore uguale non si abbina (`<` e `>` stretti) |

Le colonne `by` di ogni coppia hanno lo stesso tipo Arrow, fra quelli
ammessi come chiave da [`table.join`](#tablejoin), e si confrontano come
lì.

#### Schema

Prima tutte le colonne di sinistra, con il loro nome, poi quelle di destra
tranne `right_on` e le `right_by`. Una colonna di destra con lo stesso nome
di una colonna di sinistra diventa `<nome>_R`; le altre tengono il nome.
Tipi e metadati di campo restano quelli d'origine; ogni colonna d'uscita è
nullable. I metadati di schema dei due lati si fondono: una chiave presente
da un lato solo, o con lo stesso valore, resta. L'uscita ammette una sola
colonna geometrica. Le proprietà del contratto (`sorted_by`, `row_count`)
sono quelle della sinistra.

#### Righe

Esattamente una riga per ogni riga di sinistra; dove non c'è candidato le
colonne di destra sono nulle. Il candidato:

- sta nello stesso gruppo: tutte le colonne `by` uguali, nessuna nulla
  (senza `by` il gruppo è unico);
- ha un valore `right_on` finito: le righe destre con valore nullo, NaN o
  infinito non sono candidate, e una riga sinistra con uno di questi valori
  resta senza candidato;
- `backward`: il valore più grande `<=` (`<` con `allow_exact: false`); a
  parità di valore la riga destra con l'indice più alto;
- `forward`: il valore più piccolo `>=` (`>`); a parità di valore la riga
  destra con l'indice più basso;
- `nearest`: fra il candidato `backward` e quello `forward` il più vicino;
  a pari distanza quello `backward`;
- con `tolerance`, la distanza `|destra - sinistra|` non la supera.

Una stessa riga di destra può abbinarsi a più righe di sinistra.

#### Ordine

Quello della sinistra.

#### Errori

In validazione, `InvalidPlan`:

- config con campi sconosciuti, `direction` fuori elenco, `left_on` o
  `right_on` assenti;
- `left_by` e `right_by` di lunghezza diversa, con nomi ripetuti o oltre
  `max_columns`;
- `tolerance` negativa, o intera e non esatta in `f64` (oltre `2^53` con
  bit bassi non nulli: diventerebbe un'altra soglia); `tolerance` zero con
  `allow_exact: false` (nessun candidato si abbinerebbe mai);
- colonna assente; `left_on` e `right_on` non dello stesso tipo, o di tipo
  diverso da `int64` e `float64`; colonne `by` di tipi diversi nella
  coppia, o di tipo non ammesso;
- nomi d'uscita che collidono dopo i suffissi, più colonne di
  `max_columns`, due colonne geometriche nell'uscita, metadati di schema
  con la stessa chiave e valori diversi.

In esecuzione, `Schema`:

- un valore `int64` di `left_on` o `right_on` senza un `f64` esatto (oltre
  `2^53` con bit bassi non nulli): si rifiuta invece di arrotondarlo;
- una cella `by` `date32`, `date64` o `timestamp` fuori dall'intervallo delle
  date rappresentabili, un `date64` non allineato al
  giorno, o un dizionario malformato.

#### Limiti e deviazioni

I valori `int64` si confrontano come `f64` esatti. Le distanze di
`tolerance` e di `nearest` si decidono sulla differenza esatta dei due
valori, non su quella arrotondata: una distanza appena sopra la
tolleranza non si abbina, e di due candidati a distanze diverse vince il
più vicino anche se le differenze arrotondate coincidono. Un valore
esattamente al bordo della tolleranza si abbina, e a pari distanza vince
il candidato prima. Una tolleranza decimale vale il suo `f64`.

#### Complessità

Tempo O(m log m + n log m): i candidati di ogni gruppo si ordinano una
volta, ogni riga di sinistra li cerca per bisezione. Memoria O(m) per i
gruppi, O(n) per l'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.asof_join", "in": ["ordini", "prezzi"],
 "config": {"left_on": "ts", "right_on": "ts", "tolerance": 3}}
```

Ingresso `ordini`:

| `ts: int64` | `id: utf8` |
| --- | --- |
| 1 | o1 |
| 5 | o2 |
| 10 | o3 |

Ingresso `prezzi`:

| `ts: int64` | `prezzo: float64` |
| --- | --- |
| 0 | 10.0 |
| 4 | 11.0 |
| 6 | 12.0 |
| 20 | 13.0 |

Uscita `risultato`:

| `ts: int64` | `id: utf8` | `prezzo: float64` |
| --- | --- | --- |
| 1 | o1 | 10.0 |
| 5 | o2 | 11.0 |
| 10 | o3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_cardinality`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_cardinality` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Verifica il numero di righe della tabella: esattamente `exact_rows`, oppure
almeno `min_rows` e al più `max_rows`. Se l'asserzione regge, l'uscita è
l'ingresso invariato; se non regge, il passo fallisce e non produce uscita.
Quando il numero di righe dell'ingresso è già attestato nel contratto,
l'esito si decide in validazione.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `exact_rows` | intero | assente | da 0 a `max_input_rows` | numero esatto di righe |
| `min_rows` | intero | assente | da 1 a `max_input_rows`, non oltre `max_rows` | minimo di righe, incluso |
| `max_rows` | intero | assente | da 0 a `max_input_rows` | massimo di righe, incluso |

Almeno uno dei tre; `exact_rows` non si combina con `min_rows` o
`max_rows`. `min_rows = 0` non vincola niente e si rifiuta.
`max_input_rows` è il limite di righe del piano.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

#### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- nessuno dei tre vincoli, o `exact_rows` insieme a `min_rows`/`max_rows`;
- `min_rows = 0`, `min_rows` maggiore di `max_rows`, o un vincolo oltre
  `max_input_rows`;
- il contratto d'ingresso attesta il numero di righe (`row_count`
  dimostrato, per esempio dopo `table.reconcile`) e questo viola il vincolo;
- config con campi sconosciuti o valori negativi; `exact_rows`,
  `min_rows` o `max_rows` `null` espliciti (un parametro facoltativo si
  omette).

Le regole sui vincoli (almeno uno, `exact_rows` da solo, `min_rows` non
oltre `max_rows`, `min_rows` non zero) le applica anche il kernel, con la
stessa funzione della validazione.

In esecuzione, `InvalidPlan` (`N righe fuori contratto`): il numero di righe
viola il vincolo. Non c'è diagnostica per riga: il difetto è della tabella,
non di una riga.

#### Limiti e deviazioni

L'errore in esecuzione è `InvalidPlan`, non `DataMapping` come le altre
asserzioni sui dati: chi lo classifica per categoria lo vede come un difetto
del piano.

#### Complessità

Tempo e memoria O(1): conta le righe della tabella, l'uscita condivide le
colonne dell'ingresso.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_cardinality", "in": ["lotto"],
 "config": {"min_rows": 1, "max_rows": 3}}
```

Ingresso `lotto`:

| `id: int64` |
| --- |
| 10 |
| 20 |

Uscita `risultato`:

| `id: int64` |
| --- |
| 10 |
| 20 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_foreign_key`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_foreign_key` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 2, kernel 4 |

#### Che cosa fa

Verifica un vincolo di chiave esterna: ogni chiave `left_keys` della tabella
sinistra deve comparire fra le chiavi `right_keys` della tabella destra. Se
l'asserzione regge, l'uscita è la tabella sinistra invariata; se una chiave
sinistra non trova riscontro, o è nulla senza `allow_null`, il passo
fallisce con una diagnostica per riga e non produce uscita. La tabella
destra serve solo come riferimento.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne leggibili come testo scalare | colonne della chiave esterna, nella tabella sinistra |
| `right_keys` | lista di stringhe | obbligatorio | tanti nomi quanti `left_keys`, senza ripetizioni; stesse condizioni | colonne della chiave riferita, nella tabella destra |
| `allow_null` | booleano | `false` | `true`, `false` | con `true` una riga sinistra con un null nella chiave passa senza controllo |

Le colonne si abbinano per posizione: `left_keys[i]` con `right_keys[i]`, e
ogni coppia ha lo stesso tipo Arrow. Leggibili come testo scalare: `utf8`,
`int64`, `uint64`, `float64`, `bool`, `binary`, `date32`, `date64`,
`timestamp` di ogni unità (con fuso valido), `decimal128` (scala da 0 a 38), dizionario
`int32`→`utf8`.

Due chiavi sono uguali quando lo sono tutte le loro colonne, con
l'uguaglianza della forma testuale: sugli interi, i testi, le date e i
decimali è l'uguaglianza dei valori; su `float64` tutti i `NaN` sono uguali e
`0.0` è diverso da `-0.0`; un `binary` si confronta sui byte. Una chiave con
un null in una qualunque colonna è nulla: a destra non si registra mai, a
sinistra decide `allow_null`.

#### Schema

Quello della tabella sinistra, invariato: colonne, tipi, nullabilità,
metadati di campo e di schema; il contratto conserva ordinamento dichiarato
(`sorted_by`) e numero di righe della sinistra. Nulla della destra entra
nell'uscita.

#### Righe

La sinistra 1:1 se l'asserzione regge. Le righe destre duplicate o senza
riscontro a sinistra non contano. Se l'asserzione non regge, nessuna
uscita; nessuna riga viene scartata.

#### Ordine

L'ordine della tabella sinistra.

#### Errori

In validazione, `InvalidPlan`:

- `left_keys` vuoto, liste di lunghezza diversa, nomi ripetuti o non validi,
  oltre 4096 nomi;
- una colonna chiave non esiste, non è leggibile come testo scalare, o ha
  un tipo diverso da quello della colonna abbinata;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: righe sinistre con una chiave
  assente a destra (causa `validation.foreign_key_missing`) e, senza
  `allow_null`, con una chiave nulla (causa `validation.foreign_key_null`),
  senza colonna. La diagnostica dà il conteggio per causa e fino a 10
  esempi in ordine di riga, con l'indice (da zero) della riga della
  sinistra nella base del runner
  ([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga));
  mai i valori;
- `ResourceLimit`: le chiavi distinte della destra superano il margine di
  memoria che il runner passa al kernel (`max_governed_memory_bytes`):
  ogni chiave conta la lunghezza della sua forma testuale più 64 byte;
- `Schema`: una cella di chiave non si converte in testo (`date32`, `date64` o
  `timestamp` fuori dall'intervallo di calendario, `date64` non allineato al giorno).

#### Limiti e deviazioni

L'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).
La memoria contata è quella delle chiavi della destra, non quella della
mappa che le contiene.

#### Complessità

Tempo O(n + m) atteso su righe sinistre e destre; memoria O(d) per le
chiavi distinte della destra, più O(r) per le righe rifiutate.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_foreign_key", "in": ["ordini", "clienti"],
 "config": {"left_keys": ["cliente"], "right_keys": ["id"], "allow_null": true}}
```

Ingresso `ordini`:

| `ordine: int64` | `cliente: int64` |
| --- | --- |
| 101 | 1 |
| 102 | null |
| 103 | 1 |

Ingresso `clienti`:

| `id: int64` | `nome: utf8` |
| --- | --- |
| 1 | Anna |
| 2 | Bruno |

Uscita `risultato`:

| `ordine: int64` | `cliente: int64` |
| --- | --- |
| 101 | 1 |
| 102 | null |
| 103 | 1 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_metadata`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_metadata` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Verifica i metadati di schema dell'ingresso: ogni coppia di `expected` deve
esserci, con lo stesso valore; con `allow_extra=false` non devono esserci
altre chiavi. Se l'asserzione regge, l'uscita è l'ingresso invariato; se non
regge, il passo fallisce e non produce uscita. Guarda solo lo schema: nel
runner l'esito si decide in validazione.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `expected` | oggetto stringa → stringa | obbligatorio | da 1 a 4096 coppie; chiavi non vuote; chiavi e valori al più `max_string_bytes` byte | coppie che i metadati di schema devono contenere |
| `allow_extra` | booleano | `true` | `true`, `false` | con `false` i metadati hanno esattamente le chiavi di `expected` |

Il confronto è fra testi, byte per byte. Solo i metadati di schema, non
quelli di campo. Il runner toglie dagli ingressi la chiave `pandas` prima
della validazione: non si può asserire.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

#### Righe

1:1: tutte le righe, invariate.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `expected` vuoto, oltre 4096 coppie, con una chiave vuota o una chiave
  o un valore oltre `max_string_bytes`;
- una chiave di `expected` manca dai metadati di schema o ha un altro
  valore (il messaggio nomina la chiave, non i valori);
- `allow_extra=false` e metadati con chiavi in più;
- config con campi sconosciuti.

In esecuzione: nel runner nessuno, perché i metadati dei dati sono quelli
del contratto controllato in validazione. Il kernel chiamato fuori dal
runner rifiuta gli stessi casi con `Schema` (`metadata non conforme`).

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(k) sulle coppie attese, indipendente dalle righe; memoria O(1):
l'uscita condivide le colonne dell'ingresso.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_metadata", "in": ["comuni"],
 "config": {"expected": {"fonte": "anagrafe"}}}
```

Ingresso `comuni` (metadati di schema `{"fonte":"anagrafe"}`):

| `codice: utf8` |
| --- |
| 001 |
| 002 |

Uscita `risultato`:

| `codice: utf8` |
| --- |
| 001 |
| 002 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_not_null`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_not_null` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 1, kernel 2 |

#### Che cosa fa

Verifica che nessuna delle colonne `columns` contenga un null. Se
l'asserzione regge, l'uscita è l'ingresso invariato; se una riga ha un null
in una di quelle colonne, il passo fallisce con una diagnostica che indica
quali righe e per quale colonna, senza produrre uscita. Conta il null logico:
anche una voce di dizionario che punta a un valore nullo.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne dell'ingresso di qualunque tipo | colonne che non devono contenere null |

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe. La nullabilità dichiarata non cambia: l'asserzione non la
restringe a `false`.

#### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita; nessuna riga viene scartata.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con nomi ripetuti o non validi, o oltre 4096 nomi;
- una colonna non esiste nell'ingresso;
- config con campi sconosciuti.

In esecuzione, `DataMapping` con diagnostica per riga: una o più righe hanno
un null in una colonna di `columns`. Ogni riga conta una volta, con la
prima colonna di `columns` in cui è nulla; causa
`validation.required_value_missing`. La diagnostica dà il conteggio per
causa e fino a 10 esempi in ordine di riga, con l'indice (da zero) della
riga nella base del runner
([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga))
e il nome della colonna; mai i valori.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(n·c) su righe e colonne controllate; memoria O(r) per le righe
rifiutate, l'uscita condivide le colonne dell'ingresso.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_not_null", "in": ["clienti"],
 "config": {"columns": ["id"]}}
```

Ingresso `clienti`:

| `id: int64` | `email: utf8` |
| --- | --- |
| 1 | a@x.it |
| 2 | null |
| 3 | c@x.it |

Uscita `risultato`:

| `id: int64` | `email: utf8` |
| --- | --- |
| 1 | a@x.it |
| 2 | null |
| 3 | c@x.it |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_range`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_range` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 2, analisi 3, kernel 5 |

#### Che cosa fa

Verifica che ogni valore della colonna `column` stia fra `min` e `max`.
Se l'asserzione regge, l'uscita è l'ingresso invariato; se un valore è fuori
intervallo, non è finito o è nullo senza `allow_null`, il passo fallisce con
una diagnostica per riga e non produce uscita.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `int64`, `uint64`, `float64`, `decimal128`, `date32`, `date64`, `timestamp` di ogni unità o `utf8` | colonna da controllare |
| `min` | numero | assente | numero finito, non maggiore di `max` | estremo inferiore |
| `max` | numero | assente | numero finito | estremo superiore |
| `inclusive_min` | booleano | assente (incluso) | `true`, `false`; solo con `min`; `null` non ammesso | se `min` fa parte dell'intervallo |
| `inclusive_max` | booleano | assente (incluso) | `true`, `false`; solo con `max`; `null` non ammesso | se `max` fa parte dell'intervallo |
| `allow_null` | booleano | `false` | `true`, `false` | con `true` le celle nulle passano |

Almeno uno fra `min` e `max`. `inclusive_min` senza `min` (o
`inclusive_max` senza `max`) non avrebbe effetto e si rifiuta.

Come si confronta: nel dominio nativo della colonna, mai attraverso `f64`
lato cella: esatto sugli interi oltre `2^53` e sui decimali. Una `date32` vale
i giorni dall'epoca, un `date64` i millisecondi dall'epoca, un `timestamp` il
suo valore nell'unità della colonna (secondi, milli, micro o nanosecondi
dall'epoca: l'istante, qualunque sia il fuso). Un `utf8` si legge come numero, con gli spazi ai lati
ignorati e la virgola decimale ammessa. Un valore non finito (`inf`, `-inf`,
`NaN`, in `float64` o come testo) è sempre fuori intervallo.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

#### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita; nessuna riga viene scartata.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- né `min` né `max`; `min` maggiore di `max`;
- `inclusive_min` senza `min` o `inclusive_max` senza `max`;
- `min`, `max`, `inclusive_min` o `inclusive_max` `null` espliciti (un
  parametro facoltativo si omette);
- `column` assente o di un tipo fuori dall'elenco;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: righe fuori intervallo o non
  finite (causa `validation.value_out_of_range`) e, senza `allow_null`,
  righe nulle (causa `validation.required_value_missing`). La diagnostica dà
  il conteggio per causa e fino a 10 esempi in ordine di riga, con l'indice
  (da zero) della riga nella base del runner
  ([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga))
  e il nome della colonna; mai i valori;
- `Schema`: una cella `utf8` non nulla che non è un numero. Il passo
  fallisce subito, senza diagnostica per riga.

#### Limiti e deviazioni

`min` e `max` si leggono esatti dal JSON: un intero resta intero anche
oltre `2^53` (`9007199254740993` è quel numero, non il double più vicino),
un decimale resta decimale, anche con esponente (`0.1` è un decimo, `1e-7`
un decimo di milionesimo, non il double più vicino). Un numero che il
double non rappresenta come è scritto (`9007199254740993.0`, un intero
oltre `u64` non riletto esatto) si rifiuta nel piano
([README, «Letterali JSON oltre `u64`»](../README.md#letterali-json-oltre-u64)).
Il testo non numerico in una colonna `utf8` fallisce solo in esecuzione
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).

#### Complessità

Tempo O(n) sulle righe; memoria O(r) per le righe rifiutate, l'uscita
condivide le colonne dell'ingresso.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_range", "in": ["ordini"],
 "config": {"column": "sconto", "min": 0, "max": 50, "inclusive_max": false, "allow_null": true}}
```

Ingresso `ordini`:

| `id: int64` | `sconto: decimal128(5, 2)` |
| --- | --- |
| 1 | 0.00 |
| 2 | null |
| 3 | 49.99 |

Uscita `risultato`:

| `id: int64` | `sconto: decimal128(5, 2)` |
| --- | --- |
| 1 | 0.00 |
| 2 | null |
| 3 | 49.99 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_regex`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_regex` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 1, kernel 2 |

#### Che cosa fa

Verifica che ogni valore della colonna di testo `column` corrisponda
all'espressione regolare `pattern`. Se l'asserzione regge, l'uscita è
l'ingresso invariato; se un valore non corrisponde, o è nullo senza
`allow_null`, il passo fallisce con una diagnostica per riga e non produce
uscita. La corrispondenza è una ricerca nel testo: senza `^` e `$` basta che
una parte del valore corrisponda.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | colonna da controllare |
| `pattern` | stringa | obbligatorio | regex non vuota, sintassi del crate `regex`, al più `max_regex_bytes` byte | espressione che ogni valore deve contenere |
| `allow_null` | booleano | `false` | `true`, `false` | con `true` le celle nulle passano |

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

#### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita; nessuna riga viene scartata.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `pattern` vuoto, oltre `max_regex_bytes` o non compilabile;
- `column` assente o non `utf8` (un dizionario di testi non basta);
- config con campi sconosciuti.

In esecuzione, `DataMapping` con diagnostica per riga: righe il cui valore
non corrisponde (causa `validation.regex_mismatch`) e, senza `allow_null`,
righe nulle (causa `validation.required_value_missing`). La diagnostica dà il
conteggio per causa e fino a 10 esempi in ordine di riga, con l'indice (da
zero) della riga nella base del runner
([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga))
e il nome della colonna; mai i valori.

#### Limiti e deviazioni

Sintassi e limiti del crate `regex`: niente riferimenti all'indietro né
lookaround, tempo lineare nella lunghezza del testo. `max_regex_bytes` è un
limite del piano (default 65536 byte, 64 KiB), lo stesso per il piano e per
i kernel.

#### Complessità

Tempo O(n·m) su righe e lunghezza dei testi; memoria O(r) per le righe
rifiutate, più l'automa della regex.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_regex", "in": ["sedi"],
 "config": {"column": "provincia", "pattern": "^[A-Z]{2}$", "allow_null": true}}
```

Ingresso `sedi`:

| `id: int64` | `provincia: utf8` |
| --- | --- |
| 1 | MI |
| 2 | null |
| 3 | TO |

Uscita `risultato`:

| `id: int64` | `provincia: utf8` |
| --- | --- |
| 1 | MI |
| 2 | null |
| 3 | TO |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_schema`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_schema` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Verifica che lo schema dell'ingresso sia quello atteso: per ogni voce di
`fields` una colonna con quel nome, un tipo della famiglia indicata e, se
richiesta, quella nullabilità. Se l'asserzione regge, l'uscita è l'ingresso
invariato; se non regge, il passo fallisce e non produce uscita. Guarda solo
lo schema, mai i valori: nel runner l'esito si decide in validazione.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `fields` | lista di oggetti | obbligatorio | almeno una voce, nomi non ripetuti, al più 4096 | colonne attese, ciascuna con `name`, `data_type`, `nullable` |
| `fields[].name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte | nome della colonna attesa |
| `fields[].data_type` | stringa | obbligatorio | vedi sotto (maiuscole e spazi ai lati ignorati) | famiglia di tipo attesa |
| `fields[].nullable` | booleano | assente | `true`, `false` | nullabilità attesa; assente, non si controlla |
| `allow_extra` | booleano | `false` | `true`, `false` | con `false` l'ingresso ha esattamente tante colonne quante voci in `fields` |
| `ordered` | booleano | `true` | `true`, `false` | con `true` la voce *i* descrive la colonna in posizione *i*; con `false` la colonna si cerca per nome |

Valori di `data_type` e tipi che accettano:

- `utf8` o `string`: `utf8`; `int64` o `integer`: `int64`; `float64`,
  `float` o `double`: `float64`; `boolean` o `bool`: `bool`; `uint64` o
  `unsigned`: `uint64`; `date32`; `binary`: tipo identico;
- `timestamp_seconds`, `timestamp_millis`, `timestamp_micros`,
  `timestamp_nanos`: `timestamp` in secondi, millisecondi, microsecondi o
  nanosecondi, con o senza fuso orario (l'unità conta, il fuso no);
- `decimal128`: `decimal128` di qualunque precisione e scala;
- `dictionary_utf8`: dizionario con chiavi `int32` e valori `utf8`;
- `list`: qualunque lista; `struct`: qualunque struct.

Gli altri tipi Arrow (`int32`, `float32`, `date64`…) non si possono
asserire. Con `ordered=true` e `allow_extra=true` le prime colonne devono
essere quelle di `fields`, nell'ordine, e le altre seguono libere.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

#### Righe

1:1: tutte le righe dell'ingresso, invariate.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `fields` vuoto, con nomi ripetuti o non validi, o oltre 4096 voci;
- `allow_extra=false` e numero di colonne diverso dal numero di voci;
- colonna attesa assente (per posizione con `ordered=true`, per nome con
  `ordered=false`), o in posizione con un nome diverso;
- tipo della colonna fuori dalla famiglia attesa, o `data_type` non in
  elenco;
- `nullable` scritto e diverso da quello della colonna;
- config con campi sconosciuti.

In esecuzione: nel runner nessuno, perché lo schema dei dati è quello del
contratto controllato in validazione. Il kernel chiamato fuori dal runner
rifiuta gli stessi casi con `Schema` (colonna assente o fuori posto, tipo o
nullabilità diversi, numero di colonne) e con `InvalidPlan` (`data_type` non
in elenco).

#### Limiti e deviazioni

Le famiglie di tipo sono volutamente larghe: `decimal128`, i `timestamp_*`,
`list` e `struct` non controllano precisione, scala, fuso, tipo degli
elementi né campi.

#### Complessità

Tempo O(k) sulle voci di `fields`, indipendente dalle righe; memoria O(1):
l'uscita condivide le colonne dell'ingresso.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_schema", "in": ["clienti"],
 "config": {"fields": [
    {"name": "id", "data_type": "int64", "nullable": true},
    {"name": "nome", "data_type": "string"}
  ]}}
```

Ingresso `clienti`:

| `id: int64` | `nome: utf8` |
| --- | --- |
| 1 | Anna |
| 2 | null |

Uscita `risultato`:

| `id: int64` | `nome: utf8` |
| --- | --- |
| 1 | Anna |
| 2 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.assert_unique`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `assert_unique` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 2, kernel 4 |

#### Che cosa fa

Verifica che la chiave formata dalle colonne `columns` sia unica nella
tabella: nessuna coppia di righe ha gli stessi valori in tutte le colonne
della chiave. Se l'asserzione regge, l'uscita è l'ingresso invariato; se
ci sono duplicati, il passo fallisce con una diagnostica che indica tutte le
righe dei gruppi duplicati, senza produrre uscita.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne leggibili come testo scalare | colonne della chiave |
| `nulls_equal` | booleano | `true` | `true`, `false` | con `true` il null è un valore della chiave e due null sono uguali; con `false` le righe con un null in una colonna della chiave non si controllano |

Leggibili come testo scalare: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`binary`, `date32`, `date64`,
`timestamp` di ogni unità (con fuso valido), `decimal128` (scala
da 0 a 38), dizionario `int32`→`utf8`.

Due valori sono uguali quando lo è la loro forma testuale: sugli interi, i
testi, le date e i decimali è l'uguaglianza dei valori; su `float64` tutti
i `NaN` sono uguali fra loro e `0.0` è diverso da `-0.0`; un `binary` si
confronta sui byte.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità, metadati di campo
e di schema; il contratto conserva ordinamento dichiarato (`sorted_by`) e
numero di righe.

#### Righe

1:1 se l'asserzione regge: tutte le righe, invariate. Altrimenti nessuna
uscita; nessuna riga viene scartata.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con nomi ripetuti o non validi, o oltre 4096 nomi;
- una colonna non esiste o non è leggibile come testo scalare;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: almeno una chiave compare in più
  righe. Sono rifiutate tutte le righe di ogni gruppo duplicato, la prima
  compresa, con causa `validation.duplicate_key` e senza colonna. La
  diagnostica dà il conteggio delle righe e fino a 10 esempi in ordine di
  riga, con l'indice (da zero) della riga nella base del runner
  ([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga));
  mai i valori;
- `Schema`: una cella della chiave non si converte in testo (`date32`, `date64` o
  `timestamp` fuori dall'intervallo di calendario, `date64` non allineato al giorno, chiave di dizionario
  fuori dal dizionario). Le righe saltate con `nulls_equal=false` non si
  convertono.

#### Limiti e deviazioni

La mappa delle chiavi non si conta su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n·c) atteso su righe e colonne della chiave; memoria O(d) per le
chiavi distinte, più O(r) per le righe rifiutate.

#### Memoria

Memoria: da misura v4.

#### Esempio

Con `nulls_equal=false` i due null non sono un duplicato.

Passo del piano:

```json
{"out": "risultato", "op": "table.assert_unique", "in": ["prodotti"],
 "config": {"columns": ["codice"], "nulls_equal": false}}
```

Ingresso `prodotti`:

| `codice: utf8` | `prezzo: float64` |
| --- | --- |
| A1 | 9.5 |
| null | 3.0 |
| B2 | 12.0 |
| null | 3.0 |

Uscita `risultato`:

| `codice: utf8` | `prezzo: float64` |
| --- | --- |
| A1 | 9.5 |
| null | 3.0 |
| B2 | 12.0 |
| null | 3.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.bin`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `bin` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 4, kernel 4 |

#### Che cosa fa

Divide i valori numerici di una colonna in classi e scrive, per ogni riga,
l'etichetta della sua classe in una colonna di testo. Le classi sono
intervalli chiusi a destra, `(a, b]`, dati come bordi espliciti oppure come
numero di classi di uguale ampiezza fra il minimo e il massimo dei dati.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica | colonna da classificare |
| `bins` | intero o lista di numeri | `5` | intero da 2 a 100, oppure da 3 a 101 bordi strettamente crescenti | numero di classi di uguale ampiezza, o bordi delle classi |
| `labels` | lista di stringhe | assente | tante quante le classi, ciascuna al più `max_string_bytes` byte | etichette delle classi, nell'ordine; assenti, `(a, b]` |
| `output_column` | stringa | `<column>_bin` | nome valido | colonna d'uscita |

Colonna numerica: `float64`, `int64`, `uint64`, `date32` (giorni
dall'epoca), `date64` (millisecondi dall'epoca), `timestamp` di ogni unità
(il valore nell'unità della colonna), `decimal128`,
`utf8` il cui testo è un numero (spazi ai lati ignorati, virgola decimale
ammessa).

Con bordi espliciti `[e0, e1, …, ek]` la classe i è `(e_i, e_i+1]`, e la
prima comprende anche `e0`; un valore sotto `e0` o sopra `ek` non ha
classe. Con un numero k di classi i bordi si calcolano in `f64`: minimo e
massimo dei valori finiti, ampiezza `(max - min) / k`, ultimo bordo uguale
al massimo, primo bordo abbassato di `(max - min) · 0,001` (come
`pandas.cut`); con tutti i valori uguali i bordi vanno da `v - d` a `v + d`,
con `d = |v| · 0,001` (`0,001` se `v` è zero). In questo modo un valore
sotto il primo bordo o sopra l'ultimo (anche `±inf`) cade nella classe
esterna.

I bordi espliciti si leggono esatti dal JSON, come `min` e `max` di
[`table.assert_range`](#tableassert_range): un bordo intero oltre `2^53`
resta quello scritto.

L'etichetta di default (senza `labels`) scrive i bordi con la resa decimale
più corta di `f64` (`(0, 18]`, `(2.5, 5]`); un bordo esplicito intero si
scrive con tutte le sue cifre (`(9007199254740992, 9007199254740993]`).
Non supera `max_string_bytes` byte, controllato in esecuzione.

#### Schema

La colonna d'uscita è `utf8` nullable: si aggiunge in coda, o sostituisce
al suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1. Una cella nulla, un `NaN` o un valore fuori dai bordi espliciti danno
null.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non numerica;
- numero di classi fuori da 2..=100, bordi fuori da 3..=101 o non
  strettamente crescenti;
- numero di `labels` diverso dal numero di classi; un'etichetta oltre
  `max_string_bytes` byte;
- config con campi sconosciuti, o `null` esplicito in `labels` o
  `output_column` (il parametro si omette).

In esecuzione:

- `Schema`: con un numero di classi, nessun valore finito nella colonna
  (anche una tabella vuota o tutta nulla); una cella `utf8` che non è un
  numero;
- `ResourceLimit`: senza `labels`, un'etichetta di default oltre
  `max_string_bytes` byte.

#### Limiti e deviazioni

I bordi di uguale ampiezza sono calcolati in `f64`; la classe invece la
decide il confronto esatto del valore d'origine con il bordo, quindi un
`int64` oltre `2^53` o un `decimal128` non cadono nella classe accanto per
arrotondamento ([README, «Validazione»](../README.md#validazione)).

#### Complessità

Tempo O(n log k) per n righe e k classi (ricerca binaria sui bordi,
scansione lineare se i bordi calcolati coincidono per arrotondamento);
memoria O(n) per i valori letti e la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.bin", "in": ["persone"],
 "config": {"column": "eta", "bins": [0, 18, 65, 120], "labels": ["minore", "adulto", "anziano"]}}
```

Ingresso `persone`:

| `eta: int64` |
| --- |
| 0 |
| 18 |
| 70 |
| 130 |

Uscita `risultato`:

| `eta: int64` | `eta_bin: utf8` |
| --- | --- |
| 0 | minore |
| 18 | minore |
| 70 | anziano |
| 130 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.coalesce`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `coalesce` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 1, analisi 1, kernel 2 |

#### Che cosa fa

Riga per riga, prende il primo valore non nullo fra le colonne `columns`,
nell'ordine in cui sono elencate, e lo scrive in `output_column`. Se tutte
sono nulle, il risultato è null. Le colonne devono avere lo stesso tipo.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne dell'ingresso con tipi Arrow identici | colonne da cui prendere il valore, in ordine di precedenza |
| `output_column` | stringa | obbligatorio | nome non vuoto, al più 1024 byte | colonna del risultato |

Il null è quello logico: una voce di dizionario che punta a un valore nullo
conta come null e si passa alla colonna successiva.

#### Schema

`output_column` ha il tipo comune delle colonne `columns` (anche un
dizionario, una lista o una struct) ed è sempre nullabile. Se il nome esiste
già, anche se è una delle colonne di `columns`, la colonna si sostituisce
nella sua posizione e perde i propri metadati di campo; altrimenti si
aggiunge in coda. Le altre colonne restano invariate, e così i metadati di
schema. Se la colonna sostituita è la geometria, l'uscita non ha più una
colonna geometrica.

Il contratto conserva il numero di righe; l'ordinamento dichiarato
(`sorted_by`) resta solo se nessuna colonna è stata sostituita.

#### Righe

1:1: una riga d'uscita per riga d'ingresso.

#### Ordine

L'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con nomi ripetuti o non validi, o oltre 4096 nomi;
- una colonna non esiste, o i tipi Arrow non sono identici (anche
  precisione, scala, fuso orario);
- `output_column` vuoto o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione: nessuno che dipenda dai dati. `ResourceLimit` se gli indici
interni del percorso generico traboccano (righe per colonne oltre
`usize`), e `DataMapping` (`arrow error`) se Arrow non riesce a concatenare
le colonne: entrambi fuori dai volumi ammessi dai limiti.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(n·c) su righe e colonne elencate; memoria O(n) per la colonna
prodotta. Per i tipi fuori da `int64`, `uint64`, `float64`, `bool` e `utf8`
il percorso generico concatena le colonne: memoria O(n·c).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.coalesce", "in": ["contatti"],
 "config": {"columns": ["cellulare", "fisso"], "output_column": "telefono"}}
```

Ingresso `contatti`:

| `id: int64` | `cellulare: utf8` | `fisso: utf8` |
| --- | --- | --- |
| 1 | 333 1 | 02 9 |
| 2 | null | 06 5 |
| 3 | null | null |

Uscita `risultato`:

| `id: int64` | `cellulare: utf8` | `fisso: utf8` | `telefono: utf8` |
| --- | --- | --- | --- |
| 1 | 333 1 | 02 9 | 333 1 |
| 2 | null | 06 5 | 06 5 |
| 3 | null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.concat`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `concat` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | N-aria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine d'arrivo degli ingressi |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Impila le righe di due tabelle con lo stesso schema: prima tutte le righe
di sinistra, poi tutte quelle di destra. Le colonne si abbinano per
posizione e devono avere, posizione per posizione, lo stesso nome e lo
stesso tipo. Per unire tabelle con colonne diverse o in ordine diverso si
usa [`table.concat_by_name`](#tableconcat_by_name).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `ignore_index` | booleano | assente | nessuno: scritto si rifiuta, anche `null` | una tabella Arrow non ha indice di riga, nessun valore avrebbe effetto |

#### Schema

Le colonne della sinistra, con nome, tipo e metadati di campo della
sinistra; una colonna è nullable se lo è in almeno un ingresso. I metadati
di schema dei due ingressi si fondono: una chiave presente in un ingresso
solo, o con lo stesso valore, resta. La colonna geometrica è quella della
sinistra. Del contratto resta il conteggio delle righe, somma dei due
ingressi quando entrambi lo dichiarano con la stessa portata e la stessa
confidenza; l'ordinamento dichiarato (`sorted_by`) no.

#### Righe

Tutte le righe di entrambi gli ingressi, senza deduplicazione e con i
valori invariati: l'uscita ha `n + m` righe.

#### Ordine

Le righe di sinistra nel loro ordine, poi quelle di destra nel loro
ordine.

#### Errori

In validazione, `InvalidPlan`:

- numero di colonne diverso, o nome o tipo diversi in una posizione (la
  nullabilità non conta);
- metadati di schema con la stessa chiave e valori diversi;
- `ignore_index` scritto, con qualunque valore, anche `null` (anche nel
  kernel, con la stessa funzione);
- config con campi sconosciuti.

Nel runner, `table.concat` con più di due ingressi è `Unsupported`, con
meno di due `InvalidPlan`.

In esecuzione, `ResourceLimit`:

- righe totali oltre `max_rows` (nel runner `max_input_rows`);
- uscita stimata (righe per la larghezza di riga maggiore fra i due
  ingressi) oltre `max_governed_memory_bytes`, prima di copiare.

#### Limiti e deviazioni

Il catalogo la dichiara N-aria, ma il runner esegue solo la forma a due
ingressi ([README, «Validazione»](../README.md#validazione)); più tabelle
si impilano con passi in catena. I metadati di campo della destra non si
conservano.

#### Complessità

Tempo e memoria O(n + m): ogni colonna si copia nell'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.concat", "in": ["gennaio", "febbraio"],
 "config": {}}
```

Ingresso `gennaio`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 1 | 10.5 |
| 2 | null |

Ingresso `febbraio`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 2 | 7.0 |
| 3 | 3.25 |

Uscita `risultato`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 1 | 10.5 |
| 2 | null |
| 2 | 7.0 |
| 3 | 3.25 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.concat_by_name`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | N-aria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine d'arrivo degli ingressi |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Impila le righe di due tabelle abbinando le colonne per nome, non per
posizione: prima tutte le righe di sinistra, poi quelle di destra. Una
colonna che manca in un ingresso vale null nelle sue righe. Con `strict`
gli schemi devono essere identici, come in [`table.concat`](#tableconcat).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `strict` | booleano | `false` | `true`, `false` | `true`: stesse colonne, con stesso nome e tipo, nello stesso ordine |

#### Schema

Senza `strict`, l'unione delle colonne nell'ordine di prima apparizione:
le colonne di sinistra, poi quelle di destra con un nome nuovo. Una colonna
presente in entrambi gli ingressi ha lo stesso tipo nei due (nessuna
conversione) e prende i metadati di campo della sinistra. Una colonna è
nullable se manca in un ingresso o è nullable in almeno uno. Con `strict`
lo schema è quello della sinistra, nullable dove lo è in almeno un
ingresso.

I metadati di schema dei due ingressi si fondono: una chiave presente in un
ingresso solo, o con lo stesso valore, resta. La colonna geometrica della
sinistra si conserva solo se la destra ha una colonna con lo stesso nome e
lo stesso tipo; altrimenti l'uscita non dichiara geometria. Del contratto
resta il conteggio delle righe (somma, come in `table.concat`), non
l'ordinamento dichiarato.

#### Righe

Tutte le righe di entrambi gli ingressi, senza deduplicazione: `n + m`
righe. Nelle righe di un ingresso le colonne che quell'ingresso non ha
sono nulle.

#### Ordine

Le righe di sinistra nel loro ordine, poi quelle di destra nel loro
ordine.

#### Errori

In validazione, `InvalidPlan`:

- una colonna con lo stesso nome e tipi diversi nei due ingressi;
- con `strict`, numero di colonne diverso o nome o tipo diversi in una
  posizione;
- metadati di schema con la stessa chiave e valori diversi;
- config con campi sconosciuti.

Nel runner, più di due ingressi sono `Unsupported`, meno di due
`InvalidPlan`.

In esecuzione, `ResourceLimit`:

- righe totali oltre `max_rows` (nel runner `max_input_rows`);
- uscita stimata oltre `max_governed_memory_bytes`, prima di copiare. La
  stima conta per ogni colonna dell'unione la larghezza maggiore fra gli
  ingressi che la hanno, così anche le colonne di null aggiunte.

#### Limiti e deviazioni

Come `table.concat`, il catalogo la dichiara N-aria e il runner esegue solo
la forma a due ingressi ([README, «Validazione»](../README.md#validazione)).
Due colonne con lo stesso nome e tipi diversi si rifiutano: nessuna
promozione di tipo.

#### Complessità

Tempo e memoria O((n + m)·c), con `c` le colonne dell'unione: ogni colonna
si copia, e quelle mancanti si riempiono di null.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.concat_by_name", "in": ["negozio", "online"],
 "config": {}}
```

Ingresso `negozio`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 1 | 10.5 |
| 2 | 4.0 |

Ingresso `online`:

| `canale: utf8` | `id: int64` |
| --- | --- |
| web | 3 |

Uscita `risultato`:

| `id: int64` | `importo: float64` | `canale: utf8` |
| --- | --- | --- |
| 1 | 10.5 | null |
| 2 | 4.0 | null |
| 3 | null | web |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.concat_columns`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `concat_columns` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Unisce riga per riga il testo delle colonne `columns`, nell'ordine dato e
con `separator` fra una parte e l'altra, in una colonna `utf8`. I null si
saltano (default) o contano come testo vuoto.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | colonne `utf8` dell'ingresso, almeno una, senza ripetizioni, al più 4096 | parti da unire, nell'ordine |
| `output_column` | stringa | `"concatenated"` | nome non vuoto, al più 1024 byte | colonna d'uscita |
| `separator` | stringa | `" "` | al più `max_string_bytes` byte, anche vuota; solo con almeno due colonne; `null` non ammesso | testo messo fra due parti |
| `skip_null` | booleano | `true` | `true`, `false` | salta i null invece di trattarli come testo vuoto |

Con `skip_null` il separatore sta solo fra le parti non nulle, e una riga
di soli null dà null. Senza, un null vale `""` e il separatore resta
(`"a"`, null, `"b"` con `"-"` dà `"a--b"`); il risultato non è mai null.

#### Schema

La colonna `output_column`, `utf8` nullable: se esiste già si sostituisce
nella sua posizione (perde tipo e metadati di campo di prima), altrimenti
si aggiunge in coda. Le altre colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
se `output_column` è una colonna nuova, cade se ne sostituisce una.

#### Righe

1:1: una riga d'uscita per riga d'ingresso.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `columns` assente, vuota, con un nome ripetuto o con più di 4096 nomi;
- una colonna di `columns` assente o non `utf8`;
- `separator` oltre `max_string_bytes`, `null` esplicito (il parametro si
  omette), o scritto con una sola colonna in `columns` (non avrebbe
  effetto);
- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione, `ResourceLimit`: un valore unito oltre `max_string_bytes`
byte.

#### Limiti e deviazioni

Solo colonne `utf8`: un numero o una data vanno prima convertiti in testo
con [`table.type_cast`](#tabletype_cast).

#### Complessità

Tempo O(n·k) per n righe e k colonne, più i byte copiati; memoria pari ai
byte della colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.concat_columns", "in": ["persone"],
 "config": {"columns": ["nome", "secondo", "cognome"], "output_column": "completo"}}
```

Ingresso `persone`:

| `nome: utf8` | `secondo: utf8` | `cognome: utf8` |
| --- | --- | --- |
| Anna | Maria | Rossi |
| Luca | null | Bianchi |
| null | null | null |

Uscita `risultato`:

| `nome: utf8` | `secondo: utf8` | `cognome: utf8` | `completo: utf8` |
| --- | --- | --- | --- |
| Anna | Maria | Rossi | Anna Maria Rossi |
| Luca | null | Bianchi | Luca Bianchi |
| null | null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.conditional`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `conditional` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 4, kernel 4 |

#### Che cosa fa

Aggiunge una colonna calcolata con regole «se… allora»: per ogni riga
valuta le `conditions` sulla colonna `column`, nell'ordine, e scrive il
`result` della prima vera; se nessuna è vera scrive `default_value`. Le
condizioni si valutano come in [`table.filter`](#tablefilter). La colonna
d'uscita è numerica (`float64`) se tutti i risultati possibili sono numeri,
testuale (`utf8`) altrimenti.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna dell'ingresso | colonna su cui si valutano le condizioni |
| `conditions` | lista di oggetti | obbligatorio | da 1 a 4096 condizioni | regole, in ordine di precedenza |
| `conditions[].operator` | stringa | `"=="` | gli operatori di `table.filter` | confronto fra la cella e `value` |
| `conditions[].value` | JSON | `null` | come `value` di `table.filter`; con `isnull`, `notnull` non si scrive | termine di confronto |
| `conditions[].result` | JSON | `null` | stringa, numero, booleano o `null`; testo al più `max_string_bytes` byte | valore scritto se la condizione è la prima vera |
| `default_value` | JSON | `null` | stringa, numero, booleano o `null`; testo al più `max_string_bytes` byte | valore scritto se nessuna condizione è vera |
| `output_column` | stringa | `"result"` | nome non vuoto, al più 1024 byte | colonna d'uscita |

Il tipo d'uscita dipende solo dalla config. Ogni `result` e il
`default_value` si leggono come testo (una stringa com'è, un numero o un
booleano come testo JSON, `null` come testo vuoto):

- se ogni testo è vuoto o, sostituite le virgole con punti, un numero per
  il parse `f64` di Rust (esponente ammesso), l'uscita è `float64`
  nullable: il testo vuoto dà null, gli altri il numero (`"1,5"` dà 1,5,
  `"1e3"` dà 1000). Un risultato scritto come intero che il `float64` non
  rappresenta esattamente (`9007199254740993`) si rifiuta invece di
  diventare un altro intero;
- altrimenti l'uscita è `utf8` non nullable e ogni cella è il testo del
  valore scelto: `null` dà `""`, `true` dà `"true"`, `2` dà `"2"`.

Un `result` o un `default_value` che si legge come numero non finito
(`"NaN"`, `"inf"`, `"1e999"`) si rifiuta, in validazione e nel kernel.

Una cella nulla non soddisfa nessun operatore tranne `isnull`. Con
`isnull` e `notnull` il `value` non avrebbe effetto: scritto, anche
`null`, si rifiuta.

#### Schema

La colonna `output_column`: se esiste già si sostituisce nella sua
posizione (con il tipo nuovo, senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se `output_column` è una colonna nuova.

#### Righe

1:1: ogni riga riceve esattamente un valore.

#### Ordine

Righe nell'ordine d'ingresso; per ogni riga le condizioni nell'ordine di
`conditions`, e vince la prima vera.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente;
- `conditions` assente, vuota o con più di 4096 condizioni;
- per ogni condizione, gli stessi rifiuti di operatore, tipo di colonna e
  `value` di `table.filter`, compreso `value` scritto (anche `null`) con
  `isnull` o `notnull`;
- il testo di un `result` o del `default_value` oltre `max_string_bytes`
  byte, o che si legge come numero non finito;
- uscita `float64` con un risultato intero non esatto in `float64`;
- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione, `Schema`: una cella `utf8` che non è un numero sotto un
operatore ordinato o `between`, o una cella che non si legge come testo
sotto un operatore testuale.

#### Limiti e deviazioni

- **Tipo deciso dai letterali**: basta un risultato non numerico perché
  anche i risultati numerici diventino testo (`2` diventa `"2"`), e nella
  colonna `utf8` il `null` diventa testo vuoto, non null.
- **Virgola decimale**: in un risultato ogni virgola vale un punto, quindi
  `"1,5"` è 1,5 ma `"1.000,5"` non è un numero e rende testuale l'uscita.

#### Complessità

Tempo O(n·k) per n righe e k condizioni; memoria O(n) per la colonna
d'uscita (un testo per riga prima della conversione numerica).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.conditional", "in": ["t"],
 "config": {
    "column": "classe",
    "conditions": [
      {"operator": "==", "value": "A", "result": "1,5"},
      {"operator": "==", "value": "B", "result": 1},
      {"operator": "isnull", "result": null}
    ],
    "default_value": "0",
    "output_column": "coefficiente"
  }}
```

Ingresso `t`:

| `classe: utf8` |
| --- |
| A |
| B |
| null |
| C |

Uscita `risultato`:

| `classe: utf8` | `coefficiente: float64` |
| --- | --- |
| A | 1.5 |
| B | 1.0 |
| null | null |
| C | 0.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.cross_join`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `cross_join` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Prodotto cartesiano: ogni riga di sinistra affiancata a ogni riga di
destra. Non ci sono chiavi; le colonne con lo stesso nome nei due lati
prendono i suffissi `_x` (sinistra) e `_y` (destra).

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Prima tutte le colonne di sinistra, poi tutte quelle di destra. Una colonna
di sinistra il cui nome compare anche a destra diventa `<nome>_x`, una di
destra il cui nome compare anche a sinistra `<nome>_y`; le altre tengono il
nome. Tipi e metadati di campo restano quelli d'origine; ogni colonna
d'uscita è nullable. I metadati di schema dei due lati si fondono: una
chiave presente da un lato solo, o con lo stesso valore, resta. L'uscita
ammette una sola colonna geometrica. Il conteggio delle righe del
contratto è il prodotto dei due, quando entrambi lo dichiarano con la
stessa portata e la stessa confidenza; l'ordinamento dichiarato no.

#### Righe

`n·m` righe, una per ogni coppia (riga sinistra, riga destra). Con un lato
vuoto l'uscita è vuota.

#### Ordine

Comanda la sinistra: per ogni riga di sinistra, nell'ordine d'ingresso,
tutte le righe di destra nel loro ordine.

#### Errori

In validazione, `InvalidPlan`:

- nomi d'uscita che collidono dopo i suffissi (a sinistra `a` e `a_x`, a
  destra `a`), nome oltre 1024 byte, più colonne di `max_columns`;
- due colonne geometriche nell'uscita (una per lato);
- metadati di schema con la stessa chiave e valori diversi;
- config non vuota.

In esecuzione, `ResourceLimit`, prima di allocare l'uscita:

- `n·m` oltre `max_rows` (nel runner `max_input_rows`) o non
  rappresentabile;
- uscita stimata oltre `max_governed_memory_bytes`: righe per la somma
  delle larghezze di riga dei due lati più 32 byte di indici per riga.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo e memoria O(n·m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.cross_join", "in": ["taglie", "colori"],
 "config": {}}
```

Ingresso `taglie`:

| `id: int64` | `taglia: utf8` |
| --- | --- |
| 1 | S |
| 2 | M |

Ingresso `colori`:

| `id: int64` | `colore: utf8` |
| --- | --- |
| 10 | rosso |
| 20 | blu |

Uscita `risultato`:

| `id_x: int64` | `taglia: utf8` | `id_y: int64` | `colore: utf8` |
| --- | --- | --- | --- |
| 1 | S | 10 | rosso |
| 1 | S | 20 | blu |
| 2 | M | 10 | rosso |
| 2 | M | 20 | blu |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.date_add`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `date_add` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 3, analisi 4, kernel 6 |

#### Che cosa fa

Legge date e ore scritte come testo, vi aggiunge (o toglie, con `amount`
negativo) una quantità fissa di anni, mesi, settimane, giorni, ore, minuti
o secondi e scrive il risultato come testo in una colonna nuova. Anni e mesi
seguono il calendario: il giorno oltre la fine del mese diventa l'ultimo
giorno del mese (31 gennaio più un mese è 29 febbraio 2024).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo | colonna da leggere |
| `input_format` | stringa | assente | formato `chrono` non vuoto, al più `max_string_bytes` byte; obbligatorio per un testo, rifiutato per una colonna temporale; `null` non ammesso | formato di lettura di un testo |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte, senza fuso, che scrive al più `max_string_bytes` byte per valore | formato di scrittura |
| `amount` | intero | obbligatorio | intero a 64 bit che almeno una data sopporta | quantità da aggiungere, con segno |
| `unit` | stringa | obbligatorio | `years`, `months`, `weeks`, `days`, `hours`, `minutes`, `seconds` | unità di `amount` |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non leggibile rifiuta sempre la riga, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` in secondi, millisecondi,
microsecondi o nanosecondi, con o senza fuso) si legge dal valore nativo,
senza `input_format` (scritto, si rifiuta): vale l'ora locale della
colonna (del suo fuso; senza fuso, il valore com'è), e una data è la sua
mezzanotte ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)). Ogni altra colonna si legge come testo, con
`input_format` obbligatorio; leggibili come testo: `utf8`, `int64`,
`uint64`, `float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

La lettura deve consumare tutto il testo della cella; un formato senza
campi orari legge una data e la pone a mezzanotte. Il valore non ha fuso:
settimane, giorni, ore, minuti e secondi sono durate fisse (un giorno è
sempre 24 ore, senza ora legale). `years` vale 12 mesi. Un valore non
leggibile rifiuta sempre la riga.

Il testo scritto da `output_format` non può superare `max_string_bytes`
byte per valore. Il limite si controlla in validazione, esatto per campo:
il testo letterale conta per la sua lunghezza, ogni campo `strftime` per
la sua larghezza massima (anno 7 byte col segno; secolo `%C` 2, perché si
scrive solo per gli anni 0..=9999; mese, giorno, ora 2; nome del mese o del
giorno 9; offset `%z` 5, `+0530`, `%:z` 6, `+05:30`, `%::z` 9, `%:::z` 3;
frazioni `%3f`, `%6f`, `%9f` 3, 6, 9 e `%.3f`, `%.6f`, `%.9f` 4, 7, 10;
nome del fuso 32); `%Y%m` scrive al più 9 byte.

#### Schema

La colonna d'uscita è `utf8` nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1. Una cella nulla dà null.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non leggibile come testo; `input_format` assente con
  una colonna di testo, o scritto con una colonna temporale;
- un formato vuoto, oltre `max_string_bytes`, con un campo non
  riconosciuto, o `output_format` con campi di fuso (`%z`, `%:z`, `%Z`,
  `%+`);
- `output_format` che può scrivere più di `max_string_bytes` byte per
  valore;
- `amount` che nessuna data rappresentabile sopporta nell'unità data;
- `output_column` non valido;
- `invalid` scritto, con qualunque valore, anche `null`;
- config con campi sconosciuti o `unit` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non si
  legge con `input_format` (`conversion.invalid_datetime`) o il cui
  risultato esce dalle date rappresentabili (`conversion.datetime_range`);
  il passo non produce uscita;
- `DataMapping`, senza diagnostica per riga: un valore che `output_format`
  non sa scrivere (anno fuori da 0..=9999 con `%C`);
- `Schema`: una cella che non si converte in testo.

#### Limiti e deviazioni

Le date rappresentabili sono quelle di `chrono` (anni da -262143 a
262142). Un `amount` che alcune date sopportano e quelle dei dati no
fallisce in esecuzione
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).

#### Complessità

Tempo O(n) (due letture per riga: il controllo, poi la conversione);
memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.date_add", "in": ["contratti"],
 "config": {"column": "scadenza", "input_format": "%Y-%m-%d", "output_format": "%Y-%m-%d", "amount": 1, "unit": "months", "output_column": "rinnovo"}}
```

Ingresso `contratti`:

| `scadenza: utf8` |
| --- |
| 2024-01-31 |
| 2024-03-15 |
| null |

Uscita `risultato`:

| `scadenza: utf8` | `rinnovo: utf8` |
| --- | --- |
| 2024-01-31 | 2024-02-29 |
| 2024-03-15 | 2024-04-15 |
| null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.date_diff`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `date_diff` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 3, analisi 4, kernel 6 |

#### Che cosa fa

Legge due colonne di date e ore scritte come testo, con lo stesso formato,
e scrive la differenza `fine - inizio` in giorni, ore, minuti o secondi,
come numero con parte frazionaria (un giorno e mezzo è `1.5`) e con segno.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `start_column` | stringa | obbligatorio | colonna temporale o leggibile come testo | istante iniziale |
| `end_column` | stringa | obbligatorio | colonna dello stesso genere di `start_column` | istante finale |
| `input_format` | stringa | assente | formato `chrono` non vuoto, al più `max_string_bytes` byte; obbligatorio per colonne di testo, rifiutato per colonne temporali; `null` non ammesso | formato di lettura di entrambe le colonne |
| `unit` | stringa | obbligatorio | `days`, `hours`, `minutes`, `seconds` | unità della differenza |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non leggibile rifiuta sempre la riga, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` in secondi, millisecondi,
microsecondi o nanosecondi, con o senza fuso) si legge dal valore nativo,
senza `input_format` (scritto, si rifiuta): vale l'ora locale della
colonna (del suo fuso; senza fuso, il valore com'è), e una data è la sua
mezzanotte ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)). Ogni altra colonna si legge come testo, con
`input_format` obbligatorio; leggibili come testo: `utf8`, `int64`,
`uint64`, `float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Le due colonne sono dello stesso genere: due istanti (`timestamp`), due
date (`date32`) o due testi. La lettura di un testo deve consumarlo tutto;
un formato senza campi orari legge una data e la pone a mezzanotte. Due
istanti (colonne `timestamp`, o testi letti con un offset, `%z`/`%:z`) si
sottraggono come istanti; date e ore senza offset come ore locali, e un
giorno è sempre 86 400 secondi. La differenza è il numero di nanosecondi
diviso per `10^9` e poi per 86 400, 3 600, 60 o 1. Un valore non leggibile
rifiuta sempre la riga.

#### Schema

La colonna d'uscita è `float64` nullable: si aggiunge in coda o sostituisce
al suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1. Se una delle due celle è nulla il risultato è null.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `start_column` o `end_column` assenti o non leggibili come testo; di
  generi diversi (un istante e una data, una colonna temporale e un
  testo); `input_format` assente con colonne di testo, o scritto con
  colonne temporali;
- `input_format` vuoto, oltre `max_string_bytes` o con un campo non
  riconosciuto;
- `output_column` non valido;
- `invalid` scritto, con qualunque valore, anche `null`;
- config con campi sconosciuti o `unit` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non si
  legge (`conversion.invalid_datetime`, sulla colonna iniziale se non si
  legge quella, altrimenti sulla finale), o una differenza oltre `i64`
  nanosecondi, circa 292 anni (`conversion.datetime_range`); il passo non
  produce uscita;
- `Schema`: una cella che non si converte in testo.

#### Limiti e deviazioni

Il risultato è `float64` per contratto: oltre `2^53` nanosecondi (circa 104
giorni) il numero esatto di nanosecondi si arrotonda al double più vicino
prima della divisione. Mesi e anni non sono unità ammesse, perché non
hanno durata fissa.

#### Complessità

Tempo O(n) (due letture per riga: il controllo, poi il calcolo); memoria
O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.date_diff", "in": ["attivita"],
 "config": {"start_column": "inizio", "end_column": "fine", "input_format": "%Y-%m-%d %H:%M", "unit": "days", "output_column": "giorni"}}
```

Ingresso `attivita`:

| `inizio: utf8` | `fine: utf8` |
| --- | --- |
| 2024-01-01 00:00 | 2024-01-02 12:00 |
| 2024-03-10 12:00 | 2024-03-01 12:00 |
| 2024-01-01 00:00 | null |

Uscita `risultato`:

| `inizio: utf8` | `fine: utf8` | `giorni: float64` |
| --- | --- | --- |
| 2024-01-01 00:00 | 2024-01-02 12:00 | 1.5 |
| 2024-03-10 12:00 | 2024-03-01 12:00 | -9.0 |
| 2024-01-01 00:00 | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.date_extract`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `date_extract` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 2, analisi 4, kernel 6 |

#### Che cosa fa

Legge la colonna `column` come data o data e ora e ne estrae le parti
chieste (anno, mese, giorno, trimestre, giorno della settimana, settimana
ISO, ora, minuto, secondo), una colonna `int64` per parte. Se anche un
solo valore non si interpreta come data il passo fallisce e dice quali
righe.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo | date da leggere |
| `parts` | lista di stringhe | `["year"]` | non vuota, senza ripetizioni, fra `year`, `month`, `day`, `quarter`, `weekday`, `week`, `hour`, `minute`, `second` | parti da estrarre, nell'ordine delle colonne d'uscita |
| `prefix` | stringa | `""` | qualunque; `""` vale `<column>_` | prefisso dei nomi d'uscita (`<prefix><parte>`) |
| `date_format` | stringa o `null` | `null` | formato strftime di chrono, non vuoto, al più `max_string_bytes` byte; solo con una colonna di testo | formato delle date; `null` usa i formati ISO di default |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non interpretabile fa sempre fallire il passo, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` di ogni unità, con o senza
fuso) si legge dal valore nativo, senza `date_format` (scritto, si
rifiuta): le parti sono dell'ora locale della colonna (del suo fuso; senza
fuso, il valore com'è). Ogni altra cella non nulla si legge come testo e
si interpreta:

- con `date_format`: prima come data e ora con quel formato, poi come sola
  data a mezzanotte;
- senza, solo ISO 8601: RFC 3339 con offset (`2024-01-31T10:00:00Z`,
  `2024-01-31 10:00:00.5+01:00`: le parti sono dell'ora scritta), data e
  ora con `T` o spazio e frazione facoltativa (`%Y-%m-%dT%H:%M:%S%.f`,
  `%Y-%m-%d %H:%M:%S%.f`), poi `%Y-%m-%d` a mezzanotte. Nessun formato con
  giorno e mese in un ordine da indovinare (`31/01/2024` serve un
  `date_format`) ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)). Gli spazi non si tolgono.

Le parti: `year` l'anno del calendario gregoriano; `month` 1-12; `day`
1-31; `quarter` 1-4; `weekday` 0 per il lunedì fino a 6 per la domenica;
`week` il numero di settimana ISO 8601, 1-53, che a cavallo d'anno può
appartenere all'anno vicino (il 2021-01-01 è nella settimana 53);
`hour`, `minute`, `second`.

Un valore non interpretabile fa sempre fallire il passo: per questo
`invalid` scritto, con qualunque valore (anche `null`), si rifiuta.

#### Schema

Una colonna `int64` nullable per parte, di nome `<prefix><parte>`: se
esiste già si sostituisce nella sua posizione (senza i metadati di campo di
prima), altrimenti si aggiunge in coda nell'ordine di `parts`. Le altre
colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita.

#### Righe

1:1; la cella null dà null in ogni parte.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o di un tipo che non si legge come testo;
- `date_format` vuoto, oltre `max_string_bytes`, con un elemento strftime
  non riconosciuto, o scritto con una colonna temporale;
- un nome d'uscita `<prefix><parte>` vuoto, di soli spazi o oltre 1024
  byte;
- `parts` vuota o con una parte ripetuta;
- una parte fuori elenco, `invalid` scritto, config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: almeno un valore non si
  interpreta come data. Causa `conversion.invalid_datetime`, conteggio e
  primi 10 esempi con indice di riga (da 0) e colonna, mai il valore;
- `Schema`: una cella che non si legge come testo.

#### Limiti e deviazioni

Nessuna oltre quelle dette sopra: una colonna temporale si legge dal
valore nativo, un testo con `date_format` o con i formati ISO di default.

#### Complessità

Tempo O(n) sulle righe (due passate: controllo e estrazione); memoria O(n)
per ogni parte estratta.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.date_extract", "in": ["eventi"],
 "config": {"column": "quando", "parts": ["year", "quarter", "weekday", "week"], "prefix": "q_"}}
```

Ingresso `eventi`:

| `quando: utf8` |
| --- |
| 2021-01-01 |
| 2019-12-30T23:59:59+01:00 |
| null |

Uscita `risultato`:

| `quando: utf8` | `q_year: int64` | `q_quarter: int64` | `q_weekday: int64` | `q_week: int64` |
| --- | --- | --- | --- | --- |
| 2021-01-01 | 2021 | 1 | 4 | 53 |
| 2019-12-30T23:59:59+01:00 | 2019 | 4 | 0 | 1 |
| null | null | null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.date_format`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `date_format` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 3, analisi 4, kernel 6 |

#### Che cosa fa

Legge date e ore scritte come testo con un formato e le riscrive con un
altro formato in una colonna nuova (per esempio da `31/01/2024` a
`2024-01-31`). I formati sono quelli di `strftime` della libreria `chrono`.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo | colonna da leggere |
| `input_format` | stringa | assente | formato `chrono` non vuoto, al più `max_string_bytes` byte; obbligatorio per un testo, rifiutato per una colonna temporale; `null` non ammesso | formato di lettura di un testo |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte, senza fuso, che scrive al più `max_string_bytes` byte per valore | formato di scrittura |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non leggibile rifiuta sempre la riga, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` in secondi, millisecondi,
microsecondi o nanosecondi, con o senza fuso) si legge dal valore nativo,
senza `input_format` (scritto, si rifiuta): vale l'ora locale della
colonna (del suo fuso; senza fuso, il valore com'è), e una data è la sua
mezzanotte ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)). Ogni altra colonna si legge come testo, con
`input_format` obbligatorio; leggibili come testo: `utf8`, `int64`,
`uint64`, `float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

La lettura deve consumare tutto il testo della cella. Un formato senza
campi orari legge una data e la pone a mezzanotte. `output_format` non può
contenere campi di fuso o di offset (`%z`, `%:z`, `%Z`, `%+`): il valore
letto non ha fuso. Un valore non leggibile rifiuta sempre la riga.

Il testo scritto da `output_format` non può superare `max_string_bytes`
byte per valore. Il limite si controlla in validazione, esatto per campo:
il testo letterale conta per la sua lunghezza, ogni campo `strftime` per
la sua larghezza massima (anno 7 byte col segno; secolo `%C` 2, perché si
scrive solo per gli anni 0..=9999; mese, giorno, ora 2; nome del mese o del
giorno 9; offset `%z` 5, `+0530`, `%:z` 6, `+05:30`, `%::z` 9, `%:::z` 3;
frazioni `%3f`, `%6f`, `%9f` 3, 6, 9 e `%.3f`, `%.6f`, `%.9f` 4, 7, 10;
nome del fuso 32); `%Y%m` scrive al più 9 byte.

#### Schema

La colonna d'uscita è `utf8` nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1. Una cella nulla dà null.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non leggibile come testo;
- `input_format` assente con una colonna di testo, o scritto con una
  colonna temporale;
- un formato vuoto, oltre `max_string_bytes`, con un campo non riconosciuto
  (`%Q`, `%` finale), o `output_format` con campi di fuso;
- `output_format` che può scrivere più di `max_string_bytes` byte per
  valore;
- `output_column` non valido;
- `invalid` scritto, con qualunque valore, anche `null`;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: ogni cella non nulla che non si
  legge con `input_format` (`conversion.invalid_datetime`); il passo non
  produce uscita;
- `DataMapping`, senza diagnostica per riga: un valore che `output_format`
  non sa scrivere (anno fuori da 0..=9999 con `%C`);
- `Schema`: una cella che non si converte in testo.

#### Limiti e deviazioni

Non esiste un modo di trasformare un valore non leggibile in null: per
questo `invalid` si rifiuta.

#### Complessità

Tempo O(n) (due letture per riga: il controllo, poi la conversione con il
formato compilato una volta); memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.date_format", "in": ["fatture"],
 "config": {"column": "data", "input_format": "%d/%m/%Y", "output_format": "%Y-%m-%d", "output_column": "data_iso"}}
```

Ingresso `fatture`:

| `data: utf8` |
| --- |
| 31/01/2024 |
| 29/02/2024 |
| null |

Uscita `risultato`:

| `data: utf8` | `data_iso: utf8` |
| --- | --- |
| 31/01/2024 | 2024-01-31 |
| 29/02/2024 | 2024-02-29 |
| null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.dedup_advanced`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `dedup_advanced` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 2, analisi 2, kernel 3 |

#### Che cosa fa

Come [`table.distinct`](#tabledistinct), ma prima ordina le righe per
`order_column`: «prima» e «ultima» occorrenza si riferiscono a
quell'ordine. Serve, per esempio, a tenere per ogni cliente la riga più
recente.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `subset` | lista di stringhe | obbligatorio | nomi di colonne leggibili come testo, almeno uno, senza ripetizioni | colonne della chiave |
| `keep` | stringa | `"first"` | `"first"`, `"last"` | tiene la prima o l'ultima occorrenza di ogni chiave |
| `order_column` | stringa | nessuno | nome di una colonna di tipo ordinabile | ordinamento stabile prima della deduplica |
| `ascending` | booleano | `true` | `true`, `false`; solo con `order_column`; `null` non ammesso | verso dell'ordinamento |

Colonne leggibili come testo e uguaglianza delle chiavi come in
[`table.distinct`](#tabledistinct); tipi ordinabili e confronto come in
[`table.sort`](#tablesort). `keep: "false"` si rifiuta, e `ascending`
scritto senza `order_column` si rifiuta invece di essere ignorato.

#### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati. Con
`order_column` il contratto dichiara l'uscita ordinata su quella colonna
nel verso chiesto; senza, conserva l'ordinamento dichiarato dell'ingresso.
Il conteggio delle righe non è più noto.

#### Righe

Filtro: una riga per chiave distinta.

#### Ordine

Con `order_column`, le righe tenute sono nell'ordine di
[`table.sort`](#tablesort) su quella colonna: stabile, null in coda in
ascendente e in testa in discendente. Senza, nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `keep: "false"`;
- `subset` vuoto, con un nome ripetuto o non valido, o oltre il limite di
  colonne; una sua colonna assente o non leggibile come testo;
- `ascending` senza `order_column`; `ascending` o `order_column` `null`
  espliciti (il parametro si omette);
- `order_column` assente o di tipo non ordinabile;
- campi sconosciuti.

In esecuzione:

- `Schema`: una cella della chiave che non si converte in testo (date
  fuori intervallo), una chiave di dizionario fuori dal proprio dizionario;
- `ResourceLimit`: più di `u32::MAX` righe.

#### Limiti e deviazioni

La mappa delle chiavi non è contabilizzata
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Con `order_column`, tempo O(n log n) per l'ordinamento più O(n) per la
deduplica; senza, O(n). Memoria O(n) per la copia ordinata e O(k) per le k
chiavi distinte.

#### Memoria

Memoria: da misura v4.

#### Esempio

La riga più recente di ogni cliente.

Passo del piano:

```json
{"out": "risultato", "op": "table.dedup_advanced", "in": ["ordini"],
 "config": {"subset": ["cliente"], "order_column": "data", "ascending": false}}
```

Ingresso `ordini`:

| `cliente: utf8` | `data: date32` | `importo: float64` |
| --- | --- | --- |
| a | 2024-01-10 | 10.0 |
| b | 2024-02-01 | 20.0 |
| a | 2024-03-05 | 30.0 |

Uscita `risultato`:

| `cliente: utf8` | `data: date32` | `importo: float64` |
| --- | --- | --- |
| a | 2024-03-05 | 30.0 |
| b | 2024-02-01 | 20.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.distinct`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `distinct` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Toglie le righe ripetute: due righe sono uguali se hanno gli stessi valori
nelle colonne di `subset` (tutte le colonne se `subset` è vuoto). Di ogni
gruppo di righe uguali tiene la prima, l'ultima, oppure nessuna.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `subset` | lista di stringhe | `[]` | nomi di colonne leggibili come testo, senza ripetizioni | colonne della chiave; vuoto vale tutte le colonne |
| `keep` | stringa | `"first"` | `"first"`, `"last"`, `"false"` | tiene la prima occorrenza, l'ultima, o solo le righe la cui chiave compare una volta |

Colonne leggibili come testo: `utf8`, `int64`, `uint64`, `float64`,
`bool`, `date32`, `date64`, `timestamp` di ogni unità (secondi, millisecondi,
microsecondi, nanosecondi; con timezone valida), `decimal128` con scala da
0 a 38, `binary`, `dictionary<utf8>` (chiavi `int32`). Un `timestamp` si
scrive in RFC 3339 con tutte le cifre frazionarie che servono: due istanti
distinti, anche di un solo nanosecondo, hanno testi distinti, e lo stesso
istante ha lo stesso testo in ogni unità; un istante il cui offset nel
fuso della colonna ha i secondi (ora media locale) non ha forma RFC 3339,
e dove serve il testo la cella si rifiuta. Un `date64` si scrive
`AAAA-MM-GG` se è allineato al giorno; altrimenti non è una data, e la
cella si rifiuta.

Uguaglianza delle chiavi, colonna per colonna: sul valore nella sua forma
in testo (un `timestamp` sul suo istante, non sul testo), quindi `-0.0` e
`0.0` sono diversi e ogni NaN è uguale a ogni
altro NaN; `binary` sui byte; un null è uguale a un null e diverso da ogni
valore, anche dal testo vuoto; la voce nulla di un dizionario è un null.

#### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati. Delle
proprietà del contratto resta solo l'ordinamento dichiarato: il conteggio
delle righe non è più noto.

#### Righe

Filtro: una riga per chiave distinta con `"first"` e `"last"`; con
`"false"` solo le righe la cui chiave compare una volta sola.

#### Ordine

Le righe tenute restano nell'ordine d'ingresso, per ogni valore di `keep`.

#### Errori

In validazione, `InvalidPlan`:

- `subset` con un nome ripetuto o non valido, o oltre il limite di colonne;
- una colonna di `subset` assente o non leggibile come testo; con `subset`
  vuoto, una colonna qualsiasi dell'ingresso non leggibile come testo;
- `keep` fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella che non si converte in testo (`date32`, `date64` o
  `timestamp` fuori dall'intervallo delle date, `date64` non allineato al
  giorno);
- `ResourceLimit`: più di `u32::MAX` righe.

#### Limiti e deviazioni

La mappa delle chiavi non è contabilizzata
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n) sulle righe (una passata con una mappa delle chiavi), più
l'ordinamento degli indici tenuti; memoria O(k) per le k chiavi distinte e
O(k) indici.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.distinct", "in": ["ordini"],
 "config": {"subset": ["cliente"], "keep": "last"}}
```

Ingresso `ordini`:

| `id: int64` | `cliente: utf8` |
| --- | --- |
| 1 | a |
| 2 | b |
| 3 | a |
| 4 | null |

Uscita `risultato`:

| `id: int64` | `cliente: utf8` |
| --- | --- |
| 2 | b |
| 3 | a |
| 4 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.drop_columns`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `drop_columns` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Toglie dalla tabella le colonne elencate in `columns` e lascia le altre
come sono, nello stesso ordine. Le colonne che restano non si copiano:
l'uscita condivide gli array dell'ingresso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | da 1 a 4096 nomi, non vuoti (non di soli spazi), al più 1024 byte ciascuno, senza ripetizioni | colonne da togliere |

Una lista vuota si rifiuta. Un nome che non è una colonna dell'ingresso
si accetta e non toglie niente: dipende dall'ingresso, e lo stesso piano
gira su tabelle diverse.

#### Schema

Le colonne non elencate restano nell'ordine d'ingresso, con tipo,
nullabilità e metadati di campo; i metadati di schema restano. Se le
colonne tolte sono tutte, l'uscita ha zero colonne e lo stesso numero di
righe dell'ingresso.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato
(`sorted_by`) cade se almeno una colonna è stata tolta davvero, anche se
non era una chiave. Se si
toglie la colonna geometrica il contratto diventa tabellare.

#### Righe

1:1: stesse righe, stessi valori.

#### Ordine

Righe nell'ordine d'ingresso; colonne rimaste nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `columns` assente o vuota, con un nome ripetuto, vuoto, di soli spazi o
  oltre 1024 byte, o con più di 4096 nomi;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(c) sulle colonne dello schema, indipendente dalle righe; nessuna
memoria per i dati (array condivisi).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.drop_columns", "in": ["ordini"],
 "config": {"columns": ["note", "assente"]}}
```

Ingresso `ordini`:

| `id: int64` | `note: utf8` | `importo: float64` |
| --- | --- | --- |
| 1 | urgente | 5.5 |
| 2 | null | 12.0 |

Uscita `risultato`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 1 | 5.5 |
| 2 | 12.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.except`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `except` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Differenza insiemistica di due tabelle con lo stesso schema (`EXCEPT` di
SQL): le righe distinte di sinistra che non compaiono a destra, ognuna una
volta. Due righe sono uguali se lo sono tutte le loro colonne.

#### Parametri

Nessuno: la config è `{}`.

Ogni colonna ha un tipo fra `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64`, `timestamp` di ogni unità, `decimal128`, `binary` e
`dictionary<utf8>`
(chiave int32).

#### Schema

Identico alla sinistra: stesse colonne, tipi, nullabilità, metadati e
colonna geometrica. Nessuna proprietà del contratto sopravvive, nemmeno
l'ordinamento dichiarato, benché l'ordine sia quello della sinistra.

#### Righe

Una riga per ogni riga distinta di sinistra che non compare a destra, presa
dalla sua prima comparsa a sinistra: anche i duplicati interni alla
sinistra si riducono a una riga. L'uguaglianza è quella di
[`table.union_distinct`](#tableunion_distinct): per valore, colonna per
colonna, null uguale a null, NaN uguali fra loro, `-0.0` diverso da `0.0`.

#### Ordine

Quello della sinistra.

#### Errori

In validazione, `InvalidPlan`:

- numero di colonne diverso, o nome o tipo diversi in una posizione (la
  nullabilità non conta);
- una colonna di tipo fuori dall'elenco sopra;
- config non vuota.

In esecuzione, `ResourceLimit`: più di `u32::MAX` righe tenute.

#### Limiti e deviazioni

Le chiavi non si contano su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata)),
e gli insiemi usano un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n + m) atteso; memoria O(byte delle chiavi distinte dei due lati)
più la copia delle righe tenute.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.except", "in": ["a", "b"],
 "config": {}}
```

Ingresso `a`:

| `id: int64` |
| --- |
| 3 |
| 1 |
| 2 |
| 3 |
| null |

Ingresso `b`:

| `id: int64` |
| --- |
| 1 |
| null |
| 4 |

Uscita `risultato`:

| `id: int64` |
| --- |
| 3 |
| 2 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.explode`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `explode` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 1, kernel 2 |

#### Che cosa fa

Espande una colonna lista in una riga per elemento: ogni riga d'ingresso
si ripete una volta per ogni elemento della sua lista, e la colonna
d'uscita contiene l'elemento. Una lista vuota o nulla dà una riga con
elemento nullo, quindi nessuna riga sparisce.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna `list<…>` | colonna da espandere |
| `output_column` | stringa | `column` | nome di colonna valido | colonna con gli elementi |
| `empty_policy` | stringa | `"null"` | `"null"` | liste vuote e nulle danno una riga con null |

`empty_policy: "drop"` si rifiuta: togliere le righe con lista vuota è una
selezione, e si scrive come passo a parte. Solo `list<…>`: non
`large_list` né liste a dimensione fissa.

#### Schema

Con `output_column` uguale a `column` (il default), la colonna lista è
sostituita al suo posto dalla colonna degli elementi. Con un nome diverso
la colonna lista resta e la colonna degli elementi va in coda, o sostituisce
al suo posto una colonna esistente con quel nome. La colonna degli elementi
ha il tipo degli elementi, è nullabile e non ha metadati di campo; le altre
colonne e i metadati di schema si conservano. Il contratto non dichiara né
ordinamento né conteggio.

#### Righe

Espansione 1:N: per ogni riga, una riga per elemento (un elemento nullo dà
un valore nullo), una sola se la lista è vuota o nulla.

#### Ordine

Righe nell'ordine d'ingresso, elementi nell'ordine della lista.

#### Errori

In validazione, `InvalidPlan`:

- `empty_policy: "drop"`;
- `column` assente o non di tipo `list<…>`;
- `output_column` non valido o `null` esplicito (il parametro si omette);
  campi sconosciuti.

In esecuzione, `ResourceLimit`: righe d'uscita oltre `max_rows`; un
elemento o una riga d'uscita oltre l'indice `u32::MAX`.

#### Limiti e deviazioni

Il runner controlla sui dati righe per arco e fattore di espansione
([README, «Esecuzione»](../README.md#esecuzione)).

#### Complessità

Tempo e memoria O(n + e) per n righe ed e elementi. Quando la colonna
d'uscita sostituisce la lista, la lista non si copia per ogni riga
d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.explode", "in": ["articoli"],
 "config": {"column": "tag"}}
```

Ingresso `articoli`:

| `id: int64` | `tag: list<utf8>` |
| --- | --- |
| 1 | ["a", "b"] |
| 2 | [] |
| 3 | null |

Uscita `risultato`:

| `id: int64` | `tag: utf8` |
| --- | --- |
| 1 | a |
| 1 | b |
| 2 | null |
| 3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.expression`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `expression` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 6, config 3, analisi 5, kernel 7 |

#### Che cosa fa

Calcola una colonna nuova da un'espressione scritta come albero JSON:
colonne, letterali, operatori aritmetici, di confronto e logici, funzioni
e `case`. Il tipo del risultato (numero, booleano, testo, data o istante) si
decide dallo schema prima di leggere i dati. I confronti fra numeri sono
esatti sul valore d'origine; un numero non finito, in ingresso o nel
risultato, rifiuta la riga.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | obbligatorio | nome valido (non vuoto, al più 1024 byte) | colonna d'uscita |
| `expression` | oggetto | obbligatorio | nodo della grammatica sotto; profondità al più 64, al più 4096 nodi | espressione da calcolare |
| `output_type` | stringa | `auto` | `auto`, `number`, `boolean`, `text`, `date32`, `timestamp_ms` | tipo della colonna d'uscita; `auto` lo deduce |
| `on_division_by_zero` | stringa | `"null"` | `"null"`, `"error"`; solo in un'espressione con almeno una divisione; `null` non ammesso | che cosa dà una divisione con divisore zero (sotto) |

Nodi (campo `kind`):

```text
{"kind": "column",   "name": "<colonna>"}
{"kind": "literal",  "value": null | booleano | numero finito | stringa}
{"kind": "unary",    "op": "not" | "negate" | "is_null" | "is_not_null", "value": nodo}
{"kind": "binary",   "op": <operatore>, "left": nodo, "right": nodo}
{"kind": "function", "name": <funzione>, "args": [nodo, …]}          (al più 64 argomenti)
{"kind": "case",     "branches": [{"when": nodo, "then": nodo}, …],  (da 1 a 64 rami)
                     "else_value": nodo}
```

Un campo non previsto dentro un nodo si rifiuta. Un letterale di testo non
supera `max_string_bytes` byte, anche dentro la lista di `in`.

Tipi delle colonne: `bool` è booleano; `int64`, `uint64`, `float64`,
`decimal128`, `date32` (giorni dall'epoca), `date64` (millisecondi
dall'epoca) e `timestamp` di ogni unità, con o senza fuso (il valore
nell'unità della colonna) sono numeri; `utf8`, `binary` e `dictionary<utf8>` sono testo.
Ogni altro tipo si rifiuta.

Operatori (`binary`), con null che propaga salvo dove detto:

- `add`, `subtract`, `multiply`, `divide`: numero, numero → numero, in
  `f64`; per la divisione per zero vedi `on_division_by_zero`;
- `equal`, `not_equal`, `greater`, `greater_equal`, `less`, `less_equal`:
  due operandi dello stesso tipo → booleano; i numeri si confrontano sul
  valore esatto (`int64` oltre `2^53`, `decimal128`, il letterale `0.1` come
  decimale), i testi per byte, `false < true`;
- `and`, `or`: booleani, logica a tre valori (`false and null` è `false`,
  `true or null` è `true`).

Unari: `not` (booleano), `negate` (numero, esatto), `is_null` e
`is_not_null` (qualsiasi, mai null).

Funzioni (argomenti → risultato):

| funzione | argomenti | risultato | note |
| --- | --- | --- | --- |
| `coalesce` | 1..N qualsiasi | tipo degli argomenti | primo non nullo |
| `null_if` | 2 dello stesso tipo | tipo del primo | null se uguali |
| `lower`, `upper`, `trim` | 1 testo | testo | Unicode |
| `length` | 1 testo | numero | caratteri Unicode |
| `year` | 1 testo | numero | anno dei primi 10 byte letti come `AAAA-MM-GG` |
| `concat` | 1..N testo | testo | null se un argomento è null |
| `contains`, `starts_with`, `ends_with` | 2 testo | booleano | con distinzione di maiuscole |
| `abs`, `round`, `floor`, `ceil` | 1 numero | numero | `round`: metà lontano da zero; `abs` esatto |
| `power` | 2 numero | numero | base elevata all'esponente |
| `substring` | testo, numero, numero? | testo | inizio da 0 e lunghezza in caratteri, troncati verso zero; senza lunghezza fino alla fine |
| `regex_replace` | 3 testo | testo | sintassi della crate `regex`, tutte le occorrenze, `$1`/`$nome` nella sostituzione |
| `between` | 3 dello stesso tipo | booleano | estremi compresi; null se un argomento è null |
| `in` | valore, lista letterale | booleano | la lista è `{"kind": "literal", "value": [ … ]}` di scalari; lista vuota → `false` |
| `greatest`, `least` | 1..N dello stesso tipo | tipo degli argomenti | null se un argomento è null |
| `date_trunc` | unità letterale, colonna | `date32` o `timestamp(ms)` | unità `year`, `month`, `day` (e `hour`, `minute`, `second` solo per i `timestamp`); il secondo argomento è una colonna `date32` o `timestamp` di ogni unità senza fuso, un altro `date_trunc` o `null`. Un `timestamp` esce in millisecondi: il troncamento è almeno al secondo, quindi esatto; un valore in secondi oltre la gamma dei millisecondi è un errore |

`case` valuta i rami in ordine e rende il `then` del primo `when` vero; un
`when` null vale falso; nessun ramo vero rende `else_value`. I rami non
scelti non si valutano.

Divisione con operandi non nulli e divisore zero:

- `on_division_by_zero = "null"` (default): il nodo della divisione vale
  null, e il null segue poi le regole di sopra (`coalesce(a / b, 0)` dà 0;
  in un ramo di `case` non scelto la divisione non si valuta). Ogni riga in
  cui è successo si conta una volta nel campo `righe_divisione_per_zero`
  del resoconto del passo nel runner: un conteggio, mai valori;
- `on_division_by_zero = "error"`: la riga si rifiuta con diagnostica
  `evaluation.division_by_zero` e il passo fallisce.

Un divisore letterale zero (`x / 0`) si rifiuta in validazione con
qualunque politica.

Un pattern letterale di `regex_replace` non supera `max_regex_bytes` byte
e, se non è una regex valida, si rifiuta in validazione. Un pattern
calcolato dalle colonne che non è una regex valida rifiuta la riga
(`evaluation.invalid_regex`), senza il testo d'errore della crate `regex`,
che riporterebbe il pattern cioè un dato di cella.
Ogni testo prodotto da una funzione (`concat`, `lower`, `upper`,
`regex_replace`, `substring`, …) non supera `max_string_bytes` byte.

Con `auto` il tipo d'uscita è l'unico tipo che l'espressione può produrre
(`case` e `coalesce` uniscono i tipi dei rami); solo null dà `text`. Con un
tipo dichiarato, l'espressione deve poterlo produrre: non si converte, e una
riga che produce un altro tipo fallisce.

#### Schema

La colonna d'uscita è `float64` (numero), `bool`, `utf8`, `date32` o
`timestamp(ms)` senza fuso, nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- nodo non riconosciuto o con un campo sconosciuto, profondità oltre 64,
  più di 4096 nodi, più di 64 argomenti o rami, `case` senza rami, nome di
  colonna vuoto;
- una colonna assente o di tipo non ammesso; colonne temporali di unità
  diverse lette come numero nella stessa espressione (anche `date32` con un
  `timestamp`: il numero è nell'unità della colonna);
- letterale non scalare o non finito; letterale di testo oltre
  `max_string_bytes` byte, anche nella lista di `in`; `in` senza lista
  letterale di scalari;
- numero di argomenti o tipo di un operando non ammessi; confronto fra tipi
  diversi (anche solo possibili, come `coalesce` di testo e numero);
- unità di `date_trunc` non letterale o fuori elenco, unità oraria su
  `date32`, `date_trunc` su testo o su `timestamp` con fuso;
- divisione per il numero zero scritto nell'espressione, con qualunque
  `on_division_by_zero`;
- `on_division_by_zero` scritto in un'espressione senza divisioni, con un
  valore fuori elenco o `null` esplicito (il parametro si omette);
- pattern letterale di `regex_replace` non valido o oltre
  `max_regex_bytes` byte, indice letterale negativo di `substring` (solo
  dove la valutazione lo guarderebbe);
- con `auto`, più tipi possibili; con un tipo dichiarato, un tipo che
  l'espressione non produce mai;
- `output_column` non valido; config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: divisione per zero, solo con
  `on_division_by_zero = "error"` (`evaluation.division_by_zero`), `NaN` o
  infinito letto da una colonna
  (`evaluation.non_finite_input`), risultato non finito di un'operazione o
  di `power` (`evaluation.non_finite_result`), pattern di `regex_replace`
  calcolato dalle colonne che non è una regex valida
  (`evaluation.invalid_regex`); il passo non produce uscita;
- `InvalidPlan`: indice di `substring` calcolato dalle colonne e negativo;
- `Schema`: `year` su un testo che non inizia con una data; una riga che
  produce un tipo diverso da `output_type`; `negate` o `abs` di un
  `decimal128` fuori dominio; una cella che non si converte in testo;
- `ResourceLimit`: `length` di un testo oltre `u32::MAX` caratteri; un
  pattern di `regex_replace` calcolato dalle colonne oltre
  `max_regex_bytes` byte; un testo prodotto da una funzione oltre
  `max_string_bytes` byte.

#### Limiti e deviazioni

L'aritmetica e l'uscita numerica sono in `f64`: gli interi oltre `2^53` e i
decimali si arrotondano nel calcolo, non nei confronti. `date_trunc` tronca
in UTC e non accetta istanti con fuso. Gli errori che dipendono dai valori
(regex e indici calcolati) arrivano in esecuzione
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).
Un divisore calcolato uguale a zero dà null per default, contato in
`righe_divisione_per_zero`; con `on_division_by_zero = "error"` rifiuta la
riga in esecuzione. Il divisore letterale zero si vede già in validazione.

#### Complessità

Tempo O(n · t) per t nodi dell'espressione (più la compilazione di una
regex calcolata per riga); memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Un `when` nullo vale falso: la terza riga prende `else_value`.

Passo del piano:

```json
{"out": "risultato", "op": "table.expression", "in": ["ordini"],
 "config": {"output_column": "fascia", "expression": {"kind": "case", "branches": [{"when": {"kind": "binary", "op": "greater_equal", "left": {"kind": "column", "name": "importo"}, "right": {"kind": "literal", "value": 100}}, "then": {"kind": "literal", "value": "alta"}}], "else_value": {"kind": "literal", "value": "bassa"}}}}
```

Ingresso `ordini`:

| `importo: float64` |
| --- |
| 150.0 |
| 20.0 |
| null |

Uscita `risultato`:

| `importo: float64` | `fascia: utf8` |
| --- | --- |
| 150.0 | alta |
| 20.0 | bassa |
| null | bassa |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.fill_na`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `fill_na` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Riempie le celle null di una colonna, o di tutte se `column` manca: con un
valore fisso (`method = "value"`), con l'ultimo valore non nullo che le
precede (`ffill`) o con il primo che le segue (`bfill`), nell'ordine delle
righe. Il tipo delle colonne non cambia.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa o `null` | `null` | colonna dell'ingresso di tipo `utf8`, `int64`, `float64` o `bool` | colonna da riempire; `null` le riempie tutte |
| `method` | stringa | `"value"` | `value`, `ffill`, `bfill` | come si riempie |
| `value` | JSON | assente | convertibile nel tipo di ogni colonna da riempire (sotto); testo al più `max_string_bytes` byte | valore di riempimento, solo con `method = "value"` |

Senza `column` ogni colonna dell'ingresso deve essere di uno dei quattro
tipi, e `value` deve convertirsi nel tipo di ognuna.

`value` si converte così:

- `utf8`: una stringa com'è; ogni altro valore JSON diventa il suo testo
  JSON (`5` dà `"5"`, `true` dà `"true"`);
- `int64`: un intero JSON nel dominio di `int64`, o una stringa che lo è
  (senza spazi); `1.5` si rifiuta;
- `float64`: un numero JSON, o una stringa con la virgola decimale ammessa
  (`"2,5"`, senza spazi);
- `bool`: `true`/`false` JSON, o le stringhe `"true"`/`"false"` in
  qualunque combinazione di maiuscole.

`value` assente o `null` con `method = "value"` è accettato e non cambia
niente. `value` scritto, anche `null`, con `ffill` o `bfill` si rifiuta.

#### Schema

Stesse colonne, stessi tipi, stesse posizioni e stessi metadati di campo e
di schema; le colonne riempite diventano nullable (possono restare null
all'inizio con `ffill` e alla fine con `bfill`).

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato cade.

#### Righe

1:1. Si riempie solo il null: un `NaN` in un `float64` non è null e
resta. Con `ffill` i null prima del primo valore restano null; con `bfill`
quelli dopo l'ultimo.

#### Ordine

Righe nell'ordine d'ingresso, che è anche l'ordine in cui `ffill` e `bfill`
cercano il valore.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente dall'ingresso;
- una colonna da riempire di tipo diverso da `utf8`, `int64`, `float64`,
  `bool` (senza `column`: una qualunque colonna dell'ingresso);
- `value` non convertibile nel tipo di una colonna da riempire;
- il testo di `value` oltre `max_string_bytes` byte;
- `value` scritto con `ffill` o `bfill`;
- `method` fuori elenco, config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

Solo i quattro tipi elencati: per date, decimali o `uint64` il riempimento
non c'è. `ffill` e `bfill` seguono l'ordine delle righe, quindi hanno senso
dopo un `table.sort`.

#### Complessità

Tempo O(n) per colonna riempita; memoria pari a una copia di ciascuna
colonna che contiene null (quelle senza null restano condivise).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.fill_na", "in": ["listino"],
 "config": {"column": "prezzo", "method": "ffill"}}
```

Ingresso `listino`:

| `giorno: int64` | `prezzo: float64` |
| --- | --- |
| 1 | null |
| 2 | 10.5 |
| 3 | null |
| 4 | 11.0 |

Uscita `risultato`:

| `giorno: int64` | `prezzo: float64` |
| --- | --- |
| 1 | null |
| 2 | 10.5 |
| 3 | 10.5 |
| 4 | 11.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.filter`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `filter` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 4, kernel 5 |

#### Che cosa fa

Tiene le righe in cui la colonna `column` soddisfa la condizione
`operator value` e scarta le altre. Le colonne e i loro tipi non cambiano.
Una cella nulla non soddisfa nessun operatore tranne `isnull`; `!=` scarta
quindi anche i null.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna dell'ingresso | colonna su cui si valuta la condizione |
| `operator` | stringa | obbligatorio | `==`, `!=`, `>`, `>=`, `<`, `<=`, `contains`, `startswith`, `endswith`, `isnull`, `notnull`, `between` | confronto fra la cella e `value` |
| `value` | JSON | `null` | stringa, numero, booleano o `null`; per `between` il testo `"min,max"`; con `isnull`, `notnull` non si scrive | termine di confronto; un non-stringa vale il suo testo JSON, `null` vale `""` |

Come si confronta, per operatore:

- `==`, `!=` su `int64` e `float64`: confronto numerico esatto con `value`,
  che deve essere un numero (intero, decimale o esponenziale). Su
  `float64` `0.0 == -0.0` e `NaN == NaN`. Su ogni altro tipo si confronta il
  testo della cella con il testo di `value`, carattere per carattere
  (su `uint64` `5` e `"5"` sono uguali, `5.0` e `"05"` no; una `date32` vale
  `"2024-01-31"`, un `decimal128` di scala 2 vale `"12.30"`, un `bool`
  `"true"`/`"false"`);
- `>`, `>=`, `<`, `<=`, `between`: confronto nel dominio nativo del tipo,
  mai attraverso `f64`, su `int64`, `uint64`, `float64`, `decimal128`,
  `date32` (giorni dall'epoca), `date64` (millisecondi dall'epoca),
  `timestamp` di ogni unità (il valore nell'unità della colonna) e
  `utf8` il cui testo è un numero (spazi ai lati ignorati, virgola decimale
  ammessa). `value` deve essere numerico; `between` include entrambi gli
  estremi. Un `NaN`, nella cella o nell'estremo, rende falso il confronto;
- `contains` (senza distinzione fra maiuscole e minuscole), `startswith`,
  `endswith` (con distinzione): sul testo della cella;
- `isnull`, `notnull`: sulla nullità logica della cella (anche la voce
  nulla di un dizionario). `value` non avrebbe effetto: scritto, anche
  `null`, si rifiuta.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati. Delle
proprietà del contratto resta solo l'ordinamento dichiarato (`sorted_by`):
il conteggio delle righe non è più noto.

#### Righe

Filtro: da 0 a tutte le righe dell'ingresso, ciascuna al più una volta.

#### Ordine

Le righe tenute restano nell'ordine d'ingresso.

#### Errori

In validazione (analisi del contratto), `InvalidPlan`:

- `column` non è una colonna dell'ingresso;
- `==`/`!=` su `int64` o `float64` con `value` non numerico;
- `>`, `>=`, `<`, `<=`, `between` su un tipo fuori dall'elenco sopra, o con
  `value` non numerico; `between` senza la forma `min,max` o con un estremo
  non numerico;
- `contains`, `startswith`, `endswith` (e `==`/`!=` fuori da `int64` e
  `float64`) su una colonna che non si legge come testo scalare;
- `value` scritto (anche `null`) con `isnull` o `notnull`;
- config con campi sconosciuti o `operator` fuori elenco.

In esecuzione:

- `Schema`: una cella `utf8` che non è un numero sotto un operatore
  ordinato o `between` (dipende dai dati, quindi l'analisi non lo vede);
- `Schema`: una cella che sotto un operatore testuale non si legge come
  testo (un `binary` non UTF-8, una data fuori dall'intervallo di chrono);
- `ResourceLimit`: una riga tenuta con indice oltre `u32::MAX` (ingressi
  di più di 2^32 righe).

#### Limiti e deviazioni

Il confronto fra numeri è esatto per costruzione, anche oltre `2^53` e fra
interi e decimali ([README, «Validazione»](../README.md#validazione)). Gli
operatori ordinati confrontano le date come numero di giorni dall'epoca e i
timestamp come numero di unità della colonna (un `timestamp(us)` in
microsecondi): `value` è un intero, non una data in testo;
`==` e `!=` invece confrontano il testo della data.

#### Complessità

Tempo O(n) sulle righe dell'ingresso; memoria O(k) per le righe tenute
(copia delle colonne selezionate), più O(k) indici.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.filter", "in": ["ordini"],
 "config": {"column": "importo", "operator": ">", "value": 10}}
```

Ingresso `ordini`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 1 | 5.5 |
| 2 | 12.0 |
| 3 | null |
| 4 | 40.25 |

Uscita `risultato`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 2 | 12.0 |
| 4 | 40.25 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.flatten_json`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `flatten_json` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 2, kernel 5 |

#### Che cosa fa

Legge ogni cella di una colonna come documento JSON e ne estrae i valori
nominati in `output_columns`, uno per colonna d'uscita. Il nome di una
colonna d'uscita è `prefix` seguito dal percorso della chiave nel documento,
con i livelli separati da un punto (`doc_indirizzo.citta` legge
`{"indirizzo": {"citta": …}}`). Ogni valore diventa testo.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna leggibile come testo | colonna con i documenti JSON |
| `prefix` | stringa | `""`, cioè `<column>_` | qualsiasi | prefisso dei nomi d'uscita; vuoto vale `<column>_` |
| `max_level` | intero | `1` | da 0 a 5 | livelli di annidamento attraversati |
| `output_columns` | lista di stringhe | `[]` | non vuota nei piani; nomi che iniziano con `prefix`, senza ripetizioni, al più `max_columns` | colonne da estrarre |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Un percorso con k punti si estrae solo se `k <= max_level`: con il default
`1` si leggono le chiavi della radice e quelle di un oggetto figlio, non
oltre. Il testo estratto è la stringa stessa per una stringa JSON, il testo
JSON per numeri e booleani, il testo JSON compatto per un array
(`[1,2]`), la stringa vuota per un `null` JSON. Un oggetto non si estrae mai
come valore: si attraversa. Un percorso assente nel documento dà una cella
nulla. Una chiave ripetuta nello stesso oggetto vale l'ultima occorrenza. Una
chiave che contiene un punto si confonde con un percorso annidato: se due
derivazioni danno lo stesso percorso vale quella che viene dopo
nell'ordine lessicografico delle chiavi.

#### Schema

Le colonne di `output_columns`, nell'ordine scritto, `utf8` nullable: una
colonna con lo stesso nome di una esistente la sostituisce al suo posto
(perdendone tipo e metadati di campo), le altre si aggiungono in coda. La
colonna sorgente resta. Metadati di schema conservati; `row_count` resta;
`sorted_by` resta solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1. Una cella nulla dà null in tutte le colonne estratte.

#### Ordine

Invariato.

#### Errori

In validazione:

- `InvalidPlan`: `column` assente o non leggibile come testo; `max_level`
  oltre 5; un nome di `output_columns` che non inizia con `prefix`, vuoto,
  oltre 1024 byte o ripetuto; più di `max_columns` nomi; config con campi
  sconosciuti;
- `Unsupported`: `output_columns` vuoto (i nomi delle colonne dipenderebbero
  dai documenti, e lo schema non si inferisce prima dei dati);
- `ResourceLimit`: colonne dell'ingresso più `output_columns` oltre
  `max_columns` (anche quando una colonna prodotta ne sostituisce una
  esistente).

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non è JSON
  valido (`json.invalid_syntax`) o la cui radice non è un oggetto
  (`json.root_not_object`). Il passo non produce uscita; la diagnostica
  conta tutte le righe rifiutate e ne riporta le prime 10;
- `Schema`: una cella che non si converte in testo (`binary` non UTF-8);
- `ResourceLimit`: un testo emesso (valori annidati e numeri riscritti come
  testo JSON) oltre `max_string_bytes` byte.

#### Limiti e deviazioni

Un documento si valida come lo valida `serde_json`: numeri oltre il
dominio di `f64`, surrogati isolati e annidamento oltre 128 livelli lo
rendono invalido, anche dove il valore non verrebbe estratto. Il kernel
chiamato fuori dal runner accetta `output_columns` vuoto e crea una colonna
per ogni percorso trovato; il runner lo rifiuta in validazione
([README, «Validazione»](../README.md#validazione)).

#### Complessità

Tempo O(byte dei documenti): una passata di parsing per riga, senza
costruire l'albero JSON (salvo le righe con chiavi ambigue o ripetute, che
passano dall'albero); memoria O(n · k) per le k colonne estratte.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.flatten_json", "in": ["anagrafe"],
 "config": {"column": "doc", "output_columns": ["doc_nome", "doc_indirizzo.citta", "doc_tag"]}}
```

Ingresso `anagrafe`:

| `doc: utf8` |
| --- |
| {"nome": "Anna", "indirizzo": {"citta": "Roma"}, "tag": [1, 2]} |
| {"nome": null} |
| null |

Uscita `risultato`:

| `doc: utf8` | `doc_nome: utf8` | `doc_indirizzo.citta: utf8` | `doc_tag: utf8` |
| --- | --- | --- | --- |
| {"nome": "Anna", "indirizzo": {"citta": "Roma"}, "tag": [1, 2]} | Anna | Roma | [1,2] |
| {"nome": null} | "" | null | null |
| null | null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.formula`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `formula` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 2, analisi 3, kernel 5 |

#### Che cosa fa

Calcola una colonna nuova da una formula aritmetica scritta come testo, per
esempio `prezzo * quantita + 2`: quattro operazioni, parentesi, numeri,
testi fra apici e nomi di colonna. Con un operando di testo `+` concatena.
Il tipo del risultato (numero o testo) si decide dallo schema, prima di
leggere i dati. Per condizioni, confronti e funzioni c'è
[`table.expression`](#tableexpression).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `new_column` | stringa | obbligatorio | nome valido (non vuoto, al più 1024 byte) | colonna d'uscita |
| `formula` | stringa | obbligatorio | formula della grammatica sotto, non vuota, al più `max_string_bytes` byte | espressione da calcolare |
| `on_division_by_zero` | stringa | `"null"` | `"null"`, `"error"`; solo in una formula con almeno un `/`; `null` non ammesso | che cosa dà una divisione con divisore zero (sotto) |

Grammatica (spazi ASCII ignorati fra i simboli):

```text
formula  := termine (("+" | "-") termine)*
termine  := fattore (("*" | "/") fattore)*
fattore  := "-" fattore | "(" formula ")" | numero | testo | colonna
numero   := cifre e punti, con esponente facoltativo (1, 2.5, .5, 3e-2)
testo    := '…' oppure "…", senza sequenze di escape
colonna  := [A-Za-z_][A-Za-z0-9_]*   (nome esatto della colonna)
```

Non ci sono funzioni, confronti né `+` unario. Una colonna con un nome fuori
da questa forma (spazi, accenti, trattini) non si può nominare.

Tipi:

- `int64` e `float64` sono numeri; ogni altro tipo, anche `int32`,
  `uint64` e `decimal128`, è testo, letto con la sua resa testuale e deve
  essere leggibile come testo (`utf8`, `int64`, `uint64`, `float64`,
  `bool`, `date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38,
  `binary`, `dictionary<utf8>` con chiavi `int32`);
- numero op numero dà un numero (`f64`); `+` con almeno un operando di testo
  concatena i due testi (un numero con la resa più corta di `f64`: `2.0`
  diventa `2`); `-`, `*`, `/` e il `-` unario su un testo si rifiutano;
- un operando nullo rende nullo il risultato, anche nella concatenazione;
- un testo concatenato non supera `max_string_bytes` byte.

Divisione con operandi non nulli e divisore calcolato zero: con
`on_division_by_zero = "null"` (default) la divisione vale null e, poiché
ogni operatore propaga il null, tutta la riga dà null; ogni riga in cui è
successo si conta una volta nel campo `righe_divisione_per_zero` del
resoconto del passo nel runner (un conteggio, mai valori). Con `"error"` la
riga si rifiuta. Un divisore letterale zero si rifiuta in validazione con
qualunque politica.

Un'operazione con entrambi gli operandi finiti il cui risultato non è
finito (overflow di `f64`, anche intermedio: `a / (a * a)` con `a = 1e308`)
rifiuta la riga, con qualunque `on_division_by_zero`: la politica riguarda
solo il divisore zero.

#### Schema

La colonna d'uscita è `float64` se la formula è numerica, `utf8` se
contiene una concatenazione, nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `formula` vuota, oltre `max_string_bytes` o non conforme alla grammatica
  (parentesi non bilanciate, testo non chiuso o con `\`, numero o esponente
  non validi, carattere non ammesso, simboli in coda);
- un letterale numerico non finito (`1e999`, `-1e400`);
- una divisione per il numero zero scritto nella formula (`x / 0`,
  `x / -0.0`), con qualunque `on_division_by_zero`;
- `on_division_by_zero` scritto in una formula senza `/`, con un valore
  fuori elenco o `null` esplicito (il parametro si omette);
- una colonna assente o non leggibile come testo;
- `-`, `*`, `/` o il `-` unario applicati a un testo;
- `new_column` non valido; config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga, solo con
  `on_division_by_zero = "error"`: una divisione per un divisore calcolato
  che vale zero (`evaluation.division_by_zero`); il passo non produce
  uscita;
- `DataMapping` con diagnostica per riga, con qualunque
  `on_division_by_zero`: un'operazione su operandi finiti con risultato non
  finito (`evaluation.non_finite_result`); il passo non produce uscita;
- `ResourceLimit`: un testo concatenato oltre `max_string_bytes` byte;
- `Schema`: una cella che non si converte in testo.

#### Limiti e deviazioni

Il calcolo è in `f64`: un `int64` oltre `2^53` si arrotonda. Un `NaN` o
un infinito già presenti in una colonna si propagano senza errore (a
differenza di [`table.expression`](#tableexpression), che rifiuta la
riga); solo un risultato non finito da operandi finiti si rifiuta.

#### Complessità

Tempo O(n · t) per t simboli della formula; memoria O(n) per la colonna
d'uscita e O(t) per la pila di valutazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.formula", "in": ["righe"],
 "config": {"new_column": "totale", "formula": "prezzo * quantita + 2"}}
```

Ingresso `righe`:

| `prezzo: float64` | `quantita: int64` |
| --- | --- |
| 10.5 | 2 |
| 3.0 | 4 |
| null | 1 |

Uscita `risultato`:

| `prezzo: float64` | `quantita: int64` | `totale: float64` |
| --- | --- | --- |
| 10.5 | 2 | 23.0 |
| 3.0 | 4 | 14.0 |
| null | 1 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.fuzzy_join`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Join per somiglianza fra due colonne di testo, per anagrafiche sporche:
abbina ogni riga di sinistra alle righe di destra la cui chiave ha una
somiglianza (`metric`) almeno pari a `threshold`, e aggiunge in coda la
colonna della somiglianza. Per limitare i confronti si confrontano solo le
coppie dello stesso blocco (`blocking`): stesso prefisso, stesso codice
Soundex o, senza blocking, tutte.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_key` | stringa | obbligatorio | colonna `utf8` della sinistra | testo da abbinare |
| `right_key` | stringa | obbligatorio | colonna `utf8` della destra | testo candidato |
| `metric` | stringa | obbligatorio | `jaro_winkler`, `levenshtein`, `jaccard` | misura di somiglianza, in `[0, 1]` |
| `threshold` | numero | obbligatorio | in `(0, 1]` | somiglianza minima di una coppia |
| `blocking` | stringa | obbligatorio | `prefix`, `soundex`, `none` | come si formano i blocchi di candidati |
| `blocking_param` | intero | `2` | `>= 1`; solo con `blocking: prefix` | caratteri del prefisso |
| `how` | stringa | `inner` | `inner`, `left` | `left`: tiene anche le righe sinistre senza coppie |
| `score_column` | stringa | `"score"` | nome non vuoto, al più 1024 byte | nome della colonna della somiglianza |
| `max_candidates` | intero | `50` | `>= 1` | righe massime di un blocco di destra |
| `case_sensitive` | booleano | `false` | `true`, `false` | `false`: i testi si confrontano in minuscolo (Unicode) |

`blocking_param` scritto con `blocking` `soundex` o `none` si rifiuta.

Le metriche, sui testi normalizzati (in minuscolo se non `case_sensitive`)
e sui caratteri Unicode, non sui byte:

- `jaro_winkler`: Jaro più il bonus del prefisso comune (al più 4
  caratteri, peso 0,1), senza soglia minima per il bonus;
- `levenshtein`: `1 - distanza / lunghezza della più lunga`;
- `jaccard`: parole in comune (separate da spazi, come insiemi) su parole
  totali.

Due testi vuoti (per `jaccard`, senza parole) valgono 1.

I blocchi, sul testo normalizzato: `prefix` i primi `blocking_param`
caratteri; `soundex` il codice American Soundex delle sole lettere ASCII
(un testo senza lettere ASCII ha il codice vuoto, e questi testi formano un
blocco); `none` un solo blocco con tutte le righe di destra.

#### Schema

Prima tutte le colonne di sinistra, poi tutte quelle di destra, poi la
colonna della somiglianza. `left_key` tiene il nome, le altre colonne di
sinistra prendono il suffisso `_L`, tutte quelle di destra, `right_key`
compresa, `_R`. Tipi e metadati di campo restano quelli d'origine e ogni
colonna degli ingressi è nullable. La colonna `score_column` è `float64`,
nullable solo con `how: left`. I metadati di schema dei due lati si
fondono: una chiave presente da un lato solo, o con lo stesso valore,
resta. L'uscita ammette una sola colonna geometrica. Nessuna proprietà del
contratto sopravvive.

#### Righe

Per ogni riga di sinistra, una riga per ogni riga di destra dello stesso
blocco con somiglianza `>= threshold`: tutte le coppie sopra soglia, non
solo la migliore. Le chiavi nulle non si abbinano mai. Con `how: left` una
riga di sinistra senza coppie (chiave nulla compresa) resta una volta, con
le colonne di destra e la somiglianza nulle. Le colonne d'uscita portano i
testi originali, non quelli normalizzati.

#### Ordine

Comanda la sinistra: per ogni riga di sinistra, nell'ordine d'ingresso, le
sue coppie nell'ordine delle righe di destra. L'ordine non dipende dal
parallelismo della sonda.

#### Errori

In validazione, `InvalidPlan`:

- config con campi sconosciuti, `metric`, `blocking` o `how` fuori
  elenco, parametri obbligatori assenti;
- `threshold` fuori da `(0, 1]`; `blocking_param` zero, o scritto senza
  `blocking: prefix`; `max_candidates` zero; `score_column` vuoto o oltre
  1024 byte;
- `blocking_param`, `max_candidates` o `score_column` `null` espliciti (un
  parametro facoltativo si omette);
- `left_key` o `right_key` assente o non `utf8`;
- nomi d'uscita che collidono (per esempio `score_column` uguale a
  `left_key` o a un nome con suffisso), più colonne di `max_columns`, due
  colonne geometriche nell'uscita, metadati di schema con la stessa chiave
  e valori diversi.

In esecuzione, `ResourceLimit`:

- un blocco di destra con più di `max_candidates` righe, anche se nessuna
  riga di sinistra vi cade: si rifiuta invece di troncare;
- righe d'uscita oltre `max_rows` (nel runner `max_input_rows`).

#### Limiti e deviazioni

Con `blocking: none` il blocco è tutta la destra, quindi la destra non può
avere più di `max_candidates` chiavi non nulle (50 senza config). Le
somiglianze sono `f64`; il confronto con `threshold` è sullo stesso valore
che la colonna riporta. Le mappe dei blocchi usano un hash deterministico
senza seme; il blocco rifiutato per `max_candidates` è il più grande, a
parità quello con la chiave minore
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n·b·L) nel caso peggiore, con `b` le righe del blocco (al più
`max_candidates`) e `L` il costo della metrica sulla coppia: quadratico
nella lunghezza dei testi per Jaro-Winkler, a banda per Levenshtein
(le coppie certamente sotto soglia si scartano prima del calcolo), lineare
nelle parole per Jaccard. Sonda in parallelo a blocchi di 256 righe
sinistre. Memoria O(m) per i blocchi e i testi decodificati di destra, più
l'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.fuzzy_join", "in": ["clienti", "anagrafe"],
 "config": {"left_key": "nome", "right_key": "nome", "metric": "levenshtein", "threshold": 0.7, "blocking": "prefix", "how": "left"}}
```

Ingresso `clienti`:

| `id: int64` | `nome: utf8` |
| --- | --- |
| 1 | Rossetti |
| 2 | Neri |
| 3 | null |

Ingresso `anagrafe`:

| `nome: utf8` | `codice: utf8` |
| --- | --- |
| rosetti | A1 |
| nero | A2 |
| bianchi | A3 |

Uscita `risultato`:

| `id_L: int64` | `nome: utf8` | `nome_R: utf8` | `codice_R: utf8` | `score: float64` |
| --- | --- | --- | --- | --- |
| 1 | Rossetti | rosetti | A1 | 0.875 |
| 2 | Neri | nero | A2 | 0.75 |
| 3 | null | null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.hmac_sha256`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 4 |

#### Che cosa fa

Aggiunge a ogni riga l'HMAC-SHA256 (RFC 2104), in esadecimale minuscolo,
dei valori di alcune colonne, con una chiave segreta letta da una variabile
d'ambiente. È una pseudonimizzazione: chi non ha la chiave non può
ricalcolare il valore da un dato noto. Il piano contiene solo il nome della
variabile, mai la chiave.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | da 1 a `max_columns` colonne leggibili come testo, senza ripetizioni | colonne del messaggio, nell'ordine scritto |
| `key_env` | stringa | obbligatorio | nome di variabile d'ambiente, non vuoto | variabile che contiene la chiave |
| `output_column` | stringa | `hmac` | nome valido | colonna d'uscita |
| `null_policy` | stringa | `empty` | `empty`, `null`, `skip` | come entra una cella nulla |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

La chiave sono i byte UTF-8 del valore della variabile. Il messaggio di una
riga, con `f(x)` = lunghezza di x in 8 byte big-endian seguita da x:

```text
"plenora-hmac-sha256-v1\0"
per ogni colonna, nell'ordine di columns:
  f(nome) f(tipo Arrow, es. "Utf8") 0x01 f(testo della cella)
```

Una cella nulla con `empty` vale il testo vuoto; con `null` rende nulla
l'uscita della riga; con `skip` la colonna manca dal messaggio (intestazione
compresa). Nessuna politica rifiuta la riga.

#### Schema

La colonna d'uscita è `utf8`, 64 cifre esadecimali, nullable solo con
`null_policy` `null`: si aggiunge in coda o sostituisce al suo posto una
colonna con lo stesso nome (perdendone tipo e metadati di campo). Metadati
di schema conservati; `row_count` resta; `sorted_by` resta solo se nessuna
colonna esistente è sovrascritta.

#### Righe

1:1.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `key_env` vuoto o di soli spazi;
- `columns` vuoto, con ripetizioni o con più di `max_columns` colonne;
- una colonna assente o non leggibile come testo;
- `output_column` non valido;
- config con campi sconosciuti o `null_policy` fuori elenco;
- (dal runner) la variabile `key_env` non esiste, è vuota o il suo valore
  non è UTF-8 valido: tre cause distinte, lette dalla stessa funzione che
  usa il kernel. Il messaggio non nomina la variabile e non contiene la
  chiave.

In esecuzione:

- `InvalidPlan`: la chiave non è disponibile (variabile rimossa, svuotata
  o resa non UTF-8 dopo la validazione), con le stesse tre cause. Il
  messaggio non nomina la variabile e non contiene la chiave;
- `Schema`: una cella che non si converte in testo (`binary` non UTF-8,
  data o istante fuori intervallo). Con `null` le colonne dopo la prima
  nulla di una riga non si leggono.

#### Limiti e deviazioni

La variabile d'ambiente si controlla in validazione, ma può cambiare prima
dell'esecuzione: allora l'errore arriva al passo
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner)).
Con `empty` una cella nulla e una vuota danno lo stesso valore (con `skip`
no: la colonna nulla manca dal messaggio). Nome e tipo Arrow
entrano nel messaggio: rinominare o cambiare tipo cambia il risultato.

#### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

La chiave è il valore della variabile `PLENORA_ESEMPIO_CHIAVE_HMAC`, che la
prova imposta a `chiave-di-esempio`.

Passo del piano:

```json
{"out": "risultato", "op": "table.hmac_sha256", "in": ["ordini"],
 "config": {"columns": ["cliente"], "key_env": "PLENORA_ESEMPIO_CHIAVE_HMAC", "null_policy": "null"}}
```

Ingresso `ordini`:

| `cliente: utf8` |
| --- |
| C001 |
| null |

Uscita `risultato`:

| `cliente: utf8` | `hmac: utf8` |
| --- | --- |
| C001 | 5a3b8265221dd25c118b9f3d07d05a346737a1a5417ef81b6a60f960e0c44e86 |
| null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.intersect`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `intersect` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Intersezione insiemistica di due tabelle con lo stesso schema (`INTERSECT`
di SQL): le righe distinte di sinistra che compaiono anche a destra,
ognuna una volta. Due righe sono uguali se lo sono tutte le loro colonne.

#### Parametri

Nessuno: la config è `{}`.

Ogni colonna ha un tipo fra `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64`, `timestamp` di ogni unità, `decimal128`, `binary` e
`dictionary<utf8>`
(chiave int32).

#### Schema

Identico alla sinistra: stesse colonne, tipi, nullabilità, metadati e
colonna geometrica. Nessuna proprietà del contratto sopravvive, nemmeno
l'ordinamento dichiarato, benché l'ordine sia quello della sinistra.

#### Righe

Una riga per ogni riga distinta di sinistra che compare a destra, presa
dalla sua prima comparsa a sinistra. L'uguaglianza è quella di
[`table.union_distinct`](#tableunion_distinct): per valore, colonna per
colonna, null uguale a null, NaN uguali fra loro, `-0.0` diverso da `0.0`.

#### Ordine

Quello della sinistra.

#### Errori

In validazione, `InvalidPlan`:

- numero di colonne diverso, o nome o tipo diversi in una posizione (la
  nullabilità non conta);
- una colonna di tipo fuori dall'elenco sopra;
- config non vuota.

In esecuzione, `ResourceLimit`: più di `u32::MAX` righe tenute.

#### Limiti e deviazioni

Le chiavi non si contano su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata)),
e l'insieme usa un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n + m) atteso; memoria O(byte delle chiavi distinte di destra) più
la copia delle righe tenute.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.intersect", "in": ["a", "b"],
 "config": {}}
```

Ingresso `a`:

| `id: int64` |
| --- |
| 3 |
| 1 |
| 2 |
| 1 |
| null |

Ingresso `b`:

| `id: int64` |
| --- |
| 1 |
| null |
| 4 |

Uscita `risultato`:

| `id: int64` |
| --- |
| 1 |
| null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.join`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `join` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Unisce ogni riga di sinistra alle righe di destra che hanno la stessa
chiave, fatta di una o più colonne (`left_keys[i]` con `right_keys[i]`).
`how` sceglie quali righe senza corrispondenza restano: nessuna (`inner`),
le sinistre (`left`), le destre (`right`), tutte (`outer`). Le colonne
chiave di destra non compaiono nell'uscita; le altre colonne prendono un
suffisso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | colonne della sinistra, almeno una, senza ripetizioni | colonne chiave del lato sinistro |
| `right_keys` | lista di stringhe | obbligatorio | colonne della destra, tante quante `left_keys`, senza ripetizioni | colonne chiave del lato destro, nello stesso ordine |
| `how` | stringa | `inner` | `inner`, `left`, `right`, `outer` | righe senza corrispondenza da tenere |

Le due colonne di ogni coppia hanno lo stesso tipo Arrow (timezone,
precisione e scala comprese), scelto fra `utf8`, `int64`, `uint64`,
`float64`, `bool`, `date32`, `date64`, `timestamp` di ogni unità (timezone assente o valida),
`decimal128` con scala da 0 a 38, `binary` e `dictionary<utf8>`. Con
`right` e `outer` la chiave d'uscita fonde i due lati, e i tipi ammessi
sono solo `utf8`, `int64`, `uint64`, `float64`, `bool`, `date32`, `date64`
e `timestamp` (la chiave fusa tiene unità e fuso).

#### Schema

Prima tutte le colonne di sinistra, nel loro ordine, poi quelle di destra
che non sono chiave. Le colonne chiave di sinistra tengono il nome, le
altre di sinistra prendono il suffisso `_L`, tutte quelle di destra `_R`,
anche quando il nome non collide. Tipi e metadati di campo restano quelli
d'origine; ogni colonna d'uscita è nullable. I metadati di schema dei due
lati si fondono: una chiave presente da un lato solo, o con lo stesso
valore, resta. La colonna geometrica di un lato si conserva con il suo
nome d'uscita; l'uscita ne ammette una sola. Nessuna proprietà del
contratto (`sorted_by`, `row_count`) sopravvive.

#### Righe

Per ogni riga di sinistra, una riga per ogni riga di destra con la stessa
chiave: una chiave presente `m` volte a sinistra e `n` a destra dà `m·n`
righe. Due chiavi sono uguali se lo sono tutte le loro colonne, confrontate
per valore nel tipo comune: testi e `binary` byte per byte, `float64` con
tutti i NaN uguali fra loro e `-0.0` diverso da `0.0`, `dictionary` sul
testo della voce. Una chiave con almeno una colonna nulla non si abbina
mai, nemmeno a un'altra chiave nulla.

Le righe senza corrispondenza:

- `inner`: si scartano;
- `left` e `outer`: ogni riga sinistra senza corrispondenza resta una
  volta, con le colonne di destra nulle;
- `right` e `outer`: ogni riga destra senza corrispondenza si aggiunge una
  volta, con le colonne di sinistra nulle.

Con `inner` e `left` una colonna chiave d'uscita vale la chiave sinistra.
Con `right` e `outer` vale la chiave sinistra e, dove questa è nulla,
quella destra: le righe destre senza corrispondenza portano così la
propria chiave.

#### Ordine

Comanda la sinistra: per ogni riga di sinistra, nell'ordine d'ingresso, le
sue corrispondenze nell'ordine delle righe di destra (o la riga senza
corrispondenza, al suo posto). Con `right` e `outer` le righe destre senza
corrispondenza vengono in coda, nell'ordine di destra. Anche con `right`
l'ordine segue la sinistra: non è `left` con i lati scambiati. L'ordine non
dipende dal parallelismo della sonda.

#### Errori

In validazione (analisi del contratto), `InvalidPlan`:

- config con campi sconosciuti, `how` fuori elenco, `left_keys` o
  `right_keys` assenti;
- liste di chiavi vuote, di lunghezza diversa, con nomi ripetuti o oltre
  `max_columns`; colonna assente; tipi diversi nella coppia; tipo fuori
  dall'elenco sopra;
- `how` `right` o `outer` con una chiave di tipo non fondibile;
- nomi d'uscita che collidono dopo i suffissi (a sinistra una chiave
  `a_L` e una colonna non chiave `a`), nome oltre 1024 byte, più colonne
  di `max_columns`;
- due colonne geometriche nell'uscita (una per lato);
- metadati di schema con la stessa chiave e valori diversi sui due lati.

In esecuzione:

- `ResourceLimit`: righe d'uscita oltre `max_rows` (nel runner
  `max_input_rows`), contate prima di costruirle;
- `Schema`: una cella chiave `date32`, `date64` o `timestamp` fuori
  dall'intervallo delle date rappresentabili, un `date64` non allineato al
  giorno, o un dizionario malformato.

Chiamato senza l'analisi, il kernel ripete i controlli su chiavi, tipi e
nomi con `Schema` (le colonne oltre `max_columns` con `ResourceLimit`).

#### Limiti e deviazioni

Nessuna conversione fra tipi: `int64` contro `float64`, o contro un intero
di altra larghezza, si rifiuta, e gli interi diversi da `int64`/`uint64`
non sono chiavi. Le mappe delle chiavi usano un hash deterministico senza
seme ([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).
Il kernel non confronta con `max_governed_memory_bytes` né la mappa delle
chiavi né l'uscita: il suo limite è `max_rows`; nel runner il picco lo
prevede il modello di costo ([README, «Budget di memoria»](../README.md#budget-di-memoria)).

#### Complessità

Tempo O(n + m + k) atteso, con `n` e `m` le righe dei due lati e `k` quelle
d'uscita: mappa sulle chiavi di destra, sonda delle chiavi di sinistra
(in parallelo da 65.536 righe sinistre). Memoria O(m) per la mappa, O(k)
per gli indici e le colonne d'uscita. Le chiavi `int64`, `uint64`,
`float64`, `bool` e `utf8` si confrontano sui valori nativi; gli altri tipi
passano da una chiave in byte costruita per riga.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.join", "in": ["ordini", "clienti"],
 "config": {"left_keys": ["cliente"], "right_keys": ["cliente"], "how": "outer"}}
```

Ingresso `ordini`:

| `id: int64` | `cliente: utf8` |
| --- | --- |
| 1 | a |
| 2 | b |
| 3 | null |

Ingresso `clienti`:

| `cliente: utf8` | `nome: utf8` |
| --- | --- |
| a | Anna |
| a | Alba |
| c | Carla |

Uscita `risultato`:

| `id_L: int64` | `cliente: utf8` | `nome_R: utf8` |
| --- | --- | --- |
| 1 | a | Anna |
| 1 | a | Alba |
| 2 | b | null |
| 3 | null | null |
| null | c | Carla |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.limit`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine d'arrivo degli ingressi |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Tiene al più `n` righe consecutive, a partire dalla riga `offset` (contata
da 0), e scarta le altre. Nel runner si applica alla tabella intera. Le
righe tenute non si copiano: l'uscita è una finestra sull'ingresso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `n` | intero | obbligatorio | da 0 a `max_rows` | righe da tenere al più |
| `offset` | intero | `0` | da 0 a `max_rows`; con `n = 0` solo 0 | righe da saltare in testa |

`max_rows` è il `max_input_rows` del piano. Un `offset` oltre le righe
dell'ingresso dà una tabella vuota; `n = 0` dà una tabella vuota con lo
stesso schema, e un `offset` positivo non avrebbe effetto: si rifiuta.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati.
Contratto: l'ordinamento dichiarato resta, il conteggio delle righe non è
più noto.

#### Righe

Filtro: le righe da `offset` a `offset + n - 1`, quelle che esistono.

#### Ordine

Le righe tenute restano nell'ordine d'ingresso: `limit` dopo un
`table.sort` dà le prime `n` secondo l'ordinamento.

#### Errori

In validazione, `InvalidPlan`:

- `n` assente, `n` o `offset` negativi o oltre `max_rows`;
- `offset > 0` con `n = 0`;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati (un `ResourceLimit` solo
se `n` o `offset` non stanno in un `usize` della piattaforma).

#### Limiti e deviazioni

Chiamato direttamente su un solo blocco di righe, il kernel limita quel
blocco: non tiene stato fra blocchi. Nel runner ogni tabella è un blocco
solo, quindi il limite vale per la tabella.

#### Complessità

Tempo O(c) sulle colonne, indipendente dalle righe; nessuna memoria per i
dati (finestra sugli array d'ingresso).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.limit", "in": ["t"],
 "config": {"n": 2, "offset": 1}}
```

Ingresso `t`:

| `id: int64` | `nome: utf8` |
| --- | --- |
| 10 | a |
| 20 | null |
| 30 | c |
| 40 | d |

Uscita `risultato`:

| `id: int64` | `nome: utf8` |
| --- | --- |
| 20 | null |
| 30 | c |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.lookup`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `lookup` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 3, kernel 2 |

#### Che cosa fa

Traduce i valori di una colonna con una tabella di corrispondenza scritta
nella config: ogni cella il cui testo è una chiave di `mapping` diventa il
valore associato; le altre diventano `default`, oppure, se `default` è
`null`, restano come sono. Il risultato è testo, nella stessa colonna o in
una nuova.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna leggibile come testo | colonna da tradurre |
| `mapping` | oggetto | obbligatorio | chiavi stringa, valori JSON qualsiasi il cui testo è al più `max_string_bytes` byte; al più `max_rows` voci | corrispondenze testo della cella → valore |
| `default` | JSON | `null` | qualsiasi, con testo al più `max_string_bytes` byte | valore delle celle non nulle senza voce; `null` le lascia invariate |
| `output_column` | stringa | `column` | nome valido (non vuoto, al più 1024 byte) | colonna d'uscita; assente, sovrascrive `column` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. La chiave si confronta con il testo
della cella byte per byte: un `int64` `5` è `"5"`, un `float64` `2.0` è
`"2"`, una data è `AAAA-MM-GG`.

Il valore scritto è il testo del valore JSON: una stringa vale sé stessa, un
numero o un booleano il suo testo JSON (`1.50` diventa `1.5`), un `null` la
stringa vuota (non una cella nulla). Una chiave ripetuta in `mapping` vale
l'ultima occorrenza.

#### Schema

La colonna d'uscita è `utf8` nullable: sostituisce `column` al suo posto
(tipo e metadati di campo originali si perdono) o si aggiunge in coda con il
nome `output_column`; se `output_column` è un'altra colonna esistente, la
sostituisce al suo posto. Le altre colonne e i metadati di schema restano.
`row_count` resta; `sorted_by` resta solo se nessuna colonna esistente è
sovrascritta.

#### Righe

1:1. Una cella nulla resta nulla, anche con `default`.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non leggibile come testo;
- `mapping` con più di `max_rows` voci;
- il testo di un valore di `mapping` o di `default` oltre
  `max_string_bytes` byte;
- `output_column` vuoto o oltre 1024 byte, o `null` esplicito (il
  parametro si omette; `default: null` resta ammesso);
- config con campi sconosciuti.

In esecuzione, `Schema`: una cella che non si converte in testo (`binary`
non UTF-8, data o istante fuori intervallo).

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(n + m) con m voci di `mapping` (una mappa hash costruita una volta);
memoria O(m) per la mappa più la colonna d'uscita. Su `utf8` la ricerca
procede in parallelo per blocchi di righe, con uscita nell'ordine delle
righe.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.lookup", "in": ["clienti"],
 "config": {"column": "stato", "mapping": {"A": "attivo", "S": "sospeso"}, "default": "altro", "output_column": "stato_esteso"}}
```

Ingresso `clienti`:

| `id: int64` | `stato: utf8` |
| --- | --- |
| 1 | A |
| 2 | S |
| 3 | X |
| 4 | null |

Uscita `risultato`:

| `id: int64` | `stato: utf8` | `stato_esteso: utf8` |
| --- | --- | --- |
| 1 | A | attivo |
| 2 | S | sospeso |
| 3 | X | altro |
| 4 | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.mask_data`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `mask_data` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 2, analisi 3, kernel 4 |

#### Che cosa fa

Maschera il contenuto di una o più colonne sostituendo con un carattere di
maschera la parte centrale del testo: codici fiscali, indirizzi email,
numeri di telefono e IBAN hanno una forma fissa, `custom` lascia in chiaro
un numero scelto di caratteri all'inizio e alla fine. Il risultato va in
una colonna nuova `<colonna>_masked` o sovrascrive la colonna.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `maskings` | lista di oggetti | obbligatorio | da 1 a `max_columns` voci | mascherature, applicate in sequenza |
| `maskings[].column` | stringa | obbligatorio | colonna dell'ingresso leggibile come testo | colonna da mascherare |
| `maskings[].mask_type` | stringa | `custom` | `cf`, `email`, `phone`, `iban`, `custom` | forma della maschera |
| `maskings[].chars_start` | intero | `3` | intero non negativo; solo con `custom`; `null` non ammesso | caratteri iniziali in chiaro |
| `maskings[].chars_end` | intero | `3` | intero non negativo; solo con `custom`; `null` non ammesso | caratteri finali in chiaro |
| `maskings[].mask_char` | stringa | `"*"` | un solo carattere; solo con `custom`; `null` non ammesso | carattere di maschera |
| `overwrite` | booleano | `false` | `true`, `false` | `true` sovrascrive la colonna, `false` scrive `<colonna>_masked` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Le forme, contando i caratteri Unicode (non i byte):

- `custom`: restano `chars_start` caratteri iniziali e `chars_end` finali,
  ognuno degli altri diventa `mask_char`; un testo non più lungo di
  `chars_start + chars_end` resta com'è;
- `cf`: come `custom` con 3 e 3 e `*`;
- `iban`: come `custom` con 4 e 4 e `*` (gli spazi contano come caratteri);
- `email`: la parte prima dell'ultima `@` diventa il suo primo carattere
  seguito da un `*` per ciascuno degli altri (un solo `*` se ha al più un
  carattere); il dominio resta; senza `@` il testo resta com'è;
- `phone`: si tengono solo cifre e `+`; se ne restano meno di 6 il testo
  originale resta com'è, altrimenti il testo compattato con 3 caratteri
  iniziali e 4 finali in chiaro (`+39 333 1234567` → `+39******4567`).

Con `overwrite` una seconda voce sulla stessa colonna maschera il risultato
della prima. Senza `overwrite` la stessa `column` due volte si rifiuta: la
seconda riscriverebbe `<colonna>_masked` partendo dalla colonna originale e
la prima non avrebbe effetto. Una voce non può nominare una colonna
`_masked` creata da una voce precedente: l'analisi cerca le colonne
nell'ingresso e la rifiuta.

#### Schema

Ogni colonna d'uscita è `utf8` nullable. Con `overwrite` sostituisce la
colonna al suo posto (perdendone tipo e metadati di campo); senza,
`<colonna>_masked` si aggiunge in coda nell'ordine delle voci (o sostituisce
al suo posto una colonna esistente con quel nome). Metadati di schema
conservati; `row_count` resta; `sorted_by` resta solo se nessuna colonna
esistente è sovrascritta.

#### Righe

1:1. Una cella nulla resta nulla.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `maskings` vuoto o con più di `max_columns` voci;
- una `column` assente dall'ingresso o non leggibile come testo;
- la stessa `column` in due voci senza `overwrite`;
- `chars_start`, `chars_end` o `mask_char` con un `mask_type` diverso da
  `custom`;
- `mask_char` che non è un solo carattere;
- `chars_start`, `chars_end` o `mask_char` `null` espliciti (un parametro
  facoltativo si omette);
- nome d'uscita non valido (vuoto o oltre 1024 byte);
- config con campi sconosciuti o `mask_type` fuori elenco.

In esecuzione:

- `Schema`: una cella che non si converte in testo (`binary` non UTF-8,
  data o istante fuori intervallo);
- `ResourceLimit`: un valore mascherato oltre `max_string_bytes` byte.

#### Limiti e deviazioni

La maschera conta i caratteri Unicode, non i grafemi: un carattere composto
da più code point (emoji con modificatori, lettere con diacritici
combinanti) conta per più di uno. Non riconosce il formato del dato: un
`cf` o un `iban` non validi si mascherano con la stessa regola.

#### Complessità

Tempo O(byte delle colonne mascherate); memoria O(n) per ogni colonna
d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.mask_data", "in": ["clienti"],
 "config": {"maskings": [{"column": "email", "mask_type": "email"}, {"column": "codice", "chars_start": 2, "chars_end": 1}]}}
```

Ingresso `clienti`:

| `email: utf8` | `codice: utf8` |
| --- | --- |
| mario.rossi@example.com | ABC12345 |
| null | XY |

Uscita `risultato`:

| `email: utf8` | `codice: utf8` | `email_masked: utf8` | `codice_masked: utf8` |
| --- | --- | --- | --- |
| mario.rossi@example.com | ABC12345 | m**********@example.com | AB*****5 |
| null | XY | null | XY |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.md5_hash`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `md5_hash` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 3, kernel 4 |

#### Che cosa fa

Aggiunge una colonna con l'hash MD5, in esadecimale minuscolo, dei valori di
alcune colonne di ogni riga. Serve a confrontare o raggruppare righe per
contenuto; con `normalize` (default) differenze di maiuscole e di spazi ai
lati non cambiano l'hash.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | da 1 a `max_columns` colonne leggibili come testo, senza ripetizioni | colonne da cui si calcola l'hash |
| `output_column` | stringa | `md5_hash` | nome valido | colonna d'uscita |
| `normalize` | booleano | `true` | `true`, `false` | toglie gli spazi ai lati e porta in minuscolo ogni valore |
| `null_policy` | stringa | `empty` | `empty`, `literal`, `error` | come entra una cella nulla |
| `null_literal` | stringa | `"<null>"` | al più `max_string_bytes` byte; solo con `null_policy = "literal"`; `null` non ammesso | testo di una cella nulla con `literal` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Il messaggio di una riga è il testo delle celle, con le colonne in ordine
di nome (l'ordine scritto in `columns` non conta), unite dal carattere
U+001F. Con `normalize` ogni testo passa da `trim` e `to_lowercase`
(Unicode), e così `null_literal`. Una cella nulla con `empty` vale il testo
vuoto, con `literal` vale `null_literal`, con `error` rifiuta la riga.

`null_literal` scritto con una `null_policy` diversa da `literal` si
rifiuta; se manca, con `literal` vale `<null>`.

#### Schema

La colonna d'uscita è `utf8` non nullable, 32 cifre esadecimali: si
aggiunge in coda o sostituisce al suo posto una colonna con lo stesso nome
(perdendone tipo e metadati di campo). Metadati di schema conservati;
`row_count` resta; `sorted_by` resta solo se nessuna colonna esistente è
sovrascritta.

#### Righe

1:1. Con `null_policy` `error` una sola cella nulla nelle colonne scelte fa
fallire il passo.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con ripetizioni o con più di `max_columns` colonne;
- una colonna assente o non leggibile come testo;
- `output_column` non valido; `null_literal` oltre `max_string_bytes`, o
  `null` esplicito (il parametro si omette);
- `null_literal` scritto con una `null_policy` diversa da `literal`: non
  avrebbe effetto;
- config con campi sconosciuti o `null_policy` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga, solo con `null_policy` `error`:
  ogni riga con una cella nulla (`validation.required_value_missing`, sulla
  prima colonna nulla in ordine di nome); il passo non produce uscita;
- `Schema`: una cella che non si converte in testo (`binary` non UTF-8,
  data o istante fuori intervallo).

#### Limiti e deviazioni

MD5 non è resistente alle collisioni: non usarlo come impronta di
sicurezza. Il messaggio non ha delimitazioni: un valore che contiene U+001F
può produrre lo stesso messaggio di valori diversi; con `empty` una cella
nulla e una vuota coincidono, con `literal` una cella nulla e una che
contiene `null_literal`. Nome e tipo delle colonne non entrano nell'hash.
Per un'impronta senza ambiguità: [`table.sha256_hash`](#tablesha256_hash) o
[`table.stable_fingerprint`](#tablestable_fingerprint).

#### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Con `normalize` le prime due righe hanno lo stesso hash; nella terza il
nome nullo vale il testo vuoto.

Passo del piano:

```json
{"out": "risultato", "op": "table.md5_hash", "in": ["clienti"],
 "config": {"columns": ["nome", "citta"]}}
```

Ingresso `clienti`:

| `nome: utf8` | `citta: utf8` |
| --- | --- |
| Anna | Roma |
|  anna  | ROMA |
| null | Roma |

Uscita `risultato`:

| `nome: utf8` | `citta: utf8` | `md5_hash: utf8` |
| --- | --- | --- |
| Anna | Roma | e31b753eebf823f115975d5448d1756f |
|  anna  | ROMA | e31b753eebf823f115975d5448d1756f |
| null | Roma | 1a3394144c6d6416f9e84e1e86eaf230 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.melt`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `melt` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 2, kernel 4 |

#### Che cosa fa

Porta la tabella da larga a lunga: ogni colonna valore diventa un blocco di
righe, con il nome della colonna in `var_name` e la cella in `value_name`;
le colonne id si ripetono su ogni blocco.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `id_columns` | lista di stringhe | obbligatorio | nomi di colonne dell'ingresso, senza ripetizioni; anche vuota | colonne ripetute su ogni riga d'uscita |
| `value_columns` | lista di stringhe | `[]` | nomi di colonne dell'ingresso, senza ripetizioni | colonne da portare in righe; vuota vale tutte le colonne non id |
| `var_name` | stringa | `"variable"` | nome di colonna valido, diverso da `value_name` | colonna con il nome della colonna valore |
| `value_name` | stringa | `"value"` | nome di colonna valido | colonna con la cella |
| `type_policy` | stringa | `"reject"` | `"reject"`, `"string"`; `null` non ammesso | colonne valore di tipi diversi: rifiuto, o conversione in testo |

`var_name` e `value_name` non collidono con nessuna colonna dell'ingresso,
nemmeno con quelle che spariscono: un nome già preso riceve il primo
suffisso libero fra `_1` e `_99`, prima `var_name` poi `value_name`
(sciogliere una colonna di nome `value` dà una colonna `value_1`).

`type_policy` con colonne valore tutte dello stesso tipo si accetta e non
cambia niente: dipende dall'ingresso, e lo stesso piano gira su tabelle
diverse.

#### Schema

Le colonne di `id_columns` nel loro ordine, con tipo, nullabilità e
metadati di campo; poi `var_name`, `utf8` non nullabile; poi `value_name`,
nullabile e senza metadati di campo, del tipo comune se le colonne valore
hanno tutte lo stesso tipo Arrow, altrimenti `utf8` con
`type_policy: "string"`. I metadati di schema si conservano. Una colonna
geometrica resta geometrica solo se è una colonna id. Il contratto dichiara
righe × colonne valore quando il conteggio d'ingresso è noto, e nessun
ordinamento.

Con `type_policy: "string"` le celle diventano il loro testo: `1.0` è
`"1"`, `-0.0` è `"-0"`, le date `AAAA-MM-GG`, i timestamp in RFC 3339, i
booleani `true`/`false`, i `decimal128` con tutte le cifre della scala.

#### Righe

Espansione: righe × colonne valore. Una cella nulla dà una riga con valore
nullo: nessuna riga si scarta.

#### Ordine

Per colonna valore, nell'ordine di `value_columns` (o delle colonne
dell'ingresso): prima tutte le righe per la prima colonna, in ordine
d'ingresso, poi tutte per la seconda, e così via.

#### Errori

In validazione:

- `InvalidPlan`: `var_name` uguale a `value_name`; una lista con un nome
  ripetuto o non valido, o oltre il limite di colonne; una colonna assente;
  nessuna colonna valore; colonne valore di tipi diversi con
  `type_policy: "reject"`; `type_policy: null` esplicito (il parametro si
  omette); nessun suffisso libero per un nome d'uscita; campi sconosciuti;
- `Schema`: con `type_policy: "string"` e tipi diversi, una colonna valore
  di tipo non convertibile in testo o con una timezone non valida.

In esecuzione:

- `ResourceLimit`: righe d'uscita oltre `max_rows`; stima dei byte
  d'uscita oltre `max_governed_memory_bytes`; un testo oltre
  `max_string_bytes`;
- `Schema`: con la conversione in testo, una cella che non si converte
  (`binary` non UTF-8, date fuori intervallo).

#### Limiti e deviazioni

Prima di allocare, il kernel stima i byte dell'uscita (colonne id
ripetute, nome della colonna valore più lungo, colonna valore più larga,
misurata in testo con la conversione) e rifiuta oltre
`max_governed_memory_bytes`. L'espansione (righe × colonne valore) è
fissata da config e schema: il runner non le applica il fattore di
espansione e dopo il passo controlla invece il numero esatto di righe,
oltre alle righe per arco
([README, «Esecuzione»](../README.md#esecuzione)).

#### Complessità

Tempo e memoria O(n · k) per n righe e k colonne valore.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.melt", "in": ["larga"],
 "config": {"id_columns": ["id"], "value_columns": ["q1", "q2"], "var_name": "trimestre", "value_name": "vendite"}}
```

Ingresso `larga`:

| `id: int64` | `q1: int64` | `q2: int64` |
| --- | --- | --- |
| 1 | 10 | 20 |
| 2 | null | 30 |

Uscita `risultato`:

| `id: int64` | `trimestre: utf8` | `vendite: int64` |
| --- | --- | --- |
| 1 | q1 | 10 |
| 2 | q1 | null |
| 1 | q2 | 20 |
| 2 | q2 | 30 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.pivot`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `pivot` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 4, kernel 5 |

#### Che cosa fa

Porta la tabella da lunga a larga: una riga per ogni chiave delle colonne
`index_col`, una colonna per ogni valore distinto di `pivot_col`, e in ogni
cella l'aggregazione (`aggr_func`) dei valori di `value_col` delle righe
con quella chiave e quel valore. Con `mapping` le colonne d'uscita le
fissa la config e il runner esegue l'operazione; senza, dipendono dai dati:
il runner la rifiuta in validazione, e l'esempio sotto (senza `mapping`) è
eseguito chiamando il kernel.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `index_col` | stringa | obbligatorio | nomi di colonne separati da virgola, almeno uno, senza ripetizioni né voci vuote (`"a,,b"`, `"a,"`) | chiave delle righe; spazi ai lati tolti |
| `pivot_col` | stringa | obbligatorio | nome di una colonna dell'ingresso | i suoi valori diventano colonne |
| `value_col` | stringa | obbligatorio | nome di una colonna dell'ingresso | valori aggregati nelle celle |
| `aggr_func` | stringa | `"first"` | `first`, `last`, `min`, `max`, `sum`, `mean`, `count`, `concat` | aggregazione di una cella |
| `mapping` | oggetto | `{}` | testo → nome di colonna | se non vuoto, fissa le colonne pivot: una per voce, con il nome della voce |

Il valore pivot è il testo della cella (`1.0` è `"1"`, `-0.0` è `"-0"`,
ogni NaN `"NaN"`, le date `AAAA-MM-GG`); le righe con il valore pivot
nullo, o escluso da `mapping`, non riempiono celle ma la loro chiave dà
comunque una riga. Con `mapping` il valore si confronta con le chiavi come
testo: la `pivot_col` deve essere `utf8`, dizionario di `utf8`, `int64` o
`uint64`, e con un intero ogni chiave deve essere la forma canonica di un
intero (`"1"`, non `"01"` né `"1.0"`), perché una chiave che nessun valore
può incontrare darebbe una colonna tutta null senza errore. Le celle, dalle
righe della cella in ordine d'ingresso:

- `first`, `last`: la cella di `value_col` nella prima o nell'ultima riga,
  null compreso, con il tipo di `value_col`;
- `count`: `int64`, le celle non nulle;
- `concat`: `utf8`, i testi delle celle non nulle uniti da `,`; solo null
  dà il testo vuoto; il testo di una cella non supera `max_string_bytes`
  byte;
- `sum`, `mean`, `min`, `max` (tipi numerici di
  [`table.aggregate`](#tableaggregate)): i null si saltano, solo null dà
  null. Sulle colonne intere (`int64`, `uint64`) `sum` è esatta ed esce
  `int64` (oltre `int64` è un errore); `sum` su `date32`, `date64` o
  `timestamp` si rifiuta; `mean` su interi, date e istanti parte dalla somma esatta; `min`
  e `max` sulle colonne intere e `decimal128` rendono la cella estrema nel
  tipo di `value_col`. Altrove l'uscita è `float64` sulla cella letta come
  `f64`; `min` e `max` ignorano i NaN salvo che siano tutti NaN, `sum` e
  `mean` no.

Una combinazione chiave-valore che non compare nei dati è null, anche con
`count`.

#### Schema

Le colonne di `index_col` nel loro ordine, con tipo, nullabilità e
metadati di campo; poi le colonne pivot, nullabili e senza metadati di
campo, con il tipo sopra:

- senza `mapping`, una per valore pivot distinto dei dati, nell'ordine dei
  byte del testo del valore;
- con `mapping`, una per voce, nell'ordine delle chiavi (byte per byte) e
  con il nome della voce, anche per un valore che i dati non contengono
  (colonna tutta null); i valori fuori dal mapping non danno colonne.

I metadati di schema si conservano.

#### Righe

Aggregazione: una riga per chiave distinta di `index_col`, con
l'uguaglianza di [`table.distinct`](#tabledistinct).

#### Ordine

Le righe nell'ordine delle chiavi di [`table.aggregate`](#tableaggregate):
null per primo, poi per la stringa `<lunghezza>:<testo>` byte per byte
(una chiave `timestamp` in ordine cronologico).

#### Errori

In validazione, in quest'ordine:

1. `InvalidPlan`: campi sconosciuti; una colonna assente fra quelle di
   `index_col`, la `pivot_col` e la `value_col`. Questi due controlli
   valgono anche senza `mapping`;
2. `Unsupported`: `mapping` assente o vuoto (lo schema d'uscita dipende
   dai dati). Senza `mapping` la validazione si ferma qui: `index_col`
   senza colonne, con una voce vuota o con una colonna ripetuta, e ogni
   controllo di tipo sotto, danno `Unsupported`, non `InvalidPlan` (il
   kernel, chiamato direttamente, li rifiuta con `InvalidPlan` o
   `Schema`);
3. solo con `mapping` non vuoto:
   - `InvalidPlan`: `index_col` senza colonne, con una voce vuota
     (`"a,,b"`, `"a,"`), con una colonna ripetuta o oltre `max_columns`;
     una colonna indice o la `pivot_col` che non si legge come testo; con
     `sum`, `mean`, `min`, `max` una `value_col` non numerica, con
     `concat` una che non si legge come testo; un nome
     del mapping non valido, ripetuto o uguale a una colonna indice; una
     chiave che non è la forma canonica di un intero su una `pivot_col`
     intera; `mapping` su una `pivot_col` che non è testo né intero
     (float, date, istanti, decimali, booleani);
   - `ResourceLimit`: colonne indice più voci del mapping oltre
     `max_columns`.

In esecuzione (dal runner con `mapping`, o chiamando il kernel):

- `Schema`: una colonna assente; con `sum`, `mean`, `min`, `max`, una
  `value_col` di tipo non numerico o una sua cella `utf8` che non è un
  numero; una cella di chiave, di valore pivot o (con `concat`) di valore
  che non si converte in testo (tipo non leggibile come testo, `binary`
  non UTF-8 fra i valori pivot, date fuori intervallo);
- `InvalidPlan`: le regole di `Pivot::verifica_mapping`, le stesse della
  validazione (`index_col`, nomi e chiavi del mapping); senza `mapping`, un
  valore pivot vuoto o di soli spazi, o uguale a una colonna indice;
- `ResourceLimit`: righe oltre `max_rows` o colonne oltre `max_columns`;
  più di `u32::MAX` righe; con `concat`, il testo di una cella oltre
  `max_string_bytes` byte;
- `DataMapping`: `sum` su una colonna intera oltre la gamma di `int64`.

#### Limiti e deviazioni

Senza `mapping` il runner non esegue l'operazione (lo schema dipende dai
dati); `table.transpose` ha lo stesso limite. Su `decimal128` e testo
`sum` e `mean` arrotondano, perché il risultato è `float64`; su una colonna
intera `mean` arrotonda una volta, dopo la somma esatta ([README, «Somme intere esatte e tipi delle riduzioni»](../README.md#somme-intere-esatte-e-tipi-delle-riduzioni)).
L'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n) sulle righe più O(g log g + p log p) per ordinare g chiavi e p
valori pivot; memoria O(c) per le c celle presenti e O(g · p) per l'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.pivot", "in": ["vendite"],
 "config": {"index_col": "negozio", "pivot_col": "mese", "value_col": "vendite", "aggr_func": "sum"}}
```

Ingresso `vendite`:

| `negozio: utf8` | `mese: utf8` | `vendite: float64` |
| --- | --- | --- |
| A | gen | 10.0 |
| A | feb | 5.0 |
| B | gen | 7.0 |
| A | gen | 1.0 |

Uscita `risultato`:

| `negozio: utf8` | `feb: float64` | `gen: float64` |
| --- | --- | --- |
| A | 5.0 | 11.0 |
| B | null | 7.0 |

Verifica: eseguito dal kernel (il runner rifiuta questa config in validazione, perché lo schema d'uscita dipende dai dati); l'uscita è confrontata cella per cella.

### `table.reconcile`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `reconcile` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 3, kernel 3 |

#### Che cosa fa

Confronta due tabelle sulle chiavi `left_keys` e `right_keys` e restituisce
un resoconto di cinque metriche: quante righe si abbinano, quante restano
solo a sinistra o solo a destra, quante sono duplicati di una chiave su
ciascun lato. Le righe si abbinano una a una per chiave: se una chiave
compare 3 volte a sinistra e 2 a destra, 2 righe sono abbinate e 1 resta
solo a sinistra.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | almeno un nome, senza ripetizioni, al più 4096; colonne leggibili come testo scalare | colonne della chiave nella tabella sinistra |
| `right_keys` | lista di stringhe | obbligatorio | tanti nomi quanti `left_keys`, senza ripetizioni; stesse condizioni | colonne della chiave nella tabella destra |
| `nulls_equal` | booleano | `true` | `true`, `false` | con `true` il null è un valore della chiave e due null si abbinano; con `false` una riga con un null nella chiave resta sola sul suo lato |

Le colonne si abbinano per posizione: `left_keys[i]` con `right_keys[i]`, e
ogni coppia ha lo stesso tipo Arrow. Leggibili come testo scalare: `utf8`,
`int64`, `uint64`, `float64`, `bool`, `binary`, `date32`, `date64`,
`timestamp` di ogni unità (con fuso valido), `decimal128` (scala da 0 a 38), dizionario
`int32`→`utf8`.

Due chiavi sono uguali quando lo sono tutte le loro colonne, con
l'uguaglianza della forma testuale: sugli interi, i testi, le date e i
decimali è l'uguaglianza dei valori; su `float64` tutti i `NaN` sono uguali e
`0.0` è diverso da `-0.0`; un `binary` si confronta sui byte.

#### Schema

Una tabella nuova, indipendente dagli ingressi: `metric` (`utf8`) e `value`
(`uint64`), non nullabili. Nessuna colonna e nessun metadato di schema degli
ingressi. Il contratto dichiara 5 righe, dimostrate.

#### Righe

Sempre 5 righe, una per metrica, anche da due ingressi vuoti (metriche a
zero): il numero di righe non dipende dagli ingressi, e il catalogo esenta
l'operazione dal fattore di espansione. Per ogni chiave, con `L` righe a sinistra
e `R` a destra:

- `matched_rows`: somma di `min(L, R)`;
- `left_only_rows`: somma di `L − min(L, R)`, più le righe sinistre con un
  null nella chiave se `nulls_equal=false`;
- `right_only_rows`: somma di `R − min(L, R)`, più le righe destre con un
  null nella chiave se `nulls_equal=false`;
- `left_duplicate_rows`: somma di `L − 1` sulle chiavi presenti a sinistra;
- `right_duplicate_rows`: somma di `R − 1` sulle chiavi presenti a destra.

Con `nulls_equal=false` le righe con una chiave nulla non contano fra i
duplicati.

#### Ordine

Le metriche nell'ordine sopra, sempre lo stesso.

#### Errori

In validazione, `InvalidPlan`:

- `left_keys` vuoto, liste di lunghezza diversa, nomi ripetuti o non validi,
  oltre 4096 nomi;
- una colonna chiave non esiste, non è leggibile come testo scalare, o ha
  un tipo diverso da quello della colonna abbinata;
- config con campi sconosciuti.

In esecuzione:

- `ResourceLimit`: le chiavi distinte, contate lato per lato (una chiave
  presente nei due lati conta due volte), superano il margine di memoria
  che il runner passa al kernel (`max_governed_memory_bytes`), a lunghezza
  della forma testuale più 64 byte per chiave; oppure le chiavi distinte di
  un lato superano `max_input_rows`;
- `Schema`: una cella di chiave non si converte in testo (`date32`, `date64` o
  `timestamp` fuori dall'intervallo di calendario, `date64` non allineato al giorno).

#### Limiti e deviazioni

L'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).
La memoria contata è quella delle chiavi, non quella delle mappe che le
contengono.

#### Complessità

Tempo O(n + m) atteso su righe sinistre e destre; memoria O(d) per le
chiavi distinte dei due lati.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.reconcile", "in": ["contabilita", "banca"],
 "config": {"left_keys": ["codice"], "right_keys": ["codice"]}}
```

Ingresso `contabilita`:

| `codice: utf8` |
| --- |
| A |
| A |
| B |
| C |

Ingresso `banca`:

| `codice: utf8` |
| --- |
| A |
| B |
| B |
| D |

Uscita `risultato`:

| `metric: utf8` | `value: uint64` |
| --- | --- |
| matched_rows | 2 |
| left_only_rows | 2 |
| right_only_rows | 2 |
| left_duplicate_rows | 1 |
| right_duplicate_rows | 1 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.rename`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `rename` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Cambia il nome delle colonne secondo le coppie `old_name` → `new_name`.
Le rinomine valgono tutte insieme, quindi si possono scambiare due nomi. Le
colonne non nominate restano come sono. I dati non si copiano.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `renames` | lista di oggetti | obbligatorio | da 1 a 4096 coppie | rinomine da applicare |
| `renames[].old_name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte, mai ripetuto fra le coppie | colonna da rinominare |
| `renames[].new_name` | stringa | obbligatorio | nome non vuoto, al più 1024 byte, diverso da `old_name`, mai ripetuto fra le coppie | nuovo nome |

Una lista vuota o una coppia con `old_name` uguale a `new_name` si
rifiutano. Un `old_name` che non è una colonna dell'ingresso si accetta e
non rinomina niente: dipende dall'ingresso, e lo stesso piano gira su
tabelle diverse. I nomi dell'uscita devono essere tutti diversi: un
`new_name` uguale al nome di una colonna che resta com'è si rifiuta.

#### Schema

Stesse colonne, nello stesso ordine, con tipo, nullabilità e metadati di
campo; cambiano solo i nomi. I metadati di schema restano.

Contratto: conteggio delle righe e ordinamento dichiarato restano, anche
quando una chiave dell'ordinamento è stata rinominata; la colonna
geometrica resta tale col nuovo nome.

#### Righe

1:1: stesse righe, stessi valori.

#### Ordine

Righe e colonne nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `renames` vuota;
- un `old_name` uguale al suo `new_name`;
- lo stesso `old_name` in due coppie, o lo stesso `new_name` in due coppie
  (anche se uno degli `old_name` non è una colonna dell'ingresso);
- un nome vuoto, di soli spazi o oltre 1024 byte (in entrambe le
  posizioni), o più di 4096 coppie;
- l'uscita avrebbe due colonne con lo stesso nome;
- config con campi sconosciuti.

Le stesse regole le applica il kernel, con la stessa funzione della
validazione: due coppie con lo stesso `new_name` si rifiutano anche
chiamando il kernel, invece di perdere in silenzio una delle rinomine.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(c + r) sulle colonne e sulle coppie, indipendente dalle righe;
nessuna memoria per i dati.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.rename", "in": ["t"],
 "config": {"renames": [
    {"old_name": "a", "new_name": "b"},
    {"old_name": "b", "new_name": "a"},
    {"old_name": "cod", "new_name": "codice"}
  ]}}
```

Ingresso `t`:

| `a: int64` | `b: utf8` | `cod: utf8` |
| --- | --- | --- |
| 1 | x | K1 |
| 2 | null | K2 |

Uscita `risultato`:

| `b: int64` | `a: utf8` | `codice: utf8` |
| --- | --- | --- |
| 1 | x | K1 |
| 2 | null | K2 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.reorder_columns`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `reorder_columns` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Cambia l'ordine delle colonne senza toglierne né aggiungerne: prima quelle
elencate in `columns`, nell'ordine dato, poi tutte le altre, nell'ordine
d'ingresso oppure, con `alphabetical`, in ordine alfabetico. I dati non si
copiano.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | `[]` | colonne dell'ingresso, senza ripetizioni; vuota solo con `alphabetical = true` | colonne da mettere in testa, nell'ordine dato |
| `alphabetical` | booleano | `false` | `true`, `false`; alias `sort_alphabetical`; `null` non ammesso | ordina alfabeticamente le colonne non elencate |

`alphabetical` riguarda solo le colonne non elencate in `columns`. L'ordine
alfabetico confronta i nomi in minuscolo (minuscole Unicode) byte per byte
in UTF-8, quindi le lettere accentate vanno dopo la `z`; due nomi uguali in
minuscolo restano nell'ordine d'ingresso.

`columns` vuota senza `alphabetical = true` non sposterebbe niente con
nessun ingresso e si rifiuta. `alphabetical` quando al più una colonna
d'ingresso non è elencata in `columns` si accetta e non ordina niente:
dipende dall'ingresso, e lo stesso piano gira su tabelle diverse.
`alphabetical: null` esplicito si rifiuta: il parametro si omette.

#### Schema

Stesse colonne, con tipo, nullabilità e metadati di campo; cambia solo la
posizione. I metadati di schema restano. Il contratto conserva il
conteggio delle righe e l'ordinamento dichiarato.

#### Righe

1:1: stesse righe, stessi valori.

#### Ordine

Righe nell'ordine d'ingresso; colonne come descritto sopra.

#### Errori

In validazione, `InvalidPlan`:

- un nome di `columns` ripetuto o che non è una colonna dell'ingresso;
- `columns` vuota senza `alphabetical = true`;
- `alphabetical` (o `sort_alphabetical`) `null` esplicito;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(c log c) sulle colonne, indipendente dalle righe; nessuna memoria
per i dati.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.reorder_columns", "in": ["t"],
 "config": {"columns": ["id"], "alphabetical": true}}
```

Ingresso `t`:

| `zona: utf8` | `Anno: int64` | `id: int64` | `citta: utf8` |
| --- | --- | --- | --- |
| N | 2023 | 1 | Roma |
| S | 2024 | 2 | null |

Uscita `risultato`:

| `id: int64` | `Anno: int64` | `citta: utf8` | `zona: utf8` |
| --- | --- | --- | --- |
| 1 | 2023 | Roma | N |
| 2 | 2024 | null | S |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.replace`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `replace` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Sostituisce testo nella colonna `utf8` `column`. Senza `regex` il
confronto è sulla cella intera: una cella uguale a `old_value` diventa
`new_value`, le altre restano (non si sostituiscono sottostringhe). Con
`regex` ogni match di `old_value` nella cella, da sinistra e senza
sovrapposizioni, si sostituisce con `new_value`.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | colonna da modificare |
| `old_value` | stringa | obbligatorio | con `regex`, regex valida del crate `regex` di al più `max_regex_bytes` byte; senza, al più `max_string_bytes` byte | cella da sostituire, o pattern |
| `new_value` | stringa | obbligatorio | al più `max_string_bytes` byte | testo sostitutivo |
| `regex` | booleano | `false` | `true`, `false` | interpreta `old_value` come espressione regolare |

Con `regex` il `new_value` riconosce i riferimenti ai gruppi: `$1`,
`${1}`, `$nome`, `${nome}`; `$$` scrive un `$`. Un riferimento seguito da
lettere o cifre va scritto tra graffe: `$1a` è il gruppo di nome `1a` (che
non esiste, e vale testo vuoto), `${1}a` è il gruppo 1 seguito da `a`.
Senza `regex` il `$` non ha significato speciale.

Con `regex` ogni cella sostituita non supera `max_string_bytes` byte; una
regex che combacia con il testo vuoto inserisce `new_value` in ogni
posizione, e può superarlo.

#### Schema

Stesse colonne; `column` resta nella sua posizione, `utf8`, con i suoi
metadati di campo, e diventa nullable. Contratto: il conteggio delle righe
resta; l'ordinamento dichiarato cade.

#### Righe

1:1; il null resta null.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- con `regex`, `old_value` non valido o oltre `max_regex_bytes`; senza,
  `old_value` oltre `max_string_bytes`;
- `new_value` oltre `max_string_bytes`;
- config con campi sconosciuti.

Il limite di `old_value` lo applica anche il kernel.

In esecuzione, `ResourceLimit`: con `regex`, una cella sostituita oltre
`max_string_bytes` byte.

#### Limiti e deviazioni

- Per sostituire una sottostringa letterale serve `regex: true` con i
  metacaratteri protetti da `\`.

#### Complessità

Tempo lineare nei byte della colonna (confronto, o ricerca regex in tempo
lineare); memoria pari ai byte della colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.replace", "in": ["contatti"],
 "config": {"column": "telefono", "old_value": "^\\+39\\s*(\\d+)$", "new_value": "0039-${1}", "regex": true}}
```

Ingresso `contatti`:

| `telefono: utf8` |
| --- |
| +39 0612345 |
| 0612345 |
| null |

Uscita `risultato`:

| `telefono: utf8` |
| --- |
| 0039-0612345 |
| 0612345 |
| null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.rolling_window`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `rolling_window` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 3, kernel 5 |

#### Che cosa fa

Calcola per ogni riga un'aggregazione (somma, media, minimo, massimo,
deviazione standard) della colonna `column` sulle ultime `window` righe
della sua partizione, riga corrente compresa, e la aggiunge come colonna
(`float64`, o del tipo detto sotto). Con `order_column` le righe si
riordinano prima su quella colonna.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica (come in [`table.window_function`](#tablewindow_function)) | colonna aggregata |
| `function` | stringa | obbligatorio | `sum`, `mean`, `min`, `max`, `stddev` | aggregazione |
| `window` | intero | obbligatorio | da `1` a `max_rows` | righe della finestra, corrente compresa |
| `min_periods` | intero | `1` | da `1` a `window` | valori non nulli minimi per un risultato |
| `group_by` | stringa | nessuno | colonna leggibile come testo | partizione; senza, una partizione sola |
| `order_column` | stringa | nessuno | colonna di tipo ordinabile | ordinamento ascendente prima del calcolo |
| `ddof` | intero | `1` | `0` o più; solo con `stddev`; `null` non ammesso | gradi di libertà sottratti al divisore |
| `output_column` | stringa | obbligatorio | nome di colonna valido | colonna d'uscita |

La finestra si misura in righe, non in valori: una cella nulla occupa il
suo posto e non conta fra i valori. Con meno di `min_periods` valori non
nulli nella finestra il risultato è null. Sulle colonne intere (`int64`,
`uint64`) `sum` è esatta ed esce `int64` (una somma oltre `int64` è un
errore); `sum` su `date32`, `date64` o `timestamp` si rifiuta in validazione. Su
interi, date e istanti `mean` parte dalla somma esatta e `stddev` dagli
scarti esatti; altrove `sum` somma in `f64` in ordine di riga. `mean` è la somma
divisa per i valori. `min` e `max` sulle colonne intere e `decimal128`
rendono la cella estrema nel tipo della colonna; altrove ignorano i NaN
salvo che la finestra abbia solo NaN, `sum` e `mean` no. `stddev` divide
per `valori - ddof` e dà null con `valori <= ddof`.

#### Schema

L'ingresso più la colonna `output_column`, nullabile, senza metadati di
campo, in coda: `int64` con `sum` su una colonna intera, il tipo di
`column` con `min`/`max` su una colonna intera o `decimal128`, `float64`
altrimenti; se il nome esiste già, la colonna è sostituita
al suo posto. Gli altri metadati si conservano. Il contratto dichiara
l'uscita ordinata in ascendente su `order_column` se c'è (altrimenti
conserva l'ordinamento dell'ingresso) e conserva il conteggio.

#### Righe

1:1.

#### Ordine

Come [`table.window_function`](#tablewindow_function): con `order_column`,
le righe escono ordinate su quella colonna in ascendente (stabile, null in
coda), senza raggrupparle per partizione; senza, nell'ordine d'ingresso.
Le partizioni si formano sul testo di `group_by` (il null è una partizione
a sé), e la finestra scorre le loro righe in quest'ordine.

#### Errori

In validazione, `InvalidPlan`:

- `window` o `min_periods` uguali a 0, `min_periods` maggiore di `window`,
  `window` oltre `max_rows`;
- `ddof` con una funzione diversa da `stddev`; `ddof`, `group_by` o
  `order_column` `null` espliciti (il parametro si omette);
- `column` assente o non numerica; `group_by` non leggibile come testo;
  `order_column` di tipo non ordinabile; `output_column` non valido;
- funzione fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella `utf8` di `column` che non è un numero; una cella di
  `group_by` che non si converte in testo; una chiave di dizionario di
  `order_column` fuori dal proprio dizionario;
- `DataMapping`: un risultato di `sum`, `mean` o `stddev` non finito
  calcolato da valori finiti (overflow di `f64`); con `sum` su una colonna
  intera, una somma oltre la gamma di `int64`;
- `ResourceLimit`: più di `u32::MAX` righe con `order_column`.

#### Limiti e deviazioni

Su `decimal128` e testo numerico `sum`, `mean` e `stddev` leggono la cella
come `f64` e arrotondano senza errore, perché il risultato è `float64`;
sulle colonne intere `mean` e `stddev` arrotondano alla fine del calcolo
esatto
([README, «Somme intere esatte e tipi delle riduzioni»](../README.md#somme-intere-esatte-e-tipi-delle-riduzioni)).
Un `NaN` o un infinito già nei dati si propagano senza errore; solo
l'overflow di un calcolo su valori finiti si rifiuta.

#### Complessità

Tempo O(n · window): ogni riga riscorre la propria finestra (due volte per
`stddev`), più O(n log n) per l'ordinamento su `order_column`; memoria
O(n). Da 32.768 righe, con più di una partizione, le partizioni si
calcolano in parallelo, con lo stesso risultato.

#### Memoria

Memoria: da misura v4.

#### Esempio

Somma mobile su due righe: il null occupa il suo posto ma non conta, e
la somma di una colonna intera resta `int64`, esatta.

Passo del piano:

```json
{"out": "risultato", "op": "table.rolling_window", "in": ["giorni"],
 "config": {"column": "vendite", "function": "sum", "window": 2, "output_column": "somma_2"}}
```

Ingresso `giorni`:

| `giorno: int64` | `vendite: int64` |
| --- | --- |
| 1 | 1 |
| 2 | 2 |
| 3 | null |
| 4 | 4 |

Uscita `risultato`:

| `giorno: int64` | `vendite: int64` | `somma_2: int64` |
| --- | --- | --- |
| 1 | 1 | 1 |
| 2 | 2 | 3 |
| 3 | null | 2 |
| 4 | 4 | 4 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.sample`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `sample` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 2, analisi 3, kernel 3 |

#### Che cosa fa

Estrae un campione pseudocasuale delle righe: `n` righe, oppure una
frazione `fraction` della tabella, eventualmente per strati di una colonna.
Il campione è deterministico: stessi dati e stesso `random_state` danno
sempre le stesse righe nello stesso ordine.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `n` | intero | `100` | intero non negativo; non insieme a `fraction`; `null` non ammesso | righe del campione (senza `fraction`) |
| `fraction` | numero | assente | da 0 a 1 compresi | frazione delle righe; esclude `n` |
| `random_state` | intero | assente (seme fisso `0x9e3779b97f4a7c15`) | intero senza segno a 64 bit; si rifiuta se il campione è sempre vuoto (`n = 0` o `fraction = 0` senza `stratify_column`) | seme del generatore |
| `stratify_column` | stringa | assente | colonna leggibile come testo | colonna degli strati |

Senza strati il campione ha `min(n, righe)` righe, oppure
`round(righe · fraction)` (arrotondamento di `f64`, metà lontano da zero).

Con `stratify_column` le righe si raggruppano per il testo della cella (le
nulle formano uno strato); ogni strato di s righe contribuisce
`floor(n · s / righe)` righe, oppure `floor(s · fraction)`, ma sempre
almeno una e al più s. Il totale può quindi differire da `n`.

`n` e `fraction` insieme si rifiutano: con `fraction` il valore di `n` non
avrebbe effetto. I semi `0` e `1` danno lo stesso campione.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati.
`sorted_by` non resta. Senza strati `row_count` è noto esattamente se era
noto quello d'ingresso; con strati non è noto.

#### Righe

Filtro: ogni riga compare al più una volta.

#### Ordine

Non conserva l'ordine d'ingresso. Senza strati le righe escono nell'ordine
del rimescolamento (Fisher–Yates con un generatore xorshift a 64 bit dal
seme). Con strati escono strato per strato, nell'ordine dei testi degli
strati (prima lo strato nullo, poi in ordine lessicografico dei byte), e
dentro lo strato nell'ordine del suo rimescolamento (seme più il numero
d'ordine dello strato).

#### Errori

In validazione, `InvalidPlan`:

- `fraction` fuori da 0..=1;
- `stratify_column` assente o non leggibile come testo;
- `n` scritto insieme a `fraction`;
- `random_state` scritto senza `stratify_column` quando il campione è
  sempre vuoto (`n = 0` o `fraction = 0`): nessun seme avrebbe effetto;
- config con campi sconosciuti, `n` negativo o `null` esplicito in `n`,
  `fraction`, `random_state` o `stratify_column` (il parametro si omette).

In esecuzione:

- `Schema`: una cella di `stratify_column` che non si converte in testo;
- `ResourceLimit`: dimensione del campione non rappresentabile, o indice
  di riga oltre `u32::MAX`.

#### Limiti e deviazioni

Il generatore non è crittografico e riduce con il modulo, con una
distorsione trascurabile ma non nulla verso gli indici bassi: il campione
serve all'esplorazione, non a garanzie statistiche.

#### Complessità

Tempo O(n) per il rimescolamento e la copia delle righe scelte; con strati
O(n log g) per g strati. Memoria O(n) indici più le righe copiate.

#### Memoria

Memoria: da misura v4.

#### Esempio

Con `random_state` 42 il rimescolamento di quattro righe mette per prime la
quarta e la prima.

Passo del piano:

```json
{"out": "risultato", "op": "table.sample", "in": ["ordini"],
 "config": {"n": 2, "random_state": 42}}
```

Ingresso `ordini`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 1 | 5.5 |
| 2 | 12.0 |
| 3 | null |
| 4 | 40.25 |

Uscita `risultato`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 4 | 40.25 |
| 1 | 5.5 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.select_columns`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Tiene solo le colonne elencate in `columns`, nell'ordine in cui sono
elencate, e scarta le altre. È il contrario di
[`table.drop_columns`](#tabledrop_columns): qui un nome che non esiste è un
errore. I dati non si copiano.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | colonne dell'ingresso, almeno una, senza ripetizioni | colonne da tenere, nell'ordine d'uscita |

#### Schema

Le colonne elencate, nell'ordine di `columns`, con tipo, nullabilità e
metadati di campo; i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se non si scarta nessuna colonna (una semplice permutazione), e cade
altrimenti. Se la colonna geometrica non è fra quelle tenute il contratto
diventa tabellare.

#### Righe

1:1: stesse righe, stessi valori.

#### Ordine

Righe nell'ordine d'ingresso; colonne nell'ordine di `columns`.

#### Errori

In validazione, `InvalidPlan`:

- `columns` assente o vuota;
- un nome ripetuto o che non è una colonna dell'ingresso;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(k) sulle colonne scelte, indipendente dalle righe; nessuna memoria
per i dati.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.select_columns", "in": ["ordini"],
 "config": {"columns": ["importo", "id"]}}
```

Ingresso `ordini`:

| `id: int64` | `note: utf8` | `importo: float64` |
| --- | --- | --- |
| 1 | urgente | 5.5 |
| 2 | null | null |

Uscita `risultato`:

| `importo: float64` | `id: int64` |
| --- | --- |
| 5.5 | 1 |
| null | 2 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.semi_join`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `semi_join` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Tiene le righe di sinistra la cui chiave compare almeno una volta a
destra, e scarta le altre. Dalla destra non si prende nessuna colonna: la
destra decide solo quali righe di sinistra restano. È il complemento di
[`table.anti_join`](#tableanti_join).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | colonne della sinistra, almeno una, senza ripetizioni | colonne chiave del lato sinistro |
| `right_keys` | lista di stringhe | obbligatorio | colonne della destra, tante quante `left_keys`, senza ripetizioni | colonne chiave del lato destro, nello stesso ordine |

Le due colonne di ogni coppia hanno lo stesso tipo Arrow (timezone,
precisione e scala comprese), scelto fra `utf8`, `int64`, `uint64`,
`float64`, `bool`, `date32`, `date64`, `timestamp` di ogni unità (timezone assente o valida),
`decimal128` con scala da 0 a 38, `binary` e `dictionary<utf8>`.

#### Schema

Identico alla sinistra: stesse colonne, tipi, nullabilità, metadati e
colonna geometrica. Delle proprietà del contratto resta l'ordinamento
dichiarato (`sorted_by`) della sinistra; il conteggio delle righe non è più
noto.

#### Righe

Filtro della sinistra: ogni riga al più una volta, anche se la sua chiave
compare più volte a destra. Le chiavi si confrontano come in
[`table.join`](#tablejoin): per valore nel tipo comune, tutti i NaN uguali,
`-0.0` diverso da `0.0`. Una riga sinistra con una colonna chiave nulla non
ha mai corrispondenza e si scarta; le righe destre con una chiave nulla non
contano.

#### Ordine

Le righe tenute restano nell'ordine della sinistra.

#### Errori

In validazione, `InvalidPlan`:

- config con campi sconosciuti, `left_keys` o `right_keys` assenti;
- liste di chiavi vuote, di lunghezza diversa, con nomi ripetuti o oltre
  `max_columns`; colonna assente; tipi diversi nella coppia; tipo fuori
  dall'elenco sopra.

In esecuzione, `Schema`: una cella chiave `date32`, `date64` o `timestamp` fuori
dall'intervallo delle date rappresentabili, un `date64` non allineato al
  giorno, o un dizionario malformato.

#### Limiti e deviazioni

Nessuna conversione fra tipi di chiave, come in `table.join`. L'insieme
delle chiavi di destra usa un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n + m) atteso (insieme delle chiavi di destra, sonda della
sinistra, in parallelo da 65.536 righe sinistre); memoria O(m) per
l'insieme, più la copia delle righe tenute.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.semi_join", "in": ["ordini", "attivi"],
 "config": {"left_keys": ["cliente"], "right_keys": ["codice"]}}
```

Ingresso `ordini`:

| `id: int64` | `cliente: utf8` |
| --- | --- |
| 1 | a |
| 2 | b |
| 3 | null |
| 4 | a |

Ingresso `attivi`:

| `codice: utf8` |
| --- |
| a |
| a |
| null |

Uscita `risultato`:

| `id: int64` | `cliente: utf8` |
| --- | --- |
| 1 | a |
| 4 | a |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.sha256_hash`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `sha256_hash` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 3, kernel 4 |

#### Che cosa fa

Aggiunge una colonna con l'hash SHA-256, in esadecimale minuscolo, dei
valori di alcune colonne di ogni riga. A differenza di
[`table.md5_hash`](#tablemd5_hash) ogni valore entra delimitato dalla
propria lunghezza, insieme al nome e al tipo della colonna: nessuna
concatenazione di valori diversi dà lo stesso messaggio. Con `normalize` (default) differenze di
maiuscole e di spazi ai lati non cambiano l'hash.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | da 1 a `max_columns` colonne leggibili come testo, senza ripetizioni | colonne da cui si calcola l'hash |
| `output_column` | stringa | `sha256_hash` | nome valido | colonna d'uscita |
| `normalize` | booleano | `true` | `true`, `false` | toglie gli spazi ai lati e porta in minuscolo ogni valore |
| `null_policy` | stringa | `empty` | `empty`, `literal`, `error` | come entra una cella nulla |
| `null_literal` | stringa | `"<null>"` | al più `max_string_bytes` byte; solo con `null_policy = "literal"`; `null` non ammesso | testo di una cella nulla con `literal` |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

Il messaggio di una riga, con `f(x)` = lunghezza di x in 8 byte big-endian
seguita da x:

```text
"plenora-sha256-v1\0"
per ogni colonna, in ordine di nome:
  f(nome) f(tipo Arrow, es. "Utf8") 0x01 f(testo della cella)
```

Con `normalize` il testo passa da `trim` e `to_lowercase` (Unicode), e così
`null_literal`. Una cella nulla con `empty` vale il testo vuoto, con
`literal` vale `null_literal`, con `error` rifiuta la riga.
`null_literal` scritto con una `null_policy` diversa da `literal` si
rifiuta; se manca, con `literal` vale `<null>`.

#### Schema

La colonna d'uscita è `utf8` non nullable, 64 cifre esadecimali: si
aggiunge in coda o sostituisce al suo posto una colonna con lo stesso nome
(perdendone tipo e metadati di campo). Metadati di schema conservati;
`row_count` resta; `sorted_by` resta solo se nessuna colonna esistente è
sovrascritta.

#### Righe

1:1. Con `null_policy` `error` una sola cella nulla nelle colonne scelte fa
fallire il passo.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con ripetizioni o con più di `max_columns` colonne;
- una colonna assente o non leggibile come testo;
- `output_column` non valido; `null_literal` oltre `max_string_bytes`, o
  `null` esplicito (il parametro si omette);
- `null_literal` scritto con una `null_policy` diversa da `literal`: non
  avrebbe effetto;
- config con campi sconosciuti o `null_policy` fuori elenco.

In esecuzione:

- `DataMapping` con diagnostica per riga, solo con `null_policy` `error`:
  ogni riga con una cella nulla (`validation.required_value_missing`, sulla
  prima colonna nulla in ordine di nome); il passo non produce uscita;
- `Schema`: una cella che non si converte in testo (`binary` non UTF-8,
  data o istante fuori intervallo).

#### Limiti e deviazioni

Con `empty` una cella nulla e una vuota danno lo stesso hash, con `literal`
una cella nulla e una che contiene `null_literal`; per distinguerle
[`table.stable_fingerprint`](#tablestable_fingerprint). Nome e tipo Arrow
entrano nell'hash: rinominare una colonna o cambiarne il tipo (anche da
`utf8` a `large_utf8`) cambia l'hash. Non è un'impronta con chiave: chi
conosce i valori possibili può ricalcolarla
([`table.hmac_sha256`](#tablehmac_sha256)).

#### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Con `normalize` le prime due righe hanno lo stesso hash; la terza è
l'hash di `<null>`.

Passo del piano:

```json
{"out": "risultato", "op": "table.sha256_hash", "in": ["articoli"],
 "config": {"columns": ["codice"], "null_policy": "literal"}}
```

Ingresso `articoli`:

| `codice: utf8` |
| --- |
| A1 |
| a1 |
| null |

Uscita `risultato`:

| `codice: utf8` | `sha256_hash: utf8` |
| --- | --- |
| A1 | 58425b65410b46c41250fc521d6f76745f2d4ebb307139af96090359e5f393cb |
| a1 | 58425b65410b46c41250fc521d6f76745f2d4ebb307139af96090359e5f393cb |
| null | 5636dc6e756406d6553fdb4b6d3750aa830edc19a1ef4e2c9ddd01bdf3679534 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.sort`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `sort` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Riordina le righe secondo le colonne `columns`: decide la prima, a parità
la seconda, e così via. Le colonne e i valori non cambiano, cambia solo
l'ordine delle righe. Il confronto è sul valore nativo di ogni tipo, mai
sulla sua forma in testo.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | nomi di colonne dell'ingresso, almeno uno, senza ripetizioni, di tipo ordinabile | chiavi d'ordinamento, dalla più significativa |
| `ascending` | booleano | `true` | `true`, `false` | verso, lo stesso per tutte le chiavi |

Tipi ordinabili e come si confrontano:

- `int64`, `uint64`: interi esatti, anche oltre `2^53`;
- `float64`: ordine totale IEEE (`total_cmp`): `-0.0` prima di `0.0`, un
  NaN positivo dopo `+inf`, uno negativo prima di `-inf`;
- `utf8`: byte per byte del testo UTF-8 (nessuna collazione linguistica);
- `bool`: `false` prima di `true`;
- `date32`: giorni dall'epoca; `date64`: millisecondi dall'epoca;
  `timestamp` di ogni unità, con o senza timezone: il valore dall'epoca
  nella sua unità, cioè l'istante (la timezone non conta; due unità diverse
  si confrontano esatte, in nanosecondi);
- `decimal128`: per valore;
- `binary`: byte per byte;
- `dictionary<utf8>` (chiavi `int32`): sul testo decodificato.

Ogni altro tipo (fra gli altri `int32`, `float32`, `large_utf8`, le liste)
non è ordinabile.

#### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati di campo e di
schema. Il contratto dichiara l'uscita ordinata sulle colonne di `columns`
(`sorted_by`, con i null in coda in ascendente e in testa in discendente) e
conserva il conteggio delle righe.

#### Righe

1:1: le stesse righe, permutate.

#### Ordine

Una cella nulla (anche la voce nulla di un dizionario) viene dopo ogni
valore; `ascending: false` rovescia l'intero confronto, quindi in
discendente i null vengono per primi. Il sort è stabile: righe con le
stesse chiavi restano nell'ordine d'ingresso, in entrambi i versi.

#### Errori

In validazione, `InvalidPlan`:

- `columns` vuoto, con un nome ripetuto o non valido, o oltre il limite di
  colonne;
- una colonna di `columns` assente o di un tipo non ordinabile;
- config con campi sconosciuti.

In esecuzione:

- `Schema`: una chiave di dizionario fuori dal proprio dizionario;
- `ResourceLimit`: più di `u32::MAX` righe.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(n log n) confronti su n righe, ciascuno fino al numero di chiavi;
memoria O(n) indici più la copia dell'uscita. Da 32.768 righe il sort è un
merge sort parallelo, con la stessa permutazione del sequenziale.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.sort", "in": ["ordini"],
 "config": {"columns": ["importo"], "ascending": false}}
```

Ingresso `ordini`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 1 | 5.5 |
| 2 | null |
| 3 | 12.0 |
| 4 | 5.5 |

Uscita `risultato`:

| `id: int64` | `importo: float64` |
| --- | --- |
| 2 | null |
| 3 | 12.0 |
| 1 | 5.5 |
| 4 | 5.5 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.split_column`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `split_column` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Divide il testo della colonna `column` sul delimitatore `delimiter`, da
sinistra, e scrive le parti nelle colonne `new_columns`, una parte per
colonna. Le parti sono al più tante quante le colonne: l'ultima tiene il
resto del testo, delimitatori compresi, e nessun carattere si perde; le
colonne senza parte ricevono null.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo da dividere |
| `delimiter` | stringa | `","` | non vuota, al più `max_string_bytes` byte; solo con almeno due colonne d'uscita; `null` non ammesso | separatore, testo letterale (non regex) |
| `new_columns` | lista di stringhe | obbligatorio | da 1 a 256 nomi, senza ripetizioni, ciascuno non vuoto e al più 1024 byte | colonne d'uscita, nell'ordine delle parti |
| `max_splits` | intero | assente | da 1 a `len(new_columns) - 2`; `null` non ammesso | al più `max_splits` divisioni, cioè `max_splits + 1` parti |

Senza `max_splits` le parti sono al più `len(new_columns)`. Con
`max_splits` sono al più `max_splits + 1`, e le colonne in coda restano
null: con tre colonne e `max_splits = 1`, `"a,b,c"` dà `"a"`, `"b,c"`,
null. `max_splits` ha effetto solo se riduce le parti: un valore non
positivo, o almeno `len(new_columns) - 1`, si rifiuta.

#### Schema

Ogni colonna di `new_columns` è `utf8` nullable: se esiste già (anche
`column` stessa) si sostituisce nella sua posizione, perdendo i metadati di
campo, altrimenti si aggiunge in coda nell'ordine di `new_columns`. Le
altre colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita.

#### Righe

1:1. Una cella null dà null in tutte le colonne d'uscita; il testo vuoto dà
`""` nella prima e null nelle altre.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- `delimiter` vuoto o oltre `max_string_bytes`, o scritto con una sola
  colonna in `new_columns`;
- `max_splits` non positivo o almeno `len(new_columns) - 1`;
- `delimiter` o `max_splits` `null` espliciti: un parametro facoltativo si
  omette;
- `new_columns` assente, vuota, con più di 256 nomi, con un nome ripetuto,
  vuoto, di soli spazi o oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

Al più 256 colonne d'uscita per passo, limite interno dei kernel che il
piano non può cambiare.

#### Complessità

Tempo O(b) sui byte della colonna; memoria pari ai byte delle colonne
d'uscita (le parti si copiano).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.split_column", "in": ["t"],
 "config": {"column": "codice", "delimiter": "-", "new_columns": ["area", "resto"]}}
```

Ingresso `t`:

| `codice: utf8` |
| --- |
| MI-2024-07 |
| RM |
| null |

Uscita `risultato`:

| `codice: utf8` | `area: utf8` | `resto: utf8` |
| --- | --- | --- |
| MI-2024-07 | MI | 2024-07 |
| RM | RM | null |
| null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.stable_fingerprint`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Aggiunge a ogni riga un'impronta stabile, in esadecimale minuscolo, dei
valori di alcune colonne (di default tutte): SHA-256 o MD5 di una codifica
canonica in cui ogni valore è delimitato dalla propria lunghezza e
accompagnato da nome e tipo della colonna. I valori entrano come sono, senza
normalizzazione, e una cella nulla è distinta da una vuota: due righe hanno
la stessa impronta solo se hanno gli stessi valori.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | `[]`, cioè tutte | colonne leggibili come testo, senza ripetizioni, al più `max_columns` | colonne dell'impronta, nell'ordine scritto |
| `output_column` | stringa | `fingerprint` | nome valido | colonna d'uscita |
| `algorithm` | stringa | `sha256` | `sha256`, `md5` | funzione di hash |

Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`. Con `columns` vuoto entrano tutte le
colonne nell'ordine dello schema, e tutte devono esserlo.

La codifica di una riga, con `f(x)` = lunghezza di x in 8 byte big-endian
seguita da x:

```text
"plenora-fingerprint-v1\0"
per ogni colonna, nell'ordine di columns (o dello schema):
  f(nome) f(tipo Arrow, es. "Int64")
  poi 0x00 se la cella è nulla, altrimenti 0x01 f(testo della cella)
```

Il testo della cella è la sua resa testuale: decimali per gli interi, la
resa più corta di `f64` (`2.0` è `2`), `true`/`false`, `AAAA-MM-GG` per le
date, RFC 3339 per gli istanti.

#### Schema

La colonna d'uscita è `utf8` non nullable, 64 cifre esadecimali (32 con
`md5`): si aggiunge in coda o sostituisce al suo posto una colonna con lo
stesso nome (perdendone tipo e metadati di campo). Metadati di schema
conservati; `row_count` resta; `sorted_by` resta solo se nessuna colonna
esistente è sovrascritta.

#### Righe

1:1.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `columns` con ripetizioni o con più di `max_columns` colonne;
- una colonna assente o non leggibile come testo (con `columns` vuoto,
  una qualunque colonna dello schema);
- `columns` vuoto su un ingresso senza colonne;
- `output_column` non valido;
- config con campi sconosciuti o `algorithm` fuori elenco.

In esecuzione, `Schema`: una cella che non si converte in testo (`binary`
non UTF-8, data o istante fuori intervallo).

#### Limiti e deviazioni

L'impronta dipende da nome e tipo Arrow delle colonne: rinominare o cambiare
tipo cambia l'impronta. Con `columns` vuoto entra anche una colonna
esistente con il nome di `output_column`, prima di essere sostituita. `md5`
non è resistente alle collisioni costruite apposta. Non è un'impronta con
chiave ([`table.hmac_sha256`](#tablehmac_sha256)).

#### Complessità

Tempo O(byte delle colonne scelte), in parallelo per blocchi di righe;
memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Una cella vuota e una nulla danno impronte diverse.

Passo del piano:

```json
{"out": "risultato", "op": "table.stable_fingerprint", "in": ["clienti"],
 "config": {"columns": ["id", "nome"], "algorithm": "md5"}}
```

Ingresso `clienti`:

| `id: int64` | `nome: utf8` |
| --- | --- |
| 1 | "" |
| 1 | null |

Uscita `risultato`:

| `id: int64` | `nome: utf8` | `fingerprint: utf8` |
| --- | --- | --- |
| 1 | "" | 52e58079ed6208a36c5131ac15569f8c |
| 1 | null | 9faaee9b07aec554bd05233d0ee0fa81 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.statistics`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `statistics` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 4, kernel 5 |

#### Che cosa fa

Calcola statistiche descrittive (conteggio, minimo, massimo, somma, media,
mediana, deviazione standard, varianza, quartili) dei valori di una colonna
numerica, sull'intera tabella o per gruppi, e le scrive in colonne nuove
ripetute su ogni riga del gruppo. Le righe non si aggregano.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica | colonna dei valori |
| `group_by` | stringa | assente | colonna leggibile come testo | colonna dei gruppi; assente, un gruppo solo |
| `stats` | lista di stringhe | `["count", "min", "max", "mean", "median", "std"]` | non vuota, senza ripetizioni, fra `count`, `min`, `max`, `sum`, `mean`, `median`, `std`, `var`, `q25`, `q75` | statistiche, una colonna ciascuna nell'ordine scritto |
| `output_prefix` | stringa | `""`, cioè `<column>_` | qualsiasi | prefisso dei nomi: la colonna di `mean` è `<output_prefix>mean` |

Colonna numerica: `float64`, `int64`, `uint64`, `date32` (giorni
dall'epoca), `date64` (millisecondi dall'epoca), `timestamp` di ogni unità
(il valore nell'unità della colonna), `decimal128`,
`utf8` il cui testo è un numero (spazi ai lati ignorati, virgola decimale
ammessa). Leggibile come testo: `utf8`, `int64`, `uint64`, `float64`,
`bool`, `date32`, `date64` (allineato al giorno), `timestamp` di ogni unità, `decimal128` con scala da 0 a 38,
`binary`, `dictionary<utf8>` con chiavi `int32`.

Le statistiche, sui valori non nulli del gruppo:

- `count`: quanti sono;
- `min`, `max`, `median`, `q25`, `q75`: sui valori ordinati; i quantili
  interpolano linearmente fra i due valori vicini (posizione `q · (c - 1)`).
  `min` e `max` sulle colonne intere (`int64`, `uint64`, `date32`,
  `date64`, `timestamp`) e `decimal128` rendono la cella estrema nel tipo della
  colonna;
- `sum`, `mean`: sulle colonne intere (`int64`, `uint64`) la somma è
  esatta ed esce `int64` (oltre `int64` è un errore); `sum` su `date32`, `date64` o
  `timestamp` si rifiuta in validazione; la media su interi, date e istanti
  è la somma esatta diviso `count`; altrove somma in `f64` nell'ordine delle righe, e somma diviso
  `count`;
- `var`, `std`: varianza campionaria (divisore `count - 1`) e sua radice;
  null con meno di due valori.

I gruppi si formano sul testo della cella di `group_by`; le celle nulle
formano un gruppo. Una statistica ripetuta in `stats` si rifiuta: darebbe
due colonne con lo stesso nome
([README, «Nomi delle colonne d'uscita»](../README.md#nomi-delle-colonne-duscita)).

#### Schema

Una colonna nullable per statistica, nell'ordine di `stats`, in coda:
`int64` per `sum` su una colonna intera, il tipo della colonna per `min` e
`max` su una colonna intera o `decimal128`, `float64` altrimenti; una colonna con lo stesso nome di una esistente la sostituisce al suo
posto (perdendone tipo e metadati di campo). Le colonne d'ingresso restano.
Metadati di schema conservati; `row_count` resta; `sorted_by` resta solo se
nessuna colonna esistente è sovrascritta.

#### Righe

1:1: ogni riga riceve le statistiche del proprio gruppo. Un gruppo senza
valori non nulli ha tutte le statistiche nulle, `count` compreso.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non numerica;
- `group_by` assente o non leggibile come testo, o `null` esplicito (il
  parametro si omette);
- `stats` vuota o con una statistica ripetuta;
- una voce di `stats` fuori elenco, un nome d'uscita non valido, o config
  con campi sconosciuti.

In esecuzione, `Schema`: una cella `utf8` di `column` che non è un numero;
una cella di `group_by` che non si converte in testo. `DataMapping`: con
`sum` su una colonna intera, una somma oltre la gamma di `int64`.

#### Limiti e deviazioni

Le statistiche `float64` sono tali per contratto: i `decimal128` e il testo
si convertono arrotondando e la loro somma accumula gli errori di
arrotondamento di `f64` nell'ordine delle righe; sulle colonne intere
la media parte dalla somma esatta, varianza e deviazione dagli scarti
esatti, e mediana e
quartili interpolano i valori arrotondati ([README, «Somme intere esatte e tipi delle riduzioni»](../README.md#somme-intere-esatte-e-tipi-delle-riduzioni)). Un `NaN` nei valori entra
nei calcoli: somma, media e varianza diventano `NaN`, e nell'ordinamento di
minimo, massimo e quantili sta dopo ogni numero.

#### Complessità

Tempo O(n) per somme e varianza più O(n log n) per l'ordinamento quando
servono minimo, massimo o quantili; memoria O(n) per i valori raggruppati e
le colonne d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.statistics", "in": ["vendite"],
 "config": {"column": "importo", "group_by": "regione", "stats": ["count", "mean", "max"]}}
```

Ingresso `vendite`:

| `regione: utf8` | `importo: float64` |
| --- | --- |
| nord | 10.0 |
| nord | 20.0 |
| sud | 5.0 |
| sud | null |

Uscita `risultato`:

| `regione: utf8` | `importo: float64` | `importo_count: float64` | `importo_mean: float64` | `importo_max: float64` |
| --- | --- | --- | --- | --- |
| nord | 10.0 | 2.0 | 15.0 | 20.0 |
| nord | 20.0 | 2.0 | 15.0 | 20.0 |
| sud | 5.0 | 1.0 | 5.0 | 5.0 |
| sud | null | 1.0 | 5.0 | 5.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.string_extract`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `string_extract` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 1, analisi 1, kernel 3 |

#### Che cosa fa

Cerca l'espressione regolare `pattern` nel testo della colonna `column` ed
estrae ciò che trova in colonne `utf8`. Con gruppi con nome
(`(?P<nome>...)`) scrive una colonna per gruppo, col nome del gruppo, dal
primo match. Senza, scrive una colonna sola con il primo gruppo di cattura,
o con il match intero se il pattern non ha gruppi; con `extract_all` unisce
con `","` i valori di tutti i match.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo in cui cercare |
| `pattern` | stringa | obbligatorio | regex non vuota della sintassi del crate `regex`, al più `max_regex_bytes` byte | espressione da cercare |
| `output_column` | stringa o `null` | `null` | nome non vuoto, al più 1024 byte | colonna d'uscita senza gruppi con nome; `null` vale `<column>_extracted` |
| `extract_all` | booleano | `false` | `true`, `false` | estrae tutti i match invece del primo |

Con gruppi con nome `output_column` ed `extract_all` si rifiutano: le
colonne prendono il nome dei gruppi e si estrae il primo match.

La sintassi è quella del crate Rust `regex`: niente lookaround né
riferimenti all'indietro, classi Unicode per default (`\d` riconosce anche
le cifre non latine, come `٣`).

#### Schema

Ogni colonna prodotta è `utf8` nullable: se esiste già si sostituisce nella
sua posizione (senza i metadati di campo di prima), altrimenti si aggiunge
in coda; i gruppi con nome nell'ordine in cui compaiono nel pattern. Le
altre colonne e i metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita.

#### Righe

1:1. Danno null: la cella null, nessun match, un gruppo che nel match non
partecipa. Con `extract_all` i match in cui il primo gruppo non partecipa
si saltano; nessun valore dà null.

#### Ordine

Righe nell'ordine d'ingresso; con `extract_all` i match da sinistra a
destra, senza sovrapposizioni.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- `pattern` vuoto, oltre `max_regex_bytes`, o non valido (compresi i nomi
  di gruppo ripetuti e i pattern troppo grandi una volta compilati);
- gruppi con nome insieme a `output_column` o a `extract_all`;
- un nome d'uscita (scritto, derivato o di gruppo) oltre 1024 byte;
- config con campi sconosciuti.

In esecuzione, `ResourceLimit`: con `extract_all`, il testo unito di una
cella oltre `max_string_bytes` byte.

#### Limiti e deviazioni

Con `extract_all` la virgola separa i match anche se un valore estratto
contiene una virgola: il risultato non si può sempre ridividere senza
ambiguità.

#### Complessità

Tempo lineare nei byte della colonna per un pattern fissato (il crate
`regex` garantisce ricerca in tempo lineare); memoria pari ai byte delle
colonne d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.string_extract", "in": ["t"],
 "config": {"column": "codice", "pattern": "(?P<sede>[A-Z]{2})-(?P<numero>\\d+)"}}
```

Ingresso `t`:

| `codice: utf8` |
| --- |
| rif MI-204 |
| nessuno |
| null |

Uscita `risultato`:

| `codice: utf8` | `sede: utf8` | `numero: utf8` |
| --- | --- | --- |
| rif MI-204 | MI | 204 |
| nessuno | null | null |
| null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.string_length`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `string_length` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Scrive in una colonna `int64` la lunghezza del testo della colonna
`column`, contata in code point Unicode (non in byte e non in grafemi).
Il null dà null.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo da misurare |
| `output_column` | stringa o `null` | `null` | nome non vuoto, al più 1024 byte | colonna d'uscita; `null` vale `<column>_length` |

#### Schema

La colonna d'uscita è `int64` nullable: se esiste già si sostituisce nella
sua posizione (con il tipo nuovo e senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se l'uscita è una colonna nuova.

#### Righe

1:1; il null resta null, il testo vuoto vale 0.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- il nome d'uscita (scritto o derivato) vuoto, di soli spazi o oltre 1024
  byte;
- config con campi sconosciuti.

In esecuzione, `ResourceLimit`: una lunghezza che non sta in `int64`
(irraggiungibile con le stringhe Arrow `utf8`).

#### Limiti e deviazioni

La lunghezza è in code point: `"é"` precomposta conta 1, `e` più accento
combinante conta 2.

#### Complessità

Tempo O(b) sui byte della colonna; memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.string_length", "in": ["t"],
 "config": {"column": "nome"}}
```

Ingresso `t`:

| `nome: utf8` |
| --- |
| città |
| "" |
| null |

Uscita `risultato`:

| `nome: utf8` | `nome_length: int64` |
| --- | --- |
| città | 5 |
| "" | 0 |
| null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.string_pad`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `string_pad` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Allunga il testo della colonna `column` fino a `width` caratteri,
aggiungendo `fill_char` a sinistra (default) o a destra. Un testo lungo
già almeno `width` caratteri resta com'è, senza troncamento. I caratteri
sono code point Unicode, non byte e non grafemi.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | testo da allungare |
| `width` | intero | `5` | da 1 a `max_string_bytes` | lunghezza minima in caratteri |
| `side` | stringa | `"left"` | `left`, `right` | lato su cui si aggiunge il riempimento |
| `fill_char` | stringa | `"0"` | esattamente un code point | carattere di riempimento |
| `output_column` | stringa o `null` | `null` | nome non vuoto, al più 1024 byte | colonna d'uscita; `null` sostituisce `column` |

#### Schema

La colonna d'uscita è `utf8` nullable: se esiste già (anche `column`
stessa, il caso di default) si sostituisce nella sua posizione, perdendo i
metadati di campo, altrimenti si aggiunge in coda. Le altre colonne e i
metadati di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se l'uscita è una colonna nuova.

#### Righe

1:1; il null resta null.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non `utf8`;
- `fill_char` vuoto o di più di un code point;
- `width` oltre `max_string_bytes` (o negativo, che la config non legge);
- `width = 0`: nessun testo si allungherebbe, e `side` e `fill_char` non
  avrebbero effetto;
- `output_column` vuoto, di soli spazi o oltre 1024 byte;
- `side` fuori elenco, config con campi sconosciuti.

In esecuzione, `ResourceLimit`: un valore allungato oltre
`max_string_bytes` byte (possibile con un `fill_char` di più byte).

#### Limiti e deviazioni

La lunghezza si conta in code point: un carattere composto da lettera e
accento combinante conta due, un emoji composto conta quanti code point ha.

#### Complessità

Tempo O(b) sui byte della colonna più il riempimento; memoria pari ai
byte della colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.string_pad", "in": ["indirizzi"],
 "config": {"column": "cap", "width": 5, "fill_char": "0"}}
```

Ingresso `indirizzi`:

| `cap: utf8` |
| --- |
| 186 |
| 20121 |
| null |
| 123456 |

Uscita `risultato`:

| `cap: utf8` |
| --- |
| 00186 |
| 20121 |
| null |
| 123456 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.table_diff`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `table_diff` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 2, analisi 3, kernel 4 |

#### Che cosa fa

Confronta due versioni di una tabella, allineate su una chiave: la
sinistra è la versione vecchia, la destra la nuova. Ogni chiave riceve uno
stato: `ADDED` (solo a destra), `DELETED` (solo a sinistra), `MODIFIED`
(una colonna confrontata cambia) o `UNCHANGED`; per le righe modificate
elenca le colonne cambiate e i loro valori precedenti.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_keys` | lista di stringhe | obbligatorio | colonne della sinistra leggibili come testo, almeno una, senza ripetizioni | chiave a sinistra |
| `right_keys` | lista di stringhe | obbligatorio | colonne della destra, tante quante `left_keys`, dello stesso tipo coppia per coppia | chiave a destra |
| `compare_columns` | lista di stringhe | `[]` | colonne presenti in entrambe, dello stesso tipo, leggibili come testo | colonne confrontate; vuota vale le colonne non chiave della sinistra presenti anche a destra |
| `include_unchanged` | stringa | `"no"` | `"yes"`, `"no"` | emette anche le righe `UNCHANGED` |
| `separator` | stringa | `"#"` | al più `max_string_bytes` byte; `null` non ammesso; non con una sola colonna in `compare_columns` | separatore di `_diff_columns` e `_diff_old_values` |

Colonne leggibili come testo: i tipi di [`table.distinct`](#tabledistinct).
Due chiavi si abbinano con l'uguaglianza di `table.distinct`: un null è
uguale a un null. Due celle confrontate sono uguali se hanno lo stesso
valore (su `float64` per bit, con `-0.0` diverso da `0.0` e ogni NaN
uguale a ogni NaN; sugli altri tipi fuori da `int64`, `uint64`, `utf8`,
`bool`, per testo); un null è uguale solo a un null.

#### Schema

Le colonne di `left_keys` (nomi della sinistra), poi quelle confrontate,
tutte nullabili e con il tipo d'ingresso; poi `_diff_status` (`utf8` non
nullabile), `_diff_columns` e `_diff_old_values` (`utf8` nullabili).
Nessun metadato di campo; i metadati di schema dei due lati si fondono, e
una chiave con valori diversi è un errore. Il contratto non dichiara né
ordinamento né conteggio.

I valori di chiavi e colonne confrontate vengono dalla destra quando la
riga c'è (`ADDED`, `MODIFIED`, `UNCHANGED`), dalla sinistra per `DELETED`.
`_diff_columns` e `_diff_old_values` sono non nulli solo per `MODIFIED`: i
nomi delle colonne cambiate nell'ordine di confronto e il testo dei loro
valori a sinistra, uniti da `separator`; un valore precedente nullo si
scrive come testo vuoto.

#### Righe

Allineamento binario sulla chiave: una riga per chiave di uno dei due
lati, tranne le `UNCHANGED` con `include_unchanged: "no"`. Una chiave
ripetuta su un lato è un errore.

#### Ordine

Prima le chiavi della sinistra, nel suo ordine; poi quelle solo a destra
(`ADDED`), nell'ordine della destra.

#### Errori

In validazione, `InvalidPlan`:

- `left_keys` vuota o di lunghezza diversa da `right_keys`; una lista con
  un nome ripetuto o non valido, o oltre il limite di colonne;
- una colonna assente; una coppia di chiavi di tipi diversi; una chiave
  non leggibile come testo;
- una colonna confrontata assente da un lato, non leggibile come testo, o
  di tipi diversi fra i lati;
- `separator` oltre `max_string_bytes`, `null` esplicito (il parametro si
  omette), o scritto quando `compare_columns` elenca esattamente una
  colonna (non avrebbe effetto con nessun ingresso); con `compare_columns`
  vuota le colonne vengono dagli schemi e `separator` si accetta anche se
  se ne confronta una sola: dipende dall'ingresso;
- metadati di schema in conflitto; `include_unchanged` fuori da
  `"yes"`/`"no"`; campi sconosciuti.

In esecuzione:

- `InvalidPlan`: una chiave ripetuta nella sinistra o nella destra;
- `Schema`: una cella che non si converte in testo (date fuori
  intervallo, `binary` non UTF-8 fra i valori confrontati per testo);
- `ResourceLimit`: righe d'uscita oltre `max_rows`; colonne d'uscita oltre
  `max_columns`; più di `u32::MAX` righe; `_diff_columns` o
  `_diff_old_values` di una riga oltre `max_string_bytes` byte.

#### Limiti e deviazioni

In `_diff_old_values` un valore precedente nullo e il testo vuoto si
scrivono allo stesso modo, e un `separator` che compare nei valori rende
il testo ambiguo. La mappa delle chiavi non è contabilizzata
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata));
l'hash delle chiavi non ha seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n + m) sulle righe dei due lati (una mappa delle chiavi) per il
numero di colonne confrontate; memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

La chiave `1` è invariata e non esce; `30.0` si scrive `30` fra i valori
precedenti.

Passo del piano:

```json
{"out": "risultato", "op": "table.table_diff", "in": ["prima", "dopo"],
 "config": {"left_keys": ["id"], "right_keys": ["id"]}}
```

Ingresso `prima`:

| `id: int64` | `prezzo: float64` |
| --- | --- |
| 1 | 10.0 |
| 2 | 20.0 |
| 3 | 30.0 |

Ingresso `dopo`:

| `id: int64` | `prezzo: float64` |
| --- | --- |
| 1 | 10.0 |
| 3 | 35.0 |
| 4 | 40.0 |

Uscita `risultato`:

| `id: int64` | `prezzo: float64` | `_diff_status: utf8` | `_diff_columns: utf8` | `_diff_old_values: utf8` |
| --- | --- | --- | --- | --- |
| 2 | 20.0 | DELETED | null | null |
| 3 | 35.0 | MODIFIED | prezzo | 30 |
| 4 | 40.0 | ADDED | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.text_normalize`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `text_normalize` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 1, analisi 1, kernel 2 |

#### Che cosa fa

Normalizza il testo delle colonne `columns` con una regola sola, scelta in
`operations`: toglie gli spazi ai lati, cambia maiuscole e minuscole, toglie
gli accenti, riduce gli spazi multipli, o fa tutto insieme (`full`, il
default). Il risultato sostituisce la colonna o va in `<colonna>_norm`.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | colonne `utf8` dell'ingresso, almeno una, senza ripetizioni, al più 4096 | colonne da normalizzare |
| `operations` | stringa | `"full"` | `trim`, `lower`, `upper`, `title`, `strip_accents`, `strip_double_spaces`, `full` | la regola da applicare (una sola, nonostante il plurale) |
| `overwrite` | booleano | `true` | `true`, `false` | sostituisce la colonna; con `false` scrive `<colonna>_norm` |

Le regole, su ogni cella non nulla:

- `trim`: toglie gli spazi Unicode ai due lati;
- `lower`, `upper`: minuscole e maiuscole Unicode, che possono cambiare la
  lunghezza (`"straße"` in maiuscolo dà `"STRASSE"`);
- `title`: maiuscola la prima lettera o cifra di ogni parola e minuscole le
  altre; una parola comincia dopo ogni carattere che non è una lettera né
  una cifra (`"d'ÉCOLE"` dà `"D'École"`, `"rust_lang"` dà `"Rust_Lang"`);
- `strip_accents`: decomposizione NFKD e rimozione dei segni combinanti;
  NFKD scompone anche i caratteri di compatibilità (la legatura `ﬁ` diventa
  `fi`, lo spazio non separabile diventa uno spazio, `²` diventa `2`);
- `strip_double_spaces`: spezza sugli spazi Unicode e riunisce con uno
  spazio solo, quindi toglie anche gli spazi ai lati e trasforma tabulazioni
  e a capo in spazi;
- `full`: `trim`, poi `lower`, poi `strip_accents`, poi
  `strip_double_spaces`.

#### Schema

Con `overwrite` ogni colonna si sostituisce nella sua posizione, `utf8`
nullable, senza i metadati di campo di prima. Senza, ogni
`<colonna>_norm` è `utf8` nullable, sostituita se esiste già, altrimenti
aggiunta in coda nell'ordine di `columns`. Le altre colonne e i metadati di
schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se nessuna colonna esistente è stata sostituita (quindi mai con
`overwrite`).

#### Righe

1:1; il null resta null.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `columns` assente, vuota, con un nome ripetuto o con più di 4096 nomi;
- una colonna assente o non `utf8`;
- un nome d'uscita `<colonna>_norm` oltre 1024 byte;
- `operations` fuori elenco, config con campi sconosciuti.

In esecuzione, `ResourceLimit`: un valore normalizzato oltre
`max_string_bytes` byte.

#### Limiti e deviazioni

`strip_accents` e `full` usano NFKD, non NFD: oltre agli accenti cambiano
i caratteri di compatibilità, e un testo con legature o apici non torna
uguale.

#### Complessità

Tempo O(b) sui byte delle colonne; memoria pari ai byte delle colonne
d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.text_normalize", "in": ["t"],
 "config": {"columns": ["citta"], "overwrite": false}}
```

Ingresso `t`:

| `citta: utf8` |
| --- |
|   Forlì  |
| SAN   Donà |
| null |

Uscita `risultato`:

| `citta: utf8` | `citta_norm: utf8` |
| --- | --- |
|   Forlì  | forli |
| SAN   Donà | san dona |
| null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.timezone_convert`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `timezone_convert` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 3, analisi 4, kernel 6 |

#### Che cosa fa

Legge date e ore scritte come testo, le interpreta come ora locale di un
fuso orario di partenza e le riscrive come ora locale di un fuso di arrivo,
con un formato che può mostrare l'offset o il nome del fuso. I fusi sono
nomi IANA (`Europe/Rome`, `UTC`, `America/New_York`).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo | colonna da leggere |
| `input_format` | stringa | assente | formato `chrono` non vuoto, al più `max_string_bytes` byte; obbligatorio per un testo, rifiutato per una colonna temporale; `null` non ammesso | formato di lettura di un testo |
| `output_format` | stringa | `"%Y-%m-%d %H:%M:%S"` | formato `chrono` non vuoto, al più `max_string_bytes` byte, che scrive al più `max_string_bytes` byte per valore | formato di scrittura; ammette `%z`, `%:z`, `%Z`, `%+` |
| `source_timezone` | stringa | obbligatorio | nome IANA noto a `chrono-tz` | fuso dei valori letti |
| `target_timezone` | stringa | obbligatorio | nome IANA noto a `chrono-tz` | fuso dei valori scritti |
| `output_column` | stringa | obbligatorio | nome valido | colonna d'uscita |
| `invalid` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un valore non leggibile rifiuta sempre la riga, nessun valore avrebbe effetto |
| `ambiguous` | stringa | assente | nessuno: scritto si rifiuta, anche `null` | un'ora locale ambigua o inesistente rifiuta sempre la riga, nessun valore avrebbe effetto |

Una colonna temporale (`date32`, `timestamp` in secondi, millisecondi,
microsecondi o nanosecondi, con o senza fuso) si legge dal valore nativo,
senza `input_format` (scritto, si rifiuta): vale l'ora locale della
colonna (del suo fuso; senza fuso, il valore com'è), e una data è la sua
mezzanotte ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)). Ogni altra colonna si legge come testo, con
`input_format` obbligatorio; leggibili come testo: `utf8`, `int64`,
`uint64`, `float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>` con chiavi `int32`.

La lettura deve consumare tutto il testo della cella; un formato senza
campi orari legge una data e la pone a mezzanotte. Un valore con un
istante si converte dall'istante: una colonna `timestamp` con fuso (che
dev'essere `source_timezone`, altrimenti il piano si rifiuta) o un testo
letto con un offset (`%z`/`%:z`: l'offset letto prevale su
`source_timezone`). Ogni altro valore (testo senza offset, `date32` a
mezzanotte, `timestamp` senza fuso) è ora locale di `source_timezone`. Un'ora locale che nel fuso di partenza si ripete (il
ritorno all'ora solare) o non esiste (il passaggio all'ora legale) rifiuta
sempre la riga, come un valore non leggibile: per questo `ambiguous` e
`invalid` non si accettano.

Il testo scritto da `output_format` non può superare `max_string_bytes`
byte per valore. Il limite si controlla in validazione, esatto per campo:
il testo letterale conta per la sua lunghezza, ogni campo `strftime` per
la sua larghezza massima (anno 7 byte col segno; secolo `%C` 2, perché si
scrive solo per gli anni 0..=9999; mese, giorno, ora 2; nome del mese o del
giorno 9; offset `%z` 5, `+0530`, `%:z` 6, `+05:30`, `%::z` 9, `%:::z` 3;
frazioni `%3f`, `%6f`, `%9f` 3, 6, 9 e `%.3f`, `%.6f`, `%.9f` 4, 7, 10;
nome del fuso 32); `%Y%m` scrive al più 9 byte.

#### Schema

La colonna d'uscita è `utf8` nullable: si aggiunge in coda o sostituisce al
suo posto una colonna con lo stesso nome (perdendone tipo e metadati di
campo). Metadati di schema conservati; `row_count` resta; `sorted_by` resta
solo se nessuna colonna esistente è sovrascritta.

#### Righe

1:1. Una cella nulla dà null.

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non leggibile come testo; `input_format` assente con
  una colonna di testo, o scritto con una colonna temporale; una colonna
  `timestamp` con un fuso diverso da `source_timezone`;
- `source_timezone` o `target_timezone` non riconosciuti;
- un formato vuoto, oltre `max_string_bytes`, con un campo non
  riconosciuto, o che non si sa scrivere per un valore con fuso. La
  validazione guarda la struttura del formato, non l'offset, che è della
  cella: `%:::z` con `target_timezone` `Australia/Adelaide` si accetta, e in
  esecuzione il 1896 (+09:00) si scrive, il 2000 (+10:30) rifiuta la riga;
- `output_format` che può scrivere più di `max_string_bytes` byte per
  valore;
- `output_column` non valido;
- `invalid` o `ambiguous` scritti, con qualunque valore, anche `null`;
- config con campi sconosciuti.

In esecuzione:

- `DataMapping` con diagnostica per riga: una cella non nulla che non si
  legge (`conversion.invalid_datetime`), un'ora locale ambigua
  (`conversion.ambiguous_local_time`) o inesistente
  (`conversion.nonexistent_local_time`) nel fuso di partenza, o il cui
  offset nel fuso di arrivo è più fine di quanto `output_format` lo scriva
  (`conversion.offset_precision`: un offset ai secondi, come l'ora media
  locale prima dei fusi standard, con `%z`, `%:z`, `%#z`, `%+`; un offset
  non a ore intere con `%:::z`), che chrono arrotonderebbe e il testo
  indicherebbe un altro istante; `%::z` scrive l'offset coi secondi. Il
  passo non produce uscita;
- `DataMapping`, senza diagnostica per riga: un valore che `output_format`
  non sa scrivere (anno fuori da 0..=9999 con `%C`);
- `Schema`: una cella che non si converte in testo.

#### Limiti e deviazioni

Le regole dei fusi sono quelle della banca dati IANA inclusa in
`chrono-tz` 0.10.4: un cambio di regole successivo non si vede finché la
dipendenza non si aggiorna.

Un `target_timezone` che non ha mai un offset scrivibile con
`output_format` (`Asia/Kolkata` con `%:::z`: +05:30 e, prima, offset
locali mai a ore intere) non si rifiuta in validazione: `chrono-tz` non
espone l'elenco delle transizioni di un fuso, e un controllo su istanti di
prova non sarebbe esatto. Ogni cella non nulla si rifiuta in esecuzione
(`conversion.offset_precision`), mai con un testo arrotondato.

#### Complessità

Tempo O(n) (due letture per riga: il controllo, poi la conversione);
memoria O(n) per la colonna d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.timezone_convert", "in": ["eventi"],
 "config": {"column": "ora", "input_format": "%Y-%m-%d %H:%M", "output_format": "%Y-%m-%d %H:%M %z", "source_timezone": "Europe/Rome", "target_timezone": "UTC", "output_column": "ora_utc"}}
```

Ingresso `eventi`:

| `ora: utf8` |
| --- |
| 2024-01-15 10:00 |
| 2024-07-15 10:00 |
| null |

Uscita `risultato`:

| `ora: utf8` | `ora_utc: utf8` |
| --- | --- |
| 2024-01-15 10:00 | 2024-01-15 09:00 +0000 |
| 2024-07-15 10:00 | 2024-07-15 08:00 +0000 |
| null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.top_n`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 2, analisi 3, kernel 3 |

#### Che cosa fa

Tiene le prime `n` righe secondo l'ordinamento di [`table.sort`](#tablesort)
sulle colonne `columns`: l'uscita è quella di `table.sort` seguita dalle
prime `n` righe, calcolata senza ordinare tutto l'ingresso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `columns` | lista di stringhe | obbligatorio | come `columns` di [`table.sort`](#tablesort) | chiavi d'ordinamento, dalla più significativa |
| `n` | intero | obbligatorio | da `1` a `max_rows` | righe da tenere |
| `descending` | booleano | `false` | `true`, `false` | `true` tiene i valori più grandi |

Il verso si scrive `descending`, non `ascending`: `ascending` è un campo
sconosciuto e si rifiuta.

#### Schema

Identico all'ingresso: colonne, tipi, nullabilità e metadati. Il contratto
dichiara l'uscita ordinata sulle colonne di `columns` e un conteggio di
`min(n, righe)` quando quello d'ingresso è noto.

#### Righe

Selezione: `min(n, righe)` righe dell'ingresso.

#### Ordine

Quello di [`table.sort`](#tablesort) con `ascending = !descending`: null
dopo ogni valore in ascendente, prima di ogni valore in discendente (con
`descending: true` i null sono quindi le prime righe tenute), pareggi
nell'ordine d'ingresso. Fra righe a pari merito sul confine delle prime
`n` passano quelle che vengono prima nell'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- come [`table.sort`](#tablesort) per `columns`;
- `n` oltre `max_rows`;
- `n = 0`: l'uscita sarebbe sempre vuota, e `columns` e `descending` non
  avrebbero effetto;
- config con campi sconosciuti.

In esecuzione:

- `Schema`: una chiave di dizionario fuori dal proprio dizionario;
- `ResourceLimit`: più di `u32::MAX` righe;
- `InvalidPlan`: `n` non rappresentabile come indice della piattaforma
  (solo dove `usize` ha 32 bit).

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo O(n_righe) confronti per separare le prime `n` più O(n log n) per
ordinarle; memoria O(n_righe) indici più la copia delle `n` righe.

#### Memoria

Memoria: da misura v4.

#### Esempio

Con `descending: true` il null è la prima riga tenuta.

Passo del piano:

```json
{"out": "risultato", "op": "table.top_n", "in": ["classifica"],
 "config": {"columns": ["punti"], "n": 2, "descending": true}}
```

Ingresso `classifica`:

| `nome: utf8` | `punti: int64` |
| --- | --- |
| anna | 7 |
| bruno | 3 |
| carla | null |
| dario | 9 |

Uscita `risultato`:

| `nome: utf8` | `punti: int64` |
| --- | --- |
| carla | null |
| dario | 9 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.transpose`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `transpose` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 2, kernel 4 |

#### Che cosa fa

Traspone la tabella: le colonne diventano righe e le righe colonne. La
prima colonna d'uscita elenca i nomi delle colonne dati; ogni riga
d'ingresso diventa una colonna. Il numero di colonne d'uscita dipende dai
dati: il runner rifiuta l'operazione in validazione, e l'esempio sotto è
eseguito chiamando il kernel.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `id_column` | stringa | nessuno | nome di una colonna dell'ingresso | i suoi valori danno i nomi delle colonne d'uscita, e non si traspone |
| `output_columns` | lista di stringhe | `[]` | nomi di colonna | nomi delle colonne d'uscita, per posizione di riga |
| `type_policy` | stringa | `"reject"` | `"reject"`, `"string"`; `null` non ammesso | colonne dati di tipi diversi: rifiuto, o conversione in testo |

Nome della colonna della riga i (da 0): `output_columns[i]` se c'è;
altrimenti il testo della cella di `id_column` alla riga i, se non è
nulla; altrimenti `col_<i+1>`. Una voce di `output_columns` vuota non vale
«assente»: si rifiuta, in validazione e nel kernel. I nomi in più di
`output_columns` si ignorano. I nomi d'uscita, prima colonna compresa, sono
tutti distinti
([README, «Nomi delle colonne d'uscita»](../README.md#nomi-delle-colonne-duscita)).
`type_policy` con colonne dati tutte dello stesso tipo si accetta e non
cambia niente: dipende dall'ingresso.

#### Schema

Prima colonna: `id_column` (o `col_0` senza), `utf8` non nullabile, con i
nomi delle colonne dati (tutte tranne `id_column`, nel loro ordine). Poi
una colonna per riga d'ingresso, nullabile: del tipo comune se le colonne
dati hanno tutte lo stesso tipo Arrow, altrimenti `utf8` con
`type_policy: "string"`, con il testo delle celle (`1.0` è `"1"`, le date
`AAAA-MM-GG`). Metadati di campo e di schema non si conservano. Un
ingresso senza righe esce invariato, schema compreso.

#### Righe

Una riga per colonna dati dell'ingresso.

#### Ordine

Righe nell'ordine delle colonne dati; colonne nell'ordine delle righe
d'ingresso.

#### Errori

In validazione, il runner rifiuta sempre `table.transpose`: `InvalidPlan`
per una config con campi sconosciuti o `type_policy` o `id_column` `null`
espliciti (il parametro si omette), `output_columns` con un nome
ripetuto o non valido (anche vuoto) o oltre il limite di colonne, o
`id_column` assente; altrimenti `Unsupported` (lo schema d'uscita dipende
dai dati).

Chiamando il kernel:

- `Schema`: `id_column` assente; una cella di `id_column`, o con la
  conversione in testo una cella dati, che non si converte in testo (tipo
  non leggibile come testo, `binary` non UTF-8, date fuori intervallo);
- `InvalidPlan`: colonne dati di tipi diversi con `type_policy: "reject"`;
  una voce di `output_columns` vuota o ripetuta; un nome di colonna
  d'uscita non valido (per esempio il testo di `id_column` vuoto o di soli
  spazi) o uguale a un altro (valori ripetuti di `id_column`, una voce
  uguale al nome della prima colonna);
- `ResourceLimit`: colonne dati oltre `max_rows` o righe più una oltre
  `max_columns`; un testo oltre `max_string_bytes`.

#### Limiti e deviazioni

Nessuno oltre allo schema che dipende dai dati.

#### Complessità

Tempo e memoria O(n · c) per n righe e c colonne dati.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.transpose", "in": ["misure"],
 "config": {"id_column": "metrica"}}
```

Ingresso `misure`:

| `metrica: utf8` | `a: int64` | `b: int64` |
| --- | --- | --- |
| min | 1 | 2 |
| max | 9 | 8 |

Uscita `risultato`:

| `metrica: utf8` | `min: int64` | `max: int64` |
| --- | --- | --- |
| a | 1 | 9 |
| b | 2 | 8 |

Verifica: eseguito dal kernel (il runner rifiuta questa config in validazione, perché lo schema d'uscita dipende dai dati); l'uscita è confrontata cella per cella.

### `table.type_cast`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `type_cast` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 4, config 2, analisi 4, kernel 6 |

#### Che cosa fa

Converte la colonna `column` nel tipo `target_type`, nella stessa
posizione. Ogni cella non nulla si legge come testo e il testo si
interpreta nel tipo chiesto. Se anche una sola cella non si converte il
passo fallisce e dice quali righe (con `coerce` e `raise`, i due
equivalenti): nessuna cella diventa null in silenzio.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna temporale o leggibile come testo (sotto) | colonna da convertire |
| `target_type` | stringa | `"str"` | `str`, `int`, `float`, `bool`, `date`, `datetime`, `date32`, `timestamp_millis`, `decimal128`, `binary_utf8`, `uint64`, `dictionary_utf8` | tipo d'arrivo |
| `date_format` | stringa | `""` | formato strftime di chrono, solo con `date`, `datetime`, `date32`, `timestamp_millis` e una colonna di testo, al più `max_string_bytes` byte | formato delle date; `""` usa i formati ISO di default |
| `errors` | stringa | `"coerce"` | `coerce`, `raise`, `ignore`; non con `str`, `binary_utf8`, `dictionary_utf8`; `null` non ammesso | che cosa succede a una cella che non si converte |
| `precision` | intero | assente | da 1 a 38, obbligatorio con `decimal128` e solo lì; `null` non ammesso | cifre totali del decimale |
| `scale` | intero | assente | da 0 a `precision`, obbligatorio con `decimal128` e solo lì; `null` non ammesso | cifre dopo la virgola |
| `timezone` | stringa | assente | nome IANA (`Europe/Rome`), solo con `timestamp_millis`; `null` non ammesso | fuso dei testi senza fuso, e fuso della colonna d'uscita |

Tipi d'arrivo: `str`, `date`, `datetime` → `utf8` (**testo**: `date`
scrive `AAAA-MM-GG`, `datetime` `AAAA-MM-GGTHH:MM:SS`; i tipi temporali
Arrow sono `date32` e `timestamp_millis`); `int` → `int64`;
`uint64` → `uint64`; `float` → `float64`; `bool` → `bool`; `date32` →
`date32`; `timestamp_millis` → `timestamp(ms)`, con `timezone` se data;
`decimal128` → `decimal128(precision, scale)`; `binary_utf8` → `binary`;
`dictionary_utf8` → `dictionary<utf8>` (chiavi `int32`).

Una colonna **temporale** (`date32`; `timestamp` in secondi,
millisecondi, microsecondi o nanosecondi, con o senza fuso) si converte
dal valore nativo, senza passare dal testo e senza `date_format`
(scritto, si rifiuta) ([README, «Colonne temporali e formati di data»](../README.md#colonne-temporali-e-formati-di-data)):

- `str`, `binary_utf8`, `dictionary_utf8`: la data `AAAA-MM-GG`, l'istante
  in RFC 3339 nel fuso della colonna (senza fuso `+00:00`), con le cifre
  frazionarie che servono (`"2024-01-31T10:00:00.123+00:00"`);
- `date`, `datetime`, `date32`: la data e l'ora **locali** della colonna
  (del suo fuso; senza fuso, il valore com'è); `datetime` scrive la
  frazione di secondo quando c'è;
- `timestamp_millis`: lo stesso istante (una data è la sua mezzanotte nel
  fuso `timezone`, o in UTC); un istante con una parte sotto il
  millisecondo si rifiuta (`conversion.timestamp_precision`) invece di
  troncarla;
- `int`, `uint64`, `float`, `bool`, `decimal128`: rifiutati in validazione
  (un numero da un istante non ha un significato scritto).

Ogni altra colonna si legge come testo: `utf8`, `int64`, `uint64`,
`float64`, `bool`, `decimal128` con scala da 0 a 38, `binary`,
`dictionary<utf8>`. Il suo testo è: l'intero in decimale; il `float64`
nella forma più corta che lo rilegge (`1.0` dà `"1"`, niente esponente,
`NaN`, `inf`); `true`/`false`; il decimale con tutte le cifre della scala
(`"12.30"`); i byte di un `binary` letti come UTF-8.

Come si interpreta il testo, per tipo d'arrivo:

- `str`, `binary_utf8`, `dictionary_utf8`: il testo com'è; non falliscono;
- `int`, `uint64`: senza spazi ai lati, un intero decimale con segno
  facoltativo nel dominio del tipo; `"1.0"`, `"1e3"`, `"12.30"` falliscono;
- `float`: senza spazi ai lati, ogni virgola diventa un punto (`"1,5"` vale
  1,5), poi il parse `f64` di Rust (esponente, `NaN`, `inf` ammessi), al
  `f64` più vicino;
- `bool`: senza spazi ai lati e in minuscolo, `true`, `1`, `yes`, `si`,
  `sì`, `vero`, `t`, `y`, `s` sono vero; `false`, `0`, `no`, `falso`, `f`,
  `n` sono falso;
- `date`, `date32`: con `date_format`, quel formato; senza, i soli
  formati ISO 8601: RFC 3339 con offset, data e ora con `T` o spazio e
  frazione facoltativa, `%Y-%m-%d`. Nessun formato con giorno e mese in un
  ordine da indovinare (`31/01/2024`, `2024/01/31` servono un
  `date_format`). Gli spazi non si tolgono. Con un offset vale la data
  scritta. `date` scrive `AAAA-MM-GG`;
- `datetime`: con `date_format`, quel formato, che deve avere anche l'ora;
  senza, i formati ISO di sopra (la data sola a mezzanotte). Scrive
  `AAAA-MM-GGTHH:MM:SS` più la frazione di secondo quando c'è (tre, sei o
  nove cifre: non si tronca); con un offset vale l'ora scritta;
- `timestamp_millis`: con un offset (RFC 3339 senza `date_format`, o `%z`
  nel formato) l'istante; con `date_format`, solo quello, che deve avere
  anche l'ora. Un testo senza fuso è l'ora locale di `timezone` (un'ora
  ambigua o inesistente nel cambio d'ora fallisce), o UTC senza
  `timezone`. Una frazione sotto il millisecondo fallisce invece di
  troncarsi;
- `decimal128`: senza spazi ai lati, un segno facoltativo (`-` o `+`, uno
  solo: `"-+5"` fallisce come `"--5"`), cifre, al
  più un punto; almeno una cifra prima del punto (`".5"` fallisce, `"5."`
  no), al più `scale` cifre dopo (nessun arrotondamento: con scala 1
  `"1.50"` fallisce), al più `precision` cifre significative contando le
  `scale` cifre decimali.

Con `errors`:

- `coerce`, `raise`: prima di convertire si controllano tutte le celle; se
  una non si converte il passo fallisce con la diagnostica per riga;
- `ignore`: accettato in validazione; se una cella non si converte il passo
  fallisce alla prima, senza diagnostica per riga.

Con `str`, `binary_utf8` e `dictionary_utf8` nessuna cella può fallire:
`errors` scritto, con qualunque valore, si rifiuta.

#### Schema

`column` resta nella sua posizione con il tipo d'arrivo, nullable, senza i
metadati di campo di prima (una colonna geometrica convertita non è più
geometrica: il contratto diventa tabellare). Le altre colonne e i metadati
di schema restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato cade.

#### Righe

1:1; il null resta null.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o di un tipo che non si legge come testo né è
  temporale (`int32`, `list`, `struct`…), o `timestamp` con un fuso Arrow
  non valido;
- una colonna temporale con un target numerico o booleano, o con
  `date_format`;
- `date_format` con un target che non lo usa, oltre `max_string_bytes`, o
  con un elemento strftime non riconosciuto;
- `decimal128` senza `precision` o `scale`, o fuori da
  `1 <= precision <= 38`, `0 <= scale <= precision`; `precision` o `scale`
  con un altro target;
- `timezone` con un target diverso da `timestamp_millis`, o non un nome
  IANA;
- `errors` scritto con `str`, `binary_utf8` o `dictionary_utf8`;
- `errors`, `precision`, `scale` o `timezone` `null` espliciti: un
  parametro facoltativo si omette;
- valori fuori elenco, config con campi sconosciuti.

Le regole su `date_format`, `precision`, `scale`, `timezone` ed `errors`
le applica anche il kernel, con la stessa funzione della validazione.

In esecuzione:

- `DataMapping` con diagnostica per riga (`coerce`, `raise`): almeno una
  cella non si converte. Per ogni causa il conteggio
  (`conversion.invalid_integer`, `invalid_unsigned_integer`,
  `invalid_float`, `invalid_boolean`, `invalid_date`, `invalid_datetime`,
  `invalid_timestamp`, `timestamp_precision`, `invalid_decimal`) e i primi
  10 esempi, con l'indice
  di riga (da 0) e la colonna, mai il valore;
- `InvalidPlan` (`ignore`): una cella non si converte;
- `Schema`: una cella che non si legge come testo (un `binary` non UTF-8,
  una data fuori dall'intervallo di chrono).

#### Limiti e deviazioni

- **`coerce` non trasforma in null**: è un nome storico; qui una cella
  non convertibile è sempre un errore.
- **Arrotondamento di `float`**: un intero oltre 2^53 o un decimale
  diventa il `f64` più vicino, con perdita delle cifre basse.
- **Nomi dei target**: `date` e `datetime` producono testo, `date32` e
  `timestamp_millis` tipi temporali Arrow; i nomi restano per
  compatibilità, e un nome fuori elenco (`timestamp`, `date64`) si
  rifiuta.
- **Offset nei testi verso `date`/`datetime`**: si legge e vale l'ora
  scritta; l'istante si perde, come nel testo d'uscita che non ha fuso. Per
  tenerlo serve `timestamp_millis`.
- Il testo di un `float64` con parte decimale non diventa mai `int`: si
  arrotonda prima con un'altra operazione.

#### Complessità

Tempo O(n) sulle righe (due passate con `coerce` e `raise`: controllo e
conversione); memoria pari alla colonna d'uscita, più testi temporanei
per cella.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.type_cast", "in": ["righe"],
 "config": {"column": "importo", "target_type": "decimal128", "precision": 10, "scale": 2}}
```

Ingresso `righe`:

| `id: int64` | `importo: utf8` |
| --- | --- |
| 1 |  12.5 |
| 2 | -0.05 |
| 3 | null |

Uscita `risultato`:

| `id: int64` | `importo: decimal128(10, 2)` |
| --- | --- |
| 1 | 12.50 |
| 2 | -0.05 |
| 3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.union_distinct`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `union_distinct` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Unione insiemistica di due tabelle con lo stesso schema (`UNION` di SQL):
le righe distinte che compaiono in almeno uno dei due ingressi, ognuna una
volta. Due righe sono uguali se lo sono tutte le loro colonne.

#### Parametri

Nessuno: la config è `{}`.

Ogni colonna ha un tipo fra `utf8`, `int64`, `uint64`, `float64`, `bool`,
`date32`, `date64`, `timestamp` di ogni unità, `decimal128`, `binary` e
`dictionary<utf8>`
(chiave int32).

#### Schema

Le colonne della sinistra, con nome, tipo e metadati di campo della
sinistra; una colonna è nullable se lo è in almeno un ingresso. I metadati
di schema dei due ingressi si fondono: una chiave presente in un ingresso
solo, o con lo stesso valore, resta. La colonna geometrica è quella della
sinistra. Nessuna proprietà del contratto sopravvive.

#### Righe

Una riga per ogni riga distinta dei due ingressi, presa dalla sua prima
comparsa (prima a sinistra, poi a destra). L'uguaglianza è per valore,
colonna per colonna: null è uguale a null, tutti i NaN sono uguali fra
loro, `-0.0` è diverso da `0.0`, un `dictionary` vale il testo della sua
voce (una voce nulla è null), un `timestamp` o un `date64` il suo valore
nella sua unità (i due lati hanno lo stesso tipo: la stessa unità).

#### Ordine

Le righe di sinistra tenute, nell'ordine di sinistra, poi quelle di destra
che non comparivano prima, nell'ordine di destra.

#### Errori

In validazione, `InvalidPlan`:

- numero di colonne diverso, o nome o tipo diversi in una posizione (la
  nullabilità non conta);
- una colonna di tipo fuori dall'elenco sopra;
- metadati di schema con la stessa chiave e valori diversi;
- config non vuota.

In esecuzione, `ResourceLimit`: righe dei due ingressi insieme oltre
`max_rows` (nel runner `max_input_rows`), anche se le righe distinte sono
meno.

#### Limiti e deviazioni

Le chiavi non si contano su `max_governed_memory_bytes`
([README, «Memoria delle chiavi dei kernel in memoria non governata»](../README.md#memoria-delle-chiavi-dei-kernel-in-memoria-non-governata)),
e l'insieme usa un hash deterministico senza seme
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

#### Complessità

Tempo O(n + m) atteso; memoria O(byte delle chiavi distinte) più la copia
delle righe tenute.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.union_distinct", "in": ["a", "b"],
 "config": {}}
```

Ingresso `a`:

| `id: int64` | `tag: utf8` |
| --- | --- |
| 1 | x |
| 2 | y |
| 1 | x |
| null | z |

Ingresso `b`:

| `id: int64` | `tag: utf8` |
| --- | --- |
| 2 | y |
| 3 | x |
| null | z |

Uscita `risultato`:

| `id: int64` | `tag: utf8` |
| --- | --- |
| 1 | x |
| 2 | y |
| null | z |
| 3 | x |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.unnest`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | `unnest` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Apre una colonna struct: ogni campo dello struct diventa una colonna, con
il nome preceduto da `prefix`. Per default la colonna struct sparisce.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | nome di una colonna `struct<…>` | colonna da aprire |
| `prefix` | stringa | `""` | entro `max_string_bytes` | prefisso dei nomi delle colonne nuove |
| `drop_source` | booleano | `true` | `true`, `false` | toglie la colonna struct |

#### Schema

Le colonne dell'ingresso nel loro ordine (senza la colonna struct con
`drop_source: true`), poi in coda una colonna per campo dello struct,
nell'ordine dei campi: nome `prefix` + nome del campo, tipo del campo,
nullabile, con i metadati del campo. I metadati di schema si conservano.
Il contratto conserva il conteggio delle righe, e l'ordinamento dichiarato
solo con `drop_source: false`.

#### Righe

1:1. Dove lo struct è nullo tutte le colonne nuove sono nulle; dove è
valido portano il valore del campo (null compreso).

#### Ordine

Invariato.

#### Errori

In validazione, `InvalidPlan`:

- `column` assente o non di tipo `struct<…>`;
- `prefix` oltre `max_string_bytes`;
- colonne d'uscita oltre `max_columns`;
- un nome nuovo non valido, o uguale a una colonna che resta (o a un altro
  campo);
- campi sconosciuti.

In esecuzione, `ResourceLimit`: più di `u32::MAX` righe.

#### Limiti e deviazioni

Nessuno oltre ai limiti comuni.

#### Complessità

Tempo e memoria O(n · f) per n righe e f campi dello struct.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.unnest", "in": ["clienti"],
 "config": {"column": "indirizzo", "prefix": "ind_"}}
```

Ingresso `clienti`:

| `id: int64` | `indirizzo: struct<via: utf8, civico: int64>` |
| --- | --- |
| 1 | {via: "Roma", civico: 3} |
| 2 | null |

Uscita `risultato`:

| `id: int64` | `ind_via: utf8` | `ind_civico: int64` |
| --- | --- | --- |
| 1 | Roma | 3 |
| 2 | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.uuid_generator`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `uuid_generator` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggiunge una colonna con un UUID versione 4 casuale per riga, in forma
testuale minuscola con i trattini (36 caratteri,
`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`). I valori cambiano a ogni
esecuzione: il risultato non dipende solo dall'ingresso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `"uuid"` | nome non vuoto, al più 1024 byte | colonna d'uscita |

#### Schema

La colonna `output_column`, `utf8` non nullable: se esiste già si
sostituisce nella sua posizione (senza i metadati di campo di prima),
altrimenti si aggiunge in coda. Le altre colonne e i metadati di schema
restano.

Contratto: il conteggio delle righe resta; l'ordinamento dichiarato resta
solo se `output_column` è una colonna nuova.

#### Righe

1:1: un UUID per riga, anche per le righe con valori null.

#### Ordine

Righe nell'ordine d'ingresso.

#### Errori

In validazione, `InvalidPlan`: `output_column` vuoto, di soli spazi o
oltre 1024 byte; config con campi sconosciuti.

In esecuzione: nessun errore che dipenda dai dati.

#### Limiti e deviazioni

- **Non deterministica per contratto**: due esecuzioni dello stesso piano
  danno UUID diversi. L'esempio qui sotto confronta solo colonne, tipi e
  numero di righe.
- L'unicità è probabilistica (122 bit casuali), non verificata.

#### Complessità

Tempo O(n); memoria O(n) per la colonna d'uscita (36 byte per riga).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.uuid_generator", "in": ["t"],
 "config": {"output_column": "chiave"}}
```

Ingresso `t`:

| `id: int64` |
| --- |
| 1 |
| 2 |

Uscita `risultato`:

| `id: int64` | `chiave: utf8` |
| --- | --- |
| 1 | 9b2f4c1e-3a7d-4e2b-8f6a-0c5d1e2f3a4b |
| 2 | e1d2c3b4-a5f6-4789-9abc-def012345678 |

Verifica: eseguito dal runner come passo unico; schema e numero di righe confrontati, valori no (sono casuali per contratto).

### `table.validate_rules`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 4, config 1, analisi 4, kernel 3 |

#### Che cosa fa

Valuta un elenco di regole dichiarative su ogni riga, senza mai fallire sui
dati: una regola violata è un esito, non un errore. Con `output_mode=annotate`
aggiunge a ogni riga se è valida e quali regole ha violato; con
`output_mode=summary` restituisce una riga per regola con il numero di righe
che la violano. La gravità della regola (`error` o `warning`) decide in
quale elenco o conteggio finisce la violazione.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `rules` | lista di oggetti | obbligatorio | da 1 a 4096 regole | le regole, valutate nell'ordine scritto |
| `rules[].name` | stringa | obbligatorio | non vuoto, al più 1024 byte, unico fra le regole | nome della regola nell'uscita |
| `rules[].operator` | stringa | obbligatorio | `eq`, `ne`, `gt`, `ge`, `lt`, `le`, `isnull`, `notnull`, `regex`, `range` | condizione che la cella deve soddisfare |
| `rules[].column` | stringa | obbligatorio | colonna dell'ingresso, di un tipo ammesso dall'operatore | colonna su cui si valuta la regola |
| `rules[].value` | JSON | assente | vedi sotto; `null` vale assente | termine di confronto; una stringa vale il suo testo, un altro valore il suo testo JSON |
| `rules[].severity` | stringa | `error` | `error`, `warning` | gravità della violazione |
| `output_mode` | stringa | `annotate` | `annotate`, `summary` | forma dell'uscita |

Per operatore (colonna numerica: `int64`, `uint64`, `float64`,
`decimal128`, `date32`, `date64`, `timestamp` di ogni unità; il testo `utf8`
non è numerico
qui):

- `isnull`, `notnull`: qualunque colonna; sulla nullità logica della cella
  (anche la voce nulla di un dizionario). `value` non ammesso;
- `eq`, `ne`: colonna numerica, `utf8`, `bool` o `binary`. Su una colonna
  numerica `value` deve essere un numero (anche in una stringa) e il
  confronto è esatto nel dominio nativo, mai attraverso `f64`; su
  `float64` un `NaN` è uguale a `"NaN"`. Sulle altre si confronta il testo
  della cella con il testo di `value` (`true`, `false` per `bool`);
- `gt`, `ge`, `lt`, `le`: colonna numerica, `value` numerico, confronto
  esatto; una `date32` vale i giorni dall'epoca, un `date64` i
  millisecondi, un `timestamp` il valore nell'unità della colonna;
- `range`: colonna numerica, `value` il testo `"min,max"` (spazi ai lati
  ignorati), estremi inclusi;
- un numero che la forma esatta non tiene (più di 38 cifre significative,
  `1e-128`, `1e400`) si rifiuta in validazione, non soltanto in
  esecuzione: analisi e kernel lo leggono con lo stesso parse esatto;
- `regex`: colonna `utf8`, `value` una regex del crate `regex` al più
  `max_regex_bytes` byte; ricerca nel testo, senza `^`/`$` basta una parte.

Una cella nulla viola ogni regola tranne `isnull`. Una cella che non si
legge (un `binary` non UTF-8 sotto `eq`/`ne`) viola sia `eq` sia `ne`. Un
`NaN` nella cella o nell'estremo rende falso ogni confronto ordinato e
`range`.

#### Schema

`annotate`: le colonne dell'ingresso, poi `_valid` (`bool`), `_errors`
(`utf8`) e `_warnings` (`utf8`), non nullabili. `_valid` è falso se la riga
viola almeno una regola `error`; `_errors` e `_warnings` elencano i nomi
delle regole violate separati da `;`, nell'ordine delle regole, e sono vuoti
se non ce ne sono. Una colonna d'ingresso con lo stesso nome si sostituisce
nella sua posizione. Metadati di schema e numero di righe si conservano;
l'ordinamento dichiarato (`sorted_by`) resta se nessuna colonna è stata
sostituita.

`summary`: una tabella nuova con `name` (`utf8`), `errors` (`int64`) e
`warnings` (`int64`), non nullabili; nessuna colonna e nessun metadato di
schema dell'ingresso. Per una regola `error` conta `errors`, per una
`warning` conta `warnings`, l'altro resta 0.

#### Righe

`annotate`: 1:1, una riga d'uscita per riga d'ingresso. `summary`: una riga
per regola, anche con un ingresso vuoto (conteggi a 0): le righe le fissa
la config, e il catalogo esenta l'operazione dal fattore di espansione.

#### Ordine

`annotate`: l'ordine d'ingresso. `summary`: l'ordine delle regole in `rules`.

#### Errori

In validazione, `InvalidPlan`:

- `rules` vuoto o oltre 4096 regole; nome vuoto, oltre 1024 byte o
  ripetuto; regola senza `column`, o colonna assente;
- `value` mancante per un operatore che lo usa, o presente per `isnull` e
  `notnull`;
- tipo della colonna non ammesso dall'operatore; `value` non numerico dove
  serve un numero; `range` senza virgola o con un estremo non numerico;
  regex oltre `max_regex_bytes` o non compilabile;
- config con campi sconosciuti o valori fuori elenco.

In esecuzione: nessuno che dipenda dai valori.

#### Limiti e deviazioni

Un nome di regola può contenere `;`: in `_errors` e `_warnings` l'elenco
non si separa più senza ambiguità. Il contratto di `summary` non dichiara il
numero di righe, anche se è noto. Il runner misura l'espansione di `summary`
come per ogni unaria, righe d'uscita su righe d'ingresso, benché l'uscita
dipenda dalle regole: un ingresso vuoto la fa sempre fallire.

#### Complessità

Tempo O(n·r) su righe e regole; memoria O(n) per le colonne aggiunte in
`annotate`, O(r) in `summary`.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "table.validate_rules", "in": ["movimenti"],
 "config": {"rules": [
    {"name": "importo_positivo", "operator": "gt", "column": "importo", "value": 0},
    {"name": "codice_formato", "operator": "regex", "column": "codice", "value": "^[A-Z]{3}$", "severity": "warning"}
  ]}}
```

Ingresso `movimenti`:

| `codice: utf8` | `importo: int64` |
| --- | --- |
| ABC | 10 |
| ab1 | -5 |
| null | 7 |

Uscita `risultato`:

| `codice: utf8` | `importo: int64` | `_valid: bool` | `_errors: utf8` | `_warnings: utf8` |
| --- | --- | --- | --- | --- |
| ABC | 10 | true | "" | "" |
| ab1 | -5 | false | importo_positivo | codice_formato |
| null | 7 | true | "" | codice_formato |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `table.window_function`

| dal catalogo | |
| --- | --- |
| famiglia | tabellare, compatibile Manipola |
| alias legacy | `window_function` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | non dichiarata (tabellare) |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | nessuno |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 2, analisi 3, kernel 5 |

#### Che cosa fa

Calcola per ogni riga una funzione finestra sulla colonna `column` —
rango, somma cumulata, valore precedente o successivo, variazione
percentuale, quantile di posizione — dentro la sua partizione
(`group_by`), e la aggiunge come colonna (`float64`, o del tipo detto
sotto). Con `order_column` le righe si riordinano prima su quella colonna.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `column` | stringa | obbligatorio | colonna numerica (sotto) | colonna su cui si calcola |
| `function` | stringa | `"rank"` | `rank`, `dense_rank`, `percent_rank`, `cume_dist`, `cumsum`, `running_mean`, `cumcount`, `lag`, `lead`, `pct_change`, `ntile` | funzione |
| `group_by` | stringa | nessuno | colonna leggibile come testo | partizione; senza, una partizione sola |
| `order_column` | stringa | nessuno | colonna di tipo ordinabile | ordinamento ascendente prima del calcolo |
| `offset` | intero | `1` | da `1`; solo con `lag` e `lead`; `null` non ammesso | distanza in righe |
| `buckets` | intero | nessuno | da `1` a `max_rows`; obbligatorio con `ntile`, solo con `ntile`; `null` non ammesso | numero di gruppi di `ntile` |
| `output_column` | stringa | `<column>_<function>` | nome di colonna valido | colonna d'uscita |

Colonne numeriche: `int64`, `uint64`, `float64`, `decimal128`, `date32`,
`date64`, `timestamp` di ogni unità e `utf8` il cui testo è un numero; le funzioni di rango
(`rank`, `dense_rank`, `percent_rank`, `cume_dist`) non accettano `utf8`.
`group_by` legge i tipi di [`table.distinct`](#tabledistinct);
`order_column` quelli di [`table.sort`](#tablesort).

Funzioni, per partizione, con le righe nell'ordine descritto sotto:

- `rank`: posizione del valore della cella fra i valori non nulli della
  partizione in ordine crescente (non la posizione della riga), da 1; a
  pari merito la media delle posizioni (due valori in testa: `1.5`);
- `dense_rank`: 1, 2, 3… sui valori distinti;
- `percent_rank`: valori minori diviso (valori non nulli − 1), `0` con un
  solo valore;
- `cume_dist`: valori minori o uguali diviso valori non nulli;
- `cumsum`, `running_mean`: somma e media dei valori non nulli fin qui;
  sulle colonne intere (`int64`, `uint64`) `cumsum` è esatta ed esce
  `int64` (una somma oltre `int64` è un errore); `cumsum` su `date32`,
  `date64` o `timestamp` si rifiuta in validazione; `running_mean` su interi, date e
  istanti parte dalla somma esatta;
- `cumcount`: posizione della riga nella partizione, da 0;
- `lag`, `lead`: la cella `offset` righe prima o dopo nella partizione,
  com'è, nel tipo della colonna (un `timestamp` resta `timestamp`, nella
  sua unità, un
  `utf8` resta il suo testo);
- `pct_change`: `(corrente − precedente) / precedente` sulla riga subito
  prima (`offset` non si usa);
- `ntile`: `posizione * min(buckets, righe) / righe + 1` in divisione
  intera, con la posizione da 0.

Le funzioni di rango confrontano il valore nativo con l'ordine di
[`table.sort`](#tablesort) (su `float64`, `-0.0` prima di `0.0`);
`pct_change` sulle colonne intere sottrae esatto e arrotonda una volta; le
altre leggono la cella come `f64`.

#### Schema

L'ingresso più la colonna `output_column`, nullabile, senza metadati di
campo, in coda: del tipo di `column` con `lag` e `lead`, `int64` con
`cumsum` su una colonna intera, `float64` altrimenti; se il nome esiste già, la colonna è sostituita
al suo posto. Gli altri metadati si conservano. Il contratto dichiara
l'uscita ordinata in ascendente su `order_column` se c'è (altrimenti
conserva l'ordinamento dell'ingresso) e conserva il conteggio.

#### Righe

1:1. Null nell'uscita: sulla riga di un valore nullo per rango, `cumsum`,
`running_mean` e `pct_change`; per `lag`/`lead` quando la riga a distanza
`offset` non c'è o è nulla; per `pct_change` anche senza riga precedente,
con il precedente nullo o uguale a zero. `cumcount` e `ntile` contano
tutte le righe, null compresi.

#### Ordine

Con `order_column`, le righe escono ordinate su quella colonna come
[`table.sort`](#tablesort) in ascendente (stabile, null in coda), senza
raggrupparle per partizione; senza, nell'ordine d'ingresso. Le partizioni
si formano sul testo di `group_by` (il null è una partizione a sé) e le
loro righe sono in quest'ordine.

#### Errori

In validazione, `InvalidPlan`:

- `offset` uguale a 0, o scritto per una funzione diversa da `lag`/`lead`;
- `ntile` senza `buckets` positivo; `buckets` oltre `max_rows` o scritto
  per un'altra funzione;
- `column` assente o non numerica; una funzione di rango su `utf8`;
- `group_by` non leggibile come testo; `order_column` di tipo non
  ordinabile; un nome d'uscita non valido;
- `offset`, `buckets`, `group_by`, `order_column` o `output_column`
  `null` espliciti (un parametro facoltativo si omette);
- funzione fuori elenco, campi sconosciuti.

In esecuzione:

- `Schema`: una cella `utf8` di `column` che non è un numero (anche per
  `cumcount` e `ntile`, che non ne usano il valore); una cella di
  `group_by` che non si converte in testo; una chiave di dizionario di
  `order_column` fuori dal proprio dizionario;
- `DataMapping`: un risultato di `cumsum`, `running_mean` o `pct_change`
  non finito calcolato da valori finiti (overflow di `f64`); con `cumsum`
  su una colonna intera, una somma oltre la gamma di `int64`;
- `ResourceLimit`: più di `u32::MAX` righe con `order_column`.

#### Limiti e deviazioni

`cumsum`, `running_mean` e `pct_change` su `decimal128` e testo numerico
leggono la cella come `f64` e arrotondano senza errore; su una colonna
intera la media e la variazione arrotondano una volta sola, dopo la somma
o la differenza esatta ([README, «Somme intere esatte e tipi delle riduzioni»](../README.md#somme-intere-esatte-e-tipi-delle-riduzioni)).
Un `NaN` o un infinito già nei dati si propagano senza errore; solo
l'overflow di un calcolo su valori finiti si rifiuta. Le funzioni di rango
non arrotondano, e per questo rifiutano il testo numerico.

#### Complessità

Tempo O(n log n) per l'ordinamento su `order_column`, O(n) per le
partizioni e, per le funzioni di rango, O(p log p) per ogni partizione di p
righe; le altre funzioni O(p). Memoria O(n). Da 32.768 righe, con più di
una partizione, le partizioni si calcolano in parallelo, con lo stesso
risultato.

#### Memoria

Memoria: da misura v4.

#### Esempio

Due `10` nella squadra `a` sono a pari merito: rango `1.5`.

Passo del piano:

```json
{"out": "risultato", "op": "table.window_function", "in": ["partite"],
 "config": {"column": "punti", "function": "rank", "group_by": "squadra"}}
```

Ingresso `partite`:

| `squadra: utf8` | `punti: int64` |
| --- | --- |
| a | 10 |
| a | 30 |
| b | 5 |
| a | 10 |

Uscita `risultato`:

| `squadra: utf8` | `punti: int64` | `punti_rank: float64` |
| --- | --- | --- |
| a | 10 | 1.5 |
| a | 30 | 3.0 |
| b | 5 | 1.0 |
| a | 10 | 1.5 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

## Operazioni geografiche

### `geo.affine_transform`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `affine_transform` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Applica a ogni geometria la trasformazione affine 2D di matrice
`[a, b, xoff, d, e, yoff]`: ogni vertice `(x, y)` diventa
`(a x + b y + xoff, d x + e y + yoff)`. Tipo e struttura della geometria
non cambiano. [`geo.translate`](#geotranslate), [`geo.scale`](#geoscale) e
[`geo.rotate`](#georotate) sono casi particolari della stessa funzione.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `coefficients` | lista di numeri | obbligatorio | esattamente 6 numeri finiti | `[a, b, xoff, d, e, yoff]`, nelle unità del CRS |

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati; la
colonna geometria resta al suo posto con lo stesso CRS, le stesse
dimensioni (XY) e gli stessi tipi geometrici dichiarati. Le proprietà del
contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1: il runner chiama il kernel (`extended::affine_transform`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB (né estensione `geoarrow.wkb` né
  chiavi `plenora.geometry.*`);
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, `coefficients` assente, non
  di 6 elementi o con un valore non finito;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`ExtendedError`, che il runner porta in `PlenoraError`: `Internal` per
`ValidazioneNonConclusa` e `CalcoloNonConcluso`, `InvalidPlan` per le
altre):

- `InvalidInput`: geometria con coordinate NaN o infinite o non valida
  OGC;
- `InvalidOutput`: la geometria trasformata non è valida OGC (una matrice
  singolare che schiaccia una superficie su una retta, coordinate che
  traboccano a infinito);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: validazione OGC o
  calcolo di `geo` interrotti (non accusano l'ingresso).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Le coordinate d'uscita non si confrontano con il dominio di validità del
CRS ([README, «CRS integrati»](../README.md#crs-integrati)): una matrice
che porta la geometria fuori dal dominio non è un errore qui.

#### Precisione

Il calcolo è in `f64`, tre arrotondamenti per coordinata, senza
fusione: l'errore resta sotto pochi ulp della somma `|a x| + |b y| +
|xoff|`, cioè sotto 1 cm finché quella somma sta sotto circa `3e13` unità
del CRS, ben oltre ogni dominio dei CRS integrati. Con coefficienti e
coordinate interi (o comunque rappresentabili e con prodotti esatti) il
risultato è esatto. Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia trasformata.

#### Memoria

Memoria: da misura v4.

#### Esempio

`x' = 2 x + 10`, `y' = y - 5`: il quadrato unitario diventa un rettangolo
2 × 1.

Passo del piano:

```json
{"out": "risultato", "op": "geo.affine_transform", "in": ["lotti"],
 "config": {"coefficients": [2, 0, 10, 0, 1, -5]}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,1 0,1 1,0 1,0 0)) |
| 2 | POINT(3 4) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((10 -5,12 -5,12 -4,10 -4,10 -5)) |
| 2 | POINT(16 -1) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.area`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_area` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | misura terminale |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `float64` con l'area planare, senza segno, della
geometria di ogni riga, nelle unità del CRS al quadrato. Un poligono conta
l'esterno meno i buchi, una multi-geometria la somma delle parti; punti e
linee hanno area 0. In una `GeometryCollection` si sommano le aree dei
membri senza unirli: due poligoni sovrapposti contano la sovrapposizione
due volte.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `area` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: un'area per riga, dal kernel `operations::area`; una geometria
nulla dà una cella nulla.

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare;
- `InvalidPlan`: campi sconosciuti nella config, `output_column` vuoto o di soli spazi.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria non supera la validazione
  OGC;
- `Internal` (`ValidazioneNonConclusa`, `CalcoloNonConcluso`): la
  validazione OGC o il calcolo di `geo` vanno in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

L'area è planare, nel piano del CRS proiettato: non è l'area geodetica.
Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: l'area è la
formula dell'anello di `geo` in `f64` (anello traslato sul primo vertice
prima delle somme), senza un bilancio d'errore dichiarato rispetto alla
regola di 1 cm
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) per l'area, più la validazione OGC
dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.area", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,4 2,4 4,2 4,2 2)) |
| 2 | LINESTRING(0 0,3 4) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `area: float64` |
| --- | --- | --- |
| 1 | POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,4 2,4 4,2 4,2 2)) | 96.0 |
| 2 | LINESTRING(0 0,3 4) | 0.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.bearing`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `bearing` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS geografico |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Aggiunge una colonna `float64` con l'azimut geodetico iniziale, in gradi,
dal punto della riga al punto costante `other_wkb`: la direzione in cui
parte la geodetica, misurata in senso orario dal nord (nord 0, est 90, sud
180, ovest 270), in `[0, 360)`. Le coordinate sono longitudine (`x`) e
latitudine (`y`) in gradi; il calcolo è il problema inverso di Karney
(`geographiclib-rs`) sull'ellissoide del datum del CRS della colonna
([README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari) di un `Point`, nel CRS dell'input e dentro il suo dominio di validità | il punto d'arrivo, uguale per tutte le righe |
| `output_column` | stringa | `bearing` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi: struttura, validità OGC,
dominio del CRS (longitudine in `[-180, 180]`, latitudine in `[-90, 90]`)
e tipo (deve essere un `Point`, quello che il kernel chiede).

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: un azimut per riga. Per il contratto `other_wkb` è il secondo
operando: la riga è il punto di partenza, `other_wkb` quello d'arrivo
(`extended_algorithms::geodesic_bearing_degrees(riga, other_wkb)`). Dove
l'azimut non è definito il passo si ferma (vedi «Errori»): punti
coincidenti, partenza su un polo, geodetica più breve non unica. Una
geometria nulla dà un azimut nullo; una riga che non è un `Point` ferma il passo con un errore
(vedi «Errori»).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, coordinate NaN
  o infinite, byte in coda), non valido OGC o che non è un `Point`;
  `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non geografico; CRS
  senza l'ellissoide del datum (`ELLIPSOID_REQUIRED`); una coordinata di
  `other_wkb` fuori dal dominio lon/lat;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: la geometria della riga non è un `Point` (errore del
  runner, «tipo geometria non supportato»); dal kernel
  (`ExtendedAlgorithmError`) `InvalidInput` (coordinate non finite) e
  `InvalidGeographicCoordinate` (longitudine fuori da `[-180, 180]` o
  latitudine fuori da `[-90, 90]`) e `AzimutNonDefinito` («azimut non
  definito»): punti coincidenti (distanza geodetica nulla, anche `-180` e
  `180` alla stessa latitudine), punto della riga su un polo (latitudine
  ±90: ogni direzione è sud o nord), o destinazione sul luogo di taglio
  (latitudine opposta e longitudine quasi opposta, antipodi compresi), dove
  due geodetiche ugualmente brevi partono con azimut che, alla distanza
  della destinazione, si separano di più di 1 cm;
- `Internal`: dal kernel `ValidazioneNonConclusa` (la validazione non
  conclude) e `CalcoloNonConcluso` (panico di `geo`).

#### Limiti e deviazioni

- Dove l'azimut non è definito il passo si ferma con un errore: fino alla
  versione 1 del catalogo due punti coincidenti davano 180 e due antipodi
  0 (i valori convenzionali di `geographiclib`), e l'ellissoide era sempre
  WGS 84. `ST_Azimuth` di PostGIS rende NULL per punti coincidenti.
- Una destinazione su un polo è ammessa (l'azimut è 0 o 180); la partenza
  no.
- CRS proiettati rifiutati.
- Solo `Point` nella colonna: una `MultiPoint` ferma il passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

La regola di 1 cm riguarda gli spostamenti, e qui l'uscita è un angolo:
nessun controllo e nessun rifiuto di precisione. L'errore è quello
dell'algoritmo di Karney in `f64` sull'ellissoide del datum; per punti a
pochi millimetri l'azimut è mal condizionato (un nanometro di posizione
sono gradi di direzione) ([README, «Precisione delle operazioni
geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(1) per riga: un problema inverso geodetico. Memoria O(1).

#### Memoria

Memoria: da misura v4.

#### Esempio

L'arrivo è `POINT(0 1)`: dall'equatore verso nord, da latitudine 2 verso
sud, e da est verso ovest (poco a nord di ovest).

Passo del piano:

```json
{"out": "risultato", "op": "geo.bearing", "in": ["stazioni"],
 "config": {"other_wkb": "01010000000000000000000000000000000000f03f"}}
```

Ingresso `stazioni` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 0) |
| 2 | POINT(0 2) |
| 3 | POINT(1 1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `bearing: float64` |
| --- | --- | --- |
| 1 | POINT(0 0) | 0.0 |
| 2 | POINT(0 2) | 180.0 |
| 3 | POINT(1 1) | 270.00872642616275 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.boundary`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_boundary` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni geometria con il suo confine OGC, nella stessa colonna:

- poligoni e multi-poligoni: una `MultiLineString` con tutti gli anelli,
  per ogni poligono l'esterno e poi i buchi;
- linea aperta: il `MultiPoint` dei due estremi; linea chiusa o vuota:
  `MULTIPOINT EMPTY`;
- `MultiLineString`: il `MultiPoint` degli estremi delle linee aperte che
  compaiono un numero dispari di volte (regola mod-2);
- punti e multi-punti: `GEOMETRYCOLLECTION EMPTY`;
- `GeometryCollection`: la collezione dei confini dei membri.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`MultiPoint`,
`MultiLineString`, `GeometryCollection`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

#### Righe

1:1: il runner chiama il kernel (`operations::boundary`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. Gli anelli escono
nell'ordine d'ingresso; gli estremi di una `MultiLineString` in un ordine
deterministico per rappresentazione binaria delle coordinate (x, poi y),
non nell'ordine d'ingresso né in quello numerico per i valori negativi.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`OperationError`, che il runner porta in `PlenoraError`:
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, `Unsupported` per `PrecisionInsufficient`,
`InvalidPlan` per le altre):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `InvalidOutput`: il confine prodotto non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il confine di una `GeometryCollection`, che GEOS rifiuta, qui è la
collezione dei confini dei membri. Negli estremi di una `MultiLineString`
`-0.0` e `0.0` sono lo stesso punto.

#### Precisione

Esatta: anelli ed estremi sono coordinate d'ingresso copiate (`-0.0`
diventa `0.0` negli estremi di una `MultiLineString`, stesso valore)
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n), O(n log n) per gli estremi di una
`MultiLineString` (mappa ordinata), memoria O(n); più la validazione OGC
dell'ingresso e dell'uscita (sub-quadratica nel caso tipico, O(n²) nel
peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.boundary", "in": ["forme"],
 "config": {}}
```

Ingresso `forme` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 0,4 4,0 4,0 0)) |
| 2 | LINESTRING(0 0,2 0) |
| 3 | POINT(1 1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTILINESTRING((0 0,4 0,4 4,0 4,0 0)) |
| 2 | MULTIPOINT((0 0),(2 0)) |
| 3 | GEOMETRYCOLLECTION EMPTY |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.bounds_extractor`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_bounds_extractor` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggiunge quattro colonne `float64` con il rettangolo d'ingombro della
geometria di ogni riga: coordinate minime e massime in x e in y, nelle
unità del CRS. Per una geometria vuota il kernel non dà un rettangolo.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Aggiunge in coda, in quest'ordine, `<geometria>_minx`, `<geometria>_miny`,
`<geometria>_maxx`, `<geometria>_maxy` (`<geometria>` è il nome della
colonna geometria), `float64` nullable, senza metadati di campo. Le altre
colonne (geometria compresa), i metadati di schema e le proprietà del
contratto (`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: un rettangolo per riga, dal kernel `operations::bounds`. Una
geometria nulla dà quattro celle nulle, e anche una geometria vuota (il
kernel non ha un rettangolo da rendere).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); una delle quattro colonne esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria non supera la validazione
  OGC;
- `Internal` (`ValidazioneNonConclusa`): la validazione OGC va in panico
  dentro la barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Il catalogo chiede un CRS proiettato, anche se il rettangolo non dipende
dalla metrica. Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Esatta: i quattro valori sono coordinate d'ingresso, copiate senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) per l'ingombro, più la validazione
OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.bounds_extractor", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 1,2 3,0 0)) |
| 2 | POINT(5 6) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `geometry_minx: float64` | `geometry_miny: float64` | `geometry_maxx: float64` | `geometry_maxy: float64` |
| --- | --- | --- | --- | --- | --- |
| 1 | POLYGON((0 0,4 1,2 3,0 0)) | 0.0 | 0.0 | 4.0 | 3.0 |
| 2 | POINT(5 6) | 5.0 | 6.0 | 5.0 | 6.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.buffer`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_buffer` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni geometria con il suo buffer planare a distanza `distance`,
nelle unità del CRS: l'insieme dei punti che distano al più `distance`
dalla geometria, sempre un `MultiPolygon` (anche vuoto). Con `distance`
negativa il buffer erode: contano solo le parti areali, e punti e linee
spariscono. Con `distance` nulla l'uscita è l'unione delle parti areali
(punti e linee danno un `MultiPolygon` vuoto). Le giunzioni sono sempre
tonde; `cap` sceglie le estremità delle linee.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `distance` | numero | obbligatorio | finito, anche negativo o nullo | distanza del buffer, nelle unità del CRS |
| `cap` | stringa | `round` | `round`, `flat`, `square` | estremità delle linee: arco, taglio netto all'estremo, quadrato che sporge di `distance` |

Con `cap` `flat` i punti non hanno buffer: un ingresso di soli punti dà un
`MultiPolygon` vuoto.

#### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`MultiPolygon`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

#### Righe

1:1: il runner chiama il kernel (`operations::buffer_with_cap`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: `distance` assente o non finita, `cap` fuori elenco,
  campi sconosciuti;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`OperationError`, che il runner porta in `PlenoraError`:
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, `Unsupported` per `PrecisionInsufficient`,
`InvalidPlan` per le altre):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `PrecisionInsufficient`: la griglia di `i_overlay`, sommata al rientro
  delle direzioni intere, supererebbe metà della precisione (con 1 cm:
  `|distance|` oltre circa 5.368 km, o coordinate oltre circa `2^39` m);
- `InvalidOutput`: il buffer prodotto non supera la validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` e `i_overlay` vanno in panico dentro la barriera (il
  messaggio porta solo la forma del payload);
- `InvalidParameter`: `distance` non finita (l'analisi la rifiuta prima).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il runner passa al kernel la precisione di 1 cm a terra nelle unità del
CRS della colonna (`Precision::from_crs`, calcolata in validazione;
[README, «Operazioni geo»](../README.md#operazioni-geo), voce
«Precisione»). Non ci sono i parametri di GEOS e PostGIS per le
giunzioni (`join`, `mitre_limit`), il numero di segmenti per quarto di
cerchio (qui il passo degli archi viene dalla precisione) e il buffer da un
solo lato. Nessun controllo a posteriori del risultato contro la
definizione esatta ([README, «Limiti dichiarati»](../README.md#limiti-dichiarati),
voce «Nessun controllo a posteriori»).

#### Precisione

Entro la precisione `p` (1 cm a terra) fino a `|distance|` = 500 `p`
(5 m con 1 cm); oltre, **deviazione dichiarata**: gli archi sono corde con
freccia al più `max(p / 2, 0.001 |distance|)`, quindi il buffer può stare
verso l'interno lungo gli archi fino allo 0,1% della distanza (10 cm a
100 m), senza errore. Verso l'esterno il buffer resta entro `p / 2` da
quello esatto: la griglia di `i_overlay` (due passaggi, il buffer di `geo`
e l'unione delle parti) si controlla prima del calcolo e, se non basta, si
rifiuta con `PrecisionInsufficient`. Le componenti che la griglia
ridurrebbe a un punto non spariscono: una linea più corta di due passi
della griglia si bufferizza come il suo primo punto, un poligono minuscolo
o più sottile di due passi come il suo anello esterno
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
voci «Buffer» e «Deviazione: archi del buffer»).

#### Complessità

Dominata da `i_overlay` (offset dei contorni e unione, sulla griglia
intera); non c'è una stima asintotica dichiarata. Tempi misurati (stella e
linea da 1.000 vertici, da 1 m a 1 km) nel README, voce «Costo» di
[«Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).
Più la validazione OGC dell'ingresso e dell'uscita
([README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.buffer", "in": ["tratte"],
 "config": {"distance": 1, "cap": "flat"}}
```

Ingresso `tratte` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,10 0) |
| 2 | POINT(5 5) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOLYGON(((0 1,0 -1,10 -1,10 1,0 1))) |
| 2 | MULTIPOLYGON EMPTY |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.centroid`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_centroid` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni geometria con il suo centroide, un `Point`, nella stessa
colonna. Per le geometrie areali è il baricentro pesato per l'area, per le
lineari quello pesato per la lunghezza, per i punti la media delle
coordinate. In una collezione contano solo le parti della dimensione più
alta: un punto accanto a un poligono non sposta il centroide del poligono.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`Point`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

#### Righe

1:1: il runner chiama il kernel (`transform_geometry` con
`Operation::Centroid`) su ogni cella non nulla, in parallelo, e rimette la
geometria al suo posto; una cella nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria:

- `InvalidPlan`: la geometria d'ingresso non supera la validazione OGC, o
  è vuota (nessun centroide);
- `Internal`: la validazione OGC o il calcolo di `geo` non concludono
  (panico dentro la barriera; il messaggio porta solo la forma del
  payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine
di riga, senza diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Una geometria vuota è un errore, dove GEOS e PostGIS rendono `POINT EMPTY`.

#### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: il centroide è il
calcolo in `f64` di `geo` (ogni anello traslato sul suo primo vertice prima
delle somme), senza un bilancio d'errore dichiarato rispetto alla regola di
1 cm ([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) per il calcolo, più la validazione
OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.centroid", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 0,4 2,0 2,0 0)) |
| 2 | LINESTRING(0 0,10 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(2 1) |
| 2 | POINT(5 0) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.clean_topology`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_clean_topology` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 2, analisi 3, kernel 1 |

#### Che cosa fa

Pulisce la topologia di una tabella di poligoni già validi, riga per riga
nell'ordine della tabella (kernel `topology::clean_valid_polygon_topology`,
con la chiusura dei varchi e la regola «vince la prima riga» di Manipola):
con `fill_gaps` chiude rientranze e varchi stretti dentro ogni riga, con
`remove_overlaps` toglie a ogni riga la parte già coperta dalle righe
precedenti, così le righe non si sovrappongono più. Il runner lo esegue
su tutta la tabella insieme ([README, «Operazioni
geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `snap_tolerance` | numero | obbligatorio | finito, `>= 0`, nelle unità del CRS | raggio della chiusura morfologica di `fill_gaps` |
| `remove_overlaps` | booleano | obbligatorio | `true`, `false` | toglie a ogni riga la parte coperta dalle righe precedenti |
| `fill_gaps` | booleano | obbligatorio | `true`, `false` | chiude, dentro ogni riga, rientranze e varchi più stretti di `2 · snap_tolerance` |

`remove_overlaps` e `fill_gaps` sono obbligatori, senza valore
predefinito: cambiano la geometria delle righe, e il piano lo dice
esplicitamente. Fino alla versione 1 del catalogo erano facoltativi e il
runner, quando mancavano, usava `true` per entrambi. `fill_gaps` con
`snap_tolerance` pari a `0` non fa nulla. Con tutti e due `false` le righe
escono invariate (dopo il controllo di validità).

#### Schema

Quello dell'ingresso: stesse colonne, tipi e metadati di schema. La
colonna geometria resta al suo posto, con lo stesso nome e lo stesso CRS,
in XY; i tipi geometrici dichiarati diventano `Polygon` e `MultiPolygon`
(`exact`) e le chiavi dei tipi ereditate dal campo si tolgono. La colonna
geometria dell'uscita è sempre nullable, anche quando quella d'ingresso
non lo è: una riga coperta del tutto dalle precedenti diventa null. Le
altre colonne conservano la loro nullabilità; le proprietà del contratto
(`sorted_by`, `row_count`) restano.

#### Righe

1:1: il kernel rende un risultato per riga, nella stessa posizione.

- **Chiusura** (`fill_gaps` e `snap_tolerance > 0`): ogni riga, da sola,
  passa per un buffer di `+snap_tolerance` e poi di `-snap_tolerance`
  (estremità tonde) e diventa un `MultiPolygon`. Riempie le rientranze e i
  varchi della riga più stretti di `2 · snap_tolerance`, non i vuoti fra
  righe diverse.
- **Sovrapposizioni** (`remove_overlaps`): ogni riga perde la parte coperta
  dalle righe precedenti che la toccano, prese dopo la chiusura. Resta un
  `MultiPolygon`; una riga coperta del tutto resta senza geometria (`None`
  dal kernel). Una riga senza precedenti che la toccano resta com'è dopo
  la chiusura, senza chiusura anche nel tipo.

Il runner passa al kernel solo le geometrie non nulle, nell'ordine delle
righe, e riporta ogni risultato alla sua riga: una cella nulla resta
nulla e non conta come riga precedente; una riga che il kernel rende
senza geometria (coperta del tutto) diventa null. Gli altri attributi
restano invariati.

#### Ordine

Quello dell'ingresso, che decide anche il risultato: una riga precedente
vince sempre su una successiva nelle sovrapposizioni.

#### Errori

In validazione (analisi del contratto), `InvalidPlan`:

- config con campi sconosciuti, `snap_tolerance`, `remove_overlaps` o
  `fill_gaps` assente («missing field»), o un campo del tipo sbagliato;
- `snap_tolerance` negativa o non finita.

Sempre in validazione: `Schema` se l'ingresso non ha esattamente una
colonna geometria o la colonna non è riconoscibile come geometria WKB;
`Unsupported` se non è XY; `Crs` se il CRS non è risolto o non è
proiettato (o non ha unità lineare).

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida (la riparazione è di `geo.make_valid`), `Internal`
se la validazione non conclude.

Poi il kernel `topology::clean_valid_polygon_topology_validated`, con
errore `TopologyError` che il runner traduce così: `ValidazioneNonConclusa`
e `CalcoloNonConcluso` diventano `Internal`, `PrecisionInsufficient`
diventa `Unsupported`, le altre `InvalidPlan`. Il messaggio è quello del
kernel; un indice che vi compare conta le sole geometrie non nulle, non le
righe. Le varianti:

- `ResourceLimit`: le geometrie non nulle superano 100.000.000 o i
  vertici `MAX_CLEAN_VERTICES` (100.000.000), i limiti che il runner passa
  al kernel;
- `UnsupportedGeometry`: una geometria non è `Polygon`/`MultiPolygon`;
- `InvalidGeometry`: una geometria d'ingresso non supera la validazione
  OGC (il runner la rifiuta già alla decodifica, sopra), o la
  chiusura o la rimozione delle sovrapposizioni produce una geometria non
  valida;
- `ValidazioneNonConclusa`: una validazione OGC non ha concluso;
- `PrecisionInsufficient`: un buffer della chiusura o un overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `CalcoloNonConcluso`: un buffer o un overlay di `geo` è andato in panico.

#### Limiti e deviazioni

Gli archi della chiusura hanno freccia al più `max(p / 8, 0,001 ·
snap_tolerance)`: oltre `snap_tolerance` di 1,25 m il bordo chiuso può
rientrare fino allo 0,1% della tolleranza, senza errore ([README,
«Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Deviazione: archi del buffer»). La nullabilità dichiarata della colonna
geometria dell'uscita è sempre nullable, perché il kernel può rendere
righe senza geometria. Nessuna diagnostica per riga: il passo rende il primo
errore ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voci «Geo senza
diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Entro 1 cm a terra, diviso fra i passi. Con la chiusura: due buffer, ognuno
con archi di freccia `p / 8` (o la deviazione sopra) e griglia entro `p /
4`, e la rimozione delle sovrapposizioni (unione delle righe vicine e
differenza, due overlay in catena) con griglia entro `p / 4`: in tutto
`p`. Senza chiusura la rimozione ha mezzo centimetro. Ogni griglia è
controllata prima del calcolo; oltre, `PrecisionInsufficient`. Ogni resto
si calcola dalle righe d'ingresso vicine, non da un'unione accumulata riga
dopo riga, così gli spostamenti non si sommano lungo la tabella. Parti più
sottili di 1 cm possono sparire o fondersi senza errore; vedi [README,
«Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

#### Complessità

Chiusura: due buffer per riga, ognuno un overlay sui vertici della riga e
dei suoi archi. Sovrapposizioni: un R-tree dei rettangoli d'ingombro,
O(n log n) per `n` righe, poi per ogni riga un'unione delle precedenti
vicine e una differenza: di norma proporzionale ai vicini, nel caso
peggiore (rettangoli tutti sovrapposti) O(n²) coppie di righe. Memoria:
l'intera tabella e l'indice.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.clean_topology", "in": ["particelle"],
 "config": {"snap_tolerance": 0, "remove_overlaps": true, "fill_gaps": false}}
```

Ingresso `particelle` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |
| 2 | POLYGON((1 0,3 0,3 2,1 2,1 0)) |
| 3 | POLYGON((0.5 0.5,1.5 0.5,1.5 1.5,0.5 1.5,0.5 0.5)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |
| 2 | MULTIPOLYGON(((2 2,2 0,3 0,3 2,2 2))) |
| 3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.clip`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_clip` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Ritaglia ogni geometria della sinistra sulla maschera data dalla destra:
tutte le geometrie della destra si uniscono in una sola maschera, e ogni
riga della sinistra diventa la sua intersezione con la maschera (kernel
`topology::clip_to_mask_validated`; [README, «Operazioni
geo»](../README.md#operazioni-geo)). Lavora solo su `Polygon` e
`MultiPolygon`.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi; le colonne della destra non passano. La colonna geometria resta al
suo posto, con lo stesso nome e lo stesso CRS della sinistra (uguale a
quello della destra), in XY, ed è nullable anche quando quella della
sinistra non lo è (un ritaglio vuoto è nullo); i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

#### Righe

1:1 con la sinistra: la destra conta solo come maschera, qualunque sia il
suo numero di righe, e le sue geometrie nulle non ne fanno parte. Una riga
il cui ritaglio è vuoto (fuori dalla maschera, o con una destra senza
geometrie) resta, con la geometria nulla; una riga con la geometria
sinistra nulla resta nulla e non entra nel kernel.

#### Ordine

Quello della sinistra.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config non vuota;
  un metadato di schema presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`topology::clip_to_mask_validated`, errore `TopologyError`,
sulle geometrie già validate: restano le validazioni OGC della maschera
unita e dei ritagli), nella categoria del passo geo indicata fra
parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria, di un lato o
  dell'altro, non è `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): la maschera unita o un ritaglio non
  supera la validazione OGC;
- `PrecisionInsufficient` (`Unsupported`): la griglia di uno dei due
  overlay sposterebbe il risultato oltre la precisione (sotto,
  «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): la
  validazione OGC o un overlay di `geo` non ha concluso.

Il runner verifica che il kernel renda un risultato per ogni riga non
nulla, altrimenti `Internal`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Solo poligoni: un ritaglio che si riduce a linee o punti è vuoto, quindi
la riga resta con la geometria nulla. Nessun controllo a posteriori del risultato
contro gli ingressi ([README, «Precisione delle operazioni geografiche: 1
cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra, con
due overlay in catena: l'unione della maschera e poi l'intersezione di
ogni riga, ognuno con la griglia controllata prima del calcolo entro un
quarto di centimetro (la catena entro mezzo). Se lo spostamento a priori
supera quella quota, o le coordinate sono troppo rade per il centimetro,
`PrecisionInsufficient` e nessun calcolo. Parti più sottili di 1 cm
possono sparire o fondersi senza errore; vedi [README, «Precisione delle
operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«overlay in catena».

#### Complessità

Un overlay per l'unione della maschera, sui `m` vertici della destra, poi
per ognuna delle `n` righe un overlay con la maschera intera: di norma
O(n · (v + m) log(v + m)) con `v` i vertici della riga, perché ogni riga
si confronta con tutta la maschera. Più la validazione OGC di ingressi,
maschera e ritagli. Memoria O(m) per la maschera più i ritagli.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.clip", "in": ["lotti", "comune"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |
| 2 | POLYGON((10 10,11 10,11 11,10 11,10 10)) |

Ingresso `comune` (geometrie `geometry` in EPSG:3857):

| `parte: utf8` | `geometry: geometry` |
| --- | --- |
| sud | POLYGON((1 0,3 0,3 1,1 1,1 0)) |
| nord | POLYGON((1 1,3 1,3 2,1 2,1 1)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOLYGON(((1 2,1 0,2 0,2 2,1 2))) |
| 2 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.cluster_dbscan`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Raggruppa i punti della tabella per densità con DBSCAN e aggiunge a ogni
riga l'etichetta del suo cluster: un punto è «core» se entro `eps` (lui
compreso) ci sono almeno `min_points` punti; i cluster sono i core
collegati per densità più i punti di bordo che raggiungono; gli altri punti
sono rumore ed escono con etichetta nulla. Accetta solo geometrie `Point`.

La conversione di colonna è `cluster::dbscan_column`, che il runner
chiama su tutta la colonna e aggiunge in coda l'etichetta ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `eps` | numero | obbligatorio | finito, `> 0` | raggio del vicinato, nelle unità del CRS (distanza `<= eps`) |
| `min_points` | intero | obbligatorio | `>= 1` | punti minimi nel vicinato, il punto stesso compreso, perché sia core |
| `output_column` | stringa | `cluster_id` | nome non vuoto e libero | colonna dell'etichetta |

#### Schema

Aggiunge in coda `output_column`, `uint64` nullable. Le altre colonne, i
metadati e le proprietà del contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1. L'etichetta vale da 0 a `k - 1` per `k` cluster; è nulla per il
rumore e per le righe con geometria nulla (che non partecipano al calcolo).
I punti coincidenti contano tutti nel vicinato. Un punto di bordo
raggiungibile da due cluster va al primo che lo raggiunge.

#### Ordine

Quello d'ingresso. I cluster sono numerati nell'ordine di scoperta: si
visitano le righe per indice crescente, i vicini in ordine d'indice, e
l'espansione è in ampiezza; stesso ingresso, stesse etichette.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `eps` mancante, non finito o
  non positivo; `min_points` mancante, zero o non intero; `output_column`
  vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; `output_column` già presente;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi la conversione di colonna (messaggi del calcolo con prefisso
`geo.cluster_dbscan:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; una geometria che non è
  `Point` (il messaggio riporta la posizione della riga, non i dati);
- `Unsupported`: WKB con dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella;
- `Internal`: panico di `rstar`, invariante violata, validazione che non
  conclude.

#### Limiti e deviazioni

Solo punti: un poligono o una linea si rifiutano, senza passare dal
centroide, che non ne rappresenta la densità. Il rumore e la geometria
nulla hanno la stessa etichetta nulla. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Nessuna geometria calcolata. L'appartenenza al vicinato si decide in `f64`
come `dx² + dy² <= eps²`, senza fusione delle operazioni: un punto a
distanza pari a `eps` entro l'arrotondamento può cadere da una parte o
dall'altra
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

R-tree dei punti, O(n log n); una ricerca per raggio per coordinata
distinta, O(log n + m) con `m` i vicini trovati: O(n²) nel caso peggiore,
quando quasi tutti i punti stanno entro `eps` l'uno dall'altro. Memoria O(n)
per punti ed etichette, più i vicinati trattenuti.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.cluster_dbscan", "in": ["segnalazioni"],
 "config": {"eps": 1.5, "min_points": 2}}
```

Ingresso `segnalazioni` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 0) |
| 2 | POINT(1 0) |
| 3 | POINT(50 50) |
| 4 | POINT(0 1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `cluster_id: uint64` |
| --- | --- | --- |
| 1 | POINT(0 0) | 0 |
| 2 | POINT(1 0) | 0 |
| 3 | POINT(50 50) | null |
| 4 | POINT(0 1) | 0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.collect`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | N:1 |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 3, kernel 3 |

#### Che cosa fa

Raggruppa le righe per le colonne `group_by` e raccoglie le geometrie di
ogni gruppo in una sola, **senza unione topologica**: punti, linee o
poligoni tutti dello stesso tipo diventano il multi corrispondente, un
gruppo misto una `GeometryCollection`, un gruppo di una sola geometria
resta quella. Le geometrie nulle si saltano. Le colonne che non sono chiavi
spariscono.

Il runner forma i gruppi su tutta la tabella e per ciascuno chiama
`extensions::collect_geometries` con le geometrie del gruppo nell'ordine
delle righe ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `group_by` | lista di stringhe | obbligatorio | colonne dell'ingresso, non vuota, senza ripetizioni, diversa dalla colonna geometria, di un tipo con un ordine naturale (quelli di `table.sort`) | chiavi di gruppo |

#### Schema

Prima la colonna geometria, con nome e metadati d'ingresso (la
dichiarazione dei tipi riscritta), nullable solo se lo è quella
d'ingresso (null per un gruppo di sole geometrie null); poi le colonne `group_by`,
nell'ordine della lista, identiche all'ingresso (tipo, nullabilità,
metadati). I metadati di schema restano; nessuna proprietà del contratto
sopravvive. Tipi dichiarati, se l'ingresso li dichiara `exact` o `mixed`
con elenco: quelli d'ingresso, più il multi di `Point`, `LineString` e
`Polygon` presenti, più `GeometryCollection` salvo che l'ingresso dichiari
un solo tipo semplice.

#### Righe

Aggregazione: una riga per gruppo, con la geometria raccolta e i valori
chiave della prima riga del gruppo. Due righe stanno nello stesso gruppo
se ogni chiave è uguale per il confronto tipizzato di `table.sort`
(`compare_cells_typed` dei kernel tabellari: numeri per valore, testo per
byte, istanti per istante, `Float64` per `total_cmp`, quindi `-0.0` e
`0.0` in gruppi distinti e un NaN uguale solo a un NaN con gli stessi
bit), o è nulla in entrambe: un valore nullo è un valore di gruppo come
gli altri, distinto dal testo vuoto. Un gruppo senza geometrie non nulle dà una geometria nulla; una
tabella vuota non dà righe.

#### Ordine

I gruppi escono nell'**ordine naturale dei valori** delle chiavi, dalla
prima alla seconda a parità della prima, come `table.sort` crescente:
numeri per valore (`-10` prima di `-1`, `9` prima di `10`), testo per
byte UTF-8 (`"aaaaaaaaa"` prima di `"bbbbbbbbbb"` qualunque sia la
lunghezza), date e istanti per istante, booleani `false` prima di `true`,
`Float64` per `total_cmp`; una chiave nulla dopo i valori. Dentro una
geometria raccolta i membri seguono l'ordine delle righe. Fino alla
versione 1 del catalogo l'ordine era quello della chiave testuale di
`190c493`, con la lunghezza del valore scritta come testo in testa: un
valore di 10 caratteri prima di uno di 9, `3` prima di `-5`, e una chiave
nulla prima dei valori.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `group_by` vuota, con nomi
  vuoti, con ripetizioni o con la colonna geometria; una chiave di un tipo
  senza un ordine naturale (`is_sortable` dei kernel tabellari, lo stesso
  controllo di `table.sort`);
- `Schema`: una colonna di `group_by` non esiste; l'ingresso non ha
  esattamente una colonna geometria, o la colonna non è riconoscibile come
  WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Il confronto delle chiavi fallisce, con un errore esplicito (`Schema`),
per una chiave di dizionario fuori dal dizionario.

Il calcolo di un gruppo rifiuta una geometria che non supera la validazione
OGC (`ExtensionError::InvalidInput`) e una raccolta che non la supera
(`ExtensionError::InvalidOutput`): per esempio due poligoni che si
sovrappongono non formano un `MultiPolygon` valido. Il runner li traduce
in `InvalidPlan`; una validazione che non conclude è `Internal`.

#### Limiti e deviazioni

Nessuna unione: poligoni che si sovrappongono o si toccano lungo un lato
si rifiutano invece di fondersi (per l'unione c'è `geo.dissolve`).
Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Esatta: le geometrie si copiano senza calcolo
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(n log n) sulle righe per l'ordinamento delle chiavi e O(n) per la
raccolta, più la validazione OGC di ogni geometria
e di ogni raccolta (O(v²) nel caso peggiore sui vertici `v` del gruppo);
memoria O(n) per le geometrie raccolte.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.collect", "in": ["pozzi"],
 "config": {"group_by": ["zona"]}}
```

Ingresso `pozzi` (geometrie `geometry` in EPSG:3857):

| `zona: utf8` | `geometry: geometry` |
| --- | --- |
| a | POINT(1 1) |
| a | POINT(2 2) |
| b | POINT(5 5) |

Uscita `risultato`:

| `geometry: geometry` | `zona: utf8` |
| --- | --- |
| MULTIPOINT((1 1),(2 2)) | a |
| POINT(5 5) | b |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.concave_hull`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `concave_hull` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni geometria con un poligono concavo che ne racchiude tutte
le coordinate (vertici degli anelli interni e punti ripetuti compresi).
È l'algoritmo di `geo`, porting di `concaveman`: parte dall'inviluppo
convesso e scava verso i punti interni ogni lato più lungo di
`length_threshold`, se il punto candidato sta entro la lunghezza del lato
divisa per `concavity`. Più `concavity` è piccola, più il poligono è
concavo; molto grande, è l'inviluppo convesso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `concavity` | numero | obbligatorio | finito, maggiore di zero | concavità relativa: più piccola, più concavo |
| `length_threshold` | numero | `0` | finito, non negativo | lati più corti di così non si scavano; `0` scava ogni lato |

Il kernel (`extended::concave_hull`) riceve `length_threshold` sempre
esplicito, insieme al limite di coordinate `max_coordinates`: il runner
passa `0` quando manca (ogni lato si può scavare) e `MAX_CELL_COORDINATES`
(4 194 304) come limite.

#### Schema

Stesse colonne, tipi, nullabilità e metadati di schema. La colonna
geometria resta al suo posto con lo stesso CRS e le stesse dimensioni
(XY), ma dichiara ora il solo tipo `Polygon` (dichiarazione esatta): le
chiavi `plenora.geometry.types` e `plenora.geometry.types_declaration`
ereditate si tolgono dal campo. Le proprietà del contratto (`sorted_by`,
`row_count`) restano.

#### Righe

1:1: il runner chiama il kernel (`extended::concave_hull`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).
Una geometria senza coordinate dà un `POLYGON EMPTY`.

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, `concavity` assente, non
  finita o non positiva, `length_threshold` scritto e non finito o
  negativo;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`ExtendedError`, che il runner porta in `PlenoraError`: `Internal` per
`ValidazioneNonConclusa` e `CalcoloNonConcluso`, `ResourceLimit` per
`CoordinateLimit`, `InvalidPlan` per le altre):

- `InvalidInput`: coordinate NaN o infinite o geometria non valida OGC;
- `CoordinateLimit`: più coordinate di `max_coordinates` (il runner passa
  `MAX_CELL_COORDINATES`, 4 194 304);
- `InvalidOutput`: il poligono prodotto non è valido OGC, come per un
  punto solo, due punti distinti o punti tutti allineati;
- `CalcoloNonConcluso`: `geo` va in panico, per esempio con coordinate
  vicine al massimo di `f64`; `ValidazioneNonConclusa`: la validazione
  OGC non conclude.

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il lavoro è limitato da `max_coordinates`, un argomento del kernel e non
della config: nel runner `MAX_CELL_COORDINATES`. Il poligono non è quello
di `ST_ConcaveHull` di PostGIS, che
ha altri parametri (frazione dell'area convessa, buchi ammessi).

#### Precisione

Nessuna coordinata calcolata: i vertici del poligono sono coordinate
dell'ingresso, con i loro bit. Quali punti entrano nel bordo lo decidono
distanze e confronti in `f64` di `geo`, non predicati esatti: per punti
più vicini di 1 cm al bordo la scelta segue l'arrotondamento, il caso
fuori ambito della regola
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Nessun rifiuto `PrecisionInsufficient`.

#### Complessità

Tempo almeno O(n log n) sulle n coordinate (inviluppo convesso e R-tree
dei punti di `geo`; il costo dello scavo nel caso peggiore non è
dichiarato), più la validazione OGC di ingresso e uscita; memoria O(n).

#### Memoria

Memoria: da misura v4.

#### Esempio

Con `concavity` 1 il bordo scava fino al punto `(1.5, 1)`; con 3 il
risultato sarebbe il quadrato convesso.

Passo del piano:

```json
{"out": "risultato", "op": "geo.concave_hull", "in": ["rilievi"],
 "config": {"concavity": 1.0, "length_threshold": 0.0}}
```

Ingresso `rilievi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOINT((0 0),(2 0),(1.5 1),(2 2),(0 2)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((2 0,1.5 1,2 2,0 2,0 0,2 0)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.convex_hull`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_convex_hull` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni geometria con il suo inviluppo convesso, un `Polygon`,
nella stessa colonna: il più piccolo poligono convesso che contiene tutti i
vertici. Un inviluppo con meno di tre punti non allineati (un punto, un
segmento, punti collineari) non è un poligono valido e si rifiuta; una
geometria vuota dà `POLYGON EMPTY`.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`Polygon`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

#### Righe

1:1: il runner chiama il kernel (`transform_geometry` con
`Operation::ConvexHull`) su ogni cella non nulla, in parallelo, e rimette la
geometria al suo posto; una cella nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. L'anello
dell'inviluppo è antiorario, con il primo vertice scelto da `geo`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria:

- `InvalidPlan`: la geometria d'ingresso non supera la validazione OGC, o
  l'inviluppo è degenere (punto, segmento, punti collineari: «punti
  distinti insufficienti» sull'uscita);
- `Internal`: la validazione OGC o il calcolo di `geo` non concludono
  (panico dentro la barriera; il messaggio porta solo la forma del
  payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine
di riga, senza diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Dove GEOS e PostGIS rendono un `Point` o una `LineString` per l'inviluppo
degenere, qui c'è un errore: l'uscita dichiarata è sempre `Polygon`.
Le coordinate si dividono per il loro modulo massimo prima del calcolo e si
rimoltiplicano dopo, sempre (non solo vicino ai limiti di `f64`, dove gli
orientamenti di `geo` traboccherebbero).

#### Precisione

I vertici dell'inviluppo sono vertici dell'ingresso passati per la
divisione e la moltiplicazione per il modulo massimo: due arrotondamenti,
al più qualche ulp per coordinata, molto sotto 1 cm in ogni dominio di un
CRS reale; esatti quando il modulo massimo è una potenza di 2. Nessuna
griglia e nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: quickhull di `geo`, O(n log n) atteso e O(n²)
nel caso peggiore, più la validazione OGC dell'ingresso e dell'uscita
(sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la copia scalata delle coordinate.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.convex_hull", "in": ["siti"],
 "config": {}}
```

Ingresso `siti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOINT((0 0),(4 0),(4 4),(0 4),(2 2)) |
| 2 | LINESTRING(0 0,4 0,2 3) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((4 0,4 4,0 4,0 0,4 0)) |
| 2 | POLYGON((0 0,4 0,2 3,0 0)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.count_points_in_polygons`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_count_points_in_polygons` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 3, kernel 1 |

#### Che cosa fa

Aggiunge alla sinistra, di norma poligoni, una colonna con il numero di
geometrie della destra, di norma punti, che ogni sua geometria contiene
(kernel `analysis::count_points_in_polygons_validated`; [README,
«Operazioni geo»](../README.md#operazioni-geo)). «Contiene» è il
`contains` di `geo`: i punti sul bordo non contano, come il
`predicate="within"` di Manipola, e un punto dentro più poligoni conta in
ognuno.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `count` | nome non vuoto e libero nella sinistra | colonna aggiunta |

#### Schema

Le colonne della sinistra, invariate, più `output_column` in coda,
`uint64`, nullable solo se lo è la geometria della sinistra (null dove è
null). Le colonne della destra non passano. La colonna
geometria resta quella della sinistra, con i suoi tipi dichiarati. I
metadati di schema sono la fusione dei due lati; le proprietà del
contratto della sinistra (`sorted_by`, `row_count`) restano.

#### Righe

1:1 con la sinistra; la destra non aggiunge righe. Il kernel rende un
conteggio per ogni geometria sinistra, nella stessa posizione, 0 se non ne
contiene nessuna (anche per una geometria vuota); una geometria sinistra
nulla dà un valore nullo. Una geometria destra nulla o vuota non conta
mai. Il tipo delle geometrie non si controlla: conta ogni geometria destra
contenuta, anche una linea. Le coppie punto-poligono che il kernel
conferma sono al più il limite di righe dell'arco d'uscita
(`max_output_rows` se il passo è un output del piano, `max_rows_per_edge`
altrimenti).

#### Ordine

Quello della sinistra.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti; `output_column` vuota; un metadato di schema presente sui
  due lati con valori diversi;
- `Schema`: `output_column` esiste già nella sinistra; un lato senza
  esattamente una colonna geometria, o con una colonna non riconoscibile
  come geometria WKB (né estensione `geoarrow.wkb` né chiavi canoniche
  `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`analysis::count_points_in_polygons_validated`, errore
`AnalysisError` che avvolge `SpatialJoinError`, sulle geometrie già
validate), nella categoria del passo geo indicata fra parentesi:

- `PairLimitExceeded` (`ResourceLimit`): le coppie punto-poligono
  confermate superano il limite di righe dell'arco;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`, `Internal` (`Internal`):
  l'indice o il predicato di `geo` non ha concluso, o un'invariante
  interna violata;
- `IndexOverflow` (`InvalidPlan`): un numero di righe non entra in `u64`.

Il runner verifica che il kernel renda un conteggio per ogni riga
sinistra, altrimenti `Internal`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Il limite delle coppie conta ogni coppia punto-poligono confermata: un
punto dentro molti poligoni sovrapposti conta una coppia per poligono.

#### Precisione

Nessun calcolo di geometrie e nessuna griglia: il predicato di `geo` si
valuta sulle coordinate `f64` d'ingresso, senza tolleranza, e la regola di
1 cm non sposta nulla. Un punto a meno di 1 cm dal bordo conta o no
secondo le sue coordinate esatte ([README, «Precisione delle operazioni
geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Feature d'ingresso più vicine della precisione»).

#### Complessità

Un R-tree dei rettangoli d'ingombro della sinistra, O(n log n) per `n`
poligoni; per ognuna delle `m` geometrie destre una ricerca nell'albero e
il predicato esatto sui soli candidati. Di norma O((n + m) log n) più il
costo dei predicati; nel caso peggiore O(n · m) predicati. Più la
validazione OGC di ogni geometria. Memoria O(n) per l'albero e i
conteggi, più le coppie confermate.

#### Memoria

Memoria: da misura v4.

#### Esempio

Il terzo punto sta sul bordo del primo poligono, quindi non conta.

Passo del piano:

```json
{"out": "risultato", "op": "geo.count_points_in_polygons", "in": ["aree", "pozzi"],
 "config": {}}
```

Ingresso `aree` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |
| 2 | POLYGON((10 10,12 10,12 12,10 12,10 10)) |

Ingresso `pozzi` (geometrie `geometry` in EPSG:3857):

| `codice: utf8` | `geometry: geometry` |
| --- | --- |
| a | POINT(1 1) |
| b | POINT(0.5 0.5) |
| c | POINT(0 1) |
| d | POINT(20 20) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `count: uint64` |
| --- | --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) | 2 |
| 2 | POLYGON((10 10,12 10,12 12,10 12,10 10)) | 0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.coverage_validate`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | da tutto l'ingresso a molte righe |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Controlla che i poligoni della tabella formino una copertura senza
sovrapposizioni (le particelle di un catasto, le stanze di una pianta):
per ogni coppia di righe i cui poligoni si sovrappongono con un'area
maggiore di `tolerance` scrive una riga `overlap`, con le posizioni delle
due righe, l'area e la geometria della zona sovrapposta. I buchi fra i
poligoni (gap) non si cercano: se un buco sia atteso dipende dal dominio.

La conversione di colonna è `extensions3::coverage_validate_rows`, che il
runner chiama su tutta la colonna con i default della tabella sotto e la
precisione del CRS della colonna ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `tolerance` | numero | `0` | finito, `>= 0` | area minima, nelle unità del CRS al quadrato, perché una sovrapposizione sia segnalata (strettamente maggiore) |
| `max_issues` | intero | `1000` | `>= 1` | sovrapposizioni massime: oltre, l'operazione fallisce invece di troncare |

#### Schema

Schema nuovo, in quest'ordine: `issue_type` (`utf8`), `index_a` e
`index_b` (`uint64`), `area` (`float64`), `geometry` (`binary`, CRS della
colonna d'ingresso, dimensioni `xy`, senza dichiarazione dei tipi), tutte
non nullable. Le colonne dell'ingresso spariscono, i metadati di schema
restano; nessuna proprietà del contratto sopravvive.

#### Righe

Una riga per coppia di righe d'ingresso `(a, b)`, con `a < b`, i cui
poligoni si sovrappongono per un'area maggiore di `tolerance`:

- `issue_type`: sempre `overlap`;
- `index_a`, `index_b`: posizioni delle due righe nell'ingresso, da 0; le
  righe nulle e le geometrie vuote non partecipano ma contano nella
  posizione;
- `area`: area della sovrapposizione;
- `geometry`: la zona sovrapposta, `Polygon` se è una sola, altrimenti
  `MultiPolygon`.

Poligoni che si toccano lungo un lato o in un punto non danno righe (salvo
le schegge descritte in «Precisione»).
Un ingresso senza sovrapposizioni dà 0 righe.

#### Ordine

Per `index_a`, poi per `index_b`, crescenti.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `tolerance` negativa o non
  finita; `max_issues` pari a zero o non intero;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi la conversione di colonna (messaggi del calcolo con prefisso
`geo.coverage_validate:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; una geometria che non è
  `Polygon` o `MultiPolygon` (il messaggio riporta la posizione della riga,
  non i dati); zona sovrapposta non valida;
- `Unsupported`: `PrecisionInsufficient` (sotto, «Precisione»); WKB con
  dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella; più di
  `max_issues` sovrapposizioni (`IssueLimit`);
- `Internal`: panico di `geo`, `i_overlay` o `rstar`, validazione che non
  conclude.

#### Limiti e deviazioni

- Solo sovrapposizioni: i gap non sono rilevati.
- Oltre `max_issues` l'operazione fallisce, non tronca l'elenco.
- La decisione sull'area è presa sul risultato passato dalla griglia di
  `i_overlay` ([README, «Precisione delle operazioni geografiche: 1 cm a
  terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
  «Hazard»).
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Ogni intersezione di una coppia passa dalla griglia di `i_overlay`, con il
controllo a priori sull'ingombro della coppia: guardia di spaziatura delle
coordinate e spostamento della griglia entro `p / 2`, altrimenti
`PrecisionInsufficient`; `p` è 1 cm a terra nelle unità del CRS della
colonna (`Precision::from_crs`). Nessun controllo a posteriori: una sovrapposizione
più sottile della griglia può sparire (segnalazione mancata), e vertici
diversi su lati collineari possono lasciare una scheggia (segnalazione
spuria), con area entro la precisione per il perimetro della zona
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
`area` è calcolata in `f64` sulla zona restituita dalla griglia.

#### Complessità

R-tree sui rettangoli d'ingombro, O(n log n); poi un'intersezione per ogni
coppia di rettangoli che si toccano, O(v log v) sui vertici `v` della
coppia (O(n²) coppie nel caso peggiore, tutte sovrapposte). Memoria O(n)
per tutte le geometrie, decodificate prima del calcolo (classe bloccante).

#### Memoria

Memoria: da misura v4.

#### Esempio

Due rettangoli sovrapposti per un'area di 8 e un terzo che tocca il secondo
lungo un lato.

Passo del piano:

```json
{"out": "risultato", "op": "geo.coverage_validate", "in": ["particelle"],
 "config": {}}
```

Ingresso `particelle` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 0,4 4,0 4,0 0)) |
| 2 | POLYGON((2 0,6 0,6 4,2 4,2 0)) |
| 3 | POLYGON((6 0,8 0,8 4,6 4,6 0)) |

Uscita `risultato`:

| `issue_type: utf8` | `index_a: uint64` | `index_b: uint64` | `area: float64` | `geometry: geometry` |
| --- | --- | --- | --- | --- |
| overlap | 0 | 1 | 8.0 | POLYGON((2 4,2 0,4 0,4 4,2 4)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.delaunay`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `delaunay` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | 1:N |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Triangola i vertici di ogni geometria (triangolazione di Delaunay non
vincolata) e ne fa una riga per triangolo. I vertici sono tutte le
coordinate della geometria, anche quelle di linee e anelli, e i duplicati
contano una volta; i lati d'ingresso non vincolano la triangolazione. Ogni
triangolo è un poligono chiuso antiorario `[a, b, c, a]`.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

La colonna geometria resta al suo posto, con lo stesso nome, lo stesso CRS
e dimensioni `xy`, non nullable (una riga a geometria null non produce
triangoli); i tipi dichiarati diventano esattamente `Polygon` (le
chiavi dei tipi ereditate si tolgono dai metadati del campo). Si aggiunge in
coda `__parent_index`, `uint64` non nullable, con l'indice della riga
d'origine. Le altre colonne e i metadati di schema restano; delle proprietà
del contratto resta `sorted_by`, cade `row_count`.

#### Righe

Espansione 1:N: ogni riga dà i suoi triangoli, con le altre colonne copiate
e `__parent_index` alla riga d'origine; meno di tre punti distinti, o punti
tutti collineari, danno zero righe. Il kernel
(`extended_algorithms::delaunay`) triangola una geometria alla volta e
riceve come argomenti il massimo di coordinate d'ingresso e di triangoli.
Il runner lo chiama su ogni cella non nulla, con `MAX_CELL_COORDINATES`
come massimo di coordinate e il limite di righe dell'arco d'uscita (`max_output_rows` per un output del
piano, `max_rows_per_edge` altrimenti) come massimo di triangoli per geometria. Una cella
nulla non produce righe. `__parent_index` conta da 0. Il runner conta le righe prodotte su tutta la tabella: oltre il limite di righe dell'arco d'uscita (`max_output_rows` per un output del
piano, `max_rows_per_edge` altrimenti), `ResourceLimit`.

#### Ordine

Le righe d'origine nel loro ordine, i triangoli di ognuna consecutivi e in
ordine canonico, parte del contratto (`semantic_version` 2): ogni triangolo
parte dal suo vertice comparso per primo nell'ingresso, e i triangoli sono
in ordine lessicografico della prima comparsa dei loro tre vertici. Stesso
ingresso, stessa uscita, anche dove la triangolazione non è unica.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS, nomi):

- `InvalidPlan`: più o meno di un ingresso; config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `__parent_index` già
  presente;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi il kernel, per geometria, con errore `ExtendedAlgorithmError` che il
runner traduce così: `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso` diventano `Internal`, i limiti (`CoordinateLimit`,
`OutputLimit`, `WorkLimit`) `ResourceLimit`, le altre `InvalidPlan`. Il
kernel rifiuta la geometria con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude),
`CoordinateLimit` (coordinate d'ingresso, duplicati compresi, oltre
`MAX_CELL_COORDINATES`), `Triangulation` (coordinata non zero con modulo
fuori da `[2^-142, 2^201]`, il dominio dei predicati esatti di `spade`, per
il primo punto fuori in ordine d'ingresso; oppure vertici persi o fusi dal
caricamento in blocco), `CalcoloNonConcluso` (panico nella
triangolazione), `OutputLimit` (triangoli oltre il limite di righe
dell'arco),
`IndexOverflow`, `InvalidOutput` (triangolo non valido).

#### Limiti e deviazioni

- Triangolazione caricata in blocco con `spade`, non l'inserimento
  incrementale di `geo`: sugli ingressi con quattro o più punti
  cocircolari (griglie, reticoli) è un'altra triangolazione di Delaunay
  valida, con le stesse facce altrove; il caso peggiore resta quadratico
  ([README, «`geo.delaunay` e `geo.voronoi`: triangolazione caricata in
  blocco»](../README.md#geodelaunay-e-geovoronoi-triangolazione-caricata-in-blocco)).
- Non vincolata e senza tolleranza: `ST_DelaunayTriangles` di PostGIS ha un
  parametro di tolleranza per fondere i vertici vicini, qui assente (si
  fondono solo i punti uguali, con `-0.0` uguale a `0.0`).
- I limiti di coordinate e di triangoli sono quelli del runner, sopra; non
  si scelgono dal piano.
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Esatta: nessuna coordinata si calcola. I vertici dei triangoli sono i punti
d'ingresso con i loro bit e i predicati d'orientazione e del cerchio sono
esatti; nessun controllo e nessun rifiuto di precisione servono ([README,
«Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Due punti distinti a meno di 1 cm restano due vertici.

#### Complessità

O(n log n) tipico per geometria, con `n` le coordinate, quadratico nel caso
peggiore (punti quasi tutti allineati); più la validazione OGC
dell'ingresso e dei triangoli. Memoria O(n).

#### Memoria

Memoria: da misura v4.

#### Esempio

Quattro punti non cocircolari danno due triangoli; una linea di due punti
e un punto solo non ne danno.

Passo del piano:

```json
{"out": "risultato", "op": "geo.delaunay", "in": ["punti"],
 "config": {}}
```

Ingresso `punti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOINT((0 0),(10 0),(0 10),(12 12)) |
| 2 | LINESTRING(0 0,1 0) |
| 3 | POINT(5 5) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `__parent_index: uint64` |
| --- | --- | --- |
| 1 | POLYGON((0 0,10 0,0 10,0 0)) | 0 |
| 1 | POLYGON((10 0,12 12,0 10,10 0)) | 0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.densify`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `densify` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggiunge vertici ai lati delle geometrie: ogni lato più lungo di
`max_segment_length` si divide in parti uguali, abbastanza da non superarla.
Un lato di lunghezza `L` diventa `ceil(L / max_segment_length)` lati; i
vertici d'ingresso restano, con i loro bit. Punti e multipunti non
cambiano; le collezioni si densificano membro per membro. La lunghezza è
quella euclidea nel piano del CRS.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_segment_length` | numero | obbligatorio | finito, maggiore di zero | lunghezza massima di un lato, nelle unità del CRS |

#### Schema

Invariato: la colonna geometria si riscrive al suo posto, con lo stesso
nome, gli stessi metadati, lo stesso CRS, dimensioni `xy` e gli stessi tipi
dichiarati (la densificazione non cambia il tipo). Le altre colonne, i
metadati di schema e le proprietà del contratto (`sorted_by`, `row_count`)
restano.

#### Righe

1:1: la geometria di ogni riga diventa la sua densificata. Il runner
chiama il kernel (`extended_algorithms::densify`) su ogni cella non
nulla, in parallelo, con `MAX_CELL_COORDINATES` (4 194 304) come massimo
di coordinate d'uscita per geometria; una cella nulla resta nulla
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso. Dentro una geometria i vertici nuovi stanno fra i due
estremi del loro lato, nel verso del lato.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `max_segment_length` o con un tipo sbagliato; `max_segment_length`
  non finito o non maggiore di zero;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi il kernel rende `ExtendedAlgorithmError`, che il runner porta in
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `ResourceLimit` per `OutputLimit`, in
`InvalidPlan` per le altre. Rifiuta la geometria
con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude),
`IndexOverflow` (conteggio delle coordinate oltre `u64`), `OutputLimit`
(coordinate d'uscita stimate, prima di allocare, o contate, dopo, oltre il
limite del chiamante), `CalcoloNonConcluso` (panico di `geo`),
`InvalidOutput` (uscita non valida per l'OGC). `UnsupportedGeometry`
(`Line`, `Rect`, `Triangle`) non si raggiunge da una colonna WKB.

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

- Il limite di coordinate d'uscita è un argomento del kernel: il runner
  passa `MAX_CELL_COORDINATES` (4 194 304), non un valore della config. In
  una `GeometryCollection` vale per il totale e, di nuovo, per ogni membro.
- Solo CRS proiettati: la densificazione è planare, non lungo le
  geodetiche.
- I lati di lunghezza zero (vertici consecutivi uguali) restano come sono.

#### Precisione

Nessun controllo e nessun rifiuto di precisione. I vertici d'ingresso non
si spostano; quelli nuovi si calcolano come `inizio + (fine - inizio) * k / n`
in `f64` e stanno sul lato a meno di qualche ulp del modulo delle
coordinate (nanometri in UTM), molto sotto 1 cm in ogni dominio dei CRS
integrati ([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(n + m) per geometria, con `n` le coordinate d'ingresso e `m` quelle
d'uscita, più la validazione OGC dell'ingresso e dell'uscita (O(m²) nel
caso peggiore, [README, «Validazione OGC: la ricerca delle
auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Lati di 10 con `max_segment_length` 5: ogni lato si divide in due.

Passo del piano:

```json
{"out": "risultato", "op": "geo.densify", "in": ["confini"],
 "config": {"max_segment_length": 5}}
```

Ingresso `confini` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,10 0) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,5 0,10 0) |
| 2 | POLYGON((0 0,5 0,10 0,10 5,10 10,5 10,0 10,0 5,0 0)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.difference`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_difference` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Sostituisce la geometria della sinistra con la parte che non sta nella
geometria della destra, `sinistra \ destra`, calcolata dal kernel
`topology::boolean_operation_validated` ([README, «Operazioni
geo»](../README.md#operazioni-geo)). Lavora solo su `Polygon` e
`MultiPolygon` e rende sempre un `MultiPolygon`.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi; le colonne della destra non passano. La colonna geometria resta al
suo posto, con lo stesso nome e lo stesso CRS della sinistra (uguale a
quello della destra), in XY, ed è nullable anche quando quella della
sinistra non lo è (un risultato vuoto è nullo); i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

#### Righe

Allineate: la riga `i` della sinistra con la riga `i` della destra, e le
due tabelle devono avere le stesse righe (altrimenti `InvalidPlan`, in
esecuzione: le righe non si conoscono a secco). Una riga d'uscita per riga
della sinistra, con la geometria nulla dove una delle due è nulla o il
risultato è vuoto. Dove la destra copre tutta la sinistra il kernel rende
un `MultiPolygon` vuoto e la riga resta con la geometria nulla.

#### Ordine

Quello della sinistra.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config non vuota;
  un metadato di schema presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte);
- `InvalidPlan`: le due tabelle hanno un numero di righe diverso.

Dal kernel (`topology::boolean_operation_validated`, errore
`TopologyError`, sulle geometrie già validate: resta la validazione OGC
del risultato), nella categoria del passo geo indicata fra parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): il risultato non supera la
  validazione OGC;
- `PrecisionInsufficient` (`Unsupported`): la griglia dell'overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): la
  validazione OGC o l'overlay di `geo` non ha concluso.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Solo poligoni: una destra lineare o puntuale si rifiuta
(`UnsupportedGeometry`), dove GEOS e PostGIS renderebbero la sinistra
invariata. Nessun controllo a posteriori del risultato contro gli ingressi
([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra: un
solo overlay di `i_overlay` su interi `i64`, con la griglia controllata
prima del calcolo sull'ingombro dei due operandi. Se lo spostamento a
priori supera mezzo centimetro, o le coordinate sono troppo rade per il
centimetro, `PrecisionInsufficient` e nessun calcolo. Parti più sottili di
1 cm possono sparire o fondersi senza errore; vedi [README, «Precisione
delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

#### Complessità

Per coppia l'overlay a scansione di `i_overlay`, di norma O((v + k) log v)
con `v` i vertici dei due operandi e `k` gli incroci fra i lati, più la
validazione OGC di ingressi e risultato (di norma O(v log v), nel caso
peggiore O(v²): [README, «Validazione OGC: la
ricerca delle auto-intersezioni non è quella di `geo`, il verdetto
sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(v + k).

#### Memoria

Memoria: da misura v4.

#### Esempio

Una riga per lato: la riga `i` della sinistra con la riga `i` della destra.

Passo del piano:

```json
{"out": "risultato", "op": "geo.difference", "in": ["lotti", "vincoli"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |

Ingresso `vincoli` (geometrie `geometry` in EPSG:3857):

| `zona: utf8` | `geometry: geometry` |
| --- | --- |
| A | POLYGON((1 0,3 0,3 2,1 2,1 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOLYGON(((0 2,0 0,1 0,1 2,0 2))) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.dissolve`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_dissolve` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | N:1 |
| determinismo | ordine canonico dei valori |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Unisce tutte le geometrie della tabella in una sola (kernel
`topology::dissolve`, `unary_union` di `geo`): le parti che si toccano o
si sovrappongono si fondono, quelle disgiunte restano poligoni distinti
dello stesso `MultiPolygon`. Lavora solo su `Polygon` e `MultiPolygon`. Le
colonne attributo non passano e non ci sono gruppi.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Una sola colonna: la colonna geometria dell'ingresso, con lo stesso nome e
lo stesso CRS, `Binary` GeoArrow-WKB, nullable, in XY. I tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono; gli altri metadati di campo e i metadati
di schema restano. Le proprietà del contratto (`sorted_by`, `row_count`)
si perdono.

#### Righe

Aggregazione: sempre una riga. Il runner salta le celle nulle e passa al
kernel le altre geometrie, nell'ordine delle righe. Senza geometrie non
nulle (tabella vuota o tutta nulla) il kernel non si chiama e la riga ha
la geometria nulla, come l'analisi dichiara.

#### Ordine

Una riga sola. Le parti del `MultiPolygon` e i loro vertici sono
nell'ordine che `i_overlay` produce, lo stesso a ogni esecuzione.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: CRS non risolto, o non proiettato (o senza unità lineare).

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel `topology::dissolve_validated`, con errore `TopologyError`
che il runner traduce così: `ValidazioneNonConclusa` e
`CalcoloNonConcluso` diventano `Internal`, `PrecisionInsufficient`
diventa `Unsupported`, le altre `InvalidPlan`:

- `UnsupportedGeometry`: una geometria non è `Polygon`/`MultiPolygon`;
- `InvalidGeometry`: il risultato non supera la validazione OGC (una
  geometria d'ingresso non valida si rifiuta già alla decodifica);
- `ValidazioneNonConclusa`: la validazione OGC non ha concluso;
- `PrecisionInsufficient`: la griglia dell'overlay sposterebbe il risultato
  oltre la precisione (sotto, «Precisione»);
- `CalcoloNonConcluso`: l'overlay di `geo` è andato in panico.

#### Limiti e deviazioni

Nessun raggruppamento per attributo: tutta la tabella diventa una
geometria. Gli ingressi si orientano (anello esterno antiorario) prima
dell'unione, perché `unary_union` di `geo` sceglie la regola di
riempimento dal verso del primo anello e un poligono valido di verso
opposto sparirebbe ([README, «Precisione delle operazioni geografiche: 1 cm
a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Nessun controllo a posteriori del risultato contro gli ingressi.
Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Entro 1 cm a terra, nelle unità del CRS della colonna (il runner passa al
kernel `Precision::from_crs`): un solo overlay di `i_overlay` su interi `i64`, con la
griglia controllata prima del calcolo sull'ingombro di tutti gli ingressi.
Se lo spostamento a priori supera mezzo centimetro, o le coordinate sono
troppo rade per il centimetro, `PrecisionInsufficient` e nessun calcolo.
Due poligoni separati da meno di 1 cm possono fondersi, e parti più
sottili di 1 cm sparire, senza errore; vedi [README, «Precisione delle
operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

#### Complessità

Un overlay a scansione su tutti i vertici `v` della tabella, di norma
O((v + k) log v) con `k` gli incroci fra i lati, più la validazione OGC di
ingressi e risultato. Memoria O(v + k): l'intera tabella in memoria.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.dissolve", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |
| 2 | POLYGON((1 0,3 0,3 2,1 2,1 0)) |
| 3 | POLYGON((10 10,11 10,11 11,10 11,10 10)) |

Uscita `risultato`:

| `geometry: geometry` |
| --- |
| MULTIPOLYGON(((0 2,0 0,3 0,3 2,0 2)),((10 11,10 10,11 10,11 11,10 11))) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.distance`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_distance` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `float64` con la distanza euclidea planare fra la
geometria di ogni riga e una geometria fissa scritta nella config
(`other_wkb`), nelle unità del CRS: la distanza minima fra i due insiemi di
punti, 0 quando si intersecano (anche quando uno contiene l'altro). Con una
geometria vuota, da una parte o dall'altra, il kernel non dà una distanza.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB in esadecimale (cifre maiuscole o minuscole) di una geometria valida OGC, solo XY, senza SRID, coordinate nel dominio del CRS dell'ingresso | secondo operando, nel CRS della colonna geometria |
| `output_column` | stringa | `distance` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

L'ingresso ha una sola colonna geometria: il secondo operando arriva dalla
config ed è assunto nello stesso CRS, che nessun dato può confermare.
L'analisi ne verifica la struttura WKB, la validità OGC e il dominio delle
coordinate.

#### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: una distanza per riga, dal kernel `operations::distance` (riga,
`other_wkb`). Una geometria nulla dà una distanza nulla; una geometria
vuota, da una parte o dall'altra, anche: il kernel non ha una distanza da
rendere.

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`; `other_wkb`
  porta dimensioni Z/M o uno SRID EWKB;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare; una coordinata di `other_wkb` fuori dal dominio del
  CRS;
- `InvalidPlan`: config con campi sconosciuti o senza `other_wkb`;
  `other_wkb` vuoto, di lunghezza dispari o con caratteri non esadecimali,
  o con una struttura WKB non valida (conteggi, anelli non chiusi, byte in
  coda, coordinate non finite, oltre 64 MiB o 64 livelli d'annidamento),
  o che non supera la validazione OGC; `output_column` vuoto o di soli
  spazi;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria della riga non supera la
  validazione OGC (`other_wkb` l'ha già superata in analisi);
- `Internal` (`ValidazioneNonConclusa`, `CalcoloNonConcluso`): la
  validazione OGC o il calcolo di `geo` vanno in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Il secondo operando è uno solo per tutto il passo: la distanza fra due
colonne o fra due tabelle non c'è. La distanza è planare, non geodetica. Errori senza indice di riga
della sorgente ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: la distanza è il
calcolo in `f64` di `geo` (punto-segmento sulle coppie più vicine), senza un
bilancio d'errore dichiarato rispetto alla regola di 1 cm; per linee e
poligoni che si intersecano lo zero viene dal predicato d'intersezione di
`geo`, non da una differenza di coordinate
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per una riga di n vertici e `other_wkb` di m vertici: il test
d'intersezione di `geo` confronta coppie di segmenti, O(n·m) nel caso
peggiore; la ricerca della distanza minima che segue usa R-tree o
proiezioni ordinate dei segmenti. Più la
validazione OGC dei due operandi (sub-quadratica nel caso tipico, O(n²)
nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)),
ripetuta per `other_wkb` a ogni riga.

#### Memoria

Memoria: da misura v4.

#### Esempio

La geometria fissa è `POINT(0 0)`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.distance", "in": ["siti"],
 "config": {"other_wkb": "010100000000000000000000000000000000000000"}}
```

Ingresso `siti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(3 4) |
| 2 | LINESTRING(1 -1,1 1) |
| 3 | POLYGON((-1 -1,1 -1,1 1,-1 1,-1 -1)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `distance: float64` |
| --- | --- | --- |
| 1 | POINT(3 4) | 5.0 |
| 2 | LINESTRING(1 -1,1 1) | 1.0 |
| 3 | POLYGON((-1 -1,1 -1,1 1,-1 1,-1 -1)) | 0.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.envelope`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_envelope` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | protocollo pubblico |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni geometria con il suo rettangolo d'ingombro, nella stessa
colonna. Il rettangolo è un `Polygon`; se larghezza e altezza sono entrambe
nulle diventa il `Point` comune, se solo una è nulla la `LineString` di due
vertici dall'angolo minimo al massimo.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`Point`, `LineString`,
`Polygon`]: le chiavi `plenora.geometry.types` e
`plenora.geometry.types_declaration` ereditate si tolgono dal campo.

#### Righe

1:1: il runner chiama il kernel (`transform_geometry` con
`Operation::Envelope`) su ogni cella non nulla, in parallelo, e rimette la
geometria al suo posto; una cella nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. L'anello del
rettangolo parte da (`max x`, `min y`) e gira in senso antiorario.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria:

- `InvalidPlan`: la geometria d'ingresso non supera la validazione OGC, o
  è vuota (nessun rettangolo);
- `Internal`: la validazione OGC non conclude (panico dentro la barriera;
  il messaggio porta solo la forma del payload).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine
di riga, senza diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Una geometria vuota è un errore, dove PostGIS rende la geometria vuota.

#### Precisione

Esatta: le coordinate d'uscita sono i minimi e i massimi delle coordinate
d'ingresso, copiati senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) per l'ingombro, più la validazione
OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.envelope", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 1,2 3,0 0)) |
| 2 | LINESTRING(1 4,5 4) |
| 3 | POINT(2 3) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((4 0,4 3,0 3,0 0,4 0)) |
| 2 | LINESTRING(1 4,5 4) |
| 3 | POINT(2 3) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.explode`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_explode` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:N |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Spezza le geometrie multi-parte in una riga per parte: un `MultiPoint`, una
`MultiLineString` o un `MultiPolygon` danno una riga per ogni punto, linea
o poligono; una `GeometryCollection` una riga per ogni membro, di un solo
livello (una collezione annidata resta una riga). Una geometria semplice
resta una riga, invariata. Le altre colonne si ripetono sulle righe della
stessa madre, e `__parent_index` dice da quale riga vengono.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Le colonne d'ingresso restano, nell'ordine, con gli stessi tipi; la
colonna geometria tiene nome, CRS e dimensioni, ed è non nullable (una
riga a geometria null non produce parti). In coda si aggiunge
`__parent_index`, `uint64` non nullable: l'indice della riga madre. I
metadati di schema restano; delle proprietà del contratto resta
`sorted_by`, il conteggio delle righe non è più noto. I tipi dichiarati
della colonna si mappano sulle parti: `Point` e `MultiPoint` danno `Point`,
`LineString` e `MultiLineString` danno `LineString`, `Polygon` e
`MultiPolygon` danno `Polygon`, `GeometryCollection` uno qualunque dei
sette tipi; una dichiarazione `exact` resta `exact`, `mixed` resta `mixed`,
assente o `unresolved` resta tale.

#### Righe

Espansione 1:N: da 0 righe per madre (un multi o una collezione vuoti) a
una per parte. Il kernel (`operations::explode`) lavora su una geometria
alla volta; il runner lo chiama su ogni cella non nulla e ripete sulle
parti le altre colonne della madre. Una cella nulla non produce righe.
`__parent_index` è la posizione della madre nella tabella d'ingresso del
passo, contata da 0. Il runner conta le righe prodotte su tutta la tabella: oltre il limite di righe dell'arco d'uscita (`max_output_rows` per un output del
piano, `max_rows_per_edge` altrimenti), `ResourceLimit`.

#### Ordine

Le righe d'uscita seguono l'ordine delle madri, e dentro una madre l'ordine
delle parti; l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `__parent_index` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto (ogni CRS risolto è
  ammesso).

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi il kernel, per geometria, con errore `OperationError` che il runner
traduce così: `ValidazioneNonConclusa` diventa `Internal`, le altre
`InvalidPlan`:

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Un solo livello: `ST_Dump` di PostGIS scende invece fino alle geometrie
semplici. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Esatta: le parti sono copiate senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo e memoria O(n) per la copia delle parti,
più la validazione OGC dell'ingresso (sub-quadratica nel caso tipico,
O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.explode", "in": ["siti"],
 "config": {}}
```

Ingresso `siti` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOINT((0 0),(1 1)) |
| 2 | POLYGON((0 0,1 0,1 1,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `__parent_index: uint64` |
| --- | --- | --- |
| 1 | POINT(0 0) | 0 |
| 1 | POINT(1 1) | 0 |
| 2 | POLYGON((0 0,1 0,1 1,0 0)) | 1 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.frechet_distance`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `frechet_distance` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `float64` con la distanza di Fréchet discreta fra la
linea della riga e la linea costante `other_wkb`: la più piccola, fra tutti
gli accoppiamenti dei vertici che percorrono le due linee in avanti senza
tornare indietro, della massima distanza euclidea fra due vertici
accoppiati. Misura quanto due percorsi si somigliano tenendo conto del
verso: la stessa linea percorsa al contrario è lontana. Si accoppiano solo
i vertici, quindi un vertice in più su un lato dritto può cambiare il
risultato (la distanza continua sarebbe minore o uguale).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari) di una `LineString` valida OGC, nel CRS dell'input e dentro il suo dominio di validità | la linea di confronto, uguale per tutte le righe |
| `output_column` | stringa | `frechet_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi: struttura, validità OGC,
dominio del CRS e tipo (deve essere una `LineString`, quella che il kernel
chiede).

#### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una distanza per riga, nelle unità del CRS. Per il contratto
`other_wkb` è il secondo operando; la distanza discreta è simmetrica, quindi
l'ordine non cambia il valore. Il kernel
(`extended_algorithms::frechet_distance`) riceve due `LineString`. Una
geometria nulla dà una distanza nulla, e anche una linea vuota, da una
parte o dall'altra (il kernel non rende un valore); una riga che non è una
`LineString` ferma il passo con un errore (vedi «Errori»).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda), non valido OGC o che
  non è una `LineString`; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare; una coordinata di `other_wkb` fuori dal dominio di
  validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: la geometria della riga non è una `LineString` (errore
  del runner, «tipo geometria non supportato»); dal kernel `InvalidInput`
  (coordinate non finite o linea con meno di due punti distinti),
  `IndexOverflow`;
- `ResourceLimit`: dal kernel `WorkLimit` (prodotto dei vertici delle due
  linee oltre `10^8`, il tetto che il runner passa, o non rappresentabile
  in `u64`);
- `Internal`: dal kernel `ValidazioneNonConclusa` (la validazione non
  conclude) e `CalcoloNonConcluso` (panico di `geo`).

#### Limiti e deviazioni

- Distanza discreta, sui soli vertici, come `ST_FrechetDistance` di
  PostGIS senza densificazione; nessun parametro di densificazione qui.
- Il lavoro quadratico è limitato da un argomento del kernel: il runner
  passa `10^8` coppie di vertici per riga (l'ordine di `MAX_NODING_WORK`
  dei kernel); non è un parametro della config.
- La seconda linea è una costante della config, non una seconda colonna.
- Solo `LineString`: una `MultiLineString` nella colonna ferma il passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Nessun controllo e nessun rifiuto di precisione. Il risultato è una delle
distanze euclidee fra un vertice della riga e uno di `other_wkb`, scelta
con massimi e minimi esatti; ogni distanza è calcolata in `f64` con
l'errore relativo di un arrotondamento, molto sotto 1 cm ([README,
«Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(n·m) per riga, con `n` e `m` i vertici delle due linee, limitato prima
del calcolo; memoria O(min(n, m)). In più la validazione OGC delle due
linee.

#### Memoria

Memoria: da misura v4.

#### Esempio

La linea di confronto è `LINESTRING(0 0,10 0)`. La seconda riga ha gli
stessi punti, ma il vertice in mezzo si accoppia con un estremo: distanza 5;
la terza è la stessa linea al contrario.

Passo del piano:

```json
{"out": "risultato", "op": "geo.frechet_distance", "in": ["tracce"],
 "config": {"other_wkb": "0102000000020000000000000000000000000000000000000000000000000024400000000000000000"}}
```

Ingresso `tracce` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 1,10 1) |
| 2 | LINESTRING(0 0,5 0,10 0) |
| 3 | LINESTRING(10 0,0 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `frechet_distance: float64` |
| --- | --- | --- |
| 1 | LINESTRING(0 1,10 1) | 1.0 |
| 2 | LINESTRING(0 0,5 0,10 0) | 5.0 |
| 3 | LINESTRING(10 0,0 0) | 10.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.from_coords`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_from_coords` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | produttore 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Costruisce una colonna geometria di punti da due colonne numeriche: per
ogni riga il `Point` (x, y), nel CRS dato dalla config o dal piano. È un
produttore: l'ingresso non ha colonne geometria, l'uscita ne ha una.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `x_column` | stringa | `x` | colonna `float64` o `int64` dell'ingresso | coordinata x (est) |
| `y_column` | stringa | `y` | colonna `float64` o `int64` dell'ingresso | coordinata y (nord) |
| `geometry_column` | stringa | `geometry` | nome non vuoto (non di soli spazi) e non già presente | colonna aggiunta |
| `crs` | stringa | il CRS di piano | un identificatore della tabella dei CRS integrati, o la definizione del CRS di piano scritta uguale; proiettato | CRS della colonna prodotta |

Senza `crs` e senza CRS di piano il passo si rifiuta: il CRS non si
inventa.

#### Schema

Le colonne d'ingresso restano tutte, `x_column` e `y_column` comprese. In
coda si aggiunge `geometry_column`: `binary` GeoArrow-WKB (estensione
`geoarrow.wkb`, metadato `geo` con il CRS e dimensioni `xy`), non
nullable, colonna geometria attiva con un nuovo identificatore di campo e
senza tipi dichiarati. I metadati di schema e le proprietà del contratto
(`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: un punto per riga. Il runner legge le due colonne (`float64`, o
`int64` convertito in `f64` solo entro `2^53` in modulo, dove la
conversione è esatta), chiama il kernel (`construction::point_from_lon_lat`)
riga per riga e controlla che il punto stia nel dominio di validità del
CRS prodotto. Una x o una y nulla è un errore: la colonna prodotta è
dichiarata non nullable ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso ha già una colonna geometria; `x_column` o
  `y_column` non esiste o non è `float64` né `int64`; `geometry_column`
  esiste già;
- `InvalidPlan`: campi sconosciuti; `x_column`, `y_column` o
  `geometry_column` vuoti o di soli spazi;
- `Crs`: nessun `crs` e nessun CRS di piano; `crs` vuoto, fuori dalla
  tabella integrata o scritto come definizione (WKT, PROJJSON…) diversa da
  quella del piano; CRS geografico, o proiettato senza unità lineare.

In esecuzione, nel runner:

- `InvalidPlan`: un valore `int64` oltre `2^53` in modulo (non esatto in
  `f64`), in una delle due colonne; una x o una y nulla;
- `Crs`: il punto sta fuori dal dominio di validità del CRS prodotto.

Dal kernel, per riga (`ConstructionError`, che il runner porta in
`PlenoraError`: `Internal` per `ValidazioneNonConclusa`, `InvalidPlan` per
le altre):

- `NonFiniteCoordinate`: x o y è NaN o infinita (il messaggio chiama le due
  coordinate `lon` e `lat`, i nomi del kernel d'origine).

La conversione delle due colonne precede i punti; poi il primo errore è
quello della prima riga, senza diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il kernel controlla solo che le coordinate siano finite; il dominio del CRS
lo controlla il runner su ogni punto. Un `int64` oltre `2^53` in modulo si
rifiuta invece di arrotondarsi. Una coordinata nulla è un errore, non un
punto nullo come nel progetto d'origine. Il catalogo chiede un CRS proiettato: punti in
longitudine e latitudine non si costruiscono qui
([README, «CRS integrati»](../README.md#crs-integrati)).

#### Precisione

Esatta: le coordinate `float64` diventano il punto senza calcolo
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sulle righe, O(1) per punto; memoria O(n) per la colonna
prodotta (21 byte di WKB per punto).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.from_coords", "in": ["rilievi"],
 "config": {"crs": "EPSG:3857"}}
```

Ingresso `rilievi`:

| `id: int64` | `x: float64` | `y: float64` |
| --- | --- | --- |
| 1 | 500000.0 | 4000000.0 |
| 2 | 0.0 | 0.0 |

Uscita `risultato`:

| `id: int64` | `x: float64` | `y: float64` | `geometry: geometry` |
| --- | --- | --- | --- |
| 1 | 500000.0 | 4000000.0 | POINT(500000 4000000) |
| 2 | 0.0 | 0.0 | POINT(0 0) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.from_wkt`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | produttore 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 3, kernel 2 |

#### Che cosa fa

Crea la colonna geometria di una tabella che non ne ha, leggendo il testo
WKT di una colonna `utf8`: ogni cella diventa la geometria WKB
corrispondente, una cella nulla una geometria nulla. Il testo deve essere
WKT 2D senza SRID e descrivere una geometria valida: basta una cella che non
lo è perché l'intera colonna si rifiuti, con la diagnostica delle righe
colpevoli.

La conversione di colonna è `extensions::from_wkt_column`; il runner ne
chiama la variante `from_wkt_column_named`, che nomina la colonna nella
diagnostica.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `wkt_column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | colonna con il testo WKT |
| `output_column` | stringa | `geometry` | nome non vuoto e libero | nome della colonna geometria creata |
| `on_error` | stringa | `null` | `null`, `fail` | accettato per compatibilità: entrambi i valori rifiutano la colonna al primo WKT invalido |
| `crs` | stringa | CRS di piano | identificatore di un CRS integrato | CRS della colonna creata |

`on_error: "null"` non trasforma le celle invalide in null, nonostante il
nome: nessun valore produce un rimedio silenzioso.

#### Schema

Le colonne dell'ingresso restano, con i loro metadati; in coda si aggiunge
`output_column`, `binary`, nullable solo se lo è la colonna WKT (una cella
WKT null dà una geometria null), con l'estensione `geoarrow.wkb` e i
metadati `geo` (CRS, dimensioni `xy`, encoding WKB). Il contratto la dichiara
colonna geometria attiva, con i tipi `mixed` dei sette tipi WKB XY (`Point`,
`LineString`, `Polygon` e i multi, `GeometryCollection`). I metadati di
schema e le proprietà del contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1: il runner chiama la conversione di colonna dei kernel
(`extensions::from_wkt_column_named`) sulla colonna intera. Una cella
nulla dà una geometria nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso (le celle si convertono in parallelo, con l'ordine
ricostruito per indice).

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti o `on_error` fuori elenco;
  `wkt_column` o `output_column` vuoti;
- `Schema`: l'ingresso ha già una colonna geometria; `wkt_column` assente
  o di tipo diverso da `utf8` (`large_utf8` compreso); `output_column` già
  presente;
- `Crs`: né `crs` né un CRS di piano; `crs` non risolvibile (codice fuori
  dalla tabella integrata, definizione WKT o PROJ).

In esecuzione (conversione di colonna):

- `DataMapping` (fase di lettura), con diagnostica per riga completa: una o
  più celle con testo non WKT, prefisso `SRID=`, dimensioni Z/M, carattere
  NUL, testo oltre 64 MiB o geometria OGC-invalida (causa
  `geometry.invalid_wkt`), o geometria il cui WKB supera il limite di byte
  per cella (causa `geometry.encoding_failed`). La diagnostica conta tutte le
  righe colpevoli per causa e ne dà al più 10 come esempio (indice di riga,
  mai il testo); nessuna cella viene pubblicata;
- `Internal`: la validazione OGC di una cella non conclude;
- `Crs`: dopo la conversione, una coordinata di una geometria prodotta
  fuori dal dominio di validità del CRS della colonna creata.

È l'unica operazione geo con diagnostica per riga nel runner; gli indici
seguono la base delle tabellari: righe della sorgente, o dell'ingresso del
passo dopo un passo che cambia le righe
([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga)).

#### Limiti e deviazioni

- Il costo in memoria del passo è una previsione dalle misure
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Modelli di costo geo»).
- Solo WKT 2D: EWKT con `SRID=` e WKT con `Z`, `M`, `ZM` si rifiutano.
- `on_error` non ha effetto (sopra).

#### Precisione

Esatta: le coordinate sono i `f64` più vicini ai numeri del testo, senza
altro calcolo. Nessun controllo di precisione si applica
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(n) nella lunghezza totale del testo, più la validazione OGC di ogni
geometria (O(v²) nel caso peggiore sui suoi `v` vertici); memoria O(n) per
le celle WKB prodotte.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.from_wkt", "in": ["luoghi"],
 "config": {"wkt_column": "wkt", "crs": "EPSG:4326"}}
```

Ingresso `luoghi`:

| `id: int64` | `wkt: utf8` |
| --- | --- |
| 1 | POINT(12 41) |
| 2 | null |

Uscita `risultato`:

| `id: int64` | `wkt: utf8` | `geometry: geometry` |
| --- | --- | --- |
| 1 | POINT(12 41) | POINT(12 41) |
| 2 | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.generate_grid`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | da tutto l'ingresso a molte righe |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Genera una griglia regolare di celle poligonali che copre il rettangolo
`extent`, una riga per cella, con gli indici di colonna e di riga della
cella e, a richiesta, il suo centro. Le celle quadrate di lato `cell_size`
coprono tutto il rettangolo, e quelle dell'ultima colonna e dell'ultima
riga sono tagliate sul suo bordo; le celle esagonali (esagoni con un lato in
alto, di lato `cell_size`) escono solo se stanno per intero dentro il
rettangolo. La tabella d'ingresso serve solo da innesco: né le sue colonne
né le sue righe entrano nell'uscita.

Il calcolo è `extensions2::generate_grid_rows`, che il runner chiama una
volta per passo, qualunque sia l'ingresso ([README, «Operazioni geo»](../README.md#operazioni-geo)); il numero di
celle calcolato a secco entra nel modello di costo del passo.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `extent` | oggetto | obbligatorio | `xmin`, `ymin`, `xmax`, `ymax` finiti, `xmax > xmin`, `ymax > ymin`, vertici nel dominio del CRS | rettangolo da coprire, nelle unità del CRS |
| `cell_size` | numero | obbligatorio | finito, `> 0` | lato della cella (quadrata o esagonale) |
| `shape` | stringa | `square` | `square`, `hex` | forma delle celle |
| `crs` | stringa | CRS di piano | identificatore di un CRS integrato | CRS della griglia |
| `include_centroid` | booleano | `false` | `true`, `false` | aggiunge le coordinate del centro della cella |

Le celle non possono superare 1.000.000 (per le esagonali, anche il numero
di colonne): il conteggio si verifica in validazione, prima di allocare.

#### Schema

Schema nuovo, in quest'ordine: `geometry` (`binary` non nullable,
estensione `geoarrow.wkb`, CRS della griglia, dimensioni `xy`; il contratto
dichiara i tipi `exact` `Polygon`), `cell_i` e `cell_j` (`uint64` non
nullable) e, con `include_centroid`, `centroid_x` e `centroid_y`
(`float64` non nullable). Le colonne dell'ingresso spariscono, i metadati
di schema restano. Il contratto dichiara `row_count` come stima
(`Estimated`) con il numero esatto di celle, calcolato a secco;
`sorted_by` non c'è.

#### Righe

Una riga per cella, indipendente dall'ingresso:

- `square`: `ceil((xmax - xmin) / cell_size)` colonne per
  `ceil((ymax - ymin) / cell_size)` righe; la cella `(i, j)` va da
  `xmin + i * cell_size` a `min(xmin + i * cell_size + cell_size, xmax)` in
  x, e allo stesso modo in y; il centro è il punto medio della cella
  (tagliata);
- `hex`: centri a passo `1.5 * cell_size` in x, a partire da
  `xmin + cell_size`, e a passo `sqrt(3) * cell_size` in y, con le colonne
  dispari sfalsate di mezzo passo; nessuna cella se il rettangolo è più
  stretto di `2 * cell_size`.

`cell_i` conta le colonne da `xmin`, `cell_j` le righe da `ymin`, da 0.

#### Ordine

`square`: per riga (`cell_j` crescente), poi per colonna (`cell_i`
crescente). `hex`: per colonna, poi per riga.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti o `shape` fuori elenco;
  `extent` non finito o degenere; `cell_size` non finito o non positivo;
  più di 1.000.000 celle; numero di celle per asse non rappresentabile;
- `Schema`: l'ingresso ha già una colonna geometria;
- `Crs`: né `crs` né un CRS di piano; `crs` non risolvibile; un vertice di
  `extent` fuori dal dominio del CRS.

In esecuzione il calcolo rifà gli stessi controlli di `extent`,
`cell_size` e numero di celle (`InvalidPlan`, messaggio con prefisso
`geo.generate_grid:`).

#### Limiti e deviazioni

Al più 1.000.000 di celle. Le celle esagonali non coprono tutto il
rettangolo: restano scoperte una fascia a destra larga meno di
`1.5 * cell_size`, una in alto larga meno di `sqrt(3) * cell_size` e, sotto
le colonne dispari, mezzo esagono.

#### Precisione

Nessun controllo dedicato: le coordinate si calcolano in `f64`, senza
fusione delle operazioni
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Sono esatte quando `xmin`, `ymin`, `cell_size` e i loro multipli sono
rappresentabili; altrimenti:

- `square`: ogni coordinata ha pochi arrotondamenti; il lato destro
  di una cella (`x0 + cell_size`) e il sinistro della successiva
  (`xmin + (i + 1) * cell_size`) si calcolano in due modi e possono
  differire di un'unità in ultima posizione: fessure o sovrapposizioni di
  quell'ordine, molto sotto 1 cm;
- `hex`: l'ordinata del centro si accumula per somme successive, quindi lo
  scarto dalla griglia esatta cresce con il numero di righe, al più
  `10^6 * ulp(M) / 2` con `M` il modulo massimo delle coordinate (circa 1,9
  mm a 2·10^7 m); i vertici in comune fra esagoni vicini si calcolano da
  centri diversi e possono differire di un'unità in ultima posizione.

#### Complessità

O(c) in tempo e memoria sulle celle `c`, al più 1.000.000; il conteggio in
validazione costa O(1) per `square` e O(c) per `hex`.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.generate_grid", "in": ["innesco"],
 "config": {"extent": {"xmin": 0, "ymin": 0, "xmax": 20, "ymax": 10}, "cell_size": 10, "crs": "EPSG:3857", "include_centroid": true}}
```

Ingresso `innesco`:

| `id: int64` |
| --- |
| 1 |

Uscita `risultato`:

| `geometry: geometry` | `cell_i: uint64` | `cell_j: uint64` | `centroid_x: float64` | `centroid_y: float64` |
| --- | --- | --- | --- | --- |
| POLYGON((0 0,10 0,10 10,0 10,0 0)) | 0 | 0 | 5.0 | 5.0 |
| POLYGON((10 0,20 0,20 10,10 10,10 0)) | 1 | 0 | 15.0 | 5.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.geodesic_area`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `geodesic_area` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS geografico |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Aggiunge una colonna `float64` con l'area geodetica, in metri quadrati,
dei poligoni e multi-poligoni di ogni riga, sull'ellissoide del datum del
CRS della colonna ([README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)). Le
coordinate sono longitudine (`x`) e latitudine (`y`) in gradi e i lati sono
geodetiche fra vertici consecutivi. Il verso degli anelli non conta: ogni
poligono si orienta prima (esterno antiorario, buchi orari), e l'area è
quella dell'esterno meno quella dei buchi (algoritmo di Karney, il
calcolo di `geodesic_area_unsigned` di `geo` sul `PolygonArea` di
`geographiclib-rs` dell'ellissoide del datum). Un `MultiPolygon` somma le aree dei
suoi poligoni.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `geodesic_area` | nome non vuoto e non già nello schema | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: un'area per riga. Il kernel (`extended_algorithms::geodesic_area_m2`)
accetta solo `Polygon` e `MultiPolygon`; un poligono o un multi-poligono
vuoto dà `-0.0`. Una geometria nulla dà un'area nulla; una riga di altro
tipo ferma il passo con un errore (vedi «Errori»): l'analisi accetta ogni
tipo nella colonna, perché non conosce le celle.

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti o
  `output_column` non stringa; `output_column` vuoto;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non geografico;
  CRS senza l'ellissoide del datum (`ELLIPSOID_REQUIRED`).

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: una geometria diversa da `Polygon` e `MultiPolygon`
  (errore del runner, «tipo geometria non supportato», prima del kernel);
  dal kernel (`ExtendedAlgorithmError`) `InvalidInput` (coordinate non
  finite o geometria non valida per l'OGC; un lato con almeno 180 gradi
  di longitudine o un anello che sul globo gira al contrario o copre mezzo
  ellissoide, o un poligono che non passa la verifica di topologia delle
  geodetiche, vedi «Limiti e deviazioni»), `InvalidGeographicCoordinate`
  (longitudine fuori da `[-180, 180]` o latitudine fuori da `[-90, 90]`),
  `InvalidOutput` (area non finita);
- `Internal`: dal kernel `ValidazioneNonConclusa` (la validazione non
  conclude) e `CalcoloNonConcluso` (panico di `geo`).

#### Limiti e deviazioni

- L'interno si decide orientando nel piano lon/lat, mentre i lati sono
  geodetiche. Un lato con almeno 180 gradi di longitudine (un poligono
  sull'antimeridiano, scritto con longitudini da una parte e dall'altra di
  ±180, o un anello attorno a un polo) la geodetica lo percorre
  dall'altra parte, e l'area sarebbe quella del complemento sul globo: si
  rifiuta (`InvalidInput`), invece di rendere un'area sbagliata come fino
  alla versione 2 del catalogo. Un poligono sull'antimeridiano va diviso
  in due. Lo stesso per ogni anello che, letto come geodetiche, gira al
  contrario del piano (un lato lungo che passa dall'altra parte di un
  vertice: il triangolo `0 30, 170 30, 85 31` è antiorario nel piano ma
  orario sul globo, perché il lato fra i primi due vertici sale oltre 80°)
  o copre mezzo ellissoide o più: l'area di ogni anello si calcola con
  segno e un segno non positivo si rifiuta.
- **Topologia delle geodetiche.** La validità OGC si verifica nel piano
  lon/lat, i lati sono geodetiche: un lato lungo può passare dall'altra
  parte di un buco (`POLYGON((-80 30,80 30,80 80,-80 80,-80 30),(-1 40,-1
  41,1 41,1 40,-1 40))`: il lato inferiore sale oltre 70° a longitudine 0,
  e il buco a 40° è fuori dall'esterno geodetico) o di un'altra parte.
  L'area si calcola solo se la topologia delle geodetiche è dimostrabilmente
  quella del piano ([README, «Misure geodetiche: l'ellissoide del
  datum»](../README.md#misure-geodetiche-lellissoide-del-datum)): ogni lato
  deve avere una lunghezza maggiorata di al più 1000 km, una latitudine
  maggiorata sotto 90° e una rotazione `K L` al più 1 (maggioranti
  certificati, senza problema inverso), e gli anelli devono stare più
  lontani degli scarti fra geodetiche e corde. Poligoni catastali e
  comunali non ne sono toccati.
  Altrimenti `InvalidInput`. La verifica è prudente: rifiuta anche poligoni
  corretti con lati oltre 1000 km (in lunghezza maggiorata) o anelli
  vicini ai lati lunghi.
- CRS proiettati rifiutati; fino alla versione 2 del catalogo l'ellissoide
  era sempre WGS 84 (su ED50, a 42° di latitudine, circa 8e-5 di area in
  meno, oltre perimetro per 1 cm già su un quadrato di 500 m).
- Un poligono vuoto dà `-0.0`, non `0.0`.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Nessun controllo e nessun rifiuto di precisione. L'area è quella
dell'algoritmo di Karney in `f64` sull'ellissoide del datum; la regola di
1 cm per le aree ammette circa perimetro per 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
La somma non è compensata (come in `geo`): su un poligono piccolo lontano
dall'equatore lo scarto da GeographicLib è dell'ordine di 1e-5 m²
(l'oracolo ammette 1e-3 m²).

#### Complessità

O(n) per geometria, con `n` i vertici, più la validazione OGC
dell'ingresso (O(n²) nel caso peggiore). Memoria O(n) per la copia
orientata.

#### Memoria

Memoria: da misura v4.

#### Esempio

Il quadrato di un grado all'equatore, e lo stesso con un buco di mezzo
grado.

Passo del piano:

```json
{"out": "risultato", "op": "geo.geodesic_area", "in": ["celle"],
 "config": {}}
```

Ingresso `celle` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,1 0,1 1,0 1,0 0)) |
| 2 | POLYGON((0 0,1 0,1 1,0 1,0 0),(0.25 0.25,0.25 0.75,0.75 0.75,0.75 0.25,0.25 0.25)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `geodesic_area: float64` |
| --- | --- | --- |
| 1 | POLYGON((0 0,1 0,1 1,0 1,0 0)) | 12308778361.469452 |
| 2 | POLYGON((0 0,1 0,1 1,0 1,0 0),(0.25 0.25,0.25 0.75,0.75 0.75,0.75 0.25,0.25 0.25)) | 9231614224.814873 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.geodesic_distance`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `geodesic_distance` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS geografico |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Aggiunge una colonna `float64` con la distanza geodetica in metri fra il
punto di ogni riga e un punto fisso della config (`other_wkb`), calcolata
**sull'ellissoide del datum del CRS della colonna** (Internazionale 1924
per ED50 e Monte Mario, Clarke 1866 per NAD27, Airy per OSGB36, GRS 80,
WGS 84...) con l'algoritmo di Karney (`geographiclib-rs`), mai su un
ellissoide di comodo ([README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)). Le coordinate sono longitudine e
latitudine in gradi.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari) di un `Point`, longitudine in `[-180, 180]` e latitudine in `[-90, 90]` | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `geodesic_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null). Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

#### Righe

1:1: una distanza per riga, dal kernel (`extended::geodesic_distance_m`), che
riceve **due punti**. `other_wkb` deve essere un `Point` (l'analisi lo
verifica); una geometria nulla dà una distanza nulla, e una riga che non è
un `Point` ferma il passo con un errore (vedi «Errori»): l'analisi accetta
ogni tipo nella colonna, perché non conosce le celle.

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate), o `other_wkb` con Z/M o SRID;
- `Crs`: CRS della colonna mancante o non risolto, o non geografico
  (`GEOGRAPHIC_CRS_REQUIRED`); CRS senza l'ellissoide del datum (risolto
  dal chiamante, fuori dalla tabella integrata: `ELLIPSOID_REQUIRED`); una coordinata di `other_wkb` fuori da
  longitudine e latitudine ammesse (`COORDINATE_OUT_OF_CRS_DOMAIN`);
- `InvalidPlan`: config con campi sconosciuti, `other_wkb` assente, non
  esadecimale, WKB non valido nella struttura o nella validità OGC, o che
  non è un `Point`; `output_column` vuoto;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: la geometria della riga non è un `Point` (errore del
  runner, «tipo geometria non supportato»); dal kernel (`ExtendedError`)
  `InvalidGeographicCoordinate`, una coordinata non finita o fuori da
  `[-180, 180]` × `[-90, 90]`, e `InvalidOutput`, una distanza non finita
  (mai attesa);
- `Internal`: dal kernel `CalcoloNonConcluso` (il calcolo di `geo` va in
  panico).

#### Limiti e deviazioni

- **CRS proiettati rifiutati**, non riportati al loro CRS geografico di
  base: la distanza si chiede sulle coordinate geografiche
  (`geo.reproject` al CRS geografico del datum, prima).
- Fino alla versione 1 del catalogo l'ellissoide era sempre WGS 84: su
  ED50 circa 4 m ogni 100 km di troppo o di meno, senza errore.
- Solo punti, vedi «Righe»: una `MultiPoint` nella colonna ferma il
  passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Sull'ellissoide del datum l'algoritmo di Karney resta molto sotto 1 cm
(l'errore dichiarato da `geographiclib` è dell'ordine dei nanometri;
l'oracolo `tests/geodetica_oracolo.rs` lo confronta con GeographicLib e
PROJ su ogni ellissoide della tabella entro un micrometro). Nessun
rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo e memoria O(1) per riga (poche iterazioni del problema inverso).

#### Memoria

Memoria: da misura v4.

#### Esempio

`other_wkb` è `POINT(0 0)`: sull'ellissoide un grado di latitudine è più
corto di uno di longitudine all'equatore.

Passo del piano:

```json
{"out": "risultato", "op": "geo.geodesic_distance", "in": ["stazioni"],
 "config": {"other_wkb": "010100000000000000000000000000000000000000"}}
```

Ingresso `stazioni` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 1) |
| 2 | POINT(1 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `geodesic_distance: float64` |
| --- | --- | --- |
| 1 | POINT(0 1) | 110574.38855779878 |
| 2 | POINT(1 0) | 111319.49079327357 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.geodesic_line_length`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `geodesic_line_length` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS geografico |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Aggiunge una colonna `float64` con la lunghezza geodetica in metri della
linea di ogni riga: la somma delle geodetiche fra vertici consecutivi
**sull'ellissoide del datum del CRS della colonna**, con l'algoritmo di
Karney di [`geo.geodesic_distance`](#geogeodesic_distance)
([README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)). Le coordinate sono
longitudine e latitudine in gradi.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `geodesic_line_length` | nome non vuoto e non già nello schema | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null). Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

#### Righe

1:1: una lunghezza per riga, dal kernel
(`extended::geodesic_line_length_m`), che riceve **una `LineString`**
(vuota o di un solo vertice: 0). Una geometria nulla dà una lunghezza
nulla; una multilinea, un poligono o un punto fermano il passo con un
errore (vedi «Errori»): l'analisi accetta ogni tipo nella colonna, perché
non conosce le celle.

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `Crs`: CRS della colonna mancante o non risolto, o non geografico
  (`GEOGRAPHIC_CRS_REQUIRED`); CRS senza l'ellissoide del datum (risolto
  dal chiamante, fuori dalla tabella integrata: `ELLIPSOID_REQUIRED`);
- `InvalidPlan`: config con campi sconosciuti, `output_column` vuoto.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: la geometria non è una `LineString` (errore del runner,
  «tipo geometria non supportato»); dal kernel (`ExtendedError`)
  `InvalidGeographicCoordinate`, un vertice non finito o fuori da
  `[-180, 180]` × `[-90, 90]`, e `InvalidOutput`, una lunghezza non
  finita (mai attesa);
- `Internal`: dal kernel `CalcoloNonConcluso` (il calcolo di `geo` va in
  panico).

#### Limiti e deviazioni

- Ogni tratto è la geodetica **più breve** fra i due vertici: un tratto
  scritto con più di 180 gradi di longitudine (oltre l'antimeridiano) si
  misura dalla parte corta, non come il segmento che il piano lon/lat
  disegna.
- CRS proiettati rifiutati; fino alla versione 2 del catalogo l'ellissoide
  era sempre WGS 84 (come [`geo.geodesic_distance`](#geogeodesic_distance)).
- Solo `LineString`, vedi «Righe»: una `MultiLineString` ferma il passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Ogni tratto è una geodetica di Karney sull'ellissoide del datum, con
errore dell'ordine dei nanometri; la somma aggiunge un arrotondamento per
tratto, molto sotto 1 cm su ogni linea realistica. Nessun rifiuto
`PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sui vertici della linea; memoria O(1) oltre all'ingresso.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.geodesic_line_length", "in": ["rotte"],
 "config": {}}
```

Ingresso `rotte` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,1 0) |
| 2 | LINESTRING(0 0,0 1,1 1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `geodesic_line_length: float64` |
| --- | --- | --- |
| 1 | LINESTRING(0 0,1 0) | 111319.49079327357 |
| 2 | LINESTRING(0 0,0 1,1 1) | 221877.0378972296 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.geometry_accessors`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge colonne che descrivono la geometria di ogni riga: il tipo, il
numero di parti, il numero di anelli interni, il primo e l'ultimo punto di
una linea aperta e se la geometria è chiusa. Si sceglie quali colonne con
`fields`; la geometria non cambia.

Il calcolo per geometria è `extensions::geometry_accessors`, che il
runner chiama su ogni riga non nulla.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `fields` | lista di stringhe | tutte e sei | `geometry_type`, `num_geometries`, `num_interior_rings`, `start_point`, `end_point`, `is_closed`; non vuota, senza ripetizioni | colonne da aggiungere |
| `output_prefix` | stringa | `""` | qualunque | prefisso dei nomi delle colonne aggiunte |

Significato dei campi:

- `geometry_type`: `Point`, `LineString`, `Polygon`, `MultiPoint`,
  `MultiLineString`, `MultiPolygon`, `GeometryCollection`;
- `num_geometries`: 1 per le geometrie semplici, il numero di membri per
  multi-geometrie e collezioni (0 se vuote);
- `num_interior_rings`: i buchi di un `Polygon`, la loro somma su un
  `MultiPolygon`, 0 per gli altri tipi;
- `start_point`, `end_point`: `POINT(x y)` in WKT, solo per una
  `LineString` aperta con almeno due punti; null per ogni altro caso
  (linee chiuse, poligoni, multi-geometrie);
- `is_closed`: per una `LineString` se il primo punto coincide con
  l'ultimo (`true` anche per la linea vuota, come in `geo`), `true` per `Polygon` e `MultiPolygon`, `false` per gli altri
  tipi (`MultiLineString` compresa).

#### Schema

Le colonne dell'ingresso restano; in coda si aggiungono, nell'ordine fisso
della lista sopra e non in quello di `fields`, le colonne richieste, con
nome `output_prefix` + campo: `geometry_type` `utf8`, `num_geometries`
`uint64`, `num_interior_rings` `uint64`, `start_point` `utf8`, `end_point`
`utf8`, `is_closed` `bool`. `start_point` ed `end_point` sono nullable
(nulli per una geometria che non è una linea aperta); le altre solo se lo
è la colonna geometria (nulle dove la geometria è null). Colonna geometria, metadati e
proprietà del contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1. Una geometria nulla dà una cella nulla in ogni colonna aggiunta;
`start_point` e `end_point` sono nulli anche nei casi detti sopra.

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `fields` vuota, con
  ripetizioni o con un nome fuori elenco;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; una colonna da aggiungere esiste
  già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`ExtensionError`), per geometria: una geometria che non
supera la validazione OGC (`InvalidInput`) è `InvalidPlan`; una
validazione che non conclude è `Internal`.

#### Limiti e deviazioni

Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Esatta: conteggi e tipi non calcolano nulla, e `start_point`/`end_point`
riportano le coordinate del vertice nel testo più corto che, riletto, dà lo
stesso `f64`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(v) per riga sui suoi `v` vertici, più la validazione OGC della geometria
(O(v²) nel caso peggiore); memoria O(n) per le colonne aggiunte.

#### Memoria

Memoria: da misura v4.

#### Esempio

`fields` in un ordine qualunque: le colonne escono nell'ordine fisso.

Passo del piano:

```json
{"out": "risultato", "op": "geo.geometry_accessors", "in": ["oggetti"],
 "config": {"fields": ["is_closed", "geometry_type", "start_point"]}}
```

Ingresso `oggetti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,3 4) |
| 2 | POLYGON((0 0,8 0,8 8,0 8,0 0),(2 2,4 2,4 4,2 2)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `geometry_type: utf8` | `start_point: utf8` | `is_closed: bool` |
| --- | --- | --- | --- | --- |
| 1 | LINESTRING(0 0,3 4) | LineString | POINT(0 0) | false |
| 2 | POLYGON((0 0,8 0,8 8,0 8,0 0),(2 2,4 2,4 4,2 2)) | Polygon | null | true |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.geometry_diagnostics`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `geometry_diagnostics` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | diagnostica |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Sostituisce la colonna geometria con dieci colonne che la descrivono: tipo,
numero di coordinate, se è vuota, se ha solo coordinate finite, se è valida
per l'OGC e perché no, e il rettangolo d'ingombro. Accetta di proposito le
geometrie non valide, che descrive invece di rifiutare; non esegue alcun
algoritmo su coordinate non finite.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

La colonna geometria si toglie e al suo posto, nello stesso punto, entrano
dieci colonne senza metadati; `validity_reason` e i quattro `bounds_*`
sono nullable (nulli per una geometria valida o vuota), le altre cinque
solo se lo è la colonna geometria (nulle dove la geometria è null):

| colonna | tipo | contenuto |
| --- | --- | --- |
| `geometry_type` | `utf8` | `Point`, `LineString`, `Polygon`, `MultiPoint`, `MultiLineString`, `MultiPolygon`, `GeometryCollection` |
| `coordinate_count` | `uint64` | coordinate, duplicati e chiusure degli anelli compresi |
| `is_empty` | `bool` | `coordinate_count` è 0 |
| `is_finite` | `bool` | nessuna coordinata NaN o infinita |
| `is_valid` | `bool` | la geometria supera la validazione OGC; falso con coordinate non finite |
| `validity_reason` | `utf8` | nullo se valida; altrimenti la ragione, senza coordinate |
| `bounds_minx`, `bounds_miny`, `bounds_maxx`, `bounds_maxy` | `float64` | rettangolo d'ingombro; nulli se la geometria è vuota o non finita |

Le ragioni sono `coordinate NaN o infinite`, `punti distinti insufficienti`,
`anello con auto-intersezione`, `anelli che si intersecano`, `anello
interno fuori dal proprio esterno`, `poligoni sovrapposti` e `forma non
valida non ulteriormente distinta`. Il contratto d'uscita non ha più
colonne geometria; le altre colonne, i metadati di schema e le proprietà del
contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1: un referto per riga. Il kernel (`extended_algorithms::geometry_diagnostics`)
descrive una geometria alla volta. Una geometria nulla dà dieci celle
nulle. Il runner verifica solo la struttura WKB e il dominio del CRS, non
la validità OGC: una geometria non valida si descrive, non si rifiuta.
Dal WKB validato nella struttura non arrivano coordinate non finite:
`is_finite` falso si vede solo con geometrie costruite altrove.

#### Ordine

Quello d'ingresso; le dieci colonne nell'ordine della tabella sopra, nella
posizione della colonna geometria.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS, nomi):

- `InvalidPlan`: più o meno di un ingresso; config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; un'altra colonna ha già
  il nome di una delle dieci;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto (serve un CRS noto, di
  qualunque tipo).

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Il kernel (`ExtendedAlgorithmError`) non rifiuta le geometrie non valide;
rifiuta solo con `ValidazioneNonConclusa` (la validazione non conclude: il
referto direbbe un verdetto che non esiste), che diventa `Internal`, e
`IndexOverflow` (conteggio oltre `u64`), che diventa `InvalidPlan`.

#### Limiti e deviazioni

- La ragione è una classificazione del messaggio di `geo`, senza posizione
  né coordinate: indica il tipo di difetto, non dove sta.
- La validazione è quella di `geo` 0.33.1 con la ricerca rapida delle
  auto-intersezioni ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)),
  con in più il rifiuto degli anelli con una punta
  (`anello con auto-intersezione`).
- Una coordinata fuori dal dominio del CRS non si descrive: il controllo
  del runner prima del kernel ferma il passo con `Crs`.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Esatta: nessuna coordinata si calcola. Il rettangolo è il minimo e il
massimo delle coordinate, con i loro bit; il verdetto di validità è quello
della validazione OGC sulle coordinate come sono, senza tolleranza
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(n) per geometria, con `n` le coordinate, più la validazione OGC (O(n²)
nel caso peggiore). Memoria O(1) oltre la geometria.

#### Memoria

Memoria: da misura v4.

#### Esempio

Un quadrato valido, un anello a farfalla e una collezione vuota.

Passo del piano:

```json
{"out": "risultato", "op": "geo.geometry_diagnostics", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |
| 2 | POLYGON((0 0,10 10,10 0,0 10,0 0)) |
| 3 | GEOMETRYCOLLECTION EMPTY |

Uscita `risultato`:

| `id: int64` | `geometry_type: utf8` | `coordinate_count: uint64` | `is_empty: bool` | `is_finite: bool` | `is_valid: bool` | `validity_reason: utf8` | `bounds_minx: float64` | `bounds_miny: float64` | `bounds_maxx: float64` | `bounds_maxy: float64` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Polygon | 5 | false | true | true | null | 0.0 | 0.0 | 10.0 | 10.0 |
| 2 | Polygon | 5 | false | true | false | anello con auto-intersezione | 0.0 | 0.0 | 10.0 | 10.0 |
| 3 | GeometryCollection | 0 | true | true | true | null | null | null | null | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.hausdorff_distance`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `hausdorff_distance` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `float64` con la distanza di Hausdorff discreta fra
la geometria di ogni riga e una geometria fissa della config
(`other_wkb`), nelle unità del CRS. È la distanza **fra vertici**
(`HausdorffDistance` di `geo`): il massimo, nei due versi, della distanza
euclidea fra un vertice di una geometria e il vertice più vicino
dell'altra. I lati non contano.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari) di una geometria valida OGC, coordinate nel dominio di validità del CRS dell'ingresso | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `hausdorff_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

#### Righe

1:1: una distanza per riga, dal kernel `extended::hausdorff_distance`
(riga, `other_wkb`). Una geometria nulla dà una distanza nulla, e anche
una geometria senza coordinate, da una parte o dall'altra (il kernel
rende «nessun valore»).

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate), o `other_wkb` con Z/M o SRID;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`); una coordinata di `other_wkb` fuori dal
  dominio di validità del CRS (`COORDINATE_OUT_OF_CRS_DOMAIN`);
- `InvalidPlan`: config con campi sconosciuti, `other_wkb` assente, non
  esadecimale o WKB non valido nella struttura (anelli aperti,
  coordinate non finite, byte residui) o nella validità OGC,
  `output_column` vuoto;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`extended::hausdorff_distance`, `ExtendedError`), per riga:

- `InvalidPlan`: `InvalidInput` (coordinate non finite o geometria della
  riga non valida OGC; `other_wkb` è già validata in analisi),
  `IndexOverflow`;
- `ResourceLimit`: `WorkLimit` (il prodotto dei vertici delle due
  geometrie supera `10^8` coppie, il tetto che il runner passa al
  kernel);
- `Internal`: `ValidazioneNonConclusa` e `CalcoloNonConcluso`
  (validazione o calcolo interrotti).

#### Limiti e deviazioni

- **Solo vertici.** Diversa da `ST_HausdorffDistance` di PostGIS (GEOS
  `DiscreteHausdorffDistance`), che misura dai vertici di una geometria ai
  lati dell'altra: qui un vertice che sta su un lato dell'altra geometria
  ma lontano dai suoi vertici pesa per la distanza da quei vertici, e il
  risultato può essere maggiore (vedi l'esempio). Nessuna densificazione.
- Il lavoro è limitato da `max_coordinate_pairs` (prodotto dei vertici
  delle due geometrie): il runner passa `10^8` per riga, l'ordine di
  `MAX_NODING_WORK` dei kernel; non è un parametro della config.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Le distanze fra vertici sono calcolate in `f64` (differenze e `hypot`),
con errore relativo di pochi ulp: sotto 1 cm per ogni distanza sotto
circa `1e13` unità del CRS. Il massimo e il minimo non arrotondano.
Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n·m) sui vertici delle due geometrie, limitato da
`max_coordinate_pairs`, più la validazione OGC dei due ingressi; memoria
O(1) oltre agli ingressi.

#### Memoria

Memoria: da misura v4.

#### Esempio

`other_wkb` è `LINESTRING(0 0,10 0)`. Per la seconda riga il vertice
`(5, 1)` dista 1 dal segmento, ma `sqrt(26)` dai suoi vertici.

Passo del piano:

```json
{"out": "risultato", "op": "geo.hausdorff_distance", "in": ["tracce"],
 "config": {"other_wkb": "0102000000020000000000000000000000000000000000000000000000000024400000000000000000"}}
```

Ingresso `tracce` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 3,10 3) |
| 2 | LINESTRING(0 1,5 1,10 1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `hausdorff_distance: float64` |
| --- | --- | --- |
| 1 | LINESTRING(0 3,10 3) | 3.0 |
| 2 | LINESTRING(0 1,5 1,10 1) | 5.0990195135927845 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.haversine_distance`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `haversine_distance` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS geografico |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Aggiunge una colonna `float64` con la distanza in metri fra il punto di
ogni riga e un punto fisso della config (`other_wkb`), lungo il cerchio
massimo di una **sfera** con il raggio medio IUGG `R1 = a (1 - f / 3)`
dell'ellissoide del datum del CRS della colonna (per WGS 84 6 371 008,771 m,
per Internazionale 1924 6 371 229,315 m;
[README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)). Le coordinate sono longitudine e latitudine in gradi. Il
nome resta `haversine`, ma il calcolo non è la formula dell'emiseno: è il
problema inverso di `geographiclib-rs` a schiacciamento nullo, ben
condizionato anche agli antipodi. Per la distanza sull'ellissoide c'è
[`geo.geodesic_distance`](#geogeodesic_distance).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, lunghezza pari) di un `Point`, longitudine in `[-180, 180]` e latitudine in `[-90, 90]` | secondo operando, nello stesso CRS della colonna |
| `output_column` | stringa | `haversine_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null). Le altre colonne,
la geometria e i metadati restano; restano anche le proprietà del
contratto (`sorted_by`, `row_count`).

#### Righe

1:1: una distanza per riga, dal kernel (`extended::haversine_distance_m`), che
riceve **due punti**. `other_wkb` deve essere un `Point` (l'analisi lo
verifica); una geometria nulla dà una distanza nulla, e una riga che non è
un `Point` ferma il passo con un errore (vedi «Errori»): l'analisi accetta
ogni tipo nella colonna, perché non conosce le celle.

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, la
  colonna non si dichiara geometria WKB, o `output_column` esiste già;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate), o `other_wkb` con Z/M o SRID;
- `Crs`: CRS della colonna mancante o non risolto, o non geografico
  (`GEOGRAPHIC_CRS_REQUIRED`); CRS senza l'ellissoide del datum (risolto
  dal chiamante, fuori dalla tabella integrata: `ELLIPSOID_REQUIRED`); una coordinata di `other_wkb` fuori da
  longitudine e latitudine ammesse (`COORDINATE_OUT_OF_CRS_DOMAIN`);
- `InvalidPlan`: config con campi sconosciuti, `other_wkb` assente, non
  esadecimale, WKB non valido nella struttura o nella validità OGC, o che
  non è un `Point`; `output_column` vuoto;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: la geometria della riga non è un `Point` (errore del
  runner, «tipo geometria non supportato»); dal kernel (`ExtendedError`)
  `InvalidGeographicCoordinate`, una coordinata non finita o fuori da
  `[-180, 180]` × `[-90, 90]`, e `InvalidOutput`, una distanza non finita
  (mai attesa);
- `Internal`: dal kernel `CalcoloNonConcluso` (il calcolo di `geo` va in
  panico).

#### Limiti e deviazioni

- **Sfera, non ellissoide.** La distanza differisce dalla geodetica
  sull'ellissoide di qualche millesimo del valore. Dall'origine, su
  WGS 84, un grado di latitudine vale 111 195,08 m qui e 110 574,39 m
  sull'ellissoide (5,6 per mille), un grado di longitudine 111 195,08 m
  qui e 111 319,49 m sull'ellissoide.
- Fino alla versione 1 del catalogo il raggio era fisso (6 371 008,8 m,
  quello di `Haversine` di `geo`) qualunque fosse il datum, e il calcolo
  era la formula dell'emiseno: vicino agli antipodi, con l'emiseno
  arrotondato sopra 1, rendeva NaN senza errore, e poco prima perdeva
  decimetri per arrotondamento.
- CRS proiettati rifiutati.
- Solo punti, vedi «Righe»: una `MultiPoint` nella colonna ferma il
  passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

La regola di 1 cm non riguarda il modello: il risultato è la distanza
sulla sfera, non la distanza vera sull'ellissoide (sopra). Sulla sfera il
calcolo è accurato ai nanometri ovunque, antipodi compresi (l'oracolo
`tests/geodetica_oracolo.rs` lo confronta con GeographicLib entro un
micrometro su ogni ellissoide della tabella); il risultato è sempre
finito. Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo e memoria O(1) per riga.

#### Memoria

Memoria: da misura v4.

#### Esempio

`other_wkb` è `POINT(0 0)`: un grado di latitudine e uno di longitudine
all'equatore valgono lo stesso arco sulla sfera.

Passo del piano:

```json
{"out": "risultato", "op": "geo.haversine_distance", "in": ["stazioni"],
 "config": {"other_wkb": "010100000000000000000000000000000000000000"}}
```

Ingresso `stazioni` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 1) |
| 2 | POINT(1 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `haversine_distance: float64` |
| --- | --- | --- |
| 1 | POINT(0 1) | 111195.07973463158 |
| 2 | POINT(1 0) | 111195.07973463158 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.intersection`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_intersection` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Sostituisce la geometria della sinistra con la sua intersezione con la
geometria della destra, `sinistra ∩ destra`, calcolata dal kernel
`topology::boolean_operation_validated` ([README, «Operazioni
geo»](../README.md#operazioni-geo)). Lavora solo su `Polygon` e
`MultiPolygon` e rende sempre un `MultiPolygon`.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi; le colonne della destra non passano. La colonna geometria resta al
suo posto, con lo stesso nome e lo stesso CRS della sinistra (uguale a
quello della destra), in XY, ed è nullable anche quando quella della
sinistra non lo è (un risultato vuoto è nullo); i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

#### Righe

Allineate: la riga `i` della sinistra con la riga `i` della destra, e le
due tabelle devono avere le stesse righe (altrimenti `InvalidPlan`, in
esecuzione: le righe non si conoscono a secco). Una riga d'uscita per riga
della sinistra, con la geometria nulla dove una delle due è nulla o il
risultato è vuoto. Dove le due geometrie non si intersecano, o si toccano
solo su un lato o in un punto, il kernel rende un `MultiPolygon` vuoto e
la riga resta con la geometria nulla.

#### Ordine

Quello della sinistra.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config non vuota;
  un metadato di schema presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte);
- `InvalidPlan`: le due tabelle hanno un numero di righe diverso.

Dal kernel (`topology::boolean_operation_validated`, errore
`TopologyError`, sulle geometrie già validate: resta la validazione OGC
del risultato), nella categoria del passo geo indicata fra parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): il risultato non supera la
  validazione OGC;
- `PrecisionInsufficient` (`Unsupported`): la griglia dell'overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): la
  validazione OGC o l'overlay di `geo` non ha concluso.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Solo poligoni: un'intersezione che si riduce a linee o punti (due quadrati
che si toccano su un lato) è vuota, quindi la riga ha la geometria nulla,
dove GEOS e PostGIS renderebbero la linea o il punto. Nessun controllo a posteriori del
risultato contro gli ingressi ([README, «Precisione delle operazioni
geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra: un
solo overlay di `i_overlay` su interi `i64`, con la griglia controllata
prima del calcolo sull'ingombro dei due operandi. Se lo spostamento a
priori supera mezzo centimetro, o le coordinate sono troppo rade per il
centimetro, `PrecisionInsufficient` e nessun calcolo. Parti più sottili di
1 cm possono sparire o fondersi senza errore; vedi [README, «Precisione
delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

#### Complessità

Per coppia l'overlay a scansione di `i_overlay`, di norma O((v + k) log v)
con `v` i vertici dei due operandi e `k` gli incroci fra i lati, più la
validazione OGC di ingressi e risultato (di norma O(v log v), nel caso
peggiore O(v²): [README, «Validazione OGC: la
ricerca delle auto-intersezioni non è quella di `geo`, il verdetto
sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(v + k).

#### Memoria

Memoria: da misura v4.

#### Esempio

Una riga per lato: la riga `i` della sinistra con la riga `i` della destra.

Passo del piano:

```json
{"out": "risultato", "op": "geo.intersection", "in": ["lotti", "vincoli"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |

Ingresso `vincoli` (geometrie `geometry` in EPSG:3857):

| `zona: utf8` | `geometry: geometry` |
| --- | --- |
| A | POLYGON((1 0,3 0,3 2,1 2,1 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOLYGON(((1 2,1 0,2 0,2 2,1 2))) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.length`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_length` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | misura terminale |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `float64` con la lunghezza planare della geometria di
ogni riga, nelle unità del CRS. Una linea vale la somma dei suoi segmenti;
un poligono il suo perimetro, anello esterno più buchi (la semantica di
`length` di Shapely); una multi-geometria o una collezione la somma delle
parti; i punti valgono 0.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `length` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: una lunghezza per riga, dal kernel `operations::length`; una
geometria nulla dà una cella nulla.

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare;
- `InvalidPlan`: campi sconosciuti nella config, `output_column` vuoto o di soli spazi.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria non supera la validazione
  OGC;
- `Internal` (`ValidazioneNonConclusa`, `CalcoloNonConcluso`): la
  validazione OGC o il calcolo di `geo` vanno in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Un poligono ha la lunghezza del suo perimetro, dove `ST_Length` di PostGIS
rende 0. La lunghezza è planare, non geodetica.
Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: la lunghezza è la
somma in `f64` delle lunghezze dei segmenti calcolate da `geo`, senza un
bilancio d'errore dichiarato rispetto alla regola di 1 cm
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) per la somma, più la validazione OGC
dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.length", "in": ["tratte"],
 "config": {}}
```

Ingresso `tratte` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,3 4) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |
| 3 | POINT(1 1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `length: float64` |
| --- | --- | --- |
| 1 | LINESTRING(0 0,3 4) | 5.0 |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) | 40.0 |
| 3 | POINT(1 1) | 0.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.line_builder`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_line_builder` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | N:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Costruisce una `LineString` dai punti della colonna geometria, nell'ordine
delle righe. La config è vuota: non ci sono colonne di gruppo né d'ordine,
quindi per contratto tutta la tabella diventa una sola geometria, e le
colonne attributo si perdono.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

L'uscita ha la sola colonna geometria: stesso nome, CRS, dimensioni e
metadati di campo, nullable. Le altre colonne non passano; i metadati di
schema sì. Le proprietà del contratto (`sorted_by`, `row_count`) cadono, e
la dichiarazione dei tipi ereditata si toglie senza sostituirla (nessuna
dichiarazione).

#### Righe

Aggregazione: tutte le righe in una. Il kernel
(`construction::line_from_ordered_points`) riceve il gruppo ordinato delle
geometrie, salta quelle assenti (`None`) e, con meno di due punti, non
costruisce la linea (`None`, non un errore). Il runner gli passa tutte le
celle della colonna nell'ordine delle righe, una cella nulla come `None`,
e rende sempre una riga: la linea, o una geometria nulla quando la linea
manca (anche per una tabella vuota).

#### Ordine

I vertici seguono l'ordine delle righe d'ingresso: è l'ordine a dare la
forma della linea.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel, sul gruppo, con errore `ConstructionError` che il runner
traduce così: `ValidazioneNonConclusa` diventa `Internal`, le altre
`InvalidPlan`:

- `ExpectedPoint`: una geometria non è un `Point` (il messaggio dà la
  posizione nel gruppo, assenti comprese, e il tipo trovato);
- `InvalidOutput`: la linea non supera la validazione OGC (per esempio
  tutti i punti coincidono);
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Non ci sono colonne di gruppo né d'ordine (il kernel lavora su un gruppo
già ordinato, ma nessun parametro lo forma): una linea per gruppo non si
può chiedere. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Esatta: i vertici sono le coordinate dei punti, copiate
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sulle righe più la validazione OGC della linea (O(n) per una
`LineString`); memoria O(n): tutta la colonna diventa una geometria.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.line_builder", "in": ["tappe"],
 "config": {}}
```

Ingresso `tappe` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 0) |
| 2 | POINT(4 0) |
| 3 | POINT(4 3) |

Uscita `risultato`:

| `geometry: geometry` |
| --- |
| LINESTRING(0 0,4 0,4 3) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.line_interpolate_point`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `line_interpolate_point` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni linea con il punto che sta alla frazione `ratio` della sua
lunghezza, misurata dall'inizio: la distanza `ratio * L` si percorre lato
per lato e il punto si interpola sul lato in cui cade. `ratio` 0 dà il
primo vertice, 1 l'ultimo. La lunghezza è quella euclidea nel piano del
CRS.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `ratio` | numero | obbligatorio | finito, in `[0, 1]` | frazione della lunghezza dall'inizio della linea |

#### Schema

La colonna geometria si riscrive al suo posto, con lo stesso nome, lo
stesso CRS e dimensioni `xy`; i tipi dichiarati diventano esattamente
`Point` (le chiavi dei tipi ereditate si tolgono dai metadati del campo). Le altre colonne,
i metadati di schema e le proprietà del contratto (`sorted_by`,
`row_count`) restano.

#### Righe

1:1. Il kernel (`extended_algorithms::line_interpolate_point`) riceve una
`LineString` e rende nessun punto se è vuota. Il runner lo chiama su ogni
cella non nulla, in parallelo: una cella nulla resta nulla; una linea
vuota dà null, e se la colonna d'uscita (con la nullabilità di quella
d'ingresso) non ammette null il passo si rifiuta con `InvalidPlan`; una
riga di altro tipo (anche `MultiLineString`) è `InvalidPlan` («tipo
geometria non supportato»). L'analisi non controlla i tipi dichiarati
dell'ingresso ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `ratio` o con un tipo sbagliato; `ratio` non finito o fuori da
  `[0, 1]`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi, per riga: `InvalidPlan` per una geometria che non è una
`LineString`. Il kernel rende `ExtendedAlgorithmError`, che il runner
porta in `Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre. Rifiuta la linea con `InvalidInput` (coordinate non finite o meno di due punti
distinti; `ValidazioneNonConclusa` se la validazione non conclude) e con
`CalcoloNonConcluso` (panico di `geo`).

Dopo il kernel, `InvalidPlan` per una linea vuota in una colonna che il
contratto dichiara non nullable (sopra, «Righe»). Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

- Solo `LineString` nel kernel; solo CRS proiettati.
- La frazione è della lunghezza euclidea planare, non geodetica.

#### Precisione

Nessun controllo e nessun rifiuto di precisione. Con `ratio` 0 il punto è
il primo vertice, con i suoi bit; negli altri casi è calcolato in `f64`
(somma delle lunghezze dei lati, interpolazione sul lato), con un errore
relativo dell'ordine del numero di lati per 1e-16 della lunghezza: molto
sotto 1 cm su ogni linea realistica. Con `ratio` 1 il punto è l'ultimo
vertice a meno di qualche ulp ([README, «Precisione delle operazioni
geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(n) per linea, con `n` i vertici, più la validazione OGC dell'ingresso.
Memoria O(1) oltre la linea.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.line_interpolate_point", "in": ["percorsi"],
 "config": {"ratio": 0.25}}
```

Ingresso `percorsi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,0 10,10 10) |
| 2 | LINESTRING(0 0,4 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 5) |
| 2 | POINT(1 0) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.line_locate_point`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `float64` con la posizione, lungo la linea di ogni
riga, del punto della linea più vicino a un punto fisso dato nella config:
la frazione della lunghezza totale da 0 (inizio) a 1 (fine), come
`ST_LineLocatePoint` di PostGIS. Le geometrie che non sono `LineString`
danno null.

Il calcolo per geometria è `extensions::line_locate_point`, che il
runner chiama su ogni riga non nulla.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `point_wkb` | stringa | obbligatorio | WKB esadecimale di un `Point` valido, nel dominio del CRS della colonna | il punto da proiettare, nello stesso CRS della colonna |
| `output_column` | stringa | `fraction` | nome non vuoto e libero | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64` nullable. Le altre colonne, i
metadati e le proprietà del contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1. Il valore è:

- per una `LineString` di almeno due punti, la frazione in `[0, 1]`: un
  punto oltre un estremo si proietta sull'estremo; se due segmenti sono
  alla stessa distanza dal punto vince il primo nel verso della linea;
- null per ogni altro tipo, `MultiLineString` compresa, e per la linea
  vuota.

Una linea con un solo punto distinto non supera la validazione OGC: errore,
non un valore.

Una geometria nulla dà una cella nulla.

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `point_wkb` mancante, non
  esadecimale, malformato, OGC-invalido o non `Point`; `output_column`
  vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; `output_column` già presente;
- `Unsupported`: dimensioni della geometria diverse da `xy`; `point_wkb`
  con dimensioni Z/M o SRID;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta; punto fuori
  dal dominio del CRS.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`ExtensionError`), per geometria: una geometria che non
supera la validazione OGC (`InvalidInput`) è `InvalidPlan`; un panico di
`geo` o una validazione che non conclude sono `Internal`.

#### Limiti e deviazioni

Una `MultiLineString` dà null, non la frazione lungo la parte più vicina.
Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Nessun controllo dedicato: distanze e frazione sono calcolate in `f64` da
`geo` (distanza dai segmenti, proiezione sul segmento più vicino, lunghezze
cumulate), con errori d'arrotondamento relativi alla lunghezza della linea,
molto sotto 1 cm per le lunghezze dei CRS reali. Dove due segmenti sono alla
stessa distanza dal punto entro l'arrotondamento, la scelta fra i due, e
con essa la frazione, può cambiare con l'ultimo bit delle coordinate
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(v) per riga sui suoi `v` vertici, più la validazione OGC della geometria
(O(v²) nel caso peggiore); memoria O(n) per la colonna aggiunta.

#### Memoria

Memoria: da misura v4.

#### Esempio

`point_wkb` è `POINT(5 3)`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.line_locate_point", "in": ["tratte"],
 "config": {"point_wkb": "010100000000000000000014400000000000000840"}}
```

Ingresso `tratte` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,10 0) |
| 2 | LINESTRING(0 0,10 0,10 10) |
| 3 | POLYGON((0 0,4 0,4 4,0 4,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `fraction: float64` |
| --- | --- | --- |
| 1 | LINESTRING(0 0,10 0) | 0.5 |
| 2 | LINESTRING(0 0,10 0,10 10) | 0.25 |
| 3 | POLYGON((0 0,4 0,4 4,0 4,0 0)) | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.line_merge`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `line_merge` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | da tutto l'ingresso a molte righe |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 3, kernel 1 |

#### Che cosa fa

Fonde le linee in percorsi più lunghi possibile: due linee si uniscono in
un estremo solo se quell'estremo è condiviso da esattamente due linee. Un
nodo in cui si incontrano tre o più linee, o l'estremo libero di una linea,
è sempre un confine fra percorsi. Una linea percorsa al contrario si
inverte per proseguire il percorso. Gli estremi si confrontano per bit, con
`-0.0` uguale a `0.0`, senza tolleranza.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Solo la colonna geometria, non nullable (una riga per linea fusa), con lo stesso nome, lo stesso CRS,
dimensioni `xy` e i metadati del campo d'ingresso, tranne le chiavi dei
tipi ereditate; i tipi dichiarati diventano esattamente `LineString`. Le
colonne attributo cadono. I metadati di schema restano; le proprietà del
contratto (`sorted_by`, `row_count`) cadono.

#### Righe

Aggregazione: per il contratto l'uscita ha solo la colonna geometria, una
riga per percorso fuso. Il kernel (`extended_algorithms::line_merge`) fonde
le linee di una geometria (`LineString`, `MultiLineString` o una
`GeometryCollection` di sole linee, anche annidata) e riceve come
argomenti il massimo di coordinate d'ingresso e di linee d'uscita. Il
runner riunisce le geometrie non nulle di tutte le righe, nell'ordine delle
righe, in una `GeometryCollection` e la fonde con un solo calcolo: una
cella nulla si salta, e una tabella vuota o tutta nulla non dà righe. I
limiti che passa sono `MAX_CLEAN_VERTICES` (100.000.000) coordinate e il
limite di righe dell'arco d'uscita (`max_output_rows` per un output del
piano, `max_rows_per_edge` altrimenti) come linee. Le linee vuote si
ignorano; una linea chiusa esce sempre da sola, com'è.

#### Ordine

Deterministico e indipendente dall'hash. Prima, nell'ordine delle linee
d'ingresso, le linee chiuse e i percorsi che toccano un nodo di grado
diverso da due, ciascuno dal primo estremo della sua prima linea se quello
non ha grado due, altrimenti dall'ultimo; poi gli anelli fatti solo di nodi
di grado due, ciascuno dalla prima linea non ancora usata e dal suo estremo
minore (bit di `x`, poi di `y`). Nelle giunzioni un vertice uguale al
precedente non si ripete.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config non vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel, con errore `ExtendedAlgorithmError` che il runner traduce
così: `Internal`, `ValidazioneNonConclusa` e `CalcoloNonConcluso`
diventano `Internal`, i limiti (`CoordinateLimit`, `OutputLimit`)
`ResourceLimit`, le altre `InvalidPlan`. Il kernel rifiuta la
geometria riunita con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude),
`CoordinateLimit` (coordinate d'ingresso oltre il limite passato dal
runner),
`UnsupportedGeometry` (punti o poligoni, anche dentro una collezione),
`IndexOverflow`, `OutputLimit` (linee d'uscita oltre il limite di righe
dell'arco), `InvalidOutput` (linea fusa non valida), `Internal`
(invariante interna violata).

#### Limiti e deviazioni

- Nessuna tolleranza: estremi distinti anche di un ulp non si uniscono.
- Come il line merge di GEOS (`ST_LineMerge` di PostGIS) un nodo di grado
  diverso da due separa i percorsi; l'ordine e il verso dei percorsi
  d'uscita sono quelli descritti sopra, non quelli di GEOS.
- I limiti di coordinate e di linee sono quelli del runner, sopra; non si
  scelgono dal piano.
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Esatta: nessuna coordinata si calcola, i vertici d'uscita sono quelli
d'ingresso con i loro bit. Due estremi distinti a meno di 1 cm non si
uniscono: sono feature più vicine della precisione, fuori dalla garanzia
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

O(n) per geometria, con `n` le coordinate (una mappa degli estremi, ogni
linea percorsa una volta), più la validazione OGC dell'ingresso e delle
linee d'uscita. Memoria O(n).

#### Memoria

Memoria: da misura v4.

#### Esempio

Due linee con un estremo in comune (la seconda al contrario) e una linea
staccata, in una sola riga.

Passo del piano:

```json
{"out": "risultato", "op": "geo.line_merge", "in": ["tratte"],
 "config": {}}
```

Ingresso `tratte` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTILINESTRING((0 0,1 0),(2 0,1 0),(5 5,6 5)) |

Uscita `risultato`:

| `geometry: geometry` |
| --- |
| LINESTRING(0 0,1 0,2 0) |
| LINESTRING(5 5,6 5) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.line_substring`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `line_substring` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni linea con la sua porzione fra le frazioni `start_ratio` e
`end_ratio` della lunghezza, misurate dall'inizio. Il primo e l'ultimo
punto sono quelli di [`geo.line_interpolate_point`](#geoline_interpolate_point)
alle due frazioni; in mezzo restano, con i loro bit, i vertici d'ingresso
che cadono strettamente fra le due distanze. Con due frazioni uguali la
porzione è un punto. La lunghezza è quella euclidea nel piano del CRS.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `start_ratio` | numero | obbligatorio | finito, in `[0, 1]`, non maggiore di `end_ratio` | frazione della lunghezza a cui comincia la porzione |
| `end_ratio` | numero | obbligatorio | finito, in `[0, 1]` | frazione della lunghezza a cui finisce la porzione |

#### Schema

La colonna geometria si riscrive al suo posto, con lo stesso nome, lo
stesso CRS e dimensioni `xy`; i tipi dichiarati diventano esattamente
`Point` e `LineString` (le chiavi dei tipi ereditate si tolgono dai
metadati del campo). Le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1. Il kernel (`extended_algorithms::line_substring`) riceve una
`LineString`, rende una `LineString`, un `Point` se `start_ratio` e
`end_ratio` sono uguali (confronto numerico `==`: `-0.0` e `0.0` sono
uguali), e nessuna geometria se la linea
è vuota. Il runner lo chiama su ogni cella non nulla, in parallelo: una
cella nulla resta nulla; una linea vuota dà null, e se la colonna d'uscita
(con la nullabilità di quella d'ingresso) non ammette null il passo si
rifiuta con `InvalidPlan`; una riga di altro tipo (anche
`MultiLineString`) è `InvalidPlan` («tipo geometria non supportato»).
L'analisi non controlla i tipi dichiarati dell'ingresso
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso; dentro la porzione i vertici seguono il verso della
linea.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza una delle due frazioni o con un tipo sbagliato; frazione non
  finita o fuori da `[0, 1]`; `start_ratio` maggiore di `end_ratio`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi, per riga: `InvalidPlan` per una geometria che non è una
`LineString`. Il kernel rende `ExtendedAlgorithmError`, che il runner
porta in `Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre. Rifiuta la linea con `InvalidInput` (coordinate non finite o meno di due punti
distinti; `ValidazioneNonConclusa` se la validazione non conclude),
`CalcoloNonConcluso` (panico di `geo`) e `InvalidOutput` quando la porzione
non è una geometria valida: due frazioni diverse i cui punti coincidono in
`f64` danno una linea di un solo punto distinto (per esempio le frazioni
0,5 e 0,5000000000000001 su un lato di 1 m a un milione di metri
dall'origine).

Dopo il kernel, `InvalidPlan` per una linea vuota in una colonna che il
contratto dichiara non nullable (sopra, «Righe»). Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

- Solo `LineString` nel kernel; solo CRS proiettati.
- Due frazioni diverse ma troppo vicine per dare due punti distinti sono
  un errore, non un punto: il punto esce solo con frazioni uguali.

#### Precisione

Nessun controllo e nessun rifiuto di precisione. I vertici interni sono
quelli d'ingresso, con i loro bit. I due estremi sono calcolati in `f64`
come in [`geo.line_interpolate_point`](#geoline_interpolate_point), con un
errore relativo dell'ordine del numero di lati per 1e-16 della lunghezza:
molto sotto 1 cm su ogni linea realistica ([README, «Precisione delle
operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Un vertice a distanza cumulata quasi uguale a un estremo può entrare o
restare fuori secondo l'arrotondamento delle due somme.

#### Complessità

O(n) per linea, con `n` i vertici, più la validazione OGC dell'ingresso e
dell'uscita. Memoria O(n).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.line_substring", "in": ["percorsi"],
 "config": {"start_ratio": 0.25, "end_ratio": 0.75}}
```

Ingresso `percorsi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,0 10,10 10) |
| 2 | LINESTRING(0 0,8 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 5,0 10,5 10) |
| 2 | LINESTRING(2 0,6 0) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.make_valid`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_make_valid` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione non interrompibile |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 1, kernel 2 |

#### Che cosa fa

Ripara la geometria di ogni riga che non supera la validazione OGC del
workspace (un anello che si auto-interseca, un buco che esce dalla shell,
parti di un multipoligono che si sovrappongono) con la regola `MakeValid`
`LINEWORK` di GEOS, eseguita in Rust puro sui lati nodati del bordo. Le
geometrie già valide escono invariate, byte per byte; i null restano null.
La riparazione può cambiare tipo: un poligono a farfalla diventa un
`MultiPolygon`, e le parti collassate (lati ripercorsi, anelli ridotti a un
punto) escono come linee o punti accanto all'area, in una
`GeometryCollection`.

La semantica a livello di tabella è quella dell'esecuzione Arrow
(`rust_backend::arrow::make_valid_batches`), che il runner chiama su tutta
la tabella con la precisione del CRS della colonna ([README, «Operazioni
geo»](../README.md#operazioni-geo)).

#### Parametri

Nessuno: la config è `{}`. Metodo e parti collassate non si scelgono dal
piano: l'esecuzione Arrow usa sempre `LINEWORK` con le parti collassate
conservate. `STRUCTURE` (la regola di `GeometryFixer`) è raggiungibile solo
dall'API dei kernel (`rust_backend::make_valid_wkb`).

#### Schema

Le colonne restano quelle dell'ingresso, nelle stesse posizioni, con gli
stessi tipi e la stessa nullabilità. Il campo geometria conserva tutti i
suoi metadati (CRS, dimensioni, encoding, chiavi di lineage) tranne la
dichiarazione dei tipi geometrici, che la riparazione riscrive: il
contratto dichiara `mixed` senza elenco. I metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1. Una cella null resta null; una cella valida esce con gli stessi byte;
una cella invalida esce riparata e rivalidata.

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con un campo qualunque;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB (né estensione `geoarrow.wkb` né
  chiavi `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione, prima del kernel, su ogni cella non nulla (runner, [README,
«Operazioni geo»](../README.md#operazioni-geo)): la decodifica strutturale
(`InvalidPlan` per un WKB malformato o con coordinate non finite,
`Unsupported` per dimensioni Z/M o SRID), `Crs` per una coordinata fuori
dal dominio di validità del CRS della colonna, `Schema` per una geometria
di un tipo che il contratto dell'ingresso dichiara con un elenco e che non
vi compare. La validità OGC non si controlla qui: è il lavoro del kernel.

Poi l'esecuzione Arrow (vince la prima cella che fallisce in ordine di
riga):

- `Schema`: colonna geometria assente o non `Binary`;
- `ResourceLimit`: cella oltre il limite di byte per cella; prenotazione di
  memoria fallita; input invalido con più di 10.000 segmenti (`WorkLimit`:
  il lavoro stimato è il quadrato dei segmenti); coordinate o componenti
  d'uscita oltre i limiti per cella;
- `InvalidPlan`: WKB malformato o con coordinate non finite; riparazione
  che resta invalida; precisione del CRS non valida;
- `Unsupported`: WKB con dimensioni Z/M o SRID; noding non convergente;
  segno d'area non decidibile su coordinate fuori da `[2^-450, 2^450]`
  (`NumericRange`); `PrecisionInsufficient` (sotto, «Precisione»);
- `Internal`: panico di `geo` dentro il kernel, invariante violata,
  validazione OGC che non conclude.

#### Limiti e deviazioni

- Il kernel è quello del laboratorio, qualificato contro GEOS per
  equivalenza semantica, non byte per byte: ordine delle parti, punto
  iniziale e verso degli anelli e scelta fra `Polygon`, `MultiPolygon` e
  `GeometryCollection` possono differire da GEOS a parità di geometria
  ([README, «`geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a
  GEOS verificata, non dimostrata»](../README.md#geomake_valid-geopolygonize-geosplit-equivalenza-a-geos-verificata-non-dimostrata)).
- `LINEWORK` è una funzione dell'insieme dei lati d'ingresso: anelli e parti
  permutati, ruotati o invertiti danno la stessa geometria. Non è il
  pari-dispari su tutti gli anelli: un buco che condivide un lato con la
  shell e ne sporge, o parti sovrapposte, seguono la regola di GEOS.
- «Valido» è la validazione OGC del workspace (quella di `geo` più il
  controllo degli anelli con punta), non `IsValid` di GEOS: dove le due
  divergono, una geometria può essere riparata qui e restituita invariata
  da GEOS, o viceversa.
- Oltre 10.000 segmenti un input invalido si rifiuta (GEOS non aveva
  limite); un input valido passa a ogni dimensione. Il caso peggiore dei
  giri di `LINEWORK` non ha un budget di tempo proprio.
- Elenco completo: [README, «Differenze da GEOS»](../README.md#differenze-da-geos)
  e [README, «Operazioni topologiche in Rust puro»](../README.md#operazioni-topologiche-in-rust-puro).
- Nel runner il passo rende il primo errore, senza diagnostica per riga
  ([README, «Limiti dichiarati del
  runner»](../README.md#limiti-dichiarati-del-runner), voci «Geo senza
  diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

`LINEWORK` non passa dalla griglia di `i_overlay`: l'unico calcolo che
arrotonda è il noding del bordo, e ogni passo successivo è un'operazione
esatta sull'insieme dei lati nodati. Le regole di 1 cm
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)):

- coordinate troppo rade (unità in ultima posizione del modulo massimo
  oltre `p / 64`): `PrecisionInsufficient`, prima di ogni calcolo;
- ogni punto d'incrocio arrotondato del noding sta entro `p / 5` da
  entrambi i segmenti che divide (al più cinque giri, quindi entro `p`);
  oltre, `PrecisionInsufficient`;
- un incrocio arrotondato a meno di `p` da un altro vertice o da un lato
  non incidente: `PrecisionInsufficient`, perché lì la topologia delle
  facce si deciderebbe sotto la precisione. Due incroci arrotondati sullo
  stesso vertice non sono riconosciuti;
- segni e confronti d'area sono esatti; una geometria valida non si tocca.

Sotto la precisione feature d'ingresso più vicine di 1 cm possono fondersi
o cambiare la topologia senza errore, come dichiarato. `p` è la precisione
del CRS della colonna (`Precision::from_crs`).

#### Complessità

Per riga, con `v` i vertici della cella: la validazione OGC del workspace
(scansione sui rettangoli d'ingombro, O(v²) nel caso peggiore). Solo per le
celle invalide, in più: la validazione interna del kernel
(`check_validation` di `geo`, O(v²)), il noding (coppie di segmenti con
filtro sugli inviluppi, quadratico nel caso peggiore), un polygonize per
giro (i giri sono al più i lati nodati: il caso peggiore, anelli
concentrici collegati, è quadratico nei lati per il logaritmo, entro il
tetto di 10.000 segmenti) e la rivalidazione dell'uscita. Memoria O(v) per
la cella in lavorazione, più le celle d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Una farfalla, un quadrato valido e un null.

Passo del piano:

```json
{"out": "risultato", "op": "geo.make_valid", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 2,2 0,0 2,0 0)) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |
| 3 | null |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOLYGON(((0 2,0 0,1 1,0 2)),((1 1,2 0,2 2,1 1))) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |
| 3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.nearest`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_nearest` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:N |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Abbina ogni riga della sinistra alla riga della destra più vicina, con la
distanza planare fra le due geometrie (kernel
`analysis::nearest_matches_validated`, [README, «Operazioni
geo»](../README.md#operazioni-geo)): in caso di pari tutte le destre alla
distanza minima, come `sjoin_nearest` di GeoPandas.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_distance` | numero | assente: nessun limite | finito, `>= 0`, nelle unità del CRS | una riga sinistra il cui vicino è più lontano non ha abbinamenti; un vicino esattamente a `max_distance` vale |

#### Schema

Le colonne della sinistra, invariate, più in coda `__right_index`
(`uint64`, la posizione della riga destra) e `distance` (`float64`, nelle
unità del CRS), entrambe non nullable: ogni riga d'uscita è una coppia
trovata. Anche la colonna geometria della sinistra è non nullable
nell'uscita (una geometria null non ha vicini); fino alla versione 1 del
catalogo le tre si dichiaravano nullable senza che il runner ne emettesse
mai un null. Le colonne della destra non passano: si
ricollegano con `__right_index`. La colonna geometria resta quella della
sinistra. I metadati di schema sono la fusione dei due lati; le proprietà
del contratto (`sorted_by`, `row_count`) si perdono.

#### Righe

Una riga per abbinamento, con le colonne della riga sinistra: per ogni
riga sinistra con geometria non nulla e non vuota, le righe destre (non
nulle, non vuote) alla distanza minima, una di solito, più di una in caso
di pari, quindi l'uscita può superare la sinistra. Una riga sinistra
nulla o vuota, senza destre utilizzabili o con il vicino oltre
`max_distance` non compare: il runner non emette righe con
`__right_index` e `distance` nulli, anche se il contratto li dichiara
nullable. Gli abbinamenti sono al più il limite di righe dell'arco
d'uscita (`max_output_rows` se il passo è un output del piano,
`max_rows_per_edge` altrimenti).

#### Ordine

Per riga sinistra, poi per `__right_index` crescente.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti; `max_distance` negativa o non finita; un metadato di
  schema presente sui due lati con valori diversi;
- `Schema`: `__right_index` o `distance` esiste già nella sinistra; un
  lato senza esattamente una colonna geometria, o con una colonna non
  riconoscibile come geometria WKB (né estensione `geoarrow.wkb` né chiavi
  canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`analysis::nearest_matches_validated`, errore
`AnalysisError`, sulle geometrie già validate), nella categoria del passo
geo indicata fra parentesi:

- `WorkLimitExceeded` (`ResourceLimit`): i confronti della forza bruta,
  righe sinistre non nulle per righe destre non nulle e non vuote,
  superano il quadrato del maggiore fra `max_input_rows` e
  `max_rows_per_edge` (anche se l'indice ne fa meno);
- `ResultLimitExceeded` (`ResourceLimit`): gli abbinamenti superano il
  limite di righe dell'arco (ogni altro errore, il primo in ordine di
  riga, ha la precedenza);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): un
  calcolo di `geo` (la distanza su un candidato) non ha concluso;
- `IndexOverflow` (`InvalidPlan`): un indice non entra in `u64`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Un R-tree sceglie i candidati e scarta una destra solo quando la sua
distanza non può essere il minimo, con un margine appoggiato alla stima
d'errore della distanza di `geo` letta dai sorgenti, non dimostrata; le
geometrie fuori da quella stima non si scartano mai. L'oracolo confronta
il risultato con la forza bruta sui bit ([README, «`geo.nearest`: lo
scarto dell'R-tree si appoggia alla stima d'errore di
`geo`»](../README.md#geonearest-lo-scarto-dellr-tree-si-appoggia-alla-stima-derrore-di-geo)).
Il catalogo dichiara il vincolo `uscita / sinistra`, ma i pari possono dare
più righe della sinistra. Gli abbinamenti dipendono dai dati: il modello di
costo non li prevede, e li limita solo il limite di righe dell'arco
([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Modelli di costo
geo»).

#### Precisione

Nessun calcolo di geometrie e nessuna griglia: la distanza è
`Euclidean.distance` di `geo` in `f64` sulle coordinate d'ingresso, con un
errore di arrotondamento stimato sotto `64 · eps` volte il modulo massimo
delle coordinate su geometrie regolari (circa 0,14 µm con coordinate
fino a 10.000 km), molto sotto 1 cm. I pari sono
uguaglianze esatte dei valori calcolati: due destre la cui distanza vera è
uguale possono non risultare pari se i loro `f64` differiscono nell'ultima
cifra ([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Un R-tree dei rettangoli d'ingombro della destra, O(m log m) per `m`
righe; per ognuna delle `n` righe sinistre, in parallelo, un primo vicino
nell'albero, una finestra di candidati e la distanza sui soli candidati.
Di norma O((n + m) log m) più le distanze; nel caso peggiore (destre
equidistanti da molte sinistre, rettangoli grandi sovrapposti) O(n · m).
Memoria O(m) per l'albero più gli abbinamenti.

#### Memoria

Memoria: da misura v4.

#### Esempio

Il primo pozzo ha due fontane alla stessa distanza, ed esce due volte.

Passo del piano:

```json
{"out": "risultato", "op": "geo.nearest", "in": ["pozzi", "fontane"],
 "config": {}}
```

Ingresso `pozzi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 0) |
| 2 | POINT(10 10) |

Ingresso `fontane` (geometrie `geometry` in EPSG:3857):

| `codice: utf8` | `geometry: geometry` |
| --- | --- |
| a | POINT(-1 0) |
| b | POINT(1 0) |
| c | POINT(10 13) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `__right_index: uint64` | `distance: float64` |
| --- | --- | --- | --- |
| 1 | POINT(0 0) | 0 | 1.0 |
| 1 | POINT(0 0) | 1 | 1.0 |
| 2 | POINT(10 10) | 2 | 3.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.overlay`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_overlay` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:N |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Sovrappone due tabelle di poligoni e ne produce i pezzi, ognuno con la
riga sinistra e la riga destra da cui viene (kernel
`topology::polygon_overlay_validated`; [README, «Operazioni
geo»](../README.md#operazioni-geo)): le intersezioni delle coppie che si
sovrappongono e, secondo `mode`, i resti di ciascun lato fuori dall'altro.
Lavora solo su `Polygon` e `MultiPolygon`.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `mode` | stringa | obbligatorio | `intersection`, `union`, `identity`, `symmetric_difference` | quali pezzi emettere |

Pezzi per `mode`:

- `intersection`: le intersezioni delle coppie;
- `union`: le intersezioni, i resti della sinistra e i resti della destra;
- `identity`: le intersezioni e i resti della sinistra (la sinistra resta
  coperta per intero, la destra solo dove la tocca);
- `symmetric_difference`: i resti della sinistra e quelli della destra.

Il resto di una riga è la riga meno l'unione di tutte le righe dell'altro
lato; se l'altro lato non ha righe, è la riga invariata.

#### Schema

Tre colonne, in quest'ordine: la colonna geometria della sinistra (stesso
nome e CRS, `Binary` GeoArrow-WKB, non nullable (un pezzo non è mai vuoto), XY, tipi dichiarati `Polygon`
e `MultiPolygon` `exact`, senza le chiavi dei tipi ereditate), poi
`__left_index` e `__right_index`, `uint64` nullable: le posizioni delle
righe d'origine, nulle dove il pezzo è un resto dell'altro lato. Le
colonne attributo dei due lati non passano: si ricollegano con gli indici.
I metadati di schema sono la fusione dei due lati; le proprietà del
contratto si perdono.

#### Righe

Una riga per pezzo, da 0 a molte per riga d'ingresso. Le coppie candidate
si trovano con il join spaziale `intersects` sui rettangoli d'ingombro e
si confermano con il predicato esatto; una coppia che si tocca solo sul
bordo dà un'intersezione vuota, che non si emette. Nessun pezzo vuoto esce.
Una riga sinistra coperta del tutto dalla destra non ha resto. Le righe
con la geometria nulla, da un lato o dall'altro, non entrano nel kernel e
non danno pezzi; `__left_index` e `__right_index` sono le posizioni delle
righe negli ingressi, contando anche le nulle. Le coppie candidate e i
pezzi sono ciascuno al più il limite di righe dell'arco d'uscita
(`max_output_rows` se il passo è un output del piano, `max_rows_per_edge`
altrimenti).

#### Ordine

Prima le intersezioni, in ordine `(sinistra, destra)` crescente; poi i
resti della sinistra in ordine di riga; poi i resti della destra in ordine
di riga.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti, `mode` assente o fuori elenco; un metadato di schema
  presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`topology::polygon_overlay_validated`, errore
`TopologyError`, sulle geometrie già validate: restano le validazioni OGC
delle unioni e dei pezzi), nella categoria del passo geo indicata fra
parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): un ingresso che il join delle
  coppie candidate rifiuta, un'unione o un pezzo che non supera la
  validazione OGC;
- `ResourceLimit` (`ResourceLimit`): le coppie candidate superano il
  limite di righe dell'arco (`candidate_pairs`), o i pezzi lo superano
  (`overlay_results`);
- `IndexOverflow` (`InvalidPlan`): un indice non entra in `u64`;
- `PrecisionInsufficient` (`Unsupported`): la griglia di un overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): una
  validazione, un predicato o un overlay di `geo` non ha concluso.

Un indice di pezzo che non corrisponde a una riga d'ingresso è
`Internal`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Solo parti poligonali: le intersezioni che si riducono a linee o punti
sono escluse, come `keep_geom_type=True` di GeoPandas. Fino alla versione 1
del catalogo il superamento del limite delle coppie candidate usciva come
`InvalidGeometry`, e i due limiti avevano la categoria `InvalidPlan`. I
pezzi dipendono dai
dati: il modello di costo non li prevede, e li limita solo il limite di
righe dell'arco ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Modelli di
costo geo»). Nessun controllo a posteriori del risultato contro
gli ingressi ([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra.
Ogni intersezione è un overlay con la griglia entro mezzo centimetro; ogni
resto sono due overlay in catena (l'unione dell'altro lato, poi la
differenza), ognuno entro un quarto di centimetro. Ogni griglia è
controllata prima del calcolo sull'ingombro dei suoi operandi; oltre, o
con coordinate troppo rade per il centimetro, `PrecisionInsufficient` e
nessun calcolo. Parti più sottili di 1 cm possono sparire o fondersi senza
errore; vedi [README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

#### Complessità

Join delle coppie candidate con un R-tree, O((n + m) log m) più le coppie
trovate (nel caso peggiore O(n · m)); un overlay per coppia; per i resti
un'unione di ciascun lato e una differenza per riga contro l'unione intera
dell'altro lato. Più la validazione OGC di ingressi, unioni e pezzi.
Memoria: i due lati, le unioni e i pezzi.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.overlay", "in": ["lotti", "vincoli"],
 "config": {"mode": "union"}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |

Ingresso `vincoli` (geometrie `geometry` in EPSG:3857):

| `zona: utf8` | `geometry: geometry` |
| --- | --- |
| A | POLYGON((1 0,3 0,3 2,1 2,1 0)) |

Uscita `risultato`:

| `geometry: geometry` | `__left_index: uint64` | `__right_index: uint64` |
| --- | --- | --- |
| MULTIPOLYGON(((1 2,1 0,2 0,2 2,1 2))) | 0 | 0 |
| MULTIPOLYGON(((0 2,0 0,1 0,1 2,0 2))) | 0 | null |
| MULTIPOLYGON(((2 2,2 0,3 0,3 2,2 2))) | null | 0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.perimeter`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_perimeter` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | misura terminale |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `float64` con il perimetro planare della geometria di
ogni riga, nelle unità del CRS. Il kernel è quello di
[`geo.length`](#geolength): un poligono vale anello esterno più buchi, ma
anche una linea vale la sua lunghezza (non 0), una multi-geometria o una
collezione la somma delle parti, un punto 0.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `perimeter` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: un perimetro per riga; una geometria nulla dà una cella nulla. Il
runner chiama `operations::length`, di cui il kernel `operations::perimeter`
è un alias: stessa semantica, stessi errori.

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare;
- `InvalidPlan`: campi sconosciuti nella config, `output_column` vuoto o di soli spazi.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria non supera la validazione
  OGC;
- `Internal` (`ValidazioneNonConclusa`, `CalcoloNonConcluso`): la
  validazione OGC o il calcolo di `geo` vanno in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Il perimetro di una linea è la sua lunghezza, come in Manipola
(`GeoSeries.length`), dove `ST_Perimeter` di PostGIS rende 0.
Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`: il perimetro è la
somma in `f64` delle lunghezze dei segmenti calcolate da `geo`, senza un
bilancio d'errore dichiarato rispetto alla regola di 1 cm
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) per la somma, più la validazione OGC
dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.perimeter", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,4 2,4 4,2 4,2 2)) |
| 2 | LINESTRING(0 0,3 4) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `perimeter: float64` |
| --- | --- | --- |
| 1 | POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,4 2,4 4,2 4,2 2)) | 48.0 |
| 2 | LINESTRING(0 0,3 4) | 5.0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.point_on_surface`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_point_on_surface` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sostituisce ogni geometria con un `Point` che le appartiene, nella stessa
colonna. Per i poligoni è il punto medio del tratto interno più lungo di
una linea orizzontale a metà altezza dell'ingombro, spostata se passa per
un vertice (se nessun tratto risulta interno, il primo vertice); in un
`MultiPolygon` vince il tratto più lungo fra le parti; per le linee il vertice non estremo più vicino al
centroide, o il primo se la linea ha due vertici; per i punti quello più
vicino al centroide. A differenza del centroide il punto sta sempre sulla
geometria. Per una geometria vuota il kernel non dà un punto.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`Point`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

#### Righe

1:1: il runner chiama il kernel (`operations::point_on_surface`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).
Una geometria senza punto interno (vuota) dà null: la colonna d'uscita ha
la nullabilità di quella d'ingresso, e se non ammette null il passo si
rifiuta con `InvalidPlan`.

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`OperationError`, che il runner porta in `PlenoraError`:
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, `Unsupported` per `PrecisionInsufficient`,
`InvalidPlan` per le altre):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o
  `interior_point` di `geo` (che passa da `relate`, capace di andare in
  panico anche su geometrie valide) vanno in panico dentro la barriera (il
  messaggio porta solo la forma del payload).

Dopo il kernel, `InvalidPlan` per una geometria vuota in una colonna che il
contratto dichiara non nullable (sopra, «Righe»). Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Il punto è quello di `interior_point` di `geo`, che non coincide
necessariamente con quello di `ST_PointOnSurface` di PostGIS. Le coordinate
`-0.0` si portano a `0.0` su una copia prima del calcolo: con `-0.0` e
`0.0` insieme la scansione di `geo` 0.33.1 rende in release un punto
sbagliato senza errore.

#### Precisione

Nessuna griglia e nessun rifiuto `PrecisionInsufficient`. Per linee e
punti il risultato è un vertice d'ingresso, esatto; per i poligoni le
coordinate sono calcolate in `f64` da `geo` (punto medio del tratto) e
`relate` conferma che il punto sta dentro: la garanzia è topologica, senza
un bilancio metrico rispetto alla regola di 1 cm
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per un poligono di n vertici: la scansione di `geo` incrocia la linea con
tutti i lati (sweep line, O(n log n) più le intersezioni) e verifica i
tratti, dal più lungo, con `relate` finché uno è interno; per linee e punti
O(n). Più la validazione OGC
dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.point_on_surface", "in": ["lotti"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 0,4 2,0 2,0 0)) |
| 2 | LINESTRING(0 0,2 0,4 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(2 1) |
| 2 | POINT(2 0) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.polygon_builder`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_polygon_builder` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | N:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Costruisce un `Polygon` senza buchi dai punti della colonna geometria,
nell'ordine delle righe: i punti sono l'anello esterno, chiuso da sé se
l'ultimo non ripete il primo. La config è vuota: non ci sono colonne di
gruppo né d'ordine, quindi per contratto tutta la tabella diventa una sola
geometria, e le colonne attributo si perdono.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

L'uscita ha la sola colonna geometria: stesso nome, CRS, dimensioni e
metadati di campo, nullable. Le altre colonne non passano; i metadati di
schema sì. Le proprietà del contratto (`sorted_by`, `row_count`) cadono, e
la dichiarazione dei tipi ereditata si toglie senza sostituirla (nessuna
dichiarazione).

#### Righe

Aggregazione: tutte le righe in una. Il kernel
(`construction::polygon_from_ordered_points`) riceve il gruppo ordinato
delle geometrie, salta quelle assenti (`None`) e, con meno di tre punti,
non costruisce il poligono (`None`, non un errore). Il runner gli passa
tutte le celle della colonna nell'ordine delle righe, una cella nulla come
`None`, e rende sempre una riga: il poligono, o una geometria nulla quando
il poligono manca (anche per una tabella vuota).

#### Ordine

I vertici seguono l'ordine delle righe d'ingresso: un ordine diverso dà un
poligono diverso, o un anello che si incrocia e quindi un errore.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: config diversa da `{}`;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel, sul gruppo, con errore `ConstructionError` che il runner
traduce così: `ValidazioneNonConclusa` diventa `Internal`, le altre
`InvalidPlan`:

- `ExpectedPoint`: una geometria non è un `Point` (il messaggio dà la
  posizione nel gruppo, assenti comprese, e il tipo trovato);
- `InvalidOutput`: il poligono non supera la validazione OGC (anello che si
  incrocia, punti collineari o troppo pochi punti distinti);
- `ValidazioneNonConclusa`: la validazione OGC va in panico dentro la
  barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Non ci sono colonne di gruppo né d'ordine (il kernel lavora su un gruppo
già ordinato, ma nessun parametro lo forma): un poligono per gruppo non si
può chiedere. Nessun buco,
nessun riordino dei punti, nessuna riparazione dell'anello. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Esatta: i vertici sono le coordinate dei punti, copiate
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sulle righe più la validazione OGC del poligono (sub-quadratica
nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n): tutta la colonna diventa una geometria.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.polygon_builder", "in": ["vertici"],
 "config": {}}
```

Ingresso `vertici` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 0) |
| 2 | POINT(4 0) |
| 3 | POINT(4 3) |

Uscita `risultato`:

| `geometry: geometry` |
| --- |
| POLYGON((0 0,4 0,4 3,0 0)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.polygonize`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `polygonize` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione non interrompibile |
| forma del risultato | da tutto l'ingresso a molte righe |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 3, kernel 3 |

#### Che cosa fa

Costruisce i poligoni racchiusi dalle linee di tutta la tabella: raccoglie
le geometrie lineari non nulle di tutte le righe, le noda nei punti
d'incrocio (salvo `node_input: false`) ed estrae le facce del grafo
planare. Ogni faccia diventa una riga `polygon`; le linee che non chiudono
una faccia escono come residui, classificati come in GEOS: `cut_edge` (lato
con la stessa faccia da entrambe le parti), `dangle` (tratto pendente con un
estremo libero), `invalid_ring` (anello che non forma un poligono valido).
Gli attributi delle righe d'ingresso non passano.

La semantica a livello di tabella è quella dell'esecuzione Arrow
(`rust_backend::arrow::polygonize_batches`), che il runner chiama su tutta
la tabella con la precisione del CRS della colonna e il limite di righe
dell'arco d'uscita ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `node_input` | booleano | `true` | `true`, `false` | noda le linee negli incroci prima di costruire il grafo |
| `require_complete` | booleano | `false` | `true`, `false` | fallisce se restano residui, invece di emetterli |

Con `node_input: false` il grafo usa i segmenti così come sono: due linee
che si incrociano senza un vertice comune non si dividono, e le linee
duplicate escono come `cut_edge` nell'ordine d'ingresso.

#### Schema

Due colonne, in quest'ordine:

- la colonna geometria, con il nome e tutti i metadati del campo
  d'ingresso (CRS, dimensioni, encoding, lineage) tranne la dichiarazione
  dei tipi, non nullable (una riga per faccia o residuo); il contratto
  dichiara i tipi `exact`
  `LineString` e `Polygon`;
- `__class`, `utf8` non nullable: `polygon`, `cut_edge`, `dangle`,
  `invalid_ring`.

Le altre colonne dell'ingresso spariscono; i metadati di schema restano.
Nessuna proprietà del contratto sopravvive (`sorted_by` e `row_count`
cadono).

#### Righe

Aggregazione dell'intera tabella: da 0 righe (ingresso vuoto o tutto
nullo) a una per faccia e per residuo. Le celle nulle si saltano; nessuna
riga d'uscita ha geometria nulla. Un poligono
con buchi resta una riga sola.

#### Ordine

Prima i poligoni, nell'ordine di estrazione delle facce (chiavi di
coordinata ordinate), con l'anello esterno antiorario; poi i residui per
classe (`cut_edge`, `dangle`, `invalid_ring`), ognuna nell'ordine del grafo.
Il contenuto coincide con GEOS sui casi qualificati, l'ordine no.
Deterministico: stesso ingresso, stesse righe nello stesso ordine.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti o valori non booleani;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto, o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. La validità OGC non si controlla qui: la
controlla l'esecuzione Arrow.

Poi l'esecuzione Arrow:

- `Schema`: colonna geometria assente o non `Binary`;
- `ResourceLimit`: cella oltre il limite di byte per cella; prenotazione di
  memoria fallita; più di 100.000.000 coordinate in ingresso o in uscita;
  più di 100.000.000 coppie di segmenti esaminate dal noding; righe
  d'uscita oltre il limite di righe dell'arco (`max_output_rows` per un
  output del piano, `max_rows_per_edge` altrimenti; contato anche sulle
  facce intermedie);
- `InvalidPlan`: una cella che non è `LineString`, `MultiLineString` o
  collezione di linee; WKB malformato o OGC-invalido;
  `require_complete` con residui (il messaggio riporta il numero di residui
  per classe); una faccia che non supera la validazione;
- `Unsupported`: WKB con dimensioni Z/M o SRID; noding non convergente;
  segno o confronto d'area non decidibile su coordinate fuori da
  `[2^-450, 2^450]`; `PrecisionInsufficient` (sotto, «Precisione»);
- `Internal`: panico di `geo` dentro il kernel, invariante violata.

#### Limiti e deviazioni

- Kernel del laboratorio qualificato contro GEOS per equivalenza semantica
  (stesse facce, stessi residui, stessa area), non per identità
  ([README, «`geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a
  GEOS verificata, non dimostrata»](../README.md#geomake_valid-geopolygonize-geosplit-equivalenza-a-geos-verificata-non-dimostrata)).
- Forma canonica solo quando gli incroci sono esattamente rappresentabili:
  altrimenti, su ingresso permutato o invertito, classi e aree coincidono ma
  i bit di un incrocio, il segno di uno zero e l'ordine delle linee
  duplicate seguono l'ordine d'ingresso.
- Il lavoro di noding conta le coppie di segmenti esaminate, addebitate
  mentre accadono (GEOS stimava prima il quadrato dei segmenti); i limiti
  d'uscita valgono anche sulle facce intermedie.
- Segni esatti dove GEOS non lo è: una faccia degenere solo per la
  precisione di GEOS resta un poligono.
- Elenco completo: [README, «Differenze da GEOS»](../README.md#differenze-da-geos).
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Nessuna griglia di `i_overlay`: il solo calcolo che sposta punti è il
noding
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)):

- coordinate troppo rade (unità in ultima posizione del modulo massimo
  oltre `p / 64`): `PrecisionInsufficient`, prima del noding;
- ogni punto d'incrocio, calcolato in doppia-doppia e arrotondato in `f64`,
  deve stare entro `p / 5` da entrambi i segmenti che divide; il noding si
  ripete al più cinque volte, quindi il grafo dista dalle linee d'ingresso
  al più `p`. Oltre, `PrecisionInsufficient`;
- orientamento delle facce e annidamento per area sono esatti; restano
  decisioni in `f64` entro la precisione il punto medio dei lati che
  ricuce i buchi, il punto interno di `geo` per l'annidamento e lo
  spareggio fra archi sovrapposti (solo senza noding).

Con `node_input: false` nessun punto è calcolato: le facce hanno le
coordinate d'ingresso. `p` è la precisione del CRS della colonna
(`Precision::from_crs`, 1 cm a terra).

#### Complessità

`n` segmenti in tutto: noding con filtro sugli inviluppi, O(n²) coppie nel
caso peggiore, entro il tetto di 100.000.000 coppie; estrazione delle facce
O(e log e) sui lati nodati `e`. Memoria O(n) per tutte le linee della
tabella, raccolte prima del calcolo (classe bloccante), più le righe
d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Un quadrato chiuso da tre linee, con un tratto pendente.

Passo del piano:

```json
{"out": "risultato", "op": "geo.polygonize", "in": ["linee"],
 "config": {}}
```

Ingresso `linee` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,10 0) |
| 2 | LINESTRING(10 0,10 10) |
| 3 | LINESTRING(10 10,0 10,0 0) |
| 4 | LINESTRING(10 10,15 10) |

Uscita `risultato`:

| `geometry: geometry` | `__class: utf8` |
| --- | --- |
| POLYGON((0 10,0 0,10 0,10 10,0 10)) | polygon |
| LINESTRING(10 10,15 10) | dangle |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_contains`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_contains` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) contiene la geometria costante `other_wkb` (B): nessun punto di B è fuori da A e almeno un punto dell'interno di B è nell'interno di A.

Maschera DE-9IM `T*****FF*`: interno/interno non vuota, esterno di A/interno di B ed esterno di A/confine di B vuote. Un B tutto sul confine di A (un punto sul lato di un poligono, un lato dell'anello) non è contenuto: per questo c'è [`geo.predicate_covers`](#geopredicate_covers). Una geometria contiene se stessa. Il contrario di [`geo.predicate_within`](#geopredicate_within) a operandi scambiati. Con una geometria vuota è falso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_contains` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Contains)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il punto `(10 5)` è interno al quadrato grande e sul lato di quello piccolo; B è `POINT(10 5)`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_contains", "in": ["luoghi"],
 "config": {"other_wkb": "010100000000000000000024400000000000001440"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,20 0,20 20,0 20,0 0)) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |
| 3 | POLYGON((20 0,30 0,30 10,20 10,20 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_contains: bool` |
| --- | --- | --- |
| 1 | POLYGON((0 0,20 0,20 20,0 20,0 0)) | true |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) | false |
| 3 | POLYGON((20 0,30 0,30 10,20 10,20 0)) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_contains_properly`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_contains_properly` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) contiene propriamente la geometria costante `other_wkb` (B): ogni punto di B è nell'interno di A, senza toccarne il confine.

Maschera DE-9IM `T**FF*FF*`: interno/interno non vuota; confine di A vuoto contro interno e confine di B; esterno di A vuoto contro interno e confine di B. Più forte di [`geo.predicate_contains`](#geopredicate_contains): basta che B tocchi il confine di A perché sia falso, quindi una geometria non contiene propriamente se stessa. Con una geometria vuota è falso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_contains_properly` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::ContainsProperly)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il quadrato uguale e quello che ne condivide due lati lo contengono, ma non propriamente; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_contains_properly", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((-5 -5,15 -5,15 15,-5 15,-5 -5)) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |
| 3 | POLYGON((0 0,20 0,20 20,0 20,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_contains_properly: bool` |
| --- | --- | --- |
| 1 | POLYGON((-5 -5,15 -5,15 15,-5 15,-5 -5)) | true |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) | false |
| 3 | POLYGON((0 0,20 0,20 20,0 20,0 0)) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_covered_by`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_covered_by` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) è coperta dalla geometria costante `other_wkb` (B): nessun punto di A è fuori da B, confine di B compreso.

Maschera DE-9IM: vero se interno di A/esterno di B e confine di A/esterno di B sono vuote e almeno una fra interno/interno, interno/confine, confine/interno, confine/confine non lo è (`T*F**F***`, `*TF**F***`, `**FT*F***`, `**F*TF***`). È [`geo.predicate_covers`](#geopredicate_covers) a operandi scambiati; a differenza di [`geo.predicate_within`](#geopredicate_within), un A tutto sul confine di B è coperto. Con una geometria vuota (anche A soltanto) è falso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_covered_by` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::CoveredBy)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il punto sul lato è coperto, anche se non è dentro; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_covered_by", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(5 5) |
| 2 | POINT(10 5) |
| 3 | POINT(20 5) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_covered_by: bool` |
| --- | --- | --- |
| 1 | POINT(5 5) | true |
| 2 | POINT(10 5) | true |
| 3 | POINT(20 5) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_covers`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_covers` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) copre la geometria costante `other_wkb` (B): nessun punto di B è fuori da A, confine di A compreso.

Maschera DE-9IM: vero se esterno di A/interno di B ed esterno di A/confine di B sono vuote e almeno una fra interno/interno, interno/confine, confine/interno, confine/confine non lo è (`T*****FF*`, `*T****FF*`, `***T**FF*`, `****T*FF*`). A differenza di [`geo.predicate_contains`](#geopredicate_contains), un B tutto sul confine di A è coperto. Una geometria copre se stessa. Il contrario di [`geo.predicate_covered_by`](#geopredicate_covered_by) a operandi scambiati. Con una geometria vuota (anche B soltanto) è falso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_covers` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Covers)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il punto sul lato del quadrato piccolo è coperto, anche se non è contenuto; B è `POINT(10 5)`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_covers", "in": ["luoghi"],
 "config": {"other_wkb": "010100000000000000000024400000000000001440"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,20 0,20 20,0 20,0 0)) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |
| 3 | POLYGON((20 0,30 0,30 10,20 10,20 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_covers: bool` |
| --- | --- | --- |
| 1 | POLYGON((0 0,20 0,20 20,0 20,0 0)) | true |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) | true |
| 3 | POLYGON((20 0,30 0,30 10,20 10,20 0)) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_crosses`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_crosses` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) attraversa la geometria costante `other_wkb` (B): gli interni si intersecano, ma solo in parte.

Con le dimensioni ricavate dalla matrice (quella degli interni): se dim(A) < dim(B), interno/interno non vuota e parte dell'interno di A fuori da B (`T*T******`); se dim(A) > dim(B), interno/interno non vuota e parte dell'interno di B fuori da A (`T*****T**`); fra due linee, interni che si toccano solo in punti (`0********`). Falso fra due poligoni, fra due punti e con una geometria vuota. Una linea tutta dentro un poligono non lo attraversa (è [`geo.predicate_within`](#geopredicate_within)). Simmetrico.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_crosses` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Crosses)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Solo la linea che entra da fuori attraversa il quadrato; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_crosses", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(-5 5,5 5) |
| 2 | LINESTRING(2 2,8 8) |
| 3 | LINESTRING(-5 -5,-1 -1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_crosses: bool` |
| --- | --- | --- |
| 1 | LINESTRING(-5 5,5 5) | true |
| 2 | LINESTRING(2 2,8 8) | false |
| 3 | LINESTRING(-5 -5,-1 -1) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_disjoint`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_disjoint` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) e la geometria costante `other_wkb` (B) non hanno alcun punto in comune. È la negazione di [`geo.predicate_intersects`](#geopredicate_intersects).

Maschera DE-9IM `FF*FF****`: vuote le celle interno/interno, interno/confine, confine/interno, confine/confine. Un punto sul confine di un poligono non ne è disgiunto. Simmetrico. Con una geometria vuota (da una parte o dall'altra) è vero.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_disjoint` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Disjoint)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Solo il punto lontano è disgiunto; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_disjoint", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(5 5) |
| 2 | POINT(10 5) |
| 3 | POINT(20 5) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_disjoint: bool` |
| --- | --- | --- |
| 1 | POINT(5 5) | false |
| 2 | POINT(10 5) | false |
| 3 | POINT(20 5) | true |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_equals_topo`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_equals_topo` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) e la geometria costante `other_wkb` (B) sono lo stesso insieme di punti, qualunque sia la loro rappresentazione: vertici in più su un lato, punto iniziale e verso degli anelli, ordine delle parti.

Maschera DE-9IM `T*F**FFF*`: interno/interno non vuota; interno e confine di ciascuna mai nell'esterno dell'altra. Non confronta i vertici: `LINESTRING(0 0,2 0)` e `LINESTRING(0 0,1 0,2 0)` sono uguali, e lo sono anche due geometrie di tipo diverso con gli stessi punti. Due geometrie vuote sono uguali (anche di tipo diverso); una vuota e una no, mai. Simmetrico.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_equals_topo` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::EqualsTopo)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Punto iniziale diverso e vertice in più sul lato non contano; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_equals_topo", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((10 0,10 10,0 10,0 0,10 0)) |
| 2 | POLYGON((0 0,5 0,10 0,10 10,0 10,0 0)) |
| 3 | POLYGON((0 0,10 0,10 5,0 5,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_equals_topo: bool` |
| --- | --- | --- |
| 1 | POLYGON((10 0,10 10,0 10,0 0,10 0)) | true |
| 2 | POLYGON((0 0,5 0,10 0,10 10,0 10,0 0)) | true |
| 3 | POLYGON((0 0,10 0,10 5,0 5,0 0)) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_intersects`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_intersects` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) e la geometria costante `other_wkb` (B) hanno almeno un punto in comune: interno o confine dell'una che tocca interno o confine dell'altra. È la negazione di [`geo.predicate_disjoint`](#geopredicate_disjoint).

Maschera DE-9IM: vero se è non vuota almeno una fra le celle interno/interno, interno/confine, confine/interno, confine/confine (`T********`, `*T*******`, `***T*****`, `****T****`). Un punto sul confine di un poligono lo interseca. Simmetrico. Con una geometria vuota (da una parte o dall'altra) è falso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_intersects` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Intersects)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il punto sul lato del quadrato lo interseca; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_intersects", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(5 5) |
| 2 | POINT(10 5) |
| 3 | POINT(20 5) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_intersects: bool` |
| --- | --- | --- |
| 1 | POINT(5 5) | true |
| 2 | POINT(10 5) | true |
| 3 | POINT(20 5) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_overlaps`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_overlaps` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) e la geometria costante `other_wkb` (B) si sovrappongono: stessa dimensione, interni che si intersecano, e ciascuna con punti interni fuori dall'altra.

Con le dimensioni ricavate dalla matrice: fra due linee, interno/interno di dimensione 1 (un tratto in comune) e parte dell'interno di ciascuna fuori dall'altra (`1*T***T**`); fra punti o fra poligoni `T*T***T**`. Falso fra dimensioni diverse, quando una copre l'altra, quando si toccano solo sul confine e con una geometria vuota. Simmetrico.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_overlaps` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Overlaps)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il quadrato interno non si sovrappone (è coperto), quello adiacente si tocca soltanto; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_overlaps", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((5 5,15 5,15 15,5 15,5 5)) |
| 2 | POLYGON((2 2,8 2,8 8,2 8,2 2)) |
| 3 | POLYGON((10 0,20 0,20 10,10 10,10 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_overlaps: bool` |
| --- | --- | --- |
| 1 | POLYGON((5 5,15 5,15 15,5 15,5 5)) | true |
| 2 | POLYGON((2 2,8 2,8 8,2 8,2 2)) | false |
| 3 | POLYGON((10 0,20 0,20 10,10 10,10 0)) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_touches`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_touches` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) e la geometria costante `other_wkb` (B) si toccano soltanto: hanno punti in comune, ma solo sui confini, mai fra gli interni.

Maschera DE-9IM: interno/interno vuota e almeno una fra interno/confine, confine/interno, confine/confine non vuota (`FT*******`, `F**T*****`, `F***T****`). Due poligoni con un lato in comune si toccano; un punto sul lato di un poligono lo tocca. Fra due punti è sempre falso (un punto non ha confine). Simmetrico. Con una geometria vuota è falso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_touches` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Touches)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il punto interno non tocca soltanto: entra; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_touches", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(10 5) |
| 2 | POLYGON((10 0,20 0,20 10,10 10,10 0)) |
| 3 | POINT(5 5) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_touches: bool` |
| --- | --- | --- |
| 1 | POINT(10 5) | true |
| 2 | POLYGON((10 0,20 0,20 10,10 10,10 0)) | true |
| 3 | POINT(5 5) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.predicate_within`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `predicate_within` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `bool` che dice se la geometria della riga (A) sta dentro la geometria costante `other_wkb` (B): nessun punto di A è fuori da B e almeno un punto dell'interno di A è nell'interno di B.

Maschera DE-9IM `T*F**F***`: interno/interno non vuota, interno di A/esterno di B e confine di A/esterno di B vuote. È [`geo.predicate_contains`](#geopredicate_contains) a operandi scambiati: un A tutto sul confine di B (un punto sul lato) non è dentro; per questo c'è [`geo.predicate_covered_by`](#geopredicate_covered_by). Una geometria sta dentro se stessa. Con una geometria vuota è falso.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la geometria B, uguale per tutte le righe |
| `output_column` | stringa | `predicate_within` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi, una volta per piano:
struttura (conteggi, anelli chiusi, coordinate finite), validità OGC e
dominio del CRS. Il kernel la rivaluta comunque a ogni riga.

#### Schema

Aggiunge in coda `output_column`, `bool`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le altre
colonne restano nell'ordine e con i loro metadati; la colonna geometria
resta com'è (tipi dichiarati, CRS, dimensioni `xy`). Metadati di schema e
proprietà del contratto (`sorted_by`, `row_count`) si conservano.

#### Righe

1:1: una cella per riga. Per il contratto `other_wkb` è il secondo
operando: la geometria della riga è A, `other_wkb` è B, e il kernel valuta
`predicates::evaluate(A, B, SpatialPredicate::Within)` su ogni riga. Una
geometria nulla dà una cella nulla, senza chiamare il kernel; una
geometria vuota dà il verdetto del predicato sul vuoto (vedi sopra). Le
righe sono indipendenti: il runner le calcola in parallelo e rende il
primo errore in ordine di riga
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `other_wkb` non valida
  per l'OGC; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare (requisito del catalogo); una coordinata di
  `other_wkb` fuori dal dominio di validità del CRS;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione, il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, su ogni cella non nulla:

- `InvalidPlan` o `Unsupported`: la cella non è WKB strutturalmente
  valido (`Unsupported` per coordinate Z/M o SRID);
- `ResourceLimit`: la cella supera il limite di byte per cella;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto dell'ingresso dichiara un elenco di tipi
  geometrici e la cella è di un altro tipo.

Poi il kernel (`predicates::evaluate`), con l'errore tradotto nella
categoria di `PlenoraError` ([README, «Operazioni geo»](../README.md#operazioni-geo),
voce «Errori»):

- `InvalidPlan`: `NonFiniteCoordinate` o `InvalidGeometry` (geometria
  della riga non valida per l'OGC; `other_wkb` l'ha già esclusa
  l'analisi);
- `Internal`: `ValidazioneNonConclusa` se la validazione non conclude,
  `CalcoloNonConcluso` se `relate` di `geo` va in panico su due geometrie
  valide (per esempio una `GeometryCollection` con membri poligonali che
  si sovrappongono).

#### Limiti e deviazioni

- Il secondo operando è una geometria costante della config, non una
  seconda colonna: un input ha una sola colonna geometria.
- La matrice è quella di `relate` di `geo` 0.33.1, non di GEOS. Il confine
  segue la regola mod-2 dell'OGC: l'estremo condiviso da un numero pari di
  linee di una `MultiLineString` è interno, una linea chiusa non ha
  confine. Le collezioni non si uniscono prima del confronto: il lato
  condiviso da due membri poligonali adiacenti di una `GeometryCollection`
  resta confine, e membri poligonali che si sovrappongono possono far
  fallire il calcolo.
- Le due geometrie devono essere valide per l'OGC: una geometria non
  valida è un errore, non un verdetto. La validazione di entrambe si
  ripete a ogni valutazione ([README, «Validazione OGC: la ricerca delle
  auto-intersezioni non è quella di `geo`, il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
- Si calcola sempre la matrice intera: nessuna scorciatoia per il singolo
  predicato.
- Nessuna diagnostica per riga: un errore è il primo in ordine di riga,
  e il suo indice è una riga dell'ingresso del passo, non della sorgente
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Geo senza diagnostica per riga»). Il costo in memoria è una
  previsione del modello geo misurato (voce «Modelli di costo geo»).

#### Precisione

Nessuna tolleranza e nessun rifiuto `PrecisionInsufficient`: il verdetto
viene dalla matrice DE-9IM calcolata sulle coordinate `f64` così come
sono, con orientazioni esatte. Due geometrie distanti meno di 1 cm non si
toccano. I punti d'incrocio fra lati che `relate` calcola sono arrotondati
in `f64`: su lati distinti più vicini della precisione (quasi coincidenti,
un vertice a pochi ulp da un lato) il verdetto può dipendere da
quell'arrotondamento, fuori dalla garanzia di 1 cm ([README, «Precisione
delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per riga, con `n` e `m` i vertici della riga e di `other_wkb`: `relate`
indicizza i lati con un R-tree, O((n + m) log(n + m)) tipico e O(n·m) nel
caso peggiore (molti lati che si incrociano); in più la validazione OGC di
entrambe, O(n²) e O(m²) nel caso peggiore. Memoria O(n + m).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il punto sul lato non è dentro; il mezzo quadrato, che ne condivide tre lati, sì; B è `POLYGON((0 0,10 0,10 10,0 10,0 0))`.

Passo del piano:

```json
{"out": "risultato", "op": "geo.predicate_within", "in": ["luoghi"],
 "config": {"other_wkb": "010300000001000000050000000000000000000000000000000000000000000000000024400000000000000000000000000000244000000000000024400000000000000000000000000000244000000000000000000000000000000000"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(5 5) |
| 2 | POINT(10 5) |
| 3 | POLYGON((0 0,10 0,10 5,0 5,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `predicate_within: bool` |
| --- | --- | --- |
| 1 | POINT(5 5) | true |
| 2 | POINT(10 5) | false |
| 3 | POLYGON((0 0,10 0,10 5,0 5,0 0)) | true |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.reproject`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_reproject` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione non interrompibile |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS d'origine e di destinazione risolti |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Riproietta la colonna geometria dal suo CRS a `target_crs`, fra i CRS
della tabella integrata, in Rust puro: proiezione inversa, cambio di
datum lungo un percorso di trasformazioni EPSG, proiezione diretta. Ogni
lato si densifica perché resti entro mezza precisione dall'immagine
esatta del lato sorgente; tipo e struttura di ogni geometria non
cambiano. Il quadro completo è in
[README, «Riproiezione»](../README.md#riproiezione).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `target_crs` | stringa | obbligatorio | identificatore della tabella integrata (`EPSG:<codice>`, `OGC:CRS84`, forme URN) | CRS d'arrivo |
| `accuratezza_accettata_m` | numero | nessuna | finito, non negativo, con effetto | accuratezza in metri accettata per un cambio di datum oltre 1 cm |
| `trasformazioni` | lista di interi | nessuna | codici EPSG che formano un percorso fra i due datum | percorso imposto, nell'ordine, per tutte le geometrie |
| `griglie` | lista di oggetti | `[]` | `{"trasformazione": <codice EPSG NTv2>, "file": <percorso>}`, codici distinti, `file` non vuoto, al più 4096 byte, senza NUL | griglie NTv2 fornite dall'utente |
| `convenzione_wgs84_etrs89` | booleano | `true` | `true`, `false`, solo se un percorso passa da EPSG:1149 | WGS 84 e la famiglia ETRS89 equivalenti per convenzione |

Parametri scritti senza effetto si rifiutano: `accuratezza_accettata_m`
quando ogni percorso ammesso sta già entro 1 cm, una griglia che nessun
percorso ammesso usa, `convenzione_wgs84_etrs89` (con l'uno o l'altro
valore) su una coppia che non passa da ETRS89 to WGS 84 (1). I percorsi
imposti da `trasformazioni` restano soggetti alla regola dell'accuratezza
([README, «La regola dell'accuratezza»](../README.md#la-regola-dellaccuratezza)).
L'analisi non legge i file delle griglie: li legge l'esecuzione.

#### Schema

Stesse colonne, tipi, nullabilità e posizione; stessi tipi geometrici
dichiarati, dimensioni (XY) e `FieldId`; metadati di schema e proprietà
del contratto (`sorted_by`, `row_count`) conservati. Cambia il campo
geometria: il CRS del contratto diventa il target, il metadato `geo`
dichiara il target (dimensioni ed encoding invariati), le chiavi
canoniche CRS della sorgente (`plenora.geometry.crs_*`,
`plenora.geometry.srid`, `plenora.geometry.axis_order`) si tolgono, e `axis_order` si riscrive con l'ordine GIS
normalizzato del target. Gli altri metadati del campo restano.

#### Righe

1:1. Il runner chiama l'adapter di tabella dei kernel
(`riproiezione::reproject_batches`) con i parametri letti in validazione:
ogni cella non nulla si decodifica, si riproietta e si ricodifica; una
cella nulla resta nulla; le altre colonne non cambiano
([README, «Operazioni geo»](../README.md#operazioni-geo)). Ogni geometria usa un solo percorso fra i datum, il primo
dell'ordine di preferenza la cui area d'uso contiene tutti i suoi punti.

#### Ordine

Quello d'ingresso, righe e colonne. Il primo errore in ordine di riga è
quello che si riporta.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `Crs`: CRS della sorgente mancante o non risolto; `axis_order`
  dichiarato diverso dall'ordine GIS normalizzato della sorgente (anche
  `unknown`); `target_crs` non risolvibile; sorgente o target fuori dalla
  tabella (`CRS_NOT_BUILTIN`); `REPROJECTION_CONFIG_INVALID` (parametri
  senza effetto o fuori dominio, griglia ripetuta o non NTv2,
  `trasformazioni` che non formano un percorso);
  `REPROJECTION_PATH_UNAVAILABLE` (nessun percorso fra i datum);
  `REPROJECTION_ACCURACY_NOT_ACCEPTED` (il percorso migliore supera 1 cm
  e l'accuratezza accettata);
- `InvalidPlan`: config non leggibile (campi sconosciuti, `target_crs`
  assente, tipi sbagliati), `file` di una griglia vuoto, oltre 4096 byte
  o con NUL. Fino alla versione 1 dell'analisi del contratto era
  `InvalidConfiguration`, unica fra le operazioni geo.

In esecuzione, prima dell'adapter, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS sorgente, `Schema` per una geometria
di un tipo che il contratto d'ingresso non dichiara, quando li dichiara con
un elenco. Poi `reproject_batches`:

- `Schema`: colonna geometria assente, non `Binary` o non dichiarata WKB;
- `Crs`: `axis_order` non normalizzato; CRS sorgente diverso da quello
  con cui il piano è stato deciso; precisione del target non definita;
  `NTV2_GRID_UNREADABLE`, `NTV2_GRID_INVALID` (file di griglia); per
  geometria, `COORDINATE_OUT_OF_CRS_DOMAIN` (dominio o regione lon/lat
  della sorgente o del target), `REPROJECTION_NOT_CONVERGED` (un'inversa
  iterativa, di proiezione o di griglia, che non converge), `REPROJECTION_OUTSIDE_TRANSFORMATION_AREA`
  (nessun percorso ammesso copre tutta la geometria),
  `REPROJECTION_MIXED_TRANSFORMATION_AREAS` (punti o lati che
  preferiscono un percorso precedente), `REPROJECTION_EDGE_NOT_CONVERGED`
  (un lato non converge in 48 divisioni, per esempio attraverso
  l'antimeridiano del target);
- `ResourceLimit`: cella d'ingresso o d'uscita oltre 64 MiB, o
  densificazione oltre `MAX_CELL_COORDINATES` (4 194 304) coordinate per
  geometria;
- `InvalidPlan`: chiavi canoniche del campo non valide, WKB o geometria
  d'ingresso non validi, geometria riproiettata non valida OGC;
- `Unsupported`: dimensioni del campo diverse da XY, celle con Z/M;
- `Internal`: calcolo o validazione OGC interrotti (barriera dei panici).

Nessun messaggio riporta coordinate. Il primo errore è quello della prima
riga in ordine di riga, senza diagnostica per riga.

#### Limiti e deviazioni

- Il cambio di datum vale quanto l'accuratezza EPSG del percorso: oltre 1
  cm solo con `accuratezza_accettata_m` dichiarata
  ([README, «`geo.reproject`: il cambio di datum vale quanto l'accuratezza accettata»](../README.md#georeproject-il-cambio-di-datum-vale-quanto-laccuratezza-accettata));
  WGS 84 ed ETRS89 sono equivalenti per convenzione
  ([README, «WGS 84 = ETRS89 per convenzione»](../README.md#wgs-84--etrs89-per-convenzione)).
- Solo i CRS della tabella integrata
  ([README, «CRS integrati»](../README.md#crs-integrati)); aree d'uso come
  riquadri, accuratezze sommate, griglie non verificate contro il
  registro, densificazione a campioni e gli altri limiti in
  [README, «Limiti dichiarati della riproiezione»](../README.md#limiti-dichiarati-della-riproiezione).
- Nel runner un errore non ha diagnostica per riga e il costo in memoria è
  una previsione dalle misure
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

La matematica resta entro la precisione del target: proiezioni e
trasformazioni entro circa `1e-8` m da PROJ
([README, «Oracolo»](../README.md#oracolo)), lati densificati entro mezza
precisione del target, cioè 5 mm a terra (per un target geografico metà
di 1 cm in gradi all'equatore;
[README, «Densificazione dei lati»](../README.md#densificazione-dei-lati)).
Stesso CRS, o CRS che differiscono solo per l'ordine d'autorità degli
assi: coordinate invariate al bit. Il cambio di datum invece vale quanto
l'accuratezza del percorso, che oltre 1 cm si accetta solo dichiarandola
(sopra): è una garanzia indebolita per scelta esplicita
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria, tempo O(P² · k) nel caso peggiore, con k coordinate
prodotte (vertici più punti di densificazione, al più 4 194 304) e P
percorsi ammessi: i percorsi si provano in ordine finché uno copre la
geometria, e ogni punto si confronta con i percorsi precedenti. Memoria
O(k) per geometria, più le griglie NTv2 lette (al più 256 MiB per file);
`reproject_batches` tiene in memoria l'intera uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

Da WGS 84 a Pseudo Mercator (stesso datum): un parallelo resta una retta e
non riceve punti di densificazione; la cella nulla resta nulla.

Passo del piano:

```json
{"out": "risultato", "op": "geo.reproject", "in": ["luoghi"],
 "config": {"target_crs": "EPSG:3857"}}
```

Ingresso `luoghi` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(1 0) |
| 2 | LINESTRING(9 45,10 45) |
| 3 | null |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(111319.49079327357 0) |
| 2 | LINESTRING(1001875.4171394621 5621521.486192066,1113194.9079327357 5621521.486192066) |
| 3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.rotate`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `rotate` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Ruota ogni geometria di `degrees` gradi in verso antiorario attorno al
centro `(x_origin, y_origin)`. È
[`geo.affine_transform`](#geoaffine_transform) con la matrice
`[cos, -sin, x_off, sin, cos, y_off]`, dove seno e coseno si calcolano in
`f64` dall'angolo in radianti e `x_off`, `y_off` tengono fermo il centro.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `degrees` | numero | obbligatorio | finito | angolo in gradi, positivo in verso antiorario |
| `x_origin` | numero | `0` | finito | x del centro di rotazione |
| `y_origin` | numero | `0` | finito | y del centro di rotazione |

`x_origin` e `y_origin` sono facoltativi, anche uno solo: il kernel
(`extended::rotate_about`) riceve il centro sempre esplicito, e il runner
mette `0` al posto di ciascuno che manca. Senza entrambi si ruota attorno
a `(0, 0)` del CRS, non attorno alla geometria.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati; la
colonna geometria resta al suo posto con lo stesso CRS, le stesse
dimensioni (XY) e gli stessi tipi geometrici dichiarati. Le proprietà del
contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1: il runner chiama il kernel (`extended::rotate_about`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, `degrees` assente o non
  finito, un centro scritto e non finito;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi il kernel rende `ExtendedError`, che il runner porta in `Internal`
per `ValidazioneNonConclusa` e `CalcoloNonConcluso`, in `InvalidPlan`
per le altre: `InvalidParameter` (`degrees` non
finito, o `coefficients` per un termine della matrice che trabocca),
`InvalidInput` (coordinate non finite o geometria non valida OGC),
`InvalidOutput` (uscita non valida OGC), `ValidazioneNonConclusa` e
`CalcoloNonConcluso` (validazione o calcolo interrotti).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Nessuna rotazione è esatta, nemmeno di 90 o 180 gradi: il coseno di 90
gradi in `f64` vale circa `6.1e-17`, non zero (vedi l'esempio). Le
coordinate d'uscita non si confrontano con il dominio di validità del CRS
([README, «CRS integrati»](../README.md#crs-integrati)).

#### Precisione

Seno e coseno portano un errore relativo di circa un ulp, che sposta un
vertice di circa `1e-16` volte la sua distanza dal centro; la matrice si
applica come in [`geo.affine_transform`](#geoaffine_transform), con
errore di pochi ulp dei termini. In tutto, sotto 1 cm finché coordinate
e centro stanno sotto circa `1e13` unità del CRS, ben oltre ogni dominio
dei CRS integrati. Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia ruotata.

#### Memoria

Memoria: da misura v4.

#### Esempio

Un quarto di giro attorno all'origine: la x attesa è 0, quella calcolata
è il residuo del coseno di 90 gradi per 10.

Passo del piano:

```json
{"out": "risultato", "op": "geo.rotate", "in": ["assi"],
 "config": {"degrees": 90, "x_origin": 0, "y_origin": 0}}
```

Ingresso `assi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(10 0) |
| 2 | LINESTRING(0 0,10 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0.0000000000000006123233995736766 10) |
| 2 | LINESTRING(0 0,0.0000000000000006123233995736766 10) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.scale`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `scale` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Scala ogni geometria di `x_factor` lungo x e `y_factor` lungo y attorno
all'origine `(x_origin, y_origin)`, che resta ferma: `x' = x_factor x +
x_origin (1 - x_factor)`, e lo stesso per y. È
[`geo.affine_transform`](#geoaffine_transform) con la matrice
corrispondente. Un fattore negativo riflette la geometria.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `x_factor` | numero | obbligatorio | finito | fattore lungo x |
| `y_factor` | numero | obbligatorio | finito | fattore lungo y |
| `x_origin` | numero | `0` | finito | x dell'origine fissa |
| `y_origin` | numero | `0` | finito | y dell'origine fissa |

`x_origin` e `y_origin` sono facoltativi, anche uno solo: il kernel
(`extended::scale_about`) riceve l'origine sempre esplicita, e il runner
mette `0` al posto di ciascuna che manca. Senza entrambe si scala attorno
a `(0, 0)` del CRS, non attorno alla geometria.

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati; la
colonna geometria resta al suo posto con lo stesso CRS, le stesse
dimensioni (XY) e gli stessi tipi geometrici dichiarati. Le proprietà del
contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1: il runner chiama il kernel (`extended::scale_about`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, un fattore assente o non
  finito, un'origine scritta e non finita;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi il kernel rende `ExtendedError`, che il runner porta in `Internal`
per `ValidazioneNonConclusa` e `CalcoloNonConcluso`, in `InvalidPlan`
per le altre: `InvalidInput` (coordinate non finite o
geometria non valida OGC), `InvalidParameter` con nome `coefficients`
(un coefficiente della matrice non finito, anche per overflow di
`x_origin (1 - x_factor)`), `InvalidOutput` (uscita non valida OGC: un
fattore nullo schiaccia una superficie su una retta o un punto),
`ValidazioneNonConclusa` e `CalcoloNonConcluso` (validazione o calcolo
interrotti).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Le coordinate d'uscita non si confrontano con il dominio di validità del
CRS ([README, «CRS integrati»](../README.md#crs-integrati)).

#### Precisione

Come [`geo.affine_transform`](#geoaffine_transform): calcolo in `f64`
senza fusione, errore di pochi ulp di `|x_factor x| + |x_origin (1 -
x_factor)|`, sotto 1 cm finché quei termini stanno sotto circa `3e13`
unità del CRS; esatto con fattori, origine e coordinate interi. Nessun
rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia scalata.

#### Memoria

Memoria: da misura v4.

#### Esempio

Fattori 2 e 3 attorno al vertice `(1, 1)`, che resta fermo.

Passo del piano:

```json
{"out": "risultato", "op": "geo.scale", "in": ["lotti"],
 "config": {"x_factor": 2, "y_factor": 3, "x_origin": 1, "y_origin": 1}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((1 1,2 1,2 2,1 2,1 1)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((1 1,3 1,3 4,1 4,1 1)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.shared_paths`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | da tutto l'ingresso a molte righe |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | esente da `max_expansion_factor` (restano i limiti di righe) |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Trova i confini in comune fra i poligoni della tabella (i muri fra stanze
adiacenti, i confini fra particelle): per ogni coppia di righe i cui bordi
condividono tratti collineari scrive una riga con le posizioni delle due
righe, la lunghezza condivisa totale e i tratti, sul modello di
`ST_SharedPaths` di PostGIS. I contatti in un punto solo non contano.

La conversione di colonna è `extensions3::shared_paths_rows`, che il
runner chiama su tutta la colonna con i default della tabella sotto
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `tolerance` | numero | `0` | finito, `>= 0` | lunghezza sotto cui (compresa) un singolo tratto condiviso si scarta, nelle unità del CRS |
| `min_length` | numero | `0` | finito, `>= 0` | lunghezza condivisa totale minima perché la coppia dia una riga |

`tolerance` è una soglia di lunghezza dei tratti, non una distanza: due
bordi che corrono paralleli a meno di 1 cm senza essere collineari non
condividono nulla.

#### Schema

Schema nuovo, in quest'ordine: `index_a` e `index_b` (`uint64`),
`shared_length` (`float64`), `geometry` (`binary`, CRS della colonna
d'ingresso, dimensioni `xy`, senza dichiarazione dei tipi), tutte non
nullable. Le colonne dell'ingresso spariscono, i metadati di schema
restano; nessuna proprietà del contratto sopravvive.

#### Righe

Una riga per coppia `(a, b)`, con `a < b`, i cui rettangoli d'ingombro si
toccano e i cui bordi (anelli esterni e buchi) condividono tratti
collineari di lunghezza totale non nulla e almeno `min_length`:

- `index_a`, `index_b`: posizioni delle due righe nell'ingresso, da 0; le
  righe nulle e le geometrie vuote non partecipano ma contano nella
  posizione;
- `shared_length`: somma delle lunghezze dei tratti tenuti;
- `geometry`: un `LineString` di due punti se il tratto è uno, altrimenti
  una `MultiLineString` di segmenti, uno per coppia di lati sovrapposti
  (non fusi in catene).

Anche le coppie di poligoni che si sovrappongono danno una riga, se i loro
bordi hanno tratti collineari.

#### Ordine

Per `index_a`, poi per `index_b`, crescenti. Dentro una riga, i segmenti
seguono anelli e lati del primo poligono, poi quelli del secondo.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `tolerance` o `min_length`
  negativi o non finiti;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi la conversione di colonna (messaggi del calcolo con prefisso
`geo.shared_paths:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; una geometria che non è
  `Polygon` o `MultiPolygon` (il messaggio riporta la posizione della riga,
  non i dati); tratti prodotti non validi;
- `Unsupported`: WKB con dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella;
- `Internal`: panico di `geo` o `rstar`, validazione che non conclude.

#### Limiti e deviazioni

Solo tratti esattamente collineari: nessuna tolleranza di distanza, nessuna
fusione dei segmenti consecutivi in una linea sola. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Esatta sulla geometria: la collinearità si decide con il predicato
`orient2d` esatto di `geo`, e gli estremi di un tratto condiviso sono
vertici d'ingresso. Solo le lunghezze (`shared_length`, e il confronto con
`tolerance` e `min_length`) si calcolano in `f64`. Nessuna griglia, nessun
controllo di precisione
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

R-tree sui rettangoli d'ingombro, O(n log n); per ogni coppia candidata un
confronto di tutti i lati dell'uno con tutti i lati dell'altro, O(a · b),
con un filtro sui rettangoli dei segmenti. Memoria O(n) per tutte le
geometrie, decodificate prima del calcolo (classe bloccante).

#### Memoria

Memoria: da misura v4.

#### Esempio

Due stanze con un muro in comune e una terza che le tocca in un punto.

Passo del piano:

```json
{"out": "risultato", "op": "geo.shared_paths", "in": ["stanze"],
 "config": {}}
```

Ingresso `stanze` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 0,4 4,0 4,0 0)) |
| 2 | POLYGON((4 0,8 0,8 4,4 4,4 0)) |
| 3 | POLYGON((8 4,10 4,10 6,8 6,8 4)) |

Uscita `risultato`:

| `index_a: uint64` | `index_b: uint64` | `shared_length: float64` | `geometry: geometry` |
| --- | --- | --- | --- |
| 0 | 1 | 4.0 | LINESTRING(4 4,4 0) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.simplify`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_simplify` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 3, config 2, analisi 2, kernel 2 |

#### Che cosa fa

Semplifica ogni geometria togliendo vertici, nella stessa colonna, con uno
di due algoritmi:

- `douglas_peucker` (default): Ramer-Douglas-Peucker su ogni linea e ogni
  anello; toglie i vertici che distano meno di `tolerance` (una
  **distanza**, unità del CRS) dalla linea semplificata. Un anello resta di
  almeno quattro coordinate; la topologia non è garantita e un risultato
  non valido è un errore;
- `preserve_topology`: Visvalingam-Whyatt con conservazione della topologia
  di `geo` (`simplify_vw_preserve`): toglie i vertici il cui triangolo con i
  due vicini ha **area** non maggiore di `min_area` (unità del CRS al
  quadrato), senza creare intersezioni.

La soglia ha un nome per algoritmo perché non è la stessa grandezza:
`tolerance` con `douglas_peucker`, `min_area` con `preserve_topology`, e
l'altra si rifiuta. Punti e multi-punti passano invariati; una collezione
si semplifica membro per membro. Con soglia 0 la geometria non cambia.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `tolerance` | numero | obbligatorio con `douglas_peucker`, vietato con `preserve_topology` | finito, maggiore o uguale a 0 | distanza massima di un vertice tolto (unità del CRS) |
| `min_area` | numero | obbligatorio con `preserve_topology`, vietato con `douglas_peucker` | finito, maggiore o uguale a 0 | area del triangolo sotto la quale, uguaglianza compresa, un vertice si toglie (unità del CRS al quadrato) |
| `policy` | stringa | `douglas_peucker` | `douglas_peucker`, `preserve_topology` | algoritmo |

#### Schema

Identico all'ingresso: la colonna geometria resta al suo posto con lo
stesso nome, CRS, dimensioni e dichiarazione dei tipi; le altre colonne, i
metadati di schema e le proprietà del contratto (`sorted_by`, `row_count`)
passano invariati.

#### Righe

1:1: il runner chiama il kernel (`operations::simplify_with_policy`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`. I vertici tenuti
restano nel loro ordine.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: la soglia dell'algoritmo assente (`tolerance` con
  `douglas_peucker`, `min_area` con `preserve_topology`), non finita o
  negativa; la soglia dell'altro algoritmo presente (`tolerance` con
  `preserve_topology`: «la soglia di Visvalingam-Whyatt è un'area,
  `min_area`»; `min_area` con `douglas_peucker`); `policy` fuori elenco;
  campi sconosciuti;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi dal kernel, per geometria (`OperationError`, che il runner porta in `PlenoraError`:
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, `Unsupported` per `PrecisionInsufficient`,
`InvalidPlan` per le altre):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `InvalidOutput`: la geometria semplificata non supera la validazione OGC
  (per esempio anelli che si incrociano dopo Douglas-Peucker);
- `Internal`: con `douglas_peucker`, una distanza intermedia non è un
  numero finito (coordinate vicine ai limiti di `f64`); nessun risultato
  parziale;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` vanno in panico dentro la barriera (il messaggio porta
  solo la forma del payload);
- `InvalidParameter`: soglia non finita o negativa (l'analisi la
  rifiuta prima); sul percorso scalato (sotto), un'area positiva che nella
  scala delle coordinate diventa zero.

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
`preserve_topology` non è `TopologyPreservingSimplifier` di GEOS
(`ST_SimplifyPreserveTopology`), che usa una distanza: qui la soglia è
un'area, e per questo si chiama `min_area`. Fino alla versione 2 del
catalogo si scriveva `tolerance` anche qui, e un piano scritto pensando a
GEOS semplificava in modo molto diverso senza errore: ora quel piano si
rifiuta in validazione. Le coordinate di modulo oltre `1e150`, o non
nulle e sotto `1e-150`, si semplificano in uno spazio scalato
uniformemente e si riportano indietro, con la soglia scalata come la sua
grandezza (una distanza per il fattore, un'area per il suo quadrato; fino
alla versione 2 l'area si scalava come una distanza).

#### Precisione

Esatta: i vertici d'uscita sono vertici d'ingresso, senza calcolo, fuori
dal percorso scalato (coordinate oltre `1e150` o sotto `1e-150` in modulo,
fuori da ogni dominio di un CRS reale), dove passano per una divisione e
una moltiplicazione. Lo scarto dalla forma originale è quello chiesto con
la soglia, non un errore di precisione
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: `douglas_peucker` O(n log n) nel caso tipico e
O(n²) nel peggiore, con una seconda traversata dello stesso costo che
verifica le distanze; `preserve_topology` O(n log n) nel caso tipico (coda di priorità e
indice spaziale di `geo`). Più la validazione OGC dell'ingresso e
dell'uscita (sub-quadratica nel caso tipico, O(n²) nel peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.simplify", "in": ["tratte"],
 "config": {"tolerance": 0.5}}
```

Ingresso `tratte` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,5 0.2,10 0) |
| 2 | POLYGON((0 0,10 0,10 10,5 10.1,0 10,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,10 0) |
| 2 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.sjoin`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `sjoin` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:N |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / (sinistra + destra) |
| fusione geo | non fondibile |
| maturità | protocollo pubblico |
| versioni | semantica 3, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Join spaziale interno: abbina ogni riga della sinistra alle righe della
destra la cui geometria soddisfa `predicate` con la sua, e dà una riga per
coppia, con la posizione della riga destra in `__right_index` (kernel
`spatial_join::spatial_join_nullable_validated`, [README, «Operazioni
geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `predicate` | stringa | obbligatorio | `intersects`, `contains`, `within`, `crosses`, `overlaps`, `touches` | la relazione fra la geometria sinistra `L` e la destra `R` |

I predicati sono quelli di `geo`, esatti; tutti tranne `intersects`
passano dalla matrice DE-9IM:

- `intersects`: almeno un punto in comune, bordo compreso;
- `contains`: `L` contiene `R` (nessun punto di `R` fuori da `L`, almeno
  un punto dell'interno di `R` nell'interno di `L`: `R` sul solo bordo di
  `L` non conta);
- `within`: `R` contiene `L`;
- `crosses`, `overlaps`, `touches`: come nella DE-9IM (attraversamento,
  sovrapposizione parziale, contatto solo sul bordo).

#### Schema

Le colonne della sinistra, invariate, più `__right_index` in coda, `uint64`
non nullable. Le colonne della destra non passano: si ricollegano con
`__right_index`; non c'è `__left_index`. La colonna geometria resta quella
della sinistra, non nullable: una geometria null non ha coppie. I metadati di schema sono la fusione dei due lati; le
proprietà del contratto (`sorted_by`, `row_count`) si perdono.

#### Righe

Espansione 1:N: una riga per coppia che soddisfa il predicato, con le
colonne della riga sinistra della coppia, quindi una riga sinistra si
ripete per ogni destra abbinata e non compare se non ne ha nessuna. Le
geometrie nulle o vuote, da un lato o dall'altro, non si abbinano mai;
`__right_index` è la posizione della riga destra contando anche le nulle.
Le coppie sono al più il limite di righe dell'arco d'uscita
(`max_output_rows` se il passo è un output del piano, `max_rows_per_edge`
altrimenti).

#### Ordine

Per riga sinistra, poi per `__right_index` crescente (ordine
lessicografico delle coppie).

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti, `predicate` assente o fuori elenco; un metadato di schema
  presente sui due lati con valori diversi;
- `Schema`: `__right_index` esiste già nella sinistra; un lato senza
  esattamente una colonna geometria, o con una colonna non riconoscibile
  come geometria WKB (né estensione `geoarrow.wkb` né chiavi canoniche
  `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`spatial_join::spatial_join_nullable_validated`, errore
`SpatialJoinError`, sulle geometrie già validate), nella categoria del
passo geo indicata fra parentesi:

- `PairLimitExceeded` (`ResourceLimit`): le coppie confermate superano il
  limite di righe dell'arco (si controlla coppia per coppia, prima di
  materializzarle; ogni altro errore, il primo in ordine di riga, ha la
  precedenza);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`, `Internal` (`Internal`):
  l'indice o il predicato di `geo` non ha concluso, o un'invariante
  interna violata;
- `IndexOverflow` (`InvalidPlan`): numero di righe oltre `u64`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Solo join interno: il kernel non emette le righe sinistre senza
abbinamento, e non c'è una variante che le tenga con valori nulli. Gli
attributi della destra non entrano nell'uscita, a differenza di `sjoin` di
GeoPandas: si ricollegano con `__right_index`. Le coppie dipendono dai
dati: il modello di costo non le prevede, e le limita solo il limite di
righe dell'arco ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Modelli di costo
geo»).

#### Precisione

Nessun calcolo di geometrie e nessuna griglia: i predicati di `geo` si
valutano sulle coordinate `f64` d'ingresso, senza tolleranza, e la regola
di 1 cm non sposta nulla. Due geometrie a meno di 1 cm si toccano o no
secondo le loro coordinate esatte ([README, «Precisione delle operazioni
geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Feature d'ingresso più vicine della precisione»).

#### Complessità

Un R-tree dei rettangoli d'ingombro della destra, O(m log m) per `m`
righe; per ognuna delle `n` righe sinistre, in parallelo, una ricerca
nell'albero e il predicato esatto sui soli candidati. Di norma
O((n + m) log m) più il costo dei predicati e delle coppie `p`; nel caso
peggiore (rettangoli tutti sovrapposti) O(n · m) predicati. Più la
validazione OGC di ogni geometria. Memoria O(m + p).

#### Memoria

Memoria: da misura v4.

#### Esempio

Il primo punto sta sull'angolo comune dei due quadrati e il terzo sul lato
comune: ognuno interseca entrambi e dà due righe; il secondo non ne
interseca nessuno e non esce.

Passo del piano:

```json
{"out": "risultato", "op": "geo.sjoin", "in": ["pozzi", "aree"],
 "config": {"predicate": "intersects"}}
```

Ingresso `pozzi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(2 2) |
| 2 | POINT(5 5) |
| 3 | POINT(2 1) |

Ingresso `aree` (geometrie `geometry` in EPSG:3857):

| `nome: utf8` | `geometry: geometry` |
| --- | --- |
| A | POLYGON((0 0,2 0,2 2,0 2,0 0)) |
| B | POLYGON((2 0,4 0,4 2,2 2,2 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `__right_index: uint64` |
| --- | --- | --- |
| 1 | POINT(2 2) | 0 |
| 1 | POINT(2 2) | 1 |
| 3 | POINT(2 1) | 0 |
| 3 | POINT(2 1) | 1 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.snap`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Aggancia ogni vertice della geometria di ogni riga al vertice più vicino di
una geometria di riferimento data nella config, se dista al più
`tolerance`; i vertici più lontani restano dove sono. Serve ad allineare
geometrie che dovrebbero condividere vertici e ne differiscono di poco. Il
tipo geometrico non cambia; il risultato deve restare valido.

La conversione di colonna è `extensions2::snap_column`, che il runner
chiama sulla colonna intera.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `reference_wkb` | stringa | obbligatorio | WKB esadecimale di una geometria valida, nel dominio del CRS della colonna | riferimento, nello stesso CRS della colonna: contano solo i suoi vertici |
| `tolerance` | numero | obbligatorio | finito, `>= 0` | distanza massima di aggancio, nelle unità del CRS; `0` aggancia solo i vertici che coincidono |

#### Schema

Identico all'ingresso: colonne, tipi, nullabilità, metadati e proprietà del
contratto (`sorted_by`, `row_count`).

#### Righe

1:1: il runner chiama la conversione di colonna dei kernel
(`extensions2::snap_column`) sulla colonna intera, con il riferimento letto
una volta in validazione. Una cella nulla resta nulla. Con un riferimento
senza vertici ogni geometria esce invariata ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso (le celle si elaborano in parallelo, con l'ordine
ricostruito per indice).

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `reference_wkb` mancante,
  non esadecimale, malformato o OGC-invalido; `tolerance` mancante,
  negativa o non finita;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
  `reference_wkb` con dimensioni Z/M o SRID;
- `Crs`: colonna senza CRS risolto o CRS non proiettato; riferimento fuori
  dal dominio del CRS.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi la conversione di colonna, che rende già `PlenoraError` (vince la
prima cella che fallisce in ordine di riga, senza diagnostica per riga):

- `InvalidPlan`: WKB malformato o OGC-invalido; geometria agganciata non
  più valida (un anello che collassa, un lato che si sovrappone): messaggio
  con prefisso `geo.snap:`;
- `Unsupported`: WKB con dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella;
- `Internal`: panico nella costruzione dell'R-tree o in `geo`, validazione
  che non conclude.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
A differenza di `ST_Snap` di PostGIS e dello snap di GEOS, si agganciano
solo vertici a vertici: i vertici non vanno sui lati del riferimento, e i
vertici del riferimento non si inseriscono nei lati della geometria. Se
due vertici del riferimento sono alla stessa distanza, la scelta è
deterministica ma non specificata. Un'uscita invalida è un errore, non una
geometria riparata.

#### Precisione

Esatta sulle coordinate: un vertice agganciato prende i bit del vertice di
riferimento, gli altri restano quelli d'ingresso. Solo la decisione
«distanza `<= tolerance`» si calcola in `f64` (`hypot`): su un vertice a
distanza pari a `tolerance` entro l'arrotondamento può cadere da una parte
o dall'altra
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

R-tree dei vertici del riferimento costruito una volta, O(r log r); poi
O(log r) per vertice d'ingresso, più la validazione OGC dell'uscita di ogni
riga. Memoria O(r) per l'albero, più le celle d'uscita.

#### Memoria

Memoria: da misura v4.

#### Esempio

`reference_wkb` è `POINT(10 10)`, `tolerance` 1 cm.

Passo del piano:

```json
{"out": "risultato", "op": "geo.snap", "in": ["confini"],
 "config": {"reference_wkb": "010100000000000000000024400000000000002440", "tolerance": 0.01}}
```

Ingresso `confini` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,9.995 10) |
| 2 | LINESTRING(0 0,9 9) |
| 3 | null |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,10 10) |
| 2 | LINESTRING(0 0,9 9) |
| 3 | null |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.snap_to_grid`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `snap_to_grid` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Porta ogni vertice sul nodo più vicino di una griglia regolare di passo
`grid_size` con origine in `(0, 0)`: per asse `round(x / grid_size) *
grid_size`, a metà strada lontano da zero, con `-0.0` reso `0.0`. Non
ripara e non semplifica: i vertici consecutivi che cadono sullo stesso nodo
restano duplicati, e un collasso che rende la geometria non valida (una
linea ridotta a un punto, un anello degenere o auto-intersecato) è un
errore, non una correzione.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `grid_size` | numero | obbligatorio | finito, maggiore di zero | passo della griglia, nelle unità del CRS |

#### Schema

Invariato: la colonna geometria si riscrive al suo posto, con lo stesso
nome, gli stessi metadati, lo stesso CRS, dimensioni `xy` e gli stessi tipi
dichiarati. Le altre colonne, i metadati di schema e le proprietà del
contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1: la geometria di ogni riga diventa quella agganciata alla griglia. Il
runner chiama il kernel (`extended_algorithms::snap_to_grid`) su ogni
cella non nulla, in parallelo; una cella nulla resta nulla
([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Quello d'ingresso; dentro una geometria i vertici restano nel loro ordine,
anche quelli diventati uguali.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `grid_size` o con un tipo sbagliato; `grid_size` non finito o non
  maggiore di zero;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi il kernel rende `ExtendedAlgorithmError`, che il runner porta in
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre. Rifiuta la geometria
con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude) e con
`InvalidOutput` quando un vertice agganciato non è finito (overflow) o
quando la geometria agganciata non è valida per l'OGC: per esempio un
quadrato di lato 0,4 con `grid_size` 1 (`punti distinti insufficienti`).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

- A differenza di `ST_SnapToGrid` di PostGIS, che toglie i vertici
  consecutivi uguali e rende NULL una geometria collassata, qui i
  duplicati restano e il collasso che viola la validità OGC è un errore;
  un collasso che la lascia valida (una parte sottile che cambia forma)
  passa senza errore.
- Griglia con origine in `(0, 0)` e passo uguale sui due assi.

#### Precisione

Lo spostamento è voluto e dichiarato dal parametro: ogni vertice si sposta
fino a `grid_size * sqrt(2) / 2`, entro 1 cm solo se `grid_size` è sotto
circa 1,4 cm; oltre, la regola di 1 cm non si applica a questa
operazione, perché lo spostamento è la sua definizione ([README,
«Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Il nodo è il prodotto `round(x / grid_size) * grid_size` arrotondato in
`f64`: con un passo non rappresentabile esattamente (0,1) è il `f64` più
vicino al multiplo, non il multiplo esatto. Nessun rifiuto
`PrecisionInsufficient`.

#### Complessità

O(n) per geometria, con `n` le coordinate, più la validazione OGC
dell'ingresso e dell'uscita (O(n²) nel caso peggiore, [README,
«Validazione OGC: la ricerca delle auto-intersezioni non è quella di `geo`,
il verdetto sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(n).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.snap_to_grid", "in": ["rilievi"],
 "config": {"grid_size": 1}}
```

Ingresso `rilievi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(1.4 2.6) |
| 2 | LINESTRING(0.2 0.1,9.7 0.4) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(1 3) |
| 2 | LINESTRING(0 0,10 0) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.split`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `split` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione non interrompibile |
| forma del risultato | 1:N |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 3 |

#### Che cosa fa

Divide la geometria di ogni riga con una lama lineare e scrive una riga per
parte, con gli attributi della riga d'origine e il suo indice in
`__parent_index`. Una sorgente `Polygon` o `MultiPolygon` si taglia in
Rust puro: bordo e lama si nodano insieme, se ne estraggono le facce e si
tengono quelle interne alla sorgente, poi area e copertura del bordo si
verificano contro la sorgente. Una sorgente `LineString` si spezza nei
punti in cui la lama la tocca, entro `tolerance`.

La lama arriva dalla config (`other_wkb`), nello stesso CRS della colonna.
L'esecuzione Arrow (`rust_backend::arrow::split_batches`) riceve una lama
per riga, allineata alle sorgenti: il runner la chiama su tutta la tabella
con la stessa lama su ogni riga ([README, «Operazioni
geo»](../README.md#operazioni-geo)).

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB esadecimale di una geometria valida (OGC), coordinate nel dominio del CRS della colonna | la lama: linee (`LineString`, `MultiLineString`, collezioni di linee) per le sorgenti poligonali; per le sorgenti lineari anche punti e contorni di poligoni |
| `tolerance` | numero | `0` | finito, `>= 0` | distanza entro cui un punto della lama taglia una sorgente `LineString` |

`tolerance` vale solo per le sorgenti `LineString`: sulle poligonali si
accetta e non ha effetto.

#### Schema

Le colonne dell'ingresso, nelle stesse posizioni e con gli stessi tipi, più
`__parent_index` (`uint64`, non nullable) in coda. Il campo geometria
conserva i metadati d'ingresso tranne la dichiarazione dei tipi, ed è non
nullable (una sorgente null non produce parti). Il contratto dichiara i tipi `exact`: `Polygon` per sorgenti
`Polygon`/`MultiPolygon`, `LineString` per sorgenti `LineString`, entrambi
se l'ingresso non ha una dichiarazione `exact` che li restringa. Resta
`sorted_by`; `row_count` cade.

#### Righe

Espansione 1:N: ogni sorgente dà le sue parti (una sola se la lama non la
taglia). Su una sorgente lineare tagli consecutivi distanti al più
`tolerance` lungo la linea si fondono in uno; un tratto collineare della lama taglia ai suoi
due estremi. La lama della config non è mai nulla: una riga con la
sorgente nulla non produce righe, e la lama si decodifica e si valida lo
stesso. `__parent_index` conta da 0. Le facce del taglio che cadono fuori
dalla sorgente (una lama chiusa che sporge) si scartano.

#### Ordine

Quello delle sorgenti; dentro una sorgente poligonale, l'ordine delle facce
del polygonize interno, non quello di GEOS; dentro una sorgente lineare,
dall'inizio alla fine della linea.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `other_wkb` mancante, non
  esadecimale, strutturalmente malformato o non valido per l'OGC;
  `tolerance` negativa o non finita;
- `Internal`: la validazione OGC di `other_wkb` non conclude;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; una colonna `__parent_index` esiste
  già;
- `Unsupported`: dimensioni della geometria diverse da `xy`; `other_wkb`
  con dimensioni Z/M o SRID;
- `Crs`: colonna senza CRS risolto o CRS non proiettato; coordinate della
  lama fuori dal dominio del CRS.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi l'esecuzione Arrow (vince la prima riga che fallisce):

- `InvalidPlan`: sorgente di tipo
  diverso da `LineString`, `Polygon`, `MultiPolygon`; lama di tipo non
  ammesso; WKB malformato o OGC-invalido; area non conservata
  (`AreaMismatch`) o bordo non ricoperto (`CoverageMismatch`);
- `ResourceLimit`: cella oltre il limite di byte; prenotazione di memoria
  fallita; limiti superati (coordinate per cella, per ciascun ingresso e
  per la loro somma; 100.000.000 coppie di noding o test d'intersezione;
  righe d'uscita oltre il limite di righe dell'arco, `max_output_rows` per
  un output del piano e `max_rows_per_edge` altrimenti, cumulate su tutte
  le righe);
- `Unsupported`: noding non convergente; segno d'area non decidibile;
  `PrecisionInsufficient` (sotto, «Precisione»);
- `Internal`: panico di `geo` dentro il kernel, invariante violata.

#### Limiti e deviazioni

- Lo split poligonale gira sul kernel del laboratorio, qualificato contro
  GEOS per equivalenza semantica
  ([README, «`geo.make_valid`, `geo.polygonize`, `geo.split`: equivalenza a
  GEOS verificata, non dimostrata»](../README.md#geomake_valid-geopolygonize-geosplit-equivalenza-a-geos-verificata-non-dimostrata));
  lo split lineare è il codice precedente al porting.
- La scelta delle facce usa un campione interno e un test pari-dispari con
  il lato del punto deciso in modo esatto, non `point_on_surface` e `covers`
  di GEOS; area e copertura del bordo sono verificate dopo: un'incoerenza è
  un errore, mai parti in più o in meno.
- Il budget di parti e coordinate conta tutto l'output del polygonize
  interno, anche facce fuori dalla sorgente e residui scartati, non solo le
  parti tenute; il limite di coordinate vale per ciascun ingresso e per la
  somma.
- Elenco completo: [README, «Differenze da GEOS»](../README.md#differenze-da-geos).
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Sorgenti poligonali
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)):

- guardia di spaziatura delle coordinate e noding del polygonize interno:
  ogni incrocio arrotondato entro `p / 5` dai segmenti che divide, al più
  cinque giri; oltre, `PrecisionInsufficient`;
- verifiche locali dopo il calcolo: l'area delle parti può differire da
  quella della sorgente al più di `p` per la lunghezza dei lati di bordo con
  un estremo calcolato dal noding (più l'arrotondamento delle aree), e la
  lunghezza del bordo sorgente non coperta dalle parti, sommata per anello,
  al più `p`. Oltre, `AreaMismatch` o `CoverageMismatch`: una parte mancante
  più larga di 1 cm è sempre un errore.

Sorgenti lineari: prima del taglio la stessa guardia di spaziatura, che
tiene il margine numerico di `split_line` sotto `p / 2`: un punto a più di
1 cm dalla linea, con `tolerance` nulla, non taglia.

Nessuna griglia di `i_overlay`. `p` è la precisione del CRS della colonna
(`Precision::from_crs`, 1 cm a terra).

#### Complessità

Per riga, sorgente poligonale con `n` segmenti fra bordo e lama: noding
O(n²) coppie nel caso peggiore (tetto 100.000.000), facce O(e log e), poi la
verifica della copertura, O(b · s) con `b` i lati di bordo delle parti e
`s` i lati della sorgente. Sorgente lineare: O(s · l) test fra segmenti
della sorgente e primitive della lama, entro 100.000.000. Memoria O(n) per
la riga più le parti prodotte; l'uscita si costruisce in un batch solo.

#### Memoria

Memoria: da misura v4.

#### Esempio

Un quadrato tagliato da una retta verticale (`other_wkb` è
`LINESTRING(5 -1,5 11)`).

Passo del piano:

```json
{"out": "risultato", "op": "geo.split", "in": ["lotti"],
 "config": {"other_wkb": "0102000000020000000000000000001440000000000000f0bf00000000000014400000000000002640"}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 7 | POLYGON((0 0,10 0,10 10,0 10,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `__parent_index: uint64` |
| --- | --- | --- |
| 7 | POLYGON((0 10,0 0,5 0,5 10,0 10)) | 0 |
| 7 | POLYGON((5 10,5 0,10 0,10 10,5 10)) | 0 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.subdivide`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | nessuno |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:N |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | non disponibile |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Spezza ogni geometria con più di `max_vertices` vertici in parti che ne
hanno al più `max_vertices`, una riga per parte con gli attributi della
riga d'origine e il suo indice in `__parent_index`, come `ST_Subdivide` di
PostGIS. I poligoni si tagliano a metà del rettangolo d'ingombro, sul lato
lungo, finché ogni pezzo sta sotto la soglia; linee e `MultiPoint` si
dividono a blocchi. Una geometria sotto la soglia passa invariata, in una
riga sola.

Il calcolo per cella è `extensions2::subdivide_wkb`; il runner lo chiama
su ogni cella non nulla, con la precisione del CRS della colonna, e scrive
l'indice della riga d'origine.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_vertices` | intero | obbligatorio | `>= 4` | vertici massimi per parte (contati come `coords_count`: il vertice di chiusura e i buchi compresi) |
| `output_column` | stringa | nome della colonna geometria | nome non vuoto; libero, oppure uguale al nome della colonna geometria (che allora resta com'è) | rinomina la colonna geometria dell'uscita |

#### Schema

Le colonne dell'ingresso, nelle stesse posizioni; la colonna geometria
prende il nome `output_column` se dato (stesso tipo, metadati e
nullabilità). In coda `__parent_index`, `uint64` non nullable. Tipi
dichiarati, se l'ingresso li dichiara: `Point`, `LineString`, `Polygon` e
`MultiPoint` restano tali; `MultiLineString` dà `LineString` o
`MultiLineString`, `MultiPolygon` dà `Polygon` o `MultiPolygon`,
`GeometryCollection` uno qualunque dei sette tipi. Resta `sorted_by`,
`row_count` cade.

#### Righe

Espansione 1:N, per tipo:

- `LineString`: blocchi di `max_vertices` vertici, in cui l'ultimo vertice
  di un blocco è il primo del successivo;
- `MultiLineString`: ogni linea sotto la soglia intera, le altre a blocchi;
  le parti sono `LineString`;
- `MultiPoint`: blocchi di `max_vertices` punti, ciascuno un `MultiPoint`;
- `Polygon`, `MultiPolygon`: taglio ricorsivo di ogni poligono,
  intersecandolo con le due metà del suo rettangolo d'ingombro (la metà sul
  lato più lungo, sull'asse x a parità), fino a 32 livelli; i pezzi di area
  nulla lungo la linea di taglio si scartano. La somma delle aree resta
  quella del poligono, entro la precisione;
- `GeometryCollection`: ogni membro per sé.

Ogni parte si valida (OGC). Una geometria nulla dà una riga, con la
geometria nulla e il suo `__parent_index`. `__parent_index` conta da 0.
Il runner conta le righe prodotte su tutta la tabella: oltre il limite di righe dell'arco d'uscita (`max_output_rows` per un output del
piano, `max_rows_per_edge` altrimenti), `ResourceLimit`.

#### Ordine

Le parti di una riga seguono la riga; dentro una riga, i blocchi di una
linea dall'inizio alla fine, e i pezzi di un poligono in profondità, prima
la metà sinistra (o inferiore), e dentro una metà nell'ordine delle parti
restituite da `i_overlay`.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `max_vertices` mancante,
  non intero non negativo o minore di 4; `output_column` vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; `output_column` già presente
  come altra colonna (il nome della colonna geometria stessa è ammesso),
  o `__parent_index` già presente;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi il calcolo per cella (messaggi con prefisso `geo.subdivide:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; taglio che non converge in
  32 livelli; parte prodotta non valida;
- `Unsupported`: `PrecisionInsufficient` (sotto, «Precisione»); WKB con
  dimensioni Z/M o SRID;
- `ResourceLimit`: cella d'ingresso o parte oltre il limite di byte per
  cella; righe prodotte oltre il limite di righe dell'arco;
- `Internal`: panico di `geo` o `i_overlay`, validazione che non conclude.

#### Limiti e deviazioni

Le parti di un poligono non sono uniche: due versioni di `i_overlay`
possono scegliere tagli diversi, con la stessa area totale
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Hazard»). Il taglio si ferma a 32 livelli con un errore, non con parti
sopra la soglia. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

Linee e `MultiPoint` sono esatti: le parti copiano i vertici. I tagli dei
poligoni passano dalla griglia di `i_overlay` e sono in catena, un livello
sul risultato del precedente: prima di ogni taglio il controllo a priori
dà a ognuno dei 32 livelli `1/32` di `p / 2` sull'ingombro del pezzo, e
se lo spostamento della griglia lo supera, o le coordinate sono troppo rade
per `p`, il taglio non si esegue (`PrecisionInsufficient`). `p` è 1 cm a
terra nelle unità del CRS della colonna (`Precision::from_crs`). Le foglie non
sono confrontate con il poligono di partenza: sotto la precisione parti più
sottili di 1 cm possono fondersi o sparire
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Linee e punti: O(v) per riga. Poligoni: a ogni livello due intersezioni
per pezzo, O(v log v) sui vertici del pezzo, per al più 32 livelli;
memoria O(v) più le parti prodotte.

#### Memoria

Memoria: da misura v4.

#### Esempio

Una linea di 7 vertici con `max_vertices` 4 e una linea sotto la soglia.

Passo del piano:

```json
{"out": "risultato", "op": "geo.subdivide", "in": ["reti"],
 "config": {"max_vertices": 4}}
```

Ingresso `reti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | LINESTRING(0 0,1 0,2 0,3 0,4 0,5 0,6 0) |
| 2 | LINESTRING(0 5,1 5) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `__parent_index: uint64` |
| --- | --- | --- |
| 1 | LINESTRING(0 0,1 0,2 0,3 0) | 0 |
| 1 | LINESTRING(3 0,4 0,5 0,6 0) | 0 |
| 2 | LINESTRING(0 5,1 5) | 1 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.symmetric_difference`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_symmetric_difference` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 1, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Sostituisce la geometria della sinistra con le parti che stanno in uno
solo dei due lati, `(sinistra \ destra) ∪ (destra \ sinistra)`, calcolate
dal kernel `topology::boolean_operation_validated` ([README, «Operazioni
geo»](../README.md#operazioni-geo)). Lavora solo su `Polygon` e
`MultiPolygon` e rende sempre un `MultiPolygon`.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi; le colonne della destra non passano. La colonna geometria resta al
suo posto, con lo stesso nome e lo stesso CRS della sinistra (uguale a
quello della destra), in XY, ed è nullable anche quando quella della
sinistra non lo è (un risultato vuoto è nullo); i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

#### Righe

Allineate: la riga `i` della sinistra con la riga `i` della destra, e le
due tabelle devono avere le stesse righe (altrimenti `InvalidPlan`, in
esecuzione: le righe non si conoscono a secco). Una riga d'uscita per riga
della sinistra, con la geometria nulla dove una delle due è nulla o il
risultato è vuoto. Dove le due geometrie coincidono il kernel rende un
`MultiPolygon` vuoto e la riga resta con la geometria nulla.

#### Ordine

Quello della sinistra.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config non vuota;
  un metadato di schema presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte);
- `InvalidPlan`: le due tabelle hanno un numero di righe diverso.

Dal kernel (`topology::boolean_operation_validated`, errore
`TopologyError`, sulle geometrie già validate: resta la validazione OGC
del risultato), nella categoria del passo geo indicata fra parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): il risultato non supera la
  validazione OGC;
- `PrecisionInsufficient` (`Unsupported`): la griglia dell'overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): la
  validazione OGC o l'overlay di `geo` non ha concluso.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Solo poligoni: una geometria lineare o puntuale si rifiuta
(`UnsupportedGeometry`). Nessun controllo a posteriori del risultato contro
gli ingressi ([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra: un
solo overlay di `i_overlay` su interi `i64`, con la griglia controllata
prima del calcolo sull'ingombro dei due operandi. Se lo spostamento a
priori supera mezzo centimetro, o le coordinate sono troppo rade per il
centimetro, `PrecisionInsufficient` e nessun calcolo. Parti più sottili di
1 cm possono sparire o fondersi senza errore; vedi [README, «Precisione
delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

#### Complessità

Per coppia l'overlay a scansione di `i_overlay`, di norma O((v + k) log v)
con `v` i vertici dei due operandi e `k` gli incroci fra i lati, più la
validazione OGC di ingressi e risultato (di norma O(v log v), nel caso
peggiore O(v²): [README, «Validazione OGC: la
ricerca delle auto-intersezioni non è quella di `geo`, il verdetto
sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(v + k).

#### Memoria

Memoria: da misura v4.

#### Esempio

Una riga per lato: la riga `i` della sinistra con la riga `i` della destra.

Passo del piano:

```json
{"out": "risultato", "op": "geo.symmetric_difference", "in": ["lotti", "vincoli"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |

Ingresso `vincoli` (geometrie `geometry` in EPSG:3857):

| `zona: utf8` | `geometry: geometry` |
| --- | --- |
| A | POLYGON((1 0,3 0,3 2,1 2,1 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOLYGON(((0 2,0 0,1 0,1 2,0 2)),((2 2,2 0,3 0,3 2,2 2))) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.to_wkt`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_to_wkt` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | misura terminale |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `utf8` con il testo WKT della geometria di ogni riga.
Il testo è quello del crate `wkt` vendorizzato: nome del tipo attaccato
alla parentesi (`POINT(1.5 -2)`), virgole senza spazi fra i vertici, ogni
coordinata nella forma decimale più breve che rilegge lo stesso `f64`,
senza esponente e con il segno dello zero conservato (`-0`); le geometrie
vuote come `POINT EMPTY`, `MULTIPOLYGON EMPTY`.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `wkt` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `utf8`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: un testo per riga, dal kernel `operations::to_wkt`; una geometria
nulla dà una cella nulla.

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto (ogni CRS risolto è
  ammesso);
- `InvalidPlan`: campi sconosciuti nella config, `output_column` vuoto o
  di soli spazi.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan`: `InvalidInput`, la geometria non supera la validazione
  OGC; `WktSerialization`, il writer rifiuta la geometria (un poligono con
  buchi e anello esterno vuoto, anche dentro una collezione), con un testo
  fisso: da una cella WKB non arriva, perché il validatore strutturale
  vuole almeno quattro coordinate per anello;
- `Internal` (`ValidazioneNonConclusa`): la validazione OGC va in panico
  dentro la barriera (il messaggio porta solo la forma del payload).

#### Limiti e deviazioni

Nessun SRID nel testo (non è EWKT) e nessuna Z/M. I numeri non si
arrotondano: un valore grande o piccolo si scrive per intero
(`1e21` diventa `1000000000000000000000`), dove `ST_AsText` di PostGIS
limita le cifre significative. Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Esatta: ogni coordinata si scrive con le cifre che la rileggono bit per bit
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) e memoria O(n) per il testo, più la
validazione OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel
peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.to_wkt", "in": ["siti"],
 "config": {}}
```

Ingresso `siti` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(1.5 -2) |
| 2 | POLYGON((0 0,4 0,4 4,0 4,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `wkt: utf8` |
| --- | --- | --- |
| 1 | POINT(1.5 -2) | POINT(1.5 -2) |
| 2 | POLYGON((0 0,4 0,4 4,0 4,0 0)) | POLYGON((0 0,4 0,4 4,0 4,0 0)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.translate`

| dal catalogo | |
| --- | --- |
| famiglia | geo, estensione |
| alias legacy | `translate` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | trasformazione sul posto |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 1, kernel 1 |

#### Che cosa fa

Sposta ogni geometria di `(x_offset, y_offset)` nelle unità del CRS: è
[`geo.affine_transform`](#geoaffine_transform) con la matrice
`[1, 0, x_offset, 0, 1, y_offset]`. Tipo e struttura della geometria non
cambiano.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `x_offset` | numero | obbligatorio | finito | spostamento lungo x |
| `y_offset` | numero | obbligatorio | finito | spostamento lungo y |

#### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati; la
colonna geometria resta al suo posto con lo stesso CRS, le stesse
dimensioni (XY) e gli stessi tipi geometrici dichiarati. Le proprietà del
contratto (`sorted_by`, `row_count`) restano.

#### Righe

1:1: il runner chiama il kernel (`extended::translate`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

#### Ordine

Per contratto quello d'ingresso (forma 1:1).

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, un offset assente o non
  finito;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi il kernel (`extended::translate`) rende `ExtendedError`, che il
runner porta in `Internal` per `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre: `InvalidInput`
(coordinate non finite o geometria non valida OGC), `InvalidOutput`
(uscita non valida OGC, per esempio coordinate che traboccano),
`ValidazioneNonConclusa` e `CalcoloNonConcluso` (validazione o calcolo
interrotti). Un offset non finito passato al kernel è
`InvalidParameter` con nome `coefficients`.

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

#### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Le coordinate d'uscita non si confrontano con il dominio di validità del
CRS ([README, «CRS integrati»](../README.md#crs-integrati)).

#### Precisione

Un arrotondamento per coordinata (`1 x + 0 y` è esatto): l'errore è al
più mezzo ulp del risultato, sotto 1 cm per coordinate d'uscita sotto
circa `7e13` unità del CRS. Esatto quando la somma è rappresentabile, come
con coordinate e offset interi. Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia spostata.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.translate", "in": ["pozzi"],
 "config": {"x_offset": 100, "y_offset": -50}}
```

Ingresso `pozzi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(1 2) |
| 2 | LINESTRING(0 0,3 4) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(101 -48) |
| 2 | LINESTRING(100 -50,103 -46) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.union`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_union` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Sostituisce la geometria della sinistra con la sua unione con la geometria
della destra, `sinistra ∪ destra`, calcolata dal kernel
`topology::boolean_operation_validated` ([README, «Operazioni
geo»](../README.md#operazioni-geo)). Lavora solo su `Polygon` e
`MultiPolygon` e rende sempre un `MultiPolygon`.

#### Parametri

Nessuno: la config è `{}`.

#### Schema

Quello della sinistra: stesse colonne, nello stesso ordine, con gli stessi
tipi; le colonne della destra non passano. La colonna geometria resta al
suo posto, con lo stesso nome e lo stesso CRS della sinistra (uguale a
quello della destra), in XY, ed è nullable anche quando quella della
sinistra non lo è (un risultato vuoto è nullo); i tipi geometrici
dichiarati diventano `MultiPolygon` (`exact`) e le chiavi dei tipi
ereditate dal campo si tolgono. Gli altri metadati di campo restano. I
metadati di schema sono la fusione dei due lati: una chiave presente da un
solo lato o uguale sui due passa. Le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

#### Righe

Allineate: la riga `i` della sinistra con la riga `i` della destra, e le
due tabelle devono avere le stesse righe (altrimenti `InvalidPlan`, in
esecuzione: le righe non si conoscono a secco). Una riga d'uscita per riga
della sinistra, con la geometria nulla dove una delle due è nulla o il
risultato è vuoto. Le parti che si toccano o si sovrappongono si fondono;
parti disgiunte restano poligoni distinti dello stesso `MultiPolygon`.

#### Ordine

Quello della sinistra.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config non vuota;
  un metadato di schema presente sui due lati con valori diversi;
- `Schema`: un lato senza esattamente una colonna geometria, o con una
  colonna non riconoscibile come geometria WKB (né estensione
  `geoarrow.wkb` né chiavi canoniche `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte);
- `InvalidPlan`: le due tabelle hanno un numero di righe diverso.

Dal kernel (`topology::boolean_operation_validated`, errore
`TopologyError`, sulle geometrie già validate: resta la validazione OGC
del risultato), nella categoria del passo geo indicata fra parentesi:

- `UnsupportedGeometry` (`InvalidPlan`): una geometria non è
  `Polygon`/`MultiPolygon`;
- `InvalidGeometry` (`InvalidPlan`): il risultato non supera la
  validazione OGC;
- `PrecisionInsufficient` (`Unsupported`): la griglia dell'overlay
  sposterebbe il risultato oltre la precisione (sotto, «Precisione»);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso` (`Internal`): la
  validazione OGC o l'overlay di `geo` non ha concluso.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Solo poligoni: una geometria lineare o puntuale, da un lato o dall'altro,
si rifiuta (`UnsupportedGeometry`), dove GEOS e PostGIS renderebbero una
collezione. Nessun
controllo a posteriori del risultato contro gli ingressi ([README,
«Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Precisione

La precisione è 1 cm a terra nelle unità del CRS della sinistra
(`Precision::from_crs`, calcolata in validazione). Entro 1 cm a terra: un
solo overlay di `i_overlay` su interi `i64`, con la griglia controllata
prima del calcolo sull'ingombro dei due operandi. Se lo spostamento a
priori supera mezzo centimetro, o le coordinate sono troppo rade per il
centimetro, `PrecisionInsufficient` e nessun calcolo. Parti più sottili di
1 cm possono sparire o fondersi senza errore; vedi [README, «Precisione
delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).

#### Complessità

Per coppia l'overlay a scansione di `i_overlay`, di norma O((v + k) log v)
con `v` i vertici dei due operandi e `k` gli incroci fra i lati, più la
validazione OGC di ingressi e risultato (di norma O(v log v), nel caso
peggiore O(v²): [README, «Validazione OGC: la
ricerca delle auto-intersezioni non è quella di `geo`, il verdetto
sì»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(v + k).

#### Memoria

Memoria: da misura v4.

#### Esempio

Una riga per lato: la riga `i` della sinistra con la riga `i` della destra.

Passo del piano:

```json
{"out": "risultato", "op": "geo.union", "in": ["lotti", "vincoli"],
 "config": {}}
```

Ingresso `lotti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,2 0,2 2,0 2,0 0)) |

Ingresso `vincoli` (geometrie `geometry` in EPSG:3857):

| `zona: utf8` | `geometry: geometry` |
| --- | --- |
| A | POLYGON((1 0,3 0,3 2,1 2,1 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | MULTIPOLYGON(((0 2,0 0,3 0,3 2,0 2))) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.vertex_count`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_vertex_count` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | streaming (1:1, batch per batch); cancellazione cooperativa |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS noto |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | misura terminale |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 2, kernel 1 |

#### Che cosa fa

Aggiunge una colonna `uint64` con il numero di coordinate della geometria
di ogni riga, sommato su tutte le parti. Ogni anello conta anche il vertice
di chiusura (un quadrato ne ha 5); una geometria vuota ne ha 0.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `vertex_count` | nome non vuoto (non di soli spazi) e non già presente nell'ingresso | colonna aggiunta |

#### Schema

Aggiunge in coda `output_column`, `uint64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati di
campo. Le altre colonne (geometria compresa), i metadati di schema e le
proprietà del contratto (`sorted_by`, `row_count`) passano invariati.

#### Righe

1:1: un conteggio per riga, dal kernel `operations::vertex_count`; una
geometria nulla dà una cella nulla.

#### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

#### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`); `output_column` esiste già;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto (ogni CRS risolto è
  ammesso);
- `InvalidPlan`: campi sconosciuti nella config, `output_column` vuoto o
  di soli spazi.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Dal kernel (`OperationError`), per geometria:

- `InvalidPlan` (`InvalidInput`): la geometria non supera la validazione
  OGC;
- `Internal` (`ValidazioneNonConclusa`, `Internal`): la validazione OGC va
  in panico dentro la barriera (il messaggio porta solo la forma del
  payload), o il conteggio non entra in `u64` (mai sulle piattaforme
  supportate).

#### Limiti e deviazioni

Il conteggio è quello di `ST_NPoints` di PostGIS: le coordinate ripetute
contano tutte. Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

#### Precisione

Esatta: è un conteggio
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

#### Complessità

Per geometria di n vertici: tempo O(n) per il conteggio, più la
validazione OGC dell'ingresso (sub-quadratica nel caso tipico, O(n²) nel
peggiore:
[README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì));
memoria O(n) per la validazione.

#### Memoria

Memoria: da misura v4.

#### Esempio

Passo del piano:

```json
{"out": "risultato", "op": "geo.vertex_count", "in": ["forme"],
 "config": {}}
```

Ingresso `forme` (geometrie `geometry` in EPSG:4326):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((0 0,4 0,4 4,0 4,0 0)) |
| 2 | LINESTRING(0 0,1 1,2 0) |
| 3 | POINT(1 1) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `vertex_count: uint64` |
| --- | --- | --- |
| 1 | POLYGON((0 0,4 0,4 4,0 4,0 0)) | 5 |
| 2 | LINESTRING(0 0,1 1,2 0) | 3 |
| 3 | POINT(1 1) | 1 |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.voronoi`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_voronoi` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | unaria |
| esecuzione | bloccante (tutto l'ingresso); cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | CRS proiettato |
| capability richieste | nessuna |
| vincolo di espansione | uscita / ingresso |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 3, config 1, analisi 2, kernel 2 |

#### Che cosa fa

Sostituisce il punto di ogni riga con la sua cella di Voronoi, calcolata sui
punti di tutte le righe: la regione del piano più vicina a quel punto che a
ogni altro. Le celle di bordo, infinite, si chiudono ritagliandole sul
rettangolo d'ingombro dei punti allargato su ogni lato della metà del suo
lato maggiore. I punti duplicati ricevono la stessa cella.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `max_points` | intero | `100000` | intero non negativo, almeno 2 | numero massimo di punti (righe non nulle) |

#### Schema

La colonna geometria si riscrive al suo posto, con lo stesso nome, lo
stesso CRS e dimensioni `xy`; i tipi dichiarati diventano esattamente
`Polygon` (le chiavi dei tipi ereditate si tolgono dai metadati del campo).
Le altre colonne, i metadati di schema e le proprietà del contratto
(`sorted_by`, `row_count`) restano.

#### Righe

1:1: una cella per riga, ma ogni cella dipende da tutte le righe (il kernel
`advanced::voronoi_cells` riceve tutti i punti insieme, e il limite
`max_points` come argomento). A ogni punto va la prima cella, in ordine di
prima comparsa del sito, che lo interseca. Il kernel non conosce i
null: il runner gli passa solo le geometrie non nulle, nell'ordine delle
righe, e riporta ogni cella alla sua riga; una riga nulla resta nulla e
non è un sito. Gli altri attributi restano invariati.

#### Ordine

Quello d'ingresso (parte del contratto, `semantic_version` 2). Stesso
ingresso, stessa uscita, anche sui punti cocircolari.

#### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti o
  `max_points` non intero non negativo; `max_points` minore di 2;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

Poi il kernel, con errore `AdvancedError` che il runner traduce così:
`ValidazioneNonConclusa` e `CalcoloNonConcluso` diventano `Internal`,
`PrecisionInsufficient` e `VerticeMalCondizionato` diventano
`Unsupported`, `PointLimitExceeded` diventa `ResourceLimit`, le altre
`InvalidPlan`; il messaggio è quello del kernel, e
un indice che vi compare conta le sole geometrie non nulle, non le righe.
Il kernel rifiuta l'intera colonna, nell'ordine, con `InsufficientPoints`
(meno di due righe non nulle), `PointLimitExceeded` (più righe non nulle
di `max_points`), `InvalidPoint` (una geometria non valida per l'OGC, con
il suo indice; `ValidazioneNonConclusa` se la validazione non conclude),
`ExpectedPoint`
(una geometria valida che non è un `Point`, con il suo indice), `Voronoi`
(coordinata non zero con modulo fuori da `[2^-142, 2^201]`, meno di due
punti distinti, punti tutti collineari), `PrecisionInsufficient` e
`VerticeMalCondizionato` (sotto, «Precisione»), `CalcoloNonConcluso`
(panico di `geo`, `spade` o `rstar`), `InvalidOutput` (cella non valida),
`UnmatchedPoint` (nessuna cella interseca un punto).

#### Limiti e deviazioni

- Celle costruite sulla triangolazione caricata in blocco di `spade`, con
  il corpo di `voronoi_cells` di `geo` 0.33.1 ricopiato e un circocentro
  indipendente dalla rotazione della faccia: rispetto all'inserimento
  incrementale un vertice può differire di qualche ulp, e con quattro o più
  punti cocircolari i circocentri vengono da triangoli diversi; il caso
  peggiore resta quadratico ([README, «`geo.delaunay` e `geo.voronoi`:
  triangolazione caricata in blocco»](../README.md#geodelaunay-e-geovoronoi-triangolazione-caricata-in-blocco)).
- Solo `Point`: un `MultiPoint` è `ExpectedPoint`. `ST_VoronoiPolygons` di
  PostGIS prende invece una geometria e rende una collezione di celle.
- Punti tutti collineari sono un errore, non celle a striscia.
- Ritaglio fisso sul rettangolo allargato della metà del lato maggiore:
  nessun parametro d'inviluppo.
- Nessuna diagnostica per riga: il passo rende il primo errore ([README,
  «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

#### Precisione

La precisione `p` è 1 cm a terra nelle unità del CRS della colonna: il
runner la ricava con `Precision::from_crs` e il kernel la riceve come
argomento ([README, «Precisione delle operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
I vertici interni delle celle sono circocentri calcolati con un maggiorante
del loro errore d'arrotondamento: oltre `p / 4` l'operazione si rifiuta
(`VerticeMalCondizionato`; succede con triangoli molto sottili sul bordo
dell'inviluppo: misurato sotto `3,4e-4 p` fino a 30 km di lato, ma a
1.000 km con 200.000 punti un triangolo arriva a `1,17 p`). Si
rifiuta con `PrecisionInsufficient` se la spaziatura dei `f64` supera
`p / 64` al modulo dei punti più la distanza dei punti lontani dei raggi, o
al modulo dei vertici delle celle prima del ritaglio, e se la griglia di
`i_overlay` del ritaglio di una cella di bordo supererebbe `p / 2` (lo
stesso controllo a priori delle booleane). I vertici sul rettangolo di
ritaglio vengono dall'intersezione di `geo` e non sono confrontati con
l'esatto dopo il calcolo.

#### Complessità

O(n log n) tipico sulla colonna, con `n` le righe: caricamento in blocco
della triangolazione, celle, ritaglio delle celle di bordo, associazione
punto-cella con un R-tree; quadratico nel caso peggiore. Memoria O(n).

#### Memoria

Memoria: da misura v4.

#### Esempio

Tre punti e un duplicato del primo, che riceve la stessa cella.

Passo del piano:

```json
{"out": "risultato", "op": "geo.voronoi", "in": ["siti"],
 "config": {"max_points": 1000}}
```

Ingresso `siti` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(0 0) |
| 2 | POINT(4 0) |
| 3 | POINT(0 4) |
| 4 | POINT(0 0) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POLYGON((-2 2,-2 -2,2 -2,2 2,-2 2)) |
| 2 | POLYGON((2 2,2 -2,6 -2,6 6,2 2)) |
| 3 | POLYGON((-2 6,-2 2,2 2,6 6,-2 6)) |
| 4 | POLYGON((-2 2,-2 -2,2 -2,2 2,-2 2)) |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.

### `geo.within`

| dal catalogo | |
| --- | --- |
| famiglia | geo, compatibile Manipola |
| alias legacy | `geo_within` (schema 3) (risolti da `find_operation`; il runner li rifiuta) |
| arietà | binaria ordinata (sinistra, destra) |
| esecuzione | bloccante su due ingressi; cancellazione solo ai confini |
| forma del risultato | 1:1 |
| determinismo | ordine definito dall'operazione |
| indice della riga sorgente | conservata |
| requisito CRS | stesso CRS proiettato sui due ingressi |
| capability richieste | nessuna |
| vincolo di espansione | uscita / sinistra |
| fusione geo | non fondibile |
| maturità | kernel validato |
| versioni | semantica 2, config 1, analisi 3, kernel 1 |

#### Che cosa fa

Aggiunge alla sinistra una colonna booleana che dice se la sua geometria
sta dentro almeno una geometria della destra (kernel
`analysis::within_indexes_validated`, predicato `within` del join
spaziale; [README, «Operazioni geo»](../README.md#operazioni-geo)).
«Dentro» è il `contains` di `geo` letto dalla destra: una geometria sul
solo bordo, come un punto sul lato di un poligono, non è dentro.

#### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `within` | nome non vuoto e libero nella sinistra | colonna aggiunta |

#### Schema

Le colonne della sinistra, invariate, più `output_column` in coda, `bool`,
nullable solo se lo è la geometria della sinistra (null dove è null). Le colonne della destra non passano. La colonna geometria resta
quella della sinistra, con i suoi tipi dichiarati. I metadati di schema
sono la fusione dei due lati; le proprietà del contratto della sinistra
(`sorted_by`, `row_count`) restano.

#### Righe

1:1 con la sinistra; la destra non aggiunge righe. Il kernel rende le
posizioni delle righe sinistre dentro almeno una destra, che nella colonna
sono `true`, le altre `false`. Una geometria sinistra nulla dà un valore
nullo; una vuota, o dentro solo geometrie destre nulle o vuote, `false`.
Le coppie (sinistra, destra) che il kernel conferma sono al più il limite
di righe dell'arco d'uscita (`max_output_rows` se il passo è un output
del piano, `max_rows_per_edge` altrimenti).

#### Ordine

Quello della sinistra.

#### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: un numero di ingressi diverso da due; config con campi
  sconosciuti; `output_column` vuota; un metadato di schema presente sui
  due lati con valori diversi;
- `Schema`: `output_column` esiste già nella sinistra; un lato senza
  esattamente una colonna geometria, o con una colonna non riconoscibile
  come geometria WKB (né estensione `geoarrow.wkb` né chiavi canoniche
  `plenora.geometry.*`);
- `Unsupported`: una colonna geometria non XY;
- `Crs`: un lato senza CRS risolto, un CRS non proiettato (o senza unità
  lineare), CRS dei due lati non equivalenti.

In esecuzione, prima del kernel, su ogni cella non nulla dei due lati
([README, «Operazioni geo»](../README.md#operazioni-geo)):

- `Schema`: il contratto di un lato dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `InvalidPlan`: la cella viola il contratto WKB o non supera la
  validazione OGC (`Unsupported` per dimensioni Z o M, `Internal` se la
  validazione non conclude, `ResourceLimit` per una cella oltre il limite
  di byte).

Dal kernel (`analysis::within_indexes_validated`, errore `AnalysisError`
che avvolge `SpatialJoinError`, sulle geometrie già validate), nella
categoria del passo geo indicata fra parentesi:

- `PairLimitExceeded` (`ResourceLimit`): le coppie (sinistra, destra)
  confermate superano il limite di righe dell'arco; conta ogni destra che
  contiene una sinistra, anche se ne basta una;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`, `Internal` (`Internal`):
  l'indice o il predicato di `geo` non ha concluso, o un'invariante
  interna violata;
- `IndexOverflow` (`InvalidPlan`): un numero di righe non entra in `u64`.

Il primo errore è quello della prima riga, in ordine di riga, senza
diagnostica per riga ([README, «Limiti dichiarati del
runner»](../README.md#limiti-dichiarati-del-runner), voce «Geo senza
diagnostica per riga»).

#### Limiti e deviazioni

Il limite delle coppie conta tutte le destre che contengono una
sinistra, non solo la prima: una sinistra dentro molte destre sovrapposte
può superarlo anche se la colonna ha una riga per riga sinistra.

#### Precisione

Nessun calcolo di geometrie e nessuna griglia: il predicato di `geo` si
valuta sulle coordinate `f64` d'ingresso, senza tolleranza, e la regola di
1 cm non sposta nulla. Una geometria a meno di 1 cm dal bordo è dentro o
fuori secondo le sue coordinate esatte ([README, «Precisione delle
operazioni geografiche: 1 cm a
terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
«Feature d'ingresso più vicine della precisione»).

#### Complessità

Un R-tree dei rettangoli d'ingombro della destra, O(m log m) per `m`
righe; per ognuna delle `n` righe sinistre una ricerca nell'albero e il
predicato esatto sui soli candidati. Di norma O((n + m) log m) più il
costo dei predicati; nel caso peggiore (rettangoli tutti sovrapposti)
O(n · m) predicati. Più la validazione OGC di ogni geometria. Memoria O(m)
per l'albero più le coppie confermate.

#### Memoria

Memoria: da misura v4.

#### Esempio

Il secondo punto sta sul bordo del poligono, quindi non è dentro.

Passo del piano:

```json
{"out": "risultato", "op": "geo.within", "in": ["pozzi", "aree"],
 "config": {}}
```

Ingresso `pozzi` (geometrie `geometry` in EPSG:3857):

| `id: int64` | `geometry: geometry` |
| --- | --- |
| 1 | POINT(1 1) |
| 2 | POINT(0 1) |
| 3 | POINT(5 5) |

Ingresso `aree` (geometrie `geometry` in EPSG:3857):

| `nome: utf8` | `geometry: geometry` |
| --- | --- |
| parco | POLYGON((0 0,2 0,2 2,0 2,0 0)) |

Uscita `risultato`:

| `id: int64` | `geometry: geometry` | `within: bool` |
| --- | --- | --- |
| 1 | POINT(1 1) | true |
| 2 | POINT(0 1) | false |
| 3 | POINT(5 5) | false |

Verifica: eseguito dal runner come passo unico; l'uscita è confrontata cella per cella.
