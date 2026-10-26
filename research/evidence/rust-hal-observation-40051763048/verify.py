#!/usr/bin/env python3
"""Offline original ZIP, member and compact evidence verification. No audio/API."""
import argparse
import hashlib
import json
from pathlib import Path
import zipfile
from diagnose import diagnose

HERE = Path(__file__).resolve().parent
DEFAULT = HERE.parents[2] / 'build/rust-hal-failed-40051763048'


def digest(stream):
    h = hashlib.sha256()
    size = 0
    for data in iter(lambda: stream.read(1024 * 1024), b''):
        size += len(data)
        h.update(data)
    return {'bytes': size, 'sha256': h.hexdigest()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--archives', type=Path, default=DEFAULT)
    args = parser.parse_args()
    collection = json.loads((HERE / 'collection.json').read_text(encoding='utf-8'))
    verified = []
    for item in collection['archives']:
        path = args.archives / item['archive']
        with path.open('rb') as stream:
            assert digest(stream) == {key: item[key] for key in ('bytes', 'sha256')}, path
        with zipfile.ZipFile(path) as archive:
            assert archive.testzip() is None
            if item['kind'] != 'workflow-logs':
                manifest = json.loads(archive.read('custody-manifest.json'))
                assert manifest['source_revision'] == collection['source_revision']
                expected_names = {'custody-manifest.json'}
                for member in manifest['files']:
                    expected_names.add(member['path'])
                    with archive.open(member['path']) as stream:
                        assert digest(stream) == {key: member[key] for key in ('bytes', 'sha256')}, member['path']
                assert set(archive.namelist()) == expected_names
                for member in item['compact_original_text_members']:
                    assert (HERE / (item['kind'] + '-original-text') / member).read_bytes() == archive.read(member)
        verified.append(item['archive'])
    expected = json.loads((HERE / 'derived-diagnosis.json').read_text(encoding='utf-8'))
    assert diagnose(args.archives) == expected
    manifest = HERE / 'SHA256SUMS.json'
    if manifest.exists():
        for relative, expected_digest in json.loads(manifest.read_text(encoding='utf-8')).items():
            with (HERE / relative).open('rb') as stream:
                assert digest(stream) == expected_digest, relative
    print(json.dumps({'verified_original_archives': verified, 'all_original_manifest_members_verified': True, 'compact_original_text_bytes_verified': True, 'derived_diagnosis_reproduced': True, 'overall_run_conclusion': 'FAIL', 'native_runtime_job_conclusion': 'PASS', 'no_audio_or_network_operations': True}, indent=2))


if __name__ == '__main__':
    main()
