#!/usr/bin/env python3
"""One CI-only observation of two original native outputs by independent readers.

No local device operations are permitted. The existing installation ownership,
bounded child supervision and exact manifest utilities are reused unchanged.
Every started process finishes independently, even if another participant fails.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import analyze_multioutput as analysis
import run_system as system

DEFAULT_OUTPUT = system.REPOSITORY / "build/rust-multioutput-system"
DEFAULT_RUNTIME = system.REPOSITORY / "target/release/examples/runtime-multioutput-probe"


def wait_readers_ready(readers):
    deadline = min(reader.started_at for reader in readers) + 8
    pending = list(readers)
    while pending:
        for reader in list(pending):
            if reader.ready.exists():
                if json.loads(reader.ready.read_text()).get("ready") is not True:
                    raise RuntimeError(f"{reader.name} published malformed readiness")
                pending.remove(reader)
            elif reader.process.poll() is not None:
                raise RuntimeError(f"{reader.name} exited before recorder readiness")
        if pending and time.monotonic() >= deadline:
            raise RuntimeError("both independent readers did not become ready within shared eight-second budget")
        if pending:
            time.sleep(0.02)


class RuntimeProcess(system.ManagedProcess):
    def __init__(self, binary: Path, output: Path):
        self.name, self.mode, self.output = "multioutput-runtime", "two-native-outputs", output
        self.ready = output / "multioutput-ready.json"
        if any(output.glob("multioutput-*")) or (output / "multioutput.go").exists():
            raise RuntimeError("refusing to overwrite multi-output evidence")
        environment = dict(os.environ, RAVELINK_MULTIOUTPUT_ACCEPTANCE="1")
        command = [str(binary), "--output-a-id", analysis.OUTPUT_IDS[0],
                   "--output-b-id", analysis.OUTPUT_IDS[1], "--directory", str(output)]
        self._spawn(command, output / "multioutput-runtime.log", 55, environment)

    def wait_ready(self):
        deadline = time.monotonic() + 12
        while not self.ready.exists():
            if self.process.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("multi-output runtime did not publish actual submitted-frame readiness")
            time.sleep(0.02)
        ready = json.loads(self.ready.read_text())
        routes = ready.get("outputs", [])
        if (ready.get("schema") != 1 or ready.get("protocol") != analysis.PROTOCOL
                or ready.get("ready") is not True or len(routes) != 2
                or any(route.get("route_index") != index
                       or route.get("output_id") != analysis.OUTPUT_IDS[index]
                       or route.get("is_clock") is not (index == 1)
                       or route.get("submitted_frames", 0) < 4096
                       for index, route in enumerate(routes))):
            raise RuntimeError("multi-output readiness did not prove both exact output routes")


def observe(probe: Path, output: Path, runtime_binary: Path) -> dict:
    children, errors, exits = [], [], []
    try:
        runtime = RuntimeProcess(runtime_binary, output)
        children.append(runtime)
        runtime.wait_ready()
        readers = []
        # Start both before waiting for either. Neither recorder is started by
        # the product or reads a product buffer; each opens its exact HAL UID.
        for index, uid in enumerate(system.UIDS):
            reader = system.ProbeProcess(probe, output, f"multioutput-reader-{index}",
                                         "record", analysis.RECORD_SECONDS, uid)
            children.append(reader)
            readers.append(reader)
        wait_readers_ready(readers)
        # Fixed prefix, never selected from the resulting PCM. Both complete
        # recording clocks (including this prefix) remain subject to continuity.
        time.sleep(analysis.LIMITS["initial_settling_seconds"])
        with (output / "multioutput.go").open("x") as file:
            file.write("Both independent recorder callbacks ready; fixed two-second prefix elapsed.\n")
    except (OSError, ValueError, RuntimeError) as error:
        errors.append(str(error))
    finally:
        for child in children:
            try:
                exits.append(child.finish())
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                errors.append(f"{child.name} finish failed: {error}")
                exits.append({"name": child.name, "exit_code": None, "timed_out": True})
    observations = []
    for child in children:
        diagnostics = system.child_diagnostics(child)
        if diagnostics is not None:
            observations.append(diagnostics)
    try:
        pcm = analysis.analyze(output)
    except system.OBSERVATION_ERRORS + (TypeError, IndexError, OverflowError) as error:
        pcm = {"schema": 1, "protocol": analysis.PROTOCOL, "pass": False, "observer_error": str(error)}
    system.save(output / "multioutput-analysis.json", pcm)
    observations.append(pcm)
    diagnostics_enabled = os.environ.get("RAVELINK_OBSERVER_DIAGNOSTICS") == "1"
    result = {"schema": 1, "protocol": analysis.PROTOCOL,
              "observer_diagnostics_enabled": diagnostics_enabled,
              "pass": len(children) == 3 and not errors and diagnostics_enabled
              and len(observations) == 3 and all(item["pass"] for item in observations)
              and all(item["exit_code"] == 0 and not item["timed_out"] for item in exits),
              "exceptions": errors, "processes": exits, "observations": observations}
    system.save(output / "multioutput-acceptance.json", result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["run", "cleanup", "manifest"])
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--probe", type=Path, default=DEFAULT_OUTPUT / "system-probe")
    parser.add_argument("--bundle", type=Path, default=system.REPOSITORY / "dist/macos/ReLink.driver")
    parser.add_argument("--runtime", type=Path, default=DEFAULT_RUNTIME)
    args = parser.parse_args()
    try:
        if args.mode == "manifest":
            system.manifest(args.output)
            return 0
        if args.mode == "cleanup":
            return 0 if system.cleanup(args.output, args.probe)["pass"] else 1
        ok = system.run(args.output, args.probe, args.bundle,
                        observation=lambda probe, output: observe(probe, output, args.runtime))
        return 0 if ok else 1
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        if args.output.is_dir():
            system.save(args.output / f"{args.mode}-error.json", {"pass": False, "error": str(error)})
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
