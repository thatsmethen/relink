# Public renamed-source verification

Anonymous GitHub verification confirmed source `69f49da28a9bae16dec9d4bc29bd0d639fb73a08`
was public on main. All five Rust crate directories, ten archived Swift source/test
folders, six branding assets and all 32 README links resolved. Thirty-six linked
or active blobs matched local bytes and Git tree hashes. Historical evidence
paths were excluded from the old-folder-name check.

The report verifies the public source tree, not installed software behavior.
`ci-dispatch-snapshot.json` records separate CI observations at that exact source.
The new multi-output protocol was dispatched once as run 40257448775. Running
statuses in this snapshot are not passing results. Native Windows driver build
40257398430 had passed when the snapshot was taken; installation and audio were
not observed. Later collection must preserve every original failure or partial
observation and must not replace this time-specific snapshot.
