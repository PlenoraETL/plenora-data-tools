# CLI `plenora-data`

`crates/plenora-cli` è la superficie pubblica del componente
`plenora-data-tools` secondo il profilo data-tools di `plenora-contracts`,
versione 2, fissato al commit `4c1569d4b7fb7f0b451566b71e0f01165f9d6bcc`: il binario
`plenora-data` (CLI 2.0) e le stesse quattro operazioni come funzioni Rust
(`plenora_cli::api`). Una sola tabella (`plenora_cli::operazioni::OPERAZIONI`)
dà comandi, aiuto, Capability Discovery 2.0 e la mappa degli export Rust; il
registro dei kernel di `data.catalog` deriva dal catalogo di `plenora-core`.

```sh
plenora-data --help [--format json]
plenora-data --version [--format json]
plenora-data capabilities [--format json]
plenora-data catalog [--format json]
plenora-data describe --input INPUT.arrow [CONTROLLI] [--format json]
plenora-data validate --plan PLAN.json [--input NAME=INPUT.arrow]... [CONTROLLI] [--format json]
plenora-data run --plan PLAN.json [--input NAME=INPUT.arrow]... --output [NAME=]OUTPUT.arrow...
                 [--overwrite] [CONTROLLI] [--format json]
# CONTROLLI: --deadline RFC3339 | --timeout-ms MS
```

| comando | operazione | contratto del risultato | controlli |
| --- | --- | --- | --- |
| `catalog` | `data.catalog` | `plenora-data-catalog-result-v2` | nessuno |
| `describe` | `data.describe` | `plenora-data-description-v1` | scadenza, annullamento |
| `validate` | `data.validate` | `plenora-data-plan-validation-result-v1` | scadenza, annullamento |
| `run` | `data.run` | `plenora-data-execution-result-v2` | scadenza, annullamento |

## Uscita e codici

Ogni invocazione scrive **un** documento JSON seguito da un a capo su
stdout, l'inviluppo `cli-envelope-v2` (`status`, `protocol_version` 2,
`component` `plenora-data-tools`, `component_version`, `contract`,
`command`, e `result` o `error`), e **niente su stderr**, tranne gli aborti
del processo che nessun hook intercetta (limite «Aborti senza inviluppo»
sotto): l'hook di panico è silenzioso (`plenora_core::panic_policy`, `Silent`) e un
panico diventa l'errore `internal` (exit 70) senza il testo del payload, con
effetto `unknown` per `run` (un output può essere già scritto). Senza
`--format` l'uscita è comunque JSON; solo `--help` senza `--format json`
stampa il testo d'aiuto (exit 0). Le chiavi escono in ordine: stesso
esito, stessi byte (provato su scoperta e `run`).

L'errore è la proiezione pubblica di `PlenoraError` ([«Errori»](errori.md#errori)),
mai un documento costruito a parte; il codice d'uscita è la proiezione
della categoria di CLI 2.0, sezione 8, con un `match` esaustivo:

| codice | categorie |
| --- | --- |
| 0 | successo |
| 2 | `invalid_plan`, `invalid_configuration` |
| 3 | `schema`, `data_mapping`, `crs`, `unsupported` |
| 4 | `resource_limit` |
| 5 | `io`, `not_found`, `conflict`, `concurrent_modification`, `protocol`, `authentication`, `authorization`, `timeout`, `transient` |
| 6 | `execution` |
| 70 | `internal` |
| 130 | `cancelled` |

Gli argomenti falliscono chiusi (`invalid_configuration`, exit 2): comando
o flag sconosciuto, flag di un altro comando, valore mancante (un valore
non comincia con `--`), flag ripetuto, argomento posizionale, `--format`
diverso da `json`, argomento non UTF-8 (tradurlo cambierebbe un percorso
in silenzio). I messaggi dicono la posizione, mai il testo ricevuto: un
argomento può essere un percorso. Gli alias deprecati del binding
(`inspect-dataset`, `transform`, …) non ci sono.

## Comandi

- **`catalog`**: `registry` è un documento `plenora-operation-registry-v1`
  con i kernel che un piano può eseguire (`{id, version, family}`);
  `kernels` un descrittore per ognuno dei 146 kernel del catalogo, con
  `status` (`available`, o `unavailable` con `reason`), `versions`
  (semantica, schema della config, analisi del contratto, kernel),
  `arity`, `result_shape`, `determinism`, `crs_requirement`,
  `required_backends` (sempre vuoto: niente GEOS né PROJ); `plan_format`
  `plenora-data-plan-v1`. `table.transpose` è `unavailable`
  (`plenora_pipeline::disponibilita`: il runner la rifiuta con ogni config,
  e `operazioni_doc.rs` prova l'elenco in entrambe le direzioni).
- **`describe`**: legge la tabella (Arrow IPC file o stream, o Parquet) con
  il budget di default (`DEFAULT_MAX_GOVERNED_MEMORY_BYTES`, 512 MiB), ne
  legge il contratto come lo leggerebbe un piano (stessa normalizzazione,
  stesse regole: metadati contraddittori sono un errore) e rende `rows`,
  `contract_version`, `active_geometry` e per colonna `name`, `type`
  (`descrivi_tipo`: senza metadati né fuso), `nullable`, `field_id` se
  c'è, e per una geometria il blocco canonico `plenora.geometry.*` senza
  `crs_definition`.
- **`validate`**: legge il piano (al più `max_plan_json_bytes`, UTF-8,
  `Pipeline::from_json`) e gli input, valida senza eseguire e rende
  `plan_format`, `plan_version`, `inputs`, `steps` (`out`, `op`) e per ogni
  output lo schema pubblicato (le colonne come in `describe`, con
  l'identità dei campi che `run` scriverà).
- **`run`**: `esegui_da_file_interrompibile`; i dati vanno nei file d'uscita
  (formato dall'estensione: `.arrow`/`.feather`/`.ipc` file, `.arrows`
  stream, `.parquet`), il documento dice per output `name`, `content_type`,
  `rows`, `columns` (mai il percorso) e per passo `out`, `op`, `rows_in`,
  `rows_out`, `division_by_zero_rows`. Una destinazione esistente è
  `conflict` senza `--overwrite`. `--output PATH` senza nome vale per un
  piano con un solo output; con più output ognuno si nomina
  (`--output NAME=PATH`, diviso al primo `=`).

## Scadenza e annullamento della CLI

`--deadline` (istante RFC 3339, come `plenora.execution.deadline` del
binding di runtime) e `--timeout-ms` (dall'avvio del comando), uno solo dei
due, diventano la scadenza dell'`Interruzione`; Ctrl-C e SIGTERM (crate
`ctrlc`, feature `termination`) alzano il segnale di annullamento. Si
controllano prima di leggere ogni input, prima di ogni passo, prima di
consegnare gli output, prima di scrivere ognuno e dopo l'ultimo: `timeout`
(exit 5, `EXECUTION_DEADLINE_EXCEEDED`) o `cancelled` (exit 130,
`EXECUTION_CANCELLED`), con la fase del punto di controllo. Dopo l'ultima
scrittura (fase `finalize`) gli output sono tutti alla destinazione e
l'effetto è `committed`, ritentativo `requires_recovery`: un annullamento
arrivato durante l'ultima scrittura non diventa un successo. Se il gestore
dei segnali non si installa, `describe`, `validate` e `run` non partono
(`internal`): dichiarano l'annullamento, e non girano senza.

## Capacità e attributi

`capabilities` descrive il binario che risponde: un'interfaccia (`cli`,
`plenora-cli-v2`, artefatto `plenora-data`) e le quattro operazioni che il
catalogo pubblico mette sulla CLI (non `data.run` 3, che sta solo su Rust e
runtime) con i suoi contratti, tipi di contenuto e controlli. Gli
`attributes` seguono il contratto `plenora-data-capability-attributes-v1`
(di questo componente; CAP-013):

| campo | significato |
| --- | --- |
| `contract` | `plenora-data-capability-attributes-v1` |
| `kernel_registry` | `plenora-data-kernel-catalog-v2`: l'operazione usa il registro di `catalog` |
| `plan_contract` | `plenora-data-plan-v1`: il formato del piano di `validate` e `run` |
| `extension_content_types.input`, `.output` | tipi in più rispetto al catalogo pubblico: `application/vnd.apache.parquet` |
| `bounded_materialization` | `true`: le tabelle si materializzano intere, entro il budget (ARROW-011) |

La superficie Rust è `plenora_cli::api::{catalogo, descrivi, valida,
esegui}` con le forme che prendono anche tabelle in memoria
(`descrivi_tabella`, `valida_ingressi`, `esegui_ingressi`,
`esegui_in_memoria`: stesso corpo, quelle che l'SDK Python chiama); la mappa
operazione → export (`plenora_cli::capacita::mappa_rust`, contratto
`plenora-data-rust-surface-v1`) nasce dalla stessa tabella, e
`tests/superficie_rust.rs` la compila da consumatore. La mappa comprende
anche `data.run` 3 ([«`data.run` 3»](#datarun-3-sul-runtime)). Dalla stessa tabella
vengono le capacità dell'SDK Python e la sua mappa dei simboli
(`capacita::documento_della`, `capacita::mappa_python`; README di
`crates/plenora-data-py`).

## `data.run` 3 sul runtime

`plenora_cli::api::esegui_artefatti` è `data.run` versione 3 (profilo
data-tools v2, DT-RUN-001..DT-RUN-008, decisione 0008 di
`plenora-contracts`): la rappresentazione runtime di un piano con output
nominati. Prende il testo della richiesta
`plenora-data-execution-input-v3` (il piano e, per ogni input e output, un
riferimento opaco ad artefatto) e un `RisolutoreArtefatti` dell'applicazione,
che legge le sorgenti, prepara e pubblica le destinazioni (RT-015);
restituisce il manifesto `plenora-data-execution-result-v3`. Non ha comando
CLI né simbolo Python, e i documenti `capabilities` della CLI e dell'SDK non
la elencano; il trasporto runtime (messaggi, autorizzazione, risoluzione dei
riferimenti) lo costruisce l'applicazione sopra questa funzione.

Il percorso, in ordine: richiesta chiusa (chiavi ripetute, campi sconosciuti,
riferimenti con la forma di un percorso e tipi fuori elenco sono
`invalid_configuration`; il piano si legge dal suo testo con le regole di un
file di piano); nomi uguali a quelli del piano e destinazioni distinte;
`prepara` del risolutore; ogni sorgente in un file temporaneo privato con
byte, SHA-256 e firma del formato dichiarato verificati; il piano con la
semantica di `data.run` 2, uscite in file temporanei; poi la pubblicazione,
una destinazione alla volta nell'ordine del piano. Fino alla pubblicazione
un errore non ha effetti (`remote_effect: none`); un fallimento della
pubblicazione porta l'effetto che il risolutore sa provare
(`PubblicazioneFallita`: `none` solo se nulla è stato scritto e nulla prima,
`partial`, `unknown`), e un'interruzione dopo la prima pubblicazione è
`partial`. `tests/run_artefatti.rs` lo prova con un risolutore strumentato
che registra l'ordine di letture e pubblicazioni e inietta i guasti; le
tabelle pubblicate sono quelle di `esegui_in_memoria` sullo stesso piano.

## Verifica e adozione

- `cargo test -p plenora-cli`: il binario come sottoprocesso (stdout,
  stderr, codice, documento intero) contro gli schemi dei contratti copiati
  in `crates/plenora-cli/tests/fixtures/contratti/` con il loro SHA-256
  (`provenienza.json`): inviluppo, errore, capacità (più CAP-005 e
  CAP-007), registro dei kernel contro `data-kernels-v2` (DT-001),
  operazioni, contratti, effetti e attributi contro `data-tools-v2`, comandi
  contro `bindings/cli-v1.json`, diagnostica per
  riga, canarini (dati e percorsi) mai nell'uscita;
- `python scripts/verifica_cli_contratti.py --binario <plenora-data>
  --contratti <checkout di plenora-contracts>`: le stesse verifiche di
  scoperta e d'errore con `jsonschema` e `tools/conformance_checks.py` dei
  contratti (il venv dei contratti ha i pacchetti);
- `python scripts/genera_manifesto_adozione.py`: il manifesto v4 dagli
  artefatti costruiti (versione e digest), con la sorgente
  `crates/plenora-cli/adozione.json` (pin, profilo, contratti). Non c'è
  un manifesto nel repository: senza un artefatto rilasciato il digest
  sarebbe di una build qualsiasi.

## Contratti adottati

Il profilo è data-tools versione 2 (`plenora-data-tools-profile-v2`, decisione
0007 di `plenora-contracts`), con il catalogo `catalogs/data-tools-v2.json`:
le differenze che alla revisione `ade868c` erano deviazioni dichiarate sono
diventate contratto, e la sorgente del manifesto
(`crates/plenora-cli/adozione.json`) non ne dichiara nessuna.

- **Piano**: `plenora-data-plan-v1` (Data Plan 1.0, [«Il piano»](runner.md#il-piano)),
  senza hash di piano; un piano dei formati 4, 5 o 6 si rifiuta con
  `invalid_plan` (DPLAN-013).
- **Kernel**: la versione è quella della semantica osservabile, uguale al
  registro comune `data-kernels-v2`; `table.transpose` è `unavailable` con
  il motivo e fuori dal `registry` del risultato (DT-001).
- **`data.run`**: la versione 2 ha `side_effect: local`, output nominati
  (`--output NAME=OUTPUT.arrow`, o `--output OUTPUT.arrow` per un piano con
  un solo output) e non è legata al runtime; la versione 3
  (`side_effect: remote`, solo superficie Rust) ne è la rappresentazione
  runtime ([«`data.run` 3»](#datarun-3-sul-runtime)).
- **Parquet**: estensione dichiarata negli attributi
  (`extension_content_types`), fuori da `content_types`.
- **Materializzazione**: `describe`, `validate` e `run` dichiarano
  `bounded_materialization` (ARROW-011 lo ammette): le tabelle stanno nel
  budget del piano ([«Budget di memoria»](runner.md#budget-di-memoria)).
- **Budget**: `max_governed_memory_bytes` (default pubblicato
  `DEFAULT_MAX_GOVERNED_MEMORY_BYTES`, 536 870 912 byte) e i limiti del
  runner (DPLAN-009..DPLAN-011); il profilo isolato non esiste.
- **SDK Python**: superficie condizionale, binding `plenora-data` /
  `plenora_data` ([README del crate](../crates/plenora-data-py/README.md)).
- **Metadati Arrow**: le regole DT-ARROW-001..DT-ARROW-004 del profilo sono
  quelle di [«Metadati Arrow»](metadati-arrow.md#metadati-arrow).

## Limiti dichiarati della CLI

- **`data.run` 3: interruzione fra le fasi, non durante il risolutore.**
  *Regola*: scadenza e annullamento si controllano prima di leggere ogni
  sorgente, nel runner e prima di ogni pubblicazione.
  *Ambito*: `api::esegui_artefatti`.
  *Hazard*: una lettura o una pubblicazione lenta del risolutore non si
  interrompe dall'interno; un'interruzione che arriva durante una
  pubblicazione ha effetto alla successiva.
  *Rientro*: un risolutore che riceva l'`Interruzione`.
- **`data.run` 3: le sorgenti passano dal disco.**
  *Regola*: ogni sorgente e ogni uscita si scrivono in una cartella
  temporanea privata, tolta alla fine, per riusare il confine di lettura e
  la scrittura atomica di `plenora-io`.
  *Ambito*: `api::esegui_artefatti`.
  *Hazard*: serve spazio su disco pari a sorgenti più uscite; un processo
  terminato a forza può lasciare la cartella temporanea.
  *Rientro*: una lettura del confine da un flusso in memoria.

- **`validate` legge le tabelle intere.**
  *Regola*: la validazione guarda solo gli schemi.
  *Ambito*: `validate`, `plenora_io::valida_da_file`.
  *Hazard*: per avere lo schema si leggono gli input interi, entro il
  budget del piano: costa tempo e memoria quanto la lettura di `run`, e un
  input oltre il budget fallisce anche in validazione.
  *Rientro*: una lettura del solo schema (footer IPC, metadati Parquet)
  con le verifiche del confine di lettura.
- **Annullamento dal sistema operativo non provato da un test.**
  *Regola*: Ctrl-C e SIGTERM diventano `cancelled`, exit 130.
  *Ambito*: `main.rs` e il gestore di `ctrlc`.
  *Hazard*: i test alzano il segnale direttamente (lo stesso
  `Arc<AtomicBool>` del gestore); che il sistema operativo consegni Ctrl-C
  al gestore lo garantisce `ctrlc`, non un test di questo repository
  (mandare Ctrl-C a un sottoprocesso vorrebbe FFI). Un secondo Ctrl-C non
  interrompe di più: il processo finisce al controllo successivo.
  *Rientro*: un test d'integrazione Unix che manda SIGTERM al sottoprocesso.
- **Controlli fra le fasi, mai dentro.** Scadenza e annullamento si
  controllano fra letture, passi e scritture ([«Limiti dichiarati del
  runner»](runner.md#limiti-dichiarati-del-runner)): la lettura di un file grande o
  un passo lungo finiscono anche oltre la scadenza.
- **Stdout non scrivibile.**
  *Regola*: CLI 2.0, sezione 4: un documento JSON completo su stdout.
  *Ambito*: ogni comando (`plenora_cli::consegna`).
  *Hazard*: con una pipe chiusa o un disco pieno il documento manca o è
  troncato; il processo esce con 5 (la categoria `io`), mai 0, non scrive un
  secondo documento e niente su stderr. Il codice non dice più l'esito del
  comando: un `run` può aver scritto i suoi output, quindi chi riceve 5
  senza un documento completo tratta l'esito come ignoto.
  *Rientro*: nessuno dentro il processo (stdout è il solo canale del
  contratto); un chiamante che legge stdout fino in fondo non lo vede.
- **Panici fuori dal thread principale.**
  *Regola*: un panico diventa `internal` (CLI 2.0, sezione 6).
  *Ambito*: i thread che la CLI non intercetta con `catch_unwind`: il
  thread del gestore di Ctrl-C (`ctrlc` vi chiama `expect`).
  *Hazard*: l'hook di `main.rs` conta, senza payload, ogni panico fuori
  dalle barriere di dipendenza in qualunque thread
  (`panic_policy::panici_fuori_dalle_barriere`); la base si prende subito
  dopo l'installazione dell'hook e prima di avviare il gestore
  (`esegui_invocazione_dal`). Un panico contato prima del controllo finale
  dell'invocazione (l'ultima lettura del conto in `proteggi`) trasforma un
  successo in `internal` (exit 70, effetto `unknown` per `run`); un panico
  in corsa con quel controllo, o successivo (fino alla scrittura su
  stdout), non si osserva, e l'invocazione resta `ok`: altre letture
  sposterebbero solo la finestra, e il conto non è una sincronizzazione
  con il thread che va in panico. È accettabile perché a quel punto il
  lavoro è concluso e il suo esito è vero: si perde solo la possibilità di
  annullarlo in quella finestra. Il controllo è alla fine, non ai punti di
  controllo dell'annullamento: se il thread del gestore muore prima, il
  comando prosegue senza annullamento fino in fondo, e solo allora
  fallisce. Il conto è del processo: chi usa la libreria con più
  invocazioni concorrenti nello stesso processo vede il panico di una,
  contato prima del controllo finale di un'altra, far fallire anche
  quella con un `internal` falso al posto di un successo vero. Il test
  della CLI (`tests/panico_di_un_altro_thread.rs`) prova la base passata a
  `esegui_invocazione_dal`; che `main.rs` la prenda prima di avviare il
  gestore, e il passaggio da `esegui_invocazione_os`, non sono provati da
  un test (`main.rs` resta minimo e senza test per scelta).
  *Rientro*: un gestore dell'annullamento per invocazione, sorvegliato e
  riunito (`join`) prima del controllo finale, così che un suo panico sia
  sempre visto; il conto letto anche ai punti di controllo
  dell'`Interruzione`; un conto per invocazione, se servirà la concorrenza
  in un processo.
- **Aborti senza inviluppo.**
  *Regola*: CLI 2.0, sezione 4: un documento su stdout e niente su stderr
  in ogni caso.
  *Ambito*: ogni comando.
  *Hazard*: un'allocazione impossibile (la libreria standard scrive
  «memory allocation of N bytes failed» su stderr e abortisce), uno stack
  esaurito e un panico dentro un `Drop` durante un altro panico terminano
  il processo senza inviluppo e senza passare dall'hook; il codice d'uscita
  è quello dell'aborto del sistema operativo, non uno della tabella sopra.
  Con file costruiti apposta l'allocazione impossibile è raggiungibile
  (limite «File costruiti apposta: aborto del processo» di
  [«File»](file.md#file)). Garanzia indebolita: un chiamante che non trova un
  documento su stdout deve trattare l'esito come ignoto.
  *Rientro*: la CLI in un processo figlio sorvegliato da un processo padre
  che trasforma l'aborto in un errore `internal` con effetto `unknown`.
- **Messaggi delle config con i valori del piano.**
  *Regola*: i messaggi pubblici non portano valori di righe o colonne
  ([«Errori»](errori.md#errori)).
  *Ambito*: config dei passi rifiutate dalla deserializzazione
  (`config non valida: …`), con il testo di `serde`.
  *Hazard*: il testo può citare un valore scritto nella config del piano
  (un tipo sbagliato, una variante sconosciuta): è testo del piano, non dei
  dati, ma finisce nel messaggio pubblico. La lettura del piano intero
  (`Pipeline::from_json`) invece dice solo genere e posizione.
  *Rientro*: la stessa riduzione per le config, quando i messaggi dei
  campi sconosciuti (oggi utili a chi scrive il piano) avranno un codice.
- **Attributi senza schema JSON.** Il contratto
  `plenora-data-capability-attributes-v1` e i documenti dei risultati
  (`plenora-data-*-v1`) sono descritti qui, non da uno schema JSON
  pubblicato; i test ne verificano la forma campo per campo.
