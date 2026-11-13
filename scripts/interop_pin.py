"""Pin del produttore storico, letto dalla provenienza della fixture."""

import re
from fonti_prove import ROOT
import strict_json


def revisione():
    path = ROOT / "crates/plenora-io/tests/dati/io700_multigeometria.provenienza.json"
    value = strict_json.loads(path.read_text("utf-8"))["checkout_revision"]
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise ValueError("revisione IO non valida")
    return value


if __name__ == "__main__":
    print("revision=" + revisione())
