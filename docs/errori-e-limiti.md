# Errori e limiti

Che cosa succede quando qualcosa va storto, e **dove le garanzie si
fermano**. La seconda parte è quella che conta: un limite dichiarato è un
limite gestibile, un limite taciuto è una sorpresa.

## Envelope e canali

Ogni errore esce come **envelope JSON su stdout**, una riga, `protocol_version
1`. **stderr resta vuoto.**

Limite: se stdout non accetta la scrittura (chiuso, pieno), l'envelope non
esce o esce a metà. Ambito: i comandi della CLI, non le modalità riservate
dello spawner, del worker e del verificatore. Hazard: chi legge stdout non ha
un documento, o ne ha uno troncato. La CLI esce allora con `70`, qualunque
sia la categoria, e stderr resta vuoto. Rientro: nessuno, finché il canale è
stdout; un chiamante che vede `70` senza un documento intero sa che l'errore
vero non è arrivato.

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

### Il percorso isolato usa un altro meccanismo, non lo stesso handler

**La regola.** `run` sul percorso isolato (`isolamento::esecuzione_isolata`,
solo Linux) non installa l'handler di `ctrlc`: farebbe nascere un thread per
tutta la vita del processo, e `isolamento::canale::accerta_monothread`, che
pretende un task solo prima di rendere ereditabili i descrittori, fallirebbe a
entrambe le finestre di spawn. Registra invece
`signal_hook::flag::register` e `register_conditional_shutdown`
(`installa_gestore_segnale_isolato` in `plenora-cli`), che non creano thread e
scrivono il bit che `CancellationToken::is_cancelled` già legge
(`CancellationToken::condividi_flag`). La registrazione è una sola per tutta la
vita del processo: copre fase worker, transizione e fase verificatore allo
stesso modo. L'`unsafe` attorno alla `sigaction` resta nella dipendenza; nel
workspace `unsafe_code = "forbid"` non cambia.

La transizione fra i due domini, dove nessuna sorveglianza osserva il token da
sola, ha tre controlli propri in `esecuzione_isolata.rs`: prima di creare il
dominio del verificatore, prima dello spawn, e subito dopo — se il verificatore
è già nato quando la cancellazione si osserva, viene terminato e raccolto. La
cancellazione singola esce `cancelled`, exit **130**, come sul percorso non
isolato; qualificata con segnali reali sulle tre finestre, senza pubblicazione
né residui.

**Che cosa è diverso, e va detto.**

- Nessun messaggio sul primo né sul secondo Ctrl-C: `flag::register` scrive
  solo un `AtomicBool`, e l'uscita del secondo è async-signal-safe, senza un
  «dopo» in cui stampare.
- **Il secondo Ctrl-C esce 130 senza pulizia.** `signal_hook::low_level::exit`
  salta `atexit` e ogni `Drop`: osservato dal vivo, un secondo Ctrl-C durante
  la fase worker lascia **vivo il worker** e **il suo dominio cgroup** non
  rimosso. Nessun output è pubblicato — la barriera di `verifica_poi_pubblica`
  sta a monte — ma la macchina resta con un processo e un cgroup da togliere.

**Il pericolo.** Chi usa il secondo Ctrl-C come «esci subito» sul profilo
isolato lascia residui che nessuno raccoglie.

**La condizione di rientro.** Un'uscita forzata che termini il dominio con
`cgroup.kill` prima di uscire, restando async-signal-safe.

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

### Una destinazione occupata è un `Conflict`, per entrambe le strade

**La regola.** Se la destinazione esiste, la pubblicazione fallisce con
`PlenoraError::Conflict`, fase `Commit`, e non tocca ciò che c'era. Vale sia
per il controllo che precede la scrittura, sia per l'`AlreadyExists` che il
commit osserva.

**Perché `Conflict` e non `InvalidPlan`.** Perché la variante è documentata per
questo caso — «destinazione già esistente o conflitto di scrittura» — e la sua
fase è `Commit` per costruzione. Il piano, quando la destinazione è occupata,
non ha niente di sbagliato: è il posto a essere preso. Chiamarlo piano invalido
manda chi legge a correggere qualcosa che è già corretto, e cambia l'exit code
da 5 — condizione operativa — a 2, che dice «sistema qualcosa a monte prima di
riprovare».

**Perché le due strade devono concordare.** Perché il controllo preliminare è
un'anticipazione, non l'autorità: fra lui e il commit c'è la scrittura intera, e
la destinazione può comparire nel mezzo. L'autorità è l'`AlreadyExists`
osservato al persist, che parla dell'unico istante che conta. Se le due strade
dessero classi diverse, la stessa condizione avrebbe due nomi a seconda di
quanto è durata la scrittura, e chi automatizza dovrebbe conoscerli entrambi.

**Che cosa cambia per chi legge.** L'exit code di una destinazione occupata
passa da 2 a 5 e il testo da `contract violation:` a `conflict:`. È un
cambiamento visibile, ed è la correzione di una classificazione che i documenti
già prescrivevano e il codice non applicava.

### La pulizia del temporaneo esce sempre nella stessa forma

`temp_cleanup` è **sempre un oggetto**, anche quando lo stato è `removed`: chi
consuma il documento non deve prima scoprire di che tipo sia il valore, e il
giorno che «rimosso» acquistasse un campo la forma non cambierebbe sotto chi la
legge. Lo stato sta sempre in `state`; gli altri campi dipendono da lui.

Il percorso del residuo **non passa da `Path::display()`**, che è dichiaratamente
lossy: sostituisce con `U+FFFD` ciò che non è testo valido, e su Unix un percorso
è una sequenza di byte che non deve essere testo. Un'indicazione di bonifica con
un carattere sostituito indica un file che non esiste, ed è peggio di nessuna
indicazione — manda a cancellare il nome sbagliato, o a cercare invano.

`path_encoding` è **sempre** presente e dichiara come leggere il resto: `utf8`
col campo `path`, che è il caso ordinario e resta leggibile da un umano;
altrimenti `unix_bytes` o `windows_utf16` col campo `path_units`. Il dettaglio,
e come si ricostruisce, sta più avanti in questo documento.

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

### Una colonna che l'operazione sostituisce non si materializza: provato solo per `explode`

**La regola.** Un kernel che sostituisce una colonna non ne materializza la
versione vecchia: `select_rows` fa `take()` su tutte le colonne, e su una
colonna lista concentrata su poche righe l'intermedio cresce col quadrato
della lunghezza, fuori da ogni prenotazione del governor.

**L'ambito.** `table.explode` usa `select_rows_except`, che mette un
segnaposto nullo sulla colonna che sta per sostituire; lo provano
`reshape::tests::explode_su_riga_singola_con_lista_lunga_non_e_quadratico` e
`reshape::tests::explode_con_output_column_distinto_mantiene_la_colonna_sorgente`.
Gli altri kernel di `reshape.rs` — `unnest` per primo, che gestisce gli indici
a modo suo — **non** sono stati verificati.

**Il pericolo.** Un intermedio non governato che porta il processo all'OOM
prima che l'errore di budget esista.

**La condizione di rientro.** Una verifica degli altri chiamanti di
`select_rows` che sostituiscono una colonna, con un test per ciascuno.

### Il tetto duro per esecuzione è il profilo isolato

Un tetto duro per singola esecuzione **non è realizzabile in-process**, e la
ragione non è pigrizia:

- Arrow chiama `handle_alloc_error` sul fallimento di allocazione, che
  **aborta il processo**: un allocatore che rifiuta non produce un errore
  diagnosticabile, produce un abort senza stack, senza messaggio e senza
  `execution_id`;
- GEOS e PROJ allocano dal `malloc` di sistema, fuori da qualunque
  contabilità Rust;
- «per processo» non è «per esecuzione».

Il tetto duro è quindi il **profilo isolato**: un piano v6 che dichiara
`max_domain_memory_bytes` esegue in un processo worker dentro un dominio
`cgroup2` con `memory.max`, su **Linux** soltanto
([`isolamento.md`](isolamento.md)). Il profilo promette **contenimento e
attribuzione**, non «mai un byte oltre N»: `memory.max` ammette superamenti
temporanei. Un `ResourceLimit` attribuito al dominio si riporta solo con
evidenza attribuibile (`memory.events.local`); un segnale o un exit code
anomalo sono compatibili con un crash quanto con un superamento, e senza
evidenza l'errore resta `internal`. Su Windows e macOS il profilo è rifiutato
in validazione.

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

**Che cosa resta aperto, per intero.** `Field.children` è solo la voce più
visibile; la superficie non ancora validata è questa:

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

### Il coordinatore del profilo isolato legge fuori dal dominio

**La regola.** Nel profilo isolato il coordinatore — il processo che esegue
`run` — non ha un dominio, e prima dell'autorizzazione legge soltanto ciò che
serve a validare il piano, con tetti **costanti**:

| che cosa | tetto |
|---|---|
| il testo del piano | `MAX_CONTROL_JSON_BYTES`, 16 MiB |
| quanti ingressi apre | `max_inputs` effettivo — il default, 16, o quello più basso che il piano dichiara — verificato sugli input dichiarati **prima** della prima apertura, in `run` come in `validate` |
| lo schema di un ingresso file format | il footer già convalidato, entro `max_metadata_bytes` (16 MiB): lo schema si ricava da quei byte, senza `FileReader`, quindi **senza decodificare i dizionari** |
| lo schema di un ingresso stream format | il solo messaggio di schema, entro lo stesso tetto; un messaggio Schema con un corpo è rifiutato dal confine, perché `StreamReader` lo allocherebbe prima di guardarne il tipo |

Nessuna riga e nessun dizionario è decodificato fuori dal dominio: i dati li
legge il worker, dentro il proprio `memory.max`. Dopo lo spawn il coordinatore
legge frame del protocollo entro `MAX_PROTOCOL_FRAME_BYTES`, l'evidenza entro
1 MiB per file, e pubblica copiando a blocchi di 64 KiB.

**Il perimetro.** I tetti sono del confine, non del piano né della politica
dell'host: un host con un tetto di dominio da 64 MiB ammette comunque un
coordinatore che legge un footer da 16 MiB. Gli ingressi si leggono **in
sequenza**, e di ciascuno resta solo lo schema.

**Che cosa la scoperta non attesta più.** Lo schema dal footer ripete i
controlli che `FileReaderBuilder::build` fa **prima** dei dizionari — verifica
`FlatBuffer`, vettore dei record batch presente, schema presente, endianness —
e non quelli che fa **leggendoli**: che i valori siano decodificabili (per
esempio UTF-8 valido), che l'id di ogni dizionario corrisponda a un campo dello
schema, che le versioni di messaggio e footer siano compatibili. `describe` e
`validate` possono quindi accettare un file il cui dizionario `run` rifiuterà;
in `run` il rifiuto arriva quando il worker apre l'ingresso, dentro il dominio,
ed è lo stesso `DataMapping` di fase `Read`.

**Il pericolo.** Tre, distinti:

- `describe` e `validate` non sono una prova che l'ingresso si legga per
  intero: chi li usa come controllo preventivo deve saperlo;

- il rifiuto per politica dell'host assente arriva **dopo** la lettura degli
  schemi: un host non configurato paga quelle letture per un'esecuzione che non
  partirà;
- sullo **stream format** resta la finestra di
  [Finestra TOCTOU sull'ingresso IPC](#finestra-toctou-sullingresso-ipc):
  `StreamReader` rilegge dal file la lunghezza del messaggio di schema, e un
  file mutato sul posto dopo la convalida può dichiararne una che nessun tetto
  ha visto. Sul file format la finestra per lo schema non c'è, perché i byte
  sono quelli convalidati.

**La condizione di rientro.** La scoperta dei contratti dentro un dominio — il
worker la ripete già, e il coordinatore potrebbe riceverne l'esito invece di
calcolarlo — oppure lo schema stream ricavato dai byte convalidati, come per il
file format. Per `describe` e `validate`, una verifica dei dizionari in streaming
e sotto tetto, separata dalla scoperta dello schema.

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

Il degrado non richiede input costruiti apposta. Sulle chiavi a più blocchi
in cui i byte che variano stanno in cima a un blocco e nel blocco di coda —
la chiave binaria di un Int64, marcatore più 8 byte big-endian — due blocchi
si annullano: un milione di interi distinti danno 32 768 hash diversi. La
tabella di chiavi dei raggruppamenti in memoria (`KeyInterner`: `aggregate`,
`distinct`, set operation, `assert_unique`, `table_diff`) usa per questo un
hash proprio, anch'esso non keyed, che ripiega i bit alti a ogni blocco. Le
mappe di chiavi dello spill delle set operation usano ancora `KeyHasher`
sulle stesse chiavi binarie: il rischio residuo è di tempo, non di
risultato, e rientra quando anche quelle mappe passano alla tabella di
chiavi.

### Arrotondamento nelle operazioni a risultato `Float64`

La conversione intero/decimal → `f64` è **esatta o errore**, tranne nelle
operazioni il cui risultato è un `Float64` per contratto: lì il double è il
tipo del risultato, non un passaggio intermedio, e pretendere l'esattezza
rifiuterebbe input legittimi. Un valore oltre 2^53, o un decimal frazionario,
perde precisione senza errore. Un percorso decisionale non deve avere questa
proprietà: in `table.expression` i confronti (`equal`…`less_equal`,
`between`, `in`, `null_if`, `greatest`/`least`) decidono sul valore esatto
d'origine di colonne e letterali, non sul loro double. Un operando prodotto da
un calcolo (aritmetica, `round`, `floor`, `ceil`, `power`) vale il proprio
double, che è il risultato dichiarato di quel calcolo; `negate` e `abs`
restano esatti, e rifiutano un `Decimal128` il cui opposto non sta in `i128`.
Allo stesso modo `table.bin` assegna la classe e gli aggregati con
`distinct` deduplicano sul valore esatto (`scalar_as_numero`), mentre bordi e
riduzioni restano sul double.

Non è una deroga con rientro: è la semantica dichiarata di quelle operazioni.

Il contro-esempio sta nella stessa famiglia: le funzioni di **rango**
(`rank`, `dense_rank`, `percent_rank`, `cume_dist`) producono un `Float64` ma
**ordinano**, quindi non convertono — confrontano il dominio originale.

### Un letterale numerico JSON vale il double che il parser ne ricava

**La regola.** I letterali numerici del piano passano da `serde_json`, che
senza la feature `arbitrary_precision` conserva esatti gli interi in gamma
`i64`/`u64` e rende ogni altro numero come `f64`. Chi confronta un letterale
(`table.filter`, `table.conditional`, `table.expression` e le regole di
governance) ne legge il testo più corto che rappresenta quel double, come
numero o come stringa secondo l'operatore: `0.1` resta 0,1, un esponenziale
come `1e-7` resta un double, e un letterale scritto
con più cifre significative di un double, come `0.100000000000000001`, o un
intero oltre `u64`, arriva già arrotondato.

**L'ambito.** I letterali **numerici** JSON dei piani. Un valore scritto come
stringa dove l'operazione accetta il testo (`table.filter`) è letto esatto
dal proprio testo; le colonne sono sempre lette esatte.

**Il pericolo che questo dichiara.** Un confronto con un letterale scritto
oltre la precisione di un double decide su un valore vicino a quello scritto,
senza errore.

**La condizione di rientro.** Leggere i numeri del piano dal loro testo:
`arbitrary_precision` di `serde_json`, oppure il testo grezzo dei letterali
(`RawValue`), con la verifica che l'hash canonico del piano non cambi.
Nessuna delle due è una modifica locale: la prima cambia `Value` per tutto il
workspace.

### Il testo numerico in notazione esponenziale si legge come double

**La regola.** Una cella `Utf8` interpretata come numero si legge esatta con
`NumericBound::parse` quando è un intero o un decimale posizionale fino a 38
cifre significative; ogni altra forma che `f64` accetta (`9007199254740993e0`,
`inf`, `NaN`, un decimale più lungo) diventa il suo double.

**L'ambito.** Le decisioni sul testo numerico: le classi di `table.bin`, i
distinti degli aggregati e i confronti con un letterale (`scalar_compare`:
filtri e regole di governance su colonne `Utf8`).

**Il pericolo che questo dichiara.** Due testi numerici in notazione
esponenziale che differiscono oltre la precisione di un double contano come lo
stesso valore, senza errore.

**La condizione di rientro.** Un parser esatto per la notazione esponenziale
(mantissa decimale e esponente in scala), oppure il rifiuto esplicito di quelle
forme dove il valore serve a decidere.

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

### I token `invalid` e `ambiguous` delle operazioni temporali sono accettati e disattesi

**Regola violata.** Un parametro di configurazione accettato deve avere
effetto. Qui non lo ha: `invalid` (`null` o `error`) e `ambiguous` (`error`,
`null`, `earliest` o `latest`) si deserializzano senza errore, ma il kernel non
li legge.

**Ambito.** `table.date_format`, `table.date_add`, `table.date_diff`,
`table.timezone_convert` e `table.date_extract`; il token `invalid` in tutte e
cinque, il token `ambiguous` in `timezone_convert`. Qualunque valore dei due
token produce lo stesso comportamento: una riga non parsabile, fuori range o
con un'ora locale ambigua o inesistente rifiuta il batch con diagnostica
row-scoped (`conversion.invalid_datetime`, `conversion.datetime_range`,
`conversion.ambiguous_local_time`, `conversion.nonexistent_local_time`). Mai
un null sintetico, mai una scelta implicita fra le due ore ambigue.

**Hazard.** Un piano scritto per la semantica precedente — `invalid: null`
per ottenere null sulle righe cattive, `ambiguous: earliest` o `latest` per
far scegliere l'ora — è accettato in analisi e riceve in esecuzione un
rifiuto invece del null o dell'ora scelta. La garanzia è più forte, non più
debole: nessun dato sbagliato esce. Ma il piano non fa più ciò che dichiara,
e l'analisi non lo segnala.

**Condizione di rientro.** Una delle due: una versione del piano che tolga i
token e li rifiuti in analisi, oppure il ripristino della loro semantica con
un contratto esplicito su che cosa entra nella diagnostica quando una riga è
resa null o risolta per scelta.

### Row diagnostics: le collettive di solo trasporto

Le operazioni collettive e one-to-many del solo trasporto non raccolgono
diagnostica per riga: l'attribuzione sarebbe a **insiemi** di righe (l'intera
collezione, o coppie), fuori dallo schema single-row corrente. Su quel
percorso una failure del kernel su input già validato dal gate è fail-closed
senza `source_index`. Nel DAG quelle operazioni non sono dispatchate.

### `with_row_diagnostics` degrada a `Internal`

**La regola.** Un payload `RowDiagnostics` che non supera
`validate_for_emission` non si scarta da solo: sostituisce l'intero errore.
`with_row_diagnostics` non restituisce il fallimento ordinario del batch con
la diagnostica scartata — restituisce `ArrowTransportError::Internal("row
diagnostics interne non valide")`, senza traccia né della causa originale né
di una diagnostica di riga.

**Perché.** Propagare l'errore originale accompagnato da un payload che non
ha superato la validazione lascerebbe aperta la possibilità che quel payload
attraversi comunque il confine in una forma non conforme. `Internal` dichiara
«difetto nostro» invece di lasciar credere che la riga sia stata
diagnosticata correttamente.

### Chiavi canoniche emesse prima della ratifica

Le chiavi di metadata canoniche sono emesse prima che la sezione normativa
corrispondente sia ratificata. Arrow le preserva per costruzione e nessun
consumatore attuale le interpreta: il rischio è limitato a un'incompatibilità
di **nomi**, coperta dalla migrazione se i nomi ratificati risultassero
diversi.

<a id="panici-attesi-nel-fuzzing"></a>
### Il fuzzing tollera solo i panici attesi delle dipendenze

**La regola.** Ogni target installa l'hook comune di
`fuzz/fuzz_targets/comune/aggancio.rs`, e `scripts/verifica_target_fuzz.py` lo
pretende. L'hook tace su un panico nato dentro una
`plenora_core::panic_policy::barriera_di_dipendenza` e lascia tutti gli altri
all'hook di `libfuzzer-sys`, che stampa e interrompe **prima**
dell'unwinding.

**Il perché.** `libfuzzer-sys` interrompe prima dell'unwinding perché un
`catch_unwind` nel codice sotto prova non nasconda i difetti. Ma un
`catch_unwind` non vale l'altro. Una **barriera di dipendenza** esiste perché
una dipendenza nominata va in panico su un ingresso che riceviamo per
mestiere — `relate` di `geo`, `fb_to_schema` di `arrow-ipc`
(`apache/arrow-rs#10575`) — e ne fa un esito classificato: è il comportamento
corretto, e col solo hook di `libfuzzer-sys` `arrow_transform` e
`wkt_operations` resterebbero rossi a barriera funzionante. Una **rete di sicurezza** — il
`catch_unwind` dell'executor attorno a un kernel, quello del worker e del
verificatore — intercetta un panico che non doveva avvenire: per il fuzz resta
un crash, anche se in produzione diventa un errore `Internal`.

**Il perimetro.** Le barriere di dipendenza sono cinque:
`validazione_protetta` e `calcolo_protetto` in `plenora-kernels-geo`,
`guarded` e `BoundaryBatches::next` in `ipc_boundary`, `decode_ipc` nel
trasporto. Il loro lavoro contiene la sola chiamata alla dipendenza, più
controlli nostri senza primitive di panico.

**Il pericolo.** Un panico **nostro** dentro il lavoro di una barriera di
dipendenza sarebbe tollerato dal fuzz come quello della dipendenza. È la
ragione per cui il lavoro resta minimo, e per cui una barriera nuova si
dichiara qui.

**La controprova.** Un panico iniettato nel corpo di `wkt_operations`, fuori da
ogni barriera, termina ancora il target con `deadly signal`.

**La condizione di rientro.** Nessuna, per la regola: è la forma in cui il
fuzz distingue. Le singole barriere cadono quando la dipendenza smette di
andare in panico — per `arrow-ipc`, con `apache/arrow-rs#10575`.

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
  il PROJJSON malformato **prodotto da PROJ**, con il testo di serde.

`geo` non figura qui: il testo di `geo::Validation` interpola indici
dell'ingresso, quindi si legge solo per classificare e ne esce una voce
chiusa, `RagioneNonValida`, con un caso per la forma non riconosciuta. Vale
per i kernel e per la trasformazione PROJ; lo presidia il canary
`ogc_validation_classifies_overlap_without_leaking_the_member_index`.

**Hazard:** la riga di confine è il *percorso*, non la libreria. Se
`serde_json` venisse usato per deserializzare contenuto di celle su un
percorso che propaga l'errore, o PROJ per interpretare una definizione presa
dai dati, quei due punti diventerebbero fughe.
`scripts/verifica_privacy_dipendenze.py` intercetta i nuovi siti diretti dei
pattern noti, ma è un controllo testuale: la mappa resta una revisione da fare
a ogni nuovo uso di una dipendenza sul percorso dati.
**Condizione di rientro:** adattatori tipizzati per dipendenza **e
operazione**, che espongano solo un codice stabile e un testo controllato,
come già fatto per arrow, GEOS e la trasformazione PROJ.

### Scavenging temporaneo: comanda l'heartbeat, il PID può solo accelerare

Una directory `plenora-*` è rimossa se il suo heartbeat è più vecchio del TTL,
oppure — più in fretta — se l'heartbeat è fermo da oltre cinque minuti **e**
il lock viene da questa macchina **e** il PID registrato non esiste più. Un
heartbeat fresco non è mai toccato, qualunque cosa dica il PID; su Linux, dove
il PID è interrogabile, **un processo locale vivo blocca anche la rimozione
per TTL**.

**Regola:** il PID non è mai da solo motivo di rimozione, e la scadenza non
prevale su una prova positiva di vita. Un lock che esiste e non si lascia
leggere non vale come assente: la directory si tiene sempre, e la conta
`kept_conservative`.
**Ambito:** `plenora_engine::temp_store::scavenge_stale_temp_dirs`; la
verifica reale del PID esiste solo su Linux.

**Hazard.**

*Identità di macchina.* L'hostname registrato **non è un'identità**: immagini
clonate, container e host configurati allo stesso modo lo condividono.
Un'esecuzione remota **sospesa** da oltre cinque minuti su un host omonimo con
radice condivisa non è distinguibile da un crash locale.

*L'heartbeat non è un timer.* Lo scrive l'executor ai confini di batch: I/O
bloccata a lungo, ibernazione o salto in avanti dell'orologio possono
invecchiarlo oltre il TTL a esecuzione viva. Su Linux il PID vivo la protegge;
su **Windows e sugli altri Unix il PID non è verificabile**, la scadenza decide
da sola, e la directory di un processo bloccato oltre le 24 ore può essere
raccolta.

*PID riciclati.* Il veto su TTL può poggiare su un PID riassegnato a un
processo estraneo: la directory resta, contata in `kept_conservative`. Si perde
spazio, non dati.

*La decisione e la rimozione non sono atomiche.* La classificazione si ripete
subito prima della `remove_dir_all`, con l'orologio riletto; un heartbeat
rinnovato fra quel controllo e la rimozione non salva la directory. «Un
heartbeat fresco non è mai toccato» è esatta al momento del controllo, non per
l'intera rimozione.

*L'hostname può cambiare.* Il veto del PID vivo confronta l'hostname corrente
con quello del lock: una macchina rinominata a esecuzione in corso rende la sua
directory cancellabile per TTL, benché il processo sia vivo e locale.

*Un heartbeat che fallisce è tollerato, ma solo per cinque minuti.* Un
fallimento singolo della scrittura del lock si ritenta al batch successivo; uno
**persistente** interrompe l'esecuzione con categoria `io` al primo confine di
batch dopo cinque minuti dal primo di una serie di heartbeat falliti
consecutivi. La soglia sta ben sotto
il TTL perché l'errore arrivi prima che un altro avvio possa raccogliere la
directory con dentro lo spill di questa esecuzione.

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
tipo pubblico: con l'array nella firma, `MAX_ANTENATI_OSSERVATI` farebbe parte
dell'API, e cambiarlo sarebbe una rottura.

Serve una gerarchia reale più profonda di otto in cui `Oa` sia risultato
diagnosticamente insufficiente. Otto **non** è una misura: i prototipi non
hanno rilevato la profondità delle gerarchie ospiti, e il campo del
troncamento esiste anche perché quella scelta resti rivedibile.

### Protocollo del worker: i tetti sono del profilo isolato

**La regola.** Ogni frame del protocollo supervisore/worker è un prefisso di
lunghezza `u32` big-endian seguito da JSON UTF-8 compatto. Nessun tetto è
configurabile né viaggia come valore modificabile: un protocollo interno con
limiti negoziabili ha una superficie d'attacco negoziabile. I limiti
elementari — lunghezze di percorsi e identificatori, numero di ingressi, di
capability, di esempi, di messaggi — sono costanti in
`plenora_engine::protocollo::limiti`, dove sta il loro valore;
`MAX_PROTOCOL_FRAME_BYTES` ne è **derivato** con aritmetica `checked` e un
involucro JSON contato carattere per carattere, e sta accanto a chi lo applica
(`protocollo::codifica`). È un maggiorante: nessun frame valido lo raggiunge.

**Il perimetro.** Solo il canale fra supervisore e worker isolato: non tocca
`PlanLimits`, il confine IPC dei dati, né ciò che il motore accetta in-process.

**Il pericolo che copre.** Il decoder legge byte scritti da un altro processo.
Due presidi distinti: in lettura, `protocollo::lettore` decide con
`lunghezza_dichiarata` sui soli quattro byte del prefisso, **prima** di
allocare; in scrittura il writer si ferma appena supera il tetto, invece di
costruire un buffer illimitato e misurarlo dopo.

**Che cosa non garantisce.** `MAX_PIANO_CANONICO_BYTES` è un limite di
**policy**, non un massimo dimostrato: `examples/calibra_canonico.rs` misura il
rapporto fra testo e forma canonica su piani costruiti per massimizzarlo, ed
esce non-zero se la proiezione sul tetto di `PlanLimits::default()` lo supera;
nessun insieme finito di piani fissa quel rapporto per tutti i documenti. Il
presidio è il writer limitato, non il numero.

**La conseguenza.** `max_plan_json_bytes`, `max_inputs` e `max_identifier_bytes`
di `PlanLimits` sono default ampliabili dalla policy di chi esegue, questi tetti
no: un piano valido sotto una policy più larga può essere **non isolabile**, e
riceve `Unsupported` in validazione (`isolamento::attivazione`), mai un errore
di serializzazione.

**Il modulo non è pubblico, nemmeno sotto feature.** Il fuzzer e la sonda di
calibrazione passano da `plenora_engine::interni`, una facciata instabile e
non-production che espone un verdetto e una costante, non i tipi del
protocollo: un modulo pubblico sotto una feature che il fuzzer attiva sarebbe
API pubblica.

**La condizione di rientro.** Alzare un tetto richiede la misura del caso reale
che lo supera e la verifica, con un test, che i massimi serializzati stiano
ancora sotto `MAX_PROTOCOL_FRAME_BYTES`; quei massimi usano caratteri che JSON
espande in `\uXXXX`, o l'espansione degli escape non verrebbe esercitata.
Renderli configurabili è fuori discussione finché il canale resta interno.

### Moduli compilati solo sotto `test` e `internals`

**La regola.** Il codice senza un chiamante di produzione è compilato solo
sotto `test` o la feature `internals`, e il `cfg` lo dichiara **elemento per
elemento**: un `cfg` sul modulo intero direbbe che nessuno lo usa, e di solito
non è vero. Mai un `allow(dead_code)`: un `allow` zittisce l'avviso e lascia il
codice nella build, un `cfg` dichiara la condizione — e se qualcuno aggiunge il
chiamante e dimentica il `cfg`, il compilatore glielo dice. Nemmeno `pub` per
far tacere `dead_code`: allargherebbe la superficie invece di deciderla.

**Il perimetro, oggi.**

| elemento | `cfg` | perché |
|---|---|---|
| `commit_footer::leggi_commit_token` | `test` | fa una traversata propria; il verificatore estrae il token nella **sua** e condivide con lei solo `interpreta_commit_token`, che ha un chiamante di produzione in `pubblicazione::risolvi_commit` |
| la forma breve di `geo_transport::ipc::parse_footer` | `test` | la produzione passa tutta per `parse_footer_estraendo` |
| gli inventari `TUTTE` e `NOMI` dei messaggi del protocollo | `test` | servono ai casi che attraversano ogni variante; la produzione converte una variante per volta |
| il campo `commit_token` di `HandshakeAccettato`, con la copia in `SupervisoreInAttesa` | `any(test, internals)` | la produzione tiene la **propria** copia del token (`esecuzione_isolata`) e la consegna al verificatore; questa la leggono solo i casi |
| `WorkerAccordato::commit_token` | `test` | worker e verificatore ricevono il token da `ricevi_incarico` e `ricevi_incarico_verifica`, insieme all'incarico; l'accessore lo leggono solo i casi dell'handshake |
| `Registro::concluso` e `quattro_fatti_positivi`, `Coda::terminale`, `rimasti` e `chiudi_e_drena` | `any(test, internals)` | introspezione dei casi; il giudizio vero è `classifica`, la conduzione vera `si_puo_smettere_di_ascoltare` e `chiudi_e_drena_entro` |

Il braccio `internals` c'è dove la facciata `interni` porta il fuzzer; sugli
elementi `pub(crate)` la feature non darebbe un chiamante in più, e un `cfg`
più largo del necessario dichiarerebbe una condizione falsa.

**La misura.** `RUSTFLAGS="-D dead-code" cargo check -p plenora-engine` su
**Linux** è pulito: zero diagnostiche, e una sola voce morta lo fa fallire. Su
Windows il codice `cfg(target_os = "linux")` non è compilato, quindi ciò che
solo lui usa resta senza consumatori: quel conteggio è informativo, non
l'oracolo. Un elenco come quello sopra si aggiorna **insieme** alla misura, e
lo produce la misura, non una lettura a mano.

Il `cfg` di piattaforma sta sui soli sottomoduli che toccano il kernel:
l'orchestrazione e i suoi casi provano la procedura, non l'ambiente, e un `cfg`
sul modulo intero li renderebbe verdi per assenza sulle altre piattaforme.

**Il pericolo.** Codice che entra nel binario distribuito senza che nessuno lo
eserciti.

**Che cosa questo perde.** Un elemento sotto `cfg` non è compilato dalla build
di produzione; lo compilano `cargo test` e `cargo clippy --all-targets`, che la
CI esegue.

**La condizione di rientro.** Per ciascun elemento, un chiamante di produzione:
a quel punto il suo `cfg` cade, e la misura lo conferma. Finché non esiste,
non se ne inventa uno.

### Il profilo isolato non descrive l'ambiente `proj-backend`

**La regola.** Con la feature `proj-backend` attiva, il worker **rifiuta prima
dell'handshake**: `protocollo::descrizione::di_questa_build` rende
`PlenoraError::InvalidConfiguration` e nessun `Saluto` viene letto. Il profilo
isolato è quello **senza** backend CRS, e l'`Ambiente` che dichiara ha
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

### `geo.buffer` è strutturalmente inutilizzabile nel profilo isolato, con qualunque CRS

**Non è una scelta di CRS, è una conseguenza del limite qui sopra.** La voce
precedente documenta che il profilo isolato non descrive l'ambiente
`proj-backend`. Questa ne registra una conseguenza operativa concreta,
verificata leggendo il codice e non dedotta: `geo.buffer` richiede
`CrsRequirement::Projected` (catalogo, `docs/operazioni.md`), cioè una
classificazione CRS risolta. Sul percorso **senza** backend —
l'unico compatibile col profilo isolato —
`plenora_core::crs::resolve_crs` (`crs.rs`) fa
`validate_definition_text(definition, name)?; Err(CrsError::BackendUnavailable)`:
convalida solo il testo (lunghezza, non vuoto, nessun NUL) e poi fallisce
**incondizionatamente**, per qualunque stringa di CRS, per quanto nota o
ben formata.

**Non esiste quindi un CRS che faccia funzionare `geo.buffer` nel profilo
isolato.** Non è un problema di scegliere un CRS migliore o più comune: è
strutturale, perché la risoluzione CRS stessa non ha un percorso di successo
senza PROJ, e PROJ non è descrivibile nel profilo isolato per la ragione già
registrata sopra (cache delle griglie sempre attiva, percorsi che si
aggiungono invece di sostituirsi).

**Ambito.** Vale per `geo.buffer` e per qualunque altra operazione del
catalogo con `CRS: projected` o equivalente nella sua scheda — non solo per
`geo.buffer`, che è il caso verificato in questo giro.

**Non risolto in questo giro, e non era lo scopo.** Questa voce documenta il
limite così com'è oggi; la condizione di rientro è la stessa già scritta
sopra per il profilo PROJ (un unico provider che soddisfi le cinque
condizioni). Nessuna modifica al prodotto è stata fatta o proposta qui.

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

### La pulizia del temporaneo si osserva, e ha tre esiti

**La regola.** Dopo il commit point, `publish_with_profile` **accerta** se il
proprio temporaneo sia sopravvissuto, e lo riporta su un asse a sé:
`PuliziaDelTemporaneo`, con tre valori — rimosso; presente, con **percorso e
byte osservati**; non accertabile, con percorso e ragione sanitizzata.

**Perché serve.** `persist_noclobber` non è una primitiva sola. Dove kernel e
filesystem offrono `RENAME_NOREPLACE` il commit è un rename e non lascia
niente; dove non la offrono si ripiega su `hard_link` seguito da `unlink`, e
**l'errore dell'`unlink` è ignorato**. Il no-clobber regge lo stesso — il
collegamento fallisce se il nome esiste — ma il temporaneo può restare senza
che nessuno lo dica, ed è quel silenzio a contraddire la politica per cui un
cleanup fallito è un esito riportato.

**Perché tre esiti e non due.** Perché «non c'è» e «non ho potuto guardare» sono
cose diverse, e un solo valore per entrambe le rende indistinguibili proprio
dove la distinzione serve: chi legge concluderebbe «niente da bonificare» senza
che nessuno abbia guardato. È fail-open, ed è la stessa ragione per cui la
quiescenza di un dominio ha tre esiti.

**Perché non porta la causa dell'`unlink`.** Perché `tempfile` non la espone.
Riportarla vorrebbe dire inventarla: si dice ciò che si osserva, e la ragione
riguarda l'osservazione, non il commit. La distinzione fra `ENOSYS` — che
degrada l'intero processo — ed `EINVAL` — che vale per una chiamata sola —
resta dov'è, dentro `tempfile`, perché il commit resta delegato a lui.

**Dopo il commit nessuno dei due assi è un errore.** L'output è visibile, e
nessun evento successivo può renderlo non riuscito: durabilità non confermata e
temporaneo rimasto sono **avvertenze**. Dirle come fallimenti manderebbe chi
legge a rifare una cosa già fatta, e rifarla troverebbe la destinazione
occupata. Prima del commit, invece, un errore resta un errore.

### Ogni comando che pubblica dice com'è andata la pulizia

**La regola.** Nessun percorso che pubblica scarta l'esito. `run` — sia nel ramo
DAG sia in quello legacy — `transform`, `transform-arrow`, `pair-arrow` e
`spatial-join` emettono `durability_confirmed` e `temp_cleanup` nel proprio
documento di successo.

**Che cosa è cambiato, e perché va detto.** Il ramo legacy di `run` non stampava
niente in caso di successo, e per un giro questa sezione ha registrato quel
silenzio come **limite dichiarato**: l'avvertenza veniva osservata e consumata
da una funzione dal nome esplicito, che però non la rendeva visibile a nessuno.
Registrare un silenzio non lo toglie. Un'esecuzione che lascia un temporaneo e
non lo dice resta un fallimento silenzioso anche se il codice dichiara di
saperlo, e la regola del progetto è che un cleanup fallito sia un **esito
riportato**.

Il ramo legacy emette perciò ora un documento, come il ramo DAG dello stesso
comando: il formato d'uscita è già JSON per contratto — `run` lo impone in testa
— quindi non c'è un canale nuovo, c'è l'uso di quello che il comando dichiara di
avere. La funzione che consumava l'avvertenza non esiste più, perché non ha più
un sito.

### Il percorso del residuo si dichiara nella codifica nativa

**La regola.** `temp_cleanup` porta sempre `path_encoding`, che dice come
leggere il resto: `utf8` col campo `path`, `unix_bytes` o `windows_utf16` col
campo `path_units`, un vettore di interi.

**Perché non `Path::display()`.** Perché è dichiaratamente lossy: sostituisce con
`U+FFFD` ciò che non è testo valido. Un'indicazione di bonifica con un carattere
sostituito indica un file che **non esiste**, ed è peggio di nessuna indicazione:
manda a cancellare il nome sbagliato, o a cercare invano.

**Perché non `OsStr::as_encoded_bytes`.** Perché è una forma **interna**, che la
libreria standard dichiara non specificata: la si può ridare a `OsStr` nello
stesso processo, e nient'altro è promesso. Chi legge il documento quel processo
non ce l'ha, e non ha nessun contratto su come interpretare quegli ottetti.
Riportarli sarebbe dire «esatti» di byte che nessuno sa rileggere.

**Come si ricostruisce.** Con la funzione standard della propria piattaforma:
`OsStringExt::from_vec` sugli ottetti Unix, `OsStringExt::from_wide` sulle unità
UTF-16 di Windows. Sono le codifiche che i due sistemi usano davvero — su Unix
un percorso è una sequenza di byte che non deve essere testo, su Windows una
sequenza di unità UTF-16 che può contenere surrogati spaiati — e il caso che le
verifica **ricostruisce dal documento** invece di riconfrontare col medesimo
encoder, che direbbe soltanto che una funzione è uguale a sé stessa.

### Una chiave riservata di `serde_json` non è ammessa nel JSON di controllo

**La regola.** `$serde_json::private::RawValue` è rifiutata **come chiave**, a
ogni posizione e profondità, in ogni documento JSON di controllo. Come *valore*
stringa è testo qualunque e passa.

**Perché.** `serde_json` riserva quel nome per trasportare JSON grezzo, e quando
è la prima chiave di un oggetto non la legge come una chiave. Con un valore non
stringa **fallisce**; con una stringa di JSON valido è peggio: **riesce**, e
rende un documento diverso da quello scritto.

```text
{"$serde_json::private::RawValue": "{\"a\":1,\"a\":2}"}   →   {"a": 2}
```

Il testo letterale ha una chiave; il documento effettivo ne ha due uguali, già
risolte con «vince l'ultima». È la stessa perdita della chiave duplicata portata
all'estremo — il piano eseguito non è quello scritto — e **aggira il controllo
dei duplicati**, perché la passata vede una chiave sola.

**È una restrizione, non una correzione a costo zero.** Un documento con quella
chiave in seconda posizione, o annidata dove nessuna riscrittura la riordina, non
è ambiguo per nessun lettore, e viene respinto lo stesso. Si restringe perché la
posizione non è una proprietà stabile: la canonicalizzazione riordina le chiavi e
`$` precede ogni lettera, quindi una chiave innocua diventa la prima del testo
canonico — ed è esattamente il percorso su cui `fuzz plan_v5_parse` ha trovato un
canonico che il progetto stesso non rilegge.

**L'escape non è una via d'uscita**: `serde_json` decodifica `\u0024`
prima di consegnare la chiave, quindi la forma con escape è la stessa chiave e
riceve lo stesso rifiuto.

**Ambito.** `plenora_core::json::ensure_no_duplicate_keys`, cioè ogni lettore di
JSON di controllo del progetto.
**Condizione di rientro.** Il giorno che `serde_json` smetta di riservare quel
nome, o offra un lettore che non lo reinterpreta.

### La validazione rifiuta gli anelli con una punta

**La regola.** Un poligono con un anello che torna indietro su se stesso — una
**punta**: tre vertici consecutivi distinti e collineari, col terzo dalla stessa
parte del primo rispetto al centro — non è valido, e
`ValidazioneProtetta::validazione_protetta` lo rifiuta con la ragione
«anello con auto-intersezione», **prima** di chiamare `geo`. Vale per i gusci e
i buchi di ogni `Polygon`, anche dentro `MultiPolygon` e `GeometryCollection`.

**Perché.** La ricerca di auto-intersezioni di `geo` 0.33.1
(`validation::utils::linestring_has_self_intersection`) salta le coppie di
segmenti **adiacenti**, e una punta sta esattamente lì. In un triangolo tutte
le coppie sono adiacenti: un guscio di tre punti collineari, con area zero, per
`geo` è valido. Su di lui `relate` rende matrici prive di senso — un triangolo
degenere che «contiene» un buco esterno — quindi anche la validazione dei
buchi, che di `relate` si fida, accetta poligoni invalidi. In `release`, dove
l'asserzione di `relate` è compilata via, il risultato è sbagliato **in
silenzio**; in debug e nel fuzz è un panico. Il fuzz target `wkt_operations` ne
ha trovati tre reperti, versionati in `tests/anelli_con_punta.rs`.

**Esattezza.** La collinearità è il segno esatto di `orient2d` del kernel
robusto di `geo`; il verso, fra punti già collineari, è un confronto di
coordinate. Nessuna tolleranza: un vertice quasi collineare non è una punta.

**Il perimetro.** Un `Triangle` coi vertici collineari è degenere, e si
giudica con lo stesso segno esatto: la validazione di `geo` usa
`robust::orient2d` direttamente, che a coordinate estreme trabocca in NaN e
lascia passare il triangolo. Un `Rect` resta alla validazione di `geo`:
l'inviluppo di un punto è un `Rect` degenere legittimo. I vertici ripetuti
consecutivi non sono una punta.

**Che cosa cambia per chi legge.** Un poligono con una punta, che prima
passava, è rifiutato come geometria invalida. È fail-closed: la geometria non
era valida per OGC, e il prodotto la trattava come tale solo per il difetto
sopra.

**Che cosa resta a `geo`.** Le altre forme di non semplicità — un anello che
tocca se stesso in un vertice non adiacente — le giudica ancora
`check_validation`, con i suoi limiti.

**La condizione di rientro.** Una versione di `geo` la cui ricerca di
auto-intersezioni veda le sovrapposizioni fra segmenti adiacenti: allora il
controllo diventa ridondante, e i reperti restano come regressione.

### Il WKT è tutto il testo

**La regola.** `construction::geometry_from_wkt` rifiuta con `InvalidWkt` un
testo che contiene un carattere NUL, o che dopo la geometria di primo livello
ha qualcosa di diverso dallo spazio bianco: dopo la parentesi che chiude la
prima aperta, oppure dopo `EMPTY`.

**Perché.** Il parser di `wkt` 0.14 si ferma alla fine della geometria e non
guarda il resto, e il suo tokenizer tratta `\0` come fine dell'ingresso:
`POINT(1 2) garbage`, `POINT(1 2))` e `POINT(1 2)\0resto` diventavano tutti
`POINT(1 2)` senza errore. Un testo che dice di più di quel che viene letto è
malformato, e accettarlo scarta una parte dell'ingresso in silenzio.

**Esattezza.** Il WKT non ha stringhe né commenti: le parentesi sono solo
strutturali, e la fine della geometria si trova contandole. Lo spazio bianco
ammesso è quello del tokenizer: spazio, tabulazione, `\n`, `\r`.

**Le dimensioni.** Allo stesso modo, una Z o una M che il prefisso non mostra —
`POINTZ(1 2 3)`, o un componente `POINT Z(1 2 3)` dentro una
`GEOMETRYCOLLECTION` — si perdevano nella conversione in `geo`, che conserva x
e y. La dimensione si legge ora da ogni nodo di `wkt`, prima della conversione,
e tutto ciò che non è XY è `UnsupportedWktDimension`.

**Che cosa cambia per chi legge.** Un testo con una coda o con una dimensione
nascosta, che prima passava, è rifiutato. Il perimetro è la sola funzione: è
l'unico punto del prodotto che analizza WKT.

**La condizione di rientro.** Un parser WKT che rifiuti da sé i token dopo la
geometria e non tronchi al NUL.

### La validazione OGC sta dietro una barriera

**La regola.** Nessun sito di `plenora-kernels-geo` chiama `check_validation` di
`geo` direttamente: tutti passano da `ValidazioneProtetta::validazione_protetta`,
che cattura il panico e lo rende un errore. I calcoli che passano da `relate` su
geometrie già validate — `point_on_surface`, i predicati spaziali, il predicato
esatto dello spatial join — girano dentro `calcolo_protetto`, che rende
`CalcoloNonConcluso`. Entrambe sono
[barriere di dipendenza](#panici-attesi-nel-fuzzing), e sono condivise: una
barriera per sito è una barriera che qualcuno dimentica.

**Perché.** La `relate` di `geo` costruisce un grafo topologico in virgola
mobile e va in `panic!` quando due conclusioni sullo stesso punto si
contraddicono. Il panico noto è un `debug_assert!`
(`edge_end_bundle_star.rs:116`), assente nel profilo `release`, dove quei casi
diventano un rifiuto ordinario; ma venti righe sotto
`assert!(left_position.is_some(), "found single null side")` è attivo anche in
`release`, e quello la barriera lo copre davvero. Il difetto a monte è una
precondizione: il guardiano guarda un operando solo, mentre il conflitto nasce
dalla coppia. `geo 0.33.1` è l'ultima versione pubblicata.

**Che cosa la barriera non copre.**

- **Il canale `log`.** `geo` pubblica coordinate dell'ingresso con `warn!` e
  `debug!` non condizionati dalle asserzioni di debug: con un logger a livello
  `DEBUG`, anche in `release`, escono dati dell'ingresso. Chi ospita il crate e
  installa un logger deve saperlo.
- **L'hook di panico di `std`**, che stampa il payload **prima** che
  `catch_unwind` lo veda: chi ospita il crate installa la politica sanitizzata
  di `plenora_core::panic_policy`.

**Che cosa porta l'errore.** La *forma* del payload, mai il contenuto: il
messaggio di `geo` nomina le coordinate. E la barriera non è un filtro sui
numeri: rifiutare una classe di magnitudini respingerebbe geometrie valide senza
chiudere il difetto, che nasce dalla precisione su punti molto vicini.

**La distinzione vale fino all'errore finale.** Chi converte l'errore della
validazione guarda la categoria di sotto, e non attribuisce all'ingresso un
difetto che nessuno ha dimostrato: una validazione o un calcolo non conclusi
sono `Internal`, mai `InvalidPlan` o `DataMapping`. La decisione vive in un
punto per strada — `ArrowTransportError::errore_del_passo` per i passi
dell'executor, fuso e non fuso; `causa_di_riga` ed `esito_kernel` per la
misura; `ArrowTransportError::Interno` per il trasporto — perché due copie
divergono.

**Precedenza quando un batch porta più fallimenti.** Un `Internal` prevale su
ogni fallimento ordinario dello stesso batch e propaga **senza** diagnostica di
riga; fra più `Internal` resta il primo in ordine logico di riga, non di
calcolo. La regola è una sola per trasformazione (`collect_cell_failures`) e
misura (`collect_measure_failures`).

**Dentro il fuzz**, l'hook comune dei target tollera il panico che nasce nella
barriera e interrompe su ogni altro. I reperti stanno nei **test versionati**,
con i byte in chiaro (`crates/plenora-kernels-geo/tests/barriera_validazione.rs`,
e il modulo `barriera_privacy_processo` dei test di `src/lib.rs` per ciò che
esce su stderr): `fuzz/corpus` è ignorato da Git e non è una regressione.

**Ambito.** `plenora-kernels-geo`, ogni validazione OGC e ogni `relate`.
**Condizione di rientro.** Il giorno che `geo` non abbia più cammini di panico
raggiungibili da byte esterni, e che i suoi `log` non pubblichino coordinate.

### `geo.coverage_validate` rileva le sovrapposizioni, non i buchi

**La regola.** `coverage_validate` segnala le coppie di poligoni che si
sovrappongono per un'area maggiore della tolleranza. Non segnala i buchi fra
poligoni adiacenti: che un buco sia un difetto o una scelta dipende dal
dominio, e l'operazione non lo sa.

**L'ambito.** `geo.coverage_validate` (`plenora_kernels_geo::extensions3`).

**Il pericolo.** Una copertura con buchi risulta valida. Chi usa l'operazione
come controllo completo di una tassellazione non vede i buchi.

**La condizione di rientro.** Un parametro che dichiari l'area da coprire, o
un'operazione separata per i buchi.

### Semplificazione RDP e scale numeriche miste

**Regola e ambito.** `operations::simplify_with_policy`, per la politica
`DouglasPeucker`, controlla le distanze RDP sulla geometria di lavoro prima di
invocare `geo` — linee, anelli esterni e interni, multi-geometrie, componenti
delle collezioni — ripercorrendo gli stessi segmenti con lo stesso spareggio
`>=`, senza cambiare vertici o tolleranza. **Ogni** distanza deve essere
finita, non solo il massimo: un'altra distanza finita nasconderebbe un `NaN`
nel fold.

**Hazard.** Coordinate finite e validità OGC non assicurano che `dx*dx + dy*dy`
sia rappresentabile, e la normalizzazione globale non basta quando componenti
molto piccole convivono con componenti di scala ordinaria: un segmento con
estremi distinti può avere denominatore arrotondato a zero, e `0/0` dà `NaN`.
In `geo 0.33.1`, se tutte le distanze intermedie sono `NaN`, il massimo
conserva l'indice zero: in debug raggiunge il `debug_assert_ne!` in
`simplify.rs:108`, in release il ramo di eliminazione può scartare un vertice
oltre la tolleranza richiesta. Il denominatore replica i prodotti e la somma
separati di `geo-types 0.7.19`: l'eccezione locale a `clippy::suboptimal_flops`
evita che un `mul_add` cambi gli arrotondamenti della precondizione verificata.

**Rifiuto esplicito.** Denominatore nullo per estremi distinti, denominatore
non finito o distanza non finita rendono
`OperationError::Internal("semplificazione: distanza non rappresentabile")`,
senza attribuire invalidità all'ingresso OGC-valido, pubblicare coordinate o
restituire componenti parziali. La categoria `Internal`, senza diagnostica che
attribuisca il difetto alla riga, si mantiene nelle conversioni del trasporto e
delle misure fuse/blocking e nelle varianti interne dell'algoritmo esteso e del
join spaziale. La tolleranza zero non calcola distanze. Il vendor resta
invariato: la protezione è al confine del prodotto, non una correzione
upstream.

**Reperti riproducibili.** `tests/simplify_scale_miste.rs` contiene una
`MultiLineString` finita con componente `[(0,0),(0,1e-200),(1e-200,0)]` e
componente ordinaria `[(1,1),(2,2)]`, anche costruita dal parser WKT del
prodotto: con tolleranza `1e-220` il vertice intermedio (distanza `1e-200`)
deve restare. Il vendor senza presidio panica in debug e in release rende i
soli estremi; il prodotto deve rendere il rifiuto tipizzato in entrambi i
profili. Un secondo reperto usa due poligoni finiti a scale diverse; gli
oracoli ordinari confrontano i vertici con quelli di `geo`, spareggi inclusi.

**Limite storico.** Il WKT del finding fuzz del 2026-09-01 non è disponibile:
questi sono nuovi reperti della stessa classe, non un replay. La prova isolata
con NaN letterali non dimostra la raggiungibilità dal prodotto e non giustifica
il presidio.

**Costo e non-garanzie.** Il controllo aggiunge una traversata RDP, con stack
esplicito O(n), prima di quella della dipendenza; non è una misura di costo
applicativo né una prova di aritmetica esatta di RDP. Non modifica né qualifica
`PreserveTopology`, la normalizzazione delle coordinate o gli altri kernel che
calcolano distanze.

**Condizione di rientro.** Sostituire la doppia traversata richiede un
percorso fallibile che controlli ogni distanza durante il calcolo, oppure una
dipendenza corretta e riqualificata; reperti, rifiuti tipizzati e oracoli
restano obbligatori. Lo stato della qualifica è in
[`stato-e-roadmap.md`](stato-e-roadmap.md).

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

Scelte che si allontanano dalla forma ovvia, ciascuna con un rientro proprio,
nella sequenza di
[`isolamento.md`](isolamento.md#9-bis-preflight-del-dominio-scrivere-non-e-configurare).

**1. Sui descrittori si verifica e si rifiuta, non si chiude.** Un descrittore
già aperto in scrittura sul filesystem del control plane sopravvive al cambio
d'identità: i permessi si controllano all'apertura, non a ogni scrittura.
Chiuderne uno ereditato per numero richiede di costruirne un proprietario da un
intero grezzo, e ogni via è `unsafe`, che questo progetto non ammette.
Rifiutare è fail-closed; chiuderlo di
nascosto nasconderebbe che qualcuno lo ha passato. *Rientro:* una via sicura
per chiudere un descrittore ereditato, o una decisione esplicita sul perimetro
`unsafe`.

**2. Lo spawner deve essere monothread, e lo accerta.**
`rustix::thread::{set_thread_groups, set_thread_res_gid, set_thread_res_uid}`
sono i syscall **per-thread**: a differenza del wrapper di glibc non propagano
le credenziali agli altri thread, che in un processo multithread resterebbero
privilegiati. Il primo passo della sequenza conta `/proc/self/task` e rifiuta
se i task non sono esattamente uno: **queste API valgono solo lì**, dove la
`exec` conserva le credenziali del chiamante. *Rientro:* una API sicura che
cambi le credenziali del processo intero.

**3. Il passo 7 rilegge le credenziali, non l'identità intera.** Fra il cambio
d'identità e la `exec` il kernel azzera *dumpable* e passa `/proc/<pid>` a
root: `ns` non è più attraversabile dal processo stesso. Namespace e
descrittori si portano avanti dal passo 4, lecitamente perché in mezzo stanno
solo `prctl`, `setgroups` e le `setres*id`, che non aprono descrittori né
cambiano namespace. Rimettere *dumpable* aprirebbe il `ptrace` a un altro processo dello
stesso uid. *Rientro:* nessuno previsto; la misura sta nel modo `finestra` di
`scripts/verifica_isolamento_linux.sh`, e un kernel che concedesse la lettura
anche lì renderebbe la scelta non obbligata, non sbagliata.

**4. I due estremi del canale si riaprono, e gli ereditati restano aperti.** Il
worker riceve i due estremi come **numeri**, e adottarli richiede `unsafe`
(`OwnedFd::from_raw_fd`, `BorrowedFd::borrow_raw`): li riapre da
`/proc/self/fd/<n>`, ottenendo descrittori posseduti e nati `CLOEXEC`. Gli
ereditati restano aperti e **non** sono `CLOEXEC`, perché il supervisore lo
toglie per farglieli ereditare: è misurato che, caduto il descrittore
riaperto, il numero originale nomina ancora la stessa pipe.

**La conseguenza è una non-garanzia.** «Il worker non avvia altri processi» è
un'**invariante operativa**, non qualcosa che il codice impedisce: un processo
avviato dal worker erediterebbe quei descrittori, e un estraneo che tenesse
aperto l'altro capo impedirebbe per sempre l'EOF del canale. Che i riaperti
siano puliti non dice nulla degli ereditati. *Rientro:* lo stesso della
deviazione 1.

**A quale condizione questa non-garanzia è accettabile.** Una pipe trattenuta
da un discendente deve poter produrre *un ritardo*, mai *un risultato
sbagliato*. Lo
vincola la macchina a stati del supervisore:

1. **`Esito` da solo non autorizza il successo**: è un'affermazione del worker,
   non una prova.
2. **Servono anche le altre tre**: la morte del worker *e la sua raccolta* (un
   processo raccolto male resta zombie), l'**EOF** del canale (nessuno tiene più
   l'altro capo), la **quiescenza del dominio** (nel cgroup non è rimasto
   nulla). Nessuna delle quattro sostituisce le altre.
3. **Un discendente che trattiene la pipe porta a timeout o incompletezza, mai
   a pubblicazione**: l'esito dice che non si è potuto concludere, non che si è
   concluso bene.

### La proprietà del figlio non si scarica su una riga di rapporto

Fra lo `spawn` e la consegna al chiamante il figlio sta sotto una **guardia**:
ogni uscita voluta lo passa a qualcuno o lo chiude, e le porte si chiamano per
ciò che fanno — consegna, attesa, chiusura, arresto. Nessuna garantisce di
riuscire: un processo che non si lascia raccogliere entro il limite **esiste
ancora**, e qualcuno deve restarne responsabile. **Non esiste una porta che
rinuncia e prosegue**, e non deve esistere (pid nel rapporto, difetto accanto,
avanti): la riga
lascerebbe vivo un processo che nessuno aspetta né raccoglie, mentre il
supervisore dichiara di aver finito. Le vie sono due, nessuna silenziosa:

1. **la guardia risale** a chi può ancora riprovare: la conduzione la porta
   fuori nel proprio contorno, insieme al difetto che dice perché, e riprovare,
   farla risalire o fermarsi è scelta del chiamante. Se la lascia cadere senza
   deciderla, la sentinella `Drop` è ancora armata: la proprietà non si perde
   mai in silenzio, si perde con un abort;
2. **ci si arrende**, quando sopra non c'è nessuno: è il caso dello spawner, che
   rende un errore tipizzato, e un errore non tiene un processo. La riga è
   **diversa** da quella della sentinella: la sentinella dice «sfuggito» e manda
   a cercare un cammino che non passa da nessuna porta; la resa dice «non si è
   lasciato raccogliere, e nessuno può riprovare» e manda a guardare il figlio.

**La resa manda la terminazione, e riporta che cosa ha potuto fare.** `abort`
ferma **noi**, non il figlio, che passerebbe al reaper del sistema
sopravvivendo al supervisore: la resa, come la sentinella, tenta esplicitamente
la terminazione prima di fermarsi.

**Nessuna delle risposte dice «terminato».** Mandare la terminazione non è
osservare l'uscita (nel cammino ordinario `termina()` è seguita da
`prova_a_raccogliere`): un processo in attesa ininterrompibile sopravvive a un
`SIGKILL` accettato finché la chiamata di sistema non ritorna, e un log che
affermasse di più farebbe smettere di cercarlo.

| risposta di `termina()` | ciò che si riporta |
|---|---|
| `Ok(())` | segnale di terminazione inviato; uscita non osservata |
| `InvalidInput` | processo non più terminabile; uscita non osservata |
| altro errore | segnale non inviato (*motivo*); può restare vivo |

Nemmeno `InvalidInput` autorizza «già uscito»: è compatibile con un figlio
finito ma non lo prova. Quel percorso non raccoglie il figlio, e non è una dimenticanza: dopo l'`abort`
nessuno può più aspettare: un figlio non raccolto passa al reaper del sistema, e la riga deve
nominare l'altro esito, un figlio a cui **non** è arrivato niente.

*Rientro:* la seconda via scompare quando lo spawner avrà sopra di sé un
responsabile del ciclo di vita capace di **tenere una guardia** — un chiamante
che riceva `FiglioVivo` invece di un errore tipizzato. È una condizione
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

### Un fallimento prima della conduzione passa dall'evidenza del dominio

**La regola.** L'istantanea «prima» dell'evidenza si prende dal dominio
preparato, **prima dello spawn**: il tipo `EvidenzaDaPrimaDelloSpawn` si
costruisce solo da un `&DominioPreparato`, che lo spawner consuma. Ogni
fallimento fra lo spawn e la conduzione — il supervisore che non si costruisce,
l'handshake, l'incarico che non si scrive, il canale operativo che non si apre
— chiude il figlio e poi rilegge la propria causa attraverso l'evidenza: un OOM
attribuito diventa `ResourceLimit`, una pressione osservata
`UnattributedMemoryPressure`, un'evidenza incoerente `Internal`.

**Perché.** Un worker ucciso per memoria mentre saluta si presenta come un
canale chiuso, e la causa del dialogo direbbe «isolamento non disponibile». Con
l'istantanea presa dopo l'`Incarico`, poi, un OOM avvenuto nel frattempo finiva
nel «prima» e il delta lo cancellava.

**Il perimetro.** L'evidenza si legge solo a dominio quiescente, come nella
conduzione (`F4-10`). Se la quiescenza non arriva entro l'attesa della
conduzione (500 ms), il dominio si termina con `cgroup.kill` e si riattende,
come prescrive la §10.3 di [`isolamento.md`](isolamento.md). La lettura
dell'evidenza decide per prima: un OOM attribuito è `ResourceLimit`, una
pressione non attribuita `UnattributedMemoryPressure`, un'evidenza incoerente
`Internal`. Altrimenti, se è servito `cgroup.kill`, l'esito è ambiguo e diventa
`Internal` — tranne quando la causa del dialogo è già un fatto accertato
dall'handshake (`Protocol` o `InvalidConfiguration`, righe 9 e 10), che una
quiescenza tardiva non rende ambiguo. Una cancellazione osservata prima della
conduzione cede al solo OOM attribuito.

**Il pericolo.** Un dominio che non si svuota **nemmeno** dopo `cgroup.kill`
non si legge: l'errore è `Internal` e dice che processi possono essere rimasti
nel dominio, e la rimozione della directory del dominio fallisce e lo riporta
su stderr.

**La condizione di rientro.** Una conduzione che cominci allo spawn invece che
dopo l'handshake, e sorvegli quindi l'intero intervallo con la stessa macchina.

### Un worker orfano finisce il proprio lavoro

**La regola.** Se il coordinatore muore dopo lo spawn, il worker non se ne
accorge. La chiusura del canale dopo l'`Incarico` non è un annullamento
(`Ascoltato::FineDelCanale`: un supervisore può chiudere la propria direzione
senza voler annullare), e il controllo sui namespace del padre
(`identita::namespace_del_padre`) passa anche con un padre adottato da `init`.
Il worker prosegue fino alla fine dentro il proprio dominio, e il suo esito non
lo legge nessuno.

**L'ambito.** Il profilo isolato su Linux, worker e verificatore.

**Il pericolo.** Un processo che consuma CPU, e memoria fino al tetto del
dominio, senza timeout — lo applica il coordinatore, che non c'è più — e un
dominio `cgroup2` che nessuno rimuove. Nessun output è pubblicato: la
pubblicazione è del coordinatore.

**La condizione di rientro.** Rilevare la morte del supervisore — pid con
start-time, un pidfd, o `PR_SET_PDEATHSIG` — e trattarla come un annullamento.

### La pulizia dei domini non esce su un canale machine-readable

**La regola.** La riga 16 della matrice di [`isolamento.md`](isolamento.md)
chiede che un cleanup fallito dopo un publish riuscito sia un successo **con
avvertenza machine-readable**. Il documento di `run` la dà per il temporaneo
della pubblicazione (`temp_cleanup`). Non la dà per le directory dei due domini
`cgroup2` e per la directory di lavoro del worker.

**Il perimetro.** Un dominio che non si rimuove (`remove_dir` sul cgroup del
worker o del verificatore) finisce su **stderr**, e l'esecuzione resta un
successo. La directory di lavoro del worker è una `tempfile::TempDir`, e il suo
`Drop` scarta l'errore di rimozione: un residuo lì non lo dice nessuno.

Lo store dell'executor del worker (`plenora-exec-*`, con il suo `lock.json`)
sta nella directory temporanea del sistema, di proprietà dell'identità del
worker, e si toglie al `Drop`. Un worker terminato da fuori — OOM, timeout,
cancellazione, `SIGKILL` — non arriva al `Drop`, e lo store resta. Lo scavenging
all'avvio di un'esecuzione successiva sulla stessa radice **può** raccoglierlo,
quando il PID registrato non esiste più e l'heartbeat è fermo da oltre cinque
minuti
([scavenging temporaneo](#scavenging-temporaneo-comanda-lheartbeat-il-pid-può-solo-accelerare)):
è best-effort, e l'executor ne scarta il resoconto. Serve il permesso di
rimuovere lo store, quindi la stessa identità del worker o root: un'esecuzione
con un'altra identità non privilegiata lo lascia dov'è, e il residuo può restare
a tempo indeterminato. Nessun campo dell'envelope lo riporta: `temp_cleanup`
descrive il temporaneo della pubblicazione, non questo.

**Il pericolo.** Un chiamante automatico che legge solo stdout non vede i
residui: cgroup vuoti che si accumulano sotto la radice delegata, file
dell'artefatto temporaneo e store dell'executor del worker sotto la directory
temporanea del sistema, che uno scavenging successivo raccoglie solo se ha i
permessi per farlo.

**La condizione di rientro.** Campi propri nel documento di `run` per i
residui dei domini e della directory di lavoro, nella forma di `temp_cleanup`
(stato sempre presente, percorso nella codifica nativa), la rimozione
esplicita della directory di lavoro al posto del `Drop`, e uno store
dell'executor del worker che il coordinatore conosce e rimuove dopo una
terminazione da fuori.

### I quattro tempi del supervisore

Sono limiti **governati**: costanti nominate e motivate nel codice, dove sta il
loro valore. Nessuno è una stima di quanto ci vuole: i primi tre segnano il
punto oltre il quale aspettare smette di essere un'attesa, il quarto è il passo
con cui si guarda un interruttore.

| tempo | costante | che cosa separa | allo scadere | rientro |
|---|---|---|---|---|
| margine di cortesia | `MARGINE_DI_CORTESIA` | «ci ha messo un momento» da «non è successo», fra la decisione di chiudere (tempo scaduto, cancellazione) e la quiescenza | si forza con `cgroup.kill`, e ciò che non muore è un fatto da riportare | nessuno previsto |
| attesa della quiescenza | `ATTESA_DELLA_QUIESCENZA` | l'effetto di `cgroup.kill` dalla sua richiesta: l'evidenza di un dominio non vuoto è la fotografia di qualcosa che si muove (il prototipo ha visto l'OOM arrivare dopo la `wait`) | l'evidenza **non si legge** e la barriera si dichiara incompleta | una notifica di svuotamento attendibile dal kernel |
| drenaggio della coda | `TETTO_DEL_DRENAGGIO` | una chiusura che finisce da una chiusura sbagliata (una sorgente non terminabile, una bocchetta dimenticata), che altrimenti aspetterebbe per sempre | i fatti già drenati restano nel rapporto, accanto al difetto | nessuno previsto |

Il drenaggio si ferma solo su `Disconnected`, mai su un istante vuoto: un
produttore vivo può ancora accodare, e i fatti dell'ultimo istante sono quelli
che dicono com'è finita. La sua scadenza è **assoluta** — non riparte a ogni
fatto, o un produttore che invia poco prima di ogni scadenza lo terrebbe aperto
— e si calcola con `checked_add`: un'impossibilità si osserva, non si presume.

**L'arresto del lettore.** Un lettore fermo dentro una `read` non si sveglia
perché altrove cade un mandante: il descrittore è non bloccante e la lettura
passa da un adattatore che guarda un interruttore.

- **Garantito:** l'interruttore si guarda prima di ogni lettura e di ogni
  attesa, anche dopo un `Interrupted`; l'adattatore non chiede mai un'attesa più
  lunga di `PASSO_DI_ATTESA`; dopo `INTERRUZIONI_PRIMA_DEL_RESPIRO`
  interruzioni consecutive chiede un passo, così una sorgente che interrompe
  sempre non occupa un core. Un caso lo prova contando i giri, non l'orologio.
- **Non garantito:** un limite di tempo reale. `sleep` promette di non
  risvegliarsi prima, non di non risvegliarsi dopo, e senza uno scheduler
  real-time decide il kernel. Chi vuole un tetto sull'orologio lo prende più in
  alto — un timeout, un `SIGKILL`.
- **Solo qualificazione:** la soglia ambientale dei casi sta sotto `cfg(test)`,
  perché una costante pubblica prima o poi si legge come una promessa. Se il
  caso che la usa fallisce, la prima ipotesi è la macchina.

*Rientro:* un modo sicuro di interrompere una lettura bloccante senza
sondaggio, che toglierebbe il passo, la soglia e questa distinzione insieme.

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
**minuscoli**, controllati in un punto solo, il costruttore: non esiste un
`CommitToken` non valido, e chi ne ha uno non deve validarlo. La
rappresentazione è opaca e la forma testuale si **ricostruisce** dai byte, così
coincide con quella del footer, dove il token vive sotto una chiave sola:
`plenora.commit.token`.

**Il perimetro.** I quattro confini che il token attraversa: il chiamante che
lo fornisce, l'handshake, il writer del footer, il verificatore. La regola vale
su tutti e quattro perché è nel tipo, non nei quattro punti.

**Il pericolo che copre.** Due, distinti:

- **due grafie dello stesso valore.** Il footer si confronta byte per byte, e
  `ABC…` accanto a `abc…` darebbe due artefatti per lo stesso commit: la forma
  non canonica si **rifiuta**, non si normalizza;
- **il valore in un log.** `Debug` e `Display` non lo mostrano e nessun errore
  lo contiene: `Debug` finisce in un log per sbaglio, dentro il `{:?}` di una
  struttura più grande. L'errore del footer non canonico è un `&'static str`:
  è il tipo a non consentire di portarci dentro il valore. La serializzazione
  invece lo emette, per scelta: sul filo serve, in un log no.

**Le quattro forme del footer.**

| forma | esito |
|---|---|
| assente | **legittimo** — un artefatto ordinario non ha un token |
| canonico | accettato |
| presente ma non canonico | **rifiutato sempre**, in ogni percorso |
| chiave duplicata | rifiutato dalla traversata rinforzata |

Il duplicato arriva solo da un produttore estraneo (`FileWriter::write_metadata`
tiene le coppie in una mappa, e due scritture della stessa chiave collassano),
e lo rifiuta il parser. Che il token sia **obbligatorio** è una proprietà del
percorso isolato, non della lettura: `leggi_commit_token` dice cosa c'è, non se
doveva esserci.

**Come si legge, e come non si legge.** Il token si estrae dalla **stessa
traversata** che convalida il footer, non da `FileReader::custom_metadata`, che
salterebbe i controlli del parser rinforzato (allocazione limitata, chiavi e
valori presenti, duplicati rifiutati): un valore autoritativo raggiungibile
senza convalida è peggio di un valore assente.

**Senza token i byte non cambiano.** Con `None`, che il percorso in-process
passa sempre, non si scrive nulla, nemmeno una chiave vuota, che cambierebbe
ogni artefatto già prodotto.

**La condizione di rientro.** Cambiare la forma canonica — lunghezza,
alfabeto, grafia — significa cambiare insieme la chiave del footer: due grafie
sotto lo stesso nome non sono distinguibili da chi rilegge. Mostrare il valore
in `Debug` non ha condizione di rientro: se serve, esiste già
`in_esadecimale`.

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
tetto cumulativo del confine, perché i dizionari sono l'unica cosa che il
lettore trattiene tutta insieme (`FileReader` li decodifica in `try_new` e li
tiene per l'intera scansione, uno `StreamReader` accumula quelli che incontra):
mille dizionari da un megabyte rispettano il tetto per singolo body e insieme
trattengono un gigabyte. Si applica in **due punti complementari**, nessuno dei
quali sostituisce l'altro:

| dove | che cosa vede |
|---|---|
| la traversata dei messaggi | i `DictionaryBatch` incontrati percorrendo la regione dati — vale per lo stream, per il file e per ogni lettore ostile |
| i blocchi del footer | quelli che `FileReader` leggerà **davvero**, saltando agli offset senza percorrere la regione |

Il superamento rende sempre `IpcRetainedDictionariesTooLarge`, con la somma
vera e il tetto; il **trabocco** della somma non porta nessun numero, e la
variante dipende da **dove** avviene:

| percorso | trabocco |
|---|---|
| traversata dei messaggi | `IpcTruncated`: si sta percorrendo la regione dei messaggi, e un footer può non esserci affatto — lo stream non ne ha uno |
| conteggio dei blocchi del footer | `IpcFooterInvalid`: lì il footer c'è per definizione, ed è la struttura incoerente |

**Il framing invalido vince sul tetto.** `validate_footer_blocks` **precede**
`verifica_tetto_dizionari`: sommare i `bodyLength` di blocchi troncati,
disallineati o fuori dalla regione dati classificherebbe un file
strutturalmente rotto come `IpcRetainedDictionariesTooLarge` invece di
`IpcFooterInvalid`, e manderebbe ad alzare un tetto che nessun tetto può
salvare.

**La sovrastima dichiarata.** La traversata somma **tutti** i
`DictionaryBatch`, anche quando lo stream **sostituisce** un dizionario già
visto (stesso `id`, valori nuovi) e Arrow ne tiene uno solo: uno stream con
molte sostituzioni può essere rifiutato pur restando entro il consumo reale. È
**conservativa e voluta**: sommare per `id` vorrebbe dire fidarsi che il
lettore a valle rimpiazzi come previsto. La condizione di rientro: se un carico
reale usa sostituzioni ripetute, la somma diventa per `id`, provando — non
assumendo — che la regola di rimpiazzo coincide con quella del lettore.

**I dizionari delta sono rifiutati.** Con `isDelta` Arrow concatena il
dizionario precedente e il nuovo in un buffer ulteriore, mentre entrambi gli
originali sono vivi: il picco si avvicina al **doppio** della somma, e la
formula della memoria trattenuta di [`isolamento.md`](isolamento.md) sarebbe
falsa proprio quando il tetto dice che va tutto bene. Il rifiuto, in
prevalidazione e comune a tutti i lettori, è una **deviazione dal formato**: un
delta è un ingresso Arrow stream valido. Nessuno dei nostri produttori lo
genera (il `FileWriter` non ne emette), ma un lettore esterno che ne mandasse
uno sarebbe rifiutato senza aver sbagliato niente. Il rientro non è «alzare il
tetto»: è rifarlo sul picco della concatenazione.

**`header` e `data` sono obbligatori.** Un messaggio che dichiara
`header_type` senza `header` salterebbe ogni controllo della prevalidazione
fino all'`unwrap()` di Arrow; un `DictionaryBatch` senza `data` fa lo stesso
dentro `read_dictionary`. Entrambi sono rifiutati: la barriera anti-panico tradurrebbe il panico in
errore, ma è l'ultima difesa, non la prima.

**Il pericolo che copre.** Un ingresso che dichiara molti dizionari piccoli, o
un delta, e fa trattenere al lettore molto più di quanto qualunque tetto
per-messaggio ammetta — con la formula della memoria che continua a dire che
il picco è limitato.

### Fuzzing su toolchain nightly

Il solo step `cargo fuzz run` gira su nightly, mentre build, test, clippy e
gate anti-panic restano sulla toolchain pinnata. Un crash conta **solo se
riproducibile sulla pinnata**: riproduzione e minimizzazione avvengono lì
prima di aprire una correzione.

### Candidato geo esatto (kernel sempre-esatto, senza filtro): il costo è misurato sul join spaziale, non una qualifica generale

**La misura.** Il candidato vendorizzato `geo-0.33.1-exact` (diff 1,
orientamento esatto — vedi `vendor/geo-0.33.1-exact/PROVENANCE.md`) è stato
confrontato col predicato originale su un benchmark applicativo mirato: join
spaziale reale (`spatial_join_nullable_validated`), predicati
`Overlaps`/`Touches`/`Crosses`, caso positivo e negativo, due densità di
candidati, `N = 4000`, un riscaldamento scartato più sette ripetizioni
verificate. Il rapporto candidato/baseline misurato va da **3,06× a 5,94×**
a seconda dello scenario; l'esecuzione totale del candidato è stata 1,466s
contro 0,361s del baseline. Nei 16 scenari le coppie prodotte sono risultate
**identiche byte per byte** fra i due rami: nessuna divergenza di
correttezza, solo di tempo.

**Il perimetro della misura — dichiarato perché non sia letto oltre.** Non
è una qualifica prestazionale generale del candidato, né una soglia
universale. Copre esclusivamente: il join spaziale (non le singole chiamate
a `relate`/predicati punto-per-cella, misurate a parte nel laboratorio —
vedi sotto), i tre predicati elencati, `N = 4000` con selettività bbox nota
per costruzione, la macchina locale su cui è girato il benchmark. Non copre
gli altri kernel del pacchetto (`voronoi_cells`, `dissolve`, buffer, …), gli
altri predicati (`Intersects`, `Contains`, `Within`), altre scale di `N`, né
l'esecuzione nella VM di CI.

**Continuità con la misura di laboratorio.** Il laboratorio aveva già
misurato il costo per-chiamata isolato di `relate` (circa 1,03–1,20
µs/chiamata, fino a 705,5× il baseline nel microbenchmark ordinario —
`handoff-final-20260909/CONSEGNA-DATA-TOOLS.md`, punto 2): un numero di
throughput di libreria, non applicativo. Questa misura lo completa sul
carico che il prodotto esegue davvero, e i due non sono la stessa grandezza:
un rapporto di libreria enorme può tradursi in un rapporto applicativo molto
più piccolo quando il costo per chiamata è una piccola frazione del lavoro
totale del join (indicizzazione R-tree, allocazione delle coppie, I/O).

**Dove sono i dati.** `benchmarks/join/geo_join_predicates.jsonl` (candidato)
e l'albero gemello di confronto (baseline: stesso file, stesso schema,
`geo-0.33.1-exact` costruito con `logging.patch` soltanto, senza
`geo-exact-orientation.patch`) — entrambi locali, non nel repository del
prodotto. Output precedenti conservati in `benchmarks/join/precedenti/`. Il
generatore è `crates/plenora-kernels-geo/examples/bench_geo_join_predicates.rs`,
un binario `example`, non incluso nel binario di produzione.

**Perché è accettato.** Il predicato originale è dimostrabilmente sbagliato
su un sottoinsieme di ingressi (auto-intersezioni all'orientamento
sbagliato — la ragione stessa del diff 1): tornare indietro scambierebbe un
costo misurato con un errore silenzioso.

**Condizione di rientro.** Non è una deroga con una condizione di chiusura:
è un costo accettato come compromesso per procedere verso la qualificazione
del candidato esatto. Rivederlo — verso una misura più ampia sugli altri
kernel — resta lavoro futuro distinto, non anticipato né sostituito da
questa nota. Un percorso rapido con fallback esatto e limite d'errore
dimostrato non è più assente: esiste come candidato sperimentale separato,
vedi la sezione seguente.

### Candidato filtrato sperimentale: la regressione O(n²) su buffer e validazione, e il percorso rapido proposto

**Da dove nasce.** Il kernel sempre-esatto sopra chiama
`exact_orientation::orient2d_sign_bits` (aritmetica intera a 4224 bit,
nessun percorso rapido) su OGNI confronto di orientamento, senza eccezioni.
Per un predicato di join il numero di chiamate è piccolo e il costo resta
nell'ordine di 3-6× (misura sopra). Per operazioni che invocano
l'orientamento molte volte per geometria — la validazione OGC in ingresso a
ogni kernel, `buffer` — il conteggio delle chiamate cresce quadraticamente
col numero di vertici, e il fattore costante si traduce in una regressione
di ordini di grandezza. Isolata con
`crates/plenora-kernels-geo/examples/diag_geo_scaling.rs` (`validazione_sola`,
`buffer`, `simplify`, `centroid`), a parità di forma di crescita O(n²) su
entrambi i rami (nessun cambio di classe di complessità, solo del fattore
costante).

**Un limite dichiarato sulla misura, non sulla diagnosi.** L'albero candidato
usato per questa misura aveva, al momento della compilazione, un
`Cargo.lock` con 32 pacchetti transitivi e una nuova crate (`zlib-rs`) a
versioni diverse da quelle dell'albero di confronto — deriva accidentale di
un comando `cargo` non vincolato eseguito durante questa integrazione,
indipendente dal filtro, dichiarata e corretta (`Cargo.lock` riportato allo
stato del commit di base più le sole tre sostituzioni vendorizzate del
`[patch.crates-io]`). Fra i pacchetti derivati, `geo-types` e' raggiungibile
dal percorso geometrico (dipendenza diretta di `geo`, verificato con
`cargo tree`): la lettura del sorgente del kernel stabilisce la causa
qualitativa (la chiamata incondizionata all'aritmetica esatta a 4224 bit),
ma non isola quantitativamente il contributo delle dipendenze cambiate sul
rapporto misurato — e gli orari dei file usati per ricostruire la sequenza
non certificano quali versioni fossero effettivamente compilate nel binario
che ha prodotto quel numero. Il rapporto riportato qui (ordini di
grandezza, non una cifra unica) resta quindi la misura di quel confronto
specifico: indicativo della causa, non certificato a `Cargo.lock` corretto
e non ripetuto.

**Il filtro.** Progettato, derivato con disuguaglianze esplicite (lemma di
Higham, limite di errore provato `< 4,4u·S + e₀`) e qualificato nel banco
standalone `plenora-memlab-filtro-sperimentale/`: 6788 casi contro l'oracolo
razionale (`fractions.Fraction`), zero fallimenti in debug e in release, un
controesempio reale trovato e corretto durante la qualifica (sottoflusso di
prodotto trattato come zero genuino). Certifica il segno con un limite
d'errore dimostrato quando l'ingresso lo consente, e ricade sullo stesso
kernel sempre-esatto — invariato — in ogni altro caso: vedi
`vendor/geo-0.33.1-exact-filtered/PROVENANCE-FILTRO-SPERIMENTALE.md` per la
derivazione e la provenienza complete.

**Stato in questo albero.** `Cargo.toml` qui risolve `geo` al vendor
filtrato (`vendor/geo-0.33.1-exact-filtered`), non al candidato sempre-esatto
descritto sopra: `scripts/verifica_risoluzione_vendor.py` in questo albero
lo pretende esplicitamente, a differenza della copia dello stesso script
nell'albero del candidato congelato. Con il filtro, il test prima
catastrofico (`diag_geo_scaling`/la suite completa) converge: 67-71s in
debug, 1,75-1,8s in release, misurato qui — non dedotto dalla derivazione
numerica. Il benchmark del join spaziale sopra (3,06×-5,94×) descrive il
kernel sempre-esatto SENZA filtro e non è stato ripetuto col filtro: il
filtro si applica anche a quel percorso, quindi il rapporto atteso con il
filtro attivo è minore, ma resta una previsione, non una misura.

**Stato dell'adozione.** Sperimentale. Nessuna sostituzione del candidato
congelato, commit, push o VM senza autorizzazione esplicita.

### Fallimenti upstream comuni e divergenze GEOS: non risolti da questa integrazione

**Fallimenti upstream comuni.** La suite di test di `geo-0.33.1-exact` non è
interamente verde, né sulla base né sul candidato: `test_polygon_densify`,
`test_non_standard_geoid` e il doctest `GeodesicMeasure` falliscono
**identici in entrambi i rami**. Non sono causati dal diff 1 né dal diff 2
(stesso esito prima e dopo la patch): sono fallimenti della libreria
upstream indipendenti da questa integrazione. Log conservati nel laboratorio
(`R/windows/correctness-followup-01`), non ripetuti né riportati qui.

**Perché non sono stati "risolti" qui.** Correggerli richiederebbe
modificare codice di `geo` estraneo ai diff 1/3/5 vendorizzati — fuori dal
mandato di questa integrazione, che lascia il candidato numerico invariato.
Restano un difetto noto della versione upstream, non della patch.

**Divergenze GEOS.** Il confronto fra `geo` (candidato esatto) e GEOS su un
insieme di reperti mostra divergenze le cui cause interne **non sono
spiegate**. L'accordo fra il candidato e l'oracolo razionale sui reperti
esaminati **non risolve**, e non va letto come se risolvesse, la divergenza
generale fra le due librerie: sono osservazioni su un insieme finito di
casi, non una prova di equivalenza. Audit conservati nel laboratorio
(`R/windows/independent-geos-01`).

**Condizione di rientro.** Nessuna, dichiarata come tale: sia i fallimenti
upstream sia le divergenze GEOS restano aperti finché non esiste
un'indagine dedicata — fuori dal perimetro di questa integrazione.

## Il verificatore

`scripts/verifica_memoria_governata.py` verifica che i siti di allocazione e
prenotazione descritti in questo documento esistano ancora **nella forma
descritta**, e che i pattern eliminati non riappaiano. Un elenco del genere
marcisce in silenzio: basta che qualcuno sposti una reservation e il documento
resta convincente e falso. I comportamenti del governor e della consegna
dell'output non passano dallo script: li fissano i test di `governor.rs` e di
`executor/tests.rs`, che verificano che cosa succede e non come è scritto.
