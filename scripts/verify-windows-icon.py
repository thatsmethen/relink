#!/usr/bin/env python3
"""Verify the built Windows PE icon bytes against ReLink's checked-in ICO/RES.

Python standard library only. Native verification maps the executable as resource
data; it never starts the application or invokes its entry point. This verifies
embedded resources, not Explorer/taskbar rendering. --self-test runs portably.

Contracts: Microsoft Learn /windows/win32/menurc/{resourceheader,newheader,resdir}
and /windows/win32/api/libloaderapi/{nf-libloaderapi-loadlibraryexw,
nf-libloaderapi-enumresourcenamesexw,nf-libloaderapi-enumresourcelanguagesexw,
nf-libloaderapi-findresourceexw}. No third-party PE parser is used.
"""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import struct
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
MAX_SOURCE_BYTES = 4 * 1024 * 1024
SIZES = (16, 24, 32, 48, 64, 128, 256)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_bounded(path):
    with path.open("rb") as source:
        data = source.read(MAX_SOURCE_BYTES + 1)
    if len(data) > MAX_SOURCE_BYTES:
        raise ValueError(f"icon source exceeds {MAX_SOURCE_BYTES} bytes: {path.name}")
    return data


def expected_resources(ico):
    """Derive RT_ICON/RT_GROUP_ICON records from the checked-in ICO directory."""
    if len(ico) < 6 or struct.unpack_from("<HHH", ico) != (0, 1, len(SIZES)):
        raise ValueError("ICO must contain the seven expected icon resolutions")
    table_end = 6 + 16 * len(SIZES)
    if len(ico) < table_end:
        raise ValueError("truncated ICO directory")
    result, group, next_offset = {}, [ico[:6]], table_end
    for i, dimension in enumerate(SIZES):
        w, h, colors, reserved, planes, bits, size, offset = struct.unpack_from("<BBBBHHII", ico, 6 + 16 * i)
        if (w or 256, h or 256, colors, reserved, planes, bits) != (dimension, dimension, 0, 0, 1, 32):
            raise ValueError(f"unexpected ICO metadata at image {i + 1}")
        if offset != next_offset or size < 33 or offset + size > len(ico):
            raise ValueError(f"invalid ICO payload span at image {i + 1}")
        data = ico[offset:offset + size]
        if data[:16] != b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR" or struct.unpack_from(">II", data, 16) != (dimension, dimension):
            raise ValueError(f"ICO payload dimensions/type mismatch at image {i + 1}")
        result[3, i + 1, 0] = data
        group.append(struct.pack("<BBBBHHIH", w, h, colors, reserved, planes, bits, size, i + 1))
        next_offset = offset + size
    if next_offset != len(ico):
        raise ValueError("unexpected trailing ICO bytes")
    result[14, 1, 0] = b"".join(group)
    return result


def res_resources(data):
    """Read this project's fixed ordinal-name .res records, rejecting extras."""
    offset, result, null_seen = 0, {}, False
    while offset < len(data):
        if len(data) - offset < 32:
            raise ValueError("truncated resource header")
        size, header, tm, kind, nm, name, version, flags, language, version2, characteristics = struct.unpack_from("<IIHHHHIHHII", data, offset)
        if header != 32 or tm != 0xffff or nm != 0xffff or version or version2 or characteristics:
            raise ValueError("unsupported resource header")
        end = offset + header + size
        padded_end = (end + 3) & ~3
        if padded_end > len(data) or any(data[end:padded_end]):
            raise ValueError("truncated or nonzero resource padding")
        payload = data[offset + header:end]
        if kind == name == size == 0 and offset == 0 and flags == language == 0:
            null_seen = True
        elif kind in (3, 14) and name > 0 and flags == 0x1030:
            key = kind, name, language
            if key in result:
                raise ValueError("duplicate resource entry")
            result[key] = payload
        else:
            raise ValueError("unexpected resource entry")
        offset = padded_end
    if not null_seen:
        raise ValueError("missing null resource header")
    return result


def verify_equal(expected, actual):
    if set(expected) != set(actual):
        raise ValueError(f"icon resource keys differ: missing={sorted(set(expected) - set(actual))}, extra={sorted(set(actual) - set(expected))}")
    mismatches = [key for key in expected if expected[key] != actual[key]]
    if mismatches:
        raise ValueError(f"icon resource bytes differ: {mismatches}")


def native_resources(executable):
    if sys.platform != "win32":
        raise OSError("native PE resource verification requires Windows")
    import ctypes as c
    from ctypes import wintypes as w

    kernel = c.WinDLL("kernel32", use_last_error=True)
    pointer, signed_pointer = c.c_void_p, c.c_ssize_t
    name_callback = c.WINFUNCTYPE(w.BOOL, w.HMODULE, pointer, pointer, signed_pointer)
    lang_callback = c.WINFUNCTYPE(w.BOOL, w.HMODULE, pointer, pointer, w.WORD, signed_pointer)
    kernel.LoadLibraryExW.argtypes = [w.LPCWSTR, w.HANDLE, w.DWORD]
    kernel.LoadLibraryExW.restype = w.HMODULE
    kernel.FreeLibrary.argtypes = [w.HMODULE]
    kernel.FreeLibrary.restype = w.BOOL
    kernel.EnumResourceNamesExW.argtypes = [w.HMODULE, pointer, name_callback, signed_pointer, w.DWORD, w.WORD]
    kernel.EnumResourceNamesExW.restype = w.BOOL
    kernel.EnumResourceLanguagesExW.argtypes = [w.HMODULE, pointer, pointer, lang_callback, signed_pointer, w.DWORD, w.WORD]
    kernel.EnumResourceLanguagesExW.restype = w.BOOL
    kernel.FindResourceExW.argtypes = [w.HMODULE, pointer, pointer, w.WORD]
    kernel.FindResourceExW.restype = pointer
    kernel.SizeofResource.argtypes = [w.HMODULE, pointer]
    kernel.SizeofResource.restype = w.DWORD
    kernel.LoadResource.argtypes = [w.HMODULE, pointer]
    kernel.LoadResource.restype = pointer
    kernel.LockResource.argtypes = [pointer]
    kernel.LockResource.restype = pointer

    # DATAFILE_EXCLUSIVE | IMAGE_RESOURCE: no executable initialization/imports;
    # deny concurrent writers while mapped. Never use ordinary LoadLibrary.
    module = kernel.LoadLibraryExW(str(executable.resolve()), None, 0x40 | 0x20)
    if not module:
        raise c.WinError(c.get_last_error())
    result = {}
    try:
        for kind in (3, 14):
            names, errors = [], []

            @name_callback
            def collect_name(_module, _kind, name, _parameter):
                # All our resources use integer ordinals; do not dereference
                # native string-name pointers, and never raise through ctypes.
                if not name or name > 0xffff or len(names) >= 256:
                    errors.append("unexpected named/oversized icon resource set")
                    return 0
                names.append(name)
                return 1

            # RESOURCE_ENUM_LN | VALIDATE searches only the mapped executable,
            # never an adjacent/localized .mui file or a user-language fallback.
            if not kernel.EnumResourceNamesExW(module, kind, collect_name, 0, 1 | 8, 0):
                raise ValueError(errors[0]) if errors else c.WinError(c.get_last_error())
            if errors or len(names) != len(set(names)):
                raise ValueError("invalid/duplicate native icon resource names")
            for name in names:
                languages = []

                @lang_callback
                def collect_language(_module, _kind, _name, language, _parameter):
                    if len(languages) >= 256:
                        errors.append("oversized icon language set")
                        return 0
                    languages.append(language)
                    return 1

                if not kernel.EnumResourceLanguagesExW(module, kind, name, collect_language, 0, 1 | 8, 0):
                    raise ValueError(errors[0]) if errors else c.WinError(c.get_last_error())
                if languages != [0]:
                    raise ValueError(f"icon resource must be exactly language-neutral: {(kind, name, languages)}")
                resource = kernel.FindResourceExW(module, kind, name, 0)
                if not resource:
                    raise c.WinError(c.get_last_error())
                size = kernel.SizeofResource(module, resource)
                if not 0 < size <= MAX_SOURCE_BYTES:
                    raise ValueError("native icon resource has invalid/bounded size")
                loaded = kernel.LoadResource(module, resource)
                if not loaded:
                    raise c.WinError(c.get_last_error())
                address = kernel.LockResource(loaded)
                if not address:
                    raise c.WinError(c.get_last_error())
                result[kind, name, 0] = c.string_at(address, size)
    finally:
        if not kernel.FreeLibrary(module):
            raise c.WinError(c.get_last_error())
    return result


def self_test():
    class Controls(unittest.TestCase):
        def setUp(self):
            self.ico = read_bounded(ROOT / "packaging/branding/relink.ico")
            self.res = read_bounded(ROOT / "packaging/branding/relink.res")
            self.expected = expected_resources(self.ico)

        def test_checked_in_sources_match(self):
            verify_equal(self.expected, res_resources(self.res))

        def test_changed_payload_and_missing_group_fail(self):
            changed = dict(self.expected)
            changed[3, 1, 0] = changed[3, 1, 0][:-1] + bytes([changed[3, 1, 0][-1] ^ 1])
            with self.assertRaises(ValueError):
                verify_equal(self.expected, changed)
            del changed[14, 1, 0]
            with self.assertRaises(ValueError):
                verify_equal(self.expected, changed)

        def test_incorrect_language_or_extra_image_fails(self):
            changed = dict(self.expected)
            changed[3, 1, 1033] = changed[3, 1, 0]
            with self.assertRaises(ValueError):
                verify_equal(self.expected, changed)

        def test_truncated_and_invalid_source_spans_fail(self):
            for data in (self.ico[:5], self.ico[:-1], self.ico + b"x"):
                with self.assertRaises(ValueError):
                    expected_resources(data)
            damaged = bytearray(self.ico)
            struct.pack_into("<I", damaged, 18, 0xffffffff)
            with self.assertRaises(ValueError):
                expected_resources(damaged)
            for data in (self.res[:31], self.res[:-1], self.res + self.res[32:]):
                with self.assertRaises(ValueError):
                    res_resources(data)

    return 0 if unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Controls)).wasSuccessful() else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path, nargs="?")
    parser.add_argument("--icon", type=Path, default=ROOT / "packaging/branding/relink.ico")
    parser.add_argument("--resource", type=Path, default=ROOT / "packaging/branding/relink.res")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if args.executable is None:
        parser.error("executable is required except with --self-test")
    report = {"status": "unavailable", "checked_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "method": "native Windows resource-only mapping and exact RT_ICON/RT_GROUP_ICON byte comparison",
              "executable_started": False, "shell_rendering_verified": False,
              "script_sha256": digest(Path(__file__).read_bytes())}
    code = 2
    try:
        ico, res = read_bounded(args.icon), read_bounded(args.resource)
        expected = expected_resources(ico)
        verify_equal(expected, res_resources(res))
        before = digest(args.executable.read_bytes())
        actual = native_resources(args.executable)
        after = digest(args.executable.read_bytes())
        if before != after:
            raise ValueError("executable changed during verification")
        verify_equal(expected, actual)
        report.update(status="verified", executable_sha256=before, ico_sha256=digest(ico), res_sha256=digest(res),
                      resolutions=list(SIZES), resources=[{"type": k[0], "id": k[1], "language": k[2],
                      "size": len(actual[k]), "sha256": digest(actual[k])} for k in sorted(actual)])
        code = 0
    except (OSError, ValueError, struct.error) as error:
        report["error"] = str(error)
        if isinstance(error, (ValueError, struct.error)):
            report["status"], code = "failed", 1
    serialized = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(serialized, encoding="utf-8")
    print(serialized, end="")
    return code


if __name__ == "__main__":
    sys.exit(main())
