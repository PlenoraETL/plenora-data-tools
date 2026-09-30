### Che cosa fa

Aggiunge una colonna `float64` con la distanza di Fréchet discreta fra la
linea della riga e la linea costante `other_wkb`: la più piccola, fra tutti
gli accoppiamenti dei vertici che percorrono le due linee in avanti senza
tornare indietro, della massima distanza euclidea fra due vertici
accoppiati. Misura quanto due percorsi si somigliano tenendo conto del
verso: la stessa linea percorsa al contrario è lontana. Si accoppiano solo
i vertici, quindi un vertice in più su un lato dritto può cambiare il
risultato (la distanza continua sarebbe minore o uguale).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari) di una `LineString` valida OGC, nel CRS dell'input e dentro il suo dominio di validità | la linea di confronto, uguale per tutte le righe |
| `output_column` | stringa | `frechet_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi: struttura, validità OGC,
dominio del CRS e tipo (deve essere una `LineString`, quella che il kernel
chiede).

### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

### Righe

1:1: una distanza per riga, nelle unità del CRS. Per il contratto
`other_wkb` è il secondo operando; la distanza discreta è simmetrica, quindi
l'ordine non cambia il valore. Il kernel
(`extended_algorithms::frechet_distance`) riceve due `LineString`. Una
geometria nulla dà una distanza nulla, e anche una linea vuota, da una
parte o dall'altra (il kernel non rende un valore); una riga che non è una
`LineString` ferma il passo con un errore (vedi «Errori»).

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda), non valido OGC o che
  non è una `LineString`; `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare; una coordinata di `other_wkb` fuori dal dominio di
  validità del CRS;
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

- `InvalidPlan`: la geometria della riga non è una `LineString` (errore
  del runner, «tipo geometria non supportato»); dal kernel `InvalidInput`
  (coordinate non finite o linea con meno di due punti distinti),
  `WorkLimit` (prodotto dei vertici delle due linee oltre `10^8`, il
  tetto che il runner passa, o non rappresentabile in `u64`),
  `IndexOverflow`;
- `Internal`: dal kernel `ValidazioneNonConclusa` (la validazione non
  conclude) e `CalcoloNonConcluso` (panico di `geo`).

### Limiti e deviazioni

- Distanza discreta, sui soli vertici, come `ST_FrechetDistance` di
  PostGIS senza densificazione; nessun parametro di densificazione qui.
- Il lavoro quadratico è limitato da un argomento del kernel: il runner
  passa `10^8` coppie di vertici per riga (l'ordine di `MAX_NODING_WORK`
  dei kernel); non è un parametro della config.
- La seconda linea è una costante della config, non una seconda colonna.
- Solo `LineString`: una `MultiLineString` nella colonna ferma il passo.
- Errori senza indice di riga della sorgente
([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voce «Geo senza diagnostica per riga»).

### Precisione

Nessun controllo e nessun rifiuto di precisione. Il risultato è una delle
distanze euclidee fra un vertice della riga e uno di `other_wkb`, scelta
con massimi e minimi esatti; ogni distanza è calcolata in `f64` con
l'errore relativo di un arrotondamento, molto sotto 1 cm ([README,
«Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n·m) per riga, con `n` e `m` i vertici delle due linee, limitato prima
del calcolo; memoria O(min(n, m)). In più la validazione OGC delle due
linee.

### Esempio

La linea di confronto è `LINESTRING(0 0,10 0)`. La seconda riga ha gli
stessi punti, ma il vertice in mezzo si accoppia con un estremo: distanza 5;
la terza è la stessa linea al contrario.

```json
{
  "config": {"other_wkb": "0102000000020000000000000000000000000000000000000000000000000024400000000000000000"},
  "ingressi": [
    {"nome": "tracce", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 1,10 1)", "LINESTRING(0 0,5 0,10 0)", "LINESTRING(10 0,0 0)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 1,10 1)", "LINESTRING(0 0,5 0,10 0)", "LINESTRING(10 0,0 0)"]},
    {"nome": "frechet_distance", "tipo": "float64", "valori": [1.0, 5.0, 10.0]}
  ]}
}
```
