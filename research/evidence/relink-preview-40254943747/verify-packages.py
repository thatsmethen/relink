from pathlib import Path
import hashlib,json,subprocess,plistlib,stat,struct,datetime,importlib.util,shutil
root=Path.cwd();build=root/'build/relink-preview-40254943747';ev=root/'research/evidence/relink-preview-40254943747';rev='902118a60b070bb9cefdef75d5b477b5350b9847'
def gitbytes(p):return subprocess.check_output(['git','show',f'{rev}:{p}'])
def sha(b):return hashlib.sha256(b).hexdigest()
run=json.loads((build/'run.json').read_text());jobs=json.loads((build/'jobs.json').read_text())
assert run['status']=='completed' and run['conclusion']=='success' and run['head_sha']==rev
assert len(jobs['jobs'])==2 and all(j['conclusion']=='success' for j in jobs['jobs'])
report=json.loads((build/'windows/icon-resource-verification.json').read_bytes());exe=(build/'windows/ReLink.exe').read_bytes();assert sha(exe)==report['executable_sha256']
for field,path in [('ico_sha256','packaging/branding/ravelink.ico'),('res_sha256','packaging/branding/ravelink.res'),('script_sha256','scripts/verify-windows-icon.py')]:assert sha(gitbytes(path))==report[field]
spec=importlib.util.spec_from_file_location('icons',root/'scripts/verify-windows-icon.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
resources=module.expected_resources(gitbytes('packaging/branding/ravelink.ico'));module.verify_equal(resources,module.res_resources(gitbytes('packaging/branding/ravelink.res')));assert len(report['resources'])==len(resources)==8
for r in report['resources']:
 data=resources[r['type'],r['id'],r['language']];assert r['sha256']==sha(data) and r['size']==len(data)
pe=struct.unpack_from('<I',exe,0x3c)[0];assert exe[pe:pe+4]==b'PE\0\0';assert struct.unpack_from('<H',exe,pe+4)[0]==0x8664;opt=pe+24;assert struct.unpack_from('<H',exe,opt)[0]==0x20b
certificate=struct.unpack_from('<II',exe,opt+112+4*8);assert certificate==(0,0)
app=build/'macos/ReLink.app';contents=app/'Contents';plist=plistlib.loads((contents/'Info.plist').read_bytes());assert plist['CFBundleName']==plist['CFBundleDisplayName']=='ReLink';assert plist['CFBundleExecutable']=='ravelink-app' and (contents/'MacOS/ravelink-app').stat().st_mode&stat.S_IXUSR
arch=subprocess.check_output(['lipo','-archs',str(contents/'MacOS/ravelink-app')],text=True).strip();assert arch=='arm64'
for filepath,gitpath in [(contents/'Resources/RaveLink.icns','packaging/branding/ravelink.icns'),(contents/'Resources/ReLink-LICENSE.txt','LICENSE'),(contents/'Resources/THIRD-PARTY-NOTICES.txt','THIRD-PARTY-NOTICES.txt'),(build/'windows/LICENSE','LICENSE'),(build/'windows/THIRD-PARTY-NOTICES.txt','THIRD-PARTY-NOTICES.txt')]:assert filepath.read_bytes()==gitbytes(gitpath),filepath
for directory in [contents/'Resources/Rust-runtime-notices',build/'windows/Rust-runtime-notices']:
 for p in directory.iterdir():assert p.read_bytes()==gitbytes('docs/third-party-licenses/rust-1.91.1/'+p.name)
codesign=subprocess.run(['codesign','--verify','--deep','--strict','--verbose=2',str(app)],capture_output=True,text=True);assert codesign.returncode==0
signinfo=subprocess.run(['codesign','-dvv',str(app)],capture_output=True,text=True);assert signinfo.returncode==0 and 'Signature=adhoc' in signinfo.stderr
(ev/'macos-codesign.txt').write_text((codesign.stdout+codesign.stderr+signinfo.stdout+signinfo.stderr).replace(str(root),'<workspace>'))
(ev/'windows-icon-resource-verification.json').write_bytes((build/'windows/icon-resource-verification.json').read_bytes())
observation=json.loads((root/'research/evidence/relink-screenshots-902118a/observation.json').read_bytes());assert observation['source_commit']==rev
for entry in observation['images']:
 image=(root/entry['published']).read_bytes();assert sha(image)==entry['png_sha256'];shutil.copyfile(root/entry['published'],build/Path(entry['published']).name)
icon=gitbytes('packaging/branding/ravelink.png');(build/'ReLink-icon.png').write_bytes(icon)
verification={'schema':1,'verified_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_revision':rev,'run_id':40254943747,'run_url':run['html_url'],'ci_status':'completed','ci_conclusion':'success','platforms':{'macos':{'architecture':arch,'bundle':'ReLink.app','strict_codesign_exit':0,'signature':'ad hoc; not notarized','executable_sha256':sha((contents/'MacOS/ravelink-app').read_bytes()),'executable_mode_preserved':True,'icon_and_notices_match_exact_source':True,'application_started_in_this_verification':False},'windows':{'architecture':'x86_64','executable':'ReLink.exe','executable_sha256':sha(exe),'native_icon_report_matches_exact_packaged_executable_and_source':True,'icon_resources':8,'certificate_directory':list(certificate),'signature':'unsigned','application_started_in_this_verification':False,'shell_rendering_verified':False}},'screenshots':{'source_revision':rev,'kind':'Stopped native macOS UI from isolated local debug build of same source; separate from CI release binaries','images':observation['images']},'standalone_icon':{'asset_name':'ReLink-icon.png','sha256':sha(icon),'bytes':len(icon)},'limitations':['Development preview; app-only packages contain no driver installer.','Installed single-output run40254943736 failed both its separate HAL and runtime observations; no audio qualification is inferred.','Installed multi-output runtime not yet observed.','Windows interaction and installed audio remain unverified.','macOS signature is ad hoc without Developer ID signing or notarization.']}
(ev/'preview-verification.json').write_text(json.dumps(verification,indent=2)+'\n');shutil.copyfile(ev/'preview-verification.json',build/'preview-verification.json')
manifest={str(p.relative_to(build)):{'bytes':p.stat().st_size,'sha256':sha(p.read_bytes())} for base in [build/'macos',build/'windows'] for p in sorted(base.rglob('*')) if p.is_file()};(ev/'package-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
for name in ['run.json','jobs.json','artifacts.json']:shutil.copy2(build/name,ev/name)
(ev/'source-workflow.yml').write_bytes(gitbytes('.github/workflows/rust.yml'))
assets=['ReLink-macOS-development.zip','ReLink-Windows-development.zip','mixer-macos.png','mix-effects-macos.png','output-routing-macos.png','ReLink-icon.png','preview-verification.json']
checksums=''.join(sha((build/n).read_bytes())+'  '+n+'\n' for n in assets)
(build/'SHA256SUMS.txt').write_text(checksums);(ev/'SHA256SUMS.txt').write_text(checksums)
print(json.dumps({'verified':True,'package_files':len(manifest),'release_assets':len(assets)+1,'macos':verification['platforms']['macos'],'windows':verification['platforms']['windows']},indent=2))
