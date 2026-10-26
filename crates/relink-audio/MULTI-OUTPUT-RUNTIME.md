# Simultaneous native output runtime — source candidate

Recorded 2026-10-26. The runtime now connects `OutputPlan`, the original fanout
transport and original CoreAudio/WASAPI owners. Offline controls and compilation
are development evidence. This checkpoint has not established installed
multi-device audio behavior, Windows runtime operation, physical alignment,
long-duration reliability, or production distribution readiness.

## Public configuration and observations

`AudioRuntime::start_outputs(session, plan)` and
`start_outputs_cancellable(session, plan, cancellation)` accept explicit routes
and one explicit clock output. Existing `start` methods construct a one-route
plan. All selected endpoints must exist, support the exact nominal session rate
and have a supported stereo-or-larger PCM layout. Endpoint identities are unique;
different endpoints may share a bus. Selection never chooses a system default,
changes its format/rate, or elects a replacement clock.

`RunningConfiguration.outputs` preserves plan order. Each `RunningOutput` has
`output`, `bus_id`, `sample_rate`, `device_channels`, and `sample_format`.
`clock_output_index` identifies the graph owner. Existing singular output fields
remain aliases for that clock route. The selected bus goes to channels 1–2 and
additional physical channels remain at native equilibrium.

`AudioSnapshot.outputs` preserves every route's identity, native timing,
preparation/submission frame counters and optional target-version watermarks.
`ready` means that owner successfully handed off at least one program buffer;
it does not mean sound reached a speaker. Source position, occupancy, startup
target and ASRC correction are exposed for followers. Occupancy is observed at
whole-callback reader entry. A snapshot is a set of individually acquired
observations, not a sample-synchronous multi-device measurement. Observations are
retained after stop or failure, including when native teardown remains unproven.
The legacy `output_timing` field remains the clock owner's timing alias.

The global `applied_control_version` reports graph rendering and complete fanout
publication. Per-route rendered versions report prepared PCM; submitted versions
advance only at that platform's successful native handoff. A follower's initial
FIR prehistory is zero and versionless. Its first valid program buffer can
therefore carry PCM with no accepted version. PCM-versus-silence classification
is independent of version presence on both platforms. These watermarks do not
prove settled ramps, DSP tails, individual coalesced requests, or playback.

## One graph, independent destination clocks

Only the clock renderer owns the compiled mixer, control bank reader and capture
consumers. Native capture identities remain deduplicated. Before a whole callback
consumes capture or advances graph state, the writer checks available space in
all follower queues. The graph renders once per chunk; each follower receives
its selected borrowed post-DSP bus view. The primary receives its selected bus
through the existing planar sink path.

All route PCM and target tags remain unpublished until the whole callback has
completed and one shared release CAS commits the cursor and maximum quantum.
Failed final chunks publish nothing and cancel the complete epoch. A full route
is attributed to its original plan index and fails before advancing the graph.
Followers exclusively own their ASRC reader and full-callback stereo scratch.
They start at source frame zero, retain a delayed program prefix, and use the
existing bounded ±2000 ppm correction. Each has independent queue/FIR latency;
these endpoints are not sample-synchronous. Quantum changes beyond the primed
margin fail explicitly rather than dropping or duplicating program frames.

## Activation, cancellation and ownership

There are separate owner lifetime/local program gates and one common epoch
program gate. All native owners are built closed. Every native Start must return
success and every owner must then produce valid silent progress. Inputs and
independent per-route supervisors start before local activation. Every local
gate is activated while the common gate remains closed; the common gate opens
once only after the complete transaction succeeds. A later endpoint failure
cannot expose an earlier endpoint's program audio.

Each supervisor observes its native progress counter on a separate 2 MiB stack.
The native no-progress deadline remains the larger of two seconds or three
maximum callbacks at the session rate. First program submission has a separate
bounded deadline: two maximum periods for a tone-only clock, five for a captured
clock or tone-only follower, and eight for a captured-source follower, each at
least two seconds. Capture's callback-frame priming limit is also the larger of
two seconds or five maximum periods. Capture and fanout each require two quanta
plus FIR lookahead; independent phases can delay the first graph callback until
period four and a follower until period seven. The extra period provides margin
without allowing progressing silence to hide an indefinitely unready route.
Both wall-clock deadlines are observed at a 20 ms polling interval. The supervisor needs no
control-state mutex or native API call. OS thread scheduling and permanently
blocked native operations still require an external process supervisor.

A per-start `EpochGate` owns the permanent wanted token, common activation state,
and fanout cancellation control. Cancel invokes the same atomic cancellation bit
that the fanout commit CAS compares. Commit-before-cancel publishes one complete
transaction; cancel-before-commit cannot publish. Native owners additionally
recheck authorization at their handoff. Already submitted hardware buffers cannot
be recalled, and a cross-device hardware handoff is not atomic.

The runtime installs its current gate in pinned `arc-swap` 1.9.3. Emergency stop
loads one guard and cancels only the epoch it invalidated, without taking the
control-state mutex or waiting on a device. A concurrent newer installation is
not cancelled by the old generation. Old `StartCancellation` values own their
own gate. Callbacks retain their own gate and never load the ArcSwap slot.
Reclamation is provided by the dependency's guarded Arc ownership; the runtime
introduces no raw-pointer reclamation scheme. See the official
[ArcSwapAny API](https://docs.rs/arc-swap/1.9.3/arc_swap/struct.ArcSwapAny.html)
and [strategy guarantees](https://docs.rs/arc-swap/1.9.3/arc_swap/strategy/index.html).
Slot/library thread-local bookkeeping is outside callback graph payload; native
and allocator-private memory are outside the application's payload estimate.

Output fault attribution now includes the original plan index. Capture faults,
producer/consumer faults, native failures and stalled progress revoke the common
epoch. Source and control history are fresh on restart. Existing native ownership
rules remain in force: pending or unproven cleanup retains its context/reservation;
handle disposal or command timeout does not prove teardown.

## Resource budget and validation boundary

A cheap checked lower bound rejects excessive endpoint requests before discovery.
The exact selected owner payload estimates, every route's supervisor stack,
fanout PCM/tag arrays and shared kernel, follower scratch, route handles,
telemetry and configuration storage are deducted from the 64 MiB working budget
before mixer compilation. Capture owns its separately charged kernel; the
fanout kernel is shared among followers. Endpoint capacity is therefore bounded
by resource cost and native ownership reservations. No new logical source or bus
count limit is introduced. This is an application-owned payload estimate, not a
resident-memory or opaque native allocation guarantee.

Offline tests exercise the same `configure` and `OutputRenderer` paths used by
native owners: distinct and duplicate bus destinations, a clock in the middle of
plan order, one graph/capture consumer, delayed source-zero playback, first FIR
prehistory without ACK, failure after a completed first chunk, full-route
preflight, callback allocation checks, paused-render cancellation with the
control mutex held, paused retired-epoch readers during ArcSwap replacement,
fresh restart state, later-owner activation failure, progressing-silence priming
timeout, and early memory-budget rejection. Existing native packet/lease,
first-fault, owner-lifecycle and original single-output probe controls remain.

The original `examples/runtime-capture-probe.rs` has exactly one compatibility
change: its test-only `AudioSnapshot` fixture gains `outputs: Vec::new()`.
Removing that line and reversing the later `ravelink_*` → `relink_*` import renames reproduces the complete source at `910ef63` byte-for-byte,
SHA-256 `11e72104137dbc503ca2ecaf6b036d5af7ba6cbddcdc17e9df6f085cf95b0f0b`.
No production measurement, threshold, assertion or protocol code changes there.
Existing frozen evidence remains bound to its original source.

The first [independently recorded two-output tone observation](../../research/evidence/rust-multioutput-observation-40257448775/README.md) passed its fixed short protocol for route distinction, controls, Stop silence and both native owner cleanups. This does not qualify captured-source fanout: that needs three endpoints, one independent source and two output destinations. The separate [single-output captured-input observation](../../research/evidence/rust-rename-single-observation-40257398426/README.md) failed with a latched input overload. Captured-source reliability, Windows execution, drift/soak qualification and recovery remain outstanding.
