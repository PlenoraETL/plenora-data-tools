# Assembla docs/kernel-signatures.md dai frammenti (_fragments/*.md) e dai
# metadati del catalogo (crates/plenora-core/src/catalog.rs).
# Uso: python docs/_build/assemble.py   (dalla radice del repo)
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "crates/plenora-core/src/catalog.rs"
FRAG_DIR = ROOT / "docs/_fragments"
OUT = ROOT / "docs/kernel-signatures.md"

ARITY = {"Unary": "unaria", "BinaryOrdered": "binaria (left, right)", "NAry": "n-aria"}
EXEC = {"Streaming": "streaming", "Blocking": "blocking", "BinaryBlocking": "binary-blocking"}
ORIGIN = {"ManipolaCompat": "manipola-compat", "Extension": "estensione"}
CRS = {"Known": "known", "Projected": "projected", "Geographic": "geographic",
       "SameProjected": "same-projected", "Reprojection": "reprojection"}

# I nomi dei parametri POSIZIONALI di `op!`, nell'ordine della macro.
POSIZIONALI = ["id", "family", "origin", "arity", "exec", "cancel",
               "shape", "crs", "caps", "det", "maturity"]

# Le chiavi opzionali che la macro accetta. Ignorarne una sconosciuta
# significherebbe documentare un'operazione con meno di quello che dichiara:
# meglio fallire e farsi aggiornare.
CHIAVI_OPZIONALI = {"semantic_version", "config_schema_version",
                    "contract_analysis_version", "kernel_version",
                    "expansion_constraint", "expansion_factor_exempt",
                    "geo_fusion"}

ANCORA_CATALOGO = "pub static CATALOG: &[OperationDescriptor] = &["

# Operazioni presenti nel catalogo e NON ancora documentate: nessuna sezione,
# nessun frammento. Sono elencate qui, e nel documento generato, invece di
# sparire in un conteggio: il generatore controllava che ogni op documentata
# esistesse nel catalogo, mai il verso opposto, e diciannove operazioni sono
# rimaste fuori senza che nulla lo dicesse.
#
# L'elenco e' un'ASSERZIONE, non una configurazione: se il divario cambia — in
# meglio o in peggio — la generazione fallisce e va aggiornato a mano. Chiuderlo
# significa scrivere diciannove firme, ed e' un lavoro suo, non di questo blocco.
NON_DOCUMENTATE = (
    "geo.cluster_dbscan",
    "geo.collect",
    "geo.coverage_validate",
    "geo.from_wkt",
    "geo.generate_grid",
    "geo.geometry_accessors",
    "geo.line_locate_point",
    "geo.shared_paths",
    "geo.snap",
    "geo.subdivide",
    "table.align_schema",
    "table.concat_by_name",
    "table.fuzzy_join",
    "table.hmac_sha256",
    "table.limit",
    "table.select_columns",
    "table.stable_fingerprint",
    "table.top_n",
    "table.validate_rules",
)


class ErroreCatalogo(Exception):
    """Il catalogo non e' nella forma che questo generatore sa leggere."""


def _chiusura(testo, apertura):
    """Indice della parentesi che chiude quella aperta in `apertura`.

    Conta `()[]{}` annidate e salta il contenuto delle stringhe: la macro
    `op!` contiene sia `Some(...)` sia `&[...]` sia letterali con virgole, e
    un parser che non li distingue taglia gli argomenti nel posto sbagliato.
    """
    profondita = 0
    in_stringa = False
    fuga = False
    for indice in range(apertura, len(testo)):
        carattere = testo[indice]
        if in_stringa:
            if fuga:
                fuga = False
            elif carattere == "\\":
                fuga = True
            elif carattere == '"':
                in_stringa = False
            continue
        if carattere == '"':
            in_stringa = True
        elif carattere in "([{":
            profondita += 1
        elif carattere in ")]}":
            profondita -= 1
            if profondita == 0:
                return indice
    raise ErroreCatalogo(
        "parentesi non chiusa a partire dal carattere %d" % apertura)


def _fetta_catalogo(testo):
    """Il corpo dell'array `CATALOG`, e nient'altro.

    Delimitare la fetta e' cio' che tiene fuori la DEFINIZIONE della macro
    (`op!(@munch ...)`, `op!($id ...)`) e le sue invocazioni nei test: sono
    `op!(` a tutti gli effetti, e un parser che guarda l'intero file le
    conterebbe come operazioni.
    """
    inizio = testo.find(ANCORA_CATALOGO)
    if inizio < 0:
        raise ErroreCatalogo(
            "`%s` non trovato in %s: il catalogo e' stato rinominato o spostato"
            % (ANCORA_CATALOGO, CATALOG))
    apertura = inizio + len(ANCORA_CATALOGO) - 1  # sulla `[` dell'array
    return testo[apertura + 1:_chiusura(testo, apertura)]


def _argomenti(testo):
    """Gli argomenti di primo livello, separati dalle virgole a profondita' 0."""
    argomenti = []
    corrente = []
    profondita = 0
    in_stringa = False
    fuga = False
    for carattere in testo:
        if in_stringa:
            corrente.append(carattere)
            if fuga:
                fuga = False
            elif carattere == "\\":
                fuga = True
            elif carattere == '"':
                in_stringa = False
            continue
        if carattere == '"':
            in_stringa = True
        elif carattere in "([{":
            profondita += 1
        elif carattere in ")]}":
            profondita -= 1
        elif carattere == "," and profondita == 0:
            argomenti.append("".join(corrente).strip())
            corrente = []
            continue
        corrente.append(carattere)
    ultimo = "".join(corrente).strip()
    if ultimo:
        argomenti.append(ultimo)
    return argomenti


def _invocazioni(fetta):
    """Ogni `op!( ... )` della fetta, come testo degli argomenti."""
    posizione = 0
    while True:
        trovato = re.search(r"\bop!\s*\(", fetta[posizione:])
        if trovato is None:
            return
        apertura = posizione + trovato.end() - 1
        fine = _chiusura(fetta, apertura)
        yield fetta[apertura + 1:fine]
        posizione = fine + 1


def _valore_fra(testo, prefisso):
    """`Some(Prefisso::X)` -> `"X"`; `None` -> `None`."""
    testo = testo.strip()
    if testo == "None":
        return None
    atteso = "Some(" + prefisso + "::"
    if not testo.startswith(atteso) or not testo.endswith(")"):
        raise ErroreCatalogo("atteso `None` o `%sX)`, trovato %r"
                             % (atteso, testo))
    return testo[len(atteso):-1].strip()


def _crs(variante):
    """Il requisito CRS in forma leggibile, o `None` se l'op non ne ha.

    `CRS.get(...)` restituiva `None` anche per una variante che la tabella non
    conosce: un requisito CRS nuovo sarebbe sparito dal documento, che avrebbe
    continuato a sembrare completo. Una variante sconosciuta e' un errore.
    """
    if variante is None:
        return None
    if variante not in CRS:
        raise ErroreCatalogo(
            "variante CrsRequirement sconosciuta: %r. Aggiungerla a CRS in "
            "questo file: omettendola, il documento tacerebbe su un requisito "
            "che l'operazione ha davvero." % variante)
    return CRS[variante]


def _capability(testo):
    testo = testo.strip()
    if not testo.startswith("&[") or not testo.endswith("]"):
        raise ErroreCatalogo("atteso `&[...]` per le capability, trovato %r"
                             % testo)
    return [voce.strip().strip('"')
            for voce in _argomenti(testo[2:-1]) if voce.strip()]


def parse_catalog(testo=None):
    """Le operazioni del catalogo, lette dalla macro `op!`.

    Non e' piu' una regex: la macro accetta chiavi opzionali in qualsiasi
    ordine e rustfmt la riformatta su piu' righe appena cresce, e una regex
    che assume una forma smette di agganciare senza dirlo — e' cosi' che
    questo generatore e' rimasto rotto in silenzio.
    """
    if testo is None:
        testo = CATALOG.read_text(encoding="utf-8")
    ops = {}
    for grezzo in _invocazioni(_fetta_catalogo(testo)):
        argomenti = _argomenti(grezzo)
        if len(argomenti) < len(POSIZIONALI):
            raise ErroreCatalogo(
                "invocazione con %d argomenti posizionali invece di %d: %r"
                % (len(argomenti), len(POSIZIONALI), grezzo[:80]))
        posizionali = dict(zip(POSIZIONALI, argomenti))
        oid = posizionali["id"].strip()
        if not (oid.startswith('"') and oid.endswith('"')):
            raise ErroreCatalogo("id non letterale: %r" % oid)
        oid = oid[1:-1]
        if oid in ops:
            raise ErroreCatalogo("operazione duplicata nel catalogo: %s" % oid)

        opzionali = {}
        for extra in argomenti[len(POSIZIONALI):]:
            if "=" not in extra:
                raise ErroreCatalogo(
                    "argomento extra non nella forma `chiave = valore` in %s: %r"
                    % (oid, extra))
            chiave, valore = extra.split("=", 1)
            chiave = chiave.strip()
            if chiave not in CHIAVI_OPZIONALI:
                raise ErroreCatalogo(
                    "chiave opzionale sconosciuta in %s: %r. Se la macro ne ha "
                    "una nuova, va deciso se entra nella documentazione."
                    % (oid, chiave))
            opzionali[chiave] = valore.strip()

        try:
            ops[oid] = {
                "origin": ORIGIN[posizionali["origin"]],
                "arity": ARITY[posizionali["arity"]],
                "exec": EXEC[posizionali["exec"]],
                "shape": _valore_fra(posizionali["shape"], "ResultShape"),
                "crs": _crs(_valore_fra(posizionali["crs"], "CrsRequirement")),
                "caps": _capability(posizionali["caps"]),
                "maturity": posizionali["maturity"],
                "kver": opzionali.get("kernel_version"),
            }
        except KeyError as errore:
            raise ErroreCatalogo("valore non riconosciuto in %s: %s"
                                 % (oid, errore)) from errore

    if not ops:
        raise ErroreCatalogo(
            "zero operazioni lette da %s. Il catalogo non e' vuoto: e' il "
            "parser a non agganciarlo piu'. Non generare nulla." % CATALOG)
    return ops


def parse_fragments(sorgenti=None):
    """I blocchi `### <id>` dei frammenti.

    `sorgenti` e' una sequenza `(nome, testo)`; se manca si leggono i file di
    `FRAG_DIR`. Passarla serve alle regressioni: un parser di documentazione
    va provato su input costruiti, non solo sul repository.

    Un id ripetuto e' un ERRORE. Prima l'ultimo blocco sovrascriveva il
    primo: due firme diverse per la stessa operazione, e nel documento ne
    finiva una sola, senza che nulla dicesse quale fosse stata scartata.
    """
    if sorgenti is None:
        sorgenti = [(path.name, path.read_text(encoding="utf-8"))
                    for path in sorted(FRAG_DIR.glob("*.md"))]
    blocks = {}
    provenienza = {}

    def deposita(nome, cur_id, cur_lines):
        if cur_id in blocks:
            raise ErroreCatalogo(
                "frammento duplicato per `%s`: gia' definito in %s, di nuovo "
                "in %s. Due firme per la stessa operazione: una delle due "
                "sparirebbe in silenzio."
                % (cur_id, provenienza[cur_id], nome))
        blocks[cur_id] = "\n".join(cur_lines).strip()
        provenienza[cur_id] = nome

    for nome, testo in sorgenti:
        cur_id, cur_lines = None, []
        for line in testo.splitlines():
            if line.startswith("### "):
                if cur_id:
                    deposita(nome, cur_id, cur_lines)
                cur_id, cur_lines = line[4:].strip(), []
            else:
                cur_lines.append(line)
        if cur_id:
            deposita(nome, cur_id, cur_lines)
    return blocks


# Sezioni: (titolo, [op in ordine])
TABLE_SECTIONS = [
    ("filtering", ["table.filter", "table.conditional"]),
    ("aggregation", ["table.sort", "table.distinct", "table.dedup_advanced",
                     "table.aggregate", "table.rolling_window", "table.window_function"]),
    ("joins", ["table.join", "table.semi_join", "table.anti_join",
               "table.asof_join", "table.cross_join", "table.concat"]),
    ("cleansing", ["table.fill_na", "table.replace", "table.type_cast"]),
    ("strings", ["table.string_pad", "table.string_length",
                 "table.string_extract", "table.text_normalize"]),
    ("dates", ["table.date_format", "table.date_add",
               "table.date_diff", "table.timezone_convert"]),
    ("columns", ["table.drop_columns", "table.rename", "table.reorder_columns",
                 "table.concat_columns", "table.split_column"]),
    ("analysis", ["table.lookup", "table.bin", "table.flatten_json",
                  "table.statistics", "table.sample"]),
    ("reshape", ["table.melt", "table.pivot", "table.transpose",
                 "table.explode", "table.unnest", "table.table_diff"]),
    ("setops", ["table.except", "table.intersect", "table.union_distinct"]),
    ("security", ["table.md5_hash", "table.sha256_hash", "table.mask_data"]),
    ("quality", ["table.assert_schema", "table.assert_not_null", "table.assert_unique",
                 "table.assert_range", "table.assert_regex", "table.coalesce"]),
    ("governance", ["table.assert_cardinality", "table.assert_metadata",
                    "table.assert_foreign_key", "table.reconcile"]),
    ("utility", ["table.add_row_number", "table.date_extract", "table.uuid_generator"]),
    ("formula", ["table.formula"]),
    ("expressions", ["table.expression"]),
]

GEO_SECTIONS = [
    ("Geo Manipola-compat",
     ["geo.centroid", "geo.convex_hull", "geo.envelope", "geo.sjoin", "geo.area",
      "geo.boundary", "geo.bounds_extractor", "geo.buffer", "geo.clean_topology",
      "geo.clip", "geo.count_points_in_polygons", "geo.difference", "geo.dissolve",
      "geo.distance", "geo.explode", "geo.from_coords", "geo.intersection",
      "geo.length", "geo.line_builder", "geo.nearest", "geo.overlay", "geo.perimeter",
      "geo.point_on_surface", "geo.polygon_builder", "geo.simplify",
      "geo.symmetric_difference", "geo.to_wkt", "geo.union", "geo.vertex_count",
      "geo.voronoi", "geo.within", "geo.make_valid", "geo.reproject"]),
    ("Predicati DE-9IM (estensioni geo)",
     ["geo.predicate_intersects", "geo.predicate_disjoint", "geo.predicate_contains",
      "geo.predicate_within", "geo.predicate_equals_topo", "geo.predicate_covers",
      "geo.predicate_covered_by", "geo.predicate_contains_properly",
      "geo.predicate_touches", "geo.predicate_crosses", "geo.predicate_overlaps"]),
    ("Estensioni geo",
     ["geo.affine_transform", "geo.translate", "geo.scale", "geo.rotate",
      "geo.concave_hull", "geo.hausdorff_distance", "geo.haversine_distance",
      "geo.geodesic_distance", "geo.geodesic_line_length", "geo.densify",
      "geo.snap_to_grid", "geo.delaunay", "geo.polygonize", "geo.line_merge",
      "geo.split", "geo.line_substring", "geo.line_interpolate_point",
      "geo.frechet_distance", "geo.bearing", "geo.geodesic_area",
      "geo.geometry_diagnostics"]),
]


def metadata_line(oid, meta):
    parts = [meta["origin"], f"arietà: {meta['arity']}", f"execution class: {meta['exec']}"]
    if meta["crs"]:
        parts.append(f"CRS: {meta['crs']}")
    if meta["caps"]:
        parts.append("capability: " + ", ".join(meta["caps"]))
    if meta["shape"]:
        parts.append(f"shape: {meta['shape']}")
    if meta["maturity"] == "BackendPending":
        parts.append("maturità: backend-pending")
    if meta["kver"] and meta["kver"] != "1":
        parts.append(f"kernel v{meta['kver']}")
    return "*" + " · ".join(parts) + "*"


INTRO = """# plenora-data-tools — Firme dei kernel (v1)

Documento generato dal codice sorgente del workspace (`crates/plenora-core`,
`crates/plenora-kernels-table`, `crates/plenora-kernels-geo`). Copre **@documentate@
delle @catalogate@ operazioni** del catalogo unificato: @tabellari@ tabellari (`table.*`) e
@geografiche@ geografiche (`geo.*`).

Le @non_documentate@ operazioni catalogate e **non ancora documentate** sono elencate in
fondo. Il conteggio qui sopra e l'elenco sono generati dal catalogo: non
possono divergere da esso senza che la generazione fallisca.

## Come si legge una firma

Un piano v5 è un DAG dichiarativo (`PlanV5`): gli **input di dati** di un nodo
arrivano dagli archi (`in`: riferimenti agli input dichiarati del piano o ad
altri nodi; per le operazioni binarie ordinate l'ordine è semantico —
`[left, right]`), mentre la **configurazione** è nominata nel nodo (`config`,
oggetto JSON; `null` od omessa equivale a `{}`). La config viene deserializzata
in modo tipizzato contro la struct serde dell'operazione, fail-closed: tutte le
struct hanno `deny_unknown_fields`, quindi un campo sconosciuto è un errore. I
vincoli ulteriori (campi non vuoti, range, colonne esistenti, tipi ammessi)
sono verificati in `validate` del kernel e/o nell'analisi di contratto
(`analyze.rs`), che inferisce anche lo schema dell'arco in uscita.

Esempio di nodo piano v5:

```json
{
  "schema_version": 5,
  "crs": "EPSG:32632",
  "inputs": ["sorgente_a"],
  "nodes": [
    {
      "id": "n1",
      "op": "table.filter",
      "in": ["sorgente_a"],
      "config": {"column": "importo", "operator": ">", "value": 0}
    }
  ],
  "output": "n1"
}
```

Per ogni kernel il blocco riporta: id canonico, metadati di catalogo
(`crates/plenora-core/src/catalog.rs`: provenienza, arietà, execution class;
per le geo anche requisito CRS, capability richieste, result shape), la tabella
dei campi config (campo | tipo JSON | default | vincoli), i requisiti sull'
input e l'output prodotto. Convenzioni: `obbligatorio` = campo senza default
serde; `Option<T>` senza default esplicito è riportato con default `null`.
Il modulo `spill.rs` del crate tabellare non corrisponde ad alcuna operazione
del catalogo (spilling interno degli operatori blocking) e non ha una firma.

## Indice

"""


def _catalogo_finto(corpo):
    """Un file sorgente minimo con la definizione della macro e il CATALOG.

    La definizione c'e' apposta: contiene `op!(@munch ...)` e `op!($id ...)`,
    che un parser che guarda l'intero file conterebbe come operazioni.
    """
    return (
        "macro_rules! op {\n"
        "    ($id:literal, $family:ident) => { op!(@munch ($id, $family)) };\n"
        "    (@munch ($($base:tt)*)) => { op!(@build ($($base)*)) };\n"
        "}\n"
        "\n" + ANCORA_CATALOGO + "\n" + corpo + "\n];\n"
        "\n#[cfg(test)]\nmod tests {\n"
        "    fn finta() { op!(\"table.non_deve_comparire\", Table); }\n"
        "}\n"
    )


UNA_RIGA = (
    '    op!("table.uno", Table, ManipolaCompat, Unary, Streaming, Cooperative, '
    'None, None, &[], DefinedOrder, PublicProtocol),\n'
)

# Come rustfmt riformatta l'invocazione appena cresce: un argomento per riga,
# e la parentesi di chiusura su una riga sua. E' questa forma che la regex
# precedente non agganciava piu'.
PIU_RIGHE = """    op!(
        "geo.due",
        Geo,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &["geos-backend", "proj-backend"],
        CanonicalOrder,
        BackendPending,
        kernel_version = 3
    ),
"""

# Parentesi e parentesi quadre annidate, virgole dentro le stringhe e dentro
# `&[...]`: tagliare gli argomenti sulle virgole senza contare la profondita'
# spezzerebbe questa invocazione nel posto sbagliato.
ANNIDATA = (
    '    op!("table.tre", Table, Extension, NAry, Blocking, BoundaryOnly, '
    'Some(ResultShape::OneToMany), None, &["a-b", "c-d"], DefinedOrder, '
    'PublicProtocol, semantic_version = 2, kernel_version = 4),\n'
)


def autotest():
    """Regressioni del parser. Girano a ogni invocazione: costano microsecondi.

    Un parser di documentazione che smette di agganciare non rompe nulla di
    visibile — il documento resta com'era e sembra aggiornato. E' cosi' che
    questo generatore e' rimasto rotto per mesi senza che nessuno lo notasse,
    ed e' la ragione per cui le prove stanno qui dentro invece che in una
    suite che qualcuno deve ricordarsi di lanciare.
    """
    ops = parse_catalog(_catalogo_finto(UNA_RIGA + PIU_RIGHE + ANNIDATA))
    atteso = {"table.uno", "geo.due", "table.tre"}
    if set(ops) != atteso:
        raise ErroreCatalogo("autotest: lette %r invece di %r"
                             % (sorted(ops), sorted(atteso)))

    uno = ops["table.uno"]
    if (uno["arity"], uno["exec"], uno["shape"], uno["crs"], uno["caps"],
            uno["kver"]) != ("unaria", "streaming", None, None, [], None):
        raise ErroreCatalogo("autotest: invocazione su una riga letta male: %r"
                             % uno)

    due = ops["geo.due"]
    if (due["origin"], due["arity"], due["exec"], due["shape"], due["crs"],
            due["caps"], due["maturity"], due["kver"]) != (
            "estensione", "binaria (left, right)", "binary-blocking",
            "OneToOne", "same-projected", ["geos-backend", "proj-backend"],
            "BackendPending", "3"):
        raise ErroreCatalogo("autotest: invocazione multilinea letta male: %r"
                             % due)

    tre = ops["table.tre"]
    if (tre["shape"], tre["caps"], tre["kver"]) != (
            "OneToMany", ["a-b", "c-d"], "4"):
        raise ErroreCatalogo("autotest: invocazione annidata letta male: %r"
                             % tre)

    # Zero operazioni non e' un catalogo vuoto: e' un parser che non aggancia.
    # Deve fallire forte, non generare un documento senza metadati.
    try:
        parse_catalog(_catalogo_finto("    // nessuna operazione\n"))
    except ErroreCatalogo as errore:
        if "zero operazioni" not in str(errore):
            raise ErroreCatalogo(
                "autotest: catalogo vuoto rifiutato per la ragione sbagliata: %s"
                % errore) from errore
    else:
        raise ErroreCatalogo(
            "autotest: un catalogo senza operazioni non ha fatto fallire il "
            "parser. E' esattamente il modo in cui questo generatore si e' "
            "rotto in silenzio.")

    # Una chiave opzionale sconosciuta va vista: documentare un'operazione con
    # meno di quello che dichiara e' peggio che fermarsi.
    ignota = (
        '    op!("table.quattro", Table, Extension, Unary, Streaming, '
        'Cooperative, None, None, &[], DefinedOrder, PublicProtocol, '
        'chiave_ignota = 1),\n'
    )
    try:
        parse_catalog(_catalogo_finto(ignota))
    except ErroreCatalogo as errore:
        if "chiave_ignota" not in str(errore):
            raise ErroreCatalogo(
                "autotest: chiave sconosciuta rifiutata per la ragione "
                "sbagliata: %s" % errore) from errore
    else:
        raise ErroreCatalogo(
            "autotest: una chiave opzionale sconosciuta e' passata inosservata.")

    # Una variante CRS che la tabella non conosce spariva dal documento: il
    # requisito c'era nel catalogo e la firma non lo diceva.
    crs_ignoto = (
        '    op!("geo.cinque", Geo, Extension, Unary, Streaming, Cooperative, '
        'None, Some(CrsRequirement::VarianteNuova), &[], DefinedOrder, '
        'PublicProtocol),\n'
    )
    try:
        parse_catalog(_catalogo_finto(crs_ignoto))
    except ErroreCatalogo as errore:
        if "VarianteNuova" not in str(errore):
            raise ErroreCatalogo(
                "autotest: variante CRS sconosciuta rifiutata per la ragione "
                "sbagliata: %s" % errore) from errore
    else:
        raise ErroreCatalogo(
            "autotest: una variante CrsRequirement sconosciuta e' stata "
            "omessa invece di fermare la generazione.")

    # Due frammenti con lo stesso id: prima l'ultimo sovrascriveva il primo.
    doppio = [
        ("primo.md", "### table.uno\nfirma A\n"),
        ("secondo.md", "### table.uno\nfirma B\n"),
    ]
    try:
        parse_fragments(doppio)
    except ErroreCatalogo as errore:
        if "primo.md" not in str(errore) or "secondo.md" not in str(errore):
            raise ErroreCatalogo(
                "autotest: frammento duplicato rifiutato senza dire dove: %s"
                % errore) from errore
    else:
        raise ErroreCatalogo(
            "autotest: due frammenti con lo stesso id non hanno fatto fallire "
            "la lettura: uno dei due sparirebbe in silenzio.")

    # E un frammento per id resta il caso normale.
    singoli = parse_fragments([("uno.md", "### table.uno\nfirma A\n"),
                               ("due.md", "### table.due\nfirma B\n")])
    if sorted(singoli) != ["table.due", "table.uno"]:
        raise ErroreCatalogo("autotest: frammenti distinti letti male: %r"
                             % sorted(singoli))

    # Le sezioni non devono elencare due volte la stessa operazione.
    ripetute = [oid for _, ops in TABLE_SECTIONS + GEO_SECTIONS for oid in ops]
    if len(ripetute) != len(set(ripetute)):
        raise ErroreCatalogo(
            "autotest: le sezioni di questo file elencano gia' un'operazione "
            "piu' di una volta: %r"
            % sorted({o for o in ripetute if ripetute.count(o) > 1}))


def genera():
    """Il testo completo di `kernel-signatures.md`, senza scriverlo."""
    catalog = parse_catalog()
    blocks = parse_fragments()
    documentate = [oid for _, ops in TABLE_SECTIONS + GEO_SECTIONS for oid in ops]
    # Un'operazione elencata due volte comparirebbe due volte nel documento e
    # gonfierebbe i conteggi dell'intestazione, che sono calcolati proprio da
    # questa lista. `set()` altrove la assorbirebbe senza dire niente.
    ripetute = sorted({oid for oid in documentate if documentate.count(oid) > 1})
    if ripetute:
        raise ErroreCatalogo(
            "operazioni elencate piu' di una volta nelle sezioni: %r. "
            "Comparirebbero due volte e falserebbero i conteggi." % ripetute)
    missing_cat = [oid for oid in documentate if oid not in catalog]
    missing_blk = [oid for oid in documentate if oid not in blocks]
    extra_blk = sorted(set(blocks) - set(documentate))
    # Il verso che mancava: operazioni catalogate che nessuna sezione copre.
    scoperte = sorted(set(catalog) - set(documentate))
    divario_atteso = sorted(NON_DOCUMENTATE)
    if missing_cat or missing_blk or extra_blk:
        raise ErroreCatalogo(
            "MISMATCH fra sezioni, catalogo e frammenti: "
            "documentate assenti dal catalogo %r, senza frammento %r, "
            "frammenti non sezionati %r" % (missing_cat, missing_blk, extra_blk))
    if scoperte != divario_atteso:
        nuove = sorted(set(scoperte) - set(divario_atteso))
        chiuse = sorted(set(divario_atteso) - set(scoperte))
        raise ErroreCatalogo(
            "il divario fra catalogo e documento e' cambiato.\n"
            "  operazioni nuove non documentate: %r\n"
            "  operazioni non piu' scoperte: %r\n"
            "Aggiornare NON_DOCUMENTATE in questo file: l'elenco e' "
            "un'asserzione, non una configurazione." % (nuove, chiuse))

    tabellari = sum(len(ops) for _, ops in TABLE_SECTIONS)
    geografiche = sum(len(ops) for _, ops in GEO_SECTIONS)
    # Sostituzione a segnaposti `@nome@` e non `str.format`: l'INTRO contiene
    # un esempio JSON, e le graffe di quello non sono campi da formattare.
    intro = INTRO
    for chiave, valore in (("documentate", len(documentate)),
                           ("catalogate", len(catalog)),
                           ("tabellari", tabellari),
                           ("geografiche", geografiche),
                           ("non_documentate", len(scoperte))):
        intro = intro.replace("@%s@" % chiave, str(valore))
    residui = re.findall(r"@[a-z_]+@", intro)
    if residui:
        raise ErroreCatalogo("segnaposto non sostituiti nell'INTRO: %r" % residui)
    out = [intro]
    out.append(f"### Operazioni tabellari ({tabellari})\n")
    for name, ops in TABLE_SECTIONS:
        out.append(f"- **{name}** ({len(ops)}): " + ", ".join(f"`{o}`" for o in ops))
    out.append(f"\n### Operazioni geografiche ({geografiche})\n")
    for name, ops in GEO_SECTIONS:
        out.append(f"- **{name}** ({len(ops)}): " + ", ".join(f"`{o}`" for o in ops))

    def emit(oid):
        out.append(f"\n### {oid}\n")
        out.append(metadata_line(oid, catalog[oid]) + "\n")
        out.append(blocks[oid])

    for name, ops in TABLE_SECTIONS:
        out.append(f"\n## Tabellari — {name}\n")
        for oid in ops:
            emit(oid)
    for name, ops in GEO_SECTIONS:
        out.append(f"\n## Geo — {name}\n")
        for oid in ops:
            emit(oid)

    out.append("\n## Operazioni catalogate non ancora documentate\n")
    out.append(
        f"Queste {len(scoperte)} operazioni esistono nel catalogo e **non hanno "
        "ancora una firma in questo documento**. Sono elencate perche' un "
        "documento che tace su cio' che non copre sembra completo:\n")
    for oid in scoperte:
        out.append(f"- `{oid}`")

    return "\n".join(out) + "\n"


def main(argomenti):
    verifica = "--verify" in argomenti
    sconosciuti = [a for a in argomenti if a != "--verify"]
    if sconosciuti:
        print("argomenti sconosciuti: %r (atteso: --verify)" % sconosciuti)
        return 2

    # Le regressioni e la generazione condividono il gestore: un autotest che
    # fallisse con un traceback direbbe la stessa cosa in modo illeggibile, e
    # nel log di CI un traceback si scambia per un guasto dello strumento
    # invece che per il difetto che ha appena trovato.
    try:
        autotest()
        testo = genera()
    except ErroreCatalogo as errore:
        print("ERRORE: %s" % errore)
        return 1

    # Fine riga LF esplicita, come impone `.gitattributes` (`eol=lf`): il
    # documento e' lo stesso su Windows e su Linux, o il confronto in CI
    # fallirebbe per la piattaforma invece che per il contenuto.
    atteso = testo.encode("utf-8")

    if verifica:
        # Non tocca il worktree: confronta e basta. Un gate che riscrive il
        # file che sta verificando non verifica nulla.
        if not OUT.exists():
            print("ERRORE: %s non esiste" % OUT)
            return 1
        attuale = OUT.read_bytes()
        if attuale != atteso:
            print("DIVERGENZA: %s non coincide con la rigenerazione." % OUT)
            print("  su disco %d byte, rigenerato %d byte"
                  % (len(attuale), len(atteso)))
            print(_primo_scostamento(
                attuale.decode("utf-8", "replace"), testo))
            return 1
        print("OK: %s coincide byte per byte con la rigenerazione "
              "(%d byte, %d righe)" % (OUT, len(atteso), len(testo.splitlines())))
        return 0

    OUT.write_bytes(atteso)
    print("OK: %s (%d byte, %d righe)"
          % (OUT, len(atteso), len(testo.splitlines())))
    return 0


def _primo_scostamento(attuale, atteso):
    righe_a = attuale.splitlines()
    righe_b = atteso.splitlines()
    for numero, (a, b) in enumerate(zip(righe_a, righe_b), 1):
        if a != b:
            return ("  prima differenza alla riga %d:\n"
                    "    su disco:      %r\n"
                    "    rigenerato:    %r" % (numero, a, b))
    return ("  i primi %d righe coincidono; il file su disco ha %d righe, "
            "la rigenerazione %d" % (min(len(righe_a), len(righe_b)),
                                     len(righe_a), len(righe_b)))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
