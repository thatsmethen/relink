# Local Rust HAL package installation checkpoint

The approved local test package was installed by the user through Apple Installer.
Read-only verification confirmed every installed payload file matches the reviewed
package, its strict ad-hoc signature verifies, and the expected package receipt
exists. The package SHA-256 is
`3d0c22953f79e2264c73290260e813754fdc83cddf52194e67c1cb26eda1bba3`;
the installed driver SHA-256 is
`804c545dd1ac44a25d8727b2bcd51be15ec646f87680de911bf060d8023335e2`.
The destination is `/Library/Audio/Plug-Ins/HAL/ReLink.driver`.

Neither owned virtual cable was loaded at the subsequent read-only preflight.
Both exact UID queries returned zero matches. This verifies installation bytes,
not loaded-device or runtime operation. No audio service restart, local recording,
playback, system-default change or macOS recording permission grant was performed.
The prepared application fixture remains unexecuted; local recording is excluded
from further work in this session. Current work continues through source review,
offline controls and separately identified disposable-host evidence.

`initial-query-error/` preserves the first verification wrapper's incomplete
attempt. It omitted the required `--report` argument to the C observer; the
observer printed usage and performed no device query. The wrapper then failed
while decoding empty stdout. The corrected, separate read-only verification is
under `verification/`; none of the initial files was overwritten or counted as
an audio observation.

The final Installer success screen was not captured: the user had closed it
before reporting completion. The subsequent accessibility read reopened a file
dialog, which was closed without another installation. Installed payload bytes,
sealed signature and receipt provide the independently checked installation
facts. The earlier [package preparation](../application-fixture-preparation-dbab3fd/README.md)
retains its original source and build record, including pkgbuild warnings.

`manifest.json` and `SHA256SUMS` cover all files except the manifests themselves.
