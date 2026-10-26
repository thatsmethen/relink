#!/usr/bin/env python3
"""Independent PCM/timestamp acceptance for the installed original Rust HAL.

Uses only the Python standard library. This instrument does not import product
code. Raw evidence is never repaired, normalized, filled, or rewritten.
"""
from __future__ import annotations

import argparse
import array
import csv
import json
import math
from pathlib import Path
import struct
import sys

RATE = 48_000
AMPLITUDE = 0.125
FREQUENCIES = (440, 997)


def load_wav(path: Path) -> array.array:
    raw = path.read_bytes()
    if len(raw) < 44 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise ValueError("not a RIFF/WAVE file")
    if struct.unpack_from("<I", raw, 4)[0] + 8 != len(raw):
        raise ValueError("RIFF length mismatch")
    cursor, fmt, payload = 12, None, None
    while cursor + 8 <= len(raw):
        kind, length = struct.unpack_from("<4sI", raw, cursor)
        cursor += 8
        if cursor + length > len(raw):
            raise ValueError("truncated WAV chunk")
        chunk = raw[cursor:cursor + length]
        if kind == b"fmt ":
            fmt = struct.unpack("<HHIIHH", chunk)
        elif kind == b"data":
            if payload is not None:
                raise ValueError("duplicate data chunk")
            payload = chunk
        cursor += length + length % 2
    if fmt != (3, 2, RATE, RATE * 8, 8, 32) or payload is None or len(payload) % 8:
        raise ValueError("expected interleaved stereo Float32 48 kHz")
    samples = array.array("f")
    samples.frombytes(payload)
    if sys.byteorder != "little":
        samples.byteswap()
    if any(not math.isfinite(value) for value in samples):
        raise ValueError("nonfinite PCM")
    return samples


def load_events(path: Path) -> list[dict]:
    result = []
    with path.open(newline="") as file:
        for raw in csv.DictReader(file):
            event = {key: float(value) if key == "sample_time" else int(value) for key, value in raw.items()}
            if not math.isfinite(event["sample_time"]) or event["sample_time"] < 0:
                raise ValueError("invalid sample timestamp")
            if event["flags"] & 3 != 3 or event["host_time"] <= 0 or event["observed_host_time"] <= 0:
                raise ValueError("missing sample/host timestamp")
            if not 0 < event["stored_frames"] <= event["frames"] <= 8192:
                raise ValueError("invalid callback frame count")
            result.append(event)
    return result


def inspect_clock(events: list[dict], report: dict) -> dict:
    gaps, reversals = [], 0
    for previous, current in zip(events, events[1:]):
        difference = current["sample_time"] - previous["sample_time"] - previous["frames"]
        if abs(difference) > 0.001:
            gaps.append({"after_sample": previous["sample_time"], "delta_frames": difference})
        if current["host_time"] <= previous["host_time"]:
            reversals += 1
    rate = None
    if len(events) >= 2:
        seconds = ((events[-1]["host_time"] - events[0]["host_time"])
                   * report["host_time_numer"] / report["host_time_denom"] / 1e9)
        if seconds > 0:
            rate = (events[-1]["sample_time"] - events[0]["sample_time"]) / seconds
    return {"callback_count": len(events), "sample_gaps": gaps, "host_reversals": reversals,
            "measured_sample_rate": rate,
            "pass": bool(events) and not gaps and not reversals and rate is not None and abs(rate - RATE) / RATE <= 0.002}


def sine_fit(samples: array.array, channel: int, begin: int, end: int, first_sample: float, frequency: float) -> dict:
    # Solve both sine and cosine coefficients; an unknown constant transport
    # phase is reported rather than silently assumed to be the declared delay.
    ss = cc = sc = ys = yc = yy = 0.0
    w = 2 * math.pi * frequency / RATE
    for index in range(begin, end):
        phase = w * (first_sample + index)
        sine, cosine, value = math.sin(phase), math.cos(phase), samples[index * 2 + channel]
        ss += sine * sine; cc += cosine * cosine; sc += sine * cosine
        ys += value * sine; yc += value * cosine; yy += value * value
    determinant = ss * cc - sc * sc
    if determinant <= 0 or end <= begin:
        raise ValueError("cannot fit tone")
    a = (ys * cc - yc * sc) / determinant
    b = (yc * ss - ys * sc) / determinant
    residual = math.sqrt(max(0.0, (yy - a * ys - b * yc) / (end - begin)))
    return {"frequency_hz": frequency, "amplitude": math.hypot(a, b), "phase_radians": math.atan2(b, a),
            "rms": math.sqrt(yy / (end - begin)), "residual_rms": residual}


def longest_zero_run(samples: array.array, begin: int, end: int) -> int:
    longest = run = 0
    for index in range(begin, end):
        if samples[index * 2] == 0.0 and samples[index * 2 + 1] == 0.0:
            run += 1; longest = max(longest, run)
        else:
            run = 0
    return longest


def analyze(report_path: Path, event_path: Path, wav_path: Path | None, silence: bool = False, trim: float = 2.0) -> dict:
    report = json.loads(report_path.read_text())
    events = load_events(event_path)
    failures = []
    if report.get("sample_rate") != RATE or report.get("channels") != 2:
        failures.append("probe format does not match the fixed stereo 48 kHz protocol")
    if report.get("amplitude") != AMPLITUDE or (report.get("left_hz"), report.get("right_hz")) != FREQUENCIES:
        failures.append("probe generator does not match the fixed 440/997 Hz, 0.125 amplitude protocol")
    for key in ("status", "stop_status", "destroy_status", "fault"):
        if report.get(key) != 0:
            failures.append(f"probe {key}={report.get(key)!r}")
    if not report.get("complete") or not report.get("ready") or report.get("matches") != 1:
        failures.append("probe incomplete, not ready, or UID was ambiguous")
    frames = sum(event["stored_frames"] for event in events)
    if frames != report.get("frames") or frames != report.get("target_frames") or len(events) != report.get("events"):
        failures.append("frame/event counts do not match")
    clock = inspect_clock(events, report)
    if not clock["pass"]:
        failures.append("sample clock or callback continuity failed")
    result = {"schema": 1, "source_report": str(report_path), "clock": clock, "frames": frames,
              "mode": "silence" if silence else report.get("mode"), "failures": failures}
    if wav_path is not None:
        samples = load_wav(wav_path)
        if len(samples) != frames * 2:
            failures.append("PCM length does not match callback custody")
        elif silence:
            peak = max((abs(value) for value in samples), default=0.0)
            result["peak"] = peak
            if peak != 0.0:
                failures.append("stopped/restarted driver exposed stale audio")
        else:
            begin, end = round(trim * RATE), frames - round(trim * RATE)
            if end - begin < RATE:
                raise ValueError("less than one second remains after fixed trimming")
            amplitude = AMPLITUDE
            channels = [sine_fit(samples, side, begin, end, events[0]["sample_time"], frequency)
                        for side, frequency in enumerate(FREQUENCIES)]
            cross = [sine_fit(samples, side, begin, end, events[0]["sample_time"], frequency)["amplitude"]
                     for side, frequency in enumerate(reversed(FREQUENCIES))]
            zero_run = longest_zero_run(samples, begin, end)
            for side, channel in enumerate(channels):
                if abs(channel["amplitude"] - amplitude) > amplitude * 0.01:
                    failures.append(f"channel {side} tone amplitude outside 1%")
                if channel["residual_rms"] > amplitude * 0.001:
                    failures.append(f"channel {side} continuous PCM residual above 0.1% of amplitude")
                if cross[side] > amplitude * 0.005:
                    failures.append(f"channel {side} wrong-channel frequency above 0.5%")
            if zero_run >= 8:
                failures.append("interior contains an exact-zero hole of at least eight stereo frames")
            result.update(channels=channels, wrong_channel_amplitude=cross, longest_stereo_zero_run=zero_run,
                          interior_frames=end - begin, trim_seconds=trim,
                          criteria={"relative_amplitude_error": 0.01, "residual_relative_to_peak": 0.001,
                                    "wrong_channel_relative_peak": 0.005, "forbidden_zero_run_frames": 8})
    result["pass"] = not failures
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--events", type=Path, required=True)
    parser.add_argument("--wav", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--silence", action="store_true")
    parser.add_argument("--trim", type=float, default=2.0)
    args = parser.parse_args()
    try:
        result = analyze(args.report, args.events, args.wav, args.silence, args.trim)
    except (OSError, ValueError, KeyError, ZeroDivisionError, struct.error) as error:
        result = {"schema": 1, "pass": False, "observer_error": str(error)}
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    return 0 if result["pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
