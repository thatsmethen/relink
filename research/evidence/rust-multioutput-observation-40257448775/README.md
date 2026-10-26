# Original native two-output observation — 2026-10-26

**PASS for this fixed, short, two-virtual-output protocol.** The first and only
attempt of [workflow run 40257448775](https://github.com/thatsmethen/relink/actions/runs/40257448775)
used source `69f49da28a9bae16dec9d4bc29bd0d639fb73a08` (tree
`e761d228a3f0998b9addc97c11a4fd3774b90545`). The fixed acceptance analysis, both
independent diagnostic reports, runtime result, and owned-driver cleanup pass.
Offline recomputation from the original PCM and exact pinned analyzers matches
**every original analysis field, with no exclusions or changed thresholds**.
No acquisition was repeated. Older failed observations remain unchanged.

## What was measured

One macOS 15.7.9 arm64 CI host ran the original Rust native runtime against two
original HAL cable outputs at 48 kHz. Cable 1 was the follower with a 997 Hz
internal tone; Cable 2 was the clock with a distinct 1543 Hz internal tone. Both
stereo channels carried identical samples. The fixed five phases held for
4/4/3/3/4 seconds and independently changed route gain, mute, restore and Stop.
Both independent native recorders ran for the complete fixed 32 seconds.

| Measured result | Follower / Cable 1 | Clock / Cable 2 |
| --- | ---: | ---: |
| Recorder callbacks | 3,000 | 3,000 |
| Recorded stereo frames | 1,536,000 | 1,536,000 |
| Sample-clock gaps / host reversals | 0 / 0 | 0 / 0 |
| Maximum steady amplitude error | 0.001525% | 0.001710% |
| Maximum estimated frequency error | 7.912 ppm | 0.008746 ppm |
| Changed-control envelope response | about 55–60 ms | about 25–35 ms |
| Stop envelope response | 25.004 ms | 30.004 ms |
| Exact-zero tail after the fixed Stop margin | 11.05725 s | 11.19475 s |

Displayed upper bounds round upward where needed; complete numeric values are
in the original analysis and `derived-verification.json`. All five phases passed
on both routes, including exact-zero steady mute windows. Both channels were
identical and there were no exact-zero runs in the checked joined unmuted spans,
including gain handoff margins. Initial phase 0 and unchanged controls do not
claim a response measurement. Both-route native submission acknowledgment took at most
70.077 ms across the five phases, within the original 250 ms limit.

The fixed finite-window wrong-bus projection peaked at 0.000745 and 0.000699 of
the expected tone peak, below the preregistered 0.005 limit. This is the stated
spectral diagnostic, not a guaranteed physical crosstalk bound. The unchanged
criteria also check full recorder clocks, PCM recurrence, stereo agreement,
fixed steady windows, native packet presence/progress, phase throughput, and
Stop's persistent silence. The workflow first passed 57 offline Python controls
and eight Rust probe controls; those controls are distinct from the observation.

## Native and installed-driver cleanup

All 353 runtime telemetry rows have no global fault, no first-fault record,
no asynchronous error, and no shutdown fault. Terminal native records for both
routes have successful Create, Start, Stop, Destroy and listener-removal statuses,
zero callbacks in flight at quiescence and owner completion, and no retained
context. Both strict output shutdown flags are true. The terminal record is
preserved as an exact original JSONL line in `multioutput-terminal-original.json`.

The follower submitted 1,006,592 frames and recorded 1,991 native callbacks and
progress completions. The clock submitted 1,008,128 frames and recorded 1,977
callbacks and progress completions. Native lifecycle records are retained in
full; their software submission counters are not claims of physical playback.
All three processes exited zero without timeout, termination, or supervision
error. The owned bundle was removed and subsequent discovery found both stable
virtual cable UIDs absent. The separately repeated workflow cleanup also passed.

## Custody and offline verification

The original API bodies, artifact ZIP and workflow-log ZIP are preserved in
`build/rust-multioutput-observation-40257448775`. The artifact is **6,819,656
bytes**, SHA-256
`003487396b907bb9b03d00f83d717b46e46ade750f87a5ff896526fdfda2d85b`, matching
GitHub's supplied artifact digest. All 56 manifested files and exact 57-member
archive membership verify, including both original PCM files. The workflow logs
are 21,633 bytes, SHA-256
`0c2c733a2561c7eff8cb7e21980d820464536908f4405a87c8021a0440071787`;
that endpoint supplies no upstream digest, so its local hash and ZIP CRC are
recorded without claiming one.

`multioutput-original-text/` contains exact compact original text members.
Larger telemetry, recorder diagnostics, CSV and PCM remain in the original ZIP.
`run-metadata.json` is explicitly a selected-field API projection; original API
bodies remain in the build collection. `pinned-source/` contains 13 unchanged
source files fetched from the exact source commit's tree/blob APIs. Every blob
identity, byte count and SHA-256 verifies, and every pinned file's hash matches
the workflow's original build hash list. The archived independent observer
binary also matches its build hash.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 research/evidence/rust-multioutput-observation-40257448775/verify.py --archives build/rust-multioutput-observation-40257448775
```

`verify.py` performs no Git, network or device operations. It checks original
archive membership/hashes and compact copies, then calls only pinned pure
analysis functions on temporary copies of the original inputs. Those temporary
copies are removed afterward. It recomputes the full fixed analysis and both
diagnostic reports and verifies that the original aggregate acceptance contains
those exact three observations. `derived-verification.json` retains the first
successful verification output. `SHA256SUMS.json` covers every evidence-directory
file except itself. Collection-time `no_rerun_no_publication` records the scope
of this collection, not any later publication activity.

## Limits

This is one short observation on two virtual outputs, using internal tones. It
establishes neither audio capture nor physical playback, acoustic synchronization,
long-duration/load/restart reliability, arbitrary numbers of output routes, or
installed Windows operation. The new tone-only protocol does not replace,
repair, or invalidate the earlier failed capture/mix and HAL observations. It
does not establish production completion or a regression/improvement claim
against those different observations. This collection has not been published.
