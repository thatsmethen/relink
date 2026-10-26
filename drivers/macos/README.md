# ReLink macOS HAL

The independently authored Rust AudioServerPlugIn is in `crates/relink-hal`.
It publishes two independently buffered stereo virtual cables named **ReLink**
and **ReLink 2**. Playback written to each output stream can be captured from
that cable's input stream. Both use 48,000 Hz, packed interleaved Float32, two
channels. Their device UIDs are `au.textual.ravelink.cable.1` and
`au.textual.ravelink.cable.2`; the bundle ID is `au.textual.ravelink.hal`.

The earlier one-cable source passed the fixed independent installed-host
protocol in GitHub run 40039161891 (source `6484db8`, macOS 15.7.9 arm64).
That observation covers basic stereo loopback, two readers and silence after a
fresh start; it does not qualify the subsequent second cable or application
capture/mix path. The dual-cable candidate has synthetic/private-host coverage
and a separate installed runtime protocol awaiting its first observation.
An unchanged-protocol run, **40041055533** at `b30dea6`, subsequently failed on a
568-frame reader timestamp gap; a later run **40041275322** passed. Both outcomes
remain evidence. Intermittent reader continuity is an unresolved release risk.
Sleep/wake, aggregate devices, long-duration behavior and release distribution
remain unqualified. See `tests/SYSTEM-ACCEPTANCE.md`,
`tests/RUNTIME-ACCEPTANCE.md` and the preserved evidence under
`research/evidence/rust-hal-installed-40039161891/`.

## Build and validate

From the repository root on macOS:

```sh
cargo test -p relink-hal
sh drivers/macos/tests/run-native-host.sh
sh drivers/macos/build-bundle.sh
```

The bundle script produces `dist/macos/ReLink.driver` for the current machine's
architecture. Set `RELINK_HAL_TARGET` to `aarch64-apple-darwin` or
`x86_64-apple-darwin` after installing that Rust target to build another
architecture. `RELINK_HAL_OUTPUT` chooses the bundle location. The default
signature is local ad-hoc signing; `RELINK_SIGN_IDENTITY` selects an authorized
release signing identity. These commands do not install anything or restart
audio services. A universal bundle and Developer ID/notarized release installer
are separate release work.

## Behavior and limits

- IDs: plugin 1; Cable 1 device 100, capture 101, playback 102; Cable 2 device
  200, capture 201, playback 202. Each cable has independent clients and storage.
- Sample ring: 16,384 stereo frames, timestamp-addressed; independent capture
  readers. A missing/overwritten/contested frame yields silence. Full Float32 bit
  patterns are preserved without clamping or lossy encoding.
- Capture delay: 512 frames, reported as device input latency. IO calls accept
  up to 8,192 frames; larger calls fail before accessing a buffer.
- Each IO callback performs bounded work with preallocated storage and atomic
  operations. It makes no heap allocations, CF calls, notifications, filesystem
  calls or mutex acquisitions. Each frame publication makes at most one CAS;
  writers that encounter a contested slot drop that frame without waiting.
- Client registration and start/stop use a mutex only on control paths, support
  64 clients, reject unbalanced lifecycle calls, and notify the host when running
  state changes. Neither client bundle strings nor caller-owned pointers are kept.
- The clock uses Mach absolute time and integer-rational conversion to 48 kHz.
  Its 16,384-frame timestamp period exceeds Apple's documented minimum. A single
  timeline lasts for the plug-in lifetime; restarting IO does not rewind it.
- Device/stream format properties are read-only. Reasserting the exact existing
  ASBD/rate is a harmless no-op. Different formats/rates are rejected. No dynamic
  configuration request is made; configuration callbacks reject unsolicited work.
- `device::DEVICES` is the extensibility point for independently buffered cables.
  Up to eight registry entries fit the current bounded property response arrays.
  Dynamic creation, persistence, renaming and application control are not yet
  implemented.

Rust tests exercise ring wrap/overwrite, competing producers and readers, clock
monotonicity, device isolation, malformed properties/formats, lifecycle and
stereo loopback through the vtable, and allocation counting around IO callbacks.
The C host independently compiles the SDK ABI, opens the local dylib and calls
the exported factory and complete IO lifecycle. It checks 1-, 127-, 512-, 4,096-
and 8,192-frame blocks, read-before-write silence, wraparound and repeated IO
restart clearing. Neither test is a system HAL installation test, CFPlugIn
discovery test or real-host latency measurement.

See `PROVENANCE.md` for source origins.

The previous `RAVELINK_HAL_TARGET`, `RAVELINK_HAL_OUTPUT` and
`RAVELINK_SIGN_IDENTITY` environment variables remain accepted as fallbacks.
Installed bundle and device identifiers and the `RaveLinkHALFactory` entry point
remain stable so existing saved routes continue to identify these devices.
