# ADR 12 — Fusione dei segmenti geo: forma decodificata transiente, semantica errori invariata

- **Stato**: accettato (design ratificato dall'owner 2026-07-29; attuazione
  M1 in corso)
- **Decisioni collegate**: ADR 1 (determinismo), ADR 2 (resource
  accounting), ADR 3 (failure/cancellazione), ADR 5 (ValidatedGraph vs
  ExecutionPlan — la fusione e' decisione fisica), ADR 6 (semantica
  limiti), ADR 11 (decoder validante; precedenza al validatore
  strutturale), ADR-0009/deroghe (registro deroghe R16.2)
- **Riferimenti**: `Prestazioni.md` V6/G1/G2; `plenora-contracts` R0.1
  (evidenza riproducibile), R2.4 (lineage), H-03 (memoria)

## Contesto

Ogni nodo `geo.*` del DAG decodifica WKB → `geo::Geometry` → kernel →
ri-encoda WKB (`transform_cells`, `geo_transport/unary.rs`). Su una catena
di nodi unari consecutivi ogni forma intermedia viene serializzata e
riletta senza che nessun osservabile la richieda: l'harness di baseline
misura `chain_wkb` 29,0s vs `chain_fused` 15,8s (**−45%**) su
buffer→simplify→centroid→area a 1M righe
(`benchmarks/baseline/baseline.json`).

Lo steer dell'owner (2026-07-29) ha fissato che il rischio non e'
nell'executor ma nella **semantica degli errori**: fondere non deve
introdurre errori che prima non esistevano ne' farne sparire di legittimi
(caso concreto: i limiti di cella in encode, che oggi scattano a ogni
nodo intermedio); contratto e lineage R2.4 non devono cambiare; un
fallimento deve continuare a dire *quale* nodo ha rotto. Due condizioni
vincolanti: fusione **capability-gated per coppia** e **oracolo esteso
agli errori** (stesso input → stesso output o stesso errore, stessa riga,
stessa fase, stesso nodo — non solo il percorso felice).

## Decisione

### D12.1 — Forma decodificata transiente, nessun tipo nuovo sugli archi

Gli archi restano `RecordBatch` WKB canonico ISO XY (G1: il WKB resta il
contratto esterno e degli archi osservabili). Per un gruppo di kernel
unari consecutivi fondibili dentro uno stesso segmento streaming, un
unico loop per cella mantiene `Geometry<f64>` in un
`Vec<Option<Geometry<f64>>>` che vive **solo per la durata del gruppo sul
singolo batch** e viene ri-codificato una sola volta. Niente varianti di
`BatchStream`, niente cache di decode tra batch o globali. Tee/fan-out,
`GovernedBatch`, `BatchSequence` (propagazione 1:1) e publish atomico
(ADR 7) sono intoccati. Al confine di segmento restano encode canonico +
rivalidazione (R0.1, nessuna deroga «valido per costruzione»).

### D12.2 — Capability di catalogo, fuori dal fingerprint

Nuovo campo `geo_fusion` su `OperationDescriptor`
(`NotFusible` / `TransformInPlace` / `TerminalMeasure`), stesso principio
dichiarativo di `cancellation_behavior` e di §10; esposto in
`capabilities.rs` dalla stessa fonte. Il planner fonde solo run massimali
di nodi consecutivi in cui *entrambi* i nodi lo permettono, a parita' di
colonna geometria e ruolo (`TransformInPlace`).

**Decisione deliberata: `geo_fusion` NON entra in
`descriptor_canonical`** e quindi non cambia `catalog_fingerprint`. Il
fingerprint guarda la compatibilita' semantica dei piani
(`planner.rs:628`); la fondibilita' e' una capability *fisica* (ADR 5) —
aggiungerla invaliderebbe piani semanticamente identici. Nessun bump di
versione per questo campo (ADR 4).

### D12.3 — Limiti di cella ri-applicati, esatti, con attribuzione

Nessuna deroga sui limiti di cella. La dimensione del WKB XY di una
`Geometry` e' funzione pura della struttura; un helper `wkb_size_xy`
(plenora-kernels-geo) la calcola esattamente (stessa camminata del
decoder, nessuna stima). Dopo ogni kernel del gruppo il runner fuso
applica il check `MAX_CELL_BYTES` **con attribuzione al nodo che ha
prodotto l'output** — stesso errore, stesso nodo, stessa riga del
percorso non fuso; la selezione del primo errore resta in ordine di riga
(pattern rayon con collect indicizzato, `unary.rs:149-171`). Il check
input-side sui nodi interni, oggi irraggiungibile (l'encode del nodo
precedente scatta prima), non e' riprodotto: l'oracolo lo dimostra
invariante.

### D12.4 — Validazione inter-passo con attribuzione per profilo

Tra i passi del gruppo il runner esegue `validate_geometry_structural`
(nuovo helper: finitezza coordinate, chiusura anelli, profondita',
conteggi — stesse regole/ordine di `wkb_decoder`) + `check_validation`
OGC, riproducendo i due profili del percorso non fuso:

- **Profilo A** (op via `transform_wkb`: centroid, convex_hull, envelope):
  l'output e' rivalidato *al nodo k* (`lib.rs:647`) — errore attribuito a
  *k* in entrambi i percorsi (es. centroide con overflow a `inf`).
- **Profilo B** (tutte le altre op via `encode_geometry`): un intermedio
  invalido fallirebbe *al decode del nodo k+1* — errore attribuito a
  *k+1*.

**Precedenza al validatore strutturale** (come ADR 11): se il round-trip
WKB odierno *normalizza* cio' che il validatore rifiuta (es. `-0.0`,
payload con NaN), dare precedenza al validatore **cambia il comportamento
attuale** — e' il cambiamento voluto, registrato qui come tale e NON come
invarianza (decisione owner, punto 1).

### D12.5 — Contratto, lineage, FieldId: fusione solo fisica

Gli `analyze_*` girano in `validate` per **ogni** nodo, a secco e senza
dati, indipendentemente dalla fusione: nessun contratto d'arco cambia,
l'assegnazione dei `FieldId` e' identica, la lineage R2.4 non
«sopravvive» a nulla perche' nel modello logico non c'e' ricostruzione
intermedia. Regola: **la fusione non puo' cambiare nessun contratto
d'arco** — test di identita' contratto/schema/FieldId con fusione on/off
(incluso il blocco canonico R2.2).

### D12.6 — Errori e metriche per nodo preservati

Il runner fuso e' un'esecuzione alternativa di un gruppo, non una
rimozione dei nodi: righe in/out per nodo esatte (1:1),
`check_cancellation` per kernel con granularita' identica, errori mappati
al kernel che li ha prodotti via `step_error` (stessa forma
`Execution { node, operation, execution_id }`), `catch_unwind` sul gruppo
con attribuzione al kernel in corso, metriche `node_rows`/`wall_time`
per nodo. La domanda «quale step ha rotto» resta rispondibile.

### D12.7 — Memoria: reservation esatta, fallback strumentato

Prima del loop del gruppo: reservation sul governor dei byte decodificati
(esatti, dalla stessa camminata di `wkb_size_xy`); rilascio prima della
reservation del lease di uscita. **Fallback al percorso non fuso** solo
come scelta fisica blocco-contiguo-vs-allocazioni-frammentate (decisione
owner, punto 3): NON silenzioso — ogni fallback emette una metrica/evento
dedicato, cosi' una pressione ricorrente e' osservabile invece di
manifestarsi come rallentamento inspiegabile. Nessun errore nuovo
introdotto.

### D12.8 — Deroga `max_batch_bytes` sugli archi interni fusi

Su un arco interno fuso il batch non e' materializzato: il check byte di
`check_edge_batch` non e' applicabile. **Deroga registrata**
(`docs/deroghe.md`) con la dicitura: «su questo percorso H-03 e' coperto
dal governor (reservation esatta D12.7), non da `max_batch_bytes`». Righe
e batch per arco restano esatti (1:1) e i limiti corrispondenti si
applicano. **Condizione di accettazione (owner, punto 2): esiste un test
che dimostra che il governor scatta davvero su un batch oltre soglia** —
senza quel test la deroga non e' in vigore.

### D12.9 — Kill switch

`RuntimeContext.geo_fusion: bool` (default `true`), registrato nel piano.
Con `false` i gruppi non si formano e l'esecuzione e' quella attuale:
serve alla disattivazione operativa (G2), all'oracolo differenziale e ai
benchmark A/B.

## Perimetro (M1)

Fondibili (`TransformInPlace`): buffer, simplify, centroid, convex_hull,
envelope, boundary, point_on_surface, affine_transform, translate, scale,
rotate, concave_hull, densify, snap_to_grid — 14 op 1:1 pure Rust,
revistati uno per uno con l'owner (criterio: infallibile, o fallisce
esattamente sulle stesse righe nei due percorsi; mai se cambia il numero
di righe). Esclusi: `reproject`/`make_valid` (backend feature-gated,
`NonInterruptible` — cantiere M3), `line_substring`/
`line_interpolate_point` (check di tipo per-riga — candidati M3), misure
terminali (M2, cantiere separato), join/binari, 1:N, blocking, collettive,
fusione cross-segmento e table↔geo.

## Oracolo (gate di M1)

Test differenziale fusione on/off, esteso agli errori: stesso input →
stesso output byte-per-byte O stesso errore (variante, nodo, fase, riga —
con `diagnostics` attivo). Casi avversari obbligatori: (a) percorso
felice multi-tipo; (b) cella che eccede `MAX_CELL_BYTES` al nodo 1 di 3;
(c) input malformato al primo nodo; (d) non-finiti prodotti da kernel
(centroide overflow, scale ×1e308); (e) OGC-invalido prodotto a meta'
catena; (f) cancellazione a meta' gruppo; (g) panic iniettato via hook.
Piu': identita' contratto/schema/FieldId on/off e test governor-oltre-
soglia (condizione D12.8). L'oracolo diventa obbligatorio per qualunque
nuovo op dichiarato fondibile in futuro (regola 3: «check che sparisce
fondendo» e' una classe).

## Cambi di comportamento (dichiarati)

- Dove il round-trip WKB odierno normalizza input che il validatore
  strutturale rifiuta, il percorso fuso rifiuta: cambiamento voluto
  (D12.4), non invarianza.
- Metrica/evento nuovo per il fallback non fuso (D12.7): osservabilita'
  aggiunta, nessun errore nuovo.
- `catalog_fingerprint` invariato per scelta deliberata (D12.2); lo
  snapshot di catalogo cambia per il nuovo campo — PR esplicita.

## Stato di attuazione

- 2026-07-29: design ratificato dall'owner con le risposte ai 6 punti
  aperti (precedenza validatore come cambiamento dichiarato; deroga H-03
  condizionata al test governor; fallback solo frammentazione +
  strumentato; elenco 14 op revistato; nessun bump fingerprint; M2 in
  cantiere separato). Milestone M1 avviata.
- 2026-07-29: M1 attuato. `RuntimeContext.geo_fusion` (D12.9, registrato
  in `ExecutionPlan::geo_fusion`); gruppi annotati in `prepare`
  (`PreparedKernel.fusion_group`, run massimali capability-gated D12.2);
  runner fuso in `geo_transport/unary.rs` (`transform_cells_fused` +
  `one_to_one_batch_fused`, loop kernel-esterno/celle-interno, tabella di
  attribuzione D12.3/D12.4 con gli helper
  `transform_geometry_canonical`/`check_geometry_valid` di
  plenora-kernels-geo); ramo fuso in `run_streaming_chain` con reservation
  D12.7, fallback strumentato (contatore `geo_fusion_fallbacks` in
  `ExecutionMetrics`) e contabilita' per nodo D12.6. Test: formazione
  gruppi/kill switch (prepare), parita' byte-per-byte e attribuzione
  errori (unary), A/B end-to-end e governor-oltre-soglia (executor —
  condizione DER-003 soddisfatta). Oracolo differenziale esteso: task
  separato, resta gate per nuovi op fondibili.
- 2026-07-29: oracolo esteso completato
  (`crates/plenora-engine/tests/geo_fusion_oracle.rs`): i casi (a)-(f)
  piu' l'identita' contratto/schema on/off — doppia esecuzione con kill
  switch e confronto di variante, nodo, categoria, fase, effetto e
  disposizione retry; NESSUNA divergenza trovata (il caso -0.0/NaN di
  D12.4 non si e' manifestato sulle fixture). Caso (g) in
  `executor/tests.rs` (`g_fused_group_panic_is_attributed_to_the_panicking_kernel`,
  nel crate perche' l'hook `PANIC_AT_NODES` e' privato): panic al nodo
  centrale, stessa attribuzione nei due percorsi. 8+1 test, tutti verdi.
- 2026-07-29: M1 CHIUSO. Misura A/B engine-level
  (`crates/plenora-engine/examples/bench_geo_fusion.rs`: catena
  buffer→simplify→centroid, 200k righe miste poligoni/punti/null, 20
  batch, mediana di 5 run alternate): fuso 0,522s vs non fuso 0,611s =
  **−14,6%** (bande min/max non sovrapposte), output byte-identici,
  `geo_fusion_fallbacks` 0. Il −45% kernel-level del baseline si conserva
  parzialmente a livello engine (overhead fisso condiviso: framing
  RecordBatch, lease governor, metriche per nodo, validazione al gate).
  Prossimi cantieri: M2 misure terminali (TerminalMeasure), M3
  reproject/make_valid ed estensioni controllate.
