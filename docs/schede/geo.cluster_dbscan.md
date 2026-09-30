### Che cosa fa

Raggruppa i punti della tabella per densità con DBSCAN e aggiunge a ogni
riga l'etichetta del suo cluster: un punto è «core» se entro `eps` (lui
compreso) ci sono almeno `min_points` punti; i cluster sono i core
collegati per densità più i punti di bordo che raggiungono; gli altri punti
sono rumore ed escono con etichetta nulla. Accetta solo geometrie `Point`.

La conversione di colonna è `cluster::dbscan_column`, che il runner
chiama su tutta la colonna e aggiunge in coda l'etichetta ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `eps` | numero | obbligatorio | finito, `> 0` | raggio del vicinato, nelle unità del CRS (distanza `<= eps`) |
| `min_points` | intero | obbligatorio | `>= 1` | punti minimi nel vicinato, il punto stesso compreso, perché sia core |
| `output_column` | stringa | `cluster_id` | nome non vuoto e libero | colonna dell'etichetta |

### Schema

Aggiunge in coda `output_column`, `uint64` nullable. Le altre colonne, i
metadati e le proprietà del contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1. L'etichetta vale da 0 a `k - 1` per `k` cluster; è nulla per il
rumore e per le righe con geometria nulla (che non partecipano al calcolo).
I punti coincidenti contano tutti nel vicinato. Un punto di bordo
raggiungibile da due cluster va al primo che lo raggiunge.

### Ordine

Quello d'ingresso. I cluster sono numerati nell'ordine di scoperta: si
visitano le righe per indice crescente, i vicini in ordine d'indice, e
l'espansione è in ampiezza; stesso ingresso, stesse etichette.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `eps` mancante, non finito o
  non positivo; `min_points` mancante, zero o non intero; `output_column`
  vuota;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB; `output_column` già presente;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS risolto o CRS non proiettato.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare.

Poi la conversione di colonna (messaggi del calcolo con prefisso
`geo.cluster_dbscan:`):

- `InvalidPlan`: WKB malformato o OGC-invalido; una geometria che non è
  `Point` (il messaggio riporta la posizione della riga, non i dati);
- `Unsupported`: WKB con dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella;
- `Internal`: panico di `rstar`, invariante violata, validazione che non
  conclude.

### Limiti e deviazioni

Solo punti: un poligono o una linea si rifiutano, senza passare dal
centroide, che non ne rappresenta la densità. Il rumore e la geometria
nulla hanno la stessa etichetta nulla. Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo
provvisori»).

### Precisione

Nessuna geometria calcolata. L'appartenenza al vicinato si decide in `f64`
come `dx² + dy² <= eps²`, senza fusione delle operazioni: un punto a
distanza pari a `eps` entro l'arrotondamento può cadere da una parte o
dall'altra
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

R-tree dei punti, O(n log n); una ricerca per raggio per coordinata
distinta, O(log n + m) con `m` i vicini trovati: O(n²) nel caso peggiore,
quando quasi tutti i punti stanno entro `eps` l'uno dall'altro. Memoria O(n)
per punti ed etichette, più i vicinati trattenuti.

### Esempio

```json
{
  "config": {"eps": 1.5, "min_points": 2},
  "ingressi": [
    {"nome": "segnalazioni", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0 0)", "POINT(1 0)", "POINT(50 50)", "POINT(0 1)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(0 0)", "POINT(1 0)", "POINT(50 50)", "POINT(0 1)"]},
    {"nome": "cluster_id", "tipo": "uint64", "valori": [0, 0, null, 0]}
  ]}
}
```
