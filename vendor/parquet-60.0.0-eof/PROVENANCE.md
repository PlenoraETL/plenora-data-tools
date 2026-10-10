# Provenienza — `parquet` 60.0.0, eof

**Adottato**: è il `parquet` del workspace, di `fuzz/` e di chi dipende dai
crate di data-tools: pacchetto `plenora-parquet`, dipendenza per percorso
con la chiave `parquet` (`Cargo.toml`, «Copie vendorizzate»).

- Pacchetto: `parquet-60.0.0.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto (dal `Cargo.lock` prima del vendor):
  `8af83d2940bc0510f9aef86d865f56fdc6095f87ab115ac885a80b7c5226d3ba`.
- Contenuto: il pacchetto pubblicato, intero, tolto solo il marcatore di
  Cargo `.cargo-ok`; i file cambiati sono quelli di
  `patches/parquet-eof.patch` e il nome del pacchetto in `Cargo.toml`
  (`patches/parquet-nome-proprio.patch`: `plenora-parquet`, la libreria
  resta `parquet`; una `[patch.crates-io]` non vale per chi dipende dai
  crate di data-tools). Il pacchetto più la patch ricostruisce questa
  cartella byte per byte (verificato il 4 ottobre 2026 estraendo il
  `.crate` dal checksum sopra e applicando la patch con `patch -p1`).
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
