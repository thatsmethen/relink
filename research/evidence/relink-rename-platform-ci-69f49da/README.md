# ReLink rename: original macOS and Windows CI packages

The two original attempt-1 runs at source
[`69f49da28a9bae16dec9d4bc29bd0d639fb73a08`](https://github.com/thatsmethen/relink/commit/69f49da28a9bae16dec9d4bc29bd0d639fb73a08)
completed successfully:

- [Windows driver run 40257398430](https://github.com/thatsmethen/relink/actions/runs/40257398430): native Windows build, nine host tests, SDK ABI compilation, INF validation and unsigned catalog generation.
- [Desktop run 40257398458](https://github.com/thatsmethen/relink/actions/runs/40257398458): both macOS and Windows jobs passed their checks and produced application packages.

This is build and packaging evidence. No application was launched during this verification. The Windows driver report explicitly records `kernel_loaded: false` and `signed: false`; installation, upgrade, endpoint enumeration, audio, UI interaction and Windows shell icon rendering remain unobserved. The macOS CI HAL harness runs in a local synthetic host. The macOS application is ad-hoc signed, without Developer ID signing or notarization. The Windows application is unsigned. No CI rerun or release publication was performed for this record.

## Package checks

The three original GitHub artifact ZIP digests match the API records. Their complete member lists and hashes, and both original CI log ZIPs, are preserved: five archives containing 58 files. Both nested application ZIPs also have complete member manifests. Verification checked 35 extracted package files and 20 source files obtained at the exact commit and checked against their Git blob IDs.

- macOS: `ReLink.app/Contents/MacOS/relink-app`, `ReLink.icns`, original bundle identifier `au.textual.ravelink.rust`, arm64 architecture, executable permission and strict code-signature verification. The old executable/icon/license payload names are absent. Plist, icon and notices match the exact source bytes.
- Windows application: `ReLink.exe`, x86-64, no Authenticode certificate directory. The original native Windows icon verification report binds the packaged executable to the exact source verifier, ICO and RES hashes, and to all eight resource byte hashes. This macOS verification checks those bindings; it does not repeat Windows resource enumeration.
- Windows driver: `ReLink.sys`, `ReLink.inf`, `relink.cat` and `relink_windows_driver.pdb`; no old-named payloads. Inf2Cat emits the lowercase catalog filename, which matches the INF's `ReLink.cat` under Windows filename matching. The original PE report was recomputed exactly. The PDB GUID and age match the SYS CodeView record. Packaged INF, first-party licence, provenance and toolchain file match source bytes. All six runtime-notice hashes match their manifest, and its source paths and generated explanatory text match the pinned notice script.

The installed Windows `Root\RaveLinkAudio` hardware identity and `RaveLinkAudio` service identity remain stable. Catalog presence/name and the successful Inf2Cat log were verified; no independent Windows catalog trust or installation check is claimed.

## Custody and reproduction

`driver/` and `desktop/` retain exact original API responses and archive manifests. Original ZIP bytes and the exact source snapshot are under `build/relink-rename-platform-ci-69f49da/`; original archives are never normalized, repacked or overwritten. The large ZIPs are deliberately outside this compact repository record. `collection.json` records their paths and hashes. The transient in-progress desktop API snapshot remains separately preserved in `build`.

`collect.py` downloads the specified run attempt and pinned source; it performs no CI dispatch, Git change, audio operation or publication. It refuses to replace differing existing originals. Downloads require `gh` access while CI artifacts/logs remain available; source files use the public exact-commit raw URLs. Existing original API responses may differ from later API state and must not be replaced.

With the preserved build directory in place, on macOS run:

```sh
python3 research/evidence/relink-rename-platform-ci-69f49da/verify.py
python3 research/evidence/relink-rename-platform-ci-69f49da/verify-windows-driver.py
```

These commands only extract and inspect the saved bytes. The first recomputes derived verification reports and uses system `ditto`, `lipo` and `codesign`; it imports the exact pinned icon and PE verifier source from the saved snapshot. The second rechecks the independently reviewed Windows driver, including SYS/PDB binding and original log markers, and writes a new report under `build`. No installed driver, native audio device, Git checkout or executable launch is needed. `independent-windows-review.json` retains the original independent review; the adapted standalone verifier only changes its input/output locations.

`metadata-manifest.json` and `SHA256SUMS.txt` cover every compact record file except themselves. They describe the frozen record and must be regenerated if derived reports are deliberately recomputed.
