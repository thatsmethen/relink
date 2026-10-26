# Multi-output source, single-output observation 40254943736

[Original workflow run 40254943736, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40254943736)
used exact source `902118a60b070bb9cefdef75d5b477b5350b9847`, tree
`3487df8646e06526b536e12450be71536172da36`.
**Overall FAIL, installed HAL FAIL, installed capture/mix runtime FAIL.**
This source contains multi-output implementation, but this workflow's runtime
probe uses its single-output `start` path: Cable 1 capture into Cable 2 output.
The observation does not test simultaneous outputs.

This local collection preserves the original outcomes, PCM, reports, errors and
thresholds. No audio or CI job was rerun, no original bytes were repaired, and no
release was published. Collection used read-only GitHub APIs, without Git commands.

## Runtime abort and ownership observations

The runtime process exited with status 1; completion was observed after
approximately **8.9505 seconds**, without a timeout or termination signal.
`runtime-result.json` reports `runtime stopped: fault=Xrun, error=None`.
Only the start event and phases 0 and 1 were recorded (unmuted gains of −6 and
−12 dB). Mute, unmute and protocol Stop events are absent. The original analyzer
raised **“missing or repeated runtime protocol events”**. Offline recomputation
reproduces that exception exactly; there is no completed PCM/control protocol
analysis to interpret as a PASS.

The first immutable runtime fault is **NativeInputOverload**, direction Input,
Cable 1 (`coreaudio:au.textual.ravelink.cable.1`), endpoint index 0, native object
92, epoch 1, property selector 1870030194 (`over`), global scope, element 0.
Its recorded observation time is raw Mach tick **6,197,399,594**. Packet index and
OSStatus are absent. It first appears in line 166 of 167 timing rows and remains
identical in the final row. This identifies the winning fault observation, not
the true onset or underlying scheduling, driver, application or observer cause.

Input Cable 1 recorded **808 packets / 413,696 frames**. Its packet-timing
`first_fault` is null; that does **not** negate the global attributed input
overload notification. Output Cable 2 recorded **814 callback entries / 813
progress completions**. Its final callback 814 reports `cancelled`, while the
last retained validated packet is 813. These distinct counters are preserved.

Both native owners recorded successful Start, Stop, Destroy and listener-removal
statuses, with zero callbacks in flight at drain and `OwnerCompleted`, and no
`ContextRetained`. Both terminal-observation flags are true. The stricter
`native_output_shutdown_proven` is false because the runtime Xrun remains
latched; **successful cleanup calls do not make this failed run pass**. The
complete original lifecycle rows are retained, including independently observed
handle-cancellation and owner events; their array order is not a claim of global
time ordering between threads.

The runtime's independent source writer records **1,532- and 940-frame gaps**
before zero-based callbacks 845 and 1112. The recorder records **624-, 884- and
560-frame gaps** before callbacks 781, 1050 and 1056. Both clocks fail the original
continuity criterion. The writer completed its 60-second acquisition and the
recorder its 32-second acquisition after the runtime aborted. The later writer
gap and later two recorder gaps therefore cannot explain the initial abort.

The writer's first overload notification was observed 113.625 microseconds before
the runtime's latched notification; the recorder's first was observed 225.042
microseconds after it. These are nearby observations from the same job and host,
not a causal ordering of the underlying fault. The raw notifications, native
timestamps and gap-adjacent callback records remain available.

## Separate HAL failure

All four HAL observer processes exited zero without timeouts or termination
signals, and supplemental diagnostic-integrity checks passed. The original
acceptance still failed:

| Observer | Missing sample-clock frames | Before callback (zero-based) | Longest interior stereo-zero run |
| --- | ---: | ---: | ---: |
| Tone writer | 1,404 | 861 | Not a recorder |
| Reader A | 1,228 | 843 | 1,672 frames |
| Reader B | 1,340 | 826 | 1,812 frames |

Both readers also fail the original fixed whole-interior sine-fit amplitude and
residual checks on both channels. These fit failures are preserved; they do not
by themselves prove uniform attenuation, particularly across a discontinuous
sample clock. The unchanged independent-reader comparison reports maximum
difference `0.21370533108711243` and 861,414 differing samples above `1e-6` over
759,548 overlapping frames. Its initial-offset alignment is not independent
proof of driver inconsistency when both callback timelines already have gaps.

The separate three-second restarted-silence observation passes: 144,000 frames,
zero peak, no sample gaps. That narrower result does not override the failed
tone/clock observations. Both jobs' cleanup reports pass and show both owned
virtual cable UIDs absent. HAL and runtime are separate hosted jobs; their Mach
clocks must not be joined.

## Custody and offline verification

The original API bodies and three ZIPs are retained under
`build/rust-multioutput-single-observation-40254943736`. Artifact ZIP hashes match
GitHub's supplied digests. The workflow-log endpoint supplies no original digest;
its retained local hash and ZIP CRC are recorded.

| Original archive | Bytes | SHA-256 |
| --- | ---: | --- |
| original-runtime.zip | 3,340,292 | `8f2a0bcfedf264df71d0f70c6aac9f8fec13e0b752676a0158d77cbf9700adc3` |
| original-hal.zip | 10,523,192 | `c58fc70b4a6033da20fe27f7d6301530053993347788c1e6652f436024b59423` |
| original-workflow-logs.zip | 34,836 | `fc52600abb26d69d467d7dbe40432f7bd5d53a45a5381a4954b22bd79bc7163a` |

Every **65 HAL plus 56 runtime manifested file**, exact archive membership and
compact original copy verifies. First-fault and terminal JSON files are exact
original timing lines, including newlines. `run-metadata.json` is explicitly a
selected-field projection; original API bodies remain unchanged in the local
collection. `original-local-files.json` identifies those collected local bytes.

`pinned-source/` contains eight unchanged source files obtained from the exact
commit's Git tree and blob APIs. Each blob's Git SHA-1 and byte count were checked
against the tree, and its SHA-256 is retained. The source includes the workflow,
analyzers, harnesses, native observer and Rust runtime probe. No working-tree
analyzer is substituted.

`verify.py` loads the pinned analysis modules and calls only offline analysis
functions. It recomputes **13 original analysis reports**, including the original
protocol exception, HAL PCM analyses, reader comparison, source writer analysis
and all six diagnostic summaries. Every outcome, metric and threshold matches.
Only descriptive `source_report` paths are excluded from report equality because
the recomputation reads temporary local inputs; the original path is retained
explicitly beside each comparison. Both runtime observer clocks are separately
recomputed, including the recorder clock that the incomplete protocol prevented
the original aggregate analyzer from reporting.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 research/evidence/rust-multioutput-single-observation-40254943736/verify.py --archives build/rust-multioutput-single-observation-40254943736
```

The verifier performs no Git, network or device operations. It removes temporary
analysis inputs afterward and leaves original files unchanged.
`derived-verification.json` retains the bounded derivation;
`offline-verification-result.json` retains the first successful verification
output. `SHA256SUMS.json` covers every file in this evidence directory except
itself. Collection-time `no_rerun_no_publication` remains true.

Neither these failures nor comparison with earlier passing observations establish
a regression from the multi-output source change. Root cause, sustained/load
reliability, physical hardware, complete control protocols, simultaneous outputs
and installed Windows operation remain unqualified by this observation.
