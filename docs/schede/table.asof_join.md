### Che cosa fa

Affianca a ogni riga di sinistra al più una riga di destra: quella con il
valore ordinato (`right_on`) più vicino al valore della riga
(`left_on`), nella direzione scelta, dentro lo stesso gruppo (`left_by` con
`right_by`) ed entro la tolleranza. Serve ad abbinare eventi a istanti non
coincidenti, per esempio a ogni ordine l'ultimo prezzo noto.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_on` | stringa | obbligatorio | colonna `int64` o `float64` della sinistra | valore ordinato della riga sinistra |
| `right_on` | stringa | obbligatorio | colonna della destra, dello stesso tipo di `left_on` | valore ordinato dei candidati |
| `left_by` | lista di stringhe | `[]` | colonne della sinistra, senza ripetizioni | gruppo: si abbinano solo righe con gli stessi valori |
| `right_by` | lista di stringhe | `[]` | colonne della destra, tante quante `left_by`, senza ripetizioni | colonne di gruppo del lato destro, nello stesso ordine |
| `direction` | stringa | `backward` | `backward`, `forward`, `nearest` | `backward`: il più grande `<=` del valore; `forward`: il più piccolo `>=`; `nearest`: il più vicino dei due |
| `tolerance` | numero o `null` | `null` | finito, `>= 0`; non `0` con `allow_exact: false` | distanza massima fra i due valori; `null` nessun limite |
| `allow_exact` | booleano | `true` | `true`, `false` | `false`: un candidato con valore uguale non si abbina (`<` e `>` stretti) |

Le colonne `by` di ogni coppia hanno lo stesso tipo Arrow, fra quelli
ammessi come chiave da [`table.join`](#tablejoin), e si confrontano come
lì.

### Schema

Prima tutte le colonne di sinistra, con il loro nome, poi quelle di destra
tranne `right_on` e le `right_by`. Una colonna di destra con lo stesso nome
di una colonna di sinistra diventa `<nome>_R`; le altre tengono il nome.
Tipi e metadati di campo restano quelli d'origine; ogni colonna d'uscita è
nullable. I metadati di schema dei due lati si fondono: una chiave presente
da un lato solo, o con lo stesso valore, resta. L'uscita ammette una sola
colonna geometrica. Le proprietà del contratto (`sorted_by`, `row_count`)
sono quelle della sinistra.

### Righe

Esattamente una riga per ogni riga di sinistra; dove non c'è candidato le
colonne di destra sono nulle. Il candidato:

- sta nello stesso gruppo: tutte le colonne `by` uguali, nessuna nulla
  (senza `by` il gruppo è unico);
- ha un valore `right_on` finito: le righe destre con valore nullo, NaN o
  infinito non sono candidate, e una riga sinistra con uno di questi valori
  resta senza candidato;
- `backward`: il valore più grande `<=` (`<` con `allow_exact: false`); a
  parità di valore la riga destra con l'indice più alto;
- `forward`: il valore più piccolo `>=` (`>`); a parità di valore la riga
  destra con l'indice più basso;
- `nearest`: fra il candidato `backward` e quello `forward` il più vicino;
  a pari distanza quello `backward`;
- con `tolerance`, la distanza `|destra - sinistra|` non la supera.

Una stessa riga di destra può abbinarsi a più righe di sinistra.

### Ordine

Quello della sinistra.

### Errori

In validazione, `InvalidPlan`:

- config con campi sconosciuti, `direction` fuori elenco, `left_on` o
  `right_on` assenti;
- `left_by` e `right_by` di lunghezza diversa, con nomi ripetuti o oltre
  `max_columns`;
- `tolerance` negativa; `tolerance` zero con `allow_exact: false` (nessun
  candidato si abbinerebbe mai);
- colonna assente; `left_on` e `right_on` non dello stesso tipo, o di tipo
  diverso da `int64` e `float64`; colonne `by` di tipi diversi nella
  coppia, o di tipo non ammesso;
- nomi d'uscita che collidono dopo i suffissi, più colonne di
  `max_columns`, due colonne geometriche nell'uscita, metadati di schema
  con la stessa chiave e valori diversi.

In esecuzione, `Schema`:

- un valore `int64` di `left_on` o `right_on` senza un `f64` esatto (oltre
  `2^53` con bit bassi non nulli): si rifiuta invece di arrotondarlo;
- una cella `by` `date32` o `timestamp(ms)` fuori dall'intervallo delle
  date rappresentabili, o un dizionario malformato.

### Limiti e deviazioni

I valori `int64` si confrontano come `f64` esatti. Le distanze di
`tolerance` e di `nearest` sono sottrazioni in `f64`: esatte per valori
`int64` finché la differenza non supera `2^53`, arrotondate come ogni
sottrazione IEEE per i `float64`. Un valore esattamente al bordo della
tolleranza si abbina.

### Complessità

Tempo O(m log m + n log m): i candidati di ogni gruppo si ordinano una
volta, ogni riga di sinistra li cerca per bisezione. Memoria O(m) per i
gruppi, O(n) per l'uscita.

### Esempio

```json
{
  "config": {"left_on": "ts", "right_on": "ts", "tolerance": 3},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "ts", "tipo": "int64", "valori": [1, 5, 10]},
      {"nome": "id", "tipo": "utf8", "valori": ["o1", "o2", "o3"]}
    ]},
    {"nome": "prezzi", "colonne": [
      {"nome": "ts", "tipo": "int64", "valori": [0, 4, 6, 20]},
      {"nome": "prezzo", "tipo": "float64", "valori": [10.0, 11.0, 12.0, 13.0]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "ts", "tipo": "int64", "valori": [1, 5, 10]},
    {"nome": "id", "tipo": "utf8", "valori": ["o1", "o2", "o3"]},
    {"nome": "prezzo", "tipo": "float64", "valori": [10.0, 11.0, null]}
  ]}
}
```
