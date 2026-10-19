"""Lettura JSON di controllo: chiavi uniche in ogni oggetto, senza dati negli errori."""

import json


def _unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise json.JSONDecodeError("chiave JSON duplicata", "", 0)
        result[key] = value
    return result


def loads(text):
    """Legge un documento; object_pairs_hook controlla anche gli oggetti annidati."""
    return json.loads(text, object_pairs_hook=_unique_pairs)


def load(handle):
    """Legge un documento da un file aperto."""
    return loads(handle.read())
