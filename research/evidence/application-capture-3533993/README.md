# Application capture: implementation and stopped-audio validation

Source [`353399309004a1be5a196ebaa68c08218aff0555`](https://github.com/thatsmethen/relink/commit/353399309004a1be5a196ebaa68c08218aff0555), tree `ad5f11535be216e38b4d80145e47aba3b8ee7463`.

**279 local workspace tests passed. Native application audio capture remains unqualified.** This record covers deterministic tests, static Windows checks, a signed local macOS debug bundle, and application-picker/persistence observations with audio stopped throughout. No capture permission was accepted.

`logs/workspace-tests.log` records 279 passes across ten suites, with no failures or ignored tests. Strict workspace Clippy and strict Windows audio/app all-target cross-Clippy completed successfully; their original output is preserved. Cross-compilation does not establish Windows execution. The earlier stale dependency-notice check and subsequent regeneration are retained separately; `independent-checks.json` records a successful read-only check of the final notices and strict verification of the isolated bundle's local signature. Ad-hoc signing is not Developer ID signing or notarization.

The UI observations have two distinct build boundaries:

| Build | Observed workflow |
| --- | --- |
| Development binary, SHA-256 `b65780c58911132bc1410953279afc9cd53e7010136b1543657cda9349d3265e` | Discovered actual applications; added Google Chrome; routed it to Monitor; undid and redid that route; saved, quit and reopened the fixture with five sources and thirteen routes. |
| Rebuilt exact committed source, isolated signed binary SHA-256 `7bac331de9b2cd1b18c09c26f0a0d4362cd983f744c81bcc4682a9d394d39819` | Reopened the saved five-source/thirteen-route fixture, then independently rediscovered and selected Google Chrome in the picker. The complete earlier add/route/undo/redo journey was not repeated on this binary. |

The root agent performed these native UI observations. This record's collector did not replay them. `ui/committed-reopen-ax.txt` preserves the fixture-only committed reopen state, including “Saved locally” and “AUDIO STOPPED”. Full picker accessibility text and screenshots are withheld because they contain the actual application inventory. Committed discovery/selection is recorded as the root observer's report, without publishing that inventory.

`ui/committed-build-binding.json` binds the packaged executable, isolated signed executable and test plist to the committed build. The isolated bundle uses a separate bundle identifier and explicit `RELINK_STUDIO_PATH` pointing to the generated fixture. Its executable hash differs from the packaged executable because of the isolated bundle's signing identity. The collector independently checked all three recorded file identities and the fixture override.

`ui/studio.json` contains the final generated fixture, including the stable selector for `/Applications/Google Chrome.app`. Its SHA-256 is `fc3dd9acfb698a237869dbdab22a34fa451be674d26f132506af6bbecf4d354f`. The original `precommit-before.json` and `precommit-after.json` both retain the initial fixture hash; neither is used as proof of the final saved fixture bytes. The normal user's studio was independently rehashed and still equals its recorded before/after SHA-256, `94efd0f900d0d205944bbd291fa87a078ea469d7a3c1b462fedc5b123f3a6615`. Its contents are not included.

`validation.json` separates observations, artifact checks and limitations. `source-files.json` binds the maintained Rust workspace and selected packaging/notice inputs to their exact committed Git blobs and SHA-256 values. `SHA256SUMS.json` and `SHA256SUMS.txt` cover every file in this compact record except the two manifests themselves. The original logs are copied unchanged; no tests, native capture or audio observations were rerun to create this record.

This evidence does not establish native process selection/audio isolation, idle/resume behavior, permission denial/revocation, seamless app relaunch, Windows runtime behavior, sustained reliability or production readiness. It does not alter any earlier failed native observations.
