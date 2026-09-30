### Che cosa fa

Estrae un campione pseudocasuale delle righe: `n` righe, oppure una
frazione `fraction` della tabella, eventualmente per strati di una colonna.
Il campione è deterministico: stessi dati e stesso `random_state` danno
sempre le stesse righe nello stesso ordine.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `n` | intero | `100` | intero non negativo; non insieme a `fraction` | righe del campione (senza `fraction`) |
| `fraction` | numero | assente | da 0 a 1 compresi | frazione delle righe; esclude `n` |
| `random_state` | intero | assente (seme fisso `0x9e3779b97f4a7c15`) | intero senza segno a 64 bit; si rifiuta se il campione è sempre vuoto (`n = 0` o `fraction = 0` senza `stratify_column`) | seme del generatore |
| `stratify_column` | stringa | assente | colonna leggibile come testo | colonna degli strati |

Senza strati il campione ha `min(n, righe)` righe, oppure
`round(righe · fraction)` (arrotondamento di `f64`, metà lontano da zero).

Con `stratify_column` le righe si raggruppano per il testo della cella (le
nulle formano uno strato); ogni strato di s righe contribuisce
`floor(n · s / righe)` righe, oppure `floor(s · fraction)`, ma sempre
almeno una e al più s. Il totale può quindi differire da `n`.

`n` e `fraction` insieme si rifiutano: con `fraction` il valore di `n` non
avrebbe effetto. I semi `0` e `1` danno lo stesso campione.

### Schema

Identico all'ingresso: stesse colonne, tipi, nullabilità e metadati.
`sorted_by` non resta. Senza strati `row_count` è noto esattamente se era
noto quello d'ingresso; con strati non è noto.

### Righe

Filtro: ogni riga compare al più una volta.

### Ordine

Non conserva l'ordine d'ingresso. Senza strati le righe escono nell'ordine
del rimescolamento (Fisher–Yates con un generatore xorshift a 64 bit dal
seme). Con strati escono strato per strato, nell'ordine dei testi degli
strati (prima lo strato nullo, poi in ordine lessicografico dei byte), e
dentro lo strato nell'ordine del suo rimescolamento (seme più il numero
d'ordine dello strato).

### Errori

In validazione, `InvalidPlan`:

- `fraction` fuori da 0..=1;
- `stratify_column` assente o non leggibile come testo;
- `n` scritto insieme a `fraction`;
- `random_state` scritto senza `stratify_column` quando il campione è
  sempre vuoto (`n = 0` o `fraction = 0`): nessun seme avrebbe effetto;
- config con campi sconosciuti o `n` negativo.

In esecuzione:

- `Schema`: una cella di `stratify_column` che non si converte in testo;
- `ResourceLimit`: dimensione del campione non rappresentabile, o indice
  di riga oltre `u32::MAX`.

### Limiti e deviazioni

Il generatore non è crittografico e riduce con il modulo, con una
distorsione trascurabile ma non nulla verso gli indici bassi: il campione
serve all'esplorazione, non a garanzie statistiche.

### Complessità

Tempo O(n) per il rimescolamento e la copia delle righe scelte; con strati
O(n log g) per g strati. Memoria O(n) indici più le righe copiate.

### Esempio

Con `random_state` 42 il rimescolamento di quattro righe mette per prime la
quarta e la prima.

```json
{
  "config": {"n": 2, "random_state": 42},
  "ingressi": [
    {"nome": "ordini", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3, 4]},
      {"nome": "importo", "tipo": "float64", "valori": [5.5, 12.0, null, 40.25]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [4, 1]},
    {"nome": "importo", "tipo": "float64", "valori": [40.25, 5.5]}
  ]}
}
```
