import pathlib,json,hashlib,urllib.request,concurrent.futures,datetime
build=pathlib.Path('build/relink-preview-40254943747');ev=pathlib.Path('research/evidence/relink-preview-40254943747');out=build/'public-download';out.mkdir(exist_ok=True)
def fetch(url):
 request=urllib.request.Request(url,headers={'User-Agent':'ReLink-public-byte-verifier'})
 with urllib.request.urlopen(request,timeout=60) as r:return r.read()
raw=fetch('https://api.github.com/repos/thatsmethen/relink/releases/tags/v0.1.0-preview.20261026-relink');release=json.loads(raw);assert not release['draft'] and release['prerelease'];assert len(release['assets'])==8;(ev/'release.json').write_bytes(raw)
tagraw=fetch('https://api.github.com/repos/thatsmethen/relink/git/ref/tags/v0.1.0-preview.20261026-relink');tag=json.loads(tagraw);assert tag['object']['type']=='commit' and tag['object']['sha']=='902118a60b070bb9cefdef75d5b477b5350b9847';(ev/'release-tag.json').write_bytes(tagraw)
def check(a):
 data=fetch(a['browser_download_url']);expected=(build/a['name']).read_bytes();assert data==expected and len(data)==a['size'] and a['digest']=='sha256:'+hashlib.sha256(data).hexdigest();(out/a['name']).write_bytes(data);print(a['name'],len(data),flush=True);return {'name':a['name'],'url':a['browser_download_url'],'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest(),'github_digest':a['digest'],'matches_original':True}
with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:assets=list(pool.map(check,release['assets']))
report={'verified_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'authorization_header':False,'cookies':False,'gh_credentials_read':False,'release':release['html_url'],'source_revision':tag['object']['sha'],'assets':assets,'pass':True};(ev/'public-download-verification.json').write_text(json.dumps(report,indent=2)+'\n');print('All8publicassets exact.',flush=True)
