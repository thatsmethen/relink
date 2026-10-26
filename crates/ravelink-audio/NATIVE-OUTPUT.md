# Original CoreAudio output owner

The original Rust output owner replaces the CPAL output stream boundary on macOS
when connected by the runtime adapter. Its controls have been exercised with
synthetic native callbacks and lifecycle failures, without opening a local
device. Native startup, device scheduling and installed end-to-end qualification
remain separate requirements. Existing failed installed observations are not
repaired or promoted by these tests.

## Selection and ownership

Selection resolves the explicitly supplied CPAL CoreAudio UID, verifies current
device liveness and the exact nominal rate, and reads the current output virtual
ASBDs. Native stream order must already match contiguous starting channels;
sorting cannot silently reassociate formats with native buffer/usage slots.
`OutputLayout` applies the separately reviewed PCM policy. Native output buffer
configuration must match every selected plane. The owner changes only its own
IOProc stream usage: all output streams enabled and all input streams disabled.
It never changes defaults, nominal rates, virtual/physical stream formats or
process mute properties. Reinspection follows listener registration and precedes
native start.

Before a worker stack is allocated, an exact UID reservation consumes one of 64
fixed process-wide output slots. Pending, active and unproven teardown owners
share that bound. Successful cleanup frees the slot; a retained owner keeps its
slot until process exit. The output registry is separate from the existing input
registry, allowing ordinary duplex use. A pending OS call keeps its thread,
resources and slot; no GUI or public-handle drop joins that thread.

The owner stack is explicitly 2 MiB. `Selection::owner_bytes` charges selection
and vector capacities, context/gate/timing storage and Arc headers, original
block/listener capture payload reserves, UID strings, fixed registry storage,
bounded control-channel reserves and temporary native-property/reinspection
storage. It excludes allocator bookkeeping and opaque OS/backend memory. The
runtime charges its graph/PCM allocations and independent progress supervisor
separately before construction. The budget is an application-storage estimate,
not a bound on native driver allocations or a process resident-memory guarantee.

## Start, activation and callbacks

`start` acknowledges only an actual successful `AudioDeviceStart` return. It
records the completed-callback baseline at that return, then requires a later
valid complete callback in `wait_for_progress`. Callback activity while Start
is blocked cannot fulfill this requirement. Startup callbacks clear every valid
output channel to its native equilibrium, including unsigned 8-bit value 128.
The separate program gate opens only after start, post-return progress and
current authorization are all valid. Failure/timeouts revoke lifetime. The root
runtime's independent wall-clock supervisor detects absence of later progress;
it neither calls native APIs nor depends on the native owner returning.

CoreAudio's copied IO block captures an Arc context. Its lifetime therefore
covers callback entry before the in-flight counter is incremented. A bounded
single-writer CAS protects mutable mixer/tracker state; it never waits or spins.
The callback copies at most 256 native buffer descriptors, verifies the complete
shape, then checks every address/length for overflow and overlapping PCM ranges.
It rejects buffers overlapping their descriptor list or either native timestamp.
Only after all checks pass does it create exclusive mutable byte slices. Native
ABI allocation/readability remains the host's contract; no pointer is retained
beyond the callback.

The callback calls `RenderState::write_to` once for the entire buffer. The
portable encoder clears all extra channels and rejects partial writes. Complete
encoding remains revocable until a final authorization/program-gate check and
the explicit byte handoff. Cancelled, stale-epoch and closed-activation callbacks
still clear every valid plane. This cannot recall a buffer already handed back
to the host and is not an atomic hardware mute guarantee.

Invalid native descriptors are not guessed or dereferenced as PCM. They fault
without touching unknown ranges. A reentrant call might share a buffer with the
active writer, so the losing call cannot safely clear it or form another mutable
borrow. It records the fault and returns; the active writer's final gate clears
its valid buffer. These boundaries are explicit rather than a silence guarantee
for a host violating its memory or callback contract.

## Timing custody and shutdown

The output journal stores unmodified `inNow` and `inOutputTime` sample/host/rate
fields and flags, frame count, epoch and original callback index. Presentation
time is a native future declaration, not capture acquisition or evidence of
physical playback. Accepted callbacks require finite valid timestamps, exact
presentation-frame continuity and monotonically increasing presentation host
ticks. There is no conversion from host ticks to sample count and no fixed host
clock-rate tolerance, so drifting clocks do not manufacture missing frames.
`inNow` scheduling latency is retained separately and does not define continuity.

The first rejected/current and previous accepted packet pair is immutable. Last
packet/completion reads are coherent bounded attempts with explicit contention;
they do not spin. Completion clocks are sampled immediately before publishing
the journal and returning, not after native handoff. Progress counts complete
valid callbacks including closed-gate silence. Graph accepted-version tags,
encoded software buffers and actual playback remain distinct observations.

Property listeners retain the actual object/selector/scope/element and separate
overload from format/device/lifecycle reporting. Their callbacks perform no
device query, formatting or allocation. All native return statuses for
create/start/stop/destroy and listener removal are retained in fixed lifecycle
slots, with the first failed removal also attributed to its original address.
Handle-cancel events use an absent in-flight count because the handle does not
own the callback context; owner-side counts are actual atomic observations.

Shutdown revokes gates before Stop, Destroy or listener removal. Every operation
has distinct requested/returned evidence; a blocked call lacks its return event.
The owner then waits at most 250 ms for callbacks already in flight. Any failed
native operation or unproven quiescence retains context, callback/listener
captures and UID reservation. Successful cleanup drops these resources and
releases the reservation before recording `OwnerCompleted`. Retention is a
bounded safety fallback, not recovery or a successful stop claim.

## Offline controls

Synthetic tests cover post-native-start silent progress and activation, native
stereo/extra-channel output, U8 silence after cancellation/stale epochs, exact
presentation-gap custody, malformed/null/aliased/overlapping buffer rejection,
copied-block late invocation after owner references disappear, reentry without
aliasing mutable state, first-callback/error/cancel zero allocation, property
metadata, each teardown failure, late starts, in-flight drainage, blocked-owner
reservations, actual cleanup-versus-retention behavior, fixed registry
exhaustion/reuse and storage estimates. Portable journal tests cover timestamp
flags/precision/drift, immutable first fault, coherent concurrent snapshots,
preserved float bit patterns and zero allocation.

SDK contracts and reviewed source identities are recorded in
`OUTPUT-OWNERSHIP-DESIGN.md`; portable PCM behavior is in `NATIVE-OUTPUT-PCM.md`.
No legacy driver or application source was consulted for this implementation.
