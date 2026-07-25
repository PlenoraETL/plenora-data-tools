# Sweep kernel tabellari non ottimizzati (bench_sweep)

Fixture deterministica (seed 42), mediana di 3 run, container Docker
`--cpus=4 --memory=10g`, release. Scala 1M righe; le op con mediana
< 1s a 1M sono rimisurate a 10M (righe `[10M]`). Classifica ordinata
per lentezza (righe/s crescenti). Il peak RSS e' il `VmHWM` di
processo (cumulativo, cresce con le fixture condivise a 10M).

| # | op | righe | mediana (s) | righe/s | righe output | peak RSS (MiB) | note |
|---|----|-------|-------------|---------|--------------|----------------|------|
| 1 | `table.statistics` | 1000000 | 100.6004 | 9940 | 1000000 | 2800 | 10 statistiche x 1024 gruppi |
| 2 | `table.flatten_json` | 1000000 | 1.2656 | 790111 | 1000000 | 2800 | JSON annidati 3 livelli |
| 3 | `table.table_diff` | 1000000 | 1.0810 | 925106 | 1000000 | 2892 | 1M x 1M, chiave id, diff su num |
| 4 | `table.reconcile` | 1000000 | 0.8310 | 1203319 | 5 | 3688 | 1M x 1M, frequenze chiave |
| 5 | `table.union_distinct` | 1000000 | 0.6413 | 1559301 | 1500000 | 2892 | 1M + 1M righe, overlap 50% |
| 6 | `table.except` | 1000000 | 0.6089 | 1642331 | 500000 | 2892 | 1M x 1M righe, overlap 50% |
| 7 | `table.distinct` | 10000000 | 6.0435 | 1654682 | 999943 | 2892 | subset key, ~1M valori distinti su 10M righe [10M dichiarate] |
| 8 | `table.assert_foreign_key` | 1000000 | 0.5581 | 1791926 | 1000000 | 3688 | 1M chiavi vs 1M referenze |
| 9 | `table.dedup_advanced` | 1000000 | 0.4521 | 2212068 | 631781 | 2892 | subset key, order id |
| 10 | `table.intersect` | 1000000 | 0.3462 | 2888774 | 500000 | 2892 | 1M x 1M righe, overlap 50% |
| 11 | `table.mask_data` | 10000000 | 2.4881 | 4019057 | 10000000 | 3688 | mask custom 3+3 su text [10M] |
| 12 | `table.mask_data` | 1000000 | 0.2359 | 4239756 | 1000000 | 3688 | mask custom 3+3 su text |
| 13 | `table.window_function` | 1000000 | 0.2311 | 4327098 | 1000000 | 2892 | rank, partizione grp, order num |
| 14 | `table.transpose` | 4000 | 0.0009 | 4418757 | 8 | 2892 | 8 colonne f64 x 4000 righe (contratto: righe <= max_columns; un take per colonna output) |
| 15 | `table.assert_unique` | 1000000 | 0.2148 | 4655973 | 1000000 | 3688 | chiave id unica |
| 16 | `table.rolling_window` | 1000000 | 0.2065 | 4842439 | 1000000 | 2892 | mean w=10, partizione grp |
| 17 | `table.sha256_hash` | 10000000 | 2.0547 | 4866794 | 10000000 | 3688 | 1 colonna utf8 40 char [10M] |
| 18 | `table.md5_hash` | 10000000 | 1.8870 | 5299304 | 10000000 | 3176 | 1 colonna utf8 40 char [10M] |
| 19 | `table.md5_hash` | 1000000 | 0.1662 | 6016334 | 1000000 | 2892 | 1 colonna utf8 40 char |
| 20 | `table.sha256_hash` | 1000000 | 0.1573 | 6359082 | 1000000 | 3176 | 1 colonna utf8 40 char |
| 21 | `table.bin` | 1000000 | 0.1545 | 6470854 | 1000000 | 321 | 20 bucket equal-width |
| 22 | `table.bin` | 10000000 | 1.5111 | 6617553 | 10000000 | 2800 | 20 bucket equal-width [10M] |
| 23 | `table.asof_join` | 1000000 | 0.1336 | 7485396 | 1000000 | 2892 | 1M x 1M backward su ts int64 |
| 24 | `table.conditional` | 10000000 | 1.2065 | 8288288 | 10000000 | 2892 | 3 condizioni numeriche [10M] |
| 25 | `table.conditional` | 1000000 | 0.1115 | 8972472 | 1000000 | 2892 | 3 condizioni numeriche |
| 26 | `table.split_column` | 10000000 | 1.1144 | 8973647 | 10000000 | 2892 | 3 colonne su '/' [10M] |
| 27 | `table.lookup` | 10000000 | 0.9848 | 10154727 | 10000000 | 2800 | mappa 1024 chiavi utf8 [10M] |
| 28 | `table.split_column` | 1000000 | 0.0951 | 10517754 | 1000000 | 2824 | 3 colonne su '/' |
| 29 | `table.lookup` | 1000000 | 0.0938 | 10663267 | 1000000 | 2800 | mappa 1024 chiavi utf8 |
| 30 | `table.uuid_generator` | 10000000 | 0.8024 | 12462268 | 10000000 | 3688 | uuid v4 per riga [10M] |
| 31 | `table.uuid_generator` | 1000000 | 0.0630 | 15872824 | 1000000 | 3688 | uuid v4 per riga |
| 32 | `table.assert_regex` | 10000000 | 0.4735 | 21120402 | 10000000 | 3688 | ^[0-9a-f]{40}$ [10M] |
| 33 | `table.assert_regex` | 1000000 | 0.0468 | 21350012 | 1000000 | 3688 | ^[0-9a-f]{40}$ |
| 34 | `table.string_pad` | 10000000 | 0.4428 | 22582466 | 10000000 | 3688 | width 48 left su 40 char [10M] |
| 35 | `table.concat_columns` | 10000000 | 0.4085 | 24477913 | 10000000 | 2824 | 2 colonne utf8 [10M] |
| 36 | `table.string_pad` | 1000000 | 0.0397 | 25170789 | 1000000 | 3688 | width 48 left su 40 char |
| 37 | `table.explode` | 1000000 | 0.0371 | 26950738 | 2200340 | 2892 | List<Int64> 0..4 elementi (~2.5M out) |
| 38 | `table.cross_join` | 1000000 | 0.0345 | 28981055 | 1000000 | 2892 | 1000 x 1000 righe (righe = output) |
| 39 | `table.concat_columns` | 1000000 | 0.0328 | 30492183 | 1000000 | 2800 | 2 colonne utf8 |
| 40 | `table.sample` | 10000000 | 0.1721 | 58113188 | 1000000 | 2800 | fraction 0.1 [10M] |
| 41 | `table.sample` | 1000000 | 0.0102 | 98114572 | 100000 | 2800 | fraction 0.1 |
| 42 | `table.replace` | 10000000 | 0.1003 | 99689798 | 10000000 | 2892 | sostituzione letterale utf8 [10M] |
| 43 | `table.replace` | 1000000 | 0.0096 | 104043516 | 1000000 | 2892 | sostituzione letterale utf8 |
| 44 | `table.string_length` | 10000000 | 0.0758 | 131923782 | 10000000 | 3688 | stringhe 40 char [10M] |
| 45 | `table.string_length` | 1000000 | 0.0073 | 137021439 | 1000000 | 3688 | stringhe 40 char |
| 46 | `table.concat` | 1000000 | 0.0051 | 194948609 | 1000000 | 2892 | 500k + 500k righe |
| 47 | `table.unnest` | 1000000 | 0.0050 | 198411006 | 1000000 | 2892 | Struct{a,b,c} |
| 48 | `table.assert_range` | 10000000 | 0.0393 | 254362818 | 10000000 | 3688 | num in 0..10000 [10M] |
| 49 | `table.assert_range` | 1000000 | 0.0038 | 260452275 | 1000000 | 3688 | num in 0..10000 |
| 50 | `table.add_row_number` | 1000000 | 0.0029 | 343780360 | 1000000 | 3688 | senza partizione |
| 51 | `table.add_row_number` | 10000000 | 0.0263 | 379791132 | 10000000 | 3688 | senza partizione [10M] |
| 52 | `table.assert_not_null` | 1000000 | 0.0025 | 396385283 | 1000000 | 3688 | 2 colonne |
| 53 | `table.assert_not_null` | 10000000 | 0.0247 | 404862201 | 10000000 | 3688 | 2 colonne [10M] |
| 54 | `table.rename` | 1000000 | 0.0000 | 1440922190202 | 1000000 | 2800 | 2 rinomini |
| 55 | `table.reorder_columns` | 1000000 | 0.0000 | 2463054187192 | 1000000 | 2800 | ordine inverso |
| 56 | `table.drop_columns` | 1000000 | 0.0000 | 3759398496241 | 1000000 | 2800 | drop 2 colonne su 6 |
| 57 | `table.assert_schema` | 1000000 | 0.0000 | 4830917874396 | 1000000 | 3688 | 6 campi ordinati |
| 58 | `table.assert_metadata` | 1000000 | 0.0000 | 12048192771084 | 1000000 | 3688 | 1 chiave metadata |
| 59 | `table.rename` | 10000000 | 0.0000 | 15174506828528 | 10000000 | 2800 | 2 rinomini [10M] |
| 60 | `table.assert_cardinality` | 1000000 | 0.0000 | 16666666666667 | 1000000 | 3688 | min_rows=1 |
| 61 | `table.reorder_columns` | 10000000 | 0.0000 | 19762845849802 | 10000000 | 2800 | ordine inverso [10M] |
| 62 | `table.drop_columns` | 10000000 | 0.0000 | 37037037037037 | 10000000 | 2800 | drop 2 colonne su 6 [10M] |
| 63 | `table.assert_schema` | 10000000 | 0.0000 | 46082949308756 | 10000000 | 3688 | 6 campi ordinati [10M] |
| 64 | `table.assert_metadata` | 10000000 | 0.0000 | 138888888888889 | 10000000 | 3688 | 1 chiave metadata [10M] |
| 65 | `table.assert_cardinality` | 10000000 | 0.0000 | 188679245283019 | 10000000 | 3688 | min_rows=1 [10M] |

## Commento

Il quadro e' fortemente asimmetrico: 3 op sotto 1M righe/s, una fascia
"calda" di ~10 op tra 1M e 8M righe/s, e tutto il resto oltre 10M righe/s
(o solo metadati, tempo trascurabile).

- **`table.statistics` e' l'outlier assoluto** (~9.9k righe/s a 1M, 100 s):
  ~650x piu' lenta della seconda. Per ogni gruppo riesegue sort separati per
  ciascuna statistica basata su quantili e ricalcola i momenti stat per stat:
  basta un sort unico per gruppo + momenti in singola passata. Guadagno
  atteso: ordini di grandezza. Priorita' 1.
- **Fascia 0.8-3M righe/s** (`flatten_json`, `table_diff`, `reconcile`,
  `union_distinct`, `except`, `distinct`, `assert_foreign_key`,
  `dedup_advanced`, `intersect`): dominata da codifica chiavi per-riga con
  allocazioni (`key_for_row`/`composite_key` su `String`/`BTreeMap`) o da
  parsing JSON riga per riga. `table_diff`/`reconcile`/`distinct`/
  `dedup_advanced`/`assert_foreign_key` beneficerebbero del pattern
  `CompactRowEncoder` gia' usato dalle setops (zero-copy, senza String);
  `flatten_json` richiede parsing selettivo dei path. Guadagno atteso 2-10x.
- **Fascia 4-8M righe/s** (`mask_data`, `window_function`,
  `assert_unique`, `rolling_window`, `md5_hash`, `sha256_hash`, `bin`,
  `asof_join`): accessi scalari per-riga (`scalar_as_f64`/`scalar_as_string`)
  e, per `rolling_window`, riaggregazione della finestra con `Vec` allocato a
  ogni riga (O(n*w) con allocazione). `mask_data` fa `chars().collect()` per
  riga: fix banale. md5/sha256 sono vicini al costo crittografico: guadagno
  marginale, candidati deboli.
- **Lasciar stare**: op di soli metadati (`rename`, `drop_columns`,
  `reorder_columns`, `assert_schema`, `assert_metadata`,
  `assert_cardinality`), le gia' veloci (>15M righe/s: `string_length`,
  `replace`, `sample`, `concat`, `unnest`, `explode`, `assert_not_null`,
  `assert_range`, `assert_regex`, `add_row_number`, `string_pad`,
  `concat_columns`, `uuid_generator`, `lookup`, `split_column`,
  `conditional`, `cross_join`) e `transpose`, limitata per contratto a
  `max_columns` righe (4095) e gia' <1 ms a scala massima.

Nota di metodo: il peak RSS e' il `VmHWM` cumulativo di processo (le fixture
condivise a 10M lo portano a ~3.7 GiB); per-scenario va letto come bound
superiore monotono, non come consumo puntuale dell'op.
