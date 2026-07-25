# ADR 1 — Livelli di determinismo

- **Stato**: accettato (design)
- **Decisioni collegate**: D10, D13, D18
- **Riferimenti**: `Architetture.md` §6.3, §6.4; `Prestazioni.md` §6

## Contesto

L'executor è libero di scegliere lo schedule (seriale, parallelo per batch,
parallelo per ramo). Se l'output dipendesse dallo schedule, il sistema non
sarebbe riproducibile — inaccettabile per la postura fail-closed. Allo stesso
tempo, una promessa di output identico **byte-per-byte** è troppo forte: il
contenuto di un file Arrow IPC dipende da batch boundaries, ordine dei
metadati, dictionary encoding, configurazione del writer, compressione,
versione di Arrow, piattaforma e librerie esterne (GEOS).

Per le geometrie il problema è ulteriore: due geometrie semanticamente
equivalenti possono avere rappresentazioni WKB diverse (orientamento degli
anelli, punto iniziale di un ring, ordine dei componenti di un multipoligono,
differenze floating point, output GEOS dipendente dalla versione).

## Decisione

Esistono **due livelli di determinismo**, dichiarati e testati separatamente.

### Livello 1 — Determinismo semantico (sempre garantito)

A parità di piano validato e input, qualunque schedule produce:

- stesse righe, stessi valori, stessi null;
- stesso ordine dichiarato (se l'operazione definisce un ordine);
- stesse geometrie secondo l'uguaglianza geometrica definita sotto.

È il livello verificato dal property test obbligatorio: *stesso piano, schedule
forzato seriale vs parallelo, risultato semanticamente identico*.

### Livello 2 — Determinismo IPC canonico (opzionale)

In aggiunta al livello 1: stessi confini di batch, stesso ordine dei metadati,
stesso dictionary layout, stessa configurazione del writer, stesso formato
binario finale.

- Garantito **solo a parità di versione dell'engine**.
- Uso: test di regressione, cache, hashing dell'output.
- Non è mai promesso tra versioni, piattaforme o configurazioni diverse.

### Uguaglianza geometrica

Il confronto tra geometrie è **geometrico, non byte-per-byte sul WKB**:

- uguaglianza con tolleranza dichiarata per coordinate floating point;
- normalizzazione topologica opzionale (orientamento anelli, punto iniziale,
  ordine componenti) quando il confronto la richiede;
- `-0.0` e `+0.0` sono considerati uguali; `NaN` è considerato uguale a `NaN`
  ai fini del determinismo (mai propagato come valore valido nelle geometrie:
  la validazione dinamica rifiuta coordinate non finite);
- garanzia limitata alla **stessa versione dei backend** (GEOS/PROJ): output
  di operazioni booleane o di riproiezione possono variare tra versioni.

### Regola di catalogo

Ogni operazione con ordine non definito (union, concat di rami paralleli, set
operations, aggregazioni) dichiara nel catalogo il campo `determinism`: la
politica di ordinamento deterministico applicata all'output (es. ordinamento
stabile su chiave, ordinamento canonico). I kernel paralleli usano collect
indicizzato; nessuna iterazione su hash map con ordine indefinito raggiunge
l'output.

**Ordine logico, non temporale**: le operazioni parallele ricompongono
l'output secondo un **ordine logico assegnato dal piano**, mai secondo
l'ordine temporale di completamento dei task — l'"ordine di arrivo" non è
deterministico in presenza di rami paralleli. Ogni batch porta una sequenza
logica:

```rust
struct BatchSequence {
    source_node: NodeId,
    input_partition: u32,
    sequence_number: u64,
}
```

La politica `InputOrder` significa "ordine logico delle `BatchSequence` in
ingresso", non "chi ha finito prima".

## Conseguenze

- I test di determinismo confrontano semanticamente (confronto geometrico per
  le colonne geometria), non i byte IPC — tranne quando testano
  esplicitamente il livello canonico.
- La cache degli output può affidarsi al livello canonico solo internamente
  alla stessa versione dell'engine.
- Chi consuma output tra versioni diverse dell'engine deve aspettarsi
  equivalenza semantica, non identità binaria.
