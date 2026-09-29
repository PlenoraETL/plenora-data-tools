### Che cosa fa

Genera una griglia regolare di celle poligonali che copre il rettangolo
`extent`, una riga per cella, con gli indici di colonna e di riga della
cella e, a richiesta, il suo centro. Le celle quadrate di lato `cell_size`
coprono tutto il rettangolo, e quelle dell'ultima colonna e dell'ultima
riga sono tagliate sul suo bordo; le celle esagonali (esagoni con un lato in
alto, di lato `cell_size`) escono solo se stanno per intero dentro il
rettangolo. La tabella d'ingresso serve solo da innesco: né le sue colonne
né le sue righe entrano nell'uscita.

Il calcolo è `extensions2::generate_grid_rows`; il runner non esegue
ancora l'operazione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `extent` | oggetto | obbligatorio | `xmin`, `ymin`, `xmax`, `ymax` finiti, `xmax > xmin`, `ymax > ymin`, vertici nel dominio del CRS | rettangolo da coprire, nelle unità del CRS |
| `cell_size` | numero | obbligatorio | finito, `> 0` | lato della cella (quadrata o esagonale) |
| `shape` | stringa | `square` | `square`, `hex` | forma delle celle |
| `crs` | stringa | CRS di piano | identificatore di un CRS integrato | CRS della griglia |
| `include_centroid` | booleano | `false` | `true`, `false` | aggiunge le coordinate del centro della cella |

Le celle non possono superare 1.000.000 (per le esagonali, anche il numero
di colonne): il conteggio si verifica in validazione, prima di allocare.

### Schema

Schema nuovo, in quest'ordine: `geometry` (`binary` non nullable,
estensione `geoarrow.wkb`, CRS della griglia, dimensioni `xy`; il contratto
dichiara i tipi `exact` `Polygon`), `cell_i` e `cell_j` (`uint64` non
nullable) e, con `include_centroid`, `centroid_x` e `centroid_y`
(`float64` non nullable). Le colonne dell'ingresso spariscono, i metadati
di schema restano. Il contratto dichiara `row_count` come stima
(`Estimated`) con il numero esatto di celle, calcolato a secco;
`sorted_by` non c'è.

### Righe

Una riga per cella, indipendente dall'ingresso:

- `square`: `ceil((xmax - xmin) / cell_size)` colonne per
  `ceil((ymax - ymin) / cell_size)` righe; la cella `(i, j)` va da
  `xmin + i * cell_size` a `min(xmin + i * cell_size + cell_size, xmax)` in
  x, e allo stesso modo in y; il centro è il punto medio della cella
  (tagliata);
- `hex`: centri a passo `1.5 * cell_size` in x, a partire da
  `xmin + cell_size`, e a passo `sqrt(3) * cell_size` in y, con le colonne
  dispari sfalsate di mezzo passo; nessuna cella se il rettangolo è più
  stretto di `2 * cell_size`.

`cell_i` conta le colonne da `xmin`, `cell_j` le righe da `ymin`, da 0.

### Ordine

`square`: per riga (`cell_j` crescente), poi per colonna (`cell_i`
crescente). `hex`: per colonna, poi per riga.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti o `shape` fuori elenco;
  `extent` non finito o degenere; `cell_size` non finito o non positivo;
  più di 1.000.000 celle; numero di celle per asse non rappresentabile;
- `Schema`: l'ingresso ha già una colonna geometria;
- `Crs`: né `crs` né un CRS di piano; `crs` non risolvibile; un vertice di
  `extent` fuori dal dominio del CRS.

In esecuzione il calcolo rifà gli stessi controlli di `extent`,
`cell_size` e numero di celle (`InvalidPlan`, messaggio con prefisso
`geo.generate_grid:`).

### Limiti e deviazioni

Al più 1.000.000 di celle. Le celle esagonali non coprono tutto il
rettangolo: restano scoperte una fascia a destra larga meno di
`1.5 * cell_size`, una in alto larga meno di `sqrt(3) * cell_size` e, sotto
le colonne dispari, mezzo esagono.

### Precisione

Nessun controllo dedicato: le coordinate si calcolano in `f64`, senza
fusione delle operazioni
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Sono esatte quando `xmin`, `ymin`, `cell_size` e i loro multipli sono
rappresentabili; altrimenti:

- `square`: ogni coordinata ha pochi arrotondamenti; il lato destro
  di una cella (`x0 + cell_size`) e il sinistro della successiva
  (`xmin + (i + 1) * cell_size`) si calcolano in due modi e possono
  differire di un'unità in ultima posizione: fessure o sovrapposizioni di
  quell'ordine, molto sotto 1 cm;
- `hex`: l'ordinata del centro si accumula per somme successive, quindi lo
  scarto dalla griglia esatta cresce con il numero di righe, al più
  `10^6 * ulp(M) / 2` con `M` il modulo massimo delle coordinate (circa 1,9
  mm a 2·10^7 m); i vertici in comune fra esagoni vicini si calcolano da
  centri diversi e possono differire di un'unità in ultima posizione.

### Complessità

O(c) in tempo e memoria sulle celle `c`, al più 1.000.000; il conteggio in
validazione costa O(1) per `square` e O(c) per `hex`.

### Esempio

```json
{
  "config": {"extent": {"xmin": 0, "ymin": 0, "xmax": 20, "ymax": 10}, "cell_size": 10, "crs": "EPSG:3857", "include_centroid": true},
  "ingressi": [
    {"nome": "innesco", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,10 10,0 10,0 0))", "POLYGON((10 0,20 0,20 10,10 10,10 0))"]},
    {"nome": "cell_i", "tipo": "uint64", "valori": [0, 1]},
    {"nome": "cell_j", "tipo": "uint64", "valori": [0, 0]},
    {"nome": "centroid_x", "tipo": "float64", "valori": [5.0, 15.0]},
    {"nome": "centroid_y", "tipo": "float64", "valori": [5.0, 5.0]}
  ]}
}
```
