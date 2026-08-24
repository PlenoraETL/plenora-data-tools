# plenora-data-tools

Motore Rust Arrow-in / Arrow-out per pipeline dichiarative di trasformazione
tabellare e geografica. Valida integralmente il piano e i contratti **prima**
dell'esecuzione, applica limiti di risorsa e pubblica gli output in modo
atomico.

> Versione workspace: `1.0.3`. Il progetto **non è ancora rilasciabile in
> produzione**: la ragione, e che cosa manca, sono in
> [`docs/stato-e-roadmap.md`](docs/stato-e-roadmap.md).

## Installazione

Toolchain Rust **1.98.0**, pinnata da `rust-toolchain.toml`.

```sh
cargo build --release --locked
# il binario è in target/release/plenora-data-tools
```

Con i backend geografici nativi (GEOS statico, PROJ bundled; richiede `cmake`,
`sqlite3`, `libsqlite3-dev`):

```sh
cargo build --release --locked --features full-backends
```

Senza quelle feature il comportamento è fail-closed e dichiarato: un piano che
richiede la riproiezione fallisce in validazione invece di produrre un
risultato approssimato. `plenora-data-tools capabilities` riporta che cosa la
build offre davvero.

## Il primo piano

Un piano è un documento JSON dichiarativo: dice **che cosa** produrre, non
come.

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

`citta` è il nome dell'input **dichiarato dal piano**, non un'etichetta
libera: lega un percorso a quell'input, e un nome non dichiarato è un errore.

La versione canonica è la **5**. Un piano `schema_version: 4` continua a
funzionare — viene migrato prima della validazione — e il formato lineare
`schema_version <= 3` è invariato. Il dettaglio è in
[`docs/piano-v5.md`](docs/piano-v5.md).

## Il giro completo

```sh
# 1. che cosa contiene l'input: campi, tipi, colonna geometrica, CRS, fingerprint.
#    Legge il solo header Arrow, non i dati.
plenora-data-tools describe --input citta.arrow

# 2. il piano regge contro quel contratto?  Non esegue nulla.
plenora-data-tools validate --plan piano.json --input citta=citta.arrow

# 3. esecuzione: output pubblicato atomicamente, metriche JSON su stdout.
plenora-data-tools run --plan piano.json --input citta=citta.arrow --output output.arrow
```

`validate` risponde **prima** di leggere i dati: struttura del piano,
contratti degli input, CRS, capability della build, limiti effettivi. Un piano
semanticamente valido ma fuori dal dispatch corrente fallisce qui, non a metà
esecuzione.

L'esempio completo ed eseguibile è in
[`examples/e1-filtro-ordinamento/`](examples/e1-filtro-ordinamento/README.md),
rieseguito dalla suite a ogni CI.

## Errori

Ogni errore esce come **envelope JSON su stdout**, una riga. **stderr resta
vuoto**, anche sui percorsi d'errore: è una garanzia verificata da test, non
una convenzione.

```json
{"status":"error","protocol_version":1,
 "error":{"category":"invalid_plan","phase":"validate","remote_effect":"none",
          "retry":{"kind":"never"},"message":"...",
          "context":{"node":"grandi","operation":"table.filter","execution_id":"..."}}}
```

Quattro assi espliciti — categoria, fase, effetto remoto, disposizione di
retry — mai dedotti dal testo del messaggio. L'exit code è la proiezione della
categoria (`2` piano invalido, `3` schema o dati, `4` limite di risorsa, `5`
I/O e affini, `6` esecuzione, `70` interno, `130` cancellato).

**Nessun dato nei messaggi d'errore**: mai valori di cella, mai payload. Un
errore porta ciò che serve a diagnosticare, non ciò che serve a ricostruire i
dati.

## Documentazione

| documento | contenuto |
|---|---|
| [`docs/architettura.md`](docs/architettura.md) | crate, flusso planner/executor/kernel, determinismo, memoria, backend |
| [`docs/piano-v5.md`](docs/piano-v5.md) | schema canonico, contratti, identità, migrazione dalla v4 |
| [`docs/cli.md`](docs/cli.md) | comandi, binding degli input, formati, canali, exit code |
| [`docs/operazioni.md`](docs/operazioni.md) | riferimento completo delle 146 operazioni, generato dal codice |
| [`docs/errori-e-limiti.md`](docs/errori-e-limiti.md) | tassonomia, privacy, cancellazione, panic policy, **e i limiti non coperti** |
| [`docs/stato-e-roadmap.md`](docs/stato-e-roadmap.md) | che cosa manca, in ordine |
| [`docs/release.md`](docs/release.md) | gate, piattaforme, packaging, procedura |
| [`AGENTS.md`](AGENTS.md) | regole di lavoro sul repository |

Se legge un solo documento oltre a questo, legga
[`docs/errori-e-limiti.md`](docs/errori-e-limiti.md): la seconda metà dice
dove le garanzie si fermano, ed è l'informazione che serve prima di mettere il
motore su dati che contano.
