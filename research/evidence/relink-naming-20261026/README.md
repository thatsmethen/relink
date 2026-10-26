# ReLink naming migration

Renamed five Rust crate folders/packages and ten archived Swift source/test
folders. Build scripts, package filenames, icon paths, CI filters and current
documentation now follow those names. Existing studio data and native device
identifiers remain compatible; see [the naming policy](../../../docs/naming.md).

The checks in `verification.json` are source and local packaging checks. All
261 workspace tests, nine standalone Windows driver tests, 57 Python observer
controls and 13 dependency audit controls passed. The archived Swift suite passed
with hardware cases skipped. Native and Windows cross-target Clippy, dependency
notices, icon checks, the debug macOS package, the synthetic HAL ABI host and a
local driver package passed. Old generated payloads were absent after rebuilding.

The independent review found no remaining naming/path blockers. Original icon
bytes, observation criteria and frozen historical evidence were preserved.
No installed audio, driver installation or user-studio mutation was performed
for these rename checks. Windows native package CI and installed observations
remain separate results.
