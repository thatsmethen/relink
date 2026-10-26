# Current ReLink screenshots — source 902118a

These are actual native macOS captures of the debug application built from
`902118a60b070bb9cefdef75d5b477b5350b9847`, including the new saved Outputs window.
`build-binding.json` records the source tree, built binary, ad-hoc signed test
bundle binary and fixture hashes. Wrapping changes only the app's test identity
and signing metadata; the app uses the separately selected studio path.

The explicit fixture contains four test tones, three muted mixes and two saved
routes: Monitor to BlackHole 2ch (clock), Stream to MacBook Pro Speakers. **Audio
remained stopped.** The captures show interface configuration, not hardware
playback, driver installation, Windows interaction or multi-output qualification.
The user's ordinary studio was unchanged byte-for-byte before and after.

Original JPEG and accessibility captures are preserved. Repository PNGs encode
exactly the original decoded RGB pixels, without cropping, scaling or retouching.
The macOS capture badge remains visible. `observation.json` records every image's
sizes and hashes. The effects image shows the genuine scrollable inspector;
controls below the viewport are not composited into view.

An initial screenshot fixture had an invalid `mute` field instead of `muted`.
The app displayed “Recovery not saved”; closing it preserved that invalid file
exactly. The invalid fixture and observation remain separate. Correcting only the
fixture allowed the captures above; no application source was modified to pass.

The earlier [9cd350f screenshots](../relink-screenshots-9cd350f/README.md) remain
preserved as historical evidence for the older development preview.
