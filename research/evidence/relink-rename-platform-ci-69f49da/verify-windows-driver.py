from datetime import datetime,timezone
from pathlib import Path,PureWindowsPath
import hashlib,importlib.util,json,re,struct,zipfile
from collect import BUILD
BASE=BUILD; HERE=BUILD/'independent-windows-review'; HERE.mkdir(parents=True,exist_ok=True); SOURCE=BASE/'source'; DRIVER=BASE/'driver'
REV='69f49da28a9bae16dec9d4bc29bd0d639fb73a08'
def digest(data):return {'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
art=json.loads((DRIVER/'artifacts.json').read_text())['artifacts'];assert len(art)==1
archive=DRIVER/'ReLink-Windows-driver-unsigned.zip';raw=archive.read_bytes();assert 'sha256:'+hashlib.sha256(raw).hexdigest()==art[0]['digest'] and len(raw)==art[0]['size_in_bytes']
publictree=json.loads((BASE/'source-tree.json').read_text()); blobs={e['path']:e['sha']for e in publictree['tree']if e['type']=='blob'}
sourcepaths=['drivers/windows/tests/check-pe.py','drivers/windows/ReLink.inf','drivers/windows/toolchain.json','drivers/windows/PROVENANCE.md','drivers/windows/build.ps1','LICENSE','.github/workflows/rust-windows-driver.yml']
sourceids={}
for p in sourcepaths:
 data=(SOURCE/p).read_bytes();blob=hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest();assert blob==blobs[p],p;sourceids[p]={'git_blob_sha':blob,**digest(data)}
with zipfile.ZipFile(archive)as z:
 assert z.testzip()is None
 names=z.namelist();assert len(names)==len(set(names))==17
 assert not any('ravelink'in Path(n).name.lower()for n in names)
 expected={'build-evidence.json','DRIVER-RUNTIME-NOTICES.txt','LICENSE','pe-evidence.json','PROVENANCE.md','relink.cat','ReLink.inf','ReLink.sys','relink_windows_driver.pdb','toolchain.json','runtime-notices/manifest.json'}
 notices=json.loads(z.read('runtime-notices/manifest.json'));assert len(notices)==6
 expected|={'runtime-notices/'+n for n in notices};assert set(names)==expected
 for n in names:
  p=HERE/'extracted'/n;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(z.read(n))
 members={n:digest(z.read(n))for n in names}
 for n,p in {'ReLink.inf':'drivers/windows/ReLink.inf','toolchain.json':'drivers/windows/toolchain.json','PROVENANCE.md':'drivers/windows/PROVENANCE.md','LICENSE':'LICENSE'}.items():assert z.read(n)==(SOURCE/p).read_bytes(),n
 for n,record in notices.items():assert hashlib.sha256(z.read('runtime-notices/'+n)).hexdigest()==record['sha256']
 build=json.loads(z.read('build-evidence.json'));pe=json.loads(z.read('pe-evidence.json'));pin=json.loads(z.read('toolchain.json'))
 assert build['target']==pin['target'] and build['sdk']==build['wdk']==pin['packages'][0]['version']
 assert pin['rust_commit']in build['rust'] and build['abi_compiler'].startswith(pin['msvc_validation_toolset']+'.')
 assert build['kernel_loaded']is False and build['signed']is False and build['catalog_created']is True
 inf=z.read('ReLink.inf').decode();assert 'Root\\RaveLinkAudio'in inf and 'AddService=RaveLinkAudio,'in inf
 assert 'ServiceBinary=%13%\\ReLink.sys'in inf
 catalog=re.search(r'^CatalogFile=(.+)$',inf,re.M).group(1);assert catalog.casefold()in {n.casefold()for n in names}
 data=z.read('ReLink.sys');pdb=z.read('relink_windows_driver.pdb')
spec=importlib.util.spec_from_file_location('pinned_pe',(SOURCE/'drivers/windows/tests/check-pe.py'));module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
recomputed=module.inspect(HERE/'extracted/ReLink.sys');assert recomputed==pe
# Match the PE's CodeView GUID/age to the supplied MSF 7 PDB info stream.
u16=lambda p:struct.unpack_from('<H',data,p)[0];u32=lambda p:struct.unpack_from('<I',data,p)[0]
peoff=u32(0x3c);op=peoff+24;sec=op+u16(peoff+20)
sections=[struct.unpack_from('<IIII',data,sec+i*40+8)for i in range(u16(peoff+6))]
def locate(rva):
 for virtual,base,rawsize,offset in sections:
  if base<=rva<base+max(virtual,rawsize):assert rva-base<rawsize;return offset+rva-base
 raise AssertionError(rva)
debugrva,debugsize=struct.unpack_from('<II',data,op+112+6*8);debug=locate(debugrva);records=[]
for p in range(debug,debug+debugsize,28):
 typ,size,_,rawpos=struct.unpack_from('<IIII',data,p+12)
 if typ==2:
  cv=data[rawpos:rawpos+size];assert cv[:4]==b'RSDS';records.append((cv[4:20],struct.unpack_from('<I',cv,20)[0],cv[24:].split(b'\0',1)[0].decode()))
assert len(records)==1;guid,age,path=records[0];assert PureWindowsPath(path).name=='relink_windows_driver.pdb'
assert pdb[:32]==b'Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0'
block,_,count,dirbytes,_,blockmap=struct.unpack_from('<IIIIII',pdb,32);assert len(pdb)==block*count
pages=(dirbytes+block-1)//block;indices=struct.unpack_from('<'+'I'*pages,pdb,blockmap*block)
directory=b''.join(pdb[i*block:(i+1)*block]for i in indices)[:dirbytes]
streams=struct.unpack_from('<I',directory)[0];sizes=struct.unpack_from('<'+'I'*streams,directory,4);cursor=4+streams*4;stream1=None
for index,size in enumerate(sizes):
 if size==0xffffffff:continue
 n=(size+block-1)//block;ix=struct.unpack_from('<'+'I'*n,directory,cursor);cursor+=4*n
 if index==1:stream1=b''.join(pdb[i*block:(i+1)*block]for i in ix)[:size]
assert stream1 is not None and struct.unpack_from('<I',stream1,8)[0]==age and stream1[12:28]==guid
with zipfile.ZipFile(DRIVER/'original-logs.zip')as z:
 assert z.testzip()is None;log=z.read('unsigned-package/5_Build unsigned driver, verify SDK ABI and INF, create unsigned catalog.txt').decode('utf-8-sig')
 for marker in ('9 passed; 0 failed','abi-check.cpp','Signability test complete.','Catalog generation complete.','relink.cat','Unsigned package and validation evidence:'):assert marker in log,marker
report={'schema':1,'verified_at_utc':datetime.now(timezone.utc).isoformat(),'result':'PASS','run':40257398430,'source_revision':REV,'artifact':{'id':art[0]['id'],'name':art[0]['name'],**digest(raw),'github_digest_match':True},'archive_crc_exact_membership':True,'members':members,'exact_products':['ReLink.sys','ReLink.inf','relink.cat','relink_windows_driver.pdb'],'old_named_artifacts_present':False,'catalog_name_note':'Inf2Cat emitted lowercase relink.cat. INF specifies ReLink.cat; the names match Windows case-insensitive file resolution.','inf_preserved_hardware_id':'Root\\RaveLinkAudio','inf_preserved_service_id':'RaveLinkAudio','packaged_source_files_match_exact_revision':True,'pinned_source_files':sourceids,'build_report':build,'original_pe_assertions_recomputed_exactly':recomputed,'pdb_matches_pe_codeview':{'guid_raw_hex':guid.hex(),'age':age,'filename':PureWindowsPath(path).name},'runtime_notice_files_hash_verified':6,'native_job_log_markers_verified':['nine host tests passed','compile-only SDK ABI source','Inf2Cat signability complete','catalog generation complete'],'limits':['Unsigned build/packaging and PE structure only. No Windows kernel loading, endpoint discovery, native audio, driver signing, production qualification or installed upgrade was observed.','Catalog presence/name and successful Inf2Cat log were verified; no independent Windows catalog trust or installation validation was performed.'],'reviewer_git_audio_or_public_mutations':False,'metadata_read_note':'Small GitHub run/jobs/artifact API reads were performed; artifact bytes came from the parent collector.'}
(HERE/'verification-recomputed.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'result':'PASS','members':len(members),'exact_products':report['exact_products'],'pe_assertions':'exact match','pdb_codeview':'match','notice_files':6,'kernel_loaded':False,'signed':False,'report':str(HERE/'verification-recomputed.json')},indent=2))
