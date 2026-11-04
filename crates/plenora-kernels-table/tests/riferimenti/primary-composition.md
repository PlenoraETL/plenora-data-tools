# Oracolo di composizione della primaria

I tredici JSON in `primary-composition-v1/` sono copie byte per byte da
plenora-contracts #57, merge `38d68b402389855edbcecb7ce57c31e91282cb4e`,
cartella `vectors/primary-composition-v1` al pin di adozione. Digest e inventario sono in
`primary-composition-provenienza.json`; `scripts/verifica_adozione.py`
confronta ogni copia con i blob Git del pin.
La regola è PRIM-006 di Arrow Primary Geometry 1.0; lo stato atteso e il
canone sono dati dei vettori, non calcolati dal codice sotto prova.
Il test li applica anche invertendo gli ingressi e rimappando la provenienza.
