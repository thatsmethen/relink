# Original installed HAL/runtime observation 40272244069

[Run 40272244069, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40272244069)
completed successfully on 26 October 2026 at source
`b864bc6430b6ccc5b6b861395a2071be29c0a096`. Both `installed-hal` and
`installed-runtime-capture` passed, including their cleanup and artifact-upload
steps. This record independently checks those original completed observations;
no job was rerun or dispatched, and no local audio device or recording was used.

The runner environment was macOS 15.7.9 arm64, SDK 15.5 and Rust 1.91.1. These
results qualify only the generated-tone protocols and exact source below. They
do not qualify later working-tree DSP changes, application capture, Windows
routing, physical clock drift, sleep/wake, long workloads or production release.
The failed run `40270207703`, including its missing runtime artifact, remains
unchanged and is not superseded by this successful observation.

## Custody and independent checks

The exact original ZIP responses are retained once under
`build/ci-40272244069/`. Their byte lengths and SHA-256 values match the GitHub
artifact API. Every non-manifest member matches the corresponding source-bound
custody manifest: **65 HAL files** and **56 runtime files**, with no missing or
unexpected files. Safe extraction was performed once under the same build
directory; later verification reuses and hashes that extraction.

| Artifact | GitHub artifact ID | Archive bytes | SHA-256 |
| --- | --- | ---: | --- |
| HAL | 12628639271 | 10,535,660 | `3ad0dc51e356764e455396352f41ffe339d43339e38d3f46fea34b593820c5c5` |
| Runtime | 12628504402 | 7,180,935 | `b180cfd0ebff7048b7b2b3fe1d57e2863d56ee6283ef6770a97420aa4462329a` |

The unchanged analyzers and the independent-reader comparison function were
loaded from exact `git show` bytes at the observed commit, preserved in `source/`.
All **13** PCM, reader-comparison and diagnostic results passed again and exactly
matched their original JSON results, excluding only the `source_report` pathname
that necessarily identifies the new local extraction. No samples, timestamps,
gains, thresholds or original JSON values were modified. The frozen runner/analyzer
files that appear in CI's build-hash records also match those recorded hashes.
The archived independent observer binary matches its build record; it was not
executed locally.

`original/` contains 24 byte-for-byte compact original report/metadata copies
(67,092 bytes), individually bound to archive members by `compact-originals.json`.
`original-run.json`, `original-jobs.json` and `original-artifacts.json` are original
API response bytes. `derived-verification.json` contains the separate local
re-analysis and custody results. `source-custody.json` identifies frozen source
bytes, and `collection.json` records acquisition scope.

## Observed envelope

- **HAL:** two simultaneous 20-second stereo readers, 960,000 frames each, at
  48 kHz. Their 753,448-frame common comparison interval had zero differing
  samples above the unchanged `1e-6` comparison threshold and maximum difference
  exactly zero. Both continuous-tone analyses passed. A fresh 3-second capture
  after writer stop contained 144,000 frames with peak exactly zero.
- **Runtime:** generated Cable 1 input passed through the actual mixer into
  Cable 2 and an independent 32-second recorder. All four fixed gain/mute phases
  passed. The recorder had 3,000 callbacks, no sample gaps or host-time reversals,
  and measured 48,000 frames/second. Both unmuted intervals had zero forbidden
  zero runs; the mute interval and stopped tail had peak exactly zero.
- The fixed detector found the gain change at 30.001 ms, mute at 35.006 ms,
  unmute at 30.004 ms and stop at 24.990 ms. These are this protocol's observed
  envelope-transition detections, not general end-to-end latency guarantees.
  The initial already-established gain phase's negative detector result is
  retained in the original report and is not presented as response latency.
- Polled render throughput was 47,992.660 frames/second. The 228 native telemetry
  records contained no runtime fault/error. The final record reports stopped
  state and proved native output shutdown; its lifecycle records show successful
  stop/destroy/listener-removal returns and zero callbacks at quiescence/owner
  completion. This telemetry summary is separate from the independent PCM checks.
- Both cleanup reports independently discovered **zero matches for both cable
  UIDs**, with successful, complete discovery after removing only the owned driver.

## Reproduce the offline verification

The raw ZIPs and their single extracted copies are intentionally kept out of this
compact evidence directory. The unchanged archives are durably published in the
[evidence prerelease](https://github.com/thatsmethen/relink/releases/tag/evidence-b864bc6-40272244069):
[hal.zip](https://github.com/thatsmethen/relink/releases/download/evidence-b864bc6-40272244069/hal.zip)
and [runtime.zip](https://github.com/thatsmethen/relink/releases/download/evidence-b864bc6-40272244069/runtime.zip).
Its lightweight tag points exactly to `b864bc6430b6ccc5b6b861395a2071be29c0a096`;
it is a prerelease created with `latest=false`, separate from production releases.
Both public asset URLs were downloaded anonymously to fresh local paths and their
full byte lengths and SHA-256 values matched the original archives and GitHub's
uploaded-asset digests. `publication.json` records asset IDs, URLs and this check.
GitHub Actions currently expires the original run artifacts on **25 November
2026**; these independent release assets preserve the exact ZIPs beyond that
artifact retention window. The source tag predates this later custody report.

With the downloaded original ZIPs available:

```sh
python3 research/evidence/rust-hal-runtime-installed-40272244069/verify.py \
  --archives build/ci-40272244069 \
  --extracted build/ci-40272244069/extracted
```

The verifier hashes existing extracted files and refuses mismatches. For a fresh
extraction, choose a new `--extracted` directory. It never repairs or overwrites an
existing extraction. Reproduction equality is an enforced gate; three offline
controls check source-path relocation and rejection of altered or missing numeric
observations (`python3 -m unittest discover -s research/evidence/rust-hal-runtime-installed-40272244069 -p test_verifier.py`). Its JSON output is derived verification, not an original CI
report. The driver SHA in CI's build records is explicitly reported metadata:
the installed driver bundle itself is not present in these artifacts, so this
record does not independently re-hash that installed bundle.
