"""Confine unico prima dell'esportazione C degli oggetti PyArrow originali."""

from typing import Any

import pyarrow as pa


def prepara(value: Any) -> Any:
    """Valida prima di trasformare; i reader conservano la lettura lazy.

    Le capsule di terzi garantiscono le capacità dei buffer per contratto
    Arrow: solo i controlli dopo l'import possono essere applicati a loro.
    Gli errori del produttore sono sanificati dal confine Rust chiamante.
    """
    if isinstance(value, pa.Table):
        value.validate(full=True)
        batches = value.to_batches()
    elif isinstance(value, pa.RecordBatch):
        value.validate(full=True)
        batches = [value]
    elif isinstance(value, pa.RecordBatchReader):
        batches = value
    elif isinstance(value, (pa.Array, pa.ChunkedArray)):
        value.validate(full=True)
        return value
    else:
        return value

    def validated() -> Any:
        for batch in batches:
            batch.validate(full=True)
            yield batch

    return pa.RecordBatchReader.from_batches(value.schema, validated())


def lettore(value: Any) -> Any:
    """Adapter lazy C Stream; la costruzione Rust usa l'import fallibile."""
    if hasattr(value, "__arrow_c_stream__"):
        return pa.RecordBatchReader.from_stream(value)
    batch = pa.RecordBatch._import_from_c_capsule(*value.__arrow_c_array__())
    batch.validate(full=True)
    return pa.RecordBatchReader.from_batches(batch.schema, [batch])
