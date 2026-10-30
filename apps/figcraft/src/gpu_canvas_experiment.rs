//! Native GPU canvas. Unsupported content falls back to the ordinary CPU canvas; the legacy
//! geometry atlas remains available for measurements through `LIZARD_GPU_CANVAS_ATLAS=1`.

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui_wgpu::{RenderState, wgpu};
use figcraft_canvas_gpu::geometry_atlas::{GeometryAtlas, required_slots};
use figcraft_canvas_gpu::vello_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_with_cached_noise_in_view, make_scene_with_images};
use figcraft_doc::{Document, NodeId};
use figcraft_render::View;
use figcraft_ui_egui::canvas::CanvasGpuBackend;

pub struct GpuCanvasExperiment {
    state: RenderState,
    atlas: Option<GeometryAtlas>,
    direct: Option<GpuCanvas>,
    direct_texture_id: Option<egui::TextureId>,
    direct_size: (u32, u32),
    direct_document: Option<Arc<Document>>,
    direct_page: Option<NodeId>,
    direct_camera: Option<(f64, f64, f64)>,
    direct_provisional_effects: bool,
    direct_mode: bool,
    direct_force: bool,
    direct_cached_blur: bool,
    direct_fallback_logged: bool,
    texture_id: Option<egui::TextureId>,
    document: Option<Arc<Document>>,
    page: Option<NodeId>,
    size: (u32, u32),
    pending_resize: Option<((u32, u32), Instant)>,
    started: Instant,
    first_ready: bool,
    disabled: bool,
    last_camera: Option<(f64, f64, f64)>,
    last_camera_changed: Instant,
    camera_states: u64,
    covered_states: u64,
}

impl GpuCanvasExperiment {
    pub fn new(state: RenderState) -> Self {
        Self {
            state,
            atlas: None,
            direct: None,
            direct_texture_id: None,
            direct_size: (0, 0),
            direct_document: None,
            direct_page: None,
            direct_camera: None,
            direct_provisional_effects: false,
            direct_mode: std::env::var_os("LIZARD_GPU_CANVAS_ATLAS").is_none(),
            direct_force: std::env::var_os("LIZARD_GPU_CANVAS_DIRECT_FORCE").is_some(),
            direct_cached_blur: std::env::var_os("LIZARD_GPU_CANVAS_NO_EFFECT_CACHE").is_none(),
            direct_fallback_logged: false,
            texture_id: None,
            document: None,
            page: None,
            size: (0, 0),
            pending_resize: None,
            started: Instant::now(),
            first_ready: false,
            disabled: false,
            last_camera: None,
            last_camera_changed: Instant::now(),
            camera_states: 0,
            covered_states: 0,
        }
    }

    fn update(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
        ctx: &egui::Context,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        if self.direct_mode {
            let cached_effects = self.direct_cached_blur;
            if self.direct_force || !has_unhandled_effects_with_cached_noise_in_view(&doc, view, cached_effects) {
                return self.update_direct(doc, page, view, clear, gesture_active);
            }
        }
        if self.direct_mode {
            if !self.direct_fallback_logged {
                eprintln!("Lizard direct Vello GPU: view contains unsupported effects; using the CPU canvas");
                self.direct_fallback_logged = true;
            }
            return Ok(None);
        }
        if view.width == 0
            || view.height == 0
            || view.width.max(view.height) > self.state.device.limits().max_texture_dimension_2d
            || !view.scale.is_finite()
            || !(0.0..=614.4).contains(&view.scale)
        {
            return Ok(None);
        }
        let size = (view.width, view.height);
        if self.atlas.is_some() && self.size != size {
            let started = match self.pending_resize {
                Some((pending, started)) if pending == size => started,
                _ => {
                    let started = Instant::now();
                    self.pending_resize = Some((size, started));
                    started
                }
            };
            let remaining = Duration::from_millis(150).saturating_sub(started.elapsed());
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
                return Ok(None);
            }
        } else {
            self.pending_resize = None;
        }
        if self.atlas.is_none() || self.size != size {
            if let Some(id) = self.texture_id.take() {
                self.state.renderer.write().free_texture(&id);
            }
            let desired = if view.width >= 4000 { 240 } else { required_slots(view, 2).saturating_mul(2).clamp(64, 240) };
            let capacity = self.state.device.limits().max_texture_array_layers.min(desired as u32);
            eprintln!("Lizard GPU atlas allocation: viewport={}x{}, slots={capacity}", size.0, size.1);
            let mut atlas = GeometryAtlas::new(
                Arc::clone(&doc),
                page,
                self.state.device.clone(),
                self.state.queue.clone(),
                size.0,
                size.1,
                capacity,
                128,
                false,
            )?;
            atlas.upload_on_owner_thread();
            let texture_id = self.state.renderer.write().register_native_texture(&self.state.device, atlas.output_view(), wgpu::FilterMode::Linear);
            self.atlas = Some(atlas);
            self.texture_id = Some(texture_id);
            self.document = Some(Arc::clone(&doc));
            self.page = Some(page);
            self.size = size;
            self.pending_resize = None;
            self.started = Instant::now();
            self.first_ready = false;
            self.last_camera = None;
            self.last_camera_changed = Instant::now();
            self.camera_states = 0;
            self.covered_states = 0;
        }
        let atlas = self.atlas.as_mut().ok_or("GPU atlas missing after creation")?;
        if self.page != Some(page) || !self.document.as_ref().is_some_and(|current| Arc::ptr_eq(current, &doc)) {
            atlas.invalidate_document_auto(Arc::clone(&doc), page)?;
            self.document = Some(doc);
            self.page = Some(page);
            self.started = Instant::now();
            self.first_ready = false;
            self.last_camera = None;
            self.last_camera_changed = Instant::now();
            self.camera_states = 0;
            self.covered_states = 0;
        }
        atlas.request_margin(view, 1)?;
        atlas.install(view, 4)?;
        let covered = atlas.covers(view);
        let presentable = atlas.covers_presentable(view);
        let camera = (view.origin.0, view.origin.1, view.scale);
        if self.last_camera != Some(camera) {
            self.last_camera = Some(camera);
            self.last_camera_changed = Instant::now();
            self.camera_states += 1;
            self.covered_states += u64::from(covered);
            if self.camera_states.is_multiple_of(40) {
                eprintln!(
                    "Lizard GPU atlas camera coverage: {}/{} complete, resident={}, pending={}",
                    self.covered_states,
                    self.camera_states,
                    atlas.stats().1,
                    atlas.stats().2
                );
            }
        }
        let idle = self.last_camera_changed.elapsed();
        if view.scale <= 2.4 && idle >= Duration::from_millis(300) && idle < Duration::from_secs(4) && atlas.covers_margin(view, 1) {
            let center = (view.origin.0 + f64::from(view.width) / (2.0 * view.scale), view.origin.1 + f64::from(view.height) / (2.0 * view.scale));
            for scale in [0.81, 1.61, 3.21] {
                let projected = View {
                    origin: (center.0 - f64::from(view.width) / (2.0 * scale), center.1 - f64::from(view.height) / (2.0 * scale)),
                    scale,
                    width: view.width,
                    height: view.height,
                };
                atlas.prefetch_view_guarded(&projected);
            }
        }
        if !presentable {
            ctx.request_repaint_after(Duration::from_millis(16));
            return Ok(None);
        }
        atlas.render_submit(view, clear)?;
        if !self.first_ready {
            eprintln!(
                "Lizard GPU atlas first drawable view: {:.1} ms, desired_covered={covered}, {:?}",
                self.started.elapsed().as_secs_f64() * 1000.0,
                atlas.stats()
            );
            self.first_ready = true;
        }
        if atlas.stats().2 > 0 {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if !covered {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if view.scale <= 2.4 && idle < Duration::from_millis(300) {
            ctx.request_repaint_after(Duration::from_millis(300).saturating_sub(idle));
        }
        Ok(self.texture_id)
    }

    fn update_direct(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        if view.width == 0
            || view.height == 0
            || view.width > u16::MAX as u32
            || view.height > u16::MAX as u32
            || view.width.max(view.height) > self.state.device.limits().max_texture_dimension_2d
            || !view.scale.is_finite()
            || view.scale <= 0.0
        {
            return Ok(None);
        }
        let size = (view.width, view.height);
        if self.direct.is_none() {
            if let Some(id) = self.direct_texture_id.take() {
                self.state.renderer.write().free_texture(&id);
            }
            let canvas =
                GpuCanvas::from_device(self.state.device.clone(), self.state.queue.clone(), "eframe shared device".to_owned(), size.0, size.1);
            let id = self.state.renderer.write().register_native_texture(&self.state.device, canvas.output_view(), wgpu::FilterMode::Linear);
            self.direct = Some(canvas);
            self.direct_texture_id = Some(id);
            self.direct_size = size;
            self.direct_document = None;
            self.direct_camera = None;
            self.direct_provisional_effects = false;
            eprintln!("Lizard direct Vello GPU canvas: viewport={}x{}", size.0, size.1);
        } else if self.direct_size != size {
            if let Some(id) = self.direct_texture_id.take() {
                self.state.renderer.write().free_texture(&id);
            }
            let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing during resize")?;
            canvas.resize(size.0, size.1);
            let id = self.state.renderer.write().register_native_texture(&self.state.device, canvas.output_view(), wgpu::FilterMode::Linear);
            self.direct_texture_id = Some(id);
            self.direct_size = size;
            self.direct_camera = None;
            eprintln!("Lizard direct Vello GPU canvas: resized viewport={}x{}; retained effect textures", size.0, size.1);
        }
        let changed_doc = !self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, &doc));
        let camera = (view.origin.0, view.origin.1, view.scale);
        let finish_preview = self.direct_provisional_effects && !gesture_active;
        let backdrops = BackdropIndex::new(&doc, page);
        if !backdrops.visible_ids(view).is_empty() {
            if changed_doc {
                let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing")?;
                canvas.register_images(&doc)?;
                if self.direct_cached_blur {
                    canvas.prepare_static_effects(&doc)?;
                }
            }
            if changed_doc || self.direct_page != Some(page) || self.direct_camera != Some(camera) {
                self.direct.as_mut().ok_or("direct GPU canvas missing")?.render_with_backdrop_blurs_indexed(&doc, page, view, clear, &backdrops)?;
                self.direct_document = Some(doc);
                self.direct_page = Some(page);
                self.direct_camera = Some(camera);
                self.direct_provisional_effects = false;
            }
            return Ok(self.direct_texture_id);
        }
        if changed_doc || self.direct_page != Some(page) || self.direct_camera != Some(camera) || finish_preview {
            let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing after creation")?;
            if changed_doc {
                let translated = self.direct_cached_blur
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_translated_effects(previous, &doc));
                let duplicated = !translated
                    && gesture_active
                    && self.direct_cached_blur
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_duplicated_effects(previous, &doc));
                let unaffected = !translated
                    && !duplicated
                    && self.direct_cached_blur
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_unaffected_effects(previous, &doc));
                let rotated = !translated
                    && !duplicated
                    && !unaffected
                    && gesture_active
                    && self.direct_cached_blur
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_rotated_effects(previous, &doc));
                let scaled = !translated
                    && !duplicated
                    && !unaffected
                    && !rotated
                    && gesture_active
                    && self.direct_cached_blur
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_scaled_effects(previous, &doc));
                let resized = !translated
                    && !duplicated
                    && !unaffected
                    && !rotated
                    && !scaled
                    && gesture_active
                    && self.direct_cached_blur
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_resized_effects(previous, &doc));
                if duplicated || rotated || scaled || resized {
                    self.direct_provisional_effects = true;
                }
                if !translated && !duplicated && !unaffected && !rotated && !scaled && !resized {
                    canvas.register_images(&doc)?;
                }
                if self.direct_cached_blur && ((!translated && !duplicated && !unaffected && !rotated && !scaled && !resized) || finish_preview) {
                    let (blur, drop, inner) = canvas.prepare_static_effects(&doc)?;
                    self.direct_provisional_effects = false;
                    eprintln!(
                        "Lizard direct Vello GPU: prepared {blur} blur, {drop} drop shadow, {inner} inner shadow, {} noise, {} scatter texture(s)",
                        canvas.cached_noise_count(),
                        canvas.cached_texture_count()
                    );
                }
            } else if finish_preview && self.direct_cached_blur {
                canvas.prepare_static_effects(&doc)?;
                self.direct_provisional_effects = false;
            }
            let scene = if self.direct_cached_blur {
                canvas.make_scene_document(&doc, page, view)
            } else {
                make_scene_with_images(&doc, page, view, Some(canvas.image_info()))
            };
            let ms = canvas.render_submit_with_clear(&scene, clear)?;
            if ms > 16.0 {
                eprintln!("Lizard direct Vello GPU frame: {ms:.1} ms at {:.2}x", view.scale);
            }
            self.direct_document = Some(doc);
            self.direct_page = Some(page);
            self.direct_camera = Some(camera);
        }
        Ok(self.direct_texture_id)
    }
}

impl CanvasGpuBackend for GpuCanvasExperiment {
    fn texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
        ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled {
            return None;
        }
        match self.update(doc, page, view, clear, gesture_active, ctx) {
            Ok(texture) => texture,
            Err(error) => {
                eprintln!("Lizard GPU atlas disabled after an error: {error}");
                self.disabled = true;
                None
            }
        }
    }
}

impl Drop for GpuCanvasExperiment {
    fn drop(&mut self) {
        if let Some(id) = self.direct_texture_id.take() {
            self.state.renderer.write().free_texture(&id);
        }
        if let Some(id) = self.texture_id.take() {
            self.state.renderer.write().free_texture(&id);
        }
    }
}
