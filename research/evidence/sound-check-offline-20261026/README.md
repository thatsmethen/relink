# Sound Check components and capture completion: offline checkpoint

The 115 source files in `validation.json` identify the exact candidate checked
on 26 October 2026, based on `3573d1f58571d7762bc711b28af0dce04461f740`.
All hashes remained unchanged through the recorded checks. The base revision
does not contain this candidate; compare the committing tree against the inventory.

- **372** workspace/all-target Rust tests passed: 78 core, 261 audio including
  generated probe controls, 25 headless application controls and 8 HAL controls.
- Workspace formatting and strict all-target Clippy passed on macOS.
- Strict all-target Windows MSVC cross-target Clippy passed. This is static
  checking, not Windows execution or installation.

The bounded dry-clip engine checks selected-source identity, generation guards,
30-second caps, partial callbacks, exact loops, stale/replaced/cleared clip access,
unique ownership transfer, actual capacity and off-callback erasure. The private
audition planner excludes other sources and studio routes. Its processor applies
current effects once, preserves dry PCM through live edits, supports exact loops
with continuous DSP history and silences partial EOF and rejected callbacks.
Preparation failures retain the same clip allocation. Exact budget boundaries
and positive allocator instruments cover first and repeated callbacks, live bank
application, rejected calls, Stop/Clear and EOF with zero allocation, reallocation
or deallocation in those measured paths.

Capture completion tickets separately count owner tasks and retained callback,
activation-handler and membership-worker lifetimes. Pending work cannot claim
completion; failed cleanup or unwind stays retained. Fake-resource tests cover
drop order, failed spawning, held native block references, reservation cleanup,
blocked dependencies and event-close insufficiency. Existing failure and PCM
acceptance criteria were not relaxed. Independent source review found no blocker
within these components and added the exact audition-budget regression.

The full native Sound Check workflow remains **unimplemented**: exclusive runtime
lease, record/playback owner handoff, final-frame submission gate, desktop journey
and OS/device recovery still require implementation and qualification. Tokens and
offline completion tests are not proof of native shutdown or output isolation.
No microphone, application or system audio was captured; no native device was
opened and no recording permission or audio-service operation occurred. All PCM
was generated in memory. These tests do not establish listening quality, native
runtime timing, pinned-reference fidelity or production readiness.

See the [core contract](../../../crates/relink-core/SOUND-CHECK.md),
[audition integration obligations](../../../crates/relink-audio/SOUND-CHECK-AUDITION.md),
[capture completion contract](../../../crates/relink-audio/CAPTURE-COMPLETION.md)
and [reference evidence limits](../../../docs/sound-check-research.md).
