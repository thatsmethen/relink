#!/usr/bin/env python3
"""Offline release custody and original analysis using an explicit local Git source.

This wrapper does not modify the bundled original verifier or its criteria.
It reconstructs lossless archive parts when present, then supplies the exact
local Git repository that the unchanged verifier uses for its frozen analyzers.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import zipfile

SOURCE = 'f47df76d100da25c5e98d261d92792a1d507d5f1'
TREE = '92fe31337e7f59f40d25cde9c84a2c125da38d16'


def digest(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def checked_name(name):
    assert isinstance(name, str) and Path(name).name == name and name not in ('', '.', '..')
    return name


def checked_bytes(path, expected):
    data = path.read_bytes()
    assert digest(data) == {key: expected[key] for key in ('bytes', 'sha256')}, path.name
    return data


def write_once(path, data):
    if path.exists():
        assert path.read_bytes() == data, f'refusing to replace different bytes: {path}'
    else:
        with path.open('xb') as stream:
            stream.write(data)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--downloads', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--source-repo', type=Path, required=True,
                        help='Local Git clone containing exact source f47df76; no checkout or network fetch is performed')
    args = parser.parse_args()
    folder = args.downloads.resolve()
    source_repo = args.source_repo.resolve()
    gitdir = subprocess.check_output(['git', '-C', str(source_repo), 'rev-parse', '--absolute-git-dir'], text=True).strip()
    source_tree = subprocess.check_output(['git', '--git-dir', gitdir, 'show', '-s', '--format=%T', SOURCE], text=True).strip()
    assert source_tree == TREE
    manifest = json.loads((folder / 'publication-assets.json').read_text(encoding='utf-8'))
    assert manifest['source_revision'] == SOURCE and manifest['source_tree'] == TREE
    assert manifest['tag'] == 'evidence-native-output-40249452671'
    assets = {item['name']: item for item in manifest['assets']}
    assert len(assets) == len(manifest['assets'])
    for item in assets.values():
        checked_bytes(folder / checked_name(item['name']), item)

    used_assets = set()
    archives = []
    for archive in manifest['archives']:
        name = checked_name(archive['name'])
        if archive['parts']:
            chunks = []
            offset = 0
            for index, part in enumerate(archive['parts']):
                assert part['index'] == index and part['offset'] == offset
                part_name = checked_name(part['name'])
                assert part_name == f'{name}.part{index:03}' and part_name not in used_assets
                data = checked_bytes(folder / part_name, part)
                assert digest(data) == {key: assets[part_name][key] for key in ('bytes', 'sha256')}
                assert 0 < len(data) <= 524288
                if index + 1 < len(archive['parts']):
                    assert len(data) == 524288
                chunks.append(data)
                offset += len(data)
                used_assets.add(part_name)
            data = b''.join(chunks)
            assert digest(data) == {key: archive[key] for key in ('bytes', 'sha256')}
            write_once(folder / name, data)
        else:
            assert name not in used_assets
            checked_bytes(folder / name, archive)
            used_assets.add(name)
        with zipfile.ZipFile(folder / name) as contents:
            assert contents.testzip() is None
        archives.append({'name': name, **digest((folder / name).read_bytes())})
    assert used_assets == set(assets)

    compact = folder / 'collection-evidence'
    compact.mkdir(exist_ok=True)
    with zipfile.ZipFile(folder / 'collection-evidence-40249452671.zip') as archive:
        hashes = json.loads(archive.read('SHA256SUMS.json'))
        assert set(archive.namelist()) == {*hashes, 'SHA256SUMS.json'}
        assert len(set(archive.namelist())) == len(archive.namelist())
        for name in archive.namelist():
            relative = PurePosixPath(name)
            assert not relative.is_absolute() and '..' not in relative.parts
            data = archive.read(name)
            if name in hashes:
                assert digest(data) == hashes[name], name
            destination = compact / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            write_once(destination, data)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', GIT_DIR=gitdir)
    # git show reads the exact pinned objects. The user's checkout is not changed.
    env.pop('GIT_WORK_TREE', None)
    result = subprocess.run(
        [sys.executable, str(compact / 'verify.py'), '--archives', str(folder)],
        check=True, capture_output=True, text=True, encoding='utf-8', errors='strict', env=env)
    report = {
        'schema': 1,
        'all_release_payload_hashes_verified': True,
        'complete_original_archive_hashes_verified': True,
        'archives': archives,
        'local_source_revision': SOURCE,
        'local_source_tree': source_tree,
        'unchanged_bundled_verifier': json.loads(result.stdout),
        'original_analyzer_sha256': json.loads((compact / 'derived-verification.json').read_text())['analyzer_sources'],
        'original_verifier_modified': False,
        'original_outcomes_changed': False,
        'network_or_audio_operations': False,
        'temporary_pcm_extraction': 'The original verifier temporarily extracts original PCM/clock/control inputs for its offline recomputation, then removes its temporary directory.',
    }
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
