# ADR 12 — Fusione dei segmenti geo: forma decodificata transiente, semantica errori invariata

- **Stato**: accettato (design ratificato dall'owner 2026-07-29; attuazione
  M1, M2 e M3 completate)
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
colonna geometria e ruolo (`TransformInPlace`). Con M3 il run ammette anche
`reproject` e `make_valid`; a feature spenta i piani che li contengono sono
rifiutati in validazione (capability `proj`/`geos` fail-closed), quindi la
formazione di gruppi con questi op esiste solo a backend compilato.

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
`NonInterruptible` — attuati in M3, vedi sotto), `line_substring`/
`line_interpolate_point` (check di tipo per-riga — candidati futuri), misure
terminali (M2, attuato — vedi sotto), join/binari, 1:N, blocking,
collettive, fusione cross-segmento e table↔geo.

## Perimetro (M2) — misure terminali in coda al gruppo

Un gruppo di fusione e' un run massimale di `TransformInPlace` (>= 1) piu'
UNA misura `TerminalMeasure` opzionale in coda (`area`, `length`,
`perimeter`, `vertex_count`, `to_wkt` — le 5 misure "add column" dei piani
v4), stessa colonna geometria. Una misura da sola NON forma gruppo: non
c'e' nulla da fondere e resta sul percorso nodo-per-nodo. Vincoli
strutturali emersi in attuazione (risposte del design check):

1. **La colonna geometria SOPRAVVIVE alla misura** (semantica v4 "add
   column": `geo_measure_batch` appende la colonna scalare in coda allo
   schema; `output_column` cambia solo il nome della colonna misura). Il
   gruppo con misura ri-encoda quindi la geometria UNA volta al confine,
   come senza misura, e produce in piu' la colonna scalare.
2. **Le misure dei piani v4 non passano per `one_to_one_batch_prepared`**:
   sono un ramo executor dedicato (`PreparedConfig::GeoMeasure` ->
   `geo_measure_batch`). I bracci misura di `transform_cells` (trasporto
   legacy v3, sostituzione della colonna geometria) non sono raggiunti dai
   piani v4 e restano intoccati.
3. **Attribuzione errori del ramo non fuso, riprodotta esatta**: decode
   per cella via `decode_geometry_cell` chiuso da `step_error` SENZA
   transito da `ArrowTransportError`; kernel misura chiuso in
   `InvalidPlan` dal display dell'`OperationError`; null-in -> null-out;
   primo errore in ordine di riga. Per questo il runner fuso ha la
   variante dedicata `FusedStepError::Measure` che porta il `PlenoraError`
   grezzo: la validazione inter-passo prima della misura (strutturale +
   OGC, profilo B di D12.4) e' attribuita al NODO MISURA (e' il decode
   che fallirebbe li'), gli errori del kernel misura al nodo misura.
   Il check `MAX_CELL_BYTES` input-side del decode della misura non e'
   riprodotto: irraggiungibile (l'encode del nodo a monte scatta prima,
   stessa classe di D12.3). Il passo misura e' DOPO il loop dei kernel:
   nel percorso non fuso il nodo trasformazione completa tutte le righe
   (kernel + encode) prima che il nodo misura decodifichi la prima cella —
   la precedenza degli errori e' identica per costruzione.
4. **Governor invariato**: la reservation D12.7 (byte decodificati della
   colonna geometria) copre il gruppo con misura senza cambi; l'output
   scalare e' nel lease di uscita del segmento, come per ogni nodo.

Nota di copertura (come i casi (d2)/(e) dell'oracolo): con gli op del
perimetro M1+M2 nessun intermedio OGC-invalido puo' raggiungere il nodo
misura — ogni kernel fondibile valida il proprio output. La validazione
pre-misura e' difesa in profondita', verificata a livello runner; i casi
errore raggiungibili con misura in coda (limiti di cella, input invalido)
sono nell'oracolo e mantengono l'attribuzione ai nodi trasformazione.

## Perimetro (M3) — reproject e make_valid

I due op rinviati in M1 (backend feature-gated, `NonInterruptible`)
entrano nel perimetro come `TransformInPlace`: le liste chiuse diventano
16 trasformazioni + 5 misure. Decisioni specifiche:

1. **Nessuna variante di capability nuova.** La relazione di
   raggruppamento e' identica per entrambi (1:1 in place sulla stessa
   colonna, ruolo `TransformInPlace`): la differenza semantica di
   `make_valid` (input OGC-invalido ammesso) e' una proprieta' del suo
   gate di decode, non della fondibilita', e il runner gia' distingue i
   profili per operazione. Una variante dedicata avrebbe cambiato la
   regola di adiacenza di `annotate_fusion_groups`, il test del perimetro
   e i nomi del JSON capability senza alterare nessuna decisione presa
   dal catalogo. `prepare` e' INVARIATO: la capability decide; a feature
   spenta la validazione fail-closed (capability `geos`/`proj` mancante)
   rifiuta il piano prima di `prepare`, quindi i gruppi con questi op si
   formano solo a backend compilato — nessun gate `cfg` necessario in
   `prepare` (sarebbe codice irraggiungibile).

2. **D12.4-M3 — eccezione OGC per make_valid (trappola 1).** Nel percorso
   non fuso il "decode" di `make_valid` e' il SOLO gate strutturale di
   `make_valid_wkb` (`validate_wkb_contract`, nessun check OGC: l'input
   invalido e' esattamente cio' che l'operazione ripara). Il runner fuso
   riproduce la stessa semantica in due punti:
   - **decode iniziale del gruppo**: se il primo kernel e' `make_valid`,
     decode SOLO strutturale (`wkb_decoder::decode_validated`, la stessa
     camminata validante senza il check OGC) invece di
     `geometry_from_wkb`;
   - **validazione inter-passo**: davanti a un nodo `make_valid` il
     profilo B e' SOLO strutturale (il check OGC e' omesso; restano
     finitezza/anelli/conteggi e il check `MAX_CELL_BYTES` del
     produttore).
   L'eccezione e' speculare a quella di `geometry_diagnostics` (che
   valuta la validita' come dato). Dopo `make_valid` la validazione
   inter-passo standard (strutturale + OGC) resta in vigore: l'output
   riparato e' valido per contratto del kernel (`InvalidRepair`
   altrimenti, errore del kernel al nodo make_valid). Il kernel su forma
   decodificata e' `make_valid_geometry` (plenora-kernels-geo): ri-encoda
   la forma canonica XY e riusa LETTERALMENTE `make_valid_wkb` — stesso
   gate, stessa riparazione GEOS, stessa rivalidazione dell'output,
   stessi errori.
   Nota di raggiungibilita' (come (d2)/(e)): nessun op del perimetro
   produce output OGC-invalido, quindi la meta' inter-passo
   dell'eccezione e' difesa in profondita'; la meta' raggiungibile
   (make_valid in testa, input OGC-invalido dall'arco) e' il caso (m3-a)
   dell'oracolo.
   Divergenza registrata, irraggiungibile dai piani: su input
   STRUTTURALMENTE invalido in testa a un gruppo che apre con
   `make_valid`, il messaggio del runner fuso (`geometria non valida: …`
   da `decode_validated`) differisce nel prefisso da quello non fuso
   (`make_valid GEOS fallito: …` da `make_valid_wkb`) — stessa variante
   `InvalidPlan`, stesso nodo, stessa riga. Il caso non puo' verificarsi
   nel motore: l'arco di input e ogni nodo a monte rifiutano il WKB
   strutturalmente invalido prima del gruppo (fail-closed, caso (c)
   dell'oracolo).

3. **reproject: guardie interne invariate (trappola 2).** Il kernel
   riceve la geometria decodificata e le sue guardie (input finito,
   `check_validation`, dominio CRS, limite coordinate, output finito,
   output valido) si applicano identiche: errori del kernel attribuiti al
   nodo reproject (come oggi), output al successore col profilo B
   standard. La pipeline PROJ resta thread-local (`REPROJECTOR`), riusata
   per kernel nel loop rayon — stesso pattern del braccio non fuso.

4. **Schema di output del gruppo = schema dell'ULTIMA trasformazione.**
   `reproject` cambia il CRS del campo geometria a meta' catena: il batch
   di confine e' costruito sull'handle prepared dell'ultima
   trasformazione, non del primo kernel. La ricostruzione canonica del
   campo dipende solo da (nome colonna, CRS di output) e gli altri campi
   passano invariati, quindi per gli op M1/M2 (CRS invariato) lo schema
   coincide con quello del primo kernel — comportamento precedente
   invariato — e l'handle risolto sullo schema del batch e' identico a
   quello che il percorso non fuso risolverebbe sullo schema intermedio.

5. **Attribuzione (tabella).** Per `make_valid`: errori del kernel GEOS
   (riparazione fallita, output ancora invalido, limiti) -> nodo
   make_valid; input strutturalmente invalido -> primo kernel se in testa
   (come il gate di `make_valid_wkb`), altrimenti irraggiungibile; output
   al successore: profilo B standard. Per `reproject`: errori di
   risoluzione parametri/CRS -> nodo reproject (estrazione una tantum,
   stessa posizione del braccio non fuso); errori del kernel PROJ -> nodo
   reproject; output al successore: profilo B standard.
   `BackendUnavailable` a feature spenta: stessa variante e stesso nodo
   nei due percorsi (difesa in profondita' a livello runner — i piani
   sono rifiutati prima, in validazione).

6. **NonInterruptible compatibile per costruzione.** La cancellazione
   della fusione e' controllata TRA i kernel (callback `control`, stessa
   granularita' del loop non fuso), mai dentro un kernel: il callback
   invoca `check_cancellation` del nodo, che onora il
   `CancellationBehavior` di catalogo — davanti a `make_valid`/
   `reproject` il check e' saltato in entrambi i percorsi e il
   `Cancelled` e' osservato al primo confine cooperativo successivo. Il
   comportamento generico e' coperto da
   `non_interruptible_op_is_never_interrupted`; il callback del runner fuso
   e il ramo positivo `FusedStepError::Control` sono coperti direttamente da
   `fused_control_observes_cancellation_after_non_interruptible_make_valid`.
   Il caso m3-d dell'oracolo differenziale copre invece la cancellazione
   durante lo staging WKB atomico, osservata a `main` prima che il gruppo
   inizi, con attribuzione identica nei due percorsi.

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
- 2026-07-29: M2 attuato. I gruppi ammettono UNA misura `TerminalMeasure`
  in coda (run di `TransformInPlace` >= 1 + misura opzionale, stessa
  colonna; misura singola: nessun gruppo) — annotazione in `prepare`
  (`annotate_fusion_groups`), runner fuso esteso in
  `geo_transport/unary.rs` (`transform_cells_fused` con terminale,
  `apply_fused_measure`/`measure_cells` in un passo dedicato DOPO il loop
  dei kernel — precedenza errori identica al non fuso — e nuova variante
  `FusedStepError::Measure` col `PlenoraError` grezzo: il ramo non fuso
  delle misure non transita da `ArrowTransportError`), innesto in
  `try_run_fused_group` (batch costruito in due `try_new`: confine ultima
  trasformazione -> ultimo transform, append colonna misura sul contratto
  -> nodo misura, come `append_output_column`). Colonna geometria
  SOPRAVVIVENTE ri-encodata una sola volta; governor invariato (D12.7);
  cancellazione/metriche/`node_rows` per nodo come M1 (misura 1:1).
  Test: formazione gruppi M2 + misura singola senza gruppo (prepare);
  parita' byte-per-byte runner (unary) e A/B engine translate+area /
  translate+to_wkt con null e multi-tipo (executor); oracolo esteso
  (happy path con misura in coda, cella oversize e input OGC-invalido con
  misura in coda — attribuzione ai transform invariata). Il caso «misura
  su intermedio invalido» e' non realizzabile con gli op M1 (come i casi
  (d2)/(e)): coperto a livello runner come difesa in profondita'.
- 2026-07-29: misura A/B M2 (`bench_geo_fusion` esteso allo scenario
  `chain_terminal_measure` = buffer→simplify→centroid→area, la catena
  completa del baseline). NOTA METODOLOGICA: il delta fuso/non fuso
  dipende fortemente dalle condizioni dell'host — host rumoroso con
  allocatore di default: −2/−8% con bande sovrapposte; con
  `MALLOC_ARENA_MAX=4 MALLOC_MMAP_THRESHOLD_=32768` (mitigazione
  documentata in `benchmarks/sweep/geo_sweep.md`): **−20,2%** (catena
  trasformazioni) e **−16,2%** (con misura terminale), bande min/max non
  sovrapposte; −14,6% gia' misurato per M1 in condizioni quiete. La
  direzione e' coerente in ogni condizione; i numeri canonici sono quelli
  a bande non sovrapposte. Output byte-identici, zero fallback.
- 2026-07-29: M3 attuato. `geo.reproject` e `geo.make_valid` entrano nel
  perimetro come `TransformInPlace` (16+5; test del perimetro e snapshot
  di catalogo aggiornati, `catalog_fingerprint` invariato per D12.2).
  Catalogo: nessuna variante nuova (decisione 1 del perimetro M3).
  Runner: bracci `MakeValid`/`Reproject` in `resolve_transform`/
  `apply_transform_cell` (con i bracci `BackendUnavailable` a feature
  spenta, stessa variante del non fuso); kernel `make_valid` su forma
  decodificata via `make_valid_geometry` (nuovo, riusa `make_valid_wkb`
  letteralmente); `reproject` col thread-local `REPROJECTOR` per kernel.
  Eccezione D12.4-M3 attuata nei due punti (decode iniziale del gruppo e
  validazione inter-passo davanti a make_valid, entrambe SOLO
  strutturali). Batch di confine sullo schema dell'ULTIMA trasformazione
  (cambio CRS di reproject a meta' gruppo; identico al precedente per
  CRS invariato). `prepare` invariato (validazione fail-closed a feature
  spente). Test: formazione gruppi M3 (prepare, feature-gated); parita'
  byte-per-byte, trappola 1 e `BackendUnavailable` (unary, entrambe le
  configurazioni); oracolo esteso (m3-a riparazione di input
  OGC-invalido senza errori, m3-b catena con reproject e schema al CRS
  target, m3-c make_valid a meta' catena, m3-d cancellazione con
  `NonInterruptible`, piu' il caso senza gate a feature spente).
  Benchmark (`bench_geo_fusion`, scenario `chain_reproject` = reproject
  EPSG:32632 -> EPSG:3857 -> translate -> rotate, con warmup delle
  pipeline PROJ thread-local): delta fuso/non fuso favorevole ma piccolo
  (−3% / −12% a seconda delle condizioni dell'host, con la mitigazione
  allocatore documentata; la riproiezione domina il costo del gruppo ed
  e' identica nei due percorsi), output byte-identici, zero fallback.
  NOTA di ambiente: il build bundled di PROJ nel container `rust:1.92`
  richiede `cmake`, `sqlite3` e `libsqlite3-dev` (assenti
  dall'immagine): installati nel container effimero per la verifica
  full-backends.
