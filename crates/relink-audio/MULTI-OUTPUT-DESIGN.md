# Simultaneous output design — original proposal

This historical proposal described the former one-output runtime. The current
[source integration](MULTI-OUTPUT-RUNTIME.md) implements simultaneous explicit
endpoints; installed multi-output qualification remains pending.

At this proposal checkpoint the runtime opened one explicitly selected output. This document records
the proposed next implementation. It does not qualify multi-device playback or
change the frozen single-output installed-runtime protocol.

The [bus accessor](../relink-core/BUS-ACCESS.md), pure `OutputPlan` validation
and [offline fanout transport](FANOUT-TRANSPORT.md) are implemented prerequisites.
They are not yet connected to native output streams or the app. The
[native output ownership review](OUTPUT-OWNERSHIP-DESIGN.md) records the remaining
startup, encoding and teardown work.

## Public plan and validation

Add `OutputPlan { clock_output: OutputId, routes: Vec<OutputRoute> }`, where each
route contains one explicit native output identity and one session bus ID. Add
`start_outputs` and its cancellable counterpart. Existing start methods remain
one-route wrappers. Existing `RunningConfiguration` output fields describe the
primary output; additive output configuration/telemetry lists describe every
route. A caller explicitly chooses the render clock.

Validate the complete plan before opening any stream: nonempty routes, exactly
one clock output present, existing buses, supported host-qualified identities,
no duplicate physical output identity, supported bounded layouts, and exact
nominal session sample rate. Multiple physical devices may receive the same bus.
The first implementation sends stereo to channels 1–2 of each device and zeros
additional channels. Multiple bus pairs on a single multichannel endpoint are a
separate feature. There is no default-device fallback or automatic clock election.

## One render owner, independent destination clocks

Only the primary output callback owns the `CompiledMixer`, live control reader,
test-tone/DSP state and capture consumers. Native input IDs remain deduplicated
across logical channels. Each input stream and capture FIFO is opened and drained
once. Secondary callbacks never render the graph or consume input rings.

The core already processes all buses on each render. Add a graph-bound handle to
a bounded read-only view of each bus's last successful post-DSP interleaved
`&[[f32; 2]]` block. Render once with no output copy descriptors, then fan out those
borrowed frames through a precompiled bus-to-route map. This avoids rendering a
source, effect, ramp or meter once per destination.
The primary output receives its selected bus directly. Every secondary output
gets an independent bounded SPSC queue and ASRC reader driven by its own device
callback. Reuse the original 128-tap bandlimited kernel and bounded ±2000 ppm
clock correction after separately validating the output-fanout adaptation.

Follower playback starts at primary source frame zero, with explicit zero FIR
prehistory and fresh version history. Do not reuse capture priming's
`position = written - target`: it can discard the beginning of the program when
the first follower callback is late. Priming may delay playback but must not skip
the queued source prefix. Test an initial impulse with delayed first callbacks,
different callback sizes and the required FIR lookahead.

Start with equal nominal device/session rates. Independent devices still drift;
the secondary queue's occupancy governs its ASRC correction. A queue cannot wait
on another callback, take a mutex, grow, or silently discard/duplicate samples.
FIFO, scratch planes, fixed version tags and kernel coefficients are allocated
before activation. Keep the existing 16384-frame callback and 256-physical-channel
bounds; logical input/bus/route counts are governed by checked memory budgets.

## Publication and controls

Preflight the complete primary callback's capacity in every secondary queue
before consuming capture PCM or advancing graph state. Produce all chunks into
reserved, unpublished space, then release-publish one shared epoch-wide committed
source cursor only after the entire primary callback has rendered successfully
and every queue's PCM and version tags are written. Followers acquire that shared
cursor and cannot read beyond it; independently publishing each queue's cursor
would expose partial fanout if the producer were suspended between stores. A
consumer can only free more capacity during this operation. No fallible render or
queue operation remains after commit. Test producer suspension between queue
writes, failed final chunks and cancellation at publication. Any fault invalidates
the shared epoch; no secondary may authorize PCM solely because its queue
contains data. This transaction controls visibility of source frames, not
simultaneous handoff to independent hardware buffers. A fault racing an already
authorized hardware handoff cannot recall that buffer.

Publish a global control acknowledgment only after the entire primary block has
been processed and committed to every enabled secondary queue. This is a graph
render/publication acknowledgment, not proof that hardware or an acoustic output
has played it. The single-output implementation now publishes its watermark
only after a complete, nonempty, authorized render; priming, empty callbacks and
underflow do not acknowledge prepared controls. Multi-output integration must
add the all-route commit boundary. The 5 ms control ramps advance once on the
primary render clock and may span blocks.

Each secondary reports submitted device frames, consumed primary source position,
priming state, queue occupancy and clock correction. Exact per-device control
acknowledgments need version tags alongside PCM: one u64 per source frame adds
512 KiB to each 65536-frame secondary queue. Its acknowledged version must be the
minimum version across all FIR-support samples contributing to the whole submitted
callback; merely reading a new control tag does not mean old filter history is
gone. Queue epoch is immutable; no old tags or ASRC history survive a restart.
These tags identify accepted control targets, not completed 5 ms ramps or settled
DSP tails. A control bank can coalesce updates before rendering: an acknowledgment
at or above a requested version does not prove that exact intermediate request
was rendered. Publish acknowledgments only for successfully prepared, authorized
program buffers; priming silence or a buffer zeroed after cancellation cannot
advance them.

## Atomic activation, cancellation and faults

Build the complete set of streams with a shared activation gate closed. Start
outputs while their callbacks emit silence, then start captures. Require each
endpoint's native start completion or positive running-callback evidence for this
epoch before opening the gate. CPAL WASAPI `play()` only enqueues `PlayStream`;
its success does not establish that the worker's later `IAudioClient::Start`
succeeded. Validate endpoint evidence, first fault and cancellation before
opening the gate once for the new epoch. The activation gate is separate from
owner cancellation so closed-gate startup does not tear down a starting stream.
Partially successful startup cannot emit program audio. Start success means the
native activation transaction was accepted; secondary priming/readiness remains
separately visible, with a bounded startup failure condition.
Use independent monotonic wall-time deadlines and progress observation outside
callbacks; a missing primary or follower callback cannot be responsible for
advancing its own failure deadline.

Every start uses fresh queues, transport positions, ASRC history and epoch. Stop
first revokes the common gate, then asks owner threads to release native resources.
Callbacks check authorization before work and again before committing each OS
buffer, zeroing a buffer whose authorization was revoked while it was being
prepared. Samples already handed to hardware cannot be recalled. A primary or
secondary device/transport failure stops the complete plan. Do not silently
elect a new primary or degrade to partial playback. A permanently blocked OS
call still needs an external process supervisor; thread cancellation is not proof
of native teardown.

The bounded first-fault attribution checkpoint precedes this implementation so
the endpoint, direction and reporting path survive an all-output cancellation.
Native capture teardown retention does not establish the same guarantee for CPAL
output streams. The pinned macOS `coreaudio-rs` destructor ignores native stop
and uninitialize errors before freeing callback storage; the pinned WASAPI CPAL
destructor joins its worker without a deadline. The implementation must explicitly
represent this limitation or supply an output owner with observable completion.
Keep shared plan resources and explicit endpoint reservations until native owner
completion is proven; a control request timeout is not proof of teardown. Test
that one blocked owner leaves every callback unauthorized and cannot permit
endpoint reuse while ownership remains unresolved.

## Budget and user-visible limits

Deduct every secondary ring, version array, callback scratch plane, ASRC state,
route map and diagnostic record from the 64 MiB graph budget before compilation;
count the shared kernel only once. Reject arithmetic overflow before allocation.
No per-callback allocation, formatting, locks, waiting, retry loops or control
channel sends are allowed in first-party render/transport work.

Independent endpoints are not sample-synchronous. Equal buses share the same
post-DSP source sequence, but each secondary introduces queue/FIR latency and
ASRC interpolation. For small 48 kHz callback sizes the current capture transport
target is roughly 21 ms; larger packets need more buffering. This is a design
reference, not a measured output latency guarantee. Expose actual occupancy and
readiness. Do not promise identical output PCM or wall-clock alignment between
devices without a separate synchronized-clock architecture and qualification.

## Implementation and qualification sequence

1. Add pure plan validation and bounded post-DSP bus access; preserve the old API.
2. Implement secondary queue reservation/publication, ASRC and version semantics
   offline. Test two buses, two destinations for one bus, single capture/graph
   advance, long ±1000 ppm clocks, wraparound, cancellation and stale callbacks.
3. Integrate owner lifecycle/activation and all-output fault gates. Test failed
   starts, missing progress, full queues, fault/cancel races, fresh restart state,
   zero application allocations and checked budget rejection.
4. Integrate explicit UI output/clock selection and separate rendered-versus-
   submitted acknowledgments.
5. Freeze a new installed protocol and independently measure actual macOS and
   Windows output timing, routing, silence, controls and failure behavior.

Material risks remain CPAL output's native ownership/global reconfiguration,
per-route CPU cost, follower latency and FIR acknowledgment delay, and OS calls
that exceed owner deadlines. The earlier failed installed observations remain
unchanged evidence and cannot be promoted by synthetic transport tests.
