#!/usr/bin/env python3
"""Exercise workflow Python with synthetic staged bytes and an in-memory GitHub API."""

from contextlib import ExitStack, chdir, redirect_stdout
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import socket
import tempfile
import textwrap
import unittest
from unittest.mock import patch
import urllib.error
import urllib.parse
import urllib.request


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/publish-release.yml"
SOURCE = "a" * 40
VERSION = "0.7.0"
REPOSITORY = "fixture/publication-boundaries"
TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu")
IMAGE = "ubuntu@sha256:224a1869083a311ef3f13648a154ba79832fbef6364d31493642ca03082da254"


def workflow_programs():
    programs = []
    lines = iter(WORKFLOW.read_text().splitlines())
    for line in lines:
        if line.strip() != "python3 - <<'PY'":
            continue
        indentation = line[:len(line) - len(line.lstrip())]
        body = []
        for line in lines:
            if line == indentation + "PY":
                break
            body.append(line)
        else:
            raise ValueError("Unterminated workflow Python heredoc")
        programs.append(compile(textwrap.dedent("\n".join(body)), str(WORKFLOW), "exec"))
    if len(programs) != 2:
        raise ValueError("Expected authorization and publication workflow programs")
    return programs


class FakeGitHub:
    def __init__(self):
        self.calls = []
        self.roles = {"reviewer": "maintain", "rerunner": "admin"}
        self.fault = None
        self.tag = None
        self.release = None
        self.upload_count = 0

    def response(self, value):
        return io.BytesIO(json.dumps(value).encode())

    def error(self, request, code):
        raise urllib.error.HTTPError(request.full_url, code, "synthetic refusal", {}, None)

    def __call__(self, request):
        url = urllib.parse.urlsplit(request.full_url)
        method = request.get_method()
        self.calls.append((method, url.path))
        prefix = f"/repos/{REPOSITORY}"
        if url.scheme != "https" or url.netloc not in ("api.github.com", "uploads.github.com"):
            raise AssertionError("Production request left the fixed HTTPS GitHub APIs")
        if not url.path.startswith(prefix + "/"):
            raise AssertionError("Production request escaped the fixture repository")
        path = url.path[len(prefix):]
        if url.netloc == "uploads.github.com":
            if method != "POST" or path != "/releases/17/assets" or self.release is None:
                raise AssertionError("Unexpected upload operation")
            self.upload_count += 1
            if self.fault == "partial_upload" and self.upload_count == 2:
                self.error(request, 503)
            name = urllib.parse.parse_qs(url.query)["name"][0]
            if self.fault == "upload_race":
                self.release["assets"].append({"name": name, "id": 999})
                self.error(request, 422)
            if any(asset["name"] == name for asset in self.release["assets"]):
                self.error(request, 422)
            asset = {"id": self.upload_count, "name": name, "size": len(request.data),
                     "digest": "sha256:" + hashlib.sha256(request.data).hexdigest(), "state": "uploaded"}
            if self.fault == "upload_digest":
                asset["digest"] = "sha256:" + "0" * 64
            elif self.fault == "upload_size":
                asset["size"] += 1
            elif self.fault == "upload_name":
                asset["name"] = "unexpected.bin"
            elif self.fault == "upload_state":
                asset["state"] = "starter"
            self.release["assets"].append(asset)
            return self.response(asset)
        if method == "GET" and path.startswith("/collaborators/") and path.endswith("/permission"):
            actor = path.split("/")[2]
            if self.fault == "permission_unavailable":
                self.error(request, 403)
            return self.response({"role_name": self.roles[actor]})
        if method == "GET" and path == f"/git/ref/tags/v{VERSION}":
            if self.fault == "tag_lookup_denied":
                self.error(request, 403)
            if self.tag is None:
                self.error(request, 404)
            tag = copy.deepcopy(self.tag)
            if self.fault == "tag_drift" and self.release is not None:
                tag["object"]["sha"] = "b" * 40
            return self.response(tag)
        if method == "GET" and path == f"/releases/tags/v{VERSION}":
            if self.fault == "release_lookup_unavailable":
                self.error(request, 503)
            if self.release is None:
                self.error(request, 404)
            return self.response(self.release)
        if method == "POST" and path == "/git/refs":
            if self.tag is not None or self.fault == "tag_race":
                self.error(request, 422)
            data = json.loads(request.data)
            self.tag = {"ref": data["ref"], "object": {"type": "commit", "sha": data["sha"]}}
            return self.response(self.tag)
        if method == "POST" and path == "/releases":
            if self.release is not None or self.fault == "release_race":
                self.error(request, 422)
            self.release = {**json.loads(request.data), "id": 17, "assets": [],
                            "upload_url": "http://untrusted.invalid/collect{?name,label}",
                            "html_url": f"https://github.com/{REPOSITORY}/releases/tag/v{VERSION}"}
            if self.fault == "new_draft_assets":
                self.release["assets"].append({"id": 999, "name": "unexpected.bin"})
            elif self.fault == "new_draft_published":
                self.release["draft"] = False
            return self.response(self.release)
        if method == "GET" and path == "/releases/17" and self.release is not None:
            current = copy.deepcopy(self.release)
            if self.fault == "extra_asset":
                current["assets"].append({"id": 999, "name": "unexpected.bin"})
            elif self.fault == "missing_asset":
                current["assets"].pop()
            elif self.fault == "duplicate_asset":
                current["assets"][-1] = copy.deepcopy(current["assets"][0])
            elif self.fault == "replaced_asset":
                current["assets"][0]["id"] = 999
            elif self.fault == "changed_asset_digest":
                current["assets"][0]["digest"] = "sha256:" + "0" * 64
            elif self.fault == "changed_asset_size":
                current["assets"][0]["size"] += 1
            elif self.fault == "draft_published":
                current["draft"] = False
            elif self.fault == "draft_tag":
                current["tag_name"] = "v0.8.0"
            elif self.fault == "draft_source":
                current["target_commitish"] = "b" * 40
            elif self.fault == "draft_id":
                current["id"] = 999
            return self.response(current)
        if method == "PATCH" and path == "/releases/17" and self.release is not None:
            data = json.loads(request.data)
            if data != {"draft": False, "make_latest": "false"}:
                raise AssertionError("Publication attempted an unrelated release mutation")
            self.release.update(data)
            return self.response(self.release)
        raise AssertionError(f"Unexpected API operation: {method} {path}")

    def writes(self):
        return [(method, path) for method, path in self.calls if method != "GET"]


class ReleaseBoundaries(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.authorize_code, cls.publish_code = workflow_programs()

    def setUp(self):
        self.context = ExitStack()
        self.addCleanup(self.context.close)
        directory = self.context.enter_context(tempfile.TemporaryDirectory(prefix="release-boundaries-"))
        self.context.enter_context(chdir(directory))
        self.context.enter_context(patch.dict(os.environ, {
            "SOURCE": SOURCE, "VERSION": VERSION, "CONFIRMATION": f"publish v{VERSION} from {SOURCE}",
            "GITHUB_SHA": SOURCE, "WORKFLOW_SHA": SOURCE, "GITHUB_REPOSITORY": REPOSITORY,
            "GITHUB_ACTOR": "reviewer", "GITHUB_TRIGGERING_ACTOR": "rerunner", "GH_TOKEN": "fake-token",
        }, clear=True))
        self.api = FakeGitHub()
        self.context.enter_context(patch.object(urllib.request, "urlopen", self.api))
        self.context.enter_context(patch.object(socket.socket, "connect", side_effect=AssertionError("Network forbidden")))
        self.context.enter_context(patch.object(socket, "create_connection", side_effect=AssertionError("Network forbidden")))
        self.output = self.context.enter_context(redirect_stdout(io.StringIO()))
        self.paths = {}
        for target in TARGETS:
            directory = Path("staged") / target
            directory.mkdir(parents=True)
            stem = f"grill-perf-{VERSION}-{target}"
            archive = directory / f"{stem}.tar.gz"
            # Publication verifies already-smoked bytes; this fixture never runs a binary.
            archive.write_bytes(f"synthetic archive for {target}".encode())
            archive_hash = hashlib.sha256(archive.read_bytes()).hexdigest()
            checksum = directory / f"{stem}.tar.gz.sha256"
            checksum.write_text(f"{archive_hash}  {archive.name}\n")
            binary_hash = hashlib.sha256(f"synthetic binary for {target}".encode()).hexdigest()
            receipt = {"schema": "grill-perf-build-receipt-v1", "release_version": VERSION,
                       "source_commit": SOURCE, "target": target,
                       "archive": {"file": archive.name, "sha256": archive_hash},
                       "checksum": {"sha256": hashlib.sha256(checksum.read_bytes()).hexdigest()},
                       "binary": {"sha256": binary_hash}, "publication_blockers": []}
            smoke = {"schema_version": 1, "status": "passed", "release_version": VERSION,
                     "source_commit": SOURCE, "target": target, "archive_sha256": archive_hash,
                     "binary_sha256": binary_hash,
                     "runtime": {"image": IMAGE, "architecture": target.split("-")[0], "glibc": "glibc 2.39",
                                 "rust_available": False, "source_checkout_available": False}}
            receipt_path, smoke_path = directory / f"{stem}.receipt.json", directory / f"{stem}.smoke.json"
            receipt_path.write_text(json.dumps(receipt))
            smoke_path.write_text(json.dumps(smoke))
            self.paths[target] = {"archive": archive, "checksum": checksum,
                                  "receipt": receipt_path, "smoke": smoke_path}

    def authorize(self):
        exec(self.authorize_code, {"__name__": "__main__"})

    def publish(self):
        exec(self.publish_code, {"__name__": "__main__"})

    def assert_stops_before_publication(self):
        with self.assertRaises((SystemExit, urllib.error.HTTPError)):
            self.publish()
        self.assertFalse(any(method in ("PATCH", "PUT", "DELETE") for method, _ in self.api.calls))

    def test_approval_accepts_both_maintainer_roles_without_writes(self):
        self.authorize()
        self.assertEqual({path.rsplit("/", 2)[1] for _, path in self.api.calls}, {"reviewer", "rerunner"})
        self.assertEqual(self.api.writes(), [])

    def test_approval_rejects_wrong_dispatch_identity_before_api(self):
        for field, value in (("SOURCE", "abc"), ("SOURCE", "b" * 40), ("GITHUB_SHA", "b" * 40),
                             ("WORKFLOW_SHA", "b" * 40), ("VERSION", "1.0.0"),
                             ("VERSION", "0.8.0"), ("CONFIRMATION", "publish")):
            with self.subTest(field=field, value=value), patch.dict(os.environ, {field: value}):
                with self.assertRaises(SystemExit):
                    self.authorize()
                self.assertEqual(self.api.calls, [])

    def test_either_nonmaintainer_actor_refuses_approval(self):
        for actor in self.api.roles:
            for role in ("write", "read", None):
                with self.subTest(actor=actor, role=role), patch.dict(self.api.roles, {actor: role}):
                    with self.assertRaises(SystemExit):
                        self.authorize()
                    self.assertEqual(self.api.writes(), [])

    def test_permission_lookup_failure_is_not_approval(self):
        self.api.fault = "permission_unavailable"
        with self.assertRaises(urllib.error.HTTPError):
            self.authorize()
        self.assertEqual(self.api.writes(), [])

    def test_staged_validation_creates_no_tag_or_release(self):
        namespace = {"__name__": "boundary_validation"}
        exec(self.publish_code, namespace)
        assets = namespace["validate_assets"]()
        self.assertEqual(set(assets), {path for paths in self.paths.values() for path in paths.values()})
        self.assertEqual(self.api.calls, [])
        self.assertIsNone(self.api.tag)
        self.assertIsNone(self.api.release)

    def test_valid_publication_preserves_exact_bytes_and_source(self):
        self.authorize()
        self.publish()
        self.assertEqual(self.api.tag["object"]["sha"], SOURCE)
        self.assertEqual(self.api.tag["ref"], f"refs/tags/v{VERSION}")
        self.assertIs(self.api.release["draft"], False)
        self.assertEqual(self.api.release["target_commitish"], SOURCE)
        expected = {path.name: (path.stat().st_size, "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest())
                    for paths in self.paths.values() for path in paths.values()}
        actual = {asset["name"]: (asset["size"], asset["digest"]) for asset in self.api.release["assets"]}
        self.assertEqual(actual, expected)
        self.assertEqual(self.api.writes()[-1], ("PATCH", f"/repos/{REPOSITORY}/releases/17"))
        self.assertFalse(any(method in ("PUT", "DELETE") for method, _ in self.api.calls))

    def test_corrupt_archive_and_checksum_refuse_before_api(self):
        for target in TARGETS:
            for kind in ("archive", "checksum"):
                path = self.paths[target][kind]
                original = path.read_bytes()
                with self.subTest(target=target, kind=kind):
                    path.write_bytes(original + b"corruption")
                    self.assert_stops_before_publication()
                    self.assertEqual(self.api.calls, [])
                path.write_bytes(original)

    def test_stale_or_mismatched_receipt_and_smoke_refuse_before_api(self):
        mutations = {
            "receipt": (("schema", "unknown"), ("release_version", "0.8.0"), ("source_commit", "b" * 40),
                        ("target", "wrong-target"), ("archive.sha256", "0" * 64), ("archive.file", "wrong.tar.gz"),
                        ("checksum.sha256", "0" * 64), ("binary.sha256", "0" * 64),
                        ("publication_blockers", ["unreviewed payload"])),
            "smoke": (("schema_version", 99), ("status", "failed"), ("release_version", "0.8.0"),
                      ("source_commit", "b" * 40), ("target", "wrong-target"), ("archive_sha256", "0" * 64),
                      ("binary_sha256", "0" * 64), ("runtime.image", "ubuntu:latest"),
                      ("runtime.architecture", "wrong-architecture"), ("runtime.glibc", "glibc 2.40"),
                      ("runtime.rust_available", True), ("runtime.source_checkout_available", True)),
        }
        for target in TARGETS:
            for kind, cases in mutations.items():
                path = self.paths[target][kind]
                original = path.read_text()
                for field, value in cases:
                    with self.subTest(target=target, kind=kind, field=field):
                        document = json.loads(original)
                        parts = field.split(".")
                        parent = document
                        for part in parts[:-1]:
                            parent = parent[part]
                        parent[parts[-1]] = value
                        path.write_text(json.dumps(document))
                        self.assert_stops_before_publication()
                        self.assertEqual(self.api.calls, [])
                    path.write_text(original)

    def test_extra_missing_and_symlinked_artifacts_refuse_before_api(self):
        directory = self.paths[TARGETS[1]]["archive"].parent
        extra = directory / "private-receipt.json"
        extra.write_text("private fixture")
        self.assert_stops_before_publication()
        extra.unlink()
        path = self.paths[TARGETS[1]]["smoke"]
        original = path.read_bytes()
        path.unlink()
        self.assert_stops_before_publication()
        outside = Path("outside-smoke.json")
        outside.write_bytes(original)
        path.symlink_to(outside.resolve())
        self.assert_stops_before_publication()
        self.assertEqual(self.api.calls, [])

    def test_lookup_failure_is_not_resource_absence(self):
        for fault in ("tag_lookup_denied", "release_lookup_unavailable"):
            with self.subTest(fault=fault):
                self.api.fault = fault
                self.assert_stops_before_publication()
                self.assertEqual(self.api.writes(), [])

    def test_existing_tag_or_release_is_untouched(self):
        for kind in ("tag", "release"):
            with self.subTest(kind=kind):
                self.api.tag = {"ref": f"refs/tags/v{VERSION}", "object": {"sha": "b" * 40}} if kind == "tag" else None
                self.api.release = {"id": 999, "draft": False, "assets": [{"name": "existing"}]} if kind == "release" else None
                before = copy.deepcopy((self.api.tag, self.api.release))
                self.assert_stops_before_publication()
                self.assertEqual((self.api.tag, self.api.release), before)
                self.assertEqual(self.api.writes(), [])

    def test_creation_races_stop_without_retry_or_cleanup(self):
        for fault in ("tag_race", "release_race"):
            with self.subTest(fault=fault):
                self.api.tag = self.api.release = None
                self.api.calls.clear()
                self.api.fault = fault
                self.assert_stops_before_publication()
                expected = [("POST", f"/repos/{REPOSITORY}/git/refs")]
                if fault == "release_race":
                    expected.append(("POST", f"/repos/{REPOSITORY}/releases"))
                    self.assertEqual(self.api.tag["object"]["sha"], SOURCE)
                self.assertEqual(self.api.writes(), expected)
                self.assertIsNone(self.api.release)

    def test_partial_upload_leaves_draft_and_uploaded_bytes(self):
        self.api.fault = "partial_upload"
        self.assert_stops_before_publication()
        self.assertIs(self.api.release["draft"], True)
        self.assertEqual(self.api.tag["object"]["sha"], SOURCE)
        self.assertEqual([asset["name"] for asset in self.api.release["assets"]],
                         [self.paths[TARGETS[0]]["archive"].name])
        self.assertEqual(self.api.upload_count, 2)

    def test_asset_upload_race_never_clobbers_existing_slot(self):
        self.api.fault = "upload_race"
        self.assert_stops_before_publication()
        self.assertEqual(self.api.release["assets"], [{"name": self.paths[TARGETS[0]]["archive"].name, "id": 999}])
        self.assertEqual(self.api.upload_count, 1)
        self.assertIs(self.api.release["draft"], True)

    def test_changed_draft_upload_or_tag_refuses_final_publication(self):
        faults = ("new_draft_assets", "new_draft_published", "upload_digest", "upload_size", "upload_name",
                  "upload_state", "extra_asset", "missing_asset", "duplicate_asset", "replaced_asset",
                  "changed_asset_digest", "changed_asset_size", "draft_published", "draft_tag", "draft_source",
                  "draft_id", "tag_drift")
        for fault in faults:
            with self.subTest(fault=fault):
                self.api.tag = self.api.release = None
                self.api.calls.clear()
                self.api.upload_count = 0
                self.api.fault = fault
                self.assert_stops_before_publication()
                self.assertEqual(self.api.tag["object"]["sha"], SOURCE)


if __name__ == "__main__":
    unittest.main()
