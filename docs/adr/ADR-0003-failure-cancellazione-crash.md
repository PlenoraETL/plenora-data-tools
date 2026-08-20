# ADR 3 — Failure propagation, cancellazione e crash

- **Stato**: attuato parzialmente (Fase 2B-M1, executor seriale) — vedi
  "Stato di attuazione" in coda
- **Decisioni collegate**: D12, D14, D21, D23, D24
- **Riferimenti**: `Architetture.md` §6.4

## Contesto

Nel DAG parallelo un fallimento in un ramo deve fermare l'intera esecuzione in
modo controllato: niente output parziali, niente risorse trattenute, diagnosi
sufficiente a capire la causa. Tre livelli di problema: errori attesi,
cancellazione cooperativa (non sempre possibile per kernel monolitici), e crash
che nessun meccanismo in-process può intercettare.

## Decisione

### Errori: first-error wins

- Il primo errore **osservato** cancella immediatamente gli altri rami.
- L'errore **riportato** non è necessariamente il primo osservato: in un DAG
  parallelo due rami possono fallire quasi contemporaneamente e il "primo"
  cambierebbe tra esecuzioni. L'errore primario riportato è scelto con regola
  stabile: (1) errore non causato da cancellazione; (2) minore profondità
  topologica; (3) `NodeId` minore; (4) sequence number minore. Così la
  diagnosi è deterministica anche se l'osservazione non lo è.
- L'errore primario conserva: `execution_id`, nodo, operazione, categoria,
  source chain interna, disposizione di retry (R9.7, da milestone D — in M1d
  era il booleano retryable/non-retryable) se applicabile.
  **Mai** valori o payload sensibili.
- Gli errori secondari (conseguenti alla cancellazione) sono telemetria, non
  sostituiscono la causa iniziale.
- **Eccezione normativa row-scoped (Contracts 2.0-rc17, R9.9-R9.14):** un
  rifiuto deterministico di una riga in uno stream seriale non termina subito
  la scansione. Il componente sopprime ogni output successivo, continua solo
  per completare conteggi ed esempi bounded e riporta un unico errore terminale
  `plenora-row-diagnostics-v1`. Se la scansione si interrompe, il report diventa
  `partial`/`unknown` con `knowledge_limits`; non si dichiara falsa completezza.
  Questa eccezione non avvia altri rami, non modifica la selezione errori del DAG
  parallelo e non autorizza remediation o drop silenziosi.
  **Dove attendono gli accepted (M2d, ADR-0002):** la barriera impone che
  nessun accepted esca prima della fine della scansione, non che attenda su
  disco. Da M2d gli accepted sono trattenuti **in memoria** con il lease vivo
  finché il budget lo consente, e travasati nello staging IPC — in ordine, con
  passaggio definitivo — quando non lo consente più. Su rejection tardiva,
  errore o cancellazione l'esito è identico nelle due modalità: **zero accepted
  pubblicati** e cleanup completo. La soglia è deterministica e derivata dai
  limiti del piano: vedi ADR-0002 §M2d.
- **Modalità diagnostica opt-in** (solo per input fidati): l'errore può
  includere nodo, indice batch, indice riga, colonna e tipo di violazione
  (es. `node=buffer batch=12 row=941 field=geometry reason=WKB_DEPTH_LIMIT`),
  con hash o descrizione strutturale del valore — mai il valore.

### Cancellazione

Ogni operazione dichiara in catalogo `cancellation_behavior`:

- `Cooperative`: controlla il token periodicamente durante il lavoro.
- `BoundaryOnly`: controlla solo ai confini di batch.
- `NonInterruptible` (alcune chiamate GEOS/PROJ): non offre punti di
  interruzione. Per queste: nessuna nuova attività dopo la cancellazione,
  latenza massima **documentata e osservabile nelle metriche**, lease
  trattenuti visibili al governor, gli altri rami possono attendere il
  completamento prima del cleanup finale. La v1 accetta questa attesa
  esplicitamente: **non si promette cancellazione immediata**. Isolamento in
  processo separato solo per backend dimostrabilmente instabili.

### Panic

- I panic dei worker sono intercettati al confine dell'executor con
  `catch_unwind`, causano cancellazione globale e sono convertiti in errore
  interno privo di dati sensibili.
- I kernel non usano panic per errori attesi.
- I confini di `catch_unwind` sono dichiarati; i tipi che attraversano il
  confine rispettano `UnwindSafe` o wrapper espliciti.
- L'errore primario è preservato; cleanup di spill, code e writer eseguito
  comunque.
- **Nessun panic può portare al publish dell'output.**

### Crash non intercettabili

`catch_unwind` non copre `panic = "abort"`, crash in GEOS/PROJ, OOM killer,
kill esterni, perdita del filesystem. Difesa strutturale:

- directory temporanee isolate per `execution_id`;
- **lock file come prova principale** di esecuzione viva (rilasciato dal
  sistema operativo dopo un crash);
- PID, identificativo host e timestamp di heartbeat come segnali diagnostici
  aggiuntivi — mai prove sufficienti (PID riutilizzabile; una macchina
  sospesa/ibernata può rendere vecchio il timestamp senza che l'esecuzione
  sia orfana);
- **scavenging all'avvio**: elimina solo directory senza lock attivo **e** con
  heartbeat scaduto; TTL conservativo;
- test di riavvio con directory lasciate intenzionalmente incomplete.

### Cleanup e publish

A successo, errore, cancellazione o panic intercettabile: cleanup di tutti i
file di spill, chiusura di code e broadcaster. Il publish avviene **solo** a
grafo completato con successo (invariante I8). Nessun nodo produce side effect
esterni osservabili (invariante I2): solo il sink finale pubblica.

## Conseguenze

- Test obbligatori: errore in un ramo con altri attivi; cancellazione durante
  kernel lungo; panic con cleanup completo; preservazione dell'errore primario;
  scavenging dopo riavvio simulato.
- La CLI resta potenzialmente in attesa di un kernel `NonInterruptible` dopo
  una cancellazione: comportamento accettato, documentato e osservabile.

## Stato di attuazione (Fase 2B-M1, executor seriale)

Implementato:

- **Panic**: `catch_unwind` ai due punti di dispatch dei kernel (streaming/
  blocking e binario), conversione in `PlenoraError::Step` con attribuzione di
  nodo, solo il messaggio del panic; publish mai raggiunto (test dedicato).
- **Cancellazione**: `CancellationToken` (`Arc<AtomicBool>` dietro tipo
  dedicato) in `RuntimeContext`; check ai confini executor (tra batch, tra
  kernel, durante il drain dei blocking, al confine di output) con onore del
  `cancellation_behavior` di catalogo (`Cooperative` per batch, `BoundaryOnly`
  tra kernel, `NonInterruptible` solo confini di piano); errore dedicato
  `PlenoraError::Cancelled`; CLI con handler Ctrl-C (1° pressione = cancel
  cooperativo, 2° = exit forzato), exit code 130, nessun publish al cancel.
- **Errori arricchiti**: `execution_id` (`exec-<uuid v4>`) in `Execution`
  (ex `Step`) e `Cancelled`; `ErrorCategory` con `category()` (mapping
  dichiarato per variante) e — da milestone D — `retry_disposition()` a 5
  valori canonici R9.7 al posto del `retryable()` (true solo per `Io`)
  qui deciso; **modalità diagnostica
  opt-in** (`RuntimeContext::diagnostics`): reason arricchita con
  nodo/batch/riga/colonna, mai valori.
- **Diagnostica row-scoped**: carrier boxed in `PlenoraError`, envelope CLI
  additivo e aggregazione seriale cross-batch per conversioni tipate,
  trasformazioni temporali, `flatten_json`, assert di qualita', hash con
  `null_policy=error` e `geo.from_wkt`; indici sorgente zero-based checked,
  conteggi completi ed esempi bounded. Le righe invalide non entrano
  nell'output accettato: le policy legacy di coercizione/null non autorizzano
  remediation implicita.
- **Provenance originale**: il catalogo classifica conservativamente i nodi
  che preservano cardinalita' e ordine. Un consumer row-diagnostic dopo
  filter/sample/explode/join/aggregate/sort/reshape o qualunque sibling path
  non conservativo viene rifiutato dal planner finche' non esiste un sidecar
  di lineage. La provenance non viene ricostruita o inventata.
- **Partizioni**: Data Tools non espone ancora nodi di quarantena. Di
  conseguenza non crea implicitamente output `accepted`/`rejected` e rifiuta
  `explode.empty_policy=drop`; una futura partizione dovra' essere un nodo
  esplicito con due archi distinti.
- **Crash defense**: `TempStore` per `execution_id` con `lock.json`
  (execution_id, PID, host, heartbeat), scavenging all'avvio fail-safe
  (PID morto o heartbeat oltre TTL 24h; lock corrotto conservativo con TTL×2;
  solo directory `plenora-*`), test di riavvio simulato; test crash tra
  scrittura e persist del publish.

**Rinvii a M3 (DAG parallelo)**: selezione deterministica dell'errore primario
multi-ramo, errori secondari come telemetria, cancellazione di rami concorrenti,
confini `UnwindSafe` ridisegnati per il parallelo, token passato ai kernel,
isolamento in processo per backend instabili.

## Emendamento 2026-08-16 — canale dell'envelope ed exit code

Due decisioni sull'uscita della CLI, prese insieme perche' riguardano lo stesso
confine: come un chiamante viene a sapere che qualcosa e' andato storto.

### L'envelope passa da stderr a stdout

**Stato precedente.** L'envelope a quattro assi (R9.1, `protocol_version` 1)
veniva emesso su `stderr`, con `stdout` riservato a risultati e metriche. Non
era una svista: la review P1-5 del 2026-08-03 lo aveva riportato li'
deliberatamente, come «contratto pubblico storico», e due test di canale
(`nogeo_cli`, `dag_v4_cli`) asserivano che `stdout` restasse vuoto in caso di
errore.

**Decisione.** L'envelope e' emesso su `stdout`; `stderr` resta vuoto; l'exit
code resta non zero.

**Motivo.** `plenora-database-tools` — il componente gemello, gia' in esercizio
con CLI e SDK Python — emette gli errori su `stdout` e lascia `stderr` vuoto
(`docs/cli/ERROR-PROTOCOL.md` di quel repository). Un orchestratore che invoca
entrambi i componenti dovrebbe altrimenti sapere, per ciascuno, dove cercare
l'errore: due convenzioni opposte nella stessa famiglia sono un difetto di
prodotto, non una differenza di stile. La decisione del 2026-08-03 precede
l'esistenza di quel gemello e non aveva questo vincolo da soddisfare.

**Impatto.** Rottura per chi legge gli errori da `stderr`: registrata in
`docs/api-breaking-2026-08-16.md`. I test di canale sono stati invertiti —
asseriscono ora che `stderr` resti vuoto — invece di essere rimossi: la
proprieta' «l'envelope sta in UN canale solo, e i due non si mescolano» e'
quella che conta, ed e' rimasta.

**Condizione di ripensamento.** Se `plenora-database-tools` tornasse su
`stderr`, questo emendamento si ritira e la decisione del 2026-08-03 torna in
vigore: il criterio e' l'uniformita' della famiglia, non la preferenza per un
canale.

### Gli exit code sono una DIVERGENZA dichiarata, non un allineamento

L'exit code non e' allineato al gemello, e va detto invece di lasciarlo
credere: `plenora-database-tools` usa `ExitCode::FAILURE` — cioe' **1** — per
qualunque errore, e il suo contratto documentato e' solo «exit code non-zero».

Qui i codici restano piu' informativi, come proiezione della `category`
dell'envelope: `2` piano o configurazione invalidi, `3` contratto/schema/
capability, `4` limite di risorsa, `5` I/O, pubblicazione, rete o
autorizzazioni, `6` fallimento di esecuzione di un nodo, `70` difetto interno,
`130` cancellazione (128 + SIGINT, gia' deciso in questo ADR e non
rinunciabile).

**Perche' non si allinea.** Allinearsi significherebbe collassare tutto su `1`,
cioe' buttare via informazione che qui esiste — e rinunciare al `130`, che
questo ADR usa per distinguere la cancellazione cooperativa da un fallimento.
Un'uniformita' ottenuta impoverendo entrambi non e' un guadagno.

**Cosa resta condiviso, e cosa no.** La garanzia comune e' debole e vale:
`0` in caso di successo, non zero in caso di errore. Tutto il resto e' una
convenzione **di questo componente**: codice portabile fra i due deve leggere
`error.category` dall'envelope, che e' la fonte di verita' condivisa, e non
confrontare il codice numerico. La divergenza e' registrata come tale nella
matrice di `docs/piano-usabilita.md`.

## Emendamento 2026-08-16 (secondo giro) — il canale unico e' una garanzia, non un'intenzione

L'emendamento precedente ha spostato l'envelope su `stdout` dichiarando
`stderr` vuoto. La seconda review indipendente ha trovato che la garanzia
aveva **due crepe**, entrambe fuori dal percorso dell'envelope:

1. **Ctrl-C.** L'handler scriveva due avvisi su `stderr` («annullamento in
   corso», «uscita forzata»).
2. **Hook di panico.** Un panico intercettato da `catch_unwind` diventa un
   errore, ma l'hook di default aveva gia' stampato su `stderr` prima
   dell'unwinding.
3. (terza, minore) **Avviso di durabilita' del publish**, anch'esso su
   `stderr` — e per giunta invisibile a un consumatore automatico.

Si e' scelta la garanzia reale, non la deviazione dichiarata:

- **Ctrl-C**: gli avvisi restano, ma **solo quando `stderr` e' un terminale**
  (`IsTerminal`). Per ogni consumatore non interattivo — cioe' per ogni
  programma — `stderr` e' vuoto senza eccezioni; una persona davanti a un
  terminale continua a vedere che la cancellazione e' partita. L'esito
  formale resta comunque l'envelope `cancelled` su `stdout` con exit 130.
- **Panico**: l'hook di default e' sostituito da un hook silenzioso e `main`
  avvolge l'intero processo in `catch_unwind`. Un panico che sfugge diventa
  un envelope `internal` su `stdout` con exit 70, che e' piu' di quanto
  facesse prima: l'informazione non si perde, cambia canale e acquista un
  codice.
- **Durabilita' del publish**: non e' piu' un avviso ma il campo
  `durability_confirmed` del documento di uscita di `run`. Un esito che
  interessa a un programma non puo' vivere su un canale che per contratto non
  porta nulla.

La garanzia dichiarata e' quindi: **su `stderr` non interattivo la CLI non
scrive nulla, in nessun esito** — successo, errore, cancellazione o panico.
Verificata da `matrice_cli::stderr_resta_vuoto_su_ogni_esito_incluso_il_panico`
e dai golden di canale.

## Emendamento 2026-08-16 (terzo giro) — testo dei panici, e la garanzia verificata

**Il testo di un panico non viene piu' pubblicato.** L'emendamento precedente
convertiva un panico in errore riportandone il messaggio. Quel messaggio non e'
scritto da noi: un `assert_eq!` dentro una dipendenza puo' includervi i valori
confrontati, cioe' dati della riga, che finirebbero nei log di chi ci invoca.
Gli errori da panico riportano ora solo la FORMA del payload (statico,
dinamico, non testuale); il contesto diagnostico — nodo, operazione,
`execution_id` — resta invariato ed e' quello che serve davvero.

**La garanzia su `stderr` e' ora verificata, non solo dichiarata.**
L'emendamento precedente affermava che gli avvisi di Ctrl-C fossero
condizionati a `is_terminal`. Non lo erano: la modifica era stata scritta ma
non salvata, e il codice conteneva ancora due `eprintln!` incondizionati. La
terza review indipendente ha trovato la divergenza fra questo ADR e il codice.

Il controllo e' ora nel codice, e un test **strutturale**
(`matrice_cli::nessun_eprintln_incondizionato_nel_sorgente_della_cli`) verifica
che ogni `eprintln!` della CLI stia dentro un ramo governato da `is_terminal`.
Un test d'integrazione non puo' inviare `SIGINT` in modo portabile: la
proprieta' si verifica dove e' verificabile, invece di essere solo affermata.

**Exit code: la riga morta e' stata chiusa.** La tabella dei codici prevedeva
`4` per «limite di risorsa superato», ma nessuna variante di `PlenoraError`
produceva la categoria `resource_limit`: il codice era irraggiungibile. Ora
esiste `PlenoraError::ResourceLimit` e i limiti di risorsa a runtime — righe,
colonne, quota di spill, fattore di espansione, budget di memoria — lo
producono. Il criterio: `invalid_plan` per un piano sbagliato,
`resource_limit` per un piano corretto i cui dati non entrano nel budget.
