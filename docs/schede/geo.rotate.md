### Che cosa fa

Ruota ogni geometria di `degrees` gradi in verso antiorario attorno al
centro `(x_origin, y_origin)`. È
[`geo.affine_transform`](#geoaffine_transform) con la matrice
`[cos, -sin, x_off, sin, cos, y_off]`, dove seno e coseno si calcolano in
`f64` dall'angolo in radianti e `x_off`, `y_off` tengono fermo il centro.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `degrees` | numero | obbligatorio | finito | angolo in gradi, positivo in verso antiorario |
| `x_origin` | numero | `0` | finito | x del centro di rotazione |
| `y_origin` | numero | `0` | finito | y del centro di rotazione |

`x_origin` e `y_origin` sono facoltativi, anche uno solo: il kernel
(`extended::rotate_about`) riceve il centro sempre esplicito, e il runner
mette `0` al posto di ciascuno che manca. Senza entrambi si ruota attorno
a `(0, 0)` del CRS, non attorno alla geometria.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati; la
colonna geometria resta al suo posto con lo stesso CRS, le stesse
dimensioni (XY) e gli stessi tipi geometrici dichiarati. Le proprietà del
contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1: il runner chiama il kernel (`extended::rotate_about`) su ogni cella non
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
- `InvalidPlan`: config con campi sconosciuti, `degrees` assente o non
  finito, un centro scritto e non finito;
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
per le altre: `InvalidParameter` (`degrees` non
finito, o `coefficients` per un termine della matrice che trabocca),
`InvalidInput` (coordinate non finite o geometria non valida OGC),
`InvalidOutput` (uscita non valida OGC), `ValidazioneNonConclusa` e
`CalcoloNonConcluso` (validazione o calcolo interrotti).

Una geometria prodotta oltre il limite di byte per cella (64 MiB di WKB)
è `ResourceLimit`. Il primo errore è quello della prima riga in ordine di riga, senza
diagnostica per riga.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione provvisoria
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo provvisori»).
Nessuna rotazione è esatta, nemmeno di 90 o 180 gradi: il coseno di 90
gradi in `f64` vale circa `6.1e-17`, non zero (vedi l'esempio). Le
coordinate d'uscita non si confrontano con il dominio di validità del CRS
([README, «CRS integrati»](../README.md#crs-integrati)).

### Precisione

Seno e coseno portano un errore relativo di circa un ulp, che sposta un
vertice di circa `1e-16` volte la sua distanza dal centro; la matrice si
applica come in [`geo.affine_transform`](#geoaffine_transform), con
errore di pochi ulp dei termini. In tutto, sotto 1 cm finché coordinate
e centro stanno sotto circa `1e13` unità del CRS, ben oltre ogni dominio
dei CRS integrati. Nessun rifiuto `PrecisionInsufficient`
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

Tempo O(n) sulle coordinate della geometria, più la validazione OGC di
ingresso e uscita; memoria O(n) per la copia ruotata.

### Esempio

Un quarto di giro attorno all'origine: la x attesa è 0, quella calcolata
è il residuo del coseno di 90 gradi per 10.

```json
{
  "config": {"degrees": 90, "x_origin": 0, "y_origin": 0},
  "ingressi": [
    {"nome": "assi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(10 0)", "LINESTRING(0 0,10 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0.0000000000000006123233995736766 10)", "LINESTRING(0 0,0.0000000000000006123233995736766 10)"]}
  ]}
}
```
