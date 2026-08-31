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

**La regola.** `plenora_engine::protocollo` e `plenora_engine::verifica` sono
compilati solo con `#[cfg(any(test, feature = "internals"))]`;
`plenora_engine::commit_footer::leggi_commit_token` e
`geo_transport::ipc::parse_footer` solo con `#[cfg(test)]`. Non è
un'ottimizzazione: è la dichiarazione che quel codice **non ha ancora un
chiamante di produzione**.

**Il perimetro.** Il modulo `protocollo` per intero — messaggi, codifica,
lettore limitato, handshake — il modulo `verifica`, che esegue i passi da 3 a
8-bis della sequenza di [`isolamento.md`](isolamento.md), la sola funzione di
lettura del token dal footer, e la forma breve di `parse_footer` (da quando la
convalida estrae anche un custom metadata, la produzione passa tutta per
`parse_footer_estraendo`). `commit_footer::scrivi_commit_token` è invece
incondizionato, perché il writer in-process lo chiama davvero (con `None`).

Il perimetro include anche il **supporto esclusivo del verificatore**, che ha
lo stesso stato — nessun chiamante di produzione fino a `PR-10` — e che senza
`cfg` sposterebbe altrove gli avvisi che il `cfg` doveva chiudere:
`commit_footer::interpreta_commit_token`, `Esadecimale32::dai_byte`,
`ipc_boundary::ArtefattoConvalidato` con i suoi metodi, e
`ipc_boundary::convalida_artefatto`. Un `cfg` sul modulo che lascia scoperto
ciò che solo quel modulo usa non è un perimetro, è una linea tracciata a
metà.

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

**La condizione di rientro.** Il `cfg` su `protocollo` sparisce quando esiste
un chiamante esterno al modulo, cioè con **`PR-8`** — il supervisore con
worker fittizio. Non con `PR-5`: l'handshake che `PR-5` aggiunge sta *dentro*
`protocollo`, quindi ne consuma i messaggi ma non è un chiamante del modulo, e
toglierlo lì rimetterebbe in piedi le decine di `dead_code` che il `cfg`
evita — verificato rimuovendolo e leggendo l'output, non dedotto.

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
