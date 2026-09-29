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

1:1 per contratto. Il runner non esegue ancora le operazioni geo e nessun
esecutore chiama il kernel su una tabella: l'analisi conserva la
nullabilità della colonna, il kernel lavora su una geometria alla volta.

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

In esecuzione il kernel (`extended::translate`) rende `ExtendedError`,
che nessun esecutore traduce ancora in `PlenoraError`: `InvalidInput`
(coordinate non finite o geometria non valida OGC), `InvalidOutput`
(uscita non valida OGC, per esempio coordinate che traboccano),
`ValidazioneNonConclusa` e `CalcoloNonConcluso` (validazione o calcolo
interrotti). Un offset non finito passato al kernel è
`InvalidParameter` con nome `coefficients`.

### Limiti e deviazioni

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
