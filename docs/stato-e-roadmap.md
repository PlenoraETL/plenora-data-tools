# Stato e roadmap

Solo il **lavoro ancora aperto**. Quello che è fatto sta nel codice, nei test
e negli altri documenti; qui c'è ciò che manca, in ordine.

## Dove siamo

Il core è una release candidate credibile: il piano v5 è il formato canonico,
la CLI è l'eseguibile distribuito, le 146 operazioni del catalogo sono
documentate, e la CI verifica Linux e Windows con quindici job.

Non è ancora rilasciabile in produzione. Le ragioni sono qui sotto, in
ordine di precedenza.

---

## 1. Reset documentale — **completato**

La documentazione descrive ora soltanto ciò che il progetto è, i contratti
validi, i limiti aperti e la strada verso la produzione. La storia resta in
Git, non nel worktree: nessun archivio, nessun file conservato «per memoria».

La superficie pubblica è chiusa da un gate che rifiuta ogni Markdown fuori
dalla allowlist, i collegamenti rotti e i riferimenti a documenti eliminati.

## 2. La memoria governata non è un tetto duro

**È il limite più serio del progetto**, ed è la ragione per cui il core non è
in produzione.

Il budget governa la **ritenzione** del risultato, non la sua costruzione:
dove il lease è preso dopo l'allocazione — quasi ovunque — un input che
produce un output molto più grande del budget porta all'esaurimento della
memoria **prima** che l'errore esista. Il fallimento è un OOM, non un errore
diagnosticabile. Il quadro completo è in
[`errori-e-limiti.md`](errori-e-limiti.md).

### La decisione normativa: isolamento, non previsione

Questo documento presentava due strade — preflight per operazione, oppure
reservation preventiva — ed entrambe poggiavano sulla stessa assunzione: che
la correttezza si ottenga **prevedendo** quanta memoria servirà. La decisione
è ora una sola, e l'assunzione è respinta.

**La garanzia è l'isolamento.** Il lavoro si esegue in un processo worker con
un limite imposto dal sistema operativo, un supervisore che ne osserva l'esito,
e una pubblicazione atomica verificata. Se il worker sfora, il sistema
operativo lo termina; il supervisore lo constata e produce un errore
diagnosticabile; nulla è stato pubblicato, perché la pubblicazione avviene solo
alla fine e in modo atomico.

Perché questa e non la previsione: **per le operazioni che dipendono dai dati
una stima resta una stima**. Il fattore di espansione di un join, la
cardinalità di un raggruppamento, la dimensione di un buffer geometrico non si
conoscono prima di leggere i dati. Un preflight conservativo abbastanza da
essere sempre corretto rifiuterebbe piani legittimi; uno abbastanza permissivo
da accettarli non garantisce nulla. Fondare la correttezza su una stima
significa avere una garanzia che vale finché la stima è giusta — cioè non una
garanzia.

**La profilazione preventiva resta utile, ma cambia ruolo**: non è il
fondamento della correttezza, è l'ottimizzazione che fa fallire *prima e
meglio*. Un piano che si sa insostenibile può essere rifiutato in validazione
invece che dopo dieci minuti di lavoro. È valore reale, ed è subordinato.

I prerequisiti già in piedi restano validi: il governor ha un permesso atomico
— verifica e prenotazione in una sola operazione — e la contabilità è
linearizzabile. Servono al secondo livello, non al primo.

Il costo dichiarato di questa scelta: **nessun meccanismo è stato ancora
prototipato**, e su macOS il profilo isolato resterà non supportato finché un
prototipo non dimostri copertura *e* attribuzione. È un lavoro più grande di un
preflight, e la ragione per farlo comunque è che il preflight non risolverebbe
il problema.

### Il refactor strutturale è il veicolo, non un lavoro parallelo

Qualunque cosa la fase 4 aggiunga — il confine col worker, il protocollo col
supervisore, l'eventuale profilazione preventiva — ha bisogno di **un posto
solo dove abitare**. Oggi l'engine conosce i tipi di configurazione di ogni
singolo kernel: senza le facciate di famiglia lo stesso lavoro andrebbe
ripetuto per ciascuna delle 146 operazioni.

L'ordine delle fasi va letto così: le prime tre non cambiano comportamento e
servono a costruire il posto, la quarta è questo punto 2 e chiude il blocco.

| fase | contenuto | cambia semantica | stato |
|---|---|---|---|
| 0 | baseline e oracoli | no | **chiusa** |
| 1 | alleggerire la CLI, scomporre l'executor | no | **chiusa** |
| 2 | autorità unica per Arrow/CRS, errori, limiti | no | **chiusa** |
| 3 | `OperationId` esaustivo, facciate di famiglia | no | **chiusa** |
| **4** | **il contratto di memoria — questo punto 2** | **sì** | due cicli di prototipi come **evidenza esplorativa** ([`prototipi-isolamento.md`](prototipi-isolamento.md)); progetto **non approvato** per l'implementazione ([`isolamento.md`](isolamento.md)) |
| 5 | il legacy ridotto a un confine di migrazione | sì | |
| 6 | superficie pubblica e commenti | no | |

**Criteri di uscita delle fasi 5 e 6**, perché finora mancavano:

| fase | è chiusa quando |
|---|---|
| 5 | esiste **un solo executor** per tutti i piani semanticamente traducibili; i piani non traducibili sono in un modulo isolato con la matrice di ciò che li rende tali; la rimozione è pianificata per una major dichiarata |
| 6 | nessun modulo fuori dall'API è pubblico; i `lib.rs` espongono facciate ristrette; nessun commento di produzione dichiara uno stato di avanzamento («fase», «in corso», «milestone»), mentre restano le condizioni di rientro e le ragioni `D*`/`R*`; un gate lo tiene, con la distinzione fra cronologia e hazard scritta nella sua doc |

### Baseline pre-fase-4 — freeze strutturale

**Le modifiche strutturali sono ferme.** Lo stato registrato è:

| | |
|---|---|
| commit | `922aea30a611727daee8aaf2c5bd924b49b71207` |
| tag | `baseline-pre-fase-4` |
| suite | 1511 test, **con le feature predefinite** |
| gate | `fmt`, `clippy`, R6, i cinque gate Python nel container di riferimento |
| toolchain | `rust:1.98` nel container di riferimento |
| CI | **non verde a quella revisione** — vedi sotto |

**Il tag è una baseline strutturale, non una baseline con CI verde.** La
distinzione non è formale: ciò che era stato verificato è un giro nel
container di riferimento **a feature predefinite**, e da lì è stato scritto
«tutti verdi» come se coprisse anche il resto. Non lo copriva.

Il commit `9966508`, che il tag contiene, ha introdotto l'oracolo della
superficie CLI come istantanea unica. L'istantanea fissa `backends: []`, che è
vero con le feature predefinite e falso con `full-backends`, dove la CLI
dichiara `["geos", "proj"]`. Il job `test full-backends (linux)` era quindi
rosso da `9966508` in avanti, baseline compresa, e nessun giro locale a feature
predefinite poteva accorgersene.

Il difetto è chiuso: l'oracolo è ora un **modello** con un marcatore per il
valore dei backend, materializzato da costanti esaustive per combinazione di
feature, e il confronto resta byte per byte. Il tag **non è stato spostato**:
una baseline che si muove per nascondere un difetto smette di essere una
baseline.

**Profili di build supportati:** `default` e `full-backends`, come dichiara
[`release.md`](release.md). Le combinazioni parziali — solo `geos-backend`,
solo `proj-backend` — restano compilabili e l'oracolo ha aspettative
esplicite anche per loro, così una build parziale non produce un verde
falso; ma non sono coperte dal CI e non sono profili supportati.

Da qui ogni passo sulla memoria appartiene alla fase 4 e non può più
nascondersi dentro un refactor strutturale: se un commit tocca allocazioni,
lo dichiara.

**Che cosa hanno prodotto le fasi 0-3.**

| | prima | dopo |
|---|---|---|
| `main.rs` | 5116 righe | **2835** in nove moduli |
| `executor.rs` | 5481 righe | **975** in dodici moduli |
| identità delle operazioni | stringhe libere | `OperationId`, 146 varianti in bijezione verificata |
| dispatch dei kernel | un enum con 15 varianti di due famiglie | due facciate, ciascuna esaustiva sulla propria |
| autorità Arrow↔contratto | divisa fra CLI ed executor | unica, in `plenora-core::contract` |
| limiti tabellari | adattatore con due default nascosti | mappatura dichiarata fra due autorità |

**Cinque oracoli** sorvegliano che nulla di osservabile cambi:

| oracolo | che cosa fissa |
|---|---|
| `catalog_snapshot.snap` | i 146 descrittori, campo per campo |
| `oracoli_identita.snap` | `plan_hash` e fingerprint di nove piani, col JSON canonico accanto |
| `oracolo_superficie_cli.snap` | stdout, stderr ed exit code di 29 invocazioni, byte per byte |
| `oracolo_metriche.snap` | righe, batch e spill per nodo, esclusi i tempi |
| `oracolo_round_trip_contratto.rs` | contratto → schema → contratto, e la convergenza in un giro |


### Perché le facciate non espongono `memory_profile()`

Le facciate della fase 3 espongono l'esecuzione e nient'altro. Il metodo
`memory_profile()` sembra preparazione neutra e non lo è: incorpora il modello
predittivo che la decisione qui sopra ha respinto. Metterlo nell'interfaccia
lo avrebbe promosso a contratto — e ogni implementazione futura avrebbe dovuto
onorarlo, anche dopo aver scelto l'isolamento.

I quattro livelli restano distinti, e solo il terzo garantisce la correttezza:

| livello | che cosa dà |
|---|---|
| `OperationId` e facciate | struttura e tipizzazione, indipendenti dalla strategia di memoria |
| profilo di allocazione | prima decisione concreta della fase 4, non un prerequisito |
| **supervisore, isolamento, pubblicazione atomica verificata** | **la garanzia effettiva che il risultato sia valido** |
| profilazione preventiva | ottimizzazione successiva, mai fondamento della correttezza |

### Requisiti della fase 4

Il progetto tecnico che li attua e' in
[`isolamento.md`](isolamento.md): garanzie e non-garanzie, macchine a stati,
protocollo, handshake, verifica, matrici e suddivisione in PR.

Da soddisfare, non ancora da implementare. Nessuno di questi esiste oggi nel
codice: worker, supervisore, protocollo fra i due e limiti di processo sono
**progettati** in [`isolamento.md`](isolamento.md) e **non implementati**.

I prototipi hanno avuto **due cicli**, e sono **evidenza esplorativa**: le
misure stanno in [`prototipi-isolamento.md`](prototipi-isolamento.md).

Nascita vincolata e contenimento sono dimostrati. L'**attribuzione no**:

- su **Linux** regge solo se il dominio è reso foglia dal kernel. Il secondo
  ciclo ha eseguito un'evasione in tre passi — crea un sottogruppo, ci si
  sposta, delega il controller — dopo la quale `memory.events.local` non vede
  più l'uccisione;
- su **Linux** regge solo con la separazione dei privilegi (`F4-15`): con lo
  stesso UID il worker disfa il preflight e esce dal dominio;
- su **Windows** nessuna fonte è sufficiente da sola: la notifica non è
  garantita, la violazione interrogabile vive 25 millisecondi — meno di quanto
  duri la barriera di quiescenza — e l'unico indicatore durevole ha falsi
  positivi misurati, mentre l'assenza di falsi negativi non è dimostrata. La
  piattaforma è **non supportata** (`F4-11`), e le sue misure restano solo
  come motivazione.

Il progetto **non è congelato** e non è approvato per l'implementazione.
Nessuna PR di codice prima dell'approvazione esplicita.

**F4-9 — Il dominio è una foglia dimostrata.** L'attribuzione su Linux vale
solo se il worker non può creare discendenti: `cgroup.max.depth = 0`, worker
senza privilegi sulla gerarchia, e lettura anche degli eventi gerarchici come
rete. Un dominio non sigillato non è un dominio osservabile.

**F4-10 — La pubblicazione è preceduta da una barriera causale.** Quiescenza
del dominio, poi snapshot e drain dell'evidenza, poi classificazione
definitiva, poi la pubblicazione. Un OOM tardivo non può essere degradato ad
avvertenza: il prototipo ha misurato un capofila uscito con 0, un processo
ancora vivo e l'evidenza arrivata duecento millisecondi dopo.

La quiescenza si legge da `cgroup.events`, campo `populated`, che il kernel
garantisce comprendere i discendenti — **non** da `cgroup.procs`, che ha
riportato `0` mentre un processo del dominio era vivo un livello sotto, e
nemmeno da una scansione ricorsiva, che è una corsa.

**F4-12 — Il dominio deve essere uccidibile.** `memory.oom.group = 1` non
uccide i task con `oom_score_adj = -1000`, che si eredita da un chiamante
protetto. Il prototipo ha misurato trecentocinque invocazioni dell'OOM e zero
uccisioni, con il dominio bloccato. Lo spawner normalizza `oom_score_adj`, lo
**rilegge** e fallisce chiuso; il supervisore dispone di `cgroup.kill`, che
non consulta quel valore.

**F4-13 — L'evidenza è una struttura, non un booleano.** Delta locali e
gerarchici insieme — `oom`, `oom_kill`, `oom_group_kill` — con una
classificazione dichiarata per ogni combinazione, compresa quella in cui il
limite è raggiunto e nessuno è uccidibile.

**F4-14 — Ogni proprietà configurata va riletta.** Tetto, group kill, sigillo,
swap, `oom_score_adj` e leggibilità di `cgroup.events`: il preflight le scrive
e le verifica, e il profilo isolato non parte se una sola diverge.

**F4-15 — Il worker non possiede alcuna autorità sul proprio dominio.** Non
«UID distinto», che è un modo e non la proprietà: nessuna fra identità reale,
effettiva e salvata, gruppi supplementari, capability, namespace, descrittori
scrivibili ereditati e permessi sulla gerarchia deve dargli scrittura sul
control plane né la possibilità di lasciare il dominio. Il prototipo ha
misurato un worker con lo stesso UID del supervisore rimettere
`cgroup.max.depth` a 10, `memory.max` a 1 GiB, `memory.oom.group` a 0 e
**uscire dal dominio**.

Il **provider iniziale è uno solo** — identità distinta con gerarchia di
proprietà del control plane — perché è l'unico provato. Helper del control
plane e mount namespace restano strade documentate, ciascuna col proprio
prototipo prima di essere offerta.

La verifica sta in **`PreparaIsolamento`, prima dello spawn**, non nella
validazione del piano: la disponibilità dipende dall'ambiente, non dal piano,
e lo stesso piano è eseguibile su una macchina e non su un'altra. L'esito
negativo è `IsolationUnavailable`.

**F4-16 — Il commit point è dichiarato, e l'ambiguità si risolve con
un'identità d'esecuzione.** La creazione no-clobber della destinazione rende
atomico il file, non la coppia *pubblicato + riportato*: se il coordinatore muore dopo il commit, l'output è
visibile e il chiamante non lo sa. `GA-1` copre i guasti **precedenti** il
commit.

Sigillo e contratto **non bastano** a risolvere: dimostrano che l'artefatto è
valido, non che sia di quella esecuzione — due esecuzioni dello stesso piano su
input diversi producono lo stesso contratto.

Serve un **`commit_token` fornito dal chiamante prima dell'invocazione**, non
scelto da noi e restituito alla fine: un coordinatore morto non restituisce
nulla, e poiché è il processo del chiamante non può nemmeno comunicare la
propria morte. La catena è handshake → metadati dell'artefatto → verifica
prima della pubblicazione.

È distinto dall'`execution_id`, che **resta com'è**: generato dall'engine a
ogni `execute`, con un test che ne verifica la diversità, e senza alcun ruolo
nella risoluzione — il chiamante non lo conosce prima, quindi non può
confrontarlo con nulla.

**L'unicità è una precondizione esterna e non verificabile da noi**, perché il
token deve essere noto al chiamante prima dell'esecuzione. La garanzia è
quindi condizionata: se il token non si ripete fra tentativi la risoluzione
distingue questo output da qualunque altro; se si ripete, non lo distingue e
non ce ne accorgiamo. Per il profilo isolato il token è **obbligatorio** —
un'invocazione che non lo porta è rifiutata in validazione.

La risoluzione è una procedura indipendente,
`risolvi_commit(commit_token, destinazione)`, chiamabile da un processo
successivo, e rende **osservazioni, non decisioni**: `CommittedMatching` —
che richiede sigillo e struttura validi, non solo la chiave —
`OccupiedByOtherAttempt`, `IdentityMissing`, `Absent`,
`InvalidOrUnreadable`. In particolare `Absent` **non** autorizza a riprovare:
un file può mancare anche per perdita di durabilità o rimozione esterna.

**F4-19 — Una sola semantica di concorrenza sulla destinazione: no-clobber.**
La prima pubblicazione vince, la seconda fallisce con `Conflict`. Mai una
sostituzione silenziosa.

Il commit point è la **creazione no-clobber della destinazione**, non un
rename: `persist_noclobber` usa `RENAME_NOREPLACE` dove kernel e filesystem lo
offrono, e altrimenti ripiega su `hard_link` più `unlink` — con l'errore
dell'`unlink` ignorato, quindi con un temporaneo che può restare senza che
nessuno lo dica. Il no-clobber regge in entrambi i rami; il silenzio no. Dopo
il commit si verifica che il temporaneo non ci sia più, e se c'è lo si riporta
con l'avvertenza machine-readable.

**F4-20 — I metadati operativi stanno nel footer IPC, non nello schema.** Il
formato Arrow IPC ha i custom metadata di file, che vivono nel footer:
`FileWriter::write_metadata` li scrive, `FileReader::custom_metadata` li
rilegge, e il percorso d'uscita usa già `FileWriter`. Il `commit_token` va lì.

`Schema`, `DataContract`, `plan_hash` e i confronti restano invariati **per
costruzione**, senza regole da applicare in tre punti e senza il rischio che
un namespace escluso «perché operativo» diventi un giorno semanticamente
importante mentre nessuno lo guarda più. L'artefatto resta un singolo file
Arrow IPC standard.

Una stesura precedente sosteneva che un contenitore IPC non avesse posto per
byte propri fuori dallo schema, e da quella premessa falsa faceva discendere
una proiezione semantica applicata a tre punti. Non serve.

**F4-21 — `CommitToken` è un tipo chiuso, con una forma concreta.** Un valore
opaco di 32 byte reso da **esattamente 64 caratteri esadecimali minuscoli**,
chiave `plenora.commit.token` nel footer, validazione prima dello spawn,
costruzione solo tramite un costruttore che valida, e **mai il valore grezzo
negli errori**.

Il costruttore garantisce **forma, lunghezza e alfabeto**, che è tutto ciò che
sessantaquattro caratteri gli permettono di vedere. Non garantisce la
**casualità** — un token di soli zeri è formalmente valido — né l'**unicità**.
La generazione con un generatore crittograficamente sicuro è una
raccomandazione al chiamante, non una proprietà del tipo.

**F4-22 — I custom metadata entrano nel confine ostile.** Il validatore
percorre oggi i campi 1, 2 e 3 del footer e **non il campo 4**, che è dove
finirà il token; e `arrow-ipc` legge il footer con `key().unwrap()` e
`value().unwrap()`, quindi una voce senza chiave o senza valore **panica**
dentro la dipendenza — mentre il percorso dello schema, che usa `if let`, la
salterebbe. Serve, prima che qualcuno scriva un token in un footer:

| | |
|---|---|
| **1** | validazione grezza di `Footer.custom_metadata` **prima** di costruire il `FileReader` |
| **2** | chiave e valore **obbligatori**: oggi `fb_key_value` salta l'offset zero invece di rifiutarlo |
| **3** | tetti applicati **prima** delle allocazioni: **256** coppie per collezione, **128** byte di chiave, **64 KiB** di valore. Costanti proprie del confine IPC — `MAX_IPC_CUSTOM_METADATA_*` — **non** derivate dal tetto sul CRS: il numero coincide, l'autorità no, e accoppiarle farebbe cambiare in silenzio ciò che il parser accetta il giorno in cui il tetto sul CRS si muove |
| **3-ter** | sono **costanti interne non ampliabili**, non campi di `IpcLimits`: quella struttura è pubblica e riesportata, e serve ai limiti che un piano può modulare — un tetto contro l'abuso che il chiamante può alzare non è un tetto |
| **3-bis** | UTF-8 verificato da noi; chiave vuota rifiutata; valore vuoto accettato; chiavi sconosciute accettate e ignorate — il confine valida la **forma**, non il vocabolario |
| **4** | **chiavi duplicate rifiutate**: nessuna semantica «vince l'ultima», che per un token autoritativo sceglierebbe un vincitore arbitrario |
| **5** | lettura autoritativa del token dal **footer validato**, non dalla `HashMap` di Arrow |
| **6** | sedici test, non sette, e **divisi fra due PR**: dodici casi strutturali a `PR-0` — i tre tetti superati **separatamente**, chiave e valore assenti, chiave e valore vuoti, UTF-8 invalido in entrambi, duplicati uguali e divergenti, chiavi sconosciute — e quattro casi del token a `PR-5`, che è la prima in cui `CommitToken` esiste |

**È una classe, non un caso.** `fb_key_value` è condiviso da tre chiamanti —
campi, schema, messaggi — quindi la correzione vale per tutti, non solo per il
footer. Ma «sta nell'helper» sarebbe impreciso e lascerebbe i duplicati senza
proprietario: `fb_key_value` valida **una** coppia e la **restituisce**;
`fb_custom_metadata`, che vede l'intera collezione, applica il tetto sul
conteggio e rifiuta i duplicati — che sono una proprietà dell'insieme, non di
un elemento.

**Va chiuso in una PR propria, prima della fase 4.** È l'unica correzione di
questo blocco che sarebbe necessaria anche se la fase 4 non esistesse: il
campo 4 è invalidato da sempre, e adottare il footer per il token non ha
creato il difetto ma l'ha scoperto. Cambia semantica in senso fail-closed — input che oggi passano domani sono
rifiutati — e la registrazione in [`errori-e-limiti.md`](errori-e-limiti.md),
con regola, perimetro, pericolo e condizione di rientro, è un **criterio
d'uscita** della PR, non una nota a margine.

Va dichiarato anche che cosa questo rifiuta: Arrow consente metadati
arbitrari, quindi un file con una chiave sconosciuta e un valore da 100 KiB è
un file Arrow **valido** che il confine Plenora rifiuta di proposito.

**F4-17 — Si attribuisce solo con il group kill locale.** I delta sono
contatori aggregati su un intervallo e la loro coesistenza non prova un nesso:
`oom_kill` conta le uccisioni da **qualunque** OOM killer, quindi un antenato
con un tetto più basso lo fa salire senza che il tetto del dominio venga
raggiunto. L'unico segnale che lega causa ed effetto in un solo fatto è
`memory.events.local` → `oom_group_kill`: il kernel ha ucciso *questo* dominio
*come gruppo*.

`ResourceLimit` si attribuisce **solo** lì. Ogni altra combinazione con
evidenza di pressione è **non attribuita** — una categoria propria, da
aggiungere in `PR-1`, che porta i contatori e non conclude al posto di chi
legge. In nessun caso ambiguo si pubblica.

**F4-18 — I picchi non fondano garanzie.** `memory.peak` di un figlio ha
superato quello del padre che lo contiene di 233 472 byte, mentre il kernel lo
definisce come massimo del cgroup **e dei discendenti**: i due valori non
rispettano quella relazione. Non se ne conclude né che il tetto sia stato
superato né che non lo sia stato — il picco del padre non è più affidabile di
quello del figlio, e usarlo per provare «superamento zero» mentre si dichiara
il contatore inaffidabile sarebbe incoerente.

**Su questo kernel il superamento temporaneo non è né dimostrato né escluso.**
Resta che i picchi servono al dimensionamento con l'incertezza dichiarata, non
a fondare garanzie. L'ipotesi dell'addebito *tentato* — la stessa firma
trovata su Windows in `PeakJobMemoryUsed` — resta un'ipotesi finché non la
prova una sonda dedicata.

**F4-11 — Il profilo isolato su Windows è non supportato** finché non esiste
una dipendenza vettata che copra tetto ed evidenza senza `unsafe`. La deroga
alla regola permanente di `AGENTS.md` non è una decisione di questa fase.

**F4-7 — Il tetto del dominio non è ampliabile dall'input.** Il limite in
vigore è il minimo fra ciò che il piano chiede e la politica dell'host, che non
viaggia nel piano né nel protocollo. Esiste anche un pavimento: sotto una certa
soglia il dominio non è vitale, e i prototipi mostrano che i due sistemi lo
comunicano male — `SIGKILL` prima di qualunque riga su Linux,
`STATUS_STACK_OVERFLOW` su Windows. Un tetto sotto il pavimento va respinto in
validazione.

**F4-8 — L'esito si classifica dal dominio, mai dal codice d'uscita del
capofila.** Su entrambe le piattaforme i prototipi hanno osservato un processo
capofila **vivo, con uscita 0**, mentre il dominio aveva appena registrato un
evento di limite. Su Linux `memory.oom.group=1` è quindi obbligatorio, non
consigliato; su Windows, dove non esiste un equivalente, la precedenza
dell'evidenza sul dichiarato è l'unica difesa.

**F4-1 — Il limite è imposto dall'esterno.** Il worker esegue sotto un tetto
di memoria imposto dal sistema operativo, non sotto un tetto che si
autoapplica. Un processo che decide da solo quanto può allocare non è
vincolato: è d'accordo con se stesso.

**F4-2 — Il supervisore osserva l'esito, non lo deduce.** Se il worker viene
terminato, il supervisore deve distinguere «terminato per il limite» da
«uscito con errore» da «uscito bene». Un esito ambiguo diventerebbe un errore
inventato o un successo non verificato.

**F4-3 — La pubblicazione è atomica e verificata.** Nulla è visibile prima
della fine, e ciò che diventa visibile è confrontato con ciò che era atteso.
La garanzia non è che il worker si comporti bene, è che un worker che si
comporta male non pubblichi.

**F4-4 — Il resolver CRS è lo stesso da entrambe le parti.** Supervisore e
worker devono usare **la stessa implementazione** di `CrsResolver`. Non è un
dettaglio di configurazione: `plenora-core::crs::resolve_crs` e
`plenora-kernels-geo::crs::resolve_crs` danno risposte diverse — il primo
rifiuta con `CRS_BACKEND_UNAVAILABLE` ciò che il secondo risolve con PROJ. Se
i due lati ne usassero due diversi, il supervisore potrebbe **rifiutare come
invalido uno schema che il worker ha prodotto correttamente**, o accettarne
uno che il worker non avrebbe potuto scrivere.

Il confine è già pronto a riceverlo: dalla chiusura della fase 2 il resolver è
un **argomento** di `contract_from_arrow_schema`, non una proprietà della
compilazione. Il protocollo dovrà quindi trasportare quale resolver è in uso e
il supervisore dovrà rifiutare un worker che ne dichiari uno diverso — un
disaccordo qui è una condizione di errore, non una differenza da tollerare.

**F4-5 — Nessuna allocazione critica prima dell'autorizzazione, oppure
rifiuto esplicito.** È il criterio di uscita del punto 2. Con l'isolamento,
«autorizzazione» significa che il limite è già in vigore quando il worker
inizia, non che qualcuno abbia stimato in anticipo quanto servirà.

**F4-6 — macOS resta non supportato** per il profilo isolato finché un
prototipo non dimostri copertura *e* attribuzione del limite.


**Stato della fase 0.** Working tree pulito e suite eseguita nel container
1.98. Gli oracoli: il catalogo era già coperto da
`crates/plenora-engine/tests/catalog_snapshot.snap`, l'IPC canonico e la
fusione geo dai rispettivi test. Mancava l'identità dei piani — i test
verificavano che `plan_hash` fosse lungo 64 caratteri, non **quale** valore
avesse, quindi una riorganizzazione che cambiasse la forma canonica sarebbe
passata senza far fallire nulla. Ora c'è
`crates/plenora-engine/tests/oracoli_identita.snap`, che fissa i valori per
nove piani rappresentativi insieme al JSON canonico da cui derivano, così un
diff dice *cosa* è cambiato e non solo *che* qualcosa è cambiato.

**Regola per i PR del refactor.** Nessun PR mescola spostamenti strutturali e
cambiamenti semantici: un PR dichiara quale dei due è, e non è mai entrambi.
Chi sposta codice non tocca algoritmi né visibilità pubbliche, e dimostra
l'equivalenza attraverso gli oracoli; chi cambia semantica lo fa a struttura
ferma, e versiona esplicitamente ciò che rompe.

La regola nasce con un'eccezione già consumata, e vale la pena dirlo invece di
lasciarla sembrare disattesa: il commit che ha chiuso la fase 0 mescola tre
filoni — correzioni di review, migrazione a Rust 1.98, aggiornamento delle
dipendenze. Erano intrecciati negli stessi file, e separarli avrebbe prodotto
stati intermedi mai eseguiti. La regola vale **da lì in avanti**, dove la
struttura è ferma e la separazione è possibile.

### Blocker dichiarato: la nuova linea normativa di `plenora-contracts`

La fase 4 introduce un confine pubblico nuovo (`max_domain_memory_bytes`) e
rende costruibili categorie d'errore finora solo dichiarabili. Entrambi
toccano ciò che `plenora-contracts` descrive.

Il repository è stato **sostituito il 2026-08-18** e il suo contenuto attuale è
una linea normativa nuova, che non descrive i requisiti citati in questo
codice ([`architettura.md`](architettura.md)). Ne segue un blocker esplicito,
perché finora era implicito e un blocker implicito non ferma nessuno:

| | |
|---|---|
| **che cosa** | l'adozione della nuova linea normativa è una **modifica semantica separata** |
| **che cosa NON è** | non è un effetto collaterale della fase 4, e non si fa «già che ci siamo» |
| **le citazioni `R…`** | restano riferimenti storici alla fonte congelata (`v2.0-rc10`, revisione `3598259`): non vanno reinterpretate, riscritte né tradotte contro il nuovo profilo |
| **conseguenza pratica** | finché l'adozione non è decisa e pianificata, i confini pubblici che la fase 4 aggiunge vanno descritti **nella forma attuale**, e la loro traduzione nel nuovo profilo è lavoro successivo |

Chi affronterà quell'adozione dovrà decidere, per ogni citazione, se il
requisito nuovo dice la stessa cosa, una diversa, o non dice nulla — tre esiti
diversi che non si distinguono con una sostituzione meccanica.

## 3. Campagna fuzz finale

Alla chiusura del punto 2, non prima: una campagna cambia significato se il
codice sotto è ancora in movimento.

Va sciolta la posizione di `arrow_transform`, oggi in quarantena per una
ragione dichiarata: `libfuzzer` aborta prima dell'unwinding, quindi il target
resterebbe rosso a barriera funzionante. Due esiti ammessi, e nessuno dei due
è il silenzio: **riattivarlo**, se `apache/arrow-rs#10575` avrà reso fallibile
la conversione dello schema, oppure **riconfermare la quarantena per
iscritto**, con la ragione aggiornata alla data del rilascio.

## 4. Qualifica prestazionale

Oggi le prestazioni non sono qualificate: esistono una baseline di
riferimento, gli harness e gli esempi `bench_*`, ma **nessun gate le
consuma** e nessuna soglia è applicata da qualcosa (vedi
[`release.md`](release.md)). Finché è così, «è abbastanza veloce» è
un'opinione, non un esito.

Serve, prima della produzione:

- una **matrice fissata**: quali scenari, quali scale, quali feature — decisa
  una volta e non a ogni misura, altrimenti due campagne non si confrontano;
- un **ambiente controllato** su cui misurare, dichiarato insieme ai numeri:
  le esecuzioni note vengono da container su host di sviluppo, e la
  variabilità dell'host oggi è dentro il dato;
- **soglie esplicite**, con la regressione che le fa fallire: una soglia che
  nessuno applica è una nota, non un limite;
- il **confronto con la baseline e con la release precedente**, perché il
  numero che conta non è il valore assoluto ma la differenza.

Il lavoro non è in questo ramo e non lo anticipa: qui c'è la dichiarazione di
che cosa manca, così che nessuno legga i numeri sparsi nel codice come se
fossero un verdetto.

## 5. Release

Gate, piattaforme, packaging e procedura sono in [`release.md`](release.md).
Il bump di versione è **maggiore**: la superficie pubblica è cambiata in modo
incompatibile (rinomina del campo dei limiti, versione canonica del piano,
tipi rinominati, `plan_hash` in un dominio nuovo).

---

## Dopo la release

Non prima, e in quest'ordine.

### M3 — scheduler parallelo

Oggi l'esecuzione fra i nodi del DAG è **seriale**; il parallelismo esiste
solo dentro i kernel. M3 introduce l'esecuzione concorrente dei rami
indipendenti.

I prerequisiti già fatti: permesso atomico del governor (senza il quale
nascerebbe un TOCTOU), `BatchSequence` assegnata e propagata, contabilità
linearizzabile. Quello che manca: lo scheduler, e il consumatore che riordina
l'output dei rami paralleli secondo la sequenza logica — oggi assegnata e
testata, ma non ancora usata per riordinare, perché in esecuzione seriale
l'ordine logico coincide con quello di scansione.

**Criteri di accettazione**, oltre allo scheduler stesso:

- il property test «stesso piano, schedule forzato **seriale contro
  parallelo**, risultato semanticamente identico». È il test che dimostrerebbe
  il determinismo di livello 1 nel caso che conta, e oggi non è eseguibile
  perché manca il secondo termine del confronto;
- la `BatchSequence` **usata** per riordinare, non solo assegnata;
- nessuna regressione sul picco di memoria governata con rami concorrenti: il
  permesso è atomico, ma la contesa non è ancora stata misurata.

Un vincolo noto: `max_parallelism` dimensiona il pool globale del processo,
non del piano. Un pool per esecuzione richiederebbe un executor con stato
`Send`.

### SDK Python

**Non esiste.** Due vincoli sono già decisi per quando esisterà:

- il percorso permissivo degli input non sarà esposto: solo il profilo
  stretto, dove un input senza contratto è un errore e non una dimenticanza;
- l'inizializzazione del modulo chiamerà `install(PanicPolicy::Sanitized)` —
  installare un hook di panico è un atto esplicito dell'embedder, non un
  effetto collaterale del caricamento di una libreria;
- in-process non ci sarà un tetto duro sulla memoria; il profilo isolato sarà
  disponibile ma rinuncia allo zero-copy e attraversa IPC.

### Nuove trasformazioni

Il catalogo cresce solo dopo che le garanzie di cui sopra sono chiuse.
Aggiungere operazioni a un motore che può ancora andare in OOM senza
diagnosticarlo sposta il problema, non lo risolve.

---

## Debito dichiarato, senza data

Non blocca la release, ma è scritto perché non si perda.

| voce | dove |
|---|---|
| hasher delle chiavi non keyed: costo peggiore quadratico entro il tetto di righe | [`errori-e-limiti.md`](errori-e-limiti.md) |
| finestra TOCTOU fra pre-validazione del framing IPC e lettura | idem |
| `max_parallelism` di processo, non di piano | idem |
| `max_temp_bytes` per dominio: picco fino a ~3× | idem |
| chiavi canoniche emesse prima della ratifica normativa | idem |
| nessuna policy dell'host sui limiti dati/runtime: un piano non fidato sceglie il proprio budget | idem |
| tetti strutturali del piano applicati dopo la deserializzazione: in parse limita il solo tetto sui byte | idem |
| messaggi delle dipendenze: la riga di confine è il **percorso**, non la libreria, e nessun controllo automatico la tiene onesta | idem |
| scavenging temporaneo: l'hostname non è un'identità di macchina, e il PID è verificabile solo su Linux | idem |
| fuzzing su toolchain nightly | [`release.md`](release.md) |
| `rstar` fermo a 0.12.2: `geo` lo impone come dipendenza obbligatoria, e aggiornarlo da solo metterebbe due R-tree nello stesso binario | idem |
