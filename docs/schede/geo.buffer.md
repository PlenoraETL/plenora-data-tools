### Che cosa fa

Sostituisce ogni geometria con il suo buffer planare a distanza `distance`,
nelle unità del CRS: l'insieme dei punti che distano al più `distance`
dalla geometria, sempre un `MultiPolygon` (anche vuoto). Con `distance`
negativa il buffer erode: contano solo le parti areali, e punti e linee
spariscono. Con `distance` nulla l'uscita è l'unione delle parti areali
(punti e linee danno un `MultiPolygon` vuoto). Le giunzioni sono sempre
tonde; `cap` sceglie le estremità delle linee.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `distance` | numero | obbligatorio | finito, anche negativo o nullo | distanza del buffer, nelle unità del CRS |
| `cap` | stringa | `round` | `round`, `flat`, `square` | estremità delle linee: arco, taglio netto all'estremo, quadrato che sporge di `distance` |

Con `cap` `flat` i punti non hanno buffer: un ingresso di soli punti dà un
`MultiPolygon` vuoto.

### Schema

La colonna geometria resta al suo posto con lo stesso nome, CRS e
dimensioni (`xy`); le altre colonne, i metadati di schema e le proprietà
del contratto (`sorted_by`, `row_count`) passano invariati. I tipi
dichiarati della colonna diventano `exact` [`MultiPolygon`]: le chiavi
`plenora.geometry.types` e `plenora.geometry.types_declaration` ereditate
si tolgono dal campo.

### Righe

1:1 per contratto. Il kernel (`operations::buffer_with_cap`) lavora su una
geometria alla volta e nessun adapter lo chiama ancora sulle righe: il
trattamento di una cella nulla non è definito da codice eseguito.

### Ordine

L'ordine d'ingresso: l'analisi conserva `sorted_by`.

### Errori

In validazione (analisi del contratto):

- `Schema`: l'ingresso non ha esattamente una colonna geometria, o la
  colonna non si riconosce come WKB (né estensione `geoarrow.wkb` né chiavi
  `plenora.geometry.*`);
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `InvalidPlan`: `distance` assente o non finita, `cap` fuori elenco,
  campi sconosciuti;
- `Crs`: CRS della colonna assente o non risolto, geografico, o proiettato
  senza unità lineare.

In esecuzione, dal kernel, per geometria (`OperationError`, che nessun
codice traduce ancora in `PlenoraError`):

- `InvalidInput`: la geometria non supera la validazione OGC;
- `PrecisionInsufficient`: la griglia di `i_overlay`, sommata al rientro
  delle direzioni intere, supererebbe metà della precisione (con 1 cm:
  `|distance|` oltre circa 5.368 km, o coordinate oltre circa `2^39` m);
- `InvalidOutput`: il buffer prodotto non supera la validazione OGC;
- `ValidazioneNonConclusa`, `CalcoloNonConcluso`: la validazione OGC o il
  calcolo di `geo` e `i_overlay` vanno in panico dentro la barriera (il
  messaggio porta solo la forma del payload);
- `InvalidParameter`: `distance` non finita (l'analisi la rifiuta prima).

### Limiti e deviazioni

Il runner non esegue ancora le operazioni geo
([README, «Che cosa non c'è ancora»](../README.md#che-cosa-non-cè-ancora)).
Il kernel riceve la precisione come argomento esplicito (`Precision`):
nessun codice la ricava ancora dal CRS della colonna con
`Precision::from_crs`. Non ci sono i parametri di GEOS e PostGIS per le
giunzioni (`join`, `mitre_limit`), il numero di segmenti per quarto di
cerchio (qui il passo degli archi viene dalla precisione) e il buffer da un
solo lato. Nessun controllo a posteriori del risultato contro la
definizione esatta ([README, «Limiti dichiarati»](../README.md#limiti-dichiarati),
voce «Nessun controllo a posteriori»).

### Precisione

Entro la precisione `p` (1 cm a terra) fino a `|distance|` = 500 `p`
(5 m con 1 cm); oltre, **deviazione dichiarata**: gli archi sono corde con
freccia al più `max(p / 2, 0.001 |distance|)`, quindi il buffer può stare
verso l'interno lungo gli archi fino allo 0,1% della distanza (10 cm a
100 m), senza errore. Verso l'esterno il buffer resta entro `p / 2` da
quello esatto: la griglia di `i_overlay` (due passaggi, il buffer di `geo`
e l'unione delle parti) si controlla prima del calcolo e, se non basta, si
rifiuta con `PrecisionInsufficient`. Le componenti che la griglia
ridurrebbe a un punto non spariscono: una linea più corta di due passi
della griglia si bufferizza come il suo primo punto, un poligono minuscolo
o più sottile di due passi come il suo anello esterno
([README, «Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra),
voci «Buffer» e «Deviazione: archi del buffer»).

### Complessità

Dominata da `i_overlay` (offset dei contorni e unione, sulla griglia
intera); non c'è una stima asintotica dichiarata. Tempi misurati (stella e
linea da 1.000 vertici, da 1 m a 1 km) nel README, voce «Costo» di
[«Precisione delle operazioni geografiche»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra).
Più la validazione OGC dell'ingresso e dell'uscita
([README, «Validazione OGC»](../README.md#validazione-ogc-la-ricerca-delle-auto-intersezioni-non-è-quella-di-geo-il-verdetto-sì)).

### Esempio

```json
{
  "config": {"distance": 1, "cap": "flat"},
  "ingressi": [
    {"nome": "tratte", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["LINESTRING(0 0,10 0)", "POINT(5 5)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOLYGON(((0 1,0 -1,10 -1,10 1,0 1)))", "MULTIPOLYGON EMPTY"]}
  ]}
}
```
