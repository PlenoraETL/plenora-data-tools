import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.check_release_candidate import validate_candidate


class ReleaseCandidateCheckerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        self._write("Cargo.toml", '[workspace]\nmembers=["crates/a"]\n[workspace.package]\nversion="1.0.2"\n')
        self._write("crates/a/Cargo.toml", '[package]\nname="plenora-a"\nversion.workspace=true\nedition="2021"\n')
        self._write("Cargo.lock", 'version = 4\n\n[[package]]\nname="plenora-a"\nversion="1.0.2"\n')
        self._write("fuzz/Cargo.toml", '[package]\nname="fuzz"\nversion="0.0.0"\n[dependencies]\nplenora-a={path="../crates/a"}\n')
        self._write("fuzz/Cargo.lock", 'version = 4\n\n[[package]]\nname="plenora-a"\nversion="1.0.2"\n')
        self._write("README.md", "v1.0.2\n")
        self._write(".github/workflows/ci.yml", "historical\n")
        self._git("init", "-q")
        self._git("config", "user.email", "test@example.invalid")
        self._git("config", "user.name", "Test")
        self._git("add", ".")
        self._git("commit", "-qm", "base")
        self.base = self._git("rev-parse", "HEAD").stdout.strip()
        self._git("tag", "-a", "v1.0.2", "-m", "v1.0.2")
        self.previous_tag_object = self._git("rev-parse", "refs/tags/v1.0.2").stdout.strip()
        self._write("Cargo.toml", '[workspace]\nmembers=["crates/a"]\n[workspace.package]\nversion="1.0.3"\n')
        self._write("Cargo.lock", 'version = 4\n\n[[package]]\nname="plenora-a"\nversion="1.0.3"\n')
        self._write("fuzz/Cargo.lock", 'version = 4\n\n[[package]]\nname="plenora-a"\nversion="1.0.3"\n')
        self._write("README.md", "v1.0.3\n")
        self._write(".github/workflows/ci.yml", "candidate\n")
        manifest = {
            "manifest_version": 1,
            "component": "plenora-data-tools",
            "component_version": "1.0.3",
            "revision": self.base,
            "verification_claim": "verified_internally",
            "independent_review": False,
            "release_state": "metadata_candidate_pending_same_sha_ci",
            "claims": {"component_rc": True, "system_rc": False, "avionic_certification": False},
            "supersedes": {
                "component_version": "1.0.2", "revision": self.base,
                "tag": "v1.0.2", "tag_object": self.previous_tag_object
            },
            "candidate": {
                "evidence_base_revision": self.base,
                "evidence_base_ci_run": 123,
                "evidence_base_ci_url": "https://github.com/PlenoraETL/plenora-data-tools/actions/runs/123",
                "evidence_base_result": "passed",
                "metadata_delta_requires_new_same_sha_ci": True,
                "release_tag_created": False,
                "intended_release_tag": "v1.0.3",
                "publish": False,
                "declared_delta": [
                    ".github/workflows/ci.yml", "Cargo.lock", "Cargo.toml",
                    "README.md", "fuzz/Cargo.lock", "release/1.0.3.json"
                ],
            },
            "evidence": {
                "functional_revision": {"revision": self.base, "same_sha_ci_run": 123, "result": "passed"},
                "metadata_candidate": {"status": "pending_same_sha_ci", "transferable_from_evidence_base": False},
            },
        }
        self._write("release/1.0.3.json", json.dumps(manifest))
        self._git("add", ".")

    def tearDown(self):
        self.tmp.cleanup()

    def _write(self, relative, content):
        path = self.repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def _git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repo, check=True, text=True, capture_output=True)

    def _errors(self):
        return validate_candidate(self.repo, self.repo / "release/1.0.3.json")

    def test_valid_staged_candidate_passes(self):
        self.assertEqual([], self._errors())

    def test_workspace_version_mismatch_fails(self):
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["component_version"] = "9.9.9"
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("workspace version" in error for error in self._errors()))

    def test_root_lock_mismatch_fails(self):
        self._write("Cargo.lock", 'version = 4\n\n[[package]]\nname="plenora-a"\nversion="1.0.2"\n')
        self._git("add", "Cargo.lock")
        self.assertTrue(any("Cargo.lock" in error for error in self._errors()))

    def test_fuzz_lock_mismatch_fails(self):
        self._write("fuzz/Cargo.lock", 'version = 4\n\n[[package]]\nname="plenora-a"\nversion="1.0.2"\n')
        self._git("add", "fuzz/Cargo.lock")
        self.assertTrue(any("fuzz/Cargo.lock" in error for error in self._errors()))

    def test_declared_delta_mismatch_fails(self):
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["candidate"]["declared_delta"].remove("README.md")
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("declared_delta" in error for error in self._errors()))

    def test_publish_or_tag_state_fails_closed(self):
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["candidate"]["publish"] = True
        data["candidate"]["release_tag_created"] = True
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        errors = self._errors()
        self.assertTrue(any("publish" in error for error in errors))
        self.assertTrue(any("release_tag_created" in error for error in errors))

    def test_existing_candidate_tag_fails(self):
        self._git("tag", "v1.0.3", self.base)
        self.assertTrue(any("must not already exist" in error for error in self._errors()))

    def test_untracked_file_fails(self):
        self._write("unexpected.txt", "not part of the candidate")
        self.assertTrue(any("untracked files are forbidden" in error for error in self._errors()))

    def test_ci_run_binding_mismatch_fails(self):
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["evidence"]["functional_revision"]["same_sha_ci_run"] = 999
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("same_sha_ci_run" in error for error in self._errors()))

    def test_annotated_tag_object_mismatch_fails(self):
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["supersedes"]["tag_object"] = "0" * 40
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("tag object" in error for error in self._errors()))

    def test_committed_ci_candidate_passes(self):
        self._git("commit", "-qm", "candidate")
        self.assertEqual([], self._errors())

    def test_member_must_inherit_workspace_version(self):
        self._write("crates/a/Cargo.toml", '[package]\nname="plenora-a"\nversion="1.0.3"\nedition="2021"\n')
        self._git("add", "crates/a/Cargo.toml")
        self.assertTrue(any("version.workspace" in error for error in self._errors()))

    def test_tracked_unstaged_change_fails(self):
        self._write("README.md", "dirty but not staged\n")
        self.assertTrue(any("unstaged changes are forbidden" in error for error in self._errors()))

    def test_revision_tag_object_is_not_accepted_as_commit(self):
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["revision"] = self.previous_tag_object
        data["candidate"]["evidence_base_revision"] = self.previous_tag_object
        data["evidence"]["functional_revision"]["revision"] = self.previous_tag_object
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("directly identify a commit" in error for error in self._errors()))

    def test_revision_must_be_ancestor(self):
        side = self._git("commit-tree", "HEAD^{tree}", "-m", "side").stdout.strip()
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["revision"] = side
        data["candidate"]["evidence_base_revision"] = side
        data["evidence"]["functional_revision"]["revision"] = side
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("ancestor" in error for error in self._errors()))

    def test_lightweight_previous_tag_fails(self):
        self._git("tag", "-d", "v1.0.2")
        self._git("tag", "v1.0.2", self.base)
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["supersedes"]["tag_object"] = self.base
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("must be annotated" in error for error in self._errors()))

    def test_previous_tag_wrong_target_fails(self):
        side = self._git("commit-tree", "HEAD^{tree}", "-m", "wrong target").stdout.strip()
        self._git("tag", "-d", "v1.0.2")
        self._git("tag", "-a", "v1.0.2", side, "-m", "moved")
        data = json.loads((self.repo / "release/1.0.3.json").read_text())
        data["supersedes"]["tag_object"] = self._git("rev-parse", "refs/tags/v1.0.2").stdout.strip()
        self._write("release/1.0.3.json", json.dumps(data))
        self._git("add", "release/1.0.3.json")
        self.assertTrue(any("declared revision" in error for error in self._errors()))

    def test_duplicate_first_party_lock_entry_fails(self):
        with (self.repo / "Cargo.lock").open("a", encoding="utf-8") as handle:
            handle.write('\n[[package]]\nname="plenora-a"\nversion="1.0.2"\n')
        self._git("add", "Cargo.lock")
        self.assertTrue(any("Cargo.lock" in error for error in self._errors()))

    def test_target_specific_fuzz_path_dependency_is_checked(self):
        with (self.repo / "fuzz/Cargo.toml").open("a", encoding="utf-8") as handle:
            handle.write("\n[target.'cfg(unix)'.dependencies]\nmissing={path='../missing',package='plenora-missing'}\n")
        self._git("add", "fuzz/Cargo.toml")
        self.assertTrue(any("plenora-missing" in error for error in self._errors()))

    def test_pending_state_and_claim_mutations_fail(self):
        original = json.loads((self.repo / "release/1.0.3.json").read_text())
        mutations = (
            ("claim", lambda d: d.__setitem__("verification_claim", "verified_independently")),
            ("review", lambda d: d.__setitem__("independent_review", True)),
            ("state", lambda d: d.__setitem__("release_state", "published")),
            ("system", lambda d: d["claims"].__setitem__("system_rc", True)),
            ("metadata", lambda d: d["evidence"]["metadata_candidate"].__setitem__("status", "passed")),
            ("transfer", lambda d: d["evidence"]["metadata_candidate"].__setitem__("transferable_from_evidence_base", True)),
        )
        for label, mutate in mutations:
            with self.subTest(label=label):
                data = json.loads(json.dumps(original))
                mutate(data)
                self._write("release/1.0.3.json", json.dumps(data))
                self._git("add", "release/1.0.3.json")
                self.assertTrue(self._errors())


if __name__ == "__main__":
    unittest.main()
