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
- **Precisione delle geometrie riscritte.**
  *Regola*: ARROW-010 richiede di dichiarare la normalizzazione.
  *Ambito*: colonne i cui valori geometrici vengono ricodificati.
  *Hazard*: la precisione d'origine diventa `float64`; misure e predicati
  che leggono soltanto la geometria mantengono la precisione ricevuta.
  *Rientro*: rappresentare anche la precisione d'origine nel contratto.
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
Le concatenazioni verificano compatibilita' del framing e delle dimensioni;
i residui seguono la tabella per classi sotto, senza ereditare i riepiloghi
del primo campo.


## Dichiarazioni geometriche residue (correzione 4.1.0)

La tabella unica e' `geometry_metadata_policy::KEYS` in `plenora-core`.
La politica e' per colonna: leggere una geometria non la trasforma.
Le proprieta' tipizzate vengono poi riscritte dal codec del contratto;
encoding, precisione e tipi dell'uscita descrivono i byte prodotti.

| Classe | Invariata | RigheFiltrate | ValoriModificati | CrsCambiato | Fusione |
| --- | --- | --- | --- | --- | --- |
| C1: proprieta' CRS | conserva | conserva | conserva | elimina/riscrive | uguali conserva; diversi o assenti da un lato rifiuta |
| C2: derivata | conserva | solo bbox valido come contenitore | elimina | elimina | elimina |
| C3: provenienza/rappresentazione | conserva | conserva | elimina/riscrive | elimina/riscrive | uguali conserva; diversi elimina |
| C4: tipi/dimensioni | conserva | conserva | riscrive/toglie se cambiano | riscrive/toglie se cambiano | unione coerente o unresolved |

Il bbox non e' necessariamente minimo. Il formato nativo IO usa anche
quattro o sei numeri separati da virgole. Il covering che referenzia
colonne non presenti si elimina, anche nel contenitore opaco
`native.geoparquet.column`: non rende invalido il piano identita'.
Le chiavi sconosciute nel namespace geometrico appartengono a C2.
Le chiavi utente esterne al namespace non cambiano classe implicitamente.

| Chiave | Classe |
| --- | --- |
| `crs` | C1 |
| `epoch` | C1 |
| `edges` | C1 |
| `crs_type` | C1 |
| `axis_order` | C1 |
| `plenora.geometry.crs_resolution` | C1 |
| `plenora.geometry.crs_id` | C1 |
| `plenora.geometry.crs_definition` | C1 |
| `plenora.geometry.crs_definition_format` | C1 |
| `plenora.geometry.axis_order` | C1 |
| `plenora.geometry.srid` | C1 |
| `plenora.geometry.spatial_semantics` | C1 |
| `plenora.geometry.native.geoparquet.epoch` | C1 |
| `plenora.geometry.native.geoparquet.edges` | C1 |
| `bbox` | C2 |
| `covering` | C2 |
| `geometry_types` | C2 |
| `orientation` | C2 |
| `precision` | C2 |
| `plenora.geometry.native.geoparquet.column` | C2 |
| `plenora.geometry.native.geoparquet.bbox` | C2 |
| `plenora.geometry.native.geoparquet.orientation` | C2 |
| `encoding` | C3 |
| `ARROW:extension:name` | C3 |
| `plenora.geometry.encoding` | C3 |
| `plenora.geometry.precision` | C3 |
| `plenora.geometry.native.geoparquet.primary_column` | C3 |
| `plenora.geometry.native.geoparquet.version` | C3 |
| `plenora.geometry.native.geoparquet.compatibilita` | C3 |
| `plenora.geometry.native.geoparquet.altre_colonne` | C3 |
| `plenora.geometry.native.gpkg.rtree_index` | C3 |
| `plenora.geometry.native.shp.measure_no_data` | C3 |
| `plenora.geometry.native.filegdb.geometry_field_count` | C3 |
| `plenora.geometry.native.dxf.geometry_model` | C3 |
| `plenora.geometry.native.dxf.z_inference` | C3 |
| `plenora.geometry.native.csv.geometry_encoding` | C3 |
| `plenora.geometry.native.xlsx.geometry_encoding` | C3 |
| `dimensions` | C4 |
| `types` | C4 |
| `types_declaration` | C4 |
| `plenora.geometry.dimensions` | C4 |
| `plenora.geometry.types` | C4 |
| `plenora.geometry.types_declaration` | C4 |
| `plenora.geometry.native.gpkg.geometry_type_name` | C4 |
| `plenora.geometry.native.gpkg.z` | C4 |
| `plenora.geometry.native.gpkg.m` | C4 |
| `plenora.geometry.native.shp.shape_type` | C4 |
| `plenora.geometry.native.filegdb.ogr_geometry_type` | C4 |

Le chiavi brevi della tabella classificano anche i membri dei contenitori
`geo` di campo e `ARROW:extension:metadata`. I documenti malformati non
vengono riparati silenziosamente. I numeri si verificano prima di una
riscrittura JSON; i contenitori invariati restano byte per byte.

Le regressioni `metadati_derivati` comprendono una fixture prodotta dal
binario IO 7.0.0, con epoca e covering senza colonna Arrow referenziata.
Provenienza: `crates/plenora-io/tests/riferimenti/io700/PROVENANCE.md`.
L'enumerazione `geometry_metadata_policy` visita il vocabolario unico e
ogni voce del catalogo; e' una prova della politica, non una prova completa
di ogni configurazione dei kernel. Le prove di trasporto end-to-end
coprono IPC file, IPC stream e GeoParquet sulle regressioni documentate.
