//! Native raster exports use the same Vello scene as the editor when its pixels can be rendered
//! without a backdrop or unsupported compositing operation. Other exports use the CPU renderer.

use std::sync::OnceLock;

use figcraft_color::ColorProfile;
use figcraft_doc::{Document, EffectKind, NodeId};
use figcraft_geom::Rect;
use figcraft_render::{Pixmap, View};

use crate::vello_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_with_cached_noise_in_view};

type DevicePair = (wgpu::Device, wgpu::Queue);

static DEVICE: OnceLock<Option<DevicePair>> = OnceLock::new();

const EXPORT_TILE_SIDE: u32 = 2048;
const MAX_EXPORT_PIXELS: u64 = 64_000_000;

fn render_tiled(view: &View, mut render: impl FnMut(&View) -> Option<Pixmap>) -> Option<Pixmap> {
    if u64::from(view.width) * u64::from(view.height) > MAX_EXPORT_PIXELS {
        return None;
    }
    let mut result = Pixmap::new(view.width, view.height)?;
    let output_stride = usize::try_from(view.width).ok()?.checked_mul(4)?;
    for y in (0..view.height).step_by(EXPORT_TILE_SIDE as usize) {
        for x in (0..view.width).step_by(EXPORT_TILE_SIDE as usize) {
            let tile_view = View {
                origin: (view.origin.0 + f64::from(x) / view.scale, view.origin.1 + f64::from(y) / view.scale),
                scale: view.scale,
                width: (view.width - x).min(EXPORT_TILE_SIDE),
                height: (view.height - y).min(EXPORT_TILE_SIDE),
            };
            let tile = render(&tile_view)?;
            let tile_stride = usize::try_from(tile_view.width).ok()?.checked_mul(4)?;
            for row in 0..tile_view.height {
                let source = usize::try_from(row).ok()?.checked_mul(tile_stride)?;
                let target = usize::try_from(y + row).ok()?.checked_mul(output_stride)?.checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
                result.data_mut().get_mut(target..target + tile_stride)?.copy_from_slice(tile.data().get(source..source + tile_stride)?);
            }
        }
    }
    Some(result)
}

fn simple_tiling_supported(doc: &Document) -> bool {
    doc.nodes().all(|node| !node.visible || node.effects.iter().all(|effect| !effect.visible))
}

/// Rasterize selected page roots at export resolution on the native GPU. `None` means that the
/// faithful CPU renderer should handle this export instead.
pub fn render_area_gpu(doc: &Document, ids: &[NodeId], area: Rect, scale: f64) -> Option<Pixmap> {
    if ids.is_empty() || !scale.is_finite() || scale <= 0.0 || doc.color_profile != ColorProfile::Srgb {
        return None;
    }
    let [x0, y0, x1, y1] = [area.x0, area.y0, area.x1, area.y1];
    if ![x0, y0, x1, y1].into_iter().all(f64::is_finite) || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let width = ((area.width() * scale - 1e-6).ceil()).max(1.0);
    let height = ((area.height() * scale - 1e-6).ceil()).max(1.0);
    if !width.is_finite() || !height.is_finite() || width > f64::from(u32::MAX) || height > f64::from(u32::MAX) {
        return None;
    }
    let view = View { origin: (x0, y0), scale, width: width as u32, height: height as u32 };
    if view.width > 4096 || view.height > 4096 {
        return simple_tiling_supported(doc).then(|| render_tiled(&view, |tile| render_area_gpu(doc, ids, tile.page_rect(), scale)))?;
    }
    let page = doc.page_of(*ids.first()?)?;
    if ids.iter().any(|id| *id == page || doc.page_of(*id) != Some(page)) {
        return None;
    }
    let unsupported_subtree = doc.nodes().any(|node| {
        ids.iter().any(|id| *id == node.id || doc.is_ancestor(*id, node.id))
            && node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::Glass(_) | EffectKind::BackgroundBlur(_)))
    });
    if unsupported_subtree || has_unhandled_effects_with_cached_noise_in_view(doc, &view, true) {
        return None;
    }
    let (device, queue) = DEVICE.get_or_init(|| GpuCanvas::new(1, 1).ok().map(|canvas| canvas.device_queue())).as_ref()?;
    if view.width.max(view.height) > device.limits().max_texture_dimension_2d {
        return None;
    }
    let mut canvas = GpuCanvas::from_device(device.clone(), queue.clone(), "native raster export".into(), view.width, view.height);
    canvas.register_images(doc).ok()?;
    canvas.prepare_static_effects(doc).ok()?;
    canvas.render_export_roots(doc, ids, &view).ok()?;
    let pixels = canvas.pixels().ok()?;
    let mut pixmap = Pixmap::new(view.width, view.height)?;
    pixmap.data_mut().copy_from_slice(&pixels);
    Some(pixmap)
}

/// Render a page for CLI PNG/JPEG/WebP exports, retaining its background alpha and effect order.
pub fn render_page_gpu(doc: &Document, page: NodeId, view: &View) -> Option<Pixmap> {
    let page_node = doc.node(page)?;
    let clear = match &page_node.kind {
        figcraft_doc::NodeKind::Page { background, background_visible: true, .. } => [background.r, background.g, background.b, background.a],
        figcraft_doc::NodeKind::Page { .. } => [0.0; 4],
        _ => return None,
    };
    if doc.color_profile != ColorProfile::Srgb
        || view.width == 0
        || view.height == 0
        || has_unhandled_effects_with_cached_noise_in_view(doc, view, true)
    {
        return None;
    }
    if view.width > 4096 || view.height > 4096 {
        return simple_tiling_supported(doc).then(|| render_tiled(view, |tile| render_page_gpu(doc, page, tile)))?;
    }
    let (device, queue) = DEVICE.get_or_init(|| GpuCanvas::new(1, 1).ok().map(|canvas| canvas.device_queue())).as_ref()?;
    if view.width.max(view.height) > device.limits().max_texture_dimension_2d {
        return None;
    }
    let mut canvas = GpuCanvas::from_device(device.clone(), queue.clone(), "native page export".into(), view.width, view.height);
    canvas.register_images(doc).ok()?;
    canvas.prepare_static_effects(doc).ok()?;
    let backdrops = BackdropIndex::new(doc, page);
    if backdrops.visible_ids(view).is_empty() {
        canvas.render_submit_document_rgba(doc, page, view, clear).ok()?;
    } else {
        // The same ordered backdrop pass used by the native editor is required here. The
        // capability check above excludes its CPU-only cases before this call.
        canvas.render_with_backdrop_blurs_indexed_rgba(doc, page, view, clear, &backdrops).ok()?;
    }
    let pixels = canvas.pixels().ok()?;
    let mut pixmap = Pixmap::new(view.width, view.height)?;
    pixmap.data_mut().copy_from_slice(&pixels);
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use figcraft_color::{Color, Paint};
    use figcraft_doc::NodeKind;

    #[test]
    fn gpu_export_keeps_alpha_and_only_draws_selected_roots() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("transparent export");
        let page = doc.current_page.ok_or("page missing")?;
        let selected = doc.create(page, NodeKind::Rectangle, "Selected", 16.0, 16.0, 48.0, 48.0)?;
        let other = doc.create(page, NodeKind::Rectangle, "Other", 72.0, 16.0, 16.0, 16.0)?;
        doc.node_mut(selected).ok_or("selected node missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 0.5))];
        doc.node_mut(other).ok_or("other node missing")?.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        let area = Rect::new(0.0, 0.0, 96.0, 96.0);
        let actual = render_area_gpu(&doc, &[selected], area, 1.0).ok_or("GPU export unavailable")?;
        let reference = figcraft_render::render_area(&doc, &[selected], area, 1.0).ok_or("reference export unavailable")?;
        assert_eq!(actual.width(), 96);
        assert_eq!(actual.height(), 96);
        assert_eq!(actual.pixel(4, 4).ok_or("corner missing")?.alpha(), 0);
        assert_eq!(actual.pixel(80, 20).ok_or("excluded layer pixel missing")?.alpha(), 0);
        let expected = reference.pixel(32, 32).ok_or("reference center missing")?;
        let found = actual.pixel(32, 32).ok_or("GPU center missing")?;
        assert!(found.alpha().abs_diff(expected.alpha()) <= 2);
        for (a, b) in [found.red(), found.green(), found.blue()].into_iter().zip([expected.red(), expected.green(), expected.blue()]) {
            assert!(a.abs_diff(b) <= 2);
        }
        Ok(())
    }

    #[test]
    fn backdrop_effects_keep_cpu_export() -> Result<(), Box<dyn std::error::Error>> {
        let mut doc = Document::new("backdrop export");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Glass", 8.0, 8.0, 40.0, 40.0)?;
        doc.node_mut(id)
            .ok_or("glass node missing")?
            .effects
            .push(figcraft_doc::Effect { kind: EffectKind::Glass(Default::default()), visible: true });
        assert!(render_area_gpu(&doc, &[id], Rect::new(0.0, 0.0, 64.0, 64.0), 1.0).is_none());
        Ok(())
    }

    #[test]
    fn gpu_exports_nested_selection_and_opaque_page() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("nested export");
        let page = doc.current_page.ok_or("page missing")?;
        if let Some(NodeKind::Page { background, background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background = Color::new(0.8, 0.9, 1.0, 1.0);
            *background_visible = true;
        }
        let group = doc.create(page, NodeKind::Group, "Group", 12.0, 8.0, 64.0, 64.0)?;
        let child = doc.create(group, NodeKind::Rectangle, "Child", 5.0, 7.0, 24.0, 24.0)?;
        doc.node_mut(child).ok_or("child missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.6, 0.3, 1.0))];
        let area = Rect::new(0.0, 0.0, 80.0, 80.0);
        let selected = render_area_gpu(&doc, &[child], area, 1.0).ok_or("nested GPU export unavailable")?;
        let reference = figcraft_render::render_area(&doc, &[child], area, 1.0).ok_or("nested reference unavailable")?;
        assert_eq!(selected.pixel(20, 20), reference.pixel(20, 20));
        assert_eq!(selected.pixel(2, 2).ok_or("empty pixel missing")?.alpha(), 0);
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 80, height: 80 };
        let actual_page = render_page_gpu(&doc, page, &view).ok_or("page GPU export unavailable")?;
        let reference_page = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("page reference unavailable")?;
        for (x, y) in [(2, 2), (20, 20)] {
            let actual = actual_page.pixel(x, y).ok_or("GPU pixel missing")?;
            let expected = reference_page.pixel(x, y).ok_or("reference pixel missing")?;
            assert_eq!(actual.alpha(), expected.alpha());
            for (a, b) in [actual.red(), actual.green(), actual.blue()].into_iter().zip([expected.red(), expected.green(), expected.blue()]) {
                assert!(a.abs_diff(b) <= 2);
            }
        }
        Ok(())
    }

    #[test]
    fn wide_gpu_exports_keep_pixels_at_tile_edges() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("wide export");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Wide", 0.0, 0.0, 4104.0, 24.0)?;
        doc.node_mut(id).ok_or("shape missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.6, 0.9, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 4104, height: 24 };
        let gpu_page = render_page_gpu(&doc, page, &view).ok_or("wide GPU page unavailable")?;
        let cpu_page = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("wide CPU page unavailable")?;
        let gpu_area = render_area_gpu(&doc, &[id], Rect::new(0.0, 0.0, 4104.0, 24.0), 1.0).ok_or("wide GPU selection unavailable")?;
        let cpu_area = figcraft_render::render_area(&doc, &[id], Rect::new(0.0, 0.0, 4104.0, 24.0), 1.0).ok_or("wide CPU selection unavailable")?;
        for x in [0, 2047, 2048, 4095, 4096, 4103] {
            assert_eq!(gpu_page.pixel(x, 12), cpu_page.pixel(x, 12), "page x={x}");
            assert_eq!(gpu_area.pixel(x, 12), cpu_area.pixel(x, 12), "area x={x}");
        }
        Ok(())
    }

    #[test]
    fn gpu_selected_group_export_keeps_its_alpha_mask() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("masked selection");
        let page = doc.current_page.ok_or("page missing")?;
        let group = doc.create(page, NodeKind::Group, "Group", 0.0, 0.0, 80.0, 80.0)?;
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 0.0, 40.0, 80.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let shape = doc.create(group, NodeKind::Rectangle, "Shape", 0.0, 0.0, 80.0, 80.0)?;
        doc.node_mut(shape).ok_or("shape missing")?.fills = vec![Paint::solid(Color::new(0.8, 0.2, 0.1, 1.0))];
        let area = Rect::new(0.0, 0.0, 80.0, 80.0);
        let gpu = render_area_gpu(&doc, &[group], area, 1.0).ok_or("masked GPU selection unavailable")?;
        let cpu = figcraft_render::render_area(&doc, &[group], area, 1.0).ok_or("masked CPU selection unavailable")?;
        for x in [20, 60] {
            let actual = gpu.pixel(x, 40).ok_or("GPU sample missing")?;
            let expected = cpu.pixel(x, 40).ok_or("CPU sample missing")?;
            assert!(actual.alpha().abs_diff(expected.alpha()) <= 2);
            assert!(actual.red().abs_diff(expected.red()) <= 2);
        }
        Ok(())
    }

    #[test]
    fn simple_leaf_blend_modes_match_cpu_samples() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("blend export");
        let page = doc.current_page.ok_or("page missing")?;
        if let Some(NodeKind::Page { background, background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background = Color::WHITE;
            *background_visible = true;
        }
        let base = doc.create(page, NodeKind::Rectangle, "Base", 0.0, 0.0, 80.0, 80.0)?;
        doc.node_mut(base).ok_or("base missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.5, 0.8, 1.0))];
        let top = doc.create(page, NodeKind::Rectangle, "Top", 10.0, 10.0, 60.0, 60.0)?;
        doc.node_mut(top).ok_or("top missing")?.fills = vec![Paint::solid(Color::new(0.8, 0.4, 0.2, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 80, height: 80 };
        for blend in figcraft_color::BlendMode::ALL {
            if matches!(blend, figcraft_color::BlendMode::PassThrough | figcraft_color::BlendMode::Normal | figcraft_color::BlendMode::PlusDarker) {
                continue;
            }
            doc.node_mut(top).ok_or("top missing")?.blend = blend;
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{blend:?} rejected");
            let gpu = render_page_gpu(&doc, page, &view).ok_or("GPU blend export unavailable")?;
            let cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("CPU blend reference unavailable")?;
            let actual = gpu.pixel(40, 40).ok_or("GPU sample missing")?;
            let expected = cpu.pixel(40, 40).ok_or("CPU sample missing")?;
            for (a, b) in [actual.red(), actual.green(), actual.blue()].into_iter().zip([expected.red(), expected.green(), expected.blue()]) {
                assert!(a.abs_diff(b) <= 10, "{blend:?}: GPU={actual:?}, CPU={expected:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn simple_paint_blend_modes_match_cpu_samples() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("paint blend export");
        let page = doc.current_page.ok_or("page missing")?;
        let base = doc.create(page, NodeKind::Rectangle, "Base", 0.0, 0.0, 80.0, 80.0)?;
        doc.node_mut(base).ok_or("base missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.5, 0.8, 1.0))];
        let top = doc.create(page, NodeKind::Rectangle, "Top", 10.0, 10.0, 60.0, 60.0)?;
        doc.node_mut(top).ok_or("top missing")?.fills = vec![Paint::solid(Color::new(0.8, 0.4, 0.2, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 80, height: 80 };
        for blend in figcraft_color::BlendMode::ALL {
            if matches!(blend, figcraft_color::BlendMode::PassThrough | figcraft_color::BlendMode::Normal | figcraft_color::BlendMode::PlusDarker) {
                continue;
            }
            doc.node_mut(top).ok_or("top missing")?.fills[0].blend = blend;
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{blend:?} rejected");
            let gpu = render_page_gpu(&doc, page, &view).ok_or("GPU paint blend export unavailable")?;
            let cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("CPU paint blend reference unavailable")?;
            let actual = gpu.pixel(40, 40).ok_or("GPU sample missing")?;
            let expected = cpu.pixel(40, 40).ok_or("CPU sample missing")?;
            for (a, b) in [actual.red(), actual.green(), actual.blue()].into_iter().zip([expected.red(), expected.green(), expected.blue()]) {
                assert!(a.abs_diff(b) <= 10, "{blend:?}: GPU={actual:?}, CPU={expected:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn gpu_page_export_keeps_mask_and_ordered_backdrop() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("page with backdrop");
        let page = doc.current_page.ok_or("page missing")?;
        if let Some(NodeKind::Page { background, background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background = Color::new(245.0 / 255.0, 245.0 / 255.0, 245.0 / 255.0, 1.0);
            *background_visible = true;
        }
        let dark = doc.create(page, NodeKind::Rectangle, "Dark half", 0.0, 0.0, 50.0, 100.0)?;
        doc.node_mut(dark).ok_or("dark half missing")?.fills = vec![Paint::solid(Color::BLACK)];
        let pane = doc.create(page, NodeKind::Rectangle, "Blur pane", 20.0, 20.0, 60.0, 60.0)?;
        let node = doc.node_mut(pane).ok_or("pane missing")?;
        node.fills.clear();
        node.effects = vec![figcraft_doc::Effect { kind: EffectKind::BackgroundBlur(figcraft_doc::Blur::uniform(12.0)), visible: true }];
        let group = doc.create(page, NodeKind::Group, "Masked group", 0.0, 0.0, 20.0, 20.0)?;
        let mask = doc.create(group, NodeKind::Rectangle, "Mask", 0.0, 0.0, 10.0, 20.0)?;
        doc.node_mut(mask).ok_or("mask missing")?.is_mask = true;
        let content = doc.create(group, NodeKind::Rectangle, "Content", 0.0, 0.0, 20.0, 20.0)?;
        doc.node_mut(content).ok_or("content missing")?.fills = vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
        let actual = render_page_gpu(&doc, page, &view).ok_or("GPU page export unavailable")?;
        let reference = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("reference export unavailable")?;
        for (x, y) in [(5, 10), (15, 10), (35, 50), (47, 50), (53, 50), (65, 50)] {
            let gpu = actual.pixel(x, y).ok_or("GPU sample missing")?;
            let cpu = reference.pixel(x, y).ok_or("CPU sample missing")?;
            assert!(gpu.red().abs_diff(cpu.red()) <= 20, "x={x}, y={y}: GPU={gpu:?}, CPU={cpu:?}");
        }
        if let Some(NodeKind::Page { background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background_visible = false;
        }
        let transparent_gpu = render_page_gpu(&doc, page, &view).ok_or("transparent GPU page export unavailable")?;
        let transparent_cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("transparent reference unavailable")?;
        for (x, y) in [(5, 10), (15, 10), (35, 50), (47, 50), (53, 50), (65, 50), (95, 95)] {
            let gpu = transparent_gpu.pixel(x, y).ok_or("transparent GPU sample missing")?;
            let cpu = transparent_cpu.pixel(x, y).ok_or("transparent CPU sample missing")?;
            assert!(gpu.alpha().abs_diff(cpu.alpha()) <= 20, "x={x}, y={y}: GPU alpha={gpu:?}, CPU alpha={cpu:?}");
        }
        Ok(())
    }
}
