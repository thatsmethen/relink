# Public preservation of native macOS output observation

The [evidence release](https://github.com/thatsmethen/relink/releases/tag/evidence-native-output-40249452671) is public. Anonymous download and independent
verification completed at **2026-10-26T01:02:15.374201+00:00**. The original result is
**HAL PASS, native capture/output runtime PASS, overall PASS** for one short hosted
macOS virtual-cable run at `f47df76d100da25c5e98d261d92792a1d507d5f1`.

This pass does not establish production quality. The original unmuted phases
retained approximately **1,215, 926 and 237 ppm** frequency-error magnitudes under
the frozen **2,500 ppm** criterion. Pitch settling, long-term drift, physical
hardware, sustained load, multi-output operation and installed Windows audio
remain unqualified. Native callback progress describes prepared software buffers;
the independent recorder provides the observed output evidence. Input cleanup
semantics are those of the original source, not subsequent lifecycle changes.

All **39 public assets** matched their expected byte counts and SHA-256 values
and GitHub's release digests. Whole uploads of the two large originals reached
their single 90-second bound, so their bytes are distributed as **35 ordered
512 KiB-or-smaller parts**. Concatenation reconstructed both original ZIPs exactly,
matching their original GitHub artifact digests. The original workflow-log ZIP
and frozen collection snapshot are supplied whole. No accepted asset was
replaced; only the two unaccepted, digestless whole-upload starters were removed.

The unchanged bundled verifier checked all **65 HAL plus 56 runtime** manifest
files, exact archive membership, compact original text, terminal lifecycle and
cleanup reports. It loaded the two analyzer modules from the explicitly supplied
local Git clone at the exact source commit and recomputed the entire original PCM
analysis, matching the original report exactly. All **115 frozen collection
files**, including README, collector, original verifier and derived report, remain
byte-for-byte unchanged. `collection-SHA256SUMS.json` records that snapshot.

Download all release assets into one directory and run:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD" --source-repo /path/to/relink-clone
```

The clone must already contain the source commit. The wrapper does not fetch or
check out code. The original verifier temporarily extracts the original
PCM/clock/control inputs for offline recomputation and removes its temporary
directory; it never opens an audio device or reruns the experiment.

From this repository evidence folder, independent anonymous download verification
can be repeated with:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-public.py --downloads /path/to/new-directory --source-repo /path/to/relink-clone
```

`public-verification.json` contains the public download and recomputation proof;
`publication-state.json` records the later state separately from collection-time
metadata. `publication-upload.json` records authenticated upload metadata and is
not used as a substitute for public download verification. A local wrapper control
also verified artifacts outside the checkout and rejected a one-byte corruption.
The original collection's `no_rerun_no_publication` flag describes its collection
time and remains unchanged. Earlier failed runs remain separate evidence.
