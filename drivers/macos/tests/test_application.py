"""Offline fixture controls: no device enumeration, installation, recording or playback."""
import array
import ctypes
import copy
import csv
import json
import math
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
import analyze_application as analysis
import application_protocol as protocol
import run_application as runner
import test_runtime as original_controls


def selector(bundle):
    return "relink-app-v1:" + json.dumps({"type": "mac_bundle", "bundle_id": protocol.SELECTED_ID, "path": str(bundle)})


def identities(bundle):
    values = []
    for index, (name, basename, identity, *_) in enumerate(protocol.WRITERS):
        path = Path(bundle).parent / basename
        values.append({"name": name, "pid": 9000 + index, "birth_microseconds": 1234000 + index,
            "bundle": str(path), "bundle_id": identity,
            "executable": str(runner.executable(path)), "executable_sha256": "a" * 64})
    return values



def clock_fixture(root, name, seconds, template):
    frames, period = seconds * 48000, 256
    with (root / f"{name}.csv").open("w", newline="") as file:
        writer = csv.writer(file)
        writer.writerow(["sample_time", "host_time", "observed_host_time", "flags", "frames", "stored_frames"])
        for frame in range(0, frames, period):
            tick = original_controls.ProtocolControls.tick(frame / 48000)
            writer.writerow([100000 + frame, tick, tick + 1000, 3, period, min(period, frames - frame)])
    report = dict(template, frames=frames, target_frames=frames, events=math.ceil(frames / period))
    (root / f"{name}.json").write_text(json.dumps(report))
    return report


def composite(frames=96000, excluded=True):
    samples = array.array("f")
    for frame in range(frames):
        for selected, sentinel in analysis.WITNESS_FREQUENCIES:
            samples.append(.125 * (math.sin(2 * math.pi * selected * (frame + .371) / 48000)
                + (math.sin(2 * math.pi * sentinel * (frame + .371) / 48000) if excluded else 0)))
    return samples


def native_timing(listener):
    phases = ("CreateReturned", "StartReturned", "StopReturned", "DestroyReturned", listener,
              "QuiescenceChecked", "OwnerCompleted")
    return {"first_fault": None, "first_fault_pending": False,
            "lifecycle": [{"phase": phase, "host_time": 1000 + index,
                           "status": None if index >= 5 else 0,
                           "callbacks_in_flight": 0 if index >= 5 else None}
                          for index, phase in enumerate(phases)]}


def native_record(bundle):
    return {"schema": 1, "protocol": protocol.PROTOCOL, "observation": "post-stop-final",
            "running": False, "fault": 0, "first_fault": None, "error": None,
            "native_shutdown_observed": True, "native_capture_shutdown_proven": True,
            "native_output_shutdown_proven": True,
            "output_timing": native_timing("ListenerRemovalReturned"),
            "captures": [{"input_id": selector(bundle), "native_timing": native_timing("ListenersRemovalReturned")}]}


class IdentityControls(unittest.TestCase):
    bundle = "/tmp/owned-fixtures/ReLinkToneFixture.app"

    def test_selector_is_real_application_identity_not_hardware(self):
        self.assertTrue(protocol.selector_matches(selector(self.bundle), self.bundle))
        for bad in ("coreaudio:au.textual.ravelink.cable.1", selector(self.bundle + "/"),
                    selector(self.bundle).replace("mac_bundle", "windows_executable"),
                    selector(self.bundle).replace(protocol.SELECTED_ID, "some.real.app"),
                    'relink-app-v1:{"type":"mac_bundle","type":"mac_bundle"}'):
            self.assertFalse(protocol.selector_matches(bad, self.bundle))

    def test_two_instances_same_executable_with_distinct_owned_pids(self):
        protocol.validate_owned_identities(identities(self.bundle), self.bundle)
        for field, value in (("pid", 9000), ("bundle_id", protocol.EXCLUDED_ID),
                             ("executable_sha256", "b" * 64), ("birth_microseconds", 0)):
            records = identities(self.bundle)
            records[1][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                protocol.validate_owned_identities(records, self.bundle)
        with self.assertRaises(ValueError):
            protocol.validate_owned_identities(identities(self.bundle)[:2], self.bundle)
        for bundle in ("relative/ReLinkToneFixture.app", "/tmp/../owned-fixtures/ReLinkToneFixture.app", self.bundle + "/"):
            with self.subTest(bundle=bundle), self.assertRaises(ValueError):
                protocol.validate_owned_identities(identities(bundle), bundle)

    def test_consent_denied_before_even_filesystem_or_process_access(self):
        with mock.patch.object(runner, "verify_prepared") as verify, mock.patch.object(runner.subprocess, "Popen") as spawn:
            with self.assertRaises(PermissionError):
                runner.run(Path("/does-not-exist"), False)
            verify.assert_not_called()
            spawn.assert_not_called()

    def test_pid_abi_offsets_match_static_sdk_controls(self):
        self.assertEqual(ctypes.sizeof(runner.BSDInfo), 136)
        self.assertEqual(runner.BSDInfo.pid.offset, 12)
        self.assertEqual(runner.BSDInfo.start_sec.offset, 120)

    def test_fixed_recording_covers_declared_schedule_and_source_lifetime(self):
        # Worst legal phase durations include the analyzer's 0.5 s per-phase
        # scheduling allowance in addition to each callback ACK deadline.
        required = (8.0 + 0.5 + sum(phase[2] for phase in analysis.PHASES)
                    + len(analysis.PHASES) * (analysis.LIMITS["control_ack_seconds"] + 0.5)
                    + analysis.LIMITS["transition_seconds"] + 1.0)
        self.assertGreater(protocol.RECORD_SECONDS, required)
        self.assertLess(protocol.STARTUP_SECONDS + protocol.RECORD_SECONDS, protocol.SOURCE_SECONDS)

    def test_busy_or_unknown_cable_never_counts_as_unused(self):
        valid = {"uid": protocol.CABLE1, "matches": 1, "status": 0, "complete": True,
                 "device_running_status": 0, "device_is_running_somewhere": False}
        runner.require_unused_cable(valid, protocol.CABLE1)
        for key, value in (("device_is_running_somewhere", True), ("device_is_running_somewhere", None),
                           ("device_is_running_somewhere", 0), ("device_running_status", -1), ("matches", 2)):
            with self.subTest(key=key, value=value), self.assertRaises(RuntimeError):
                runner.require_unused_cable(dict(valid, **{key: value}), protocol.CABLE1)

    def test_ready_deadline_is_spawn_based_not_wait_order(self):
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "ready"
            ready.write_text('{"ready":true,"frames":512}')
            child = mock.Mock(name="child")
            child.name, child.started_at, child.ready = "owned", 10.0, ready
            with mock.patch.object(runner.time, "monotonic", return_value=18.1):
                with self.assertRaisesRegex(RuntimeError, "fixed deadline"):
                    runner.wait_ready(child, 100.0)
            with mock.patch.object(runner.time, "monotonic", return_value=17.9):
                runner.wait_ready(child, 100.0)
            with mock.patch.object(runner.time, "monotonic", return_value=17.9):
                with self.assertRaises(RuntimeError):
                    runner.wait_ready(child, 17.0)

    def test_runtime_consent_environment_only_set_by_run_child(self):
        with mock.patch.object(runner.ApplicationProcess, "_spawn") as spawn:
            runner.ApplicationProcess(Path("/owned/runtime"), Path("/owned/observation"), Path(self.bundle))
        args, kwargs = spawn.call_args
        self.assertEqual(args[3]["RELINK_APPLICATION_CAPTURE_CONSENT"], "1")
        self.assertIn("--application-bundle", args[0])
        self.assertNotIn("--input-id", args[0])
        self.assertEqual(args[0][args[0].index("--output-id") + 1], "coreaudio:" + protocol.CABLE2)
        self.assertFalse(kwargs)

    def test_prepared_bytes_freshness_and_paths_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            for name, identity in ((protocol.SELECTED_BUNDLE, protocol.SELECTED_ID),
                                   (protocol.EXCLUDED_BUNDLE, protocol.EXCLUDED_ID),
                                   (protocol.PROBE_BUNDLE, protocol.PROBE_ID)):
                bundle = root / "fixtures" / name
                binary = runner.executable(bundle)
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"offline fake fixture, never executed")
                (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": identity,
                                                                          "CFBundleExecutable": bundle.stem}))
            data = {"protocol": protocol.PROTOCOL, "prepared_directory": str(root),
                    "selected_bundle": str(root / "fixtures" / protocol.SELECTED_BUNDLE), "files": runner.inventory(root)}
            runner.save_new(root / "prepared.json", data)
            runner.verify_prepared(root)
            binary.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "bytes changed"):
                runner.verify_prepared(root)
            with self.assertRaises(FileExistsError):
                runner.save_new(root / "prepared.json", data)
            (root / "link").symlink_to(binary)
            with self.assertRaisesRegex(ValueError, "symlinks"):
                runner.inventory(root)


class ArmingControls(unittest.TestCase):
    def observe_arm(self, scenario):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            clock = [18.1 if scenario == "slow-second-spawn" else 17.0]
            readers = []
            for index in range(2):
                child = mock.Mock()
                child.name, child.started_at = f"reader-{index}", 10.0 + (8.1 if scenario == "slow-second-spawn" else 1) * index
                child.ready = output / f"reader-{index}.ready"
                child.ready.write_text('{"ready":true,"frames":512}')
                child.process.poll.return_value = None
                readers.append(child)
            owned = [{"pid": index} for index in range(3)]
            writers = [(index, None, None) for index in range(3)]
            def identity(index, *_):
                if scenario == "slow-identity" and index == 2:
                    clock[0] = 18.1
                return owned[index]
            def sleep(seconds):
                clock[0] += 2 if scenario == "late-preroll" else seconds
            with mock.patch.object(runner.time, "monotonic", side_effect=lambda: clock[0]), \
                    mock.patch.object(runner.time, "sleep", side_effect=sleep), \
                    mock.patch.object(runner, "owned_identity", side_effect=identity):
                if scenario == "valid":
                    runner.arm_recorders(readers, writers, owned, readers, output, 30)
                    self.assertTrue((output / "runtime.go").exists())
                else:
                    with self.assertRaises(RuntimeError):
                        runner.arm_recorders(readers, writers, owned, readers, output, 30)
                    self.assertFalse((output / "runtime.go").exists())

    def test_shared_arm_deadline_allows_ready_observers_and_preroll(self):
        self.observe_arm("valid")

    def test_slow_second_spawn_does_not_extend_first_recorder_budget(self):
        self.observe_arm("slow-second-spawn")

    def test_identity_checks_must_finish_before_reserved_preroll(self):
        self.observe_arm("slow-identity")

    def test_late_preroll_cannot_publish_go(self):
        self.observe_arm("late-preroll")


class SignalControls(unittest.TestCase):
    def test_witness_requires_all_four_tones_in_every_window(self):
        self.assertTrue(analysis.witness_metrics(composite(), 0, 96000)["pass"])
        self.assertFalse(analysis.witness_metrics(composite(excluded=False), 0, 96000)["pass"])
        values = composite(120000)
        values[96000:97024] = array.array("f", [0] * 1024)
        result = analysis.witness_metrics(values, 0, 120000)
        self.assertFalse(result["pass"])
        self.assertEqual(result["longest_stereo_zero_run"], 512)
        self.assertEqual(result["windows"][-1]["end_frame"], 120000)

    def test_capture_rejects_sentinel_leak_and_missing_instance(self):
        pure = original_controls.signal()
        self.assertTrue(analysis.tone_metrics(pure, 0, 96000, -6)["pass"])
        leaked = array.array("f", pure)
        gain = .125 * 10 ** (-6 / 20)
        for frame in range(96000):
            leaked[frame * 2] += gain * math.sin(2 * math.pi * 1733 * frame / 48000)
        self.assertFalse(analysis.tone_metrics(leaked, 0, 96000, -6)["pass"])
        missing = array.array("f", pure)
        missing[1::2] = array.array("f", [0] * 96000)
        self.assertFalse(analysis.tone_metrics(missing, 0, 96000, -6)["pass"])


class FullProtocolControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        original_controls.ProtocolControls.setUpClass()
        cls.base = original_controls.ProtocolControls
        cls.storage = tempfile.TemporaryDirectory()
        cls.root = Path(cls.storage.name)
        cls.base().fixture(cls.root)
        cls.bundle = "/tmp/owned-fixtures/ReLinkToneFixture.app"
        records = [json.loads(line) for line in (cls.root / "runtime-events.jsonl").read_text().splitlines()]
        records[0].update(input_id=selector(cls.bundle), protocol=protocol.PROTOCOL,
                          input_format=protocol.TWO_PROCESS_FORMAT, expected_current_bundle_audio_processes=2)
        (cls.root / "runtime-events.jsonl").write_text("".join(json.dumps(r) + "\n" for r in records))
        owned = identities(cls.bundle)
        prepared_files = {"fixtures/" + Path(i["bundle"]).name + "/Contents/MacOS/" + Path(i["executable"]).name:
                          {"sha256": i["executable_sha256"]} for i in owned}
        runner.save_new(cls.root / "fixture-prepared.json", {"protocol": protocol.PROTOCOL,
            "selected_bundle": cls.bundle, "files": prepared_files})
        runner.save_new(cls.root / "fixture-observation.json", {"protocol": protocol.PROTOCOL,
            "selected_bundle": cls.bundle, "owned_generators": owned,
            "prepared_manifest_sha256": runner.digest(cls.root / "fixture-prepared.json")})
        runner.save_new(cls.root / "fixture-identity-before-go.json", {"owned_generators": owned})
        (cls.root / "runtime-result.json").write_text(json.dumps({"schema": 1, "protocol": protocol.PROTOCOL,
            "pass": True, "native_capture_shutdown_proven": True, "native_output_shutdown_proven": True, "error": None}))
        (cls.root / "runtime-native-timing.jsonl").write_text(json.dumps(native_record(cls.bundle)) + "\n")
        cls.samples = array.array("f", cls.base.samples)
        cls.samples.extend([0] * (protocol.RECORD_SECONDS * 48000 * 2 - len(cls.samples)))
        original_controls.write_wav(cls.root / "runtime-reader.wav", cls.samples)
        report = json.loads((cls.root / "runtime-reader.json").read_text())
        report = clock_fixture(cls.root, "runtime-reader", protocol.RECORD_SECONDS, report)
        witness = composite(protocol.RECORD_SECONDS * 48000)
        original_controls.write_wav(cls.root / "source-witness.wav", witness)
        report.update(uid=protocol.CABLE1, mode="record")
        clock_fixture(cls.root, "source-witness", protocol.RECORD_SECONDS, report)
        for name, _, _, left, right, left_amplitude, right_amplitude in protocol.WRITERS:
            clock_fixture(cls.root, name, protocol.SOURCE_SECONDS, dict(report, mode="play", amplitude=.125,
                left_hz=left, right_hz=right, left_amplitude=left_amplitude, right_amplitude=right_amplitude))

    @classmethod
    def tearDownClass(cls):
        cls.storage.cleanup()
        cls.base.tearDownClass()

    def test_full_application_sequence_with_independent_witness_passes(self):
        result = analysis.analyze(self.root)
        self.assertTrue(result["pass"], result)
        self.assertIs(analysis.LIMITS, original_controls.analysis.LIMITS)
        self.assertLess(result["results"]["runtime_output"]["stop_response_seconds"], .06)

    def test_self_consistent_shortened_output_recording_fails_fixed_length(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for path in self.root.glob("runtime-*"):
                shutil.copyfile(path, output / path.name)
            report = json.loads((output / "runtime-reader.json").read_text())
            clock_fixture(output, "runtime-reader", 26, report)
            original_controls.write_wav(output / "runtime-reader.wav", self.samples[:26 * 48000 * 2])
            result = analysis.analyze_runtime_output(output, self.bundle)
            self.assertFalse(result["pass"])
            self.assertIn("independent recorder duration differs from fixed application protocol", result["failures"])

    def test_self_consistent_shortened_source_witness_fails_fixed_length(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for path in self.root.glob("source-witness.*"):
                shutil.copyfile(path, output / path.name)
            shutil.copyfile(self.root / "runtime-events.jsonl", output / "runtime-events.jsonl")
            report = json.loads((output / "source-witness.json").read_text())
            clock_fixture(output, "source-witness", 26, report)
            original_controls.write_wav(output / "source-witness.wav",
                                       analysis.load_wav(output / "source-witness.wav")[:26 * 48000 * 2])
            result = analysis.analyze_witness(output)
            self.assertFalse(result["pass"])
            self.assertFalse(result["report_valid"])

    def test_self_consistent_shortened_generators_fail_fixed_length(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for writer in protocol.WRITERS:
                name = writer[0]
                report = json.loads((self.root / f"{name}.json").read_text())
                clock_fixture(output, name, 26, report)
                with self.subTest(generator=name):
                    result = analysis.analyze_generator(output, writer)
                    self.assertFalse(result["pass"])
                    self.assertTrue(result["clock"]["pass"])

    def test_identity_must_match_prepared_hash_and_before_go_record(self):
        for corruption in ("hash", "before-go"):
            with tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                for path in self.root.glob("fixture-*.json"):
                    shutil.copyfile(path, output / path.name)
                path = output / ("fixture-prepared.json" if corruption == "hash" else "fixture-identity-before-go.json")
                value = json.loads(path.read_text())
                if corruption == "hash":
                    next(iter(value["files"].values()))["sha256"] = "b" * 64
                else:
                    value["owned_generators"][1]["birth_microseconds"] += 1
                path.write_text(json.dumps(value))
                with self.subTest(corruption=corruption), self.assertRaises(ValueError):
                    analysis.analyze(output)

    def test_runtime_config_cannot_claim_one_process_or_fake_hardware_selector(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for path in self.root.glob("runtime-*"):
                shutil.copyfile(path, output / path.name)
            records = [json.loads(line) for line in (output / "runtime-events.jsonl").read_text().splitlines()]
            records[0]["expected_current_bundle_audio_processes"] = 1
            records[0]["input_id"] = "coreaudio:" + protocol.CABLE1
            (output / "runtime-events.jsonl").write_text("".join(json.dumps(r) + "\n" for r in records))
            result = analysis.analyze_runtime_output(output, self.bundle)
            self.assertFalse(result["pass"])
            self.assertTrue(any("two-process" in f for f in result["failures"]))

    def test_late_ack_does_not_extend_request_based_response_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for path in self.root.glob("runtime-*"):
                shutil.copyfile(path, output / path.name)
            records = [json.loads(line) for line in (output / "runtime-events.jsonl").read_text().splitlines()]
            phase = records[3]  # Mute transition.
            requested = phase["requested_host_time"]
            phase["acknowledged_host_time"] = requested + 200_000_000
            samples = array.array("f", self.samples)
            begin = round(self.base.seconds(requested) * 48000)
            for frame in range(begin, begin + 19200):
                for side, hz in enumerate((440, 997)):
                    samples[frame * 2 + side] = .125 * 10 ** (-12 / 20) * math.sin(2 * math.pi * hz * frame / 48000)
            original_controls.write_wav(output / "runtime-reader.wav", samples)
            (output / "runtime-events.jsonl").write_text("".join(json.dumps(r) + "\n" for r in records))
            result = analysis.analyze_runtime_output(output, self.bundle)
            self.assertFalse(result["pass"])
            self.assertIsNone(result["phases"][2]["detected_response_seconds"])


class LifecycleControls(unittest.TestCase):
    bundle = "/tmp/owned-fixtures/ReLinkToneFixture.app"

    def check(self, records):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "runtime-native-timing.jsonl").write_text("".join(json.dumps(r) + "\n" for r in records))
            return analysis.analyze_lifecycle(output, self.bundle)

    def test_independent_lifecycle_proof_rejects_claimed_success_with_missing_or_failed_phase(self):
        self.assertTrue(self.check([native_record(self.bundle)])["pass"])
        for side in ("capture", "output"):
            for corruption in ("missing", "failed", "inflight", "retained", "fault", "duplicate", "pending"):
                record = native_record(self.bundle)
                timing = record["output_timing"] if side == "output" else record["captures"][0]["native_timing"]
                if corruption == "missing":
                    timing["lifecycle"].pop(2)
                elif corruption == "failed":
                    timing["lifecycle"][2]["status"] = -1
                elif corruption == "inflight":
                    timing["lifecycle"][-1]["callbacks_in_flight"] = 1
                elif corruption == "retained":
                    timing["lifecycle"].append({"phase": "ContextRetained", "host_time": 1008})
                elif corruption == "fault":
                    timing["first_fault"] = {"fault": 5}
                elif corruption == "duplicate":
                    timing["lifecycle"].append(copy.deepcopy(timing["lifecycle"][0]))
                else:
                    timing["first_fault_pending"] = True
                with self.subTest(side=side, corruption=corruption):
                    self.assertFalse(self.check([record])["pass"])

    def test_late_fault_cannot_disappear_in_clean_terminal_snapshot(self):
        before = native_record(self.bundle)
        before.update(observation="post-stop", fault=5)
        result = self.check([before, native_record(self.bundle)])
        self.assertFalse(result["pass"])

    def test_terminal_observation_is_not_cleanup_proof(self):
        record = native_record(self.bundle)
        record["native_capture_shutdown_proven"] = False
        self.assertFalse(self.check([record])["pass"])
        with self.assertRaises(ValueError):
            self.check([])


@unittest.skipUnless(sys.platform == "darwin", "SDK compile controls require macOS")
class CGeneratorControls(unittest.TestCase):
    def test_actual_parser_and_pure_callback_without_hardware_main(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "controls"
            subprocess.run(["/usr/bin/xcrun", "clang", "-std=c11", "-Wall", "-Wextra", "-Werror",
                str(Path(__file__).with_name("application_generator_controls.c")), "-framework", "CoreAudio",
                "-framework", "CoreFoundation", "-o", str(executable)], check=True, capture_output=True)
            result = subprocess.run([str(executable)], check=True, capture_output=True, text=True, timeout=10)
            self.assertIn("no audio API called", result.stdout)


if __name__ == "__main__":
    unittest.main()
