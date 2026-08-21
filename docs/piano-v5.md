# Il piano v5

Un piano è un documento JSON dichiarativo: dice **che cosa** produrre, non
come. È l'unico ingresso di `validate` e `run`.

## Schema canonico

```json
{
  "schema_version": 5,
  "limits": { "max_rows_per_edge": 10000000, "max_governed_memory_bytes": 536870912 },
  "crs": "EPSG:32632",
  "inputs": ["citta", "fiumi"],
  "crs_decisions": { "fiumi": "EPSG:4326" },
  "nodes": [
    { "id": "a", "op": "table.filter",     "in": ["citta"],    "config": { } },
    { "id": "b", "op": "geo.buffer",       "in": ["fiumi"],    "config": { "distance": 100 } },
    { "id": "c", "op": "geo.sjoin",        "in": ["a", "b"],   "config": { } },
    { "id": "d", "op": "table.aggregate",  "in": ["c"],        "config": { } }
  ],
  "output": "d"
}
```

Regole strutturali, tutte fail-closed:

- campi sconosciuti **rifiutati a ogni livello** (`deny_unknown_fields`);
- grafo **aciclico**, un solo nodo di output, ogni `in` riferisce un nodo o un
  input dichiarato;
- ogni nodo dev'essere antenato dell'output (niente nodi morti) e ogni input
  dichiarato dev'essere referenziato (niente input morti), a meno che l'input
  non sia esso stesso l'output;
- **nessuna annotazione di esecuzione** nei nodi: il piano dichiara solo
  dipendenze e configurazioni. Come vengano eseguite è dell'executor;
- `max_parallelism` sta in `limits` — è una risorsa — non nei nodi;
- `config` omessa o `null` equivale a `{}`.

Per le operazioni **binarie ordinate** l'ordine di `in` è semantico:
`["left", "right"]`.

## Ordine di validazione

L'ingresso è fail-closed, e l'ordine è parte del contratto:

1. **tetto sui byte** del testo fornito (`max_plan_json_bytes`), prima di
   costruire qualunque albero JSON;
2. **chiavi duplicate** rifiutate — mai «vince l'ultima»: due testi diversi
   non devono poter produrre lo stesso piano canonico, e la risoluzione
   avverrebbe prima della validazione;
3. lettura della `schema_version` — dopo il punto 2, altrimenti la scelta del
   percorso dipenderebbe da quale duplicato ha vinto;
4. **migrazione** se il piano dichiara la v4;
5. **ricontrollo del tetto** sul testo migrato;
6. parse, validazione strutturale, risoluzione degli alias verso gli id
   canonici.

I limiti del piano (`PlanLimits`: byte del JSON, numero di nodi, archi,
profondità, fan-out, input, byte di config per nodo, byte degli
identificatori) si applicano **durante** il parsing, prima di qualsiasi
allocazione guidata dal contenuto. Un piano ostile consuma risorse già in
parse.

## Migrazione dalla v4

La versione canonica è la **5**. Un piano `schema_version: 4` è accettato, ma
**solo** attraverso la migrazione esplicita, che traduce il nome del budget di
memoria: `max_memory_bytes` (v4) → `max_governed_memory_bytes` (v5).

**Non c'è alias**, e il rifiuto è **simmetrico**:

| il piano dichiara | e scrive | esito |
|---|---|---|
| `schema_version: 5` | `max_governed_memory_bytes` | accettato |
| `schema_version: 5` | `max_memory_bytes` | **rifiutato** |
| `schema_version: 4` | `max_memory_bytes` | accettato, migrato |
| `schema_version: 4` | `max_governed_memory_bytes` | **rifiutato** |
| qualunque | entrambe le chiavi | **rifiutato** |

È questa simmetria a distinguere una **traduzione** da un alias: un alias fa
funzionare il nome vecchio ovunque, una traduzione lo fa funzionare in un
formato solo — il suo — e lo converte all'ingresso.

La proprietà non è garantita da un controllo, che si può dimenticare, ma da
**strutture separate** con `deny_unknown_fields`: quella della v5 conosce solo
il nome nuovo, quella della v4 — privata del modulo di migrazione — solo il
vecchio. La traduzione fra le due è una costruzione **per campi**: se qualcuno
aggiunge un limite alla v5 e dimentica la migrazione, il codice non compila.

Che cosa la migrazione promette: **equivalenza canonica**, non testuale. Il
piano attraversa un `serde_json::Value`, quindi ordine delle chiavi,
spaziatura e forma dei letterali numerici possono cambiare. Ciò che è
garantito è che un v4 e il v5 equivalente producano lo **stesso piano
canonico** e lo **stesso `plan_hash`**.

L'ingresso di versione è idempotente anche nell'**esito**: se la prima
passata riesce, la seconda riesce. Per questo il tetto sui byte si applica
anche al migrato — il nome della v5 è più lungo di nove byte, e senza quel
controllo un piano al limite esatto sarebbe accettato una volta e rifiutato la
successiva.

Un `plan_hash` calcolato prima della v5 appartiene a un **dominio precedente**
ed è da considerarsi invalidato: vedi *Identità e fingerprint*.

## Compatibilità con il formato lineare

Il formato **lineare** (`Plan { steps }`, `schema_version <= 3`) resta
supportato ed è **invariato**. È un formato pubblicato, distinguibile dagli
altri proprio dalla sua `schema_version`, e un piano già scritto non cambia
perché è cambiato il nome del campo nella libreria: sul filo continua a
scrivere `max_memory_bytes`, tradotto all'ingresso da una struttura privata.
Anche qui il rifiuto è simmetrico — un piano lineare che scrive
`max_governed_memory_bytes` è rifiutato.

Un piano lineare è il caso degenerato del DAG: ogni nodo ha un solo `in`, il
precedente. La conversione verso il DAG è esplicita e **arriva direttamente al
canonico v5**: non esiste una forma intermedia da cui ripartire.

Sul percorso lineare non esistono le row diagnostics: un'operazione che le
emette richiede un piano DAG e viene rifiutata prima dell'esecuzione.

## Contratti di input

Ogni input porta un **contratto**: schema Arrow, proprietà dichiarate e, se
c'è una colonna geometrica, il contratto geometrico.

- la geometria è riconosciuta dai metadati GeoArrow
  (`ARROW:extension:name = geoarrow.wkb` più il metadato `geo` con la chiave
  `crs`) o dalle chiavi canoniche;
- lo **stato del CRS** è `Resolved` se esiste una sola rappresentazione,
  `DeclaredUnresolved` se dichiarato o in conflitto decidibile, `Missing` se
  assente. `crs_decisions` nel piano risolve esplicitamente uno stato
  `DeclaredUnresolved`;
- metadati incoerenti sono rifiutati: estensione senza `geo.crs`, `geo` senza
  estensione, colonna non `Binary`, più di una colonna geometrica;
- il modello geometrico è **dimensionale**: XY, XYZ, XYM, XYZM sono distinti
  nel contratto, non un dettaglio del payload;
- una chiave canonica **presente si valida sempre**: «illeggibile» non è
  «assente».

Il `FieldId` della geometria di input è provvisorio: il planner lo rimappa nel
namespace del grafo. Le chiavi `sorted_by` nel namespace del chiamante sono
rifiutate fail-closed.

`validate` non esegue il piano: controlla struttura, contratti, CRS,
capability e limiti, e restituisce il riepilogo con nodi, archi, ordine
topologico, segmenti, capability richieste, fingerprint degli input e
`plan_hash`. Un piano semanticamente valido ma fuori dal dispatch corrente
fallisce **qui**, non a metà esecuzione.

## Identità e fingerprint

Un piano validato ha un'identità stabile, pensata perché un grafo possa
essere conservato e riusato.

```
plan_hash = SHA256("plenora/plan_hash/v5\0" ‖ canonical_json)
```

La forma **canonica** su cui si calcola: chiavi ordinate, nodi in ordine
topologico deterministico, alias sostituiti dagli id canonici, numeri
normalizzati (`100` ≡ `100.0`, ma `-0.0` **non** è `0`), default noti
materializzati (limiti effettivi, config omessa ≡ `{}`). Due piani
semanticamente equivalenti hanno lo stesso hash.

Il prefisso è un **separatore di dominio** che nomina la versione del formato
canonico. Rende disgiunti gli **input** della funzione di hash: nessun testo
canonico prodotto sotto la regola nuova coincide con uno prodotto sotto quella
vecchia. Da input disgiunti non segue che i digest lo siano — un `plan_hash`
uguale fra i due domini richiederebbe una **collisione SHA-256**, che era già
l'assunzione di prima. Cambiare il formato canonico significa cambiare il
dominio, e un test lo verifica.

L'identità completa comprende anche:

- **`catalog_fingerprint`**: SHA-256 dei descrittori delle sole operazioni
  usate, con le versioni esplicite per componente (`semantic_version`,
  `config_schema_version`, `contract_analysis_version`, `kernel_version`),
  capability, classe di esecuzione e determinismo. Mai un hash del binario,
  che sarebbe instabile fra compilatori e piattaforme; mai i soli id, che
  sarebbero ciechi ai cambi di comportamento. La `geo_fusion` resta
  deliberatamente **fuori**: è una capability fisica, e includerla
  invaliderebbe piani semanticamente identici;
- **`ContractFingerprint`** per ogni input;
- `plan_format_version`, versione dell'engine, versione di Arrow, capability
  richieste.

`check_compatibility` rifiuta con `GRAPH_MISMATCH` al primo scostamento, e
controlla **per primo** `plan_format_version`: un grafo validato sotto un'altra
versione del formato verrebbe interpretato con regole che non sono le sue.
Seguono engine, Arrow, catalogo e capability.

## Limiti dichiarabili nel piano

Il blocco `limits` restringe i default; ciò che non dichiara resta al default
della libreria.

| gruppo | campi |
|---|---|
| righe | `max_input_rows`, `max_output_rows`, `max_rows_per_edge`, `max_expansion_factor` |
| memoria | `max_governed_memory_bytes`, `max_temp_bytes`, `spill_partitions` |
| esecuzione | `max_parallelism` |
| geometria | `max_wkb_cell_bytes`, `max_payload_bytes`, `max_batches`, `max_geometry_depth` |
| stringhe | `max_string_bytes`, `max_regex_bytes` |
| piano | sotto-oggetto `plan`: `max_plan_json_bytes`, `max_plan_nodes`, `max_plan_edges`, `max_plan_depth`, `max_fan_out`, `max_inputs`, `max_config_bytes_per_node`, `max_identifier_bytes` |

I limiti effettivi sono validati **in un punto solo**, prima di qualunque
decisione, così li attraversano tutti i piani — compresi quelli solo-geo, che
non passano dal preparer tabellare.

Che cosa `max_governed_memory_bytes` garantisce davvero è in
[`errori-e-limiti.md`](errori-e-limiti.md): non è un tetto duro, e il
documento dice esattamente dove la garanzia si ferma.
