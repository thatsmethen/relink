# Original WASAPI output owner

The [simultaneous-output runtime](MULTI-OUTPUT-RUNTIME.md) supplies either the
primary graph renderer or a follower ASRC reader to each owner. Local activation
is additionally gated by the common epoch program gate; optional version ACK
is independent of whether a submitted buffer contains program PCM.

This checkpoint implements a dedicated Rust MTA render owner for an explicitly
selected Windows endpoint. Portable tests and Windows compilation are development
checks. Native Windows startup, scheduling, audio continuity and teardown have not
been executed or qualified by this checkpoint.

The control thread selects an exact CPAL WASAPI identity and supported encoding.
The owner checks the endpoint's returned native ID, render direction, active state,
exact shared-mode format (`S_OK`), and bounded buffer size before starting. It does
not substitute a default endpoint, closest format, nominal rate or channel order.
Formats are F32, F64, I16, high-aligned 24 valid bits in 32, I32 and U8; the original
[portable encoder and lease](WASAPI-RENDER-PACKET.md) retain their contract.

The endpoint reservation precedes native open and survives public-handle drop,
timeout and native blocking. A fixed 64-entry output-only registry bounds retained
identities. A reservation can be released only after native resources have been
released on their owning thread. Failed Stop, failed buffer release or failed
native handle cleanup leaves a reservation tombstone and `ReleasedUnproven`, with
no automatic retry/reopen. A blocked native call leaves `Pending`. A process
supervisor is required to recover from permanently blocked native code.

Lifecycle and program activation are separate atomic gates. Full-buffer SILENT
prefill precedes actual `IAudioClient::Start`; only a complete successful release
*after* Start satisfies startup progress. Activation requires this progress and a
current render permit. Late Start success cannot undo cancellation. Handle cancel
and Drop never join or call native APIs. Control waits have monotonic deadlines;
they cannot interrupt an owner already blocked inside a native call. The runtime's
independent progress supervisor is a separate integration responsibility.

Each event/padding pass obtains at most one complete available-frame lease. No
wait occurs while it is held. The validated byte borrow ends before exactly one
full-frame `ReleaseBuffer`, SILENT on closed activation, cancellation, incomplete
render or malformed acquisition. PCM is committed only after the final permit and
local-gate check. Cancellation after native submission cannot retract that buffer.
Graph-target acknowledgment and successful native submission are different:
submitted frames/version/progress advance only after successful ReleaseBuffer.
SILENT submission and prefill are counted separately. These counters do not prove
hardware presentation, acoustic output or ramp settling.

Diagnostics retain raw Start/Stop/Release HRESULTs, first failure, the most
recent entered/returned operation, fixed lifecycle slots, and raw QPC work entry
and exit ticks with the QPC frequency. Work covers GetBuffer through completed
ReleaseBuffer (or failed acquisition, with no release status), not an audio-device clock or a hardware presentation timestamp.
Adapter validation failures are identified separately and carry no invented native
HRESULT. The Open phase is an aggregate owner-open operation; a reported status
is its failing native call, or no status for a validation rejection. A
concurrent/unfinished work record is explicitly unavailable rather than torn.
The owner does not use `IAudioClock`, endpoint notification callbacks, or recovery
retries in this first slice. Device/service failures are surfaced by native calls
and progress deadlines; there is no notification-based format-change journal.

The Rust budget includes the explicit 2 MiB owner stack, selection/UTF16 data,
reservation identity, fixed registry and Arc/native-wrapper payloads. RenderState
is budgeted separately by its constructor; OS/COM/audio-engine allocations and
allocator bookkeeping are outside this payload estimate. The healthy leased
render path allocates no application storage and takes no application lock. The
endpoint registry mutex is used only before open and after native teardown.

## Provenance

Independently authored from this project's original portable packet/permit
contracts and Microsoft's published interface definitions. No legacy driver,
SysVAD implementation or third-party render-owner implementation was used as a
source template.

- Windows SDK 10.0.26100.0 `Audioclient.idl`, from WDK 26100.6584:
  SHA-256 `145c2d5a33effd1d139ccf11dab73dff05ef82316bae772c8b5e9e72386a836a`.
- Windows SDK `mmdeviceapi.idl`:
  SHA-256 `2c4ddb6711089f823da4fa70fb3674366e7acdea61eefc5eee4ebeb9c014ec57`.
- Pinned `windows` 0.62.2 generated interface types and vtable definitions. Raw
  vtable Start/Stop/Release preserve positive HRESULTs that generated `Result<()>`
  wrappers discard.
- Microsoft contracts: [GetBuffer](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-getbuffer),
  [ReleaseBuffer](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-releasebuffer),
  [GetCurrentPadding](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-getcurrentpadding),
  [Start](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-start),
  [Stop](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-stop),
  [SetEventHandle](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-seteventhandle),
  [QueryPerformanceCounter](https://learn.microsoft.com/en-us/windows/win32/api/profileapi/nf-profileapi-queryperformancecounter),
  [QueryPerformanceFrequency](https://learn.microsoft.com/en-us/windows/win32/api/profileapi/nf-profileapi-queryperformancefrequency).
