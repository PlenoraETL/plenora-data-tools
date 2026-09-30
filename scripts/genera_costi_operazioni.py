#!/usr/bin/env python3
"""Genera il modello di costo delle operazioni tabellari del runner.

Scrive `crates/plenora-pipeline/src/costi_operazioni.rs` dalle misure
`data/misure/catalogo-memoria-v4.json` (campagna Windows v4,
`PeakWorkingSet64`, profili wide/narrow/distinct/spilled; provenienza e
avvertenze nel file), con la forma e le regole di `modello_costi.py`. Il
file generato riporta lo SHA-256 delle misure: un test del runner lo
confronta con il file nel repository, e l'oracolo
`crates/plenora-pipeline/tests/oracolo_costi.rs` verifica che il modello
copra ogni punto misurato.

Uso:

    python scripts/genera_costi_operazioni.py            # rigenera
    python scripts/genera_costi_operazioni.py --verifica # rigenera in memoria e confronta

Rigenerare dalle stesse misure da' un file identico byte per byte: nessuna
data, nessun ordine di dizionario, solo aritmetica esatta.
"""
import argparse
import sys

import modello_costi as mc

USCITA = mc.RADICE / "crates" / "plenora-pipeline" / "src" / "costi_operazioni.rs"

FATTORE_SICUREZZA = (3, 2)
SOGLIA_SUPERLINEARE = 1.5

# Crescita per riga superlineare in ogni profilo: prodotto cartesiano e
# confronto a coppie.
SUPERLINEARI_A_COPPIE = {"table.cross_join", "table.fuzzy_join"}

# Uscita larga quanto la config la fa (una colonna per voce del `mapping`):
# termine per cella d'uscita, K = righe in ingresso * colonne d'uscita.
PER_CELLA = {"table.pivot"}

# Output e transitorio che dipendono dal contenuto oltre che da righe e byte
# (cardinalita' delle chiavi, lunghezza delle liste, valori distinti): il
# modello copre il caso peggiore misurato; l'output lo limitano i preflight
# dei kernel con il margine passato e il controllo esatto dopo il passo.
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

# Uscita fino a una copia intera degli ingressi (righe dell'input, o dei due
# input senza espansione delle chiavi; le chiavi indice di `pivot`), anche
# dove le fixture ne tengono una parte: `c >= 1`.
USCITA_COPIA = {
    "table.anti_join",
    "table.asof_join",
    "table.dedup_advanced",
    "table.distinct",
    "table.except",
    "table.filter",
    "table.intersect",
    "table.join",
    "table.pivot",
    "table.semi_join",
    "table.sort",
    "table.top_n",
    "table.union_distinct",
}


def fallisci(messaggio):
    sys.exit(f"genera_costi_operazioni: {messaggio}")


def punti_del_profilo(operazione, profilo):
    punti = []
    for punto in mc.punti_osservati(profilo):
        unita = mc.unita_del_punto(operazione, punto)
        if unita["R"] <= 0 or unita["B"] <= 0:
            fallisci(f"{operazione} {profilo['profile_id']}: unita' non positive")
        unita["y"] = mc.y(punto)
        unita["classe"] = mc.classe_di_larghezza(profilo, punto)
        punti.append(unita)
    return punti


def per_riga_ultimi_due(punti):
    prima, ultima = punti[-2], punti[-1]
    if prima["y"] <= 0:
        return None
    return (ultima["y"] / ultima["R"]) / (prima["y"] / prima["R"])


def costruisci(dati):
    per_operazione = {}
    for profilo in dati["profiles"]:
        if profilo["family"] != "table":
            continue
        if profilo["status"] != "measured" or not mc.punti_osservati(profilo):
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
            for punto in mc.punti_osservati(profilo):
                budget_spill.add(punto["governed_budget_bytes"])
        rapporti = []
        for profilo in varianti["memoria"]:
            punti = punti_del_profilo(operazione, profilo)
            if len(punti) >= 2:
                rapporti.append((profilo["profile_id"], per_riga_ultimi_due(punti)))
        superlineare_ovunque = bool(rapporti) and all(
            rapporto is not None and rapporto > SOGLIA_SUPERLINEARE for _, rapporto in rapporti
        )
        if operazione in SUPERLINEARI_A_COPPIE and not superlineare_ovunque:
            fallisci(f"{operazione}: dichiarata superlineare, i profili non lo sono")
        if superlineare_ovunque and operazione not in SUPERLINEARI_A_COPPIE:
            for profilo_id, rapporto in rapporti:
                sospetti.append(f"`{operazione}` {profilo_id}: {rapporto:.2f}")
        modelli = {}
        for variante, profili in varianti.items():
            punti = [p for profilo in profili for p in punti_del_profilo(operazione, profilo)]
            modello = mc.adatta(
                punti,
                celle=operazione in PER_CELLA,
                coppie=operazione in SUPERLINEARI_A_COPPIE,
                c_minimo=1 if operazione in USCITA_COPIA else 0,
            )
            mc.verifica_copertura(operazione, modello, punti)
            modelli[variante] = modello
        voci.append({
            "op": operazione,
            "memoria": modelli["memoria"],
            "spill": modelli.get("spill"),
            "profili": sorted(p["profile_id"] for v in ("memoria", "spill") for p in varianti.get(v, [])),
            "dipende_dai_dati": operazione in DIPENDENTI_DAI_DATI,
        })
    if len(budget_spill) != 1:
        fallisci(f"budget governato dei profili spilled non unico: {sorted(budget_spill)}")
    for insieme, nome in (
        (SUPERLINEARI_A_COPPIE, "SUPERLINEARI_A_COPPIE"),
        (PER_CELLA, "PER_CELLA"),
        (DIPENDENTI_DAI_DATI, "DIPENDENTI_DAI_DATI"),
        (USCITA_COPIA, "USCITA_COPIA"),
    ):
        ignote = insieme - set(per_operazione)
        if ignote:
            fallisci(f"{nome}: operazioni non misurate {sorted(ignote)}")
    return voci, budget_spill.pop(), sospetti


def genera():
    mc.verifica_controesempio_rami()
    dati, impronta = mc.carica()
    voci, budget_spill, sospetti = costruisci(dati)
    righe = [
        "//! Modello di costo delle operazioni tabellari: GENERATO da",
        "//! `scripts/genera_costi_operazioni.py`, non si modifica a mano.",
        "//!",
        "//! Fonte: `data/misure/catalogo-memoria-v4.json` (campagna Windows v4 a",
        f"//! `{dati['data_tools_commit'][:7]}`, `PeakWorkingSet64`; SHA-256 in [`SHA256_MISURE`]).",
        "//!",
        "//! Per operazione e variante, con `R` righe in ingresso (somma dei lati),",
        "//! `B` byte Arrow in ingresso, `K = R * colonne d'uscita`, `P` = righe",
        "//! sinistra per righe destra:",
        "//!",
        "//! ```text",
        "//! picco = S * (a + max(r*R + c*B, r_s*R, c_l*B) + k*K + p*P)",
        "//! ```",
        "//!",
        "//! - `y_i = max(budget_estimate_bytes_i, output_new_buffer_bytes_i, 0)`",
        "//!   per ogni punto osservato di tutti i profili della variante;",
        "//! - piano `a + r*R + c*B (+ k*K | p*P)`: il minimo (somma dei rapporti",
        "//!   previsto/misurato) che copre ogni `y_i`, con `a <= min y` e `c >= 1`",
        "//!   dove l'uscita puo' essere una copia intera degli ingressi;",
        "//! - rami di estrapolazione in larghezza: `r_s` inviluppo per riga del",
        "//!   profilo con le righe piu' strette, `c_l` inviluppo per byte di quello",
        "//!   con le righe piu' larghe;",
        "//! - `k` solo per `pivot` (senza rami), `p` solo per le superlineari",
        "//!   (`cross_join`, `fuzzy_join`, senza altri termini);",
        "//! - coefficienti in millesimi di byte, per eccesso; `S` =",
        "//!   [`FATTORE_SICUREZZA`], per eccesso. Regole in `scripts/modello_costi.py`.",
        "//!",
        "//! Le varianti spilled sono misurate con `max_governed_memory_bytes` pari",
        "//! a [`BUDGET_SPILL_MISURATO`]: il runner non passa ai kernel spilled un",
        "//! margine piu' grande.",
    ]
    if sospetti:
        righe.append("//!")
        righe.append("//! Crescita per riga oltre la soglia in ogni profilo, fra gli ultimi")
        righe.append("//! due campioni, per operazioni lineari nel modello (coperte fino al")
        righe.append("//! campione piu' grande; oltre il dominio misurato, estrapolazione):")
        righe.append("//!")
        for sospetto in sospetti:
            righe.append(f"//! - {sospetto}")
    righe += [
        "",
        "use crate::budget::{Costo, CostoOperazione};",
        "",
        "/// SHA-256 delle misure da cui il modello e' generato.",
        f'pub const SHA256_MISURE: &str = "{impronta}";',
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
        spill = f"Some({mc.costo_rust(voce['spill'], 8)})" if voce["spill"] else "None"
        righe += [
            "    CostoOperazione {",
            f'        op: "{voce["op"]}",',
            f"        in_memoria: {mc.costo_rust(voce['memoria'], 8)},",
            f"        spill: {spill},",
            f"        dipende_dai_dati: {'true' if voce['dipende_dai_dati'] else 'false'},",
            mc.lista_rust("profili", voce["profili"], 8),
            "        esclusi: &[],",
            "    },",
        ]
    righe.append("];")
    return "\n".join(righe) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--verifica", action="store_true", help="confronta senza scrivere")
    argomenti = parser.parse_args()
    testo = genera()
    if argomenti.verifica:
        attuale = USCITA.read_text(encoding="utf-8") if USCITA.exists() else None
        if attuale != testo:
            fallisci(f"{USCITA.relative_to(mc.RADICE)} non corrisponde alle misure: rigenerare")
        print("costi_operazioni.rs allineato alle misure")
        return
    USCITA.write_bytes(testo.encode("utf-8"))
    print(f"scritto {USCITA.relative_to(mc.RADICE)}")


if __name__ == "__main__":
    main()
