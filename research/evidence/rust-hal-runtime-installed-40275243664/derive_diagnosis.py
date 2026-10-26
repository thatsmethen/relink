#!/usr/bin/env python3
"""Read-only context for the original failed CI observation; no acceptance changes."""
import argparse
import array
import csv
import hashlib
import json
from pathlib import Path
import struct
import sys
import zipfile

HERE = Path(__file__).resolve().parent
PREFIX = 'CI_ACCEPTANCE_SUMMARY '


def digest(data):
    return hashlib.sha256(data).hexdigest()


def console_fallback(logs, artifacts):
    result = {}
    jobs = json.loads((HERE / 'original-jobs.json').read_text())['jobs']
    with zipfile.ZipFile(logs) as archive:
        for kind, job_name in [('hal', 'installed-hal'), ('runtime', 'installed-runtime-capture')]:
            member = next(n for n in archive.namelist() if n.startswith(job_name + '/')
                          and 'Print bounded report fallback' in n)
            raw = archive.read(member)
            payloads = [line.split(PREFIX, 1)[1] for line in raw.decode('utf-8-sig').splitlines() if PREFIX in line]
            records = [json.loads(line) for line in payloads]
            assert records[0]['runner_identity'] == {
                'run_id': '40275243664', 'run_attempt': '1',
                'source_revision': '23c7ed6302bcb21ae99452487f137b50301254a3'}
            lengths = [len((PREFIX + line + '\n').encode()) for line in payloads]
            assert sum(lengths) <= 49152 and max(lengths) <= 8192
            checked, sampled, missing = [], [], []
            for record in records[1:]:
                name = record['file']
                assert Path(name).name == name and not name.endswith('.wav')
                path = artifacts / kind / name
                if record['status'] == 'missing':
                    assert not path.exists()
                    missing.append(name)
                elif record['status'] == 'read':
                    data = path.read_bytes()
                    assert len(data) == record['bytes'] and digest(data) == record['sha256']
                    checked.append(name)
                elif record['status'] == 'partial':
                    data = path.read_bytes()
                    assert len(data) == record['bytes']
                    assert digest(data[:65536] + data[-65536:]) == record['sampled_sha256']
                    assert record['input_truncated'] and record['unsampled_fault_absence_not_established']
                    sampled.append(name)
                else:
                    raise ValueError('Unexpected original console record status')
            job = next(j for j in jobs if j['name'] == job_name)
            steps = job['steps']
            acceptance = next(s for s in steps if s['name'].startswith('Install, '))
            manifest = next(s for s in steps if s['name'].startswith('Hash original evidence'))
            summary = next(s for s in steps if s['name'].startswith('Print bounded report'))
            upload = next(s for s in steps if s['name'].startswith('Preserve original'))
            assert acceptance['number'] < manifest['number'] < summary['number'] < upload['number']
            assert summary['conclusion'] == upload['conclusion'] == manifest['conclusion'] == 'success'
            assert job['conclusion'] == acceptance['conclusion'] == ('success' if kind == 'hal' else 'failure')
            result[kind] = {
                'original_step_log': member, 'step_log_sha256': digest(raw),
                'summary_records': len(records), 'summary_bytes': sum(lengths), 'largest_line_bytes': max(lengths),
                'complete_report_hashes_match_archive': checked, 'partial_sample_hashes_match_archive': sampled,
                'explicitly_missing_reports_confirmed': missing,
                'original_acceptance_step_conclusion': acceptance['conclusion'],
                'summary_step_conclusion': summary['conclusion'], 'original_job_conclusion': job['conclusion'],
                'order': 'acceptance then manifest then read-only summary then upload',
                'acceptance_result_unchanged': True,
                'limits': 'Partial native telemetry is explicitly sampled; manifest claims in console are not payload verification.'}
    return result


def gap_context(artifacts):
    directory = artifacts / 'runtime'
    rows = list(csv.DictReader((directory / 'runtime-reader.csv').open()))
    report = json.loads((directory / 'runtime-reader.json').read_text())
    events = [json.loads(line) for line in (directory / 'runtime-events.jsonl').read_text().splitlines()]
    diagnostics = json.loads((directory / 'runtime-reader-diagnostics.json').read_text())
    ratio = report['host_time_numer'] / report['host_time_denom'] / 1e9
    stopped = next(e for e in events if e['event'] == 'stopped')
    wave = (directory / 'runtime-reader.wav').read_bytes()
    assert wave[:4] == b'RIFF' and wave[8:12] == b'WAVE' and wave[36:40] == b'data'
    assert struct.unpack_from('<HHIIHH', wave, 20) == (3, 2, 48000, 384000, 8, 32)
    pcm = array.array('f'); pcm.frombytes(wave[44:])
    if sys.byteorder != 'little':
        pcm.byteswap()
    gaps = []
    stored = 0
    for index, (previous, current) in enumerate(zip(rows, rows[1:])):
        stored += int(previous['stored_frames'])
        gap = float(current['sample_time']) - float(previous['sample_time']) - int(previous['frames'])
        if abs(gap) <= 0.001:
            continue
        host = int(current['host_time'])
        notifications = [dict(n, selector_fourcc=int(n['selector']).to_bytes(4, 'big').decode('ascii', 'replace'),
                              seconds_from_next_native_timestamp=(n['host_time'] - host) * ratio)
                         for n in diagnostics['notifications'] if abs((n['host_time'] - host) * ratio) <= 0.1]
        gaps.append({
            'previous_callback_index': index, 'next_callback_index': index + 1,
            'excess_sample_frames': gap, 'file_frame_offset_at_next_callback': stored,
            'previous': previous, 'next': current,
            'seconds_after_first_recorder_native_timestamp': (host - int(rows[0]['host_time'])) * ratio,
            'seconds_after_runtime_stop_request': (host - stopped['requested_host_time']) * ratio,
            'native_host_delta_seconds': (host - int(previous['host_time'])) * ratio,
            'observed_host_delta_seconds': (int(current['observed_host_time']) - int(previous['observed_host_time'])) * ratio,
            'original_diagnostic_callback_timings': diagnostics['timings'][index:index + 2],
            'nearby_notifications': notifications,
            'pcm_peak_within_2048_frames_each_side': max(abs(v) for v in pcm[max(0, stored - 2048) * 2:(stored + 2048) * 2])})
    assert len(gaps) == 1 and gaps[0]['excess_sample_frames'] == 588.0
    return {
        'original_acceptance_remains_failed': True, 'gaps': gaps,
        'inputs': {name: digest((directory / name).read_bytes()) for name in
                   ['runtime-reader.wav', 'runtime-reader.csv', 'runtime-reader.json',
                    'runtime-events.jsonl', 'runtime-reader-diagnostics.json']},
        'interpretation': 'The gap follows runtime stop and coincides with a recorded over notification and late observer entry. Correlation does not establish driver, observer, scheduler, or DSP causality.',
        'protocol_limit': 'The frozen runtime probe changes send gain/mute only; it does not edit DSP effects.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--logs', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps({'source_revision': '23c7ed6302bcb21ae99452487f137b50301254a3',
                      'run_id': 40275243664,
                      'console_fallback': console_fallback(args.logs, args.artifacts),
                      'failure_context': gap_context(args.artifacts)}, indent=2, allow_nan=False))
