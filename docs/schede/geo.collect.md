### Che cosa fa

Raggruppa le righe per le colonne `group_by` e raccoglie le geometrie di
ogni gruppo in una sola, **senza unione topologica**: punti, linee o
poligoni tutti dello stesso tipo diventano il multi corrispondente, un
gruppo misto una `GeometryCollection`, un gruppo di una sola geometria
resta quella. Le geometrie nulle si saltano. Le colonne che non sono chiavi
spariscono.

Il runner forma i gruppi su tutta la tabella e per ciascuno chiama
`extensions::collect_geometries` con le geometrie del gruppo nell'ordine
delle righe ([README, «Operazioni geo»](../README.md#operazioni-geo)).

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `group_by` | lista di stringhe | obbligatorio | colonne dell'ingresso, non vuota, senza ripetizioni, diversa dalla colonna geometria, di un tipo leggibile come testo | chiavi di gruppo |

### Schema

Prima la colonna geometria, con nome e metadati d'ingresso (la
dichiarazione dei tipi riscritta) e nullable; poi le colonne `group_by`,
nell'ordine della lista, identiche all'ingresso (tipo, nullabilità,
metadati). I metadati di schema restano; nessuna proprietà del contratto
sopravvive. Tipi dichiarati, se l'ingresso li dichiara `exact` o `mixed`
con elenco: quelli d'ingresso, più il multi di `Point`, `LineString` e
`Polygon` presenti, più `GeometryCollection` salvo che l'ingresso dichiari
un solo tipo semplice.

### Righe

Aggregazione: una riga per gruppo, con la geometria raccolta e i valori
chiave della prima riga del gruppo. Due righe stanno nello stesso gruppo
se ogni chiave ha lo stesso tipo e la stessa forma testuale
(`scalar_as_string` dei kernel tabellari), o è nulla in entrambe: un
valore nullo è un valore di gruppo come gli altri, distinto dal testo
vuoto. Un gruppo senza geometrie non nulle dà una geometria nulla; una
tabella vuota non dà righe.

### Ordine

I gruppi escono in ordine lessicografico della chiave testuale del
progetto d'origine (`190c493`): per ogni colonna chiave il tipo, la
presenza e la lunghezza del valore, poi il valore. È un ordine
deterministico, non quello dei valori (`"pari"` prima di `"dispari"`,
perché più corto; `10` dopo `9`; a parità delle chiavi precedenti una
chiave nulla viene prima). Dentro una geometria raccolta i membri
seguono l'ordine delle righe.

### Errori

In validazione (analisi del contratto):

- `InvalidPlan`: config con campi sconosciuti; `group_by` vuota, con nomi
  vuoti, con ripetizioni o con la colonna geometria; una chiave di un tipo
  che non si legge come testo (`validate_text_convertible` dei kernel
  tabellari: tipo e fuso orario);
- `Schema`: una colonna di `group_by` non esiste; l'ingresso non ha
  esattamente una colonna geometria, o la colonna non è riconoscibile come
  WKB;
- `Unsupported`: dimensioni della geometria diverse da `xy`;
- `Crs`: colonna senza CRS o con un'incoerenza CRS non risolta.

In esecuzione, prima del kernel, su ogni cella non nulla ([README,
«Operazioni geo»](../README.md#operazioni-geo)): `InvalidPlan` per un WKB
malformato o con coordinate non finite, `Unsupported` per dimensioni Z/M o
SRID, `Crs` per una coordinata fuori dal dominio di validità del CRS della
colonna, `Schema` per una geometria di un tipo che il contratto
dell'ingresso dichiara con un elenco e che non vi compare. Poi la
decodifica completa con la validazione OGC: `InvalidPlan` per una
geometria non valida, `Internal` se la validazione non conclude.

La lettura delle chiavi come testo fallisce, con un errore esplicito, per
un `Binary` non UTF-8, una data o un istante fuori intervallo, una chiave
di dizionario fuori dal dizionario.

Il calcolo di un gruppo rifiuta una geometria che non supera la validazione
OGC (`ExtensionError::InvalidInput`) e una raccolta che non la supera
(`ExtensionError::InvalidOutput`): per esempio due poligoni che si
sovrappongono non formano un `MultiPolygon` valido. Il runner li traduce
in `InvalidPlan`; una validazione che non conclude è `Internal`.

### Limiti e deviazioni

Nessuna unione: poligoni che si sovrappongono o si toccano lungo un lato
si rifiutano invece di fondersi (per l'unione c'è `geo.dissolve`).
L'ordine dei gruppi è quello della chiave testuale, non quello dei valori.
Nessuna diagnostica per riga: il passo rende il primo errore ([README,
«Limiti dichiarati del runner»](../README.md#limiti-dichiarati-del-runner),
voci «Geo senza diagnostica per riga» e «Modelli di costo geo»).

### Precisione

Esatta: le geometrie si copiano senza calcolo
([README, «Precisione delle operazioni geografiche: 1 cm a terra»](../README.md#precisione-delle-operazioni-geografiche-1-cm-a-terra)).

### Complessità

O(n) sulle righe per la raccolta, più la validazione OGC di ogni geometria
e di ogni raccolta (O(v²) nel caso peggiore sui vertici `v` del gruppo);
memoria O(n) per le geometrie raccolte.

### Esempio

```json
{
  "config": {"group_by": ["zona"]},
  "ingressi": [
    {"nome": "pozzi", "colonne": [
      {"nome": "zona", "tipo": "utf8", "valori": ["a", "a", "b"]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POINT(1 1)", "POINT(2 2)", "POINT(5 5)"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["MULTIPOINT((1 1),(2 2))", "POINT(5 5)"]},
    {"nome": "zona", "tipo": "utf8", "valori": ["a", "b"]}
  ]}
}
```
