#!/usr/bin/env python3
"""Verify original custody and independently recompute the frozen PCM analysis.

No device operations, network calls, reruns or changes to original reports.
The two analyzer modules are read from the exact local Git source commit.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SOURCE = 'f47df76d100da25c5e98d261d92792a1d507d5f1'


def digest(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def original_json(archive, name):
    return json.loads(archive.read(name))


def derive(archives):
    with zipfile.ZipFile(archives / 'original-hal.zip') as hal, zipfile.ZipFile(archives / 'original-runtime.zip') as runtime:
        hal_acceptance = original_json(hal, 'acceptance.json')
        acceptance = original_json(runtime, 'runtime-acceptance.json')
        result = original_json(runtime, 'runtime-result.json')
        analysis = original_json(runtime, 'runtime-output-analysis.json')
        lines = runtime.read('runtime-native-timing.jsonl').splitlines(keepends=True)
        native = [json.loads(line) for line in lines]
        terminal = native[-1]
        output = terminal['output_timing']
        lifecycle = {event['phase']: event for event in output['lifecycle']}
        assert hal_acceptance['pass'] and not hal_acceptance['exceptions']
        assert acceptance['pass'] and not acceptance['exceptions']
        assert result == {'schema': 1, 'pass': True, 'native_output_shutdown_proven': True, 'error': None}
        assert analysis['pass'] and not analysis['failures'] and len(analysis['phases']) == 4
        assert all(record['fault'] == 0 and record['first_fault'] is None and record['error'] is None for record in native)
        assert terminal['observation'] == 'post-stop-final' and not terminal['running']
        assert terminal['native_output_shutdown_proven'] and terminal['native_output_shutdown_observed']
        assert output['backend'] == 'coreaudio' and output['first_fault'] is None and not output['first_fault_pending']
        assert 'ContextRetained' not in lifecycle
        for phase in ('StartReturned', 'StopReturned', 'DestroyReturned', 'ListenerRemovalReturned'):
            assert lifecycle[phase]['status'] == 0, phase
        for phase in ('QuiescenceChecked', 'OwnerCompleted'):
            assert lifecycle[phase]['callbacks_in_flight'] == 0, phase
        assert lifecycle['OwnerCompleted']['host_time'] >= lifecycle['QuiescenceChecked']['host_time']
        cleanups = {}
        for kind, archive in (('hal', hal), ('runtime', runtime)):
            cleanup = original_json(archive, 'cleanup.json')
            assert cleanup['pass']
            assert len(cleanup['devices']) == 2
            assert all(device['matches'] == 0 for device in cleanup['devices'].values())
            cleanups[kind] = {'pass': True, 'both_owned_devices_absent': True, 'attempt': cleanup['attempt']}

        # Recompute from unchanged source and original PCM/clock/control inputs.
        analyzer_identities = {}
        with tempfile.TemporaryDirectory(prefix='relink-frozen-pcm-') as scratch:
            temporary = Path(scratch)
            for module in ('analyze_system', 'analyze_runtime'):
                relative = f'drivers/macos/tests/{module}.py'
                code = subprocess.check_output(['git', 'show', f'{SOURCE}:{relative}'], cwd=ROOT)
                (temporary / f'{module}.py').write_bytes(code)
                analyzer_identities[relative] = digest(code)
            for name in ('runtime-reader.json', 'runtime-reader.csv', 'runtime-reader.wav', 'runtime-result.json', 'runtime-events.jsonl', 'runtime-telemetry.csv'):
                (temporary / name).write_bytes(runtime.read(name))
            previous = sys.modules.pop('analyze_system', None)
            sys.path.insert(0, str(temporary))
            try:
                spec = importlib.util.spec_from_file_location('relink_frozen_analyzer', temporary / 'analyze_runtime.py')
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                recomputed = module.analyze(temporary)
            finally:
                sys.path.pop(0)
                sys.modules.pop('analyze_system', None)
                if previous is not None:
                    sys.modules['analyze_system'] = previous
        assert recomputed == analysis, 'Recomputed report differs; preserve both observations and investigate'
        assert digest(lines[-1]) == digest((HERE / 'runtime-terminal-original.json').read_bytes())
        return {
            'schema': 1, 'source_revision': SOURCE, 'run_id': 40249452671,
            'overall_conclusion': 'PASS', 'hal_conclusion': 'PASS', 'runtime_conclusion': 'PASS',
            'original_pcm_analysis_recomputed_exactly': True, 'analyzer_sources': analyzer_identities,
            'criteria': analysis['criteria'], 'independent_reader_clock': analysis['clock'],
            'phases': [{key: phase[key] for key in ('phase', 'pass', 'acknowledgement_seconds', 'detected_response_seconds')} for phase in analysis['phases']],
            'unmuted_zero_runs': analysis['unmuted_zero_runs'],
            'stopped_tail_peak': analysis['stopped_tail_peak'],
            'stop_response_seconds': analysis['stop_response_seconds'],
            'native_timing_rows': len(native), 'native_output_callbacks_seen': output['callbacks_seen'],
            'native_output_progress_count': output['progress_count'],
            'native_output_shutdown_proven': True, 'native_output_lifecycle': output['lifecycle'],
            'all_observed_runtime_faults_absent': True, 'driver_cleanup': cleanups,
            'limits': 'One short hosted macOS virtual-cable run. No physical drift, sustained reliability, multi-output or native Windows qualification; input OwnerCompleted has its earlier implementation semantics.'
        }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--archives', type=Path, default=ROOT / 'build/rust-native-output-installed-40249452671')
    parser.add_argument('--record', action='store_true')
    args = parser.parse_args()
    collection = json.loads((HERE / 'collection.json').read_text())
    assert collection['source_revision'] == SOURCE and collection['overall_conclusion'] == 'PASS'
    total = 0
    for item in collection['archives']:
        data = (args.archives / item['archive']).read_bytes()
        assert digest(data) == {key: item[key] for key in ('bytes', 'sha256')}
        with zipfile.ZipFile(args.archives / item['archive']) as archive:
            assert archive.testzip() is None
            if item['kind'] == 'workflow-logs':
                continue
            manifest = original_json(archive, 'custody-manifest.json')
            assert manifest['source_revision'] == SOURCE
            expected = {'custody-manifest.json'}
            for member in manifest['files']:
                expected.add(member['path'])
                assert digest(archive.read(member['path'])) == {key: member[key] for key in ('bytes', 'sha256')}
                total += 1
            assert set(archive.namelist()) == expected
            for name in item['compact_original_text_members']:
                assert (HERE / (item['kind'] + '-original-text') / name).read_bytes() == archive.read(name)
    derived = derive(args.archives)
    target = HERE / 'derived-verification.json'
    if args.record and not target.exists():
        target.write_text(json.dumps(derived, indent=2, sort_keys=True) + '\n')
    assert json.loads(target.read_text()) == derived
    print(json.dumps({'original_archives_verified': 3, 'original_manifest_files_verified': total,
        'pcm_analysis_recomputed_exactly': True, 'native_output_cleanup_gate_verified': True,
        'overall_result': 'PASS', 'audio_or_network_operations': False}, indent=2))


if __name__ == '__main__':
    main()
