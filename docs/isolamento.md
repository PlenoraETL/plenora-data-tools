# Esecuzione isolata — progetto tecnico della fase 4

Progetto, non implementazione. Alla data di questo documento **non esiste
codice** di worker, supervisore, protocollo o limiti di processo: qui c'è che
cosa dovranno fare e perché.

La scelta architetturale è presa e non si riapre: la correttezza viene
dall'**isolamento**, non dalla previsione. Il ragionamento è in
[`stato-e-roadmap.md`](stato-e-roadmap.md); i requisiti vincolanti sono `F4-1`
… `F4-6` nello stesso documento.

Baseline strutturale: commit `922aea3`, tag `baseline-pre-fase-4`.

I due prototipi bloccanti della §11.1 sono stati **eseguiti**: le misure e le
loro conseguenze stanno in [`prototipi-isolamento.md`](prototipi-isolamento.md).
Hanno dimostrato tutte e tre le proprietà richieste su entrambe le
piattaforme, e hanno smentito sette cose che questo documento dava per
scontate. Dove un paragrafo qui sotto riporta un numero, quel numero viene da
lì.

---

## 1. Garanzie osservabili

Ciascuna è una proprietà che un consumatore può verificare dall'esterno, senza
leggere il nostro codice.

| | garanzia |
|---|---|
| **GA-1** | Dopo OOM, crash, panic, timeout, cancellazione o risultato invalido, **nessun output finale è visibile**. Il percorso di destinazione non esiste, oppure contiene ciò che c'era prima. |
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
| **NG-10** | **Contenere non significa terminare.** Linux uccide, Windows nega l'allocazione e lascia il processo vivo. Non garantiamo un modo unico di fermarsi: garantiamo che la memoria resti sotto il tetto e che l'evento sia attribuibile. |
| **NG-11** | Il tetto del dominio **non protegge dalle richieste**, solo dagli addebiti. Su Linux una richiesta da 64 TiB non toccata è stata accettata senza generare alcun evento. Un worker non può difendersi interrogando l'allocatore. |
| **NG-9** | `GA-5` è una proprietà del **protocollo**, non del filesystem. Senza un contenimento del filesystem il worker *potrebbe* scrivere ovunque abbia permessi: semplicemente non sa dove sia la destinazione, perché nessuno gliela dice. Un contenimento vero — handle già aperti passati al posto dei percorsi, oppure una sandbox del filesystem — è un rafforzamento futuro, non una garanzia odierna. |

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
descrittori e i contatori. Il suo consumo è funzione del numero di domini
vivi, non della dimensione del dato — ed è ciò che rende non circolare il fatto
che osservi entrambi.

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

Il coordinatore ha il proprio tetto, separato e molto più piccolo, dichiarato
dal componente e non dal piano. È l'unica quantità che si somma alle altre.

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
| piani v5 che non hanno il campo | restano **validi e invariati**. Il campo è facoltativo, e la sua assenza significa una cosa sola: il profilo isolato non è selezionabile per quel piano. Nessuna migrazione forzata |
| obbligatorietà | il campo è obbligatorio **solo** nel profilo isolato. Chiedere l'isolamento senza dichiarare il tetto è incoerente, e va respinto in validazione |
| forma canonica e `plan_hash` | il campo entra nella forma canonica **solo quando è presente**. Un piano che non lo dichiara ha la stessa forma canonica di prima, quindi lo **stesso** `plan_hash`: nessun identificativo già pubblicato cambia. Un piano che lo dichiara è un piano diverso e ha un identificativo diverso, che è il comportamento giusto |
| versionamento | nessun incremento della versione del piano: è un campo additivo e facoltativo. La cosa nuova è il **profilo**, non il formato |
| ratifica in `plenora-contracts` | la ratifica **mirata di questo campo** precede la `PR-2`. È separata dall'adozione generale della nuova linea normativa, che resta un blocker a sé ([`stato-e-roadmap.md`](stato-e-roadmap.md)) |

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
| `Saluto` | primo | versione del protocollo, identità dell'artefatto, identità del resolver, limiti del protocollo |
| `Incarico` | dopo l'accordo | piano validato, descrizione degli input, percorso dell'artefatto temporaneo |
| `Annulla` | in qualunque momento | motivo |

Direzione worker → supervisore:

| messaggio | quando | contenuto |
|---|---|---|
| `Risposta` | primo | versione, identità dell'artefatto, identità del resolver, capability del backend |
| `Progresso` | facoltativo, ripetibile | contatori deterministici (righe, batch); **mai** dati |
| `Esito` | ultimo | successo con sigillo, oppure errore tipizzato con i quattro assi |

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
| 9 | **publish atomico** | il rename fallisce, o la destinazione esiste già |

Solo dopo il passo 9 l'output è visibile. I passi 1-8 non producono alcun
effetto osservabile all'esterno.

Il passo 6 è il punto in cui `F4-4` diventa concreto: la lettura usa **lo
stesso resolver** che il worker ha usato per scrivere, perché l'handshake l'ha
imposto. Senza quell'accordo, questo passo confronterebbe due
interpretazioni invece di due risultati.

Il passo 9 riusa la pubblicazione atomica esistente
([`errori-e-limiti.md`](errori-e-limiti.md)), inclusa la distinzione fra
pubblicato e pubblicato-con-durabilità-non-confermata.

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
| evidenza di attribuzione | `memory.events.local` | notifica del job sulla completion port | — |
| terminazione forzata | uccisione dell'intero cgroup | terminazione del job | — |
| stato | **prototipato**, tutte e tre le proprietà dimostrate | **prototipato**, tutte e tre le proprietà dimostrate | **non supportato** (`F4-6`, `NG-6`) |

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

**`memory.oom.group` è obbligatorio, non consigliato.** Senza, il prototipo ha
osservato un figlio ucciso per il limite e il processo capofila **vivo, con
uscita 0**: un successo apparente sopra un guasto di risorse. Con
`memory.oom.group=1` il dominio muore intero e nessuno può dichiarare un
successo che non c'è.

**Windows — due strade, e la finestra è di una sola.** La prima crea il
processo con `CREATE_SUSPENDED`, lo associa al Job Object e poi lo riprende. Il
thread primario non ha eseguito un'istruzione, quindi il codice del worker non
ha allocato nulla. **Ma la mappatura dell'immagine e le allocazioni del loader
precedono l'associazione.**

Il prototipo l'ha misurato: **376 832 byte** impegnati prima
dell'associazione, e — sorpresa utile — il job li **contabilizza** al momento
in cui il processo vi entra. Non è quindi memoria invisibile al tetto. Le
parole «piccolo e costante» che stavano qui sono state tolte: erano una stima
scritta come se fosse un fatto, e una sola misura non autorizza a chiamare
costante alcunché.

La seconda strada — quella dello **spawner**, un processo intermedio che entra
nel job e poi genera il worker, che ne eredita l'appartenenza — porta quel
residuo a zero per il worker, al prezzo di 651 264 byte di processo intermedio
che restano a carico del dominio. Il residuo non sparisce: passa da *non
coperto* a *coperto e consumato*, ed è preferibile perché governabile.

**Si sceglie la seconda**, e per tre ragioni che il prototipo ha reso
concrete: non lascia finestra, è la stessa forma della strada scelta su Linux —
una simmetria che vale, perché due meccanismi che si assomigliano si sbagliano
meno di due che divergono — e taglia via la parte peggiore della prima, cioè la
ripresa del processo sospeso. `std` non espone l'handle del thread primario:
riprenderlo obbliga a enumerare tutti i thread del sistema per ritrovare i
propri.

**Perché `memory.events.local` e non `memory.events`.** Il secondo aggrega i
discendenti; il primo riguarda il cgroup che abbiamo creato noi. Attribuire a
un worker un evento generato altrove sarebbe una classificazione inventata.

**Perché la notifica del job e non l'exit code.** Su Windows un processo
terminato dal job non ha un codice d'uscita che lo dica in modo affidabile. La
notifica è la prova; il codice d'uscita non lo è. Il prototipo ha rafforzato la
ragione: un figlio del worker ha esaurito il tetto mentre il processo capofila
usciva con **0**.

**E nemmeno i contatori.** `PeakJobMemoryUsed` ha riportato circa 64 TiB per
allocazioni che il sistema aveva **rifiutato**: misura gli addebiti tentati,
non la memoria posseduta. Serve alla diagnostica, non come evidenza.

**I job annidati non sono una via di fuga.** Il prototipo ha fatto creare al
worker un job proprio con un tetto quattro volte più alto e ve l'ha assegnato:
l'operazione riesce, e il tetto esterno **continua a valere**. Il contenimento
regge anche contro un worker che provi a uscirne — il che è un margine, non
una promessa, perché il modello di minaccia resta il guasto (`NG-7`).

**macOS.** Nessun meccanismo è stato prototipato. Finché un prototipo non
dimostra **contenimento** e **attribuzione** — entrambi, non uno — il profilo
isolato non è supportato e l'esecuzione resta in-process con le garanzie
attuali, che sono minori e dichiarate.

---

## 10. Matrice degli esiti

`ResourceLimit` compare **solo** dove c'è evidenza attribuibile (`F4-2`).

| # | esito | evidenza | categoria | output visibile | artefatto |
|---|---|---|---|---|---|
| 1 | successo verificato | `Esito` di successo + verifica 1-8 superata | — (successo) | **sì**, pubblicato | rimosso |
| 2 | errore tipizzato del worker | `Esito` con i quattro assi | quella dichiarata dal worker | no | rimosso |
| 3 | panic nel worker | `Esito` di panic, forma del payload senza contenuto | `Internal` | no | rimosso |
| 4 | crash | il processo muore senza `Esito`, nessuna evidenza di limite | `Internal` | no | rimosso |
| 5 | OOM attribuito | evidenza del dominio (`memory.events.local` / notifica del job) | **`ResourceLimit`** | no | rimosso |
| 6 | terminazione ambigua | il processo muore, evidenza assente o non attribuibile | `Internal` — **mai** dedotto come OOM | no | rimosso |
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

**La riga 5 non presuppone la morte del worker.** Su Windows il limite si
manifesta come allocazione negata e il processo resta vivo (`NG-10`): può
quindi uscire con un errore tipizzato *proprio*, mentre il dominio ha
registrato la notifica di limite. Le due righe 2 e 5 sarebbero entrambe
candidate, e la precedenza della §10.3 decide: **l'evidenza del dominio batte
l'esito dichiarato**, quindi l'esito è `ResourceLimit`.

È la scelta giusta perché l'errore che il worker riesce a riportare in quelle
condizioni è quasi sempre la conseguenza, non la causa: dire al chiamante
«fallita l'allocazione di un buffer» invece di «hai superato il tetto» lo
manderebbe a cercare un difetto dove c'è un dimensionamento.

### 10.1 Tassonomia: che cosa manca davvero

`ErrorCategory` ha già `Protocol`, `Timeout`, `Conflict` e
`InvalidConfiguration`. **Nessuna variante di `PlenoraError` le produce oggi**:
esistono come categorie dichiarabili, non come errori costruibili.

Le tre righe che ne hanno bisogno:

| esito | categoria | perché non le esistenti |
|---|---|---|
| protocollo incompatibile | `Protocol` | è precisamente «violazione del protocollo». `Internal` direbbe «difetto nostro», e lo è solo a volte: due binari diversi in esecuzione sono una condizione di dispiegamento |
| resolver incompatibile | `InvalidConfiguration` | **non `InvalidPlan`**: il piano può essere perfettamente valido, e lo sarebbe di nuovo con un ambiente coerente. È il componente a essere configurato male, non il piano a essere sbagliato |
| timeout | `Timeout` | oggi non c'è modo di costruirlo |

Servono quindi **varianti nuove** di `PlenoraError`. È una **modifica
semantica di un tipo pubblico**, non un dettaglio: va in una PR propria, con
il proprio impatto sulla superficie e sugli exit code, prima di qualunque
codice che le usi.

Anche `Conflict` va reso costruibile: la riga 15 (fallimento del publish) lo
usa quando la destinazione esiste già, ed è diverso da un errore di I/O.

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

Il **punto di linearizzazione precede il rename**, ed è così definito:

1. tutti gli eventi — cancellazione, timeout, uscita del worker, evidenza del
   dominio — confluiscono in **una** coda, consumata da **un** solo esecutore.
   Non ci sono due strade per diventare un esito;
2. superata la verifica e prima di tentare la pubblicazione, l'esecutore
   **chiude gli ingressi** e congela l'esito provvisorio. Da quell'istante
   nessun evento successivo può cambiare la classificazione: gli eventi che
   arrivano dopo vengono registrati per la diagnostica e non votano;
3. l'unico ingresso rimasto è **l'esito del rename stesso**. Riesce: l'esito è
   successo, e la riga 1 della precedenza è soddisfatta da un fatto, non da una
   regola. Fallisce: si torna all'esito congelato al punto 2, oppure alla riga
   15 se il fallimento è del publish.

È questo che rende la riga 1 vera invece che sperata. «Publish completato
vince» non è una priorità che si applica *dopo*: è la constatazione che dopo la
chiusura degli ingressi l'unico fatto ancora capace di cambiare l'esito è
quello che stiamo per compiere noi.

Un evento che arrivi **fra** il punto 2 e il ritorno del rename — un OOM del
verificatore, una cancellazione tardiva — non annulla una pubblicazione
avvenuta. Viene riportato come avvertenza, con lo stesso canale
machine-readable della §10.2.

Chiuso l'insieme, l'ordine è **totale** e va dal fatto più esterno al meno
verificabile:

| # | evento | vince perché |
|---|---|---|
| 1 | **publish completato** | è un fatto osservabile fuori dal sistema: se l'output è visibile, l'operazione è riuscita, e nessun evento successivo può renderla non riuscita |
| 2 | **OOM attribuito** | ha evidenza specifica del dominio. Un processo con quella prova è morto per il limite, anche se stavamo cancellando |
| 3 | **timeout scaduto** | è il nostro orologio, misurato |
| 4 | **cancellazione richiesta** | è la nostra decisione, e precede l'auto-dichiarazione del worker |
| 5 | **esito dichiarato dal worker** | è un'affermazione di un processo che potrebbe essere in difficoltà |
| 6 | **terminazione ambigua** | l'ultimo, e per costruzione mai `ResourceLimit` |

Il punto 4 sopra il 5 merita una parola: se abbiamo cancellato **e** il worker
riporta successo ma non abbiamo ancora pubblicato, l'esito è `Cancelled`. Un
successo che non è diventato visibile non è un successo per chi ha chiesto di
annullare.

Il punto 2 sopra il 4 merita l'altra: se abbiamo cancellato e c'è prova di OOM,
l'esito è `ResourceLimit`. La prova batte l'intenzione — altrimenti una
cancellazione tempestiva maschererebbe un difetto di dimensionamento.

Le righe 5 e 6 sono la stessa terminazione vista con e senza prova, e la
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
| **2** | contenimento | la memoria **posseduta** resta sotto il tetto — *comunque* il sistema ci arrivi |
| **3** | attribuzione | l'evento è leggibile e riferito al nostro dominio, non dedotto |
| **4** | comportamento dopo il rifiuto | se l'allocazione è **negata** invece che fatale: che cosa fa il worker, e resta classificabile? |
| **5** | evidenza per ogni variante | terminazione forzata *e* rifiuto devono lasciare entrambi una prova |

La riga 2 è stata riscritta. Diceva «il processo viene fermato al tetto», che
descrive Linux e non Windows: il primo uccide, il secondo nega l'allocazione e
lascia il processo vivo. Il tetto può inoltre essere superato per un istante
prima che il sistema reagisca, quindi la proprietà è sulla memoria posseduta,
non sull'istante.

**Esito.** Entrambi i prototipi hanno dimostrato tutte e cinque le voci. Le
misure, le sette conseguenze sul disegno e — soprattutto — l'elenco di ciò che
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
| **PR-1** | varianti nuove di `PlenoraError` per `Protocol`, `Timeout`, `Conflict`, `InvalidConfiguration` | **sì** (tipo pubblico) | mapping variante→categoria esaustivo, exit code invariati per le esistenti |
| **PR-2** | `max_domain_memory_bytes` nel formato del piano e nei contratti pubblici | **sì** (formato) | rifiuto della combinazione incoerente col budget governato |
| **PR-3** | tipi dell'esito e della classificazione: `EsitoWorker`, `EvidenzaDiLimite`, la matrice §10 come `match` esaustivo, la precedenza §10.3 | no | test di tabella sulla matrice e sulle corse |
| **PR-4** | protocollo: codifica, tetti, versione, fail-closed. Solo serializzazione | no | round-trip e rifiuto di ogni forma malformata |
| **PR-5** | handshake: identità artefatto, resolver, insieme content-addressed, backend dinamici | no | test di disaccordo su ciascun campo |
| **PR-6** | verificatore **in streaming** con tetti dimostrabili, ancora in-process | no | prova che la memoria trattenuta non cresce col numero di righe né di batch |
| **PR-7** | dominio di isolamento su Linux, promosso da PT-Linux. Strada dello spawner, `memory.oom.group=1` obbligatorio, nessun `unsafe` | no | contenimento e attribuzione, come il prototipo |
| **PR-8** | supervisore: lifecycle, timeout, cancellazione, cleanup. Worker fittizio | no | matrice degli esiti su un worker che simula ogni riga |
| **PR-9** | worker reale come modalità dell'eseguibile | no | esecuzione end-to-end sotto limite |
| **PR-10** | sequenza di verifica 1-9 e publish atomico dal supervisore | no | `GA-1`, `GA-3` e `GA-4` su ogni riga della matrice |
| **PR-10-bis** | **deroga per l'`unsafe` di Windows**, oppure adozione di una dipendenza vettata che assorba la FFI dei Job Object. Nessuna riga di `PR-11` prima | **sì** (regola permanente) | la deroga dichiara regola, perimetro, pericolo e condizione di rientro |
| **PR-11** | dominio di isolamento su Windows, promosso da PT-Windows | no | come PR-7 |
| **PR-12** | **attivazione**: il profilo isolato diventa selezionabile | **sì** | l'intera matrice, su entrambe le piattaforme |

Quattro PR cambiano semantica — `PR-1`, `PR-2`, `PR-10-bis` e `PR-12` — e
nessuna delle quattro è nascosta in mezzo alle altre. `PR-10-bis` è l'unica che
non tocca il codice: tocca una **regola permanente** del progetto, ed è per
questo che non può essere una riga dentro un'altra PR. Le restanti costruiscono senza sostituire: il percorso
in-process resta attivo, e ognuna può essere fermata senza lasciare il sistema
a metà.

Il criterio di uscita della fase 4 (`F4-5`) si verifica su PR-12: nessun
percorso noto produce un'allocazione critica prima dell'autorizzazione, oppure
il piano viene rifiutato esplicitamente.
