# plenora-data-tools

Motore Rust Arrow-in/Arrow-out per pipeline dichiarative di trasformazione
tabellare e geografica. Valida integralmente il piano e i contratti prima
dell'esecuzione, applica limiti di risorsa e pubblica gli output in modo
atomico.

> Versione workspace: `1.0.3`.
> Versione, evidenze e stato di pubblicazione sono registrati nei manifesti
> sotto `release/` e nelle release GitHub; questo README non sostituisce i gate.

## In tre comandi

Il giro completo su un input Arrow (`examples/e1-filtro-ordinamento`, che gira
in CI a ogni build):

```sh
# 1. Cosa contiene l'input: campi, tipi, colonna geometrica, CRS, fingerprint.
plenora-data-tools describe --input citta.arrow

# 2. Il piano si valida contro il contratto dell'input, senza leggere i dati.
plenora-data-tools validate --plan piano.json --input citta=citta.arrow

# 3. Esecuzione: output pubblicato atomicamente, metriche JSON su stdout.
plenora-data-tools run --plan piano.json --input citta=citta.arrow --output output.arrow
```

Il piano e' un documento dichiarativo: dice **cosa** produrre, non come.

```json
{
  "schema_version": 5,
  "inputs": ["citta"],
  "nodes": [
    {"id": "grandi",   "op": "table.filter", "in": ["citta"],
     "config": {"column": "abitanti", "operator": ">", "value": 300000}},
    {"id": "ordinate", "op": "table.sort",   "in": ["grandi"],
     "config": {"columns": ["abitanti"], "ascending": false}}
  ],
  "output": "ordinate"
}
```

`citta=` e' il nome dell'input **dichiarato dal piano**, non un'etichetta
libera: lega quel percorso a quell'input, e un nome non dichiarato e' un
errore. La forma `--inputs a.arrow b.arrow`, che accoppia per posizione, e'
**rifiutata** quando il piano dichiara piu' di un input: due file scambiati con
lo stesso schema darebbero un risultato sbagliato invece di un errore, e
l'ordine non e' verificabile da nessuno. Resta accettata con un input solo,
dove non c'e' niente da scambiare.

### Cosa succede se qualcosa non torna

Gli errori sono envelope JSON con quattro assi — categoria, fase, effetto
remoto, disposizione di retry — emessi su **stdout**, con stderr lasciato
vuoto e un exit code non zero. E' la convenzione di `plenora-database-tools`:
chi orchestra i due componenti cerca gli errori in un posto solo.

```json
{"status":"error","protocol_version":1,
 "error":{"category":"schema","phase":"validate","remote_effect":"none",
          "retry":{"kind":"never"},"message":"colonna `abitanti` assente"}}
```

Nessun output esistente viene sovrascritto, e nessun input viene corretto in
silenzio: un valore che non e' rappresentabile e' un errore, non un
arrotondamento.

## Esempi

| | |
|---|---|
| [`examples/e1-filtro-ordinamento`](examples/e1-filtro-ordinamento) | il giro minimo: describe, validate, run |

Ogni esempio porta i propri dati e il proprio output atteso, ed e' rieseguito
dalla suite: un esempio che non riproduce il proprio risultato rompe la build.

## Ruolo nell'ecosistema Plenora

```text
plenora-IO-tools       file e formati  <-> Arrow
plenora-data-tools     Arrow           <-> Arrow
plenora-database-tools database        <-> Arrow
```

I tre componenti comunicano tramite schema Arrow e metadata canonici definiti
da `plenora-contracts`. `plenora-data-tools` non legge direttamente CSV, XLSX,
SHP, GeoPackage o database: riceve `RecordBatch`/Arrow IPC prodotti dai
componenti di bordo, preserva il contratto e restituisce Arrow.

## Requisiti

- Rust `1.92` (fissato in `rust-toolchain.toml`);
- dipendenze bloccate da `Cargo.lock`;
- CMake, Clang e SQLite per il backend PROJ bundled;
- toolchain native GEOS/PROJ solo quando richieste dalle relative feature.

Il workspace usa Arrow `59.1.0` con pin esatti.

## Build e test

```sh
cargo build --workspace --locked
cargo test --workspace --no-fail-fast --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Esecuzione riproducibile in container:

```sh
docker run --rm -v "$PWD:/work" -w /work rust:1.92 \
  cargo test --workspace --no-fail-fast --locked
```

Il gate safety-critical completo, inclusi i lint anti-panic sulle librerie, è
documentato in [`AGENTS.md`](AGENTS.md).

## Feature

La CLI non abilita backend geografici per impostazione predefinita:

- `geos-backend`: operazioni che richiedono GEOS;
- `proj-backend`: risoluzione e riproiezione CRS tramite PROJ;
- `full-backends`: abilita entrambi.

Esempio:

```sh
cargo build -p plenora-cli --features full-backends --locked
```

## CLI — riferimento

```sh
plenora-data-tools --help
plenora-data-tools catalog [--family table|geo]      # le operazioni disponibili
plenora-data-tools capabilities                      # backend compilati
plenora-data-tools describe --input INPUT.arrow      # contratto di un input
plenora-data-tools validate --plan PLAN.json --input NOME=INPUT.arrow...
plenora-data-tools run --plan PLAN.json --input NOME=INPUT.arrow... --output OUTPUT.arrow
```

`describe` ha l'alias `inspect-dataset`, il nome che lo stesso comando ha in
`plenora-database-tools`.

`validate` non esegue il piano. `run` accetta piani legacy fino alla versione 3
(`--input PERCORSO`, senza nome) e piani DAG (`--input NOME=PERCORSO`). Per i
piani DAG con piu' di un input la forma nominale e' l'unica ammessa.

Il formato DAG corrente e' la **v5** (`"schema_version": 5`). Un piano
`schema_version: 4` continua a funzionare: viene migrato al canonico prima
della validazione, e i comandi riportano la versione sotto cui il piano viene
davvero eseguito. La v4 e la v5 differiscono per un campo — il budget di
memoria, che si chiama `max_governed_memory_bytes` — e non c'e' alias fra i
due nomi (ADR 15).

I comandi `transform`, `spatial-join`, `transform-arrow` e `pair-arrow` restano
disponibili ma sono **deprecati**: la stessa cosa si esprime come piano e si
esegue con `run`.

### Convenzioni di output

- **Errori**: envelope JSON su stdout (`{"status":"error","protocol_version":1,
  "error":{…}}`), stderr vuoto, exit code non zero.
- **Formato**: flag globale `--format json|markdown`, default `json`, valido
  prima o dopo il sottocomando. `markdown` e' disponibile dove esiste una resa
  leggibile (`describe`, `catalog`, `capabilities`); altrove il flag e'
  **rifiutato**, non ignorato.
- **Exit code**: convenzione **di questo componente**, non della famiglia —
  `plenora-database-tools` usa `1` per qualunque errore. L'unica garanzia
  condivisa e' «0 successo, non-zero errore»; codice portabile fra i due deve
  leggere `error.category` dall'envelope, che resta la fonte di verita'.
  Qui il codice ne e' una proiezione:

  | codice | significato |
  |---|---|
  | 0 | successo |
  | 2 | piano o configurazione invalidi |
  | 3 | contratto, schema o capability incompatibili |
  | 4 | limite di risorsa superato |
  | 5 | I/O, pubblicazione, rete o autorizzazioni |
  | 6 | fallimento di esecuzione di un nodo |
  | 70 | difetto interno |
  | 130 | cancellato (128 + SIGINT) |

- **`--version`** e `capabilities` emettono JSON con versione del componente,
  versione Arrow, backend compilati e numero di operazioni a catalogo.
- **Argomenti**: ogni sottocomando accetta solo i flag che dichiara. Un flag
  sconosciuto, o un flag a valore singolo ripetuto, e' un errore — non viene
  ignorato. `--input` si ripete solo su `run` e `validate`, dove ogni
  occorrenza e' un input nominato diverso.
- Gli output esistenti non vengono sovrascritti silenziosamente.

## Contratti e compatibilità

- Nessun CRS predefinito e nessuna riproiezione implicita.
- Le dichiarazioni CRS incompatibili non vengono conciliate silenziosamente.
- Schema, metadata, nullability e dimensionalità fanno parte del contratto.
- Arrow è parte della superficie pubblica Rust: un aggiornamento maggiore o
  incompatibile è trattato come cambiamento potenzialmente breaking.
- Le baseline normative e Git esatte appartengono ai documenti di release, non
  a riferimenti mobili in questo file.

Per il modello completo vedere:

- [`Architetture.md`](Architetture.md);
- [`Prestazioni.md`](Prestazioni.md);
- [`docs/adr/`](docs/adr/);
- [`docs/deroghe.md`](docs/deroghe.md).

## Release

Una release stabile richiede sulla stessa revisione immutabile:

1. test default e all-features;
2. Clippy, safety lint e build sulle piattaforme dichiarate;
3. conformance e roundtrip con `plenora-IO-tools` e
   `plenora-database-tools`;
4. aggiornamento del manifesto `release/` e della baseline normativa;
5. revisione indipendente prima del tag.

Il bump di versione, il tag e la pubblicazione non sono impliciti nel semplice
superamento della suite locale.
