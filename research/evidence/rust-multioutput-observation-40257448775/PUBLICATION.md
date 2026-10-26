# Public preservation of the original native two-output observation

The [evidence prerelease](https://github.com/thatsmethen/relink/releases/tag/evidence-multioutput-native-40257448775) is public. Anonymous download and complete offline verification finished at **2026-10-26T03:17:29.485433+00:00**. The lightweight tag points exactly to `69f49da28a9bae16dec9d4bc29bd0d639fb73a08`, tree `e761d228a3f0998b9addc97c11a4fd3774b90545`; it has no tagger metadata, and the source author and committer use the anonymous ReLink Contributors identity. This is original run **40257448775, attempt 1**, not a new execution or the latest application source.

The original result remains **PASS for one short, fixed two-tone virtual-output protocol**: 18 seconds of five gain/mute/restore phases and two independent 32-second recordings at 48 kHz on one macOS CI host. Both native output-owner cleanup gates and owned-driver cleanup passed. This establishes neither application or input capture, physical playback, acoustic synchronization, sustained load/restart reliability, arbitrary output counts, installed Windows operation, nor production completion. Native progress describes software submissions; the finite-window wrong-bus projection is not a physical crosstalk guarantee. Earlier failed observations remain unchanged.

All **19 public release assets** were downloaded without authorization or cookie headers, matched their expected full byte counts/SHA-256 values, and matched GitHub's release asset digests. The transport used 14 ordered lossless parts of at most 512 KiB after bounded whole uploads failed. Full original archive hashes verified after reconstruction.

| Preserved archive | Bytes | SHA-256 |
| --- | ---: | --- |
| Original artifact ZIP | 6,819,656 | `003487396b907bb9b03d00f83d717b46e46ade750f87a5ff896526fdfda2d85b` |
| Original workflow-log ZIP | 21,633 | `0c2c733a2561c7eff8cb7e21980d820464536908f4405a87c8021a0440071787` |
| Frozen collection/API snapshot | 322,418 | `2642427a0207c7b2960f70ff8f4091ccd13fd4d35f3b48bce3a310fd407f0adc` |

The artifact ZIP matches GitHub Actions' original artifact digest. The workflow-log endpoint provides no upstream digest; its originally collected bytes, local SHA-256 and ZIP CRC are preserved. The snapshot contains all **72 frozen evidence files and 18 original API bodies**. Anonymous roundtrip verification checked all **56 custody-manifest files and exact 57 artifact ZIP members**, the compact original text, source identities, native lifecycle records and cleanup. The unchanged bundled verifier recomputed the full original PCM analysis and both independent diagnostic reports without excluding fields or changing thresholds; its complete derived report matched the frozen report.

Download every release asset into one directory and run with Python 3.10 or newer:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-release.py --downloads "$PWD"
```

This needs only the Python standard library. The exact analyzers are bundled; no local Git clone, network, device access, installation or audio acquisition is used by this offline command. Temporary reconstruction/extraction files are removed afterward. The subprocess uses isolated Python mode so environment optimization cannot disable the original verifier's assertions. Local controls also confirmed exact reconstruction from 512 KiB parts and rejection of a one-byte corruption or missing part.

Anonymous download verification can be repeated from this evidence directory into a new directory:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-public.py --downloads /path/to/new-directory
```

`public-verification.json` records the later publication proof and all public asset hashes/URLs. `publication-upload.json` is authenticated upload metadata, distinct from public download proof. `publication-assets.json` defines the download layout. The original `SHA256SUMS.json`, README, collector, verifier and collection-time "not published" / `no_rerun_no_publication` statements remain byte-for-byte unchanged and describe the historical collection time. The separate `publication-SHA256SUMS.json` covers only these later publication records; it does not rewrite the original collection manifest.
