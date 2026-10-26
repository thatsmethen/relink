# Application-code checkpoint: original single-output observation

**Overall PASS; separate installed HAL job PASS; installed single-output runtime job PASS. Application capture was not exercised.**

This preserves [run 40261653701, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40261653701) at exact source [`353399309004a1be5a196ebaa68c08218aff0555`](https://github.com/thatsmethen/relink/commit/353399309004a1be5a196ebaa68c08218aff0555). The fixed runtime protocol uses one `InputKind::Hardware` virtual Cable1 input, one mixer route and one Cable2 output, at 48 kHz stereo Float32. The source includes new application-capture code, but this run never selects an application or activates a private process tap or Windows process loopback. No acquisition rerun, threshold change or publication was performed for this collection.

## Bounded result

The original runtime event stream contains `started`, all four phases and `stopped`: −6 dB unmuted for 6 seconds, −12 dB unmuted for 6 seconds, mute for 4 seconds, and restoration to −6 dB for 6 seconds. Readiness was reported after 8,704 captured and 7,680 rendered frames. The recorded interval from `started` to the Stop request is approximately 23.426 seconds. All three runtime child processes exited successfully without a supervision timeout, termination, kill or supervision error.

The exact pinned runtime analyzer reproduces the complete original report with **no excluded fields**. Its unchanged amplitude, frequency, waveform recurrence, channel mean, callback clock, control acknowledgement, recorder-response, exact-zero mute and post-Stop silence criteria all pass. The independent recorder contains 1,536,000 frames in 3,000 callbacks; its sample clock and the source writer's 5,625 callbacks have no gaps or reversals. The muted steady region and stopped tail have a recorded peak of exactly zero. The three unmuted steady regions have no stereo zero-frame runs.

Control acknowledgements are approximately 36–45 ms after their requests. The original request-based envelope estimates for gain change, mute and restore are approximately 25, 40 and 30 ms; the Stop estimate is approximately 30 ms. These are the fixed analyzer's 20 ms envelope measurements, not end-to-end physical latency. Its phase-0 estimate is negative because the requested initial −6 dB state already matches the preceding signal; it is retained unchanged and is not evidence of a response before a request. The approximately 2.406 ms Stop API return is separate from native owner completion and recorded PCM silence.

Across 233 original native timing rows, the global fault/error fields and available input/output first-fault records remain clear. At the terminal snapshot:

- Input: 2,196 packets and 1,124,352 captured frames; the last completion is `published`.
- Output: 2,214 callbacks and 2,214 progress records; the last completion is `encoded` with accepted control version 8.
- Both native owners record successful Create, Start, Stop, Destroy and listener-removal statuses, followed by zero callbacks in flight at `QuiescenceChecked` and `OwnerCompleted`. No `ContextRetained` phase is present.
- The original result is `pass: true`, `error: null`, `native_output_shutdown_proven: true`. Input shutdown is observed, with lifecycle records preserved; there is no separate strict input-shutdown boolean to infer.

The independent writer/reader diagnostic reports contain no overload notifications. Both jobs observed both owned device UIDs absent before installation, exactly one match for each after installation, and both absent after cleanup. The separate HAL job's writer, both readers, paired-reader comparison and restart-silence reports pass. Its clocks are separate from the runtime job's clocks.

This short hosted virtual-device observation does not qualify application identity or permissions, macOS process taps, Windows process loopback, simultaneous outputs, physical playback, long-duration/load/restart reliability or production completion. Earlier failed observations remain unchanged. This run is not a controlled comparison and establishes neither their root cause nor a remedy.

## Retrieval and custody

The previously timed-out HAL download is preserved unchanged under `build/rust-application-checkpoint-observation-40261653701/failed-downloads/`, together with attempt metadata. It contains 10,076,160 bytes, fails ZIP parsing, and is not accepted as an artifact. The original file at its prior location remains untouched. The precise original download start/deadline/exit output was unavailable; the metadata says so rather than reconstructing it.

Fresh HAL, runtime and workflow-log downloads each succeeded on their first bounded attempt. Both artifact ZIP sizes and SHA-256 hashes match the original GitHub artifact API records, and all three ZIPs pass CRC and unique-membership checks. The logs endpoint provides no artifact digest; its downloaded bytes are preserved with a local hash. The failed partial is an exact prefix of the independently accepted HAL ZIP, missing 458,287 bytes; it was neither repaired nor substituted for the fresh original. `retrieval-verification.json` records these distinctions and every fresh attempt.

| Original archive | Bytes | SHA-256 |
| --- | ---: | --- |
| `original-hal.zip` | 10,534,447 | `ed987c066e544582a363caa468718b3d289823df49f5426dd23330e985b3ace7` |
| `original-runtime.zip` | 6,929,117 | `a5ff3d5328dfb5f091d54992c065d35d08c0e4b1dd084b05e21d2befbcc067f5` |
| `original-workflow-logs.zip` | 36,962 | `2b593d729c97518a9d0eb58a5a8cd932c7c81cfa9c78e66bd40399421cefb4c8` |

Original ZIPs, raw API/source-blob responses, failed partial and successful attempt files remain under `build/rust-application-checkpoint-observation-40261653701/`. `original-local-files.json` hashes all 27 preserved local files. The compact `hal-original-text/` and `runtime-original-text/` members are byte-identical extracts; `runtime-terminal-original.json` is the unchanged last native timing line. Full PCM, callback CSVs and native timing remain in the original archives.

## Offline recomputation

`verify.py` checks original-file hashes; source/run/attempt/outcome bindings; both GitHub artifact digests; ZIP CRCs; all 121 custody-manifest members and exact artifact membership; eight pinned source files against stored SHA-256, Git blob identities, source tree, original blob API bodies and applicable original build hashes; and exact compact extracts. It also verifies the retained failed partial and fresh download records.

It recomputes all 13 original analysis reports from original PCM, timestamps and diagnostics with the pinned source. The four HAL system reports and runtime source-writer report exclude only the descriptive `source_report` path because temporary input locations differ; those original paths are retained in the derived comparison. Every metric, threshold and outcome matches. Other reports, including the full runtime PCM/control/Stop analysis, match without field exclusions. Original aggregate acceptance observations, protocol events, readiness, independent clocks, native lifecycle and driver cleanup records are checked separately.

```sh
python3 research/evidence/rust-application-checkpoint-observation-40261653701/verify.py
```

Use `--archives /absolute/path/to/original-collection` if the build folder moved. The verifier requires Python and the preserved collection, with no Git, network, native execution, device access or acquisition rerun. It writes temporary data only; `--record` can create a missing derived report once and never replaces one. `derived-verification.json` records the exact recomputation. `SHA256SUMS.json` and `SHA256SUMS.txt` cover every compact file except themselves.
