# Superficie API rotta dalla review statica del 2026-08-16

Inventario delle modifiche alla **API pubblica** introdotte dai cinque giri di
correzione della review statica. Serve al version bump: sono cambiamenti
semver-maggiori e vanno dichiarati insieme, non scoperti a valle.

Versione corrente al momento della stesura: `1.0.3`.

## Rotture (richiedono un bump maggiore)

### `plenora-engine`

- **`ExecutionMetrics`**: nuovo campo pubblico `counters_saturated: bool`, e
  la struct e' ora `#[non_exhaustive]`. Chi la costruisce con struct literal o
  la destruttura in modo esaustivo non compila piu'; da qui in avanti pero'
  l'aggiunta di una metrica non sara' piu' una rottura. Nessun sito nel
  workspace la costruisce.
- **`Inputs::add` / `Inputs::with`**: **deprecate** (`#[deprecated]`).
  Continuano a funzionare — un chiamante esistente compila, con un warning —
  ma il percorso senza contratto non e' piu' una forma sostenuta. Migrazione:
  `add_with_contract` / `with_contract`, preferibilmente su un insieme
  costruito con `Inputs::strict`. Nel profilo stretto `add` restituisce
  `Err`.
- **`NodeMetrics`, `SegmentMetrics`, `MemoryMetrics`**: ora
  `#[non_exhaustive]`, come `ExecutionMetrics`.
- **`Input::Batches`**: da variante tupla a variante struct
  (`Input::Batches { batches }`). Il pattern matching esterno va aggiornato.

### `plenora-kernels-table`

- **`SpillMetrics`**: nuovo campo pubblico `saturated: bool`, nuovo metodo
  `accumulate`, e la struct e' ora `#[non_exhaustive]`. E' ri-esportata anche
  da `plenora-engine`.
- **`NumericBound`**: nuove varianti `U64` e `Decimal { unscaled, scale }`. Un
  `match` esaustivo esterno non compila piu'. Cambia anche la FORMA del
  risultato di `NumericBound::parse` per i letterali posizionali: i decimali
  sono normalizzati (`"64.0"` → `unscaled: 64, scale: 0`, non `640/1`). Il
  valore rappresentato e' lo stesso.

### CLI

- **Flag sconosciuti rifiutati.** Prima venivano ignorati: `catalog
  --sconosciuto`, `capabilities --pippo`, `self-test --qualunque` uscivano con
  **successo** facendo il lavoro. Ora ogni sottocomando dichiara la propria
  superficie di flag e rifiuta quello che non conosce (`invalid_plan`, exit 2),
  elencando gli ammessi. Uno script che passava flag di troppo — per un
  refuso o per una versione futura — ora fallisce invece di fingere.
- **Flag a valore singolo non ripetibili.** `describe --input a --input b`
  usava `a` e scartava `b` in silenzio; ora e' un errore. Resta ripetibile
  `--input` su `run`/`validate`, dove ogni occorrenza e' un input nominato
  diverso.
- **Comando inesistente: niente help su stderr.** Prima l'help finiva su
  stderr insieme all'envelope su stdout — due canali per un errore solo. Ora
  l'envelope e' l'unico documento e il suo messaggio indica `--help`.
- **Canale dell'envelope di errore**: da **stderr** a **stdout**, con stderr
  lasciato vuoto. Chi oggi legge gli errori da stderr non li trova piu'.
  Inverte esplicitamente la decisione della review P1-5 del 2026-08-03, che
  aveva riportato l'envelope su stderr come contratto storico: quella scelta
  precede `plenora-database-tools`, che usa stdout, e due componenti della
  stessa famiglia non possono avere convenzioni opposte.
- **Exit code**: non piu' `2` per ogni errore. Ora sono una proiezione della
  categoria — 2, 3, 4, 5, 6, 70, piu' 130 per la cancellazione (invariato).
  Uno script che confronta con `2` va aggiornato. **Non sono allineati a
  `plenora-database-tools`**, che usa `1` per qualunque errore: e' una
  divergenza dichiarata (emendamento a ADR-0003). Chi scrive codice portabile
  fra i due componenti deve leggere `error.category` dall'envelope, non il
  codice numerico; l'unica garanzia condivisa e' «0 successo, non-zero
  errore».
- **`--version`** emette JSON per default (component, versione, Arrow,
  backend, operazioni). La forma testuale resta con `--format markdown`.
- **`run` / `validate` di un piano DAG v4 con piu' di un input**: la forma
  posizionale `--inputs` non e' piu' accettata e va sostituita con
  `--input NOME=PERCORSO`. E' una rottura di comportamento voluta: quella forma
  poteva pubblicare in silenzio un risultato calcolato sugli input scambiati.
  Con un input solo continua a funzionare.

### `plenora-core` (terzo giro di review)

- **`PlenoraError::ResourceLimit(String)`**: variante NUOVA. Un `match`
  esaustivo su `PlenoraError` fuori dal workspace non compila piu'. Produce
  categoria `resource_limit`, fase `write`, retry `never`.
- **Categoria degli errori di limite**: `max_rows` e `max_columns` dei join,
  quota `max_temp_bytes` dello spill, `max_expansion_factor`, limiti di righe
  e budget di memoria della CLI legacy passano da `invalid_plan`/exit 2 a
  **`resource_limit`/exit 4**. Chi filtrava su `invalid_plan` per intercettare
  i limiti va aggiornato. Restano `invalid_plan` i limiti di FORMA della
  configurazione.
- **`ArrowTransportError::IpcMetadataTooLarge`**: da un campo a due — il
  secondo e' il tetto EFFETTIVAMENTE applicato. Il messaggio riportava la
  costante di default anche quando il tetto derivava dal piano.
- **Testo dei panici**: gli errori generati da un panico intercettato non
  riportano piu' il messaggio del panico, che puo' contenere dati della riga.
  Riportano la forma del payload. Chi cercava il testo originale nel messaggio
  non lo trova piu'.

### `plenora-core`

- **`limits::expansion_exceeded`**: un fattore non finito o non positivo e'
  ora **fail-closed** (risponde «limite superato») invece di rispondere
  `false`. Una soglia che non si sa confrontare non autorizza piu' l'output.

### `plenora-kernels-table` (secondo giro di review)

- **`compare_f64` con estremo `Decimal`**: il confronto e' esatto e non passa
  piu' per `10^-scale`. L'esito cambia ai bordi, dove prima due valori
  distinti risultavano uguali — un filtro `>` che escludeva una riga ora la
  include, correttamente.
- **`exact_f64_from_decimal128`**: lo zero e' esatto a ogni scala, e i valori
  esatti con scala negativa non vengono piu' persi per il traboccamento del
  fattore.
- **`NumericBound::as_f64` rimossa** (era privata): nessun percorso converte
  piu' un estremo decimale a double.
- **Nuove**: `dictionary_utf8_value`, `is_logically_null` — il null logico
  delle `DictionaryArray` ha ora una sola definizione condivisa.
- **Comportamento**: una riga dictionary la cui chiave punta a una entry nulla
  e' ora trattata come NULL da filtri, ordinamento, set operation e profilo
  scalare. Prima era la stringa vuota: chi contava quelle righe come valori
  vedra' numeri diversi, ed e' la correzione.

### `plenora-core`

- **`FieldAllocator::{alloc, intern, derive}`**: restituiscono ora `Result`
  invece del valore. `alloc` non e' piu' `const fn`.
- **`DataContract::validate`** e **`Limits::validate`**: rifiutano input che
  prima passavano (chiavi canoniche malformate, limiti nulli o fuori dominio,
  `max_expansion_factor` non finito). Non e' una rottura di firma ma di
  COMPORTAMENTO: un chiamante che costruiva contratti o limiti degeneri ora
  riceve un errore.

### Comportamento osservabile (ottavo giro di review, 2026-08-17)

Nessuna firma cambia; cambiano **categoria e fase** di errori che i
consumatori possono aver classificato.

- **Limiti di risorsa alzati dentro un passo** (join oltre `max_rows`, quota
  di spill): la categoria non e' piu' `execution`/exit 6 ma
  `resource_limit`/exit 4. Nodo e operazione restano nella diagnostica.
- **Tetti del confine IPC** (`max_metadata_bytes`, `max_body_bytes`, numero di
  messaggi e di record batch, complessita' dello schema, dimensione dello
  stream, righe/colonne/batch/celle): da `data_mapping`/exit 3 a
  `resource_limit`/exit 4, fase `read`.
- **Tetti di risorsa del confine d'ingresso** (`max_input_rows`,
  `max_batches`, `max_payload_bytes`): fase da `validate` a `read`
  (ADR-0009, emendamento del 2026-08-17). La categoria era gia' cambiata nel
  terzo giro.
- **Lunghezza dichiarata non contenuta nella sorgente**: resta `data_mapping`
  ma cambia la VARIANTE — un payload minuscolo che dichiara metadati enormi
  esce ora come troncamento e non come tetto superato. La verifica di
  disponibilita' precede quella del tetto: nessuno supera un budget con byte
  che non esistono.
- **`--json`**: accettato solo da `--version`, che e' l'unico comando che lo
  dichiara. Prima passava inosservato altrove.
- **`--help` / `-h`**: sono la stessa opzione anche ai fini del controllo dei
  duplicati; `run --help -h` e' ora un errore di riga di comando.

### Comportamento osservabile (nono giro di review, 2026-08-17)

Nessuna firma cambia; cambiano **categoria** ed **exit code** di un insieme
ampio di errori. E' la rottura piu' estesa di questa serie e va letta prima
del bump.

- **Circa cento limiti di risorsa dei kernel** passano da `invalid_plan`/exit 2
  a `resource_limit`/exit 4. Il criterio che li separa dai limiti di
  configurazione — che restano `invalid_plan` — e' scritto in ADR-0009,
  emendamento del 2026-08-17 bis: decide la PROVENIENZA della quantita'
  misurata (dal piano, oppure dai dati). Famiglie toccate: governor della
  memoria, `concat_by_name`, `cross_join`, `fuzzy_join`, `union_distinct`,
  `melt`, `pivot`, `transpose`, `explode`, `unnest`, `table_diff`, tetti sulle
  stringhe, accounting di `governance` e di `spill`, conteggi di
  `aggregation`, tetti su righe e colonne del batch d'ingresso.
- **Percorso legacy** (`schema_version <= 3`): un limite di risorsa alzato da
  un kernel non viene piu' declassato a `execution`/exit 6. Prima lo stesso
  limite dava categorie diverse a seconda della versione dello schema del
  piano.
- **Fase della reservation del governor al confine d'ingresso**: `read`
  (tag esplicito), come i tetti su righe/batch/byte accanto. La reservation
  in esecuzione resta `write`, derivata dalla variante.
- **Percorso legacy, budget di memoria**: `max_memory_bytes` copre ora anche
  l'esecuzione e l'output, non solo il caricamento. Piani che prima
  riuscivano trattenendo piu' del dichiarato ora falliscono con
  `resource_limit`. Attenzione al livello della garanzia: per
  `cross_join`, `concat`, `concat_by_name` e `melt` il rifiuto e'
  **preventivo** (prima dell'allocazione); per le altre operazioni e' un
  **controllo di ammissione post-allocazione**, che impedisce di pubblicare
  un risultato fuori budget ma non l'allocazione che lo ha prodotto. Non e'
  un tetto duro sulla memoria del processo: vedi DER-011.
- **Pivot `count` e ordinamento**: una chiave dictionary valida che punta a
  una entry nulla e' una riga NULLA. `count` non la conta piu'; l'ordinamento
  la equipara agli altri null invece di ordinarla in base al modo in cui e'
  nulla. Conteggi e ordinamenti su colonne dictionary possono cambiare.

### Comportamento osservabile (settimo giro di review, 2026-08-17)

- **Tetti sulle colonne, ora preventivi**: `cross_join`/`join` e
  `concat_by_name` rifiutano prima di allocare, e `concat_by_name` li
  applicava *per niente*. Un piano che superava `max_columns` con
  `concat_by_name` prima riusciva; ora fallisce con `resource_limit`.
- **Stime del preflight per operazione**: piu' accurate e quindi piu'
  restrittive. Un `cross_join`, un `concat_by_name` con input dagli schemi
  disgiunti o un `melt` con nomi di colonna lunghi possono ora essere
  rifiutati con budget che prima passavano. E' una correzione, non un
  irrigidimento arbitrario: le stime precedenti erano sbagliate in difetto.
- **Lunghezza di una stringa della riga** (`string_length`, `length` nelle
  espressioni): da `invalid_plan`/2 a `resource_limit`/4.
- **Integrita' del file temporaneo di spill**: da `invalid_plan`/2 e
  `resource_limit`/4 a `internal`/70. Sono invarianti nostre, non budget del
  chiamante.
- **`Internal` sopravvive al contesto di passo**: un'invariante violata dentro
  un kernel esce ora come `internal`/70 su ENTRAMBI i percorsi, non piu' come
  `execution`/6.

### Comportamento osservabile (ottavo giro di review, 2026-08-17)

- **Categoria degli errori di passo — rottura ampia.** La propagazione non
  sostituisce piu' la categoria con `execution`: un errore che porta gia' una
  classificazione la conserva, e il contesto del passo si aggiunge tramite
  `Replayed`. La maggior parte degli errori di kernel esce quindi con la
  PROPRIA categoria ed exit code invece di `execution`/6. In particolare gli
  errori ritentabili (`io`, `timeout`, `transient`) mantengono la propria
  disposizione di ritentativo, che prima diventava `never`.
- **Panico dentro un kernel**: da `invalid_plan`/2 (nascosto sotto
  `execution`/6) a `internal`/70. Non e' un difetto del piano di chi chiama.
- **Batch senza colonne**: `concat`, `concat_by_name`, `cross_join`, `join`,
  le set operation, `select_rows`, `select_columns`, `align_schema`,
  `rename` e `reorder` restituivano ZERO righe su input a zero colonne.
  Ora restituiscono la cardinalita' corretta. Chi dipendeva del vecchio
  comportamento dipendeva da un difetto.
- **`melt` con `type_policy = "string"`**: il preflight stima ora la
  larghezza TESTUALE dei valori convertiti, quindi puo' rifiutare con budget
  che prima passavano. Correzione di una sottostima, non un irrigidimento.
- **Stime del preflight**: aritmetica controllata. Un traboccamento della
  stima e' ora un `resource_limit` invece di un'autorizzazione implicita.

### Comportamento osservabile (nono giro di review, 2026-08-17)

- **Batch senza colonne, percorsi dell'engine**: la pubblicazione con
  rivestimento dello schema, la compattazione dello staging e la
  normalizzazione `LargeUtf8` FALLIVANO su un batch a zero colonne. Ora
  conservano la cardinalita'. (Il giro precedente aveva chiuso solo i kernel
  tabellari; e il sintomo era un errore, non un conteggio a zero.)
- **`melt` con `type_policy = "string"` su colonne `Dictionary`**: il
  preflight stima ora i byte DEREFERENZIATI — la voce piu' lunga del
  dizionario per ogni riga — quindi puo' rifiutare con budget che prima
  passavano. Correzione di una sottostima di ordini di grandezza.
- **`melt` con un tipo valore non convertibile in testo**: rifiutato PRIMA di
  allocare, con un errore che nomina colonna e tipo, invece che a meta'
  scansione.
- **Diagnostica opt-in (`[batch_seq=N]`)**: ricompare negli errori di
  categoria `execution` e `cancelled`, dove veniva cancellata dalla
  rigenerazione del messaggio.

### Comportamento osservabile (decimo giro di review, 2026-08-18)

- **`melt` con `type_policy = "string"`** su una colonna `Timestamp` non in
  millisecondi o `Decimal128` a scala negativa: il rifiuto arriva ora dalla
  PREVALIDAZIONE, prima di allocare, con il messaggio «non convertibile in
  testo» invece di «tipo non supportato dal profilo scalare» a meta'
  scansione. L'esito resta un errore in entrambi i casi: cambia il momento e
  il testo.

### Comportamento osservabile (undicesimo giro di review, 2026-08-18)

- **Nomi di output di `melt`**: risolti in sequenza. Una configurazione in cui
  i due nomi collidevano DOPO la risoluzione — per esempio `var_name = "v"` e
  `value_name = "v_1"` su un input che contiene `v` — produceva due colonne
  omonime e ora produce nomi distinti (`v_1`, `v_1_1`). Chi dipendeva dallo
  schema precedente dipendeva da un contratto rotto.
- **`melt` con `type_policy = "reject"` e colonne eterogenee**: il rifiuto
  arriva ora in preparazione, quindi resta `invalid_plan` anche con budget
  stretti, dove prima poteva uscire prima un `resource_limit`.
- **`melt` con una timezone Arrow non valida**: rifiutato dallo schema, prima
  di allocare, invece che a meta' scansione.

### Comportamento osservabile (dodicesimo giro di review, 2026-08-18)

- **Analisi del contratto di `table.melt`**: con `type_policy = "string"` e
  colonne di valore eterogenee, `plenora_kernels_table::analyze::analyze_table_contract` (e quindi la
  validazione del piano, `plenora validate`) applica ora la stessa
  prevalidazione del kernel. Un piano con una value column non convertibile in
  testo — unita' temporale diversa da `Millisecond`, timezone non risolvibile,
  `Decimal128` a scala fuori da `0..=38` — riceve ora un errore di categoria
  `schema` in VALIDAZIONE, dove prima riceveva un contratto valido con la
  colonna dichiarata `Utf8` e falliva in esecuzione. Chi si aspettava che
  `validate` passasse su questi piani vedra' ora un rifiuto: e' il piano che
  non era eseguibile, non la validazione che e' diventata piu' severa del
  necessario.
- **`resolve_output_names` e i nomi generati**: il nome prodotto dal suffisso
  viene ora validato come quello richiesto. Un `var_name`/`value_name` lungo
  1024 byte (il massimo ammesso) che collide non produce piu' `nome_1` di 1026
  byte: la risoluzione fallisce con `invalid_plan`. Il caso richiede un nome
  al limite esatto, quindi la superficie toccata e' stretta, ma il nome che
  usciva prima violava il vincolo che `validate_output_name` esiste per
  imporre.
- **Numero di suffissi**: il messaggio d'errore dichiarava cento suffissi, il
  codice ne provava novantanove. Il numero e' ora la costante pubblica
  `plenora_kernels_table::MAX_SUFFISSI_COLLISIONE` (99), e il messaggio la
  riporta. Il comportamento non cambia: cambia il testo, che prima era falso.

### Comportamento osservabile (tredicesimo giro di review, 2026-08-18)

L'analizzatore di `table.formula` e `table.expression` replicava a mano il
sistema di tipi dell'interprete, e le due copie erano scivolate. Ora
l'analisi rifiuta cio' che il runtime non puo' eseguire. Tutti i piani
elencati qui sotto **fallivano gia'** in esecuzione: cambia il momento della
scoperta, non l'esito.

- **`table.formula`**: una colonna con timezone Arrow non risolvibile e'
  rifiutata in validazione. Il kernel legge come testo ogni colonna che non
  sia `Int64`/`Float64`, quindi passa da `scalar_as_string`, che risolve la
  timezone a ogni riga.
- **`table.expression`, colonne `Timestamp`**: solo i millisecondi sono un
  numero. `Timestamp(Second)`, `(Microsecond)`, `(Nanosecond)` sono ora
  rifiutati: `scalar::column` li manda al percorso numerico e li' non esiste
  conversione.
- **`table.expression`, `output_type` dichiarato**: l'AST viene **sempre**
  analizzato. Prima con un `output_type` esplicito l'analisi non guardava
  affatto l'espressione, quindi una colonna inesistente o un operando di tipo
  sbagliato ottenevano un contratto valido. Inoltre il tipo dichiarato deve
  essere quello che l'espressione produce: il kernel non converte. Casi ora
  rifiutati che prima passavano la validazione: colonna `Utf8` dichiarata
  `boolean`, colonna `Boolean` dichiarata `number`, colonna `Date32`
  dichiarata `date32` (solo `date_trunc` produce una data nativa).
- **`year`**: richiede un argomento **testuale**. L'analisi pretendeva un
  numero, il runtime fa il parsing di una stringa come `%Y-%m-%d`: le due
  richieste erano l'una il contrario dell'altra, e `year` su una colonna
  temporale — che l'analisi accettava — falliva sempre.
- **`concat`**: tutti gli argomenti devono essere testuali (`text()` non
  converte un numero).
- **`case`**: la condizione `when` deve essere booleana (`boolean()` non
  converte).
- **`in`**: la lista di letterali deve avere il tipo del valore confrontato
  (`compare` rifiuta i tipi incompatibili).
- **Nodi che confrontano e `output_type` dichiarato**: `equal`…`less_equal`,
  `between`, `null_if`, `greatest`/`least` e `in` richiedono operandi omogenei
  **anche** con un tipo di output dichiarato. Resta invece ammessa
  l'eterogeneita' di `coalesce` e `case`, dove il runtime decide riga per
  riga e la riuscita dipende dai dati.

### Comportamento osservabile (quattordicesimo giro di review, 2026-08-18)

L'analisi di `table.expression` tiene ora l'**insieme** dei tipi che
l'espressione puo' produrre, invece di un tipo singolo con uno stato `Any` che
significava sia «solo null» sia «eterogeneo». Conseguenze osservabili, tutte
su piani che il runtime non poteva eseguire:

- **Confronti con eterogeneita' annidata**: `equal(coalesce(a, b), c)` con `a`
  e `b` di tipo diverso e' rifiutato anche con `output_type` dichiarato. Prima
  il `coalesce` collassava su `Any` e il confronto lo assorbiva; a runtime
  `compare` riceveva tipi diversi e falliva. Vale per `equal`…`less_equal`,
  `between`, `null_if`, `greatest`/`least` e `in`.
- **`coalesce`/`case` con tre o piu' tipi**: l'esito non dipende piu'
  dall'ORDINE dei rami. Prima `coalesce(numero, testo, booleano)` collassava a
  `Any` e poi assumeva il tipo dell'ultimo ramo, quindi lo stesso piano scritto
  in ordine diverso riceveva un contratto diverso — e in alcuni ordini un
  rifiuto.
- **`output_type` dichiarato**: e' accettato se appartiene all'insieme dei
  tipi possibili e **rifiutato** se non vi appartiene (prima un sotto-albero
  eterogeneo faceva passare qualunque dichiarazione). Un'espressione «solo
  null» resta compatibile con qualunque dichiarazione.
- **Operandi di tipo singolo**: `not`, `negate`, l'aritmetica, le funzioni
  testuali e numeriche rifiutano un operando il cui insieme contiene piu' di
  un tipo. Prima l'insieme eterogeneo diventava `Any` e passava.

### Comportamento osservabile (quindicesimo giro di review, 2026-08-18)

**Lo schema di output di `table.expression` e `table.formula` non dipende piu'
dai VALORI.** Il tipo si ricava dallo schema, con la stessa funzione che usa
l'analizzatore del contratto.

- **`table.expression` con `output_type = auto`**: il kernel risolveva il tipo
  osservando i valori calcolati. Su un batch **vuoto o tutto null** non ne
  osservava nessuno e ripiegava su `Utf8`, anche dove il contratto diceva
  `Boolean` o `Float64`. Ora il tipo e' quello dichiarato dal contratto in
  tutti i casi.
- **`table.formula`**: il difetto simmetrico. Sceglieva `Float64` quando
  nessun valore era testo — e su un batch vuoto o tutto null nessuno lo e' —
  quindi un'espressione testuale usciva `Float64` sul vuoto e `Utf8` sul
  pieno.
- **Conseguenza sui batch vuoti**: entrambe le operazioni **risolvono ora le
  colonne referenziate anche senza righe**. Un piano che nomina una colonna
  inesistente falliva su un batch pieno e riusciva su uno vuoto; ora fallisce
  in entrambi i casi. Senza risolvere le colonne non esiste un tipo di output
  da dichiarare, quindi non esiste una risposta giusta da dare.
- **`table.expression` con `auto` ed espressione eterogenea**: il rifiuto
  arriva ora dallo schema, quindi non dipende piu' da quali righe imboccano
  quale ramo. Analisi ed esecuzione danno lo stesso verdetto; prima il kernel
  poteva riuscire su dati che imboccavano sempre lo stesso ramo.
- **Categoria**: gli errori di tipo statico di `table.expression` sono ora
  `invalid_plan` e arrivano prima della valutazione, dove prima erano `schema`
  a meta' scansione. L'esito resta un errore in entrambi i casi.

## Aggiunte (compatibili)

- `plenora_core::limits::expansion_exceeded` — predicato esatto del fattore di
  espansione; `Limits::{MIN_SPILL_PARTITIONS, MAX_SPILL_PARTITIONS}`.
- `plenora_core::catalog::ExpansionConstraint::exceeded` — decisione esatta del
  vincolo binario.
- `plenora_core::json` — `ensure_no_duplicate_keys`.
- `plenora_kernels_table::MAX_SUFFISSI_COLLISIONE` — numero di suffissi che
  `resolve_output_names` prova prima di dichiarare una collisione non
  risolvibile.
- `plenora_kernels_table::setops::key_encodable` — predicato dei tipi che
  l'encoder di chiave di riga sa codificare, esposto accanto all'encoder che
  descrive invece di essere ricopiato nell'analizzatore.
- `plenora_kernels_geo::wkb_hex_to_bytes` — decodifica di un WKB esadecimale
  sui BYTE, sorgente unica per i kernel geo e per `prepare` dell'engine (che
  ne avevano due copie, entrambe capaci di andare in panic su un input non
  ASCII).
- `plenora_core::contract` — costanti delle chiavi canoniche
  (`PLENORA_GEOMETRY_*_KEY`), ora la fonte unica anche per `plenora-kernels-geo`.
- `plenora_core::diagnostics` — `RowDiagnosticsMergeError`,
  `RowDiagnostics::{merge_into, into_partial}`.
- `plenora_engine::ipc_boundary` — confine unico di lettura Arrow IPC
  (`open`, `open_with_format`, `header_schema`, `sniff_format`, `IpcLimits`,
  `BoundaryBatches`, `IpcFormat`).
- `plenora_engine::parallelism` — `configure`, `configured`.
- `plenora_engine::table_engine::ValidatedPlan::with_memory_budget` — copia
  del piano con `max_memory_bytes` ridotto (puo' solo scendere). Serve a
  far vedere ai kernel il budget RESIDUO invece di quello iniziale. Non
  trasforma il budget in un tetto duro: vedi DER-011.
- `plenora_kernels_table::{preflight_output_bytes, batch_bytes_per_row,
  column_bytes_per_row, type_bytes_floor, text_bytes_floor}` — mattoni della
  stima preventiva dell'output. La firma di `preflight_output_bytes` prende
  la larghezza della riga di output calcolata dal chiamante: il modello lo
  scrive il kernel. `batch_bytes_per_row` restituisce `Result` (aritmetica
  controllata).
- `plenora_core::batch_with_rows` — costruttore di `RecordBatch` che dichiara
  la cardinalita', necessario per i batch a zero colonne. **Spostato** da
  `plenora-kernels-table`, che ora lo ri-esporta: l'invariante e' del
  workspace, e tenerlo in una foglia aveva lasciato l'engine indietro.
- `plenora_core::ErrorCategory::{ALL, index}` — elenco canonico delle
  categorie e indice esaustivo, per chi deve ragionare su tutte senza
  scriversene una copia.
- `plenora_kernels_table::{text_bytes_per_row, text_convertible}` — larghezza
  testuale misurata sull'array (necessaria per le `Dictionary`) e predicato
  di convertibilita' allineato a `scalar_as_string`.
- `plenora_kernels_table::{resolve_output_names, validate_text_convertible}` e
  `reshape::resolve_melt_names` — risolutore SEQUENZIALE dei nomi di output
  (riserva ogni nome prima di calcolare il successivo) e verifica sullo schema
  di tipo e timezone.
- `plenora_core::panic_policy` — `PanicPolicy`, `install`, `riga_sanitizzata`,
  `forma_payload`: politica esplicita e idempotente per l'hook di panico del
  processo, pensata anche per gli embedder e per il binding Python. Residuo
  dichiarato in `docs/deroghe.md`, DER-010.
- `plenora_engine::executor::Inputs::{strict, is_strict, add_with_contract,
  with_contract}`; `Input::{read_ipc, read_ipc_with_limits}`.
- `plenora_engine::planner::check_declared_input_contracts`,
  `plenora_engine::planner::contract_fingerprint` (serve a `describe` per
  stampare lo stesso fingerprint che l'esecuzione verifichera').
- **CLI**: comando `describe` (alias `inspect-dataset`) e forma nominale
  `--input NOME=PERCORSO` su `run` e `validate`; i comandi
  `transform`, `spatial-join`, `transform-arrow`, `pair-arrow` sono segnalati
  deprecati nell'help.
- `plenora_kernels_table` — modulo `exact_compare`, modulo `hashing`
  (`KeyHasher`, `FastHasher`), `exact_f64_from_*`, `compare_i128`,
  `compare_decimal128*`, `compare_bounds`, `scalar_compare`,
  `scalar_as_f64_rounded`, `aggregation::{is_sortable, validate_sortable}`,
  `joins::coalesce_supported`, `SpillMetrics::accumulate`.

## Decisioni di rilascio, applicate il 2026-08-16

1. **`#[non_exhaustive]` sulle struct di metriche** — applicato a
   `ExecutionMetrics`, `NodeMetrics`, `SegmentMetrics`, `MemoryMetrics`,
   `SpillMetrics`. Farlo in questo bump, gia' rotto, evita che ogni futura
   metrica sia un'altra rottura. Costo accettato: i consumer non possono piu'
   costruirle, neanche nei propri test. `MetricsConfig` NON e' toccata: e'
   configurazione in ingresso, e renderla non costruibile sarebbe una
   restrizione funzionale, non una protezione.
2. **Deprecazione di `Inputs::add` e `Inputs::with`** — applicata. Restano
   per compatibilita' e restano testate; i siti interni che le usano hanno un
   `#[allow(deprecated)]` mirato e commentato, che e' l'elenco esatto dei
   punti da toccare quando la deprecazione diventera' rimozione. Rimozione
   proposta: il primo bump maggiore successivo a questo.
3. **CLI** — usa `Inputs::strict()`. Non puo' piu' inviare un input senza
   contratto neanche per errore di manutenzione.
4. **SDK Python** — non esporra' il percorso permissivo: li' non ci sono
   chiamanti da non rompere, quindi il profilo stretto e' l'unico. Vincolo
   registrato in `docs/piano-usabilita.md`.

## Da fare al bump

- scegliere il numero di versione (`1.0.3` → maggiore: la superficie e' rotta
  in piu' punti) e aggiornare `Cargo.toml` del workspace;
- emettere il manifesto in `release/<versione>.json` con revisione e run CI di
  evidenza, come i precedenti;
- riportare `since = "<versione>"` negli attributi `#[deprecated]`, che oggi
  ne sono privi proprio perche' il numero non era deciso.


---

# Aggiunta 2026-08-20 — contratto della memoria (ADR 15, livello 1)

Rotture **ulteriori** rispetto all'inventario del 2026-08-16, dallo stesso
bump maggiore. Le sezioni sopra restano com'erano scritte: descrivono lo stato
dell'API a quella data, quando il campo aveva ancora il nome della v4, e
riscriverle renderebbe l'inventario una cronaca inaffidabile.

## Il campo, in tutte le sue facce

`max_memory_bytes` si chiama ora **`max_governed_memory_bytes`**. Il nome
precedente prometteva un tetto sull'intero processo che in-process non è
realizzabile (ADR 15 §3); il nuovo dice che cosa il limite fa davvero.
**Non c'è alias, in nessuna forma**: né `serde(alias)`, né un campo
deprecato, né una chiave accettata «solo per compatibilità».

| dove | prima | dopo |
|---|---|---|
| `plenora_core::limits::Limits` | `max_memory_bytes: u64` | `max_governed_memory_bytes: u64` |
| `plenora_kernels_table::Limits` | `max_memory_bytes: usize` | `max_governed_memory_bytes: usize` |
| piano DAG, `limits` | `"max_memory_bytes"` | `"max_governed_memory_bytes"` |
| piano lineare (`schema_version <= 3`), `limits` | `"max_memory_bytes"` | `"max_governed_memory_bytes"` |
| `ValidatedPlan::with_memory_budget` | riduce `max_memory_bytes` | riduce `max_governed_memory_bytes` |

Chi costruisce `Limits` con struct literal, chi legge il campo, e chi scrive
piani JSON deve aggiornare il nome. Il compilatore segnala i primi due; per il
terzo la diagnosi è a runtime, ed è un **errore**, non un default silenzioso:
entrambe le strutture hanno `deny_unknown_fields`.

## Formato del piano: la canonica è la v5

- la `schema_version` canonica passa da **4 a 5**;
- un piano `schema_version: 4` è ancora accettato, ma **solo** attraverso la
  migrazione esplicita, che riscrive il nome del budget. Il resto del piano
  attraversa invariato;
- un piano v4 che dichiara il nome nuovo è **rifiutato**, come un piano v5 che
  dichiara il nome vecchio, come un piano che li dichiara entrambi;
- i piani `schema_version <= 3` migrano **direttamente al canonico v5**: non
  esiste una forma intermedia da cui ripartire;
- `validate` e `run` della CLI riportano `"schema_version": 5` anche per un
  piano v4 in ingresso: è la versione sotto cui il piano viene effettivamente
  eseguito.

**Nessuna migrazione automatica per i piani lineari.** La loro
`schema_version` non distingue il prima dal dopo, quindi un piano
`schema_version: 1` con il nome vecchio viene rifiutato e va aggiornato a
mano. È l'unica rottura di questo blocco che colpisce un formato che non
stavamo cambiando; la ragione è che il campo vive in una sola struttura
condivisa fra i due percorsi, e lasciarla col nome vecchio avrebbe reso la
decisione parziale (ADR 15 §7).

## `plan_hash`: invalidazione esplicita

```
plan_hash = SHA256("plenora/plan_hash/v5\0" || canonical_json)
```

Ogni `plan_hash` calcolato prima di questo cambiamento è **diverso**. Chi lo
conserva (cache, log di riproducibilità, confronti fra esecuzioni) deve
considerare invalidati i valori precedenti: non c'è collisione possibile con
la regola di prima, ed è deliberato — vedi ADR 4, emendamento 2026-08-20.

`catalog_fingerprint` e `ContractFingerprint` **non** cambiano.

## Superficie nuova

- `plenora_engine::plan::migrazione_v4` — `testo_canonico_v5` (ingresso di
  versione, `Cow`: un piano già v5 attraversa senza copia), `migra_v4_a_v5`,
  `versione_dichiarata`;
- `plenora_engine::plan::PLAN_SCHEMA_VERSION_V5` (= 5) e
  `PLAN_SCHEMA_VERSION_V4` (= 4, accettata solo dalla migrazione). La
  costante `PLAN_SCHEMA_VERSION_V4` di prima **valeva 4 ed era la canonica**:
  chi la usava per scrivere piani deve passare a `PLAN_SCHEMA_VERSION_V5`;
- i tipi `PlanV4`, `NodeV4`, `ValidatedPlanV4` sono rinominati in `PlanV5`,
  `NodeV5`, `ValidatedPlanV5`.

## Non introdotto

`hard_process_memory_bytes` (livello 2 di ADR 15) **non esiste**: arriverà col
profilo isolato che lo realizza. Introdurre il nome prima del meccanismo
ripeterebbe l'errore che ADR 15 corregge.
