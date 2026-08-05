#!/usr/bin/env python3
"""Fail-closed checks for a Plenora Data Tools release metadata candidate."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


def _git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args], cwd=repo, check=check, text=True, capture_output=True
    )


def _load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def _load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def _path_package_names(repo: Path, workspace: dict[str, Any]) -> list[str]:
    names: list[str] = []
    for member in workspace.get("workspace", {}).get("members", []):
        manifest = _load_toml(repo / member / "Cargo.toml")
        package = manifest.get("package", {})
        name = package.get("name")
        if not isinstance(name, str):
            raise ValueError(f"{member}/Cargo.toml has no package.name")
        declared_version = package.get("version")
        if not isinstance(declared_version, dict) or declared_version.get("workspace") is not True:
            raise ValueError(f"{member}/Cargo.toml must inherit version.workspace")
        names.append(name)
    return sorted(names)


def _lock_versions(path: Path, names: set[str]) -> dict[str, list[str]]:
    lock = _load_toml(path)
    found: dict[str, list[str]] = {name: [] for name in names}
    for package in lock.get("package", []):
        name = package.get("name")
        if name in names and package.get("source") is None:
            found[name].append(str(package.get("version")))
    return found


def _fuzz_path_package_names(repo: Path) -> set[str]:
    manifest = _load_toml(repo / "fuzz/Cargo.toml")
    names: set[str] = set()
    dependency_tables = [manifest]
    dependency_tables.extend(manifest.get("target", {}).values())
    for container in dependency_tables:
        for table_name in ("dependencies", "dev-dependencies", "build-dependencies"):
            for name, declaration in container.get(table_name, {}).items():
                if isinstance(declaration, dict) and "path" in declaration:
                    names.add(str(declaration.get("package", name)))
    return names


def _changed_files(repo: Path, revision: str, errors: list[str]) -> set[str]:
    untracked = {
        line
        for line in _git(repo, "ls-files", "--others", "--exclude-standard").stdout.splitlines()
        if line
    }
    if untracked:
        errors.append(f"untracked files are forbidden: {sorted(untracked)}")

    unstaged = {
        line for line in _git(repo, "diff", "--name-only").stdout.splitlines() if line
    }
    if unstaged:
        errors.append(f"unstaged changes are forbidden: {sorted(unstaged)}")

    staged = {
        line
        for line in _git(repo, "diff", "--cached", "--name-only").stdout.splitlines()
        if line
    }
    head = _git(repo, "rev-parse", "HEAD").stdout.strip()
    if staged:
        if head != revision:
            errors.append("staged candidate requires HEAD == manifest revision")
        return staged

    result = _git(repo, "diff", "--name-only", f"{revision}..HEAD", check=False)
    if result.returncode != 0:
        errors.append(f"cannot compute candidate delta from {revision}")
        return set()
    return {line for line in result.stdout.splitlines() if line}


def verify_github_evidence(manifest: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    candidate = manifest.get("candidate", {})
    run_id = candidate.get("evidence_base_ci_run")
    if not isinstance(run_id, int) or run_id <= 0:
        return ["evidence_base_ci_run must be a positive integer"]
    api_url = (
        "https://api.github.com/repos/PlenoraETL/plenora-data-tools/"
        f"actions/runs/{run_id}"
    )
    request = urllib.request.Request(
        api_url,
        headers={"Accept": "application/vnd.github+json", "User-Agent": "plenora-release-checker"},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            run = json.load(response)
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as exc:
        return [f"cannot verify evidence GitHub run {run_id}: {exc}"]
    if run.get("id") != run_id:
        errors.append("GitHub evidence run id mismatch")
    if run.get("head_sha") != manifest.get("revision"):
        errors.append("GitHub evidence run head_sha != manifest revision")
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        errors.append("GitHub evidence run is not completed/success")
    return errors


def validate_candidate(repo: Path, manifest_path: Path) -> list[str]:
    repo = repo.resolve()
    manifest_path = manifest_path.resolve()
    errors: list[str] = []
    try:
        manifest = _load_json(manifest_path)
        workspace = _load_toml(repo / "Cargo.toml")
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
        return [str(exc)]

    version = workspace.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(version, str) or not re.fullmatch(r"\d+\.\d+\.\d+", version):
        errors.append("workspace version must be a stable x.y.z SemVer")
        return errors

    expected_manifest_name = f"{version}.json"
    if manifest_path.name != expected_manifest_name:
        errors.append(
            f"manifest filename {manifest_path.name} != workspace version {expected_manifest_name}"
        )
    if manifest.get("component") != "plenora-data-tools":
        errors.append("component must be plenora-data-tools")
    if manifest.get("component_version") != version:
        errors.append("manifest component_version != workspace version")

    try:
        package_names = _path_package_names(repo, workspace)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        errors.append(str(exc))
        package_names = []

    expected_names = set(package_names)
    try:
        fuzz_names = _fuzz_path_package_names(repo)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        errors.append(f"fuzz/Cargo.toml: {exc}")
        fuzz_names = set()
    for relative, required_names in (
        ("Cargo.lock", expected_names),
        ("fuzz/Cargo.lock", fuzz_names),
    ):
        try:
            versions = _lock_versions(repo / relative, required_names)
        except (OSError, tomllib.TOMLDecodeError) as exc:
            errors.append(f"{relative}: {exc}")
            continue
        missing = {name for name in required_names if not versions.get(name)}
        if missing:
            errors.append(f"{relative} missing first-party packages: {sorted(missing)}")
        mismatched = {
            name: found for name, found in versions.items() if found != [version]
        }
        if mismatched:
            errors.append(f"{relative} first-party versions != {version}: {mismatched}")

    if manifest.get("verification_claim") != "verified_internally":
        errors.append("metadata candidate verification_claim must be verified_internally")
    if manifest.get("independent_review") is not False:
        errors.append("metadata candidate independent_review must be false")
    if manifest.get("release_state") != "metadata_candidate_pending_same_sha_ci":
        errors.append("release_state must be metadata_candidate_pending_same_sha_ci")

    claims = manifest.get("claims", {})
    if claims.get("system_rc") is not False:
        errors.append("system_rc must be false")
    if claims.get("avionic_certification") is not False:
        errors.append("avionic_certification must be false")

    revision = manifest.get("revision")
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        errors.append("revision must be a lowercase 40-hex commit")
        return errors
    revision_type = _git(repo, "cat-file", "-t", revision, check=False)
    if revision_type.returncode or revision_type.stdout.strip() != "commit":
        errors.append("manifest revision must directly identify a commit object")
        return errors
    if _git(repo, "merge-base", "--is-ancestor", revision, "HEAD", check=False).returncode:
        errors.append("manifest revision must be an ancestor of HEAD")

    candidate = manifest.get("candidate", {})
    if candidate.get("evidence_base_revision") != revision:
        errors.append("candidate evidence_base_revision must equal manifest revision")
    evidence_run = candidate.get("evidence_base_ci_run")
    if not isinstance(evidence_run, int) or evidence_run <= 0:
        errors.append("evidence_base_ci_run must be a positive integer")
    expected_ci_url = (
        "https://github.com/PlenoraETL/plenora-data-tools/actions/runs/"
        f"{evidence_run}"
    )
    if candidate.get("evidence_base_ci_url") != expected_ci_url:
        errors.append("evidence_base_ci_url does not match evidence_base_ci_run")
    if candidate.get("evidence_base_result") != "passed":
        errors.append("candidate evidence_base_result must be passed")
    if candidate.get("metadata_delta_requires_new_same_sha_ci") is not True:
        errors.append("metadata_delta_requires_new_same_sha_ci must be true")
    if candidate.get("release_tag_created") is not False:
        errors.append("release_tag_created must be false before tag")
    if candidate.get("publish") is not False:
        errors.append("publish must be false before release")

    intended_tag = candidate.get("intended_release_tag")
    expected_tag = f"v{version}"
    if intended_tag != expected_tag:
        errors.append(f"intended_release_tag must be {expected_tag}")
    elif _git(repo, "rev-parse", "--verify", f"refs/tags/{expected_tag}", check=False).returncode == 0:
        errors.append(f"candidate tag {expected_tag} must not already exist")

    supersedes = manifest.get("supersedes", {})
    previous_version = supersedes.get("component_version")
    previous_revision = supersedes.get("revision")
    if not isinstance(previous_version, str) or not isinstance(previous_revision, str):
        errors.append("supersedes must name the previous version and revision")
    else:
        previous_tag = f"v{previous_version}"
        if supersedes.get("tag") != previous_tag:
            errors.append(f"supersedes tag must be {previous_tag}")
        tag_ref = _git(repo, "rev-parse", f"refs/tags/{previous_tag}", check=False)
        tag_object = tag_ref.stdout.strip() if tag_ref.returncode == 0 else ""
        if not tag_object or supersedes.get("tag_object") != tag_object:
            errors.append(f"superseded tag object {previous_tag} does not match declaration")
        tag_type = _git(repo, "cat-file", "-t", tag_object, check=False) if tag_object else None
        if tag_type is None or tag_type.returncode or tag_type.stdout.strip() != "tag":
            errors.append(f"superseded tag {previous_tag} must be annotated")
        peeled = _git(repo, "rev-parse", f"{previous_tag}^{{commit}}", check=False)
        if peeled.returncode or peeled.stdout.strip() != previous_revision:
            errors.append(f"superseded tag {previous_tag} does not match declared revision")

    evidence = manifest.get("evidence", {})
    functional = evidence.get("functional_revision", {})
    metadata = evidence.get("metadata_candidate", {})
    if functional.get("revision") != revision or functional.get("result") != "passed":
        errors.append("functional_revision evidence must bind the passed base revision")
    if functional.get("same_sha_ci_run") != evidence_run:
        errors.append("functional_revision same_sha_ci_run != candidate evidence run")
    if metadata.get("status") != "pending_same_sha_ci":
        errors.append("metadata candidate status must be pending_same_sha_ci")
    if metadata.get("transferable_from_evidence_base") is not False:
        errors.append("metadata evidence must not be transferable from the base")

    declared = candidate.get("declared_delta")
    if not isinstance(declared, list) or not all(isinstance(item, str) for item in declared):
        errors.append("declared_delta must be a list of paths")
    else:
        actual = _changed_files(repo, revision, errors)
        if set(declared) != actual or len(declared) != len(set(declared)):
            errors.append(
                f"declared_delta mismatch: declared={sorted(declared)} actual={sorted(actual)}"
            )

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--verify-github-run", action="store_true")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    errors = validate_candidate(repo, (repo / args.manifest).resolve())
    if not errors and args.verify_github_run:
        errors.extend(verify_github_evidence(_load_json((repo / args.manifest).resolve())))
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"{args.manifest}: release candidate checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
