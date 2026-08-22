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

**Categoria** (18 valori stabili):

`invalid_plan`, `invalid_configuration`, `schema`, `data_mapping`, `crs`,
`unsupported`, `not_found`, `conflict`, `authentication`, `authorization`,
`timeout`, `cancelled`, `resource_limit`, `io`, `protocol`, `transient`,
`execution`, `internal`.

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

### Finestra TOCTOU sull'ingresso IPC

La pre-validazione del framing è un *time-of-check*, la lettura di Arrow è il
*time-of-use*: fra i due c'è una finestra. Se il file viene mutato sotto i
piedi resta attiva la barriera anti-panico, ma **decade il tetto sulle
allocazioni**, che vale solo sui byte effettivamente pre-validati.

Il rientro richiede uno snapshot immutabile disponibile su tutte le
piattaforme supportate, senza imporre una copia dei dati.

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
