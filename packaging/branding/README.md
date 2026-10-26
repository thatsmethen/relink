# ReLink icon

An original geometric mixer mark in the application’s mint and charcoal palette. No Elgato or eframe artwork is used. It is covered by the repository’s first-party proprietary licence.

`relink.svg` is an editable vector. `scripts/build-icons.py` renders the same explicit geometry to PNG, macOS ICNS, Windows ICO and an MSVC resource file using Pillow 10.3+. Generated assets are checked in; normal builds need no image-generation dependency. Regenerate with `python3 scripts/build-icons.py`.

The Rust viewport uses the bundled 256-pixel PNG. The macOS app includes its ICNS and `CFBundleIconFile`; Windows MSVC builds link the original icon resource into the executable. Resource structure was independently checked; shell rendering on Windows still requires native inspection.

The original mark contains no letters. The asset filenames now use `relink`; their geometry, raster assets and Windows resource bytes are unchanged. The application and packaging scripts use these renamed assets.
