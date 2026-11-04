# Geometrie multiple

Dalla 4.1.0 ogni tabella pu? portare pi? geometrie indipendenti: CRS, assi,
encoding e null appartengono alla singola colonna. Non esiste un CRS globale
che rietichetti le passanti. Il limite `data-una-geometria` ? rimosso.

## Adozione

Il repository adotta `plenora-arrow-primary-geometry-v1`
([ARROW-PRIMARY-GEOMETRY-1.0](https://github.com/PlenoraETL/plenora-contracts/blob/38d68b402389855edbcecb7ce57c31e91282cb4e/specs/data/ARROW-PRIMARY-GEOMETRY-1.0.md)),
PRIM-001..009, al pin `38d68b402389855edbcecb7ce57c31e91282cb4e` di contracts
(merge #57). Il manifesto sorgente ? `crates/plenora-cli/adozione.json`.
I tredici vettori `primary-composition-v1` sono copie esatte verificate da
`scripts/verifica_adozione.py` e applicate dai test della provenienza e
`integrazione_primary_composition`. Non si rivendica l'esecuzione delle
catene fra componenti: la suite interop richiede la VM separata.

## Primaria e selettori

La chiave di schema `plenora.primary_geometry` contiene l'identit? pubblica
`plenora.field_id`, non il nome o la posizione della colonna, oppure `none`.
Senza chiave, una sola geometria ? primaria; con pi? geometrie nessuna lo ?.
Per scegliere esplicitamente la primaria, il produttore imposta questa chiave
sullo schema Arrow ricevuto dal piano. Rinomina e riordino seguono l'identit?;
eliminare la primaria non promuove una secondaria. Join con geometrie da
entrambi i lati produce assenza di primaria. Append e union la conservano
solo se le primarie di tutti gli ingressi confluiscono nella stessa uscita.

`input_geometry` sceglie per nome l'operando delle unarie; `left_geometry` e
`right_geometry` scelgono quelli delle binarie. Omettere un selettore usa la
primaria dell'ingresso corrispondente; senza primaria si riceve `Schema`.
La selezione di una secondaria non cambia la primaria. Una riproiezione
modifica solo la colonna selezionata; per riproiettarne due servono due passi.

## Esempi eseguiti

I blocchi seguenti sono fixture leggibili del test di documentazione
`guida_geometrie_multiple`: `piano` ? il JSON da passare al runner;
`ingressi` descrive le tabelle Arrow da fornire e `attesa` l'oracolo.
La fixture assegna `plenora.field_id` alle colonne nell'ordine dello schema
e imposta la chiave di primaria sull'ID di `posizione`.
La primaria `posizione` ? fisicamente dopo `traccia`; si misura la secondaria.
`is_null` e `is_not_null` possono leggere direttamente una geometria, usando
solo la bitmap: non si pu? invece trattare il WKB come testo.

```json
{
  "piano": {
    "version": 1,
    "inputs": [
      "t"
    ],
    "steps": [
      {
        "out": "a",
        "op": "geo.area",
        "in": [
          "t"
        ],
        "config": {
          "input_geometry": "traccia"
        }
      },
      {
        "out": "b",
        "op": "table.expression",
        "in": [
          "a"
        ],
        "config": {
          "output_column": "assente",
          "expression": {
            "kind": "unary",
            "op": "is_null",
            "value": {
              "kind": "column",
              "name": "traccia"
            }
          }
        }
      }
    ],
    "outputs": [
      "b"
    ]
  },
  "ingressi": [
    {
      "nome": "t",
      "colonne": [
        {
          "nome": "traccia",
          "tipo": "geometry",
          "crs": "EPSG:3857",
          "valori": [
            "POINT(1 2)",
            null
          ]
        },
        {
          "nome": "posizione",
          "tipo": "geometry",
          "crs": "EPSG:4326",
          "valori": [
            "POINT(12 45)",
            "POINT(13 46)"
          ]
        }
      ]
    }
  ],
  "primaria": "posizione",
  "attesa": {
    "primaria": "posizione",
    "geometrie": 2,
    "colonna": "assente",
    "valori": [
      false,
      true
    ]
  }
}
```

Una binaria pu? usare due operandi secondari dello stesso CRS. `sjoin`
conserva le colonne sinistre e aggiunge `__right_index`.

```json
{
  "piano": {
    "version": 1,
    "inputs": [
      "t"
    ],
    "steps": [
      {
        "out": "b",
        "op": "geo.sjoin",
        "in": [
          "t",
          "t"
        ],
        "config": {
          "left_geometry": "traccia",
          "right_geometry": "traccia",
          "predicate": "intersects"
        }
      }
    ],
    "outputs": [
      "b"
    ]
  },
  "ingressi": [
    {
      "nome": "t",
      "colonne": [
        {
          "nome": "traccia",
          "tipo": "geometry",
          "crs": "EPSG:3857",
          "valori": [
            "POINT(1 2)",
            null
          ]
        },
        {
          "nome": "posizione",
          "tipo": "geometry",
          "crs": "EPSG:4326",
          "valori": [
            "POINT(12 45)",
            "POINT(13 46)"
          ]
        }
      ]
    }
  ],
  "primaria": "posizione",
  "attesa": {
    "primaria": "posizione",
    "geometrie": 2,
    "righe": 1
  }
}
```

La projection elimina la primaria: la sola geometria rimasta ? esplicitamente
senza primaria e attraversa IPC senza promozione.

```json
{
  "piano": {
    "version": 1,
    "inputs": [
      "t"
    ],
    "steps": [
      {
        "out": "b",
        "op": "table.select_columns",
        "in": [
          "t"
        ],
        "config": {
          "columns": [
            "traccia"
          ]
        }
      }
    ],
    "outputs": [
      "b"
    ]
  },
  "ingressi": [
    {
      "nome": "t",
      "colonne": [
        {
          "nome": "traccia",
          "tipo": "geometry",
          "crs": "EPSG:3857",
          "valori": [
            "POINT(1 2)",
            null
          ]
        },
        {
          "nome": "posizione",
          "tipo": "geometry",
          "crs": "EPSG:4326",
          "valori": [
            "POINT(12 45)",
            "POINT(13 46)"
          ]
        }
      ]
    }
  ],
  "primaria": "posizione",
  "attesa": {
    "primaria": null,
    "geometrie": 1,
    "righe": 2
  }
}
```

## GeoParquet multicolonna

IPC file e stream conservano tutte le geometrie e anche l'assenza della
primaria. GeoParquet scrive ogni geometria in `geo.columns`, con il suo CRS,
e il nome della primaria in `geo.primary_column`, indipendente dall'ordine
fisico. Senza primaria la scrittura rifiuta con `Unsupported`; non scrive
soltanto la prima geometria. Gli esempi sopra vengono riletti dai tre
trasporti: l'ultimo verifica proprio il rifiuto GeoParquet previsto.
Le colonne passanti mantengono i byte WKB; i metadati del file seguono le
[normalizzazioni GeoParquet](file.md#limiti-dichiarati).
