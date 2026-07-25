# ADR 5 — Separazione `ValidatedGraph` / `ExecutionPlan`

- **Stato**: accettato (design)
- **Decisioni collegate**: D15, D25, D26
- **Riferimenti**: `Architetture.md` §6.3; `Prestazioni.md` §5 (E1–E3)

## Contesto

Il `ValidatedGraph` contiene il risultato della validazione: ciò che è vero
indipendentemente dai dati effettivi. Ma input con lo stesso schema possono
avere scale opposte (cento righe o un miliardo, geometrie semplici o con
migliaia di componenti), e la strategia fisica migliore cambia di conseguenza.
Se la strategia fosse fissata in validazione, il riuso del grafo su dati diversi
produrrebbe piani subottimali; se fosse rifatta da zero a ogni esecuzione, si
perderebbe il valore della validazione upfront.

## Decisione

L'API pubblica ha esattamente due passi; `prepare` ed `execute_physical` sono
interni:

```rust
// API pubblica
fn validate(plan_json: &str, input_contracts: &[DataContract])
    -> Result<ValidatedGraph>;
fn execute(graph: &ValidatedGraph, inputs: Inputs, runtime: RuntimeContext)
    -> Result<Output>;

// implementazione interna di execute
let execution_plan = prepare(graph, &runtime)?;
execute_physical(&execution_plan, inputs)
```

### Cosa vive nel `ValidatedGraph` (semantico, stabile)

- struttura del DAG, config deserializzate e migrate;
- `DataContract` di ogni arco (schema, geometrie, proprietà `Proven`);
- CRS risolti, capability richieste, ordini dichiarati, policy di determinismo;
- identità (hash, fingerprint — ADR 4).

### Cosa vive nell'`ExecutionPlan` (fisico, per-esecuzione)

- segmenti fisici (`PhysicalSegment`) con `SegmentMode` esplicita
  (`LinearStreaming`, `GeoFused`, `Blocking`, `BinaryBlocking`);
- strategia di parallelismo per segmento (`SerialFused`, `ParallelPerBatch`,
  `ParallelPerBranch`, `BlockingSingleTask`);
- punti di materializzazione, batch size, quote di risorse per nodo;
- last consumer di ogni arco e risorsa intermedia;
- metriche da raccogliere.

Ogni kernel fisico riceve una **configurazione preparata**: deserializzata,
validata, tipizzata, normalizzata, risolta rispetto agli indici di colonna —
nessun JSON né ricerche per nome nel loop di esecuzione (vincolo V2/E1).

### Statistiche di runtime

```rust
enum RuntimeStatistic<T> { Known(T), Estimated(T), Unknown }
```

Negli Arrow IPC stream il numero di righe/batch non è noto prima della lettura;
nel file format può esserlo parzialmente. Regole:

- `prepare` produce **sempre un piano valido** anche con statistiche
  completamente assenti (`Unknown` → scelta conservativa);
- le statistiche `Known`/`Estimated` possono solo **migliorare** scelte
  fisiche correggibili (batch size, seriale vs parallelo, spill preventivo);
- nessuna scelta semantica può dipendere da una statistica `Estimated`
  (coerente con D25: solo `Proven` come precondizione semantica).

### Fusione senza perdita di osservabilità (E3)

Ogni segmento fisico fuso mantiene la mappa verso i nodi logici originari:
attribuzione degli errori, metriche per nodo, limiti e conteggi per arco,
cancellazione, proprietà intermedie nei test, determinismo, tracciamento dei
decode/encode WKB.

## Conseguenze

- Uno stesso `ValidatedGraph` produce `ExecutionPlan` diversi su dati di scala
  diversa: comportamento voluto, non un difetto.
- Il planner semantico resta una funzione pura testabile; il preparer è
  testabile separatamente con contesti runtime sintetici (statistiche assenti,
  minime, complete).
- Due `ExecutionPlan` semanticamente equivalenti possono differire nei limiti
  fisici dipendenti dal piano (es. `max_total_rows_processed`, ADR 6): per
  questo quel limite è solo metrica/limite avanzato.
