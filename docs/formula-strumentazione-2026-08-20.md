# `table.formula`: dove va il tempo, e perche'

Ramo `perf/orchestrazione-v1`. **Misura, non ottimizzazione.** La
strumentazione usata qui e' **temporanea e non fa parte del codice**: e'
allegata come `docs/misure/sonde-formula/strumentazione.patch` e va riapplicata
per riprodurre i numeri. Nessuna API pubblica e nessuna metrica esistente sono
state toccate: le sonde scrivono su un canale separato, thread-locale.

Grezzi: `docs/misure/sonde-formula/sonde-1.json` e `sonde-2.json`, due
campagne indipendenti a host quieto.

Il documento precedente (`docs/decomposizione-wall-2026-08-20.md`) aveva
localizzato il residuo all'operazione `table.formula` — l'88% del suo wall
fuori dal timer del suo kernel — senza dire dove. Questo lo dice.

---

## 1. La causa

`table.formula` e' nell'elenco delle operazioni che **emettono diagnostica per
riga**:

```rust
// plenora-core/src/catalog.rs:462
pub fn emits_row_diagnostics(&self, config: &serde_json::Value) -> bool {
    match self.family {
        Family::Table => match self.id {
            "table.flatten_json" | ... | "table.formula" | "table.expression"
            | "table.assert_not_null" | ... => true,
```

`table.string_pad` e `table.filter` non ci sono. E la differenza non e' locale
al nodo: decide il **percorso fisico dell'intero segmento**.

```rust
// executor.rs:1920, in segment_stream
if segment_emits_row_diagnostics(&self.plan, index) {
    Ok(row_diagnostic_stream(input, ..., index))   // <- scan + staging + replay
} else {
    Ok(Box::new(input.map(move |item| {            // <- attraversamento diretto
        run_streaming_chain(&plan, index, &state, item?, None)
    })))
}
```

Il percorso `row_diagnostic_stream` (R9,9) esiste per una ragione di
correttezza dichiarata: **nessun accepted esce prima che la scansione sia
completa**, cosi' una rejection tardiva non pubblica righe gia' consegnate.
Per ottenerlo:

1. `scan_row_diagnostic_segment` (executor.rs:2822) esegue la catena su ogni
   batch e ne fa **staging IPC su file temporaneo** (`stage_one_batch`,
   executor.rs:2175 — `StreamWriter::write` su `CountingFile`);
2. a scansione completa, il file viene **riletto** (`replay_staged_batch`,
   executor.rs:2095): `StreamReader::next`, poi `compact_staged_batch`, che
   fa un **`take` di ogni colonna** in buffer right-sized — una copia completa
   per batch;
3. il lease del governor viene ri-riservato per batch, la directory
   temporanea rimossa alla fine.

Quindi ogni riga viene **serializzata, scritta su disco, riletta, decodificata
e ricopiata**. Nessuna di queste fasi e' dentro `run_kernel`, che e' l'unica
cosa che `NodeMetrics.wall_time` misura.

---

## 2. I numeri

Piani a nodo singolo, stesso input (24 batch x 8192 righe), stesso percorso
fisico `LinearStreaming`, mediana su 20 ripetizioni. Le sonde sono annidate:
`scansione_row_diagnostics` contiene lettura, catena e staging;
`replay_batch` contiene decodifica, compattazione e reservation; la somma usa
solo i contenitori piu' esterni.

### Campagna 1

| piano | wall | kernel | residuo | staging+replay | quota del residuo |
|---|---|---|---|---|---|
| `solo_formula` | 16,95 ms | 1,53 ms | 15,42 ms | 12,95 ms | **84%** |
| `solo_formula_costante` | 15,58 ms | 0,78 ms | 14,80 ms | 12,73 ms | **86%** |
| `solo_formula_intero` | 16,67 ms | 1,30 ms | 15,37 ms | 12,69 ms | **83%** |
| `solo_string_pad` | 11,70 ms | 11,04 ms | 0,66 ms | 0,00 ms | **0%** |
| `solo_filter` | 4,00 ms | 3,48 ms | 0,52 ms | 0,00 ms | **0%** |

### Campagna 2

| piano | wall | kernel | residuo | staging+replay | quota del residuo |
|---|---|---|---|---|---|
| `solo_formula` | 20,60 ms | 2,32 ms | 18,28 ms | 15,39 ms | **84%** |
| `solo_formula_costante` | 22,22 ms | 1,44 ms | 20,78 ms | 17,68 ms | **85%** |
| `solo_formula_intero` | 22,49 ms | 1,67 ms | 20,82 ms | 15,94 ms | **77%** |
| `solo_string_pad` | 13,86 ms | 12,90 ms | 0,96 ms | 0,00 ms | **0%** |
| `solo_filter` | 5,04 ms | 4,35 ms | 0,69 ms | 0,00 ms | **0%** |

Dettaglio delle sottofasi di `solo_formula`, campagna 1:

| sottofase | ms | quota del wall | ruolo |
|---|---|---|---|
| `scansione_row_diagnostics` | 7,483 | 44,1% | contenitore |
| `replay_batch` | 7,073 | 41,7% | contenitore |
| `staging_ipc` | 5,783 | 34,1% | dentro un contenitore |
| `replay_decodifica_ipc` | 4,281 | 25,3% | dentro un contenitore |
| `replay_compattazione` | 2,760 | 16,3% | dentro un contenitore |
| `catena_streaming` | 1,568 | 9,2% | contenitore |
| `run_kernel` | 1,534 | 9,1% | dentro un contenitore |
| `staging_pulizia` | 0,078 | 0,5% | foglia |
| `lettura_routing` | 0,041 | 0,2% | dentro un contenitore |
| `replay_apertura` | 0,037 | 0,2% | foglia |

**Lo staging IPC e il suo replay valgono da soli il 77–86% del residuo**
sui tre piani con `formula`. Su `string_pad` e `filter`, che non passano da
quel percorso, quelle sottofasi **non esistono** e il residuo e' sotto il
millisecondo.

Il costo **non dipende dall'espressione**: una formula costante (`1`), che non
legge alcuna colonna, ha lo stesso staging e lo stesso replay di `valore * 2`.
Il suo kernel costa meno di un millisecondo e il suo wall resta sopra i
quattordici.

### Copertura e calibrazione

La somma delle sottofasi copre il **83–92%** del residuo sui piani con
`formula`. Cio' che resta — circa un millisecondo, meno dell'11% — sta nello
strato di raccolta: l'iteratore di `collect_batches`, il rilascio del lease per
batch e la costruzione del `Vec` di uscita, che non sono strumentati.

Ogni sonda costa **42–90 ns**. Con circa dodici sottofasi e
ventiquattro batch, la strumentazione pesa attorno a **0,03 ms per esecuzione**,
cioe' meno dell'0,2% del wall misurato: non e' lei a produrre questi numeri.

---

## 3. Il punto preciso

| dove | file:riga | che cosa |
|---|---|---|
| classificazione | `plenora-core/src/catalog.rs:471` | `table.formula` fra le operazioni con diagnostica per riga |
| biforcazione | `crates/plenora-engine/src/executor.rs:1920` | il segmento intero prende il percorso con staging |
| scansione | `crates/plenora-engine/src/executor.rs:2822` | `scan_row_diagnostic_segment` |
| scrittura | `crates/plenora-engine/src/executor.rs:2175` | `stage_one_batch`: `StreamWriter::write` per batch |
| rilettura | `crates/plenora-engine/src/executor.rs:2095` | `replay_staged_batch`: `StreamReader::next` |
| copia | `compact_staged_batch`, chiamata da `replay_staged_batch` | `take` di ogni colonna, una copia piena per batch |

---

## 4. Che cosa questa misura NON dice

- **non dice che il percorso sia sbagliato.** La garanzia che serve —
  nessun accepted pubblicato prima della fine della scansione — e' reale, ed e'
  la ragione per cui lo staging esiste. Questo documento misura il suo prezzo,
  non ne discute la necessita';
- **non propone una correzione.** Le alternative concepibili (buffer in memoria
  entro un tetto, staging solo quando una rejection si e' vista, riuso del
  batch senza `take`) hanno ciascuna implicazioni su memoria e semantica che
  non sono state valutate qui;
- **non generalizza alle altre operazioni della lista.** `expression`, gli
  `assert_*`, `date_*` e `flatten_json` prendono lo stesso percorso e pagheranno
  qualcosa di simile, ma **non sono state misurate**;
- **non copre l'ultimo ~10% del residuo**, che resta attribuito allo strato di
  raccolta senza sonde.

---

## 5. Fatti verificati

1. `table.formula` fa prendere all'**intero segmento** il percorso
   row-diagnostics, che serializza ogni batch su file temporaneo e lo rilegge.
2. Quel meccanismo vale il **77–86%** del residuo di un piano con una sola
   `formula`, su due campagne indipendenti.
3. Il costo **non dipende dall'espressione**: una costante che non legge
   colonne costa quanto un prodotto.
4. `string_pad` e `filter`, stesso input e stesso percorso fisico, hanno un
   residuo sotto il millisecondo.
5. La strumentazione pesa meno dell'0,2% del wall: i numeri non sono un
   artefatto delle sonde.
