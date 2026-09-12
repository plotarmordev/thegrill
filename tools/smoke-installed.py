#!/usr/bin/env python3
"""Maintainer-only CPU protocol smoke; never a model runner or installer."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import threading
import uuid
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

IMAGE = "ubuntu@sha256:224a1869083a311ef3f13648a154ba79832fbef6364d31493642ca03082da254"
TARGETS = {"x86_64-unknown-linux-gnu": "x86_64", "aarch64-unknown-linux-gnu": "aarch64"}
TOKEN = "installed-cpu-fixture-token"
WORKLOADS = {
    "baseline-v1.json", "glm-decode-v1.json", "glm-prefill-v1.json",
    "prefill-ladder-v1.json", "quick.json", "recipe-smoke.json", "recipes-v1.json",
    "sparkdash-decode-v1.json", "sparkdash-prefill-v1.json",
    "concurrency-v1.json", "concurrency-selection-v1.json",
    "concurrency-enable-thinking-v1.json", "concurrency-enable-thinking-selection-v1.json",
    "conversation-v2.json", "conversation-selection-v2.json",
}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify_checksum(archive, checksum):
    parts = checksum.read_text().split()
    require(len(parts) == 2 and parts[1].lstrip("*") == archive.name,
            "checksum must name the exact selected archive")
    require(len(parts[0]) == 64 and digest(archive) == parts[0], "archive checksum mismatch")
    return parts[0]


def unpack(archive, destination, root_name, receipt):
    expected = receipt["files_sha256"]
    required = {"bin/grill-perf", "LICENSE", "NOTICE", "INSTALL.md", "licenses/sparkDash-LICENSE"}
    required.update("workloads/" + name for name in WORKLOADS)
    require(required <= set(expected), "archive receipt omits required assets/notices")
    require(all(name in required or name.startswith("licenses/") for name in expected),
            "unexpected payload outside the bounded archive layout")
    files = set()
    with tarfile.open(archive, "r:gz") as contents:
        members = contents.getmembers()
        require(len(members) <= 1024 and sum(m.size for m in members) <= 256 * 1024 * 1024,
                "archive exceeds smoke admission bounds")
        for member in members:
            path = Path(member.name)
            require(not path.is_absolute() and ".." not in path.parts and path.parts
                    and path.parts[0] == root_name, "unsafe or wrong archive root")
            require(member.isdir() or member.isfile(), "archive links/devices are not permitted")
            if member.isfile():
                relative = str(Path(*path.parts[1:]))
                require(relative not in files and relative in expected, "duplicate/unexpected archive member")
                files.add(relative)
        require(files == set(expected), "archive members differ from the receipt")
        contents.extractall(destination, filter="data")
    root = destination / root_name
    for relative, expected_hash in expected.items():
        require(digest(root / relative) == expected_hash, "installed payload digest mismatch: " + relative)
    return root


class Fixture:
    def __init__(self, model, control, token):
        self.model, self.control, self.token = model, control, token
        self.delay = 0.02
        self.reject = False
        self.requests = []
        self.errors = []
        self.lock = threading.Lock()
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    size = int(self.headers["Content-Length"])
                    require(0 < size <= 2 * 1024 * 1024, "fixture request bound")
                    body = json.loads(self.rfile.read(size))
                    with fixture.lock:
                        fixture.requests.append(body)
                        delay, reject = fixture.delay, fixture.reject
                    if fixture.token and self.headers.get("Authorization") != "Bearer " + fixture.token:
                        self.send_error(401, "fixture authentication required")
                        return
                    if reject:
                        self.send_error(400, "fixture rejects declared control")
                        return
                    require(body["model"] == fixture.model, "model declaration not preserved")
                    require(body["chat_template_kwargs"] == {fixture.control: False}, "control spelling changed")
                    require(body["stream"] and body["stream_options"] == {"include_usage": True},
                            "fixture requires the declared streaming profile")
                    tokens = body["max_tokens"]
                    require(tokens in (64, 400) and body["min_tokens"] == tokens and body["ignore_eos"],
                            "fixture output controls changed")
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    time.sleep(delay)
                    first = {"choices": [{"index": 0, "delta": {"content": "1 2"}}]}
                    final = {"choices": [{"index": 0, "delta": {"content": " 3 4"}, "finish_reason": "length"}]}
                    usage = {"choices": [], "usage": {"prompt_tokens": 32, "completion_tokens": tokens}}
                    self.wfile.write(("data: " + json.dumps(first) + "\n\n").encode())
                    self.wfile.flush()
                    time.sleep(0.002)
                    self.wfile.write(("data: " + json.dumps(final) + "\n\ndata: "
                                      + json.dumps(usage) + "\n\ndata: [DONE]\n\n").encode())
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    # Expected when a deliberately exhausted capture cancels an admitted request.
                    pass
                except Exception as error:
                    with fixture.lock:
                        fixture.errors.append(str(error))

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.endpoint = f"http://127.0.0.1:{self.server.server_port}/v1/chat/completions"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        require(not self.errors, "CPU fixture rejected unexpected request semantics: " + repr(self.errors))


class Installed:
    def __init__(self, root, evidence):
        self.root, self.evidence = root, evidence
        self.index = 0
        self.base = ["docker", "run", "--rm", "--read-only",
                     "--cap-drop=ALL", "--security-opt=no-new-privileges",
                     "--user", f"{os.getuid()}:{os.getgid()}",
                     "--tmpfs", "/tmp:rw,noexec,nosuid,size=64m",
                     "--mount", f"type=bind,src={root},dst=/opt/grill,readonly",
                     "--mount", f"type=bind,src={evidence},dst=/evidence",
                     "--workdir", "/tmp"]

    def raw(self, arguments, token=None, network=True):
        owner = uuid.uuid4().hex
        name = "grill-install-smoke-" + owner
        command = self.base + ["--network", "host" if network else "none",
                               "--name", name, "--label", "org.thegrill.install-smoke=" + owner]
        command += ["--env", "GRILL_INSTALL_TOKEN=" + token] if token else []
        try:
            return subprocess.run(command + [IMAGE] + arguments, capture_output=True, text=True, timeout=150)
        finally:
            inspected = subprocess.run(["docker", "inspect", "--format",
                '{{index .Config.Labels "org.thegrill.install-smoke"}}', name],
                capture_output=True, text=True, timeout=30)
            if inspected.returncode == 0 and inspected.stdout.strip() == owner:
                subprocess.run(["docker", "rm", "--force", name], check=True, capture_output=True, timeout=30)

    def run(self, arguments, token=None, network=True):
        result = self.raw(["/opt/grill/bin/grill-perf"] + arguments, token, network)
        self.index += 1
        (self.evidence / f"command-{self.index:02}.stdout").write_text(result.stdout)
        (self.evidence / f"command-{self.index:02}.stderr").write_text(result.stderr)
        return result

    def report(self, name):
        return json.loads((self.evidence / name / "report.json").read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--checksum", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", type=Path, required=True)
    options = parser.parse_args()
    require(platform.system() == "Linux" and platform.machine() == TARGETS[options.target],
            "native target mismatch: no emulation/cross-execution qualification")
    root_name = f"grill-perf-{options.version}-{options.target}"
    require(options.archive.name == root_name + ".tar.gz", "archive version/target filename mismatch")
    archive_hash = verify_checksum(options.archive, options.checksum)
    receipt = json.loads(options.receipt.read_text())
    require(receipt["schema"] == "grill-perf-build-receipt-v1"
            and receipt["release_version"] == options.version and receipt["target"] == options.target
            and receipt["archive"]["sha256"] == archive_hash, "build receipt identity mismatch")
    options.out.mkdir(mode=0o700, parents=False, exist_ok=False)
    evidence = options.out.resolve() / "private"
    evidence.mkdir(mode=0o700)
    extraction = options.out.resolve() / "extracted"
    extraction.mkdir()
    root = unpack(options.archive, extraction, root_name, receipt)
    binary_hash = digest(root / "bin/grill-perf")
    require(binary_hash == receipt["binary"]["sha256"], "installed binary hash mismatch")
    with (root / "bin/grill-perf").open("rb") as binary:
        elf = binary.read(20)
    expected_machine = 62 if options.target == "x86_64-unknown-linux-gnu" else 183
    require(elf[:6] == b"\x7fELF\x02\x01" and int.from_bytes(elf[18:20], "little") == expected_machine,
            "binary ELF architecture differs from the declared native target")
    corrupted = options.out / options.archive.name
    shutil.copyfile(options.archive, corrupted)
    with corrupted.open("r+b") as changed:
        first = changed.read(1)
        changed.seek(0)
        changed.write(bytes([first[0] ^ 1]))
    try:
        verify_checksum(corrupted, options.checksum)
    except RuntimeError:
        pass
    else:
        raise RuntimeError("corrupted download was not rejected before execution")
    corrupted.unlink()
    subprocess.run(["docker", "pull", IMAGE], check=True, capture_output=True, text=True, timeout=300)
    installed = Installed(root, evidence)
    clean = installed.raw(["/bin/sh", "-ec", "if command -v rustc || command -v cargo; then exit 1; fi; test ! -e /opt/grill/Cargo.toml; test ! -e /workspace; uname -m; getconf GNU_LIBC_VERSION"], network=False)
    require(clean.returncode == 0, "clean runtime/build-tool isolation check failed")
    runtime_lines = clean.stdout.strip().splitlines()
    require(runtime_lines == [TARGETS[options.target], "glibc 2.39"], "unexpected native runtime/libc baseline")
    version_result = installed.run(["--version"], network=False)
    require(version_result.returncode == 0 and version_result.stdout.strip() == "grill-perf " + options.version,
            "installed binary version mismatch")
    require(installed.run(["--help"], network=False).returncode == 0, "installed help failed")
    require(installed.run(["bundle", "verify", "/opt/grill/workloads/recipes-v1.json", "--json"], network=False).returncode == 0,
            "installed historical bundle verification failed")
    require(installed.run(["bundle", "inspect", "/opt/grill/workloads/concurrency-v1.json"], network=False).returncode == 0,
            "installed workload inspection failed")
    scenarios = []
    checks = ["checksum_before_execution", "corrupt_download_rejected", "payload_hashes",
              "native_clean_runtime", "version", "help", "bundle_verify", "bundle_inspect"]
    for name, control, token, selection in [
        ("neutral-alpha", "thinking", None, None),
        ("neutral-beta", "enable_thinking", TOKEN, "concurrency-enable-thinking-selection-v1.json"),
    ]:
        fixture = Fixture(name, control, token)
        try:
            declaration = {"model_revision": "cpu-fixture-weights", "runtime": "cpu-fixture-runtime",
                           "hardware": "cpu-loopback", "settings": "before"}
            before_file = name + "-before.json"
            after_file = name + "-after.json"
            (evidence / before_file).write_text(json.dumps(declaration))
            declaration["settings"] = "after"
            (evidence / after_file).write_text(json.dumps(declaration))
            def baseline_args(output):
                arguments = ["baseline", "--endpoint", fixture.endpoint, "--model", name,
                             "--deployment", "/evidence/" + before_file, "--out", "/evidence/" + output,
                             "--local-http", "--seconds", "60"]
                if selection:
                    arguments += ["--selection", "/opt/grill/workloads/" + selection]
                if token:
                    arguments += ["--auth-env", "GRILL_INSTALL_TOKEN"]
                return arguments
            base_name, control_name, candidate_name = [name + suffix for suffix in ("-baseline", "-control", "-candidate")]
            before = installed.run(baseline_args(base_name), token)
            require(before.returncode == 0 and installed.report(base_name)["baseline_ready"], "installed baseline failed")
            common = ["check", "/evidence/" + base_name, "--seconds", "60", "--out"]
            control_run = installed.run(common + ["/evidence/" + control_name, "--deployment", "/evidence/" + before_file, "--change", "none"], token)
            fixture.delay = 0.002
            candidate = installed.run(common + ["/evidence/" + candidate_name, "--deployment", "/evidence/" + after_file, "--change", "settings"], token)
            control_report, candidate_report = installed.report(control_name), installed.report(candidate_name)
            allowed = {"DESCRIPTIVE"} if selection else {"IMPROVED", "REGRESSED", "INCONCLUSIVE"}
            require(control_report["result"] in allowed and candidate_report["result"] in allowed,
                    "installed comparison changed qualification/result scope")
            labels = {"DESCRIPTIVE": "COMPLETE - DESCRIPTIVE ONLY", "IMPROVED": "MEASURED FASTER",
                      "REGRESSED": "MEASURED SLOWER", "INCONCLUSIVE": "INCONCLUSIVE"}
            for output, report in [(control_run, control_report), (candidate, candidate_report)]:
                require(labels[report["result"]] in output.stdout.splitlines()[0], "terminal result not distinguishable")
                expected_exit = 0 if report["result"] in {"DESCRIPTIVE", "IMPROVED"} else 2
                require(output.returncode == expected_exit, "result/exit mismatch")
            planned = 224 if selection else 32
            require(len(fixture.requests) == planned * 3, "unexpected retry/replacement/request inventory")
            count = len(fixture.requests)
            # The same CLI, not a wrapper, diagnoses representative user mistakes before traffic.
            missing = installed.run(baseline_args(name + "-missing/child"), token)
            require(missing.returncode == 1 and len(fixture.requests) == count
                    and "parent" in missing.stderr and not (evidence / (name + "-missing")).exists(),
                    "missing parent did not fail before dispatch with parent guidance")
            bad_decl = dict(declaration)
            bad_decl.pop("hardware")
            (evidence / (name + "-missing-field.json")).write_text(json.dumps(bad_decl))
            args = baseline_args(name + "-missing-field")
            args[args.index("--deployment") + 1] = "/evidence/" + name + "-missing-field.json"
            invalid = installed.run(args, token)
            require(invalid.returncode == 1 and installed.report(name + "-missing-field")["result"] == "INVALID"
                    and len(fixture.requests) == count, "missing declaration was guessed or dispatched")
            require(any("hardware" in reason for reason in installed.report(name + "-missing-field")["reasons"]),
                    "missing declaration diagnostic does not identify the required input")
            bad_decl = dict(declaration, hardware="changed-too")
            (evidence / (name + "-incompatible.json")).write_text(json.dumps(bad_decl))
            invalid = installed.run(common + ["/evidence/" + name + "-incompatible", "--deployment",
                                              "/evidence/" + name + "-incompatible.json", "--change", "settings"], token)
            require(invalid.returncode == 1 and len(fixture.requests) == count, "incompatible baseline dispatched")
            if selection:
                tampered = evidence / "tampered-workloads"
                shutil.copytree(root / "workloads", tampered)
                with (tampered / "concurrency-enable-thinking-v1.json").open("ab") as changed:
                    changed.write(b"\n")
                args = baseline_args(name + "-bad-pin")
                args[args.index("--selection") + 1] = "/evidence/tampered-workloads/" + selection
                invalid = installed.run(args, token)
                require(invalid.returncode == 1 and len(fixture.requests) == count, "pin drift dispatched")
                unsupported = json.loads((root / "workloads/concurrency-enable-thinking-v1.json").read_text())
                unsupported["request"]["profile"] = "portable-chat-v1"
                unsupported_path = tampered / "concurrency-enable-thinking-v1.json"
                unsupported_path.write_text(json.dumps(unsupported))
                unsupported_selection = json.loads((tampered / selection).read_text())
                unsupported_selection["source_sha256"] = digest(unsupported_path)
                (tampered / selection).write_text(json.dumps(unsupported_selection))
                args = baseline_args(name + "-unsupported-profile")
                args[args.index("--selection") + 1] = "/evidence/tampered-workloads/" + selection
                invalid = installed.run(args, token)
                require(invalid.returncode == 1 and len(fixture.requests) == count
                        and any("profile" in reason for reason in installed.report(name + "-unsupported-profile")["reasons"]),
                        "unsupported locally declared controls were not diagnosed before dispatch")
                missing_auth = installed.run(baseline_args(name + "-missing-auth"))
                require(missing_auth.returncode == 1 and len(fixture.requests) == count, "missing credential dispatched")
                require(any("GRILL_INSTALL_TOKEN" in reason for reason in installed.report(name + "-missing-auth")["reasons"]),
                        "missing authentication diagnostic does not identify the named variable")
                rejected_auth = installed.run(baseline_args(name + "-bad-auth"), "incorrect-fixture-token")
                require(rejected_auth.returncode == 1 and len(fixture.requests) == count + 1, "authentication failure retried")
                failure = installed.report(name + "-bad-auth")["first_failure"]
                require(failure["http_status"] == 401, "authentication status evidence missing")
                response = evidence / Path(failure["response_path"]).relative_to("/evidence")
                require(b"fixture authentication required" in response.read_bytes(), "raw authentication failure evidence missing")
                count = len(fixture.requests)
                fixture.reject = True
                rejected_control = installed.run(baseline_args(name + "-rejected-control"), token)
                require(rejected_control.returncode == 1 and len(fixture.requests) == count + 1, "backend rejection retried/stripped")
                failure = installed.report(name + "-rejected-control")["first_failure"]
                require(failure["http_status"] == 400, "backend rejection status evidence missing")
                response = evidence / Path(failure["response_path"]).relative_to("/evidence")
                require(b"fixture rejects declared control" in response.read_bytes(), "raw backend rejection evidence missing")
                fixture.reject = False
                count = len(fixture.requests)
            fixture.delay = 2.0
            args = baseline_args(name + "-budget")
            args[args.index("--seconds") + 1] = "1"
            budget = installed.run(args, token)
            budget_report = installed.report(name + "-budget")
            require(budget.returncode == 2 and budget_report["result"] == "INCONCLUSIVE"
                    and not budget_report["baseline_ready"] and len(fixture.requests) <= count + 1,
                    "budget exhaustion became server failure or replacement")
            scenarios.append({"id": name, "baseline_result": "ready", "control_result": control_report["result"],
                              "candidate_result": candidate_report["result"], "request_count": len(fixture.requests)})
        finally:
            fixture.close()
        # Server is shut down and credentials are omitted: replay must be strictly offline.
        replay = installed.run(["compare", "/evidence/" + base_name, "/evidence/" + candidate_name, "--json"], network=False)
        require(replay.returncode == candidate.returncode and json.loads(replay.stdout) == candidate_report,
                "offline replay differs or needs server/credential")
    for path in evidence.rglob("*"):
        if path.is_file():
            contents = path.read_bytes()
            require(all(value.encode() not in contents for value in [TOKEN, "incorrect-fixture-token"]),
                    "credential leaked into retained evidence")
    checks += ["two_neutral_capabilities", "baseline_control_candidate", "inherited_inputs", "offline_replay",
               "terminal_labels", "missing_parent", "missing_declaration", "incompatible_baseline",
               "pin_drift", "unsupported_local_controls", "missing_credential", "authentication_rejection", "backend_rejection",
               "budget_exhaustion", "no_credential_disclosure"]
    summary = {"schema_version": 1, "status": "passed", "release_version": options.version,
               "source_commit": receipt["source_commit"], "target": options.target,
               "archive_sha256": archive_hash, "binary_sha256": binary_hash,
               "runtime": {"image": IMAGE, "architecture": runtime_lines[0], "glibc": runtime_lines[1],
                           "rust_available": False, "source_checkout_available": False},
               "checks": checks, "scenarios": scenarios,
               "claim": "native clean installed CPU protocol verification, not model/backend qualification"}
    (options.out / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))


if __name__ == "__main__":
    main()
