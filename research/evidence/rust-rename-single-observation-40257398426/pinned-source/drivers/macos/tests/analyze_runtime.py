#!/usr/bin/env python3
"""Independent fixed-protocol analysis of real Rust capture/mix output.

Expected amplitudes/frequencies are constants, not supplied by runtime reports.
The recurrence residual permits continuous bounded ASRC phase motion but rejects
isolated sample discontinuities. Analysis never modifies original PCM.
"""
from __future__ import annotations
import array
import bisect
import csv
import json
import math
from pathlib import Path

from analyze_system import RATE, inspect_clock, load_events, load_wav, longest_zero_run

FREQUENCIES = (440.0, 997.0)
SOURCE_AMPLITUDE = 0.125
PHASES = ((-6.0, False, 6), (-12.0, False, 6), (-12.0, True, 4), (-6.0, False, 6))
LIMITS = {
    "relative_amplitude_error": 0.01,
    "frequency_error_ppm": 2500.0,
    "recurrence_peak_relative": 0.005,
    "recurrence_rms_relative": 0.0005,
    "mean_relative": 0.001,
    "control_ack_seconds": 0.25,
    "transition_seconds": 0.25,
    "steady_leading_margin_seconds": 0.25,
    "steady_trailing_margin_seconds": 0.05,
    "transition_envelope_window_seconds": 0.02,
    "transition_envelope_relative_tolerance": 0.08,
    "forbidden_zero_run_frames": 8,
}


def tone_metrics(samples: array.array, begin: int, end: int, gain_db: float) -> dict:
    if end - begin < RATE:
        raise ValueError("less than one second in fixed steady phase")
    expected = SOURCE_AMPLITUDE * 10 ** (gain_db / 20)
    channels = []
    for side, frequency in enumerate(FREQUENCIES):
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
        relative_amplitude_error = abs(amplitude - expected) / expected
        frequency_error = abs(measured / frequency - 1) * 1e6
        recurrence_rms = math.sqrt(residual_energy / (end - begin - 2)) / expected
        mean = abs(total / (end - begin)) / expected
        channels.append({"expected_hz": frequency, "measured_hz": measured,
            "expected_peak": expected, "rms_equivalent_peak": amplitude,
            "relative_amplitude_error": relative_amplitude_error, "frequency_error_ppm": frequency_error,
            "recurrence_peak_relative": residual_peak / expected, "recurrence_rms_relative": recurrence_rms,
            "mean_relative": mean,
            "pass": relative_amplitude_error <= LIMITS["relative_amplitude_error"]
                and frequency_error <= LIMITS["frequency_error_ppm"]
                and residual_peak / expected <= LIMITS["recurrence_peak_relative"]
                and recurrence_rms <= LIMITS["recurrence_rms_relative"] and mean <= LIMITS["mean_relative"]})
    zeros = longest_zero_run(samples, begin, end)
    return {"pass": all(c["pass"] for c in channels) and zeros < LIMITS["forbidden_zero_run_frames"],
        "begin_frame": begin, "end_frame": end, "channels": channels, "longest_stereo_zero_run": zeros}


def first_envelope(samples: array.array, begin: int, end: int, amplitude: float) -> int | None:
    window = round(RATE * LIMITS["transition_envelope_window_seconds"])
    step = round(RATE * 0.005)
    begin, end = max(0, begin), min(len(samples) // 2, end)
    for start in range(begin, end - window + 1, step):
        if amplitude == 0:
            matched = all(samples[i] == 0 for i in range(start * 2, (start + window) * 2))
        else:
            matched = all(abs(math.sqrt(2 * sum(samples[f * 2 + side] ** 2 for f in range(start, start + window)) / window)
                / amplitude - 1) <= LIMITS["transition_envelope_relative_tolerance"] for side in range(2))
        if matched:
            return start
    return None


class CaptureTimeline:
    """Map control Mach ticks to the independent recorder's actual timeline.

    Interpolate adjacent callback host timestamps. Do not accumulate a nominal
    rate across a whole recording or infer timing from product telemetry.
    """
    def __init__(self, events: list[dict], scale: float, frames: int):
        self.ticks = [event["host_time"] for event in events]
        self.positions = [0]
        self.scale, self.frames = scale, frames
        for event in events:
            self.positions.append(self.positions[-1] + event["stored_frames"])
        if not events or self.positions[-1] != frames or scale <= 0:
            raise ValueError("invalid independent recorder timeline")
        # The last callback has no successor. Its recorded final frame count
        # supplies the one bounded nominal-rate extrapolation at the tail.
        self.ticks.append(self.ticks[-1] + events[-1]["stored_frames"] / RATE / scale)
        if any(b <= a for a, b in zip(self.ticks, self.ticks[1:])):
            raise ValueError("independent recorder host timestamps are not increasing")

    def frame(self, tick: int, seconds: float = 0.0) -> int:
        tick += seconds / self.scale
        index = bisect.bisect_right(self.ticks, tick) - 1
        if not 0 <= index < len(self.ticks) - 1:
            raise ValueError("protocol event/margin lies outside independent capture")
        fraction = (tick - self.ticks[index]) / (self.ticks[index + 1] - self.ticks[index])
        return round(self.positions[index] + fraction * (self.positions[index + 1] - self.positions[index]))

    def response_seconds(self, frame: int, requested: int) -> float:
        index = bisect.bisect_right(self.positions, frame) - 1
        fraction = (frame - self.positions[index]) / (self.positions[index + 1] - self.positions[index])
        tick = self.ticks[index] + fraction * (self.ticks[index + 1] - self.ticks[index])
        return (tick - requested) * self.scale


def analyze(directory: Path) -> dict:
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
    if not runtime.get("pass"):
        failures.append("runtime reported failure")
    if any(report.get(key) != 0 for key in ("status", "stop_status", "destroy_status", "fault")) or not report.get("complete") or not report.get("ready"):
        failures.append("independent recorder reported incomplete/faulted execution")
    if report.get("uid") != "au.textual.ravelink.cable.2" or report.get("matches") != 1 or report.get("sample_rate") != RATE or report.get("channels") != 2:
        failures.append("independent recorder did not select exactly Cable2")
    frames = sum(event["stored_frames"] for event in events)
    if frames != report.get("frames") or frames != report.get("target_frames") or len(samples) != frames * 2 or len(events) != report.get("events"):
        raise ValueError("PCM/event/frame custody mismatch")
    if len(phases) != len(PHASES) or len(starts) != 1 or len(stops) != 1:
        raise ValueError("missing or repeated runtime protocol events")
    if starts[0].get("input_id") != "coreaudio:au.textual.ravelink.cable.1" or starts[0].get("output_id") != "coreaudio:au.textual.ravelink.cable.2" or starts[0].get("sample_rate") != RATE:
        failures.append("runtime endpoints/sample rate differ from fixed protocol")
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
