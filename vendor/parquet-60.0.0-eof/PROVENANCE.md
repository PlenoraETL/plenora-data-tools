# Provenienza — `parquet` 60.0.0, eof

**Adottato**: è il `parquet` del workspace e di `fuzz/` (`[patch.crates-io]`
nei due `Cargo.toml`).

- Pacchetto: `parquet-60.0.0.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto (dal `Cargo.lock` prima del vendor):
  `8af83d2940bc0510f9aef86d865f56fdc6095f87ab115ac885a80b7c5226d3ba`.
- Contenuto: il pacchetto pubblicato, intero, tolto solo il marcatore di
  Cargo `.cargo-ok`; i file cambiati, con
  `patches/parquet-eof.patch`, sono `src/parquet_thrift.rs`,
  `src/file/metadata/thrift/mod.rs` e `src/file/page_index/offset_index.rs`.
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

## Limiti che restano

Correzione mirata, non un irrobustimento completo contro file ostili
(scelta del 2 ottobre 2026, limite «File costruiti apposta» in
`docs/file.md`). La seconda lettura di Codex ha trovato, anche upstream,
altre allocazioni da dimensioni dichiarate prima di verificarle:

- `schema/types.rs` riserva i figli dichiarati da `num_children`;
- `file/reader.rs` e `file/serialized_reader.rs` riservano la dimensione
  compressa di una pagina (o del bloom filter) prima di leggerla, e quella
  non compressa prima di decomprimere;
- i dizionari (`encodings/decoding.rs`, `arrow/array_reader/byte_array*.rs`)
  riservano il conteggio dell'intestazione di pagina;
- `arrow/array_reader/fixed_len_byte_array.rs` moltiplica la larghezza
  dichiarata nello schema per le righe richieste;
- le codifiche delta riservano il conteggio dichiarato nella pagina.

Un file piccolo costruito apposta può quindi chiedere gigabyte. Il target di
fuzz `lettura_parquet` gira con `-malloc_limit_mb` e `-ignore_ooms`
(README, «Fuzz»): questi casi si contano senza fermare la campagna.

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
sorgente, non una matrice eseguita).

Il difetto è anche su `main` di arrow-rs (2 ottobre 2026): la segnalazione
upstream è da fare; quando una release lo corregge, il vendor si toglie e
`parquet` torna dal registro.
