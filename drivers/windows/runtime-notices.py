#!/usr/bin/env python3
"""Copy notices from the exact compiler sysroot used for the no_std image."""
import argparse
import hashlib
import json
import pathlib
import shutil

parser = argparse.ArgumentParser()
parser.add_argument('--sysroot', type=pathlib.Path, required=True)
parser.add_argument('--output', type=pathlib.Path, required=True)
args = parser.parse_args()
source = args.sysroot
output = args.output / 'runtime-notices'
output.mkdir(parents=True, exist_ok=True)
files = {
    'Rust-COPYRIGHT-library.html': 'share/doc/rust/COPYRIGHT-library.html',
    'Rust-MIT.txt': 'share/doc/rust/licenses/MIT.txt',
    'Rust-Apache-2.0.txt': 'share/doc/rust/licenses/Apache-2.0.txt',
    'Rust-Unicode-3.0.txt': 'share/doc/rust/licenses/Unicode-3.0.txt',
    'compiler-builtins-LICENSE.txt': 'lib/rustlib/src/rust/library/compiler-builtins/LICENSE.txt',
    'compiler-builtins-libm-LICENSE.txt': 'lib/rustlib/src/rust/library/compiler-builtins/libm/LICENSE.txt',
}
manifest = {}
for name, relative in files.items():
    original = source / relative
    if not original.is_file():
        raise SystemExit(f'Required runtime notice missing: {original}')
    shutil.copyfile(original, output / name)
    manifest[name] = {'source': relative, 'sha256': hashlib.sha256(original.read_bytes()).hexdigest()}
(output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
(args.output / 'DRIVER-RUNTIME-NOTICES.txt').write_text('''ReLink Windows driver runtime notices

The driver is proprietary original Rust code. Its binary also includes compiled
portions of Rust core and compiler_builtins from nightly-2025-11-10, rustc
1.93.0-nightly (6647be936 2025-11-09). Their notices and licenses remain applicable.

Rust core: MIT OR Apache-2.0. Copyright The Rust Project Developers and
contributors; see https://thanks.rust-lang.org and the bundled upstream inventory.
Core's Unicode data carries Unicode-3.0 terms; its notice is supplied conservatively.
compiler_builtins 0.1.160: MIT AND Apache-2.0 WITH LLVM-exception AND
(MIT OR Apache-2.0), including upstream compiler-rt attribution and bundled libm
notices. Complete upstream license files are in runtime-notices/.

Rust-COPYRIGHT-library.html is the complete upstream standard-library copyright
inventory, supplied conservatively. It lists libraries beyond the no_std subset
linked here; its inclusion does not mean those other libraries are in this image.
The manifest records the exact source paths and SHA-256 hashes of supplied notices.

Microsoft system components ntoskrnl.exe, hal.dll, and portcls.sys are imported
from Windows and are not redistributed in this package. Microsoft SDK/WDK tools,
headers, and import libraries are build inputs, not bundled driver implementations.
''')
