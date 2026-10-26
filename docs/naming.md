# ReLink source and compatibility names

The maintained Rust crates use `relink-*` directories and package names. Rust
imports use `relink_*`. The desktop executable is `relink-app` (`.exe` on
Windows), and distributable applications remain `ReLink.app` and `ReLink.exe`.
Icon assets use `packaging/branding/relink.*`.

The archived Swift source and test folders also use `ReLink*` names. Explicit
paths in `legacy/Package.swift` retain that prototype's original module names;
the archived implementation is not the maintained desktop application.

Some identifiers deliberately retain their original spelling:

- The existing `RaveLinkRust/session.json` data location keeps saved studios
  available. Opening a studio does not migrate or rewrite it merely because
  the source folders were renamed.
- macOS application and HAL bundle identifiers, cable UIDs, model identifiers
  and the HAL factory symbol preserve installed-device and permission identity.
- Windows hardware and service identifiers preserve the existing device identity.
- Previously published observation protocol identifiers and environment guards
  continue to identify the same protocols.
- Anonymous historical commit identities, accepted release assets, pinned legacy
  driver inputs and frozen evidence retain their original bytes and paths.

These compatibility names are not instructions to create old-named source
folders. New source, build outputs and documentation links use ReLink names.
