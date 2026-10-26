ReLink's development preview now saves explicit output routes with the studio. Assign a mix to each device and choose one clock; selecting a different mix in the editor leaves those assignments intact. The Rust runtime connects simultaneous CoreAudio/WASAPI outputs with independent clock compensation and per-device control submission status.

Also included: recovery saves preserve the original file before replacement, saved recovery work reopens normally, late cleanup failures remain visible after Stop, and startup priming has separate bounded timing allowances.

![Current ReLink mixer](https://raw.githubusercontent.com/thatsmethen/relink/e552ed967db17bb0096d9f03251d8d917a0ac85b/docs/screenshots/mixer-macos.png)

[Output routing screenshot](https://raw.githubusercontent.com/thatsmethen/relink/e552ed967db17bb0096d9f03251d8d917a0ac85b/docs/screenshots/output-routing-macos.png) · [Mix effects](https://raw.githubusercontent.com/thatsmethen/relink/e552ed967db17bb0096d9f03251d8d917a0ac85b/docs/screenshots/mix-effects-macos.png)

Source: `902118a60b070bb9cefdef75d5b477b5350b9847`. [Desktop CI](https://github.com/thatsmethen/relink/actions/runs/40254943747) passed on macOS 15 and Windows Server 2022. Local unchanged-source verification passed 253 tests. The screenshots are actual stopped macOS views from a debug build of that same source, with explicit test tones; they do not depict live audio.

**Development preview, not production audio qualification.** The separate [installed single-output observation](https://github.com/thatsmethen/relink/actions/runs/40254943736) failed both its HAL and runtime jobs. Earlier intermittent failures remain unresolved. Installed simultaneous-output PCM and native Windows audio/interaction remain unqualified.

Downloads are app-only: macOS Apple silicon is ad-hoc signed without notarization; Windows x64 is unsigned. No virtual-driver installer is included. Both packages embed the original ReLink icon and matching dependency notices. Existing internal identifiers and studio paths remain compatible; legacy studio files load without modification until saved.

`SHA256SUMS.txt` covers both original CI package ZIPs, all three screenshots, the icon and verification report. The report distinguishes package checks from runtime acceptance. First-party code remains proprietary despite public repository visibility.
