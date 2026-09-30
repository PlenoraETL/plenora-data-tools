### Che cosa fa

Crea la colonna geometria di una tabella che non ne ha, leggendo il testo
WKT di una colonna `utf8`: ogni cella diventa la geometria WKB
corrispondente, una cella nulla una geometria nulla. Il testo deve essere
WKT 2D senza SRID e descrivere una geometria valida: basta una cella che non
lo è perché l'intera colonna si rifiuti, con la diagnostica delle righe
colpevoli.

La conversione di colonna è `extensions::from_wkt_column`; il runner ne
chiama la variante `from_wkt_column_named`, che nomina la colonna nella
diagnostica.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `wkt_column` | stringa | obbligatorio | colonna `utf8` dell'ingresso | colonna con il testo WKT |
| `output_column` | stringa | `geometry` | nome non vuoto e libero | nome della colonna geometria creata |
| `on_error` | stringa | `null` | `null`, `fail` | accettato per compatibilità: entrambi i valori rifiutano la colonna al primo WKT invalido |
| `crs` | stringa | CRS di piano | identificatore di un CRS integrato | CRS della colonna creata |

`on_error: "null"` non trasforma le celle invalide in null, nonostante il
nome: nessun valore produce un rimedio silenzioso.

### Schema

Le colonne dell'ingresso restano, con i loro metadati; in coda si aggiunge
`output_column`, `binary`, nullable solo se lo è la colonna WKT (una cella
WKT null dà una geometria null), con l'estensione `geoarrow.wkb` e i
metadati `geo` (CRS, dimensioni `xy`, encoding WKB). Il contratto la dichiara
colonna geometria attiva, con i tipi `mixed` dei sette tipi WKB XY (`Point`,
`LineString`, `Polygon` e i multi, `GeometryCollection`). I metadati di
schema e le proprietà del contratto (`sorted_by`, `row_count`) restano.

### Righe

1:1: il runner chiama la conversione di colonna dei kernel
(`extensions::from_wkt_column_named`) sulla colonna intera. Una cella
nulla dà una geometria nulla ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Ordine

Quello d'ingresso (le celle si convertono in parallelo, con l'ordine
ricostruito per indice).

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti o `on_error` fuori elenco;
  `wkt_column` o `output_column` vuoti;
- `Schema`: l'ingresso ha già una colonna geometria; `wkt_column` assente
  o di tipo diverso da `utf8` (`large_utf8` compreso); `output_column` già
  presente;
- `Crs`: né `crs` né un CRS di piano; `crs` non risolvibile (codice fuori
  dalla tabella integrata, definizione WKT o PROJ).

In esecuzione (conversione di colonna):

- `DataMapping` (fase di lettura), con diagnostica per riga completa: una o
  più celle con testo non WKT, prefisso `SRID=`, dimensioni Z/M, carattere
  NUL, testo oltre 64 MiB o geometria OGC-invalida (causa
  `geometry.invalid_wkt`), o geometria il cui WKB supera il limite di byte
  per cella (causa `geometry.encoding_failed`). La diagnostica conta tutte le
  righe colpevoli per causa e ne dà al più 10 come esempio (indice di riga,
  mai il testo); nessuna cella viene pubblicata;
- `Internal`: la validazione OGC di una cella non conclude;
- `Crs`: dopo la conversione, una coordinata di una geometria prodotta
  fuori dal dominio di validità del CRS della colonna creata.

È l'unica operazione geo con diagnostica per riga nel runner; gli indici
seguono la base delle tabellari: righe della sorgente, o dell'ingresso del
passo dopo un passo che cambia le righe
([README, «Diagnostica per riga»](../README.md#diagnostica-per-riga)).

### Limiti e deviazioni

- Il costo in memoria del passo è una previsione dalle misure
  ([README, «Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
  voce «Modelli di costo geo»).
- Solo WKT 2D: EWKT con `SRID=` e WKT con `Z`, `M`, `ZM` si rifiutano.
- `on_error` non ha effetto (sopra).

### Precisione

Esatta: le coordinate sono i `f64` più vicini ai numeri del testo, senza
altro calcolo. Nessun controllo di precisione si applica
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n) nella lunghezza totale del testo, più la validazione OGC di ogni
geometria (O(v²) nel caso peggiore sui suoi `v` vertici); memoria O(n) per
le celle WKB prodotte.

### Esempio

```json
{
  "config": {"wkt_column": "wkt", "crs": "EPSG:4326"},
  "ingressi": [
    {"nome": "luoghi", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "wkt", "tipo": "utf8", "valori": ["POINT(12 41)", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "wkt", "tipo": "utf8", "valori": ["POINT(12 41)", null]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:4326", "valori": ["POINT(12 41)", null]}
  ]}
}
```
