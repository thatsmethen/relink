# Superseding product requirements — 2026-10-24

The user explicitly changed the product requirements to independently authored proprietary application and virtual-device drivers, implemented in Rust, for Windows and macOS. This supersedes the original open-source-driver request and the Swift/macOS prototype. The autonomous goal remains active; its original tool summary predates this requirement change.

New shipping implementation lives in `crates/` and `drivers/`. First-party new code is proprietary. Permissively licensed Rust libraries and platform SDK interfaces are allowed dependencies; no GPL implementation is copied, translated, linked or packaged into the new shipping targets. All former GPL notices and rights remain in force for the archived prototype. Repository visibility does not revoke any previously granted rights.

The user subsequently requested public GitHub visibility and continued incremental pushes. The repository is now public; first-party proprietary licensing and the archived prototype's existing licence are unchanged. `scripts/push-public.sh` checks the requested visibility before and after publication.

The user also requested the shared contribution identity used for Rebase. Public branch/tag history now uses `RaveLink Contributors <contributors@ravelink.invalid>`; source trees, messages, dates and mapped parents were verified unchanged. Frozen artifacts retain historical IDs. See [identity policy](../docs/contributor-identity.md).

The product was subsequently renamed **ReLink** at the user's request. The public repository is now [thatsmethen/relink](https://github.com/thatsmethen/relink); GitHub redirects the prior repository URL. Visible app and package names use ReLink. Source folders and crates now use ReLink names. Bundle/device identifiers and studio paths remain compatible with previous builds; see [naming compatibility](../docs/naming.md). Historical evidence and archives retain their original names and bytes; future commits use the anonymous ReLink project identity.

The previous prototype is frozen under `legacy/`. It is retained to preserve history, failed experiments and test evidence. Native code, old driver patches and GPL build scripts are not a shipping foundation for the Rust rewrite. Fresh driver work uses official SDK ABI contracts and independently authored implementation. This is a provenance boundary, not a claim of an independently audited clean-room process.

## Frozen results

- Source f733a8c: macOS signed universal prototype built; not notarized.
- Installed CI 40022691623: driver lifecycle passed; audio loopback/continuous PCM failed. Original raw WAVs and hashes are retained. No successful production capacity claim.
- Native 128-input/32-mix interface stalled even with indexed routes. Later publication deduplication passed deterministic tests but received no repeated native scale acceptance.
- Helper resolution and bounded diagnostic/source-archive work remained experimental at freeze. None changes the failed installed-audio result.
- User's original studio was restored byte-for-byte and checked natively with audio stopped.
- No old test driver or Elgato reference driver was installed on this host during the migration.

## New acceptance gates

1. Rust shared graph, session persistence, bounded realtime audio processing and a native desktop application on both platforms.
2. Original macOS HAL and Windows virtual audio devices, actual installed capture/render loopback and independent PCM verification.
3. Dynamic input and bus counts limited by measured resources rather than product licensing. Scalable UI, accessibility, topology changes and routing clarity.
4. Device/app capture, independent monitor/stream/voice mixes, effects, Sound Check and recoverable session workflows measured against pinned reference behavior.
5. Device loss, restarts, suspend/resume, rate changes, clock drift, long-run stability and security/permission behavior.
6. Signed/notarized macOS distribution and Windows driver qualification/signing, clean install/upgrade/uninstall and native OS/architecture verification.

A compiling Rust prototype or passing generated-audio test does not satisfy installed-driver, native Windows, reference-fidelity or production acceptance.

## Current evidence checkpoints

- `6484db8`: native macOS and Windows desktop CI passed. The original macOS HAL passed a disposable-host installation, two independent 20-second stereo recordings, bit-identical reader overlap, fresh-client silence and owned-driver removal. Original PCM and observer records are preserved under `research/evidence/rust-hal-installed-40039161891/`. This measured the driver, not the complete Rust application runtime.
- `6484db8`: the original Windows driver built on Windows Server 2022 and passed nine user-mode host tests, SDK ABI assertions, PE checks, INF validation and unsigned catalog generation. The original artifact and logs are preserved under `research/evidence/rust-windows-native-40039161836/`. No installed-kernel, endpoint, audio, Verifier, HLK or signing result is claimed.
- `b30dea6`: explicit hardware/virtual-device input capture, a bounded clock-compensating transport and native input selection were added. The stopped macOS picker was exercised and the original studio restored byte-for-byte. Offline clock, spectral and allocation controls passed; physical input, permission and runtime audio evidence remained pending.
- `4aa6394`: Windows capture was replaced with an original Rust WASAPI owner thread and portable packet decoder. Independent source review corrected failed-construction event lifetime and an exact-drain-budget error. The 34 audio tests pass locally, including silent-packet, release, timestamp, cancellation and transport controls; Windows-target checks pass. Native execution remains a distinct gate.
- Repeated driver observation `40041055533` at `b30dea6` failed: one reader omitted 568 frames early in capture, despite clean later sine measurements. Original PCM, timestamps and failed analysis are retained under `research/evidence/rust-hal-failed-40041055533/`. Separately triggered run `40041275322` passed. The intermittent continuity risk remains unresolved; a passing repeat does not replace the failure.

- `03ff13a`: original app icon, complete built-in source/mix effect parameters and keyboard history fixes passed stopped macOS interaction checks and native desktop CI on both platforms. The public development preview packages and screenshots are preserved under `research/evidence/rust-preview-40045963463/`; this is not audio qualification.
- `db639ff`: original Rust macOS IOProc capture, native timing and bounded lifecycle diagnostics passed 51 audio tests, 3 probe tests, independent source review and cross-target checks. The [first installed diagnostic observation](evidence/rust-native-runtime-installed-40046411757/README.md) still failed; its exact archives and public re-download verification are retained independently. The first-phase Xrun origin remains unresolved. Three [incidental observations](evidence/rust-incidental-hal-observations/README.md) are preserved separately, without changing criteria or replacing failed results.
- `a4b21a3`: atomic first-fault attribution passed 106 offline workspace tests and independent source review. [Installed run 40048380199](evidence/rust-attribution-installed-40048380199/README.md) then failed with an identified capture-consumer underflow on Cable 1 after 7.33 seconds. Its separate basic HAL job passed. Original archives and verified public downloads are retained; neither the earlier Xrun's origin nor the new underflow's scheduling cause is established.
- `db052e4`: borrowed bus views, offline atomic fanout, pure output-plan validation and post-render control acknowledgment passed 134 workspace tests and independent source review. [Installed run 40050165081](evidence/rust-ack-installed-40050165081/README.md) passed the complete runtime and separate basic HAL protocols, including all live-control phases, continuous independent recording and Stop silence. Both original archives and every manifest member were verified after anonymous public download. This is one unchanged-protocol observation; output still uses CPAL, and the earlier intermittent failures remain unresolved.
- Five-millisecond live gain/mute/pan/solo ramps passed deterministic partition, retargeting, silence-endpoint and allocation controls. A complete installed runtime observation passed every PCM phase and live-control response but failed native callback continuity; it does not establish full runtime quality.

Upcoming work includes resolving installed capture-to-mixer-to-output continuity, captured-source simultaneous independent outputs, native application/process capture qualification, plugin effects and Sound Check workflows, recovery and sustained resource measurements, native Windows interaction/audio tests, and distributable signed installers. Reference mixer fidelity remains unverified because the pinned Elgato reference stops at its separate vendor-driver installation gate.


## Output routing journey checkpoint — 2026-10-26

Inspected: the single-output picker tied launch to the currently edited mix and
could not express independent destinations. Implemented: a global Outputs window
assigns each unique device a mix and an explicit clock. Saved routes are separate
from editor selection; removing the clock requires another explicit choice.
Route edits join session edits in Undo/Redo, and the complete versioned studio
saves atomically. Legacy reads preserve bytes; quitting an unedited studio does
not trigger migration. `RELINK_STUDIO_PATH` accepts an explicit absolute file for
isolated developer runs; invalid overrides do not fall back to the real studio.

Verified in the stopped macOS test build: empty Start opens actionable routing,
two distinct devices and mixes can be selected, duplicate devices are disabled,
changing the editor mix preserves assignments, removing the clock exposes the
missing choice, Undo restores it, and save/quit/reopen retains both routes and the
clock. Audio remained stopped throughout. The prior test binary and source hashes
are recorded with the [routing screenshots](evidence/relink-output-routing-ui-20261026/README.md); the final checkpoint adds a separate
priming deadline correction and avoids saving an unchanged studio on exit.
Corrupt or future studio files remain untouched until Save or an edit. The first
recovery save preserves an exact synced sibling backup before replacing the
canonical file, so recovered work reopens normally. A failed backup blocks the
replacement. Focused tests cover these boundaries. Asynchronous cleanup failures
remain visible after Stop; a healthy restart clears only stale queued errors.

A post-quit accessibility read automatically relaunched the test bundle without
its isolated environment and migrated the ordinary studio wrapper on exit. Its
session content was verified identical to the preserved original; the original
bytes were restored exactly. Subsequent test captures close the app without a
post-quit UI read. No audio stream or system default changed.

Remaining journey queue: running multi-device status/controls/stop (implemented,
not natively verified), large route lists and minimum window width (implemented,
not yet qualified), unavailable-device recovery (source-reviewed), process
capture and Sound Check (pending). This bounded stopped journey is not a whole
product usability or runtime acceptance result. Native Windows interaction,
installed simultaneous-output PCM, sustained load and recovery remain gates.

## Application capture implementation — 26 October 2026

The maintained Rust source now connects a separate asynchronous application picker
to stable saved identities, complete current process resolution, native macOS
process taps and Windows include-tree loopback owners. Multiple raw streams are
summed before one logical channel DSP pass. Native permission, idle/resume,
process-relaunch, Stop/cleanup and independent PCM observations remain unqualified.
The interface marks this as an early preview. See [usage and limits](../docs/application-capture.md)
and [API research and ownership boundaries](../docs/audio-application-capture-research.md).

At `dbab3fd`, the [native synthetic application fixture](evidence/application-fixture-preparation-dbab3fd/README.md)
was prepared from a source-bound Rust build and ad-hoc signed generator
bundles. All 92 offline instrument/installer controls passed. The reviewed local
Rust HAL test package preserves its exact source, final signed binary and payload
hashes. Preparation launched no audio; the local preflight found neither owned
cable loaded. This is preparation evidence, not an application-capture result.
