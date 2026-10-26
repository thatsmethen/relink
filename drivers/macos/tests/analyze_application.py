#!/usr/bin/env python3
"""Separate application protocol; existing hardware observations remain untouched.

PCM/control checks below preserve analyze_runtime's orchestration and import its
original constants and metrics. Only the source identity check is different.
The independent Cable1 witness additionally proves that exclusion was exercised.
"""
from __future__ import annotations
import argparse
import array
import csv
import hashlib
import json
import math
from pathlib import Path

from analyze_runtime import (CaptureTimeline, FREQUENCIES, SOURCE_AMPLITUDE,
                             PHASES, LIMITS, tone_metrics, first_envelope)
from analyze_system import RATE, inspect_clock, load_events, load_wav, longest_zero_run
from application_protocol import (PROTOCOL, CABLE1, CABLE2, TWO_PROCESS_FORMAT,
                                  WRITERS, selector_matches, validate_owned_identities, SOURCE_SECONDS, RECORD_SECONDS)

def analyze_runtime_output(directory: Path, selected_bundle: str) -> dict:
    report = json.loads((directory / "runtime-reader.json").read_text())
    events = load_events(directory / "runtime-reader.csv")
    samples = load_wav(directory / "runtime-reader.wav")
    runtime = json.loads((directory / "runtime-result.json").read_text())
    records = [json.loads(line) for line in (directory / "runtime-events.jsonl").read_text().splitlines()]
    phases = [record for record in records if record["event"] == "phase"]
    starts = [record for record in records if record["event"] == "started"]
    stops = [record for record in records if record["event"] == "stopped"]
    failures, results = [], []
    clock = inspect_clock(events, report)
    if not clock["pass"]:
        failures.append("independent recorder clock/continuity failed")
    if (runtime.get("schema") != 1 or runtime.get("protocol") != PROTOCOL
            or runtime.get("pass") is not True or runtime.get("error") is not None
            or runtime.get("native_capture_shutdown_proven") is not True
            or runtime.get("native_output_shutdown_proven") is not True):
        failures.append("runtime result schema/protocol/success/cleanup flags are invalid")
    if any(report.get(key) != 0 for key in ("status", "stop_status", "destroy_status", "fault")) or not report.get("complete") or not report.get("ready"):
        failures.append("independent recorder reported incomplete/faulted execution")
    if report.get("uid") != "au.textual.ravelink.cable.2" or report.get("matches") != 1 or report.get("sample_rate") != RATE or report.get("channels") != 2:
        failures.append("independent recorder did not select exactly Cable2")
    frames = sum(event["stored_frames"] for event in events)
    if frames != report.get("frames") or frames != report.get("target_frames") or len(samples) != frames * 2 or len(events) != report.get("events"):
        raise ValueError("PCM/event/frame custody mismatch")
    if frames != RECORD_SECONDS * RATE:
        failures.append("independent recorder duration differs from fixed application protocol")
    if len(phases) != len(PHASES) or len(starts) != 1 or len(stops) != 1:
        raise ValueError("missing or repeated runtime protocol events")
    if (not selector_matches(starts[0].get("input_id"), selected_bundle)
            or starts[0].get("output_id") != "coreaudio:" + CABLE2
            or starts[0].get("sample_rate") != RATE
            or starts[0].get("protocol") != PROTOCOL
            or starts[0].get("expected_current_bundle_audio_processes") != 2
            or starts[0].get("input_format") != TWO_PROCESS_FORMAT):
        failures.append("runtime application identity/two-process configuration differs from fixed protocol")
    scale = report["host_time_numer"] / report["host_time_denom"] / 1e9
    timeline = CaptureTimeline(events, scale, frames)
    frame = timeline.frame
    previous_version = 0
    for index, (phase, expected) in enumerate(zip(phases, PHASES)):
        gain, muted, seconds = expected
        if phase.get("phase") != index or phase.get("gain_db") != gain or phase.get("muted") is not muted:
            failures.append(f"phase {index} controls differ from fixed protocol")
        requested, acknowledged = phase["requested_host_time"], phase["acknowledged_host_time"]
        if requested > acknowledged or phase["version"] <= previous_version:
            failures.append(f"phase {index} invalid callback acknowledgement")
        previous_version = phase["version"]
        acknowledgement_seconds = (acknowledged - requested) * scale
        if acknowledgement_seconds > LIMITS["control_ack_seconds"]:
            failures.append(f"phase {index} callback acknowledgement exceeded fixed deadline")
        boundary = phases[index + 1]["requested_host_time"] if index + 1 < len(phases) else stops[0]["requested_host_time"]
        duration = (boundary - acknowledged) * scale
        if not seconds - 0.02 <= duration <= seconds + 0.5:
            failures.append(f"phase {index} duration differs from fixed protocol")
        begin, end = frame(acknowledged, 0.25), frame(boundary, -0.05)
        if begin >= end:
            raise ValueError("phase has no steady interval")
        if muted:
            peak = max(abs(value) for value in samples[begin * 2:end * 2])
            result = {"pass": peak == 0.0, "peak": peak, "begin_frame": begin, "end_frame": end}
        else:
            result = tone_metrics(samples, begin, end, gain)
        expected_amplitude = 0.0 if muted else SOURCE_AMPLITUDE * 10 ** (gain / 20)
        onset = first_envelope(samples, frame(requested, -0.05),
                               frame(requested, LIMITS["transition_seconds"]), expected_amplitude)
        result.update(phase=index, acknowledgement_seconds=acknowledgement_seconds,
            detected_response_seconds=None if onset is None else timeline.response_seconds(onset, requested))
        if onset is None:
            result["pass"] = False
            failures.append(f"phase {index} output did not reach expected envelope by fixed deadline")
        if not result["pass"]:
            failures.append(f"phase {index} PCM failed")
        results.append(result)
    # This range crosses the live -6 -> -12 dB handoff. It must never become
    # silent: the steady-phase margins cannot hide a gain-change dropout.
    nonmuted = [(frame(phases[0]["acknowledged_host_time"], 0.25), frame(phases[2]["requested_host_time"], -0.05)),
                (frame(phases[3]["acknowledged_host_time"], 0.25), frame(stops[0]["requested_host_time"], -0.05))]
    zero_runs = [longest_zero_run(samples, begin, end) for begin, end in nonmuted]
    if max(zero_runs) >= LIMITS["forbidden_zero_run_frames"]:
        failures.append("unmuted output contains a silent packet, including the live gain handoff")
    stopped = stops[0]["requested_host_time"]
    stop_onset = first_envelope(samples, frame(stopped, -0.05),
                               frame(stopped, LIMITS["transition_seconds"]), 0.0)
    tail = samples[frame(stopped, LIMITS["transition_seconds"]) * 2:]
    tail_peak = max((abs(value) for value in tail), default=0.0)
    if stop_onset is None or len(tail) < RATE * 2 or tail_peak != 0.0:
        failures.append("stop did not produce and preserve silence within 250 ms")
    with (directory / "runtime-telemetry.csv").open(newline="") as file:
        telemetry = list(csv.DictReader(file))
    if not telemetry or any(int(row["fault"]) != 0 or row["running"] != "1" or not math.isfinite(float(row["clock_correction_ppm"])) for row in telemetry):
        failures.append("runtime telemetry reported failure or was absent")
    for previous, current in zip(telemetry, telemetry[1:]):
        if any(int(current[key]) < int(previous[key]) for key in ("host_time", "rendered_frames", "callback_count", "captured_frames", "applied_control_version")):
            failures.append("runtime telemetry regressed")
            break
    active = [row for row in telemetry if int(row["host_time"]) >= phases[0]["acknowledged_host_time"]]
    throughput = None
    if len(active) >= 2:
        seconds = (int(active[-1]["host_time"]) - int(active[0]["host_time"])) * scale
        throughput = (int(active[-1]["rendered_frames"]) - int(active[0]["rendered_frames"])) / seconds
    if throughput is None or abs(throughput / RATE - 1) > 0.01:
        failures.append("observed rendered-frame throughput differs from wall clock by more than 1%")
    return {"schema": 1, "pass": not failures, "failures": failures, "criteria": LIMITS,
        "clock": clock, "phases": results, "unmuted_zero_runs": zero_runs, "stopped_tail_peak": tail_peak,
        "stop_response_seconds": None if stop_onset is None else timeline.response_seconds(stop_onset, stopped),
        "observed_rendered_frames_per_second": throughput,
        "note": "Runtime telemetry is polled off callback; independent recorder CSV contains each native capture callback timestamp."}


WITNESS_FREQUENCIES = ((440.0, 1733.0), (997.0, 2371.0))
WITNESS_LIMITS = {"relative_amplitude_error": 0.01, "residual_relative_to_peak": 0.001,
                  "forbidden_zero_run_frames": 8, "window_frames": RATE}


def witness_metrics(samples: array.array, begin: int, end: int) -> dict:
    """Fixed one-second integer-frequency Fourier basis, covering the whole range.

    Each 48,000-frame window makes the four sine/cosine basis vectors orthogonal.
    The last window overlaps the preceding one rather than omitting a short tail.
    No expected amplitude/frequency is read from a generator report.
    """
    if begin < 0 or end > len(samples) // 2 or end - begin < RATE:
        raise ValueError("insufficient witness coverage")
    starts = list(range(begin, end - RATE + 1, RATE))
    if starts[-1] != end - RATE:
        starts.append(end - RATE)
    windows = []
    # Bounded fixed bases are allocated by the offline observer, never audio code.
    bases = [[[(math.sin(2 * math.pi * hz * n / RATE), math.cos(2 * math.pi * hz * n / RATE))
               for n in range(RATE)] for hz in frequencies] for frequencies in WITNESS_FREQUENCIES]
    for start in starts:
        channels = []
        for side, frequencies in enumerate(WITNESS_FREQUENCIES):
            values = samples[start * 2 + side:(start + RATE) * 2:2]
            coefficients = [(2 * sum(v * basis[n][0] for n, v in enumerate(values)) / RATE,
                             2 * sum(v * basis[n][1] for n, v in enumerate(values)) / RATE)
                            for basis in bases[side]]
            amplitudes = [math.hypot(a, b) for a, b in coefficients]
            residual = math.sqrt(sum((v - sum(a * basis[n][0] + b * basis[n][1]
                for (a, b), basis in zip(coefficients, bases[side]))) ** 2
                for n, v in enumerate(values)) / RATE) / SOURCE_AMPLITUDE
            errors = [abs(a / SOURCE_AMPLITUDE - 1) for a in amplitudes]
            channels.append({"frequencies_hz": frequencies, "amplitudes": amplitudes,
                "relative_amplitude_errors": errors, "residual_relative_to_peak": residual,
                "pass": max(errors) <= WITNESS_LIMITS["relative_amplitude_error"]
                    and residual <= WITNESS_LIMITS["residual_relative_to_peak"]})
        windows.append({"begin_frame": start, "end_frame": start + RATE, "channels": channels,
                        "pass": all(c["pass"] for c in channels)})
    zeros = longest_zero_run(samples, begin, end)
    return {"pass": all(w["pass"] for w in windows) and zeros < WITNESS_LIMITS["forbidden_zero_run_frames"],
            "criteria": WITNESS_LIMITS, "begin_frame": begin, "end_frame": end,
            "windows": windows, "longest_stereo_zero_run": zeros}


def analyze_witness(directory: Path) -> dict:
    report = json.loads((directory / "source-witness.json").read_text())
    events = load_events(directory / "source-witness.csv")
    samples = load_wav(directory / "source-witness.wav")
    frames = sum(e["stored_frames"] for e in events)
    if (frames != report.get("frames") or frames != report.get("target_frames")
            or len(samples) != frames * 2 or len(events) != report.get("events")):
        raise ValueError("witness PCM/event/frame custody mismatch")
    clock = inspect_clock(events, report)
    valid = (frames == RECORD_SECONDS * RATE and report.get("uid") == CABLE1 and report.get("matches") == 1
             and report.get("sample_rate") == RATE and report.get("channels") == 2
             and report.get("complete") is True and report.get("ready") is True
             and all(report.get(k) == 0 for k in ("status", "stop_status", "destroy_status", "fault")))
    records = [json.loads(line) for line in (directory / "runtime-events.jsonl").read_text().splitlines()]
    phases = [r for r in records if r.get("event") == "phase"]
    stops = [r for r in records if r.get("event") == "stopped"]
    if len(phases) != len(PHASES) or len(stops) != 1:
        raise ValueError("missing witness alignment events")
    timeline = CaptureTimeline(events, report["host_time_numer"] / report["host_time_denom"] / 1e9, frames)
    result = witness_metrics(samples, timeline.frame(phases[0]["requested_host_time"]),
                             timeline.frame(stops[0]["requested_host_time"]))
    result.update(clock=clock, report_valid=valid)
    result["pass"] = result["pass"] and valid and clock["pass"]
    return result



def lifecycle_proof(timing: dict, listener_phase: str) -> dict:
    failures, phases = [], {}
    if not isinstance(timing, dict):
        return {"pass": False, "failures": ["native timing missing"]}
    if "first_fault" not in timing or timing["first_fault"] is not None:
        failures.append("native first fault missing or present")
    if listener_phase == "ListenerRemovalReturned" and timing.get("first_fault_pending") is not False:
        failures.append("output first fault pending or absent")
    for event in timing.get("lifecycle", []):
        phase = event.get("phase")
        if phase in phases:
            failures.append("duplicate lifecycle phase")
        phases[phase] = event
    if "ContextRetained" in phases:
        failures.append("native context retained")
    expected = ("CreateReturned", "StartReturned", "StopReturned", "DestroyReturned", listener_phase,
                "QuiescenceChecked", "OwnerCompleted")
    ticks = []
    for phase in expected:
        event = phases.get(phase, {})
        tick = event.get("host_time")
        if type(tick) is not int or tick <= 0:
            failures.append(f"missing valid {phase} timestamp")
        else:
            ticks.append(tick)
        key = "callbacks_in_flight" if phase in ("QuiescenceChecked", "OwnerCompleted") else "status"
        if type(event.get(key)) is not int or event[key] != 0:
            failures.append(f"{phase} requires explicit {key}=0")
    if any(later < earlier for earlier, later in zip(ticks, ticks[1:])):
        failures.append("lifecycle return chronology regressed")
    return {"pass": not failures, "failures": failures}


def analyze_lifecycle(directory: Path, selected_bundle: str) -> dict:
    records = [json.loads(line) for line in (directory / "runtime-native-timing.jsonl").read_text().splitlines()]
    failures = []
    if not records or sum(r.get("observation") == "post-stop-final" for r in records) != 1:
        raise ValueError("missing/repeated terminal native lifecycle observation")
    for record in records:
        if (record.get("schema") != 1 or record.get("protocol") != PROTOCOL
                or type(record.get("fault")) is not int or record["fault"] != 0
                or "error" not in record or record["error"] is not None
                or "first_fault" not in record or record["first_fault"] is not None):
            failures.append("runtime journal reported fault/pending attribution or invalid identity")
        timings = [record.get("output_timing")] + [c.get("native_timing") for c in record.get("captures", [])]
        for timing in timings:
            if not isinstance(timing, dict):
                continue  # Startup may precede publication; final proof below requires both.
            if timing.get("first_fault") is not None or timing.get("first_fault_pending") is True:
                failures.append("native fault observed before terminal snapshot")
            if any(e.get("phase") == "ContextRetained" or
                   (e.get("phase", "").endswith("Returned") and e.get("status") not in (None, 0))
                   for e in timing.get("lifecycle", [])):
                failures.append("native retained context or failed return observed")
    final = records[-1]
    if (final.get("observation") != "post-stop-final" or final.get("running") is not False
            or final.get("native_shutdown_observed") is not True
            or final.get("native_capture_shutdown_proven") is not True
            or final.get("native_output_shutdown_proven") is not True):
        failures.append("terminal observation/cleanup flags incomplete")
    captures = final.get("captures", [])
    if len(captures) != 1 or not selector_matches(captures[0].get("input_id"), selected_bundle):
        raise ValueError("native terminal capture selector/count differs from fixture")
    capture = lifecycle_proof(captures[0].get("native_timing"), "ListenersRemovalReturned")
    output = lifecycle_proof(final.get("output_timing"), "ListenerRemovalReturned")
    if not capture["pass"] or not output["pass"]:
        failures.append("native lifecycle return/quiescence proof failed")
    return {"pass": not failures, "failures": failures, "capture": capture, "output": output}


def analyze_generator(directory: Path, writer: tuple) -> dict:
    name, _, _, left, right, left_amplitude, right_amplitude = writer
    report = json.loads((directory / f"{name}.json").read_text())
    events = load_events(directory / f"{name}.csv")
    clock = inspect_clock(events, report)
    frames = sum(e["stored_frames"] for e in events)
    failures = []
    if (not clock["pass"] or report.get("matches") != 1
            or report.get("complete") is not True or report.get("ready") is not True
            or report.get("sample_rate") != RATE or report.get("channels") != 2
            or any(report.get(key) != 0 for key in ("status", "stop_status", "destroy_status", "fault"))
            or frames != SOURCE_SECONDS * RATE
            or frames != report.get("frames") or frames != report.get("target_frames")
            or len(events) != report.get("events")):
        failures.append("generator execution/clock/custody failed")
    result = {"pass": not failures, "failures": failures, "clock": clock}
    if (report.get("uid") != CABLE1 or report.get("mode") != "play"
            or report.get("left_hz") != left or report.get("right_hz") != right
            or report.get("left_amplitude") != left_amplitude
            or report.get("right_amplitude") != right_amplitude):
        result["pass"] = False
        result["failures"].append("generator metadata differs from fixed fixture")
    return result


def analyze(directory: Path) -> dict:
    fixture = json.loads((directory / "fixture-observation.json").read_text())
    if fixture.get("protocol") != PROTOCOL:
        raise ValueError("wrong application fixture protocol")
    validate_owned_identities(fixture["owned_generators"], fixture["selected_bundle"])
    prepared_raw = (directory / "fixture-prepared.json").read_bytes()
    prepared = json.loads(prepared_raw)
    checked = json.loads((directory / "fixture-identity-before-go.json").read_text())
    if (prepared.get("protocol") != PROTOCOL or prepared.get("selected_bundle") != fixture["selected_bundle"]
            or checked.get("owned_generators") != fixture["owned_generators"]
            or hashlib.sha256(prepared_raw).hexdigest() != fixture.get("prepared_manifest_sha256")):
        raise ValueError("frozen prepared manifest or before-go identity custody mismatch")
    for identity in fixture["owned_generators"]:
        relative = "fixtures/" + Path(identity["bundle"]).name + "/Contents/MacOS/" + Path(identity["executable"]).name
        if identity["executable_sha256"] != prepared["files"][relative]["sha256"]:
            raise ValueError("owned executable differs from frozen prepared hash")
    results = {"native_lifecycle": analyze_lifecycle(directory, fixture["selected_bundle"]),
               "runtime_output": analyze_runtime_output(directory, fixture["selected_bundle"]),
               "source_witness": analyze_witness(directory)}
    for writer in WRITERS:
        results[writer[0]] = analyze_generator(directory, writer)
    return {"schema": 1, "protocol": PROTOCOL, "pass": all(r["pass"] for r in results.values()),
            "results": results, "runtime_criteria": LIMITS,
            "scope": "Two owned instances of one bundle, with an audible unrelated bundle excluded; no real application inventory."}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        result = analyze(args.directory)
    except (OSError, ValueError, KeyError, TypeError, IndexError, ZeroDivisionError) as error:
        result = {"schema": 1, "protocol": PROTOCOL, "pass": False, "observer_error": str(error)}
    with args.output.open("x") as file:
        json.dump(result, file, indent=2, allow_nan=False)
        file.write("\n")
    return 0 if result["pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
