# ADR 6 — Semantica dei limiti

- **Stato**: accettato (design)
- **Decisioni collegate**: D19, D26
- **Riferimenti**: `Architetture.md` §3.1, §5; `Prestazioni.md` §3

## Contesto

"Un limite sulle righe" è ambiguo in un DAG: righe lette? prodotte? per arco?
in totale? Operazioni come `explode`, join molti-a-molti e fan-out rendono la
definizione critica — un limite mal definito è facile da aggirare o punitivo
verso pipeline legittime. Anche la memoria ha due facce: i byte reali
(ADR 2) e le espansioni logiche che i byte non catturano subito.

## Decisione

### Limiti di righe (`RowLimits`)

```rust
struct RowLimits {
    max_input_rows: u64,        // per singola sorgente di input
    max_output_rows: u64,       // righe dell'output finale
    max_rows_per_edge: u64,     // righe su qualunque arco intermedio del DAG
    max_expansion_factor: f64,  // output/input per operazione, base per classe
}
```

- **`max_rows_per_edge`** si applica a ogni arco singolarmente: un fan-out non
  "moltiplica" il conteggio — ogni ramo ha il proprio arco con il proprio
  conteggio.
- **`max_expansion_factor`**: la base di confronto è **specifica per classe di
  operazione**, dichiarata in catalogo:
  - unarie (`explode`): `output_rows / input_rows`;
  - binarie (join): nessuna base singola è adeguata — il runtime calcola tutte
    le metriche e il catalogo dichiara quali sono vincolanti:

```rust
struct JoinExpansion {
    output_over_sum_inputs: f64,
    output_over_left: f64,
    output_over_right: f64,
}
// vincolo dichiarato in catalogo:
// SumRelative | LeftRelative | RightRelative | MaxRelative | Custom
```

  Un lookup join (output ≤ left) usa tipicamente `LeftRelative`; un join
  molti-a-molti va vincolato con `MaxRelative` o una stima `Custom`.
  La stima a priori usa le statistiche disponibili (ADR 5); il rispetto
  effettivo è verificato a runtime dall'executor.
- **`max_total_rows_processed`**: **non** è un limite della v1 — è una metrica
  obbligatoria e può diventare limite avanzato. Il suo valore dipende dal piano
  fisico (due `ExecutionPlan` semanticamente equivalenti possono conteggiarlo
  diversamente, es. segmenti fusi), quindi non può essere un criterio di
  rifiuto deterministico.

### Limiti di memoria

`max_memory_bytes` e `max_temp_bytes` operano su byte reali (perimetro e
protocollo in ADR 2). I limiti di righe proteggono dalle espansioni logiche,
non sostituiscono il budget in byte.

### Limiti del piano (`PlanLimits`)

Applicati **durante il parsing**, prima di qualunque allocazione guidata dal
contenuto:

```rust
struct PlanLimits {
    max_plan_json_bytes: usize,
    max_plan_nodes: usize,
    max_plan_edges: usize,
    max_plan_depth: usize,
    max_fan_out: usize,
    max_inputs: usize,
    max_config_bytes_per_node: usize,
    max_identifier_bytes: usize,
}
```

Un piano che supera questi limiti è rifiutato come invalido, non come errore di
runtime.

### Limiti stringa

`max_string_bytes`, `max_regex_bytes`: per singolo valore/pattern, ereditati da
nogeo-tools.

## Conseguenze

- Ogni limite ha un punto esatto di applicazione (parsing, validazione,
  runtime) e un significato non ambiguo: la documentazione utente li riporta
  verbatim.
- Test obbligatori: explode oltre il fattore di espansione; join molti-a-molti
  sotto `max_rows_per_edge`; fan-out che non moltiplica i conteggi; piano
  ostile oltre `PlanLimits` rifiutato in parsing.
- Se in futuro `max_total_rows_processed` diventerà limite operativo, la sua
  semantica dovrà essere ridefinita in modo indipendente dal piano fisico
  (nuovo ADR).
