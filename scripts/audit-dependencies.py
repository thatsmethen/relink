#!/usr/bin/env python3
"""Audit Cargo.lock without suppressions, using a hash-pinned RustSec release.

Requires Python 3.11+, git, and cargo. No tool is installed globally and no
production dependency is changed. Exit 1 means advisories/warnings were found;
exit 2 means the audit or its provenance checks failed. See --help.
"""

import argparse
import datetime
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[1]
PINS = Path(__file__).with_name("audit-dependencies-pins.json")
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def run(command, cwd, *, check=True):
    result = subprocess.run(
        command, cwd=cwd, capture_output=True, text=True,
        encoding="utf-8", errors="strict", timeout=180,
    )
    if check and result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {command}\n{result.stderr}")
    return result


def host_target():
    machine = platform.machine().lower()
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "amd64": "x86_64", "x86_64": "x86_64"}.get(machine)
    suffix = {"Darwin": "apple-darwin", "Windows": "pc-windows-msvc", "Linux": "unknown-linux-gnu"}.get(platform.system())
    if not arch or not suffix:
        raise ValueError(f"no pinned audit binary for {platform.system()}/{machine}")
    return f"{arch}-{suffix}"


def acquire_tool(pins, cache):
    target = host_target()
    asset = pins["assets"][target]
    archive = cache / asset["filename"]
    if not archive.exists():
        request = urllib.request.Request(asset["url"], headers={"User-Agent": "ReLink-dependency-audit"})
        with urllib.request.urlopen(request, timeout=60) as response:
            data = response.read(32 * 1024 * 1024 + 1)
        if len(data) > 32 * 1024 * 1024:
            raise ValueError("audit tool archive exceeds 32 MiB limit")
        if digest(data) != asset["sha256"]:
            raise ValueError("downloaded audit tool archive checksum mismatch")
        archive.write_bytes(data)
    # Verify cached bytes as well, every time and before parsing or execution.
    if digest(archive.read_bytes()) != asset["sha256"]:
        raise ValueError("cached audit tool archive checksum mismatch")
    executable_name = "cargo-audit.exe" if target.endswith("windows-msvc") else "cargo-audit"
    # Copy only the executable by basename; never extract archive paths/symlinks.
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as bundle:
            members = [name for name in bundle.namelist() if Path(name).name == executable_name]
            if len(members) != 1:
                raise ValueError("expected exactly one audit executable")
            binary = bundle.read(members[0])
    else:
        with tarfile.open(archive) as bundle:
            members = [member for member in bundle.getmembers() if member.isfile() and Path(member.name).name == executable_name]
            if len(members) != 1:
                raise ValueError("expected exactly one audit executable")
            with bundle.extractfile(members[0]) as source:
                binary = source.read()
    executable = cache / executable_name
    executable.write_bytes(binary)
    executable.chmod(0o700)
    actual_version = run([str(executable), "--version"], cache).stdout.strip()
    if actual_version != f"cargo-audit {pins['version']}":
        raise ValueError(f"unexpected audit tool version: {actual_version}")
    return executable, {"target": target, "asset": asset, "binary_sha256": digest(binary), "version_output": actual_version}


def audit(args):
    pins = json.loads(PINS.read_text(encoding="utf-8"))
    revision = args.database_revision or pins["database"]["commit"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("database revision must be a full lowercase 40-character Git commit")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    cache = args.cache.resolve()
    cache.mkdir(parents=True, exist_ok=True)
    lock = (ROOT / "Cargo.lock").read_bytes()
    (out / "Cargo.lock.snapshot").write_bytes(lock)
    manifest_paths = [ROOT / "Cargo.toml", *sorted((ROOT / "crates").glob("*/Cargo.toml"))]
    manifests = {str(path.relative_to(ROOT)): digest(path.read_bytes()) for path in manifest_paths}
    input_record = {
        "captured_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "repository_head": run(["git", "rev-parse", "HEAD"], ROOT).stdout.strip(),
        "lockfile_sha256": digest(lock),
        "manifest_sha256": manifests,
        "cargo_version": run(["cargo", "--version"], ROOT).stdout.strip(),
        "rustc_version": run(["rustc", "--version"], ROOT).stdout.strip(),
        "script_sha256": digest(Path(__file__).read_bytes()),
        "pins_sha256": digest(PINS.read_bytes()),
    }
    write_json(out / "input.json", input_record)
    executable, tool = acquire_tool(pins, cache)
    write_json(out / "tool.json", tool)
    with tempfile.TemporaryDirectory(prefix="relink-dependency-audit-") as temporary:
        work = Path(temporary)
        database = work / "advisory-db"
        run(["git", "init", "-q", str(database)], work)
        hooks = work / "empty-hooks"
        hooks.mkdir()
        git = ["git", "-C", str(database), "-c", "credential.helper=", "-c", f"core.hooksPath={hooks}"]
        run([*git, "fetch", "--depth=1", pins["database"]["repository"], revision], work)
        run([*git, "checkout", "-q", "--detach", "FETCH_HEAD"], work)
        actual = run([*git, "rev-parse", "HEAD"], work).stdout.strip()
        if actual != revision:
            raise ValueError("fetched database revision mismatch")
        tree = run([*git, "rev-parse", "HEAD^{tree}"], work).stdout.strip()
        if revision == pins["database"]["commit"] and tree != pins["database"]["tree"]:
            raise ValueError("fetched database tree mismatch")
        write_json(out / "database.json", {
            "repository": pins["database"]["repository"], "commit": actual, "tree": tree,
            "commit_date": run([*git, "show", "-s", "--format=%cI", "HEAD"], work).stdout.strip(),
        })
        # cargo-audit 0.22.2 prefers cwd/.cargo/audit.toml over the user's config.
        # This explicit empty configuration prevents inherited ignore/target rules.
        (work / ".cargo").mkdir()
        (work / ".cargo/audit.toml").write_text("# No suppressions or target filters.\n", encoding="utf-8")
        command = [str(executable), "audit", "--file", str(out / "Cargo.lock.snapshot"),
                   "--db", str(database), "--no-fetch", "--deny", "warnings", "--json"]
        result = run(command, work, check=False)
        (out / "audit-full.json").write_text(result.stdout, encoding="utf-8")
        (out / "audit-full.stderr").write_text(result.stderr, encoding="utf-8")
        write_json(out / "audit-command.json", {"command": command, "exit_code": result.returncode,
                   "working_directory": "temporary directory with explicit empty .cargo/audit.toml",
                   "yanked_check": "cached index only: --no-fetch also prevents crates.io index refresh in cargo-audit 0.22.2"})
        report = json.loads(result.stdout)
        if result.returncode not in (0, 1) or report["settings"]["ignore"] or report["settings"]["target_os"] or report["settings"]["target_arch"]:
            raise ValueError("audit failed or unexpectedly used suppression/target rules")
        findings = list(report["vulnerabilities"]["list"])
        for warnings in report["warnings"].values():
            findings.extend(warnings)
        records = []
        graphs = {}
        for target in TARGETS:
            for label, edges, package in (("app-all", "normal,build", "relink-app"),
                                          ("app-runtime", "normal,no-proc-macro", "relink-app"),
                                          ("hal-all", "normal,build", "relink-hal")):
                cmd = ["cargo", "tree", "--locked", "-p", package, "--target", target,
                       "--edges", edges, "--prefix", "none", "--format", "{p}"]
                graph = run(cmd, ROOT)
                filename = f"{target}-{label}.txt"
                (out / filename).write_text(graph.stdout, encoding="utf-8")
                records.append({"command": cmd, "stdout": filename, "stderr": graph.stderr})
                graphs[(target, label)] = set(re.findall(r"^([\w-]+) v([^\s]+)", graph.stdout, re.MULTILINE))
        matrix = []
        for finding in findings:
            pkg = finding["package"]
            key = (pkg["name"], pkg["version"])
            matrix.append({"package": pkg["name"], "version": pkg["version"],
                           "advisory": (finding.get("advisory") or {}).get("id"), "kind": finding.get("kind", "vulnerability"),
                           "targets": {target: {label: key in graphs[(target, label)] for label in ("app-all", "app-runtime", "hal-all")} for target in TARGETS}})
        write_json(out / "graph-commands.json", records)
        write_json(out / "target-membership.json", matrix)
    if digest((ROOT / "Cargo.lock").read_bytes()) != input_record["lockfile_sha256"] or any(digest((ROOT / name).read_bytes()) != value for name, value in manifests.items()):
        raise ValueError("workspace manifest/lockfile changed during audit; graph and scan cannot be combined")
    warning_count = sum(len(items) for items in report["warnings"].values())
    summary = {"vulnerability_count": report["vulnerabilities"]["count"], "warning_count": warning_count,
               "audit_exit_code": result.returncode, "database_commit": revision,
               "limitations": ["Package graph membership is not function-level reachability or exploit analysis.",
                               "Yanked status uses the host's cached crates.io index, not a fresh index snapshot.",
                               "Rust standard/core/compiler builtins, SDKs and system libraries are outside Cargo.lock."]}
    write_json(out / "summary.json", summary)
    write_json(out / "SHA256SUMS.json", {str(path.relative_to(out)): digest(path.read_bytes()) for path in sorted(out.rglob("*")) if path.is_file()})
    print(json.dumps(summary, indent=2))
    print(f"Evidence: {out}")
    return result.returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new evidence directory (refuses overwrite)")
    parser.add_argument("--cache", type=Path, default=Path(tempfile.gettempdir()) / "relink-audit-tools", help="verified download cache; does not install tools")
    parser.add_argument("--database-revision", help="explicit full RustSec commit; default is the reviewed pin, not the latest DB")
    args = parser.parse_args()
    try:
        return audit(args)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Dependency audit failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
