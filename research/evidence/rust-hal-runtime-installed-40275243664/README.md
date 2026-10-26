# Original CI observation at 23c7ed6: runtime continuity failure retained

At exact source `23c7ed6302bcb21ae99452487f137b50301254a3`, original
[installed run 40275243664](https://github.com/thatsmethen/relink/actions/runs/40275243664)
**failed** on 26 October 2026: `installed-hal` passed and
`installed-runtime-capture` failed its independent recorder continuity check.
Both cleanup, manifest, console-summary and artifact-upload paths completed.
The separate [desktop run 40275243677](https://github.com/thatsmethen/relink/actions/runs/40275243677)
passed on macOS 15 and Windows 2022.

This folder preserves the original outcomes. No job was rerun or dispatched; no
local audio device, recording, installation or service operation occurred. Prior
observations `40270207703` and `40272244069` remain unchanged. The tested runtime
probe changes send gain and mute only: it does **not** exercise edits to the new
DSP effects, Sound Check, multiple runtime outputs, application capture or Windows
audio routing. Passing builds do not establish those behaviors or production quality.

## Original failure and its bounded context

The unchanged analyzer reports one failure: `independent recorder clock/continuity
failed`. In the 32-second, 48 kHz recorder, callback 2987 starts at sample
1,586,608 and contains 512 frames. Callback 2988 starts at 1,587,708, leaving
**588 excess timeline frames** between the expected and actual next timestamp.
The next callback is 31.88425 seconds after the first recorder native timestamp,
8.9364125 seconds after the runtime stop request. The stored PCM remains the
original concatenated callback data; no missing timeline samples were filled.

The diagnostic record places a device processor-overload (`over`) notification
166.75 microseconds before that next native timestamp. The preceding callback
entered 22.415125 ms after its native timestamp and 11.2985 ms after the OS-current
timestamp; its recorded callback work lasted 3.208 microseconds. The next native
timestamps are 22.916667 ms apart, while the corresponding observed callback
timestamps are 11.430292 ms apart. These are correlations, not proof that a
particular driver, observer, scheduler or DSP component caused the gap.

All four fixed gain/mute phase checks passed. Both unmuted checks had zero
forbidden silence runs; mute and stopped-tail peak were exactly zero. The 2,048
stored frames on each side of this late timestamp gap were also exactly silent.
The runtime result reports proved native output shutdown, and all 340 recorded
native telemetry records had no fault/error. These findings do not excuse the
independent continuity failure or turn the run into a pass. Both cable UIDs were
absent in the successful final cleanup.

`diagnosis.json` contains exact adjacent CSV rows, original diagnostic timing
records, the nearby notification and source-file hashes. `derive_diagnosis.py`
reproduces that read-only context separately from acceptance.

## Custody and unchanged re-analysis

The original archive responses are retained once under `build/ci-40275243664/`,
with one extraction under `extracted/`. Both sizes and SHA-256 values match the
GitHub artifact API:

| Archive | Artifact ID | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `hal.zip` | 12630181398 | 10,536,205 | `d5da927cf0b6f3b8b477563bbfa2b9096df4b67ba2655184be77f0c2e9ef67f0` |
| `runtime.zip` | 12629449730 | 6,958,055 | `d08bb1bb171f7be78c244eb8cc2546317d7099e3f7d98483a8904d2b41282ae2` |

All **121** non-manifest members match their original source-bound manifests
(65 HAL, 56 runtime). The frozen analyzers, reader-comparison function and
protocol sources in `source/` are exact Git bytes from the observed revision.
All **13** recomputed results exactly match their original results, excluding
only relocated `source_report` paths: **12 passed and the runtime PCM/clock
result failed**. Reproduction equality is enforced by `verify.py`; its
`custody_and_reproduction_verified` result does not mean acceptance passed.
Five offline comparison controls include rejecting a relabeling of the original
failure as success.

`original/` holds 26 byte-for-byte compact report/event copies, mapped to verified
archive members by `compact-originals.json`. The original run/job/artifact API
responses are preserved without rewriting. `derived-verification.json` is the
separate local result. Archived observer binaries match their CI build records;
they were not executed locally. The installed driver bundle itself is absent
from these artifacts, so its build-record digest is reported metadata rather
than an independently rehashed installed payload.

The exact original archives are durably published in the
[evidence prerelease](https://github.com/thatsmethen/relink/releases/tag/evidence-23c7ed6-40275243664):
[hal.zip](https://github.com/thatsmethen/relink/releases/download/evidence-23c7ed6-40275243664/hal.zip)
and [runtime.zip](https://github.com/thatsmethen/relink/releases/download/evidence-23c7ed6-40275243664/runtime.zip).
Its lightweight tag points exactly to `23c7ed6302bcb21ae99452487f137b50301254a3`;
it is a prerelease created with `latest=false`, separate from production releases.
Both public asset URLs were downloaded anonymously to fresh local paths. Their
full byte lengths and SHA-256 values match the unchanged original archives and
GitHub's uploaded-asset digests. `publication.json` records the release/tag/asset
identities, original custody verification and independent download checks;
`release-notes.md` preserves the exact published notes. Publication retains the
failed runtime observation and makes no new native or DSP qualification claim.
The original Actions artifacts expire on **25 November 2026**; these separate
release assets preserve their exact ZIP bytes beyond that retention window.
The source tag predates this later custody documentation. `collection.json`
retains the original acquisition-time record; this later publication is recorded
separately in `publication.json`.

## Console fallback preserved the failure

The new summary ran after acceptance and manifest creation and before artifact
upload. Runtime acceptance exited 1, the summary exited successfully, and the
job remained failed. The frozen workflow retains `if: always()` and
`continue-on-error: true` for the diagnostic step. Its frozen source opens
allowlisted reports read-only and does not invoke analyzers or read PCM.

The original fallback contained 10,081 bytes for HAL and 12,305 bytes for runtime
(largest lines 2,492 and 3,623 bytes), within the 48 KiB total and 8 KiB line limits.
It preserved the runtime failure, the exact 588-frame gap, successful cleanup,
runtime shutdown and report hashes. Every complete report hash and the bounded
native-telemetry sample hash match the uploaded originals. Missing reports and
partial telemetry were explicit; the summary did not claim that its sample
proved absence of unsampled faults. All manifest members still match after the
summary, supporting that it did not modify the original evidence.

`original-logs/` contains byte-exact selected step logs. `log-custody.json` binds
them to the downloaded original log ZIPs and records local archive hashes;
GitHub does not supply an API digest for those log responses. The full log ZIPs
remain under `build/ci-40275243664/`.

## Desktop results and offline commands

The original deterministic-test logs report **311 passed** on macOS and **277
passed** on Windows, each across 13 result groups with zero failed/ignored tests.
Both application builds and uploads succeeded. The macOS private-process ABI
harness and Windows embedded-icon resource check passed. Desktop package assets
were not downloaded or executed in this task; the icon check does not establish
Windows shell rendering, and desktop CI does not establish native audio routing.

```sh
python3 research/evidence/rust-hal-runtime-installed-40275243664/verify.py \
  --archives build/ci-40275243664 \
  --extracted build/ci-40275243664/extracted
python3 research/evidence/rust-hal-runtime-installed-40275243664/derive_diagnosis.py \
  --artifacts build/ci-40275243664/extracted \
  --logs build/ci-40275243664/logs.zip
python3 -m unittest discover \
  -s research/evidence/rust-hal-runtime-installed-40275243664 -p test_verifier.py
```

These commands analyze downloaded generated CI data only. The verifier refuses
an altered existing extraction rather than repairing it. Both tools write
derived JSON to stdout and preserve the original failed outcome.
