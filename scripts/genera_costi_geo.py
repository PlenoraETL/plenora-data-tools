#!/usr/bin/env python3
"""Genera il modello di costo PROVVISORIO delle operazioni geo del runner.

Le operazioni geo non sono ancora state rimisurate sul runner (README,
«Budget di memoria», modelli provvisori). Fino ad allora il loro modello e'
dichiarato e conservativo, derivato dal catalogo empirico della memoria di
`plenora-memory-lab` (`results/memory-catalog/catalog.json`, schema 2,
campagna Windows, `PeakWorkingSet64`), che misura i kernel su geometrie gia'
decodificate, senza gli adapter Arrow.

Due passi, entrambi riproducibili:

    # 1. estrazione (una volta, dal catalogo del laboratorio, sola lettura):
    python scripts/genera_costi_geo.py --estrai <plenora-memory-lab>/results/memory-catalog/catalog.json
    #    scrive data/misure/profili-geo-memory-lab.json (i soli campi usati,
    #    con lo SHA-256 del catalogo d'origine)

    # 2. generazione del modello da quel file:
    python scripts/genera_costi_geo.py            # rigenera costi_geo.rs
    python scripts/genera_costi_geo.py --verifica # rigenera in memoria e confronta

Rigenerare dallo stesso file da' un modulo identico byte per byte.

Modello, per operazione (stessa forma del modello tabellare):

    picco = S * (a + max(r * righe, c * byte_in))

Per ogni punto osservato del catalogo, y = picco incrementale misurato e
u = byte di geometria in ingresso, stimati per difetto dalle dimensioni del
punto: 16 byte per vertice piu' 9 per geometria (il WKB piu' l'offset
Arrow di una cella non sono mai meno: un punto WKB sono 21 byte piu' 4 di
offset), per `from_coords`/`from_wkt` i byte nativi dell'ingresso
(coordinate `Float64`, testo WKT). Poi, per operazione:

- a0 = y al campione piu' piccolo; c0 = max_i (y_i - a0) / u_i: l'inviluppo
  superiore del kernel, sul peggiore dei profili dell'operazione;
- il kernel misurato non decodifica ne' ricodifica: il runner aggiunge le
  geometrie decodificate (byte nativi fino a circa 2,3 volte il WKB nelle
  fixture: 48 byte per un punto da 21) e l'uscita codificata piu' la sua
  copia Arrow. Da qui le costanti, generose:
      c = 2 * c0 + 4        (byte per byte in ingresso)
      a = a0 + 4 MiB        (thread di rayon, strutture fisse)
- `generate_grid` non ha ingresso: r = 2 * (y / celle) + 64 per cella
  prevista (il contratto la conosce a secco);
- operazioni misurate con un backend diverso da quello del runner (GEOS per
  `make_valid`, `polygonize`, `split`; PROJ per `reproject`): il massimo
  dei profili dell'operazione e, per `split` (misurato solo su 100
  geometrie, dove il costo fisso domina il rapporto), il massimo della
  classe delle espansioni Rust (`explode`, `delaunay`, `subdivide`);
- r, c in millesimi di byte arrotondati per eccesso, a in byte arrotondati
  ai 4 KiB superiori; S = 3/2 come per le tabellari, dal runner.
"""
import argparse
import hashlib
import json
import pathlib
import sys

RADICE = pathlib.Path(__file__).resolve().parent.parent
PROFILI = RADICE / "data" / "misure" / "profili-geo-memory-lab.json"
USCITA = RADICE / "crates" / "plenora-pipeline" / "src" / "costi_geo.rs"

MILLE = 1000
MIB = 1024 * 1024
RISERVA_FISSA = 4 * MIB
FATTORE_ADAPTER = 2
BYTE_DECODIFICA = 4
BYTE_PER_CELLA_GRIGLIA = 64

# Output e transitorio che dipendono dal contenuto (sovrapposizioni,
# selettivita', espansione) oltre che dai byte: il modello copre il caso
# misurato, l'output lo limita il controllo esatto dopo il passo.
DIPENDENTI_DAI_DATI = {
    "geo.buffer",
    "geo.clean_topology",
    "geo.clip",
    "geo.coverage_validate",
    "geo.delaunay",
    "geo.densify",
    "geo.difference",
    "geo.dissolve",
    "geo.explode",
    "geo.intersection",
    "geo.line_merge",
    "geo.make_valid",
    "geo.nearest",
    "geo.overlay",
    "geo.polygonize",
    "geo.reproject",
    "geo.shared_paths",
    "geo.sjoin",
    "geo.split",
    "geo.subdivide",
    "geo.symmetric_difference",
    "geo.union",
    "geo.voronoi",
}

# Classe con cui si modella `split` (vedi l'intestazione).
CLASSE_ESPANSIONI = ("geo.delaunay", "geo.explode", "geo.subdivide")

PRODUTTORI_DA_TESTO = {"geo.from_coords", "geo.from_wkt"}


def fallisci(messaggio):
    sys.exit(f"genera_costi_geo: {messaggio}")


def ceil_div(numeratore, denominatore):
    return -(-numeratore // denominatore)


def sha256(percorso):
    return hashlib.sha256(percorso.read_bytes()).hexdigest()


def estrai(catalogo):
    """I campi usati dei profili geo del catalogo, in ordine stabile."""
    testo = catalogo.read_bytes()
    dati = json.loads(testo.decode("utf-8-sig"))
    if dati.get("schema_version") != 2:
        fallisci("atteso il catalogo schema 2")
    profili = []
    for profilo in dati["profiles"]:
        operazione = profilo["operation_id"]
        if not operazione.startswith("geo."):
            continue
        punti = []
        for punto in profilo["model"]["points"]:
            if punto.get("measurement_status") != "observed":
                continue
            ingressi = punto["inputs"]
            if operazione == "geo.generate_grid":
                righe, byte = ingressi["sample_value"], 0
            elif "total_input_vertices" in ingressi:
                righe = ingressi.get("primary_features", 0) + ingressi.get(
                    "secondary_features", 0
                )
                if operazione in PRODUTTORI_DA_TESTO:
                    byte = ingressi["input_native_bytes_estimate"]
                else:
                    byte = 16 * ingressi["total_input_vertices"] + 9 * righe
            elif "coordinate_bytes" in ingressi:
                righe = ingressi["geometries"]
                byte = ingressi["coordinate_bytes"] + 9 * righe
            elif "data_bytes" in ingressi:
                righe = ingressi["rows"]
                byte = ingressi["data_bytes"] + 9 * righe
            else:
                fallisci(f"{operazione}: dimensioni del punto non riconosciute")
            punti.append(
                {
                    "campione": ingressi.get("sample_value", righe),
                    "righe": righe,
                    "byte": byte,
                    "picco": punto["incremental_peak_working_set_bytes"],
                }
            )
        punti.sort(key=lambda p: p["campione"])
        profili.append(
            {
                "op": operazione,
                "profilo": profilo["profile_id"],
                "backend": profilo["backend"],
                "fixture": profilo["fixture"],
                "punti": punti,
            }
        )
    profili.sort(key=lambda p: (p["op"], p["profilo"]))
    return {
        "fonte": "plenora-memory-lab results/memory-catalog/catalog.json",
        "sha256_fonte": hashlib.sha256(testo).hexdigest(),
        "profili": profili,
    }


def modello_profilo(profilo):
    punti = profilo["punti"]
    if not punti:
        fallisci(f"{profilo['op']} {profilo['profilo']}: nessun punto osservato")
    a = max(punti[0]["picco"], 0)
    c = r = 0
    for punto in punti:
        crescita = max(punto["picco"] - a, 0)
        if profilo["op"] == "geo.generate_grid":
            r = max(r, ceil_div(punto["picco"] * MILLE, max(punto["righe"], 1)))
        else:
            if punto["byte"] <= 0:
                fallisci(f"{profilo['op']}: byte in ingresso non positivi")
            c = max(c, ceil_div(crescita * MILLE, punto["byte"]))
    return {"a": a, "c": c, "r": r}


def modelli(dati):
    per_operazione = {}
    for profilo in dati["profili"]:
        voce = per_operazione.setdefault(
            profilo["op"], {"a": 0, "c": 0, "r": 0, "profili": []}
        )
        misurato = modello_profilo(profilo)
        for chiave in ("a", "c", "r"):
            voce[chiave] = max(voce[chiave], misurato[chiave])
        voce["profili"].append(f"{profilo['backend']}:{profilo['profilo']}")
    split = per_operazione.get("geo.split")
    if split is None:
        fallisci("manca geo.split")
    split["c"] = max(per_operazione[op]["c"] for op in CLASSE_ESPANSIONI)
    split["profili"].append("classe:espansioni")
    uscita = {}
    for operazione, voce in sorted(per_operazione.items()):
        a = ceil_div(voce["a"] + RISERVA_FISSA, 4096) * 4096
        c = FATTORE_ADAPTER * voce["c"] + BYTE_DECODIFICA * MILLE if voce["c"] else 0
        r = (
            FATTORE_ADAPTER * voce["r"] + BYTE_PER_CELLA_GRIGLIA * MILLE
            if voce["r"]
            else 0
        )
        if operazione != "geo.generate_grid" and c == 0:
            c = BYTE_DECODIFICA * MILLE
        uscita[operazione] = {
            "a": a,
            "c": c,
            "r": r,
            "dipende": operazione in DIPENDENTI_DAI_DATI,
            "profili": sorted(voce["profili"]),
        }
    return uscita


def rust(dati, impronta):
    righe = [
        "//! Modello di costo PROVVISORIO delle operazioni geo: GENERATO da",
        "//! `scripts/genera_costi_geo.py`, non si modifica a mano.",
        "//!",
        "//! Fonte: `data/misure/profili-geo-memory-lab.json` (SHA-256 in",
        "//! [`SHA256_PROFILI_GEO`]), estratto dal catalogo empirico di",
        "//! `plenora-memory-lab` (SHA-256 del catalogo nel file). I kernel sono",
        "//! misurati su geometrie gia' decodificate, senza gli adapter Arrow del",
        "//! runner: le costanti aggiungono decodifica e uscita codificata",
        "//! (formula nell'intestazione del generatore). Provvisorio finche' le",
        "//! operazioni geo non sono rimisurate sul runner (README, «Budget di",
        "//! memoria»).",
        "",
        "use crate::budget::{Costo, CostoOperazione};",
        "",
        "/// SHA-256 dei profili geo da cui il modello e' generato.",
        "pub const SHA256_PROFILI_GEO: &str =",
        f'    "{impronta}";',
        "",
        "/// Un modello per operazione geo del catalogo, in ordine di id.",
        "pub static COSTI_GEO: &[CostoOperazione] = &[",
    ]
    for operazione, voce in modelli(dati).items():
        profili = ", ".join(f'"{p}"' for p in voce["profili"])
        riga_profili = f"        profili: &[{profili}],"
        if len(f"&[{profili}]") > 60:
            # Oltre `array_width` di rustfmt (60): un profilo per riga.
            riga_profili = "\n".join(
                ["        profili: &["]
                + [f'            "{p}",' for p in voce["profili"]]
                + ["        ],"]
            )
        righe += [
            "    CostoOperazione {",
            f'        op: "{operazione}",',
            "        in_memoria: Costo {",
            f"            a: {voce['a']:_},",
            f"            r_millesimi: {voce['r']:_},",
            f"            c_millesimi: {voce['c']:_},",
            "            p_millesimi: 0,",
            "        },",
            "        spill: None,",
            f"        dipende_dai_dati: {'true' if voce['dipende'] else 'false'},",
            riga_profili,
            "    },",
        ]
    righe.append("];")
    return "\n".join(righe) + "\n"


def main():
    argomenti = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    argomenti.add_argument("--estrai", type=pathlib.Path)
    argomenti.add_argument("--verifica", action="store_true")
    scelti = argomenti.parse_args()
    if scelti.estrai:
        estratti = estrai(scelti.estrai)
        PROFILI.write_text(
            json.dumps(estratti, indent=1, ensure_ascii=False) + "\n",
            encoding="utf-8",
            newline="\n",
        )
    dati = json.loads(PROFILI.read_text(encoding="utf-8"))
    generato = rust(dati, sha256(PROFILI))
    if scelti.verifica:
        if USCITA.read_text(encoding="utf-8") != generato:
            fallisci(f"{USCITA} non corrisponde ai profili: rigenerare")
        return
    USCITA.write_text(generato, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
