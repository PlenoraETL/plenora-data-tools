#!/usr/bin/env python3
"""Prova la provenienza di vendor/{geo,wkt,i_shape}-*: pacchetto .crate
verificato per checksum + patch congelate applicate in ordine, digest
dell'albero risultante confrontato con quanto committato.

Due nozioni distinte, apposta non confuse:

- **checksum del pacchetto**: sha256 del file `.crate` (tar.gz) cosi' come
  pubblicato su crates.io, lo stesso valore gia' registrato in `Cargo.lock`.
  E' la prova che il pacchetto di partenza e' quello vero, prima di qualunque
  estrazione.
- **digest dell'albero**: sha256 calcolato sui file estratti (e patchati),
  percorso per percorso. Cambia rappresentazione (tar -> directory), quindi
  e' un numero diverso dal checksum del pacchetto per costruzione: non e' un
  refuso se non coincidono, sarebbe un refuso se qualcuno li confondesse.

Uso: `python scripts/verifica_vendor_provenienza.py` dalla radice del
repository. Richiede l'eseguibile `patch` (git-bash/MSYS su Windows, di
sistema su Linux/macOS) e una copia in cache o raggiungibile in rete del
pacchetto .crate di ciascuna dipendenza.
"""

from __future__ import annotations

import hashlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PATCHES = ROOT / "patches"
VENDOR = ROOT / "vendor"

# Radice di fiducia: checksum del pacchetto .crate al momento della
# vendorizzazione, verificato a mano contro i byte scaricati (non preso a
# scatola chiusa da Cargo.lock). Deliberatamente NON letto dal Cargo.lock
# corrente: una volta che [patch.crates-io] e' adottato, Cargo.lock smette di
# portare un checksum per queste tre dipendenze (source e' un path, non un
# registry) — e' esattamente il momento in cui questo controllo serve di
# piu'. Se in futuro si aggiorna la versione a monte, questi valori vanno
# aggiornati qui insieme al nuovo pacchetto e alle patch riadattate; non
# prima.
CHECKSUM_PACCHETTO = {
    "geo": ("0.33.1", "30eb1fdc57c1e5cfd11826fe0caec4b9dc7901f3758263bb506228d88c8d9e9a"),
    "wkt": ("0.14.0", "efb2b923ccc882312e559ffaa832a055ba9d1ac0cc8e86b3e25453247e4b81d7"),
    "i_shape": ("1.18.0", "bfa9eac533d7509a8ab87672b60ac610c17240f9ea4851d26227689fdfe349c8"),
}

# Identita' delle patch: sha256 cosi' come registrato in
# handoff-final-20260909/manifest.json (il congelamento del laboratorio),
# non ricalcolato da questo repository. Prova che patches/*.patch sono copie
# esatte di cio' che il laboratorio ha davvero testato — non una
# riscrittura, non una versione rigenerata che applica lo stesso effetto con
# byte diversi. Per questo `patches/.gitattributes` le esclude dalla
# normalizzazione di fine riga del repository: cambiare quei byte a valle
# romperebbe questa identita' senza toccare il contenuto logico.
PATCH_ATTESO = {
    "geo-exact-orientation.patch": "e58e1f2742d5ff28e3968a63629c811d35d089345fc3eadbb9beed7c1f45b2dc",
    "logging.patch": "8e10ce7af6fa3c457c789dd3ffe1c0b28b5ce099f24f6d708ffacae4a7777924",
    "wkt-v2.patch": "0e1eed40f381de10bc9378640073027b588bffa63f46b18998485e7bca3ae1dd",
    "i_shape.patch": "1df0739b088cbfd6cfabc88ea145b3fb6af3391ff7d4bd250ab8c305b9d553e0",
}


def verifica_identita_patch() -> None:
    for nome, atteso in PATCH_ATTESO.items():
        percorso = PATCHES / nome
        if not percorso.is_file():
            raise SystemExit(f"patch attesa assente: {percorso}")
        reale = hashlib.sha256(percorso.read_bytes()).hexdigest()
        if reale != atteso:
            raise SystemExit(
                f"{nome}: sha256 diverso dal manifesto del laboratorio "
                f"(atteso {atteso}, ottenuto {reale}) — non e' la stessa patch congelata."
            )
        print(f"OK  {nome}: identica al file registrato in handoff-final-20260909/manifest.json")

# Percorsi che esistono SOLO nella ricostruzione fresca (bookkeeping di
# `patch`/cargo) o SOLO nell'albero committato (licenze aggiunte a parte,
# verificate a se' sotto): esclusi dal digest, altrimenti la provenienza
# risulterebbe rotta per un file che non fa parte del contenuto del pacchetto.
DIGEST_ESCLUSI = {
    "Cargo.toml.orig",
    ".cargo-ok",
    ".cargo-checksum.json",
    # Documentazione aggiunta da questo repository, non parte del pacchetto
    # ne' delle patch: esclusa dal digest come le licenze, per lo stesso
    # motivo — la ricostruzione fresca non la produce.
    "PROVENANCE.md",
    # Il Cargo.lock che alcuni pacchetti pubblicano insieme al proprio
    # sorgente e' inerte qui: una dipendenza `path` dentro un workspace non
    # consulta il proprio lockfile, solo quello del workspace che la ospita
    # (verificato: `cargo metadata --locked` risolve senza di esso). Rimosso
    # dai tre alberi vendorizzati anche per un motivo pratico scoperto in
    # revisione: `wkt` e `i_shape` spediscono un proprio `.gitignore` che
    # esclude `Cargo.lock` — `git add` lo avrebbe scartato in silenzio,
    # lasciando il file presente su disco ma assente da un checkout pulito.
    # Tolto ovunque per coerenza, non solo dove il .gitignore lo forza.
    "Cargo.lock",
}

# I file di licenza NON fanno parte del pacchetto .crate pubblicato (per
# `geo`, verificato: crates.io non li include perche' vivono alla radice del
# workspace upstream, non nella singola crate). Si verificano a parte, contro
# un digest fissato qui — non contro la ricostruzione pacchetto+patch, che
# non li produce.
LICENZE_ATTESE = {
    "geo-0.33.1-exact": {
        # georust/geo, blob al commit 90a0469e2c692b87785b8d0d830852d61bbae142,
        # revisione 2017-08-14, invariati da allora: LICENSE-APACHE, LICENSE-MIT.
        #
        # Presi con `git show <rev>:<path>`, non dal checkout su disco: un
        # checkout Windows con core.autocrlf=true riscrive LF in CRLF, e un
        # digest calcolato su quella copia non e' piu' il digest dell'oggetto
        # Git upstream. `git show` legge il blob, non il file di lavoro, ed e'
        # per questo immune alla normalizzazione di fine riga della macchina
        # che esegue la verifica.
        "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
        "LICENSE-MIT": "2f8267b247fd81e9555a2e196e119a5b341bcae21011cd93985c312f1312f727",
    },
    # wkt e i_shape spediscono la propria licenza nel pacchetto pubblicato:
    # restano dentro il digest pacchetto+patch, nessuna voce qui.
}

# name -> (percorso vendorizzato, [patch in ordine di applicazione])
CRATE = {
    "geo": (
        "geo-0.33.1-exact",
        ["geo-exact-orientation.patch", "logging.patch"],
    ),
    "wkt": (
        "wkt-0.14.0-v2",
        ["wkt-v2.patch"],
    ),
    "i_shape": (
        "i_shape-1.18.0-buffer",
        ["i_shape.patch"],
    ),
}


def trova_o_scarica_crate(nome: str, versione: str, checksum_atteso: str, dest: Path) -> Path:
    """Preferisce la cache locale di cargo; scarica da crates.io solo se
    assente, e verifica SEMPRE il checksum prima di fidarsi del file."""
    cache_home = Path.home() / ".cargo" / "registry" / "cache"
    candidati = list(cache_home.glob(f"*/{nome}-{versione}.crate")) if cache_home.is_dir() else []
    if candidati:
        origine = candidati[0]
        dati = origine.read_bytes()
    else:
        url = f"https://crates.io/api/v1/crates/{nome}/{versione}/download"
        with urllib.request.urlopen(url) as risposta:  # nosec - host fisso, crates.io
            dati = risposta.read()

    reale = hashlib.sha256(dati).hexdigest()
    if reale != checksum_atteso:
        raise SystemExit(
            f"{nome} {versione}: checksum del pacchetto .crate non corrisponde "
            f"a Cargo.lock — atteso {checksum_atteso}, ottenuto {reale}. "
            "Provenienza non verificata, ricostruzione interrotta."
        )

    percorso_crate = dest / f"{nome}-{versione}.crate"
    percorso_crate.write_bytes(dati)
    return percorso_crate


def estrai_crate(percorso_crate: Path, dest: Path) -> Path:
    with tarfile.open(percorso_crate, "r:gz") as tar:
        tar.extractall(dest)  # nosec - contenuto verificato per checksum sopra
    voci = [p for p in dest.iterdir() if p.is_dir()]
    if len(voci) != 1:
        raise SystemExit(f"estrazione inattesa in {dest}: {voci}")
    return voci[0]


def applica_patch(albero: Path, patch_file: Path) -> None:
    esito = subprocess.run(
        ["patch", "-p1", "--forward"],
        cwd=albero,
        stdin=patch_file.open("rb"),
        capture_output=True,
        check=False,
    )
    if esito.returncode != 0:
        raise SystemExit(
            f"applicazione di {patch_file.name} fallita in {albero}:\n"
            f"{esito.stdout.decode(errors='replace')}\n{esito.stderr.decode(errors='replace')}"
        )


def digest_albero(radice: Path, esclusi: set[str]) -> str:
    """sha256 su percorso+contenuto di ogni file, ordinato: deterministico
    indipendentemente dall'ordine di visita del filesystem.

    L'unica esclusione posizionale e' `target/` alla RADICE della crate — il
    residuo di build che `cargo` ci lascerebbe se costruita da sola. Non un
    controllo su qualunque percorso che contiene la parola: quello
    escluderebbe anche un file estraneo infilato sotto una directory
    chiamata `target` in un punto qualsiasi dell'albero, che e' esattamente
    cio' che questa funzione deve invece rilevare come uno scostamento."""
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


def verifica_una_crate(nome: str, versione: str, checksum: str) -> None:
    sotto_vendor, patch_nomi = CRATE[nome]
    vendorizzato = VENDOR / sotto_vendor
    if not vendorizzato.is_dir():
        raise SystemExit(f"{vendorizzato} assente: nulla da verificare")

    with tempfile.TemporaryDirectory(prefix=f"provenienza-{nome}-") as tmp:
        tmp_path = Path(tmp)
        crate_file = trova_o_scarica_crate(nome, versione, checksum, tmp_path)
        albero = estrai_crate(crate_file, tmp_path / "estratto")
        for patch_nome in patch_nomi:
            applica_patch(albero, PATCHES / patch_nome)

        atteso = digest_albero(albero, DIGEST_ESCLUSI)
        reale = digest_albero(vendorizzato, DIGEST_ESCLUSI | set(LICENZE_ATTESE.get(sotto_vendor, {})))
        if atteso != reale:
            raise SystemExit(
                f"{nome}: digest dell'albero non corrisponde.\n"
                f"  ricostruito da pacchetto+patch: {atteso}\n"
                f"  vendor/{sotto_vendor}:            {reale}\n"
                "vendor/ non e' cio' che pacchetto verificato + patch congelate producono."
            )
        print(f"OK  {nome} {versione}: digest albero {atteso[:16]}... coincide con vendor/{sotto_vendor}")

    for nome_licenza, sha_atteso in LICENZE_ATTESE.get(sotto_vendor, {}).items():
        file_licenza = vendorizzato / nome_licenza
        if not file_licenza.is_file():
            raise SystemExit(f"{nome}: licenza mancante — vendor/{sotto_vendor}/{nome_licenza}")
        reale = hashlib.sha256(file_licenza.read_bytes()).hexdigest()
        if reale != sha_atteso:
            raise SystemExit(
                f"{nome}: {nome_licenza} non corrisponde al testo atteso "
                f"(atteso {sha_atteso}, ottenuto {reale})"
            )
        print(f"OK  {nome}: {nome_licenza} verificata contro il testo upstream fissato")


def main() -> int:
    if shutil.which("patch") is None:
        raise SystemExit("eseguibile 'patch' non trovato in PATH")
    verifica_identita_patch()
    for nome, (versione, checksum) in CHECKSUM_PACCHETTO.items():
        verifica_una_crate(nome, versione, checksum)
    print(f"\nProvenienza verificata per {len(CHECKSUM_PACCHETTO)} crate vendorizzate.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
