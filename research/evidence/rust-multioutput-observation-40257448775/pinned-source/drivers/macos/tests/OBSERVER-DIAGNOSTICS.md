# Supplemental observer timing and lifecycle records

These instruments were added after the failed installed runtime observation in
run **40042679024**, original source **4fad7fca69415bc86aae17caa5cab2641576a26e**.
That observation retained coincident post-stop callback sample gaps, but its
single post-PCM callback timestamp could not distinguish late callback entry
from time spent executing or preempted inside a callback. The original archives,
reports, protocol and failures remain unchanged. These additions have offline
controls only; they have not yet been observed against installed devices.

## Opt-in independent C diagnostics

Set `RAVELINK_OBSERVER_DIAGNOSTICS=1` on a future disposable-runner harness
invocation. Each independent C player/recorder then receives
`--diagnostics <name>-diagnostics.json`. The ordinary waveform, timestamp CSV,
report and acceptance thresholds retain their existing meanings. Diagnostic
records are additional evidence, and an incomplete diagnostic observation fails
the enabled observation. The flag does not bypass the runner-only installation
guard. The subsequent diagnostic workflow explicitly enables this flag for both installed jobs. No audio was run locally for these controls.

For each accepted PCM callback, schema 1 records:

- Raw Mach ticks at IOProc entry, the existing post-PCM observation, and just
  before callback return. The original CSV's post-PCM field must match exactly.
- The OS-supplied `inNow` sample time, host time and flags, including whether the
  pointer was present. Flags determine validity; a nonfinite sample value is
  encoded as JSON `null`. No replacement timestamp is invented.
- The unchanged original input/output sample timestamp remains in the CSV.
  The analyzer reports entry relative to that timestamp, callback work elapsed
  time, and entry relative to a valid OS current timestamp. A negative difference
  can be valid for future output timestamps. Elapsed work includes preemption.

Timing storage has the same 262,144-record ceiling as the original event array
and is allocated and touched before IO starts. Only callbacks that retain PCM
have timing rows. Rejected, closed-gate and target-complete callbacks are not
represented as accepted PCM. An overlapping invocation fails the observer with
fault 8 rather than concurrently modifying its single-owner PCM/event storage.

The observer also registers available properties for device processor overload,
running state, alive state, nominal sample rate and buffer frame size, plus each
input/output stream's physical format. It records availability, registration and
removal OSStatus separately. An unsupported property is explicitly unavailable;
an available property that cannot be registered or removed fails diagnostics.
Notifications retain object, selector, scope, element and callback receipt tick.
They are notifications, not a contemporaneous read of the property's new value.

The notification journal has 1,024 immutable slots, at most 16 addresses handled
per invocation and no overwriting. Multiple publishers reserve unique slots and
release-publish each record. Closing admission and observing in-flight callbacks
uses one sequentially consistent order. The journal records truncated/dropped
addresses; overflow, unpublished reservations and failure to quiesce are failures.
No allocation, locking, file I/O or property query occurs inside either callback.
Listener storage remains alive until process exit, including after an unregister
error, so a late callback can safely observe its closed gate.

Start, stop and IOProc-destroy request/return ticks are recorded off callback.
Their OSStatus results remain in the original report. Listener removal follows
IO teardown. If IO is not quiescent, mutable timing rows are not read or serialized
and the result fails. JSON files use exclusive creation. The parser caps input
at 128 MiB, validates the schema, counts, integers, flags, publication indices,
ordering and exact CSV custody, and retains original sample-gap context.

There is no new scheduling-performance threshold. A diagnostic-integrity pass
does not excuse a PCM or clock failure or establish a cause. Extra clock reads,
atomic publication and listener registration can perturb scheduling; an enabled
diagnostic run cannot qualify the uninstrumented driver's timing performance.
The Rust runtime's separate native timing JSONL comes from the application
adapter; these C observations remain independent of its implementation.

## Independent child deadlines

Previously, each child's deadline was enforced when the orchestrator reached
that child's sequential `finish()` call. Waiting for a longer writer first could
grant a shorter runtime extra time, and an already exited late runtime could be
reported as successful. Original observations are retained as recorded; their
bounded PCM and clock failures are not reclassified by this repair.

Every new child starts a dedicated supervisor immediately. Its deadline is
measured before launch, without waiting for siblings. A timeout sends TERM to
the process group, allows two seconds for the direct child, then kills any
remaining group members and reaps the direct child within five seconds. A child
that exits after its deadline is observed as a timeout even if no signal was
needed. Completion observed late because the supervisor itself was descheduled
is conservatively a timeout; observation time is not claimed to be exact exit
time. Results include deadline, observed completion time, exit code, signal
delivery and supervision errors. A failed child does not cancel other bounded
observers before they preserve their evidence. Existing log paths are refused.

## Offline controls

`python3 -m unittest discover -s drivers/macos/tests -p 'test_*.py' -v`
includes malformed timing/custody, absent OS timestamps, failed listeners,
overflow, duplicate notification indices and uncommitted slots. A C contract
publishes concurrently into the bounded journal, invokes the actual IOProc with
owned synthetic buffers, exercises the actual JSON/CSV writers and parses their
result. It also checks reentrant callback rejection and no overwrite. It never
calls the device-opening main routine or any audio-device API.

Real subprocess controls demonstrate deadline enforcement while waiting for a
longer sibling, independent evidence completion after another child fails,
SIGTERM escalation, cleanup of an ignoring descendant and prior-log protection.
These controls validate the observer, not installed HAL or application behavior.

The local checkpoint passed all 43 Python controls, the strict Apple SDK C build
(`-std=c11 -O2 -Wall -Wextra -Werror`), Python compilation and diff checks. The
synthetic C contract also passed UndefinedBehaviorSanitizer. AddressSanitizer
compilation succeeded, but the contract timed out silently after 15 seconds;
an independently compiled empty `main` also timed out silently after 8 seconds
with AddressSanitizer, while its UndefinedBehaviorSanitizer control exited 0.
Both timed-out processes were killed and reaped. AddressSanitizer execution is
therefore unavailable in this environment and is not counted as a pass.
