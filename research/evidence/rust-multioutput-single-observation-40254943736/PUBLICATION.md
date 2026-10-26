# Public preservation of the failed single-output observation

The [evidence prerelease](https://github.com/thatsmethen/relink/releases/tag/evidence-multioutput-single-40254943736) is public. Anonymous download and independent
verification completed at **2026-10-26T01:54:52.523226+00:00**. Run **40254943736**,
attempt 1, at source `902118a60b070bb9cefdef75d5b477b5350b9847` remains
**overall FAIL, installed HAL FAIL, installed runtime-capture FAIL**. This source
contains multi-output implementation, but the observed protocol used one input
and one output. It does not qualify simultaneous outputs.

The runtime exited with status 1; completion was observed at approximately
8.9505 seconds. Its immutable first fault was `NativeInputOverload` / Xrun,
attributed to input Cable 1. Only start and phases 0 and 1 were recorded; the
original analyzer exception, **“missing or repeated runtime protocol events,”**
was reproduced exactly. A complete runtime PCM/control analysis is unavailable.
Input recorded 808 packets / 413,696 frames. Its separate packet-timing fault
field being null does not negate the global input overload notification.

Native Stop/Destroy, listener removal, draining and both `OwnerCompleted`
observations succeeded, with zero callbacks in flight at drain/completion and
no retained context. The fault-free output shutdown gate remained false because
the Xrun was latched. Successful native cleanup does not change the failed
experiment. Both jobs' separate owned-device cleanup reports passed.

The independent runtime writer clock has 1,532- and 940-frame gaps, and its
recorder has 624-, 884- and 560-frame gaps. The later writer gap and later two
recorder gaps occurred after runtime abort. The separate HAL job has writer,
reader A and reader B clock gaps of 1,404, 1,228 and 1,340 frames respectively,
with reader interior stereo-zero runs of 1,672 and 1,812 frames. Its original
sine-fit and pair checks failed; restarted silence passed. Pair alignment across
gapped clocks is not independent proof of driver inconsistency. The jobs used
separate host clocks and must not be time-correlated. The underlying cause
remains unknown; these observations and comparisons with earlier passing runs
do not establish a source-code regression.

All **32 public assets** matched expected byte counts, SHA-256 values and GitHub
release digests. **28 ordered lossless parts**, each at most 512 KiB, reconstructed
the original HAL and runtime ZIPs without changing compressed bytes. The original
workflow-log ZIP and frozen collection ZIP were whole assets. Every upload used
an explicit Content-Length; no accepted asset was replaced.

The downloaded wrapper reconstructed and checked all **3 original ZIPs**, their
CRC and exact archive membership, and **65 HAL plus 56 runtime manifest files**.
It ran the unchanged bundled verifier, which reproduced **13 original analysis
results** using eight source files pinned to the exact commit. Only descriptive
temporary `source_report` paths are excluded from equality; metrics, thresholds
and outcomes match. No source checkout, Git command, network access or audio
device is required for that offline verification. This verifies custody and the
bounded diagnosis; it does not repair the failed experiment.

All **126 frozen collection files** remain byte-for-byte unchanged.
`collection-SHA256SUMS.json` records that snapshot; the collection ZIP includes
those files and its original checksum manifest. Collection-time metadata,
including its then-true `no_rerun_no_publication` flag, remains unchanged.
Publication records are later additions covered by the current `SHA256SUMS.json`.

Download every release asset into one directory and run:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD"
```

The verifier temporarily extracts original PCM, clock and control inputs for
offline analysis and removes the temporary directory afterward. Local wrapper
controls passed outside the checkout and rejected a one-byte archive corruption.

From this repository evidence folder, anonymous verification can be repeated:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-public.py --downloads /path/to/new-directory
```

`public-verification.json` records independent anonymous downloads and the
downloaded wrapper's verification. `publication-state.json` records publication
separately from collection-time metadata. Authenticated upload metadata and local
controls are retained separately and do not substitute for public downloads.

Root cause, the complete control protocol, sustained/load reliability, physical
devices, simultaneous outputs and installed Windows audio remain unqualified by
this run. Earlier passing and failing observations retain their separate outcomes.
