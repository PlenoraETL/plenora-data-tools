### Che cosa fa

Valuta un elenco di regole dichiarative su ogni riga, senza mai fallire sui
dati: una regola violata è un esito, non un errore. Con `output_mode=annotate`
aggiunge a ogni riga se è valida e quali regole ha violato; con
`output_mode=summary` restituisce una riga per regola con il numero di righe
che la violano. La gravità della regola (`error` o `warning`) decide in
quale elenco o conteggio finisce la violazione.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `rules` | lista di oggetti | obbligatorio | da 1 a 4096 regole | le regole, valutate nell'ordine scritto |
| `rules[].name` | stringa | obbligatorio | non vuoto, al più 1024 byte, unico fra le regole | nome della regola nell'uscita |
| `rules[].operator` | stringa | obbligatorio | `eq`, `ne`, `gt`, `ge`, `lt`, `le`, `isnull`, `notnull`, `regex`, `range` | condizione che la cella deve soddisfare |
| `rules[].column` | stringa | obbligatorio | colonna dell'ingresso, di un tipo ammesso dall'operatore | colonna su cui si valuta la regola |
| `rules[].value` | JSON | assente | vedi sotto; `null` vale assente | termine di confronto; una stringa vale il suo testo, un altro valore il suo testo JSON |
| `rules[].severity` | stringa | `error` | `error`, `warning` | gravità della violazione |
| `output_mode` | stringa | `annotate` | `annotate`, `summary` | forma dell'uscita |

Per operatore (colonna numerica: `int64`, `uint64`, `float64`,
`decimal128`, `date32`, `timestamp(ms)`; il testo `utf8` non è numerico
qui):

- `isnull`, `notnull`: qualunque colonna; sulla nullità logica della cella
  (anche la voce nulla di un dizionario). `value` non ammesso;
- `eq`, `ne`: colonna numerica, `utf8`, `bool` o `binary`. Su una colonna
  numerica `value` deve essere un numero (anche in una stringa) e il
  confronto è esatto nel dominio nativo, mai attraverso `f64`; su
  `float64` un `NaN` è uguale a `"NaN"`. Sulle altre si confronta il testo
  della cella con il testo di `value` (`true`, `false` per `bool`);
- `gt`, `ge`, `lt`, `le`: colonna numerica, `value` numerico, confronto
  esatto; una `date32` vale i giorni dall'epoca, un `timestamp(ms)` i
  millisecondi dall'epoca;
- `range`: colonna numerica, `value` il testo `"min,max"` (spazi ai lati
  ignorati), estremi inclusi;
- un numero che la forma esatta non tiene (più di 38 cifre significative,
  `1e-128`, `1e400`) si rifiuta in validazione, non soltanto in
  esecuzione: analisi e kernel lo leggono con lo stesso parse esatto;
- `regex`: colonna `utf8`, `value` una regex del crate `regex` al più
  `max_regex_bytes` byte; ricerca nel testo, senza `^`/`$` basta una parte.

Una cella nulla viola ogni regola tranne `isnull`. Una cella che non si
legge (un `binary` non UTF-8 sotto `eq`/`ne`) viola sia `eq` sia `ne`. Un
`NaN` nella cella o nell'estremo rende falso ogni confronto ordinato e
`range`.

### Schema

`annotate`: le colonne dell'ingresso, poi `_valid` (`bool`), `_errors`
(`utf8`) e `_warnings` (`utf8`), non nullabili. `_valid` è falso se la riga
viola almeno una regola `error`; `_errors` e `_warnings` elencano i nomi
delle regole violate separati da `;`, nell'ordine delle regole, e sono vuoti
se non ce ne sono. Una colonna d'ingresso con lo stesso nome si sostituisce
nella sua posizione. Metadati di schema e numero di righe si conservano;
l'ordinamento dichiarato (`sorted_by`) resta se nessuna colonna è stata
sostituita.

`summary`: una tabella nuova con `name` (`utf8`), `errors` (`int64`) e
`warnings` (`int64`), non nullabili; nessuna colonna e nessun metadato di
schema dell'ingresso. Per una regola `error` conta `errors`, per una
`warning` conta `warnings`, l'altro resta 0.

### Righe

`annotate`: 1:1, una riga d'uscita per riga d'ingresso. `summary`: una riga
per regola, anche con un ingresso vuoto (conteggi a 0): le righe le fissa
la config, e il catalogo esenta l'operazione dal fattore di espansione.

### Ordine

`annotate`: l'ordine d'ingresso. `summary`: l'ordine delle regole in `rules`.

### Errori

In validazione, `InvalidPlan`:

- `rules` vuoto o oltre 4096 regole; nome vuoto, oltre 1024 byte o
  ripetuto; regola senza `column`, o colonna assente;
- `value` mancante per un operatore che lo usa, o presente per `isnull` e
  `notnull`;
- tipo della colonna non ammesso dall'operatore; `value` non numerico dove
  serve un numero; `range` senza virgola o con un estremo non numerico;
  regex oltre `max_regex_bytes` o non compilabile;
- config con campi sconosciuti o valori fuori elenco.

In esecuzione: nessuno che dipenda dai valori.

### Limiti e deviazioni

Un nome di regola può contenere `;`: in `_errors` e `_warnings` l'elenco
non si separa più senza ambiguità. Il contratto di `summary` non dichiara il
numero di righe, anche se è noto. Il runner misura l'espansione di `summary`
come per ogni unaria, righe d'uscita su righe d'ingresso, benché l'uscita
dipenda dalle regole: un ingresso vuoto la fa sempre fallire.

### Complessità

Tempo O(n·r) su righe e regole; memoria O(n) per le colonne aggiunte in
`annotate`, O(r) in `summary`.

### Esempio

```json
{
  "config": {"rules": [
    {"name": "importo_positivo", "operator": "gt", "column": "importo", "value": 0},
    {"name": "codice_formato", "operator": "regex", "column": "codice", "value": "^[A-Z]{3}$", "severity": "warning"}
  ]},
  "ingressi": [
    {"nome": "movimenti", "colonne": [
      {"nome": "codice", "tipo": "utf8", "valori": ["ABC", "ab1", null]},
      {"nome": "importo", "tipo": "int64", "valori": [10, -5, 7]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "codice", "tipo": "utf8", "valori": ["ABC", "ab1", null]},
    {"nome": "importo", "tipo": "int64", "valori": [10, -5, 7]},
    {"nome": "_valid", "tipo": "bool", "valori": [true, false, true]},
    {"nome": "_errors", "tipo": "utf8", "valori": ["", "importo_positivo", ""]},
    {"nome": "_warnings", "tipo": "utf8", "valori": ["", "codice_formato", "codice_formato"]}
  ]}
}
```
