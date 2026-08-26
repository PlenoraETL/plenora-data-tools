# Esecuzione isolata — progetto tecnico della fase 4

Progetto, non implementazione. Alla data di questo documento **non esiste
codice** di worker, supervisore, protocollo o limiti di processo: qui c'è che
cosa dovranno fare e perché.

La scelta architetturale è presa e non si riapre: la correttezza viene
dall'**isolamento**, non dalla previsione. Il ragionamento è in
[`stato-e-roadmap.md`](stato-e-roadmap.md); i requisiti vincolanti sono `F4-1`
… `F4-22` nello stesso documento — erano sei alla prima stesura, e i
sedici aggiunti vengono quasi tutti da ciò che i prototipi hanno
smentito o da ciò che una rilettura ha trovato contraddittorio.

Baseline strutturale: commit `922aea3`, tag `baseline-pre-fase-4`.

I prototipi sono stati eseguiti in **due cicli**: le misure stanno in
[`prototipi-isolamento.md`](prototipi-isolamento.md), e dove un paragrafo qui
sotto riporta un numero, quel numero viene da lì.

Nascita vincolata e contenimento sono dimostrati su entrambe le piattaforme.
**L'attribuzione no.** Su Linux regge solo se il dominio è reso foglia dal
kernel; su Windows nessuna fonte singola è sufficiente, e la combinazione
lascia un comportamento residuo dichiarato. Il primo ciclo aveva concluso
diversamente, ed era prematuro.

---

## 1. Garanzie osservabili

Ciascuna è una proprietà che un consumatore può verificare dall'esterno, senza
leggere il nostro codice.

| | garanzia |
|---|---|
| **GA-1** | Dopo OOM, crash, panic, timeout, cancellazione o risultato invalido **avvenuti prima del commit point**, nessun output finale è visibile: il percorso di destinazione non esiste, oppure contiene ciò che c'era prima. Il commit point è la **creazione no-clobber della destinazione** (§7-bis). Dopo di esso l'output è visibile e la garanzia non è più «nulla è visibile» ma «ciò che è visibile è completo». |
| **GA-2** | Un esito classificato `ResourceLimit` porta **evidenza attribuibile** del dominio di isolamento. Senza prova specifica la classificazione è `Internal`. |
| **GA-3** | **Nessun output incompleto o strutturalmente invalido viene pubblicato.** Schema, contratto, framing, conteggi e sigillo sono verificati dall'autorità unica `plenora_core::contract` prima che qualcosa diventi visibile. |
| **GA-4** | La pubblicazione è **atomica**: il file finale c'è per intero o non c'è. Nessuno stato intermedio è osservabile. |
| **GA-5** | Il worker **non riceve** la destinazione finale: il protocollo non la trasporta, e non c'è altro canale da cui dedurla. |
| **GA-6** | Supervisore e worker concordano su identità e versione del resolver CRS **prima** che il worker tocchi dati. Un disaccordo ferma l'esecuzione. |
| **GA-7** | Il limite di memoria è in vigore **prima** che il worker possa elaborare dati o compiere allocazioni critiche. |

## 2. Non-garanzie

Dichiararle è parte del contratto: una garanzia che si crede di avere è
peggio di una che si sa di non avere.

| | non-garanzia |
|---|---|
| **NG-1** | Non garantiamo che il worker resti dentro il budget. Garantiamo che se lo sfonda **non pubblichi**. La differenza è tutto il progetto. |
| **NG-2** | Non garantiamo l'attribuzione dell'OOM dove la piattaforma non offre evidenza specifica. Lì l'esito è `Internal`, ed è meno informativo di proposito. |
| **NG-3** | Il limite copre la memoria **del dominio di isolamento**. Memoria mappata da altri processi, riservata da driver o allocata fuori dal dominio non è contenuta. |
| **NG-4** | La cancellazione è cooperativa dentro il worker e forzata solo dopo un margine: non garantiamo un tempo massimo di reazione, solo un tempo massimo di attesa prima della terminazione. |
| **NG-5** | Il cleanup degli artefatti temporanei è garantito nel percorso normale e *best-effort* dopo una terminazione violenta. La bonifica differita esiste già ([`errori-e-limiti.md`](errori-e-limiti.md)). |
| **NG-6** | Su macOS **non c'è contenimento**: il profilo isolato non è supportato finché un prototipo non dimostri contenimento *e* attribuzione. |
| **NG-7** | Non garantiamo che un worker malevolo sia contenuto. Il modello di minaccia è il **guasto**, non l'avversario: il worker è codice nostro che può sbagliare, non codice ostile. |
| **NG-8** | `GA-3` è **validità strutturale, non correttezza dei valori**. Schema, contratto, framing, conteggi e sigillo dimostrano che l'output è completo e ben formato; **non** dimostrano che i numeri dentro siano quelli giusti. Un kernel che calcola male produce un output strutturalmente perfetto, e questa verifica lo pubblica. La correttezza dei valori resta affidata al determinismo dichiarato e alla suite, dove è sempre stata. |
| **NG-9** | `GA-5` è una proprietà del **protocollo**, non del filesystem. Senza un contenimento del filesystem il worker *potrebbe* scrivere ovunque abbia permessi: semplicemente non sa dove sia la destinazione, perché nessuno gliela dice. Un contenimento vero — handle già aperti passati al posto dei percorsi, oppure una sandbox del filesystem — è un rafforzamento futuro, non una garanzia odierna. |
| **NG-10** | **Contenere non significa terminare.** Su Linux il kernel uccide; altre piattaforme possono negare l'allocazione lasciando il processo vivo ([`prototipi-isolamento.md`](prototipi-isolamento.md)). Non garantiamo un modo unico di fermarsi: garantiamo che il tetto sia imposto e che l'evento sia leggibile. |
| **NG-11** | Il tetto del dominio **non protegge dalle richieste**, solo dagli addebiti. Su Linux una richiesta da 64 TiB non toccata è stata accettata senza generare alcun evento. Un worker non può difendersi interrogando l'allocatore. |
| **NG-12** | **L'attribuzione dipende dal sigillo del dominio.** Se il worker riesce a creare discendenti, l'uccisione avviene altrove e l'evidenza locale non la registra. Il sigillo è verificato nel preflight (§9-bis): senza, il profilo isolato non parte. |
| **NG-13** | **L'esaurimento del coordinatore non produce un errore strutturato.** Il coordinatore è il processo del chiamante e non ha un tetto nostro: se muore, muore l'invocazione, e non c'è nessun esito da leggere. Se muore **dopo** il commit point, l'output è pubblicato e il chiamante non lo sa: è un esito **ambiguo**, e si risolve rileggendo la destinazione (§7-bis). |
| **NG-14** | **Il tetto non è un massimo istantaneo invalicabile.** `memory.max` è la semantica del kernel, che ammette superamenti temporanei prima che il reclaim o l'OOM intervengano. Garantiamo che il limite sia **imposto**, non che nessun byte lo oltrepassi mai (§7-ter). |

---

## 2-bis. Semantica del budget: due grandezze, non una

È la decisione centrale, e va presa prima del protocollo perché ne cambia i
messaggi.

### Che cosa misurano oggi le tre grandezze

| grandezza | misura | chi la impone |
|---|---|---|
| `max_governed_memory_bytes` | memoria **ritenuta**: i lease vivi sui risultati | il governor, in-process |
| `memory.max` (cgroup v2) | memoria **del cgroup**: tutto ciò che il dominio tocca | il kernel Linux |
| limite di memoria del Job | memoria **committed** del job | Windows |

Le tre non sono la stessa cosa e nessuna delle tre è convertibile nelle altre.
La memoria ritenuta è un sottoinsieme di quella del dominio: fuori restano
l'allocatore, lo stack, il codice mappato, i buffer Arrow non ancora sotto
lease e tutto ciò che una dipendenza alloca senza dircelo.

### La decisione

**Nasce un limite separato.** Le altre due strade sono respinte:

| strada | perché no |
|---|---|
| il budget governato diventa la memoria totale del dominio | ridefinirebbe in silenzio un campo **già presente nei piani pubblicati**: lo stesso `max_governed_memory_bytes: 512Mi` significherebbe una cosa diversa prima e dopo. È una rottura semantica travestita da chiarimento |
| solo un overhead dichiarato, senza limite nuovo | l'overhead non è una costante: dipende dall'allocatore, dalla piattaforma e dal piano. Dichiararlo come numero fisso sarebbe una stima, ed è esattamente ciò che questo progetto ha deciso di non usare come fondamento |

Quindi:

| | ruolo |
|---|---|
| `max_domain_memory_bytes` (**nuovo**) | il tetto imposto dal sistema operativo al dominio. È **la** garanzia |
| `max_governed_memory_bytes` (esistente, invariato) | ammissione, ritenzione e diagnostica **dentro** il worker. Non è un tetto duro e non lo diventa |

Il rapporto fra i due non è calcolato: è **dichiarato**. Un piano che
dichiara un budget governato maggiore del limite di dominio è respinto in
validazione — non perché sappiamo quanto overhead servirà, ma perché quella
combinazione è incoerente per costruzione.

### Il confine è pubblico

`max_domain_memory_bytes` è osservabile dall'esterno: cambia che cosa un piano
può chiedere e che cosa il componente promette. Appartiene quindi ai **confini
pubblici osservabili** di `plenora-contracts`, non al progetto interno.

Ciò che **non** vi appartiene, e che questo documento tiene per sé: worker,
supervisore, protocollo, moduli, governor, cgroup e Job Object. Sono
meccanismi, e un contratto che li prescrivesse legherebbe le mani a
un'implementazione invece di descrivere una promessa.

---

## 2-ter. La verifica non può stare fuori dal limite

Il supervisore rilegge l'artefatto e ricostruisce il contratto. **Anche quella
fase alloca**, e su un artefatto grande può esaurire la memoria: senza una
difesa, l'OOM non è eliminato, è spostato dal worker al supervisore — dove per
giunta non c'è nessuno a osservarlo.

Due difese, entrambe necessarie:

**1. Verificatore in streaming con tetti dimostrabili.** La verifica non
materializza mai più di un tetto dichiarato: legge lo schema dall'header,
scorre i batch senza accumularli, confronta i conteggi incrementalmente. Il
confine ostile per Arrow IPC che il progetto ha già
([`errori-e-limiti.md`](errori-e-limiti.md)) impone tetti sul framing prima
che arrow veda i byte: la verifica ne eredita la disciplina e vi aggiunge il
proprio tetto sui batch trattenuti.

Il tetto dev'essere **dimostrabile**, non asserito: la verifica alloca una
quantità funzione del solo schema e del batch corrente, mai del numero di
righe o di batch. Una verifica che debba tenere tutto in memoria per
rispondere non è accettabile in questo disegno.

**2. La verifica ha a sua volta un dominio.** Se la verifica in streaming
fallisse comunque, non deve portarsi via il processo che classifica. Il suo
tetto è una seconda linea, non la prima.

Il che divide in due ciò che finora questo documento ha chiamato
«supervisore»: chi **rilegge** e chi **osserva**. Il primo alloca e va
contenuto; il secondo non deve allocare affatto, altrimenti l'osservatore ha
bisogno di un osservatore. La §2-quater lo scioglie.

La sequenza di verifica in §7 va letta con questo vincolo: **ogni passo
dichiara quanta memoria può trattenere**, e nessuno risponde «tutta quella che
serve».

---

## 2-quater. Topologia: chi osserva chi

La §2-ter mette il supervisore sotto un tetto e non dice chi osserva il **suo**
esaurimento. È circolare: se il processo che classifica muore per la stessa
ragione che deve classificare, non resta nessuno a dirlo.

La topologia è quindi a tre, non a due:

```
    coordinatore minimo
    ├── dominio del worker        (elabora, scrive, sigilla)
    └── dominio del verificatore  (rilegge in streaming)
```

Il **coordinatore** non materializza mai dati: non legge l'artefatto, non
costruisce batch, non tocca Arrow. Tiene lo stato della macchina, i timeout, i
descrittori, i contatori e il fermo dell'evidenza.

«Non alloca affatto» sarebbe però falso, e la prima stesura lo scriveva. Un
processo alloca: stack, allocatore, buffer dei messaggi del protocollo. La
proprietà vera è più debole e più utile:

> la memoria del coordinatore è **limitata per costruzione e indipendente dai
> dati**: funzione del numero di domini vivi e della dimensione massima di un
> messaggio del protocollo, entrambe costanti dichiarate.

«Per costruzione» va però reso vero voce per voce, perché ogni canale che
entra nel coordinatore è una cattura potenzialmente illimitata, e sono tutti
alimentati dal worker:

| voce | tetto |
|---|---|
| coda degli eventi | numero massimo di elementi dichiarato. Alla saturazione **non si cresce**: si registra la perdita e si classifica il dominio come non osservabile |
| `stdout` e `stderr` del worker | tetto in byte, con troncamento dichiarato. Non trasportano il protocollo (§4.2), quindi troncarli non perde un esito — ma catturarli senza limite darebbe al worker il modo di esaurire il coordinatore scrivendo |
| messaggi `Progresso` | tetto sul numero e sulla frequenza. Un worker che ne manda troppi viene trattato come un worker che non ne manda: il timeout è la difesa in entrambi i casi |
| fermo dell'evidenza | dimensione fissa: contatori, non una cronologia |

Nessuna di queste è una cattura illimitata, e nessuna cresce con il numero di
righe o di batch.

### Chi osserva il coordinatore: nessuno, e va detto

**Il coordinatore è il processo del chiamante**, e non gli si applica un tetto
imposto da noi. Metterglielo riporterebbe la ricorsione — servirebbe qualcuno
che ne osservi la terminazione, e per quel qualcuno di nuovo lo stesso.

La prima stesura se la cavava scrivendo che «il chiamante osserva già la
propria morte». È falso: **un processo non osserva la propria morte**. Può
osservarla solo suo padre, o l'ambiente che lo ha avviato. Se il coordinatore
esaurisce la memoria, non c'è nessun esito da leggere e nessun errore
strutturato da produrre: l'invocazione semplicemente termina.

Va dichiarato invece che nascosto (`NG-13`):

| | |
|---|---|
| `GA-1` — nulla di parziale è visibile | **regge**, ma non grazie a una diagnosi: grazie al fatto che la pubblicazione è atomica e che, morendo prima del commit point, non c'è nulla da vedere |
| `GA-2` — esito attribuibile | **non regge**: non c'è processo che sopravviva per classificare |
| chi se ne accorge | il padre del chiamante, o l'ambiente. Non noi |

Ciò che dobbiamo al chiamante non è un tetto, ma una **quantità dichiarata** —
quanto questo componente aggiunge al suo consumo — e la ragione per cui non
cresce coi dati: nessun buffer di dati, stato per dominio vivo, messaggi con
un tetto costante.

Una prova di scala — dieci volte i dati, memoria del coordinatore invariata —
è una **regressione utile**, non una dimostrazione: mostra che non è cresciuta
in quel caso, non che non possa crescere. La dimostrazione, se la si vuole,
sta nel codice: nessuna struttura la cui dimensione dipenda dal numero di
righe o di batch.

### Il picco complessivo è una guida, non una garanzia

### Due domini distinti, non uno riusato

Sono **due**, e la ragione è l'attribuzione, non l'ordine.

L'evidenza di limite è un **delta di contatori sul dominio**
([`prototipi-isolamento.md`](prototipi-isolamento.md)). Riusare lo stesso
dominio per il worker e per il verificatore significherebbe leggere contatori
già mossi da un'altra fase: un evento registrato durante l'elaborazione
resterebbe visibile durante la verifica, e attribuirlo diventerebbe una
questione di sottrazioni. Due domini distinti rendono ogni evento
inequivocabile per costruzione.

### Il tetto vale per **ciascun** dominio

E non è una scelta arbitraria, perché i due domini non coesistono: il dominio
del worker è distrutto prima che quello del verificatore sia creato. Con
esecuzione sequenziale,

```
    picco(worker) e picco(verificatore) non si sommano mai
    picco del sistema = coordinatore + max(picco(worker), picco(verificatore))
```

il tetto per ciascuno **è** il tetto del totale, e un limite «sul totale»
sarebbe la somma di due grandezze che non esistono insieme. Il chiamante
ottiene così la promessa che può usare: *nessuna fase supera*
`max_domain_memory_bytes`.

**L'uguaglianza vale però per fase, non per l'istante.** Il kernel conosce
cgroup *morenti*: un cgroup rimosso può trattenere temporaneamente risorse
già addebitate finché non sono liberate. Fra la distruzione del dominio del
worker e la creazione di quello del verificatore può quindi esistere una
finestra in cui il sistema trattiene più di `max_domain_memory_bytes`. Non è
stato misurato.

La garanzia si limita perciò a ciò che la primitiva dà: **su ogni dominio il
tetto è imposto**, nel senso della §7-ter — non che nessun byte lo oltrepassi
mai. Il picco osservato è una misura con le sue ripetizioni, e il picco
complessivo del sistema è una guida al dimensionamento, non una promessa.

Il coordinatore **non ha un tetto imposto da noi** — il perché è in
§2-quater — e il suo consumo è l'unica quantità che si somma a quella dei
domini. Va **dichiarato**, non limitato.

**Invariante.** Il dominio del worker viene distrutto — non solo svuotato —
prima della creazione di quello del verificatore. Un'esecuzione che li tenesse
vivi insieme renderebbe falsa l'uguaglianza qui sopra, e con essa la promessa.

---

## 2-quinquies. Il piano chiede, la politica dell'host concede

Un piano che scegliesse da solo `max_domain_memory_bytes` potrebbe chiedere una
cifra enorme. Il tetto del sistema operativo esisterebbe formalmente e non
proteggerebbe niente: l'host resterebbe esposto a un input.

Il limite in vigore non è quindi quello chiesto:

```
    limite_effettivo = min(richiesta_del_piano, politica_dell_host)
```

La politica dell'host **non è ampliabile dall'input**: non sta nel piano, non
viaggia nel protocollo, e non è raggiungibile da nulla che il chiamante
fornisca. Appartiene al **dispiegamento** — chi installa il componente, non chi
gli manda un piano.

Un piano che chiede di più non è un errore: ottiene meno. Ma non in silenzio.
Il taglio va **riportato in modo machine-readable** nell'esito, con il valore
chiesto e quello concesso: un tetto ridotto senza dirlo produrrebbe una
terminazione che il chiamante non ha modo di spiegarsi, e leggerebbe come un
difetto nostro. Un piano che chiede **meno** ottiene ciò che ha chiesto:
abbassare il proprio tetto è sempre lecito.

Se la politica dell'host **non è configurata**, il profilo isolato non è
disponibile. Non si ripiega su «allora vale ciò che chiede il piano»: sarebbe
il caso in cui l'host è più esposto, ottenuto per omissione.

C'è anche un pavimento. Sotto una certa soglia il dominio non è vitale, e i
prototipi mostrano che i due sistemi lo comunicano male — `SIGKILL` prima di
qualunque riga su Linux, `STATUS_STACK_OVERFLOW` su Windows. Un tetto sotto il
pavimento va respinto **in validazione**, con un errore che dice qual è il
pavimento: un errore di configurazione non merita di manifestarsi come un
crash.

### Le decisioni che precedono la `PR-2`

Nessuna riga di `max_domain_memory_bytes` va scritta prima che queste siano
prese, perché ognuna cambia che cosa il campo significa.

| domanda | decisione |
|---|---|
| piani v5 che non hanno il campo | restano **validi e invariati**, e il campo in v5 non esiste affatto: un v5 che lo dichiarasse va rifiutato (`PLAN-007`). In v6 è facoltativo, e la sua assenza significa una cosa sola: il profilo isolato non è selezionabile per quel piano, mai un tetto implicito. Nessuna migrazione forzata |
| obbligatorietà | il campo è obbligatorio **solo** nel profilo isolato. Chiedere l'isolamento senza dichiarare il tetto è incoerente, e va respinto in validazione |
| forma canonica e `plan_hash` | ogni piano **v5 esistente mantiene il proprio `plan_hash`**: la v5 non cambia, e nessun identificativo già pubblicato si muove. Un piano **v6 appartiene al dominio nuovo**, quindi un v5 e un v6 per il resto identici hanno identità diverse. Dentro la v6, il campo entra nella forma canonica **solo quando è presente**: un v6 che lo dichiara e uno che lo omette sono piani diversi. La stesura precedente diceva «un piano che non lo dichiara ha lo stesso `plan_hash`», che è vero dentro la v5 e **falso attraverso questo confine di versione**. Nessuna migrazione v5→v6 può fingere equivalenza di hash (`PLAN-021`). Attenzione a non generalizzare: **v4→v5 conserva l'equivalenza**, perché un v4 è migrato NEL canonico v5, e la ratifica lo dichiara esplicitamente (`PLAN-019`) |
| versionamento | **non è una decisione additiva**, e la prima stesura sbagliava a chiamarla tale: vedi sotto |
| ratifica in `plenora-contracts` | **adottata**: `Plan Budget 1.0` (`plenora-plan-budget-v1`) con la decisione `0005`, `PLAN-001…021`, mergiata in `5d15107`. È separata dall'adozione generale della nuova linea normativa, che resta un blocker a sé ([`stato-e-roadmap.md`](stato-e-roadmap.md)) |

#### `deny_unknown_fields` rende il campo una rottura, non un'aggiunta

Il piano usa `deny_unknown_fields` **a ogni livello**. Un lettore v5 di oggi,
messo davanti a un piano che dichiara `max_domain_memory_bytes`, non lo
ignora: lo **rifiuta**. L'aggiunta è quindi compatibile all'indietro — i piani
vecchi restano validi — ma **non in avanti**, e la seconda è quella che rompe
in produzione, dove i lettori vecchi esistono già.

Il precedente è in casa e non lascia molto spazio.
[`migrazione_v4.rs`](../crates/plenora-engine/src/plan/migrazione_v4.rs)
documenta che il passaggio da `max_memory_bytes` a
`max_governed_memory_bytes` ha richiesto **una struttura separata e una
migrazione esplicita**, proprio perché `deny_unknown_fields` fa di ogni nome
nuovo un rifiuto. Quel cambiamento aveva incrementato la versione dello
schema.

La decisione **non è nostra**: appartiene a `plenora-contracts`, e va presa
prima della `PR-2`. Le due opzioni, con ciò che comportano:

| opzione | conseguenza |
|---|---|
| **resta v5**, con incompatibilità in avanti dichiarata | nessuna migrazione, ma un lettore v5 vecchio rifiuta un piano v5 nuovo: due cose diverse portano lo stesso numero, ed è precisamente ciò che una versione dovrebbe impedire |
| **diventa v6** | costa una migrazione e un ciclo di adozione, ma il numero torna a significare qualcosa e il rifiuto di un lettore vecchio diventa leggibile invece che misterioso |

Il progetto propendeva per **v6**, per coerenza col precedente v4→v5 e perché
`deny_unknown_fields` è una scelta deliberata di questo formato: convivere con
una sua conseguenza fingendo che sia additiva la contraddirebbe.

**Ratificato**: `plenora-contracts` ha adottato `Plan Budget 1.0`
(`plenora-plan-budget-v1`) con la decisione `0005`, revisione `1bb2c3d`
mergiata in `5d15107`. Il campo — che sul filo vive accanto a
`schema_version`, non a `plan_format_version`, che è il nome interno del grafo
validato — esiste solo in v6, dentro `limits`, è facoltativo, deve essere
`>= max_governed_memory_bytes` **effettivo** — default compreso, quando il
piano lo omette — e dichiara il tetto **richiesto**, non quello concesso: il
tetto effettivo è `min(richiesta, policy dell'host)`, e policy e meccanismo
restano fuori dal formato. Il componente deve **pubblicare** il default che
applica, altrimenti chi invia non può verificare quel vincolo prima di
inviare.

---

## 3. Macchina a stati

### 3.1 Supervisore

```
                    ┌──────────┐
                    │  Inerte  │
                    └────┬─────┘
                         │ esegui(piano, input, destinazione)
                    ┌────▼──────────────┐
                    │ PreparaIsolamento │──errore─┐
                    └────┬──────────────┘         │
                         │ dominio creato,        │
                         │ limite applicato       │
                    ┌────▼──────┐                 │
                    │  Avvia    │──errore─────────┤
                    └────┬──────┘                 │
                         │ processo nel dominio   │
                    ┌────▼──────────┐             │
                    │  Handshake    │──disaccordo─┤
                    └────┬──────────┘             │
                         │ accordo                │
                    ┌────▼──────────┐             │
                    │  InEsecuzione │◄──┐         │
                    └────┬──────────┘   │ eventi  │
                         │              └─────────┤
                         │ esito ricevuto         │
                    ┌────▼──────────┐             │
                    │  Classifica   │             │
                    └────┬──────────┘             │
                         │ successo dichiarato    │
                    ┌────▼──────────┐             │
                    │  Verifica     │──fallita────┤
                    └────┬──────────┘             │
                         │ verificato             │
                    ┌────▼──────────┐             │
                    │  Pubblica     │──fallito────┤
                    └────┬──────────┘             │
                         │                        │
                    ┌────▼──────────────────────┐ │
                    │  Cleanup (sempre)         │◄┘
                    └────┬──────────────────────┘
                    ┌────▼──────┐
                    │ Terminato │
                    └───────────┘
```

**Invarianti di stato.**

- `PreparaIsolamento` precede `Avvia`: il limite è in vigore prima che esista
  un processo da limitare (`F4-1`, `GA-7`). Non si applica un tetto a un
  processo già partito, perché fra la partenza e l'applicazione c'è una
  finestra in cui il tetto non c'è.
- `Handshake` precede qualunque consegna di dati. Un disaccordo su protocollo o
  resolver termina qui, prima che il worker abbia letto un byte (`F4-6`).
- `Classifica` è distinto da `Verifica`: il primo decide *come è finito il
  worker*, il secondo decide *se ciò che ha prodotto è valido*. Un worker che
  esce con successo può aver prodotto un output invalido, e un worker terminato
  non ha nulla da verificare.
- `Cleanup` è attraversato da **ogni** cammino, compresi quelli di errore.
- `Pubblica` è raggiungibile **solo** da `Verifica` riuscita. Non esiste un
  arco che la raggiunga altrimenti.

### 3.2 Worker

```
    ┌──────────┐
    │  Avviato │   (già dentro il dominio, già sotto il limite)
    └────┬─────┘
    ┌────▼──────────┐
    │  Handshake    │──rifiuto──┐
    └────┬──────────┘           │
         │ accordo              │
    ┌────▼──────────┐           │
    │  Elabora      │──errore───┤
    └────┬──────────┘           │
         │                      │
    ┌────▼──────────┐           │
    │  Scrive       │──errore───┤
    └────┬──────────┘           │
         │ artefatto completo   │
    ┌────▼──────────┐           │
    │  Sigilla      │──errore───┤
    └────┬──────────┘           │
         │                 ┌────▼──────────┐
         │                 │  RiportaEsito │
         │                 └────┬──────────┘
    ┌────▼───────────────────────▼─┐
    │  Esce                        │
    └──────────────────────────────┘
```

**Invarianti.**

- Il worker nasce **già vincolato**: non c'è uno stato «prima del limite».
- `Sigilla` è separato da `Scrive`: l'artefatto diventa dichiarato-completo
  solo dopo che i byte sono sul disco e sincronizzati. Un artefatto senza
  sigillo è troncato per definizione, e il supervisore lo tratta come tale
  senza doverne misurare la lunghezza.
- Il worker **non ha** uno stato `Pubblica`. Non conosce la destinazione
  (`GA-5`, `F4-3`).

---

## 4. Protocollo interno

### 4.1 Proprietà

**Versionato.** Il primo messaggio dichiara la versione. Una versione che il
ricevente non conosce ferma l'esecuzione: non si degrada, non si indovina.

**Limitato.** Ogni messaggio ha un tetto di byte; la sequenza ha un tetto di
messaggi. I due tetti sono costanti del protocollo, non configurazione: un
protocollo interno con limiti negoziabili è un protocollo con una superficie
d'attacco negoziabile.

**Fail-closed.** Qualunque cosa non riconosciuta — versione, tipo di
messaggio, campo, lunghezza — è un errore. Non esiste «ignora ciò che non
capisci»: è la regola che trasforma un'estensione futura in un guasto
silenzioso presente.

### 4.2 Canale

Due pipe anonime dedicate, create dal supervisore prima dell'avvio.

`stdout` e `stderr` del worker **non** trasportano il protocollo: restano
disponibili per ciò che il sistema operativo o una dipendenza vi scrivono
senza chiedere permesso. Mescolarli col protocollo significherebbe che una
libreria loquace corrompe un messaggio.

### 4.3 Messaggi

Direzione supervisore → worker:

| messaggio | quando | contenuto |
|---|---|---|
| `Saluto` | primo | versione del protocollo, identità dell'artefatto, identità del resolver, limiti del protocollo, **`commit_token`** |
| `Incarico` | dopo l'accordo | piano validato, descrizione degli input, percorso dell'artefatto temporaneo |
| `Annulla` | in qualunque momento | motivo |

Direzione worker → supervisore:

| messaggio | quando | contenuto |
|---|---|---|
| `Risposta` | primo | versione, identità dell'artefatto, identità del resolver, capability del backend |
| `Progresso` | facoltativo, ripetibile | contatori deterministici (righe, batch); **mai** dati |
| `Esito` | ultimo | successo con sigillo, oppure errore tipizzato con i quattro assi |

Il `commit_token` è **trasmesso e accettato** nel `Saluto`, e sta **solo lì**.

La motivazione della stesura precedente era sbagliata: diceva che l'`Incarico`
non andrebbe bene perché arriva dopo l'accordo, come se fosse troppo tardi.
Non lo è — anche l'`Incarico` precede la scrittura del primo byte, quindi
tecnicamente andrebbero bene entrambi. La scelta è **architetturale**: legare
il token all'handshake gli dà **una sola autorità**, quella su cui i due lati
si sono già accordati prima di toccare i dati, invece di due copie che possono
divergere.

E va chiamato con il nome giusto: **trasmesso e accettato**, non
«negoziato». Negoziare implica che entrambi i lati propongano e convergano,
mentre qui il coordinatore lo impone e il worker lo usa. Diventerebbe
negoziazione solo se la `Risposta` ne confermasse esplicitamente
l'accettazione — cosa che oggi non fa, e che non serve: un worker che
scrivesse un token diverso verrebbe fermato dal passo 8-bis, che è una
verifica sull'artefatto e non sulla buona volontà.

Il `Progresso` è facoltativo e il supervisore non ne dipende: un worker che
non ne manda nessuno è indistinguibile da uno lento, e il timeout è la sola
difesa contro entrambi.

---

## 5. Handshake

Un accordo su quattro cose, tutte prima dei dati.

### 5.1 Protocollo

Versione esatta. Nessuna compatibilità all'indietro fra versioni: il worker è
lo stesso artefatto del supervisore, quindi una differenza di versione è già
un sintomo — significa che stanno girando due binari diversi.

### 5.2 Identità dell'artefatto

Se supervisore e worker sono **modalità dello stesso eseguibile**, l'identità
è il **digest dell'artefatto** più la versione del componente. Il digest è la
prova: due binari con la stessa versione dichiarata ma contenuto diverso
falliscono qui, che è esattamente quello che si vuole quando qualcuno ha
sostituito un file.

Se in futuro il worker fosse un artefatto distinto, l'identità dichiarata
resta la stessa forma e il digest si riferisce a quell'artefatto.

### 5.3 Resolver CRS — `F4-4`

È il requisito che merita più spazio, perché il modo in cui può fallire è
silenzioso.

Esistono due implementazioni con la stessa firma:

| implementazione | comportamento |
|---|---|
| `plenora_core::crs::resolve_crs` | rifiuta con `CRS_BACKEND_UNAVAILABLE` ciò che richiede il backend |
| `plenora_kernels_geo::crs::resolve_crs` | risolve usando PROJ |

Se supervisore e worker ne usassero due diverse, il supervisore potrebbe
**rifiutare come invalido uno schema che il worker ha prodotto correttamente**,
oppure accettarne uno che il worker non avrebbe potuto scrivere. In entrambi i
casi il guasto non assomiglia a un guasto: assomiglia a un dato sbagliato.

L'handshake negozia quindi:

| campo | perché |
|---|---|
| identità del resolver | quale delle due implementazioni |
| versione del resolver | la stessa implementazione può cambiare fra versioni del componente |
| capability del backend | quali trasformazioni il backend dichiara di saper fare |
| **dati ambientali** | ciò che può cambiare il risultato senza cambiare il codice |

I dati ambientali sono la parte che si dimentica. PROJ consulta griglie e
database esterni: **la stessa versione della libreria con griglie diverse dà
risultati diversi**.

#### Perché «digest delle risorse caricate» non funziona

La prima stesura di questo documento chiedeva un digest delle risorse
*effettivamente caricate*. Non è realizzabile prima dei dati, ed è un errore
istruttivo: PROJ sceglie quali griglie aprire **in funzione della
trasformazione e dell'area elaborata**. Prima di vedere i dati non si sa quali
risorse serviranno, quindi non se ne può calcolare il digest — e l'handshake
deve concludersi *prima* dei dati.

#### Che cosa si negozia davvero

Si sposta la domanda da «che cosa è stato caricato» a «che cosa **può** essere
caricato»:

| | che cosa | quando |
|---|---|---|
| **insieme immutabile e content-addressed** | l'elenco completo delle risorse *disponibili* — database e griglie — con il digest dell'insieme, non dei singoli file usati | handshake, prima dei dati |
| **acquisizione dinamica disabilitata** | nessun download, nessuna rete, nessuna cache che si popola da sola durante l'esecuzione | handshake, verificato da entrambi i lati |
| **librerie e database realmente risolti** | nome, versione riportata a runtime, percorso risolto — di ciò che il caricatore ha effettivamente aperto | handshake |

L'insieme è **immutabile** per costruzione: una directory di risorse fissata e
sola-lettura, il cui contenuto è identificato dal digest dell'insieme. Se il
contenuto cambia, cambia il digest, e i due lati non si accordano più.

L'acquisizione dinamica va **disabilitata**, non solo dichiarata. Un backend
che può scaricare una griglia a metà esecuzione rende l'insieme negoziato una
fotografia scaduta: due worker con lo stesso digest iniziale potrebbero finire
con risorse diverse. Disabilitarla è ciò che rende il digest una prova invece
di un'istantanea.

**Backend dinamici.** Se il backend è collegato dinamicamente, l'identità
dell'artefatto non lo copre: il digest dell'eseguibile non dice nulla della
libreria che il caricatore risolverà. I backend dinamici vanno quindi
identificati **separatamente** — nome, versione riportata a runtime, percorso
risolto — e la loro identità entra nell'accordo come le altre.

Un disaccordo su qualunque di questi campi ferma l'esecuzione **prima dei
dati**, con esito `resolver incompatibile`.

### 5.4 Artefatto temporaneo

Il supervisore comunica **un** percorso, dentro una directory che ha creato
lui. Il worker non ne sceglie né il nome né la posizione.

---

## 6. Artefatti temporanei: chi possiede che cosa

| | supervisore | worker |
|---|---|---|
| crea la directory | ✅ | ❌ |
| sceglie il percorso dell'artefatto | ✅ | ❌ |
| scrive l'artefatto | ❌ | ✅ |
| legge l'artefatto per verificarlo | ✅ | ❌ |
| rimuove l'artefatto | ✅ | ❌ |
| conosce la destinazione finale | ✅ | ❌ |
| scrive la destinazione finale | ✅ | ❌ |

La riga che conta è l'ultima. Il worker non può pubblicare **perché non sa
dove**, non perché gli è stato chiesto di non farlo. Una regola che dipende
dall'obbedienza del worker non sarebbe una garanzia.

Il worker scrive **un solo** artefatto. Se in futuro servissero più file, il
supervisore ne comunicherà l'elenco esatto e la verifica coprirà l'insieme:
un worker che scrive qualcosa che il supervisore non si aspetta è un guasto.

---

## 7. Verifica e pubblicazione

L'ordine è vincolante. Ogni passo può solo fermare la sequenza.

| # | passo | fallisce se |
|---|---|---|
| 1 | **stato terminale** | il worker non è uscito, o è uscito in modo non classificabile come successo |
| 2 | **esito dichiarato** | l'`Esito` dichiara un errore, oppure manca |
| 3 | **presenza** | l'artefatto non esiste |
| 4 | **sigillo** | il sigillo manca o non corrisponde: l'artefatto è troncato |
| 5 | **framing** | il file non è un contenitore Arrow IPC leggibile dal confine ostile esistente |
| 6 | **schema** | `contract_from_arrow_schema` fallisce, **con il resolver negoziato** |
| 7 | **contratto** | il contratto letto non corrisponde a quello atteso dal piano validato |
| 8 | **completezza** | i conteggi dichiarati nell'`Esito` non corrispondono a quelli osservati |
| 8-bis | **identità del tentativo** | il `commit_token` nei metadati non è quello trasmesso nel `Saluto`: l'artefatto non è di questo tentativo (§7-quater) |
| 9 | **publish atomico no-clobber** | `persist_noclobber` fallisce, o la destinazione esiste già → `Conflict`. **Mai** una sostituzione |

Solo dopo il passo 9 l'output è visibile. I passi da 1 a 8-bis non producono
alcun effetto osservabile all'esterno.

Il passo 6 è il punto in cui `F4-4` diventa concreto: la lettura usa **lo
stesso resolver** che il worker ha usato per scrivere, perché l'handshake l'ha
imposto. Senza quell'accordo, questo passo confronterebbe due
interpretazioni invece di due risultati.

Il passo 9 riusa la pubblicazione atomica esistente
([`errori-e-limiti.md`](errori-e-limiti.md)), inclusa la distinzione fra
pubblicato e pubblicato-con-durabilità-non-confermata.

---

## 7-bis. Il commit point, e l'esito che nessuno riceve

`GA-4` dice che la pubblicazione è atomica. È vero, e non basta: la
pubblicazione rende atomico **il file**, non la coppia *file pubblicato +
esito restituito*. Fra i due c'è una finestra che nessuna primitiva del
filesystem chiude:

```
    pubblicazione riuscita
      -> il nuovo output e' visibile
         -> il coordinatore muore
            -> il chiamante non riceve alcun esito
```

Il chiamante si trova con un'operazione che *potrebbe* essere riuscita e
nessun modo di saperlo dal nostro valore di ritorno. La prima stesura
sosteneva che l'atomicità della pubblicazione coprisse anche questo. Non lo
copre.

### La decisione: il commit point è dichiarato, e con esso l'ambiguità

Le tre strade possibili, e perché questa:

| strada | perché no, o perché sì |
|---|---|
| un protocollo durevole esterno — journal, lock file, record di transazione | risolverebbe davvero, ma introduce uno stato persistente che questo componente oggi non ha, con la sua bonifica, il suo formato e la sua compatibilità. È un progetto, non una riga |
| restringere `GA-1` ai soli guasti precedenti il commit | è **ciò che facciamo**, ma da solo lascerebbe il chiamante senza rimedio |
| **dichiarare il commit point e rendere l'esito ambiguo risolvibile** | è la scelta: non elimina la finestra, la rende una condizione con un nome e una procedura |

Il **commit point è la creazione no-clobber della destinazione** (§7-bis,
più avanti: non è sempre un rename). Prima: nessun effetto osservabile, e
`GA-1` vale integralmente. Dopo: l'output è visibile, e ciò che può mancare è
solo la *notizia*.

### Perché sigillo e contratto non bastano

La prima stesura diceva che il chiamante risolve l'ambiguità rileggendo la
destinazione, perché l'artefatto porta sigillo e contratto. Non regge, e il
documento lo ammetteva due paragrafi dopo senza accorgersi della
contraddizione: sigillo e contratto dimostrano che l'artefatto è **valido**,
non che sia **quello di questa esecuzione**.

Due esecuzioni dello stesso piano su input diversi producono lo stesso
contratto. Un artefatto trovato sulla destinazione, integro e coerente col
piano, può quindi essere di chiunque abbia eseguito quel piano — inclusa
un'esecuzione precedente andata a buon fine, o una concorrente. Rileggerlo
dice «qui c'è un output valido», che non è la domanda.

### La catena che rende l'ambiguità risolvibile

**L'identificativo lo sceglie il chiamante, non noi.** La prima stesura lo
faceva scegliere al coordinatore e restituire alla fine, il che non risolve
nulla nel solo caso che deve risolvere: se il coordinatore muore dopo il
commit non restituisce niente, e il chiamante non ha il token da
confrontare. Peggio — il coordinatore **è** il processo del chiamante
(§2-quater), quindi non può nemmeno restituire una condizione che descriva la
propria morte.

Un'API in due passi — `start()` che rende un handle, `attendi()` che rende
l'esito — sposterebbe il problema senza risolverlo: l'handle vive nello stesso
processo che muore. L'unica forma che sopravvive è un identificativo che il
chiamante **possiede già prima**, e che può aver scritto dove vuole.

```
    commit_token fornito dal chiamante, prima dell'invocazione
      -> trasportato nel Saluto dell'handshake
         -> scritto dal worker nei metadati dell'artefatto
            -> verificato dal supervisore PRIMA della pubblicazione
```

### Due identificativi, non uno

Un identificativo solo non basta, e la stesura precedente ne usava uno per due
lavori incompatibili.

| | `execution_id` | `commit_token` |
|---|---|---|
| a che serve | correlare log, tracce, diagnostica | rispondere a *questo output è mio?* |
| esiste già? | **sì**, `exec-<uuid>` generato a ogni `execute` | no, è nuovo |
| chi lo sceglie | **l'engine**, e resta così | **il chiamante** |
| può ripetersi? | no, ma il chiamante non lo conosce prima | **non deve**, ed è una sua precondizione |

L'`execution_id` **non cambia**. Una stesura precedente lo rendeva fornito dal
chiamante, che era un cambiamento gratuito: è generato dall'engine, un test ne
verifica la diversità a ogni esecuzione, e serve a correlare diagnostica —
lavoro che fa già bene. Il chiamante non può usarlo per la risoluzione proprio
perché non lo conosce prima, ed è per questo che il token è un'altra cosa.

Con un identificativo solo, riusabile, la risoluzione dà un falso positivo:
l'esecuzione A pubblica con `X`; il chiamante riusa `X` per B; B fallisce e non
pubblica; la risoluzione trova l'artefatto di A e dichiara B riuscita.

**L'unicità è una precondizione esterna, e non possiamo verificarla.** Non c'è
modo di aggirarlo: il token deve essere noto al chiamante *prima*
dell'esecuzione — altrimenti muore col coordinatore — quindi lo sceglie lui,
quindi l'unicità è sua. La garanzia va perciò dichiarata **condizionata**:

> Se il `commit_token` non si ripete fra tentativi, la risoluzione distingue
> l'output di questo tentativo da qualunque altro. Se si ripete, non lo
> distingue, e noi non ce ne accorgiamo.

Non è una garanzia più debole di quanto sembri: è la stessa forma di una
chiave d'idempotenza, e chi ne ha già usata una sa che cosa promette.

### `CommitToken` è un tipo chiuso, non una stringa

Arriva dall'esterno, finisce in un file pubblicato e viene confrontato: le tre
cose che rendono una stringa libera una cattiva idea.

La stesura precedente diceva «alfabeto dichiarato» e «massimo dichiarato»
senza dichiarare né l'uno né l'altro, il che non è una specifica ma un
promemoria. Eccoli.

| | regola |
|---|---|
| valore | un **valore opaco di 32 byte**: il tipo non lo interpreta e non gli attribuisce struttura |
| forma wire | **esattamente 64 caratteri esadecimali minuscoli**, `[0-9a-f]{64}` — nessun prefisso, nessun separatore, nessuna forma alternativa. Due token uguali si scrivono in un modo solo, e il confronto è un confronto di stringhe |
| lunghezza | fissa, quindi non c'è un massimo da far rispettare: una lunghezza diversa **è** una forma invalida |
| chiave nel footer | `plenora.commit.token`, nel namespace `plenora.` che il progetto già usa per i propri metadati |
| validazione | **prima dello spawn**, insieme al resto del preflight: un token invalido è un errore di invocazione, non un guasto scoperto al momento di scrivere il file |
| costruzione | solo attraverso un costruttore che valida. Non esiste un `CommitToken` che non sia già valido, quindi nessun percorso deve ricontrollarlo |
| negli errori | **mai il valore grezzo**. È una regola che il progetto ha già — gli errori non trasportano dati (`AGENTS.md`) — e qui vale doppio: il token è un identificativo che il chiamante considera suo |

La lunghezza fissa non è un dettaglio estetico: rende il campo del footer di
dimensione nota, quindi il limite sul footer non dipende da che cosa il
chiamante ha passato.

**Che cosa il costruttore garantisce, e che cosa no.** Vede sessantaquattro
caratteri, quindi può garantire **forma, lunghezza e alfabeto** — e nient'altro.

| | |
|---|---|
| garantito dal tipo | la stringa è canonica: `[0-9a-f]{64}`, una sola forma per un solo valore |
| **non** garantito | che i 32 byte siano **casuali**. Un token di soli zeri è formalmente valido, e non c'è modo di distinguerlo guardandolo |
| **non** garantito | l'**unicità**, che resta la precondizione esterna dichiarata sopra |

La generazione con un generatore **crittograficamente sicuro** è quindi una
**raccomandazione al chiamante**, non una proprietà del tipo: rende la
collisione accidentale trascurabile, e non impedisce a nessuno di riusare
deliberatamente lo stesso token — nel qual caso ottiene un token valido e una
risoluzione che non distingue.

Dire «32 byte di entropia», come faceva la stesura precedente, attribuiva al
costruttore una garanzia che non può dare: l'entropia non è una proprietà
osservabile di una stringa.

L'ultima riga ha una conseguenza concreta sulla diagnostica: quando un
artefatto porta un token diverso da quello atteso, l'errore dice *che* sono
diversi, non *quali* sono. Chi deve confrontarli ha già il proprio, e l'altro
non gli appartiene.

### `risolvi_commit` rende osservazioni, non decisioni

```
    risolvi_commit(commit_token, destinazione) -> OsservazioneDelCommit
```

Non è un metodo dell'oggetto d'esecuzione, perché quell'oggetto potrebbe non
esistere più. E non conclude al posto di chi chiama:

| osservazione | che cosa è stato visto |
|---|---|
| `CommittedMatching` | la destinazione esiste, porta **questo** token, **e** sigillo e struttura sono validi |
| `OccupiedByOtherAttempt` | esiste e porta un token diverso |
| `IdentityMissing` | esiste ma non porta alcun token |
| `Absent` | la destinazione non esiste |
| `InvalidOrUnreadable` | esiste ma non è leggibile, o non supera la verifica strutturale — **con la ragione**, vedi sotto |

Tre cose che la stesura precedente sbagliava, e che questa tabella corregge.

**«Il contenuto precedente» non è osservabile.** Con `(token, destinazione)`
non c'è modo di sapere che cosa ci fosse prima: un file che esiste e non porta
il nostro token è `IdentityMissing` o `OccupiedByOtherAttempt`, e quale dei
due lo dice il token, non la storia.

**`Absent` non significa «riprova pure».** Un file può mancare perché il
commit non è avvenuto, ma anche perché la durabilità è andata persa, perché
qualcuno l'ha rimosso, o perché il filesystem è tornato indietro. Riprovare
può essere giusto, e la decisione è di chi conosce quel percorso — non nostra.

**`CommittedMatching` richiede la verifica, non solo la chiave.** Una chiave
uguale su un file troncato direbbe «riuscito» di un output che non lo è: il
sigillo e la struttura vanno riletti, con lo stesso verificatore in streaming
della §2-ter.

**`InvalidOrUnreadable` non cancella la causa.** Il nome dice che non si può
concludere, non che non si sappia perché: l'osservazione porta una **ragione
strutturata** — permesso negato, framing non valido, sigillo assente, sigillo
non corrispondente, footer rifiutato dal confine ostile — perché le decisioni
che ne seguono sono diverse. Un permesso negato si risolve con i permessi; un
sigillo che non corrisponde è un file da non toccare.

La ragione è **sanitizzata** come ogni altro errore del progetto: dice di che
genere di guasto si tratta, non che cosa conteneva il file. Nessun byte dei
dati, nessun frammento di percorso oltre quello che il chiamante ha già
passato.

Il passo che conta resta a monte: il token si verifica **prima** della
pubblicazione (passo 8-bis), altrimenti si controllerebbe a valle un campo che
nessuno ha controllato quando contava.

Il `commit_token` non è un'identità globale né un lock, e la §7-bis dichiara
la precondizione da cui dipende.

**Due esecuzioni sulla stessa destinazione non si sovrascrivono**: la prima
che pubblica vince, la seconda fallisce con `Conflict`. La stesura precedente
scriveva «vince l'ultima», che contraddiceva sia il passo 9 della verifica —
dove una destinazione già esistente è un fallimento — sia il codice che c'è
già.

### Il commit point è una creazione no-clobber, non sempre un rename

`persist_noclobber` di `tempfile` — che il percorso in-process usa da tempo —
non è una primitiva sola. Legge il sorgente di `tempfile` 3.27.0:

| condizione | che cosa fa davvero |
|---|---|
| kernel e filesystem offrono `RENAME_NOREPLACE` | `renameat_with(NOREPLACE)`: commit atomico, nessuna sostituzione |
| `ENOSYS` o `EINVAL` | ripiega su **`hard_link` seguito da `unlink`** |

Nel ripiego il commit point è il `hard_link` — che fallisce se il nome esiste,
quindi il no-clobber regge — ma **l'`unlink` che segue ha l'errore ignorato**,
con tanto di commento nel sorgente. Se fallisce, il file temporaneo resta
dov'era, senza che nessuno lo dica.

C'è un dettaglio in più che il sorgente rivela, e va detto con precisione
perché una stesura precedente lo aveva descritto male. I due errori non sono
trattati allo stesso modo:

| errore | che cosa significa | effetto |
|---|---|---|
| `ENOSYS` | la **syscall non esiste** nel kernel | viene registrato in uno **static globale di processo**: tutte le pubblicazioni successive usano il ripiego, senza riprovare |
| `EINVAL` | tipicamente il **filesystem** non la supporta | ripiego **solo per quella chiamata**; lo static non viene toccato, e la chiamata dopo riprova |

La distinzione conta: un filesystem che non supporta la primitiva non
degrada l'intero processo, mentre un kernel che non ha la syscall sì — ed è
corretto, perché nel secondo caso non c'è nulla da riprovare.

**Il commit point si chiama quindi «creazione no-clobber della
destinazione»**, e non «rename». La proprietà su cui il disegno poggia — la
destinazione non viene mai sostituita — regge in entrambi i rami. Ciò che non
regge è il silenzio del ripiego, e va chiuso, perché contraddice la politica
per cui un cleanup fallito è un esito riportato (§8.3, riga 16 della matrice):

| decisione | conseguenza |
|---|---|
| **il preflight accerta la primitiva** e rifiuta il filesystem che non la offre | il ripiego non si presenta mai, ma un percorso di destinazione su un filesystem comune diventa non utilizzabile col profilo isolato |
| **si accetta il ripiego e si rileva il residuo**: dopo il commit si verifica che il temporaneo non ci sia più, e se c'è lo si riporta | nessun silenzio, e il profilo resta utilizzabile ovunque |

**Si sceglie la seconda**, con l'accertamento del preflight come
*informazione* e non come rifiuto: la prima renderebbe il profilo isolato
indisponibile per una ragione che non riguarda la memoria, che è ciò che
questa fase deve governare. La rilevazione del residuo è la stessa avvertenza
machine-readable della §10.2 — un successo pubblicato con spazzatura da
bonificare, non un fallimento.

### «Esito ignoto» non è un errore che restituiamo

`CommitOutcomeUnknown` non è una variante d'errore che qualcuno riceve: è il
**nome della situazione** in cui si trova un chiamante che non ha ricevuto
nulla. Nessuno gliela comunica, perché il processo che avrebbe dovuto
comunicargliela è morto. Metterla fra le varianti pubbliche di `PlenoraError`
sarebbe stato un errore di categoria.

Ciò che è pubblico è invece `risolvi_commit`, e il tipo che rende.

**Per il profilo isolato il `commit_token` è obbligatorio.** Non «esegue lo
stesso, con una garanzia in meno»: quella formulazione, che la stesura
precedente conteneva insieme al suo contrario, farebbe dipendere una garanzia
da ciò che il chiamante si è ricordato di fare. Un'invocazione del profilo
isolato senza token è **rifiutata in validazione**, come un piano che chiede
l'isolamento senza dichiarare il tetto (§2-quinquies).

L'`execution_id` resta quello che è: generato dall'engine, mai chiesto al
chiamante, e senza alcun ruolo nella risoluzione.

---

## 7-ter. Che cosa `memory.max` garantisce davvero

Il prototipo ha misurato cinque esecuzioni con il picco **esattamente** pari
al tetto. È un risultato pulito, e sarebbe scorretto trasformarlo in una
promessa: cinque misure su un kernel sono evidenza sperimentale, non la
semantica della primitiva.

Il kernel dichiara che l'uso **può superare temporaneamente** `memory.max`
prima che il reclaim o l'OOM killer intervengano. La garanzia che possiamo
fare è quindi quella del meccanismo, non quella del numero:

| non promettiamo | promettiamo |
|---|---|
| che nessun byte oltrepassi mai il tetto | che il tetto sia **imposto**: raggiunto il limite, il kernel recupera memoria e, se non basta, uccide |
| che il superamento sia innocuo | nulla: il kernel promette reclaim e OOM secondo la semantica di `memory.max`, **non** che durante un superamento temporaneo non si compia lavoro |
| un massimo istantaneo verificabile byte per byte | che l'esito di un dominio che ha superato il tetto sia comunque **classificato**, non ignorato |
| un valore di picco riproducibile | che il picco osservato sia una misura, riportata con le sue ripetizioni |

**Sul superamento la misura dice una cosa scomoda.** La prima stesura
affermava che non era mai stato osservato: era un'affermazione senza prove,
perché il picco dell'antenato non era mai stato letto. Ora lo è, e il quadro
non è quello che ci si aspetta da nessuna delle due parti:

```
    padre   memory.max    33 554 432
    padre   memory.peak   33 554 432     <- superamento: zero
    dominio memory.peak   33 787 904     <- 233 472 oltre il picco del padre
```

Il kernel definisce `memory.peak` come il massimo del cgroup **e dei suoi
discendenti**. I due valori non rispettano quella relazione: il figlio supera
il padre che lo contiene.

**La conclusione onesta è che non se ne può trarre nessuna.** La stesura
precedente usava il picco del padre per concludere «superamento zero» e nello
stesso paragrafo dichiarava i picchi inaffidabili: non si possono fare
entrambe le cose. Se i due contatori sono incoerenti, il valore del padre non
prova più di quello del figlio.

Quindi, su questo kernel e in questa configurazione: **i due `memory.peak` non
sono confrontabili in modo coerente, e il superamento temporaneo non è né
dimostrato né escluso.**

Resta una conseguenza sola, ed è già sufficiente: **i picchi non fondano
garanzie**. Servono al dimensionamento, con la loro incertezza dichiarata.

Un'ipotesi sul perché — che l'addebito venga registrato al livello del figlio
e respinto più in alto, quindi *tentato* e mai posseduto — sta nel registro delle misure
([`prototipi-isolamento.md`](prototipi-isolamento.md)) e **resta un'ipotesi**:
diventerebbe una conclusione solo con una sonda dedicata, che non è stata
scritta.

La differenza conta per chi dimensiona un host: sommare i tetti dei domini dà
una stima, non un massimo garantito. È la stessa ragione per cui il picco
complessivo del sistema è una guida e non una promessa (§2-quater).

---

## 7-quater. Il token tocca l'artefatto, quindi tocca il determinismo

Scrivere l'identificativo nei metadati dell'artefatto non è un dettaglio
implementativo: cambia i **byte** del file prodotto. Tre cose vanno conciliate
prima di scriverne una riga, e sono la ragione per cui `PR-5` cambia semantica.

### 1. Il determinismo di livello 2

L'oracolo dice: *stesso input, stesso IPC byte per byte*. Con
l'identificativo dentro l'artefatto, due esecuzioni dello stesso piano sugli
stessi dati producono byte diversi — a meno di dichiarare che
il `commit_token` **fa parte dell'input dell'esecuzione**.

È la formulazione corretta, non un cavillo: l'identificativo *è* un ingresso,
scelto dal chiamante prima dell'invocazione come il percorso di destinazione o
il tetto di memoria. Il determinismo diventa quindi:

> stesso piano, stessi dati **e stesso `commit_token`** producono lo stesso
> IPC byte per byte.

L'oracolo esistente va aggiornato di conseguenza: fissare un token nel caso di
prova, invece di generarne uno e poi normalizzarlo via. Normalizzare sarebbe
la scorciatoia sbagliata — nasconderebbe proprio il campo che si vuole
sorvegliare.

Il footer non sottrae nulla a questo punto: i suoi byte fanno parte del file,
quindi un token diverso dà un file diverso. Ciò che il footer risolve è
l'**altro** problema, quello del contratto.

### 4. Il footer entra nel confine ostile, e oggi non c'è

Adottare il footer risolve il problema del contratto e **ne apre uno nuovo**:
una parte del file che nessuno validava comincia a contare.

Il confine ostile per Arrow IPC che il progetto ha
([`errori-e-limiti.md`](errori-e-limiti.md)) percorre il footer e ne valida i
campi 1, 2 e 3 — lo Schema, i blocchi dei dizionari, i blocchi dei record
batch. **Il campo 4, i custom metadata, non è percorso affatto**: finora non
lo leggeva nessuno.

E il modo in cui `arrow-ipc` 59.2.0 lo legge rende la lacuna urgente, perché
il percorso del footer non è difensivo come quello dello schema:

| dove | come legge |
|---|---|
| metadati dello **schema** (`convert.rs`) | `if let (Some(k), Some(v)) = (kv.key(), kv.value())` — una voce senza chiave o senza valore viene saltata |
| metadati del **footer** (`reader.rs`) | `kv.key().unwrap()`, `kv.value().unwrap()` — una voce senza chiave o senza valore **panica** |

Un footer malformato supererebbe quindi la nostra prevalidazione e
raggiungerebbe una primitiva di panic dentro la dipendenza. È esattamente ciò
che il confine ostile esiste per impedire.

C'è un secondo problema, indipendente dal panic: Arrow costruisce una
`HashMap`, quindi **chiavi duplicate si comprimono con «vince l'ultima»**. Per
un token autoritativo è inaccettabile: due voci `plenora.commit.token` con
valori diversi darebbero un file che sembra appartenere a chi ha scritto per
ultimo, e non c'è nessuna ragione per cui debba essere lui.

**La lacuna è più larga del footer.** Il validatore che il progetto ha —
`fb_key_value` — accetta chiave o valore **assenti**: se l'offset è zero, non
valida e prosegue. È lo stesso helper usato in tre punti — campi, schema,
messaggi — quindi rinforzare il solo footer lascerebbe la stessa apertura
altrove. Per la regola secondo cui **ogni difetto è una classe**
([`AGENTS.md`](../AGENTS.md)), la correzione è nell'helper condiviso e vale
per tutti i chiamanti.

### I limiti, con autorità e valori

Il tetto esistente sul footer, 16 MiB, **non basta**: limita i byte, non il
numero di elementi, e centomila coppie minuscole stanno comodamente dentro 16
MiB producendo centomila allocazioni nella mappa.

| costante | valore | da dove viene |
|---|---|---|
| `MAX_IPC_CUSTOM_METADATA_PAIRS` | **256** | l'uso legittimo più denso è una colonna geometrica, con ~10 chiavi `plenora.geometry.*` più le estensioni Arrow. Le chiavi di geometria sono **per campo**, non di schema, quindi il conteggio non cresce col numero di colonne |
| `MAX_IPC_CUSTOM_METADATA_KEY_BYTES` | **128** | la chiave più lunga che il progetto scrive è `plenora.geometry.crs_definition_format`, 38 byte. Tre volte tanto lascia spazio senza aprire la porta |
| `MAX_IPC_CUSTOM_METADATA_VALUE_BYTES` | **64 KiB** | il valore legittimo più grande che il progetto scriva è una definizione di CRS, e quel tetto oggi vale 64 KiB. Il numero coincide; l'**autorità no** — vedi sotto |

I tre si applicano **prima di qualunque allocazione proporzionale al
conteggio**, che è il punto: il conteggio si legge dal vettore flatbuffer e si
confronta col tetto senza costruire niente.

**Perché non riusare `MAX_CRS_DEFINITION_BYTES`.** Una stesura precedente lo
derivava da lì, e sarebbe stato un accoppiamento sbagliato: quella costante
governa **una definizione di CRS**, questa governa **un confine IPC
generico**. Che oggi valgano entrambe 64 KiB è una coincidenza numerica, non
un'identità di autorità: il giorno in cui il tetto sul CRS cambiasse — perché
un formato nuovo richiede definizioni più lunghe — cambierebbe **in silenzio**
che cosa il parser IPC accetta, e nessuno collegherebbe le due cose. Ogni
confine ha la sua costante e la sua motivazione.

**Che cosa questo rifiuta, e va detto.** Arrow consente metadati arbitrari:
un file con una chiave sconosciuta e un valore da 100 KiB è un file Arrow
**valido**, e questo confine lo **rifiuta di proposito**. È la stessa scelta
che il progetto ha già fatto altrove — un confine ostile accetta meno di quanto
lo standard permetta — e va dichiarata, non scoperta da chi vede fallire un
file che `pyarrow` legge senza storie.

### Dove vivono, e chi ne è l'autorità

Due cose distinte che la stesura precedente confondeva:

| | |
|---|---|
| **autorità normativa** | [`errori-e-limiti.md`](errori-e-limiti.md), il registro unico imposto da [`AGENTS.md`](../AGENTS.md). Ogni limite vi **andrà** registrato con regola, perimetro, pericolo e condizione di rientro. Al momento non c'è: è un criterio d'uscita di `PR-0`, non un fatto |
| **autorità runtime** | `IpcLimits`, la struttura che il trasporto consulta a ogni lettura. È il posto da cui il codice legge, non il registro che dichiara perché |

**I tre tetti non diventano campi di `IpcLimits`.** La struttura è **pubblica
e riesportata** da `plenora-engine`, quindi aggiungere campi romperebbe ogni
inizializzazione letterale che un consumatore abbia scritto. Ma soprattutto:
`IpcLimits` esiste per i limiti che un piano può **modulare** — quanti batch,
quanto grande un body — mentre questi sono **tetti duri contro l'abuso**, e
un tetto contro l'abuso che il chiamante può alzare non è un tetto.

Restano quindi costanti interne, non ampliabili e non configurabili. La
decisione è presa qui perché altrimenti emergerebbe durante l'implementazione,
dove «aggiungo un campo» sembra la cosa naturale da fare.

| domanda | politica |
|---|---|
| UTF-8 | chiave e valore devono essere **UTF-8 valido**, verificato da noi. Non si delega a un accessore che potrebbe non controllarlo |
| chiave vuota | **rifiutata**: non ha significato, e più chiavi vuote sarebbero duplicati per costruzione |
| valore vuoto | **accettato**. Rifiutarlo romperebbe file legittimi che rappresentano un campo assente con la stringa vuota; per la chiave del token la forma canonica lo esclude comunque |
| chiave sconosciuta | **accettata e ignorata**. Il confine ostile valida la **forma**, non il vocabolario: rifiutare le chiavi altrui romperebbe l'interoperabilità con qualunque produttore Arrow che aggiunga le proprie |
| chiave del token duplicata | **rifiutata**, come ogni duplicato: è la ragione per cui il duplicato va rifiutato e non risolto |

### Chi controlla che cosa

Dire «la correzione sta nell'helper» lascerebbe senza proprietario il
controllo dei duplicati, che nessun singolo elemento può fare:

| funzione | responsabilità |
|---|---|
| `fb_key_value` | valida **una** coppia — presenza, UTF-8, lunghezze — e **restituisce** chiave e valore invece di scartarli |
| `fb_custom_metadata` | vede l'**intera collezione**: applica il tetto sul conteggio prima del ciclo, raccoglie ciò che la prima le rende, e rifiuta i duplicati |

I duplicati appartengono alla seconda perché sono una proprietà
dell'insieme, non di un elemento.

### I test non sono sette, e non sono di una PR sola

I tre tetti vanno superati **separatamente** — un test che li supera insieme
non dimostra quale dei tre abbia parato — e i casi si dividono fra due PR,
perché `PR-0` viene **prima** che `CommitToken` esista e non può avere come
criterio d'uscita qualcosa che non c'è ancora.

**A `PR-0`, i dodici casi strutturali:**

| | caso |
|---|---|
| 1-3 | ciascuno dei tre tetti superato **da solo** |
| 4 | chiave assente (offset zero) |
| 5 | valore assente (offset zero) |
| 6 | chiave vuota — rifiutata |
| 7 | valore vuoto — **accettato** |
| 8 | UTF-8 invalido nella chiave |
| 9 | UTF-8 invalido nel valore |
| 10 | duplicati con lo stesso valore |
| 11 | duplicati con valori divergenti |
| 12 | chiavi sconosciute — **accettate e ignorate** |

**A `PR-5`, i casi del token**, che presuppongono il tipo:

| | caso |
|---|---|
| 13 | token assente dal footer |
| 14 | token presente e canonico |
| 15 | token presente e **non** canonico |
| 16 | oracoli del footer: stesso token stessi byte, token diverso e contratto invariato |

`F4-22` descrive la copertura complessiva; la tabella delle PR attribuisce
ogni test a chi può davvero scriverlo.

Il requisito è `F4-22` ([`stato-e-roadmap.md`](stato-e-roadmap.md)), e va
chiuso **prima** che qualcuno scriva un token in un footer — in una PR sua,
`PR-0`, perché non è lavoro di isolamento.

### Le tre cose che l'oracolo deve verificare

| | verifica |
|---|---|
| **1** | stesso piano, stessi dati, **stesso token** → stessi byte IPC |
| **2** | token **diverso** → `Schema`, `DataContract` e `plan_hash` **identici** |
| **3** | il verificatore e `risolvi_commit` leggono il token **dal footer validato**, non dalla `HashMap` che Arrow restituisce |

La seconda è quella che vale la pena scrivere per prima: è la proprietà che il
footer regala e che con il token nello schema sarebbe stata falsa.

### 2. Il token sta nel footer, non nello schema

La stesura precedente affermava che un contenitore Arrow IPC non ha un posto
per byte propri fuori dallo schema. **È falso**, e la conseguenza era un
disegno molto più complicato del necessario.

Il formato IPC ha i **custom metadata di file**, che vivono nel *footer* e non
nello `Schema`. `arrow-ipc` 59.2.0 li espone da entrambi i lati, e c'è persino
un test di round-trip nel crate:

| | |
|---|---|
| scrittura | `FileWriter::write_metadata(chiave, valore)`, che finisce nel footer al `finish` |
| lettura | `FileReader::custom_metadata() -> &HashMap<String, String>` |

Il percorso d'uscita usa già `FileWriter`
([`output.rs`](../crates/plenora-engine/src/executor/output.rs)), quindi non
serve nemmeno cambiare come si scrive.

Mettendo lì il token, quattro cose restano invariate **per costruzione** —
non per una regola che qualcuno deve ricordarsi di applicare:

```
    commit_token  ->  custom metadata del footer
    Schema        ->  invariato
    DataContract  ->  invariato
    plan_hash     ->  invariato
    confronti     ->  invariati
```

**Perché la proiezione semantica era peggio.** Sarebbe servita perché il
token, stando nello schema, sarebbe entrato in tre posti: `DataContract` che
conserva l'intero `Schema`, il canonico del piano che include
`sorted_metadata` di *tutti* i metadati, e il confronto dei contratti
nell'esecutore. Escluderlo avrebbe richiesto una regola applicata in tre
punti, con l'obbligo di ricordarsene per sempre — e il rischio peggiore: che
un giorno un namespace escluso «perché operativo» diventasse semanticamente
importante e nessuno se ne accorgesse, perché la proiezione lo toglieva prima
che qualcuno lo guardasse.

Con il footer quel rischio non esiste: lo schema resta l'unica fonte del
contratto, e nulla vi viene tolto.

L'artefatto resta un **singolo file Arrow IPC standard**, leggibile da
chiunque, `pyarrow` compreso.

### 3. Chi lo verifica, e quando

Il supervisore lo confronta al passo 8-bis della verifica, **prima** del
publish: se l'artefatto porta un token diverso da quello trasmesso nel
`Saluto`, non è di questo tentativo e non si pubblica. È l'unico modo
per cui la lettura del chiamante in §7-bis significhi qualcosa — altrimenti si
verificherebbe a valle un campo che nessuno ha verificato a monte.

---

## 8. Cancellazione, timeout, cleanup

### 8.1 Cancellazione

| fase | comportamento |
|---|---|
| prima dell'avvio | nessun processo: si termina subito |
| dopo l'avvio | il supervisore invia `Annulla`, poi attende un **margine di cortesia** |
| scaduto il margine | terminazione forzata del dominio di isolamento |

Il margine esiste perché un worker che sta chiudendo un file ordinatamente
lascia meno lavoro al cleanup di uno terminato a metà. Non è una garanzia di
reazione (`NG-4`): è una preferenza con una scadenza.

### 8.2 Timeout

Due timeout distinti, perché misurano cose diverse:

| timeout | misura | scadenza significa |
|---|---|---|
| **handshake** | dall'avvio alla `Risposta` | il worker non è vivo o non parla il protocollo |
| **esecuzione** | dall'`Incarico` all'`Esito` | il worker è vivo ma non finisce |

Un timeout non è mai classificato come `ResourceLimit`: scadere non è prova di
esaurimento (`F4-2`).

### 8.3 Cleanup

Attraversato da ogni cammino. Rimuove l'artefatto temporaneo e la directory
che lo conteneva.

Un fallimento del cleanup **non annulla** un publish riuscito: l'output è
valido e pubblicato, e ciò che resta è spazzatura da bonificare. È però un
esito distinto e riportato, perché spazzatura silenziosa che si accumula è il
modo in cui un disco si riempie senza che nessuno sappia dire quando è
iniziato.

---

## 9. Matrice delle piattaforme

| | Linux | Windows | macOS |
|---|---|---|---|
| dominio di isolamento | cgroup v2 dedicato | Job Object | — |
| meccanismo del limite | `memory.max` | limite di memoria del job | — |
| **come nasce vincolato** | processo intermedio che entra nel cgroup e poi si sostituisce con `exec`: il worker non esiste mai fuori. Nessuna finestra e **nessun `unsafe`** | processo intermedio che entra nel Job e genera il worker, che eredita l'appartenenza: dentro dalla prima istruzione | — |
| **come si ferma** | il kernel **uccide** (`SIGKILL`) | il sistema **nega** l'allocazione, il processo resta vivo (`NG-10`) | — |
| **serve `unsafe`?** | **no** | **sì**, inevitabilmente: i Job Object sono solo FFI | — |
| evidenza di attribuzione | i **quattro delta** della §10.0-bis, locali e gerarchici insieme | — (piattaforma non supportata) | — |
| terminazione forzata | `cgroup.kill`, che non consulta `oom_score_adj` | — | — |
| stato | **prototipato**: nascita, contenimento e attribuzione, quest'ultima **solo con dominio sigillato** | **prototipato**: nascita e contenimento sì, **attribuzione non infallibile**; e nessuna implementazione possibile senza `unsafe` → **non supportato** | **non supportato** (`F4-6`, `NG-6`) |

**«Prima dello spawn» non basta, e il meccanismo cambia per piattaforma.**
La macchina a stati §3.1 dice che il limite precede l'avvio; qui si dice
*come*, perché le due piattaforme lo ottengono in modi diversi e con residui
diversi.

**Linux — nessuna finestra, e senza `unsafe`.** Il prototipo ha verificato che
`clone3` e `CLONE_INTO_CGROUP` sono disponibili, ma ha anche trovato che non
servono. Un processo intermedio scrive il proprio identificativo in
`cgroup.procs` — una scrittura su file — e poi si sostituisce con l'immagine
del worker tramite `exec`, che in Rust è una funzione **sicura**. Il worker non
esiste mai fuori dal dominio: la sua immagine viene mappata quando il processo
è già dentro, e il tetto vale già durante il caricamento — con un tetto di
128 KiB il worker è stato ucciso prima di riuscire a stampare una riga.

Fra le due strade si sceglie questa, perché dà la stessa proprietà senza
`unsafe`. `clone3` resta la riserva documentata se un caso futuro la
richiedesse.

La strada alternativa — `fork` e poi spostamento nel cgroup — **non è
accettabile**: fra la nascita e lo spostamento c'è una finestra in cui il
processo alloca senza tetto, e una finestra è abbastanza. Se nessuno dei due
meccanismi fosse disponibile, il profilo isolato non è supportato lì: non
esiste un ripiego silenzioso.

**La delega non è gratuita.** Un cgroup non-radice non può avere insieme
processi propri e controller delegati. Dentro un container, `/sys/fs/cgroup`
**non è la radice reale**: la regola si applica, e la prima esecuzione del
prototipo ha fallito con `EBUSY` su ogni scenario finché il cgroup corrente non
è stato svuotato. Il supervisore dovrà farlo, o dichiarare il profilo isolato
non disponibile. E va detto senza attenuazioni: il prototipo ha richiesto
privilegi e namespace di cgroup privato, quindi **in un container Docker
predefinito, con `/sys/fs/cgroup` in sola lettura, il profilo isolato non
gira**.

**Il dominio va sigillato come foglia, e non e' un dettaglio.** Il worker
che vive nel dominio non riesce a delegargli il controller della memoria —
glielo impedisce la regola dei processi interni — ma **se prima si sposta in
un sottogruppo, il dominio resta vuoto e la delega passa**. Il prototipo ha
eseguito la sequenza intera: da quel momento `memory.events.local` non vede
piu' l'uccisione, e il supervisore classificherebbe `Internal` un guasto di
risorse.

**Il sigillo è una scrittura su un file, e il worker può riscriverla.** È il
punto che la prima stesura trattava come uno strato fra tre, e che il
prototipo ha ribaltato: con gli **stessi privilegi** del supervisore — il caso
normale di una libreria embedded, dove coordinatore e worker girano con lo
stesso UID — il worker ha rimesso `cgroup.max.depth` a 10, `memory.max` a
1 GiB, `memory.oom.group` a 0, e infine **è uscito dal dominio** spostandosi
nel cgroup del supervisore. Cinque riscritture su cinque, tutte dopo che il
preflight aveva finito.

Il preflight verifica quindi uno stato che il worker può cambiare un istante
dopo. Non è un difetto del preflight: è che mancava il meccanismo.

### La proprietà non è «UID distinto»

L'UID è il modo con cui il prototipo l'ha ottenuta, non la proprietà. Scriverla
come «UID distinto» la renderebbe una ricetta, e una ricetta si aggira senza
violarla. La proprietà è:

> **il worker non possiede alcuna autorità capace di modificare il proprio
> dominio o di abbandonarlo.**

Il che vuol dire, per esteso, che nessuna di queste gli dà scrittura sul
control plane:

| via | perché va guardata |
|---|---|
| `uid` effettivo, reale e salvato | il salvato può essere riacquisito |
| `gid` e **gruppi supplementari** | un gruppo dimenticato che possiede la gerarchia annulla l'UID distinto |
| **capability** — `CAP_DAC_OVERRIDE`, `CAP_SYS_ADMIN`, `CAP_SETUID` | scavalcano i permessi del filesystem, quindi l'UID non basta |
| **namespace** — utente, mount, cgroup | un namespace utente proprio può rendere il worker `root` rispetto alla gerarchia |
| **descrittori scrivibili ereditati** | un descrittore già aperto su un file del control plane sopravvive alla `exec` e non passa dai permessi |
| permessi sulla gerarchia | proprietario e modo delle directory e dei file del dominio |

L'ultima riga è quella che il prototipo ha misurato; le altre cinque sono
strade che portano allo stesso posto senza toccarla.

### Il provider iniziale, e perché uno solo

Le tre forme non sono equivalenti, perché **una sola è stata provata**:

| forma | stato |
|---|---|
| **UID/GID distinto** con gerarchia di proprietà del control plane | **provata**: `L19b`, cinque operazioni su cinque respinte con `EACCES`. È il **provider iniziale** |
| helper proprietario del control plane | non prototipata. Separerebbe chi configura da chi esegue anche a parità di UID del chiamante, al prezzo di un processo e di un canale in più |
| mount namespace con gerarchia non scrivibile | non prototipata |

Il profilo isolato su Linux nasce quindi con **un** provider. Le altre due
restano strade documentate per ambienti in cui il coordinatore non può
cambiare identità al figlio, e ciascuna richiederà il proprio prototipo prima
di essere offerta: metterle qui come alternative equivalenti significherebbe
promettere due meccanismi che nessuno ha verificato.

### Si verifica in `PreparaIsolamento`, non nel piano

La disponibilità della separazione **non è una proprietà del piano**: dipende
da chi esegue, con quali privilegi, in quale ambiente. Lo stesso piano è
eseguibile su una macchina e non su un'altra, quindi respingerlo in validazione
sarebbe respingere la cosa sbagliata.

Il controllo sta in `PreparaIsolamento` (§3.1), **prima dello spawn**: il
supervisore accerta di poter costruire un dominio che il worker non potrà
toccare, e se non ci riesce l'esito è `IsolationUnavailable` — una condizione
d'ambiente, con un nome proprio, distinta da un piano invalido.

Un ambiente in cui il worker può riscrivere il proprio dominio non è un
ambiente in cui il profilo isolato vale meno: è un ambiente in cui **non
vale**.

Resta il terzo elemento, che non è uno strato di difesa ma di lettura:
`memory.events` gerarchico e degli antenati (§10.0-bis). Non sostituisce la
separazione dei privilegi — dice *che* è successo, non *a chi* — e serve a non
perdere l'informazione quando l'attribuzione è comunque compromessa.

**`memory.oom.group` è obbligatorio, non consigliato.** Senza, il prototipo ha
osservato un figlio ucciso per il limite e il processo capofila **vivo, con
uscita 0**: un successo apparente sopra un guasto di risorse. Con
`memory.oom.group=1` il dominio muore intero e nessuno può dichiarare un
successo che non c'è.

**Windows — non supportato, e perché.** Il prototipo ha dimostrato nascita
vincolata e contenimento, ma **non** un'attribuzione utilizzabile, e nessuna
implementazione è possibile senza `unsafe`. Le misure — le tre fonti di
evidenza, la loro durata, il residuo del loader — restano in
[`prototipi-isolamento.md`](prototipi-isolamento.md) come **motivazione**, non
come progetto: nulla di ciò che descrivono è normativo per questa fase.

In sintesi, e senza riaprire il merito: la notifica del job non è garantita,
la violazione interrogabile scade in poche decine di millisecondi — prima che
la barriera di quiescenza possa leggerla — e l'unico indicatore durevole ha
falsi positivi misurati, mentre l'assenza di falsi negativi **non** è stata
dimostrata. Non si può quindi nemmeno affermare che il rischio residuo sia
solo «rifiutare output validi».

La condizione di rientro è in §11.2: una dipendenza vettata che copra tetto
**ed** evidenza senza `unsafe`.

**macOS.** Nessun meccanismo è stato prototipato. Finché un prototipo non
dimostra **contenimento** e **attribuzione** — entrambi, non uno — il profilo
isolato non è supportato e l'esecuzione resta in-process con le garanzie
attuali, che sono minori e dichiarate.

---

## 9-bis. Preflight del dominio: scrivere non è configurare

Ogni proprietà su cui poggia l'attribuzione è una **scrittura su un file**
che può fallire in silenzio, essere ignorata da un kernel diverso, o essere
sovrascritta da qualcun altro. Il preflight le scrive e **le rilegge**, e il
profilo isolato non parte se una sola diverge.

| | si scrive | si rilegge e deve valere | se diverge |
|---|---|---|---|
| tetto | `memory.max` | esattamente il valore chiesto, dopo il taglio della politica dell'host | il dominio non è limitato: **non si parte** |
| swap | `memory.swap.max` = 0 | `0` | il tetto misurerebbe un'altra cosa: **non si parte**, o si dichiara lo swap non governabile |
| group kill | `memory.oom.group` = 1 | `1` | un OOM parziale sarebbe indistinguibile da un successo (`F4-8`): **non si parte** |
| sigillo | `cgroup.max.depth` = 0 | `0` | il worker potrebbe evadere l'osservazione (`F4-9`): **non si parte** |
| quiescenza | — | `cgroup.events` leggibile, e `populated` = 0 **prima** dell'avvio | senza questo segnale la barriera non è implementabile: **non si parte** |
| uccidibilità | `oom_score_adj` = 0 nello spawner, prima della `exec` | `0` | un worker protetto resiste al group kill: **non si parte** |
| montaggio | — | opzioni di `cgroup2` in `/proc/self/mounts` | vedi sotto |

**La rilettura non è pedanteria.** Il prototipo ha misurato tutte e quattro le
scritture concordare con la rilettura in un kernel 6.18 — il che dimostra che
il controllo è a costo zero quando le cose vanno bene, non che sia inutile
quando vanno male.

### `oom_score_adj` va normalizzato **e** verificato

È il valore che si eredita e che nessuno guarda. A `-1000` il kernel non
uccide il task, **nemmeno con `memory.oom.group = 1`**: un worker che lo
eredita da un chiamante protetto — un servizio di sistema, un container con
priorità — sopravvive al group kill e riproduce `F4-8`.

Il prototipo ha misurato che il risultato è peggiore della sopravvivenza: il
dominio ha raggiunto il limite **305 volte** senza uccidere nessuno, e non
avanzava più. Non è un worker che continua: è un dominio bloccato, che
richiede `cgroup.kill` dall'esterno.

Lo spawner lo scrive a `0` prima della `exec`, lo **rilegge**, e se la
rilettura non torna esce con un codice dedicato. Scrivere senza verificare non
è normalizzare: è sperare.

### `memory_localevents` va guardato, e la risposta dipende dal sigillo

L'opzione di montaggio `memory_localevents` rende **non gerarchico** anche
`memory.events`, cioè toglie la terza riga della struttura di evidenza.

Con il dominio sigillato non ci sono discendenti, quindi locale e gerarchico
coincidono e l'opzione non cambia nulla. La conclusione però **dipende dal
sigillo**, che è a sua volta una scrittura da rileggere: se il sigillo non si
può stabilire, il profilo non parte comunque, e il caso non si presenta.

Il preflight registra perciò l'opzione senza respingerla, e la registra
**insieme** all'esito del sigillo: sono la stessa domanda vista da due lati.
Va detto che il caso non è stato misurato — la gerarchia provata non aveva
`memory_localevents` — quindi la conclusione è un ragionamento, non
un'osservazione.

---

## 10. Matrice degli esiti

`ResourceLimit` compare **solo** dove c'è evidenza attribuibile (`F4-2`).

| # | esito | evidenza | categoria | output visibile | artefatto |
|---|---|---|---|---|---|
| 1 | successo verificato | `Esito` di successo + verifica 1-8 superata | — (successo) | **sì**, pubblicato | rimosso |
| 2 | errore tipizzato del worker | `Esito` con i quattro assi | quella dichiarata dal worker | no | rimosso |
| 3 | panic nel worker | `Esito` di panic, forma del payload senza contenuto | `Internal` | no | rimosso |
| 4 | crash | il processo muore senza `Esito`, nessuna evidenza di limite | `Internal` | no | rimosso |
| 5 | OOM attribuito | la struttura di evidenza della §10.0-bis, letta dopo la quiescenza | **`ResourceLimit`** | no | rimosso |
| 5-bis | dominio non uccidibile | limite raggiunto ripetutamente, nessun task uccidibile (`oom_score_adj` non normalizzato): `Ol ≥ 1`, `Kl = 0`, `Kh = 0`, `G = 0` | **`UnattributedMemoryPressure`**, causa dichiarata | no | rimosso dopo `cgroup.kill` |
| 6a | terminazione senza evidenza | il processo muore, **nessuna** evidenza di pressione | `Internal` — **mai** dedotto come OOM | no | rimosso |
| 6b | terminazione con pressione non attribuibile | il processo muore, c'è evidenza di pressione ma non autorizza l'attribuzione | **`UnattributedMemoryPressure`** | no | rimosso |
| 7 | timeout | scadenza del timeout di esecuzione | `Timeout` | no | rimosso |
| 8 | cancellazione | `Annulla` inviato, worker terminato | `Cancelled` | no | rimosso |
| 9 | protocollo incompatibile | versione non riconosciuta nell'handshake | **`Protocol`** | no | nessuno creato |
| 10 | resolver incompatibile | disaccordo su identità, versione, capability o ambiente | **`InvalidConfiguration`** | no | nessuno creato |
| 11 | output assente | verifica passo 3 | `Internal` | no | — |
| 12 | output troncato | verifica passo 4 (sigillo) o 5 (framing) | `Internal` | no | rimosso |
| 13 | output semanticamente invalido | verifica passo 7 (contratto) | `Schema` | no | rimosso |
| 14 | fallimento della verifica | qualunque passo 1-8 non coperto sopra | secondo il passo | no | rimosso |
| 15 | fallimento del publish | verifica passata, passo 9 fallito | `Io` o `Conflict` | no | rimosso |
| 16 | fallimento del cleanup | publish riuscito, rimozione fallita | successo **con avvertenza machine-readable** | **sì** | **residuo** |

**Perché la 5-bis non è `ResourceLimit`.** Diceva «causa dichiarata», e
suonava come un'attribuzione con una nota a margine. Non lo è: senza group
kill locale non c'è l'operazione del kernel che lega il limite *di questo
dominio* alla morte *di questo dominio*, e `Ol` alto con `Kl` a zero è
compatibile con un limite raggiunto e recuperato. Dire `ResourceLimit`
manderebbe chi legge ad alzare un budget che potrebbe non essere il colpevole.

La causa resta dichiarata — `oom_score_adj` non normalizzato è una firma
riconoscibile, e va riportata — ma è **diagnostica dentro l'evidenza**, non
una categoria.

**Perché la riga 6 si divide.** «Evidenza assente o non attribuibile» metteva
sotto `Internal` due situazioni opposte. `Internal` dichiara un difetto
nostro: è giusto quando non c'è nulla da vedere (6a), ed è falso quando la
pressione c'è ed è solo l'attribuzione a mancare (6b) — lì il difetto può
essere dell'ambiente, di un vicino, o di nessuno.

Fonderle costava esattamente ciò che `F4-2` esiste per impedire, nel verso
opposto a quello sorvegliato: non «dedurre un OOM che non c'è», ma «dichiarare
un difetto nostro dove c'è una pressione che non abbiamo saputo attribuire».

**La riga 5 non presuppone la morte del worker.** Anche dove il limite si
manifesta come allocazione negata (`NG-10`), il worker può uscire con un
errore tipizzato *proprio* mentre il dominio ha registrato l'evento. Le righe
2 e 5 sarebbero entrambe candidate, e la precedenza della §10.3 decide:
**l'evidenza del dominio batte l'esito dichiarato**, quindi `ResourceLimit`.

È la scelta giusta perché l'errore che il worker riesce a riportare in quelle
condizioni è quasi sempre la conseguenza, non la causa: dire al chiamante
«fallita l'allocazione di un buffer» invece di «hai superato il tetto» lo
manderebbe a cercare un difetto dove c'è un dimensionamento.

### 10.0-bis L'evidenza è una struttura, non un booleano

La prima stesura leggeva un solo contatore. Il prototipo mostra che i segnali
sono almeno tre e che le loro combinazioni **non collassano** in un sì o un
no: `L12` ha prodotto `oom` locale a 1 con `oom_kill` locale a 0, e `L16` ha
prodotto `oom` a 305 con `oom_kill` a 0. Sono tre situazioni diverse che un
booleano appiattirebbe in una.

I segnali, tutti come **delta** fra prima e dopo l'esecuzione:

| | segnale | che cosa conta |
|---|---|---|
| `Ol` | `memory.events.local` → `oom` | quante volte il limite **di questo dominio** ha invocato l'OOM |
| `Kl` | `memory.events.local` → `oom_kill` | processi uccisi **appartenenti a questo dominio** |
| `Kh` | `memory.events` → `oom_kill` | processi uccisi nel dominio **o nei discendenti** |
| `G` | `memory.events.local` → `oom_group_kill` | il group kill è scattato |

La classificazione copre ogni combinazione, e nessuna cade in un ramo
predefinito:

Serve anche un quinto segnale, e la ragione è nella §10.0-ter: `Ol` da solo
non dice *quale* limite è stato raggiunto, perché sopra al nostro dominio ce
ne possono essere altri.

| | segnale | che cosa conta |
|---|---|---|
| `Oa` | `memory.events.local` → `oom` **degli antenati** del dominio | quale livello della gerarchia ha esaurito il proprio tetto |

### Una sola combinazione autorizza l'attribuzione

La prima stesura di questa tabella restituiva `ResourceLimit` in quattro righe
su otto, e nella sezione successiva spiegava che i delta non dimostrano una
causa. Le due cose non stanno insieme: se `Ol` e `Kl` possono appartenere a
eventi distinti, allora la loro coesistenza **non autorizza** ad attribuire.

L'unico segnale che lega causa ed effetto in un solo fatto è `G`, il group
kill **locale**: il kernel ha ucciso questo dominio *come gruppo*, e lo ha
fatto perché il tetto di questo dominio è stato raggiunto. Non è una
coincidenza di contatori, è un'operazione.

#### `G` è necessario, non sufficiente

L'attribuzione forte richiede la **riga intera**: `Ol ≥ 1`, `Kl ≥ 1`,
`Kh ≥ 1` **e** `G ≥ 1`. `G` da solo non basta.

E un `G ≥ 1` con `Ol = 0` **osservati entrambi** non è «non attribuito»: è
**incoerente**. Descrive un group kill scattato senza che il tetto di questo
dominio sia mai stato invocato, cioè una lettura che non torna, e va trattato
come il caso `Kl > Kh` — `Internal`, mai riparato in silenzio.

Con `G = None` la distinzione non si può fare: non attribuito, ma non
incoerente. Non aver letto un segnale non è una contraddizione fra segnali.

#### L'incoerenza si controlla sui valori, non sui livelli

`Kl > Kh` va verificato sui `u64` **originali**, prima di ridurli a
livelli. Dopo la normalizzazione `Kl = 2` e `Kh = 1` sono entrambi «positivo»,
la disuguaglianza sparisce, e la riga incoerente si presenta come una riga
normale. È l'ordine dei due passi a decidere se il controllo esiste.

#### `None` non autorizza mai, e non è pressione

I livelli della tabella sono `0` e `≥ 1`, e presuppongono che il segnale sia
stato **osservato**. Un `None` non è né l'uno né l'altro: significa che non lo
abbiamo letto.

Nessuna riga con un `None` autorizza l'attribuzione, e vale in particolare per
`G`: confondere «non letto» con «non è scattato» porterebbe a non attribuire
mai, confonderlo con «è scattato» porterebbe ad attribuire su un contatore mai
visto. Entrambi gli errori sono peggiori del non concludere.

Ma **«non attribuito» è a sua volta un'affermazione**, e una stesura
precedente la faceva troppo larga: diceva che un `None` qualunque dà «non
attribuito», e con

    Ol = None, Kl = 0, Kh = 0, G = 0

avrebbe dichiarato pressione di memoria non attribuibile dove non se n'è
osservata nessuna. Un'osservazione incompleta non è una pressione.

Servono quindi **cinque** classi, non quattro:

| classe | quando | esito |
|---|---|---|
| **attribuita** | tutti e quattro osservati e positivi | `ResourceLimit` |
| **assente** | tutti e quattro `Some(0)` | `Internal` |
| **non attribuita** | almeno una pressione **osservata**, prova insufficiente | `UnattributedMemoryPressure` |
| **indeterminata** | letture mancanti e **nessun** segnale positivo | `Internal` |
| **incoerente** | relazioni impossibili fra i segnali | `Internal` |

#### L'ordine delle cinque classi è normativo

Le condizioni si sovrappongono — `Ol+`, `Kl = 2`, `Kh = 1`, `G+` soddisfa sia
«tutti positivi» sia `Kl > Kh` — quindi l'ordine non è un dettaglio
d'implementazione:

1. **incoerente**, valutata sui **valori grezzi** e prima di tutto il resto;
2. **attribuita**;
3. **assente**;
4. **non attribuita**;
5. **indeterminata**.

L'incoerenza viene per prima perché è l'unica che dice «questa misura non è
una misura»: applicare le altre a una lettura rotta produrrebbe una
classificazione dall'aria normale.

**Indeterminata** e **assente** finiscono entrambe su `Internal` e non sono la
stessa cosa: la prima dice «non ho letto», la seconda «ho letto e non c'era
nulla». Tenerle distinte serve a chi legge l'evidenza — e a chi un giorno
dovrà capire perché una piattaforma classifica sempre indeterminato.

Tutte e cinque conservano l'evidenza e **nessuna pubblica**.

Nella tabella `0` e `≥1` significano **osservati**. Le righe con un `None`
sono in coda, separate, perché un `None` non è un livello.

| `Ol` | `Kl` | `Kh` | `G` | classe | perché |
|---|---|---|---|---|---|
| ≥1 | ≥1 | ≥1 | **≥1** | **attribuita** | l'unica riga con attribuzione forte: la riga **intera**, non il solo `G` |
| ≥1 | ≥1 | ≥1 | 0 | non attribuita | pressione **senza** group kill locale: qualcuno è sopravvissuto, e i due delta possono essere eventi distinti |
| ≥1 | 0 | ≥1 | 0 | non attribuita | limite del dominio raggiunto, uccisione in un discendente: il sigillo ha fallito. Più difetto del preflight |
| ≥1 | 0 | ≥1 | ≥1 | non attribuita | group kill locale con `Kl = 0`: nessun processo **del dominio** è stato ucciso, quindi la riga non è quella dell'attribuzione |
| ≥1 | 0 | 0 | 0 | non attribuita | limite raggiunto e nessun task uccidibile: firma di `oom_score_adj` non normalizzato. Causa dichiarata, richiede `cgroup.kill` |
| ≥1 | 0 | 0 | ≥1 | **incoerente** | un group kill locale che non ha ucciso nulla, né nel dominio né sotto |
| 0 | ≥1 | ≥1 | 0 | non attribuita | il worker è morto senza che il **suo** tetto sia stato raggiunto: l'OOM viene da fuori. `Oa` dice *dove* c'era pressione, non che sia stata la causa |
| 0 | ≥1 | ≥1 | ≥1 | **incoerente** | group kill locale con il tetto locale mai invocato: `G` senza `Ol` è una lettura che non torna |
| 0 | 0 | ≥1 | 0 | non attribuita | uccisione in un discendente senza pressione locale |
| 0 | 0 | ≥1 | ≥1 | **incoerente** | stessa ragione: `G ≥ 1` con `Ol = 0` |
| 0 | 0 | 0 | 0 | **assente** | nessuna evidenza, e **tutti e quattro osservati**. `Internal`, mai dedotto come OOM (`F4-2`) |
| 0 | 0 | 0 | ≥1 | **incoerente** | stessa ragione |
| `Kl > Kh`, entrambi osservati | | | | **incoerente** | `Kl` è un sottoinsieme di `Kh`: quella disuguaglianza non può accadere. Va controllata sui **valori**, non sui livelli — dopo la normalizzazione `2` e `1` sono entrambi «positivo» e il difetto sparirebbe |
| almeno un `None`, e **almeno un** positivo osservato | | | | non attribuita | qualcosa si è visto, e non basta |
| almeno un `None`, e **nessun** positivo osservato | | | | **indeterminata** | non abbiamo osservato pressione: abbiamo un'osservazione incompleta. `Internal` |

**Che cos'è «non attribuito».** Non è `ResourceLimit` — non possiamo dire al
chiamante che ha superato il suo budget quando non lo sappiamo — e non è
`Internal`, che direbbe «difetto nostro» quando spesso è l'ambiente. È una
categoria propria: *terminazione con evidenza di pressione di memoria non
attribuibile*, che porta con sé i cinque contatori e non conclude al posto di
chi legge.

`PR-1` **l'ha introdotta**, insieme alle altre varianti nuove di
`PlenoraError`, e `PR-3` la consuma: la classificazione produce
`UnattributedMemoryPressure` per queste righe. Il ripiego su `Internal`, che
valeva finché la variante non esisteva, non serve più — e lasciarlo scritto
avrebbe mandato `PR-3` a implementare un'approssimazione già superata.

**In nessuna di queste righe si pubblica.** L'attribuzione decide che cosa si
dice al chiamante, non se l'output diventa visibile: un esito terminale
ambiguo non pubblica mai (`GA-1`).

**Che cosa NON entra nella classificazione.** `Oa`, il picco, il tetto e il
contatore `max` di `memory.events.local` sono **diagnostici**: viaggiano
nell'evidenza, aiutano a leggerla, e **non cambiano la categoria**. La
classificazione guarda i quattro segnali e basta.

Non è una svista di completezza. Il picco non prova nulla in nessuna delle due
direzioni — i prototipi hanno misurato picchi del figlio superiori a quelli
letti sul padre — e `max` dice che il tetto è stato toccato, non che qualcuno
sia stato ucciso per averlo toccato. Farli pesare sulla categoria
significherebbe far dipendere l'attribuzione da misure che il progetto ha già
dichiarato non causali.

**Su `Oa`.** Dice che un antenato ha registrato pressione. Non dice che sia
stata la causa di *questa* terminazione, e tanto meno se l'antenato contiene
altri domini concorrenti — nel qual caso la pressione può venire da un
vicino. Resta diagnostico: entra nell'evidenza riportata, non nella
classificazione. L'unico caso in cui potrebbe diventare causale è un antenato
**dedicato** a un solo dominio, e allora però il tetto utile è il suo, non il
nostro.

La quinta riga è quella che la prima stesura dichiarava esaustiva senza
esserlo. `oom_kill` conta i processi del cgroup uccisi da **qualunque** OOM
killer, non solo dal proprio limite: un antenato con un tetto più basso
uccide il worker facendo salire `Kl` mentre `Ol` resta a zero. Il prototipo
l'ha riprodotta — padre a 32 MiB, dominio a 256 MiB — ottenendo `Ol = 0`,
`Kl = 1`, `Kh = 1`, con il picco del dominio fermo al tetto **del padre**.

La quarta riga è quella che il primo ciclo non sapeva esistere: `Ol` alto con
`Kl` a zero non significa «niente è successo», significa limite raggiunto
ripetutamente e nessuno da uccidere. Misurate **305 invocazioni** e **zero
uccisioni**, con il dominio fermo.

### 10.0-ter I delta non dimostrano una causa

Va detto perché la tabella qui sopra è più debole di quanto sembri: i delta
sono **contatori aggregati su un intervallo**, non un registro di eventi
ordinati. Che `Ol ≥ 1` e `Kl ≥ 1` compaiano insieme non dimostra che
l'uccisione sia stata *causata* dal limite locale: potrebbero essere due
eventi distinti nella stessa finestra — un limite raggiunto e recuperato, e
più tardi un OOM di un antenato.

Il prototipo ne ha misurato un caso: con il padre stretto, il padre registra
`oom = 1` e `oom_kill = 0` mentre il dominio registra `oom = 0` e
`oom_kill = 1`. Il *perché* e il *chi* stanno su due livelli diversi, e solo
metterli insieme dà una storia.

Ne seguono due regole, entrambe conservative:

1. **la classificazione è fail-closed sulla causalità**: l'attribuzione forte
   richiede il group kill locale, che è un'operazione del kernel e non una
   coincidenza di contatori. Ovunque manchi, non si attribuisce;
2. **l'evidenza riportata è la struttura, non la conclusione**: l'esito porta
   i cinque contatori con sé, così chi legge può ricostruire ciò che il
   classificatore ha visto invece di fidarsi della sua sintesi.

Un registro ordinato di eventi migliorerebbe la **tempestività** della
lettura, e va detto che non basterebbe: sorvegliare `memory.events` con una
notifica invece di campionarlo dice *quando* un contatore è cambiato, non
*perché*, e non lega l'evento al processo che l'ha causato. Per una vera
catena causale servirebbe una fonte diversa — tracciamento degli eventi OOM
per processo — che non è stata prototipata e che non è detto sia disponibile
ovunque.

Finché quella fonte non c'è, `G` resta l'unico legame causa-effetto
osservabile, ed è per questo che la tabella ci si appoggia interamente.

### 10.1 Tassonomia: che cosa manca davvero

`ErrorCategory` aveva già `Protocol`, `Timeout`, `Conflict` e
`InvalidConfiguration`, e **nessuna variante di `PlenoraError` le produceva**:
esistevano come categorie dichiarabili, non come errori costruibili — i loro
exit code erano irraggiungibili.

`PR-1` ha colmato il divario. Le varianti ci sono tutte, con i quattro assi
cablati, e le righe qui sotto le usano invece di ripiegare.

Le tre righe che ne avevano bisogno:

| esito | categoria | perché non le esistenti |
|---|---|---|
| protocollo incompatibile | `Protocol` | è precisamente «violazione del protocollo». `Internal` direbbe «difetto nostro», e lo è solo a volte: due binari diversi in esecuzione sono una condizione di dispiegamento |
| resolver incompatibile | `InvalidConfiguration` | **non `InvalidPlan`**: il piano può essere perfettamente valido, e lo sarebbe di nuovo con un ambiente coerente. È il componente a essere configurato male, non il piano a essere sbagliato |
| timeout | `Timeout` | oggi non c'è modo di costruirlo |

Erano **varianti nuove** di `PlenoraError`, cioè una modifica semantica di un
tipo pubblico: sono andate in una PR propria (`PR-1`), con il proprio impatto
sulla superficie e sugli exit code, prima di qualunque codice che le usasse.

`Conflict` è costruibile insieme alle altre: la riga 15 (fallimento del
publish) lo usa quando la destinazione esiste già, ed è diverso da un errore
di I/O.

### 10.2 L'avvertenza di cleanup ha un canale proprio

La riga 16 non è un errore, ma non deve nemmeno essere una stringa in un log.
L'esito di successo porta un campo dedicato, machine-readable, che dichiara
che cosa è rimasto e dove: chi amministra lo spazio su disco deve poterlo
leggere senza fare *grep*.

Un successo senza quel campo è un successo pulito. La sua presenza non cambia
la categoria dell'esito — resta successo — e non deve indurre un chiamante a
riprovare l'operazione.

### 10.3 Precedenza fra eventi concorrenti

Cancellazione, timeout, uscita del worker, evidenza di OOM e publish possono
accadere quasi insieme. Senza una precedenza dichiarata, due esecuzioni
identiche potrebbero classificarsi diversamente per una corsa.

#### Un ordine non basta: serve un istante

Una precedenza dichiarata dice **come** confrontare due eventi, non **quali**
eventi entrano nel confronto. Senza un istante in cui l'insieme si chiude, due
esecuzioni identiche possono ancora divergere: basta che un evento arrivi
mentre la classificazione è già in corso.

Chiudere la coda **non basta**, e il prototipo lo ha misurato: al ritorno
della `wait` il capofila era uscito con 0, un processo era ancora vivo nel
dominio e l'evidenza di OOM valeva **zero**; duecento millisecondi dopo valeva
**uno**. La chiusura degli ingressi dimostra che non accettiamo più eventi,
non che non ne stiano arrivando. Un OOM consegnato dopo non è un OOM avvenuto
dopo.

Serve quindi una **barriera causale**, non solo un istante di chiusura. Il
punto di linearizzazione precede il commit point ed è così definito:

1. tutti gli eventi — cancellazione, timeout, uscita del worker, evidenza del
   dominio — confluiscono in **una** coda, consumata da **un** solo esecutore.
   Non ci sono due strade per diventare un esito;
2. **quiescenza del dominio**: si attende che non vi resti alcun processo
   vivo, **nemmeno in un discendente**. Il segnale è `cgroup.events`, campo
   `populated`: il kernel garantisce che valga 1 finché il cgroup **o un suo
   discendente** contiene processi vivi, e notifica il cambiamento.

   **Non `cgroup.procs`**, che elenca il solo cgroup corrente: il prototipo ha
   misurato il caso in cui dice `0` mentre `populated` dice `1` e un processo
   del dominio è vivo un livello sotto. E nemmeno una scansione ricorsiva
   delle directory, che è una lettura non atomica di molti file mentre i
   processi si spostano — la corsa non è teorica: si è manifestata in una
   prima stesura dello scenario, e nell'unico verso che non produce un
   allarme.

   Un tempo massimo d'attesa, oltre il quale si termina il dominio con
   `cgroup.kill` e l'esito è ambiguo;
3. **snapshot e drain dell'evidenza**: si preleva ciò che il sistema ha da
   dire e ciò che il fermo del supervisore ha accumulato durante l'esecuzione.
   Su Windows il fermo è l'unica fonte utilizzabile a questo punto, perché la
   violazione interrogabile è già scaduta;
4. **classificazione definitiva**, e solo ora si chiudono gli ingressi;
5. l'unico ingresso rimasto è **l'esito della pubblicazione stessa**. Riesce: l'esito è
   successo, e la riga 1 della precedenza è soddisfatta da un fatto, non da una
   regola. Fallisce: si torna all'esito congelato al punto 4, oppure alla riga
   15 se il fallimento è del publish.

**Un OOM tardivo non è un'avvertenza.** La prima stesura degradava a
diagnostica gli eventi che arrivano dopo la chiusura: era possibile solo
perché la chiusura avveniva prima della quiescenza. Con i passi 2 e 3 al posto
giusto, un evento successivo al punto 4 riguarda per costruzione un dominio
già morto e già letto — e se ne arrivasse uno che contraddice la
classificazione, è un difetto della barriera, non un'avvertenza da riportare.

È questo che rende la riga 1 vera invece che sperata. «Publish completato
vince» non è una priorità che si applica *dopo*: è la constatazione che dopo la
chiusura degli ingressi l'unico fatto ancora capace di cambiare l'esito è
quello che stiamo per compiere noi.

Un evento che arrivi **fra** il punto 4 e il ritorno della pubblicazione non annulla
una pubblicazione avvenuta: a quel punto il dominio è quiescente da prima
dello snapshot, quindi non può trattarsi di un esaurimento del worker o del
verificatore. Resta riportabile come avvertenza, con il canale
machine-readable della §10.2, e **solo** per ciò che non riguarda le risorse
dei domini.

Chiuso l'insieme, l'ordine è **totale** e va dal fatto più esterno al meno
verificabile:

| # | evento | vince perché |
|---|---|---|
| 1 | **publish completato** | è un fatto osservabile fuori dal sistema: se l'output è visibile, l'operazione è riuscita, e nessun evento successivo può renderla non riuscita |
| 2 | **OOM attribuito** | ha evidenza specifica del dominio. Un processo con quella prova è morto per il limite, anche se stavamo cancellando |
| 3 | **timeout scaduto** | è il nostro orologio, misurato |
| 4 | **cancellazione richiesta** | è la nostra decisione, e precede l'auto-dichiarazione del worker |
| 5 | **pressione non attribuita** | c'è una prova, e non basta ad attribuire. Vale meno di un timeout e di una cancellazione, che sono fatti nostri e misurati, e più dell'auto-dichiarazione del worker, che in quelle condizioni riporta quasi sempre la conseguenza invece della causa |
| 6 | **esito dichiarato dal worker** | è un'affermazione di un processo che potrebbe essere in difficoltà |
| 7 | **terminazione ambigua** | l'ultimo, e per costruzione mai `ResourceLimit` |

Il punto 4 sopra il 5 merita una parola: se abbiamo cancellato **e** il worker
riporta successo ma non abbiamo ancora pubblicato, l'esito è `Cancelled`. Un
successo che non è diventato visibile non è un successo per chi ha chiesto di
annullare.

Il punto 2 sopra il 4 merita l'altra: se abbiamo cancellato e c'è prova di OOM,
l'esito è `ResourceLimit`. La prova batte l'intenzione — altrimenti una
cancellazione tempestiva maschererebbe un difetto di dimensionamento.

**Il livello 5 non pubblica.** Come tutti quelli sopra il 6: una pressione di
memoria non attribuita è un esito terminale, e un esito terminale ambiguo non
pubblica mai (`GA-1`). Che la categoria dica «non lo sappiamo» non la rende
meno terminale — semmai il contrario.

**Perché sotto timeout e cancellazione.** Sono fatti nostri: il timeout è il
nostro orologio misurato, la cancellazione la nostra decisione. La pressione
non attribuita è una prova che non conclude, e una prova che non conclude non
può scavalcare due fatti che concludono.

**Perché sopra l'esito del worker.** Per la stessa ragione per cui l'OOM
attribuito sta sopra: l'errore che un processo riesce a riportare mentre il
dominio è sotto pressione è quasi sempre la conseguenza, non la causa. Dire
«fallita l'allocazione di un buffer» invece di «c'era pressione di memoria che
non sappiamo attribuire» manda a cercare un difetto dove c'è forse un
dimensionamento — e nasconde che l'attribuzione manca.

Le righe 6 e 7 sono la stessa terminazione vista con e senza prova, e la
differenza fra le due è tutto il valore di `F4-2`. Un sistema che deduce l'OOM
da un segnale dice al chiamante «hai superato il budget» anche quando il
processo è morto per un difetto nostro — e il chiamante alza il budget invece
di segnalare il bug.

La riga 16 è l'unico caso in cui l'output è visibile e qualcosa è comunque
andato storto. È classificato come successo perché **lo è**: il risultato è
valido e pubblicato. L'avvertenza serve a chi amministra lo spazio su disco,
non a chi consuma il dato.

---

## 11. Suddivisione in PR

### 11.1 I due prototipi bloccanti — conclusi

Erano `PR-4` e `PR-9` nella prima stesura, cioè dopo protocollo e supervisore:
sbagliato. Se il meccanismo di piattaforma non avesse retto, protocollo e
supervisore sarebbero stati costruiti sopra un'assunzione mai verificata.
Spostati davanti a tutto, sono stati **eseguiti**.

Che cosa dovevano dimostrare, con «contenimento» detto in modo che valga per
entrambe le piattaforme e non solo per Linux:

| | proprietà | forma verificabile |
|---|---|---|
| **1** | nascita già vincolata | il worker non esiste, in nessun istante, fuori dal dominio |
| **2** | contenimento | il tetto è **imposto** secondo la semantica della primitiva — reclaim e, se non basta, uccisione — *comunque* il sistema ci arrivi. Non «nessun byte lo oltrepassa mai» (`NG-14`) |
| **3** | attribuzione | l'evento è leggibile e riferito al nostro dominio, non dedotto |
| **4** | comportamento dopo il rifiuto | se l'allocazione è **negata** invece che fatale: che cosa fa il worker, e resta classificabile? |
| **5** | evidenza per ogni variante | terminazione forzata *e* rifiuto devono lasciare entrambi una prova |

La riga 2 è stata riscritta. Diceva «il processo viene fermato al tetto», che
descrive Linux e non Windows: il primo uccide, il secondo nega l'allocazione e
lascia il processo vivo. Il tetto può inoltre essere superato per un istante
prima che il sistema reagisca, quindi la proprietà è sull'**imposizione** del
tetto, non su un massimo istantaneo.

**Esito.** Nascita vincolata e contenimento sono dimostrati su entrambe le
piattaforme. **L'attribuzione no**: su Linux regge solo sotto le condizioni
verificabili del preflight, e su Windows non regge affatto — motivo per cui
quella piattaforma è uscita dal perimetro. Le misure e l'elenco di ciò che
**non** è stato dimostrato stanno in
[`prototipi-isolamento.md`](prototipi-isolamento.md).

Restava la regola in caso di fallimento, e va lasciata scritta perché vale per
i prototipi futuri (macOS, e qualunque piattaforma nuova):

- **nessun ripiego silenzioso**. Non si sostituisce un meccanismo con uno più
  debole, non si accetta un'attribuzione dedotta, non si dichiara supportata
  una piattaforma che contiene ma non attribuisce;
- **il disegno torna in review**, con la proprietà mancante dichiarata e le
  conseguenze sulle garanzie §1 rifatte.

Un prototipo che dimostra quattro voci su cinque non è un successo parziale: è
un risultato che cambia ciò che possiamo promettere.

### 11.2 Le PR, dopo i prototipi

Piccole e revisionabili. Ognuna dichiara se cambia semantica.

| PR | contenuto | cambia semantica | verificabile con |
|---|---|---|---|
| **PR-0** | **hardening dei custom metadata IPC** (`F4-22`): validazione del campo 4 del footer, presenza obbligatoria di chiave e valore, i tre tetti come **costanti interne non ampliabili**, rifiuto dei duplicati, UTF-8 verificato — nell'helper condiviso, quindi anche per campi, schema e messaggi | **sì**, fail-closed: cambia quali input IPC sono accettati | i **dodici casi strutturali** della §7-quater; la suite esistente verde; e i tre limiti **registrati** in [`errori-e-limiti.md`](errori-e-limiti.md) con regola, perimetro, pericolo e condizione di rientro |
| **PR-1** | varianti nuove di `PlenoraError`: `Protocol`, `Timeout`, `Conflict`, `InvalidConfiguration`, **`IsolationUnavailable`** (§9), **pressione di memoria non attribuita** (§10.0-bis), con il tipo `EvidenzaDiLimite` che la accompagna — i cinque segnali della §10.0-bis, `Oa` in forma limitata dal tipo e per distanza dal dominio, e tetto/picco/`max` tenuti separati come diagnostica perché non fondano attribuzione. La **matrice** che li legge resta a `PR-3`. Il tipo reso da `risolvi_commit` **no**: appartiene a `PR-10`, che è la PR che introduce quella funzione | **sì** (varianti e tipo pubblici) | mapping variante→categoria esaustivo, exit code invariati per le esistenti e proiezione tipizzata che non compila se una categoria resta senza decisione |
| **PR-2** | `max_domain_memory_bytes` nel formato del piano e nei contratti pubblici | **sì** (formato) | rifiuto della combinazione incoerente col budget governato |
| **PR-3** | tipi dell'esito e della **classificazione**: `EsitoWorker`, la matrice §10 come `match` esaustivo, la precedenza §10.3. **Consuma** `EvidenzaDiLimite`, che `PR-1` ha già introdotto perché è superficie pubblica e va decisa una volta sola; qui vive la regola per cui solo `G` autorizza l'attribuzione al dominio | no | test di tabella sulla matrice e sulle corse |
| **PR-4** | protocollo: codifica, tetti, versione, fail-closed. Solo serializzazione | no | round-trip e rifiuto di ogni forma malformata |
| **PR-5** | handshake: identità artefatto, resolver, insieme content-addressed, backend dinamici, **`commit_token`**; il token nei **custom metadata del footer IPC**; e `CommitToken` come tipo chiuso | **sì** (formato dell'artefatto) | test di disaccordo su ciascun campo; i **quattro casi del token** della §7-quater — assente, canonico, non canonico, oracoli del footer |
| **PR-6** | verificatore **in streaming** con tetti dimostrabili, ancora in-process | no | prova che la memoria trattenuta non cresce col numero di righe né di batch |
| **PR-7** | dominio di isolamento su Linux, promosso da PT-Linux. Strada dello spawner, `memory.oom.group=1` obbligatorio, sigillo `cgroup.max.depth=0`, **separazione dei privilegi (`F4-15`) col provider UID/GID**, verifica in `PreparaIsolamento` con esito `IsolationUnavailable`, nessun `unsafe` | no | le sei riletture del preflight, e il worker che non riesce a riscrivere nessuna delle proprietà né a lasciare il dominio |
| **PR-8** | supervisore: lifecycle, timeout, cancellazione, cleanup. Worker fittizio | no | matrice degli esiti su un worker che simula ogni riga |
| **PR-9** | worker reale come modalità dell'eseguibile | no | esecuzione end-to-end sotto limite |
| **PR-10** | sequenza di verifica da 1 a 9 — 8-bis compreso — publish no-clobber con rilevazione del residuo, e **`risolvi_commit`** con l'enum delle osservazioni | **sì** (superficie pubblica) | `GA-1`, `GA-3` e `GA-4` su ogni riga della matrice; e le cinque osservazioni su destinazioni costruite a mano |
| ~~`PR-11`~~ | dominio di isolamento su Windows | — | **rimossa dal perimetro della fase 4**: vedi sotto |
| **PR-12** | **attivazione**: il profilo isolato diventa selezionabile **su Linux** | **sì** | l'intera matrice, su Linux; su Windows e macOS il profilo è rifiutato in validazione, non ignorato |

### `PR-0` non è lavoro di isolamento, ed è la ragione per cui viene prima

È l'unica PR di questo elenco che sarebbe necessaria **anche se la fase 4 non
esistesse**. Il campo 4 del footer è invalidato da sempre; nessuno ci passava,
quindi nessuno lo vedeva. Adottare il footer per il token non ha creato il
difetto: l'ha scoperto.

Tenerla dentro `PR-5` — dove il token viene scritto — significherebbe due
cose sbagliate insieme: una correzione di sicurezza che aspetta l'isolamento
per essere fatta, e una PR che rinforza un confine *mentre* comincia a
dipenderne. Separata, può essere revisionata, misurata e rilasciata per conto
suo.

**Cambia semantica**, e in senso fail-closed: input IPC che oggi passano
domani vengono rifiutati. Va quindi registrata in
[`errori-e-limiti.md`](errori-e-limiti.md) come le altre restrizioni del
confine ostile, con regola, perimetro, pericolo e condizione di rientro.

Sei PR cambiano semantica — `PR-0`, `PR-1`, `PR-2`, `PR-5`, `PR-10` e
`PR-12` — e nessuna delle sei è nascosta in mezzo alle altre. `PR-10` lo è diventata
portando `risolvi_commit`: è una funzione pubblica nuova, non un dettaglio
della pubblicazione. `PR-5` lo è diventata scrivendo
il token nell'artefatto: la stesura precedente la marcava «nessun cambiamento
semantico» quando cambia i byte del file prodotto.

### Perché Windows esce dal perimetro

La prima stesura offriva due strade per l'`unsafe` che i Job Object
richiedono: una **deroga** dichiarata in
[`errori-e-limiti.md`](errori-e-limiti.md), oppure una dipendenza vettata.

La prima non è una strada disponibile. `AGENTS.md` proibisce `unsafe` nel
workspace come **regola permanente e non opzionale**: derogarvi non è una PR
della fase 4, è un cambiamento di governance, e spetta a chi quella regola
l'ha scritta. Metterla in un elenco di PR la faceva sembrare una decisione
tecnica fra le altre.

La seconda è stata **verificata invece che immaginata**, e oggi non esiste.
`win32job` — l'unico involucro sicuro dei Job Object in circolazione — non
espone `JOB_OBJECT_LIMIT_JOB_MEMORY`, cioè **il tetto stesso**: l'unico limite
di memoria che offre è quello sul working set, che è un'altra grandezza. Non
espone nemmeno la completion port, i limiti di notifica o le informazioni
sulle violazioni, cioè la fonte dell'evidenza. Il campo della struttura estesa
è privato al crate, quindi non si aggira dall'esterno.

Finché non esiste una dipendenza vettata che copra **tetto ed evidenza**, il
profilo isolato su Windows è **non supportato**, accanto a macOS. Non è un
ripiego silenzioso: è ciò che le regole del progetto impongono quando l'unica
implementazione richiederebbe di violarle. Le tre vie d'uscita, tutte fuori
dalla fase 4:

| via | chi decide |
|---|---|
| estendere a monte un involucro sicuro esistente, e vettarlo | lavoro esterno, poi una PR di dipendenza |
| adottare una dipendenza nuova che copra tetto ed evidenza | valutazione secondo `AGENTS.md`, motivazione documentata e pin esatto |
| cambiare la regola sull'`unsafe` | **il maintainer**, non questa fase |

Le restanti PR costruiscono senza sostituire: il percorso in-process resta
attivo, e ognuna può essere fermata senza lasciare il sistema a metà.

Il criterio di uscita della fase 4 (`F4-5`) si verifica su PR-12: nessun
percorso noto produce un'allocazione critica prima dell'autorizzazione, oppure
il piano viene rifiutato esplicitamente.
