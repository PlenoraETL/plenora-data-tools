# plenora-data

SDK Python di plenora-data-tools: distribuzione `plenora-data`, pacchetto
d'import `plenora_data` (*Python SDK 1.0* di `plenora-contracts`, commit
`ade868cf89c6652cffe20019e7194b383384ee78`). Espone le quattro operazioni
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
risultato.result                          # plenora-data-execution-result-v1

# Output su file (formato dall'estensione: .arrow/.feather/.ipc, .arrows, .parquet)
pd.run(piano, {"t": "dati.parquet"},
       outputs={"alti": "alti.arrow", "ordinati": "ordinati.parquet"},
       overwrite=True)

# Asincrono, stessa semantica
risultato = await pd.arun(piano, {"t": tabella}, cancel=pd.CancellationToken())
```

| operazione | sincrona | asincrona | risultato |
| --- | --- | --- | --- |
| `data.catalog` | `catalog()` | `acatalog()` | `dict`, `plenora-data-kernel-catalog-v1` |
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
  I buffer passano senza copia; schema e metadati (`plenora.*`, GeoArrow
  `ARROW:extension:name`, chiavi sconosciute) arrivano intatti.
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
controllata prima di ogni input (anche fra un blocco Arrow e l'altro),
prima di ogni passo, prima di consegnare gli output, prima di scrivere
ognuno e dopo l'ultimo. `catalog` non ha controlli, come sulla CLI.

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
  `asyncio.CancelledError` con l'esito come causa.

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
- scambio Arrow per IPC (copia): qui Arrow C Stream Interface, senza copia;
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

## Deviazioni dai contratti

- **Python SDK fuori dal catalogo di data-tools.**
  *Regola*: Public Catalogs 1.0 e il profilo data-tools: `data-tools-v1`
  seleziona `python_sdk` come `not_applicable`, nessuna operazione elenca la
  superficie `python_sdk`, e `bindings/python-sdk-v1.json` ha `artifact:
  null` per questo componente.
  *Ambito*: tutto il pacchetto.
  *Hazard*: un consumatore che legge solo il catalogo comune non sa che
  l'SDK esiste; chi lo usa vede la superficie dichiarata da
  `capabilities()` (`surfaces: ["python_sdk"]`), che è la verità
  dell'artefatto.
  *Rientro*: l'aggiornamento dei contratti (sotto).
- **Effetto di `data.run`.** Come sulla CLI: `side_effect: local`, perché
  con `outputs` scrive file; senza `outputs` non ha effetti fuori dal
  processo. *Rientro*: il catalogo distingue l'effetto per superficie.
- Le deviazioni della CLI sul formato del piano, le versioni dei kernel,
  `table.transpose`, la materializzazione limitata e il frammento del
  budget valgono identiche qui (README della radice, «CLI
  `plenora-data`», deviazioni; `crates/plenora-cli/adozione.json` le
  dichiara per entrambe le superfici).

## Limiti dichiarati

- **Ctrl-C dopo l'ultimo controllo.**
  *Regola*: un Ctrl-C ferma l'operazione e l'eccezione porta l'esito.
  *Ambito*: forme sincrone sul thread principale.
  *Hazard*: se il segnale arriva quando il lavoro ha già passato il suo
  ultimo controllo, il lavoro finisce con successo, il risultato si scarta
  e `KeyboardInterrupt` esce senza `__cause__`: con `outputs` i file sono
  scritti. Chi riceve `KeyboardInterrupt` senza causa da un `run` con
  `outputs` tratta l'esito come ignoto.
  *Rientro*: rendere l'esito riuscito come attributo dell'eccezione.
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
- **Stream di più blocchi.**
  *Regola*: ogni tabella sta nel budget del piano.
  *Ambito*: un input in memoria con più blocchi (una `pyarrow.Table` a più
  chunk, un `RecordBatchReader`).
  *Hazard*: il runner vuole un blocco per tabella: i blocchi si uniscono
  (`concat_batches`, una copia) prima del controllo del budget, che vede
  solo la tabella unita; la copia transitoria non è contata. Una tabella di
  un blocco passa senza copia.
  *Rientro*: un runner a più blocchi per tabella.
- **Import con il GIL.** Gli oggetti Arrow si importano sul thread del
  chiamante con il GIL (lo stream C chiama il produttore): per una
  `pyarrow.Table` è una copia di puntatori, per un produttore Python è il
  suo codice. Il lavoro vero (lettura dei file, runner, scrittura) gira
  senza GIL.
- **Panici provati dal codice, non da una prova Python.** Ogni funzione del
  modulo e il thread di lavoro intercettano i panici (`catch_unwind`) e li
  rendono `internal`; nessuna prova Python provoca un panico (il modulo non
  ha un punto d'ingresso per farlo). Gli aborti del processo (allocazione
  impossibile, stack esaurito) non si intercettano, come sulla CLI (README
  della radice, «Aborti senza inviluppo»).
- **Serie di pyarrow.** La dipendenza è `pyarrow>=25,<26`, la serie
  provata dalla suite; pyarrow non pubblica tipi, quindi per mypy i suoi
  oggetti sono `Any`.
- **Executor del loop.** La forma asincrona usa l'executor di default del
  loop: con l'executor saturo la chiamata aspetta un thread libero, e
  l'annullamento di un task aspetta che il lavoro arrivi al suo controllo
  successivo (al più un passo del piano).
- **Messaggi delle config.** Come sulla CLI, un messaggio di config non
  valida può citare un valore scritto nel piano (README della radice,
  «Limiti dichiarati della CLI»).

## Aggiornamento dei contratti

Ciò che l'unico aggiornamento di `plenora-contracts` deve dire per questo
componente:

1. `catalogs/data-tools-v1.json`: `target_surfaces.python_sdk` da
   `not_applicable` a `conditional` (il profilo non richiede l'SDK, ma
   quando c'è vale Python SDK 1.0); `"python_sdk"` in `surfaces` di
   `data.catalog`, `data.describe`, `data.validate`, `data.run`; per
   `data.run` l'effetto per superficie (`local` quando la superficie scrive
   file), o la deviazione resta.
2. `bindings/python-sdk-v1.json`, sezione `plenora-data-tools`: il documento
   di `plenora_cli::capacita::mappa_python()`, cioè
   `"artifact": "plenora-data / plenora_data"`, `"discovery":
   ["plenora_data.version", "plenora_data.capabilities"]` e per ognuna delle
   quattro operazioni (versione 1, `"requirement": "required"`, come nel
   catalogo) gli entrypoint `plenora_data.catalog`/`acatalog`,
   `describe`/`adescribe`, `validate`/`avalidate`, `run`/`arun`. La prova
   `superficie_python.rs` fallisce quando i contratti fissati hanno già la
   sezione, per riallinearla.
3. `specs/surfaces/SURFACE-BINDINGS-1.0.md`, sezione 4: la riga
   `data-tools | plenora-data | plenora_data`.
4. `profiles/data-tools.md`: Python SDK 1.0 fra i contratti applicabili
   «when exposed», e «Python SDK: optional» fra le superfici.
5. Il manifesto di adozione v4 del componente porterà il wheel con
   `api_modes: ["sync", "async"]` e `plenora-python-sdk-v1` conforme
   (`scripts/genera_manifesto_adozione.py`); la deviazione «Python SDK
   fuori dal catalogo» cade con i punti 1-3.
