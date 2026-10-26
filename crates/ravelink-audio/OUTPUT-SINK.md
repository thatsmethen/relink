# Whole-callback rendering boundary

`RenderState::write_to` renders one complete callback into a borrowed `StereoSink`.
The sink validates its complete frame extent, clears every channel to the format's
equilibrium, and accepts sequential stereo chunks. Capture preparation and control
bank application still happen once per callback; splitting native planes or PCM
chunks cannot advance the mixer or capture clock twice.

The original CoreAudio and WASAPI output owners now use this boundary in the
single-output runtime. `InterleavedSink` remains an offline test adapter retaining
the former CPAL clamp, `FromSample`, first-channel-pair mapping and extra-channel silence.
Invalid buffers, failed encoding, render failure and cancellation clear the whole
callback. A nonempty, fully rendered callback publishes the graph's accepted
control target and returns its frame count and version. Priming, empty, rejected
and cancelled callbacks do not acknowledge new targets. This is graph completion,
not device submission, settled ramp completion or physical playback.

Native owners check authorization again at their actual handoff and retain
their own submission result. [CoreAudio PCM packets](NATIVE-OUTPUT-PCM.md) remain
default-silent until explicit completion handoff. [WASAPI render leases](WASAPI-RENDER-PACKET.md)
release the full acquired buffer once and use `SILENT` unless complete PCM is
explicitly authorized. Their native owners maintain separate startup activation,
progress and teardown evidence; CPAL supplies discovery and identity/configuration
types. Graph completion remains distinct from either native handoff. The offline
fanout transport is not yet connected to simultaneous outputs.

The earlier portable-boundary checkpoint included four rendering controls covering
a continuous analytical sine through different callback/chunk sizes, failure after
a partial write, cancellation inside copying, and first-callback/rejected-extent
allocation behavior. Twelve CoreAudio encoding
controls and fourteen WASAPI encoding/lease controls cover their separate native
byte and lifetime contracts. Independent source review and focused reruns found no
blocking issues. At that checkpoint, the macOS workspace passed 164 tests, strict
Clippy and formatting checks; Windows MSVC cross-target strict audio Clippy passed,
and dependency notices were unchanged and verified. Those historical checks did
not exercise a native output owner. See [the integrated native-output checkpoint](NATIVE-OUTPUT-VALIDATION.md)
for the subsequent source review, offline tests and preregistered cleanup gate.
Installed native-output startup, callback timing and teardown remain unqualified;
Windows audio runtime execution remains pending.
