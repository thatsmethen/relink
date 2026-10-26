# Live built-in effects: offline implementation checks

The source inventory in `validation.json` identifies the exact uncommitted candidate checked on 26 October 2026, based on `b864bc6430b6ccc5b6b861395a2071be29c0a096`. All 104 inventoried files remained byte-identical before and after validation. The committing revision can be checked against those hashes; this record does not claim that the base commit contains the new implementation.

- 311 Rust workspace tests passed: 61 core, 217 audio, 25 application and 8 HAL controls.
- Workspace formatting and strict all-target Clippy passed on macOS.
- Strict all-target Clippy passed for `x86_64-pc-windows-msvc`; this is cross-target static checking, not Windows execution.
- 12 bounded CI-report summary controls passed. Original logs and their hashes are preserved.

The new controls exercise live DSP coefficients and retained histories; unchanged-target and rapid-retarget behavior; exact bypass endpoints; persistent tuned parameters across JSON reopen; invalid whole-bank rejection; concurrent gain/mute/effect publication; once-only input and mix processing; callback phase continuity; no callback allocation/deallocation; cancelled and stalled updates; actual headless egui checkbox gestures; UI coalescing and render acknowledgement; and direct cancellation on a failed control dispatch.

Independent source review identified two defects before this candidate: effect toggles reset custom settings, and a disconnected or full UI worker could leave old audio running after a failed edit dispatch. Both were corrected and independently re-reviewed. The final review found no remaining source blocker within this change. Earlier intermediate checks remain local under `build/`; they are not substituted for this final source inventory.

All audio test data was generated in memory. These tests do not enumerate/open native audio devices, record microphones or application/system audio, request permission, or restart services. Headless interface tests do not establish native desktop interaction quality. Bank acknowledgements establish an accepted rendered target; they do not prove completed parameter ramps/envelopes, native per-output submission, sound heard, timing capacity, or production reliability. The limiter's dry/wet transition can exceed its configured ceiling; see the [DSP behavior](../../../crates/relink-core/LIVE-DSP.md).

Sound Check, plugin hosting, native application capture qualification, installed Windows audio, recovery/soak measurements and signed distribution remain separate unfinished work. Local recording is excluded from the current authorization.
