# File

`plenora-io` legge e scrive le tabelle del runner. Ogni tabella è un solo
`RecordBatch` in memoria (decisione del maintainer: niente streaming, dati
tipici sotto i 10 milioni di righe).

```rust
let tabella = leggi_tabella(Path::new("ordini.parquet"), None, u64::MAX)?;
scrivi_tabella(&tabella, Path::new("ordini.arrow"), &OpzioniScrittura::default())?;
let report = esegui_da_file(&piano, &ingressi, &uscite, &OpzioniScrittura::default())?;
```

## Formati

| formato | estensioni | lettura | scrittura |
| --- | --- | --- | --- |
| Arrow IPC | `.arrow`, `.feather`, `.ipc` | file (Feather v2) o stream, riconosciuti dal contenuto; tutti i blocchi ricomposti in uno | file, a blocchi di circa 8 MiB, senza compressione |
| Arrow IPC stream | `.arrows` (`Formato::ArrowIpcStream`) | come sopra (il contenuto decide) | stream con il marcatore di fine, stessi blocchi, senza compressione |
| Parquet | `.parquet` | un batch grande quanto il file, anche con più row group | proprietà fisse, `ZSTD` livello 3 (o `SNAPPY`, o nessuna) |
| GeoParquet 1.1 | `.parquet` | quando c'è il metadato di file `geo` | quando lo schema ha una colonna geometrica |

Il formato viene dall'estensione (senza distinzione di maiuscole) o da
`Formato` esplicito; un'estensione diversa è `Unsupported`, mai un formato
indovinato. Feather v1 si rifiuta. Fino a questo ciclo `.arrows` si
scriveva in formato file: ora è lo stream (il nome che Arrow dà agli
stream), e un `.arrows` scritto prima si rilegge comunque, perché la
lettura riconosce il formato dal contenuto.

**Arrow IPC** conserva schema, metadati di schema e di campo, tipi e bit
(NaN, `-0.0`) esattamente. **Parquet** anche, per i tipi provati in
`crates/plenora-io/tests/round_trip.rs` (interi con e senza segno, float con
NaN e `-0.0`, booleani, `Utf8`, `LargeUtf8`, `Utf8View`, binari, dizionari,
date, timestamp con fuso, ore, durate, intervalli anno-mese e giorno-ora,
decimali 32/64/128/256, liste, `LargeList`, `FixedSizeList`, strutture,
mappe, `Null`, tabelle vuote): lo schema Arrow incorporato (`ARROW:schema`)
deve coincidere campo per campo con quello che `parquet` applica, altrimenti
la lettura si rifiuta (`Schema`), perché `parquet` ricadrebbe in silenzio sul
tipo Parquet. Senza schema incorporato (file di scrittori non Arrow) vale il
tipo che `parquet` deduce. Si rifiutano anche, prima di decodificare: le
colonne `INT96` (timestamp legacy di Impala e Spark, che `parquet` converte
con aritmetica che avvolge), una chiave ripetuta nei metadati chiave-valore
del file (`parquet` terrebbe l'ultima) e una chiave del file che contraddice
i metadati dello schema incorporato. In scrittura ogni decimale deve stare
nella precisione del suo tipo, a ogni profondità: `parquet` restringe il
valore alla larghezza fisica e un decimale fuori precisione tornerebbe un
altro numero.

**Scrittura deterministica.** `created_by` costante (`parquet-rs version
60.0.0`), pagine formato 1.0, row group di al più 1 048 576 righe,
statistiche di pagina, niente bloom filter, metadati JSON con chiavi in
ordine: la stessa tabella dà gli stessi byte, in Parquet e in Arrow IPC
(provato). I byte Parquet dipendono dalla versione di `parquet`: da 60.0.0
cambiano `created_by`, l'ordine dichiarato delle colonne float
(`IEEE_754_TOTAL_ORDER` al posto di `TYPE_DEFINED_ORDER`, PARQUET-2249) e
le statistiche float, che portano `nan_count` anche negli indici di
pagina; pagine e valori restano gli stessi byte. Un lettore che non
conosce quell'ordine ignora il minimo e il massimo delle colonne float
(pyarrow 25 li riporta assenti), non i valori. Dopo ogni scrittura si rilegge il footer: per Parquet lo schema
incorporato deve essere quello scritto e applicarsi senza cambiare, per
Arrow IPC lo schema del file deve essere quello della tabella.

## GeoParquet

In lettura il metadato di file `geo` è l'autorità:

| chiave | lettura |
| --- | --- |
| `version` | `1.0.0` o `1.1.0`, altrimenti `Unsupported` |
| `primary_column` | deve essere fra le `columns` e la prima colonna geometrica dello schema |
| `encoding` | solo `WKB`; le codifiche GeoArrow native (`point`, …) sono `Unsupported` |
| `geometry_types` | tipi dichiarati (`exact`) e dimensionalità: tutti ` Z` → `xyz`, nessuno → `xy`, misti o elenco vuoto → `unknown`; ogni cella si verifica, un tipo non dichiarato è `DataMapping` |
| `crs` | assente → `OGC:CRS84`; `null` → CRS mancante; PROJJSON → identificatore integrato per `id` (sotto) |
| `edges` | assente o `planar`; `spherical` è `Unsupported` |
| `epoch` | `Unsupported` (il contratto non ha epoche) |
| `orientation`, `bbox`, `covering` | validati nella forma e lasciati cadere |
| altre chiavi di colonna | `Unsupported` |
| altre chiavi di primo livello | ignorate, come chiede la specifica |

Ogni colonna diventa il campo che il contratto accetta: `Binary`
(`LargeBinary` si converte), `ARROW:extension:name = geoarrow.wkb`,
metadato di campo `geo` con `crs`, `encoding` e `dimensions` (solo se
`geometry_types` la decide: con un elenco vuoto resta quella delle chiavi
canoniche del campo, o `unknown`); poi
`contract_from_arrow_schema` e `arrow_schema_from_contract` aggiungono il
blocco canonico `plenora.geometry.*`, completo, e `plenora.contract.version`,
e rifiutano chiavi canoniche già presenti in conflitto (categoria `schema`,
o `crs` per le chiavi del CRS: [«Metadati Arrow»](metadati-arrow.md#metadati-arrow)). Il
metadato `geo` di schema si toglie, `ARROW:extension:metadata` si
sostituisce. Le identità dei campi (`plenora.field_id`) le assegna il
runner all'uscita, non la lettura.

**CRS.** Un PROJJSON si riconduce alla tabella integrata solo per il suo
`id` (o un `ids` di un elemento): `EPSG` con codice numerico → `EPSG:<n>`,
`OGC`/`CRS84` → `OGC:CRS84`; il `type` del documento deve essere quello del
CRS integrato. Ogni altra cosa (senza `id`, altra autorità, codice fuori
tabella, `BoundCRS`, CRS 3D) è `CRS_NOT_BUILTIN`, mai un CRS indovinato. Le
coordinate WKB di GeoParquet sono sempre x = est/longitudine, come l'ordine
GIS normalizzato del contratto: `EPSG:4326` si legge `lon_lat`.

In scrittura, dal contratto: `version` `1.1.0`, `primary_column` la
geometria attiva, `encoding` `WKB`, `crs` il PROJJSON **completo** del CRS
integrato (`crates/plenora-io/data/projjson_integrati.json`, generato con
PROJ 9.5.1 da `scripts/genera_projjson_integrati.py` con la terna di
`genera_crs_integrati.py`; un test verifica che copra esattamente i CRS
integrati) o `null` per un CRS mancante, `bbox` il riquadro XY di tutte le
coordinate quando le geometrie sono 2D. `geometry_types` porta insieme tipi
e dimensionalità, e si scrive solo ciò che il contratto decide, così la
rilettura ridà lo stesso contratto:

| contratto | `geometry_types` |
| --- | --- |
| dimensionalità `xy`/`xyz`, tipi dichiarati con elenco | l'elenco dichiarato (anche se i dati ne usano una parte), con ` Z` per `xyz` |
| dimensionalità `xy`/`xyz`, tipi non dichiarati (anche `unresolved`, che si legge come non dichiarati) | i tipi trovati nei dati (la rilettura li dichiara `exact`) |
| dimensionalità `unknown`, o dichiarazione `mixed` senza elenco | `[]` |

Tipi e dimensionalità dei dati si verificano comunque contro il contratto. Il `geo` di campo non entra nello schema incorporato: lo porta il
metadato di file. Si rifiutano: EWKB, coordinate M, curve, un ordine degli
assi dichiarato diverso da `lon_lat`/`easting_northing`, un CRS dichiarato
ma non risolto, geometrie di un tipo o di una dimensionalità diversi dal
contratto.

La camminata delle celle (`plenora_io::wkb`) è una validazione strutturale:
WKB ISO dei sette tipi, 2D o 3D, byte order per geometria, conteggi limitati
dai byte rimasti, figli coerenti con la multi-geometria, profondità 64,
nessun byte in eccesso, coordinate finite (il punto vuoto, tutto NaN, è
ammesso e resta fuori dal riquadro). Non verifica la chiusura degli anelli e
ammette anelli vuoti: la validità geometrica resta ai kernel. Un proptest la confronta con la
codifica dei kernel e con le coordinate di `geo`.

Fixture: `crates/plenora-io/tests/dati/` contiene file scritti da pyarrow
(Parquet C++): due GeoParquet nella forma di GeoPandas, con il PROJJSON di
PROJ, uno con timestamp `INT96` e uno con i checksum di pagina
(`scripts/genera_fixture_geoparquet.py`).

## Scrittura atomica

Il contenuto va in un temporaneo `.plenora-io-*.tmp` nella directory della
destinazione, si porta su disco (`sync_all`), si verifica, e solo allora si
rinomina. Un errore prima della rinomina cancella il temporaneo: la
destinazione non vede mai un file parziale. Una destinazione esistente è
`Conflict` senza `OpzioniScrittura::sovrascrivi`, anche se compare durante la
scrittura (`persist_noclobber`); con la sovrascrittura la rinomina la
sostituisce.

## Un piano da file a file

`esegui_da_file` controlla prima di leggere che ogni output del piano abbia
un solo percorso, che i percorsi d'uscita siano distinti fra loro e dagli
ingressi (per testo e, per i file esistenti, per percorso canonico) e
scrivibili; poi carica gli input nell'ordine dato, esegue `validate` e `run`,
e scrive gli output nell'ordine del piano, liberando ognuno appena scritto.
Un errore nella lettura di un input ha fase `read`.

`esegui_da_file_interrompibile` è la stessa esecuzione con
un'`Interruzione` (scadenza e annullamento, [«Scadenza e
annullamento»](runner.md#scadenza-e-annullamento)): oltre ai controlli del runner,
prima di leggere ogni input (fase `read`) e prima di scrivere ogni output
(fase `write`; dopo il primo output scritto l'effetto è `partial`); rende
anche nome, righe, colonne e formato di ogni output scritto (`EsitoFile`).
`valida_da_file` carica gli input allo stesso modo e valida il piano senza
eseguirlo.

## Memoria

Le tabelle caricate contano nel budget del piano
(`max_governed_memory_bytes`) come in `run`: ogni input si legge con il
budget residuo, e dopo la lettura i byte vivi esatti devono starci.

| passo | controllo prima | misura (Windows, allocazioni contate, 1 000–5 000 000 righe) |
| --- | --- | --- |
| lettura Arrow IPC | dimensione del file prima di leggerlo, il doppio prima di decodificare se i blocchi sono più di uno, e prima di ricomporli i byte vivi dei blocchi decodificati più la loro copia | picco 1,0 volte la tabella con un blocco, 2,0 con più blocchi |
| lettura Parquet | 5 volte la stima dal footer, più 1 MiB | picco fino a 2,8 volte la tabella da 20 000 righe in su (liste, interi e float con null; 4 volte a 1 000 righe, per i buffer fissi), sempre sotto il 75% della previsione |
| scrittura Arrow IPC | due volte il blocco più grande (circa 8 MiB, di più con righe molto più grandi della media), misurato sui blocchi veri con i dizionari interi, più 1 MiB | picco sotto 3 MiB |
| scrittura Parquet | 4 volte i byte del row group più grande, misurato sulle fette vere, più 8 MiB | picco fino a 45 MiB (liste, 5 milioni di righe) |

La **stima dal footer** Parquet è il maggiore fra i byte non compressi dei
column chunk e i valori per la larghezza fisica di ogni foglia, più i byte
decodificati dei `BYTE_ARRAY` quando il file li dichiara
(`unencoded_byte_array_data_bytes`). Le misure si rifanno con un
allocatore che conta, fuori dal workspace (niente `unsafe` qui).

## Confine di lettura

Il minimo perché un file malformato diventi un errore esplicito invece di
un panico o di un risultato sbagliato (`plenora_io::confine`). Non è una
difesa da file costruiti apposta (limiti dichiarati sotto).

- **Barriera anti-panico**: ogni chiamata ad `arrow-ipc`, `parquet` e
  `concat_batches` sui byte del file gira in
  `plenora_core::panic_policy::barriera_di_dipendenza`: un panico (per
  esempio di un `unwrap` sui campi opzionali del footer) diventa
  `DataMapping` con la sola forma del payload. Lo schema IPC si converte
  con `try_fb_to_schema`, che da `arrow-ipc` 60 restituisce un errore dove
  `fb_to_schema` andava in panico.
- **Budget prima di leggere, decodificare e ricomporre** (tabella in
  «Memoria»).
- **Arrow IPC**: il file si legge intero in un buffer allineato (già nel
  budget) e, prima di Arrow, se ne percorre la struttura: prefissi e
  lunghezze dei messaggi, metadati entro il tetto, corpi e blocchi del
  footer dentro il file. `FileDecoder` e `StreamDecoder` decodificano poi
  per viste dello stesso buffer. Alcuni controlli evitano un risultato
  sbagliato senza errore: il marcatore di fine dello stream (senza, uno
  stream tagliato fra due messaggi darebbe meno righe); solo messaggi di
  schema, dizionario e blocco (`StreamDecoder` salta un messaggio `NONE`,
  e un blocco col tipo cambiato sparirebbe); blocchi del footer né ripetuti
  né sovrapposti (moltiplicherebbero le righe) e coerenti col loro
  messaggio, cioè lunghezza dei metadati uguale a prefisso più messaggio,
  tipo atteso e stessa lunghezza del corpo (`FileDecoder` prende il corpo
  dall'offset del blocco: un blocco spostato leggerebbe metadati come
  valori); l'endianness (`StreamDecoder` non la guarda); le voci dei
  metadati di schema e di campo, a ogni profondità, con chiave e valore
  (anche vuoto) e chiavi non ripetute (`try_fb_to_schema` scarterebbe una
  voce senza chiave o senza valore e terrebbe l'ultima di due chiavi).
- **Parquet**: la lunghezza del footer, letta dalla coda, entro il tetto
  prima che `parquet` la usi; i row group entro il massimo e la loro somma
  di righe uguale a quella del footer dopo; i metadati chiave-valore del
  file con un valore (anche vuoto) e chiavi non ripetute, e lo schema
  incorporato con le stesse verifiche di un file IPC (`parquet` scarterebbe
  una voce senza valore). È una restrizione voluta: la specifica Parquet
  ammette una voce senza valore, ma qui sparirebbe in silenzio, quindi si
  rifiuta; pyarrow e arrow-rs scrivono sempre il valore, anche vuoto.
- **Codifiche Parquet lette**: solo `PLAIN`, `PLAIN_DICTIONARY` e
  `RLE_DICTIONARY` per i valori, `RLE` per i booleani e per i livelli
  ([«Codifiche Parquet non qualificate»](limiti.md#codifiche-parquet-non-qualificate)).
  Livelli `BIT_PACKED` (deprecata) sono `Unsupported`; una codifica che
  non esiste nella specifica è un file malformato (`DataMapping`).
  `DELTA_BINARY_PACKED`, `DELTA_LENGTH_BYTE_ARRAY`, `DELTA_BYTE_ARRAY` e
  `BYTE_STREAM_SPLIT` (e ogni altra) sono `Unsupported`, con un testo
  fisso. Il controllo è doppio: le codifiche che il column chunk dichiara
  nel footer (l'elenco e le statistiche delle pagine), prima di leggere, e
  quella di ogni pagina letta davvero, nel fork `parquet`: un footer che
  dichiara `PLAIN` e una pagina in `DELTA_*` si rifiutano come il
  contrario. pyarrow (anche con pagine v2), data e IO-tools con le
  impostazioni predefinite non le scrivono.
- **Tipi letti**: un tipo Arrow testo (`Utf8`, `LargeUtf8`, `Utf8View`,
  anche come valori di un dizionario) solo su una colonna annotata come
  testo (`UTF8`, `JSON`, `ENUM`), perché la validazione UTF-8 segue
  l'annotazione; un dizionario Arrow di valori binari solo su una colonna
  non annotata come testo; una colonna `FIXED_LEN_BYTE_ARRAY` letta come
  dizionario Arrow; tutte altrimenti `Unsupported`. Per la stessa ragione un
  dizionario di `FixedSizeBinary` non si scrive (`ArrowWriter` ne
  scriverebbe il dizionario con i prefissi di lunghezza, fuori dalla
  specifica). Un `INT32` annotato `INT_8`,
  `UINT_8`, `INT_16` o `UINT_16` fuori dalla sua larghezza è un errore,
  non un altro numero.

| limite (`LimitiLettura`) | predefinito | a che cosa si applica | errore |
| --- | --- | --- | --- |
| `max_byte_metadati` | 16 MiB, e mai oltre il budget residuo | ogni messaggio IPC, footer IPC, footer Parquet | `ResourceLimit` |
| `max_byte_metadati_custom` | 4 MiB | chiavi e valori dei metadati di schema e di tutti i campi, a ogni profondità; per Parquet più tutti i metadati chiave-valore del file, `ARROW:schema` compreso | `ResourceLimit` |
| `max_blocchi` | 100 000 | blocchi IPC (record batch), row group Parquet | `ResourceLimit` |

`leggi_tabella` usa i predefiniti, `leggi_tabella_con_limiti` li prende
espliciti. Gli errori dicono che cosa non va e quale limite, mai byte del
file o valori. Le prove sono in `crates/plenora-io/tests/confine.rs`: file
troncati a ogni lunghezza (Arrow IPC e Parquet: sempre errori), Arrow IPC
con ogni byte invertito (nessun panico; ogni posizione con la suite lunga,
testa, coda e una ogni 11 senza), lunghezze enormi di metadati, blocchi e
footer, stream senza fine o con byte dopo la fine, un blocco dello stream
cambiato in `NONE`, blocchi del footer ripetuti o spostati rispetto al loro
messaggio, una pagina Parquet con checksum e un valore corrotto, e ogni
limite della tabella.

## Limiti dichiarati

- **Transitori di file previsti, non misurati.**
  *Regola*: prima di leggere un Parquet e prima di scrivere si verifica un
  picco previsto; dopo la lettura, i byte vivi esatti.
  *Ambito*: `parquet_io::leggi`, `ipc::leggi`, `esegui_da_file`.
  *Hazard*: la previsione Parquet viene dalle misure dei profili sopra; un
  file senza `unencoded_byte_array_data_bytes` con stringhe lunghe
  codificate a dizionario si decodifica in molti più byte di quanti il
  footer ne mostri, e la decodifica può superare il budget prima del
  controllo esatto, che allora fallisce con `ResourceLimit` dopo il picco.
  Come il budget del runner, non è un tetto duro sulla memoria del processo.
  *Rientro*: un allocatore contato per il processo.
- **Tipi rifiutati in scrittura**: in Parquet `Interval(MonthDayNano)`
  (errore di `parquet`, nessun file); in ogni formato `RunEndEncoded` e
  `Union`, a qualunque profondità, prima di creare il file (limite «Run-end
  e union rifiutati al confine» del runner, anche in lettura). Un test ne
  tiene l'elenco. Una tabella senza colonne non si scrive in Parquet (il numero di
  righe andrebbe perso): si usa Arrow IPC.
- **Codec Parquet**: solo `UNCOMPRESSED`, `SNAPPY`, `ZSTD` sono compilati;
  `GZIP`, `BROTLI`, `LZ4`, `LZ4_RAW`, `LZO` si rifiutano prima di decodificare
  (`Unsupported`). Arrow IPC compresso (LZ4/ZSTD) si rifiuta con l'errore di
  Arrow.
- **File costruiti apposta: aborto del processo.**
  *Regola*: il confine di lettura ferma i panici e verifica le lunghezze
  che costano poco (sopra); il contenuto dei footer, delle intestazioni di
  pagina e dei messaggi IPC resta a `parquet` e `arrow-ipc`.
  *Ambito*: `parquet_io::leggi` (footer Thrift, intestazioni e pagine),
  `ipc::leggi` (buffer e nodi dei messaggi, dizionari delta, compressione
  dichiarata).
  *Hazard*: un file Parquet malformato o costruito apposta (e i casi IPC non
  coperti) può far terminare il processo: `parquet` e `arrow-ipc` allocano
  dalle lunghezze dichiarate prima di verificarle, e un'allocazione
  impossibile è un aborto, che la barriera anti-panico non ferma. La patch
  di `parquet` (`vendor/parquet-60.0.0-eof/PROVENANCE.md`) chiude i casi
  trovati dal fuzz: header di pagina che giravano a vuoto per minuti,
  footer che riservavano gigabyte per i row group o le posizioni
  dell'offset index, interi Thrift troncati, `FIXED_LEN_BYTE_ARRAY` di
  larghezza 0 e `BYTE_STREAM_SPLIT` con valori dichiarati oltre i byte
  della pagina (panici del decoder), e i decoder che si fidavano delle
  lunghezze, degli indici e dei conteggi letti dal file (panici, e tre
  letture sbagliate senza errore: prefissi `DELTA_BYTE_ARRAY`, resti delle
  pagine a larghezza fissa, corse RLE oltre `u32`); e le dimensioni che un
  header di pagina, un dizionario, una codifica delta o lo schema
  dichiarano oltre i metadati del column chunk, che il budget ha già
  confrontato prima di leggere. Restano le espansioni vere (dizionari
  ripetuti, `FixedLenByteArray` larghi), che il budget stima per un
  fattore fisso; in IPC le copie di buffer sovrapposti e non allineati. Il
  tetto vero resta il limite di memoria del processo. Uno schema Parquet
  annidato oltre 64 livelli (`MAX_PROFONDITA_SCHEMA`), che esauriva lo
  stack, e un footer la cui decodifica supera il suo tetto
  (`budget_del_footer`: 16 volte il tetto dei metadati, mai oltre il budget
  residuo) sono `ResourceLimit`. I file scritti da scrittori conformi non
  fanno nulla di tutto questo.
  *Rientro*: se si devono leggere file di fonti non fidate, aggiungere una
  pre-validazione (footer e intestazioni di pagina percorsi prima di
  `parquet`, contenuto dei messaggi IPC prima di Arrow).
- **Pagine Parquet senza checksum.**
  *Regola*: `parquet` verifica il CRC di ogni pagina che lo porta (feature
  `crc`); una pagina con CRC sbagliato è un errore (`DataMapping`).
  *Ambito*: `parquet_io::leggi`.
  *Hazard*: una pagina senza CRC (il default di pyarrow e di `parquet-rs`)
  corrotta ma ancora decodificabile, per esempio un byte di un valore
  numerico in una pagina non compressa, torna con altri valori senza errore.
  **I file scritti da `plenora-io` non portano CRC**: lo scrittore di
  `parquet` 60.0.0 (come 59.2.0) non sa scriverli (l'intestazione di
  pagina ha sempre `crc: None`, `column/page.rs`, «TODO: Add support for
  crc checksum», e `WriterProperties` non ha un'opzione), quindi la
  verifica protegge solo i file di altri scrittori che li hanno scritti.
  *Rientro*: scrivere i CRC (`write_page_checksum` in pyarrow) nei file da
  proteggere; per i nostri, una versione di `parquet` che li scriva; un
  controllo d'integrità del file intero a monte.
- **Parquet modificato durante la lettura.**
  *Regola*: la lunghezza del footer si verifica sul file aperto, poi
  `parquet` lo rilegge.
  *Ambito*: `parquet_io::leggi` (Arrow IPC no: si decodifica dal buffer
  verificato).
  *Hazard*: un altro processo che riscrive il file fra la verifica e la
  lettura può presentare a `parquet` un footer di lunghezza non verificata.
  *Rientro*: decodificare da un buffer letto una volta, come per Arrow IPC,
  al prezzo del file intero in memoria durante la decodifica.
- **Righe senza byte.**
  *Regola*: il budget conta i byte; una colonna `Null` o una tabella senza
  colonne dichiara righe che non occupano byte.
  *Ambito*: `ipc::leggi` (lunghezza del blocco e dei nodi `Null`),
  `parquet_io::leggi` (tabella senza colonne foglia).
  *Hazard*: un file di pochi byte può dichiarare miliardi di righe; la
  lettura riesce, e un'operazione che poi alloca per riga risponde al
  budget del runner, non al confine.
  *Rientro*: un tetto sulle righe dichiarate.
- **Strutture Arrow per blocco non contate.**
  *Regola*: il confine IPC conta i byte del file, non le strutture che
  Arrow crea per ogni colonna di ogni blocco (qualche centinaio di byte
  ciascuna), né le copie dei buffer non allineati.
  *Ambito*: `ipc::leggi` prima della verifica esatta dei byte vivi.
  *Hazard*: un file con molti blocchi di molte colonne vuote occupa durante
  la decodifica qualche volta i suoi byte di metadati, oltre la previsione;
  i blocchi restano entro `max_blocchi` e i metadati entro il file.
  *Rientro*: contare colonne per blocchi nella previsione.
- **Arrow IPC file V4.** `FileDecoder` di `arrow-rs` 60.0.0 (come 59.2.0)
  rifiuta un file (non uno stream) scritto con `metadata_version` V4, come
  lo scrive pyarrow con `metadata_version=V4` (`DataMapping`, «arrow error:
  ipc»); lo rifiuta anche `FileReader` da solo, prima di questo confine. Lo
  stream V4 si legge. *Rientro*: una versione di `arrow-ipc` che lo accetti.
- **Hook di panico.** La barriera trasforma il panico in errore, ma l'hook
  di `std` ne stampa il testo su stderr prima, e quel testo può contenere
  byte del file: chi usa `plenora-io` installa
  `plenora_core::panic_policy::install` (limite già dichiarato lì).
- **GeoParquet, ciò che il contratto non porta**: `orientation`, `bbox` e
  `covering` letti si validano e si perdono; `epoch` e `edges: spherical` si
  rifiutano; il contratto ammette una sola colonna geometrica, quindi
  un file con più colonne geometriche si rifiuta (`Schema`). Il PROJJSON
  letto si identifica per `id` e `type`: il resto del documento non si
  confronta con la tabella, e un documento che dichiara un `id` EPSG con
  parametri diversi da quelli del registro passa come quel codice.
- **Metadati geometrici normalizzati**: dopo GeoParquet il campo porta il
  `geo` di campo e il blocco canonico nella forma del contratto, non i byte
  di metadati che aveva prima della scrittura; il contratto si conserva
  (provato su ogni combinazione di dimensionalità, dati e dichiarazione dei
  tipi), i byte dei metadati no. Due cambi voluti: con dimensionalità nota
  e tipi non dichiarati (o `unresolved`) la rilettura dichiara `exact` i
  tipi dei dati; senza
  blocco canonico e senza geometrie (zero righe o tutte nulle) la
  dimensionalità `xy`/`xyz` non si rappresenta (`geometry_types` vuoto) e si
  rilegge `unknown`.
- **Interi stretti da file malformati.**
  *Regola*: `parquet` legge `Int8`, `Int16`, `UInt8`, `UInt16` da colonne
  fisiche `INT32` con un cast che tronca.
  *Ambito*: `parquet_io::leggi` su file di altri scrittori.
  *Hazard*: un file malformato con un valore `INT32` fuori dal dominio del
  tipo logico (o dello schema incorporato) torna un altro valore, senza
  errore; gli scrittori conformi (e `plenora-io`) non producono questi file.
  *Rientro*: rilettura di quelle colonne come `Int32` con un cast
  verificato.
- **Permessi dei file su Unix**: `tempfile` crea il temporaneo con modo
  `0600` e la rinomina lo conserva, quindi i file scritti sono leggibili solo
  dal proprietario, anche quando sostituiscono un file con altri permessi.
  Su Windows valgono le ACL ereditate dalla directory.
- **Più output, non una transazione**: ogni file è atomico, l'insieme degli
  output no; un errore sul secondo lascia scritto il primo. La directory non
  si sincronizza dopo la rinomina (su un crash del sistema la rinomina può
  non essere durevole).
- **Errori di `parquet` come codici**: il testo della dipendenza non entra
  nei messaggi (può contenere valori), solo la variante (`parquet error:
  general`, …), come per Arrow.
