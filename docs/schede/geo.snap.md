### Che cosa fa

Aggancia ogni vertice della geometria di ogni riga al vertice più vicino di
una geometria di riferimento data nella config, se dista al più
`tolerance`; i vertici più lontani restano dove sono. Serve ad allineare
geometrie che dovrebbero condividere vertici e ne differiscono di poco. Il
tipo geometrico non cambia; il risultato deve restare valido.

La conversione di colonna è `extensions2::snap_column`, che il runner
chiama sulla colonna intera.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `reference_wkb` | stringa | obbligatorio | WKB esadecimale di una geometria valida, nel dominio del CRS della colonna | riferimento, nello stesso CRS della colonna: contano solo i suoi vertici |
| `tolerance` | numero | obbligatorio | finito, `>= 0` | distanza massima di aggancio, nelle unità del CRS; `0` aggancia solo i vertici che coincidono |

### Schema

Identico all'ingresso: colonne, tipi, nullabilità, metadati e proprietà del
contratto (`sorted_by`, `row_count`).

### Righe

1:1: il runner chiama la conversione di colonna dei kernel
(`extensions2::snap_column`) sulla colonna intera, con il riferimento letto
una volta in validazione. Una cella nulla resta nulla. Con un riferimento
senza vertici ogni geometria esce invariata ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

### Ordine

Quello d'ingresso (le celle si elaborano in parallelo, con l'ordine
ricostruito per indice).

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `reference_wkb` mancante,
  non esadecimale, malformato o OGC-invalido; `tolerance` mancante,
  negativa o non finita;
- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non è riconoscibile come WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
  `reference_wkb` con dimensioni Z/M o SRID;
- `Crs`: colonna senza CRS risolto o CRS non proiettato; riferimento fuori
  dal dominio del CRS.

In esecuzione, prima del kernel, su tutta la colonna: `InvalidPlan` per
una cella che non è WKB strutturalmente valido, `Crs` per una coordinata
fuori dal dominio di validità del CRS della colonna, `Schema` per una
geometria di un tipo che il contratto d'ingresso non dichiara, quando li
dichiara con un elenco ([Runner, «Operazioni geo»](runner.md#operazioni-geo)).

Poi la conversione di colonna, che rende già `PlenoraError` (vince la
prima cella che fallisce in ordine di riga, senza diagnostica per riga):

- `InvalidPlan`: WKB malformato o OGC-invalido; geometria agganciata non
  più valida (un anello che collassa, un lato che si sovrappone): messaggio
  con prefisso `geo.snap:`;
- `Unsupported`: WKB con dimensioni Z/M o SRID;
- `ResourceLimit`: cella oltre il limite di byte per cella;
- `Internal`: panico nella costruzione dell'R-tree o in `geo`, validazione
  che non conclude.

### Limiti e deviazioni

Nel runner un errore non ha diagnostica per riga e il costo in memoria è
una previsione dalle misure
([Runner, «Limiti dichiarati del runner»](runner.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).
A differenza di `ST_Snap` di PostGIS e dello snap di GEOS, si agganciano
solo vertici a vertici: i vertici non vanno sui lati del riferimento, e i
vertici del riferimento non si inseriscono nei lati della geometria. Se
due vertici del riferimento sono alla stessa distanza, la scelta è
deterministica ma non specificata. Un'uscita invalida è un errore, non una
geometria riparata.

### Precisione

Esatta sulle coordinate: un vertice agganciato prende i bit del vertice di
riferimento, gli altri restano quelli d'ingresso. Solo la decisione
«distanza `<= tolerance`» si calcola in `f64` (`hypot`): su un vertice a
distanza pari a `tolerance` entro l'arrotondamento può cadere da una parte
o dall'altra
([Limiti dichiarati, «Precisione delle operazioni geografiche: 1 cm a terra»](limiti.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

R-tree dei vertici del riferimento costruito una volta, O(r log r); poi
O(log r) per vertice d'ingresso, più la validazione OGC dell'uscita di ogni
riga. Memoria O(r) per l'albero, più le celle d'uscita.

### Esempio

`reference_wkb` è `POINT(10 10)`, `tolerance` 1 cm.

```json
{
  "config": {"reference_wkb": "010100000000000000000024400000000000002440", "tolerance": 0.01},
  "ingressi": [
    {"nome": "confini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,9.995 10)", "LINESTRING(0 0,9 9)", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 10)", "LINESTRING(0 0,9 9)", null]}
  ]}
}
```
