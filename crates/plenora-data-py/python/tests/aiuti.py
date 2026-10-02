"""Tabelle e piani delle prove."""

from __future__ import annotations

import struct
from typing import Any

import pyarrow as pa

# Un valore di cella e un percorso che non devono mai comparire in un
# messaggio d'errore, in `repr` o nei documenti.
CANARINO = "CANARINO-7f3a"

# Il blocco canonico di una geometria GeoArrow-WKB risolta (il vettore
# `resolved-point` di Arrow Metadata Vocabulary 1.0).
GEOMETRIA = {
    "plenora.field_id": "2",
    "ARROW:extension:name": "geoarrow.wkb",
    "plenora.geometry.encoding": "ewkb",
    "plenora.geometry.dimensions": "xy",
    "plenora.geometry.spatial_semantics": "geometry",
    "plenora.geometry.precision": "float64",
    "plenora.geometry.srid": "4326",
    "plenora.geometry.types_declaration": "exact",
    "plenora.geometry.types": "point",
    "plenora.geometry.crs_resolution": "resolved",
    "plenora.geometry.crs_id": "EPSG:4326",
    "plenora.geometry.axis_order": "lat_lon",
}


def punto(nord: float, est: float) -> bytes:
    """Un punto EWKB little-endian con SRID 4326 (ordine lat_lon)."""
    return struct.pack("<BIIdd", 1, 0x20000001, 4326, nord, est)


def tabella_geo() -> pa.Table:
    schema = pa.schema(
        [
            pa.field("id", pa.int64(), False, metadata={"plenora.field_id": "1"}),
            pa.field("geometry", pa.binary(), True, metadata=GEOMETRIA),
        ],
        metadata={"plenora.contract.version": "1", "altro.chiave": "conservata"},
    )
    return pa.table(
        [pa.array([3, 1, 2], pa.int64()), pa.array([punto(41.9, 12.5), None, punto(45.4, 9.2)])],
        schema=schema,
    )


def tabella_semplice() -> pa.Table:
    return pa.table(
        {
            "id": pa.array([3, 1, 2, 5, 4], pa.int64()),
            "nome": pa.array(["c", None, "b", "e", "d"], pa.string()),
            "importo": pa.array([3.5, 1.0, 2.25, 5.0, 4.75], pa.float64()),
        }
    )


def piano_filtro_e_ordine() -> dict[str, Any]:
    """Due output: le righe con `id > 1` e le stesse in ordine di `id`."""
    return {
        "version": 1,
        "inputs": ["t"],
        "steps": [
            {
                "out": "alti",
                "op": "table.filter",
                "in": ["t"],
                "config": {"column": "id", "operator": ">", "value": 1},
            },
            {"out": "ordinati", "op": "table.sort", "in": ["alti"], "config": {"columns": ["id"]}},
        ],
        "outputs": ["alti", "ordinati"],
    }


def piano_identita(nome: str = "t") -> dict[str, Any]:
    """Nessun passo: l'input esce com'è, con lo schema pubblicato."""
    return {"version": 1, "inputs": [nome], "steps": [], "outputs": [nome]}


def piano_lungo(passi: int) -> dict[str, Any]:
    """Una catena di ordinamenti: abbastanza passi da durare secondi, ognuno
    breve, così un annullamento si vede al controllo fra due passi."""
    lista = []
    precedente = "t"
    for indice in range(passi):
        lista.append(
            {
                "out": f"s{indice}",
                "op": "table.sort",
                "in": [precedente],
                "config": {"columns": ["id"], "ascending": indice % 2 == 0},
            }
        )
        precedente = f"s{indice}"
    return {"version": 1, "inputs": ["t"], "steps": lista, "outputs": [precedente]}


def tabella_grande(righe: int = 1_000_000) -> pa.Table:
    return pa.table({"id": pa.array([(i * 7919) % righe for i in range(righe)], pa.int64())})
