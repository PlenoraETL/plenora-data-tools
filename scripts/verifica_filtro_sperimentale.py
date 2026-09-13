#!/usr/bin/env python3
"""Prova la provenienza del vendor FILTRATO sperimentale
(`vendor/geo-0.33.1-exact-filtered`), separata dalla provenienza del
candidato congelato adottato (quella resta `verifica_vendor_provenienza.py`,
non toccata da questo script).

Stessa metodologia del candidato congelato — ricostruzione verificata, non
fiducia sul contenuto committato — ma con una base diversa: qui la radice di
fiducia non e' il pacchetto .crate upstream, e' `vendor/geo-0.33.1-exact`
**gia' verificato** da `verifica_vendor_provenienza.py` (eseguito a parte:
questo script non lo ripete). Sopra quella base si applica
`patches/orient2d-filtro-sperimentale.patch` (identita' verificata per
sha256, come le patch congelate) e il digest dell'albero risultante deve
coincidere con `vendor/geo-0.33.1-exact-filtered`.

In piu', un controllo che le patch del candidato congelato non hanno: il
contenuto di `orient2d_filtered.rs` deve corrispondere all'hash registrato
qui, lo stesso prodotto dalla qualifica standalone
(`plenora-memlab-filtro-sperimentale/`, 6788/6788 casi contro l'oracolo
razionale, debug e release) — prova che il file integrato in questo
workspace e' esattamente quello qualificato, non una copia ridigitata.

Uso: `python scripts/verifica_filtro_sperimentale.py` dalla radice del
repository. Richiede l'eseguibile `patch`, nessun accesso alla rete (la base
e' gia' su disco, verificata a parte).
"""

from __future__ import annotations

import hashlib
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PATCHES = ROOT / "patches"
VENDOR = ROOT / "vendor"

BASE_VENDOR = VENDOR / "geo-0.33.1-exact"
FILTRATO_VENDOR = VENDOR / "geo-0.33.1-exact-filtered"
PATCH_FILE = PATCHES / "orient2d-filtro-sperimentale.patch"

# sha256 del patch file, calcolato quando la ricostruzione sotto e' stata
# verificata corrispondere bit-per-bit a vendor/geo-0.33.1-exact-filtered.
PATCH_ATTESO_SHA256 = "5179b095e515c86765a78433f179c061f504629c1c93ce2876e94c25b22ffc98"

# sha256 di orient2d_filtered.rs cosi' come qualificato in
# plenora-memlab-filtro-sperimentale/ (6788/6788, debug e release, oracolo
# razionale + riferimento esatto + confini del filtro) — non ricalcolato
# da questo repository, registrato al momento della qualifica.
FILTRO_QUALIFICATO_SHA256 = "221a04dbfcaeaf9814f8c8fbf1a7a82e46cc07db6097e476e15ee6848f4ce121"

# Stessa esclusione di verifica_vendor_provenienza.py, per lo stesso motivo:
# PROVENANCE*.md sono documentazione aggiunta da questo repository, non
# prodotta dalla ricostruzione patch+base.
DIGEST_ESCLUSI = {
    "Cargo.toml.orig",
    ".cargo-ok",
    ".cargo-checksum.json",
    "PROVENANCE.md",
    "PROVENANCE-FILTRO-SPERIMENTALE.md",
    "Cargo.lock",
}


def digest_albero(radice: Path, esclusi: set[str]) -> str:
    voci = []
    for percorso in sorted(radice.rglob("*")):
        if not percorso.is_file():
            continue
        relativo = percorso.relative_to(radice).as_posix()
        if relativo in esclusi or relativo.split("/", 1)[0] == "target":
            continue
        voci.append((relativo, hashlib.sha256(percorso.read_bytes()).hexdigest()))
    hash_finale = hashlib.sha256()
    for relativo, hash_file in voci:
        hash_finale.update(f"{relativo}\n{hash_file}\n".encode("utf-8"))
    return hash_finale.hexdigest()


def verifica_identita_patch() -> None:
    if not PATCH_FILE.is_file():
        raise SystemExit(f"patch attesa assente: {PATCH_FILE}")
    reale = hashlib.sha256(PATCH_FILE.read_bytes()).hexdigest()
    if reale != PATCH_ATTESO_SHA256:
        raise SystemExit(
            f"{PATCH_FILE.name}: sha256 {reale} diverso dall'atteso "
            f"{PATCH_ATTESO_SHA256} — non e' la stessa patch verificata."
        )
    print(f"OK  {PATCH_FILE.name}: identica alla patch verificata")


def verifica_base_intatta() -> None:
    """La base non e' riverificata contro crates.io qui (compito di
    verifica_vendor_provenienza.py, che va eseguito a parte): si controlla
    solo che esista, non che questo script la finga verificata."""
    if not BASE_VENDOR.is_dir():
        raise SystemExit(
            f"{BASE_VENDOR} assente: eseguire prima verifica_vendor_provenienza.py, "
            "questo script parte da quella base gia' su disco."
        )


def verifica_ricostruzione() -> None:
    if not FILTRATO_VENDOR.is_dir():
        raise SystemExit(f"{FILTRATO_VENDOR} assente: nulla da verificare")

    with tempfile.TemporaryDirectory(prefix="filtro-sperimentale-") as tmp:
        albero = Path(tmp) / "geo-0.33.1-exact-filtered"
        import shutil

        shutil.copytree(BASE_VENDOR, albero)
        esito = subprocess.run(
            ["patch", "-p1", "--forward"],
            cwd=albero,
            stdin=PATCH_FILE.open("rb"),
            capture_output=True,
            check=False,
        )
        if esito.returncode != 0:
            raise SystemExit(
                f"applicazione di {PATCH_FILE.name} fallita:\n"
                f"{esito.stdout.decode(errors='replace')}\n{esito.stderr.decode(errors='replace')}"
            )

        atteso = digest_albero(albero, DIGEST_ESCLUSI)
        reale = digest_albero(FILTRATO_VENDOR, DIGEST_ESCLUSI)
        if atteso != reale:
            raise SystemExit(
                "vendor/geo-0.33.1-exact-filtered: digest non corrisponde.\n"
                f"  ricostruito da base+patch: {atteso}\n"
                f"  albero committato:         {reale}\n"
                "Non e' cio' che la base verificata + la patch registrata producono."
            )
        print(
            f"OK  geo-0.33.1-exact-filtered: digest albero {atteso[:16]}... "
            "coincide con base verificata + patch"
        )


def verifica_filtro_qualificato() -> None:
    file_filtro = FILTRATO_VENDOR / "src" / "algorithm" / "kernels" / "orient2d_filtered.rs"
    if not file_filtro.is_file():
        raise SystemExit(f"file del filtro assente: {file_filtro}")
    reale = hashlib.sha256(file_filtro.read_bytes()).hexdigest()
    if reale != FILTRO_QUALIFICATO_SHA256:
        raise SystemExit(
            f"orient2d_filtered.rs: sha256 {reale} diverso dal file qualificato "
            f"{FILTRO_QUALIFICATO_SHA256} (plenora-memlab-filtro-sperimentale/, "
            "6788/6788 casi) — il file integrato qui non e' quello qualificato."
        )
    print("OK  orient2d_filtered.rs: identico al file qualificato (6788/6788, debug e release)")


def verifica_fallback_esatto_intatto() -> None:
    """Il kernel sempre-esatto usato come ricaduta dal filtro deve restare
    l'identico file congelato -- non una copia modificata."""
    frozen = BASE_VENDOR / "src" / "algorithm" / "kernels" / "exact_orientation.rs"
    filtrato = FILTRATO_VENDOR / "src" / "algorithm" / "kernels" / "exact_orientation.rs"
    a, b = frozen.read_bytes(), filtrato.read_bytes()
    if a != b:
        raise SystemExit(
            "exact_orientation.rs: il fallback esatto nel vendor filtrato "
            "differisce dal congelato — non dovrebbe mai essere toccato."
        )
    print("OK  exact_orientation.rs: fallback esatto identico al congelato, non modificato")


def main() -> int:
    verifica_base_intatta()
    verifica_identita_patch()
    verifica_ricostruzione()
    verifica_filtro_qualificato()
    verifica_fallback_esatto_intatto()
    print(
        "\nProvenienza del filtro sperimentale verificata: base congelata (verificata a parte) "
        "+ patch registrata = vendor committato; filtro identico al qualificato; "
        "fallback esatto intatto."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
