# ADR 14 — Binari geo nel plan executor: segmenti BinaryBlocking geo, validazione OGC totale, ordine canonico

- **Stato**: accettato (design ratificato dall'owner 2026-07-30 con le
  risposte ai 5 punti aperti; ratifica formale di D14.3/D14.4 in arrivo
  separatamente — il revisore le raccomanda entrambe)
- **Decisioni collegate**: ADR 1 (determinismo/R12), ADR 2 (resource
  accounting), ADR 3 (failure/cancellazione), ADR 5 (ValidatedGraph vs
  ExecutionPlan), ADR 6 (semantica limiti), ADR-0009 (contratto nello
  schema), ADR-0011 (decoder validante), ADR-0012 (fusione segmenti geo —
  pattern attribuzione errori e oracolo esteso), ADR-0013 (varianti
  `*_validated`: precondizione che il decode totale rende disponibile)
- **Riferimenti**: `plenora-contracts` R0.1, R2.4, R12, H-03;
  `docs/deroghe.md` (modello DER-003); `Prestazioni.md` V6/V7

## Contesto

I dieci op geo `BinaryOrdered`/`BinaryBlocking` di catalogo — `geo.sjoin`,
`geo.nearest`, `geo.within`, `geo.count_points_in_polygons`, `geo.clip`,
`geo.overlay`, `geo.intersection`, `geo.union`, `geo.difference`,
`geo.symmetric_difference` — sono oggi eseguibili **solo** come comando
CLI standalone `pair-arrow` (un'operazione per processo su envelope v3).
Nel plan executor cadono nel rifiuto `Unsupported … Fase 2B/2C` di
`prepare_geo`: la validazione del piano passa (i contratti analyze
esistono per tutti e dieci, `analyze_binary` in
`plenora-kernels-geo/src/analyze/dispatch.rs`), e' il prepare fisico a
mancare.

Fatti di scoping (base del design):

1. **I contratti analyze v4 ci sono gia' per tutti e dieci gli op** —
   nessun lavoro analyze nel primo cantiere. Attenzione: il contratto v4
   di sjoin NON e' quello legacy v3 (solo indici): e' left passthrough +
   `right_index` (inner join via `take`); nearest aggiunge `distance`
   nullable.
2. **`geo.distance` e i `geo.predicate_*` NON sono binari nel modello
   v4**: sono unari con secondo operando scalare da config (`other_wkb`)
   — cantiere unario separato (decisione owner 1). In questo ADR
   «binari geo» = i dieci `BinaryBlocking`.
3. Infrastruttura executor pronta e provata su `table.*`: drenaggio dei
   due rami via EdgeStream, `concat_batches`, tetti V7, reservation
   governor in ordine globale fisso left→right, `catch_unwind`,
   `check_join_expansion` (ADR 6), `blocking_output_sequence`.
4. Cancellazione: tutti e dieci gli op sono `BoundaryOnly` in catalogo.
5. **Rilievo owner 1 (vincolante per il cantiere unario)**: gli operandi
   `other_wkb` non portano contratto — niente `crs_id`, `axis_order`,
   `dimensions`. Confrontare geometrie in due sistemi di riferimento
   diversi senza accorgersene e' la classe di guasto H-01. Verificato
   2026-07-30: `OtherWkbConfig` e' solo una stringa hex,
   `validate_other_wkb` controlla la struttura, nessun meccanismo dichiara
   il CRS dell'operando. Registrato come debito di design del cantiere
   unario (fuori da questo ADR).
6. RSS collettivi ~3,3 GiB nello sweep: un lato decodificato puo'
   superare `max_memory_bytes` — serve contabilita' preflight (D14.4).

## Decisione

### D14.1 — Perimetro primo cantiere: sjoin, nearest, within, count_points_in_polygons

Quattro op, criterio confermato dall'owner: **nessuna ri-encode** — i due
reperti della suite trasformazioni (types, reproject) erano entrambi su
operazioni che riscrivono geometrie. L'output e' indici + `take` sulle
colonne left (sjoin/nearest) o left passthrough + colonna scalare
(within/count): niente `encode_geometry`, niente gate `MAX_CELL_BYTES` in
uscita, niente CRS rewrite. `clip`, `overlay`, le quattro booleane
pairwise al **secondo cantiere** (ri-encodano WKB). `geo.split` e'
unario v4, `distance`/`predicates` al cantiere unario separato.

### D14.2 — Architettura: `PreparedConfig::GeoBinary`, kernel riusati, CLI invariata

- **Prepare**: braccio in `prepare_geo` che per i quattro id costruisce
  `PreparedConfig::GeoBinary(Box<GeoBinaryPlan>)` (operazione, parametri
  tipizzati rivalidati in prepare — stessa tabella parametri-per-op del
  v3 estratta in forma pura, indice colonna geometria, CRS di output).
- **Executor**: `run_binary_blocking` smista sul `PreparedConfig`; il
  ramo table e' invariato riga per riga; il ramo geo riusa il guscio
  (drenaggio, concat, tetti V7, ordine reservation, `catch_unwind`,
  `check_join_expansion`, `blocking_output_sequence`, metriche) e
  sostituisce il cuore: decode dei due lati → kernel → costruzione
  output (`take` / append colonna).
- **Decodifica**: il cuore di `decode_geometry_side` (schema +
  `&[RecordBatch]` → `Vec<Option<Geometry<f64>>>` con gate
  `MAX_CELL_BYTES`/`MAX_ROWS`) fattorizzato come funzione condivisa
  (`decode_geometry_batches`) usata da `pair_arrow` e dall'executor: una
  sola camminata validante (ADR-0011), una sola fonte di verita' sui
  gate.
- **Kernel**: le varianti `*_validated` di `plenora-kernels-geo`
  (ADR-0013) — precondizione soddisfatta per costruzione dal decode
  totale (D14.3). **Dipendenza dichiarata (nota di metodo dell'owner):
  se ADR-0013 cambia forma, D14.3 va riletta.**
- **La CLI `pair-arrow` resta**: stessa libreria, due ingressi; nessuna
  modifica comportamentale al v3.

### D14.3 — Validazione OGC TOTALE, mai lazy (ratificata dal revisore; owner in arrivo)

Decode totale di entrambi i lati, sempre, senza opzione lazy nel piano.

1. **Il lazy e' non-determinismo di contratto (R12, argomento
   dell'owner)**: con prefilter R-tree, un'auto-intersezione in una cella
   mai candidata passa o fallisce a seconda del contenuto dell'altro
   lato — lo stesso dataset invalido accettato o rifiutato in funzione di
   dati che non contiene. Non e' solo failure silenziosa di classe
   (regola 1): la validazione dipenderebbe da dati esterni al dataset,
   e R12 chiede che il determinismo dichiarato sia rispettato.
2. **La deroga non e' simmetrica a DER-003** (criterio dell'owner per
   ogni deroga futura): DER-003 *sposta* una protezione con copertura
   equivalente dimostrata da test; il lazy OGC *toglie* copertura senza
   sostituto — l'hazard (geometria invalida consumata a valle) resta
   scoperto.
3. L'opt-in per piano sposta l'hazard sull'autore del piano, che non ha
   gli strumenti per valutarlo.
4. **Unica condizione di rientro**: un benchmark futuro che dimostri il
   decode dominante sul kernel su un profilo reale → deroga registrata
   con gate strutturale totale + hazard + test alla DER-003.

Conseguenza: la precondizione delle varianti `*_validated` (ADR-0013)
vale per costruzione su entrambi i lati.

### D14.4 — Preflight con conteggio ESATTO della forma decodificata (ratificata dal revisore; owner in arrivo)

R7 nell'ordine giusto (parole del revisore): **riservare prima di
decodificare, rifiutare prima di allocare.** Conteggio esatto e
conservativo, funzione pura della struttura (stesso principio di
`wkb_size_xy`, D12.3): nuovo helper `decoded_size_xy`
(`plenora-kernels-geo`) che percorre la cella con la camminata del
decoder e accumula `16·n_coord + 24·n_vec + 8·n_enum + 24` per slot
`Option` (costanti `size_of` verificate da un test di conservativita' su
corpus — stima ≥ memoria reale, stesso ruolo del test DER-003).

Sequenza nel ramo geo di `run_binary_blocking`:

1. concat left/right + `check_batch_bytes` (esistente);
2. preflight left → reservation governor → decode left;
3. preflight right → reservation → decode right;
4. rilascio lease Arrow right (left resta: passthrough/`take`);
5. `check_cancellation` post-drenaggio → kernel sotto `catch_unwind`;
6. lease output, rilascio lease decodificati, `check_join_expansion`,
   `blocking_output_sequence`.

Rifiuto fail-closed prima dell'allocazione: nessun partial state.
Opzione (b) spill: cantiere separato (dopo M3 spill unario). Opzione
(c) join batch-by-batch: **respinta** — l'R-tree del lato build va
comunque materializzato intero.

### D14.5 — Semantica errori

1. Ogni errore e' `Execution { node, operation, execution_id }` via
   `step_error` (come D12.6): «quale nodo ha rotto» resta rispondibile.
2. Nessun transito da `ArrowTransportError` nel messaggio (precedente
   M2): carrier dedicato (`GeoBinaryStepError`) con sorgente grezza +
   **side** + **indice di riga** come campi strutturati — contesto
   strutturale ammesso nei diagnostics, **mai nel testo del messaggio**
   (regola 8: la posizione va nel campo, non nella frase).
3. Primo errore in ordine (side, riga): decode left completo prima di
   right (l'ordine globale del governor lo impone); errori kernel con
   collect indicizzato rayon, mai selezione temporale (ADR-0001).
4. Fasi: drenaggio/decode input → `Read`; kernel/costruzione output →
   `Write`.
5. Cancellazione `BoundaryOnly` (catalogo): confini di batch in
   drenaggio + post-drenaggio pre-kernel, nessun check dentro il kernel —
   comportamento voluto, non lacuna.
6. `catch_unwind` sul kernel: panic → `panic_step_error`, mai publish
   dopo panic; hook `PANIC_AT_NODES` esteso al ramo.

### D14.6 — Limiti di espansione: ADR 6 + tetto assoluto (decisione owner 2)

Verificato 2026-07-30: `ExpansionConstraint` esprime SOLO vincoli
relativi (`SumRelative`, `LeftRelative`, `Custom(factor)`, `MaxRelative`)
— nessun tetto assoluto. La condizione dell'owner scatta quindi:
**sjoin e' O(n×m) nel caso peggiore e un vincolo relativo su un prodotto
cartesiano non e' un vincolo** — serve un tetto assoluto per-op, «non
come seconda politica, ma perche' la prima non copre il caso». Forma da
fissare in M1: tetto assoluto dichiarato in catalogo per op binaria
(una sola fonte per op, coerente col principio «due limiti sono due
posti dove sbagliare»), senza reintrodurre `max_pairs` da config nodo
stile v3.

### D14.7 — Ordine canonico delle coppie (decisione owner 3, NON facoltativa)

Se il kernel legacy non garantisce un ordine canonico (left-major,
right-minor), il porting lo fissa per **ordinamento esplicito in
entrambi i percorsi, v3 incluso** — dichiarato qui come cambiamento di
comportamento del v3. Motivazione owner: senza ordine canonico l'ordine
delle coppie dipende dall'iterazione interna (R12); e lo stesso piano
deve dare righe nello stesso ordine su entrambe le versioni, altrimenti
chi confronta due esecuzioni non sa se e' cambiato il dato o il motore.

### D14.8 — Lineage R2.4 (gia' decisa dai contratti analyze)

sjoin/nearest = left passthrough (chiavi canoniche ereditate sulle
colonne left) + colonne indice/distanza derivate senza metadati ereditati;
proprieta' di contratto azzerate (righe moltiplicate); metadati di
schema = `merge_schema_metadata` (conflitto → errore in validazione, mai
a runtime). within/count = left invariato + colonna derivata. Indici
posizionali, nessuna chiave su di essi.

### D14.9 — Oracolo esteso agli errori (gate)

`crates/plenora-engine/tests/geo_binary_oracle.rs`: doppia esecuzione
plan executor vs CLI `pair-arrow` — stesso output byte-per-byte (per
sjoin: coppie e attributi codificati nell'oracolo, con l'ordine canonico
di D14.7) oppure stesso errore su variante/categoria, nodo, fase, side,
riga. Casi obbligatori: (a) percorso felice multi-tipo per i 4 op; (b)
geometria OGC-invalida su left e su right — inclusa una cella invalida
MAI candidata al prefilter (dimostra che non esiste lazy, D14.3); (c)
cella oltre `MAX_CELL_BYTES` su ciascun lato; (d) cancellazione in
drenaggio e post-drenaggio; (e) panic iniettato via hook; (f) espansione
oltre il vincolo; (g) **governor che rifiuta la reservation decodificata
— condizione di attivazione del perimetro** (stesso ruolo del test
DER-003); (h) conservativita' di `decoded_size_xy` su corpus.

### D14.10 — Benchmark

`crates/plenora-engine/examples/bench_geo_binary.rs`: CLI standalone vs
piano v4 (sjoin e within, fixture misti), mediana di 5 run alternate con
mitigazione allocatore documentata; controllo di non regressione dei
`table.*` binari sul guscio condiviso. Accettazione: bande non
sovrapposte o delta entro rumore documentato, output byte-identici.

## Perimetro esplicito — cosa NON fare

- Niente decode lazy (D14.3; rientro solo via deroga registrata).
- Niente spill della forma decodificata; niente join batch-by-batch.
- Niente clip/overlay/booleane (secondo cantiere); niente split, niente
  distance/predicates (cantiere unario, con il debito `other_wkb` senza
  contratto da risolvere PRIMA — punto owner 1).
- Nessuna modifica di catalogo → nessun cambio di snapshot/fingerprint.
  Il vincolo di espansione mancante di `geo.union` e' **debito separato**
  (decisione owner 4: mai mescolare due change nella stessa CIA).
- Nessuna modifica comportamentale al trasporto v3 tranne l'ordinamento
  canonico di D14.7 (dichiarato).
- Nessuna fusione ADR-0012 estesa ai binari; niente kill switch nuovo.

## Milestone

- **M1 — Dispatch prepare + output (4 op, percorso felice)**:
  `GeoBinaryPlan`, smistamento in `run_binary_blocking`,
  `decode_geometry_batches`, kernel wiring, output via `take`/append;
  tetto assoluto D14.6; test prepare + percorso felice per-op, identita'
  contratto vs analyze, `node_rows` esatte.
- **M2 — Errori, governor, oracolo (il cuore)**:
  `GeoBinaryStepError` (side+riga strutturati), fasi, `decoded_size_xy`
  + preflight, primo errore in ordine, `catch_unwind`, oracolo D14.9
  completo (condizione governor inclusa), ordinamento canonico D14.7 su
  entrambi i percorsi. Review da secondo lettore obbligatoria (regola 2).
- **M3 — Benchmark A/B e chiusura**: `bench_geo_binary.rs`, scenari
  CLI-vs-piano e table-non-regressione, stato di attuazione,
  `Prestazioni.md`.

## Cambi di comportamento (dichiarati)

- **v3 sjoin: ordine delle coppie fissato canonicamente** (D14.7, owner:
  non facoltativo) — stesso insieme, stesso ordine su v3 e v4.
- Gli op del perimetro smettono di rispondere `Unsupported` nel plan
  executor: un piano che li usa oggi fallisce in validate, domani
  esegue. Cambiamento voluto, e' il punto dell'ADR.

## Stato di attuazione

- 2026-07-30: design ratificato dall'owner con le risposte ai 5 punti
  aperti (perimetro 4 op + cantiere unario separato con il debito
  `other_wkb`; tetto assoluto perche' ADR 6 e' solo-relativo; ordine
  canonico obbligatorio su entrambi i percorsi; `geo.union` debito
  separato; D14.3/D14.4 raccomandate dal revisore, ratifica formale
  owner in arrivo). Pin di conformita' sul tag `v0.1.0-rc1`. Dipendenza
  dichiarata dalla forma finale di ADR-0013.
