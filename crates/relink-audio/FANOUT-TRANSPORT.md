# Output fanout transport

`src/fanout.rs` is the original portable transport for simultaneous outputs.
The [native runtime integration](MULTI-OUTPUT-RUNTIME.md) now connects it to
`AudioRuntime` and original native owners. The controls below remain offline
transport evidence; they do not establish installed multi-device playback.
It changes no capture FIFO, capture ASRC arithmetic, installed-run threshold, or
acceptance protocol. The existing bandlimited Kernel only gains read accessors.

## Ownership and API

`Fanout::build(routes, sample_rate, epoch, budget)` constructs one `Writer`, one
`Reader` per destination and a terminal cancellation `Control`. Every route owns
65536 stereo slots and 65536 u64 accepted-target-version tags. Readers share one
Kernel allocation. Equal nominal rates are required; valid rates follow the
core's 8000–384000 Hz range. Physical native configuration is outside this module.

The primary caller must successfully obtain `writer.begin(frames, version)`
**before** advancing its graph or input consumers. This checks capacity for the
whole callback on every route. `RouteFull { route }` identifies the first full
route in plan order; source-clock exhaustion is a separate `Overflow`. All memory
and per-route progress storage are allocated before callbacks. Version numbers
are nondecreasing between successful transactions; coalesced requests need not
appear in PCM tags.

The borrowed `Transaction` accepts interleaved stereo chunks via `write(route,
frames)`. Each route must receive exactly its reserved length. Invalid writes
poison the transaction even if a caller ignores the error. A failed, incomplete
or dropped transaction publishes nothing. An aborted attempt may dirty free
slots; the next attempt must overwrite every slot it publishes, including after
wrap and when retrying with a shorter callback. This does not roll back a graph
that a caller has already advanced. Native runtime integration must cancel the
complete plan after an unrecoverable render/transport error.

## One publication boundary

One `AtomicU64` contains 48 source-frame cursor bits, 15 bits for the largest
committed producer callback, and one irreversible cancellation bit. Each commit
CAS compares the previous complete cursor/quantum word and release-publishes both
new fields only after all route PCM and tags have been written. There is exactly
one producer and no retry loop. A cancelled-before-commit transaction cannot
publish; a commit-before-cancel remains a complete publication which every
subsequent reader rejects through the same cancellation bit.

The finite frame clock lasts over 23 years even at 384 kHz; overflow is rejected
before any private write. Neither the frame cursor nor epoch is silently reused.
Restarts need a new `Fanout`, including fresh PCM, version, ASRC and cancellation
state. A stale control handle only addresses its own allocation/epoch.

Readers acquire that common publication word and read strictly below its frame
extent. Each route has one released-history cursor, written only by its reader.
The producer cannot overwrite unreleased PCM or tags. Atomic frame/tag loads can
therefore be relaxed after the shared acquire; a future unpublished transaction
has no ownership of those history slots. Per-ring write cursors would not provide
this complete-fanout guarantee and are deliberately absent.

## Source origin, clocks and callback sizes

A reader starts at source frame zero. Negative 128-tap FIR support is explicit
silence with no accepted control version. Initial priming waits for at least
`max(2 * max(largest_committed_producer_callback, follower_callback) + 128, 1024)
+ 64` frames, as well as a conservative whole-callback ASRC lookahead check.
The actual accumulated backlog at that first successful read becomes the fixed
occupancy target. A delayed reader keeps the program prefix and reports its
additional latency; it never jumps to `written - target`.

Clock correction is bounded to ±2000 ppm and uses the original 128-tap,
phase-interpolated bandlimited Kernel with a separate follower occupancy servo.
No discrete sample insertion, deletion, queue growth or silent re-priming occurs.
Before every read, the full callback must fit inside the acquired committed
extent at the maximum allowed correction. Missing data returns `Underflow` with
the whole provided output cleared and without advancing the reader.

Variable callback partitions are supported within the initial buffering margin.
If a later primary or follower quantum requires more margin than the fixed
startup target, `QuantumExceeded` reports the required margin and target before
consumption. Integration must stop explicitly; arbitrary later quantum growth
is not claimed safe. A first reader delayed until near full queue capacity can
also cause a later producer reservation failure even though its prefix was
preserved. Bounded capacity cannot recover an arbitrary scheduling outage.

Each read reports source start/end/fraction, buffered frames, fixed target,
largest committed producer quantum and correction. The final cancellation check
zeros a completed callback if cancellation won while it was being prepared.
This cannot recall an OS buffer already handed off, and it is not a native
device startup/teardown or hardware scheduling guarantee.

## Control version meaning

`accepted_version` is the minimum version across every FIR-support sample for
the entire returned callback. It is absent during empty/priming reads and any
callback whose support reaches zero prehistory. Looking ahead to a newer version
does not acknowledge it while older support remains in the same callback.
This reports targets represented in the submitted source support; it does not
prove a coalesced intermediate request was ever rendered, a gain ramp finished,
an IIR tail disappeared, or a hardware/acoustic output played these samples.

## Resource and verification boundary

`required_bytes` uses checked arithmetic for all PCM/tag payloads, queue/shared
objects, Arc control blocks, vectors, reader/writer/control state, one transaction
and one Kernel allocation. It excludes allocator-internal bookkeeping. The
current constructor allocates its own Kernel once. A later graph-wide shared
Kernel constructor must charge the actual shared allocation once, not once per
route. Budget/configuration rejection occurs before allocations. Callback work
contains no allocation/deallocation, locks, waits, channel sends or retry loops.

Offline controls cover source-zero impulses with delayed readers, same and
distinct route PCM, producer suspension between routes, incomplete/failed/drop
rollback, shorter retry and wrap, both commit/cancel orders, packed exhaustion,
first-full-route identification, startup quantum publication, unequal/variable
callback partitions, explicit quantum-growth failure, FIR version boundaries,
checked budgets, zero allocation, and simultaneous independent ±1000 ppm clocks
for 60 simulated seconds with analytical stereo PCM and repeated ring wraps.

These tests do not qualify native macOS/Windows multi-output timing, lifecycle,
device drift, latency or production reliability. Those require later integration
and a separately frozen installed protocol. Earlier failed observations remain
unchanged.

Local checkpoint validation on 2026-10-24: all 12 fanout controls passed within
the 78-test audio library suite, and all 4 runtime-probe tests passed. The full
audio suite took approximately 15 seconds; simulated clock duration is separate
from that wall-clock measurement. Independent source review found no blocking
issue and repeated all 78 library and four probe tests successfully; strict
Clippy passed on macOS and for the Windows MSVC cross-target. Windows native
execution and native multi-output integration remain separate qualification steps.

The combined checkpoint also includes pure `OutputPlan` validation, the core's
borrowed bus views, and the existing single-output render acknowledgment fix.
All 134 workspace/all-target tests, strict workspace Clippy, formatting and
dependency-notice verification passed on macOS. The acknowledgment change passed
independent review and five focused controls. No native multi-output stream was
opened by these checks.
