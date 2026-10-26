> **Current publication:** The [public evidence release](https://github.com/thatsmethen/relink/releases/tag/evidence-native-pcm-40051763048) has passed anonymous download and complete original-byte verification. See [PUBLICATION.md](PUBLICATION.md). The October 24 draft/failure status below is retained as a historical observation; original workflow outcomes are unchanged.

# Installed observation 40051763048 — overall FAIL

[Original workflow run](https://github.com/thatsmethen/relink/actions/runs/40051763048),
attempt 1, used exact source `91d73e4cf0ecf820d93a68f73d59f1cb09d00b13`.
The **installed-hal job failed**, while the separate **installed-runtime-capture
job passed**. Both cleanup steps succeeded and independently confirmed both
original Rust cable UIDs absent. These are distinct observations on different
hosted machines. The overall workflow remains FAIL.

This is preservation of that original run, with no rerun, changed threshold,
repaired PCM or replacement outcome. Nothing is published by this folder's tools.
No development-machine audio device was opened or changed. Original files retain
their historical RaveLink names; repository URLs reflect its rename to ReLink.

## Exact failed condition

The original basic HAL acceptance report contains one failed observation:
`sample clock or callback continuity failed` in the 40-second tone writer.
Immediately before zero-based callback **3140**, the writer's output sample clock
skipped **616 frames**. The preceding callback started at sample 1,631,740 and
contained 512 frames; the next started at 1,632,868. Native host timestamps advanced
23.5 ms, consistent with the complete 1,128-frame sample-time interval at 48 kHz.

The preceding callback entered **0.789 ms after its output timestamp** and spent
**9.208 µs** in recorded callback work. Its processor-overload notification
(`'over'`, selector 1870030194) arrived **0.730 ms after that callback's entry**.
The next callback entered 9.644 ms before its future output timestamp. These are
observed clock/notification boundaries. They do not identify which host,
scheduling, driver or processing condition caused the missed IO deadline.

Both independent readers had already stopped **13.13–13.30 seconds before** the
late writer entry. Their continuous clocks and fixed tone interiors passed, and
their PCM matched exactly over 760,152 common frames. Restart capture was exact
silence. Those narrower checks do not cover the later writer-gap interval and do
not make the complete HAL protocol pass. All four observer processes exited zero
within their original deadlines; process completion is distinct from acceptance.

## Separate runtime result

The runtime job's fixed four-phase gain/mute/restore protocol passed, as did its
independent source and reader clocks. Its four control acknowledgment intervals
were 21.25, 25.11, 26.23 and 22.85 ms. The stopped tail peak was zero and the observed
stop response was 35.01 ms. The terminal native capture record had fault 0,
`first_fault: null`, 2,181 packets, native shutdown observed, and `OwnerCompleted`;
Stop, Destroy and listener removal returned status 0.

This source used the original macOS capture adapter with **CPAL output**. The
subsequently authored native output owners are not exercised by this run. This
single hosted VM observation does not qualify physical devices, Windows, repeated
reliability or the current uncommitted owner work. Instrumentation may itself
affect scheduling. Both jobs reported macOS 15.7.9 (24G830), ARM64 VMAPPLE, SDK 15.5
and Rust 1.91.1; their host clocks cannot be compared across machines.

## Original custody

Original compressed bytes are retained locally in
`build/rust-hal-failed-40051763048/`; large PCM/CSV and diagnostic members remain
inside those archives. No bulk PCM extraction was needed.

| Original archive | Bytes | SHA-256 |
| --- | ---: | --- |
| original-hal.zip | 10,532,207 | `71549c6f84f6cc8b3b2183e0e70b45a4df4a9169667b5b37bd15eadf9111788f` |
| original-runtime.zip | 7,048,981 | `4c89e5a29ac31a564c35e550a7a3f9e3764b2f1f4cc025a07c302e329a4a5979` |
| original-workflow-logs.zip | 13,648 | `67b25b835f09ad5210e3551832d600ec93d67e89a5d527ab5ad6914f0e63fd79` |

Artifact IDs are 12543373454 (HAL) and 12542779812 (runtime). Both complete ZIP
hashes match the GitHub artifact API digests. The workflow-log API supplies no
digest; its retained bytes have a local SHA-256 and a valid ZIP CRC. All 65 HAL and
56 runtime original manifest files verify against their recorded byte counts and
hashes, with no extra or missing archive members apart from each manifest itself.

`run-metadata.json` is explicitly a **metadata projection**, not the original API
response. Full original run/jobs/artifact/commit API bodies remain unchanged in
the local collection and are hashed by `original-local-files.json`. Source tree
identity was checked against both the local commit object and GitHub's Git commit
API. `source-workflow.yml` contains the exact workflow bytes from that source.
`hal-original-text/` and `runtime-original-text/` contain selected small original
members copied byte-for-byte; they are not rewritten summaries. `collection.json`
lists each copy. `derived-diagnosis.json` is a separate reproducible derivation;
it does not alter or rerun the original acceptance reports.

To verify supplied copies of all three original archives, offline:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify.py --archives /path/to/archives
PYTHONDONTWRITEBYTECODE=1 python3 diagnose.py --archives /path/to/archives
```

`collect.py` only retrieves the already completed run and verifies existing
original bytes. It never starts a workflow or publishes assets. Publication,
if requested separately, must preserve these originals and label both job
outcomes and the overall FAIL explicitly.

## Subsequent publication attempt — incomplete

The separately authorized publication was prepared under tag
`evidence-native-pcm-40051763048`. Its lightweight tag points directly to the exact
tested commit, with no tagger metadata. The source author and committer remain
the anonymous contributor identity. Explicit prerelease draft **422528575** is
retained, with no uploaded assets. The anonymous public release API returned
**404** at the recorded observation: this evidence release is **not published**.

Large archive transfers failed with GitHub HTTP 408 or client write timeouts.
The final bounded, rate-limited attempt reported 5,767,168 bytes sent before HTTP
408; that partial transfer is not a preserved or accepted asset. Further large
upload attempts stopped. `publication-attempts.json` records these transport
observations separately from the original workflow evidence;
`publication-state.json` is a metadata projection of the retained draft and tag,
not an original API response. The original API responses from these checks remain
in the local `build/rust-hal-failed-40051763048/publication/` directory.

`release-notes.md` and `publication-assets.json` are prepared for the release.
The compact `collection-evidence-40051763048.zip` in that local directory contains
the exact pre-publication collection snapshot, including its original checksum
manifest and offline verifier. `collection-README.md` and
`collection-SHA256SUMS.json` preserve that snapshot's report and manifest before
this publication-status addition. The current `SHA256SUMS.json` additionally
covers the later publication metadata and verification tool.

Anonymous public download/hash and full manifest verification remain
**unperformed**, because uploading the original archives did not complete.
`verify-public.py --downloads /path/to/a/new/directory` is prepared to perform
that bounded verification after a later publication: it uses no authentication
or cookies, downloads each public asset, checks the exact commit/tag and release
notes, and executes the bundled offline verifier. It extracts only the compact
snapshot; large PCM stays compressed. No publication failure changes the
original overall **FAIL**, HAL **FAIL** or separate runtime **PASS** outcomes.
