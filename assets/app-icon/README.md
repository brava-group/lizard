# Lizard app icon

The app icon uses the exact vector mark in `crates/ui-egui/assets/nav-logo.svg`, supplied for
Lizard's navigation panel by the project owner. It is black on a white background.

`cargo xtask icon` composes the mark on a 512-unit canvas and renders committed PNGs from 16 to
1024 px. The macOS `.icns` source has an 824 px rounded tile centred in a transparent 1024 px
canvas, matching the native Dock icon grid. The Windows and Linux fallback fills its canvas.
No shadow, highlight or glass effect is baked into either source.

The legacy `figcraft-*` asset filenames remain until the remaining packaging paths are migrated.
`figcraft.svg` is the padded macOS source; `figcraft-small.svg` is the full-size fallback. Both are
generated from the navigation logo by `cargo xtask icon`.

The icon is original project artwork, licensed under MIT OR Apache-2.0; see `LICENSE.txt`.
