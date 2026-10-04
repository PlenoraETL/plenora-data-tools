#!/usr/bin/env python3
"""Genera `docs/inventario.md` leggendo i sorgenti, e verifica che sia aggiornato.

Crate, operazioni del catalogo, operazioni pubbliche della CLI e numero dei
test vivono nel codice. Questo generatore li rende senza duplicarli a mano;
con `--check` (o `--verifica`, come gli altri generatori del repository)
non scrive ed esce 1 se una rigenerazione produrrebbe differenze.

Uso:

    python scripts/genera_inventario.py            # riscrive il documento
    python scripts/genera_inventario.py --check    # esce 1 se è disallineato

Legge il testo dei sorgenti, non compila nulla: un formato che non
riconosce (una voce del catalogo con meno campi, una tabella vuota) è un
errore, non una riga che sparisce.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from collections import Counter
from pathlib import Path

RADICE = Path(__file__).resolve().parents[1]
DESTINAZIONE = RADICE / "docs" / "inventario.md"
CATALOGO = RADICE / "crates" / "plenora-core" / "src" / "catalog.rs"
OPERAZIONI_CLI = RADICE / "crates" / "plenora-cli" / "src" / "operazioni.rs"

# Campi posizionali di `op!` (`crates/plenora-core/src/catalog_op.rs`).
CAMPI_OP = (
    "id",
    "famiglia",
    "origine",
    "arita",
    "esecuzione",
    "annullamento",
    "forma",
    "crs",
    "capacita",
    "determinismo",
    "maturita",
)
ATTRIBUTO_TEST = re.compile(r"^\s*#\[test\]", re.MULTILINE)
SELF_TEST = re.compile(r"^\s*def (test_\w+)\(", re.MULTILINE)
# Le prove pytest dell'SDK Python, anche asincrone.
PROVA_PYTHON = re.compile(r"^\s*(?:async\s+)?def (test_\w+)\(", re.MULTILINE)
CARTELLA_PROVE_PYTHON = Path("crates/plenora-data-py/python/tests")


class ErroreInventario(RuntimeError):
    """Un sorgente non ha la forma che il generatore sa leggere."""


def corpo_tra(testo: str, apertura: int, aperta: str, chiusa: str) -> tuple[str, int]:
    """Il contenuto fra la parentesi in `apertura` e la sua chiusura.

    Le stringhe Rust ordinarie si saltano: una parentesi in un letterale non
    chiude niente.
    """
    profondita = 0
    indice = apertura
    while indice < len(testo):
        carattere = testo[indice]
        if carattere == '"':
            indice += 1
            while indice < len(testo) and testo[indice] != '"':
                indice += 2 if testo[indice] == "\\" else 1
        elif carattere == aperta:
            profondita += 1
        elif carattere == chiusa:
            profondita -= 1
            if profondita == 0:
                return testo[apertura + 1 : indice], indice
        indice += 1
    raise ErroreInventario(f"parentesi {aperta} non chiusa")


def argomenti(corpo: str) -> list[str]:
    """Gli argomenti di una chiamata, separati dalle virgole di primo livello."""
    parti: list[str] = []
    profondita = 0
    corrente: list[str] = []
    in_stringa = False
    for carattere in corpo:
        if in_stringa:
            corrente.append(carattere)
            if carattere == '"':
                in_stringa = False
            continue
        if carattere == '"':
            in_stringa = True
        elif carattere in "([{":
            profondita += 1
        elif carattere in ")]}":
            profondita -= 1
        elif carattere == "," and profondita == 0:
            parti.append("".join(corrente).strip())
            corrente = []
            continue
        corrente.append(carattere)
    resto = "".join(corrente).strip()
    if resto:
        parti.append(resto)
    return parti


def senza_commenti(testo: str) -> str:
    return "\n".join(
        riga for riga in testo.split("\n") if not riga.lstrip().startswith("//")
    )


def voci_catalogo() -> list[dict[str, str]]:
    """Le voci `op!` di `CATALOG`, con i campi posizionali per nome."""
    testo = senza_commenti(CATALOGO.read_text(encoding="utf-8"))
    marcatore = "pub static CATALOG: &[OperationDescriptor] = &"
    inizio = testo.find(marcatore)
    if inizio < 0:
        raise ErroreInventario("CATALOG non trovato")
    corpo, _ = corpo_tra(testo, testo.index("[", inizio + len(marcatore)), "[", "]")
    voci = []
    cursore = 0
    while (posizione := corpo.find("op!(", cursore)) >= 0:
        argomenti_op, fine = corpo_tra(corpo, posizione + 3, "(", ")")
        valori = argomenti(argomenti_op)
        if len(valori) < len(CAMPI_OP):
            raise ErroreInventario(f"voce del catalogo con {len(valori)} campi")
        voce = dict(zip(CAMPI_OP, valori))
        voce["id"] = voce["id"].strip('"')
        voci.append(voce)
        cursore = fine
    if not voci:
        raise ErroreInventario("catalogo vuoto")
    return voci


def numero_alias() -> int:
    testo = senza_commenti(CATALOGO.read_text(encoding="utf-8"))
    marcatore = "pub static ALIASES: &[(u16, &str, &str)] = &"
    inizio = testo.find(marcatore)
    if inizio < 0:
        raise ErroreInventario("ALIASES non trovato")
    corpo, _ = corpo_tra(testo, testo.index("[", inizio + len(marcatore)), "[", "]")
    return len(re.findall(r"\(\s*\d+\s*,", corpo))


def operazioni_cli() -> list[dict[str, str]]:
    """Le voci di `OPERAZIONI` della CLI, con i campi letterali."""
    testo = senza_commenti(OPERAZIONI_CLI.read_text(encoding="utf-8"))
    marcatore = "pub const OPERAZIONI: &[OperazionePubblica] = &"
    inizio = testo.find(marcatore)
    if inizio < 0:
        raise ErroreInventario("OPERAZIONI non trovato")
    corpo, _ = corpo_tra(testo, testo.index("[", inizio + len(marcatore)), "[", "]")
    voci = []
    cursore = 0
    while (posizione := corpo.find("OperazionePubblica {", cursore)) >= 0:
        campi, fine = corpo_tra(corpo, corpo.index("{", posizione), "{", "}")
        voce = {}
        for campo in argomenti(campi):
            nome, _, valore = campo.partition(":")
            voce[nome.strip()] = valore.strip()
        voci.append(voce)
        cursore = fine
    if not voci:
        raise ErroreInventario("nessuna operazione pubblica della CLI")
    return voci


def crate() -> list[tuple[str, str, str]]:
    """Nome, versione e descrizione di ogni crate del workspace."""
    radice = tomllib.loads((RADICE / "Cargo.toml").read_text(encoding="utf-8"))
    condivisa = radice["workspace"]["package"]["version"]
    trovati = []
    for manifesto in sorted((RADICE / "crates").glob("*/Cargo.toml")):
        pacchetto = tomllib.loads(manifesto.read_text(encoding="utf-8"))["package"]
        versione = pacchetto.get("version")
        if isinstance(versione, dict) and versione.get("workspace") is True:
            versione = condivisa
        if not isinstance(versione, str):
            raise ErroreInventario(f"versione assente: {manifesto}")
        trovati.append((pacchetto["name"], versione, pacchetto.get("description", "")))
    if not trovati:
        raise ErroreInventario("nessun crate")
    return trovati


def conta_test(cartella: Path) -> int:
    return sum(
        len(ATTRIBUTO_TEST.findall(percorso.read_text(encoding="utf-8")))
        for percorso in sorted(cartella.rglob("*.rs"))
    ) if cartella.is_dir() else 0


def inventario_test() -> list[tuple[str, int, int]]:
    """Funzioni `#[test]` per crate: nei sorgenti (unitari) e in `tests/`."""
    return [
        (cartella.name, conta_test(cartella / "src"), conta_test(cartella / "tests"))
        for cartella in sorted((RADICE / "crates").iterdir())
        if (cartella / "Cargo.toml").is_file()
    ]


def self_test_script() -> list[tuple[str, int]]:
    """Le prove Python delle guardie e dei generatori in `scripts/`."""
    return [
        (percorso.name, len(SELF_TEST.findall(percorso.read_text(encoding="utf-8"))))
        for percorso in sorted((RADICE / "scripts").glob("test_*.py"))
    ]


def prove_python() -> list[tuple[str, int]]:
    """Le prove pytest dell'SDK Python, per file (`test_*.py`)."""
    cartella = RADICE / CARTELLA_PROVE_PYTHON
    return [
        (percorso.name, len(PROVA_PYTHON.findall(percorso.read_text(encoding="utf-8"))))
        for percorso in sorted(cartella.glob("test_*.py"))
    ] if cartella.is_dir() else []


def tabella(intestazione: list[str], righe: list[list[str]]) -> list[str]:
    linee = ["| " + " | ".join(intestazione) + " |"]
    linee.append("| " + " | ".join("---" for _ in intestazione) + " |")
    linee += ["| " + " | ".join(riga) + " |" for riga in righe]
    return linee


def conteggi(voci: list[dict[str, str]], campo: str) -> list[list[str]]:
    contati = Counter(voce[campo] for voce in voci)
    return [[f"`{nome}`", str(numero)] for nome, numero in sorted(contati.items())]


def render() -> str:
    voci = voci_catalogo()
    famiglie = Counter(voce["famiglia"] for voce in voci)
    linee = [
        "# Inventario del codice",
        "",
        "<!-- Generato da scripts/genera_inventario.py: non si modifica a mano.",
        "Rigenerare con python scripts/genera_inventario.py -->",
        "",
        "Ogni numero qui sotto è letto dai sorgenti. Se è sbagliato, è",
        "sbagliato nel codice, oppure il documento non è stato rigenerato:",
        "`python scripts/genera_inventario.py --check` lo dice, e la CI lo",
        "esegue.",
        "",
        "## Crate",
        "",
    ]
    linee += tabella(
        ["crate", "versione", "descrizione"],
        [[f"`{nome}`", versione, descrizione] for nome, versione, descrizione in crate()],
    )
    linee += [
        "",
        "## Catalogo delle operazioni",
        "",
        f"`plenora_core::catalog::CATALOG` ha {len(voci)} operazioni "
        f"({famiglie.get('Table', 0)} tabellari, {famiglie.get('Geo', 0)} geografiche) "
        f"e `ALIASES` {numero_alias()} alias. Le schede sono in",
        "[`operazioni.md`](operazioni.md).",
        "",
        "### Per famiglia e provenienza",
        "",
    ]
    coppie = Counter((voce["famiglia"], voce["origine"]) for voce in voci)
    linee += tabella(
        ["famiglia", "provenienza", "operazioni"],
        [
            [f"`{famiglia}`", f"`{origine}`", str(numero)]
            for (famiglia, origine), numero in sorted(coppie.items())
        ],
    )
    for titolo, campo in (
        ("Per classe di esecuzione", "esecuzione"),
        ("Per annullamento", "annullamento"),
        ("Per maturità", "maturita"),
    ):
        linee += ["", f"### {titolo}", ""]
        linee += tabella([campo, "operazioni"], conteggi(voci, campo))
    linee += [
        "",
        "## Operazioni pubbliche della CLI",
        "",
        "Da `plenora_cli::operazioni::OPERAZIONI`, la tabella da cui la CLI",
        "ricava aiuto, `capabilities` e mappa degli export Rust",
        "([`cli.md`](cli.md)).",
        "",
    ]
    linee += tabella(
        ["id", "versione", "comando", "effetto", "annullamento", "scadenza",
         "tabelle intere in memoria"],
        [
            [
                f"`{voce['id'].strip(chr(34))}`",
                voce["versione"],
                f"`{voce['comando'].strip(chr(34))}`",
                f"`{voce['effetto'].removeprefix('Effetto::')}`",
                voce["annullamento"],
                voce["scadenza"],
                voce["materializzazione_limitata"],
            ]
            for voce in operazioni_cli()
        ],
    )
    test = inventario_test()
    linee += [
        "",
        "## Test",
        "",
        "Funzioni annotate `#[test]` (comprese quelle dentro `proptest!`), per",
        "crate: nei sorgenti i test unitari, in `tests/` quelli d'integrazione.",
        "Doctest ed esempi non sono contati. Quanti casi gira un test dipende",
        "dalla suite ([«Suite lunga»](../README.md#suite-lunga)).",
        "",
    ]
    linee += tabella(
        ["crate", "unitari", "integrazione", "totale"],
        [[f"`{nome}`", str(src), str(tst), str(src + tst)] for nome, src, tst in test]
        + [[
            "**totale**",
            str(sum(src for _, src, _ in test)),
            str(sum(tst for _, _, tst in test)),
            str(sum(src + tst for _, src, tst in test)),
        ]],
    )
    python = prove_python()
    if python:
        linee += [
            "",
            "### Prove Python dell'SDK",
            "",
            "Funzioni `test_*` (anche `async`) della suite pytest di `plenora-data-py`,",
            "che gira sul wheel installato (`scripts/verifica_sdk_python.py`): il",
            "`plenora-data-py` della tabella sopra conta solo i `#[test]` Rust.",
            "",
        ]
        linee += tabella(
            ["file", "prove"],
            [[f"`{CARTELLA_PROVE_PYTHON.as_posix()}/{nome}`", str(numero)] for nome, numero in python]
            + [["**totale**", str(sum(numero for _, numero in python))]],
        )
    script = self_test_script()
    if script:
        linee += [
            "",
            "### Prove delle guardie",
            "",
            "Funzioni `test_*` dei self-test Python in `scripts/`.",
            "",
        ]
        linee += tabella(
            ["file", "prove"], [[f"`scripts/{nome}`", str(numero)] for nome, numero in script]
        )
    linee.append("")
    return "\n".join(linee)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument(
        "--check",
        "--verifica",
        dest="verifica",
        action="store_true",
        help="non scrive: esce 1 se il documento è disallineato",
    )
    opzioni = parser.parse_args(argv)
    try:
        generato = render()
    except (ErroreInventario, OSError, KeyError, ValueError) as errore:
        print(f"inventario: {errore}", file=sys.stderr)
        return 1
    if opzioni.verifica:
        attuale = (
            DESTINAZIONE.read_text(encoding="utf-8") if DESTINAZIONE.is_file() else ""
        )
        if attuale != generato:
            print(
                "inventario: docs/inventario.md non è allineato ai sorgenti; "
                "rigenerarlo con python scripts/genera_inventario.py",
                file=sys.stderr,
            )
            return 1
        print("inventario: docs/inventario.md allineato")
        return 0
    DESTINAZIONE.write_text(generato, encoding="utf-8", newline="\n")
    print(f"inventario: scritto {DESTINAZIONE.relative_to(RADICE).as_posix()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
