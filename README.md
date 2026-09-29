# plenora-data-tools2

Kernel tabellari e geografici su Arrow `RecordBatch`, in Rust puro: tabelle
in ingresso, una trasformazione, tabelle in uscita.

Deriva da `plenora-data-tools` al commit `190c493` (fase 1 di un successore
semplificato). I nomi dei crate sono rimasti quelli, così le correzioni del
progetto d'origine si portano qui senza rinomine.

## Che cosa c'è

| crate | contenuto |
| --- | --- |
| `plenora-core` | re-export Arrow, `PlenoraError`, limiti, catalogo delle operazioni, contratti dati, contratto CRS fail-closed, politica dei panici |
| `plenora-kernels-table` | kernel tabellari (filtri, ordinamenti, aggregazioni, join, espressioni, date, stringhe, qualità, spill) |
| `plenora-kernels-geo` | kernel geografici su `geo::Geometry` e adapter GeoArrow-WKB |
| `vendor/` | `geo`, `wkt`, `i_shape` con le patch di `patches/` (provenienza in `vendor/*/PROVENANCE*.md`) |

## Che cosa non c'è ancora

- **Runner di pipeline**: nessun motore che concateni le trasformazioni; si
  chiamano i kernel direttamente.
- **`geo.reproject`**: richiedeva PROJ, è fuori dal catalogo.
- **`geo.make_valid`, `geo.polygonize`, `geo.split`**: richiedevano GEOS,
  sono fuori dal catalogo (le versioni in Rust puro arriveranno in una fase
  successiva).
- **Risoluzione CRS**: senza PROJ `resolve_crs` fallisce sempre chiuso; un
  CRS entra solo già risolto dal chiamante.

Engine, CLI, isolamento e protocollo del progetto d'origine non sono stati
portati. I riferimenti a `docs/…` nei commenti rimandano alla documentazione
di `plenora-data-tools`.

## Limiti dichiarati

### Validazione OGC: la ricerca delle auto-intersezioni non è quella di `geo`, il verdetto sì

**Regola.** Per `Polygon`, `MultiPolygon`, `GeometryCollection` e `Geometry`
la barriera `ValidazioneProtetta` non chiama `check_validation` di `geo`:
esegue `validazione_ogc::ValidazioneOgc::valida_ogc_rapida`, che rifà con le
API pubbliche di `geo` la stessa sequenza di `visit_validation` di `geo`
0.33.1 (stessi controlli, ordine, errori, stessa `relate`) e cambia **solo**
come si trovano le coppie di segmenti candidate: una scansione sui rettangoli
d'ingombro al posto del doppio ciclo O(n²). Il predicato per coppia è quello
di `geo`; gli anelli con coordinate non finite passano dal doppio ciclo.
L'oracolo è in `crates/plenora-kernels-geo/src/validazione_ogc/tests.rs`.

**Ambito.** `plenora-kernels-geo`, ogni validazione OGC che passa da
`ValidazioneProtetta`.

**Hazard.**

- il caso peggiore resta O(n²): con molti segmenti lunghi a rettangoli
  sovrapposti le coppie candidate sono quadratiche come nel doppio ciclo (il
  verdetto non cambia, il tempo sì);
- la sequenza è copiata da `geo` 0.33.1: a ogni aggiornamento di `geo` va
  riallineata a mano, e l'oracolo rileva una divergenza solo sulle forme che
  esercita;
- la correttezza dello scarto delle coppie dipende dal segno esatto di
  `orient2d` nel kernel di `geo`: con un kernel non esatto il filtro potrebbe
  scartare una coppia che il doppio ciclo dichiara intersecante;
- i controlli `relate` fra anelli e fra i poligoni di un `MultiPolygon` restano
  quadratici nel numero di anelli e di poligoni, come in `geo`.

**Condizione di rientro.** Una versione di `geo` con una ricerca delle
auto-intersezioni sub-quadratica a verdetto identico: la sequenza copiata si
toglie, la barriera torna a `check_validation` e l'oracolo resta come
regressione.

### Hash delle chiavi non keyed

**Regola.** Le mappe di chiavi dei kernel tabellari usano due hash
deterministici senza seme (`crates/plenora-kernels-table/src/hashing.rs`):
`KeyHasher` per i valori nativi (interi, testi, valori di join) e
`ChiaveHasher` per le chiavi binarie di riga (arena `KeyInterner` di
aggregate, distinct, set operation, assert_unique e table_diff; mappe e
scelta della partizione dello spill). L'uguaglianza delle chiavi si decide
sempre sui valori o sui byte: l'hash sceglie i candidati, mai il risultato.

**Ambito.** `plenora-kernels-table`: raggruppamenti, join, set operation,
qualità, spill.

**Hazard.**

- nessuno dei due è keyed: dati costruiti apposta per collidere degradano
  build e probe fino al quadratico entro i limiti di riga, che limitano `n`
  ma non il comportamento dentro `n`. Nessuna perdita di correttezza;
- `KeyHasher` ha un passo per blocco che propaga le differenze solo verso i
  bit alti: su chiavi di più blocchi in cui i byte che variano stanno in
  cima a un blocco e nel blocco di coda collide anche senza avversario (un
  milione di chiavi compatte Int64 davano 32 768 hash). Per questo le chiavi
  binarie di riga e lo spill usano `ChiaveHasher`; le mappe di valori nativi
  su più blocchi (testi, chiavi composte dei join) usano ancora `KeyHasher`,
  e lì il rischio residuo è di tempo.

**Condizione di rientro.** Un hasher con chiave per processo, verificato su
tutti gli usi (nessun output deve dipendere dall'ordine di una mappa), e
`KeyHasher` corretto o sostituito sulle chiavi di più blocchi.

### Memoria delle chiavi dei kernel in memoria non governata

**Regola.** `aggregate`, `distinct`/`dedup_advanced`, le set operation,
`assert_unique` e `table_diff` in memoria non contabilizzano le proprie
strutture di chiavi (arena, indici, gruppi) su `max_governed_memory_bytes`:
il budget decide solo il passaggio allo spill, sulla stima dei byte
dell'input. Nelle varianti spilled la contabilità dipende dall'operatore:

- set operation: le chiavi distinte di **ciascuna partizione** (lunghezza
  della chiave più 64 byte per chiave), quindi più partizioni riducono il
  picco;
- `distinct`: la mappa delle statistiche è **globale**, una
  voce per chiave distinta di tutto l'input (lunghezza più 64 byte), e più
  partizioni non la riducono;
- `aggregate`: i batch Arrow letti di una partizione, **non** le strutture
  di chiavi e gruppi costruite su di essi.

**Ambito.** I kernel elencati, percorso in memoria; nelle varianti spilled,
le strutture di chiavi e gruppi di `aggregate`.

**Hazard.** Con molte chiavi distinte il picco reale supera la stima
dell'input: l'arena delle chiavi, due `usize` e una voce di mappa per chiave
distinta, fino a due `usize` per riga per l'assegnazione ai gruppi.

**Condizione di rientro.** Contabilità esplicita delle strutture di chiavi,
con errore `ResourceLimit` oltre il budget.

## Costruire e provare

Serve solo `rustup`: la toolchain (1.98.0) è fissata in
`rust-toolchain.toml`. Niente dipendenze native.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

I gate completi prima di un commit sono in [`AGENTS.md`](AGENTS.md).
