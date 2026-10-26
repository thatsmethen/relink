"""Local package guards and archive inspector controls; never install a driver."""
import importlib.util
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest

DRIVER = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('test_package_builder', DRIVER / 'build-test-package.py')
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class LocalTestPackageTests(unittest.TestCase):
    def guard(self, directory, volume='/'):
        # Exercise the exact guard against a disposable path, never host HAL.
        source = (DRIVER / 'test-install-preflight.sh').read_text()
        source = source.replace('/Library', str(Path(directory) / 'Library'))
        return subprocess.run(['/bin/sh', '-s', '--', 'package', '/', volume], input=source,
                              text=True, capture_output=True, timeout=5)

    def test_first_install_allows_absent_destination(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.guard(directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((Path(directory) / 'Library').exists())

    def test_refuses_every_existing_destination_including_own_bundle(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / builder.DESTINATION
            destination.mkdir(parents=True)
            sentinel = destination / 'unrelated-owner'; sentinel.write_text('unchanged')
            result = self.guard(directory)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('destination already exists', result.stderr)
            self.assertEqual(sentinel.read_text(), 'unchanged')

    def test_refuses_symlink_target_and_each_ancestor(self):
        for relative in ('Library', 'Library/Audio', 'Library/Audio/Plug-Ins',
                         'Library/Audio/Plug-Ins/HAL', str(builder.DESTINATION)):
            with self.subTest(relative=relative), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.symlink_to(Path(directory) / 'absent-target')
                result = self.guard(directory)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('symlink', result.stderr)
                self.assertTrue(path.is_symlink())

    def test_refuses_non_directory_and_external_volume(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertIn('startup volume', self.guard(directory, '/Volumes/Other').stderr)
            (Path(directory) / 'Library').write_text('ordinary-file')
            self.assertIn('non-directory ancestor', self.guard(directory).stderr)

    def test_refuses_own_identity_under_other_name_or_symlink(self):
        for symlink in (False, True):
            with self.subTest(symlink=symlink), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                hal = root / builder.DESTINATION.parent; hal.mkdir(parents=True)
                bundle = root / 'original' if symlink else hal / 'RustReLink.driver'
                (bundle / 'Contents').mkdir(parents=True)
                info = bundle / 'Contents/Info.plist'
                info.write_bytes(plistlib.dumps({'CFBundleIdentifier': builder.BUNDLE_ID}))
                if symlink:
                    (hal / 'OtherName.driver').symlink_to(bundle)
                result = self.guard(directory)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('ReLink bundle identity already exists', result.stderr)
                self.assertTrue(info.is_file())

    def test_unrelated_bundle_is_not_changed(self):
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / builder.DESTINATION.parent / 'Unrelated.driver'
            (bundle / 'Contents').mkdir(parents=True)
            info = bundle / 'Contents/Info.plist'
            original = plistlib.dumps({'CFBundleIdentifier': 'example.unrelated.audio'})
            info.write_bytes(original)
            result = self.guard(directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(info.read_bytes(), original)

    def test_existing_build_output_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / 'marker'; marker.write_text('preserved')
            with self.assertRaisesRegex(ValueError, 'Output already exists'):
                builder.build(Path(directory))
            self.assertEqual(marker.read_text(), 'preserved')

    def test_payload_manifest_rejects_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'linked').symlink_to('/tmp')
            with self.assertRaisesRegex(ValueError, 'Symlink'):
                builder.files(Path(directory))

    def test_archive_inspection_rejects_extra_payload_and_mutated_driver(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle = root / 'staged'; bundle.mkdir()
            (bundle / 'driver').write_bytes(b'original-driver')
            expanded = root / 'expanded'; (expanded / 'Scripts').mkdir(parents=True)
            guard = b'#!/bin/sh\nexit 0\n'
            (expanded / 'Scripts/preinstall').write_bytes(guard)
            payload = expanded / 'Payload' / builder.PAYLOAD_BUNDLE; payload.mkdir(parents=True)
            (payload / 'driver').write_bytes(b'original-driver')
            xml = (f'<pkg-info identifier="{builder.PACKAGE_ID}" install-location="{builder.INSTALL_LOCATION}" '
                   f'postinstall-action="none" relocatable="false"><bundle id="{builder.BUNDLE_ID}"/>'
                   '<scripts><preinstall file="./preinstall"/></scripts></pkg-info>')
            (expanded / 'PackageInfo').write_text(xml)
            self.assertEqual(builder.inspect_expanded(expanded, bundle, guard)['onlyDestination'], '/' + str(builder.DESTINATION))
            (expanded / 'Payload/foreign').write_text('unexpected')
            with self.assertRaisesRegex(ValueError, 'outside'):
                builder.inspect_expanded(expanded, bundle, guard)
            (expanded / 'Payload/foreign').unlink()
            (payload / 'driver').write_bytes(b'changed-driver')
            with self.assertRaisesRegex(ValueError, 'driver bytes differ'):
                builder.inspect_expanded(expanded, bundle, guard)
            (payload / 'driver').write_bytes(b'original-driver')
            (expanded / 'PackageInfo').write_text(xml.replace('<preinstall file="./preinstall"/>', '<postinstall file="./postinstall"/>'))
            with self.assertRaisesRegex(ValueError, 'only its preinstall'):
                builder.inspect_expanded(expanded, bundle, guard)


if __name__ == '__main__':
    unittest.main()
