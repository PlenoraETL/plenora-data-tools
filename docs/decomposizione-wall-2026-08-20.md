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

Non e' solo lettura del codice: l'harness **asserisce l'uguaglianza** delle due
somme su ogni ripetizione di ogni cella (`assert_eq!(s.segmenti, s.kernel)`).
E' uguaglianza esatta, non tolleranza: sono le stesse `Duration` sommate nello
stesso ordine. Se un giorno l'executor registrasse altrove — o smettesse di
registrare per segmento — la misura **fallirebbe** invece di riportare un
numero vecchio e questa sezione invece di restare vera.

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
resta. Su tutte le 50 celle misurate le quote chiudono fra
97,7% e 100,4%: lo scarto e' quello fra mediane di serie diverse
(la mediana della somma non e' la somma delle mediane), non un termine
mancante.

Tre difetti del primo tentativo, corretti prima di produrre questi numeri:

| difetto | conseguenza | correzione |
|---|---|---|
| i batch di uscita venivano distrutti **dentro** la finestra cronometrata (`let (_, m) = collect_batches()`) | il residuo includeva la deallocazione dell'output, che la fase temporale non conta | i batch si legano a un nome e si distruggono **dopo** `elapsed()` |
| `saturating_sub` sulle differenze A/B | le differenze negative diventavano zero e la somma delle quote superava il 100% (fino a 129%) | differenze **con segno**, e partizione ridefinita in modo che non serva alcuna sottrazione satura |
| metriche accese e spente misurate in **campagne separate** | la differenza aveva segno casuale: era deriva dell'host, non effetto | le due configurazioni si **alternano dentro lo stesso ciclo**, con l'ordine invertito a ogni giro, e si confrontano per coppia |

Poi cinque presidi **fail-open** — controlli che passavano anche quando la
misura era rotta. Nessuno cambiava i numeri di questo documento (la
rigenerazione con tutti attivi li lascia entro qualche punto percentuale), ma
ciascuno poteva nascondere il giorno in cui li avrebbe cambiati:

| presidio | come falliva in silenzio | ora |
|---|---|---|
| il residuo usava `saturating_sub` in due punti | `kernel > drenaggio` — cioe' una partizione che non e' una partizione — dava residuo zero, perfettamente credibile | un solo helper con `checked_sub` che **fallisce** dicendo quali termini eccedono |
| il controllo dei campioni percorreva i nodi **osservati** | un nodo assente da **tutte** le ripetizioni non ha una voce, quindi non viene controllato: sparisce dal profilo e dai rami senza traccia | si confronta l'**insieme** dei nodi metricati con quello dichiarato dal piano, e si stampano differenze in entrambe le direzioni |
| sui segmenti si verificava `segmenti <= kernel` | il documento dichiara **uguaglianza**: `<=` sarebbe passato anche se i segmenti avessero smesso del tutto di registrare, cioe' proprio quando la conclusione del §0 andrebbe rivista | si asserisce l'**uguaglianza**, che e' esatta perche' sono le stesse `Duration` sommate nello stesso ordine |
| le campagne di parallelismo erano raccolte con `filter_map` | una campagna che dichiara il parallelismo non misurabile spariva, e si pubblicava un «range su tre processi» calcolato su uno o due | servono **tutte** le campagne, altrimenti il dato e' **non disponibile** e la tabella lo scrive |
| una cella si chiudeva al **solo** superamento della soglia di tempo | una singola ripetizione abbastanza lenta la soddisfa da sola — ed e' la ripetizione contaminata a essere lenta. E' successo: una cella misurata su **un** campione da 989 ms contro ~20 attesi, con la regressione sull'asse righe passata da R² 0,99 a 0,002 | soglia di tempo **e** minimo di ripetizioni, entrambe da soddisfare |

---

## 2. La partizione, alla forma canonica (24 batch x 8192 righe)

| carico | wall | costruzione | kernel | residuo | somma |
|---|---|---|---|---|---|
| `streaming_lineare` | 32,93 ms | 1,62% | 43,3% | **55,3%** | 100,2% |
| `blocking_sort` | 24,32 ms | 2,25% | 71,2% | **26,4%** | 99,9% |
| `blocking_aggregate` | 11,79 ms | 4,71% | 42,6% | **52,5%** | 99,8% |
| `fan_out_tee` | 20,03 ms | 2,89% | 58,4% | **38,7%** | 100,0% |
| `rami_indipendenti` | 210,78 ms | 0,25% | 98,6% | **1,2%** | 100,0% |

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
| `streaming_lineare` | 5,9 µs | 0,058 |
| `blocking_sort` | 10,2 µs | 0,624 |
| `blocking_aggregate` | 0,5 µs | 0,006 |
| `fan_out_tee` | -71,8 µs | 0,767 |
| `rami_indipendenti` | 5,7 µs | 0,856 |

Alla forma canonica, 24 batch, questo termine moltiplicato per 24 vale al
massimo il **5,4%** del residuo, e su `fan_out_tee` la pendenza e'
**negativa** — cioe' li' non c'e' alcun costo per batch da misurare. Il costo
per batch esiste ma **non e'** il tempo mancante, e i R² bassi dicono che su
piu' di un carico la retta non descrive nemmeno bene i dati: il segnale e'
sotto il rumore.

### Asse 2 — batch costanti (24), righe totali variabili (24 576 -> 393 216)

Separa il costo per riga da quello fisso per esecuzione: sul primo asse i due
sono indistinguibili, perche' le righe totali non cambiano mai.

| carico | costo per riga | intercetta | R² |
|---|---|---|---|
| `streaming_lineare` | **90,6 ns** | -1,91 ms | 0,992 |
| `blocking_sort` | **42,5 ns** | -1,65 ms | 0,974 |
| `blocking_aggregate` | **33,1 ns** | -1,27 ms | 0,979 |
| `fan_out_tee` | **37,4 ns** | -2,13 ms | 0,858 |
| `rami_indipendenti` | **12,7 ns** | 0,43 ms | 0,988 |

**Questa e' la risposta.** Con R² fra 0,858 e 0,992 e intercette prossime allo
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
| `string_pad` x k | 0,17 ms | 0,073 ms | 0,690 |
| `formula` x k | 18,46 ms | 1,462 ms | 0,765 |

Una catena di quattro `string_pad` ha un residuo di **0,41 ms**:
l'attraversamento fra nodi, di per se', non costa quasi nulla. Una catena di
quattro `formula` ne ha uno di **23,3 ms**, due ordini di grandezza sopra. Non
e' l'orchestrazione: e' l'operazione.

Sulla ripartizione fra fisso e per nodo dentro la catena di `formula` questa
misura e' **debole**: R² 0,765 su quattro punti. Il termine costante e' grande e
solido (compare gia' con un nodo solo, §4), la pendenza per nodo va presa come
ordine di grandezza e nulla piu'.

---

## 4. Prima causa: `table.formula`

Ogni operazione della catena streaming, misurata **da sola** sullo stesso input
(24 x 8192), stesso percorso fisico `LinearStreaming`, stessi 24 batch:

| piano a nodo singolo | wall | kernel dichiarato | residuo | residuo % |
|---|---|---|---|---|
| `formula` (`valore * 2`) | 21,62 ms | 1,87 ms | 17,79 ms | **87,4%** |
| `formula` (`1`, costante) | 18,82 ms | 0,93 ms | 17,43 ms | **91,9%** |
| `formula` (`id * 2`, intero) | 18,34 ms | 1,40 ms | 16,33 ms | **89,0%** |
| `string_pad` | 13,69 ms | 11,93 ms | 0,24 ms | **1,8%** |
| `filter` | 7,58 ms | 6,67 ms | 0,23 ms | **3,0%** |

Un nodo `table.formula` dichiara **1,87 ms** di kernel e ne costa **21,62**: il
87% del suo tempo non e' nel timer del suo kernel. `string_pad` e `filter`,
sullo stesso input e sullo stesso percorso, hanno un residuo dell'1–3%.

**Non e' il calcolo.** Una formula costante (`1`), che non legge alcuna colonna,
ha lo stesso residuo di `valore * 2`: 17,43 ms contro 17,79. Cambiare tipo
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
`blocking_aggregate` hanno un residuo per riga di 33,1–42,5 ns, dello stesso
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
| `streaming_lineare` | 44,9% | 55,3% | `table.formula` (§4) |
| `blocking_sort` | 73,5% | 26,4% | materializzazione `concat_batches` (§5) |
| `blocking_aggregate` | 47,3% | 52,5% | materializzazione `concat_batches` (§5) |
| `fan_out_tee` | 61,3% | 38,7% | materializzazione del `concat` di convergenza (§5) |
| `rami_indipendenti` | 98,8% | 1,2% | sotto il 2%: nulla da spiegare |

Il 90% di attribuzione **per componente nominato** e' raggiunto su
`rami_indipendenti` soltanto. Sugli altri quattro il residuo e' **spiegato**:
per due carichi e' la materializzazione, localizzata a due righe di
`executor.rs`; per `streaming_lineare` e' `table.formula`, localizzata
all'operazione ma non alla riga.

Cio' che resta esplicitamente aperto:

- **la riga esatta** del costo di `table.formula`. Serve strumentazione dentro
  l'engine;
- **il costo dell'osservabilita'**. Il confronto appaiato fra metriche accese e
  spente e' risolto in **3 celle su 25**: altrove le differenze cambiano
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

1. **`table.formula`** costa ~12 volte cio' che dichiara. Su un piano
   streaming con una formula, e' la voce piu' grossa del wall;
2. **la materializzazione dei nodi bloccanti** e' invisibile alle metriche e
   quindi non e' mai stata contabilizzata come costo.

Entrambe sono costi che il parallelismo fra nodi **sovrapporrebbe** senza
ridurre, mentre potrebbero essere ridotti direttamente. Quale delle due
convenga aggredire per prima non e' deciso da questo documento: prima serve
sapere **dove**, dentro `formula`, va quel tempo.
