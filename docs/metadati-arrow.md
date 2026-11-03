# Metadati Arrow

Gli schemi che entrano ed escono dal componente seguono i contratti
pubblici *Arrow Interchange 1.0* e *Arrow Metadata Vocabulary 1.0* di
`plenora-contracts` (commit `3c395a8`, tag `v1.1.0`), con le regole DT-ARROW-001..004 del
profilo data-tools versione 2 per i casi che quei contratti lasciano alle
operazioni: identità nelle collisioni e nelle concatenazioni (sotto,
«Identità dei campi»), chiavi mancanti in ingresso e rifiuti delle operazioni
geo («In ingresso»). I quattro vettori di conformità
del vocabolario sono copiati byte per byte, con provenienza e SHA-256, in
`crates/plenora-io/tests/fixtures/contratti/arrow-v1/`, e
`crates/plenora-io/tests/contratti_arrow.rs` li fa passare dal confine
pubblico: file Arrow IPC, `esegui_da_file` con un piano identità, file
d'uscita riletto. Il codec delle chiavi è
`plenora_core::contract::arrow_metadata`, la conversione fra schema e
contratto e la pubblicazione `plenora_core::contract::arrow_schema`.

## In uscita

Ogni output di `run` porta lo schema pubblicato
(`PipelineValidata::schema_uscita`, deciso in validazione), e con lui ogni
file di `esegui_da_file`:

- `plenora.contract.version = 1` nei metadati di schema, anche su una
  tabella senza geometrie (ARROW-001);
- `plenora.field_id` su ogni campo di primo livello (ARROW-003, ARROW-004,
  sotto);
- sulla colonna geometrica, `ARROW:extension:name = geoarrow.wkb` e il
  blocco canonico sempre completo (vocabolario, sezione 4). Ciò che il
  contratto non sa si dichiara con il valore che dice «non so», mai con
  una pretesa:

| chiave | valore |
| --- | --- |
| `encoding` | quello del contratto; non dichiarato: `wkb`, come la lettura lo completa dal nome d'estensione. Una colonna `ewkb` d'ingresso esce `ewkb`, anche dopo un'operazione geo che la riscrive al suo posto: le celle che un kernel scrive sono WKB ISO senza SRID incorporato, EWKB valido. Una colonna geometrica nuova non eredita l'encoding ed esce `wkb` (i produttori `geo.from_coords`, `geo.from_wkt`, `geo.generate_grid`, e le tabelle di `geo.coverage_validate` e `geo.shared_paths`). Un EWKB con SRID incorporato attraversa intatto le operazioni tabellari e un'operazione geo lo rifiuta (`unsupported`). Prove: `crates/plenora-io/tests/contratti_arrow.rs` |
| `dimensions` | quella del contratto, anche `unknown` |
| `spatial_semantics` | `geometry`: i kernel sono planari, e `geography` si rifiuta in ingresso |
| `precision` | quella ereditata, che attraversa intatta le operazioni tabellari; dopo un'operazione geo, o se nessuno la dichiara, `float64` (ogni kernel geo ricodifica le coordinate in `f64`, e ogni coordinata WKB è un double) |
| `types_declaration`, `types` | la dichiarazione del contratto; non dichiarata: `unresolved` senza elenco |
| `crs_resolution` | `resolved`, `declared_unresolved` o `missing`, dallo stato del CRS |
| `crs_id`, `crs_definition`, `crs_definition_format`, `axis_order`, `srid` | `axis_order` resta `unknown` senza dichiarazione esplicita o provenienza certa; `srid` deriva dall autorita' se rappresentabile in `i32`; con `missing` resta solo un eventuale `srid` ereditato |

Dentro il piano gli schemi restano quelli interni (versione e blocco
canonico solo sulle tabelle con geometrie, nessuna identità assegnata):
versione e identità sono del confine.

## Identità dei campi

L'identità segue la lineage dei metadati di campo: un'operazione che
propaga una colonna, la rinomina (anche i suffissi `_L`/`_R` di `join`) o
la riscrive al suo posto ne clona i metadati, e con loro
`plenora.field_id`; una colonna nuova nasce senza. È la stessa regola del
`FieldId` delle geometrie nel piano (`geo.buffer` riscrive la colonna e ne
tiene l'identità). All'uscita (`pubblica_schema`):

- un'identità portata da un solo campo dello schema, e dichiarata da un
  solo ingresso del piano, resta byte per byte;
- un'identità portata da più campi (una colonna duplicata, un self-join) o
  dichiarata da più di un ingresso si toglie a tutti i campi che la
  portano: i namespace degli ingressi sono indipendenti, e la stessa cifra
  direbbe due campi. L'identità si perde, non passa a un altro campo;
- ogni campo senza identità ne riceve una nuova, in ordine di colonna,
  dalla prima sopra la massima dichiarata da tutti gli ingressi del piano
  (non solo da quelli da cui l'uscita discende): una colonna nuova non
  prende mai il numero che un ingresso usava per un altro campo. Due uscite
  dello stesso piano possono dare lo stesso numero a campi diversi:
  l'unicità vale nello schema.

`concat`, `concat_by_name` e `union_distinct` tengono i metadati di campo
della prima sorgente che contiene la colonna, e con loro la sua identita':
per `concat_by_name` non strict puo' essere anche il secondo ingresso.
Anche la dichiarazione geometrica segue questa sorgente, con nullabilita'
aggiornata. L'identita' degli ingressi successivi per la stessa colonna
non sopravvive.

Stesso piano e stessi schemi d'ingresso, stesse identità
(`crates/plenora-core/tests/identita_dei_campi.rs`,
`crates/plenora-pipeline/tests/identita_campi.rs`).

## In ingresso

`contract_from_arrow_schema` legge lo schema di ogni input del piano (e
`GeoParquet` in lettura e scrittura):

- uno schema senza alcuna chiave `plenora.` non è uno schema Plenora (una
  tabella di pandas, di GDAL, di GeoPandas) e si accetta: le geometrie si
  leggono dall'estensione `geoarrow.wkb` e dal metadato `geo`, come prima.
  ARROW-001 riguarda lo schema Plenora che attraversa il confine;
- con almeno una chiave `plenora.` (anche solo `plenora.field_id`, e
  anche su un figlio di struct, lista o mappa, a qualunque profondità:
  prima si guardavano solo i campi di primo livello)
  `plenora.contract.version` è obbligatoria e vale esattamente `1`: una
  versione decimale maggiore è `Unsupported` (ARROW-002), ogni altro testo
  (`0`, `01`, `1.0`) è `Schema`. Prima `0` e `01` passavano;
- `plenora.field_id`: intero decimale non negativo, unico nello schema
  (`Schema`); oltre `u32::MAX` o su un campo annidato `Unsupported`;
- `srid`: intero decimale con segno a 32 bit (`-1` ammesso; prima solo
  senza segno); una forma malformata e `Schema` (REJ-001). Con `crs_resolution = missing` è ammesso (il vocabolario
  vieta con `missing` identificatore, definizione, formato e assi, non lo
  `srid`; prima si rifiutava) e lo stato resta `missing`: un indizio
  numerico non diventa un CRS (ARROW-007). `declared_unresolved` con il
  solo `srid` invece si rifiuta (`Crs`): il vocabolario vuole un
  identificatore o una definizione, e prima l'uscita dichiarava uno stato
  senza nessuno dei due;
- chiavi `plenora.geometry.*` su un campo senza `ARROW:extension:name =
  geoarrow.wkb` si rifiutano (`Schema`; prima bastavano da sole a
  dichiarare la colonna);
- storage `LargeBinary` di una colonna `geoarrow.wkb`: il runner lo
  converte in `Binary` all'ingresso, come `LargeUtf8` in `Utf8` (oltre i
  2 GiB di un `Binary`, `ResourceLimit`), e l'uscita è `Binary`. Lo
  ammette il vocabolario per entrambi i lati;
- `spatial_semantics = geography`, un `edges` non planare in
  `ARROW:extension:metadata` si rifiutano (`Unsupported`): i kernel sono
  planari. Il `crs` PROJJSON o WKT2 passa dal confronto semantico completo;
  se coesiste con altre dichiarazioni devono risolversi allo stesso CRS.
  Una dichiarazione canonica esplicitamente `declared_unresolved` resta
  irrisolta, conservata nella lineage. `crs_type`, se presente, deve essere
  `projjson` con un oggetto oppure `wkt2:2019` con una stringa WKT2.
  Senza `crs_type` entrambe le forme sono riconosciute; tipi dichiarati
  discordanti e stringhe di altri formati sono rifiutati;
- metadati contraddittori falliscono con categoria `crs` se riguardano il
  CRS (stato, identificatore, definizione e formato, ordine degli assi,
  divergenza dal `geo`), `schema` altrimenti (vocabolario,
  sezione 4). Fino a questo ciclo erano `InvalidPlan`: il cambio di
  categoria è una rottura dichiarata, voluta dal contratto;
- `types_declaration = unresolved` e la dichiarazione assente si leggono
  allo stesso modo (nessuna pretesa sui tipi): il contratto sopravvive al
  giro emissione-lettura.

Un `axis_order` con la prima coordinata nord (`lat_lon`,
`northing_easting`) si accetta: il vettore `resolved-point` lo dichiara
per `EPSG:4326`, e un piano di sole operazioni tabellari lo attraversa
intatto (ARROW-008). Dalla 4.0 le operazioni sensibili all'ordine rifiutano
anche assenza e `unknown` (`crs/validate`). Le operazioni sulle sole tuple
continuano: [censimento completo](limiti.md#censimento-delle-operazioni).

`scrivi_tabella` resta un codec senza perdite: non aggiunge versione né
identità a una tabella qualsiasi, ma rifiuta uno schema con chiavi
`plenora.` senza versione `1` o con identità non valide, invece di
scrivere uno schema Plenora non conforme.

## Versioni del catalogo

Ogni operazione geo prende `contract_analysis_version` + 2 nei due cicli: la sua analisi
passa da `analyze_geo_contract`, che ha una regola di validazione nuova
(rifiuto degli assi scambiati) e cambia lo schema d'uscita inferito (toglie
una `precision` ereditata diversa da `float64`), e la versione dell'analisi
si incrementa per l'una e per l'altra cosa. Semantica, config e kernel
restano. La tabella con le versioni prima del ciclo e l'incremento è
`contratti_arrow_portano_l_incremento_dell_analisi` in
`crates/plenora-core/src/catalog.rs`, e copre per costruzione ogni
operazione geo del catalogo.

Nel ciclo Arrow qui descritto le tabellari non cambiano versione: né la loro analisi né i loro kernel
cambiano. Versione, identità dei campi e blocco geometrico completo li
aggiungono la pubblicazione dello schema e l'emissione canonica, che stanno
sotto il catalogo e valgono per tutte le operazioni allo stesso modo: è un
cambio del contratto di trasporto (Arrow Interchange 1.0), non di
un'operazione.

## Limiti dichiarati

- **Identità persa nelle collisioni.**
  *Regola*: ARROW-004 chiede di conservare l'identità di un campo
  invariato; DT-ARROW-001 del profilo v2 stabilisce che, in una collisione,
  l'identità si perde (è il comportamento sotto, ora contratto).
  *Ambito*: `pubblica_schema`, uscite di `run`.
  *Hazard*: un campo invariato perde l'identità quando lo stesso numero è
  dichiarato da due ingressi del piano o compare su due campi dell'uscita;
  riceve un'identità nuova, come una colonna derivata. Garanzia indebolita:
  il consumatore non ritrova il campo, ma non ne trova mai un altro. Non è
  un caso raro: ogni uscita numera da 0 i campi che non avevano identità,
  quindi due tabelle prodotte da questo runner condividono quasi sempre
  0, 1, 2…, e in un piano che le usa entrambe quasi tutte le identità si
  perdono.
  *Rientro*: un namespace d'identità condiviso fra gli ingressi (per
  esempio identità uniche per sorgente dichiarate dal contratto).
- **Identità per lineage.**
  *Regola*: ARROW-004 rimanda al contratto di ogni operazione la scelta di
  quali campi sono nuovi.
  *Ambito*: tutte le operazioni del catalogo.
  *Hazard*: qui un campo tiene l'identità quando l'operazione ne clona i
  metadati: una colonna riscritta al suo posto (`fill_na`, `replace`,
  `geo.buffer`) è lo stesso campo con valori nuovi, non un campo derivato.
  Le schede non lo ripetono operazione per operazione.
  *Rientro*: una dichiarazione per operazione nel catalogo, se il
  contratto la chiederà.
- **Precisione normalizzata dalle operazioni geo.**
  *Regola*: ARROW-010 vuole dichiarata ogni normalizzazione di metadati.
  *Ambito*: `analyze_geo_contract`, ogni operazione `geo.*`.
  *Hazard*: dopo un'operazione geo `precision` esce `float64` anche se
  l'ingresso dichiarava `float32` o `native`, e anche per le operazioni che
  restituiscono la geometria com'era (misure, predicati: dove la scheda
  dice che la colonna geometria passa invariata, vale salvo questa
  chiave): una `float32`
  ereditata direbbe il falso dopo un kernel che ricodifica in `f64`, e
  distinguere le operazioni che non ricodificano non vale la complessità.
  Si perde l'informazione sulla precisione d'origine; le operazioni
  tabellari la conservano.
  *Rientro*: la precisione nel contratto della colonna, con le operazioni
  che la conservano dichiarate.
- **`encoding` completato dal nome d'estensione.**
  *Regola*: l'encoding emesso è una dichiarazione sui byte.
  *Ambito*: colonne `geoarrow.wkb` senza `encoding` né `geo.encoding`.
  *Hazard*: l'uscita dichiara `wkb` anche per celle che nessun kernel ha
  letto (un piano di sole operazioni tabellari). È la lettura che lo
  standard GeoArrow dà del nome d'estensione e quella che il runner già
  faceva in ingresso, ma un produttore che mettesse EWKB sotto
  `geoarrow.wkb` senza dichiararlo vedrebbe la sua colonna etichettata
  `wkb`; i kernel geo rifiutano comunque lo SRID incorporato.
  *Rientro*: la dichiarazione obbligatoria in ingresso, con il rifiuto delle
  chiavi mancanti (sotto).
- **Chiavi mancanti tollerate in ingresso.**
  *Regola*: il vocabolario (sezione 4) vuole su ogni campo `geoarrow.wkb`
  identità, encoding, dimensionalità, semantica, precisione, dichiarazione
  dei tipi e stato del CRS; DT-ARROW-003 del profilo v2 ammette la lettura
  di un campo che ne omette, con i completamenti sotto.
  *Ambito*: `contract_from_arrow_schema`.
  *Hazard*: uno schema versionato a cui ne manca qualcuna si legge con i
  completamenti di sempre (encoding dall'estensione, dimensionalità
  `unknown`, tipi non dichiarati, stato del CRS dalle rappresentazioni,
  semantica `geometry` come in GeoArrow), e l'uscita le porta tutte. Le
  contraddizioni restano errori.
  *Rientro*: un rifiuto delle chiavi mancanti quando i produttori le
  emetteranno tutte.
- **Storage `binary` in uscita.** Un ingresso `large_binary` esce `binary`
  (sopra): lo storage non è un passaggio senza perdite, i byte delle celle
  sì.

L'adozione di [Arrow Geometry Semantics 1.0](geometry-semantics.md) censisce
le deviazioni residue. In ingresso un `axis_order` assente con CRS viene
completato a `unknown` (DT-ARROW-003): il registro CRS non prova l'ordine
delle coordinate memorizzate. Prima del calcolo il runner controlla buona
forma, SRID, tipi e dimensioni contro il campo ricevuto; gli errori di valore
sono `data_mapping/read` o `crs/read`, senza effetti.

L'audit successivo della 4.0.0 incrementa invece le versioni delle
concatenazioni, delle fusioni geometriche e di `flatten_json`, e quelle di
analisi di finestre e rinomina. I controlli contro nomi duplicati proteggono
anche l'API Rust diretta; tali schemi erano gia' contratti non validi.
Le concatenazioni conservano i metadati geometrici del primo campo solo
se encoding, dimensioni, tipi e precisione concordano fra tutti gli ingressi.


## Dichiarazioni geometriche residue (correzione 4.1.0)

**Classe:** metadati derivati conservati dopo la modifica del fatto che
rappresentano. Sono colpite la **4.0.0 e le versioni precedenti**: in
particolare `geo.reproject` poteva lasciare il campo `crs` di IO-tools al
CRS sorgente, accanto al CRS canonico e al footer GeoParquet di destinazione.
La lettura di quella chiave dava un CRS errato senza errore; IO-tools 7.0.0
rifiuta tale incoerenza. La correzione riguarda IPC file, IPC stream e
GeoParquet, le analisi dei piani e gli adapter Arrow geo diretti.

La pubblicazione dello schema non e' una trasformazione geometrica.
Identita' (`steps: []`), select, filter e sort conservano i metadati delle
colonne geometriche invariate: valori, CRS e tipi non sono riscritti.
Questo conserva anche l'evidenza che IO-tools usa per dichiarare
`crs_coordinate_epoch_not_preserved` e `geometry_orientation_not_preserved`
verso formati incapaci di rappresentarle. La rimozione indiscriminata alla
pubblicazione introdotta durante la PR 145 e' corretta prima del rilascio.

L'allowlist `plenora.geometry.*` e dei contenitori JSON si applica invece
alla geometria operando di una geo, ai produttori e alle colonne derivate
da piu' sorgenti. Le altre colonne trasportate restano invariate. Le chiavi
utente esterne restano, incluse `driver.native`, `utente.*`,
`plenora.field_id` (con le regole d'identita') e `plenora.contract.version`.

| chiavi di campo | invariate | trasformate |
| --- | --- | --- |
| `crs` semplice di IO | conservata e confrontata con le altre rappresentazioni CRS | rimossa da reproject/decisione CRS; conservata se CRS invariato |
| `crs_resolution`, `crs_id`, `crs_definition`, `crs_definition_format`, `axis_order` | conservate | reproject/decisione emettono il target |
| `srid` | conservato come intero indipendente, anche 100001 con EPSG:32632 | reproject elimina la sorgente; mapping documentato: codice numerico di autorita' del target se rappresentabile in i32, altrimenti assente |
| `encoding`, `dimensions`, `spatial_semantics` | conservate | produttori WKB XY; semantica planare; Z/M e archi non planari non ridotti in silenzio |
| `precision` | conservata nelle tabellari | `float64` per l'operando geo, anche nelle API dirette |
| `types_declaration`, `types` | dichiarazione dei tipi ammessi, conservata anche sui sottoinsiemi | riscritta per cambio tipo, altrimenti conservata |
| `ARROW:extension:name` | conservata | `geoarrow.wkb` |
| `geo` di campo | conservato, inclusi epoch e orientation | allowlist `crs`, `dimensions`, `encoding`; CRS riscritto da reproject |
| `ARROW:extension:metadata` | conservato | allowlist `crs`, `crs_type`, `edges`; reproject elimina CRS e formato; edges solo planar |
| `bbox`, `covering`, `geometry_types` | conservati con la semantica sotto | rimossi |
| `plenora.geometry.native.*` e chiavi geometriche non tipizzate | conservate | rimosse, anche se sconosciute |

Inventario dei suffissi sotto `plenora.geometry.native.` di IO-tools 7.0.0:
la colonna invariata conserva **ciascuna** chiave; la colonna trasformata
le elimina tutte, senza reinterpretare i payload opachi.

| suffissi | natura e decisione sulle righe invariate |
| --- | --- |
| `geoparquet.epoch`, `geoparquet.orientation`, `geoparquet.edges` | dichiarazioni per colonna; conservate dopo identita', select, filter e sort |
| `gpkg.geometry_type_name`, `gpkg.z`, `gpkg.m`, `shp.shape_type`, `filegdb.ogr_geometry_type` | tipi/dimensioni per colonna, conservati anche su sottoinsiemi |
| `shp.measure_no_data`, `dxf.geometry_model`, `dxf.z_inference`, `csv.geometry_encoding`, `xlsx.geometry_encoding` | rappresentazione e provenienza per colonna, conservate |
| `geoparquet.bbox` | derivata dalle righe: contenitore valido, non promessa di minimo; conservata su sottoinsiemi e riordinamenti |
| `geoparquet.column` | contenitore misto: CRS, tipo, epoch/orientation per colonna, bbox contenitore e covering con percorsi; conservato sulle colonne invariate |
| `geoparquet.primary_column`, `geoparquet.version`, `geoparquet.compatibilita`, `geoparquet.altre_colonne`, `filegdb.geometry_field_count`, `gpkg.rtree_index` | provenienza/struttura nativa della sorgente, conservata come tale; non promessa di un indice fisico ricostruito nell'Arrow d'uscita |
| chiavi native future | opache, conservate sulla colonna invariata; eliminate sulla trasformata |

`bbox` e' un contenitore, anche dopo filter: puo' non essere minimo, ma non
esclude alcuna geometria rimasta. Sort non cambia neppure gli estremi.
`geometry_types` dichiara tipi ammessi, non frequenze o presenza obbligatoria.
`covering` GeoParquet contiene percorsi a colonne di bounds per riga:
filter/sort muovono insieme geometrie e bounds, quindi resta valido.
Un percorso a una colonna eliminata da select viene rifiutato con `Schema`,
senza cancellazione silenziosa. Nessuna di queste conservazioni sopprime
le dichiarazioni di perdita di epoca o orientamento di IO.

Le fusioni (`concat`, `concat_by_name`, `union_distinct` e colonne derivate)
rifiutano con `Schema` dichiarazioni native diverse, incluse epoche diverse
o presenti da un solo lato. Il confronto delle chiavi native opache e'
esatto; per `geo` e l'estensione confronta i soli membri non interpretati,
cosi' il formato testuale del CRS non sostituisce il confronto semantico.
Una fusione derivata elimina poi i riepiloghi ereditati; la concatenazione
puo' conservare dichiarazioni identiche. I predicati fra ingressi distinti
non richiedono l'uguaglianza dei metadati nativi.

A livello schema i riepiloghi restano nei passaggi tabellari; `geo.columns`
esclude i campi eliminati e `primary_column` passa deterministicamente al
primo nome geometrico rimasto (senza colonne si elimina `geo`). Le uscite
geo eliminano i riepiloghi di schema opachi che non possono ricostruire.
Il writer GeoParquet ricalcola dal contratto e dalle celle CRS, tipi e bbox;
non interpreta i metadati nativi di IO-tools come istruzioni di scrittura.
Il suo supporto delle epoche nel footer resta quello dichiarato in
[File](file.md): questa correzione conserva l'evidenza Arrow per IO-tools,
non aggiunge al codec locale il supporto dei CRS dinamici.

GEO-008 distingue lo SRID numerico dal CRS: nessun confronto fra `srid` e
codice EPSG, neppure nel risolutore del contratto o nelle API geo dirette.
Il mapping codice di autorita' -> SRID si usa solo in assenza di un valore
esplicito e dopo una riproiezione; non converte un ID nativo in un CRS.
Restano i confronti dello SRID fra campi da fondere e fra EWKB e campo.
`crs` semplice si confronta con ID, definizione, `geo` ed estensione:
una discordanza provata e' `Crs`; un confronto non eseguibile propaga
l'errore del risolutore, incluso `CRS_NOT_BUILTIN` per WKT1 non riconosciuta,
senza chiamarlo discordanza e senza valori nel messaggio.

Prove: `crates/plenora-io/tests/metadati_derivati.rs` copre identita', select,
filter e sort, conservazione epoch/orientation/SRID su IPC file, IPC stream
e Parquet, reproject, snap, buffer, explode, produttori e fusioni discordanti.
`contratto_da_schema_arrow.rs` copre SRID GPKG indipendente e confronto WKT1
non eseguibile. `kernel_crosscheck` confronta gli schemi delle API dirette
con l'analisi. Il percorso esterno IO-tools -> GPKG non e' eseguito da questi
test: si verifica che l'evidenza necessaria gli arrivi intatta.
