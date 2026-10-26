#!/usr/bin/env python3
"""Preserve exact original CI archives and API records; no dispatch or target execution."""
import argparse, datetime, hashlib, json, pathlib, subprocess, urllib.request, zipfile

REVISION = '69f49da28a9bae16dec9d4bc29bd0d639fb73a08'
REPOSITORY = 'thatsmethen/relink'
EVIDENCE = pathlib.Path(__file__).resolve().parent
ROOT = EVIDENCE.parents[2]
BUILD = ROOT / 'build' / EVIDENCE.name
RUNS = {'driver': 40257398430, 'desktop': 40257398458}

def sha(data): return hashlib.sha256(data).hexdigest()
def api(path): return subprocess.check_output(['gh', 'api', f'repos/{REPOSITORY}/{path}'])
def keep(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        if path.read_bytes() != data: raise RuntimeError(f'refusing to replace original {path}')
    else:
        with path.open('xb') as output: output.write(data)
def dump(path, data): keep(path, (json.dumps(data, indent=2) + '\n').encode())
def archive(path, data):
    keep(path, data)
    with zipfile.ZipFile(path) as z:
        assert z.testzip() is None
        names=z.namelist(); assert len(names)==len(set(names))
        for n in names:
            assert not pathlib.PurePosixPath(n).is_absolute() and '..' not in pathlib.PurePosixPath(n).parts
        return {n:{'bytes':len(z.read(n)), 'sha256':sha(z.read(n))} for n in names if not n.endswith('/')}

def collect_run(which):
    run_id=RUNS[which]; raw=api(f'actions/runs/{run_id}'); run=json.loads(raw)
    assert run['head_sha']==REVISION and run['run_attempt']==1
    if run['status']!='completed':
        stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        keep(BUILD/which/f'run-pending-{stamp}.json',raw)
        print(json.dumps({'run':run_id,'status':run['status'],'pending':True}));return
    destination=BUILD/which
    keep(destination/'run.json',raw)
    jobs=api(f'actions/runs/{run_id}/attempts/1/jobs?per_page=100');keep(destination/'jobs.json',jobs)
    arts=api(f'actions/runs/{run_id}/artifacts?per_page=100');keep(destination/'artifacts.json',arts)
    manifest={}
    for item in json.loads(arts)['artifacts']:
        assert not item['expired'] and item['workflow_run']['head_sha']==REVISION
        name=item['name']+'.zip';path=destination/name
        data=path.read_bytes() if path.exists() else api(f'actions/artifacts/{item["id"]}/zip')
        digest='sha256:'+sha(data);assert item['digest']==digest,(name,item['digest'],digest)
        manifest[name]={'artifact_id':item['id'],'bytes':len(data),'sha256':sha(data),'github_digest':item['digest'],'members':archive(path,data)}
    logs=destination/'original-logs.zip'
    data=logs.read_bytes() if logs.exists() else api(f'actions/runs/{run_id}/attempts/1/logs')
    manifest[logs.name]={'bytes':len(data),'sha256':sha(data),'members':archive(logs,data)}
    dump(destination/'original-archive-manifest.json',manifest)
    for n in ['run.json','jobs.json','artifacts.json','original-archive-manifest.json']:
        keep(EVIDENCE/which/n,(destination/n).read_bytes())
    print(json.dumps({'run':run_id,'status':run['status'],'conclusion':run['conclusion'],'archives':len(manifest)}))

def collect_source():
    commit_raw=api(f'git/commits/{REVISION}');commit=json.loads(commit_raw);assert commit['sha']==REVISION
    tree_raw=api(f'git/trees/{commit["tree"]["sha"]}?recursive=1');tree=json.loads(tree_raw);assert not tree['truncated']
    keep(BUILD/'source-commit.json',commit_raw);keep(BUILD/'source-tree.json',tree_raw)
    keep(EVIDENCE/'source-commit.json',commit_raw)
    paths=['.github/workflows/rust.yml','.github/workflows/rust-windows-driver.yml',
      'scripts/build-macos-app.sh','scripts/verify-windows-icon.py', 'packaging/macos/Info.plist',
      'packaging/branding/relink.icns','packaging/branding/relink.ico','packaging/branding/relink.res',
      'drivers/windows/ReLink.inf','drivers/windows/build.ps1','drivers/windows/runtime-notices.py',
      'drivers/windows/tests/check-pe.py','drivers/windows/toolchain.json','drivers/windows/PROVENANCE.md',
      'LICENSE','THIRD-PARTY-NOTICES.txt']
    paths += [x['path'] for x in tree['tree'] if x['type']=='blob' and x['path'].startswith('docs/third-party-licenses/rust-1.91.1/')]
    index={x['path']:x for x in tree['tree']};manifest={}
    for name in paths:
        path=BUILD/'source'/name
        if path.exists(): data=path.read_bytes()
        else:
            req=urllib.request.Request(f'https://raw.githubusercontent.com/{REPOSITORY}/{REVISION}/{name}',headers={'User-Agent':'ReLink-CI-evidence'})
            with urllib.request.urlopen(req,timeout=60) as response: data=response.read()
        blob=hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()
        assert blob==index[name]['sha'],name
        keep(path,data);manifest[name]={'bytes':len(data),'sha256':sha(data),'git_blob_sha1':blob}
    dump(EVIDENCE/'source-file-manifest.json',manifest)
    for name in ['.github/workflows/rust.yml','.github/workflows/rust-windows-driver.yml']:
        keep(EVIDENCE/'source-workflows'/pathlib.Path(name).name,(BUILD/'source'/name).read_bytes())
    print(json.dumps({'source_revision':REVISION,'source_files':len(manifest)}))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('kind',choices=['driver','desktop','source']);args=parser.parse_args()
    collect_source() if args.kind=='source' else collect_run(args.kind)
