# Filtro veloce di `orient2d`

**Adottato**: `geo` risolve a questa cartella (pacchetto `plenora-geo`,
`Cargo.toml`, «Copie vendorizzate»). Il nome «sperimentale» viene dal progetto d'origine, dove
il filtro nacque accanto a un candidato sempre-esatto senza filtro
(`vendor/geo-0.33.1-exact`, non portato qui).

**Che cosa aggiunge.** Sopra le patch di `PROVENANCE.md` (orientamento
esatto e logging), `patches/orient2d-filtro-sperimentale.patch` aggiunge
davanti a `RobustKernel::orient2d` un filtro veloce
(`src/algorithm/kernels/orient2d_filtered.rs`) che certifica il segno con
un limite d'errore dimostrato quando possibile, e ricade sul kernel esatto
invariato (`exact_orientation.rs`, non toccato dalla patch) in ogni altro
caso.

**Perche' esiste.** Il kernel sempre-esatto e' molto lento sulle
operazioni che chiamano il predicato di orientamento molte volte per
geometria (`buffer`, e la validazione OGC che ogni kernel invoca in
ingresso): nel progetto d'origine un test dell'executor non convergeva in
5 minuti a 2000 vertici, contro 10,83 s del predicato originale.

**Qualifica del filtro.** Progettato e qualificato in un banco separato
del progetto d'origine: 6788 casi contro l'oracolo razionale
(`fractions.Fraction`), compresi i reperti storici del laboratorio, le sei
permutazioni note che espongono il difetto del vecchio kernel (`robust`
crate v1.2.0, 220/1266 fallimenti), una griglia sistematica di confini
rappresentabili (soglia di sicurezza, `f64::MIN_POSITIVE`, overflow di
`detsum`, il confine dell'errbound trovato per bisezione sul bit pattern),
e 5000 triple quasi-collineari casuali a scale da 1e-290 a 1e290. Tutti e
quattro i rami del filtro esercitati; 0 fallimenti in debug e in release.
Un controesempio reale e' stato trovato e corretto durante la qualifica
(sottoflusso di un prodotto trattato erroneamente come zero genuino): il
commento di modulo di `orient2d_filtered.rs` porta la derivazione completa
del limite d'errore e delle condizioni di applicabilita'.

**Verifica.** La ricostruzione di questa cartella dal candidato esatto
piu' la patch, con il confronto dei digest, la facevano gli script di
verifica del progetto d'origine, che questo repository non ha portato.
