# Original runtime failure with missing artifact — 40270207703

[Run 40270207703, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40270207703)
used exact source `dbab3fdeefe1c4833f8b56f3fb00cbe49cf3d0e1`:
**overall FAIL; separate installed HAL PASS; installed runtime FAIL; runtime
artifact upload FAIL**. These are the original outcomes. No job was rerun,
recording substituted, PCM repaired, or acceptance threshold changed.

## Runtime failure: confirmed exit, unavailable underlying cause

The installed hardware-capture/mixing command
`python3 drivers/macos/tests/run_runtime.py run` exited **1** at
`2026-10-26T04:43:57.2904880Z` in its original step log. Its command group began at
`04:42:53.0771100Z`. The console contains no runtime fault, phase report, PCM
analysis, or exception detail. The check-run output title/summary/text are null;
its failure annotation only repeats the exit code.

The underlying audio/test failure is **unrecoverable from the available original
evidence**. The runtime result JSON, native timing journal, phase events, raw WAV,
callback CSVs, individual child results and original custody manifest are not in
any downloadable artifact or console output. We cannot identify an overload,
underflow, discontinuity, wrong gain, startup failure, cleanup failure or another
specific cause, nor determine which phases completed. Earlier runs are not a
substitute for this missing observation. The roughly 64-second command duration
also cannot establish how long the runtime itself ran: its independent writer
can continue after the runtime exits.

The cleanup and evidence-manifest steps concluded `success`, but their raw
reports were lost with the runtime artifact. Consequently we retain those step
outcomes without claiming an independently verified per-UID absence result for
this job. The separate HAL cleanup records *are* available and verified below.

This was the existing Cable1 hardware-input → mixer → Cable2 protocol. It did
**not** execute the newly prepared synthetic application-capture fixture. The
92 offline instrument controls passed beforehand; those controls do not qualify
the failed native runtime journey.

## Upload failure: artifact creation timed out

After the test failed, `actions/upload-artifact@v4` selected **57 files** from
`build/rust-runtime-system/` for `original-rust-runtime-40270207703-1`. Its original
log records four retry messages and then:

> Failed to CreateArtifact: Failed to make request after 5 attempts: Request timeout: /twirp/github.actions.results.api.v1.ArtifactService/CreateArtifact

The final error was logged at `2026-10-26T04:44:56.0139430Z`. The failure is at
**artifact creation**, not a reported ZIP digest mismatch, quota error, invalid
path, or successful upload followed by a failed download. The available evidence
does not identify the underlying network/service cause. Node version and runner
capacity notices are retained, but their presence does not establish causation.

The completed run's official artifact listing contains exactly **one** artifact:
`original-rust-hal-40270207703-1`. There is no runtime artifact ID or downloadable
runtime ZIP. No substitute archive has been manufactured. The runner did create
evidence locally (the upload action reports 57 selected files), but its manifest
and individual bytes cannot be recovered through the available run/artifact APIs.

## Separate HAL result and custody

The original HAL archive hash matches GitHub's supplied SHA-256. ZIP CRCs, all
**65 manifested files**, and exact **66-member** archive membership verify.
Original compact copies remain byte-identical to their archive members.

The retained HAL reports record passing independent 20-second stereo readers,
no callback sample-clock gaps, **758,928 overlapping frames with zero difference**,
and exact zero peak after stop/restart. Its raw WAVs and callback data remain in
the original archive. Both owned UIDs were absent in the retained cleanup report.
Before IO, both installed-device discovery records also reported successful
`DeviceIsRunningSomewhere == false`; this establishes that property's support in
this hosted observation, not on the user's local machine.

| Original archive | Bytes | SHA-256 |
| --- | ---: | --- |
| `original-hal.zip` | 10,535,212 | `ef88974fca35490ef3d8544047435347a9806b1848d8773ee3fe114a1d5a5300` |
| `original-workflow-logs.zip` | 37,510 | `88b01b98708c5ca58841db886895850a0a60f27cf745a634b09458c8217eadd6` |

The workflow-log API supplies no original archive digest; its retained local
SHA-256, CRCs and member hashes are recorded instead. Original run/job/artifact/
check-run/annotation API responses and selected raw logs are retained here.
Listed observer source hashes were checked against the exact Git commit. The
recorded driver build hash is retained, but that job's driver executable is not
included in its artifact, so that binary hash cannot be independently rechecked.

Archives are retained in `build/rust-runtime-upload-failure-40270207703/`.
No release publication or commit was performed during collection. This collection
uses approximately 10.6 MB for original ZIPs and less than 0.2 MB of compact
metadata, logs and verification code; no desktop artifacts were downloaded.

## Offline verification

From a checkout retaining the exact source commit and original archives:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 \
  research/evidence/rust-runtime-upload-failure-40270207703/verify.py \
  --archives build/rust-runtime-upload-failure-40270207703
```

The verifier checks all original archive/member/compact-copy hashes, the HAL
manifest, recorded outcomes, source identities, official runtime step failures
and the absent runtime artifact. It reproduces `derived-verification.json`
without devices, network access, an audio rerun, or a replacement PCM analysis.
Custody verification does not turn this failed and partly unobservable run into
a pass or establish production reliability.
