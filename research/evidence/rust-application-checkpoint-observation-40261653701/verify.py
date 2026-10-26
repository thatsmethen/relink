#!/usr/bin/env python3
"""Offline custody and observation recomputation from frozen source and observations.

No Git, network, device operations, original-file writes or experiment reruns.
Only original pure analysis functions are called. Temporary inputs are removed.
"""
import argparse
from collections import Counter
import base64
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
SOURCE = "353399309004a1be5a196ebaa68c08218aff0555"
RUN = 40261653701


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
                    assert acceptance["pass"] is True and not acceptance["exceptions"]
                    acceptances[kind] = acceptance
                    cleanup = read_json(archive, "cleanup.json")
                    assert cleanup["pass"] and len(cleanup["devices"]) == 2
                    assert all(d["matches"] == 0 for d in cleanup["devices"].values())
                    cleanups[kind] = {"pass": True, "both_owned_devices_absent": True, "attempt": cleanup["attempt"]}
                    original_reports = []
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
                        original_reports.append(read_json(archive, name + "-diagnostics-analysis.json"))
                        actual = diagnostics.summarize(target / (name + "-diagnostics.json"), target / (name + ".csv"), target / (name + ".json"))
                        recomputed[kind][name + "-diagnostics"] = compare_report(actual, read_json(archive, name + "-diagnostics-analysis.json"))
                        notifications[kind][name] = [e for e in actual["notifications"] if e["selector"] == 1870030194]
                        if kind == "hal" or name == "source-writer":
                            actual = system.analyze(target / (name + ".json"), target / (name + ".csv"),
                                None if report["mode"] == "play" else target / (name + ".wav"), silence=name == "restarted-silence")
                            recomputed[kind][name] = compare_report(actual, read_json(archive, name + "-analysis.json"))
                            original_reports.append(read_json(archive, name + "-analysis.json"))
                    if kind == "hal":
                        readers = [SimpleNamespace(wav=target / (n + ".wav"), events=target / (n + ".csv")) for n in ("reader-a", "reader-b")]
                        actual = harness.pair_consistency(*readers)
                        recomputed[kind]["independent-readers"] = compare_report(actual, read_json(archive, "independent-readers.json"))
                        original_reports.append(read_json(archive, "independent-readers.json"))
                    else:
                        actual = runtime_analyzer.analyze(target)
                        original = read_json(archive, "runtime-output-analysis.json")
                        assert actual == original, "Full original runtime analysis differs"
                        recomputed[kind]["runtime-output"] = compare_report(actual, original)
                        original_reports.append(original)
                    encode = lambda value: json.dumps(value, sort_keys=True, allow_nan=False)
                    assert Counter(map(encode, original_reports)) == Counter(map(encode, acceptance["observations"]))
                    assert all(p["exit_code"] == 0 and not p["timed_out"] and not p["term_sent"] and not p["kill_sent"] and p["supervision_error"] is None for p in acceptance["processes"])
                    for cable in (1, 2):
                        uid = f"au.textual.ravelink.cable.{cable}"
                        before = read_json(archive, f"before-cable-{cable}-discovery.json")
                        installed = read_json(archive, f"installed-cable-{cable}-0-discovery.json")
                        assert before["uid"] == installed["uid"] == uid
                        assert before["matches"] == 0 and installed["matches"] == 1
                        assert before["complete"] and installed["complete"]
                        assert before["status"] == installed["status"] == 0
        with zipfile.ZipFile(archives / "original-runtime.zip") as archive:
            result = read_json(archive, "runtime-result.json")
            assert result == {"schema": 1, "pass": True, "native_output_shutdown_proven": True, "error": None}
            lines = archive.read("runtime-native-timing.jsonl").splitlines(keepends=True)
            records = [json.loads(line) for line in lines]
            assert records and all(r["fault"] == 0 and r["first_fault"] is None and r["error"] is None for r in records)
            assert (HERE / "runtime-terminal-original.json").read_bytes() == lines[-1]
            terminal = records[-1]
            assert terminal["observation"] == "post-stop-final" and not terminal["running"]
            assert terminal["native_shutdown_observed"] and terminal["native_output_shutdown_observed"] and terminal["native_output_shutdown_proven"]
            assert len(terminal["captures"]) == 1
            capture = terminal["captures"][0]
            timing = capture["native_timing"]
            output = terminal["output_timing"]
            assert capture["input_id"] == "coreaudio:au.textual.ravelink.cable.1"
            assert timing["first_fault"] is None and timing["packets_seen"] > 0
            assert capture["captured_frames"] > 4096
            assert output["first_fault"] is None and not output["first_fault_pending"]
            assert output["callbacks_seen"] > 0 and output["progress_count"] > 0
            lifecycles = {}
            for label, owner, listener in (("input", timing, "ListenersRemovalReturned"), ("output", output, "ListenerRemovalReturned")):
                phases = {e["phase"]: e for e in owner["lifecycle"]}
                assert len(phases) == len(owner["lifecycle"])
                assert "ContextRetained" not in phases
                assert all(phases[p]["status"] == 0 for p in ("CreateReturned", "StartReturned", "StopReturned", "DestroyReturned", listener))
                assert all(phases[p]["callbacks_in_flight"] == 0 for p in ("QuiescenceChecked", "OwnerCompleted"))
                assert phases["OwnerCompleted"]["host_time"] >= phases["QuiescenceChecked"]["host_time"] > 0
                lifecycles[label] = owner["lifecycle"]
            for row in records:
                assert len(row["captures"]) == 1
                input_timing = row["captures"][0]["native_timing"]
                if input_timing is not None:
                    assert input_timing["first_fault"] is None
                output_timing = row["output_timing"]
                if output_timing is not None:
                    assert output_timing["first_fault"] is None and not output_timing["first_fault_pending"]
            events = [json.loads(l) for l in archive.read("runtime-events.jsonl").splitlines()]
            assert [e["event"] for e in events] == ["started", "phase", "phase", "phase", "phase", "stopped"]
            assert [e["phase"] for e in events[1:-1]] == [0, 1, 2, 3]
            assert [(e["gain_db"], e["muted"]) for e in events[1:-1]] == [(-6,False),(-12,False),(-12,True),(-6,False)]
            assert events[0]["input_id"] == capture["input_id"] and events[0]["output_id"] == "coreaudio:au.textual.ravelink.cable.2" and events[0]["sample_rate"] == 48000
            ready = read_json(archive, "runtime.ready.json")
            assert ready["ready"] and ready["captured_frames"] >= 4096 and ready["rendered_frames"] >= 4096
            assert all(not gaps[kind][name] and clocks[kind][name]["pass"] for kind in clocks for name in clocks[kind])
            scale_report = read_json(archive, "runtime-reader.json")
            scale = scale_report["host_time_numer"] / scale_report["host_time_denom"] / 1e9
            reader = {k: scale_report[k] for k in ("frames", "events", "complete", "fault", "sample_rate", "channels")}
        return {
            "schema": 1, "source_revision": SOURCE, "run_id": RUN,
            "overall_conclusion": "PASS", "hal_conclusion": "PASS", "runtime_conclusion": "PASS",
            "scope": "One existing Cable1 hardware/virtual input -> mixer -> one Cable2 output protocol at the application-code checkpoint. No process/application capture, simultaneous outputs, Windows audio or physical playback was exercised.",
            "original_analysis_recomputation": recomputed,
            "analysis_comparison": "Every original analysis field matched except descriptive source_report paths in system reports; those original paths are retained in this record. The complete runtime PCM/control/Stop report matched exactly with no field exclusions. No threshold, metric or outcome changed.",
            "complete_runtime_pcm_protocol_analysis_available": True,
            "runtime_analysis_recomputed_without_field_exclusions": True,
            "aggregate_acceptance_observations_match_original_reports": True,
            "independent_clocks_recomputed": clocks, "independent_clock_gaps": gaps,
            "native_timing_rows": len(records), "no_global_or_native_fault_observed": True,
            "runtime_result": result, "runtime_events": events, "runtime_ready": ready,
            "runtime_reader": reader,
            "runtime_processes": acceptances["runtime"]["processes"], "hal_processes": acceptances["hal"]["processes"],
            "runtime_cleanup": {"successful_native_api_statuses": True, "zero_callbacks_in_flight_at_drain_and_completion": True,
                "retained_context": False, "strict_output_shutdown_gate": True,
                "input_shutdown_observed": terminal["native_shutdown_observed"],
                "input_shutdown_limit": "No separate strict input-shutdown boolean is reported; original lifecycle observations are preserved.",
                "lifecycles": lifecycles},
            "runtime_input": {"packets_seen": timing["packets_seen"], "captured_frames": capture["captured_frames"], "packet_timing_first_fault": timing["first_fault"], "last_completion": timing["last_completed_callback"]},
            "runtime_output": {"callbacks_seen": output["callbacks_seen"], "progress_count": output["progress_count"], "packet_timing_first_fault": output["first_fault"], "last_completion": output["last_completion"]},
            "runtime_stop_request_to_return_seconds": (events[-1]["returned_host_time"] - events[-1]["requested_host_time"]) * scale,
            "runtime_started_to_stop_request_seconds": (events[-1]["requested_host_time"] - events[0]["host_time"]) * scale,
            "overload_notification_observations": notifications,
            "driver_cleanup": cleanups,
            "uncertainty": "This short hosted virtual-device observation does not qualify the new macOS process taps, Windows process loopback, application identity/permissions, physical playback, long-duration/load/restart reliability or production completion. HAL and runtime were separate jobs. Earlier failures remain unchanged; this is not a controlled comparison or proof of their cause/remedy.",
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
    parser.add_argument("--archives", type=Path, default=ROOT / "build/rust-application-checkpoint-observation-40261653701")
    parser.add_argument("--record", action="store_true")
    args = parser.parse_args()
    compact_manifest = HERE / "SHA256SUMS.json"
    if compact_manifest.exists():
        compact = json.loads(compact_manifest.read_text())
        excluded = {"SHA256SUMS.json", "SHA256SUMS.txt"}
        actual_names = {p.relative_to(HERE).as_posix() for p in HERE.rglob("*") if p.is_file() and p.name not in excluded}
        assert actual_names == set(compact), "Compact record membership changed"
        for name, expected in compact.items():
            assert digest((HERE / name).read_bytes()) == expected, name
        expected_text = "".join(f"{item['sha256']}  {name}\n" for name, item in sorted(compact.items()))
        assert (HERE / "SHA256SUMS.txt").read_text() == expected_text
    collection = json.loads((HERE / "collection.json").read_text())
    assert collection["source_revision"] == SOURCE and collection["overall_conclusion"] == "PASS"
    # Bind all preserved API bodies/source blobs and the source tree, offline.
    originals = json.loads((HERE / "original-local-files.json").read_text())
    for name, expected in originals.items():
        assert digest((args.archives / name).read_bytes()) == expected, name
    run = json.loads((args.archives / "original-run.json").read_text())
    jobs = json.loads((args.archives / "original-jobs.json").read_text())
    artifacts = json.loads((args.archives / "original-artifacts.json").read_text())["artifacts"]
    assert run["id"] == RUN and run["head_sha"] == SOURCE and run["run_attempt"] == 1
    assert run["status"] == "completed" and run["conclusion"] == "success"
    assert {j["name"]: j["conclusion"] for j in jobs["jobs"]} == {"installed-hal": "success", "installed-runtime-capture": "success"}
    commit = json.loads((args.archives / "original-source-commit.json").read_text())
    tree = json.loads((args.archives / "original-source-tree.json").read_text())
    assert commit["sha"] == SOURCE and tree["sha"] == commit["tree"]["sha"] == collection["source_tree"] and not tree["truncated"]
    entries = {e["path"]: e for e in tree["tree"]}
    source_ids = json.loads((HERE / "pinned-source-identities.json").read_text())
    for source in source_ids["files"]:
        assert entries[source["path"]]["sha"] == source["git_blob_sha"]
        blob = json.loads((args.archives / ("original-source-blob-" + source["git_blob_sha"] + ".json")).read_text())
        assert blob["encoding"] == "base64" and blob["sha"] == source["git_blob_sha"]
        assert base64.b64decode(blob["content"]) == (HERE / "pinned-source" / source["path"]).read_bytes()
    assert digest((HERE / "collect.py").read_bytes())["sha256"] == collection["collector_sha256"]
    total = 0
    for item in collection["archives"]:
        path = args.archives / item["archive"]
        assert digest(path.read_bytes()) == {k: item[k] for k in ("bytes", "sha256")}
        with zipfile.ZipFile(path) as archive:
            assert archive.testzip() is None
            if item["kind"] == "workflow-logs":
                continue
            original_artifact = next(a for a in artifacts if a["id"] == item["artifact_id"])
            assert original_artifact["digest"] == "sha256:" + item["sha256"] == item["github_digest"]
            assert original_artifact["workflow_run"]["head_sha"] == SOURCE
            # Check every source hash present in each job's original build manifest.
            built = dict((line.split()[1], line.split()[0]) for line in archive.read("build-sha256.txt").decode().splitlines())
            for source in source_ids["files"]:
                if source["path"] in built:
                    assert built[source["path"]] == source["sha256"]
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
    recovery = json.loads((HERE / "retrieval-verification.json").read_text())
    prior = args.archives / "failed-downloads/preexisting-hal-download.partial"
    assert digest(prior.read_bytes()) == recovery["prior_partial"]["observed_bytes"]
    assert (args.archives / "original-hal.zip").read_bytes().startswith(prior.read_bytes())
    assert not recovery["prior_partial"]["accepted"] and recovery["prior_partial_is_exact_prefix_of_verified_hal"]
    for attempt in recovery["fresh_attempts"]:
        assert attempt["accepted"] and not attempt["timed_out"] and attempt["exit_code"] == 0
        assert digest((args.archives / attempt["fresh_destination"]).read_bytes()) == {k: attempt[k] for k in ("bytes", "sha256")}
    derived = derive(args.archives)
    path = HERE / "derived-verification.json"
    if args.record and not path.exists():
        with path.open("x") as file:
            file.write(json.dumps(derived, indent=2, sort_keys=True, allow_nan=False) + "\n")
    assert json.loads(path.read_text()) == derived
    print(json.dumps({"original_archives_verified": 3, "original_manifest_files_verified": total,
        "original_analysis_reports_recomputed": sum(len(v) for v in derived["original_analysis_recomputation"].values()),
        "runtime_analysis_recomputed_without_field_exclusions": True, "complete_runtime_pcm_protocol_analysis_available": True,
        "outcomes": {"overall": "PASS", "hal": "PASS", "runtime": "PASS"},
        "git_network_or_audio_operations": False}, indent=2))


if __name__ == "__main__":
    main()
