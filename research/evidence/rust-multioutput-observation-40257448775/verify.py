#!/usr/bin/env python3
"""Offline verification of one frozen two-output observation; no device/network/Git.

Calls only pinned pure analysis functions on temporary copies of original inputs.
Original files, protocol, thresholds, source and acquisition are never changed.
"""
import argparse
import hashlib
import importlib
import json
from pathlib import Path, PurePosixPath
import sys
import tempfile
import zipfile

HERE = Path(__file__).resolve().parent
SOURCE = '69f49da28a9bae16dec9d4bc29bd0d639fb73a08'
RUN = 40257448775


def digest(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def normalized(value):
    return json.loads(json.dumps(value, allow_nan=False))


def derive(archives):
    collection = json.loads((HERE / 'collection.json').read_text())
    assert collection['source_revision'] == SOURCE and collection['run_id'] == RUN
    assert collection['run_attempt'] == 1
    assert collection['collector_sha256'] == digest((HERE / 'collect.py').read_bytes())['sha256']
    local = json.loads((HERE / 'original-local-files.json').read_text())
    for name, identity in local.items():
        assert digest((archives / name).read_bytes()) == identity, name
    run = json.loads((archives / 'original-run.json').read_text())
    assert run['head_sha'] == SOURCE and run['run_attempt'] == 1 and run['status'] == 'completed'
    assert run['conclusion'] == 'success'
    identities = json.loads((HERE / 'pinned-source-identities.json').read_text())
    assert identities['source_revision'] == SOURCE
    for item in identities['files']:
        data = (HERE / 'pinned-source' / item['path']).read_bytes()
        assert digest(data) == {key: item[key] for key in ('bytes', 'sha256')}
        assert hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest() == item['git_blob_sha']
    archive_identity = next(a for a in collection['archives'] if a['kind'] == 'multioutput')
    for item in collection['archives']:
        data = (archives / item['archive']).read_bytes()
        assert digest(data) == {key: item[key] for key in ('bytes', 'sha256')}
        if item['github_digest'] is not None:
            assert item['github_digest'] == 'sha256:' + digest(data)['sha256']
        with zipfile.ZipFile(archives / item['archive']) as archive:
            assert archive.testzip() is None
    with zipfile.ZipFile(archives / 'original-multioutput.zip') as archive:
        names = archive.namelist()
        assert len(names) == len(set(names))
        assert all(not PurePosixPath(name).is_absolute() and '..' not in PurePosixPath(name).parts for name in names)
        manifest = json.loads(archive.read('custody-manifest.json'))
        assert manifest['source_revision'] == SOURCE and int(manifest['run_id']) == RUN and int(manifest['run_attempt']) == 1
        assert set(names) == {'custody-manifest.json'} | {item['path'] for item in manifest['files']}
        for item in manifest['files']:
            assert digest(archive.read(item['path'])) == {key: item[key] for key in ('bytes', 'sha256')}
        for name in archive_identity['compact_original_text_members']:
            assert (HERE / 'multioutput-original-text' / name).read_bytes() == archive.read(name), name
        build_hashes = dict((line.split(maxsplit=1)[1], line.split(maxsplit=1)[0]) for line in archive.read('build-sha256.txt').decode().splitlines())
        for item in identities['files']:
            assert build_hashes[item['path']] == item['sha256'], item['path']
        assert build_hashes['build/rust-multioutput-system/system-probe'] == digest(archive.read('system-probe'))['sha256']
        acceptance = json.loads(archive.read('multioutput-acceptance.json'))
        result = json.loads(archive.read('multioutput-result.json'))
        original = json.loads(archive.read('multioutput-analysis.json'))
        cleanup = json.loads(archive.read('cleanup.json'))
        assert acceptance['pass'] is True and not acceptance['exceptions']
        assert acceptance['observer_diagnostics_enabled'] is True
        assert len(acceptance['processes']) == 3
        assert all(p['exit_code'] == 0 and not p['timed_out'] and not p['term_sent'] and not p['kill_sent'] and p['supervision_error'] is None for p in acceptance['processes'])
        assert cleanup['pass'] is True and len(cleanup['devices']) == 2
        assert all(d['matches'] == 0 and d['status'] == 0 and d['complete'] for d in cleanup['devices'].values())
        lines = archive.read('multioutput-telemetry.jsonl').splitlines(keepends=True)
        rows = [json.loads(line) for line in lines]
        terminal = rows[-1]
        assert (HERE / 'multioutput-terminal-original.json').read_bytes() == lines[-1]
        assert all(row['fault'] == 0 and row['first_fault'] is None and row['error'] is None and row['shutdown_fault_observed'] is False for row in rows)
        assert terminal['observation'] == 'post-stop-final' and not terminal['running']
        assert terminal['native_outputs_shutdown_proven'] is True and len(terminal['outputs']) == 2
        lifecycles = []
        for route in terminal['outputs']:
            native = route['native_timing']
            assert route['shutdown_observed'] and route['shutdown_proven']
            assert native['first_fault'] is None and not native['first_fault_pending']
            phases = {e['phase']: e for e in native['lifecycle']}
            assert len(phases) == len(native['lifecycle']) and 'ContextRetained' not in phases
            statuses = ('CreateReturned', 'StartReturned', 'StopReturned', 'DestroyReturned', 'ListenerRemovalReturned')
            assert all(phases[p]['status'] == 0 for p in statuses)
            assert all(phases[p]['callbacks_in_flight'] == 0 for p in ('QuiescenceChecked', 'OwnerCompleted'))
            order = ('CreateRequested', 'CreateReturned', 'StartRequested', 'StartReturned', 'StopRequested', 'StopReturned', 'DestroyRequested', 'DestroyReturned', 'ListenerRemovalRequested', 'ListenerRemovalReturned', 'QuiescenceChecked', 'OwnerCompleted')
            times = [phases[p]['host_time'] for p in order]
            assert all(t > 0 for t in times) and times == sorted(times)
            lifecycles.append({'route_index': route['route_index'], 'output_id': route['output_id'], 'submitted_frames': route['submitted_frames'], 'callbacks_seen': native['callbacks_seen'], 'progress_count': native['progress_count'], 'successful_statuses': True, 'zero_in_flight_at_quiescence_and_completion': True, 'retained_context': False, 'lifecycle': native['lifecycle']})
        events = [json.loads(line) for line in archive.read('multioutput-events.jsonl').splitlines()]
        modules = ('analyze_multioutput', 'analyze_runtime', 'analyze_system', 'analyze_diagnostics')
        previous = {name: sys.modules.pop(name, None) for name in modules}
        sys.path.insert(0, str(HERE / 'pinned-source/drivers/macos/tests'))
        prior_bytecode, sys.dont_write_bytecode = sys.dont_write_bytecode, True
        try:
            analyzer = importlib.import_module('analyze_multioutput')
            diagnostic = importlib.import_module('analyze_diagnostics')
            with tempfile.TemporaryDirectory(prefix='relink-frozen-multioutput-') as temporary:
                target = Path(temporary)
                for name in names:
                    path = PurePosixPath(name)
                    if len(path.parts) == 1 and path.suffix in ('.json', '.jsonl', '.csv', '.wav'):
                        (target / name).write_bytes(archive.read(name))
                actual = normalized(analyzer.analyze(target))
                assert actual == original, 'Recomputed fixed analysis differs from original'
                assert actual['pass'] is True and not actual['failures']
                diagnostics = []
                for index in range(2):
                    stem = f'multioutput-reader-{index}'
                    computed = normalized(diagnostic.summarize(target / (stem + '-diagnostics.json'), target / (stem + '.csv'), target / (stem + '.json')))
                    saved = json.loads(archive.read(stem + '-diagnostics-analysis.json'))
                    assert computed == saved, stem
                    assert computed == actual['routes'][index]['diagnostics']
                    diagnostics.append(computed)
                assert acceptance['observations'] == [*diagnostics, actual]
        finally:
            sys.dont_write_bytecode = prior_bytecode
            sys.path.pop(0)
            for name, value in previous.items():
                sys.modules.pop(name, None)
                if value is not None: sys.modules[name] = value
        report = json.loads(archive.read('multioutput-reader-0.json'))
        scale = report['host_time_numer'] / report['host_time_denom'] / 1e9
        phases = [event for event in events if event['event'] == 'phase']
        acknowledgments = [{'phase': p['phase'], 'all_routes_ack_seconds': (p['acknowledged_host_time'] - p['requested_host_time']) * scale} for p in phases]
        return {'schema': 1, 'run_id': RUN, 'source_revision': SOURCE, 'overall_conclusion': 'PASS',
                'all_original_manifest_members_verified': len(manifest['files']), 'exact_archive_members': len(names),
                'original_analysis_recomputed_without_field_exclusions': True,
                'original_diagnostic_reports_recomputed_without_field_exclusions': 2,
                'original_acceptance_observations_match': True,
                'fixed_analysis': actual, 'runtime_result': result, 'runtime_processes': acceptance['processes'],
                'native_telemetry_rows': len(rows), 'no_global_or_native_faults_observed': True,
                'native_cleanup': lifecycles, 'control_acknowledgments': acknowledgments,
                'driver_cleanup': cleanup, 'stop_request_to_return_seconds': (events[-1]['returned_host_time'] - events[-1]['requested_host_time']) * scale,
                'scope': 'One 18-second fixed five-phase internal-tone protocol on two original virtual outputs, two independent 32-second recordings on one macOS CI host. No audio capture, physical playback, acoustic synchronization, long-duration/load/restart stability, arbitrary route counts, or installed Windows qualification.',
                'uncertainty': 'Polled native records establish software submissions, not physical playback. Wrong-bus projection is the fixed finite-window diagnostic, not a guaranteed physical crosstalk bound. A passing new protocol does not replace or repair older failed observations.'}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--archives', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(derive(args.archives.resolve()), indent=2, sort_keys=True, allow_nan=False))


if __name__ == '__main__':
    main()
