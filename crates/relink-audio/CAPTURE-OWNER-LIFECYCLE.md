# Capture owner admission and lifecycle checkpoint

This checkpoint changes application-owned capture resources and the macOS native
owner lifecycle. It does not change PCM decoding, the capture transport, ASRC,
output ownership, or the installed runtime measurement protocol. The controls
below use synthetic callbacks and injected native operation results; they do not
establish installed-device reliability.

## Admission and resource accounting

The macOS input registry has 64 slots. An exact UID is reserved before its worker
thread is created, so a worker blocked in discovery or native creation cannot be
bypassed by another start for the same input. Failed creation releases its slot
when ownership unwinds; a call that has not returned still owns its slot. A
poisoned registry, duplicate UID, full registry, or UID exceeding 64 KiB is rejected.
UIDs are neither truncated nor normalized. Input and output registries are
separate, permitting the same duplex device to own one input and one output.
These endpoint limits do not cap logical input aliases in a session.

Both CoreAudio and WASAPI capture threads request a 2 MiB stack. Before compiling
the graph or creating capture workers, `capture::Selected::owner_bytes` calculates
checked owner estimates and adds them to the existing 64 MiB graph budget. The
macOS estimate includes worker/handle/context/gate storage, selection and layout
copies, listener and copied-block payload reserves, bounded channel metadata,
registry storage and UID payloads, and bounded reinspection/property storage.
Windows includes the stack, native/handle/state/tracker values, channels, ID/name
copies and UTF-16 endpoint storage. Selection/configuration metadata is also
charged. Sum or multiplication overflow rejects the start.

The ring, ASRC kernel, capture render scratch, and render source aliases retain
their existing independent charges; owner estimates do not add a second ring or
kernel. Estimates are conservative application admission accounting, not a
measurement of process RSS: native driver/COM/CoreAudio allocations, allocator
metadata, OS stack guard pages, and caller-owned snapshot copies are not known
from these APIs. Retained owners from older epochs remain subject to the fixed
macOS process registry, rather than being counted as part of a new graph's
per-start budget.

## macOS teardown and late returns

Before Stop, DestroyIOProcID, or listener removal, the owner revokes its alive and
accepting gates. Every applicable operation is attempted even when an earlier
operation reports failure. The first failing native status and, for listener
failure, the exact object/property are retained. Raw return statuses remain in
the existing lifecycle observations. A bounded callback drain follows.

Only success from every applicable native operation plus zero in-flight callbacks
allows completion. The application then releases listeners, its copied IOProc
block, the callback context, and the UID reservation before publishing
`OwnerCompleted`. Completing a successful teardown twice is a no-op. This event
means application ownership was released under the native API success contract;
it does not claim that no other native owner or externally copied reference exists.

If any operation fails or callback quiescence is unproven, `ContextRetained` is
recorded instead. The application keeps the callback context, callback block,
listener blocks and their captured references alive, and leaves the registry slot
as a tombstone. Subsequent callbacks cannot publish. The same UID cannot reopen
and distinct failed UIDs cannot create an unbounded number of retained owners.
The retained resources remain until process exit; no unsafe retry or reclamation
is attempted.

If AudioDeviceStart returns after cancellation, its successful native status is
still recorded, but activation fails and the accepting gate is closed. The owner
then follows the normal teardown path. Control-thread timeouts still revoke the
handle without joining a worker blocked inside a native call.

## Verification and limits

The macOS controls exercise all 64 registry slots and reuse, duplicate admission
while an actual test worker is blocked, late Start return after handle/epoch/alive
revocation, Stop/Destroy/each listener failure, multiple cleanup failures and
first-status preservation, skipped nonexistent handles, callback drain failure,
retention of all application ownership, and release-before-completion. A listener
capture destructor checks that completion is not already published. Synthetic
failed-ownership fixtures recover only their own deliberately retained references
after verifying the production branch; they never reclaim real native ownership.
Existing malformed-packet, timing-fault, format-notification, stale-epoch and
callback-allocation controls remain applicable. Budget controls cover checked
addition/multiplication failure and rejection at the combined graph/owner limit.

Windows changes here are limited to stack sizing and application-owner accounting.
Its capture worker does not yet have the matching fixed reservation/tombstone
policy, complete raw lifecycle diagnostics, or proven teardown ordering under
all native failures. Cross-compilation and lint do not qualify Windows runtime
capture, and this checkpoint makes no such claim. Installed macOS tests, blocked
real-driver behavior and restart/recovery/soak qualification remain separate work.
