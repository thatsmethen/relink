#!/usr/bin/env python3
"""Anonymous release download, reconstruction and original member verification."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import urllib.error
import urllib.request

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
        with path.open('xb') as stream:
            stream.write(data)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--downloads', type=Path, required=True)
    args = parser.parse_args()
    out = args.downloads.resolve()
    out.mkdir(parents=True, exist_ok=True)
    opener = urllib.request.build_opener()

    def fetch(url):
        request = urllib.request.Request(url, headers={
            'User-Agent': 'ReLink-public-evidence-verifier/2',
            'Accept': 'application/vnd.github+json' if url.startswith('https://api.github.com/') else '*/*',
        })
        assert not request.has_header('Authorization') and not request.has_header('Cookie')
        for attempt in range(3):
            try:
                with opener.open(request, timeout=45) as response:
                    assert response.status == 200
                    return response.read()
            except urllib.error.HTTPError as error:
                if error.code not in (408, 429, 500, 502, 503, 504) or attempt == 2:
                    raise
                print(f'Anonymous HTTP {error.code}; bounded retry {attempt + 1}/2', file=sys.stderr)
            except (TimeoutError, urllib.error.URLError):
                if attempt == 2:
                    raise
                print(f'Anonymous request interrupted; bounded retry {attempt + 1}/2', file=sys.stderr)
            time.sleep(attempt + 1)
        raise AssertionError('unreachable request state')

    fetched = datetime.now(timezone.utc).isoformat()
    api = f'https://api.github.com/repos/{REPO}'
    release_bytes = fetch(f'{api}/releases/tags/{TAG}')
    ref_bytes = fetch(f'{api}/git/ref/tags/{TAG}')
    commit_bytes = fetch(f'{api}/git/commits/{REVISION}')
    raw_api = {'public-release-api.json': release_bytes,
               'public-tag-api.json': ref_bytes,
               'public-source-commit-api.json': commit_bytes}
    for name, data in raw_api.items():
        write_once(out / name, data)
    release, ref, commit = map(json.loads, (release_bytes, ref_bytes, commit_bytes))
    assert release['id'] == 422528575
    assert release['tag_name'] == TAG and release['prerelease'] and not release['draft']
    assert release['body'] == (HERE / 'release-notes-parts.md').read_text(encoding='utf-8')
    assert ref['object']['type'] == 'commit' and ref['object']['sha'] == REVISION
    assert commit['sha'] == REVISION and commit['tree']['sha'] == TREE
    for role in ('author', 'committer'):
        assert commit[role]['name'] == 'RaveLink Contributors'
        assert commit[role]['email'] == 'contributors@ravelink.invalid'

    expected = json.loads((HERE / 'publication-parts.json').read_text(encoding='utf-8'))
    records = {item['name']: item for item in expected['assets']}
    for name in ('publication-assets.json', 'publication-parts.json', 'reconstruct-parts.py'):
        records[name] = {'name': name, **digest((HERE / name).read_bytes())}
    assert len(release['assets']) == len(records) == 40
    assert {asset['name'] for asset in release['assets']} == set(records)
    verified = []
    for asset in release['assets']:
        name = asset['name']
        assert Path(name).name == name
        data = fetch(asset['browser_download_url'])
        actual = digest(data)
        assert actual == {key: records[name][key] for key in ('bytes', 'sha256')}, name
        assert asset['state'] == 'uploaded' and asset['size'] == actual['bytes']
        assert asset.get('digest') == 'sha256:' + actual['sha256']
        write_once(out / name, data)
        verified.append({'name': name, 'asset_id': asset['id'], 'url': asset['browser_download_url'],
                         **actual, 'github_release_digest_matches': True})
        print(f'Anonymous download verified: {name}', file=sys.stderr, flush=True)

    result = subprocess.run(
        [sys.executable, str(out / 'reconstruct-parts.py'), '--downloads', str(out)],
        check=True, capture_output=True, text=True, encoding='utf-8', errors='strict',
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'))
    reconstruction = json.loads(result.stdout)
    write_once(out / 'public-reconstruction-verification.json', result.stdout.encode('utf-8'))
    assert (out / 'collection-evidence/SHA256SUMS.json').read_bytes() == (HERE / 'collection-SHA256SUMS.json').read_bytes()
    report = {
        'schema': 2, 'verification_started_at_utc': fetched,
        'verified_at_utc': datetime.now(timezone.utc).isoformat(),
        'kind': 'metadata projection and independent public byte verification; not an original API body',
        'anonymous_public_requests': {'authorization_header': False, 'cookie_header': False, 'gh_credentials_read': False},
        'release': {key: release[key] for key in ('id', 'html_url', 'tag_name', 'name', 'prerelease', 'draft', 'published_at')},
        'tag': {'type': ref['object']['type'], 'revision': ref['object']['sha'], 'tagger_metadata': 'none: lightweight tag'},
        'source_tree': commit['tree']['sha'],
        'source_author_and_committer_are_anonymous_contributor_identity': True,
        'notes_match_local_utf8': True,
        'assets': verified,
        'raw_public_api_responses_retained_locally': {name: digest(data) for name, data in raw_api.items()},
        'compact_collection_snapshot_manifest_verified': True,
        'reconstruction_and_original_offline_verification': reconstruction,
        'bulk_pcm_extracted': False,
        'rerun_or_original_outcome_changes': False,
        'runtime_output_boundary': expected['runtime_output_boundary'],
    }
    write_once(out / 'public-verification.json', (json.dumps(report, indent=2) + '\n').encode('utf-8'))
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
