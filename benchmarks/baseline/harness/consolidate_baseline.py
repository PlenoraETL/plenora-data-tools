"""Consolida i risultati grezzi in benchmarks/baseline/baseline.json.

Legge i file raw/*.json (output degli harness Rust e degli esempi dei progetti
di origine) e raw/*.time (output di /usr/bin/time -v) e produce il documento
di baseline usato dal benchmark gate (Prestazioni.md par. 9).
"""

from __future__ import annotations

import json
import re
from datetime import datetime, timezone
from pathlib import Path

BASELINE_DIR = Path(__file__).resolve().parent.parent
RAW = BASELINE_DIR / "raw"


def load_json(path: Path):
    data = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(data, list):
        return data[0]
    return data


def parse_time(path: Path) -> dict:
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8", errors="replace")
    result = {}
    elapsed = re.search(
        r"Elapsed \(wall clock\) time.*:\s*([\d:.]+)\s*$", text, re.MULTILINE
    )
    if elapsed:
        parts = elapsed.group(1).split(":")
        seconds = 0.0
        for part in parts:
            seconds = seconds * 60 + float(part)
        result["wall_clock_seconds"] = round(seconds, 3)
    rss = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", text)
    if rss:
        result["peak_rss_kib"] = int(rss.group(1))
    return result


def mib(kib):
    return round(kib / 1024, 1) if kib is not None else None


def tabular_entries() -> list[dict]:
    entries = []
    for path in sorted(RAW.glob("nogeo_*.json")):
        data = load_json(path)
        scale = "1M" if path.stem.endswith("_1M") else "10M"
        name = data.get("operation") or data.get("name")
        entries.append(
            {
                "name": name,
                "scale": scale,
                "rows": data["rows"],
                "repetitions": data.get("repetitions", 5),
                "median_seconds": data["median_seconds"],
                "rows_per_second": round(data["rows_per_second"], 1),
                "input_bytes": data["input_bytes"],
                "output_bytes": data["output_bytes"],
                "peak_rss_kib": data.get("peak_rss_kib"),
                "peak_rss_mib": mib(data.get("peak_rss_kib")),
            }
        )
    return entries


def geo_kernel_entries() -> list[dict]:
    entries = []
    skip = ("transport_", "spatial_join", "line_merge", "split_", "polygonize")
    for path in sorted(RAW.glob("geo_*_1M.json")):
        if any(token in path.stem for token in skip):
            continue
        data = load_json(path)
        entries.append(
            {
                "name": data["benchmark"],
                "scale": "1M",
                "rows": data["rows"],
                "repetitions": data["repetitions"],
                "median_seconds": data["median_seconds"],
                "geometries_per_second": round(data["geometries_per_second"], 1),
                "input_wkb_bytes": data["input_wkb_bytes"],
                "peak_rss_kib": data.get("peak_rss_kib"),
                "peak_rss_mib": mib(data.get("peak_rss_kib")),
            }
        )
    return entries


def geo_transport_entries() -> list[dict]:
    entries = []
    for path in sorted(RAW.glob("geo_transport_*_1M.json")):
        data = load_json(path)
        timing = parse_time(path.with_suffix(".time"))
        internal = sum(
            data[key]
            for key in (
                "envelope_read_seconds",
                "ipc_decode_seconds",
                "transform_seconds",
                "ipc_encode_seconds",
                "envelope_write_seconds",
            )
        )
        entries.append(
            {
                "name": f"transport_{data['operation']}",
                "scale": "1M",
                "rows": data["rows"],
                "wall_clock_seconds": timing.get("wall_clock_seconds"),
                "internal_seconds": round(internal, 6),
                "phases": {
                    key: data[key]
                    for key in (
                        "envelope_read_seconds",
                        "ipc_decode_seconds",
                        "transform_seconds",
                        "ipc_encode_seconds",
                        "envelope_write_seconds",
                    )
                },
                "geometries_per_second_wall": round(
                    data["rows"] / timing["wall_clock_seconds"], 1
                )
                if timing.get("wall_clock_seconds")
                else None,
                "output_bytes": data["output_bytes"],
                "peak_rss_kib": timing.get("peak_rss_kib"),
                "peak_rss_mib": mib(timing.get("peak_rss_kib")),
            }
        )
    # buffer e simplify sono stati eseguiti via CLI transform-arrow (parametri
    # obbligatori non esponibili da profile_arrow): l'output e' il JSON di
    # stato del comando, senza breakdown di fase.
    for path in sorted(RAW.glob("geo_transport_*_1M.log")):
        operation = path.stem.removeprefix("geo_transport_").removesuffix("_1M")
        data = load_json(path)
        timing = parse_time(path.with_suffix(".time"))
        entries.append(
            {
                "name": f"transport_{operation}",
                "scale": "1M",
                "rows": data["rows"],
                "wall_clock_seconds": timing.get("wall_clock_seconds"),
                "geometries_per_second_wall": round(
                    data["rows"] / timing["wall_clock_seconds"], 1
                )
                if timing.get("wall_clock_seconds")
                else None,
                "output_rows": data.get("output_rows"),
                "output_sha256": data.get("sha256"),
                "peak_rss_kib": timing.get("peak_rss_kib"),
                "peak_rss_mib": mib(timing.get("peak_rss_kib")),
            }
        )
    return entries


def geo_chain_transport_entry() -> dict:
    steps = []
    total = 0.0
    max_rss = 0
    for step in ("step1_buffer", "step2_simplify", "step3_centroid"):
        timing = parse_time(RAW / f"geo_chain_{step}.time")
        steps.append({"step": step, **timing})
        total += timing.get("wall_clock_seconds", 0.0)
        max_rss = max(max_rss, timing.get("peak_rss_kib", 0))
    return {
        "name": "transport_chain_buffer_simplify_centroid",
        "scale": "1M",
        "rows": 1_000_000,
        "note": "tre processi transform-arrow in sequenza; wall = somma dei tre processi",
        "wall_clock_seconds": round(total, 3),
        "steps": steps,
        "peak_rss_kib": max_rss,
        "peak_rss_mib": mib(max_rss),
    }


def spatial_join_entries() -> list[dict]:
    entries = []
    for path in sorted(RAW.glob("geo_spatial_join_*.json")):
        data = load_json(path)
        entries.append(
            {
                "name": "spatial_join_indexed_vs_brute",
                "rows_per_side": data["rows_per_side"],
                "candidate_pairs": data["candidate_pairs"],
                "matches": data["matches"],
                "indexed_seconds": data["indexed_seconds"],
                "brute_force_seconds": data["brute_force_seconds"],
                "speedup": data["speedup"],
            }
        )
    return entries


def extended_topology_entries() -> list[dict]:
    entries = []
    for name in ("line_merge_1M", "split_line_100k", "polygonize_250", "split_polygon_2k"):
        path = RAW / f"geo_{name}.json"
        data = load_json(path)
        timing = parse_time(path.with_suffix(".time"))
        entries.append(
            {
                **data,
                "build": "full-backends (GEOS 3.14 + PROJ statici)"
                if data["operation"] in ("polygonize", "split_polygon")
                else "arrow-transport",
                "wall_clock_seconds": timing.get("wall_clock_seconds"),
                "peak_rss_kib": timing.get("peak_rss_kib"),
                "peak_rss_mib": mib(timing.get("peak_rss_kib")),
            }
        )
    return entries


def main() -> int:
    baseline = {
        "baseline_name": "plenora-data-tools Fase 1f — baseline progetti di origine",
        "created_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "machine": {
            "cpu": "13th Gen Intel(R) Core(TM) i9-13900K",
            "logical_cpus_container": 32,
            "container_ram_kib": 32_716_476,
            "host_ram_gb": 63.7,
            "kernel": "Linux 6.18.33.2-microsoft-standard-WSL2 x86_64",
            "docker": "Docker 29.6.1 (Docker Desktop / WSL2, bind mount Windows)",
            "image": "rust:1.92 (bookworm)",
        },
        "versions": {
            "rustc": "1.92.0 (ded5c06cf 2025-12-08)",
            "cargo": "1.92.0",
            "plenora-nogeo-tools": "0.1.0",
            "plenora-geo": "0.1.0",
            "arrow": "59.1.0",
            "geo": "0.33.1",
            "geozero": "0.15.1",
            "geos_full_backends": "3.14 (statico, solo benchmark extended topology)",
        },
        "builds": {
            "nogeo": "cargo build --release --examples (nessuna feature)",
            "geo_primary": "cargo build --release --examples --features arrow-transport (CARGO_TARGET_DIR=target-docker)",
            "geo_full_backends": "cargo build --release --examples --features full-backends (CARGO_TARGET_DIR=target-docker-full)",
        },
        "datasets": {
            "tabular": "fixture in-process di examples/candidate_benchmark.rs: "
            "id int64, num float64 (0..9999), group utf8 (1024 gruppi), text utf8",
            "geo_kernel": "box 5 vertici da LCG seed 42 (harness/geo_kernels), "
            "coordinate in [0,10000), lati in [0.1,50), WKB little-endian 2D",
            "geo_transport": "envelope PLNGEO3 da harness/make_geo_fixture.py "
            "(stessa fixture di spike_arrow.py: box casuali seed 42, attributi "
            "id/label/weight, 5% null, metadati geoarrow.wkb, EPSG:3857)",
            "geo_transport_fixture_sha256_note": "fixture 1M = 123.363.400 byte, rigenerabile con --rows 1000000 --seed 42",
        },
        "results": {
            "tabular_kernel_in_process": tabular_entries(),
            "geo_kernel_in_process": geo_kernel_entries(),
            "geo_transport_v3": geo_transport_entries(),
            "geo_transport_chain": geo_chain_transport_entry(),
            "geo_spatial_join_in_process": spatial_join_entries(),
            "geo_extended_topology": extended_topology_entries(),
        },
    }
    out = BASELINE_DIR / "baseline.json"
    out.write_text(json.dumps(baseline, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"scritto {out} ({out.stat().st_size} byte)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
