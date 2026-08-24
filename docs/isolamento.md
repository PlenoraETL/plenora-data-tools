# Esecuzione isolata — progetto tecnico della fase 4

Progetto, non implementazione. Alla data di questo documento **non esiste
codice** di worker, supervisore, protocollo o limiti di processo: qui c'è che
cosa dovranno fare e perché.

La scelta architetturale è presa e non si riapre: la correttezza viene
dall'**isolamento**, non dalla previsione. Il ragionamento è in
[`stato-e-roadmap.md`](stato-e-roadmap.md); i requisiti vincolanti sono `F4-1`
… `F4-6` nello stesso documento.

Baseline strutturale: commit `922aea3`, tag `baseline-pre-fase-4`.

---

## 1. Garanzie osservabili

Ciascuna è una proprietà che un consumatore può verificare dall'esterno, senza
leggere il nostro codice.

| | garanzia |
|---|---|
| **GA-1** | Dopo OOM, crash, panic, timeout, cancellazione o risultato invalido, **nessun output finale è visibile**. Il percorso di destinazione non esiste, oppure contiene ciò che c'era prima. |
| **GA-2** | Un esito classificato `ResourceLimit` porta **evidenza attribuibile** del dominio di isolamento. Senza prova specifica la classificazione è `Internal`. |
| **GA-3** | L'output pubblicato ha uno schema **verificato** contro il contratto atteso, letto dall'autorità unica `plenora_core::contract`. |
| **GA-4** | La pubblicazione è **atomica**: il file finale c'è per intero o non c'è. Nessuno stato intermedio è osservabile. |
| **GA-5** | Il worker **non conosce** la destinazione finale e non può scriverla. |
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
risultati diversi**. Vanno quindi dichiarati la versione del database PROJ, i
percorsi di ricerca in uso e un digest delle risorse effettivamente caricate.

**Backend dinamici.** Se il backend è collegato dinamicamente, l'identità
dell'artefatto non lo copre: il digest dell'eseguibile non dice nulla della
libreria che verrà caricata. I backend dinamici vanno quindi identificati
**separatamente** — nome, versione riportata a runtime, percorso risolto — e
la loro identità entra nell'accordo come le altre.

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
| chi applica | il supervisore, prima dello spawn | il supervisore, prima dello spawn | — |
| evidenza di attribuzione | `memory.events.local` | notifica del job sulla completion port | — |
| terminazione forzata | uccisione dell'intero cgroup | terminazione del job | — |
| stato | da prototipare | da prototipare | **non supportato** (`F4-6`, `NG-6`) |

**Perché `memory.events.local` e non `memory.events`.** Il secondo aggrega i
discendenti; il primo riguarda il cgroup che abbiamo creato noi. Attribuire a
un worker un evento generato altrove sarebbe una classificazione inventata.

**Perché la notifica del job e non l'exit code.** Su Windows un processo
terminato dal job non ha un codice d'uscita che lo dica in modo affidabile. La
notifica è la prova; il codice d'uscita non lo è.

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
| 9 | protocollo incompatibile | versione non riconosciuta nell'handshake | `Internal` | no | nessuno creato |
| 10 | resolver incompatibile | disaccordo su identità, versione, capability o ambiente | `InvalidPlan` | no | nessuno creato |
| 11 | output assente | verifica passo 3 | `Internal` | no | — |
| 12 | output troncato | verifica passo 4 (sigillo) o 5 (framing) | `Internal` | no | rimosso |
| 13 | output semanticamente invalido | verifica passo 7 (contratto) | `Schema` | no | rimosso |
| 14 | fallimento della verifica | qualunque passo 1-8 non coperto sopra | secondo il passo | no | rimosso |
| 15 | fallimento del publish | verifica passata, passo 9 fallito | `Io` o `Conflict` | no | rimosso |
| 16 | fallimento del cleanup | publish riuscito, rimozione fallita | successo **con avvertenza** | **sì** | **residuo** |

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

Piccole e revisionabili. Ognuna dichiara se cambia semantica.

| PR | contenuto | cambia semantica | verificabile con |
|---|---|---|---|
| **PR-1** | tipi dell'esito e della classificazione, senza processi: `EsitoWorker`, `EvidenzaDiLimite`, la matrice §10 come `match` esaustivo | no | test di tabella sulla matrice |
| **PR-2** | protocollo: codifica, tetti, versione, fail-closed. Nessun processo, solo serializzazione | no | round-trip e rifiuto di ogni forma malformata |
| **PR-3** | handshake: identità artefatto, resolver, ambiente, backend dinamici. Ancora in-process | no | test di disaccordo su ciascun campo |
| **PR-4** | dominio di isolamento su **Linux**: creazione, limite, attribuzione. Nessun worker ancora | no | prova che il limite contiene e che l'evidenza è leggibile |
| **PR-5** | supervisore: lifecycle, timeout, cancellazione, cleanup. Worker fittizio | no | matrice degli esiti su un worker che simula ogni riga |
| **PR-6** | worker reale come modalità dell'eseguibile | no | esecuzione end-to-end sotto limite |
| **PR-7** | sequenza di verifica 1-8 | no | rifiuto di ciascun difetto iniettato |
| **PR-8** | publish atomico dal supervisore | no | GA-1 e GA-4 su ogni riga della matrice |
| **PR-9** | dominio di isolamento su **Windows** | no | come PR-4 |
| **PR-10** | **attivazione**: il profilo isolato diventa selezionabile | **sì** | l'intera matrice, su entrambe le piattaforme |

Solo **PR-10** cambia semantica. Tutte le precedenti costruiscono senza
sostituire: il percorso in-process resta quello attivo, e ciò significa che
ognuna può essere fermata senza lasciare il sistema a metà.

Il criterio di uscita della fase 4 (`F4-5`) si verifica su PR-10: nessun
percorso noto produce un'allocazione critica prima dell'autorizzazione, oppure
il piano viene rifiutato esplicitamente.
