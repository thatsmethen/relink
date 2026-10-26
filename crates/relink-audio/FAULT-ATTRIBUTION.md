# First-fault attribution

`AudioSnapshot.fault` retains its existing numeric `AudioFault` values.
`first_fault` adds the first reporting path, input/output/runtime direction,
endpoint index, native endpoint ID when known, and fixed native metadata.
One compare-exchange publishes the code, origin, direction and endpoint index
together. This immediately revokes callback authorization. Only that winner can
publish metadata, with a release/acquire ready flag; no later reporter can replace
or complete it. A nonblocking snapshot may expose `metadata: None` while the
winner is suspended. It never guesses absent metadata and never retries a read.
"First" means the first successful claim, not proof of earliest physical onset.

The record is inline in `Telemetry`; its complete fixed storage is deducted from
the graph budget. Endpoint strings are resolved/copied only by the control-thread
snapshot. Application callback recording uses bounded atomics and native host
clock reads; it has no allocation, formatting, locks, waits or spins. Backend
error strings are deliberately not copied. CPAL's own internal behavior is
outside this claim. Fresh runtime telemetry creates a fresh latch per start.

## Source distinctions

| Reporting path | Origin | Raw context |
| --- | --- | --- |
| macOS invalid native packet/sample gap | `NativeInputPacket` | Input index, epoch, object ID, Mach observation ticks, packet index linking the retained native fault pair |
| macOS callback reentry guard | `NativeInputReentry` | Input index, epoch, object ID, callback-entry Mach ticks |
| macOS input overload notification | `NativeInputOverload` | Input index, epoch, registered object ID, actual notified selector/scope/element, observation ticks |
| Other macOS input property notification | `NativeInputProperty` | Same native property context; malformed address count has no guessed address |
| Native input start/teardown failure | `NativeInputLifecycle` | Object/epoch/time and OSStatus where exposed; existing lifecycle journal remains authoritative for each operation |
| Explicit CPAL CoreAudio output Xrun | `CpalCoreAudioOutputOverload` | Output zero, epoch, observation ticks; CPAL does not forward native object/address/status |
| Other CPAL output error | `CpalOutputError` | Explicit output, epoch, mapped original fault code |
| Capture producer / consumer | `CaptureTransportProducer` / `CaptureTransportConsumer` | Unique input index, epoch, observation ticks on macOS |
| Missing input progress | `CaptureStartup` | First unready input in sorted capture order; this is not a claim that no other input is unready |
| Invalid output / mixer controls / render | `OutputBuffer` / `MixerControls` / `MixerRender` | Output zero and epoch |
| WASAPI input | `WasapiInput` | Input index/epoch; HRESULT for activation, count, acquisition and release errors when that error wins |

Mach ticks are sampled at observation and are not converted or compared across
clock domains here. Native packet timestamps remain in `CaptureSnapshot`'s
existing native timing record. Windows observation time and native property
fields are explicitly unavailable in this checkpoint. Windows packet position/
QPC/flags are still validated by the existing tracker, but a retained Windows
packet journal is not implemented. Synchronous startup failures retain the
existing returned `AudioError` behavior; this checkpoint does not publish a
failed activation as a running configuration or redesign its lifecycle.

The CoreAudio output-overload attribution is specific to pinned CPAL 0.18.2:
its `src/host/coreaudio/macos/mod.rs` device monitor invokes `ErrorKind::Xrun`
from its `kAudioDeviceProcessorOverload` listener. Its default-output helper also
maps that selector to Xrun; this runtime uses explicit device IDs. This identifies
the reporting path, not CPU starvation or the underlying hardware cause. The
Apple SDK `CoreAudio.framework/Headers/HostTime.h` declares
`AudioGetCurrentHostTime` as `CA_REALTIME_API`.

## Evidence and controls

The runtime probe adds `first_fault` to the existing JSONL record. It retains
large integer ticks, signed status, raw property selectors, endpoint identity and
an explicit `metadata_pending` flag. Existing CSV fields, PCM tests, acceptance
thresholds, deadlines, queue capacity and ASRC policy remain unchanged.

Controls cover concurrent first-winner identity/metadata consistency, a suspended
winner, absent native fields, no-fault rejection, callback allocation counting,
native gap/reentry/overload/property distinctions and JSON preservation. They
open no audio device. These synthetic controls do not establish installed timing,
device teardown or production reliability. In particular they do not retrospectively
attribute the ambiguous Xrun in installed run `40046411757`.

At the 2026-10-24 implementation checkpoint, all 106 workspace/all-target tests
passed, including 58 audio-library and four probe tests. Independent source
review repeated the 58 + 4 tests and found no blocking issue in the first-winner
publication, native reporting paths, endpoint resolution or Windows error paths.
Strict workspace Clippy and formatting passed on macOS; the audio crate also
passed strict Windows MSVC cross-target Clippy. Cross-target compilation is not
native Windows execution. No dependency versions or graph edges changed.

Checkpoint validation on 2026-10-24:

| Command | Result |
| --- | --- |
| `cargo test --locked -p ravelink-audio --lib --example runtime-capture-probe` | 58 library tests and 4 probe tests passed on aarch64 macOS |
| `cargo clippy --locked -p ravelink-audio --all-targets -- -D warnings` | Passed |
| `cargo clippy --locked -p ravelink-audio --all-targets --target x86_64-pc-windows-msvc -- -D warnings` | Passed cross-target compilation; no Windows execution |
| `git diff --check` | Passed |

No dependency or lockfile changes were required. Independent review and a fresh
installed observation are separate from these local results.
