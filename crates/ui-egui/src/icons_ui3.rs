//! UI3 SVG paths tessellated into egui meshes. No icon bitmaps or textures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use egui::{Color32, Mesh, Painter, Rect, Shape, pos2};
use lyon_tessellation::path::{Path, math::point};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};
use usvg::tiny_skia_path::PathSegment;

#[path = "ui3_mapping.rs"]
mod mapping;

const VECTORS: &str = include_str!("../assets/ui3-vectors.json");
const TAB_DOCUMENT_SVG: &str = include_str!("../assets/tab-document.svg");
const MODE_SVGS: [&str; 4] = [
    include_str!("../assets/mode-draw.svg"),
    include_str!("../assets/mode-design.svg"),
    include_str!("../assets/mode-motion.svg"),
    include_str!("../assets/mode-dev.svg"),
];
const LAYER_SVGS: [(&str, [f32; 2]); 8] = [
    (include_str!("../assets/layer-frame.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-layout-horizontal.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-layout-vertical.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-layout-grid.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-text.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-absolute.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-chevron-down.svg"), [5.237, 3.122]),
    (include_str!("../assets/layer-chevron-right.svg"), [3.121, 5.243]),
];
const NAV_SVGS: [(&str, [f32; 2]); 5] = [
    (include_str!("../assets/nav-logo.svg"), [85.84124, 94.0]),
    (include_str!("../assets/nav-file.svg"), [10.0, 14.0]),
    (include_str!("../assets/nav-agent.svg"), [14.0, 14.0]),
    (include_str!("../assets/nav-assets.svg"), [14.0, 14.0]),
    (include_str!("../assets/nav-variables.svg"), [12.0, 13.23847]),
];
const HEADER_SVGS: [(&str, [f32; 2]); 10] = [
    (include_str!("../assets/header-present.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-zoom-chevron.svg"), [16.0, 16.0]),
    (include_str!("../assets/header-avatar-ring.svg"), [30.0, 40.0]),
    (include_str!("../assets/header-create-component.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-mask.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-mask-tertiary.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-union.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-union-tertiary.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-edit-object.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-select-matching.svg"), [24.0, 24.0]),
];
const POSITION_SVGS: [&str; 10] = [
    include_str!("../assets/position-align-left.svg"),
    include_str!("../assets/position-align-hcenter.svg"),
    include_str!("../assets/position-align-right.svg"),
    include_str!("../assets/position-align-top.svg"),
    include_str!("../assets/position-align-vcenter.svg"),
    include_str!("../assets/position-align-bottom.svg"),
    include_str!("../assets/position-rotation.svg"),
    include_str!("../assets/position-rotate-right.svg"),
    include_str!("../assets/position-flip-horizontal.svg"),
    include_str!("../assets/position-flip-vertical.svg"),
];
const APPEARANCE_SVGS: [&str; 9] = [
    include_str!("../assets/appearance-visible.svg"),
    include_str!("../assets/appearance-blend.svg"),
    include_str!("../assets/appearance-opacity.svg"),
    include_str!("../assets/appearance-corners.svg"),
    include_str!("../assets/appearance-corner-top-left.svg"),
    include_str!("../assets/appearance-corner-top-right.svg"),
    include_str!("../assets/appearance-corner-bottom-left.svg"),
    include_str!("../assets/appearance-corner-bottom-right.svg"),
    include_str!("../assets/appearance-corner-settings.svg"),
];
const COLORPICKER_SVGS: [(&str, [f32; 2]); 17] = [
    (include_str!("../assets/colorpicker-solid.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-gradient.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-pattern.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-image.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-video.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-shader.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-contrast.svg"), [14.0, 14.0]),
    (include_str!("../assets/colorpicker-chevron.svg"), [5.001_525, 3.0017872]),
    (include_str!("../assets/colorpicker-add-style.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-close.svg"), [10.009888, 10.005533]),
    (include_str!("../assets/colorpicker-eyedropper.svg"), [13.980761, 13.977587]),
    (include_str!("../assets/colorpicker-gradient-flip.svg"), [12.000786, 13.000001]),
    (include_str!("../assets/colorpicker-gradient-orientation.svg"), [10.001458, 14.258431]),
    (include_str!("../assets/colorpicker-gradient-remove.svg"), [12.0, 1.0]),
    (include_str!("../assets/colorpicker-pattern-source.svg"), [13.0, 13.0]),
    (include_str!("../assets/colorpicker-pattern-scale.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-image-create.svg"), [13.939, 13.939]),
];
const PROPERTY_SVGS: [&str; 4] = [
    include_str!("../assets/property-styles.svg"),
    include_str!("../assets/property-stroke-weight.svg"),
    include_str!("../assets/property-stroke-settings.svg"),
    include_str!("../assets/property-stroke-sides.svg"),
];
const STROKE_SIDE_SVGS: [(&str, [f32; 2]); 5] = [
    (include_str!("../assets/stroke-side-check.svg"), [8.992507, 8.487045]),
    (include_str!("../assets/stroke-side-top.svg"), [12.0, 11.0]),
    (include_str!("../assets/stroke-side-bottom.svg"), [12.0, 11.0]),
    (include_str!("../assets/stroke-side-left.svg"), [11.0, 12.0]),
    (include_str!("../assets/stroke-side-right.svg"), [11.0, 12.0]),
];
const LAYOUT_SVGS: [(&str, [f32; 2]); 18] = [
    (include_str!("../assets/layout-aspect-ratio.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-auto.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-free.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-vertical.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-horizontal.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-grid.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-wrap.svg"), [16.0, 16.0]),
    (include_str!("../assets/layout-gap-vertical.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-settings.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-horizontal.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-vertical.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-individual.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-auto-active.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-individual-active.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-left.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-right.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-top.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-bottom.svg"), [24.0, 24.0]),
];
const SIZING_SVGS: [(&str, [f32; 2]); 12] = [
    (include_str!("../assets/sizing-fixed.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-hug.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-fill.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-check.svg"), [18.0, 24.0]),
    (include_str!("../assets/sizing-min.svg"), [16.0, 16.0]),
    (include_str!("../assets/sizing-max.svg"), [16.0, 16.0]),
    (include_str!("../assets/sizing-variable.svg"), [12.0, 12.0]),
    (include_str!("../assets/sizing-height-fixed.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-height-hug.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-height-fill.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-height-min.svg"), [16.0, 16.0]),
    (include_str!("../assets/sizing-height-max.svg"), [16.0, 16.0]),
];
const ALIGNMENT_SVGS: [&str; 6] = [
    include_str!("../assets/alignment-column-left.svg"),
    include_str!("../assets/alignment-column-center.svg"),
    include_str!("../assets/alignment-column-right.svg"),
    include_str!("../assets/alignment-row-top.svg"),
    include_str!("../assets/alignment-row-middle.svg"),
    include_str!("../assets/alignment-row-bottom.svg"),
];
/// Extra width in the source SVG's 24-unit view box. A 20-point toolbar icon
/// gains one third of a point, keeping small icons legible without soft edges.
const STROKE_BOOST: f32 = 0.4;

struct Part {
    positions: Vec<[f32; 2]>,
    indices: Vec<u32>,
    opacity: f32,
}

struct Geometry(Vec<Part>);

#[derive(Clone, Default)]
struct TabDocumentTextures(Arc<Mutex<HashMap<u32, egui::TextureHandle>>>);

fn sources() -> &'static [String] {
    static SOURCES: OnceLock<Vec<String>> = OnceLock::new();
    SOURCES.get_or_init(|| serde_json::from_str(VECTORS).unwrap_or_default())
}

fn cached(index: usize, exact: bool) -> Option<Arc<Geometry>> {
    type GeometryCache = Mutex<HashMap<(usize, bool), Option<Arc<Geometry>>>>;
    static CACHE: OnceLock<GeometryCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(value) = cache.lock().ok()?.get(&(index, exact)) {
        return value.clone();
    }
    let geometry = sources().get(index).and_then(|svg| build(svg, exact)).map(Arc::new);
    cache.lock().ok()?.insert((index, exact), geometry.clone());
    geometry
}

fn to_path(source: &usvg::tiny_skia_path::Path) -> Path {
    let mut builder = Path::builder();
    let mut open = false;
    for segment in source.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                if open {
                    builder.end(false);
                }
                builder.begin(point(p.x, p.y));
                open = true;
            }
            PathSegment::LineTo(p) if open => {
                builder.line_to(point(p.x, p.y));
            }
            PathSegment::QuadTo(c, p) if open => {
                builder.quadratic_bezier_to(point(c.x, c.y), point(p.x, p.y));
            }
            PathSegment::CubicTo(c1, c2, p) if open => {
                builder.cubic_bezier_to(point(c1.x, c1.y), point(c2.x, c2.y), point(p.x, p.y));
            }
            PathSegment::Close if open => {
                builder.end(true);
                open = false;
            }
            _ => {}
        }
    }
    if open {
        builder.end(false);
    }
    builder.build()
}

fn part(buffer: VertexBuffers<lyon_tessellation::math::Point, u32>, opacity: f32) -> Option<Part> {
    if buffer.indices.is_empty() {
        return None;
    }
    Some(Part { positions: buffer.vertices.into_iter().map(|p| [p.x, p.y]).collect(), indices: buffer.indices, opacity })
}

fn add_path(geometry: &mut Geometry, source: &usvg::Path, exact: bool) {
    let transformed = source.data().clone().transform(source.abs_transform());
    let Some(transformed) = transformed else { return };
    let path = to_path(&transformed);
    let width =
        source.stroke().map_or(if exact { 0.0 } else { STROKE_BOOST }, |stroke| stroke.width().get() + if exact { 0.0 } else { STROKE_BOOST });
    if !exact {
        let mut fringe = VertexBuffers::new();
        let fringe_options = StrokeOptions::default().with_line_width(width + 0.55).with_tolerance(0.05);
        if StrokeTessellator::new()
            .tessellate_path(&path, &fringe_options, &mut BuffersBuilder::new(&mut fringe, |vertex: StrokeVertex<'_, '_>| vertex.position()))
            .is_ok()
            && let Some(part) = part(fringe, 0.22)
        {
            geometry.0.push(part);
        }
    }
    if let Some(fill) = source.fill() {
        let rule = match fill.rule() {
            usvg::FillRule::EvenOdd => FillRule::EvenOdd,
            usvg::FillRule::NonZero => FillRule::NonZero,
        };
        let options = FillOptions::default().with_fill_rule(rule).with_tolerance(0.05);
        let mut buffer = VertexBuffers::new();
        if FillTessellator::new()
            .tessellate_path(&path, &options, &mut BuffersBuilder::new(&mut buffer, |vertex: FillVertex<'_>| vertex.position()))
            .is_ok()
            && let Some(part) = part(buffer, 1.0)
        {
            geometry.0.push(part);
        }
    }
    // The published set consists of filled paths except for one stroked icon.
    // A narrow geometric stroke around the fills improves the toolbar weight.
    if width > 0.0 {
        let options = StrokeOptions::default().with_line_width(width).with_tolerance(0.05);
        let mut buffer = VertexBuffers::new();
        if StrokeTessellator::new()
            .tessellate_path(&path, &options, &mut BuffersBuilder::new(&mut buffer, |vertex: StrokeVertex<'_, '_>| vertex.position()))
            .is_ok()
            && let Some(part) = part(buffer, 1.0)
        {
            geometry.0.push(part);
        }
    }
}

fn build(svg: &str, exact: bool) -> Option<Geometry> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut geometry = Geometry(Vec::new());
    fn visit(nodes: &[usvg::Node], geometry: &mut Geometry, exact: bool) {
        for node in nodes {
            match node {
                usvg::Node::Path(path) => add_path(geometry, path, exact),
                usvg::Node::Group(group) => visit(group.children(), geometry, exact),
                _ => {}
            }
        }
    }
    visit(tree.root().children(), &mut geometry, exact);
    (!geometry.0.is_empty()).then_some(geometry)
}

pub(super) fn paint(painter: &Painter, rect: Rect, icon: super::Icon, color: Color32) -> bool {
    let Some(index) = mapping::index(icon) else { return false };
    // Keep the chevron's original SVG outline, including its one-unit stroke.
    let Some(geometry) = cached(index, icon == super::Icon::Chevron) else { return false };
    paint_geometry(painter, rect, &geometry, color, [24.0, 24.0]);
    true
}

pub(super) fn paint_mode(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static MODES: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let modes = MODES.get_or_init(|| MODE_SVGS.iter().map(|svg| build(svg, true)).collect());
    if let Some(Some(geometry)) = modes.get(index) {
        paint_geometry(painter, rect, geometry, color, [24.0, 24.0]);
    }
}

pub(super) fn paint_tab_document(painter: &Painter, rect: Rect, color: Color32) {
    // egui meshes have hard triangle edges. Rasterize this small SVG at twice the display
    // resolution so its curves and diagonal remain smooth after texture filtering.
    let pixels = (rect.width() * painter.ctx().pixels_per_point().clamp(1.0, 4.0) * 2.0).round() as u32;
    let cache = painter.ctx().data_mut(|data| data.get_temp_mut_or_default::<TabDocumentTextures>(egui::Id::new("tab_document_textures")).clone());
    let existing = cache.0.lock().ok().and_then(|textures| textures.get(&pixels).cloned());
    let texture = existing.or_else(|| {
        let rgba = raster_tab_document(pixels)?;
        let image = egui::ColorImage::from_rgba_unmultiplied([pixels as usize, pixels as usize], &rgba);
        let texture = painter.ctx().load_texture("tab-document", image, egui::TextureOptions::LINEAR);
        cache.0.lock().ok()?.insert(pixels, texture.clone());
        Some(texture)
    });
    if let Some(texture) = texture {
        painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    }
}

fn raster_tab_document(pixels: u32) -> Option<Vec<u8>> {
    if !(1..=256).contains(&pixels) {
        return None;
    }
    let tree = usvg::Tree::from_str(TAB_DOCUMENT_SVG, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(pixels, pixels)?;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let scale = pixels as f32 / 19.0;
    for node in tree.root().children() {
        if let usvg::Node::Path(source) = node
            && let Some(fill) = source.fill()
            && let Some(path) = source.data().clone().transform(source.abs_transform())
        {
            let rule = match fill.rule() {
                usvg::FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
                usvg::FillRule::NonZero => tiny_skia::FillRule::Winding,
            };
            pixmap.fill_path(&path, &paint, rule, tiny_skia::Transform::from_scale(scale, scale), None);
        }
    }
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.data().chunks_exact(4) {
        rgba.extend_from_slice(&[255, 255, 255, pixel[3]]);
    }
    Some(rgba)
}

pub(super) fn paint_header(painter: &Painter, rect: Rect, icon: super::HeaderIcon, color: Color32) {
    static HEADER: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = HEADER.get_or_init(|| HEADER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::HeaderIcon::Present => 0,
        super::HeaderIcon::ZoomChevron => 1,
        super::HeaderIcon::AvatarRing => 2,
        super::HeaderIcon::CreateComponent => 3,
        super::HeaderIcon::Mask => 4,
        super::HeaderIcon::MaskTertiary => 5,
        super::HeaderIcon::Union => 6,
        super::HeaderIcon::UnionTertiary => 7,
        super::HeaderIcon::EditObject => 8,
        super::HeaderIcon::SelectMatching => 9,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, HEADER_SVGS[index].1);
    }
}

pub(super) fn paint_position(painter: &Painter, rect: Rect, icon: super::PositionIcon, color: Color32, tertiary: Color32) {
    static POSITION: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = POSITION.get_or_init(|| POSITION_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = match icon {
        super::PositionIcon::AlignLeft => 0,
        super::PositionIcon::AlignHCenter => 1,
        super::PositionIcon::AlignRight => 2,
        super::PositionIcon::AlignTop => 3,
        super::PositionIcon::AlignVCenter => 4,
        super::PositionIcon::AlignBottom => 5,
        super::PositionIcon::Rotation => 6,
        super::PositionIcon::RotateRight => 7,
        super::PositionIcon::FlipH => 8,
        super::PositionIcon::FlipV => 9,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry_colored(painter, rect, geometry, color, Some(tertiary), [24.0, 24.0]);
    }
}

pub(super) fn paint_layout(painter: &Painter, rect: Rect, icon: super::LayoutIcon, color: Color32) {
    static LAYOUT: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = LAYOUT.get_or_init(|| LAYOUT_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::LayoutIcon::AspectRatio => 0,
        super::LayoutIcon::AutoLayout => 1,
        super::LayoutIcon::Free => 2,
        super::LayoutIcon::Vertical => 3,
        super::LayoutIcon::Horizontal => 4,
        super::LayoutIcon::Grid => 5,
        super::LayoutIcon::Wrap => 6,
        super::LayoutIcon::GapVertical => 7,
        super::LayoutIcon::Settings => 8,
        super::LayoutIcon::PaddingHorizontal => 9,
        super::LayoutIcon::PaddingVertical => 10,
        super::LayoutIcon::PaddingIndividual => 11,
        super::LayoutIcon::AutoLayoutActive => 12,
        super::LayoutIcon::PaddingIndividualActive => 13,
        super::LayoutIcon::PaddingLeft => 14,
        super::LayoutIcon::PaddingRight => 15,
        super::LayoutIcon::PaddingTop => 16,
        super::LayoutIcon::PaddingBottom => 17,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, LAYOUT_SVGS[index].1);
    }
}

pub(super) fn paint_appearance(painter: &Painter, rect: Rect, icon: super::AppearanceIcon, color: Color32) {
    static APPEARANCE: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = APPEARANCE.get_or_init(|| APPEARANCE_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = match icon {
        super::AppearanceIcon::Visible => 0,
        super::AppearanceIcon::Blend => 1,
        super::AppearanceIcon::Opacity => 2,
        super::AppearanceIcon::Corners => 3,
        super::AppearanceIcon::CornerTopLeft => 4,
        super::AppearanceIcon::CornerTopRight => 5,
        super::AppearanceIcon::CornerBottomLeft => 6,
        super::AppearanceIcon::CornerBottomRight => 7,
        super::AppearanceIcon::CornerSettings => 8,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, [24.0, 24.0]);
    }
}

pub(super) fn paint_colorpicker(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static COLORPICKER: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = COLORPICKER.get_or_init(|| COLORPICKER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, COLORPICKER_SVGS[index].1);
    }
}

pub(super) fn paint_property(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static PROPERTY: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = PROPERTY.get_or_init(|| PROPERTY_SVGS.iter().map(|svg| build(svg, true)).collect());
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, [12.0, 12.0]);
    }
}

pub(super) fn paint_stroke_side(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static STROKE_SIDES: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = STROKE_SIDES.get_or_init(|| STROKE_SIDE_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    if let (Some(Some(geometry)), Some((_, size))) = (icons.get(index), STROKE_SIDE_SVGS.get(index)) {
        paint_geometry(painter, rect, geometry, color, *size);
    }
}

pub(super) fn paint_layer(painter: &Painter, rect: Rect, icon: super::LayerIcon, color: Color32) {
    static LAYERS: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = LAYERS.get_or_init(|| LAYER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::LayerIcon::Frame => 0,
        super::LayerIcon::LayoutH => 1,
        super::LayerIcon::LayoutV => 2,
        super::LayerIcon::LayoutGrid => 3,
        super::LayerIcon::Text => 4,
        super::LayerIcon::Absolute => 5,
        super::LayerIcon::ChevronDown => 6,
        super::LayerIcon::ChevronRight => 7,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, LAYER_SVGS[index].1);
    }
}

pub(super) fn paint_nav(painter: &Painter, rect: Rect, icon: super::NavIcon, color: Color32) {
    static NAV: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = NAV.get_or_init(|| NAV_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::NavIcon::Logo => 0,
        super::NavIcon::File => 1,
        super::NavIcon::Agent => 2,
        super::NavIcon::Assets => 3,
        super::NavIcon::Variables => 4,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, NAV_SVGS[index].1);
    }
}

pub(super) fn paint_sizing(painter: &Painter, rect: Rect, icon: super::SizingIcon, color: Color32) {
    static SIZING: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = SIZING.get_or_init(|| SIZING_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::SizingIcon::Fixed => 0,
        super::SizingIcon::Hug => 1,
        super::SizingIcon::Fill => 2,
        super::SizingIcon::Check => 3,
        super::SizingIcon::Minimum => 4,
        super::SizingIcon::Maximum => 5,
        super::SizingIcon::Variable => 6,
        super::SizingIcon::HeightFixed => 7,
        super::SizingIcon::HeightHug => 8,
        super::SizingIcon::HeightFill => 9,
        super::SizingIcon::HeightMinimum => 10,
        super::SizingIcon::HeightMaximum => 11,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, SIZING_SVGS[index].1);
    }
}

pub(super) fn paint_alignment(painter: &Painter, rect: Rect, horizontal: bool, position: usize) {
    static ALIGNMENT: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = ALIGNMENT.get_or_init(|| ALIGNMENT_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = (if horizontal { 3 } else { 0 }) + position.min(2);
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, Color32::from_rgb(0x7c, 0xc4, 0xf8), [16.0, 16.0]);
    }
}

fn paint_geometry(painter: &Painter, rect: Rect, geometry: &Geometry, color: Color32, view_box: [f32; 2]) {
    paint_geometry_colored(painter, rect, geometry, color, None, view_box);
}

fn paint_geometry_colored(painter: &Painter, rect: Rect, geometry: &Geometry, color: Color32, tertiary: Option<Color32>, view_box: [f32; 2]) {
    let scale = (rect.width() / view_box[0]).min(rect.height() / view_box[1]);
    let origin = rect.center() - egui::vec2(view_box[0] * scale / 2.0, view_box[1] * scale / 2.0);
    for (index, part) in geometry.0.iter().enumerate() {
        let mut mesh = Mesh::default();
        mesh.vertices.reserve(part.positions.len());
        let tint = if index == 1 { tertiary.unwrap_or(color) } else { color }.gamma_multiply(part.opacity);
        for &[x, y] in &part.positions {
            mesh.vertices.push(egui::epaint::Vertex::untextured(pos2(origin.x + x * scale, origin.y + y * scale), tint));
        }
        mesh.indices.extend_from_slice(&part.indices);
        painter.add(Shape::mesh(mesh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_document_has_antialiased_edges() {
        let rgba = raster_tab_document(56).expect("tab document SVG rasterizes");
        assert!(rgba.chunks_exact(4).any(|pixel| (1..255).contains(&pixel[3])));
    }

    #[test]
    fn sizing_menu_svgs_have_vector_geometry() {
        for (index, (svg, _)) in SIZING_SVGS.into_iter().enumerate() {
            let Some(geometry) = build(svg, true) else { panic!("sizing menu SVG {index} has no geometry") };
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn alignment_svgs_have_vector_geometry() {
        for (index, svg) in ALIGNMENT_SVGS.into_iter().enumerate() {
            let Some(geometry) = build(svg, true) else { panic!("alignment SVG {index} has no geometry") };
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn every_ui_icon_has_vector_geometry() {
        for &icon in super::super::Icon::ALL {
            if icon == super::super::Icon::Logo {
                continue;
            }
            let index = mapping::index(icon).unwrap();
            let geometry = cached(index, icon == super::super::Icon::Chevron).unwrap_or_else(|| panic!("{icon:?} has no geometry"));
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()), "{icon:?} has an empty mesh");
        }
    }

    #[test]
    fn mode_icons_use_their_original_svg_paths() {
        for svg in MODE_SVGS {
            let geometry = build(svg, true).expect("mode icon has valid SVG geometry");
            assert!(geometry.0.iter().all(|part| part.opacity == 1.0));
        }
    }

    #[test]
    fn header_icons_use_their_original_svg_paths() {
        for (svg, _) in HEADER_SVGS {
            let geometry = build(svg, true).expect("header icon has valid SVG geometry");
            assert!(geometry.0.iter().all(|part| part.opacity == 1.0));
        }
    }

    #[test]
    fn position_icons_keep_svg_paths_and_secondary_alignment_guides() {
        for (index, svg) in POSITION_SVGS.iter().enumerate() {
            let geometry = build(svg, true).expect("position icon has valid SVG geometry");
            assert_eq!(geometry.0.len(), if index < 6 { 2 } else { 1 });
        }
    }

    #[test]
    fn appearance_reference_svgs_have_renderable_paths() {
        for (index, svg) in APPEARANCE_SVGS.iter().enumerate() {
            let geometry = build(svg, true).unwrap_or_else(|| panic!("appearance SVG {index} has no geometry"));
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn layout_reference_svgs_have_renderable_paths() {
        for (svg, _) in LAYOUT_SVGS {
            assert!(build(svg, true).is_some(), "layout SVG did not produce vector geometry");
        }
    }
}
