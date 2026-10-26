# Public preservation of the capture-owner failure observation

The [evidence release](https://github.com/thatsmethen/relink/releases/tag/evidence-capture-owner-40251052292) is public. Anonymous download and independent
verification completed at **2026-10-26T01:20:39.397167+00:00**. The preserved result is
**overall FAIL, separate HAL PASS, native capture/output runtime FAIL** for run
40251052292, attempt 1, at source `910ef63b20782268af8cbd2e35a56b606f4a6b82`.

The runtime aborted after approximately 2.177 seconds on its immutable
`NativeOutputOverload` / Xrun fault attributed to Cable 2. Native Stop/Destroy,
listener removal and callback draining succeeded. The fault-free shutdown gate
remained false because the fault was latched; successful cleanup does not change
the failed experiment. Input Cable 1 recorded 177 packets with no input timing
fault. Its observed post-release `OwnerCompleted` covers this short fault path,
not every lifecycle path or a complete gain/mute/stop protocol.

Callback 187's **recorded entry-to-completion interval was 14.3255 ms** for
10.6667 ms of nominal audio. The completion timestamp is sampled before final
timing publication, guard destruction and return, so this interval excludes
those final steps. It is not a measurement of CPU usage or physical playback,
and the observed overload notification does not establish the true fault onset.
The frozen collection README's timing discussion should be read with this
precision clarification; the frozen source and observations remain unchanged.

Original writer gaps of 1,472, 576, 580 and 572 frames and the recorder's 764-frame
gap were independently recomputed. The writer's later three gaps occurred after
runtime abort. Same-run coincidences, nearby overload notifications and a prior
passing run do not establish the overload's cause or a capture-code regression.
The HAL PASS came from a separate host and its clock cannot be correlated with
the runtime host. Both owned-device cleanup reports passed.

All **26 public assets** matched expected byte counts, SHA-256
values and GitHub release digests. There are **21 ordered lossless parts**,
each at most 512 KiB. Reconstruction preserved the original ZIP compressed bytes.
Whole archives: `original-workflow-logs.zip`, `original-runtime.zip`, `collection-evidence-40251052292.zip`.
Large originals used the bounded parts transport after prior evidence uploads
had demonstrated repeated whole-archive timeouts. No accepted asset was replaced.

The unchanged bundled verifier checked all **65 HAL plus 56 runtime manifest
files**, exact archive membership, compact originals, terminal lifecycle and
cleanup observations. Using the original analyzer modules from the exact source
in an explicitly supplied local Git clone, it recomputed both original clocks
and reproduced **“missing or repeated runtime protocol events”** exactly. There
is no complete PCM/control protocol analysis to promote to PASS. Verification
confirms custody and the bounded diagnosis; it does not repair the failed run.

All **117 frozen collection files**, including collector, README, original
verifier and derived report, remain byte-for-byte unchanged.
`collection-SHA256SUMS.json` records that snapshot. Original collection metadata,
including its then-true `no_rerun_no_publication` flag, remains unchanged.

Download every release asset into one directory and run:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD" --source-repo /path/to/relink-clone
```

The local clone must already contain the exact source commit. The wrapper does
not fetch or check out code. Its bundled verifier temporarily extracts original
PCM/clock/control inputs for offline verification and removes the temporary
directory afterward; no audio device is opened or experiment rerun.

From this repository evidence folder, anonymous download verification can be
repeated with:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-public.py --downloads /path/to/new-directory --source-repo /path/to/relink-clone
```

`public-verification.json` contains public byte and original-analysis verification.
`publication-state.json` records publication separately from collection-time
metadata. Authenticated upload metadata is retained separately and does not
substitute for public downloads. Local wrapper controls also verified the actual
transport from outside the checkout and rejected a one-byte archive corruption.

The underlying cause, full control protocol, physical hardware, sustained load,
simultaneous outputs and installed Windows audio remain unqualified. Earlier
passing and failing observations remain separate records.
