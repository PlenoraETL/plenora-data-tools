### Che cosa fa

Aggiunge una colonna `float64` con l'area planare, senza segno, della
geometria di ogni riga, nelle unità del CRS al quadrato.

### Parametri

| parametro | tipo | default | valori ammessi | significato |
| --- | --- | --- | --- | --- |
| `output_column` | stringa | `area` | nome di colonna valido e libero | colonna aggiunta |

### Schema

Aggiunge in coda `output_column`, `float64` nullable.

### Righe

1:1.

### Ordine

Quello d'ingresso.

### Errori

Provvisorio.

### Limiti e deviazioni

Provvisorio.

### Precisione

Provvisorio.

### Complessità

O(n).

### Esempio

```json
{
  "config": {},
  "ingressi": [
    {"nome": "lotti", "colonne": [
      {"nome": "id", "tipo": "int64", "valori": [1, 2]},
      {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,10 10,0 10,0 0))", null]}
    ]}
  ],
  "uscita": {"colonne": [
    {"nome": "id", "tipo": "int64", "valori": [1, 2]},
    {"nome": "geometry", "tipo": "geometry", "crs": "EPSG:3857", "valori": ["POLYGON((0 0,10 0,10 10,0 10,0 0))", null]},
    {"nome": "area", "tipo": "float64", "valori": [100.0, null]}
  ]}
}
```
