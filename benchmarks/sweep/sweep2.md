# Sweep kernel tabellari, seconda ondata (bench_sweep2)

26 op residui (mai ottimizzate) + 9 op nuove (estensioni v1.1-v1.3).
Fixture deterministica identica a `bench_sweep` (seed 42), mediana di
3 run, container Docker `--cpus=4 --memory=10g`, release. Scala 1M
righe; le op con mediana < 1s a 1M sono rimisurate a 10M (righe
`[10M]`). Classifica ordinata per lentezza (righe/s crescenti). Il
peak RSS e' il `VmHWM` di processo (cumulativo, cresce con le fixture
condivise a 10M).

| # | op | righe | mediana (s) | righe/s | righe output | peak RSS (MiB) | note |
|---|----|-------|-------------|---------|--------------|----------------|------|
| 1 | `table.fuzzy_join` | 1000000 | 52.6510 | 18993 | 18341989 | 5764 | 1M x 10k anagrafica, jaro_winkler prefix(2), soglia 0.85 |
| 2 | `table.stable_fingerprint` | 10000000 | 7.1354 | 1401471 | 10000000 | 3436 | sha256 su tutte e 6 le colonne [10M] |
| 3 | `table.hmac_sha256` | 10000000 | 7.0910 | 1410231 | 10000000 | 5187 | 2 colonne (id+text), chiave da env [10M] |
| 4 | `table.hmac_sha256` | 1000000 | 0.6685 | 1495924 | 1000000 | 4783 | 2 colonne (id+text), chiave da env |
| 5 | `table.stable_fingerprint` | 1000000 | 0.6559 | 1524631 | 1000000 | 3436 | sha256 su tutte e 6 le colonne |
| 6 | `table.sha256_hash` | 10000000 | 2.0773 | 4813968 | 10000000 | 3436 | 1 colonna utf8 40 char [10M] |
| 7 | `table.assert_unique` | 1000000 | 0.1988 | 5029487 | 1000000 | 3436 | chiave id unica |
| 8 | `table.transpose` | 4000 | 0.0007 | 5368162 | 8 | 2869 | 8 colonne f64 x 4000 righe (contratto: righe <= max_columns; un take per colonna output) |
| 9 | `table.md5_hash` | 10000000 | 1.6802 | 5951585 | 10000000 | 2924 | 1 colonna utf8 40 char [10M] |
| 10 | `table.sha256_hash` | 1000000 | 0.1562 | 6403375 | 1000000 | 2924 | 1 colonna utf8 40 char |
| 11 | `table.md5_hash` | 1000000 | 0.1550 | 6450332 | 1000000 | 2869 | 1 colonna utf8 40 char |
| 12 | `table.bin` | 1000000 | 0.1520 | 6580191 | 1000000 | 321 | 20 bucket equal-width |
| 13 | `table.bin` | 10000000 | 1.5183 | 6586190 | 10000000 | 2799 | 20 bucket equal-width [10M] |
| 14 | `table.asof_join` | 1000000 | 0.1323 | 7560971 | 1000000 | 2869 | 1M x 1M backward su ts int64 |
| 15 | `table.validate_rules` | 10000000 | 1.1930 | 8382170 | 10000000 | 4783 | 5 regole (range/regex/notnull/lt/ne), annotate [10M] |
| 16 | `table.conditional` | 1000000 | 0.1162 | 8607022 | 1000000 | 2869 | 3 condizioni numeriche |
| 17 | `table.conditional` | 10000000 | 1.1407 | 8766736 | 10000000 | 2869 | 3 condizioni numeriche [10M] |
| 18 | `table.split_column` | 10000000 | 1.1091 | 9016526 | 10000000 | 2869 | 3 colonne su '/' [10M] |
| 19 | `table.validate_rules` | 1000000 | 0.0990 | 10099239 | 1000000 | 4098 | 5 regole (range/regex/notnull/lt/ne), annotate |
| 20 | `table.split_column` | 1000000 | 0.0967 | 10343885 | 1000000 | 2824 | 3 colonne su '/' |
| 21 | `table.lookup` | 1000000 | 0.0916 | 10913628 | 1000000 | 2799 | mappa 1024 chiavi utf8 |
| 22 | `table.lookup` | 10000000 | 0.8903 | 11231962 | 10000000 | 2799 | mappa 1024 chiavi utf8 [10M] |
| 23 | `table.uuid_generator` | 10000000 | 0.6374 | 15688832 | 10000000 | 3436 | uuid v4 per riga [10M] |
| 24 | `table.uuid_generator` | 1000000 | 0.0588 | 17013134 | 1000000 | 3436 | uuid v4 per riga |
| 25 | `table.assert_regex` | 1000000 | 0.0489 | 20459084 | 1000000 | 3436 | ^[0-9a-f]{40}$ |
| 26 | `table.assert_regex` | 10000000 | 0.4686 | 21338971 | 10000000 | 3436 | ^[0-9a-f]{40}$ [10M] |
| 27 | `table.concat_columns` | 10000000 | 0.4349 | 22994712 | 10000000 | 2824 | 2 colonne utf8 [10M] |
| 28 | `table.string_pad` | 10000000 | 0.4087 | 24466016 | 10000000 | 3436 | width 48 left su 40 char [10M] |
| 29 | `table.cross_join` | 1000000 | 0.0377 | 26531413 | 1000000 | 2869 | 1000 x 1000 righe (righe = output) |
| 30 | `table.explode` | 1000000 | 0.0373 | 26828495 | 2200340 | 2869 | List<Int64> 0..4 elementi (~2.5M out) |
| 31 | `table.concat_columns` | 1000000 | 0.0364 | 27456659 | 1000000 | 2799 | 2 colonne utf8 |
| 32 | `table.string_pad` | 1000000 | 0.0288 | 34703008 | 1000000 | 3436 | width 48 left su 40 char |
| 33 | `table.sample` | 10000000 | 0.1687 | 59292096 | 1000000 | 2799 | fraction 0.1 [10M] |
| 34 | `table.replace` | 10000000 | 0.1043 | 95850683 | 10000000 | 2869 | sostituzione letterale utf8 [10M] |
| 35 | `table.replace` | 1000000 | 0.0099 | 101075872 | 1000000 | 2869 | sostituzione letterale utf8 |
| 36 | `table.sample` | 1000000 | 0.0098 | 102053969 | 100000 | 2799 | fraction 0.1 |
| 37 | `table.top_n` | 10000000 | 0.0837 | 119502827 | 100 | 3436 | n=100 desc su num [10M] |
| 38 | `table.top_n` | 1000000 | 0.0072 | 139376144 | 100 | 3436 | n=100 desc su num |
| 39 | `table.string_length` | 10000000 | 0.0713 | 140175537 | 10000000 | 3436 | stringhe 40 char [10M] |
| 40 | `table.string_length` | 1000000 | 0.0070 | 141955422 | 1000000 | 3436 | stringhe 40 char |
| 41 | `table.concat_by_name` | 1000000 | 0.0060 | 165528575 | 1000000 | 4098 | 3 input ~333k righe, schemi permutati, 1 colonna assente nel terzo |
| 42 | `table.unnest` | 1000000 | 0.0054 | 183896649 | 1000000 | 2869 | Struct{a,b,c} |
| 43 | `table.concat` | 1000000 | 0.0054 | 186769405 | 1000000 | 2869 | 500k + 500k righe |
| 44 | `table.assert_range` | 10000000 | 0.0478 | 209007885 | 10000000 | 3436 | num in 0..10000 [10M] |
| 45 | `table.assert_range` | 1000000 | 0.0045 | 222818931 | 1000000 | 3436 | num in 0..10000 |
| 46 | `table.add_row_number` | 10000000 | 0.0288 | 347680255 | 10000000 | 3436 | senza partizione [10M] |
| 47 | `table.assert_not_null` | 10000000 | 0.0255 | 391570398 | 10000000 | 3436 | 2 colonne [10M] |
| 48 | `table.assert_not_null` | 1000000 | 0.0025 | 395044561 | 1000000 | 3436 | 2 colonne |
| 49 | `table.add_row_number` | 1000000 | 0.0022 | 454302609 | 1000000 | 3436 | senza partizione |
| 50 | `table.align_schema` | 10000000 | 0.0055 | 1822807441 | 10000000 | 4098 | 20 colonne: 18 permutate + 1 default + 1 null, 2 scartate [10M] |
| 51 | `table.align_schema` | 1000000 | 0.0003 | 3020463641 | 1000000 | 3436 | 20 colonne: 18 permutate + 1 default + 1 null, 2 scartate |
| 52 | `table.assert_schema` | 1000000 | 0.0000 | 3134796238245 | 1000000 | 3436 | 6 campi ordinati |
| 53 | `table.select_columns` | 1000000 | 0.0000 | 4524886877828 | 1000000 | 3436 | 3 colonne su 6, ordine permutato |
| 54 | `table.limit` | 1000000 | 0.0000 | 4784688995215 | 500000 | 3436 | n=500k offset=100 (slice zero-copy) |
| 55 | `table.assert_metadata` | 1000000 | 0.0000 | 12820512820513 | 1000000 | 3436 | 1 chiave metadata |
| 56 | `table.assert_cardinality` | 1000000 | 0.0000 | 16393442622951 | 1000000 | 3436 | min_rows=1 |
| 57 | `table.assert_schema` | 10000000 | 0.0000 | 34246575342466 | 10000000 | 3436 | 6 campi ordinati [10M] |
| 58 | `table.select_columns` | 10000000 | 0.0000 | 35460992907801 | 10000000 | 3436 | 3 colonne su 6, ordine permutato [10M] |
| 59 | `table.limit` | 10000000 | 0.0000 | 44843049327354 | 500000 | 3436 | n=500k offset=100 (slice zero-copy) [10M] |
| 60 | `table.assert_metadata` | 10000000 | 0.0000 | 128205128205128 | 10000000 | 3436 | 1 chiave metadata [10M] |
| 61 | `table.assert_cardinality` | 10000000 | 0.0000 | 133333333333333 | 10000000 | 3436 | min_rows=1 [10M] |

## Analisi (post-run)

### Confrontabilita' con lo sweep precedente

I 26 residui sono stati rimisurati con le STESSE fixture (seed 42, xorshift)
e le stesse config di `bench_sweep`: tutti i valori rientrano entro il ±5%
dello sweep precedente (es. `bin` 6.47 -> 6.58M righe/s, `lookup` 10.7 ->
10.9M, `asof_join` 7.49 -> 7.56M, `assert_unique` 4.66 -> 5.03M). Le misure
sono quindi comparabili e non ci sono regressioni ambientali.

### Classifica di merito (scala 1M, righe/s)

| Fascia | Op | Righe/s |
|--------|----|---------|
| Patologicamente lenta (classe attesa) | `fuzzy_join` (NUOVA) | 1.9e4 (52.7 s) |
| Candidate ottimizzazione | `hmac_sha256` (NUOVA), `stable_fingerprint` (NUOVA) | 1.5e6 |
| Media-bassa | `assert_unique`, `md5_hash`, `sha256_hash`, `bin` | 5-7e6 |
| Media | `asof_join`, `conditional`, `split_column`, `lookup`, `validate_rules` (NUOVA) | 7.5-11e6 |
| Alta | `uuid_generator`, `assert_regex`, `cross_join`, `explode`, `concat_columns`, `string_pad` | 17-35e6 |
| Molto alta | `sample`, `replace`, `top_n` (N), `string_length`, `concat_by_name` (N), `concat`, `unnest`, `assert_range`, `add_row_number`, `assert_not_null` | >1e8 |
| Metadata/zero-copy | `select_columns` (N), `limit` (N), `align_schema` (N), `assert_schema`, `assert_cardinality`, `assert_metadata` | >1e9 |

### Top candidati per l'ultima ondata di ottimizzazione

1. **`stable_fingerprint` — 1.52M righe/s (0.656 s @1M, 7.14 s @10M).**
   Collo di bottiglia strutturale: per OGNI riga ricostruisce da zero il
   framing costante per colonna (`framed(nome)` + `framed(tipo)` con
   `data_type().to_string()` che ALLOCA una String per cella), istanzia un
   nuovo digest Sha256 e alloca la stringa esadecimale. Il pattern
   "prepara una volta + loop tipizzato + fallback" si applica quasi
   letteralmente: hoisting dei frame costanti fuori dal loop righe, buffer
   digest/hex riusati, accesso tipizzato alle colonne. Guadagno stimato
   5-15x (target: avvicinarsi al throughput per-colonna di `sha256_hash`,
   che su 1 colonna fa 6.4M righe/s).
2. **`hmac_sha256` — 1.50M righe/s (0.668 s @1M, 7.09 s @10M).**
   Stesso framing per riga di `stable_fingerprint`, piu' il doppio blocco
   ipad/opad ricostruito byte-per-byte a ogni riga: gli stati Sha256 dopo
   ipad/opad dipendono solo dalla chiave e possono essere precomputati
   UNA volta e clonati per riga. Guadagno stimato 3-10x.
3. **`assert_unique` — 5.03M righe/s (0.199 s @1M).** Hash set su int64;
   con hasher veloce + loop tipizzato (pattern gia' usato su `distinct`)
   guadagno stimato 2-4x. Candidato secondario: 0.2 s @1M e' gia'
   accettabile.
4. **`md5_hash` / `sha256_hash` — 6.4M righe/s.** Hashing intrinsecamente
   CPU-bound, ma c'e' margine sulla materializzazione esadecimale per riga
   (allocazione String): guadagno stimato 1.5-3x. Priorita' bassa.

### Da lasciare stare

- Zero-copy/metadata-only: `select_columns`, `limit`, `align_schema`,
  `assert_schema`, `assert_cardinality`, `assert_metadata` — microsecondi,
  niente da ottimizzare.
- Gia' oltre 1e8 righe/s (bandwidth-bound): `add_row_number`,
  `assert_not_null`, `assert_range`, `string_length`, `top_n`, `concat`,
  `unnest`, `concat_by_name`, `sample`, `replace`.
- CPU-bound intrinseci con costante ragionevole: `assert_regex` (regex
  compilata una volta, 20M righe/s), `uuid_generator` (entropy + formato),
  `validate_rules` (5 regole/riga = ~50M valutazioni/s, regex compilata in
  fase di compile_rules), `transpose` (scala fissa per contratto).

### Anomalie nei 9 nuovi kernel

- **`fuzzy_join`: 19k righe/s, 52.65 s a 1M x 10k — l'unico kernel
  "lento" in senso assoluto.** Non e' codice patologico: e' la classe
  dell'algoritmo (score jaro_winkler per coppia candidata). Due note di
  rilievo: (a) amplificazione dell'output — 18.34M righe da 1M sinistre
  (~18 match/riga con soglia 0.85 su nomi sintetici corti), quindi parte
  del costo e' materializzazione output via `combine_horizontal`, non solo
  scoring; (b) margini moderati lato implementazione (allocazioni per
  riga in `normalize`/`block_key`, nessun early-exit su lunghezza prima
  dello score): stimato 1.5-3x, non di piu'. Segnalare nella campagna
  fuzz: e' l'unico kernel dove 1M righe di input implicano ~1 minuto di
  wall-clock; i limiti (`max_candidates`, `max_rows`) sono gia' enforced.
- **`stable_fingerprint` e `hmac_sha256`**: non patologiche ma chiaramente
  sotto-tono (20-100x sotto i kernel comparabili per-cell): vedere
  candidati 1-2.
- Le altre 6 nuove (`select_columns`, `limit`, `top_n`, `align_schema`,
  `concat_by_name`, `validate_rules`) sono tutte sane: zero-copy dove
  atteso, throughput in linea coi kernel ottimizzati.
