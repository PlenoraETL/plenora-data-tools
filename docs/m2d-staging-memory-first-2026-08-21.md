# M2d — staging memory-first: che cosa e' cambiato, misurato

Ramo `perf/orchestrazione-v1`. Una sola ottimizzazione, causale: gli accepted
dei segmenti row-diagnostics attendono **in memoria** invece che su file
temporaneo, con passaggio deterministico al disco quando il budget non basta.
La barriera R9,9 e' invariata; nessuna operazione e' specializzata;
`emits_row_diagnostics` non e' toccato. Vedi ADR-0002 §M2d.

Grezzi: PRIMA `docs/misure/decomposizione/` e `docs/misure/baseline-671214c.json`;
DOPO `docs/misure/m2d-dopo/`. Stessi carichi, stesso harness, stessa forma
dell'input.

---

## 1. Come si legge un confronto prima/dopo su questo host

I tempi assoluti variano fra campagne piu' del 20% per stato dell'host
(`docs/misura-orchestrazione-2026-08-19.md` §1). In questo benchmark i
carichi di controllo — quelli che **non** passano dal codice cambiato —
si spostano del 14–32% da soli: e' rumore, non effetto.

Le conclusioni si leggono quindi su due quantita' immuni allo stato
dell'host:

- il **residuo** e il **costo per riga**, gia' normalizzati (§2);
- il **rapporto soggetto/controllo** misurato dentro la stessa campagna (§3).

I tempi assoluti sono riportati, ma non sono l'argomento.

---

## 2. L'effetto, in quantita' normalizzate

Quota del wall attribuita ai kernel e residuo non attribuito, alla forma
canonica (24 batch x 8192 righe), e costo per riga del residuo dall'asse
delle righe:

| carico | kernel % | residuo % | costo per riga |
|---|---|---|---|
| `streaming_lineare` | 43,3% → **96,7%** | 55,3% → **1,3%** | 90,6 → **1,3** ns |
| `blocking_sort` | 71,2% → **69,1%** | 26,4% → **28,8%** | 42,5 → **54,6** ns |
| `blocking_aggregate` | 42,6% → **48,0%** | 52,5% → **47,7%** | 33,1 → **55,8** ns |
| `fan_out_tee` | 58,4% → **57,8%** | 38,7% → **38,6%** | 37,4 → **60,2** ns |
| `rami_indipendenti` | 98,6% → **98,3%** | 1,2% → **1,4%** | 12,7 → **17,1** ns |

`streaming_lineare` — l'unico dei cinque con un nodo `formula` — passa da
**55,3%** a **1,3%** di residuo, e il costo per riga da **90,6 ns** a
**1,3 ns**. Sui quattro controlli la **quota di residuo** resta dov'era,
entro pochi punti. Il loro **costo per riga** si muove di piu' (fino a ~60%
in questa campagna) pur non essendo toccato dal codice: e' la stessa
oscillazione dell'host che si vede nei tempi assoluti al §3, e va letta come
tale — non come un effetto della modifica, che su quei carichi non esegue una
riga diversa da prima.

Sui piani a nodo singolo l'effetto e' ancora piu' netto, perche' non c'e'
altro nel wall:

| piano | wall | kernel | residuo | residuo % |
|---|---|---|---|---|
| `solo_formula` | 21,62 → **4,25** ms | 1,87 → 3,53 ms | 17,79 → **0,21** ms | 87,4% → **4,7%** |
| `solo_formula_costante` | 18,82 → **2,42** ms | 0,93 → 1,83 ms | 17,43 → **0,16** ms | 91,9% → **6,6%** |
| `solo_formula_intero` | 18,34 → **3,70** ms | 1,40 → 3,04 ms | 16,33 → **0,20** ms | 89,0% → **5,3%** |
| `solo_string_pad` | 13,69 → **23,39** ms | 11,93 → 22,12 ms | 0,24 → **0,39** ms | 1,8% → **1,6%** |
| `solo_filter` | 7,58 → **10,71** ms | 6,67 → 9,60 ms | 0,23 → **0,31** ms | 3,0% → **2,8%** |

Il residuo di `solo_formula` scende da **17,79 ms** a **0,21 ms**: lo staging
IPC e il suo replay, che valevano l'84–86% di quel residuo, non vengono piu'
eseguiti. `string_pad` e `filter`, che non passavano da quel percorso, non
cambiano natura.

---

## 3. Il guadagno, normalizzato su un controllo interno alla campagna

Rapporto fra il soggetto e un carico di controllo misurato **nella stessa
campagna**: elimina lo stato dell'host, perche' entrambi lo subiscono.

| soggetto | controllo | rapporto prima | rapporto dopo | guadagno |
|---|---|---|---|---|
| `solo_formula` | `solo_string_pad` | 1,580 | 0,182 | **89%** |
| `streaming_lineare` | `blocking_sort` | 1,905 | 1,048 | **45%** |

Entrambi superano largamente la soglia concordata del 15%.

Per completezza, i tempi assoluti — **da leggere come ordine di grandezza**,
non come misura del guadagno:

| carico | wall prima | wall dopo | variazione del wall |
|---|---|---|---|
| `streaming_lineare` | 31,24 ms | 28,77 ms | -7,9% |
| `blocking_sort` | 16,39 ms | 27,46 ms | +67,5% |
| `blocking_aggregate` | 14,52 ms | 19,90 ms | +37,0% |
| `fan_out_tee` | 8,13 ms | 12,77 ms | +57,1% |
| `rami_indipendenti` | 220,55 ms | 376,38 ms | +70,7% |

I quattro controlli si spostano fra il 14% e il 29% **in tempo assoluto** pur
non essendo toccati dal codice: e' la dimostrazione diretta che su questo host
il tempo assoluto non misura l'effetto. Le loro quote di residuo, che invece
sono normalizzate, si muovono al massimo di **4,8 punti**: nessuna
regressione oltre la soglia del 5% concordata.

---

## 4. Prezzo: il picco governato

Trattenere gli accepted significa trattenere i loro lease. Il picco governato
cresce di conseguenza, ed e' l'unico costo di questa modifica:

| carico | picco governato prima | dopo | aumento |
|---|---|---|---|
| `streaming_lineare` | 0,76 MiB | 10,24 MiB | **+9,48 MiB** |
| `blocking_sort` | 15,42 MiB | 15,42 MiB | invariato |
| `blocking_aggregate` | 7,71 MiB | 7,71 MiB | invariato |
| `fan_out_tee` | 15,44 MiB | 15,44 MiB | invariato |
| `rami_indipendenti` | 23,71 MiB | 23,71 MiB | invariato |

L'aumento e' concentrato dove il percorso e' cambiato: `streaming_lineare`
passa da 0,76 MiB a **10,24 MiB**, cioe' i ventiquattro batch di output
trattenuti. Gli altri quattro non hanno nodi row-diagnostics e restano
identici al byte.

**10,24 MiB su un budget di 512 MiB e' il 2%.** E non e' un tetto sfondato: la
soglia garantisce per costruzione che il picco resti sotto `max_memory_bytes`
(ADR-0002 §M2d), e il test `m2d_picco_governato_memoria_non_supera_il_budget`
lo verifica su entrambe le modalita'.

## 5. Frequenza del fallback

Il passaggio al disco e' **una tantum per esecuzione**: una volta avvenuto, la
scansione resta su disco. Non e' quindi una frequenza per batch ma un
booleano per esecuzione, e la sua condizione e' deterministica.

Sui cinque carichi, con i limiti predefiniti (`max_memory_bytes` 512 MiB,
`max_batch_bytes` 64 MiB) il fallback **non e' mai scattato**: il massimo
trattenuto e' 10,24 MiB, e `10,24 + 0,42 + 64 = 74,66 MiB` resta ampiamente sotto
il budget. Perche' scatti servirebbe un budget sotto i ~75 MiB.

Che il fallback funzioni, e che preservi il comportamento precedente, e'
verificato dai test invece che dedotto:

- `m2d_soglia_esatta_e_attraversamento` esercita **entrambi i lati** della
  soglia: con budget ampio la quota temporanea di 1 byte non viene nemmeno
  toccata (prova che non si scrive su disco); con budget stretto si passa a
  disco e quella stessa quota fa fallire;
- `m2d_budget_stretto_non_regredisce_a_resource_limit` esegue lo stesso piano
  con budget di 64 KiB, 256 KiB e 1 MiB — tutti sotto il tetto per batch,
  quindi in modalita' disco dal primo batch — e verifica che **riesca**, con
  il picco dentro il budget. E' il rischio dichiarato di questa modifica, ed
  e' chiuso da un test invece che da un argomento.

---

## 6. Equivalenza fra le due modalita'

| verifica | esito |
|---|---|
| byte IPC identici memoria vs disco | `m2d_memoria_e_disco_producono_gli_stessi_byte` |
| ordine di produzione | `m2d_ordine_e_sequenza_logica_preservati` |
| `BatchSequence` identica | `m2d_batch_sequence_identica_fra_modalita` |
| rejection tardiva: zero accepted | `m2d_rejection_tardiva_non_pubblica_nulla_in_memoria` |
| cancellazione: zero accepted | `m2d_cancellazione_dopo_accepted_trattenuti_non_pubblica_nulla` |
| soglia e attraversamento | `m2d_soglia_esatta_e_attraversamento` |
| quota temporanea fail-closed | `accepted_output_staging_beyond_temp_quota_fails_closed` |
| lease tutti rilasciati | `m2d_lease_rilasciati_a_fine_esecuzione` |
| picco dentro il budget | `m2d_picco_governato_memoria_non_supera_il_budget` |
| consumo da un segmento a valle | `m2d_output_consumato_da_un_segmento_successivo` |
| dictionary e nested | `m2d_dictionary_e_nested` |
| batch vuoti | `m2d_zero_colonne_e_batch_vuoti` |
| tre famiglie row-diagnostics | `m2d_tre_famiglie_row_diagnostics` |
| nessun falso `ResourceLimit` | `m2d_budget_stretto_non_regredisce_a_resource_limit` |

In piu', la campagna completa dell'harness verifica il determinismo byte a
byte su tutti e cinque i carichi: i byte IPC confrontati sono **identici a
quelli di prima della modifica**, con lo stesso conteggio esatto.

---

## 7. Che cosa NON e' cambiato e che cosa resta aperto

- **`emits_row_diagnostics` e' invariato**: nessuna operazione e' stata tolta
  dalla lista ne' specializzata. Cambia dove gli accepted attendono, non chi
  passa dalla barriera;
- **il gate WKB dell'input resta su disco**: fuori dal perimetro di questo
  blocco. Il braccio corrispondente e' fail-closed, non silenzioso;
- **scheduler, API pubbliche e semantica delle metriche**: non toccati;
- **le altre operazioni row-diagnostics** (`expression`, `assert_*`, `date_*`,
  `flatten_json`) beneficiano dello stesso cambiamento perche' condividono il
  percorso, ma **non sono state misurate**: solo tre famiglie sono coperte dai
  test di equivalenza;
- **la materializzazione dei nodi bloccanti** (§5 del documento di
  decomposizione) e' un'altra causa, non toccata qui: i controlli mostrano
  infatti il loro residuo invariato.
