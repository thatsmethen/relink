#!/usr/bin/env python3
"""Reconstruct lossless original archives and run the unchanged offline verifier.

All analyzer source is pinned inside the collection snapshot. No Git, source
checkout, network or audio operations are performed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import zipfile

SOURCE = "902118a60b070bb9cefdef75d5b477b5350b9847"
TREE = "3487df8646e06526b536e12450be71536172da36"
TAG = "evidence-multioutput-single-40254943736"


def digest(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def name(value):
    assert isinstance(value, str) and Path(value).name == value and value not in ("", ".", "..")
    return value


def checked(path, expected):
    data = path.read_bytes()
    assert digest(data) == {key: expected[key] for key in ("bytes", "sha256")}, path.name
    return data


def write_once(path, data):
    if path.exists():
        assert path.read_bytes() == data, f"refusing to replace different bytes: {path}"
    else:
        with path.open("xb") as file:
            file.write(data)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--downloads", type=Path, default=Path(__file__).resolve().parent)
    folder = parser.parse_args().downloads.resolve()
    manifest = json.loads((folder / "publication-assets.json").read_text())
    assert manifest["source_revision"] == SOURCE and manifest["source_tree"] == TREE and manifest["tag"] == TAG
    assets = {item["name"]: item for item in manifest["assets"]}
    assert len(assets) == len(manifest["assets"])
    for item in assets.values():
        checked(folder / name(item["name"]), item)
    used = set()
    archives = []
    for archive in manifest["archives"]:
        archive_name = name(archive["name"])
        if archive["parts"]:
            chunks = []
            offset = 0
            for index, part in enumerate(archive["parts"]):
                part_name = name(part["name"])
                assert part_name == f"{archive_name}.part{index:03}" and part_name not in used
                assert part["index"] == index and part["offset"] == offset
                data = checked(folder / part_name, part)
                assert digest(data) == {k: assets[part_name][k] for k in ("bytes", "sha256")}
                assert 0 < len(data) <= 524288
                if index + 1 < len(archive["parts"]):
                    assert len(data) == 524288
                chunks.append(data)
                offset += len(data)
                used.add(part_name)
            data = b"".join(chunks)
            assert digest(data) == {k: archive[k] for k in ("bytes", "sha256")}
            write_once(folder / archive_name, data)
        else:
            assert archive_name not in used
            checked(folder / archive_name, archive)
            used.add(archive_name)
        with zipfile.ZipFile(folder / archive_name) as content:
            assert content.testzip() is None
        archives.append({"name": archive_name, **digest((folder / archive_name).read_bytes())})
    assert used == set(assets)
    compact = folder / "collection-evidence"
    compact.mkdir(exist_ok=True)
    with zipfile.ZipFile(folder / "collection-evidence-40254943736.zip") as archive:
        hashes = json.loads(archive.read("SHA256SUMS.json"))
        assert len(hashes) == 126
        assert set(archive.namelist()) == {*hashes, "SHA256SUMS.json"}
        assert len(set(archive.namelist())) == len(archive.namelist())
        for member in archive.namelist():
            relative = PurePosixPath(member)
            assert not relative.is_absolute() and ".." not in relative.parts
            data = archive.read(member)
            if member in hashes:
                assert digest(data) == hashes[member], member
            destination = compact / member
            destination.parent.mkdir(parents=True, exist_ok=True)
            write_once(destination, data)
    result = subprocess.run([sys.executable, str(compact / "verify.py"), "--archives", str(folder)],
        check=True, capture_output=True, text=True, encoding="utf-8", errors="strict",
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"))
    verification = json.loads(result.stdout)
    assert verification["original_manifest_files_verified"] == 121
    assert verification["original_analysis_reports_recomputed"] == 13
    assert verification["outcomes"] == {"overall": "FAIL", "hal": "FAIL", "runtime": "FAIL"}
    print(json.dumps({"schema": 1, "all_release_payload_hashes_verified": True,
        "complete_original_archive_hashes_verified": True, "archives": archives,
        "source_revision": SOURCE, "source_tree": TREE, "frozen_collection_files_verified": len(hashes),
        "unchanged_bundled_verifier": verification,
        "pinned_source_identities": json.loads((compact / "pinned-source-identities.json").read_text()),
        "original_verifier_modified": False, "original_outcomes_changed": False,
        "git_network_or_audio_operations": False,
        "temporary_pcm_extraction": "The frozen verifier extracts original PCM/clock/control inputs into a temporary directory for offline analysis and removes it afterward."}, indent=2))


if __name__ == "__main__":
    main()
