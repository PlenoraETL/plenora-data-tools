### Che cosa fa

Scala ogni geometria di `x_factor` lungo x e `y_factor` lungo y attorno
all'origine `(x_origin, y_origin)`, che resta ferma: `x' = x_factor x +
x_origin (1 - x_factor)`, e lo stesso per y. È
[`geo.affine_transform`](#geoaffine_transform) con la matrice
corrispondente. Un fattore negativo riflette la geometria.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `x_factor` | numero | obbligatorio | finito | fattore lungo x |
| `y_factor` | numero | obbligatorio | finito | fattore lungo y |
| `x_origin` | numero | `0` | finito | x dell'origine fissa |
| `y_origin` | numero | `0` | finito | y dell'origine fissa |

`x_origin` e `y_origin` sono facoltativi, anche uno solo: il kernel
(`extended::scale_about`) riceve l'origine sempre esplicita, e il runner
mette `0` al posto di ciascuna che manca. Senza entrambe si scala attorno
a `(0, 0)` del CRS, non attorno alla geometria.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati; la
colonna geometria resta al suo posto con lo stesso CRS, le stesse
dimensioni (XY) e gli stessi tipi geometrici dichiarati. Le proprietà del
contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1: il runner chiama il kernel (`extended::scale_about`) su ogni cella non
nulla, in parallelo, e rimette la geometria al suo posto; una cella
nulla resta nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Ordine

Per contratto quello d'ingresso (forma 1:1).

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si dichiara geometria WKB;
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, un fattore assente o non
  finito, un'origine scritta e non finita;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi il kernel rende `ExtendedError`, che il runner porta in `Internal`
per `ValidazioneNonConclusa` e `CalcoloNonConcluso`, in `InvalidPlan`
per le altre: `InvalidInput` (coordinate non finite o
geometria non valida OGC), `InvalidParameter` con nome `coefficients`
(un coefficiente della matrice non finito, anche per overflow di
`x_origin (1 - x_factor)`), `InvalidOutput` (uscita non valida OGC: un
fattore nullo schiaccia una superficie su una retta o un punto),
`ValidazioneNonConclusa` e `CalcoloNonConcluso` (validazione o calcolo
interrotti).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
Le coordinate d'uscita non si confrontano con il dominio di validità del
CRS ([README, «CRS integrati»](../README.md#crs-integrati)).

### Precisione

Come [`geo.affine_transform`](#geoaffine_transform): calcolo in `f64`
senza fusione, errore di pochi ulp di `|x_factor x| + |x_origin (1 -
x_factor)|`, sotto 1 cm finché quei termini stanno sotto circa `3e13`
unità del CRS; esatto con fattori, origine e coordinate interi. Nessun
rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia scalata.

### Esempio

Fattori 2 e 3 attorno al vertice `(1, 1)`, che resta fermo.

```json
{
  "config": {"x_factor": 2, "y_factor": 3, "x_origin": 1, "y_origin": 1},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((1 1,2 1,2 2,1 2,1 1))"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((1 1,3 1,3 4,1 4,1 1))"]}
  ]}
}
```
