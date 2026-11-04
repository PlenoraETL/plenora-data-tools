# Fixture IO 7.0.0

`epoch-covering.arrow` e' l'uscita reale del binario in sola lettura
`C:/pg/io-700/target/debug/plenora-io.exe`, versione 7.0.0, generata il
3 novembre 2026 con `convert source.parquet output.arrow --from geoparquet --to ipc`.
Sorgente sintetica: PyArrow 25.0.1, GeoParquet 1.1, EPSG:32632 dalla tabella
PROJJSON del repository, POINT(500000 5000000), epoch 2023.5, edges planar,
bbox e covering dei quattro estremi nella struct bbox. IO elimina la
struct bbox e mantiene il covering nel contenitore nativo.
SHA256: `c96c634fcbe08977308aa7d10d165a7980f098c25495e5608af00c078677441c`.
