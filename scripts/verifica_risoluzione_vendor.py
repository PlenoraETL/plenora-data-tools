#!/usr/bin/env python3
"""Prova che geo/wkt/i_shape risolvano davvero al vendor patchato, nel
workspace principale E in `fuzz/` (workspace a se', non eredita `[patch]`).

Corretto rispetto a un tentativo precedente di questa verifica: per una
dipendenza `path`, il campo `source` di `cargo metadata` e' `null`, non una
stringa `"path+file://..."` — quella forma esiste solo dentro gli id dei nodi
di `resolve` (`PackageId`), non nel campo `source` dei pacchetti. Il controllo
quindi non cerca una stringa `source`: pretende `source is None`, la
`version` esatta attesa, il `manifest_path` canonico dentro `vendor/`, che il
`PackageId` non compaia in `workspace_members` (una dipendenza raggiunta solo
da [patch] non deve diventare membro — erediterebbe i lint del workspace
ospite), e che il grafo effettivamente risolto (`resolve.nodes[].deps`)
colleghi un consumatore vero alla crate — non solo che compaia nell'elenco
`packages`, che includerebbe anche una dipendenza presente ma non collegata
da nessuno. Calcola infine un digest di contenuto della directory risolta in
ciascun workspace e pretende che coincidano: non solo lo stesso percorso
stringa, lo stesso contenuto letto due volte.

Uso: `python scripts/verifica_risoluzione_vendor.py` dalla radice del
repository. Esegue `cargo metadata --locked` due volte (root e fuzz/); non
compila nulla, non tocca la rete se i lockfile sono gia' coerenti.

Questa copia dello script vive nell'albero sperimentale del filtro
(`plenora-memlab-filtro-integrato`), separato dal candidato congelato
adottato (`plenora-memlab-integrazione`, la cui copia di questo script resta
invariata e continua a pretendere `geo-0.33.1-exact`). Qui `geo` risolve
DELIBERATAMENTE al vendor filtrato: la voce sotto e' stata cambiata di
conseguenza, non lasciata puntare al congelato con l'aspettativa che la
verifica fallisca — un fallimento qui indicherebbe una vera discrepanza fra
`Cargo.toml` e cio' che questo albero dichiara di essere, non lo stato atteso
dell'esperimento.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FUZZ_MANIFEST = ROOT / "fuzz" / "Cargo.toml"
VENDOR = ROOT / "vendor"

# name -> (sotto-cartella attesa in vendor/, versione attesa)
#
# 'geo' punta al vendor FILTRATO in questo albero sperimentale -- vedi la
# nota nel docstring di modulo. wkt e i_shape restano quelli congelati,
# invariati: solo il diff 1 (orientamento) e' toccato dal filtro.
ATTESO_VENDOR = {
    "geo": ("geo-0.33.1-exact-filtered", "0.33.1"),
    "wkt": ("wkt-0.14.0-v2", "0.14.0"),
    "i_shape": ("i_shape-1.18.0-buffer", "1.18.0"),
}


def digest_directory(radice: Path) -> str:
    """sha256 su percorso+contenuto di ogni file sotto radice, esclusi solo
    i residui di build del crate stesso (`target/` alla radice del crate,
    non un qualunque percorso che contiene la parola: quella sarebbe
    un'esclusione piu' larga di quella che serve, e nasconderebbe un file
    estraneo infilato sotto un percorso che la contiene senza esserlo)."""
    voci = []
    for percorso in sorted(radice.rglob("*")):
        if not percorso.is_file():
            continue
        relativo = percorso.relative_to(radice).as_posix()
        if relativo.split("/", 1)[0] == "target":
            continue
        voci.append((relativo, hashlib.sha256(percorso.read_bytes()).hexdigest()))
    hash_finale = hashlib.sha256()
    for relativo, hash_file in voci:
        hash_finale.update(f"{relativo}\n{hash_file}\n".encode("utf-8"))
    return hash_finale.hexdigest()


def metadata(manifest_path: Path | None) -> dict:
    comando = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if manifest_path is not None:
        comando += ["--manifest-path", str(manifest_path)]
    esito = subprocess.run(comando, cwd=ROOT, capture_output=True, text=True, check=False)
    if esito.returncode != 0:
        etichetta = manifest_path or "workspace principale"
        raise SystemExit(
            f"cargo metadata --locked fallito per {etichetta}:\n{esito.stderr}\n"
            "Il lockfile non e' coerente con i manifesti — rigenerarlo "
            "(senza --locked) prima di pretendere che la risoluzione sia stabile."
        )
    return json.loads(esito.stdout)


def pacchetto_unico(meta: dict, nome: str, etichetta: str) -> dict:
    trovati = [p for p in meta["packages"] if p["name"] == nome]
    if len(trovati) != 1:
        raise SystemExit(
            f"{etichetta}: attesa esattamente 1 voce per '{nome}' in packages, "
            f"trovate {len(trovati)}"
        )
    return trovati[0]


def collegato_da_un_consumatore_vero(meta: dict, package_id: str, nome: str, etichetta: str) -> None:
    """Non basta comparire in `packages`: deve essere una dipendenza di
    almeno un nodo risolto, altrimenti e' presente ma inerte."""
    for nodo in meta["resolve"]["nodes"]:
        for dip in nodo.get("deps", []):
            if dip["pkg"] == package_id:
                return
    raise SystemExit(
        f"{etichetta}: '{nome}' compare fra i pacchetti risolti ma nessun nodo "
        "del grafo lo dichiara come dipendenza — non e' collegato, l'esito "
        "sarebbe un falso positivo."
    )


def non_membro_del_workspace(meta: dict, package_id: str, nome: str, etichetta: str) -> None:
    """Una dipendenza raggiunta solo da [patch.crates-io] non deve diventare
    membro del workspace che la patcha: se lo fosse, erediterebbe
    [workspace.lints] (unsafe_code=forbid, clippy pedantic/nursery=deny) —
    codice di terze parti quasi certamente non li supera, e la build
    romperebbe per un motivo estraneo al patch stesso."""
    if package_id in meta["workspace_members"]:
        raise SystemExit(
            f"{etichetta}: '{nome}' e' un membro del workspace ({package_id}) "
            "— atteso che ci arrivi solo tramite [patch.crates-io], mai come "
            "membro. Erediterebbe i lint del workspace ospite."
        )


def verifica_workspace(manifest_path: Path | None, etichetta: str) -> dict[str, tuple[Path, str]]:
    meta = metadata(manifest_path)
    risultati: dict[str, tuple[Path, str]] = {}
    for nome, (sotto_vendor, versione_attesa) in ATTESO_VENDOR.items():
        pacchetto = pacchetto_unico(meta, nome, etichetta)

        if pacchetto.get("source") is not None:
            raise SystemExit(
                f"{etichetta}: '{nome}' ha source={pacchetto.get('source')!r}, "
                "atteso None (path dependency) — sta risolvendo dal registry, "
                "non dal vendor patchato. Il [patch.crates-io] non sta agendo qui."
            )

        if pacchetto["version"] != versione_attesa:
            raise SystemExit(
                f"{etichetta}: '{nome}' risolve alla versione {pacchetto['version']!r}, "
                f"attesa {versione_attesa!r} — [patch.crates-io] richiede una corrispondenza "
                "di versione esatta con la voce di [workspace.dependencies]; se non corrisponde "
                "piu', cargo smette di applicare la patch, non lo segnala come errore a parte."
            )

        manifest = Path(pacchetto["manifest_path"]).resolve()
        atteso = (VENDOR / sotto_vendor / "Cargo.toml").resolve()
        if manifest != atteso:
            raise SystemExit(
                f"{etichetta}: '{nome}' risolve a {manifest}, atteso {atteso}"
            )

        collegato_da_un_consumatore_vero(meta, pacchetto["id"], nome, etichetta)
        non_membro_del_workspace(meta, pacchetto["id"], nome, etichetta)

        digest = digest_directory(manifest.parent)
        risultati[nome] = (manifest.parent, digest)
        print(
            f"OK  {etichetta}: {nome} {pacchetto['version']} -> {manifest.parent} "
            f"(source=None, non-membro, collegata, digest {digest[:16]}...)"
        )

    return risultati


def main() -> int:
    risultati_root = verifica_workspace(None, "workspace principale")
    risultati_fuzz = verifica_workspace(FUZZ_MANIFEST, "workspace fuzz/")

    for nome in ATTESO_VENDOR:
        percorso_root, digest_root = risultati_root[nome]
        percorso_fuzz, digest_fuzz = risultati_fuzz[nome]
        if percorso_root != percorso_fuzz:
            raise SystemExit(
                f"'{nome}' risolve a directory DIVERSE nei due workspace:\n"
                f"  principale: {percorso_root}\n"
                f"  fuzz/:      {percorso_fuzz}\n"
                "Il fuzz workspace non eredita [patch] dal manifesto principale: "
                "controllare fuzz/Cargo.toml. Un disallineamento qui e' esattamente "
                "la trappola gia' nota — fuzz che ricade sul registry non patchato "
                "da' un ESITO=1 indistinguibile da un esito negativo vero."
            )
        if digest_root != digest_fuzz:
            raise SystemExit(
                f"'{nome}' ha lo STESSO percorso nei due workspace ma digest diverso "
                f"(principale {digest_root}, fuzz/ {digest_fuzz}) — il contenuto e' "
                "cambiato fra le due letture, o qualcosa lo modifica fra una risoluzione "
                "e l'altra."
            )

    print("\nRisoluzione verificata: stesso vendor/, stesso digest di contenuto calcolato "
          "in entrambi i workspace (non solo lo stesso percorso).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
