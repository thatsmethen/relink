"""Controls for the observer; these never enumerate or open an audio device."""
import array
import csv
import json
import math
import os
import plistlib
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
from analyze_system import analyze, load_wav
from run_system import DESTINATION, guard_environment, ProbeProcess, REPOSITORY
import run_system


class AnalysisControls(unittest.TestCase):
    def fixture(self, directory, transform=None, silence=False):
        root = Path(directory)
        frames, period, origin = 48000, 512, 100000
        samples = array.array("f")
        for frame in range(frames):
            for frequency in (440, 997):
                samples.append(0.0 if silence else 0.125 * math.sin(2 * math.pi * frequency * (frame + origin - 512) / 48000))
        if transform:
            transform(samples)
        wav = root / "capture.wav"
        payload = samples.tobytes()
        wav.write_bytes(b"RIFF" + struct.pack("<I", 36 + len(payload)) + b"WAVEfmt "
                        + struct.pack("<IHHIIHH", 16, 3, 2, 48000, 384000, 8, 32)
                        + b"data" + struct.pack("<I", len(payload)) + payload)
        events = root / "events.csv"
        with events.open("w", newline="") as file:
            writer = csv.writer(file)
            writer.writerow(["sample_time", "host_time", "observed_host_time", "flags", "frames", "stored_frames"])
            for position in range(0, frames, period):
                host = round(1e9 + position * 1e9 / 48000)
                writer.writerow([origin + position, host, host + 1000, 3, period, min(period, frames - position)])
        report = root / "report.json"
        report.write_text(json.dumps(dict(schema=1, status=0, stop_status=0, destroy_status=0, fault=0,
            complete=True, ready=True, matches=1, frames=frames, events=math.ceil(frames / period), target_frames=frames,
            mode="record", sample_rate=48000, channels=2, host_time_numer=1, host_time_denom=1,
            amplitude=0.125, left_hz=440, right_hz=997)))
        return report, events, wav

    def test_correct_distinct_stereo_tone_and_nonzero_phase_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            result = analyze(*self.fixture(directory), trim=0)
            self.assertTrue(result["pass"], result)
            self.assertEqual(result["interior_frames"], 48000)
            self.assertLess(result["channels"][0]["residual_rms"], 1e-7)

    def test_half_amplitude_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            report, events, wav = self.fixture(directory)
            raw = bytearray(wav.read_bytes())
            for offset in range(44, len(raw), 4):
                value = struct.unpack_from("<f", raw, offset)[0]
                struct.pack_into("<f", raw, offset, value * 0.5)
            wav.write_bytes(raw)
            result = analyze(report, events, wav, trim=0)
            self.assertFalse(result["pass"])
            self.assertTrue(any("amplitude outside" in item for item in result["failures"]))

    def test_generator_metadata_cannot_change_acceptance_signal(self):
        with tempfile.TemporaryDirectory() as directory:
            report, events, wav = self.fixture(directory)
            data = json.loads(report.read_text())
            data.update(amplitude=0.0625, left_hz=400)
            report.write_text(json.dumps(data))
            result = analyze(report, events, wav, trim=0)
            self.assertFalse(result["pass"])
            self.assertTrue(any("fixed 440/997" in item for item in result["failures"]))

    def test_exact_zero_packet_hole_is_rejected(self):
        def hole(samples):
            samples[12000 * 2:12512 * 2] = array.array("f", [0.0] * 1024)
        with tempfile.TemporaryDirectory() as directory:
            result = analyze(*self.fixture(directory, hole), trim=0)
            self.assertFalse(result["pass"])
            self.assertEqual(result["longest_stereo_zero_run"], 512)

    def test_phase_discontinuity_without_zeros_is_rejected(self):
        def phase_jump(samples):
            for frame in range(24000, 48000):
                samples[frame * 2] = 0.125 * math.sin(2 * math.pi * 440 * (frame + 100000 - 512 + 9) / 48000)
        with tempfile.TemporaryDirectory() as directory:
            result = analyze(*self.fixture(directory, phase_jump), trim=0)
            self.assertFalse(result["pass"])
            self.assertTrue(any("continuous PCM residual" in item for item in result["failures"]))

    def test_swapped_channels_fail(self):
        def swap(samples):
            for index in range(0, len(samples), 2):
                samples[index], samples[index + 1] = samples[index + 1], samples[index]
        with tempfile.TemporaryDirectory() as directory:
            result = analyze(*self.fixture(directory, swap), trim=0)
            self.assertFalse(result["pass"])

    def test_stale_audio_after_restart_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            result = analyze(*self.fixture(directory), silence=True)
            self.assertFalse(result["pass"])
            result = analyze(*self.fixture(directory, silence=True), silence=True)
            self.assertTrue(result["pass"], result)

    def test_timestamp_gap_fails_even_with_valid_pcm(self):
        with tempfile.TemporaryDirectory() as directory:
            report, events, wav = self.fixture(directory)
            text = events.read_text().replace("100512,", "100513,")
            events.write_text(text)
            result = analyze(report, events, wav, trim=0)
            self.assertFalse(result["pass"])
            self.assertEqual(len(result["clock"]["sample_gaps"]), 2)

    def test_nonfinite_or_truncated_pcm_is_observer_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            _, _, wav = self.fixture(directory)
            raw = bytearray(wav.read_bytes())
            struct.pack_into("<f", raw, 100, math.nan)
            wav.write_bytes(raw)
            with self.assertRaisesRegex(ValueError, "nonfinite"):
                load_wav(wav)
            wav.write_bytes(raw[:-1])
            with self.assertRaisesRegex(ValueError, "length"):
                load_wav(wav)

    def test_probe_failure_and_partial_counts_are_not_promoted(self):
        with tempfile.TemporaryDirectory() as directory:
            report, events, wav = self.fixture(directory)
            data = json.loads(report.read_text())
            data.update(complete=False, stop_status=-50, target_frames=96000)
            report.write_text(json.dumps(data))
            result = analyze(report, events, wav, trim=0)
            self.assertFalse(result["pass"])
            self.assertGreaterEqual(len(result["failures"]), 3)


class EnvironmentControls(unittest.TestCase):
    def test_local_mutating_execution_is_refused(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(RuntimeError, "disposable GitHub"):
                guard_environment()

    def test_cleanup_target_is_exact_owned_bundle(self):
        self.assertEqual(str(DESTINATION), "/Library/Audio/Plug-Ins/HAL/RustReLink.driver")
        self.assertTrue((REPOSITORY / "Cargo.toml").exists())

    def test_existing_record_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "prior.wav").write_bytes(b"preserve")
            with self.assertRaisesRegex(RuntimeError, "overwrite"):
                ProbeProcess(Path("unused"), output, "prior", "record", 1)
            self.assertEqual((output / "prior.wav").read_bytes(), b"preserve")

    def test_restart_retries_query_error_without_calling_it_absence(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with mock.patch.object(run_system, "discover", side_effect=[RuntimeError("query failed"), {"matches": 0}]), \
                 mock.patch.object(run_system.time, "sleep"):
                result = run_system.wait_presence(Path("unused"), output, "retry", 0)
            self.assertEqual(result["matches"], 0)
            self.assertEqual(json.loads((output / "retry-0-error.json").read_text())["error"], "query failed")

    def test_only_query_errors_expire_without_successful_absence(self):
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(run_system, "discover", side_effect=RuntimeError("query failed")), \
                 mock.patch.object(run_system.time, "monotonic", side_effect=[0, 26]):
                with self.assertRaisesRegex(RuntimeError, "deadline expired"):
                    run_system.wait_presence(Path("unused"), Path(directory), "retry", 0)

    def test_cleanup_refuses_foreign_bundle_without_mutating(self):
        with tempfile.TemporaryDirectory() as directory:
            output, target = Path(directory), Path(directory) / "owned.driver"
            (target / "Contents").mkdir(parents=True)
            (target / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "foreign.driver"}))
            (output / "installation-owner.json").write_text(json.dumps({"destination": str(target), "run_id": "123"}))
            with mock.patch.object(run_system, "DESTINATION", target), mock.patch.object(run_system, "guard_environment"), \
                 mock.patch.dict(os.environ, {"GITHUB_RUN_ID": "123"}), mock.patch.object(run_system, "bounded") as command:
                with self.assertRaisesRegex(RuntimeError, "identifier changed"):
                    run_system.cleanup(output, Path("unused"))
                command.assert_not_called()
            self.assertTrue(target.exists())
            self.assertFalse(json.loads((output / "cleanup-attempt-0.json").read_text())["pass"])

    def test_cleanup_requires_matching_run_ownership(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "installation-owner.json").write_text(json.dumps({"destination": str(DESTINATION), "run_id": "other"}))
            with mock.patch.object(run_system, "guard_environment"), mock.patch.dict(os.environ, {"GITHUB_RUN_ID": "123"}), \
                 mock.patch.object(run_system, "bounded") as command:
                with self.assertRaisesRegex(RuntimeError, "ownership does not match"):
                    run_system.cleanup(output, Path("unused"))
                command.assert_not_called()

    def test_successful_cleanup_recheck_does_not_restart_daemon(self):
        with tempfile.TemporaryDirectory() as directory:
            output, target = Path(directory), Path(directory) / "owned.driver"
            (output / "installation-owner.json").write_text(json.dumps({"destination": str(target), "run_id": "123"}))
            (output / "cleanup.json").write_text(json.dumps({"pass": True}))
            with mock.patch.object(run_system, "DESTINATION", target), mock.patch.object(run_system, "guard_environment"), \
                 mock.patch.dict(os.environ, {"GITHUB_RUN_ID": "123"}), mock.patch.object(run_system, "restart_service") as restart, \
                 mock.patch.object(run_system, "wait_presence", return_value={"matches": 0}) as observe:
                result = run_system.cleanup(output, Path("unused"))
                restart.assert_not_called()
                self.assertEqual(observe.call_count, 2)
                self.assertTrue(result["pass"])

    def test_manifest_retains_original_failed_bytes_and_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            original = b'{"pass":false,"reason":"frozen failure"}\n'
            (output / "failed.json").write_bytes(original)
            run_system.manifest(output)
            manifest = json.loads((output / "custody-manifest.json").read_text())
            self.assertEqual(manifest["files"], [{"path": "failed.json", "bytes": len(original),
                "sha256": run_system.hashlib.sha256(original).hexdigest()}])
            self.assertEqual((output / "failed.json").read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
