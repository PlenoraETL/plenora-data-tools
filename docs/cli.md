# Interfaccia a riga di comando

`plenora-data-tools` è l'unico eseguibile distribuito.

## Comandi

```
plenora-data-tools catalog [--family table|geo]
plenora-data-tools describe --input INPUT.arrow                          (alias: inspect-dataset)
plenora-data-tools validate --plan PLAN.json --input NOME=INPUT.arrow...
plenora-data-tools run --plan PLAN.json --input NOME=INPUT.arrow... --output OUTPUT.arrow [--no-geo-fusion]
plenora-data-tools capabilities
plenora-data-tools self-test [--output RESULT.bin]
plenora-data-tools --version
```

| comando | che cosa fa |
|---|---|
| `catalog` | elenca le operazioni del catalogo unificato, con le versioni per componente |
| `describe` | legge il solo header Arrow IPC e riporta schema, geometrie e `contract_fingerprint`. Non legge i dati |
| `validate` | fase 1: struttura, contratti, CRS, capability, limiti, strategia fisica. **Non esegue il piano** |
| `run` | esegue il piano e pubblica l'output; metriche per nodo e per segmento su stdout |
| `capabilities` | backend compilati e profili di publish supportati in questa build |
| `self-test` | integrità del catalogo; con `--output`, scrive un frame di controllo geo |

`describe` ha l'alias `inspect-dataset`, il nome che lo stesso comando ha in
`plenora-database-tools`.

### Comandi deprecati

`transform`, `spatial-join`, `transform-arrow`, `pair-arrow` restano
disponibili ma sono **deprecati**: la stessa cosa si esprime come piano e si
esegue con `run`.

## Binding degli input

Due forme, e la scelta non è stilistica.

**Nominale** — `--input NOME=PERCORSO`, ripetibile. `NOME` è il nome
dichiarato nel blocco `inputs` del piano, non un'etichetta libera: lega quel
percorso a quell'input. Un nome non dichiarato è un errore.

```bash
plenora-data-tools run --plan piano.json \
  --input citta=citta.arrow --input fiumi=fiumi.arrow \
  --output uscita.arrow
```

**Posizionale** — `--inputs PERCORSO...`, legati agli input dichiarati **in
ordine di dichiarazione**. Un conteggio diverso è un errore.

Per un piano DAG con **più di un input** la forma nominale è l'unica ammessa:
il posizionale invertirebbe due input senza che nulla lo dica, e
quell'ambiguità non raggiunge mai l'esecuzione.

**Piani lineari** (`schema_version <= 3`): `--input PERCORSO` singolo, senza
nome, più `--right` per il secondo lato dei piani binari. La forma
`NOME=PERCORSO` non si applica — un piano lineare non ha input nominati — ed
è rifiutata con un messaggio che lo dice. Simmetricamente, `--right` non è
ammesso per i piani DAG.

## Formati di uscita

Flag globale `--format json|markdown`, default `json`, valido **prima o dopo**
il sottocomando.

`markdown` è disponibile dove esiste una resa leggibile — `describe`,
`catalog`, `capabilities` — e altrove è rifiutato invece di essere ignorato in
silenzio.

## stdout e stderr

**Tutto l'output strutturato va su stdout. stderr resta vuoto**, anche sui
percorsi d'errore. È una garanzia verificata da test golden, non una
convenzione: chi compone la CLI in una pipeline legge stdout e basta.

- **successo**: riepilogo JSON su stdout (schema, nodi, archi, ordine
  topologico, segmenti, `plan_hash` per `validate`; metriche per nodo e per
  segmento per `run`);
- **errore**: envelope JSON su stdout, una riga, exit code non zero. Forma e
  campi in [`errori-e-limiti.md`](errori-e-limiti.md).

Su `run`, `"schema_version"` nel riepilogo è **5** anche per un piano v4 in
ingresso: è la versione sotto cui il piano viene effettivamente eseguito.

## Exit code

Proiezione della categoria dell'errore.

| codice | categorie |
|---|---|
| `0` | successo |
| `2` | `invalid_plan`, `invalid_configuration` |
| `3` | `schema`, `data_mapping`, `crs`, `unsupported` |
| `4` | `resource_limit` |
| `5` | `io`, `not_found`, `conflict`, `protocol`, `authentication`, `authorization`, `timeout`, `transient`, `isolation_unavailable`, `unattributed_memory_pressure` |
| `6` | `execution` |
| `70` | `internal`, e ogni categoria non riconosciuta |
| `130` | `cancelled` (128 + SIGINT) |

Sono una **convenzione di questo componente**, dichiarata come divergenza
rispetto ai componenti gemelli, non un allineamento.

Il numero è **grossolano di proposito**: distingue l'azione da intraprendere,
non la condizione. La condizione precisa sta in `error.category`
dell'envelope, che è machine-readable — chi ne ha bisogno legge quella. Il `5`
raccoglie perciò casi molto diversi (un disco assente, una destinazione
occupata, un ambiente senza isolamento) accomunati solo dal fatto che il
piano è corretto e sono le condizioni operative a non esserlo.

Cosa impedisce alla tabella di divergere dal codice, e cosa no:

- una categoria nuova **non compila** finché non le si assegna un exit code:
  la proiezione è un `match` esaustivo su `ErrorCategory`, non un confronto
  di stringhe;
- un test itera *tutte* le categorie dichiarate e pretende che ciascuna
  compaia nella propria tabella, scritta a mano come seconda opinione: una
  categoria nuova non può restare scoperta in silenzio;
- **questa tabella no**: è prosa, nessun controllo automatico la legge. È
  già divergita una volta — `protocol` era proiettato su `5` dal codice e
  mancava da questa riga. Chi tocca la proiezione aggiorni anche qui.

## Cancellazione

`run` installa un handler Ctrl-C: al primo segnale l'esecuzione è cancellata
cooperativamente, **nessun output è pubblicato**, il messaggio è pulito e
l'exit code è `130`. Un secondo Ctrl-C forza l'uscita immediata.

## Publish dell'output

Sempre atomico: tempfile nella directory di destinazione, poi rename
**no-clobber**. `run` non sovrascrive un output esistente — fallisce con
`conflict`, exit `5`.

## Un giro completo

```bash
# 1. che cosa c'è nell'input
plenora-data-tools describe --input citta.arrow

# 2. il piano regge contro quel contratto?  (non esegue)
plenora-data-tools validate --plan piano.json --inputs citta.arrow

# 3. esegui
plenora-data-tools run --plan piano.json --inputs citta.arrow --output uscita.arrow
```

L'esempio eseguibile completo è in
[`examples/e1-filtro-ordinamento/`](../examples/e1-filtro-ordinamento/README.md),
ed è rieseguito dalla suite a ogni CI.
