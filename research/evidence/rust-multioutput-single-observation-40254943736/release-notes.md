**Overall FAIL · installed HAL FAIL · single-output capture/mix runtime FAIL**

This evidence prerelease preserves [run 40254943736, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40254943736), at exact source `902118a60b070bb9cefdef75d5b477b5350b9847` (tree `3487df8646e06526b536e12450be71536172da36`). The source contains multi-output implementation, but this observation used **one capture and one output**. It does not qualify simultaneous outputs.

The runtime exited with status 1; completion was observed after approximately **8.9505 seconds**. Its immutable first fault was **NativeInputOverload / Xrun**, attributed to input Cable 1, native object 92, selector 1870030194 (`over`), observed at Mach tick 6,197,399,594. Only start and phases 0 and 1 were recorded. Mute, unmute and protocol Stop events are absent. The original analyzer reported **“missing or repeated runtime protocol events.”** There is no completed PCM/control protocol analysis to promote to PASS.

**Native cleanup succeeded; the fault-free output shutdown gate remained false.** Input and output Stop/Destroy/listener removal returned zero, with zero callbacks in flight at drain and `OwnerCompleted`, and no retained context. The Xrun remains latched despite successful cleanup. Input recorded 808 packets / 413,696 frames; its separate packet-timing fault field being null does not negate the global input overload notification. Output recorded 814 entries / 813 progress completions; its final callback was cancelled.

The runtime source writer has **1,532- and 940-frame** clock gaps; its recorder has **624-, 884- and 560-frame** gaps. The later writer gap and later two recorder gaps occurred after runtime abort. Nearby same-job overload notifications do not establish the fault's onset or underlying cause.

The separate HAL job failed despite zero observer process exits and passing diagnostic-integrity checks. Writer / reader A / reader B clock gaps were **1,404 / 1,228 / 1,340 frames**. The readers contain interior stereo-zero runs of **1,672 and 1,812 frames**, and fail the original sine-fit and independent-reader checks. The pair comparison's offset alignment is not separate proof of driver inconsistency across already gapped clocks. The narrower restarted-silence observation passed. Both jobs' owned-device cleanup reports passed. The two jobs have separate host clocks and must not be time-correlated.

No audio or CI job was rerun. Original PCM, failures, thresholds and outcomes are unchanged. The underlying host, scheduling, driver, application or observer cause remains unknown; comparison with earlier passing runs does not establish a source-code regression.

The three original ZIPs retain exact compressed bytes. HAL/runtime archive hashes match GitHub artifact digests; the workflow-log endpoint supplied no original digest, so its local SHA-256 and ZIP CRC are recorded. Large archives are carried in **ordered lossless parts, each at most 512 KiB**, using explicit Content-Length uploads. `publication-assets.json` records all whole-archive and part hashes. `collection-evidence-40254943736.zip` preserves the frozen 126-file collection plus its checksum manifest.

Download all release assets into one directory and run:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD"
```

The wrapper reconstructs the original archives and runs the unchanged bundled verifier. It checks all **65 HAL plus 56 runtime manifest files**, exact membership, compact originals, lifecycle observations and cleanup reports. Eight pinned source files are bundled, with their Git blob identities checked against the source tree during collection and byte hashes rechecked offline. The verifier reproduces **13 original analysis results**; only descriptive temporary `source_report` paths are excluded from equality. All thresholds, metrics and outcomes match, including the original protocol exception. This is custody and bounded diagnosis verification, **not a repaired or passing experiment**. No Git checkout, source fetch, audio device or experiment is used during offline verification.

The full control protocol, sustained/load reliability, physical devices, simultaneous outputs and installed Windows audio remain unqualified by this run. Earlier passing and failing observations retain their separate original outcomes.
