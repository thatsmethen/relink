**Overall PASS · installed HAL PASS · native macOS capture/output runtime PASS**

This evidence prerelease preserves [workflow run 40249452671, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40249452671), at exact source `f47df76d100da25c5e98d261d92792a1d507d5f1` (tree `92fe31337e7f59f40d25cde9c84a2c125da38d16`). It is one short hosted macOS virtual-cable observation. It is **not a production application or driver qualification**. No job was rerun, PCM repaired, or acceptance threshold changed.

Unlike earlier CPAL-output runtime observations, this source exercised the independently authored **native CoreAudio output owner**, its packet encoder, post-start progress gate and recorded teardown. The independent recorder supplies the observed audio evidence. Callback completion and progress describe prepared software buffers under the API contract; they do not establish physical playback. The input owner's earlier completion semantics remain part of this source and are not upgraded by subsequent changes.

The basic HAL protocol passed. The runtime's four gain/mute phases and stopped-silence protocol passed, with 3,000 continuous recorder callbacks and a zero stopped-tail peak. Changed-state responses were observed at approximately 34.99, 40.00 and 30.00 ms; stop was 24.99 ms. The initial phase already had the requested gain, so its negative detection offset is not a latency result. These measurements are recorder-relative software observations, not physical input-to-output latency.

**Pitch settling remains unqualified.** The three unmuted phases showed frequency-error magnitudes of approximately 1,215, 926 and 237 ppm. They passed the frozen 2,500 ppm criterion; passing does not establish exact pitch, long-term drift or production-quality resampling. All 347 recorded native timing snapshots report no runtime fault. Output Stop, Destroy and listener removal returned zero, callback drain and `OwnerCompleted` recorded zero callbacks in flight, and the stricter `native_output_shutdown_proven` gate passed. Both owned test-device cleanups passed.

The original HAL/runtime ZIPs match GitHub artifact digests; the original workflow-log ZIP has a retained local SHA-256 because that API supplied no digest. `collection-evidence-40249452671.zip` is a separate frozen snapshot of compact original text, collection metadata, exact terminal timing bytes and the unchanged offline verifier. Its collection-time `no_rerun_no_publication` flag describes the collection stage, before this later publication. Historical names and original bytes remain intact.

`publication-assets.json` identifies every delivered asset, its byte count and SHA-256, the complete original archive hashes, and any ordered lossless parts required by upload transport. `verify-release.py` reconstructs parts if present and verifies the complete original archives, all **65 HAL plus 56 runtime manifest files**, original compact copies, lifecycle/cleanup reports and a fresh offline recomputation of the original PCM analysis. The recomputed report must equal the original exactly.

The original verifier reads its two analyzer modules from **the exact local Git commit** above. Download all release assets into one directory and use an existing local clone of this repository that contains that commit:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD" --source-repo /path/to/relink-clone
```

The wrapper neither checks out another revision nor fetches code. It runs the bundled verifier unchanged with the supplied Git object database. The original verifier temporarily extracts original PCM/clock/control inputs for offline analysis and removes that temporary directory afterward; no device is opened and the experiment is not rerun.

Physical-device clock drift, sustained or loaded operation, unplug/replug, real blocked native calls, simultaneous outputs, installed Windows audio, complete user journeys, signing and notarization remain unqualified. Earlier failed observations remain separate evidence and are not repaired or superseded by this pass.
