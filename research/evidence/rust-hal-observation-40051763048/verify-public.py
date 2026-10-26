#!/usr/bin/env python3
"""Anonymous public custody verification; no audio, workflow or upload actions."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import urllib.request
import zipfile

HERE = Path(__file__).resolve().parent
REPO = 'thatsmethen/relink'
TAG = 'evidence-native-pcm-40051763048'
REVISION = '91d73e4cf0ecf820d93a68f73d59f1cb09d00b13'
TREE = 'f42729259cbade01f60b64b2b5726de83f4c4cf6'


def digest(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def write_once(path, data):
    if path.exists():
        assert path.read_bytes() == data, f'existing bytes differ: {path}'
    else:
        path.write_bytes(data)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--downloads', type=Path, required=True)
    args = parser.parse_args()
    out = args.downloads.resolve()
    out.mkdir(parents=True, exist_ok=True)
    # This opener has no auth or cookie processor and never reads gh credentials.
    opener = urllib.request.build_opener()

    def fetch(url):
        request = urllib.request.Request(url, headers={
            'User-Agent': 'ReLink-public-evidence-verifier/1',
            'Accept': 'application/vnd.github+json' if url.startswith('https://api.github.com/') else '*/*',
        })
        assert not request.has_header('Authorization')
        assert not request.has_header('Cookie')
        with opener.open(request, timeout=45) as response:
            assert response.status == 200
            return response.read()

    fetched = datetime.now(timezone.utc).isoformat()
    api = f'https://api.github.com/repos/{REPO}'
    release_bytes = fetch(f'{api}/releases/tags/{TAG}')
    ref_bytes = fetch(f'{api}/git/ref/tags/{TAG}')
    commit_bytes = fetch(f'{api}/git/commits/{REVISION}')
    for name, data in [('public-release-api.json', release_bytes),
                       ('public-tag-api.json', ref_bytes),
                       ('public-source-commit-api.json', commit_bytes)]:
        write_once(out / name, data)
    release, ref, commit = map(json.loads, (release_bytes, ref_bytes, commit_bytes))
    assert release['tag_name'] == TAG and release['prerelease'] and not release['draft']
    assert release['body'] == (HERE / 'release-notes.md').read_text(encoding='utf-8')
    assert ref['object']['type'] == 'commit' and ref['object']['sha'] == REVISION
    assert commit['sha'] == REVISION and commit['tree']['sha'] == TREE
    for role in ('author', 'committer'):
        assert commit[role]['name'] == 'RaveLink Contributors'
        assert commit[role]['email'] == 'contributors@ravelink.invalid'

    expected = json.loads((HERE / 'publication-assets.json').read_text(encoding='utf-8'))
    asset_records = {item['name']: item for item in expected['assets']}
    asset_records['publication-assets.json'] = {'name': 'publication-assets.json', **digest((HERE / 'publication-assets.json').read_bytes())}
    assert {asset['name'] for asset in release['assets']} == set(asset_records)
    verified = []
    for asset in release['assets']:
        name = asset['name']
        assert Path(name).name == name
        data = fetch(asset['browser_download_url'])
        actual = digest(data)
        assert actual == {key: asset_records[name][key] for key in ('bytes', 'sha256')}, name
        assert asset['size'] == actual['bytes']
        assert asset.get('digest') == 'sha256:' + actual['sha256']
        write_once(out / name, data)
        verified.append({'name': name, 'asset_id': asset['id'], 'url': asset['browser_download_url'],
                         **actual, 'github_release_digest_matches': True})

    # Only the compact evidence snapshot is extracted; PCM/CSV stay in original ZIPs.
    compact = out / 'collection-evidence'
    compact.mkdir(exist_ok=True)
    with zipfile.ZipFile(out / 'collection-evidence-40051763048.zip') as archive:
        assert archive.testzip() is None
        manifest = json.loads(archive.read('SHA256SUMS.json'))
        assert set(archive.namelist()) == {*manifest, 'SHA256SUMS.json'}
        assert archive.read('SHA256SUMS.json') == (HERE / 'collection-SHA256SUMS.json').read_bytes()
        for name in archive.namelist():
            relative = PurePosixPath(name)
            assert not relative.is_absolute() and '..' not in relative.parts
            data = archive.read(name)
            if name in manifest:
                assert digest(data) == manifest[name], name
            destination = compact / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            write_once(destination, data)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    result = subprocess.run([sys.executable, str(compact / 'verify.py'), '--archives', str(out)],
                            check=True, capture_output=True, text=True, encoding='utf-8', errors='strict', env=env)
    offline = json.loads(result.stdout)
    write_once(out / 'public-bundled-offline-verification.json', result.stdout.encode('utf-8'))
    report = {
        'schema': 1, 'verified_at_utc': fetched,
        'kind': 'metadata projection and independent byte verification; not an original API body',
        'anonymous_public_requests': {'authorization_header': False, 'cookie_header': False, 'gh_credentials_read': False},
        'release': {key: release[key] for key in ('id', 'html_url', 'tag_name', 'name', 'prerelease', 'draft', 'published_at')},
        'tag': {'type': ref['object']['type'], 'revision': ref['object']['sha'], 'tagger_metadata': 'none: lightweight tag'},
        'source_tree': commit['tree']['sha'],
        'source_author_and_committer_are_anonymous_contributor_identity': True,
        'notes_match_local_utf8': True,
        'assets': verified,
        'raw_public_api_responses_retained_locally': {
            'public-release-api.json': digest(release_bytes),
            'public-tag-api.json': digest(ref_bytes),
            'public-source-commit-api.json': digest(commit_bytes),
        },
        'compact_collection_snapshot_manifest_verified': True,
        'bundled_offline_verifier': offline,
        'bulk_pcm_extracted': False,
        'rerun_or_original_outcome_changes': False,
        'runtime_output_boundary': expected['runtime_output_boundary'],
    }
    write_once(out / 'public-verification.json', (json.dumps(report, indent=2) + '\n').encode('utf-8'))
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
