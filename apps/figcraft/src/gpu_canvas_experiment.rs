//! Native GPU canvas. Unsupported content falls back to the ordinary CPU canvas.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui_wgpu::{RenderState, wgpu};
use figcraft_canvas_gpu::vello_scene::{
    BackdropIndex, GpuCanvas, GpuPixelSample, focused_document, has_unhandled_effects_except_outlined_on_page_in_view,
    has_unhandled_effects_in_focused_subtree, has_unhandled_effects_on_page_in_view, make_focused_outline_scene, make_outline_scene,
    make_scene_with_images,
};
use figcraft_color::{Color, ColorProfile};
use figcraft_doc::{Document, NodeId};
use figcraft_geom::Point;
use figcraft_render::View;
use figcraft_ui_egui::canvas::{CanvasGpuBackend, GpuSample};

pub struct GpuCanvasExperiment {
    state: RenderState,
    direct: Option<GpuCanvas>,
    direct_texture_id: Option<egui::TextureId>,
    direct_profile: Option<ColorProfile>,
    direct_pixel_preview: bool,
    direct_proof: bool,
    direct_outline: Option<figcraft_render::Outline>,
    direct_focus: Option<NodeId>,
    direct_selected_outlines: Option<(Vec<NodeId>, figcraft_render::Outline)>,
    direct_size: (u32, u32),
    direct_document: Option<Arc<Document>>,
    direct_page: Option<NodeId>,
    direct_camera: Option<(f64, f64, f64)>,
    direct_current: bool,
    eyedrop_pending: Option<(Point, GpuPixelSample)>,
    direct_provisional_effects: bool,
    direct_force: bool,
    direct_cached_blur: bool,
    direct_fallback_logged: bool,
    disabled: bool,
}

impl GpuCanvasExperiment {
    pub fn new(state: RenderState) -> Self {
        Self {
            state,
            direct: None,
            direct_texture_id: None,
            direct_profile: None,
            direct_pixel_preview: false,
            direct_proof: false,
            direct_outline: None,
            direct_focus: None,
            direct_selected_outlines: None,
            direct_size: (0, 0),
            direct_document: None,
            direct_page: None,
            direct_camera: None,
            direct_current: false,
            eyedrop_pending: None,
            direct_provisional_effects: false,
            direct_force: std::env::var_os("LIZARD_GPU_CANVAS_DIRECT_FORCE").is_some(),
            direct_cached_blur: std::env::var_os("LIZARD_GPU_CANVAS_NO_EFFECT_CACHE").is_none(),
            direct_fallback_logged: false,
            disabled: false,
        }
    }

    fn register_direct_view(&mut self, profile: ColorProfile, pixel_preview: bool) -> Result<(), Box<dyn Error>> {
        if let Some(id) = self.direct_texture_id.take() {
            self.state.renderer.write().free_texture(&id);
        }
        let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing for texture registration")?;
        let view = if profile == ColorProfile::DisplayP3 { canvas.srgb_output_view() } else { canvas.output_view() };
        let filter = if pixel_preview { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
        let id = self.state.renderer.write().register_native_texture(&self.state.device, view, filter);
        self.direct_texture_id = Some(id);
        self.direct_profile = Some(profile);
        self.direct_pixel_preview = pixel_preview;
        Ok(())
    }

    fn update(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
        proof_srgb: bool,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        if self.direct_force || !has_unhandled_effects_on_page_in_view(&doc, page, view, self.direct_cached_blur) {
            return self.update_direct(doc, page, view, clear, gesture_active, proof_srgb, false, None, None);
        }
        if !self.direct_fallback_logged {
            eprintln!("Lizard direct Vello GPU: view contains unsupported effects; using the CPU canvas");
            self.direct_fallback_logged = true;
        }
        Ok(None)
    }

    fn update_direct(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
        proof_srgb: bool,
        pixel_preview: bool,
        focus: Option<NodeId>,
        selected_outlines: Option<(&[NodeId], figcraft_render::Outline)>,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        if self.direct_outline.take().is_some() {
            self.direct_document = None;
        }
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
        let profile = doc.color_profile;
        let clear = if profile == ColorProfile::DisplayP3 {
            let converted = ColorProfile::Srgb.conversion_to(profile).apply(Color::new(clear[0], clear[1], clear[2], 1.0));
            [converted.r, converted.g, converted.b]
        } else {
            clear
        };
        if self.direct.is_none() {
            let canvas =
                GpuCanvas::from_device(self.state.device.clone(), self.state.queue.clone(), "eframe shared device".to_owned(), size.0, size.1);
            self.direct = Some(canvas);
            self.register_direct_view(profile, pixel_preview)?;
            self.direct_size = size;
            self.direct_document = None;
            self.direct_camera = None;
            self.direct_provisional_effects = false;
            eprintln!("Lizard direct Vello GPU canvas: viewport={}x{}", size.0, size.1);
        } else if self.direct_size != size {
            let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing during resize")?;
            canvas.resize(size.0, size.1);
            self.register_direct_view(profile, pixel_preview)?;
            self.direct_size = size;
            self.direct_camera = None;
            eprintln!("Lizard direct Vello GPU canvas: resized viewport={}x{}; retained effect textures", size.0, size.1);
        }
        if self.direct_profile != Some(profile) || self.direct_pixel_preview != pixel_preview {
            self.register_direct_view(profile, pixel_preview)?;
        }
        let changed_doc = !self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, &doc));
        let changed_focus = self.direct_focus != focus;
        let changed_selection = self.direct_selected_outlines.as_ref().map(|(ids, options)| (ids.as_slice(), *options)) != selected_outlines;
        let camera = (view.origin.0, view.origin.1, view.scale);
        if !changed_doc
            && !changed_focus
            && !changed_selection
            && self.direct_page == Some(page)
            && self.direct_camera == Some(camera)
            && self.direct_proof != proof_srgb
            && let Some(canvas) = self.direct.as_mut()
        {
            canvas.convert_for_presentation_with_proof(profile, proof_srgb);
            self.direct_proof = proof_srgb;
            return Ok(self.direct_texture_id);
        }
        if !doc.shaders.is_empty() && (changed_doc || self.direct_camera.is_none_or(|previous| previous.2 != view.scale)) {
            let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing during shader zoom")?;
            canvas.register_images_at_scale(&doc, 0.0, view.scale)?;
        }
        let finish_preview = self.direct_provisional_effects && !gesture_active;
        let page_backdrops = BackdropIndex::new(&doc, page);
        let focus_has_backdrop =
            focus.is_some_and(|id| page_backdrops.visible_ids(view).iter().any(|candidate| *candidate == id || doc.is_ancestor(id, *candidate)));
        let focused = focus.filter(|_| focus_has_backdrop).and_then(|id| focused_document(&doc, page, id));
        let render_doc = focused.as_ref().unwrap_or(&doc);
        let backdrops = if focused.is_some() { BackdropIndex::new(render_doc, page) } else { page_backdrops };
        if (focus.is_none() || focus_has_backdrop) && !backdrops.visible_ids(view).is_empty() {
            if changed_doc {
                let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing")?;
                canvas.register_images_at_scale(&doc, 0.0, view.scale)?;
                if self.direct_cached_blur {
                    canvas.prepare_static_effects(&doc)?;
                }
            }
            if changed_doc || changed_focus || changed_selection || self.direct_page != Some(page) || self.direct_camera != Some(camera) {
                let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing")?;
                if let Some((selected, options)) = selected_outlines {
                    canvas.render_with_backdrop_blurs_indexed_outlined(render_doc, page, view, clear, &backdrops, selected, options)?;
                } else {
                    canvas.render_with_backdrop_blurs_indexed(render_doc, page, view, clear, &backdrops)?;
                }
                canvas.convert_for_presentation_with_proof(profile, proof_srgb);
                self.direct_document = Some(doc);
                self.direct_page = Some(page);
                self.direct_camera = Some(camera);
                self.direct_proof = proof_srgb;
                self.direct_focus = focus;
                self.direct_selected_outlines = selected_outlines.map(|(ids, options)| (ids.to_vec(), options));
                self.direct_provisional_effects = false;
            }
            return Ok(self.direct_texture_id);
        }
        if changed_doc || changed_focus || changed_selection || self.direct_page != Some(page) || self.direct_camera != Some(camera) || finish_preview
        {
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
                    canvas.register_images_at_scale(&doc, 0.0, view.scale)?;
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
            let scene = if let Some((selected, options)) = selected_outlines {
                canvas.make_scene_selected_outlines(&doc, page, view, selected, options)
            } else if let Some(focus) = focus {
                canvas.make_scene_roots(&doc, &[focus], view)
            } else if self.direct_cached_blur {
                canvas.make_scene_document(&doc, page, view)
            } else {
                make_scene_with_images(&doc, page, view, Some(canvas.image_info()))
            };
            let ms = canvas.render_submit_with_clear(&scene, clear)?;
            canvas.convert_for_presentation_with_proof(profile, proof_srgb);
            if ms > 16.0 {
                eprintln!("Lizard direct Vello GPU frame: {ms:.1} ms at {:.2}x", view.scale);
            }
            self.direct_document = Some(doc);
            self.direct_page = Some(page);
            self.direct_camera = Some(camera);
            self.direct_proof = proof_srgb;
            self.direct_focus = focus;
            self.direct_selected_outlines = selected_outlines.map(|(ids, options)| (ids.to_vec(), options));
        }
        Ok(self.direct_texture_id)
    }

    fn update_outline(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        options: figcraft_render::Outline,
        focus: Option<NodeId>,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        if view.width == 0
            || view.height == 0
            || view.width > u16::MAX.into()
            || view.height > u16::MAX.into()
            || view.width.max(view.height) > self.state.device.limits().max_texture_dimension_2d
            || !view.scale.is_finite()
            || view.scale <= 0.0
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
        if self.direct.is_none() {
            self.direct =
                Some(GpuCanvas::from_device(self.state.device.clone(), self.state.queue.clone(), "eframe shared device".to_owned(), size.0, size.1));
            self.register_direct_view(profile, false)?;
            self.direct_size = size;
            self.direct_document = None;
        } else if self.direct_size != size {
            self.direct.as_mut().ok_or("GPU outline canvas missing on resize")?.resize(size.0, size.1);
            self.register_direct_view(profile, false)?;
            self.direct_size = size;
            self.direct_document = None;
        }
        if self.direct_profile != Some(profile) || self.direct_pixel_preview {
            self.register_direct_view(profile, false)?;
        }
        let camera = (view.origin.0, view.origin.1, view.scale);
        if self.direct_outline == Some(options)
            && self.direct_focus == focus
            && self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, &doc))
            && self.direct_page == Some(page)
            && self.direct_camera == Some(camera)
        {
            return Ok(self.direct_texture_id);
        }
        let scene = if let Some(focus) = focus {
            make_focused_outline_scene(&doc, focus, view, options)
        } else {
            make_outline_scene(&doc, page, view, options)
        };
        let canvas = self.direct.as_mut().ok_or("GPU outline canvas missing before render")?;
        canvas.render_submit_with_clear(&scene, clear)?;
        canvas.convert_for_presentation(profile);
        self.direct_document = Some(doc);
        self.direct_page = Some(page);
        self.direct_camera = Some(camera);
        self.direct_outline = Some(options);
        self.direct_focus = focus;
        self.direct_selected_outlines = None;
        Ok(self.direct_texture_id)
    }
}

impl CanvasGpuBackend for GpuCanvasExperiment {
    fn selected_outline_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        selected: &[NodeId],
        view: &View,
        clear: [f32; 3],
        options: figcraft_render::Outline,
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled || has_unhandled_effects_except_outlined_on_page_in_view(&doc, page, view, self.direct_cached_blur, selected) {
            return None;
        }
        match self.update_direct(doc, page, view, clear, false, proof_srgb, false, None, Some((selected, options))) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU selected outline view failed: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn selected_outline_pixel_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        selected: &[NodeId],
        view: &View,
        clear: [f32; 3],
        options: figcraft_render::Outline,
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled || has_unhandled_effects_except_outlined_on_page_in_view(&doc, page, view, self.direct_cached_blur, selected) {
            return None;
        }
        match self.update_direct(doc, page, view, clear, false, proof_srgb, true, None, Some((selected, options))) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU selected-outline pixel preview failed: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn focus_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        focus: NodeId,
        view: &View,
        clear: [f32; 3],
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled || doc.page_of(focus) != Some(page) || has_unhandled_effects_in_focused_subtree(&doc, view, self.direct_cached_blur, focus) {
            return None;
        }
        match self.update_direct(doc, page, view, clear, false, proof_srgb, false, Some(focus), None) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU focus view failed: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn focused_pixel_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        focus: NodeId,
        view: &View,
        clear: [f32; 3],
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled || doc.page_of(focus) != Some(page) || has_unhandled_effects_in_focused_subtree(&doc, view, self.direct_cached_blur, focus) {
            return None;
        }
        match self.update_direct(doc, page, view, clear, false, proof_srgb, true, Some(focus), None) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU focused pixel preview failed: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn pixel_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled || (!self.direct_force && has_unhandled_effects_on_page_in_view(&doc, page, view, self.direct_cached_blur)) {
            return None;
        }
        match self.update_direct(doc, page, view, clear, false, proof_srgb, true, None, None) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU pixel preview failed: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn outline_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        options: figcraft_render::Outline,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled {
            return None;
        }
        match self.update_outline(doc, page, view, clear, options, None) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU outline failed: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn focused_outline_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        focus: NodeId,
        view: &View,
        clear: [f32; 3],
        options: figcraft_render::Outline,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled {
            return None;
        }
        match self.update_outline(doc, page, view, clear, options, Some(focus)) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU focused outline failed: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled {
            return None;
        }
        match self.update(doc, page, view, clear, gesture_active, proof_srgb) {
            Ok(texture) => {
                self.direct_current = texture.is_some() && texture == self.direct_texture_id;
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU canvas disabled after an error: {error}");
                self.disabled = true;
                self.direct_current = false;
                None
            }
        }
    }

    fn sample_eyedropper(&mut self, point: Point, ctx: &egui::Context) -> GpuSample {
        if !self.direct_current {
            self.eyedrop_pending = None;
            return GpuSample::Unavailable;
        }
        let Some(canvas) = self.direct.as_ref() else { return GpuSample::Unavailable };
        if let Some((at, pending)) = self.eyedrop_pending.as_mut() {
            if *at == point {
                match pending.try_pixels(&self.state.device) {
                    Ok(Some(pixels)) => {
                        self.eyedrop_pending = None;
                        return GpuSample::Ready(pixels);
                    }
                    Ok(None) => {
                        ctx.request_repaint_after(Duration::from_millis(16));
                        return GpuSample::Pending;
                    }
                    Err(error) => {
                        eprintln!("Lizard GPU eyedropper readback failed: {error}");
                        self.eyedrop_pending = None;
                        return GpuSample::Unavailable;
                    }
                }
            }
            self.eyedrop_pending = None;
        }
        let Some((origin_x, origin_y, scale)) = self.direct_camera else { return GpuSample::Unavailable };
        let x = ((point.x - origin_x) * scale).floor() as i32;
        let y = ((point.y - origin_y) * scale).floor() as i32;
        match canvas.sample_9x9(x, y, self.direct_profile == Some(ColorProfile::DisplayP3)) {
            Ok(Some(pending)) => {
                self.eyedrop_pending = Some((point, pending));
                ctx.request_repaint_after(Duration::from_millis(16));
                GpuSample::Pending
            }
            Ok(None) => GpuSample::Unavailable,
            Err(error) => {
                eprintln!("Lizard GPU eyedropper copy failed: {error}");
                GpuSample::Unavailable
            }
        }
    }
}

impl Drop for GpuCanvasExperiment {
    fn drop(&mut self) {
        if let Some(id) = self.direct_texture_id.take() {
            self.state.renderer.write().free_texture(&id);
        }
    }
}
