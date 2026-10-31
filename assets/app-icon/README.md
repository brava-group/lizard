# Lizard app icon

`Lizard-new-icon.lizard` is the editable source drawn by the project owner in Lizard. Its
`Lizard New Icon` frame is the finished 1024 × 1024 macOS artwork. It contains the 824 px icon
body, its transparent margins, gradients, and shadows.

Run `cargo xtask icon` to render that frame at its native size on a transparent canvas. The command makes
the committed PNG sizes, Windows ICO and Linux hicolor icons. macOS packaging builds `Lizard.icns`
from the PNGs. The rendered artwork is used directly, without an additional Icon Composer
background or glass treatment.

The `lizard-*` filenames and Linux icon id remain for compatibility with existing packaging.
`lizard.svg` is the separate README logo. The app icon is original project artwork licensed
under MIT OR Apache-2.0; see `LICENSE.txt`.
