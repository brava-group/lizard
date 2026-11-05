//! Canvas cursors from the owner's local `cursores.html` reference.
//!
//! Its SVG files contain embedded PNG images, so the corresponding PNG payloads are shown
//! directly. This preserves the supplied artwork without tracing or redrawing it.

use std::{collections::HashMap, sync::Arc};

use egui::{CursorIcon, CustomCursorImage, vec2};
use lizard_engine::tools::{Cursor, PenCursor};

/// The engine cursor's visual state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyph {
    Arrow,
    Move,
    Crosshair,
    Resize(f32),
    Hand(bool),
    Copy,
    Zoom(bool),
    Brush,
    Pencil,
    PaintBucket,
    PaintBucketAdd,
    PaintBucketRemove,
    Lasso,
    Cut,
    Eraser,
    Rotate(f32),
    Pen(PenCursor),
    Eyedropper,
    Comment,
    Pointer,
}

/// A cursor rendered by the OS or by the reference image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shown {
    System(CursorIcon),
    Drawn(Glyph),
}

/// Decoded cursor bitmaps are shared across frames so winit can cache each OS cursor.
#[derive(Default)]
pub struct CursorTextures {
    loaded: HashMap<(Asset, u32, u8), CustomCursorImage>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Asset {
    Move,
    Crosshair,
    ResizeHorizontal,
    ResizeVertical,
    ResizeDiagonalDr,
    ResizeDiagonalUr,
    Rotate,
    Hand,
    HandPress,
    Duplicate,
    ZoomIn,
    ZoomOut,
    Pen,
    Brush,
    Pencil,
    PenDot,
    PenInsert,
    Bend,
    BendCurve,
    Lasso,
    Cut,
    Eraser,
    Eyedropper,
    Comment,
    PaintBucket,
    PaintBucketInsert,
    PaintBucketRemove,
    Click,
}

impl Asset {
    #[cfg(test)]
    fn name(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Crosshair => "crosshair",
            Self::ResizeHorizontal => "resize-horizontal",
            Self::ResizeVertical => "resize-vertical",
            Self::ResizeDiagonalDr => "resize-diagonal-dr",
            Self::ResizeDiagonalUr => "resize-diagonal-ur",
            Self::Rotate => "rotate",
            Self::Hand => "hand",
            Self::HandPress => "hand-press",
            Self::Duplicate => "duplicate",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
            Self::Pen => "pen",
            Self::Brush => "brush",
            Self::Pencil => "pencil",
            Self::PenDot => "pen-dot",
            Self::PenInsert => "pen-insert",
            Self::Bend => "bend",
            Self::BendCurve => "bend-curve",
            Self::Lasso => "lasso",
            Self::Cut => "cut",
            Self::Eraser => "eraser",
            Self::Eyedropper => "eyedropper",
            Self::Comment => "comment",
            Self::PaintBucket => "paint-bucket",
            Self::PaintBucketInsert => "paint-bucket-insert",
            Self::PaintBucketRemove => "paint-bucket-remove",
            Self::Click => "click",
        }
    }

    fn png(self) -> &'static [u8] {
        match self {
            Self::Move => include_bytes!("../assets/cursors/move.png"),
            Self::Crosshair => include_bytes!("../assets/cursors/crosshair.png"),
            Self::ResizeHorizontal => include_bytes!("../assets/cursors/resize-horizontal.png"),
            Self::ResizeVertical => include_bytes!("../assets/cursors/resize-vertical.png"),
            Self::ResizeDiagonalDr => include_bytes!("../assets/cursors/resize-diagonal-dr.png"),
            Self::ResizeDiagonalUr => include_bytes!("../assets/cursors/resize-diagonal-ur.png"),
            Self::Rotate => include_bytes!("../assets/cursors/rotate.png"),
            Self::Hand => include_bytes!("../assets/cursors/hand.png"),
            Self::HandPress => include_bytes!("../assets/cursors/hand-press.png"),
            Self::Duplicate => include_bytes!("../assets/cursors/duplicate.png"),
            Self::ZoomIn => include_bytes!("../assets/cursors/zoom-in.png"),
            Self::ZoomOut => include_bytes!("../assets/cursors/zoom-out.png"),
            Self::Pen => include_bytes!("../assets/cursors/pen.png"),
            Self::Brush => include_bytes!("../assets/cursors/brush.png"),
            Self::Pencil => include_bytes!("../assets/cursors/pencil.png"),
            Self::PenDot => include_bytes!("../assets/cursors/pen-dot.png"),
            Self::PenInsert => include_bytes!("../assets/cursors/pen-insert.png"),
            Self::Bend => include_bytes!("../assets/cursors/bend.png"),
            Self::BendCurve => include_bytes!("../assets/cursors/bend-curve.png"),
            Self::Lasso => include_bytes!("../assets/cursors/lasso.png"),
            Self::Cut => include_bytes!("../assets/cursors/cut.png"),
            Self::Eraser => include_bytes!("../assets/cursors/eraser.png"),
            Self::Eyedropper => include_bytes!("../assets/cursors/eyedropper.png"),
            Self::Comment => include_bytes!("../assets/cursors/comment.png"),
            Self::PaintBucket => include_bytes!("../assets/cursors/paint-bucket.png"),
            Self::PaintBucketInsert => include_bytes!("../assets/cursors/paint-bucket-insert.png"),
            Self::PaintBucketRemove => include_bytes!("../assets/cursors/paint-bucket-remove.png"),
            Self::Click => include_bytes!("../assets/cursors/click.png"),
        }
    }

    /// Hot spot in the 32 × 32 image, in logical points.
    fn hotspot(self) -> egui::Vec2 {
        match self {
            Self::Move | Self::Duplicate | Self::Pen | Self::PenDot | Self::PenInsert | Self::Lasso | Self::Cut | Self::Click => vec2(0.0, 0.0),
            Self::Brush | Self::Pencil => vec2(2.0, 2.0),
            Self::Eyedropper => vec2(5.0, 27.0),
            Self::Comment => vec2(6.0, 27.0),
            Self::PaintBucket | Self::PaintBucketInsert | Self::PaintBucketRemove => vec2(8.0, 24.0),
            _ => vec2(16.0, 16.0),
        }
    }
}

impl CursorTextures {
    fn get(&mut self, ctx: &egui::Context, asset: Asset, quarter_turns: u8) -> Option<&CustomCursorImage> {
        // winit's macOS cursor creates an NSImage whose logical size equals this pixel size.
        // Scaling by Retina density there would make the pointer physically twice as large.
        let pixels = if cfg!(target_os = "macos") { 32 } else { (32.0 * ctx.pixels_per_point()).round().clamp(32.0, 256.0) as u32 };
        let quarter_turns = if asset == Asset::Rotate { quarter_turns % 4 } else { 0 };
        let key = (asset, pixels, quarter_turns);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.loaded.entry(key) {
            let png = image::load_from_memory_with_format(asset.png(), image::ImageFormat::Png).ok()?.into_rgba8();
            // egui's linear sampler has no mip levels. Downsample with a proper reconstruction
            // filter so the 256 px source stays crisp at its 32 pt display size.
            let rgba = image::imageops::resize(&png, pixels, pixels, image::imageops::FilterType::Lanczos3);
            let rgba = match quarter_turns {
                1 => image::imageops::rotate90(&rgba),
                2 => image::imageops::rotate180(&rgba),
                3 => image::imageops::rotate270(&rgba),
                _ => rgba,
            };
            let scale = pixels as f32 / 32.0;
            let hotspot = asset.hotspot() * scale;
            let side = u16::try_from(pixels).ok()?;
            entry.insert(CustomCursorImage {
                rgba: Arc::from(rgba.into_raw()),
                size: [side, side],
                hotspot: [hotspot.x.round() as u16, hotspot.y.round() as u16],
            });
        }
        self.loaded.get(&key)
    }
}

impl Glyph {
    /// The source image points up and left, matching the bottom-right corner. The corner's
    /// centre-to-handle angle selects one of four right-angle rotations of that exact image.
    fn quarter_turns(self) -> u8 {
        match self {
            Self::Rotate(degrees) if degrees.is_finite() => (degrees.rem_euclid(360.0) / 90.0).floor() as u8,
            _ => 0,
        }
    }

    fn asset(self) -> Asset {
        match self {
            Self::Arrow | Self::Move => Asset::Move,
            Self::Crosshair => Asset::Crosshair,
            Self::Resize(degrees) => match resize_icon(f64::from(degrees)) {
                CursorIcon::ResizeHorizontal => Asset::ResizeHorizontal,
                CursorIcon::ResizeVertical => Asset::ResizeVertical,
                CursorIcon::ResizeNwSe => Asset::ResizeDiagonalDr,
                _ => Asset::ResizeDiagonalUr,
            },
            Self::Hand(false) => Asset::Hand,
            Self::Hand(true) => Asset::HandPress,
            Self::Copy => Asset::Duplicate,
            Self::Zoom(false) => Asset::ZoomIn,
            Self::Zoom(true) => Asset::ZoomOut,
            Self::Pen(PenCursor::New) => Asset::Pen,
            Self::Brush => Asset::Brush,
            Self::Pencil => Asset::Pencil,
            Self::Pen(PenCursor::Close) => Asset::PenDot,
            Self::Pen(PenCursor::Add) => Asset::PenInsert,
            Self::Pen(PenCursor::Bend) => Asset::Bend,
            Self::Pen(PenCursor::BendCurve) => Asset::BendCurve,
            Self::PaintBucket => Asset::PaintBucket,
            Self::PaintBucketAdd => Asset::PaintBucketInsert,
            Self::PaintBucketRemove => Asset::PaintBucketRemove,
            Self::Lasso => Asset::Lasso,
            Self::Cut => Asset::Cut,
            Self::Eraser => Asset::Eraser,
            Self::Rotate(_) => Asset::Rotate,
            Self::Eyedropper => Asset::Eyedropper,
            Self::Comment => Asset::Comment,
            Self::Pointer => Asset::Click,
        }
    }
}

/// The nearest resize arrow for a screen angle (0 = horizontal, 90 = vertical).
pub fn resize_icon(degrees: f64) -> CursorIcon {
    let a = degrees.rem_euclid(180.0);
    if !(22.5..157.5).contains(&a) {
        CursorIcon::ResizeHorizontal
    } else if a < 67.5 {
        CursorIcon::ResizeNwSe
    } else if a < 112.5 {
        CursorIcon::ResizeVertical
    } else {
        CursorIcon::ResizeNeSw
    }
}

pub fn of(cursor: Cursor) -> Shown {
    use Shown::{Drawn, System};
    match cursor {
        Cursor::Default => Drawn(Glyph::Arrow),
        Cursor::Move => Drawn(Glyph::Move),
        Cursor::Crosshair => Drawn(Glyph::Crosshair),
        Cursor::Resize(a) if a.is_finite() => Drawn(Glyph::Resize(a.rem_euclid(180.0) as f32)),
        Cursor::Resize(_) => System(CursorIcon::Default),
        Cursor::Rotate(a) => Drawn(Glyph::Rotate(if a.is_finite() { a.rem_euclid(360.0) as f32 } else { 0.0 })),
        Cursor::Grab => Drawn(Glyph::Hand(false)),
        Cursor::Grabbing => Drawn(Glyph::Hand(true)),
        Cursor::Text => System(CursorIcon::Text),
        Cursor::Pen(p) => Drawn(Glyph::Pen(p)),
        Cursor::Brush => Drawn(Glyph::Brush),
        Cursor::Pencil => Drawn(Glyph::Pencil),
        Cursor::PaintBucket => Drawn(Glyph::PaintBucket),
        Cursor::PaintBucketAdd => Drawn(Glyph::PaintBucketAdd),
        Cursor::PaintBucketRemove => Drawn(Glyph::PaintBucketRemove),
        Cursor::Lasso => Drawn(Glyph::Lasso),
        Cursor::Cut => Drawn(Glyph::Cut),
        Cursor::Eraser => Drawn(Glyph::Eraser),
        Cursor::Copy => Drawn(Glyph::Copy),
        Cursor::Pointer => Drawn(Glyph::Pointer),
        Cursor::Eyedropper => Drawn(Glyph::Eyedropper),
        Cursor::Comment => Drawn(Glyph::Comment),
        Cursor::NotAllowed => System(CursorIcon::NotAllowed),
    }
}

/// Pan and zoom override the active tool's cursor.
pub fn on_canvas(panning: bool, button_down: bool, zoom_out: Option<bool>, cursor: Cursor) -> Shown {
    match (panning, zoom_out) {
        (true, _) => Shown::Drawn(Glyph::Hand(button_down)),
        (false, Some(out)) => Shown::Drawn(Glyph::Zoom(out)),
        (false, None) => of(cursor),
    }
}

impl Shown {
    pub fn icon(self) -> CursorIcon {
        match self {
            Self::System(i) => i,
            Self::Drawn(glyph) => match glyph {
                Glyph::Arrow | Glyph::Move => CursorIcon::Default,
                Glyph::Crosshair
                | Glyph::Pen(_)
                | Glyph::Brush
                | Glyph::Pencil
                | Glyph::PaintBucket
                | Glyph::PaintBucketAdd
                | Glyph::PaintBucketRemove
                | Glyph::Lasso
                | Glyph::Cut
                | Glyph::Eraser
                | Glyph::Rotate(_)
                | Glyph::Eyedropper => CursorIcon::Crosshair,
                Glyph::Resize(degrees) => resize_icon(f64::from(degrees)),
                Glyph::Hand(false) => CursorIcon::Grab,
                Glyph::Hand(true) => CursorIcon::Grabbing,
                Glyph::Copy => CursorIcon::Copy,
                Glyph::Zoom(false) => CursorIcon::ZoomIn,
                Glyph::Zoom(true) => CursorIcon::ZoomOut,
                Glyph::Comment | Glyph::Pointer => CursorIcon::PointingHand,
            },
        }
    }

    /// Name reported through `ui.inspect`.
    pub fn name(self) -> String {
        match self {
            Self::System(i) => match i {
                CursorIcon::Default => "default",
                CursorIcon::Text => "text",
                CursorIcon::NotAllowed => "not-allowed",
                _ => "other",
            }
            .into(),
            Self::Drawn(glyph) => match glyph {
                Glyph::Arrow => "default".into(),
                Glyph::Move => "move".into(),
                Glyph::Crosshair => "crosshair".into(),
                Glyph::Resize(a) => match resize_icon(f64::from(a)) {
                    CursorIcon::ResizeHorizontal => "ew-resize",
                    CursorIcon::ResizeVertical => "ns-resize",
                    CursorIcon::ResizeNwSe => "nwse-resize",
                    _ => "nesw-resize",
                }
                .into(),
                Glyph::Hand(false) => "grab".into(),
                Glyph::Hand(true) => "grabbing".into(),
                Glyph::Copy => "copy".into(),
                Glyph::Zoom(false) => "zoom-in".into(),
                Glyph::Zoom(true) => "zoom-out".into(),
                Glyph::Pencil => "pencil".into(),
                Glyph::Brush => "brush".into(),
                Glyph::PaintBucket => "paint-bucket".into(),
                Glyph::PaintBucketAdd => "paint-bucket-add".into(),
                Glyph::PaintBucketRemove => "paint-bucket-remove".into(),
                Glyph::Lasso => "lasso".into(),
                Glyph::Cut => "cut".into(),
                Glyph::Eraser => "eraser".into(),
                Glyph::Rotate(a) => format!("rotate {}", a.round()),
                Glyph::Pen(PenCursor::New) => "pen".into(),
                Glyph::Pen(PenCursor::Close) => "pen-close".into(),
                Glyph::Pen(PenCursor::Add) => "pen-add".into(),
                Glyph::Pen(PenCursor::Bend) => "pen-bend".into(),
                Glyph::Pen(PenCursor::BendCurve) => "pen-bend-curve".into(),
                Glyph::Eyedropper => "eyedropper".into(),
                Glyph::Comment => "comment".into(),
                Glyph::Pointer => "pointer".into(),
            },
        }
    }

    /// Use the same supplied artwork as a native OS cursor, independent of canvas repaint speed.
    pub fn show(self, ctx: &egui::Context, textures: &mut CursorTextures) {
        ctx.set_cursor_icon(self.icon());
        let Self::Drawn(glyph) = self else {
            return;
        };
        let asset = glyph.asset();
        let Some(image) = textures.get(ctx, asset, glyph.quarter_turns()) else {
            ctx.set_cursor_icon(CursorIcon::Default);
            return;
        };
        // Native winit uses cursor_image; web and integrations without bitmap support
        // retain a usable system cursor instead of hiding the pointer.
        ctx.set_cursor_image(Some(image.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::{Asset, Glyph};
    use lizard_engine::tools::{Cursor, PenCursor};

    #[test]
    fn supplied_cursor_images_decode_and_states_use_the_reference_files() {
        for asset in [
            Asset::Move,
            Asset::Crosshair,
            Asset::ResizeHorizontal,
            Asset::ResizeVertical,
            Asset::ResizeDiagonalDr,
            Asset::ResizeDiagonalUr,
            Asset::Rotate,
            Asset::Hand,
            Asset::HandPress,
            Asset::Duplicate,
            Asset::ZoomIn,
            Asset::ZoomOut,
            Asset::Pen,
            Asset::Brush,
            Asset::Pencil,
            Asset::PenDot,
            Asset::PenInsert,
            Asset::Bend,
            Asset::BendCurve,
            Asset::Lasso,
            Asset::Cut,
            Asset::Eraser,
            Asset::Eyedropper,
            Asset::Comment,
            Asset::PaintBucket,
            Asset::PaintBucketInsert,
            Asset::PaintBucketRemove,
            Asset::Click,
        ] {
            let png = image::load_from_memory_with_format(asset.png(), image::ImageFormat::Png).unwrap();
            assert_eq!((png.width(), png.height()), (256, 256), "{}", asset.name());
        }
        assert_eq!(Glyph::Move.asset(), Asset::Move);
        assert_eq!(Glyph::Crosshair.asset(), Asset::Crosshair);
        assert_eq!(Glyph::Copy.asset(), Asset::Duplicate);
        assert_eq!(Glyph::Pen(PenCursor::Close).asset(), Asset::PenDot);
        assert_eq!(Glyph::Pen(PenCursor::Add).asset(), Asset::PenInsert);
        assert_eq!(Glyph::Pen(PenCursor::Bend).asset(), Asset::Bend);
        assert_eq!(Glyph::Pen(PenCursor::BendCurve).asset(), Asset::BendCurve);
        assert_eq!(Glyph::Brush.asset(), Asset::Brush);
        assert_eq!(Glyph::Pencil.asset(), Asset::Pencil);
        assert_eq!(super::of(Cursor::Brush), super::Shown::Drawn(Glyph::Brush));
        assert_eq!(super::of(Cursor::Pencil), super::Shown::Drawn(Glyph::Pencil));
        assert_eq!(Glyph::PaintBucketAdd.asset(), Asset::PaintBucketInsert);
        assert_eq!(Glyph::PaintBucketRemove.asset(), Asset::PaintBucketRemove);
        assert_eq!(Glyph::Lasso.asset(), Asset::Lasso);
        assert_eq!(Glyph::Cut.asset(), Asset::Cut);
        assert_eq!(Glyph::Eraser.asset(), Asset::Eraser);
        assert_eq!(Glyph::Pointer.asset(), Asset::Click);
    }

    #[test]
    fn rotation_cursor_faces_each_selection_corner() {
        // Angles are measured from the box centre, clockwise in screen coordinates.
        for (angle, quarter_turns) in [(45.0, 0), (135.0, 1), (225.0, 2), (315.0, 3), (-45.0, 3)] {
            assert_eq!(Glyph::Rotate(angle).quarter_turns(), quarter_turns, "{angle}°");
        }
        // Non-square selections keep the same visual variant throughout a corner's quadrant.
        for (angle, quarter_turns) in [(12.0, 0), (168.0, 1), (192.0, 2), (348.0, 3)] {
            assert_eq!(Glyph::Rotate(angle).quarter_turns(), quarter_turns, "{angle}°");
        }
    }
}
