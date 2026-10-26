#!/usr/bin/env python3
"""Install/test/remove only this original driver on a disposable GitHub runner.

Refuses local execution. Child failures preserve completed/partial PCM and logs;
cleanup failures fail the job. This is an observer, never a driver repair tool.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import signal
import struct
import subprocess
import sys
import threading
import time

from analyze_system import analyze, load_events, load_wav
from analyze_diagnostics import summarize as summarize_diagnostics

UID = "au.textual.ravelink.cable.1"
UIDS = (UID, "au.textual.ravelink.cable.2")
BUNDLE_ID = "au.textual.ravelink.hal"
DESTINATION = Path("/Library/Audio/Plug-Ins/HAL/RustRaveLink.driver")
REPOSITORY = Path(__file__).resolve().parents[3]
DEFAULT_OUTPUT = REPOSITORY / "build/rust-hal-system"
OBSERVATION_ERRORS = (OSError, ValueError, KeyError, ZeroDivisionError, struct.error)


def guard_environment() -> None:
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_OS") != "macOS":
        raise RuntimeError("This mutating harness runs only on a disposable GitHub macOS runner")
    if not os.environ.get("GITHUB_RUN_ID") or not os.environ.get("RUNNER_TEMP"):
        raise RuntimeError("GitHub runner identity is missing")
    if DESTINATION.parent.resolve() != DESTINATION.parent or DESTINATION.is_symlink():
        raise RuntimeError("Refusing a symlinked HAL path")


def save(path: Path, data: dict) -> None:
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n")


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def bounded(command: list[str], log: Path, timeout: float = 20) -> subprocess.CompletedProcess:
    with log.open("wb") as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            # Kill the command's entire group, including a sudo child, before
            # returning to cleanup. A timed-out copy must not race removal.
            signal_group(process, signal.SIGKILL)
            process.wait(timeout=5)
            raise RuntimeError(f"command timed out: {command[0]}; see {log}") from error
    return subprocess.CompletedProcess(command, code)


def signal_group(process: subprocess.Popen, value: int) -> bool:
    try:
        os.killpg(process.pid, value)
        return True
    except ProcessLookupError:
        return False  # The child exited between its timeout and this signal.


def restart_service(output: Path, label: str) -> None:
    result = bounded(["sudo", "/usr/bin/killall", "coreaudiod"], output / f"{label}-restart.log")
    # A missing daemon is permitted only when killall explicitly says so; actual
    # device enumeration/absence must still succeed independently afterward.
    if result.returncode and not (result.returncode == 1 and "No matching processes" in (output / f"{label}-restart.log").read_text()):
        raise RuntimeError(f"coreaudiod restart failed ({result.returncode})")


def discover(probe: Path, output: Path, label: str, uid: str = UID) -> dict:
    report = output / f"{label}-discovery.json"
    result = bounded([str(probe), "discover", "--uid", uid, "--report", str(report)], output / f"{label}-discovery.log")
    if result.returncode or not report.exists():
        raise RuntimeError(f"device discovery failed; see {label}-discovery.log")
    data = json.loads(report.read_text())
    if data.get("status") != 0 or not data.get("complete"):
        raise RuntimeError("discovery did not complete successfully")
    return data


def wait_presence(probe: Path, output: Path, label: str, wanted: int, uid: str = UID) -> dict:
    deadline, attempt = time.monotonic() + 25, 0
    last_error = "no discovery completed"
    while True:
        try:
            data = discover(probe, output, f"{label}-{attempt}", uid)
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            # CoreAudio can be unavailable briefly during daemon restart. Each
            # failed observation remains a separate file; valid absence never
            # comes from swallowing a query error.
            last_error = str(error)
            save(output / f"{label}-{attempt}-error.json", {"error": last_error})
        else:
            if data["matches"] == wanted:
                return data
            if data["matches"] > 1:
                raise RuntimeError(f"ambiguous device UID: {data['matches']} matches")
            last_error = f"expected {wanted} installed UID match, found {data['matches']}"
        if time.monotonic() >= deadline:
            raise RuntimeError(f"device discovery deadline expired: {last_error}")
        attempt += 1
        time.sleep(0.5)


def cleanup(output: Path, probe: Path) -> dict:
    marker = output / "installation-owner.json"
    if not marker.exists():
        return {"pass": True, "skipped": "No installation ownership was recorded"}
    guard_environment()
    owned = json.loads(marker.read_text())
    if owned.get("destination") != str(DESTINATION) or owned.get("run_id") != os.environ["GITHUB_RUN_ID"]:
        raise RuntimeError("cleanup ownership does not match this path and run")
    attempt = len(list(output.glob("cleanup-attempt-*.json")))
    label = f"cleanup-attempt-{attempt}"
    try:
        removed = False
        if DESTINATION.exists():
            if DESTINATION.is_symlink():
                raise RuntimeError("refusing to remove a symlinked bundle")
            info = DESTINATION / "Contents/Info.plist"
            if info.exists() and plistlib.loads(info.read_bytes()).get("CFBundleIdentifier") != BUNDLE_ID:
                raise RuntimeError("cleanup bundle identifier changed")
            result = bounded(["sudo", "/bin/rm", "-r", str(DESTINATION)], output / f"{label}-remove.log")
            if result.returncode:
                raise RuntimeError("owned driver removal failed")
            removed = True
        # The always-run workflow step verifies an already successful cleanup
        # without needlessly restarting the daemon a second time.
        previous = output / "cleanup.json"
        previously_clean = previous.exists() and json.loads(previous.read_text()).get("pass")
        if removed or not previously_clean:
            restart_service(output, label)
        absent = {uid: wait_presence(probe, output, f"{label}-cable-{index + 1}", 0, uid) for index, uid in enumerate(UIDS)}
        result = {"pass": not DESTINATION.exists(), "devices": absent, "removed_only": str(DESTINATION), "attempt": attempt}
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        result = {"pass": False, "error": str(error), "attempt": attempt}
        save(output / f"{label}.json", result)
        save(output / "cleanup.json", result)
        raise
    save(output / f"{label}.json", result)
    save(output / "cleanup.json", result)
    return result


class ManagedProcess:
    """Observe/reap every child immediately, independently of finish order.

    Completion observed after its deadline is conservatively a timeout even if
    the OS has already reaped it. A stalled sibling cannot grant extra runtime.
    """
    TERM_GRACE_SECONDS = 2
    KILL_GRACE_SECONDS = 5

    def _spawn(self, command: list[str], log: Path, seconds: float, environment=None):
        self.log = log.open("xb")
        self.started_at = time.monotonic()
        self.deadline = self.started_at + seconds
        self.timed_out = False
        self.result = None
        try:
            self.process = subprocess.Popen(command, env=environment, stdout=self.log,
                                            stderr=subprocess.STDOUT, start_new_session=True)
        except OSError:
            self.log.close()
            raise
        self._supervisor = threading.Thread(target=self._watch, name=f"observe-{self.name}", daemon=True)
        try:
            self._supervisor.start()
        except RuntimeError:
            signal_group(self.process, signal.SIGKILL)
            self.process.wait(timeout=self.KILL_GRACE_SECONDS)
            self.log.close()
            raise

    def _watch(self):
        code, error, term, kill = None, None, False, False
        observed_at = None
        try:
            try:
                code = self.process.wait(timeout=max(0.0, self.deadline - time.monotonic()))
                observed_at = time.monotonic()
                self.timed_out = observed_at > self.deadline
            except subprocess.TimeoutExpired:
                self.timed_out = True
                term = signal_group(self.process, signal.SIGTERM)
                try:
                    code = self.process.wait(timeout=self.TERM_GRACE_SECONDS)
                except subprocess.TimeoutExpired:
                    pass
                # The parent can exit on TERM while a descendant ignores it.
                # Always end the timed-out process group before cleanup proceeds.
                kill = signal_group(self.process, signal.SIGKILL)
                code = self.process.wait(timeout=self.KILL_GRACE_SECONDS)
                observed_at = time.monotonic()
        except (OSError, subprocess.SubprocessError) as caught:
            error = str(caught)
            self.timed_out = True
            try:
                kill = signal_group(self.process, signal.SIGKILL) or kill
                code = self.process.wait(timeout=self.KILL_GRACE_SECONDS)
                observed_at = time.monotonic()
            except (OSError, subprocess.SubprocessError) as cleanup_error:
                error += f"; final reap failed: {cleanup_error}"
        finally:
            self.log.close()
            self.result = {"name": self.name, "mode": self.mode, "exit_code": code,
                "timed_out": self.timed_out, "deadline_seconds": self.deadline - self.started_at,
                "completion_observed_seconds": observed_at - self.started_at if observed_at is not None else None,
                "supervision_finished_seconds": time.monotonic() - self.started_at,
                "term_sent": term, "kill_sent": kill, "supervision_error": error}

    def finish(self) -> dict:
        self._supervisor.join(timeout=max(0.0, self.deadline - time.monotonic())
                              + self.TERM_GRACE_SECONDS + 2 * self.KILL_GRACE_SECONDS + 1)
        if self._supervisor.is_alive() or self.result is None:
            signal_group(self.process, signal.SIGKILL)
            self.process.wait(timeout=self.KILL_GRACE_SECONDS)
            raise RuntimeError(f"{self.name} supervisor did not finish within its bounded cleanup deadline")
        return self.result


class ProbeProcess(ManagedProcess):
    def __init__(self, probe: Path, output: Path, name: str, mode: str, seconds: float, uid: str = UID):
        self.name, self.mode, self.output = name, mode, output
        self.report, self.events = output / f"{name}.json", output / f"{name}.csv"
        self.wav, self.ready = output / f"{name}.wav", output / f"{name}.ready.json"
        if self.ready.exists() or self.report.exists() or self.wav.exists():
            raise RuntimeError(f"refusing to overwrite existing observation {name}")
        command = [str(probe), mode, "--uid", uid, "--seconds", str(seconds), "--report", str(self.report),
                   "--events", str(self.events), "--ready", str(self.ready)]
        if mode == "record":
            command += ["--wav", str(self.wav)]
        self.diagnostics = output / f"{name}-diagnostics.json" if os.environ.get("RAVELINK_OBSERVER_DIAGNOSTICS") == "1" else None
        if self.diagnostics:
            if self.diagnostics.exists():
                raise RuntimeError(f"refusing to overwrite diagnostics {name}")
            command += ["--diagnostics", str(self.diagnostics)]
        self._spawn(command, output / f"{name}.log", seconds + 15)

    def wait_ready(self) -> None:
        deadline = time.monotonic() + 8
        while not self.ready.exists():
            if self.process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f"{self.name} did not publish actual-callback readiness")
            time.sleep(0.02)
        if not json.loads(self.ready.read_text()).get("ready"):
            raise RuntimeError(f"{self.name} published malformed readiness")

def child_diagnostics(child) -> dict | None:
    path = getattr(child, "diagnostics", None)
    if path is None:
        return None
    try:
        result = summarize_diagnostics(path, child.events, child.report)
    except OBSERVATION_ERRORS + (TypeError, IndexError, OverflowError) as error:
        result = {"pass": False, "observer_error": str(error)}
    save(child.output / f"{child.name}-diagnostics-analysis.json", result)
    return result

def pair_consistency(first: ProbeProcess, second: ProbeProcess) -> dict:
    a, b = load_wav(first.wav), load_wav(second.wav)
    ea, eb = load_events(first.events), load_events(second.events)
    origin_a, origin_b = ea[0]["sample_time"], eb[0]["sample_time"]
    delta = origin_b - origin_a
    if abs(delta - round(delta)) > 0.001:
        raise ValueError("independent readers have non-integral relative sample positions")
    begin = max(origin_a, origin_b) + 2 * 48000
    end = min(origin_a + len(a) // 2, origin_b + len(b) // 2) - 2 * 48000
    count = int(end - begin)
    if count < 48000:
        raise ValueError("insufficient independent-reader overlap")
    offset_a, offset_b = round(begin - origin_a), round(begin - origin_b)
    maximum, differing = 0.0, 0
    for frame in range(count):
        for side in range(2):
            error = abs(a[(offset_a + frame) * 2 + side] - b[(offset_b + frame) * 2 + side])
            maximum = max(maximum, error)
            differing += error > 0.000001
    return {"pass": differing == 0, "overlap_frames": count, "maximum_difference": maximum,
            "differing_samples_above_1e-6": differing, "relative_start_frames": delta}


def run_observation(probe: Path, output: Path) -> dict:
    children: list[ProbeProcess] = []
    observations, exceptions, exits = [], [], []
    try:
        # Forty seconds cover both bounded eight-second reader starts and each
        # full twenty-second recording without adaptive analysis trimming.
        writer = ProbeProcess(probe, output, "tone-writer", "play", 40); children.append(writer); writer.wait_ready()
        a = ProbeProcess(probe, output, "reader-a", "record", 20); children.append(a); a.wait_ready()
        b = ProbeProcess(probe, output, "reader-b", "record", 20); children.append(b); b.wait_ready()
    except (OSError, ValueError, RuntimeError) as error:
        exceptions.append(str(error))
    finally:
        # A failed child does not terminate the other bounded observers before
        # they can save their PCM and timestamps.
        for child in children:
            try:
                exits.append(child.finish())
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                exceptions.append(f"{child.name} cleanup failed: {error}")
                exits.append({"name": child.name, "mode": child.mode, "exit_code": None, "timed_out": True})
    for child in children:
        try:
            analysis = analyze(child.report, child.events, child.wav if child.mode == "record" else None)
        except OBSERVATION_ERRORS as error:
            analysis = {"pass": False, "observer_error": str(error)}
        save(output / f"{child.name}-analysis.json", analysis); observations.append(analysis)
        diagnostics = child_diagnostics(child)
        if diagnostics is not None:
            observations.append(diagnostics)
    if len(children) == 3:
        try:
            pair = pair_consistency(children[1], children[2])
        except OBSERVATION_ERRORS as error:
            pair = {"pass": False, "observer_error": str(error)}
        save(output / "independent-readers.json", pair); observations.append(pair)
    # No writer remains. Restart capture alone so first-client StartIO must
    # expose silence, not a prior ring generation or replayed tone.
    try:
        silence = ProbeProcess(probe, output, "restarted-silence", "record", 3)
        exits.append(silence.finish())
        silent = analyze(silence.report, silence.events, silence.wav, silence=True)
        diagnostics = child_diagnostics(silence)
        if diagnostics is not None:
            observations.append(diagnostics)
    except OBSERVATION_ERRORS + (RuntimeError, subprocess.SubprocessError) as error:
        silent = {"pass": False, "observer_error": str(error)}
    save(output / "restarted-silence-analysis.json", silent); observations.append(silent)
    result = {"schema": 1, "observer_diagnostics_enabled": os.environ.get("RAVELINK_OBSERVER_DIAGNOSTICS") == "1",
              "pass": not exceptions and all(o["pass"] for o in observations)
              and all(e["exit_code"] == 0 and not e["timed_out"] for e in exits),
              "exceptions": exceptions, "processes": exits, "observations": observations}
    save(output / "acceptance.json", result)
    return result


def run(output: Path, probe: Path, bundle: Path, observation=None) -> bool:
    guard_environment()
    output.mkdir(parents=True, exist_ok=True)
    if (output / "installation-owner.json").exists() or DESTINATION.exists():
        raise RuntimeError("a driver ownership marker or destination already exists")
    before = {uid: discover(probe, output, f"before-cable-{index + 1}", uid) for index, uid in enumerate(UIDS)}
    if any(data["matches"] for data in before.values()):
        raise RuntimeError("refusing installation because a test UID already exists")
    info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
    if info.get("CFBundleIdentifier") != BUNDLE_ID:
        raise RuntimeError("unexpected source bundle identity")
    ownership = {"destination": str(DESTINATION), "run_id": os.environ["GITHUB_RUN_ID"],
                 "source_revision": os.environ.get("GITHUB_SHA"), "bundle_id": BUNDLE_ID,
                 "source_binary_sha256": sha(bundle / "Contents/MacOS/RaveLinkHAL"),
                 "observer_sha256": sha(probe), "uids_absent_before": list(UIDS),
                 "initial_observation": before}
    save(output / "installation-owner.json", ownership)
    result, cleaned = False, False
    try:
        for command, label in [(["sudo", "/usr/bin/ditto", str(bundle), str(DESTINATION)], "install"),
                               (["sudo", "/usr/sbin/chown", "-R", "root:wheel", str(DESTINATION)], "ownership"),
                               (["sudo", "/bin/chmod", "-R", "go-w", str(DESTINATION)], "permissions")]:
            if bounded(command, output / f"{label}.log").returncode:
                raise RuntimeError(f"{label} failed")
        if sha(DESTINATION / "Contents/MacOS/RaveLinkHAL") != ownership["source_binary_sha256"]:
            raise RuntimeError("installed binary differs from the built artifact")
        restart_service(output, "install")
        installed = {uid: wait_presence(probe, output, f"installed-cable-{index + 1}", 1, uid) for index, uid in enumerate(UIDS)}
        save(output / "installed-devices.json", installed)
        result = (observation or run_observation)(probe, output)["pass"]
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        save(output / "harness-error.json", {"pass": False, "error": str(error)})
    finally:
        try:
            cleaned = cleanup(output, probe)["pass"]
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            save(output / "cleanup.json", {"pass": False, "error": str(error)})
    return result and cleaned


def manifest(output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    records = []
    for path in sorted(output.rglob("*")):
        if path.is_file() and path.name != "custody-manifest.json":
            records.append({"path": str(path.relative_to(output)), "bytes": path.stat().st_size, "sha256": sha(path)})
    save(output / "custody-manifest.json", {"schema": 1, "source_revision": os.environ.get("GITHUB_SHA"),
        "run_id": os.environ.get("GITHUB_RUN_ID"), "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "files": records, "note": "Original bytes, including failures. This manifest does not assert acceptance."})


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["run", "cleanup", "manifest"])
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--probe", type=Path, default=DEFAULT_OUTPUT / "system-probe")
    parser.add_argument("--bundle", type=Path, default=REPOSITORY / "dist/macos/RaveLink.driver")
    args = parser.parse_args()
    try:
        if args.mode == "manifest":
            manifest(args.output)
            ok = True
        elif args.mode == "cleanup":
            ok = cleanup(args.output, args.probe)["pass"]
        else:
            ok = run(args.output, args.probe, args.bundle)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        if args.output.is_dir():
            save(args.output / f"{args.mode}-error.json", {"pass": False, "error": str(error)})
        print(str(error), file=sys.stderr)
        return 1
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
