# Assembla docs/kernel-signatures.md dai frammenti (_fragments/*.md) e dai
# metadati del catalogo (crates/plenora-core/src/catalog.rs).
# Uso: python docs/_build/assemble.py   (dalla radice del repo)
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "crates/plenora-core/src/catalog.rs"
FRAG_DIR = ROOT / "docs/_fragments"
OUT = ROOT / "docs/kernel-signatures.md"

OP_RE = re.compile(
    r'op!\("([^"]+)",\s*(\w+),\s*(\w+),\s*(\w+),\s*(\w+),\s*(\w+),'
    r"\s*(None|Some\(ResultShape::(\w+)\)),\s*(None|Some\(CrsRequirement::(\w+)\)),"
    r"\s*&\[([^\]]*)\],\s*(\w+),\s*(\w+)(?:,\s*kernel_version\s*=\s*(\d+))?\)"
)

ARITY = {"Unary": "unaria", "BinaryOrdered": "binaria (left, right)", "NAry": "n-aria"}
EXEC = {"Streaming": "streaming", "Blocking": "blocking", "BinaryBlocking": "binary-blocking"}
ORIGIN = {"ManipolaCompat": "manipola-compat", "Extension": "estensione"}
CRS = {"Known": "known", "Projected": "projected", "Geographic": "geographic",
       "SameProjected": "same-projected", "Reprojection": "reprojection"}


def parse_catalog():
    ops = {}
    text = CATALOG.read_text(encoding="utf-8")
    for m in OP_RE.finditer(text):
        (oid, _family, origin, arity, exec_, _cancel, _shape_raw, shape,
         _crs_raw, crs, caps, _det, maturity, kver) = m.groups()
        caps = [c.strip().strip('"') for c in caps.split(",") if c.strip()]
        ops[oid] = {
            "origin": ORIGIN[origin], "arity": ARITY[arity], "exec": EXEC[exec_],
            "shape": shape, "crs": CRS.get(crs or ""), "caps": caps,
            "maturity": maturity, "kver": kver,
        }
    return ops


def parse_fragments():
    blocks = {}
    for path in sorted(FRAG_DIR.glob("*.md")):
        cur_id, cur_lines = None, []
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.startswith("### "):
                if cur_id:
                    blocks[cur_id] = "\n".join(cur_lines).strip()
                cur_id, cur_lines = line[4:].strip(), []
            else:
                cur_lines.append(line)
        if cur_id:
            blocks[cur_id] = "\n".join(cur_lines).strip()
    return blocks


# Sezioni: (titolo, [op in ordine])
TABLE_SECTIONS = [
    ("filtering", ["table.filter", "table.conditional"]),
    ("aggregation", ["table.sort", "table.distinct", "table.dedup_advanced",
                     "table.aggregate", "table.rolling_window", "table.window_function"]),
    ("joins", ["table.join", "table.semi_join", "table.anti_join",
               "table.asof_join", "table.cross_join", "table.concat"]),
    ("cleansing", ["table.fill_na", "table.replace", "table.type_cast"]),
    ("strings", ["table.string_pad", "table.string_length",
                 "table.string_extract", "table.text_normalize"]),
    ("dates", ["table.date_format", "table.date_add",
               "table.date_diff", "table.timezone_convert"]),
    ("columns", ["table.drop_columns", "table.rename", "table.reorder_columns",
                 "table.concat_columns", "table.split_column"]),
    ("analysis", ["table.lookup", "table.bin", "table.flatten_json",
                  "table.statistics", "table.sample"]),
    ("reshape", ["table.melt", "table.pivot", "table.transpose",
                 "table.explode", "table.unnest", "table.table_diff"]),
    ("setops", ["table.except", "table.intersect", "table.union_distinct"]),
    ("security", ["table.md5_hash", "table.sha256_hash", "table.mask_data"]),
    ("quality", ["table.assert_schema", "table.assert_not_null", "table.assert_unique",
                 "table.assert_range", "table.assert_regex", "table.coalesce"]),
    ("governance", ["table.assert_cardinality", "table.assert_metadata",
                    "table.assert_foreign_key", "table.reconcile"]),
    ("utility", ["table.add_row_number", "table.date_extract", "table.uuid_generator"]),
    ("formula", ["table.formula"]),
    ("expressions", ["table.expression"]),
]

GEO_SECTIONS = [
    ("Geo Manipola-compat",
     ["geo.centroid", "geo.convex_hull", "geo.envelope", "geo.sjoin", "geo.area",
      "geo.boundary", "geo.bounds_extractor", "geo.buffer", "geo.clean_topology",
      "geo.clip", "geo.count_points_in_polygons", "geo.difference", "geo.dissolve",
      "geo.distance", "geo.explode", "geo.from_coords", "geo.intersection",
      "geo.length", "geo.line_builder", "geo.nearest", "geo.overlay", "geo.perimeter",
      "geo.point_on_surface", "geo.polygon_builder", "geo.simplify",
      "geo.symmetric_difference", "geo.to_wkt", "geo.union", "geo.vertex_count",
      "geo.voronoi", "geo.within", "geo.make_valid", "geo.reproject"]),
    ("Predicati DE-9IM (estensioni geo)",
     ["geo.predicate_intersects", "geo.predicate_disjoint", "geo.predicate_contains",
      "geo.predicate_within", "geo.predicate_equals_topo", "geo.predicate_covers",
      "geo.predicate_covered_by", "geo.predicate_contains_properly",
      "geo.predicate_touches", "geo.predicate_crosses", "geo.predicate_overlaps"]),
    ("Estensioni geo",
     ["geo.affine_transform", "geo.translate", "geo.scale", "geo.rotate",
      "geo.concave_hull", "geo.hausdorff_distance", "geo.haversine_distance",
      "geo.geodesic_distance", "geo.geodesic_line_length", "geo.densify",
      "geo.snap_to_grid", "geo.delaunay", "geo.polygonize", "geo.line_merge",
      "geo.split", "geo.line_substring", "geo.line_interpolate_point",
      "geo.frechet_distance", "geo.bearing", "geo.geodesic_area",
      "geo.geometry_diagnostics"]),
]


def metadata_line(oid, meta):
    parts = [meta["origin"], f"arietà: {meta['arity']}", f"execution class: {meta['exec']}"]
    if meta["crs"]:
        parts.append(f"CRS: {meta['crs']}")
    if meta["caps"]:
        parts.append("capability: " + ", ".join(meta["caps"]))
    if meta["shape"]:
        parts.append(f"shape: {meta['shape']}")
    if meta["maturity"] == "BackendPending":
        parts.append("maturità: backend-pending")
    if meta["kver"] and meta["kver"] != "1":
        parts.append(f"kernel v{meta['kver']}")
    return "*" + " · ".join(parts) + "*"


INTRO = """# plenora-data-tools — Firme dei kernel (v1)

Documento generato dal codice sorgente del workspace (`crates/plenora-core`,
`crates/plenora-kernels-table`, `crates/plenora-kernels-geo`). Copre le **127
operazioni** del catalogo unificato: 62 tabellari (`table.*`) e 65 geografiche
(`geo.*`).

## Come si legge una firma

Un piano v4 è un DAG dichiarativo (`PlanV4`): gli **input di dati** di un nodo
arrivano dagli archi (`in`: riferimenti agli input dichiarati del piano o ad
altri nodi; per le operazioni binarie ordinate l'ordine è semantico —
`[left, right]`), mentre la **configurazione** è nominata nel nodo (`config`,
oggetto JSON; `null` od omessa equivale a `{}`). La config viene deserializzata
in modo tipizzato contro la struct serde dell'operazione, fail-closed: tutte le
struct hanno `deny_unknown_fields`, quindi un campo sconosciuto è un errore. I
vincoli ulteriori (campi non vuoti, range, colonne esistenti, tipi ammessi)
sono verificati in `validate` del kernel e/o nell'analisi di contratto
(`analyze.rs`), che inferisce anche lo schema dell'arco in uscita.

Esempio di nodo piano v4:

```json
{
  "schema_version": 4,
  "crs": "EPSG:32632",
  "inputs": ["sorgente_a"],
  "nodes": [
    {
      "id": "n1",
      "op": "table.filter",
      "in": ["sorgente_a"],
      "config": {"column": "importo", "operator": ">", "value": 0}
    }
  ],
  "output": "n1"
}
```

Per ogni kernel il blocco riporta: id canonico, metadati di catalogo
(`crates/plenora-core/src/catalog.rs`: provenienza, arietà, execution class;
per le geo anche requisito CRS, capability richieste, result shape), la tabella
dei campi config (campo | tipo JSON | default | vincoli), i requisiti sull'
input e l'output prodotto. Convenzioni: `obbligatorio` = campo senza default
serde; `Option<T>` senza default esplicito è riportato con default `null`.
Il modulo `spill.rs` del crate tabellare non corrisponde ad alcuna operazione
del catalogo (spilling interno degli operatori blocking) e non ha una firma.

## Indice

"""


def main():
    catalog = parse_catalog()
    blocks = parse_fragments()
    missing_cat = [oid for _, ops in TABLE_SECTIONS + GEO_SECTIONS for oid in ops
                   if oid not in catalog]
    missing_blk = [oid for _, ops in TABLE_SECTIONS + GEO_SECTIONS for oid in ops
                   if oid not in blocks]
    extra_blk = sorted(set(blocks) - {oid for _, ops in TABLE_SECTIONS + GEO_SECTIONS
                                      for oid in ops})
    if missing_cat or missing_blk or extra_blk:
        print("MISMATCH", missing_cat, missing_blk, extra_blk)
        sys.exit(1)

    out = [INTRO]
    out.append("### Operazioni tabellari (62)\n")
    for name, ops in TABLE_SECTIONS:
        out.append(f"- **{name}** ({len(ops)}): " + ", ".join(f"`{o}`" for o in ops))
    out.append("\n### Operazioni geografiche (65)\n")
    for name, ops in GEO_SECTIONS:
        out.append(f"- **{name}** ({len(ops)}): " + ", ".join(f"`{o}`" for o in ops))

    def emit(oid):
        out.append(f"\n### {oid}\n")
        out.append(metadata_line(oid, catalog[oid]) + "\n")
        out.append(blocks[oid])

    for name, ops in TABLE_SECTIONS:
        out.append(f"\n## Tabellari — {name}\n")
        for oid in ops:
            emit(oid)
    for name, ops in GEO_SECTIONS:
        out.append(f"\n## Geo — {name}\n")
        for oid in ops:
            emit(oid)

    OUT.write_text("\n".join(out) + "\n", encoding="utf-8")
    n_ops = sum(len(ops) for _, ops in TABLE_SECTIONS + GEO_SECTIONS)
    print(f"OK: {OUT} ({n_ops} op, {len(OUT.read_text(encoding='utf-8').splitlines())} righe)")


if __name__ == "__main__":
    main()
