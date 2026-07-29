# Sweep kernel geografici (bench_geo_sweep)

Fixture deterministiche (seed 42), mediana di 3 run, container Docker
`--cpus=4 --memory=10g`, release. Le op per-cella misurano la pipeline
decode WKB + kernel + encode WKB (come l'adapter Arrow); quelle
collettive includono il decode dell'intera tabella. `decode_share` e'
la quota del tempo per-cella attribuita a decode(+encode) WKB dai
riferimenti `_ref.*`; `bound_class` deriva da soglie 0.5/0.25.
Classifica ordinata per lentezza (geom/s crescenti, skipped in coda).
Il peak RSS e' il `VmHWM` cumulativo di processo (le fixture lazy
restano in memoria).

| # | op | tipo | fixture | celle | mediana (s) | geom/s | decode share | classe | peak RSS (MiB) | note |
|---|----|------|---------|-------|-------------|--------|--------------|--------|----------------|------|
| 1 | `geo.clean_topology` | collective | grid100 jitter | 1000 | 6.8275 | 146 | - | n/a | 3288 | snap_tolerance=1, remove_overlaps+fill_gaps; scala ridotta a 1000 (stima > 4s/rep a scala piena, esponente 1.5) |
| 2 | `geo.subdivide` | per_cell | poly_complex | 1000 | 6.3076 | 159 | 0.56 | decode-bound | 2925 | max_vertices=500 (helper WKB: decode+encode inclusi); scala ridotta a 1000 (stima > 4s/rep a 1M o fixture limitata) |
| 3 | `geo.snap` | per_cell | poly_simple | 1000 | 3.6156 | 277 | 0.00 | compute-bound | 2925 | tol=0.5, riferimento poligono 2000v (R-tree ricostruito per cella, come snap_column); scala ridotta a 1000 (stima > 4s/rep a 1M o fixture limitata) |
| 4 | `_ref.wkb_decode.poly_complex` | ref_decode | poly_complex | 1000 | 3.5106 | 285 | - | n/a | 1300 | solo decode WKB; scala ridotta a 1000 (stima > 4s/rep a 1M o fixture limitata) |
| 5 | `_ref.wkb_encode.poly_complex` | ref_encode | poly_complex | 1000 | 3.4694 | 288 | - | n/a | 2120 | decode + encode WKB (quota encode = misura - ref decode); scala ridotta a 1000 (stima > 4s/rep a 1M o fixture limitata) |
| 6 | `geo.clip` | collective | poly_simple x mask | 1000 | 0.6536 | 1530 | - | n/a | 3288 | poligoni 100v vs maschera (dissolve di 100 rettangoli); scala ridotta a 1000 (stima > 4s/rep a scala piena, esponente 1.5) |
| 7 | `geo.symmetric_difference` | collective | poly_simple pairs (overlap) | 1000 | 0.1510 | 6624 | - | n/a | 3288 | coppie di poligoni 100v sovrapposti; scala ridotta a 1000 (stima > 4s/rep a scala piena, esponente 1.5) |
| 8 | `geo.buffer` | per_cell | poly_simple | 10000 | 1.4914 | 6705 | 0.07 | compute-bound | 2120 | d=10, cap round; scala ridotta a 10000 (stima > 4s/rep a 1M o fixture limitata) |
| 9 | `geo.union` | collective | poly_simple pairs (overlap) | 10000 | 0.8643 | 11570 | - | n/a | 3288 | coppie di poligoni 100v sovrapposti; scala ridotta a 10000 (stima > 4s/rep a scala piena, esponente 1.5) |
| 10 | `geo.explode` | per_cell | multipoly | 10000 | 0.8431 | 11861 | 0.50 | mixed | 2120 | 1:N, encode di ogni parte; scala ridotta a 10000 (stima > 4s/rep a 1M o fixture limitata) |
| 11 | `geo.difference` | collective | poly_simple pairs (overlap) | 10000 | 0.7847 | 12744 | - | n/a | 3288 | coppie di poligoni 100v sovrapposti; scala ridotta a 10000 (stima > 4s/rep a scala piena, esponente 1.5) |
| 12 | `geo.intersection` | collective | poly_simple pairs (overlap) | 10000 | 0.7706 | 12977 | - | n/a | 3288 | coppie di poligoni 100v sovrapposti; scala ridotta a 10000 (stima > 4s/rep a scala piena, esponente 1.5) |
| 13 | `geo.concave_hull` | per_cell | poly_simple | 10000 | 0.7480 | 13368 | 0.14 | compute-bound | 2136 | concavity=2; scala ridotta a 10000 (stima > 4s/rep a 1M o fixture limitata) |
| 14 | `geo.voronoi` | collective | points | 10000 | 0.5696 | 17557 | - | n/a | 3288 | celle Voronoi da n punti, encode di ogni cella; scala ridotta a 10000 (stima > 4s/rep a scala piena, esponente 1.2) |
| 15 | `_ref.wkb_encode.multipoly` | ref_encode | multipoly | 10000 | 0.4182 | 23914 | - | n/a | 2120 | decode + encode WKB (quota encode = misura - ref decode); scala ridotta a 10000 (stima > 4s/rep a 1M o fixture limitata) |
| 16 | `_ref.wkb_decode.multipoly` | ref_decode | multipoly | 10000 | 0.4129 | 24220 | - | n/a | 1692 | solo decode WKB; scala ridotta a 10000 (stima > 4s/rep a 1M o fixture limitata) |
| 17 | `geo.simplify[preserve_topology]` | per_cell | poly_simple | 100000 | 3.1599 | 31647 | 0.33 | mixed | 2136 | tol=1, variante VW preserve; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 18 | `geo.snap_to_grid` | per_cell | poly_simple | 100000 | 3.1517 | 31729 | 0.33 | mixed | 2231 | grid=1; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 19 | `geo.scale` | per_cell | poly_simple | 100000 | 3.1034 | 32223 | 0.34 | mixed | 2136 | 1.5x0.75 su origine; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 20 | `geo.translate` | per_cell | poly_simple | 100000 | 3.1002 | 32256 | 0.34 | mixed | 2136 | dx=10 dy=20; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 21 | `geo.distance` | per_cell | poly_simple | 100000 | 3.0670 | 32605 | 0.33 | mixed | 2120 | vs poligono di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 22 | `geo.simplify` | per_cell | poly_simple | 100000 | 3.0651 | 32625 | 0.34 | mixed | 2136 | tol=1, douglas_peucker; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 23 | `geo.affine_transform` | per_cell | poly_simple | 100000 | 3.0406 | 32889 | 0.34 | mixed | 2136 | [1.1,0.05,3,-0.02,0.9,7]; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 24 | `geo.rotate` | per_cell | poly_simple | 100000 | 3.0228 | 33082 | 0.34 | mixed | 2136 | 30 gradi; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 25 | `geo.collect` | collective | disjoint polys groups of 10 | 10000 | 0.3023 | 33083 | - | n/a | 219 | raggruppamento senza unione, gruppi di 10 poligoni disgiunti (MultiPolygon valido); scala ridotta a 10000 (stima > 4s/rep a scala piena, esponente 1) |
| 26 | `geo.geodesic_area` | per_cell | geo_polys | 10000 | 0.2961 | 33774 | - | n/a | 2530 | ; scala ridotta a 10000 (stima > 4s/rep a 1M o fixture limitata) |
| 27 | `geo.point_on_surface` | per_cell | poly_simple | 100000 | 2.8823 | 34695 | 0.36 | mixed | 2136 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 28 | `geo.nearest` | collective | points x points | 10000 | 0.2585 | 38679 | - | n/a | 3288 | O(n*m) esatto: n punti vs 10k punti, pareggi multipli inclusi |
| 29 | `geo.to_wkt` | per_cell | poly_simple | 100000 | 2.3737 | 42128 | 0.43 | mixed | 2136 | output WKT (niente encode WKB); scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 30 | `geo.geometry_diagnostics` | per_cell | poly_simple | 100000 | 2.0553 | 48655 | 0.50 | mixed | 2530 | struct diagnostico; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 31 | `geo.boundary` | per_cell | poly_simple | 100000 | 2.0210 | 49481 | 0.52 | decode-bound | 2120 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 32 | `geo.area` | per_cell | poly_simple | 100000 | 2.0113 | 49720 | 0.51 | decode-bound | 2120 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 33 | `geo.bounds_extractor` | per_cell | poly_simple | 100000 | 1.9993 | 50016 | 0.51 | decode-bound | 2120 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 34 | `geo.vertex_count` | per_cell | poly_simple | 100000 | 1.9959 | 50103 | 0.51 | decode-bound | 2136 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 35 | `geo.perimeter` | per_cell | poly_simple | 100000 | 1.9896 | 50261 | 0.52 | decode-bound | 2136 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 36 | `geo.geometry_accessors` | per_cell | hetero | 100000 | 1.5893 | 62920 | 0.48 | mixed | 2925 | WKB eterogeneo; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 37 | `geo.from_wkt` | per_cell | wkt_polys | 100000 | 1.2830 | 77940 | - | n/a | 2925 | parse WKT + encode WKB (niente decode); scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 38 | `geo.convex_hull` | per_cell | poly_simple | 100000 | 1.1483 | 87086 | 0.91 | decode-bound | 2120 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 39 | `geo.envelope` | per_cell | poly_simple | 100000 | 1.0853 | 92139 | 0.96 | decode-bound | 2120 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 40 | `_ref.wkb_encode.poly_simple` | ref_encode | poly_simple | 100000 | 1.0414 | 96023 | - | n/a | 2120 | decode + encode WKB (quota encode = misura - ref decode); scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 41 | `_ref.wkb_decode.poly_simple` | ref_decode | poly_simple | 100000 | 1.0274 | 97335 | - | n/a | 674 | solo decode WKB; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 42 | `geo.centroid` | per_cell | poly_simple | 100000 | 1.0135 | 98671 | 1.03 | decode-bound | 2120 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 43 | `geo.predicate_contains` | per_cell | points | 100000 | 0.9642 | 103712 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 44 | `geo.predicate_covered_by` | per_cell | points | 100000 | 0.9623 | 103914 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 45 | `geo.predicate_intersects` | per_cell | points | 100000 | 0.9612 | 104032 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 46 | `geo.predicate_contains_properly` | per_cell | points | 100000 | 0.9582 | 104368 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 47 | `geo.predicate_touches` | per_cell | points | 100000 | 0.9461 | 105693 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 48 | `geo.predicate_within` | per_cell | points | 100000 | 0.9362 | 106812 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 49 | `geo.predicate_crosses` | per_cell | points | 100000 | 0.9351 | 106936 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 50 | `geo.predicate_covers` | per_cell | points | 100000 | 0.9311 | 107403 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 51 | `geo.predicate_overlaps` | per_cell | points | 100000 | 0.9289 | 107654 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 52 | `geo.predicate_disjoint` | per_cell | points | 100000 | 0.9127 | 109569 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 53 | `geo.predicate_equals_topo` | per_cell | points | 100000 | 0.9120 | 109648 | 0.00 | compute-bound | 2136 | punti vs poligono 100v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 54 | `geo.delaunay` | per_cell | multipoint50 | 100000 | 0.8333 | 120009 | - | n/a | 2430 | multipoint 50 punti, encode triangoli; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 55 | `_ref.wkb_decode.hetero` | ref_decode | hetero | 100000 | 0.7693 | 129988 | - | n/a | 2069 | solo decode WKB eterogeneo; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 56 | `geo.hausdorff_distance` | per_cell | lines50 | 100000 | 0.5865 | 170504 | 0.03 | compute-bound | 2136 | vs linea 50v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 57 | `geo.overlay` | collective | grid100 x grid100 shift | 10000 | 0.0547 | 182805 | - | n/a | 3288 | mode intersection, griglie sfasate di (50,50) |
| 58 | `geo.coverage_validate` | collective | grid100 jitter | 10000 | 0.0488 | 205016 | - | n/a | 338 | griglia 100x100 con overlap jitterati, tolerance=0 |
| 59 | `geo.geodesic_line_length` | per_cell | geo_lines | 100000 | 0.3795 | 263538 | - | n/a | 2231 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 60 | `geo.frechet_distance` | per_cell | lines50 | 100000 | 0.2944 | 339704 | 0.05 | compute-bound | 2431 | vs linea 50v di riferimento; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 61 | `geo.dissolve` | collective | grid100 jitter | 10000 | 0.0223 | 448017 | - | n/a | 3288 | unary union di rettangoli con overlap jitterati |
| 62 | `geo.cluster_dbscan` | collective | points dense [0,1000)^2 | 100000 | 0.1251 | 799590 | - | n/a | 374 | eps=10, min_points=5; scala ridotta a 100000 (stima > 4s/rep a scala piena, esponente 1.3) |
| 63 | `geo.shared_paths` | collective | grid100 | 10000 | 0.0118 | 851034 | - | n/a | 338 | griglia 100x100 esatta (bordi condivisi), min_length=1 |
| 64 | `geo.densify` | per_cell | lines50 | 100000 | 0.0997 | 1002513 | 0.23 | compute-bound | 2231 | max_segment_length=5; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 65 | `geo.polygon_builder` | collective | circle points groups of 1000 | 1000000 | 0.9079 | 1101415 | - | n/a | 338 | gruppi ordinati di 1000 punti -> poligono (anelli validi) |
| 66 | `geo.line_substring` | per_cell | lines50 | 100000 | 0.0765 | 1307120 | 0.30 | mixed | 2431 | [0.2, 0.8]; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 67 | `geo.line_locate_point` | per_cell | lines50 | 100000 | 0.0446 | 2240065 | 0.34 | mixed | 2925 | vs punto (5000,5000); scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 68 | `geo.line_interpolate_point` | per_cell | lines50 | 100000 | 0.0329 | 3040759 | 0.69 | decode-bound | 2431 | ratio=0.5; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 69 | `geo.length` | per_cell | lines50 | 100000 | 0.0241 | 4145645 | 0.63 | decode-bound | 2136 | ; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 70 | `_ref.wkb_encode.lines50` | ref_encode | lines50 | 100000 | 0.0226 | 4425951 | - | n/a | 2120 | decode + encode WKB (quota encode = misura - ref decode); scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 71 | `geo.generate_grid` | generative | extent 1000x1000 | 1000000 | 0.1957 | 5108943 | - | n/a | 3288 | cell_size=1, shape square, encode WKB incluso |
| 72 | `geo.line_merge` | collective | chain lines 10x10k | 100000 | 0.0190 | 5258091 | - | n/a | 3288 | un MultiLineString di n segmenti in catene da 10 |
| 73 | `geo.bearing` | per_cell | geo_points | 1000000 | 0.1827 | 5473385 | 0.10 | compute-bound | 2435 | vs punto di riferimento |
| 74 | `_ref.wkb_decode.lines50` | ref_decode | lines50 | 100000 | 0.0151 | 6617306 | - | n/a | 276 | solo decode WKB; scala ridotta a 100000 (stima > 4s/rep a 1M o fixture limitata) |
| 75 | `geo.geodesic_distance` | per_cell | geo_points | 1000000 | 0.1475 | 6778264 | 0.12 | compute-bound | 2136 | vs punto di riferimento |
| 76 | `geo.sjoin` | collective | points x grid100 | 1000000 | 0.1217 | 8218394 | - | n/a | 3288 | punti vs griglia 10k rettangoli, predicato intersects (R-tree) |
| 77 | `geo.count_points_in_polygons` | collective | points x grid100 | 1000000 | 0.1204 | 8306314 | - | n/a | 3288 | punti vs griglia 10k rettangoli |
| 78 | `geo.within` | collective | points x grid100 | 1000000 | 0.1158 | 8637286 | - | n/a | 3288 | punti vs griglia 10k rettangoli (within) |
| 79 | `geo.line_builder` | collective | circle points groups of 1000 | 1000000 | 0.0382 | 26163865 | - | n/a | 338 | gruppi ordinati di 1000 punti -> linea |
| 80 | `geo.haversine_distance` | per_cell | geo_points | 1000000 | 0.0291 | 34321435 | 0.62 | decode-bound | 2136 | vs punto di riferimento |
| 81 | `_ref.wkb_encode.points` | ref_encode | points | 1000000 | 0.0277 | 36149629 | - | n/a | 2120 | decode + encode WKB (quota encode = misura - ref decode) |
| 82 | `_ref.wkb_decode.geo_points` | ref_decode | geo_points | 1000000 | 0.0180 | 55552985 | - | n/a | 2120 | solo decode WKB |
| 83 | `_ref.wkb_decode.points` | ref_decode | points | 1000000 | 0.0169 | 59271159 | - | n/a | 79 | solo decode WKB |
| 84 | `geo.from_coords` | per_cell | coords_xy | 1000000 | 0.0159 | 62868014 | - | n/a | 2136 | da colonne x/y (niente decode) |
| 85 | `geo.make_valid` | skipped | - | 0 | 0.0000 | 0 | - | n/a | 374 | backend `geos` non abilitato (BackendPending) |
| 86 | `geo.reproject` | skipped | - | 0 | 0.0000 | 0 | - | n/a | 374 | backend `proj` non abilitato (BackendPending) |
| 87 | `geo.polygonize` | skipped | - | 0 | 0.0000 | 0 | - | n/a | 374 | backend `geos` non abilitato (BackendPending) |
| 88 | `geo.split` | skipped | - | 0 | 0.0000 | 0 | - | n/a | 374 | backend `geos` non abilitato (BackendPending) |

## Commento

**Metodo.** Pipeline per-cella = decode WKB + kernel + encode WKB (come
l'adapter Arrow del trasporto); `decode_share` = quota di decode(+encode di
una geometria della stessa fixture — approssimazione) sul tempo per-cella.
Scala adattiva (target 4 s/run, mediana di 3): le note indicano le riduzioni.
Run eseguita in Docker `--cpus=4 --memory=10g` (rust:1.92, release, rayon su
4 thread). Dati grezzi: `geo_sweep.jsonl` (streaming per riga), merge in
`geo_sweep.json` via `_merge_geo_sweep.py`.

**Avvertenza ambiente.** Il kernel WSL2 6.18 ha uno stallo in
`brk`/`__vma_start_write` che congela il processo in stato D (race su
carico di allocazioni intensive); workaround usato nelle run:
`MALLOC_ARENA_MAX=4 MALLOC_MMAP_THRESHOLD_=32768`
(`MALLOC_TOP_PAD_=268435456` non si e' dimostrato sufficiente). I container
andati in stallo restano non killabili fino al restart di Docker Desktop.
Aggiornamento 2026-07-28: su WSL2 piu' recente lo stallo si ripresenta
ANCHE con il workaround e a 2 CPU (3 run su 4 congelate a punti diversi —
race, non scenario specifico); una build musl e' risultata impraticabile
per tempi di link. Per la verifica mirata dei percorsi geo in questa
condizione usare `examples/bench_geo_perfcheck.rs` (fixture compatte a
bassa pressione di allocazione, stessa classe di carico).

**L'adapter WKB e' il collo di bottiglia sistemico.** Riferimenti `_ref.*`:
punti ~59M geom/s, linee 50v ~6.6M geom/s (~330M vert/s), poligoni 100v
~97k geom/s (~10M vert/s), poligoni 2000v ~285 geom/s (~0.6M vert/s),
multipoligoni ~24k/s, eterogeneo ~130k/s. Per vertice, il decode dei
poligoni e' ~30x piu' lento delle linee (percorso geozero per gli anelli):
qualsiasi op su poligoni realistici paga il decode piu' del kernel. Tutte le
op con `decode_share >= 0.5` (centroid 1.03, envelope 0.96, convex_hull
0.91, area/boundary/bounds/vertex_count/perimeter ~0.51, haversine 0.62,
length 0.63, line_interpolate_point 0.69, explode 0.50, subdivide 0.56) sono
gia' coperte dalla fusione di segmento Fase 2C (cache di decode): NON vale
lavoro diretto sul kernel. Idem, parzialmente, per le op "mixed"
(0.25-0.5): simplify (entrambe le policy ~32k/s), distance, point_on_surface,
affine/translate/scale/rotate, snap_to_grid, to_wkt, densify, line_substring,
line_locate_point, geometry_diagnostics, geometry_accessors.

**Candidati compute-bound (la fusione 2C NON li copre), in priorita':**

1. `geo.snap` — 277 geom/s, share 0.003. Causa trovata: `snap_validated`
   ricostruisce l'R-tree dei vertici di riferimento a OGNI cella
   (`snap_column` lo richiama per riga). Fix banale (albero costruito una
   volta per riferimento/segmento): il costo residuo e' map_coords + lookup
   per vertice, atteso >100x (verso il regime decode-bound ~50k+ geom/s).
   ROI piu' alto di tutto lo sweep.
2. `geo.clean_topology` — 146 geom/s a 1k celle (6.8 s/run): l'op piu' lenta
   in assoluto, di 40x sul successivo. Collettiva (morfologia gap/overlap su
   griglia jitterata): profilare la politica first-row-wins, sospetta
   complessita' superlineare sugli overlap.
3. `geo.buffer` — 6.7k geom/s, share 0.07. Costo nel buffer della crate
   `geo` (offset poligonale); op calda nelle pipeline reali, migliorarla
   richiede lavoro algoritmico diretto (o valutare il backend GEOS).
4. Predicati DE-9IM (tutti e 11 a ~104-110k/s, share ~0.002): velocita'
   identiche => il costo e' `relate()` a matrice completa, senza
   short-circuit nemmeno per intersects/disjoint. Fast path (bbox precheck,
   early-exit, prepared geometry sul riferimento da config) darebbe un
   guadagno uniforme su tutta la famiglia, stimabile 3-10x.
5. Booleane pairwise + clip: intersection 13.0k, difference 12.7k, union
   11.6k, symmetric_difference 6.6k coppie/s su poligoni 100v (i_overlay);
   `geo.clip` misurato 1.5k/s ma vedi caveat sotto. `geo.overlay` (183k/s su
   rettangoli) e `geo.dissolve` (448k/s) sono invece a posto.
6. `geo.concave_hull` 13.4k/s (share 0.14) e `geo.voronoi` 17.6k/s (10k
   punti): compute-bound, ottimizzabili ma op meno calde.
7. `geo.nearest` — 38.7k/s a 10k x 10k: O(n*m) esatto per contratto
   (restituisce tutti i pareggi). Un indice R-tree come filtro candidati
   resterebbe esatto ma va gestita la semantica dei ties.

**Lasciare stare:** from_coords 63M/s, haversine 34M/s (gia' decode-bound),
line_builder 26M/s, sjoin/within/count_points_in_polygons 8.2-8.6M/s
(R-tree gia' efficace), line_merge 5.3M/s, generate_grid 5.1M/s,
geodesic_distance/bearing 5.5-6.8M/s, length 4.1M/s, line_interpolate_point
3.0M/s, line_locate_point 2.2M/s, densify 1.0M/s, polygon_builder 1.1M/s,
shared_paths 851k/s, cluster_dbscan 800k/s, dissolve 448k/s, frechet 340k/s,
geodesic_line_length 264k/s, overlay 183k/s, hausdorff 170k/s, delaunay
120k/s, from_wkt 78k/s (parse WKT), geodesic_area 34k/s, collect 33k/s,
geometry_accessors 63k/s (mista). Costi gia' vicini al regime dell'adapter o
assolutamente bassi per i casi d'uso tipici.

**Caveat di misura.**
- `geo.clip`: nella run misurata la maschera (primi 100 rettangoli) era
  disgiunta dai poligoni => intersezioni vuote con early-out bbox; il valore
  1.5k/s e' un UPPER bound. L'example e' stato corretto (maschera = striscia
  centrale della griglia); rieseguire per il valore realistico, atteso
  peggiore delle booleane pairwise.
- `peak_rss_kib` e' il VmHWM cumulativo di processo (le fixture restano in
  memoria): utile come tetto, non come RSS per-op.
- Op `BackendPending` (make_valid, reproject, polygonize, split): non
  misurate, richiedono le feature `geos`/`proj` (build
  `--features full-backends`).
- Le scale ridotte (nota "scala ridotta a ...") riflettono il target 4 s/run
  a 4 CPU: geom/s rimane confrontabile, ma le op piu' lente sono misurate su
  campioni piccoli (1k-10k celle).
