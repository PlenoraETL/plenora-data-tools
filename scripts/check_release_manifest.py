#!/usr/bin/env python3
"""Verifica dei manifesti di rilascio STORICI di plenora-data-tools (rc–1.0.3).

# Che cosa e' stato recuperato, e che cosa no

Fino al 2026-08-18 questa verifica era eseguita da uno script di
``PlenoraETL/plenora-contracts``, clonato in CI al tag ``v2.0-rc10``. Quel
repository e' stato **sostituito** il 2026-08-18: storia, branch e tag non sono
stati riportati (vedi il suo ``CUTOVER.md``).

Dai log dell'ultima esecuzione riuscita (run 31166125840, 2026-08-07, su
``4b9edda``) sono stati recuperati **due fatti soltanto**:

1. la **provenienza** del pin — il tag annotato ``ba77b16…`` risolveva al
   commit ``3598259bbe07d1c853453ff34ca2c1d1d28a0272``, lo stesso valore che
   tutti e cinque i manifesti dichiarano in ``icd.revision``;
2. il **contratto osservabile** della verifica — verdetto, testo dell'avviso
   ``C2.2``, elenco dei criteri non automatizzati.

**Non e' stato recuperato il repository storico, e il contenuto dello script
originale non e' stato ricostruito ne' riverificato.** Questo file non e' una
riproduzione di quello script e non ne dichiara l'equivalenza: e' una verifica
scritta qui, sulle primitive di ``check_release_candidate.py``, il cui
perimetro e' esattamente quello elencato in ``CRITERI_AUTOMATIZZATI``.

# Perche' e' interamente locale e immutabile

I cinque manifesti ``rc``, ``1.0.0``, ``1.0.1``, ``1.0.2``, ``1.0.3`` sono
documenti **immutabili**: descrivono rilasci gia' avvenuti. La loro verifica
non deve dipendere da un repository esterno — la sostituzione del 2026-08-18 ha
mostrato che cosa succede quando ci dipende — ne' dalle regole di una linea
normativa successiva.

Il ``plenora-contracts`` attuale e' quindi trattato come una **nuova linea
normativa**: non viene clonato, e ne' i suoi tag ne' i suoi contenuti entrano
in questa verifica. I manifesti storici non vanno reinterpretati secondo regole
nate dopo di loro.

# Manifesti futuri

Richiedono un **profilo separato**, costruito sulla struttura del nuovo
``plenora-contracts``. Finche' non esiste, ``--profilo nuovo`` fallisce con un
errore esplicito invece di applicare per inerzia le regole storiche: vedi
``PROFILI``.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path, PurePosixPath
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from check_release_candidate import _git, _load_json  # noqa: E402

COMPONENT = "plenora-data-tools"
ICD_REPOSITORY = "plenora-contracts"
CRITERI = "PLENORA-CRITERI-RC.md"

#: Il pin ICD dei manifesti storici, ESATTO e non una forma.
#:
#: Non basta «un tag non vuoto e una revisione di 40 cifre esadecimali»: questi
#: documenti citano tutti la stessa revisione, quella recuperata dai log. Un
#: tag o una revisione formalmente validi ma diversi sarebbero una riscrittura
#: della provenienza, ed e' esattamente cio' che qui non deve poter passare.
ICD_TAG = "v2.0-rc10"
ICD_REVISION = "3598259bbe07d1c853453ff34ca2c1d1d28a0272"

#: La catena esatta dei cinque manifesti emessi: `nome file -> il dizionario
#: `supersedes` STORICAMENTE DICHIARATO`, `None` per il capostipite.
#:
#: E' una **mappa locale ed esplicita**, non una deduzione: i manifesti storici
#: non cambiano, e la catena e' un fatto del rilascio. I valori registrati sono
#: quelli dichiarati dai manifesti stessi e confermati dai tag annotati di
#: QUESTO repository al momento della scrittura (2026-08-18).
#:
#: Il confronto e' sulla **forma esatta**, non sui soli valori presenti:
#: l'insieme delle chiavi e' parte del documento. `1.0.3` dichiara anche `tag`
#: e `tag_object`, i tre precedenti no — e per una catena dichiarata immutabile
#: togliere quei campi da `1.0.3`, aggiungerli dove non c'erano o introdurre
#: chiavi sconosciute e' una riscrittura, non una variante ammessa.
#:
#: Fail-closed su tre lati: un manifesto non elencato qui e' un errore; un
#: manifesto elencato che dichiari valori diversi e' un errore; un manifesto
#: elencato che dichiari un insieme di chiavi diverso e' un errore.
CATENA: dict[str, dict[str, str] | None] = {
    "rc.json": None,
    # I tre manifesti di mezzo NON dichiarano `tag`/`tag_object`: e' la forma
    # con cui sono stati emessi e resta tale.
    "1.0.0.json": {
        "manifest": "release/rc.json",
        "component_version": "0.1.0-rc1",
        "revision": "7d530318760ccfa93b2baa2049e181fd57deed1e",
    },
    "1.0.1.json": {
        "manifest": "release/1.0.0.json",
        "component_version": "1.0.0",
        "revision": "7a47504482569636b6c0e268477d155010d3b030",
    },
    "1.0.2.json": {
        "manifest": "release/1.0.1.json",
        "component_version": "1.0.1",
        "revision": "aab8152902a209955b2ea657dfeaeea10408f866",
    },
    "1.0.3.json": {
        "manifest": "release/1.0.2.json",
        "component_version": "1.0.2",
        "tag": "v1.0.2",
        "tag_object": "94177cb78be94808909a524f5c2f541e8e829d46",
        "revision": "f4b669867b195dd20cf7cf4a76980d26a61b9629",
    },
}

#: Tag annotato del predecessore, per manifesto: serve a riverificare contro
#: git anche quando il manifesto storico non lo dichiara.
#:
#: Vive fuori da [`CATENA`] proprio perche' quella registra la FORMA del
#: documento e non deve contenere campi che il documento non ha.
TAG_PREDECESSORE: dict[str, tuple[str, str]] = {
    "1.0.0.json": ("v0.1.0-rc1", "2ad20a1d2906d4eae13454d57ae5b8e1a885262a"),
    "1.0.1.json": ("v1.0.0", "b4a5bcd827c1c815460029681e3ce25bff915c2c"),
    "1.0.2.json": ("v1.0.1", "c47a82f5b63a2c5cdf264057e5dbcc1a69a58d62"),
    "1.0.3.json": ("v1.0.2", "94177cb78be94808909a524f5c2f541e8e829d46"),
}

#: Criteri che questa verifica NON copre, elencati verbatim come nell'ultima
#: esecuzione riuscita: restano responsabilita' della revisione umana e sono
#: stampati a ogni esecuzione perche' non si dimentichino.
NON_AUTOMATIZZATI = ("C2.1", "C2.3", "C3.2", "C4.3", "C5.1", "C5.2")

#: Il perimetro effettivo di questa verifica, dichiarato per esteso: e' cio'
#: che il verdetto `pass` significa, e nulla di piu'.
CRITERI_AUTOMATIZZATI = {
    "C1.1": "manifest_version, component, component_version",
    "C1.2": "il nome del file e' la versione (eccezione storica rc.json)",
    "C2.2": "pin ICD esatto: repository, tag e revisione registrati",
    "C3.1": "revision e' un commit di questo repository e antenato di HEAD",
    "C4.1": "claims: system_rc e avionic_certification false, component_rc booleano",
    "C4.2": "coerenza fra verification_claim e independent_review",
    "C5.3": "catena supersedes: predecessore, versione, tag e revisione registrati",
}

#: Profili di verifica. Solo quello storico esiste: i manifesti futuri
#: nasceranno sotto la nuova linea normativa e vanno verificati con regole
#: costruite su quella, non con queste.
PROFILI = ("storico",)

#: Il capostipite precede la convenzione «nome file = versione».
NOME_STORICO = {"rc": re.compile(r"^0\.1\.0-rc\d+$")}

SHA40 = re.compile(r"^[0-9a-f]{40}$")


def _versione_da_nome(path: Path) -> str | None:
    """Versione attesa dal nome del file, o `None` se il nome e' storico."""
    return None if path.stem in NOME_STORICO else path.stem


def _percorso_sicuro(relativo: Any) -> str | None:
    """Motivo per cui `relativo` NON e' un percorso ammesso, o `None`.

    Un manifesto e' un dato di configurazione: il percorso che dichiara non
    deve poter uscire da `release/`. Percorsi assoluti, risalite `..` e radici
    diverse sono rifiutati PRIMA di toccare il filesystem, non dopo.
    """
    if not isinstance(relativo, str) or not relativo:
        return "deve essere un percorso non vuoto"
    if relativo.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:", relativo):
        return f"percorso assoluto non ammesso: {relativo}"
    puro = PurePosixPath(relativo.replace("\\", "/"))
    if puro.is_absolute():
        return f"percorso assoluto non ammesso: {relativo}"
    if ".." in puro.parts:
        return f"risalita di directory non ammessa: {relativo}"
    if len(puro.parts) != 2 or puro.parts[0] != "release":
        return f"deve essere release/<manifesto>.json, trovato {relativo}"
    return None


def _verifica_catena(repo: Path, nome: str, supersedes: Any) -> list[str]:
    """Controlli C5.3 contro la mappa locale della catena.

    Il confronto e' sulla **forma esatta** del dizionario `supersedes`: chiavi,
    presenza e valori. Verificare i soli campi presenti lasciava passare due
    riscritture opposte — togliere `tag`/`tag_object` da `1.0.3` e aggiungerli
    ai manifesti che non li avevano — su una catena dichiarata immutabile.

    Il registro dice CHE COSA il manifesto deve dichiarare; il repository dice
    se quella dichiarazione e' ancora vera (tag annotato presente, che si
    riduce al commit dichiarato).
    """
    errori: list[str] = []
    atteso = CATENA[nome]

    if atteso is None:
        if supersedes is not None:
            errori.append(f"C5.3: {nome} e' il capostipite e non deve dichiarare supersedes")
        return errori

    if supersedes is None:
        errori.append(
            f"C5.3: {nome} deve dichiarare supersedes {atteso['manifest']} "
            f"(versione {atteso['component_version']}): solo rc.json puo' non avere predecessore"
        )
        return errori
    if not isinstance(supersedes, dict):
        errori.append(f"C5.3: {nome}:supersedes deve essere un oggetto")
        return errori

    # --- forma esatta: prima le chiavi, poi i valori ---------------------
    mancanti = sorted(set(atteso) - set(supersedes))
    se_in_piu = sorted(set(supersedes) - set(atteso))
    if mancanti:
        errori.append(
            f"C5.3: {nome}:supersedes ha perso le chiavi storiche {mancanti}: "
            "la forma dichiarata e' immutabile"
        )
    if se_in_piu:
        errori.append(
            f"C5.3: {nome}:supersedes dichiara chiavi non storiche {se_in_piu}: "
            "la forma dichiarata e' immutabile"
        )

    # Il percorso e' un dato di configurazione: si valida comunque, anche
    # quando coincide con quello registrato.
    if "manifest" in supersedes:
        motivo = _percorso_sicuro(supersedes.get("manifest"))
        if motivo is not None:
            errori.append(f"C5.3: {nome}:supersedes.manifest {motivo}")

    for chiave in sorted(set(atteso) & set(supersedes)):
        if supersedes[chiave] != atteso[chiave]:
            errori.append(
                f"C5.3: {nome}:supersedes.{chiave} deve essere {atteso[chiave]!r}, "
                f"trovato {supersedes[chiave]!r}"
            )

    if supersedes.get("manifest") == atteso["manifest"] and not (
        repo / atteso["manifest"]
    ).is_file():
        errori.append(f"C5.3: {nome}:supersedes.manifest {atteso['manifest']} non esiste")

    # --- il registro contro git ------------------------------------------
    tag, oggetto_atteso = TAG_PREDECESSORE[nome]
    riferimento = _git(repo, "rev-parse", f"refs/tags/{tag}", check=False)
    oggetto = riferimento.stdout.strip() if riferimento.returncode == 0 else ""
    if oggetto != oggetto_atteso:
        errori.append(
            f"C5.3: {nome}: il tag {tag} di questo repository non e' l'oggetto "
            f"registrato {oggetto_atteso[:12]}"
        )
        return errori
    # Superato il confronto qui sopra, l'oggetto del tag e' ESATTAMENTE quello
    # registrato, e in git l'identita' di un oggetto ne determina il contenuto:
    # tipo (`tag` annotato) e commit di destinazione non sono piu' variabili.
    # Il controllo che segue non e' quindi raggiungibile modificando un
    # manifesto — copre solo un object store incoerente, in cui il riferimento
    # esiste e l'oggetto no. Resta perche' e' una difesa reale, ma NESSUN test
    # di mutazione puo' coprirlo, e va detto invece di contarlo fra i coperti.
    peeled = _git(repo, "rev-parse", f"{tag}^{{commit}}", check=False)
    if peeled.returncode or peeled.stdout.strip() != atteso["revision"]:
        errori.append(
            f"C5.3: {nome}: il tag {tag} non si riduce al commit registrato "
            f"{atteso['revision'][:12]} (object store incoerente)"
        )
    return errori


def validate_manifest(
    repo: Path, manifest_path: Path, profilo: str = "storico"
) -> tuple[list[str], list[str]]:
    """Ritorna `(errori, avvisi)` per un manifesto storico.

    Fail-closed: qualunque campo assente o malformato e' un errore, mai un
    valore assunto per difetto.
    """
    repo = repo.resolve()
    manifest_path = manifest_path.resolve()
    errori: list[str] = []
    avvisi: list[str] = []
    nome = manifest_path.name

    # Il documento verificato deve stare nella directory canonica, e la
    # verifica avviene DOPO la risoluzione del percorso: percorsi assoluti,
    # risalite e symlink che puntano fuori sono gia' stati risolti qui, quindi
    # un confronto sul parent risolto li coglie tutti. Senza, una copia esterna
    # chiamata `1.0.3.json` veniva validata sul solo basename — e il basename
    # e' cio' che decide catena e criteri.
    canonica = (repo / "release").resolve()
    if manifest_path.parent != canonica:
        return (
            [
                f"C1.2: {nome}: il manifesto deve trovarsi in {canonica}, "
                f"trovato {manifest_path.parent}"
            ],
            avvisi,
        )

    if profilo != "storico":
        return (
            [
                f"profilo {profilo!r} non definito: i manifesti nati sotto la nuova linea "
                "normativa richiedono un profilo costruito su quella struttura; i manifesti "
                "storici non vanno reinterpretati con regole successive"
            ],
            avvisi,
        )

    try:
        manifesto = _load_json(manifest_path)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        # `_load_json` rifiuta anche un JSON valido la cui radice non sia un
        # oggetto: senza, ogni `.get()` qui sotto sarebbe un traceback.
        return [f"C1.1: {nome}: {exc}"], avvisi

    # --- C1.1 identita' del documento -------------------------------------
    if manifesto.get("manifest_version") != 1:
        errori.append(f"C1.1: {nome}:manifest_version deve essere 1")
    if manifesto.get("component") != COMPONENT:
        errori.append(f"C1.1: {nome}:component deve essere {COMPONENT}")

    versione = manifesto.get("component_version")
    if not isinstance(versione, str) or not versione:
        errori.append(f"C1.1: {nome}:component_version deve essere una stringa non vuota")
        versione = None

    # --- C1.2 il nome del file e' la versione -----------------------------
    attesa = _versione_da_nome(manifest_path)
    if versione is not None:
        if attesa is None:
            forma = NOME_STORICO[manifest_path.stem]
            if not forma.match(versione):
                errori.append(
                    f"C1.2: {nome}: nome storico ammesso solo per {forma.pattern}, "
                    f"trovato {versione}"
                )
        elif versione != attesa:
            errori.append(
                f"C1.2: {nome}:component_version {versione} non corrisponde al nome del file"
            )

    # --- C2.2 pin ICD, esatto ---------------------------------------------
    icd = manifesto.get("icd")
    if not isinstance(icd, dict):
        errori.append(f"C2.2: {nome}:icd assente")
    else:
        if icd.get("repository") != ICD_REPOSITORY:
            errori.append(f"C2.2: {nome}:icd.repository deve essere {ICD_REPOSITORY}")
        if icd.get("tag") != ICD_TAG:
            errori.append(
                f"C2.2: {nome}:icd.tag deve essere {ICD_TAG}, trovato {icd.get('tag')!r}"
            )
        revisione_icd = icd.get("revision")
        if not isinstance(revisione_icd, str) or not SHA40.match(revisione_icd):
            errori.append(
                f"C2.2: {nome}:icd.revision deve essere un commit di 40 cifre esadecimali"
            )
        elif revisione_icd != ICD_REVISION:
            errori.append(
                f"C2.2: {nome}:icd.revision deve essere {ICD_REVISION}, trovato {revisione_icd}"
            )
        else:
            # L'ICD vive in un altro repository, e quel repository e' stato
            # sostituito: l'esistenza della revisione non e' verificabile da
            # qui e non lo era nemmeno prima. Resta un avviso, non un errore.
            avvisi.append(
                f"C2.2: {nome}:icd.revision appartiene a un altro repository, "
                "esistenza non verificabile da qui"
            )

    # --- C3.1 la revisione e' un commit di QUESTO repository --------------
    revisione = manifesto.get("revision")
    if not isinstance(revisione, str) or not SHA40.match(revisione):
        errori.append(f"C3.1: {nome}:revision deve essere un commit di 40 cifre esadecimali")
    else:
        tipo = _git(repo, "cat-file", "-t", revisione, check=False)
        if tipo.returncode or tipo.stdout.strip() != "commit":
            errori.append(
                f"C3.1: {nome}:revision {revisione[:12]} non e' un commit di questo repository"
            )
        elif _git(repo, "merge-base", "--is-ancestor", revisione, "HEAD", check=False).returncode:
            errori.append(f"C3.1: {nome}:revision {revisione[:12]} non e' un antenato di HEAD")

    # --- C4.1 rivendicazioni ----------------------------------------------
    claims = manifesto.get("claims")
    if not isinstance(claims, dict):
        errori.append(f"C4.1: {nome}:claims assente")
    else:
        if not isinstance(claims.get("component_rc"), bool):
            errori.append(f"C4.1: {nome}:claims.component_rc deve essere booleano")
        if claims.get("system_rc") is not False:
            errori.append(f"C4.1: {nome}:claims.system_rc deve essere false")
        if claims.get("avionic_certification") is not False:
            errori.append(f"C4.1: {nome}:claims.avionic_certification deve essere false")

    # --- C4.2 coerenza fra dichiarazione di verifica e revisione ----------
    dichiarazione = manifesto.get("verification_claim")
    indipendente = manifesto.get("independent_review")
    ammesse = {"verified_internally": False, "verified_independently": True}
    if dichiarazione not in ammesse:
        errori.append(f"C4.2: {nome}:verification_claim deve essere uno di {sorted(ammesse)}")
    elif indipendente is not ammesse[dichiarazione]:
        errori.append(
            f"C4.2: {nome}: {dichiarazione} richiede independent_review "
            f"{str(ammesse[dichiarazione]).lower()}, trovato {indipendente!r}"
        )

    # --- C5.3 catena dei manifesti superati -------------------------------
    if nome not in CATENA:
        errori.append(
            f"C5.3: {nome} non e' nella catena storica dichiarata "
            f"({', '.join(sorted(CATENA))}): un manifesto nuovo richiede il profilo "
            "della linea normativa sotto cui nasce"
        )
    else:
        errori.extend(_verifica_catena(repo, nome, manifesto.get("supersedes")))

    return errori, avvisi


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Verifica un manifesto di rilascio STORICO di plenora-data-tools (rc-1.0.3). "
            "Interamente locale: non clona ne' consulta plenora-contracts."
        )
    )
    parser.add_argument("manifest")
    parser.add_argument("--repo", default=".")
    parser.add_argument(
        "--profilo",
        default="storico",
        help=(
            "profilo di verifica; solo 'storico' e' definito. I manifesti nati sotto la "
            "nuova linea normativa richiedono un profilo costruito su quella struttura."
        ),
    )
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    percorso = (repo / args.manifest).resolve()
    nome = percorso.name

    errori, avvisi = validate_manifest(repo, percorso, args.profilo)
    for avviso in avvisi:
        print(f"avviso   {avviso}")
    for errore in errori:
        print(f"errore   {errore}", file=sys.stderr)
    esito = "fail" if errori else "pass"
    print(f"\n{nome}: {esito} ({len(errori)} errori, {len(avvisi)} avvisi) — {CRITERI}")
    print(f"automatizzati: {', '.join(sorted(CRITERI_AUTOMATIZZATI))}")
    print(f"non automatizzati: {', '.join(NON_AUTOMATIZZATI)}")
    return 1 if errori else 0


if __name__ == "__main__":
    raise SystemExit(main())
