"""Aggiorna solo le esclusioni WKT1 dai documenti fissati, senza PROJ."""
import json
from pathlib import Path
from genera_wkt_integrati import esclusioni_wkt1


def main():
    radice = Path(__file__).resolve().parents[1]
    fonte = radice / "crates/plenora-io/data/projjson_integrati.json"
    percorso = radice / "crates/plenora-core/src/crs/definizioni/wkt_corpus.json"
    documenti = json.loads(fonte.read_text(encoding="utf-8"))
    corpus = json.loads(percorso.read_text(encoding="utf-8"))
    if documenti.keys() != corpus.keys():
        raise ValueError("Corpus incompleto")
    semantica = json.loads((radice / "crates/plenora-core/src/crs/definizioni/semantica.json").read_text(encoding="utf-8"))
    datum = {v["id"]["authority"] + ":" + v["id"]["code"]:
             v["base"]["datum"]["identita"]["id"] for v in semantica}
    for codice, voce in corpus.items():
        voce.update(esclusioni_wkt1(voce["wkt1_gdal"], voce["wkt1_assi"],
                                   datum[codice], voce["esclusione_2019"]))
    percorso.write_text(json.dumps(corpus, ensure_ascii=False, indent=2) + "\n",
                        encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
