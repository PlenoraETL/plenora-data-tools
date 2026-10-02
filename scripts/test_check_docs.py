"""Prove della guardia sui documenti (`scripts/check_docs.py`).

Uso: python -m unittest discover -s scripts -p "test_*.py"
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import check_docs


def write(root: Path, relative: str, text: str) -> Path:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


class SlugTests(unittest.TestCase):
    def test_slugs_follow_github(self) -> None:
        self.assertEqual(check_docs.github_slug("Che cosa c'è"), "che-cosa-cè")
        self.assertEqual(check_docs.github_slug("WGS 84 = ETRS89 per convenzione"),
                         "wgs-84--etrs89-per-convenzione")
        self.assertEqual(
            check_docs.github_slug("`geo.nearest`: lo scarto dell'R-tree"),
            "geonearest-lo-scarto-dellr-tree",
        )

    def test_headings_in_code_fences_are_not_anchors(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = write(Path(directory), "a.md", "# Vero\n\n```sh\n# commento\n```\n")
            self.assertEqual(check_docs.anchors(path), {"vero"})

    def test_repeated_headings_get_numbered_anchors(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = write(Path(directory), "a.md", "# Limiti\n## Limiti\n## Limiti\n")
            self.assertEqual(check_docs.anchors(path), {"limiti", "limiti-1", "limiti-2"})


class LinkTests(unittest.TestCase):
    def test_upstream_vendor_documents_are_skipped_but_provenance_is_read(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(root, "vendor/geo/README.md", "[upstream](missing.md)")
            provenance = write(root, "vendor/geo/PROVENANCE.md", "# Provenienza")
            readme = write(root, "README.md", "# Prodotto")
            self.assertEqual(check_docs.markdown_documents(root), [readme, provenance])

    def test_missing_local_link_and_anchor_are_reported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(root, "README.md", "# Home\n\n[missing](no.md) [anchor](target.md#missing)\n")
            write(root, "target.md", "# Present\n")
            documents = check_docs.markdown_documents(root)
            reasons = [item.reason for item in check_docs.validate_links(root, documents)]
            self.assertTrue(any("link locale inesistente" in item for item in reasons))
            self.assertTrue(any("ancora inesistente" in item for item in reasons))

    def test_reference_style_links_are_checked(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(
                root,
                "README.md",
                "# Home\n\nVedi [il target][t] e [l'altro][rotto].\n\n"
                "[t]: target.md#present\n"
                "  [rotto]: <target.md#manca> \"titolo\"\n"
                "```md\n[nel codice]: assente.md\n```\n",
            )
            write(root, "target.md", "# Present\n")
            documents = check_docs.markdown_documents(root)
            reasons = [item.reason for item in check_docs.validate_links(root, documents)]
            self.assertEqual(reasons, ["ancora inesistente: target.md#manca"])

    def test_schede_links_resolve_from_the_assembled_document(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(root, "docs/operazioni.md", "# Operazioni\n## `table.sort`\n")
            write(root, "docs/limiti.md", "# Limiti dichiarati\n")
            write(
                root,
                "docs/schede/table.top_n.md",
                "[sort](#tablesort) [limiti](limiti.md#limiti-dichiarati) "
                "[radice](../README.md)\n",
            )
            write(root, "README.md", "# Radice\n")
            documents = check_docs.markdown_documents(root)
            self.assertEqual(check_docs.validate_links(root, documents), [])
            # Il percorso spezzato non è un rimando a un file del repository.
            write(root, "docs/schede/" + "table.rotta.md", "[rotto](#tableassente)\n")
            documents = check_docs.markdown_documents(root)
            self.assertEqual(len(check_docs.validate_links(root, documents)), 1)

    def test_missing_python_command_is_reported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            document = write(root, "README.md", "```sh\npython scripts/" + "missing.py\n```\n")
            violations = check_docs.validate_commands(root, [document])
            self.assertEqual(len(violations), 1)
            self.assertIn("missing.py", violations[0].reason)

    def test_invalid_python_example_is_reported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            document = write(Path(directory), "a.md", "```python\ndef (:\n```\n")
            self.assertEqual(len(check_docs.validate_python_examples([document])), 1)

    def test_double_encoded_document_is_reported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            twice = "più".encode("utf-8").decode("cp1252")
            document = write(Path(directory), "a.md", f"# Titolo\n\nscritto in {twice}\n")
            self.assertEqual(len(check_docs.validate_encoding([document])), 1)


class CodeReferenceTests(unittest.TestCase):
    def test_references_must_name_an_existing_title(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(
                root,
                "docs/runner.md",
                "# Runner\n## Budget di memoria\n- **Modelli di costo geo.** testo\n",
            )
            write(root, "README.md", "# Radice\n## Suite lunga\n")
            write(
                root,
                "a.rs",
                "// vale il budget (docs/runner.md, «Budget di\n"
                "// memoria») e i modelli (docs/runner.md, «Modelli di costo geo»);\n"
                "/// la suite (README, «Suite lunga»).\n",
            )
            self.assertEqual(check_docs.validate_code_references(root), [])
            write(root, "b.py", "# vedi docs/runner.md, «Titolo che non c'è»\n")
            write(root, "c.toml", "# vedi docs/" + "assente.md, «Runner»\n")
            reasons = [item.reason for item in check_docs.validate_code_references(root)]
            self.assertEqual(len(reasons), 2)

    def test_code_references_are_reported_in_path_order(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(root, "README.md", "# Radice\n")
            for name in ("z.rs", "m/b.py", "a.toml", "m/a.rs"):
                marker = "#" if name.endswith((".py", ".toml")) else "//"
                write(root, name, f"{marker} vedi README, «Assente»\n")
            paths = [item.path for item in check_docs.validate_code_references(root)]
            self.assertEqual(paths, sorted(paths))
            self.assertEqual(len(paths), 4)

    def test_apostrophe_accents_match_accented_titles(self) -> None:
        self.assertEqual(
            check_docs.normalized_title("Feature d'ingresso piu' vicine"),
            check_docs.normalized_title("Feature d'ingresso più vicine"),
        )


class RepositoryTests(unittest.TestCase):
    def test_repository_documents_pass(self) -> None:
        checked, violations = check_docs.scan()
        self.assertGreaterEqual(checked, 15)
        self.assertEqual(violations, [])


if __name__ == "__main__":
    unittest.main()
