# Capture owner completion

`CaptureSnapshot.owner_status` is a per-capture-ring lifetime observation. It is
independent of packet progress, a successful Stop request, event-handle closure,
or a native lifecycle journal's terminal marker.

- `NotStarted`: this ring has not entered owner construction.
- `Pending`: construction, activation, capture, cleanup or a registered dependent
  owner still exists. A native call that never returns stays pending.
- `Completed`: all registered participants returned or were destroyed after
  releasing their owned resources, and none reported retained cleanup.
- `Retained`: cleanup failed, ownership was quarantined, or a worker unwound.
  This state is terminal; subsequent successful drops cannot erase it.

The ticket is allocated with the ring before native construction and remains
available in retained telemetry after a timeout, error or handle cancellation.
Each construction attempt starts one participant before reserving/spawning the
worker. Failed spawn destroys the captured resource closure before finishing
that participant. Normal completion consumes the whole closure, including all
local/field drops, before publishing completion. Registration cannot reopen a
completed ticket. None of these operations acquire the runtime control mutex.

CoreAudio additionally counts the callback Context and listener Gate lifetimes:
OS-copied blocks can hold these after local owner teardown. A successful native
journal marker alone therefore does not imply the ticket is complete. Failed
Stop/destroy/listener/dependency cleanup or reservation release latches Retained.

Windows hardware capture completes only after its native services, client,
event, MMCSS guard, apartment and reservation are released. Process capture also
counts its asynchronous activation completion handler and membership worker;
the latter can still be checking the selected process scope after capture stops.
A never-completing activation stays Pending even when no event was ever created.
Stop/event failure or an unreleased reservation latches Retained. Existing fault
and native status evidence is preserved separately.

Sound Check must retain its recording until every attempted capture ticket and
the output owner's separate completion proof finish successfully. Pending and
Retained cannot enable playback or reclaim callback-owned data. An unattempted
ring is not proof of a successful recording.
The full native Sound Check record/play workflow is not yet connected by this
checkpoint.

The ticket allocation is charged by `Ring::bytes` and conservatively reserved in
native owner budgets too, because reservation metadata may outlive the ring.
Participant fields are inline within their already-budgeted contexts, handlers,
reservations and owner stacks.
No new native calls, default-device changes, permissions or retry behavior are
introduced. Probes serialize the additive status without changing their frozen
PCM or cleanup acceptance criteria.

Offline controls cover resource/guard ordering, failed spawn closure disposal,
blocked owner plus membership worker, sticky retention, unwind, event-close
insufficiency, copied CoreAudio blocks, reservation rejection and native cleanup
failure. These controls and cross-target checks do not establish native runtime
qualification on either operating system.
