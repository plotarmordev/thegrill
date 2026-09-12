#!/usr/bin/env python3
"""Stage a native, exact-source grill-perf archive; never create a tag or release."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib


TOOLCHAIN = "1.98.0"
TARGETS = {
    "x86_64-unknown-linux-gnu": "x86_64",
    "aarch64-unknown-linux-gnu": "aarch64",
}
WORKLOADS = (
    "baseline-v1.json",
    "concurrency-enable-thinking-selection-v1.json",
    "concurrency-enable-thinking-v1.json",
    "concurrency-selection-v1.json",
    "concurrency-v1.json",
    "conversation-selection-v2.json",
    "conversation-v2.json",
    "glm-decode-v1.json",
    "glm-prefill-v1.json",
    "prefill-ladder-v1.json",
    "quick.json",
    "recipe-smoke.json",
    "recipes-v1.json",
    "sparkdash-decode-v1.json",
    "sparkdash-prefill-v1.json",
)
PAYLOAD = {
    "LICENSE": "LICENSE",
    "NOTICE": "NOTICE",
    "licenses/sparkDash-LICENSE": "licenses/sparkDash-LICENSE",
    "INSTALL.md": "docs/performance/INSTALL.md",
    "licenses/NOTICE-INPUTS.json": "licenses/NOTICE-INPUTS.json",
    "licenses/THIRD-PARTY-NOTICES.txt": "licenses/THIRD-PARTY-NOTICES.txt",
    "licenses/rust-standard-library/COPYRIGHT-library.html": "licenses/rust-standard-library/COPYRIGHT-library.html",
    **{f"licenses/rust-standard-library/licenses/{name}.txt":
       f"licenses/rust-standard-library/licenses/{name}.txt"
       for name in ("MIT", "Apache-2.0", "Unicode-3.0", "BSD-2-Clause")},
    **{f"workloads/{name}": f"crates/grill-perf/examples/{name}" for name in WORKLOADS},
}


def run(args, cwd, env=None):
    return subprocess.check_output(args, cwd=cwd, env=env, text=True).strip()


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def clean_source(root, source):
    require(run(["git", "rev-parse", "HEAD"], root) == source,
            "--source must equal the full current HEAD")
    require(not run(["git", "status", "--porcelain=v1", "--untracked-files=all"], root),
            "source has tracked changes or untracked files; commit reviewed inputs first")
    # These flags can hide worktree changes from ordinary git status.
    flags = run(["git", "ls-files", "-v"], root).splitlines()
    require(all(line.startswith("H ") for line in flags),
            "source index uses unsupported skip-worktree/assume-unchanged entries")


def build_environment(root):
    allowed = {"CARGO_HOME", "CARGO_TERM_COLOR", "RUSTUP_HOME"}
    overrides = []
    for key, value in os.environ.items():
        if key in allowed:
            continue
        if key == "CARGO_INCREMENTAL" and value == "0":
            continue
        if (key.startswith(("CARGO_", "RUST", "CC_", "CXX_", "AR_", "CFLAGS_",
                            "CXXFLAGS_", "LDFLAGS_", "CMAKE_", "AWS_LC_", "BINDGEN_",
                            "PKG_CONFIG", "LD_"))
                or key in {"CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "CPPFLAGS",
                           "LDFLAGS", "LIBRARY_PATH", "CPATH", "C_INCLUDE_PATH",
                           "CPLUS_INCLUDE_PATH", "MAKEFLAGS", "SOURCE_DATE_EPOCH"}):
            overrides.append(key)
    require(not overrides, "unsupported build override variables: " + ", ".join(sorted(overrides)))
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    directories = {root, *root.parents, cargo_home.parent, Path.home()}
    configs = [cargo_home / name for name in ("config", "config.toml")]
    configs += [directory / ".cargo" / name for directory in directories
                for name in ("config", "config.toml")]
    require(not any(path.exists() or path.is_symlink() for path in configs),
            "Cargo configuration overrides are unsupported; use a clean build host")
    # Do not pass credentials or unrelated host settings to dependency build scripts.
    env = {key: os.environ[key] for key in ("HOME", "PATH", "RUSTUP_HOME", "CARGO_HOME")
           if key in os.environ}
    env.update(LANG="C.UTF-8", LC_ALL="C.UTF-8", CARGO_INCREMENTAL="0", CARGO_TERM_COLOR="never")
    return env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, help="full reviewed HEAD commit SHA")
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--out", required=True, type=Path, help="new directory outside the checkout")
    args = parser.parse_args()
    require(re.fullmatch(r"[0-9a-f]{40}", args.source), "--source must be a full lowercase commit SHA")
    root = Path(__file__).resolve().parents[1]
    require(Path(run(["git", "rev-parse", "--show-toplevel"], root)).resolve() == root,
            "staging script must belong to the source repository root")
    clean_source(root, args.source)
    env = build_environment(root)
    require(platform.system() == "Linux" and platform.machine() == TARGETS[args.target],
            "target must match this native Linux host; cross-builds and emulation are not qualification")
    release = platform.freedesktop_os_release()
    require(release.get("ID") == "ubuntu" and release.get("VERSION_ID") == "24.04",
            "build host must be Ubuntu 24.04")
    libc = run(["getconf", "GNU_LIBC_VERSION"], root, env)
    require(libc == "glibc 2.39", "build host must use the glibc 2.39 baseline")
    rustc = run(["rustc", f"+{TOOLCHAIN}", "-vV"], root, env)
    cargo = run(["cargo", f"+{TOOLCHAIN}", "--version"], root, env)
    require(f"release: {TOOLCHAIN}" in rustc.splitlines(), "unexpected Rust compiler release")
    require(f"host: {args.target}" in rustc.splitlines(), "Rust compiler host must equal native target")
    require(cargo.startswith(f"cargo {TOOLCHAIN} "), "unexpected Cargo release")
    tool_hashes = {name: digest(Path(run(["rustup", "which", "--toolchain", TOOLCHAIN, name], root, env)))
                   for name in ("rustc", "cargo")}
    native_tools = {name: run([name, "--version"], root, env).splitlines()[0]
                    for name in ("cc", "c++", "ld", "cmake")}
    tree = run(["git", "rev-parse", "HEAD^{tree}"], root)
    entries = run(["git", "ls-tree", "-r", "HEAD"], root).splitlines()
    require(all(line.startswith(("100644 blob ", "100755 blob ")) for line in entries),
            "source tree must contain only ordinary files, not symlinks or submodules")
    require(not any("\t" + path in line for line in entries
                    for path in (".cargo/config", ".cargo/config.toml")),
            "tracked Cargo build overrides are unsupported")
    out = args.out.absolute()
    require(not out.exists() and not out.is_symlink(), "--out must not already exist")
    require(out.parent.is_dir() and not out.parent.is_symlink(), "--out parent must be an existing directory")
    require(not out.resolve().is_relative_to(root), "--out must be outside the source checkout")
    epoch = int(run(["git", "show", "-s", "--format=%ct", "HEAD"], root))
    command = ["cargo", f"+{TOOLCHAIN}", "build", "--locked", "--release",
               "--target", args.target, "-p", "grill-perf", "--bin", "grill-perf"]
    # The build sees only committed files and a fresh target directory, never local leftovers.
    with tempfile.TemporaryDirectory(prefix="grill-release-") as temporary:
        work = Path(temporary)
        source = work / "source"
        source.mkdir()
        with (work / "source.tar").open("wb") as snapshot:
            subprocess.run(["git", "archive", "--format=tar", args.source], cwd=root,
                           stdout=snapshot, check=True)
        with tarfile.open(work / "source.tar") as snapshot:
            snapshot.extractall(source, filter="data")
        env = build_environment(source)
        versions = [tomllib.loads((source / path).read_text())["package"]["version"]
                    for path in ("Cargo.toml", "crates/grill-perf/Cargo.toml")]
        version = versions[0]
        require(versions[1] == version and re.fullmatch(r"0\.[0-9]+\.[0-9]+", version),
                "root and grill-perf package versions must agree on a pre-1.0 release")
        locked = tomllib.loads((source / "Cargo.lock").read_text())
        require(all(next(item["version"] for item in locked["package"] if item["name"] == name)
                    == version for name in ("grill", "grill-perf")), "Cargo.lock package versions disagree")
        lock_hash = digest(source / "Cargo.lock")
        notices = json.loads((source / "licenses/NOTICE-INPUTS.json").read_text())
        require(notices["schema_version"] == 1 and notices["rust_toolchain"] == TOOLCHAIN
                and notices["cargo_lock_sha256"] == lock_hash
                and f"commit-hash: {notices['rust_source_commit']}" in rustc.splitlines()
                and set(notices["targets"]) == set(TARGETS),
                "dependency/toolchain notice audit is stale; review notices before staging")
        require(set(notices["manifest_sha256"]) ==
                {"Cargo.toml", "crates/grill-perf/Cargo.toml", "crates/grill-sse/Cargo.toml"}
                and all(digest(source / name) == value for name, value in notices["manifest_sha256"].items()),
                "dependency feature declarations changed; review redistribution notices")
        expected_notices = {name for name in PAYLOAD if name.startswith("licenses/")
                            and name not in {"licenses/NOTICE-INPUTS.json", "licenses/sparkDash-LICENSE"}}
        require(set(notices["notice_sha256"]) == expected_notices
                and all(digest(source / name) == value for name, value in notices["notice_sha256"].items()),
                "required redistribution notice payload is missing or changed")
        files = {name: source / path for name, path in PAYLOAD.items()}
        require(all(path.is_file() and not path.is_symlink() for path in files.values()),
                "a required allowlisted release input is missing or not an ordinary file")
        source_hashes = {name: digest(path) for name, path in files.items()}
        env["CARGO_TARGET_DIR"] = str(work / "target")
        subprocess.run(command, cwd=source, env=env, check=True)
        binary = work / "target" / args.target / "release" / "grill-perf"
        require(run([str(binary), "--version"], work, env) == f"grill-perf {version}",
                "built binary version disagrees with package version")
        require(digest(source / "Cargo.lock") == lock_hash, "build changed Cargo.lock")
        require(all(digest(path) == source_hashes[name] for name, path in files.items()),
                "build changed a packaged source asset")
        clean_source(root, args.source)
        files["bin/grill-perf"] = binary
        stem = f"grill-perf-{version}-{args.target}"
        out.mkdir(mode=0o700)
        archive = out / f"{stem}.tar.gz"
        payload = {}
        with archive.open("xb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as tar:
                    for name, path in sorted(files.items()):
                        info = tarfile.TarInfo(f"{stem}/{name}")
                        info.size = path.stat().st_size
                        info.mode = 0o755 if name == "bin/grill-perf" else 0o644
                        info.mtime = epoch
                        with path.open("rb") as content:
                            tar.addfile(info, content)
                        payload[name] = {"sha256": digest(path), "bytes": info.size, "mode": oct(info.mode)}
                        if name in PAYLOAD:
                            payload[name]["source_path"] = PAYLOAD[name]
        archive_hash = digest(archive)
        checksum = out / f"{archive.name}.sha256"
        checksum.write_text(f"{archive_hash}  {archive.name}\n")
        receipt = {
            "schema": "grill-perf-build-receipt-v1", "release_version": version, "target": args.target,
            "source_commit": args.source, "source_tree": tree, "cargo_lock_sha256": lock_hash,
            "toolchain": {"rust": TOOLCHAIN, "rustc_verbose": rustc, "cargo_version": cargo,
                          "executable_sha256": tool_hashes, "native_tools": native_tools},
            "build": {"command": command, "profile": "release", "incremental": False,
                      "fresh_target_directory": True, "build_overrides": False},
            "runtime": {"os": "Ubuntu", "version": "24.04", "libc": libc,
                        "architecture": platform.machine(), "native_build": True,
                        "installed_smoke": "required separately; not attested by this receipt"},
            "archive": {"file": archive.name, "sha256": archive_hash, "bytes": archive.stat().st_size},
            "checksum": {"file": checksum.name, "sha256": digest(checksum)},
            "binary": payload["bin/grill-perf"], "payload": payload,
            "files_sha256": {name: item["sha256"] for name, item in payload.items()},
            "publication_blockers": [],
            "redistribution_notices": {"input_ledger_sha256": digest(source / "licenses/NOTICE-INPUTS.json"),
                                       "scope": "pinned Cargo/native and Rust runtime notices; not a legal attestation"},
            "packaging": {"mtime": epoch, "uid": 0, "gid": 0,
                          "bit_reproducible_build": "not claimed"},
        }
        (out / f"{stem}.receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"archive": archive.name, "version": version, "target": args.target,
                          "sha256": archive_hash}, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError, tarfile.TarError) as error:
        sys.exit(f"stage-release: {error}")
