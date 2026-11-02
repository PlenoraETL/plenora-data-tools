# Oracolo di composizione della primaria

I tredici JSON in `primary-composition-v1/` sono copie byte per byte da
plenora-contracts #57, commit `0ec02c0b7957a65754dae3fafad353d9971d4c3e`,
cartella `vectors/primary-composition-v1` del checkout `C:/pg/ct-primaria`.
La regola è PRIM-006 di Arrow Primary Geometry 1.0; lo stato atteso e il
canone sono dati dei vettori, non calcolati dal codice sotto prova.
Il test li applica anche invertendo gli ingressi e rimappando la provenienza.
