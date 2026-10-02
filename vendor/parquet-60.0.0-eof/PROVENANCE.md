# Provenienza — `parquet` 60.0.0, eof

**Adottato**: è il `parquet` del workspace e di `fuzz/` (`[patch.crates-io]`
nei due `Cargo.toml`).

- Pacchetto: `parquet-60.0.0.crate`, `source = registry+https://github.com/rust-lang/crates.io-index`.
- Checksum del pacchetto (dal `Cargo.lock` prima del vendor):
  `8af83d2940bc0510f9aef86d865f56fdc6095f87ab115ac885a80b7c5226d3ba`.
- Contenuto: il pacchetto pubblicato, intero, tolti solo i marcatori di
  Cargo (`.cargo-ok`, `.cargo_vcs_info.json`); l'unico file cambiato è
  `src/parquet_thrift.rs`, con `patches/parquet-eof.patch`.
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

Il salto e le letture sono metodi di default del trait: le correzioni
valgono anche per il lettore su slice (footer), che saltava gli elementi
booleani di una collezione senza avanzare.

## Limiti che restano

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
