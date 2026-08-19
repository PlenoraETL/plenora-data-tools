"""Test di mutazione di `check_release_manifest.py`.

Ogni controllo automatizzato del checker ha qui una mutazione che lo fa
scattare: un controllo che non fallisce quando il manifesto e' sbagliato non
sta verificando niente, ed e' la ragione per cui questi test esistono invece
di un solo caso felice.

In coda, la verifica che i cinque manifesti storici REALI passino, e che la
mappa della catena copra esattamente i file presenti.

Dei controlli originali di `plenora-contracts` non e' stato recuperato nulla:
dai log della CI sono stati ricostruiti la provenienza del pin e il contratto
osservabile del verdetto, non il repository ne' il contenuto dello script.
Quanto e' verificato qui e' quindi cio' che il checker locale verifica, e
niente di piu'.
"""

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.check_release_manifest import (
    CATENA,
    TAG_PREDECESSORE,
    ICD_REVISION,
    ICD_TAG,
    NON_AUTOMATIZZATI,
    validate_manifest,
)

REPO = Path(__file__).resolve().parents[1]


class ManifestCheckerMutationTests(unittest.TestCase):
    """Un repository sintetico: le mutazioni non toccano quello vero."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        (self.repo / "release").mkdir(parents=True)
        (self.repo / "file.txt").write_text("base\n", encoding="utf-8")
        self._git("init", "-q")
        self._git("config", "user.email", "test@example.invalid")
        self._git("config", "user.name", "Test")
        self._git("add", ".")
        self._git("commit", "-qm", "base")
        self.base = self._git("rev-parse", "HEAD").stdout.strip()
        # Un commit ESTRANEO alla storia di HEAD, per il controllo di antenato.
        self._git("checkout", "-q", "-b", "laterale")
        (self.repo / "file.txt").write_text("laterale\n", encoding="utf-8")
        self._git("commit", "-aqm", "laterale")
        self.estraneo = self._git("rev-parse", "HEAD").stdout.strip()
        self._git("checkout", "-q", "-")

        # La catena verificata dal checker e' quella REALE: il repository
        # sintetico non puo' riprodurla. I test di C5.3 girano quindi contro
        # il repository vero (vedi `CatenaStoricaTests`); qui si esercita
        # tutto il resto usando il nome di un manifesto della catena.
        self.precedente = {
            "manifest_version": 1,
            "component": "plenora-data-tools",
            "component_version": "1.0.2",
            "revision": self.base,
            "icd": {
                "repository": "plenora-contracts",
                "tag": "v2.0-rc10",
                "revision": "3598259bbe07d1c853453ff34ca2c1d1d28a0272",
            },
            "claims": {"component_rc": True, "system_rc": False, "avionic_certification": False},
            "verification_claim": "verified_internally",
            "independent_review": False,
        }
        self._scrivi("release/1.0.2.json", self.precedente)

    def tearDown(self):
        self.tmp.cleanup()

    def _git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.repo, check=True, text=True, capture_output=True
        )

    def _scrivi(self, relativo, contenuto):
        percorso = self.repo / relativo
        percorso.parent.mkdir(parents=True, exist_ok=True)
        percorso.write_text(json.dumps(contenuto, indent=2), encoding="utf-8")
        return percorso

    def _manifesto(self, **mutazioni):
        """Manifesto valido, con le mutazioni richieste applicate."""
        base = {
            "manifest_version": 1,
            "component": "plenora-data-tools",
            "component_version": "1.0.3",
            "revision": self.base,
            "icd": {
                "repository": "plenora-contracts",
                "tag": "v2.0-rc10",
                "revision": "3598259bbe07d1c853453ff34ca2c1d1d28a0272",
            },
            "claims": {"component_rc": True, "system_rc": False, "avionic_certification": False},
            "verification_claim": "verified_internally",
            "independent_review": False,
            "supersedes": {
                "manifest": "release/1.0.2.json",
                "component_version": "1.0.2",
                "revision": self.base,
            },
        }
        base.update(mutazioni)
        return base

    def _valida(self, manifesto, nome="release/1.0.3.json"):
        """Valida nel repository sintetico.

        Gli errori C5.3 sono attesi qui — la catena reale non esiste in un
        repo temporaneo — e i test generici filtrano su di essi guardando solo
        il criterio che stanno esercitando. C5.3 ha i suoi test dedicati, che
        girano contro il repository vero.
        """
        percorso = self._scrivi(nome, manifesto)
        return validate_manifest(self.repo, percorso)

    def _senza_catena(self, errori):
        return [e for e in errori if not e.startswith("C5.3")]

    # -- il caso felice ---------------------------------------------------

    def test_manifesto_valido_passa_con_il_solo_avviso_icd(self):
        errori, avvisi = self._valida(self._manifesto())
        self.assertEqual(self._senza_catena(errori), [])
        self.assertEqual(len(avvisi), 1)
        self.assertIn("icd.revision appartiene a un altro repository", avvisi[0])

    # -- C1.1 identita' ---------------------------------------------------

    def test_versione_del_manifesto_sbagliata(self):
        errori, _ = self._valida(self._manifesto(manifest_version=2))
        self.assertTrue(any("C1.1" in e and "manifest_version" in e for e in errori), errori)

    def test_componente_sbagliato(self):
        errori, _ = self._valida(self._manifesto(component="plenora-io-tools"))
        self.assertTrue(any("C1.1" in e and "component" in e for e in errori), errori)

    def test_versione_componente_assente(self):
        errori, _ = self._valida(self._manifesto(component_version=None))
        self.assertTrue(any("C1.1" in e and "component_version" in e for e in errori), errori)

    # -- C1.2 nome del file ------------------------------------------------

    def test_nome_del_file_non_corrisponde_alla_versione(self):
        errori, _ = self._valida(self._manifesto(component_version="9.9.9"))
        self.assertTrue(any("C1.2" in e for e in errori), errori)

    def test_nome_storico_ammesso_solo_per_la_forma_dichiarata(self):
        manifesto = self._manifesto(component_version="0.1.0-rc1")
        manifesto.pop("supersedes")
        errori, _ = self._valida(manifesto, nome="release/rc.json")
        self.assertEqual(errori, [], errori)
        errori, _ = self._valida(
            self._manifesto(component_version="1.2.3"), nome="release/rc.json"
        )
        self.assertTrue(any("C1.2" in e for e in errori), errori)

    # -- C2.2 riferimento all'ICD -----------------------------------------

    def test_icd_assente(self):
        errori, avvisi = self._valida(self._manifesto(icd=None))
        self.assertTrue(any("C2.2" in e and "icd assente" in e for e in errori), errori)
        self.assertEqual(avvisi, [], "senza icd non si emette l'avviso")

    def test_icd_di_un_altro_repository(self):
        errori, _ = self._valida(
            self._manifesto(
                icd={
                    "repository": "plenora-altro",
                    "tag": "v2.0-rc10",
                    "revision": "3598259bbe07d1c853453ff34ca2c1d1d28a0272",
                }
            )
        )
        self.assertTrue(any("C2.2" in e and "repository" in e for e in errori), errori)

    def test_icd_senza_tag(self):
        errori, _ = self._valida(
            self._manifesto(
                icd={
                    "repository": "plenora-contracts",
                    "revision": "3598259bbe07d1c853453ff34ca2c1d1d28a0272",
                }
            )
        )
        self.assertTrue(any("C2.2" in e and "tag" in e for e in errori), errori)

    def test_icd_con_revisione_non_esadecimale(self):
        errori, avvisi = self._valida(
            self._manifesto(
                icd={"repository": "plenora-contracts", "tag": "v2.0-rc10", "revision": "HEAD"}
            )
        )
        self.assertTrue(any("C2.2" in e and "40 cifre" in e for e in errori), errori)
        self.assertEqual(avvisi, [], "una revisione malformata non produce un avviso")

    def test_pin_icd_con_tag_ben_formato_ma_sbagliato(self):
        errori, avvisi = self._valida(
            self._manifesto(
                icd={
                    "repository": "plenora-contracts",
                    "tag": "v2.0-rc11",
                    "revision": ICD_REVISION,
                }
            )
        )
        self.assertTrue(any("C2.2" in e and "v2.0-rc10" in e for e in errori), errori)
        self.assertEqual(len(avvisi), 1, "la revisione resta corretta: l'avviso c'e'")

    def test_pin_icd_con_revisione_ben_formata_ma_sbagliata(self):
        errori, avvisi = self._valida(
            self._manifesto(
                icd={
                    "repository": "plenora-contracts",
                    "tag": ICD_TAG,
                    # 40 cifre esadecimali valide, ma non e' la revisione
                    # dell'ICD storico.
                    "revision": "a" * 40,
                }
            )
        )
        self.assertTrue(any("C2.2" in e and ICD_REVISION in e for e in errori), errori)
        self.assertEqual(avvisi, [], "una revisione sbagliata non produce l'avviso")

    # -- C3.1 la revisione e' un commit di questo repository ---------------

    def test_revisione_non_esadecimale(self):
        errori, _ = self._valida(self._manifesto(revision="non-una-sha"))
        self.assertTrue(any("C3.1" in e for e in errori), errori)

    def test_revisione_inesistente(self):
        errori, _ = self._valida(self._manifesto(revision="0" * 40))
        self.assertTrue(any("C3.1" in e and "non e' un commit" in e for e in errori), errori)

    def test_revisione_non_antenata_di_head(self):
        errori, _ = self._valida(self._manifesto(revision=self.estraneo))
        self.assertTrue(any("C3.1" in e and "antenato" in e for e in errori), errori)

    # -- C4.1 rivendicazioni ----------------------------------------------

    def test_rivendicazione_di_sistema(self):
        errori, _ = self._valida(
            self._manifesto(
                claims={"component_rc": True, "system_rc": True, "avionic_certification": False}
            )
        )
        self.assertTrue(any("C4.1" in e and "system_rc" in e for e in errori), errori)

    def test_rivendicazione_di_certificazione_avionica(self):
        errori, _ = self._valida(
            self._manifesto(
                claims={"component_rc": True, "system_rc": False, "avionic_certification": True}
            )
        )
        self.assertTrue(
            any("C4.1" in e and "avionic_certification" in e for e in errori), errori
        )

    def test_component_rc_non_booleano(self):
        errori, _ = self._valida(
            self._manifesto(
                claims={"component_rc": "si", "system_rc": False, "avionic_certification": False}
            )
        )
        self.assertTrue(any("C4.1" in e and "component_rc" in e for e in errori), errori)

    def test_claims_assenti(self):
        errori, _ = self._valida(self._manifesto(claims=None))
        self.assertTrue(any("C4.1" in e and "assente" in e for e in errori), errori)

    # -- C4.2 coerenza della dichiarazione di verifica ---------------------

    def test_dichiarazione_di_verifica_sconosciuta(self):
        errori, _ = self._valida(self._manifesto(verification_claim="verified_somehow"))
        self.assertTrue(any("C4.2" in e for e in errori), errori)

    def test_verifica_indipendente_dichiarata_senza_revisione_indipendente(self):
        errori, _ = self._valida(
            self._manifesto(verification_claim="verified_independently", independent_review=False)
        )
        self.assertTrue(any("C4.2" in e and "independent_review" in e for e in errori), errori)

    def test_verifica_interna_con_revisione_indipendente(self):
        errori, _ = self._valida(
            self._manifesto(verification_claim="verified_internally", independent_review=True)
        )
        self.assertTrue(any("C4.2" in e and "independent_review" in e for e in errori), errori)


class CloneRealeMixin:
    """Un clone a oggetti condivisi del repository, con `release/` popolata.

    Serve perche' il checker esige ora che il manifesto verificato stia in
    `repo/release/`: le mutazioni non possono piu' vivere in una directory
    temporanea qualunque. Il clone e' `--shared --no-checkout`, quindi non
    copia oggetti ne' albero di lavoro, ma porta con se' storia e **tag
    annotati** — che sono cio' che i controlli C5.3 verificano.
    """

    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory()
        cls.repo = Path(cls._tmp.name) / "clone"
        subprocess.run(
            ["git", "clone", "--shared", "--no-checkout", "--quiet", str(REPO), str(cls.repo)],
            check=True,
            capture_output=True,
            text=True,
        )
        (cls.repo / "release").mkdir(parents=True, exist_ok=True)
        for originale in (REPO / "release").glob("*.json"):
            shutil.copy2(originale, cls.repo / "release" / originale.name)

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def _con_manifesto(self, nome, mutazione):
        """Scrive `nome` nel clone applicando `mutazione(manifesto)` e valida.

        Il file originale e' ripristinato a fine test: le mutazioni non si
        accumulano fra un caso e l'altro.
        """
        percorso = self.repo / "release" / nome
        originale = percorso.read_text(encoding="utf-8")
        self.addCleanup(percorso.write_text, originale, encoding="utf-8")
        manifesto = json.loads(originale)
        mutazione(manifesto)
        percorso.write_text(json.dumps(manifesto, indent=2), encoding="utf-8")
        return validate_manifest(self.repo, percorso)


class CatenaStoricaTests(CloneRealeMixin, unittest.TestCase):
    """C5.3 contro un clone del repository reale: la catena e' un suo fatto."""

    def test_i_cinque_manifesti_passano_anche_nel_clone(self):
        for nome in CATENA:
            with self.subTest(manifesto=nome):
                errori, _ = validate_manifest(self.repo, self.repo / "release" / nome)
                self.assertEqual(errori, [], f"{nome}: {errori}")

    # -- presenza del predecessore ----------------------------------------

    def test_catena_assente_dove_e_obbligatoria(self):
        errori, _ = self._con_manifesto("1.0.3.json", lambda m: m.pop("supersedes"))
        self.assertTrue(any("C5.3" in e and "solo rc.json" in e for e in errori), errori)

    def test_il_capostipite_non_deve_dichiarare_un_predecessore(self):
        errori, _ = self._con_manifesto(
            "rc.json", lambda m: m.update(supersedes=dict(CATENA["1.0.3.json"]))
        )
        self.assertTrue(any("C5.3" in e and "capostipite" in e for e in errori), errori)

    # -- FORMA esatta: chiavi ---------------------------------------------

    def test_rimuovere_tag_da_1_0_3_fallisce(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].pop("tag")
        )
        self.assertTrue(
            any("C5.3" in e and "perso le chiavi storiche" in e and "tag" in e for e in errori),
            errori,
        )

    def test_rimuovere_tag_object_da_1_0_3_fallisce(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].pop("tag_object")
        )
        self.assertTrue(
            any("C5.3" in e and "perso le chiavi storiche" in e for e in errori), errori
        )

    def test_aggiungere_tag_dove_non_c_era_fallisce(self):
        for nome in ("1.0.0.json", "1.0.1.json", "1.0.2.json"):
            with self.subTest(manifesto=nome):
                tag, oggetto = TAG_PREDECESSORE[nome]
                errori, _ = self._con_manifesto(
                    nome,
                    lambda m, tag=tag, oggetto=oggetto: m["supersedes"].update(
                        tag=tag, tag_object=oggetto
                    ),
                )
                self.assertTrue(
                    any("C5.3" in e and "chiavi non storiche" in e for e in errori),
                    f"{nome}: {errori}",
                )

    def test_chiave_sconosciuta_nel_predecessore_fallisce(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].update(nota="aggiunta a posteriori")
        )
        self.assertTrue(
            any("C5.3" in e and "chiavi non storiche" in e and "nota" in e for e in errori),
            errori,
        )

    # -- FORMA esatta: valori ---------------------------------------------

    def test_predecessore_sbagliato(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].update(manifest="release/1.0.1.json")
        )
        self.assertTrue(
            any("C5.3" in e and "supersedes.manifest" in e for e in errori), errori
        )

    def test_versione_del_predecessore_sbagliata(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].update(component_version="1.0.1")
        )
        self.assertTrue(
            any("C5.3" in e and "component_version" in e for e in errori), errori
        )

    def test_revisione_del_predecessore_sbagliata(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].update(revision="b" * 40)
        )
        self.assertTrue(
            any("C5.3" in e and "supersedes.revision" in e for e in errori), errori
        )

    def test_tag_del_predecessore_sbagliato(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].update(tag="v1.0.1")
        )
        self.assertTrue(any("C5.3" in e and "supersedes.tag" in e for e in errori), errori)

    def test_oggetto_del_tag_del_predecessore_sbagliato(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].update(tag_object="c" * 40)
        )
        self.assertTrue(any("C5.3" in e and "tag_object" in e for e in errori), errori)

    # -- percorso del predecessore ----------------------------------------

    def test_percorso_del_predecessore_con_risalita(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json",
            lambda m: m["supersedes"].update(manifest="release/../../etc/passwd"),
        )
        self.assertTrue(any("C5.3" in e and "risalita" in e for e in errori), errori)

    def test_percorso_del_predecessore_assoluto(self):
        for assoluto in ["/etc/passwd", "C:/Windows/win.ini", "\\\\server\\share"]:
            with self.subTest(percorso=assoluto):
                errori, _ = self._con_manifesto(
                    "1.0.3.json",
                    lambda m, a=assoluto: m["supersedes"].update(manifest=a),
                )
                self.assertTrue(
                    any("C5.3" in e and "assoluto" in e for e in errori), errori
                )

    def test_percorso_del_predecessore_fuori_da_release(self):
        errori, _ = self._con_manifesto(
            "1.0.3.json", lambda m: m["supersedes"].update(manifest="docs/1.0.2.json")
        )
        self.assertTrue(
            any("C5.3" in e and "release/<manifesto>.json" in e for e in errori), errori
        )

    def test_supersedes_non_e_un_oggetto(self):
        for valore in ["release/1.0.2.json", ["release/1.0.2.json"], 42, True]:
            with self.subTest(valore=valore):
                errori, _ = self._con_manifesto(
                    "1.0.3.json", lambda m, v=valore: m.update(supersedes=v)
                )
                self.assertTrue(
                    any("C5.3" in e and "deve essere un oggetto" in e for e in errori),
                    errori,
                )

    def test_manifesto_del_predecessore_mancante(self):
        percorso = self.repo / "release/1.0.2.json"
        contenuto = percorso.read_text(encoding="utf-8")
        percorso.unlink()
        self.addCleanup(percorso.write_text, contenuto, encoding="utf-8")
        errori, _ = validate_manifest(self.repo, self.repo / "release/1.0.3.json")
        self.assertTrue(any("C5.3" in e and "non esiste" in e for e in errori), errori)

    def test_tag_del_repository_diverso_da_quello_registrato(self):
        """Il registro dice quale oggetto deve essere il tag; git deve confermarlo."""
        tag, _ = TAG_PREDECESSORE["1.0.3.json"]
        originale = subprocess.run(
            ["git", "rev-parse", f"refs/tags/{tag}"],
            cwd=self.repo, check=True, capture_output=True, text=True,
        ).stdout.strip()

        def ripristina():
            subprocess.run(["git", "tag", "-d", tag], cwd=self.repo,
                           check=False, capture_output=True)
            subprocess.run(["git", "update-ref", f"refs/tags/{tag}", originale],
                           cwd=self.repo, check=True, capture_output=True)

        self.addCleanup(ripristina)
        # Un tag annotato NUOVO sullo stesso commit: oggetto diverso, perche'
        # un oggetto tag contiene anche messaggio e data.
        subprocess.run(
            ["git", "tag", "-f", "-a", tag, "-m", "riscritto",
             CATENA["1.0.3.json"]["revision"]],
            cwd=self.repo, check=True, capture_output=True,
            env=dict(os.environ, GIT_COMMITTER_NAME="Test",
                     GIT_COMMITTER_EMAIL="test@example.invalid",
                     GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid"),
        )
        errori, _ = validate_manifest(self.repo, self.repo / "release/1.0.3.json")
        self.assertTrue(
            any("C5.3" in e and "non e' l'oggetto registrato" in e for e in errori), errori
        )

    def test_manifesto_fuori_dalla_catena_dichiarata(self):
        percorso = self.repo / "release" / "9.9.9.json"
        shutil.copy2(self.repo / "release/1.0.3.json", percorso)
        self.addCleanup(percorso.unlink)
        errori, _ = validate_manifest(self.repo, percorso)
        self.assertTrue(
            any("C5.3" in e and "catena storica dichiarata" in e for e in errori), errori
        )


class DirectoryCanonicaTests(CloneRealeMixin, unittest.TestCase):
    """Il manifesto verificato deve stare in `repo/release/`.

    Senza questo vincolo una copia esterna chiamata `1.0.3.json` veniva
    validata sul solo basename — e il basename decide catena e criteri.
    """

    def test_copia_esterna_rifiutata(self):
        esterna = tempfile.TemporaryDirectory()
        self.addCleanup(esterna.cleanup)
        percorso = Path(esterna.name) / "1.0.3.json"
        shutil.copy2(self.repo / "release/1.0.3.json", percorso)
        errori, avvisi = validate_manifest(self.repo, percorso)
        self.assertTrue(any("C1.2" in e and "deve trovarsi in" in e for e in errori), errori)
        self.assertEqual(avvisi, [], "il documento non viene nemmeno caricato")

    def test_sottodirectory_di_release_rifiutata(self):
        annidata = self.repo / "release" / "vecchi"
        annidata.mkdir(exist_ok=True)
        self.addCleanup(shutil.rmtree, annidata, ignore_errors=True)
        percorso = annidata / "1.0.3.json"
        shutil.copy2(self.repo / "release/1.0.3.json", percorso)
        errori, _ = validate_manifest(self.repo, percorso)
        self.assertTrue(any("C1.2" in e and "deve trovarsi in" in e for e in errori), errori)

    def test_risalita_dal_percorso_richiesto(self):
        # La forma con cui la CLI compone il percorso: `repo / argomento`.
        percorso = (self.repo / "release/../release/1.0.3.json").resolve()
        errori, _ = validate_manifest(self.repo, percorso)
        self.assertEqual(errori, [], "una risalita che RIENTRA in release/ e' legittima")
        fuori = (self.repo / "release/../1.0.3.json").resolve()
        shutil.copy2(self.repo / "release/1.0.3.json", fuori)
        self.addCleanup(fuori.unlink)
        errori, _ = validate_manifest(self.repo, fuori)
        self.assertTrue(any("C1.2" in e for e in errori), errori)

    def test_symlink_che_risolve_fuori_da_release(self):
        esterna = tempfile.TemporaryDirectory()
        self.addCleanup(esterna.cleanup)
        bersaglio = Path(esterna.name) / "1.0.3.json"
        shutil.copy2(self.repo / "release/1.0.3.json", bersaglio)
        collegamento = self.repo / "release" / "collegato.json"
        try:
            os.symlink(bersaglio, collegamento)
        except (OSError, NotImplementedError) as errore:
            self.skipTest(f"symlink non creabili su questo sistema: {errore}")
        self.addCleanup(collegamento.unlink)
        # `resolve()` segue il symlink: il parent risolto e' la directory
        # esterna, non `release/`.
        errori, _ = validate_manifest(self.repo, collegamento)
        self.assertTrue(any("C1.2" in e and "deve trovarsi in" in e for e in errori), errori)


class ProfiloTests(CloneRealeMixin, unittest.TestCase):
    """I manifesti futuri non si verificano con le regole storiche."""

    def test_profilo_non_storico_rifiutato(self):
        errori, avvisi = validate_manifest(
            self.repo, self.repo / "release/1.0.3.json", profilo="nuovo"
        )
        self.assertTrue(any("non definito" in e for e in errori), errori)
        self.assertEqual(avvisi, [])


class RadiceJsonTests(CloneRealeMixin, unittest.TestCase):
    """Un JSON valido la cui radice non e' un oggetto e' un errore, non un traceback."""

    def _scrivi_e_valida(self, contenuto):
        percorso = self.repo / "release" / "1.0.3.json"
        originale = percorso.read_text(encoding="utf-8")
        self.addCleanup(percorso.write_text, originale, encoding="utf-8")
        percorso.write_text(contenuto, encoding="utf-8")
        return validate_manifest(self.repo, percorso)

    def test_radice_non_oggetto(self):
        for contenuto in ["[1, 2, 3]", '"stringa"', "42", "null", "true"]:
            with self.subTest(radice=contenuto):
                errori, avvisi = self._scrivi_e_valida(contenuto)
                self.assertTrue(any("C1.1" in e for e in errori), errori)
                self.assertEqual(avvisi, [])

    def test_json_malformato(self):
        errori, _ = self._scrivi_e_valida("{non json")
        self.assertTrue(any("C1.1" in e for e in errori), errori)


class ManifestiStoriciRealiTests(unittest.TestCase):
    """I cinque manifesti veri devono passare, qui e in CI."""

    def test_i_cinque_manifesti_storici_passano(self):
        attesi = ["1.0.3.json", "1.0.2.json", "1.0.1.json", "1.0.0.json", "rc.json"]
        presenti = sorted(p.name for p in (REPO / "release").glob("*.json"))
        self.assertEqual(
            sorted(attesi), presenti, "l'insieme dei manifesti storici e' cambiato"
        )
        for nome in attesi:
            with self.subTest(manifesto=nome):
                errori, avvisi = validate_manifest(REPO, REPO / "release" / nome)
                self.assertEqual(errori, [], f"{nome}: {errori}")
                self.assertEqual(
                    len(avvisi), 1, f"{nome}: atteso il solo avviso C2.2, trovato {avvisi}"
                )

    def test_la_catena_dichiarata_copre_esattamente_i_manifesti_presenti(self):
        presenti = {p.name for p in (REPO / "release").glob("*.json")}
        self.assertEqual(
            presenti, set(CATENA), "la mappa della catena e i file devono coincidere"
        )
        self.assertEqual(
            set(TAG_PREDECESSORE),
            {nome for nome, atteso in CATENA.items() if atteso is not None},
            "ogni manifesto con predecessore deve avere il tag registrato",
        )

    def test_la_forma_registrata_e_quella_dei_manifesti_reali(self):
        # La mappa non e' una descrizione approssimata: e' il dizionario
        # `supersedes` come i manifesti lo dichiarano, chiavi comprese.
        for nome, atteso in CATENA.items():
            with self.subTest(manifesto=nome):
                reale = json.loads(
                    (REPO / "release" / nome).read_text(encoding="utf-8")
                ).get("supersedes")
                self.assertEqual(reale, atteso)

    def test_l_elenco_dei_criteri_non_automatizzati_e_quello_dichiarato(self):
        # Ricostruito dai log dell'ultima esecuzione riuscita della verifica
        # perduta (run 31166125840): se cambia, cambia una dichiarazione verso
        # chi legge il verdetto, non un dettaglio interno.
        self.assertEqual(
            NON_AUTOMATIZZATI, ("C2.1", "C2.3", "C3.2", "C4.3", "C5.1", "C5.2")
        )


if __name__ == "__main__":
    unittest.main()
