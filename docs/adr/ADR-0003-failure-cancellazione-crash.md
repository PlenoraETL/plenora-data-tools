# ADR 3 — Failure propagation, cancellazione e crash

- **Stato**: attuato parzialmente (Fase 2B-M1, executor seriale) — vedi
  "Stato di attuazione" in coda
- **Decisioni collegate**: D12, D14, D21, D23, D24
- **Riferimenti**: `Architetture.md` §6.4

## Contesto

Nel DAG parallelo un fallimento in un ramo deve fermare l'intera esecuzione in
modo controllato: niente output parziali, niente risorse trattenute, diagnosi
sufficiente a capire la causa. Tre livelli di problema: errori attesi,
cancellazione cooperativa (non sempre possibile per kernel monolitici), e crash
che nessun meccanismo in-process può intercettare.

## Decisione

### Errori: first-error wins

- Il primo errore **osservato** cancella immediatamente gli altri rami.
- L'errore **riportato** non è necessariamente il primo osservato: in un DAG
  parallelo due rami possono fallire quasi contemporaneamente e il "primo"
  cambierebbe tra esecuzioni. L'errore primario riportato è scelto con regola
  stabile: (1) errore non causato da cancellazione; (2) minore profondità
  topologica; (3) `NodeId` minore; (4) sequence number minore. Così la
  diagnosi è deterministica anche se l'osservazione non lo è.
- L'errore primario conserva: `execution_id`, nodo, operazione, categoria,
  source chain interna, disposizione di retry (R9.7, da milestone D — in M1d
  era il booleano retryable/non-retryable) se applicabile.
  **Mai** valori o payload sensibili.
- Gli errori secondari (conseguenti alla cancellazione) sono telemetria, non
  sostituiscono la causa iniziale.
- **Eccezione normativa row-scoped (Contracts 2.0-rc17, R9.9-R9.14):** un
  rifiuto deterministico di una riga in uno stream seriale non termina subito
  la scansione. Il componente sopprime ogni output successivo, continua solo
  per completare conteggi ed esempi bounded e riporta un unico errore terminale
  `plenora-row-diagnostics-v1`. Se la scansione si interrompe, il report diventa
  `partial`/`unknown` con `knowledge_limits`; non si dichiara falsa completezza.
  Questa eccezione non avvia altri rami, non modifica la selezione errori del DAG
  parallelo e non autorizza remediation o drop silenziosi.
- **Modalità diagnostica opt-in** (solo per input fidati): l'errore può
  includere nodo, indice batch, indice riga, colonna e tipo di violazione
  (es. `node=buffer batch=12 row=941 field=geometry reason=WKB_DEPTH_LIMIT`),
  con hash o descrizione strutturale del valore — mai il valore.

### Cancellazione

Ogni operazione dichiara in catalogo `cancellation_behavior`:

- `Cooperative`: controlla il token periodicamente durante il lavoro.
- `BoundaryOnly`: controlla solo ai confini di batch.
- `NonInterruptible` (alcune chiamate GEOS/PROJ): non offre punti di
  interruzione. Per queste: nessuna nuova attività dopo la cancellazione,
  latenza massima **documentata e osservabile nelle metriche**, lease
  trattenuti visibili al governor, gli altri rami possono attendere il
  completamento prima del cleanup finale. La v1 accetta questa attesa
  esplicitamente: **non si promette cancellazione immediata**. Isolamento in
  processo separato solo per backend dimostrabilmente instabili.

### Panic

- I panic dei worker sono intercettati al confine dell'executor con
  `catch_unwind`, causano cancellazione globale e sono convertiti in errore
  interno privo di dati sensibili.
- I kernel non usano panic per errori attesi.
- I confini di `catch_unwind` sono dichiarati; i tipi che attraversano il
  confine rispettano `UnwindSafe` o wrapper espliciti.
- L'errore primario è preservato; cleanup di spill, code e writer eseguito
  comunque.
- **Nessun panic può portare al publish dell'output.**

### Crash non intercettabili

`catch_unwind` non copre `panic = "abort"`, crash in GEOS/PROJ, OOM killer,
kill esterni, perdita del filesystem. Difesa strutturale:

- directory temporanee isolate per `execution_id`;
- **lock file come prova principale** di esecuzione viva (rilasciato dal
  sistema operativo dopo un crash);
- PID, identificativo host e timestamp di heartbeat come segnali diagnostici
  aggiuntivi — mai prove sufficienti (PID riutilizzabile; una macchina
  sospesa/ibernata può rendere vecchio il timestamp senza che l'esecuzione
  sia orfana);
- **scavenging all'avvio**: elimina solo directory senza lock attivo **e** con
  heartbeat scaduto; TTL conservativo;
- test di riavvio con directory lasciate intenzionalmente incomplete.

### Cleanup e publish

A successo, errore, cancellazione o panic intercettabile: cleanup di tutti i
file di spill, chiusura di code e broadcaster. Il publish avviene **solo** a
grafo completato con successo (invariante I8). Nessun nodo produce side effect
esterni osservabili (invariante I2): solo il sink finale pubblica.

## Conseguenze

- Test obbligatori: errore in un ramo con altri attivi; cancellazione durante
  kernel lungo; panic con cleanup completo; preservazione dell'errore primario;
  scavenging dopo riavvio simulato.
- La CLI resta potenzialmente in attesa di un kernel `NonInterruptible` dopo
  una cancellazione: comportamento accettato, documentato e osservabile.

## Stato di attuazione (Fase 2B-M1, executor seriale)

Implementato:

- **Panic**: `catch_unwind` ai due punti di dispatch dei kernel (streaming/
  blocking e binario), conversione in `PlenoraError::Step` con attribuzione di
  nodo, solo il messaggio del panic; publish mai raggiunto (test dedicato).
- **Cancellazione**: `CancellationToken` (`Arc<AtomicBool>` dietro tipo
  dedicato) in `RuntimeContext`; check ai confini executor (tra batch, tra
  kernel, durante il drain dei blocking, al confine di output) con onore del
  `cancellation_behavior` di catalogo (`Cooperative` per batch, `BoundaryOnly`
  tra kernel, `NonInterruptible` solo confini di piano); errore dedicato
  `PlenoraError::Cancelled`; CLI con handler Ctrl-C (1° pressione = cancel
  cooperativo, 2° = exit forzato), exit code 130, nessun publish al cancel.
- **Errori arricchiti**: `execution_id` (`exec-<uuid v4>`) in `Execution`
  (ex `Step`) e `Cancelled`; `ErrorCategory` con `category()` (mapping
  dichiarato per variante) e — da milestone D — `retry_disposition()` a 5
  valori canonici R9.7 al posto del `retryable()` (true solo per `Io`)
  qui deciso; **modalità diagnostica
  opt-in** (`RuntimeContext::diagnostics`): reason arricchita con
  nodo/batch/riga/colonna, mai valori.
- **Diagnostica row-scoped**: carrier boxed in `PlenoraError`, envelope CLI
  additivo e aggregazione seriale cross-batch per conversioni tipate,
  trasformazioni temporali, `flatten_json`, assert di qualita', hash con
  `null_policy=error` e `geo.from_wkt`; indici sorgente zero-based checked,
  conteggi completi ed esempi bounded. Le righe invalide non entrano
  nell'output accettato: le policy legacy di coercizione/null non autorizzano
  remediation implicita.
- **Provenance originale**: il catalogo classifica conservativamente i nodi
  che preservano cardinalita' e ordine. Un consumer row-diagnostic dopo
  filter/sample/explode/join/aggregate/sort/reshape o qualunque sibling path
  non conservativo viene rifiutato dal planner finche' non esiste un sidecar
  di lineage. La provenance non viene ricostruita o inventata.
- **Partizioni**: Data Tools non espone ancora nodi di quarantena. Di
  conseguenza non crea implicitamente output `accepted`/`rejected` e rifiuta
  `explode.empty_policy=drop`; una futura partizione dovra' essere un nodo
  esplicito con due archi distinti.
- **Crash defense**: `TempStore` per `execution_id` con `lock.json`
  (execution_id, PID, host, heartbeat), scavenging all'avvio fail-safe
  (PID morto o heartbeat oltre TTL 24h; lock corrotto conservativo con TTL×2;
  solo directory `plenora-*`), test di riavvio simulato; test crash tra
  scrittura e persist del publish.

**Rinvii a M3 (DAG parallelo)**: selezione deterministica dell'errore primario
multi-ramo, errori secondari come telemetria, cancellazione di rami concorrenti,
confini `UnwindSafe` ridisegnati per il parallelo, token passato ai kernel,
isolamento in processo per backend instabili.
