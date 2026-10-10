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
  ```

  Il risultato è questa cartella byte per byte, tolto questo file
  (verificato il 10 ottobre 2026 dal `.crate` con il checksum sopra).
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
    dizionario corto; ora `checked_mul`, e un overflow è un errore.

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
cercata in tutti i decoder di `encodings/`, `arrow/decoder/`,
`arrow/array_reader/`, `arrow/buffer/` e nei livelli delle pagine. Ogni
valore letto dal file che diventa una lunghezza, un indice o un estremo
di slice passa da una conversione fallibile e da aritmetica controllata;
il caso limite è un errore. Prove rosse in
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
   `BIT_PACKED` dichiarati oltre la pagina (`column/reader.rs`); una pagina
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
che questo workspace non chiede.

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
