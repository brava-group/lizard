//! An experimental GPU renderer for the icon benchmark, with explicit unsupported content.

use std::collections::HashMap;
use std::error::Error;
use std::sync::mpsc;
use std::time::Instant;

use figcraft_color::{BlendMode, Color, GradientKind, ImagePaint, ImageScaleMode, Paint, PaintKind};
use figcraft_doc::{Document, EffectKind, Node, NodeId, NodeKind, Shadow};
use figcraft_geom::{Affine, Rect, Shape};
use figcraft_render::{FillRule, Options, View, draw, render_view};
use vello_common::TextureId;
use vello_common::filter_effects::{EdgeMode, Filter, FilterFunction, FilterPrimitive};
use vello_common::geometry::RectU16;
use vello_common::paint::{Image, ImageSource};
use vello_common::peniko::{self, Fill};
use vello_gpu::{RenderSize, RenderTargetConfig, Scene, TargetInit, TextureBindings};

use crate::gpu_blur::GpuBlur;

pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 600;

fn color(c: Color, opacity: f32) -> peniko::Color {
    peniko::Color::new([c.r, c.g, c.b, c.a * opacity])
}

#[cfg(test)]
mod effect_capability_tests {
    use super::*;

    #[test]
    fn analytic_rectangle_shadow_is_gpu_supported_but_other_shapes_fallback() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Drop shadow capability");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Shadow", 20.0, 20.0, 80.0, 80.0)?;
        let node = doc.node_mut(id).ok_or("shadow node missing")?;
        node.fills.push(Paint::solid(Color::BLACK));
        node.effects.push(figcraft_doc::Effect::drop_shadow());
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 512, height: 512 };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        let node = doc.node_mut(id).ok_or("shadow node missing")?;
        if let Some(figcraft_doc::Effect { kind: EffectKind::DropShadow(shadow), .. }) = node.effects.first_mut() {
            shadow.spread = -2.0;
        }
        assert!(has_unhandled_effects_in_view(&doc, &view));
        if let Some(figcraft_doc::Effect { kind: EffectKind::DropShadow(shadow), .. }) =
            doc.node_mut(id).ok_or("shadow node missing")?.effects.first_mut()
        {
            shadow.spread = 0.0;
            shadow.color.a = 0.7;
        }
        assert!(has_unhandled_effects_in_view(&doc, &view));
        if let Some(figcraft_doc::Effect { kind: EffectKind::DropShadow(shadow), .. }) =
            doc.node_mut(id).ok_or("shadow node missing")?.effects.first_mut()
        {
            shadow.color.a = 0.25;
        }
        doc.node_mut(id).ok_or("shadow node missing")?.fills = vec![Paint::solid(Color::new(0.0, 0.0, 0.0, 0.2))];
        assert!(has_unhandled_effects_in_view(&doc, &view));
        Ok(())
    }

    #[test]
    fn masks_and_unimplemented_paints_use_fidelity_fallback() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU content capability");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Masked group", 0.0, 0.0, 120.0, 120.0)?;
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 0.0, 60.0, 120.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let content = doc.create(group, NodeKind::Rectangle, "Content", 0.0, 0.0, 120.0, 120.0)?;
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 512, height: 512 };
        assert!(has_unhandled_effects_in_view(&doc, &view));

        doc.node_mut(mask).ok_or("mask missing")?.is_mask = false;
        doc.node_mut(content).ok_or("content missing")?.blend = BlendMode::Multiply;
        assert!(has_unhandled_effects_in_view(&doc, &view));

        doc.node_mut(content).ok_or("content missing")?.blend = BlendMode::Normal;
        doc.node_mut(content).ok_or("content missing")?.fills =
            vec![Paint::gradient(figcraft_color::Gradient::default_for(GradientKind::Diamond, Color::new(1.0, 0.0, 0.0, 1.0)))];
        assert!(has_unhandled_effects_in_view(&doc, &view));
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct ImageInfo {
    pub id: TextureId,
    pub width: u16,
    pub height: u16,
}

/// Rebuild when document geometry or effect properties change.
pub struct BackdropIndex {
    candidates: Vec<(NodeId, f64, Rect, bool)>,
}

impl BackdropIndex {
    pub fn new(doc: &Document, page: NodeId) -> Self {
        let mut paint_order = Vec::new();
        collect_paint_order(doc, page, &mut paint_order);
        let candidates = paint_order
            .into_iter()
            .filter_map(|id| {
                let node = doc.node(id)?;
                let radius = node.effects.iter().find_map(|effect| match &effect.kind {
                    EffectKind::BackgroundBlur(blur) if effect.visible && blur.radius > 0.0 && !blur.is_progressive() => Some(blur.radius),
                    _ => None,
                })?;
                Some((id, radius, figcraft_render::raster_bounds(doc, id)?, backdrop_requires_cpu(doc, id, page)))
            })
            .collect();
        Self { candidates }
    }

    pub fn visible_ids(&self, view: &View) -> Vec<NodeId> {
        let world = Rect::new(
            view.origin.0,
            view.origin.1,
            view.origin.0 + f64::from(view.width) / view.scale,
            view.origin.1 + f64::from(view.height) / view.scale,
        );
        self.candidates.iter().filter(|(_, _, bounds, _)| bounds.intersect(world).area() > 0.0).map(|(id, _, _, _)| *id).collect()
    }
}

fn backdrop_requires_cpu(doc: &Document, id: NodeId, page: NodeId) -> bool {
    let mut current = id;
    while current != page {
        let Some(node) = doc.node(current) else { return true };
        let xf = doc.world_transform(current).as_coeffs();
        if node.opacity < 0.999
            || node.children.iter().any(|child| doc.node(*child).is_some_and(|child| child.is_mask))
            || (node.clip && (xf[1].abs() > 1e-6 || xf[2].abs() > 1e-6 || node.corners.uniform().is_none_or(|radius| radius > 0.0)))
        {
            return true;
        }
        let Some(parent) = node.parent else { return true };
        current = parent;
    }
    false
}

fn isolated_group_for_backdrop(doc: &Document, id: NodeId, page: NodeId) -> Option<(NodeId, f32)> {
    let mut current = id;
    let mut group = None;
    while current != page {
        let node = doc.node(current)?;
        let transform = doc.world_transform(current).as_coeffs();
        if node.children.iter().any(|child| doc.node(*child).is_some_and(|child| child.is_mask))
            || (node.clip && (transform[1].abs() > 1e-6 || transform[2].abs() > 1e-6 || node.corners.uniform().is_none_or(|radius| radius > 0.0)))
        {
            return None;
        }
        if current != id && (!node.strokes.is_empty() || !node.effects.is_empty()) {
            return None;
        }
        if node.opacity < 0.999 {
            if current == id
                || !matches!(node.kind, NodeKind::Group)
                || !matches!(node.blend, BlendMode::Normal | BlendMode::PassThrough)
                || group.is_some()
            {
                return None;
            }
            group = Some((current, node.opacity));
        }
        current = node.parent?;
    }
    group
}

fn backdrop_clip_rect(doc: &Document, id: NodeId, page: NodeId, view: &View) -> Option<[u32; 4]> {
    let mut current = id;
    let mut bounds: Option<Rect> = None;
    while current != page {
        let node = doc.node(current)?;
        let parent = node.parent?;
        let ancestor = doc.node(parent)?;
        if ancestor.clip {
            let frame = figcraft_geom::transform_rect_bbox(doc.world_transform(parent), Rect::new(0.0, 0.0, ancestor.width, ancestor.height));
            bounds = Some(bounds.map_or(frame, |current| current.intersect(frame)));
        }
        current = parent;
    }
    let bounds = bounds?;
    let coordinate = |world: f64, origin: f64, limit: u32, upper: bool| -> u32 {
        let px = (world - origin) * view.scale;
        (if upper { px.ceil() } else { px.floor() }).clamp(0.0, f64::from(limit)) as u32
    };
    Some([
        coordinate(bounds.x0, view.origin.0, view.width, false),
        coordinate(bounds.y0, view.origin.1, view.height, false),
        coordinate(bounds.x1, view.origin.0, view.width, true),
        coordinate(bounds.y1, view.origin.1, view.height, true),
    ])
}

fn image_key(img: &ImagePaint) -> String {
    format!("{}:{:?}", img.image, img.filters)
}

fn collect_paint_order(doc: &Document, id: NodeId, output: &mut Vec<NodeId>) {
    let mut pending = vec![id];
    while let Some(id) = pending.pop() {
        let Some(node) = doc.node(id).filter(|node| node.visible) else { continue };
        output.push(id);
        pending.extend(node.paint_order().collect::<Vec<_>>().into_iter().rev());
    }
}

fn hide_below(original: &Document, output: &mut Document, id: NodeId, page: NodeId) {
    let mut child = id;
    while child != page {
        let Some(parent) = original.node(child).and_then(|node| node.parent) else { break };
        let earlier =
            original.node(parent).map(|node| node.paint_order().take_while(|candidate| *candidate != child).collect::<Vec<_>>()).unwrap_or_default();
        for sibling in earlier {
            if let Some(node) = output.node_mut(sibling) {
                node.visible = false;
            }
        }
        if let Some(node) = output.node_mut(parent) {
            node.fills.clear();
            node.strokes.clear();
            node.effects.clear();
        }
        child = parent;
    }
}

fn hide_after_subtree(original: &Document, output: &mut Document, id: NodeId, page: NodeId) {
    let mut child = id;
    while child != page {
        let Some(parent) = original.node(child).and_then(|node| node.parent) else { break };
        let later = original
            .node(parent)
            .map(|node| node.paint_order().skip_while(|candidate| *candidate != child).skip(1).collect::<Vec<_>>())
            .unwrap_or_default();
        for sibling in later {
            if let Some(node) = output.node_mut(sibling) {
                node.visible = false;
            }
        }
        if let Some(node) = output.node_mut(parent) {
            node.strokes.clear();
        }
        child = parent;
    }
}

fn set_paint(scene: &mut Scene, paint: &Paint, node: &Node, images: Option<&HashMap<String, ImageInfo>>) -> bool {
    if !paint.visible || paint.opacity <= 0.0 {
        return false;
    }
    match &paint.kind {
        PaintKind::Solid { color: c } => scene.set_paint(color(*c, paint.opacity)),
        PaintKind::Gradient(g) if g.kind != GradientKind::Diamond => {
            let start = (g.from.0 * node.width, g.from.1 * node.height);
            let end = (g.to.0 * node.width, g.to.1 * node.height);
            let width = (g.width.0 * node.width, g.width.1 * node.height);
            let stops: Vec<(f32, peniko::Color)> = g.sorted_stops().iter().map(|s| (s.position, color(s.color, paint.opacity))).collect();
            scene.set_paint_transform(Affine::new([end.0 - start.0, end.1 - start.1, width.0 - start.0, width.1 - start.1, start.0, start.1]));
            let gradient = if g.kind == GradientKind::Linear {
                peniko::Gradient::new_linear((0.0, 0.0), (1.0, 0.0))
            } else if g.kind == GradientKind::Radial {
                peniko::Gradient::new_radial((0.0, 0.0), 1.0)
            } else {
                peniko::Gradient::new_sweep((0.0, 0.0), 0.0, 360.0)
            };
            scene.set_paint(gradient.with_stops(stops.as_slice()));
        }
        PaintKind::Gradient(g) => {
            let Some(stop) = g.stops.first() else { return false };
            scene.set_paint(color(stop.color, paint.opacity));
        }
        PaintKind::Image(img) => {
            let Some(info) = images.and_then(|entries| entries.get(&image_key(img))) else { return false };
            let placement = figcraft_render::paint::image_transform(img, f64::from(info.width), f64::from(info.height), node.width, node.height);
            scene.set_paint_transform(Affine::new([
                f64::from(placement.sx),
                f64::from(placement.ky),
                f64::from(placement.kx),
                f64::from(placement.sy),
                f64::from(placement.tx),
                f64::from(placement.ty),
            ]));
            let extend = if img.scale_mode == ImageScaleMode::Tile { peniko::Extend::Repeat } else { peniko::Extend::Pad };
            scene.set_paint(Image {
                image: ImageSource::external_texture(info.id, RectU16::new(0, 0, info.width, info.height), true),
                sampler: peniko::ImageSampler::default().with_extend(extend).with_alpha(paint.opacity.clamp(0.0, 1.0)),
            });
        }
        _ => return false,
    }
    true
}

fn set_rule(scene: &mut Scene, rule: FillRule) {
    scene.set_fill_rule(if rule == FillRule::EvenOdd { Fill::EvenOdd } else { Fill::NonZero });
}

fn draw_content(
    scene: &mut Scene,
    doc: &Document,
    node: &Node,
    transform: Affine,
    images: Option<&HashMap<String, ImageInfo>>,
    backdrops: Option<&HashMap<NodeId, ImageInfo>>,
    size: (u16, u16),
) {
    if let Some(info) = backdrops.and_then(|all| all.get(&node.id))
        && let Some((mut path, rule)) = draw::node_outline(doc, node)
    {
        path.apply_affine(transform);
        scene.set_transform(Affine::IDENTITY);
        scene.set_paint_transform(Affine::scale_non_uniform(f64::from(size.0) / f64::from(info.width), f64::from(size.1) / f64::from(info.height)));
        set_rule(scene, rule);
        scene.set_paint(Image {
            image: ImageSource::external_texture(info.id, RectU16::new(0, 0, info.width, info.height), true),
            sampler: peniko::ImageSampler::default(),
        });
        scene.fill_path(&path);
    }
    if let Some((path, rule)) = draw::node_outline(doc, node) {
        scene.set_transform(transform);
        set_rule(scene, rule);
        for paint in &node.fills {
            scene.reset_paint_transform();
            if set_paint(scene, paint, node, images) {
                scene.fill_path(&path);
            }
        }
    }
    draw_inner_shadows(scene, doc, node, transform);
    if let Some((path, rule)) = draw::stroke_fill(doc, node) {
        scene.set_transform(transform);
        set_rule(scene, rule);
        for paint in &node.strokes {
            scene.reset_paint_transform();
            if set_paint(scene, paint, node, images) {
                scene.fill_path(&path);
            }
        }
    }
}

/// Vello's analytic inverted blurred rounded rectangle runs in the GPU paint pass.
/// The original outline clips the inversion, so only pixels inside the node receive shadow.
fn draw_inner_shadows(scene: &mut Scene, doc: &Document, node: &Node, transform: Affine) {
    if std::env::var_os("LIZARD_GPU_NO_EFFECTS").is_some()
        || std::env::var_os("LIZARD_GPU_NO_INNER_SHADOW").is_some()
        || !matches!(node.kind, NodeKind::Rectangle | NodeKind::Frame)
    {
        return;
    }
    let Some(radius) = node.corners.uniform() else { return };
    let Some((outline, rule)) = draw::node_outline(doc, node) else { return };
    for effect in &node.effects {
        let EffectKind::InnerShadow(shadow) = &effect.kind else { continue };
        if !effect.visible || !shadow.radius.is_finite() || !shadow.spread.is_finite() || !shadow.offset.0.is_finite() || !shadow.offset.1.is_finite()
        {
            continue;
        }
        let inset = shadow.spread;
        let inner =
            Rect::new(inset + shadow.offset.0, inset + shadow.offset.1, node.width - inset + shadow.offset.0, node.height - inset + shadow.offset.1);
        if inner.width() <= 0.0 || inner.height() <= 0.0 {
            continue;
        }
        scene.set_transform(transform);
        scene.reset_paint_transform();
        set_rule(scene, rule);
        scene.push_clip_path(&outline);
        scene.set_paint(color(shadow.color, 1.0));
        scene.fill_blurred_rounded_rect(&inner, (radius - inset).max(0.0) as f32, (shadow.radius / 2.0).max(0.0) as f32, true);
        scene.pop_clip();
    }
}

pub fn gpu_drop_shadow_supported(doc: &Document, node: &Node, shadow: &Shadow) -> bool {
    let opaque_fill = node
        .fills
        .iter()
        .any(|paint| paint.visible && paint.opacity >= 0.999 && matches!(paint.kind, PaintKind::Solid { color } if color.a >= 0.999));
    let frame_is_opaque = matches!(node.kind, NodeKind::Frame) && opaque_fill;
    let children_inside = if frame_is_opaque && !node.clip {
        let radius = node.corners.uniform().unwrap_or(0.0).max(0.0);
        let safe = Rect::new(radius, radius, node.width - radius, node.height - radius);
        let inv = doc.world_transform(node.id).inverse();
        node.children.iter().all(|child| {
            doc.world_render_bounds(*child)
                .map(|bounds| figcraft_geom::transform_rect_bbox(inv, bounds))
                .is_some_and(|bounds| bounds.x0 >= safe.x0 && bounds.y0 >= safe.y0 && bounds.x1 <= safe.x1 && bounds.y1 <= safe.y1)
        })
    } else {
        true
    };
    (matches!(node.kind, NodeKind::Rectangle) && node.children.is_empty() && (opaque_fill || shadow.spread > 0.0)
        || frame_is_opaque && (node.clip || children_inside))
        && node.width > 0.0
        && node.height > 0.0
        && node.strokes.is_empty()
        && node.corners.uniform().is_some()
        && node.corners.smoothing.abs() < 1e-6
        && node.fills.iter().any(|paint| paint.visible && paint.opacity > 0.0)
        && node.fills.iter().all(|paint| matches!(paint.kind, PaintKind::Solid { .. }) && paint.blend == BlendMode::Normal)
        && shadow.blend == BlendMode::Normal
        && shadow.color.a <= 0.25
        && [shadow.radius, shadow.spread, shadow.offset.0, shadow.offset.1].into_iter().all(f64::is_finite)
        && shadow.radius >= 0.0
        && shadow.spread >= 0.0
}

fn draw_shadow(scene: &mut Scene, doc: &Document, node: &Node, transform: Affine) {
    let Some((path, rule)) = draw::node_outline(doc, node) else { return };
    for effect in &node.effects {
        let EffectKind::DropShadow(shadow) = &effect.kind else { continue };
        if !effect.visible {
            continue;
        }
        if gpu_drop_shadow_supported(doc, node, shadow)
            && let Some(radius) = node.corners.uniform()
        {
            let spread = shadow.spread;
            let rect = Rect::new(
                shadow.offset.0 - spread,
                shadow.offset.1 - spread,
                node.width + shadow.offset.0 + spread,
                node.height + shadow.offset.1 + spread,
            );
            scene.set_transform(transform);
            scene.reset_paint_transform();
            scene.set_paint(color(shadow.color, 1.0));
            if !shadow.show_behind {
                let pad = spread + shadow.radius * 2.0 + shadow.offset.0.abs().max(shadow.offset.1.abs()) + 10.0;
                let outer = Rect::new(-pad, -pad, node.width + pad, node.height + pad);
                let mut clip = outer.to_path(0.1);
                clip.extend(path.elements().iter().copied());
                scene.set_fill_rule(Fill::EvenOdd);
                scene.push_clip_path(&clip);
            }
            scene.fill_blurred_rounded_rect(&rect, (radius + spread) as f32, (shadow.radius / 2.0) as f32, false);
            if !shadow.show_behind {
                scene.pop_clip();
            }
            continue;
        }
        scene.set_transform(transform * Affine::translate(shadow.offset));
        scene.reset_paint_transform();
        set_rule(scene, rule);
        scene.set_paint(peniko::Color::new([1.0, 1.0, 1.0, 1.0]));
        scene.push_filter_layer(Filter::from_primitive(FilterPrimitive::DropShadowOnly {
            dx: 0.0,
            dy: 0.0,
            std_deviation: (shadow.radius / 2.0) as f32,
            color: color(shadow.color, 1.0),
            edge_mode: EdgeMode::None,
        }));
        scene.fill_path(&path);
        if shadow.spread > 0.0 {
            scene.set_stroke(peniko::kurbo::Stroke::new(shadow.spread * 2.0));
            scene.stroke_path(&path);
        }
        scene.pop_layer();
    }
}

fn draw_node(
    scene: &mut Scene,
    doc: &Document,
    id: NodeId,
    camera: Affine,
    view: Rect,
    depth: usize,
    images: Option<&HashMap<String, ImageInfo>>,
    backdrops: Option<&HashMap<NodeId, ImageInfo>>,
    size: (u16, u16),
) {
    if depth > 64 || !figcraft_render::raster_bounds(doc, id).is_some_and(|bounds| bounds.intersect(view).area() > 0.0) {
        return;
    }
    let Some(node) = doc.node(id) else { return };
    if !node.visible || node.is_mask {
        return;
    }
    let transform = camera * doc.world_transform(id);
    let opacity_layer = node.opacity < 0.999;
    if opacity_layer {
        scene.push_opacity_layer(node.opacity);
    }
    let effects = std::env::var_os("LIZARD_GPU_NO_EFFECTS").is_none();
    if effects {
        draw_shadow(scene, doc, node, transform);
    }
    let blur = node.effects.iter().find_map(|effect| match &effect.kind {
        EffectKind::LayerBlur(blur) if effects && effect.visible && blur.radius > 0.0 => Some(blur.radius),
        _ => None,
    });
    if let Some(radius) = blur {
        scene.set_transform(transform);
        scene.push_filter_layer(Filter::from_function(FilterFunction::Blur { radius: (radius / 2.0) as f32 }));
    }
    draw_content(scene, doc, node, transform, images, backdrops, size);
    let clipped = if node.clip && !node.children.is_empty() {
        if let Some((path, rule)) = draw::node_outline(doc, node) {
            scene.set_transform(transform);
            set_rule(scene, rule);
            scene.push_clip_path(&path);
            true
        } else {
            false
        }
    } else {
        false
    };
    for child in &node.children {
        draw_node(scene, doc, *child, camera, view, depth + 1, images, backdrops, size);
    }
    if clipped {
        scene.pop_clip();
    }
    if blur.is_some() {
        scene.pop_layer();
    }
    if opacity_layer {
        scene.pop_layer();
    }
}

pub fn make_scene(doc: &Document, page: NodeId, view: &View) -> Scene {
    make_scene_with_images(doc, page, view, None)
}

pub fn has_effects_in_view(doc: &Document, view: &View) -> bool {
    has_matching_effects_in_view(doc, view, false, false)
}

pub fn has_unsupported_effects_in_view(doc: &Document, view: &View) -> bool {
    has_matching_effects_in_view(doc, view, true, false)
}

pub fn has_unhandled_effects_in_view(doc: &Document, view: &View) -> bool {
    if has_matching_effects_in_view(doc, view, true, true) {
        return true;
    }
    let rect = view.page_rect();
    doc.nodes().any(|node| {
        if !node.visible {
            return false;
        }
        let unsupported = node.is_mask
            || !matches!(node.blend, BlendMode::Normal | BlendMode::PassThrough)
            || node.fills.iter().chain(&node.strokes).any(|paint| {
                paint.visible
                    && paint.opacity > 0.0
                    && (paint.blend != BlendMode::Normal
                        || match &paint.kind {
                            PaintKind::Gradient(gradient) => gradient.kind == GradientKind::Diamond,
                            PaintKind::Image(image) => !image.filters.is_identity(),
                            PaintKind::Pattern(_) | PaintKind::Shader(_) => true,
                            PaintKind::Solid { .. } => false,
                        })
            });
        unsupported
            && (if node.is_mask {
                node.parent.and_then(|parent| doc.world_render_bounds(parent))
            } else {
                figcraft_render::raster_bounds(doc, node.id)
            })
            .is_none_or(|bounds| bounds.intersect(rect).area() > 0.0)
    })
}

fn has_matching_effects_in_view(doc: &Document, view: &View, unsupported_only: bool, gpu_backdrop: bool) -> bool {
    let rect = Rect::new(
        view.origin.0,
        view.origin.1,
        view.origin.0 + f64::from(view.width) / view.scale,
        view.origin.1 + f64::from(view.height) / view.scale,
    );
    doc.nodes().any(|node| {
        node.visible
            && node.effects.iter().any(|effect| {
                effect.visible
                    && (!unsupported_only
                        || match &effect.kind {
                            // Only the measured low-opacity analytic rectangle/frame shadow
                            // uses the GPU. Other shadows retain the faithful CPU route.
                            EffectKind::DropShadow(shadow) => !gpu_drop_shadow_supported(doc, node, shadow),
                            EffectKind::InnerShadow(_) | EffectKind::Noise(_) | EffectKind::Texture(_) | EffectKind::Glass(_) => true,
                            EffectKind::BackgroundBlur(blur) => !gpu_backdrop || blur.is_progressive(),
                            EffectKind::LayerBlur(blur) => blur.is_progressive(),
                        })
            })
            && figcraft_render::raster_bounds(doc, node.id).is_some_and(|bounds| bounds.intersect(rect).area() > 0.0)
    })
}

pub fn make_scene_with_images(doc: &Document, page: NodeId, view: &View, images: Option<&HashMap<String, ImageInfo>>) -> Scene {
    make_scene_with_backdrops(doc, page, view, images, None)
}

pub fn make_scene_with_backdrops(
    doc: &Document,
    page: NodeId,
    view: &View,
    images: Option<&HashMap<String, ImageInfo>>,
    backdrops: Option<&HashMap<NodeId, ImageInfo>>,
) -> Scene {
    let mut scene = Scene::new(view.width as u16, view.height as u16);
    let camera = Affine::scale(view.scale) * Affine::translate((-view.origin.0, -view.origin.1));
    let world = Rect::new(
        view.origin.0,
        view.origin.1,
        view.origin.0 + f64::from(view.width) / view.scale,
        view.origin.1 + f64::from(view.height) / view.scale,
    );
    if let Some(page_node) = doc.node(page) {
        for child in &page_node.children {
            draw_node(&mut scene, doc, *child, camera, world, 0, images, backdrops, (view.width as u16, view.height as u16));
        }
    }
    scene
}

pub struct GpuCanvas {
    adapter_name: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: vello_gpu::Renderer,
    resources: vello_gpu::Resources,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    image_textures: Vec<wgpu::Texture>,
    temporary_textures: Vec<wgpu::Texture>,
    temporary_ids: Vec<TextureId>,
    image_bindings: TextureBindings,
    image_info: HashMap<String, ImageInfo>,
    next_external_id: u64,
    gpu_blur: Option<GpuBlur>,
}

impl GpuCanvas {
    pub fn device(&self) -> wgpu::Device {
        self.device.clone()
    }

    pub fn queue(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn new(width: u32, height: u32) -> Result<Self, Box<dyn Error>> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        Ok(Self::with_device(device, queue, format!("{} ({:?})", info.name, info.backend), width, height))
    }

    pub fn shared_with(other: &Self, width: u32, height: u32) -> Self {
        Self::with_device(other.device.clone(), other.queue.clone(), other.adapter_name.clone(), width, height)
    }

    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue, adapter_name: String, width: u32, height: u32) -> Self {
        Self::with_device(device, queue, adapter_name, width, height)
    }

    fn with_device(device: wgpu::Device, queue: wgpu::Queue, adapter_name: String, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard GPU benchmark target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let config = RenderTargetConfig { format: texture.format(), width: width as u16, height: height as u16 };
        let (renderer, resources) = vello_gpu::Renderer::new(&device, &config);
        let depth = vello_gpu::Renderer::create_depth_texture_view(&device, &RenderSize { width: width as u16, height: height as u16 });
        Self {
            adapter_name,
            device,
            queue,
            renderer,
            resources,
            texture,
            view,
            depth,
            image_textures: Vec::new(),
            temporary_textures: Vec::new(),
            temporary_ids: Vec::new(),
            image_bindings: TextureBindings::new(),
            image_info: HashMap::new(),
            next_external_id: 1_000_000,
            gpu_blur: None,
        }
    }

    pub fn image_info(&self) -> &HashMap<String, ImageInfo> {
        &self.image_info
    }

    pub fn register_external_texture(&mut self, texture: wgpu::Texture) -> TextureId {
        let id = TextureId(self.next_external_id);
        self.next_external_id += 1;
        self.image_bindings.insert(id, texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.temporary_textures.push(texture);
        self.temporary_ids.push(id);
        id
    }

    fn release_temporary_textures(&mut self) {
        for id in self.temporary_ids.drain(..) {
            self.image_bindings.remove(id);
        }
        self.temporary_textures.clear();
    }

    /// Experimental GPU backdrop blur. Each affected node reads a GPU-rendered prefix of the
    /// display list, then Vello blurs that texture before the final scene samples it in the
    /// node's outline. This deliberately remains opt-in until pixel comparisons pass.
    pub fn render_with_backdrop_blurs(&mut self, doc: &Document, page: NodeId, view: &View, clear: [f32; 3]) -> Result<(f64, usize), Box<dyn Error>> {
        let index = BackdropIndex::new(doc, page);
        self.render_with_backdrop_blurs_indexed(doc, page, view, clear, &index)
    }

    pub fn render_with_backdrop_blurs_indexed(
        &mut self,
        doc: &Document,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        index: &BackdropIndex,
    ) -> Result<(f64, usize), Box<dyn Error>> {
        let started = Instant::now();
        let world = Rect::new(
            view.origin.0,
            view.origin.1,
            view.origin.0 + f64::from(view.width) / view.scale,
            view.origin.1 + f64::from(view.height) / view.scale,
        );
        let roots = doc.node(page).map(|node| node.children.as_slice()).ok_or("page missing")?;
        let matching: Vec<_> = index.candidates.iter().filter(|(_, _, bounds, _)| bounds.intersect(world).area() > 0.0).collect();
        let candidates: Vec<_> = matching.iter().map(|(id, radius, _, _)| (*id, *radius)).collect();
        if candidates.is_empty() {
            let scene = make_scene_with_images(doc, page, view, Some(&self.image_info));
            self.render_with_clear(&scene, clear)?;
            self.release_temporary_textures();
            return Ok((started.elapsed().as_secs_f64() * 1000.0, 0));
        }
        if std::env::var_os("LIZARD_GPU_GROUP_ISOLATION").is_some()
            && candidates.len() == 1
            && let Some((group, opacity)) = isolated_group_for_backdrop(doc, candidates[0].0, page)
        {
            let result = self.render_isolated_group_backdrop(doc, page, view, clear, candidates[0], group, opacity);
            self.release_temporary_textures();
            return result;
        }
        if matching.iter().any(|(_, _, _, requires_cpu)| *requires_cpu) {
            let tile = render_view(doc, page, view, &Options { lod: true, ..Default::default() }).ok_or("CPU effect fallback failed")?;
            self.upload_premul(tile.data())?;
            self.release_temporary_textures();
            return Ok((started.elapsed().as_secs_f64() * 1000.0, candidates.len()));
        }
        if std::env::var_os("LIZARD_GPU_SEGMENT").is_some() && !candidates.is_empty() {
            let result = self.render_segmented_backdrops(doc, page, view, clear, &candidates);
            self.release_temporary_textures();
            return result;
        }
        let mut backdrops = HashMap::new();
        let mut prefix_ms = 0.0;
        let mut blur_ms = 0.0;
        for (id, radius) in candidates {
            let stage = Instant::now();
            let mut beneath = doc.clone();
            figcraft_render::backdrop::hide_above(&mut beneath, id, roots);
            let prefix = make_scene_with_images(&beneath, page, view, Some(&self.image_info));
            self.render_submit_with_clear(&prefix, clear)?;
            let prefix_texture = self.snapshot_submit();
            prefix_ms += stage.elapsed().as_secs_f64() * 1000.0;

            let stage = Instant::now();
            let method = std::env::var("LIZARD_GPU_BLUR_METHOD").unwrap_or_default();
            let blurred = if method == "copy" {
                prefix_texture
            } else if method == "compute" {
                let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
                let scale = std::env::var("LIZARD_GPU_BLUR_SCALE").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1).clamp(1, 4);
                blur.submit(&self.device, &self.queue, &prefix_texture, (radius * view.scale / 2.0) as f32, scale, None)
            } else {
                let prefix_id = self.register_external_texture(prefix_texture);
                let mut blur_scene = Scene::new(view.width as u16, view.height as u16);
                blur_scene.set_transform(Affine::IDENTITY);
                blur_scene.reset_paint_transform();
                blur_scene.push_filter_layer(Filter::from_function(FilterFunction::Blur { radius: (radius * view.scale / 2.0) as f32 }));
                blur_scene.set_paint(Image {
                    image: ImageSource::external_texture(prefix_id, RectU16::new(0, 0, view.width as u16, view.height as u16), true),
                    sampler: peniko::ImageSampler::default(),
                });
                blur_scene.fill_rect(&Rect::new(0.0, 0.0, f64::from(view.width), f64::from(view.height)));
                blur_scene.pop_layer();
                self.render_submit_with_clear(&blur_scene, clear)?;
                self.snapshot_submit()
            };
            let (Ok(width), Ok(height)) = (u16::try_from(blurred.width()), u16::try_from(blurred.height())) else { continue };
            let texture_id = self.register_external_texture(blurred);
            backdrops.insert(id, ImageInfo { id: texture_id, width, height });
            blur_ms += stage.elapsed().as_secs_f64() * 1000.0;
        }
        let stage = Instant::now();
        let scene = make_scene_with_backdrops(doc, page, view, Some(&self.image_info), Some(&backdrops));
        self.render_with_clear(&scene, clear)?;
        self.release_temporary_textures();
        let final_ms = stage.elapsed().as_secs_f64() * 1000.0;
        println!("backdrop_gpu_stages count={} prefix_ms={prefix_ms:.2} blur_ms={blur_ms:.2} final_ms={final_ms:.2}", backdrops.len());
        Ok((started.elapsed().as_secs_f64() * 1000.0, backdrops.len()))
    }

    fn render_segmented_backdrops(
        &mut self,
        doc: &Document,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        candidates: &[(NodeId, f64)],
    ) -> Result<(f64, usize), Box<dyn Error>> {
        let started = Instant::now();
        let roots = doc.node(page).map(|node| node.children.as_slice()).ok_or("page missing")?;
        let scale = std::env::var("LIZARD_GPU_BLUR_SCALE").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(4).clamp(1, 4);
        let mut backdrops = HashMap::new();
        let mut previous = None;
        let mut prefix_prepare_ms = 0.0;
        let mut prefix_submit_ms = 0.0;
        let mut blur_submit_ms = 0.0;
        for &(id, radius) in candidates {
            let stage = Instant::now();
            let mut segment = doc.clone();
            if let Some(previous_id) = previous {
                hide_below(doc, &mut segment, previous_id, page);
            }
            figcraft_render::backdrop::hide_above(&mut segment, id, roots);
            let scene = make_scene_with_backdrops(&segment, page, view, Some(&self.image_info), Some(&backdrops));
            prefix_prepare_ms += stage.elapsed().as_secs_f64() * 1000.0;
            let stage = Instant::now();
            if previous.is_some() {
                self.render_submit_with_target(&scene, TargetInit::SrcOver)?;
            } else {
                self.render_submit_with_clear(&scene, clear)?;
            }
            let source = self.snapshot_submit();
            prefix_submit_ms += stage.elapsed().as_secs_f64() * 1000.0;
            let stage = Instant::now();
            let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
            let clip = backdrop_clip_rect(doc, id, page, view);
            let blurred = blur.submit(&self.device, &self.queue, &source, (radius * view.scale / 2.0) as f32, scale, clip);
            let (width, height) = (u16::try_from(blurred.width())?, u16::try_from(blurred.height())?);
            let texture_id = self.register_external_texture(blurred);
            blur_submit_ms += stage.elapsed().as_secs_f64() * 1000.0;
            backdrops.insert(id, ImageInfo { id: texture_id, width, height });
            previous = Some(id);
        }
        let stage = Instant::now();
        let mut suffix = doc.clone();
        if let Some(last_id) = previous {
            hide_below(doc, &mut suffix, last_id, page);
        }
        let rest = make_scene_with_backdrops(&suffix, page, view, Some(&self.image_info), Some(&backdrops));
        let suffix_prepare_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let stage = Instant::now();
        self.render_with_load(&rest)?;
        let suffix_render_ms = stage.elapsed().as_secs_f64() * 1000.0;
        if std::env::var_os("LIZARD_PROFILE_EFFECT_STAGES").is_some() {
            println!(
                "effect_stage prefix_prepare_ms={prefix_prepare_ms:.2} prefix_submit_ms={prefix_submit_ms:.2} blur_submit_ms={blur_submit_ms:.2} suffix_prepare_ms={suffix_prepare_ms:.2} suffix_render_ms={suffix_render_ms:.2} total_ms={:.2}",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        Ok((started.elapsed().as_secs_f64() * 1000.0, candidates.len()))
    }

    fn render_isolated_group_backdrop(
        &mut self,
        doc: &Document,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        candidate: (NodeId, f64),
        group: NodeId,
        opacity: f32,
    ) -> Result<(f64, usize), Box<dyn Error>> {
        let started = Instant::now();
        let (id, radius) = candidate;
        let roots = doc.node(page).map(|node| node.children.as_slice()).ok_or("page missing")?;

        let mut base_doc = doc.clone();
        figcraft_render::backdrop::hide_above(&mut base_doc, group, roots);
        let base_scene = make_scene_with_images(&base_doc, page, view, Some(&self.image_info));
        self.render_submit_with_clear(&base_scene, clear)?;
        let base = self.snapshot_submit();

        let mut prefix_doc = doc.clone();
        figcraft_render::backdrop::hide_above(&mut prefix_doc, id, roots);
        let prefix_scene = make_scene_with_images(&prefix_doc, page, view, Some(&self.image_info));
        self.render_submit_with_clear(&prefix_scene, clear)?;
        let source = self.snapshot_submit();
        let scale = std::env::var("LIZARD_GPU_BLUR_SCALE").ok().and_then(|value| value.parse::<u32>().ok()).unwrap_or(4).clamp(1, 4);
        let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
        let clip = backdrop_clip_rect(doc, id, page, view);
        let blurred = blur.submit(&self.device, &self.queue, &source, (radius * view.scale / 2.0) as f32, scale, clip);
        let (width, height) = (u16::try_from(blurred.width())?, u16::try_from(blurred.height())?);
        let blur_id = self.register_external_texture(blurred);
        let backdrops = HashMap::from([(id, ImageInfo { id: blur_id, width, height })]);

        let mut full_prefix_doc = doc.clone();
        if let Some(node) = full_prefix_doc.node_mut(group) {
            node.opacity = 1.0;
        }
        figcraft_render::backdrop::hide_above(&mut full_prefix_doc, id, roots);
        let full_prefix_scene = make_scene_with_images(&full_prefix_doc, page, view, Some(&self.image_info));
        self.render_submit_with_clear(&full_prefix_scene, clear)?;

        let mut group_suffix = doc.clone();
        hide_below(doc, &mut group_suffix, id, page);
        hide_after_subtree(doc, &mut group_suffix, group, page);
        if let Some(node) = group_suffix.node_mut(group) {
            node.opacity = 1.0;
        }
        let suffix_scene = make_scene_with_backdrops(&group_suffix, page, view, Some(&self.image_info), Some(&backdrops));
        self.render_submit_with_target(&suffix_scene, TargetInit::SrcOver)?;
        let group_surface = self.snapshot_submit();

        let base_id = self.register_external_texture(base);
        let group_id = self.register_external_texture(group_surface);
        let mut blend = Scene::new(view.width as u16, view.height as u16);
        blend.set_transform(Affine::IDENTITY);
        for (texture, alpha) in [(base_id, 1.0), (group_id, opacity)] {
            blend.reset_paint_transform();
            blend.set_paint(Image {
                image: ImageSource::external_texture(texture, RectU16::new(0, 0, view.width as u16, view.height as u16), true),
                sampler: peniko::ImageSampler::default().with_alpha(alpha),
            });
            blend.fill_rect(&Rect::new(0.0, 0.0, f64::from(view.width), f64::from(view.height)));
        }
        self.render_submit_with_clear(&blend, clear)?;

        let mut after_doc = doc.clone();
        hide_below(doc, &mut after_doc, group, page);
        if let Some(node) = after_doc.node_mut(group) {
            node.visible = false;
        }
        let after_scene = make_scene_with_images(&after_doc, page, view, Some(&self.image_info));
        self.render_with_load(&after_scene)?;
        Ok((started.elapsed().as_secs_f64() * 1000.0, 1))
    }

    pub fn register_images(&mut self, doc: &Document) -> Result<usize, Box<dyn Error>> {
        let mut count = 0;
        for node in doc.nodes() {
            for paint in node.fills.iter().chain(&node.strokes) {
                let PaintKind::Image(img) = &paint.kind else { continue };
                let key = image_key(img);
                if self.image_info.contains_key(&key) {
                    continue;
                }
                let Some(pm) = figcraft_render::images::get(&img.image, doc.images.get(&img.image), &img.filters, doc.color_profile) else {
                    continue;
                };
                let (Ok(width), Ok(height)) = (u16::try_from(pm.width()), u16::try_from(pm.height())) else { continue };
                let id = TextureId(self.image_info.len() as u64 + 1);
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Lizard benchmark image paint"),
                    size: wgpu::Extent3d { width: pm.width(), height: pm.height(), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    pm.data(),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(pm.width() * 4), rows_per_image: Some(pm.height()) },
                    texture.size(),
                );
                self.image_bindings.insert(id, texture.create_view(&wgpu::TextureViewDescriptor::default()));
                self.image_info.insert(key, ImageInfo { id, width, height });
                self.image_textures.push(texture);
                count += 1;
            }
        }
        Ok(count)
    }

    /// Upload a faithful CPU-rendered tile for effects the experimental scene path cannot match.
    pub fn upload_premul(&self, pixels: &[u8]) -> Result<(), Box<dyn Error>> {
        let size = self.texture.size();
        if pixels.len() != size.width as usize * size.height as usize * 4 {
            return Err("tile pixel buffer has the wrong size".into());
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width * 4), rows_per_image: Some(size.height) },
            size,
        );
        self.wait()?;
        Ok(())
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn device_queue(&self) -> (wgpu::Device, wgpu::Queue) {
        (self.device.clone(), self.queue.clone())
    }

    /// Keep a rendered tile in GPU memory for later composition.
    pub fn snapshot(&self) -> Result<wgpu::Texture, Box<dyn Error>> {
        let snapshot = self.snapshot_submit();
        self.wait()?;
        Ok(snapshot)
    }

    /// Queue a tile copy without forcing a CPU/GPU fence after each job.
    pub fn snapshot_submit(&self) -> wgpu::Texture {
        let size = self.texture.size();
        let snapshot = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard GPU benchmark cached tile"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.texture.format(),
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU benchmark tile copy") });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: &snapshot, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            size,
        );
        self.queue.submit([encoder.finish()]);
        snapshot
    }

    pub fn render(&mut self, scene: &Scene) -> Result<f64, Box<dyn Error>> {
        let ms = self.render_submit(scene)?;
        self.wait()?;
        Ok(ms)
    }

    pub fn render_with_clear(&mut self, scene: &Scene, clear: [f32; 3]) -> Result<f64, Box<dyn Error>> {
        let ms = self.render_submit_with_clear(scene, clear)?;
        self.wait()?;
        Ok(ms)
    }

    pub fn render_with_load(&mut self, scene: &Scene) -> Result<f64, Box<dyn Error>> {
        let ms = self.render_submit_with_target(scene, TargetInit::SrcOver)?;
        self.wait()?;
        Ok(ms)
    }

    /// Measure encoding and submission; callers must fence at a presentation boundary.
    pub fn render_submit(&mut self, scene: &Scene) -> Result<f64, Box<dyn Error>> {
        self.render_submit_with_clear(scene, [30.0 / 255.0; 3])
    }

    pub fn render_submit_with_clear(&mut self, scene: &Scene, clear: [f32; 3]) -> Result<f64, Box<dyn Error>> {
        self.render_submit_with_target(
            scene,
            TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([clear[0], clear[1], clear[2], 1.0]) }),
        )
    }

    fn render_submit_with_target(&mut self, scene: &Scene, init: TargetInit) -> Result<f64, Box<dyn Error>> {
        let started = Instant::now();
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU benchmark render") });
        let size = self.texture.size();
        self.renderer.render(
            scene,
            &mut self.resources,
            &self.device,
            &self.queue,
            &mut encoder,
            &RenderSize { width: size.width as u16, height: size.height as u16 },
            &self.view,
            Some(&self.depth),
            &self.image_bindings,
            init,
        )?;
        self.queue.submit([encoder.finish()]);
        Ok(started.elapsed().as_secs_f64() * 1000.0)
    }

    pub fn wait(&self) -> Result<(), Box<dyn Error>> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }

    pub fn pixels(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let size = self.texture.size();
        let stride = size.width.saturating_mul(4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard GPU benchmark readback"),
            size: u64::from(stride) * u64::from(size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU benchmark readback encoder") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &output,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(size.height) },
            },
            size,
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        output.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let mapped = output.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity(size.width as usize * size.height as usize * 4);
        for row in mapped.chunks_exact(stride as usize) {
            pixels.extend_from_slice(&row[..size.width as usize * 4]);
        }
        drop(mapped);
        output.unmap();
        Ok(pixels)
    }
}
