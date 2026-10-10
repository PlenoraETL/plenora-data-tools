# Changelog

Le modifiche di plenora-data-tools per versione. Le versioni seguono il
versionamento semantico in senso stretto: un piano, una config o un uso
dell'API valido in una versione e rifiutato o interpretato in modo diverso
nella successiva è un cambiamento incompatibile e fa salire la major.

Il corpo di ogni release GitHub riprende la voce della sua versione. Una
voce è «non rilasciata» finché il tag non esiste; prima di pubblicare la
release si sostituisce con la data, e `rilascio.yml` rifiuta una release
la cui versione non ha qui una voce datata.

## [Non rilasciata]

### Contratti

- `plenora-contracts` passa da `1e902df` al tag `v1.1.0`
  (`3c395a8db96df739024e203b22794af340dc8a7f`), sempre profilo data-tools
  versione 2, senza deviazioni. Delle copie dei fixture cambia solo
  `bindings/cli-v1.json`, nelle sezioni di io-tools e rest-tools; le altre
  e i loro SHA-256 restano gli stessi.
- SB-001 e CAP-013 (decisione 0009): nessuna superficie aggiunge un effetto
  oltre il `side_effect` dell'operazione, e le capacità non usano chiavi
  `plenora.`.
- Le sonde runtime RT-016..RT-023 (decisione 0010) non si applicano: il
  trasporto runtime di `data.run` 3 resta all'applicazione, e nessuna
  superficie del componente annuncia la loro richiesta di base.

### Corretto

- **Schema Parquet profondo e footer costoso: errori, non aborti.** Uno
  schema valido annidato per migliaia di livelli esauriva lo stack nella
  lettura (un aborto del processo, senza inviluppo). Ora oltre 64 livelli
  e oltre il tetto di memoria della decodifica del footer
  (`plenora_io::parquet_io::budget_del_footer`) la lettura è
  `ResourceLimit`. Il delta del fork è quello di plenora-IO-tools
  (`patches/parquet-footer-budget.patch`); prove in
  `crates/plenora-io/tests/parquet_footer.rs`.
- **`parquet`: i decoder non si fidano più dei valori del file.** Lunghezze,
  indici ed estremi letti dal file passano da conversioni fallibili e
  aritmetica controllata in tutti i decoder (`patches/parquet-decoder.patch`,
  `vendor/parquet-60.0.0-eof/PROVENANCE.md`). Erano panici, che il confine
  rendeva un errore della barriera, e tre accettazioni silenziose: un
  prefisso `DELTA_BYTE_ARRAY` oltre il valore precedente (un valore
  sbagliato), il resto di una pagina `PLAIN`/`BYTE_STREAM_SPLIT` a
  larghezza fissa, una corsa RLE oltre `u32`. Prove in
  `crates/plenora-io/tests/parquet_decoder.rs`, semi del fuzz in
  `tests/dati/fuzz-decoder/`.
- **`parquet`: larghezza 0 e `BYTE_STREAM_SPLIT` non panicano più.** Un
  `FIXED_LEN_BYTE_ARRAY` di larghezza 0 faceva dividere per zero il
  lettore Arrow, e `BYTE_STREAM_SPLIT` con conteggi dichiarati oltre i byte
  della pagina indicizzava fuori limite; dal confine di lettura erano un
  errore della barriera anti-panico. Ora sono errori del decoder
  (`patches/parquet-flba-bss.patch`, dal fork di plenora-IO-tools, più
  l'`assert!` del decoder `PLAIN` per colonne); prove in
  `crates/plenora-io/tests/parquet_fork.rs`.
- **I fork di `geo`, `wkt` e `parquet` valgono anche per i consumatori.**
  Erano `[patch.crates-io]`, che Cargo applica solo al workspace radice: un
  crate che dipendeva da data-tools per percorso o git (un worker, una
  suite di interoperabilità) riceveva `geo` 0.33.1 con `i_overlay` 4.5.2
  (regioni perse senza errore oltre circa 16.000 segmenti), `wkt` e
  `parquet` di crates.io. Ora sono pacchetti con nome proprio
  (`plenora-geo`, `plenora-wkt`, `plenora-parquet`) dichiarati come
  dipendenze per percorso, con le chiavi d'uso invariate; le patch dei
  nomi sono in `patches/*-nome-proprio.patch`. Il `wkt` di crates.io resta
  nel grafo solo per il lettore WKT di `geozero`, vietato da `clippy.toml`.
  La prova `consumatore_esterno` risolve un crate fuori dal workspace e
  controlla il grafo, anche del workspace e di `fuzz/`. Stessa classe, non
  corretta: il profilo (`overflow-checks = true` in release) vale solo nel
  workspace radice; limite dichiarato in `docs/limiti.md`.
- **Il divieto del lettore WKT di `geozero` copre anche il CSV.**
  `geozero::csv` (feature `with-csv`) legge la colonna geometria con il
  `wkt` di crates.io senza nominare il modulo `wkt`: `clippy.toml` vieta
  `Csv`, `CsvString`, `CsvReader`, `process_csv_geom` e
  `process_csv_features`, e la prova sui sorgenti vieta il modulo `csv` e
  l'import glob di `geozero` in ogni forma di percorso.
- **Pulizia dopo la pubblicazione di `data.run` 3** (ERR-015). Se la
  cartella temporanea non si toglie dopo che ogni output è pubblicato,
  l'errore ha fase `cleanup` (era `finalize`) ed effetto `committed`, con
  ritentativo `never` qualunque sia la causa: il residuo è solo locale e un
  nuovo tentativo pubblicherebbe di nuovo. Prima una causa transitoria
  (`Interrupted`, `TimedOut`, `WouldBlock`, `ResourceBusy`) dava
  `requires_recovery`.

### Documentazione

- `docs/errori.md` non dice più che nessun errore ha effetto `unknown`:
  elenca chi lo produce (panico in un'operazione che scrive file,
  documento di riserva della CLI e dell'SDK Python, temporaneo non
  cancellato, pubblicazione `Ignoto` di `data.run` 3). `docs/cli.md` conta anche `data.run` 3 fra
  le funzioni Rust. `docs/metadati-arrow.md` dichiara, con le prove, che
  una colonna `ewkb` esce `ewkb` (anche riscritta da un'operazione geo),
  che una colonna geometrica nuova esce `wkb` e che un EWKB con SRID
  incorporato si rifiuta nelle operazioni geo.
- Gli attributi di capacità di un'operazione su artefatti (`data.run` 3)
  usano `source`/`sink` e portano `artifact_content_types` e
  `artifact_interchange_contracts`, come il catalogo pubblico. Nessun
  documento delle capacità cambia: `data.run` 3 non vi compare; una prova
  confronta gli attributi di ogni operazione con il catalogo.

## [2.0.0] - 2026-10-06

Adotta il profilo data-tools versione 2 di `plenora-contracts` al commit
`1e902df` (dal `23fed27` della 1.1.0), senza deviazioni.

### Incompatibile

**`null` nelle config dei passi non vale più «omesso»** (#113). In circa
40 campi facoltativi un `null` scritto valeva «omesso» e prendeva il
default. Ora è un piano non valido (`InvalidPlan`, messaggio fisso, in
validazione e nel kernel) in ogni campo, tranne quelli dove `null` ha un
significato proprio. Rifiutati ora:

- ogni campo delle config `geo.*`;
- `geo.reproject`: `accuratezza_accettata_m`, `trasformazioni`,
  `convenzione_wgs84_etrs89`;
- `table.add_row_number`: `partition_column`, `order_column`,
  `ascending`;
- `table.fill_na`: `column`;
- `table.date_extract`: `date_format`;
- `table.string_extract`, `table.string_length`, `table.string_pad`:
  `output_column`;
- `table.asof_join`: `tolerance`;
- `table.lookup`: `default` (prima lasciava invariate le celle senza voce,
  anche a chi chiedeva un null);
- `table.assert_schema`: `fields[].nullable`;
- `table.validate_rules`: `rules[].column` e `rules[].value`;
- i campi facoltativi del payload `plenora-row-diagnostics-v1` letto da
  `plenora-core`.

**`null` non vale più il testo vuoto** (#115).
Fino alla 1.1.0 un `null` in quattro punti delle config si leggeva come
`""`. Così `{"operator": "==", "value": null}` teneva le celle vuote invece
delle celle nulle, e un `null` scelto come uscita diventava una cella `""`.
Ora:

- `value` di `table.filter` e delle condizioni di `table.conditional` è
  obbligatorio e non `null` con ogni operatore tranne `isnull` e `notnull`
  (con questi due resta vietato). `null`, o l'assenza, si rifiuta con
  `InvalidPlan`;
- `result` e `default_value` di `table.conditional` `null` danno la cella
  nulla, come `THEN NULL` di SQL. Il `default_value` assente vale `null`,
  come un `CASE` senza `ELSE`. Nell'uscita `utf8` questo cambia `""` in
  null, e la colonna è nullable se un risultato è `null`. Un `""` accanto a
  risultati numerici rende l'uscita `utf8`, mentre prima dava un `float64`
  nullo;
- i valori `null` di `mapping` di `table.lookup` danno la cella nulla
  invece di `""`;
- una regola di `table.validate_rules` costruita dall'API Rust con
  `value: Some(Value::Null)` si rifiuta con `InvalidPlan`. Prima passava il
  controllo di presenza, e confrontava con `""` o compilava la regex vuota.
  Dal piano JSON `null` era già rifiutato.

Dove `null` resta ammesso, con il significato dichiarato nella scheda:
`result` e `default_value` di `table.conditional` (la cella nulla), `value`
di `table.fill_na` (riempie con null), `default` delle colonne di
`table.align_schema` (la colonna di null) e i valori di `mapping` di
`table.lookup` (la cella nulla).

**Migrazione.**

- Un parametro facoltativo scritto `null` per dire «usa il default» si
  toglie dalla config.
- Un filtro o una condizione con `"value": null`, o senza `value`, diventa
  `"value": ""` se cercava il testo vuoto, oppure `"operator": "isnull"`
  (senza `value`) se cercava le celle nulle.
- Una `table.conditional` che contava sul `""` al posto di `null` scrive
  `""` in `result` o `default_value`. Lo stesso vale per un valore di
  `mapping` di `table.lookup`.

### Corretto

- **Niente testo di terzi nei messaggi d'errore** (#112). Il testo degli
  errori di serde e di regex entrava nel messaggio pubblico di un errore di
  piano citando un valore scritto nel piano (es. «config non valida:
  invalid type: integer `7`…»), contro la regola degli errori senza dati.
  Ora il messaggio tiene solo le parti che vengono dal codice: campo
  mancante o ripetuto, chiave o variante sconosciuta con gli ammessi, o il
  solo genere dell'errore. È chiusa la deviazione dichiarata «Messaggi
  delle config con i valori del piano». Per un tipo o un valore sbagliato
  il messaggio non dice più quale campo.
- **Guardie a 2^64** (#111). In `geo` le guardie dei conteggi da `f64` a
  `u64` usano `>= 2^64`. Con `>` il valore 2^64 passava e il cast
  saturante lo rendeva `u64::MAX`: il numero di celle per asse era
  sbagliato in silenzio.

### Contratti

- `plenora-contracts` passa da `23fed27` a `1e902df` (#116). Fra i due commit non
  cambia nessuno schema, catalogo, binding, vettore né
  `conformance_checks.py`: le copie dei fixture e i loro SHA-256 restano
  gli stessi.

## [1.1.0] - 2026-10-05

Adotta il profilo data-tools versione 2 di `plenora-contracts` al commit
`23fed27` (decisioni 0007 e 0008, errata della 0008), senza deviazioni.

### Aggiunto

- **`data.run` 3** (`plenora_cli::api::esegui_artefatti`, solo superficie
  Rust). Esegue un piano da sorgenti a destinazioni identificate da
  riferimenti opachi, attraverso un `RisolutoreArtefatti`
  dell'applicazione, e restituisce il manifesto
  `plenora-data-execution-result-v3`. Le sorgenti si verificano prima di
  eseguire; si pubblica solo dopo aver codificato tutti gli output, con esiti
  `none`/`partial`/`unknown` (DT-RUN-001..008).
- **CI**: copertura con soglie per superficie e campagne di fuzz
  settimanali e sulle PR che toccano `fuzz/`; immagine manylinux fissata per
  digest.

### Modificato

- **Parquet**: la patch di `parquet` 60.0.0 vendorizzata si estende ai siti
  che allocavano da dimensioni dichiarate. Due file malformati che prima si
  leggevano senza errore ora sono rifiutati.
- **Piano**: un campo facoltativo del piano scritto `null` (`crs`,
  `limits`, un limite) è un piano malformato, non un campo omesso.

## [1.0.0] - 2026-10-04

Primo rilascio stabile: kernel tabellari e geografici su Arrow in Rust puro,
il runner di piani `plenora-data-plan-v1`, la CLI `plenora-data` e l'SDK
Python `plenora-data` / `plenora_data`. Adotta il profilo data-tools
versione 2 di `plenora-contracts` (commit `e7e9d3d`, decisione 0007) senza
deviazioni: `data.catalog` 2, `data.describe` 1, `data.validate` 2,
`data.run` 2, 146 kernel alle versioni semantiche del registro
`data-kernels-v2`.
