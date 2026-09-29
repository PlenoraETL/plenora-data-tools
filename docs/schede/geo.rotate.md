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
| `x_origin` | numero | non deciso | finito | x del centro di rotazione |
| `y_origin` | numero | non deciso | finito | y del centro di rotazione |

`x_origin` e `y_origin` sono facoltativi per l'analisi, ma il kernel
(`extended::rotate_about`) riceve il centro sempre esplicito e nessun
esecutore lo chiama ancora: il centro usato quando mancano non è deciso.
Un piano che vuole un risultato definito li scrive.

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
- `InvalidPlan`: config con campi sconosciuti, `degrees` assente o non
  finito, un centro scritto e non finito;
- `Crs`: CRS della colonna mancante o non risolto, non proiettato
  (`PROJECTED_CRS_REQUIRED`) o senza unità lineare
  (`LINEAR_UNIT_REQUIRED`).

In esecuzione il kernel rende `ExtendedError`, che nessun esecutore
traduce ancora in `PlenoraError`: `InvalidParameter` (`degrees` non
finito, o `coefficients` per un termine della matrice che trabocca),
`InvalidInput` (coordinate non finite o geometria non valida OGC),
`InvalidOutput` (uscita non valida OGC), `ValidazioneNonConclusa` e
`CalcoloNonConcluso` (validazione o calcolo interrotti).

### Limiti e deviazioni

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
