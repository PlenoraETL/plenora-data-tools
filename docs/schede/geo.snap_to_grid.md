### Che cosa fa

Porta ogni vertice sul nodo più vicino di una griglia regolare di passo
`grid_size` con origine in `(0, 0)`: per asse `round(x / grid_size) *
grid_size`, a metà strada lontano da zero, con `-0.0` reso `0.0`. Non
ripara e non semplifica: i vertici consecutivi che cadono sullo stesso nodo
restano duplicati, e un collasso che rende la geometria non valida (una
linea ridotta a un punto, un anello degenere o auto-intersecato) è un
errore, non una correzione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `grid_size` | numero | obbligatorio | finito, maggiore di zero | passo della griglia, nelle unità del CRS |

### Schema

Invariato: la colonna geometria si riscrive al suo posto, con lo stesso
nome, gli stessi metadati, lo stesso CRS, dimensioni `xy` e gli stessi tipi
dichiarati. Le altre colonne, i metadati di schema e le proprietà del
contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1: la geometria di ogni riga diventa quella agganciata alla griglia. Il
runner chiama il kernel (`extended_algorithms::snap_to_grid`) su ogni
cella non nulla, in parallelo; una cella nulla resta nulla
([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

### Ordine

Quello d'ingresso; dentro una geometria i vertici restano nel loro ordine,
anche quelli diventati uguali.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
config, CRS):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `grid_size` o con un tipo sbagliato; `grid_size` non finito o non
  maggiore di zero;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB;
- `Unsupported`: colonna geometria con dimensioni diverse da `xy`;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

Poi il kernel rende `ExtendedAlgorithmError`, che il runner porta in
`Internal` per `Internal`, `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre. Rifiuta la geometria
con `InvalidInput` (coordinate non finite o geometria non valida
per l'OGC; `ValidazioneNonConclusa` se la validazione non conclude) e con
`InvalidOutput` quando un vertice agganciato non è finito (overflow) o
quando la geometria agganciata non è valida per l'OGC: per esempio un
quadrato di lato 0,4 con `grid_size` 1 (`punti distinti insufficienti`).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

- A differenza di `ST_SnapToGrid` di PostGIS, che toglie i vertici
  consecutivi uguali e rende NULL una geometria collassata, qui i
  duplicati restano e il collasso che viola la validità OGC è un errore;
  un collasso che la lascia valida (una parte sottile che cambia forma)
  passa senza errore.
- Griglia con origine in `(0, 0)` e passo uguale sui due assi.

### Precisione

Lo spostamento è voluto e dichiarato dal parametro: ogni vertice si sposta
fino a `grid_size * sqrt(2) / 2`, entro 1 cm solo se `grid_size` è sotto
circa 1,4 cm; oltre, la regola di 1 cm non si applica a questa
operazione, perché lo spostamento è la sua definizione ([Limiti dichiarati,
«Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).
Il nodo è il prodotto `round(x / grid_size) * grid_size` arrotondato in
`f64`: con un passo non rappresentabile esattamente (0,1) è il `f64` più
vicino al multiplo, non il multiplo esatto. Nessun rifiuto
`PrecisionInsufficient`.

### Complessità

O(n) per geometria, con `n` le coordinate, più la validazione OGC
dell'ingresso e dell'uscita (O(n²) nel caso peggiore, [Limiti dichiarati,
«Validazione OGC: la ricerca delle auto-intersezioni non è quella di `geo`,
il verdetto sì»](limiti.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).
Memoria O(n).

### Esempio

```json
{
  "config": {"grid_size": 1},
  "ingressi": [
    {"nome": "rilievi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1.4 2.6)", "LINESTRING(0.2 0.1,9.7 0.4)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1 3)", "LINESTRING(0 0,10 0)"]}
  ]}
}
```
