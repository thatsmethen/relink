# ReLink development preview — CI 40068367011

Published prerelease:
<https://github.com/thatsmethen/relink/releases/tag/v0.1.0-preview.20261024-relink>

The release tag resolves to `25453d2eb47891b43df19591b94b80c14eda4203`.
Both desktop CI jobs completed successfully for that exact source. The original
GitHub Actions artifact archives were downloaded and verified against the API's
SHA-256 digests before extracting their inner application packages. The inner
ZIPs were uploaded unchanged; these are app-only packages, with no virtual-device
driver installer. The earlier preview was not edited or replaced.

| Application package | Bytes | SHA-256 |
| --- | ---: | --- |
| `ReLink-macOS-development.zip` | 4,243,112 | `23cd455b046445e1cda853759e026c4ce121bc97a51b148d356affc9a0b51830` |
| `ReLink-Windows-development.zip` | 4,176,056 | `0edc46d871b9fe2412f6ea303e9d27b40ad4dcd9cf9dc1bad06635f826f13338` |

The macOS Apple Silicon bundle is named `ReLink.app`, preserves its executable
permission and passes strict ad-hoc signature verification. It is not Developer
ID signed or notarized. The x64 Windows package contains `ReLink.exe` and has no
embedded Authenticode certificate. Its native CI icon-resource report matches
the exact packaged executable, source verification script, ICO/RES files and
all eight resource payloads. This checks embedded bytes, not Windows shell
rendering. Both packages' licence, third-party notices and Rust runtime notices
match the release source byte for byte.

The two PNG screenshots are the existing stopped native macOS captures from
`9cd350f`; their hashes match the [original provenance](../relink-screenshots-9cd350f/).
All 79 recorded screenshot build inputs were compared with release source.
The only code difference is formatting of the `RE / LINK` label; documentation
and screenshot files were added separately. The screenshots are local debug
build captures, not captures of these CI release binaries. The standalone
`ReLink-icon.png` retains the original letter-free mixer mark bytes.

All seven release assets were downloaded anonymously after publication and
matched both their local originals and GitHub's asset digest. The complete
readback record is in `public-download-verification.json`; the tag object and
published release metadata are retained alongside it. No application was started
or audio device opened by this package verification. These checks do not establish
installed driver behaviour, end-to-end audio acceptance or production readiness.

## Records

- `run.json`, `jobs.json`, `artifacts.json`: original GitHub API responses.
- `source-workflow.yml`: workflow bytes at the exact release commit.
- `package-manifest.json`: original artifact/archive identity and every inner ZIP member.
- `preview-verification.json`: package, source, icon and screenshot verification.
- `macos-codesign.txt`: verification output; only the local workspace path is replaced.
- `windows-icon-resource-verification.json`: original native CI report, including its original CRLF bytes.
- `release.json`, `release-tag.json`, `public-download-verification.json`: published release identity and anonymous byte checks.
- `release-notes.md`, `SHA256SUMS.txt`: exact published notes and checksum asset.
- `earlier-preview.json`: read-only snapshot of the preserved earlier release.

Original outer artifact ZIPs and extracted packages are retained locally in
`build/relink-preview-40068367011/`; application ZIPs are also public release
assets. The original outer archive SHA-256 values are
`92348151eef1860377618ebf08e6c11b96d163194c55f874f395f82801fb26e1`
(macOS) and `033c1743b4666baaf2ee44cc71968fa20135ff5483f8970acd710443eac707ef`
(Windows). Publication and public readback completed on 26 October 2026 UTC;
the tag date identifies the source/CI build from 24 October.
