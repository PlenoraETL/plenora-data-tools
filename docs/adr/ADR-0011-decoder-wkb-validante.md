# ADR 11 — Decoder WKB validante: una scansione, stessa garanzia

- **Stato**: accettato (design; attuazione in corso)
- **Decisioni collegate**: ADR 1 (determinismo), ADR 8 (modello
  dimensionale), ADR-0009/deroghe (principio: nessuna garanzia ceduta per
  inferenza sul produttore)
- **Riferimenti**: `Prestazioni.md` V6/G1 (decode minimizzato);
  `plenora-contracts` R0.1 (evidenza riproducibile, non inferenza)

## Contesto

Il percorso di decode di una cella WKB (`geometry_from_wkb`) esegue oggi
**tre passate**: (1) `validate_wkb_contract` — scansione strutturale dei
byte (bound, conteggi, type code, chiusura anelli, finitezza X/Y,
annidamento); (2) `geozero::wkb::Wkb::to_geo()` — seconda scansione
identica dei byte per costruire `geo::Geometry`; (3) `check_validation` —
validazione OGC della geometria costruita. La (2) rilegge integralmente ciò
che (1) ha già verificato: il 33-50% del tempo di decode è doppio lavoro
(analisi performance 2026-07-28, finding geo #3).

La scorciatoia respinta è «l'output di `to_wkb` su geometria validata è
valido per costruzione, quindi le passate si possono saltare»: è un'**inferenza
sul produttore**, esclusa da R0.1 (evidenza riproducibile richiesta) e con
costo dell'errore asimmetrico in un ETL patrimoniale (un WKB invalido che
esce senza intercettazione finisce in un GeoPackage usato come sorgente).
La garanzia fail-closed non si sposta: si elimina il costo *dentro* la
garanzia.

## Decisione

**Un solo decoder che valida durante il decode.** Un nuovo modulo
`wkb_decoder` in `plenora-kernels-geo` esegue UNA passata sui byte che,
nella stessa camminata del validatore strutturale esistente (stesse
regole, stesso ordine di valutazione, stessi errori):

1. verifica byte order, type code (rifiuto SRID/riservati/serie Z-M per il
   protocollo 2D — come `parse_wkb_type_code`);
2. verifica conteggi contro i byte residui (`checked_count`);
3. legge X/Y rifiutando NaN/infiniti (come `read_coordinate`);
4. verifica la chiusura degli anelli (`first == last` esatto);
5. verifica annidamento (`max_depth`), conteggio componenti
   (`MAX_WKB_COMPONENTS`), tipi figli delle multi-geometrie, byte residui;
6. **e nel frattempo costruisce** la `geo::Geometry` (Point, LineString,
   Polygon, Multi*, GeometryCollection) direttamente dalle stesse
   coordinate consumate.

La validazione OGC (`check_validation`, passata 3) resta invariata e
separata: è una verifica topologica sulla geometria costruita, non
strutturale sui byte. `geozero` resta in uso per l'**encode**
(`ToWkb`) — il decoder sostituisce solo il lato lettura.

**Parità esatta di accettazione/rifiuto e di output** con il percorso
precedente è il contratto: ogni payload accettato (rifiutato) dal vecchio
percorso è accettato (rifiutato) dal nuovo, con lo stesso tipo di errore e
la stessa geometria risultante coordinata per coordinata. È verificata
per costruzione (stesse regole nello stesso ordine) e dimostrata da oracolo
differenziale (sotto).

## Conseguenze

- Una scansione invece di due: il costo di decode torna al minimo V6/G1
  senza alcuna cessione di garanzia. La rivalidazione dell'output in
  `transform_wkb` **resta** (nessuna deroga «valido per costruzione»),
  eseguita con lo stesso decoder — anche lei a una passata.
- `geometry_from_wkb` diventa: `wkb_decoder::decode(payload)` +
  `check_validation`. `validate_wkb_contract*` resta per i gate che
  validano senza decodificare (archi IPC, dimensionalità passthrough) —
  il codice non si duplica: il decoder riusa le stesse primitive
  (`WkbCursor`, `checked_count`, `parse_wkb_type_code`).
- La dipendenza `geozero` resta per l'encode; la rimozione dal lato
  lettura riduce la superficie di terza parte sul percorso critico.
- Rischio principale: divergenza di semantica tra decoder nuovo e
  percorso geozero (angoli limite del parser geozero non documentati
  nelle regole strutturali — es. NaN nelle coordinate Z/M, anelli con
  chiusura approssimata). Mitigazione: oracolo differenziale obbligatorio
  prima dell'adozione (proptest + corpus fuzz `wkb_contract` esistente);
  se emergesse una divergenza, vince il comportamento del validatore
  strutturale esistente (la garanzia del progetto), non quello di
  geozero, e la divergenza va dichiarata qui.

## Verifica

- **Oracolo differenziale obbligatorio**: per la fixture completa (validi,
  malformati, annidati, Z/M/SRID rifiutati, anelli aperti, conteggi
  oltre i byte, byte residui, NaN/inf) il nuovo decoder e il percorso
  `validate_wkb_contract` + `Wkb::to_geo` devono produrre: stesso esito
  (Ok/Err), stessa categoria d'errore, stessa geometria (confronto
  coordinata-per-coordinata bit-esatto).
- Suite completa (880 test) verde; fuzz `wkb_contract` esteso con il
  confronto vecchio/nuovo in campagna notturna.
- Benchmark: `bench_geo_perfcheck` ref decode — atteso ~30-45% in meno
  sulle fixture decode-bound (misura registrata nel commit di adozione).

## Stato di attuazione

**Attuato (2026-07-29).** `wkb_decoder` in `plenora-kernels-geo`
(`decode_validated`/`decode_validated_with_depth`): una passata con le
stesse regole del validatore strutturale, stesso ordine, stessi errori;
`geometry_from_wkb` e' stata commutata su di esso (geozero resta solo per
l'encode). Verifiche eseguite:

- **Oracolo differenziale**: 20+ payload (validi, malformati, annidati
  oltre limite, Z/SRID rifiutati, anelli aperti, conteggi oltre i byte,
  byte residui, NaN/inf, endianness miste, vuoti) — stesso esito e
  stessa geometria coordinata-per-coordinata rispetto a
  `validate_wkb_contract` + `Wkb::to_geo` (test in `wkb_decoder.rs`);
  il target fuzz `wkb_contract` esteso con il confronto vecchio/nuovo
  per la campagna notturna.
- **Suite completa**: 888 test verdi; clippy 0/0 su Linux e target
  Windows; gate R6 verde.
- **Benchmark** (`bench_geo_perfcheck`, ref decode): **35,8M punti/s
  contro 18,5M/s del percorso precedente (+94%)** — oltre la stima
  30-45% dell'ADR: la doppia scansione era ~meta' del costo di decode.
  Il per-cella completo (decode+kernel+encode) resta dominato dal
  lavoro geometrico: invariato entro la varianza dell'host.
