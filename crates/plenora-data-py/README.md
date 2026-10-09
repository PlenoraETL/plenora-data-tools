# plenora-data

SDK Python di plenora-data-tools: distribuzione `plenora-data`, pacchetto
d'import `plenora_data` (*Python SDK 1.0* e profilo data-tools versione 2 di
`plenora-contracts`, commit `1e902dfaab5819c1d9ce785878d5b26dbeae48b3`).
Espone le quattro operazioni
del catalogo pubblico, le stesse della CLI `plenora-data`, sulle tabelle
Arrow in memoria o su file.

Il modulo nativo (`plenora_data._native`, PyO3) chiama le stesse funzioni
della CLI (`plenora_cli::api`), nelle forme con input in memoria: ogni
operazione ha una sola implementazione, con la stessa validazione, lo
stesso budget, gli stessi documenti e gli stessi errori su ogni superficie.
La tabella delle operazioni (`plenora_cli::operazioni::OPERAZIONI`) è
l'unica fonte delle capacità e della mappa dei simboli Python.

## Uso

```python
import pyarrow as pa
import plenora_data as pd

tabella = pa.table({"id": [3, 1, 2], "importo": [3.5, 1.0, 2.25]})
piano = {
    "version": 1,
    "inputs": ["t"],
    "steps": [
        {"out": "alti", "op": "table.filter", "in": ["t"],
         "config": {"column": "id", "operator": ">", "value": 1}},
        {"out": "ordinati", "op": "table.sort", "in": ["alti"],
         "config": {"columns": ["id"]}},
    ],
    "outputs": ["alti", "ordinati"],
}

pd.describe(tabella)                      # plenora-data-description-v1
pd.validate(piano, {"t": tabella})        # plenora-data-plan-validation-result-v1
risultato = pd.run(piano, {"t": tabella}, timeout=30)
risultato.tables["ordinati"]              # pyarrow.Table
risultato.result                          # plenora-data-execution-result-v2

# Output su file (formato dall'estensione: .arrow/.feather/.ipc, .arrows, .parquet)
pd.run(piano, {"t": "dati.parquet"},
       outputs={"alti": "alti.arrow", "ordinati": "ordinati.parquet"},
       overwrite=True)

# Asincrono, stessa semantica
risultato = await pd.arun(piano, {"t": tabella}, cancel=pd.CancellationToken())
```

| operazione | sincrona | asincrona | risultato |
| --- | --- | --- | --- |
| `data.catalog` | `catalog()` | `acatalog()` | `dict`, `plenora-data-catalog-result-v2` |
| `data.describe` | `describe(data, ...)` | `adescribe` | `dict`, `plenora-data-description-v1` |
| `data.validate` | `validate(plan, inputs, ...)` | `avalidate` | `dict`, `plenora-data-plan-validation-result-v1` |
| `data.run` | `run(plan, inputs, *, outputs, overwrite, ...)` | `arun` | `RunResult(result, tables)` |

Scoperta: `version()` (uguale ai metadati installati, al nome del wheel e a
`component_version`) e `capabilities()` (il documento `capabilities-v2`
dell'SDK: interfaccia `python_sdk`, contratto `plenora-python-sdk-v1`,
artefatto `plenora-data`; operazioni, contratti, tipi di contenuto, effetti
e controlli identici a quelli della CLI, tranne `surfaces`).

- **Tabelle in ingresso**: un percorso (`str` o `os.PathLike`: Arrow IPC
  file o stream, Parquet, letti come li legge la CLI) o un oggetto Arrow:
  `pyarrow.Table`, `pyarrow.RecordBatch`, `pyarrow.RecordBatchReader`, o
  qualunque oggetto con `__arrow_c_stream__` (Arrow PyCapsule Interface).
  I buffer allineati passano senza copia (limite «Senza copia, non
  sempre»); schema e metadati (`plenora.*`, GeoArrow
  `ARROW:extension:name`, chiavi sconosciute) arrivano intatti. Ogni stream
  si importa dentro il budget del piano (o quello di default per
  `describe`), contando i blocchi mentre arrivano: l'import si ferma con
  `resource_limit` al primo blocco di troppo, senza chiedere il successivo.
  È una riduzione del rischio, non un tetto di memoria (limite «Budget
  degli stream»).
- **Tabelle in uscita**: `pyarrow.Table` di un blocco, senza copia, con lo
  schema pubblicato (`plenora.contract.version`, `plenora.field_id`, blocco
  canonico delle geometrie); nel documento il loro `content_type` è
  `application/vnd.apache.arrow.stream`. Con `outputs` le tabelle vanno su
  file (scrittura atomica per file) e `tables` è vuoto.
- **Piano**: un `Mapping` (serializzato in JSON senza NaN né infiniti), il
  testo JSON (`str`) o il percorso di un file (`os.PathLike`, per esempio
  `pathlib.Path`). Un `str` è sempre testo JSON, mai un percorso.
- **Nomi**: `inputs` dà una tabella per ogni input del piano, `outputs` un
  percorso per ogni output; un nome in più, mancante o ripetuto è
  `invalid_configuration` prima di leggere qualunque dato, senza ripetere il
  nome ricevuto nel messaggio. `overwrite` senza `outputs` si rifiuta
  (nessun argomento ignorato in silenzio).

## Errori

La radice è `PlenoraError` (sottoclasse di `RuntimeError`, come in
plenora-database-tools), con una sottoclasse per ognuna delle 19 categorie
di `plenora-error-v1` (`PlenoraTimeoutError`, `PlenoraCancelledError`,
`PlenoraResourceLimitError`, …). Ogni istanza ha come attributi
`category`, `phase`, `remote_effect`, `retry` (dict), `code`, `message`,
`details` (con `row_diagnostics`, il documento `plenora-row-diagnostics-v1`)
e `to_dict()` rende il documento `error-v1` intero: nessun testo da
leggere per decidere.

Anche la preparazione degli argomenti è classificata: un'eccezione di un
argomento (un `Mapping` che fallisce mentre si itera, un intero che non sta
in un float, un oggetto Arrow che fallisce mentre lo si guarda) diventa
`invalid_configuration` o `data_mapping` con un testo fisso, mai il suo.

Il documento viene dalla proiezione pubblica di `PlenoraError` di
`plenora-core` (`public_projection`), la stessa dell'inviluppo d'errore della
CLI: stessi assi, stesso codice (`EXECUTION_DEADLINE_EXCEEDED`,
`EXECUTION_CANCELLED`, i codici CRS), messaggi senza valori di righe o
colonne e senza percorsi (l'I/O dice il tipo d'errore del sistema, mai il
percorso). Gli argomenti rifiutati dal pacchetto Python hanno la stessa
forma (`invalid_configuration`, fase `prepare`, come gli argomenti della
CLI). Nessuna eccezione nativa attraversa il confine: un'eccezione Python
inattesa dal modulo nativo diventa `PlenoraInternalError` senza il suo
testo, un'eccezione del produttore di uno stream Arrow diventa
`PlenoraDataMappingError` senza il suo testo, un panico Rust diventa
`PlenoraInternalError` senza payload e senza nulla su stderr (l'hook di
panico del modulo è silenzioso, `plenora_core::panic_policy`). Le
eccezioni si ricostruiscono con `pickle`.

## Scadenza e annullamento

`describe`, `validate` e `run` (e le forme asincrone) accettano
`timeout` (secondi) o `deadline` (`datetime` con fuso; senza fuso si
rifiuta), uno solo dei due, e `cancel` (`CancellationToken`, alzabile da
qualunque thread). Sono l'`Interruzione` del runner, la stessa della CLI:
controllata all'ingresso del modulo nativo (fase `prepare`, prima di
qualunque lavoro), prima di ogni input (anche fra un blocco Arrow e
l'altro), prima di ogni passo, prima di consegnare gli output, prima di
scrivere ognuno e dopo l'ultimo, e di nuovo alla consegna del risultato.
`catalog` non ha controlli, come sulla CLI.

`timeout` vale dall'ingresso della chiamata pubblica: il pacchetto fissa
l'istante sul clock monotono prima di mettere in coda una chiamata
asincrona, e una scadenza passata mentre la chiamata aspettava un thread
dell'executor la ferma prima di qualunque lavoro. Un gettone già alzato
ferma la chiamata allo stesso punto, senza leggere né scrivere nulla. Un
annullamento (gettone, Ctrl-C, task) arrivato mentre il lavoro finiva non
diventa un successo: è `PlenoraCancelledError` in fase `finalize`, con
effetto `committed` (ritentativo `requires_recovery`) se gli output sono
stati scritti, `none` altrimenti.

Il lavoro gira in un thread suo, senza il GIL; il thread del chiamante
aspetta senza il GIL e ogni 10 ms porta i gettoni nel segnale del runner e
chiama `PyErr_CheckSignals`:

- **Ctrl-C** sul thread principale ferma il lavoro al controllo successivo
  e propaga `KeyboardInterrupt` (mai trasformato in un'altra eccezione), con
  l'esito del lavoro come `__cause__`: `PlenoraCancelledError` con
  l'effetto vero (`none`, `partial` o `committed` se un output era già
  scritto).
- **Forma asincrona**: la chiamata gira nell'executor del loop, il loop non
  si blocca mai. Annullare il task alza un gettone interno, aspetta che il
  lavoro si fermi (nessun lavoro continua dopo l'annullamento) e propaga
  `asyncio.CancelledError` con l'esito come causa, anche quando il lavoro
  era già finito con successo (annullamento alla consegna, con l'effetto
  vero). In Python 3.10 chi aspetta il task riceve un `CancelledError`
  nuovo, con quello sollevato dentro il task come `__context__`.

## Scelte rispetto a plenora-database-tools

Adottate: PyO3 0.29.2 con ABI stabile `abi3-py310` (un wheel per
piattaforma), maturin 1.15.0 fissata, `extension-module` solo nella build del
wheel (`pyproject.toml`), modulo nativo privato `_native` con i wrapper
Python sopra, gerarchia `PlenoraError` sotto `RuntimeError` con una classe
per categoria, PEP 561 (`py.typed`, stub del modulo nativo), consumatore
statico per mypy `--strict` su ogni Python, requisiti con versioni esatte,
prova del wheel installato fuori dal checkout (`site-packages`, versione,
modulo nativo), action fissate per SHA.

Scartate o migliorate:

- l'hook di panico: niente su stderr, un panico è `internal` senza payload;
- la scadenza dichiarata e non accettata: qui `timeout`/`deadline` e
  annullamento sono parametri, applicati dal runner;
- `code` assente dagli attributi: qui c'è, con `to_dict()`;
- la versione duplicata fra `Cargo.toml` e `pyproject.toml`: qui
  `dynamic = ["version"]`, una sola fonte;
- mypy con `ignore_errors` sul pacchetto stesso: qui `--strict` copre il
  pacchetto oltre al consumatore;
- scambio Arrow per IPC (copia): qui Arrow C Stream Interface, senza copia
  per i buffer allineati;
- runtime tokio: non serve (nessun I/O di rete); un thread di lavoro per
  chiamata.

## `unsafe` e macro PyO3

`unsafe_code = "forbid"` del workspace vale anche per questo crate, e il
codice del crate non ha `unsafe` (un blocco `unsafe` scritto qui non
compila). Le macro di PyO3 (`#[pymodule]`, `#[pyfunction]`, `#[pyclass]`)
generano codice `unsafe` per l'interfaccia C di CPython: è espansione di
una macro di un altro crate, che il lint `unsafe_code` non esamina (rustc
non riporta i lint di questo tipo dentro le macro esterne), ed è codice di
`pyo3`, come l'`unsafe` della C Data Interface sta in `arrow-array` e
`arrow-pyarrow`. È la stessa soluzione di plenora-database-tools, che tiene
lo stesso `forbid` di workspace sul suo crate PyO3: nessun `allow` né
eccezione al lint. Anche il gate anti-panico (`-D unsafe-code` e gli altri
lint, `--workspace --lib`) passa sul crate.

## Che cosa non c'è

- risorse a lunga vita: l'SDK è fatto di funzioni; non c'è nulla da aprire
  o chiudere, e la sezione 5 di Python SDK 1.0 (`close`, `aclose`,
  quarantena) non ha oggetto;
- rete, credenziali, segreti: nessuno (sezione 9 senza oggetto);
- pubblicazione: la CI costruisce e prova i wheel, non li pubblica; SBOM,
  attestazioni e controllo del tag di rilascio non ci sono, e un wheel
  della CI non è un artefatto rilasciato.

## Verifica

- `.github/workflows/sdk-python.yml`: wheel con maturin su Linux
  (manylinux 2_34) e Windows; ogni wheel installato e provato con Python 3.10
  e 3.14 da `scripts/verifica_sdk_python.py` (nome, metadati, PEP 561,
  import da `site-packages`, versioni, modulo nativo byte per byte, suite
  pytest intera senza test saltati); mypy `--strict` su 3.10 e 3.14
  (`typing/mypy.ini`, `typing/sdk_contract.py`).
- `cargo test -p plenora-cli --test superficie_python`: il documento delle
  capacità dell'SDK contro `capabilities-v2`, la sezione Python contro
  `surface-bindings-v1` dei contratti, l'equivalenza fra le forme in memoria
  e da file dell'API (stessi documenti, stesse tabelle).
- L'istante fra la fine del lavoro e il controllo della consegna si prova
  in modo deterministico con una sonda privata del modulo nativo
  (`_native._sonda_consegna`, non API, inerte senza registrazione): la prova
  alza il gettone o arma SIGINT esattamente lì, e fallisce se il controllo
  alla consegna manca (verificato togliendolo). Allo stesso modo il lavoro
  in corso: una seconda sonda (`_native._sonda_lavoro`) gira nel thread
  del lavoro prima che cominci, alza gettone, SIGINT o annullamento del
  task e tiene il lavoro finché l'interruzione non gli arriva (o la sua
  scadenza non scatta). Nessuna prova dei controlli dipende da sleep,
  timer o durate misurate: senza la sorveglianza durante l'attesa, o senza
  la scadenza passata al lavoro, le prove falliscono (verificato).
- La suite Python (`python/tests`) valida errori, capacità e diagnostica
  con `jsonschema` contro le copie degli schemi dei contratti
  (`crates/plenora-cli/tests/fixtures/contratti`, SHA-256 verificato).

Costruire e provare in locale (Python >= 3.10 nel `PATH` anche per
`cargo build --workspace`, che collega libpython):

```sh
pip install -r requirements-sdk-build.txt
maturin build --release --locked --out dist -m crates/plenora-data-py/Cargo.toml
pip install -r requirements-sdk-tests.txt
pip install --no-deps dist/plenora_data-*.whl
python scripts/verifica_sdk_python.py --wheel dist/plenora_data-<versione>-<tag>.whl
```

## Corrispondenza con Python SDK 1.0

| sezione | dove |
| --- | --- |
| 2, identità | `pyproject.toml` (`plenora-data`, `requires-python >=3.10`, versione dal crate), `version()` in `src/lib.rs`, prova in `scripts/verifica_sdk_python.py` |
| 3, tipi e superficie | `py.typed`, `_native.pyi`, annotazioni in `_api.py` ed `errors.py`, `__all__` in `__init__.py`; risultati `dict` e `RunResult`, tabelle PyArrow |
| 4, sync e async | `_api.py` (`_prepara_*` comuni, `_in_thread`); prova in `tests/test_superficie.py` (firme uguali) e `tests/test_controlli.py` |
| 5, ciclo di vita | senza oggetto (nessuna risorsa a lunga vita) |
| 6, errori | `errors.py`, `src/errori.rs`; prova in `tests/test_errori.py` |
| 7, capacità | `capabilities()`, `plenora_cli::capacita::documento_della`; argomenti rifiutati, mai ignorati |
| 8, annullamento, scadenza, budget | `src/controlli.rs`, `CancellationToken`; budget dal piano (`limits`) |
| 9, sicurezza | senza rete né segreti; messaggi senza dati né percorsi |
| 10, verifica del wheel | `scripts/verifica_sdk_python.py`, `.github/workflows/sdk-python.yml` |
| 11, compatibilità | SemVer del componente (versione del workspace) |
| 12, identità delle operazioni | `export_python` in `plenora_cli::operazioni::OPERAZIONI`, `plenora_cli::capacita::mappa_python` |

## Contratti adottati

Il catalogo `data-tools-v2` seleziona la superficie `python_sdk` per le quattro
operazioni, e la sezione di questo componente in `bindings/python-sdk-v1.json`
è, alla lettera, `plenora_cli::capacita::mappa_python()`
(`tests/superficie_python.rs` lo verifica sul commit fissato). `data.run`
dichiara `side_effect: local`: con `outputs` scrive file, senza non ha effetti
fuori dal processo. Le altre scelte del profilo v2 (piano, versioni dei
kernel, `table.transpose`, materializzazione, budget) valgono identiche
sulla CLI ([«Contratti adottati»](../../docs/cli.md#contratti-adottati)).
Nessuna deviazione.

## Limiti dichiarati

- **Annullamento dopo la consegna.**
  *Regola*: un annullamento ferma l'operazione e l'eccezione porta l'esito.
  *Ambito*: tutte le operazioni con controlli.
  *Hazard*: l'ultimo controllo è alla consegna del risultato; un gettone
  alzato, o un Ctrl-C arrivato, dopo quel controllo trova la chiamata già
  conclusa con successo, e il `KeyboardInterrupt` esce dal codice Python
  successivo come per qualunque funzione già ritornata. Mai un successo
  taciuto: il risultato è stato consegnato.
  *Rientro*: nessuno; dopo la consegna l'operazione è finita.
- **Ctrl-C dentro un produttore Python.**
  *Regola*: Ctrl-C diventa `KeyboardInterrupt`.
  *Ambito*: un input che è un `RecordBatchReader` costruito da un
  generatore Python.
  *Hazard*: se il segnale interrompe il generatore, pyarrow chiude lo stream
  con un errore e l'operazione fallisce con `PlenoraDataMappingError`
  (fase `read`, nessun effetto) invece che con `KeyboardInterrupt`. Mai un
  successo.
  *Rientro*: riconoscere l'interruzione dal produttore, se pyarrow la
  distinguerà nello stream.
- **Budget degli stream: conti per input, non per allocazione.**
  *Regola*: ogni tabella sta nel budget del piano.
  *Ambito*: input in memoria (`pyarrow.Table` a più chunk,
  `RecordBatchReader`, altri produttori Arrow).
  *Hazard*: i blocchi si contano con i loro byte di dati
  (`plenora_core::memoria::byte_dati`) contro il budget meno le tabelle già
  importate, e l'import si ferma al primo blocco di troppo; con più blocchi
  l'unione (`concat_batches`, una copia) vuole spazio per blocchi e copia
  insieme, e il picco si stima per eccesso
  (`plenora_core::memoria::picco_unione`): la memoria dei blocchi, più per
  la copia due volte (crescita per raddoppio) i loro byte di dati e una
  bitmap di validità piena su ogni nodo (Arrow la materializza appena un
  blocco ha dei null), più 320 byte per nodo del tipo per l'arrotondamento
  dei buffer, più, per i tipi che `concat` unisce per il percorso generico
  (`FixedSizeList`, `Union`, e sotto di loro liste, struct e map), 16 byte
  per riga del padre per ogni discendente, che quel percorso prealloca
  anche quando resta vuoto. Un oracolo
  (`crates/plenora-core/tests/picco_unione.rs`) la confronta con il picco
  vero (allocazioni di blocchi e unione) su 4.500 casi generati: 19 tipi
  (primitivi, booleani, stringhe e binari anche `Large` e `View`,
  decimali, istanti, liste, struct, dizionari, `Null`, run-end,
  `FixedSizeList` di primitivi e di liste vuote) più union sparse, fette,
  null assenti, sparsi o totali mescolati fra i blocchi. I transitori
  interni di `concat` (le tabelle della fusione dei dizionari) non sono nel
  conto né nell'oracolo, e un tipo annidato fuori dall'oracolo può
  superare la stima: il budget dell'import riduce il rischio di esaurire la
  memoria, non è un tetto. Il tetto vero è il limite di memoria del
  processo dato dal sistema operativo. Il blocco che supera il budget è già in memoria quando si
  rifiuta (lo ha prodotto il produttore), e le allocazioni del produttore
  (per esempio un generatore Python che costruisce i suoi blocchi) non si
  vedono. Le tabelle in memoria si riservano nel budget prima di leggere
  qualunque file (`plenora-io`, `carica_ingressi`). La tabella finale passa
  poi dal conto per allocazione del runner. Hazard di un superamento:
  esaurimento della memoria (il processo muore), mai un risultato sbagliato.
  *Rientro*: un runner a più blocchi per tabella, senza unione; fino ad
  allora il limite di memoria del processo.
- **Senza copia, non sempre.**
  *Regola*: le tabelle passano fra Python e il runner senza copia.
  *Ambito*: l'import per l'Arrow C Data Interface.
  *Hazard*: `arrow-array` 60 riallinea con una copia un buffer che non
  rispetta l'allineamento richiesto dal tipo (un produttore che esporta
  buffer sotto-allineati); quella copia non è contata nel budget
  dell'import. I buffer di pyarrow sono allineati e passano senza copia; in
  uscita i buffer del runner sono quelli di arrow-rs, allineati, e pyarrow
  li importa senza copia.
  *Rientro*: contare i buffer riallineati, se un produttore reale li darà.
- **Import con il GIL.** Gli oggetti Arrow si importano sul thread del
  chiamante con il GIL (lo stream C chiama il produttore): per una
  `pyarrow.Table` è una copia di puntatori, per un produttore Python è il
  suo codice. Il lavoro vero (lettura dei file, runner, scrittura) gira
  senza GIL.
- **Panici provati dal codice, non da una prova Python.** Ogni funzione del
  modulo e il thread di lavoro intercettano i panici (`catch_unwind`) e li
  rendono `internal`; nessuna prova Python provoca un panico (il modulo non
  ha un punto d'ingresso per farlo). Gli aborti del processo (allocazione
  impossibile, stack esaurito) non si intercettano, come sulla CLI («Aborti
  senza inviluppo» in
  [«Limiti dichiarati della CLI»](../../docs/cli.md#limiti-dichiarati-della-cli)).
- **Serie di pyarrow.** La dipendenza è `pyarrow>=25,<26`, la serie
  provata dalla suite; pyarrow non pubblica tipi, quindi per mypy i suoi
  oggetti sono `Any`.
- **Executor del loop.** La forma asincrona usa l'executor di default del
  loop: con l'executor saturo la chiamata aspetta un thread libero (la
  scadenza corre da prima, dall'ingresso di `arun`), e l'annullamento di un
  task aspetta che il lavoro arrivi al suo controllo successivo (al più un
  passo del piano).
