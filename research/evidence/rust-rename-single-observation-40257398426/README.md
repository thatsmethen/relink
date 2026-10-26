# Renamed source: original failed single-output observation

**Overall FAIL; separate installed HAL job PASS; installed single-output runtime job FAIL.**

This preserves [run 40257398426, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40257398426)
at source [`69f49da28a9bae16dec9d4bc29bd0d639fb73a08`](https://github.com/thatsmethen/relink/commit/69f49da28a9bae16dec9d4bc29bd0d639fb73a08).
It is the original Cable1 capture → one mix → Cable2 output protocol. It did not exercise simultaneous outputs, process/application capture, Windows, physical playback or a complete sustained workflow. No rerun, release publication or acceptance-criteria change was performed for this record.

## Bounded result

The original runtime process exited with code 1 after approximately 7.715 seconds, without a supervision timeout, termination or kill. Its event stream contains `started` and phases 0 and 1, without the remaining phases or a Stop event. The exact pinned analyzer reproduces the original error, `missing or repeated runtime protocol events`; a complete runtime PCM protocol analysis is unavailable. Partial phase acknowledgements do not qualify the complete control-response or Stop protocol.

The first latched runtime fault is `NativeInputOverload` / `Xrun`, attributed to `coreaudio:au.textual.ravelink.cable.1`, endpoint index 0, native object 92, epoch 1. The recorded property selector is 1870030194, with host tick 8116969359. It appears at zero-based row 67 of 70 native timing records and remains identical through the final record. This attribution identifies the observer that won the first-fault record, not fault onset or root cause.

At the terminal snapshot:

- Input: 677 packets and 346624 captured frames; the last recorded callback completed with `published`. The packet-timing `first_fault` field is null, distinct from the global property-listener overload record.
- Output: 696 callbacks, 692 progress records; the last recorded completion is `cancelled`. Output packet-timing fault fields are clear.
- Both native owners record successful Create/Start/Stop/Destroy/listener-removal statuses, followed by zero callbacks in flight at `QuiescenceChecked` and `OwnerCompleted`. No `ContextRetained` phase is recorded. These cleanup observations do not clear the global Xrun: the original `native_output_shutdown_proven` value remains false and the original result remains FAIL.
- Both jobs' final driver-cleanup reports pass and observe both owned virtual device UIDs absent.

The independent runtime source writer's 5625 callbacks have no sample-clock gaps or reversals and its original analysis passes. The independent recorder's 3000 callbacks have one 588-frame sample-clock discontinuity; recomputing its clock check fails. Its separate diagnostic-integrity analysis passes because it checks record integrity, not clock/PCM acceptance. The recorder also observed an overload notification on Cable2/object 100 at host tick 8116914046, approximately 2.305 ms before the runtime's latched input-fault observation in the same job. The source-writer diagnostics contain no overload notification. These nearby observations do not establish whether the host, driver, application or instrumentation caused the failure, nor a regression caused by the naming change.

The HAL job's original writer, both independent readers, paired-reader check and post-restart silence analyses pass. It was a separate hosted job with its own clock; its PASS does not override the runtime failure or establish reliable operation across runs.

## Custody and recomputation

The original three ZIPs and raw API/source-blob responses remain under `build/rust-rename-single-observation-40257398426/`. `collection.json` preserves the collection-time result and `no_rerun_no_publication` state. `original-local-files.json` hashes every original local file. `hal-original-text/` and `runtime-original-text/` contain byte-identical compact archive members; the first-fault and final timing lines are also preserved unchanged as convenience extracts.

`verify.py` performs offline checks only:

1. Verify all original local-file hashes, exact API source/run/attempt/outcome bindings, both artifact API digests, ZIP CRCs, all 121 custody-manifest members and exact ZIP membership.
2. Verify eight pinned source files against their stored SHA-256 and Git blob identities, the exact source tree, original blob API bodies and applicable original build hashes.
3. Recompute all 13 original analysis reports from original PCM, callback timestamps and diagnostics using those pinned modules: every analysis value, threshold and outcome matches. Only descriptive `source_report` paths are excluded because recomputation uses temporary files; the original paths are retained in the derived comparison record.
4. Recompute the incomplete runtime-protocol error and independent clocks, and check first-fault persistence, original event sequence and both native-owner/driver cleanup records.

```sh
python3 research/evidence/rust-rename-single-observation-40257398426/verify.py
```

Use `--archives /absolute/path/to/original-collection` if the original build folder was moved. The verifier needs Python and the preserved original collection; it requires no Git checkout, network, device access or audio rerun. It only extracts data into a temporary directory and imports pure analysis functions. `--record` creates a missing derived report once and never replaces an existing one.

`derived-verification.json` records the bounded diagnosis and exact recomputation. `SHA256SUMS.json` and `SHA256SUMS.txt` cover all compact record files except themselves. The original failure, PCM, timestamps and thresholds remain unchanged.
