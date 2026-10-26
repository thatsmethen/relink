#!/usr/bin/env python3
"""Acquire one completed original workflow observation; never run audio or publish.

Writes original API bodies/ZIPs under build, verifies all artifact manifest
members in-place, and copies compact original text members into this evidence
folder. Existing files must match exactly and are never overwritten. Stdlib only.
"""
import base64
import datetime
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent
BUILD = ROOT / 'build/rust-application-checkpoint-observation-40261653701'
REPO = 'thatsmethen/relink'
RUN = 40261653701
SOURCE = '353399309004a1be5a196ebaa68c08218aff0555'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def save(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        if path.read_bytes() != data:
            raise ValueError(f'Refusing to overwrite different original bytes: {path}')
    else:
        with path.open('xb') as stream:
            stream.write(data)


def write(path, data):
    save(path, (json.dumps(data, indent=2, sort_keys=True) + '\n').encode('utf-8'))


def fetch(relative, filename):
    path = BUILD / filename
    if path.exists():
        return path.read_bytes()
    if not filename.endswith('.zip'):
        result = subprocess.run(['gh', 'api', f'repos/{REPO}/{relative}'], capture_output=True, timeout=45, check=True)
        save(path, result.stdout)
        return result.stdout
    expected = None
    if relative.startswith('actions/artifacts/'):
        artifact_id = int(relative.split('/')[2])
        expected = next(a for a in json.loads((BUILD / 'original-artifacts.json').read_text())['artifacts'] if a['id'] == artifact_id)
    for index in range(1, 4):
        prefix = BUILD / 'download-attempts' / f'{filename}.attempt{index:02}'
        candidate = prefix.with_suffix(prefix.suffix + '.bytes')
        metadata = prefix.with_suffix(prefix.suffix + '.json')
        if metadata.exists():
            prior = json.loads(metadata.read_text())
            if prior['accepted']:
                save(path, candidate.read_bytes())
                return path.read_bytes()
            continue
        assert not candidate.exists(), 'Unrecorded attempt bytes must be reviewed before retrying'
        candidate.parent.mkdir(parents=True, exist_ok=True)
        started = datetime.datetime.now(datetime.timezone.utc).isoformat()
        record = {'url': f'https://api.github.com/repos/{REPO}/{relative}', 'attempt': index, 'started_at_utc': started, 'deadline_seconds': 55, 'fresh_destination': candidate.relative_to(BUILD).as_posix()}
        with candidate.open('xb') as stream:
            try:
                result = subprocess.run(['gh', 'api', f'repos/{REPO}/{relative}'], stdout=stream, stderr=subprocess.PIPE, timeout=55)
                record.update(exit_code=result.returncode, timed_out=False)
                stderr = result.stderr
            except subprocess.TimeoutExpired as error:
                record.update(exit_code=None, timed_out=True)
                stderr = error.stderr or b''
        save(prefix.with_suffix(prefix.suffix + '.stderr'), stderr)
        data = candidate.read_bytes()
        record.update(completed_at_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), bytes=len(data), sha256=sha(data), expected_github_digest=None if expected is None else expected['digest'], expected_api_bytes=None if expected is None else expected['size_in_bytes'])
        try:
            with zipfile.ZipFile(candidate) as archive:
                record['zip_crc_error'] = archive.testzip()
                record['zip_opened'] = True
                record['unique_zip_members'] = len(archive.namelist()) == len(set(archive.namelist()))
        except (zipfile.BadZipFile, OSError, RuntimeError) as error:
            record.update(zip_opened=False, zip_error_type=type(error).__name__, zip_error=str(error))
        record['github_digest_matches'] = None if expected is None else expected['digest'] == 'sha256:' + sha(data)
        record['accepted'] = record['exit_code'] == 0 and not record['timed_out'] and record['zip_opened'] and record.get('zip_crc_error') is None and record.get('unique_zip_members') and (expected is None or (record['github_digest_matches'] and len(data) == expected['size_in_bytes']))
        write(metadata, record)
        print(f"Download {filename} attempt {index}: {len(data)} bytes; accepted={record['accepted']}", flush=True)
        if record['accepted']:
            save(path, data)
            return data
    raise RuntimeError(f'Three bounded download attempts failed for {filename}; exact partial bytes and metadata preserved, no original accepted')


def safe(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts:
        raise ValueError(f'Unsafe ZIP member: {name}')
    return path


def digest_member(archive, name):
    digest = hashlib.sha256()
    size = 0
    with archive.open(name) as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            size += len(chunk)
            digest.update(chunk)
    return {'bytes': size, 'sha256': digest.hexdigest()}


def main():
    BUILD.mkdir(parents=True, exist_ok=True)
    run_raw = fetch(f'actions/runs/{RUN}', 'original-run.json')
    jobs_raw = fetch(f'actions/runs/{RUN}/jobs?per_page=100', 'original-jobs.json')
    artifacts_raw = fetch(f'actions/runs/{RUN}/artifacts?per_page=100', 'original-artifacts.json')
    commit_raw = fetch(f'git/commits/{SOURCE}', 'original-source-commit.json')
    run, jobs, artifacts, commit = map(json.loads, (run_raw, jobs_raw, artifacts_raw, commit_raw))
    assert run['head_sha'] == SOURCE and run['status'] == 'completed' and run['conclusion'] == 'success'
    assert run['run_attempt'] == 1
    assert len(jobs['jobs']) == jobs['total_count']
    assert len(artifacts['artifacts']) == artifacts['total_count']
    assert {job['name']: job['conclusion'] for job in jobs['jobs']} == {'installed-hal': 'success', 'installed-runtime-capture': 'success'}
    assert commit['sha'] == SOURCE
    source_tree = commit['tree']['sha']
    tree_raw = fetch(f'git/trees/{source_tree}?recursive=1', 'original-source-tree.json')
    tree = json.loads(tree_raw)
    assert tree['sha'] == source_tree and not tree['truncated']
    entries = {item['path']: item for item in tree['tree']}
    sources = []
    for relative in ('.github/workflows/rust-hal.yml',
                     'drivers/macos/tests/analyze_system.py',
                     'drivers/macos/tests/analyze_runtime.py',
                     'drivers/macos/tests/analyze_diagnostics.py',
                     'drivers/macos/tests/run_runtime.py',
                     'drivers/macos/tests/run_system.py',
                     'drivers/macos/tests/system-probe.c',
                     'crates/relink-audio/examples/runtime-capture-probe.rs'):
        entry = entries[relative]
        assert entry['type'] == 'blob'
        body = fetch(f'git/blobs/{entry["sha"]}', 'original-source-blob-' + entry['sha'] + '.json')
        blob = json.loads(body)
        assert blob['sha'] == entry['sha'] and blob['encoding'] == 'base64'
        data = base64.b64decode(blob['content'])
        assert blob['size'] == len(data)
        assert hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest() == entry['sha']
        save(OUT / 'pinned-source' / relative, data)
        sources.append({'path': relative, 'git_blob_sha': entry['sha'], 'bytes': len(data), 'sha256': sha(data),
                        'original_blob_api_sha256': sha(body)})
    write(OUT / 'pinned-source-identities.json', {'source_revision': SOURCE, 'source_tree': source_tree,
                                              'original_source_tree_api_sha256': sha(tree_raw), 'files': sources})
    metadata = {key: run[key] for key in ('id', 'head_sha', 'head_branch', 'event', 'status', 'conclusion', 'run_attempt', 'workflow_id', 'html_url', 'created_at', 'updated_at')}
    metadata.update(metadata_projection=True, source_tree=source_tree, note='Selected fields from original API bodies; original bytes remain unchanged under the local build collection. Repository URL reflects the rename at collection time.')
    metadata['original_api'] = {name: {'bytes': len(raw), 'sha256': sha(raw)} for name, raw in [('original-run.json', run_raw), ('original-jobs.json', jobs_raw), ('original-artifacts.json', artifacts_raw), ('original-source-commit.json', commit_raw)]}
    metadata['jobs'] = [{**{key: job[key] for key in ('id', 'name', 'status', 'conclusion', 'started_at', 'completed_at', 'html_url')}, 'steps': job['steps']} for job in jobs['jobs']]
    write(OUT / 'run-metadata.json', metadata)
    print('Preserved completed success run metadata; downloading original archives.', flush=True)
    archives = []
    logs = fetch(f'actions/runs/{RUN}/logs', 'original-workflow-logs.zip')
    with zipfile.ZipFile(BUILD / 'original-workflow-logs.zip') as archive:
        assert archive.testzip() is None
    archives.append({'kind': 'workflow-logs', 'archive': 'original-workflow-logs.zip', 'bytes': len(logs), 'sha256': sha(logs), 'github_digest': None, 'github_digest_matches': None, 'zip_crc_verified': True})
    for artifact in artifacts['artifacts']:
        kind = 'runtime' if 'runtime' in artifact['name'] else 'hal'
        assert not any(item['kind'] == kind for item in archives), 'duplicate artifact kind'
        archive_name = f'original-{kind}.zip'
        raw = fetch(f'actions/artifacts/{artifact["id"]}/zip', archive_name)
        item = {'kind': kind, 'archive': archive_name, 'artifact_id': artifact['id'], 'artifact_name': artifact['name'], 'bytes': len(raw), 'sha256': sha(raw), 'github_digest': artifact.get('digest'), 'github_digest_matches': artifact.get('digest') == f'sha256:{sha(raw)}'}
        assert item['github_digest_matches'], item
        assert artifact['workflow_run']['head_sha'] == SOURCE
        with zipfile.ZipFile(BUILD / archive_name) as archive:
            assert archive.testzip() is None
            item['zip_crc_verified'] = True
            names = [entry.filename for entry in archive.infolist() if not entry.is_dir()]
            assert len(names) == len(set(names))
            manifests = [name for name in names if name.endswith('custody-manifest.json')]
            assert len(manifests) == 1
            manifest_name = manifests[0]
            prefix = str(PurePosixPath(manifest_name).parent)
            prefix = '' if prefix == '.' else prefix + '/'
            manifest = json.loads(archive.read(manifest_name))
            assert manifest['source_revision'] == SOURCE
            assert int(manifest['run_id']) == RUN and int(manifest['run_attempt']) == 1
            expected_names = {manifest_name}
            for row in manifest['files']:
                safe(row['path'])
                name = prefix + row['path']
                expected_names.add(name)
                assert digest_member(archive, name) == {'bytes': row['bytes'], 'sha256': row['sha256']}, name
            assert expected_names == set(names), 'Unexpected/missing members relative to original custody manifest'
            item['manifest_file_count'] = len(manifest['files'])
            item['member_count'] = len(names)
            item['all_manifest_members_verified'] = True
            selected = []
            for name in names:
                safe(name)
                info = archive.getinfo(name)
                if info.file_size <= 512 * 1024 and PurePosixPath(name).suffix in ('.json', '.jsonl', '.log', '.txt'):
                    relative = name[len(prefix):]
                    save(OUT / f'{kind}-original-text' / relative, archive.read(name))
                    selected.append(relative)
            item['compact_original_text_members'] = selected
        archives.append(item)
        print(f'Verified {kind}: {len(raw)} bytes, {item["manifest_file_count"]} original manifest files.', flush=True)
    collection = {'schema': 1, 'run_id': RUN, 'run_attempt': 1, 'source_revision': SOURCE, 'source_tree': source_tree, 'repository_at_collection': REPO, 'overall_conclusion': 'PASS', 'job_conclusions': {job['name']: job['conclusion'] for job in jobs['jobs']}, 'local_original_collection': str(BUILD.relative_to(ROOT)), 'archives': archives, 'no_rerun_no_publication': True, 'collector_sha256': sha(Path(__file__).read_bytes())}
    write(OUT / 'collection.json', collection)
    local_files = {path.relative_to(BUILD).as_posix(): {'bytes': path.stat().st_size, 'sha256': sha(path.read_bytes())} for path in sorted(BUILD.rglob('*')) if path.is_file()}
    write(OUT / 'original-local-files.json', local_files)
    stamp = OUT / 'collected-at.json'
    if not stamp.exists():
        write(stamp, {'collected_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat()})


if __name__ == '__main__':
    main()
