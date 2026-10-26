#!/usr/bin/env python3
"""Derive bounded chronology from original ZIP members, without rescoring PCM."""
import argparse
import csv
import io
import json
from pathlib import Path
import zipfile

HERE = Path(__file__).resolve().parent
DEFAULT = HERE.parents[2] / 'build/rust-hal-failed-40051763048'


def read(archive, name):
    return json.loads(archive.read(name))


def diagnose(archives):
    with zipfile.ZipFile(archives / 'original-hal.zip') as hal, zipfile.ZipFile(archives / 'original-runtime.zip') as runtime:
        acceptance = read(hal, 'acceptance.json')
        writer = read(hal, 'tone-writer-diagnostics.json')
        rows = list(csv.DictReader(io.StringIO(hal.read('tone-writer.csv').decode('utf-8'))))
        scale = writer['host_time_numer'] / writer['host_time_denom'] / 1e9
        gaps = []
        for index in range(1, len(rows)):
            previous, current = rows[index - 1], rows[index]
            delta = float(current['sample_time']) - float(previous['sample_time']) - int(previous['frames'])
            if delta == 0:
                continue
            before, after = writer['timings'][index - 1], writer['timings'][index]
            assert before['index'] == index - 1 and after['index'] == index
            def context(row, timing):
                return {'callback_index_zero_based': timing['index'], 'sample_time': float(row['sample_time']), 'frames': int(row['frames']), 'native_host_time': int(row['host_time']), **timing, 'entry_minus_output_seconds': (timing['entry_host_time'] - int(row['host_time'])) * scale, 'work_seconds': (timing['exit_host_time'] - timing['entry_host_time']) * scale}
            gaps.append({'current_callback_index_zero_based': index, 'missing_frames': delta, 'previous': context(previous, before), 'current': context(current, after), 'native_host_interval_seconds': (int(current['host_time']) - int(previous['host_time'])) * scale, 'sample_interval_seconds_at_48000': (float(current['sample_time']) - float(previous['sample_time'])) / 48000, 'overload_notifications': [{**notification, 'relative_to_previous_entry_seconds': (notification['host_time'] - before['entry_host_time']) * scale} for notification in writer['notifications'] if notification['selector'] == int.from_bytes(b'over', 'big')], 'previous_entry_after_reader_stop_seconds': {name: (before['entry_host_time'] - read(hal, name + '-diagnostics.json')['lifecycle']['stop_return']) * scale for name in ('reader-a', 'reader-b')}})
        runtime_acceptance = read(runtime, 'runtime-acceptance.json')
        phases = next(item for item in runtime_acceptance['observations'] if 'phases' in item)
        terminal = json.loads(runtime.read('runtime-native-timing.jsonl').splitlines()[-1])
        capture = terminal['captures'][0]['native_timing']
        result = {
            'schema': 1, 'source_revision': '91d73e4cf0ecf820d93a68f73d59f1cb09d00b13',
            'overall_conclusion': 'FAIL',
            'hal': {'pass': acceptance['pass'], 'failed_original_observations': [item for item in acceptance['observations'] if not item['pass']], 'gaps': gaps, 'all_processes_exited_zero_without_timeout': all(item['exit_code'] == 0 and not item['timed_out'] for item in acceptance['processes']), 'cleanup': read(hal, 'cleanup.json')},
            'runtime': {'pass': runtime_acceptance['pass'], 'result': read(runtime, 'runtime-result.json'), 'all_processes_exited_zero_without_timeout': all(item['exit_code'] == 0 and not item['timed_out'] for item in runtime_acceptance['processes']), 'phase_passes': [item['pass'] for item in phases['phases']], 'acknowledgement_seconds': [item['acknowledgement_seconds'] for item in phases['phases']], 'stopped_tail_peak': phases['stopped_tail_peak'], 'stop_response_seconds': phases['stop_response_seconds'], 'terminal_fault': terminal['fault'], 'terminal_first_fault': terminal['first_fault'], 'native_shutdown_observed': terminal['native_shutdown_observed'], 'capture_packets_seen': capture['packets_seen'], 'capture_first_fault': capture['first_fault'], 'capture_lifecycle': capture['lifecycle'], 'cleanup': read(runtime, 'cleanup.json')},
            'limits': ['Derived chronology only; original acceptance results and thresholds are unchanged.', 'The overload notification and sample gap do not establish which scheduling, host or processing condition caused the missed deadline.', 'The two jobs ran on different hosted machines; their host clocks cannot be cross-correlated.', 'This source used native macOS capture and CPAL output. The subsequently authored native output owners are not tested by this run.', 'No rerun, threshold adjustment, publication, local device access or repaired observation.'],
        }
        return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--archives', type=Path, default=DEFAULT)
    args = parser.parse_args()
    print(json.dumps(diagnose(args.archives), indent=2, sort_keys=True))
