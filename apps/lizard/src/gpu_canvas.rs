//! Native GPU canvas. Unsupported content falls back to the ordinary CPU canvas.

use std::error::Error;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui_wgpu::{RenderState, wgpu};
use lizard_canvas_gpu::vello_scene::{
    BackdropIndex, GpuCanvas, GpuPixelSample, focused_document, has_unhandled_effects_except_outlined_in_focused_subtree,
    has_unhandled_effects_except_outlined_on_page_in_view, has_unhandled_effects_in_focused_subtree, has_unhandled_effects_on_page_in_view,
    make_focused_outline_scene, make_outline_scene,
};
use lizard_color::{Color, ColorProfile, Paint, PaintKind};
use lizard_doc::{Document, NodeId, NodeKind};
use lizard_geom::{Point, Rect};
use lizard_render::View;
use lizard_ui_egui::canvas::{CanvasGpuBackend, GpuSample};

const PAN_TILE_BUDGET_PIXELS: u64 = 96_000_000;
const PAN_MARGIN_PIXELS: u32 = 128;

fn animated_shader_affects_cached_effects(doc: &Document, page: NodeId) -> bool {
    let animated = |paint: &Paint| {
        paint.visible
            && paint.opacity > 0.0
            && matches!(&paint.kind, PaintKind::Shader(fill) if doc.shaders.get(&fill.shader_id).is_some_and(|def| def.features.animated))
    };
    let animated_nodes: Vec<_> = doc
        .nodes()
        .filter(|node| {
            node.visible
                && doc.is_ancestor(page, node.id)
                && (node.fills.iter().chain(&node.strokes).any(animated)
                    || matches!(&node.kind, NodeKind::Vector { network }
                        if network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().any(animated))
                    || matches!(&node.kind, NodeKind::Text(text)
                        if std::iter::once(&text.style).chain(text.runs.iter().map(|run| &run.style))
                            .filter_map(|style| style.fills.as_ref()).flatten().any(animated)))
        })
        .map(|node| node.id)
        .collect();
    !animated_nodes.is_empty()
        && doc.nodes().any(|node| {
            node.visible
                && node.effects.iter().any(|effect| effect.visible)
                && animated_nodes.iter().any(|id| node.id == *id || doc.is_ancestor(node.id, *id))
        })
}

pub struct NativeGpuCanvas {
    state: RenderState,
    direct: Option<GpuCanvas>,
    direct_texture_id: Option<egui::TextureId>,
    direct_profile: Option<ColorProfile>,
    direct_pixel_preview: bool,
    direct_proof: bool,
    direct_outline: Option<lizard_render::Outline>,
    direct_focus: Option<NodeId>,
    direct_selected_outlines: Option<(Vec<NodeId>, lizard_render::Outline)>,
    direct_size: (u32, u32),
    direct_document: Option<Arc<Document>>,
    direct_page: Option<NodeId>,
    direct_camera: Option<(f64, f64, f64)>,
    animation_time: Option<f64>,
    direct_animation_time: Option<u64>,
    direct_render_view: Option<View>,
    direct_complete_page: bool,
    pan_tiles: Vec<CachedPanTile>,
    presented_pan_tiles: Vec<(egui::TextureId, View)>,
    direct_current: bool,
    eyedrop_pending: Option<(Point, GpuPixelSample)>,
    direct_provisional_effects: bool,
    direct_fallback_logged: bool,
    profile: bool,
    last_render_ms: f64,
    profile_submission: u64,
    expansion: Option<PanExpansion>,
    deferred_expansion: Option<DeferredExpansion>,
    disabled: bool,
}

struct PanExpansion {
    document: Arc<Document>,
    page: NodeId,
    view: View,
    complete_page: bool,
    proof_srgb: bool,
    receiver: Receiver<Result<GpuCanvas, String>>,
}

struct DeferredExpansion {
    document: Arc<Document>,
    page: NodeId,
    view: View,
    complete_page: bool,
    clear: [f32; 3],
    proof_srgb: bool,
    camera: (f64, f64, f64),
    ready_at: Instant,
}

struct CachedPanTile {
    _texture: wgpu::Texture,
    id: egui::TextureId,
    view: View,
    document: Arc<Document>,
    page: NodeId,
    proof_srgb: bool,
    complete_page: bool,
}

impl NativeGpuCanvas {
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
            animation_time: None,
            direct_animation_time: None,
            direct_render_view: None,
            direct_complete_page: false,
            pan_tiles: Vec::new(),
            presented_pan_tiles: Vec::new(),
            direct_current: false,
            eyedrop_pending: None,
            direct_provisional_effects: false,
            direct_fallback_logged: false,
            profile: std::env::var_os("LIZARD_GPU_PROFILE").is_some(),
            last_render_ms: 0.0,
            profile_submission: 0,
            expansion: None,
            deferred_expansion: None,
            disabled: false,
        }
    }

    fn retain_pan_tiles(&mut self, doc: &Arc<Document>, page: NodeId, view: &View, proof_srgb: bool) {
        let mut retained = Vec::with_capacity(self.pan_tiles.len());
        for tile in self.pan_tiles.drain(..) {
            if Arc::ptr_eq(&tile.document, doc)
                && tile.page == page
                && tile.proof_srgb == proof_srgb
                && tile.view.scale.to_bits() == view.scale.to_bits()
            {
                retained.push(tile);
            } else {
                self.state.renderer.write().free_texture(&tile.id);
            }
        }
        self.pan_tiles = retained;
    }

    fn save_current_pan_tile(&mut self, doc: &Arc<Document>, page: NodeId, view_scale: f64, proof_srgb: bool) {
        let Some(view) = self.direct_render_view.as_ref() else { return };
        if doc.color_profile != ColorProfile::Srgb
            || self.direct_outline.is_some()
            || self.direct_focus.is_some()
            || self.direct_selected_outlines.is_some()
            || self.direct_provisional_effects
            || self.direct_page != Some(page)
            || self.direct_proof != proof_srgb
            || view.scale.to_bits() != view_scale.to_bits()
            || !self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, doc))
            || self.direct_camera != Some((view.origin.0, view.origin.1, view.scale))
            || self.pan_tiles.iter().any(|tile| tile.view.origin == view.origin && tile.view.width == view.width && tile.view.height == view.height)
        {
            return;
        }
        let Some(canvas) = self.direct.as_ref() else { return };
        let snapshot = canvas.snapshot_submit();
        let texture_view = snapshot.create_view(&wgpu::TextureViewDescriptor::default());
        let id = self.state.renderer.write().register_native_texture(&self.state.device, &texture_view, wgpu::FilterMode::Linear);
        self.pan_tiles.push(CachedPanTile {
            _texture: snapshot,
            id,
            view: view.clone(),
            document: doc.clone(),
            page,
            proof_srgb,
            complete_page: self.direct_complete_page,
        });
        let mut pixels = self.pan_tiles.iter().map(|tile| u64::from(tile.view.width) * u64::from(tile.view.height)).sum::<u64>();
        while pixels > PAN_TILE_BUDGET_PIXELS && self.pan_tiles.len() > 1 {
            let old = self.pan_tiles.remove(0);
            pixels -= u64::from(old.view.width) * u64::from(old.view.height);
            self.state.renderer.write().free_texture(&old.id);
        }
    }

    fn cached_pan_coverage(&self, doc: &Arc<Document>, page: NodeId, view: &View, proof_srgb: bool) -> Option<Vec<(egui::TextureId, View)>> {
        if doc.color_profile != ColorProfile::Srgb {
            return None;
        }
        let mut views = Vec::new();
        let mut rects = Vec::new();
        for tile in &self.pan_tiles {
            if !Arc::ptr_eq(&tile.document, doc)
                || tile.page != page
                || tile.proof_srgb != proof_srgb
                || tile.view.scale.to_bits() != view.scale.to_bits()
            {
                continue;
            }
            if tile.complete_page {
                return Some(vec![(tile.id, tile.view.clone())]);
            }
            if let Some(rect) = pan_tile_rect(&tile.view, view) {
                views.push((tile.id, tile.view.clone()));
                rects.push(rect);
            }
        }
        if self.direct_outline.is_none()
            && self.direct_focus.is_none()
            && self.direct_selected_outlines.is_none()
            && !self.direct_provisional_effects
            && self.direct_page == Some(page)
            && self.direct_proof == proof_srgb
            && self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, doc))
            && let (Some(source), Some(id)) = (self.direct_render_view.as_ref(), self.direct_texture_id)
            && source.scale.to_bits() == view.scale.to_bits()
            && self.direct_camera == Some((source.origin.0, source.origin.1, source.scale))
        {
            if self.direct_complete_page {
                return Some(vec![(id, source.clone())]);
            }
            if let Some(rect) = pan_tile_rect(source, view) {
                views.push((id, source.clone()));
                rects.push(rect);
            }
        }
        smallest_pan_cover(view, views, rects)
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

    fn poll_expansion(&mut self, doc: &Arc<Document>, page: NodeId, view: &View, proof_srgb: bool) -> Result<(), Box<dyn Error>> {
        let Some(expansion) = self.expansion.take() else { return Ok(()) };
        match expansion.receiver.try_recv() {
            Err(TryRecvError::Empty) => self.expansion = Some(expansion),
            Err(TryRecvError::Disconnected) => eprintln!("Lizard GPU pan cache expansion worker disconnected"),
            Ok(Err(error)) => eprintln!("Lizard GPU pan cache expansion failed: {error}"),
            Ok(Ok(canvas)) => {
                let valid = Arc::ptr_eq(&expansion.document, doc)
                    && expansion.page == page
                    && expansion.proof_srgb == proof_srgb
                    && expansion.view.scale.to_bits() == view.scale.to_bits()
                    && (expansion.complete_page || view_inside_source(&expansion.view, view, 16.0));
                if valid {
                    self.direct = Some(canvas);
                    self.direct_size = (expansion.view.width, expansion.view.height);
                    self.register_direct_view(doc.color_profile, false)?;
                    self.direct_document = Some(doc.clone());
                    self.direct_page = Some(page);
                    self.direct_camera = Some((expansion.view.origin.0, expansion.view.origin.1, expansion.view.scale));
                    self.direct_render_view = Some(expansion.view);
                    self.direct_complete_page = expansion.complete_page;
                    self.direct_proof = proof_srgb;
                    self.direct_outline = None;
                    self.direct_focus = None;
                    self.direct_selected_outlines = None;
                    self.direct_provisional_effects = false;
                    if self.profile {
                        eprintln!("gpu_pan_cache expansion_ready size={}x{}", self.direct_size.0, self.direct_size.1);
                    }
                }
            }
        }
        Ok(())
    }

    fn start_expansion(&mut self, doc: Arc<Document>, page: NodeId, view: View, complete_page: bool, clear: [f32; 3], proof_srgb: bool) {
        if self.expansion.as_ref().is_some_and(|pending| {
            Arc::ptr_eq(&pending.document, &doc)
                && pending.page == page
                && pending.view.scale.to_bits() == view.scale.to_bits()
                && pending.proof_srgb == proof_srgb
                && (pending.complete_page || view_inside_source(&pending.view, &view, 0.0))
        }) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let device = self.state.device.clone();
        let queue = self.state.queue.clone();
        let worker_doc = doc.clone();
        let worker_view = view.clone();
        let profile = doc.color_profile;
        let gpu_clear = if profile == ColorProfile::DisplayP3 {
            let converted = ColorProfile::Srgb.conversion_to(profile).apply(Color::new(clear[0], clear[1], clear[2], 1.0));
            [converted.r, converted.g, converted.b]
        } else {
            clear
        };
        std::thread::spawn(move || {
            let result = (|| -> Result<GpuCanvas, String> {
                let mut canvas =
                    GpuCanvas::from_device(device, queue, "eframe shared device pan cache".to_owned(), worker_view.width, worker_view.height);
                canvas.register_images_at_scale(&worker_doc, 0.0, worker_view.scale).map_err(|error| error.to_string())?;
                canvas.prepare_static_effects(&worker_doc).map_err(|error| error.to_string())?;
                let backdrops = BackdropIndex::new(&worker_doc, page);
                if backdrops.visible_ids(&worker_view).is_empty() {
                    let scene = canvas.make_scene_document(&worker_doc, page, &worker_view);
                    canvas.render_submit_with_clear(&scene, gpu_clear).map_err(|error| error.to_string())?;
                } else {
                    canvas
                        .render_with_backdrop_blurs_indexed(&worker_doc, page, &worker_view, gpu_clear, &backdrops)
                        .map_err(|error| error.to_string())?;
                }
                canvas.convert_for_presentation_with_proof(profile, proof_srgb);
                Ok(canvas)
            })();
            let _ = sender.send(result);
        });
        self.expansion = Some(PanExpansion { document: doc, page, view, complete_page, proof_srgb, receiver });
    }

    fn drive_deferred_expansion(
        &mut self,
        doc: &Arc<Document>,
        page: NodeId,
        view: &View,
        gesture_active: bool,
        proof_srgb: bool,
        ctx: &egui::Context,
    ) {
        let Some(mut pending) = self.deferred_expansion.take() else { return };
        if !Arc::ptr_eq(&pending.document, doc)
            || pending.page != page
            || pending.proof_srgb != proof_srgb
            || pending.view.scale.to_bits() != view.scale.to_bits()
        {
            return;
        }
        let camera = (view.origin.0, view.origin.1, view.scale);
        if pending.camera != camera || gesture_active {
            pending.camera = camera;
            pending.ready_at = Instant::now() + Duration::from_millis(250);
        }
        if Instant::now() >= pending.ready_at && !gesture_active && self.expansion.is_none() {
            self.start_expansion(pending.document, pending.page, pending.view, pending.complete_page, pending.clear, pending.proof_srgb);
            ctx.request_repaint_after(Duration::from_millis(16));
        } else {
            ctx.request_repaint_after(pending.ready_at.saturating_duration_since(Instant::now()));
            self.deferred_expansion = Some(pending);
        }
    }

    fn update(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &View,
        clear: [f32; 3],
        gesture_active: bool,
        proof_srgb: bool,
        ctx: &egui::Context,
    ) -> Result<Option<egui::TextureId>, Box<dyn Error>> {
        self.last_render_ms = 0.0;
        self.presented_pan_tiles.clear();
        if self.animation_time.is_some() {
            // A cached pan tile contains one instant of an animated fill. Render the current
            // viewport instead of presenting stale pixels or starting a static expansion.
            for tile in self.pan_tiles.drain(..) {
                self.state.renderer.write().free_texture(&tile.id);
            }
            self.expansion = None;
            self.deferred_expansion = None;
            let started = Instant::now();
            let texture = self.update_direct(doc, page, view, clear, gesture_active, proof_srgb, false, None, None)?;
            self.last_render_ms = started.elapsed().as_secs_f64() * 1000.0;
            self.direct_render_view = texture.map(|_| view.clone());
            self.direct_complete_page = false;
            return Ok(texture);
        }
        if self.direct_animation_time.is_some() {
            self.direct_camera = None;
            self.direct_render_view = None;
        }
        self.retain_pan_tiles(&doc, page, view, proof_srgb);
        self.poll_expansion(&doc, page, view, proof_srgb)?;
        self.drive_deferred_expansion(&doc, page, view, gesture_active, proof_srgb, ctx);
        if self.expansion.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if !has_unhandled_effects_on_page_in_view(&doc, page, view, true) {
            if (!self.direct_provisional_effects || gesture_active)
                && self.direct_outline.is_none()
                && self.direct_focus.is_none()
                && self.direct_selected_outlines.is_none()
                && self.direct_page == Some(page)
                && self.direct_proof == proof_srgb
                && self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, &doc))
                && let Some(source) = self.direct_render_view.as_ref()
                && source.scale.to_bits() == view.scale.to_bits()
                && self.direct_camera == Some((source.origin.0, source.origin.1, source.scale))
                && self.direct_texture_id.is_some()
            {
                let left = (view.origin.0 - source.origin.0) * view.scale;
                let top = (view.origin.1 - source.origin.1) * view.scale;
                if self.direct_complete_page || view_inside_source(source, view, 16.0) {
                    return Ok(self.direct_texture_id);
                }
                if self.profile {
                    eprintln!("gpu_pan_cache miss boundary left={left:.1} top={top:.1} source={}x{}", source.width, source.height);
                }
            } else if self.profile {
                eprintln!(
                    "gpu_pan_cache miss state doc={} page={} scale={} camera={} proof={} outline={} focus={} selected={} texture={} provisional={}",
                    self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, &doc)),
                    self.direct_page == Some(page),
                    self.direct_render_view.as_ref().is_some_and(|source| source.scale.to_bits() == view.scale.to_bits()),
                    self.direct_render_view
                        .as_ref()
                        .is_some_and(|source| self.direct_camera == Some((source.origin.0, source.origin.1, source.scale))),
                    self.direct_proof == proof_srgb,
                    self.direct_outline.is_some(),
                    self.direct_focus.is_some(),
                    self.direct_selected_outlines.is_some(),
                    self.direct_texture_id.is_some(),
                    self.direct_provisional_effects && !gesture_active,
                );
            }
            if (!self.direct_provisional_effects || gesture_active)
                && let Some(tiles) = self.cached_pan_coverage(&doc, page, view, proof_srgb)
            {
                if self.profile {
                    eprintln!("gpu_pan_cache tile_hit count={}", tiles.len());
                }
                self.presented_pan_tiles = tiles;
                return Ok(self.presented_pan_tiles.first().map(|(id, _)| *id));
            }
            self.save_current_pan_tile(&doc, page, view.scale, proof_srgb);
            let best = self
                .pan_tiles
                .iter()
                .filter_map(|tile| pan_tile_rect(&tile.view, view))
                .max_by(|a, b| ((a.2 - a.0) * (a.3 - a.1)).total_cmp(&((b.2 - b.0) * (b.3 - b.1))));
            let strips = best.and_then(|rect| pan_strip_views(view, rect));
            // An uncached effect elsewhere in the viewport does not affect a narrow
            // uncovered strip. Check each strip that will actually be rendered.
            let mut strip_safe = self
                .direct
                .as_ref()
                .is_some_and(|canvas| strips.as_ref().is_some_and(|strips| strips.iter().all(|strip| canvas.pan_strip_safe(&doc, page, strip))));
            if !strip_safe
                && self.direct_document.as_ref().is_some_and(|current| Arc::ptr_eq(current, &doc))
                && self.direct_page == Some(page)
                && self.direct_render_view.as_ref().is_some_and(|source| source.scale.to_bits() == view.scale.to_bits())
                && self.direct_outline.is_none()
                && self.direct_focus.is_none()
                && self.direct_selected_outlines.is_none()
                && !self.direct_pixel_preview
                && let Some(strips) = strips.as_ref()
                && let Some(canvas) = self.direct.as_mut()
            {
                let index = BackdropIndex::new(&doc, page);
                strip_safe = canvas.prepare_backdrops_for_pan_strips(&doc, page, view, strips, clear, &index)?;
            }
            if self.profile {
                eprintln!("gpu_pan_cache strip_safe={strip_safe} scale={:.2}", view.scale);
                if !strip_safe {
                    eprintln!(
                        "gpu_pan_cache strip_reject overlap={best:?} strip_count={} retained_tiles={}",
                        strips.as_ref().map_or(0, Vec::len),
                        self.pan_tiles.len()
                    );
                }
            }
            if strip_safe {
                if let Some(strips) = strips {
                    if self.profile {
                        let sizes: Vec<_> = strips.iter().map(|strip| (strip.width, strip.height)).collect();
                        eprintln!("gpu_pan_cache strips count={} sizes={sizes:?}", strips.len());
                    }
                    for (index, strip) in strips.iter().enumerate() {
                        let texture = self.update_direct(doc.clone(), page, strip, clear, gesture_active, proof_srgb, false, None, None)?;
                        self.direct_render_view = texture.map(|_| strip.clone());
                        self.direct_complete_page = false;
                        if texture.is_none() {
                            break;
                        }
                        if index + 1 < strips.len() {
                            self.save_current_pan_tile(&doc, page, view.scale, proof_srgb);
                        }
                    }
                    if let Some(tiles) = self.cached_pan_coverage(&doc, page, view, proof_srgb) {
                        self.presented_pan_tiles = tiles;
                        return Ok(self.presented_pan_tiles.first().map(|(id, _)| *id));
                    }
                    if self.profile {
                        eprintln!("gpu_pan_cache strip_fallback_reason=coverage");
                    }
                } else if self.profile {
                    eprintln!("gpu_pan_cache strip_fallback_reason=no_overlap_or_geometry");
                }
            }
            let limit = self.state.device.limits().max_texture_dimension_2d;
            let candidate = document_pan_view(&doc, page, view, limit);
            let visible_pixels = u64::from(view.width) * u64::from(view.height);
            let expand_in_background =
                candidate.as_ref().is_some_and(|(source, _)| u64::from(source.width) * u64::from(source.height) > visible_pixels.saturating_mul(2));
            let (buffered, complete_page) = candidate.as_ref().filter(|_| !expand_in_background).cloned().unwrap_or_else(|| {
                let margin = PAN_MARGIN_PIXELS.min(view.width.min(view.height) / 4).min(limit.saturating_sub(view.width.max(view.height)) / 2);
                (
                    View {
                        origin: (view.origin.0 - f64::from(margin) / view.scale, view.origin.1 - f64::from(margin) / view.scale),
                        scale: view.scale,
                        width: view.width.saturating_add(margin.saturating_mul(2)),
                        height: view.height.saturating_add(margin.saturating_mul(2)),
                    },
                    false,
                )
            });
            let stage = Instant::now();
            let texture = self.update_direct(doc.clone(), page, &buffered, clear, gesture_active, proof_srgb, false, None, None)?;
            self.last_render_ms = stage.elapsed().as_secs_f64() * 1000.0;
            if self.profile {
                eprintln!("gpu_pan_cache refill_ms={:.2} size={}x{}", self.last_render_ms, buffered.width, buffered.height);
                self.profile_submission = self.profile_submission.wrapping_add(1);
                let submission = self.profile_submission;
                let submitted_at = Instant::now();
                eprintln!("gpu_queue_watch submit={} scale={:.2} size={}x{}", submission, view.scale, buffered.width, buffered.height);
                self.state.queue.on_submitted_work_done(move || {
                    eprintln!("gpu_queue_watch done={} elapsed_ms={:.2}", submission, submitted_at.elapsed().as_secs_f64() * 1000.0);
                });
            }
            self.direct_render_view = texture.map(|_| buffered);
            self.direct_complete_page = texture.is_some() && complete_page;
            // A full-page worker with live backdrops can monopolize the same GPU queue after
            // the camera moves to another zoom. Native pan strips already reuse those effects.
            if texture.is_some()
                && expand_in_background
                && self.expansion.is_none()
                && BackdropIndex::new(&doc, page).is_empty()
                && let Some((source, complete_page)) = candidate
            {
                self.deferred_expansion = Some(DeferredExpansion {
                    document: doc,
                    page,
                    view: source,
                    complete_page,
                    clear,
                    proof_srgb,
                    camera: (view.origin.0, view.origin.1, view.scale),
                    ready_at: Instant::now() + Duration::from_millis(250),
                });
                ctx.request_repaint_after(Duration::from_millis(250));
            }
            return Ok(texture);
        }
        self.direct_render_view = None;
        self.direct_complete_page = false;
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
        selected_outlines: Option<(&[NodeId], lizard_render::Outline)>,
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
        let frame_time = self.animation_time.unwrap_or(0.0);
        let animation_changed = self.direct_animation_time != self.animation_time.map(f64::to_bits);
        if let Some(canvas) = self.direct.as_mut() {
            if animation_changed && self.animation_time.is_some() {
                canvas.set_backdrop_cache_document(None, view.scale);
            }
            let cache_document = (focus.is_none() && selected_outlines.is_none() && !pixel_preview).then(|| doc.clone());
            canvas.set_backdrop_cache_document(cache_document, view.scale);
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
            && !animation_changed
            && self.direct_proof != proof_srgb
            && let Some(canvas) = self.direct.as_mut()
        {
            canvas.convert_for_presentation_with_proof(profile, proof_srgb);
            self.direct_proof = proof_srgb;
            return Ok(self.direct_texture_id);
        }
        if !changed_doc && (animation_changed || self.direct_camera.is_none_or(|previous| previous.2 != view.scale)) {
            let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing during zoom")?;
            if canvas.has_scale_dependent_paints() {
                canvas.register_images_at_scale(&doc, frame_time, view.scale)?;
            }
        }
        let finish_preview = self.direct_provisional_effects && !gesture_active;
        let stage = Instant::now();
        let page_backdrops = BackdropIndex::new(&doc, page);
        if self.profile {
            eprintln!("gpu_stage index_ms={:.2}", stage.elapsed().as_secs_f64() * 1000.0);
        }
        let focus_has_backdrop =
            focus.is_some_and(|id| page_backdrops.visible_ids(view).iter().any(|candidate| *candidate == id || doc.is_ancestor(id, *candidate)));
        let focused = focus.filter(|_| focus_has_backdrop).and_then(|id| focused_document(&doc, page, id));
        let render_doc = focused.as_ref().unwrap_or(&doc);
        let backdrops = if focused.is_some() { BackdropIndex::new(render_doc, page) } else { page_backdrops };
        if (focus.is_none() || focus_has_backdrop) && !backdrops.visible_ids(view).is_empty() {
            let cache_started = Instant::now();
            if changed_doc {
                let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing")?;
                canvas.register_images_at_scale(&doc, frame_time, view.scale)?;
                canvas.prepare_static_effects(&doc)?;
            } else if animation_changed && animated_shader_affects_cached_effects(&doc, page) {
                self.direct.as_mut().ok_or("direct GPU canvas missing")?.prepare_static_effects(&doc)?;
            }
            if self.profile {
                eprintln!("gpu_stage cache_ms={:.2} changed_doc={changed_doc}", cache_started.elapsed().as_secs_f64() * 1000.0);
            }
            if changed_doc
                || animation_changed
                || changed_focus
                || changed_selection
                || self.direct_page != Some(page)
                || self.direct_camera != Some(camera)
            {
                let render_started = Instant::now();
                let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing")?;
                if let Some((selected, options)) = selected_outlines {
                    canvas.render_with_backdrop_blurs_indexed_outlined(render_doc, page, view, clear, &backdrops, selected, options)?;
                } else {
                    canvas.render_with_backdrop_blurs_indexed(render_doc, page, view, clear, &backdrops)?;
                }
                if self.profile {
                    eprintln!("gpu_stage backdrop_render_ms={:.2}", render_started.elapsed().as_secs_f64() * 1000.0);
                }
                let present_started = Instant::now();
                canvas.convert_for_presentation_with_proof(profile, proof_srgb);
                if self.profile {
                    eprintln!("gpu_stage conversion_submit_ms={:.2}", present_started.elapsed().as_secs_f64() * 1000.0);
                }
                self.direct_document = Some(doc);
                self.direct_page = Some(page);
                self.direct_camera = Some(camera);
                self.direct_animation_time = self.animation_time.map(f64::to_bits);
                self.direct_proof = proof_srgb;
                self.direct_focus = focus;
                self.direct_selected_outlines = selected_outlines.map(|(ids, options)| (ids.to_vec(), options));
                self.direct_provisional_effects = false;
            }
            return Ok(self.direct_texture_id);
        }
        if changed_doc
            || animation_changed
            || changed_focus
            || changed_selection
            || self.direct_page != Some(page)
            || self.direct_camera != Some(camera)
            || finish_preview
        {
            let cache_started = Instant::now();
            let canvas = self.direct.as_mut().ok_or("direct GPU canvas missing after creation")?;
            if changed_doc {
                // A shader value changes its texture key. Effect reuse may still classify the
                // rest of the document as unchanged, but the new key must be registered before
                // building the scene or the fill has no texture for this frame.
                canvas.register_images_at_scale(&doc, frame_time, view.scale)?;
                let translated = self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_translated_effects(previous, &doc));
                let duplicated = !translated
                    && gesture_active
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_duplicated_effects(previous, &doc));
                let unaffected = !translated
                    && !duplicated
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_unaffected_effects(previous, &doc));
                let rotated = !translated
                    && !duplicated
                    && !unaffected
                    && gesture_active
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_rotated_effects(previous, &doc));
                let scaled = !translated
                    && !duplicated
                    && !unaffected
                    && !rotated
                    && gesture_active
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_scaled_effects(previous, &doc));
                let resized = !translated
                    && !duplicated
                    && !unaffected
                    && !rotated
                    && !scaled
                    && gesture_active
                    && self.direct_document.as_deref().is_some_and(|previous| canvas.reuse_resized_effects(previous, &doc));
                if duplicated || rotated || scaled || resized {
                    self.direct_provisional_effects = true;
                }
                if (!translated && !duplicated && !unaffected && !rotated && !scaled && !resized) || finish_preview {
                    let (blur, drop, inner) = canvas.prepare_static_effects(&doc)?;
                    self.direct_provisional_effects = false;
                    eprintln!(
                        "Lizard direct Vello GPU: prepared {blur} blur, {drop} drop shadow, {inner} inner shadow, {} noise, {} scatter texture(s)",
                        canvas.cached_noise_count(),
                        canvas.cached_texture_count()
                    );
                }
            } else if finish_preview || (animation_changed && animated_shader_affects_cached_effects(&doc, page)) {
                canvas.prepare_static_effects(&doc)?;
                self.direct_provisional_effects = false;
            }
            if self.profile {
                eprintln!("gpu_stage cache_ms={:.2} changed_doc={changed_doc}", cache_started.elapsed().as_secs_f64() * 1000.0);
            }
            let scene_started = Instant::now();
            let scene = if let (Some(focus), Some((selected, options))) = (focus, selected_outlines) {
                canvas.make_scene_roots_outlined(&doc, &[focus], view, selected, options)
            } else if let Some((selected, options)) = selected_outlines {
                canvas.make_scene_selected_outlines(&doc, page, view, selected, options)
            } else if let Some(focus) = focus {
                canvas.make_scene_roots(&doc, &[focus], view)
            } else {
                canvas.make_scene_document(&doc, page, view)
            };
            if self.profile {
                eprintln!("gpu_stage scene_ms={:.2}", scene_started.elapsed().as_secs_f64() * 1000.0);
            }
            let ms = canvas.render_submit_with_clear(&scene, clear)?;
            let present_started = Instant::now();
            canvas.convert_for_presentation_with_proof(profile, proof_srgb);
            if self.profile {
                eprintln!("gpu_stage submit_ms={ms:.2} conversion_submit_ms={:.2}", present_started.elapsed().as_secs_f64() * 1000.0);
            }
            if ms > 16.0 {
                eprintln!("Lizard direct Vello GPU frame: {ms:.1} ms at {:.2}x", view.scale);
            }
            self.direct_document = Some(doc);
            self.direct_page = Some(page);
            self.direct_camera = Some(camera);
            self.direct_animation_time = self.animation_time.map(f64::to_bits);
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
        options: lizard_render::Outline,
        focus: Option<NodeId>,
        pixel_preview: bool,
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
            self.register_direct_view(profile, pixel_preview)?;
            self.direct_size = size;
            self.direct_document = None;
        } else if self.direct_size != size {
            self.direct.as_mut().ok_or("GPU outline canvas missing on resize")?.resize(size.0, size.1);
            self.register_direct_view(profile, pixel_preview)?;
            self.direct_size = size;
            self.direct_document = None;
        }
        if self.direct_profile != Some(profile) || self.direct_pixel_preview != pixel_preview {
            self.register_direct_view(profile, pixel_preview)?;
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

fn pan_tile_rect(source: &View, view: &View) -> Option<(f64, f64, f64, f64)> {
    if source.scale.to_bits() != view.scale.to_bits() {
        return None;
    }
    let left = (source.origin.0 - view.origin.0) * view.scale + 16.0;
    let top = (source.origin.1 - view.origin.1) * view.scale + 16.0;
    let right = left + f64::from(source.width) - 32.0;
    let bottom = top + f64::from(source.height) - 32.0;
    let rect = (left.max(0.0), top.max(0.0), right.min(f64::from(view.width)), bottom.min(f64::from(view.height)));
    (rect.0 < rect.2 && rect.1 < rect.3).then_some(rect)
}

fn pan_strip_views(view: &View, covered: (f64, f64, f64, f64)) -> Option<Vec<View>> {
    let width = i32::try_from(view.width).ok()?;
    let height = i32::try_from(view.height).ok()?;
    let (left, top, right, bottom) = (covered.0.ceil() as i32, covered.1.ceil() as i32, covered.2.floor() as i32, covered.3.floor() as i32);
    if left >= right || top >= bottom {
        return None;
    }
    let visible_area = i64::from(width) * i64::from(height);
    let uncovered_area = visible_area - i64::from(right - left) * i64::from(bottom - top);
    if uncovered_area <= 0 || uncovered_area * 10 > visible_area * 7 {
        return None;
    }
    let cores = [(0, 0, left, height), (right, 0, width, height), (left, 0, right, top), (left, bottom, right, height)];
    let mut strips = Vec::new();
    for (x0, y0, x1, y1) in cores {
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let pad = 32;
        let sx = x0 - pad;
        let sy = y0 - pad;
        let sw = x1 - x0 + pad * 2;
        let sh = y1 - y0 + pad * 2;
        strips.push(View {
            origin: (view.origin.0 + f64::from(sx) / view.scale, view.origin.1 + f64::from(sy) / view.scale),
            scale: view.scale,
            width: u32::try_from(sw).ok()?,
            height: u32::try_from(sh).ok()?,
        });
    }
    (!strips.is_empty()).then_some(strips)
}

fn pan_rects_cover(view: &View, rects: &[(f64, f64, f64, f64)]) -> bool {
    if rects.is_empty() {
        return false;
    }
    let width = f64::from(view.width);
    let height = f64::from(view.height);
    let mut x_edges = vec![0.0, width];
    for &(left, _, right, _) in rects {
        x_edges.extend([left, right]);
    }
    x_edges.sort_by(f64::total_cmp);
    x_edges.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    x_edges.windows(2).all(|span| {
        let mid = (span[0] + span[1]) * 0.5;
        let mut y_spans: Vec<_> = rects.iter().filter(|rect| rect.0 <= mid && rect.2 >= mid).map(|rect| (rect.1, rect.3)).collect();
        y_spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut covered = 0.0;
        for (top, bottom) in y_spans {
            if top > covered + 0.01 {
                return false;
            }
            covered = covered.max(bottom);
            if covered >= height - 0.01 {
                return true;
            }
        }
        false
    })
}

fn smallest_pan_cover(view: &View, tiles: Vec<(egui::TextureId, View)>, rects: Vec<(f64, f64, f64, f64)>) -> Option<Vec<(egui::TextureId, View)>> {
    if !pan_rects_cover(view, &rects) {
        return None;
    }
    // A pan can leave several overlapping viewport snapshots in the cache. Painting all
    // of them multiplies the textured area submitted to the compositor each frame.
    if rects.len() <= 12 {
        for count in 1..=rects.len() {
            for mask in 1_usize..(1_usize << rects.len()) {
                if mask.count_ones() as usize != count {
                    continue;
                }
                let selected: Vec<_> = rects.iter().enumerate().filter(|(index, _)| mask & (1 << index) != 0).map(|(_, rect)| *rect).collect();
                if pan_rects_cover(view, &selected) {
                    return Some(tiles.into_iter().enumerate().filter(|(index, _)| mask & (1 << index) != 0).map(|(_, tile)| tile).collect());
                }
            }
        }
    }
    let mut selected: Vec<usize> = (0..rects.len()).collect();
    for index in (0..rects.len()).rev() {
        let candidate: Vec<_> = selected.iter().copied().filter(|item| *item != index).map(|item| rects[item]).collect();
        if pan_rects_cover(view, &candidate) {
            selected.retain(|item| *item != index);
        }
    }
    Some(tiles.into_iter().enumerate().filter(|(index, _)| selected.contains(index)).map(|(_, tile)| tile).collect())
}

#[cfg(test)]
#[test]
fn adjacent_gpu_pan_tiles_cover_view_without_a_gap() {
    let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
    let left = View { origin: (-20.0, -20.0), width: 90, height: 140, ..view };
    let touching = View { origin: (38.0, -20.0), width: 100, height: 140, ..view };
    let gap = View { origin: (40.0, -20.0), ..touching };
    let left_rect = pan_tile_rect(&left, &view);
    let touching_rect = pan_tile_rect(&touching, &view);
    let gap_rect = pan_tile_rect(&gap, &view);
    assert!(left_rect.is_some() && touching_rect.is_some() && gap_rect.is_some());
    let (Some(left_rect), Some(touching_rect), Some(gap_rect)) = (left_rect, touching_rect, gap_rect) else { return };
    assert!(pan_rects_cover(&view, &[left_rect, touching_rect]));
    assert!(!pan_rects_cover(&view, &[left_rect, gap_rect]));
}

#[cfg(test)]
#[test]
fn overlapping_gpu_pan_tiles_choose_the_smallest_cover() {
    let view = View { origin: (0.0, 0.0), scale: 1.0, width: 100, height: 100 };
    let whole = (0.0, 0.0, 100.0, 100.0);
    let left = (0.0, 0.0, 60.0, 100.0);
    let right = (60.0, 0.0, 100.0, 100.0);
    let tiles = vec![(egui::TextureId::User(1), view.clone()), (egui::TextureId::User(2), view.clone()), (egui::TextureId::User(3), view.clone())];
    let result = smallest_pan_cover(&view, tiles, vec![left, right, whole]);
    assert!(result.is_some_and(|tiles| tiles.len() == 1 && tiles[0].0 == egui::TextureId::User(3)));
}

#[cfg(test)]
#[test]
fn pan_strips_cover_a_diagonal_pan_with_small_native_views() {
    let view = View { origin: (0.0, 0.0), scale: 2.0, width: 1000, height: 800 };
    let source = View { origin: (-100.0, -60.0), width: 1000, height: 800, ..view };
    let Some(covered) = pan_tile_rect(&source, &view) else { return };
    let Some(strips) = pan_strip_views(&view, covered) else { return };
    assert!(strips.len() <= 4);
    assert!(strips.iter().all(|strip| u64::from(strip.width) * u64::from(strip.height) < u64::from(view.width) * u64::from(view.height)));
    let rects = std::iter::once(covered).chain(strips.iter().filter_map(|strip| pan_tile_rect(strip, &view))).collect::<Vec<_>>();
    assert!(pan_rects_cover(&view, &rects));
}

#[cfg(test)]
#[test]
fn pan_strip_covers_a_larger_jump_without_a_full_viewport_refill() {
    let view = View { origin: (0.0, 0.0), scale: 1.0, width: 3963, height: 2020 };
    let covered = (2001.3, 0.0, 3963.0, 2020.0);
    let strips = pan_strip_views(&view, covered);
    assert!(strips.as_ref().is_some_and(|strips| strips.len() == 1));
    let Some(strips) = strips else { return };
    let rects = std::iter::once(covered).chain(strips.iter().filter_map(|strip| pan_tile_rect(strip, &view))).collect::<Vec<_>>();
    assert!(pan_rects_cover(&view, &rects));
}

fn view_inside_source(source: &View, view: &View, guard: f64) -> bool {
    if source.scale.to_bits() != view.scale.to_bits() {
        return false;
    }
    let left = (view.origin.0 - source.origin.0) * view.scale;
    let top = (view.origin.1 - source.origin.1) * view.scale;
    left >= guard
        && top >= guard
        && left + f64::from(view.width) + guard <= f64::from(source.width)
        && top + f64::from(view.height) + guard <= f64::from(source.height)
}

/// Hold completed page artwork in document coordinates while the camera pans. The page
/// candidate stays fixed even when the viewport extends outside the artwork; the UI paints
/// the page background around that texture. A frame candidate also covers the viewport.
fn document_pan_view(doc: &Document, page: NodeId, view: &View, limit: u32) -> Option<(View, bool)> {
    let visible = view.page_rect();
    let usable = |bounds, cover_view| {
        let source = bounded_pan_view(bounds, view, limit, cover_view)?;
        (!has_unhandled_effects_on_page_in_view(doc, page, &source, true)).then_some(source)
    };
    let roots = doc.node(page)?.children.as_slice();
    let whole = roots.iter().filter_map(|id| lizard_render::raster_bounds(doc, *id)).reduce(|a, b| a.union(b));
    if let Some(source) = whole.and_then(|bounds| usable(bounds, false)) {
        return Some((source, true));
    }
    doc.top_frames(page)
        .into_iter()
        .filter_map(|id| lizard_render::raster_bounds(doc, id))
        .filter(|bounds| bounds.intersect(visible).area() > 0.0)
        .max_by(|a, b| a.intersect(visible).area().total_cmp(&b.intersect(visible).area()))
        .and_then(|bounds| usable(bounds, true))
        .map(|source| (source, false))
}

fn bounded_pan_view(bounds: Rect, view: &View, limit: u32, cover_view: bool) -> Option<View> {
    if !view.scale.is_finite() || view.scale <= 0.0 {
        return None;
    }
    let bounds = if cover_view { bounds.union(view.page_rect()) } else { bounds };
    let padding = 256.0;
    let left = (bounds.x0 * view.scale).floor() - padding;
    let top = (bounds.y0 * view.scale).floor() - padding;
    let right = (bounds.x1 * view.scale).ceil() + padding;
    let bottom = (bounds.y1 * view.scale).ceil() + padding;
    let (width, height) = (right - left, bottom - top);
    if ![left, top, right, bottom, width, height].into_iter().all(f64::is_finite)
        || width <= 0.0
        || height <= 0.0
        || width > f64::from(limit.min(u16::MAX.into()))
        || height > f64::from(limit.min(u16::MAX.into()))
        || width * height > 24_000_000.0
    {
        return None;
    }
    Some(View { origin: (left / view.scale, top / view.scale), scale: view.scale, width: width as u32, height: height as u32 })
}

#[cfg(test)]
#[test]
fn document_pan_buffer_covers_artwork_at_native_scale_without_exceeding_budget() {
    let view = View { origin: (100.0, 150.0), scale: 2.0, width: 1200, height: 1600 };
    let frame = Rect::new(0.0, 0.0, 1200.0, 760.0);
    let source = bounded_pan_view(frame, &view, 16_384, true);
    assert!(source.is_some(), "source view should fit the budget");
    let Some(source) = source else { return };
    assert!(source.page_rect().intersect(view.page_rect()).area() >= view.page_rect().area());
    assert!(u64::from(source.width) * u64::from(source.height) <= 24_000_000);
    assert!(bounded_pan_view(frame, &View { scale: 20.0, ..view }, 16_384, true).is_none());
    let outside = View { origin: (5_000.0, 5_000.0), ..view };
    let fixed_page = bounded_pan_view(frame, &outside, 16_384, false);
    assert_eq!(
        fixed_page.as_ref().map(|source| (source.origin, source.width, source.height)),
        bounded_pan_view(frame, &view, 16_384, false).as_ref().map(|source| (source.origin, source.width, source.height))
    );
}

impl CanvasGpuBackend for NativeGpuCanvas {
    fn set_animation_time(&mut self, time: Option<f64>) {
        self.animation_time = time;
    }

    fn source_view(&self) -> Option<View> {
        self.direct_render_view.clone()
    }

    fn presented_tiles(&self) -> Vec<(egui::TextureId, View)> {
        self.presented_pan_tiles.clone()
    }

    fn last_render_ms(&self) -> Option<f64> {
        Some(self.last_render_ms)
    }

    fn selected_outline_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        selected: &[NodeId],
        view: &View,
        clear: [f32; 3],
        options: lizard_render::Outline,
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled || has_unhandled_effects_except_outlined_on_page_in_view(&doc, page, view, true, selected) {
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
        options: lizard_render::Outline,
        proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled || has_unhandled_effects_except_outlined_on_page_in_view(&doc, page, view, true, selected) {
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

    fn focused_selected_outline_texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        focus: NodeId,
        selected: &[NodeId],
        view: &View,
        clear: [f32; 3],
        options: lizard_render::Outline,
        proof_srgb: bool,
        pixel_preview: bool,
    ) -> Option<egui::TextureId> {
        if self.disabled
            || doc.page_of(focus) != Some(page)
            || has_unhandled_effects_except_outlined_in_focused_subtree(&doc, view, true, focus, selected)
        {
            return None;
        }
        match self.update_direct(doc, page, view, clear, false, proof_srgb, pixel_preview, Some(focus), Some((selected, options))) {
            Ok(texture) => {
                self.direct_current = texture.is_some();
                texture
            }
            Err(error) => {
                eprintln!("Lizard GPU focused selected-outline view failed: {error}");
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
        if self.disabled || doc.page_of(focus) != Some(page) || has_unhandled_effects_in_focused_subtree(&doc, view, true, focus) {
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
        if self.disabled || doc.page_of(focus) != Some(page) || has_unhandled_effects_in_focused_subtree(&doc, view, true, focus) {
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
        if self.disabled || has_unhandled_effects_on_page_in_view(&doc, page, view, true) {
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
        options: lizard_render::Outline,
        pixel_preview: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled {
            return None;
        }
        match self.update_outline(doc, page, view, clear, options, None, pixel_preview) {
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
        options: lizard_render::Outline,
        pixel_preview: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        if self.disabled {
            return None;
        }
        match self.update_outline(doc, page, view, clear, options, Some(focus), pixel_preview) {
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
        match self.update(doc, page, view, clear, gesture_active, proof_srgb, _ctx) {
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

impl Drop for NativeGpuCanvas {
    fn drop(&mut self) {
        if let Some(id) = self.direct_texture_id.take() {
            self.state.renderer.write().free_texture(&id);
        }
    }
}
