#!/usr/bin/env python3
"""Genera il modello di costo delle operazioni geo del runner.

Scrive `crates/plenora-pipeline/src/costi_geo.rs` dalle misure
`data/misure/catalogo-memoria-v4.json` (campagna Windows v4, geo al livello
del runner: `RecordBatch` con colonne GeoArrow-WKB, decodifica con
validazione OGC, kernel, codifica e `RecordBatch` d'uscita; profili default
e avversari su due assi, feature per vertici), con la stessa forma e le
stesse regole delle tabellari (`modello_costi.py`).

Uso:

    python scripts/genera_costi_geo.py            # rigenera
    python scripts/genera_costi_geo.py --verifica # rigenera in memoria e confronta

Rigenerare dalle stesse misure da' un modulo identico byte per byte.

Differenze dalle tabellari:

- le classi di larghezza sono profilo per numero di vertici per geometria
  (5, 100, 1000 nelle fixture), non solo il profilo;
- `geo.generate_grid` non ha ingresso: solo `a` e un termine per cella
  d'uscita (R = celle, note a secco);
- alcuni profili avversari sono ESCLUSI dall'adattamento (`ESCLUSI`, con il
  motivo): la loro memoria cresce con una grandezza che il runner non
  conosce prima del passo (coppie candidate, sovrapposizioni, pareggi,
  forma della geometria), e coprirli renderebbe il modello di ordini di
  grandezza piu' alto sui profili ordinari. Sono un limite dichiarato
  (README, «Limiti dichiarati del runner»); l'oracolo li salta per nome e ne
  riporta il rapporto.
"""
import argparse
import sys

import modello_costi as mc

USCITA = mc.RADICE / "crates" / "plenora-pipeline" / "src" / "costi_geo.rs"

FATTORE_SICUREZZA = (3, 2)

# Profili avversari esclusi dall'adattamento, con il motivo.
ESCLUSI = {
    ("geo.buffer", "adversarial_zigzag"): "linee a zig-zag con distanza molto piu' grande del passo: memoria superlineare nei vertici per geometria",
    ("geo.count_points_in_polygons", "adversarial_all_candidates"): "ogni punto candidato di ogni poligono: memoria per coppia",
    ("geo.coverage_validate", "adversarial_overlaps"): "sovrapposizioni fra tutte le celle vicine",
    ("geo.nearest", "adversarial_ties"): "pareggi: uscita che cresce con i vicini equidistanti",
    ("geo.overlay", "adversarial_overlaps"): "dischi sovrapposti: pezzi d'uscita per coppia",
    ("geo.sjoin", "adversarial_all_candidates"): "ogni coppia candidata: uscita per coppia",
    ("geo.within", "adversarial_all_candidates"): "ogni coppia candidata: memoria per coppia",
}

# Output e transitorio che dipendono dal contenuto (sovrapposizioni,
# selettivita', espansione) oltre che dai byte: il modello copre i profili
# misurati non esclusi, l'output lo limita il controllo esatto dopo il passo.
DIPENDENTI_DAI_DATI = {
    "geo.buffer",
    "geo.clean_topology",
    "geo.clip",
    "geo.count_points_in_polygons",
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
    "geo.within",
}

SENZA_INGRESSO = {"geo.generate_grid"}


def fallisci(messaggio):
    sys.exit(f"genera_costi_geo: {messaggio}")


def modelli(dati):
    per_operazione = {}
    for profilo in dati["profiles"]:
        if profilo["family"] != "geo":
            continue
        if profilo["status"] != "measured" or not mc.punti_osservati(profilo):
            fallisci(f"{profilo['operation_id']} {profilo['profile_id']}: profilo senza punti osservati")
        per_operazione.setdefault(profilo["operation_id"], []).append(profilo)
    for chiave in ESCLUSI:
        profili = per_operazione.get(chiave[0], [])
        if not any(p["profile_id"] == chiave[1] for p in profili):
            fallisci(f"escluso non misurato: {chiave}")
        if chiave[0] not in DIPENDENTI_DAI_DATI:
            fallisci(f"escluso senza dipendenza dai dati dichiarata: {chiave}")
    ignote = (DIPENDENTI_DAI_DATI | SENZA_INGRESSO) - set(per_operazione)
    if ignote:
        fallisci(f"operazioni non misurate: {sorted(ignote)}")
    uscita = {}
    for operazione, profili in sorted(per_operazione.items()):
        punti = []
        for profilo in profili:
            if (operazione, profilo["profile_id"]) in ESCLUSI:
                continue
            for punto in mc.punti_osservati(profilo):
                unita = mc.unita_del_punto(operazione, punto)
                if unita["R"] <= 0 or (unita["B"] <= 0 and operazione not in SENZA_INGRESSO):
                    fallisci(f"{operazione}: unita' non positive")
                unita["y"] = mc.y(punto)
                unita["classe"] = mc.classe_di_larghezza(profilo, punto)
                punti.append(unita)
        modello = mc.adatta(punti, senza_byte=operazione in SENZA_INGRESSO)
        mc.verifica_copertura(operazione, modello, punti)
        uscita[operazione] = {
            "modello": modello,
            "dipende": operazione in DIPENDENTI_DAI_DATI,
            "profili": sorted(p["profile_id"] for p in profili),
            "esclusi": sorted(p for (op, p) in ESCLUSI if op == operazione),
        }
    return uscita


def rust(dati, impronta):
    righe = [
        "//! Modello di costo delle operazioni geo: GENERATO da",
        "//! `scripts/genera_costi_geo.py`, non si modifica a mano.",
        "//!",
        "//! Fonte: `data/misure/catalogo-memoria-v4.json` (campagna Windows v4 a",
        f"//! `{dati['data_tools_commit'][:7]}`, geo al livello del runner: colonne GeoArrow-WKB,",
        "//! decodifica con validazione OGC, kernel, codifica; SHA-256 in",
        "//! [`SHA256_MISURE`]). Stessa forma e stesse regole delle tabellari",
        "//! ([`crate::costi_operazioni`], `scripts/modello_costi.py`); le classi di",
        "//! larghezza sono profilo per vertici per geometria, e `generate_grid` ha",
        "//! solo il termine per cella d'uscita. I profili in `esclusi` non entrano",
        "//! nel modello (motivi in `scripts/genera_costi_geo.py`): limite",
        "//! dichiarato nel README.",
        "",
        "use crate::budget::{Costo, CostoOperazione};",
        "",
        "/// SHA-256 delle misure da cui il modello e' generato.",
        f'pub const SHA256_MISURE: &str = "{impronta}";',
        "",
        "/// Un modello per operazione geo del catalogo, in ordine di id.",
        "pub static COSTI_GEO: &[CostoOperazione] = &[",
    ]
    for operazione, voce in modelli(dati).items():
        righe += [
            "    CostoOperazione {",
            f'        op: "{operazione}",',
            f"        in_memoria: {mc.costo_rust(voce['modello'], 8)},",
            "        spill: None,",
            f"        dipende_dai_dati: {'true' if voce['dipende'] else 'false'},",
            mc.lista_rust("profili", voce["profili"], 8),
            mc.lista_rust("esclusi", voce["esclusi"], 8),
            "    },",
        ]
    righe.append("];")
    return "\n".join(righe) + "\n"


def main():
    argomenti = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    argomenti.add_argument("--verifica", action="store_true")
    scelti = argomenti.parse_args()
    mc.verifica_controesempio_rami()
    dati, impronta = mc.carica()
    generato = rust(dati, impronta)
    if scelti.verifica:
        if USCITA.read_text(encoding="utf-8") != generato:
            fallisci(f"{USCITA.relative_to(mc.RADICE)} non corrisponde alle misure: rigenerare")
        print("costi_geo.rs allineato alle misure")
        return
    USCITA.write_text(generato, encoding="utf-8", newline="\n")
    print(f"scritto {USCITA.relative_to(mc.RADICE)}")


if __name__ == "__main__":
    main()
