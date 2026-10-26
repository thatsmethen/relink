# ReLink archived Swift prototype

The source and test folders use the ReLink name. The archived Swift module names, protocol identifiers and historical records retain their original names for compatibility. The supported Rust workspace is at the repository root.

A native macOS audio router and mixer, developed using Elgato's documentation and [morluto/rea](https://github.com/morluto/rea) inspection of pinned Wave Link artifacts. Independently implemented; no Elgato code or branding is distributed.

**Status: in development. Not yet a production release.**

Inputs and mix buses are user-created without a product-imposed channel count. Actual capacity depends on the audio hardware, driver, CPU and memory.

## What is implemented

- A native routing matrix with independent send level, pan, mute, solo and mix masters.
- Multiple sources per input and multiple output devices or stereo pairs per mix.
- Built-in gain, high-pass, gate and compression, followed by ordered Audio Unit effects with saved settings and bypass.
- Sound Check: capture a dry sample in memory and loop it through the effects to an explicit listening output. Opening Sound Check pauses the studio; closing it leaves audio stopped.
- A supervised audio helper with bounded control messages, startup/stop deadlines and process failure recovery. Audio never starts automatically on launch.
- GPL virtual-device builds from a pinned BlackHole source revision, with distinct device identities and corresponding source included.

Application/system capture, device lifecycle, sustained scale and distribution still have open acceptance gates. The [release ledger](docs/RELEASE-ACCEPTANCE.md) distinguishes implementation, deterministic checks, native UI observations and independent installed-driver recordings. Debug and release scale runs failed; their original evidence is retained. Shared capture corrected the measured duplicate-source level cancellation, but the latest installed run still failed continuous-audio checks. There is no production capacity guarantee yet.

## Run a development build

```sh
scripts/build-app.sh
open build/RaveLink.app
```

Select an explicit output for a mix, add an input, enable the desired route, then choose **Start audio**. New routes start disabled. For a receiving app such as OBS, choose a virtual device as the mix output and select that device as the receiver's audio input. The app does not change system default audio devices.

See the [virtual-device guide](docs/virtual-devices.md) for source builds and installation, the [audio engine notes](docs/audio-engine.md) for routing constraints, and the [signing boundary](docs/application-signing.md) for distribution requirements. Development bundles are ad-hoc signed unless an existing Developer ID identity is supplied; they are not notarized releases.

## Development

Requires macOS 15+ and Swift 6. `swift build` builds the application; `scripts/test.sh` runs the Swift Testing suites. The test wrapper supplies framework paths for standalone Command Line Tools and uses normal discovery with full Xcode. On CLT distributions missing the Foundation testing overlay module, it disables automatic cross-import overlays for the test build. Runtime and virtual-device acceptance are tracked separately in `docs/` and [research/ACCEPTANCE.md](research/ACCEPTANCE.md).

Installed-driver acceptance runs only on a disposable Mac with the named test devices. CI builds this job in release configuration and retains binary hashes, raw recordings, analyzer output and failed-run evidence. Local unit tests skip hardware cases unless their explicit environment switches are set.
