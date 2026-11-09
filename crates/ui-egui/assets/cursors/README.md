# Lizard cursors

`source.lzd` is the owner's editable `cursors-lizard.lzd` document. The PNGs used by
the desktop cursor and the agent canvas presence were exported from its named
components at 8 pixels per document point. Canvas cursor exports are cropped to
each component's 32 × 32 point bounds and retain transparency.

`cursors.rs` maps each editor state to its component and records its click point.
`cut.png` and `bend-curve.png` retain the previous artwork because the source
document does not contain matching components for those two tool states.
