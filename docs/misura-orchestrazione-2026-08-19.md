# Misura dell'orchestrazione — linea di base

Ramo `perf/orchestrazione-v1`, da `671214c`. **Nessuna ottimizzazione, nessuna
modifica di comportamento, nessuna API pubblica toccata.** L'unico codice nuovo
e' l'harness di misura, che sta in `examples/` e non entra nella libreria.

Harness: `crates/plenora-engine/examples/misura_orchestrazione.rs`.
Grezzi di questo verbale, **dallo stesso singolo run**:
`docs/misure/baseline-671214c.json` e `.txt`.
Grezzi della verifica di riproducibilita': `docs/misure/varianza/`.

```
cargo build --release --example misura_orchestrazione -p plenora-engine --locked
# in un container SEPARATO, senza compilazione in corso:
./target-linux/release/examples/misura_orchestrazione --json docs/misure/baseline-671214c.json
```

Ambiente: container `rust:1,92,0-slim`, 16 core logici, profilo `release`.
Input sintetico deterministico: 24 batch x 8192 righe = **196 608 righe,
8,28 MiB Arrow**.

---

## 0. Che cosa e' cambiato in questa stesura

Le due stesure precedenti avevano difetti di **metodo**. Correzioni applicate
in questo giro, tutte dentro l'harness:

| difetto | conseguenza | correzione |
|---|---|---|
| la soglia di 1,5 s non era garantita: il tetto di ripetizioni scattava prima | 4 carichi su 5 misurati su meno di 1,5 s cumulati | ripetizioni **a blocchi** finche' il wall cronometrato raggiunge davvero 1,5 s; nel JSON `soglia_raggiunta` e `max_raggiunto` |
| tempo e memoria misurati nello stesso processo | il `VmHWM` includeva i warm-up e decine di esecuzioni consecutive | **processo dedicato** alla memoria, **una sola** esecuzione misurata, `VmHWM` azzerato via `/proc/self/clear_refs` con **verifica** dell'azzeramento; se non riesce, la misura e' dichiarata **non disponibile** |
| «incertezza ±0,28%» | era la sola quantizzazione del tick, presentata come incertezza della misura | rinominata **risoluzione del tick**, e la campagna temporale ripetuta in **3 processi isolati** per carico: si riportano **mediana e range** del parallelismo |
| `filter_map` nel calcolo dei tetti dei rami | un nodo mancante spariva in silenzio e il rapporto restava plausibile | **lookup fail-closed** (`panic` sul nodo assente) e asserzione che ogni nodo abbia **esattamente** un campione per ripetizione |
| tabella scritta a mano leggendo il JSON | possibile divergenza fra grezzo e documento | la tabella del §2 e' **generata dal programma** (campo `tabella_markdown`) e incollata verbatim; il resto del documento e' generato da uno script che legge lo stesso JSON |
| rapporti RSS/governor e «tetto di 90 ripetizioni» | numeri costruiti su misure contaminate | **ritirati** entrambi; sostituiti da quanto segue |

Un settimo difetto e' emerso durante il run stesso, ed e' stato corretto: la
verifica dell'azzeramento confrontava `VmHWM` con il `VmRSS` letto **dopo** la
scrittura su `clear_refs`. Se fra i due istanti l'allocatore restituisce pagine
al sistema, `VmRSS` scende sotto `VmHWM` pur essendo l'azzeramento riuscito —
e `rami_indipendenti` veniva dichiarato non misurabile proprio per questo. Ora
lo stato si legge **a cavallo** della scrittura e il confronto e' col massimo
dei due.

---

## 1. Riproducibilita': quanto valgono questi numeri

Questa sezione viene per prima perche' condiziona la lettura di tutte le altre.

**I tempi assoluti dipendono dallo stato dell'host.** Lo stesso binario, sullo
stesso carico, ha dato **27 ms** con l'host quieto e **60 ms** con la
compilazione nello stesso container; in una terza serie, subito dopo una
campagna completa, ha dato **113–203 ms**. La causa di quest'ultima serie **non
e' stata isolata**: e' registrata come osservazione, non come spiegazione. Per
questo la campagna gira ora in un container **separato** dalla build, dopo una
pausa.

Riproducibilita' misurata a host quieto — 6 campagne di
`streaming_lineare` distribuite su **3 invocazioni di container distinte**
(`docs/misure/varianza/`):

| grandezza | intervallo osservato | ampiezza |
|---|---|---|
| tempo mediano | 25,59 – 28,47 ms | **1,11x** |
| fattore di parallelismo | 0,981 – 0,999 | **1,02x** |
| quota per nodo (`string_pad`) | 64,6% – 65,2% | **0,6 punti** |
| quota per nodo (`filter`) | 22,8% – 23,3% | **0,6 punti** |
| quota per nodo (`formula`) | 11,9% – 12,3% | **0,4 punti** |

Due campagne complete consecutive, entrambe a host quieto, danno per i cinque
carichi 29,60 / 15,11 / 12,38 / 7,53 / 215,59 ms e 31,24 / 16,39 / 14,52 /
8,13 / 220,55 ms: **entro 1,17x**. Solo la seconda e' conservata come artefatto
ed e' quella riportata qui sotto; la prima e' registrata solo in questo verbale.

**Conseguenza operativa.** Le grandezze **normalizzate** — fattore di
parallelismo, quote per nodo, tetti dei rami, rapporto RSS/governor — sono
stabili e si possono usare per decidere. I **tempi assoluti** valgono come
ordine di grandezza: un confronto prima/dopo su di essi richiede che le due
misure girino nelle stesse condizioni, e **una differenza sotto il 20% non e'
distinguibile dal rumore dell'host**.

---

## 2. Il quadro

Tabella **generata dal programma di misura**, incollata verbatim dal campo
`tabella_markdown` del JSON:

| carico | tempo mediano | ripetizioni | soglia 1,5 s | parallelismo (mediana su 3 processi) | range |
|---|---|---|---|---|---|
| `streaming_lineare` | 31.24 ms | 48 | si | **0.99x** | 0.93–0.99 |
| `blocking_sort` | 16.39 ms | 96 | si | **3.55x** | 3.50–3.57 |
| `blocking_aggregate` | 14.52 ms | 96 | si | **1.68x** | 1.65–1.76 |
| `fan_out_tee` | 8.13 ms | 168 | si | **0.98x** | 0.98–0.99 |
| `rami_indipendenti` | 220.55 ms | 8 | si | **1.72x** | 1.69–1.72 |

| carico | picco governato | RSS a freddo (max) | RSS a caldo (min) | rapporto a freddo |
|---|---|---|---|---|
| `streaming_lineare` | 0.76 MiB | 12.00 MiB | 9.04 MiB | 15.85x |
| `blocking_sort` | 15.42 MiB | 19.44 MiB | 16.91 MiB | 1.26x |
| `blocking_aggregate` | 7.71 MiB | 11.47 MiB | 7.98 MiB | 1.49x |
| `fan_out_tee` | 15.44 MiB | 17.75 MiB | 16.83 MiB | 1.15x |
| `rami_indipendenti` | 23.71 MiB | 55.70 MiB | 0.20 MiB | 2.35x |

Tutti e cinque i carichi hanno raggiunto la soglia di **1,5 s cumulati**
cronometrati (`soglia_raggiunta: true`), e nessuno ha toccato il tetto di
`RIPETIZIONI_MAX` (`max_raggiunto: false`). Il numero di ripetizioni varia da
**8** a **168** a seconda della durata del carico: e' la soglia a fissarlo, non
un valore scelto. L'affermazione della stesura precedente, secondo cui
`fan_out_tee` si fermava a 90 ripetizioni prima della soglia, e' **ritirata**:
quel tetto non e' piu' un vincolo effettivo.

Determinismo: **byte IPC identici** su 6 esecuzioni per carico — da 2 248 a
10 496 456 byte confrontati direttamente, non per hash.

---

## 3. Parallelismo: esiste, ma e' tutto dentro i kernel

Mediana su 3 processi isolati, range fra parentesi:

- `streaming_lineare` **0,99x** [0,93–0,99] e `fan_out_tee` **0,98x**
  [0,98–0,99]: questi piani girano **su un core solo**, su una macchina che
  ne ha 16;
- `blocking_sort` **3,55x** [3,50–3,57] e `blocking_aggregate` **1,68x**
  [1,65–1,76]: qui il parallelismo c'e' perche' i kernel usano Rayon al
  proprio interno;
- `rami_indipendenti` **1,72x** [1,69–1,72]: media dei regimi lungo la
  catena, non parallelismo fra rami.

Nessun carico supera **3,55x su 16 core**. Il parallelismo **fra nodi del
DAG** e' zero, come dichiarato (`SerialFused`).

### Il tetto del guadagno, con intervallo

Calcolato su **ogni** ripetizione come
`1 - (max(ramo A, ramo B) + convergenza) / (ramo A + ramo B + convergenza)`,
con i tempi per nodo di quella ripetizione e lookup fail-closed:

| carico | rami | tetto mediano | intervallo | campioni |
|---|---|---|---|---|
| `rami_indipendenti` | A = `a1`+`a2`, B = `b1`+`b2`, conv = `fine` | **19,9%** | 16,9% – 21,7% | 8 |
| `fan_out_tee` | A = `ramo_a`, B = `ramo_b`, conv = `unione` | **36,0%** | 21,7% – 44,0% | 168 |

Il tetto di `rami_indipendenti` e' **stretto** e quindi utilizzabile. Quello di
`fan_out_tee` ha intervallo largo perche' il carico e' breve e un `unione`
occasionalmente lento (fino a 5,3 ms contro 1,1 ms mediani) schiaccia il
rapporto: **la mediana e' significativa, l'estremo inferiore no.**

Restano tetti **ottimistici**: ignorano sincronizzazione e contesa, e i kernel
dentro i rami usano gia' piu' core — eseguirli insieme li mette in
competizione. Sono limiti superiori, non obiettivi.

---

## 4. Memoria: due regimi, una sola esecuzione misurata per ciascuno

Processo dedicato per carico, `VmHWM` azzerato e **verificato** immediatamente
prima dell'esecuzione, RSS e governor letti dalla **stessa** esecuzione. I
rapporti della stesura precedente (1,1x–1,9x sui blocking, 26,7x sullo
streaming) sono **ritirati**: erano costruiti su un `VmHWM` che includeva i
warm-up e decine di esecuzioni.

Le due colonne non sono due stime dello stesso numero, sono **due limiti
osservati in questo banco**:

- **a freddo** — nessun warm-up: l'unica esecuzione misurata paga anche
  l'inizializzazione irripetibile (pool di thread, prime pagine
  dell'allocatore). E' il **piu' alto dei due valori osservati**;
- **a caldo** — dopo il warm-up: l'allocatore ha gia' le pagine che servono,
  quindi l'incremento tende a zero. E' il **piu' basso dei due**.

**Non sono limiti universali.** Sono cio' che questi due regimi del benchmark
producono, con questo allocatore, questo input e questa sequenza di esecuzioni.
Un altro allocatore, un input piu' grande o un uso che non parte mai a freddo
possono cadere fuori da entrambi: il fabbisogno reale di un processo che esegue
il piano una volta sola non e' stato misurato, e non e' deducibile da queste
due colonne.

| carico | picco governato | RSS a freddo | RSS a caldo | rapporto a freddo |
|---|---|---|---|---|
| `streaming_lineare` | 0,76 MiB | 12,00 MiB | 9,04 MiB | **15,85x** |
| `blocking_sort` | 15,42 MiB | 19,44 MiB | 16,91 MiB | **1,26x** |
| `blocking_aggregate` | 7,71 MiB | 11,47 MiB | 7,98 MiB | **1,49x** |
| `fan_out_tee` | 15,44 MiB | 17,75 MiB | 16,83 MiB | **1,15x** |
| `rami_indipendenti` | 23,71 MiB | 55,70 MiB | 0,20 MiB | **2,35x** |

Il caso a caldo di `rami_indipendenti` (**0,20 MiB**) e' l'artefatto in forma
pura: dopo il warm-up il fabbisogno di quel carico e' gia' interamente
residente, quindi la singola esecuzione misurata non fa crescere l'RSS. Non
significa che il carico costi 0,20 MiB; significa che **quel** valore, per
quel carico, non e' informativo.

Il caso estremo utile e' `streaming_lineare`: governa **0,76 MiB** perche' non
materializza nulla — il governor vede solo il batch in transito — mentre
il processo cresce di **12,00 MiB** per i buffer dei kernel, l'allocatore e il
pool. Sui carichi che materializzano, dove il governor conta le
materializzazioni, il rapporto a freddo sta fra **1,15x** e **2,35x**.

E' esattamente cio' che DER-011 dichiara: `max_memory_bytes` **non e' un tetto
duro** sull'occupazione del processo. Quanto resta fuori dal governo dipende dal
carico, e questa misura lo quantifica per cinque forme.

---

## 5. Byte Arrow attraversati

Rapporto `somma(bytes_in per nodo) / byte dell'input`:
`blocking_*` 1,00x, `fan_out_tee` 2,93x, `streaming_lineare` 3,73x,
`rami_indipendenti` 4,59x.

**Non e' un conteggio di copie**, e non va spacciato per tale: misura quante
volte i dati sono *osservati* da un nodo. Con tre nodi in catena il rapporto e'
~3x anche se nessun buffer viene duplicato, perche' Arrow condivide i buffer per
riferimento. Per distinguere attraversamenti da copie servirebbe un allocatore
tracciante: **non e' stato fatto**.

---

## 6. Backpressure e spill

Ogni carico rieseguito con `max_memory_bytes` pari **alla meta' del picco
governato mediano**:

| carico | esito |
|---|---|
| `streaming_lineare` | riesce |
| `blocking_sort` | **`ResourceLimit`** |
| `blocking_aggregate` | **riesce spillando** |
| `fan_out_tee` | **`ResourceLimit`** |
| `rami_indipendenti` | **`ResourceLimit`** |

Il fatto che conta: **lo spill e' preventivo, non reattivo**. L'attivazione e'
su soglia stimata ai punti di dispatch (ADR-0002), quindi `sort` con budget
dimezzato *fallisce* dove `aggregate` *spilla* — non perche' `sort` non sappia
spillare, ma perche' la sua soglia preventiva non e' scattata e una reservation
fallita non ha modo di tornare indietro. `ReservationResult::MustSpill` e
`RetryAfterProgress` esistono nell'API e non sono **mai emessi**.

---

## 7. Dove sta il tempo

`rami_indipendenti`, mediana per nodo su 8 ripetizioni:

| nodo | op | mediana | intervallo |
|---|---|---|---|
| `a2` | `table.distinct` | **110,97 ms** | 96,24 – 126,63 |
| `fine` | `table.join` | **49,24 ms** | 46,12 – 62,07 |
| `b1` | `table.aggregate` | **39,18 ms** | 35,71 – 45,02 |
| `a1` | `table.sort` | **14,16 ms** | 12,53 – 17,08 |
| `b2` | `table.sort` | **2,94 ms** | 2,01 – 4,08 |

`streaming_lineare`: `string_pad` 10,62 ms, `filter` 3,66 ms, `formula` 1,94 ms,
su un wall mediano di 31,24 ms.

### Quanto del wall e' coperto dai tempi per nodo

Rapporto fra la **somma delle mediane per nodo** e la **mediana del wall**.
Non e' la mediana del rapporto — il JSON conserva per nodo solo mediana, minimo
e massimo, non la serie — quindi vale come stima, non come statistica esatta:

| carico | somma nodi | wall mediano | copertura |
|---|---|---|---|
| `streaming_lineare` | 16,22 ms | 31,24 ms | **52%** |
| `blocking_sort` | 14,39 ms | 16,39 ms | **88%** |
| `blocking_aggregate` | 6,97 ms | 14,52 ms | **48%** |
| `fan_out_tee` | 5,28 ms | 8,13 ms | **65%** |
| `rami_indipendenti` | 216,50 ms | 220,55 ms | **98%** |

**Questo corregge la stesura precedente**, che affermava che «la somma dei
tempi per nodo copre la quasi totalita' del wall time». E' vero solo dove i
kernel sono lunghi: **98%** su `rami_indipendenti` e **88%** su
`blocking_sort`. Sui carichi corti — `blocking_aggregate` **48%**,
`streaming_lineare` **52%** — meta' del wall sta **fuori** dai tempi per
nodo.

Dove stia quella meta' **non e' stato stabilito da questa misura**: i candidati
sono il dispatch per batch, la costruzione dell'output e la raccolta finale,
ma nessuno dei tre e' strumentato. E' il primo punto da chiarire nella fase
successiva, perche' cambia la risposta: se il costo fuori dai nodi e' fisso per
esecuzione, un orchestratore parallelo non lo tocca; se e' per batch, lo tocca
eccome.

Quello che resta stabilito: sui carichi dominati dai kernel il tempo e' **nei
kernel**, e li' un orchestratore parallelo puo' **sovrapporre** i costi, non
ridurli.

---

## 8. Che cosa questa misura NON dice

- **copie Arrow reali**: misurati gli attraversamenti, non le allocazioni;
- **profilazione a livello di funzione**: nessun `perf`, nessun flamegraph; il
  profilo e' per nodo, la granularita' che l'engine gia' espone;
- **carichi geo**: nessuno dei cinque tocca i kernel geo, dove esistono la
  fusione (ADR-0012) e i suoi fallback;
- **input reali**: dati sintetici, distribuzione uniforme;
- **scala**: un solo ordine di grandezza (8,28 MiB). Il comportamento a 1 GiB
  — dove lo spill conta davvero — non e' stato osservato;
- **contesa fra piani**: un solo piano per volta, 16 core nel container;
- **dove sta il wall non coperto dai nodi**: fino a meta' su due carichi su
  cinque, e questa misura non lo attribuisce (vedi §7);
- **tempi assoluti come base di confronto**: vedi §1. Riproducibili entro
  1,17x a host quieto, non oltre.

---

## 9. Fatti verificati

1. Il parallelismo **fra nodi** e' zero; quello **dentro i kernel** arriva a
   **3,55x** (sort) e **1,68x** (aggregate), ed e' assente nelle catene
   streaming (0,99x) e nel tee (0,98x). Range su 3 processi isolati per
   carico.
2. Il tetto del guadagno da parallelismo fra rami e' **19,9%** [16,9–21,7]
   su `rami_indipendenti`, e **36,0%** (mediana, intervallo largo) su
   `fan_out_tee`.
3. Il tempo sta **nei kernel** sui carichi dominati dai kernel — copertura
   **98%** su `rami_indipendenti`, **88%** su `blocking_sort` — ma
   **non** sui carichi corti, dove scende a **48%** e **52%**. La quota
   non coperta non e' attribuita: e' il primo punto da strumentare.
4. Il rapporto fra incremento di RSS e picco governato, a freddo, sta fra
   **1,15x** e **2,35x** sui carichi che materializzano, e vale **15,85x** su
   quello streaming, che non materializza nulla. Il 3,7x della prima stesura e
   i rapporti della seconda sono ritirati.
5. Lo spill e' **preventivo e non reattivo**: `MustSpill` e
   `RetryAfterProgress` non sono mai emessi.
6. L'esecuzione e' **deterministica byte a byte** su tutti e cinque i carichi.
7. La misura e' riproducibile **entro 1,11x** sui tempi e **entro 0,6 punti**
   sulle quote per nodo, a host quieto e con la compilazione fuori dal
   container di misura.
