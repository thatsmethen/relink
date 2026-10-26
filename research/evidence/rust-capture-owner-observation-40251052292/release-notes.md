**Overall FAIL · separate installed HAL PASS · native capture/output runtime FAIL**

This evidence prerelease preserves [workflow run 40251052292, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40251052292), at exact source `910ef63b20782268af8cbd2e35a56b606f4a6b82` (tree `f94a2198714611f6447452951c40050338e044b1`). No audio job was rerun, original PCM repaired, acceptance threshold changed, or outcome replaced.

The runtime exited with status 1 after approximately **2.177 seconds**, reporting `runtime stopped: fault=Xrun, error=None`. Only start and phase 0 events were recorded. The complete gain/mute/stop protocol never ran; the original analysis reported **“missing or repeated runtime protocol events.”** There is no complete PCM/control protocol PASS.

The first immutable fault is **NativeOutputOverload**, attributed to output Cable 2, native object 100, selector 1870030194 (`over`), observed at raw Mach tick 6,131,263,519. Output callback 187 had a **recorded entry-to-completion interval of 14.3255 ms** for 10.6667 ms of nominal audio. Its completion timestamp was 8.8652 ms after its supplied presentation timestamp and 64.08 microseconds before the overload observation. That timestamp is sampled before final timing publication, guard destruction and callback return, so the interval excludes those final steps. These are recorded software timing boundaries, not CPU usage, physical playback measurements, proven fault onset or a causal diagnosis.

**Native cleanup calls succeeded; the fault-free shutdown acceptance gate failed.** Output and input Stop/Destroy/listener removal returned zero, and callback drain plus `OwnerCompleted` reported zero callbacks in flight with no retained context. The output's strict shutdown gate remained false because Xrun was latched. Input Cable 1 recorded 177 packets / 90,624 frames with no input timing fault. This source's input completion is published after resource release; the observed short fault path does not qualify every lifecycle path or overall runtime reliability.

Original independent clocks contain writer gaps of **1,472, 576, 580 and 572 frames**, and one recorder gap of **764 frames**. The writer's later three gaps occurred after runtime abort. Same-run discontinuities and nearby overload notifications do not establish which condition caused another, or whether the capture-code change was a regression. The HAL PASS came from a separate hosted job; its clock cannot be correlated with the runtime host. Both owned-device cleanups passed.

The three original archives retain exact compressed bytes. HAL/runtime SHA-256 values match GitHub artifact digests; the log API supplied no digest, so its retained local hash and ZIP CRC are recorded. `collection-evidence-40251052292.zip` is a separate frozen snapshot of original compact files, exact first-fault/terminal lines, bounded derivation and offline verifier. Collection-time metadata is preserved, including its then-true `no_rerun_no_publication` flag.

`publication-assets.json` identifies complete archive hashes and any ordered lossless 512 KiB parts. Download all release assets into one directory and run:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD" --source-repo /path/to/relink-clone
```

The local clone must contain the exact source commit. The wrapper does not fetch or check out code. It reconstructs original archives, verifies all **65 HAL plus 56 runtime manifest files**, and runs the bundled verifier unchanged. Pinned original analyzer modules independently recompute both clocks and reproduce the original protocol-event exception exactly. This is custody/diagnosis verification, **not a repaired analysis or passing experiment**. Temporary original analysis inputs are removed afterward; no audio device is opened.

The underlying overload cause, full control protocol, physical hardware, sustained/load reliability, simultaneous outputs and installed Windows audio remain unqualified. Earlier passing and failing observations retain their original, separate outcomes.
