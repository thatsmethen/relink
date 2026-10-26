# Native input capture

The shipping Rust runtime accepts `Hardware.device_id` and `Virtual.endpoint_id`
values returned by `AudioRuntime::enumerate_input_devices()`. These are CPAL's
host-qualified CoreAudio device UIDs or WASAPI endpoint IDs. A display name,
default-device alias, or absent ID is never substituted. Restart after a
failure requires an explicit new start; no reconnect or route-following loop
runs in the background. Creating the runtime does not enumerate or open devices.

Input selection is independent of output selection. One native input stream is
opened per distinct input ID; duplicate logical source channels share its PCM.
Mono input is duplicated to left and right. Stereo and wider PCM devices use
channels 1 and 2 only. Input and output formats must expose the exact nominal
session rate. Physical PCM layouts are bounded to 256 device channels and
16,384 frames per callback; the number of logical inputs is memory-derived.
Application/process capture still requires a separate platform implementation.

## Clock transport

Every device has an independently allocated 65,536-frame stereo SPSC queue.
Frames are packed into `AtomicU64` slots; the write cursor publishes a complete
packet with release ordering and the read cursor releases old filter history.
Neither callback allocates, locks, spins, waits, does I/O, or changes a graph.
There is one writer and one reader per queue. The output callback is the master
clock. Captured PCM passes through an original 128-tap Blackman-windowed sinc
resampler, using a shared precomputed 1,025-phase table and interpolating adjacent
phase rows. No third-party resampling implementation was copied or linked.

Both devices request the same nominal sample rate. A low-pass-filtered FIFO
occupancy error drives a bounded PI correction of at most +/-2,000 ppm. This is
asynchronous clock compensation, not general sample-rate conversion. Startup
waits for at least 1,024 frames, or two observed device packet spans plus filter
history if larger. Initial excess prefill is discarded when choosing the first
filter center. At 48 kHz and 256-frame packets, the queue target is 21.3 ms;
actual total latency also includes device buffers and scheduling. After startup,
no individual frames are inserted, removed, repeated, or overwritten to repair a
clock mismatch. Underflow, overflow, or failure to prime within the larger of two
seconds or five maximum output callback periods stops the whole engine and
exposes an explicit fault. The maximum-period allowance covers FIR lookahead and
independent callback phases at supported low rates and large buffer sizes;
native no-progress supervision retains its separate deadline.

Every capture and output callback checks the start cancellation token, generation
and latched fault. Stopping revokes those gates without waiting for an OS call.
Fresh starts allocate fresh queues, so an old stream cannot feed a new session.
Already captured OS buffers and already submitted output cannot be recalled.
A stuck OS call still requires the planned process supervisor for hard recovery.

## Evidence and limits

Offline tests exercise distinct stereo PCM across multiple ring wraps, mono and
wider-channel selection, non-finite sample sanitization, startup/underflow/
overflow, duplicate logical source mapping, cancellation, fresh-start isolation,
and zero application allocations/deallocations in capture plus mix callbacks.
A 60-second simulation at each of -1,000 and +1,000 ppm checks occupancy,
convergence and signal continuity. The filter response is tested at 44.1/48 kHz
from 20 Hz through 20 kHz within 0.1 dB and across the correction's folded Nyquist
band below -65 dB. These are algorithm tests, not installed-driver, device,
latency, microphone permission, Windows execution, or long-duration qualification.

Discovery and identity handling were checked against the pinned `cpal 0.18.2`
primary source: `src/traits.rs` (device discovery),
`src/host/coreaudio/macos/device.rs` (device UID),
`src/host/wasapi/device.rs` (endpoint ID/configuration), and
`src/host/wasapi/stream.rs` (the superseded capture callback behavior).
Native host tests must still qualify real callback cadence, permissions,
disconnection, the ASRC servo with real clocks, and operating-system buffer flags.

### Original macOS CoreAudio capture adapter

macOS input now uses original `native_capture.rs` and portable `native_packet.rs`,
with pinned `objc2` SDK bindings. CPAL still supplies input identity discovery and
output. The capture owner resolves the selected CoreAudio UID again and requires
the same AudioObjectID, stream identities, channel order, ASBDs and nominal rate
at activation. It rejects a disappeared/replaced device rather than following a
new object with the same UID during that start. It does not select a default,
change the device's global rate/format/buffer size, or request exclusive access.
Per-IOProc stream usage enables input streams and disables output streams.

Only the current native format is accepted: the exact requested nominal rate,
linear PCM with one frame per packet, contiguous channels, and a fully validated
plane layout. Interleaved, planar and multiple native streams are supported.
Supported samples are 32/64-bit float; signed 8/16/24/32-bit integer (including
explicit high/low-aligned 24-in-32); and unsigned 8-bit. Integer endianness is
explicit. Nonmixable formats are rejected because the HAL can acquire hog mode
automatically for them. Unknown flags, ambiguous widths/padding, unsupported
rates and mismatched stream/buffer layouts fail before activation. Actual callback
sizes may vary within 1–16,384 frames; every enabled plane must contain the same
whole-frame count. A null plane or malformed buffer faults before any PCM borrow.

The direct `AudioDeviceCreateIOProcIDWithBlock` callback checks native input
`mSampleTime`, `mHostTime`, `mFlags`, and valid `mRateScalar`. Every next native
sample position must equal the previous position plus the previous actual frame
count. A strictly sub-frame floating-point tolerance handles arithmetic rounding;
positions too large to preserve single-frame precision are rejected. Missing,
duplicated or overlapping native sample intervals fault the whole engine before
publishing that packet. Host ticks must increase, but elapsed host time is not
converted into an expected frame count: real clock drift does not look like lost
PCM. Native timestamps therefore provide evidence that CPAL's former macOS
capture callback did not expose. This is separate from FIFO/ASRC health checks.

Device and stream property listeners latch removal, identity, rate, layout,
format, abnormal-stop and overload faults. Listeners inspect only the delivered
address list; they never query properties or allocate on the callback thread.
Buffer-size changes are handled through actual bounded packet lengths. Property
notifications are supplied by CoreAudio; this implementation does not invent a
notification delivery deadline or infer an unreported format from PCM contents.

CoreAudio copies and retains both the IO block and property-listener blocks.
The IO block owns an `Arc<Context>` through the interval before callback entry;
the callback does not clone it. A nonspinning entry guard protects the sole
mutable packet state. The dedicated owner revokes acceptance before Stop,
DestroyIOProcID, or listener removal. It drains entered callbacks for at most
250 ms after those calls return. Any failed operation or unproven quiescence
retains the context and reserves that UID until application exit. A blocked OS
call retains the owner on its thread. Reopening that UID cannot create repeated
unquiesced contexts. Command acknowledgement remains bounded; the OS calls
themselves cannot be forcibly timed out safely inside this process.

`CaptureSnapshot.native_timing` retains the latest native packet, the first
rejected packet and its last accepted predecessor, and the most recently
completed authorized callback. The latter contains host ticks at entry, after
PCM processing and immediately before diagnostic publication/return, plus the
unmodified CoreAudio `inNow` sample/host/flags. Completion is an observation
inside the callback, not an OS-observed return time. Fixed lifecycle slots record
IOProc create/start/stop/destroy requests and native return statuses, cancellation,
gate revocation, listener removal, quiescence and retained/completed ownership.
A request without its return exposes an operation that has not returned when
read. Listener removal retains the first failed OSStatus. An unavailable
in-flight count is `None`, not zero. The first rejected-packet pair cannot be
replaced by later packets; property or reentry faults may have no rejected-packet
pair. Metadata preserves original host ticks without conversion or filtering.

Each snapshot performs one bounded atomic read, without retries. A concurrent
writer may yield an explicit contended/missing latest record. Packet, completion
and lifecycle records have independent publication boundaries and include packet
indices or phase identities; they are not claimed to be one simultaneous global
observation. All fixed metadata storage is included by `Ring::bytes()` in the
runtime graph budget. Stopped telemetry keeps these records. Fresh starts use
fresh records and queues, and never continue an earlier sample-time anchor.

Offline controls invoke copied native blocks with synthetic PCM only. They check
the exact observed 568-frame gap, expected PCM, variable callback sizes, malformed
planes, native flags, stale epochs, cancellation, reentry, allocation-free first
callback and metadata publication, property-change rejection, copied-block
lifetime after owner drop, and context/UID retention when quiescence is unproven.
Portable timing controls simulate 30,000 variable-sized packets at each of
±1,000/±2,000 ppm host-clock drift. These tests do not open devices or establish
installed-driver cadence, real teardown behavior, microphone permissions, or a
production qualification. Earlier hosted outcomes remain unchanged.

The disposable-runner `runtime-capture-probe` writes additive
`runtime-native-timing.jsonl` observations beside the existing CSV and phase
events. Every float includes its original IEEE-754 bits; a nonfinite numeric
value is JSON `null`, preserving invalid timestamp evidence without invalid JSON.
After stop (including active-run failure cleanup), the probe observes owner
diagnostics for up to two seconds, ending early at a completed/retained owner.
This is a bounded diagnostic window, not an acceptance threshold. The final
record says whether a terminal owner phase was observed. Existing acceptance
CSV rows and phase timing/PCM thresholds are unchanged. Extra diagnostic writes
run on the probe control thread and may affect scheduling; the instrument does
not claim zero observer overhead. If startup fails before `AudioRuntime` publishes
its capture telemetry, the public snapshot may contain no native records; the
error and absence are retained rather than presented as successful teardown.

Primary contracts: the installed Apple SDK's `AudioHardware.h`, `HostTime.h`, and
`CoreAudioBaseTypes.h`, and these Apple references:

- [Audio device IO block and timestamp parameters](https://developer.apple.com/documentation/coreaudio/audiodeviceioblock)
- [Create an IOProc with a retained block](https://developer.apple.com/documentation/coreaudio/audiodevicecreateioprocidwithblock(_:_:_:_:))
- [Native sample time](https://developer.apple.com/documentation/coreaudiotypes/audiotimestamp/msampletime)
- [Audio timestamp validity flags](https://developer.apple.com/documentation/coreaudiotypes/audiotimestampflags)

### Original Windows WASAPI capture adapter

Windows capture now uses first-party `wasapi_capture.rs` and portable
`wasapi_packet.rs`. CPAL still supplies explicit endpoint discovery/configuration
and output. The native capture worker resolves the exact backend ID through
`IMMDeviceEnumerator::GetDevice`, validates active capture direction and the
requested shared PCM format, installs an event before starting, and registers
with MMCSS Pro Audio. COM interfaces, packet leases and the event stay on that
one worker. Normal waits are bounded to 20 ms; command acknowledgement remains
bounded by the runtime deadline, and teardown never joins a hung driver call.

`AUDCLNT_BUFFERFLAGS_SILENT` packets create zero stereo frames without borrowing
the OS payload, even if its pointer is null or its contents are nonzero. Every
nonempty acquired packet is released exactly once, including invalid packets and
early returns. Frame-position gaps and later discontinuity flags stop capture.
The initial discontinuity flag at position zero is permitted. Timestamp-error
flags and non-increasing packet QPC values cause a separate capture-timing fault.
Fresh starts do not reuse old native streams or transport queues.

The former pinned-CPAL capture path ignored the SILENT flag. This adapter removes
that path from Windows builds. Portable tests exercise silence, malformed data,
position/QPC checks, PCM valid bits, packet release errors, and no application
allocation while converting packets into the ring. Windows-only tests also check
SDK flag values, WAVEFORMATEXTENSIBLE layout values and device-error mapping.
Cross-target compilation is not native Windows execution or qualification.

Primary API contracts used for this original implementation:

- [Buffer flags](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/ne-audioclient-_audclnt_bufferflags)
- [GetBuffer packet and release lifetime](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer)
- [Initialize format and event contract](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-initialize)
- [SetEventHandle ordering](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-seteventhandle)
