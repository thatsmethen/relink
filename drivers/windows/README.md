# Original Rust Windows virtual audio driver

This directory packages the original `relink-windows-driver` WDM/PortCls implementation. It builds an unsigned x64 native `.sys` image. It has not yet been loaded into a Windows kernel or qualified as a working Windows audio endpoint. See [VALIDATION.md](VALIDATION.md) for the exact evidence boundary.

The production driver is Rust (`no_std`, no external crate dependencies). The C++ file under `tests/` contains compile-only assertions against Microsoft's SDK declarations; it is never linked into the driver. No SysVAD, legacy driver, or GPL implementation was consulted or translated. Source custody and SDK hashes are in [PROVENANCE.md](PROVENANCE.md).

## Implemented contract

- A root-enumerated `Root\RaveLinkAudio` adapter, a WaveRT subdevice named `Wave`, and a topology subdevice named `Topology`.
- One render stream and one capture stream per adapter, stereo interleaved IEEE Float32 at 48,000 Hz. Format negotiation accepts the exact IEEE_FLOAT WAVEFORMATEX or WAVEFORMATEXTENSIBLE representation, including bounded size/rate/channel/mask validation.
- Original COM-shaped interfaces for miniports, streams, adapter power, and adapter PnP; inherited slots are represented explicitly in Rust `repr(C)` tables.
- PortCls creates the normal WDM/KS dispatch path. An original PnP wrapper tears down the adapter on surprise/ordinary removal, then delegates the IRP to `PcDispatchIrp`. Rebalance teardown clears the adapter extension slot so restart can create fresh state. Outstanding stream handles retain their buffers until their final release.
- WaveRT buffers are allocated/mapped/freed using `IPortWaveRTStream` helpers. Mappings are detached under the adapter spin lock before unmap/free. Startup failures unwind previously acquired PortCls registrations and references.
- A monotonic integer QPC timeline, polled positions, a 16,384-frame timestamped cable ring, and a fixed 512-frame delay. Float32 data is copied as integer bits. Each position call processes at most one cyclic buffer per direction; missed older periods are dropped rather than doing unbounded catch-up work.
- Stream buffers are frame-aligned and bounded to 65,536 bytes (8,192 stereo frames). A nonpaged kernel spin lock protects only metadata snapshots and position publication. PCM copying and STOP clearing run outside that lock at the caller’s IRQL. Position queries never wait for another copy; they return the last committed position during contention. Passive controls use kernel events to wait for an in-flight copy before changing or freeing its buffers. Controls and passive copy owners defer normal kernel APCs until ownership ends. There are no driver-owned timers, work items, or audio-path allocations.

Each FDO owns independent cable state. This avoids global shared audio buffers and permits more than one adapter instance. This checkpoint exposes one cable per adapter; dynamic user-created cable management is not implemented here. It does not implement event-driven WaveRT notifications, audio effects, volume/mute topology controls, DRM, offload, ARM64, or arbitrary formats.

## Build on Windows

Use Windows x64 with Visual Studio 2022's MSVC 14.44 toolset and Python 3. Install the pinned Rust toolchains as development tools:

```powershell
rustup toolchain install 1.91.1 --profile minimal --component rustfmt,clippy
rustup toolchain install nightly-2025-11-10 --profile minimal --component rust-src --target x86_64-pc-windows-msvc
./drivers/windows/build.ps1
```

`toolchain.json` pins Rust, the target, and exact Microsoft WDK/SDK NuGet versions and SHA-512 digests. The script retrieves verified Microsoft packages into a temporary build cache, compiles Rust with the bundled LLVM linker, runs host tests, compiles the SDK ABI assertions with MSVC, validates the PE and INF, and generates an unsigned catalog. Output goes to `dist/windows-driver/` with the proprietary license and complete Rust core/compiler-builtins runtime notices from the exact compiler sysroot. `-SkipCatalog` skips only catalog creation, and records that explicitly.

The nightly toolchain is required only because `-Zbuild-std=core` rebuilds core with panic-abort. The prebuilt stable Windows core pulls user-mode exception-runtime references into this kernel build. No fake exception handler is supplied. `_fltused` is an inert linker metadata marker; no floating-point arithmetic is used by the transport.

The GitHub workflow `.github/workflows/rust-windows-driver.yml` performs these build checks without installing the driver or changing signing policy. This crate is currently deliberately excluded from the main desktop workspace and has its own minimal lockfile. Use `--manifest-path crates/relink-windows-driver/Cargo.toml` for host checks.

## Cross-build on macOS/Linux

Extract the exact WDK and SDK packages from `toolchain.json`, preserving their `c/` trees, and install the same pinned nightly Rust tools. A Clang compiler that supports `x86_64-pc-windows-msvc` is required for the compile-only SDK checks:

```sh
RAVELINK_WDK_ROOT=/path/to/extracted/wdk \
RAVELINK_SDK_ROOT=/path/to/extracted/sdk \
  drivers/windows/cross-build.sh
```

This produces a real native Windows driver PE and runs the host/SDK checks, but cannot run Windows INF tools or load the driver. The Windows build remains the packaging authority. Neither build script installs a device, signs a binary, changes Secure Boot/test-signing settings, or starts an audio service.

## Runtime qualification still required

Before distribution, use an authorized disposable Windows test machine with an appropriate signing setup to establish endpoint discovery, shared/exclusive WASAPI operation, loopback fidelity, notification/poll compatibility, bounded latency, repeated PnP stop/start/removal, suspend/resume, and multiple adapter isolation. Measure metadata-lock hold time and total position callback time at maximum buffer size, including DISPATCH_LEVEL calls and contention. Copying outside the lock removes the bulk high-IRQL critical section; source bounds still do not establish deadline compliance. Run Driver Verifier, HVCI checks, relevant audio HLK tests, and the required signing/certification process. These are release gates, not claims satisfied by a successful build.

Build outputs use `ReLink.sys`, `ReLink.inf` and `ReLink.cat`. Both builders
accept `RELINK_OUTPUT` (with `RAVELINK_OUTPUT` as a fallback); an explicit
PowerShell `-OutputDirectory` takes precedence. The installed `RaveLinkAudio`
service and `Root\RaveLinkAudio` hardware identity remain stable.
