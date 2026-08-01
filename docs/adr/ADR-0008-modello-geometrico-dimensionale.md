# ADR 8 — Modello geometrico dimensionale (ICD §3)

- **Stato**: accettato e attuato (Fase B1)
- **Decisioni collegate**: D16; ICD §3.2, §3.3, §3.4, §3.5 (ratificate nel
  registro di `plenora-contracts`, v2.0-draft)
- **Riferimenti**: `Architetture.md` §4.2; ADR 1 (determinismo)

## Contesto

L'ICD trasversale impone che ogni componente **rappresenti e propaghi** le
cinque dimensionalità canoniche (`xy`, `xyz`, `xym`, `xyzm`, `unknown`) anche
quando non sa elaborarle (§3.3), senza mai degradare o approssimare in
silenzio (§3.2, §3.4). Il motore decodifica le geometrie in
`geo::Geometry<f64>` (solo XY): decodificare Z/M le distruggerebbe. La scelta
è quindi tra rifiutare tutto (stato precedente, conforme ma insufficiente per
la propagazione) e un modello a due vie.

## Decisione

1. **`GeometryDimensions` a cinque varianti** (`Xy`, `Xyz`, `Xym`, `Xyzm`,
   `Unknown`) con serde nella forma ICD minuscola. `Unknown` = byte preservati,
   dimensionalità non risolta: **mai mappata a `Xy`** (§3.4). Un campo senza
   metadato dimensionale è `Unknown`, mai `Xy` di default.
2. **Due vie, decise a compile-plan**:
   - *passthrough* (op tabellari, trasporto, publish): byte e metadati
     propagati invariati per qualunque dimensionalità;
   - *elaboranti* (kernel geo che decodificano in `Geometry<f64>`): rifiuto
     tipizzato `Unsupported` per `dimensions != Xy` — **incluso `Unknown`**
     (accettarlo equivarrebbe a mapparlo a `Xy`) — in fase di analisi del
     piano, mai a metà esecuzione. L'errore cita operazione e dimensionalità,
     mai dati.
3. **Validatore WKB stride-aware**: coerenza type-code ↔ dimensionalità del
   contratto al gate d'arco, con stride 16/24/32; le ordinate extra sono
   saltate, mai lette né validate (finitezza e chiusura anelli solo su X/Y —
   le Z/M non sono elaborate). `Unknown` deriva lo stride dal type code di
   ogni geometria.
4. **`GeometryEncoding` enum chiuso** (`Wkb`, `Ewkb`; §3.5): dichiarato nel
   contratto come `Option` (chiave omessa se assente → fingerprint invariati)
   e nei metadati solo quando dichiarato. Framing non rappresentabili
   (GeoPackage header, TWKB) rifiutati in discovery. Il gate di trasporto legge
   lo SRID embedded EWKB e lo confronta con lo SRID del contratto: match → byte
   e metadati invariati nei percorsi passthrough; mismatch → errore `Crs` prima
   dell'output. I decoder dei kernel elaboranti continuano a rifiutare lo SRID
   embedded finché non possono preservarlo. **EWKB puro-XY è byte-identico a
   ISO** e passa i gate come `xy`: la validazione è sui type code e sulla
   coerenza SRID, mai sulla sola chiave `encoding`.
5. **Produttori** (`from_wkt`, `from_coords`, `generate_grid`, output
   ricodificati) dichiarano sempre `Xy`; la riproiezione preserva
   dimensionalità ed encoding dichiarati nei metadati riscritti.

## Conseguenze

- Test sistematico sulle 75 op del catalogo: elaboranti → rifiuto per
  xyz/xym/xyzm/Unknown; passthrough → propagazione invariata; produttori →
  `Xy` coerente con i metadati.
- Round-trip end-to-end verificato: Point Z → filtro tabellare → publish →
  celle byte-identiche e metadato `xyz` preservato.
- Incoerenza celle ↔ contratto (metadato `xyz`, celle XY) → errore dedicato
  al gate, mai passthrough silenzioso.

## Fuori scope (dichiarato)

Elaborazione Z/M nei kernel, CRS 3D (PROJJSON tridimensionale non collegato a
`dimensions`), backend GEOS con Z/M, ri-validazione degli archi intermedi
prodotti da kernel geo (immutata dal comportamento precedente).
