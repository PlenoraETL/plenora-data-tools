### Che cosa fa

Aggiunge una colonna `float64` con l'azimut geodetico iniziale, in gradi,
dal punto della riga al punto costante `other_wkb`: la direzione in cui
parte la geodetica, misurata in senso orario dal nord (nord 0, est 90, sud
180, ovest 270), in `[0, 360)`. Le coordinate sono longitudine (`x`) e
latitudine (`y`) in gradi; il calcolo è il problema inverso di Karney
(`Geodesic.bearing` di `geo`, `geographiclib-rs`) sull'ellissoide WGS 84.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | il punto d'arrivo, uguale per tutte le righe |
| `output_column` | stringa | `bearing` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi solo nella struttura e nel
dominio del CRS (longitudine in `[-180, 180]`, latitudine in `[-90, 90]`):
il tipo (il kernel vuole un `Point`) non si controlla lì.

### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

### Righe

1:1: un azimut per riga. Per il contratto `other_wkb` è il secondo
operando: la riga è il punto di partenza, `other_wkb` quello d'arrivo
(`extended_algorithms::geodesic_bearing_degrees(riga, other_wkb)`). Due
punti coincidenti danno 180, senza errore. Il runner non esegue ancora le
operazioni geo ([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)):
che cosa rendano una cella nulla e una riga che non è un `Point` lo
fisserà l'esecutore geo.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, coordinate NaN
  o infinite, byte in coda); `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non geografico; una
  coordinata di `other_wkb` fuori dal dominio lon/lat;
- `Internal`: la decodifica di `other_wkb` non conclude.

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel rifiuta la
coppia con `InvalidInput` (coordinate non finite; `ValidazioneNonConclusa`
se la validazione non conclude), `InvalidGeographicCoordinate` (longitudine
fuori da `[-180, 180]` o latitudine fuori da `[-90, 90]`),
`CalcoloNonConcluso` (panico di `geo`).

### Limiti e deviazioni

- **Ellissoide sempre WGS 84**, qualunque sia il datum del CRS geografico
  della colonna (Monte Mario, ED50, NAD27, OSGB36 hanno altri ellissoidi):
  l'azimut è quello delle stesse coordinate su WGS 84.
- Punti coincidenti: 180, non un errore né un valore nullo (`ST_Azimuth`
  di PostGIS rende NULL, e in radianti).
- Ai poli l'azimut dipende dalla longitudine scritta del polo, come in
  `geographiclib`.

### Precisione

La regola di 1 cm riguarda gli spostamenti, e qui l'uscita è un angolo:
nessun controllo e nessun rifiuto di precisione. L'errore è quello
dell'algoritmo di Karney in `f64` più, per un datum diverso da WGS 84,
quello dell'ellissoide sbagliato ([README, «Precisione delle operazioni
geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(1) per riga: un problema inverso geodetico. Memoria O(1).

### Esempio

L'arrivo è `POINT(0 1)`: dall'equatore verso nord, da latitudine 2 verso
sud, e dallo stesso punto.

```json
{
  "config": {"other_wkb": "01010000000000000000000000000000000000f03f"},
  "ingressi": [
    {"nome": "stazioni", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "valori": ["POINT(0 0)", "POINT(0 2)", "POINT(0 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "valori": ["POINT(0 0)", "POINT(0 2)", "POINT(0 1)"]},
    {"nome": "bearing", "tipo": "float64", "valori": [0.0, 180.0, 180.0]}
  ]}
}
```
