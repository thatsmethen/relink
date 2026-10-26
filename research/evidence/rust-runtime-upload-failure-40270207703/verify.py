#!/usr/bin/env python3
"""Verify original custody and bounded conclusions offline; never rerun audio.

The unavailable runtime artifact is deliberately not reconstructed. This checks
original hashes/reports/logs, not a replacement runtime experiment or PCM result.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SOURCE = "dbab3fdeefe1c4833f8b56f3fb00cbe49cf3d0e1"
RUN = 40270207703


def digest(raw):
    return {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}


def read(name):
    return json.loads((HERE / name).read_bytes())


def verify(archives):
    custody = read("custody.json")
    assert custody["source_revision"] == SOURCE and custody["run_id"] == RUN
    for name, expected in custody["compact_original_files"].items():
        assert digest((HERE / name).read_bytes()) == expected, name
    for name, expected in custody["archives"].items():
        assert digest((archives / name).read_bytes()) == {k: expected[k] for k in ("bytes", "sha256")}, name
        with zipfile.ZipFile(archives / name) as archive:
            assert archive.testzip() is None
            assert len(archive.infolist()) == len(expected["members"])
            assert set(archive.namelist()) == set(expected["members"])
            for member, identity in expected["members"].items():
                assert digest(archive.read(member)) == identity, member
            for local, original in custody["compact_copies"].items():
                if original["archive"] == name:
                    assert (HERE / local).read_bytes() == archive.read(original["member"]), local
    artifacts = read("original-artifacts.json")
    assert artifacts["total_count"] == len(artifacts["artifacts"]) == 1
    artifact = artifacts["artifacts"][0]
    assert artifact["name"] == f"original-rust-hal-{RUN}-1"
    assert artifact["workflow_run"]["head_sha"] == SOURCE
    assert artifact["digest"] == "sha256:" + custody["archives"]["original-hal.zip"]["sha256"]
    with zipfile.ZipFile(archives / "original-hal.zip") as archive:
        manifest = json.loads(archive.read("custody-manifest.json"))
        assert manifest["source_revision"] == SOURCE and manifest["run_id"] == str(RUN)
        assert manifest["run_attempt"] == "1"
        assert len(manifest["files"]) == 65
        assert set(archive.namelist()) == {r["path"] for r in manifest["files"]} | {"custody-manifest.json"}
        for record in manifest["files"]:
            assert digest(archive.read(record["path"])) == {k: record[k] for k in ("bytes", "sha256")}
        acceptance = json.loads(archive.read("acceptance.json"))
        assert acceptance["pass"] is True and not acceptance["exceptions"]
        assert all(r["exit_code"] == 0 and not r["timed_out"] for r in acceptance["processes"])
        assert all(r["pass"] for r in acceptance["observations"])
        cleanup = json.loads(archive.read("cleanup.json"))
        assert cleanup["pass"] is True and len(cleanup["devices"]) == 2
        assert all(r["matches"] == 0 and r["status"] == 0 for r in cleanup["devices"].values())
        pair = json.loads(archive.read("independent-readers.json"))
        assert pair["pass"] and pair["maximum_difference"] == 0 and pair["differing_samples_above_1e-6"] == 0
        silent = json.loads(archive.read("restarted-silence-analysis.json"))
        assert silent["pass"] and silent["peak"] == 0
        idle = [json.loads(archive.read(f"installed-cable-{i}-0-discovery.json")) for i in (1, 2)]
        assert all(d["matches"] == 1 and d["device_running_status"] == 0
                   and d["device_is_running_somewhere"] is False for d in idle)
    for relative, expected in custody["verified_listed_source_files"].items():
        raw = subprocess.check_output(["git", "show", f"{SOURCE}:{relative}"], cwd=ROOT)
        assert digest(raw) == expected, relative
    assert (HERE / "source-workflow.yml").read_bytes() == subprocess.check_output(
        ["git", "show", f"{SOURCE}:.github/workflows/rust-hal.yml"], cwd=ROOT)
    workflow = read("original-run.json")
    assert workflow["head_sha"] == SOURCE and workflow["conclusion"] == "failure" and workflow["run_attempt"] == 1
    jobs = {job["name"]: job for job in read("original-jobs.json")["jobs"]}
    assert jobs["installed-hal"]["conclusion"] == "success"
    runtime = jobs["installed-runtime-capture"]
    steps = {step["number"]: step for step in runtime["steps"]}
    assert runtime["conclusion"] == "failure"
    assert [steps[i]["conclusion"] for i in (6, 7, 8, 9)] == ["failure", "success", "success", "failure"]
    runtime_log = (HERE / "runtime-step-original.txt").read_text(encoding="utf-8-sig")
    upload_log = (HERE / "runtime-upload-original.txt").read_text(encoding="utf-8-sig")
    assert "Process completed with exit code 1." in runtime_log
    assert "With the provided path, there will be 57 files uploaded" in upload_log
    assert "Failed to make request after 5 attempts: Request timeout:" in upload_log
    assert "ArtifactService/CreateArtifact" in upload_log
    check = read("original-runtime-check-run.json")
    assert check["head_sha"] == SOURCE and check["conclusion"] == "failure"
    assert all(check["output"][key] is None for key in ("title", "summary", "text"))
    failures = [a["message"] for a in read("original-runtime-annotations.json") if a["annotation_level"] == "failure"]
    assert len(failures) == 2
    assert "Process completed with exit code 1." in failures
    assert any("CreateArtifact" in message and "5 attempts" in message for message in failures)
    return {
        "schema": 1, "run_id": RUN, "source_revision": SOURCE, "run_attempt": 1,
        "overall": "FAIL", "hal": "PASS (original reports and archive custody verified)",
        "hal_manifested_files_verified": 65, "hal_archive_members_verified": 66,
        "hal_reader_overlap_frames": pair["overlap_frames"], "hal_reader_maximum_difference": 0.0,
        "hal_restarted_silence_peak": 0.0, "hal_cleanup_both_uids_absent_verified": True,
        "hal_idle_property_observed_before_io": True,
        "runtime_step_exit_code": 1, "runtime_audio_failure_cause": None,
        "runtime_audio_failure_cause_status": "Unrecoverable from available originals: no report/PCM/timing artifact, console gives exit code only.",
        "runtime_artifact_available": False, "runtime_original_manifest_available": False,
        "runtime_cleanup_step_conclusion": "success", "runtime_cleanup_raw_absence_verified": False,
        "upload_failure_stage": "ArtifactService/CreateArtifact", "upload_attempts_reported": 5,
        "upload_error": "Request timeout", "upload_selected_file_count_reported": 57,
        "no_runtime_substitute_or_rerun": True, "publication_performed": False,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--archives", type=Path, required=True)
    parser.add_argument("--write-derived", action="store_true")
    args = parser.parse_args()
    derived = verify(args.archives)
    if args.write_derived:
        with (HERE / "derived-verification.json").open("x") as output:
            json.dump(derived, output, indent=2)
            output.write("\n")
    else:
        assert derived == read("derived-verification.json")
    print(json.dumps(derived, indent=2))


if __name__ == "__main__":
    main()
