**Overall FAIL · installed HAL FAIL · separate runtime capture PASS**

This prerelease preserves the original bytes from [workflow run 40051763048, attempt 1](https://github.com/thatsmethen/relink/actions/runs/40051763048), tested at exact commit `91d73e4cf0ecf820d93a68f73d59f1cb09d00b13` (tree `f42729259cbade01f60b64b2b5726de83f4c4cf6`). It is an evidence archive, not a qualified application or driver release. There was no rerun, threshold adjustment, PCM repair or replacement result.

The installed HAL protocol failed on the tone writer's sample-clock continuity: **616 frames were missing before callback 3140**, with a nearby processor-overload notification. The independent readers had stopped more than 13 seconds earlier; their successful narrower checks do not cover that later gap. These observations do not establish its underlying scheduling, host, driver or processing cause.

The separate runtime job passed its fixed gain/mute/restore and stopped-silence protocol. **That source used the original macOS native capture adapter with CPAL output. It does not exercise the subsequently authored native output owners.** The jobs ran on different hosted macOS machines; their host timestamps cannot be correlated. Both cleanup steps passed. This single hosted observation does not qualify physical hardware, Windows or repeated reliability.

The original HAL and runtime ZIPs are distributed as ordered **512 KiB lossless parts** (`original-hal.zip.part000`, and so on) because earlier whole-file uploads timed out. Concatenating each listed sequence reconstructs the byte-for-byte original download, including PCM, CSV and diagnostics. There is no recompression or archive-member modification. The small `original-workflow-logs.zip` is supplied whole. The HAL and runtime ZIP hashes match GitHub's artifact digests. GitHub supplied no original workflow-log digest; its retained local SHA-256 and ZIP CRC were verified. No large PCM extraction was performed.

`collection-evidence-40051763048.zip` is a separate compact snapshot of the collection report, original small text members, original member manifests and offline verification scripts. Metadata explicitly labelled as a projection is not an original API response. Its collection-stage statement that no publication occurred describes the collection stage; this prerelease is the subsequent, separately authorized publication. Historical RaveLink names inside original files are preserved.

`publication-assets.json` preserves the original four archive sizes and SHA-256 hashes. `publication-parts.json` adds each ordered segment, byte offset, size and hash. To reconstruct those exact archives, verify every original manifest member and reproduce the recorded diagnosis, download all release assets into one directory and run:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 reconstruct-parts.py --downloads "$PWD"
```

The reconstructor validates every part and complete original ZIP hash before running the unchanged, bundled collection verifier. It extracts only the small collection snapshot; the verifier reads original PCM and CSV members directly from their compressed ZIPs. It does not rerun audio, change acceptance criteria or access the network. A successful custody verification confirms preserved bytes; the original overall and HAL outcomes remain **FAIL**.
