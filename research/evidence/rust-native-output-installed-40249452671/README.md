# Native macOS output — installed observation 40249452671

[Original workflow run](https://github.com/thatsmethen/relink/actions/runs/40249452671),
attempt 1, source `f47df76d100da25c5e98d261d92792a1d507d5f1`:
**HAL PASS, native capture/output runtime PASS, overall PASS**. This is one short
hosted macOS virtual-cable observation, not production qualification. No job was
rerun, no PCM was repaired, and no acceptance threshold was changed.

The runtime used the independently authored CoreAudio output owner introduced at
this source revision. Unlike earlier runtime observations that used CPAL output,
this run exercised the native output packet codec, post-start callback progress,
activation gate, and recorded teardown. The input owner still has its earlier
completion semantics at this revision; subsequent capture lifecycle changes are
not covered by these results.

## What the original artifacts show

Both jobs ran on disposable GitHub macOS 15.7.9 ARM machines, using the pinned
Rust 1.91.1 toolchain. They installed only the two project-owned test cable
devices. Both cleanup reports passed, with neither owned UID remaining visible
at the final observation. These jobs did not change the local workstation.

The HAL job recorded 3,750 writer callbacks over 40 seconds, two readers with
1,875 callbacks each, 758,996 identical overlapping frames, and three seconds
of silence after restarting the device service. Its original acceptance and
clock/PCM reports passed.

The runtime's independent recorder observed 3,000 callbacks at a measured
48,000 frames/second, with no sample-clock gaps or host-clock reversal. All four
gain/mute phases passed the existing PCM checks. Control acknowledgements were
11.73, 24.90, 25.58 and 21.58 ms. The three changed-state responses were detected
at 34.99, 40.00 and 30.00 ms; stop was detected at 24.99 ms. The initial phase
already had its expected gain, so its negative detection offset is not a latency
measurement. These are recorder-relative software observations, not physical
input-to-output latency. The observed frequency errors remain subject to the
original 2,500 ppm criterion; passing does not establish exact pitch or drift
qualification. Unmuted zero-run checks passed, the muted phase was silent, and
the stopped tail peak was zero.

All 347 native timing snapshots report no runtime fault. The final output report
records 2,187 callbacks seen and 2,186 progress completions; the cancelled final
callback is not accepted progress. Native Stop, DestroyIOProcID and listener
removal returned status zero. The callback drain and `OwnerCompleted` each
reported zero callbacks in flight, with no retained context or first fault.
The probe's stricter `native_output_shutdown_proven` gate passed. The records
describe prepared buffers and native callback progress; the independent recorder
PCM supplies observed output evidence. They do not establish physical playback.

## Custody and independent verification

The collector preserved the original GitHub API responses and downloaded the
original artifact ZIPs without recompression. Both artifact SHA-256 values match
GitHub's digests. ZIP CRC checks and all **65 HAL plus 56 runtime** manifested
files passed, with exact archive membership. Compact original text files retain
their original bytes. `runtime-terminal-original.json` is the exact last line
of the original timing JSONL, including its newline.

| Original archive | Bytes | SHA-256 |
| --- | ---: | --- |
| `original-hal.zip` | 10,534,930 | `cb4f3e16f2014aef2ede1788b22b58817fb0f19142fb5424b769c6e7462d1441` |
| `original-runtime.zip` | 7,072,328 | `55536918a9d72a9299b08c4897b1b193b6d412a8fd4836644013a4df038213f6` |
| `original-workflow-logs.zip` | 34,679 | `e0c62cd7d55c2a8667d60fe330849eaa719ab1a5df1fc70dbcb467c435d6d986` |

`verify.py` checks the original archives, manifest members, compact copies,
terminal lifecycle, cleanup reports and derived conclusions. It also loads the
two analyzer modules from the exact local Git commit above and recomputes the
entire runtime PCM analysis from the original WAV, callback clocks, controls,
telemetry and result. The recomputed report equals the original report exactly.
This is an offline observation check; it neither opens an audio device nor reruns
the experiment.

From the repository root, with that source commit available locally:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 research/evidence/rust-native-output-installed-40249452671/verify.py --archives build/rust-native-output-installed-40249452671
```

`collection.json` describes collection-time state. `derived-verification.json`
records the independent calculation and bounded conclusions. Publication status
is recorded separately when public copies have been verified; collection records
are not rewritten to imply that publication had already occurred.

## Remaining qualification

This run covers one selected native macOS output and the original test cables.
It does not cover simultaneous outputs, physical-device clock drift, hardware
unplug/replug, real drivers blocked in native calls, sustained or loaded operation,
Windows installed audio, complete UI journeys, signing or notarization. Earlier
failed observations remain separate records and are not superseded by this pass.
