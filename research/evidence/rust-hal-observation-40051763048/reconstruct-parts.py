#!/usr/bin/env python3
"""Reconstruct original archives and verify preserved evidence, entirely offline."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import zipfile


def digest(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def checked_name(name):
    assert isinstance(name, str) and Path(name).name == name and name not in ('', '.', '..')
    return name


def checked_bytes(path, record):
    data = path.read_bytes()
    assert digest(data) == {key: record[key] for key in ('bytes', 'sha256')}, path.name
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
    args = parser.parse_args()
    folder = args.downloads.resolve()
    manifest = json.loads((folder / 'publication-parts.json').read_text(encoding='utf-8'))
    assert manifest['source_revision'] == '91d73e4cf0ecf820d93a68f73d59f1cb09d00b13'
    assert manifest['tag'] == 'evidence-native-pcm-40051763048'
    assert manifest['outcomes'] == {'overall': 'FAIL', 'installed-hal': 'FAIL', 'installed-runtime-capture': 'PASS'}
    prior = manifest['historical_manifest']
    historical = json.loads(checked_bytes(folder / checked_name(prior['name']), prior))
    original_records = {item['name']: item for item in historical['assets']}
    assert len(original_records) == len(manifest['original_archives']) == 4
    assert {entry['name'] for entry in manifest['original_archives']} == set(original_records)
    asset_records = {item['name']: item for item in manifest['assets']}
    assert len(asset_records) == len(manifest['assets'])
    for item in asset_records.values():
        checked_bytes(folder / checked_name(item['name']), item)

    reconstructed = []
    used_assets = set()
    for archive in manifest['original_archives']:
        name = checked_name(archive['name'])
        assert {key: archive[key] for key in ('bytes', 'sha256')} == {
            key: original_records[name][key] for key in ('bytes', 'sha256')}
        if archive['parts']:
            chunks = []
            offset = 0
            for index, part in enumerate(archive['parts']):
                assert part['index'] == index and part['offset'] == offset
                part_name = checked_name(part['name'])
                assert part_name == f'{name}.part{index:03}'
                assert part_name not in used_assets
                assert {key: part[key] for key in ('bytes', 'sha256')} == {
                    key: asset_records[part_name][key] for key in ('bytes', 'sha256')}
                data = checked_bytes(folder / part_name, part)
                assert 0 < len(data) <= manifest['part_bytes'] == 524288
                if index + 1 < len(archive['parts']):
                    assert len(data) == manifest['part_bytes']
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
        reconstructed.append({'name': name, **digest((folder / name).read_bytes())})
    assert used_assets == set(asset_records)

    # Only the compact collection is extracted. Original PCM and CSV stay compressed.
    compact = folder / 'collection-evidence'
    compact.mkdir(exist_ok=True)
    with zipfile.ZipFile(folder / 'collection-evidence-40051763048.zip') as archive:
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
    result = subprocess.run(
        [sys.executable, str(compact / 'verify.py'), '--archives', str(folder)],
        check=True, capture_output=True, text=True, encoding='utf-8', errors='strict',
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'))
    report = {
        'schema': 1,
        'all_parts_verified': True,
        'all_reconstructed_original_archive_hashes_match': True,
        'archives': reconstructed,
        'bundled_original_offline_verifier': json.loads(result.stdout),
        'bulk_pcm_extracted': False,
        'network_or_audio_operations': False,
        'original_outcomes_changed': False,
    }
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
