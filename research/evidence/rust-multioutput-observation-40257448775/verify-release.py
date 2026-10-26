#!/usr/bin/env python3
"""Offline byte verification and exact recomputation of one frozen observation.

No network, Git, devices, native audio, installation or acquisition is performed.
Requires only Python 3.10+ standard library. All temporary files are removed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile
import zipfile

RUN = 40257448775
SOURCE = '69f49da28a9bae16dec9d4bc29bd0d639fb73a08'
TREE = 'e761d228a3f0998b9addc97c11a4fd3774b90545'
TAG = 'evidence-multioutput-native-40257448775'


def digest(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def check(data, expected, label):
    if digest(data) != {key: expected[key] for key in ('bytes', 'sha256')}:
        raise ValueError(f'Byte identity mismatch: {label}')


def safe_name(name):
    if not isinstance(name, str) or Path(name).name != name or name in ('', '.', '..'):
        raise ValueError(f'Unsafe transport filename: {name!r}')
    return name


def verify(downloads):
    manifest = json.loads((downloads / 'publication-assets.json').read_text())
    if (manifest['run_id'], manifest['source_revision'], manifest['source_tree'], manifest['tag']) != (RUN, SOURCE, TREE, TAG):
        raise ValueError('Wrong evidence identity')
    records = manifest['assets']
    if len({r['name'] for r in records}) != len(records):
        raise ValueError('Repeated asset names')
    for record in records:
        name = safe_name(record['name'])
        check((downloads / name).read_bytes(), record, name)
    with tempfile.TemporaryDirectory(prefix='relink-multioutput-public-') as temporary:
        work = Path(temporary)
        archives = {}
        for record in manifest['archives']:
            name = safe_name(record['name'])
            if name in archives:
                raise ValueError('Repeated original archive')
            if record['parts']:
                parts = record['parts']
                if [p['index'] for p in parts] != list(range(len(parts))):
                    raise ValueError('Parts are not in exact zero-based order')
                if len({p['name'] for p in parts}) != len(parts):
                    raise ValueError('Repeated part name')
                chunks = []
                for part in parts:
                    part_name = safe_name(part['name'])
                    data = (downloads / part_name).read_bytes()
                    check(data, part, part_name)
                    if not 0 < len(data) <= 524288:
                        raise ValueError('Part exceeds fixed 512 KiB bound')
                    chunks.append(data)
                data = b''.join(chunks)
            else:
                data = (downloads / name).read_bytes()
            check(data, record, name)
            path = work / name
            path.write_bytes(data)
            archives[name] = path
        required = {'original-multioutput.zip', 'original-workflow-logs.zip', 'collection-evidence-40257448775.zip'}
        if set(archives) != required:
            raise ValueError('Incomplete original archive set')
        with zipfile.ZipFile(archives['collection-evidence-40257448775.zip']) as snapshot:
            names = snapshot.namelist()
            if len(names) != len(set(names)) or snapshot.testzip() is not None:
                raise ValueError('Corrupt or duplicate snapshot members')
            for name in names:
                path = PurePosixPath(name)
                if path.is_absolute() or '..' in path.parts or len(path.parts) < 2 or path.parts[0] not in ('collection-evidence', 'original-api'):
                    raise ValueError('Unsafe snapshot path')
                info = snapshot.getinfo(name)
                if info.is_dir() or (info.external_attr >> 16) & 0o170000 != 0o100000:
                    raise ValueError('Snapshot member is not an ordinary file')
                target = work.joinpath(*path.parts)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(snapshot.read(name))
        frozen = work / 'collection-evidence'
        original = work / 'original-api'
        hashes = json.loads((frozen / 'SHA256SUMS.json').read_text())
        if {p.relative_to(frozen).as_posix() for p in frozen.rglob('*') if p.is_file()} != set(hashes) | {'SHA256SUMS.json'}:
            raise ValueError('Frozen evidence membership differs')
        for name, identity in hashes.items():
            check((frozen / name).read_bytes(), identity, name)
        for name in ('original-multioutput.zip', 'original-workflow-logs.zip'):
            (original / name).write_bytes(archives[name].read_bytes())
        local = json.loads((frozen / 'original-local-files.json').read_text())
        if {p.name for p in original.iterdir()} != set(local):
            raise ValueError('Original collection membership differs')
        for name, identity in local.items():
            check((original / safe_name(name)).read_bytes(), identity, name)
        result = subprocess.run([sys.executable, '-I', '-B', str(frozen / 'verify.py'), '--archives', str(original)],
            check=True, capture_output=True, text=True, encoding='utf-8', errors='strict',
            env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'))
        computed = json.loads(result.stdout)
        saved = json.loads((frozen / 'derived-verification.json').read_text())
        if computed != saved:
            raise ValueError('Full frozen derived report differs')
        if computed['overall_conclusion'] != 'PASS' or computed['all_original_manifest_members_verified'] != 56 or computed['exact_archive_members'] != 57:
            raise ValueError('Unexpected original result or member counts')
        return {'schema': 1, 'run_id': RUN, 'source_revision': SOURCE, 'source_tree': TREE,
            'tag': TAG, 'transport_assets_verified': len(records),
            'original_archives': [{k:r[k] for k in ('name', 'bytes', 'sha256')} for r in manifest['archives']],
            'snapshot_files_verified': len(names), 'frozen_evidence_files_verified': len(hashes) + 1,
            'original_api_files_verified': len(local) - 2, 'original_manifest_files_verified': 56,
            'original_artifact_exact_archive_members': 57,
            'full_original_analysis_and_two_diagnostics_recomputed_exactly': True,
            'full_original_derived_report_matches': True, 'original_overall_result': 'PASS',
            'temporary_files_removed_on_return': True, 'audio_rerun_or_changed_criteria': False,
            'scope': computed['scope'], 'uncertainty': computed['uncertainty']}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--downloads', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(verify(args.downloads.resolve()), indent=2, sort_keys=True))


if __name__ == '__main__':
    main()
