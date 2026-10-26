#!/usr/bin/env python3
"""Independent fixed two-output PCM, clock, protocol and lifecycle analysis.

Only standard-library code and existing independent observer readers are used.
No product implementation is imported. Expected signals and limits are constants,
never learned from product reports or selected retrospectively from recordings.
"""
from __future__ import annotations

import array
import json
import math
from pathlib import Path
import uuid

from analyze_runtime import CaptureTimeline, first_envelope
from analyze_system import RATE, inspect_clock, load_events, load_wav
from analyze_diagnostics import summarize as diagnostics

PROTOCOL = "relink-native-multioutput-tones-v1"
OUTPUT_IDS = ("coreaudio:au.textual.ravelink.cable.1", "coreaudio:au.textual.ravelink.cable.2")
FREQUENCIES = (997.0, 1543.0)
SOURCE_AMPLITUDE = 10 ** (-18 / 20)
RECORD_SECONDS = 32
PHASES = ((-6.0, False, -12.0, False, 4), (-12.0, False, -6.0, False, 4),
          (-12.0, True, -6.0, False, 3), (-12.0, False, -6.0, True, 3),
          (-6.0, False, -12.0, False, 4))
LIMITS = {
    "relative_amplitude_error": 0.01, "frequency_error_ppm": 2500.0,
    "recurrence_peak_relative": 0.005, "recurrence_rms_relative": 0.0005,
    "mean_relative": 0.001, "wrong_bus_relative_peak": 0.005,
    "stereo_difference_relative_peak": 0.000001,
    "control_ack_seconds": 0.25, "transition_seconds": 0.25,
    "initial_settling_seconds": 2.0, "steady_leading_margin_seconds": 0.25,
    "steady_trailing_margin_seconds": 0.05, "minimum_stopped_tail_seconds": 1.0,
    "forbidden_zero_run_frames": 8, "telemetry_max_gap_seconds": 0.25,
    "rendered_throughput_relative_error": 0.01,
    "phase_duration_early_tolerance_seconds": 0.02,
    "phase_duration_late_tolerance_seconds": 0.5,
    "wrong_bus_scan_step_ppm": 125, "wrong_bus_scan_extent_ppm": 2500,
    "wrong_bus_scan_window_seconds": 1.0, "wrong_bus_scan_decimation": 4,
    "wrong_bus_scan_conservative_factor": 1.03,
    "transition_envelope_window_seconds": 0.02,
    "transition_envelope_relative_tolerance": 0.08,
}


def read_json(path):
    return json.loads(path.read_text(), parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f"nonfinite JSON {value}")))


def read_records(path):
    return [json.loads(line, parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f"nonfinite JSON {value}")))
            for line in path.read_text().splitlines()]


def header(value):
    if value.get("schema") != 1 or value.get("protocol") != PROTOCOL:
        raise ValueError("unexpected multi-output schema/protocol")


def identities(routes, buses=None):
    if len(routes) != 2:
        raise ValueError("expected exactly two output routes")
    for index, route in enumerate(routes):
        if (route.get("route_index") != index or route.get("output_id") != OUTPUT_IDS[index]
                or route.get("is_clock") is not (index == 1)):
            raise ValueError("output route identity/order/clock differs from fixed protocol")
        uuid.UUID(route["bus_id"])
        if buses is not None and route["bus_id"] != buses[index]:
            raise ValueError("output bus identity changed")
    if routes[0]["bus_id"] == routes[1]["bus_id"]:
        raise ValueError("two distinct buses required")


def zero_runs(samples, begin, end):
    longest, current = [0, 0], [0, 0]
    for frame in range(begin, end):
        for side in range(2):
            current[side] = current[side] + 1 if samples[frame * 2 + side] == 0 else 0
            longest[side] = max(longest[side], current[side])
    return longest


def wrong_bus_peak(samples, side, begin, end, frequency):
    """Fixed narrow-band scan, including the allowed independent-clock drift.

    One-second windows avoid long-window cancellation. A 125 ppm grid's worst
    half-bin mismatch at 1543 Hz attenuates by <1.6%; the 3% conservative factor
    also covers finite-window projection normalization. Fixed decimation is
    confined to this diagnostic; all PCM continuity tests use every sample.
    """
    window = round(RATE * LIMITS["wrong_bus_scan_window_seconds"])
    stride = LIMITS["wrong_bus_scan_decimation"]
    starts = list(range(begin, end - window + 1, window))
    if not starts or starts[-1] != end - window:
        starts.append(end - window)
    maximum = 0.0
    for start in starts:
        data = samples[start * 2 + side:(start + window) * 2:2 * stride]
        for ppm in range(-LIMITS["wrong_bus_scan_extent_ppm"], LIMITS["wrong_bus_scan_extent_ppm"] + 1,
                         LIMITS["wrong_bus_scan_step_ppm"]):
            omega = 2 * math.pi * frequency * (1 + ppm / 1e6) * stride / RATE
            step_cos, step_sin = math.cos(omega), math.sin(omega)
            cosine, sine, real, imaginary = 1.0, 0.0, 0.0, 0.0
            for value in data:
                real += value * cosine
                imaginary += value * sine
                cosine, sine = cosine * step_cos - sine * step_sin, sine * step_cos + cosine * step_sin
            maximum = max(maximum, 2 * math.hypot(real, imaginary) / len(data))
    return maximum * LIMITS["wrong_bus_scan_conservative_factor"]


def tone_metrics(samples: array.array, begin: int, end: int, route: int, gain_db: float) -> dict:
    if end - begin < RATE:
        raise ValueError("less than one second in fixed steady phase")
    expected = SOURCE_AMPLITUDE * 10 ** (gain_db / 20)
    frequency = FREQUENCIES[route]
    channels = []
    for side in range(2):
        energy = total = residual_energy = residual_peak = 0.0
        crossings = []
        previous2, previous1 = samples[begin * 2 + side], samples[(begin + 1) * 2 + side]
        coefficient = 2 * math.cos(2 * math.pi * frequency / RATE)
        for frame in range(begin, end):
            value = samples[frame * 2 + side]
            total += value
            energy += value * value
            if frame > begin:
                previous = samples[(frame - 1) * 2 + side]
                if previous <= 0 < value:
                    crossings.append(frame - 1 - previous / (value - previous))
            if frame >= begin + 2:
                residual = value - coefficient * previous1 + previous2
                residual_peak = max(residual_peak, abs(residual))
                residual_energy += residual * residual
                previous2, previous1 = previous1, value
        measured = (len(crossings) - 1) * RATE / (crossings[-1] - crossings[0]) if len(crossings) >= 2 else 0.0
        amplitude = math.sqrt(2 * energy / (end - begin))
        values = {
            "relative_amplitude_error": abs(amplitude - expected) / expected,
            "frequency_error_ppm": abs(measured / frequency - 1) * 1e6,
            "recurrence_peak_relative": residual_peak / expected,
            "recurrence_rms_relative": math.sqrt(residual_energy / (end - begin - 2)) / expected,
            "mean_relative": abs(total / (end - begin)) / expected,
            "wrong_bus_relative_peak": wrong_bus_peak(samples, side, begin, end, FREQUENCIES[1 - route]) / expected,
        }
        channels.append({"pass": all(value <= LIMITS[key] for key, value in values.items()),
                         "expected_hz": frequency, "measured_hz": measured, "expected_peak": expected,
                         "rms_equivalent_peak": amplitude, **values})
    stereo = max(abs(samples[frame * 2] - samples[frame * 2 + 1]) for frame in range(begin, end)) / expected
    zeros = zero_runs(samples, begin, end)
    return {"pass": all(channel["pass"] for channel in channels)
            and stereo <= LIMITS["stereo_difference_relative_peak"]
            and max(zeros) < LIMITS["forbidden_zero_run_frames"],
            "begin_frame": begin, "end_frame": end, "channels": channels,
            "stereo_difference_relative_peak": stereo, "longest_channel_zero_runs": zeros}


def cleanup_proof(route):
    timing = route["native_timing"]
    events = timing["lifecycle"]
    by_phase = {event["phase"]: event for event in events}
    if len(by_phase) != len(events):
        return False
    return (timing.get("backend") == "coreaudio" and timing.get("first_fault") is None
            and timing.get("first_fault_pending") is False and "ContextRetained" not in by_phase
            and all(by_phase.get(phase, {}).get("status") == 0 for phase in
                    ("CreateReturned", "StartReturned", "StopReturned", "DestroyReturned", "ListenerRemovalReturned"))
            and all(by_phase.get(phase, {}).get("callbacks_in_flight") == 0 for phase in
                    ("QuiescenceChecked", "OwnerCompleted"))
            and all(by_phase.get(phase, {}).get("host_time", 0) > 0 for phase in
                    ("StopReturned", "DestroyReturned", "ListenerRemovalReturned", "QuiescenceChecked", "OwnerCompleted"))
            and route.get("shutdown_observed") is True and route.get("shutdown_proven") is True)


def inspect_telemetry(rows, buses, phases, stop, scale, failures):
    if not rows:
        raise ValueError("missing native multi-output telemetry")
    previous = None
    active = []
    final = []
    native = [None, None]
    for row in rows:
        header(row)
        identities(row["outputs"], buses)
        if (row.get("fault") != 0 or row.get("first_fault") is not None
                or row.get("error") is not None or row.get("shutdown_fault_observed") is not False):
            failures.append("native telemetry contains a global runtime or shutdown fault")
        observation = row["observation"]
        if observation not in ("running", "post-stop", "post-stop-final"):
            raise ValueError("unknown native observation stage")
        if observation == "running":
            if row.get("running") is not True:
                failures.append("active native telemetry is not running")
            active.append(row)
        elif row.get("running") is not False:
            failures.append("post-stop native telemetry still running")
        if observation == "post-stop-final":
            final.append(row)
        if previous:
            if any(row[key] < previous[key] for key in
                   ("observed_host_time", "rendered_frames", "callback_count", "applied_control_version")):
                failures.append("native aggregate telemetry regressed")
            if (row["observed_host_time"] - previous["observed_host_time"]) * scale > LIMITS["telemetry_max_gap_seconds"]:
                failures.append("native telemetry observation gap exceeded fixed bound")
        for index, route in enumerate(row["outputs"]):
            timing = route["native_timing"]
            if (timing.get("backend") != "coreaudio" or timing.get("first_fault") is not None
                    or timing.get("first_fault_pending") is not False):
                failures.append(f"route {index} native output timing fault or missing backend")
            if any(event["phase"] == "ContextRetained" or event.get("status") not in (None, 0)
                   or (event["phase"] in ("QuiescenceChecked", "OwnerCompleted")
                       and event.get("callbacks_in_flight") not in (None, 0))
                   for event in timing["lifecycle"]):
                failures.append(f"route {index} native lifecycle failure occurred")
            if not math.isfinite(route["clock_correction_ppm"]) or not math.isfinite(route["source_position"]):
                failures.append(f"route {index} nonfinite clock/source position")
            if previous:
                old = previous["outputs"][index]
                if any(route[key] < old[key] for key in ("rendered_frames", "submitted_frames", "source_position")):
                    failures.append(f"route {index} counters regressed")
                for key in ("submitted_control_version", "rendered_control_version"):
                    if old[key] is not None and (route[key] is None or route[key] < old[key]):
                        failures.append(f"route {index} control version regressed")
                if any(timing[key] < old["native_timing"][key] for key in ("callbacks_seen", "progress_count")):
                    failures.append(f"route {index} native callback progress regressed")
            packet = timing.get("last_packet")
            if packet:
                presentation = packet["presentation"]
                if (presentation["flags"] & 3 != 3 or not math.isfinite(presentation["sample_time"])
                        or presentation["host_time"] <= 0 or not 0 < packet["frames"] <= 16384
                        or packet["epoch"] <= 0 or packet["callback_index"] <= 0):
                    failures.append(f"route {index} native presentation timestamp invalid")
                old_packet = native[index]
                if old_packet:
                    if packet["epoch"] != old_packet["epoch"] or packet["callback_index"] < old_packet["callback_index"]:
                        failures.append(f"route {index} native epoch/callback regressed")
                    if packet["callback_index"] > old_packet["callback_index"] and (
                            presentation["sample_time"] <= old_packet["presentation"]["sample_time"]
                            or presentation["host_time"] <= old_packet["presentation"]["host_time"]):
                        failures.append(f"route {index} native presentation clock regressed")
                native[index] = packet
        previous = row
    if any(packet is None for packet in native):
        failures.append("both outputs require observed native presentation timestamps")
    if len(final) != 1 or rows[-1] is not final[0]:
        raise ValueError("missing, repeated or nonterminal final shutdown observation")
    terminal = final[0]
    if terminal.get("native_outputs_shutdown_proven") is not True or not all(cleanup_proof(route) for route in terminal["outputs"]):
        failures.append("both native output owners did not independently prove strict cleanup")
    if not active or active[0]["observed_host_time"] > phases[0]["requested_host_time"]:
        failures.append("native telemetry does not cover protocol beginning")
    if not active or (stop["requested_host_time"] - active[-1]["observed_host_time"]) * scale > LIMITS["telemetry_max_gap_seconds"]:
        failures.append("native telemetry does not cover protocol end")
    if terminal["observed_host_time"] < stop["returned_host_time"]:
        failures.append("terminal cleanup observation predates Stop return")
    for phase in phases:
        for index, route in enumerate(phase["routes"]):
            matched = [row for row in active if row["observed_host_time"] == route["submitted_observed_host_time"]]
            if (len(matched) != 1 or matched[0]["outputs"][index]["submitted_control_version"] != phase["version"]
                    or matched[0]["outputs"][index]["ready"] is not True):
                failures.append(f"phase {phase['phase']} route {index} ACK lacks exact native submitted observation")
    rates = []
    for phase_index, phase in enumerate(phases):
        boundary = phases[phase_index + 1]["requested_host_time"] if phase_index + 1 < len(phases) else stop["requested_host_time"]
        observed = [row for row in active if phase["acknowledged_host_time"] <= row["observed_host_time"] < boundary]
        if len(observed) < 2:
            failures.append(f"phase {phase_index} lacks native output progress observations")
            continue
        seconds = (observed[-1]["observed_host_time"] - observed[0]["observed_host_time"]) * scale
        for index in range(2):
            a, b = observed[0]["outputs"][index], observed[-1]["outputs"][index]
            rate = (b["submitted_frames"] - a["submitted_frames"]) / seconds if seconds > 0 else 0
            rates.append({"phase": phase_index, "route": index, "submitted_frames_per_second": rate})
            if abs(rate / RATE - 1) > LIMITS["rendered_throughput_relative_error"] or not all(row["outputs"][index]["ready"] for row in observed):
                failures.append(f"phase {phase_index} route {index} readiness/throughput failed")
            if any(b["native_timing"][key] <= a["native_timing"][key] for key in ("callbacks_seen", "progress_count")):
                failures.append(f"phase {phase_index} route {index} native callback progress absent")
    return {"rows": len(rows), "per_phase_submitted_throughput": rates,
            "strict_cleanup_per_route": [cleanup_proof(route) for route in terminal["outputs"]],
            "note": "Polled native records show prepared/submitted software buffers, not physical playback. Recorder clocks cover every captured callback independently."}


def analyze(directory: Path) -> dict:
    result = read_json(directory / "multioutput-result.json")
    ready = read_json(directory / "multioutput-ready.json")
    records = read_records(directory / "multioutput-events.jsonl")
    rows = read_records(directory / "multioutput-telemetry.jsonl")
    for record in [result, ready, *records]:
        header(record)
    expected_events = ["started"] + [value for _ in PHASES for value in ("phase-request", "phase")] + ["stopped"]
    if [record.get("event") for record in records] != expected_events:
        raise ValueError("missing, repeated or reordered fixed protocol events")
    start, stop = records[0], records[-1]
    phases = records[2:-1:2]
    requests = records[1:-1:2]
    identities(start["routes"])
    buses = [route["bus_id"] for route in start["routes"]]
    identities(ready["outputs"], buses)
    failures, observations = [], []
    if (result.get("pass") is not True or result.get("native_outputs_shutdown_proven") is not True
            or result.get("outputs_shutdown_proven") != [True, True]
            or result.get("shutdown_fault_observed") is not False or result.get("error") is not None):
        failures.append("runtime result or strict cleanup gate failed")
    if start.get("sample_rate") != RATE or start.get("clock_output_index") != 1:
        failures.append("runtime format or chosen clock differs from fixed protocol")
    for index, route in enumerate(start["routes"]):
        if (route.get("frequency_hz") != FREQUENCIES[index] or route.get("tone_amplitude_db") != -18
                or route.get("sample_rate") != RATE or route.get("device_channels") != 2):
            failures.append(f"route {index} declared generator/format differs from fixed protocol")
    if ready.get("ready") is not True or any(route["submitted_frames"] < 4096 or route["submitted_control_version"] != 0 for route in ready["outputs"]):
        failures.append("readiness lacks both initial native program submissions")
    reports = [read_json(directory / f"multioutput-reader-{index}.json") for index in range(2)]
    timebases = [(report["host_time_numer"], report["host_time_denom"]) for report in reports]
    if timebases[0] != timebases[1] or min(timebases[0]) <= 0:
        raise ValueError("independent recorder timebases invalid or unequal on shared host")
    scale = timebases[0][0] / timebases[0][1] / 1e9
    if not start["host_time"] <= ready["observed_host_time"] <= phases[0]["requested_host_time"] < stop["requested_host_time"] <= stop["returned_host_time"]:
        failures.append("runtime start/readiness/phase/stop order invalid")
    previous_version = 0
    for index, (phase, request, expected) in enumerate(zip(phases, requests, PHASES)):
        if phase["phase"] != index or request["phase"] != index or request["requested_host_time"] != phase["requested_host_time"] or phase["hold_seconds"] != expected[4]:
            failures.append(f"phase {index} differs from fixed sequence")
        requested, acknowledged = phase["requested_host_time"], phase["acknowledged_host_time"]
        if phase["version"] <= previous_version or requested > acknowledged:
            failures.append(f"phase {index} invalid control version or ACK ordering")
        previous_version = phase["version"]
        if len(phase["routes"]) != 2:
            raise ValueError("phase requires both submitted routes")
        for route_index, route in enumerate(phase["routes"]):
            gain, muted = expected[route_index * 2:route_index * 2 + 2]
            if (route["route_index"] != route_index or route["output_id"] != OUTPUT_IDS[route_index]
                    or route["frequency_hz"] != FREQUENCIES[route_index] or route["gain_db"] != gain
                    or route["muted"] is not muted or route["submitted_version"] != phase["version"]):
                failures.append(f"phase {index} route {route_index} controls or submission differ")
            if not requested <= route["submitted_observed_host_time"] <= acknowledged:
                failures.append(f"phase {index} route {route_index} invalid ACK timestamp")
        if acknowledged != max(route["submitted_observed_host_time"] for route in phase["routes"]):
            failures.append(f"phase {index} all-output ACK does not match last independent route ACK")
        if (acknowledged - requested) * scale > LIMITS["control_ack_seconds"]:
            failures.append(f"phase {index} output submission exceeded 250 ms")
        boundary = phases[index + 1]["requested_host_time"] if index + 1 < len(phases) else stop["requested_host_time"]
        duration = (boundary - acknowledged) * scale
        if not expected[4] - LIMITS["phase_duration_early_tolerance_seconds"] <= duration <= expected[4] + LIMITS["phase_duration_late_tolerance_seconds"]:
            failures.append(f"phase {index} hold duration differs from fixed protocol")
    native = inspect_telemetry(rows, buses, phases, stop, scale, failures)
    for route, report in enumerate(reports):
        prefix = directory / f"multioutput-reader-{route}"
        events = load_events(prefix.with_suffix(".csv"))
        samples = load_wav(prefix.with_suffix(".wav"))
        clock = inspect_clock(events, report)
        if not clock["pass"]:
            failures.append(f"route {route} independent clock/continuity failed")
        if (any(report.get(key) != 0 for key in ("status", "stop_status", "destroy_status", "fault"))
                or report.get("complete") is not True or report.get("ready") is not True
                or report.get("uid") != OUTPUT_IDS[route].split(":", 1)[1] or report.get("matches") != 1
                or report.get("sample_rate") != RATE or report.get("channels") != 2 or report.get("mode") != "record"):
            failures.append(f"route {route} recorder identity/execution failed")
        frames = RECORD_SECONDS * RATE
        if (report.get("frames") != frames or report.get("target_frames") != frames
                or sum(event["stored_frames"] for event in events) != frames or len(samples) != frames * 2
                or len(events) != report.get("events")
                or any(event["stored_frames"] != event["frames"] for event in events[:-1])):
            raise ValueError(f"route {route} exact recording duration/frame/PCM custody mismatch")
        diag = diagnostics(directory / f"multioutput-reader-{route}-diagnostics.json", prefix.with_suffix(".csv"), prefix.with_suffix(".json"))
        if not diag["pass"]:
            failures.append(f"route {route} independent diagnostics incomplete/faulted")
        timeline = CaptureTimeline(events, scale, frames)
        if (phases[0]["requested_host_time"] - events[0]["host_time"]) * scale < LIMITS["initial_settling_seconds"]:
            failures.append(f"route {route} fixed two-second initial prefix missing")
        phase_results = []
        for index, (phase, expected) in enumerate(zip(phases, PHASES)):
            gain, muted = expected[route * 2:route * 2 + 2]
            requested, acknowledged = phase["requested_host_time"], phase["acknowledged_host_time"]
            boundary = phases[index + 1]["requested_host_time"] if index + 1 < len(phases) else stop["requested_host_time"]
            begin = timeline.frame(acknowledged, LIMITS["steady_leading_margin_seconds"])
            end = timeline.frame(boundary, -LIMITS["steady_trailing_margin_seconds"])
            if end - begin < RATE:
                raise ValueError("insufficient steady PCM for fixed phase")
            if muted:
                peak = max(abs(value) for value in samples[begin * 2:end * 2])
                measured = {"pass": peak == 0.0, "peak": peak, "begin_frame": begin, "end_frame": end}
            else:
                measured = tone_metrics(samples, begin, end, route, gain)
            changed = index > 0 and PHASES[index - 1][route * 2:route * 2 + 2] != expected[route * 2:route * 2 + 2]
            onset = None
            if changed:
                amplitude = 0 if muted else SOURCE_AMPLITUDE * 10 ** (gain / 20)
                onset = first_envelope(samples, timeline.frame(requested),
                                       timeline.frame(requested, LIMITS["transition_seconds"]), amplitude)
                if onset is None:
                    measured["pass"] = False
                    failures.append(f"phase {index} route {route} expected envelope not reached within 250 ms")
            measured.update(phase=index, response_measured=changed,
                            detected_response_seconds=None if onset is None else timeline.response_seconds(onset, requested))
            if not measured["pass"]:
                failures.append(f"phase {index} route {route} PCM failed")
            phase_results.append(measured)
        # Join consecutive unmuted phases before checking each channel. A
        # silent packet in excluded gain-transition margins must still fail.
        uninterrupted = []
        index = 0
        while index < len(PHASES):
            if PHASES[index][route * 2 + 1]:
                index += 1
                continue
            start_index = index
            while index + 1 < len(PHASES) and not PHASES[index + 1][route * 2 + 1]:
                index += 1
            boundary = phases[index + 1]["requested_host_time"] if index + 1 < len(phases) else stop["requested_host_time"]
            begin = timeline.frame(phases[start_index]["acknowledged_host_time"], 0.25)
            end = timeline.frame(boundary, -0.05)
            longest = zero_runs(samples, begin, end)
            if max(longest) >= LIMITS["forbidden_zero_run_frames"]:
                failures.append(f"route {route} unmuted channel dropout, including transition margins")
            uninterrupted.append({"first_phase": start_index, "last_phase": index, "longest_channel_zero_runs": longest})
            index += 1
        stopped = stop["requested_host_time"]
        onset = first_envelope(samples, timeline.frame(stopped), timeline.frame(stopped, LIMITS["transition_seconds"]), 0)
        tail = samples[timeline.frame(stopped, LIMITS["transition_seconds"]) * 2:]
        tail_peak = max((abs(value) for value in tail), default=0.0)
        if onset is None or len(tail) < RATE * 2 * LIMITS["minimum_stopped_tail_seconds"] or tail_peak != 0:
            failures.append(f"route {route} Stop did not produce persistent silence within 250 ms")
        observations.append({"route_index": route, "output_id": OUTPUT_IDS[route], "clock": clock,
                             "diagnostics": diag, "phases": phase_results,
                             "uninterrupted_unmuted_intervals": uninterrupted, "stopped_tail_peak": tail_peak,
                             "stopped_tail_frames": len(tail) // 2,
                             "stop_response_seconds": None if onset is None else timeline.response_seconds(onset, stopped)})
    return {"schema": 1, "protocol": PROTOCOL, "pass": not failures, "failures": sorted(set(failures)),
            "criteria": LIMITS, "record_seconds": RECORD_SECONDS, "fixed_phases": PHASES,
            "routes": observations, "native_telemetry": native,
            "scope": "One short two-virtual-output run, internal distinct tones, independent recordings. No capture, physical playback, acoustic synchronization, long-duration or Windows qualification."}
