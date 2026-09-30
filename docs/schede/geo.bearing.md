### Che cosa fa

Aggiunge una colonna `float64` con l'azimut geodetico iniziale, in gradi,
dal punto della riga al punto costante `other_wkb`: la direzione in cui
parte la geodetica, misurata in senso orario dal nord (nord 0, est 90, sud
180, ovest 270), in `[0, 360)`. Le coordinate sono longitudine (`x`) e
latitudine (`y`) in gradi; il calcolo è il problema inverso di Karney
(`geographiclib-rs`) sull'ellissoide del datum del CRS della colonna
([README, «Misure geodetiche: l'ellissoide del datum»](../README.md#misure-geodetiche-lellissoide-del-datum)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari) di un `Point`, nel CRS dell'input e dentro il suo dominio di validità | il punto d'arrivo, uguale per tutte le righe |
| `output_column` | stringa | `bearing` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi: struttura, validità OGC,
dominio del CRS (longitudine in `[-180, 180]`, latitudine in `[-90, 90]`)
e tipo (deve essere un `Point`, quello che il kernel chiede).

### Schema

Aggiunge in coda `output_column`, `float64`, nullable solo se lo è la colonna geometria (null dove la geometria è null), senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

### Righe

1:1: un azimut per riga. Per il contratto `other_wkb` è il secondo
operando: la riga è il punto di partenza, `other_wkb` quello d'arrivo
(`extended_algorithms::geodesic_bearing_degrees(riga, other_wkb)`). Dove
l'azimut non è definito il passo si ferma (vedi «Errori»): punti
coincidenti, partenza su un polo, geodetica più breve non unica. Una
geometria nulla dà un azimut nullo; una riga che non è un `Point` ferma il passo con un errore
(vedi «Errori»).

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, coordinate NaN
  o infinite, byte in coda), non valido OGC o che non è un `Point`;
  `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non geografico; CRS
  senza l'ellissoide del datum (`ELLIPSOID_REQUIRED`); una coordinata di
  `other_wkb` fuori dal dominio lon/lat;
- `Internal`: la decodifica o la validazione OGC di `other_wkb` non
  conclude.

In esecuzione ([README, «Operazioni geo»](../README.md#operazioni-geo))
il passo rende il primo errore in ordine di riga, senza diagnostica per
riga. Prima del kernel, per ogni cella non nulla della colonna geometria:

- `InvalidPlan`: struttura WKB non valida; `Unsupported`: la cella porta
  Z/M o uno SRID; `ResourceLimit`: la cella supera 64 MiB;
- `Crs`: una coordinata fuori dal dominio di validità del CRS della
  colonna;
- `Schema`: il contratto d'ingresso dichiara i tipi geometrici con un
  elenco e la cella è di un altro tipo.

Poi, per riga:

- `InvalidPlan`: la geometria della riga non è un `Point` (errore del
  runner, «tipo geometria non supportato»); dal kernel
  (`ExtendedAlgorithmError`) `InvalidInput` (coordinate non finite) e
  `InvalidGeographicCoordinate` (longitudine fuori da `[-180, 180]` o
  latitudine fuori da `[-90, 90]`) e `AzimutNonDefinito` («azimut non
  definito»): punti coincidenti (distanza geodetica nulla, anche `-180` e
  `180` alla stessa latitudine), punto della riga su un polo (latitudine
  ±90: ogni direzione è sud o nord), o destinazione sul luogo di taglio
  (latitudine opposta e longitudine quasi opposta, antipodi compresi), dove
  due geodetiche ugualmente brevi partono con azimut che, alla distanza
  della destinazione, si separano di più di 1 cm;
- `Internal`: dal kernel `ValidazioneNonConclusa` (la validazione non
  conclude) e `CalcoloNonConcluso` (panico di `geo`).

### Limiti e deviazioni

- Dove l'azimut non è definito il passo si ferma con un errore: fino alla
  versione 1 del catalogo due punti coincidenti davano 180 e due antipodi
  0 (i valori convenzionali di `geographiclib`), e l'ellissoide era sempre
  WGS 84. `ST_Azimuth` di PostGIS rende NULL per punti coincidenti.
- Una destinazione su un polo è ammessa (l'azimut è 0 o 180); la partenza
  no.
- CRS proiettati rifiutati.
- Solo `Point` nella colonna: una `MultiPoint` ferma il passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

La regola di 1 cm riguarda gli spostamenti, e qui l'uscita è un angolo:
nessun controllo e nessun rifiuto di precisione. L'errore è quello
dell'algoritmo di Karney in `f64` sull'ellissoide del datum; per punti a
pochi millimetri l'azimut è mal condizionato (un nanometro di posizione
sono gradi di direzione) ([README, «Precisione delle operazioni
geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(1) per riga: un problema inverso geodetico. Memoria O(1).

### Esempio

L'arrivo è `POINT(0 1)`: dall'equatore verso nord, da latitudine 2 verso
sud, e da est verso ovest (poco a nord di ovest).

```json
{
  "config": {"other_wkb": "01010000000000000000000000000000000000f03f"},
  "ingressi": [
    {"nome": "stazioni", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "valori": ["POINT(0 0)", "POINT(0 2)", "POINT(1 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "valori": ["POINT(0 0)", "POINT(0 2)", "POINT(1 1)"]},
    {"nome": "bearing", "tipo": "float64", "valori": [0.0, 180.0, 270.00872642616275]}
  ]}
}
```
