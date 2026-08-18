# E1 — Filtro e ordinamento

Il giro minimo: guardare un input, validare un piano contro di esso, eseguirlo.
Nessun backend geografico richiesto.

## File

| | |
|---|---|
| `dati/citta.json` | le righe di partenza, in chiaro perche' siano leggibili in una revisione |
| `piano.json` | il piano: filtra i comuni sopra 300 000 abitanti, poi ordina per abitanti decrescenti |
| `atteso/output.json` | il risultato esatto |

L'input Arrow si materializza dal JSON: questo componente legge Arrow, non
JSON — i formati di bordo sono compito di `plenora-IO-tools`. Il test
`crates/plenora-cli/tests/examples_e2e.rs` lo fa e riesegue l'intero esempio a
ogni build.

## Comandi

Se hai gia' `citta.arrow` (per esempio prodotto da `plenora-IO-tools` o da
`plenora-database`):

```sh
# 1. Cosa contiene l'input: campi, tipi, geometria, CRS, fingerprint.
plenora-data-tools describe --input citta.arrow

# 2. Il piano si valida contro il contratto dell'input, senza toccare i dati.
plenora-data-tools validate --plan piano.json --input citta=citta.arrow

# 3. Esecuzione: output pubblicato atomicamente, metriche su stdout.
plenora-data-tools run --plan piano.json --input citta=citta.arrow --output output.arrow
```

`citta=` e' il **nome dell'input dichiarato dal piano** (`"inputs": ["citta"]`),
non un'etichetta libera: lega quel percorso a quell'input. Un nome che il piano
non dichiara e' un errore, non un tentativo di indovinare.

Questo piano ha un input solo, quindi `--inputs citta.arrow` funziona ancora.
Con due o piu' input la forma posizionale e' rifiutata: l'ordine non sarebbe
verificabile.

## Cosa mostra

- **`describe` prima di scrivere il piano.** I nomi delle colonne e i tipi
  vengono dall'input, non dalla memoria di chi scrive il piano.
- **`validate` non esegue nulla.** Un piano incompatibile con l'input fallisce
  prima che un solo dato venga letto.
- **L'output non si sovrascrive.** Rieseguire `run` sullo stesso `--output`
  fallisce invece di distruggere il file precedente.
- **Il filtro non e' approssimato.** `abitanti > 300000` confronta interi con
  interi: nessun passaggio da `f64`, quindi nessun valore al limite che cambia
  lato per arrotondamento.
