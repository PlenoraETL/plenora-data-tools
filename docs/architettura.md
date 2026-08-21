# Architettura

## I crate

| crate | responsabilità |
|---|---|
| `plenora-core` | contratti dati, catalogo unificato delle operazioni, limiti, CRS, tassonomia degli errori, panic policy |
| `plenora-kernels-table` | kernel tabellari puri `&RecordBatch -> Result<RecordBatch>` |
| `plenora-kernels-geo` | kernel geografici, adattatori Arrow, backend GEOS e PROJ |
| `plenora-engine` | formato del piano, planner, executor, governor della memoria, confine Arrow IPC, trasporto geo |
| `plenora-cli` | l'unico eseguibile distribuito |

Il catalogo vive in `plenora-core` ed è **unico**: tabellari e geografiche
sono descritte dalla stessa struttura, con le stesse versioni per componente.

## Dal piano all'output

```
piano JSON
   │  ingresso di versione: migrazione v4 → canonico v5
   ▼
planner::validate ─────────────► ValidatedGraph   (semantico, stabile)
   │  struttura, contratti, CRS,        │  plan_hash, catalog_fingerprint,
   │  capability, limiti effettivi      │  contratti per arco, ordine topologico
   ▼                                    ▼
explain ───────────────────────► ExecutionPlan    (fisico, per esecuzione)
   │                                    │  segmenti, strategia, fusione
   ▼                                    ▼
execute ───────────────────────► Output           (batch + metriche)
```

La separazione fra i due non è formale. Nel **`ValidatedGraph`** vive ciò che
è semantico e stabile: il piano canonico, l'identità, i contratti per arco. Se
due esecuzioni hanno lo stesso grafo, hanno lo stesso significato.
Nell'**`ExecutionPlan`** vive ciò che è fisico e per-esecuzione: segmenti,
strategia, fusione dei nodi geo. Cambiare la strategia non cambia il
significato, quindi non cambia l'identità.

Un piano semanticamente valido ma fuori dal dispatch corrente fallisce in
`validate`, **non a metà esecuzione**.

## Planner ed executor

Il planner raggruppa i nodi in **segmenti**. Un segmento streaming attraversa
i batch senza materializzare; un segmento blocking accumula prima di produrre.
La classe di esecuzione (`Streaming`, `Blocking`, `BinaryBlocking`) è
dichiarata in catalogo per operazione, non dedotta.

**L'esecuzione fra i nodi del DAG è seriale.** Il parallelismo esiste solo
**dentro** i kernel, tramite Rayon. Uno scheduler che esegua rami indipendenti
in parallelo non è implementato: vedi
[`stato-e-roadmap.md`](stato-e-roadmap.md).

La **fusione** dei segmenti geo evita di ri-materializzare la geometria fra
operazioni consecutive: il payload resta in forma decodificata transiente. La
fusione non toglie osservabilità — le metriche restano per nodo anche dentro
un gruppo fuso — e non cambia la semantica degli errori.

Le strutture di metriche pubbliche (`ExecutionMetrics`, `NodeMetrics`,
`SegmentMetrics`, `MemoryMetrics`, `SpillMetrics`) sono `#[non_exhaustive]`:
crescono con l'osservabilità del componente, e un campo nuovo non è una
rottura per chi le legge.

## Determinismo

Due livelli, dichiarati e testati separatamente.

**Livello 1 — semantico, sempre garantito.** A parità di piano validato e
input, qualunque schedule produce le stesse righe, gli stessi valori, gli
stessi null, lo stesso ordine dichiarato dove l'operazione ne definisce uno, e
le stesse geometrie. È verificato da un property test obbligatorio: stesso
piano, schedule forzato seriale contro parallelo, risultato semanticamente
identico.

**Livello 2 — IPC canonico, opzionale.** In aggiunta: stessi confini di batch,
stesso ordine dei metadati, stesso dictionary layout, stesso formato binario.
Garantito **solo a parità di versione dell'engine**, e mai promesso fra
versioni, piattaforme o configurazioni diverse. Serve a test di regressione,
cache e hashing dell'output.

**Uguaglianza geometrica**: il confronto fra geometrie è geometrico, non
byte-per-byte sul WKB. Tolleranza dichiarata sulle coordinate floating point,
normalizzazione topologica opzionale, `-0.0` uguale a `+0.0`, `NaN` uguale a
`NaN` ai fini del determinismo — mai propagato come valore valido, perché la
validazione dinamica rifiuta le coordinate non finite. La garanzia è limitata
alla **stessa versione dei backend**: l'output di operazioni booleane o di
riproiezione può variare fra versioni di GEOS o PROJ.

**Forma numerica**: nessun fused multiply-add. La conversione intero/decimal
verso `f64` è esatta o è un errore, con l'eccezione dichiarata delle
operazioni il cui risultato è `Float64` per contratto
([`errori-e-limiti.md`](errori-e-limiti.md)).

**Regola di catalogo**: ogni operazione con ordine non definito — union,
concat di rami paralleli, set operations, aggregazioni — dichiara la propria
politica di determinismo. Nessuna iterazione su hash map con ordine indefinito
raggiunge l'output; i kernel paralleli usano collect indicizzato.

## Ordine e `BatchSequence`

L'ordine dell'output è **logico**, assegnato dal piano, mai temporale:
«chi ha finito prima» non è deterministico in presenza di rami paralleli.

Ogni batch porta una `BatchSequence`: `source_node` (il nome dell'input),
`sequence_number` (contatore per input), `input_partition`. È assegnata agli
input, trasportata nel `GovernedBatch` attraverso lo stream, propagata 1:1 nei
segmenti streaming e **riassegnata deterministicamente** nei blocking, secondo
l'ordine di scansione.

Stato reale: la sequenza è assegnata, propagata e testata, ma **non ancora
usata per riordinare**, perché in esecuzione seriale l'ordine logico coincide
con quello di scansione. Il consumatore che riordina l'output dei rami
paralleli arriverà con lo scheduler.

## Memoria

Il `MemoryGovernor` tiene il budget di ammissione del piano. La quota si
prende con un **permesso atomico** — verifica e prenotazione in una sola
operazione — e viaggia col batch come `MemoryLease`.

Al fan-out il lease è **condiviso**, reference-counted, mai duplicato: il
batch resta contabilizzato una volta fino al rilascio dell'ultimo riferimento.
Il conteggio è per batch e per buffer, **mai per riga**.

I lease sono osservabili, perché un riferimento trattenuto è quota occupata e
dev'essere diagnosticabile: età, nodo proprietario, numero di riferimenti,
byte trattenuti, lease più vecchio, lease vivi durante la cancellazione.

La contabilità sta sotto un **lock unico**, con aritmetica controllata: lo
snapshot è linearizzabile — chi legge vede uno stato realmente esistito, non
un miscuglio di istanti — e una corruzione è visibile in
`MemoryMetrics.accounting_corrupted` invece di restare un'invariante rotta in
silenzio.

Per i segmenti **row-diagnostics** gli output accettati restano in memoria con
lease vivo, con ripiego deterministico allo staging IPC su disco quando il
budget non basta.

Che cosa questo budget **non** garantisce è in
[`errori-e-limiti.md`](errori-e-limiti.md), ed è la parte da leggere prima di
dimensionare una macchina.

## Confine Arrow IPC

`plenora_engine::ipc_boundary` è l'**unico** ingresso di lettura Arrow. Sniffa
il formato dal magic (`ARROW1` per il file format, altrimenti stream), applica
`IpcLimits` — tetti su byte del body, numero di messaggi, dimensione dei
metadati — **prima** delle allocazioni, e rifiuta i body compressi.

Ha una barriera **anti-panico**: `catch_unwind` sullo schema e su ogni batch.
Un panico di una dipendenza — per esempio nella conversione dello schema
FlatBuffers — diventa un errore tipizzato e sanitizzato invece di abbattere il
processo.

La scrittura passa dal publish atomico: tempfile più rename no-clobber.

## Geometrie

Il modello è **dimensionale**: XY, XYZ, XYM, XYZM sono distinti nel contratto,
non un dettaglio del payload.

Il decoder WKB è **validante in una sola scansione**: la stessa garanzia senza
pagarla due volte. Sui percorsi dove la validazione OGC è già dimostrata i
kernel usano varianti `*_validated`, che non la ripetono — la ripetizione non
aggiungerebbe garanzia, aggiungerebbe solo costo.

I nodi geo **binari** (`geo.sjoin`, predicati, overlay) sono segmenti
`BinaryBlocking` con validazione OGC totale e ordine canonico dell'output.

Il CRS è parte del contratto, con tre stati distinti: risolto, dichiarato ma
non risolto, assente. Il piano può risolvere esplicitamente uno stato
`DeclaredUnresolved` con `crs_decisions`.

## Row diagnostics

Alcune operazioni possono fallire **su singole righe** senza che l'intera
esecuzione fallisca: cast di tipo, formule, espressioni, assert, funzioni di
data, `flatten_json`. Il catalogo dichiara quali (`emits_row_diagnostics`), e
i segmenti che le contengono passano da un percorso dedicato che raccoglie la
diagnostica per riga con l'indice della riga sorgente.

Il planner ha un **gate di provenance**: un'operazione diagnostica messa dopo
un nodo che cambia la cardinalità è rifiutata in **validazione**, perché
l'indice della riga sorgente non sarebbe più attribuibile. Il rifiuto arriva
prima dell'esecuzione, non a metà.

Sul percorso lineare le row diagnostics non esistono: quelle operazioni
richiedono un piano DAG.

## Backend

`geos-backend` e `proj-backend` sono **feature**. Senza di esse il
comportamento è fail-closed e dichiarato: un piano che richiede la
riproiezione fallisce in validazione con `CRS_BACKEND_UNAVAILABLE`, invece di
produrre un risultato approssimato.

`capabilities` riporta che cosa questa build offre davvero.

## Osservabilità

`run` riporta metriche per nodo e per segmento: righe in ingresso e in uscita,
batch, tempo. Le metriche di memoria riportano riservato, picco, lease vivi,
età del più vecchio e lo stato di integrità della contabilità.
