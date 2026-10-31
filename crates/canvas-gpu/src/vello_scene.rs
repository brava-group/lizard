//! GPU scene renderer with explicit capability checks for unsupported content.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use figcraft_color::{
    BlendMode, Color, ColorProfile, Gradient, GradientKind, ImagePaint, ImageScaleMode, Paint, PaintKind, PatternPaint, ShaderFill,
};
use figcraft_doc::{Blur, Document, EffectKind, Glass, MaskType, Node, NodeId, NodeKind, Noise, Shadow, Texture};
use figcraft_geom::{Affine, BezPath, Point, Rect, Shape};
use figcraft_render::{FillRule, Options, View, draw, render_view};
use vello_common::TextureId;
use vello_common::filter_effects::{EdgeMode, Filter, FilterFunction, FilterPrimitive};
use vello_common::geometry::RectU16;
use vello_common::paint::{Image, ImageSource};
use vello_common::peniko::{self, Fill};
use vello_gpu::{RenderSize, RenderTargetConfig, Scene, TargetInit, TextureBindings};

use crate::gpu_blur::GpuBlur;
use crate::gpu_color_profile::GpuColorProfile;
use crate::gpu_diamond::GpuDiamond;
use crate::gpu_glass::GpuGlass;
use crate::gpu_image_filter::GpuImageFilter;
use crate::gpu_morph::GpuMorph;
use crate::gpu_noise::GpuNoise;
use crate::gpu_progressive_blur::GpuProgressiveBlur;
use crate::gpu_shader::GpuShader;
use crate::gpu_shadow_clip::GpuShadowClip;
use crate::gpu_silhouette_shadow::GpuSilhouetteShadow;
use crate::gpu_texture::GpuTexture;

pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 600;

fn color(c: Color, opacity: f32) -> peniko::Color {
    peniko::Color::new([c.r, c.g, c.b, c.a * opacity])
}

#[cfg(test)]
mod shader_cache_tests {
    use super::*;

    #[test]
    fn shader_uniform_edits_replace_the_gpu_texture() -> Result<(), Box<dyn Error>> {
        let Ok(mut canvas) = GpuCanvas::new(64, 64) else { return Ok(()) };
        let mut doc = Document::new("shader cache");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Shader", 0.0, 0.0, 32.0, 32.0)?;
        let definition = figcraft_shader::demo();
        doc.shaders.insert(definition.id.clone(), definition);
        doc.node_mut(id).ok_or("shader node missing")?.fills = vec![Paint {
            kind: PaintKind::Shader(ShaderFill { shader_id: figcraft_shader::DEMO_ID.into(), values: Default::default() }),
            ..Paint::solid(Color::WHITE)
        }];
        assert_eq!(canvas.register_images_at_scale(&doc, 0.0, 1.0)?, 1);
        assert_eq!(canvas.register_images_at_scale(&doc, 0.0, 1.5)?, 1);
        assert_eq!(canvas.shader_textures.len(), 1);
        let node = doc.node_mut(id).ok_or("shader node missing after registration")?;
        if let Some(Paint { kind: PaintKind::Shader(fill), .. }) = node.fills.first_mut() {
            fill.values.insert("frequency".into(), 8.0);
        }
        assert_eq!(canvas.register_images_at_scale(&doc, 0.0, 1.5)?, 1);
        assert_eq!(canvas.shader_textures.len(), 1);
        assert_eq!(canvas.shader_info.len(), 1);
        Ok(())
    }
}

fn same_global_inputs(before: &Document, after: &Document) -> bool {
    !(before.pages != after.pages
        || before.styles != after.styles
        || before.shaders != after.shaders
        || before.variables != after.variables
        || before.color_profile != after.color_profile
        || before.images.len() != after.images.len()
        || before.images.iter().any(|(key, image)| {
            after
                .images
                .get(key)
                .is_none_or(|other| image.width != other.width || image.height != other.height || !std::sync::Arc::ptr_eq(&image.bytes, &other.bytes))
        }))
}

fn same_render_inputs(before: &Document, after: &Document) -> bool {
    before.node_count() == after.node_count() && same_global_inputs(before, after)
}

/// Match a newly inserted copy to the sibling it was duplicated from. The check includes every
/// render property and descendant, allowing only fresh ids and one world-space translation.
fn duplicated_subtrees(before: &Document, after: &Document) -> Option<Vec<(NodeId, NodeId, Affine)>> {
    if after.node_count() <= before.node_count() || !same_global_inputs(before, after) {
        return None;
    }
    let new_roots: Vec<_> = after
        .nodes()
        .filter(|node| before.node(node.id).is_none() && node.parent.is_some_and(|parent| before.node(parent).is_some()))
        .map(|node| node.id)
        .collect();
    if new_roots.is_empty() {
        return None;
    }
    let mut pairs = Vec::new();
    for copy_root in new_roots {
        let parent = after.node(copy_root)?.parent?;
        let siblings = &after.node(parent)?.children;
        let index = siblings.iter().position(|id| *id == copy_root)?;
        let source_root = siblings[..index].iter().rev().find(|id| before.node(**id).is_some()).copied()?;
        let source_world = before.world_transform(source_root).as_coeffs();
        let copy_world = after.world_transform(copy_root).as_coeffs();
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-7 * a.abs().max(b.abs()).max(1.0);
        if source_world[..4].iter().zip(&copy_world[..4]).any(|(a, b)| !near(*a, *b)) {
            return None;
        }
        let delta = Affine::translate((copy_world[4] - source_world[4], copy_world[5] - source_world[5]));
        if !delta.as_coeffs().iter().all(|value| value.is_finite()) {
            return None;
        }
        let mut stack = vec![(source_root, copy_root, parent, true)];
        while let Some((source_id, copy_id, expected_parent, root)) = stack.pop() {
            let source = before.node(source_id)?;
            let copy = after.node(copy_id)?;
            if source.children.len() != copy.children.len() || copy.parent != Some(expected_parent) {
                return None;
            }
            let source_world = before.world_transform(source_id).as_coeffs();
            let copy_world = after.world_transform(copy_id).as_coeffs();
            if source_world[..4].iter().zip(&copy_world[..4]).any(|(a, b)| !near(*a, *b))
                || !near(copy_world[4] - source_world[4], delta.as_coeffs()[4])
                || !near(copy_world[5] - source_world[5], delta.as_coeffs()[5])
            {
                return None;
            }
            let mut restored = copy.clone();
            restored.id = source_id;
            restored.parent = source.parent;
            restored.children = source.children.clone();
            if root {
                restored.transform = source.transform;
            }
            if &restored != source {
                return None;
            }
            pairs.push((source_id, copy_id, delta));
            stack.extend(source.children.iter().zip(&copy.children).map(|(source, child_copy)| (*source, *child_copy, copy_id, false)));
        }
    }
    if pairs.len() != after.node_count() - before.node_count() {
        return None;
    }
    for source in before.nodes() {
        let mut restored = after.node(source.id)?.clone();
        restored.children.retain(|id| before.node(*id).is_some());
        if &restored != source {
            return None;
        }
    }
    Some(pairs)
}

fn cached_effects_survive_geometry_edit(
    before: &Document,
    after: &Document,
    isolated: impl Iterator<Item = NodeId>,
    shape_shadows: impl Iterator<Item = NodeId>,
) -> bool {
    if after.node_count() < before.node_count() || !same_global_inputs(before, after) {
        return false;
    }
    for node in after.nodes() {
        if before.node(node.id).is_none() && (node.is_mask || node.effects.iter().any(|effect| effect.visible)) {
            return false;
        }
    }
    for old in before.nodes() {
        let Some(new) = after.node(old.id) else { return false };
        if old.effects != new.effects || old.is_mask != new.is_mask {
            return false;
        }
    }
    let stable = |id, children_independent| {
        let (Some(old), Some(new)) = (before.node(id), after.node(id)) else { return false };
        let mut restored = new.clone();
        if children_independent {
            restored.children = old.children.clone();
        }
        &restored == old
            && before.world_transform(id).as_coeffs() == after.world_transform(id).as_coeffs()
            && figcraft_render::raster_bounds(before, id) == figcraft_render::raster_bounds(after, id)
    };
    isolated.into_iter().all(|id| stable(id, false)) && shape_shadows.into_iter().all(|id| stable(id, true))
}

/// Find a gesture that edits only one node's transform. Its descendants inherit the same
/// world-space change without modifying their own geometry or effects.
fn transform_only_node(before: &Document, after: &Document) -> Option<(NodeId, Affine, Affine)> {
    if !same_render_inputs(before, after) {
        return None;
    }
    let mut changed = None;
    for old in before.nodes() {
        let new = after.node(old.id)?;
        if old == new {
            continue;
        }
        if changed.is_some() {
            return None;
        }
        let mut restored = new.clone();
        restored.transform = old.transform;
        if &restored != old {
            return None;
        }
        changed = Some((old.id, before.world_transform(old.id), after.world_transform(old.id)));
    }
    changed
}

/// A translation also covers a frame whose parent has a transform.
fn translated_node(before: &Document, after: &Document) -> Option<(NodeId, f64, f64)> {
    let (id, old, new) = transform_only_node(before, after)?;
    let old = old.as_coeffs();
    let new = new.as_coeffs();
    if old[..4] != new[..4] {
        return None;
    }
    let dx = new[4] - old[4];
    let dy = new[5] - old[5];
    (dx.is_finite() && dy.is_finite()).then_some((id, dx, dy))
}

/// Reuse an already rasterized effect while its entire subtree rotates in world space.
fn rotated_node(before: &Document, after: &Document) -> Option<(NodeId, Affine)> {
    let (id, old, new) = transform_only_node(before, after)?;
    let delta = new * old.inverse();
    let [a, b, c, d, x, y] = delta.as_coeffs();
    let near = |value: f64, expected: f64| (value - expected).abs() < 1e-7;
    ([a, b, c, d, x, y].iter().all(|value| value.is_finite())
        && near(a * a + b * b, 1.0)
        && near(c * c + d * d, 1.0)
        && near(a * c + b * d, 0.0)
        && near(a * d - b * c, 1.0))
    .then_some((id, delta))
}

fn scaled_kind_matches(old: &NodeKind, new: &NodeKind, k: f64) -> bool {
    match (old, new) {
        (NodeKind::Vector { network: old }, NodeKind::Vector { network: new }) => scaled_vector_matches(old, new, k, k),
        _ => old == new,
    }
}

fn scaled_vector_matches(old: &figcraft_doc::VectorNetwork, new: &figcraft_doc::VectorNetwork, kx: f64, ky: f64) -> bool {
    let mut expected = old.clone();
    expected.scale(kx, ky);
    if expected.vertices.len() != new.vertices.len() || expected.segments.len() != new.segments.len() {
        return false;
    }
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0);
    let mut aligned = new.clone();
    for (vertex, target) in aligned.vertices.iter_mut().zip(&expected.vertices) {
        if !near(vertex.x, target.x) || !near(vertex.y, target.y) {
            return false;
        }
        vertex.x = target.x;
        vertex.y = target.y;
    }
    for (segment, target) in aligned.segments.iter_mut().zip(&expected.segments) {
        if !near(segment.ta.0, target.ta.0)
            || !near(segment.ta.1, target.ta.1)
            || !near(segment.tb.0, target.tb.0)
            || !near(segment.tb.1, target.tb.1)
        {
            return false;
        }
        segment.ta = target.ta;
        segment.tb = target.tb;
    }
    expected == aligned
}

fn scaled_subtree(before: &Document, after: &Document) -> Option<(NodeId, Affine)> {
    if !same_render_inputs(before, after) {
        return None;
    }
    let changed: Vec<_> = before.nodes().filter_map(|old| (after.node(old.id)? != old).then_some(old.id)).collect();
    let changed_set: HashSet<_> = changed.iter().copied().collect();
    let roots: Vec<_> = changed
        .iter()
        .copied()
        .filter(|id| {
            let mut parent = before.node(*id).and_then(|node| node.parent);
            let mut depth = 0;
            while let Some(ancestor) = parent {
                depth += 1;
                if depth > 10_000 {
                    return false;
                }
                if changed_set.contains(&ancestor) {
                    return false;
                }
                parent = before.node(ancestor).and_then(|node| node.parent);
            }
            true
        })
        .collect();
    let [root] = roots.as_slice() else { return None };
    let old_root = before.node(*root)?;
    let new_root = after.node(*root)?;
    if old_root.width <= 0.0 || old_root.height <= 0.0 {
        return None;
    }
    let k = new_root.width / old_root.width;
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0);
    if !k.is_finite() || !(0.25..=4.0).contains(&k) || !near(new_root.height, old_root.height * k) {
        return None;
    }
    let delta = after.world_transform(*root) * Affine::scale(k) * before.world_transform(*root).inverse();
    let [a, b, c, d, x, y] = delta.as_coeffs();
    if ![a, b, c, d, x, y].iter().all(|value| value.is_finite()) || !near(a, k) || !near(b, 0.0) || !near(c, 0.0) || !near(d, k) {
        return None;
    }
    for old in before.nodes() {
        let new = after.node(old.id)?;
        if !before.is_ancestor(*root, old.id) {
            if old != new {
                return None;
            }
            continue;
        }
        if !near(new.width, old.width * k)
            || !near(new.height, old.height * k)
            || old.parent != new.parent
            || old.children != new.children
            || old.visible != new.visible
            || old.opacity != new.opacity
            || old.blend != new.blend
            || old.clip != new.clip
            || old.is_mask != new.is_mask
            || old.fills != new.fills
            || old.strokes != new.strokes
            || !scaled_kind_matches(&old.kind, &new.kind, k)
            || old.effects.len() != new.effects.len()
            || !near(new.stroke.weight, old.stroke.weight * k)
            || old.corners.smoothing != new.corners.smoothing
            || old.corners.radii.iter().zip(new.corners.radii).any(|(old, new)| !near(new, old * k))
        {
            return None;
        }
        let old_mapping = (delta * before.world_transform(old.id)).as_coeffs();
        let new_mapping = (after.world_transform(old.id) * Affine::scale(k)).as_coeffs();
        if old_mapping.iter().zip(new_mapping).any(|(old, new)| !near(*old, new)) {
            return None;
        }
        for (old, new) in old.effects.iter().zip(&new.effects) {
            if old.visible != new.visible {
                return None;
            }
            let scaled = match (&old.kind, &new.kind) {
                (EffectKind::DropShadow(a), EffectKind::DropShadow(b)) | (EffectKind::InnerShadow(a), EffectKind::InnerShadow(b)) => {
                    a.color == b.color
                        && a.blend == b.blend
                        && a.show_behind == b.show_behind
                        && near(b.radius, a.radius * k)
                        && near(b.spread, a.spread * k)
                        && near(b.offset.0, a.offset.0 * k)
                        && near(b.offset.1, a.offset.1 * k)
                }
                (EffectKind::LayerBlur(a), EffectKind::LayerBlur(b)) => {
                    !a.is_progressive()
                        && a.blur_type == b.blur_type
                        && a.start == b.start
                        && a.end == b.end
                        && near(b.radius, a.radius * k)
                        && near(b.start_radius, a.start_radius * k)
                }
                _ => false,
            };
            if !scaled {
                return None;
            }
        }
    }
    Some((*root, delta))
}

/// A Move-tool resize can change a frame or shape's box while leaving its effect parameters
/// unchanged. Child effects follow inherited translations; the resized layer's own effect is
/// stretched only for the duration of the gesture.
fn resized_shape(before: &Document, after: &Document) -> Option<(NodeId, Affine)> {
    if !same_render_inputs(before, after) {
        return None;
    }
    let mut root = None;
    for old in before.nodes() {
        let new = after.node(old.id)?;
        if matches!(
            old.kind,
            NodeKind::Frame
                | NodeKind::Rectangle
                | NodeKind::Ellipse { .. }
                | NodeKind::Polygon { .. }
                | NodeKind::Star { .. }
                | NodeKind::Vector { .. }
        ) && (old.width != new.width || old.height != new.height)
            && root.replace(old.id).is_some()
        {
            return None;
        }
    }
    let root = root?;
    let old_root = before.node(root)?;
    let new_root = after.node(root)?;
    let (kx, ky) = (new_root.width / old_root.width, new_root.height / old_root.height);
    if !kx.is_finite() || !ky.is_finite() || !(0.5..=2.0).contains(&kx) || !(0.5..=2.0).contains(&ky) {
        return None;
    }
    for old in before.nodes() {
        let new = after.node(old.id)?;
        if old.id == root {
            let kind_matches = match (&old.kind, &new.kind) {
                (NodeKind::Vector { network: old }, NodeKind::Vector { network: new }) => scaled_vector_matches(old, new, kx, ky),
                _ => old.kind == new.kind,
            };
            if !kind_matches {
                return None;
            }
            let mut restored = new.clone();
            restored.transform = old.transform;
            restored.width = old.width;
            restored.height = old.height;
            restored.kind = old.kind.clone();
            restored.child_layout.sizing_h = old.child_layout.sizing_h;
            restored.child_layout.sizing_v = old.child_layout.sizing_v;
            if &restored != old {
                return None;
            }
        } else if before.is_ancestor(root, old.id) {
            let mut restored = new.clone();
            restored.transform = old.transform;
            if &restored != old || translated_world_delta(before, after, old.id).is_none() {
                return None;
            }
        } else if old != new {
            return None;
        }
    }
    let delta = after.world_transform(root) * Affine::scale_non_uniform(kx, ky) * before.world_transform(root).inverse();
    delta.as_coeffs().iter().all(|value| value.is_finite()).then_some((root, delta))
}

fn translated_world_delta(before: &Document, after: &Document, id: NodeId) -> Option<Affine> {
    let old = before.world_transform(id).as_coeffs();
    let new = after.world_transform(id).as_coeffs();
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-7 * a.abs().max(b.abs()).max(1.0);
    if old[..4].iter().zip(&new[..4]).any(|(a, b)| !near(*a, *b)) {
        return None;
    }
    let (dx, dy) = (new[4] - old[4], new[5] - old[5]);
    (dx.is_finite() && dy.is_finite()).then_some(Affine::translate((dx, dy)))
}

#[cfg(test)]
mod translation_cache_tests {
    use super::*;

    #[test]
    fn drawing_inside_a_frame_preserves_its_shape_shadow_cache() -> Result<(), Box<dyn Error>> {
        let mut before = Document::new("shape creation cache");
        let page = before.current_page.ok_or("missing page")?;
        let frame = before.create(page, NodeKind::Frame, "Frame", 10.0, 20.0, 100.0, 100.0)?;
        let mut drawn = before.clone();
        let shape = drawn.create(frame, NodeKind::Rectangle, "Rectangle", 15.0, 15.0, 30.0, 20.0)?;
        assert!(cached_effects_survive_geometry_edit(&before, &drawn, std::iter::empty(), [frame].into_iter()));
        assert!(!cached_effects_survive_geometry_edit(&before, &drawn, [frame].into_iter(), std::iter::empty()));

        let mut resized = drawn.clone();
        resized.node_mut(shape).ok_or("missing shape")?.width = 35.0;
        assert!(cached_effects_survive_geometry_edit(&drawn, &resized, std::iter::empty(), [frame].into_iter()));

        let mut changed_frame = drawn.clone();
        changed_frame.node_mut(frame).ok_or("missing frame")?.width = 110.0;
        assert!(!cached_effects_survive_geometry_edit(&before, &changed_frame, std::iter::empty(), [frame].into_iter()));
        Ok(())
    }

    #[test]
    fn duplicate_cache_requires_an_unchanged_translated_subtree() -> Result<(), Box<dyn Error>> {
        let mut before = Document::new("duplicate cache");
        let page = before.current_page.ok_or("missing page")?;
        let frame = before.create(page, NodeKind::Frame, "Frame", 10.0, 20.0, 100.0, 100.0)?;
        before.create(frame, NodeKind::Rectangle, "Child", 2.0, 3.0, 20.0, 20.0)?;
        let mut duplicate = before.clone();
        let copy = duplicate.duplicate_into(frame, page, None)?;
        duplicate.set_world_transform(copy, Affine::translate((40.0, 10.0)) * before.world_transform(frame))?;
        let pairs = duplicated_subtrees(&before, &duplicate).ok_or("valid duplicate was not recognized")?;
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, frame);
        assert_eq!(pairs[0].1, copy);

        let mut changed_copy = duplicate.clone();
        changed_copy.node_mut(copy).ok_or("missing copy")?.width += 1.0;
        assert!(duplicated_subtrees(&before, &changed_copy).is_none());

        let mut changed_source = duplicate.clone();
        changed_source.node_mut(frame).ok_or("missing source")?.opacity = 0.5;
        assert!(duplicated_subtrees(&before, &changed_source).is_none());
        Ok(())
    }

    #[test]
    fn detects_only_a_single_subtree_translation() -> Result<(), Box<dyn Error>> {
        let mut before = Document::new("translation cache");
        let page = before.current_page.ok_or("missing page")?;
        let frame = before.create(page, NodeKind::Frame, "Frame", 10.0, 20.0, 100.0, 100.0)?;
        let child = before.create(frame, NodeKind::Rectangle, "Child", 2.0, 3.0, 20.0, 20.0)?;
        let mut moved = before.clone();
        moved.node_mut(frame).ok_or("missing frame")?.transform =
            Affine::translate((7.0, -4.0)) * before.node(frame).ok_or("missing frame")?.transform;
        assert_eq!(translated_node(&before, &moved), Some((frame, 7.0, -4.0)));
        assert!(moved.is_ancestor(frame, child));

        let mut rotated = before.clone();
        rotated.node_mut(frame).ok_or("missing frame")?.transform = Affine::rotate(0.5);
        assert_eq!(translated_node(&before, &rotated), None);
        assert!(rotated_node(&before, &rotated).is_some());

        let mut scaled = before.clone();
        scaled.node_mut(frame).ok_or("missing frame")?.transform = Affine::scale(1.5) * before.node(frame).ok_or("missing frame")?.transform;
        assert!(rotated_node(&before, &scaled).is_none());

        let mut changed_child = moved.clone();
        changed_child.node_mut(child).ok_or("missing child")?.visible = false;
        assert_eq!(translated_node(&before, &changed_child), None);

        let mut changed_size = moved.clone();
        changed_size.node_mut(frame).ok_or("missing frame")?.width += 1.0;
        assert_eq!(translated_node(&before, &changed_size), None);
        Ok(())
    }

    #[test]
    fn shape_resize_reuses_unchanged_children_but_rejects_changed_geometry() -> Result<(), Box<dyn Error>> {
        let mut before = Document::new("resize cache");
        let page = before.current_page.ok_or("missing page")?;
        let frame = before.create(page, NodeKind::Frame, "Frame", 10.0, 20.0, 100.0, 100.0)?;
        let child = before.create(frame, NodeKind::Rectangle, "Child", 2.0, 3.0, 20.0, 20.0)?;
        let mut resized = before.clone();
        let root = resized.node_mut(frame).ok_or("missing frame")?;
        root.width = 120.0;
        root.height = 130.0;
        root.transform = Affine::translate((5.0, 7.0)) * root.transform;
        let (id, delta) = resized_shape(&before, &resized).ok_or("resize not recognized")?;
        assert_eq!(id, frame);
        assert_eq!(delta.as_coeffs()[..4], [1.2, 0.0, 0.0, 1.3]);
        assert_eq!(translated_world_delta(&before, &resized, child), Some(Affine::translate((5.0, 7.0))));

        let mut changed_child = resized.clone();
        changed_child.node_mut(child).ok_or("missing child")?.width += 1.0;
        assert!(resized_shape(&before, &changed_child).is_none());

        let mut resized_rectangle = before.clone();
        resized_rectangle.node_mut(child).ok_or("missing child")?.width = 30.0;
        assert_eq!(resized_shape(&before, &resized_rectangle).map(|(id, _)| id), Some(child));
        Ok(())
    }
}

#[cfg(test)]
mod cached_shadow_capability_tests {
    use super::*;

    #[test]
    fn eligible_inner_shadow_uses_the_prepared_gpu_texture() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("inner shadow GPU capability");
        let page = doc.current_page.ok_or("missing page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Rectangle", 20.0, 20.0, 100.0, 80.0)?;
        doc.node_mut(id).ok_or("missing rectangle")?.effects =
            vec![figcraft_doc::Effect { kind: EffectKind::InnerShadow(Shadow::default()), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 200, height: 200 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
        doc.node_mut(id).ok_or("missing rectangle")?.opacity = 0.5;
        assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        Ok(())
    }

    #[test]
    fn eligible_drop_shadow_uses_the_prepared_gpu_texture() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("drop shadow GPU capability");
        let page = doc.current_page.ok_or("missing page")?;
        let id = doc.create(page, NodeKind::Frame, "Frame", 20.0, 20.0, 100.0, 80.0)?;
        let mut shadow = Shadow::default();
        shadow.color.a = 0.35;
        let node = doc.node_mut(id).ok_or("missing frame")?;
        node.corners.smoothing = 0.6;
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::DropShadow(shadow), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 200, height: 200 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
        Ok(())
    }
}

#[cfg(test)]
mod effect_capability_tests {
    use super::*;

    #[test]
    fn skewed_noise_matches_cpu_reference() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(320, 320) else { return Ok(()) };
        let mut doc = Document::new("GPU skewed noise");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Grain", 20.0, 20.0, 100.0, 80.0)?;
        let node = doc.node_mut(id).ok_or("grain missing")?;
        node.fills = vec![Paint::solid(Color::new(0.2, 0.4, 0.7, 1.0))];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true }];
        node.transform = Affine::new([1.0, 0.0, 0.4, 1.0, 30.0, 20.0]);
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 320, height: 320 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        gpu.prepare_static_effects(&doc)?;
        assert_eq!(gpu.noise_effects.len(), 1);
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let actual = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let changed = actual
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        assert!(changed * 100 < view.width as usize * view.height as usize, "skewed grain changed {changed} pixels");
        Ok(())
    }

    #[test]
    fn rotated_noise_uses_gpu_with_reference_parity() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU noise capability");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Grain", 20.0, 20.0, 80.0, 80.0)?;
        let node = doc.node_mut(id).ok_or("grain node missing")?;
        node.fills = vec![Paint::solid(Color::BLACK)];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 512, height: 512 };
        assert!(has_unhandled_effects_in_view(&doc, &view));
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        for angle in [std::f64::consts::FRAC_PI_6, std::f64::consts::FRAC_PI_3] {
            doc.node_mut(id).ok_or("grain node missing")?.transform = Affine::translate((20.0, 20.0)) * Affine::rotate(angle);
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.prepare_static_effects(&doc)?;
                assert_eq!(gpu.noise_effects.len(), 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let mut changed = 0usize;
                let mut error = 0u64;
                for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
                    let deltas = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
                    changed += usize::from(deltas.iter().any(|delta| *delta > 8));
                    error += deltas.iter().map(|delta| u64::from(*delta)).sum::<u64>();
                }
                println!(
                    "rotated noise angle={angle:.3} scale={scale} mean_rgb={:.3}/255 changed_gt8={:.3}%",
                    error as f64 / (f64::from(view.width * view.height) * 3.0),
                    changed as f64 * 100.0 / f64::from(view.width * view.height)
                );
                assert!(changed * 100 < view.width as usize * view.height as usize, "rotated noise changed {changed} pixels");
            }
        }
        doc.node_mut(id).ok_or("grain node missing")?.transform = Affine::translate((20.0, 20.0));
        if let Some(figcraft_doc::Effect { kind: EffectKind::Noise(noise), .. }) = doc.node_mut(id).ok_or("grain node missing")?.effects.first_mut() {
            noise.blend = BlendMode::Multiply;
        }
        assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        Ok(())
    }

    #[test]
    fn translucent_rotated_noise_keeps_shape_and_alpha() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent grain");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 240.0, 220.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Grain", 0.0, 0.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("grain missing")?;
        node.transform = Affine::translate((60.0, 50.0)) * Affine::rotate(0.36);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true }];
        for opacity in [0.25, 0.5, 0.75] {
            doc.node_mut(id).ok_or("grain missing")?.opacity = opacity;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (240.0 * scale) as u32, height: (220.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.prepare_static_effects(&doc)?;
                assert_eq!(gpu.cached_noise_count(), 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                assert!(severe * 100 < view.width as usize * view.height as usize);
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_rotated_blur_noise_stacks_apply_alpha_after_effects() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent blur noise");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Effects", 0.0, 0.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.transform = Affine::translate((75.0, 60.0)) * Affine::rotate(0.36);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        for (name, effects) in
            [("blur-noise", vec![blur(), noise()]), ("noise-blur", vec![noise(), blur()]), ("blur-noise-blur", vec![blur(), noise(), blur()])]
        {
            doc.node_mut(id).ok_or("shape missing")?.effects = effects;
            for opacity in [0.25, 0.5, 0.75] {
                doc.node_mut(id).ok_or("shape missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} opacity={opacity} scale={scale}");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    gpu.prepare_static_effects(&doc)?;
                    assert_eq!(gpu.layer_blurs.len(), 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{name} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                    assert!(severe * 100 < view.width as usize * view.height as usize);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_rotated_blur_noise_shadow_stacks_keep_effect_order() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent shadow stack");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Effects", 0.0, 0.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.transform = Affine::translate((75.0, 60.0)) * Affine::rotate(0.36);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        let shadow = || {
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(value) = &mut effect.kind {
                value.radius = 8.0;
                value.offset = (4.0, 6.0);
                value.color = Color::new(0.2, 0.1, 0.3, 0.4);
            }
            effect
        };
        let spread_shadow = |spread| {
            let mut effect = shadow();
            if let EffectKind::DropShadow(value) = &mut effect.kind {
                value.spread = spread;
            }
            effect
        };
        for (name, effects) in [
            ("blur-noise-shadow", vec![blur(), noise(), shadow()]),
            ("blur-shadow-noise", vec![blur(), shadow(), noise()]),
            ("noise-blur-shadow", vec![noise(), blur(), shadow()]),
            ("noise-shadow-blur", vec![noise(), shadow(), blur()]),
            ("shadow-blur-noise", vec![shadow(), blur(), noise()]),
            ("shadow-noise-blur", vec![shadow(), noise(), blur()]),
            ("blur-noise-positive-spread-shadow", vec![blur(), noise(), spread_shadow(8.0)]),
            ("negative-spread-shadow-noise-blur", vec![spread_shadow(-8.0), noise(), blur()]),
        ] {
            doc.node_mut(id).ok_or("shape missing")?.effects = effects;
            for opacity in [0.25, 0.5, 0.75] {
                doc.node_mut(id).ok_or("shape missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} opacity={opacity} scale={scale}");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    gpu.prepare_static_effects(&doc)?;
                    assert_eq!(gpu.layer_blurs.len(), 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{name} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                    assert!(severe * 100 < view.width as usize * view.height as usize);
                }
            }
        }
        Ok(())
    }

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
    fn translucent_rectangle_shadows_reuse_gpu_cache() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent rectangle shadows");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Shadowed", 70.0, 60.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        node.corners.radii = [12.0; 4];
        for inner in [false, true] {
            let mut effect = if inner { figcraft_doc::Effect::inner_shadow() } else { figcraft_doc::Effect::drop_shadow() };
            match &mut effect.kind {
                EffectKind::DropShadow(shadow) | EffectKind::InnerShadow(shadow) => {
                    shadow.radius = 8.0;
                    shadow.offset = (4.0, 6.0);
                    shadow.color = Color::new(0.2, 0.1, 0.3, 0.2);
                }
                _ => {}
            }
            doc.node_mut(id).ok_or("shape missing")?.effects = vec![effect];
            for opacity in [0.25, 0.5, 0.75] {
                doc.node_mut(id).ok_or("shape missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    let cache_counts = gpu.prepare_static_effects(&doc)?;
                    assert_eq!(if inner { cache_counts.2 } else { cache_counts.1 }, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("inner={inner} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                    assert!(severe * 100 < view.width as usize * view.height as usize);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_masked_group_shadows_reuse_gpu_cache() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent group shadows");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 300.0, 260.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let group = doc.create(page, NodeKind::Group, "Effects", 0.0, 0.0, 140.0, 110.0)?;
        doc.node_mut(group).ok_or("group missing")?.transform = Affine::translate((95.0, 75.0)) * Affine::rotate(0.35);
        let back = doc.create(group, NodeKind::Rectangle, "Back", 0.0, 0.0, 115.0, 88.0)?;
        doc.node_mut(back).ok_or("back missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.6, 0.9, 0.85))];
        let front = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Front", 65.0, 12.0, 72.0, 72.0)?;
        doc.node_mut(front).ok_or("front missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.3, 0.2, 0.75))];
        let mask = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Mask", 88.0, 60.0, 28.0, 28.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let masked = doc.create(group, NodeKind::Rectangle, "Masked", 82.0, 54.0, 40.0, 40.0)?;
        doc.node_mut(masked).ok_or("masked child missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.8, 0.4, 1.0))];
        for inner in [false, true] {
            let mut effect = if inner { figcraft_doc::Effect::inner_shadow() } else { figcraft_doc::Effect::drop_shadow() };
            match &mut effect.kind {
                EffectKind::DropShadow(shadow) | EffectKind::InnerShadow(shadow) => {
                    shadow.radius = 8.0;
                    shadow.offset = (4.0, 6.0);
                    shadow.color = Color::new(0.2, 0.1, 0.3, 0.4);
                }
                _ => {}
            }
            doc.node_mut(group).ok_or("group missing")?.effects = vec![effect];
            for opacity in [0.25, 0.5, 0.75] {
                doc.node_mut(group).ok_or("group missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (300.0 * scale) as u32, height: (260.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("inner={inner} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                    assert!(severe * 100 < view.width as usize * view.height as usize);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_shadowed_blends_keep_backdrop_and_silhouette() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent blended shadows");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Blended shadow", 70.0, 60.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        node.corners.radii = [12.0; 4];
        let mut effect = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(shadow) = &mut effect.kind {
            shadow.radius = 8.0;
            shadow.offset = (4.0, 6.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.2);
        }
        node.effects = vec![effect];
        let mut failures = Vec::new();
        for blend in BlendMode::ALL.into_iter().filter(|blend| !matches!(blend, BlendMode::PassThrough | BlendMode::Normal | BlendMode::PlusDarker)) {
            doc.node_mut(id).ok_or("shape missing")?.blend = blend;
            for opacity in [0.25, 0.5, 0.75] {
                doc.node_mut(id).ok_or("shape missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    gpu.prepare_static_effects(&doc)?;
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{blend:?} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    if changed * 100 >= view.width as usize * view.height as usize * 10 || severe * 100 >= view.width as usize * view.height as usize
                    {
                        failures.push(format!("{blend:?} opacity={opacity} scale={scale}: changed={changed} severe={severe}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }

    #[test]
    fn uniform_blur_with_node_blend_renders_directly_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU blended layer blur");
        let page = doc.current_page.ok_or("page missing")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Blurred", 70.0, 60.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        node.corners.radii = [12.0; 4];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true }];
        let mut failures = Vec::new();
        for blend in BlendMode::ALL.into_iter().filter(|blend| !matches!(blend, BlendMode::PassThrough | BlendMode::Normal | BlendMode::PlusDarker)) {
            for opacity in [1.0, 0.25, 0.5, 0.75] {
                let node = doc.node_mut(id).ok_or("shape missing")?;
                node.blend = blend;
                node.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.0, 0);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{blend:?} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    if changed * 100 >= view.width as usize * view.height as usize * 10 || severe * 100 >= view.width as usize * view.height as usize
                    {
                        failures.push(format!("{blend:?} opacity={opacity} scale={scale}: changed={changed} severe={severe}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }

    #[test]
    fn uniform_blur_with_paint_blends_isolates_leaf_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU blurred paint blends");
        let page = doc.current_page.ok_or("page missing")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Blurred", 70.0, 60.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0)), Paint::solid(Color::new(0.28, 0.75, 0.9, 0.65))];
        node.corners.radii = [12.0; 4];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true }];
        let mut failures = Vec::new();
        for blend in BlendMode::ALL.into_iter().filter(|blend| !matches!(blend, BlendMode::PassThrough | BlendMode::Normal | BlendMode::PlusDarker)) {
            for opacity in [1.0, 0.25, 0.5, 0.75] {
                let node = doc.node_mut(id).ok_or("shape missing")?;
                node.fills[1].blend = blend;
                node.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    gpu.prepare_static_effects(&doc)?;
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{blend:?} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    if changed * 100 >= view.width as usize * view.height as usize * 10 || severe * 100 >= view.width as usize * view.height as usize
                    {
                        failures.push(format!("{blend:?} opacity={opacity} scale={scale}: changed={changed} severe={severe}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }

    #[test]
    fn uniform_blur_combines_node_and_paint_blends_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU combined blurred blends");
        let page = doc.current_page.ok_or("page missing")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 220.0, 180.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Blurred", 50.0, 40.0, 110.0, 80.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0)), Paint::solid(Color::new(0.28, 0.75, 0.9, 0.65))];
        node.corners.radii = [12.0; 4];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true }];
        let mut failures = Vec::new();
        let blends = BlendMode::ALL
            .into_iter()
            .filter(|blend| !matches!(blend, BlendMode::PassThrough | BlendMode::Normal | BlendMode::PlusDarker))
            .collect::<Vec<_>>();
        for (index, node_blend) in blends.iter().copied().enumerate() {
            let paint_blend = blends[(index + 5) % blends.len()];
            for opacity in [1.0, 0.5] {
                let node = doc.node_mut(id).ok_or("shape missing")?;
                node.blend = node_blend;
                node.fills[1].blend = paint_blend;
                node.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (220.0 * scale) as u32, height: (180.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.0, 0);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    if changed * 100 >= view.width as usize * view.height as usize * 10 || severe * 100 >= view.width as usize * view.height as usize
                    {
                        failures.push(format!("{node_blend:?}/{paint_blend:?} opacity={opacity} scale={scale}: changed={changed} severe={severe}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }

    #[test]
    fn supported_masks_use_gpu_and_unimplemented_paints_fall_back() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU content capability");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Masked group", 0.0, 0.0, 120.0, 120.0)?;
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 0.0, 60.0, 120.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let content = doc.create(group, NodeKind::Rectangle, "Content", 0.0, 0.0, 120.0, 120.0)?;
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 512, height: 512 };
        assert!(!has_unhandled_effects_in_view(&doc, &view));

        doc.node_mut(mask).ok_or("mask missing")?.mask_type = MaskType::Luminance;
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        doc.node_mut(mask).ok_or("mask missing")?.strokes = vec![Paint::solid(Color::BLACK)];
        assert!(has_unhandled_effects_in_view(&doc, &view));
        doc.node_mut(mask).ok_or("mask missing")?.strokes.clear();
        doc.node_mut(mask).ok_or("mask missing")?.mask_type = MaskType::Vector;
        assert!(!has_unhandled_effects_in_view(&doc, &view));

        doc.node_mut(mask).ok_or("mask missing")?.is_mask = false;
        doc.node_mut(content).ok_or("content missing")?.blend = BlendMode::Multiply;
        assert!(!has_unhandled_effects_in_view(&doc, &view));

        doc.node_mut(content).ok_or("content missing")?.blend = BlendMode::PlusDarker;
        assert!(has_unhandled_effects_in_view(&doc, &view));

        doc.node_mut(content).ok_or("content missing")?.blend = BlendMode::Normal;
        doc.node_mut(content).ok_or("content missing")?.fills =
            vec![Paint::gradient(figcraft_color::Gradient::default_for(GradientKind::Diamond, Color::new(1.0, 0.0, 0.0, 1.0)))];
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        Ok(())
    }

    #[test]
    fn gpu_mask_run_clips_only_its_content() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU mask pixels");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Group", 0.0, 0.0, 100.0, 100.0)?;
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 0.0, 50.0, 100.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let content = doc.create(group, NodeKind::Rectangle, "Content", 0.0, 0.0, 100.0, 100.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        let mut gpu = GpuCanvas::new(100, 100)?;
        for mask_type in [MaskType::Alpha, MaskType::Vector] {
            doc.node_mut(mask).ok_or("mask missing")?.mask_type = mask_type;
            gpu.render_submit_document(&doc, page, &view, [1.0, 1.0, 1.0])?;
            let pixels = gpu.pixels()?;
            let sample = |x: usize| &pixels[(50 * 100 + x) * 4..(50 * 100 + x) * 4 + 4];
            assert!(sample(25)[0] < 32, "{mask_type:?} did not show content inside mask: {:?}", sample(25));
            assert!(sample(75)[0] > 220, "{mask_type:?} leaked outside mask: {:?}", sample(75));
        }
        doc.node_mut(mask).ok_or("mask missing")?.mask_type = MaskType::Luminance;
        doc.node_mut(mask).ok_or("mask missing")?.fills = vec![Paint::solid(Color::BLACK)];
        gpu.render_submit_document(&doc, page, &view, [1.0, 1.0, 1.0])?;
        let pixels = gpu.pixels()?;
        assert!(pixels[(50 * 100 + 25) * 4] > 220, "black luminance must hide the content");
        doc.node_mut(mask).ok_or("mask missing")?.fills = vec![Paint::solid(Color::WHITE)];
        gpu.render_submit_document(&doc, page, &view, [1.0, 1.0, 1.0])?;
        let pixels = gpu.pixels()?;
        assert!(pixels[(50 * 100 + 25) * 4] < 32, "white luminance must reveal the content");
        Ok(())
    }

    #[test]
    fn gpu_luminance_gradient_matches_reference_samples() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU luminance gradient");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Group", 0.0, 0.0, 100.0, 100.0)?;
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 0.0, 100.0, 100.0)?;
        let mut gradient = figcraft_color::Gradient::default_for(GradientKind::Linear, Color::WHITE);
        gradient.stops =
            vec![figcraft_color::ColorStop { position: 0.0, color: Color::WHITE }, figcraft_color::ColorStop { position: 1.0, color: Color::BLACK }];
        let mask_node = doc.node_mut(mask).ok_or("mask missing")?;
        mask_node.is_mask = true;
        mask_node.mask_type = MaskType::Luminance;
        mask_node.fills = vec![Paint::gradient(gradient)];
        let content = doc.create(group, NodeKind::Rectangle, "Content", 0.0, 0.0, 100.0, 100.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference rendering failed")?;
        let mut gpu = GpuCanvas::new(100, 100)?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let pixels = gpu.pixels()?;
        for x in [10, 50, 90] {
            let offset = (50 * 100 + x) * 4;
            let expected = reference.data()[offset];
            let actual = pixels[offset];
            assert!(actual.abs_diff(expected) <= 12, "luminance x={x}: GPU={actual}, reference={expected}");
        }
        if let Some(Paint { kind: PaintKind::Gradient(gradient), .. }) = doc.node_mut(mask).and_then(|node| node.fills.first_mut()) {
            gradient.stops[0].color.a = 0.7;
            gradient.stops[1].color = Color::new(0.3, 0.3, 0.3, 0.4);
        }
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("translucent reference rendering failed")?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let pixels = gpu.pixels()?;
        for x in [10, 30, 50, 70, 90] {
            let offset = (50 * 100 + x) * 4;
            let expected = reference.data()[offset];
            let actual = pixels[offset];
            assert!(actual.abs_diff(expected) <= 16, "translucent luminance x={x}: GPU={actual}, reference={expected}");
        }
        Ok(())
    }

    #[test]
    fn single_stroke_luminance_mask_uses_gpu() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(100, 100) else { return Ok(()) };
        let mut doc = Document::new("GPU stroke luminance mask");
        let page = doc.current_page.ok_or("page missing")?;
        let group = doc.create(page, NodeKind::Group, "Group", 0.0, 0.0, 100.0, 100.0)?;
        let mask = doc.create(group, NodeKind::Rectangle, "Stroke mask", 20.0, 20.0, 60.0, 60.0)?;
        let node = doc.node_mut(mask).ok_or("mask missing")?;
        node.is_mask = true;
        node.mask_type = MaskType::Luminance;
        node.fills.clear();
        node.strokes = vec![Paint::solid(Color::new(1.0, 1.0, 1.0, 0.6))];
        node.stroke.weight = 10.0;
        let content = doc.create(group, NodeKind::Rectangle, "Black content", 0.0, 0.0, 100.0, 100.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("stroke mask reference unavailable")?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let pixels = gpu.pixels()?;
        for (x, y) in [(50, 20), (20, 50), (50, 50), (5, 5), (85, 50)] {
            let offset = ((y * view.width + x) * 4) as usize;
            for channel in 0..3 {
                assert!(pixels[offset + channel].abs_diff(reference.data()[offset + channel]) <= 16, "stroke mask at ({x},{y})");
            }
        }
        let mut gradient = figcraft_color::Gradient::default_for(GradientKind::Linear, Color::WHITE);
        gradient.stops = vec![
            figcraft_color::ColorStop { position: 0.0, color: Color::new(1.0, 1.0, 1.0, 0.8) },
            figcraft_color::ColorStop { position: 1.0, color: Color::new(0.2, 0.2, 0.2, 0.5) },
        ];
        doc.node_mut(mask).ok_or("mask missing")?.strokes = vec![Paint::gradient(gradient)];
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("gradient stroke mask reference unavailable")?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let pixels = gpu.pixels()?;
        for (x, y) in [(50, 20), (20, 50), (50, 50), (5, 5), (85, 50)] {
            let offset = ((y * view.width + x) * 4) as usize;
            for channel in 0..3 {
                assert!(pixels[offset + channel].abs_diff(reference.data()[offset + channel]) <= 16, "gradient stroke mask at ({x},{y})");
            }
        }
        Ok(())
    }

    #[test]
    fn uniform_text_luminance_mask_uses_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU text luminance mask");
        let page = doc.current_page.ok_or("page missing")?;
        let group = doc.create(page, NodeKind::Group, "Group", 0.0, 0.0, 200.0, 100.0)?;
        let mut text = figcraft_doc::TextData::new("Lizard");
        text.style.font_size = 52.0;
        let mask = doc.create(group, NodeKind::Text(text), "Text mask", 12.0, 10.0, 180.0, 70.0)?;
        let mask_node = doc.node_mut(mask).ok_or("mask missing")?;
        mask_node.is_mask = true;
        mask_node.mask_type = MaskType::Luminance;
        mask_node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.8, 0.75))];
        let content = doc.create(group, NodeKind::Rectangle, "Content", 0.0, 0.0, 200.0, 100.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::BLACK)];
        if draw::node_outline(&doc, doc.node(mask).ok_or("mask missing")?).is_none() {
            return Ok(());
        }
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 200, height: 100 };
        let mut gpu = GpuCanvas::new(view.width, view.height)?;
        let mut gradient = figcraft_color::Gradient::default_for(GradientKind::Linear, Color::WHITE);
        gradient.stops = vec![
            figcraft_color::ColorStop { position: 0.0, color: Color::new(1.0, 1.0, 1.0, 0.8) },
            figcraft_color::ColorStop { position: 1.0, color: Color::new(0.3, 0.3, 0.3, 0.5) },
        ];
        for (label, paint) in [("solid", Paint::solid(Color::new(0.8, 0.8, 0.8, 0.75))), ("gradient", Paint::gradient(gradient))] {
            doc.node_mut(mask).ok_or("mask missing")?.fills = vec![paint];
            assert!(!has_unhandled_effects_in_view(&doc, &view), "GPU capability for {label} text mask");
            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference rendering failed")?;
            gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
            let actual = gpu.pixels()?;
            let revealed = reference.data().chunks_exact(4).filter(|pixel| pixel[0] < 220).count();
            assert!(revealed > 100, "{label} text mask fixture must reveal glyphs");
            let changed = actual
                .chunks_exact(4)
                .zip(reference.data().chunks_exact(4))
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 16))
                .count();
            eprintln!("{label} text luminance mask revealed={revealed} changed={changed}");
            assert!(changed * 100 < view.width as usize * view.height as usize, "{label} text luminance mask changed {changed} pixels");
        }
        if let NodeKind::Text(text) = &mut doc.node_mut(mask).ok_or("mask missing")?.kind {
            text.style.fills = Some(vec![Paint::solid(Color::new(0.2, 0.3, 0.8, 1.0))]);
            let mut style = text.style.clone();
            style.fills = Some(vec![Paint::solid(Color::new(0.9, 0.8, 0.1, 0.7))]);
            text.runs.push(figcraft_doc::StyleRun { start: 3, end: 6, style });
        }
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("styled reference rendering failed")?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let actual = gpu.pixels()?;
        let changed = actual
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 16))
            .count();
        println!("styled text luminance mask changed={changed}");
        assert!(changed * 100 < view.width as usize * view.height as usize);
        Ok(())
    }

    #[test]
    fn nested_alpha_and_luminance_masks_render_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Nested GPU masks");
        let page = doc.current_page.ok_or("page missing")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 160.0, 140.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.1, 0.2, 0.3, 1.0))];
        let outer = doc.create(page, NodeKind::Group, "Outer", 0.0, 0.0, 120.0, 110.0)?;
        doc.node_mut(outer).ok_or("outer missing")?.transform = Affine::translate((20.0, 15.0)) * Affine::rotate(0.12);
        let alpha = doc.create(outer, NodeKind::Rectangle, "Alpha mask", 0.0, 0.0, 105.0, 100.0)?;
        let node = doc.node_mut(alpha).ok_or("alpha mask missing")?;
        node.is_mask = true;
        node.mask_type = MaskType::Alpha;
        node.fills = vec![Paint::solid(Color::new(1.0, 1.0, 1.0, 0.8))];
        let inner = doc.create(outer, NodeKind::Group, "Inner", 0.0, 0.0, 120.0, 110.0)?;
        let luminance = doc.create(inner, NodeKind::Rectangle, "Luminance mask", 5.0, 5.0, 105.0, 90.0)?;
        let node = doc.node_mut(luminance).ok_or("luminance mask missing")?;
        node.is_mask = true;
        node.mask_type = MaskType::Luminance;
        node.fills = vec![Paint::solid(Color::new(0.6, 0.6, 0.6, 1.0))];
        let content = doc.create(inner, NodeKind::Rectangle, "Content", 20.0, 20.0, 60.0, 50.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.3, 0.2, 1.0))];
        let mut baseline = HashMap::<u64, Vec<u8>>::new();
        for shadowed in [false, true] {
            if shadowed {
                let mut effect = figcraft_doc::Effect::drop_shadow();
                if let EffectKind::DropShadow(shadow) = &mut effect.kind {
                    shadow.radius = 6.0;
                    shadow.offset = (3.0, 4.0);
                    shadow.color = Color::new(0.0, 0.0, 0.0, 0.5);
                    shadow.show_behind = true;
                }
                doc.node_mut(content).ok_or("content missing")?.effects = vec![effect];
            }
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (160.0 * scale) as u32, height: (140.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                if shadowed {
                    let plain = baseline.get(&scale.to_bits()).ok_or("plain mask comparison missing")?;
                    let shadow_pixels = reference
                        .data()
                        .chunks_exact(4)
                        .zip(plain.chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 4))
                        .count();
                    println!("shadow fixture at {scale}x changed {shadow_pixels} pixels");
                    assert!(shadow_pixels > 50, "shadow fixture must show the effect inside nested masks");
                } else {
                    baseline.insert(scale.to_bits(), reference.data().to_vec());
                }
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.prepare_static_effects(&doc)?;
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("nested masks shadow={shadowed} at {scale}x: changed={changed} severe={severe}");
                assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                assert!(severe * 100 < view.width as usize * view.height as usize);
            }
        }
        Ok(())
    }

    #[test]
    fn page_level_alpha_mask_preserves_siblings() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Page mask order");
        let page = doc.current_page.ok_or("new document has no page")?;
        let background = doc.create(page, NodeKind::Rectangle, "Background", 0.0, 0.0, 100.0, 100.0)?;
        doc.node_mut(background).ok_or("background missing")?.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        let mask = doc.create(page, NodeKind::Rectangle, "Mask", 0.0, 0.0, 50.0, 100.0)?;
        let mask_node = doc.node_mut(mask).ok_or("mask missing")?;
        mask_node.is_mask = true;
        mask_node.fills = vec![Paint::solid(Color::new(1.0, 1.0, 1.0, 0.5))];
        let content = doc.create(page, NodeKind::Rectangle, "Content", 0.0, 0.0, 100.0, 100.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        let mut gpu = GpuCanvas::new(100, 100)?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let pixels = gpu.pixels()?;
        let inside = (50 * 100 + 25) * 4;
        let outside = (50 * 100 + 75) * 4;
        assert!((110..=145).contains(&pixels[inside]), "translucent mask should blend black with red");
        assert!(pixels[outside] > 240, "unmasked red sibling must remain visible");
        assert!(pixels[inside + 1] < 16 && pixels[outside + 1] < 16);
        Ok(())
    }

    #[test]
    fn alpha_group_mask_uses_child_geometry_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Group alpha mask");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Masked group", 0.0, 0.0, 100.0, 100.0)?;
        let mask = doc.create(group, NodeKind::Group, "Group mask", 0.0, 0.0, 100.0, 100.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let shape = doc.create(mask, NodeKind::Rectangle, "Mask shape", 0.0, 0.0, 50.0, 100.0)?;
        doc.node_mut(shape).ok_or("shape missing")?.fills = vec![Paint::solid(Color::WHITE)];
        let content = doc.create(group, NodeKind::Rectangle, "Content", 0.0, 0.0, 100.0, 100.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        let mut gpu = GpuCanvas::new(100, 100)?;
        gpu.render_submit_document(&doc, page, &view, [1.0; 3])?;
        let pixels = gpu.pixels()?;
        assert!(pixels[(50 * 100 + 25) * 4] < 32);
        assert!(pixels[(50 * 100 + 75) * 4] > 220);
        Ok(())
    }

    #[test]
    fn uniform_background_blur_uses_ordered_gpu_backdrop() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU background blur");
        let page = doc.current_page.ok_or("new document has no page")?;
        let dark = doc.create(page, NodeKind::Rectangle, "Dark half", 0.0, 0.0, 50.0, 100.0)?;
        doc.node_mut(dark).ok_or("dark half missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let pane = doc.create(page, NodeKind::Rectangle, "Blur pane", 20.0, 20.0, 60.0, 60.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.fills.clear();
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::BackgroundBlur(figcraft_doc::Blur::uniform(12.0)), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(100, 100)?;
        let index = BackdropIndex::new(&doc, page);
        assert_eq!(index.visible_ids(&view), vec![pane]);
        gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
        let pixels = gpu.pixels()?;
        for x in [35, 47, 53, 65] {
            let offset = (50 * 100 + x) * 4;
            let actual = pixels[offset];
            let expected = reference.data()[offset];
            assert!(actual.abs_diff(expected) <= 20, "x={x}: GPU={actual}, reference={expected}");
        }
        Ok(())
    }

    #[test]
    fn progressive_background_blur_uses_ordered_gpu_backdrop() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU progressive background blur");
        let page = doc.current_page.ok_or("new document has no page")?;
        let dark = doc.create(page, NodeKind::Rectangle, "Dark half", 0.0, 0.0, 50.0, 100.0)?;
        doc.node_mut(dark).ok_or("dark half missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let pane = doc.create(page, NodeKind::Rectangle, "Blur pane", 20.0, 20.0, 60.0, 60.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.fills.clear();
        node.effects = vec![figcraft_doc::Effect {
            kind: EffectKind::BackgroundBlur(figcraft_doc::Blur {
                radius: 16.0,
                blur_type: figcraft_doc::BlurType::Progressive,
                start_radius: 0.0,
                start: (0.5, 0.0),
                end: (0.5, 1.0),
            }),
            visible: true,
        }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(100, 100)?;
        let index = BackdropIndex::new(&doc, page);
        assert_eq!(index.visible_ids(&view), vec![pane]);
        gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
        let pixels = gpu.pixels()?;
        for y in [25, 50, 75] {
            for x in [47, 53] {
                let offset = (y * 100 + x) * 4;
                let actual = pixels[offset];
                let expected = reference.data()[offset];
                assert!(actual.abs_diff(expected) <= 20, "x={x}, y={y}: GPU={actual}, reference={expected}");
            }
        }
        let warm_ms = gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?.0;
        println!("progressive_backdrop_warm_ms={warm_ms:.2}");
        Ok(())
    }

    #[test]
    fn progressive_layer_blur_matches_reference_at_two_scales() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU progressive layer blur");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Blurred shape", 55.0, 45.0, 110.0, 100.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.25, 0.15, 0.8, 1.0))];
        node.effects = vec![figcraft_doc::Effect {
            kind: EffectKind::LayerBlur(figcraft_doc::Blur {
                radius: 16.0,
                blur_type: figcraft_doc::BlurType::Progressive,
                start_radius: 0.0,
                start: (0.0, 0.0),
                end: (1.0, 1.0),
            }),
            visible: true,
        }];
        for opacity in [1.0, 0.25, 0.5, 0.75] {
            doc.node_mut(id).ok_or("shape missing")?.opacity = opacity;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: 220 * scale as u32, height: 200 * scale as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                assert!(
                    changed * 1000 < view.width as usize * view.height as usize,
                    "progressive blur at {scale}x opacity={opacity} changed {changed} pixels"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_uniform_blur_reuses_gpu_cache() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent layer blur");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Blurred", 0.0, 0.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.transform = Affine::translate((75.0, 60.0)) * Affine::rotate(0.36);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true }];
        for opacity in [0.25, 0.5, 0.75] {
            doc.node_mut(id).ok_or("shape missing")?.opacity = opacity;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                assert!(severe * 100 < view.width as usize * view.height as usize);
            }
        }
        Ok(())
    }

    #[test]
    fn stacked_layer_blur_then_noise_stays_on_gpu_in_order() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU ordered layer effects");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Blurred grain", 40.0, 36.0, 120.0, 96.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 1.0))];
        node.effects = vec![
            figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true },
            figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true },
        ];
        for blur_first in [true, false] {
            if !blur_first {
                doc.node_mut(id).ok_or("shape missing")?.effects.reverse();
            }
            for angle in [0.0, 0.5] {
                doc.node_mut(id).ok_or("shape missing")?.transform = Affine::rotate(angle);
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (300.0 * scale) as u32, height: (260.0 * scale) as u32 };
                    assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    // The two grain generators differ at individual pixels after rotating a
                    // blurred surface. Preserve the silhouette and effect order instead of
                    // requiring their random samples to be byte-identical.
                    assert!(
                        changed * 100 < view.width as usize * view.height as usize * 8,
                        "stacked effects blur_first={blur_first} angle={angle} at {scale}x changed {changed} pixels"
                    );
                    assert!(severe * 100 < view.width as usize * view.height as usize, "severe effect difference at {scale}x: {severe}");
                }
            }
        }
        doc.node_mut(id).ok_or("shape missing")?.transform = Affine::rotate(0.5);
        doc.node_mut(id).ok_or("shape missing")?.effects = vec![
            figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true },
            figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true },
            figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true },
        ];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 300, height: 260 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "rotated blur/noise stacks should use the GPU cache");
        Ok(())
    }

    #[test]
    fn longer_blur_noise_stacks_preserve_order_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU ordered effect graph");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Effects", 40.0, 36.0, 120.0, 96.0)?;
        doc.node_mut(id).ok_or("shape missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 1.0))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        for (name, effects) in [
            ("blur-noise-blur", vec![blur(), noise(), blur()]),
            ("noise-blur-noise", vec![noise(), blur(), noise()]),
            ("blur-noise-blur-noise", vec![blur(), noise(), blur(), noise()]),
        ] {
            doc.node_mut(id).ok_or("shape missing")?.effects = effects;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (220.0 * scale) as u32, height: (200.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} capability");
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let changed = gpu
                    .pixels()?
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                assert!(changed * 100 < view.width as usize * view.height as usize, "{name} at {scale}x changed {changed} pixels");
            }
        }
        Ok(())
    }

    #[test]
    fn rotated_longer_blur_noise_stacks_keep_contour_and_order() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU rotated effect graph");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Effects", 0.0, 0.0, 120.0, 96.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.transform = Affine::translate((100.0, 80.0)) * Affine::rotate(0.4);
        node.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 1.0))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        for (name, effects) in [
            ("blur-noise-blur", vec![blur(), noise(), blur()]),
            ("noise-blur-noise", vec![noise(), blur(), noise()]),
            ("blur-noise-blur-noise", vec![blur(), noise(), blur(), noise()]),
        ] {
            doc.node_mut(id).ok_or("shape missing")?.effects = effects;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (300.0 * scale) as u32, height: (260.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} GPU capability");
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                if let Ok(prefix) = std::env::var("LIZARD_GPU_ROTATED_STACK_SCREENSHOT") {
                    image::save_buffer(format!("{prefix}-{name}-{scale}-gpu.png"), &actual, view.width, view.height, image::ColorType::Rgba8)?;
                    image::save_buffer(
                        format!("{prefix}-{name}-{scale}-cpu.png"),
                        reference.data(),
                        view.width,
                        view.height,
                        image::ColorType::Rgba8,
                    )?;
                }
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("{name} {scale}x: changed={changed} severe={severe} pixels={}", view.width * view.height);
                assert!(changed * 100 < view.width as usize * view.height as usize * 8, "{name} at {scale}x changed {changed} pixels");
                assert!(severe * 100 < view.width as usize * view.height as usize, "{name} at {scale}x severe {severe} pixels");
            }
        }
        Ok(())
    }

    #[test]
    fn rotated_blur_noise_shadow_stacks_keep_shadow_and_grain() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU rotated shadow and grain");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Effects", 0.0, 0.0, 120.0, 96.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.transform = Affine::translate((100.0, 80.0)) * Affine::rotate(0.4);
        node.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 1.0))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        let shadow = || {
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(shadow) = &mut effect.kind {
                shadow.radius = 8.0;
                shadow.offset = (4.0, 6.0);
                shadow.color = Color::new(0.2, 0.1, 0.3, 0.4);
            }
            effect
        };
        for (name, effects) in [
            ("blur-noise-shadow", vec![blur(), noise(), shadow()]),
            ("noise-blur-shadow", vec![noise(), blur(), shadow()]),
            ("shadow-noise-blur", vec![shadow(), noise(), blur()]),
            ("shadow-blur-noise", vec![shadow(), blur(), noise()]),
            ("blur-shadow-noise", vec![blur(), shadow(), noise()]),
            ("noise-shadow-blur", vec![noise(), shadow(), blur()]),
        ] {
            doc.node_mut(id).ok_or("shape missing")?.effects = effects;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (300.0 * scale) as u32, height: (260.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} GPU capability");
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                if let Ok(prefix) = std::env::var("LIZARD_GPU_ROTATED_STACK_SCREENSHOT") {
                    image::save_buffer(format!("{prefix}-{name}-{scale}-gpu.png"), &actual, view.width, view.height, image::ColorType::Rgba8)?;
                    image::save_buffer(
                        format!("{prefix}-{name}-{scale}-cpu.png"),
                        reference.data(),
                        view.width,
                        view.height,
                        image::ColorType::Rgba8,
                    )?;
                }
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("{name} {scale}x: changed={changed} severe={severe} pixels={}", view.width * view.height);
                assert!(changed * 100 < view.width as usize * view.height as usize * 8, "{name} at {scale}x changed {changed} pixels");
                assert!(severe * 100 < view.width as usize * view.height as usize, "{name} at {scale}x severe {severe} pixels");
            }
        }
        Ok(())
    }

    #[test]
    fn rotated_group_blur_noise_shadow_orders_keep_children_and_mask() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU rotated group shadow and grain");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Effects", 0.0, 0.0, 140.0, 110.0)?;
        doc.node_mut(group).ok_or("group missing")?.transform = Affine::translate((95.0, 75.0)) * Affine::rotate(0.35);
        let back = doc.create(group, NodeKind::Rectangle, "Back", 0.0, 0.0, 115.0, 88.0)?;
        doc.node_mut(back).ok_or("back missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.6, 0.9, 0.85))];
        let front = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Front", 65.0, 12.0, 72.0, 72.0)?;
        doc.node_mut(front).ok_or("front missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.3, 0.2, 0.75))];
        let mask = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Mask", 88.0, 60.0, 28.0, 28.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let masked = doc.create(group, NodeKind::Rectangle, "Masked", 82.0, 54.0, 40.0, 40.0)?;
        doc.node_mut(masked).ok_or("masked child missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.8, 0.4, 1.0))];
        let mut shadow = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(value) = &mut shadow.kind {
            value.radius = 8.0;
            value.offset = (4.0, 6.0);
            value.color = Color::new(0.2, 0.1, 0.3, 0.4);
        }
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        for (name, effects) in [
            ("blur-noise-shadow", vec![blur(), noise(), shadow.clone()]),
            ("noise-blur-shadow", vec![noise(), blur(), shadow.clone()]),
            ("shadow-noise-blur", vec![shadow.clone(), noise(), blur()]),
            ("shadow-blur-noise", vec![shadow.clone(), blur(), noise()]),
            ("blur-shadow-noise", vec![blur(), shadow.clone(), noise()]),
            ("noise-shadow-blur", vec![noise(), shadow.clone(), blur()]),
        ] {
            doc.node_mut(group).ok_or("group missing")?.effects = effects;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (300.0 * scale) as u32, height: (260.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} capability");
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                if let Ok(prefix) = std::env::var("LIZARD_GPU_ROTATED_STACK_SCREENSHOT") {
                    image::save_buffer(
                        format!("{prefix}-rotated-group-{name}-{scale}-gpu.png"),
                        &actual,
                        view.width,
                        view.height,
                        image::ColorType::Rgba8,
                    )?;
                    image::save_buffer(
                        format!("{prefix}-rotated-group-{name}-{scale}-cpu.png"),
                        reference.data(),
                        view.width,
                        view.height,
                        image::ColorType::Rgba8,
                    )?;
                }
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("group {name} {scale}x: changed={changed} severe={severe} pixels={}", view.width * view.height);
                assert!(changed * 100 < view.width as usize * view.height as usize * 8, "group {name} at {scale}x changed {changed} pixels");
                assert!(severe * 100 < view.width as usize * view.height as usize, "group {name} at {scale}x severe {severe} pixels");
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_masked_group_effect_stacks_keep_children() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent masked group effects");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 300.0, 260.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let group = doc.create(page, NodeKind::Group, "Effects", 0.0, 0.0, 140.0, 110.0)?;
        let node = doc.node_mut(group).ok_or("group missing")?;
        node.transform = Affine::translate((95.0, 75.0)) * Affine::rotate(0.35);
        node.opacity = 0.5;
        let back = doc.create(group, NodeKind::Rectangle, "Back", 0.0, 0.0, 115.0, 88.0)?;
        doc.node_mut(back).ok_or("back missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.6, 0.9, 0.85))];
        let front = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Front", 65.0, 12.0, 72.0, 72.0)?;
        doc.node_mut(front).ok_or("front missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.3, 0.2, 0.75))];
        let mask = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Mask", 88.0, 60.0, 28.0, 28.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let masked = doc.create(group, NodeKind::Rectangle, "Masked", 82.0, 54.0, 40.0, 40.0)?;
        doc.node_mut(masked).ok_or("masked child missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.8, 0.4, 1.0))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        let texture = || figcraft_doc::Effect {
            kind: EffectKind::Texture(Texture { size: 2.0, size_y: None, radius: 3.0, clip_to_shape: false }),
            visible: true,
        };
        let shadow = || {
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(value) = &mut effect.kind {
                value.radius = 8.0;
                value.offset = (4.0, 6.0);
                value.color = Color::new(0.2, 0.1, 0.3, 0.4);
            }
            effect
        };
        let spread_shadow = || {
            let mut effect = shadow();
            if let EffectKind::DropShadow(value) = &mut effect.kind {
                value.spread = 8.0;
            }
            effect
        };
        for (name, effects) in [
            ("blur-noise-shadow", vec![blur(), noise(), shadow()]),
            ("noise-blur-shadow", vec![noise(), blur(), shadow()]),
            ("shadow-noise-blur", vec![shadow(), noise(), blur()]),
            ("shadow-blur-noise", vec![shadow(), blur(), noise()]),
            ("blur-shadow-noise", vec![blur(), shadow(), noise()]),
            ("noise-shadow-blur", vec![noise(), shadow(), blur()]),
            ("texture-blur", vec![texture(), blur()]),
            ("shadow-texture-noise", vec![shadow(), texture(), noise()]),
            ("blur-noise-texture-shadow", vec![blur(), noise(), texture(), shadow()]),
            ("texture-group-spread-shadow", vec![texture(), spread_shadow()]),
        ] {
            doc.node_mut(group).ok_or("group missing")?.effects = effects;
            for opacity in [0.25, 0.5, 0.75] {
                doc.node_mut(group).ok_or("group missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (300.0 * scale) as u32, height: (260.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} capability");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{name} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                    assert!(severe * 100 < view.width as usize * view.height as usize);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn blended_rotated_blur_noise_cache_matches_reference() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU blended cached effects");
        let page = doc.current_page.ok_or("new document has no page")?;
        let back = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 220.0)?;
        doc.node_mut(back).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.25, 0.65, 0.8, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Blended", 0.0, 0.0, 120.0, 96.0)?;
        let node = doc.node_mut(id).ok_or("blended shape missing")?;
        node.transform = Affine::translate((85.0, 65.0)) * Affine::rotate(0.35);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.35, 0.2, 1.0))];
        let mut failures = Vec::new();
        for blend in [
            BlendMode::Darken,
            BlendMode::Multiply,
            BlendMode::ColorBurn,
            BlendMode::Screen,
            BlendMode::PlusLighter,
            BlendMode::Overlay,
            BlendMode::SoftLight,
            BlendMode::Exclusion,
            BlendMode::Hue,
            BlendMode::Saturation,
            BlendMode::Color,
            BlendMode::Luminosity,
        ] {
            for blur_first in [true, false] {
                let node = doc.node_mut(id).ok_or("blended shape missing")?;
                node.blend = blend;
                let blur = figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
                let noise = figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
                node.effects = if blur_first { vec![blur, noise] } else { vec![noise, blur] };
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (220.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{blend:?} blur_first={blur_first} capability");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    if let Ok(prefix) = std::env::var("LIZARD_GPU_BLEND_SCREENSHOT") {
                        image::save_buffer(
                            format!("{prefix}-{blend:?}-{blur_first}-{scale}-gpu.png"),
                            &actual,
                            view.width,
                            view.height,
                            image::ColorType::Rgba8,
                        )?;
                        image::save_buffer(
                            format!("{prefix}-{blend:?}-{blur_first}-{scale}-cpu.png"),
                            reference.data(),
                            view.width,
                            view.height,
                            image::ColorType::Rgba8,
                        )?;
                    }
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{blend:?} blur_first={blur_first} {scale}x: changed={changed} severe={severe}");
                    if changed * 100 >= view.width as usize * view.height as usize * 8 || severe * 100 >= view.width as usize * view.height as usize {
                        failures.push(format!("{blend:?} blur_first={blur_first} {scale}x changed={changed} severe={severe}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
        for blend in [BlendMode::Lighten, BlendMode::ColorDodge, BlendMode::HardLight, BlendMode::Difference] {
            doc.node_mut(id).ok_or("blended shape missing")?.blend = blend;
            let view = View { origin: (0.0, 0.0), scale: 1.0, width: 260, height: 220 };
            assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{blend:?} stays guarded");
        }
        Ok(())
    }

    #[test]
    fn partially_opaque_blended_blur_noise_cache_keeps_alpha() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU partial-opacity cached blend");
        let page = doc.current_page.ok_or("page missing")?;
        let back = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 220.0)?;
        doc.node_mut(back).ok_or("back missing")?.fills = vec![Paint::solid(Color::new(0.25, 0.65, 0.8, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Blended", 0.0, 0.0, 120.0, 96.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.transform = Affine::translate((85.0, 65.0)) * Affine::rotate(0.35);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.35, 0.2, 1.0))];
        let mut failures = Vec::new();
        for blend in [
            BlendMode::Darken,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::SoftLight,
            BlendMode::Exclusion,
            BlendMode::Hue,
            BlendMode::Saturation,
            BlendMode::Color,
            BlendMode::Luminosity,
        ] {
            for opacity in [0.25, 0.5, 0.75] {
                for blur_first in [true, false] {
                    let node = doc.node_mut(id).ok_or("shape missing")?;
                    node.blend = blend;
                    node.opacity = opacity;
                    let blur = figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
                    let noise = figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
                    node.effects = if blur_first { vec![blur, noise] } else { vec![noise, blur] };
                    for scale in [1.0, 2.0] {
                        let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (220.0 * scale) as u32 };
                        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{blend:?} opacity={opacity} capability");
                        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                        let mut gpu = GpuCanvas::new(view.width, view.height)?;
                        assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                        let actual = gpu.pixels()?;
                        if let Ok(prefix) = std::env::var("LIZARD_GPU_PARTIAL_BLEND_SCREENSHOT")
                            && blend == BlendMode::Screen
                            && (opacity - 0.5).abs() < 1e-6
                            && blur_first
                            && scale == 2.0
                        {
                            image::save_buffer(format!("{prefix}-gpu.png"), &actual, view.width, view.height, image::ColorType::Rgba8)?;
                            image::save_buffer(format!("{prefix}-cpu.png"), reference.data(), view.width, view.height, image::ColorType::Rgba8)?;
                        }
                        let changed = actual
                            .chunks_exact(4)
                            .zip(reference.data().chunks_exact(4))
                            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                            .count();
                        let severe = actual
                            .chunks_exact(4)
                            .zip(reference.data().chunks_exact(4))
                            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                            .count();
                        println!("{blend:?} opacity={opacity} blur_first={blur_first} {scale}x: changed={changed} severe={severe}");
                        if changed * 100 >= view.width as usize * view.height as usize * 10
                            || severe * 100 >= view.width as usize * view.height as usize
                        {
                            failures.push(format!("{blend:?} opacity={opacity} blur_first={blur_first} {scale}x changed={changed} severe={severe}"));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
        for blend in [BlendMode::ColorBurn, BlendMode::PlusLighter] {
            let node = doc.node_mut(id).ok_or("shape missing")?;
            node.blend = blend;
            node.opacity = 0.5;
            let view = View { origin: (0.0, 0.0), scale: 1.0, width: 260, height: 220 };
            assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{blend:?} partial opacity stays guarded");
        }
        Ok(())
    }

    #[test]
    fn group_blur_noise_stack_matches_reference() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU group effect graph");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Group", 25.0, 30.0, 150.0, 115.0)?;
        let first = doc.create(group, NodeKind::Rectangle, "Back", 5.0, 10.0, 100.0, 85.0)?;
        doc.node_mut(first).ok_or("first child missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.6, 0.9, 0.85))];
        let second = doc.create(group, NodeKind::Star { count: 5, inner_ratio: 0.45 }, "Front", 60.0, 0.0, 75.0, 95.0)?;
        doc.node_mut(second).ok_or("second child missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.3, 0.2, 0.75))];
        let mask = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Mask", 90.0, 68.0, 32.0, 32.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let masked = doc.create(group, NodeKind::Rectangle, "Masked", 85.0, 65.0, 50.0, 50.0)?;
        doc.node_mut(masked).ok_or("masked child missing")?.fills = vec![Paint::solid(Color::new(0.4, 0.8, 0.2, 0.9))];
        for blur_first in [true, false] {
            let blur = figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true };
            let noise = figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
            doc.node_mut(group).ok_or("group missing")?.effects = if blur_first { vec![blur, noise] } else { vec![noise, blur] };
            let group_node = doc.node(group).ok_or("group missing")?;
            assert!(gpu_isolated_grain_supported(&doc, group_node), "group source capability");
            assert!(gpu_stacked_blur_noise_supported(&doc, group_node), "group stack capability");
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (220.0 * scale) as u32, height: (190.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let changed = gpu
                    .pixels()?
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                assert!(
                    changed * 100 < view.width as usize * view.height as usize,
                    "group blur_first={blur_first} at {scale}x changed {changed} pixels"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn group_blur_noise_shadow_stack_uses_gpu_in_reference_order() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU mixed group effects");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Group", 30.0, 25.0, 160.0, 120.0)?;
        let back = doc.create(group, NodeKind::Rectangle, "Back", 5.0, 10.0, 105.0, 95.0)?;
        doc.node_mut(back).ok_or("back missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.6, 0.9, 0.85))];
        let front = doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Front", 65.0, 0.0, 75.0, 95.0)?;
        doc.node_mut(front).ok_or("front missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.3, 0.2, 0.75))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        let shadow = || {
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(shadow) = &mut effect.kind {
                shadow.radius = 8.0;
                shadow.offset = (4.0, 6.0);
                shadow.color = Color::new(0.2, 0.1, 0.3, 0.4);
            }
            effect
        };
        for (name, effects) in [
            ("blur-noise-shadow", vec![blur(), noise(), shadow()]),
            ("shadow-noise-blur", vec![shadow(), noise(), blur()]),
            ("blur-shadow", vec![blur(), shadow()]),
            ("noise-shadow", vec![noise(), shadow()]),
        ] {
            doc.node_mut(group).ok_or("group missing")?.effects = effects;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (240.0 * scale) as u32, height: (210.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} capability");
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                if let Ok(prefix) = std::env::var("LIZARD_GPU_ROTATED_STACK_SCREENSHOT") {
                    image::save_buffer(format!("{prefix}-group-{name}-{scale}-gpu.png"), &actual, view.width, view.height, image::ColorType::Rgba8)?;
                    image::save_buffer(
                        format!("{prefix}-group-{name}-{scale}-cpu.png"),
                        reference.data(),
                        view.width,
                        view.height,
                        image::ColorType::Rgba8,
                    )?;
                }
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("{name} {scale}x: changed={changed} severe={severe} pixels={}", view.width * view.height);
                assert!(changed * 100 < view.width as usize * view.height as usize * 2, "{name} at {scale}x changed {changed} pixels");
                assert!(severe * 100 < view.width as usize * view.height as usize, "{name} at {scale}x severe {severe} pixels");
            }
        }
        Ok(())
    }

    #[test]
    fn rotated_blur_shadow_stack_matches_reference() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU rotated mixed effects");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Star { count: 5, inner_ratio: 0.42 }, "Rotated star", 65.0, 55.0, 90.0, 90.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 1.0))];
        node.transform = Affine::translate((100.0, 100.0)) * Affine::rotate(0.5);
        let blur = figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true };
        let mut shadow = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(value) = &mut shadow.kind {
            value.radius = 8.0;
            value.offset = (4.0, 6.0);
            value.color = Color::new(0.2, 0.1, 0.3, 0.4);
        }
        for effects in [vec![blur.clone(), shadow.clone()], vec![shadow.clone(), blur.clone()]] {
            doc.node_mut(id).ok_or("shape missing")?.effects = effects;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (300.0 * scale) as u32, height: (280.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "GPU capability at {scale}x");
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let changed = gpu
                    .pixels()?
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                assert!(changed * 100 < view.width as usize * view.height as usize, "rotated stack scale={scale} changed={changed}");
            }
        }
        Ok(())
    }

    #[test]
    fn multiple_drop_shadows_preserve_order_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU multiple shadows");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Shadowed shape", 70.0, 55.0, 100.0, 80.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::WHITE)];
        let mut close = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(shadow) = &mut close.kind {
            shadow.radius = 5.0;
            shadow.offset = (3.0, 4.0);
            shadow.color = Color::new(0.1, 0.2, 0.3, 0.18);
        }
        let mut far = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(shadow) = &mut far.kind {
            shadow.radius = 12.0;
            shadow.offset = (-4.0, 7.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.14);
        }
        node.effects = vec![close, far];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 240, height: 200 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(view.width, view.height)?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let actual = gpu.pixels()?;
        let changed = actual
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        assert!(changed * 100 < view.width as usize * view.height as usize, "multiple shadows changed {changed} pixels");
        Ok(())
    }

    #[test]
    fn texture_scatter_uses_native_cache_at_one_and_two_times_zoom_including_rotation() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU texture levels");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Textured shape", 25.0, 22.0, 118.0, 97.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.8, 0.7, 0.6, 1.0))];
        node.effects = vec![figcraft_doc::Effect {
            kind: EffectKind::Texture(Texture { size: 2.0, size_y: None, radius: 3.0, clip_to_shape: false }),
            visible: true,
        }];
        for scale in [1.0, 2.0] {
            let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
            let mut gpu = GpuCanvas::new(view.width, view.height)?;
            gpu.prepare_static_effects(&doc)?;
            assert_eq!(gpu.texture_effects.len(), 1);
            assert_eq!(gpu.texture_effects_1x.len(), 1);
            gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
            let actual = gpu.pixels()?;
            let changed = actual
                .chunks_exact(4)
                .zip(reference.data().chunks_exact(4))
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                .count();
            assert_eq!(changed, 0, "texture differs at {scale}x");
        }
        for (angle, clipped) in [(0.4, false), (-0.7, true)] {
            let node = doc.node_mut(id).ok_or("shape missing")?;
            node.transform = Affine::translate((170.0, 170.0)) * Affine::rotate(angle);
            if let EffectKind::Texture(texture) = &mut node.effects[0].kind {
                texture.clip_to_shape = clipped;
            }
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (384.0 * scale) as u32, height: (384.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let painted = reference.data().chunks_exact(4).filter(|pixel| pixel[..3] != [245, 245, 245]).count();
                assert!(painted > 1000, "rotated texture fixture must be visible");
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.prepare_static_effects(&doc)?;
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                assert_eq!(changed, 0, "rotated texture differs at {scale}x, angle {angle}, clipped {clipped}");
            }
        }
        if let EffectKind::Texture(texture) = &mut doc.node_mut(id).ok_or("shape missing")?.effects[0].kind {
            texture.size = 0.5;
            texture.radius = 4.0;
        }
        for scale in [1.0, 2.0] {
            let view = View { origin: (0.5, 0.5), scale, width: (384.0 * scale) as u32, height: (384.0 * scale) as u32 };
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
            let mut gpu = GpuCanvas::new(view.width, view.height)?;
            gpu.prepare_static_effects(&doc)?;
            gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
            let actual = gpu.pixels()?;
            let changed = actual
                .chunks_exact(4)
                .zip(reference.data().chunks_exact(4))
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                .count();
            // The scatter stays in the same place and preserves its coverage; interpolation
            // during fractional pan may legitimately soften individual grains.
            assert!(changed * 100 < view.width as usize * view.height as usize * 5, "fractional texture changed {changed} pixels at {scale}x");
        }
        Ok(())
    }

    #[test]
    fn texture_scatter_stays_on_gpu_during_fractional_zoom() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU texture fractional zoom");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Rectangle, "Textured shape", 30.0, 25.0, 120.0, 90.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.fills = vec![Paint::solid(Color::new(0.8, 0.7, 0.6, 1.0))];
        node.effects = vec![figcraft_doc::Effect {
            kind: EffectKind::Texture(Texture { size: 2.0, size_y: None, radius: 3.0, clip_to_shape: false }),
            visible: true,
        }];
        for angle in [0.0, 0.4] {
            doc.node_mut(id).ok_or("shape missing")?.transform = Affine::translate((30.0, 25.0)) * Affine::rotate(angle);
            for scale in [0.75, 1.25, 1.5] {
                let view = View { origin: (0.0, 0.0), scale, width: (230.0 * scale) as u32, height: (180.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "GPU capability at {scale}x, angle {angle}");
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.prepare_static_effects(&doc)?;
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                eprintln!("fractional texture scale={scale} angle={angle} pixels={} changed={changed}", view.width as usize * view.height as usize);
                assert!(
                    changed * 100 < view.width as usize * view.height as usize * 5,
                    "fractional texture changed {changed} pixels at {scale}x, angle {angle}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn texture_with_blur_or_noise_orders_use_gpu_effect_cache() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU texture blur stack");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Textured shape", 0.0, 0.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("shape missing")?;
        node.transform = Affine::translate((75.0, 60.0)) * Affine::rotate(0.36);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let texture = || figcraft_doc::Effect {
            kind: EffectKind::Texture(Texture { size: 2.0, size_y: None, radius: 3.0, clip_to_shape: false }),
            visible: true,
        };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        let shadow = || {
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(value) = &mut effect.kind {
                value.radius = 8.0;
                value.offset = (4.0, 6.0);
                value.color = Color::new(0.0, 0.0, 0.0, 0.4);
            }
            effect
        };
        let shadow2 = || {
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(value) = &mut effect.kind {
                value.radius = 4.0;
                value.offset = (-3.0, 2.0);
                value.color = Color::new(0.14, 0.25, 0.72, 0.55);
            }
            effect
        };
        let spread_shadow = |spread| {
            let mut effect = shadow();
            if let EffectKind::DropShadow(value) = &mut effect.kind {
                value.spread = spread;
            }
            effect
        };
        let mut cases = vec![
            ("blur-texture", vec![blur(), texture()]),
            ("texture-blur", vec![texture(), blur()]),
            ("noise-texture", vec![noise(), texture()]),
            ("texture-noise", vec![texture(), noise()]),
            ("texture-shadow", vec![texture(), shadow()]),
            ("shadow-texture", vec![shadow(), texture()]),
            ("blur-texture-shadow", vec![blur(), texture(), shadow()]),
            ("blur-shadow-texture", vec![blur(), shadow(), texture()]),
            ("texture-blur-shadow", vec![texture(), blur(), shadow()]),
            ("texture-shadow-blur", vec![texture(), shadow(), blur()]),
            ("shadow-blur-texture", vec![shadow(), blur(), texture()]),
            ("shadow-texture-blur", vec![shadow(), texture(), blur()]),
            ("noise-texture-shadow", vec![noise(), texture(), shadow()]),
            ("noise-shadow-texture", vec![noise(), shadow(), texture()]),
            ("texture-noise-shadow", vec![texture(), noise(), shadow()]),
            ("texture-shadow-noise", vec![texture(), shadow(), noise()]),
            ("shadow-noise-texture", vec![shadow(), noise(), texture()]),
            ("shadow-texture-noise", vec![shadow(), texture(), noise()]),
            ("blur-noise-texture", vec![blur(), noise(), texture()]),
            ("blur-texture-noise", vec![blur(), texture(), noise()]),
            ("noise-blur-texture", vec![noise(), blur(), texture()]),
            ("noise-texture-blur", vec![noise(), texture(), blur()]),
            ("texture-blur-noise", vec![texture(), blur(), noise()]),
            ("texture-noise-blur", vec![texture(), noise(), blur()]),
            ("blur-noise-texture-shadow-blur", vec![blur(), noise(), texture(), shadow(), blur()]),
            ("shadow-texture-noise-blur-noise", vec![shadow(), texture(), noise(), blur(), noise()]),
            ("blur-shadow-noise-texture-blur-noise", vec![blur(), shadow(), noise(), texture(), blur(), noise()]),
            ("texture-shadow-shadow", vec![texture(), shadow(), shadow2()]),
            ("shadow-texture-shadow", vec![shadow(), texture(), shadow2()]),
            ("shadow-shadow-texture", vec![shadow(), shadow2(), texture()]),
            ("blur-texture-shadow-noise-shadow", vec![blur(), texture(), shadow(), noise(), shadow2()]),
            ("texture-positive-spread-shadow", vec![texture(), spread_shadow(8.0)]),
            ("negative-spread-shadow-texture", vec![spread_shadow(-8.0), texture()]),
            ("blur-texture-positive-spread-shadow", vec![blur(), texture(), spread_shadow(16.0)]),
            ("texture-max-positive-spread-shadow", vec![texture(), spread_shadow(32.0)]),
            ("texture-max-negative-spread-shadow", vec![texture(), spread_shadow(-32.0)]),
        ]
        .into_iter()
        .map(|(name, effects)| (name.to_owned(), effects))
        .collect::<Vec<_>>();
        let effect_names = ["blur", "noise", "texture", "shadow"];
        for first in 0..4 {
            for second in 0..4 {
                for third in 0..4 {
                    for fourth in 0..4 {
                        let indices = [first, second, third, fourth];
                        if indices.iter().enumerate().any(|(position, index)| indices[..position].contains(index)) {
                            continue;
                        }
                        let name = indices.map(|index| effect_names[index]).join("-");
                        let effects = indices
                            .map(|index| match index {
                                0 => blur(),
                                1 => noise(),
                                2 => texture(),
                                _ => shadow(),
                            })
                            .to_vec();
                        cases.push((name, effects));
                    }
                }
            }
        }
        for (name, effects) in cases {
            doc.node_mut(id).ok_or("shape missing")?.effects = effects;
            for opacity in [1.0, 0.25, 0.5, 0.75] {
                doc.node_mut(id).ok_or("shape missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} rejected at {scale}x");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut plain_doc = doc.clone();
                    plain_doc.node_mut(id).ok_or("plain shape missing")?.effects.clear();
                    let plain = render_view(&plain_doc, page, &view, &Options::default()).ok_or("plain reference render failed")?;
                    let effect_pixels = reference
                        .data()
                        .chunks_exact(4)
                        .zip(plain.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 4))
                        .count();
                    assert!(effect_pixels > 100, "{name} at {scale}x must visibly change the shape");
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    gpu.prepare_static_effects(&doc)?;
                    assert_eq!(gpu.layer_blurs.len(), 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("{name} opacity={opacity} at {scale}x: changed={changed} severe={severe}");
                    assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                    assert!(severe * 100 < view.width as usize * view.height as usize);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_rotated_texture_keeps_scatter_and_alpha() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent texture");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 260.0, 240.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.13, 0.55, 0.71, 1.0))];
        let id = doc.create(page, NodeKind::Rectangle, "Texture", 0.0, 0.0, 120.0, 84.0)?;
        let node = doc.node_mut(id).ok_or("texture missing")?;
        node.transform = Affine::translate((75.0, 60.0)) * Affine::rotate(0.36);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.45, 0.28, 1.0))];
        node.effects = vec![figcraft_doc::Effect {
            kind: EffectKind::Texture(Texture { size: 2.0, size_y: None, radius: 3.0, clip_to_shape: false }),
            visible: true,
        }];
        for opacity in [0.25, 0.5, 0.75] {
            doc.node_mut(id).ok_or("texture missing")?.opacity = opacity;
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (240.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.prepare_static_effects(&doc)?;
                assert_eq!(gpu.texture_effects.len(), 1);
                assert_eq!(gpu.texture_effects_1x.len(), 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                assert!(severe * 100 < view.width as usize * view.height as usize);
            }
        }
        Ok(())
    }

    #[test]
    fn vector_texture_effect_stacks_preserve_faces_and_order() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU vector texture stack");
        let page = doc.current_page.ok_or("page missing")?;
        let network = figcraft_doc::VectorNetwork::from_points(
            &[Point::new(10.0, 10.0), Point::new(140.0, 25.0), Point::new(95.0, 65.0), Point::new(110.0, 150.0), Point::new(20.0, 140.0)],
            true,
        );
        let id = doc.create(page, NodeKind::Vector { network }, "Vector", 35.0, 30.0, 140.0, 150.0)?;
        let node = doc.node_mut(id).ok_or("vector missing")?;
        node.transform = Affine::translate((35.0, 30.0)) * Affine::rotate(0.2);
        node.fills = vec![Paint::solid(Color::new(0.8, 0.5, 0.3, 0.8))];
        let texture = || figcraft_doc::Effect {
            kind: EffectKind::Texture(Texture { size: 2.0, size_y: None, radius: 3.0, clip_to_shape: false }),
            visible: true,
        };
        let blur = || figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(6.0)), visible: true };
        let noise = || figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        for (name, effects) in [("texture-blur", vec![texture(), blur()]), ("blur-noise-texture", vec![blur(), noise(), texture()])] {
            doc.node_mut(id).ok_or("vector missing")?.effects = effects;
            for opacity in [1.0, 0.5] {
                doc.node_mut(id).ok_or("vector missing")?.opacity = opacity;
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (260.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} capability");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut plain_doc = doc.clone();
                    plain_doc.node_mut(id).ok_or("plain vector missing")?.effects.clear();
                    let plain = render_view(&plain_doc, page, &view, &Options::default()).ok_or("plain reference render failed")?;
                    let effect_pixels = reference
                        .data()
                        .chunks_exact(4)
                        .zip(plain.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 4))
                        .count();
                    assert!(effect_pixels > 100, "{name} at {scale}x must visibly change the vector");
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    let severe = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                        .count();
                    println!("vector {name} opacity={opacity} scale={scale}: changed={changed} severe={severe}");
                    assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                    assert!(severe * 100 < view.width as usize * view.height as usize);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_vector_without_shadow_matches_reference() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Translucent vector baseline");
        let page = doc.current_page.ok_or("new document has no page")?;
        let network = figcraft_doc::VectorNetwork::from_points(
            &[Point::new(10.0, 10.0), Point::new(140.0, 25.0), Point::new(95.0, 65.0), Point::new(110.0, 150.0), Point::new(20.0, 140.0)],
            true,
        );
        let id = doc.create(page, NodeKind::Vector { network }, "Vector", 35.0, 30.0, 140.0, 150.0)?;
        doc.node_mut(id).ok_or("vector missing")?.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, 0.5))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 256, height: 256 };
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(view.width, view.height)?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let actual = gpu.pixels()?;
        let changed = actual
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        assert!(changed * 100 < view.width as usize * view.height as usize, "translucent fill changed {changed} pixels");
        Ok(())
    }

    #[test]
    fn multiple_opaque_path_shadows_match_reference() -> Result<(), Box<dyn Error>> {
        for kind in [
            NodeKind::Polygon { count: 6 },
            NodeKind::Star { count: 5, inner_ratio: 0.4 },
            NodeKind::Vector {
                network: figcraft_doc::VectorNetwork::from_points(
                    &[Point::new(10.0, 10.0), Point::new(120.0, 20.0), Point::new(105.0, 130.0), Point::new(20.0, 125.0)],
                    true,
                ),
            },
        ] {
            let mut doc = Document::new("GPU multiple path shadows");
            let page = doc.current_page.ok_or("new document has no page")?;
            let id = doc.create(page, kind.clone(), "Shape", 40.0, 35.0, 130.0, 140.0)?;
            let node = doc.node_mut(id).ok_or("shape missing")?;
            node.fills = vec![Paint::solid(Color::WHITE)];
            let mut close = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(shadow) = &mut close.kind {
                shadow.radius = 5.0;
                shadow.offset = (3.0, 4.0);
                shadow.color = Color::new(0.1, 0.2, 0.3, 0.18);
            }
            let mut far = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(shadow) = &mut far.kind {
                shadow.radius = 12.0;
                shadow.offset = (-4.0, 7.0);
                shadow.color = Color::new(0.2, 0.1, 0.3, 0.14);
            }
            node.effects = vec![close, far];
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (240.0 * scale) as u32, height: (220.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                println!("multiple {kind:?} shadows scale={scale} changed_gt8={:.3}%", changed as f64 * 100.0 / f64::from(view.width * view.height));
                assert!(changed * 100 < view.width as usize * view.height as usize, "multiple {kind:?} shadows changed {changed} pixels");
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_vector_shadow_clips_to_silhouette_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU translucent vector shadow");
        let page = doc.current_page.ok_or("new document has no page")?;
        let network = figcraft_doc::VectorNetwork::from_points(
            &[Point::new(10.0, 10.0), Point::new(140.0, 25.0), Point::new(95.0, 65.0), Point::new(110.0, 150.0), Point::new(20.0, 140.0)],
            true,
        );
        let id = doc.create(page, NodeKind::Vector { network }, "Vector", 35.0, 30.0, 140.0, 150.0)?;
        let node = doc.node_mut(id).ok_or("vector missing")?;
        node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, 0.5))];
        let mut effect = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(shadow) = &mut effect.kind {
            shadow.radius = 16.0;
            shadow.offset = (5.0, 9.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
        }
        node.effects = vec![effect];
        for alpha in [0.25, 0.5, 0.75] {
            doc.node_mut(id).ok_or("vector missing")?.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, alpha))];
            for show_behind in [false, true] {
                if let Some(node) = doc.node_mut(id)
                    && let EffectKind::DropShadow(shadow) = &mut node.effects[0].kind
                {
                    shadow.show_behind = show_behind;
                }
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.1, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let mut changed = 0usize;
                    let mut error = 0u64;
                    for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
                        let deltas = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
                        changed += usize::from(deltas.iter().any(|delta| *delta > 8));
                        error += deltas.iter().map(|delta| u64::from(*delta)).sum::<u64>();
                    }
                    println!(
                        "translucent vector shadow alpha={alpha} behind={show_behind} scale={scale} mean_rgb={:.3}/255 changed_gt8={:.3}%",
                        error as f64 / (f64::from(view.width * view.height) * 3.0),
                        changed as f64 * 100.0 / f64::from(view.width * view.height)
                    );
                    assert!(
                        changed * 100 < view.width as usize * view.height as usize,
                        "translucent shadow alpha={alpha} behind={show_behind} scale={scale} changed {changed} pixels"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn translucent_shape_drop_shadows_match_reference() -> Result<(), Box<dyn Error>> {
        for kind in [
            NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 },
            NodeKind::Polygon { count: 6 },
            NodeKind::Star { count: 5, inner_ratio: 0.4 },
        ] {
            let mut doc = Document::new("GPU translucent shape shadow");
            let page = doc.current_page.ok_or("new document has no page")?;
            let id = doc.create(page, kind.clone(), "Shape", 35.0, 30.0, 140.0, 150.0)?;
            let node = doc.node_mut(id).ok_or("shape missing")?;
            node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, 0.5))];
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(shadow) = &mut effect.kind {
                shadow.radius = 12.0;
                shadow.offset = (5.0, 7.0);
                shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
            }
            node.effects = vec![effect];
            for show_behind in [false, true] {
                if let Some(node) = doc.node_mut(id)
                    && let EffectKind::DropShadow(shadow) = &mut node.effects[0].kind
                {
                    shadow.show_behind = show_behind;
                }
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert_eq!(gpu.prepare_static_effects(&doc)?.1, 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let mut changed = 0usize;
                    let mut error = 0u64;
                    for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
                        let deltas = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
                        changed += usize::from(deltas.iter().any(|delta| *delta > 8));
                        error += deltas.iter().map(|delta| u64::from(*delta)).sum::<u64>();
                    }
                    println!(
                        "translucent {kind:?} shadow behind={show_behind} scale={scale} mean_rgb={:.3}/255 changed_gt8={:.3}%",
                        error as f64 / (f64::from(view.width * view.height) * 3.0),
                        changed as f64 * 100.0 / f64::from(view.width * view.height)
                    );
                    assert!(changed * 100 < view.width as usize * view.height as usize, "translucent {kind:?} shadow changed {changed} pixels");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn vector_inner_shadow_matches_reference_at_multiple_opacities() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU vector inner shadow");
        let page = doc.current_page.ok_or("new document has no page")?;
        let network = figcraft_doc::VectorNetwork::from_points(
            &[Point::new(10.0, 10.0), Point::new(140.0, 25.0), Point::new(95.0, 65.0), Point::new(110.0, 150.0), Point::new(20.0, 140.0)],
            true,
        );
        let id = doc.create(page, NodeKind::Vector { network }, "Vector", 35.0, 30.0, 140.0, 150.0)?;
        let node = doc.node_mut(id).ok_or("vector missing")?;
        node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, 1.0))];
        let mut effect = figcraft_doc::Effect::inner_shadow();
        if let EffectKind::InnerShadow(shadow) = &mut effect.kind {
            shadow.radius = 12.0;
            shadow.offset = (5.0, 7.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
        }
        node.effects = vec![effect];
        for alpha in [1.0, 0.5] {
            doc.node_mut(id).ok_or("vector missing")?.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, alpha))];
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert_eq!(gpu.prepare_static_effects(&doc)?.2, 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let mut changed = 0usize;
                let mut error = 0u64;
                for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
                    let deltas = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
                    changed += usize::from(deltas.iter().any(|delta| *delta > 8));
                    error += deltas.iter().map(|delta| u64::from(*delta)).sum::<u64>();
                }
                println!(
                    "vector inner shadow alpha={alpha} scale={scale} mean_rgb={:.3}/255 changed_gt8={:.3}%",
                    error as f64 / (f64::from(view.width * view.height) * 3.0),
                    changed as f64 * 100.0 / f64::from(view.width * view.height)
                );
                assert!(
                    changed * 100 < view.width as usize * view.height as usize,
                    "vector inner shadow alpha={alpha} differs at {scale}x: {changed} pixels"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn ellipse_inner_shadow_matches_reference() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU ellipse inner shadow");
        let page = doc.current_page.ok_or("new document has no page")?;
        let id = doc.create(page, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Ellipse", 35.0, 30.0, 140.0, 150.0)?;
        let node = doc.node_mut(id).ok_or("ellipse missing")?;
        node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, 1.0))];
        let mut effect = figcraft_doc::Effect::inner_shadow();
        if let EffectKind::InnerShadow(shadow) = &mut effect.kind {
            shadow.radius = 12.0;
            shadow.offset = (5.0, 7.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
        }
        node.effects = vec![effect];
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 512, height: 512 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(view.width, view.height)?;
        assert_eq!(gpu.prepare_static_effects(&doc)?.2, 1);
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let changed = gpu
            .pixels()?
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        assert!(changed * 100 < view.width as usize * view.height as usize, "ellipse inner shadow changed {changed} pixels");
        Ok(())
    }

    #[test]
    fn polygon_and_star_inner_shadows_match_reference() -> Result<(), Box<dyn Error>> {
        for kind in [NodeKind::Polygon { count: 6 }, NodeKind::Star { count: 5, inner_ratio: 0.4 }] {
            let mut doc = Document::new("GPU polygon inner shadow");
            let page = doc.current_page.ok_or("new document has no page")?;
            let id = doc.create(page, kind, "Shape", 35.0, 30.0, 140.0, 150.0)?;
            let node = doc.node_mut(id).ok_or("shape missing")?;
            node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, 1.0))];
            let mut effect = figcraft_doc::Effect::inner_shadow();
            if let EffectKind::InnerShadow(shadow) = &mut effect.kind {
                shadow.radius = 12.0;
                shadow.offset = (5.0, 7.0);
                shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
            }
            node.effects = vec![effect];
            let view = View { origin: (0.0, 0.0), scale: 2.0, width: 512, height: 512 };
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
            let mut gpu = GpuCanvas::new(view.width, view.height)?;
            assert_eq!(gpu.prepare_static_effects(&doc)?.2, 1);
            gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
            let changed = gpu
                .pixels()?
                .chunks_exact(4)
                .zip(reference.data().chunks_exact(4))
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                .count();
            assert!(changed * 100 < view.width as usize * view.height as usize, "inner shadow changed {changed} pixels");
        }
        Ok(())
    }

    #[test]
    fn opaque_path_drop_shadows_match_reference() -> Result<(), Box<dyn Error>> {
        for kind in [
            NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 },
            NodeKind::Polygon { count: 6 },
            NodeKind::Star { count: 5, inner_ratio: 0.4 },
        ] {
            let mut doc = Document::new("GPU path drop shadow");
            let page = doc.current_page.ok_or("new document has no page")?;
            let id = doc.create(page, kind, "Shape", 35.0, 30.0, 140.0, 150.0)?;
            let node = doc.node_mut(id).ok_or("shape missing")?;
            node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, 1.0))];
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(shadow) = &mut effect.kind {
                shadow.radius = 12.0;
                shadow.offset = (5.0, 7.0);
                shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
                shadow.spread = 2.0;
            }
            node.effects = vec![effect];
            for spread in [0.0, 3.0, 10.0, 20.0, 32.0] {
                doc.node_mut(id).ok_or("shape missing")?.transform =
                    if spread == 32.0 { Affine::translate((85.0, 80.0)) * Affine::rotate(0.4) } else { Affine::translate((35.0, 30.0)) };
                if let Some(EffectKind::DropShadow(shadow)) =
                    doc.node_mut(id).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind)
                {
                    shadow.spread = spread;
                }
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_in_view(&doc, &view));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let changed = gpu
                        .pixels()?
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    assert!(changed * 100 < view.width as usize * view.height as usize, "spread={spread} scale={scale} changed {changed} pixels");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn text_drop_shadow_without_spread_matches_reference() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU text shadow");
        let page = doc.current_page.ok_or("new document has no page")?;
        let mut text = figcraft_doc::TextData::new("Lizard");
        text.style.font_size = 48.0;
        let id = doc.create(page, NodeKind::Text(text), "Text", 25.0, 35.0, 180.0, 70.0)?;
        let node = doc.node_mut(id).ok_or("text missing")?;
        node.fills = vec![Paint::solid(Color::WHITE)];
        let mut effect = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(shadow) = &mut effect.kind {
            shadow.spread = 0.0;
            shadow.radius = 8.0;
            shadow.offset = (3.0, 5.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
        }
        node.effects = vec![effect];
        if draw::node_outline(&doc, doc.node(id).ok_or("text missing")?).is_none() {
            return Ok(());
        }
        for scale in [1.0, 2.0] {
            let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (150.0 * scale) as u32 };
            assert!(!has_unhandled_effects_in_view(&doc, &view));
            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
            let mut gpu = GpuCanvas::new(view.width, view.height)?;
            gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
            let changed = gpu
                .pixels()?
                .chunks_exact(4)
                .zip(reference.data().chunks_exact(4))
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                .count();
            assert!(changed * 100 < view.width as usize * view.height as usize, "text shadow at {scale}x changed {changed} pixels");
        }
        Ok(())
    }

    #[test]
    fn styled_text_run_fills_render_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU styled text runs");
        let page = doc.current_page.ok_or("page missing")?;
        let mut text = figcraft_doc::TextData::new("Lizard");
        text.style.font_size = 48.0;
        let mut style = text.style.clone();
        style.fills = Some(vec![Paint::solid(Color::new(0.1, 0.3, 0.9, 1.0))]);
        text.runs.push(figcraft_doc::StyleRun { start: 3, end: 6, style });
        let id = doc.create(page, NodeKind::Text(text), "Text", 25.0, 35.0, 220.0, 70.0)?;
        doc.node_mut(id).ok_or("text missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.2, 0.1, 1.0))];
        let Some((layout, _)) = figcraft_text::layout_node(doc.node(id).ok_or("text missing")?) else { return Ok(()) };
        if layout.glyphs.is_empty() {
            return Ok(());
        }
        for styled_run in [true, false] {
            if !styled_run && let NodeKind::Text(text) = &mut doc.node_mut(id).ok_or("text missing")?.kind {
                text.runs.clear();
                text.style.fills = Some(vec![Paint::solid(Color::new(0.1, 0.3, 0.9, 1.0))]);
            }
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (260.0 * scale) as u32, height: (150.0 * scale) as u32 };
                assert!(!has_unhandled_effects_in_view(&doc, &view));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                let severe = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 48))
                    .count();
                println!("styled text run={styled_run} at {scale}x: changed={changed} severe={severe}");
                assert!(changed * 100 < view.width as usize * view.height as usize * 10);
                assert!(severe * 100 < view.width as usize * view.height as usize);
            }
        }
        Ok(())
    }

    #[test]
    fn styled_text_run_with_unsupported_blend_keeps_fallback() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Unsupported styled run blend");
        let page = doc.current_page.ok_or("page missing")?;
        let mut text = figcraft_doc::TextData::new("Lizard");
        text.style.font_size = 48.0;
        let mut paint = Paint::solid(Color::new(0.1, 0.3, 0.9, 1.0));
        paint.blend = BlendMode::PlusDarker;
        let mut style = text.style.clone();
        style.fills = Some(vec![paint]);
        text.runs.push(figcraft_doc::StyleRun { start: 0, end: 6, style });
        let id = doc.create(page, NodeKind::Text(text), "Text", 25.0, 35.0, 220.0, 70.0)?;
        doc.node_mut(id).ok_or("text missing")?.fills = vec![Paint::solid(Color::WHITE)];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 260, height: 150 };
        assert!(has_unhandled_effects_in_view(&doc, &view));
        Ok(())
    }

    #[test]
    fn stroked_shape_drop_shadow_uses_gpu_cache() -> Result<(), Box<dyn Error>> {
        for kind in [
            NodeKind::Star { count: 5, inner_ratio: 0.4 },
            NodeKind::Line,
            NodeKind::Vector {
                network: figcraft_doc::VectorNetwork::from_points(
                    &[
                        Point::new(8.0, 10.0),
                        Point::new(110.0, 5.0),
                        Point::new(130.0, 100.0),
                        Point::new(70.0, 70.0),
                        Point::new(20.0, 125.0),
                        Point::new(50.0, 60.0),
                    ],
                    true,
                ),
            },
        ] {
            let mut doc = Document::new("GPU stroked shadow");
            let page = doc.current_page.ok_or("new document has no page")?;
            let is_line = matches!(kind, NodeKind::Line);
            let id = doc.create(page, kind, "Stroked shape", 45.0, 35.0, 140.0, 130.0)?;
            let node = doc.node_mut(id).ok_or("shape missing")?;
            node.fills = if is_line { Vec::new() } else { vec![Paint::solid(Color::new(0.9, 0.9, 0.95, 1.0))] };
            node.strokes = vec![Paint::solid(Color::new(0.2, 0.3, 0.5, 1.0))];
            node.stroke.weight = 6.0;
            node.stroke.align = figcraft_doc::StrokeAlign::Outside;
            let mut effect = figcraft_doc::Effect::drop_shadow();
            if let EffectKind::DropShadow(shadow) = &mut effect.kind {
                shadow.radius = 12.0;
                shadow.offset = (5.0, 7.0);
                shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
            }
            node.effects = vec![effect];
            for show_behind in [false, true] {
                if let Some(EffectKind::DropShadow(shadow)) =
                    doc.node_mut(id).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind)
                {
                    shadow.show_behind = show_behind;
                }
                for spread in [0.0, 10.0, 20.0, 32.0] {
                    if let Some(EffectKind::DropShadow(shadow)) =
                        doc.node_mut(id).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind)
                    {
                        shadow.spread = spread;
                    }
                    doc.node_mut(id).ok_or("shape missing")?.stroke.dashes = if spread == 0.0 { vec![8.0, 4.0] } else { Vec::new() };
                    for scale in [1.0, 2.0] {
                        let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                        let mut gpu = GpuCanvas::new(view.width, view.height)?;
                        assert!(gpu.prepare_static_effects(&doc)?.0 >= 1);
                        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                        let changed = gpu
                            .pixels()?
                            .chunks_exact(4)
                            .zip(reference.data().chunks_exact(4))
                            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                            .count();
                        assert!(
                            changed * 100 < view.width as usize * view.height as usize,
                            "stroked shadow behind={show_behind} spread={spread} scale={scale} changed {changed} pixels"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn simple_group_inner_shadow_uses_composited_gpu_cache() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU group inner shadow");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Group", 30.0, 25.0, 165.0, 130.0)?;
        for (x, y, w, h, color) in
            [(5.0, 5.0, 90.0, 110.0, Color::new(0.8, 0.8, 0.85, 1.0)), (70.0, 20.0, 75.0, 80.0, Color::new(0.3, 0.6, 0.8, 0.5))]
        {
            let child = doc.create(group, NodeKind::Rectangle, "Child", x, y, w, h)?;
            doc.node_mut(child).ok_or("child missing")?.fills = vec![Paint::solid(color)];
        }
        let outlined = doc.create(group, NodeKind::Rectangle, "Outlined", 110.0, 0.0, 30.0, 20.0)?;
        let stroke_child = doc.node_mut(outlined).ok_or("outlined child missing")?;
        stroke_child.fills.clear();
        stroke_child.strokes = vec![Paint::solid(Color::WHITE)];
        stroke_child.stroke.weight = 4.0;
        stroke_child.stroke.align = figcraft_doc::StrokeAlign::Outside;
        stroke_child.stroke.dashes = vec![4.0, 2.0];
        stroke_child.opacity = 0.7;
        let nested = doc.create(group, NodeKind::Group, "Nested", 100.0, 85.0, 30.0, 30.0)?;
        doc.node_mut(nested).ok_or("nested group missing")?.opacity = 0.6;
        let dot = doc.create(nested, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Dot", 0.0, 0.0, 28.0, 28.0)?;
        doc.node_mut(dot).ok_or("dot missing")?.fills = vec![Paint::solid(Color::new(0.4, 0.2, 0.7, 0.8))];
        let gradient_child =
            doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Gradient", 45.0, 42.0, 48.0, 48.0)?;
        doc.node_mut(gradient_child).ok_or("gradient child missing")?.fills =
            vec![Paint::gradient(figcraft_color::Gradient::default_for(GradientKind::Linear, Color::new(0.2, 0.7, 0.6, 1.0)))];
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 95.0, 25.0, 25.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let masked = doc.create(group, NodeKind::Rectangle, "Masked", 0.0, 95.0, 55.0, 25.0)?;
        doc.node_mut(masked).ok_or("masked child missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.2, 0.2, 1.0))];
        let complex_mask = doc.create(group, NodeKind::Group, "Vector mask group", 92.0, 95.0, 42.0, 32.0)?;
        let mask_part =
            doc.create(complex_mask, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Mask part", 0.0, 0.0, 35.0, 30.0)?;
        doc.node_mut(mask_part).ok_or("mask part missing")?.fills = vec![Paint::solid(Color::WHITE)];
        let second_mask_part = doc.create(complex_mask, NodeKind::Rectangle, "Rotated mask part", 14.0, 8.0, 18.0, 18.0)?;
        let second_mask_node = doc.node_mut(second_mask_part).ok_or("second mask part missing")?;
        second_mask_node.fills = vec![Paint::solid(Color::WHITE)];
        second_mask_node.transform = Affine::translate((14.0, 8.0)) * Affine::rotate(0.4);
        let mask_node = doc.node_mut(complex_mask).ok_or("mask group missing")?;
        mask_node.is_mask = true;
        mask_node.mask_type = MaskType::Alpha;
        let masked_by_group = doc.create(group, NodeKind::Rectangle, "Masked by group", 90.0, 95.0, 50.0, 30.0)?;
        doc.node_mut(masked_by_group).ok_or("masked by group missing")?.fills = vec![Paint::solid(Color::new(0.1, 0.7, 0.5, 0.9))];
        let mut effect = figcraft_doc::Effect::inner_shadow();
        if let EffectKind::InnerShadow(shadow) = &mut effect.kind {
            shadow.radius = 12.0;
            shadow.offset = (5.0, 7.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
        }
        doc.node_mut(group).ok_or("group missing")?.effects = vec![effect];
        for spread in [0.0_f64, 2.0, -2.0, 8.0, -8.0, 16.0, -16.0, 32.0, -32.0] {
            if let Some(EffectKind::InnerShadow(shadow)) =
                doc.node_mut(group).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind)
            {
                shadow.spread = spread;
            }
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                assert!(gpu.prepare_static_effects(&doc)?.0 >= 1);
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let mut changed = 0usize;
                let mut error = 0u64;
                for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
                    let deltas = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
                    changed += usize::from(deltas.iter().any(|delta| *delta > 8));
                    error += deltas.iter().map(|delta| u64::from(*delta)).sum::<u64>();
                }
                println!(
                    "group inner shadow spread={spread} scale={scale} mean_rgb={:.3}/255 changed_gt8={:.3}%",
                    error as f64 / (f64::from(view.width * view.height) * 3.0),
                    changed as f64 * 100.0 / f64::from(view.width * view.height)
                );
                assert!(changed * 100 < view.width as usize * view.height as usize, "group inner shadow differs at {scale}x: {changed} pixels");
            }
        }
        doc.node_mut(group).ok_or("group missing")?.transform = Affine::translate((65.0, 45.0)) * Affine::rotate(0.35);
        for spread in [-32.0, -16.0, -8.0, 0.0, 8.0, 16.0, 32.0] {
            if let Some(EffectKind::InnerShadow(shadow)) =
                doc.node_mut(group).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind)
            {
                shadow.spread = spread;
            }
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                let mut gpu = GpuCanvas::new(view.width, view.height)?;
                gpu.prepare_static_effects(&doc)?;
                gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                let actual = gpu.pixels()?;
                let changed = actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                    .count();
                assert!(
                    changed * 100 < view.width as usize * view.height as usize,
                    "rotated group inner shadow spread={spread} scale={scale} differs: {changed} pixels"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn simple_group_drop_shadow_uses_composited_gpu_cache() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU group drop shadow");
        let page = doc.current_page.ok_or("new document has no page")?;
        let group = doc.create(page, NodeKind::Group, "Group", 30.0, 25.0, 165.0, 130.0)?;
        for (x, y, w, h, color) in
            [(5.0, 5.0, 90.0, 110.0, Color::new(0.8, 0.8, 0.85, 1.0)), (70.0, 20.0, 75.0, 80.0, Color::new(0.3, 0.6, 0.8, 0.5))]
        {
            let child = doc.create(group, NodeKind::Rectangle, "Child", x, y, w, h)?;
            doc.node_mut(child).ok_or("child missing")?.fills = vec![Paint::solid(color)];
        }
        let outlined = doc.create(group, NodeKind::Rectangle, "Outlined", 110.0, 0.0, 30.0, 20.0)?;
        let stroke_child = doc.node_mut(outlined).ok_or("outlined child missing")?;
        stroke_child.fills.clear();
        stroke_child.strokes = vec![Paint::solid(Color::WHITE)];
        stroke_child.stroke.weight = 4.0;
        stroke_child.stroke.align = figcraft_doc::StrokeAlign::Outside;
        stroke_child.stroke.dashes = vec![4.0, 2.0];
        let nested = doc.create(group, NodeKind::Group, "Nested", 100.0, 85.0, 30.0, 30.0)?;
        doc.node_mut(outlined).ok_or("outlined child missing")?.opacity = 0.7;
        doc.node_mut(nested).ok_or("nested group missing")?.opacity = 0.6;
        let dot = doc.create(nested, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Dot", 0.0, 0.0, 28.0, 28.0)?;
        doc.node_mut(dot).ok_or("dot missing")?.fills = vec![Paint::solid(Color::new(0.4, 0.2, 0.7, 0.8))];
        let gradient_child =
            doc.create(group, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Gradient", 45.0, 42.0, 48.0, 48.0)?;
        doc.node_mut(gradient_child).ok_or("gradient child missing")?.fills =
            vec![Paint::gradient(figcraft_color::Gradient::default_for(GradientKind::Linear, Color::new(0.2, 0.7, 0.6, 1.0)))];
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 95.0, 25.0, 25.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let masked = doc.create(group, NodeKind::Rectangle, "Masked", 0.0, 95.0, 55.0, 25.0)?;
        doc.node_mut(masked).ok_or("masked child missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.2, 0.2, 1.0))];
        let complex_mask = doc.create(group, NodeKind::Group, "Vector mask group", 92.0, 95.0, 42.0, 32.0)?;
        let mask_part =
            doc.create(complex_mask, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Mask part", 0.0, 0.0, 35.0, 30.0)?;
        doc.node_mut(mask_part).ok_or("mask part missing")?.fills = vec![Paint::solid(Color::WHITE)];
        let second_mask_part = doc.create(complex_mask, NodeKind::Rectangle, "Rotated mask part", 14.0, 8.0, 18.0, 18.0)?;
        let second_mask_node = doc.node_mut(second_mask_part).ok_or("second mask part missing")?;
        second_mask_node.fills = vec![Paint::solid(Color::WHITE)];
        second_mask_node.transform = Affine::translate((14.0, 8.0)) * Affine::rotate(0.4);
        let mask_node = doc.node_mut(complex_mask).ok_or("mask group missing")?;
        mask_node.is_mask = true;
        mask_node.mask_type = MaskType::Vector;
        let masked_by_group = doc.create(group, NodeKind::Rectangle, "Masked by group", 90.0, 95.0, 50.0, 30.0)?;
        doc.node_mut(masked_by_group).ok_or("masked by group missing")?.fills = vec![Paint::solid(Color::new(0.1, 0.7, 0.5, 0.9))];
        let mut effect = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(shadow) = &mut effect.kind {
            shadow.radius = 12.0;
            shadow.offset = (5.0, 7.0);
            shadow.color = Color::new(0.2, 0.1, 0.3, 0.5);
        }
        doc.node_mut(group).ok_or("group missing")?.effects = vec![effect];
        for show_behind in [false, true] {
            if let Some(EffectKind::DropShadow(shadow)) = doc.node_mut(group).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind)
            {
                shadow.show_behind = show_behind;
            }
            for spread in [0.0_f64, 2.0, -2.0, 8.0, -8.0, 16.0, -16.0, 32.0, -32.0] {
                doc.node_mut(group).ok_or("group missing")?.transform =
                    Affine::translate((if spread == 0.0 { 65.0 } else { 30.0 }, if spread == 0.0 { 45.0 } else { 25.0 })) * Affine::rotate(0.35);
                if let Some(EffectKind::DropShadow(shadow)) =
                    doc.node_mut(group).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind)
                {
                    shadow.spread = spread;
                }
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (256.0 * scale) as u32, height: (256.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    assert!(gpu.prepare_static_effects(&doc)?.0 >= 1);
                    gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
                    let actual = gpu.pixels()?;
                    let mut changed = 0usize;
                    let mut error = 0u64;
                    for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
                        let deltas = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
                        changed += usize::from(deltas.iter().any(|delta| *delta > 8));
                        error += deltas.iter().map(|delta| u64::from(*delta)).sum::<u64>();
                    }
                    println!(
                        "group drop shadow behind={show_behind} spread={spread} scale={scale} mean_rgb={:.3}/255 changed_gt8={:.3}%",
                        error as f64 / (f64::from(view.width * view.height) * 3.0),
                        changed as f64 * 100.0 / f64::from(view.width * view.height)
                    );
                    assert!(changed * 100 < view.width as usize * view.height as usize, "group drop shadow differs at {scale}x: {changed} pixels");
                }
            }
        }
        doc.node_mut(group).ok_or("group missing")?.transform = Affine::translate((65.0, 45.0)) * Affine::rotate(0.35);
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 256, height: 256 };
        if let Some(EffectKind::DropShadow(shadow)) = doc.node_mut(group).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind) {
            shadow.spread = 0.0;
        }
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        if let Some(EffectKind::DropShadow(shadow)) = doc.node_mut(group).and_then(|node| node.effects.first_mut()).map(|effect| &mut effect.kind) {
            shadow.spread = 32.0;
        }
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        Ok(())
    }

    #[test]
    fn background_blur_preserves_cached_gpu_layer_effects() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU combined blurs");
        let page = doc.current_page.ok_or("new document has no page")?;
        let dark = doc.create(page, NodeKind::Rectangle, "Soft dark half", 0.0, 0.0, 50.0, 100.0)?;
        let node = doc.node_mut(dark).ok_or("dark half missing")?;
        node.fills = vec![Paint::solid(Color::BLACK)];
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::LayerBlur(figcraft_doc::Blur::uniform(8.0)), visible: true }];
        let pane = doc.create(page, NodeKind::Rectangle, "Blur pane", 20.0, 20.0, 60.0, 60.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.fills.clear();
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::BackgroundBlur(figcraft_doc::Blur::uniform(12.0)), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(100, 100)?;
        assert_eq!(gpu.prepare_static_effects(&doc)?.0, 1);
        let index = BackdropIndex::new(&doc, page);
        gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
        assert_eq!(gpu.layer_blurs.len(), 1);
        gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
        let pixels = gpu.pixels()?;
        for x in [35, 47, 53, 65] {
            let offset = (50 * 100 + x) * 4;
            let actual = pixels[offset];
            let expected = reference.data()[offset];
            assert!(actual.abs_diff(expected) <= 20, "x={x}: GPU={actual}, reference={expected}");
        }
        Ok(())
    }

    #[test]
    fn focused_group_with_backdrop_matches_solo_view() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU focus backdrop");
        let page = doc.current_page.ok_or("new document has no page")?;
        let outside = doc.create(page, NodeKind::Rectangle, "Outside focus", 0.0, 0.0, 120.0, 120.0)?;
        let outside_node = doc.node_mut(outside).ok_or("outside missing")?;
        outside_node.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        outside_node.blend = BlendMode::PlusDarker;
        let section = doc.create(page, NodeKind::Section { contents_hidden: false }, "Section", 20.0, 0.0, 120.0, 120.0)?;
        let group = doc.create(section, NodeKind::Group, "Focused group", 5.0, 0.0, 100.0, 100.0)?;
        let dark = doc.create(group, NodeKind::Rectangle, "Dark half", 0.0, 0.0, 50.0, 100.0)?;
        doc.node_mut(dark).ok_or("dark missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let pane = doc.create(group, NodeKind::Rectangle, "Blur pane", 20.0, 20.0, 60.0, 60.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.fills.clear();
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::BackgroundBlur(figcraft_doc::Blur::uniform(12.0)), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 160, height: 120 };
        let focused = focused_document(&doc, page, group).ok_or("focus document missing")?;
        assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        assert!(!has_unhandled_effects_in_focused_subtree(&doc, &view, true, group));
        assert_eq!(focused.node(page).ok_or("page missing")?.children, vec![group]);
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&focused, &view, true));
        let index = BackdropIndex::new(&focused, page);
        assert_eq!(index.visible_ids(&view), vec![pane]);
        let reference = render_view(&doc, page, &view, &Options { solo: Some(group), ..Default::default() }).ok_or("solo reference failed")?;
        let mut gpu = GpuCanvas::new(view.width, view.height)?;
        gpu.prepare_static_effects(&focused)?;
        gpu.render_with_backdrop_blurs_indexed(&focused, page, &view, [245.0 / 255.0; 3], &index)?;
        let actual = gpu.pixels()?;
        for (x, y) in [(10, 10), (40, 50), (70, 50), (100, 50), (145, 50)] {
            let offset = (y * view.width + x) as usize * 4;
            for channel in 0..3 {
                assert!(
                    actual[offset + channel].abs_diff(reference.data()[offset + channel]) <= 20,
                    "focus pixel ({x},{y}) channel {channel}: GPU={} reference={}",
                    actual[offset + channel],
                    reference.data()[offset + channel]
                );
            }
        }
        Ok(())
    }

    #[test]
    fn stacked_gpu_backdrops_sample_prior_backdrop() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU ordered backdrops");
        let page = doc.current_page.ok_or("new document has no page")?;
        let dark = doc.create(page, NodeKind::Rectangle, "Dark", 0.0, 0.0, 50.0, 100.0)?;
        doc.node_mut(dark).ok_or("dark missing")?.fills = vec![Paint::solid(Color::BLACK)];
        for (x, radius) in [(20.0, 12.0), (40.0, 16.0)] {
            let pane = doc.create(page, NodeKind::Rectangle, "Blur pane", x, 20.0, 60.0, 60.0)?;
            let node = doc.node_mut(pane).ok_or("pane missing")?;
            node.fills.clear();
            node.effects = vec![figcraft_doc::Effect { kind: EffectKind::BackgroundBlur(figcraft_doc::Blur::uniform(radius)), visible: true }];
        }
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 120, height: 100 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(120, 100)?;
        gpu.render_with_backdrop_blurs(&doc, page, &view, [245.0 / 255.0; 3])?;
        let actual = gpu.pixels()?;
        for x in [42, 48, 52, 58] {
            let offset = (50 * 120 + x) * 4;
            let delta = actual[offset].abs_diff(reference.data()[offset]);
            assert!(delta <= 24, "x={x}: GPU={} reference={}", actual[offset], reference.data()[offset]);
        }
        Ok(())
    }

    #[test]
    fn simple_glass_uses_gpu_backdrop() -> Result<(), Box<dyn Error>> {
        let wide = std::env::var_os("LIZARD_GLASS_WIDE").is_some();
        let (width, height) = if wide { (3440, 1440) } else { (256, 256) };
        let mut doc = Document::new("GPU glass");
        let page = doc.current_page.ok_or("new document has no page")?;
        let dark = doc.create(page, NodeKind::Rectangle, "Dark half", 0.0, 0.0, 128.0, 256.0)?;
        doc.node_mut(dark).ok_or("dark half missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let pane = doc.create(page, NodeKind::Rectangle, "Glass pane", 80.0, 80.0, 96.0, 96.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.fills.clear();
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::Glass(figcraft_doc::Glass::default()), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width, height };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, false));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(width, height)?;
        let index = BackdropIndex::new(&doc, page);
        gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
        let actual = gpu.pixels()?;
        let mut changed = 0usize;
        let mut sum = 0u64;
        let mut maximum = 0u8;
        for y in 80..176 {
            for x in 80..176 {
                let offset = (y * width as usize + x) * 4;
                for channel in 0..3 {
                    let delta = actual[offset + channel].abs_diff(reference.data()[offset + channel]);
                    sum += u64::from(delta);
                    maximum = maximum.max(delta);
                    changed += usize::from(delta > 8);
                }
            }
        }
        println!("glass_gpu_mean={:.3}/255 changed_channels={changed} max={maximum}", sum as f64 / (96 * 96 * 3) as f64);
        let warm_ms = gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?.0;
        println!("glass_gpu_warm_ms={warm_ms:.2}");
        assert!(sum < 96 * 96 * 3 * 2, "glass RGB error exceeded 2/255 per channel");
        for kind in [
            NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 },
            NodeKind::Vector {
                network: figcraft_doc::VectorNetwork::from_points(
                    &[Point::new(12.0, 12.0), Point::new(92.0, 25.0), Point::new(76.0, 88.0), Point::new(4.0, 76.0)],
                    true,
                ),
            },
        ] {
            let label = kind.label();
            doc.node_mut(pane).ok_or("pane missing")?.kind = kind;
            let index = BackdropIndex::new(&doc, page);
            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
            gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
            let actual = gpu.pixels()?;
            let mut sum = 0u64;
            let mut changed = 0usize;
            for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
                for channel in 0..3 {
                    let delta = a[channel].abs_diff(b[channel]);
                    sum += u64::from(delta);
                    changed += usize::from(delta > 8);
                }
            }
            println!("glass_{label}_mean={:.3}/255 changed_channels={changed}", sum as f64 / (width * height * 3) as f64);
        }
        Ok(())
    }

    #[test]
    fn glass_and_layer_blur_combine_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU glass blur stack");
        let page = doc.current_page.ok_or("new document has no page")?;
        let dark = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 240.0, 480.0)?;
        doc.node_mut(dark).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let pane = doc.create(page, NodeKind::Rectangle, "Glass pane", 80.0, 80.0, 96.0, 96.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.fills.clear();
        let glass = figcraft_doc::Effect { kind: EffectKind::Glass(figcraft_doc::Glass::default()), visible: true };
        let blur = figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true };
        for effects in [vec![glass.clone(), blur.clone()], vec![blur.clone(), glass.clone()]] {
            doc.node_mut(pane).ok_or("pane missing")?.effects = effects;
            for angle in [0.0, 0.5] {
                doc.node_mut(pane).ok_or("pane missing")?.transform = Affine::translate((160.0, 160.0)) * Affine::rotate(angle);
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (480.0 * scale) as u32, height: (480.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "glass+blur angle={angle} scale={scale}");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    let index = BackdropIndex::new(&doc, page);
                    gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    assert!(
                        changed * 100 < view.width as usize * view.height as usize,
                        "glass+blur angle={angle} at {scale}x changed {changed} pixels"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn glass_blur_noise_combinations_use_ordered_gpu_passes() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU glass noise stack");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 480.0, 480.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.25, 0.45, 0.8, 1.0))];
        let pane = doc.create(page, NodeKind::Rectangle, "Glass pane", 160.0, 160.0, 96.0, 96.0)?;
        doc.node_mut(pane).ok_or("pane missing")?.fills.clear();
        let glass = figcraft_doc::Effect { kind: EffectKind::Glass(figcraft_doc::Glass::default()), visible: true };
        let noise = figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true };
        let blur = figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true };
        for effects in [
            vec![glass.clone(), noise.clone()],
            vec![noise.clone(), glass.clone()],
            vec![glass.clone(), blur.clone(), noise.clone()],
            vec![noise.clone(), glass.clone(), blur.clone()],
            vec![blur.clone(), glass.clone(), noise.clone()],
            vec![glass.clone(), noise.clone(), blur.clone()],
        ] {
            doc.node_mut(pane).ok_or("pane missing")?.effects = effects;
            for angle in [0.0, 0.5] {
                doc.node_mut(pane).ok_or("pane missing")?.transform = Affine::translate((160.0, 160.0)) * Affine::rotate(angle);
                for scale in [1.0, 2.0] {
                    let view = View { origin: (0.0, 0.0), scale, width: (480.0 * scale) as u32, height: (480.0 * scale) as u32 };
                    assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "glass+noise angle={angle} scale={scale}");
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                    let mut gpu = GpuCanvas::new(view.width, view.height)?;
                    let index = BackdropIndex::new(&doc, page);
                    gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
                    let changed = gpu
                        .pixels()?
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                        .count();
                    assert!(
                        changed * 100 < view.width as usize * view.height as usize,
                        "glass+noise angle={angle} at {scale}x changed {changed} pixels"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn glass_and_drop_shadow_combine_on_gpu() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU glass shadow stack");
        let page = doc.current_page.ok_or("new document has no page")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 480.0, 480.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.25, 0.45, 0.8, 1.0))];
        let pane = doc.create(page, NodeKind::Rectangle, "Glass pane", 160.0, 160.0, 96.0, 96.0)?;
        doc.node_mut(pane).ok_or("pane missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.9, 1.0, 1.0))];
        let glass = figcraft_doc::Effect { kind: EffectKind::Glass(figcraft_doc::Glass::default()), visible: true };
        let mut shadow = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(value) = &mut shadow.kind {
            value.radius = 8.0;
            value.spread = 2.0;
            value.offset = (4.0, 6.0);
            value.color = Color::new(0.1, 0.1, 0.1, 0.2);
            value.show_behind = true;
        }
        for fill_alpha in [1.0, 0.6] {
            doc.node_mut(pane).ok_or("pane missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.9, 1.0, fill_alpha))];
            for show_behind in [true, false] {
                if let EffectKind::DropShadow(value) = &mut shadow.kind {
                    value.show_behind = show_behind;
                }
                for effects in [vec![glass.clone(), shadow.clone()], vec![shadow.clone(), glass.clone()]] {
                    doc.node_mut(pane).ok_or("pane missing")?.effects = effects;
                    for angle in [0.0, 0.5] {
                        doc.node_mut(pane).ok_or("pane missing")?.transform = Affine::translate((160.0, 160.0)) * Affine::rotate(angle);
                        for scale in [1.0, 2.0] {
                            let view = View { origin: (0.0, 0.0), scale, width: (480.0 * scale) as u32, height: (480.0 * scale) as u32 };
                            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "glass+shadow angle={angle} scale={scale}");
                            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
                            let mut gpu = GpuCanvas::new(view.width, view.height)?;
                            let index = BackdropIndex::new(&doc, page);
                            gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
                            let changed = gpu
                                .pixels()?
                                .chunks_exact(4)
                                .zip(reference.data().chunks_exact(4))
                                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                                .count();
                            assert!(
                                changed * 100 < view.width as usize * view.height as usize,
                                "glass+shadow angle={angle} at {scale}x changed {changed} pixels"
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn glass_blur_noise_shadow_stack_stays_on_gpu_at_viewport_edge() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU glass effect stack");
        let page = doc.current_page.ok_or("page missing")?;
        let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 240.0, 480.0)?;
        doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.45, 0.8, 1.0))];
        let pane = doc.create(page, NodeKind::Rectangle, "Pane", 150.0, 150.0, 180.0, 180.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.transform = Affine::translate((180.0, 180.0)) * Affine::rotate(0.25);
        node.fills = vec![Paint::solid(Color::new(0.9, 0.9, 1.0, 0.6))];
        let mut shadow = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(value) = &mut shadow.kind {
            value.radius = 12.0;
            value.spread = 4.0;
            value.offset = (10.0, 12.0);
            value.color = Color::new(0.02, 0.02, 0.08, 0.25);
        }
        node.effects = vec![
            shadow,
            figcraft_doc::Effect { kind: EffectKind::Noise(Noise::default()), visible: true },
            figcraft_doc::Effect { kind: EffectKind::Glass(figcraft_doc::Glass::default()), visible: true },
            figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(8.0)), visible: true },
        ];
        for scale in [1.0, 2.0] {
            let view = View { origin: (0.0, 0.0), scale, width: (480.0 * scale) as u32, height: (480.0 * scale) as u32 };
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "stack should stay on GPU at {scale}x");
            let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
            let mut gpu = GpuCanvas::new(view.width, view.height)?;
            gpu.prepare_static_effects(&doc)?;
            let index = BackdropIndex::new(&doc, page);
            gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [245.0 / 255.0; 3], &index)?;
            let changed = gpu
                .pixels()?
                .chunks_exact(4)
                .zip(reference.data().chunks_exact(4))
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                .count();
            assert!(changed * 100 < view.width as usize * view.height as usize * 15, "four-effect stack at {scale}x changed {changed} pixels");
        }
        Ok(())
    }

    #[test]
    fn individually_painted_vector_regions_use_gpu_when_supported() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Vector region capability");
        let page = doc.current_page.ok_or("new document has no page")?;
        let mut network = figcraft_doc::VectorNetwork::from_points(
            &[
                figcraft_geom::Point::new(0.0, 0.0),
                figcraft_geom::Point::new(40.0, 0.0),
                figcraft_geom::Point::new(40.0, 40.0),
                figcraft_geom::Point::new(0.0, 40.0),
            ],
            true,
        );
        let id = doc.create(page, NodeKind::Vector { network: network.clone() }, "Region", 20.0, 20.0, 40.0, 40.0)?;
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 256, height: 256 };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        if let Some(region) = network.regions.first_mut() {
            region.fills = Some(vec![Paint::solid(Color::BLACK)]);
        }
        doc.node_mut(id).ok_or("vector node missing")?.kind = NodeKind::Vector { network: network.clone() };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        if let Some(region) = network.regions.first_mut() {
            region.fills = Some(vec![Paint::gradient(figcraft_color::Gradient::default_for(GradientKind::Diamond, Color::new(1.0, 0.0, 0.0, 1.0)))]);
        }
        doc.node_mut(id).ok_or("vector node missing")?.kind = NodeKind::Vector { network };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        Ok(())
    }

    #[test]
    fn isolated_opaque_vector_shadow_uses_gpu_and_translucent_fill_needs_cache() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Vector shadow capability");
        let page = doc.current_page.ok_or("new document has no page")?;
        let network = figcraft_doc::VectorNetwork::from_points(
            &[figcraft_geom::Point::new(0.0, 0.0), figcraft_geom::Point::new(40.0, 0.0), figcraft_geom::Point::new(20.0, 35.0)],
            true,
        );
        let id = doc.create(page, NodeKind::Vector { network }, "Vector", 20.0, 20.0, 40.0, 35.0)?;
        let node = doc.node_mut(id).ok_or("vector missing")?;
        node.fills = vec![Paint::solid(Color::BLACK)];
        node.effects = vec![figcraft_doc::Effect::drop_shadow()];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 256, height: 256 };
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        if let Some(figcraft_doc::Effect { kind: EffectKind::DropShadow(shadow), .. }) = doc.node_mut(id).ok_or("vector missing")?.effects.first_mut()
        {
            shadow.spread = 3.0;
        }
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        if let Some(figcraft_doc::Effect { kind: EffectKind::DropShadow(shadow), .. }) = doc.node_mut(id).ok_or("vector missing")?.effects.first_mut()
        {
            shadow.spread = 10.0;
        }
        assert!(!has_unhandled_effects_in_view(&doc, &view));
        if let Some(figcraft_doc::Effect { kind: EffectKind::DropShadow(shadow), .. }) = doc.node_mut(id).ok_or("vector missing")?.effects.first_mut()
        {
            shadow.spread = 0.0;
        }
        doc.node_mut(id).ok_or("vector missing")?.fills = vec![Paint::solid(Color::new(0.0, 0.0, 0.0, 0.5))];
        assert!(has_unhandled_effects_in_view(&doc, &view));
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct ImageInfo {
    pub id: TextureId,
    pub width: u16,
    pub height: u16,
    /// Screen-space placement for cropped backdrop textures; ordinary images use `None`.
    pub region: Option<(u16, u16, u16, u16)>,
}

/// Rebuild when document geometry or effect properties change.
pub struct BackdropIndex {
    candidates: Vec<(NodeId, f64, Rect, bool)>,
}

/// Build the page that Focus mode actually paints while retaining each node's original
/// ancestry, so world transforms and backdrop ordering remain in document coordinates.
pub fn focused_document(doc: &Document, page: NodeId, focus: NodeId) -> Option<Document> {
    if focus == page || doc.page_of(focus)? != page {
        return None;
    }
    let kept: HashSet<_> = doc.subtree(focus).into_iter().collect();
    let mut focused = doc.clone();
    for id in focused.ids().collect::<Vec<_>>() {
        if id != page
            && !kept.contains(&id)
            && let Some(node) = focused.node_mut(id)
        {
            node.visible = false;
        }
    }
    focused.node_mut(page)?.children = vec![focus];
    Some(focused)
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
                    EffectKind::BackgroundBlur(blur) if effect.visible && blur.max_radius() > 0.0 => Some(blur.max_radius()),
                    EffectKind::Glass(glass) if effect.visible => Some(glass.frost),
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

fn diamond_key(gradient: &Gradient, opacity: f32) -> String {
    format!("diamond:{gradient:?}:{}", opacity.to_bits())
}

fn shader_key(node: &Node, fill: &ShaderFill) -> String {
    format!("shader:{:?}:{}:{:?}", node.id, fill.shader_id, fill.values)
}

fn pattern_key(node: &Node, fill: &PatternPaint) -> String {
    format!("pattern:{:?}:{fill:?}", node.id)
}

fn gpu_pattern_source<'a>(doc: &'a Document, fill: &PatternPaint) -> Option<&'a Node> {
    let source = doc.node(NodeId(fill.source))?;
    (source.visible && !source.is_page() && !source.is_mask && gpu_pattern_subtree_supported(doc, source, 0)).then_some(source)
}

fn gpu_pattern_subtree_supported(doc: &Document, node: &Node, depth: usize) -> bool {
    if depth > 64
        || !gpu_styled_text_paints_supported(node)
        || node.effects.iter().filter(|effect| effect.visible).count() > 3
        || node.effects.iter().any(|effect| {
            effect.visible
                && !matches!(&effect.kind, EffectKind::LayerBlur(blur) if !blur.is_progressive() && blur.radius.is_finite() && (0.0..=32.0).contains(&blur.radius))
        })
        || node.is_mask && node.mask_type == MaskType::Luminance && !gpu_luminance_mask_supported(node)
    {
        return false;
    }
    let region_paints = match &node.kind {
        NodeKind::Vector { network } => network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    matches!(node.blend, BlendMode::Normal | BlendMode::PassThrough)
        && node.fills.iter().chain(&node.strokes).chain(region_paints).all(|paint| {
            !paint.visible
                || ((gpu_blend_mode(paint.blend).is_some() || paint.blend == BlendMode::Normal)
                    && match &paint.kind {
                        PaintKind::Pattern(_) => false,
                        PaintKind::Shader(shader) => doc.shaders.contains_key(&shader.shader_id),
                        _ => true,
                    })
        })
        && node.children.iter().all(|id| doc.node(*id).is_some_and(|child| gpu_pattern_subtree_supported(doc, child, depth + 1)))
}

fn gpu_styled_text_paints_supported(node: &Node) -> bool {
    let NodeKind::Text(text) = &node.kind else { return true };
    std::iter::once(&text.style).chain(text.runs.iter().map(|run| &run.style)).all(|style| {
        style.fills.as_ref().is_none_or(|fills| {
            fills.iter().all(|paint| {
                !paint.visible || paint.blend == BlendMode::Normal && matches!(paint.kind, PaintKind::Solid { .. } | PaintKind::Gradient(_))
            })
        })
    })
}

fn pattern_source_state(doc: &Document, id: NodeId, state: &mut String) {
    let Some(node) = doc.node(id) else { return };
    state.push_str(&format!("{node:?}"));
    for child in &node.children {
        pattern_source_state(doc, *child, state);
    }
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

fn set_paint(scene: &mut Scene, doc: &Document, paint: &Paint, node: &Node, images: Option<&HashMap<String, ImageInfo>>) -> bool {
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
            let Some(info) = images.and_then(|entries| entries.get(&diamond_key(g, paint.opacity))) else { return false };
            let w = node.width.max(1e-6);
            let h = node.height.max(1e-6);
            let from = (g.from.0 * w, g.from.1 * h);
            let axis = ((g.to.0 - g.from.0) * w, (g.to.1 - g.from.1) * h);
            let width = ((g.width.0 - g.from.0) * w, (g.width.1 - g.from.1) * h);
            scene.set_paint_transform(Affine::new([
                axis.0 / 128.0,
                axis.1 / 128.0,
                width.0 / 128.0,
                width.1 / 128.0,
                from.0 - axis.0 - width.0,
                from.1 - axis.1 - width.1,
            ]));
            scene.set_paint(Image {
                image: ImageSource::external_texture(info.id, RectU16::new(0, 0, info.width, info.height), true),
                sampler: peniko::ImageSampler::default().with_extend(peniko::Extend::Pad),
            });
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
        PaintKind::Shader(fill) => {
            let Some(info) = images.and_then(|entries| entries.get(&shader_key(node, fill))) else { return false };
            scene.set_paint_transform(Affine::scale_non_uniform(node.width / f64::from(info.width), node.height / f64::from(info.height)));
            scene.set_paint(Image {
                image: ImageSource::external_texture(info.id, RectU16::new(0, 0, info.width, info.height), true),
                sampler: peniko::ImageSampler::default().with_alpha(paint.opacity.clamp(0.0, 1.0)),
            });
        }
        PaintKind::Pattern(fill) => {
            let Some(source) = gpu_pattern_source(doc, fill) else { return false };
            let Some(cell) = fill.cell(source.width, source.height, node.width, node.height) else { return false };
            let Some(info) = images.and_then(|entries| entries.get(&pattern_key(node, fill))) else { return false };
            scene.set_paint_transform(
                Affine::translate(cell.origin) * Affine::scale_non_uniform(cell.size.0 / f64::from(info.width), cell.size.1 / f64::from(info.height)),
            );
            scene.set_paint(Image {
                image: ImageSource::external_texture(info.id, RectU16::new(0, 0, info.width, info.height), true),
                sampler: peniko::ImageSampler::default().with_extend(peniko::Extend::Repeat).with_alpha(paint.opacity.clamp(0.0, 1.0)),
            });
        }
    }
    true
}

fn set_rule(scene: &mut Scene, rule: FillRule) {
    scene.set_fill_rule(if rule == FillRule::EvenOdd { Fill::EvenOdd } else { Fill::NonZero });
}

fn fill_paint_path(scene: &mut Scene, path: &peniko::kurbo::BezPath, paint: &Paint) {
    let blend = gpu_blend_mode(paint.blend);
    if let Some(blend) = blend {
        scene.push_blend_layer(blend);
    }
    scene.fill_path(path);
    if blend.is_some() {
        scene.pop_layer();
    }
}

fn draw_content(
    scene: &mut Scene,
    doc: &Document,
    node: &Node,
    transform: Affine,
    images: Option<&HashMap<String, ImageInfo>>,
    backdrops: Option<&HashMap<NodeId, ImageInfo>>,
    inner_shadows: Option<&HashMap<NodeId, CachedLayerBlur>>,
    camera: Affine,
    size: (u16, u16),
) {
    if let Some(info) = backdrops.and_then(|all| all.get(&node.id))
        && let Some((mut path, rule)) = draw::node_outline(doc, node)
    {
        path.apply_affine(transform);
        scene.set_transform(Affine::IDENTITY);
        let (x, y, width, height) = info.region.unwrap_or((0, 0, size.0, size.1));
        scene.set_paint_transform(
            Affine::translate((f64::from(x), f64::from(y)))
                * Affine::scale_non_uniform(f64::from(width) / f64::from(info.width), f64::from(height) / f64::from(info.height)),
        );
        set_rule(scene, rule);
        scene.set_paint(Image {
            image: ImageSource::external_texture(info.id, RectU16::new(0, 0, info.width, info.height), true),
            sampler: peniko::ImageSampler::default(),
        });
        scene.fill_path(&path);
    }
    if let NodeKind::Vector { network } = &node.kind
        && network.regions.iter().any(|region| region.fills.is_some())
    {
        // A painted vector face owns its fill. Drawing the node outline once would recolor every
        // face and make an explicitly empty face impossible, so follow the document's face order.
        let rounded = network.rounded();
        let drawn = rounded.as_ref().unwrap_or(network);
        scene.set_transform(transform);
        let open = drawn.unassigned_paint_path();
        if !open.elements().is_empty() {
            set_rule(scene, FillRule::Winding);
            for paint in &node.fills {
                scene.reset_paint_transform();
                if set_paint(scene, doc, paint, node, images) {
                    fill_paint_path(scene, &open, paint);
                }
            }
        }
        for (index, region) in drawn.regions.iter().enumerate() {
            let Some(path) = drawn.region_path(index) else { continue };
            set_rule(scene, if region.winding == figcraft_doc::WindingRule::EvenOdd { FillRule::EvenOdd } else { FillRule::Winding });
            for paint in region.fills.as_ref().unwrap_or(&node.fills) {
                scene.reset_paint_transform();
                if set_paint(scene, doc, paint, node, images) {
                    fill_paint_path(scene, &path, paint);
                }
            }
        }
    } else if let NodeKind::Text(text) = &node.kind
        && (!text.runs.is_empty() || text.style.fills.is_some())
    {
        if let Some((layout, dy)) = figcraft_text::layout_node(node) {
            let mut paths = std::collections::BTreeMap::<usize, BezPath>::new();
            for glyph in &layout.glyphs {
                paths.entry(glyph.style).or_default().extend(glyph.path().elements().iter().copied());
            }
            for decoration in &layout.decorations {
                paths.entry(decoration.style).or_default().extend(decoration.path().elements().iter().copied());
            }
            scene.set_transform(transform * Affine::translate((0.0, dy)));
            set_rule(scene, FillRule::Winding);
            for (index, path) in paths {
                let paints = layout.styles.get(index).and_then(|style| style.fills.as_ref()).unwrap_or(&node.fills);
                for paint in paints {
                    scene.reset_paint_transform();
                    if set_paint(scene, doc, paint, node, images) {
                        fill_paint_path(scene, &path, paint);
                    }
                }
            }
        }
    } else if let Some((path, rule)) = draw::node_outline(doc, node) {
        scene.set_transform(transform);
        set_rule(scene, rule);
        for paint in &node.fills {
            scene.reset_paint_transform();
            if set_paint(scene, doc, paint, node, images) {
                fill_paint_path(scene, &path, paint);
            }
        }
    }
    if let Some(cached) = inner_shadows.and_then(|entries| entries.get(&node.id)) {
        draw_cached_effect(scene, camera, cached);
    } else {
        draw_inner_shadows(scene, doc, node, transform);
    }
    if let Some((path, rule)) = draw::stroke_fill(doc, node) {
        scene.set_transform(transform);
        set_rule(scene, rule);
        for paint in &node.strokes {
            scene.reset_paint_transform();
            if set_paint(scene, doc, paint, node, images) {
                fill_paint_path(scene, &path, paint);
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

fn draw_cached_effect(scene: &mut Scene, camera: Affine, cached: &CachedLayerBlur) {
    scene.set_transform(camera * cached.placement);
    scene.set_paint_transform(
        Affine::translate((cached.bounds.x0, cached.bounds.y0))
            * Affine::scale_non_uniform(cached.bounds.width() / f64::from(cached.width), cached.bounds.height() / f64::from(cached.height)),
    );
    scene.set_paint(Image {
        image: ImageSource::external_texture(cached.id, RectU16::new(0, 0, cached.width, cached.height), true),
        sampler: peniko::ImageSampler::default(),
    });
    scene.fill_rect(&cached.bounds);
}

#[derive(Clone, Copy)]
struct CachedEffects<'a> {
    layer_blurs: Option<&'a HashMap<NodeId, CachedLayerBlur>>,
    layer_blurs_1x: Option<&'a HashMap<NodeId, CachedLayerBlur>>,
    drop_shadows: Option<&'a HashMap<NodeId, CachedLayerBlur>>,
    inner_shadows: Option<&'a HashMap<NodeId, CachedLayerBlur>>,
    noise: Option<&'a HashMap<NodeId, CachedLayerBlur>>,
    texture: Option<&'a HashMap<NodeId, CachedLayerBlur>>,
    outlined: &'a [NodeId],
    outline_options: figcraft_render::Outline,
}

impl CachedEffects<'_> {
    fn empty() -> Self {
        Self {
            layer_blurs: None,
            layer_blurs_1x: None,
            drop_shadows: None,
            inner_shadows: None,
            noise: None,
            texture: None,
            outlined: &[],
            outline_options: figcraft_render::Outline::default(),
        }
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

/// The generic Vello filter matches simple opaque path shadows with small positive spread.
/// Translucent fills, stroke geometry, and mixed effect stacks use other guarded paths.
fn gpu_vector_drop_shadow_supported(node: &Node, shadow: &Shadow) -> bool {
    (matches!(&node.kind, NodeKind::Vector { network } if network.regions.iter().all(|region| region.fills.is_none()))
        || matches!(node.kind, NodeKind::Ellipse { start, sweep, inner }
            if start == 0.0 && (sweep - std::f64::consts::TAU).abs() < 1e-6 && inner == 0.0)
        || matches!(node.kind, NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::Text(_)))
        && node.children.is_empty()
        && node.strokes.is_empty()
        && node.opacity >= 0.999
        && node.blend == BlendMode::Normal
        && node.fills.len() == 1
        && node.fills.iter().all(|paint| {
            paint.visible
                && paint.opacity >= 0.999
                && paint.blend == BlendMode::Normal
                && matches!(paint.kind, PaintKind::Solid { color } if color.a >= 0.999)
        })
        && shadow.blend == BlendMode::Normal
        && shadow.spread.is_finite()
        && (0.0..=32.0).contains(&shadow.spread)
        && (!matches!(node.kind, NodeKind::Text(_)) || shadow.spread == 0.0)
        && shadow.radius.is_finite()
        && shadow.radius >= 0.0
        && shadow.offset.0.is_finite()
        && shadow.offset.1.is_finite()
        && node.width > 0.0
        && node.height > 0.0
}

fn draw_shadow(scene: &mut Scene, doc: &Document, node: &Node, camera: Affine) {
    let Some((path, rule)) = draw::node_outline(doc, node) else { return };
    let transform = camera * doc.world_transform(node.id);
    for effect in &node.effects {
        let EffectKind::DropShadow(shadow) = &effect.kind else { continue };
        if !effect.visible {
            continue;
        }
        let shadow_transform = camera * Affine::translate(shadow.offset) * doc.world_transform(node.id);
        if gpu_drop_shadow_supported(doc, node, shadow)
            && let Some(radius) = node.corners.uniform()
        {
            let spread = shadow.spread;
            let rect = Rect::new(-spread, -spread, node.width + spread, node.height + spread);
            scene.reset_paint_transform();
            scene.set_paint(color(shadow.color, 1.0));
            if !shadow.show_behind {
                let pad = spread + shadow.radius * 2.0 + shadow.offset.0.abs().max(shadow.offset.1.abs()) + 10.0;
                let outer = Rect::new(-pad, -pad, node.width + pad, node.height + pad);
                let mut clip = outer.to_path(0.1);
                clip.extend(path.elements().iter().copied());
                scene.set_transform(transform);
                scene.set_fill_rule(Fill::EvenOdd);
                scene.push_clip_path(&clip);
            }
            scene.set_transform(shadow_transform);
            scene.fill_blurred_rounded_rect(&rect, (radius + spread) as f32, (shadow.radius / 2.0) as f32, false);
            if !shadow.show_behind {
                scene.pop_clip();
            }
            continue;
        }
        scene.set_transform(shadow_transform);
        scene.reset_paint_transform();
        set_rule(scene, rule);
        let source_alpha =
            if matches!(&node.kind, NodeKind::Vector { .. } | NodeKind::Ellipse { .. } | NodeKind::Polygon { .. } | NodeKind::Star { .. })
                && node.strokes.is_empty()
                && node.fills.len() == 1
            {
                node.fills
                    .first()
                    .and_then(|paint| match paint.kind {
                        PaintKind::Solid { color } if paint.visible => Some((color.a * paint.opacity).clamp(0.0, 1.0)),
                        _ => None,
                    })
                    .unwrap_or(1.0)
            } else {
                1.0
            };
        scene.set_paint(peniko::Color::new([1.0, 1.0, 1.0, source_alpha]));
        scene.push_filter_layer(Filter::from_primitive(FilterPrimitive::DropShadowOnly {
            dx: 0.0,
            dy: 0.0,
            std_deviation: (shadow.radius / 2.0) as f32,
            color: color(shadow.color, 1.0),
            edge_mode: EdgeMode::None,
        }));
        scene.fill_path(&path);
        if shadow.spread > 0.0 {
            scene.set_stroke(
                peniko::kurbo::Stroke::new(shadow.spread * 2.0)
                    .with_join(peniko::kurbo::Join::Miter)
                    .with_caps(peniko::kurbo::Cap::Butt)
                    .with_miter_limit(figcraft_doc::SPREAD_MITER_LIMIT),
            );
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
    cached: CachedEffects<'_>,
    size: (u16, u16),
) {
    let selected = cached.outlined.contains(&id);
    if depth > 64 || !selected && !figcraft_render::raster_bounds(doc, id).is_some_and(|bounds| bounds.intersect(view).area() > 0.0) {
        return;
    }
    let Some(node) = doc.node(id) else { return };
    if node.is_mask {
        return;
    }
    if selected {
        stroke_outline_node(scene, doc, id, camera, camera.as_coeffs()[0].hypot(camera.as_coeffs()[1]), cached.outline_options, depth);
        return;
    }
    if !node.visible {
        return;
    }
    if let Some(effect) = (camera.determinant().abs().sqrt() <= 1.0)
        .then(|| cached.layer_blurs_1x.and_then(|entries| entries.get(&id)))
        .flatten()
        .or_else(|| cached.layer_blurs.and_then(|entries| entries.get(&id)))
        .or_else(|| cached.noise.and_then(|entries| entries.get(&id)))
        .or_else(|| cached.texture.and_then(|entries| entries.get(&id)))
    {
        let blend = gpu_node_blend(node);
        if let Some(blend) = blend {
            scene.push_blend_layer(blend);
        }
        let opacity_layer =
            node.opacity < 0.999 && (node.blend == BlendMode::Normal || matches!(node.kind, NodeKind::Group) && node.blend == BlendMode::PassThrough);
        if opacity_layer {
            scene.push_opacity_layer(node.opacity);
        }
        draw_cached_effect(scene, camera, effect);
        if opacity_layer {
            scene.pop_layer();
        }
        if blend.is_some() {
            scene.pop_layer();
        }
        return;
    }
    let transform = camera * doc.world_transform(id);
    let blend = gpu_node_blend(node);
    let shadow_outside_blend = gpu_shadowed_node_blend_supported(node);
    if let Some(blend) = blend.filter(|_| !shadow_outside_blend) {
        scene.push_blend_layer(blend);
    }
    let opacity_layer = node.opacity < 0.999;
    let split_shadow_opacity = opacity_layer && shadow_outside_blend;
    if opacity_layer && !split_shadow_opacity {
        scene.push_opacity_layer(node.opacity);
    }
    let effects = std::env::var_os("LIZARD_GPU_NO_EFFECTS").is_none();
    if effects {
        if split_shadow_opacity {
            scene.push_opacity_layer(node.opacity);
        }
        if let Some(effect) = cached.drop_shadows.and_then(|entries| entries.get(&id)) {
            draw_cached_effect(scene, camera, effect);
        } else {
            draw_shadow(scene, doc, node, camera);
        }
        if split_shadow_opacity {
            scene.pop_layer();
        }
    }
    if let Some(blend) = blend.filter(|_| shadow_outside_blend) {
        scene.push_blend_layer(blend);
    }
    if split_shadow_opacity {
        scene.push_opacity_layer(node.opacity);
    }
    let blurs = if backdrops.is_some_and(|entries| entries.contains_key(&id)) && gpu_glass_stack_supported(doc, node) {
        Vec::new()
    } else {
        node.effects
            .iter()
            .filter_map(|effect| match &effect.kind {
                EffectKind::LayerBlur(blur) if effects && effect.visible && blur.radius > 0.0 => Some(blur.radius),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    for radius in &blurs {
        scene.set_transform(transform);
        scene.push_filter_layer(Filter::from_function(FilterFunction::Blur { radius: (radius / 2.0) as f32 }));
    }
    // Paint blends see earlier paints of the same leaf, not the page behind it. Isolate those
    // paints before applying a layer blur or placing a drop shadow underneath them.
    let region_blended = matches!(&node.kind, NodeKind::Vector { network }
        if network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().any(|paint| paint.visible && paint.blend != BlendMode::Normal));
    let isolate_paints = (gpu_shadowed_paint_blend_supported(node) || gpu_uniform_blur_paint_blend_supported(node))
        && (region_blended || node.fills.iter().chain(&node.strokes).any(|paint| paint.visible && paint.blend != BlendMode::Normal));
    if isolate_paints {
        scene.push_opacity_layer(1.0);
    }
    draw_content(scene, doc, node, transform, images, backdrops, cached.inner_shadows, camera, size);
    if isolate_paints {
        scene.pop_layer();
    }
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
    draw_paint_runs(scene, doc, node, camera, view, depth + 1, images, backdrops, cached, size);
    if clipped {
        scene.pop_clip();
    }
    for _ in &blurs {
        scene.pop_layer();
    }
    if opacity_layer {
        scene.pop_layer();
    }
    if blend.is_some() {
        scene.pop_layer();
    }
}

fn gpu_node_blend(node: &Node) -> Option<peniko::BlendMode> {
    gpu_blend_mode(node.blend)
}

fn gpu_blend_mode(mode: BlendMode) -> Option<peniko::BlendMode> {
    use peniko::{Compose, Mix};
    let mix = match mode {
        BlendMode::PassThrough | BlendMode::Normal | BlendMode::PlusDarker => return None,
        BlendMode::Darken => Mix::Darken,
        BlendMode::Multiply => Mix::Multiply,
        BlendMode::ColorBurn => Mix::ColorBurn,
        BlendMode::Lighten => Mix::Lighten,
        BlendMode::Screen => Mix::Screen,
        BlendMode::PlusLighter => return Some(Compose::Plus.into()),
        BlendMode::ColorDodge => Mix::ColorDodge,
        BlendMode::Overlay => Mix::Overlay,
        BlendMode::SoftLight => Mix::SoftLight,
        BlendMode::HardLight => Mix::HardLight,
        BlendMode::Difference => Mix::Difference,
        BlendMode::Exclusion => Mix::Exclusion,
        BlendMode::Hue => Mix::Hue,
        BlendMode::Saturation => Mix::Saturation,
        BlendMode::Color => Mix::Color,
        BlendMode::Luminosity => Mix::Luminosity,
    };
    Some(mix.into())
}

fn gpu_simple_node_blend_supported(node: &Node) -> bool {
    gpu_node_blend(node).is_some()
        && (node.effects.iter().all(|effect| !effect.visible)
            || gpu_shadowed_node_blend_supported(node)
            || gpu_cached_node_blend_supported(node)
            || gpu_uniform_blur_node_blend_supported(node))
}

fn gpu_uniform_blur_node_blend_supported(node: &Node) -> bool {
    let mut visible = node.effects.iter().filter(|effect| effect.visible);
    node.children.is_empty()
        && (0.0..=1.0).contains(&node.opacity)
        && visible.next().is_some_and(|effect| {
            matches!(&effect.kind, EffectKind::LayerBlur(blur) if !blur.is_progressive() && blur.radius.is_finite() && (0.0..=80.0).contains(&blur.radius))
        })
        && visible.next().is_none()
        && node.fills.iter().chain(&node.strokes).all(|paint| !paint.visible || paint.blend == BlendMode::Normal || gpu_blend_mode(paint.blend).is_some())
        && match &node.kind {
            NodeKind::Vector { network } => network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().all(|paint| !paint.visible || paint.blend == BlendMode::Normal || gpu_blend_mode(paint.blend).is_some()),
            _ => true,
        }
}

fn gpu_cached_node_blend_supported(node: &Node) -> bool {
    let visible = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
    node.children.is_empty()
        && (node.opacity >= 0.999 || (0.0..=1.0).contains(&node.opacity) && !matches!(node.blend, BlendMode::ColorBurn | BlendMode::PlusLighter))
        && matches!(
            node.blend,
            BlendMode::Darken
                | BlendMode::Multiply
                | BlendMode::ColorBurn
                | BlendMode::Screen
                | BlendMode::PlusLighter
                | BlendMode::Overlay
                | BlendMode::SoftLight
                | BlendMode::Exclusion
                | BlendMode::Hue
                | BlendMode::Saturation
                | BlendMode::Color
                | BlendMode::Luminosity
        )
        && visible.len() == 2
        && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::LayerBlur(_))).count() == 1
        && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::Noise(_))).count() == 1
        && node.fills.iter().chain(&node.strokes).all(|paint| !paint.visible || paint.blend == BlendMode::Normal)
}

fn gpu_shadowed_node_blend_supported(node: &Node) -> bool {
    node.children.is_empty()
        && (0.0..=1.0).contains(&node.opacity)
        && gpu_node_blend(node).is_some()
        && node.effects.iter().any(|effect| effect.visible)
        && node.effects.iter().all(|effect| !effect.visible || matches!(effect.kind, EffectKind::DropShadow(_)))
}

fn gpu_simple_paint_blend_supported(node: &Node, paint: &Paint) -> bool {
    gpu_blend_mode(paint.blend).is_some()
        && node.children.is_empty()
        && (node.effects.iter().all(|effect| !effect.visible)
            || gpu_shadowed_paint_blend_supported(node)
            || gpu_uniform_blur_paint_blend_supported(node))
}

fn gpu_uniform_blur_paint_blend_supported(node: &Node) -> bool {
    let mut visible = node.effects.iter().filter(|effect| effect.visible);
    node.children.is_empty()
        && (node.blend == BlendMode::Normal || gpu_node_blend(node).is_some())
        && (0.0..=1.0).contains(&node.opacity)
        && visible.next().is_some_and(|effect| {
            matches!(&effect.kind, EffectKind::LayerBlur(blur) if !blur.is_progressive() && blur.radius.is_finite() && (0.0..=80.0).contains(&blur.radius))
        })
        && visible.next().is_none()
}

fn gpu_shadowed_paint_blend_supported(node: &Node) -> bool {
    node.children.is_empty()
        && node.effects.iter().any(|effect| effect.visible)
        && node.effects.iter().all(|effect| !effect.visible || matches!(effect.kind, EffectKind::DropShadow(_)))
}

fn draw_paint_runs(
    scene: &mut Scene,
    doc: &Document,
    node: &Node,
    camera: Affine,
    view: Rect,
    child_depth: usize,
    images: Option<&HashMap<String, ImageInfo>>,
    backdrops: Option<&HashMap<NodeId, ImageInfo>>,
    cached: CachedEffects<'_>,
    size: (u16, u16),
) {
    for run in doc.paint_runs(node) {
        let Some(&first_id) = node.children.get(run.start) else { continue };
        let Some(first) = doc.node(first_id) else { continue };
        if first.is_mask && first.visible {
            let content = node.children.get(run.start + 1..run.end).unwrap_or(&[]);
            if content.is_empty() {
                continue;
            }
            match first.mask_type {
                MaskType::Vector => {
                    let Some((path, rule)) = draw::subtree_outline(doc, first, child_depth) else { continue };
                    scene.set_transform(camera * doc.world_transform(first_id));
                    set_rule(scene, rule);
                    scene.push_clip_layer(&path);
                    for child in content {
                        draw_node(scene, doc, *child, camera, view, child_depth, images, backdrops, cached, size);
                    }
                    scene.pop_layer();
                }
                MaskType::Alpha | MaskType::Luminance => {
                    // Isolate the run so destination-in can only modify its own content.
                    scene.push_opacity_layer(1.0);
                    for child in content {
                        draw_node(scene, doc, *child, camera, view, child_depth, images, backdrops, cached, size);
                    }
                    scene.push_blend_layer(peniko::Compose::DestIn.into());
                    let mask_transform = camera * doc.world_transform(first_id);
                    if first.opacity < 0.999 {
                        scene.push_opacity_layer(first.opacity);
                    }
                    if first.mask_type == MaskType::Luminance {
                        let mut luminance_mask = first.clone();
                        for paint in luminance_mask.fills.iter_mut().chain(&mut luminance_mask.strokes) {
                            convert_paint_to_luminance_alpha(paint);
                        }
                        if let NodeKind::Text(text) = &mut luminance_mask.kind {
                            if let Some(fills) = &mut text.style.fills {
                                fills.iter_mut().for_each(convert_paint_to_luminance_alpha);
                            }
                            for run in &mut text.runs {
                                if let Some(fills) = &mut run.style.fills {
                                    fills.iter_mut().for_each(convert_paint_to_luminance_alpha);
                                }
                            }
                        }
                        draw_content(scene, doc, &luminance_mask, mask_transform, images, None, cached.inner_shadows, camera, size);
                    } else {
                        draw_content(scene, doc, first, mask_transform, images, None, cached.inner_shadows, camera, size);
                        if !first.children.is_empty() {
                            let clip = if first.clip {
                                if let Some((path, rule)) = draw::node_outline(doc, first) {
                                    scene.set_transform(mask_transform);
                                    set_rule(scene, rule);
                                    scene.push_clip_path(&path);
                                    true
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                            draw_paint_runs(scene, doc, first, camera, view, child_depth + 1, images, None, cached, size);
                            if clip {
                                scene.pop_clip();
                            }
                        }
                    }
                    if first.opacity < 0.999 {
                        scene.pop_layer();
                    }
                    scene.pop_layer();
                    scene.pop_layer();
                }
            }
        } else {
            draw_node(scene, doc, first_id, camera, view, child_depth, images, backdrops, cached, size);
        }
    }
}

fn luminance_alpha(source: Color) -> Color {
    let luminance = 0.2126 * source.r + 0.7152 * source.g + 0.0722 * source.b;
    Color::new(1.0, 1.0, 1.0, source.a * luminance.clamp(0.0, 1.0))
}

fn convert_paint_to_luminance_alpha(paint: &mut Paint) {
    match &mut paint.kind {
        PaintKind::Solid { color } => *color = luminance_alpha(*color),
        PaintKind::Gradient(gradient) => {
            for stop in &mut gradient.stops {
                stop.color = luminance_alpha(stop.color);
            }
        }
        _ => {}
    }
}

fn gpu_luminance_paints_supported(paints: &[Paint]) -> bool {
    paints.iter().filter(|paint| paint.visible && paint.opacity > 0.0).count() <= 1
        && paints.iter().all(|paint| {
            !paint.visible
                || paint.blend == BlendMode::Normal
                    && match &paint.kind {
                        PaintKind::Solid { .. } => true,
                        PaintKind::Gradient(gradient) => gradient.stops.iter().all(|stop| (0.0..=1.0).contains(&stop.color.a)),
                        _ => false,
                    }
        })
}

fn gpu_luminance_mask_supported(node: &Node) -> bool {
    node.effects.iter().all(|effect| !effect.visible)
        && node.children.is_empty()
        && !matches!(node.kind, NodeKind::Vector { ref network } if network.regions.iter().any(|region| region.fills.is_some()))
        && node.fills.iter().chain(&node.strokes).filter(|paint| paint.visible && paint.opacity > 0.0).count() == 1
        && gpu_luminance_paints_supported(&node.fills)
        && gpu_luminance_paints_supported(&node.strokes)
        && match &node.kind {
            NodeKind::Text(text) => std::iter::once(&text.style)
                .chain(text.runs.iter().map(|run| &run.style))
                .all(|style| style.fills.as_ref().is_none_or(|fills| gpu_luminance_paints_supported(fills))),
            _ => true,
        }
}

pub fn make_scene(doc: &Document, page: NodeId, view: &View) -> Scene {
    make_scene_with_images(doc, page, view, None)
}

/// Build outline mode as GPU strokes. Effects and fills do not participate in this view.
pub fn make_outline_scene(doc: &Document, page: NodeId, view: &View, options: figcraft_render::Outline) -> Scene {
    let roots = doc.node(page).map(|node| node.paint_order().collect::<Vec<_>>()).unwrap_or_default();
    make_outline_scene_roots(doc, &roots, view, options)
}

/// Outline only the focused subtree, keeping its normal document-space position.
pub fn make_focused_outline_scene(doc: &Document, focus: NodeId, view: &View, options: figcraft_render::Outline) -> Scene {
    make_outline_scene_roots(doc, &[focus], view, options)
}

fn make_outline_scene_roots(doc: &Document, roots: &[NodeId], view: &View, options: figcraft_render::Outline) -> Scene {
    let mut scene = Scene::new(view.width as u16, view.height as u16);
    let camera = Affine::scale(view.scale) * Affine::translate((-view.origin.0, -view.origin.1));
    for id in roots {
        stroke_outline_node(&mut scene, doc, *id, camera, view.scale, options, 0);
    }
    scene
}

#[cfg(test)]
mod outline_gpu_tests {
    use super::*;

    #[test]
    fn focused_outline_omits_sibling_contours() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(100, 50) else { return Ok(()) };
        let mut doc = Document::new("Focused GPU outline");
        let page = doc.current_page.ok_or("page missing")?;
        let focused = doc.create(page, NodeKind::Rectangle, "Focused", 10.0, 10.0, 20.0, 20.0)?;
        let _sibling = doc.create(page, NodeKind::Rectangle, "Sibling", 60.0, 10.0, 20.0, 20.0)?;
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 50 };
        let options = figcraft_render::Outline { hidden: false, bounds: false, light: false };
        gpu.render_with_clear(&make_outline_scene(&doc, page, &view, options), [1.0; 3])?;
        let full = gpu.pixels()?;
        gpu.render_with_clear(&make_focused_outline_scene(&doc, focused, &view, options), [1.0; 3])?;
        let focused_pixels = gpu.pixels()?;
        let sample = |pixels: &[u8], x: usize, y: usize| pixels[(y * 100 + x) * 4];
        assert!(sample(&full, 60, 20) < 230, "full outline should contain the sibling, got {}", sample(&full, 60, 20));
        assert!(sample(&focused_pixels, 60, 20) > 230, "focused outline leaked a sibling");
        assert!(sample(&focused_pixels, 10, 20) < 230, "focused outline lost its contour, got {}", sample(&focused_pixels, 10, 20));
        Ok(())
    }

    #[test]
    fn outline_strokes_visible_hidden_and_bounds_on_gpu() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(100, 70) else { return Ok(()) };
        let mut doc = Document::new("GPU outlines");
        let page = doc.current_page.ok_or("page missing")?;
        let visible = doc.create(page, NodeKind::Rectangle, "visible", 10.0, 10.0, 40.0, 30.0)?;
        let hidden = doc.create(page, NodeKind::Rectangle, "hidden", 60.0, 10.0, 20.0, 30.0)?;
        let _group = doc.create(page, NodeKind::Group, "bounds", 10.0, 50.0, 70.0, 15.0)?;
        doc.node_mut(hidden).ok_or("hidden rectangle missing")?.visible = false;
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 70 };
        let options = figcraft_render::Outline { hidden: false, bounds: false, light: false };
        let scene = make_outline_scene(&doc, page, &view, options);
        gpu.render_with_clear(&scene, [1.0; 3])?;
        let pixels = gpu.pixels()?;
        let red = |pixels: &[u8], x: usize, y: usize| pixels[(y * 100 + x) * 4];
        assert!(red(&pixels, 10, 20) < 180, "visible outline missing");
        assert!(red(&pixels, 20, 20) > 230, "outline mode filled a shape");
        assert!(red(&pixels, 60, 20) > 230, "hidden outline appeared without the option");
        let scene = make_outline_scene(&doc, page, &view, figcraft_render::Outline { hidden: true, ..options });
        gpu.render_with_clear(&scene, [1.0; 3])?;
        let pixels = gpu.pixels()?;
        if let Ok(path) = std::env::var("LIZARD_GPU_OUTLINE_SCREENSHOT") {
            image::save_buffer(path, &pixels, view.width, view.height, image::ColorType::Rgba8)?;
        }
        assert!(red(&pixels, 60, 20) < 180, "hidden outline option did not draw the shape");
        let scene = make_outline_scene(&doc, page, &view, figcraft_render::Outline { bounds: true, ..options });
        gpu.render_with_clear(&scene, [1.0; 3])?;
        let pixels = gpu.pixels()?;
        assert!(red(&pixels, 10, 55) < 180, "group bounds option did not draw the box");
        let scene = make_outline_scene(&doc, page, &view, figcraft_render::Outline { light: true, ..options });
        gpu.render_with_clear(&scene, [0.0; 3])?;
        let pixels = gpu.pixels()?;
        assert!(red(&pixels, 10, 20) > 100, "light outline missing on a dark canvas: {}", red(&pixels, 10, 20));
        assert!(red(&pixels, 20, 20) < 30, "light outline filled a shape");
        let reference = render_view(&doc, page, &view, &Options { outline_mode: true, outline: options, ..Default::default() })
            .ok_or("CPU outline reference failed")?;
        assert!(red(reference.data(), 10, 20) < 180);
        assert!(doc.node(visible).is_some());
        Ok(())
    }
}

#[cfg(test)]
mod focus_gpu_tests {
    use super::*;

    #[test]
    fn focused_root_keeps_its_world_position_and_hides_siblings() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(100, 50) else { return Ok(()) };
        let mut doc = Document::new("GPU focus");
        let page = doc.current_page.ok_or("page missing")?;
        let first = doc.create(page, NodeKind::Rectangle, "first", 10.0, 10.0, 20.0, 20.0)?;
        let second = doc.create(page, NodeKind::Rectangle, "second", 60.0, 10.0, 20.0, 20.0)?;
        doc.node_mut(first).ok_or("first missing")?.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        doc.node_mut(second).ok_or("second missing")?.fills = vec![Paint::solid(Color::new(0.0, 0.0, 1.0, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 50 };
        let scene = gpu.make_scene_roots(&doc, &[second], &view);
        let background = Color::DEFAULT_PAGE;
        gpu.render_with_clear(&scene, [background.r, background.g, background.b])?;
        let pixels = gpu.pixels()?;
        fn pixel(source: &[u8], x: usize, y: usize) -> &[u8] {
            &source[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4]
        }
        assert!(pixel(&pixels, 20, 20)[..3].iter().all(|channel| *channel > 200), "sibling leaked into focus view");
        assert!(pixel(&pixels, 70, 20)[2] > 240 && pixel(&pixels, 70, 20)[0] < 20, "focused root moved or disappeared");
        let reference =
            render_view(&doc, page, &view, &Options { solo: Some(second), ..Default::default() }).ok_or("CPU focus reference unavailable")?;
        for (x, y) in [(20, 20), (70, 20)] {
            assert!(pixel(&pixels, x, y)[..3].iter().zip(&pixel(reference.data(), x, y)[..3]).all(|(a, b)| a.abs_diff(*b) < 8));
        }
        Ok(())
    }
}

#[cfg(test)]
mod selected_outline_gpu_tests {
    use super::*;

    #[test]
    fn selected_outline_and_sibling_backdrop_keep_paint_order_on_gpu() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(100, 80) else { return Ok(()) };
        let mut doc = Document::new("outlined GPU backdrop");
        let page = doc.current_page.ok_or("page missing")?;
        let base = doc.create(page, NodeKind::Rectangle, "dark base", 0.0, 0.0, 50.0, 80.0)?;
        doc.node_mut(base).ok_or("base missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let selected = doc.create(page, NodeKind::Rectangle, "selected red shape", 22.0, 20.0, 30.0, 40.0)?;
        doc.node_mut(selected).ok_or("selection missing")?.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        let pane = doc.create(page, NodeKind::Rectangle, "background blur", 40.0, 10.0, 40.0, 60.0)?;
        let pane_node = doc.node_mut(pane).ok_or("pane missing")?;
        pane_node.fills.clear();
        pane_node.effects = vec![figcraft_doc::Effect { kind: EffectKind::BackgroundBlur(Blur::uniform(8.0)), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 80 };
        let clear = [245.0 / 255.0; 3];
        let index = BackdropIndex::new(&doc, page);
        for outlined in [[selected], [pane]] {
            assert!(!has_unhandled_effects_except_outlined_in_view(&doc, &view, true, &outlined));
            gpu.render_with_backdrop_blurs_indexed_outlined(&doc, page, &view, clear, &index, &outlined, figcraft_render::Outline::default())?;
            let actual = gpu.pixels()?;
            let reference = render_view(&doc, page, &view, &Options { outlined: outlined.to_vec(), ..Default::default() })
                .ok_or("CPU outline backdrop reference unavailable")?;
            for (x, y) in [(35, 30), (45, 30), (55, 40), (75, 40), (90, 40)] {
                let offset = ((y * view.width + x) * 4) as usize;
                for channel in 0..3 {
                    assert!(actual[offset + channel].abs_diff(reference.data()[offset + channel]) <= 24, "outline={outlined:?} at ({x},{y})");
                }
            }
        }
        doc.node_mut(pane).ok_or("pane missing for glass")?.effects =
            vec![figcraft_doc::Effect { kind: EffectKind::Glass(Glass::default()), visible: true }];
        let glass_index = BackdropIndex::new(&doc, page);
        assert!(!has_unhandled_effects_except_outlined_in_view(&doc, &view, true, &[selected]));
        gpu.render_with_backdrop_blurs_indexed_outlined(&doc, page, &view, clear, &glass_index, &[selected], figcraft_render::Outline::default())?;
        let glass_actual = gpu.pixels()?;
        let glass_reference = render_view(&doc, page, &view, &Options { outlined: vec![selected], ..Default::default() })
            .ok_or("CPU outlined glass reference unavailable")?;
        for (x, y) in [(35, 30), (45, 30), (55, 40), (75, 40), (90, 40)] {
            let offset = ((y * view.width + x) * 4) as usize;
            for channel in 0..3 {
                assert!(glass_actual[offset + channel].abs_diff(glass_reference.data()[offset + channel]) <= 40, "outlined glass at ({x},{y})");
            }
        }
        Ok(())
    }

    #[test]
    fn selected_outline_excludes_only_its_replaced_subtree_from_gpu_capability() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("GPU selected outline capability");
        let page = doc.current_page.ok_or("page missing")?;
        let group = doc.create(page, NodeKind::Group, "selected group", 5.0, 5.0, 40.0, 40.0)?;
        let child = doc.create(group, NodeKind::Rectangle, "unsupported child", 0.0, 0.0, 30.0, 30.0)?;
        let sibling = doc.create(page, NodeKind::Rectangle, "sibling", 60.0, 5.0, 30.0, 30.0)?;
        doc.node_mut(child).ok_or("child missing")?.blend = BlendMode::PlusDarker;
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 120, height: 80 };
        assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        assert!(!has_unhandled_effects_except_outlined_in_view(&doc, &view, true, &[group]));
        doc.node_mut(sibling).ok_or("sibling missing")?.blend = BlendMode::PlusDarker;
        assert!(has_unhandled_effects_except_outlined_in_view(&doc, &view, true, &[group]));
        Ok(())
    }

    #[test]
    fn selected_outline_keeps_a_siblings_cached_blur() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(120, 80) else { return Ok(()) };
        let mut doc = Document::new("GPU selected outline with sibling blur");
        let page = doc.current_page.ok_or("page missing")?;
        let selected = doc.create(page, NodeKind::Rectangle, "selected", 5.0, 20.0, 20.0, 20.0)?;
        let sibling = doc.create(page, NodeKind::Rectangle, "blurred sibling", 55.0, 20.0, 25.0, 25.0)?;
        let outlined = doc.node_mut(selected).ok_or("selected shape missing")?;
        outlined.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        outlined.effects = vec![figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(10.0)), visible: true }];
        let other = doc.node_mut(sibling).ok_or("sibling missing")?;
        other.fills = vec![Paint::solid(Color::new(0.0, 0.0, 1.0, 1.0))];
        other.effects = vec![figcraft_doc::Effect { kind: EffectKind::LayerBlur(Blur::uniform(10.0)), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 120, height: 80 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        gpu.prepare_static_effects(&doc)?;
        assert!(gpu.layer_blurs.contains_key(&selected));
        assert!(gpu.layer_blurs.contains_key(&sibling));
        let scene = gpu.make_scene_selected_outlines(&doc, page, &view, &[selected], figcraft_render::Outline::default());
        let background = Color::DEFAULT_PAGE;
        gpu.render_with_clear(&scene, [background.r, background.g, background.b])?;
        let actual = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options { outlined: vec![selected], ..Default::default() })
            .ok_or("CPU selected outline reference unavailable")?;
        let changed = actual
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        assert!(changed * 100 < view.width as usize * view.height as usize, "selected outline lost sibling blur: {changed} pixels");
        let center = ((30 * view.width + 15) * 4) as usize;
        assert!(actual[center..center + 3].iter().all(|channel| *channel > 200), "selected fill was restored from a cache");
        Ok(())
    }

    #[test]
    fn selected_shape_is_an_outline_while_its_sibling_stays_filled() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(100, 50) else { return Ok(()) };
        let mut doc = Document::new("GPU selected outlines");
        let page = doc.current_page.ok_or("page missing")?;
        let first = doc.create(page, NodeKind::Rectangle, "first", 10.0, 10.0, 20.0, 20.0)?;
        let second = doc.create(page, NodeKind::Rectangle, "second", 60.0, 10.0, 20.0, 20.0)?;
        doc.node_mut(first).ok_or("first missing")?.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        doc.node_mut(second).ok_or("second missing")?.fills = vec![Paint::solid(Color::new(0.0, 0.0, 1.0, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 50 };
        let scene = gpu.make_scene_selected_outlines(&doc, page, &view, &[first], figcraft_render::Outline::default());
        let background = Color::DEFAULT_PAGE;
        gpu.render_with_clear(&scene, [background.r, background.g, background.b])?;
        let pixels = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options { outlined: vec![first], ..Default::default() })
            .ok_or("CPU selected outline reference unavailable")?;
        for (x, y) in [(10, 20), (20, 20), (70, 20)] {
            let index = (y * 100 + x) * 4;
            assert!(
                pixels[index..index + 3].iter().zip(&reference.data()[index..index + 3]).all(|(a, b)| a.abs_diff(*b) < 12),
                "GPU and CPU differ at {x},{y}"
            );
        }
        doc.node_mut(first).ok_or("first missing")?.visible = false;
        let options = figcraft_render::Outline { hidden: true, ..Default::default() };
        let scene = gpu.make_scene_selected_outlines(&doc, page, &view, &[first], options);
        gpu.render_with_clear(&scene, [background.r, background.g, background.b])?;
        let pixels = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options { outlined: vec![first], outline: options, ..Default::default() })
            .ok_or("CPU hidden selected outline unavailable")?;
        let index = (20 * 100 + 10) * 4;
        assert!(pixels[index..index + 3].iter().zip(&reference.data()[index..index + 3]).all(|(a, b)| a.abs_diff(*b) < 12));
        Ok(())
    }
}

#[cfg(test)]
mod focused_selected_outline_tests {
    use super::*;

    #[test]
    fn focused_selected_outline_ignores_outside_and_selected_unsupported_paints() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(110, 60) else { return Ok(()) };
        let mut doc = Document::new("GPU focused selected outline");
        let page = doc.current_page.ok_or("page missing")?;
        let outside = doc.create(page, NodeKind::Rectangle, "outside", 0.0, 0.0, 110.0, 60.0)?;
        let outer = doc.node_mut(outside).ok_or("outside missing")?;
        outer.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        outer.blend = BlendMode::PlusDarker;
        let focus = doc.create(page, NodeKind::Group, "focus", 5.0, 5.0, 90.0, 50.0)?;
        let selected = doc.create(focus, NodeKind::Rectangle, "selected", 10.0, 10.0, 25.0, 25.0)?;
        let selected_node = doc.node_mut(selected).ok_or("selected missing")?;
        selected_node.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        selected_node.blend = BlendMode::PlusDarker;
        let sibling = doc.create(focus, NodeKind::Rectangle, "sibling", 50.0, 10.0, 25.0, 25.0)?;
        doc.node_mut(sibling).ok_or("sibling missing")?.fills = vec![Paint::solid(Color::new(0.0, 0.0, 1.0, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 110, height: 60 };
        assert!(has_unhandled_effects_except_outlined_in_focused_subtree(&doc, &view, true, focus, &[]));
        assert!(!has_unhandled_effects_except_outlined_in_focused_subtree(&doc, &view, true, focus, &[selected]));
        let options = figcraft_render::Outline::default();
        let scene = gpu.make_scene_roots_outlined(&doc, &[focus], &view, &[selected], options);
        let background = Color::DEFAULT_PAGE;
        gpu.render_with_clear(&scene, [background.r, background.g, background.b])?;
        let actual = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options { solo: Some(focus), outlined: vec![selected], ..Default::default() })
            .ok_or("focused selected-outline reference unavailable")?;
        for (x, y) in [(15, 25), (25, 25), (65, 25), (95, 25)] {
            let index = ((y * view.width + x) * 4) as usize;
            for channel in 0..3 {
                assert!(
                    actual[index + channel].abs_diff(reference.data()[index + channel]) <= 20,
                    "focused selected outline at ({x}, {y}) channel {channel}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn focused_selected_outline_keeps_ordered_backdrop_inside_focus() -> Result<(), Box<dyn Error>> {
        let Ok(mut gpu) = GpuCanvas::new(110, 80) else { return Ok(()) };
        let mut doc = Document::new("GPU focused selected backdrop");
        let page = doc.current_page.ok_or("page missing")?;
        let outside = doc.create(page, NodeKind::Rectangle, "outside", 0.0, 0.0, 110.0, 80.0)?;
        doc.node_mut(outside).ok_or("outside missing")?.blend = BlendMode::PlusDarker;
        let focus = doc.create(page, NodeKind::Group, "focus", 5.0, 0.0, 100.0, 80.0)?;
        let base = doc.create(focus, NodeKind::Rectangle, "dark base", 0.0, 0.0, 50.0, 80.0)?;
        doc.node_mut(base).ok_or("base missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let selected = doc.create(focus, NodeKind::Rectangle, "selected", 22.0, 20.0, 30.0, 40.0)?;
        doc.node_mut(selected).ok_or("selected missing")?.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        let pane = doc.create(focus, NodeKind::Rectangle, "background blur", 40.0, 10.0, 40.0, 60.0)?;
        let pane_node = doc.node_mut(pane).ok_or("pane missing")?;
        pane_node.fills.clear();
        pane_node.effects = vec![figcraft_doc::Effect { kind: EffectKind::BackgroundBlur(Blur::uniform(8.0)), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 110, height: 80 };
        assert!(!has_unhandled_effects_except_outlined_in_focused_subtree(&doc, &view, true, focus, &[selected]));
        let focused = focused_document(&doc, page, focus).ok_or("focused document missing")?;
        let index = BackdropIndex::new(&focused, page);
        assert_eq!(index.visible_ids(&view), vec![pane]);
        let clear = [245.0 / 255.0; 3];
        gpu.render_with_backdrop_blurs_indexed_outlined(&focused, page, &view, clear, &index, &[selected], figcraft_render::Outline::default())?;
        let actual = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options { solo: Some(focus), outlined: vec![selected], ..Default::default() })
            .ok_or("focused selected backdrop reference unavailable")?;
        for (x, y) in [(15, 30), (35, 30), (50, 30), (60, 40), (85, 40), (100, 40)] {
            let offset = ((y * view.width + x) * 4) as usize;
            for channel in 0..3 {
                assert!(actual[offset + channel].abs_diff(reference.data()[offset + channel]) <= 28, "focused backdrop at ({x},{y})");
            }
        }
        Ok(())
    }
}

fn stroke_outline_node(scene: &mut Scene, doc: &Document, id: NodeId, camera: Affine, scale: f64, options: figcraft_render::Outline, depth: usize) {
    if depth > 64 {
        return;
    }
    let Some(node) = doc.node(id) else { return };
    if node.is_mask || node.visible && node.opacity <= 0.0 || !node.visible && !options.hidden {
        return;
    }
    scene.set_transform(camera * doc.world_transform(id));
    scene.set_paint(peniko::Color::new(if options.light { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] }));
    scene.set_stroke(peniko::kurbo::Stroke::new(1.0 / scale.max(1e-6)));
    if !matches!(node.kind, NodeKind::Group)
        && let Some((path, _)) = draw::node_outline(doc, node)
    {
        scene.stroke_path(&path);
    }
    let boxed = matches!(node.kind, NodeKind::Rectangle) && node.corners.uniform() == Some(0.0) || node.kind.is_frame_like();
    if options.bounds && !boxed {
        scene.stroke_path(&node.local_rect().to_path(0.1));
    }
    if !matches!(node.kind, NodeKind::BooleanOp { .. }) && !node.hides_contents() {
        for child in node.paint_order() {
            stroke_outline_node(scene, doc, child, camera, scale, options, depth + 1);
        }
    }
}

pub fn has_effects_in_view(doc: &Document, view: &View) -> bool {
    has_matching_effects_in_view(doc, view, false, false, false)
}

pub fn has_unsupported_effects_in_view(doc: &Document, view: &View) -> bool {
    has_matching_effects_in_view(doc, view, true, false, false)
}

pub fn has_unhandled_effects_in_view(doc: &Document, view: &View) -> bool {
    has_unhandled_effects_with_cached_noise_in_view(doc, view, false)
}

/// Capability check for scenes prepared with [`GpuCanvas::prepare_static_effects`].
pub fn has_unhandled_effects_with_cached_noise_in_view(doc: &Document, view: &View, cached_noise: bool) -> bool {
    has_unhandled_effects_excluding_in_view(doc, view, cached_noise, &HashSet::new())
}

/// Only the rendered page can require a fallback. Other pages may occupy the same document
/// coordinates, but none of their paints or effects enter this scene.
pub fn has_unhandled_effects_on_page_in_view(doc: &Document, page: NodeId, view: &View, cached_noise: bool) -> bool {
    if doc.pages.len() <= 1 {
        return has_unhandled_effects_with_cached_noise_in_view(doc, view, cached_noise);
    }
    let kept: HashSet<_> = doc.subtree(page).into_iter().collect();
    let excluded = doc.ids().filter(|id| !kept.contains(id)).collect();
    has_unhandled_effects_excluding_in_view(doc, view, cached_noise, &excluded)
}

#[cfg(test)]
mod page_capability_tests {
    use super::*;

    #[test]
    fn another_page_does_not_force_cpu_for_the_visible_page() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("independent GPU pages");
        let first = doc.current_page.ok_or("first page missing")?;
        let second = doc.add_page("Other page");
        let unsupported = doc.create(second, NodeKind::Rectangle, "Unsupported blend", 0.0, 0.0, 64.0, 64.0)?;
        doc.node_mut(unsupported).ok_or("blend node missing")?.blend = BlendMode::PlusDarker;
        let visible = doc.create(first, NodeKind::Rectangle, "Visible GPU shape", 0.0, 0.0, 64.0, 64.0)?;
        doc.node_mut(visible).ok_or("visible shape missing")?.fills = vec![Paint::solid(Color::new(0.1, 0.7, 0.3, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 64, height: 64 };
        assert!(has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        assert!(!has_unhandled_effects_on_page_in_view(&doc, first, &view, true));
        assert!(has_unhandled_effects_on_page_in_view(&doc, second, &view, true));
        assert!(!has_unhandled_effects_except_outlined_on_page_in_view(&doc, first, &view, true, &[visible]));
        Ok(())
    }

    #[test]
    fn hidden_ancestor_does_not_force_cpu_for_unsupported_descendants() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("hidden unsupported subtree");
        let page = doc.current_page.ok_or("page missing")?;
        let group = doc.create(page, NodeKind::Group, "Hidden group", 0.0, 0.0, 64.0, 64.0)?;
        let child = doc.create(group, NodeKind::Rectangle, "Unsupported child", 0.0, 0.0, 64.0, 64.0)?;
        doc.node_mut(child).ok_or("child missing")?.blend = BlendMode::PlusDarker;
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 64, height: 64 };
        assert!(has_unhandled_effects_on_page_in_view(&doc, page, &view, true));
        doc.node_mut(group).ok_or("group missing")?.visible = false;
        assert!(!has_unhandled_effects_on_page_in_view(&doc, page, &view, true));
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let mut gpu = GpuCanvas::new(view.width, view.height)?;
        gpu.render_submit_document(&doc, page, &view, [245.0 / 255.0; 3])?;
        let actual = gpu.pixels()?;
        let center = ((32 * view.width + 32) * 4) as usize;
        assert!(actual[center..center + 3].iter().zip(&reference.data()[center..center + 3]).all(|(a, b)| a.abs_diff(*b) <= 8));
        doc.node_mut(child).ok_or("child missing")?.blend = BlendMode::Normal;
        let mut shadow = figcraft_doc::Effect::drop_shadow();
        if let EffectKind::DropShadow(value) = &mut shadow.kind {
            value.blend = BlendMode::PlusDarker;
        }
        doc.node_mut(child).ok_or("child missing")?.effects = vec![shadow];
        assert!(!has_unhandled_effects_on_page_in_view(&doc, page, &view, true));
        doc.node_mut(group).ok_or("group missing")?.visible = true;
        assert!(has_unhandled_effects_on_page_in_view(&doc, page, &view, true));
        Ok(())
    }
}

/// Selected outline roots replace their entire subtrees, so those paints and effects cannot
/// require a CPU fallback. All other visible nodes keep the normal capability check.
pub fn has_unhandled_effects_except_outlined_in_view(doc: &Document, view: &View, cached_noise: bool, outlined: &[NodeId]) -> bool {
    let excluded = outlined.iter().flat_map(|id| doc.subtree(*id)).collect();
    has_unhandled_effects_excluding_in_view(doc, view, cached_noise, &excluded)
}

pub fn has_unhandled_effects_except_outlined_on_page_in_view(
    doc: &Document,
    page: NodeId,
    view: &View,
    cached_noise: bool,
    outlined: &[NodeId],
) -> bool {
    let mut excluded: HashSet<_> = outlined.iter().flat_map(|id| doc.subtree(*id)).collect();
    if doc.pages.len() > 1 {
        let kept: HashSet<_> = doc.subtree(page).into_iter().collect();
        excluded.extend(doc.ids().filter(|id| !kept.contains(id)));
    }
    has_unhandled_effects_excluding_in_view(doc, view, cached_noise, &excluded)
}

/// Focus mode only draws the selected subtree; unsupported paints in its siblings do not
/// require a CPU fallback for this view.
pub fn has_unhandled_effects_in_focused_subtree(doc: &Document, view: &View, cached_noise: bool, focus: NodeId) -> bool {
    has_unhandled_effects_except_outlined_in_focused_subtree(doc, view, cached_noise, focus, &[])
}

/// Focused selected-outline mode skips siblings outside focus and the painted contents of
/// selected subtrees. Neither can force a CPU render for pixels absent from this view.
pub fn has_unhandled_effects_except_outlined_in_focused_subtree(
    doc: &Document,
    view: &View,
    cached_noise: bool,
    focus: NodeId,
    outlined: &[NodeId],
) -> bool {
    let kept: HashSet<_> = doc.subtree(focus).into_iter().collect();
    let mut excluded: HashSet<_> = doc.ids().filter(|id| !kept.contains(id)).collect();
    excluded.extend(outlined.iter().flat_map(|id| doc.subtree(*id)));
    has_unhandled_effects_excluding_in_view(doc, view, cached_noise, &excluded)
}

fn has_unhandled_effects_excluding_in_view(doc: &Document, view: &View, cached_noise: bool, excluded: &HashSet<NodeId>) -> bool {
    if has_matching_effects_excluding_in_view(doc, view, true, true, cached_noise, excluded) {
        return true;
    }
    let rect = view.page_rect();
    doc.nodes().any(|node| {
        if !node.visible || excluded.contains(&node.id) {
            return false;
        }
        let unsupported_paint = |paint: &Paint| {
            paint.visible
                && paint.opacity > 0.0
                && ((paint.blend != BlendMode::Normal && !gpu_simple_paint_blend_supported(node, paint))
                    || match &paint.kind {
                        PaintKind::Gradient(_) => false,
                        PaintKind::Image(_) => false,
                        PaintKind::Pattern(fill) => gpu_pattern_source(doc, fill).is_none(),
                        // Registration validates once; parsing WGSL in this per-frame scan
                        // would repeat for every node during every pan and zoom update.
                        PaintKind::Shader(fill) => !doc.shaders.contains_key(&fill.shader_id),
                        PaintKind::Solid { .. } => false,
                    })
        };
        let unsupported = !gpu_styled_text_paints_supported(node)
            || (node.is_mask && node.mask_type == MaskType::Luminance && !gpu_luminance_mask_supported(node))
            || (!matches!(node.blend, BlendMode::Normal | BlendMode::PassThrough) && !gpu_simple_node_blend_supported(node))
            || node.fills.iter().chain(&node.strokes).any(unsupported_paint)
            || matches!(&node.kind, NodeKind::Vector { network }
                if network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().any(unsupported_paint));
        unsupported
            && !has_hidden_ancestor(doc, node)
            && (if node.is_mask {
                node.parent.and_then(|parent| doc.world_render_bounds(parent))
            } else {
                figcraft_render::raster_bounds(doc, node.id)
            })
            .is_none_or(|bounds| bounds.intersect(rect).area() > 0.0)
    })
}

fn has_hidden_ancestor(doc: &Document, node: &Node) -> bool {
    let mut parent = node.parent;
    for _ in 0..128 {
        let Some(id) = parent else { return false };
        let Some(ancestor) = doc.node(id) else { return true };
        if !ancestor.visible {
            return true;
        }
        parent = ancestor.parent;
    }
    true
}

/// The isolated GPU grain pass samples in the node's local space, including rotations.
/// Effect stacks and texture scatter have separate alignment requirements below.
fn gpu_noise_supported(doc: &Document, node: &Node, noise: &Noise) -> bool {
    noise.blend == BlendMode::Normal
        && noise.density.is_finite()
        && noise.opacity.is_finite()
        && node.effects.iter().filter(|effect| effect.visible).count() == 1
        && gpu_isolated_grain_supported(doc, node)
}

fn gpu_stacked_blur_noise_supported(doc: &Document, node: &Node) -> bool {
    let visible = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
    visible.len() >= 2
        && visible.iter().any(|effect| matches!(effect.kind, EffectKind::LayerBlur(_) | EffectKind::Noise(_)))
        && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::DropShadow(_))).count() <= 1
        && visible.iter().all(|effect| match &effect.kind {
            EffectKind::LayerBlur(blur) => !blur.is_progressive() && blur.radius.is_finite() && (0.0..=80.0).contains(&blur.radius),
            EffectKind::Noise(noise) => noise.blend == BlendMode::Normal && noise.density.is_finite() && noise.opacity.is_finite(),
            EffectKind::DropShadow(shadow) => {
                shadow.blend == BlendMode::Normal
                    && shadow.spread.is_finite()
                    && (-32.0..=32.0).contains(&shadow.spread)
                    && shadow.radius.is_finite()
                    && (0.0..=32.0).contains(&shadow.radius)
                    && shadow.offset.0.is_finite()
                    && shadow.offset.1.is_finite()
                    && shadow_cache_offset_aligned(doc, node, shadow)
            }
            _ => false,
        })
        && matches!(
            &node.kind,
            NodeKind::Rectangle
                | NodeKind::Ellipse { .. }
                | NodeKind::Polygon { .. }
                | NodeKind::Star { .. }
                | NodeKind::Vector { .. }
                | NodeKind::Group
        )
        && (gpu_axis_aligned(doc, node)
            || visible.iter().all(|effect| !matches!(effect.kind, EffectKind::Noise(_)))
            || visible.iter().all(|effect| matches!(effect.kind, EffectKind::LayerBlur(_) | EffectKind::Noise(_)))
            || (visible.len() == 3
                && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::LayerBlur(_))).count() == 1
                && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::Noise(_))).count() == 1
                && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::DropShadow(_))).count() == 1))
        && gpu_isolated_grain_supported(doc, node)
}

fn gpu_stacked_blur_texture_supported(doc: &Document, node: &Node) -> bool {
    let visible = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
    visible.len() >= 2
        && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::Texture(_))).count() == 1
        && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::DropShadow(_))).count() <= 2
        && visible.iter().all(|effect| match &effect.kind {
            EffectKind::LayerBlur(blur) => !blur.is_progressive() && blur.radius.is_finite() && (0.0..=32.0).contains(&blur.radius),
            EffectKind::Noise(noise) => noise.blend == BlendMode::Normal && noise.density.is_finite() && noise.opacity.is_finite(),
            EffectKind::Texture(texture) => texture.radius.is_finite() && (0.5..=16.0).contains(&texture.radius),
            EffectKind::DropShadow(shadow) => {
                shadow.blend == BlendMode::Normal
                    && shadow.spread.is_finite()
                    && (-32.0..=32.0).contains(&shadow.spread)
                    && shadow.radius.is_finite()
                    && (0.0..=32.0).contains(&shadow.radius)
                    && shadow.offset.0.is_finite()
                    && shadow.offset.1.is_finite()
                    && shadow_cache_offset_aligned(doc, node, shadow)
            }
            _ => false,
        })
        && (0.0..=1.0).contains(&node.opacity)
        && (node.blend == BlendMode::Normal || matches!(node.kind, NodeKind::Group) && node.blend == BlendMode::PassThrough)
        && matches!(node.kind, NodeKind::Rectangle | NodeKind::Ellipse { .. } | NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::Vector { .. } | NodeKind::Group)
        && gpu_isolated_grain_supported(doc, node)
}

fn gpu_glass_stack_supported(doc: &Document, node: &Node) -> bool {
    let visible = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
    visible.len() >= 2
        && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::Glass(_))).count() == 1
        && visible.iter().filter(|effect| matches!(effect.kind, EffectKind::DropShadow(_))).count() <= 1
        && visible.iter().all(|effect| match &effect.kind {
            EffectKind::Glass(_) => true,
            EffectKind::LayerBlur(blur) => !blur.is_progressive() && blur.radius.is_finite() && (0.0..=32.0).contains(&blur.radius),
            EffectKind::Noise(noise) => noise.blend == BlendMode::Normal && noise.density.is_finite() && noise.opacity.is_finite(),
            EffectKind::DropShadow(shadow) => {
                gpu_drop_shadow_supported(doc, node, shadow) && (0.0..=32.0).contains(&shadow.radius) && (0.0..=32.0).contains(&shadow.spread)
            }
            _ => false,
        })
        && node.fills.iter().all(|paint| !paint.visible || paint.blend == BlendMode::Normal && matches!(paint.kind, PaintKind::Solid { .. }))
        && node.strokes.is_empty()
}

fn gpu_glass_shadow_supported(doc: &Document, node: &Node) -> bool {
    let visible = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
    visible.len() >= 2
        && visible.iter().any(|effect| matches!(effect.kind, EffectKind::Glass(_)))
        && visible.iter().any(|effect| match &effect.kind {
            EffectKind::DropShadow(shadow) => gpu_drop_shadow_supported(doc, node, shadow),
            _ => false,
        })
        && (visible.len() == 2 || gpu_glass_stack_supported(doc, node))
}

fn gpu_multiple_drop_shadows_supported(doc: &Document, node: &Node) -> bool {
    let effects = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
    effects.len() > 1
        && effects.iter().all(|effect| match &effect.kind {
            EffectKind::DropShadow(shadow) => gpu_drop_shadow_supported(doc, node, shadow) || gpu_vector_drop_shadow_supported(node, shadow),
            _ => false,
        })
}

fn gpu_texture_cache_supported(doc: &Document, node: &Node, texture: &Texture) -> bool {
    texture.radius.is_finite() && texture.radius >= 0.5 && gpu_isolated_grain_supported(doc, node)
}

fn gpu_axis_aligned(doc: &Document, node: &Node) -> bool {
    let [a, b, c, d, _, _] = doc.world_transform(node.id).as_coeffs();
    a.abs() > 1e-6 && d.abs() > 1e-6 && b.abs() < 1e-6 && c.abs() < 1e-6
}

fn gpu_texture_view_supported(doc: &Document, node: &Node, view: &View, texture: &Texture) -> bool {
    if !gpu_texture_cache_supported(doc, node, texture)
        || node.effects.iter().filter(|effect| effect.visible).count() != 1
        || !view.scale.is_finite()
        || view.scale <= 0.0
        || view.scale > 2.0
    {
        return false;
    }
    let Some(bounds) = figcraft_render::raster_bounds(doc, node.id) else { return false };
    let bounds = Rect::new(bounds.x0.floor(), bounds.y0.floor(), bounds.x1.ceil(), bounds.y1.ceil());
    let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
    // A fractional camera origin interpolates the cached grain. That changes a few edge
    // samples but keeps the shape and effect stable during smooth pan without a CPU frame.
    scale == 2.0 && bounds.width().ceil() * bounds.height().ceil() <= 2_000_000.0
}

fn gpu_cached_shadow_bounds_supported(doc: &Document, node: &Node) -> bool {
    node.visible
        && !node.is_mask
        && (node.opacity >= 0.999
            || (0.0..=1.0).contains(&node.opacity) && matches!(node.kind, NodeKind::Rectangle | NodeKind::Frame | NodeKind::Group))
        && matches!(node.blend, BlendMode::Normal | BlendMode::PassThrough)
        && node.effects.iter().filter(|effect| effect.visible).count() == 1
        && figcraft_render::raster_bounds(doc, node.id).is_some_and(|bounds| {
            let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
            bounds.width().is_finite() && bounds.height().is_finite() && bounds.width() > 0.0 && bounds.height() > 0.0 && scale >= 0.5
        })
}

fn gpu_cached_drop_shadow_supported(doc: &Document, node: &Node, shadow: &Shadow) -> bool {
    (matches!(node.kind, NodeKind::Rectangle | NodeKind::Frame)
        || gpu_translucent_vector_shadow_supported(node, shadow)
        || gpu_stroked_shape_shadow_supported(doc, node, shadow)
        || gpu_simple_group_shadow_supported(doc, node, shadow))
        && shadow.radius.is_finite()
        && shadow.radius >= 0.0
        && shadow.spread.is_finite()
        && shadow.offset.0.is_finite()
        && shadow.offset.1.is_finite()
        && shadow.blend == BlendMode::Normal
        && gpu_cached_shadow_bounds_supported(doc, node)
}

fn gpu_stroked_shape_shadow_supported(doc: &Document, node: &Node, shadow: &Shadow) -> bool {
    matches!(
        node.kind,
        NodeKind::Rectangle | NodeKind::Ellipse { .. } | NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::Vector { .. } | NodeKind::Line
    ) && node.children.is_empty()
        && node.has_visible_stroke()
        && draw::stroke_fill(doc, node).is_some()
        && node.opacity >= 0.999
        && node.blend == BlendMode::Normal
        && node.effects.iter().filter(|effect| effect.visible).count() == 1
        && (0.0..=32.0).contains(&shadow.spread)
        && (shadow.spread == 0.0
            || node.stroke.dashes.is_empty() && node.stroke.sides.is_none() && node.stroke.width_profile.is_none() && node.stroke.brush.is_none())
        && (0.0..=32.0).contains(&shadow.radius)
        && node.fills.iter().chain(&node.strokes).all(|paint| {
            !paint.visible
                || paint.blend == BlendMode::Normal && paint.opacity >= 0.999 && matches!(paint.kind, PaintKind::Solid { color } if color.a >= 0.999)
        })
        && draw::node_outline(doc, node).is_some()
}

fn gpu_translucent_vector_shadow_supported(node: &Node, shadow: &Shadow) -> bool {
    (matches!(&node.kind, NodeKind::Vector { network } if network.regions.iter().all(|region| region.fills.is_none()))
        || matches!(node.kind, NodeKind::Ellipse { start, sweep, inner }
            if start == 0.0 && (sweep - std::f64::consts::TAU).abs() < 1e-6 && inner == 0.0)
        || matches!(node.kind, NodeKind::Polygon { .. } | NodeKind::Star { .. }))
        && node.children.is_empty()
        && node.strokes.is_empty()
        && node.fills.len() == 1
        && node.fills.iter().all(|paint| {
            paint.visible
                && paint.blend == BlendMode::Normal
                && matches!(paint.kind, PaintKind::Solid { color } if color.a * paint.opacity > 0.0 && color.a * paint.opacity < 0.999)
        })
        && node.opacity >= 0.999
        && node.blend == BlendMode::Normal
        && node.effects.iter().filter(|effect| effect.visible).count() == 1
        && shadow.spread == 0.0
        && shadow.blend == BlendMode::Normal
        && shadow.radius.is_finite()
        && (0.0..=32.0).contains(&shadow.radius)
        && shadow.offset.0.is_finite()
        && shadow.offset.1.is_finite()
}

fn gpu_cached_inner_shadow_supported(doc: &Document, node: &Node, shadow: &Shadow) -> bool {
    (matches!(node.kind, NodeKind::Rectangle | NodeKind::Frame) && node.corners.uniform().is_some()
        || gpu_path_inner_shadow_supported(doc, node, shadow)
        || matches!(node.kind, NodeKind::Group) && gpu_simple_group_shadow_supported(doc, node, shadow))
        && shadow.radius.is_finite()
        && shadow.radius >= 0.0
        && shadow.spread.is_finite()
        && shadow.offset.0.is_finite()
        && shadow.offset.1.is_finite()
        && shadow.blend == BlendMode::Normal
        && gpu_cached_shadow_bounds_supported(doc, node)
}

fn gpu_path_inner_shadow_supported(doc: &Document, node: &Node, shadow: &Shadow) -> bool {
    (matches!(&node.kind, NodeKind::Vector { network } if network.regions.iter().all(|region| region.fills.is_none()))
        || matches!(node.kind, NodeKind::Ellipse { start, sweep, inner }
            if start == 0.0 && (sweep - std::f64::consts::TAU).abs() < 1e-6 && inner == 0.0)
        || matches!(node.kind, NodeKind::Polygon { .. } | NodeKind::Star { .. }))
        && node.children.is_empty()
        && node.strokes.is_empty()
        && node.fills.len() == 1
        && node.fills.iter().all(|paint| {
            paint.visible && paint.blend == BlendMode::Normal && matches!(paint.kind, PaintKind::Solid { color } if color.a * paint.opacity > 0.0)
        })
        && node.opacity >= 0.999
        && node.blend == BlendMode::Normal
        && node.effects.iter().filter(|effect| effect.visible).count() == 1
        && shadow.spread == 0.0
        && shadow.radius.is_finite()
        && (0.0..=32.0).contains(&shadow.radius)
        && shadow.offset.0.is_finite()
        && shadow.offset.1.is_finite()
        && shadow_cache_offset_aligned(doc, node, shadow)
        && shadow.blend == BlendMode::Normal
}

fn shadow_cache_offset_aligned(doc: &Document, node: &Node, shadow: &Shadow) -> bool {
    figcraft_render::raster_bounds(doc, node.id).is_some_and(|bounds| {
        let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
        [shadow.offset.0 * scale, shadow.offset.1 * scale]
            .into_iter()
            .all(|offset| offset.is_finite() && offset.abs() <= 16384.0 && (offset - offset.round()).abs() < 1e-5)
    })
}

fn gpu_simple_group_shadow_supported(doc: &Document, node: &Node, shadow: &Shadow) -> bool {
    matches!(node.kind, NodeKind::Group)
        && !node.is_mask
        && !node.clip
        && !node.children.is_empty()
        && node.fills.is_empty()
        && node.strokes.is_empty()
        && (0.0..=1.0).contains(&node.opacity)
        && matches!(node.blend, BlendMode::Normal | BlendMode::PassThrough)
        && node.effects.iter().filter(|effect| effect.visible).count() == 1
        && shadow.spread.is_finite()
        && figcraft_render::raster_bounds(doc, node.id).is_some_and(|bounds| {
            let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
            (shadow.spread * scale).abs() <= 64.0
        })
        && shadow.radius.is_finite()
        && (0.0..=32.0).contains(&shadow.radius)
        && shadow.offset.0.is_finite()
        && shadow.offset.1.is_finite()
        && shadow_cache_offset_aligned(doc, node, shadow)
        && shadow.blend == BlendMode::Normal
        && node.children.iter().all(|id| doc.node(*id).is_some_and(|child| gpu_simple_group_child_supported(doc, child, 0)))
}

fn gpu_simple_group_child_supported(doc: &Document, child: &Node, depth: usize) -> bool {
    if depth >= 8 || !child.visible || child.clip || !child.effects.is_empty() {
        return false;
    }
    if matches!(child.kind, NodeKind::Group) {
        return (!child.is_mask || matches!(child.mask_type, MaskType::Alpha | MaskType::Vector))
            && matches!(child.blend, BlendMode::Normal | BlendMode::PassThrough)
            && child.fills.is_empty()
            && child.strokes.is_empty()
            && !child.children.is_empty()
            && child.children.iter().all(|id| doc.node(*id).is_some_and(|node| gpu_simple_group_child_supported(doc, node, depth + 1)));
    }
    child.children.is_empty()
        && (!child.is_mask || matches!(child.mask_type, MaskType::Alpha | MaskType::Vector))
        && (child.strokes.is_empty() || draw::stroke_fill(doc, child).is_some())
        && child.blend == BlendMode::Normal
        && matches!(
            child.kind,
            NodeKind::Rectangle | NodeKind::Ellipse { .. } | NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::Vector { .. }
        )
        && child.fills.len() <= 1
        && !(child.fills.is_empty() && child.strokes.is_empty())
        && child
            .fills
            .iter()
            .chain(&child.strokes)
            .all(|paint| paint.visible && paint.blend == BlendMode::Normal && matches!(&paint.kind, PaintKind::Solid { .. } | PaintKind::Gradient(_)))
        && draw::node_outline(doc, child).is_some()
}

fn gpu_isolated_grain_supported(doc: &Document, node: &Node) -> bool {
    let [a, b, c, d, _, _] = doc.world_transform(node.id).as_coeffs();
    let bounds = figcraft_render::raster_bounds(doc, node.id);
    let visible_effect_count = node.effects.iter().filter(|effect| effect.visible).count();
    let partial_grain = (0.0..=1.0).contains(&node.opacity)
        && (node.blend == BlendMode::Normal || matches!(node.kind, NodeKind::Group) && node.blend == BlendMode::PassThrough)
        && (node.children.is_empty() || matches!(node.kind, NodeKind::Group))
        && (visible_effect_count == 1
            && node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::Noise(_) | EffectKind::Texture(_)))
            || visible_effect_count >= 2
                && node
                    .effects
                    .iter()
                    .filter(|effect| effect.visible)
                    .all(|effect| matches!(effect.kind, EffectKind::LayerBlur(_) | EffectKind::Noise(_)))
            || visible_effect_count == 3
                && node.effects.iter().filter(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))).count() == 1
                && node.effects.iter().filter(|effect| effect.visible && matches!(effect.kind, EffectKind::Noise(_))).count() == 1
                && node.effects.iter().filter(|effect| effect.visible && matches!(effect.kind, EffectKind::DropShadow(_))).count() == 1
            || visible_effect_count >= 2
                && node.effects.iter().filter(|effect| effect.visible && matches!(effect.kind, EffectKind::Texture(_))).count() == 1
                && node.effects.iter().filter(|effect| effect.visible).all(|effect| {
                    matches!(effect.kind, EffectKind::LayerBlur(_) | EffectKind::Noise(_) | EffectKind::Texture(_) | EffectKind::DropShadow(_))
                }));
    let paint_supported = |paint: &Paint| {
        !paint.visible || (paint.blend == BlendMode::Normal && matches!(&paint.kind, PaintKind::Solid { .. } | PaintKind::Gradient(_)))
    };
    node.visible
        && !node.is_mask
        && (node.children.is_empty()
            || matches!(node.kind, NodeKind::Group)
                && node.children.iter().all(|id| doc.node(*id).is_some_and(|child| gpu_simple_group_child_supported(doc, child, 0))))
        && (partial_grain || node.opacity >= 0.999 || gpu_cached_node_blend_supported(node))
        && (node.blend == BlendMode::Normal
            || matches!(node.kind, NodeKind::Group) && node.blend == BlendMode::PassThrough
            || gpu_cached_node_blend_supported(node))
        && [a, b, c, d].iter().all(|value| value.is_finite())
        && (a * d - b * c).abs() > 1e-6
        && node.fills.iter().chain(&node.strokes).all(paint_supported)
        && match &node.kind {
            NodeKind::Vector { network } => network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().all(paint_supported),
            _ => true,
        }
        && bounds.is_some_and(|bounds| {
            let width = bounds.width();
            let height = bounds.height();
            let scale = (4096.0 / width.max(height)).min(2.0);
            width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 && scale >= 0.5
        })
}

fn gpu_progressive_layer_blur_supported(doc: &Document, node: &Node, blur: &Blur) -> bool {
    if !blur.is_progressive()
        || !node.visible
        || node.is_mask
        || !node.children.is_empty()
        || !(0.0..=1.0).contains(&node.opacity)
        || node.blend != BlendMode::Normal
        || node.effects.iter().filter(|effect| effect.visible).count() != 1
        || ![blur.radius, blur.start_radius, blur.start.0, blur.start.1, blur.end.0, blur.end.1].into_iter().all(f64::is_finite)
        || node.fills.iter().chain(&node.strokes).any(|paint| !matches!(paint.kind, PaintKind::Solid { .. } | PaintKind::Gradient(_)))
    {
        return false;
    }
    figcraft_render::raster_bounds(doc, node.id).is_some_and(|bounds| {
        let width = bounds.width();
        let height = bounds.height();
        let scale = (4096.0 / width.max(height)).min(2.0);
        width.is_finite()
            && height.is_finite()
            && width > 0.0
            && height > 0.0
            && scale >= 0.5
            && (width * scale).ceil() * (height * scale).ceil() <= 2_000_000.0
    })
}

fn gpu_glass_supported(doc: &Document, node: &Node, view: &View, glass: &Glass) -> bool {
    if !matches!(node.kind, NodeKind::Rectangle | NodeKind::Ellipse { .. } | NodeKind::Vector { .. })
        || !node.children.is_empty()
        || (node.effects.iter().filter(|effect| effect.visible).count() != 1
            && !gpu_glass_stack_supported(doc, node)
            && !gpu_glass_shadow_supported(doc, node))
        || node.opacity < 0.999
        || node.blend != BlendMode::Normal
        || draw::node_outline(doc, node).is_none()
    {
        return false;
    }
    gpu_glass_region(doc, node.id, view, glass).is_some()
}

fn gpu_glass_region(doc: &Document, id: NodeId, view: &View, glass: &Glass) -> Option<(u16, u16, u16, u16)> {
    let bounds = figcraft_render::backdrop::shape_bounds(doc, id)?;
    let safe = glass.sanitized();
    let layer_blur_reach = doc.node(id).map_or(0.0, |node| {
        node.effects
            .iter()
            .filter(|effect| effect.visible)
            .filter_map(|effect| match &effect.kind {
                EffectKind::LayerBlur(blur) => Some(blur.radius * 1.5),
                _ => None,
            })
            .sum::<f64>()
    });
    let shadow_reach = doc.node(id).map_or(0.0, |node| {
        node.effects
            .iter()
            .filter(|effect| effect.visible)
            .filter_map(|effect| match &effect.kind {
                EffectKind::DropShadow(shadow) => Some(shadow.radius * 3.0 + shadow.spread.abs() + shadow.offset.0.abs().max(shadow.offset.1.abs())),
                _ => None,
            })
            .fold(0.0, f64::max)
    });
    let margin = ((safe.max_bend() + safe.frost * 3.0 + layer_blur_reach + shadow_reach) * view.scale + 24.0).ceil();
    let requested = [
        ((bounds.x0 - view.origin.0) * view.scale - margin).floor(),
        ((bounds.y0 - view.origin.1) * view.scale - margin).floor(),
        ((bounds.x1 - view.origin.0) * view.scale + margin).ceil(),
        ((bounds.y1 - view.origin.1) * view.scale + margin).ceil(),
    ];
    if !requested.into_iter().all(f64::is_finite) {
        return None;
    }
    let [x0, y0, x1, y1] = [
        requested[0].clamp(0.0, f64::from(view.width)),
        requested[1].clamp(0.0, f64::from(view.height)),
        requested[2].clamp(0.0, f64::from(view.width)),
        requested[3].clamp(0.0, f64::from(view.height)),
    ];
    if x1 <= x0 || y1 <= y0 || (x1 - x0) * (y1 - y0) > 2_000_000.0 {
        return None;
    }
    Some((
        u16::try_from(x0 as u32).ok()?,
        u16::try_from(y0 as u32).ok()?,
        u16::try_from((x1 - x0) as u32).ok()?,
        u16::try_from((y1 - y0) as u32).ok()?,
    ))
}

fn has_matching_effects_in_view(doc: &Document, view: &View, unsupported_only: bool, gpu_backdrop: bool, cached_noise: bool) -> bool {
    has_matching_effects_excluding_in_view(doc, view, unsupported_only, gpu_backdrop, cached_noise, &HashSet::new())
}

fn has_matching_effects_excluding_in_view(
    doc: &Document,
    view: &View,
    unsupported_only: bool,
    gpu_backdrop: bool,
    cached_noise: bool,
    excluded: &HashSet<NodeId>,
) -> bool {
    let rect = Rect::new(
        view.origin.0,
        view.origin.1,
        view.origin.0 + f64::from(view.width) / view.scale,
        view.origin.1 + f64::from(view.height) / view.scale,
    );
    doc.nodes().any(|node| {
        node.visible
            && !excluded.contains(&node.id)
            && node.effects.iter().any(|effect| {
                effect.visible
                    && (!unsupported_only
                        || (node.effects.iter().filter(|effect| effect.visible).count() > 1
                            && !gpu_stacked_blur_noise_supported(doc, node)
                            && !gpu_stacked_blur_texture_supported(doc, node)
                            && !gpu_glass_stack_supported(doc, node)
                            && !gpu_glass_shadow_supported(doc, node)
                            && !gpu_multiple_drop_shadows_supported(doc, node))
                        || match &effect.kind {
                            // Only the measured low-opacity analytic rectangle/frame shadow
                            // uses the GPU. Other shadows retain the faithful CPU route.
                            EffectKind::DropShadow(shadow) => {
                                !(gpu_drop_shadow_supported(doc, node, shadow)
                                    || gpu_vector_drop_shadow_supported(node, shadow)
                                    || gpu_backdrop && gpu_glass_shadow_supported(doc, node)
                                    || cached_noise && gpu_stacked_blur_noise_supported(doc, node)
                                    || cached_noise && gpu_stacked_blur_texture_supported(doc, node)
                                    || cached_noise && gpu_cached_drop_shadow_supported(doc, node, shadow))
                            }
                            EffectKind::Noise(noise) => {
                                !cached_noise
                                    || !(gpu_noise_supported(doc, node, noise)
                                        || gpu_stacked_blur_noise_supported(doc, node)
                                        || gpu_stacked_blur_texture_supported(doc, node)
                                        || gpu_backdrop && gpu_glass_stack_supported(doc, node))
                            }
                            EffectKind::InnerShadow(shadow) => !cached_noise || !gpu_cached_inner_shadow_supported(doc, node, shadow),
                            EffectKind::Texture(texture) => {
                                !cached_noise
                                    || !(gpu_texture_view_supported(doc, node, view, texture) || gpu_stacked_blur_texture_supported(doc, node))
                            }
                            EffectKind::Glass(glass) => {
                                !gpu_backdrop
                                    || !gpu_glass_supported(doc, node, view, glass)
                                    || doc.page_of(node.id).is_none_or(|page| backdrop_requires_cpu(doc, node.id, page))
                            }
                            EffectKind::BackgroundBlur(_) => {
                                !gpu_backdrop || doc.page_of(node.id).is_none_or(|page| backdrop_requires_cpu(doc, node.id, page))
                            }
                            EffectKind::LayerBlur(blur) => {
                                blur.is_progressive() && (!cached_noise || !gpu_progressive_layer_blur_supported(doc, node, blur))
                            }
                        })
            })
            && !has_hidden_ancestor(doc, node)
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
    make_scene_with_cached_layers(doc, page, view, images, backdrops, CachedEffects::empty())
}

fn make_scene_with_cached_layers(
    doc: &Document,
    page: NodeId,
    view: &View,
    images: Option<&HashMap<String, ImageInfo>>,
    backdrops: Option<&HashMap<NodeId, ImageInfo>>,
    cached: CachedEffects<'_>,
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
        draw_paint_runs(&mut scene, doc, page_node, camera, world, 0, images, backdrops, cached, (view.width as u16, view.height as u16));
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
    shader_textures: HashMap<String, wgpu::Texture>,
    shader_info: HashMap<String, (u16, u16, String, u32)>,
    pattern_textures: HashMap<String, wgpu::Texture>,
    pattern_info: HashMap<String, (u16, u16, String, u64)>,
    temporary_textures: Vec<wgpu::Texture>,
    temporary_ids: Vec<TextureId>,
    cached_textures: Vec<wgpu::Texture>,
    cached_ids: Vec<TextureId>,
    image_bindings: TextureBindings,
    image_info: HashMap<String, ImageInfo>,
    next_external_id: u64,
    gpu_blur: Option<GpuBlur>,
    gpu_morph: Option<GpuMorph>,
    gpu_glass: Option<GpuGlass>,
    gpu_image_filter: Option<GpuImageFilter>,
    gpu_diamond: Option<GpuDiamond>,
    gpu_color_profile: Option<GpuColorProfile>,
    gpu_progressive_blur: Option<GpuProgressiveBlur>,
    gpu_noise: Option<GpuNoise>,
    gpu_texture: Option<GpuTexture>,
    gpu_shadow_clip: Option<GpuShadowClip>,
    gpu_shader: Option<GpuShader>,
    gpu_silhouette_shadow: Option<GpuSilhouetteShadow>,
    glass_prefix_canvas: Option<Box<GpuCanvas>>,
    glass_mask_canvas: Option<Box<GpuCanvas>>,
    glass_content_canvas: Option<Box<GpuCanvas>>,
    layer_blurs: HashMap<NodeId, CachedLayerBlur>,
    layer_blurs_1x: HashMap<NodeId, CachedLayerBlur>,
    drop_shadows: HashMap<NodeId, CachedLayerBlur>,
    inner_shadows: HashMap<NodeId, CachedLayerBlur>,
    noise_effects: HashMap<NodeId, CachedLayerBlur>,
    texture_effects: HashMap<NodeId, CachedLayerBlur>,
    texture_effects_1x: HashMap<NodeId, CachedLayerBlur>,
    grain_cache: HashMap<NodeId, GrainCacheState>,
}

#[derive(Clone)]
struct CachedLayerBlur {
    id: TextureId,
    width: u16,
    height: u16,
    bounds: Rect,
    placement: Affine,
}

struct GrainCacheState {
    node: Node,
    world: [f64; 6],
    bounds: [f64; 4],
    id: TextureId,
    _texture: wgpu::Texture,
    secondary: Option<(TextureId, wgpu::Texture)>,
}

impl GpuCanvas {
    /// New effect-free geometry can be drawn in the Vello scene without rebaking effects on
    /// existing layers. Verify every cached owner's render inputs and bounds before retaining it.
    pub fn reuse_unaffected_effects(&mut self, before: &Document, after: &Document) -> bool {
        let isolated =
            self.layer_blurs.keys().chain(self.noise_effects.keys()).chain(self.texture_effects.keys()).chain(self.grain_cache.keys()).copied();
        let shape_shadows = self.drop_shadows.keys().chain(self.inner_shadows.keys()).copied();
        cached_effects_survive_geometry_edit(before, after, isolated, shape_shadows)
    }

    /// A duplicate has the same effect pixels as its source. Bind the already prepared GPU
    /// textures to the copy at its new placement; a release still prepares final textures.
    pub fn reuse_duplicated_effects(&mut self, before: &Document, after: &Document) -> bool {
        let Some(pairs) = duplicated_subtrees(before, after) else {
            return false;
        };
        if pairs.iter().any(|(source, _, _)| self.grain_cache.contains_key(source)) {
            return false;
        }
        for cache in [&mut self.layer_blurs, &mut self.layer_blurs_1x, &mut self.drop_shadows, &mut self.inner_shadows] {
            let additions: Vec<_> = pairs
                .iter()
                .filter_map(|(source, copy, delta)| {
                    cache.get(source).cloned().map(|mut effect| {
                        effect.placement = *delta * effect.placement;
                        (*copy, effect)
                    })
                })
                .collect();
            cache.extend(additions);
        }
        true
    }

    /// Keep prepared effect textures when an edit only translates one subtree.
    /// Their pixels are unchanged; only the world-space placement changes.
    pub fn reuse_translated_effects(&mut self, before: &Document, after: &Document) -> bool {
        let Some((root, dx, dy)) = translated_node(before, after) else {
            return false;
        };
        for cache in [
            &mut self.layer_blurs,
            &mut self.layer_blurs_1x,
            &mut self.drop_shadows,
            &mut self.inner_shadows,
            &mut self.noise_effects,
            &mut self.texture_effects,
            &mut self.texture_effects_1x,
        ] {
            for (id, effect) in cache.iter_mut() {
                if after.is_ancestor(root, *id) {
                    effect.placement = Affine::translate((dx, dy)) * effect.placement;
                }
            }
        }
        for (id, state) in &mut self.grain_cache {
            if after.is_ancestor(root, *id)
                && let Some(node) = after.node(*id)
            {
                state.node = node.clone();
                state.world = after.world_transform(*id).as_coeffs();
                for (index, delta) in [(0, dx), (1, dy), (2, dx), (3, dy)] {
                    state.bounds[index] += delta;
                }
            }
        }
        true
    }

    /// Rotate already prepared effect images during a gesture. Rebuild them after release to
    /// restore their native sampling quality at the final angle.
    pub fn reuse_rotated_effects(&mut self, before: &Document, after: &Document) -> bool {
        let Some((root, delta)) = rotated_node(before, after) else {
            return false;
        };
        // The procedural grain caches also encode world coordinates. They need a fresh pass.
        if self.grain_cache.keys().any(|id| after.is_ancestor(root, *id)) {
            return false;
        }
        for cache in [&mut self.layer_blurs, &mut self.layer_blurs_1x, &mut self.drop_shadows, &mut self.inner_shadows] {
            for (id, effect) in cache.iter_mut() {
                if after.is_ancestor(root, *id) {
                    effect.placement = delta * effect.placement;
                }
            }
        }
        true
    }

    /// Uniform Scale changes effect radii with the geometry. A temporary GPU transform gives
    /// the mathematically scaled preview; a fresh texture is prepared when the gesture ends.
    pub fn reuse_scaled_effects(&mut self, before: &Document, after: &Document) -> bool {
        let Some((root, delta)) = scaled_subtree(before, after) else {
            return false;
        };
        if self.grain_cache.keys().any(|id| after.is_ancestor(root, *id)) {
            return false;
        }
        for cache in [&self.layer_blurs, &self.layer_blurs_1x, &self.drop_shadows, &self.inner_shadows] {
            for (id, effect) in cache {
                if after.is_ancestor(root, *id) {
                    let [a, b, _, _, _, _] = (delta * effect.placement).as_coeffs();
                    if !(0.5..=2.0).contains(&a.hypot(b)) {
                        return false;
                    }
                }
            }
        }
        for cache in [&mut self.layer_blurs, &mut self.layer_blurs_1x, &mut self.drop_shadows, &mut self.inner_shadows] {
            for (id, effect) in cache.iter_mut() {
                if after.is_ancestor(root, *id) {
                    effect.placement = delta * effect.placement;
                }
            }
        }
        true
    }

    /// Reuse child effects during a Move-tool resize. The resized shape's own effect is a
    /// temporary affine preview and is rebuilt once the pointer is released.
    pub fn reuse_resized_effects(&mut self, before: &Document, after: &Document) -> bool {
        let Some((root, root_delta)) = resized_shape(before, after) else {
            return false;
        };
        if self.grain_cache.keys().any(|id| after.is_ancestor(root, *id)) {
            return false;
        }
        for cache in [&mut self.layer_blurs, &mut self.layer_blurs_1x, &mut self.drop_shadows, &mut self.inner_shadows] {
            for (id, effect) in cache.iter_mut() {
                if !after.is_ancestor(root, *id) {
                    continue;
                }
                let delta = if *id == root {
                    root_delta
                } else if let Some(delta) = translated_world_delta(before, after, *id) {
                    delta
                } else {
                    return false;
                };
                effect.placement = delta * effect.placement;
            }
        }
        true
    }

    /// Render document-stable effects once on the GPU and reuse them while the camera moves.
    pub fn prepare_static_effects(&mut self, doc: &Document) -> Result<(usize, usize, usize), Box<dyn Error>> {
        self.layer_blurs.clear();
        self.layer_blurs_1x.clear();
        self.drop_shadows.clear();
        self.inner_shadows.clear();
        self.release_cached_textures();
        self.release_temporary_textures();
        self.retain_grain_caches(doc);
        let candidates: Vec<_> = doc
            .nodes()
            .filter_map(|node| {
                let effect = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
                let [effect] = effect.as_slice() else { return None };
                let EffectKind::LayerBlur(blur) = &effect.kind else { return None };
                if !node.visible
                    || node.is_mask
                    || !node.children.is_empty()
                    || !(0.0..=1.0).contains(&node.opacity)
                    || node.blend != BlendMode::Normal
                    || !blur.radius.is_finite()
                    || blur.max_radius() <= 0.0
                    || node.fills.iter().chain(&node.strokes).any(|paint| !matches!(paint.kind, PaintKind::Solid { .. } | PaintKind::Gradient(_)))
                    || (blur.is_progressive() && !gpu_progressive_layer_blur_supported(doc, node, blur))
                {
                    return None;
                }
                let bounds = figcraft_render::raster_bounds(doc, node.id)?;
                if !bounds.width().is_finite() || !bounds.height().is_finite() || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
                    return None;
                }
                let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
                if scale < 0.5 {
                    return None;
                }
                let width = (bounds.width() * scale).ceil() as u32;
                let height = (bounds.height() * scale).ceil() as u32;
                Some((node.id, blur.clone(), bounds, scale, width.max(1), height.max(1)))
            })
            .collect();
        for (id, blur_effect, bounds, scale, width, height) in candidates {
            let mut source_doc = doc.clone();
            if let Some(node) = source_doc.node_mut(id) {
                node.effects.clear();
                node.opacity = 1.0;
            }
            let camera = Affine::scale(scale) * Affine::translate((-bounds.x0, -bounds.y0));
            let mut scene = Scene::new(width as u16, height as u16);
            draw_node(&mut scene, &source_doc, id, camera, bounds, 0, None, None, CachedEffects::empty(), (width as u16, height as u16));
            let mut source = Self::shared_with(self, width, height);
            source
                .render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
            let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
            let texture = if blur_effect.is_progressive() {
                let mix = self.gpu_progressive_blur.get_or_insert_with(|| GpuProgressiveBlur::new(&self.device));
                let Some(node) = doc.node(id) else { continue };
                let local_to_pixels = camera * doc.world_transform(id);
                let safe = blur_effect.sanitized();
                let start = local_to_pixels * Point::new(safe.start.0 * node.width, safe.start.1 * node.height);
                let end = local_to_pixels * Point::new(safe.end.0 * node.width, safe.end.1 * node.height);
                mix.submit(&self.device, &self.queue, blur, &source.texture, &blur_effect, scale, start, end)
            } else {
                blur.submit(&self.device, &self.queue, &source.texture, (blur_effect.radius * scale / 2.0) as f32, 2, None)
            };
            let (Ok(image_width), Ok(image_height)) = (u16::try_from(texture.width()), u16::try_from(texture.height())) else { continue };
            let texture_id = self.register_cached_texture(texture);
            self.layer_blurs
                .insert(id, CachedLayerBlur { id: texture_id, width: image_width, height: image_height, bounds, placement: Affine::IDENTITY });
        }
        let shadow_candidates: Vec<_> = doc
            .nodes()
            .filter_map(|node| {
                let effects = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
                let [effect] = effects.as_slice() else { return None };
                let inner = match &effect.kind {
                    EffectKind::DropShadow(shadow) if shadow.radius.is_finite() && shadow.radius >= 0.0 => false,
                    EffectKind::InnerShadow(shadow) if gpu_cached_inner_shadow_supported(doc, node, shadow) => true,
                    _ => return None,
                };
                if !node.visible
                    || node.is_mask
                    || (node.opacity < 0.999
                        && (!(0.0..=1.0).contains(&node.opacity) || !matches!(node.kind, NodeKind::Rectangle | NodeKind::Frame | NodeKind::Group)))
                    || !matches!(node.blend, BlendMode::Normal | BlendMode::PassThrough)
                {
                    return None;
                }
                let bounds = figcraft_render::raster_bounds(doc, node.id)?;
                let bounds = Rect::new(bounds.x0.floor(), bounds.y0.floor(), bounds.x1.ceil(), bounds.y1.ceil());
                if !bounds.width().is_finite() || !bounds.height().is_finite() || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
                    return None;
                }
                let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
                if scale < 0.5 {
                    return None;
                }
                Some((node.id, bounds, scale, inner))
            })
            .collect();
        for (id, bounds, scale, inner) in shadow_candidates {
            let Some(cached) = self.render_static_shadow(doc, id, bounds, scale, inner)? else { continue };
            if doc.node(id).is_some_and(|node| {
                matches!(node.kind, NodeKind::Group)
                    || node
                        .effects
                        .iter()
                        .any(|effect| matches!(&effect.kind, EffectKind::DropShadow(shadow) if gpu_stroked_shape_shadow_supported(doc, node, shadow)))
            }) {
                self.layer_blurs.insert(id, cached);
            } else if inner {
                self.inner_shadows.insert(id, cached);
            } else {
                self.drop_shadows.insert(id, cached);
            }
        }
        self.prepare_static_noise(doc)?;
        self.prepare_static_texture(doc)?;
        self.prepare_stacked_blur_noise(doc)?;
        Ok((self.layer_blurs.len(), self.drop_shadows.len(), self.inner_shadows.len()))
    }

    /// Apply the layer effects in document order while both intermediate images remain on the GPU.
    fn prepare_stacked_blur_noise(&mut self, doc: &Document) -> Result<(), Box<dyn Error>> {
        let candidates: Vec<_> = doc
            .nodes()
            .filter_map(|node| {
                if !gpu_stacked_blur_noise_supported(doc, node) && !gpu_stacked_blur_texture_supported(doc, node) {
                    return None;
                }
                let effects = node.effects.iter().filter(|effect| effect.visible).map(|effect| effect.kind.clone()).collect::<Vec<_>>();
                let bounds = figcraft_render::raster_bounds(doc, node.id)?;
                let bounds = if gpu_stacked_blur_texture_supported(doc, node) {
                    Rect::new(bounds.x0.floor(), bounds.y0.floor(), bounds.x1.ceil(), bounds.y1.ceil())
                } else {
                    bounds
                };
                let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
                let width = (bounds.width() * scale).ceil() as u32;
                let height = (bounds.height() * scale).ceil() as u32;
                let (Ok(width), Ok(height)) = (u16::try_from(width), u16::try_from(height)) else { return None };
                Some((node.id, effects, bounds, scale, width, height))
            })
            .collect();
        for (id, effects, bounds, scale, width, height) in candidates {
            let primary = self.render_stacked_effects(doc, id, &effects, bounds, scale, width, height)?;
            self.layer_blurs.insert(id, primary);
            if effects.iter().any(|effect| matches!(effect, EffectKind::Texture(_)))
                && scale == 2.0
                && bounds.width() * bounds.height() <= 2_000_000.0
                && let (Ok(width_1x), Ok(height_1x)) = (u16::try_from(bounds.width().ceil() as u32), u16::try_from(bounds.height().ceil() as u32))
            {
                let secondary = self.render_stacked_effects(doc, id, &effects, bounds, 1.0, width_1x, height_1x)?;
                self.layer_blurs_1x.insert(id, secondary);
            }
        }
        Ok(())
    }

    fn render_stacked_effects(
        &mut self,
        doc: &Document,
        id: NodeId,
        effects: &[EffectKind],
        bounds: Rect,
        scale: f64,
        width: u16,
        height: u16,
    ) -> Result<CachedLayerBlur, Box<dyn Error>> {
        let mut source_doc = doc.clone();
        if let Some(node) = source_doc.node_mut(id) {
            node.effects.clear();
            let post_effect_opacity = node.blend == BlendMode::Normal || matches!(node.kind, NodeKind::Group) && node.blend == BlendMode::PassThrough;
            node.blend = BlendMode::Normal;
            if post_effect_opacity && node.opacity < 0.999 {
                node.opacity = 1.0;
            }
        }
        let camera = Affine::scale(scale) * Affine::translate((-bounds.x0, -bounds.y0));
        let mut scene = Scene::new(width, height);
        draw_node(&mut scene, &source_doc, id, camera, bounds, 0, None, None, CachedEffects::empty(), (width, height));
        let mut source = Self::shared_with(self, u32::from(width), u32::from(height));
        source.render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
        let surface_to_local = doc.world_transform(id).inverse() * Affine::translate((bounds.x0, bounds.y0)) * Affine::scale(1.0 / scale);
        let mut result = source.texture.clone();
        for effect in effects {
            result = match effect {
                EffectKind::LayerBlur(blur_effect) => {
                    let pass = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
                    pass.submit(&self.device, &self.queue, &result, (blur_effect.radius * scale / 2.0) as f32, 1, None)
                }
                EffectKind::Noise(noise) => {
                    let pass = self.gpu_noise.get_or_insert_with(|| GpuNoise::new(&self.device));
                    pass.submit(&self.device, &self.queue, &result, noise, surface_to_local, id.0 as u32)
                }
                EffectKind::Texture(texture) => {
                    let pass = self.gpu_texture.get_or_insert_with(|| GpuTexture::new(&self.device));
                    pass.submit(&self.device, &self.queue, &result, texture, surface_to_local, scale, id.0 as u32)
                }
                EffectKind::DropShadow(shadow) => {
                    let spread_px = (shadow.spread * scale).round() as i32;
                    let silhouette = (spread_px > 0)
                        .then(|| doc.node(id).filter(|node| !matches!(node.kind, NodeKind::Group)))
                        .flatten()
                        .and_then(|node| figcraft_render::shadow::silhouette(doc, node));
                    let geometric_caster = if let Some(outlines) = silhouette {
                        let mut mask_scene = Scene::new(width, height);
                        mask_scene.set_transform(camera * doc.world_transform(id));
                        mask_scene.set_paint(peniko::Color::new([1.0; 4]));
                        for (path, rule) in outlines {
                            set_rule(&mut mask_scene, rule);
                            mask_scene.fill_path(&path);
                            mask_scene.set_stroke(
                                peniko::kurbo::Stroke::new(shadow.spread * 2.0)
                                    .with_join(peniko::kurbo::Join::Miter)
                                    .with_caps(peniko::kurbo::Cap::Butt)
                                    .with_miter_limit(figcraft_doc::SPREAD_MITER_LIMIT),
                            );
                            mask_scene.stroke_path(&path);
                        }
                        let mut mask = Self::shared_with(self, u32::from(width), u32::from(height));
                        mask.render_submit_with_target(
                            &mask_scene,
                            TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }),
                        )?;
                        Some(mask.texture.clone())
                    } else {
                        None
                    };
                    let pixel_caster = (spread_px != 0 && geometric_caster.is_none()).then(|| {
                        let morph = self.gpu_morph.get_or_insert_with(|| GpuMorph::new(&self.device));
                        morph.submit(&self.device, &self.queue, &result, spread_px.unsigned_abs(), spread_px > 0)
                    });
                    let caster = geometric_caster.as_ref().or(pixel_caster.as_ref()).unwrap_or(&result);
                    let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
                    let pass = self.gpu_silhouette_shadow.get_or_insert_with(|| GpuSilhouetteShadow::new(&self.device));
                    pass.submit_drop_composited(
                        &self.device,
                        &self.queue,
                        blur,
                        &result,
                        caster,
                        ((shadow.offset.0 * scale) as f32, (shadow.offset.1 * scale) as f32),
                        (shadow.radius * scale / 2.0) as f32,
                        shadow.color,
                        shadow.show_behind,
                    )
                }
                _ => continue,
            };
        }
        let texture_id = self.register_cached_texture(result);
        Ok(CachedLayerBlur { id: texture_id, width, height, bounds, placement: Affine::IDENTITY })
    }

    pub fn cached_noise_count(&self) -> usize {
        self.noise_effects.len()
    }

    pub fn cached_texture_count(&self) -> usize {
        self.texture_effects.len()
    }

    fn retain_grain_caches(&mut self, doc: &Document) {
        let stale: Vec<_> = self
            .grain_cache
            .iter()
            .filter_map(|(id, state)| {
                let same_node = doc.node(*id).is_some_and(|node| node == &state.node);
                let same_world = doc.world_transform(*id).as_coeffs() == state.world;
                let same_bounds =
                    figcraft_render::raster_bounds(doc, *id).is_some_and(|bounds| [bounds.x0, bounds.y0, bounds.x1, bounds.y1] == state.bounds);
                (!same_node || !same_world || !same_bounds).then_some(*id)
            })
            .collect();
        for id in stale {
            if let Some(state) = self.grain_cache.remove(&id) {
                self.image_bindings.remove(state.id);
                if let Some((secondary, _)) = state.secondary {
                    self.image_bindings.remove(secondary);
                }
            }
            self.noise_effects.remove(&id);
            self.texture_effects.remove(&id);
            self.texture_effects_1x.remove(&id);
        }
    }

    fn register_grain_texture(&mut self, doc: &Document, id: NodeId, bounds: Rect, texture: wgpu::Texture) -> TextureId {
        let texture_id = TextureId(self.next_external_id);
        self.next_external_id += 1;
        self.image_bindings.insert(texture_id, texture.create_view(&wgpu::TextureViewDescriptor::default()));
        if let Some(node) = doc.node(id) {
            self.grain_cache.insert(
                id,
                GrainCacheState {
                    node: node.clone(),
                    world: doc.world_transform(id).as_coeffs(),
                    bounds: [bounds.x0, bounds.y0, bounds.x1, bounds.y1],
                    id: texture_id,
                    _texture: texture,
                    secondary: None,
                },
            );
        }
        texture_id
    }

    fn prepare_static_noise(&mut self, doc: &Document) -> Result<(), Box<dyn Error>> {
        let candidates: Vec<_> = doc
            .nodes()
            .filter_map(|node| {
                let effects = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
                let [effect] = effects.as_slice() else { return None };
                let EffectKind::Noise(noise) = &effect.kind else { return None };
                if !gpu_noise_supported(doc, node, noise) {
                    return None;
                }
                let bounds = figcraft_render::raster_bounds(doc, node.id)?;
                if !bounds.width().is_finite() || !bounds.height().is_finite() || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
                    return None;
                }
                let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
                if scale < 0.5 {
                    return None;
                }
                let width = (bounds.width() * scale).ceil() as u32;
                let height = (bounds.height() * scale).ceil() as u32;
                Some((node.id, noise.clone(), bounds, scale, width.max(1), height.max(1)))
            })
            .collect();
        for (id, noise, bounds, scale, width, height) in candidates {
            if self.noise_effects.contains_key(&id) {
                continue;
            }
            let (Ok(width_u16), Ok(height_u16)) = (u16::try_from(width), u16::try_from(height)) else { continue };
            let camera = Affine::scale(scale) * Affine::translate((-bounds.x0, -bounds.y0));
            let mut scene = Scene::new(width_u16, height_u16);
            // Apply node opacity after the grain pass, to its complete result.
            let source_doc = doc.node(id).filter(|node| node.opacity < 0.999).map(|_| {
                let mut copy = doc.clone();
                if let Some(node) = copy.node_mut(id) {
                    node.opacity = 1.0;
                }
                copy
            });
            draw_node(
                &mut scene,
                source_doc.as_ref().unwrap_or(doc),
                id,
                camera,
                bounds,
                0,
                None,
                None,
                CachedEffects::empty(),
                (width_u16, height_u16),
            );
            let mut source = Self::shared_with(self, width, height);
            source
                .render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
            let surface_to_local = doc.world_transform(id).inverse() * Affine::translate((bounds.x0, bounds.y0)) * Affine::scale(1.0 / scale);
            let pass = self.gpu_noise.get_or_insert_with(|| GpuNoise::new(&self.device));
            let texture = pass.submit(&self.device, &self.queue, &source.texture, &noise, surface_to_local, id.0 as u32);
            let texture_id = self.register_grain_texture(doc, id, bounds, texture);
            self.noise_effects
                .insert(id, CachedLayerBlur { id: texture_id, width: width_u16, height: height_u16, bounds, placement: Affine::IDENTITY });
        }
        Ok(())
    }

    fn prepare_static_texture(&mut self, doc: &Document) -> Result<(), Box<dyn Error>> {
        let candidates: Vec<_> = doc
            .nodes()
            .filter_map(|node| {
                let effects = node.effects.iter().filter(|effect| effect.visible).collect::<Vec<_>>();
                let [effect] = effects.as_slice() else { return None };
                let EffectKind::Texture(texture) = &effect.kind else { return None };
                if !gpu_texture_cache_supported(doc, node, texture) {
                    return None;
                }
                let bounds = figcraft_render::raster_bounds(doc, node.id)?;
                let bounds = Rect::new(bounds.x0.floor(), bounds.y0.floor(), bounds.x1.ceil(), bounds.y1.ceil());
                let scale = (4096.0 / bounds.width().max(bounds.height())).min(2.0);
                let width = (bounds.width() * scale).ceil() as u32;
                let height = (bounds.height() * scale).ceil() as u32;
                Some((node.id, texture.clone(), bounds, scale, width.max(1), height.max(1)))
            })
            .collect();
        for (id, texture, bounds, scale, width, height) in candidates {
            if self.texture_effects.contains_key(&id) {
                continue;
            }
            let (Ok(width_u16), Ok(height_u16)) = (u16::try_from(width), u16::try_from(height)) else { continue };
            let camera = Affine::scale(scale) * Affine::translate((-bounds.x0, -bounds.y0));
            let mut scene = Scene::new(width_u16, height_u16);
            let source_doc = doc.node(id).filter(|node| node.opacity < 0.999).map(|_| {
                let mut copy = doc.clone();
                if let Some(node) = copy.node_mut(id) {
                    node.opacity = 1.0;
                }
                copy
            });
            let cache_source_doc = source_doc.as_ref().unwrap_or(doc);
            draw_node(&mut scene, cache_source_doc, id, camera, bounds, 0, None, None, CachedEffects::empty(), (width_u16, height_u16));
            let mut source = Self::shared_with(self, width, height);
            source
                .render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
            let surface_to_local = doc.world_transform(id).inverse() * Affine::translate((bounds.x0, bounds.y0)) * Affine::scale(1.0 / scale);
            let pass = self.gpu_texture.get_or_insert_with(|| GpuTexture::new(&self.device));
            let result = pass.submit(&self.device, &self.queue, &source.texture, &texture, surface_to_local, scale, id.0 as u32);
            let texture_id = self.register_grain_texture(doc, id, bounds, result);
            self.texture_effects
                .insert(id, CachedLayerBlur { id: texture_id, width: width_u16, height: height_u16, bounds, placement: Affine::IDENTITY });
            // A texture scatter changes pixel positions, so resampling its 2x cache at 1x
            // changes the grain pattern. Keep an exact 1x result for that common camera scale.
            if scale == 2.0 {
                let (width_1x, height_1x) = (bounds.width().ceil() as u32, bounds.height().ceil() as u32);
                if u64::from(width_1x) * u64::from(height_1x) <= 2_000_000
                    && let (Ok(width_1x), Ok(height_1x)) = (u16::try_from(width_1x), u16::try_from(height_1x))
                {
                    let camera_1x = Affine::translate((-bounds.x0, -bounds.y0));
                    let mut scene_1x = Scene::new(width_1x, height_1x);
                    draw_node(&mut scene_1x, cache_source_doc, id, camera_1x, bounds, 0, None, None, CachedEffects::empty(), (width_1x, height_1x));
                    let mut source_1x = Self::shared_with(self, u32::from(width_1x), u32::from(height_1x));
                    source_1x.render_submit_with_target(
                        &scene_1x,
                        TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }),
                    )?;
                    let local = doc.world_transform(id).inverse() * Affine::translate((bounds.x0, bounds.y0));
                    let pass = self.gpu_texture.get_or_insert_with(|| GpuTexture::new(&self.device));
                    let result_1x = pass.submit(&self.device, &self.queue, &source_1x.texture, &texture, local, 1.0, id.0 as u32);
                    let secondary_id = TextureId(self.next_external_id);
                    self.next_external_id += 1;
                    self.image_bindings.insert(secondary_id, result_1x.create_view(&wgpu::TextureViewDescriptor::default()));
                    if let Some(state) = self.grain_cache.get_mut(&id) {
                        state.secondary = Some((secondary_id, result_1x));
                        self.texture_effects_1x.insert(
                            id,
                            CachedLayerBlur { id: secondary_id, width: width_1x, height: height_1x, bounds, placement: Affine::IDENTITY },
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn render_static_shadow(
        &mut self,
        doc: &Document,
        id: NodeId,
        bounds: Rect,
        scale: f64,
        inner: bool,
    ) -> Result<Option<CachedLayerBlur>, Box<dyn Error>> {
        let (width, height) = ((bounds.width() * scale).ceil() as u32, (bounds.height() * scale).ceil() as u32);
        let (Ok(width_u16), Ok(height_u16)) = (u16::try_from(width), u16::try_from(height)) else { return Ok(None) };
        let Some(node) = doc.node(id) else { return Ok(None) };
        let camera = Affine::scale(scale) * Affine::translate((-bounds.x0, -bounds.y0));
        let mut scene = Scene::new(width_u16, height_u16);
        let transform = camera * doc.world_transform(id);
        if let Some(shadow) = node.effects.iter().find_map(|effect| {
            let shadow = match &effect.kind {
                EffectKind::InnerShadow(shadow) if inner => Some(shadow),
                EffectKind::DropShadow(shadow) if !inner => Some(shadow),
                _ => None,
            }?;
            (effect.visible && (gpu_simple_group_shadow_supported(doc, node, shadow) || gpu_stroked_shape_shadow_supported(doc, node, shadow)))
                .then_some(shadow)
        }) {
            let mut source_doc = doc.clone();
            if let Some(group) = source_doc.node_mut(id) {
                group.effects.clear();
                group.opacity = 1.0;
            }
            draw_node(&mut scene, &source_doc, id, camera, bounds, 0, None, None, CachedEffects::empty(), (width_u16, height_u16));
            let mut source = Self::shared_with(self, width, height);
            source
                .render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
            let spread_px = (shadow.spread * scale).round() as i32;
            let caster_texture = if gpu_stroked_shape_shadow_supported(doc, node, shadow) && shadow.spread > 0.0 {
                let mut mask_scene = Scene::new(width_u16, height_u16);
                mask_scene.set_transform(transform);
                mask_scene.set_paint(peniko::Color::new([1.0; 4]));
                if let Some(outlines) = figcraft_render::shadow::silhouette(doc, node) {
                    for (path, rule) in outlines {
                        set_rule(&mut mask_scene, rule);
                        mask_scene.fill_path(&path);
                        mask_scene.set_stroke(
                            peniko::kurbo::Stroke::new(shadow.spread * 2.0)
                                .with_join(peniko::kurbo::Join::Miter)
                                .with_caps(peniko::kurbo::Cap::Butt)
                                .with_miter_limit(figcraft_doc::SPREAD_MITER_LIMIT),
                        );
                        mask_scene.stroke_path(&path);
                    }
                }
                let mut mask = Self::shared_with(self, width, height);
                mask.render_submit_with_target(
                    &mask_scene,
                    TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }),
                )?;
                Some(mask.texture.clone())
            } else {
                (spread_px != 0).then(|| {
                    let morph = self.gpu_morph.get_or_insert_with(|| GpuMorph::new(&self.device));
                    morph.submit(&self.device, &self.queue, &source.texture, spread_px.unsigned_abs(), (spread_px > 0) != inner)
                })
            };
            let caster = caster_texture.as_ref().unwrap_or(&source.texture);
            let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
            let pass = self.gpu_silhouette_shadow.get_or_insert_with(|| GpuSilhouetteShadow::new(&self.device));
            let offset = ((shadow.offset.0 * scale) as f32, (shadow.offset.1 * scale) as f32);
            let sigma = (shadow.radius * scale / 2.0) as f32;
            let texture = if inner {
                pass.submit_composited(&self.device, &self.queue, blur, &source.texture, caster, offset, sigma, shadow.color)
            } else {
                pass.submit_drop_composited(&self.device, &self.queue, blur, &source.texture, caster, offset, sigma, shadow.color, shadow.show_behind)
            };
            let texture_id = self.register_cached_texture(texture);
            return Ok(Some(CachedLayerBlur { id: texture_id, width: width_u16, height: height_u16, bounds, placement: Affine::IDENTITY }));
        }
        if inner
            && node.effects.iter().any(|effect| {
                effect.visible && matches!(&effect.kind, EffectKind::InnerShadow(shadow) if gpu_path_inner_shadow_supported(doc, node, shadow))
            })
        {
            let Some((path, rule)) = draw::node_outline(doc, node) else { return Ok(None) };
            let mut mask_scene = Scene::new(width_u16, height_u16);
            mask_scene.set_transform(transform);
            set_rule(&mut mask_scene, rule);
            mask_scene.set_paint(peniko::Color::new([1.0; 4]));
            mask_scene.fill_path(&path);
            let mut mask = Self::shared_with(self, width, height);
            mask.render_submit_with_target(
                &mask_scene,
                TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }),
            )?;
            let Some(shadow) = node.effects.iter().find_map(|effect| {
                if effect.visible
                    && let EffectKind::InnerShadow(shadow) = &effect.kind
                {
                    Some(shadow)
                } else {
                    None
                }
            }) else {
                return Ok(None);
            };
            let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
            let pass = self.gpu_silhouette_shadow.get_or_insert_with(|| GpuSilhouetteShadow::new(&self.device));
            let texture = pass.submit(
                &self.device,
                &self.queue,
                blur,
                &mask.texture,
                ((shadow.offset.0 * scale) as f32, (shadow.offset.1 * scale) as f32),
                (shadow.radius * scale / 2.0) as f32,
                shadow.color,
            );
            let texture_id = self.register_cached_texture(texture);
            return Ok(Some(CachedLayerBlur { id: texture_id, width: width_u16, height: height_u16, bounds, placement: Affine::IDENTITY }));
        }
        if inner {
            draw_inner_shadows(&mut scene, doc, node, transform);
        } else {
            draw_shadow(&mut scene, doc, node, camera);
        }
        let mut source = Self::shared_with(self, width, height);
        source.render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
        let texture = if !inner
            && node.effects.iter().any(|effect| {
                effect.visible && matches!(&effect.kind, EffectKind::DropShadow(shadow) if !shadow.show_behind && gpu_translucent_vector_shadow_supported(node, shadow))
            })
            && let Some((path, rule)) = draw::node_outline(doc, node)
        {
            let mut mask_scene = Scene::new(width_u16, height_u16);
            mask_scene.set_transform(transform);
            set_rule(&mut mask_scene, rule);
            mask_scene.set_paint(peniko::Color::new([1.0; 4]));
            mask_scene.fill_path(&path);
            let mut mask = Self::shared_with(self, width, height);
            mask.render_submit_with_target(
                &mask_scene,
                TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }),
            )?;
            let pass = self.gpu_shadow_clip.get_or_insert_with(|| GpuShadowClip::new(&self.device));
            pass.submit(&self.device, &self.queue, &source.texture, &mask.texture)
        } else {
            source.texture.clone()
        };
        let texture_id = self.register_cached_texture(texture);
        Ok(Some(CachedLayerBlur { id: texture_id, width: width_u16, height: height_u16, bounds, placement: Affine::IDENTITY }))
    }

    pub fn render_submit_document(&mut self, doc: &Document, page: NodeId, view: &View, clear: [f32; 3]) -> Result<f64, Box<dyn Error>> {
        self.render_submit_document_rgba(doc, page, view, [clear[0], clear[1], clear[2], 1.0])
    }

    pub fn render_submit_document_rgba(&mut self, doc: &Document, page: NodeId, view: &View, clear: [f32; 4]) -> Result<f64, Box<dyn Error>> {
        let scene = self.make_scene_document(doc, page, view);
        self.render_submit_with_clear_rgba(&scene, clear)
    }

    pub fn make_scene_document(&self, doc: &Document, page: NodeId, view: &View) -> Scene {
        self.make_scene_document_with_backdrops(doc, page, view, None)
    }

    /// Build a scene for selected roots, preserving their world transforms and descendants.
    /// Callers must check any backdrop dependency before using this outside export.
    pub fn make_scene_roots(&self, doc: &Document, roots: &[NodeId], view: &View) -> Scene {
        self.make_scene_roots_outlined(doc, roots, view, &[], figcraft_render::Outline::default())
    }

    /// Draw selected outlines within the supplied roots without traversing page siblings.
    pub fn make_scene_roots_outlined(
        &self,
        doc: &Document,
        roots: &[NodeId],
        view: &View,
        outlined: &[NodeId],
        options: figcraft_render::Outline,
    ) -> Scene {
        let mut scene = Scene::new(view.width as u16, view.height as u16);
        let camera = Affine::scale(view.scale) * Affine::translate((-view.origin.0, -view.origin.1));
        let world = Rect::new(
            view.origin.0,
            view.origin.1,
            view.origin.0 + f64::from(view.width) / view.scale,
            view.origin.1 + f64::from(view.height) / view.scale,
        );
        let cached = CachedEffects {
            layer_blurs: Some(&self.layer_blurs),
            layer_blurs_1x: Some(&self.layer_blurs_1x),
            drop_shadows: Some(&self.drop_shadows),
            inner_shadows: Some(&self.inner_shadows),
            noise: Some(&self.noise_effects),
            texture: if view.scale <= 1.0 { Some(&self.texture_effects_1x) } else { Some(&self.texture_effects) },
            outlined,
            outline_options: options,
        };
        for id in roots {
            draw_node(&mut scene, doc, *id, camera, world, 0, Some(&self.image_info), None, cached, (view.width as u16, view.height as u16));
        }
        scene
    }

    /// Render selected page roots to a transparent target for raster export. The caller checks
    /// that the selected subtree has no backdrop dependency or unsupported paint operation.
    pub fn render_export_roots(&mut self, doc: &Document, roots: &[NodeId], view: &View) -> Result<(), Box<dyn Error>> {
        let scene = self.make_scene_roots(doc, roots, view);
        self.render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
        self.wait()
    }

    fn make_scene_document_with_backdrops(&self, doc: &Document, page: NodeId, view: &View, backdrops: Option<&HashMap<NodeId, ImageInfo>>) -> Scene {
        self.make_scene_document_with_backdrops_and_outlines(doc, page, view, backdrops, &[], figcraft_render::Outline::default())
    }

    fn make_scene_document_with_backdrops_and_outlines(
        &self,
        doc: &Document,
        page: NodeId,
        view: &View,
        backdrops: Option<&HashMap<NodeId, ImageInfo>>,
        outlined: &[NodeId],
        outline_options: figcraft_render::Outline,
    ) -> Scene {
        make_scene_with_cached_layers(
            doc,
            page,
            view,
            Some(&self.image_info),
            backdrops,
            CachedEffects {
                layer_blurs: Some(&self.layer_blurs),
                layer_blurs_1x: Some(&self.layer_blurs_1x),
                drop_shadows: Some(&self.drop_shadows),
                inner_shadows: Some(&self.inner_shadows),
                noise: Some(&self.noise_effects),
                texture: if view.scale <= 1.0 { Some(&self.texture_effects_1x) } else { Some(&self.texture_effects) },
                outlined,
                outline_options,
            },
        )
    }

    /// Replace selected subtrees with their outlines while keeping normal paint order.
    pub fn make_scene_selected_outlines(
        &self,
        doc: &Document,
        page: NodeId,
        view: &View,
        outlined: &[NodeId],
        options: figcraft_render::Outline,
    ) -> Scene {
        // A selected subtree is outlined before draw_node checks its effect cache. Keep the
        // caches available for siblings so their blur, grain and shadow survive this view.
        make_scene_with_cached_layers(
            doc,
            page,
            view,
            Some(&self.image_info),
            None,
            CachedEffects {
                layer_blurs: Some(&self.layer_blurs),
                layer_blurs_1x: Some(&self.layer_blurs_1x),
                drop_shadows: Some(&self.drop_shadows),
                inner_shadows: Some(&self.inner_shadows),
                noise: Some(&self.noise_effects),
                texture: if view.scale <= 1.0 { Some(&self.texture_effects_1x) } else { Some(&self.texture_effects) },
                outlined,
                outline_options: options,
            },
        )
    }

    pub fn device(&self) -> wgpu::Device {
        self.device.clone()
    }

    pub fn queue(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Stable sRGB texture view for presenting a Display P3 document in an sRGB native window.
    pub fn srgb_output_view(&mut self) -> &wgpu::TextureView {
        let pass = self.gpu_color_profile.get_or_insert_with(|| GpuColorProfile::new(&self.device, self.texture.size()));
        pass.view()
    }

    /// Convert the finished Display P3 canvas on the GPU without changing document-space pixels.
    pub fn convert_for_presentation(&mut self, profile: ColorProfile) {
        self.convert_for_presentation_with_proof(profile, false);
    }

    /// Present Display P3 in sRGB and optionally mark out-of-gamut pixels with the editor's
    /// diagonal proof stripes in the same GPU pass.
    pub fn convert_for_presentation_with_proof(&mut self, profile: ColorProfile, proof: bool) {
        if profile == ColorProfile::DisplayP3 {
            let pass = self.gpu_color_profile.get_or_insert_with(|| GpuColorProfile::new(&self.device, self.texture.size()));
            if let Some(matrix) = profile.conversion_to(ColorProfile::Srgb).linear_matrix() {
                pass.submit(&self.device, &self.queue, &self.texture, matrix, proof);
            }
        }
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

    /// Change the presentation target while retaining document-stable effect textures and image
    /// bindings. Panes and cached shadows do not depend on the window dimensions.
    pub fn resize(&mut self, width: u32, height: u32) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
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
        let (renderer, resources) = vello_gpu::Renderer::new(&self.device, &config);
        let depth = vello_gpu::Renderer::create_depth_texture_view(&self.device, &RenderSize { width: width as u16, height: height as u16 });
        self.texture = texture;
        self.view = view;
        self.renderer = renderer;
        self.resources = resources;
        self.depth = depth;
        self.gpu_color_profile = None;
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
            shader_textures: HashMap::new(),
            shader_info: HashMap::new(),
            pattern_textures: HashMap::new(),
            pattern_info: HashMap::new(),
            temporary_textures: Vec::new(),
            temporary_ids: Vec::new(),
            cached_textures: Vec::new(),
            cached_ids: Vec::new(),
            image_bindings: TextureBindings::new(),
            image_info: HashMap::new(),
            next_external_id: 1_000_000,
            gpu_blur: None,
            gpu_morph: None,
            gpu_glass: None,
            gpu_image_filter: None,
            gpu_diamond: None,
            gpu_color_profile: None,
            gpu_progressive_blur: None,
            gpu_noise: None,
            gpu_texture: None,
            gpu_shadow_clip: None,
            gpu_shader: None,
            gpu_silhouette_shadow: None,
            glass_prefix_canvas: None,
            glass_mask_canvas: None,
            glass_content_canvas: None,
            layer_blurs: HashMap::new(),
            layer_blurs_1x: HashMap::new(),
            drop_shadows: HashMap::new(),
            inner_shadows: HashMap::new(),
            noise_effects: HashMap::new(),
            texture_effects: HashMap::new(),
            texture_effects_1x: HashMap::new(),
            grain_cache: HashMap::new(),
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

    fn register_cached_texture(&mut self, texture: wgpu::Texture) -> TextureId {
        let id = TextureId(self.next_external_id);
        self.next_external_id += 1;
        self.image_bindings.insert(id, texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.cached_textures.push(texture);
        self.cached_ids.push(id);
        id
    }

    fn release_cached_textures(&mut self) {
        for id in self.cached_ids.drain(..) {
            self.image_bindings.remove(id);
        }
        self.cached_textures.clear();
    }

    fn crop_texture(&self, source: &wgpu::Texture, region: (u16, u16, u16, u16)) -> wgpu::Texture {
        let (x, y, width, height) = region;
        let size = wgpu::Extent3d { width: u32::from(width), height: u32::from(height), depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard cropped GPU backdrop"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: source.format(),
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard glass backdrop crop") });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d { x: u32::from(x), y: u32::from(y), z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            size,
        );
        self.queue.submit([encoder.finish()]);
        texture
    }

    fn render_outline_mask(&mut self, doc: &Document, id: NodeId, width: u16, height: u16, camera: Affine) -> Result<wgpu::Texture, Box<dyn Error>> {
        let node = doc.node(id).ok_or("glass node missing")?;
        let (path, rule) = draw::node_outline(doc, node).ok_or("glass outline missing")?;
        let mut scene = Scene::new(width, height);
        scene.set_transform(camera * doc.world_transform(id));
        set_rule(&mut scene, rule);
        scene.set_paint(peniko::Color::new([1.0; 4]));
        scene.fill_path(&path);
        let mut surface = self.glass_mask_canvas.take().unwrap_or_else(|| Box::new(Self::shared_with(self, u32::from(width), u32::from(height))));
        if surface.texture.width() != u32::from(width) || surface.texture.height() != u32::from(height) {
            surface.resize(u32::from(width), u32::from(height));
        }
        surface.render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
        let mask = surface.snapshot_submit();
        self.glass_mask_canvas = Some(surface);
        Ok(mask)
    }

    fn render_glass_mask(&mut self, doc: &Document, id: NodeId, view: &View, region: (u16, u16, u16, u16)) -> Result<wgpu::Texture, Box<dyn Error>> {
        let (x, y, width, height) = region;
        let camera =
            Affine::translate((-f64::from(x), -f64::from(y))) * Affine::scale(view.scale) * Affine::translate((-view.origin.0, -view.origin.1));
        self.render_outline_mask(doc, id, width, height, camera)
    }

    fn release_temporary_textures(&mut self) {
        for id in self.temporary_ids.drain(..) {
            self.image_bindings.remove(id);
        }
        self.temporary_textures.clear();
    }

    /// Isolate the glass backdrop and the pane's own paint before running its layer effects.
    /// The temporary surface stays on the same device; no pixel readback enters this path.
    fn compose_glass_content(
        &mut self,
        doc: &Document,
        id: NodeId,
        view: &View,
        region: (u16, u16, u16, u16),
        glass: wgpu::Texture,
    ) -> Result<wgpu::Texture, Box<dyn Error>> {
        let (x, y, width, height) = region;
        let glass_id = self.register_external_texture(glass);
        let backdrop = HashMap::from([(id, ImageInfo { id: glass_id, width, height, region: Some((0, 0, width, height)) })]);
        let origin = (view.origin.0 + f64::from(x) / view.scale, view.origin.1 + f64::from(y) / view.scale);
        let world = Rect::new(origin.0, origin.1, origin.0 + f64::from(width) / view.scale, origin.1 + f64::from(height) / view.scale);
        let camera = Affine::scale(view.scale) * Affine::translate((-origin.0, -origin.1));
        let mut source_doc = doc.clone();
        if let Some(node) = source_doc.node_mut(id) {
            node.effects.clear();
        }
        let mut scene = Scene::new(width, height);
        draw_node(&mut scene, &source_doc, id, camera, world, 0, Some(&self.image_info), Some(&backdrop), CachedEffects::empty(), (width, height));
        let mut surface = self.glass_content_canvas.take().unwrap_or_else(|| Box::new(Self::shared_with(self, u32::from(width), u32::from(height))));
        if surface.texture.width() != u32::from(width) || surface.texture.height() != u32::from(height) {
            surface.resize(u32::from(width), u32::from(height));
        }
        surface.image_bindings = self.image_bindings.clone();
        surface.render_submit_with_target(&scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }))?;
        let result = surface.snapshot_submit();
        self.glass_content_canvas = Some(surface);
        Ok(result)
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
        self.render_with_backdrop_blurs_indexed_rgba(doc, page, view, [clear[0], clear[1], clear[2], 1.0], index)
    }

    pub fn render_with_backdrop_blurs_indexed_rgba(
        &mut self,
        doc: &Document,
        page: NodeId,
        view: &View,
        clear: [f32; 4],
        index: &BackdropIndex,
    ) -> Result<(f64, usize), Box<dyn Error>> {
        self.render_with_backdrop_blurs_indexed_outlined_rgba(doc, page, view, clear, index, &[], figcraft_render::Outline::default())
    }

    /// Draw selected roots as contours while other roots retain their ordered GPU backdrops.
    pub fn render_with_backdrop_blurs_indexed_outlined(
        &mut self,
        doc: &Document,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        index: &BackdropIndex,
        outlined: &[NodeId],
        options: figcraft_render::Outline,
    ) -> Result<(f64, usize), Box<dyn Error>> {
        self.render_with_backdrop_blurs_indexed_outlined_rgba(doc, page, view, [clear[0], clear[1], clear[2], 1.0], index, outlined, options)
    }

    fn render_with_backdrop_blurs_indexed_outlined_rgba(
        &mut self,
        doc: &Document,
        page: NodeId,
        view: &View,
        clear: [f32; 4],
        index: &BackdropIndex,
        outlined: &[NodeId],
        options: figcraft_render::Outline,
    ) -> Result<(f64, usize), Box<dyn Error>> {
        let started = Instant::now();
        let world = Rect::new(
            view.origin.0,
            view.origin.1,
            view.origin.0 + f64::from(view.width) / view.scale,
            view.origin.1 + f64::from(view.height) / view.scale,
        );
        let roots = doc.node(page).map(|node| node.children.as_slice()).ok_or("page missing")?;
        let outlined_ids: HashSet<_> = outlined.iter().flat_map(|id| doc.subtree(*id)).collect();
        let matching: Vec<_> =
            index.candidates.iter().filter(|(id, _, bounds, _)| !outlined_ids.contains(id) && bounds.intersect(world).area() > 0.0).collect();
        let candidates: Vec<_> = matching.iter().map(|(id, radius, _, _)| (*id, *radius)).collect();
        let has_custom_backdrop = candidates.iter().any(|(id, _)| {
            doc.node(*id).is_some_and(|node| {
                node.effects.iter().any(|effect| {
                    effect.visible
                        && (matches!(&effect.kind, EffectKind::BackgroundBlur(blur) if blur.is_progressive())
                            || matches!(&effect.kind, EffectKind::Glass(_)))
                })
            })
        });
        if candidates.is_empty() {
            let scene = if outlined.is_empty() {
                make_scene_with_images(doc, page, view, Some(&self.image_info))
            } else {
                self.make_scene_document_with_backdrops_and_outlines(doc, page, view, None, outlined, options)
            };
            self.render_with_clear_rgba(&scene, clear)?;
            self.release_temporary_textures();
            return Ok((started.elapsed().as_secs_f64() * 1000.0, 0));
        }
        if outlined.is_empty()
            && std::env::var_os("LIZARD_GPU_GROUP_ISOLATION").is_some()
            && !has_custom_backdrop
            && candidates.len() == 1
            && let Some((group, opacity)) = isolated_group_for_backdrop(doc, candidates[0].0, page)
        {
            let result = self.render_isolated_group_backdrop(doc, page, view, clear, candidates[0], group, opacity);
            self.release_temporary_textures();
            return result;
        }
        if matching.iter().any(|(_, _, _, requires_cpu)| *requires_cpu) {
            if !outlined.is_empty() {
                return Err("selected outline contains an unsupported backdrop".into());
            }
            let tile = render_view(doc, page, view, &Options { lod: true, ..Default::default() }).ok_or("CPU effect fallback failed")?;
            self.upload_premul(tile.data())?;
            self.release_temporary_textures();
            return Ok((started.elapsed().as_secs_f64() * 1000.0, candidates.len()));
        }
        if outlined.is_empty() && std::env::var_os("LIZARD_GPU_SEGMENT").is_some() && !has_custom_backdrop && !candidates.is_empty() {
            let result = self.render_segmented_backdrops(doc, page, view, clear, &candidates);
            self.release_temporary_textures();
            return result;
        }
        let mut backdrops = HashMap::new();
        let mut dynamic_glass_layers = Vec::new();
        let mut prefix_ms = 0.0;
        let mut blur_ms = 0.0;
        for (id, radius) in candidates {
            let stage = Instant::now();
            let glass = doc.node(id).and_then(|node| {
                node.effects.iter().find_map(|effect| match &effect.kind {
                    EffectKind::Glass(glass) if effect.visible => Some(glass),
                    _ => None,
                })
            });
            let glass_region = glass.and_then(|effect| gpu_glass_region(doc, id, view, effect));
            let cropped_prefix = glass_region.is_some() && backdrops.is_empty();
            let mut beneath = doc.clone();
            figcraft_render::backdrop::hide_above(&mut beneath, id, roots);
            let prefix_texture = if let Some((x, y, width, height)) = glass_region.filter(|_| cropped_prefix) {
                let cropped_view = View {
                    origin: (view.origin.0 + f64::from(x) / view.scale, view.origin.1 + f64::from(y) / view.scale),
                    scale: view.scale,
                    width: u32::from(width),
                    height: u32::from(height),
                };
                let prefix = self.make_scene_document_with_backdrops_and_outlines(&beneath, page, &cropped_view, None, outlined, options);
                let mut surface =
                    self.glass_prefix_canvas.take().unwrap_or_else(|| Box::new(Self::shared_with(self, u32::from(width), u32::from(height))));
                if surface.texture.width() != u32::from(width) || surface.texture.height() != u32::from(height) {
                    surface.resize(u32::from(width), u32::from(height));
                }
                surface.image_bindings = self.image_bindings.clone();
                surface.render_submit_with_clear_rgba(&prefix, clear)?;
                if let Some(node) = doc.node(id).filter(|node| gpu_glass_shadow_supported(doc, node)) {
                    let mut shadow_scene = Scene::new(width, height);
                    let camera = Affine::scale(view.scale) * Affine::translate((-cropped_view.origin.0, -cropped_view.origin.1));
                    draw_shadow(&mut shadow_scene, doc, node, camera);
                    surface.render_submit_with_target(&shadow_scene, TargetInit::SrcOver)?;
                }
                let texture = surface.snapshot_submit();
                self.glass_prefix_canvas = Some(surface);
                texture
            } else {
                let prefix = self.make_scene_document_with_backdrops_and_outlines(&beneath, page, view, Some(&backdrops), outlined, options);
                self.render_submit_with_clear_rgba(&prefix, clear)?;
                if let Some(node) = doc.node(id).filter(|node| gpu_glass_shadow_supported(doc, node)) {
                    let mut shadow_scene = Scene::new(view.width as u16, view.height as u16);
                    let camera = Affine::scale(view.scale) * Affine::translate((-view.origin.0, -view.origin.1));
                    draw_shadow(&mut shadow_scene, doc, node, camera);
                    self.render_submit_with_target(&shadow_scene, TargetInit::SrcOver)?;
                }
                self.snapshot_submit()
            };
            prefix_ms += stage.elapsed().as_secs_f64() * 1000.0;

            let stage = Instant::now();
            let method = std::env::var("LIZARD_GPU_BLUR_METHOD").unwrap_or_default();
            let progressive = doc.node(id).and_then(|node| {
                node.effects.iter().find_map(|effect| match &effect.kind {
                    EffectKind::BackgroundBlur(blur) if effect.visible && blur.is_progressive() => Some((node, blur)),
                    _ => None,
                })
            });
            let blurred = if let Some(effect) = glass {
                let region = glass_region.unwrap_or((0, 0, view.width as u16, view.height as u16));
                let cropped = if cropped_prefix { prefix_texture.clone() } else { self.crop_texture(&prefix_texture, region) };
                let mask = self.render_glass_mask(doc, id, view, region)?;
                let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
                let glass_pass = self.gpu_glass.get_or_insert_with(|| GpuGlass::new(&self.device));
                let mut result = glass_pass.submit(&self.device, &self.queue, blur, &cropped, &mask, effect, view.scale);
                let composited = doc.node(id).is_some_and(|node| {
                    gpu_glass_stack_supported(doc, node)
                        && node.fills.iter().any(|paint| paint.visible)
                        && node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_) | EffectKind::Noise(_)))
                });
                if composited {
                    result = self.compose_glass_content(doc, id, view, region, result)?;
                }
                if let Some(node) = doc.node(id).filter(|node| gpu_glass_stack_supported(doc, node)) {
                    let surface_to_local = doc.world_transform(id).inverse()
                        * Affine::translate((view.origin.0 + f64::from(region.0) / view.scale, view.origin.1 + f64::from(region.1) / view.scale))
                        * Affine::scale(1.0 / view.scale);
                    for layer_effect in node.effects.iter().filter(|effect| effect.visible) {
                        result = match &layer_effect.kind {
                            EffectKind::LayerBlur(blur_effect) => {
                                let pass = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
                                pass.submit(&self.device, &self.queue, &result, (blur_effect.radius * view.scale / 2.0) as f32, 1, None)
                            }
                            EffectKind::Noise(noise) => {
                                let pass = self.gpu_noise.get_or_insert_with(|| GpuNoise::new(&self.device));
                                pass.submit(&self.device, &self.queue, &result, noise, surface_to_local, id.0 as u32)
                            }
                            _ => result,
                        };
                    }
                    if composited
                        && let Some(shadow) = node.effects.iter().find_map(|effect| match &effect.kind {
                            EffectKind::DropShadow(shadow) if effect.visible => Some(shadow),
                            _ => None,
                        })
                    {
                        let spread = (shadow.spread * view.scale).round() as u32;
                        let caster = (spread > 0).then(|| {
                            let morph = self.gpu_morph.get_or_insert_with(|| GpuMorph::new(&self.device));
                            morph.submit(&self.device, &self.queue, &result, spread, true)
                        });
                        let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
                        let pass = self.gpu_silhouette_shadow.get_or_insert_with(|| GpuSilhouetteShadow::new(&self.device));
                        result = pass.submit_drop_composited(
                            &self.device,
                            &self.queue,
                            blur,
                            &result,
                            caster.as_ref().unwrap_or(&result),
                            ((shadow.offset.0 * view.scale) as f32, (shadow.offset.1 * view.scale) as f32),
                            (shadow.radius * view.scale / 2.0) as f32,
                            shadow.color,
                            shadow.show_behind,
                        );
                    }
                }
                if composited {
                    let texture_id = self.register_external_texture(result.clone());
                    let (x, y, width, height) = region;
                    let x0 = view.origin.0 + f64::from(x) / view.scale;
                    let y0 = view.origin.1 + f64::from(y) / view.scale;
                    self.layer_blurs.insert(
                        id,
                        CachedLayerBlur {
                            id: texture_id,
                            width,
                            height,
                            bounds: Rect::new(x0, y0, x0 + f64::from(width) / view.scale, y0 + f64::from(height) / view.scale),
                            placement: Affine::IDENTITY,
                        },
                    );
                    dynamic_glass_layers.push(id);
                }
                result
            } else if let Some((node, effect)) = progressive {
                let screen = Affine::scale(view.scale) * Affine::translate((-view.origin.0, -view.origin.1)) * doc.world_transform(id);
                let safe = effect.sanitized();
                let start = screen * Point::new(safe.start.0 * node.width, safe.start.1 * node.height);
                let end = screen * Point::new(safe.end.0 * node.width, safe.end.1 * node.height);
                let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
                let mix = self.gpu_progressive_blur.get_or_insert_with(|| GpuProgressiveBlur::new(&self.device));
                mix.submit(&self.device, &self.queue, blur, &prefix_texture, effect, view.scale, start, end)
            } else if method == "copy" {
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
                self.render_submit_with_clear_rgba(&blur_scene, clear)?;
                self.snapshot_submit()
            };
            let (Ok(width), Ok(height)) = (u16::try_from(blurred.width()), u16::try_from(blurred.height())) else { continue };
            let texture_id = self.register_external_texture(blurred);
            backdrops.insert(id, ImageInfo { id: texture_id, width, height, region: glass_region });
            blur_ms += stage.elapsed().as_secs_f64() * 1000.0;
        }
        let stage = Instant::now();
        let scene = self.make_scene_document_with_backdrops_and_outlines(doc, page, view, Some(&backdrops), outlined, options);
        let final_result = self.render_with_clear_rgba(&scene, clear);
        for id in dynamic_glass_layers {
            self.layer_blurs.remove(&id);
        }
        final_result?;
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
        clear: [f32; 4],
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
                self.render_submit_with_clear_rgba(&scene, clear)?;
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
            backdrops.insert(id, ImageInfo { id: texture_id, width, height, region: None });
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
        clear: [f32; 4],
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
        self.render_submit_with_clear_rgba(&base_scene, clear)?;
        let base = self.snapshot_submit();

        let mut prefix_doc = doc.clone();
        figcraft_render::backdrop::hide_above(&mut prefix_doc, id, roots);
        let prefix_scene = make_scene_with_images(&prefix_doc, page, view, Some(&self.image_info));
        self.render_submit_with_clear_rgba(&prefix_scene, clear)?;
        let source = self.snapshot_submit();
        let scale = std::env::var("LIZARD_GPU_BLUR_SCALE").ok().and_then(|value| value.parse::<u32>().ok()).unwrap_or(4).clamp(1, 4);
        let blur = self.gpu_blur.get_or_insert_with(|| GpuBlur::new(&self.device));
        let clip = backdrop_clip_rect(doc, id, page, view);
        let blurred = blur.submit(&self.device, &self.queue, &source, (radius * view.scale / 2.0) as f32, scale, clip);
        let (width, height) = (u16::try_from(blurred.width())?, u16::try_from(blurred.height())?);
        let blur_id = self.register_external_texture(blurred);
        let backdrops = HashMap::from([(id, ImageInfo { id: blur_id, width, height, region: None })]);

        let mut full_prefix_doc = doc.clone();
        if let Some(node) = full_prefix_doc.node_mut(group) {
            node.opacity = 1.0;
        }
        figcraft_render::backdrop::hide_above(&mut full_prefix_doc, id, roots);
        let full_prefix_scene = make_scene_with_images(&full_prefix_doc, page, view, Some(&self.image_info));
        self.render_submit_with_clear_rgba(&full_prefix_scene, clear)?;

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
        self.render_submit_with_clear_rgba(&blend, clear)?;

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
        self.register_images_at_scale(doc, 0.0, 1.0)
    }

    /// Upload the image frame visible at `time`; static renders use the first frame.
    pub fn register_images_at(&mut self, doc: &Document, time: f64) -> Result<usize, Box<dyn Error>> {
        self.register_images_at_scale(doc, time, 1.0)
    }

    /// Register image and shader paints at the current device scale. Shader resolution changes
    /// only at powers of two, avoiding texture churn on every zoom tick.
    pub fn register_images_at_scale(&mut self, doc: &Document, time: f64, scale: f64) -> Result<usize, Box<dyn Error>> {
        let mut count = 0;
        let mut active_shader_keys = HashSet::new();
        for node in doc.nodes() {
            let region_paints = match &node.kind {
                NodeKind::Vector { network } => network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            for paint in node.fills.iter().chain(&node.strokes).chain(region_paints) {
                if let PaintKind::Shader(fill) = &paint.kind {
                    let Some(definition) = doc.shaders.get(&fill.shader_id) else { continue };
                    let key = shader_key(node, fill);
                    active_shader_keys.insert(key.clone());
                    let scale = if scale.is_finite() { scale.clamp(1.0, 64.0) } else { 1.0 };
                    let texture_scale = scale.log2().ceil().exp2();
                    let width = (node.width.abs() * texture_scale).ceil().clamp(1.0, f64::from(figcraft_shader::MAX_DIMENSION)) as u16;
                    let height = (node.height.abs() * texture_scale).ceil().clamp(1.0, f64::from(figcraft_shader::MAX_DIMENSION)) as u16;
                    let signature = format!("{:?}", definition);
                    let frame_time = if definition.features.animated && time.is_finite() { time.max(0.0) as f32 } else { 0.0 };
                    let state = (width, height, signature, frame_time.to_bits());
                    if self.shader_info.get(&key) == Some(&state) {
                        continue;
                    }
                    let generator = self.gpu_shader.get_or_insert_with(GpuShader::new);
                    let texture =
                        generator.render(&self.device, &self.queue, definition, &fill.values, u32::from(width), u32::from(height), frame_time)?;
                    let id = self.image_info.get(&key).map_or_else(|| TextureId(self.next_external_id), |info| info.id);
                    if !self.image_info.contains_key(&key) {
                        self.next_external_id += 1;
                    }
                    self.image_bindings.insert(id, texture.create_view(&wgpu::TextureViewDescriptor::default()));
                    self.image_info.insert(key.clone(), ImageInfo { id, width, height, region: None });
                    self.shader_textures.insert(key.clone(), texture);
                    self.shader_info.insert(key, state);
                    count += 1;
                    continue;
                }
                if let PaintKind::Gradient(gradient) = &paint.kind
                    && gradient.kind == GradientKind::Diamond
                {
                    let key = diamond_key(gradient, paint.opacity);
                    if self.image_info.contains_key(&key) {
                        continue;
                    }
                    let generator = self.gpu_diamond.get_or_insert_with(|| GpuDiamond::new(&self.device));
                    let texture = generator.submit(&self.device, &self.queue, gradient, paint.opacity);
                    let id = TextureId(self.image_info.len() as u64 + 1);
                    self.image_bindings.insert(id, texture.create_view(&wgpu::TextureViewDescriptor::default()));
                    self.image_info.insert(key, ImageInfo { id, width: 256, height: 256, region: None });
                    self.image_textures.push(texture);
                    count += 1;
                    continue;
                }
                let PaintKind::Image(img) = &paint.kind else { continue };
                let key = image_key(img);
                if self.image_info.contains_key(&key) {
                    continue;
                }
                let data = doc.images.get(&img.image);
                let raw_filters = figcraft_color::ImageFilters::NONE;
                let Some(pm) = figcraft_render::images::frame_at(&img.image, data, &raw_filters, doc.color_profile, time)
                    .or_else(|| figcraft_render::images::get(&img.image, data, &raw_filters, doc.color_profile))
                else {
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
                let filtered = if img.filters.is_identity() {
                    None
                } else {
                    let filter = self.gpu_image_filter.get_or_insert_with(|| GpuImageFilter::new(&self.device));
                    Some(filter.submit(&self.device, &self.queue, &texture, &img.filters))
                };
                self.image_bindings.insert(id, filtered.as_ref().unwrap_or(&texture).create_view(&wgpu::TextureViewDescriptor::default()));
                self.image_info.insert(key, ImageInfo { id, width, height, region: None });
                self.image_textures.push(texture);
                if let Some(filtered) = filtered {
                    self.image_textures.push(filtered);
                }
                count += 1;
            }
        }
        let mut active_pattern_keys = HashSet::new();
        for node in doc.nodes() {
            let region_paints = match &node.kind {
                NodeKind::Vector { network } => network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            for paint in node.fills.iter().chain(&node.strokes).chain(region_paints) {
                let PaintKind::Pattern(fill) = &paint.kind else { continue };
                let Some(source) = gpu_pattern_source(doc, fill) else { continue };
                let Some(cell) = fill.cell(source.width, source.height, node.width, node.height) else { continue };
                let key = pattern_key(node, fill);
                active_pattern_keys.insert(key.clone());
                let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
                let resolution = scale.min(2048.0 / cell.size.0.max(cell.size.1));
                let width = (cell.size.0 * resolution).ceil().clamp(1.0, 2048.0) as u16;
                let height = (cell.size.1 * resolution).ceil().clamp(1.0, 2048.0) as u16;
                let mut source_state = String::new();
                pattern_source_state(doc, source.id, &mut source_state);
                let state = (width, height, source_state, time.to_bits());
                if self.pattern_info.get(&key) == Some(&state) {
                    continue;
                }
                let kx = f64::from(width) / cell.size.0;
                let ky = f64::from(height) / cell.size.1;
                let mut scene = Scene::new(width, height);
                let Some(source_view) = doc.world_render_bounds(source.id) else { continue };
                let unrotated = doc.world_transform(source.id).inverse();
                for tile in &cell.tiles {
                    let camera = Affine::scale_non_uniform(kx, ky) * Affine::translate(*tile) * Affine::scale(cell.scale) * unrotated;
                    draw_node(
                        &mut scene,
                        doc,
                        source.id,
                        camera,
                        source_view,
                        0,
                        Some(&self.image_info),
                        None,
                        CachedEffects::empty(),
                        (width, height),
                    );
                }
                let mut tile_canvas = Self::shared_with(self, u32::from(width), u32::from(height));
                tile_canvas.image_bindings = self.image_bindings.clone();
                tile_canvas.render_submit_with_target(
                    &scene,
                    TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new([0.0; 4]) }),
                )?;
                let texture = tile_canvas.texture.clone();
                let id = self.image_info.get(&key).map_or_else(|| TextureId(self.next_external_id), |info| info.id);
                if !self.image_info.contains_key(&key) {
                    self.next_external_id += 1;
                }
                self.image_bindings.insert(id, texture.create_view(&wgpu::TextureViewDescriptor::default()));
                self.image_info.insert(key.clone(), ImageInfo { id, width, height, region: None });
                self.pattern_textures.insert(key.clone(), texture);
                self.pattern_info.insert(key, state);
                count += 1;
            }
        }
        let stale_patterns: Vec<_> = self.pattern_info.keys().filter(|key| !active_pattern_keys.contains(*key)).cloned().collect();
        for key in stale_patterns {
            if let Some(info) = self.image_info.remove(&key) {
                self.image_bindings.remove(info.id);
            }
            self.pattern_info.remove(&key);
            self.pattern_textures.remove(&key);
        }
        let stale: Vec<_> = self.shader_info.keys().filter(|key| !active_shader_keys.contains(*key)).cloned().collect();
        for key in stale {
            if let Some(info) = self.image_info.remove(&key) {
                self.image_bindings.remove(info.id);
            }
            self.shader_info.remove(&key);
            self.shader_textures.remove(&key);
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

    fn render_with_clear_rgba(&mut self, scene: &Scene, clear: [f32; 4]) -> Result<f64, Box<dyn Error>> {
        let ms = self.render_submit_with_clear_rgba(scene, clear)?;
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
        self.render_submit_with_clear_rgba(scene, [clear[0], clear[1], clear[2], 1.0])
    }

    fn render_submit_with_clear_rgba(&mut self, scene: &Scene, clear: [f32; 4]) -> Result<f64, Box<dyn Error>> {
        self.render_submit_with_target(scene, TargetInit::Clear(vello_gpu::ClearSettings::Viewport { color: peniko::Color::new(clear) }))
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
        self.texture_pixels(&self.texture)
    }

    pub fn presented_pixels(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let texture = self.gpu_color_profile.as_ref().map_or(&self.texture, GpuColorProfile::texture);
        self.texture_pixels(texture)
    }

    /// Queue a small readback from the already rendered canvas. The caller polls the result on
    /// subsequent frames; this does not block the UI or submit a second scene render.
    pub fn sample_9x9(&self, x: i32, y: i32, presented: bool) -> Result<Option<GpuPixelSample>, Box<dyn Error>> {
        let texture = if presented { self.gpu_color_profile.as_ref().map_or(&self.texture, GpuColorProfile::texture) } else { &self.texture };
        let size = texture.size();
        let left = i64::from(x) - 4;
        let top = i64::from(y) - 4;
        let x0 = left.max(0).min(i64::from(size.width)) as u32;
        let y0 = top.max(0).min(i64::from(size.height)) as u32;
        let x1 = (left + 9).max(0).min(i64::from(size.width)) as u32;
        let y1 = (top + 9).max(0).min(i64::from(size.height)) as u32;
        if x0 >= x1 || y0 >= y1 {
            return Ok(None);
        }
        let width = x1 - x0;
        let height = y1 - y0;
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard eyedropper readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard eyedropper copy") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x: x0, y: y0, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        Ok(Some(GpuPixelSample {
            buffer,
            rx,
            width,
            height,
            stride,
            offset_x: (i64::from(x0) - left) as usize,
            offset_y: (i64::from(y0) - top) as usize,
        }))
    }

    fn texture_pixels(&self, texture: &wgpu::Texture) -> Result<Vec<u8>, Box<dyn Error>> {
        let size = texture.size();
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
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
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

/// Pending 9 × 9 canvas sample. Empty space outside the rendered viewport stays transparent.
pub struct GpuPixelSample {
    buffer: wgpu::Buffer,
    rx: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    width: u32,
    height: u32,
    stride: u32,
    offset_x: usize,
    offset_y: usize,
}

impl GpuPixelSample {
    pub fn try_pixels(&mut self, device: &wgpu::Device) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
        #[cfg(not(target_arch = "wasm32"))]
        device.poll(wgpu::PollType::Poll)?;
        #[cfg(target_arch = "wasm32")]
        let _ = device;
        match self.rx.try_recv() {
            Ok(result) => result?,
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => return Err("GPU pixel readback channel disconnected".into()),
        }
        let mapped = self.buffer.slice(..).get_mapped_range()?;
        let mut pixels = vec![0; 9 * 9 * 4];
        for row in 0..self.height as usize {
            let source = &mapped[row * self.stride as usize..row * self.stride as usize + self.width as usize * 4];
            let destination = ((row + self.offset_y) * 9 + self.offset_x) * 4;
            pixels[destination..destination + source.len()].copy_from_slice(source);
        }
        drop(mapped);
        self.buffer.unmap();
        Ok(Some(pixels))
    }
}

#[cfg(test)]
mod pixel_sample_tests {
    use super::*;

    #[test]
    fn eyedropper_reads_presented_pixels_without_rerendering() -> Result<(), Box<dyn Error>> {
        let Ok(mut canvas) = GpuCanvas::new(16, 16) else { return Ok(()) };
        let scene = Scene::new(16, 16);
        canvas.render_submit_with_clear(&scene, [0.8, 0.2, 0.1])?;
        let mut middle = canvas.sample_9x9(8, 8, false)?.ok_or("middle sample missing")?;
        let mut corner = canvas.sample_9x9(0, 0, false)?.ok_or("corner sample missing")?;
        canvas.wait()?;
        let middle = middle.try_pixels(&canvas.device())?.ok_or("middle sample still pending")?;
        let corner = corner.try_pixels(&canvas.device())?.ok_or("corner sample still pending")?;
        assert_eq!(middle.len(), 9 * 9 * 4);
        assert_eq!(corner.len(), 9 * 9 * 4);
        assert!(middle.get(160..163).is_some_and(|rgb| rgb[0] > 190 && rgb[1] > 40 && rgb[2] > 15));
        assert_eq!(corner.get(0..4), Some([0, 0, 0, 0].as_slice()));
        assert!(corner.get(160..164).is_some_and(|rgba| rgba[0] > 190 && rgba[3] == 255));
        Ok(())
    }
}
