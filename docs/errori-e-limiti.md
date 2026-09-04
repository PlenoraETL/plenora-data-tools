# Errori e limiti

Che cosa succede quando qualcosa va storto, e **dove le garanzie si
fermano**. La seconda parte è quella che conta: un limite dichiarato è un
limite gestibile, un limite taciuto è una sorpresa.

## Envelope e canali

Ogni errore esce come **envelope JSON su stdout**, una riga, `protocol_version
1`. **stderr resta vuoto.**

```json
{"status":"error","protocol_version":1,
 "error":{"category":"invalid_plan","phase":"validate","remote_effect":"none",
          "retry":{"kind":"never"},"message":"...",
          "context":{"node":"f","operation":"table.filter","execution_id":"..."}}}
```

Il canale unico è una **garanzia verificata**, non un'intenzione: i test
golden asseriscono che stderr sia vuoto anche sui percorsi d'errore. Chi
compone la CLI in una pipeline può leggere stdout e ignorare stderr senza
perdere diagnosi.

`context` è presente solo per errori nati in un'esecuzione DAG, e risponde a
«quale nodo ha rotto» senza costringere a leggere il messaggio.

## Tassonomia

Quattro assi **espliciti**, mai dedotti dal testo del messaggio.

**Categoria** (20 valori stabili):

`invalid_plan`, `invalid_configuration`, `schema`, `data_mapping`, `crs`,
`unsupported`, `not_found`, `conflict`, `authentication`, `authorization`,
`timeout`, `cancelled`, `resource_limit`, `io`, `protocol`, `transient`,
`execution`, `isolation_unavailable`, `unattributed_memory_pressure`,
`internal`.

Le ultime due nascono con l'isolamento della Fase 4 e dicono ciascuna ciò che
**non** si è potuto stabilire:

- `isolation_unavailable` — l'ambiente non offre nessuna delle forme di
  separazione previste. Non è `invalid_plan`: lo stesso piano, su un'altra
  macchina, gira;
- `unattributed_memory_pressure` — c'è evidenza di pressione di memoria e
  **non** è attribuibile al dominio. Non è `resource_limit`, che direbbe al
  chiamante che ha superato il proprio budget quando non lo sappiamo, e non è
  `internal`, che dichiarerebbe un difetto nostro quando spesso è l'ambiente.
  L'errore porta con sé i cinque segnali osservati invece di concludere al
  posto di chi legge: vedi [l'evidenza è una struttura, non un
  booleano](isolamento.md#100-bis-levidenza-è-una-struttura-non-un-booleano).

Il nome della seconda dichiara ciò che manca. `resource_pressure` sarebbe
stato più comodo e più vago: comprenderebbe CPU, disco o descrittori, e
soprattutto tacerebbe il punto — che l'attribuzione non c'è.

### Due categorie sono estensioni locali, non canone

**La regola.** Dei venti valori, **diciotto** vengono dalla fonte congelata
(contratti trasversali v2.0-rc10, R9.5: sottoinsieme ammesso, valori propri
vietati). **Due no**: `isolation_unavailable` e
`unattributed_memory_pressure` sono estensioni locali introdotte
dall'isolamento della Fase 4. Vanno chiamate così ovunque, codice compreso.

**Il perimetro.** Solo l'asse **categoria**. Gli altri tre — fase, effetto
remoto, disposizione di retry — restano sottoinsiemi stretti del canone, e le
sei varianti nuove di `PlenoraError` non ne introducono valori propri: usano
`prepare`, `write`, `commit`, `none` e `never`, tutti canonici.

**Il pericolo che questo dichiara.** Un componente gemello che riceve un
envelope con una di queste due categorie **non la riconosce**. Presentarle
come canoniche direbbe a chi le legge che sono interoperabili, e non lo sono:
chi integra deve sapere che qui c'è una divergenza, non scoprirla quando il
suo `match` cade nel ramo predefinito. L'exit code non aiuta a distinguerle —
entrambe proiettano su `5` insieme a diverse categorie canoniche.

**Perché non si è riusato un valore canonico.** Perché nessuno dice queste
condizioni senza affermare qualcosa di non dimostrato: `resource_limit`
attribuirebbe al chiamante un superamento del proprio budget quando
l'attribuzione è precisamente ciò che manca, `internal` dichiarerebbe un
difetto nostro, `invalid_plan` un difetto del piano — che invece, su un'altra
macchina, gira.

**La condizione di rientro.** Quando sarà adottata la linea normativa nuova,
le due andranno **tradotte** in valori canonici se ne esisteranno di
equivalenti, oppure **ratificate** come aggiunte al canone. Fino ad allora
restano locali e dichiarate. Adottare quella linea è un lavoro separato: non è
anticipato da questa deviazione né sostituito da essa.

**Fase** del ciclo in cui l'errore è nato (10 valori canonici): `validate`,
`connect`, `probe`, `prepare`, `read`, `write`, `finalize`, `commit`,
`rollback`, `cleanup`. Non esiste una fase «execute»: l'esecuzione dei nodi
ricade in `write`, che è la produzione dell'output.

**Effetto remoto**: `none`, `rolled_back`, `partial`, `committed`, `unknown`.

**Disposizione di retry**: `never`, `safe`, `requires_idempotency_key`,
`requires_recovery`, `after(durata)`.

L'exit code è la proiezione della categoria: vedi [`cli.md`](cli.md).

## Privacy dei messaggi

**Nessun dato nei messaggi d'errore.** Mai valori di cella, mai payload, mai
frammenti di riga. Un errore porta `execution_id`, nodo, operazione,
categoria, fase e catena interna delle cause — cioè tutto quello che serve a
diagnosticare, e nulla che serva a ricostruire i dati.

Vale anche per i percorsi indiretti: il confine IPC converte i panici in
errori **sanitizzati**, e i messaggi delle dipendenze non attraversano il
confine così come sono.

## Propagazione

**First-error wins**, con una precisazione che conta in un DAG: il primo
errore **osservato** cancella immediatamente gli altri rami, ma l'errore
**riportato** è scelto con una regola stabile, perché in presenza di rami
paralleli «il primo» cambierebbe fra esecuzioni:

1. errore non causato da cancellazione;
2. minore profondità topologica;
3. `NodeId` minore;
4. sequence number minore.

La diagnosi è deterministica anche quando l'osservazione non lo è.

## Cancellazione

Cooperativa, tramite `CancellationToken`. `run` installa un handler Ctrl-C:
al primo segnale l'esecuzione viene cancellata, **nessun output è
pubblicato**, il messaggio è pulito e l'exit code è **130**. Un secondo Ctrl-C
forza l'uscita immediata.

Ogni operazione dichiara in catalogo il proprio `cancellation_behavior`:
`Cooperative` risponde ai punti di controllo, `BoundaryOnly` solo ai confini
di batch. Le operazioni non interrompibili sono **osservabili**: si sa quali
sono, invece di scoprirlo aspettando.

## Panic policy

L'engine non installa hook di panico **d'ufficio**. Chi lo usa come libreria
chiama `plenora_core::panic_policy::install(PanicPolicy::Sanitized)`; la CLI
lo fa.

La ragione è che l'hook è globale al processo: installarlo come effetto
collaterale del caricamento della libreria significherebbe cambiare il
comportamento di un programma che non l'ha chiesto.

**Che cosa resta garantito comunque**: il confine IPC ha una barriera
`catch_unwind` su schema e su ogni batch, quindi un panico di una dipendenza
diventa un errore tipizzato e sanitizzato. L'errore è senza dati sempre.

**Che cosa non è garantito**: un embedder che non installa nulla — o un
processo in cui qualcun altro reinstalla un hook dopo di noi — vede il
comportamento di `std`, cioè il payload del panico su stderr, e quel testo può
contenere valori di riga se a generarlo è stata una macro di asserzione di una
dipendenza. È una condizione dichiarata, non un difetto nascosto.

Nel codice di produzione non esistono primitive di panico né `unsafe`: è un
gate bloccante di CI (vedi [`release.md`](release.md)).

## Publish e cleanup

L'output è pubblicato **atomicamente**: scrittura su tempfile, poi rename
no-clobber. Se qualcosa fallisce prima del rename non esiste alcun file
parziale nella destinazione.

Un fallimento di `fsync` **dopo** il rename è una condizione dichiarata: il
rename è già avvenuto, e l'errore riporta l'effetto reale invece di fingere
che non sia successo nulla.

## Memoria governata

`max_governed_memory_bytes` è un **budget di ammissione** della memoria che la
libreria contabilizza. Non è RSS, non è la memoria del processo, e la
differenza con `top` non è un difetto: è la parte non governata.

Il perimetro è più stretto di quanto il nome suggerisca, e vale la pena dirlo
per esteso: **è contabilizzato ciò per cui esiste una prenotazione esplicita**,
non «la memoria che la libreria usa».

**Nel perimetro**, sito per sito:

| che cosa | dove |
|---|---|
| i **batch sugli archi** del grafo, attribuiti all'arco | ingresso di ogni segmento |
| l'**output materializzato** dei nodi blocking, unari e binari | dopo la costruzione del batch |
| l'**output del segmento** al confine, ritagliato dal permesso quando esiste | uscita del segmento |
| la **geometria decodificata** dei gruppi geo fusi e delle operazioni geo binarie | prima della decodifica |
| i **batch riletti** dallo staging su disco | replay |

**Fuori dal perimetro**, e sono le voci che contano quando la macchina va in
affanno: tabelle hash di join e aggregazioni, indici spaziali, copie
intermedie e buffer di crescita dei kernel, il writer IPC, le strutture fisse
di planner ed executor, e la memoria nativa di GEOS e PROJ. Un kernel che
costruisce una tabella hash grande quanto l'input la costruisce **senza
chiedere permesso a nessuno**.

Sulla **condivisione** la garanzia è una sola, e più stretta di «una volta per
buffer»: al fan-out lo stesso `GovernedBatch` porta un lease **condiviso**,
quindi quel batch è contabilizzato una volta sola finché l'ultimo riferimento
non lo rilascia. Non c'è invece alcuna deduplicazione **globale** dei buffer
Arrow: due batch distinti che affettano lo stesso buffer sottostante pagano
due volte. Il conteggio è per batch, **mai per riga**.

La quota si prende con un **permesso atomico**: `permesso(bytes, owner)`
verifica e prenota in una sola operazione. `ritaglia` riduce un permesso alla
dimensione reale senza riprenotare ed è fallibile — un ripiego su una nuova
prenotazione riaprirebbe esattamente la finestra che il permesso chiude. La
contabilità sta sotto un lock unico, con aritmetica controllata: lo snapshot è
linearizzabile e una corruzione è visibile in
`MemoryMetrics.accounting_corrupted`.

### Che cosa la memoria governata NON garantisce

**Non è un tetto duro sull'esecuzione**, ed è la limitazione più importante di
questo documento.

Dove il lease è preso **dopo** l'allocazione — cioè in quasi tutti i siti — il
budget governa la **ritenzione** del risultato, non la sua costruzione. Un
input che produce un output molto più grande del budget porta all'esaurimento
della memoria del processo **prima** che l'errore `resource_limit` esista. Il
fallimento è un OOM, non un errore diagnosticabile.

Il rifiuto **preventivo**, prima di allocare, esiste solo in tre punti:

- `table.cross_join`, `table.concat`, `table.concat_by_name`, `table.melt`,
  tramite `preflight_output_bytes`;
- il **gruppo geo fuso**, dove la reservation dei byte decodificati — calcolati
  esatti percorrendo la cella, senza materializzare — precede la decodifica;
- i **segmenti row-diagnostics**, dove il permesso per l'output è chiesto
  **prima** della passata e ritagliato dopo, alla dimensione reale. Se il
  budget non basta il segmento passa allo staging su disco invece di
  proseguire e scoprirlo dopo.

E anche lì la stima **non è una misura**: copre i buffer del risultato secondo
il modello dichiarato dal kernel, non le allocazioni temporanee che
l'implementazione fa per conto proprio (tabelle hash, copie intermedie, buffer
di crescita). Un modello si può sbagliare — è già successo, con una formula
unica per quattro operazioni diverse che ne sottostimava tre.

La formula corretta per descrivere la garanzia è **controllo di ammissione
post-allocazione**, non budget globale duro.

### Profilo isolato: non implementato

Un tetto duro per singola esecuzione **non è realizzabile in-process**, e la
ragione non è pigrizia:

- Arrow chiama `handle_alloc_error` sul fallimento di allocazione, che
  **aborta il processo**: un allocatore che rifiuta non produce un errore
  diagnosticabile, produce un abort senza stack, senza messaggio e senza
  `execution_id`;
- GEOS e PROJ allocano dal `malloc` di sistema, fuori da qualunque
  contabilità Rust;
- «per processo» non è «per esecuzione».

Il tetto duro arriverà quindi da un **profilo isolato** — l'esecuzione in un
processo worker con un limite imposto dal sistema operativo — che **non è
ancora implementato**. Il nome del suo limite (`hard_process_memory_bytes`)
non esiste nel codice, di proposito: introdurre il nome prima del meccanismo
ripeterebbe l'errore che si è appena finito di correggere.

Quando arriverà, quel profilo prometterà **contenimento e attribuzione**, non
«mai un byte oltre N»: su Linux `memory.max` ammette superamenti temporanei,
su Windows il limite riguarda la memoria *committed* che non coincide con
l'RSS, e su **macOS non sarà supportato** finché un prototipo non dimostri
copertura *e* attribuzione. Un `ResourceLimit` sarà riportato solo con
evidenza attribuibile (`memory.events.local` su Linux, notifica del Job Object
su Windows): un segnale o un exit code anomalo sono compatibili con un crash
quanto con un superamento, e senza evidenza specifica l'errore resta
`internal`.

## Limiti dichiarati

Tutti quelli che seguono sono **noti, deliberati e presidiati**. Nessuno è un
difetto da scoprire.

### `max_temp_bytes` è per dominio, non globale

La quota si applica **per dominio di scrittura** — staging del gate WKB,
staging degli output accettati dei segmenti row-diagnostics, spill degli
operatori — non come tetto unico su disco. Il picco può arrivare alla somma
dei domini, fino a ~3×.

Chi dimensiona i volumi temporanei deve usare la somma, non la quota singola.
Ogni dominio resta individualmente limitato e fail-closed. È un design
intenzionale e permanente: un contatore condiviso introdurrebbe accoppiamento
e rischio di stallo senza coprire un hazard reale, che è la memoria, non lo
spazio disco temporaneo.

### `max_parallelism` è del processo, non del piano

Il tetto sul parallelismo dimensiona il pool Rayon **globale del processo**.
Due piani nello stesso processo che dichiarano gradi diversi: il secondo è
**rifiutato**, non silenziosamente ridimensionato. Chi incorpora l'engine e
non chiama `plenora_engine::parallelism::configure` resta sul default di Rayon
(core logici).

Rientro possibile solo con un executor con stato `Send`, che consentirebbe un
pool per esecuzione.

### `max_batch_bytes` non si applica agli archi interni fusi

Su un arco interno di un gruppo geo fuso il batch non è materializzato — la
geometria vive in forma decodificata transiente — e il controllo sui byte non
è applicabile. La protezione è **spostata** sul governor, che prenota
esattamente i byte decodificati, non rimossa. Deroga permanente: il rientro
sarebbe la rinuncia alla fusione.

### IPC compresso rifiutato

Il confine di lettura rifiuta ogni messaggio Arrow IPC che dichiari un body
compresso (LZ4/ZSTD), su tutti gli ingressi file, stream e CLI. Con la
compressione i prefissi di lunghezza non limitano più la dimensione
decompressa, e il tetto sulle allocazioni decadrebbe.

I writer di questo progetto non comprimono mai, quindi nessun artefatto
prodotto qui è interessato. Un input compresso prodotto da terzi non è
leggibile.

### Custom metadata IPC oltre i tetti del confine

Il confine di lettura applica tre tetti alle collezioni di custom metadata —
schema, campi, messaggi e footer — **prima** di qualunque allocazione
proporzionale al conteggio:

| tetto | valore |
|---|---|
| coppie in una collezione | 256 |
| byte di una chiave | 128 |
| byte di un valore | 64 KiB |

`MAX_IPC_METADATA_BYTES` limita i **byte** dei metadati, non il numero di
elementi: centomila coppie minuscole stanno dentro 16 MiB e producono
centomila allocazioni in chi le raccoglie.

I tre valori sono costanti **interne e non ampliabili**, non campi di
`IpcLimits`: quella struttura esiste per i limiti che un piano può modulare, e
un tetto contro l'abuso che il chiamante può alzare non è un tetto. Il tetto
sul valore **non** è derivato da `MAX_CRS_DEFINITION_BYTES`: il numero
coincide, l'autorità no, e accoppiarli farebbe cambiare in silenzio ciò che il
parser accetta il giorno in cui il tetto sul CRS si muove.

**Che cosa questo rifiuta.** Arrow consente metadati arbitrari: un file con una
chiave sconosciuta e un valore da 100 KiB è un file Arrow **valido** che questo
confine rifiuta di proposito. Il rientro sarebbe alzare i tetti, e richiede un
caso d'uso legittimo che oggi non esiste — l'uso più denso del progetto è una
colonna geometrica con una decina di chiavi.

### Custom metadata IPC di forma non ammessa

Sono rifiutati, sugli stessi quattro percorsi: chiave o valore **assenti**,
chiave **vuota**, chiave o valore non **UTF-8**, chiave **duplicata**.

La ragione non è il rigore per il rigore. `arrow-ipc` legge i custom metadata
del footer con `key().unwrap()` e `value().unwrap()`, quindi una voce senza
chiave o senza valore raggiunge una primitiva di panic **dentro la
dipendenza** — mentre il percorso dello schema, che usa `if let`, la
salterebbe. E chi raccoglie le coppie in una mappa comprime i duplicati con
«vince l'ultima», che su una chiave autoritativa sceglie un vincitore
arbitrario.

Il **valore vuoto** è invece accettato: rifiutarlo romperebbe file legittimi
che rappresentano un campo assente con la stringa vuota. Le **chiavi
sconosciute** sono accettate e ignorate: questo confine valida la *forma*, non
il vocabolario, e rifiutare le chiavi altrui romperebbe l'interoperabilità con
qualunque produttore Arrow che aggiunga le proprie.

Non è una deroga con rientro: è la forma che il confine pretende.

### Campi che `arrow-ipc` dereferenzia senza controllarli

Stessa classe, altri tre punti, chiusi insieme: lo **schema del footer**, il
campo **`fields`** di uno schema, e **`indexType`** di una codifica a
dizionario. Tutti e tre sono letti da `arrow-ipc` con `unwrap`, e tutti e tre
erano trattati dal confine come opzionali — cioè saltati se assenti. Il writer
li emette sempre, quindi pretenderli non rifiuta alcun file legittimo.

**Che cosa resta aperto, per intero.** Una stesura precedente lo riduceva a
«`Field.children`», che è solo la voce più visibile. La superficie non ancora
validata è questa:

| | che cosa manca |
|---|---|
| coerenza discriminante/payload | il `type_type` di un `Field` dichiara un tipo, il `type` ne porta la tabella: che i due concordino non è verificato |
| payload del tipo assente | `type_type` presente e `type` assente — o viceversa |
| arità dei figli | `List` e `LargeList` vogliono **un** figlio, `Map` uno, `RunEndEncoded` due: `convert.rs` fa `panic!("expect a list to have one child")` |
| domini degli enum | unità di tempo, di durata, di intervallo, ampiezze di bit: valori fuori dominio finiscono in `panic!`/`unimplemented!` |
| combinazioni numeriche | ampiezza e segno di un intero, precisione e scala di un decimal, coppie che nessun tipo Arrow rappresenta |

Chiuderla richiede una tabella tipo → forma attesa, e una tabella sbagliata
rifiuta file legittimi: è lavoro con un rischio proprio, e va fatto sapendo
che cosa si sta decidendo.

Fino ad allora la **barriera anti-panico resta necessaria**, ed è di nuovo
**coperta**: l'artefatto di fuzz che la esercitava viene ora rifiutato prima,
in modo strutturato, e al suo posto c'è un caso costruito — uno stream Arrow
vero con una colonna `List` a cui viene tolto il campo `children`. Quel test
non sostituisce l'hook di panico del processo: accetta il rumore su stderr
invece di mutare stato globale mentre gli altri test girano in parallelo.

### Finestra TOCTOU sull'ingresso IPC

La pre-validazione del framing è un *time-of-check*, la lettura di Arrow è il
*time-of-use*: fra i due c'è una finestra. Se il file viene mutato sotto i
piedi resta attiva la barriera anti-panico, ma **decade il tetto sulle
allocazioni**, che vale solo sui byte effettivamente pre-validati.

Il rientro richiede uno snapshot immutabile disponibile su tutte le
piattaforme supportate, senza imporre una copia dei dati.

### Lo spill dimensiona un buffer su una lunghezza dichiarata

**La regola.** Nei confini che leggono da una sorgente **non fidata** — il
lettore dell'envelope `PLNGEO3` e quello dei frame `PLNGEO2` — la memoria
cresce con i byte che una `read` ha davvero reso, mai con la lunghezza che
l'ingresso dichiara: buffer fisso, piccolo, riusato.

**Il perimetro.** `plenora-kernels-table::spill` **non** segue quella regola:
rilegge la lunghezza di un record e dimensiona il buffer prima che quei byte
siano provati. La differenza è il confine di fiducia, e la decisione è di
tenerla così:

| | |
|---|---|
| che cosa legge | file di spill **che abbiamo scritto noi**, nella directory temporanea di questa esecuzione |
| che cosa fa fuori tetto | `PlenoraError::Internal`, non un errore del chiamante: il writer non può produrre un record oltre `max_record_bytes`, quindi rileggerne uno più grande significa che il file non è quello che abbiamo scritto |
| che cosa **non** garantisce | il riuso del buffer non toglie l'amplificazione. `key.resize(length, 0)` tocca `length` byte **prima** che `read_exact` li provi, e il tetto che li governa è `max_temp_bytes`, non 16 KiB |

**I due pericoli, distinti.**

Il primo è **il consumo**, e c'è già oggi. Un solo record troncato o corrotto,
con una lunghezza qualsiasi **entro** `max_record_bytes`, fa crescere il
buffer fino a quella misura e poi incontra l'EOF. Il riuso fra i record non
impone un'allocazione a ogni record — ma non la esclude, perché un record che
supera la capacità già allocata la fa crescere di nuovo — e soprattutto non
evita il picco:
per raggiungerlo basta un record solo, e la memoria toccata resta legata a un
numero che sta scritto nel file, non ai byte che il file contiene. Il confine
di fiducia riduce **chi** può scrivere quel numero; non riduce quanto costa
quando è sbagliato.

Il secondo è **la classificazione**. Se quei file diventassero raggiungibili
da altri — una directory temporanea condivisa, un supervisore che li passa fra
processi, uno spill che sopravvive all'esecuzione — la lunghezza smetterebbe di
essere un'invariante nostra e diventerebbe un ingresso. Il codice non se ne
accorgerebbe: continuerebbe a rispondere `Internal`, cioè «difetto nostro», a
un dato che ha scelto qualcun altro.

**Le condizioni di rientro**, tecniche e indipendenti fra loro:

- **sul consumo**: il lettore adotta una delle due discipline — lettura
  incrementale su buffer fisso, come `PLNGEO3` e `PLNGEO2`, oppure verifica
  preventiva che i byte dichiarati stiano nella finestra residua del file
  prima di dimensionare. La prima non richiede di conoscere la lunghezza del
  file; la seconda sì, ed è praticabile qui perché la sorgente è un `File` di
  cui sappiamo la taglia;
- **sulla classificazione**: il giorno in cui i file di spill escono dal
  processo che li ha scritti, il tetto smette di essere un'asserzione interna e
  diventa un errore del confine.

Fino ad allora entrambi i rischi residui sono **accettati e dichiarati qui**,
non risolti.

### Hasher delle chiavi non keyed

`KeyHasher` non è keyed: il costo peggiore dipende dai dati. I limiti di riga
limitano `n` e quindi il costo assoluto, ma non impediscono il comportamento
quadratico **entro** quel `n`. Nessuna perdita di correttezza: i risultati
restano quelli.

Un hasher con chiave per processo cambierebbe la stabilità dell'hash fra
esecuzioni, su cui poggiano più kernel, e va verificato su tutti gli usi prima
di essere introdotto.

### Arrotondamento nelle operazioni a risultato `Float64`

La conversione intero/decimal → `f64` è **esatta o errore**, tranne nelle
operazioni il cui risultato è un `Float64` per contratto: lì il double è il
tipo del risultato, non un passaggio intermedio, e pretendere l'esattezza
rifiuterebbe input legittimi. Un valore oltre 2^53, o un decimal frazionario,
perde precisione senza errore. Nessun percorso decisionale ha questa
proprietà.

Non è una deroga con rientro: è la semantica dichiarata di quelle operazioni.

Il contro-esempio sta nella stessa famiglia: le funzioni di **rango**
(`rank`, `dense_rank`, `percent_rank`, `cume_dist`) producono un `Float64` ma
**ordinano**, quindi non convertono — confrontano il dominio originale.

### Le funzioni di rango non accettano colonne testuali

**Regola.** `rank`, `dense_rank`, `percent_rank` e `cume_dist` rifiutano una
`column` di tipo `Utf8`, in analisi e in esecuzione, con lo stesso confine. Le
altre varianti di `table.window_function` continuano ad accettarla: il testo
resta ammesso dove il risultato è un valore, non un ordine.

**Perché.** Il testo numerico non ha un ordine esatto senza un'aritmetica
decimale. Interpretarlo come double renderebbe a pari merito numeri distinti —
`"9007199254740993"` e `"9007199254740992"` collassano sullo stesso `f64` —
che è esattamente ciò che il confronto sul dominio originale evita per gli
interi nativi.

**Hazard di compatibilità.** Un piano che oggi ordina per rango una colonna
`Utf8` era accettato e produceva un risultato; ora è **respinto in analisi**.
Il rifiuto è preferibile al risultato precedente, che sopra 2^53 era
silenziosamente sbagliato, ma resta una rottura di compatibilità: chi ha piani
del genere deve convertire la colonna a un tipo numerico prima del rango.

**Condizione di rientro.** La restrizione cade quando esiste un comparatore
esatto sull'intera grammatica numerica testuale — segno, parte intera,
frazionaria ed esponente — condiviso da analizzatore e kernel e sorvegliato
dagli stessi oracoli letterali che oggi coprono gli interi nativi. Finché quel
comparatore non c'è, la restrizione resta: non è una scelta di comodo, è
l'assenza di un'aritmetica.

### Row diagnostics: le collettive di solo trasporto

Le operazioni collettive e one-to-many del solo trasporto non raccolgono
diagnostica per riga: l'attribuzione sarebbe a **insiemi** di righe (l'intera
collezione, o coppie), fuori dallo schema single-row corrente. Su quel
percorso una failure del kernel su input già validato dal gate è fail-closed
senza `source_index`. Nel DAG quelle operazioni non sono dispatchate.

### Chiavi canoniche emesse prima della ratifica

Le chiavi di metadata canoniche sono emesse prima che la sezione normativa
corrispondente sia ratificata. Arrow le preserva per costruzione e nessun
consumatore attuale le interpreta: il rischio è limitato a un'incompatibilità
di **nomi**, coperta dalla migrazione se i nomi ratificati risultassero
diversi.

### `arrow_transform` in quarantena nel fuzzing

Il target fuzz `arrow_transform` è **disattivato**. Non perché la barriera non
funzioni, ma per il contrario: `libfuzzer` chiama `std::process::abort()`
prima che l'unwinding cominci, deliberatamente, perché un `catch_unwind` nel
codice sotto test nasconderebbe i difetti al fuzzer. Il target resterebbe
quindi **rosso a barriera funzionante**, e un job perennemente rosso smette di
essere letto.

Va riattivato quando `arrow-rs` renderà fallibile la conversione dello schema,
cioè quando la barriera non servirà più. Segnalato a monte:
`apache/arrow-rs#10575`.

### `max_total_rows_processed` non è un limite

È una **metrica**. Il suo valore dipende dal piano fisico — due piani
semanticamente equivalenti possono conteggiarlo diversamente, per esempio con
segmenti fusi — quindi non può essere un criterio di rifiuto deterministico.

### Non esiste una policy dell'host sui limiti dati/runtime

Il blocco `limits` di un piano **dichiara** memoria governata, spazio
temporaneo, righe, payload, batch, stringhe e geometria di quella esecuzione,
e può dichiararli anche più alti dei default della libreria. È il modo
previsto per dimensionare una corsa.

**Regola:** sui limiti dati/runtime non c'è un tetto imposto da chi ospita la
libreria; l'unico controllo è quello di dominio (`Limits::validate`: nessun
limite a zero, `spill_partitions` nell'intervallo ammesso,
`max_expansion_factor` finito e positivo).
**Ambito:** `plenora_engine::plan::LimitsOverride` e
`plenora_engine::plan::formato_v6::LimitsOverrideV6`, quindi ogni ingresso di
piano v4, v5 e v6.
**Hazard:** chi incorpora l'engine ed esegue piani **non fidati** non ha modo
di imporre un massimo: il documento sceglie il proprio budget. Per la CLI e
per chi esegue piani propri non è un hazard — piano e policy hanno lo stesso
autore — ma per un servizio multi-tenant lo sarebbe.
**Condizione di rientro:** una policy massima esplicita passata dal chiamante,
distinta dai default, con intersezione campo per campo e rifiuto di ogni
ampliamento. La stessa regola è **già imposta** sui limiti di piano
(`limits.plan`), dove la policy esiste come argomento di `PlanV5::parse`: lì
un piano può solo restringere.

### I limiti strutturali del piano si applicano dopo la deserializzazione

Il solo limite applicato **prima** di costruire un albero JSON è
`max_plan_json_bytes`, sul testo. Nodi, archi, profondità, fan-out, input,
byte di config e byte degli identificatori sono verificati sull'oggetto già
deserializzato.

**Regola:** l'allocazione guidata dal contenuto è limitata dal solo tetto sui
byte del documento, non dai tetti strutturali.
**Ambito:** `PlanV5::parse` e `PlanV6::parse`, quindi ogni ingresso di piano
DAG — v5, v6 e v4 attraverso la migrazione — sia dalla CLI sia dal target
fuzz `plan_v5_parse`.
**Hazard:** un chiamante che abbassa `max_plan_nodes` a dieci ma lascia
`max_plan_json_bytes` al default accetta comunque di allocare fino a 16 MiB di
albero JSON prima del rifiuto. Il consumo resta limitato — il tetto sui byte
c'è ed è applicato per primo — ma non dal limite specifico che credeva di
avere impostato.
**Condizione di rientro:** un `serde::Visitor` che contabilizzi nodi, archi e
byte di config **durante** la deserializzazione e rifiuti al superamento,
verificato da un test che dimostri il rifiuto prima della materializzazione
completa. Finché non esiste, chi vuole un tetto stretto sulle allocazioni di
parsing deve abbassare `max_plan_json_bytes`, che è l'unico che le governa.

### Messaggi delle dipendenze: che cosa è sanificato e che cosa no

La regola «errori senza dati» è imposta **per costruzione** sui messaggi
scritti da questo progetto e sui percorsi che citano valori di cella. Sui
messaggi prodotti dalle dipendenze la copertura è quella, esplicita, che
segue.

**Sanificati** (del messaggio originale non resta nulla; passa un codice o un
contesto scritto qui):

- `arrow-rs` → `PlenoraError::DataMapping` e `ArrowTransportError::Arrow`:
  passa il solo codice della variante (`cast`, `parse`, `schema`, …). I testi
  di arrow citano regolarmente il valore che ha causato il difetto;
- GEOS → `GeosBackendError::Geos`: passa il contesto della chiamata. I
  messaggi di GEOS citano le coordinate del difetto;
- PROJ **sul percorso dati** → `ProjBackendError::Transformation`: passa la
  sola classe del fallimento. `Reprojector::reproject` dà a PROJ le
  coordinate di ogni cella, quindi lì la libreria nativa sta elaborando dati,
  non configurazione;
- panici di `arrow-ipc` al confine IPC: passa la sola **forma** del payload.

**`serde_json`: quattro percorsi, quattro trattamenti diversi.** La libreria
è la stessa, il trattamento no — e classificarli per libreria invece che per
percorso è già stato un errore di questo documento.

| percorso | trattamento | categoria |
|---|---|---|
| contenuto di **cella** (`flatten_json`, analisi JSON) | il fallimento diventa una **causa statica** (`json.invalid_syntax`, `json.root_not_object`), il testo è scartato | diagnostica per riga |
| metadato legacy `geo` **stretto** (`arrow_adapter`) | causa **statica** scritta qui, testo di serde scartato | `invalid_plan` |
| lettura **opportunistica** dello stesso metadato (rimozione del `crs`) | l'errore è ignorato: il metadato resta com'è | nessuna |
| **piano, configurazione, metadati di schema** (CLI, contratti) | il testo originale **resta** | `data_mapping` |

L'ultima riga è l'unica in cui un messaggio di serde attraversa il confine, ed
è deliberata: sono documenti scritti da chi invoca la libreria, e un errore
che ne cita un frammento non rivela nulla che il chiamante non abbia scritto.

**Non sanificati, e per una ragione dichiarata:**

- PROJ **sulle definizioni CRS** → `CrsError::InvalidDefinition`, categoria
  `crs` (non `data_mapping`): il testo riguarda una definizione fornita nel
  piano o nei metadati, cioè configurazione, e senza di esso una definizione
  malformata non sarebbe diagnosticabile. Nella stessa variante finisce anche
  il PROJJSON malformato **prodotto da PROJ**, con il testo di serde;
- `geo` → `ProjBackendError::InvalidInput`/`InvalidOutput` e le varianti
  omologhe dei kernel: `geo::Validation` nomina **ruoli e indici** («anello
  interno numero 2», «coordinate at index 7»), mai i valori. È già la forma
  strutturale che questo progetto usa nei propri messaggi.

**Hazard:** la riga di confine è il *percorso*, non la libreria. Se
`serde_json` venisse usato per deserializzare contenuto di celle su un
percorso che propaga l'errore, o PROJ per interpretare una definizione presa
dai dati, quei due punti diventerebbero fughe. Non c'è oggi un controllo
automatico che tenga la mappa onesta: è una revisione da fare a ogni nuovo
uso di una dipendenza sul percorso dati.
**Condizione di rientro:** adattatori tipizzati per dipendenza **e
operazione**, che espongano solo un codice stabile e un testo controllato,
come già fatto per arrow, GEOS e la trasformazione PROJ.

### Scavenging temporaneo: comanda l'heartbeat, il PID può solo accelerare

Una directory `plenora-*` è rimossa se il suo heartbeat è più vecchio del TTL,
oppure — più in fretta — se l'heartbeat è fermo da oltre cinque minuti **e**
il lock viene da questa macchina **e** il PID registrato non esiste più. Un
heartbeat fresco non è mai toccato, qualunque cosa dica il PID; e su Linux,
dove il PID è davvero interrogabile, **un processo locale vivo blocca anche la
rimozione per TTL**.

**Regola:** il PID non è mai da solo motivo di rimozione, e la scadenza non
prevale su una prova positiva di vita.
**Ambito:** `plenora_engine::temp_store::scavenge_stale_temp_dirs`; la
verifica reale del PID esiste solo su Linux.

**Hazard, in tre parti.**

*Identità di macchina.* L'hostname registrato **non è un'identità**. Immagini
clonate, container e host configurati allo stesso modo condividono lo stesso
nome, quindi l'uguaglianza da sola non prova nulla. Resta il caso di
un'esecuzione remota **sospesa** da oltre cinque minuti su un host omonimo con
radice condivisa: non è distinguibile da un crash locale.

*L'heartbeat non è un timer.* Lo scrive l'executor ai confini di batch. Un'I/O
bloccata a lungo, un'ibernazione o un salto in avanti dell'orologio possono
invecchiarlo oltre il TTL mentre l'esecuzione è viva. Su Linux il PID vivo la
protegge; su **Windows e sugli altri Unix il PID non è verificabile**, quindi
lì la scadenza decide da sola e una directory di un processo bloccato oltre le
24 ore può essere raccolta.

*PID riciclati.* Il veto su TTL usa un PID che il sistema può aver riassegnato
a un processo estraneo: in quel caso la directory resta, contata in
`kept_conservative`. È il verso prudente — si perde spazio, non dati.

*La decisione e la rimozione non sono atomiche.* Lo scavenger classifica,
poi rimuove. La classificazione è ripetuta immediatamente prima della
`remove_dir_all`, con l'orologio riletto, così la finestra non è più l'intera
scansione della radice — ma resta una finestra: fra il secondo controllo e la
rimozione un'esecuzione può rinnovare l'heartbeat, e la directory viene
cancellata comunque. La garanzia «un heartbeat fresco non è mai toccato» è
quindi esatta al momento del controllo, non per l'intera durata della
rimozione.

*L'hostname può cambiare.* Il veto del PID vivo confronta l'hostname corrente
con quello scritto nel lock. Se la macchina viene rinominata mentre
un'esecuzione è in corso, il confronto fallisce e la sua directory torna
cancellabile per TTL, benché il processo sia vivo e locale.

*Un heartbeat che fallisce è tollerato, ma solo per cinque minuti.* La
scrittura del lock può fallire per una ragione transitoria, e fermare per
quello un'esecuzione lunga sarebbe sproporzionato: il singolo fallimento
viene ritentato al batch successivo. Un fallimento **persistente** è invece
un errore esplicito — l'esecuzione si interrompe con categoria `io` al primo
confine di batch dopo cinque minuti senza un heartbeat riuscito. Non è
tolleranza illimitata mascherata da robustezza: oltre quella soglia il
timestamp nel lock smette di avanzare, e superato il TTL un altro avvio
potrebbe raccogliere la directory con dentro lo spill di questa esecuzione.
La soglia è ben sotto il TTL proprio perché l'errore arrivi molto prima del
danno.

**Condizione di rientro:** una lease con identità di macchina e di avvio
verificabile (boot id, namespace) più un heartbeat scritto da un timer
indipendente dai batch; oppure l'imposizione esplicita di uno storage
temporaneo node-local.

### `max_domain_memory_bytes`: un tetto richiesto, non concesso

**La regola.** Un piano `schema_version: 6` può dichiarare
`max_domain_memory_bytes` dentro `limits`: intero positivo rappresentabile in
`u64`, **facoltativo**, e maggiore o uguale al budget governato **effettivo**
— quello dichiarato dal piano, oppure il default pubblicato quando il piano lo
omette. Ratificato in `plenora-contracts` come `Plan Budget 1.0`
(`plenora-plan-budget-v1`, `PLAN-007` … `PLAN-012`).

**L'ambito.** Solo la v6. Un piano v4 o v5 che lo dichiara è **rifiutato**, e
non per un controllo aggiunto: `LimitsOverride` ha `deny_unknown_fields` e non
conosce quel nome. Simmetricamente, un v6 con un nome che la v6 non ha è
rifiutato da `LimitsOverrideV6`. I formati lineari sotto la `4` restano fuori
dal perimetro e invariati.

**Il pericolo che copre.** Due, distinti.

Il primo è **l'assenza scambiata per un permesso**: se un campo mancante
producesse un tetto di default, un piano che non ha mai chiesto isolamento
girerebbe in un dominio dimensionato da qualcun altro, e chi lo ha inviato non
potrebbe distinguere «l'ho chiesto io» da «l'ha scelto qualcuno per me». Per
questo l'assenza significa una cosa sola: **il profilo isolato non è
selezionabile** per quel piano.

Il secondo è **una configurazione che non può riuscire**: un tetto di dominio
sotto il budget che il piano è autorizzato a governare descrive un'esecuzione
che esaurirebbe la memoria per costruzione. Rifiutarla in validazione è più
leggibile che scoprirla come esaurimento a runtime.

Lo zero è rifiutato per la stessa ragione dei due sopra messi insieme: non è
«nessun tetto» — quello si dichiara omettendo il campo — ed è un dominio che
non può allocare nulla.

**Che cosa il valore NON promette.** È il tetto che il piano **richiede**, non
quello che l'host concede. Il tetto effettivo sarà `min(richiesta, policy
dell'host)`, e policy e meccanismo di applicazione stanno fuori dal formato
(`PLAN-014`, `PLAN-015`). Il valore dichiarato non viene riscritto con quello
concesso (`PLAN-016`): lo stesso documento inviato a due host resta lo stesso
documento, con la stessa identità, e la differenza compare nell'esito.

**La condizione di rientro.** Non è un limite da alzare: è un campo del
formato. Cambierebbe se cambiasse la ratifica — per esempio se il tetto
diventasse obbligatorio in v6, o se una v7 lo spostasse. Fino ad allora
restringere o allargare queste regole significa disallinearsi da
`Plan Budget 1.0`, che è la fonte.

### Antenati portati dall'evidenza di pressione di memoria

**La regola.** L'evidenza di `unattributed_memory_pressure` porta al massimo
**otto** antenati del dominio (`MAX_ANTENATI_OSSERVATI`). Il limite è nel
tipo, non nel costruttore: `PressioneDegliAntenati` contiene un array di
lunghezza fissa, quindi la profondità della gerarchia trovata sull'host non
può decidere quanto occupa un errore.

**Il perimetro.** Solo il segnale `Oa` — la pressione registrata dagli
antenati — e solo dentro l'errore. Non limita quanti cgroup possono esistere,
non impedisce di leggerli, e non tocca i quattro segnali del dominio (`Ol`,
`Kl`, `Kh`, `G`), che non sono per-livello.

**Il pericolo che copre, e quello che NON copre.** Sono due protezioni
distinte e vanno tenute separate:

- l'evidenza viaggia in un `Box`, e **quello** impedisce che la dimensione di
  `EvidenzaDiLimite` allarghi `PlenoraError` e con esso ogni `Result` del
  workspace, compresi i milioni che tornano `Ok`. Con il `Box` una gerarchia
  profonda non fa più crescere il cammino felice, e il limite non serve a
  quello;
- il limite di otto impedisce che la **gerarchia dell'host governi** ciò che
  accade sul percorso d'errore: quanto si alloca per costruire l'evidenza,
  quanta ne viaggia, e quanto lungo diventa il messaggio formattato. Senza,
  una profondità che non controlliamo deciderebbe quelle tre quantità.

**Che cosa questo perde, e come lo dice.** Se gli antenati sono più di otto,
l'evidenza ne porta otto e dichiara gli altri in `antenati_oltre_capacita`. La
distinzione che conta resta leggibile: un livello **esistente e non letto** e
un livello **inesistente** non collassano nella stessa forma, né dentro né
oltre la capacità. Un troncamento silenzioso direbbe che la gerarchia finisce
dove invece finisce la nostra vista, e su `Oa` sarebbe grave: il segnale serve
proprio a dire *quale* livello ha esaurito il proprio tetto.

**La condizione di rientro.** Alzare il numero non rompe chi costruisce, e
non per convenzione ma per firma: `PressioneDegliAntenati::nuova` accetta una
**slice** e copia nell'array privato, quindi la capacità non compare in nessun
tipo pubblico. Una prima stesura passava l'array, e lì la promessa era falsa —
`MAX_ANTENATI_OSSERVATI` era nella firma, e cambiarlo sarebbe stata una
rottura.

Serve una gerarchia reale più profonda di otto in cui `Oa` sia risultato
diagnosticamente insufficiente. Otto **non** è una misura: i prototipi non
hanno rilevato la profondità delle gerarchie ospiti, e il campo del
troncamento esiste anche perché quella scelta resti rivedibile.

### Protocollo del worker: i tetti sono del profilo isolato

**La regola.** Ogni frame del protocollo supervisore/worker è un prefisso di
lunghezza `u32` big-endian seguito da JSON UTF-8 compatto. Nessun tetto è
configurabile e nessuno viaggia come valore modificabile: un protocollo
interno con limiti negoziabili è un protocollo con una superficie d'attacco
negoziabile.

I tetti sono di due specie, e vivono in posti diversi perché sono cose
diverse:

- i **limiti elementari** — quanto è lungo un percorso, quanti ingressi,
  quanti esempi — sono costanti scelte, e stanno in
  `plenora_engine::protocollo::limiti`;
- `MAX_PROTOCOL_FRAME_BYTES` è invece **derivato** da quelli con aritmetica
  `checked`, e sta accanto al codice che lo applica, in
  `plenora_engine::protocollo::codifica`. Non è una scelta: è una
  conseguenza, e metterlo fra le scelte inviterebbe a modificarlo
  direttamente.

| Costante | Valore | Che cosa limita |
|---|---|---|
| `MAX_PROTOCOL_FRAME_BYTES` | derivato | byte del payload di un frame, prefisso escluso |
| `MAX_PIANO_CANONICO_BYTES` | 64 MiB | forma canonica del piano dentro l'`Incarico` |
| `MAX_INGRESSI` | 16 | descrittori d'ingresso per `Incarico` |
| `MAX_IDENTIFICATORE_BYTES` | 256 | byte decodificati di un identificatore |
| `MAX_PERCORSO_BYTES` | 4096 | byte decodificati di un percorso |
| `MAX_DIGEST_BYTES` | 64, **derivato** | i caratteri della forma esadecimale di uno SHA-256, cioè `DIGEST_BYTES * 2`. Non è un tetto: la forma pretende esattamente questa lunghezza, e a garantirla è il **tipo** `DigestSha256`, non un controllo |
| `MAX_VERSIONE_BYTES` | 64 | byte decodificati di una versione dichiarata |
| `MAX_CAPABILITY` | 32 | capability dichiarate in `Risposta` |
| `MAX_RISORSE` | 64 | risorse risolte elencate nell'handshake |
| `MAX_BACKEND_DINAMICI` | 16 | backend collegati dinamicamente |
| `MAX_MOTIVO_BYTES` | 1024 | byte decodificati del motivo di un `Annulla` |
| `MAX_MESSAGGIO_BYTES` | 4096 | byte decodificati del messaggio sanificato |
| `MAX_CONTEGGI_DIAGNOSTICA` | 64 | voci di conteggio nella diagnostica di riga |
| `MAX_CHIAVE_CONTEGGIO_BYTES` | 128 | byte decodificati di una chiave di conteggio |
| `MAX_ESEMPI_DIAGNOSTICA` | 32 | esempi portati dalla diagnostica |
| `MAX_ESEMPIO_BYTES` | 512 | byte decodificati di un esempio |
| `MAX_PROGRESSO` | 1024 | quota totale di emissione dei messaggi di progresso |
| `MAX_MESSAGGI_VERSO_WORKER` | 3 | messaggi ammessi verso il worker |
| `MAX_MESSAGGI_VERSO_SUPERVISORE` | 2 + `MAX_PROGRESSO` | messaggi ammessi verso il supervisore |

`MAX_PROTOCOL_FRAME_BYTES` non è un numero scelto: è derivato con aritmetica
`checked` dai tetti qui sopra e da un involucro JSON **contato carattere per
carattere**. È un **maggiorante**, non una misura — nessun frame
strutturalmente valido lo raggiunge, e questo è il verso giusto.

**Il perimetro.** Solo il canale fra supervisore e worker isolato. Questi tetti
non toccano `PlanLimits`, non toccano il confine IPC dei dati, e non
descrivono ciò che il motore accetta in-process.

**Il pericolo che copre.** Il decoder legge byte scritti da un altro processo:
è la superficie d'attacco più diretta del sistema. Due presidi, e sono
distinti:

- il tetto sulla lunghezza si applica **prima di allocare**. Un frame che
  dichiara un gigabyte viene rifiutato con quattro byte in mano, non dopo aver
  riservato lo spazio che si voleva negare. Va detto fin dove arriva questa
  garanzia: `decodifica` riceve una slice, quindi quando viene chiamata i byte
  del payload *qualcuno li ha già letti*. Il presidio utilizzabile contro un
  pipe è `lunghezza_dichiarata`, che decide sui soli quattro byte del prefisso
  e che `decodifica` usa a sua volta — un solo confronto, non due che possono
  divergere. Il lettore che se ne serve è di `PR-5`; fino ad allora la
  garanzia esiste come predicato provato, non come lettura limitata;
- in scrittura il writer **si ferma** appena supera il tetto, invece di
  costruire un buffer illimitato e misurarlo dopo. Misurare dopo significa aver
  già allocato ciò che si voleva rifiutare.

**Che cosa questo NON garantisce, e va detto.** `MAX_PIANO_CANONICO_BYTES` è un
limite di **policy**, non un massimo dimostrato.
`examples/calibra_canonico.rs` misura il rapporto fra testo e forma canonica su
piani costruiti per massimizzarlo e proietta il caso peggiore osservato sul
tetto di `PlanLimits::default()`; il margine risultante è dichiarato dalla
sonda stessa, che esce non-zero se la proiezione supera il limite. Nessun
insieme finito di piani fissa quel rapporto per **tutti** i documenti validi:
il presidio è il writer limitato, non il numero.

**La conseguenza che va detta e non scoperta.** Questi sono tetti del **profilo
isolato**, mentre `max_plan_json_bytes`, `max_inputs` e `max_identifier_bytes`
di `PlanLimits` sono **default ampliabili** dalla policy di chi esegue. Ne
segue che un piano valido sotto una policy più larga può essere **non
isolabile**. Deve ricevere un rifiuto esplicito che lo dica — non un errore di
serializzazione, che direbbe «il messaggio non si scrive» al posto di «questo
piano non si può isolare». La selezione del profilo appartiene a `PR-12`: fino
ad allora la condizione esiste e non ha ancora il suo messaggio.

**Il modulo non è pubblico, e non lo diventa sotto feature.** Il crate `fuzz/`
e la sonda di calibrazione stanno fuori dal crate e non possono raggiungere un
modulo privato; passano da `plenora_engine::interni`, una facciata
**instabile e non-production** che espone un verdetto e una costante, non i
tipi del protocollo. Una prima stesura esponeva `pub mod protocollo` sotto la
stessa feature: era API pubblica a tutti gli effetti quando la feature era
attiva, e il crate `fuzz/` è precisamente un consumatore che la attiva.
«Privato tranne che per chi lo usa» non è privato.

**La condizione di rientro.** Alzare un tetto richiede: la misura che mostra il
caso reale che lo supera, il ricalcolo di `MAX_PROTOCOL_FRAME_BYTES` (che è
derivato, quindi si aggiorna da sé), e la verifica che i sei massimi
serializzati continuino a starci sotto — che è un test, non un controllo a
mano. Quei massimi vanno costruiti con caratteri che JSON **espande** in `\uXXXX`: con stringhe ASCII il frame massimo resta a un sesto della
taglia che il tetto gli concede, e l'espansione degli escape non viene mai
esercitata. Renderli configurabili è fuori discussione finché il canale resta
interno.

### Moduli compilati solo sotto `test` e `internals`

**La regola.** `plenora_engine::verifica` è compilato solo con
`#[cfg(any(test, feature = "internals"))]`;
`plenora_engine::commit_footer::leggi_commit_token` e
`geo_transport::ipc::parse_footer` solo con `#[cfg(test)]`. Non è
un'ottimizzazione: è la dichiarazione che quel codice **non ha ancora un
chiamante di produzione**.

Il `cfg` sul modulo `protocollo` **è caduto con `PR-9`**, e la condizione che lo
reggeva era scritta: serviva un chiamante esterno al modulo. Quel chiamante è il
worker, che si descrive, legge il `Saluto`, giudica l'accordo e risponde — da
codice di produzione, raggiunto dal dispatch della riga di comando. Ciò che
dentro il modulo non ha ancora un chiamante lo dichiara ora **elemento per
elemento**, perché un `cfg` sul modulo intero direbbe che nessuno lo usa, e non
è più vero.

**Il perimetro.** Il modulo `verifica`, che esegue i passi da 3 a 8-bis della
sequenza di [`isolamento.md`](isolamento.md), la sola funzione di lettura del
token dal footer, e la forma breve di `parse_footer` (da quando la convalida
estrae anche un custom metadata, la produzione passa tutta per
`parse_footer_estraendo`). `commit_footer::scrivi_commit_token` è invece
incondizionato, perché il writer in-process lo chiama davvero (con `None`).

Dentro `protocollo` il perimetro non è più il modulo ma un **elenco**, e
l'elenco l'ha prodotto `-D dead-code` su Linux, non una lettura a mano:

- il lato supervisore dell'handshake, sotto `any(test, internals)`:
  `AtteseSupervisore`, `SupervisoreInAttesa` con i suoi metodi,
  `confronta_capability` e `HandshakeAccettato`. Non li raggiungono soltanto i
  casi: li guida anche il percorso di qualificazione end-to-end
  (`isolamento::prova`), che sotto `internals` conduce un worker reale — ed è la
  ragione per cui il `cfg` non è il solo `test`. `HandshakeAccettato` lo
  **nomina** inoltre `isolamento::macchina::produttori`.

  Di produzione non diventano con questo: il supervisore che `PR-8` costruisce
  riceve un accordo **già concluso**, quindi non passa da lì, e il chiamante di
  produzione arriva con `PR-12`, quando una policy sceglie l'esecuzione isolata;
- gli **inventari generati dalle macro** dei messaggi, `TUTTE` e `NOMI`, sotto
  `cfg(test)`. Esistono per essere enumerati dai casi che attraversano ogni
  variante; la produzione converte una variante per volta e non ha bisogno
  dell'elenco. Né il fuzzer né la facciata `interni` li nominano, e un `cfg` più
  largo dichiarerebbe un chiamante che non esiste;
- `WorkerAccordato::commit_token`, sotto `cfg(test)`: è una **seconda porta** sul
  token, che chi esegue riceve invece da `ricevi_incarico`, insieme
  all'incarico e nello stesso momento.

Fuori dal protocollo, la stessa regola tiene tre elementi dell'isolamento:

- `SorgenteTerminabile::nuova` e `SorgenteTerminabile::con_passo`, sotto
  `any(test, internals)`. Creano il freno **insieme** alla sorgente, ed è la
  forma che serve al supervisore; il worker ha bisogno del contrario — il freno
  prima, perché il thread che legge nasce dopo — e passa da `con_interruttore`,
  che infatti è incondizionata;
- `FiglioVivo::attendi_la_fine`, sotto `any(test, internals)`. È la porta che
  **aspetta senza segnalare**, e la usa il percorso di qualificazione: la
  produzione, oggi, non ha un cammino in cui concedere cortesia a un figlio —
  quando ne avrà uno, con `PR-12`, il `cfg` cadrà da sé sotto il gate;
- `isolamento::prova`, il percorso end-to-end per intero, sotto
  `all(target_os = "linux", any(test, internals))`. Non è un pezzo del
  programma: è il modo in cui si prova che i pezzi si parlino, e guida il lato
  supervisore dell'handshake, che di produzione non è ancora.

Un elemento del filo che avesse un chiamante solo di prova senza dirlo
lascerebbe l'avviso a qualcun altro: è la ragione per cui l'elenco si aggiorna
insieme al gate, e non dopo.

Il perimetro include anche il **supporto esclusivo del verificatore**, che ha
lo stesso stato — nessun chiamante di produzione fino a `PR-10` — e che senza
`cfg` sposterebbe altrove gli avvisi che il `cfg` doveva chiudere:
`commit_footer::interpreta_commit_token`,
`ipc_boundary::ArtefattoConvalidato` con i suoi metodi, e
`ipc_boundary::convalida_artefatto`. Un `cfg` sul modulo che lascia scoperto
ciò che solo quel modulo usa non è un perimetro, è una linea tracciata a
metà.

`Esadecimale32::dai_byte` **ne è uscita** con `PR-9`, e per la ragione scritta
nella sua documentazione: serve a chi il valore lo *calcola* invece di
riceverlo come testo. Il worker calcola lo SHA-256 dell'artefatto che ha
appena scritto e lo deve dichiarare nell'`Esito`; senza quella funzione
formatterebbe i byte a mano, e nel crate ci sarebbero due grafie esadecimali al
posto di una.

Sono uscite dal perimetro, sempre con `PR-9` e sempre perché hanno acquistato un
chiamante di produzione, anche `canale::VARIABILE_DEL_CANALE` con
`in_variabile`, `EstremiDelWorker` con `numeri` e `rendi_ereditabili`,
`accerta_monothread`, `accerta_topologia` e `apri`. Non è una lettura a mano:
sono esattamente gli avvisi che spariscono confrontando `cargo clippy
--workspace --all-targets` sull'albero e sulla base. Il verso opposto — un
elemento che *entra* nel perimetro — resta quello che l'elenco qui sopra
registra elemento per elemento.

**Perché due `cfg` diversi e non uno.** `protocollo` e `verifica` portano anche
l'arm `internals` perché la facciata `interni` è il modo in cui il fuzzer li
raggiunge. `commit_footer::leggi_commit_token` e `ipc::parse_footer` sono
`pub(crate)`: la feature non porterebbe loro nessun chiamante in più,
porterebbe un `dead_code` nella build che la abilita. Un `cfg` più largo del
necessario dichiara una condizione falsa, ed è il modo educato di riaprire
l'avviso che il `cfg` esisteva per chiudere.

**Perché non un `allow(dead_code)`.** Sono due modi di trattare lo stesso
fatto, e non sono equivalenti. Un `allow` **zittisce** l'avviso e lascia il
codice nella build: il lettore vede un attributo e deve fidarsi che qualcuno
lo tolga. Un `cfg` **dichiara** la condizione: il codice non entra nel
binario di produzione finché la condizione non cambia, e la condizione è
scritta. Se un giorno qualcuno aggiunge il chiamante e dimentica il `cfg`, il
compilatore glielo dice; se dimentica un `allow`, non glielo dice nessuno.

**Il pericolo che copre.** Codice non raggiungibile che entra comunque nel
binario distribuito, e che nessuno esercita — la definizione di superficie che
esiste senza che nessuno se ne occupi.

**Che cosa questo perde, e va detto.** Il modulo non è compilato dalla build
di produzione, quindi un errore che si manifestasse solo lì non verrebbe visto
da `cargo build`. È coperto: `cargo test` e `cargo clippy --all-targets` lo
compilano entrambi, e la CI li esegue tutti e due.

**La condizione di rientro.** Quella su `protocollo` è **soddisfatta**, e non
lo era prima: non con `PR-5`, perché l'handshake che `PR-5` aggiunge sta
*dentro* `protocollo` e ne consuma i messaggi senza esserne un chiamante; e non
con `PR-8`, che porta il ciclo di vita del supervisore ma lo compila sotto
`internals`, quindi non gli dà un chiamante di produzione. È `PR-9`, col worker
reale nell'eseguibile distribuito, a soddisfarla — verificato togliendo il `cfg`
e leggendo l'output del gate, non dedotto: i diciassette elementi rimasti
scoperti sono stati dichiarati uno per uno o collegati.

I `cfg` per elemento cadono a loro volta quando il chiamante arriva: quelli del
lato supervisore con `PR-12`, gli inventari quando la produzione avrà una
ragione per enumerare le varianti — e finché non l'ha, non gliene si inventa
una.

Il `cfg` su `verifica` sparisce con **`PR-10`**, che è la PR della sequenza di
verifica e publish: è lì che il verificatore acquista un chiamante di
produzione, non con `PR-8`, che porta il ciclo di vita del supervisore e un
worker fittizio.

Il `cfg` su `leggi_commit_token` sparisce anch'esso con **`PR-10`**, e non con
`PR-6` come questo registro prevedeva: il verificatore deve riferire framing,
token, digest e consegna ad Arrow **a un solo handle**, mentre quella funzione
fa una traversata propria — chiamarla significherebbe convalidare due volte,
con una finestra in mezzo. Il verificatore estrae quindi il token durante la
**propria** traversata, e condivide con quella funzione la sola parte che
potrebbe divergere: l'interpretazione del testo trovato, in
`interpreta_commit_token`.

Il `cfg` su `parse_footer` sparisce il giorno che un percorso di produzione
torni a volere i soli blocchi; finché non esiste, la funzione è scaffolding dei
test e sta scritto che lo è.

**Il dominio di isolamento ha lo stesso perimetro, e due condizioni di rientro
invece di una.** `plenora_engine::isolamento` sta sotto
`#[cfg(any(test, feature = "internals"))]` come `protocollo` e `verifica`, e la
sua visibilità cade quando esiste un supervisore che lo chiama in produzione,
cioè con **`PR-8`**. Il `cfg` di piattaforma sui sottomoduli che toccano il
kernel resta invece finché non esiste un secondo dominio supportato. Sono
indipendenti, e vanno scritte separate: se fossero una condizione sola, la
rimozione della prima porterebbe via anche la seconda.

L'orchestrazione e i suoi casi sono **multipiattaforma** — provano la
procedura, non l'ambiente — quindi `cfg(target_os = "linux")` sta sui soli
sottomoduli. Un `cfg` di piattaforma sul modulo intero renderebbe i casi
deterministici non compilati altrove, cioè verdi per assenza.

### Il profilo isolato non descrive l'ambiente `proj-backend`

**La regola.** Con la feature `proj-backend` attiva, il worker **rifiuta prima
dell'handshake**: `protocollo::descrizione::di_questa_build` rende
`PlenoraError::InvalidConfiguration` e nessun `Saluto` viene letto. Il profilo
isolato di `PR-9` è quello **senza** backend CRS, e l'`Ambiente` che dichiara ha
un insieme di risorse realmente vuoto: `acquisizione_dinamica = false`,
`risorse = []`, `backend_dinamici = []`, e il digest dell'insieme è quello
canonico, versionato e domain-separated dell'insieme vuoto
(`plenora:insieme-risorse:v1`).

Il rifiuto è **tipizzato**, non un messaggio: la variante dice che è una
condizione della *build* e non dell'esecuzione, perché chi la legge deve poter
cambiare build e non riprovare.

**Perché non si descrive.** Perché descrivere un ambiente vuol dire elencarne
le risorse e digerirle, e ciò si può fare solo se la radice da cui provengono è
esclusiva, immutabile e nota. Con PROJ non lo è: l'API che fissa i percorsi li
**aggiunge** a quelli esistenti invece di sostituirli, e la cache delle griglie
è attiva per default. Non c'è quindi un insieme di cui si possa dire «è tutto, e
non cambia».

**Il pericolo che copre.** Un digest ricavato dal solo searchpath sarebbe una
**falsa garanzia**: due macchine con lo stesso searchpath e contenuti diversi lo
condividerebbero, e l'handshake direbbe «stesso ambiente» su due ambienti
diversi — cioè accetterebbe un worker che riproietta diversamente dal
supervisore. È l'esito peggiore dei tre possibili, perché non somiglia a un
errore: somiglia a un accordo.

**Che cosa questo perde, e va detto.** L'esecuzione isolata non è disponibile
per le build con PROJ, che sono quelle che riproiettano davvero. Il limite è
quindi sul profilo utile, non su un caso marginale, e per questo il rientro è
scritto invece di essere rinviato.

**La condizione di rientro.** Il profilo PROJ rientra quando **un unico
provider** — non cinque punti che si coordinano — soddisfa tutte e cinque le
condizioni:

1. fissa percorsi **esclusivi e in sola lettura**, sostituendoli e non
   aggiungendoli a quelli dell'ambiente;
2. **disabilita rete e cache**, perché entrambe possono introdurre una risorsa
   dopo che il digest è stato calcolato;
3. **inventaria e digerisce tutte** le risorse disponibili, non un
   sottoinsieme scelto dal chiamante;
4. **identifica versione e percorsi risolti**, così che due lati che
   convengono convengano su qualcosa di verificabile;
5. **alimenta lo stesso resolver realmente usato nell'esecuzione**, perché una
   descrizione che riguardasse un resolver diverso da quello che risolve
   sarebbe vera e inutile.

Un provider che ne soddisfacesse quattro non basta, e la quinta è quella che si
dimentica: è ciò che tiene insieme la descrizione e il comportamento. La scelta
del resolver esce da un **selettore tipizzato** unico
(`plenora_engine::risolutore::Risolutore`), che rende insieme la funzione che
risolve e l'identità che la nomina, con un caso che pretende che le due
concordino; senza quel selettore le cinque condizioni potrebbero essere
soddisfatte e la descrizione riguardare comunque un'altra implementazione.

### L'apertura dell'artefatto temporaneo: quali generi sono dell'incarico

**La regola.** `executor::output::non_apribile` classifica il rifiuto
dell'apertura esclusiva leggendo il **genere** dell'errore, non il suo testo, e
l'elenco dei generi che parlano dell'incarico è una tabella —
`GENERI_DELL_INCARICO` — non una catena di rami.

| genere | fase | che cosa dice |
|---|---|---|
| `AlreadyExists` | `Commit` | il percorso è già occupato |
| `NotFound` | `Probe` | la directory che lo conterrebbe non esiste |
| `InvalidInput` | `Probe` | il percorso non è di una forma che il sistema accetti (un byte NUL, per dire) |
| `NotADirectory` | `Probe` | un componente intermedio è un file |
| `IsADirectory` | `Commit` | il percorso nomina una directory dove serve un file |

Tutto il resto — permessi, disco pieno, filesystem in sola lettura — resta `Io`
con fase `Write`: è l'ambiente che risponde di no, e correggere l'incarico non
servirebbe.

**Perché i tre generi di forma ci sono.** Il protocollo limita la **lunghezza**
del percorso, non la sua forma: un percorso con un NUL, uno che passa per un
file, uno che nomina una directory arrivano tutti all'apertura. Non è teoria —
un caso li produce con un'apertura vera, con gli stessi flag del percorso di
scrittura, e pretende che finiscano fra i difetti dell'incarico.

**Perché la ricaduta è il verso pericoloso.** Un difetto dell'incarico
classificato `Io` manda chi legge a cercare un permesso o dello spazio che non
c'entrano, mentre il percorso sbagliato resta dov'è: l'errore non si vede. Il
verso opposto — un guasto d'ambiente chiamato `InvalidPlan` — è altrettanto
falso ma si scopre subito, perché l'incarico che si va a controllare risulta
corretto. Il mutante `mut-48` toglie `InvalidInput` dalla tabella e viene ucciso
dal caso che la scorre.

**La tabella dice come si classifica un genere, non quale genere una forma
produca.** Quello lo decide il sistema, e i sistemi non concordano. Misurato con
gli stessi flag del percorso di scrittura:

| forma del percorso | Linux | Windows |
|---|---|---|
| un componente intermedio è un file | `NotADirectory` | `NotFound` |
| il percorso nomina una directory | `AlreadyExists` | `PermissionDenied` |
| il percorso contiene un NUL | `InvalidInput` | `InvalidInput` |

Le prime e le terze righe non cambiano niente: i due generi sono entrambi in
tabella, e la classificazione resta `InvalidPlan`.

### Deviazione: su Windows una directory esistente è diagnosticata come ambiente

**Ambito.** Solo Windows, e solo l'apertura esclusiva dell'artefatto temporaneo.

**Il fatto.** Un percorso che nomina una directory esistente arriva come
`PermissionDenied`, che **non è** fra i generi dell'incarico: viene quindi
classificato `Io`, fase `Write`, come un qualunque rifiuto dell'ambiente. Su
Linux lo stesso percorso arriva come `AlreadyExists` ed è `InvalidPlan`, fase
`Commit`.

**Perché la tabella non si allarga.** Perché `PermissionDenied` su Windows è
indistinguibile da un permesso che manca davvero. Ammetterlo fra i generi
dell'incarico direbbe «correggi il percorso» anche a chi ha un problema di
permessi, e quello è il verso in cui l'errore non si vede: chi legge va a
cambiare un percorso che è giusto. Fra due diagnosi imprecise si tiene la
**conservativa** — meno precisa, mai falsa.

**Il rischio che resta.** Su Windows, un incarico che indica una directory come
percorso temporaneo riceve una diagnosi che parla di permessi. È corretta come
categoria — l'apertura è stata negata — ma non indica il rimedio vero. È il
prezzo dichiarato della scelta conservativa.

**Nessun controllo preventivo.** Non si aggiunge un `is_dir()` prima
dell'apertura: separare la domanda dall'atto aprirebbe una finestra fra i due —
il percorso può cambiare in mezzo — e trasformerebbe una diagnosi imprecisa in
una decisione sbagliata presa su uno stato che non c'è più. Il `create_new` è
atomico e resta l'unico punto in cui si guarda.

**Condizione di rientro.** Una distinzione **atomica** che separi «è una
directory» da «non hai il permesso» senza una seconda interrogazione del
filesystem — per esempio un codice d'errore nativo più fine, letto dallo stesso
tentativo. Fino ad allora la deviazione resta, ed è provata: due casi
pretendono il **genere reale** su ciascuna piattaforma, così che una piattaforma
che cambiasse risposta faccia diventare rossi i casi invece di adattarsi in
silenzio.

### Il ritardo di ritentativo può non entrare sul filo, e allora si rifiuta

**La regola.** `RetryDisposition::After` porta una `Duration`, che conta i
millisecondi in `u128`; sul filo il ritardo è un `u64`.
`protocollo::assi::ritentativo_sul_filo` **rifiuta** il valore che non ci entra,
riportandolo come `Internal`, invece di saturare all'estremo.

**Perché non si satura.** Perché saturando `Duration::from_millis(u64::MAX)` e
qualunque durata più lunga arriverebbero sul filo come lo **stesso** valore: due
domini distinti, un solo messaggio, e chi legge senza modo di sapere quale sia
passato. È una perdita che non lascia traccia — la sola specie che nessun
controllo a valle può riprendere — ed è la ragione per cui qui la conversione è
fallibile e non totale.

Che una durata simile non nasca da nessuna politica reale non cambia la forma
del controllo: «non può accadere» è un ragionamento, non un controllo. Vale la
regola generale del progetto, che un'invariante interna si verifica in modo
fallibile e si riporta come `Internal`.

**Che cosa succede quando il rifiuto scatta.** Chi lo incontra sta già
riportando un guasto e non ha un secondo canale su cui riportare il guasto del
riporto: `protocollo::assi::errore_dichiarabile` manda allora un errore
**proprio** — categoria `Internal`, ritentativo `Never`, messaggio che porta sia
la ragione del rifiuto sia il testo dell'errore che si stava riportando. Fase,
effetto e posizione restano quelli osservati: il rifiuto riguarda un asse solo.

**Oggi il ramo non si raggiunge, ed è dichiarato.** Nessuna variante di
`PlenoraError` produce `After` — `plenora-core` lo scrive, perché non ci sono
sorgenti di backoff tipizzate — quindi la guardia esiste per il giorno in cui
una sorgente nascesse. Ciò che la guardia *dice* è comunque provato: la
composizione del messaggio è una funzione a due parametri, e i casi la guardano
direttamente. Il mutante `mut-47` rimette la saturazione e viene ucciso dal caso
del millisecondo oltre il massimo.

### Il filo porta un esito solo

**La regola.** Il worker manda **un** `Esito`, e l'`Esito` ha una variante sola
per volta. Quando un cammino produce due fatti — un panico del lavoro *insieme*
a una violazione del canale di controllo — il filo ne porta uno, e del secondo
sopravvive **l'esistenza, non l'identità**.

Va detto esattamente, perché la formulazione facile è falsa. Il worker esce con
il secondo errore, quindi il codice d'uscita non è zero; ma il supervisore
osserva **il codice d'uscita**, non quel valore tipizzato né il suo motivo. Chi
legge sa che qualcosa d'altro è andato storto — un'uscita non nulla dopo un
`Panic` dichiarato lo dice — e non sa *che cosa*. Il messaggio esiste, e finisce
nell'involucro d'errore su `stdout` del worker, che non attraversa il confine:
la diagnostica di riga del worker non è osservata dal supervisore
([§](#la-diagnostica-di-riga-non-attraversa-il-confine-del-worker)).

**Quale dei due va sul filo.** Il panico. È ciò che il supervisore non potrebbe
ricostruire da fuori: un processo che muore di panico e uno ucciso si vedono
uguali dallo stato terminale, mentre un guasto del canale il supervisore lo
vede da sé, perché è il suo canale.

**Perché non si fondono in un messaggio.** Perché `Panic` porta la sola *forma*
del payload — è un enum chiuso di tre valori — e non ha un campo di testo in cui
infilare un secondo motivo. Aggiungerne uno vorrebbe dire aprire una porta per
cui il contenuto di un panico può uscire, che è esattamente ciò che quella
variante esiste per impedire. Quando invece i due fatti sono **due errori**,
uno dei due è il contesto dell'altro e viaggiano insieme: un `ErroreSulFilo` ha
un messaggio che li può portare entrambi.

**Il pericolo che copre.** Che il secondo fatto sparisca **del tutto**. La
sequenza di [`isolamento.md`](isolamento.md) guarda due cose — lo stato
terminale (passo 1) e l'esito dichiarato (passo 2) — e senza questa regola un
panico dichiarato conviverebbe con un'uscita zero, cioè con l'affermazione che
per il resto è andato tutto bene.

**Che cosa questo perde, e va detto per intero.** L'identità del secondo fatto.
Il supervisore sa *che* c'è stato, dal codice d'uscita, e non *quale*: un guasto
del canale, un messaggio fuori sequenza e un lettore andato in panico si vedono
uguali da lì. Non è «lo trova nello stato terminale»: è «lo stato terminale dice
che ce n'è uno».

**La condizione di rientro.** Il protocollo trasporta il secondo fatto: un
`Esito` che porti un elenco invece di una variante sola, oppure un campo
accanto al `Panic` che nomini la classe del difetto concorrente senza portarne
il contenuto. Non è una modifica locale — cambia la forma del messaggio, quindi
la versione del protocollo — e finché non c'è, l'affermazione che si può fare è
soltanto quella scritta qui: dell'altro fatto sopravvive l'esistenza.

### Il worker non osserva il completamento per nodo

**La regola.** Il campo `nodi_completati` del `Progresso` che il worker manda
vale **sempre zero**. Righe e batch sono reali, cumulativi ed esatti; il terzo
campo no, e questa riga è ciò che impedisce di leggerlo come se lo fosse.

**Perché.** Perché l'esecuzione è uno stream: i nodi non finiscono uno dopo
l'altro, restano attivi finché l'ultimo batch non è passato. Le metriche per
nodo esistono dal primo istante — `ExecState::new` le crea tutte in una volta,
una per kernel di ogni segmento — quindi contarle direbbe **quanti nodi ha il
piano**, non quanti ne hanno finito il lavoro.

**Perché zero e non quel numero.** Perché un progresso che parte al massimo e
non si muove è peggio di zero: sembra un'informazione. Zero dice ciò che si
osserva, e ciò che si osserva è niente.

**Il pericolo che copre.** Che il supervisore, o un domani un'interfaccia,
costruisca una percentuale di avanzamento su un numero inventato — e la mostri
a chi decide se aspettare o interrompere.

**La condizione di rientro.** Quando l'executor osserva il completamento per
nodo. Non è una lettura di ciò che c'è: è una contabilità nuova sul percorso
caldo, e va misurata prima di aggiungerla — un contatore per batch su ogni
kernel è esattamente il genere di costo che non si nota finché non è in
produzione.

### Isolamento Linux: quattro deviazioni dello spawner

Sono scelte che si allontanano dalla forma ovvia, e ciascuna ha una condizione
di rientro propria. Il contesto è la sequenza di
[`isolamento.md`](isolamento.md#9-bis-preflight-del-dominio-scrivere-non-e-configurare).

**1. Sui descrittori si verifica e si rifiuta, non si chiude.** Un descrittore
già aperto in scrittura sul filesystem del control plane sopravvive al cambio
d'identità: il controllo dei permessi avviene all'apertura, non a ogni
scrittura. La risposta ovvia sarebbe chiuderlo. Chiudere un descrittore
ereditato per numero richiede però di costruirne un proprietario da un intero
grezzo, e ogni via per farlo è `unsafe`, che questo progetto non ammette.

Rifiutare è fail-closed e non richiede niente: un ambiente che ci passa un
descrittore sul control plane non è un ambiente in cui possiamo isolare, e
chiuderlo di nascosto nasconderebbe che qualcuno ce lo ha dato. *Rientro:* una
via sicura per chiudere un descrittore ereditato, o una decisione esplicita sul
perimetro `unsafe`.

**2. Lo spawner deve essere monothread, e lo accerta.**
`rustix::thread::{set_thread_groups, set_thread_res_gid, set_thread_res_uid}`
sono i syscall **per-thread**: cambiano le credenziali del solo thread
chiamante, non del processo. Il wrapper di glibc le propaga a tutti i thread
con un segnale; queste no.

In un processo monothread la differenza non esiste — c'è un thread solo, e la
`exec` conserva le credenziali del chiamante uccidendo gli altri. In un
processo multithread è un buco: gli altri thread restano privilegiati, e uno di
essi può fare ciò che al thread spogliato è vietato. Da qui il primo passo
della sequenza, che conta `/proc/self/task` e rifiuta se i task non sono
esattamente uno, e il divieto: **queste API valgono solo lì**. *Rientro:* una
API sicura che cambi le credenziali del processo intero.

**3. Il passo 7 rilegge le credenziali, non l'identità intera.** Nella finestra
fra il cambio d'identità e la `exec` il kernel azzera *dumpable* e passa
`/proc/<pid>` a root: `ns` non è più attraversabile dal processo stesso.
Namespace e descrittori si portano avanti dalla lettura del passo 4, ed è
lecito perché in mezzo stanno solo `prctl` e le `setres*id`, che non aprono
descrittori e non cambiano namespace.

Riaprire l'accesso richiederebbe di rimettere *dumpable*, che è anche ciò che
permette a un altro processo dello stesso uid di fare `ptrace` su questo:
guadagnare uno specchio al prezzo di una porta. *Rientro:* nessuno previsto; la
misura sta nel modo `finestra` di `scripts/verifica_isolamento_linux.sh`, e un
kernel che concedesse la lettura anche lì renderebbe la scelta non obbligata,
non sbagliata.

**4. I due estremi del canale si riaprono, e gli ereditati restano aperti.** Il
worker riceve i suoi due estremi come **numeri**, e un numero non si adotta
senza `unsafe` — `OwnedFd::from_raw_fd` e `BorrowedFd::borrow_raw` lo sono
entrambi. Ciò che si può fare senza è aprirne di nuovi attraverso
`/proc/self/fd/<n>`, che rende descrittori posseduti e nati `CLOEXEC`.

I due ereditati restano però aperti, e **non** sono `CLOEXEC`: il supervisore
gliel'ha tolto apposta per farglieli ereditare. È misurato, non dedotto — dopo
che il descrittore riaperto è caduto, il numero originale nomina ancora la
stessa pipe.

**La conseguenza è una non-garanzia.** «Il worker non avvia altri processi» è
un'**invariante operativa**, non qualcosa che il codice impedisce: se un giorno
il worker eseguisse qualcosa, se li porterebbe dietro, e un estraneo che tenesse
aperto l'altro capo impedirebbe per sempre l'EOF del canale. Va detto qui perché
non si deduca dal fatto che i descrittori riaperti sono puliti che lo siano
anche gli altri. *Rientro:* lo stesso della deviazione 1 — una via sicura per
chiudere un descrittore ereditato, o una decisione esplicita sul perimetro
`unsafe`.

**A quale condizione questa non-garanzia è accettabile.** Non da sola: una
pipe trattenuta da un discendente deve poter produrre *un ritardo*, mai *un
risultato sbagliato*. La condizione sta nella macchina a stati del supervisore,
ed è vincolante:

1. **`Esito` da solo non autorizza il successo.** Un messaggio che dice «ho
   finito» è un'affermazione del worker, e un'affermazione non è una prova. Da
   sola, autorizzerebbe a pubblicare mentre qualcosa è ancora vivo.
2. **Servono anche le altre tre.** La morte del worker *e la sua raccolta* —
   che sono due cose, perché un processo raccolto male resta zombie; l'**EOF**
   del canale, che è ciò che dice che nessuno tiene più l'altro capo; e la
   **quiescenza del dominio**, che è ciò che dice che nel cgroup non è rimasto
   nulla. Nessuna delle quattro sostituisce le altre.
3. **Un discendente che trattiene la pipe porta a timeout o incompletezza, mai
   a pubblicazione.** È il modo in cui la non-garanzia si trasforma in un esito
   dichiarato invece che in un danno silenzioso: l'EOF non arriva, il tempo
   finisce, e l'esito dice che non si è potuto concludere — non che si è
   concluso bene.

### La proprietà del figlio non si scarica su una riga di rapporto

Fra lo `spawn` e la consegna al chiamante il figlio sta sotto una **guardia**, e
ogni uscita voluta o lo passa a qualcuno o lo chiude: le porte si chiamano per
ciò che fanno — consegna, attesa, chiusura, arresto — perché un elenco numerato
mentirebbe alla prima porta aggiunta. Nessuna di esse però garantisce di
riuscire: un processo che non si lascia raccogliere entro il limite **esiste
ancora**, e qualcuno deve restarne responsabile.

La tentazione è una porta che rinuncia e prosegue — pid nel rapporto, difetto
accanto, avanti. Si legge come diligenza ed è una perdita: la riga
descrive un processo vivo, e poi lo lascia vivo. Nessuno lo aspetta, nessuno lo
raccoglie, e il supervisore intanto dichiara di aver finito. **Quella porta non
esiste**, e non deve esistere: sarebbe quella che tutti userebbero.

Le vie sono quindi due, e nessuna delle due è silenziosa:

1. **la guardia risale** a chi può ancora riprovare. La conduzione la porta fuori
   nel proprio contorno, insieme al difetto che dice perché: chi ha chiamato ha
   in mano un processo vivo, e la scelta — riprovare, farla risalire ancora,
   fermarsi — è sua. Se la lascia cadere senza deciderla, la sentinella `Drop` è
   ancora armata: la proprietà non si perde mai in silenzio, si perde con un
   abort;
2. **ci si arrende**, quando sopra non c'è nessuno. È il caso dello spawner: ciò
   che quella funzione rende è un errore tipizzato, e un errore non tiene un
   processo — attraversa i confini, viene convertito, e finisce in una superficie
   pubblica dove una guardia non ha posto. Allora si ferma il processo dicendo
   che cosa è successo, con una riga **diversa** da quella della sentinella: la
   sentinella dice «sfuggito» e manda a cercare un cammino che non passa da
   nessuna porta; la resa dice «non si è lasciato raccogliere, e nessuno può
   riprovare» e manda a guardare il figlio.

**La resa manda la terminazione, e riporta che cosa ha potuto fare.** `abort`
ferma **noi**, non il figlio: un processo lasciato cadere non muore, passa al
reaper del sistema e sopravvive al supervisore che dichiarava di fermarsi per non
lasciarlo vivo. La resa tenta quindi esplicitamente la terminazione prima di
fermarsi. Lo fa anche la sentinella, per la stessa ragione.

**Nessuna delle risposte dice «terminato».** Mandare la terminazione non è
osservare l'uscita: nel cammino ordinario `termina()` è seguita da
`prova_a_raccogliere` proprio perché la prima non basta. Un `SIGKILL` accettato
dice che il segnale è partito, non che il processo sia finito — uno in attesa
ininterrompibile lo riceve e resta finché la chiamata di sistema non ritorna. In
un log l'affermazione più forte del vero è peggio del silenzio: chi legge
smetterebbe di cercare un processo che c'è ancora. Le tre risposte sono quindi:

| risposta di `termina()` | ciò che si riporta |
|---|---|
| `Ok(())` | segnale di terminazione inviato; uscita non osservata |
| `InvalidInput` | processo non più terminabile; uscita non osservata |
| altro errore | segnale non inviato (*motivo*); può restare vivo |

Nemmeno `InvalidInput` autorizza «già uscito»: il contratto dice «non più
terminabile», che è compatibile con un figlio finito ma non lo prova.

Non si raccoglie, invece, e non è una dimenticanza: dopo l'`abort` non c'è più
nessuno che possa aspettare. Un figlio non raccolto passa al reaper del sistema,
che se ne occupa; un figlio a cui **non** è arrivato niente è l'altro esito, ed è
quello che la riga deve nominare.

*Rientro:* la seconda via scompare quando lo spawner avrà sopra di sé un
responsabile del ciclo di vita capace di **tenere una guardia** — cioè un
chiamante che riceva `FiglioVivo` invece di un errore tipizzato. È una condizione
tecnica, non una data: vale quando è soddisfatta, da chiunque la soddisfi.

### Chi rinuncia a una nascita parziale chiude il dominio, e ne osserva la quiescenza

Quando un produttore non nasce — il sistema rifiuta un thread — il tentativo non
è «non cominciato»: il worker **esiste già**, può avere discendenti, e il primo
produttore può avere già accodato. Chi si ritira fa quindi tutto ciò che fa una
conduzione completa, tranne classificare: chiude il dominio, raccoglie il figlio,
drena, e riporta.

Chiudere il dominio non è però chiederne la chiusura. `cgroup.kill` è
**asincrono**: la scrittura torna, e i processi muoiono dopo. Una rinuncia che si
fermasse alla chiamata riporterebbe «ho chiesto» lasciando credere «è successo»,
e i contatori di un dominio ancora abitato non sono un'osservazione. Si guarda
quindi, fino all'attesa della quiescenza, e si accoda ciò che si è visto: la
quiescenza se arriva, l'impossibilità se l'osservazione non riesce, e **niente**
se il tempo finisce — perché «non si è svuotato entro» non è un fatto sul
dominio, è l'assenza del fatto atteso, e la barriera lo tratta come tale. Il
motivo va accanto, fra i difetti.

Perché ci sia qualcuno che guarda, l'osservatore **torna indietro** dalla nascita
mancata del sorvegliante: `Builder::spawn` lascia cadere la chiusura con tutto
ciò che ha catturato, quindi l'osservatore viaggia in una cella condivisa e chi
resta fuori lo ritrova lì. *Rientro:* nessuno previsto.

### I quattro tempi del supervisore

Sono il margine di cortesia, l'attesa della quiescenza, il tetto del drenaggio e
l'arresto del lettore. Tutti e quattro sono limiti **governati**: costanti
nominate nel codice, motivate lì, e registrate qui con regola, perimetro e
condizione di rientro.

Nessuno di essi è una stima di quanto ci vuole. I primi tre sono il punto oltre
il quale continuare ad aspettare smette di essere un'attesa; il quarto non è
un'attesa affatto, ma il passo con cui si guarda l'interruttore.

**1. Il margine di cortesia dopo la decisione di chiudere: 2 secondi**
(`MARGINE_DI_CORTESIA`). Fra la decisione di chiudere — un tempo scaduto, una
cancellazione — e la quiescenza del dominio c'è del lavoro vero: il segnale
arriva, i processi muoiono, il cgroup si svuota. Chiudere a zero riporterebbe
«non quiescente» su domini che lo diventano un istante dopo, e quella riga
manderebbe a cercare un residuo che non c'è.

Non è però un'attesa: se il dominio non si svuota, non si svuoterà guardandolo
più a lungo — c'è qualcosa che non muore, ed è un fatto da riportare. Il margine
separa «ci ha messo un momento» da «non è successo». *Rientro:* nessuno
previsto.

**2. L'attesa della quiescenza dopo la forzatura: 500 millisecondi**
(`ATTESA_DELLA_QUIESCENZA`). Non è il margine di cortesia con un altro nome: il
margine si concede **prima** di forzare, a un worker che può ancora scegliere
come chiudere; questa è il ritardo fra `cgroup.kill` e il suo effetto, e non è
concessa a nessuno.

Serve perché l'evidenza di un dominio non ancora vuoto è la fotografia di
qualcosa che si muove. Il prototipo lo ha misurato: al ritorno della `wait`
l'evidenza di OOM valeva zero, e duecento millisecondi dopo valeva uno. Senza
questa attesa la stessa esecuzione diventerebbe `Timeout` oppure
`LimiteAttribuito` secondo quando il kernel ha consegnato l'evento — e la
differenza non starebbe nel worker.

Mezzo secondo è il doppio abbondante di ciò che il prototipo ha visto, e non è
una promessa che basti sempre. Se non basta, il dominio resta non quiescente,
l'evidenza **non si legge**, e la conclusione dichiara la barriera incompleta:
per un'osservazione che non si è potuta fare, dirlo è l'esito giusto.
*Rientro:* se un giorno il kernel offrisse una notifica di svuotamento
attendibile invece di un contatore da rileggere, l'attesa scomparirebbe con
essa.

**3. Il drenaggio della coda dei fatti: 30 secondi** (`TETTO_DEL_DRENAGGIO`).
Alla chiusura il supervisore lascia cadere tutte le bocchette e drena finché il
canale non dice `Disconnected` — mai fermandosi su un istante vuoto, perché fra
la domanda e la risposta un produttore vivo può ancora accodare, e i fatti
dell'ultimo istante sono quelli che raccontano com'è finita.

Se però una delle condizioni di chiusura non è stata rispettata — una sorgente
bloccante non resa terminabile, una bocchetta dimenticata — quell'attesa non
finirebbe mai, e un supervisore appeso è il peggiore degli esiti: non conclude,
non riporta, e non si distingue da uno che sta lavorando. Il tetto trasforma
l'attesa infinita in un **difetto detto**.

La scadenza è **assoluta e monotona**: si calcola una volta e non riparte
quando arriva un fatto. Si calcola con `checked_add` e non con `+`: la somma di
un istante e una durata può andare in overflow, e l'operatore in quel caso va in
panico — un panico dentro la chiusura del supervisore sarebbe il posto peggiore
in cui scoprirlo. Con trenta secondi non accade su nessuna macchina reale, ma
un'impossibilità va **osservata** e non presunta: se accadesse, si drena ciò che
c'è già e lo si dichiara. Con una scadenza che si rinnova a ogni consegna, un
produttore che invia poco prima di ogni scadenza terrebbe aperto il drenaggio
per sempre. Allo scadere, i fatti già drenati **restano nel rapporto** insieme
al difetto: ciò che si è sentito prima di rinunciare è evidenza quanto la
rinuncia. *Rientro:* nessuno previsto; il tetto scompare solo se la chiusura
diventa impossibile da sbagliare.

**4. L'arresto del lettore: tre cose diverse, e vanno tenute separate.** Un
lettore fermo dentro una `read` non si sveglia perché qualcuno altrove lascia
cadere un mandante. Il descrittore va quindi in modalità non bloccante e la
lettura passa da un adattatore che aspetta a piccoli passi guardando un
interruttore. Ciò che si può dire di quel meccanismo sono tre affermazioni, e
solo la prima è una garanzia.

*Le proprietà implementative*, vere sempre, e sono due — nessuna delle quali
dice quanto dura un giro. Primo: l'interruttore viene guardato **prima di ogni
nuova lettura e prima di ogni attesa**, e non esiste cammino in cui la lettura
riparta senza averlo guardato, nemmeno dopo un `Interrupted`. Secondo:
l'adattatore **non chiede mai volontariamente** un'attesa più lunga di un passo
(`PASSO_DI_ATTESA`, 10 ms).

Entrambe le garantisce il codice, e le prova un caso che non guarda l'orologio:
con il freno già tirato la sorgente non viene interrogata nemmeno una volta.
Scrivere invece «ogni giro dura al più un passo» sarebbe **falso**: `sleep`
promette che l'attesa non sia più breve di quanto si chiede, non che non sia più
lunga, e fra la richiesta e il risveglio ci sono lo scheduler e le chiamate di
sistema.

*Il presidio contro il giro a vuoto.* Il freno dà una via d'uscita, non un
limite al consumo: una sorgente che rende `Interrupted` senza fermarsi mai
occuperebbe un core a non fare niente finché qualcuno non frena. Dopo sedici
interruzioni consecutive (`INTERRUZIONI_PRIMA_DEL_RESPIRO`) l'adattatore chiede
un passo di attesa, continuando a guardare il freno a ogni giro. Il contatore
riparte a ogni esito diverso, così i segnali sporadici — quelli veri — non
pagano nulla.

*La non-garanzia*, che va detta perché non si deduca il contrario: **nessun
limite di tempo reale**. Fra il momento in cui l'interruttore cambia e il
momento in cui il processo torna sulla CPU passa quanto il kernel decide, e
senza uno scheduler real-time nessun numero scritto nel codice lo impedisce. Chi
ha bisogno di un tetto sull'orologio da parete deve prenderlo altrove — un
timeout di livello più alto, o un `SIGKILL` — non da qui.

*La soglia di qualificazione ambientale*: 200 ms, misurati su una macchina che
non è in ginocchio. È un attrezzo dei casi e **non un contratto**: vive sotto
`cfg(test)` proprio perché una costante pubblica prima o poi si legge come una
promessa, e chi la leggesse così lo farebbe in buona fede.

Il caso che la usa appartiene al perimetro di qualificazione dell'ambiente, non
alla prova di correttezza: se fallisse, la prima ipotesi da verificare è lo
stato della macchina. La correttezza — che il freno venga guardato — è provata
altrove, contando i giri invece del tempo. *Rientro:* un modo
sicuro di interrompere una lettura bloccante senza sondaggio; toglierebbe il
passo, la soglia e questa distinzione insieme.

### La diagnostica di riga non attraversa il confine del worker

**La regola.** L'errore che il worker dichiara arriva al supervisore con i
quattro assi intatti: `PlenoraError::Replayed` li porta così come sono, e
`category`, `phase`, `remote_effect` e `retry_disposition` li rendono senza
ricalcolarli. Non c'è approssimazione, e non serve una variante nuova.

**Ciò che non entra nell'errore.** La diagnostica di riga. `DiagnosticaSulFilo`
non è isomorfa a `RowDiagnostics`: le mancano campi, e completarli con valori di
default direbbe di aver osservato cose che nessuno ha osservato — un dato
inventato è indistinguibile da quello vero, ed è peggio di un dato assente.

**Dove sta, allora.** Nell'esito del supervisore, che la **possiede intera** e
nella forma in cui è arrivata, accanto alla classificazione e fuori dal
`PlenoraError`. Non è un conteggio: un conteggio conserva l'esistenza e non il
contenuto, e dire «ce n'era una» buttandola è un modo più educato di buttarla.
La forma del filo è limitata per costruzione — il protocollo tetta esempi e
conteggi — quindi tenerla non apre una via a una dimensione che il chiamante
sceglie.

Le due affermazioni vanno quindi lette insieme e nessuna copre l'altra: la
conversione dell'errore è senza perdite **sui quattro assi**, e la diagnostica
è conservata **intera** altrove.

**La decisione aperta** è solo se debba arrivare fino a `RowDiagnostics`, e con
quale portatore tipizzato: estendere il formato sul filo perché porti i campi
che mancano, oppure un tipo dedicato. Non giustifica in nessun caso di
duplicare gli assi dell'errore, che un portatore ce l'hanno già. *Rientro:* la
decisione, presa e scritta qui.

Ciò che il worker **non** deve fare è credere ai due numeri. Li riguarda: che
siano pipe anonime e non FIFO del filesystem, che il verso sia quello giusto, e
che dopo la riapertura l'impronta `(dispositivo, inode)` coincida **e** il verso
regga ancora. Quest'ultimo controllo non è ridondante, ed è misurato: riaprire
in scrittura l'estremo di lettura di una pipe riesce e rende un descrittore con
impronta **identica**, perché sono due aperture della stessa pipe.

### Il perimetro di qualificazione dell'isolamento

**La regola.** Il gate ostile ha bisogno di due cose che la produzione non deve
poter avere: l'immagine che riesegue sé stessa nei tre modi, e una barriera fra
l'accertamento dell'immagine e lo `spawn`. Vivono entrambe sotto
`#[cfg(qualificazione_isolamento)]`, che **non è una feature di Cargo**.

**Perché non una feature.** Una feature la si abilita dichiarandola fra le
dipendenze, e l'unificazione la propaga anche a chi non l'ha chiesta: una build
di produzione potrebbe ritrovarsela addosso perché un'altra cosa nell'albero
l'ha voluta. Un `cfg` non si propaga — nessun crate dipendente può accenderlo — e
non arriva mai come effetto collaterale.

**Che cosa questo non garantisce.** Chi controlla il comando di build può
mettere `--cfg qualificazione_isolamento` in `RUSTFLAGS` e ottenere il
perimetro di qualificazione anche in una build che chiama di produzione.
Scrivere che «la produzione non può selezionarlo» sarebbe falso: la garanzia è
che non ci si arrivi **per sbaglio**, non che non ci si possa arrivare. Un
perimetro contro l'incidente, non contro l'intenzione — e nessun `cfg`, nessuna
feature e nessun attributo fanno di più, perché chi costruisce il binario
decide che cosa ci mette dentro.

Il `cfg` è dichiarato in `[workspace.lints.rust]` con `check-cfg`, perché un
`cfg` non dichiarato non rompe la build: spegne silenziosamente il codice.

**Che cosa la barriera può e non può fare.** Non può saltare l'accertamento —
quando corre, quello è già avvenuto — né cambiare l'inode che `/proc/self/exe`
raggiunge. Può invece rendere obsolete le osservazioni sul nome, ed è
esattamente ciò che il gate le chiede: rinominando il pathname invalida la
fotografia ` (deleted)` appena scattata. Ciò che regge non è quel controllo, ma
l'esecuzione dell'inode.

**L'immagine è un esempio, non un `bin`.** `cargo install` gli esempi non li
produce. Costruita fuori dal perimetro, il suo `main` di ripiego rifiuta di
girare invece di girare a metà: un binario che girasse a metà darebbe al gate
la diagnosi sbagliata — «non ho osservato niente» invece di «sono stato
costruito male».

### Il `commit_token`: forma canonica unica, e valore mai mostrato

**La regola.** Un `commit_token` è esattamente 64 caratteri esadecimali
**minuscoli**. Non esiste un `CommitToken` non valido: il controllo sta in un
punto solo, il costruttore, e chi ne ha uno in mano non deve validarlo. La
rappresentazione interna è opaca e la forma testuale si **ricostruisce** dai
byte, così non può esistere un token che rende una grafia diversa da quella
che finirà nel footer.

Nel footer di un artefatto vive sotto una chiave sola: `plenora.commit.token`.

**Il perimetro.** Il token attraversa quattro confini — il chiamante che lo
fornisce, l'handshake che lo trasmette, il writer che lo scrive nel footer, il
verificatore che lo rilegge. La regola vale su tutti e quattro perché è nel
tipo, non nei quattro punti.

**Il pericolo che copre.** Due, distinti:

- **due grafie dello stesso valore.** Il footer si confronta byte per byte:
  accettare `ABC…` accanto a `abc…` darebbe due artefatti diversi per lo
  stesso commit. Per questo la forma non canonica si **rifiuta** e non si
  normalizza — normalizzare significherebbe accettare due grafie e poi
  scoprire che il footer le distingue comunque;
- **il valore in un log.** `Debug` e `Display` non lo mostrano, e nessun
  errore lo contiene. `Debug` in particolare è ciò che finisce in un log per
  sbaglio, dentro il `{:?}` di una struttura più grande. Il rifiuto è proprio
  il momento in cui qualcuno vorrebbe vedere il token per capire, ed è anche
  il momento in cui mostrarlo lo consegna a chi legge quel log. L'errore del
  footer non canonico è un `&'static str`: non è una disciplina da ricordare,
  è il tipo che non consente di portarci dentro il valore.

La serializzazione invece lo emette, e l'asimmetria è voluta: sul filo serve,
in un log no.

**Le quattro forme del footer.**

| forma | esito |
|---|---|
| assente | **legittimo** — un artefatto ordinario non ha un token |
| canonico | accettato |
| presente ma non canonico | **rifiutato sempre**, in ogni percorso |
| chiave duplicata | rifiutato dalla traversata rinforzata |

Il duplicato non può nascere da questo lato: `FileWriter::write_metadata`
tiene le coppie in una mappa e due scritture della stessa chiave collassano.
Può arrivare solo da un produttore estraneo, e lì lo rifiuta il parser.

Che il token sia **obbligatorio** è una proprietà del percorso isolato, non
della lettura: `leggi_commit_token` dice cosa c'è, non se doveva esserci.

**Come si legge, e come non si legge.** Il token si estrae dalla **stessa
traversata** che convalida il footer, non da `FileReader::custom_metadata`.
Quella sarebbe una terza strada nel footer, e di tutti i controlli che il
parser rinforzato applica non ne farebbe nessuno — allocazione limitata,
chiavi e valori presenti, duplicati rifiutati. Un valore autoritativo
raggiungibile senza convalida è peggio di un valore assente.

**Senza token i byte non cambiano.** Con `None` non si scrive nulla, nemmeno
una chiave vuota: è ciò che rende innocuo aggiungere il token al percorso
in-process, che passa sempre `None`. Un writer che scrivesse una chiave vuota
supererebbe ogni prova sul contenuto e cambierebbe ogni artefatto già
prodotto.

**La condizione di rientro.** Cambiare la forma canonica — lunghezza,
alfabeto, grafia — significa cambiare la chiave del footer insieme a essa: due
grafie sotto lo stesso nome non sono distinguibili da chi rilegge. Mostrare il
valore in `Debug` non ha condizione di rientro: se serve, esiste già
`in_esadecimale`, che è una riga che si legge in review.

### Il `commit_token` si deserializza solo da formati autodescrittivi

**La regola.** `<CommitToken as Deserialize>::deserialize` chiede
`deserialize_any`, non `deserialize_str`. Un formato che non descrive da sé il
tipo di ciò che porta — `bincode` e simili, dove il tipo lo deve dichiarare il
chiamante — **non** può deserializzare un `CommitToken`.

**Il perimetro.** La sola `impl Deserialize`. La serializzazione non è toccata:
emette una stringa e funziona ovunque. `da_esadecimale` nemmeno: chi ha un
testo lo valida senza passare da serde.

**Perché la scelta è questa, e non è una comodità.** Chiedendo
`deserialize_str`, un `serde_json` che trova un numero **non chiama il
visitatore**: sbriga il disaccordo di tipo da sé con `peek_invalid_type`, che
costruisce il messaggio dal valore letto. Il rifiuto sarebbe giusto e direbbe
«invalid type: integer `1234…`» — cioè il token in chiaro, se qualcuno lo ha
scritto senza virgolette. Con `deserialize_any` il formato si limita a dire
*che cosa* ha trovato, e la decisione, col messaggio, torna al nostro
visitatore, dove il valore non ha un parametro in cui entrare.

**Il pericolo che copre, e quello che apre.** Copre il valore del token in un
messaggio d'errore, cioè in un log — lo stesso pericolo del resto di questa
sezione, per la sola via che restava aperta. Apre un vincolo sul trasporto: se
un domani il protocollo passasse a un formato non autodescrittivo, questa
`impl` smetterebbe di funzionare — **rumorosamente**, con un errore del
formato, non in silenzio. Oggi sul filo c'è JSON e il protocollo non prevede
altro: lo dice [`isolamento.md`](isolamento.md#4-protocollo-interno).

**La condizione di rientro.** Cade il giorno che il protocollo adotti un
formato non autodescrittivo. Quel giorno la strada non è tornare a
`deserialize_str` — riaprirebbe esattamente la fuga — ma dare al tipo una
`impl` che il nuovo formato sappia guidare, verificando **sul messaggio
prodotto** che il valore non compaia. La verifica va rifatta, non dedotta: che
varianti di `Unexpected` un formato costruisca è una scelta di quel formato,
non una garanzia di serde.

### `deny_unknown_fields` non copre le varianti unitarie degli enum con tag

**La regola.** In un enum serde con tag interno (`#[serde(tag = "...")]`),
`deny_unknown_fields` **non ha effetto sulle varianti unitarie**: serde le
riconosce dal tag e ignora silenziosamente il resto dell'oggetto. Ogni variante
senza campi di un enum con tag che dichiara `deny_unknown_fields` va quindi
scritta come variante di struttura vuota — `Never {}` e non `Never`. Sul filo
la forma è identica; a cambiare è che la deserializzazione passa da un visitor
di struttura, dove il controllo vale davvero.

**Il perimetro.** Gli enum con tag interno che **dichiarano**
`deny_unknown_fields`. Nel protocollo sono `RetrySulFilo` ed
`EsitoWorkerSulFilo`. `KnownOrUnknownCount` (`plenora-core`) ed `Expression`
(`plenora-kernels-table`) hanno un tag interno ma **non** dichiarano
`deny_unknown_fields`: non promettono il controllo, quindi non lo tradiscono.

**Il pericolo che copre.** Scritto `Never`, `RetrySulFilo` accettava
`{"kind":"never","delay_ms":10}` e buttava via `delay_ms` in silenzio — cioè
esattamente ciò che quel campo esiste per impedire: una disposizione che non
concede il ritentativo che porta con sé un ritardo. L'attributo era presente e
sembrava proteggere; è il caso peggiore, perché la difesa era dichiarata e
assente insieme.

**La condizione di rientro.** Nessuna: serve finché serde si comporta così.
Se un giorno `deny_unknown_fields` coprisse anche le varianti unitarie, le
graffe vuote diventerebbero rumore e si potrebbero togliere — ma solo con un
test che mostri il rifiuto senza di esse.

### Il tetto cumulativo sui dizionari

**La regola.** `IpcLimits::max_retained_dictionary_body_bytes` limita la
**somma** dei `bodyLength` dei `DictionaryBatch`, non il più grande. È l'unico
tetto cumulativo del confine, e c'è perché i dizionari sono l'unica cosa che
il lettore trattiene tutta insieme: `FileReader` li decodifica dentro
`try_new` e li tiene per l'intera scansione, uno `StreamReader` accumula
quelli che incontra. Il tetto per singolo body non li governa: mille dizionari
da un megabyte lo rispettano tutti e insieme trattengono un gigabyte.

Si applica in **due punti complementari**, e nessuno dei due sostituisce
l'altro:

| dove | che cosa vede |
|---|---|
| la traversata dei messaggi | i `DictionaryBatch` incontrati percorrendo la regione dati — vale per lo stream, per il file e per ogni lettore ostile |
| i blocchi del footer | quelli che `FileReader` leggerà **davvero**, saltando agli offset senza percorrere la regione |

Superamento e **trabocco** della somma sono cose diverse e hanno errori
diversi. Il superamento rende sempre `IpcRetainedDictionariesTooLarge`, con la
somma vera e il tetto. Il trabocco non porta nessun numero — non ce n'è uno
onesto — e la variante dipende da **dove** è avvenuto:

| percorso | trabocco |
|---|---|
| traversata dei messaggi | `IpcTruncated`: si sta percorrendo la regione dei messaggi, e un footer può non esserci affatto — lo stream non ne ha uno |
| conteggio dei blocchi del footer | `IpcFooterInvalid`: lì il footer c'è per definizione, ed è la struttura incoerente |

Nominare una struttura assente manderebbe chi legge a cercarla.

**La sovrastima dichiarata.** La traversata somma **tutti** i
`DictionaryBatch`, anche quando lo stream **sostituisce** un dizionario già
visto — stesso `id`, valori nuovi. Arrow in quel caso ne tiene uno solo, quindi
la nostra somma è maggiore di ciò che resta vivo, e uno stream con molte
sostituzioni può essere rifiutato pur restando entro il consumo reale.

È una sovrastima **conservativa e voluta**: distinguere i `dictionary id` in
prevalidazione significherebbe tenere una mappa `id → ultimo bodyLength` e
fidarsi che il lettore a valle faccia la stessa scelta di rimpiazzo che noi
prevediamo. Preferiamo rifiutare qualcosa di accettabile piuttosto che
ammettere un consumo che non abbiamo misurato. La condizione di rientro: il
giorno in cui un carico reale usi sostituzioni ripetute, la somma diventa per
`id` — e allora va provato che la regola di rimpiazzo coincide con quella del
lettore, non assunto.

**I dizionari delta sono rifiutati.** Con `isDelta`, Arrow non trattiene il
body dichiarato: concatena il dizionario precedente con il nuovo in un buffer
ulteriore, mentre entrambi gli originali sono ancora vivi. Il picco si avvicina
al **doppio** della somma, e la formula della memoria trattenuta di
[`isolamento.md`](isolamento.md) sarebbe falsa proprio quando il tetto dice che
va tutto bene.

Il rifiuto è in prevalidazione, comune a tutti i lettori, ed è una
**deviazione dal formato**: un dizionario delta è un ingresso Arrow stream
perfettamente valido, e questo confine lo esclude deliberatamente. Ciò che si
perde è un ingresso legittimo che nessuno dei nostri produttori genera — il
`FileWriter` non ne emette — quindi il costo oggi è nullo, ma il costo esiste e
un lettore esterno che ce ne mandasse uno verrebbe rifiutato senza aver
sbagliato niente. Il rientro non è «alzare il tetto»: è rifarlo sul picco della
concatenazione.

**`header` e `data` sono obbligatori.** Un messaggio che dichiara
`header_type` senza portare l'`header` salta ogni controllo della
prevalidazione e arriva ad Arrow, che lo legge con `unwrap()`; un
`DictionaryBatch` senza `data` fa lo stesso dentro `read_dictionary`. Entrambi
sono rifiutati: la barriera anti-panico tradurrebbe il panico in errore, ma è
l'ultima difesa e non la prima.

**Il pericolo che copre.** Un ingresso che dichiara molti dizionari piccoli, o
un delta, e fa trattenere al lettore molto più di quanto qualunque tetto
per-messaggio ammetta — con la formula della memoria che continua a dire che
il picco è limitato.

### Fuzzing su toolchain nightly

Il solo step `cargo fuzz run` gira su nightly, mentre build, test, clippy e
gate anti-panic restano sulla toolchain pinnata. Un crash conta **solo se
riproducibile sulla pinnata**: riproduzione e minimizzazione avvengono lì
prima di aprire una correzione.

## Il verificatore

`scripts/verifica_memoria_governata.py` verifica che i siti di allocazione e
prenotazione descritti in questo documento esistano ancora **nella forma
descritta**, e che i pattern eliminati non riappaiano. Un elenco del genere
marcisce in silenzio: basta che qualcuno sposti una reservation e il documento
resta convincente e falso.
