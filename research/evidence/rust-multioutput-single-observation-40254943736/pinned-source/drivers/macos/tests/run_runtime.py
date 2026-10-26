#!/usr/bin/env python3
"""CI-only installed capture -> mixer -> second-cable observation.

This reuses installation ownership and the independent C observer, not product
DSP or product analysis. Every started observer retains its bounded evidence
even when another process fails. No audio endpoint is selected by default.
"""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import analyze_runtime
from analyze_system import analyze
import run_system as system

DEFAULT_OUTPUT = system.REPOSITORY / "build/rust-runtime-system"
DEFAULT_RUNTIME = system.REPOSITORY / "target/release/examples/runtime-capture-probe"


class RuntimeProcess(system.ProbeProcess):
    def __init__(self, binary: Path, output: Path):
        self.name, self.mode, self.output = "runtime", "capture-mix", output
        self.ready = output / "runtime.ready.json"
        if any(output.glob("runtime-*")) or self.ready.exists() or (output / "runtime.go").exists():
            raise RuntimeError("refusing to overwrite runtime evidence")
        environment = dict(os.environ, RAVELINK_RUNTIME_ACCEPTANCE="1")
        command = [str(binary), "--input-id", f"coreaudio:{system.UIDS[0]}",
                   "--output-id", f"coreaudio:{system.UIDS[1]}", "--directory", str(output)]
        self._spawn(command, output / "runtime.log", 50, environment)

    def wait_ready(self):
        deadline = time.monotonic() + 12
        while not self.ready.exists():
            if self.process.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("runtime did not publish real capture/mix readiness")
            time.sleep(0.02)
        ready = json.loads(self.ready.read_text())
        if not ready.get("ready") or ready.get("captured_frames", 0) < 4096 or ready.get("rendered_frames", 0) < 4096:
            raise RuntimeError("runtime readiness lacked real captured/rendered frames")


def observe(probe: Path, output: Path, runtime_binary: Path) -> dict:
    children, errors, exits = [], [], []
    try:
        # The 60-second generator covers 8s writer + 12s engine + 8s reader
        # readiness budgets and the complete 32s recording after writer starts.
        writer = system.ProbeProcess(probe, output, "source-writer", "play", 60, system.UIDS[0])
        children.append(writer)
        writer.wait_ready()
        runtime = RuntimeProcess(runtime_binary, output)
        children.append(runtime)
        runtime.wait_ready()
        reader = system.ProbeProcess(probe, output, "runtime-reader", "record", 32, system.UIDS[1])
        children.append(reader)
        reader.wait_ready()
        # Retain a fixed pre-roll before phase events; first readiness is an
        # actual recorder callback, never a guessed device-open delay.
        time.sleep(0.5)
        with (output / "runtime.go").open("x") as file:
            file.write("Independent recorder callback ready; begin fixed phases.\n")
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
    for label, operation in [
        ("source-writer", lambda: analyze(output / "source-writer.json", output / "source-writer.csv", None)),
        ("runtime-output", lambda: analyze_runtime.analyze(output)),
    ]:
        try:
            result = operation()
        except system.OBSERVATION_ERRORS + (TypeError, IndexError, OverflowError) as error:
            result = {"pass": False, "observer_error": str(error)}
        system.save(output / f"{label}-analysis.json", result)
        observations.append(result)
    result = {"schema": 1, "observer_diagnostics_enabled": os.environ.get("RAVELINK_OBSERVER_DIAGNOSTICS") == "1",
              "pass": len(children) == 3 and not errors
              and all(o["pass"] for o in observations)
              and all(e["exit_code"] == 0 and not e["timed_out"] for e in exits),
              "exceptions": errors, "processes": exits, "observations": observations}
    system.save(output / "runtime-acceptance.json", result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["run", "cleanup", "manifest"])
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--probe", type=Path, default=DEFAULT_OUTPUT / "system-probe")
    parser.add_argument("--bundle", type=Path, default=system.REPOSITORY / "dist/macos/RaveLink.driver")
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
