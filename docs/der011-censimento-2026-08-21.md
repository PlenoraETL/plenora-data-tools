# DER-011 — dove si alloca ancora prima di prenotare

Ramo `fix/governor-atomic-permit`, da `main@343c033`. Blocco del **permesso
atomico del governor**: prerequisito di M3, non chiusura di DER-011.

Questo documento è il censimento richiesto: ogni punto in cui la memoria viene
allocata **prima** che il governor l'abbia autorizzata, diviso per percorso.
Verificatore: `scripts/verifica_censimento_der011.py`, che controlla per
àncore — non per numero di riga — che ogni sito citato esista ancora nella
forma descritta, e che i pattern eliminati non siano riapparsi.

---

## 1. Che cosa ha cambiato questo blocco

Il `MemoryGovernor` ha ora un **permesso atomico**: `permesso(bytes, owner)`
verifica e prenota in una sola operazione. Restituisce
`Result<Option<MemoryPermit>>`, e i tre esiti sono deliberatamente distinti:
`Ok(Some)` quota concessa, `Ok(None)` **budget insufficiente** — una
decisione, non un errore, per chi ha un'alternativa — ed `Err(Internal)`
**contabilità incoerente**, che è un'invariante nostra rotta e non si gestisce
passando al disco. `try_reserve` e `reserve` ne sono involucri: è l'unico
punto del crate in cui la quota viene presa.

`MemoryPermit::ritaglia(bytes)` riduce il permesso alla dimensione reale e
restituisce subito la differenza, **senza riprenotare** — serve a chi deve
prenotare un maggiorante prima di conoscere la dimensione esatta.

L'unico pattern `check` + `reserve` del codice è stato eliminato: la decisione
memoria/disco dei segmenti row-diagnostics non legge più `reserved_bytes()`
per poi prenotare altrove, ma **chiede un permesso** e lo tiene per tutta la
passata, ritagliandone l'uscita.

**Conseguenza per DER-011.** Il percorso row-diagnostics diventa il secondo
punto dell'engine in cui la reservation **precede** l'allocazione che deve
coprire. Prima ce n'era uno solo. È un passo reale e stretto: non chiude la
classe.

### Correzioni della seconda lettura indipendente

Cinque residui, tutti chiusi:

1. **`oldest_lease_age` sceglieva per id minore.** L'id è assegnato prima di
   leggere l'orologio, quindi sotto concorrenza l'ordine degli id non è
   l'ordine dei tempi. Ora si prende il **minimo degli istanti**;
2. **`live_leases` e `next_lease_id` usavano `fetch_add` non controllato.**
   Ora tutti gli incrementi e **anche i decrementi** passano da
   `checked_add`/`checked_sub`. Un diniego di budget (`Ok(None)`) e una
   contabilità incoerente (`Err(Internal)`) sono esiti **distinti**: il primo
   si gestisce passando al disco, il secondo no. Un `Drop` non può restituire
   un errore, ma può **marcare** la contabilità: da quel momento ogni
   richiesta fallisce, fail-closed e irreversibile;
3. **Lo snapshot non era linearizzabile.** `reserved`, `live` e `births` sono
   ora un'unica struttura sotto **un solo lock**, che acquisizioni, rilasci,
   ritagli e snapshot condividono. Chi legge vede uno stato realmente
   esistito. Il costo è un mutex per lease — per batch, mai per riga — e quel
   lock veniva già preso per registrare la nascita;
4. **La sezione `# Errors` prometteva `InvalidPlan`** dove il codice
   restituisce `ResourceLimit`. Corretta, ed estesa a `Internal`;
5. **L'ADR conteneva il picco vecchio e il nuovo.** Resta solo il misurato.

### Terza lettura: il fallback che riapriva il TOCTOU

`MemoryPermit::ritaglia` dichiarava che non si deve mai rilasciare e
riprenotare, e l'executor faceva **esattamente questo** quando il ritaglio
restituiva `None`: una nuova `reserve`. Riapriva la finestra che il permesso
esiste per chiudere, e per giunta `None` mescolava due casi diversi — output
oltre il permesso e contabilità corrotta.

Chiuso così:

- `ritaglia` restituisce `Result<MemoryLease>`. **Nessun ripiego**: un ritaglio
  impossibile significa che il maggiorante era sbagliato, cioè un'invariante
  nostra rotta, e si propaga `Internal` con il perché. Il fallback
  nell'executor è eliminato;
- `in_lease` è a sua volta fallibile: **nessuna trasformazione di un
  permesso** riesce su un governor già corrotto;
- `verifica_salute` è il **cancello** prima di dichiarare conclusa
  un'esecuzione. Una corruzione rilevata dentro un `Drop` non può propagare un
  errore da lì: senza questo controllo l'ultimo output verrebbe consegnato — o
  peggio pubblicato — da un governor che ha già perso il conto. È chiamato in
  `collect_batches` e prima del publish atomico, dove il file temporaneo non è
  ancora visibile (ADR 7) e fallire significa non pubblicare nulla;
- la **biiezione `live == births.len()`** è verificata nei due versi: un id già
  presente e una nascita mancante al rilascio marcano entrambi la contabilità.
  Il controllo sulla collisione sta **prima di ogni mutazione** — verificarlo
  dopo aver scritto `reserved` lascerebbe byte prenotati su una richiesta
  fallita, che è la prenotazione parziale che questo primitivo promette di non
  produrre. Questo difetto era presente nella prima stesura ed è stato trovato
  dalla regressione che lo cercava;
- la doc di `oldest_lease_age` dice ora il vero: **col lock unico id e istante
  non possono divergere**, e il minimo degli istanti esprime la semantica
  giusta e protegge da refactor futuri — non corregge più un caso
  raggiungibile.

### Quarta lettura: il consumo per iteratore

`Output` implementa `Iterator`, ed è così che un chiamante lo consuma con
`for batch in output`. Quel percorso **non passa** né da `collect_batches` né
dal publish atomico: a stream esaurito restituiva semplicemente `None`, quindi
una corruzione rilevata nell'ultimo `Drop` sarebbe stata un **successo
silenzioso**.

- `Iterator::next` esegue ora il **controllo di salute terminale** a stream
  esaurito, e produce `Some(Err(Internal))` **una volta sola**, poi `None`;
- uno stato terminale (`esaurito`) impedisce di ripeterlo: un errore emesso a
  ogni chiamata trasformerebbe un `for` in un ciclo che non finisce;
- `MemoryMetrics.accounting_corrupted` rende la corruzione **visibile nelle
  metriche parziali**. `Output::metrics()` è pubblica e leggibile a metà
  stream, quando l'errore non è ancora stato consegnato: senza quel campo
  mostrerebbe contatori apparentemente sani. La sua documentazione dice
  esplicitamente che quando è `true` gli altri campi non sono attendibili.

Un limite resta, ed è dichiarato nel test invece che scoperto: se lo stream è
**già** esaurito quando la corruzione avviene, il cancello è passato e
l'iteratore resta terminato. Non c'è un momento successivo in cui segnalare,
perché il consumatore ha già finito.

### Un test flaky, trovato martellando

La suite ha fallito **una volta su tre giri** con `1443 → 1442`. Non l'ho
archiviato: quaranta esecuzioni mirate dei test del governor hanno riprodotto
il caso al trentasettesimo giro. Era un difetto **del test**, non del codice:
confrontava l'età del lease caduto letta a un istante con quella del successivo
letta più tardi, e quando il tempo fra le due letture supera la differenza di
nascita — microsecondi — l'asserzione cade senza che nulla sia rotto. Ora usa
lo stesso sandwich del resto del test. Quaranta giri successivi: zero
fallimenti.

**Il benchmark non è stato eroso.** La sezione critica è cresciuta, ma il
rapporto normalizzato `streaming_lineare`/`blocking_sort` passa da 1,065
(permesso con atomici) a 1,078 (permesso sotto lock): dentro il rumore
dell'host, che nella stessa campagna sposta i controlli del 10–13%. Il residuo
di `solo_formula` resta 0,11 ms contro i 17,79 ms di partenza.

Le regressioni aggiunte sono **deterministiche e senza `sleep`**: l'età del
lease più vecchio è fissata da un sandwich di letture (le età crescono
monotonamente, quindi il valore riportato deve stare fra il massimo letto
prima e quello letto dopo); l'esaurimento dei contatori è forzato da un seam
di test esplicito; la linearizzabilità dello snapshot è verificata da
invarianti che valgono a ogni lettura, qualunque intreccio si realizzi.

---

## 2. Censimento

Legenda: **PRIMA** = la memoria è allocata prima della reservation (in
deroga); **PREVENTIVO** = la reservation precede l'allocazione; **ASSENTE** =
non c'è reservation.

### 2.1 Percorso DAG (`crates/plenora-engine/src/executor.rs`)

| sito | ordine | nota |
|---|---|---|
| arco di ingresso (`.reserve(bytes, &edge_name)`) | **PRIMA** | il batch arriva dalla sorgente già materializzato; il governor conta ciò che esiste |
| catena streaming, uscita (`reserve(bytes_at_boundary, …)`) | **PRIMA** | il kernel ha già prodotto l'output |
| catena streaming row-diagnostics, uscita | **PREVENTIVO** | novità di questo blocco: il permesso è preso prima della passata e l'uscita si ritaglia da lì (`permesso_di_trattenere`, `permesso.ritaglia`) |
| blocking unario, materializzazione (`concat_batches`) | **PRIMA** | `check_batch_bytes` e la reservation seguono la concatenazione |
| blocking unario, uscita | **PRIMA** | `run_kernel` ha già allocato |
| blocking binario, materializzazione left e right | **PRIMA** | due `concat_batches`, poi le due reservation |
| blocking binario, uscita | **PRIMA** | come sopra |
| gruppo geo fuso (`fused_group_decoded_bytes` → `try_reserve`) | **PREVENTIVO** | stima esatta dai payload WKB di input, *prima* di decodificare (ADR-0012 D12.7) |
| binario geo, forme decodificate | **PRIMA** | stimate e prenotate dopo la decodifica |
| replay dello staging IPC (`replay.reader.next()`) | **PRIMA** | il batch è decodificato, poi compattato con `take`, poi ri-riservato |
| kernel spill-capable, intermedio concatenato | **ASSENTE** | la reservation è saltata per costruzione quando `should_spill_unary` scatta (deviazione già dichiarata in ADR-0002) |

### 2.2 Percorso legacy (CLI)

| sito | ordine | nota |
|---|---|---|
| `ammissione_output` | **PRIMA** | rifiuta la *pubblicazione* di un risultato fuori budget, ma corre dopo che il kernel l'ha costruito |

### 2.3 Kernel (`plenora-kernels-table`)

| sito | ordine | nota |
|---|---|---|
| `preflight_output_bytes` per `cross_join`, `concat`, `concat_by_name`, `melt` | **PREVENTIVO, parziale** | rifiuta prima di allocare, ma il modello copre i buffer del risultato dichiarati dal kernel — non i temporanei che l'implementazione crea per conto proprio |
| ogni altra operazione tabellare | **ASSENTE** | nessun rifiuto preventivo: il governor vede solo il risultato |

### 2.4 Allocazioni Arrow

| sito | ordine | nota |
|---|---|---|
| tabelle hash di join/aggregate/distinct | **ASSENTE** | interne al kernel, mai contate |
| buffer di crescita e copie intermedie | **ASSENTE** | idem |
| `take` di compattazione al replay | **ASSENTE** | copia piena per batch, non coperta da alcun lease finché non è finita |
| allocazioni interne dei kernel geo (GEOS) | **ASSENTE nel governor** | *stimate* da `kernels-geo::memory_estimate`, mai alimentate al governor (dichiarato in ADR-0002) |

---

## 3. Valutazione

> **Prerequisito atomico chiuso, DER-011 ancora aperta.**

Il permesso atomico è realizzato, verificato e privo di pattern `check` +
`reserve`. Ma su undici siti censiti nel solo percorso DAG, **due** hanno la
reservation prima dell'allocazione e **uno** non ce l'ha affatto; il percorso
legacy, i kernel non-preflight e tutte le allocazioni Arrow interne restano
scoperti.

`max_memory_bytes` resta quindi **ciò che DER-011 dichiara**: un contatore di
ciò che è già stato allocato, non un'autorizzazione ad allocare. Un input che
produce un output molto più grande del budget continua a portare all'OOM
prima che l'errore `resource_limit` sia prodotto. **Nessun tetto duro viene
dichiarato qui.**

Che cosa servirebbe per chiudere la classe, in ordine di costo crescente:

1. **stima a priori per ogni operazione**, come il preflight delle quattro
   attuali. Possibile solo dove il conteggio delle righe di output precede
   l'allocazione, e comunque cieca ai temporanei;
2. **permesso preventivo generalizzato**: estendere ciò che questo blocco ha
   fatto per il percorso row-diagnostics a tutti i punti di allocazione —
   prendere il permesso prima di chiamare il kernel e ritagliarlo dopo. Ora è
   possibile, perché il primitivo esiste; resta da fare, sito per sito;
3. **accounting dentro l'allocatore**, che è l'unico modo di coprire le
   allocazioni Arrow interne, e cambia il rapporto con Arrow.

---

## 4. Il prezzo misurato del permesso

Tenere davvero la quota che si dichiara ha un costo osservabile, e va detto:

| carico | picco governato prima | con il permesso |
|---|---|---|
| `streaming_lineare` | 10,24 MiB | **73,83 MiB** |
| `blocking_sort` | 15,42 MiB | 15,42 MiB |
| `blocking_aggregate` | 7,71 MiB | 7,71 MiB |
| `fan_out_tee` | 15,44 MiB | 15,44 MiB |
| `rami_indipendenti` | 23,71 MiB | 23,71 MiB |

L'aumento è tutto e solo sul carico con un nodo row-diagnostics, ed è
l'headroom di `max_batch_bytes` (64 MiB di default) ora **realmente
trattenuto** invece che soltanto verificato.

Non cambia quali piani riescono: la condizione di ingresso in modalità memoria
era già `reserved + max_batch_bytes ≤ budget`, identica. Cambia che quella
quota adesso è presa, quindi il picco riportato la comprende — ed è la
differenza fra dire di avere spazio e averlo.

Chi legge `peak_reserved_bytes` su un piano con `formula`, `expression`,
`assert_*`, `date_*` o `flatten_json` vedrà quindi un picco più alto di prima.
È una misura più onesta, non una regressione: quei byte erano necessari anche
prima, semplicemente nessuno li teneva.
