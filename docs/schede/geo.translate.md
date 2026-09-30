### Che cosa fa

Sposta ogni geometria di `(x_offset, y_offset)` nelle unità del CRS: è
[`geo.affine_transform`](#geoaffine_transform) con la matrice
`[1, 0, x_offset, 0, 1, y_offset]`. Tipo e struttura della geometria non
cambiano.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `x_offset` | numero | obbligatorio | finito | spostamento lungo x |
| `y_offset` | numero | obbligatorio | finito | spostamento lungo y |

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati; la
colonna geometria resta al suo posto con lo stesso CRS, le stesse
dimensioni (XY) e gli stessi tipi geometrici dichiarati. Le proprietà del
contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1: il runner chiama il kernel (`extended::translate`) su ogni cella non
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
- `InvalidPlan`: config con campi sconosciuti, un offset assente o non
  finito;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([README, «Operazioni geo»](../README.md#operazioni-geo)).

Poi il kernel (`extended::translate`) rende `ExtendedError`, che il
runner porta in `Internal` per `ValidazioneNonConclusa` e
`CalcoloNonConcluso`, in `InvalidPlan` per le altre: `InvalidInput`
(coordinate non finite o geometria non valida OGC), `InvalidOutput`
(uscita non valida OGC, per esempio coordinate che traboccano),
`ValidazioneNonConclusa` e `CalcoloNonConcluso` (validazione o calcolo
interrotti). Un offset non finito passato al kernel è
`InvalidParameter` con nome `coefficients`.

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

Un arrotondamento per coordinata (`1 x + 0 y` è esatto): l'errore è al
più mezzo ulp del risultato, sotto 1 cm per coordinate d'uscita sotto
circa `7e13` unità del CRS. Esatto quando la somma è rappresentabile, come
con coordinate e offset interi. Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia spostata.

### Esempio

```json
{
  "config": {"x_offset": 100, "y_offset": -50},
  "ingressi": [
    {"nome": "pozzi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1 2)", "LINESTRING(0 0,3 4)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(101 -48)", "LINESTRING(100 -50,103 -46)"]}
  ]}
}
```
