#!/usr/bin/env python3
"""Prepare offline; observe only after explicit system-audio recording consent.

Never installs a driver, restarts a service, selects a default, or changes TCC.
Only this runner's child processes are inspected or signalled. The run operation
is deliberately separate from prepare, and may prompt for system recording.
"""
from __future__ import annotations
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import time

import analyze_application
from application_protocol import (PROTOCOL, SELECTED_ID, EXCLUDED_ID, PROBE_ID,
    SELECTED_BUNDLE, EXCLUDED_BUNDLE, PROBE_BUNDLE, CABLE1, CABLE2, WRITERS,
    validate_owned_identities, SOURCE_SECONDS, RECORD_SECONDS, STARTUP_SECONDS)
import run_system as system

ROOT = system.REPOSITORY


def save_new(path: Path, value: dict) -> None:
    with path.open("x") as file:
        json.dump(value, file, indent=2, allow_nan=False)
        file.write("\n")


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def inventory(directory: Path, exclude=()) -> dict:
    result = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError("fixture/evidence must not contain symlinks")
        if path.is_file() and path.relative_to(directory).as_posix() not in exclude:
            result[path.relative_to(directory).as_posix()] = {"bytes": path.stat().st_size, "sha256": digest(path)}
    return result


def executable(bundle: Path) -> Path:
    return bundle / "Contents/MacOS" / bundle.stem


def command(args: list[str], log: Path) -> None:
    result = system.bounded(args, log, timeout=90)
    if result.returncode:
        raise RuntimeError(f"preparation command failed: {args[0]}; see {log}")


def make_bundle(parent: Path, name: str, bundle_id: str, binary: Path) -> Path:
    bundle = parent / name
    target = executable(bundle)
    target.parent.mkdir(parents=True)
    shutil.copyfile(binary, target)
    target.chmod(0o755)
    info = {"CFBundleExecutable": bundle.stem, "CFBundleIdentifier": bundle_id,
            "CFBundleName": bundle.stem, "CFBundlePackageType": "APPL",
            "CFBundleVersion": "1", "CFBundleShortVersionString": "1.0",
            "LSMinimumSystemVersion": "14.2", "LSBackgroundOnly": True}
    if bundle_id == PROBE_ID:
        info["NSAudioCaptureUsageDescription"] = "Record only the owned synthetic ReLink fixture apps to verify application isolation."
    (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(info, sort_keys=True))
    command(["/usr/bin/codesign", "--force", "--sign", "-", str(bundle)], parent / f"{bundle.stem}-sign.log")
    command(["/usr/bin/codesign", "--verify", "--strict", str(bundle)], parent / f"{bundle.stem}-verify.log")
    return bundle


def prepare(output: Path, runtime: Path) -> dict:
    if sys.platform != "darwin":
        raise RuntimeError("fixture compilation requires the macOS SDK")
    runtime = runtime.resolve(strict=True)
    if not runtime.is_file():
        raise ValueError("runtime must be a prebuilt runtime-application-probe executable")
    # Refuse existing output (including symlinks); no overwrite/retry in place.
    output.mkdir(parents=True, exist_ok=False)
    output = output.resolve(strict=True)
    source_paths = [ROOT / "Cargo.lock", ROOT / "Cargo.toml", ROOT / "rust-toolchain.toml"]
    for subtree in ("crates/relink-audio", "crates/relink-core", "drivers/macos/tests"):
        source_paths += [p for p in (ROOT / subtree).rglob("*") if p.is_file()
                         and "__pycache__" not in p.parts and p.suffix != ".pyc"]
    before = {str(p.relative_to(ROOT)): digest(p) for p in sorted(source_paths)}
    probe = output / "system-probe"
    command(["/usr/bin/xcrun", "clang", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
             str(ROOT / "drivers/macos/tests/system-probe.c"), "-framework", "CoreAudio",
             "-framework", "CoreFoundation", "-o", str(probe)], output / "compile-probe.log")
    fixtures = output / "fixtures"
    fixtures.mkdir()
    for name, identity, source in ((SELECTED_BUNDLE, SELECTED_ID, probe),
                                   (EXCLUDED_BUNDLE, EXCLUDED_ID, probe),
                                   (PROBE_BUNDLE, PROBE_ID, runtime)):
        make_bundle(fixtures, name, identity, source)
    after = {str(p.relative_to(ROOT)): digest(p) for p in sorted(source_paths)}
    if before != after:
        raise RuntimeError("source changed during fixture preparation; retain failed directory and prepare afresh")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    modified = subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT, text=True)
    metadata = {"schema": 1, "protocol": PROTOCOL, "prepared_directory": str(output),
        "source_revision": revision, "source_modified": bool(modified), "source_sha256": before,
        "supplied_runtime": {"path": str(runtime), "sha256": digest(runtime)},
        "runtime_source_association": "Caller-supplied prebuilt binary; attach its build provenance separately. Preparing records current sources, not proof they produced that binary.",
        "selected_bundle": str(fixtures / SELECTED_BUNDLE), "files": inventory(output),
        "execution": "No fixture executable or audio API was run by prepare; SDK compilation and ad-hoc signing only."}
    save_new(output / "prepared.json", metadata)
    return metadata


def verify_prepared(directory: Path) -> dict:
    directory = directory.resolve(strict=True)
    data = json.loads((directory / "prepared.json").read_text())
    if (data.get("protocol") != PROTOCOL or data.get("prepared_directory") != str(directory)
            or data.get("selected_bundle") != str(directory / "fixtures" / SELECTED_BUNDLE)):
        raise ValueError("prepared fixture identity/path changed")
    if inventory(directory, exclude=("prepared.json",)) != data.get("files"):
        raise ValueError("prepared fixture bytes changed or observation already exists")
    for name, identity in ((SELECTED_BUNDLE, SELECTED_ID), (EXCLUDED_BUNDLE, EXCLUDED_ID), (PROBE_BUNDLE, PROBE_ID)):
        bundle = directory / "fixtures" / name
        info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
        if info.get("CFBundleIdentifier") != identity or info.get("CFBundleExecutable") != bundle.stem:
            raise ValueError("fixture bundle identity does not match protocol")
    return data


class BSDInfo(ctypes.Structure):
    """SDK sys/proc_info.h PROC_PIDTBSDINFO; only owned PIDs are queried."""
    _fields_ = [(name, ctypes.c_uint32) for name in ("flags", "status", "xstatus", "pid", "ppid",
                 "uid", "gid", "ruid", "rgid", "svuid", "svgid", "rfu")]
    _fields_ += [("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32)]
    _fields_ += [(name, ctypes.c_uint32) for name in ("nfiles", "pgid", "jobc", "tdev", "tpgid")]
    _fields_ += [("nice", ctypes.c_int32), ("start_sec", ctypes.c_uint64), ("start_usec", ctypes.c_uint64)]


def owned_identity(child, bundle: Path, bundle_id: str) -> dict:
    if child.process.poll() is not None:
        raise RuntimeError(f"owned child exited before identity observation: {child.name}")
    library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    library.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    library.proc_pidinfo.restype = ctypes.c_int
    library.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    library.proc_pidpath.restype = ctypes.c_int
    def birth():
        value = BSDInfo()
        count = library.proc_pidinfo(child.process.pid, 3, 0, ctypes.byref(value), ctypes.sizeof(value))
        if count != ctypes.sizeof(value) or value.pid != child.process.pid or value.start_usec >= 1_000_000:
            raise RuntimeError("owned PID birth observation failed")
        return value.start_sec * 1_000_000 + value.start_usec
    first = birth()
    path = ctypes.create_string_buffer(4096)
    count = library.proc_pidpath(child.process.pid, path, len(path))
    if count <= 0 or count >= len(path):
        raise RuntimeError("owned PID executable observation failed")
    actual = Path(os.fsdecode(path.value)).resolve(strict=True)
    if actual != executable(bundle) or birth() != first or child.process.poll() is not None:
        raise RuntimeError("owned PID identity changed or executable does not match fixture")
    return {"name": child.name, "pid": child.process.pid, "birth_microseconds": first,
            "bundle": str(bundle), "bundle_id": bundle_id, "executable": str(actual),
            "executable_sha256": digest(actual)}


class FixtureProcess(system.ManagedProcess):
    def __init__(self, binary: Path, output: Path, name: str, mode: str, seconds: int, uid: str,
                 tones: tuple | None = None):
        self.name, self.mode, self.output = name, mode, output
        self.report, self.events = output / f"{name}.json", output / f"{name}.csv"
        self.ready, self.wav = output / f"{name}.ready.json", output / f"{name}.wav"
        self.diagnostics = output / f"{name}-diagnostics.json"
        args = [str(binary), mode, "--uid", uid, "--seconds", str(seconds), "--report", str(self.report),
                "--events", str(self.events), "--ready", str(self.ready), "--diagnostics", str(self.diagnostics)]
        if mode == "record":
            args += ["--wav", str(self.wav)]
        if tones:
            for key, value in zip(("--left-hz", "--right-hz", "--left-amplitude", "--right-amplitude"), tones):
                args += [key, str(value)]
        self._spawn(args, output / f"{name}.log", seconds + 15)


class ApplicationProcess(system.ManagedProcess):
    def __init__(self, binary: Path, output: Path, bundle: Path):
        self.name, self.mode, self.output = "runtime", "application-mix", output
        self.ready = output / "runtime.ready.json"
        args = [str(binary), "--application-bundle", str(bundle), "--application-bundle-id", SELECTED_ID,
                "--output-id", "coreaudio:" + CABLE2, "--directory", str(output)]
        environment = dict(os.environ, RELINK_APPLICATION_CAPTURE_CONSENT="1")
        self._spawn(args, output / "runtime.log", 50, environment)


def wait_ready(child, combined_deadline: float, allowance: float = 8.0) -> None:
    deadline = min(combined_deadline, child.started_at + allowance)
    while not child.ready.exists():
        if child.process.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError(f"{child.name} callback readiness deadline expired")
        time.sleep(0.02)
    if time.monotonic() >= deadline:
        raise RuntimeError(f"{child.name} readiness arrived after its fixed deadline")
    value = json.loads(child.ready.read_text())
    if value.get("ready") is not True:
        raise ValueError("malformed actual-callback readiness")
    if child.name == "runtime":
        if value.get("captured_frames", 0) < 4096 or value.get("rendered_frames", 0) < 4096:
            raise ValueError("runtime readiness lacks actual captured/rendered frames")
    elif value.get("frames", 0) <= 0:
        raise ValueError("generator/recorder readiness lacks callback frames")


def arm_recorders(readers, writers, identities, children, output: Path, combined_deadline: float) -> None:
    if len(readers) != 2:
        raise ValueError("both independent recorders are required")
    # The first recorder's buffer is already advancing while the second child
    # is spawned. Include both spawn latency and identity checks in one budget.
    arm_deadline = min(combined_deadline, readers[0].started_at + 8.5)
    for reader in readers:
        wait_ready(reader, arm_deadline - 0.5)
    checked = [owned_identity(writer, bundle, identity) for writer, bundle, identity in writers]
    if checked != identities:
        raise RuntimeError("owned fixture identities changed before protocol start")
    save_new(output / "fixture-identity-before-go.json", {"owned_generators": checked})
    if time.monotonic() + 0.5 >= arm_deadline:
        raise RuntimeError("shared recorder arm deadline expired before preroll")
    time.sleep(0.5)
    if time.monotonic() >= arm_deadline or any(c.process.poll() is not None for c in children):
        raise RuntimeError("fixture ended or shared recorder arm deadline expired before go")
    with (output / "runtime.go").open("x") as file:
        file.write("Both independent recorder callbacks ready; begin the fixed application phases.\n")
    # A filesystem stall cannot turn a late publication into an accepted run.
    if time.monotonic() >= arm_deadline:
        raise RuntimeError("shared recorder arm deadline expired publishing go")


def require_unused_cable(found: dict, uid: str) -> None:
    if (found.get("uid") != uid or found.get("matches") != 1
            or found.get("status") != 0 or found.get("complete") is not True
            or found.get("device_running_status") != 0
            or found.get("device_is_running_somewhere") is not False):
        raise RuntimeError("owned cable must be uniquely discoverable and positively unused; busy/unknown is refused")


def run(prepared: Path, allow_system_audio_recording: bool) -> dict:
    # Authorization is checked before paths, subprocesses, or any audio query.
    if not allow_system_audio_recording:
        raise PermissionError("run requires explicit --allow-system-audio-recording after the user authorizes the concrete prepared fixture")
    if sys.platform != "darwin":
        raise RuntimeError("application fixture requires macOS")
    metadata = verify_prepared(prepared)
    prepared = prepared.resolve(strict=True)
    output = prepared / "observation"
    output.mkdir(exist_ok=False)
    children, errors, exits, observations, identities = [], [], [], [], []
    try:
        save_new(output / "run-authorization.json", {"protocol": PROTOCOL, "explicit_recording_flag": True,
            "prepared_manifest_sha256": digest(prepared / "prepared.json"), "source_revision": metadata["source_revision"]})
        with (output / "fixture-prepared.json").open("xb") as destination:
            destination.write((prepared / "prepared.json").read_bytes())
        probe = prepared / "system-probe"
        # Query only the two explicit owned UIDs. No install, retry, default or format change.
        for index, uid in enumerate((CABLE1, CABLE2)):
            found = system.discover(probe, output, f"preflight-cable-{index + 1}", uid)
            require_unused_cable(found, uid)
        combined_deadline = time.monotonic() + STARTUP_SECONDS
        fixtures = prepared / "fixtures"
        writers = []
        for name, bundle_name, bundle_id, *tones in WRITERS:
            bundle = fixtures / bundle_name
            writer = FixtureProcess(executable(bundle), output, name, "play", SOURCE_SECONDS, CABLE1, tuple(tones))
            children.append(writer)
            writers.append((writer, bundle, bundle_id))
        for writer, bundle, bundle_id in writers:
            wait_ready(writer, combined_deadline)
            identities.append(owned_identity(writer, bundle, bundle_id))
        validate_owned_identities(identities, str(fixtures / SELECTED_BUNDLE))
        save_new(output / "fixture-observation.json", {"protocol": PROTOCOL,
            "selected_bundle": str(fixtures / SELECTED_BUNDLE), "owned_generators": identities,
            "source_revision": metadata["source_revision"], "prepared_manifest_sha256": digest(prepared / "prepared.json")})
        runtime = ApplicationProcess(executable(fixtures / PROBE_BUNDLE), output, fixtures / SELECTED_BUNDLE)
        children.append(runtime)
        wait_ready(runtime, combined_deadline, allowance=12)
        readers = []
        for name, uid in (("source-witness", CABLE1), ("runtime-reader", CABLE2)):
            reader = FixtureProcess(probe, output, name, "record", RECORD_SECONDS, uid)
            readers.append(reader)
            children.append(reader)
        arm_recorders(readers, writers, identities, children, output, combined_deadline)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        errors.append(str(error))
    finally:
        # Each child's watcher enforces its own absolute deadline immediately;
        # sequential collection cannot extend another child's runtime.
        for child in children:
            try:
                exits.append(child.finish())
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                errors.append(f"{child.name} finalization: {error}")
        for child in children:
            try:
                result = system.child_diagnostics(child)
                if result is not None:
                    observations.append(result)
            except (OSError, ValueError, RuntimeError) as error:
                errors.append(f"{child.name} diagnostics: {error}")
        try:
            analysis = analyze_application.analyze(output)
        except system.OBSERVATION_ERRORS + (TypeError, IndexError, OverflowError) as error:
            analysis = {"pass": False, "observer_error": str(error)}
        save_new(output / "application-analysis.json", analysis)
        observations.append(analysis)
        result = {"schema": 1, "protocol": PROTOCOL,
            "pass": len(children) == 6 and len(exits) == 6 and not errors
                    and all(o["pass"] for o in observations)
                    and all(e["exit_code"] == 0 and not e["timed_out"] for e in exits),
            "errors": errors, "processes": exits, "observations": observations}
        save_new(output / "application-acceptance.json", result)
        save_new(output / "custody-manifest.json", {"schema": 1, "protocol": PROTOCOL,
            "source_revision": metadata["source_revision"], "files": inventory(output)})
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="operation", required=True)
    prep = commands.add_parser("prepare")
    prep.add_argument("--output", type=Path, required=True)
    prep.add_argument("--runtime", type=Path, required=True)
    observe = commands.add_parser("run")
    observe.add_argument("--prepared", type=Path, required=True)
    observe.add_argument("--allow-system-audio-recording", action="store_true")
    args = parser.parse_args()
    try:
        if args.operation == "prepare":
            prepare(args.output, args.runtime)
            return 0
        return 0 if run(args.prepared, args.allow_system_audio_recording)["pass"] else 1
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
