//! Browser WebGPU canvas. The WebGL2 runner keeps the ordinary CPU fallback.

use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;

use eframe::egui_wgpu::{RenderState, wgpu};
use figcraft_canvas_gpu::vello_scene::{
    BackdropIndex, GpuCanvas, GpuPixelSample, focused_document, has_unhandled_effects_except_outlined_in_view,
    has_unhandled_effects_in_focused_subtree, has_unhandled_effects_with_cached_noise_in_view, make_outline_scene,
};
use figcraft_color::{Color, ColorProfile};
use figcraft_doc::{Document, NodeId};
use figcraft_geom::Point;
use figcraft_render::View;
use figcraft_ui_egui::canvas::{CanvasGpuBackend, GpuSample};

pub struct WebGpuCanvas {
    state: RenderState,
    canvas: Option<GpuCanvas>,
    texture: Option<egui::TextureId>,
    size: (u32, u32),
    profile: Option<ColorProfile>,
    pixel_preview: bool,
    proof_srgb: bool,
    outline: Option<figcraft_render::Outline>,
    focus: Option<NodeId>,
    selected_outlines: Option<(Vec<NodeId>, figcraft_render::Outline)>,
    document: Option<Arc<Document>>,
    page: Option<NodeId>,
    camera: Option<(f64, f64, f64)>,
    current_gpu_rendered: bool,
    eyedrop_pending: Option<(Point, GpuPixelSample)>,
    disabled: bool,
}

impl WebGpuCanvas {
    pub fn new(state: RenderState) -> Self {
        Self {
            state,
            canvas: None,
            texture: None,
            size: (0, 0),
            profile: None,
            pixel_preview: false,
            proof_srgb: false,
            outline: None,
            focus: None,
            selected_outlines: None,
            document: None,
            page: None,
            camera: None,
            current_gpu_rendered: false,
            eyedrop_pending: None,
            disabled: false,
        }
    }

    fn register_view(&mut self, profile: ColorProfile, pixel_preview: bool) -> Result<(), Box<dyn Error>> {
        if let Some(id) = self.texture.take() {
            self.state.renderer.write().free_texture(&id);
        }
        let canvas = self.canvas.as_mut().ok_or("WebGPU canvas missing")?;
        let view = if profile == ColorProfile::DisplayP3 { canvas.srgb_output_view() } else { canvas.output_view() };
        let filter = if pixel_preview { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
        self.texture = Some(self.state.renderer.write().register_native_texture(&self.state.device, view, filter));
        self.profile = Some(profile);
        self.pixel_preview = pixel_preview;
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
        pixel_preview: bool,
        focus: Option<NodeId>,
        selected_outlines: Option<(&[NodeId], figcraft_render::Outline)>,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        if self.outline.take().is_some() {
            self.document = None;
        }
        if focus.is_some_and(|id| doc.page_of(id) != Some(page)) {
            return Ok(None);
        }
        let page_backdrops = BackdropIndex::new(&doc, page);
        let focus_has_backdrop =
            focus.is_some_and(|id| page_backdrops.visible_ids(view).iter().any(|candidate| *candidate == id || doc.is_ancestor(id, *candidate)));
        let focused = focus.filter(|_| focus_has_backdrop).and_then(|id| focused_document(&doc, page, id));
        let render_doc = focused.as_ref().unwrap_or(&doc);
        if view.width == 0
            || view.height == 0
            || view.width.max(view.height) > self.state.device.limits().max_texture_dimension_2d
            || view.width > u16::MAX.into()
            || view.height > u16::MAX.into()
            || !view.scale.is_finite()
            || view.scale <= 0.0
            || focus.map_or_else(
                || has_unhandled_effects_with_cached_noise_in_view(render_doc, view, true),
                |id| has_unhandled_effects_in_focused_subtree(&doc, view, true, id),
            )
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
            self.register_view(profile, pixel_preview)?;
        } else if self.size != size {
            self.canvas.as_mut().ok_or("WebGPU canvas missing during resize")?.resize(size.0, size.1);
            self.size = size;
            self.camera = None;
            self.register_view(profile, pixel_preview)?;
        }
        if self.profile != Some(profile) || self.pixel_preview != pixel_preview {
            self.register_view(profile, pixel_preview)?;
        }
        let changed_doc = !self.document.as_ref().is_some_and(|old| Arc::ptr_eq(old, &doc));
        let changed_focus = self.focus != focus;
        let changed_selection = self.selected_outlines.as_ref().map(|(ids, options)| (ids.as_slice(), *options)) != selected_outlines;
        let camera = (view.origin.0, view.origin.1, view.scale);
        if !changed_doc
            && !changed_focus
            && !changed_selection
            && self.page == Some(page)
            && self.camera == Some(camera)
            && self.proof_srgb == proof_srgb
        {
            return Ok(self.texture);
        }
        let canvas = self.canvas.as_mut().ok_or("WebGPU canvas missing before render")?;
        if !changed_doc && !changed_focus && !changed_selection && self.page == Some(page) && self.camera == Some(camera) {
            canvas.convert_for_presentation_with_proof(profile, proof_srgb);
            self.proof_srgb = proof_srgb;
            return Ok(self.texture);
        }
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
        let backdrops = if focused.is_some() { BackdropIndex::new(render_doc, page) } else { page_backdrops };
        if selected_outlines.is_some() || focus.is_some() && !focus_has_backdrop || backdrops.visible_ids(view).is_empty() {
            let scene = if let Some((selected, options)) = selected_outlines {
                canvas.make_scene_selected_outlines(&doc, page, view, selected, options)
            } else if let Some(focus) = focus {
                canvas.make_scene_roots(&doc, &[focus], view)
            } else {
                canvas.make_scene_document(&doc, page, view)
            };
            canvas.render_submit_with_clear(&scene, clear)?;
        } else {
            if changed_doc {
                canvas.register_images_at_scale(&doc, 0.0, view.scale)?;
                canvas.prepare_static_effects(&doc)?;
            }
            canvas.render_with_backdrop_blurs_indexed(render_doc, page, view, clear, &backdrops)?;
        }
        canvas.convert_for_presentation_with_proof(profile, proof_srgb);
        self.document = Some(doc);
        self.page = Some(page);
        self.camera = Some(camera);
        self.proof_srgb = proof_srgb;
        self.focus = focus;
        self.selected_outlines = selected_outlines.map(|(ids, options)| (ids.to_vec(), options));
        Ok(self.texture)
    }

    fn update_outline(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        options: figcraft_render::Outline,
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
        if self.canvas.is_none() {
            self.canvas =
                Some(GpuCanvas::from_device(self.state.device.clone(), self.state.queue.clone(), "browser WebGPU".to_owned(), size.0, size.1));
            self.size = size;
            self.register_view(profile, false)?;
        } else if self.size != size {
            self.canvas.as_mut().ok_or("WebGPU outline canvas missing during resize")?.resize(size.0, size.1);
            self.size = size;
            self.document = None;
            self.register_view(profile, false)?;
        }
        if self.profile != Some(profile) || self.pixel_preview {
            self.register_view(profile, false)?;
        }
        let camera = (view.origin.0, view.origin.1, view.scale);
        if self.outline == Some(options)
            && self.document.as_ref().is_some_and(|old| Arc::ptr_eq(old, &doc))
            && self.page == Some(page)
            && self.camera == Some(camera)
        {
            return Ok(self.texture);
        }
        let scene = make_outline_scene(&doc, page, view, options);
        let canvas = self.canvas.as_mut().ok_or("WebGPU outline canvas missing before render")?;
        canvas.render_submit_with_clear(&scene, clear)?;
        canvas.convert_for_presentation(profile);
        self.document = Some(doc);
        self.page = Some(page);
        self.camera = Some(camera);
        self.outline = Some(options);
        self.focus = None;
        self.selected_outlines = None;
        Ok(self.texture)
    }
}

impl CanvasGpuBackend for WebGpuCanvas {
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
        let outlined_ids: HashSet<_> = selected.iter().flat_map(|id| doc.subtree(*id)).collect();
        if self.disabled
            || has_unhandled_effects_except_outlined_in_view(&doc, view, true, selected)
            || BackdropIndex::new(&doc, page).visible_ids(view).iter().any(|id| !outlined_ids.contains(id))
        {
            return None;
        }
        match self.update(doc, page, view, clear, false, proof_srgb, false, None, Some((selected, options))) {
            Ok(texture) => {
                self.current_gpu_rendered = texture.is_some();
                texture
            }
            Err(error) => {
                log::error!("browser WebGPU selected outline view failed: {error}");
                self.disabled = true;
                self.current_gpu_rendered = false;
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
        if self.disabled {
            return None;
        }
        match self.update(doc, page, view, clear, false, proof_srgb, false, Some(focus), None) {
            Ok(texture) => {
                self.current_gpu_rendered = texture.is_some();
                texture
            }
            Err(error) => {
                log::error!("browser WebGPU focus view failed: {error}");
                self.disabled = true;
                self.current_gpu_rendered = false;
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
        if self.disabled {
            return None;
        }
        match self.update(doc, page, view, clear, false, proof_srgb, true, None, None) {
            Ok(texture) => {
                self.current_gpu_rendered = texture.is_some();
                texture
            }
            Err(error) => {
                log::error!("browser WebGPU pixel preview failed: {error}");
                self.disabled = true;
                self.current_gpu_rendered = false;
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
        match self.update_outline(doc, page, view, clear, options) {
            Ok(texture) => {
                self.current_gpu_rendered = texture.is_some();
                texture
            }
            Err(error) => {
                log::error!("browser WebGPU outline failed: {error}");
                self.disabled = true;
                self.current_gpu_rendered = false;
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
        match self.update(doc, page, view, clear, gesture_active, proof_srgb, false, None, None) {
            Ok(texture) => {
                self.current_gpu_rendered = texture.is_some();
                texture
            }
            Err(error) => {
                log::error!("browser WebGPU canvas failed: {error}");
                self.disabled = true;
                self.current_gpu_rendered = false;
                None
            }
        }
    }

    fn sample_eyedropper(&mut self, point: Point, ctx: &egui::Context) -> GpuSample {
        if !self.current_gpu_rendered {
            self.eyedrop_pending = None;
            return GpuSample::Unavailable;
        }
        let Some(canvas) = self.canvas.as_ref() else { return GpuSample::Unavailable };
        if let Some((at, pending)) = self.eyedrop_pending.as_mut() {
            if *at == point {
                match pending.try_pixels(&self.state.device) {
                    Ok(Some(pixels)) => {
                        self.eyedrop_pending = None;
                        return GpuSample::Ready(pixels);
                    }
                    Ok(None) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(16));
                        return GpuSample::Pending;
                    }
                    Err(error) => {
                        log::warn!("browser WebGPU eyedropper readback failed: {error}");
                        self.eyedrop_pending = None;
                        return GpuSample::Unavailable;
                    }
                }
            }
            self.eyedrop_pending = None;
        }
        let Some((origin_x, origin_y, scale)) = self.camera else { return GpuSample::Unavailable };
        let x = ((point.x - origin_x) * scale).floor() as i32;
        let y = ((point.y - origin_y) * scale).floor() as i32;
        match canvas.sample_9x9(x, y, self.profile == Some(ColorProfile::DisplayP3)) {
            Ok(Some(pending)) => {
                self.eyedrop_pending = Some((point, pending));
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
                GpuSample::Pending
            }
            Ok(None) => GpuSample::Unavailable,
            Err(error) => {
                log::warn!("browser WebGPU eyedropper copy failed: {error}");
                GpuSample::Unavailable
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
