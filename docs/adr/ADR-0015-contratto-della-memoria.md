# ADR 15 — Che cosa promette `max_memory_bytes`: contratto della memoria

- **Stato**: **ACCETTATA** dal maintainer il 2026-08-21. Nessuna strategia è
  ancora implementata e nessuna API è ancora rinominata: questo ADR fissa il
  contratto, l'attuazione è un blocco separato.
- **Decisioni collegate**: ADR 2 (resource accounting, §M2d permesso
  atomico), ADR 6 (semantica limiti), ADR 7 (publish atomico), ADR-0012
  (fusione geo, reservation preventiva del gruppo fuso)
- **Deroga interessata**: DER-011 (`max_memory_bytes` non è un tetto duro)
- **Ramo**: `design/der011-memory-contract`, da `main@4942181`
- **Perimetro**: sola analisi. Nessuna modifica eseguibile, nessuna
  rinominazione di API, nessun lavoro su M3

Questa decisione precede il freeze dell'API Rust perché determina il
significato pubblico di `Limits.max_memory_bytes`. Oggi quel nome promette un
tetto e DER-011 dichiara che il tetto non c'è: il divario va chiuso scegliendo
**che cosa vogliamo promettere**, non solo quanto codice convertire.

## 0. La decisione

Il maintainer ha deciso, il 2026-08-21:

- **contratto a due livelli: accettato**;
- **accounting nell'allocatore: scartato**;
- **`max_memory_bytes` sarà rinominato `max_governed_memory_bytes`** prima del
  freeze. Il nome attuale promette un tetto sull'intero processo che
  in-process non possiamo mantenere; si è già dentro un major bump, e
  conservare un nome ambiguo per compatibilità sarebbe un errore;
- **il profilo isolato avrà un limite distinto**, provvisoriamente
  `hard_process_memory_bytes`, valido **esclusivamente** quando l'esecuzione è
  isolata;
- **nessun alias ambiguo** nella nuova versione del piano e dell'API. La
  compatibilità con i piani storici sarà gestita esplicitamente dalla
  migrazione di versione, non da un nome che accetta entrambe le letture.

Il resto di questo documento è l'analisi che ha portato alla decisione; la
§6 fissa le precisazioni che ne fanno parte.

---

## 1. Da dove si parte

Il permesso atomico (ADR-0002 §M2d, merge `4942181`) ha chiuso il prerequisito
concorrente: verifica e prenotazione sono una sola operazione, la contabilità
è linearizzabile, e i cancelli impediscono di dichiarare successo con una
contabilità incoerente.

Non ha chiuso DER-011. Il censimento
(`docs/der011-censimento-2026-08-21.md`) conta, sul solo percorso DAG, **due**
siti in cui la reservation precede l'allocazione, **otto** in cui la segue e
**uno** in cui non c'è. Fuori dal DAG restano scoperti il percorso legacy, i
kernel senza preflight e tutte le allocazioni interne di Arrow, GEOS e PROJ.

### Tre fatti verificati, non assunti

Sono stati controllati sul codice e sulle dipendenze effettivamente in uso,
perché le quattro strategie si valutano su questi e non su come ci si aspetta
che funzionino.

1. **Arrow alloca dall'allocatore globale di Rust.** `arrow-buffer 59.1.0`
   chiama `std::alloc::alloc` e `alloc_zeroed`
   (`src/buffer/mutable.rs`). Un `#[global_allocator]` **vedrebbe** quelle
   allocazioni.
2. **Arrow non ha un allocatore iniettabile.** Il tratto `Allocation` di
   `arrow-buffer` (`src/alloc/mod.rs`) descrive la *proprietà di una regione
   già allocata altrove* — serve alle FFI — e non è un punto di estensione per
   sostituire l'allocatore. Non esiste un modo supportato di dire ad Arrow
   «alloca da qui».
3. **Arrow, se l'allocazione fallisce, ABORTISCE.** `handle_alloc_error` è
   chiamato in tre punti di `MutableBuffer`. Un allocatore che rifiuta
   restituendo null non produce un `Result`: termina il processo. Questo
   fatto, da solo, decide la strategia 3.

### Chi è dentro il perimetro, oggi e domani

- **Rust in-process**: engine DAG e kernel tabellari, tutto sopra Arrow;
- **CLI**: stesso processo dell'engine, più il percorso legacy con
  `ammissione_output`, che rifiuta la *pubblicazione* dopo che il kernel ha
  costruito il risultato;
- **GEOS e PROJ**: librerie C/C++ dietro i feature `geos-backend` e
  `proj-backend`. Allocano con il proprio `malloc`, **fuori** dall'allocatore
  Rust;
- **PyO3**: **non presente nel workspace** — verificato, nessun crate lo
  dichiara. È un consumatore futuro, e l'analisi lo tratta come tale: quando
  arriverà, l'interprete Python e le sue estensioni allocheranno per conto
  proprio, fuori da qualunque contabilità della libreria.

---

## 2. Le quattro strategie

### 2.1 Preflight per operazione

Stima a priori dei byte del risultato, con un modello scritto per la singola
operazione, e rifiuto **prima** di allocare. Esiste già:
`plenora_kernels_table::preflight_output_bytes`, usato da `cross_join`,
`concat`, `concat_by_name` e `melt`.

| ambito | copre? |
|---|---|
| Rust / kernel tabellari | **parzialmente**: solo i buffer del risultato dichiarati dal modello |
| CLI | sì, per le operazioni coperte |
| PyO3 (futuro) | no |
| Arrow (temporanei interni) | **no**: tabelle hash, buffer di crescita, copie intermedie |
| GEOS / PROJ | no |

**`ResourceLimit` invece di OOM:** sì, e con il messaggio migliore di tutte le
strategie — dice quale operazione, quante righe e quanti byte stimati. Ma solo
quando la stima è corretta e l'esplosione è nel risultato.

**Costo:** trascurabile a runtime (una moltiplicazione e un confronto). Il
costo vero è **umano e ricorrente**: un modello per operazione, da mantenere
allineato all'implementazione. Un kernel che cambia strategia interna senza
aggiornare il modello lo rende silenziosamente ottimista.

**API e packaging:** nessun impatto.

**Limite strutturale:** copre solo dove il conteggio delle righe di output
**precede** l'allocazione. Per `sort`, `distinct`, `aggregate`, `join` la
dimensione dell'output si conosce lavorando, non prima.

### 2.2 Permesso preventivo generalizzato

Estendere a tutti i siti ciò che M2d ha fatto per il percorso
row-diagnostics: prendere il permesso **prima** di chiamare il kernel, con un
maggiorante, e ritagliarlo sulla dimensione reale dopo. Il primitivo esiste già
(`MemoryGovernor::permesso`, `MemoryPermit::ritaglia`).

| ambito | copre? |
|---|---|
| Rust / DAG | **sì**, per ogni sito convertito: i nove rimanenti censiti |
| CLI | sì per il DAG; il percorso legacy va convertito a parte |
| PyO3 (futuro) | no |
| Arrow (temporanei interni) | **no** |
| GEOS / PROJ | no |

**`ResourceLimit` invece di OOM:** sì per la memoria **governata**, e in modo
uniforme: un solo meccanismo, un solo messaggio, nessun modello per
operazione da mantenere.

**Costo:** un mutex per sito per batch — misurato su M2d: il rapporto
normalizzato `streaming_lineare`/`blocking_sort` passa da 1,065 a 1,078,
dentro il rumore. Il costo reale non è il tempo ma il **picco**: prenotare un
maggiorante significa trattenerlo davvero. Su `streaming_lineare` il picco
governato è passato da 10,24 a 73,83 MiB, perché il maggiorante disponibile è
`max_batch_bytes` (64 MiB di default).

Generalizzarlo peggiora questo punto: **ogni** sito convertito trattiene il
proprio maggiorante, e più maggioranti coesistono. Con maggioranti grossolani
il budget si esaurisce per prenotazioni che non verranno usate, e piani oggi
eseguibili verrebbero rifiutati — un falso `ResourceLimit`, cioè il difetto
che M2d ha lavorato per evitare. Rendere i maggioranti stretti riporta al
problema della strategia 2.1: una stima per operazione.

**API e packaging:** nessun impatto se il permesso resta interno.

### 2.3 Accounting nell'allocatore

Un `#[global_allocator]` che conta e rifiuta oltre il budget.

| ambito | copre? |
|---|---|
| Rust / DAG e kernel | **sì, tutto**, temporanei compresi |
| CLI | sì |
| PyO3 (futuro) | **no** per l'interprete e le estensioni; sì per il codice Rust |
| Arrow | **sì** — alloca da `std::alloc` (fatto 1) |
| GEOS / PROJ | **no** — allocano dal `malloc` di sistema (fatto verificato: sono binding FFI a librerie C/C++) |

**`ResourceLimit` invece di OOM: NO.** È il punto che chiude la questione.
Arrow chiama `handle_alloc_error` quando l'allocazione fallisce (fatto 3), e
`handle_alloc_error` **aborta il processo**. Un allocatore che rifiuta non
produce un errore diagnosticabile: produce un abort, che è *peggio* dell'OOM
attuale — nessuno stack, nessun messaggio, nessun `execution_id`.

Ci sono altri due problemi, indipendenti da questo:

- **non è per esecuzione.** Un allocatore globale è, appunto, globale: conta
  tutto il processo. Con due esecuzioni concorrenti — che è il senso di M3 —
  non sa a chi attribuire i byte, e `max_memory_bytes` è un limite **di
  piano**, non di processo;
- **non vede la memoria nativa.** GEOS e PROJ sono esattamente il caso in cui
  la memoria esplode senza preavviso, e sono ciechi a questa strategia.

**Costo:** un'operazione atomica per ogni allocazione del processo, incluse
quelle che nulla hanno a che vedere con i dati.

**API e packaging:** invasivo. Un `#[global_allocator]` è una proprietà del
**binario finale**, non di una libreria: `plenora-engine` non può imporlo ai
propri consumatori, e un host Python non lo accetterebbe.

### 2.4 Isolamento dell'esecuzione con limite del sistema operativo

Eseguire il piano in un processo separato con un limite imposto dal sistema:
`RLIMIT_AS`/`RLIMIT_DATA` o un cgroup su Linux, un job object su Windows.

| ambito | copre? |
|---|---|
| Rust / DAG e kernel | **sì** |
| CLI | **sì** |
| PyO3 (futuro) | **sì**, se l'esecuzione è nel processo isolato |
| Arrow | **sì** |
| GEOS / PROJ | **sì** — è l'unica strategia che li copre |

**`ResourceLimit` invece di OOM:** **sì, ma da fuori**. Il processo figlio
muore o fallisce l'allocazione; il padre osserva il codice di uscita e produce
un errore diagnosticabile. Non è un `ResourceLimit` prodotto *dentro*
l'esecuzione, e non dice quale nodo lo ha causato: dice che quell'esecuzione
ha superato il tetto.

**Costo:** avvio di un processo per esecuzione, e il passaggio dei dati
attraverso un confine. Su piani brevi il costo dell'avvio può dominare; su
piani lunghi è trascurabile. Il trasporto dei batch è già risolto: l'IPC Arrow
esiste e `write_ipc_file` pubblica atomicamente (ADR 7).

**API e packaging:** è un **profilo di esecuzione**, non un cambio di API
della libreria. Richiede però codice specifico per sistema operativo e una
strategia di distribuzione dell'eseguibile figlio.

---

## 3. Un tetto duro per singola esecuzione è realizzabile in-process?

**No.** Tre ostacoli, ciascuno sufficiente da solo:

1. **La memoria nativa è invisibile.** GEOS e PROJ allocano dal `malloc` di
   sistema. Nessuna contabilità in Rust — né reservation, né allocatore
   globale — le vede. E sono proprio le operazioni geometriche il caso in cui
   una singola riga patologica può far esplodere l'occupazione;
2. **Il fallimento di allocazione non è recuperabile.** Arrow chiama
   `handle_alloc_error`, che aborta. Anche potendo contare ogni byte, il
   momento in cui si supera il limite non è un punto in cui si possa
   *restituire un errore*: è un punto in cui il processo finisce;
3. **«Per esecuzione» e «per processo» non coincidono.** Un allocatore globale
   conta il processo. Con esecuzioni concorrenti — l'obiettivo di M3 — non
   c'è modo di attribuire un'allocazione al piano che l'ha causata senza un
   contesto che l'allocatore non ha.

Convertire i nove siti rimanenti (strategia 2.2) migliorerebbe realmente la
copertura, ma **non renderebbe vero** «il processo non supera N byte».
Dichiarare `max_memory_bytes` un tetto duro dopo quella conversione sarebbe
ancora falso — con l'aggravante di essere falso *dopo* averlo promesso.

---

## 4. Il contratto a due livelli

Nessuna delle quattro strategie basta da sola, e la scelta non è fra loro ma
fra **promettere meno e mantenerlo** oppure promettere un tetto e non averlo.
Il maintainer ha scelto il primo (§0); i confini di ciascun livello sono
fissati in §5.

### Livello 1 — Budget di ammissione governato (nella libreria)

`max_governed_memory_bytes` governa la **memoria che la libreria controlla**: i batch
che attraversano gli archi del DAG e le materializzazioni intermedie. È un
budget di **ammissione**, non un tetto sul processo: rifiuta il lavoro che *sa*
non starci, e non promette nulla su ciò che non vede.

Realizzato con la strategia **2.2**, permesso preventivo generalizzato, dove
la conversione paga: i siti in cui il maggiorante è stretto e noto. Dove il
maggiorante sarebbe grossolano, la conversione **peggiora** le cose — trattiene
quota che non userà — e lì resta la reservation successiva, dichiarata.

La strategia **2.1** resta complementare dove esiste già e dove il conteggio
delle righe precede l'allocazione: intercetta le esplosioni di ordini di
grandezza prima di allocare, che è esattamente ciò che il livello 1 non può
fare da solo.

La strategia **2.3 è scartata**: non produrrebbe `ResourceLimit` ma abort, non
sarebbe per esecuzione, non vedrebbe la memoria nativa, e imporrebbe una
scelta globale ai consumatori della libreria.

### Livello 2 — Tetto duro solo nel profilo isolato

Il tetto duro esiste **solo** eseguendo il piano in un processo con un limite
del sistema operativo (strategia 2.4), dichiarato come
`hard_process_memory_bytes`. È l'unica configurazione in cui il contenimento
vale anche per Arrow, GEOS, PROJ e per qualunque host futuro, e in cui il
fallimento è osservabile invece che catastrofico.

«Tetto duro» va però letto con §5.4 accanto: su Linux il kernel ammette
superamenti temporanei, quindi la promessa è **contenimento e attribuzione**,
non «mai un byte oltre N». E vale su Linux e Windows, non su macOS (§5.6).

### I nomi

`max_memory_bytes` promette il livello 2 e realizza il livello 1. Il maintainer
ha scelto di **rinominare** invece di ridefinire il contratto sotto lo stesso
nome (§0):

| livello | nome | dove vale |
|---|---|---|
| 1 | `max_governed_memory_bytes` | sempre: è il budget di ammissione della memoria che la libreria controlla |
| 2 | `hard_process_memory_bytes` | **solo** nel profilo isolato; altrove non è applicabile e non va accettato in silenzio |

I due limiti non sono lo stesso numero espresso in modi diversi e non devono
poter essere confusi: il primo governa ciò che la libreria decide di
ammettere, il secondo è ciò che il sistema operativo impone al processo
worker. Un piano che li dichiara entrambi ne usa uno per profilo.

**Nessun alias.** La nuova versione del piano e dell'API non accetta
`max_memory_bytes`: un nome che continua a funzionare è un nome che continua a
promettere. I piani storici sono affare della migrazione di versione, che è
esplicita e può dire che cosa sta traducendo e in che cosa.

---

## 5. Precisazioni che fanno parte della decisione

Sei punti, posti dal maintainer all'accettazione. Non sono chiose: delimitano
ciò che i due limiti promettono, e senza di essi il contratto tornerebbe a
promettere piu' di quanto mantiene.

### 5.1 Il limite governato è un budget di ammissione

`max_governed_memory_bytes` **non è** l'RSS del processo e **non è** la
memoria totale usata dall'esecuzione. È la quota che la libreria decide di
ammettere per i batch che attraversano gli archi del DAG e per le
materializzazioni intermedie. Non comprende i temporanei interni dei kernel,
le allocazioni di Arrow che nessun lease copre, né un solo byte di GEOS o
PROJ.

Chi confronta questo numero con quello che vede in `top` troverà una
differenza, e la differenza non è un difetto: è la parte non governata, che
DER-011 dichiara.

### 5.2 Il limite isolato copre il worker, non il resto

`hard_process_memory_bytes` vale per il **processo worker** che esegue il
piano. Non copre:

- la memoria del **processo padre**, che resta soggetta al solo livello 1;
- i **buffer IPC** del trasporto fra i due, che vivono da entrambi i lati del
  confine e non sono attribuibili al solo worker.

Un piano che usa il livello 2 ha quindi due contabilità distinte, e il tetto
duro riguarda la prima.

### 5.3 `ResourceLimit` solo con evidenza attribuibile

Un processo che muore non dice **perché**. Un segnale o un codice di uscita
anomalo, da soli, sono compatibili con un superamento di memoria **e** con un
crash: dedurne un `ResourceLimit` significherebbe attribuire al budget un
difetto che potrebbe essere nostro.

La regola è quindi:

- **Linux**: `ResourceLimit` solo con l'evidenza del cgroup —
  `memory.events.local`, che attribuisce l'evento al cgroup dell'esecuzione e
  non a un antenato;
- **Windows**: `ResourceLimit` solo con la **notifica del Job Object**, non
  dal codice di uscita;
- **in assenza di evidenza specifica del sistema operativo**: `Internal`. Dire
  «non so perché il worker è morto» è corretto; dire «ha superato il budget»
  senza saperlo non lo è.

Riferimenti: [cgroup v2](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html),
[Job Objects](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information).

### 5.4 Su Linux niente promessa byte-esatta

`memory.max` è un limite del kernel, ma la documentazione cgroup v2 ammette
**superamenti temporanei**: il kernel reagisce recuperando memoria e, se non
riesce, invocando l'OOM killer sul cgroup. Non si può quindi promettere
letteralmente «mai un byte oltre N».

Ciò che si può promettere è **contenimento** — l'esecuzione non può crescere
indefinitamente e non travolge il resto della macchina — e **attribuzione**,
tramite `memory.events.local`. È una promessa più debole di quella che il nome
`hard` suggerisce, e va scritta accanto al nome.

### 5.5 Su Windows conta la memoria *committed*

Il limite del Job Object riguarda la memoria **committed**, che non coincide
con l'RSS né con la memoria virtuale riservata. Un budget espresso pensando
all'RSS si comporterà diversamente su Windows, e la differenza va documentata
invece che scoperta da chi imposta il numero.

### 5.6 macOS: profilo hard non supportato

Non esiste, allo stato, un meccanismo dimostrato equivalente a cgroup o Job
Object. `setrlimit` da solo **non prova** né copertura completa — non è chiaro
che cosa contenga rispetto alle allocazioni native — né attribuzione del
fallimento, che è il requisito di §5.3.

Il profilo isolato è quindi **non supportato su macOS** finché un prototipo
non dimostri entrambe le cose. Non «supportato con riserva»: non supportato,
e `hard_process_memory_bytes` va rifiutato lì invece di essere accettato e
ignorato.

Riferimento: [setrlimit(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setrlimit.2.html).

### 5.7 PyO3: nessun tetto duro in-process, e il profilo isolato costa lo zero-copy

Quando esisterà un binding Python, in-process **non avrà un tetto duro**:
l'interprete e le sue estensioni allocano per conto proprio, fuori da
qualunque contabilità della libreria, e il livello 1 continuerà a governare
solo ciò che governa oggi.

Il profilo isolato è disponibile anche da Python, ma con un prezzo esplicito:
i dati attraversano un confine di processo, quindi **si rinuncia allo
zero-copy** e si paga la serializzazione IPC. È un compromesso, non un
dettaglio implementativo, e chi sceglie il tetto duro lo sceglie sapendolo.

---

## 6. Che cosa questa analisi NON dice

- **non stima il costo della conversione** dei nove siti: dipende da quanto
  stretto sia il maggiorante di ciascuno, e nessuno è stato analizzato
  singolarmente;
- **non ha misurato** l'overhead di avvio del processo isolato né il costo del
  trasporto IPC su piani reali: sono affermazioni di struttura, non misure;
- **non ha prototipato nessun meccanismo del sistema operativo.** Quanto detto
  in §5.3–§5.6 su cgroup v2, Job Object e `setrlimit` viene dalla
  documentazione citata, non da un esperimento condotto qui. È esattamente per
  questo che macOS resta non supportato: la differenza fra «documentato» e
  «dimostrato» è la ragione della clausola, e vale anche per gli altri due —
  il primo blocco di attuazione dovrà dimostrarli, non fidarsi di questo
  documento;
- **non ha verificato PyO3**, che non esiste nel workspace. Quanto detto è ciò
  che vale per qualunque host che allochi per conto proprio, e andrà
  riverificato quando quel confine esisterà davvero;
- **non copre il percorso legacy della CLI** oltre a osservare che
  `ammissione_output` rifiuta la pubblicazione *dopo* la costruzione;
- **non propone un piano di implementazione**. La decisione sul contratto è
  presa (§0); l'attuazione — rinominazione, migrazione di versione dei piani
  storici, profilo isolato — è un blocco separato, e questo ADR non ne stima
  il costo né ne fissa l'ordine.


---

## 7. Attuazione del livello 1 (2026-08-20)

Il livello 1 è attuato. Il livello 2 **no**: `hard_process_memory_bytes` non
esiste ancora nel codice, di proposito — introdurre il nome prima del profilo
isolato che lo realizza ripeterebbe l'errore che questa ADR corregge.

Il titolo di questo documento cita ancora il nome della v4 perché è il nome
con cui il problema è stato posto. Non è un nome ancora in uso.

### Che cosa è cambiato

| | |
|---|---|
| campo Rust | `Limits::max_governed_memory_bytes` (`plenora-core`) e `plenora_kernels_table::Limits::max_governed_memory_bytes` |
| chiave di piano | `limits.max_governed_memory_bytes` |
| versione canonica del piano | **5** |
| v4 | accettata **solo** attraverso `plan::migrazione_v4` |
| `schema_version <= 3` | migrata da `PlanV5::from_legacy` direttamente al canonico v5 |
| `plan_hash` | separatore di dominio, ADR 4 emendata |
| alias | **nessuno**, e un gate lo verifica a ogni CI |

### Come è resa vera l'assenza di alias

Non con un controllo aggiuntivo, che si può dimenticare, ma con **due
strutture separate**:

- `LimitsOverride` (v5) conosce solo `max_governed_memory_bytes`;
- `LimitsOverrideV4`, privata del modulo di migrazione, conosce solo il nome
  della v4.

Entrambe hanno `deny_unknown_fields`. Ne segue, per costruzione: un piano v5
col nome della v4 è rifiutato, un v4 col nome nuovo è rifiutato, e un piano
che dichiara entrambe le chiavi è rifiutato in ogni versione — qualunque
versione dichiari, una delle due chiavi è sconosciuta a chi la deserializza.

La traduzione fra le due non è una riscrittura di chiavi ma una costruzione
**per campi**: se un limite nuovo viene aggiunto a `LimitsOverride`, il
letterale della migrazione smette di compilare. Una migrazione che dimentica
un campo nuovo lo lascerebbe cadere in silenzio, ed è così che un piano
migrato finisce per girare sotto limiti che non ha chiesto.

### Determinismo, idempotenza, ordine

- un piano v4 e il v5 equivalente producono lo **stesso** piano canonico e lo
  **stesso** `plan_hash`;
- la migrazione non riordina né normalizza il resto del piano: normalizzare è
  compito della canonicalizzazione, e farlo in due posti significa poterlo
  fare in due modi;
- l'ingresso di versione è idempotente (applicato al proprio risultato non
  cambia nulla); la sola funzione di migrazione, invece, **rifiuta** un piano
  già migrato, perché chiamarla due volte è un errore di chi chiama;
- l'ordine è fail-closed: tetto sui byte → chiavi duplicate → lettura della
  `schema_version` → migrazione → parse. Le chiavi duplicate sono rifiutate
  prima di leggere la versione: risolverle con «vince l'ultima» farebbe
  dipendere la scelta del percorso da quale duplicato ha vinto.

### Invalidazione esplicita delle identità

Ogni `plan_hash` prodotto prima della v5 è invalidato **per costruzione**, non
per fortuna: il digest è calcolato su `"plenora/plan_hash/v5\0" ||
canonical_json` (ADR 4, emendamento 2026-08-20). Senza il dominio
l'invalidazione sarebbe una proprietà del contenuto — vera oggi, non
garantita domani.

Il `catalog_fingerprint` e i `ContractFingerprint` degli input **non**
cambiano: non descrivono il formato del piano, e invalidarli sarebbe rumore.

### Il gate

`scripts/verifica_nome_budget_memoria.py` (in CI) fallisce se il nome della v4
ricompare nel codice di produzione fuori dal modulo di migrazione, se compare
in un file di test non dichiarato, se il modulo di migrazione smette di
nominarlo (allowlist stantia), o se un `serde(alias` compare nelle strutture
che dichiarano i limiti.

### Rottura non richiesta ma inevitabile, dichiarata qui

Il campo rinominato esiste in **due** strutture: quella di `plenora-core`, che
è il contratto del piano DAG, e quella di `plenora-kernels-table`, che è anche
il blocco `limits` dei piani lineari `schema_version <= 3`. Rinominare la
prima e non la seconda avrebbe lasciato il nome della v4 vivo nel codice di
produzione, cioè avrebbe reso il gate impossibile e la decisione parziale.

Conseguenza: **un piano legacy che dichiara il vecchio nome ora è rifiutato**
(`deny_unknown_fields`, quindi fail-closed con errore, non silenziosamente
ignorato). Per i piani legacy non esiste una migrazione automatica: la loro
versione non distingue il prima dal dopo, e inventarne una avrebbe voluto dire
decidere qui una cosa che non è stata chiesta. È registrato in
`docs/api-breaking-2026-08-16.md`.
