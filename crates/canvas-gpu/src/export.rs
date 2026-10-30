//! Native raster exports use the same Vello scene as the editor when its pixels can be rendered
//! without a backdrop or unsupported compositing operation. Other exports use the CPU renderer.

use std::sync::OnceLock;

use figcraft_doc::{Document, EffectKind, NodeId};
use figcraft_geom::Rect;
use figcraft_render::{Pixmap, View};

use crate::vello_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_with_cached_noise_in_view};

type DevicePair = (wgpu::Device, wgpu::Queue);

static DEVICE: OnceLock<Option<DevicePair>> = OnceLock::new();

const EXPORT_TILE_SIDE: u32 = 2048;
const MAX_EXPORT_PIXELS: u64 = 64_000_000;

fn render_tiled(view: &View, halo: u32, mut render: impl FnMut(&View) -> Option<Pixmap>) -> Option<Pixmap> {
    if u64::from(view.width) * u64::from(view.height) > MAX_EXPORT_PIXELS {
        return None;
    }
    let mut result = Pixmap::new(view.width, view.height)?;
    let output_stride = usize::try_from(view.width).ok()?.checked_mul(4)?;
    for y in (0..view.height).step_by(EXPORT_TILE_SIDE as usize) {
        for x in (0..view.width).step_by(EXPORT_TILE_SIDE as usize) {
            let core_width = (view.width - x).min(EXPORT_TILE_SIDE);
            let core_height = (view.height - y).min(EXPORT_TILE_SIDE);
            let left = x.saturating_sub(halo);
            let top = y.saturating_sub(halo);
            let right = x.saturating_add(core_width).saturating_add(halo).min(view.width);
            let bottom = y.saturating_add(core_height).saturating_add(halo).min(view.height);
            let tile_view = View {
                origin: (view.origin.0 + f64::from(left) / view.scale, view.origin.1 + f64::from(top) / view.scale),
                scale: view.scale,
                width: right - left,
                height: bottom - top,
            };
            let tile = render(&tile_view)?;
            let tile_stride = usize::try_from(tile_view.width).ok()?.checked_mul(4)?;
            let copy_stride = usize::try_from(core_width).ok()?.checked_mul(4)?;
            for row in 0..core_height {
                let source =
                    usize::try_from(y - top + row).ok()?.checked_mul(tile_stride)?.checked_add(usize::try_from(x - left).ok()?.checked_mul(4)?)?;
                let target = usize::try_from(y + row).ok()?.checked_mul(output_stride)?.checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
                result.data_mut().get_mut(target..target + copy_stride)?.copy_from_slice(tile.data().get(source..source + copy_stride)?);
            }
        }
    }
    Some(result)
}

/// Conservative blur/shadow footprint in device pixels. Large or backdrop-dependent effects
/// still use the fallback until they can be tiled without breaking compositing order.
fn effect_tile_halo(doc: &Document, scale: f64) -> Option<u32> {
    let mut reach = 0.0f64;
    for node in doc.nodes().filter(|node| node.visible) {
        if node.effects.iter().all(|effect| !effect.visible) {
            continue;
        }
        let [a, b, c, d, _, _] = doc.world_transform(node.id).as_coeffs();
        let sum = a * a + b * b + c * c + d * d;
        let determinant = a * d - b * c;
        let discriminant = (sum * sum - 4.0 * determinant * determinant).max(0.0);
        let world_scale = ((sum + discriminant.sqrt()) * 0.5).sqrt();
        if !world_scale.is_finite() {
            return None;
        }
        for effect in node.effects.iter().filter(|effect| effect.visible) {
            let local = match &effect.kind {
                EffectKind::DropShadow(shadow) => {
                    shadow.radius.abs() * 6.0 + shadow.spread.abs() * 4.0 + shadow.offset.0.abs() + shadow.offset.1.abs()
                }
                EffectKind::InnerShadow(shadow) => shadow.radius.abs() * 6.0 + shadow.spread.abs() * 4.0,
                EffectKind::LayerBlur(blur) if !blur.is_progressive() => blur.max_radius() * 6.0,
                _ => return None,
            };
            if !local.is_finite() {
                return None;
            }
            reach = reach.max(local * world_scale);
        }
    }
    if reach == 0.0 {
        return Some(0);
    }
    let pixels = (reach * scale + 2.0).ceil();
    (pixels.is_finite() && pixels <= 512.0).then_some(pixels as u32)
}

/// Rasterize selected page roots at export resolution on the native GPU. `None` means that the
/// faithful CPU renderer should handle this export instead.
pub fn render_area_gpu(doc: &Document, ids: &[NodeId], area: Rect, scale: f64) -> Option<Pixmap> {
    render_area_gpu_at(doc, ids, area, scale, 0.0)
}

/// Render a selected subtree using animated image frames at `time` seconds.
pub fn render_area_gpu_at(doc: &Document, ids: &[NodeId], area: Rect, scale: f64, time: f64) -> Option<Pixmap> {
    render_area_gpu_internal(doc, ids, area, scale, time, false)
}

/// GPU-rendered selected layers converted to sRGB for native screen textures.
pub fn render_area_gpu_screen(doc: &Document, ids: &[NodeId], area: Rect, scale: f64) -> Option<Pixmap> {
    render_area_gpu_internal(doc, ids, area, scale, 0.0, true)
}

/// Screen-ready animated selection at `time` seconds.
pub fn render_area_gpu_screen_at(doc: &Document, ids: &[NodeId], area: Rect, scale: f64, time: f64) -> Option<Pixmap> {
    render_area_gpu_internal(doc, ids, area, scale, time, true)
}

fn render_area_gpu_internal(doc: &Document, ids: &[NodeId], area: Rect, scale: f64, time: f64, screen: bool) -> Option<Pixmap> {
    if ids.is_empty() || !scale.is_finite() || scale <= 0.0 {
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
        let halo = effect_tile_halo(doc, scale)?;
        return render_tiled(&view, halo, |tile| render_area_gpu_internal(doc, ids, tile.page_rect(), scale, time, screen));
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
    canvas.register_images_at_scale(doc, time, view.scale).ok()?;
    canvas.prepare_static_effects(doc).ok()?;
    canvas.render_export_roots(doc, ids, &view).ok()?;
    if screen {
        canvas.convert_for_presentation(doc.color_profile);
    }
    let pixels = if screen { canvas.presented_pixels().ok()? } else { canvas.pixels().ok()? };
    let mut pixmap = Pixmap::new(view.width, view.height)?;
    pixmap.data_mut().copy_from_slice(&pixels);
    Some(pixmap)
}

/// Render a page for CLI PNG/JPEG/WebP exports, retaining its background alpha and effect order.
pub fn render_page_gpu(doc: &Document, page: NodeId, view: &View) -> Option<Pixmap> {
    render_page_gpu_internal(doc, page, view, false)
}

/// GPU-rendered page converted to sRGB for native screen textures.
pub fn render_page_gpu_screen(doc: &Document, page: NodeId, view: &View) -> Option<Pixmap> {
    render_page_gpu_internal(doc, page, view, true)
}

fn render_page_gpu_internal(doc: &Document, page: NodeId, view: &View, screen: bool) -> Option<Pixmap> {
    let page_node = doc.node(page)?;
    let clear = match &page_node.kind {
        figcraft_doc::NodeKind::Page { background, background_visible: true, .. } => [background.r, background.g, background.b, background.a],
        figcraft_doc::NodeKind::Page { .. } => [0.0; 4],
        _ => return None,
    };
    if view.width == 0 || view.height == 0 || has_unhandled_effects_with_cached_noise_in_view(doc, view, true) {
        return None;
    }
    if view.width > 4096 || view.height > 4096 {
        let halo = effect_tile_halo(doc, view.scale)?;
        return render_tiled(view, halo, |tile| render_page_gpu_internal(doc, page, tile, screen));
    }
    let (device, queue) = DEVICE.get_or_init(|| GpuCanvas::new(1, 1).ok().map(|canvas| canvas.device_queue())).as_ref()?;
    if view.width.max(view.height) > device.limits().max_texture_dimension_2d {
        return None;
    }
    let mut canvas = GpuCanvas::from_device(device.clone(), queue.clone(), "native page export".into(), view.width, view.height);
    canvas.register_images_at_scale(doc, 0.0, view.scale).ok()?;
    canvas.prepare_static_effects(doc).ok()?;
    let backdrops = BackdropIndex::new(doc, page);
    if backdrops.visible_ids(view).is_empty() {
        canvas.render_submit_document_rgba(doc, page, view, clear).ok()?;
    } else {
        // The same ordered backdrop pass used by the native editor is required here. The
        // capability check above excludes its CPU-only cases before this call.
        canvas.render_with_backdrop_blurs_indexed_rgba(doc, page, view, clear, &backdrops).ok()?;
    }
    if screen {
        canvas.convert_for_presentation(doc.color_profile);
    }
    let pixels = if screen { canvas.presented_pixels().ok()? } else { canvas.pixels().ok()? };
    let mut pixmap = Pixmap::new(view.width, view.height)?;
    pixmap.data_mut().copy_from_slice(&pixels);
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use figcraft_color::{
        Color, ColorProfile, ColorStop, Gradient, GradientKind, ImageFilters, ImageScaleMode, Paint, PaintKind, PatternPaint, PatternTile,
    };
    use figcraft_doc::{Blur, Effect, EffectKind, ImageData, NodeKind, Shadow};
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, RgbaImage};
    use std::sync::Arc;

    #[test]
    fn wide_shadow_export_matches_cpu_across_tile_seams() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("wide shadow export");
        let page = doc.current_page.ok_or("page missing")?;
        if let Some(NodeKind::Page { background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background_visible = false;
        }
        let id = doc.create(page, NodeKind::Rectangle, "Across seam", 2036.0, 32.0, 48.0, 48.0)?;
        let shadow = Shadow { radius: 5.0, offset: (5.0, 7.0), ..Shadow::default() };
        let node = doc.node_mut(id).ok_or("shadow node missing")?;
        node.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 1.0))];
        node.effects = vec![Effect { kind: EffectKind::DropShadow(shadow), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 4200, height: 128 };
        let gpu = render_page_gpu(&doc, page, &view).ok_or("wide GPU shadow page unavailable")?;
        let cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("wide CPU shadow page unavailable")?;
        let area = Rect::new(0.0, 0.0, 4200.0, 128.0);
        let gpu_selected = render_area_gpu(&doc, &[id], area, 1.0).ok_or("wide GPU shadow selection unavailable")?;
        let cpu_selected = figcraft_render::render_area(&doc, &[id], area, 1.0).ok_or("wide CPU shadow selection unavailable")?;
        for y in [34, 60, 81, 87, 96] {
            for x in [2026, 2038, 2047, 2048, 2056, 2078, 2090] {
                for (actual_image, expected_image) in [(&gpu, &cpu), (&gpu_selected, &cpu_selected)] {
                    let actual = actual_image.pixel(x, y).ok_or("GPU shadow sample missing")?;
                    let expected = expected_image.pixel(x, y).ok_or("CPU shadow sample missing")?;
                    for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                        expected.red(),
                        expected.green(),
                        expected.blue(),
                        expected.alpha(),
                    ]) {
                        assert!(a.abs_diff(b) <= 12, "at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn wide_layer_blur_export_keeps_tile_seam_continuous() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("wide blur export");
        let page = doc.current_page.ok_or("page missing")?;
        if let Some(NodeKind::Page { background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background_visible = false;
        }
        let id = doc.create(page, NodeKind::Rectangle, "Across seam", 2030.0, 32.0, 64.0, 64.0)?;
        let node = doc.node_mut(id).ok_or("blur node missing")?;
        node.fills = vec![Paint::solid(Color::new(0.9, 0.1, 0.3, 1.0))];
        node.effects = vec![Effect { kind: EffectKind::LayerBlur(Blur::uniform(5.0)), visible: true }];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 4200, height: 128 };
        let gpu = render_page_gpu(&doc, page, &view).ok_or("wide GPU blur unavailable")?;
        let cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("wide CPU blur unavailable")?;
        for y in [25, 32, 40, 80, 96, 103] {
            for x in [2022, 2030, 2047, 2048, 2064, 2094, 2102] {
                let actual = gpu.pixel(x, y).ok_or("GPU blur sample missing")?;
                let expected = cpu.pixel(x, y).ok_or("CPU blur sample missing")?;
                for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                    expected.red(),
                    expected.green(),
                    expected.blue(),
                    expected.alpha(),
                ]) {
                    assert!(a.abs_diff(b) <= 12, "at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn scaled_shadow_export_keeps_gpu_tile_halo() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("scaled shadow export");
        let page = doc.current_page.ok_or("page missing")?;
        if let Some(NodeKind::Page { background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background_visible = false;
        }
        let id = doc.create(page, NodeKind::Rectangle, "Scaled across seam", 1018.0, 16.0, 48.0, 48.0)?;
        let node = doc.node_mut(id).ok_or("scaled shadow node missing")?;
        node.transform = figcraft_geom::Affine::translate((1018.0, 16.0)) * figcraft_geom::Affine::scale(2.0);
        node.fills = vec![Paint::solid(Color::new(0.2, 0.5, 0.8, 1.0))];
        node.effects = vec![Effect { kind: EffectKind::DropShadow(Shadow { radius: 5.0, offset: (5.0, 7.0), ..Shadow::default() }), visible: true }];
        assert!(effect_tile_halo(&doc, 2.0).is_some_and(|halo| halo >= 120));
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 4200, height: 256 };
        let gpu = render_page_gpu(&doc, page, &view).ok_or("scaled GPU shadow unavailable")?;
        let cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("scaled CPU shadow unavailable")?;
        for y in [25, 40, 80, 180, 230, 245] {
            for x in [2010, 2038, 2047, 2048, 2060, 2100, 2200] {
                let actual = gpu.pixel(x, y).ok_or("GPU scaled shadow sample missing")?;
                let expected = cpu.pixel(x, y).ok_or("CPU scaled shadow sample missing")?;
                for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                    expected.red(),
                    expected.green(),
                    expected.blue(),
                    expected.alpha(),
                ]) {
                    assert!(a.abs_diff(b) <= 12, "at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn gpu_pattern_cells_match_cpu_for_grid_and_hex_tiles() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        for tile in [PatternTile::Grid, PatternTile::HorizontalHex, PatternTile::VerticalHex] {
            let mut doc = Document::new("GPU pattern");
            let page = doc.current_page.ok_or("page missing")?;
            let source = doc.create(page, NodeKind::Rectangle, "Tile", 300.0, 300.0, 8.0, 8.0)?;
            doc.node_mut(source).ok_or("source missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.2, 0.1, 0.8))];
            doc.node_mut(source).ok_or("source missing for opacity")?.opacity = 0.85;
            let target = doc.create(page, NodeKind::Rectangle, "Pattern", 0.0, 0.0, 64.0, 48.0)?;
            let mut pattern = PatternPaint::new(source.0);
            pattern.tile = tile;
            pattern.spacing = (4.0, 4.0);
            let mut paint = Paint::solid(Color::WHITE);
            paint.kind = PaintKind::Pattern(pattern);
            paint.opacity = 0.75;
            doc.node_mut(target).ok_or("target missing")?.fills = vec![paint];
            let area = Rect::new(0.0, 0.0, 64.0, 48.0);
            for scale in [1.0, 2.0] {
                let view = View { origin: (0.0, 0.0), scale, width: (64.0 * scale) as u32, height: (48.0 * scale) as u32 };
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
                let gpu = render_area_gpu(&doc, &[target], area, scale).ok_or("pattern GPU export unavailable")?;
                let cpu = figcraft_render::render_area(&doc, &[target], area, scale).ok_or("pattern CPU export unavailable")?;
                for y in [2, 6, 11, 14, 20, 25, 33, 40] {
                    for x in [2, 7, 11, 15, 23, 34, 45, 55] {
                        let px = (f64::from(x) * scale) as u32;
                        let py = (f64::from(y) * scale) as u32;
                        let actual = gpu.pixel(px, py).ok_or("GPU pattern sample missing")?;
                        let expected = cpu.pixel(px, py).ok_or("CPU pattern sample missing")?;
                        for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                            expected.red(),
                            expected.green(),
                            expected.blue(),
                            expected.alpha(),
                        ]) {
                            assert!(a.abs_diff(b) <= 12, "tile={tile:?} scale={scale} at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn gpu_pattern_group_source_matches_cpu() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("GPU pattern group");
        let page = doc.current_page.ok_or("page missing")?;
        let source = doc.create(page, NodeKind::Group, "Tile", 300.0, 300.0, 8.0, 8.0)?;
        let left = doc.create(source, NodeKind::Rectangle, "Left", 0.0, 0.0, 4.0, 8.0)?;
        let right = doc.create(source, NodeKind::Rectangle, "Right", 4.0, 0.0, 4.0, 8.0)?;
        doc.node_mut(left).ok_or("left missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.2, 0.1, 1.0))];
        doc.node_mut(right).ok_or("right missing")?.fills = vec![Paint::solid(Color::new(0.1, 0.2, 0.9, 1.0))];
        let target = doc.create(page, NodeKind::Rectangle, "Pattern", 0.0, 0.0, 40.0, 24.0)?;
        let mut paint = Paint::solid(Color::WHITE);
        paint.kind = PaintKind::Pattern(PatternPaint::new(source.0));
        doc.node_mut(target).ok_or("target missing")?.fills = vec![paint];
        let area = Rect::new(0.0, 0.0, 40.0, 24.0);
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 80, height: 48 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        let gpu = render_area_gpu(&doc, &[target], area, 2.0).ok_or("pattern group GPU export unavailable")?;
        let cpu = figcraft_render::render_area(&doc, &[target], area, 2.0).ok_or("pattern group CPU export unavailable")?;
        for y in [2, 6, 10, 14, 18, 22] {
            for x in [2, 6, 10, 14, 18, 22, 26, 30, 34, 38] {
                let actual = gpu.pixel(x * 2, y * 2).ok_or("GPU group pattern pixel missing")?;
                let expected = cpu.pixel(x * 2, y * 2).ok_or("CPU group pattern pixel missing")?;
                for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                    expected.red(),
                    expected.green(),
                    expected.blue(),
                    expected.alpha(),
                ]) {
                    assert!(a.abs_diff(b) <= 12, "group pattern at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn gpu_pattern_masked_group_source_matches_cpu() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("GPU masked pattern group");
        let page = doc.current_page.ok_or("page missing")?;
        let source = doc.create(page, NodeKind::Group, "Tile", 300.0, 300.0, 8.0, 8.0)?;
        let mask = doc.create(source, NodeKind::Rectangle, "Mask", 0.0, 0.0, 4.0, 8.0)?;
        let mask_node = doc.node_mut(mask).ok_or("mask missing")?;
        mask_node.is_mask = true;
        mask_node.fills = vec![Paint::solid(Color::WHITE)];
        let colored = doc.create(source, NodeKind::Rectangle, "Colored", 0.0, 0.0, 8.0, 8.0)?;
        doc.node_mut(colored).ok_or("colored shape missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.2, 0.1, 1.0))];
        let target = doc.create(page, NodeKind::Rectangle, "Pattern", 0.0, 0.0, 32.0, 16.0)?;
        let mut paint = Paint::solid(Color::WHITE);
        paint.kind = PaintKind::Pattern(PatternPaint::new(source.0));
        doc.node_mut(target).ok_or("target missing")?.fills = vec![paint];
        let area = Rect::new(0.0, 0.0, 32.0, 16.0);
        let view = View { origin: (0.0, 0.0), scale: 2.0, width: 64, height: 32 };
        for mask_type in [figcraft_doc::MaskType::Alpha, figcraft_doc::MaskType::Vector, figcraft_doc::MaskType::Luminance] {
            doc.node_mut(mask).ok_or("mask missing for type")?.mask_type = mask_type;
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
            let gpu = render_area_gpu(&doc, &[target], area, 2.0).ok_or("masked pattern GPU export unavailable")?;
            let cpu = figcraft_render::render_area(&doc, &[target], area, 2.0).ok_or("masked pattern CPU export unavailable")?;
            for y in [2, 6, 10, 14] {
                for x in [2, 6, 10, 14, 18, 22, 26, 30] {
                    let actual = gpu.pixel(x * 2, y * 2).ok_or("GPU masked pattern pixel missing")?;
                    let expected = cpu.pixel(x * 2, y * 2).ok_or("CPU masked pattern pixel missing")?;
                    for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                        expected.red(),
                        expected.green(),
                        expected.blue(),
                        expected.alpha(),
                    ]) {
                        assert!(a.abs_diff(b) <= 12, "{mask_type:?} pattern at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn shader_fill_stays_on_gpu_and_matches_cpu_at_zoom() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("GPU shader fill");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Shader", 8.0, 6.0, 64.0, 40.0)?;
        let definition = figcraft_shader::demo();
        doc.shaders.insert(definition.id.clone(), definition);
        let mut paint = Paint::solid(Color::WHITE);
        paint.kind =
            PaintKind::Shader(figcraft_color::ShaderFill { shader_id: figcraft_shader::DEMO_ID.into(), values: [("frequency".into(), 4.0)].into() });
        paint.opacity = 0.7;
        doc.node_mut(id).ok_or("shader node missing")?.fills = vec![paint];
        let area = Rect::new(0.0, 0.0, 80.0, 56.0);
        for scale in [1.0, 2.0] {
            let view = View { origin: (0.0, 0.0), scale, width: (80.0 * scale) as u32, height: (56.0 * scale) as u32 };
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
            let gpu = render_area_gpu(&doc, &[id], area, scale).ok_or("shader GPU export unavailable")?;
            let cpu = figcraft_render::render_area(&doc, &[id], area, scale).ok_or("shader CPU export unavailable")?;
            for (x, y) in [(20, 20), (40, 24), (60, 28), (70, 40)] {
                let px = (f64::from(x) * scale) as u32;
                let py = (f64::from(y) * scale) as u32;
                let actual = gpu.pixel(px, py).ok_or("GPU shader sample missing")?;
                let expected = cpu.pixel(px, py).ok_or("CPU shader sample missing")?;
                for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                    expected.red(),
                    expected.green(),
                    expected.blue(),
                    expected.alpha(),
                ]) {
                    assert!(a.abs_diff(b) <= 12, "scale={scale}, at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                }
            }
        }
        Ok(())
    }

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
    fn animated_image_frames_render_on_gpu_at_presentation_time() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.set_repeat(Repeat::Infinite)?;
            let frames = [[255, 0, 0, 255], [0, 255, 0, 255]]
                .into_iter()
                .map(|color| Frame::from_parts(RgbaImage::from_pixel(4, 4, image::Rgba(color)), 0, 0, Delay::from_numer_denom_ms(100, 1)));
            encoder.encode_frames(frames)?;
        }
        let mut doc = Document::new("animated GPU image");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "GIF", 0.0, 0.0, 4.0, 4.0)?;
        let hash = "animated-export-test".to_string();
        doc.images.insert(hash.clone(), ImageData { bytes: Arc::new(bytes), width: 4, height: 4 });
        doc.node_mut(id).ok_or("GIF node missing")?.fills = vec![Paint::image(hash, figcraft_color::ImageScaleMode::Fill)];
        for (time, expected) in [(0.0, [255, 0, 0]), (0.15, [0, 255, 0]), (0.25, [255, 0, 0])] {
            let gpu = render_area_gpu_at(&doc, &[id], Rect::new(0.0, 0.0, 4.0, 4.0), 1.0, time).ok_or("animated GPU export unavailable")?;
            let pixel = gpu.pixel(2, 2).ok_or("GIF sample missing")?;
            assert_eq!([pixel.red(), pixel.green(), pixel.blue()], expected, "time={time}");
        }
        if let Some(Paint { kind: PaintKind::Image(image), .. }) = doc.node_mut(id).and_then(|node| node.fills.first_mut()) {
            image.filters.saturation = -1.0;
        }
        let gpu = render_area_gpu_at(&doc, &[id], Rect::new(0.0, 0.0, 4.0, 4.0), 1.0, 0.15).ok_or("adjusted animated GPU export unavailable")?;
        let cpu = figcraft_render::render_node_at(&doc, id, 1.0, 0.15).ok_or("animated CPU reference unavailable")?;
        let actual = gpu.pixel(2, 2).ok_or("adjusted GIF sample missing")?;
        let expected = cpu.pixel(2, 2).ok_or("adjusted CPU GIF sample missing")?;
        for (actual, expected) in [actual.red(), actual.green(), actual.blue()].into_iter().zip([expected.red(), expected.green(), expected.blue()]) {
            assert!(actual.abs_diff(expected) <= 3, "adjusted animated frame differs from CPU reference");
        }
        Ok(())
    }

    #[test]
    fn image_adjustments_match_cpu_reference_on_gpu() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut image = RgbaImage::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let color = match (x >= 4, y >= 4) {
                    (false, false) => [180, 90, 40, 255],
                    (true, false) => [45, 170, 210, 255],
                    (false, true) => [225, 205, 70, 255],
                    (true, true) => [110, 35, 175, 128],
                };
                image.put_pixel(x, y, image::Rgba(color));
            }
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)?;
        let mut doc = Document::new("GPU image adjustments");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Image", 0.0, 0.0, 8.0, 8.0)?;
        let hash = "adjusted-image-test".to_string();
        doc.images.insert(hash.clone(), ImageData { bytes: Arc::new(bytes), width: 8, height: 8 });
        doc.node_mut(id).ok_or("image node missing")?.fills = vec![Paint::image(hash, ImageScaleMode::Fill)];
        let area = Rect::new(0.0, 0.0, 8.0, 8.0);
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 8, height: 8 };
        let all_controls =
            ImageFilters { exposure: 0.3, contrast: -0.25, saturation: 0.4, temperature: -0.3, tint: 0.2, highlights: -0.4, shadows: 0.25 };
        for (name, filters) in ImageFilters::PRESETS.iter().map(|(name, _, filters)| (*name, *filters)).chain([("all", all_controls)]) {
            if let Some(Paint { kind: PaintKind::Image(image), .. }) = doc.node_mut(id).and_then(|node| node.fills.first_mut()) {
                image.filters = filters;
            }
            assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{name} rejected");
            let gpu = render_area_gpu(&doc, &[id], area, 1.0).ok_or("adjusted GPU image unavailable")?;
            let cpu = figcraft_render::render_area(&doc, &[id], area, 1.0).ok_or("adjusted CPU image unavailable")?;
            for (x, y) in [(1, 1), (6, 1), (1, 6), (6, 6)] {
                let actual = gpu.pixel(x, y).ok_or("GPU image sample missing")?;
                let expected = cpu.pixel(x, y).ok_or("CPU image sample missing")?;
                for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                    expected.red(),
                    expected.green(),
                    expected.blue(),
                    expected.alpha(),
                ]) {
                    assert!(a.abs_diff(b) <= 3, "{name} at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn diamond_gradient_matches_cpu_reference_on_gpu() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("GPU diamond gradient");
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Diamond", 0.0, 0.0, 128.0, 128.0)?;
        let mut gradient = Gradient::default_for(GradientKind::Diamond, Color::new(0.9, 0.1, 0.2, 1.0));
        gradient.stops = vec![
            ColorStop { position: 0.0, color: Color::new(0.9, 0.1, 0.2, 1.0) },
            ColorStop { position: 0.5, color: Color::new(0.2, 0.8, 0.3, 0.6) },
            ColorStop { position: 1.0, color: Color::new(0.1, 0.2, 0.9, 1.0) },
        ];
        let mut paint = Paint::gradient(gradient);
        paint.opacity = 0.75;
        doc.node_mut(id).ok_or("diamond node missing")?.fills = vec![paint];
        let area = Rect::new(0.0, 0.0, 128.0, 128.0);
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 128, height: 128 };
        assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        let gpu = render_area_gpu(&doc, &[id], area, 1.0).ok_or("diamond GPU image unavailable")?;
        let cpu = figcraft_render::render_area(&doc, &[id], area, 1.0).ok_or("diamond CPU image unavailable")?;
        for (x, y) in [(64, 64), (64, 32), (96, 64), (82, 82), (3, 64), (64, 3)] {
            let actual = gpu.pixel(x, y).ok_or("GPU diamond sample missing")?;
            let expected = cpu.pixel(x, y).ok_or("CPU diamond sample missing")?;
            for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                expected.red(),
                expected.green(),
                expected.blue(),
                expected.alpha(),
            ]) {
                assert!(a.abs_diff(b) <= 6, "at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn diamond_gradient_in_vector_region_matches_cpu() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("GPU diamond region");
        let page = doc.current_page.ok_or("page missing")?;
        let network = figcraft_doc::VectorNetwork::from_points(
            &[
                figcraft_geom::Point::new(0.0, 0.0),
                figcraft_geom::Point::new(64.0, 0.0),
                figcraft_geom::Point::new(64.0, 64.0),
                figcraft_geom::Point::new(0.0, 64.0),
            ],
            true,
        );
        let id = doc.create(page, NodeKind::Vector { network }, "Diamond region", 8.0, 8.0, 64.0, 64.0)?;
        let gradient = Gradient::default_for(GradientKind::Diamond, Color::new(0.8, 0.1, 0.1, 1.0));
        let node = doc.node_mut(id).ok_or("vector missing")?;
        node.fills.clear();
        if let NodeKind::Vector { network } = &mut node.kind {
            network.regions.first_mut().ok_or("vector has no region")?.fills = Some(vec![Paint::gradient(gradient)]);
        }
        let area = Rect::new(0.0, 0.0, 80.0, 80.0);
        let gpu = render_area_gpu(&doc, &[id], area, 1.0).ok_or("region GPU render unavailable")?;
        let cpu = figcraft_render::render_area(&doc, &[id], area, 1.0).ok_or("region CPU render unavailable")?;
        for (x, y) in [(40, 40), (40, 20), (55, 40), (20, 55)] {
            let actual = gpu.pixel(x, y).ok_or("GPU region sample missing")?;
            let expected = cpu.pixel(x, y).ok_or("CPU region sample missing")?;
            for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                expected.red(),
                expected.green(),
                expected.blue(),
                expected.alpha(),
            ]) {
                assert!(a.abs_diff(b) <= 6, "at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn display_p3_page_and_selection_keep_document_rgb_values() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("GPU Display P3");
        doc.color_profile = ColorProfile::DisplayP3;
        let page = doc.current_page.ok_or("page missing")?;
        let id = doc.create(page, NodeKind::Rectangle, "Managed image", 8.0, 8.0, 16.0, 16.0)?;
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255])))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)?;
        let hash = "display-p3-image".to_string();
        doc.images.insert(hash.clone(), ImageData { bytes: Arc::new(bytes), width: 4, height: 4 });
        doc.node_mut(id).ok_or("P3 image node missing")?.fills = vec![Paint::image(hash, ImageScaleMode::Fill)];
        let gradient_id = doc.create(page, NodeKind::Rectangle, "P3 gradient", 28.0, 8.0, 16.0, 16.0)?;
        doc.node_mut(gradient_id).ok_or("P3 gradient node missing")?.fills =
            vec![Paint::gradient(Gradient::default_for(GradientKind::Linear, Color::new(0.1, 0.8, 0.3, 1.0)))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 64, height: 32 };
        let gpu = render_page_gpu(&doc, page, &view).ok_or("P3 GPU page unavailable")?;
        let cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("P3 CPU page unavailable")?;
        for (x, y) in [(2, 2), (16, 16), (36, 16)] {
            let actual = gpu.pixel(x, y).ok_or("P3 GPU sample missing")?;
            let expected = cpu.pixel(x, y).ok_or("P3 CPU sample missing")?;
            for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                expected.red(),
                expected.green(),
                expected.blue(),
                expected.alpha(),
            ]) {
                assert!(a.abs_diff(b) <= 3, "P3 page ({x},{y}): GPU={actual:?}, CPU={expected:?}");
            }
        }
        let gpu_screen = render_page_gpu_screen(&doc, page, &view).ok_or("P3 GPU screen page unavailable")?;
        let mut cpu_screen = cpu;
        figcraft_render::to_srgb(&mut cpu_screen, ColorProfile::DisplayP3);
        for (x, y) in [(2, 2), (16, 16), (36, 16)] {
            let actual = gpu_screen.pixel(x, y).ok_or("P3 GPU screen pixel missing")?;
            let expected = cpu_screen.pixel(x, y).ok_or("P3 CPU screen pixel missing")?;
            for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                expected.red(),
                expected.green(),
                expected.blue(),
                expected.alpha(),
            ]) {
                assert!(a.abs_diff(b) <= 4, "P3 screen page ({x},{y}): GPU={actual:?}, CPU={expected:?}");
            }
        }
        let area = Rect::new(8.0, 8.0, 24.0, 24.0);
        let gpu = render_area_gpu(&doc, &[id], area, 1.0).ok_or("P3 GPU selection unavailable")?;
        let cpu = figcraft_render::render_area(&doc, &[id], area, 1.0).ok_or("P3 CPU selection unavailable")?;
        let actual = gpu.pixel(8, 8).ok_or("P3 GPU selection pixel missing")?;
        let expected = cpu.pixel(8, 8).ok_or("P3 CPU selection pixel missing")?;
        for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
            expected.red(),
            expected.green(),
            expected.blue(),
            expected.alpha(),
        ]) {
            assert!(a.abs_diff(b) <= 3, "P3 selection: GPU={actual:?}, CPU={expected:?}");
        }
        let gpu_screen = render_area_gpu_screen(&doc, &[id], area, 1.0).ok_or("P3 GPU screen selection unavailable")?;
        let mut cpu_screen = cpu;
        figcraft_render::to_srgb(&mut cpu_screen, ColorProfile::DisplayP3);
        let actual = gpu_screen.pixel(8, 8).ok_or("P3 GPU screen selection pixel missing")?;
        let expected = cpu_screen.pixel(8, 8).ok_or("P3 CPU screen selection pixel missing")?;
        for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
            expected.red(),
            expected.green(),
            expected.blue(),
            expected.alpha(),
        ]) {
            assert!(a.abs_diff(b) <= 4, "P3 screen selection: GPU={actual:?}, CPU={expected:?}");
        }
        Ok(())
    }

    #[test]
    fn display_p3_presentation_converts_to_srgb_on_gpu() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(mut canvas) = GpuCanvas::new(32, 32) else { return Ok(()) };
        let mut doc = Document::new("GPU Display P3 presentation");
        doc.color_profile = ColorProfile::DisplayP3;
        let page = doc.current_page.ok_or("page missing")?;
        if let Some(NodeKind::Page { background_visible, .. }) = doc.node_mut(page).map(|node| &mut node.kind) {
            *background_visible = false;
        }
        let id = doc.create(page, NodeKind::Rectangle, "P3 color", 0.0, 0.0, 32.0, 32.0)?;
        doc.node_mut(id).ok_or("P3 color node missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.25, 0.1, 0.6))];
        for size in [32, 48] {
            if size != 32 {
                canvas.resize(size, size);
            }
            let view = View { origin: (0.0, 0.0), scale: 1.0, width: size, height: size };
            canvas.render_submit_document_rgba(&doc, page, &view, [0.0; 4])?;
            canvas.convert_for_presentation(ColorProfile::DisplayP3);
            let actual = canvas.presented_pixels()?;
            let mut expected = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("P3 CPU presentation unavailable")?;
            figcraft_render::to_srgb(&mut expected, ColorProfile::DisplayP3);
            for (x, y) in [(2usize, 2usize), (16, 16), (30, 30)] {
                let offset = (y * size as usize + x) * 4;
                for (gpu, cpu) in actual[offset..offset + 4].iter().zip(&expected.data()[offset..offset + 4]) {
                    assert!(
                        gpu.abs_diff(*cpu) <= 4,
                        "P3 presentation {size}x{size} ({x},{y}): GPU={:?}, CPU={:?}",
                        &actual[offset..offset + 4],
                        &expected.data()[offset..offset + 4]
                    );
                }
            }
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
    fn group_blend_modes_match_cpu_samples() -> Result<(), Box<dyn std::error::Error>> {
        if GpuCanvas::new(1, 1).is_err() {
            return Ok(());
        }
        let mut doc = Document::new("group blend export");
        let page = doc.current_page.ok_or("page missing")?;
        let base = doc.create(page, NodeKind::Rectangle, "Base", 0.0, 0.0, 80.0, 80.0)?;
        doc.node_mut(base).ok_or("base missing")?.fills = vec![Paint::solid(Color::new(0.3, 0.5, 0.8, 1.0))];
        let group = doc.create(page, NodeKind::Group, "Group", 10.0, 10.0, 60.0, 60.0)?;
        let first = doc.create(group, NodeKind::Rectangle, "First", 0.0, 0.0, 40.0, 40.0)?;
        doc.node_mut(first).ok_or("first missing")?.fills = vec![Paint::solid(Color::new(0.8, 0.4, 0.2, 1.0))];
        let second = doc.create(group, NodeKind::Rectangle, "Second", 20.0, 20.0, 40.0, 40.0)?;
        doc.node_mut(second).ok_or("second missing")?.fills = vec![Paint::solid(Color::new(0.2, 0.8, 0.4, 0.7))];
        let view = View { origin: (0.0, 0.0), scale: 1.0, width: 80, height: 80 };
        for opacity in [1.0, 0.65] {
            for blend in figcraft_color::BlendMode::ALL {
                if matches!(blend, figcraft_color::BlendMode::PassThrough | figcraft_color::BlendMode::Normal | figcraft_color::BlendMode::PlusDarker)
                {
                    continue;
                }
                let node = doc.node_mut(group).ok_or("group missing")?;
                node.blend = blend;
                node.opacity = opacity;
                assert!(!has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true), "{blend:?} rejected");
                let gpu = render_page_gpu(&doc, page, &view).ok_or("GPU group blend export unavailable")?;
                let cpu = figcraft_render::render_view(&doc, page, &view, &Default::default()).ok_or("CPU group blend reference unavailable")?;
                for (x, y) in [(20, 20), (40, 40), (60, 60)] {
                    let actual = gpu.pixel(x, y).ok_or("GPU sample missing")?;
                    let expected = cpu.pixel(x, y).ok_or("CPU sample missing")?;
                    for (a, b) in [actual.red(), actual.green(), actual.blue()].into_iter().zip([expected.red(), expected.green(), expected.blue()]) {
                        assert!(a.abs_diff(b) <= 10, "{blend:?} opacity={opacity} at ({x},{y}): GPU={actual:?}, CPU={expected:?}");
                    }
                }
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
