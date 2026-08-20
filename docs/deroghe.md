# Registro delle deroghe — plenora-data-tools

Unico punto di raccolta delle deroghe attive del componente (ICD §16 R16.2,
plenora-contracts `v2.0-rc10`). Ogni deroga dichiara: regola, motivo, impatto
sugli hazard, owner e condizione di rientro (R16.1). Una deroga senza
condizione di rientro e' permanente e va dichiarata tale. Le deviazioni dai
contratti documentati restano registrate anche nell'ADR pertinente
(`docs/adr/`, regola 4 di AGENTS.md).

Riferimento normativo citato nelle CIA: `plenora-contracts`, tag `v2.0-rc10`
(revisione `3598259bbe07d1c853453ff34ca2c1d1d28a0272`).

## DER-006 — `max_parallelism` applicato al processo, non al piano

- **Regola derogata:** ADR-0006 / `Limits` — i limiti sono proprieta' del
  PIANO, e due piani nello stesso processo possono dichiararne di diversi.
  `max_parallelism` non rispetta questa forma.
- **Ambito:** il tetto sul parallelismo si applica dimensionando il pool
  Rayon **globale del processo**
  (`plenora_engine::parallelism::configure`, chiamata dalla CLI dopo la
  validazione del piano). Una seconda esecuzione nello stesso processo che
  dichiari un grado DIVERSO viene rifiutata con errore esplicito, mai
  eseguita con il tetto sbagliato.
- **Motivo:** tutti i percorsi paralleli dei kernel (`joins`,
  `aggregation::sort`, `arrow_adapter`, `spatial_join`, trasporto geo)
  usano il pool globale; e' l'unica leva che li vincola insieme. Un pool
  dedicato con `ThreadPool::install` richiederebbe chiusure `Send`, mentre
  lo stato dell'executor e' volutamente thread-locale (`Rc<ExecutionPlan>`,
  stream a pull sul thread chiamante): renderlo `Send` sarebbe un
  cambiamento di architettura, non un'applicazione di limiti.
  Prima di questa deroga il limite non vincolava NULLA — era una promessa
  di risorsa, e un piano con `max_parallelism: 2` occupava tutti i core.
- **Impatto sugli hazard:** un processo che esegue piu' piani con gradi
  diversi vede il secondo rifiutato invece che silenziosamente
  ridimensionato. Chi incorpora l'engine come libreria e non chiama
  `configure` resta sul default di Rayon (core logici), come prima.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** un executor con stato `Send` renderebbe
  possibile un pool per-esecuzione; servirebbe un ADR dedicato.

## DER-005 — Arrotondamento dichiarato nelle operazioni che producono `Float64`

- **Regola derogata:** ADR-0010 (emendamento 2026-08-16) — la conversione
  intero/decimal → `f64` e' esatta o errore.
- **Ambito:** le operazioni il cui RISULTATO e' un `Float64` per contratto.
  Il double e' li' il tipo del risultato, non un passaggio intermedio, e
  pretendere l'esattezza rifiuterebbe input legittimi — un `Decimal(10,2)`
  con valore `0.1` non ha alcun double esatto, eppure la sua media e' una
  richiesta ragionevole. L'elenco completo:
  1. `cleansing::cast_to_float` (`table.cast` con `to: "float"`): l'utente
     CHIEDE un `Float64`;
  2. `dates::diff_value` (`table.date_diff`): risultato in unita'
     frazionarie. Oltre 2^53 nanosecondi — circa 104 giorni — il conteggio
     esatto non entra in un double, e l'esattezza rifiuterebbe ogni
     intervallo di qualche mese. «Fuori scala» resta riservato ai
     nanosecondi oltre `i64` (circa 292 anni);
  3. `scalar_as_f64_rounded`, e con esso `aggregation`, `analysis`,
     `reshape` (aggregazioni del pivot), `expressions` e `formula`:
     medie, varianze, quantili, statistiche e valori di formula;
  4. i letterali numerici di configurazione che alimentano quelle stesse
     operazioni (`expressions::scalar::literal`, il valore di default di
     `align_schema` e di `fill` su colonne `Float64`): il dominio del
     confronto e' li' il double, per il tipo della colonna.
  Per i percorsi 3 e 4 il fast path e il percorso generico applicano la
  STESSA conversione: un fast path esatto e un generico arrotondato
  darebbero due risposte diverse sullo stesso input, che e' peggio di
  entrambe.
- **Motivo:** il `Float64` e' il risultato richiesto. L'esattezza non e'
  rinunciata, e' fuori contratto. La deroga esiste per renderlo dichiarato
  invece che implicito: prima questi siti scrivevano `to_f64().unwrap_or(NAN)`
  o `to_f64().ok_or_else(|| "non rappresentabile")`, che simulavano un
  controllo inesistente.
- **Fuori ambito, e sono la maggioranza:** tutto cio' che DECIDE. I
  confronti (`scalar_compare`, `compare_i64/u64/i128/decimal128`), le chiavi
  di join, i filtri, le regole di governance e i vincoli di qualita' non
  convertono affatto: confrontano nel dominio nativo del tipo. `scalar_as_f64`
  resta esatto-o-errore, decimal inclusi (`exact_f64_from_decimal128`
  verifica il valore DOPO la divisione, non il solo intero non scalato).
- **Impatto sugli hazard:** un valore oltre 2^53, o un decimal frazionario,
  passato per queste operazioni perde precisione senza errore. Nessun
  percorso decisionale ha questa proprieta'.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** nessuna — e' la semantica dichiarata di queste
  operazioni. Aggregazioni a precisione decimale sarebbero operazioni nuove,
  non il rientro di questa deroga.

## DER-007 — IPC compresso rifiutato dal confine di lettura

- **Regola derogata:** nessuna regola interna; e' una restrizione
  dichiarata rispetto al formato Arrow IPC, che ammette
  `bodyCompression` (LZ4/ZSTD).
- **Ambito:** `plenora_engine::ipc_boundary` rifiuta ogni messaggio IPC che
  dichiari un body compresso, su tutti gli ingressi file/stream/CLI.
- **Motivo:** con la compressione la dimensione che arrow allochera' NON e'
  il `bodyLength` del messaggio ma la somma delle lunghezze decompresse,
  dichiarate nei prefissi a 8 byte dei singoli buffer — dentro il body, cioe'
  proprio la regione che la pre-validazione non legge. Un tetto sul body
  compresso non limiterebbe nulla, e fidarsi dei prefissi dichiarati sarebbe
  lo stesso schema «lunghezza dichiarata» da cui il confine difende. Fra
  accettare una classe che non si sa limitare e rifiutarla, un componente
  fail-closed rifiuta.
- **Impatto sugli hazard:** un input IPC compresso prodotto da terzi non e'
  leggibile. I writer di questo progetto non comprimono mai (le
  `IpcWriteOptions` di default non comprimono), quindi nessun artefatto
  nostro e' interessato.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** un percorso che legga i prefissi di lunghezza
  decompressa dei buffer e li sommi in aritmetica controllata contro un
  tetto dichiarato; a quel punto la compressione diventa limitabile e la
  deroga si ritira.

## DER-009 — Hasher delle chiavi non keyed

- **Regola derogata:** il principio fail-closed sugli input non fidati: ogni
  costo che dipende dai DATI deve essere limitabile a priori. Qui il costo
  peggiore dipende dai dati e non e' limitato entro il tetto sulle righe.
- **Ambito:** `plenora_kernels_table::hashing::KeyHasher`, usato da tutte le
  mappe e gli insiemi di chiavi dei kernel (`joins`, `governance`, `reshape`,
  `aggregation::grouping`).
- **Motivo:** `SipHash` (default std) dominerebbe il costo di build/probe su
  milioni di righe; la ricorrenza moltiplicativa scelta ha il throughput
  richiesto ma NON e' keyed. Le chiavi sono valori delle righe, cioe' dati
  che chi fornisce l'input sceglie: collisioni su piu' blocchi sono
  costruibili, e il finalizer sparpaglia i bit senza rendere la funzione
  resistente.
- **Impatto sugli hazard:** i limiti di piano (`max_input_rows`,
  `max_rows_per_edge`) limitano `n` e quindi il costo peggiore assoluto, ma
  NON impediscono il comportamento quadratico ENTRO quel `n`: un input
  costruito apposta puo' far degradare build e probe fino al tetto di righe
  ammesso. Non c'e' perdita di correttezza — i risultati restano quelli — ne'
  un percorso verso l'esecuzione di codice: e' esclusivamente un hazard di
  tempo di esecuzione.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** un hasher con chiave per processo. Non e' fatto
  ora perche' cambia una proprieta' su cui poggiano piu' kernel — la
  stabilita' dell'hash fra esecuzioni — e va verificata su tutti gli usi di
  `FastHasher` prima di essere introdotta (snapshot dei test che dipendono
  dall'ordine di iterazione, in particolare). Fino ad allora il rischio e'
  questo, dichiarato qui e in testa a `hashing.rs`.

## DER-011 — `max_memory_bytes` non e' un tetto duro sull'esecuzione

- **Regola derogata:** ADR-0006 e il testo del piano presentano
  `max_memory_bytes` come il tetto della memoria usata dall'esecuzione. Non
  lo e'. E' vero per il **caricamento** dei percorsi che contano i batch
  mentre li accumulano; per l'**esecuzione** e' vero solo dove esiste un
  rifiuto preventivo.
- **Ambito: ENTRAMBI i percorsi, non solo il legacy.** La prima stesura di
  questa deroga limitava il problema ai piani `schema_version <= 3` e
  affermava che l'executor DAG v4 avesse gia' un tetto duro. Era falso.

  **Percorso legacy.** Il controllo di ammissione (`ammissione_output` nella
  CLI) rifiuta la PUBBLICAZIONE di un risultato fuori budget, ma corre dopo
  che il kernel ha costruito l'output.

  **Aggiornamento del 2026-08-21 (permesso atomico del governor).** Il
  `MemoryGovernor` ha ora un permesso che verifica e prenota in **una sola
  operazione**, e l'unico pattern «leggi il contatore, poi prenota» del codice
  e' stato eliminato. Per i segmenti **row-diagnostics** la quota dell'uscita
  e' ora presa PRIMA della passata e ritagliata dopo: e' il secondo punto
  dell'engine — dopo il gruppo geo fuso (ADR-0012 D12.7) — in cui la
  reservation precede l'allocazione che deve coprire.

  **La deroga resta aperta.** Il censimento completo dei siti e' in
  `docs/der011-censimento-2026-08-21.md`, con verificatore ad ancore
  (`scripts/verifica_censimento_der011.py`): su undici siti del solo percorso
  DAG, due sono preventivi e uno non ha reservation affatto; percorso legacy,
  kernel senza preflight e allocazioni Arrow interne restano scoperti.
  **Nessun tetto duro e' dichiarato**: `max_memory_bytes` resta un contatore
  di cio' che e' gia' stato allocato.

  **Percorso DAG v4.** Stesso ordine, in tre punti verificati:
  `concat_batches` costruisce il batch completo e solo dopo arrivano
  `check_batch_bytes` e la reservation (`executor.rs`, intorno a 4423); il
  kernel produce l'output e il governor lo riserva subito **dopo**
  (`run_kernel` e la `reserve` che segue); lo stesso vale nel percorso
  binario. Il `MemoryGovernor` e' quindi un contatore di cio' che e' gia'
  stato allocato, non un'autorizzazione ad allocare: rifiuta il lease, non
  l'allocazione che il lease avrebbe dovuto coprire.

  **Anche le quattro operazioni con rifiuto preventivo sono in deroga.** La
  stesura precedente diceva «non sono in deroga» e due righe dopo ammetteva
  che il preflight non offre una garanzia: una contraddizione. Se il modello
  puo' sottostimare, o se l'implementazione alloca temporanei che il modello
  non conosce, il tetto duro non c'e' — e allora l'operazione e' in deroga,
  punto.

  Quello che cambia per `cross_join`, `concat`, `concat_by_name` e `melt` e'
  l'ESPOSIZIONE, non lo stato: `plenora_kernels_table::preflight_output_bytes`
  rifiuta prima di allocare, con un modello scritto per quella specifica
  operazione, e intercetta le esplosioni di ordini di grandezza. Restano
  fuori dal modello i temporanei non dichiarati e le sottostime del modello
  stesso. La deroga vale per tutte le operazioni; per queste quattro e'
  piu' stretta.
- **Motivo:** un tetto duro richiede, per ogni operazione, o una stima
  conservativa a priori della dimensione dell'output, o un'autorizzazione ad
  allocare — reservation preventiva, o accounting dentro l'allocatore — al
  posto di un lease acquisito dopo. La prima e' possibile solo dove il
  conteggio delle righe di output precede l'allocazione, ed e' stata
  implementata li'. La seconda cambia la struttura di entrambi gli executor e
  il rapporto con Arrow, che alloca per conto proprio.
- **Impatto sugli hazard:** su un'operazione senza preflight — e su tutte le
  operazioni del DAG — un input che produce un output molto piu' grande del
  budget porta all'esaurimento della memoria del processo PRIMA che l'errore
  `resource_limit` venga prodotto. Il fallimento e' un OOM, non un errore
  diagnosticabile. Nessuna perdita di correttezza e nessuna pubblicazione di
  risultati fuori budget — l'ammissione a valle continua a impedirla quando
  la macchina regge — ma la garanzia «il processo non supera N byte» NON
  vale.

  Anche dove il preflight c'e', la stima **non e' una misura**: copre i
  buffer del risultato secondo il modello dichiarato dal kernel, non le
  allocazioni temporanee che l'implementazione fa per conto proprio (tabelle
  hash, copie intermedie, buffer di crescita). Per `cross_join` il modello
  include esplicitamente i due vettori di indici, perche' sono grandi quanto
  l'output; per `melt` distingue il percorso omogeneo da quello con
  `type_policy = "string"`, dove i valori diventano testo e occupano piu'
  della loro forma binaria. Per gli altri kernel un'allocazione temporanea
  non modellata resta fuori.

  I modelli sono inoltre stati SBAGLIATI una volta — una formula unica per
  quattro operazioni diverse, che sottostimava tre di esse. Il presidio non e'
  la formula ma la coppia di regressioni per operazione, con il budget scelto
  fra la stima vecchia e quella nuova; e resta il fatto che un modello e' una
  cosa che si puo' sbagliare, mentre una reservation preventiva no.

  Contribuisce anche `execute_binary` del percorso legacy, che normalizza
  entrambi gli input mentre gli originali sono ancora vivi: la copia e'
  effettiva solo se lo schema contiene `LargeUtf8` o metadati `pandas`
  (altrimenti e' un clone di `Arc`), ma quando avviene raddoppia
  transitoriamente la memoria degli input senza addebito.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** due strade, entrambe accettabili, e nessuna
  parziale.
  1. Un preflight conservativo **per ogni operazione**, sullo schema di
     `preflight_output_bytes`, con un modello per operazione e la copertura
     delle allocazioni temporanee che ciascuna fa.
  2. Un'**autorizzazione ad allocare** al posto del lease a posteriori:
     reservation preventiva sufficientemente conservativa in entrambi gli
     executor, oppure accounting integrato nell'allocazione.

  Fino ad allora la garanzia va chiamata **controllo di ammissione
  post-allocazione**, non budget globale duro, e non va attribuito al DAG v4
  un tetto che non ha. Il nome nel codice lo dice (`ammissione_output`), la
  doc di `preflight_output_bytes` dichiara i propri limiti, e questo registro
  copre entrambi i percorsi.

## DER-010 — Hook di panico non installato d'ufficio per gli embedder

- **Regola derogata:** «errori senza dati» (ADR-0001). L'hook di panico di
  `std` stampa su stderr il payload del panico prima dell'unwinding, e quel
  testo puo' contenere valori di riga se a generarlo e' stata una macro di
  asserzione di una dipendenza.
- **Ambito:** due situazioni, non una.
  1. Ogni processo che carica `plenora-engine` o `plenora-kernels-*` come
     LIBRERIA senza chiamare `plenora_core::panic_policy::install`.
  2. **Anche dopo** una chiamata a `install`: `std::panic::set_hook` resta
     una funzione pubblica di `std` e qualunque componente del processo puo'
     sostituire il nostro hook in seguito. `Once` governa le chiamate alla
     nostra API, non quelle di `std`, e in Rust stabile non esiste modo di
     rendere un hook non sostituibile. La politica e' quindi un **protocollo
     cooperativo**, non una garanzia imponibile.

  La CLI riduce la deroga ma non la elimina: installa `PanicPolicy::Silent`
  come prima istruzione di `main` e **guarda l'esito**; se `install`
  risponde `false` — qualcuno era arrivato prima — lo dichiara nell'envelope
  di un eventuale panico invece di promettere un canale che non governa.
- **Motivo:** l'hook e' stato globale del processo. Installarlo dall'interno
  di una libreria — a un `use`, in un costruttore statico, o al primo uso di
  un kernel — sostituirebbe di nascosto l'hook di chi ci ospita: un runner di
  test che raccoglie i backtrace, `libfuzzer-sys` (che installa apposta un
  hook che aborta prima dell'unwinding), un supervisore che li invia a un
  raccoglitore. Sarebbe un danno certo per evitarne uno possibile.
- **Impatto sugli hazard:** un embedder che non installa nulla — o un
  processo in cui qualcun altro reinstalla un hook dopo di noi — vede il
  comportamento di `std`: payload del panico su stderr. La barriera
  `catch_unwind` del confine IPC continua a convertire il panico in errore
  sanitizzato, quindi l'ERRORE resta senza dati; e' la stampa dell'hook, che
  precede l'unwinding, a restare fuori controllo. Nessuna perdita di
  correttezza e nessun percorso verso l'esecuzione di codice: e' un hazard di
  esposizione, limitato allo stderr del processo ospite. Il limite e'
  verificato da un test, non solo dichiarato:
  `la_politica_non_sopravvive_a_un_set_hook_di_terze_parti`.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** nessuna completa, ed e' bene dirlo. Il binding
  Python (PyO3) chiamera' `install(PanicPolicy::Sanitized)` nella propria
  funzione di inizializzazione del modulo, dove l'installazione e' un atto
  esplicito dell'embedder e non un effetto collaterale della libreria: questo
  chiude il caso 1. Il caso 2 — un `set_hook` successivo di terze parti —
  resterebbe aperto anche allora, e si chiuderebbe solo se `std` offrisse un
  hook non sostituibile o un meccanismo di composizione. Fino ad allora la
  deroga e' permanente e va letta cosi'. Documentata anche in testa a
  `crates/plenora-core/src/panic_policy.rs`.

## DER-008 — Stabilita' dell'ingresso IPC durante la lettura (TOCTOU residuo)

- **Regola derogata:** nessuna regola interna; e' una condizione al contorno
  dichiarata del confine di lettura. La pre-validazione del framing
  (`plenora_engine::ipc_boundary`) e' un *time-of-check*, la lettura di arrow
  e' il *time-of-use*: fra i due c'e' una finestra.
- **Ambito:** tutti gli ingressi IPC su file (`Input::read_ipc_file` /
  `read_ipc_stream`, discovery dello schema della CLI). NON riguarda il
  trasporto geo, che pre-valida e decodifica gli STESSI byte gia' in memoria:
  li' la finestra non esiste.
- **Cosa il confine garantisce comunque:** la validazione e la lettura usano
  lo STESSO handle aperto una volta sola (`validated_handle` restituisce il
  file riavvolto, non il percorso). Il percorso non viene risolto due volte,
  quindi la classe «sostituisci il file fra check e use» — rename, symlink
  swap, ricreazione del path — non e' raggiungibile.
- **Cosa resta scoperto:** una modifica IN PLACE dello stesso inode/handle da
  parte di uno scrittore concorrente che abbia permesso di scrittura sul
  file. In quel caso arrow legge byte che la pre-validazione non ha visto.
- **Impatto sugli hazard:** con l'input mutato sotto i piedi resta attiva la
  barriera anti-panico (`catch_unwind` su schema e su ogni batch: un panico
  di `fb_to_schema` diventa comunque errore), mentre DECADE il tetto sulle
  allocazioni, che vale solo sui byte effettivamente pre-validati: un
  metadato riscritto dopo il check puo' tornare a dichiarare conteggi ostili.
  E' il motivo per cui la condizione e' dichiarata qui e non minimizzata.
- **Requisito operativo:** gli ingressi vanno letti da una posizione non
  scrivibile da terzi per la durata dell'esecuzione. Il modo standard di
  soddisfarlo e' copiare l'input in una directory di lavoro controllata prima
  di invocare il tool; questo componente non lo fa da se' perche' imporrebbe
  una copia integrale di ogni ingresso — costo proporzionale ai dati, non al
  rischio — su tutti gli utilizzi, compresi quelli in cui la sorgente e' gia'
  isolata.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** un meccanismo di snapshot immutabile disponibile
  su tutte le piattaforme supportate (es. apertura con lock esclusivo di
  scrittura, oppure clonazione CoW verificata), che chiuda la finestra senza
  imporre una copia dei dati. Fino ad allora la condizione e' questa,
  dichiarata.

## DER-004 — `max_temp_bytes` per dominio, non quota globale su disco

- **Regola derogata:** ADR-0002 (contesto: "i limiti di memoria devono
  valere globalmente sul piano, con rami paralleli che condividono la
  stessa quota") letto estensivamente ai byte temporanei su disco.
  ADR-0002 cita `max_temp_bytes` solo nella sezione spill e ADR-0006 non
  dichiara una quota temp globale: la lettura "quota globale su disco" e'
  quindi una possibile interpretazione, non un testo normativo — questa
  deroga fissa l'interpretazione effettiva.
- **Ambito:** i tre domini di scrittura temporanea misurano CIASCUNO la
  propria scrittura contro `max_temp_bytes` del piano, senza
  coordinamento condiviso: (1) staging IPC dell'input del gate WKB
  (`stage_input_batches`), (2) staging IPC degli output accettati dei
  segmenti row-diagnostics (`scan_row_diagnostic_segment`), (3) spill
  degli operatori blocking (`SpillWorkspace`/`RowSpillWorkspace`,
  `CountingFile`). Il picco su disco puo' arrivare alla somma dei domini
  concorrenti (fino a ~3x la quota). Invariato: ogni dominio resta
  individualmente bounded e fail-closed oltre quota.
- **Motivo:** i domini hanno lifecycle e proprietari diversi (gate input,
  segmento diagnostico, kernel spillante) e non si sovrappongono per
  intero nel tempo; un coordinamento condiviso (semaforo cross-dominio)
  introdurrebbe accoppiamento e rischio di stallo senza hazard coperto:
  l'hazard reale e' la memoria di processo, gia' sotto budget governor
  globale (`max_memory_bytes`, ADR-0002 attuato), non lo spazio disco
  temporaneo, che vive in `TempDir` per-esecuzione con cleanup al `Drop`.
- **Impatto sugli hazard:** un'esecuzione puo' occupare su disco fino a
  ~3x `max_temp_bytes` nel caso peggiore (tre domini attivi in
  sequenza/concorrenza). Chi dimensiona i volumi temporanei deve usare la
  somma dei domini, non la quota singola. Nessun effetto su correttezza,
  determinismo o bounded-ness per-dominio (ogni scrittura oltre quota
  fallisce esplicita).
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** nessuna — design intenzionale, dichiarato
  permanente. Se in futuro un requisito imporra' un tetto globale su
  disco, servira' un ADR nuovo (contatore condiviso tra i tre domini) e
  il ritiro di questa deroga.

## DER-003 — `max_batch_bytes` sugli archi interni dei gruppi fusi (fusione geo)

- **Regola derogata:** ADR-0006 / `check_edge_batch` (semantica limiti):
  ogni batch su un arco interno e' soggetto al tetto byte
  `max_batch_bytes`. Su un arco interno FUSO (ADR-0012, D12.8) il batch
  non e' materializzato — la geometria vive in forma decodificata
  transiente — e il check byte non e' applicabile.
- **Ambito:** solo gli archi interni dei gruppi di fusione geo
  (`TransformInPlace`, perimetro M1 di ADR-0012). Ingresso e uscita del
  segmento restano soggetti a tutti i limiti, byte inclusi; righe e batch
  per arco restano esatti (1:1) e i limiti corrispondenti si applicano.
- **Motivo:** la fusione e' un'ottimizzazione fisica (−45% misurato sulla
  catena di baseline); il batch intermedio non esiste in nessuna forma
  serializzata, quindi un tetto sui suoi byte non ha oggetto.
- **Impatto sugli hazard:** **su questo percorso H-03 e' coperto dal
  governor (reservation esatta dei byte decodificati, ADR-0012 D12.7),
  non da `max_batch_bytes`** — la protezione e' spostata, non rimossa
  (condizione dell'owner, 2026-07-29). Hazard residuo: un batch
  intermedio che avrebbe superato il tetto byte non e' piu' rifiutato in
  quanto tale; la memoria reale resta sotto budget governor.
- **Condizione di entrata in vigore (owner):** esiste un test che
  dimostra che il governor scatta davvero su un batch oltre soglia.
  **Finche' il test non e' in CI, la deroga NON e' in vigore** e la
  fusione non puo' essere attivata in produzione.
  **Soddisfatta (2026-07-29, M1):**
  `executor::tests::geo_fusion_falls_back_when_the_governor_rejects_the_reservation`
  — reservation D12.7 rifiutata dal governor su batch oltre soglia,
  fallback strumentato (contatore `geo_fusion_fallbacks`), output
  identico al percorso non fuso.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** nessuna — deroga permanente, dichiarata
  tale (e' intrinseca alla rappresentazione transiente; il rientro
  sarebbe la rinuncia alla fusione).

## DER-002 — Emissione delle chiavi canoniche §2 prima della ratifica

- **Regola derogata:** §15.4 passo 1 (emendata 2.0-rc5) — prima della
  ratifica di §2, l'emissione delle chiavi candidate e' ammessa solo con
  deroga registrata che dichiari l'hazard per i consumatori non allineati
  e la condizione di rientro. La deroga gemella a livello ICD e'
  DER-ICD-002 (tutti e tre i componenti nella stessa condizione).
- **Ambito:** emissione delle chiavi `plenora.geometry.*` e
  `plenora.contract.version` nel percorso DAG v4 (milestone B/C/D di
  ADR-0009) e, dal 2026-07-30 (BLOCK-06, decisione owner: parita', non
  ritiro), nel percorso LEGACY `geo_transport` — entrambi in DOPPIA
  emissione con le chiavi standard GeoArrow, la sola forma compatibile con
  DER-ICD-002. Nel legacy l'emissione e' un post-processo centrale sugli
  entry point `transform_arrow`/`pair_arrow` (`canonical_legacy_output`):
  blocco derivato dal metadato `geo` del campo di output (stessa fonte del
  GeoArrow legacy), chiavi canoniche di lineage propagate invariate (R2.4).
  Restano fuori i comandi legacy senza schema Arrow in uscita
  (`transform` v2 a frame WKB, `spatial-join` a coppie di indici): non
  hanno metadati Arrow a cui appenderle.
- **Motivo:** il protocollo serve alla cooperazione applicativa ora (la
  catena bordo-centro-bordo); §2 e' `proposta` non ratificata, ma la
  doppia lettura/emissione non rompe i consumatori esistenti.
- **Impatto sugli hazard:** un consumatore non allineato riceve chiavi di
  metadata sconosciute. Arrow le preserva per costruzione e nessun
  consumatore attuale le interpreta: il rischio e' limitato a
  incompatibilita' di NOMI se §2 fosse ratificata con nomi diversi dai
  candidati — coperto dalla condizione di rientro (migrazione).
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** ratifica di §2 con nomi compatibili, oppure
  migrazione delle emissioni ai nomi ratificati (come da DER-ICD-002).

## DER-001 — Toolchain nightly per il fuzzing con sanitizer

- **Regola derogata:** R13.1 (ICD §13, ratificata) — tutti i componenti
  compilano con la stessa versione esatta del compilatore, fissata da
  `rust-toolchain.toml` (1.92.0).
- **Ambito:** solo lo step `cargo fuzz run` del workflow
  `.github/workflows/fuzz.yml` (`RUSTUP_TOOLCHAIN: nightly`). Build, test,
  clippy e gate R6 restano sulla toolchain pinnata. Gli script locali
  (`scripts/fuzz-smoke.sh`, `scripts/fuzz-campaign.sh`) usano l'immagine
  dedicata `plenora-rust:nightly-fuzz`: stessa deroga, stesso ambito.
- **Motivo:** `-Zsanitizer=address` e le flag di coverage di cargo-fuzz
  (`-Cpasses=sancov-module`, `-Z*`) sono accettate solo dal compilatore
  nightly; non esiste modo di eseguire il fuzzing con sanitizer sulla
  toolchain stabile pinnata. Senza la deroga il fuzz notturno fallisce in
  partenza ("the option `Z` is only accepted on the nightly compiler",
  run del 2026-07-28) e la rete di sicurezza resta muta.
- **Impatto sugli hazard:** il fuzzing gira su un compilatore diverso da
  quello di produzione (H-07: difetto di compilatore indistinguibile da
  difetto di codice). Controllo: un crash conta solo se riproducibile; la
  riproduzione e la minimizzazione dei casi avvengono sulla toolchain
  pinnata 1.92.0 prima di aprire una fix, quindi nessun artefatto del
  compilatore nightly puo' entrare nel percorso di produzione.
- **Owner:** maintainer di plenora-data-tools.
- **Condizione di rientro:** quando le flag sanitizer/coverage equivalenti
  saranno disponibili sulla toolchain stabile pinnata, oppure quando R13.1
  sara' emendata per ammettere esplicitamente la toolchain nightly per il
  fuzzing. Fino ad allora la deroga e' attiva e va riesaminata a ogni
  bump di toolchain (R13.5).
