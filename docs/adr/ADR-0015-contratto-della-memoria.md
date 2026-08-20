# ADR 15 — Che cosa promette `max_memory_bytes`: contratto della memoria

- **Stato**: PROPOSTA, in attesa di decisione del maintainer. Nessuna
  strategia è stata implementata e nessuna API è stata rinominata.
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

## 4. Raccomandazione: contratto a due livelli

Nessuna delle quattro strategie basta da sola, e la scelta non è fra loro ma
fra **promettere meno e mantenerlo** oppure promettere un tetto e non averlo.

### Livello 1 — Budget di ammissione governato (nella libreria)

`max_memory_bytes` governa la **memoria che la libreria controlla**: i batch
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
del sistema operativo (strategia 2.4). È l'unica configurazione in cui «questa
esecuzione non supera N byte» è vero per Arrow, per GEOS, per PROJ e per
qualunque host futuro, e in cui il fallimento è osservabile invece che
catastrofico.

### Che cosa comporta per il nome

`max_memory_bytes` oggi promette il livello 2 e realizza il livello 1. Le
opzioni sono due, ed è la decisione da prendere:

- **mantenere il nome** e cambiare il contratto documentato, dichiarando che è
  un budget di ammissione della memoria governata e che il tetto duro è del
  profilo isolato;
- **rinominare** il campo perché il nome dica ciò che fa, accettando una
  rottura di API prima del freeze.

Questo ADR **non sceglie** e **non rinomina nulla**: la decisione è del
maintainer. Segnala però che è il momento giusto per prenderla, perché dopo il
freeze il nome sarà un impegno pubblico.

---

## 5. Che cosa questa analisi NON dice

- **non stima il costo della conversione** dei nove siti: dipende da quanto
  stretto sia il maggiorante di ciascuno, e nessuno è stato analizzato
  singolarmente;
- **non ha misurato** l'overhead di avvio del processo isolato né il costo del
  trasporto IPC su piani reali: sono affermazioni di struttura, non misure;
- **non ha verificato PyO3**, che non esiste nel workspace. Quanto detto è ciò
  che vale per qualunque host che allochi per conto proprio, e andrà
  riverificato quando quel confine esisterà davvero;
- **non copre il percorso legacy della CLI** oltre a osservare che
  `ammissione_output` rifiuta la pubblicazione *dopo* la costruzione;
- **non propone un piano di implementazione**: serve prima la decisione sul
  contratto.
