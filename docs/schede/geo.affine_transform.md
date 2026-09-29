### Che cosa fa

Applica a ogni geometria la trasformazione affine 2D di matrice
`[a, b, xoff, d, e, yoff]`: ogni vertice `(x, y)` diventa
`(a x + b y + xoff, d x + e y + yoff)`. Tipo e struttura della geometria
non cambiano. [`geo.translate`](#geotranslate), [`geo.scale`](#geoscale) e
[`geo.rotate`](#georotate) sono casi particolari della stessa funzione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `coefficients` | lista di numeri | obbligatorio | esattamente 6 numeri finiti | `[a, b, xoff, d, e, yoff]`, nelle unità del CRS |

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
  colonna non si dichiara geometria WKB (né estensione `geoarrow.wkb` né
  chiavi `plenora.geometry.*`);
- `Unsupported`: dimensioni della colonna diverse da XY (anche non
  dichiarate);
- `InvalidPlan`: config con campi sconosciuti, `coefficients` assente, non
  di 6 elementi o con un valore non finito;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione il kernel (`extended::affine_transform`) rende
`ExtendedError`, che nessun esecutore traduce ancora in `PlenoraError`:

- `InvalidInput`: geometria con coordinate NaN o infinite o non valida
  OGC;
- `InvalidOutput`: la geometria trasformata non è valida OGC (una matrice
  singolare che schiaccia una superficie su una retta, coordinate che
  traboccano a infinito);
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: validazione OGC o
  calcolo di `geo` interrotti (non accusano l'ingresso).

### Limiti e deviazioni

Le coordinate d'uscita non si confrontano con il dominio di validità del
CRS ([README, «CRS integrati»](../README.md#crs-integrati)): una matrice
che porta la geometria fuori dal dominio non è un errore qui.

### Precisione

Il calcolo è in `f64`, tre arrotondamenti per coordinata, senza
fusione: l'errore resta sotto pochi ulp della somma `|a x| + |b y| +
|xoff|`, cioè sotto 1 cm finché quella somma sta sotto circa `3e13` unità
del CRS, ben oltre ogni dominio dei CRS integrati. Con coefficienti e
coordinate interi (o comunque rappresentabili e con prodotti esatti) il
risultato è esatto. Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia trasformata.

### Esempio

`x' = 2 x + 10`, `y' = y - 5`: il quadrato unitario diventa un rettangolo
2 × 1.

```json
{
  "config": {"coefficients": [2, 0, 10, 0, 1, -5]},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,1 0,1 1,0 1,0 0))", "POINT(3 4)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((10 -5,12 -5,12 -4,10 -4,10 -5))", "POINT(16 -1)"]}
  ]}
}
```
