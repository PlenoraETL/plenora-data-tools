### Che cosa fa

Join per somiglianza fra due colonne di testo, per anagrafiche sporche:
abbina ogni riga di sinistra alle righe di destra la cui chiave ha una
somiglianza (`metric`) almeno pari a `threshold`, e aggiunge in coda la
colonna della somiglianza. Per limitare i confronti si confrontano solo le
coppie dello stesso blocco (`blocking`): stesso prefisso, stesso codice
Soundex o, senza blocking, tutte.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `left_key` | stringa | obbligatorio | colonna `utf8` della sinistra | testo da abbinare |
| `right_key` | stringa | obbligatorio | colonna `utf8` della destra | testo candidato |
| `metric` | stringa | obbligatorio | `jaro_winkler`, `levenshtein`, `jaccard` | misura di somiglianza, in `[0, 1]` |
| `threshold` | numero | obbligatorio | in `(0, 1]` | somiglianza minima di una coppia |
| `blocking` | stringa | obbligatorio | `prefix`, `soundex`, `none` | come si formano i blocchi di candidati |
| `blocking_param` | intero | `2` | `>= 1`; solo con `blocking: prefix` | caratteri del prefisso |
| `how` | stringa | `inner` | `inner`, `left` | `left`: tiene anche le righe sinistre senza coppie |
| `score_column` | stringa | `"score"` | nome non vuoto, al più 1024 byte | nome della colonna della somiglianza |
| `max_candidates` | intero | `50` | `>= 1` | righe massime di un blocco di destra |
| `case_sensitive` | booleano | `false` | `true`, `false` | `false`: i testi si confrontano in minuscolo (Unicode) |

`blocking_param` scritto con `blocking` `soundex` o `none` si rifiuta.

Le metriche, sui testi normalizzati (in minuscolo se non `case_sensitive`)
e sui caratteri Unicode, non sui byte:

- `jaro_winkler`: Jaro più il bonus del prefisso comune (al più 4
  caratteri, peso 0,1), senza soglia minima per il bonus;
- `levenshtein`: `1 - distanza / lunghezza della più lunga`;
- `jaccard`: parole in comune (separate da spazi, come insiemi) su parole
  totali.

Due testi vuoti (per `jaccard`, senza parole) valgono 1.

I blocchi, sul testo normalizzato: `prefix` i primi `blocking_param`
caratteri; `soundex` il codice American Soundex delle sole lettere ASCII
(un testo senza lettere ASCII ha il codice vuoto, e questi testi formano un
blocco); `none` un solo blocco con tutte le righe di destra.

### Schema

Prima tutte le colonne di sinistra, poi tutte quelle di destra, poi la
colonna della somiglianza. `left_key` tiene il nome, le altre colonne di
sinistra prendono il suffisso `_L`, tutte quelle di destra, `right_key`
compresa, `_R`. Tipi e metadati di campo restano quelli d'origine e ogni
colonna degli ingressi è nullable. La colonna `score_column` è `float64`,
nullable solo con `how: left`. I metadati di schema dei due lati si
fondono: una chiave presente da un lato solo, o con lo stesso valore,
resta. L'uscita ammette una sola colonna geometrica. Nessuna proprietà del
contratto sopravvive.

### Righe

Per ogni riga di sinistra, una riga per ogni riga di destra dello stesso
blocco con somiglianza `>= threshold`: tutte le coppie sopra soglia, non
solo la migliore. Le chiavi nulle non si abbinano mai. Con `how: left` una
riga di sinistra senza coppie (chiave nulla compresa) resta una volta, con
le colonne di destra e la somiglianza nulle. Le colonne d'uscita portano i
testi originali, non quelli normalizzati.

### Ordine

Comanda la sinistra: per ogni riga di sinistra, nell'ordine d'ingresso, le
sue coppie nell'ordine delle righe di destra. L'ordine non dipende dal
parallelismo della sonda.

### Errori

In validazione, `InvalidPlan`:

- config con campi sconosciuti, `metric`, `blocking` o `how` fuori
  elenco, parametri obbligatori assenti;
- `threshold` fuori da `(0, 1]`; `blocking_param` zero, o scritto senza
  `blocking: prefix`; `max_candidates` zero; `score_column` vuoto o oltre
  1024 byte;
- `blocking_param`, `max_candidates` o `score_column` `null` espliciti (un
  parametro facoltativo si omette);
- `left_key` o `right_key` assente o non `utf8`;
- nomi d'uscita che collidono (per esempio `score_column` uguale a
  `left_key` o a un nome con suffisso), più colonne di `max_columns`, due
  colonne geometriche nell'uscita, metadati di schema con la stessa chiave
  e valori diversi.

In esecuzione, `ResourceLimit`:

- un blocco di destra con più di `max_candidates` righe, anche se nessuna
  riga di sinistra vi cade: si rifiuta invece di troncare;
- righe d'uscita oltre `max_rows` (nel runner `max_input_rows`).

### Limiti e deviazioni

Con `blocking: none` il blocco è tutta la destra, quindi la destra non può
avere più di `max_candidates` chiavi non nulle (50 senza config). Le
somiglianze sono `f64`; il confronto con `threshold` è sullo stesso valore
che la colonna riporta. Le mappe dei blocchi usano un hash deterministico
senza seme; il blocco rifiutato per `max_candidates` è il più grande, a
parità quello con la chiave minore
([README, «Hash delle chiavi non keyed»](../README.md#hash-delle-chiavi-non-keyed)).

### Complessità

Tempo O(n·b·L) nel caso peggiore, con `b` le righe del blocco (al più
`max_candidates`) e `L` il costo della metrica sulla coppia: quadratico
nella lunghezza dei testi per Jaro-Winkler, a banda per Levenshtein
(le coppie certamente sotto soglia si scartano prima del calcolo), lineare
nelle parole per Jaccard. Sonda in parallelo a blocchi di 256 righe
sinistre. Memoria O(m) per i blocchi e i testi decodificati di destra, più
l'uscita.

### Esempio

```json
{
  "config": {"left_key": "nome", "right_key": "nome", "metric": "levenshtein", "threshold": 0.7, "blocking": "prefix", "how": "left"},
  "ingressi": [
    {"nome": "clienti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2, 3]},
      {"nome": "nome", "tipo": "utf8", "valori": ["Rossetti", "Neri", null]}
    ]},
    {"nome": "anagrafe", "colonne": [
      {"nome": "nome", "tipo": "utf8", "valori": ["rosetti", "nero", "bianchi"]},
      {"nome": "codice", "tipo": "utf8", "valori": ["A1", "A2", "A3"]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id_L", "tipo": "int64", "valori": [1, 2, 3]},
    {"nome": "nome", "tipo": "utf8", "valori": ["Rossetti", "Neri", null]},
    {"nome": "nome_R", "tipo": "utf8", "valori": ["rosetti", "nero", null]},
    {"nome": "codice_R", "tipo": "utf8", "valori": ["A1", "A2", null]},
    {"nome": "score", "tipo": "float64", "valori": [0.875, 0.75, null]}
  ]}
}
```
