# Stato e roadmap

Solo il **lavoro ancora aperto**. Quello che è fatto sta nel codice, nei test
e negli altri documenti; qui c'è ciò che manca, in ordine.

## Vincoli noti della Fase 4

**Il primo profilo isolato non accetta ingressi in memoria.** Solo file Arrow
IPC esplicitamente trasferibili: `Input::Batches` e `Input::Stream` non sono
raggiungibili da un altro processo, e `Input::read_ipc` oggi **scarta il
percorso** trasformando un ingresso file-backed in uno `Stream` anonimo. Il
rifiuto avviene prima dello spawn, e un piano con ingressi in memoria resta
valido ed eseguibile in-process.

Il rientro richiede due cose, in quest'ordine: che `Input::read_ipc` conservi
la provenienza file-backed invece di cancellarla, e un meccanismo di staging
isolato che materializzi batch e iteratori in file di cui il worker abbia
proprietà e cleanup. Dettaglio in [`isolamento.md`](isolamento.md).

## Dove siamo

Il core è una release candidate credibile: i formati DAG sono **due** — il
piano v5 e il piano v6, che aggiunge `max_domain_memory_bytes` e ha un dominio
d'identità proprio — la CLI è l'eseguibile distribuito, il catalogo è
documentato in [`operazioni.md`](operazioni.md), generato, e la CI verifica
Linux e Windows ([`release.md`](release.md) è l'autorità sui gate).

Il v4 continua a funzionare, migrato nel canonico v5, di cui condivide il
`plan_hash`; il confine d'identità è fra v5 e v6, e lì soltanto.

Non è ancora rilasciabile in produzione. Le ragioni sono qui sotto, in
ordine di precedenza.

---

## 2. La memoria governata non è un tetto duro

**È il limite più serio del progetto**, ed è la ragione per cui il core non è
in produzione. Il tetto duro per esecuzione esiste, ed è il profilo isolato su
Linux ([`errori-e-limiti.md`](errori-e-limiti.md#il-tetto-duro-per-esecuzione-è-il-profilo-isolato));
il profilo in-process, quello predefinito, resta come descritto qui.

Nel profilo in-process il budget governa la **ritenzione** del risultato, non la sua costruzione:
dove il lease è preso dopo l'allocazione — quasi ovunque — un input che
produce un output molto più grande del budget porta all'esaurimento della
memoria **prima** che l'errore esista. Il fallimento è un OOM, non un errore
diagnosticabile. Il quadro completo è in
[`errori-e-limiti.md`](errori-e-limiti.md).

### Le fasi 5 e 6

Le fasi 0-4 del refactor sono chiuse; la storia sta in Git (tag
`baseline-pre-fase-4`). Restano aperte la 5 — il legacy ridotto a un confine
di migrazione — e la 6 — superficie pubblica e commenti, che cambia
semantica solo la prima.

**Criteri di uscita.** Lo stato di ciascuno è verificabile, e nessuna delle due fasi si dichiara chiusa finché
tutte le righe che la riguardano non lo sono:

| fase | è chiusa quando | oggi |
|---|---|---|
| 5 | esiste **un solo executor** per tutti i piani semanticamente traducibili | **no**: `plenora-cli` dispatcha ancora i piani `schema_version <= 3` sul percorso di `cli::commands::legacy`, che è un secondo executor |
| 5 | i piani non traducibili sono in un modulo isolato con la matrice di ciò che li rende tali | **parziale**: modulo isolato sì, matrice no |
| 5 | la rimozione è pianificata per una major dichiarata | **no** |
| 6 | nessun modulo fuori dall'API è pubblico, e i `lib.rs` espongono facciate ristrette | **no**: il solo `plenora-engine` dichiara dodici `pub mod` |
| 6 | nessun commento di produzione dichiara uno stato di avanzamento («fase», «in corso», «milestone»), mentre restano le condizioni di rientro e le ragioni `D*`/`R*` | **sì**, per il perimetro che il gate copre |
| 6 | un gate lo tiene, con la distinzione fra cronologia e hazard scritta nella sua doc | **sì**: `scripts/verifica_commenti.py`, job `gate-commenti` della CI |

Che un criterio di uscita sia soddisfatto non anticipa la chiusura della fase:
la fase 6 resta aperta finché la superficie pubblica non è ristretta, ed è
l'unico dei suoi tre criteri ancora da fare. Il gate sui commenti è entrato
prima del resto perché presidia una regressione — un commento che racconta il
passato — non perché l'ordine delle fasi sia cambiato.

### La fase 4: chiusa, con i limiti dichiarati

Il progetto tecnico è in [`isolamento.md`](isolamento.md). La sequenza `PR-0`
… `PR-12` è integrata, e il criterio di uscita è soddisfatto: «l'intera
matrice, su Linux; su Windows e macOS il profilo è rifiutato in validazione»
([`isolamento.md`](isolamento.md#112-le-pr-dopo-i-prototipi)), e `F4-5`. La
tabella dice come, e dove la copertura si ferma.

| | stato |
|---|---|
| matrice §10, la logica | ogni riga ha un test: di tabella sulla classificazione, sulla conduzione con i finti, o sul verificatore. Le categorie delle righe 12 e 13 seguono la matrice |
| matrice §10, il percorso di produzione | `scripts/qualifica_profilo_isolato.sh` guida `plenora-data-tools run`, il binario distribuito, con un dominio `cgroup2` reale e il worker con identità distinta: righe 1, 2, 4/6a (un `SIGKILL` da fuori: crash e terminazione senza evidenza non si distinguono da fuori), 5 prima e durante la conduzione con il controllo positivo del carico, 7, 8. L'oracolo è l'envelope su stdout — categoria **e** messaggio — più l'assenza dell'output e dei domini residui; un segnale non consegnato rende il caso rosso. Verde su VM (kernel 6.8) sull'albero di questa fase; l'evidenza sta fuori da Git, e lo script la rigenera |
| righe senza un caso su VM | 3 (nessun kernel di produzione va in panico a comando), 6b (una pressione che non autorizza l'attribuzione non si provoca a comando), 9 e 10 (supervisore e worker sono la stessa immagine), da 11 a 14 (servirebbe manomettere l'artefatto fra worker e verificatore), 15 e 16 (publish e pulizia che falliscono dopo una verifica riuscita): coperte dai soli test, e la qualifica lo dichiara |
| riga 16 | deviazione dichiarata: la pulizia dei domini non esce su un canale machine-readable ([`errori-e-limiti.md`](errori-e-limiti.md#la-pulizia-dei-domini-non-esce-su-un-canale-machine-readable)) |
| `F4-5` | soddisfatto con i limiti dichiarati in [`errori-e-limiti.md`](errori-e-limiti.md#il-coordinatore-del-profilo-isolato-legge-fuori-dal-dominio): prima dell'autorizzazione il coordinatore legge solo il testo del piano e gli schemi degli ingressi, con tetti costanti, e nessun dato |
| Windows | rifiuto in validazione provato dal binario vero, nel job Windows della CI |
| macOS | rifiuto in validazione provato da un test unitario soltanto: la CI non ha un job macOS |

La definizione dei requisiti `F4-1` … `F4-22`, con la motivazione che viene
dai prototipi, sta in
[`isolamento.md`](isolamento.md#2-sexies-i-requisiti-f4); qui ne resta lo
stato, nella tabella sopra.

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

Non è l'unica campagna: la cadenza ordinaria — gate deterministici e suite
completa prima di ogni commit, smoke dei target coinvolti prima del merge,
campagna da 30 minuti dopo — sta in [`release.md`](release.md).

Questa è però l'**unica scadenza lunga** del ciclo di rilascio, e le due cose
coincidono: la campagna finale di cui parla questo punto **è** quella che
`release.md` colloca dopo `PR-12`. Non ce n'è una seconda alle tappe
intermedie, perché una campagna lunga misura l'albero su cui gira e quegli
alberi sono destinati a cambiare. Vale sul candidato congelato, su VM
qualificata e con watchdog esterno al processo — e ogni modifica al candidato
la invalida, perché l'albero misurato non è più quello che si rilascia.

### Criterio aperto: il watchdog non è ancora nel repository

`release.md` rende il watchdog esterno **obbligatorio** per la campagna
lunga. Oggi quel guardiano esiste come script fuori dal repository, provato a
mano: la procedura è quindi **descritta ma non riproducibile**, e una
qualificazione che dipende da uno strumento non versionato non è verificabile
da nessun altro.

**Prima della campagna finale**, e non dopo, va colmato:

| | |
|---|---|
| **che cosa entra** | lo script del watchdog in `scripts/`, con i suoi test, come qualunque altro gate del progetto |
| **conseguenza se non entra** | la campagna finale non è qualificante, perché il suo esito dipenderebbe da uno strumento che il repository non contiene e che nessuno può rieseguire |

**Che cosa devono coprire i test.** Ognuno corrisponde a un modo in cui la
procedura è già fallita a mano, o a una distinzione che sbagliata falserebbe
l'esito:

| il test | perché |
|---|---|
| l'arresto avviene al **muro di tempo** | è l'unica condizione di arresto ammessa |
| il silenzio del log **non** provoca arresto | fermare al silenzio ucciderebbe run sani e li archivierebbe come blocchi |
| corpus e artefatti sono **recuperati prima** dell'arresto | vivono nel `tmpfs` del container e muoiono con lui: è già successo |
| gli artefatti sono contati sulla **directory giusta** | un conteggio su una directory inesistente rende `0` e sembra un successo |
| i file salvati portano il **nome del bersaglio** | una variabile sovrascritta li ha già battezzati con un TID |
| un target che scrive **un artefatto ed esce non-zero** è classificato **`rilievo`** | è la distinzione che protegge il risultato: senza, un crash verrebbe archiviato come `incompleta` e il difetto andrebbe perso |
| un **artefatto preesistente** non viene attribuito al run nuovo | una directory che conserva l'artefatto di ieri produrrebbe un rilievo che nessuno ha trovato oggi, e bloccherebbe la release su un difetto già chiuso |

**L'attribuzione degli artefatti.** Un artefatto vale come rilievo solo se
l'ha prodotto **questo** run. Le directory di `fuzz/artifacts/` sono
persistenti e accumulano, quindi contarne il contenuto non basta. Lo strumento
deve garantire una delle due:

| | |
|---|---|
| **directory isolata** | artefatti in una directory propria del run, **vuota all'inizio**: tutto ciò che c'è dentro alla fine è di questo run |
| **confronto prima/dopo** | inventario all'avvio e alla chiusura, e conta solo la differenza — affidabile, cioè per contenuto e non per numero, perché un artefatto rimosso e uno aggiunto lascerebbero il conteggio invariato |

**Il preflight della VM.** «Non ha già mostrato wedge» è necessario e non
sufficiente: qualificherebbe a vuoto una macchina mai usata. Lo strumento
versionato deve **registrare e verificare**, e allegare all'esito:

| | |
|---|---|
| **kernel** | versione e linea, perché l'esclusione di un host è per ora l'unica difesa contro un blocco non attribuito |
| **risorse** | CPU, memoria e `shm` assegnate al container, e quelle della macchina |
| **configurazione** | immagine e versione di `cargo-fuzz`, con i pin di `release.md` |
| **esclusività** | che durante la campagna non giri nient'altro di significativo sulla macchina: una campagna in contesa misura anche il vicino |

Finché il criterio è aperto, ogni campagna eseguita con lo strumento fuori
repository vale come **evidenza**, non come qualificazione.

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

Il lavoro non è cominciato: qui c'è la dichiarazione di che cosa manca, così
che nessuno legga i numeri sparsi nel codice come se fossero un verdetto.

## 4-bis. Essenzialità e aggiornamento delle dipendenze per la 2.0.0

Prima del congelamento del candidato 2.0.0 serve una revisione dell'intero
albero delle dipendenze, non soltanto delle copie geometriche modificate.
L'obiettivo è eliminare ciò che non serve e qualificare gli aggiornamenti
utili, non minimizzare il numero di crate a scapito delle garanzie né
imporre sempre l'ultima versione. Una versione numericamente distante o
pubblicata da tempo non dimostra, da sola, abbandono o vulnerabilità.

**Sequenza:** nessuna modifica ai pin nel lavoro sui residui di PR-12.
Gli aggiornamenti ordinari si valutano in modifiche separate; la revisione
architetturale di GEOS
e delle copie di `geo`, `wkt` e `i_shape` conserva il vincolo post-PR-13x
descritto sotto. La qualifica finale di prestazioni e fuzzing riguarda
l'albero risultante, non un candidato precedente agli aggiornamenti.

La revisione deve produrre:

- **Inventario riproducibile:** dipendenze dirette e transitive dei workspace
  principale e `fuzz/`, distinte per produzione, test, build, piattaforma e
  feature. Per i backend nativi distinguere versione della crate wrapper e
  versione C/C++ effettivamente incorporata o collegata. Manifesti,
  lockfile, fonti e data del confronto accompagnano l'evidenza.
- **Necessità dimostrata:** per ogni dipendenza diretta, funzione servita e
  chiamanti; per le transitive, catena che le introduce. Valutare rimozione
  delle dipendenze inutilizzate, riduzione delle feature e delle versioni
  duplicate compatibili. Non riscrivere componenti complessi soltanto per
  ridurre il conteggio.
- **Confronto delle versioni:** registrare versione bloccata, ultima stabile
  pubblicata, aggiornamento compatibile con i vincoli e motivo degli
  eventuali blocchi. Verificare changelog, manutenzione, avvisi di sicurezza,
  versioni ritirate, licenze, toolchain minima e piattaforme supportate.
  L'audit di sicurezza è distinto dal confronto dei numeri di versione.
- **Priorità esplicite:** famiglia Arrow; catena geometrica
  `geo`/`rstar`/`i_overlay`/`i_shape`/`i_float`; backend GEOS e PROJ;
  strumenti di fuzzing e dipendenze comuni quali `log` e `uuid`. Una
  vulnerabilità applicabile richiede una decisione tempestiva, non il
  rinvio automatico alla revisione architetturale.
- **Parità motivata fra workspace:** rilevare le differenze di risoluzione
  fra prodotto e fuzz, allineando le dipendenze comuni dove applicabile o
  motivando le differenze di feature, test e piattaforma. Non dichiarare
  identici due alberi verificando soltanto le tre crate vendorizzate.
- **Aggiornamenti qualificati:** niente aggiornamento indiscriminato dei
  lockfile. Ogni gruppo coerente di modifiche conserva pin esatti e
  provenienza, dichiara gli impatti su API, formati, determinismo, privacy,
  memoria e prestazioni, e supera i gate pertinenti di `release.md`.

**Criterio di chiusura:** inventario completo e verificabile sul candidato,
decisione motivata di mantenere, aggiornare, rimuovere o sostituire le
dipendenze esaminate, nessun rilievo bloccante irrisolto e gate verdi sulle
versioni effettivamente distribuite. Le versioni mantenute per compatibilità
devono avere un motivo e una condizione di riesame; eventuali rischi residui
o restrizioni dei contratti vanno nel registro di
[`errori-e-limiti.md`](errori-e-limiti.md), senza esenzioni implicite.

## 5. Release

Gate, piattaforme, packaging e procedura sono in [`release.md`](release.md).
Il bump di versione è **maggiore**: la superficie pubblica è cambiata in modo
incompatibile (rinomina del campo dei limiti, versione canonica del piano,
tipi rinominati, `plan_hash` in un dominio nuovo).

Per la 2.0.0 la revisione di essenzialità e aggiornamento delle dipendenze
qui sopra è un criterio di ingresso al congelamento del candidato, non
manutenzione rinviata a dopo il rilascio.

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

## `PR-13` — il confronto shadow, condizionato a un prototipo

**`PR-13` è condizionata all'esito di `PT-shadow`: senza un provider
filesystem approvato, `PR-13a` non è implementabile e nessun profilo shadow
viene offerto.**

È la prima voce di questo documento che non è né fatta né semplicemente da
fare: è **subordinata a un prototipo che non è stato eseguito**, e potrebbe non
essere mai realizzabile. Va letta così e non come lavoro pianificato.

**Che cosa è, e che cosa non è più.** `PR-13` è l'**infrastruttura di confronto
shadow** fra un kernel candidato e il backend autorevole. Non è
«l'integrazione di `memory-lab`»: quel nome descriveva un contenitore, non un
lavoro, e progettarla ha diviso quel contenitore in tre parti con destini
diversi.

| che cosa | destino |
|---|---|
| i tre kernel Geo Rust — `polygonize`, `split`, `make_valid` | i **primi clienti** dell'infrastruttura, uno per PR, con GEOS sempre autorevole |
| i ventiquattro fast path table e Geo | **rinviati**, uno per PR ciascuno, ognuno col proprio oracolo differenziale |
| il **catalogo empirico della memoria** | **fuori dal perimetro**: le misure sono Windows-only e il profilo isolato è Linux, quindi non esiste una grandezza comune da consumare. Rientra solo dopo una campagna Linux con una metrica coerente col dominio |

`memory-lab` **esiste**, fuori da questo repository. Dal suo candidato
vengono le copie **vendorizzate** di `geo`, `wkt` e `i_shape` che il prodotto
usa tramite `[patch.crates-io]`, ciascuna ricostruita dal pacchetto pubblicato
più le patch in `patches/` e verificata da `scripts/verifica_vendor_provenienza.py`.
Nient'altro: nessun dato, nessuna misura, nessuna dipendenza di percorso verso
`memory-lab`, che il repository del prodotto non deve avere. La provenienza si
**cita** — commit immutabile e hash degli artefatti — non si copia.

**La sequenza**, e nessun passo salta il precedente:

| | |
|---|---|
| `PT-shadow` | prototipo bloccante: provider di contenimento filesystem e impossibilità del candidato |
| `PR-13a` | infrastruttura con candidato sintetico |
| `PR-13b` … `PR-13d` | `polygonize`, `split`, `make_valid` |

`PR-13` **segue `PR-12`**, e questo non è cambiato. Il progetto — prerequisito,
architettura, policy, garanzie, privacy e criteri di uscita — è in
[`isolamento.md`](isolamento.md), nella sezione dedicata al confronto shadow. I
meccanismi non vi sono fissati: li decide la progettazione di `PR-13a`, dopo che
il prototipo avrà dato un provider reale e i suoi vincoli.

### Dopo PR-13x — revisione delle dipendenze geometriche

La valutazione della riduzione delle dipendenze geometriche segue
`PR-13a` … `PR-13d`: non riapre l'integrazione numerica per tentare ora
di eliminare le copie modificate di `geo`, `wkt` e `i_shape`. Le correzioni
qualificate nel relativo ciclo restano il riferimento finché un'alternativa
non dimostra le stesse garanzie. Questo punto pianifica la revisione delle
copie distribuite, non ne autorizza la sostituzione.

La revisione distingue due decisioni, nessuna automatica:

- **Sostituzione di GEOS:** lo shadow confronta i candidati, ma GEOS resta
  autorevole durante `PR-13x`. La promozione di ciascun kernel al percorso
  canonico richiede una decisione separata, con correttezza, determinismo,
  privacy, limiti e prestazioni qualificati. Prima di rimuovere GEOS va
  censita l'intera superficie che ne dipende, non soltanto i tre kernel.
- **Ritorno alle dipendenze ufficiali:** verificare separatamente per `geo`,
  `wkt` e `i_shape` se una versione upstream corretta, un controllo esatto
  al confine o una sostituzione dell'implementazione permette di rinunciare
  alla copia modificata. Evitare i soli panici noti non basta: devono essere
  esclusi anche risultati numerici errati, perdite di dati e fughe nei log.

Ogni proposta deve coprire l'intera classe del difetto e tutti i chiamanti,
inclusi gli ingressi diretti ai kernel e gli stati intermedi degli algoritmi.
Non sono ammesse eliminazioni silenziose di componenti geometrici o
restrizioni implicite degli input. Un contratto più restrittivo richiede
approvazione esplicita e registrazione in
[`errori-e-limiti.md`](errori-e-limiti.md), con ambito, hazard e condizione di
rientro.

L'esito atteso è una decisione motivata per ciascuna dipendenza: mantenerla,
aggiornarla alla versione ufficiale o sostituirla, con regressioni conservate
e provenienza e risoluzione verificate nei workspace principale e `fuzz/`.
La rimozione non è un criterio di successo a scapito delle garanzie. Se
`PT-shadow` blocca la sequenza, la revisione non viene anticipata
automaticamente: occorre una nuova decisione sul perimetro.

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
