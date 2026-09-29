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
| `x_origin` | numero | non deciso | finito | x dell'origine fissa |
| `y_origin` | numero | non deciso | finito | y dell'origine fissa |

`x_origin` e `y_origin` sono facoltativi per l'analisi, ma il kernel
(`extended::scale_about`) riceve l'origine sempre esplicita e nessun
esecutore lo chiama ancora: l'origine usata quando mancano non è decisa.
Un piano che vuole un risultato definito le scrive.

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
- `InvalidPlan`: config con campi sconosciuti, un fattore assente o non
  finito, un'origine scritta e non finita;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione il kernel rende `ExtendedError`, che nessun esecutore
traduce ancora in `PlenoraError`: `InvalidInput` (coordinate non finite o
geometria non valida OGC), `InvalidParameter` con nome `coefficients`
(un coefficiente della matrice non finito, anche per overflow di
`x_origin (1 - x_factor)`), `InvalidOutput` (uscita non valida OGC: un
fattore nullo schiaccia una superficie su una retta o un punto),
`ValidazioneNonConclusa` e `CalcoloNonConcluso` (validazione o calcolo
interrotti).

### Limiti e deviazioni

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
