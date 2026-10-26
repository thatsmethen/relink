#!/usr/bin/env python3
"""Recheck saved CI archives/source on macOS, without loading the app or driver.

Run collect.py for each run and source first. Original ZIPs are never rewritten.
The native Windows icon evidence is checked against the exact EXE/source hashes;
Windows resource enumeration itself is not repeated by this macOS verifier.
"""
import ast, datetime, hashlib, importlib.util, json, pathlib, plistlib, stat, struct, subprocess, zipfile
from collect import BUILD, EVIDENCE, REVISION, RUNS, sha, keep


def read_json(p): return json.loads(p.read_bytes())
def load_module(name,path):
    spec=importlib.util.spec_from_file_location(name,path);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module

def check_archive(path,expected):
    data=path.read_bytes();assert len(data)==expected['bytes'] and sha(data)==expected['sha256'],path
    with zipfile.ZipFile(path) as z:
        assert z.testzip() is None
        members={n:{'bytes':len(z.read(n)),'sha256':sha(z.read(n))} for n in z.namelist() if not n.endswith('/')}
        assert members==expected['members'],path
    return len(members)

def extract(path,destination):
    with zipfile.ZipFile(path) as z:
        assert len(z.namelist())==len(set(z.namelist()))
        for info in z.infolist():
            name=pathlib.PurePosixPath(info.filename)
            assert not name.is_absolute() and '..' not in name.parts
            assert not stat.S_ISLNK(info.external_attr>>16)
            if not info.is_dir(): keep(destination/name,z.read(info))

def write_report(name,value):
    (EVIDENCE/name).write_text(json.dumps(value,indent=2)+'\n')

def main():
    sources=read_json(EVIDENCE/'source-file-manifest.json')
    for name,expected in sources.items():
        data=(BUILD/'source'/name).read_bytes()
        assert len(data)==expected['bytes'] and sha(data)==expected['sha256'],name
        assert hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()==expected['git_blob_sha1'],name
    runs={}; archive_count=0; member_count=0
    for kind,run_id in RUNS.items():
        directory=BUILD/kind;run=read_json(directory/'run.json');jobs=read_json(directory/'jobs.json')
        assert run['id']==run_id and run['head_sha']==REVISION and run['run_attempt']==1
        assert run['status']=='completed' and run['conclusion']=='success'
        assert all(j['conclusion']=='success' for j in jobs['jobs'])
        assert len(jobs['jobs'])== (1 if kind=='driver' else 2)
        arts=read_json(directory/'artifacts.json')['artifacts']
        manifest=read_json(directory/'original-archive-manifest.json')
        assert len(arts)== (1 if kind=='driver' else 2)
        for item in arts:
            entry=manifest[item['name']+'.zip']
            assert item['digest']=='sha256:'+entry['sha256'] and item['workflow_run']['head_sha']==REVISION
        for name,expected in manifest.items():
            member_count+=check_archive(directory/name,expected);archive_count+=1
        runs[kind]={'run_id':run_id,'url':run['html_url'],'status':run['status'],'conclusion':run['conclusion'],'attempt':1,'jobs':[{'name':j['name'],'conclusion':j['conclusion']} for j in jobs['jobs']]}
    driver=BUILD/'extracted/driver'
    extract(BUILD/'driver/ReLink-Windows-driver-unsigned.zip',driver)
    names={p.relative_to(driver).as_posix() for p in driver.rglob('*') if p.is_file()}
    assert {'ReLink.sys','ReLink.inf','relink.cat','relink_windows_driver.pdb'}<=names
    assert not any('ravelink' in p.lower() for p in names)
    for name,source in [('ReLink.inf','drivers/windows/ReLink.inf'),('LICENSE','LICENSE'),('PROVENANCE.md','drivers/windows/PROVENANCE.md'),('toolchain.json','drivers/windows/toolchain.json')]:
        assert (driver/name).read_bytes()==(BUILD/'source'/source).read_bytes(),name
    pe=load_module('pinned_pe',BUILD/'source/drivers/windows/tests/check-pe.py')
    original_pe=read_json(driver/'pe-evidence.json');assert pe.inspect(driver/'ReLink.sys')==original_pe
    build=read_json(driver/'build-evidence.json');pin=read_json(driver/'toolchain.json')
    assert build['target']==pin['target'] and pin['rust_commit'] in build['rust']
    assert build['wdk']==pin['packages'][0]['version'] and build['sdk']==pin['packages'][1]['version']
    assert build['catalog_created'] is True and build['signed'] is False and build['kernel_loaded'] is False
    assert original_pe['signed'] is False
    notices=read_json(driver/'runtime-notices/manifest.json')
    for name,entry in notices.items():assert sha((driver/'runtime-notices'/name).read_bytes())==entry['sha256']
    assert len(notices)==6
    notice_source=ast.parse((BUILD/'source/drivers/windows/runtime-notices.py').read_text())
    notice_paths=next(ast.literal_eval(n.value) for n in notice_source.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='files' for t in n.targets))
    assert set(notices)==set(notice_paths)
    for name,path in notice_paths.items():assert notices[name]['source']==path
    notice_text=next(ast.literal_eval(n.value.args[0]) for n in notice_source.body if isinstance(n,ast.Expr) and isinstance(n.value,ast.Call) and isinstance(n.value.func,ast.Attribute) and n.value.func.attr=='write_text' and isinstance(n.value.args[0],ast.Constant))
    assert (driver/'DRIVER-RUNTIME-NOTICES.txt').read_text()==notice_text
    for n in ['build-evidence.json','pe-evidence.json','DRIVER-RUNTIME-NOTICES.txt']:
        keep(EVIDENCE/'driver'/n,(driver/n).read_bytes())
    keep(EVIDENCE/'driver/runtime-notices-manifest.json',(driver/'runtime-notices/manifest.json').read_bytes())
    for platform in ['macOS','Windows']:
        extract(BUILD/'desktop'/f'ReLink-{platform}-development.zip',BUILD/'extracted/desktop-archives')
    archive_dir=BUILD/'extracted/desktop-archives'
    inner={}
    for path in sorted(archive_dir.glob('*.zip')):
        with zipfile.ZipFile(path) as z:
            inner[path.name]={'bytes':path.stat().st_size,'sha256':sha(path.read_bytes()),'members':{n:{'bytes':len(z.read(n)),'sha256':sha(z.read(n))} for n in z.namelist() if not n.endswith('/')}}
    assert set(inner)=={'ReLink-macOS-development.zip','ReLink-Windows-development.zip'}
    write_report('inner-package-archive-manifest.json',inner)
    macos=BUILD/'extracted/macos';macos.mkdir(parents=True,exist_ok=True)
    # ditto preserves the signed bundle's executable mode and Apple ZIP metadata.
    subprocess.run(['ditto','-x','-k',str(archive_dir/'ReLink-macOS-development.zip'),str(macos)],check=True)
    windows=BUILD/'extracted/windows';extract(archive_dir/'ReLink-Windows-development.zip',windows)
    app=macos/'ReLink.app';contents=app/'Contents';plist=plistlib.loads((contents/'Info.plist').read_bytes())
    assert (contents/'Info.plist').read_bytes()==(BUILD/'source/packaging/macos/Info.plist').read_bytes()
    assert plist['CFBundleExecutable']=='relink-app' and plist['CFBundleIconFile']=='ReLink.icns'
    assert plist['CFBundleName']==plist['CFBundleDisplayName']=='ReLink'
    assert plist['CFBundleIdentifier']=='au.textual.ravelink.rust'
    assert {p.name for p in (contents/'MacOS').iterdir()}=={'relink-app'}
    executable=contents/'MacOS/relink-app';assert executable.stat().st_mode&stat.S_IXUSR
    arch=subprocess.check_output(['lipo','-archs',str(executable)],text=True).strip();assert arch=='arm64'
    for n in ['RaveLink.icns','RaveLink-LICENSE.txt']:assert not (contents/'Resources'/n).exists()
    for packaged,source in [(contents/'Resources/ReLink.icns','packaging/branding/relink.icns'),(contents/'Resources/ReLink-LICENSE.txt','LICENSE'),(contents/'Resources/THIRD-PARTY-NOTICES.txt','THIRD-PARTY-NOTICES.txt'),(windows/'LICENSE','LICENSE'),(windows/'THIRD-PARTY-NOTICES.txt','THIRD-PARTY-NOTICES.txt')]:
        assert packaged.read_bytes()==(BUILD/'source'/source).read_bytes(),str(packaged)
    for directory in [contents/'Resources/Rust-runtime-notices',windows/'Rust-runtime-notices']:
        for p in directory.iterdir(): assert p.read_bytes()==(BUILD/'source/docs/third-party-licenses/rust-1.91.1'/p.name).read_bytes()
        assert len(list(directory.iterdir()))==4
    signature=subprocess.run(['codesign','--verify','--deep','--strict','--verbose=2',str(app)],capture_output=True,text=True);assert signature.returncode==0,signature.stderr
    signinfo=subprocess.run(['codesign','-dvv',str(app)],capture_output=True,text=True);assert signinfo.returncode==0 and 'Signature=adhoc' in signinfo.stderr
    (EVIDENCE/'macos-codesign.txt').write_text((signature.stdout+signature.stderr+signinfo.stdout+signinfo.stderr).replace(str(BUILD),'<original-artifact-build>'))
    report=read_json(windows/'icon-resource-verification.json');exe=(windows/'ReLink.exe').read_bytes();assert sha(exe)==report['executable_sha256']
    for field,path in [('ico_sha256','packaging/branding/relink.ico'),('res_sha256','packaging/branding/relink.res'),('script_sha256','scripts/verify-windows-icon.py')]:assert sha((BUILD/'source'/path).read_bytes())==report[field]
    icons=load_module('pinned_icons',BUILD/'source/scripts/verify-windows-icon.py')
    resources=icons.expected_resources((BUILD/'source/packaging/branding/relink.ico').read_bytes());icons.verify_equal(resources,icons.res_resources((BUILD/'source/packaging/branding/relink.res').read_bytes()))
    assert len(report['resources'])==len(resources)==8
    for r in report['resources']:
        data=resources[r['type'],r['id'],r['language']];assert r['sha256']==sha(data) and r['size']==len(data)
    pe_offset=struct.unpack_from('<I',exe,0x3c)[0];assert exe[pe_offset:pe_offset+4]==b'PE\0\0' and struct.unpack_from('<H',exe,pe_offset+4)[0]==0x8664
    optional=pe_offset+24;assert struct.unpack_from('<H',exe,optional)[0]==0x20b
    certificate=struct.unpack_from('<II',exe,optional+112+4*8);assert certificate==(0,0)
    assert not any('ravelink' in p.name.lower() for p in windows.rglob('*'))
    keep(EVIDENCE/'windows-icon-resource-verification.json',(windows/'icon-resource-verification.json').read_bytes())
    package_manifest={str(p.relative_to(BUILD/'extracted')):{'bytes':p.stat().st_size,'sha256':sha(p.read_bytes())} for directory in [driver,macos,windows] for p in sorted(directory.rglob('*')) if p.is_file()}
    write_report('package-manifest.json',package_manifest)
    verification={'schema':1,'source_revision':REVISION,'verified_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'runs':runs,'original_archives':archive_count,'original_archive_members':member_count,'source_files_checked':len(sources),'packaged_files_checked':len(package_manifest),
      'driver':{'architecture':'x86_64','image':'ReLink.sys','sha256':sha((driver/'ReLink.sys').read_bytes()),'native_build_sdk_abi_inf_and_catalog_steps_passed':True,'catalog_filename':'relink.cat','catalog_case':'Inf2Cat uses lowercase; Windows filename matching is case-insensitive','pe_report_recomputed_exactly':True,'source_inf_license_provenance_toolchain_match':True,'runtime_notice_hashes_verified':len(notices),'signed':False,'kernel_loaded_in_ci':False},
      'macos_app':{'architecture':arch,'bundle':'ReLink.app','executable':'relink-app','icon':'ReLink.icns','executable_sha256':sha(executable.read_bytes()),'signature':'ad hoc; not notarized','strict_signature_pass':True,'icon_notices_plist_match_exact_source':True,'old_generated_payload_names_absent':True},
      'windows_app':{'architecture':'x86_64','executable':'ReLink.exe','executable_sha256':sha(exe),'native_icon_report_bound_to_executable_and_exact_source':True,'resources_verified':8,'signature':'unsigned','certificate_directory':list(certificate),'old_generated_payload_names_absent':True},
      'limitations':['Build and packaging evidence only; neither application was launched during verification.','Windows driver was not installed or loaded; no Windows device enumeration, audio, Explorer icon rendering or UI interaction qualification.','macOS application uses ad-hoc signing without Developer ID signing or notarization.','The macOS CI HAL ABI harness uses a local synthetic host, not an installed audio device.','No CI rerun, release publication or change to prior observations was performed.']}
    write_report('verification.json',verification)
    print(json.dumps({'verified':True,'original_archives':archive_count,'original_archive_members':member_count,'packaged_files':len(package_manifest)},indent=2))

if __name__=='__main__':main()
