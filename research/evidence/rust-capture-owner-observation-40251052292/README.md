# Capture owner source — original observation 40251052292

[Workflow run 40251052292, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40251052292)
used exact source `910ef63b20782268af8cbd2e35a56b606f4a6b82`:
**overall FAIL, separate installed HAL PASS, native capture/output runtime FAIL**.
The original artifacts, failures and acceptance criteria remain unchanged. There
was no audio rerun, PCM repair, threshold change or replacement result.

## Recorded runtime failure

The runtime exited with status 1, observed after approximately **2.177 seconds**,
without exceeding its process deadline or receiving a termination signal. Its
result says `runtime stopped: fault=Xrun, error=None`. Only the start event and
phase 0 were recorded; the remaining gain/mute phases and protocol stop event
are absent. The original analyzer reported `missing or repeated runtime protocol
events`. This is an incomplete, failed protocol, not an unavailable success.

The first attributed runtime fault is **NativeOutputOverload** on output Cable 2
(`coreaudio:au.textual.ravelink.cable.2`), native object 100, property selector
1870030194 (`'over'`), observed at raw Mach host tick **6,131,263,519**. Its metadata
is complete and unchanged in every subsequent faulted snapshot through the final
record. There are 42 native timing snapshots. This attribution identifies where
the runtime recorded the fault; it does not identify the underlying host,
scheduling, driver, processing or instrumentation cause.

The last recorded output callback (187) took **14.3255 ms of elapsed wall time**
for 512 frames (**10.6667 ms** of nominal audio). It completed **8.8652 ms after**
its supplied presentation host timestamp, **64.08 microseconds before** the
overload notification was observed. This is a late callback observation, not a
measurement of CPU time, physical playback, or the underlying cause.

The output recorded 187 callbacks and progress completions. Its separate timing
fault field is null; that does not negate the global attributed overload fault.
At the final observation, Stop, Destroy and listener removal returned status zero;
callback drain and `OwnerCompleted` reported zero callbacks in flight, with no
retained context. `native_output_shutdown_observed` is true, but the stricter
`native_output_shutdown_proven` remains **false because the runtime fault is
latched**. Successful cleanup calls do not clear that fault or make the run pass.

Input Cable 1 recorded 177 packets / 90,624 captured frames and no input timing
fault. Its Stop, Destroy and listener removal also returned zero; drain and
`OwnerCompleted` reported zero callbacks in flight. This source implements input
completion after resource release. The observation covers that fault-path
completion, not all capture lifecycle paths or the complete mixing protocol.

## Independent observer boundaries

Recomputation from the original callback CSVs finds source-writer sample-clock
gaps of **1,472, 576, 580 and 572 frames**, before zero-based callbacks 210, 2623,
2627 and 4296. The independent recorder has one **764-frame** gap before callback
151. The source writer continued for 60 seconds and the recorder for 32 seconds,
well after the runtime had exited. Their zero process exit codes and diagnostic
integrity checks do not establish sample-clock continuity or protocol acceptance.

The first overload notifications and clock discontinuities occurred in the same
runtime job. Their co-occurrence does not establish which condition caused which,
and comparison with an earlier passing run does not establish a capture-code
regression. The separate HAL PASS came from a different hosted job; its host
clock cannot be joined to this runtime job's clock. Both job cleanup reports
passed and confirmed both owned virtual cable UIDs absent.

## Custody and verification

The collector retained three original ZIPs and original API responses. Both
artifact ZIP hashes match GitHub's digests; the workflow-log API supplies no
original digest, so its retained local SHA-256 and ZIP CRC are recorded. All
**65 HAL plus 56 runtime** manifested files and exact archive membership verify.
Compact originals remain byte-identical. The first-fault and terminal JSON files
are exact original timing lines, including their newlines.

| Original archive | Bytes | SHA-256 |
| --- | ---: | --- |
| original-hal.zip | 10,533,562 | `253cb82db338b9671ad741b79d0bbe961d1014cc17092155d7da3e4f31f270c4` |
| original-runtime.zip | 943,176 | `24a704c7fc9a586b53cadc50b9060b38d54da7023df246f932d739c46a8b0848` |
| original-workflow-logs.zip | 34,799 | `3fc510c3a55b9a34f592cdefbe940964afedebbcc30923330161c35c1f8d104d` |

`verify.py` verifies custody and reproduces `derived-verification.json` from the
original archives. It loads the exact analyzer modules from the local Git source
commit above, independently recomputes both observer clocks and reproduces the
original protocol-event exception exactly. It does **not** turn the absent full
PCM analysis into a passing result. Source analyzer hashes are included in the
derived report. Verification temporarily extracts original analysis inputs,
removes the temporary directory afterward, and never opens a device or reruns
the experiment.

From a local repository containing the exact source commit:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 research/evidence/rust-capture-owner-observation-40251052292/verify.py --archives build/rust-capture-owner-observation-40251052292
```

Collection-time metadata and its `no_rerun_no_publication` flag are preserved.
Any later publication state is recorded separately after public download
verification. This failed observation does not qualify production reliability,
physical devices, multi-output operation, Windows audio, or the uncompleted
control protocol. Earlier evidence retains its own original outcome.
