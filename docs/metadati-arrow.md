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

Dentro il piano gli schemi portano versione e blocco canonico sulle tabelle
con geometrie. I contratti plurali completano anche le identita' pubbliche
per trasportare il riferimento alla primaria; le identita' interne del
planner restano un namespace distinto.

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
  prende mai il numero che un ingresso usava per un altro campo. Il cursore
  è unico per piano e avanza nell'ordine di `outputs`: un ID nuovo non si
  riusa fra uscite. Un ID comune a due uscite identifica un campo conservato
  dallo stesso ingresso, con identità non ambigua.

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

- **Campo nuovo comune a più uscite.**
  *Regola*: le identità nuove sono uniche nel piano, nell'ordine di `outputs`.
  *Ambito*: pubblicazione degli schemi, anche uscite che nominano ingressi.
  *Hazard*: uno stesso campo nuovo presente in due uscite riceve due ID diversi;
  il consumatore non lo ritrova per ID, ma non trova un campo diverso.
  *Rientro*: assegnare gli ID dalla lineage interna, registrando una sola volta
  tutti i campi d'ingresso e allineando `FieldOrigin` e lineage dei metadati.

- **Identità persa nelle collisioni.**
  *Regola*: ARROW-004 chiede di conservare l'identità di un campo
  invariato; DT-ARROW-001 del profilo v2 stabilisce che, in una collisione,
  l'identità si perde (è il comportamento sotto, ora contratto).
  *Ambito*: `pubblica_schema`, uscite di `run`.
  *Hazard*: un campo invariato perde l'identità quando lo stesso numero è
  dichiarato da due ingressi del piano o compare su due campi dell'uscita;
  riceve un'identità nuova, come una colonna derivata. Garanzia indebolita:
  il consumatore non ritrova il campo, ma non ne trova mai un altro. Non è
  un caso raro: piani indipendenti numerano da 0 i campi senza identità,
  quindi due tabelle prodotte da piani indipendenti possono condividere
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
| C1: proprieta' CRS (srid sotto) | conserva | conserva | conserva | elimina/riscrive | uguali conserva; diversi o assenti da un lato rifiuta |
| C1 tipizzata: srid | conserva/completa dal CRS | conserva/completa dal CRS | conserva/completa dal CRS | elimina/riscrive | confronta i valori effettivi dopo il mapping documentato |
| C2: derivata | conserva | solo bbox valido come contenitore | elimina | elimina | elimina |
| C3: provenienza/rappresentazione | conserva | conserva | elimina/riscrive | elimina/riscrive | uguali conserva; diversi elimina |
| C4: tipi/dimensioni | conserva | conserva | riscrive/toglie se cambiano | riscrive/toglie se cambiano | unione coerente o unresolved |

`RigheFiltrate` comprende selezione, ordinamento e ripetizione di righe
senza cambiare le celle (anche join, sjoin e nearest). `orientation`, anche
nella chiave nativa, descrive il verso degli anelli di ciascuna geometria:
e' C3, si conserva con `Rows` e in fusione con dichiarazioni uguali.
`geometry_types` resta C2: [GeoParquet 1.1.0](https://geoparquet.org/releases/v1.1.0/)
richiede i tipi effettivamente presenti, non un sovrainsieme ammissibile;
un sottoinsieme di righe puo' perderne alcuni. Il writer lo ricalcola.
`covering` dipende dalle colonne referenziate e resta invalidato con Rows;
`precision` opaca e chiavi sconosciute non hanno una semantica dimostrata
per cella e restano C2. Il contenitore nativo `column` resta invalidato:
non e' una proprieta' atomica e puo' contenere riepiloghi non piu' veri.

Lo `srid` tipizzato non e' una C1 opaca: su WKB con CRS risolto il
codec completa l'assenza con il codice numerico dell'autorita', solo se
rappresentabile in `i32`. La dichiarazione esplicita prevale. La fusione
confronta questi valori effettivi: GPKG con `srid = 32632` e CSV senza
srid, entrambi WKB con CRS EPSG:32632, sono compatibili in entrambi gli
ordini; valori effettivi diversi si rifiutano. Non si deduce dal primo
ramo: ogni campo e' completato dal proprio CRS e i CRS devono coincidere.
Con CRS mancante o irrisolto non si completa; su EWKB l'assenza resta
assenza (GEO-009). Non si modificano byte WKB ne' flag SRID: GEO-008
continua a rifiutare un flag SRID incorporato sotto WKB sul dato originale.
Il mapping e' quello dell'emissione canonica nel runner: l'API diretta dei
kernel senza contratti non risolve CRS ne' deduce uno srid mancante.

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
| `plenora.geometry.srid` | C1 tipizzata: mapping CRS descritto sopra |
| `plenora.geometry.spatial_semantics` | C1 |
| `plenora.geometry.native.geoparquet.epoch` | C1 |
| `plenora.geometry.native.geoparquet.edges` | C1 |
| `bbox` | C2 |
| `covering` | C2 |
| `geometry_types` | C2 |
| `orientation` | C3 |
| `precision` | C2 |
| `plenora.geometry.native.geoparquet.column` | C2 |
| `plenora.geometry.native.geoparquet.bbox` | C2 |
| `plenora.geometry.native.geoparquet.orientation` | C3 |
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
L'enumerazione `geometry_metadata_policy` verifica le decisioni pure.
La matrice `operazioni_doc::matrice_metadati::catalogo_chiavi_trasporti`
itera invece il catalogo e le 48 chiavi, usando le fixture delle schede:
scrive e rilegge gli ingressi IPC file, IPC stream e GeoParquet, esegue
`Pipeline::validate` e `run`, scrive l'uscita e la rilegge con Arrow pubblico.
Controlla anche il footer GeoParquet, con bbox calcolato indipendentemente
dai dati. Le chiavi brevi sono provate nel campo, nei due contenitori di
campo e nelle colonne del documento `geo` di schema.

Concat, concat_by_name e union_distinct ricevono dichiarazioni uguali,
assenti da un lato e discordanti; un conflitto fuori dal vocabolario
supportato deve produrre un rifiuto esplicito. Join riceve geometria a
sinistra, a destra e su entrambi i lati, per inner e left. Il registro
`tests/riferimenti/operazioni_metadati.txt` di `plenora-io` deve contenere
esattamente il catalogo e concordare con la classificazione della politica:
un'operazione nuova senza classificazione fa fallire il test.

La qualificazione `qualification_multigeometria_catalogo`, nello stesso
modulo e con gli stessi trasporti, fixture e classificazioni, incrocia tutte
le 146 operazioni con una, due e tre geometrie, primaria iniziale,
non iniziale o assente, operando primario o secondario, valori/null/vuoto.
Controlla byte, primaria e semantica per colonna dopo ogni trasporto;
GeoParquet senza primaria deve rifiutare con `Unsupported`. I rifiuti delle
fixture hanno categorie precise: Voronoi senza punti, asserzione di
cardinalit? sul vuoto e transpose con CRS incompatibili. Pivot usa mapping
esplicito; transpose passa dall'adapter pubblico a schema dinamico.
Le mutazioni dell'oracolo perdono una secondaria, scambiano metadati o
cambiano primaria e devono essere rilevate. Il conteggio dei casi ? in
`target/qualificazione-mg5.txt`.

La variante `catalogo_chiavi_trasporti_completo` (`--ignored`) incrocia
anche tutte le sonde delle 48 chiavi con una, due e tre geometrie; il test
di default conserva un sottoinsieme delle combinazioni di queste sonde.

Il report `target/matrice-metadati.md` elenca tutti i rifiuti per fase,
le uscite effettivamente rilette per trasporto e le divergenze. Un errore
interno o IO non e' un rifiuto accettabile. La prova raccoglie tutti gli
esiti prima di fallire; non pretende di coprire ogni configurazione dei
kernel. La mappa `geometry_metadata_policy::types` calcola anche le
dichiarazioni canoniche dipendenti dai tipi d'ingresso (collect, explode,
subdivide); l'oracolo preesistente `kernel_crosscheck` la confronta con i
kernel. La matrice usa tipi d'ingresso ricavati dal WKB, non soltanto una
dichiarazione che ammette tutti i tipi. Le dichiarazioni native dei tipi
sono invalidate da `Effect::Types`.

### Precisione dichiarata e precisione della rappresentazione

`precision` (C2) e `plenora.geometry.precision` (C3) non sono alias.
Il codec legacy legge da `geo` solo `crs`, `dimensions` ed `encoding`:
non interpreta `precision`, che resta una dichiarazione opaca del
produttore, senza unita' o semantica canonica. La politica la tratta
prudenzialmente come riepilogo derivato. La chiave canonica, invece,
passa da `canonical_geometry_precision` e dall'enumerazione
`GeometryPrecision`: descrive la rappresentazione delle coordinate
(`float64`, per esempio), viene preservata dalle operazioni tabellari e
riemessa dal codec dopo le trasformazioni geometriche. Non si deduce
mai una tolleranza geometrica dal valore di nessuna delle due chiavi.

### Struttura dei contenitori geometrici

Questi nomi sono registrati separatamente dalle 48 proprieta': individuano
contenitori o elementi della sintassi GeoParquet, non una classe C1-C4.

| Nome | Ruolo |
| --- | --- |
| `geo` | Contenitore legacy di campo o documento GeoParquet di schema |
| `ARROW:extension:metadata` | Contenitore GeoArrow di campo |
| `version` | Versione del documento GeoParquet, riemessa dal writer |
| `primary_column` | Nome della geometria primaria pubblicata |
| `columns` | Mappa da nomi dei campi alle proprieta' geometriche |
| `plenora.primary_geometry` | Riferimento alla primaria per identità; ricalcolato per provenienza a ogni uscita, escluso dalla fusione dei metadati. |

### Guardia del vocabolario nel sorgente

La guardia `scripts/test_geometry_metadata_guard.py`, eseguita dalla suite
Python delle guardie, usa una scansione lessicale del Rust. Riconosce
commenti annidati, stringhe raw, item `cfg(test)`, accessi con chiave
letterale anche fuori dai codec geometrici, costanti e costruzioni con
`format!`/`concat!`. Una costruzione geometrica non risolta non e' una
chiave implicitamente ammessa. Il vocabolario delle proprieta' e i nomi
strutturali provengono dalla politica e devono comparire nelle tabelle.

`scripts/metadati_literal_contexts.json` registra i contesti dei letterali
non geometrici (configurazioni, valori degli enum, diagnostica e altre
mappe). Non e' una lista globale di nomi permessi: spostare un letterale
in un nuovo accesso richiede una nuova verifica. Il file non ha un comando
di aggiornamento automatico nella guardia. Le prove negative aggiungono
membri JSON sconosciuti, stringhe con escape e chiavi costruite per
verificare che non passino come proprieta' registrate.

La classificazione degli effetti distingue `Values` (tipi invariati) da
`Types` (dichiarazioni dei tipi riscritte). Il secondo effetto elimina
anche le dichiarazioni native C4; il codec ricostruisce quelle canoniche.
`geo.bounds_extractor` legge la geometria e ha effetto `Unchanged`.
Le geometrie nuove di `coverage_validate` e `shared_paths` ereditano le
proprieta' C1, incluse epoca e convenzione dei lati, dalla sorgente.

L'esecuzione tabellare applica la stessa politica dell'analisi allo schema
materializzato prima del confronto esatto con il contratto. Il confronto
resta obbligatorio; non si copia lo schema atteso per nascondere differenze.
Una fusione con precisioni canoniche diverse riemette `float64`; un
contenitore JSON rimasto vuoto viene eliminato in entrambi i percorsi.
La riproiezione sostituisce il CRS nel JSON legacy conservando i membri
C4 sopravvissuti alla politica.
Le concatenazioni fondono per nome le sole sorgenti della colonna:
C1 discordanti sono rifiutate, C2 eliminate, C3 conservate solo se uguali
e C4 unite o dichiarate irrisolte dalla politica.


## Primaria e contratto multigeometria (4.1.0, PR 1-4)

Il core conserva tutte le colonne geometriche con identita' interne distinte.
`active_geometry = None` significa nessuna primaria, anche con una sola
geometria; l'accessor non ripiega sulla prima. Lo schema Arrow usa
`plenora.primary_geometry` secondo Arrow Primary Geometry 1.0 (contracts PR
57): identita' pubblica `plenora.field_id` oppure `none`. Senza chiave,
l'unica geometria e' primaria; con zero o almeno due geometrie non c'e' primaria.
In emissione la chiave compare solo per una geometria senza primaria (`none`)
o per piu' geometrie con primaria (identita' del campo). La risoluzione
avviene prima del completamento delle identita'; riferimenti pendenti,
non geometrici e dichiarazioni malformate sono errori `schema`.

Il selettore comune riceve lo schema originale per risolvere un'identita'
esplicita; gli ID interni del planner non sono selettori pubblici. Il planner
costruisce l'intera mappa delle identita' prima di riscrivere la primaria.
La pubblicazione segue il campo anche quando il suo ID pubblico cambia.
I piani senza passi trasportano tutte le geometrie e i loro byte indipendenti.

Le operazioni tabellari e gli adapter diretti propagano ogni geometria
sopravvissuta con identità, CRS, assi, encoding e nullabilità per colonna.
Non esiste un CRS globale di tabella. Una proiezione che elimina la primaria
non promuove le secondarie. Un join con geometrie sopravvissute da almeno
due ingressi non ha primaria; con geometrie da un solo ingresso ne conserva
lo stato attraverso la corrispondenza dei campi. Append/union/concat
conservano la primaria solo se tutti gli ingressi la fanno confluire nello
stesso campo d'uscita. Le guardie di compatibilità operano sulle coppie di
chiavi e sulle sorgenti che alimentano lo stesso campo, anche a zero righe.
Le geo selezionano per nome con `input_geometry` (unaria), oppure
`left_geometry` e `right_geometry` (binaria). Omissione significa primaria;
assenza della primaria significa errore `Schema`, anche con una sola geometria
esplicitamente senza primaria. La selezione non modifica la primaria.
`geo.reproject` modifica soltanto la colonna scelta: le altre mantengono CRS,
assi, precisione e byte. Le verifiche operative riguardano solo gli operandi;
la validazione strutturale del contratto riguarda comunque tutti i campi.
`from_coords` e `from_wkt` aggiungono una geometria senza promuoverla quando
ne esistono altre; `buffer` e `centroid` trasformano sul posto la selezionata.
Espansioni e join duplicano le passanti secondo la lineage delle righe;
aggregazioni e overlay mantengono lo schema ridotto documentato.
`generate_grid` produce uno schema nuovo; `geometry_diagnostics` elimina solo
la geometria selezionata, senza promuovere quelle superstiti.
`geo.sjoin` conserva le colonne sinistre e aggiunge `__right_index`.

GeoParquet legge e scrive tutte le geometrie e importa la primaria dal footer,
anche non iniziale. IPC file, stream e Arrow SDK conservano identità e stato
della primaria. La scrittura GeoParquet senza primaria rifiuta con `unsupported`. La 4.1.0 adotta Arrow Primary Geometry 1.0: [adozione ed esempi](geometrie-multiple.md).

### Ricerca della classe: costruzioni dei contratti d'uscita

La verifica delle PR 1 e 2 comprende tutte le chiamate a `finish`, `new` e
`rebuild` nelle analisi. La tabella enumera i percorsi; i nomi senza prefisso
sono le funzioni dei moduli `analyze` indicati. Le prove tabellari passano
per `ok`, che verifica che ogni primaria riferisca una geometria presente;
la matrice `every_geo_op_produces_the_expected_contract` verifica anche
l'emissione canonica per tutte le operazioni geo.

| Modulo | Costruzioni verificate | Primaria e prova |
| --- | --- | --- |
| table/cleansing | `analyze_fill_na`, `analyze_replace` | Conservata; prove delle operazioni tramite `ok`. |
| table/columns | `analyze_drop_columns`, `analyze_select_columns`, `analyze_align_schema` | Filtrata sul campo sopravvissuto; prove di selezione e rimozione tramite `ok`. |
| table/columns | `analyze_rename`, `analyze_reorder_columns` | Conservata; prove di rinomina e riordino tramite `ok`. |
| table/helpers | `finish`, `analyze_append_inner`, `derive_geometry` | Costruttore comune, filtro del campo sostituito o primaria derivata; prove append/coalesce tramite `ok`. |
| table/joins | `analyze_join`, `analyze_asof_join`, `analyze_fuzzy_join`, `analyze_cross_join` | Primaria del solo lato che contribuisce geometrie; nessuna se entrambi contribuiscono; `join_primaria_sopravvissuta_oracolo_composizione`, entrambi i lati, canone e sort. |
| table/joins | `analyze_concat`, `analyze_concat_by_name`, `analyze_set_operation` | Consenso di tutti gli ingressi sul campo primario per le fusioni verticali; intersect/except conservano lo stato sinistro. |
| table/quality | `analyze_reconcile`, `analyze_validate_rules` in modalità summary | `None` intenzionale: schema di metriche senza geometrie; prove tramite `ok`. Annotate passa per append. |
| table/reshape | `analyze_melt`, `analyze_pivot`, `analyze_unnest` | Conservata sulle colonne id/indice; nessuna elezione delle derivate. Prove di catalogo e derivazioni plurali. |
| table/reshape | `analyze_table_diff` | Consenso verticale sulle colonne fuse; resoconto dei vecchi WKB in esadecimale. |
| table/sort_agg | `analyze_aggregate` | Filtrata sulle chiavi di raggruppamento; first/last derivano ogni geometria separatamente senza promuoverla. |
| geo/helpers | `with_geometry_types_property`, `rebuild`, `with_schema_metadata` | Conservata; matrice geo. |
| geo/producers e geo/dispatch | `analyze_reproject`, `with_geometry_nullability`, `analyze_subdivide`, `with_nullable_geometry` | Conservata; matrice geo. |
| geo/producers | `analyze_geometry_only` | Filtrata sul campo sopravvissuto; matrice geo, inclusi overlay. |
| geo/producers | `analyze_from_coords`, `analyze_from_wkt` | Primaria nuova solo senza geometrie precedenti; altrimenti conserva lo stato sorgente; matrice geo plurale. |
| geo/producers e geo/quality | `analyze_generate_grid`, `analyze_coverage_rows` | Schema nuovo: `generate_grid` crea una primaria autonoma; `coverage_validate` e `shared_paths` rendono primaria la geometria derivata solo se l'operando era primario, altrimenti primaria assente (`none`); matrice geo plurale. |
| geo/measures | `analyze_diagnostics` | Primaria filtrata sulla geometria rimossa; le altre sopravvivono; matrice geo. |

I percorsi che clonano il contratto preservano la primaria (fra questi
membership join, intersect/except e operazioni sulle righe). Sjoin e nearest
passano per `rebuild` del lato sinistro. Nessun altro `None` cancella una
geometria sopravvissuta nelle analisi attualmente ammesse.

L'oracolo di composizione comprende tutti i 13 vettori contracts #57
`primary-composition-v1`, anche invertendo gli ingressi. Include
`primary-composition-v1/join-one-surviving-primary-canonical.json`: il test
applica lo stato successivo alla proiezione, con l'identità d'uscita attesa;
il test dei vettori copre anche gli ingressi plurali. La
regressione `cross_join_sort_preserva_primaria_esportabile_geoparquet` prova
anche esecuzione e preparazione GeoParquet.

Gli adapter tabellari possono usare ID provvisori per rappresentare una
primaria plurale. Il registro di provenienza pubblica li distingue dagli ID
ricevuti: il confine li elimina prima di assegnare identità con tutti gli
ingressi osservati. Copie self-join e riscritture sul posto conservano la
provenienza pubblica separata dall’identità logica. Il test
`identita_nuove_solo_al_confine_con_tutti_gli_input` verifica che un ID
osservato nell'altro ingresso non venga assegnato a un campo nuovo, anche
con identità ambigue e primaria plurale presente o assente. I
[vincoli dei formati](limiti.md#geometrie-multiple-condizioni-dei-formati)
restano espliciti per le operazioni geo e GeoParquet.

### Più geometrie

Gli effetti seguono l'operando selezionato, indipendentemente dalla primaria.
Le passanti hanno effetto `Unchanged` se la provenienza delle righe è
`Preserved`, altrimenti `Rows`; `sjoin` e `nearest` applicano `Rows` a tutte.
Misure e predicati conservano la precisione. `Values`, `Types` e `Crs`
invalidano la precisione ereditata del solo operando ricodificato.
Le colonne nuove ricevono solo le proprietà CRS dell'operando (`inherit_crs`).
La primaria segue la provenienza, mai l'ordine dei campi. Il `geo` di schema
si elimina quando cambiano nomi geometrici o primaria, e a ogni effetto
diverso da `Unchanged`; il writer ricostruisce i riepiloghi per colonna.

## Celle geometriche derivate e letture testuali

La fusione di celle intatte in `melt`, `pivot` first/last, `coalesce` e
`transpose` conserva il riferimento comune, anche con più geometrie, ma
scrive sempre `plenora.geometry.types_declaration = unresolved` senza `types`
e senza ereditare l'identità del primo campo. Analisi e kernel condividono
questa regola (GD), applicata anche ad `aggregate` first/last nel supporto
multigeometria; `nunique` confronta i byte nativi. Quando una API diretta
emette chiavi canoniche su un ingresso solo GeoArrow, completa anche
`plenora.contract.version = 1` nello schema.

La regola GT rifiuta con `Crs` in validazione qualsiasi colonna geometrica
usata come testo, indipendentemente dai byte WKB, anche su input vuoti o null.
I test diretti `is_null` e `is_not_null` di `table.expression` leggono solo
la bitmap e sono ammessi anche sulle geometrie. Gli usi testuali annidati
restano rifiutati. Le chiavi native restano ammesse. Ambito, hazard e rientro sono nel
[registro dei limiti](limiti.md#geometrie-lette-come-testo).
