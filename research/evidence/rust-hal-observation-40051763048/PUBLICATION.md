# Publication completed — original overall FAIL retained

The [evidence release](https://github.com/thatsmethen/relink/releases/tag/evidence-native-pcm-40051763048) is public. Anonymous download and reconstruction
verification completed at **2026-10-26T00:41:28.388014+00:00**. This is a custody result:
the original workflow remains **FAIL**, the basic installed HAL job remains
**FAIL** with its 616-frame writer-clock gap, and the separate runtime job remains
**PASS**. That runtime used **CPAL output** and does not qualify the subsequently
authored native output owners.

The two large original ZIPs are distributed as 35 ordered segments, each no
larger than 512 KiB. The small original workflow-log ZIP and unchanged compact
collection snapshot are supplied whole. Three metadata/verification assets bring
the release to **40 assets**. Concatenation reconstructs the exact original
compressed bytes; no ZIP was recompressed, no PCM was repaired, and no acceptance
threshold or original result was changed.

All 40 assets were downloaded without authentication or cookies and matched both
the local expected SHA-256/byte counts and GitHub release digests. The reconstructed
HAL and runtime ZIP hashes matched the original GitHub artifact digests. The
unchanged bundled offline verifier checked all **65 HAL and 56 runtime manifest
files**, their complete archive membership, the original compact text copies and
the reproduced diagnosis. The compact snapshot's own original manifest also
matched. A separate one-byte corruption control failed before reconstruction.
The source tag remains a lightweight tag at exact commit
`91d73e4cf0ecf820d93a68f73d59f1cb09d00b13` with no tagger metadata; source author
and committer retain the anonymous contributor identity.

Download all release assets into one directory and run:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 reconstruct-parts.py --downloads "$PWD"
```

For an independent anonymous download from this repository folder:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 verify-public-parts.py --downloads /path/to/new-directory
```

`public-verification-parts.json` contains the completed public proof and exact
asset hashes. `publication-state-parts.json` records the later publication state;
`publication-upload-parts.json` is authenticated upload metadata, separately
labelled and not substituted for download verification. `publication-parts.json`
records the ordered segment hashes and original archive hashes.

The October 24 failed publication reports, `publication-state.json`, old release
notes, whole-file verifier and `publication-assets.json` remain unchanged as
historical records. `pre-parts-publication-README.md` preserves the exact prior
README, and `pre-parts-publication-SHA256SUMS.json` preserves its complete prior
manifest. All 125 previously manifested files were checked byte-for-byte before
adding this publication update. The only removed GitHub asset was the old,
digestless, unaccepted `original-runtime.zip` starter from October 24; no accepted
asset was replaced. Three transient part-upload HTTP 408 responses recovered
within bounded retries. The original complete archives remain retained locally.
