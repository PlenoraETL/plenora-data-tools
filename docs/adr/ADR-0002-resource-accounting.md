# ADR 2 — Resource accounting e reservation protocol

- **Stato**: attuato parzialmente (Fase 2B-M1, executor seriale) — vedi
  "Stato di attuazione" in coda
- **Decisioni collegate**: D11, D18, D26
- **Riferimenti**: `Architetture.md` §6.4; `Prestazioni.md` §3 (M1–M5)

## Contesto

I limiti di memoria devono valere **globalmente sul piano**, con rami paralleli
che condividono la stessa quota. Due errori simmetrici da evitare: reservation
incrementali che portano a deadlock (due rami con quota parziale che attendono
entrambi), e contabilità così invasiva da diventare essa stessa il collo di
bottiglia. Inoltre la memoria Arrow non è sempre contabilizzabile con
precisione assoluta: buffer condivisi, slice zero-copy, dictionary condivisi,
capacità allocata diversa dalla lunghezza, memoria nativa GEOS.

## Decisione

### Perimetro di `max_memory_bytes`

Comprende: buffer Arrow, capacità allocata, batch in coda, dictionary
condivisi, geometrie decodificate, hash table, indici spaziali, cache di
segmento, memoria temporanea dei kernel, writer IPC. Escluso ma monitorato:
strutture del planner/executor (fisse), memoria nativa GEOS (stimata).

- **Allocazioni governate dall'engine**: conteggio deterministico e
  generalmente preciso.
- **Allocazioni condivise** (slice, dictionary): il buffer sottostante è
  conteggiato **una sola volta** per lease; le slice non moltiplicano il costo.
- **Allocazioni esterne/native** (GEOS): stima dichiarata, mai presentata come
  conteggio preciso.
- Margine di sicurezza configurabile; metriche separate per memoria riservata,
  osservata e stimata.

### Ownership: `MemoryLease`

```rust
struct GovernedBatch {
    batch: RecordBatch,
    lease: Arc<MemoryLease>,
}
```

Batch e quota attraversano le code come un'unica unità; al fan-out il lease è
**condiviso** (reference-counted), mai duplicato: il batch è contabilizzato una
sola volta fino al rilascio dell'ultimo riferimento. Reference counting per
batch/buffer, **mai per riga**.

**Osservabilità obbligatoria dei lease** (un riferimento trattenuto è quota
occupata, e deve essere diagnosticabile): età del lease, nodo proprietario
originario, numero di riferimenti, byte trattenuti, lease più vecchi, lease
vivi durante la cancellazione.

### Protocollo anti-deadlock

Le due categorie di operatori sono separate:

- **Memoria stimabile** (blocking classico): reservation **completa** prima di
  iniziare; divieto di attesa con reservation parziale; acquisizione in ordine
  globale.
- **Crescita imprevedibile** (join, explode, alcune op geografiche):
  protocollo chunked — reservation minima iniziale, crescita a chunk,
  **nessuna attesa di nuova quota senza prima spillare o rilasciare memoria
  revocabile**, rispetto di `max_expansion_factor` (base per classe di
  operazione, ADR 6), interruzione controllata prima che l'intermedio diventi
  ingestibile.

**Esito della reservation a tre vie** (niente fail-fast immediato quando la
quota potrebbe liberarsi a breve):

```rust
enum ReservationResult {
    Granted(MemoryReservation),
    RetryAfterProgress,
    MustSpill,
}
```

Regole: un nodo non attende mai mantenendo reservation parziali; un nodo
**senza risorse trattenute** può essere sospeso e il runtime riprova dopo un
progresso globale (nessun busy-waiting, nessun lock globale frequente); se
esiste una strategia di spill, è preferita; il **fail-fast è l'ultima
opzione**, solo quando non esiste alcuna strategia sicura.

Invariante: *nessun nodo attende indefinitamente una nuova reservation
mantenendo risorse che impediscono agli altri nodi di progredire*. Timeout
solo come ultima protezione.

Priorità tra rami: nella v1 nessuna priorità, schedulazione equa.

### Spill selettivo

Spill per le operazioni blocking ad alto impatto (sort, hash aggregation, hash
join, spatial join, distinct, set operations), non universale. Requisiti: file
partizionati e cancellabili, `max_temp_bytes`, attivazione **prima**
dell'esaurimento della quota, metriche su byte scritti/letti, I/O fuori dal
pool CPU.

### Overhead del governor

Il governor non percorre ricorsivamente i batch a ogni nodo, non riconta
buffer noti, non usa lock globali per riga, non crea reservation per
operazioni semplici già coperte dal lease del batch. Overhead misurato da
benchmark dedicati (invariante P10 di `Prestazioni.md`).

## Conseguenze

- Test deterministici obbligatori: quote molto basse con almeno due rami
  concorrenti; fan-out con batch condiviso e accounting unico; cancellazione
  con lease vivi.
- La diagnostica di pressione memoria si basa sulle metriche di lease, non su
  euristiche.
- Chi implementa un nuovo kernel blocking deve dichiarare la categoria
  (stimabile/adattivo) e seguire il protocollo corrispondente.

## Stato di attuazione (Fase 2B-M1, executor seriale)

Implementato in `plenora-engine/src/governor.rs` e nell'executor:

- `MemoryGovernor` con budget globale di piano (`max_memory_bytes`),
  `MemoryLease` RAII reference-counted (quota restituita al Drop dell'ultimo
  clone), `GovernedBatch` che trasporta batch+lease+`BatchSequence` nello
  stream; i kernel restano su `RecordBatch` puro, il wrapper si spacca e si
  ricompone ai confini di segmento.
- Fan-out tee: quota contata una sola volta, lease condiviso fino all'ultimo
  consumatore (test dedicato).
- Osservabilità: `bytes_in`/`bytes_out` per nodo; `ExecutionMetrics.memory`
  con budget, riservato, picco, lease vivi, età del lease più vecchio.
- Acquisizione a ogni confine batch (mai per riga), reservation multiple in
  ordine globale fisso per i binari (left→right, già pronto per
  l'anti-deadlock parallelo).

### M2d — staging memory-first degli accepted row-diagnostics

I segmenti che emettono diagnostica per riga (R9.9) devono trattenere ogni
batch accettato fino a scansione completa: una rejection tardiva non deve
pubblicare righe già consegnate. Fino a M2c quell'attesa era **sempre su
disco**: staging Arrow IPC su file temporaneo, lease rilasciato per batch,
replay con decodifica e copia `take` a scansione conclusa.

La barriera non richiede il disco: richiede che nulla esca prima della fine.
Da M2d gli accepted attendono **in memoria**, come `GovernedBatch` con il
lease vivo, e si passa al disco solo quando il budget non basta più.

**Soglia, deterministica.** Prima di eseguire la catena sul batch `k` — già
prelevato, quindi di dimensione nota — si resta in memoria se e solo se:

```text
governor.reserved_bytes() + max_batch_bytes <= max_memory_bytes
```

Nessuna percentuale scelta a mano, nessuna decisione temporale, nessuna
dipendenza dall'ordine di arrivo: solo lease vivi e limiti del piano.

**La fonte è globale, non locale.** La prima stesura sommava i soli accepted
trattenuti più il batch d'ingresso corrente. Non è tutto ciò che è vivo: in un
fan-out `EdgeShared` conserva i batch già prelevati per il consumatore più
lento e ne trattiene i lease, che nessun contatore locale del ramo
row-diagnostics può vedere.

**Sulla v1 quel buco non era raggiungibile**, verificato invece che supposto:
in un fan-out i rami devono riconvergere, e qui sempre attraverso un nodo che
materializza (`concat`/`join` binari, `BinaryBlocking`), il quale drena e
trattiene comunque tutti i batch del ramo. Il picco governato risulta perciò
identico nelle due modalità — misurato: 143 744 byte sia in memoria sia su
disco. Dove il tee trattiene la memoria non aggiunge nulla al picco; dove la
memoria alza il picco (uscita diretta al piano) non c'è tee. Nessun input
costruito ha prodotto un falso `ResourceLimit`.

Il difetto era quindi **nella prova, non nel comportamento**: la sicurezza
della soglia poggiava su un accoppiamento architetturale implicito —
`max_batch_bytes` finiva per essere maggiore delle prenotazioni non contate —
che nessun invariante garantisce. Un binario streaming, un nuovo punto di
ritenzione o lo scheduler parallelo lo romperebbero **in silenzio**, e allora
il falso `ResourceLimit` diventerebbe reale. `reserved_bytes()` è la fonte
unica — accepted trattenuti, lease del batch corrente, buffer del tee e
qualunque altra prenotazione viva, presente o futura — e la garanzia smette di
dipendere dalla topologia. Un ingresso **senza** lease non è contabilizzato dal
governor: in quel caso si va su disco, fail-closed.

**Perché non può produrre un falso `ResourceLimit`.** Durante una passata le
prenotazioni *nuove* sono al più due — input e output, perché
`run_streaming_chain` acquisisce il secondo prima di rilasciare il primo — e
l'input è già dentro `reserved_bytes()`. Ogni batch di output attraversa il
wrapper d'uscita, che applica `max_batch_bytes` (V7): un output che lo supera
fa fallire il piano **in entrambe le modalità**, quindi per un piano che riesce
`output_k <= max_batch_bytes` è un maggiorante valido della sola prenotazione
nuova, e la soglia tiene il picco dentro il budget qualunque altra cosa il
piano stia già trattenendo.

**Atomicità: risolta dal permesso (2026-08-21).** La prima stesura di M2d
leggeva `reserved_bytes()` e prenotava altrove: due operazioni, corrette solo
perché l'esecuzione è seriale, e un TOCTOU non appena non lo sarà. Il
`MemoryGovernor` espone ora un **permesso atomico** — `permesso(bytes, owner)`
— che verifica e prenota in una sola operazione e restituisce `Option`, perché
un diniego è una decisione e non un errore; `try_reserve` e `reserve` ne sono
involucri, ed è l'unico punto del crate in cui la quota viene presa.

M2d lo usa così: prima della passata si chiede un permesso per
`max_batch_bytes` — maggiorante valido dell'unica prenotazione che la passata
aggiunge — e l'uscita si **ritaglia** da quel permesso
(`MemoryPermit::ritaglia`), che riduce il conteggio dello stesso lease e
restituisce solo la differenza. Nessuna riprenotazione, quindi nessuna
finestra. Un permesso negato manda su disco: fail-closed.

**Niente ripieghi.** `ritaglia` e `in_lease` sono fallibili e non ripiegano
mai su una nuova prenotazione: rilasciare e riprenotare riaprirebbe
esattamente la finestra che il permesso chiude. Un ritaglio impossibile
significa che il maggiorante era sbagliato — invariante nostra rotta,
`Internal` — e un governor già corrotto non concede alcuna trasformazione.

**Contabilità e biiezione.** Tutti gli incrementi e i decrementi passano da
aritmetica controllata, e la corrispondenza `live == births.len()` è
verificata nei due versi: id duplicato e nascita mancante marcano entrambi la
contabilità come incoerente. Il `Drop` non può restituire un errore, quindi
marca; `verifica_salute` è il cancello che intercetta la marcatura **prima di
dichiarare conclusa** un'esecuzione, su **tutti e tre** i percorsi pubblici di
consumo: `collect_batches`, il publish atomico e `Iterator::next` a stream
esaurito — quest'ultimo emette l'errore una volta sola e poi termina. Senza il
terzo, `for batch in output` avrebbe trasformato una corruzione in un successo
silenzioso.

`MemoryMetrics.accounting_corrupted` espone la marcatura: `Output::metrics()`
è pubblica e leggibile a metà stream, quando l'errore non è ancora stato
consegnato, e senza quel campo mostrerebbe contatori apparentemente validi.
Quando è `true` gli altri campi **non sono attendibili**.

Il prezzo è che la quota dichiarata adesso è **davvero trattenuta**: il picco
governato di un carico row-diagnostics comprende l'headroom (misurato su
`streaming_lineare`: da 10,24 a 73,83 MiB con `max_batch_bytes` a 64 MiB).
Non cambia quali piani riescono — la condizione di ingresso in memoria era già
la stessa — ma il picco riportato è ora una misura di ciò che si tiene, non di
ciò che si spera di poter tenere.

**Passaggio al disco.** Quando la soglia non regge, i trattenuti sono travasati
**nell'ordine di produzione** nello staging IPC esistente e i lease rilasciati
uno a uno; da quel momento la modalità è disco in via definitiva, per tutta la
scansione. Il picco durante il travaso non cresce mai sopra quello già
concesso.

**Esiti.** A scansione completata senza errori la modalità memoria consegna la
coda direttamente, con lease e `BatchSequence` originali; quella disco usa il
replay invariato. Su rejection tardiva, errore, cancellazione o fallimento del
travaso: **zero accepted pubblicati** e cleanup completo — in memoria i lease
muoiono con la coda, su disco il `TempDir` cancella il file.

**Perimetro.** Il gate WKB dell'input (`stage_input_batches`) resta su disco:
non è toccato da M2d. Nessuna operazione è specializzata e
`emits_row_diagnostics` è invariato: cambia dove gli accepted attendono, non
chi passa dalla barriera.

**Prezzo dichiarato.** Il picco governato di un segmento row-diagnostics
cresce di due voci: i byte degli accepted trattenuti **e** l'headroom del
permesso, che con l'atomicità non è più solo verificato ma davvero preso. La
misura complessiva è quella riportata sopra — `streaming_lineare` da 0,76 a
73,83 MiB su un budget di 512 MiB — e non va sommata a nessun'altra: è il
picco finale, non un incremento intermedio. È un aumento reale del picco, non
un aumento del tetto: il permesso lo tiene sotto `max_memory_bytes` per
costruzione, perché è il permesso stesso a non essere concesso quando non ci
starebbe.

**Deviazioni/rinvii rispetto al design** (documentati nel codice):

- In v1 seriale `try_reserve` emette **solo** `Granted`; se la quota manca,
  esito fail-fast `Contract`. `RetryAfterProgress`/`MustSpill` esistono
  nell'API ma non sono mai emessi: non c'è scheduler che sospenda rami né
  spill collegato. Il protocollo anti-deadlock completo (attese, sospensioni,
  protocollo chunked) è M3 insieme al DAG parallelo.
- Spill (M2): implementato per **sort** (external merge sort), **distinct** e
  **hash aggregation** (oracoli memoria-vs-spill con output identico), con
  selezione preventiva al dispatch su soglia stimata (`should_spill_unary`),
  formato Arrow IPC partizionato, `SpillMetrics` (byte scritti/letti, file) in
  `ExecutionMetrics` e nel JSON CLI, directory condivisa del `TempStore`
  per-esecuzione. **Grace hash join: non implementata** (richiede tracciamento
  degli ordinali attraverso il join per preservare l'ordine esatto
  dell'output). Set operations: spill preesistente, non ancora instradato
  sulla directory condivisa né coperto da metriche (follow-up).
- **Limite noto della v1 (dichiarato)**: per i kernel spill-capable l'input
  del segmento blocking è materializzato in RAM **senza contabilità governor**
  (lease rilasciati al drenaggio, reservation dell'intermedio saltata quando
  `should_spill_unary` scatta): in quel transitorio `max_memory_bytes` non è
  un tetto duro. Il tetto resta pienamente garantito per i kernel non
  spill-capable e dal `check_batch_bytes` sull'intermedio. La soluzione
  strutturale (spill in streaming durante il drenaggio, senza
  materializzazione) è M3.
- Memoria nativa GEOS/geometrie decodificate: **stimata** (M2b,
  `kernels-geo::memory_estimate`, formula dichiarata), mai presentata come
  conteggio preciso; non ancora alimentata al governor.
- Margine di sicurezza configurabile e benchmark di overhead (P10): rimandati.
