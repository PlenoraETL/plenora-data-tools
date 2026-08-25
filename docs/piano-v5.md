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

I limiti del piano (`PlanLimits`) si applicano in due momenti diversi, e la
differenza conta:

- `max_plan_json_bytes` è verificato sul **testo**, prima di costruire
  qualunque albero JSON. È l'unico che limita davvero le allocazioni di
  parsing, ed è per questo che è anche l'unico che non può provenire dal
  documento;
- numero di nodi, archi, profondità, fan-out, input, byte di config per nodo e
  byte degli identificatori sono verificati sull'oggetto **già
  deserializzato**, subito dopo il parse e prima di qualunque lavoro
  successivo.

Un piano ostile è quindi limitato in parse dal solo tetto sui byte: chi vuole
un tetto stretto sulle allocazioni deve abbassare quello. La limitazione è
registrata per esteso, con hazard e condizione di rientro, in
[`errori-e-limiti.md`](errori-e-limiti.md).

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

## Alias legacy

Gli id storici dei due progetti di origine restano accettati come **alias**
degli id canonici del catalogo unificato. La tabella vive in
`plenora-core/src/catalog.rs` (`ALIASES`) e ha forma `(schema_version, alias,
id canonico)`: 127 voci, tutte con `schema_version` 3, che copre sia i piani
lineari nogeo sia gli id storici del protocollo geo (v2/v3,
`TransformArrowSchema`).

Le regole di mapping sono quattro:

| origine | regola | esempio |
|---|---|---|
| tabellari | id storico invariato sotto il namespace `table.` | `filter` → `table.filter` |
| geografiche `geo_*` | il prefisso storico diventa il namespace | `geo_buffer` → `geo.buffer` |
| predicati DE-9IM | `predicate_*` sotto `geo.` | `predicate_contains` → `geo.predicate_contains` |
| estensioni geo senza prefisso | `<id>` sotto `geo.` | `geodesic_area` → `geo.geodesic_area` |

La risoluzione è **esatta e versionata**: `resolve_alias` cerca la coppia
(`schema_version`, alias), non il solo nome, così un piano non cambia
significato perché è cambiata la versione di default. `find_operation`
accetta id canonici e alias; quando la versione non è nota al chiamante usa
la prima voce corrispondente.

**Un alias pubblicato non si riassegna.** La tabella è immutabile per le
versioni già rilasciate: aggiungere una voce è consentito, cambiare la
destinazione di una voce esistente no — sarebbe lo stesso piano, con lo
stesso testo, che ieri faceva una cosa e oggi ne fa un'altra. Due test
bloccanti tengono la tabella onesta: ogni alias risolve a un id esistente del
catalogo, e nessun alias collide con l'id canonico di un'altra famiglia.

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

<a id="identita-e-fingerprint"></a>

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

Il blocco `limits` **dichiara** i limiti di questa esecuzione; ciò che non
dichiara resta al default della libreria.

| gruppo | campi |
|---|---|
| righe | `max_input_rows`, `max_output_rows`, `max_rows_per_edge`, `max_expansion_factor` |
| memoria | `max_governed_memory_bytes`, `max_temp_bytes`, `spill_partitions` |
| esecuzione | `max_parallelism` |
| geometria | `max_wkb_cell_bytes`, `max_payload_bytes`, `max_batches`, `max_geometry_depth` |
| stringhe | `max_string_bytes`, `max_regex_bytes` |
| piano | sotto-oggetto `plan`: `max_plan_json_bytes`, `max_plan_nodes`, `max_plan_edges`, `max_plan_depth`, `max_fan_out`, `max_inputs`, `max_config_bytes_per_node`, `max_identifier_bytes` |

### I default che si applicano quando il piano tace

Un campo omesso non è un campo senza valore: il piano gira sotto il default, e
chi invia deve poterlo sapere **prima** di inviare. I tre del gruppo memoria:

| campo | default | costante |
|---|---|---|
| `max_governed_memory_bytes` | `536870912` byte (512 MiB) | `DEFAULT_MAX_GOVERNED_MEMORY_BYTES` |
| `max_temp_bytes` | `8589934592` byte (8 GiB) | `DEFAULT_MAX_TEMP_BYTES` |
| `spill_partitions` | `64` | `DEFAULT_SPILL_PARTITIONS` |

Nel codice il numero è scritto **una volta sola**: sono costanti di
`plenora_core::limits` — `DEFAULT_MAX_GOVERNED_MEMORY_BYTES` è riesportata
anche dalla radice, perché il contratto la richiede pubblicamente — e i
default di `Limits` vengono da lì, in entrambi i crate.

Questa tabella però è prosa, e **nessun gate la confronta con le costanti**:
i numeri qui sopra sono trascritti a mano e possono divergere. Ciò che li
tiene onesti è che i test fissano il valore pubblicato di ciascuna costante,
quindi cambiarlo senza accorgersene non si può — ma aggiornare la costante e
dimenticare questa riga sì. Chi tocca un default passi anche di qua.

Pubblicare `max_governed_memory_bytes` non è una cortesia. `Plan Budget 1.0`
lo **richiede** (`PLAN-013`): il tetto del dominio di un piano v6 deve essere
maggiore o uguale al budget governato *effettivo*, che è quello dichiarato
oppure — quando il piano lo omette — questo default. Senza il numero pubblico
quel vincolo non è verificabile prima dell'invio, e si scoprirebbe solo al
rifiuto.

I due gruppi non hanno lo stesso statuto, e confonderli è stato un difetto
reale:

**Limiti dati/runtime** — righe, memoria, geometria, stringhe, esecuzione.
Sono **configurazione dell'esecuzione**, e un piano può dichiararli sopra o
sotto il default: è il modo previsto per dimensionare una corsa, e i test del
progetto lo usano in entrambi i versi. Il default non è un tetto imposto da
qualcun altro, è il valore che vale quando il piano tace. Ciò che li chiude è
la validazione dei limiti effettivi. **Non esiste oggi una policy dell'host
che il piano non possa superare**: chi accetta piani non fidati deve saperlo,
ed è registrato in [`errori-e-limiti.md`](errori-e-limiti.md).

**Limiti di piano** (sotto-oggetto `plan`) — sono invece una **policy vera**:
`PlanV5::parse` li riceve come argomento da chi esegue il parse, e sono il
costo massimo che quel chiamante accetta di pagare per interpretare un
documento. Un piano può solo **restringerli**: un valore che supera quello del
chiamante è rifiutato, nominando il campo, il valore chiesto e quello
consentito. Un documento che alzasse il tetto di ciò che costa leggerlo
deciderebbe da sé quanto può costare.

`spill_partitions` merita una riga a parte: non è un tetto di risorsa ma una
configurazione dello spill — più partizioni significano file più piccoli e più
descrittori, non più consumo consentito. Il dominio ammesso (`2..=4096`) resta
chiuso dalla validazione dei limiti effettivi.

La sotto-sezione `plan` governa **il piano che la dichiara**: i conteggi
strutturali — nodi, archi, profondità, fan-out, input, byte di config,
byte degli identificatori — sono applicati con i limiti così ristretti. Prima
finiva nella forma canonica, e quindi nel `plan_hash`, senza che nulla la
applicasse: l'identità del piano affermava una proprietà che il parser non
aveva verificato.

`max_plan_json_bytes` è l'**unica eccezione**, e per una ragione strutturale:
un tetto sul testo va applicato prima di leggere il testo, quindi non può
venire dal testo. Resta quello di chi esegue. Il piano può dichiararlo — vale
la regola di sola restrizione, e finisce nella forma canonica — ma non governa
se stesso. Riapplicarlo dopo il parse sembrerebbe innocuo e non lo è: la forma
canonica materializza *tutti* i limiti effettivi, quindi è sempre più grande
del documento compatto che l'ha prodotta, e un piano che dichiarasse 300 byte
verrebbe accettato mentre la sua forma canonica — quella che porta il
`plan_hash` ed è pensata per essere conservata e riletta — non rientrerebbe
più.

La forma canonica materializza i limiti di piano contro i **default della
libreria**, mai contro la policy di chi esegue il parse. L'identità di un
piano è una proprietà del piano: se dipendesse dal chiamante, lo stesso
documento — con la stessa esecuzione — avrebbe due `plan_hash` diversi sotto
due policy diverse, e un grafo persistito non sarebbe più confrontabile con sé
stesso. Il `plan_hash` non cambia quindi per nessun piano che resti accettato.

Il prezzo, dichiarato per intero perché è più grande di quanto sembri: la
materializzazione scrive i default come **dichiarazioni esplicite del piano**,
e le dichiarazioni esplicite governano il piano che le contiene. Un piano
accettato **solo** grazie a una policy più larga del default ha quindi un
canonico che **non è rileggibile con nessuna policy** — nemmeno con la
stessa che lo aveva accettato, perché il canonico dichiara i default e li
viola.

La forma canonica è dunque, per quei piani, un **input di hash** e non un
documento riproponibile. Per tutti i piani che stanno dentro i default — cioè
tutto ciò che la CLI e `validate` producono — il canonico si rilegge senza
problemi, e un test lo verifica.

Chiuderla del tutto richiederebbe di materializzare nel canonico **solo ciò
che il piano dichiara**, omettendo i default: sarebbe l'unica forma coerente
su tutti gli assi, ma cambierebbe la forma canonica di ogni piano e quindi
ogni `plan_hash`, e va perciò accompagnata da un dominio nuovo. È una
decisione di rilascio, non una correzione, ed è aperta.

### Rottura di compatibilità: piani prima accettati, ora rifiutati

Rendere reali i limiti di piano ha un prezzo, e va detto invece che nascosto
dietro «gli hash non cambiano». **Alcuni piani che le versioni precedenti
accettavano sono ora rifiutati**, e il loro `plan_hash` non è più
rigenerabile:

- un piano che **dichiara un limite di piano più largo** di quello di chi
  esegue: prima l'override era ignorato, ora la regola di sola restrizione lo
  rifiuta;
- un piano che **dichiara un limite di piano che poi viola** — per esempio
  `max_plan_nodes: 1` con due nodi: prima la dichiarazione non governava
  nulla, ora sì;
- un piano i cui limiti effettivi portano a **zero** `max_plan_json_bytes`,
  `max_inputs` o `max_identifier_bytes`.

In tutti e tre i casi il piano affermava una proprietà che non rispettava, e
la sua identità certificava quella proprietà. Il rifiuto è il punto della
correzione, non un effetto collaterale.

Il bump di versione del rilascio è **maggiore** per ragioni già note (vedi
[`release.md`](release.md)); questa rottura vi rientra e non ne aggiunge di
nuove oltre a quelle dichiarate lì.

Resta una proprietà da conoscere: **il canonico può essere più grande del
documento che lo ha prodotto**, perché materializza tutti i limiti anche
quando il documento li ometteva. Non è sempre così — un documento
pretty-printed, o ricco di spaziatura, può essere più grande del canonico
compatto — ma **esistono tetti sui byte intermedi** fra la dimensione del
documento e quella del suo canonico, e con uno di quelli il chiamante accetta
il piano e rifiuta la sua forma canonica. È inerente alla materializzazione,
non un difetto del controllo.

Fin dove quei tetti proteggano davvero dalle allocazioni di parsing è detto in
[`errori-e-limiti.md`](errori-e-limiti.md).

I limiti effettivi sono validati **in un punto solo**, prima di qualunque
decisione, così li attraversano tutti i piani — compresi quelli solo-geo, che
non passano dal preparer tabellare. Un limite a zero è rifiutato lì, ma solo
dove **nessun documento valido** potrebbe rispettarlo: byte del JSON, numero
di input, byte degli identificatori. I tetti sui nodi — nodi, archi,
profondità, fan-out, byte di config — possono legittimamente valere zero: li
rispetta un piano **pass-through**, e una policy che ammette solo
pass-through è una policy sensata. Rifiutarli avrebbe reso il parse e la
validazione discordi sullo stesso documento.

Che cosa `max_governed_memory_bytes` garantisce davvero è in
[`errori-e-limiti.md`](errori-e-limiti.md): non è un tetto duro, e il
documento dice esattamente dove la garanzia si ferma.
