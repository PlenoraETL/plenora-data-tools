"""Prove della guardia sui commenti (`scripts/check_comments.py`).

Uso: python -m unittest discover -s scripts -p "test_*.py"
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import check_comments


def violations_of(name: str, source: str) -> list[check_comments.Violation]:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        path = root / name
        path.write_text(source, encoding="utf-8")
        return check_comments.check_file(path, root)


class CommentExtractionTests(unittest.TestCase):
    def test_upstream_vendor_and_patches_are_not_product_comments(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for upstream in ("vendor", "patches"):
                (root / upstream).mkdir()
                (root / upstream / "upstream.rs").write_text("// TODO upstream\n", encoding="utf-8")
            (root / "product.rs").write_text("// TODO product\n", encoding="utf-8")
            checked, violations = check_comments.check_repository(root)
            self.assertEqual(checked, 1)
            self.assertEqual(len(violations), 1)

    def test_python_reads_comments_and_docstrings_but_not_values(self) -> None:
        source = '''"""TODO nel modulo"""
VALUE = "FIXME in una stringa"
# HACK nel commento
def operation():
    """XXX nella funzione."""
'''
        comments = list(check_comments.comments_for(Path("sample.py"), source))
        self.assertEqual([comment.line for comment in comments], [3, 1, 5])

    def test_stub_files_are_python(self) -> None:
        self.assertTrue(check_comments.commentable(Path("pacchetto/__init__.pyi")))
        comments = list(check_comments.comments_for(Path("x.pyi"), "# TODO stub\n"))
        self.assertEqual(len(comments), 1)

    def test_rust_ignores_ordinary_and_raw_strings(self) -> None:
        source = '''
const A: &str = "// TODO non e un commento";
const B: &str = r#"/* FIXME non e un commento */"#;
// HACK reale
/* XXX reale */
'''
        comments = list(check_comments.comments_for(Path("sample.rs"), source))
        self.assertEqual([comment.line for comment in comments], [4, 5])

    def test_rust_char_literals_do_not_open_strings(self) -> None:
        source = """fn f(c: char) -> bool {
    c == '"' || c == '\\'' // commento dopo i caratteri
}
fn g<'a>(x: &'a str) -> &'a str { x } // dopo una lifetime
// TODO alla riga 5
"""
        comments = list(check_comments.comments_for(Path("sample.rs"), source))
        self.assertEqual([comment.line for comment in comments], [2, 4, 5])

    def test_toml_yaml_and_git_files_read_hash_comments(self) -> None:
        for name in ("Cargo.toml", "ci.yml", ".gitattributes"):
            comments = list(check_comments.comments_for(Path(name), 'a = "#"\n# TODO\n'))
            self.assertEqual([comment.line for comment in comments], [2], name)


class CommentRuleTests(unittest.TestCase):
    def test_repository_check_reports_only_comment_violations(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "valid.py").write_text('VALUE = "TODO non commento"\n', encoding="utf-8")
            (root / "invalid.rs").write_text("// prima stesura\n", encoding="utf-8")
            checked, violations = check_comments.check_repository(root)
        self.assertEqual(checked, 2)
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].rule, "cronaca obsoleta")

    def test_roadmap_labels_are_process_history(self) -> None:
        source = """// F3-7 - vecchia fase
// === A4: session context ===
// P1.2 - read filters
// Operazioni geo del runner (F4)
"""
        violations = violations_of("history.rs", source)
        self.assertEqual({item.line for item in violations}, {1, 2, 3, 4})
        self.assertTrue(all(item.rule == "cronaca obsoleta" for item in violations))

    def test_types_and_protocol_names_are_not_labels(self) -> None:
        source = """// ParameterValue::F64 e un tipo del protocollo; f64 e f32 sono tipi.
// Un errore puo avvenire in fase Commit.
// Il risultato dipende da cosa c'era gia nella tabella.
// La previsione del modello e una stima, non una misura.
"""
        self.assertEqual(violations_of("contract.rs", source), [])

    def test_reviews_reviewers_and_rounds_are_process_history(self) -> None:
        source = """/// Regressione (revisione Codex): un caso.
// Il controesempio della seconda lettura.
// Quindicesimo giro: lo schema.
// Un caso trovato dal secondo lettore.
// Visto in review.
"""
        violations = violations_of("history.rs", source)
        self.assertEqual({item.line for item in violations}, {1, 2, 3, 4, 5})

    def test_a_contract_revision_by_commit_is_provenance(self) -> None:
        source = "// Profilo di plenora-contracts (revisione `ade868cf89c6652c`).\n"
        self.assertEqual(violations_of("lib.rs", source), [])

    def test_pr_numbers_dates_and_deciders_are_process_history(self) -> None:
        source = """# Pin scelto in PR #46.
# Decisione del maintainer.
# Il reperto del 5 settembre 2026.
# Regressione (difetto 3).
"""
        violations = violations_of("Cargo.toml", source)
        self.assertEqual({item.line for item in violations}, {1, 2, 3, 4})

    def test_upstream_issue_numbers_are_not_pr_numbers(self) -> None:
        self.assertEqual(violations_of("Cargo.toml", "# solver upstream #87\n"), [])

    def test_historical_decision_labels_are_rejected(self) -> None:
        violations = violations_of("history.rs", "// ADR 0014\n// Prima di questo fix\n")
        self.assertEqual(len(violations), 2)

    def test_debt_markers_are_case_sensitive_to_avoid_italian_todo(self) -> None:
        source = "# tutto a posto, non TODO\n"
        violations = [
            check_comments.DEBT_MARKER.findall(comment.text)
            for comment in check_comments.comments_for(Path("sample.py"), source)
        ]
        self.assertEqual(violations, [["TODO"]])


class DoubleEncodingTests(unittest.TestCase):
    def test_double_encoded_utf8_is_found(self) -> None:
        twice = "più «File» — è".encode("utf-8").decode("cp1252")
        self.assertEqual(len(check_comments.double_encoded(twice)), 5)

    def test_accented_text_and_guillemets_are_not_double_encoded(self) -> None:
        for text in ("è»", "perché», «così»", "città°", "À la carte"):
            self.assertEqual(check_comments.double_encoded(text), [], text)

    def test_a_double_encoded_comment_is_a_violation(self) -> None:
        twice = "più".encode("utf-8").decode("cp1252")
        violations = violations_of("sample.rs", f"// scritto in {twice} blocchi\n")
        self.assertEqual([item.rule for item in violations], ["doppia codifica UTF-8"])

    def test_a_double_encoded_string_literal_is_data(self) -> None:
        twice = "é".encode("utf-8").decode("cp1252")
        self.assertEqual(violations_of("sample.rs", f'const A: &str = "{twice}";\n'), [])


class RepositoryTests(unittest.TestCase):
    def test_repository_comments_pass(self) -> None:
        checked, violations = check_comments.check_repository()
        self.assertGreater(checked, 100)
        self.assertEqual(violations, [])


if __name__ == "__main__":
    unittest.main()
