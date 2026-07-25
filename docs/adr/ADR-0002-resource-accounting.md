# ADR 2 — Resource accounting e reservation protocol

- **Stato**: accettato (design)
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
