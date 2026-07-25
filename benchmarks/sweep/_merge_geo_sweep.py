#!/usr/bin/env python3
"""Merge benchmarks/sweep/geo_sweep.jsonl (prefisso run #6 + coda run #7) nei
file finali geo_sweep.json / geo_sweep.md, nello stesso formato prodotto da
write_outputs dell'example bench_geo_sweep."""
import json
from pathlib import Path

SWEEP = Path("benchmarks/sweep")
entries = []
seen = set()
for line in (SWEEP / "geo_sweep.jsonl").read_text(encoding="utf-8").splitlines():
    line = line.strip()
    if not line:
        continue
    entry = json.loads(line)
    if entry["op"] in seen:
        continue
    seen.add(entry["op"])
    entries.append(entry)

document = {
    "benchmark": "bench_geo_sweep",
    "seed": 42,
    "container": "docker run --cpus=4 --memory=10g rust:1.92 (cargo run --release)",
    "logical_cpus": 4,
    "target_rep_seconds": 4.0,
    "calibration_cells": 20000,
    "results": entries,
}
(SWEEP / "geo_sweep.json").write_text(
    json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
)

sorted_entries = sorted(
    entries, key=lambda e: (e["kind"] == "skipped", e["geoms_per_second"])
)

def fmt_share(share):
    return f"{share:.2f}" if share is not None else "-"

def fmt_mib(kib):
    return f"{kib // 1024}" if kib is not None else "n/d"

lines = []
lines.append("# Sweep kernel geografici (bench_geo_sweep)\n")
lines.append(
    "Fixture deterministiche (seed 42), mediana di 3 run, container Docker\n"
    "`--cpus=4 --memory=10g`, release. Le op per-cella misurano la pipeline\n"
    "decode WKB + kernel + encode WKB (come l'adapter Arrow); quelle\n"
    "collettive includono il decode dell'intera tabella. `decode_share` e'\n"
    "la quota del tempo per-cella attribuita a decode(+encode) WKB dai\n"
    "riferimenti `_ref.*`; `bound_class` deriva da soglie 0.5/0.25.\n"
    "Classifica ordinata per lentezza (geom/s crescenti, skipped in coda).\n"
    "Il peak RSS e' il `VmHWM` cumulativo di processo (le fixture lazy\n"
    "restano in memoria).\n"
)
lines.append(
    "| # | op | tipo | fixture | celle | mediana (s) | geom/s | decode share | classe | peak RSS (MiB) | note |\n"
    "|---|----|------|---------|-------|-------------|--------|--------------|--------|----------------|------|"
)
for position, entry in enumerate(sorted_entries, start=1):
    lines.append(
        "| {} | `{}` | {} | {} | {} | {:.4f} | {:.0f} | {} | {} | {} | {} |".format(
            position,
            entry["op"],
            entry["kind"],
            entry["fixture"],
            entry["cells"],
            entry["median_seconds"],
            entry["geoms_per_second"],
            fmt_share(entry["decode_share"]),
            entry["bound_class"],
            fmt_mib(entry["peak_rss_kib"]),
            entry["note"],
        )
    )
(SWEEP / "geo_sweep.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"entries={len(entries)} scritti geo_sweep.json e geo_sweep.md")
