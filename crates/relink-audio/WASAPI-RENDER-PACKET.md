# Portable WASAPI render packet prerequisite

`src/wasapi_render_packet.rs` is original Rust output encoding and render-lease
logic. It has no COM, device, event-loop, or native owner integration. The existing
capture decoder and capture `ReleaseGuard` are unchanged. This checkpoint does
not establish native Windows output operation or teardown.

## Buffer and encoding contract

`RenderLease::new` receives the actual acquired frame count, existing WASAPI
`Layout`, a validated mutable byte slice (or a validation error), a caller-owned
release record, and a one-shot release closure. It establishes the release
obligation before validation. A native owner can reject a null or malformed
pointer without forming a slice and still release its acquisition as silence.
The owner remains responsible for validating the native pointer's lifetime,
exclusive access, and allocation extent before creating any Rust reference.

Valid output has 2–256 interleaved channels, 1–16384 frames, and exactly the
checked byte count for that layout. Validation failure changes no PCM bytes.
Construction clears every valid channel to equilibrium. Sequential stereo
chunks fill channels 1 and 2; additional channels retain equilibrium. Missing,
overlapping, out-of-order, unequal, or oversized chunks cannot become a complete
packet. Any malformed chunk permanently poisons that lease, including after a
clear. `StereoSink` delegates to the same state and does not submit anything.

The source is stereo `f32`, matching the mixer. Every nonfinite sample becomes
zero; finite values clamp to [−1, +1]. Float32/64 are little-endian. Signed PCM
uses a `f64` intermediate, scaling by 2^(valid bits − 1), rounding nearest with
ties away from zero, and saturating to the signed range. I24-in-32 is high-aligned
with all eight low padding bits zero. Unsigned8 rounds `sample * 128 + 128`,
saturates to 0–255, and uses equilibrium 128. Rounding happens after the unsigned
bias, so its half-step tie convention is explicit. This is an application
conversion policy, not a claim that every native renderer uses this quantizer.
Byte writes require no native pointer alignment or typed sample casts.

## Release and cancellation contract

The lease owns its byte borrow and private completion state. A completion from
another buffer cannot authorize it. It is neither `Send` nor `Sync`; it must be
created, filled, and released during the same owner's buffer-processing
invocation. Native code must not retain it over a wait, command, or callback.
Thread affinity alone cannot enforce a wall-time deadline.

`commit(final_authorization)` consumes the lease. It releases PCM (flags 0) only
when every acquired frame was prepared, no earlier write failed, cancellation
was not requested, and final authorization is true. `finish_silent`, cancellation,
incomplete/error paths, and ordinary Drop release the **complete acquired frame
count** with `SILENT`. The byte borrow ends before the release closure runs.
The obligation is taken before invoking the closure, so a failed release is not
retried by Drop. Deliberately forgetting a guard or aborting the process cannot
be repaired by RAII; release closures must not panic.

The caller-owned `Cell<Option<ReleaseRecord<E>>>` retains frame count, flags, and
the release result even when cleanup runs in Drop or construction fails. `E` is
an arbitrary `Copy` status, such as a raw HRESULT. Use one fresh record for each
acquisition and inspect it. A preparation error takes precedence in `commit`'s
return value; a simultaneous release failure remains separately observable in
the record. Successful PCM submission requires `commit` to return `Ok(())`.
Neither encoded bytes nor graph completion alone authorizes submission telemetry.
Cancellation after a native handoff cannot recall the submitted buffer.

## Portable controls

Tests use independently specified bytes for all six formats, asymmetric extrema,
nonfinite values, integer ties, I24 padding, near-full-scale Float32 precision,
offset byte storage, extra-channel and U8 silence, chunk completion and malformed
extents. Release controls cover success, partial/error/drop/cancel paths,
constructor failure before a slice exists, release failure, and unwind after the
last chunk. Compile-time controls reject Send/Sync leases. Allocation counters
cover every format and successful/error/cancellation cleanup paths.

These controls contain no native device calls. Future Windows owner work must
still qualify acquisition/submission, actual startup, event timing, cancellation,
endpoint reservations, HRESULT preservation, and teardown completion.

## Original implementation sources

The code was independently authored from the project-owned layout contract and
these Microsoft API/type definitions, read on 2026-10-24; no driver example or
legacy implementation was copied:

- [IAudioRenderClient::GetBuffer](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-getbuffer)
  defines acquisition extent and same-thread pairing.
- [IAudioRenderClient::ReleaseBuffer](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-releasebuffer)
  defines full-count release and the SILENT flag's treatment of PCM bytes.
- [WAVEFORMATEXTENSIBLE](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-waveformatextensible)
  defines high-aligned valid bits and zero low padding.
- Windows SDK 10.0.26100.0 `Audioclient.idl`, from the separately extracted SDK
  at `/tmp/ravelink-wdk-26100.6584/c/Include/10.0.26100.0/um/Audioclient.idl`,
  SHA-256 `145c2d5a33effd1d139ccf11dab73dff05ef82316bae772c8b5e9e72386a836a`.

The Windows-only native capture module already checks `SILENT == 2` against the
pinned Windows bindings; this module reuses that unchanged portable constant.
