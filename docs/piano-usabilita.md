# Piano di usabilita' — da motore verificato a componente utilizzabile

Stato di partenza (2026-08-16): il motore e' corretto e verificato — 1294 test
verdi, gate anti-panic, clippy pedantic/nursery, 142 operazioni a catalogo, ADR
e deroghe aggiornate — ma **non e' ancora usabile da qualcuno che non lo abbia
scritto**. Manca tutto cio' che sta fra il motore e chi lo usa: un modo per
sapere cosa contiene un input, un piano di esempio da cui partire, un binario
da scaricare, una libreria Python, un giro end-to-end che funzioni al primo
tentativo.

Questo piano copre esattamente quel divario.

**Riferimento operativo: `plenora-database-tools`.** Il componente gemello ha
gia' CLI e SDK Python in esercizio (`plenora-database` 0.9.2, wheel abi3-py310,
21 eccezioni tipizzate, chunk Arrow IPC self-contained). Non si riprogetta cio'
che li' funziona: si adotta, e ogni scostamento e' dichiarato nella matrice
§0 con la sua ragione. Il confronto e' stato fatto in sola lettura; quel
repository non e' stato toccato.

**Vincolo trasversale, gia' applicato al codice:** ogni superficie nuova —
CLI, SDK Python, esempi — usa **esclusivamente** il percorso con contratto
(`Inputs::strict` + `add_with_contract`). Il percorso permissivo e' deprecato
in Rust per non rompere i chiamanti esistenti; fuori da li' non esiste.

---

## 0. Matrice di allineamento con `plenora-database`

Legenda: **C** compatibile (si adotta identico) · **A** adattato (stessa forma,
dettaglio diverso per il dominio) · **D** divergenza intenzionale (scelta
diversa, motivata).

### Struttura dell'SDK Python

| # | Aspetto | `plenora-database` | `plenora-data-tools` | |
|---|---|---|---|---|
| 1 | Layout | crate `plenora-database-py`, sorgenti Python in `python/plenora_database/`, modulo nativo `plenora_database._native` | crate `plenora-data-tools-py`, `python/plenora_data_tools/`, modulo nativo `plenora_data_tools._native` | **C** |
| 2 | Wrapper | ogni API pubblica e' Python idiomatico sopra `_native`; il nativo non e' superficie | identico: `_native` resta privato, i wrapper danno docstring, default e validazione degli argomenti | **C** |
| 3 | Build | `maturin>=1.7,<2.0`, `python-source = "python"`, `module-name = "pkg._native"`, `features = ["pyo3/extension-module"]` | identico | **C** |
| 4 | ABI | `pyo3` con `abi3-py310`, `requires-python = ">=3.10"`, un wheel per piattaforma | identico (decisione 3) | **C** |
| 5 | Tipi | `py.typed` + stub `.pyi` affiancati a ogni modulo, inclusi via `[tool.maturin] include` | identico, piu' `mypy --strict` sui test dell'SDK | **C** |
| 6 | Versione | il crate PyO3 ha versione PROPRIA (`0.9.2`), non eredita il workspace; `Cargo.toml` e `pyproject.toml` devono coincidere | identico, con un test che confronta le due versioni | **C** |
| 7 | Asincrono | `AsyncSession`/`AsyncTransaction` su `pyo3-async-runtimes` + tokio | **nessuna API async**: l'esecuzione di un piano e' CPU-bound e non attende I/O di rete. Si rilascia il GIL e basta | **D** |

### Errori

| # | Aspetto | `plenora-database` | `plenora-data-tools` | |
|---|---|---|---|---|
| 8 | Gerarchia | `PlenoraError(RuntimeError)` + 21 sottoclassi `Plenora*Error` | stessa **forma**, **18** sottoclassi: le categorie di `plenora_core::ErrorCategory` coincidono una a una con quelle del gemello. Mancano solo `ConcurrentModification` e `CommitOutcomeUnknown`, che sono transazionali | **A** |
| 9 | Attributi | `category`, `phase`, `retry`, `remote_effect`, `provider`, `execution_id`, `diagnostics` | gli stessi sette: `provider` e' sempre `None` (non c'e' un provider), `diagnostics` porta il report row-scoped quando esiste | **A** |
| 10 | Nomi | `PlenoraSchemaError`, `PlenoraResourceLimitError`, … | identici carattere per carattere per le 18 categorie condivise | **C** |
| 10b | **Identita' delle classi** | `plenora_database.PlenoraError` e' una classe PyO3 di **quel** modulo | `plenora_data_tools.PlenoraError` sara' una classe PyO3 **distinta**: stesso nome, stessi attributi, **oggetto diverso**. Vedi §0.2 | **D** di fatto |

### CLI

| # | Aspetto | `plenora-database` | `plenora-data-tools` | |
|---|---|---|---|---|
| 11 | Envelope errori | JSON compatto su **stdout**, `{"status":"error","protocol_version":1,"error":{…}}`, `stderr` vuoto, exit code non-zero | **fatto**: allineato a stdout con stderr vuoto, stessa forma | **C** |
| 12 | Assi dell'errore | `category`, `phase`, `remote_effect`, `retry{kind[,delay_ms]}`, `provider`, `execution_id`, `message` | stessi campi; `provider` sempre `null` | **A** |
| 13 | Formato output | flag globale `--format json\|markdown\|junit`, default json, letto prima del dispatch | **fatto**: `--format json\|markdown`, stessa posizione e stesso default. `junit` solo se servira' un gate CI che lo consumi: un formato senza consumatore e' codice non provato | **A** |
| 14 | Ispezione dataset | `inspect-dataset FILE.arrow` | `describe --input FILE.arrow`, con **alias `inspect-dataset`** per continuita' di famiglia (§6, D6) | **A** |
| 15 | Successo | JSON su stdout | identico; `run` continua a stampare le metriche come oggetto JSON | **C** |
| 15b | **Exit code** | `ExitCode::FAILURE` — cioe' **1** — per qualunque errore; contratto documentato: «non-zero» | **NON allineato**: 2, 3, 4, 5, 6, 70 come proiezione della `category`, piu' 130 per la cancellazione (ADR-0003). Allinearsi vorrebbe dire collassare tutto su `1` e rinunciare al 130. Comune resta la sola garanzia debole «0 successo, non-zero errore»: codice portabile fra i due componenti deve leggere `error.category`, non il numero | **D** |

### Dati Arrow

| # | Aspetto | `plenora-database` | `plenora-data-tools` | |
|---|---|---|---|---|
| 16 | Unita' di scambio | **chunk Arrow IPC stream self-contained**: schema header + 1 record batch + EOS, come `bytes`. Lo schema si ripete in ogni chunk perche' i consumatori possano processarli indipendentemente | **obbligatorio e simmetrico**: l'SDK accetta questi chunk in ingresso e li produce in uscita nella stessa forma. E' il formato con cui `plenora_database` legge e scrive, quindi e' il formato con cui i due componenti si parlano | **C** |
| 17 | Passthrough | `bytes` accettati e verificati dal lato Rust | identico, e con una garanzia in piu': i byte passano dal confine `ipc_boundary` (pre-validazione del framing, barriera anti-panico, tetti sulle allocazioni). La validazione EOS stretta e' compatibile per costruzione con un chunk self-contained | **A** |
| 18 | Zero-copy | via `bytes` IPC | **in aggiunta**, Arrow C Data Interface (`arrow::ffi`) per `pyarrow.Table`/`RecordBatchReader` senza serializzare. E' un percorso **aggiuntivo**, non un sostituto: l'interoperabilita' fra componenti resta sui chunk IPC, che sono il contratto | **D** additiva |
| 19 | pandas | `_to_ipc_bytes` converte un `DataFrame` in silenzio via `pyarrow.Table.from_pandas` | **rifiutato**: vedi §0.1 | **D** |

### Packaging

| # | Aspetto | `plenora-database` | `plenora-data-tools` | |
|---|---|---|---|---|
| 20 | Canale | wheel allegati alla **GitHub Release** sempre; **PyPI opt-in** via `workflow_dispatch` con `publish_pypi=true` e `PYPI_TOKEN` | identico (decisione 4) | **C** |
| 21 | Piattaforme wheel | linux x86_64 `manylinux_2_34`, macOS **aarch64** (Intel dismesso), windows x86_64 | identiche (decisione 2) | **C** |
| 22 | Toolchain | `rust-toolchain: '1.92.0'` esplicita in `maturin-action` | identica | **C** |
| 23 | Smoke test | wheel Linux installato in venv pulita: `import`, `version()`, presenza delle classi pubbliche, `PlenoraError` | identico, piu' l'esempio E1 eseguito dalla venv | **A** |
| 24 | Backend nativi | nessuno | GEOS e PROJ. Il wheel e' **`full-backends` con link statico**: chi installa una wheel non puo' ricompilare per abilitare una feature, quindi la scelta va fatta al momento del packaging | **D** |

### 0.2 Handler portabile, non interoperabilita' runtime

Va detto senza ambiguita', perche' e' il genere di dettaglio che si scopre in
produzione: le classi di eccezione sono definite in **due moduli di estensione
diversi**. `plenora_database._native.PlenoraError` e
`plenora_data_tools._native.PlenoraError` avranno lo stesso nome e gli stessi
attributi, ma sono **oggetti Python distinti**, senza alcuna relazione di
ereditarieta'.

Conseguenza pratica:

```python
try:
    graph.execute(...)                      # plenora_data_tools
except plenora_database.PlenoraError:       # NON intercetta nulla
    ...
```

Cio' che il piano promette e' un **handler portabile**: lo stesso *codice* di
gestione funziona su entrambi, perche' i nomi e i sette attributi coincidono.

```python
# Portabile: si cattura la radice del proprio package...
except pdt.PlenoraError as e:
    log(e.category, e.phase, e.retry, e.remote_effect)

# ...oppure entrambe le radici, esplicitamente.
except (pdt.PlenoraError, pdb.PlenoraError) as e:
    ...
```

**Non** e' promessa l'interoperabilita' a runtime, e nessun documento di questo
progetto deve suggerirla. Se la si vuole davvero, va progettata: e' la
decisione **D9**.

### 0.1 La divergenza sul pandas, per esteso

`plenora_database._arrow_io._to_ipc_bytes` accetta un `pandas.DataFrame` e lo
converte con `pyarrow.Table.from_pandas(...)`. Per un componente che scrive su
database e' una comodita' difendibile: la destinazione ha un proprio schema,
che rifiuta cio' che non va bene.

Qui no, ed e' una **divergenza intenzionale safety-critical**:

- `from_pandas` prende decisioni **semantiche** — `NaN` che diventa null o
  resta `NaN`, timezone applicate o dimenticate, `object` inferito a stringa,
  decimali che passano per `float64`, indice conservato o scartato. Sono
  esattamente le decisioni che questo componente ha passato cinque giri di
  review a NON prendere in silenzio (ADR-0001, ADR-0010: nessuna correzione
  implicita dell'input, conversione esatta o errore);
- il contratto di un input non e' solo lo schema: e' anche CRS, encoding
  geometrico, dimensionalita', tipi dichiarati. Un `DataFrame` non li porta, e
  dedurli sarebbe inventarli;
- l'errore risultante sarebbe attribuito al motore, non alla conversione: un
  utente vedrebbe «tipi incompatibili» su dati che credeva corretti.

**Comportamento:** l'SDK alza `TypeError` con il rimedio esplicito
(`pyarrow.Table.from_pandas(df)`), che rende la conversione un gesto del
chiamante, visibile nel suo codice e sotto il suo controllo. Il messaggio
nomina la funzione da chiamare: rifiutare senza dire come procedere sarebbe
solo scortesia.

---

## Fase 1 — CLI: verifica e completamento

### 1.1 Cosa c'e' oggi

Nove sottocomandi: `catalog`, `capabilities`, `validate`, `run`, `transform`,
`spatial-join`, `transform-arrow`, `pair-arrow`, `self-test`. Dodici flag in
tutto. Errori come envelope JSON su stderr, output mai sovrascritti in
silenzio, metriche stampate su stdout a fine `run`.

### 1.2 Lacune concrete

| # | Lacuna | Perche' blocca l'uso |
|---|---|---|
| L1 | Nessun comando per **ispezionare un input**. | Per scrivere un piano servono nomi delle colonne, tipi, colonna geometrica, CRS, encoding. Oggi si scoprono solo facendo fallire un `run`. Il gemello ha `inspect-dataset` dal principio. |
| L2 | `run --inputs` mappa i percorsi **per posizione**. | Due file scambiati danno un errore di schema se va bene, un risultato sbagliato se gli schemi coincidono. **Era il difetto piu' grave della CLI.** |
| L3 | Nessuno **schema JSON del piano** pubblicato. | Il piano si scrive a mano contro la documentazione; nessun editor puo' completarlo o validarlo. |
| L4 | Nessun **esempio funzionante** nel repository. | Il primo `run` di chiunque parte da zero. |
| L5 | Nessun **exit code documentato**. | Uno script non puo' distinguere «piano invalido» da «limite superato» senza parsare l'envelope. |
| L6 | `--version` non ha forma **machine-readable**. | Un orchestratore non puo' verificare la versione senza parsare testo libero. |
| L7 | Nessun **completamento shell** ne' pagina di manuale. | Attrito quotidiano, non bloccante. |
| L8 | I **limiti** vivono solo nel piano. | Non si puo' rieseguire lo stesso piano con un tetto diverso senza modificarlo — cioe' senza cambiarne il `plan_hash`. |
| L9 | Envelope su **stderr**, non su stdout. | Il gemello mette gli errori su stdout e lascia stderr vuoto: due componenti della stessa famiglia non possono avere due convenzioni. |

### 1.3 Lavoro

- **1.3.1 `--input nome=percorso`** — **FATTO** (2026-08-16). *(chiude L2)*
  Forma nominale, ripetibile. La forma posizionale `--inputs` e' **rifiutata**
  per i piani DAG con piu' di un input dichiarato, con il rimedio nominale nel
  messaggio; resta accettata con un input solo, dove non c'e' niente da
  scambiare. Un avviso non sarebbe bastato: nei log di una pipeline non lo
  legge nessuno.
  *Accettazione:* `due_input_invertiti_non_raggiungono_mai_l_esecuzione`
  verifica che la forma posizionale con due input fallisca — invertita **e**
  nell'ordine giusto, perche' il difetto non e' l'ordine sbagliato ma il fatto
  che l'ordine non sia verificabile — che nessun output venga pubblicato, che
  `validate` si comporti allo stesso modo, e che con la forma nominale i due
  binding producano risultati DIVERSI (la prova che, prima, la forma
  posizionale poteva pubblicare in silenzio quello sbagliato).

- **1.3.2 `describe`** — **FATTO** (2026-08-16, formato JSON; `--format
  markdown` arrivera' con il flag globale di 1.3.8). *(chiude L1)* `plenora-data-tools describe --input
  INPUT.arrow [--format json|markdown]`, alias `inspect-dataset`. Apre
  l'input dal confine IPC, esegue la discovery del contratto e riporta: campi
  con tipo, nullability e `field_id`; colonna geometrica attiva; CRS e stato
  di risoluzione; dimensionalita'; encoding; tipi geometrici dichiarati;
  fingerprint del contratto.
  *Accettazione:* il fingerprint stampato coincide con quello che `run` usera'
  davvero, verificato da un test.

- **1.3.3 Esempio E1 e README** — **FATTO** (2026-08-16): esempio in
  `examples/e1-filtro-ordinamento`, rieseguito da
  `crates/plenora-cli/tests/examples_e2e.rs`; README riscritto attorno a
  quei tre comandi. *(chiude L4)*

- **1.3.4 `plan-schema`** *(chiude L3)* — emette lo JSON Schema del piano,
  **generato** dalle stesse strutture che `PlanV4::parse` valida: uno schema
  scritto a mano che diverge dal parser e' peggio di nessuno schema.

- **1.3.5 Exit code stabili** — **FATTO** (2026-08-16), come convenzione **di
  questo componente**: non e' un allineamento al gemello (§0 riga 15b, e
  l'emendamento a ADR-0003). *(chiude L5)*
  `0` successo, `2` piano o configurazione invalidi, `3` contratto/schema/
  capability, `4` limite di risorsa, `5` I/O, pubblicazione, rete o
  autorizzazioni, **`6` fallimento di esecuzione di un nodo** — aggiunto
  rispetto alla bozza, perche' mappare `execution` su «difetto interno»
  sarebbe stato falso — `70` difetto interno, `130` cancellazione. Derivati
  dalla `category` dell'envelope, che resta la fonte di verita'.

- **1.3.6 `--version --json`** e `capabilities` — **FATTO** (2026-08-16):
  versione del componente, versione Arrow, backend **effettivamente
  compilati** (da `cfg!`, non da una lista scritta a mano) e numero di
  operazioni. Il fingerprint del catalogo resta fuori: e' privato in
  `planner` e non si tocca il core per esporlo. *(chiude L6)*

- **1.3.7 `--limits FILE.json`** su `validate` e `run`: sovrascrive i limiti
  del piano **dichiarandolo** nelle metriche (`limits_source: "override"`).
  Il `plan_hash` non cambia — i limiti non sono semantica — ma il report deve
  dire quali erano in vigore *(chiude L8)*.

- **1.3.8 Envelope su stdout** — **FATTO** (2026-08-16). *(chiude L9)*
  Errori su stdout, stderr vuoto, stessa forma del gemello; rottura
  registrata nella nota breaking. Inverte esplicitamente la review P1-5 del
  2026-08-03, che aveva riportato l'envelope su stderr: quella decisione
  precede l'esistenza del gemello, e la motivazione e' scritta accanto al
  codice che emette. Insieme, il flag globale `--format json|markdown` letto
  prima del dispatch, con `markdown` reso per `describe`, `catalog` e
  `capabilities` e **rifiutato** dove non esiste una resa leggibile.

- **1.3.9 Completamenti shell e man page**, generati *(chiude L7)*. Ultimo.

### 1.4 Verifica della CLI esistente — **FATTA** (2026-08-16)

La matrice vive in `crates/plenora-cli/tests/matrice_cli.rs`: sei dimensioni
applicate a TUTTI i sottocomandi, alias `inspect-dataset` compreso, invece che
a quelli che capita di ricordare. Undici test.

**Difetti trovati e corretti** (tre, tutti della stessa famiglia: eseguire cio'
che non si e' capito):

1. **flag sconosciuti ignorati** — `catalog --sconosciuto`, `capabilities
   --pippo`, `self-test --qualunque` uscivano con successo. Ora ogni comando
   dichiara la propria superficie di flag (`superficie()`, unica fonte) e
   rifiuta il resto elencando gli ammessi;
2. **flag a valore singolo ripetuti in silenzio** — `describe --input a
   --input b` descriveva `a`. Ora e' un errore; `--input` resta ripetibile su
   `run`/`validate`, dove ogni occorrenza e' un input diverso;
3. **due canali per un errore solo** — un comando inesistente stampava l'help
   su stderr oltre all'envelope su stdout. Ora l'envelope e' l'unico
   documento e indica `--help`.

**Verificato senza trovare difetti**: argomenti mancanti (ogni comando nudo
fallisce con `invalid_plan`/2, e il messaggio nomina il flag), valore mancante
dopo un flag, file inesistenti e illeggibili (una directory al posto di un
file: `io`/5 su ogni comando che legge), forme incompatibili (nominale +
posizionale, `--right` su un piano DAG), nessun output pubblicato su nessun
fallimento di `run`, un solo documento JSON su stdout sia in successo sia in
errore, stderr vuoto in entrambi i casi, e parita' fra help e dispatch — con
un test che estrae i flag accettati dal messaggio d'errore del comando e
verifica che l'help li documenti tutti.

**Osservato e non cambiato**: `run` scrive un avviso su stderr quando la
durabilita' del publish non e' confermabile (fsync della directory non
supportato). E' l'unico caso in cui stderr non e' vuoto, riguarda un
successo e non un errore, e il canale e' congelato: resta com'e', dichiarato
qui.

Restano fuori dalla matrice, come lavoro successivo:
- i quattro comandi legacy (`transform`, `spatial-join`, `transform-arrow`,
  `pair-arrow`) **restano disponibili e vengono deprecati** a favore di `run`
  con un piano (decisione 1): avviso nell'help e nel changelog, rimozione non
  prima della major successiva.

Nel piano di esecuzione: `1.4` e' chiusa.

---

## Fase 2 — SDK Python

### 2.1 Strada scelta

**Estensione nativa PyO3 + maturin**, identica per struttura a
`plenora-database-py`. Non e' una scelta da rifare: e' quella gia' in esercizio
nella famiglia, e un wrapper sul processo CLI costringerebbe a passare per file
temporanei proprio dove i due componenti devono scambiarsi Arrow.

```text
crates/plenora-data-tools-py/
  Cargo.toml            # versione PROPRIA, cdylib, pyo3 abi3-py310
  pyproject.toml        # maturin, python-source = "python",
                        # module-name = "plenora_data_tools._native"
  python/plenora_data_tools/
    __init__.py  __init__.pyi
    _native.pyi         # stub del modulo nativo (non e' superficie pubblica)
    errors.py           # ri-esporta le 18 Plenora*Error da _native
    plan.py   plan.pyi
    contract.py contract.pyi
    _execution.py _execution.pyi
    _arrow_io.py        # chunk IPC self-contained, in ingresso e in uscita
    py.typed
  python/tests/         # pytest, rispecchia gli esempi end-to-end
  src/                  # lib.rs, errors.rs, plan.rs, contract.rs, execute.rs
```

### 2.2 Superficie proposta

```python
import pyarrow as pa
import plenora_data_tools as pdt

# 1. Cosa contiene questo input.
contract = pdt.describe(table)              # -> Contract (sola lettura)
print(contract.geometry.crs, contract.geometry.encoding)

# 2. Il piano si valida da solo, prima di qualunque dato.
plan = pdt.Plan.from_json(path_or_str)      # -> PlenoraInvalidPlanError se invalido
graph = plan.validate({"main": contract})   # -> ValidatedGraph

# 3. Esecuzione. Gli input PORTANO il contratto: non c'e' altra firma.
result = graph.execute({"main": (table, contract)})
result.table                                # pyarrow.Table
result.metrics                              # dict, include counters_saturated
result.diagnostics                          # report row-scoped, se richiesto

# 4. Interoperabilita' con plenora_database: chunk IPC self-contained.
with pdb.connect(dsn) as s:
    for chunk in s.read("public", "citta"):        # bytes, schema+batch+EOS
        out = graph.execute_chunk({"main": chunk}) # bytes, stessa forma
        s.copy_from("public", "citta_out", out)
```

Regole non negoziabili:

1. **Nessun percorso senza contratto.** `execute` accetta `(dati, contratto)`.
   Non esiste un overload che deduce il contratto in silenzio: chi vuole
   quello dedotto chiama `describe` e lo passa — un gesto esplicito, visibile
   nel suo codice.
2. **Chunk IPC self-contained come formato di scambio** (§0 #16), accettati e
   prodotti: `execute_chunk(bytes) -> bytes`. E' il modo in cui `plenora_
   database` legge e scrive, quindi e' il modo in cui i due si parlano.
3. **C Data Interface in aggiunta** (§0 #18) per `pyarrow.Table` e
   `RecordBatchReader`: piu' veloce quando i dati sono gia' in processo, mai
   al posto dei chunk quando i dati attraversano un confine di componente.
4. **Nessuna conversione implicita da pandas** (§0.1): `TypeError` con il
   rimedio nel messaggio.
5. **Errori tipizzati** con i nomi e i sette attributi del gemello (§0 #9),
   che rendono portabile il CODICE di gestione ma non le classi: un
   `except plenora_database.PlenoraError` non intercetta un errore di questo
   package (§0.2).
6. **GIL rilasciato** durante validazione ed esecuzione; `KeyboardInterrupt`
   collegato al `CancellationToken` gia' esistente.
7. **Niente stato globale implicito**: `max_parallelism` e' di processo
   (DER-006) e la docstring lo dice.

### 2.3 Lavoro

1. crate e scheletro maturin, `version()` e test di parita' delle versioni;
2. mappatura degli errori (18 classi, sette attributi), un test per classe;
3. `_arrow_io`: chunk IPC in ingresso e in uscita, con test di round-trip
   contro chunk realmente prodotti da `plenora_database`;
4. C Data Interface, con test di identita' byte-per-byte su ogni tipo del
   catalogo;
5. `describe` / `Plan` / `ValidatedGraph` / `execute` / `execute_chunk`;
6. GIL e cancellazione;
7. `pytest` che rispecchia gli esempi end-to-end della Fase 4, piu'
   `mypy --strict`.

---

## Fase 3 — Packaging e distribuzione

Convenzioni identiche a `plenora-database` (§0 #20–#24).

- **CLI**: artefatti per `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
  `aarch64-apple-darwin` (decisione 2). Due profili per piattaforma — senza
  backend e `full-backends` — con la differenza nel nome dell'artefatto, non
  in una nota. Checksum SHA-256, allegati alla release, referenziati dal
  manifesto `release/<versione>.json`.
- **Wheel**: workflow `python-wheel.yml` modellato su quello del gemello —
  `maturin-action`, `manylinux_2_34`, macOS aarch64, windows x86_64,
  `rust-toolchain: '1.92.0'`, smoke test in venv pulita, allegato alla
  release, PyPI opt-in (decisione 4). Il wheel e' **`full-backends` statico**
  (§0 #24).
- **macOS entra nella matrice CI** dei test, non solo del packaging: una
  piattaforma di rilascio non verificata non e' una piattaforma supportata.
- **Gate di rilascio**: agli attuali si aggiungono build degli artefatti su
  tutte le piattaforme dichiarate, `pytest` della wheel, e l'esecuzione
  dell'esempio E1 da una macchina senza toolchain Rust.

---

## Fase 4 — API pubblica minima ed esempi

### 4.1 Superficie Rust minima

`plenora-engine` espone una **facciata** — `Plan`, `ValidatedGraph`, `Inputs`
(solo strict), `Output`, `ExecutionMetrics`, `RuntimeContext`, `Limits`,
`DataContract`, `PlenoraError` — e il resto passa sotto `#[doc(hidden)]` o in
un modulo `internals` esplicitamente fuori semver. Non e' una rimozione: e'
dichiarare cosa promettiamo di non rompere.

*Accettazione:* un test `cargo public-api` con baseline committata, che
fallisce quando la superficie cambia senza aggiornare la baseline — lo stesso
principio dello snapshot del catalogo gia' in uso.

### 4.2 Esempi end-to-end

Sotto `examples/`, ciascuno con dati committati (piccoli), piano, comando,
output atteso e un test che li riesegue confrontando byte per byte:

| # | Esempio | Copre |
|---|---|---|
| E1 | Filtro + ordinamento su tabella — **fatto** | il giro minimo: `describe` → `validate` → `run` → metriche |
| E2 | Join di due input | forma nominale `--input`, contratti multipli |
| E3 | Aggregazione con spill | limiti, `SpillMetrics`, metriche saturate |
| E4 | Geometria senza backend: filtro per bounding box | contratto geometrico, CRS dichiarato |
| E5 | Geometria con GEOS: intersezione | profilo `full-backends`, capability |
| E6 | Diagnostica row-scoped su input parzialmente invalido | fail-closed e report |
| E7 | E1 dallo SDK Python | parita' CLI/SDK |
| E8 | `plenora_database` → data-tools → `plenora_database` | chunk IPC self-contained fra componenti |

*Accettazione:* `examples/` gira in CI; un esempio che non riproduce l'output
atteso rompe la build. Sono anche il banco di prova della documentazione: se un
esempio ha bisogno di una spiegazione che non sta nel README, manca la
documentazione, non il commento.

### 4.3 Documentazione

- `README` riscritto attorno a E1 (oggi spiega l'architettura prima dell'uso);
- `docs/guida-piani.md`: anatomia di un piano, operazioni per famiglia;
- `docs/contratti.md`: cos'e' un contratto, come si legge da un input, cosa
  rende due contratti incompatibili — il concetto che tutto il resto
  presuppone e che oggi non e' spiegato in un posto solo;
- `CHANGELOG.md` dell'SDK Python, come nel gemello;
- `cargo doc` pubblicato per la facciata.

---

## Ordine di esecuzione

```text
1.3.1 --input nome=percorso   FATTO
1.3.2 describe                FATTO
4.2   esempio E1              FATTO
4.3   README riscritto        FATTO
1.3.5 exit code               FATTO
1.3.6 --version --json        FATTO
1.3.8 envelope + --format     FATTO
1.4   verifica CLI completa   FATTO (matrice a sei dimensioni, 3 difetti chiusi)
      ────────────────────────────────
1.3.4 plan-schema, 1.3.7 completamenti  ┐
4.1   facciata pubblica      ├─> Fase 2 SDK ─> Fase 3 packaging ─> release
1.4   verifica CLI           ┘
```

---

## Decisioni

Prese dal maintainer il 2026-08-16:

1. **Comandi legacy** (`transform`, `spatial-join`, `transform-arrow`,
   `pair-arrow`): restano disponibili, **deprecati** a favore di `run` con un
   piano.
2. **macOS** entra nel target, come per `plenora-database`: aarch64, sia nel
   packaging sia nella matrice CI.
3. **Python minimo 3.10**, `abi3-py310`.
4. **Distribuzione**: stesso canale del gemello — wheel allegati alla GitHub
   Release, PyPI opt-in.
5. **`Inputs::add`** resta per almeno una release pubblicata e si rimuove solo
   nella major successiva.

Ancora aperte:

- **D6 — Nome del comando di ispezione.** Il piano adotta `describe` come
  canonico con alias `inspect-dataset`. Se la famiglia deve avere un nome solo,
  la scelta e' `inspect-dataset` (gia' in esercizio) e `describe` diventa
  l'alias. Decisione a costo quasi nullo finche' il comando non e' rilasciato.
- **D7 — `--format junit`.** Il gemello lo ha; qui non c'e' ancora un gate CI
  che lo consumi. Si aggiunge quando esiste il consumatore.
- **D9 — Base comune delle eccezioni.** Oggi ogni SDK definisce le proprie
  classi PyO3: nomi e attributi compatibili, identita' Python diverse (§0.2).
  Per avere un `except` unico servirebbe un pacchetto Python di base — per
  esempio `plenora-errors`, puro Python, che definisce `PlenoraError` e le
  sottoclassi — da cui entrambi gli SDK derivano le proprie. Costo: una
  dipendenza in piu' per entrambi, un ciclo di versione da coordinare, e una
  modifica a `plenora-database` gia' pubblicato (le sue classi cambierebbero
  base). Beneficio: un chiamante che orchestra i due componenti scrive un
  handler solo. E' una decisione di famiglia, non di questo repository: finche'
  non e' presa, il piano promette **solo** l'handler portabile.
- **D8 — Wheel con backend geografici.** Il piano assume un solo wheel
  `full-backends` statico. Se la dimensione risultante fosse un problema,
  l'alternativa e' pubblicare due distribuzioni con nomi diversi — non due
  varianti dello stesso nome, che renderebbero l'installato ambiguo.
