#!/usr/bin/env python3
"""Offline custody and failure recomputation from frozen source and observations.

No Git, network, device operations, original-file writes or experiment reruns.
Only original pure analysis functions are called. Temporary inputs are removed.
"""
import argparse
import hashlib
import importlib
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SOURCE = "902118a60b070bb9cefdef75d5b477b5350b9847"
RUN = 40254943736


def digest(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def read_json(archive, name):
    return json.loads(archive.read(name))


def compare_report(actual, original):
    # Recomputed inputs are temporary files. Exclude only their descriptive
    # source path; every outcome, threshold, sample metric and error must match.
    excluded = original.get("source_report")
    left = {k: v for k, v in actual.items() if k != "source_report"}
    right = {k: v for k, v in original.items() if k != "source_report"}
    assert left == right, "Original and recomputed analysis differ"
    return {
        "all_analysis_fields_match": True,
        "descriptive_source_path_excluded": excluded,
        "recomputed_analysis": left,
    }


def derive(archives):
    identities = json.loads((HERE / "pinned-source-identities.json").read_text())
    assert identities["source_revision"] == SOURCE
    for item in identities["files"]:
        data = (HERE / "pinned-source" / item["path"]).read_bytes()
        assert digest(data) == {k: item[k] for k in ("bytes", "sha256")}
        assert hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() == item["git_blob_sha"]
    modules = ("analyze_system", "analyze_runtime", "analyze_diagnostics", "run_system")
    previous = {name: sys.modules.pop(name, None) for name in modules}
    sys.path.insert(0, str(HERE / "pinned-source/drivers/macos/tests"))
    prior_bytecode = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        system, runtime_analyzer, diagnostics, harness = [importlib.import_module(name) for name in modules]
        recomputed = {}
        clocks = {}
        gaps = {}
        notifications = {}
        cleanups = {}
        acceptances = {}
        with tempfile.TemporaryDirectory(prefix="relink-frozen-single-") as scratch:
            scratch = Path(scratch)
            for kind in ("hal", "runtime"):
                target = scratch / kind
                target.mkdir()
                with zipfile.ZipFile(archives / f"original-{kind}.zip") as archive:
                    # Copy only flat observation files, never a native executable.
                    for name in archive.namelist():
                        path = Path(name)
                        if len(path.parts) == 1 and path.suffix in (".json", ".jsonl", ".csv", ".wav"):
                            (target / name).write_bytes(archive.read(name))
                    acceptance = read_json(archive, "acceptance.json" if kind == "hal" else "runtime-acceptance.json")
                    assert not acceptance["pass"] and not acceptance["exceptions"]
                    acceptances[kind] = acceptance
                    cleanup = read_json(archive, "cleanup.json")
                    assert cleanup["pass"] and len(cleanup["devices"]) == 2
                    assert all(d["matches"] == 0 for d in cleanup["devices"].values())
                    cleanups[kind] = {"pass": True, "both_owned_devices_absent": True, "attempt": cleanup["attempt"]}
                    names = ("tone-writer", "reader-a", "reader-b", "restarted-silence") if kind == "hal" else ("source-writer", "runtime-reader")
                    recomputed[kind], clocks[kind], gaps[kind], notifications[kind] = {}, {}, {}, {}
                    for name in names:
                        report = read_json(archive, name + ".json")
                        events = system.load_events(target / (name + ".csv"))
                        clocks[kind][name] = system.inspect_clock(events, report)
                        gaps[kind][name] = [
                            {"current_callback_index": i, "delta_frames": c["sample_time"] - p["sample_time"] - p["frames"], "previous": p, "current": c}
                            for i, (p, c) in enumerate(zip(events, events[1:]), 1)
                            if abs(c["sample_time"] - p["sample_time"] - p["frames"]) > 0.001
                        ]
                        actual = diagnostics.summarize(target / (name + "-diagnostics.json"), target / (name + ".csv"), target / (name + ".json"))
                        recomputed[kind][name + "-diagnostics"] = compare_report(actual, read_json(archive, name + "-diagnostics-analysis.json"))
                        notifications[kind][name] = [e for e in actual["notifications"] if e["selector"] == 1870030194]
                        if kind == "hal" or name == "source-writer":
                            actual = system.analyze(target / (name + ".json"), target / (name + ".csv"),
                                None if report["mode"] == "play" else target / (name + ".wav"), silence=name == "restarted-silence")
                            recomputed[kind][name] = compare_report(actual, read_json(archive, name + "-analysis.json"))
                    if kind == "hal":
                        readers = [SimpleNamespace(wav=target / (n + ".wav"), events=target / (n + ".csv")) for n in ("reader-a", "reader-b")]
                        actual = harness.pair_consistency(*readers)
                        recomputed[kind]["independent-readers"] = compare_report(actual, read_json(archive, "independent-readers.json"))
                    else:
                        try:
                            runtime_analyzer.analyze(target)
                        except ValueError as error:
                            actual = {"pass": False, "observer_error": str(error)}
                        else:
                            raise AssertionError("Incomplete original protocol unexpectedly produced a complete analysis")
                        recomputed[kind]["runtime-output"] = compare_report(actual, read_json(archive, "runtime-output-analysis.json"))
        with zipfile.ZipFile(archives / "original-runtime.zip") as archive:
            result = read_json(archive, "runtime-result.json")
            assert result == {"schema": 1, "pass": False, "native_output_shutdown_proven": False, "error": "runtime stopped: fault=Xrun, error=None"}
            lines = archive.read("runtime-native-timing.jsonl").splitlines(keepends=True)
            records = [json.loads(line) for line in lines]
            assert len(records) == 167
            index = next(i for i, r in enumerate(records) if r["first_fault"] is not None)
            assert index == 165
            fault = records[index]["first_fault"]
            assert fault["origin"] == "NativeInputOverload" and fault["direction"] == "Input" and fault["fault"] == 5
            assert fault["endpoint_id"] == "coreaudio:au.textual.ravelink.cable.1" and fault["endpoint_index"] == 0
            assert fault["metadata"] == {"epoch": 1, "native_object": 92, "observed_mach_host_time": 6197399594, "packet_index": None,
                "property": {"element": 0, "scope": 1735159650, "selector": 1870030194}, "status": None}
            assert not fault["metadata_pending"] and all(r["first_fault"] == fault and r["fault"] == 5 for r in records[index:])
            assert (HERE / "runtime-first-fault-original.json").read_bytes() == lines[index]
            assert (HERE / "runtime-terminal-original.json").read_bytes() == lines[-1]
            terminal = records[-1]
            assert terminal["observation"] == "post-stop-final" and not terminal["running"] and terminal["error"] is None
            assert terminal["native_shutdown_observed"] and terminal["native_output_shutdown_observed"] and not terminal["native_output_shutdown_proven"]
            assert len(terminal["captures"]) == 1
            capture = terminal["captures"][0]
            timing = capture["native_timing"]
            output = terminal["output_timing"]
            assert capture["input_id"] == "coreaudio:au.textual.ravelink.cable.1"
            assert capture["captured_frames"] == 413696 and timing["packets_seen"] == 808 and timing["first_fault"] is None
            assert output["callbacks_seen"] == 814 and output["progress_count"] == 813
            assert output["first_fault"] is None and not output["first_fault_pending"]
            assert output["last_completion"]["packet"]["callback_index"] == 814
            assert output["last_completion"]["outcome"] == {"kind": "cancelled"}
            lifecycles = {}
            for label, owner, listener in (("input", timing, "ListenersRemovalReturned"), ("output", output, "ListenerRemovalReturned")):
                phases = {e["phase"]: e for e in owner["lifecycle"]}
                assert "ContextRetained" not in phases
                assert all(phases[p]["status"] == 0 for p in ("StartReturned", "StopReturned", "DestroyReturned", listener))
                assert all(phases[p]["callbacks_in_flight"] == 0 for p in ("QuiescenceChecked", "OwnerCompleted"))
                lifecycles[label] = owner["lifecycle"]
            events = [json.loads(l) for l in archive.read("runtime-events.jsonl").splitlines()]
            assert [e["event"] for e in events] == ["started", "phase", "phase"]
            assert [e["phase"] for e in events[1:]] == [0, 1]
            report = read_json(archive, "runtime-reader.json")
            scale = report["host_time_numer"] / report["host_time_denom"] / 1e9
            observed = fault["metadata"]["observed_mach_host_time"]
            nearby = {name: [{**e, "seconds_relative_to_latched_fault_observation": (e["host_time"] - observed) * scale} for e in es]
                for name, es in notifications["runtime"].items()}
        return {
            "schema": 1, "source_revision": SOURCE, "run_id": RUN,
            "overall_conclusion": "FAIL", "hal_conclusion": "FAIL", "runtime_conclusion": "FAIL",
            "scope": "Single-output capture/mix protocol at the source containing multi-output code. This run did not exercise simultaneous outputs.",
            "original_analysis_recomputation": recomputed,
            "analysis_comparison": "Every original analysis field matched exactly except descriptive source_report paths, which necessarily name temporary local inputs and are explicitly excluded. No metric, threshold or outcome is changed.",
            "complete_runtime_pcm_protocol_analysis_available": False,
            "independent_clocks_recomputed": clocks, "independent_clock_gaps": gaps,
            "runtime_first_fault": fault, "first_fault_zero_based_row": index, "native_timing_rows": len(records),
            "first_fault_immutable_through_terminal": True, "runtime_result": result, "runtime_events": events,
            "runtime_processes": acceptances["runtime"]["processes"], "hal_processes": acceptances["hal"]["processes"],
            "runtime_cleanup": {"successful_native_api_statuses": True, "zero_callbacks_in_flight_at_drain_and_completion": True,
                "retained_context": False, "strict_output_shutdown_gate": False,
                "strict_gate_failure_reason": "Global Xrun remains latched despite successful API cleanup; this does not make the run pass.", "lifecycles": lifecycles},
            "runtime_input": {"packets_seen": timing["packets_seen"], "captured_frames": capture["captured_frames"], "packet_timing_first_fault": timing["first_fault"], "last_completion": timing["last_completed_callback"]},
            "runtime_output": {"callbacks_seen": output["callbacks_seen"], "progress_count": output["progress_count"], "packet_timing_first_fault": output["first_fault"], "last_completion": output["last_completion"]},
            "runtime_overload_notification_observations": nearby, "hal_overload_notification_observations": notifications["hal"],
            "driver_cleanup": cleanups,
            "uncertainty": "First-fault attribution identifies the winning observer, not fault onset or root cause. Similar same-job clock/PCM failures and nearby overload notifications do not establish host, driver, instrumentation or application causality, or a regression from changed code. HAL and runtime are separate jobs with unrelated host clocks. Diagnostic integrity PASS does not override PCM/clock/runtime failures.",
        }
    finally:
        sys.dont_write_bytecode = prior_bytecode
        sys.path.pop(0)
        for name, old in previous.items():
            sys.modules.pop(name, None)
            if old is not None:
                sys.modules[name] = old


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--archives", type=Path, default=ROOT / "build/rust-multioutput-single-observation-40254943736")
    parser.add_argument("--record", action="store_true")
    args = parser.parse_args()
    collection = json.loads((HERE / "collection.json").read_text())
    assert collection["source_revision"] == SOURCE and collection["overall_conclusion"] == "FAIL"
    total = 0
    for item in collection["archives"]:
        path = args.archives / item["archive"]
        assert digest(path.read_bytes()) == {k: item[k] for k in ("bytes", "sha256")}
        with zipfile.ZipFile(path) as archive:
            assert archive.testzip() is None
            if item["kind"] == "workflow-logs":
                continue
            manifest = read_json(archive, "custody-manifest.json")
            assert manifest["source_revision"] == SOURCE and int(manifest["run_id"]) == RUN and int(manifest["run_attempt"]) == 1
            names = {"custody-manifest.json"}
            for member in manifest["files"]:
                names.add(member["path"])
                assert digest(archive.read(member["path"])) == {k: member[k] for k in ("bytes", "sha256")}
                total += 1
            assert set(archive.namelist()) == names and len(archive.namelist()) == len(names)
            for name in item["compact_original_text_members"]:
                assert (HERE / (item["kind"] + "-original-text") / name).read_bytes() == archive.read(name)
    derived = derive(args.archives)
    path = HERE / "derived-verification.json"
    if args.record and not path.exists():
        with path.open("x") as file:
            file.write(json.dumps(derived, indent=2, sort_keys=True, allow_nan=False) + "\n")
    assert json.loads(path.read_text()) == derived
    print(json.dumps({"original_archives_verified": 3, "original_manifest_files_verified": total,
        "original_analysis_reports_recomputed": sum(len(v) for v in derived["original_analysis_recomputation"].values()),
        "original_observer_error_reproduced_exactly": True, "complete_runtime_pcm_protocol_analysis_available": False,
        "outcomes": {"overall": "FAIL", "hal": "FAIL", "runtime": "FAIL"},
        "git_network_or_audio_operations": False}, indent=2))


if __name__ == "__main__":
    main()
