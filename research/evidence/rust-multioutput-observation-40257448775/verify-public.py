#!/usr/bin/env python3
"""Anonymous public byte roundtrip. No credentials, audio, Git or installation."""
import argparse,datetime,hashlib,json,os,subprocess,sys,time,urllib.error,urllib.request
from pathlib import Path
HERE=Path(__file__).resolve().parent
REPO='thatsmethen/relink';TAG='evidence-multioutput-native-40257448775';RELEASE=423817146
SOURCE='69f49da28a9bae16dec9d4bc29bd0d639fb73a08';TREE='e761d228a3f0998b9addc97c11a4fd3774b90545'
def digest(data):return {'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
def once(path,data):
    if path.exists():assert path.read_bytes()==data,'Existing bytes differ: '+str(path)
    else:
        with path.open('xb') as stream:stream.write(data)
def fetch(url):
    opener=urllib.request.build_opener()
    request=urllib.request.Request(url,headers={'User-Agent':'ReLink-public-evidence-verifier/3','Accept':'application/vnd.github+json' if url.startswith('https://api.github.com/') else '*/*'})
    assert not request.has_header('Authorization') and not request.has_header('Cookie')
    for attempt in range(3):
        try:
            with opener.open(request,timeout=45) as response:
                assert response.status==200
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code not in (408,429,500,502,503,504) or attempt==2:raise
        except (TimeoutError,urllib.error.URLError):
            if attempt==2:raise
        time.sleep(attempt+1)
    raise AssertionError('Unreachable request state')
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--downloads',type=Path,required=True);args=parser.parse_args()
    out=args.downloads.resolve();out.mkdir(parents=True,exist_ok=True)
    started=datetime.datetime.now(datetime.timezone.utc).isoformat()
    api='https://api.github.com/repos/'+REPO
    raw={name:fetch(url) for name,url in [('public-release-api.json',api+'/releases/tags/'+TAG),('public-tag-api.json',api+'/git/ref/tags/'+TAG),('public-source-commit-api.json',api+'/git/commits/'+SOURCE)]}
    for name,data in raw.items():once(out/name,data)
    release=json.loads(raw['public-release-api.json']);tag=json.loads(raw['public-tag-api.json']);commit=json.loads(raw['public-source-commit-api.json'])
    assert release['id']==RELEASE and release['tag_name']==TAG and release['prerelease'] and not release['draft']
    assert release['body']==(HERE/'release-notes.md').read_text()
    assert tag['object']['type']=='commit' and tag['object']['sha']==SOURCE
    assert commit['sha']==SOURCE and commit['tree']['sha']==TREE
    for role in ('author','committer'):assert commit[role]['name']=='ReLink Contributors' and commit[role]['email']=='contributors@relink.invalid'
    manifest=json.loads((HERE/'publication-assets.json').read_text())
    expected={a['name']:a for a in manifest['assets']}
    expected['publication-assets.json']={'name':'publication-assets.json',**digest((HERE/'publication-assets.json').read_bytes())}
    assert len(release['assets'])==len(expected) and {a['name'] for a in release['assets']}==set(expected)
    verified=[]
    for asset in release['assets']:
        name=asset['name'];assert Path(name).name==name
        assert asset['browser_download_url']=='https://github.com/'+REPO+'/releases/download/'+TAG+'/'+name
        data=fetch(asset['browser_download_url']);actual=digest(data)
        assert actual=={k:expected[name][k] for k in ('bytes','sha256')},name
        assert asset['state']=='uploaded' and asset['size']==actual['bytes'] and asset.get('digest')=='sha256:'+actual['sha256']
        once(out/name,data)
        verified.append({'name':name,'asset_id':asset['id'],'url':asset['browser_download_url'],**actual,'github_digest_matches':True})
        print('Anonymous download verified: '+name,file=sys.stderr,flush=True)
    result=subprocess.run([sys.executable,'-I','-B',str(out/'verify-release.py'),'--downloads',str(out)],check=True,capture_output=True,text=True,encoding='utf-8',errors='strict',env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1'))
    offline=json.loads(result.stdout);once(out/'public-offline-verification.json',result.stdout.encode())
    report={'schema':1,'kind':'later publication metadata and anonymous byte verification; not original collection metadata','verification_started_at_utc':started,'verified_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'anonymous_requests':{'authorization_header':False,'cookie_header':False,'gh_credentials_read':False},
        'release':{k:release[k] for k in ('id','html_url','tag_name','name','draft','prerelease','published_at')},
        'tag':{'type':'commit','revision':SOURCE,'tagger_metadata':'none: lightweight tag'},'source_tree':TREE,'source_author_and_committer_are_anonymous_contributors':True,
        'assets':verified,'raw_public_api_responses_preserved_locally':{name:digest(data) for name,data in raw.items()},
        'offline_verification_of_anonymously_downloaded_assets':offline,'release_notes_match_local_utf8':True,'original_collection_mutated':False,'audio_rerun_or_changed_criteria':False}
    once(out/'public-verification.json',(json.dumps(report,indent=2)+'\n').encode())
    print(json.dumps(report,indent=2))
if __name__=='__main__':main()
