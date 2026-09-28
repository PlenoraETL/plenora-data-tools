# Candidato sperimentale filtrato — separato dal riferimento congelato

**Non e' il candidato adottato.** Questo vendor e' una copia di
`vendor/geo-0.33.1-exact` (stessa provenienza, vedi `PROVENANCE.md` in
questa stessa directory) con una sola aggiunta sopra `RobustKernel::orient2d`:
un filtro veloce sperimentale (`src/algorithm/kernels/orient2d_filtered.rs`)
che certifica il segno con un limite d'errore dimostrato quando possibile, e
ricade sul kernel esatto invariato (`exact_orientation.rs`, non modificato
da questa aggiunta — stesso file, stesso digest del congelato) in ogni altro
caso.

**Perche' esiste.** Il candidato sempre-esatto e' catastroficamente lento su
operazioni che chiamano il predicato di orientamento molte volte per
geometria (`buffer`, e la validazione OGC che ogni kernel invoca in
ingresso) — diagnosticato su
`executor::tests::geo_fusion_falls_back_when_the_governor_rejects_the_reservation`:
non converge in 5 minuti a 2000 vertici contro 10,83s del predicato
originale. Vedi `docs/errori-e-limiti.md` per la misura completa.

**Provenienza del filtro.** Progettato, derivato e qualificato nel banco
standalone `plenora-memlab-filtro-sperimentale/` (albero fratello, fuori da
questo): 6788 casi contro l'oracolo razionale (`fractions.Fraction`),
comprendenti i reperti storici del laboratorio, le sei permutazioni note che
espongono il difetto del vecchio kernel (`robust` crate v1.2.0, 220/1266
fallimenti), una griglia sistematica di confini rappresentabili (soglia di
sicurezza, `f64::MIN_POSITIVE`, overflow di `detsum`, il confine
dell'errbound trovato per bisezione sul bit pattern), e 5000 triple
quasi-collineari casuali a scale da 1e-290 a 1e290. Tutti e quattro i rami
del filtro esercitati; 0 fallimenti in debug e in release. Un controesempio
reale e' stato trovato e corretto durante la qualifica (sottoflusso di un
prodotto trattato erroneamente come zero genuino) — vedi il commento di
modulo di `orient2d_filtered.rs` per la derivazione completa del limite
d'errore e delle condizioni di applicabilita'.

**Provenienza verificata in questo repository, separata dal candidato
congelato.** `scripts/verifica_filtro_sperimentale.py` (gate CI dedicato,
`.github/workflows/ci.yml`) ricostruisce questo vendor da
`vendor/geo-0.33.1-exact` **gia' verificato** (da
`scripts/verifica_vendor_provenienza.py`, non ripetuto qui) piu'
`patches/orient2d-filtro-sperimentale.patch` (identita' per sha256), e
pretende che il digest coincida con l'albero committato. Verifica inoltre
che `orient2d_filtered.rs` sia byte-per-byte il file qualificato
(sha256 registrato nello script) e che `exact_orientation.rs` resti
identico al congelato. `scripts/verifica_risoluzione_vendor.py` in questo
albero e' stato adattato di conseguenza (`geo` risolve deliberatamente a
`geo-0.33.1-exact-filtered`, non al congelato) — la copia dello stesso
script nell'albero del candidato congelato resta invariata.

**Stato.** Sperimentale. Nessuna sostituzione del candidato congelato,
commit, push o VM senza autorizzazione esplicita — vedi il mandato che ha
aperto questo lavoro.
