## Preserved native two-output evidence — short virtual-output PASS

This evidence prerelease preserves the first and only attempt of [workflow run 40257448775](https://github.com/thatsmethen/relink/actions/runs/40257448775), using source `69f49da28a9bae16dec9d4bc29bd0d639fb73a08` (tree `e761d228a3f0998b9addc97c11a4fd3774b90545`). This is an evidence archive, not an application installer or a newer product release.

The fixed 18-second five-phase test used two internal tones (997 and 1543 Hz) on two original virtual cable outputs at 48 kHz, observed by two independent 32-second recorders on one macOS CI host. Route gain/mute/restore, native timing/progress, independent recorder diagnostics, Stop silence, both output-owner cleanup gates, and owned-driver cleanup passed the preregistered criteria. Both recorders retained 1,536,000 stereo frames with no sample-clock gaps. The original full analysis and both diagnostic reports recompute exactly from original PCM using the bundled pinned analyzers.

This does not establish application or input capture, physical playback, acoustic synchronization, long-duration/load/restart reliability, arbitrary route counts, installed Windows operation, or production completion. The wrong-bus spectral check is a fixed finite-window diagnostic, not a physical crosstalk guarantee. Earlier failed observations remain unchanged and are not replaced by this different protocol.

### Verify the preserved originals

Download **every release asset** into one directory, then run with Python 3.10 or newer:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD"
```

No Git clone, Python packages, device access, driver installation or new audio execution is required. The wrapper verifies downloaded sizes and SHA-256 values, reconstructs any ordered lossless parts if present, and extracts the frozen collection and original API records into a temporary directory. The unchanged frozen verifier checks every original ZIP manifest member, compact original text, pinned source identity and exact full analysis. Temporary files are removed afterward. `publication-assets.json` records original full archive hashes and transport layout.

The original artifact ZIP is 6,819,656 bytes, SHA-256 `003487396b907bb9b03d00f83d717b46e46ade750f87a5ff896526fdfda2d85b`; its digest matches the original GitHub Actions artifact metadata. The original workflow-log ZIP is 21,633 bytes, SHA-256 `0c2c733a2561c7eff8cb7e21980d820464536908f4405a87c8021a0440071787`; that endpoint provides no upstream digest. The collection snapshot preserves the original README and collection-time "not published" statements without rewriting historical metadata. Later publication status is recorded separately.
