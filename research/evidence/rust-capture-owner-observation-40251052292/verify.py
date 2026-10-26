#!/usr/bin/env python3
"""Offline custody and bounded failure derivation. No devices, network or rerun.

Loads frozen analyzer modules from the exact local Git source commit. Original
reports, PCM, clocks, thresholds and workflow outcomes are never changed.
"""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SOURCE = "910ef63b20782268af8cbd2e35a56b606f4a6b82"


def digest(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def read_json(archive, name):
    return json.loads(archive.read(name))


def derive(archives):
    with zipfile.ZipFile(archives / "original-hal.zip") as hal, zipfile.ZipFile(
        archives / "original-runtime.zip"
    ) as runtime:
        hal_acceptance = read_json(hal, "acceptance.json")
        acceptance = read_json(runtime, "runtime-acceptance.json")
        result = read_json(runtime, "runtime-result.json")
        analysis = read_json(runtime, "runtime-output-analysis.json")
        assert hal_acceptance["pass"] and not hal_acceptance["exceptions"]
        assert not acceptance["pass"] and not acceptance["exceptions"]
        assert result == {
            "schema": 1,
            "pass": False,
            "native_output_shutdown_proven": False,
            "error": "runtime stopped: fault=Xrun, error=None",
        }
        assert analysis == {
            "pass": False,
            "observer_error": "missing or repeated runtime protocol events",
        }
        lines = runtime.read("runtime-native-timing.jsonl").splitlines(keepends=True)
        records = [json.loads(line) for line in lines]
        assert len(records) == 42
        first_index = next(
            i for i, r in enumerate(records) if r["first_fault"] is not None
        )
        fault = records[first_index]["first_fault"]
        assert (
            fault["origin"] == "NativeOutputOverload"
            and fault["direction"] == "Output"
            and fault["fault"] == 5
        )
        assert (
            fault["endpoint_id"] == "coreaudio:au.textual.ravelink.cable.2"
            and fault["endpoint_index"] == 0
        )
        assert fault["metadata"] == {
            "epoch": 1,
            "native_object": 100,
            "observed_mach_host_time": 6131263519,
            "packet_index": None,
            "property": {"element": 0, "scope": 1735159650, "selector": 1870030194},
            "status": None,
        }
        assert not fault["metadata_pending"]
        assert all(
            r["first_fault"] == fault and r["fault"] == 5 for r in records[first_index:]
        )
        terminal = records[-1]
        assert terminal["observation"] == "post-stop-final" and not terminal["running"]
        assert (
            terminal["native_shutdown_observed"]
            and terminal["native_output_shutdown_observed"]
        )
        assert (
            not terminal["native_output_shutdown_proven"] and terminal["error"] is None
        )
        output = terminal["output_timing"]
        assert output["backend"] == "coreaudio"
        assert output["first_fault"] is None and not output["first_fault_pending"]
        assert output["callbacks_seen"] == output["progress_count"] == 187
        output_lifecycle = {e["phase"]: e for e in output["lifecycle"]}
        assert "ContextRetained" not in output_lifecycle
        for phase in (
            "StartReturned",
            "StopReturned",
            "DestroyReturned",
            "ListenerRemovalReturned",
        ):
            assert output_lifecycle[phase]["status"] == 0
        for phase in ("QuiescenceChecked", "OwnerCompleted"):
            assert output_lifecycle[phase]["callbacks_in_flight"] == 0
        assert (
            output_lifecycle["OwnerCompleted"]["host_time"]
            >= output_lifecycle["QuiescenceChecked"]["host_time"]
        )
        assert len(terminal["captures"]) == 1
        capture = terminal["captures"][0]
        assert capture["input_id"] == "coreaudio:au.textual.ravelink.cable.1"
        timing = capture["native_timing"]
        assert (
            timing["packets_seen"] == 177
            and timing["first_fault"] is None
            and capture["captured_frames"] == 90624
        )
        assert all(
            r["captures"][0]["native_timing"]["first_fault"] is None for r in records
        )
        input_lifecycle = {e["phase"]: e for e in timing["lifecycle"]}
        assert "ContextRetained" not in input_lifecycle
        for phase in (
            "StartReturned",
            "StopReturned",
            "DestroyReturned",
            "ListenersRemovalReturned",
        ):
            assert input_lifecycle[phase]["status"] == 0
        for phase in ("QuiescenceChecked", "OwnerCompleted"):
            assert input_lifecycle[phase]["callbacks_in_flight"] == 0
        assert (
            input_lifecycle["OwnerCompleted"]["host_time"]
            >= input_lifecycle["QuiescenceChecked"]["host_time"]
        )
        assert (HERE / "runtime-terminal-original.json").read_bytes() == lines[-1]
        assert (HERE / "runtime-first-fault-original.json").read_bytes() == lines[
            first_index
        ]
        events = [
            json.loads(line)
            for line in runtime.read("runtime-events.jsonl").splitlines()
        ]
        assert [e["event"] for e in events] == ["started", "phase"] and events[-1][
            "phase"
        ] == 0
        cleanups = {}
        for kind, archive in [("hal", hal), ("runtime", runtime)]:
            cleanup = read_json(archive, "cleanup.json")
            assert cleanup["pass"] and len(cleanup["devices"]) == 2
            assert all(d["matches"] == 0 for d in cleanup["devices"].values())
            cleanups[kind] = {
                "pass": True,
                "both_owned_devices_absent": True,
                "attempt": cleanup["attempt"],
            }
        source_identities = {}
        with tempfile.TemporaryDirectory(prefix="relink-frozen-failure-") as scratch:
            temporary = Path(scratch)
            for module in ("analyze_system", "analyze_runtime"):
                relative = f"drivers/macos/tests/{module}.py"
                code = subprocess.check_output(
                    ["git", "show", f"{SOURCE}:{relative}"], cwd=ROOT
                )
                (temporary / f"{module}.py").write_bytes(code)
                source_identities[relative] = digest(code)
            for name in (
                "runtime-reader.json",
                "runtime-reader.csv",
                "runtime-reader.wav",
                "runtime-result.json",
                "runtime-events.jsonl",
                "runtime-telemetry.csv",
                "source-writer.json",
                "source-writer.csv",
            ):
                (temporary / name).write_bytes(runtime.read(name))
            previous = sys.modules.pop("analyze_system", None)
            sys.path.insert(0, str(temporary))
            try:
                spec = importlib.util.spec_from_file_location(
                    "relink_frozen_failure_analyzer", temporary / "analyze_runtime.py"
                )
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                system = sys.modules["analyze_system"]
                clocks = {}
                gaps = {}
                for name in ("source-writer", "runtime-reader"):
                    report = read_json(runtime, name + ".json")
                    clock_events = system.load_events(temporary / (name + ".csv"))
                    clocks[name] = system.inspect_clock(clock_events, report)
                    gaps[name] = [
                        {
                            "index": i,
                            "delta_frames": current["sample_time"]
                            - prior["sample_time"]
                            - prior["frames"],
                            "previous": prior,
                            "current": current,
                        }
                        for i, (prior, current) in enumerate(
                            zip(clock_events, clock_events[1:]), start=1
                        )
                        if abs(
                            current["sample_time"]
                            - prior["sample_time"]
                            - prior["frames"]
                        )
                        > 0.001
                    ]
                    recorded_gaps = read_json(
                        runtime, name + "-diagnostics-analysis.json"
                    )["sample_gap_context"]
                    assert [(g["index"], g["delta_frames"]) for g in gaps[name]] == [
                        (g["current"]["index"], g["delta_frames"])
                        for g in recorded_gaps
                    ]
                assert (
                    clocks["source-writer"]
                    == read_json(runtime, "source-writer-analysis.json")["clock"]
                )
                assert [g["delta_frames"] for g in gaps["source-writer"]] == [
                    1472.0,
                    576.0,
                    580.0,
                    572.0,
                ]
                assert [g["delta_frames"] for g in gaps["runtime-reader"]] == [764.0]
                try:
                    module.analyze(temporary)
                except ValueError as error:
                    reproduced = {"pass": False, "observer_error": str(error)}
                else:
                    raise AssertionError(
                        "Original failed protocol unexpectedly produced a complete analysis"
                    )
                assert reproduced == analysis
            finally:
                sys.path.pop(0)
                sys.modules.pop("analyze_system", None)
                if previous is not None:
                    sys.modules["analyze_system"] = previous
        timebase = read_json(runtime, "runtime-reader.json")
        scale = timebase["host_time_numer"] / timebase["host_time_denom"] / 1e9
        completion = output["last_completion"]
        packet = completion["packet"]
        callback_timing = {
            "callback_index": packet["callback_index"],
            "frames": packet["frames"],
            "entry_host_time": packet["entry_host_time"],
            "completion_host_time": completion["completion_host_time"],
            "presentation_host_time": packet["presentation"]["host_time"],
            "overload_observed_host_time": fault["metadata"]["observed_mach_host_time"],
            "timebase_numer": timebase["host_time_numer"],
            "timebase_denom": timebase["host_time_denom"],
            "callback_wall_seconds": (
                completion["completion_host_time"] - packet["entry_host_time"]
            )
            * scale,
            "nominal_audio_seconds": packet["frames"] / timebase["sample_rate"],
            "completion_minus_presentation_seconds": (
                completion["completion_host_time"] - packet["presentation"]["host_time"]
            )
            * scale,
            "overload_observation_minus_completion_seconds": (
                fault["metadata"]["observed_mach_host_time"]
                - completion["completion_host_time"]
            )
            * scale,
            "scope": "Elapsed callback wall time and supplied presentation timestamp; not CPU use, physical playback timing, fault onset or causal attribution.",
        }
        assert (
            callback_timing["callback_wall_seconds"]
            > callback_timing["nominal_audio_seconds"]
        )
        process = next(p for p in acceptance["processes"] if p["name"] == "runtime")
        assert (
            process["exit_code"] == 1
            and not process["timed_out"]
            and not process["term_sent"]
            and not process["kill_sent"]
        )
        return {
            "schema": 1,
            "source_revision": SOURCE,
            "run_id": 40251052292,
            "overall_conclusion": "FAIL",
            "hal_conclusion": "PASS",
            "runtime_conclusion": "FAIL",
            "original_observer_error_recomputed_exactly": True,
            "complete_pcm_protocol_analysis_available": False,
            "analyzer_sources": source_identities,
            "native_timing_rows": len(records),
            "first_fault_row_index": first_index,
            "first_fault": fault,
            "first_fault_immutable_through_terminal": True,
            "terminal_result": result,
            "runtime_process": process,
            "last_output_callback_timing": callback_timing,
            "protocol_event_counts": {"started": 1, "phase": 1, "stop": 0},
            "output_cleanup": {
                "successful_native_api_statuses": True,
                "zero_callbacks_in_flight_at_drain_and_completion": True,
                "retained_context": False,
                "lifecycle": output["lifecycle"],
                "strict_shutdown_gate": False,
                "strict_gate_failure_reason": "Global Xrun remains latched; successful API cleanup does not clear the fault or make the run pass.",
            },
            "capture_fault_path": {
                "packets_seen": timing["packets_seen"],
                "captured_frames": capture["captured_frames"],
                "first_fault": timing["first_fault"],
                "lifecycle": timing["lifecycle"],
                "scope": "This source implements OwnerCompleted after release. The recorded fault-path completion does not qualify all lifecycle paths or the complete mixing protocol.",
            },
            "independent_clocks_recomputed": clocks,
            "independent_clock_gaps": gaps,
            "driver_cleanup": cleanups,
            "uncertainty": "Output overload attribution identifies where the failure was recorded, not the scheduling, host, driver, processing or observer cause. Same-run clock gaps and the source change do not establish a capture regression. HAL PASS was a separate hosted job; runtime protocol remains incomplete and failed.",
        }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--archives",
        type=Path,
        default=ROOT / "build/rust-capture-owner-observation-40251052292",
    )
    parser.add_argument("--record", action="store_true")
    args = parser.parse_args()
    collection = json.loads((HERE / "collection.json").read_text())
    assert (
        collection["source_revision"] == SOURCE
        and collection["overall_conclusion"] == "FAIL"
    )
    total = 0
    for item in collection["archives"]:
        path = args.archives / item["archive"]
        assert digest(path.read_bytes()) == {k: item[k] for k in ("bytes", "sha256")}
        with zipfile.ZipFile(path) as archive:
            assert archive.testzip() is None
            if item["kind"] == "workflow-logs":
                continue
            manifest = read_json(archive, "custody-manifest.json")
            assert manifest["source_revision"] == SOURCE
            names = {"custody-manifest.json"}
            for member in manifest["files"]:
                names.add(member["path"])
                assert digest(archive.read(member["path"])) == {
                    k: member[k] for k in ("bytes", "sha256")
                }
                total += 1
            assert set(archive.namelist()) == names and len(archive.namelist()) == len(
                names
            )
            for name in item["compact_original_text_members"]:
                assert (
                    HERE / (item["kind"] + "-original-text") / name
                ).read_bytes() == archive.read(name)
    derived = derive(args.archives)
    target = HERE / "derived-verification.json"
    if args.record and not target.exists():
        target.write_text(json.dumps(derived, indent=2, sort_keys=True) + "\n")
    assert json.loads(target.read_text()) == derived
    print(
        json.dumps(
            {
                "original_archives_verified": 3,
                "original_manifest_files_verified": total,
                "original_observer_error_recomputed_exactly": True,
                "independent_clock_gaps_recomputed": True,
                "complete_pcm_protocol_analysis_available": False,
                "cleanup_observations_verified": True,
                "overall_result": "FAIL",
                "audio_or_network_operations": False,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
