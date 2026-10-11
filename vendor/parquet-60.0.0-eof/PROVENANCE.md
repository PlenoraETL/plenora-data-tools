# Provenienza — `parquet` 60.0.0, eof

**Adottato**: è il `parquet` del workspace, di `fuzz/` e di chi dipende dai
crate di data-tools: pacchetto `plenora-parquet`, dipendenza per percorso
con la chiave `parquet` (`Cargo.toml`, «Copie vendorizzate»).

- Pacchetto: `parquet-60.0.0.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto (dal `Cargo.lock` prima del vendor):
  `8af83d2940bc0510f9aef86d865f56fdc6095f87ab115ac885a80b7c5226d3ba`.
- Contenuto: il pacchetto pubblicato, intero, tolto solo il marcatore di
  Cargo `.cargo-ok`, più le patch di `patches/`. Ricetta, in quest'ordine:

  ```sh
  tar -xzf parquet-60.0.0.crate && cd parquet-60.0.0 && rm .cargo-ok
  patch -p1 < patches/parquet-eof.patch           # protocollo thrift, dimensioni dichiarate
  patch -p1 < patches/parquet-nome-proprio.patch  # pacchetto `plenora-parquet`
  patch -p1 < patches/parquet-flba-bss.patch      # larghezza 0, BYTE_STREAM_SPLIT oltre i byte
  patch -p1 < patches/parquet-decoder.patch       # decoder che si fidavano del file
  patch -p1 < patches/parquet-footer-budget.patch # profondità dello schema, tetto del footer
  patch -p1 < patches/parquet-decoder-2.patch     # decoder, secondo giro
  patch -p1 < patches/parquet-codifiche-e-livelli.patch  # codifiche lette, terzo giro, formattazione
  patch -p1 < patches/parquet-livelli-dizionari-indici.patch  # BIT_PACKED, dizionari, pagine v2, offset index
  patch -p1 < patches/parquet-salti-e-fine-pagina.patch  # salti non qualificati, fine pagina esatta
  ```

  Il risultato è questa cartella byte per byte, tolto questo file
  (verificato l'11 ottobre 2026 dal `.crate` con il checksum sopra).
  Il nome del pacchetto: `plenora-parquet`, la libreria resta `parquet`
  (una `[patch.crates-io]` non vale per chi dipende dai crate di
  data-tools).
- Licenza: Apache-2.0 (`LICENSE.txt`, `NOTICE.txt` invariati). Il file
  modificato porta i commenti `PLENORA:` sulle righe cambiate.

## Che cosa corregge

Il protocollo thrift compact sul lettore `Read`
(`ThriftReadInputProtocol`, usato per gli header di pagina) girava a vuoto
su dati malformati: un Parquet di 169 byte bloccava la lettura per minuti
(target di fuzz `lettura_parquet`; test
`un_header_di_pagina_oltre_la_fine_dei_dati_e_un_errore_immediato` in
`crates/plenora-io/tests/confine.rs`, che senza la patch non termina).

1. `skip_bytes`: `io::copy` su un `take(n)` si ferma alla fine dei dati
   senza errore, quindi un salto incompleto «riusciva»; una lista thrift
   dichiarata di miliardi di `double` faceva miliardi di letture a vuoto.
   Ora un salto incompleto è `Eof`.
2. `read_bytes_owned`: la capacità veniva dalla lunghezza dichiarata (un
   valore enorme fa fallire l'allocazione, un aborto che nessuna barriera
   intercetta) e i byte mancanti non erano un errore. Ora nessuna capacità
   anticipata ed `Eof` se i byte finiscono prima.
3. Elementi booleani di liste, set e mappe: nel protocollo compact sono un
   byte ciascuno (come li legge `read_bool`), ma il salto non li leggeva, e
   una lista dichiarata di miliardi di booleani girava senza consumare
   dati. Ora ogni elemento booleano si legge (`skip_element`).

4. Varint (`read_vlq`, `skip_vlq`): la sequenza non aveva limite e
   `wrapping_shl` troncava in silenzio un valore oltre 64 bit. Ora al più
   dieci byte e un valore entro `u64`, altrimenti `IntegerOverflow`;
   `skip_vlq` applica gli stessi limiti di `read_vlq`.
5. `skip_element` rispetta lo stesso limite di profondità di
   `skip_till_depth`, booleani compresi.
6. Interi Thrift (`read_i16`, `read_i32`): il valore letto si convertiva
   con `as`, troncandolo in silenzio; ora un valore fuori tipo è un errore.
7. Capacità dai conteggi dichiarati: `read_thrift_vec` rifiutava già un
   conteggio oltre i byte rimasti, ma i row group del footer
   (`parquet_metadata_from_bytes`) e le posizioni dell'offset index
   costruivano il vettore a mano con `Vec::with_capacity(dichiarato)`: un
   footer di 147 byte chiedeva 5 GB (stesso target di fuzz). Ora entrambi
   passano da `capacita_dichiarata`, la stessa regola.

Il salto e le letture sono metodi di default del trait: le correzioni
valgono anche per il lettore su slice (footer), che saltava gli elementi
booleani di una collezione senza avanzare.

### Dimensioni dichiarate contro il column chunk (4 ottobre 2026)

La seconda lettura di Codex della prima patch aveva trovato, anche upstream,
altre allocazioni da dimensioni dichiarate prima di verificarle. La regola
che le chiude: chi legge (`plenora-io`) confronta il budget con i metadati
dei column chunk prima di leggere (`stima_decodificata`); una pagina non può
dichiarare più di quei metadati, e ciò che non dipende da loro si limita con
i byte davvero presenti.

8. `File::get_bytes` (`file/reader.rs`) riservava la lunghezza chiesta (da un
   header di pagina o dal footer) prima di leggere: ora un intervallo oltre
   la fine del file è `Eof` prima della riserva.
9. Header di pagina (`file/serialized_reader.rs`, `LimitiPagina`), nei due
   stati del lettore: la dimensione non compressa di una pagina non supera
   `total_uncompressed_size` del column chunk (che la comprende), e i valori
   di una pagina dati non superano `num_values` del chunk, letti
   dall'header che il tipo di pagina seleziona (lo stesso che usa
   `decode_page`: una pagina può portare sia l'header v1 sia il v2). Anche
   in `decode_page` l'header v2 (livelli non compressi, `is_compressed`)
   conta solo su una pagina v2: scelto per presenza, su una pagina v1 o
   dizionario spegneva la decompressione e la verifica della sua
   dimensione, e i byte compressi arrivavano al decodificatore. Il
   buffer di decompressione e i decodificatori restano così entro ciò che il
   budget ha visto. La dimensione compressa era già limitata ai byte rimasti
   del chunk (`verify_page_size`).
10. Dizionari (`decode_page`): un dizionario è PLAIN, almeno un bit per
    valore, quindi un conteggio oltre otto per byte della pagina è un errore
    prima che `DictDecoder::set_dict` o i lettori Arrow riservino un posto
    per valore dichiarato. Copre tutti i chiamanti di `decode_page`.
11. Codifiche delta (`DeltaBitPackDecoder::set_data`): il conteggio
    dell'header non supera i valori della pagina (nulli compresi) passati
    dal chiamante. Un miniblocco a larghezza zero codifica qualunque
    conteggio in pochi byte, quindi sono i valori della pagina, non i byte,
    a limitarlo. I lettori Arrow (`ByteArrayDecoderDeltaLength`,
    `ByteArrayDecoderDelta`, le versioni `ByteView`, `DeltaByteArrayDecoder`
    di `arrow/decoder`, i `FixedLenByteArray` delta) passavano `0` al posto
    del conteggio: ora passano i valori della pagina.
12. Schema (`schema/types.rs`): i figli dichiarati da `num_children` non
    superano gli elementi che restano dopo il nodo.
13. Dizionario `FixedLenByteArray`: il prodotto valori × larghezza usava una
    moltiplicazione che in release si avvolge e avrebbe accettato un
    dizionario corto; ora `checked_mul`, e un overflow è un errore (dal
    quarto giro la pagina deve avere esattamente i byte delle voci).

Le prove (`crates/plenora-io/tests/parquet_dichiarazioni.rs`) prendono un
file valido di `parquet-rs`, riscrivono un solo intero dichiarato con uno più
grande nello stesso numero di byte, e verificano il rifiuto in dt2 e il
messaggio di `parquet`; senza la patch due di quei file (pagina più grande
del chunk, conteggio delta) si leggevano senza errore. Le codifiche valide
(dizionario, delta, `BYTE_STREAM_SPLIT`, `FixedLenByteArray`, pagine v1 e v2,
senza compressione, SNAPPY, ZSTD) si rileggono uguali, anche con nulli,
colonne tutte nulle e liste. Le mutazioni si leggono anche con l'offset index
caricato, cioè dall'altro stato del lettore di pagine; una pagina v2 con
anche l'header v1 prova che il controllo guarda l'header del tipo.

### Larghezza 0 e BYTE_STREAM_SPLIT (10 ottobre 2026)

Da `patches/parquet-flba-bss.patch`, portata da plenora-IO-tools (PR #46,
`vendor/parquet` a `1b3e264`, che adotta questo stesso fork): i delta di
`fixed_len_byte_array.rs` e `byte_stream_split_decoder.rs` sono quelli,
identici; quello di `data_type.rs` è di qui, sulla stessa classe.

1. `FIXED_LEN_BYTE_ARRAY` di larghezza 0 (lo schema la ammette): il
   lettore Arrow divideva per la larghezza nei decoder `PLAIN` e
   `BYTE_STREAM_SPLIT` (`attempt to divide by zero`); ora il lettore si
   rifiuta di costruirsi (`invalid FIXED_LEN_BYTE_ARRAY width`), e una
   larghezza negativa non diventa più un `usize` enorme. Il decoder a
   larghezza variabile di `BYTE_STREAM_SPLIT` rifiuta la larghezza 0 in
   `set_data`; il decoder `PLAIN` generico (API per colonne,
   `data_type.rs`) aveva un `assert!` sulla larghezza, ora un errore.
2. `BYTE_STREAM_SPLIT`: il decoder indicizzava `src[values_decoded + i + j *
   stride]` senza confronto con i byte; valori dichiarati oltre i byte della
   pagina, in modo concorde in header di pagina e metadati di colonna (che
   il controllo del column chunk sopra non vede), erano `index out of
   bounds`. Ora `values_decoded + num_values <= stride`, altrimenti un
   errore, nei due decoder. Il difetto è ancora in `parquet` 60.0.0 a monte.

Raggiungibili da `plenora-io`: un file senza `ARROW:schema` con una colonna
di larghezza 0, e un `DOUBLE` in `BYTE_STREAM_SPLIT` con i conteggi
gonfiati, facevano panicare `parquet`; la barriera di lettura li rendeva un
errore `data_mapping` («parquet in panico»), non un risultato, ma un
panico in un decoder resta un difetto. Prove rosse:
`crates/plenora-io/tests/parquet_fork.rs` (file costruiti, un campo
alterato, sul decoder diretto e dal confine).

### Decoder che si fidavano dei valori del file (10 ottobre 2026)

Da `patches/parquet-decoder.patch`, su rilievo della seconda lettura
(Codex) della patch precedente: la stessa classe dei due difetti sopra,
cercata nei decoder di `encodings/`, `arrow/decoder/`,
`arrow/array_reader/`, `arrow/buffer/` e nei livelli delle pagine. Nei
punti elencati sotto (e solo in quelli: non è una garanzia su tutto il
pacchetto, che è grande e che una seconda lettura ha già mostrato
incompleto, sezione seguente) un valore letto dal file che diventa una
lunghezza, un indice o un estremo di slice passa da una conversione
fallibile e da aritmetica controllata, e il caso limite è un errore.
Prove rosse in
`crates/plenora-io/tests/parquet_decoder.rs` (un file costruito a mano
per caso; i file sono anche semi del fuzz, `tests/dati/fuzz-decoder/`):

1. `arrow/decoder/delta_byte_array.rs`: un prefisso più lungo del valore
   precedente passava a `truncate`, che non fa nulla: il valore usciva
   sbagliato **senza errore**; una lunghezza di suffisso negativa,
   convertita in `usize`, faceva traboccare la fine del suffisso
   (`checked_value`, in lettura e nel salto). Gli `assert_eq!` sui
   conteggi letti sono errori.
2. `arrow/array_reader/fixed_len_byte_array.rs`: un indice di
   `RLE_DICTIONARY` oltre il dizionario FLBA (o negativo) tagliava una
   slice fuori dai limiti; ora `try_from`, `checked_mul`, `get`. Una
   pagina `PLAIN` o `BYTE_STREAM_SPLIT` che non è un multiplo della
   larghezza perdeva il resto **in silenzio**; ora si rifiuta. La
   dimensione del buffer prima di `build_unchecked` è un errore in ogni
   build (era un `debug_assert`).
3. `encodings/decoding.rs`: `DELTA_LENGTH_BYTE_ARRAY` (anche dentro
   `DELTA_BYTE_ARRAY`) con una lunghezza negativa o oltre i byte tagliava
   fuori dalla pagina, e la somma in `i32` del salto traboccava; ora
   `check_delta_lengths` una volta in `set_data` (lo stesso controllo dei
   decoder Arrow, che lo usano anche loro) e la somma in `usize`.
   `DELTA_BINARY_PACKED`: l'estremo del blocco con `block_size` fino a
   `usize::MAX` (2^62 passa i controlli dell'header) era
   `larghezza * valori` non controllato; ora `checked_mul`/`checked_add`.
4. `encodings/decoding/byte_stream_split_decoder.rs`: il resto della
   divisione per la larghezza era scartato **in silenzio** (65 byte di otto
   `DOUBLE`); ora un errore in `set_data`. I due `skip` dichiaravano
   saltati valori oltre i byte; ora lo stesso controllo di `get`.
5. `arrow/arrow_reader/statistics.rs`: una statistica decimale vuota o
   più lunga del tipo mandava in panico `sign_extend_be`; ora
   `sign_extend_be_checked` e la statistica manca (`None`). Nei dati, un
   decimale `BYTE_ARRAY` vuoto o più largo del tipo è un errore
   (`check_decimal_widths`).
6. Stessa classe, trovati cercandola: `ByteArrayDecoderPlain::read`
   dichiarava letti tutti i valori chiesti anche quando la pagina finiva
   prima (il chiamante, che confronta i conteggi con i livelli, non poteva
   vederlo); i salti `PLAIN` di `BYTE_ARRAY` e delle viste e il decoder
   `PLAIN` generico (`data_type.rs`) non avevano limiti; un offset oltre il
   tipo d'indice era un `expect`; una chiave di dizionario negativa faceva
   traboccare `index + 1` (`offset_buffer.rs`); i valori del dizionario FLBA
   della larghezza sbagliata facevano panicare `FixedSizeBinaryArray::new`
   (`dictionary_buffer.rs`); una corsa RLE oltre `u32` si troncava in
   silenzio (`rle.rs`); le lunghezze dei livelli di una pagina v2 si
   sommavano in `i32` e si confrontavano con la dimensione dichiarata
   invece che con i byte letti (`serialized_reader.rs`); i livelli
   `BIT_PACKED` dichiarati oltre la pagina (`column/reader.rs`; dal quarto
   giro `BIT_PACKED` per i livelli non è qualificata); una pagina
   a dizionario senza la pagina di dizionario (`column/reader/decoder.rs`,
   un `expect`).

Le prove unitarie del pacchetto (`cargo test --lib` con le feature del
workspace e i dati di `apache/parquet-testing` e `apache/arrow-testing`):
1375 passano, le stesse 5 falliscono con e senza questa patch. Sono prove
che costruiscono dichiarazioni incoerenti e che la patch `-eof` rifiuta
già (`test_delta_bit_packed_padding`,
`test_delta_bit_packed_skip_wide_miniblocks`,
`test_decode_unsupported_page`, `test_page_writer_data_pages`,
`test_page_writer_dict_pages`).

Non coperti, e perché: il lettore asincrono e la cifratura non sono
compilati (feature non abilitate); le funzioni pubbliche
`decode_column_index`/`decode_offset_index` decodificano indici di pagina
che questo workspace non chiede (dal quarto giro le posizioni dell'offset
index si verificano quando il lettore dei metadati le carica, non in
`decode_offset_index`).

### Decoder, secondo giro (10 ottobre 2026)

Da `patches/parquet-decoder-2.patch` (dopo `parquet-footer-budget.patch`),
sulla seconda lettura (Codex) della patch precedente, che ha trovato altri
punti della stessa classe. Prove rosse in
`crates/plenora-io/tests/parquet_decoder.rs` (sezione «Secondo giro»):

1. FLBA `RLE_DICTIONARY` senza pagina di dizionario: un `unwrap`.
2. Dizionario FLBA: gli indici erano confrontati con i byte della pagina e
   non con le voci dichiarate; un indice fra le une e gli altri leggeva
   byte che non sono una voce **in silenzio**. Ora il dizionario si taglia
   alle voci dichiarate (dal quarto giro: byte oltre le voci sono un
   errore).
3. `DELTA_BYTE_ARRAY` generico: con meno suffissi che prefissi il valore
   riusava il suffisso precedente (["a", "a"] da un suffisso solo), **in
   silenzio**; ora conteggi diversi e un suffisso mancante sono errori.
4. `DELTA_BINARY_PACKED`: la fine dell'ultimo blocco conta il padding, che
   una pagina troncata non ha; l'offset dopo le lunghezze cadeva oltre la
   pagina e la sezione seguente si tagliava fuori dai limiti. Ora l'offset
   si confronta con la pagina (decoder generici e Arrow,
   `check_delta_lengths`).
5. Varint (`BitReader::get_vlq_int`): oltre 10 byte un `assert!`, e i bit
   oltre i 64 scartati **in silenzio**. Ora `get_vlq_int_checked` li
   rifiuta, e il decoder RLE distingue un varint malformato dalla fine dei
   dati.
6. Il decoder ottimizzato dei livelli Arrow (`definition_levels.rs`,
   larghezza 1) non aveva le correzioni del decoder RLE: una corsa
   bit-packed oltre i dati leggeva bit fuori dal payload, la corsa di 2^32
   era accettata, il varint troncava, e un valore RLE diverso da 0 e 1 era
   letto come 1 **in silenzio**.
7. Chiavi di dizionario strette (`Int8`, ...): l'indice si decodificava
   direttamente nel tipo della chiave, con `as`, e 256 diventava 0 prima di
   ogni controllo (**in silenzio**). Ora si decodifica in `i32` e si
   converte con controllo.
8. Dizionario `FixedSizeBinary`: si controllava solo la lunghezza totale,
   ["a", "bcd"] diventava ["ab", "cd"] **in silenzio**; ora ogni voce.
9. Lo `skip` di `DELTA_BINARY_PACKED` (larghezza 0) rifiutava pagine
   valide per il prodotto intermedio fuori da `i32`; ora avvolge come la
   lettura (verificato sui valori).

Non coperto da una prova: un offset oltre il tipo d'indice (`i32`) in
`ByteArrayDecoderDeltaLength::read` richiede più di 2 GiB di valori in
una tabella.

### Codifiche lette, terzo giro, formattazione (11 ottobre 2026)

Da `patches/parquet-codifiche-e-livelli.patch`.

**Codifiche qualificate.** `basic::is_qualified_value_encoding` e
`check_qualified_value_encoding`: si leggono solo `PLAIN`,
`PLAIN_DICTIONARY`, `RLE_DICTIONARY` e `RLE` (booleani). Il controllo è
dove si sceglie un decoder di valori per una pagina di dati
(`column/reader/decoder.rs` e i decoder Arrow dei byte array, delle viste
e dei FLBA): `DELTA_*` e `BYTE_STREAM_SPLIT` sono un errore `NYI` con il
testo fisso `ENCODING_NOT_QUALIFIED`. Dal quarto giro anche le pagine di
dizionario, i livelli e le pagine saltate intere; restano fuori le pagine
saltate attraverso l'offset index (sezione seguente). Le correzioni dei
loro decoder (sezioni sopra) restano, non raggiungibili; `plenora-io`
controlla anche le codifiche dichiarate nel footer.

**Terzo giro di revisione** (Codex), con prove in
`crates/plenora-io/tests/parquet_decoder.rs`:

1. Livelli (`column/reader/decoder.rs`): un livello oltre il massimo della
   colonna sta nella larghezza in bit ma non è un livello; era contato
   come nullo (definizione) o come livello a sé (ripetizione). Ora un
   errore, per il decoder generico e quindi per il percorso Arrow che lo
   usa. Nel decoder RLE condiviso il valore di una corsa RLE deve stare
   nella larghezza dichiarata. Una corsa bit-packed finale più corta dei
   suoi gruppi resta lecita, come a monte e in C++ (`test_truncated_rle`:
   alcuni writer non la completano): si legge fino ai suoi byte, e il
   lettore confronta i livelli ottenuti con quelli dichiarati. I valori si
   confrontano con quelli dichiarati solo in una pagina v2 (dal quarto
   giro): una pagina v1 non dichiara i suoi valori, e sono i livelli a
   deciderli.
   Il decoder ottimizzato dei livelli (`definition_levels.rs`) ora segue la
   stessa regola invece di rifiutarla.
2. Colonna FLBA con tipo Arrow dizionario: passava dal lettore dei byte
   array variabili, che legge il dizionario con il prefisso di lunghezza (il
   formato che `ArrowWriter` di parquet-rs scrive per un
   `Dictionary(_, FixedSizeBinary)`, non quello della specifica): un
   dizionario FLBA valido si rifiutava. Ambiguo per costruzione:
   `Unsupported` esplicito (`FLBA_AS_DICTIONARY`).
3. Interi stretti (`primitive_array.rs`): `INT32` annotato `INT_8`,
   `UINT_8`, `INT_16`, `UINT_16` si convertiva con `as` (256 → 0); ora
   `try_unary` con un errore.
4. UTF-8: la validazione di `PLAIN` (anche per le viste e i dizionari) è
   corretta, perché le lunghezze separano i valori; il difetto vero era
   altrove: i decoder validano dall'annotazione della colonna, non dal tipo
   Arrow, e uno schema Arrow (incorporato o dato) che chiedeva testo su
   byte non annotati produceva stringhe non validate. Ora
   `STRING_WITHOUT_ANNOTATION` (`Unsupported`), e `JSON`/`ENUM` si validano
   come `UTF8`.
5. Varint: dieci byte di continuazione senza terminatore alla fine dei dati
   erano la fine dei dati (`Ok(None)`); ora un errore.

**Formattazione.** Le righe dei delta precedenti sono formattate con
`rustfmt` (edizione 2024), come fa `cargo fmt` nel fork di plenora-IO-tools:
solo spazi e a capo, nelle righe `PLENORA`.

Prove unitarie del pacchetto con `parquet-testing`: falliscono 52 prove,
le 5 di prima più 47 attese, che usano ciò che ora si rifiuta (42 codifiche
non qualificate, compresa una via l'API per righe, 2 testo su byte non
annotati, 2 FLBA come dizionario, 1 lettura di byte non UTF-8 come testo).
Letti con `plenora-io`, i file di `parquet-testing` cambiano esito solo
dove usano codifiche non qualificate (9 file).

### Livelli BIT_PACKED, dizionari, pagine v2, offset index (11 ottobre 2026)

Da `patches/parquet-livelli-dizionari-indici.patch`, sul quarto giro di revisione (Codex). Prove
rosse in `crates/plenora-io/tests/parquet_decoder.rs` («Dizionari, pagine
v2, valori di testo») e `parquet_footer.rs` («Offset index»), contro il
fork del terzo giro; i file malformati sono anche semi del fuzz.

1. Livelli `BIT_PACKED`: impacchettati dal bit più significativo, i due
   decoder dei livelli li leggevano nell'ordine dell'RLE ibrido, dal meno
   significativo. Un payload `0x80` su 8 righe metteva il valore
   nell'ultima riga invece che nella prima, **in silenzio**. Deprecata,
   non scritta dai writer attuali: non qualificata (`parse_v1_level`,
   `LevelDecoder`, il decoder ottimizzato di `definition_levels.rs`, che
   prima arrivava a `unreachable!` con una codifica sconosciuta). Nessuno
   dei 106 file di `parquet-testing` e `arrow-testing` ha una pagina letta
   che la usi; 40 la dichiarano nel footer, per i livelli assenti.
2. Dizionari: `set_dict` ignorava il conteggio letto. Una pagina con meno
   voci delle dichiarate si accettava (il decoder generico teneva valori di
   default come voci), e zero voci dichiarate con dei byte dividevano per
   zero nel lettore `PLAIN` dei byte array. Ora ogni pagina di dizionario
   (byte array, viste, dizionari Arrow, FLBA, decoder generico) deve avere
   esattamente le voci dichiarate e nessun byte dopo l'ultima
   (`DICTIONARY_NOT_AS_DECLARED`); la divisione è `checked_div`. Una codifica
   esclusa nella pagina di dizionario ha il testo `ENCODING_NOT_QUALIFIED`.
3. Pagine v2: i livelli decidono quanti valori si leggono, e una pagina i
   cui livelli contraddicevano `num_values - num_nulls` si leggeva (livelli
   `05 00` su 8 righe senza nulli dichiarati: 8 null con 8 valori nella
   pagina, **in silenzio**). Ora alla fine della pagina i valori letti
   sono quelli dichiarati. Un salto dentro la pagina sfuggiva a questo
   confronto (chiudeva la pagina senza decodificarne i valori): dal quinto
   giro non è qualificato. Il costruttore di pagine delle prove
   unitarie (`util/test_common/page_util.rs`) dichiarava `num_nulls` 0 per
   ogni pagina: ora conta i livelli sotto il massimo.
4. Il tipo dei valori di un dizionario di byte array seguiva solo
   l'annotazione `UTF8`: `JSON` ed `ENUM`, validati come testo dal terzo
   giro, tenevano valori binari, e `Dictionary(_, Utf8)` su `JSON` andava
   in panico. Ora il tipo segue `annotated_as_text`, e un dizionario Arrow
   binario su una colonna annotata come testo è `Unsupported`
   (`BINARY_DICTIONARY_OVER_TEXT`; prima, su `ENUM`, si leggeva).
5. Offset index: `first_row_index` (`i64`) si convertiva con `as usize` e
   si sottraeva senza controlli. Ora le posizioni di ogni pagina si
   verificano quando il lettore dei metadati carica l'indice e quando
   `SerializedPageReader::new_with_properties` le riceve: prima riga non
   negativa, non decrescente, entro le righe del row group; pagina dentro
   il column chunk. La sottrazione di `peek_next_page` è controllata.
6. Una pagina saltata intera (`skip_records`) si rifiuta per la sua
   codifica di valori come una pagina letta, dall'header. Senza header,
   attraverso l'offset index, il salto non la controllava (dal quinto giro
   non è qualificato); i livelli di una pagina saltata non si controllano
   (l'header ne nomina una codifica anche per una colonna senza livelli).

Una codifica sconosciuta (un identificatore fuori dall'enumerazione) è un
errore del Thrift, non `ENCODING_NOT_QUALIFIED`: dal confine `DataMapping`,
come un footer malformato.

Prove unitarie del pacchetto con `parquet-testing`: le stesse 52 di prima
falliscono (le 5 di partenza e le 47 attese). Letti con `plenora-io` e
direttamente (anche con gli indici di pagina caricati), i 106 file di
`parquet-testing` e `arrow-testing` hanno lo stesso esito del terzo giro.

### Salti dentro una pagina, fine della pagina (11 ottobre 2026)

Da `patches/parquet-salti-e-fine-pagina.patch`, sul quinto, sesto e
settimo giro di revisione (Codex), che hanno trovato le stesse classi nei
salti e nella fine delle pagine. Due regole
generali invece dei casi. Prove rosse in
`crates/plenora-io/tests/parquet_decoder.rs` («Salti e fine della
pagina») e `parquet_footer.rs` (selezione di righe), contro il fork del
quarto giro; contro quello, la prova dei salti non termina (livelli di
ripetizione troncati).

1. **Salti: solo pagine intere, dall'header.** Un salto dentro una pagina
   non decodificava ciò che saltava: una pagina v2 chiusa da un salto
   sfuggiva al confronto dei nulli, livelli di ripetizione troncati
   facevano girare il salto senza avanzare (il ciclo di lettura ha la sua
   guardia, quello del salto no), un indice di dizionario fuori dal
   dizionario passava. data e IO-tools leggono sempre per intero, senza
   selezione di righe: il salto dentro una pagina (una pagina già
   caricata, più righe di quelle da saltare, righe non note dall'header) e
   il salto di una pagina senza header (attraverso l'offset index) sono
   `SKIP_NOT_QUALIFIED` (`NYI`, `Unsupported` dal confine), finché non
   sono qualificati (dal tredicesimo giro anche i salti di pagine intere,
   sotto). Restavano i salti di pagine intere dall'header, che ne
   controllano la codifica; ogni giro del loro ciclo consuma una pagina o
   termina. Il salto di pagine intere confronta le righe dell'header con
   i suoi livelli e lo schema (sesto giro, sotto); su una colonna ripetuta
   le righe sono i livelli di ripetizione 0, che un salto non legge, e il
   salto non è qualificato (settimo giro). Non tutte le selezioni
   del lettore Arrow saltano: la selezione dei row group esclude chunk
   interi senza leggerli; `limit` può solo fermare la lettura (e una
   lettura fermata a metà pagina non arriva alla fine di quella pagina,
   quindi al suo controllo); `RowSelection`, i filtri di riga e `offset`
   saltano righe, e si rifiutano appena il salto non cade su pagine intere
   lette dall'header.
2. **Fine della pagina esatta.** In un punto unico del lettore di colonna
   (`verifica_fine_pagina`), alla fine di ogni pagina di dati letta fino
   all'ultimo livello (anche di zero livelli), ogni stream che il lettore
   decodifica deve finire con i suoi valori: livelli di ripetizione e di
   definizione (decoder generici e ottimizzato), valori `PLAIN` fino
   all'ultimo byte, indici di dizionario fino alla fine dello stream
   (contando i valori, non solo i byte: a larghezza 0 un gruppo non ha
   byte), booleani `RLE` fino alla lunghezza prefissata. Gli stream di
   livelli che lo schema non ha devono essere vuoti (sesto giro). Prima i
   valori, i livelli o i byte in più si ignoravano **in silenzio**.
   `PAGE_NOT_AS_DECLARED`. Ammesso il riempimento dell'ultimo gruppo
   bit-packed; una corsa porta tutti i gruppi che dichiara (nono giro: la
   corsa troncata, ammessa a monte, è un errore). Due deviazioni
   dichiarate (`docs/limiti.md`, «Fine delle pagine Parquet»), solo nelle
   pagine v1 (`FinePagina::V1`; le v2 finiscono esatte) e solo nella forma
   esatta misurata su scrittori reali:
   - fastparquet (2026.9) aggiunge 8 byte a zero a ogni pagina v1, dopo i
     valori, anche `PLAIN` (`writer.py`, `8 * b'\x00'`): dopo lo stream
     dei valori al più 8 byte, tutti zero (`CODA_FASTPARQUET`), misurati
     da dove comincia la prima corsa vuota, che chiude lo stream (settimo e
     ottavo giro); mai dopo i livelli, mai nello stream dei booleani `RLE`;
   - DuckDB (1.5) scrive le corse bit-packed a blocchi di 32 gruppi e
     completa l'ultimo con byte vecchi, non zeri: gruppi interi dopo
     l'ultimo valore solo nei livelli e negli indici, in una corsa che
     dichiara esattamente 32 gruppi (`BLOCCO_DUCKDB`), a larghezza sopra
     0, di cui almeno un valore si legge, qualunque cosa contengano. Perché il controllo distingua il riempimento
   dagli eccessi, i decoder dei livelli di ripetizione e degli indici di
   dizionario Arrow non leggono più in avanti oltre i valori chiesti.

**Sesto giro.**

1. Il salto di una pagina v2 si fidava di `num_rows` anche in una colonna
   senza ripetizione: 1 riga su 2 valori saltava due valori per una riga, 0
   righe saltava la pagina senza contarla. Ora le righe dell'header si
   confrontano con i livelli e lo schema (uguali senza ripetizione; con
   ripetizione almeno una riga per una pagina di livelli e non più righe
   che livelli), nel salto e alla lettura della pagina v2.
2. Dopo una pagina di zero livelli il lettore rispondeva «fine del column
   chunk», e il lettore Arrow abbandonava le pagine successive. Ora la
   pagina vuota si verifica e la lettura prosegue.
3. Una pagina v2 toglieva dal payload gli stream di livelli che lo schema
   non ha senza leggerli: ora la loro lunghezza deve essere 0.
4. Indici bit-packed a larghezza 0: un gruppo intero in più non occupa
   byte, e la fine lo accettava; ora si contano i valori rimasti.
5. Una pagina v2 compressa senza valori (tutti nulli) non decomprimeva la
   parte compressa, e un suffisso qualunque spariva. Ora si decomprime e
   deve dare zero byte (lo stream vuoto di un codec si legge).

**Quattordicesimo giro.**

1. Una `INDEX_PAGE` fra le pagine di un column chunk si saltava in
   silenzio: `get_next_page` avanzava oltre il suo contenuto e passava alla
   pagina dopo (il rifiuto in `decode_page` non si raggiungeva), e
   `peek_next_page` leggeva il suo contenuto come l'header seguente. Ora è
   un tipo di pagina non qualificato, rifiutato dove si incontra
   (`PAGE_TYPE_NOT_QUALIFIED`): nella lettura, nei due `peek` e nella
   conversione in `PageMetadata`, che per un header di pagina di dati senza
   la sua parte andava in panico (`unwrap`) e ora è un errore. Un tipo di
   pagina sconosciuto è già un errore della decodifica dell'header.
   Nessuno scrittore della matrice e nessuno dei 106 file scrive una
   pagina d'indice.
2. Statistiche deprecate (`min`/`max`): quando c'erano anche
   `min_value`/`max_value`, solo queste si verificavano; un `min` deprecato
   di 5 byte per un `INT32` passava. Ora la forma di ogni coppia presente
   si verifica, scelta o no, nel column chunk e nella pagina, con le
   stesse regole (`verifica_forma_statistiche`; la coppia deprecata non ha
   segni di esattezza, quindi è esatta).

Prove unitarie del pacchetto: le stesse 148 (due esecuzioni identiche);
`test_decode_unsupported_page` resta fra le dichiarazioni incoerenti (la
sua pagina dichiara 10 byte e ne ha 0, prima del tipo). Scrittori, FLBA e
106 file: lo stesso esito.

**Tredicesimo giro.**

1. Statistiche di pagina: con le proprietà di default
   (`read_page_statistics` falso) il decoder dell'header le saltava e
   non le verificava mai; un minimo FLBA di lunghezza 0 marcato esatto si
   leggeva. Ora l'header si decodifica sempre per intero, la forma delle
   statistiche si verifica sempre (`decode_page_con_larghezza`), e la
   proprietà decide solo se la pagina le espone. I decoder
   `read_thrift_without_stats` sono tolti. Le statistiche lette occupano i
   byte che hanno nel column chunk, come la pagina.
2. Il salto di pagine intere avanzava dall'header senza decodificare la
   pagina: un contenuto troncato in una pagina saltata passava, e il salto
   contava le sue righe. Ora ogni salto non è qualificato
   (`SKIP_NOT_QUALIFIED`, testo «Parquet skip not qualified for
   reading»): `skip_records` di una o più righe e `skip_next_page` del
   lettore di pagine. Un salto di zero righe non legge nulla. `plenora-io`
   non salta: non usa `RowSelection`, filtri di riga, `offset` né `limit`,
   e gli scrittori e i 106 file hanno lo stesso esito.
3. L'inventario dei campi Thrift usa il lexer di Rust di `proc-macro2`
   (dev-dependency della sola prova, alla versione già nel lockfile):
   stringhe, raw string e commenti annidati come il compilatore; le
   invocazioni di `thrift_struct!` e `thrift_union*!` con qualunque
   delimitatore; le voci `#[cfg(test)]` escluse come attributo, non come
   testo; i decoder scritti a mano trovati per forma (un `match` di rami
   interi o jolly che legge o salta valori), non dal nome della variabile.
   Una macro sconosciuta con un campo `required`/`optional` intero fa
   fallire la prova. L'estrattore del dodicesimo giro, sui casi sintetici
   nuovi, perdeva i campi dopo una `}` in una stringa e quelli di
   `thrift_struct!{}`/`[]`, contava i campi in un commento annidato e in
   un modulo `#[cfg(test)]` senza a capo, e non vedeva un `match` su
   un'altra variabile né una macro sconosciuta.
4. Determinismo: le prove unitarie usavano `rand::rng()` (un seme dal
   sistema a ogni esecuzione) in tredici moduli e nel generatore
   `rand_gen`; ora un generatore del pacchetto con un seme fisso per
   chiamata e per thread (`util::test_common::rng`), e la prova
   `test_rng_is_the_same_on_every_thread` lo verifica (con `rand::rng()`
   fallisce). Le due soglie di 5 secondi delle prove di `plenora-io`
   (`parquet_dichiarazioni.rs`, `confine.rs`) sono tolte: le prove
   verificano il motivo dell'errore, deterministico.

Prove unitarie del pacchetto con `parquet-testing`: **148** falliscono
(due esecuzioni identiche): le 147 di prima più
`test_skip_page_without_offset_index`, che salta una pagina intera
(`SKIP_NOT_QUALIFIED`). `test_row_selection_inside_a_page_or_on_a_boundary`
verifica che una selezione di tutte le righe le legga, valori compresi, e
che ogni salto, dentro una pagina o di pagine intere, si rifiuti. Le
statistiche di pagina verificate sempre non cambiano nessun esito: le
statistiche che le prove a monte scrivono hanno la forma del loro tipo.

**Dodicesimo giro.**

1. Statistiche `FIXED_LEN_BYTE_ARRAY`: la larghezza esatta dell'undicesimo
   giro rifiutava le statistiche troncate, e lo scrittore di questo crate
   tronca minimo e massimo a 64 byte e li marca non esatti: un FLBA di 65
   byte scritto dal fork non si rileggeva. Ora una statistica esatta
   (`is_*_value_exact` vero o assente) ha la larghezza del tipo; una marcata
   non esatta è un limite non esatto da 1 alla larghezza (il minimo
   troncato è un prefisso, il massimo troncato è incrementato;
   `verifica_statistica_flba`, nel column chunk e nella pagina). pyarrow
   25 e 26 (FLBA di 16, 64, 65 e 200 byte, pagine v1 e v2, con e senza
   dizionario) e DuckDB (`UUID`) scrivono la larghezza piena senza segno:
   si leggono tutti.
2. `Sbbf::new` era pubblico: costruiva un bloom filter da byte qualunque
   aggirando `BLOOM_FILTER_NOT_QUALIFIED`, perdeva un resto (33 byte erano
   un blocco) e con meno di 32 byte `check` indicizzava un filtro vuoto.
   Ora è del crate e rifiuta un bitset vuoto, non fatto di blocchi interi o
   oltre il massimo. Gli altri costruttori pubblici non leggono byte di un
   file: `from_bytes` passa dall'header (`BLOOM_FILTER_NOT_QUALIFIED`),
   `new_with_ndv_fpp` e `new_with_num_of_bytes` costruiscono un filtro
   vuoto per lo scrittore; gli indici si costruiscono da byte solo nel
   crate (`try_from_thrift`, `ReadThrift` non pubblico) e dai decoder
   pubblici, rifiutati.
3. L'inventario dei campi Thrift non dipende più dagli a capo: la prova
   divide i sorgenti in token, legge i campi delle macro `thrift_struct!` e
   quelli dei decoder scritti a mano dai rami del `match` sugli id, non dai
   commenti; una prova sintetica (struttura su una riga, campo spezzato,
   commenti, decoder in un `impl`) lo verifica. I campi sono 60 (prima se
   ne contavano 59).
4. I due decoder dei livelli danno lo stesso messaggio per lo stesso
   stream (lo ha trovato l'oracolo con i messaggi esatti su `80 06`): il
   decoder ottimizzato usa i testi di `RleDecoder` per la fine dei dati,
   il valore più largo della larghezza e i varint troncati o lunghi.
5. `test_row_selection_inside_a_page_or_on_a_boundary` verifica anche i
   valori selezionati, non solo il loro numero.

Prove unitarie del pacchetto: le stesse 147 (due esecuzioni identiche); le
prove nuove di `Sbbf::new` e della regola FLBA passano. Scrittori della
matrice e 106 file di prova: lo stesso esito dei giri precedenti.

**Undicesimo giro.**

1. Indici di pagina e bloom filter non qualificati: ogni loro lettura è un
   errore esplicito (`PAGE_INDEX_NOT_QUALIFIED`, in `parse_page_index`, nei
   decoder pubblici e nel lettore di pagine sulle posizioni dell'offset
   index; `BLOOM_FILTER_NOT_QUALIFIED`, nella lettura dell'header di ogni
   bloom filter). `plenora-io` non li legge (`docs/limiti.md`, «Indici di
   pagina e bloom filter Parquet»). Le verifiche dell'offset index dei
   giri precedenti restano, non raggiungibili.
2. Statistiche `FIXED_LEN_BYTE_ARRAY`: della larghezza del tipo, nel
   column chunk e nella pagina (il lettore delle statistiche di pagina
   riceve ora la larghezza della colonna, `decode_page_con_larghezza`).
   `BYTE_ARRAY` resta a lunghezza variabile, zero compreso. La regola «0 o
   1» del decimo giro valeva per ogni statistica di un byte, anche di un
   FLBA di larghezza 1 (che porta un byte qualunque): ora solo `BOOLEAN`
   (lo ha trovato la prova della larghezza 0, il cui file è ora scritto
   senza statistiche).
3. Ogni campo intero dei tipi Thrift del footer e degli header di pagina
   ha una regola, in un inventario che una prova confronta con i sorgenti
   del fork (`crates/plenora-io/tests/parquet_campi_thrift.rs`: un campo
   nuovo o rinominato senza regola la fa fallire). Le regole mancanti ora
   applicate: `PageEncodingStats.count`, `SizeStatistics` (byte e
   istogrammi), `SortingColumn.column_idx`,
   `RowGroup.total_compressed_size` (prima non letto) e `ordinal`, le righe
   del file: negativi sono un errore.
4. L'oracolo dei due decoder dei livelli confronta il messaggio d'errore
   esatto (il lettore Arrow lo avvolge solo nel suo prefisso).

**Decimo giro.**

1. Statistiche (del column chunk e delle pagine): un `distinct_count`
   negativo diventava un `u64` enorme (i conteggi dei nulli e dei NaN
   negativi si rifiutavano già); un minimo o un massimo di un tipo a
   larghezza fissa più lungo del tipo si troncava in silenzio (`01 00 00
   00 FF` era l'`INT32` 1; solo `INT96` voleva la lunghezza esatta). Ora
   conteggi non negativi e lunghezza esatta per tipo (`BOOLEAN` 1 byte, 0 o
   1; `INT32`, `FLOAT` 4; `INT64`, `DOUBLE` 8; `INT96` 12), con un errore.
   `plenora-io` non legge le statistiche, ma il fork le decodifica con il
   footer: la strada più semplice è la stessa regola di ogni altro campo,
   non scartarle (nessuno scrittore della matrice e nessuno dei 106 file
   le scrive fuori forma). Le statistiche FLBA e `BYTE_ARRAY` non hanno una
   lunghezza fissa e non cambiano.
2. Stessa classe nei metadati Thrift: offset, dimensioni e conteggi di un
   column chunk (`file_offset`, `num_values`, dimensioni compressa e non,
   `data_page_offset`, gli offset e le lunghezze di dizionario, indici di
   pagina e bloom filter, `unencoded_byte_array_data_bytes`) e di un row
   group (`num_rows`, `total_byte_size`, `file_offset`) negativi sono un
   errore alla lettura del footer: diventavano `u64` enormi con `as`, e un
   `data_page_offset` negativo arrivava all'`assert!` di `byte_range` (un
   panico).
3. I conteggi enormi di una corsa bit-packed danno lo stesso errore nei
   due decoder dei livelli («bit-packed run beyond the data»): il decoder
   generico confronta i byte prima di moltiplicare il conteggio.
4. L'oracolo verifica il testo dell'errore di ogni caso guasto nei due
   decoder, e il rifiuto del blocco di 32 gruppi in v2; una prova a parte
   lo verifica a larghezza 2, negli indici.

Due prove a monte sulle statistiche `INT96` aspettano ora il testo del
nuovo errore.

**Prove unitarie del pacchetto, deterministiche.** Le prove a monte che
generano le selezioni di righe e i filtri con un seme casuale
(`with_row_selections`, `with_row_filter`,
`test_row_numbers_with_multiple_row_groups_helper`) usano un seme fisso:
con un seme casuale una selezione cadeva dentro una pagina
(`SKIP_NOT_QUALIFIED`) o su un confine, e l'esito cambiava da
un'esecuzione all'altra (78 o 79 fallimenti). I due casi sono espliciti in
`test_row_selection_inside_a_page_or_on_a_boundary` (dal tredicesimo
giro ogni salto è `SKIP_NOT_QUALIFIED`). Gli
altri semi casuali delle prove generano valori, non selezioni: cinque
esecuzioni complete danno gli stessi fallimenti. Due prove a monte
dichiaravano un dizionario di 4 voci per una pagina vuota
(`test_byte_array_decoder_nulls`, `test_nulls`): ora 0.

Prove unitarie del pacchetto con `parquet-testing`: **148** falliscono, sempre le stesse (verificato su due esecuzioni), ciascuna per una delle cause seguenti.

- indice di pagina non qualificato (`PAGE_INDEX_NOT_QUALIFIED`), 85: `test_fixed_length_binary_column_reader`, `test_int96_single_column_reader_test`, `test_interval_day_time_column_reader`, `test_primitive_single_column_reader_test`, `test_read_with_page_index_enabled`, `test_scan_row_with_selection`, `test_unsigned_primitive_single_column_reader_test`, `test_utf8_single_column_reader_test`, `arrow_writer_page_size`, `test_disabled_statistics_with_chunk`, `test_disabled_statistics_with_page`, `test_page_encoding_statistics_roundtrip`, `test_writer_all_null`, `frontier_handles_global_and_local_exhaustion`, `frontier_reports_invalid_global_row_group_while_peeking`, `metadata_row_count_overflow_is_reported`, `queued_row_groups_encapsulates_plan_transitions`, `test_decoder_all_data`, `test_decoder_clear_all_ranges`, `test_decoder_empty_filters`, `test_decoder_filter_with_limit_short_circuits_within_row_group`, `test_decoder_filter_with_limit_skips_later_row_groups`, `test_decoder_filter_with_offset_and_limit`, `test_decoder_filter_with_row_selection_and_limit`, `test_decoder_incremental`, `test_decoder_multi_filters`, `test_decoder_multi_filters_with_limit`, `test_decoder_offset_limit`, `test_decoder_partial`, `test_decoder_reuses_filter_pages`, `test_decoder_row_group_local_selections`, `test_decoder_row_group_selection`, `test_decoder_row_selection`, `test_decoder_selection_does_one_request`, `test_decoder_single_filter_and_row_selection`, `test_decoder_single_filter_partial`, `test_decoder_try_next_reader_offset_limit`, `test_empty_row_group_local_selections_read_nothing`, `test_into_builder_allowed_while_iterating_handed_off_reader`, `test_into_builder_expand_projection_requests_new_bytes`, `test_into_builder_installs_filter_between_row_groups`, `test_into_builder_narrow_projection_requests_fewer_bytes`, `test_into_builder_preserves_buffered_bytes`, `test_into_builder_preserves_local_selections_with_row_filter`, `test_into_builder_preserves_remaining_row_group_local_selections`, `test_into_builder_rejected_mid_row_group`, `test_into_builder_rejected_on_finished_decoder`, `test_into_builder_resumes_remaining_budget`, `test_legacy_row_groups_and_row_selection_remain_composable`, `test_peek_next_row_group_basic`, `test_peek_next_row_group_budget_drains_all`, `test_peek_next_row_group_does_not_mutate_state`, `test_peek_next_row_group_during_decoding_row_group`, `test_peek_next_row_group_finished`, `test_peek_next_row_group_respects_with_row_groups`, `test_peek_next_row_group_skips_empty_selection`, `test_peek_next_row_group_skips_for_budget`, `test_row_group_local_selection_is_mutually_exclusive_with_legacy_configuration`, `test_row_group_local_selection_replaces_previous_configuration`, `test_row_group_local_selection_validates_row_group_and_length_at_build`, `test_row_group_local_selections_allow_duplicate_row_groups`, `test_row_group_local_selections_respect_offset_and_limit`, `test_row_group_local_selections_with_row_filter`, `test_short_row_group_local_selection_skips_trailing_rows`, `test_metadata_read_write_roundtrip_page_index`, `test_metadata_decoder_all_data`, `test_metadata_decoder_incremental`, `test_metadata_decoder_prefetch_retry`, `test_metadata_decoder_prefetch_success`, `test_try_parse`, `test_file_reader_filter_row_groups_and_range`, `test_filtered_rowgroup_metadata`, `test_fixed_length_index`, `test_page_index_reader`, `test_page_index_reader_all_type`, `test_peek_page_with_dictionary_page`, `test_skip_next_page_with_dictionary_page`, `test_skip_page_with_offset_index`, `test_byte_array_size_statistics`, `test_disabled_statistics`, `test_int96_interop`, `test_rewrite_missing_column_index`, `test_rewrite_no_page_indexes`, `test_size_statistics_with_repetition_and_nulls`, `test_spliced_write`.
- codifica non qualificata (`ENCODING_NOT_QUALIFIED`: `DELTA_*`, `BYTE_STREAM_SPLIT`; `test_byte_stream_split_extended` per l'API a righe, che non stampa l'errore), 38: `test_byte_array_decoder`, `test_byte_array_decoder_nulls`, `test_byte_array_decoder_skip`, `test_dictionary_fallback`, `test_dictionary_skip_fallback`, `test_nulls`, `test_byte_array_string_view_decoder`, `test_read_extended_byte_stream_split`, `test_read_float32_float64_byte_stream_split`, `all_null_primitive_single_column`, `arrow_writer_binary`, `arrow_writer_non_null`, `binary_column_bloom_filter`, `binary_single_column`, `empty_string_null_column_bloom_filter`, `f32_single_column`, `f64_single_column`, `fallback_flush_data_page`, `i16_single_column`, `i32_column_bloom_filter`, `i32_column_bloom_filter_at_end`, `i32_column_bloom_filter_fixed_ndv`, `i32_single_column`, `i64_single_column`, `i8_single_column`, `large_binary_single_column`, `large_string_single_column`, `statistics_null_counts_only_nulls`, `string_single_column`, `u16_single_column`, `u32_min_max`, `u32_single_column`, `u64_min_max`, `u64_single_column`, `u8_single_column`, `test_column_writer_dictionary_disabled_v2`, `test_byte_stream_split_extended`, `test_byte_stream_split_extended_roundtrip`.
- salto o selezione di righe, anche di pagine intere (`SKIP_NOT_QUALIFIED`), 10: `test_list_selection`, `test_list_selection_fuzz`, `test_list_skip`, `test_read_nested_lists`, `test_row_numbers_with_multiple_row_groups`, `test_row_numbers_with_multiple_row_groups_and_filter`, `test_skip_optional_records`, `test_skip_required_records`, `test_skip_records_v2_page_skip_accounts_for_partial`, `test_skip_page_without_offset_index`.
- bloom filter non qualificato (`BLOOM_FILTER_NOT_QUALIFIED`), 6: `i64_column_bloom_filter_across_dictionary_fallback`, `i64_column_bloom_filter_populated_from_dictionary`, `string_column_bloom_filter_across_dictionary_fallback`, `string_column_bloom_filter_populated_from_dictionary`, `test_bloom_filter_header_size_assumption`, `test_sbbf_write_round_trip`.
- dichiarazioni incoerenti costruite dalla prova, rifiutate dalla patch `-eof` e dal controllo delle dimensioni, 5: `test_delta_bit_packed_padding`, `test_delta_bit_packed_skip_wide_miniblocks`, `test_decode_unsupported_page`, `test_page_writer_data_pages`, `test_page_writer_dict_pages`.
- colonna FLBA letta come dizionario Arrow (`FLBA_AS_DICTIONARY`), 2: `test_read_dict_fixed_size_binary`, `test_fixed_size_binary_in_dict`.
- tipo testo su byte non annotati (`STRING_WITHOUT_ANNOTATION`), 2: `test_read_binary_as_utf8`, `test_read_non_utf8_binary_as_utf8`.

**Nono giro.**

1. I booleani `RLE` ricevevano la coda di fastparquet (`02 00 00 00 02 01
   00`): ora finiscono alla lunghezza prefissata, senza coda.
2. Una corsa bit-packed che dichiarava più gruppi di quanti ne portava si
   leggeva fino ai suoi byte (come a monte e in C++, `test_truncated_rle`),
   anche in v2 e negli indici (`05 FF` su 8 righe, `01 41` senza byte). Ora
   i gruppi dichiarati sono tutti nei dati, nei due decoder dei livelli e
   in `RleDecoder`. Nessuno scrittore della matrice e nessuno dei 106 file
   di prova ne scrive una. Le prove unitarie a monte con dati scritti a
   mano con l'ultimo gruppo troncato hanno ora il gruppo intero;
   `test_truncated_rle` verifica il rifiuto.
3. Un conteggio enorme (`81 80 80 80 04`, 2^29 gruppi) era un errore di
   overflow nel decoder generico e tagliato ai byte presenti in quello
   ottimizzato: ora lo stesso rifiuto nei due.
4. Due pagine di dizionario nello stesso column chunk: i lettori Arrow
   sostituivano il dizionario in silenzio, l'API generica rifiutava. Ora
   lo stesso errore («more than one dictionary»).
5. Una pagina di dizionario dichiarata `RLE_DICTIONARY` si leggeva come
   `PLAIN`: un dizionario è `PLAIN` o `PLAIN_DICTIONARY`, ora l'altra è un
   errore. Nessuno scrittore della matrice e nessuno dei 106 file la usa;
   il generatore delle prove unitarie e le prove a monte che la usavano
   dichiarano `PLAIN`.
6. L'oracolo dei due decoder dei livelli copre pagine v1 e v2, 1-40 righe,
   blocchi di 32 gruppi, valori coerenti con i livelli, corse vuote,
   varint non minimi, corse troncate e conteggi enormi
   (`l_oracolo_dei_livelli_copre_v2_blocchi_e_guasti`); sul fork
   dell'ottavo giro trova la divergenza del conteggio enorme e la corsa
   troncata letta.

**Ottavo giro.**

1. Il blocco di DuckDB arrivava anche ai booleani `RLE` (255 booleani in
   più), e un blocco di cui non si leggeva nessun valore passava (una
   pagina tutta nulla, 256 indici in più). Ora solo livelli e indici, e
   solo se almeno un valore del blocco si legge.
2. Una corsa vuota prima dei valori si saltava e la corsa dopo cancellava
   il segno (`01 80 00 02 00` negli indici, anche in v2; livelli `01 02
   01`), e il decoder ottimizzato dei livelli leggeva oltre una corsa vuota
   dove quello generico si fermava. Ora una corsa vuota chiude lo stream
   ovunque sia, nei due decoder, e un varint non minimo è un errore. Un
   oracolo confronta i due decoder dei livelli su 1500 stream
   deterministici (`i_due_decoder_dei_livelli_danno_lo_stesso_esito`):
   sul fork del settimo giro trova la divergenza.
3. Una pagina v2 che cominciava dentro un record (`[1, 0]`, una riga)
   passava, e il suo primo livello si attaccava al record precedente. Ora
   il primo livello di ripetizione di una pagina v2 deve essere 0; la
   compatibilità per i record su più pagine v2 è tolta (`docs/limiti.md`,
   «Record su più pagine Parquet v2»).

**Settimo giro.**

1. Le righe di una pagina v2 di una colonna ripetuta si contano alla fine
   della pagina (i livelli di ripetizione 0 letti, `righe_iniziate`); il
   salto su una colonna ripetuta, che non li legge, non è qualificato. Il
   costruttore di pagine delle prove unitarie dichiarava `num_rows` uguale
   ai livelli: ora conta gli zeri di ripetizione.
2. Le corse vuote in coda (un indicatore RLE 0, anche come varint non
   minimo, o una corsa bit-packed di zero gruppi) si leggevano prima della
   fine della pagina e i loro byte non si misuravano: ora la coda si misura
   da dove cominciano (`RleDecoder` e il decoder ottimizzato dei livelli).
3. FLBA nel lettore Arrow: il resto di una pagina `PLAIN` si rifiutava in
   `set_data`, prima che la fine della pagina potesse riconoscere la coda
   di fastparquet; ora il resto si giudica alla fine della pagina.
4. La dimensione non compressa di una pagina non decompressa si confronta
   con i suoi byte; nell'offset index la dimensione di una pagina si
   confronta con il suo header.
5. Le due deviazioni valgono solo nelle pagine v1, e quella di DuckDB solo
   a larghezza sopra 0.

Scrittori reali (11 ottobre 2026), su tipi comuni con null, stringhe,
date, timestamp, dizionari e liste, da 3 a 20000 righe, con e senza
compressione: pyarrow 25.0.1 e 26.0.0 (pagine v1 e v2, con e senza
dizionario), fastparquet 2026.9.0 (pagine v1 e v2), DuckDB 1.5.6 (`COPY TO`,
`PARQUET_VERSION` V1 e V2), polars 2.0.0 (scrittore proprio e pyarrow).
Senza le tolleranze fastparquet e DuckDB V1 si rifiutavano tutti; con
esse ogni file si legge, tranne DuckDB V2, che usa codifiche `DELTA_*`
(`Unsupported`, come prima). Le pagine v2 tutte nulle compresse (pyarrow e
fastparquet, SNAPPY e ZSTD) si leggono; GZIP, BROTLI e LZ4 non sono
compilati (`Unsupported`, come prima). Letti con `plenora-io` e
direttamente, i 106 file di prova hanno lo stesso esito del quarto giro.

Prove unitarie del pacchetto con `parquet-testing`: 79 falliscono, le 52
di prima più 27 attese, tutte su salti dentro una pagina o selezioni di
righe (`SKIP_NOT_QUALIFIED`). Il generatore casuale delle pagine
(`util/test_common/rand_gen.rs`) comincia ogni pagina con un livello di
ripetizione 0 (una pagina v2 deve); i casi negativi sono deterministici
(`test_v2_page_record_boundary_and_rows_are_checked`).

### Profondità dello schema e tetto di memoria del footer (10 ottobre 2026)

Da `patches/parquet-footer-budget.patch`: è il delta del fork di
plenora-IO-tools (PR #47, `vendor/parquet` a `dd96c3a`, con le due
correzioni chieste dalla sua revisione), adottato identico; la patch
porta anche la formattazione delle righe dei delta precedenti che quel
fork ha già, così i due alberi coincidono (tolti `data_type.rs` della
patch sopra e `parquet-decoder.patch`, solo di qui per ora).

1. `parquet` converte lo schema del footer per ricorsione senza limite: uno
   schema valido di qualche migliaio di livelli esauriva lo stack, un
   aborto. `ParquetMetaDataOptions::max_schema_depth` (default
   `DEFAULT_MAX_SCHEMA_DEPTH`, 64) lo rifiuta prima della conversione, con
   una visita iterativa (`schema_cost`); anche `parquet_schema_from_bytes`.
2. `footer_memory_budget`: ogni prenotazione del decoder del footer si
   addebita prima di farla (il buffer del footer due volte, ogni elenco di
   `read_thrift_vec`, i row group, la capacità di colonne di ogni row group
   prima di `RowGroupMetaDataBuilder::new`, il costo dei percorsi delle
   foglie, quadratico nei byte del footer). Oltre il tetto è un errore.
3. Column index e offset index si decodificano senza tetto: con un tetto
   sono rifiutati prima di leggerne un byte (lettore Arrow e seriale).

`plenora-io` passa la profondità e il tetto (`budget_del_footer`) a ogni
lettura; un rifiuto è `ResourceLimit`. Prove in
`crates/plenora-io/tests/parquet_footer.rs`; i file dello schema profondo
e del costo quadratico sono semi del fuzz (`tests/dati/fuzz-footer/`).

## Limiti che restano

Correzione mirata, non un irrobustimento completo contro file ostili
(scelte del 2 e del 4 ottobre 2026, limite «File costruiti apposta» in
`docs/file.md`).

- Espansioni vere, non dichiarazioni: un dizionario di testi ripetuto su
  molte righe, o una colonna `FixedLenByteArray` larga e tutta nulla,
  occupano davvero memoria proporzionale alle righe. Le copre il budget
  (`stima_decodificata`, che per riga conta la larghezza dichiarata anche
  quando il chunk dichiara meno valori delle righe, per un fattore di
  decodifica fisso), non la patch.
- Le prove unitarie interne di `parquet` che chiamano
  `DeltaBitPackDecoder::set_data` con `0` non sono aggiornate: il vendor non
  è un membro del workspace e quelle prove non si compilano qui.
- Bloom filter (`bloom_filter/mod.rs`): la lettura riserva la lunghezza
  dichiarata, ma `plenora-io` non legge i bloom filter (opzione del lettore
  non attiva), quindi non è raggiungibile.

- Le lunghezze di binari si convertono in `usize` con `as`: su un target a
  32 bit una lunghezza oltre `u32` si troncherebbe (il workspace gira a
  64 bit).
- `read_bytes_owned` alloca quanto il file contiene davvero (non più quanto
  dichiara): una statistica di pagina grande quanto il file occupa memoria
  proporzionale al file, che il confine di lettura ha già misurato.
- Il percorso cifrato degli header (`encryption/decrypt.rs`, `read_and_decrypt`)
  alloca dalla lunghezza dichiarata; la feature `encryption` non è abilitata
  (`Cargo.toml`), quindi non è raggiungibile. Va corretto prima di abilitarla.

Seconda lettura della patch: Codex (2 ottobre 2026), sui due lettori e
sulle codifiche valide di pyarrow, parquet-rs, Spark e DuckDB (analisi del
sorgente, non una matrice eseguita); le correzioni 8-13 hanno la loro
seconda lettura nella PR che le introduce.

Il difetto è anche su `main` di arrow-rs (2 ottobre 2026): la segnalazione
upstream è da fare; quando una release lo corregge, il vendor si toglie e
`parquet` torna dal registro.
