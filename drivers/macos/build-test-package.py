#!/usr/bin/env python3
"""Build an unsigned, first-install-only local Rust HAL test package.

No install, launch, service restart, device query or administrator action occurs.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / 'drivers/macos'
DESTINATION = Path('Library/Audio/Plug-Ins/HAL/ReLink.driver')
INSTALL_LOCATION = '/Library/Audio/Plug-Ins/HAL'
PAYLOAD_BUNDLE = Path('ReLink.driver')
BUNDLE_ID = 'au.textual.ravelink.hal'
PACKAGE_ID = 'au.textual.relink.test-hal'


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def files(root: Path) -> dict[str, dict]:
    result = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError(f'Symlink is not permitted in package/source contents: {path}')
        if path.is_file():
            result[str(path.relative_to(root))] = {'bytes': path.stat().st_size, 'sha256': digest(path)}
    return result


def source_paths() -> list[Path]:
    paths = [ROOT / name for name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'LICENSE')]
    paths += list((ROOT / 'crates').glob('*/Cargo.toml'))
    paths += [path for path in (ROOT / 'crates/relink-hal').rglob('*') if path.is_file()]
    paths += list((ROOT / 'docs/third-party-licenses/rust-1.91.1').glob('*'))
    paths += [DRIVER / name for name in ('Info.plist', 'build-bundle.sh', 'build-test-package.sh',
                                       'build-test-package.py', 'test-install-preflight.sh')]
    return sorted(set(paths))


def source_state(paths: list[Path]) -> dict[str, str]:
    result = {}
    for path in paths:
        if path.is_symlink() or not path.is_file():
            raise ValueError(f'Expected an ordinary source file: {path}')
        result[str(path.relative_to(ROOT))] = digest(path)
    return result


def run(args: list[str], log, env=None) -> str:
    log.write('$ ' + repr(args) + '\n'); log.flush()
    result = subprocess.run(args, cwd=ROOT, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log.write(result.stdout); log.flush()
    if result.returncode:
        raise RuntimeError(f'Command failed ({result.returncode}): {args[0]}; see build.log')
    return result.stdout.strip()


def inspect_expanded(expanded: Path, bundle: Path, preinstall: bytes) -> dict:
    package_info = ET.parse(expanded / 'PackageInfo').getroot()
    if package_info.get('identifier') != PACKAGE_ID or package_info.get('install-location') != INSTALL_LOCATION:
        raise ValueError('Unexpected package identity or installation root')
    scripts = package_info.find('scripts')
    if scripts is None or [child.tag for child in scripts] != ['preinstall']:
        raise ValueError('Package must contain only its preinstall guard')
    script_files = files(expanded / 'Scripts')
    if set(script_files) != {'preinstall'} or (expanded / 'Scripts/preinstall').read_bytes() != preinstall:
        raise ValueError('Archived preinstall guard differs from inspected source')
    payload = expanded / 'Payload'
    archived = payload / PAYLOAD_BUNDLE
    expected = files(bundle)
    if files(archived) != expected:
        raise ValueError('Archived driver bytes differ from the signed staged bundle')
    if set(files(payload)) != {str(PAYLOAD_BUNDLE / path) for path in expected}:
        raise ValueError('Package contains files outside the one permitted driver bundle')
    bundle_records = package_info.findall('bundle')
    if len(bundle_records) != 1 or bundle_records[0].get('id') != BUNDLE_ID:
        raise ValueError('Unexpected archived bundle record')
    if package_info.get('postinstall-action') != 'none' or package_info.get('relocatable') != 'false':
        raise ValueError('Package must not request a restart or allow relocation')
    return {'identifier': PACKAGE_ID, 'installLocation': INSTALL_LOCATION, 'payloadFiles': expected,
            'scripts': script_files, 'onlyDestination': '/' + str(DESTINATION)}


def build(output: Path) -> Path:
    if sys.platform != 'darwin':
        raise ValueError('The local test package requires macOS')
    output = output.expanduser().absolute()
    if output.exists() or output.is_symlink():
        raise ValueError(f'Output already exists; choose a fresh directory: {output}')
    if not output.parent.is_dir():
        raise ValueError('Output parent must already exist')
    # Override toolchain wrappers/flags rather than silently attributing injected code.
    for name in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
                 'RUSTC', 'CARGO_BUILD_RUSTFLAGS', 'RUSTUP_TOOLCHAIN'):
        if os.environ.get(name):
            raise ValueError(f'Unset {name} for this reproducible local test build')
    output.mkdir()  # Atomic refusal if another builder claimed the same output.
    with (output / 'build.log').open('w') as log:
        compiler = run(['rustc', '-vV'], log)
        target = next(line.removeprefix('host: ') for line in compiler.splitlines() if line.startswith('host: '))
        architecture = {'aarch64-apple-darwin': 'arm64', 'x86_64-apple-darwin': 'x86_64'}.get(target)
        if architecture is None:
            raise ValueError('A native macOS Rust target is required')
        paths = source_paths(); before = source_state(paths)
        snapshot = output / 'source-inputs'
        for path in paths:
            copied = snapshot / path.relative_to(ROOT); copied.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, copied)
        provenance = {
            'schema': 1, 'purpose': 'local first-install test; not a production release',
            'createdUTC': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'gitHEAD': run(['git', 'rev-parse', 'HEAD'], log),
            'workingTreeStatus': run(['git', 'status', '--porcelain=v1'], log).splitlines(),
            'sourceSHA256': before, 'compiler': compiler, 'cargo': run(['cargo', '--version'], log),
            'sdkVersion': run(['xcrun', '--show-sdk-version'], log),
            'sdkPath': run(['xcrun', '--show-sdk-path'], log),
            'target': target, 'architecture': architecture, 'bundleID': BUNDLE_ID,
            'deviceUIDs': ['au.textual.ravelink.cable.1', 'au.textual.ravelink.cable.2'],
            'bundleSigning': 'ad-hoc', 'packageSigning': 'unsigned', 'notarized': False,
            'installationPerformed': False,
        }
        staging = output / 'staging'; bundle = staging / PAYLOAD_BUNDLE
        env = os.environ.copy()
        env.update(RELINK_HAL_TARGET=target, RELINK_HAL_OUTPUT=str(bundle), RELINK_SIGN_IDENTITY='-',
                   CARGO_TARGET_DIR=str(output / 'cargo-target'))
        run(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], log, env)
        run(['/bin/sh', str(DRIVER / 'build-bundle.sh')], log, env)
        if source_state(source_paths()) != before:
            raise ValueError('Source inputs changed during build; discard this incomplete package')
        info = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())
        if info.get('CFBundleIdentifier') != BUNDLE_ID or info.get('CFBundleExecutable') != 'ReLinkHAL':
            raise ValueError('Unexpected built driver identity')
        binary = bundle / 'Contents/MacOS/ReLinkHAL'
        if run(['/usr/bin/lipo', '-archs', str(binary)], log) != architecture:
            raise ValueError('Driver architecture does not match the selected native target')
        # The final signature seals the embedded provenance and changes Mach-O
        # signature bytes. Only the external record can contain its final hash.
        provenance['preFinalSigningBinarySHA256'] = digest(binary)
        (bundle / 'Contents/Resources/test-build-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
        run(['/usr/bin/codesign', '--force', '--sign', '-', str(bundle)], log)
        run(['/usr/bin/codesign', '--verify', '--strict', str(bundle)], log)
        run(['/usr/bin/codesign', '--display', '--verbose=4', str(bundle)], log)
        provenance['binarySHA256'] = digest(binary)
        scripts = output / 'scripts'; scripts.mkdir()
        guard = (DRIVER / 'test-install-preflight.sh').read_bytes() + (
            f'case "$(/usr/bin/uname -m)" in {architecture}) ;; *) refuse "this package requires {architecture}" ;; esac\n'
        ).encode()
        (scripts / 'preinstall').write_bytes(guard); (scripts / 'preinstall').chmod(0o755)
        component = output / 'component.plist'
        component.write_bytes(plistlib.dumps([{
            'RootRelativeBundlePath': str(PAYLOAD_BUNDLE), 'BundleIsRelocatable': False,
            'BundleIsVersionChecked': False, 'BundleHasStrictIdentifier': True,
            'BundleOverwriteAction': 'upgrade',
        }]))
        package = output / f'ReLink-Rust-HAL-Test-{architecture}.pkg'
        run(['/usr/bin/pkgbuild', '--root', str(staging), '--component-plist', str(component),
             '--identifier', PACKAGE_ID, '--version', info['CFBundleVersion'], '--install-location', INSTALL_LOCATION,
             '--ownership', 'recommended', '--scripts', str(scripts), str(package)], log)
        expanded = output / 'expanded'
        run(['/usr/sbin/pkgutil', '--expand-full', str(package), str(expanded)], log)
        inspection = inspect_expanded(expanded, bundle, guard)
        if inspection['payloadFiles']['Contents/MacOS/ReLinkHAL']['sha256'] != provenance['binarySHA256']:
            raise ValueError('Final signed binary digest differs from archived payload')
        run(['/usr/bin/codesign', '--verify', '--strict', str(expanded / 'Payload' / PAYLOAD_BUNDLE)], log)
        signature = subprocess.run(['/usr/sbin/pkgutil', '--check-signature', str(package)],
                                   text=True, capture_output=True)
        signature_text = signature.stdout + signature.stderr
        log.write(signature_text)
        if 'Status: no signature' not in signature_text:
            raise ValueError('Expected an unsigned local test package; signature inspection was inconclusive')
        provenance.update(packageFile=package.name, packageSHA256=digest(package), packageBytes=package.stat().st_size,
                          inspection=inspection, packageSignatureInspection=signature_text.strip())
        (output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
        (output / 'SHA256SUMS').write_text(f'{digest(package)}  {package.name}\n')
        print(package)
        return package


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='Fresh output directory; its parent must already exist')
    args = parser.parse_args()
    try:
        build(args.output)
    except (OSError, ValueError, RuntimeError, StopIteration) as error:
        print(f'Test package build failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
