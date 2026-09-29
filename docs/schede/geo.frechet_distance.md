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
| `other_wkb` | stringa | obbligatorio | WKB 2D in esadecimale (cifre maiuscole o minuscole, in numero pari), nel CRS dell'input e dentro il suo dominio di validità | la linea di confronto, uguale per tutte le righe |
| `output_column` | stringa | `frechet_distance` | nome non vuoto e non già nello schema | colonna aggiunta |

`other_wkb` si decodifica e si valida in analisi solo nella struttura e nel
dominio del CRS: né il tipo (il kernel vuole una `LineString`) né la
validità OGC si controllano lì.

### Schema

Aggiunge in coda `output_column`, `float64` nullable, senza metadati. Le
altre colonne restano nell'ordine e con i loro metadati; la colonna
geometria resta com'è. Metadati di schema e proprietà del contratto
(`sorted_by`, `row_count`) si conservano.

### Righe

1:1: una distanza per riga, nelle unità del CRS. Per il contratto
`other_wkb` è il secondo operando; la distanza discreta è simmetrica, quindi
l'ordine non cambia il valore. Il kernel
(`extended_algorithms::frechet_distance`) riceve due `LineString` e rende
nessun valore se una delle due è vuota. Il runner non esegue ancora le
operazioni geo ([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)):
che cosa rendano una cella nulla, una linea vuota e una geometria di altro
tipo lo fisserà l'esecutore geo, come il limite delle coppie di vertici per
piano.

### Ordine

Quello d'ingresso.

### Errori

In validazione (analisi del contratto; nell'ordine: forma dell'ingresso,
CRS, config):

- `InvalidPlan`: più o meno di un ingresso; config con campi sconosciuti,
  senza `other_wkb` o con un tipo sbagliato; `other_wkb` vuoto, di
  lunghezza dispari o con caratteri non esadecimali; WKB strutturalmente
  non valido (byte o conteggi oltre i limiti, annidamento, anelli non
  chiusi, coordinate NaN o infinite, byte in coda); `output_column` vuoto;
- `Unsupported`: `other_wkb` con coordinate Z/M o con SRID (EWKB); colonna
  geometria con dimensioni diverse da `xy`;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come geometria WKB; `output_column` già
  presente;
- `Crs`: CRS della colonna assente o non risolto; CRS non proiettato o
  senza unità lineare; una coordinata di `other_wkb` fuori dal dominio di
  validità del CRS;
- `Internal`: la decodifica di `other_wkb` non conclude.

In esecuzione: il runner non esegue l'operazione, e agli errori del kernel
non è ancora assegnata una variante `PlenoraError`. Il kernel rifiuta la
coppia con `InvalidInput` (coordinate non finite o linea con meno di due
punti distinti; prima la riga, poi `other_wkb`; `ValidazioneNonConclusa`
se la validazione non conclude), `WorkLimit` (prodotto dei vertici delle
due linee oltre il limite del chiamante, o non rappresentabile in `u64`),
`CalcoloNonConcluso` (panico di `geo`).

### Limiti e deviazioni

- Distanza discreta, sui soli vertici, come `ST_FrechetDistance` di
  PostGIS senza densificazione; nessun parametro di densificazione qui.
- Il lavoro quadratico è limitato da un argomento del kernel, senza valore
  predefinito qui.
- La seconda linea è una costante della config, non una seconda colonna.

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
