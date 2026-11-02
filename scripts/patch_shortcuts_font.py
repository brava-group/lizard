"""Expose Shortcuts Font's single-character ligatures to egui's glyph renderer.

The upstream font leaves cmap glyphs empty and paints them through GSUB, including
single-character substitutions. egui does not shape those substitutions, so make
their outline glyphs directly addressable in cmap. Multi-character ligatures remain
unmodified; their individual characters still render with this font.
"""

from pathlib import Path

from fontTools.ttLib import TTFont


ROOT = Path(__file__).resolve().parents[1]
DIRECTORY = ROOT / "assets/fonts/shortcuts-font"
font = TTFont(DIRECTORY / "ShortcutsFont-original.ttf")
lookup = font["GSUB"].table.LookupList.Lookup[0]
ligatures = {
    glyph: ligature_list
    for subtable in lookup.SubTable
    for glyph, ligature_list in subtable.ExtSubTable.ligatures.items()
}
single = {
    glyph: ligature.LigGlyph
    for glyph, entries in ligatures.items()
    for ligature in entries
    if not ligature.Component
}

original_cmap = font.getBestCmap()
glyph_to_char = {glyph: chr(codepoint) for codepoint, glyph in original_cmap.items()}
joined = {
    "".join(glyph_to_char.get(part, "") for part in [first, *ligature.Component]): ligature.LigGlyph
    for first, entries in ligatures.items()
    for ligature in entries
    if ligature.Component
}

# Fixed private-use codepoints for the multi-character key names that egui does not shape.
names = [
    "Ctrl", "Shift", "Alt", "Enter", "Esc", "Tab", "Backspace", "Delete",
    "PageUp", "PageDown", *[f"F{number}" for number in range(1, 13)],
]
missing = [name for name in names if name not in joined]
if missing:
    raise ValueError(f"Missing upstream shortcut ligatures: {missing}")

for table in font["cmap"].tables:
    if table.isUnicode():
        table.cmap = {codepoint: single.get(glyph, glyph) for codepoint, glyph in table.cmap.items()}
        table.cmap.update({0xE000 + index: joined[name] for index, name in enumerate(names)})

font.save(DIRECTORY / "ShortcutsFont.ttf")
