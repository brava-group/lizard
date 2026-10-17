# Attribution

Every asset bundled in this repository (images, icons, fonts, data files) is listed here with its
author, source and licence. `cargo xtask assets` fails if a file under `assets/`, `docs/brand/` or
`docs/images/` is missing from this list.

FigCraft's user-interface icons use the UI3 set listed below. The FigCraft brand mark and
the fallback drawings in `crates/ui-egui/src/icons.rs` are original work,
MIT OR Apache-2.0.

## Assets in this repository

| Path | Author | Source | Licence | Notes |
|---|---|---|---|---|
| `assets/app-icon/` (all files: `figcraft.svg`, `figcraft-small.svg`, `figcraft-*.png`, `figcraft.ico`, `hicolor/`) | The project owner (engraving-style drawing, vectorised) | Original work, see [`assets/app-icon/README.md`](assets/app-icon/README.md); derived files rendered by `cargo xtask icon` | MIT OR Apache-2.0, [`assets/app-icon/LICENSE.txt`](assets/app-icon/LICENSE.txt) | FigCraft app icon (chameleon) |
| `crates/ui-egui/assets/ui3-vectors.json` | Figma (UI3 base shapes); Abhimanyu Rana / @planetabhi (polished UI3 icon set) | [UI3 Icons](https://3icons.pages.dev/), based on [UI3: Figma's UI Kit](https://www.figma.com/community/file/1486123838948777078/ui3-figmas-ui-kit) | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the free Community file; [MIT](https://www.npmjs.com/package/@ui3/icons) claimed for the icon package | SVG paths gathered by `tools/generate_ui3_vectors.py`; the UI tessellates and tints them as vector geometry at runtime, with a slight geometric stroke expansion. No icon bitmap is shipped. |
| `crates/ui-egui/assets/layout-*.svg` (18 files) | Figma (UI3 interface icons) | SVG path data supplied by the project owner from the UI3 Layout panel reference | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the free Community file | Layout controls use these paths as vector geometry at runtime; no raster icon is shipped. |
| `crates/ui-egui/assets/appearance-*.svg` (9 files) | Figma (UI3 interface icons) | SVG path data supplied by the project owner from the UI3 Appearance panel reference | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the free Community file | Appearance controls use these paths as vector geometry at runtime; no raster icon is shipped. |
| `crates/ui-egui/assets/colorpicker-*.svg` (17 files) | Figma (UI3 interface icons) | SVG path data and geometry supplied by the project owner from the UI3 color picker, pattern, and image references | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the free Community file | Color picker controls use vector geometry at their native sizes. |
| `crates/ui-egui/assets/property-*.svg` (4 files) | Figma (UI3 interface icons) | Icon geometry supplied by the project owner in the Fill and Stroke property references | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the free Community file | Property controls use vector geometry at their native sizes. |
| `crates/ui-egui/assets/stroke-side-*.svg` (5 files) | Figma (UI3 interface icons); FigCraft contributors (side variants) | Check geometry supplied by the project owner; side icons recreated as SVG primitives from the supplied Stroke popover reference | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the supplied geometry; MIT OR Apache-2.0 for the recreated variants | Stroke side menu icons use vector geometry at their native sizes. |
| `crates/ui-egui/assets/layer-*.svg` (8 files) | Figma (UI3 interface icons); FigCraft contributors (recreated geometry) | Layer tree glyphs recreated from SVG paths and dimensions supplied by the project owner | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the supplied reference; MIT OR Apache-2.0 for the recreated variants | Layer type, auto layout, absolute positioning and disclosure glyphs render as vector geometry at their native sizes. |
| `crates/ui-egui/assets/tab-document.svg` | Figma (UI3 interface icon) | SVG path supplied by the project owner for the document tabs | [CC BY 4.0](https://help.figma.com/hc/en-us/articles/360042296374-Figma-Community-copyright-and-licensing) for the free Community file | Rendered as tinted vector geometry at 19 × 19. |
| `docs/brand/artcraft-logo.svg`, `docs/brand/artcraft-logo.png`, `docs/brand/artcraft-logo-white.svg`, `docs/brand/artcraft-logo-white.png`, `docs/brand/artcraft-mark.svg`, `docs/brand/artcraft-mark.png`, `docs/brand/artcraft-mark-black.svg`, `docs/brand/artcraft-mark-black.png` | ArtCraft Team | storytold/craftrules `assets/brand/` | ArtCraft brand terms (`docs/brand/LICENSE-brand.txt`), not open source | Trademarks; unmodified use within FigCraft only |
| `docs/images/` (all screenshots and exports: `shot-*.png`, `art-*.png`) | FigCraft contributors | Screenshots of FigCraft showing synthetic demo documents made in FigCraft, and PNG exports of those documents by FigCraft's renderer | MIT OR Apache-2.0 | No third-party artwork |
| `examples/*.figcraft` | FigCraft contributors | Synthetic demo documents built through FigCraft's command API; invented copy, shapes and vectors drawn from scratch, no images or embedded fonts | MIT OR Apache-2.0 | Text uses Inter (OFL, from craft-fonts or the system); no third-party artwork |

## Fonts (build input, not in this repository)

Release builds embed fonts from the optional craft-fonts build input
([storytold/craft-fonts](https://github.com/storytold/craft-fonts), `CRAFT_FONTS_DIR`). Its own
`ATTRIBUTION.md` and licence files are authoritative; the packages ship each font's licence.

| Family | Licence | Upstream |
|---|---|---|
| Inter | SIL Open Font License 1.1 | https://github.com/rsms/inter |
| Roboto, Montserrat, Lato, Open Sans, Poppins, Nunito Sans, Merriweather, Playfair Display, Source Sans 3, Source Serif 4, Carlito, Caladea | SIL Open Font License 1.1 | Google Fonts / upstream repositories (see craft-fonts) |
| Liberation Sans, Serif, Mono | SIL Open Font License 1.1 | https://github.com/liberationfonts |
| BIZ UDPGothic, BIZ UDMincho, Shippori Mincho, Noto Sans Arabic | SIL Open Font License 1.1 | Google Fonts (see craft-fonts) |

egui's default fonts (compiled into the `egui`/`epaint` crates) keep their own licences (OFL / Apache-2.0 / Ubuntu Font Licence); see those crates.
