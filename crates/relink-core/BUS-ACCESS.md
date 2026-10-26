# Read a bus without rendering twice

`CompiledMixer` renders every bus once per call, including buses omitted from the
legacy output descriptors. A host can resolve its output routes before activation,
render with an empty destination list, then read the resulting bus frames:

```rust
// Control thread, after compiling this mixer:
let bus = mixer.resolve_bus(bus_id)?;

// Render callback; sources and destination transports are already allocated:
mixer.render_sources(sources, &mut [], frames)?;
let post_dsp = mixer.rendered_bus(bus)?; // &[[f32; 2]], exact frame count
// Copy/convert post_dsp into each destination for this bus without rerendering.
```

`resolve_bus` performs the UUID search off callback and returns `UnknownBus` for
an absent ID. `BusHandle` is an opaque, copyable graph token and bus index.
`rendered_bus` checks that token and index in constant time, then borrows the
existing interleaved stereo storage. It does no UUID search, allocation, copy,
locking, atomic operation or DSP work. The host's subsequent fanout copy is
separate work proportional to its chosen frame and destination counts.

The read view contains post-control, post-DSP, post-mute/solo bus samples. It is
not a new planar allocation and does not apply device-format conversion or an
extra full-scale clamp. Multiple destinations may borrow the same block, and
multiple buses may be borrowed together. Reads never advance an oscillator,
ramp, processor or meter. Rust's shared borrow prevents another mutable render
while the view is in use.

| Most recent operation | Read result for a valid handle |
| --- | --- |
| Compilation, no render yet | `NoSuccessfulRender` |
| Successful render of N frames | Exactly N stereo frames |
| Successful render of zero frames | Empty slice |
| Rejected render attempt | `NoSuccessfulRender`; no old block is exposed |
| Successful shorter render | Only the shorter extent; no previous tail |
| Apply or reject a control bank | Previously rendered samples remain unchanged |

Every render attempt invalidates the readable extent before validation and
publishes it only after complete success. Rejection still leaves caller-owned
output buffers, oscillator phase and DSP/ramp state untouched. A zero-frame
success advances no audio state. Hosts must treat a rejected render as a failure;
this accessor supplies no fallback audio. Buffer contents may remain allocated
internally after invalidation, but cannot be read through this API.

Handles survive moving their mixer. They are rejected with `InvalidHandle` by a
different compilation, even one made from the same session. Resolve routes again
when replacing a graph, and persist stable bus UUIDs rather than these handles.
A checked monotonic token is allocated only during compilation and is never
wrapped or reused; identity-space exhaustion returns a compilation error. The
token and valid-extent fields are included in the existing `size_of` graph-budget
accounting. There are no new frame buffers, dependencies or session-format fields.

Offline tests cover post-DSP samples on two buses, omitted output descriptors,
same-bus reuse, foreign handles, moves, zero/short/rejected renders, exact tone and
ramp progression, independent filter recurrence, and zero allocation/deallocation
for rendering plus all view success/error paths. This is a core prerequisite for
fanout. It does not implement or qualify simultaneous native device playback,
queue publication, device clock synchronization or per-device control acknowledgments.
