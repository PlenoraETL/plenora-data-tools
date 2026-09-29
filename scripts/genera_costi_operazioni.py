#!/usr/bin/env python3
"""Genera il modello di costo delle operazioni tabellari del runner.

Scrive `crates/plenora-pipeline/src/costi_operazioni.rs` dal catalogo empirico
della memoria `data/misure/catalogo-memoria-tabellare-v3.json` (campagna
Windows, `PeakWorkingSet64`, profili wide/narrow/distinct/spilled; semantica
delle metriche nel campo `semantics` del catalogo). Il file generato riporta
lo SHA-256 del catalogo: un test del runner lo confronta con il file nel
repository.

Uso:

    python scripts/genera_costi_operazioni.py            # rigenera
    python scripts/genera_costi_operazioni.py --verifica # rigenera in memoria e confronta

Rigenerare dallo stesso catalogo da' un file identico byte per byte: nessuna
data, nessun ordine di dizionario, solo aritmetica intera.

Modello, per operazione e variante (in memoria, spilled):

    picco = S * (a + max(r * righe_in, c * byte_in, p * righe_sx * righe_dx))

- y_i = max(budget_estimate_bytes_i, output_new_buffer_bytes_i, 0) per ogni
  punto osservato: la stima di budget del catalogo (massimo fra picco
  incrementale e crescita di fase) e mai meno dei byte nuovi dell'output. Il
  pavimento a zero assorbe i punti rumorosi o inconcludenti (incremento
  negativo), che non abbassano mai il modello sotto un altro punto;
- a = y al campione piu' piccolo del profilo;
- r, c, p = inviluppo superiore max_i (y_i - a) / unita'_i, con unita' le
  righe in ingresso (somma dei due lati per le binarie), i byte in ingresso
  (buffer Arrow deduplicati per allocazione, la misura di `byte_vivi`) e,
  per le operazioni superlineari, le coppie righe_sx * righe_dx;
- per operazione, il massimo componente per componente su tutti i profili
  misurati della variante (il profilo peggiore, compreso quello avversario
  `distinct` dove c'e');
- operazioni superlineari (`SUPERLINEARI_A_COPPIE`): r = c = 0 e solo il
  termine a coppie; il generatore verifica che ogni loro profilo cresca per
  riga piu' di `SOGLIA_SUPERLINEARE` fra gli ultimi due campioni;
- output che e' un sottoinsieme delle righe (`USCITA_SOTTOINSIEME`): c non
  scende sotto 1, perche' l'output puo' essere una copia intera dell'input
  anche se le fixture ne tengono una parte;
- S = 3/2 (`FATTORE_SICUREZZA`), applicato dal runner con arrotondamento per
  eccesso; r, c, p sono in millesimi di byte, arrotondati per eccesso.

Il modello copre le operazioni misurate, nel dominio misurato (fino a 5
milioni di righe, 1000 per lato per le superlineari); oltre e' una
estrapolazione, dichiarata nel README.
"""
import argparse
import hashlib
import json
import pathlib
import sys

RADICE = pathlib.Path(__file__).resolve().parent.parent
CATALOGO = RADICE / "data" / "misure" / "catalogo-memoria-tabellare-v3.json"
USCITA = RADICE / "crates" / "plenora-pipeline" / "src" / "costi_operazioni.rs"

FATTORE_SICUREZZA = (3, 2)
SOGLIA_SUPERLINEARE = 1.5
MILLE = 1000

# Crescita per riga superlineare in ogni profilo: prodotto cartesiano e
# confronto a coppie.
SUPERLINEARI_A_COPPIE = {"table.cross_join", "table.fuzzy_join"}

# Output e transitorio che dipendono dal contenuto oltre che da righe e byte
# (cardinalita' delle chiavi, lunghezza delle liste, valori distinti): il
# modello copre il transitorio calibrato sul caso peggiore misurato; l'output
# lo limitano i preflight dei kernel con il margine passato e il controllo
# esatto dopo il passo.
DIPENDENTI_DAI_DATI = {
    "table.aggregate",
    "table.anti_join",
    "table.asof_join",
    "table.cross_join",
    "table.dedup_advanced",
    "table.explode",
    "table.flatten_json",
    "table.fuzzy_join",
    "table.join",
    "table.melt",
    "table.pivot",
    "table.rolling_window",
    "table.semi_join",
    "table.transpose",
    "table.unnest",
    "table.window_function",
}

# Output fatto di righe dell'input (o dei due input), fino a una copia
# intera: c >= 1.
USCITA_SOTTOINSIEME = {
    "table.anti_join",
    "table.dedup_advanced",
    "table.distinct",
    "table.except",
    "table.filter",
    "table.intersect",
    "table.semi_join",
    "table.sort",
    "table.top_n",
    "table.union_distinct",
}


def fallisci(messaggio):
    sys.exit(f"genera_costi_operazioni: {messaggio}")


def ceil_div(numeratore, denominatore):
    return -(-numeratore // denominatore)


def punti_osservati(profilo):
    return [p for p in profilo["points"] if p.get("status") == "observed"]


def y(punto):
    return max(punto["budget_estimate_bytes"], punto["output_new_buffer_bytes"], 0)


def per_riga_ultimi_due(punti):
    prima, ultima = punti[-2], punti[-1]
    righe_prima = prima["rows"] + prima["secondary_rows"]
    righe_ultima = ultima["rows"] + ultima["secondary_rows"]
    if y(prima) <= 0:
        return None
    return (y(ultima) / righe_ultima) / (y(prima) / righe_prima)


def modello_profilo(operazione, profilo):
    punti = punti_osservati(profilo)
    if not punti:
        fallisci(f"{operazione} {profilo['profile_id']}: nessun punto osservato")
    a = y(punti[0])
    r = c = p = 0
    for punto in punti:
        crescita = max(y(punto) - a, 0)
        righe = punto["rows"] + punto["secondary_rows"]
        byte = punto["input_buffer_bytes"]
        if righe <= 0 or byte <= 0:
            fallisci(f"{operazione} {profilo['profile_id']}: unita' non positive")
        if operazione in SUPERLINEARI_A_COPPIE:
            coppie = punto["rows"] * max(punto["secondary_rows"], 1)
            p = max(p, ceil_div(crescita * MILLE, coppie))
        else:
            r = max(r, ceil_div(crescita * MILLE, righe))
            c = max(c, ceil_div(crescita * MILLE, byte))
    return {"a": a, "r": r, "c": c, "p": p}


def combina(operazione, profili):
    modello = {"a": 0, "r": 0, "c": 0, "p": 0}
    for profilo in profili:
        singolo = modello_profilo(operazione, profilo)
        for chiave in modello:
            modello[chiave] = max(modello[chiave], singolo[chiave])
    if operazione in USCITA_SOTTOINSIEME:
        modello["c"] = max(modello["c"], MILLE)
    return modello


def costruisci(testo):
    catalogo = json.loads(testo)
    if catalogo.get("schema_version") != 3:
        fallisci("schema del catalogo diverso da 3")
    per_operazione = {}
    for profilo in catalogo["profiles"]:
        if profilo["status"] != "measured" or not punti_osservati(profilo):
            continue
        variante = "memoria" if profilo["execution"] == "direct" else "spill"
        per_operazione.setdefault(profilo["operation_id"], {}).setdefault(variante, []).append(profilo)

    budget_spill = set()
    sospetti = []
    voci = []
    for operazione in sorted(per_operazione):
        varianti = per_operazione[operazione]
        if "memoria" not in varianti:
            fallisci(f"{operazione}: nessun profilo in memoria misurato")
        for profilo in varianti.get("spill", []):
            for punto in punti_osservati(profilo):
                budget_spill.add(punto["governed_budget_bytes"])
        rapporti = []
        for profilo in varianti["memoria"]:
            punti = punti_osservati(profilo)
            if len(punti) >= 2:
                rapporto = per_riga_ultimi_due(punti)
                rapporti.append((profilo["profile_id"], rapporto))
        superlineare_ovunque = bool(rapporti) and all(
            rapporto is not None and rapporto > SOGLIA_SUPERLINEARE for _, rapporto in rapporti
        )
        if operazione in SUPERLINEARI_A_COPPIE and not superlineare_ovunque:
            fallisci(f"{operazione}: dichiarata superlineare, i profili non lo sono")
        if superlineare_ovunque and operazione not in SUPERLINEARI_A_COPPIE:
            for profilo_id, rapporto in rapporti:
                sospetti.append(f"`{operazione}` {profilo_id}: {rapporto:.2f}")
        voci.append({
            "op": operazione,
            "memoria": combina(operazione, varianti["memoria"]),
            "spill": combina(operazione, varianti["spill"]) if "spill" in varianti else None,
            "profili": sorted(
                f"{p['profile_id']}" for v in ("memoria", "spill") for p in varianti.get(v, [])
            ),
            "dipende_dai_dati": operazione in DIPENDENTI_DAI_DATI,
        })
    if len(budget_spill) != 1:
        fallisci(f"budget governato dei profili spilled non unico: {sorted(budget_spill)}")
    for insieme, nome in (
        (SUPERLINEARI_A_COPPIE, "SUPERLINEARI_A_COPPIE"),
        (DIPENDENTI_DAI_DATI, "DIPENDENTI_DAI_DATI"),
        (USCITA_SOTTOINSIEME, "USCITA_SOTTOINSIEME"),
    ):
        ignote = insieme - set(per_operazione)
        if ignote:
            fallisci(f"{nome}: operazioni non misurate {sorted(ignote)}")
    return voci, budget_spill.pop(), sospetti


def costo_rust(modello, rientro):
    """Un `Costo` nella forma di rustfmt, con i separatori delle migliaia."""
    dentro = " " * (rientro + 4)
    campi = (("a", "a"), ("r_millesimi", "r"), ("c_millesimi", "c"), ("p_millesimi", "p"))
    righe = ["Costo {"]
    righe += [f"{dentro}{campo}: {modello[chiave]:_}," for campo, chiave in campi]
    righe.append(" " * rientro + "}")
    return "\n".join(righe)


def genera(testo_bytes):
    voci, budget_spill, sospetti = costruisci(testo_bytes.decode("utf-8-sig"))
    impronta = hashlib.sha256(testo_bytes).hexdigest()
    righe = [
        "//! Modello di costo delle operazioni tabellari: GENERATO da",
        "//! `scripts/genera_costi_operazioni.py`, non si modifica a mano.",
        "//!",
        "//! Fonte: `data/misure/catalogo-memoria-tabellare-v3.json` (campagna",
        "//! Windows, `PeakWorkingSet64`; SHA-256 in [`SHA256_CATALOGO`]).",
        "//!",
        "//! Per operazione e variante:",
        "//!",
        "//! ```text",
        "//! picco = S * (a + max(r * righe_in, c * byte_in, p * righe_sx * righe_dx))",
        "//! ```",
        "//!",
        "//! - `y_i = max(budget_estimate_bytes_i, output_new_buffer_bytes_i, 0)`",
        "//!   per ogni punto osservato; il pavimento a zero assorbe i punti",
        "//!   rumorosi o inconcludenti;",
        "//! - `a` = `y` al campione piu' piccolo; `r`, `c`, `p` = inviluppo",
        "//!   superiore `max_i (y_i - a) / unita'_i` (righe in ingresso, byte in",
        "//!   ingresso deduplicati per allocazione, coppie di righe), in millesimi",
        "//!   di byte arrotondati per eccesso;",
        "//! - massimo componente per componente su tutti i profili misurati della",
        "//!   variante (il peggiore, compreso l'avversario `distinct`);",
        "//! - superlineari (`cross_join`, `fuzzy_join`): solo il termine a coppie;",
        "//! - output sottoinsieme delle righe: `c` almeno 1 (copia intera);",
        "//! - `S` = [`FATTORE_SICUREZZA`], per eccesso.",
        "//!",
        "//! Le varianti spilled sono misurate con `max_governed_memory_bytes` pari",
        "//! a [`BUDGET_SPILL_MISURATO`]: il runner non passa ai kernel spilled un",
        "//! margine piu' grande.",
    ]
    if sospetti:
        righe.append("//!")
        righe.append("//! Crescita per riga oltre la soglia in ogni profilo, fra gli ultimi")
        righe.append("//! due campioni, per operazioni lineari nel modello (inviluppo sul")
        righe.append("//! campione piu' grande; oltre il dominio misurato, estrapolazione):")
        righe.append("//!")
        for sospetto in sospetti:
            righe.append(f"//! - {sospetto}")
    righe += [
        "",
        "use crate::budget::{Costo, CostoOperazione};",
        "",
        "/// SHA-256 del catalogo da cui il modello e' generato.",
        "pub const SHA256_CATALOGO: &str =",
        f'    "{impronta}";',
        "",
        "/// Fattore di sicurezza `S` come frazione (numeratore, denominatore).",
        f"pub const FATTORE_SICUREZZA: (u64, u64) = ({FATTORE_SICUREZZA[0]}, {FATTORE_SICUREZZA[1]});",
        "",
        "/// `max_governed_memory_bytes` dei profili spilled misurati.",
        f"pub const BUDGET_SPILL_MISURATO: u64 = {budget_spill:_};",
        "",
        "/// Un modello per operazione misurata, in ordine di id.",
        "pub static COSTI: &[CostoOperazione] = &[",
    ]
    for voce in voci:
        spill = f"Some({costo_rust(voce['spill'], 8)})" if voce["spill"] else "None"
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
            f'        op: "{voce["op"]}",',
            f"        in_memoria: {costo_rust(voce['memoria'], 8)},",
            f"        spill: {spill},",
            f"        dipende_dai_dati: {'true' if voce['dipende_dai_dati'] else 'false'},",
            riga_profili,
            "    },",
        ]
    righe.append("];")
    return "\n".join(righe) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--verifica", action="store_true", help="confronta senza scrivere")
    argomenti = parser.parse_args()
    testo = genera(CATALOGO.read_bytes())
    if argomenti.verifica:
        attuale = USCITA.read_text(encoding="utf-8") if USCITA.exists() else None
        if attuale != testo:
            fallisci(f"{USCITA.relative_to(RADICE)} non corrisponde al catalogo: rigenerare")
        print("costi_operazioni.rs allineato al catalogo")
        return
    USCITA.write_bytes(testo.encode("utf-8"))
    print(f"scritto {USCITA.relative_to(RADICE)}")


if __name__ == "__main__":
    main()
