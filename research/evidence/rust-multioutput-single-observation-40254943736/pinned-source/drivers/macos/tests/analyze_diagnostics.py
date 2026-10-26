"""Validate supplemental observer records without changing PCM acceptance.

No scheduling threshold is imposed here. These timings distinguish callback
entry lateness from execution/preemption, not a cause or a product qualification.
"""
import json
import math
from pathlib import Path

from analyze_system import load_events


def integer(value, label, maximum=(1 << 64) - 1):
    if type(value) is not int or not 0 <= value <= maximum:
        raise ValueError(f"invalid diagnostic integer: {label}")
    return value


def summarize(path: Path, event_path: Path, report_path: Path) -> dict:
    if path.stat().st_size > 128 * 1024 * 1024:
        raise ValueError("diagnostic file exceeds bounded capacity")
    data = json.loads(path.read_text())
    report = json.loads(report_path.read_text())
    events = load_events(event_path)
    if data.get("schema") != 1:
        raise ValueError("unsupported diagnostic schema")
    numerator = integer(data["host_time_numer"], "timebase numerator", (1 << 32) - 1)
    denominator = integer(data["host_time_denom"], "timebase denominator", (1 << 32) - 1)
    if not numerator or not denominator or (numerator, denominator) != (report["host_time_numer"], report["host_time_denom"]):
        raise ValueError("diagnostic timebase differs from original report")
    scale = numerator / denominator / 1e9
    timings = data["timings"]
    if len(timings) != len(events) or integer(data["event_count"], "event count", 262144) != len(events):
        raise ValueError("diagnostic timing count differs from original CSV")
    failures = []
    if not all(data.get(field) is True for field in ("complete", "io_quiescent", "listener_quiescent")):
        failures.append("diagnostic observation is incomplete or not quiescent")
    metrics, gaps = [], []
    for index, (timing, event) in enumerate(zip(timings, events)):
        if integer(timing["index"], "timing index") != index:
            raise ValueError("diagnostic timing indices are not exact and ordered")
        entry = integer(timing["entry_host_time"], "callback entry")
        after = integer(timing["after_pcm_host_time"], "callback after PCM")
        exit_time = integer(timing["exit_host_time"], "callback exit")
        if not 0 < entry <= after <= exit_time or after != event["observed_host_time"]:
            raise ValueError("callback diagnostic ordering/custody differs from original CSV")
        flags = integer(timing["current_flags"], "OS current flags", (1 << 32) - 1)
        current_host = integer(timing["current_host_time"], "OS current host time")
        sample = timing["current_sample_time"]
        if type(timing["current_present"]) is not bool:
            raise ValueError("invalid OS current timestamp presence")
        if not timing["current_present"] and (flags or current_host):
            raise ValueError("absent OS timestamp contains valid fields")
        if flags & 1 and (type(sample) not in (float, int) or not math.isfinite(sample)):
            raise ValueError("OS current sample time is flagged valid but nonfinite/missing")
        if flags & 2 and current_host == 0:
            raise ValueError("OS current host time is flagged valid but missing")
        metric = {"index": index, "entry_minus_native_seconds": (entry - event["host_time"]) * scale,
            "callback_work_seconds": (exit_time - entry) * scale,
            "entry_minus_os_current_seconds": (entry - current_host) * scale if flags & 2 else None}
        metrics.append(metric)
        if index:
            previous = events[index - 1]
            gap = event["sample_time"] - previous["sample_time"] - previous["frames"]
            if abs(gap) > 0.001:
                gaps.append({"delta_frames": gap, "previous": metrics[index - 1], "current": metric})
    listeners = data["listeners"]
    if len(listeners) > 8:
        raise ValueError("too many diagnostic listener records")
    for listener in listeners:
        for key in ("object", "selector", "scope", "element"):
            integer(listener[key], key, (1 << 32) - 1)
        if type(listener["available"]) is not bool or type(listener["added"]) is not bool:
            raise ValueError("invalid listener registration flags")
        for key in ("add_status", "remove_status"):
            if type(listener[key]) is not int or not -(1 << 31) <= listener[key] < 1 << 31:
                raise ValueError("invalid listener OSStatus")
        if listener["available"] and (not listener["added"] or listener["add_status"] or listener["remove_status"]):
            failures.append("an available listener could not be registered/removed")
    notifications = data["notifications"]
    reserved = integer(data["notification_reservations"], "notification reservations")
    dropped = integer(data["dropped_notifications"], "dropped notification count")
    if len(notifications) > 1024:
        raise ValueError("notification capacity exceeded")
    if dropped or len(notifications) != reserved:
        failures.append("notification records were dropped or not committed")
    previous_index = -1
    for event in notifications:
        index = integer(event["index"], "notification index", 1023)
        if index <= previous_index or index >= reserved:
            raise ValueError("duplicate or unordered notification reservation")
        previous_index = index
        if not integer(event["host_time"], "notification host time"):
            raise ValueError("notification host timestamp missing")
        for key in ("object", "selector", "scope", "element"):
            integer(event[key], key, (1 << 32) - 1)
    lifecycle = data["lifecycle"]
    for begin, end in (("start_request", "start_return"), ("stop_request", "stop_return"), ("destroy_request", "destroy_return")):
        a, b = integer(lifecycle[begin], begin), integer(lifecycle[end], end)
        if (a == 0) != (b == 0) or b < a:
            raise ValueError("invalid diagnostic lifecycle interval")
    return {"schema": 1, "pass": not failures, "failures": failures,
        "accepted_pcm_callbacks": len(timings), "max_callback_work_seconds": max((m["callback_work_seconds"] for m in metrics), default=None),
        "max_entry_minus_native_seconds": max((m["entry_minus_native_seconds"] for m in metrics), default=None),
        "sample_gap_context": gaps, "notifications": notifications, "listeners": listeners,
        "lifecycle": lifecycle, "dropped_notifications": dropped,
        "note": "Diagnostic integrity only. No scheduling performance limit or PCM acceptance threshold is changed."}
