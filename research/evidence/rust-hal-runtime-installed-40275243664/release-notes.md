This evidence prerelease preserves the original generated CI archives from [run 40275243664, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40275243664) at exact source `23c7ed6302bcb21ae99452487f137b50301254a3`.

**The installed runtime observation failed.** The `installed-hal` job passed; `installed-runtime-capture` failed its independent recorder clock/continuity check. One recorder timestamp discontinuity contains 588 excess timeline frames. The original concatenated PCM, timestamps, failed acceptance result, successful cleanup and uploaded artifacts are preserved unchanged. Correlated timing/overload diagnostics do not establish a cause or excuse the failure.

Offline custody verified all 121 non-manifest archive members (65 HAL, 56 runtime). All 13 unchanged re-analysis results matched the original reports: 12 passed and the runtime PCM/clock result failed. Reproduction equality does not turn the failed observation into a pass. Prior observations 40270207703 and 40272244069 remain unchanged.

Exact original archives:

- `hal.zip`: 10,536,205 bytes; SHA-256 `d5da927cf0b6f3b8b477563bbfa2b9096df4b67ba2655184be77f0c2e9ef67f0`.
- `runtime.zip`: 6,958,055 bytes; SHA-256 `d08bb1bb171f7be78c244eb8cc2546317d7099e3f7d98483a8904d2b41282ae2`.

This is durable evidence preservation, not a production release. No CI job was rerun, and no local recording, installation or audio-device operation was performed for this publication. The observed runtime protocol changes send gain and mute only; these archives do not qualify live DSP edits, Sound Check, application capture, multiple runtime outputs, Windows audio routing or general production quality.

The compact source-bound custody and diagnosis are in `research/evidence/rust-hal-runtime-installed-40275243664/` when committed. The lightweight release tag points to the exact observed source before those later records. This prerelease is explicitly excluded from the latest-release designation. Original Actions artifacts expire on 25 November 2026; these release assets retain their exact archive bytes separately.
