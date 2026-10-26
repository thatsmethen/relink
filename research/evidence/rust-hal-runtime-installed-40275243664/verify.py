#!/usr/bin/env python3
"""Verify downloaded CI custody and re-analyze generated PCM offline.

Never downloads, starts a process/device, records, repairs, or rewrites original
observations. --archives contains hal.zip/runtime.zip; --extracted reuses one
verified extraction or creates it once. The stdout result is derived evidence.
"""
import argparse
import hashlib
import importlib
import json
from pathlib import Path, PurePosixPath
import platform
import shutil
import stat
import sys
from types import SimpleNamespace
import zipfile

HERE = Path(__file__).resolve().parent
SOURCE = '23c7ed6302bcb21ae99452487f137b50301254a3'
RUN = 40275243664


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def load(path):
    return json.loads(path.read_text())


def verify_archive(label, archives, extracted):
    record = next(r for r in load(HERE / 'archive-custody.json') if r['label'] == label)
    asset = next(a for a in load(HERE / 'original-artifacts.json')['artifacts'] if a['id'] == record['artifact_id'])
    path = archives / record['archive']
    require(path.stat().st_size == record['bytes'] == asset['size_in_bytes'], 'Archive size mismatch')
    digest = sha(path)
    require(digest == record['sha256'] and asset['digest'] == 'sha256:' + digest, 'Archive digest mismatch')
    require(asset['workflow_run']['head_sha'] == SOURCE and asset['workflow_run']['id'] == RUN, 'Artifact source mismatch')
    actual = {}
    destination = extracted / label
    creating = not destination.exists()
    require(not destination.is_symlink(), 'Symlinked extraction directory')
    with zipfile.ZipFile(path) as archive:
        members = archive.infolist()
        require(sum(member.file_size for member in members) < 512 * 1024 * 1024, 'Unexpected expanded size')
        for member in members:
            relative = PurePosixPath(member.filename)
            require(bool(relative.parts) and not relative.is_absolute() and '..' not in relative.parts and '\\' not in member.filename
                    and str(relative) == member.filename and member.filename not in actual
                    and not member.is_dir() and not stat.S_ISLNK(member.external_attr >> 16), 'Unsafe/repeated ZIP member')
            digest = hashlib.sha256()
            with archive.open(member) as file:
                for chunk in iter(lambda: file.read(1024 * 1024), b''):
                    digest.update(chunk)
            actual[member.filename] = {'bytes': member.file_size, 'sha256': digest.hexdigest()}
        manifest = json.loads(archive.read('custody-manifest.json'))
        require(manifest['source_revision'] == SOURCE and str(manifest['run_id']) == str(RUN)
                and str(manifest['run_attempt']) == '1', 'Manifest source/run mismatch')
        expected = {r['path']: {'bytes': r['bytes'], 'sha256': r['sha256']} for r in manifest['files']}
        require(len(expected) == len(manifest['files']), 'Duplicate manifest path')
        require({k: v for k, v in actual.items() if k != 'custody-manifest.json'} == expected, 'Manifest membership/hash mismatch')
        for copied in load(HERE / 'compact-originals.json'):
            if copied['artifact'] == label:
                require(actual[copied['member']] == {'bytes': copied['bytes'], 'sha256': copied['sha256']}, 'Compact copy manifest differs from archive')
                require((HERE / copied['file']).stat().st_size == copied['bytes'] and sha(HERE / copied['file']) == copied['sha256'], 'Compact original copy differs from archive')
        if creating:
            destination.mkdir(parents=True)
            for member in members:
                target = destination / member.filename
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(member) as original, target.open('xb') as copied:
                    shutil.copyfileobj(original, copied)
    for name, expected_file in actual.items():
        target = destination / name
        require(target.resolve() == target.absolute() and target.is_file() and not target.is_symlink(), 'Unsafe extracted path')
        require(target.stat().st_size == expected_file['bytes'] and sha(target) == expected_file['sha256'], 'Extracted bytes differ')
    require({str(p.relative_to(destination)) for p in destination.rglob('*') if p.is_file()} == set(actual), 'Unexpected extraction file')
    return destination, dict(record, manifest_files_verified=len(expected),
                             expanded_bytes=sum(r['bytes'] for r in actual.values()),
                             manifest_sha256=actual['custody-manifest.json']['sha256'])


def compare_result(actual, original):
    # source_report is a provenance path, not a numeric observation. No PCM or
    # acceptance value is adjusted. Report any other cross-host result difference.
    clean = lambda value: {k: v for k, v in value.items() if k != 'source_report'}
    return clean(actual) == clean(original)


def require_original_matches(checks):
    require(all(check['exact_original_result_except_source_path'] is True for check in checks.values()),
            'Recomputed result differs from original observation')


def verify(archives, extracted):
    run = load(HERE / 'original-run.json')
    require(run['head_sha'] == SOURCE and run['id'] == RUN and run['run_attempt'] == 1
            and run['conclusion'] == 'failure', 'Run identity/conclusion mismatch')
    jobs = load(HERE / 'original-jobs.json')['jobs']
    require({j['name']: j['conclusion'] for j in jobs} == {'installed-hal': 'success', 'installed-runtime-capture': 'failure'}, 'Original job outcomes changed')
    sources = load(HERE / 'source-custody.json')
    require(sources['source_revision'] == SOURCE, 'Analyzer source revision mismatch')
    for name, digest in sources['sha256'].items():
        require(sha(HERE / 'source' / name) == digest, 'Frozen analyzer/workflow bytes changed')
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(HERE / 'source'))
    system = importlib.import_module('analyze_system')
    runtime = importlib.import_module('analyze_runtime')
    diagnostics = importlib.import_module('analyze_diagnostics')
    harness = importlib.import_module('run_system')
    for module in (system, runtime, diagnostics, harness):
        require(Path(module.__file__).resolve() == HERE / 'source' / (module.__name__ + '.py'), 'Analyzer module came from another checkout')
    results = {'source_revision': SOURCE, 'run_id': RUN, 'attempt': 1,
               'verification_python': sys.version, 'verification_platform': platform.platform(), 'jobs': {}}
    for label in ('hal', 'runtime'):
        directory, custody = verify_archive(label, archives, extracted)
        build_hashes = dict(reversed(line.split(None, 1)) for line in (directory / 'build-sha256.txt').read_text().splitlines())
        verified_build_sources = {}
        for name, digest in sources['sha256'].items():
            original_path = sources['original_paths'][name]
            if original_path in build_hashes:
                require(build_hashes[original_path] == digest, 'Frozen source differs from recorded CI build hash')
                verified_build_sources[original_path] = digest
        probe_path = 'build/rust-' + ('hal' if label == 'hal' else 'runtime') + '-system/system-probe'
        require(build_hashes[probe_path] == sha(directory / 'system-probe'), 'Archived independent observer differs from recorded build')
        acceptance = load(directory / ('acceptance.json' if label == 'hal' else 'runtime-acceptance.json'))
        cleanup = load(directory / 'cleanup.json')
        require(acceptance['pass'] is (label == 'hal'), 'Original acceptance outcome changed')
        require(cleanup['pass'], 'Original cleanup report did not pass')
        require(all(p['exit_code'] == 0 and not p['timed_out'] for p in acceptance['processes']), 'Original child process failed')
        for uid in ('au.textual.ravelink.cable.1', 'au.textual.ravelink.cable.2'):
            absent = cleanup['devices'][uid]
            require(absent['complete'] and absent['status'] == 0 and absent['matches'] == 0, 'Cleanup absence not proved')
        checks = {}
        names = ('tone-writer', 'reader-a', 'reader-b', 'restarted-silence') if label == 'hal' else ('source-writer',)
        for name in names:
            writer = name in ('tone-writer', 'source-writer')
            result = system.analyze(directory / (name + '.json'), directory / (name + '.csv'),
                                    None if writer else directory / (name + '.wav'), silence=name == 'restarted-silence')
            original = load(directory / (name + '-analysis.json'))
            checks[name] = {'pass': result['pass'], 'exact_original_result_except_source_path': compare_result(result, original), 'result': result}
        if label == 'hal':
            args = [SimpleNamespace(wav=directory / f'reader-{c}.wav', events=directory / f'reader-{c}.csv') for c in ('a', 'b')]
            pair = harness.pair_consistency(*args)
            checks['independent-readers'] = {'pass': pair['pass'], 'exact_original_result_except_source_path': pair == load(directory / 'independent-readers.json'), 'result': pair}
        else:
            result = runtime.analyze(directory)
            checks['runtime-output'] = {'pass': result['pass'], 'exact_original_result_except_source_path': result == load(directory / 'runtime-output-analysis.json'), 'result': result}
            require(load(directory / 'runtime-result.json')['native_output_shutdown_proven'], 'Native output shutdown not proved')
        for name in (('tone-writer', 'reader-a', 'reader-b', 'restarted-silence') if label == 'hal' else ('source-writer', 'runtime-reader')):
            result = diagnostics.summarize(directory / (name + '-diagnostics.json'), directory / (name + '.csv'), directory / (name + '.json'))
            checks[name + '-diagnostics'] = {'pass': result['pass'], 'exact_original_result_except_source_path': result == load(directory / (name + '-diagnostics-analysis.json'))}
        require_original_matches(checks)
        pcm = {path.name: {'bytes': path.stat().st_size, 'sha256': sha(path)} for path in sorted(directory.glob('*.wav'))}
        results['jobs'][label] = {'custody': custody, 'original_acceptance_pass': acceptance['pass'], 'original_cleanup_pass': True,
                                  'both_cable_UIDs_absent_after_cleanup': True, 'pcm': pcm, 'reanalyzed': checks,
                                  'frozen_sources_matching_CI_build_hashes': verified_build_sources,
                                  'archived_observer_build_hash_verified': True,
                                  'reported_driver_binary_sha256': build_hashes['dist/macos/ReLink.driver/Contents/MacOS/ReLinkHAL']}
        if label == 'runtime':
            native = [json.loads(line) for line in (directory / 'runtime-native-timing.jsonl').read_text().splitlines()]
            last = native[-1]
            results['jobs'][label]['native_record_summary'] = {
                'records': len(native),
                'records_with_runtime_fault_or_error': sum(bool(r.get('fault')) or r.get('first_fault') is not None or r.get('error') is not None for r in native),
                'last_observation': last['observation'], 'last_running': last['running'],
                'last_native_output_shutdown_proven': last['native_output_shutdown_proven'],
                'last_native_shutdown_observed': last['native_shutdown_observed'],
                'last_output_lifecycle': last['output_timing']['lifecycle'],
                'note': 'Recorded telemetry summary, separate from unchanged PCM analyzer results.'}
    results['custody_and_reproduction_verified'] = True
    results['original_run_conclusion'] = 'failure'
    results['original_runtime_acceptance_pass'] = False
    results['limits'] = ['One already-completed disposable macOS arm64 run at the exact source revision.',
                         'Generated CI tones and fixed protocol only; no local device or recording operation occurred.',
                         'Does not qualify later working-tree changes, Windows routing, application capture, physical clocks, or production release.',
                         'Original failed runs40270207703 and40275243664 remain unchanged; verification does not convert a failed observation to a pass.']
    return results


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archives', type=Path, required=True)
    parser.add_argument('--extracted', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(verify(args.archives.resolve(), args.extracted.resolve()), indent=2, allow_nan=False))
