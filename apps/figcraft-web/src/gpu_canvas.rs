//! Browser WebGPU canvas. The WebGL2 runner keeps the ordinary CPU fallback.

use std::error::Error;
use std::sync::Arc;

use eframe::egui_wgpu::{RenderState, wgpu};
use figcraft_canvas_gpu::vello_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_with_cached_noise_in_view};
use figcraft_color::{Color, ColorProfile};
use figcraft_doc::{Document, NodeId};
use figcraft_render::View;
use figcraft_ui_egui::canvas::CanvasGpuBackend;

pub struct WebGpuCanvas {
    state: RenderState,
    canvas: Option<GpuCanvas>,
    texture: Option<egui::TextureId>,
    size: (u32, u32),
    profile: Option<ColorProfile>,
    document: Option<Arc<Document>>,
    page: Option<NodeId>,
    camera: Option<(f64, f64, f64)>,
    disabled: bool,
}

impl WebGpuCanvas {
    pub fn new(state: RenderState) -> Self {
        Self { state, canvas: None, texture: None, size: (0, 0), profile: None, document: None, page: None, camera: None, disabled: false }
    }

    fn register_view(&mut self, profile: ColorProfile) -> Result<(), Box<dyn Error>> {
        if let Some(id) = self.texture.take() {
            self.state.renderer.write().free_texture(&id);
        }
        let canvas = self.canvas.as_mut().ok_or("WebGPU canvas missing")?;
        let view = if profile == ColorProfile::DisplayP3 { canvas.srgb_output_view() } else { canvas.output_view() };
        self.texture = Some(self.state.renderer.write().register_native_texture(&self.state.device, view, wgpu::FilterMode::Linear));
        self.profile = Some(profile);
        Ok(())
    }

    fn update(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        if view.width == 0
            || view.height == 0
            || view.width.max(view.height) > self.state.device.limits().max_texture_dimension_2d
            || view.width > u16::MAX.into()
            || view.height > u16::MAX.into()
            || !view.scale.is_finite()
            || view.scale <= 0.0
            || has_unhandled_effects_with_cached_noise_in_view(&doc, view, true)
        {
            return Ok(None);
        }
        let size = (view.width, view.height);
        let profile = doc.color_profile;
        let clear = if profile == ColorProfile::DisplayP3 {
            let converted = ColorProfile::Srgb.conversion_to(profile).apply(Color::new(clear[0], clear[1], clear[2], 1.0));
            [converted.r, converted.g, converted.b]
        } else {
            clear
        };
        if self.canvas.is_none() {
            self.canvas =
                Some(GpuCanvas::from_device(self.state.device.clone(), self.state.queue.clone(), "browser WebGPU".to_owned(), size.0, size.1));
            self.size = size;
            self.register_view(profile)?;
        } else if self.size != size {
            self.canvas.as_mut().ok_or("WebGPU canvas missing during resize")?.resize(size.0, size.1);
            self.size = size;
            self.camera = None;
            self.register_view(profile)?;
        }
        if self.profile != Some(profile) {
            self.register_view(profile)?;
        }
        let changed_doc = !self.document.as_ref().is_some_and(|old| Arc::ptr_eq(old, &doc));
        let camera = (view.origin.0, view.origin.1, view.scale);
        if !changed_doc && self.page == Some(page) && self.camera == Some(camera) {
            return Ok(self.texture);
        }
        let canvas = self.canvas.as_mut().ok_or("WebGPU canvas missing before render")?;
        if changed_doc || self.camera.is_none_or(|previous| previous.2 != view.scale) {
            let reused = changed_doc
                && self.document.as_deref().is_some_and(|old| {
                    canvas.reuse_translated_effects(old, &doc)
                        || gesture_active && canvas.reuse_duplicated_effects(old, &doc)
                        || canvas.reuse_unaffected_effects(old, &doc)
                        || gesture_active && canvas.reuse_rotated_effects(old, &doc)
                        || gesture_active && canvas.reuse_scaled_effects(old, &doc)
                        || gesture_active && canvas.reuse_resized_effects(old, &doc)
                });
            if !reused || !doc.shaders.is_empty() {
                canvas.register_images_at_scale(&doc, 0.0, view.scale)?;
            }
            if changed_doc && !reused {
                canvas.prepare_static_effects(&doc)?;
            }
        }
        let backdrops = BackdropIndex::new(&doc, page);
        if backdrops.visible_ids(view).is_empty() {
            let scene = canvas.make_scene_document(&doc, page, view);
            canvas.render_submit_with_clear(&scene, clear)?;
        } else {
            if changed_doc {
                canvas.register_images_at_scale(&doc, 0.0, view.scale)?;
                canvas.prepare_static_effects(&doc)?;
            }
            canvas.render_with_backdrop_blurs_indexed(&doc, page, view, clear, &backdrops)?;
        }
        canvas.convert_for_presentation(profile);
        self.document = Some(doc);
        self.page = Some(page);
        self.camera = Some(camera);
        Ok(self.texture)
    }
}

impl CanvasGpuBackend for WebGpuCanvas {
    fn texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled {
            return None;
        }
        match self.update(doc, page, view, clear, gesture_active) {
            Ok(texture) => texture,
            Err(error) => {
                log::error!("browser WebGPU canvas failed: {error}");
                self.disabled = true;
                None
            }
        }
    }
}

impl Drop for WebGpuCanvas {
    fn drop(&mut self) {
        if let Some(id) = self.texture.take() {
            self.state.renderer.write().free_texture(&id);
        }
    }
}
