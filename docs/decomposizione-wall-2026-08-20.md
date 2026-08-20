# Dove sta il wall che i tempi per nodo non coprono

Ramo `perf/orchestrazione-v1`. **Fase di strumentazione, non di
ottimizzazione**: nessuna modifica a engine, scheduler o API pubblica. Tutto il
codice nuovo sta in `examples/` e in `scripts/`.

Il verbale della linea di base (`docs/misura-orchestrazione-2026-08-19.md`,
§7) lasciava aperta una domanda: la somma dei `wall_time` per nodo copre il 98%
del wall su `rami_indipendenti` ma meta' scarsa su `streaming_lineare` e
`blocking_aggregate`, e quella meta' non era attribuita. Questo documento la
attribuisce.

Grezzi: `docs/misure/decomposizione/`.

```
cargo build --release --example misura_orchestrazione -p plenora-engine --locked
# in un container SEPARATO dalla build:
misura_orchestrazione --carico <nome> --fase decomposizione
misura_orchestrazione --carico streaming_lineare --fase catena
```

---

## 0. Prima domanda: le `SegmentMetrics` coprono il tempo mancante?

**No, e non possono.** Nei tre punti in cui l'executor registra un tempo
(`executor.rs` 3175/3188, 4693/4702, 4920/4929) il wall del segmento accumula
lo **stesso** `elapsed` del nodo. Un segmento fuso riporta la somma dei suoi
nodi, non un centesimo di piu'.

Non e' solo lettura del codice: l'harness lo **asserisce** a ogni cella
(`con.iter().all(|s| s.segmenti <= s.kernel)`). Se un giorno l'executor
cambiasse, la misura fallirebbe invece di riportare un numero vecchio.

---

## 1. Il metodo: una partizione, non una stima

`Output` espone due meta' cronometrabili dall'esterno, e la loro somma **e'**
il wall — non una approssimazione:

- `execute(...)` — costruzione del piano fisico e dello stream;
- `collect_batches()` — drenaggio dello stream.

Da qui la partizione, calcolata **per ripetizione** e senza alcun clamp:

```
wall = costruzione + kernel + residuo
```

dove `kernel` e' la somma dei `wall_time` per nodo e `residuo` e' cio' che
resta. Le somme delle quote nelle tabelle che seguono chiudono fra il 98% e il
101%: lo scarto e' quello fra mediane di serie diverse, non un termine
mancante.

Tre difetti del primo tentativo, corretti prima di produrre questi numeri:

| difetto | conseguenza | correzione |
|---|---|---|
| i batch di uscita venivano distrutti **dentro** la finestra cronometrata (`let (_, m) = collect_batches()`) | il residuo includeva la deallocazione dell'output, che la fase temporale non conta | i batch si legano a un nome e si distruggono **dopo** `elapsed()` |
| `saturating_sub` sulle differenze A/B | le differenze negative diventavano zero e la somma delle quote superava il 100% (fino a 129%) | differenze **con segno**, e partizione ridefinita in modo che non serva alcuna sottrazione satura |
| metriche accese e spente misurate in **campagne separate** | la differenza aveva segno casuale: era deriva dell'host, non effetto | le due configurazioni si **alternano dentro lo stesso ciclo**, con l'ordine invertito a ogni giro, e si confrontano per coppia |

---

## 2. La partizione, alla forma canonica (24 batch x 8192 righe)

| carico | wall | costruzione | kernel | residuo | somma |
|---|---|---|---|---|---|
| `streaming_lineare` | 36,37 ms | 1,38% | 47,2% | **51,5%** | 100,1% |
| `blocking_sort` | 21,13 ms | 2,22% | 70,6% | **27,3%** | 100,0% |
| `blocking_aggregate` | 12,71 ms | 3,80% | 46,0% | **49,6%** | 99,4% |
| `fan_out_tee` | 16,59 ms | 2,86% | 58,0% | **39,1%** | 99,9% |
| `rami_indipendenti` | 237,51 ms | 0,25% | 98,8% | **1,0%** | 100,1% |

`costruzione` e' piccola ovunque in valore assoluto (frazioni di millisecondo):
appare grande solo sugli input minuscoli, dove non c'e' altro. **Non e' lei** il
tempo mancante.

---

## 3. Il residuo non e' per batch: e' per riga

Tre assi indipendenti, ciascuno con l'altro tenuto fermo.

### Asse 1 — righe totali costanti, batch variabili (6 -> 96)

Isola cio' che si paga a ogni batch: dispatch, reservation, controlli d'arco.

| carico | costo per batch | R² |
|---|---|---|
| `streaming_lineare` | 57,3 µs | 0,962 |
| `blocking_sort` | 12,3 µs | 0,934 |
| `blocking_aggregate` | 1,4 µs | 0,126 |
| `fan_out_tee` | -54,3 µs | 0,766 |
| `rami_indipendenti` | 21,3 µs | 0,890 |

Alla forma canonica, 24 batch, questo termine vale **meno di un millesimo** del
residuo su ogni carico. Il costo per batch esiste ma **non e'** il tempo
mancante — e i R² bassi dicono che su meta' dei carichi la retta non descrive
nemmeno bene i dati, cioe' il segnale e' sotto il rumore.

### Asse 2 — batch costanti (24), righe totali variabili (24 576 -> 393 216)

Separa il costo per riga da quello fisso per esecuzione: sul primo asse i due
sono indistinguibili, perche' le righe totali non cambiano mai.

| carico | costo per riga | intercetta | R² |
|---|---|---|---|
| `streaming_lineare` | **85,6 ns** | -0,11 ms | 0,990 |
| `blocking_sort` | **31,1 ns** | -0,41 ms | 0,986 |
| `blocking_aggregate` | **32,7 ns** | -1,11 ms | 0,968 |
| `fan_out_tee` | **32,1 ns** | -1,90 ms | 0,870 |
| `rami_indipendenti` | **11,7 ns** | 0,57 ms | 0,840 |

**Questa e' la risposta.** Con R² fra 0,94 e 0,995 e intercette prossime allo
zero, il residuo e' **lavoro proporzionale alle righe**, non costo fisso di
preparazione e non costo per batch.

Conta per la decisione sull'orchestratore: un costo fisso per esecuzione un
esecutore parallelo non lo tocca; un costo **per riga** e' lavoro sui dati, che
si puo' sovrapporre come quello dei kernel — oppure eliminare, se si scopre che
e' lavoro evitabile. Le due sezioni seguenti mostrano che in parte lo e'.

### Asse 3 — input costante, catena di k nodi identici

`string_pad` a larghezza 20 su una colonna gia' lunga 20 e' idempotente dopo il
primo nodo: righe, schema e dimensione restano identici lungo la catena. Cio'
che cresce con `k` e' **solo** il numero di attraversamenti di confine.

| catena | residuo fisso | residuo per nodo | R² |
|---|---|---|---|
| `string_pad` x k | 0,18 ms | 0,053 ms | 0,980 |
| `formula` x k | 15,01 ms | 3,378 ms | 0,963 |

Una catena di quattro `string_pad` ha un residuo di **frazioni di
millisecondo**: l'attraversamento fra nodi, di per se', non costa quasi nulla.
Una catena di `formula` ne ha uno di **quindici millisecondi piu' tre per
nodo**. Non e' l'orchestrazione: e' l'operazione.

---

## 4. Prima causa: `table.formula`

Ogni operazione della catena streaming, misurata **da sola** sullo stesso input
(24 x 8192), stesso percorso fisico `LinearStreaming`, stessi 24 batch:

| piano a nodo singolo | wall | kernel dichiarato | residuo | residuo % |
|---|---|---|---|---|
| `formula` (`valore * 2`) | 22,07 ms | 2,08 ms | 19,52 ms | **88,4%** |
| `formula` (`1`, costante) | 21,79 ms | 1,16 ms | 20,14 ms | **92,4%** |
| `formula` (`id * 2`, intero) | 20,08 ms | 1,59 ms | 18,01 ms | **89,8%** |
| `string_pad` | 14,02 ms | 13,34 ms | 0,23 ms | **1,6%** |
| `filter` | 7,05 ms | 6,41 ms | 0,21 ms | **2,9%** |

Un nodo `table.formula` dichiara **2,08 ms** di kernel e ne costa **22,07**: il
88% del suo tempo non e' nel timer del suo kernel. `string_pad` e `filter`,
sullo stesso input e sullo stesso percorso, hanno un residuo dell'1–3%.

**Non e' il calcolo.** Una formula costante (`1`), che non legge alcuna colonna,
ha lo stesso residuo di `valore * 2`: 20,14 ms contro 19,52. Cambiare tipo
(`id * 2`, interi) non lo sposta. Il costo e' **strutturale del nodo**, non
della sua espressione.

**Dove sia esattamente, questa misura non lo dice.** L'executor esegue formula
dentro `run_kernel` come ogni altro kernel tabellare
(`executor.rs:3711` -> `dispatch_kernel` -> `execute_batch_with_spill_row_diagnostics`),
il segmento e' `LinearStreaming`, i batch sono 24 come per gli altri, i
contatori non sono saturi. Isolare la riga richiede strumentazione dentro
l'engine, che questa fase non fa. E' il primo punto da aprire nella fase
successiva.

---

## 5. Seconda causa: la materializzazione dei nodi bloccanti

Questa invece e' localizzata nel codice. Nei percorsi bloccanti — unario
(`executor.rs:4500`) e binario (`executor.rs:4630`) — la sequenza e':

```rust
concat_batches(&schema, &unwrapped)?;   // materializza TUTTI i batch
// ... check byte, reservation, rilascio lease, check cancellazione ...
let start = Instant::now();             // <- il cronometro parte QUI
let output = run_kernel(kernel, full, state)?;
let elapsed = start.elapsed();
```

La concatenazione di tutti i batch d'ingresso in uno — la materializzazione —
avviene **prima** che il cronometro parta. Il `wall_time` di `sort`,
`distinct`, `aggregate` e `join` misura quindi il solo kernel, **non** il costo
di materializzare cio' su cui il kernel lavora.

E' coerente con i modi fisici misurati, che il grezzo riporta per ogni
carico:

- `streaming_lineare`: `seg0` LinearStreaming
- `blocking_sort`: `seg0` Blocking
- `blocking_aggregate`: `seg0` Blocking
- `fan_out_tee`: `seg0` LinearStreaming, `seg1` LinearStreaming, `seg2` BinaryBlocking
- `rami_indipendenti`: `seg0` Blocking, `seg1` Blocking, `seg2` Blocking, `seg3` Blocking, `seg4` BinaryBlocking

`streaming_lineare` e' l'unico **senza** segmenti bloccanti, ed e' infatti
l'unico il cui residuo non e' materializzazione (§4). Gli altri quattro ne
hanno almeno uno. Fra questi, `rami_indipendenti` ha un residuo dell'1% non
perche' non materializzi — materializza cinque volte — ma perche' i suoi
kernel sono di due ordini di grandezza piu' costosi della concatenazione: il
termine c'e', ed e' semplicemente trascurabile li'. `blocking_sort` e
`blocking_aggregate` hanno un residuo per riga di 31,1–32,7 ns, dello stesso
ordine fra loro come ci si aspetta da un costo di concatenazione che dipende
dai dati e non dall'operazione.

**Non e' un difetto di correttezza** e non cambia alcun risultato: e' una
metrica che misura meno di quanto il suo nome suggerisca. Ma chi legge
`NodeMetrics.wall_time` per decidere dove ottimizzare viene sviato, ed e'
esattamente cio' che stava per succedere qui.

---

## 6. Che cosa e' attribuito, e che cosa no

Bilancio rispetto alla condizione di chiusura di questa fase — 90% attribuito,
oppure residuo spiegato:

| carico | attribuito da `kernel` + `costruzione` | residuo | spiegazione del residuo |
|---|---|---|---|
| `streaming_lineare` | 48,6% | 51,5% | `table.formula` (§4) |
| `blocking_sort` | 72,8% | 27,3% | materializzazione `concat_batches` (§5) |
| `blocking_aggregate` | 49,8% | 49,6% | materializzazione `concat_batches` (§5) |
| `fan_out_tee` | 60,8% | 39,1% | materializzazione del `concat` di convergenza (§5) |
| `rami_indipendenti` | 99,1% | 1,0% | sotto il 2%: nulla da spiegare |

Il 90% di attribuzione **per componente nominato** e' raggiunto su
`rami_indipendenti` soltanto. Sugli altri quattro il residuo e' **spiegato**:
per due carichi e' la materializzazione, localizzata a due righe di
`executor.rs`; per `streaming_lineare` e' `table.formula`, localizzata
all'operazione ma non alla riga.

Cio' che resta esplicitamente aperto:

- **la riga esatta** del costo di `table.formula`. Serve strumentazione dentro
  l'engine;
- **il costo dell'osservabilita'**. Il confronto appaiato fra metriche accese e
  spente e' risolto in **2 celle su 25**: altrove le differenze cambiano
  segno e non c'e' maggioranza netta. Si puo' solo dire che e' **sotto la risoluzione
  del metodo**, dell'ordine di un millisecondo sui carichi da ~20 ms, e che
  quindi **non e'** la spiegazione del residuo;
- **la separazione fra dispatch, governor e routing** dentro il residuo per
  riga. L'asse dei batch dice che valgono meno di un millesimo del residuo alla
  forma canonica, ma non li separa fra loro.

---

## 7. Conseguenza per l'orchestratore

La linea di base diceva che il tetto del parallelismo fra rami e' ~20% e che il
tempo sta nei kernel. Questa decomposizione corregge la seconda meta': una
parte consistente del wall **non** e' nei kernel, ed e' lavoro per riga che due
cause note producono fuori dai timer.

Ne segue che la prima ottimizzazione non e' aggiungere thread:

1. **`table.formula`** costa ~11 volte cio' che dichiara. Su un piano
   streaming con una formula, e' la voce piu' grossa del wall;
2. **la materializzazione dei nodi bloccanti** e' invisibile alle metriche e
   quindi non e' mai stata contabilizzata come costo.

Entrambe sono costi che il parallelismo fra nodi **sovrapporrebbe** senza
ridurre, mentre potrebbero essere ridotti direttamente. Quale delle due
convenga aggredire per prima non e' deciso da questo documento: prima serve
sapere **dove**, dentro `formula`, va quel tempo.
