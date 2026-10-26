# Sound Check audition processor

This is the device-free playback component for the maintained Rust application.
It is not yet connected to `AudioRuntime`, native output owners or the desktop
interface. [Requirements and reference limits](../../docs/sound-check-research.md)
and the [core ownership contract](../relink-core/SOUND-CHECK.md) remain applicable.

`SoundCheckSelection` binds a studio, source identity, sample rate and one explicit
output. It constructs private one-input, one-send, one-bus graphs with stable IDs.
The dry graph has neutral source controls/effects. The playback graph uses the
current source settings once, with unity send/bus controls and no bus effects.
No normal studio route or other source is imported. Renames and effect/control
edits are accepted; changed source identity, removal, studio or rate are rejected.
Preparation never discovers or opens devices, and cannot authorize recording.

`SoundCheckAudition::prepare` takes a uniquely owned immutable dry clip and returns
a control-thread writer and callback processor. Failure returns the same clip
allocation, so a host can keep its Ready state. Preparation is idle; Play is an
explicit operation. Each processor supports one playback epoch. After Stop or
EOF the host releases it and prepares a fresh processor for another explicit
Play, ensuring a defined initial DSP state with current settings. A loop keeps
DSP history across exact dry wraps; no crossfade or implicit DSP tail is added.

The writer publishes current source gain, mute, solo and supported effects through
the existing coherent control bank. Rejected updates preserve the previous bank
and clip. The callback uses fixed source planes and bounded mixer chunks, feeds
only actual clip frames through DSP, and zeros every destination on rejection.
Partial EOF produces exact zero padding, including beyond the current chunk.
Clear invalidates the clip and zeros source scratch immediately. Its Idle state
cannot replay cached graph data; physical erasure and destruction happen off
callback, after the host proves native quiescence.

The component charges actual dry-clip allocation capacity, fixed scratch and
owner metadata, selection string capacities, and existing graph/control memory
accounting. Its allowance cannot exceed the runtime's 64 MiB ceiling. The host
must first subtract native owner, capture, telemetry and other retained memory;
this component does not account for resources it never owns. A 30-second clip at
384 kHz alone exceeds that ceiling and must be rejected, not silently shortened.

`applied_control_version` means the callback accepted the target bank. It does not
prove a settled ramp, audible output or device submission. Zero-frame callbacks
do not acknowledge edits or advance playback. After final PCM is rendered, a
native host must wait for submission before revoking its epoch; otherwise the
output owner's cancellation gate would discard that final partial block.

## Native integration still required

The intended host must acquire an exclusive Sound Check lease, stop studio
routing, and block its restart. Recording requires the selected raw source and a
silent clock endpoint. All attempted [capture owners](CAPTURE-COMPLETION.md) and
the separate output owner must establish quiescence before transferring the
unique clip off callback. Playback must own no capture streams and must not call
capture selection. Clear/Close revoke the epoch immediately, reject late handoffs
by session/generation, and defer erasure until native ownership is released.
Retained owners remain charged and block reuse. None of these host transitions
are implemented by the standalone processor or established by its token checks.

Generated-buffer controls cover private graph selection, exact current DSP,
loop partition independence, partial EOF, stale calls, Stop/Clear, rejected
preparation retaining the clip, budget boundaries and callback allocation,
reallocation and deallocation. They open no devices and write no audio files.
Native capture/playback, OS permissions, output isolation, recovery, visible
workflow and listening quality remain unqualified on both operating systems.
