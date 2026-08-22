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
le stesse geometrie.

Il confronto **seriale contro parallelo** che dimostrerebbe questa garanzia
nel caso interessante **non è eseguito**, e non può esserlo: lo scheduler
parallelo non esiste ancora. Oggi il livello 1 è verificato su esecuzione
seriale — determinismo fra esecuzioni ripetute, e il property test sui kernel
che al loro interno usano Rayon. Il property test «stesso piano, schedule
forzato seriale contro parallelo» è un **criterio di accettazione di M3**, non
una garanzia già dimostrata: vedi [`stato-e-roadmap.md`](stato-e-roadmap.md).

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

## Riferimenti normativi esterni

Il codice cita due sistemi di identificatori che **non sono definiti in questo
repository**, e la distinzione conta perché si assomigliano.

### `R…` — requisiti dell'ICD

Circa **730 citazioni** della forma `R2.4`, `R4.6.3`, `R6`, `R9.1`, `R13.2`.
Sono requisiti dell'**Interface Control Document** della famiglia Plenora, la
cui fonte normativa è il repository `plenora-contracts` al tag **`v2.0-rc10`**,
revisione **`3598259`**.

Quella fonte è **congelata**: il repository è stato sostituito il 2026-08-18 e
il suo contenuto attuale è una linea normativa **nuova**, che non descrive i
requisiti citati qui. Non va clonato, e non va usato per reinterpretarli.

Le citazioni restano quindi **riferimenti storici a una fonte esterna
congelata**: dicono a quale requisito, in quale revisione, una scelta risponde.
Non sono puntatori a un documento di questo repository, e non vanno né
tradotti né cancellati meccanicamente — cancellarli perderebbe l'unica traccia
del perché una regola ha la forma che ha.

Quando la nuova linea normativa sarà ratificata servirà una traduzione
esplicita, requisito per requisito, con la sua tabella di corrispondenza. È
lavoro dichiarato, non ancora fatto.

### `D…` — decisioni di questo repository

Sono invece **interne**, e definite qui sotto. Un `D…` citato dal codice deve
essere risolvibile nel registro, e un gate lo verifica.

## Decisioni

Le decisioni ancora in vigore, con l'identificatore che il codice cita. Sono
**riferimenti stabili**: un commento che scrive `D16` deve poterlo risolvere
qui, e un gate lo verifica (`scripts/verifica_documentazione.py`). Le
decisioni superate non compaiono: questo registro non è un archivio, contiene
solo ciò che vale oggi.

### Fondamenta

| id | decisione |
|---|---|
| **D0** | Un solo punto di versione per le dipendenze: Arrow è pinnato a `=59.1.0` nel manifesto del workspace, e nessun crate lo ridichiara. |
| **D6** | Il `DataContract` è l'unità di contratto fra i nodi: schema Arrow, geometrie, proprietà dichiarate. Ogni operazione lo inferisce **a secco** con `analyze_contract`, senza leggere dati. |
| **D8** | Validazione statica e dinamica sono distinte. La fase 1 legge solo header e metadati e **non può** verificare il contenuto delle celle: limiti, struttura del grafo, config, schema e metadati geo sono statici; la validità strutturale del WKB per cella è dinamica, in lettura. |
| **D9** | Materializzazione ai fan-out e fan-in: strategia conservativa della v1. Il fan-out resta una proprietà logica del DAG, e alternative fisiche — rilettura di sorgenti seekable, spill condiviso — restano possibili senza cambiare la semantica. |
| **D16** | Al massimo **una** colonna geometrica attiva per input, e `FieldId` in un namespace globale del grafo: il planner rimappa gli id provvisori degli input, così due geometrie omonime di sorgenti diverse non si confondono. |
| **D17** | Versioni **esplicite per componente** (`semantic_version`, `config_schema_version`, `contract_analysis_version`, `kernel_version`): ogni modifica incompatibile incrementa la versione pertinente, e il fingerprint del catalogo deriva da queste, mai da un hash del binario. |
| **D19** | `Limits` in tre famiglie semanticamente distinte — righe, memoria e disco, piano — e i `PlanLimits` applicati **durante** il parsing, prima di qualunque allocazione guidata dal contenuto. |
| **D20** | Alias legacy **versionati** e pipeline di migrazione **esplicita**: la risoluzione di un alias non dipende dai default correnti, e un piano storico arriva al canonico attraverso una conversione che dichiara che cosa sta traducendo. |
| **D22** | Publish atomico definito da una **matrice di supporto** e da due profili distinti: rename atomico e durabilità non sono la stessa cosa e non vanno promessi insieme. |
| **D25** | Il `DataContract` porta anche le proprietà dichiarate dell'insieme — ordinamento provato, conteggio righe, geometria attiva — con la loro confidenza, non solo lo schema. |

### Fusione dei segmenti geo

| id | decisione |
|---|---|
| **D12.1** | Forma decodificata **transiente**: gli archi restano `RecordBatch` con WKB canonico ISO XY, e nessun tipo nuovo compare sugli archi osservabili. Un decode e un encode per batch, non uno per operazione. |
| **D12.2** | La fondibilità è una **capability di catalogo** (`geo_fusion`: `NotFusible`, `TransformInPlace`, `TerminalMeasure`), e resta **fuori dal fingerprint**: è una proprietà fisica, e includerla invaliderebbe piani semanticamente identici. |
| **D12.3** | I limiti di cella sono **ri-applicati esatti**, con attribuzione al nodo: la dimensione del WKB XY di una geometria è funzione pura della struttura, quindi si calcola senza ri-serializzare. Nessuna deroga. |
| **D12.4** | Validazione **inter-passo** con attribuzione per profilo: fra un passo e il successivo del gruppo fuso la geometria è rivalidata, così un errore resta attribuibile al nodo che lo produce. |
| **D12.5** | La fusione è **solo fisica**: gli `analyze_*` girano in `validate` per ogni nodo, a secco, indipendentemente dalla fusione. Contratto, lineage e `FieldId` sono gli stessi con fusione accesa o spenta. |
| **D12.6** | Errori e metriche **per nodo** preservati: il runner fuso è un'esecuzione alternativa del gruppo, non una rimozione dei nodi. Righe in e out per nodo restano esatte. |
| **D12.7** | Memoria: **reservation esatta** dei byte decodificati prima del ciclo del gruppo, e fallback strumentato quando il budget non basta. È uno dei due punti in cui la prenotazione precede l'allocazione. |
| **D12.8** | `max_batch_bytes` non è applicabile sugli archi interni fusi, dove il batch non è materializzato: la protezione è spostata sul governor, non rimossa. Vedi [`errori-e-limiti.md`](errori-e-limiti.md). |
| **D12.9** | Kill switch `geo_fusion` (flag `--no-geo-fusion`), registrato nel piano: a `false` i gruppi non si formano e l'esecuzione è quella non fusa. Serve a isolare un sospetto senza ricompilare. |

### Operazioni geo binarie nel piano

| id | decisione |
|---|---|
| **D14.1** | Perimetro: `geo.sjoin`, `geo.nearest`, `geo.within`, `geo.count_points_in_polygons`. Criterio: **nessuna ri-encode**. |
| **D14.2** | `PreparedConfig::GeoBinary` con i kernel riusati e la CLI invariata: il piano guadagna le binarie senza che il percorso di trasporto cambi. |
| **D14.3** | Dimensione decodificata calcolata percorrendo la cella con la stessa camminata del decoder, senza materializzare. |
| **D14.4** | Contabilità **preflight** dei due lati: le binarie possono superare il budget prima di produrre, quindi la stima precede l'allocazione. |
| **D14.5** | Semantica degli errori: nessun messaggio di trasporto grezzo attraversa il confine, e «quale nodo ha rotto» resta rispondibile. La forma è cambiata rispetto alla decisione originale: l'errore **non** diventa `Execution`, perché così la categoria spariva — un `resource_limit` usciva come `execution`, exit 6 invece di 4. Oggi ogni categoria tranne `Execution` è **preservata** e il contesto del nodo si aggiunge con `Replayed`, che porta categoria e attribuzione insieme. |
| **D14.5.1** | Il contesto del nodo si aggiunge senza perdere la categoria: `step_error` produce `Replayed`, che porta categoria e attribuzione insieme. Solo `Execution` viene sostituita, perché è già la categoria generica. |
| **D14.5.2** | Nessun transito dell'errore di trasporto grezzo nel messaggio: un carrier dedicato porta sorgente, fase, lato e indice di riga in campi **strutturati**, non dentro una stringa da parsare. |
| **D14.5.3** | **Primo errore in ordine** `(lato, riga)`: il lato sinistro è decodificato interamente prima del destro, così l'errore riportato non dipende dallo scheduling. |
| **D14.5.4** | Fasi dichiarate: drenaggio e decode degli input sono `Read`, kernel e costruzione dell'output sono `Write`. La fase è dedotta dal punto del ciclo, non dal tipo di errore. |
| **D14.5.5** | Cancellazione `BoundaryOnly`: i confini sono i batch in ingresso e la fine del drenaggio, prima del kernel binario. Un kernel binario avviato non si interrompe a metà. |
| **D14.5.6** | `catch_unwind` sul kernel: un panic diventa errore di nodo sanitizzato, e **non si pubblica mai** dopo un panic. |
| **D14.6** | Limiti di espansione: vincolo relativo **più** un tetto assoluto, perché un vincolo solo relativo non limita un prodotto cartesiano. |
| **D14.7** | **Ordine canonico** delle coppie (left-major, right-minor), fissato dal porting se il kernel non lo garantisce: senza, il risultato dipenderebbe dallo scheduling. |
| **D14.8** | Lineage: passthrough del lato sinistro con le chiavi canoniche ereditate, colonne di indice e distanza derivate senza metadati ereditati per errore. |
| **D14.9** | Oracolo esteso agli errori: doppia esecuzione, piano contro percorso di trasporto, con confronto byte per byte. |
| **D14.10** | Benchmark A/B fra percorso standalone e piano, su fixture miste, con mediana di ripetizioni alternate. |

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
