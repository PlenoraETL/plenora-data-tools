# Baseline prestazionale — Fase 1f (progetti di origine)

Data: 2026-07-24. Riferimento per il benchmark gate di `Prestazioni.md` §9:
ogni rilascio del workspace si confronta con questa baseline. I dati grezzi
sono in `raw/`, il documento macchinabile in `baseline.json`, gli harness e lo
script di consolidamento in `harness/`.

## Ambiente

- CPU: 13th Gen Intel Core i9-13900K, 32 CPU logiche visibili dal container;
  RAM host 63,7 GB, RAM visibile dal container 32 GB.
- Runtime: Docker 29.6.1 (Docker Desktop su WSL2, kernel
  6.18.33.2-microsoft-standard), immagine `rust:1.92` (bookworm);
  progetti montati via bind mount Windows (I/O su `/work` più lento del nativo).
- Toolchain: rustc 1.92.0 (ded5c06cf 2025-12-08), cargo 1.92.0.
- Versioni: plenora-nogeo-tools 0.1.0, plenora-geo 0.1.0, arrow 59.1.0,
  geo 0.33.1, geozero 0.15.1; build full-backends con GEOS 3.14 e PROJ statici.
- Build: nogeo `cargo build --release --examples`; geo `--features
  arrow-transport` (baseline principale) e `--features full-backends`
  (solo topologia estesa). I progetti di origine non sono repository git:
  la baseline vale per lo stato del filesystem alla data sopra.

## Metodologia

- Benchmark in-process: warmup + mediana di 5 ripetizioni (3 a 10M righe e per
  i kernel geo), un processo per benchmark; picco RSS da `VmHWM` di
  `/proc/self/status`.
- Benchmark CLI (trasporto v3): run singolo per processo, wall time e picco RSS
  da `/usr/bin/time -v`.
- Fixture tabellare: quella di `examples/candidate_benchmark.rs` (id int64, num
  float64, group utf8 con 1024 gruppi, text utf8); per string transform e
  pipeline quella di `examples/benchmark.rs` (name/code utf8 con null).
- Fixture geo kernel: box a 5 vertici da LCG seed 42 (`harness/geo_kernels`),
  coordinate in [0, 10000), lati in [0.1, 50), WKB LE 2D (93 MB a 1M).
- Fixture trasporto: envelope PLNGEO3 rigenerabile con
  `harness/make_geo_fixture.py --rows N --seed 42` (stessa fixture di
  `spike_arrow.py`: box casuali, attributi id/label/weight, 5% null,
  metadati geoarrow.wkb, EPSG:3857; 1M = 123.363.400 byte).
- Parametri kernel: buffer distanza 10.0 cap round; simplify Douglas-Peucker
  tolleranza 1.0; intersects su coppie deterministiche di box (~50% positive).

## 1. Tabellare (kernel in-process, nogeo)

Mediana in secondi; righe/s; picco RSS del processo (include la fixture in
memoria: ~150 MiB a 1M, ~1,45 GiB a 10M).

| Benchmark | Scala | Tempo (s) | Righe/s | Picco RSS |
|---|---:|---:|---:|---:|
| filter | 1M | 0,0319 | 31,3 M/s | 149 MiB |
| filter | 10M | 0,3110 | 32,2 M/s | 1,42 GiB |
| sort (group,num) | 1M | 0,8725 | 1,15 M/s | 150 MiB |
| sort (group,num) | 10M | 14,0294 | 0,71 M/s | 1,42 GiB |
| aggregate (sum/mean per gruppo) | 1M | 0,1194 | 8,38 M/s | 150 MiB |
| aggregate | 10M | 1,2548 | 7,97 M/s | 1,42 GiB |
| join inner su id | 1M | 0,6361 | 1,57 M/s | 380 MiB |
| rename (2 colonne) | 1M / 10M | ~1 µs | zero-copy | = fixture |
| drop_columns (1 colonna) | 1M / 10M | ~0,5 µs | zero-copy | = fixture |
| reorder_columns | 1M / 10M | ~0,9 µs | zero-copy | = fixture |
| string_length | 1M | 0,0082 | 121 M/s | 184 MiB |
| text_normalize (full) | 1M | 0,3769 | 2,65 M/s | 207 MiB |
| text_normalize (full) | 10M | 4,5427 | 2,20 M/s | 1,84 GiB |
| concat_columns | 1M | 0,0497 | 20,1 M/s | 228 MiB |
| split_column | 1M | 0,0804 | 12,4 M/s | 235 MiB |
| pipeline 5 kernel (normalize→pad→concat→length→reorder) | 1M | 0,4722 | 2,12 M/s | 302 MiB |
| pipeline 5 kernel | 10M | 4,6857 | 2,13 M/s | 3,03 GiB |

Rename/drop/reorder confermano il riuso dei buffer Arrow (tempo ~costante,
indipendente dalle righe; `output_bytes == input_bytes` per rename/reorder).

## 2. Geografico — kernel in-process (geo, fixture box 1M)

Costo per nodo del motore oggi: decode WKB (con validazione contratto + OGC) →
kernel → encode WKB. Mediana di 3 ripetizioni.

| Benchmark | Tempo (s) | Geometrie/s | Picco RSS |
|---|---:|---:|---:|
| wkb_decode (solo decode+validazione) | 0,1985 | 5,04 M/s | 308 MiB |
| wkb_encode (solo encode) | 0,1091 | 9,17 M/s | 308 MiB |
| centroid (decode+kernel+encode) | 0,2831 | 3,53 M/s | 308 MiB |
| convex_hull | 0,6008 | 1,66 M/s | 308 MiB |
| envelope | 0,4675 | 2,14 M/s | 308 MiB |
| area (decode+kernel, output scalare) | 0,1904 | 5,25 M/s | 308 MiB |
| simplify (DP, tol 1.0) | 0,7331 | 1,36 M/s | 308 MiB |
| buffer (dist 10, round) | 9,5606 | 0,105 M/s | 308 MiB |
| intersects (coppie, geometrie decodificate) | 0,2507 | 3,99 M coppie/s | 308 MiB |
| catena buffer→simplify→centroid→area, WKB tra i nodi | 28,9915 | 34,5 k/s | 308 MiB |
| stessa catena fusa (1 decode, riferimento V6) | 15,7929 | 63,3 k/s | 308 MiB |

La catena con round-trip WKB costa 1,84× la catena fusa: è la misura del
margine del vincolo V6 (al massimo un decode/encode per segmento geo).

## 3. Geografico — trasporto Arrow v3 end-to-end (1M box, envelope 123 MB)

Processo completo (lettura envelope + SHA-256, decode IPC, transform, encode
IPC, scrittura envelope). `interno` = somma delle fasi da `profile_arrow`.

| Operazione | Wall (s) | Interno (s) | Geom/s (wall) | Picco RSS |
|---|---:|---:|---:|---:|
| centroid | 0,71 | 0,684 | 1,41 M/s | 449 MiB |
| convex_hull | 0,74 | 0,707 | 1,35 M/s | 732 MiB |
| envelope | 1,00 | 0,945 | 1,00 M/s | 733 MiB |
| area | 0,95 | 0,925 | 1,05 M/s | 354 MiB |
| buffer (via CLI, dist 10) | 3,50 | n/d | 0,29 M/s | 2,33 GiB |
| simplify (via CLI, tol 1.0) | 0,99 | n/d | 1,01 M/s | 628 MiB |
| catena buffer→simplify→centroid (3 processi CLI) | 5,11 | n/d | 0,20 M/s | 2,33 GiB |

La transform nel trasporto è parallela (rayon): per questo buffer via
trasporto (3,5 s) è più veloce del benchmark kernel sequenziale (9,6 s).
Il picco RSS di buffer è dominato dall'output (multipoligoni con cap round).

## 4. Spatial join in-process (R-tree vs esaustivo)

| Righe per lato | Coppie candidate | Match | Indicizzato (s) | Esaustivo (s) | Speedup |
|---:|---:|---:|---:|---:|---:|
| 5.000 | 25 M | 5.000 | 0,0070 | 0,1291 | 18,4× |
| 20.000 | 400 M | 20.000 | 0,0163 | 1,9660 | 120,8× |

## 5. Topologia estesa (build full-backends, GEOS 3.14 statico)

| Operazione | Carico | Kernel (s) | Wall (s) | Picco RSS |
|---|---:|---:|---:|---:|
| line_merge | 1.000.000 segmenti | 0,424 | 0,59 | 285 MiB |
| split_line | 100.000 segmenti × 1.000 punti | 1,742 | 1,76 | 33 MiB |
| polygonize | griglia 250×250, 62.500 facce | 0,502 | 0,52 | 190 MiB |
| split_polygon | 2.000 cutter, 2.001 parti | 0,070 | 0,07 | 19 MiB |

Coerente con le misure storiche di `BENCHMARKS.md` del progetto (stesse
fixture, stesso ordine di grandezza), a conferma della riproducibilità.

## 6. Lacune rispetto a Prestazioni.md §8 (da coprire in Fase 2)

Tabellare (§8.1):

- select e slice come benchmark dedicati (qui coperti solo via
  drop/reorder); hash join solo a 1M (niente 10M/100M); sort con spill,
  distinct, explode non misurati; scala 100M non eseguita.

Geografico (§8.2):

- fixture solo box semplici: mancano geometrie molto grandi, molti componenti
  e WKB eterogeneo (§G3);
- spatial join e intersects misurati solo in-process, non via trasporto
  pair-arrow (`sjoin`, `predicate`);
- la catena geo multi-kernel è misurata su kernel in-process e su 3 processi
  CLI; non esiste ancora un segmento fuso nel motore (V6, Fase 2C).

Memoria (§8.3): intera sezione non coperta — riguarda l'executor del workspace
(streaming, fan-out, governor, spill, cancellazione), non esiste nei progetti
di origine. Le misure RSS qui raccolte sono il riferimento per i kernel.

Metriche (§7): raccolte wall time, rows/s, geometries/s, picco RSS. Non
raccolte (non esposte dai progetti di origine): CPU time, bytes allocated,
allocation count, bytes copied, spill bytes, WKB decode/encode count, queue
high-water mark. Vanno strumentate nel nuovo executor.

## Riproduzione

1. Build: `docker run --rm -v <proj>:/work -w /work rust:1.92 cargo build
   --release --examples [--features arrow-transport|full-backends]`
   (geo con `CARGO_TARGET_DIR=target-docker`).
2. Tabellare: `examples/candidate_benchmark <rows> <rep> <op>` ed
   `examples/benchmark <rows> <rep> <op>`; projection/rename via
   `harness/nogeo_projection`.
3. Geo kernel: `harness/geo_kernels` (`geo-kernel-bench <bench> <rows> <rep>`).
4. Trasporto: fixture con `harness/make_geo_fixture.py`, poi
   `examples/profile_arrow --input F --operation OP` oppure
   `plenora-geo transform-arrow --input F --schema S.json --output O`.
5. Consolidamento: `python harness/consolidate_baseline.py` rigenera
   `baseline.json` dai file in `raw/`.
