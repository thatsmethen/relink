ReLink development preview with the renamed application, original mixer icon and updated native macOS screenshots.

**App-only preview:** no virtual-device driver installer is included. The macOS Apple Silicon app is ad-hoc signed and not notarized; Windows x64 is unsigned. This is not a production release or a claim of completed end-to-end audio qualification.

- **macOS:** download `ReLink-macOS-development.zip` for `ReLink.app` (Apple Silicon, macOS 13+).
- **Windows:** download `ReLink-Windows-development.zip` for `ReLink.exe` (x64).
- Both packages include the proprietary first-party licence and third-party/runtime notices. `SHA256SUMS.txt` records the asset hashes.

Packages are unchanged artifacts from [desktop CI 40068367011](https://github.com/thatsmethen/relink/actions/runs/40068367011), which passed both platform jobs for source [`25453d2eb47891b43df19591b94b80c14eda4203`](https://github.com/thatsmethen/relink/commit/25453d2eb47891b43df19591b94b80c14eda4203). Local archive checks verified GitHub's original artifact digests, the macOS strict ad-hoc signature, and the Windows CI icon-resource report against the packaged executable and exact source.

The screenshots below show the actual native macOS interface with audio stopped. They were captured from an isolated local debug build of `9cd350f`, whose UI differs from this release source only by Rust formatting of the RE / LINK label. They are not captures of the release binaries. [Screenshot provenance](https://github.com/thatsmethen/relink/tree/25453d2eb47891b43df19591b94b80c14eda4203/research/evidence/relink-screenshots-9cd350f) and `preview-verification.json` describe the checks and their limits.

![ReLink mixer, audio stopped](https://github.com/thatsmethen/relink/releases/download/v0.1.0-preview.20261024-relink/mixer-macos.png)

![ReLink mix effects, audio stopped](https://github.com/thatsmethen/relink/releases/download/v0.1.0-preview.20261024-relink/mix-effects-macos.png)

The earlier RaveLink preview remains available unchanged.
