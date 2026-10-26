# Portable CoreAudio output encoding

This is the original Rust byte encoder prerequisite for a native output owner.
It does not open a device, submit native PCM, observe playback, or establish
native callback/lifecycle timing. The installed runtime failure records and
qualification thresholds are unchanged.

`OutputLayout::new` runs off the callback and reuses the input `Layout`'s
conservative ASBD validation. The output wrapper additionally requires a positive
exact nominal rate and at least two total channels. Supported representations are
Float32/Float64, signed 8/16/24/32 significant bits in one-to-four-byte containers,
and unsigned 8-bit PCM. Little/big endian, interleaved and noninterleaved streams,
and supported high/low valid-bit alignment are explicit. Unknown flags, unsigned
wide PCM, compressed/ambiguous formats, and implicit rate conversion are rejected.
Capture decoding arithmetic and its supported-format policy are unchanged.

`prepare` accepts safely borrowed mutable byte planes. It validates every plane's
channel count, full-frame extent and common duration before modifying any plane.
Enabled buffers must contain 1–16,384 frames, with at most 256 device channels.
There are no OS pointers or pointer casts. Safe mutable borrowing prohibits
overlapping planes; a future native owner must validate enabled-stream selection,
pointer ranges and callback lifetimes before constructing these slices. Disabled
native buffers are outside this API. A malformed descriptor leaves all bytes
untouched and must cause the native owner to reject the callback, rather than
guess how to clear unknown memory.

Prepared packets begin at native equilibrium in every channel. `write_stereo`
accepts only sequential chunks with equal left/right lengths and bounded extents.
The first two channels receive the stereo program; all additional channels remain
at equilibrium. A write failure is sticky and clears all valid planes. The sink's
`clear` resets the chunk position but never removes that failure. Inactive,
incomplete, abandoned and cancelled packets clear on drop, even if every chunk
was previously encoded. Successful `complete` consumes the writable packet and
returns an opaque completion retaining its byte borrows, so it cannot authorize
a different packet or subsequent byte mutations. Completion means complete
encoding only. The token remains revocable: explicit `cancel` or abandoning the
completed token clears all planes. The future native owner must perform its final
authorization check immediately before consuming `handoff`, which preserves the
bytes through callback return. This is a bounded software handoff; already
returned native output cannot be recalled by this encoder.

The conversion policy matches the portable WASAPI encoder. All nonfinite input
becomes zero, and finite input clamps to [-1, 1]. Floating output preserves finite
negative zero. Signed integer conversion uses an f64 intermediate, scales by
2^(significant_bits−1), rounds nearest with ties away from zero, then saturates to
the signed range. Only significant bits are retained and all padding bits are
canonical zero, including for negative values. Unsigned 8-bit conversion uses
`round(value * 128 + 128)` saturated to [0, 255]. Its silence is **128**, including
extra channels and inactive/cancelled callbacks; every other supported native
representation uses zero bytes for equilibrium. Single-byte endianness flags do
not change the result.

The packet operations allocate/free no application memory, acquire no locks,
wait on nothing, and do not alter native descriptors. Layout storage is bounded
and reported by `storage_bytes` using actual vector capacity. Native PCM and
descriptor storage are caller-owned and must be included separately in the
future owner budget. The portable encoder owns no additional PCM arrays.

Offline controls assert independently specified bytes for every supported
encoding family, endian order, signed extrema/rounding, nonfinite samples,
padded valid bits, mixed native streams, planar/multichannel silence and unsigned
equilibrium. They also cover untouched malformed extents/ASBD rejection,
sequential partition equivalence, sticky failure, incomplete/drop/final-chunk
cancellation, and zero allocation/deallocation at the maximum bounded planar
shape. These tests qualify the portable transformation only.

The representation contract was checked against the installed Apple SDK
`CoreAudioTypes.framework/Headers/CoreAudioBaseTypes.h` descriptions of
`AudioStreamBasicDescription`, `mBitsPerChannel`, packed/aligned flags, and
noninterleaved buffers (macOS 26.1 SDK SHA-256
`f58c4e2bccc86193afa89160bba76ab57989138e8885bed1c068d41ebe2b5c86`).
Native output ownership requirements and the separately
reviewed SDK identities remain in `OUTPUT-OWNERSHIP-DESIGN.md`.
