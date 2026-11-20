# Provenienza di plenora-rstar 0.12.2

Base: rstar 0.12.2, pacchetto crates.io SHA-256
`421400d13ccfd26dfa5858199c30a5d76f9c54e0dba7575273025b43c5175dbb`.
Gli algoritmi R-tree restano identici. La dipendenza heapless passa da 0.8
al pin =0.9.4 per RUSTSEC-2026-0574..0582; nessuna eccezione di sicurezza.

Le implementazioni per Coord, Point, Line, LineString, Polygon, MultiPolygon,
Rect e Triangle e private_utils provengono da geo-types 0.7.20, SHA-256
`777d18aa0f12f8b285331cd867133ee14422b3f023f6d388034c47d43e28786a`. Sono trasferite senza cambiare i calcoli; i percorsi dei tipi
sono adattati e Polygon usa exterior() al posto del campo privato.
Le licenze di entrambi i pacchetti sono conservate in questa directory.

Il trasferimento permette di disattivare rstar_0_12 di geo-types: quella
feature dipende da rstar upstream e reintrodurrebbe heapless 0.8. Il fork geo
usa i medesimi helper tramite rstar::private_utils. Una sola geo-types resta
nel grafo. Il nome proprio e le dipendenze per percorso proteggono anche i
consumatori esterni; non occorre una patch nel workspace del chiamante.

Patch riproducibile rispetto al pacchetto rstar: ../../patches/rstar-heapless.patch.
La configurazione Cargo.toml.orig resta il manifesto originale upstream.
Rimozione: migrazione coordinata a rstar >=0.14 e geo-types con integrazione
compatibile, riqualificando API, ordinamento delle parita e oracoli spaziali.

## Attribuzioni R-tree

I testi LICENSE-MIT e LICENSE-APACHE di rstar sono ripresi dal registro
verificato precedente del repository (assurance/licenze-crate/rstar-0.12.2).
Il pacchetto crates.io non li includeva. Provenienze dei testi:

- LICENSE-APACHE: https://github.com/georust/rstar/blob/c8c5bf9e41468281d030869d409e3f67f1aaf5b7/LICENSE-APACHE; SHA-256 `a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2`.
- LICENSE-MIT: https://github.com/georust/rstar/blob/c8c5bf9e41468281d030869d409e3f67f1aaf5b7/LICENSE-MIT; SHA-256 `9b3a76c27bac8f3e0ff2d1f64f65941c50310ae3ff69318cea230bd4a36822c5`.

Il nome proprio richiede di seguire esplicitamente anche gli avvisi per
`rstar` upstream: un audit per nome non li attribuisce automaticamente a
`plenora-rstar`. `heapless` conserva invece identita e sorgente crates.io
ed e controllato normalmente da audit/deny. Nessuna advisory e derogata.
