"""Deterministic instrument controls; never enumerate, install or open audio."""
import array
import csv
import json
import math
from pathlib import Path
import shutil
import struct
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import analyze_multioutput as analysis
import run_multioutput
import run_system


def write_json(path, value):
    path.write_text(json.dumps(value) + "\n")


def write_records(path, values):
    path.write_text("".join(json.dumps(value) + "\n" for value in values))


def write_wav(path, samples):
    payload = samples.tobytes()
    path.write_bytes(b"RIFF" + struct.pack("<I", 36 + len(payload)) + b"WAVEfmt "
                     + struct.pack("<IHHIIHH", 16, 3, 2, 48000, 384000, 8, 32)
                     + b"data" + struct.pack("<I", len(payload)) + payload)


def tick(seconds):
    return 1_000_000_000 + round(seconds * 1e9)


def seconds(value):
    return (value - 1_000_000_000) / 1e9


def metadata(**values):
    return dict(schema=1, protocol=analysis.PROTOCOL, **values)


def tone(route=0, gain=-6, ppm=0, frames=96000):
    amplitude = analysis.SOURCE_AMPLITUDE * 10 ** (gain / 20)
    output = array.array("f")
    for frame in range(frames):
        value = amplitude * math.sin(2 * math.pi * analysis.FREQUENCIES[route] * (frame * (1 + ppm / 1e6) + 0.371) / 48000)
        output.extend((value, value))
    return output


class ToneControls(unittest.TestCase):
    def test_fixed_and_bounded_independent_clock_drift_pass(self):
        for route, ppm in ((0, 0), (0, 2000), (1, -2000)):
            with self.subTest(route=route, ppm=ppm):
                result = analysis.tone_metrics(tone(route, ppm=ppm), 0, 96000, route, -6)
                self.assertTrue(result["pass"], result)

    def test_wrong_route_mixed_route_gain_stereo_and_discontinuity_fail(self):
        baseline = tone()
        other = tone(1)
        mixed = array.array("f", (a + 0.1 * b for a, b in zip(baseline, other)))
        drifting_mixed = array.array("f", (a + 0.01 * b for a, b in zip(baseline, tone(1, ppm=2000))))
        inverted = array.array("f", baseline)
        for index in range(1, len(inverted), 2):
            inverted[index] *= -1
        skipped = array.array("f", baseline)
        skipped[48000:-2] = baseline[48002:]
        for label, data in [("wrong route", other), ("mixed routes", mixed), ("drifting weak cross-route leakage", drifting_mixed),
                            ("ignored gain", tone(gain=-12)), ("inverted channel", inverted),
                            ("single sample skip", skipped), ("unbounded drift", tone(ppm=5000))]:
            with self.subTest(label=label):
                self.assertFalse(analysis.tone_metrics(data, 0, 96000, 0, -6)["pass"])


class ProtocolControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.storage = tempfile.TemporaryDirectory()
        cls.root = Path(cls.storage.name)
        cls.buses = ["00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000002"]
        cls.identities = [dict(route_index=index, output_id=output, bus_id=cls.buses[index], is_clock=index == 1)
                          for index, output in enumerate(analysis.OUTPUT_IDS)]
        cls.phases, requested = [], 2.5
        records = [metadata(event="started", host_time=tick(-0.2), sample_rate=48000, clock_output_index=1,
                            routes=[dict(identity, frequency_hz=analysis.FREQUENCIES[index], tone_amplitude_db=-18,
                                         sample_rate=48000, device_channels=2, sample_format="f32")
                                    for index, identity in enumerate(cls.identities)])]
        for index, expected in enumerate(analysis.PHASES):
            acknowledged = round(requested + 0.02, 2)
            records.append(metadata(event="phase-request", phase=index, requested_host_time=tick(requested)))
            phase = metadata(event="phase", phase=index, requested_host_time=tick(requested),
                             acknowledged_host_time=tick(acknowledged), version=index + 1, hold_seconds=expected[4],
                             routes=[dict(route_index=route, output_id=analysis.OUTPUT_IDS[route],
                                          frequency_hz=analysis.FREQUENCIES[route], gain_db=expected[route * 2],
                                          muted=expected[route * 2 + 1], submitted_version=index + 1,
                                          submitted_observed_host_time=tick(acknowledged)) for route in range(2)])
            records.append(phase)
            cls.phases.append(phase)
            requested = round(acknowledged + expected[4], 2)
        cls.stop = requested
        records.append(metadata(event="stopped", requested_host_time=tick(requested), returned_host_time=tick(requested + 0.01)))
        write_records(cls.root / "multioutput-events.jsonl", records)
        write_json(cls.root / "multioutput-result.json", dict(metadata(native_outputs_shutdown_proven=True,
                   outputs_shutdown_proven=[True, True], shutdown_fault_observed=False, error=None), **{"pass": True}))
        write_json(cls.root / "multioutput-ready.json", metadata(ready=True, observed_host_time=tick(0.1),
                   outputs=[dict(identity, submitted_frames=14800, submitted_control_version=0) for identity in cls.identities]))
        cls.samples = []
        frames, period = analysis.RECORD_SECONDS * 48000, 512
        for route in range(2):
            samples = array.array("f")
            for frame in range(frames):
                at = frame / 48000
                gain, muted = analysis.PHASES[0][route * 2:route * 2 + 2]
                for phase in cls.phases:
                    if at >= seconds(phase["requested_host_time"]) + 0.04:
                        gain, muted = phase["routes"][route]["gain_db"], phase["routes"][route]["muted"]
                if at >= cls.stop + 0.04:
                    muted = True
                amplitude = 0 if muted else analysis.SOURCE_AMPLITUDE * 10 ** (gain / 20)
                value = amplitude * math.sin(2 * math.pi * analysis.FREQUENCIES[route] * (frame + 0.371) / 48000)
                samples.extend((value, value))
            cls.samples.append(samples)
            prefix = cls.root / f"multioutput-reader-{route}"
            write_wav(prefix.with_suffix(".wav"), samples)
            events, timings = [], []
            for index, frame in enumerate(range(0, frames, period)):
                host = tick(frame / 48000)
                events.append(dict(sample_time=100000 + frame, host_time=host, observed_host_time=host + 150,
                                   flags=3, frames=period, stored_frames=min(period, frames - frame)))
                timings.append(dict(index=index, entry_host_time=host + 100, after_pcm_host_time=host + 150,
                                    exit_host_time=host + 170, current_present=True, current_host_time=host + 50,
                                    current_sample_time=100000 + frame, current_flags=3))
            with prefix.with_suffix(".csv").open("w", newline="") as stream:
                writer = csv.DictWriter(stream, events[0].keys())
                writer.writeheader()
                writer.writerows(events)
            write_json(prefix.with_suffix(".json"), dict(status=0, stop_status=0, destroy_status=0, fault=0,
                       complete=True, ready=True, mode="record", uid=analysis.OUTPUT_IDS[route].split(":", 1)[1],
                       matches=1, sample_rate=48000, channels=2, frames=frames, target_frames=frames,
                       events=len(events), host_time_numer=1, host_time_denom=1))
            write_json(cls.root / f"multioutput-reader-{route}-diagnostics.json",
                       dict(schema=1, complete=True, io_quiescent=True, listener_quiescent=True,
                            host_time_numer=1, host_time_denom=1, event_count=len(events),
                            notification_reservations=0, dropped_notifications=0, timings=timings,
                            listeners=[], notifications=[], lifecycle=dict(start_request=10, start_return=20,
                                stop_request=30, stop_return=40, destroy_request=50, destroy_return=60)))
        rows = [cls.snapshot(step / 50) for step in range(round(cls.stop * 50))]
        rows.append(cls.snapshot(cls.stop + 0.03, terminal=True))
        write_records(cls.root / "multioutput-telemetry.jsonl", rows)

    @classmethod
    def snapshot(cls, at, terminal=False):
        version = max([phase["version"] for phase in cls.phases if phase["acknowledged_host_time"] <= tick(at)] or [0])
        count = 10000 + round(at * 48000)
        outputs = []
        for index, identity in enumerate(cls.identities):
            lifecycle = [dict(phase=phase, status=0, callbacks_in_flight=None, host_time=tick(-0.1))
                         for phase in ("CreateReturned", "StartReturned")]
            if terminal:
                lifecycle += [dict(phase=phase, status=0, callbacks_in_flight=None, host_time=tick(at))
                              for phase in ("StopReturned", "DestroyReturned", "ListenerRemovalReturned")]
                lifecycle += [dict(phase=phase, status=None, callbacks_in_flight=0, host_time=tick(at))
                              for phase in ("QuiescenceChecked", "OwnerCompleted")]
            stamp = dict(sample_time=count, host_time=tick(at), flags=3)
            timing = dict(backend="coreaudio", first_fault=None, first_fault_pending=False, lifecycle=lifecycle,
                          callbacks_seen=count // 512, progress_count=count // 512,
                          last_packet=dict(callback_index=count // 512, epoch=1, frames=512, presentation=stamp, now=stamp))
            outputs.append(dict(identity, ready=True, rendered_frames=count, submitted_frames=count,
                                rendered_control_version=version, submitted_control_version=version,
                                source_position=count, buffered_frames=2048 if index == 0 else 0,
                                target_frames=2048 if index == 0 else 0, clock_correction_ppm=0,
                                native_timing=timing, shutdown_observed=terminal, shutdown_proven=terminal))
        return metadata(observation="post-stop-final" if terminal else "running", observed_host_time=tick(at),
                        running=not terminal, fault=0, first_fault=None, error=None, rendered_frames=count,
                        callback_count=count // 512, applied_control_version=version, outputs=outputs,
                        native_outputs_shutdown_proven=terminal, shutdown_fault_observed=False)

    @classmethod
    def tearDownClass(cls):
        cls.storage.cleanup()

    def fixture(self, path):
        output = Path(path)
        for item in self.root.iterdir():
            shutil.copyfile(item, output / item.name)
        return output

    def assert_failed(self, output, expected):
        result = analysis.analyze(output)
        self.assertFalse(result["pass"])
        self.assertTrue(any(expected in failure for failure in result["failures"]), result["failures"])

    def test_complete_two_output_protocol_passes(self):
        result = analysis.analyze(self.root)
        self.assertTrue(result["pass"], result["failures"])
        self.assertEqual(result["native_telemetry"]["strict_cleanup_per_route"], [True, True])
        self.assertFalse(result["routes"][0]["phases"][0]["response_measured"])
        self.assertFalse(result["routes"][1]["phases"][2]["response_measured"])

    def test_ignored_route_control_and_stale_stop_audio_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            samples = array.array("f", self.samples[0])
            begin = round(seconds(self.phases[2]["requested_host_time"]) * 48000)
            for frame in range(begin, begin + 3 * 48000):
                value = analysis.SOURCE_AMPLITUDE * 10 ** (-12 / 20) * math.sin(2 * math.pi * 997 * frame / 48000)
                samples[frame * 2:frame * 2 + 2] = array.array("f", (value, value))
            samples[round((self.stop + 0.4) * 48000) * 2] = 0.01
            write_wav(output / "multioutput-reader-0.wav", samples)
            result = analysis.analyze(output)
            self.assertFalse(result["pass"])
            self.assertTrue(any("phase 2 route 0 PCM failed" in value for value in result["failures"]))
            self.assertTrue(any("persistent silence" in value for value in result["failures"]))

    def test_one_channel_dropout_inside_gain_margin_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            samples = array.array("f", self.samples[0])
            begin = round((seconds(self.phases[1]["requested_host_time"]) + 0.1) * 48000)
            for frame in range(begin, begin + 512):
                samples[frame * 2] = 0
            write_wav(output / "multioutput-reader-0.wav", samples)
            self.assert_failed(output, "unmuted channel dropout")

    def test_bad_independent_clock_cannot_pass_with_good_pcm(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            path = output / "multioutput-reader-0.csv"
            lines = path.read_text().splitlines()
            row = lines[150].split(",")
            row[0] = str(float(row[0]) + 512)
            lines[150] = ",".join(row)
            path.write_text("\n".join(lines) + "\n")
            self.assert_failed(output, "independent clock/continuity failed")

    def test_incomplete_duplicate_or_reordered_protocol_rejected(self):
        records = analysis.read_records(self.root / "multioutput-events.jsonl")
        for changed in (records[:-1], records + [records[-1]], records[:2] + list(reversed(records[2:4])) + records[4:]):
            with tempfile.TemporaryDirectory() as directory:
                output = self.fixture(directory)
                write_records(output / "multioutput-events.jsonl", changed)
                with self.assertRaisesRegex(ValueError, "protocol events"):
                    analysis.analyze(output)

    def test_both_native_submissions_and_all_cleanup_slots_required(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            rows = analysis.read_records(output / "multioutput-telemetry.jsonl")
            ack = self.phases[1]["acknowledged_host_time"]
            next(row for row in rows if row["observed_host_time"] == ack)["outputs"][0]["submitted_control_version"] = 1
            rows[-1]["outputs"][1]["native_timing"]["lifecycle"] = [
                value for value in rows[-1]["outputs"][1]["native_timing"]["lifecycle"] if value["phase"] != "DestroyReturned"]
            write_records(output / "multioutput-telemetry.jsonl", rows)
            result = analysis.analyze(output)
            self.assertFalse(result["pass"])
            self.assertTrue(any("ACK lacks exact native" in value for value in result["failures"]))
            self.assertTrue(any("strict cleanup" in value for value in result["failures"]))

    def test_earlier_fault_and_failed_lifecycle_cannot_be_erased_by_clean_final(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            rows = analysis.read_records(output / "multioutput-telemetry.jsonl")
            rows[300]["fault"] = 5
            rows[301]["outputs"][0]["native_timing"]["lifecycle"].append(dict(phase="StopReturned", status=-50))
            write_records(output / "multioutput-telemetry.jsonl", rows)
            result = analysis.analyze(output)
            self.assertFalse(result["pass"])
            self.assertTrue(any("global runtime" in value for value in result["failures"]))
            self.assertTrue(any("lifecycle failure occurred" in value for value in result["failures"]))

    def test_missing_native_packets_or_progress_cannot_pass_on_product_summary(self):
        rows = analysis.read_records(self.root / "multioutput-telemetry.jsonl")
        for row in rows:
            row["outputs"][0]["native_timing"]["last_packet"] = None
            row["outputs"][0]["native_timing"]["progress_count"] = 1
        failures = []
        stop = analysis.read_records(self.root / "multioutput-events.jsonl")[-1]
        analysis.inspect_telemetry(rows, self.buses, self.phases, stop, 1e-9, failures)
        self.assertIn("both outputs require observed native presentation timestamps", failures)
        self.assertTrue(any("native callback progress absent" in failure for failure in failures))

    def test_late_route_ack_and_delayed_restore_fail_fixed_request_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            records = analysis.read_records(output / "multioutput-events.jsonl")
            records[4]["acknowledged_host_time"] += 300_000_000
            records[4]["routes"][0]["submitted_observed_host_time"] += 300_000_000
            write_records(output / "multioutput-events.jsonl", records)
            samples = array.array("f", self.samples[0])
            begin = round(seconds(self.phases[3]["requested_host_time"]) * 48000)
            samples[begin * 2:(begin + 19200) * 2] = array.array("f", [0] * 38400)
            write_wav(output / "multioutput-reader-0.wav", samples)
            result = analysis.analyze(output)
            self.assertFalse(result["pass"])
            self.assertTrue(any("output submission exceeded 250 ms" in value for value in result["failures"]))
            self.assertTrue(any("phase 3 route 0 expected envelope not reached" in value for value in result["failures"]))

    def test_wrong_identity_or_interior_partial_capture_is_not_normalized(self):
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            ready = analysis.read_json(output / "multioutput-ready.json")
            ready["outputs"][0]["is_clock"] = True
            write_json(output / "multioutput-ready.json", ready)
            with self.assertRaisesRegex(ValueError, "identity/order/clock"):
                analysis.analyze(output)
        with tempfile.TemporaryDirectory() as directory:
            output = self.fixture(directory)
            path = output / "multioutput-reader-1.csv"
            lines = path.read_text().splitlines()
            row = lines[2].split(",")
            row[-1] = "511"
            lines[2] = ",".join(row)
            path.write_text("\n".join(lines) + "\n")
            with self.assertRaisesRegex(ValueError, "custody mismatch"):
                analysis.analyze(output)


class RunnerControls(unittest.TestCase):
    def test_mutating_harness_rejects_non_ci_execution(self):
        with mock.patch.dict("os.environ", {}, clear=True):
            with self.assertRaisesRegex(RuntimeError, "disposable GitHub"):
                run_system.guard_environment()

    def test_first_reader_failure_still_finishes_every_started_participant(self):
        children = []

        class Child:
            def __init__(self, *args):
                self.name = "participant-" + str(len(children))
                self.finished = False
                children.append(self)

            def wait_ready(self):
                pass

            def finish(self):
                self.finished = True
                return dict(name=self.name, exit_code=0, timed_out=False)

        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(run_multioutput, "RuntimeProcess", Child), \
                 mock.patch.object(run_system, "ProbeProcess", Child), \
                 mock.patch.object(run_multioutput, "wait_readers_ready", side_effect=RuntimeError("reader failed")), \
                 mock.patch.object(run_system, "child_diagnostics", return_value={"pass": True}), \
                 mock.patch.object(analysis, "analyze", side_effect=ValueError("original partial protocol")), \
                 mock.patch.dict("os.environ", {"RAVELINK_OBSERVER_DIAGNOSTICS": "1"}):
                result = run_multioutput.observe(Path("unused"), Path(directory), Path("unused"))
            self.assertEqual(len(children), 3)
            self.assertTrue(all(child.finished for child in children))
            self.assertFalse(result["pass"])
            self.assertEqual(result["exceptions"], ["reader failed"])
            self.assertTrue((Path(directory) / "multioutput-analysis.json").exists())
            self.assertTrue((Path(directory) / "multioutput-acceptance.json").exists())


if __name__ == "__main__":
    unittest.main()
