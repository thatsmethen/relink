"""Offline synthetic controls for console fallback; no observations are rerun."""
import hashlib
import contextlib
import io
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('ci_summary', ROOT / 'scripts/ci-acceptance-summary.py')
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


class ConsoleSummaryTests(unittest.TestCase):
    def collect(self, root, kind='runtime'):
        return summary.collect(Path(root).resolve(), kind)

    def test_failure_cleanup_and_missing_reports_remain_distinct(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'runtime-result.json').write_text('{"pass":false,"native_output_shutdown_proven":false}')
            (root / 'cleanup.json').write_text('{"pass":true,"devices":{"au.textual.ravelink.cable.1":{"matches":0,"complete":true,"status":0}}}')
            before = {p.name: p.read_bytes() for p in root.iterdir()}
            records = self.collect(root)
            by_name = {record['file']: record for record in records}
            self.assertFalse(by_name['runtime-result.json']['summary']['pass'])
            self.assertTrue(by_name['cleanup.json']['summary']['pass'])
            self.assertEqual(by_name['cleanup.json']['summary']['devices']['cable1']['matches'], 0)
            self.assertEqual(by_name['runtime-acceptance.json']['status'], 'missing')
            self.assertEqual(before, {p.name: p.read_bytes() for p in root.iterdir()})
            self.assertEqual(summary.render(records, 'runtime'), summary.render(self.collect(root), 'runtime'))

    def test_private_strings_commands_unknown_keys_and_paths_are_not_printed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            secret = '/Users/private-person/secret-file ::error::TOKEN_SECRET\n\x1b[31m'
            data = {'pass': False, 'error': secret, 'source_report': secret, secret: secret,
                    'processes': [{'name': secret, 'exit_code': 9}],
                    'first_fault': {'fault': 5, 'origin': 'NativeOutputPacket', 'direction': 'Output', 'endpoint_id': secret}}
            (root / 'runtime-result.json').write_text(json.dumps(data))
            output = summary.render(self.collect(root), 'runtime')
            self.assertNotIn('private-person', output)
            self.assertNotIn('TOKEN_SECRET', output)
            self.assertNotIn('::error::', output)
            self.assertIn('NativeOutputPacket', output)
            self.assertIn('text_withheld', output)
            self.assertIn(hashlib.sha256(secret.encode()).hexdigest(), output)
            self.assertTrue(all(line.startswith(summary.PREFIX) for line in output.splitlines()))

    def test_manifest_only_prints_fixed_names_and_never_opens_pcm(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = {'source_revision': 'a' * 40, 'run_id': '123', 'run_attempt': '1', 'files': [
                {'path': 'runtime-reader.wav', 'bytes': 999, 'sha256': 'b' * 64},
                {'path': '/private/personal-secret', 'bytes': 4, 'sha256': 'c' * 64}]}
            (root / 'custody-manifest.json').write_text(json.dumps(manifest))
            os.mkfifo(root / 'runtime-reader.wav')  # Any accidental PCM read would hang.
            output = summary.render(self.collect(root), 'runtime')
            self.assertIn('runtime-reader.wav', output)
            self.assertIn('b' * 64, output)
            self.assertNotIn('personal-secret', output)
            self.assertIn('hashes_are_manifest_claims_not_reverified_payloads', output)

    def test_symlinks_fifo_and_permission_error_are_explicit_without_following(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            private = root / 'private'; private.write_text('{"pass":true}')
            (root / 'runtime-result.json').symlink_to(private)
            os.mkfifo(root / 'cleanup.json')
            records = {r['file']: r for r in self.collect(root)}
            self.assertEqual(records['runtime-result.json']['status'], 'unreadable')
            self.assertEqual(records['cleanup.json']['status'], 'not_regular_file')
            with patch.object(summary.os, 'open', side_effect=PermissionError(13, 'private path never print')):
                denied = summary.read_record(0, 'runtime-result.json', 'runtime')
            self.assertEqual(denied, {'file': 'runtime-result.json', 'status': 'unreadable', 'errno': 13})

    def test_symlinked_directory_and_missing_directory_are_reported(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            link = root / 'link'; link.symlink_to(root, target_is_directory=True)
            self.assertTrue(all(r['status'] == 'unreadable_or_symlinked' for r in summary.collect(link, 'hal')))
            self.assertTrue(all(r['status'] == 'missing' for r in summary.collect(root / 'missing', 'hal')))

    def test_large_json_is_explicitly_truncated_and_not_partially_parsed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'runtime-result.json').write_bytes(b'{"pass":true,"padding":"' + b'x' * summary.MAX_READ + b'"}')
            result = next(r for r in self.collect(root) if r['file'] == 'runtime-result.json')
            self.assertEqual(result['status'], 'input_truncated')
            self.assertNotIn('summary', result)
            self.assertNotIn('sha256', result)  # Only prefix hash, never pretend full custody.

    def test_large_jsonl_samples_tail_fault_without_claiming_full_observation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record = {'fault': 0, 'running': True, 'unlisted': 'x' * 1000}
            lines = [json.dumps(record)] * 300
            lines.append(json.dumps({'fault': 5, 'running': False, 'first_fault': {'fault': 5, 'origin': 'NativeOutputPacket', 'direction': 'Output', 'endpoint_id': '/private/hidden'}}))
            (root / 'runtime-native-timing.jsonl').write_text('\n'.join(lines) + '\n')
            result = next(r for r in self.collect(root) if r['file'].endswith('.jsonl'))
            self.assertEqual(result['status'], 'partial')
            self.assertTrue(result['input_truncated'])
            self.assertLessEqual(result['sampled_bytes'], summary.MAX_READ)
            self.assertEqual(result['first_sampled_fault']['fault'], 5)
            self.assertTrue(result['unsampled_fault_absence_not_established'])
            self.assertNotIn('/private/hidden', summary.render([result], 'runtime'))

    def test_malformed_duplicate_nonfinite_and_deep_inputs_fail_explicitly(self):
        fixtures = [b'{', b'{"pass":true,"pass":false}', b'{"fault":NaN}', b'{"fault":\xff}',
                    b'{"error":' + b'[' * 1100 + b'0' + b']' * 1100 + b'}']
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for raw in fixtures:
                with self.subTest(raw=raw[:24]):
                    (root / 'runtime-result.json').write_bytes(raw)
                    record = next(r for r in self.collect(root) if r['file'] == 'runtime-result.json')
                    self.assertEqual(record['status'], 'malformed_json')
            (root / 'runtime-native-timing.jsonl').write_bytes(b'{"fault":0}\n{bad\n')
            record = next(r for r in self.collect(root) if r['file'].endswith('.jsonl'))
            self.assertEqual(record['status'], 'partial_malformed')
            self.assertEqual(record['malformed_records'], 1)

    def test_output_cap_retains_reported_failure_and_explicit_omission(self):
        records = [{'file': name, 'status': 'read', 'summary': {'pass': False, 'huge': 'x' * 20000}}
                   for name in summary.FILES['runtime']]
        output = summary.render(records, 'runtime')
        self.assertLessEqual(len(output.encode()), summary.MAX_OUTPUT)
        self.assertEqual(output.count('summary_truncated'), len(records))
        self.assertEqual(output.count('reported_pass":false'), len(records))
        for record in records:
            record['summary']['huge'] = 'x' * (summary.MAX_LINE - 300)
        self.assertLessEqual(len(summary.render(records, 'runtime').encode()), summary.MAX_OUTPUT)

    def test_missing_reports_do_not_change_acceptance_exit_status_or_write_files(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            output = io.StringIO()
            with patch.object(summary, 'ROOT', root), patch('sys.argv', ['summary', '--kind', 'runtime']), contextlib.redirect_stdout(output):
                self.assertEqual(summary.main(), 0)
            self.assertIn('diagnostic_only_not_acceptance', output.getvalue())
            self.assertIn('directory_unavailable', output.getvalue())
            self.assertEqual(list(root.iterdir()), [])

    def test_runner_identity_is_available_without_manifest_and_is_allowlisted(self):
        with patch.dict(os.environ, {'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '1234', 'GITHUB_RUN_ATTEMPT': '2'}):
            header = json.loads(summary.render([], 'hal').removeprefix(summary.PREFIX))
            self.assertEqual(header['runner_identity'], {'source_revision': 'a' * 40, 'run_id': '1234', 'run_attempt': '2'})
        with patch.dict(os.environ, {'GITHUB_SHA': '/private/secret', 'GITHUB_RUN_ID': '::error::secret'}):
            output = summary.render([], 'hal')
            self.assertNotIn('secret', output)
            self.assertIn('missing_or_invalid', output)

    def test_workflow_keeps_original_result_and_always_upload_semantics(self):
        text = (ROOT / '.github/workflows/rust-hal.yml').read_text()
        for kind, runner in (('hal', 'run_system.py'), ('runtime', 'run_runtime.py')):
            start = text.index(f'run: python3 drivers/macos/tests/{runner} manifest')
            end = text.index('uses: actions/upload-artifact@v4', start)
            fragment = text[start:end]
            self.assertIn(f'run: python3 scripts/ci-acceptance-summary.py --kind {kind}', fragment)
            self.assertEqual(fragment.count('if: always()'), 2)
            self.assertIn('continue-on-error: true', fragment)
            self.assertIn('timeout-minutes: 1', fragment)
        self.assertEqual(text.count('continue-on-error: true'), 2)


if __name__ == '__main__':
    unittest.main()
