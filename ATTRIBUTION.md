# Asset attribution

## Shortcuts Font

- Files: `assets/fonts/shortcuts-font/ShortcutsFont-original.ttf` (upstream) and
  `assets/fonts/shortcuts-font/ShortcutsFont.ttf` (egui-compatible cmap).
- Source: [fabiospampinato/shortcuts-font](https://github.com/fabiospampinato/shortcuts-font), commit `f5439efe6b1b41514dc329abf62eae676681d32b`.
- Copyright © 2020–present Fabio Spampinato. The font is based on Inter.
- License: MIT. Full text in `assets/fonts/shortcuts-font/LICENSE`.
- The bundled font's single-character substitutions were mapped directly to visible glyphs,
  and named key ligatures to private-use codepoints, using `scripts/patch_shortcuts_font.py`,
  because egui does not apply the upstream GSUB lookup.

## NV Legible Next interface font

- Files: `assets/fonts/nv-legible-next/NV_Legible_Next-Regular.ttf` and
  `assets/fonts/nv-legible-next/NV_Legible_Next-Bold.ttf`.
- Source: [Nico Verbruggen's ebook-fonts collection](https://github.com/nicoverbruggen/ebook-fonts/tree/f875aca16d3e871a01af52c854b67bc7f7abce9d/fonts/core), commit `f875aca16d3e871a01af52c854b67bc7f7abce9d`.
- Based on Atkinson Hyperlegible Next by the Braille Institute, Applied Design Works,
  Elliott Scott, Megan Eiswerth, and Letters From Sweden. Modified and renamed by Nico Verbruggen.
- License: SIL Open Font License 1.1. The license and original copyright notice are in
  `assets/fonts/nv-legible-next/OFL.txt`.
