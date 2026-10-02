# Documentazione

Le guide descrivono le regole e i limiti correnti del codice. Numeri e
inventari derivano dai sorgenti e stanno nei documenti generati; la
cronologia delle modifiche si consulta in Git.

| documento | contenuto |
| --- | --- |
| [`inventario.md`](inventario.md) | stato generato: crate, operazioni del catalogo, operazioni pubbliche della CLI, numero dei test |
| [`operazioni.md`](operazioni.md) | riferimento generato delle operazioni, una scheda ciascuna (sorgenti in [`schede/`](schede/)) |
| [`limiti.md`](limiti.md) | registro dei limiti dichiarati: precisione geografica di 1 cm, misure geodetiche, validazione OGC, hash e memoria delle chiavi, tipi e formati |
| [`topologia.md`](topologia.md) | `geo.make_valid`, `geo.polygonize` e `geo.split` in Rust puro: che cosa è provato, differenze da GEOS |
| [`errori.md`](errori.md) | categorie d'errore ed effetto di un errore a metà della scrittura |
| [`runner.md`](runner.md) | il piano, validazione, esecuzione, scadenza, diagnostica, operazioni geo, budget di memoria e limiti del runner |
| [`metadati-arrow.md`](metadati-arrow.md) | metadati Arrow in uscita e in ingresso, identità dei campi, versioni del catalogo |
| [`file.md`](file.md) | formati, GeoParquet, scrittura atomica, piano da file a file, confine di lettura |
| [`cli.md`](cli.md) | la CLI `plenora-data`: uscite, codici, comandi, capacità, deviazioni dai contratti |
| [`crs.md`](crs.md) | CRS integrati, i loro limiti e come aggiungere un codice |
| [`riproiezione.md`](riproiezione.md) | `geo.reproject`: catena, metodi, cambi di datum, griglie NTv2, oracolo |

`scripts/genera_inventario.py` genera `inventario.md`;
`crates/plenora-io/tests/operazioni_doc.rs` genera `operazioni.md` dalle
schede. I documenti generati non si modificano a mano.

Le schede in `schede/` non si leggono da sole: entrano in `operazioni.md`, e
i loro link sono relativi a questa cartella, non a `schede/`.
