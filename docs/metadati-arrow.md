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

Scelta: **allowlist nel namespace `plenora.geometry.*` e nei contenitori
JSON geometrici**, non allowlist globale dei metadati utente. Le chiavi
canoniche note seguono le regole sotto; ogni altra chiave del namespace,
compreso l'intero `plenora.geometry.native.*`, si elimina dalle uscite geo
e dagli schemi pubblicati dal piano, incluse le operazioni tabellari.
Una nuova chiave geometrica non diventa quindi automaticamente ereditabile.
Le chiavi utente esterne restano, incluse `driver.native`, `utente.*`,
`plenora.field_id` (con le regole d'identità) e `plenora.contract.version`.
`bbox`, `covering`, `geometry_types` sui campi geometrici e sullo schema
sono trattati come riepiloghi geometrici, mai come annotazioni utente.

Inventario verificato nei produttori IO-tools 7.0.0, in ARROW-VOCABULARY e
ARROW-GEOMETRY-SEMANTICS (che non aggiunge chiavi),
nei percorsi GeoArrow/legacy e nel codec GeoParquet di data. Nella tabella,
le chiavi canoniche abbreviate hanno prefisso `plenora.geometry.`; «geo»
indica ogni operazione del catalogo `geo.*`, anche quelle che conservano
le celle (rimozione conservativa dei riepiloghi non verificabili).

| chiavi di campo | decisione |
| --- | --- |
| `crs` semplice (IO `GEO_CRS_KEY`) | confrontata con tutte le rappresentazioni in ingresso; rimossa da reproject e dalle decisioni CRS; conservata solo se il CRS resta lo stesso |
| `crs_resolution`, `crs_id`, `crs_definition`, `crs_definition_format`, `srid`, `axis_order` | reproject/decisione CRS eliminano la sorgente e il contratto emette il target; le altre operazioni conservano il riferimento verificato; i produttori dichiarano il proprio CRS e gli assi delle coordinate costruite |
| `encoding` | conservato se framing compatibile; i produttori emettono WKB; EWKB con SRID incorporato è rifiutato dalle trasformazioni |
| `dimensions` | le trasformazioni accettano solo XY; i produttori emettono XY; estensioni Z/M non vengono ridotte in silenzio |
| `precision` | uscita geo `float64`, anche negli adapter Arrow diretti; snap non eredita precisioni native |
| `spatial_semantics` | `geometry`; geography/archi non planari rifiutati |
| `types_declaration`, `types` | riscritti dal contratto per ogni cambio tipo; eliminati quando il tipo non è dichiarabile; conservati per reproject e trasformazioni che mantengono il tipo |
| `ARROW:extension:name` | `geoarrow.wkb`; altre estensioni rifiutate |
| `geo` di campo | allowlist dei membri `crs`, `dimensions`, `encoding`; CRS riscritto da reproject; ogni altro membro (bbox, covering, epoch, precisione, assi, tipi, futuri) eliminato nelle uscite geo |
| `ARROW:extension:metadata` | allowlist `crs`, `crs_type`, `edges`; reproject/decisione eliminano `crs` e `crs_type`; `edges` può essere solo planar; gli altri membri eliminati |
| `bbox`, `covering`, `geometry_types` | eliminati nelle uscite geo |
| `plenora.geometry.native.*` e altre chiavi geometriche non tipizzate | eliminate interamente nelle uscite geo, anche se sconosciute |

L'ultima riga comprende **ciascuna** delle seguenti chiavi IO, tutte con
prefisso `plenora.geometry.native.` e tutte eliminate (non reinterpretate):

| suffissi nativi enumerati | decisione |
| --- | --- |
| `geoparquet.primary_column`, `geoparquet.column`, `geoparquet.version`, `geoparquet.compatibilita`, `geoparquet.edges`, `geoparquet.orientation`, `geoparquet.bbox`, `geoparquet.epoch`, `geoparquet.altre_colonne` | rimozione; `column` è un JSON intero, può contenere CRS, encoding, tipi, bbox e covering |
| `gpkg.geometry_type_name`, `gpkg.z`, `gpkg.m`, `gpkg.rtree_index` | rimozione |
| `shp.shape_type`, `shp.measure_no_data` | rimozione |
| `filegdb.ogr_geometry_type`, `filegdb.geometry_field_count` | rimozione |
| `dxf.geometry_model`, `dxf.z_inference` | rimozione |
| `csv.geometry_encoding`, `xlsx.geometry_encoding` | rimozione |

A livello **schema**, `geo` (GeoParquet: `version`, `primary_column`,
`columns.*.{crs,encoding,geometry_types,bbox,covering,edges,orientation,epoch}`),
`bbox`, `covering`, `geometry_types` e `plenora.geometry.*` sono eliminati
nelle uscite geo. Il writer GeoParquet ricostruisce `geo` dal contratto e
dalle celle: CRS PROJJSON, WKB, tipi/dimensioni e bbox calcolati sull'uscita;
non riutilizza covering, epoca o orientamento della sorgente.
Il footer `ARROW:schema` è rigenerato dal writer Parquet; il reader lo
consuma senza trasferirlo nei metadati di schema.
Non esiste un'operazione axis swap: reproject normalizza gli assi del target,
le altre trasformazioni che interpretano gli assi esigono la convenzione
compatibile. `from_coords`, `from_wkt`, `generate_grid` creano metadati nuovi.

In ingresso `crs` semplice prima era ignorata; ora è un identificatore
controllato contro **entrambe** `crs_id` e `crs_definition`, oltre a `geo`,
estensione e SRID. Un conflitto produce `Crs`, senza valori nel messaggio,
anche se lo stato è `declared_unresolved`. Anche i conflitti canonici
numericamente o semanticamente decidibili sono rifiutati (prima potevano
diventare `declared_unresolved`). Uno stato irrisolto senza contraddizione
provata resta irrisolto: il limite del risolutore integrato non autorizza
la scelta di una rappresentazione. La presenza di una rappresentazione
aggiuntiva `crs` semplice o GeoArrow richiede invece un confronto riuscito:
se non è possibile, si rifiuta, anche con `declared_unresolved`.
L'API tipizzata `contract_crs_from_keys`
preserva le proprie regole per chiavi già costruite dal chiamante; il
confine Arrow applica prima i controlli descritti qui.

La scrittura GeoParquet richiede un CRS risolto e assi compatibili: il
precedente ramo `Missing -> null` non poteva superare la guardia degli assi.
Ora rifiuta esplicitamente il CRS assente; la lettura del `null` esterno
resta distinta. La pubblicazione di ogni schema del piano applica la stessa rimozione dei
riepiloghi opachi anche alle uscite tabellari: filtri, riordinamenti e rinomine
possono invalidare bbox e covering senza cambiare una coordinata. La semplice
lettura/scrittura fisica IPC resta distinta dalla validazione del piano:
non risolve il contratto geometrico; il rifiuto semantico avviene in validate.
GeoParquet verifica invece il contratto anche alla lettura/scrittura.

Prove: `crates/plenora-io/tests/metadati_derivati.rs` attraversa i tre
trasporti in ingresso e uscita, riproiezione, snap e cambio tipo con tutte
le chiavi native enumerate, contenitori JSON e annotazioni utente;
`kernel_crosscheck` confronta gli schemi delle API dirette con l'analisi
anche con metadati derivati residui. I casi CRS coprono il testo letterale
`crs` di IO e la definizione discordante nascosta dietro un ID concordante.
