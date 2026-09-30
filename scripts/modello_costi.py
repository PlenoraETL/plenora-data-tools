#!/usr/bin/env python3
"""Misure della memoria e adattamento del modello di costo del runner.

Modulo condiviso da `genera_costi_operazioni.py` (tabellari) e
`genera_costi_geo.py` (geo): stesse misure, stessa forma, stesse regole,
aritmetica esatta (`Fraction`), solo libreria standard. Stesso ingresso,
stesso output, byte per byte.

Le misure stanno in `data/misure/catalogo-memoria-v4.json`, estratte (i soli
campi usati, con la provenienza e lo SHA-256 del catalogo d'origine) dal
catalogo empirico v4 della campagna di misura:

    python scripts/modello_costi.py --estrai <catalog-v4.json>

Forma, per operazione e variante:

    picco = S * (a + max(r*R + c*B, r_s*R, c_l*B) + k*K + p*P)

con R = righe in ingresso (somma dei lati), B = byte Arrow in ingresso,
K = R * colonne d'uscita (solo le operazioni con il termine per cella),
P = righe_sx * righe_dx (solo le superlineari). Le regole dell'adattamento
sono in `adatta`; le unita' di un punto in `unita_del_punto`, le stesse che
il runner calcola e che l'oracolo in Rust
(`crates/plenora-pipeline/tests/oracolo_costi.rs`) riproduce.
"""
import argparse
import hashlib
import itertools
import json
import pathlib
import sys
from fractions import Fraction

RADICE = pathlib.Path(__file__).resolve().parent.parent
MISURE = RADICE / "data" / "misure" / "catalogo-memoria-v4.json"

MILLE = 1000
MIB = 1024 * 1024

# Formato del file delle misure nel repository.
SCHEMA_MISURE = 1

# Campi di un punto conservati nel repository.
CAMPI_PUNTO_TABELLARE = (
    "rows",
    "secondary_rows",
    "status",
    "noise_flag",
    "input_buffer_bytes",
    "output_rows",
    "output_columns",
    "output_new_buffer_bytes",
    "incremental_peak_bytes",
    "budget_estimate_bytes",
    "governed_budget_bytes",
)
CAMPI_PUNTO_GEO = (
    "features",
    "vertices",
    "secondary_features",
    "status",
    "noise_flag",
    "input_vertices",
    "input_buffer_bytes",
    "output_rows",
    "output_columns",
    "output_new_buffer_bytes",
    "incremental_peak_bytes",
    "budget_estimate_bytes",
)
CAMPI_PROFILO = (
    "family",
    "operation_id",
    "profile_id",
    "execution",
    "fixture",
    "right_fixture",
    "pairing",
    "configuration",
    "parameter_contract",
    "status",
    "skip_reason",
)

AVVERTENZA = (
    "Memoria affidabile, tempi no: la macchina era carica durante la "
    "campagna (CPU media 68% nella campagna tabellare, 52% in quella geo; "
    "fino a 40 processi di build). La metrica e' PeakWorkingSet64 "
    "incrementale (mediana delle ripetizioni meno la mediana della "
    "baseline), non un'allocazione esatta; nessun fattore di sicurezza "
    "applicato. Misurato a 24698d6: prima delle parti preparate della "
    "validazione OGC (fd3325c, 2697f72) e del runner geo di F4 (2ca8b45, "
    "9e45538, e79d9ee)."
)


def fallisci(messaggio):
    sys.exit(f"modello_costi: {messaggio}")


def ceil_div(numeratore, denominatore):
    return -(-numeratore // denominatore)


def ceil_frazione(valore):
    """Intero per eccesso di un `Fraction` non negativo."""
    return -(-valore.numerator // valore.denominator)


# --- Estrazione ------------------------------------------------------------


def estrai(catalogo):
    """I campi usati del catalogo v4, con provenienza e impronta."""
    testo = catalogo.read_bytes()
    dati = json.loads(testo.decode("utf-8-sig"))
    if dati.get("schema_version") != 4:
        fallisci("atteso il catalogo schema 4")
    provenienza = dati["provenance"]
    campagne = {}
    for famiglia in ("table", "geo"):
        voce = provenienza[famiglia]
        campagne[famiglia] = {
            "campaigns": [
                {
                    chiave: campagna[chiave]
                    for chiave in ("started_at_utc", "finished_at_utc", "data_tools_head", "environment")
                }
                for campagna in voce["campaigns"]
            ],
            "machine_load_summary": voce["machine_load_summary"],
        }
    profili = []
    for profilo in dati["profiles"]:
        famiglia = profilo["family"]
        campi = CAMPI_PUNTO_TABELLARE if famiglia == "table" else CAMPI_PUNTO_GEO
        voce = {chiave: profilo.get(chiave) for chiave in CAMPI_PROFILO}
        voce["points"] = [
            {chiave: punto[chiave] for chiave in campi if chiave in punto}
            for punto in profilo.get("points", [])
        ]
        profili.append(voce)
    profili.sort(key=lambda p: (p["operation_id"], p["profile_id"]))
    return {
        "schema": SCHEMA_MISURE,
        "source": "catalog-v4.json (campagna di misura plenora-data-tools2, schema_version 4)",
        "source_sha256": hashlib.sha256(testo).hexdigest(),
        "data_tools_commit": provenienza["data_tools_commit"],
        "toolchain": provenienza["toolchain"].splitlines()[0],
        "release_profile": provenienza["release_profile"],
        "caveat": AVVERTENZA,
        "semantics": {
            chiave: dati["semantics"][chiave]
            for chiave in ("memory_metric", "budget_estimate", "buffer_accounting", "geo_execution", "geo_axes")
        },
        "campaigns": campagne,
        "profiles": profili,
    }


def scrivi_misure(dati):
    """JSON stabile e leggibile nei diff: un punto per riga."""
    righe = ["{"]
    for chiave, valore in dati.items():
        if chiave == "profiles":
            continue
        blocco = json.dumps(valore, indent=1, ensure_ascii=False).replace("\n", "\n ")
        righe.append(f" {json.dumps(chiave)}: {blocco},")
    righe.append(' "profiles": [')
    profili = dati["profiles"]
    for indice, profilo in enumerate(profili):
        righe.append("  {")
        for chiave in CAMPI_PROFILO:
            righe.append(f"   {json.dumps(chiave)}: {json.dumps(profilo[chiave], ensure_ascii=False)},")
        punti = profilo["points"]
        if punti:
            righe.append('   "points": [')
            righe += [
                "    " + json.dumps(punto, ensure_ascii=False) + ("," if i + 1 < len(punti) else "")
                for i, punto in enumerate(punti)
            ]
            righe.append("   ]")
        else:
            righe.append('   "points": []')
        righe.append("  }" + ("," if indice + 1 < len(profili) else ""))
    righe += [" ]", "}"]
    testo = "\n".join(righe) + "\n"
    if json.loads(testo) != dati:
        fallisci("scrittura delle misure: il testo non rilegge gli stessi dati")
    MISURE.write_text(testo, encoding="utf-8", newline="\n")


def carica():
    """Le misure del repository e il loro SHA-256."""
    testo = MISURE.read_bytes()
    dati = json.loads(testo.decode("utf-8"))
    if dati.get("schema") != SCHEMA_MISURE:
        fallisci(f"{MISURE.name}: schema diverso da {SCHEMA_MISURE}")
    return dati, hashlib.sha256(testo).hexdigest()


# --- Punti -----------------------------------------------------------------


def y(punto):
    """Picco da coprire: la stima di budget del catalogo (massimo fra picco
    incrementale e crescita di fase), mai meno dei byte nuovi dell'output,
    mai negativa (i punti inconcludenti non abbassano il modello)."""
    return max(punto["budget_estimate_bytes"], punto["output_new_buffer_bytes"], 0)


def unita_del_punto(operazione, punto):
    """R, B, K, P di un punto osservato, come li calcola il runner.

    - tabellari: R = rows + secondary_rows, P = rows * secondary_rows;
    - geo: R = features + secondary_features, P = features *
      secondary_features; `geo.generate_grid` non ha ingresso e usa come R
      le celle d'uscita (il runner: le righe dell'uscita note a secco);
    - B = byte Arrow dell'ingresso (buffer deduplicati per allocazione,
      la misura di `byte_vivi`); K = R * colonne d'uscita.
    """
    if "rows" in punto:
        primarie, secondarie = punto["rows"], punto["secondary_rows"]
    else:
        primarie, secondarie = punto["features"], punto["secondary_features"]
    righe = primarie + secondarie
    if operazione == "geo.generate_grid":
        righe = max(righe, punto["output_rows"])
    return {
        "R": righe,
        "B": punto["input_buffer_bytes"],
        "K": righe * punto["output_columns"],
        "P": primarie * secondarie,
    }


def classe_di_larghezza(profilo, punto):
    """Le tabellari hanno una larghezza di riga per profilo; le geo una per
    profilo e numero di vertici per geometria."""
    if "vertices" in punto:
        return f"{profilo['profile_id']}/{punto['vertices']}"
    return profilo["profile_id"]


def punti_osservati(profilo):
    return [p for p in profilo["points"] if p.get("status") == "observed"]


# --- Adattamento -----------------------------------------------------------


def _risolvi(matrice, termini):
    """Soluzione esatta di un sistema quadrato; None se singolare."""
    n = len(matrice)
    righe = [list(riga) + [termine] for riga, termine in zip(matrice, termini)]
    for colonna in range(n):
        perno = next((i for i in range(colonna, n) if righe[i][colonna] != 0), None)
        if perno is None:
            return None
        righe[colonna], righe[perno] = righe[perno], righe[colonna]
        for i in range(n):
            if i != colonna and righe[i][colonna] != 0:
                fattore = righe[i][colonna] / righe[colonna][colonna]
                righe[i] = [x - fattore * z for x, z in zip(righe[i], righe[colonna])]
    return [righe[i][n] / righe[i][i] for i in range(n)]


def piano_minimo(unita, picchi, variabili, minimi, massimi):
    """Il piano `sum_v coeff_v * unita_v` che copre ogni punto (`>= picco`)
    con `minimi <= coeff <= massimi`, e che minimizza la somma dei rapporti
    previsto/misurato (misurato almeno 1 MiB: i punti piccoli non pesano
    piu' del rumore). Programma lineare risolto per enumerazione esatta dei
    vertici; a parita', il `c` piu' alto (per byte: per eccesso su righe piu'
    larghe), poi i coefficienti piu' bassi nell'ordine delle variabili.
    """
    n = len(variabili)
    pesi = [Fraction(1, max(picco, MIB)) for picco in picchi]
    costo = [sum(peso * u[v] for peso, u in zip(pesi, unita)) for v in variabili]
    vincoli = [([Fraction(u[v]) for v in variabili], Fraction(picco)) for u, picco in zip(unita, picchi)]
    for indice, v in enumerate(variabili):
        vincoli.append(([Fraction(int(j == indice)) for j in range(n)], Fraction(minimi.get(v, 0))))
        if v in massimi:
            vincoli.append(([Fraction(-int(j == indice)) for j in range(n)], -Fraction(massimi[v])))
    indice_c = variabili.index("c") if "c" in variabili else None
    migliore = None
    for scelti in itertools.combinations(range(len(vincoli)), n):
        soluzione = _risolvi([vincoli[i][0] for i in scelti], [vincoli[i][1] for i in scelti])
        if soluzione is None:
            continue
        if any(sum(x * s for x, s in zip(riga, soluzione)) < termine for riga, termine in vincoli):
            continue
        obiettivo = sum(x * s for x, s in zip(costo, soluzione))
        per_byte = soluzione[indice_c] if indice_c is not None else 0
        chiave = (obiettivo, -per_byte, soluzione)
        if migliore is None or chiave < migliore:
            migliore = chiave
    if migliore is None:
        raise ValueError("nessun vertice ammissibile")
    return dict(zip(variabili, migliore[2]))


def adatta(punti, *, celle=False, coppie=False, senza_byte=False, c_minimo=0):
    """Coefficienti interi per eccesso: `a` in byte, gli altri in millesimi
    di byte per unita'.

    `punti`: dizionari con `R`, `B`, `K`, `P`, `y`, `classe`.

    1. Il piano `a + r*R + c*B (+ k*K | p*P)` minimo che copre ogni punto
       (`piano_minimo`), con `a <= min y` (la costante c'e' a ogni
       dimensione: la crescita la portano i termini per unita') e `c >=
       c_minimo` (1 per le operazioni la cui uscita puo' essere una copia
       intera degli ingressi anche dove le fixture ne tengono una parte).
    2. Superlineari (`coppie`): solo `a` e il termine a coppie. Con il
       termine per cella (`celle`): nessun ramo, la larghezza dell'uscita la
       da' K. Senza ingresso (`senza_byte`): solo `a` e il termine per riga.
    3. Altrimenti i due rami di estrapolazione in larghezza di riga (B/R):
       `r_s` = inviluppo `max (y - a) / R` della classe di larghezza piu'
       stretta, `c_l` = inviluppo `max (y - a) / B` della piu' larga. Se il
       costo vero e' una somma non negativa di un termine per riga e uno per
       byte, `r_s*R` lo copre su righe piu' strette di ogni classe misurata
       e `c_l*B` su righe piu' larghe; con una sola classe i due rami danno
       `max(r_s*R, c_l*B)`, per eccesso a ogni larghezza.
    """
    if coppie:
        variabili = ["a", "p"]
    elif senza_byte:
        variabili = ["a", "r"]
    elif celle:
        variabili = ["a", "r", "c", "k"]
    else:
        variabili = ["a", "r", "c"]
    unita = [{"a": 1, "r": p["R"], "c": p["B"], "k": p["K"], "p": p["P"]} for p in punti]
    picchi = [p["y"] for p in punti]
    minimi = {"c": Fraction(c_minimo)} if "c" in variabili else {}
    massimi = {"a": min(picchi)}
    piano = piano_minimo(unita, picchi, variabili, minimi, massimi)
    modello = {"a": ceil_frazione(piano["a"]), "r": 0, "c": 0, "k": 0, "p": 0, "r_s": 0, "c_l": 0}
    for v in variabili:
        if v != "a":
            modello[v] = ceil_frazione(piano[v] * MILLE)
    if not (coppie or celle or senza_byte):
        classi = {}
        for p in punti:
            classi.setdefault(p["classe"], []).append(p)

        def larghezza(nome):
            grande = max(classi[nome], key=lambda q: (q["R"], q["B"]))
            return Fraction(grande["B"], grande["R"])

        stretta = min(classi, key=lambda nome: (larghezza(nome), nome))
        larga = max(classi, key=lambda nome: (larghezza(nome), nome))
        a = modello["a"]
        modello["r_s"] = max(ceil_div(max(p["y"] - a, 0) * MILLE, p["R"]) for p in classi[stretta])
        modello["c_l"] = max(ceil_div(max(p["y"] - a, 0) * MILLE, p["B"]) for p in classi[larga])
    return modello


def per_unita(millesimi, unita):
    return ceil_div(millesimi * unita, MILLE)


def base(modello, unita):
    """`a + max(r*R + c*B, r_s*R, c_l*B) + k*K + p*P` come il runner, per
    eccesso termine per termine, senza fattore di sicurezza."""
    piano = per_unita(modello["r"], unita["R"]) + per_unita(modello["c"], unita["B"])
    variabile = max(
        piano,
        per_unita(modello["r_s"], unita["R"]),
        per_unita(modello["c_l"], unita["B"]),
    )
    return modello["a"] + variabile + per_unita(modello["k"], unita["K"]) + per_unita(modello["p"], unita["P"])


def verifica_copertura(operazione, modello, punti):
    """Ogni punto adattato e' coperto dal modello arrotondato: il vincolo
    del generatore, oltre a quello dell'oracolo in Rust."""
    for punto in punti:
        if base(modello, punto) < punto["y"]:
            fallisci(f"{operazione}: un punto misurato resta sopra il modello")


def rapporto(modello, punto, fattore):
    """Previsto (con il fattore di sicurezza) su misurato."""
    numeratore, denominatore = fattore
    previsto = ceil_div(base(modello, punto) * numeratore, denominatore)
    return Fraction(previsto, max(punto["y"], 1))


# --- Emissione Rust --------------------------------------------------------


CAMPI_COSTO = (
    ("a", "a"),
    ("r_millesimi", "r"),
    ("c_millesimi", "c"),
    ("r_stretta_millesimi", "r_s"),
    ("c_larga_millesimi", "c_l"),
    ("k_millesimi", "k"),
    ("p_millesimi", "p"),
)


def costo_rust(modello, rientro):
    """Un `Costo` nella forma di rustfmt, con i separatori delle migliaia."""
    dentro = " " * (rientro + 4)
    righe = ["Costo {"]
    righe += [f"{dentro}{campo}: {modello[chiave]:_}," for campo, chiave in CAMPI_COSTO]
    righe.append(" " * rientro + "}")
    return "\n".join(righe)


def lista_rust(nome, valori, rientro):
    """`nome: &[...]`, su una riga o una voce per riga oltre `array_width`
    di rustfmt (60)."""
    spazi = " " * rientro
    elementi = ", ".join(f'"{v}"' for v in valori)
    if len(f"&[{elementi}]") <= 60:
        return f"{spazi}{nome}: &[{elementi}],"
    return "\n".join([f"{spazi}{nome}: &["] + [f'{spazi}    "{v}",' for v in valori] + [f"{spazi}],"])


def main():
    argomenti = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    argomenti.add_argument("--estrai", type=pathlib.Path, required=True, help="catalog-v4.json")
    scelti = argomenti.parse_args()
    scrivi_misure(estrai(scelti.estrai))
    print(f"scritto {MISURE.relative_to(RADICE)}")


if __name__ == "__main__":
    main()
