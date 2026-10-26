"""Fixed identities for the original synthetic application-capture fixture."""
from __future__ import annotations
import json
from pathlib import Path

PROTOCOL = "relink-native-application-tones-v1"
SOURCE_SECONDS = 60
RECORD_SECONDS = 36
STARTUP_SECONDS = 20.0
SELECTED_ID = "au.textual.relink.fixture.selected"
EXCLUDED_ID = "au.textual.relink.fixture.excluded"
PROBE_ID = "au.textual.relink.fixture.capture"
SELECTED_BUNDLE = "ReLinkToneFixture.app"
EXCLUDED_BUNDLE = "ReLinkExcludedFixture.app"
PROBE_BUNDLE = "ReLinkApplicationProbe.app"
CABLE1 = "au.textual.ravelink.cable.1"
CABLE2 = "au.textual.ravelink.cable.2"
TWO_PROCESS_FORMAT = "private stereo process tap; 2 current bundle audio processes; restart on membership change"
WRITERS = (
    ("selected-left", SELECTED_BUNDLE, SELECTED_ID, 440.0, 997.0, 0.125, 0.0),
    ("selected-right", SELECTED_BUNDLE, SELECTED_ID, 440.0, 997.0, 0.0, 0.125),
    ("excluded", EXCLUDED_BUNDLE, EXCLUDED_ID, 1733.0, 2371.0, 0.125, 0.125),
)


def selector_matches(value: str, bundle: str) -> bool:
    """Validate the persisted selector, never substitute a hardware identity."""
    try:
        path = Path(bundle)
        if not path.is_absolute() or path.name != SELECTED_BUNDLE or str(path) != bundle or ".." in path.parts:
            return False
        if not isinstance(value, str) or not value.startswith("relink-app-v1:"):
            return False
        # Reject duplicate JSON keys as well as unexpected selector fields.
        def pairs(items):
            result = {}
            for key, item in items:
                if key in result:
                    raise ValueError("duplicate selector field")
                result[key] = item
            return result
        parsed = json.loads(value[len("relink-app-v1:"):], object_pairs_hook=pairs)
        return parsed == {"type": "mac_bundle", "bundle_id": SELECTED_ID, "path": bundle}
    except (ValueError, TypeError):
        return False


def validate_owned_identities(records: list[dict], selected_bundle: str) -> None:
    """Offline check of only the three spawned children; no process inventory."""
    selected = Path(selected_bundle)
    if (not isinstance(selected_bundle, str) or not selected.is_absolute()
            or selected.name != SELECTED_BUNDLE or str(selected) != selected_bundle
            or ".." in selected.parts):
        raise ValueError("selected bundle must be canonical absolute lexical path")
    if len(records) != 3 or {r.get("name") for r in records} != {w[0] for w in WRITERS}:
        raise ValueError("expected exactly three owned generator identities")
    expected_parent = Path(selected_bundle).parent
    pids = set()
    for name, bundle_name, bundle_id, *_ in WRITERS:
        record = next(r for r in records if r.get("name") == name)
        bundle = expected_parent / bundle_name
        executable = bundle / "Contents/MacOS" / bundle.stem
        if (record.get("bundle") != str(bundle) or record.get("bundle_id") != bundle_id
                or record.get("executable") != str(executable)
                or type(record.get("pid")) is not int or record["pid"] <= 0
                or type(record.get("birth_microseconds")) is not int or record["birth_microseconds"] <= 0
                or record["pid"] in pids
                or len(record.get("executable_sha256", "")) != 64
                or any(c not in "0123456789abcdef" for c in record.get("executable_sha256", ""))):
            raise ValueError(f"invalid owned process identity: {name}")
        pids.add(record["pid"])
    selected = [r for r in records if r["bundle_id"] == SELECTED_ID]
    if selected[0]["executable_sha256"] != selected[1]["executable_sha256"]:
        raise ValueError("selected instances were not the same frozen executable")
