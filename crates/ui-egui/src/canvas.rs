//! The canvas: the rendered page plus selection, hover, guides, labels and text-editing overlays.

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::Receiver;

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use lizard_color::{BlendMode, PaintKind};
use lizard_doc::{Document, NodeId, NodeKind};
use lizard_engine::Preedit;
use lizard_engine::tools::{PointerEvent, PointerKind, ToolId, selection_bounds, selection_box, selection_frame};
use lizard_geom::Point;
use serde_json::json;

use crate::LizardApp;
use crate::i18n::{tr, trf, trn};
use crate::render_worker::{Done, Job, RenderStyle, TextureUpdate, Worker};
use crate::theme::{COMPONENT, Tokens, doc_c32, regular, semibold};

#[derive(Clone)]
struct LiveBrushTexture {
    revision: u64,
    zoom: f64,
    pixels_per_point: f32,
    bounds: lizard_geom::Rect,
    texture: egui::TextureHandle,
}

fn paint_live_brush(app: &LizardApp, painter: &egui::Painter, revision: u64, zoom: f64) {
    use lizard_geom::kurbo::Shape as _;
    use tiny_skia::{FillRule, Paint as SkPaint, Pixmap, Transform};

    let Some(style) = app.session.tool.draw_style(ToolId::Brush) else { return };
    let color = doc_c32(painter.ctx(), style.color());
    let pixels_per_point = painter.ctx().pixels_per_point();
    let cache_id = egui::Id::new("lizard-live-brush-texture");
    let cached = painter.ctx().data_mut(|data| data.get_temp::<LiveBrushTexture>(cache_id));
    let current = cached.as_ref().is_some_and(|entry| entry.revision == revision && entry.zoom == zoom && entry.pixels_per_point == pixels_per_point);
    let entry = if current {
        cached
    } else {
        let Some(mut outline) = app.session.live_brush_outline() else { return };
        let bounds = outline.bounding_box();
        if !bounds.width().is_finite() || !bounds.height().is_finite() || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        let intended_scale = zoom.max(0.001) * f64::from(pixels_per_point);
        let scale = intended_scale.min(1536.0 / bounds.width()).min(1536.0 / bounds.height()).max(0.001);
        let bounds = bounds.inflate(2.0 / scale, 2.0 / scale);
        let width = (bounds.width() * scale).ceil() as u32;
        let height = (bounds.height() * scale).ceil() as u32;
        let Some(mut pixmap) = Pixmap::new(width, height) else { return };
        outline.apply_affine(lizard_geom::Affine::scale(scale) * lizard_geom::Affine::translate((-bounds.x0, -bounds.y0)));
        let Some(path) = lizard_render::ts_path(&outline) else { return };
        let mut paint = SkPaint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &lizard_render::to_rgba8(&pixmap));
        let texture = if let Some(mut entry) = cached {
            entry.texture.set(image, egui::TextureOptions::LINEAR);
            entry.texture
        } else {
            painter.ctx().load_texture("lizard-live-brush", image, egui::TextureOptions::LINEAR)
        };
        let entry = LiveBrushTexture { revision, zoom, pixels_per_point, bounds, texture };
        painter.ctx().data_mut(|data| data.insert_temp(cache_id, entry.clone()));
        Some(entry)
    };
    if let Some(entry) = entry {
        let rect = Rect::from_min_max(app.to_screen(entry.bounds.x0, entry.bounds.y0), app.to_screen(entry.bounds.x1, entry.bounds.y1));
        painter.image(entry.texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    }
}

/// The GPU canvas. The ordinary render worker remains the fallback for unsupported scenes.
pub trait CanvasGpuBackend {
    /// `Some` advances visible animated fills; `None` uses their static first frame.
    fn set_animation_time(&mut self, _time: Option<f64>) {}

    fn texture(
        &mut self,
        doc: Arc<Document>,
        page: NodeId,
        view: &lizard_render::View,
        clear: [f32; 3],
        gesture_active: bool,
        proof_srgb: bool,
        ctx: &egui::Context,
    ) -> Option<egui::TextureId>;

    /// The document area held by a GPU texture with pan overscan.
    fn source_view(&self) -> Option<lizard_render::View> {
        None
    }

    /// Finished GPU regions to compose in document coordinates when a pan spans cached views.
    fn presented_tiles(&self) -> Vec<(egui::TextureId, lizard_render::View)> {
        Vec::new()
    }

    /// Time spent preparing the last GPU canvas update on the UI thread.
    fn last_render_ms(&self) -> Option<f64> {
        None
    }

    /// Rasterize the editor's outline view as GPU strokes when supported.
    fn outline_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _options: lizard_render::Outline,
        _pixel_preview: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Rasterize the focused subtree in Outline mode without drawing its siblings.
    fn focused_outline_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _focus: NodeId,
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _options: lizard_render::Outline,
        _pixel_preview: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Present a lower-resolution GPU scene with nearest-neighbor sampling.
    fn pixel_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Present a focused subtree at document pixel scale with nearest-neighbor sampling.
    fn focused_pixel_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _focus: NodeId,
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Render only the focused subtree at its document-space position.
    fn focus_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _focus: NodeId,
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Replace selected subtrees with outlines in their normal paint order.
    fn selected_outline_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _selected: &[NodeId],
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _options: lizard_render::Outline,
        _proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Draw selected contours at document pixel scale with nearest-neighbor sampling.
    fn selected_outline_pixel_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _selected: &[NodeId],
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _options: lizard_render::Outline,
        _proof_srgb: bool,
        _ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Show selected contours only inside a focused subtree, optionally at document pixel scale.
    fn focused_selected_outline_texture(
        &mut self,
        _doc: Arc<Document>,
        _page: NodeId,
        _focus: NodeId,
        _selected: &[NodeId],
        _view: &lizard_render::View,
        _clear: [f32; 3],
        _options: lizard_render::Outline,
        _proof_srgb: bool,
        _pixel_preview: bool,
    ) -> Option<egui::TextureId> {
        None
    }

    /// Sample the already presented canvas. `Pending` schedules a repaint; `Unavailable` lets
    /// the CPU fallback handle scenes the GPU backend did not draw.
    fn sample_eyedropper(&mut self, _point: Point, _ctx: &egui::Context) -> GpuSample {
        GpuSample::Unavailable
    }
}

pub enum GpuSample {
    Ready(Vec<u8>),
    Pending,
    Unavailable,
}

pub struct CanvasState {
    pub rect: Rect,
    eyedrop_sample: Option<(Pos2, Point, web_time::Instant, Vec<u8>)>,
    gpu_presented: bool,
    texture: Option<egui::TextureHandle>,
    tiles: Vec<CanvasTile>,
    tex_view: Option<lizard_render::View>,
    /// The file ([`lizard_engine::DocState::uid`]) and page the texture shows: it is drawn only
    /// on them.
    tex_of: Option<(u64, NodeId)>,
    /// The document version behind the texture, so stale previews cannot replace newer pixels.
    tex_doc: Option<Arc<Document>>,
    tex_style: Option<RenderStyle>,
    /// A newly created simple layer shown while the page's slower render is still in flight.
    pending_layer: Option<PendingLayer>,
    /// A wider, still-current view kept while a zoomed-in texture is shown. It fills newly
    /// exposed areas on zoom-out without doing a whole-view raster pass in the input frame.
    zoom_fallback: Option<ZoomFallback>,
    /// The texture is a coarse preview (see [`crate::render_worker`]).
    pub tex_coarse: bool,
    submitted: Option<JobKey>,
    /// The view of the last render asked for.
    want_view: Option<lizard_render::View>,
    visible_view: Option<lizard_render::View>,
    shader_started: web_time::Instant,
    /// What the last canvas render had to do (shown by `ui.inspect`).
    pub render_work: Option<lizard_render::Work>,
    /// Device pixels uploaded for the last completed render (zero when its texture was reused).
    pub upload_pixels: u64,
    /// From asking for a render to its preview, and its refined image, handed to egui as the
    /// canvas texture (ms; the texture is uploaded when that frame is painted).
    pub preview_latency_ms: Option<f64>,
    pub refine_latency_ms: Option<f64>,
    dragging: bool,
    layout_motion: LayoutMotion,
    layout_ghost: Option<LayoutGhost>,
    #[cfg(not(target_arch = "wasm32"))]
    drag_visual: Option<DragVisual>,
    /// Last drag input sent to the engine. Repainting a stationary pointer must not clone and
    /// normalize a complex document again; modifiers and pen pressure can still change it.
    last_drag_input: Option<(Pos2, egui::Modifiers, Option<f32>)>,
    space_pan: bool,
    middle_pan: Option<(Pos2, f64, f64)>,
    /// The zoom tool (Z held): zooming out (Alt) or in, and where its drag started.
    zoom_tool: Option<bool>,
    zoom_drag: Option<Pos2>,
    pub last_pointer: Option<Point>,
    /// The real pointer was over the canvas last frame (leaving it clears what it hovered).
    hovering: bool,
    /// What the pointer shows over the canvas (`ui.inspect`); none while it is elsewhere or
    /// something on the canvas with a cursor of its own has it (a ruler guide, a gradient handle).
    pub cursor: Option<crate::cursors::Shown>,
    cursor_textures: crate::cursors::CursorTextures,
    preedit: Option<PreeditView>,
    /// The pen pressure last reported during the current press (pen and stylus touches; the
    /// winit backends and browsers that report it).
    pressure: Option<f32>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            rect: Rect::NOTHING,
            eyedrop_sample: None,
            gpu_presented: false,
            texture: None,
            tiles: Vec::new(),
            tex_view: None,
            tex_of: None,
            tex_doc: None,
            tex_style: None,
            pending_layer: None,
            zoom_fallback: None,
            tex_coarse: false,
            submitted: None,
            want_view: None,
            visible_view: None,
            shader_started: web_time::Instant::now(),
            render_work: None,
            upload_pixels: 0,
            preview_latency_ms: None,
            refine_latency_ms: None,
            dragging: false,
            layout_motion: LayoutMotion::default(),
            layout_ghost: None,
            #[cfg(not(target_arch = "wasm32"))]
            drag_visual: None,
            last_drag_input: None,
            space_pan: false,
            middle_pan: None,
            cursor: None,
            cursor_textures: crate::cursors::CursorTextures::default(),
            zoom_tool: None,
            zoom_drag: None,
            last_pointer: None,
            hovering: false,
            preedit: None,
            pressure: None,
        }
    }
}

struct ZoomFallback {
    texture: Option<egui::TextureHandle>,
    tiles: Vec<CanvasTile>,
    view: lizard_render::View,
    of: (u64, NodeId),
    doc: Arc<Document>,
    style: RenderStyle,
}

struct PendingLayer {
    doc: Arc<Document>,
    id: NodeId,
    visible: lizard_render::View,
    image_view: lizard_render::View,
    texture: egui::TextureHandle,
}

struct CanvasTile {
    view: lizard_render::View,
    image: Arc<egui::ColorImage>,
    texture: egui::TextureHandle,
}

fn view_area(view: &lizard_render::View) -> f64 {
    f64::from(view.width) * f64::from(view.height) / view.scale / view.scale
}

impl CanvasState {
    pub(crate) fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub(crate) fn texture_current(&self, ds: &lizard_engine::DocState) -> bool {
        (self.texture.is_some() || !self.tiles.is_empty())
            && self.tex_of == Some((ds.uid, ds.page))
            && !self.tex_coarse
            && self.tex_doc.as_ref().is_some_and(|doc| Arc::ptr_eq(doc, &ds.doc))
    }

    /// Device pixels without any current image source. This measures texture coverage, including
    /// the retained wide view, before the page background is painted underneath it.
    pub(crate) fn uncovered_pixels(&self, ds: &lizard_engine::DocState) -> Option<u64> {
        if self.gpu_presented {
            return Some(0);
        }
        let visible = self.visible_view.as_ref()?;
        let current = self
            .tex_view
            .as_ref()
            .filter(|_| self.tex_of == Some((ds.uid, ds.page)) && self.tex_doc.as_ref().is_some_and(|doc| Arc::ptr_eq(doc, &ds.doc)));
        let fallback = self
            .zoom_fallback
            .as_ref()
            .filter(|cached| cached.of == (ds.uid, ds.page) && Arc::ptr_eq(&cached.doc, &ds.doc) && self.tex_style.as_ref() == Some(&cached.style));
        let area = |view: &lizard_render::View| {
            let (a, b) = (visible.page_rect(), view.page_rect());
            (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0) * (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0)
        };
        let overlap = current.zip(fallback).map_or(0.0, |(a, b)| {
            let (v, a, b) = (visible.page_rect(), a.page_rect(), b.view.page_rect());
            (v.x1.min(a.x1).min(b.x1) - v.x0.max(a.x0).max(b.x0)).max(0.0) * (v.y1.min(a.y1).min(b.y1) - v.y0.max(a.y0).max(b.y0)).max(0.0)
        });
        let uncovered = (visible.page_rect().width() * visible.page_rect().height()
            - current.map_or(0.0, &area)
            - fallback.map_or(0.0, |cached| area(&cached.view))
            + overlap)
            .max(0.0)
            * visible.scale
            * visible.scale;
        Some(uncovered.round().min(u64::MAX as f64) as u64)
    }
}

/// A frame can be composited separately only while its move is a pure translation. Keeping the
/// page without it avoids an image of the frame at its old position during a fast drag.
#[cfg(not(target_arch = "wasm32"))]
struct DragVisual {
    uid: u64,
    page: NodeId,
    root: NodeId,
    source: Arc<Document>,
    base_view: lizard_render::View,
    background_view: Option<lizard_render::View>,
    visible_view: lizard_render::View,
    root_view: lizard_render::View,
    detail: Option<(lizard_render::View, egui::TextureHandle)>,
    detail_pending: Option<Receiver<(lizard_render::View, egui::ColorImage)>>,
    start_transform: [f64; 6],
    zoom: f64,
    center: (f64, f64),
    canvas_size: egui::Vec2,
    ready: Option<(Option<egui::TextureHandle>, egui::TextureHandle)>,
    pending: Option<Receiver<(Option<egui::ColorImage>, egui::ColorImage, f64)>>,
    prepare_ms: Option<f64>,
}

/// Only the painted positions ease. The document always holds the final layout for hit testing,
/// insertion slots, undo and persistence.
#[derive(Default)]
struct LayoutMotion {
    of: Option<(u64, NodeId)>,
    last_doc: Option<Arc<Document>>,
    positions: HashMap<NodeId, (f64, f64)>,
    last_tick: Option<web_time::Instant>,
    last_ghost: Option<(NodeId, (f64, f64))>,
}

struct LayoutGhost {
    uid: u64,
    root: NodeId,
    source: Arc<Document>,
    view: lizard_render::View,
    texture: Option<egui::TextureHandle>,
    #[cfg(not(target_arch = "wasm32"))]
    pending: Option<Receiver<egui::ColorImage>>,
}

#[cfg(not(target_arch = "wasm32"))]
fn drag_root_is_independent(doc: &Document, page: NodeId, root: NodeId) -> bool {
    let Some(node) = doc.node(root) else { return false };
    if node.parent != Some(page) || !matches!(node.kind, NodeKind::Frame) || !node.visible || node.is_mask {
        return false;
    }
    // Painting the drag layer above the background preserves z-order only when no visible page
    // sibling is above it. Blend modes or backdrop filters can depend on pixels outside it.
    if doc.node(page).and_then(|p| p.children.iter().rev().find(|id| doc.node(**id).is_some_and(|n| n.visible))).copied() != Some(root) {
        return false;
    }
    let mut pending = vec![root];
    let mut visited = 0;
    while let Some(id) = pending.pop() {
        let Some(n) = doc.node(id) else { return false };
        if n.effects.iter().any(|e| e.visible && e.backdrop_radius() > 0.0)
            || !matches!(n.blend, lizard_color::BlendMode::Normal | lizard_color::BlendMode::PassThrough)
        {
            return false;
        }
        visited += 1;
        if visited > 20_000 {
            return false;
        }
        pending.extend(n.children.iter().copied());
    }
    true
}

fn drag_view_for_bounds(bounds: lizard_geom::Rect, base: &lizard_render::View, scale: f64, pixel_limit: f64) -> Option<lizard_render::View> {
    if !scale.is_finite() || scale <= 0.0 || ![bounds.x0, bounds.y0, bounds.x1, bounds.y1].iter().all(|n| n.is_finite()) {
        return None;
    }
    let x0 = ((bounds.x0 - base.origin.0) * scale).floor() - 2.0;
    let y0 = ((bounds.y0 - base.origin.1) * scale).floor() - 2.0;
    let x1 = ((bounds.x1 - base.origin.0) * scale).ceil() + 2.0;
    let y1 = ((bounds.y1 - base.origin.1) * scale).ceil() + 2.0;
    let (w, h) = (x1 - x0, y1 - y0);
    if !w.is_finite() || !h.is_finite() || !(1.0..=4096.0).contains(&w) || !(1.0..=4096.0).contains(&h) || w * h > pixel_limit {
        return None;
    }
    Some(lizard_render::View { origin: (base.origin.0 + x0 / scale, base.origin.1 + y0 / scale), scale, width: w as u32, height: h as u32 })
}

fn cropped_drag_view(doc: &Document, root: NodeId, base: &lizard_render::View) -> Option<lizard_render::View> {
    let bounds = doc.world_render_bounds(root)?;
    let width = bounds.width();
    let height = bounds.height();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    if let Some(full) = drag_view_for_bounds(bounds, base, base.scale, 8_000_000.0) {
        return Some(full);
    }
    // A small full-frame preview follows the cursor immediately. A full-resolution tile of the
    // visible area is prepared separately, so zoom never requires rendering the entire frame.
    let fitted = base.scale.min(2040.0 / width).min(2040.0 / height).min((2_400_000.0 / (width * height)).sqrt());
    let scale = fitted * 0.999;
    if scale < 0.05 || !scale.is_finite() {
        return None;
    }
    drag_view_for_bounds(bounds, base, scale, 2_500_000.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn drag_background_view(base: &lizard_render::View) -> Option<lizard_render::View> {
    let pixels = f64::from(base.width) * f64::from(base.height);
    let factor = (7_900_000.0 / pixels).sqrt().min(8190.0 / f64::from(base.width)).min(8190.0 / f64::from(base.height)).min(1.0);
    if !factor.is_finite() || factor <= 0.0 {
        return None;
    }
    Some(lizard_render::View {
        origin: base.origin,
        scale: base.scale * factor,
        width: (f64::from(base.width) * factor).ceil() as u32,
        height: (f64::from(base.height) * factor).ceil() as u32,
    })
}

/// The part of the original frame needed at the current pointer position, with a small reserve
/// around the viewport. The preview supplies pixels until this tile is ready.
#[cfg(not(target_arch = "wasm32"))]
fn drag_detail_view(visual: &DragVisual, delta: (f64, f64)) -> Option<lizard_render::View> {
    let frame = visual.source.world_render_bounds(visual.root)?;
    let visible = visual.visible_view.page_rect();
    let needed = lizard_geom::Rect::new(
        (visible.x0 - delta.0).max(frame.x0),
        (visible.y0 - delta.1).max(frame.y0),
        (visible.x1 - delta.0).min(frame.x1),
        (visible.y1 - delta.1).min(frame.y1),
    );
    if needed.width() <= 0.0 || needed.height() <= 0.0 {
        return None;
    }
    let scale = visual.base_view.scale;
    let mut margin = 192.0 / scale;
    for _ in 0..5 {
        let bounds = lizard_geom::Rect::new(
            (needed.x0 - margin).max(frame.x0),
            (needed.y0 - margin).max(frame.y0),
            (needed.x1 + margin).min(frame.x1),
            (needed.y1 + margin).min(frame.y1),
        );
        if let Some(view) = drag_view_for_bounds(bounds, &visual.base_view, scale, 8_000_000.0) {
            return Some(view);
        }
        margin *= 0.5;
    }
    drag_view_for_bounds(needed, &visual.base_view, scale, 8_000_000.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn detail_covers_visible(visual: &DragVisual, detail: &lizard_render::View, delta: (f64, f64)) -> bool {
    let Some(frame) = visual.source.world_render_bounds(visual.root) else { return false };
    let visible = visual.visible_view.page_rect();
    let needed = lizard_geom::Rect::new(
        (visible.x0 - delta.0).max(frame.x0),
        (visible.y0 - delta.1).max(frame.y0),
        (visible.x1 - delta.0).min(frame.x1),
        (visible.y1 - delta.1).min(frame.y1),
    );
    needed.width() <= 0.0 || needed.height() <= 0.0 || {
        let covered = detail.page_rect();
        let eps = 1.0 / detail.scale;
        covered.x0 <= needed.x0 + eps && covered.y0 <= needed.y0 + eps && covered.x1 >= needed.x1 - eps && covered.y1 >= needed.y1 - eps
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn start_drag_visual(app: &mut LizardApp, ctx: &egui::Context) {
    app.canvas.drag_visual = None;
    if app.session.tool.tool != ToolId::Move {
        return;
    }
    let Ok(ds) = app.session.ds() else { return };
    let [root] = ds.selection.as_slice() else { return };
    let root = *root;
    if ds.view.outlines
        || ds.view.outlines_selection
        || ds.view.proof_srgb
        || ds.text_edit.is_some()
        || !ds.doc.color_profile.is_srgb()
        || ds.focused().is_some()
        || !drag_root_is_independent(&ds.doc, ds.page, root)
    {
        return;
    }
    let Some(base_view) = app.canvas.tex_view.as_ref() else { return };
    if app.canvas.tex_coarse
        || app.canvas.tex_of != Some((ds.uid, ds.page))
        || !app.canvas.tex_doc.as_ref().is_some_and(|shown| Arc::ptr_eq(shown, &ds.doc))
        || (app.canvas.texture.is_none() && app.canvas.tiles.is_empty())
    {
        return;
    }
    let target_scale = ds.view.zoom * f64::from(ctx.pixels_per_point());
    if (base_view.scale - target_scale).abs() > target_scale * 1e-4 {
        return;
    }
    let visible = lizard_render::View {
        origin: ds.view.origin(),
        scale: target_scale,
        width: (app.canvas.rect.width() * ctx.pixels_per_point()).ceil().max(1.0) as u32,
        height: (app.canvas.rect.height() * ctx.pixels_per_point()).ceil().max(1.0) as u32,
    };
    if !covers(base_view, &visible) {
        return;
    }
    let has_visible_siblings =
        ds.doc.node(ds.page).is_some_and(|page| page.children.iter().any(|id| *id != root && ds.doc.node(*id).is_some_and(|n| n.visible)));
    let background_view = if has_visible_siblings { drag_background_view(base_view) } else { None };
    if has_visible_siblings && background_view.is_none() {
        return;
    }
    let Some(root_view) = cropped_drag_view(&ds.doc, root, base_view) else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    let source = ds.doc.clone();
    let job_doc = source.clone();
    let page = ds.page;
    let background_for_job = background_view.clone();
    let root_for_job = root_view.clone();
    let repaint = ctx.clone();
    let viewport = ctx.viewport_id();
    let spawned = std::thread::Builder::new().name("lizard-drag-layers".into()).spawn(move || {
        let started = web_time::Instant::now();
        let base = if let Some(view) = background_for_job.as_ref() {
            let mut background = (*job_doc).clone();
            let Some(node) = background.node_mut(root) else { return };
            node.visible = false;
            let Some(pix) = lizard_engine::cmd::file::render_page_screen_pixels(&background, page, view) else { return };
            Some(crate::render_worker::screen_image(&pix, lizard_color::ColorProfile::Srgb))
        } else {
            None
        };
        let Some(moving) = lizard_engine::cmd::file::render_export_area_screen(&job_doc, &[root], root_for_job.page_rect(), root_for_job.scale)
        else {
            return;
        };
        let images = (base, crate::render_worker::screen_image(&moving, lizard_color::ColorProfile::Srgb), started.elapsed().as_secs_f64() * 1000.0);
        if tx.send(images).is_ok() {
            repaint.request_repaint_of(viewport);
        }
    });
    if let Err(e) = spawned {
        log::warn!("drag layer worker: {e}");
        return;
    }
    app.canvas.drag_visual = Some(DragVisual {
        uid: ds.uid,
        page: ds.page,
        root,
        source,
        base_view: base_view.clone(),
        background_view,
        visible_view: visible,
        root_view,
        detail: None,
        detail_pending: None,
        start_transform: ds.doc.world_transform(root).as_coeffs(),
        zoom: ds.view.zoom,
        center: (ds.view.cx, ds.view.cy),
        canvas_size: app.canvas.rect.size(),
        ready: None,
        pending: Some(rx),
        prepare_ms: None,
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn drag_visual_matches_idle_document(app: &LizardApp) -> bool {
    let Some(visual) = app.canvas.drag_visual.as_ref() else { return false };
    let Ok(ds) = app.session.ds() else { return false };
    visual.uid == ds.uid
        && visual.page == ds.page
        && ds.selection.as_slice() == [visual.root]
        && Arc::ptr_eq(&visual.source, &ds.doc)
        && ds.view.zoom.to_bits() == visual.zoom.to_bits()
        && (ds.view.cx, ds.view.cy) == visual.center
        && app.canvas.rect.size() == visual.canvas_size
}

#[cfg(not(target_arch = "wasm32"))]
fn poll_drag_visual(app: &mut LizardApp, ctx: &egui::Context) {
    let Some(visual) = app.canvas.drag_visual.as_mut() else { return };
    if let Some(rx) = visual.pending.as_ref() {
        match rx.try_recv() {
            Ok((base, moving, ms)) => {
                visual.ready = Some((
                    base.map(|image| ctx.load_texture("lizard-drag-base", image, egui::TextureOptions::LINEAR)),
                    ctx.load_texture("lizard-drag-frame", moving, egui::TextureOptions::LINEAR),
                ));
                visual.pending = None;
                visual.prepare_ms = Some(ms);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                app.canvas.drag_visual = None;
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    if let Some(rx) = visual.detail_pending.as_ref() {
        match rx.try_recv() {
            Ok((view, image)) => {
                visual.detail = Some((view, ctx.load_texture("lizard-drag-detail", image, egui::TextureOptions::LINEAR)));
                visual.detail_pending = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => visual.detail_pending = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    if app.canvas.dragging {
        request_drag_detail(app, ctx);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn request_drag_detail(app: &mut LizardApp, ctx: &egui::Context) {
    let Some(delta) = drag_visual_delta(app) else { return };
    let Some(visual) = app.canvas.drag_visual.as_mut() else { return };
    if visual.root_view.scale == visual.base_view.scale
        || visual.detail_pending.is_some()
        || visual.detail.as_ref().is_some_and(|(view, _)| detail_covers_visible(visual, view, delta))
    {
        return;
    }
    let Some(view) = drag_detail_view(visual, delta) else { return };
    let source = visual.source.clone();
    let root = visual.root;
    let repaint = ctx.clone();
    let viewport = ctx.viewport_id();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("lizard-drag-detail".into()).spawn(move || {
        let Some(pix) = lizard_engine::cmd::file::render_export_area_screen(&source, &[root], view.page_rect(), view.scale) else { return };
        let image = crate::render_worker::screen_image(&pix, lizard_color::ColorProfile::Srgb);
        if tx.send((view, image)).is_ok() {
            repaint.request_repaint_of(viewport);
        }
    });
    if let Err(e) = spawned {
        log::warn!("drag detail worker: {e}");
        return;
    }
    visual.detail_pending = Some(rx);
}

pub(crate) fn drag_mode(app: &LizardApp) -> &'static str {
    #[cfg(not(target_arch = "wasm32"))]
    if app.canvas.dragging && drag_visual_delta(app).is_some() {
        return "composited";
    }
    #[cfg(target_arch = "wasm32")]
    let _ = app;
    "render"
}

pub(crate) fn drag_prepare_ms(app: &LizardApp) -> Option<f64> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        app.canvas.drag_visual.as_ref().and_then(|v| v.prepare_ms)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drag_visual_delta(app: &LizardApp) -> Option<(f64, f64)> {
    let visual = app.canvas.drag_visual.as_ref()?;
    visual.ready.as_ref()?;
    let ds = app.session.ds().ok()?;
    if ds.uid != visual.uid
        || ds.page != visual.page
        || ds.selection.as_slice() != [visual.root]
        || ds.view.zoom.to_bits() != visual.zoom.to_bits()
        || (ds.view.cx, ds.view.cy) != visual.center
        || app.canvas.rect.size() != visual.canvas_size
    {
        return None;
    }
    let before = visual.source.node(visual.root)?;
    let now = ds.doc.node(visual.root)?;
    if now.parent != before.parent
        || now.children != before.children
        || now.width != before.width
        || now.height != before.height
        || now.layout != before.layout
    {
        return None;
    }
    let xf = ds.doc.world_transform(visual.root).as_coeffs();
    if xf[..4] != visual.start_transform[..4] {
        return None;
    }
    let dx = xf[4] - visual.start_transform[4];
    let dy = xf[5] - visual.start_transform[5];
    (dx.is_finite() && dy.is_finite()).then_some((dx, dy))
}

#[cfg(not(target_arch = "wasm32"))]
fn paint_drag_visual(app: &LizardApp, painter: &egui::Painter) -> bool {
    // An idle prepared layer must not replace the normal canvas image. After mouse-up, keep the
    // moved layer visible until the renderer catches up with the committed document.
    if !app.canvas.dragging && drag_visual_matches_idle_document(app) {
        return false;
    }
    let Some((dx, dy)) = drag_visual_delta(app) else { return false };
    let Some(visual) = app.canvas.drag_visual.as_ref() else { return false };
    let Some((base, moving)) = visual.ready.as_ref() else { return false };
    let draw = |tex: &egui::TextureHandle, view: &lizard_render::View, delta: (f64, f64)| {
        let tl = app.to_screen(view.origin.0 + delta.0, view.origin.1 + delta.1);
        let zoom = app.session.ds().map(|ds| ds.view.zoom).unwrap_or(1.0);
        let size = vec2((f64::from(view.width) / view.scale * zoom) as f32, (f64::from(view.height) / view.scale * zoom) as f32);
        painter.image(tex.id(), Rect::from_min_size(tl, size), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    };
    if let (Some(base), Some(view)) = (base.as_ref(), visual.background_view.as_ref()) {
        draw(base, view, (0.0, 0.0));
    }
    draw(moving, &visual.root_view, (dx, dy));
    if let Some((view, detail)) = visual.detail.as_ref() {
        draw(detail, view, (dx, dy));
    }
    true
}

fn layout_motion_document(app: &mut LizardApp, ctx: &egui::Context) -> Option<Arc<Document>> {
    let reduce_motion = app.reduce_motion();
    let ds = app.session.ds().ok()?;
    let current = ds.doc.clone();
    let ghost = app.session.tool.overlay.drag_ghost;
    let motion = &mut app.canvas.layout_motion;
    if motion.of != Some((ds.uid, ds.page)) {
        *motion = LayoutMotion { of: Some((ds.uid, ds.page)), ..Default::default() };
    }
    if let Some((id, [dx, dy])) = ghost
        && let Some(source) = ds.interaction.as_ref().map(|it| &it.doc)
    {
        let xf = source.world_transform(id).as_coeffs();
        motion.last_ghost = Some((id, (xf[4] + dx, xf[5] + dy)));
    }
    if let Some(previous) = motion.last_doc.as_ref()
        && !Arc::ptr_eq(previous, &current)
        && (ghost.is_some() || motion.last_ghost.is_some())
    {
        for node in current.nodes() {
            let id = node.id;
            let old_parent = previous.node(id).and_then(|n| n.parent);
            let in_layout = node.parent.and_then(|p| current.node(p)).is_some_and(|p| p.layout.is_some())
                || old_parent.and_then(|p| previous.node(p)).is_some_and(|p| p.layout.is_some());
            if !in_layout || previous.node(id).is_none() {
                continue;
            }
            let target = current.world_transform(id).as_coeffs();
            let from = if motion.last_ghost.is_some_and(|(g, _)| g == id) && ghost.is_none() {
                motion.last_ghost.map(|(_, xy)| xy)
            } else {
                motion.positions.get(&id).copied().or_else(|| {
                    let old = previous.world_transform(id).as_coeffs();
                    Some((old[4], old[5]))
                })
            };
            if let Some((x, y)) = from
                && ((x - target[4]).abs() + (y - target[5]).abs()) * ds.view.zoom > 0.5
            {
                motion.positions.insert(id, (x, y));
            }
        }
    }
    motion.last_doc = Some(current.clone());
    let now = web_time::Instant::now();
    let dt = motion.last_tick.replace(now).map_or(0.016, |then| now.duration_since(then).as_secs_f64().min(0.05));
    let blend = if reduce_motion { 1.0 } else { 1.0 - (-dt / 0.075).exp() };
    motion.positions.retain(|id, pos| {
        let Some(node) = current.node(*id) else { return false };
        if node.opacity == 0.0 && ghost.is_some_and(|(g, _)| g == *id) {
            return false;
        }
        let target = current.world_transform(*id).as_coeffs();
        pos.0 += (target[4] - pos.0) * blend;
        pos.1 += (target[5] - pos.1) * blend;
        ((pos.0 - target[4]).abs() + (pos.1 - target[5]).abs()) * ds.view.zoom > 0.35
    });
    if motion.positions.is_empty() {
        if ghost.is_none() {
            motion.last_ghost = None;
        }
        return None;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(16));
    let mut painted = (*current).clone();
    for (id, (x, y)) in &motion.positions {
        let mut xf = current.world_transform(*id).as_coeffs();
        xf[4] = *x;
        xf[5] = *y;
        let _ = painted.set_world_transform(*id, lizard_geom::Affine::new(xf));
    }
    Some(Arc::new(painted))
}

fn update_layout_ghost(app: &mut LizardApp, ctx: &egui::Context) {
    let Some((root, _)) = app.session.tool.overlay.drag_ghost else {
        app.canvas.layout_ghost = None;
        return;
    };
    let Ok(ds) = app.session.ds() else { return };
    let Some(source) = ds.interaction.as_ref().map(|it| it.doc.clone()) else { return };
    let same = app.canvas.layout_ghost.as_ref().is_some_and(|g| g.uid == ds.uid && g.root == root && Arc::ptr_eq(&g.source, &source));
    if !same {
        let Some(base) = app.canvas.visible_view.as_ref() else { return };
        let Some(view) = cropped_drag_view(&source, root, base) else { return };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let job_doc = source.clone();
            let job_view = view.clone();
            let repaint = ctx.clone();
            let viewport = ctx.viewport_id();
            let spawned = std::thread::Builder::new().name("lizard-layout-ghost".into()).spawn(move || {
                let Some(pixels) = lizard_engine::cmd::file::render_export_area_screen(&job_doc, &[root], job_view.page_rect(), job_view.scale)
                else {
                    return;
                };
                let image = crate::render_worker::screen_image(&pixels, lizard_color::ColorProfile::Srgb);
                if tx.send(image).is_ok() {
                    repaint.request_repaint_of(viewport);
                }
            });
            if spawned.is_err() {
                return;
            }
            app.canvas.layout_ghost = Some(LayoutGhost { uid: ds.uid, root, source, view, texture: None, pending: Some(rx) });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let texture = lizard_engine::cmd::file::render_export_area_screen(&source, &[root], view.page_rect(), view.scale)
                .map(|pixels| crate::render_worker::screen_image(&pixels, lizard_color::ColorProfile::Srgb))
                .map(|image| ctx.load_texture("lizard-layout-ghost", image, egui::TextureOptions::LINEAR));
            app.canvas.layout_ghost = Some(LayoutGhost { uid: ds.uid, root, source, view, texture });
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ghost) = app.canvas.layout_ghost.as_mut()
        && let Some(rx) = ghost.pending.as_ref()
    {
        match rx.try_recv() {
            Ok(image) => {
                ghost.texture = Some(ctx.load_texture("lizard-layout-ghost", image, egui::TextureOptions::LINEAR));
                ghost.pending = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => ghost.pending = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
}

fn paint_layout_ghost(app: &LizardApp, painter: &egui::Painter) {
    let Some((root, [dx, dy])) = app.session.tool.overlay.drag_ghost else { return };
    let Some(ghost) = app.canvas.layout_ghost.as_ref().filter(|g| g.root == root) else { return };
    let Some(texture) = ghost.texture.as_ref() else { return };
    let Ok(ds) = app.session.ds() else { return };
    let cross_layout = ds.doc.node(root).and_then(|n| n.parent) != ghost.source.node(root).and_then(|n| n.parent);
    let zoom = ds.view.zoom;
    let tl = app.to_screen(ghost.view.origin.0 + dx, ghost.view.origin.1 + dy);
    let size = vec2((f64::from(ghost.view.width) / ghost.view.scale * zoom) as f32, (f64::from(ghost.view.height) / ghost.view.scale * zoom) as f32);
    painter.image(
        texture.id(),
        Rect::from_min_size(tl, size),
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::from_white_alpha(if cross_layout { 145 } else { 255 }),
    );
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod drag_visual_tests {
    use super::*;
    use serde_json::json;
    use tiny_skia::{PixmapPaint, Transform};

    #[test]
    fn isolated_top_frame_matches_full_page_at_rest() {
        let mut session = lizard_engine::Session::new();
        let id = |v: serde_json::Value| lizard_engine::params::id_of(&v["id"]).unwrap();
        session.execute("shape.rectangle", &json!({"x": 135, "y": 90, "width": 250, "height": 200, "props": {"fill": "#406080"}})).unwrap();
        let frame =
            id(session.execute("frame.create", &json!({"x": 150, "y": 100, "width": 200, "height": 150, "props": {"fill": "#203040"}})).unwrap());
        session
            .execute("shape.rectangle", &json!({"x": 20, "y": 25, "width": 80, "height": 60, "parent": frame, "props": {"fill": "#e03030"}}))
            .unwrap();
        let ds = session.ds().unwrap();
        assert!(drag_root_is_independent(&ds.doc, ds.page, frame));
        let view = lizard_render::View { origin: (0.0, 0.0), scale: 1.0, width: 512, height: 400 };
        let opts = lizard_render::Options { lod: true, ..Default::default() };
        let full = lizard_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let opts = lizard_render::Options { skip: vec![frame], lod: true, ..Default::default() };
        let mut base = lizard_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let crop = cropped_drag_view(&ds.doc, frame, &view).unwrap();
        let mut moving = lizard_render::Pixmap::new(crop.width, crop.height).unwrap();
        lizard_render::render_nodes_into_preview(&ds.doc, &[frame], &crop, &mut moving, true);
        let x = ((crop.origin.0 - view.origin.0) * view.scale).round() as i32;
        let y = ((crop.origin.1 - view.origin.1) * view.scale).round() as i32;
        base.draw_pixmap(x, y, moving.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        assert_eq!(base.data(), full.data(), "separate background and frame must have the same pixels at rest");
    }

    #[test]
    fn gpu_first_drag_layers_preserve_background_and_moving_frame() {
        let mut session = lizard_engine::Session::new();
        let id = |value: serde_json::Value| lizard_engine::params::id_of(&value["id"]).unwrap();
        session.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 120, "height": 80, "props": {"fill": "#406080"}})).unwrap();
        let frame = id(session.execute("frame.create", &json!({"x": 40, "y": 30, "width": 80, "height": 60, "props": {"fill": "#203040"}})).unwrap());
        session
            .execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 20, "height": 20, "parent": frame, "props": {"fill": "#e03030"}}))
            .unwrap();
        let ds = session.ds().unwrap();
        let view = lizard_render::View { origin: (0.0, 0.0), scale: 1.0, width: 160, height: 120 };
        let full = lizard_render::render_view(&ds.doc, ds.page, &view, &Default::default()).unwrap();
        let mut hidden = (*ds.doc).clone();
        hidden.node_mut(frame).unwrap().visible = false;
        let mut base = lizard_engine::cmd::file::render_page_pixels(&hidden, ds.page, &view).unwrap();
        let crop = cropped_drag_view(&ds.doc, frame, &view).unwrap();
        let moving = lizard_engine::cmd::file::render_export_area(&ds.doc, &[frame], crop.page_rect(), crop.scale).unwrap();
        assert_eq!((moving.width(), moving.height()), (crop.width, crop.height));
        let x = ((crop.origin.0 - view.origin.0) * view.scale).round() as i32;
        let y = ((crop.origin.1 - view.origin.1) * view.scale).round() as i32;
        base.draw_pixmap(x, y, moving.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        for (x, y) in [(20, 20), (45, 35), (55, 45), (135, 95)] {
            let actual = base.pixel(x, y).unwrap();
            let expected = full.pixel(x, y).unwrap();
            for (a, b) in [actual.red(), actual.green(), actual.blue(), actual.alpha()].into_iter().zip([
                expected.red(),
                expected.green(),
                expected.blue(),
                expected.alpha(),
            ]) {
                assert!(a.abs_diff(b) <= 2, "x={x}, y={y}: GPU-first={actual:?}, CPU={expected:?}");
            }
        }
    }

    #[test]
    fn zoomed_frame_uses_bounded_preview_and_sharp_visible_tile() {
        let mut session = lizard_engine::Session::new();
        let id = |v: serde_json::Value| lizard_engine::params::id_of(&v["id"]).unwrap();
        let frame =
            id(session.execute("frame.create", &json!({"x": 100, "y": 100, "width": 2048, "height": 1210, "props": {"fill": "#203040"}})).unwrap());
        session
            .execute("shape.rectangle", &json!({"parent": frame, "x": 20, "y": 30, "width": 60, "height": 80, "props": {"fill": "#e03030"}}))
            .unwrap();
        let ds = session.ds().unwrap();
        let view = lizard_render::View { origin: (90.0, 90.0), scale: 8.0, width: 1600, height: 900 };
        let preview = cropped_drag_view(&ds.doc, frame, &view).unwrap();
        assert!(preview.scale < view.scale);
        assert!(preview.width <= 4096 && preview.height <= 4096);
        assert!(u64::from(preview.width) * u64::from(preview.height) <= 8_000_000);
        let preview_pixels = lizard_engine::cmd::file::render_export_area(&ds.doc, &[frame], preview.page_rect(), preview.scale).unwrap();
        assert_eq!((preview_pixels.width(), preview_pixels.height()), (preview.width, preview.height));
        let visual = DragVisual {
            uid: ds.uid,
            page: ds.page,
            root: frame,
            source: ds.doc.clone(),
            base_view: view.clone(),
            background_view: None,
            visible_view: view.clone(),
            root_view: preview,
            detail: None,
            detail_pending: None,
            start_transform: ds.doc.world_transform(frame).as_coeffs(),
            zoom: 8.0,
            center: (0.0, 0.0),
            canvas_size: vec2(1600.0, 900.0),
            ready: None,
            pending: None,
            prepare_ms: None,
        };
        let detail = drag_detail_view(&visual, (0.0, 0.0)).unwrap();
        assert_eq!(detail.scale, view.scale);
        assert!(detail_covers_visible(&visual, &detail, (0.0, 0.0)));
        assert!(!detail_covers_visible(&visual, &detail, (-90.0, -60.0)));
        let detail_pixels = lizard_engine::cmd::file::render_export_area(&ds.doc, &[frame], detail.page_rect(), detail.scale).unwrap();
        assert_eq!((detail_pixels.width(), detail_pixels.height()), (detail.width, detail.height));
        let opts = lizard_render::Options { lod: true, ..Default::default() };
        let full = lizard_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let opts = lizard_render::Options { skip: vec![frame], lod: true, ..Default::default() };
        let mut base = lizard_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let mut moving = lizard_render::Pixmap::new(detail.width, detail.height).unwrap();
        lizard_render::render_nodes_into_preview(&ds.doc, &[frame], &detail, &mut moving, true);
        let x = ((detail.origin.0 - view.origin.0) * view.scale).round() as i32;
        let y = ((detail.origin.1 - view.origin.1) * view.scale).round() as i32;
        base.draw_pixmap(x, y, moving.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        assert_eq!(base.data(), full.data(), "the sharp tile must reconstruct the visible frame pixels");
    }

    #[test]
    fn large_window_background_is_bounded_without_changing_its_page_extent() {
        let base = lizard_render::View { origin: (45.0, -20.0), scale: 2.0, width: 9000, height: 3500 };
        let preview = drag_background_view(&base).unwrap();
        assert!(preview.scale < base.scale);
        assert!(preview.width <= 8192 && preview.height <= 8192);
        assert!(u64::from(preview.width) * u64::from(preview.height) <= 8_000_000);
        assert_eq!(preview.origin, base.origin);
        assert!((preview.page_rect().width() - base.page_rect().width()).abs() < 1.0);
        assert!((preview.page_rect().height() - base.page_rect().height()).abs() < 1.0);
    }
}

/// What the canvas shows while an IME composition is in progress: the document with the
/// composition inserted at the caret (the real document only gets it on commit).
pub struct PreeditView {
    /// The document, composition and caret it was built from.
    key: (Arc<Document>, Preedit, usize),
    pub doc: Arc<Document>,
    /// The composition's char range, and the IME cursor, in `doc`'s edited text.
    pub range: (usize, usize),
    pub cursor: usize,
}

impl LizardApp {
    fn preedit_key(&self) -> Option<(Arc<Document>, Preedit, usize)> {
        let ds = self.session.ds().ok()?;
        let te = ds.text_edit.as_ref()?;
        Some((ds.doc.clone(), te.preedit.clone()?, te.caret))
    }

    /// Rebuild the composition view when the document or composition changed.
    pub fn sync_preedit(&mut self) {
        if self.preedit_view().is_some() {
            return;
        }
        self.canvas.preedit = self.preedit_key().and_then(|key| {
            let (doc, range, cursor) = lizard_engine::textedit::preedit_doc(&self.session)?;
            Some(PreeditView { key, doc: Arc::new(doc), range, cursor })
        });
    }

    /// The composition view, when it matches the current state.
    pub fn preedit_view(&self) -> Option<&PreeditView> {
        let p = self.canvas.preedit.as_ref()?;
        let (doc, pe, caret) = self.preedit_key()?;
        (Arc::ptr_eq(&p.key.0, &doc) && p.key.1 == pe && p.key.2 == caret).then_some(p)
    }
}

#[derive(Clone, PartialEq)]
struct JobKey {
    doc: usize,
    page: NodeId,
    origin: (i64, i64),
    scale: i64,
    size: (u32, u32),
    outline: bool,
    outline_extras: lizard_render::Outline,
    outlined: Vec<NodeId>,
    skip: Vec<NodeId>,
    solo: Option<NodeId>,
    /// Text lays out anew when fonts arrive (system fonts load in the background).
    fonts: u64,
    proof: bool,
    animation_frame: u64,
}

/// Only visible animated paints keep the canvas clock awake. Hidden branches do not count.
fn animated_shader_visible(doc: &Document, page: NodeId, view: &lizard_render::View) -> bool {
    let visible = view.page_rect();
    let mut pending = doc.node(page).map(|node| node.children.clone()).unwrap_or_default();
    while let Some(id) = pending.pop() {
        let Some(node) = doc.node(id) else { continue };
        if !node.visible || node.opacity <= 0.0 {
            continue;
        }
        let intersects = doc.world_bounds(id).is_some_and(|b| b.x0 <= visible.x1 && b.x1 >= visible.x0 && b.y0 <= visible.y1 && b.y1 >= visible.y0);
        if intersects {
            let animated = |paint: &lizard_color::Paint| {
                paint.visible
                    && paint.opacity > 0.0
                    && matches!(&paint.kind, PaintKind::Shader(fill) if doc.shaders.get(&fill.shader_id).is_some_and(|def| def.features.animated))
            };
            let region_shader = matches!(&node.kind, NodeKind::Vector { network }
                if network.regions.iter().filter_map(|region| region.fills.as_ref()).flatten().any(animated));
            let text_shader = matches!(&node.kind, NodeKind::Text(text)
                if std::iter::once(&text.style).chain(text.runs.iter().map(|run| &run.style))
                    .filter_map(|style| style.fills.as_ref()).flatten().any(animated));
            if node.fills.iter().chain(&node.strokes).any(animated) || region_shader || text_shader {
                return true;
            }
        }
        if intersects || !node.clip {
            pending.extend(node.children.iter().copied());
        }
    }
    false
}

impl LizardApp {
    /// Page → screen (logical points).
    pub fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        let Ok(ds) = self.session.ds() else { return Pos2::ZERO };
        let (sx, sy) = ds.view.page_to_screen(x, y);
        pos2(self.canvas.rect.min.x + sx as f32, self.canvas.rect.min.y + sy as f32)
    }
    pub fn to_page(&self, p: Pos2) -> Point {
        let Ok(ds) = self.session.ds() else { return Point::ZERO };
        let (x, y) = ds.view.screen_to_page(f64::from(p.x - self.canvas.rect.min.x), f64::from(p.y - self.canvas.rect.min.y));
        Point::new(x, y)
    }
}

fn pointer_event(app: &LizardApp, kind: PointerKind, pos: Pos2, m: &egui::Modifiers) -> PointerEvent {
    let p = app.to_page(pos);
    let zoom = app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0);
    PointerEvent {
        kind,
        x: p.x,
        y: p.y,
        sx: f64::from(pos.x),
        sy: f64::from(pos.y),
        shift: m.shift,
        alt: m.alt,
        cmd: m.command,
        ctrl: m.ctrl,
        zoom,
        pressure: app.canvas.pressure,
    }
}

/// Ease pinch and unmeasured wheel input in logarithmic zoom space. A factor such as 1.25
/// becomes about 1.20, and its reciprocal stays reciprocal, so zoom in and out feel alike.
const ZOOM_GESTURE_GAIN: f64 = 0.8;

fn canvas_zoom_delta(raw: f32) -> Option<f64> {
    (raw.is_finite() && raw > 0.0).then(|| f64::from(raw).powf(ZOOM_GESTURE_GAIN))
}

/// The measured Chrome/macOS Design wheel path uses the unsmoothed per-event delta.
/// `egui::Event::MouseWheel` already negates the browser's `WheelEvent.deltaY`.
fn measured_wheel_zoom(delta_y: f32, modifiers: egui::Modifiers) -> Option<f64> {
    let delta = f64::from(delta_y);
    if !delta.is_finite() || delta == 0.0 {
        return None;
    }
    let factor = if modifiers.ctrl {
        4.0_f64.powf(delta / 100.0)
    } else if modifiers.mac_cmd {
        let magnitude = delta.abs();
        let compressed = magnitude.min(30.0 * (magnitude / 30.0).cbrt());
        0.995_f64.powf(-delta.signum() * compressed)
    } else {
        return None;
    };
    Some(factor.clamp(0.1, 10.0))
}

/// A selected, newly added page layer can appear over the last canvas texture immediately. A
/// large effect behind it may still be rendering, but it cannot change this layer's own pixels.
fn simple_pending_layer(doc: &Document, page: NodeId, id: NodeId) -> bool {
    let Some(node) = doc.node(id) else { return false };
    if node.parent != Some(page)
        || doc.node(page).and_then(|p| p.children.last()) != Some(&id)
        || !node.visible
        || node.is_mask
        || !node.children.is_empty()
        || !node.effects.is_empty()
        || node.blend != BlendMode::Normal
        || node.fills.iter().chain(&node.strokes).any(|p| !matches!(p.kind, PaintKind::Solid { .. } | PaintKind::Gradient(_)))
        || doc.nodes().any(|n| n.is_mask && doc.page_of(n.id) == Some(page))
    {
        return false;
    }
    matches!(
        node.kind,
        NodeKind::Text(_) | NodeKind::Rectangle | NodeKind::Ellipse { .. } | NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::Line
    )
}

/// The device-pixel rectangle needed for just this layer, aligned with a snapped full render.
fn pending_layer_view(doc: &Document, id: NodeId, visible: &lizard_render::View) -> Option<lizard_render::View> {
    let (snapped, px) = lizard_render::retained::snap(visible)?;
    let bounds = lizard_render::draw::paint_bounds(doc, id)?;
    let s = snapped.scale;
    let (left, top, right, bottom) = (bounds.x0 * s, bounds.y0 * s, bounds.x1 * s, bounds.y1 * s);
    if ![left, top, right, bottom].iter().all(|n| n.is_finite() && n.abs() < 1e15) {
        return None;
    }
    let x0 = ((left.floor() - 1.0) - px.0 as f64).clamp(0.0, f64::from(snapped.width)) as u32;
    let y0 = ((top.floor() - 1.0) - px.1 as f64).clamp(0.0, f64::from(snapped.height)) as u32;
    let x1 = ((right.ceil() + 1.0) - px.0 as f64).clamp(0.0, f64::from(snapped.width)) as u32;
    let y1 = ((bottom.ceil() + 1.0) - px.1 as f64).clamp(0.0, f64::from(snapped.height)) as u32;
    let (width, height) = (x1.checked_sub(x0)?, y1.checked_sub(y0)?);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 4_000_000 {
        return None;
    }
    Some(lizard_render::View { origin: ((px.0 + i64::from(x0)) as f64 / s, (px.1 + i64::from(y0)) as f64 / s), scale: s, width, height })
}

fn paint_pending_layer(app: &mut LizardApp, painter: &egui::Painter, ctx: &egui::Context, ppp: f32) {
    // A current GPU texture already contains the newly created layer. Rasterizing a second
    // CPU preview here duplicates work and can briefly paint a stale layer over the GPU scene.
    if app.canvas.gpu_presented {
        app.canvas.pending_layer = None;
        return;
    }
    let candidate = (|| {
        let ds = app.session.ds().ok()?;
        if app.canvas.texture_current(ds)
            || ds.view.outlines
            || ds.view.outlines_selection
            || ds.view.pixel_preview_scale().is_some()
            || ds.view.proof_srgb
            || ds.focused().is_some()
            || app.preedit_view().is_some()
        {
            return None;
        }
        let [id] = ds.selection.as_slice() else { return None };
        if app.canvas.tex_of == Some((ds.uid, ds.page)) && app.canvas.tex_doc.as_ref().is_some_and(|old| old.node(*id).is_some()) {
            return None;
        }
        let visible = app.canvas.visible_view.clone()?;
        simple_pending_layer(&ds.doc, ds.page, *id).then(|| (ds.doc.clone(), *id, visible, ds.view.zoom))
    })();
    let Some((doc, id, visible, zoom)) = candidate else {
        app.canvas.pending_layer = None;
        return;
    };
    let cached = app.canvas.pending_layer.as_ref().is_some_and(|old| {
        Arc::ptr_eq(&old.doc, &doc)
            && old.id == id
            && old.visible.origin == visible.origin
            && old.visible.scale.to_bits() == visible.scale.to_bits()
            && old.visible.width == visible.width
            && old.visible.height == visible.height
    });
    if !cached {
        app.canvas.pending_layer = (|| {
            let image_view = pending_layer_view(&doc, id, &visible)?;
            let mut pix = lizard_render::Pixmap::new(image_view.width, image_view.height)?;
            lizard_render::render_nodes_into_preview(&doc, &[id], &image_view, &mut pix, true);
            let image = crate::render_worker::screen_image(&pix, doc.color_profile);
            let texture = ctx.load_texture("lizard-pending-layer", image, egui::TextureOptions::LINEAR);
            Some(PendingLayer { doc, id, visible, image_view, texture })
        })();
    }
    if let Some(layer) = &app.canvas.pending_layer {
        let tl = app.to_screen(layer.image_view.origin.0, layer.image_view.origin.1);
        let image_rect = presented_image_rect(tl, &layer.image_view, zoom, ppp, false);
        painter.image(layer.texture.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// Bypass egui's delayed wheel smoothing for zoom; it would otherwise apply the
/// same raw wheel event again in later frames. Keep explicit pinch/gesture zoom.
fn canvas_zoom_events(input: &egui::InputState) -> Vec<f64> {
    if let Some(touch) = input.multi_touch() {
        return canvas_zoom_delta(touch.zoom_delta).into_iter().collect();
    }
    let mut factors = Vec::new();
    for event in &input.raw.events {
        match event {
            egui::Event::Zoom(factor) => factors.extend(canvas_zoom_delta(*factor)),
            egui::Event::MouseWheel { unit, delta, modifiers, .. } if modifiers.ctrl || modifiers.command || modifiers.mac_cmd => {
                let factor = if *unit == egui::MouseWheelUnit::Point && delta.y != 0.0 && cfg!(any(target_os = "macos", target_arch = "wasm32")) {
                    measured_wheel_zoom(delta.y, *modifiers)
                } else {
                    // Line/page wheel and other platforms have not been measured against the
                    // browser recipe; retain the old gain without egui's delayed smoothing.
                    let points = match unit {
                        egui::MouseWheelUnit::Point => delta.x + delta.y,
                        egui::MouseWheelUnit::Line => (delta.x + delta.y) * if cfg!(target_arch = "wasm32") { 8.0 } else { 40.0 },
                        egui::MouseWheelUnit::Page => (delta.x + delta.y) * input.viewport_rect().height(),
                    };
                    canvas_zoom_delta((points / 200.0).exp())
                };
                factors.extend(factor);
            }
            _ => {}
        }
    }
    factors
}

pub fn show(app: &mut LizardApp, ui: &mut Ui, worker: &mut Worker) {
    let rect = ui.max_rect();
    app.canvas.rect = rect;
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let resp = ui.interact(rect, ui.id().with("canvas"), Sense::click_and_drag());
    let key_box = app.ui.key_box;
    crate::a11y::canvas(&mut app.ui.nav, &resp, || key_box.map_or_else(|| describe(&app.session), describe_box));
    if let Ok(ds) = app.session.ds_mut()
        && ((ds.view.vw - f64::from(rect.width())).abs() > 0.5 || (ds.view.vh - f64::from(rect.height())).abs() > 0.5)
    {
        // Keep the centre; the first sizing after opening re-fits.
        let first = ds.view.vw == 1200.0 && ds.view.vh == 800.0;
        ds.view.vw = f64::from(rect.width());
        ds.view.vh = f64::from(rect.height());
        if first {
            ds.zoom_to_fit();
        }
    }
    let own_cursor = input(app, ui, &resp);
    // A browser names no dragged file until the drop, so documents there still say "import".
    let documents = ctx.input(|i| i.raw.hovered_files.iter().all(|f| f.path.as_deref().is_some_and(crate::is_document)));
    crate::drop_overlay(&ctx, ctx.content_rect(), if documents { tr("Drop to open") } else { tr("Drop to import") });
    crate::guides::end_released(app, &ctx);
    crate::live::track_pointer(app, &ctx);
    app.sync_preedit();
    // Retained rendering snaps the view outward and adds one pixel on each axis.
    let max_texture_side = (ctx.input(|i| i.max_texture_side).min(16_384) as u32).saturating_sub(1).max(1);
    // Every finished render is shown, even when a newer one is already on its way: renders finish
    // in order, and an edit every frame (a drag) would otherwise never show until it stopped.
    let shown = app.session.ds().map(|ds| (ds.uid, ds.page)).ok();
    if let Some(done) = worker.poll() {
        apply_render(app, &ctx, done, shown);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        poll_drag_visual(app, &ctx);
        if !app.canvas.dragging && app.session.ds().ok().is_some_and(|ds| app.canvas.texture_current(ds)) {
            if !drag_visual_matches_idle_document(app) {
                app.canvas.drag_visual = None;
            }
            // Render the selected top-level frame before its first pointer move. On large Retina
            // viewports this avoids a noticeable blank interval while the drag layer is built.
            if app.canvas.drag_visual.is_none() {
                start_drag_visual(app, &ctx);
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let composing_drag = drag_visual_delta(app).is_some();
    #[cfg(target_arch = "wasm32")]
    let composing_drag = false;
    // Observe the latest completed texture before submitting the next document preview. During a
    // fast drag this lets the worker produce a small patch against the frame actually on screen.
    let fast_drag = app.canvas.dragging && !composing_drag && !worker.in_flight() && (0.0..=10.0).contains(&app.ui.render_ms);
    let t = Tokens::get(&ctx);
    let bg = app
        .session
        .ds()
        .ok()
        .and_then(|ds| match ds.doc.node(ds.page).map(|n| &n.kind) {
            Some(NodeKind::Page { background, background_visible: true, .. }) => Some(doc_c32(&ctx, *background)),
            _ => None,
        })
        .unwrap_or(t.canvas);
    crate::theme::set_canvas_background(&ctx, bg);
    // The GPU backend can satisfy the view without calling `request_render`. Keep its camera and
    // texture dimensions current when the window resizes, even if the CPU worker stays idle.
    app.canvas.visible_view = app.session.ds().ok().map(|ds| visible_view_for(&ds.view, rect, ppp));
    let motion_doc = layout_motion_document(app, &ctx);
    update_layout_ghost(app, &ctx);
    // Ask the shared-device renderer first. A resident GPU texture already contains this camera
    // and document; scheduling the CPU worker as well duplicates the most expensive work.
    let mut gpu_source_view = None;
    let mut gpu_tiles = Vec::new();
    let gpu_texture = if !composing_drag
        && shown.is_some()
        && let Ok(ds) = app.session.ds()
        && let Some(view) = app.canvas.visible_view.clone()
        && let Some(mut backend) = app.gpu_canvas.take()
    {
        // The IME composition lives in a stable transient document until the next edit. Render
        // that document on the GPU as well, keeping the visible caret/underline as egui overlays.
        let gpu_doc =
            app.preedit_view().map(|preedit| Arc::clone(&preedit.doc)).or_else(|| motion_doc.clone()).unwrap_or_else(|| Arc::clone(&ds.doc));
        let animation_time = if !app.reduce_motion() && !ds.view.outlines && animated_shader_visible(&gpu_doc, ds.page, &view) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            Some((app.canvas.shader_started.elapsed().as_secs_f64() * 60.0).floor() / 60.0)
        } else {
            None
        };
        backend.set_animation_time(animation_time);
        let clear = [f32::from(bg.r()) / 255.0, f32::from(bg.g()) / 255.0, f32::from(bg.b()) / 255.0];
        let texture = if let Some(focus) = ds.focused() {
            if ds.view.outlines {
                let options = lizard_render::Outline {
                    hidden: ds.view.outline_hidden,
                    bounds: ds.view.outline_bounds,
                    light: ds.doc.node(ds.page).is_some_and(
                        |node| matches!(&node.kind, NodeKind::Page { background, background_visible: true, .. } if background.is_dark()),
                    ),
                };
                backend.focused_outline_texture(gpu_doc, ds.page, focus, &view, clear, options, ds.view.pixel_preview_scale().is_some(), &ctx)
            } else if ds.view.outlines_selection {
                let options = lizard_render::Outline { hidden: ds.view.outline_hidden, bounds: ds.view.outline_bounds, light: false };
                backend.focused_selected_outline_texture(
                    gpu_doc,
                    ds.page,
                    focus,
                    &ds.selection,
                    &view,
                    clear,
                    options,
                    ds.view.proof_srgb,
                    ds.view.pixel_preview_scale().is_some(),
                )
            } else if ds.view.pixel_preview_scale().is_some() {
                backend.focused_pixel_texture(gpu_doc, ds.page, focus, &view, clear, ds.view.proof_srgb, &ctx)
            } else {
                backend.focus_texture(gpu_doc, ds.page, focus, &view, clear, ds.view.proof_srgb, &ctx)
            }
        } else if ds.view.outlines {
            let options = lizard_render::Outline {
                hidden: ds.view.outline_hidden,
                bounds: ds.view.outline_bounds,
                light: ds
                    .doc
                    .node(ds.page)
                    .is_some_and(|node| matches!(&node.kind, NodeKind::Page { background, background_visible: true, .. } if background.is_dark())),
            };
            backend.outline_texture(gpu_doc, ds.page, &view, clear, options, ds.view.pixel_preview_scale().is_some(), &ctx)
        } else if ds.view.outlines_selection {
            let options = lizard_render::Outline { hidden: ds.view.outline_hidden, bounds: ds.view.outline_bounds, light: false };
            if ds.view.pixel_preview_scale().is_some() {
                backend.selected_outline_pixel_texture(gpu_doc, ds.page, &ds.selection, &view, clear, options, ds.view.proof_srgb, &ctx)
            } else {
                backend.selected_outline_texture(gpu_doc, ds.page, &ds.selection, &view, clear, options, ds.view.proof_srgb, &ctx)
            }
        } else if ds.view.pixel_preview_scale().is_some() {
            backend.pixel_texture(gpu_doc, ds.page, &view, clear, ds.view.proof_srgb, &ctx)
        } else {
            backend.texture(gpu_doc, ds.page, &view, clear, app.canvas.dragging || app.canvas.middle_pan.is_some(), ds.view.proof_srgb, &ctx)
        };
        if ds.focused().is_none() && !ds.view.outlines && !ds.view.outlines_selection && ds.view.pixel_preview_scale().is_none() {
            gpu_source_view = backend.source_view();
            if texture.is_some() {
                gpu_tiles = backend.presented_tiles();
            }
        }
        app.gpu_canvas = Some(backend);
        texture
    } else {
        None
    };
    app.canvas.gpu_presented = gpu_texture.is_some();
    if (!app.canvas.dragging || !composing_drag) && gpu_texture.is_none() {
        request_render(app, worker, &ctx, ppp, max_texture_side, motion_doc.clone());
    }
    if fast_drag
        && gpu_texture.is_none()
        && worker.in_flight()
        && let Some(done) = worker.poll_for(std::time::Duration::from_millis(10))
    {
        apply_render(app, &ctx, done, shown);
    }
    let painter = ui.painter_at(rect);
    // Page background (also behind a stale/partial texture).
    painter.rect_filled(rect, CornerRadius::ZERO, bg);
    #[cfg(not(target_arch = "wasm32"))]
    let painted_drag = paint_drag_visual(app, &painter);
    #[cfg(target_arch = "wasm32")]
    let painted_drag = false;
    if !painted_drag
        && let Some(fallback) = &app.canvas.zoom_fallback
        && let Ok(ds) = app.session.ds()
        && fallback.of == (ds.uid, ds.page)
        && Arc::ptr_eq(&fallback.doc, &ds.doc)
        && app.canvas.tex_style.as_ref() == Some(&fallback.style)
        && !app.canvas.tex_view.as_ref().zip(app.canvas.visible_view.as_ref()).is_some_and(|(current, visible)| {
            app.canvas.tex_of == Some((ds.uid, ds.page))
                && app.canvas.tex_doc.as_ref().is_some_and(|doc| Arc::ptr_eq(doc, &ds.doc))
                && covers(current, visible)
        })
    {
        let zoom = ds.view.zoom;
        if let Some(texture) = &fallback.texture {
            let tl = app.to_screen(fallback.view.origin.0, fallback.view.origin.1);
            let image_rect = presented_image_rect(tl, &fallback.view, zoom, ppp, false);
            painter.image(texture.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        for tile in &fallback.tiles {
            let tl = app.to_screen(tile.view.origin.0, tile.view.origin.1);
            let image_rect = presented_image_rect(tl, &tile.view, zoom, ppp, false);
            painter.image(tile.texture.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    if let (Some(tex), Some(v), true) = (&app.canvas.texture, &app.canvas.tex_view, !painted_drag && shown.is_some() && app.canvas.tex_of == shown) {
        // Re-project the last render onto the current view.
        let tl = app.to_screen(v.origin.0, v.origin.1);
        let zoom = app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0);
        let pixel_preview = app.session.ds().ok().and_then(|ds| ds.view.pixel_preview_scale()) == Some(v.scale);
        let image_rect = presented_image_rect(tl, v, zoom, ppp, pixel_preview);
        painter.image(tex.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    if !painted_drag && shown.is_some() && app.canvas.tex_of == shown {
        let zoom = app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0);
        for tile in &app.canvas.tiles {
            let tl = app.to_screen(tile.view.origin.0, tile.view.origin.1);
            let image_rect = presented_image_rect(tl, &tile.view, zoom, ppp, false);
            painter.image(tile.texture.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    if !painted_drag && gpu_texture.is_some() && !gpu_tiles.is_empty() {
        let zoom = app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0);
        for (texture, view) in gpu_tiles {
            let tl = app.to_screen(view.origin.0, view.origin.1);
            let image_rect = presented_image_rect(tl, &view, zoom, ppp, false);
            painter.with_clip_rect(image_rect.shrink(16.0 / ppp)).image(
                texture,
                image_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    } else if !painted_drag && let Some(texture) = gpu_texture {
        let image_rect = app
            .session
            .ds()
            .ok()
            .and_then(|ds| {
                let view = gpu_source_view.as_ref().or(app.canvas.visible_view.as_ref())?;
                if ds.view.pixel_preview_scale().is_some() {
                    Some(rect)
                } else {
                    let tl = app.to_screen(view.origin.0, view.origin.1);
                    Some(presented_image_rect(tl, view, ds.view.zoom, ppp, false))
                }
            })
            .unwrap_or(rect);
        painter.image(texture, image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    if !painted_drag {
        paint_pending_layer(app, &painter, &ctx, ppp);
    }
    paint_layout_ghost(app, &painter);
    #[cfg(not(target_arch = "wasm32"))]
    crate::image_vectorize::paint_preview(app, &painter, bg);
    app.session.tool.labels = overlays(app, &painter, &t, motion_doc.as_deref());
    crate::frame_labels::rename_field(app, ui);
    // The zoom tool's area.
    if let (Some(start), Some(p)) = (app.canvas.zoom_drag, ctx.input(|i| i.pointer.hover_pos())) {
        let area = Rect::from_two_pos(start, p);
        if area.width() > 4.0 || area.height() > 4.0 {
            painter.rect_filled(area, CornerRadius::ZERO, accent(&painter).gamma_multiply(0.1));
            painter.rect_stroke(area, CornerRadius::ZERO, Stroke::new(1.0, accent(&painter)), StrokeKind::Inside);
        }
    }
    crate::dev_mode::draw(app, &painter);
    crate::handles::draw(app, &painter);
    crate::selection_spacing::draw(app, &painter);
    crate::comments::draw(app, &painter);
    crate::live_text::draw(app, &painter);
    crate::live::draw(app, &painter, &ctx);
    crate::agent_presence::draw(app, &painter, &ctx);
    crate::live::chat_box(app, &ctx);
    crate::live::spotlight_banner(app, &ctx, rect);
    crate::comments::popovers(app, &ctx);
    crate::versions::preview_banner(app, &ctx, rect);
    crate::branch_ui::banner(app, &ctx, rect);
    crate::dev_mode::focus_bar(app, &ctx, rect);
    crate::widget_ui::menu_bar(app, ui, rect);
    crate::widget_ui::pointer(app, &resp);
    eyedropper_preview(app, &ctx);
    // The cursor: the tool's, unless something on the canvas with its own has the pointer.
    app.canvas.cursor = None;
    if own_cursor && resp.hovered() && ctx.input(|i| i.pointer.hover_pos()).is_some() {
        let panning = app.canvas.space_pan || app.canvas.middle_pan.is_some();
        let shown = crate::cursors::on_canvas(panning, ctx.input(|i| i.pointer.any_down()), app.canvas.zoom_tool, app.session.tool.overlay.cursor);
        shown.show(&ctx, &mut app.canvas.cursor_textures);
        app.canvas.cursor = Some(shown);
    }
}

/// The eyedropper's colour and pixel magnifier follow the pointer's lower-right corner.
fn eyedropper_preview(app: &mut LizardApp, ctx: &egui::Context) {
    if app.session.tool.tool != ToolId::Eyedropper {
        app.canvas.eyedrop_sample = None;
        return;
    }
    let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()).filter(|p| app.canvas.rect.contains(*p)) else { return };
    let point = app.to_page(pointer);
    let refresh = app
        .canvas
        .eyedrop_sample
        .as_ref()
        .is_none_or(|(at, page, when, _)| (*at != pointer || *page != point) && when.elapsed() >= std::time::Duration::from_millis(16));
    if refresh {
        let gpu = if app.canvas.gpu_presented {
            app.gpu_canvas.as_mut().map_or(GpuSample::Unavailable, |backend| backend.sample_eyedropper(point, ctx))
        } else {
            GpuSample::Unavailable
        };
        let pixels = match gpu {
            GpuSample::Ready(pixels) => Some(pixels),
            GpuSample::Pending => None,
            GpuSample::Unavailable => app.session.ds().ok().and_then(|ds| {
                let scale = ds.view.zoom.clamp(0.01, 64.0);
                let view = lizard_render::View { origin: (point.x - 4.0 / scale, point.y - 4.0 / scale), scale, width: 9, height: 9 };
                lizard_render::render_view(&ds.doc, ds.page, &view, &Default::default()).map(|pix| lizard_render::to_rgba8(&pix))
            }),
        };
        if let Some(pixels) = pixels {
            app.canvas.eyedrop_sample = Some((pointer, point, web_time::Instant::now(), pixels));
        }
    }
    let Some((_, _, _, pixels)) = &app.canvas.eyedrop_sample else { return };
    let center = pixels.get(160..164).unwrap_or(&[0, 0, 0, 255]);
    let color = lizard_color::Color::from_rgba8(center[0], center[1], center[2], 255);
    let screen = ctx.content_rect();
    let x = (pointer.x + 18.0).min(screen.max.x - 357.0).max(screen.min.x + 8.0);
    let y = (pointer.y + 18.0).min(screen.max.y - 95.0).max(screen.min.y + 8.0);
    let pixels = pixels.clone();
    egui::Area::new(egui::Id::new("eyedropper_preview")).order(egui::Order::Foreground).fixed_pos(pos2(x, y)).show(ctx, |ui| {
        let (panel, _) = ui.allocate_exact_size(vec2(349.0, 87.0), Sense::hover());
        ui.painter().rect_filled(panel, CornerRadius::same(8), Color32::from_rgb(46, 46, 46));
        ui.painter().rect_stroke(panel, CornerRadius::same(8), Stroke::new(1.0, Color32::from_rgb(65, 65, 65)), StrokeKind::Inside);
        let preview = Rect::from_min_size(panel.min + vec2(11.0, 11.0), vec2(65.0, 65.0));
        ui.painter().rect_filled(preview, CornerRadius::same(4), Color32::BLACK);
        for row in 0..9usize {
            for col in 0..9usize {
                let p = (row * 9 + col) * 4;
                if let Some(rgb) = pixels.get(p..p + 3) {
                    let r = Rect::from_min_size(
                        preview.min + vec2(col as f32 * 65.0 / 9.0, row as f32 * 65.0 / 9.0),
                        vec2(65.0 / 9.0 + 0.5, 65.0 / 9.0 + 0.5),
                    );
                    ui.painter().rect_filled(r.intersect(preview), 0.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                }
            }
        }
        ui.painter().rect_stroke(
            Rect::from_min_size(preview.min + vec2(4.0 * 65.0 / 9.0, 4.0 * 65.0 / 9.0), vec2(65.0 / 9.0, 65.0 / 9.0)),
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Inside,
        );
        for n in 1..9 {
            let line = n as f32 * 65.0 / 9.0;
            ui.painter().vline(preview.min.x + line, preview.min.y..=preview.max.y, Stroke::new(0.5, Color32::from_black_alpha(55)));
            ui.painter().hline(preview.min.x..=preview.max.x, preview.min.y + line, Stroke::new(0.5, Color32::from_black_alpha(55)));
        }
        let swatch = Rect::from_min_size(panel.min + vec2(92.0, 15.0), vec2(21.0, 21.0));
        ui.painter().rect_filled(swatch, CornerRadius::same(3), Color32::from_rgb(center[0], center[1], center[2]));
        ui.painter().text(panel.min + vec2(120.0, 25.0), Align2::LEFT_CENTER, color.to_css_hex(), semibold(13.0), Color32::from_rgb(220, 220, 220));
        let create = Rect::from_min_size(panel.min + vec2(92.0, 44.0), vec2(155.0, 28.0));
        let response = ui.interact(create, egui::Id::new("eyedrop_create_variable"), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(create, CornerRadius::same(5), Color32::from_rgb(64, 64, 64));
        }
        crate::icons::paint_panel_reference(
            ui.painter(),
            Rect::from_center_size(pos2(create.min.x + 12.0, create.center().y), vec2(14.0, 14.0)),
            49,
            Color32::WHITE,
        );
        ui.painter().text(pos2(create.min.x + 28.0, create.center().y), Align2::LEFT_CENTER, tr("Create variable"), semibold(13.0), Color32::WHITE);
        let mut right = panel.min.x + 340.0;
        for label in ["⌘", "⇧"] {
            right = crate::primitives::Shortcut::new(label).ink(Color32::WHITE).paint_right(ui, right, create.center().y) - 5.0;
        }
        if response.clicked() {
            crate::colorpicker::open_creation_from_eyedropper(ctx, color);
        }
    });
}

fn apply_render(app: &mut LizardApp, ctx: &egui::Context, done: Done, shown: Option<(u64, NodeId)>) {
    // A paste followed by Delete can finish while an older effect render is still running.
    // Never put that older document back on screen after a discrete edit. During a live drag,
    // intermediate snapshots still provide useful visual feedback until the newest one arrives.
    if !app.canvas.dragging && !app.session.ds().ok().is_some_and(|ds| Arc::ptr_eq(&ds.doc, &done.doc)) {
        return;
    }
    if shown != Some((done.uid, done.page)) || !shows(&app.canvas, &done, shown) {
        return;
    }
    // A pixel preview render is magnified with hard pixel edges.
    let filter = if app.session.ds().ok().and_then(|d| d.view.pixel_preview_scale()) == Some(done.view.scale) {
        egui::TextureOptions::NEAREST
    } else {
        egui::TextureOptions::LINEAR
    };
    let same_view = app.canvas.tex_view.as_ref().is_some_and(|v| {
        v.origin.0.to_bits() == done.view.origin.0.to_bits()
            && v.origin.1.to_bits() == done.view.origin.1.to_bits()
            && v.scale.to_bits() == done.view.scale.to_bits()
            && v.width == done.view.width
            && v.height == done.view.height
    });
    let can_patch = !done.coarse
        && !app.canvas.tex_coarse
        && same_view
        && app.canvas.tex_style.as_ref() == Some(&done.style)
        && app.canvas.tex_of == Some((done.uid, done.page))
        && app.canvas.tex_doc.as_ref().is_some_and(|shown| done.base_docs.iter().any(|base| Arc::ptr_eq(base, shown)));
    // Keep the broadest refined view across a zoom-in. Once zooming back out, its pixels fill
    // the margins immediately while the worker renders the new view. A texture handle must be
    // moved, not cloned: `set` would otherwise mutate the cached image as well.
    if matches!(done.update, TextureUpdate::Full(_) | TextureUpdate::Tiles(_)) && !app.canvas.tex_coarse && !same_view {
        let previous = app.canvas.tex_view.as_ref().zip(app.canvas.tex_doc.as_ref()).zip(app.canvas.tex_style.as_ref());
        if let Some(((view, doc), style)) = previous
            && app.canvas.tex_of == Some((done.uid, done.page))
            && Arc::ptr_eq(doc, &done.doc)
            && style == &done.style
            && view_area(view) > view_area(&done.view) * 1.25
            && app.canvas.zoom_fallback.as_ref().is_none_or(|old| view_area(view) > view_area(&old.view) * 1.1)
            && (app.canvas.texture.is_some() || !app.canvas.tiles.is_empty())
        {
            app.canvas.zoom_fallback = Some(ZoomFallback {
                texture: app.canvas.texture.take(),
                tiles: std::mem::take(&mut app.canvas.tiles),
                view: view.clone(),
                of: (done.uid, done.page),
                doc: doc.clone(),
                style: style.clone(),
            });
        }
    }
    if app.canvas.zoom_fallback.as_ref().is_some_and(|old| old.of != (done.uid, done.page) || !Arc::ptr_eq(&old.doc, &done.doc)) {
        app.canvas.zoom_fallback = None;
    }
    let applied = match (&mut app.canvas.texture, done.update) {
        (Some(t), TextureUpdate::Partial(patches)) if can_patch => {
            app.canvas.upload_pixels = patches.iter().map(|(_, image)| image.size.iter().map(|n| *n as u64).product::<u64>()).sum();
            for (at, image) in patches {
                t.set_partial(at, image, filter);
            }
            true
        }
        (Some(_), TextureUpdate::Reused) if can_patch => {
            app.canvas.upload_pixels = 0;
            true
        }
        (Some(t), TextureUpdate::Full(image)) => {
            app.canvas.tiles.clear();
            app.canvas.upload_pixels = image.size.iter().map(|n| *n as u64).product();
            t.set(image, filter);
            true
        }
        (slot @ None, TextureUpdate::Full(image)) => {
            app.canvas.tiles.clear();
            app.canvas.upload_pixels = image.size.iter().map(|n| *n as u64).product();
            *slot = Some(ctx.load_texture("lizard-canvas", image, filter));
            true
        }
        (slot, TextureUpdate::Tiles(tiles)) => {
            *slot = None;
            let mut old = std::mem::take(&mut app.canvas.tiles);
            let mut next = Vec::with_capacity(tiles.len());
            let mut uploaded = 0;
            for tile in tiles {
                if let Some(index) = old.iter().position(|cached| {
                    cached.view.origin == tile.view.origin
                        && cached.view.scale.to_bits() == tile.view.scale.to_bits()
                        && Arc::ptr_eq(&cached.image, &tile.image)
                }) {
                    next.push(old.swap_remove(index));
                } else {
                    uploaded += u64::from(tile.view.width) * u64::from(tile.view.height);
                    let texture = ctx.load_texture("lizard-canvas-tile", (*tile.image).clone(), filter);
                    next.push(CanvasTile { view: tile.view, image: tile.image, texture });
                }
            }
            app.canvas.tiles = next;
            app.canvas.upload_pixels = uploaded;
            true
        }
        _ => false,
    };
    if applied {
        if !done.coarse {
            app.canvas.render_work = Some(done.work);
            app.ui.render_ms = done.ms;
        }
        let latency = Some(done.at.elapsed().as_secs_f64() * 1000.0);
        if done.coarse {
            app.canvas.preview_latency_ms = latency;
        } else {
            app.canvas.refine_latency_ms = latency;
        }
        app.canvas.tex_view = Some(done.view);
        app.canvas.tex_of = Some((done.uid, done.page));
        app.canvas.tex_doc = Some(done.doc);
        app.canvas.tex_style = Some(done.style);
        app.canvas.tex_coarse = done.coarse;
    } else {
        // A result that depended on a skipped base cannot update the visible texture.
        // Request a new full image against the texture that is actually on screen.
        app.canvas.submitted = None;
        ctx.request_repaint();
    }
}

/// The image's screen rect; align native-density renders to device pixels to avoid linear filtering
/// blending neighbouring texels. Stale and pixel-preview renders retain their original projection.
pub(super) fn presented_image_rect(tl: Pos2, view: &lizard_render::View, zoom: f64, ppp: f32, pixel_preview: bool) -> Rect {
    let w = (f64::from(view.width) / view.scale * zoom) as f32;
    let h = (f64::from(view.height) / view.scale * zoom) as f32;
    let native_density = !pixel_preview
        && ppp.is_finite()
        && ppp > 0.0
        && zoom.is_finite()
        && view.scale.is_finite()
        && view.scale > 0.0
        && (view.scale - zoom * f64::from(ppp)).abs() <= view.scale.abs().max(1.0) * 1e-9;
    let min = if native_density { pos2((tl.x * ppp).round() / ppp, (tl.y * ppp).round() / ppp) } else { tl };
    Rect::from_min_size(min, vec2(w, h))
}

/// What the canvas says to screen readers: the selected layer ("Rectangle 1, Rectangle, 120 by 80,
/// selected"), how many are selected, or the page with nothing selected.
pub fn describe(s: &lizard_engine::Session) -> String {
    let Ok(ds) = s.ds() else { return String::new() };
    match ds.selection.as_slice() {
        [] => trf("{page}, nothing selected", &[("page", &ds.doc.node(ds.page).map(|p| p.name.as_str()).unwrap_or(tr("Page")))]),
        [id] => match ds.doc.node(*id) {
            Some(n) => {
                let kind = tr(n.kind.label());
                let name = if n.name.is_empty() { kind } else { n.name.as_str() };
                let (w, h) = (crate::widgets::fmt_num(n.width), crate::widgets::fmt_num(n.height));
                trf("{name}, {kind}, {w} by {h}, selected", &[("name", &name), ("kind", &kind), ("w", &w), ("h", &h)])
            }
            None => String::new(),
        },
        many => trn("{n} layers selected", many.len(), &[]),
    }
}

/// What the canvas says while a keyboard selection box shows: its size and place, and the keys.
fn describe_box([x0, y0, x1, y1]: [f64; 4]) -> String {
    let n = crate::widgets::fmt_num;
    let (w, h, x, y) = (n((x1 - x0).abs()), n((y1 - y0).abs()), n(x0.min(x1)), n(y0.min(y1)));
    let cmd = crate::toolbar::pretty_shortcut("Cmd");
    trf(
        "Selection box, {w} by {h} at {x}, {y}: arrows move it, Shift+arrows or {cmd}+arrows size it, Enter selects, Escape cancels",
        &[("w", &w), ("h", &h), ("x", &x), ("y", &y), ("cmd", &cmd)],
    )
}

/// Whether a finished render goes on screen: an image of the file and page shown (`shown`), and
/// not a coarse preview over a refined image of the same document that already shows the whole
/// view asked for at its zoom (the same zoom elsewhere on the page is no such image).
fn shows(c: &CanvasState, done: &Done, shown: Option<(u64, NodeId)>) -> bool {
    if shown != Some((done.uid, done.page)) {
        return false;
    }
    // A newer zoom-out preview may already cover the whole requested view. A full render from
    // the previous zoom can finish afterward; showing it would expose empty strips again.
    if c.tex_of == shown && c.tex_view.as_ref().zip(c.want_view.as_ref()).is_some_and(|(have, want)| covers(have, want) && !covers(&done.view, want))
    {
        return false;
    }
    let refined_here = !c.tex_coarse
        && c.tex_of == shown
        && c.tex_doc.as_ref().is_some_and(|doc| Arc::ptr_eq(doc, &done.doc))
        && c.tex_view
            .as_ref()
            .zip(c.want_view.as_ref())
            .is_some_and(|(have, want)| have.scale.to_bits() == want.scale.to_bits() && covers(have, want));
    !(done.coarse && refined_here)
}

/// Whether `outer` shows all of the page that `inner` does.
fn covers(outer: &lizard_render::View, inner: &lizard_render::View) -> bool {
    let (o, i) = (outer.page_rect(), inner.page_rect());
    // Rounding in the snapped origin, well under a device pixel.
    let eps = 1e-6 / inner.scale.max(1e-9);
    o.x0 <= i.x0 + eps && o.y0 <= i.y0 + eps && o.x1 >= i.x1 - eps && o.y1 >= i.y1 - eps
}

fn visible_view_for(view: &lizard_engine::ViewState, rect: Rect, ppp: f32) -> lizard_render::View {
    let (mut ox, mut oy) = view.origin();
    let (scale, width, height) = if let Some(k) = view.pixel_preview_scale() {
        // Pixel preview renders whole page pixels and magnifies them without smoothing.
        (ox, oy) = ((ox * k).floor() / k, (oy * k).floor() / k);
        let z = view.zoom.max(1e-9) / k;
        (k, (f64::from(rect.width()) / z).ceil() as u32 + 2, (f64::from(rect.height()) / z).ceil() as u32 + 2)
    } else {
        (view.zoom * f64::from(ppp), (rect.width() * ppp).ceil().max(1.0) as u32, (rect.height() * ppp).ceil().max(1.0) as u32)
    };
    lizard_render::View { origin: (ox, oy), scale, width, height }
}

#[cfg(test)]
mod visible_view_tests {
    use super::*;

    #[test]
    fn resizing_changes_gpu_view_dimensions_without_a_cpu_render() {
        let mut view = lizard_engine::ViewState { zoom: 2.0, ..Default::default() };
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0));
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 400.0));
        let before = visible_view_for(&view, small, 2.0);
        view.vw = f64::from(wide.width());
        view.vh = f64::from(wide.height());
        let after = visible_view_for(&view, wide, 2.0);
        assert_eq!((before.width, before.height), (1200, 800));
        assert_eq!((after.width, after.height), (2400, 800));
        assert_eq!(after.scale, before.scale);
    }
}

fn request_render(app: &mut LizardApp, worker: &mut Worker, ctx: &egui::Context, ppp: f32, max_texture_side: u32, motion_doc: Option<Arc<Document>>) {
    let Ok(ds) = app.session.ds() else { return };
    let visible_view = visible_view_for(&ds.view, app.canvas.rect, ppp);
    let (ox, oy) = visible_view.origin;
    let (scale, visible_w, visible_h) = (visible_view.scale, visible_view.width, visible_view.height);
    // The edited text is drawn live (an IME composition included); the caret is an overlay.
    let doc = app.preedit_view().map(|p| p.doc.clone()).or(motion_doc).unwrap_or_else(|| ds.doc.clone());
    let outlined = if ds.view.outlines_selection { ds.selection.clone() } else { vec![] };
    let proof = ds.view.proof_srgb && !doc.color_profile.is_srgb();
    let outline_extras = lizard_render::Outline { hidden: ds.view.outline_hidden, bounds: ds.view.outline_bounds, light: false };
    // Render beyond the visible viewport. During a pan the previous texture then still covers
    // the screen while the worker renders newly exposed content. Request a new buffer before the
    // visible area reaches its edge, rather than exposing an empty strip each pointer move.
    let margin = overscan_margin(visible_w, visible_h, max_texture_side);
    let origin = (ox, oy);
    let visible = (visible_w, visible_h);
    let animation_frame = if !app.reduce_motion() && !ds.view.outlines && animated_shader_visible(&doc, ds.page, &visible_view) {
        ctx.request_repaint_after(std::time::Duration::from_millis(67));
        (app.canvas.shader_started.elapsed().as_secs_f64() * 15.0).floor() as u64
    } else {
        0
    };
    let animation_time = animation_frame as f64 / 15.0;
    let requested = (ox - f64::from(margin) / scale, oy - f64::from(margin) / scale);
    let size = (visible_w.saturating_add(margin * 2).min(max_texture_side), visible_h.saturating_add(margin * 2).min(max_texture_side));
    let key = JobKey {
        doc: Arc::as_ptr(&doc) as usize,
        page: ds.page,
        origin: ((requested.0 * 1000.0) as i64, (requested.1 * 1000.0) as i64),
        scale: (scale * 10000.0) as i64,
        size,
        outline: ds.view.outlines,
        outline_extras,
        outlined: outlined.clone(),
        skip: vec![],
        solo: ds.focused(),
        fonts: lizard_text::layout_generation(),
        proof,
        animation_frame,
    };
    // A completed request is not a usable cache entry if its result was dropped or could not
    // update the texture. This can happen while several style or paint edits arrive in quick
    // succession. Retry the same view once the worker is idle.
    let texture_ready = app.canvas.texture_current(ds) && app.canvas.tex_view.as_ref().is_some_and(|view| covers(view, &visible_view));
    if !worker.in_flight() && !texture_ready {
        app.canvas.submitted = None;
    }
    if let Some(previous) = &app.canvas.submitted {
        let previous_origin = (previous.origin.0 as f64 / 1000.0, previous.origin.1 as f64 / 1000.0);
        let same_content = JobKey { origin: previous.origin, size: previous.size, ..key.clone() } == *previous;
        let covers = cached_render_covers(previous_origin, previous.size, origin, visible, scale);
        if same_content && covers {
            return;
        }
    }
    if app.canvas.submitted.as_ref() == Some(&key) {
        return;
    }
    let visible_texture = !app.canvas.tex_coarse && app.canvas.tex_of == Some((ds.uid, ds.page));
    let job = Job {
        doc,
        page: ds.page,
        view: lizard_render::View { origin: requested, scale, width: size.0, height: size.1 },
        time: animation_time,
        outline: ds.view.outlines,
        outline_extras,
        outlined,
        skip: vec![],
        solo: key.solo,
        proof,
        uid: ds.uid,
        zoom_preview: ds.view.pixel_preview_scale().is_none()
            && scale >= 4.0
            && ds.doc.nodes().any(|node| node.visible && node.effects.iter().any(|effect| effect.visible)),
        at: web_time::Instant::now(),
        texture_base: if visible_texture { app.canvas.tex_doc.clone() } else { None },
        texture_view: if visible_texture { app.canvas.tex_view.clone() } else { None },
        texture_style: if visible_texture { app.canvas.tex_style.clone() } else { None },
    };
    app.canvas.want_view = Some(job.view.clone());
    // A geometry-only native-resolution preview is prepared on the worker for effect-heavy
    // views, then replaced by the complete render. No raster work runs on the input thread.
    worker.submit(job);
    app.canvas.submitted = Some(key);
}

/// Leave a 96-pixel reserve so the next render starts before a pan reaches a cached edge.
fn cached_render_covers(buffer_origin: (f64, f64), buffer_size: (u32, u32), view_origin: (f64, f64), view_size: (u32, u32), scale: f64) -> bool {
    if !scale.is_finite() || scale <= 0.0 {
        return false;
    }
    let guard_x = 96.0_f64.min(f64::from(buffer_size.0.saturating_sub(view_size.0)) / 4.0) / scale;
    let guard_y = 96.0_f64.min(f64::from(buffer_size.1.saturating_sub(view_size.1)) / 4.0) / scale;
    view_origin.0 >= buffer_origin.0 + guard_x
        && view_origin.1 >= buffer_origin.1 + guard_y
        && view_origin.0 + f64::from(view_size.0) / scale <= buffer_origin.0 + f64::from(buffer_size.0) / scale - guard_x
        && view_origin.1 + f64::from(view_size.1) / scale <= buffer_origin.1 + f64::from(buffer_size.1) / scale - guard_y
}

/// Bound the extra rendered area to about 1.6 times the visible area. A fixed large margin can
/// multiply the pixel count several-fold on a narrow or small viewport.
fn overscan_margin(width: u32, height: u32, max_texture_side: u32) -> u32 {
    width.min(height).saturating_div(8).min(128).min(max_texture_side.saturating_sub(width.max(height)) / 2)
}

#[cfg(test)]
#[test]
fn pan_uses_cached_pixels_until_the_refill_boundary() {
    let margin = overscan_margin(1000, 800, 16_383);
    assert_eq!(margin, 100);
    let buffer = (-f64::from(margin), -f64::from(margin));
    let size = (1000 + 2 * margin, 800 + 2 * margin);
    assert!(cached_render_covers(buffer, size, (0.0, 0.0), (1000, 800), 1.0));
    assert!(cached_render_covers(buffer, size, (50.0, 0.0), (1000, 800), 1.0));
    assert!(!cached_render_covers(buffer, size, (80.0, 0.0), (1000, 800), 1.0));
    assert!(!cached_render_covers(buffer, size, (-80.0, 0.0), (1000, 800), 1.0));
    assert!(cached_render_covers((-24.0, -24.0), (1048, 848), (10.0, 0.0), (1000, 800), 1.0));
    assert!(!cached_render_covers((-24.0, -24.0), (1048, 848), (19.0, 0.0), (1000, 800), 1.0));
}

#[cfg(test)]
#[test]
fn overscan_stays_within_texture_and_pixel_budget() {
    for (width, height, limit) in [(1000, 800, 16_383), (390, 844, 16_383), (2560, 1440, 16_383), (1000, 800, 1060)] {
        let margin = overscan_margin(width, height, limit);
        assert!(width + 2 * margin <= limit || width > limit);
        assert!(height + 2 * margin <= limit || height > limit);
        let rendered = u64::from(width + 2 * margin) * u64::from(height + 2 * margin);
        let visible = u64::from(width) * u64::from(height);
        assert!(rendered * 5 <= visible * 8); // At most 1.6× the visible pixels.
    }
}

/// The zoom tool while Z is held (Alt: zoom out): a click zooms a step about the pointer, a drag
/// zooms to the dragged area. Takes the pointer from the other tools while it's active.
fn zoom_tool(app: &mut LizardApp, ui: &Ui, resp: &egui::Response, pos: Pos2, mods: egui::Modifiers, typing: bool) -> bool {
    let held = !typing && !mods.command && ui.input(|i| i.key_down(egui::Key::Z));
    app.canvas.zoom_tool = held.then_some(mods.alt);
    let Some(start) = app.canvas.zoom_drag.or_else(|| (held && resp.hovered() && ui.input(|i| i.pointer.primary_pressed())).then_some(pos)) else {
        return held;
    };
    app.canvas.zoom_drag = Some(start);
    let area = Rect::from_two_pos(start, pos);
    if ui.input(|i| i.pointer.primary_down()) {
        return true;
    }
    app.canvas.zoom_drag = None;
    let (a, b) = (app.to_page(area.min), app.to_page(area.max));
    let params = if area.width() > 4.0 && area.height() > 4.0 {
        json!({"x": a.x, "y": a.y, "width": b.x - a.x, "height": b.y - a.y})
    } else {
        let p = app.to_page(pos);
        json!({"x": p.x, "y": p.y, "out": mods.alt})
    };
    app.run("view.zoomAt", params);
    true
}

/// The pointer and keys on the canvas. False when something on it with a cursor of its own has
/// the pointer (a comment pin, a ruler guide, a gradient or padding handle): the tool's cursor
/// then doesn't show.
fn input(app: &mut LizardApp, ui: &mut Ui, resp: &egui::Response) -> bool {
    if crate::proto_canvas::input(app, ui, resp) {
        // A prototype gesture is a press on the canvas too: the Pages list gives up the keyboard.
        app.ui.pages_focus = None;
        return true;
    }
    let ctx = ui.ctx().clone();
    let (mods, scroll, zoom_events, hover_pos, space) =
        ctx.input(|i| (i.modifiers, i.smooth_scroll_delta, canvas_zoom_events(i), i.pointer.hover_pos(), i.key_down(egui::Key::Space)));
    let editing_text = app.session.ds().map(|d| d.text_edit.is_some()).unwrap_or(false);
    let typing_elsewhere = crate::a11y::field_focused(&ctx);
    app.canvas.space_pan = space && !editing_text && !typing_elsewhere;
    if !app.canvas.dragging && crate::selection_spacing::input(app, &ctx, hover_pos, resp.hovered(), app.canvas.space_pan) {
        return false;
    }
    // Dropped files import onto the canvas, behind a card while they do ([`crate::pending_open`]);
    // a dropped document opens as a file of its own, as on the home screen.
    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    for f in dropped {
        let path = f.path().to_path_buf();
        if crate::is_document(&path) {
            crate::pending_open::open_dropped(app, &f);
            continue;
        }
        let pt = hover_pos.map(|h| app.to_page(h)).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped.png".into());
        let (params, read) = if path.exists() {
            (json!({"path": path.to_string_lossy(), "x": pt.x, "y": pt.y}), None)
        } else {
            (json!({"name": name, "x": pt.x, "y": pt.y}), Some(crate::dropped_bytes(&f, &name)))
        };
        let Some(target) =
            app.session.ds().ok().map(|doc| crate::pending_open::ImportTarget { uid: doc.uid, page: doc.page, name: doc.doc.name.clone() })
        else {
            continue;
        };
        app.ui.pending_opens.push(crate::pending_open::PendingOpen { label: name, params, command: "file.import", target: Some(target), read });
    }
    let Some(pos) = hover_pos.filter(|p| app.canvas.rect.contains(*p)) else {
        // A drag released off the canvas, or off the window, ends where the pointer was last seen.
        if app.canvas.dragging && !ctx.input(|i| i.pointer.primary_down()) {
            app.canvas.dragging = false;
            app.canvas.last_drag_input = None;
            if let Some(at) = hover_pos.or_else(|| app.canvas.last_pointer.map(|p| app.to_screen(p.x, p.y))) {
                send(app, PointerKind::Up, at, &mods);
            }
        }
        // Leaving the canvas: nothing is hovered there any more.
        if !app.canvas.dragging && std::mem::take(&mut app.canvas.hovering) {
            app.session.vector_pointer_left();
        }
        // A pan whose button was released off the canvas is over.
        if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle) || i.pointer.primary_down()) {
            app.canvas.middle_pan = None;
        }
        return true;
    };
    // A press on the canvas gives the keyboard back to it (arrows nudge again).
    if resp.hovered() && ctx.input(|i| i.pointer.any_pressed()) {
        app.ui.layers_focus = false;
        app.ui.pages_focus = None;
        app.ui.key_box = None;
    }
    // Zoom (pinch, Cmd/Ctrl+scroll) about the pointer; plain scroll pans.
    if resp.hovered() {
        if !zoom_events.is_empty() {
            let anchor = app.to_page(pos);
            if let Ok(ds) = app.session.ds_mut() {
                for factor in zoom_events {
                    ds.view.set_zoom_at(ds.view.zoom * factor, (anchor.x, anchor.y));
                }
                ds.revision += 1;
                app.canvas.last_drag_input = None;
            }
        } else if scroll != egui::Vec2::ZERO
            && let Ok(ds) = app.session.ds_mut()
        {
            ds.view.scroll(f64::from(scroll.x), f64::from(scroll.y), mods.shift);
            ds.revision += 1;
            app.canvas.last_drag_input = None;
        }
    }
    if zoom_tool(app, ui, resp, pos, mods, editing_text || typing_elsewhere) {
        return true;
    }
    // Middle-button / space panning.
    let middle_down = ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
    if (middle_down || (app.canvas.space_pan && ctx.input(|i| i.pointer.primary_down()))) && resp.hovered() || app.canvas.middle_pan.is_some() {
        if let Ok(ds) = app.session.ds_mut() {
            match app.canvas.middle_pan {
                None => app.canvas.middle_pan = Some((pos, ds.view.cx, ds.view.cy)),
                Some((p0, cx, cy)) => {
                    let z = ds.view.zoom.max(1e-6);
                    ds.view.cx = cx - f64::from(pos.x - p0.x) / z;
                    ds.view.cy = cy - f64::from(pos.y - p0.y) / z;
                    ds.revision += 1;
                }
            }
        }
        if !middle_down && !ctx.input(|i| i.pointer.primary_down()) {
            app.canvas.middle_pan = None;
        }
        return true;
    }
    if !app.canvas.dragging && (resp.hovered() || app.ui.comments.dragging()) && crate::comments::input(app, &ctx, pos) {
        return false;
    }
    if !app.canvas.dragging && resp.hovered() && crate::guides::input(app, &ctx, pos) {
        return false;
    }
    if !app.canvas.dragging && resp.hovered() && crate::handles::input(app, &ctx, pos) {
        return false;
    }
    if resp.secondary_clicked() {
        open_context_menu(app, pos, mods.command);
        return true;
    }
    if resp.double_clicked() {
        let ev = pointer_event(app, PointerKind::DoubleClick, pos, &mods);
        if let Err(e) = app.session.pointer(&ev) {
            app.toast(e.to_string());
        }
        app.canvas.dragging = false;
        app.canvas.last_drag_input = None;
        return true;
    }
    let primary_pressed = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    let primary_down = ctx.input(|i| i.pointer.primary_down());
    let primary_released = ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary));
    let kind = if primary_pressed && resp.hovered() {
        app.canvas.dragging = true;
        app.ui.context_menu = None;
        app.ui.tool_flyout = None;
        // Clicking the canvas takes focus from panel fields.
        ctx.memory_mut(|m| {
            if let Some(f) = m.focused() {
                m.surrender_focus(f);
            }
        });
        Some(PointerKind::Down)
    } else if app.canvas.dragging && primary_released {
        app.canvas.dragging = false;
        Some(PointerKind::Up)
    } else if app.canvas.dragging && primary_down {
        Some(PointerKind::Move)
    } else if resp.hovered() {
        app.canvas.hovering = true;
        Some(PointerKind::Hover)
    } else {
        // Something over the canvas took the pointer.
        if std::mem::take(&mut app.canvas.hovering) {
            app.session.vector_pointer_left();
        }
        None
    };
    // Pen pressure comes with touch events (a pen on Windows, a stylus in a browser); a mouse,
    // macOS and Linux tablets report none and draw at the stroke weight.
    let force = ctx.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            egui::Event::Touch { force: Some(f), .. } if f.is_finite() && *f > 0.0 => Some(f.clamp(0.0, 1.0)),
            _ => None,
        })
    });
    if force.is_some() {
        app.canvas.pressure = force;
    }
    if let Some(k) = kind {
        let input = (pos, mods, app.canvas.pressure);
        if k != PointerKind::Move || app.canvas.last_drag_input != Some(input) {
            send(app, k, pos, &mods);
            #[cfg(not(target_arch = "wasm32"))]
            if k == PointerKind::Down && !drag_visual_matches_idle_document(app) {
                start_drag_visual(app, &ctx);
            }
        }
        app.canvas.last_drag_input = if k == PointerKind::Up { None } else { Some(input) };
    }
    true
}

/// Pointer event `kind` at screen point `pos`, to the tools.
fn send(app: &mut LizardApp, kind: PointerKind, pos: Pos2, m: &egui::Modifiers) {
    let ev = pointer_event(app, kind, pos, m);
    if kind == PointerKind::Up {
        app.canvas.pressure = None;
    }
    app.canvas.last_pointer = Some(Point::new(ev.x, ev.y));
    if let Err(e) = app.session.pointer(&ev) {
        app.toast(e.to_string());
    }
    app.handle_ui_requests();
}

/// Right-click at screen `pos`: select what's under the pointer (`deep`: the nested layer), then
/// open the context menu there.
pub fn open_context_menu(app: &mut LizardApp, pos: Pos2, deep: bool) {
    let p = app.to_page(pos);
    if let Ok(ds) = app.session.ds() {
        let (doc, page, sel, zoom) = (ds.doc.clone(), ds.page, ds.selection.clone(), ds.view.zoom.max(1e-6));
        if let Some(leaf) = app.session.hit(&doc, page, p, 2.0 / zoom) {
            let target = lizard_doc::geometry::select_target(&doc, leaf, &sel, deep);
            if !sel.contains(&target) {
                app.session.set_selection(vec![target]);
            }
        }
    }
    app.ui.context_menu = Some((pos, app.session.selection()));
}

fn stroke(c: Color32, w: f32) -> Stroke {
    Stroke::new(w, c)
}

/// A text layer's layout placed on the canvas: where carets and selected characters show (yours
/// and other people's in a live session).
pub(crate) struct TextOnScreen {
    pub layout: Arc<lizard_text::TextLayout>,
    /// How many characters the text has.
    pub chars: usize,
    /// The layout's vertical offset in the box (vertical alignment).
    dy: f64,
    world: lizard_geom::Affine,
}

impl TextOnScreen {
    /// Text layer `id` of `doc` (`None`: not a text layer).
    pub(crate) fn of(doc: &Document, id: NodeId) -> Option<Self> {
        let n = doc.node(id)?;
        let NodeKind::Text(t) = &n.kind else { return None };
        let (layout, dy) = lizard_text::layout_node(n)?;
        Some(Self { layout, chars: t.char_count(), dy, world: doc.world_transform(id) })
    }

    /// Layout point `(x, y)` on screen (on the path, for text on a path).
    pub(crate) fn at(&self, app: &LizardApp, x: f64, y: f64) -> Pos2 {
        let q = self.layout.warp_point(x, y);
        let p = self.world * Point::new(q.x, q.y + self.dy);
        app.to_screen(p.x, p.y)
    }

    /// The highlight of characters `a..b`, one screen quad per line.
    pub(crate) fn selection(&self, app: &LizardApp, a: usize, b: usize) -> Vec<[Pos2; 4]> {
        let rects = self.layout.selection_rects(a, b);
        rects.iter().map(|r| [self.at(app, r.x0, r.y0), self.at(app, r.x1, r.y0), self.at(app, r.x1, r.y1), self.at(app, r.x0, r.y1)]).collect()
    }

    /// The caret before character `index`: its top and bottom on screen (an empty layer: a line
    /// of default height at its origin).
    pub(crate) fn caret(&self, app: &LizardApp, index: usize, zoom: f64) -> [Pos2; 2] {
        let (cx, li) = self.layout.caret(index);
        match self.layout.lines.get(li).or(self.layout.lines.last()) {
            Some(line) => [self.at(app, cx, line.top), self.at(app, cx, line.top + line.height)],
            None => {
                let p = self.at(app, 0.0, 0.0);
                [p, p + vec2(0.0, 14.0 * zoom as f32)]
            }
        }
    }
}

/// A node's oriented outline in screen space.
pub(crate) fn node_quad(app: &LizardApp, doc: &Document, id: NodeId) -> Option<[Pos2; 4]> {
    let n = doc.node(id)?;
    let w = doc.world_transform(id);
    let corners = [Point::new(0.0, 0.0), Point::new(n.width, 0.0), Point::new(n.width, n.height), Point::new(0.0, n.height)];
    Some(corners.map(|c| {
        let p = w * c;
        app.to_screen(p.x, p.y)
    }))
}

fn quad(painter: &egui::Painter, q: [Pos2; 4], s: Stroke) {
    painter.add(Shape::closed_line(q.to_vec(), s));
}

/// The slots of the selected components and instances (and a slot being dropped onto): a dashed
/// outline and the slot's name above its top-left corner, like a frame label (never over the
/// content).
fn slot_badges(app: &LizardApp, painter: &egui::Painter, doc: &Document, sel: &[NodeId], drop: Option<NodeId>) {
    use lizard_engine::components::{owning_component, owning_instance, slot_metadata, slot_name, slots_of};
    type EmptyCache = ((u64, u64, Option<NodeId>), Vec<(NodeId, String)>);
    let mut roots: Vec<NodeId> = Vec::new();
    for id in sel {
        let root = match doc.node(*id).map(|n| &n.kind) {
            Some(NodeKind::Instance { .. } | NodeKind::Component { .. } | NodeKind::ComponentSet { .. }) => Some(*id),
            _ => owning_instance(doc, *id).or_else(|| owning_component(doc, *id)),
        };
        if let Some(r) = root.filter(|r| !roots.contains(r)) {
            roots.push(r);
        }
    }
    let mut slots: Vec<(NodeId, String)> = roots.iter().flat_map(|r| slots_of(doc, *r)).collect();
    let key = app.session.ds().map(|state| (state.uid, state.revision, doc.current_page)).unwrap_or_default();
    let cache_id = egui::Id::new("slot_badges_empty_instances");
    let empty =
        painter.ctx().data(|data| data.get_temp::<EmptyCache>(cache_id)).filter(|(cached, _)| *cached == key).map(|(_, slots)| slots).unwrap_or_else(
            || {
                let mut found = Vec::new();
                if let Some(page) = doc.current_page {
                    for id in doc.subtree(page) {
                        if !doc.node(id).is_some_and(|node| matches!(node.kind, NodeKind::Frame) && node.children.is_empty())
                            || owning_instance(doc, id).is_none()
                        {
                            continue;
                        }
                        if let Some((name, _, settings)) = slot_metadata(doc, id)
                            && settings.show_empty
                        {
                            found.push((id, name.to_string()));
                        }
                    }
                }
                painter.ctx().data_mut(|data| data.insert_temp(cache_id, (key, found.clone())));
                found
            },
        );
    for item in empty {
        if !slots.iter().any(|(id, _)| *id == item.0) {
            slots.push(item);
        }
    }
    if let Some(d) = drop
        && !slots.iter().any(|(s, _)| *s == d)
        && let Some(name) = slot_name(doc, d)
    {
        slots.push((d, name));
    }
    for (id, name) in slots {
        let Some(q) = node_quad(app, doc, id) else { continue };
        if !Rect::from_points(&q).intersects(app.canvas.rect) {
            continue;
        }
        painter.extend(Shape::dashed_line(&[q[0], q[1], q[2], q[3], q[0]], stroke(COMPONENT, 1.0), 4.0, 3.0));
        // The label, when the slot is wide enough on screen to hold it.
        let galley = painter.layout_no_wrap(name, regular(10.0), COMPONENT);
        let (w, h) = (galley.size().x + 14.0, galley.size().y.max(11.0 + crate::frame_labels::ICON_OPTICAL_OFFSET * 2.0));
        if (q[1] - q[0]).length() < w {
            continue;
        }
        let at = q[0] - vec2(0.0, h + crate::frame_labels::LABEL_GAP);
        let icon = Rect::from_center_size(pos2(at.x + 5.5, at.y + h / 2.0 + crate::frame_labels::ICON_OPTICAL_OFFSET), vec2(11.0, 11.0));
        crate::icons::paint(painter, icon, crate::icons::Icon::Slot, COMPONENT);
        painter.galley(pos2(at.x + 14.0, at.y + (h - galley.size().y) / 2.0), galley, COMPONENT);
    }
}

/// The instance an Alt-drop of a component from the Assets panel swaps: tinted and outlined in
/// the component colour, with a "Swap instance" label above its top-left corner.
fn swap_highlight(app: &LizardApp, painter: &egui::Painter, doc: &Document, id: NodeId) {
    let Some(q) = node_quad(app, doc, id) else { return };
    // White on the component colour, in both themes (like the canvas's selection labels).
    let ink = Color32::WHITE;
    painter.add(Shape::convex_polygon(q.to_vec(), COMPONENT.gamma_multiply(0.12), Stroke::NONE));
    quad(painter, q, stroke(COMPONENT, 2.0));
    let galley = painter.layout_no_wrap(tr("Swap instance").to_string(), semibold(10.0), ink);
    let size = vec2(galley.size().x + 26.0, 18.0);
    let top = Rect::from_points(&q).min;
    let pill = Rect::from_min_size(pos2(top.x, top.y - size.y - crate::frame_labels::LABEL_GAP), size);
    painter.rect_filled(pill, CornerRadius::same(4), COMPONENT);
    crate::icons::paint(
        painter,
        Rect::from_center_size(pos2(pill.min.x + 11.0, pill.center().y + crate::frame_labels::ICON_OPTICAL_OFFSET), vec2(10.0, 10.0)),
        crate::icons::Icon::Instance,
        ink,
    );
    painter.galley(pos2(pill.min.x + 20.0, pill.center().y - galley.size().y / 2.0), galley, ink);
}

/// The outline colour for a layer: pink for components, sets and instances and every layer in
/// them, the page's overlay `green` otherwise.
fn outline_colour(doc: &Document, id: NodeId, green: Color32) -> Color32 {
    use lizard_engine::components::{owning_component, owning_instance};
    let set = matches!(doc.node(id).map(|n| &n.kind), Some(NodeKind::ComponentSet { .. }));
    if set || owning_component(doc, id).is_some() || owning_instance(doc, id).is_some() { COMPONENT } else { green }
}

/// Shapes whose visible outline differs from their rectangular transform box.
fn has_path_outline(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Ellipse { .. } | NodeKind::Vector { .. } | NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::BooleanOp { .. })
}

/// The outline path of shapes (ellipses, vectors) for hover/selection outlines.
fn outline_shape(app: &LizardApp, doc: &Document, id: NodeId) -> Option<Vec<Vec<Pos2>>> {
    let n = doc.node(id)?;
    if !has_path_outline(&n.kind) {
        return None;
    }
    // A vector's fill path contains only closed regions. Open pen and pencil strokes must
    // follow the network's segments, including any dangling segments on a filled vector.
    let path = match &n.kind {
        NodeKind::Vector { network } => network.stroke_path(),
        _ => lizard_render::node_outline(doc, n)?.0,
    };
    let w = doc.world_transform(id);
    let mut out = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    lizard_geom::kurbo::flatten(path.iter(), 0.5 / app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0), |el| {
        use lizard_geom::kurbo::PathEl::*;
        match el {
            MoveTo(p) => {
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                let q = w * p;
                cur.push(app.to_screen(q.x, q.y));
            }
            LineTo(p) => {
                let q = w * p;
                cur.push(app.to_screen(q.x, q.y));
            }
            ClosePath => {
                if let Some(f) = cur.first().copied() {
                    cur.push(f);
                }
            }
            _ => {}
        }
    });
    if cur.len() > 1 {
        out.push(cur);
    }
    Some(out)
}

/// A layer's outline on the canvas: its shape for ellipses, vectors, polygons, stars and boolean
/// groups, its box otherwise.
fn draw_outline(app: &LizardApp, painter: &egui::Painter, doc: &Document, id: NodeId, s: Stroke) {
    match outline_shape(app, doc, id) {
        Some(lines) => {
            for l in lines {
                painter.add(Shape::line(l, s));
            }
        }
        None => {
            if let Some(q) = node_quad(app, doc, id) {
                quad(painter, q, s);
            }
        }
    }
}

/// Everything drawn over the rendered page; returns the frame labels drawn (frame, corners on the
/// page) for the engine to hit-test.
fn overlays(app: &LizardApp, painter: &egui::Painter, t: &Tokens, painted_doc: Option<&Document>) -> Vec<(NodeId, [Point; 4])> {
    let mut labels = Vec::new();
    let Ok(ds) = app.session.ds() else { return labels };
    let doc = ds.doc.clone();
    let selected_doc = painted_doc.unwrap_or(&doc);
    let page = ds.page;
    let sel = ds.selection.clone();
    let zoom = ds.view.zoom;
    let view = ds.view.clone();
    // Pixel grid at high zoom.
    if view.pixel_grid_shown() {
        let r = app.canvas.rect;
        let p0 = app.to_page(r.min);
        let p1 = app.to_page(r.max);
        let col = Color32::from_black_alpha(18);
        let mut x = p0.x.floor();
        while x <= p1.x {
            let s = app.to_screen(x, 0.0).x;
            painter.line_segment([pos2(s, r.min.y), pos2(s, r.max.y)], stroke(col, 1.0));
            x += 1.0;
        }
        let mut y = p0.y.floor();
        while y <= p1.y {
            let s = app.to_screen(0.0, y).y;
            painter.line_segment([pos2(r.min.x, s), pos2(r.max.x, s)], stroke(col, 1.0));
            y += 1.0;
        }
    }
    // Layout grids and frame labels for top-level frames (also those in sections); the focus view
    // labels only its frame.
    let focus = ds.focused();
    for c in boards(&doc, page).iter().filter(|c| focus.is_none_or(|f| f == **c)) {
        let Some(n) = doc.node(*c) else { continue };
        let Some(b) = doc.world_bounds(*c) else { continue };
        // Off-screen frames draw neither grids nor a label (labels sit just outside the frame).
        let on_screen = Rect::from_two_pos(app.to_screen(b.x0, b.y0), app.to_screen(b.x1, b.y1)).expand(20.0);
        if !on_screen.intersects(app.canvas.rect) {
            continue;
        }
        if view.layout_grids && !n.grids.is_empty() && lizard_layout::grids_shown(&doc, *c) {
            let w = doc.world_transform(*c);
            let a = w * Point::new(0.0, 0.0);
            let b = w * Point::new(n.width, n.height);
            let frame_clip = Rect::from_two_pos(app.to_screen(a.x, a.y), app.to_screen(b.x, b.y)).intersect(app.canvas.rect);
            if frame_clip.is_positive() {
                let grid_painter = painter.with_clip_rect(frame_clip);
                for g in n.grids.iter().filter(|g| g.visible) {
                    let col = doc_c32(painter.ctx(), g.color);
                    for r in lizard_layout::grid_lines(g, n.width, n.height) {
                        let a = w * Point::new(r.x0, r.y0);
                        let b = w * Point::new(r.x1, r.y1);
                        let (sa, sb) = (app.to_screen(a.x, a.y), app.to_screen(b.x, b.y));
                        if g.kind == lizard_doc::GridKind::Grid {
                            grid_painter.line_segment([sa, sb], stroke(col, 1.0));
                        } else {
                            grid_painter.rect_filled(Rect::from_two_pos(sa, sb), CornerRadius::ZERO, col);
                        }
                    }
                }
            }
        }
        // The focus view's bar names its frame.
        let labelled = focus.is_none() && (n.kind.is_frame_like() || matches!(n.kind, NodeKind::Section { .. }));
        if labelled && let Some(q) = crate::frame_labels::draw(app, painter, t, &doc, *c, sel.contains(c), ds.hover == Some(*c)) {
            labels.push((*c, q.map(|p| app.to_page(p))));
        }
    }
    // Prototype connections.
    crate::proto_canvas::draw(app, painter);
    // Hover outline (measuring outlines the measured box in red instead).
    if let Some(h) = ds.hover
        && !sel.contains(&h)
        && matches!(app.session.tool.tool, ToolId::Move | ToolId::Scale)
        && app.session.prefs.highlight_on_hover
        && app.session.tool.overlay.measure_box.is_none()
    {
        draw_outline(app, painter, &doc, h, stroke(outline_colour(&doc, h, accent(painter)), 1.5));
    }
    // Inside a boolean group (an operand selected or in vector edit mode): every operand's outline,
    // so the shapes making the live result stay visible while one of them changes.
    let operands = lizard_engine::booleans::outlined_operands(&doc, &sel, ds.vector_edit.as_ref().map(|ve| ve.id));
    for id in operands.iter().filter(|o| !sel.contains(o)) {
        draw_outline(app, painter, &doc, *id, stroke(outline_colour(&doc, *id, accent(painter)).gamma_multiply(0.6), 1.0));
    }
    // Mask outlines (a mask doesn't draw itself), frame outlines and slices, as the View menu shows.
    if view.mask_outlines || view.frame_outlines || view.show_slices {
        for id in doc.subtree(page) {
            let Some(n) = doc.node(id).filter(|n| n.visible) else { continue };
            if view.mask_outlines && n.is_mask {
                draw_outline(app, painter, &doc, id, stroke(accent(painter), 1.0));
            } else if view.frame_outlines && n.kind.is_frame_like() {
                draw_outline(app, painter, &doc, id, stroke(t.text_secondary, 1.0));
            } else if view.show_slices
                && matches!(n.kind, NodeKind::Slice)
                && let Some(q) = node_quad(app, &doc, id)
            {
                painter.extend(Shape::dashed_line(&[q[0], q[1], q[2], q[3], q[0]], stroke(t.text_secondary, 1.0), 4.0, 3.0));
            }
        }
    }
    // Drop target / insertion indicator.
    let ov = &app.session.tool.overlay;
    if let Some(d) = ov.drop_target
        && let Some(q) = node_quad(app, &doc, d)
    {
        quad(painter, q, stroke(if lizard_engine::components::is_slot(&doc, d) { COMPONENT } else { accent(painter) }, 1.0));
    }
    slot_badges(app, painter, &doc, &sel, ov.drop_target);
    if let Some(t) = ov.swap_target {
        swap_highlight(app, painter, &doc, t);
    }
    if let Some([x0, y0, x1, y1]) = ov.insert_line {
        let line = [app.to_screen(x0, y0), app.to_screen(x1, y1)];
        painter.line_segment(line, stroke(Color32::from_black_alpha(190), 5.0));
        let purple = if t.dark { Color32::from_rgb(0xB5, 0x96, 0xFF) } else { Color32::from_rgb(0x62, 0x43, 0xDE) };
        painter.line_segment(line, stroke(purple, 3.0));
    }
    if let Some([x0, y0, x1, y1]) = ov.drop_cell {
        let r = Rect::from_two_pos(app.to_screen(x0, y0), app.to_screen(x1, y1));
        painter.rect_filled(r, CornerRadius::same(2), accent(painter).gamma_multiply(0.12));
        let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.add(Shape::dashed_line(&pts, stroke(accent(painter), 1.5), 4.0, 3.0));
    }
    // Vector edit mode, with the transform box around several selected points.
    // The selection's size label, which measurement labels keep clear of.
    let mut size_label = None;
    if let Some(ve) = ds.vector_edit.clone()
        && let Some(o) = lizard_engine::vectoredit::overlay(&doc, &ve)
    {
        // The Variable width tool shows width points in place of the path's points.
        let width_tool = ve.tool == lizard_engine::vectoredit::VTool::Width;
        // Other layers edited at once go under the active one; the hover shows on its own layer.
        let ov = &app.session.tool.overlay;
        let hover_on = |id| ov.vector_hover.filter(|_| ov.vector_hover_on.unwrap_or(ve.id) == id);
        for part in &ve.others {
            if let Some(po) = lizard_engine::vectoredit::overlay(&doc, &ve.view(part)) {
                vector_overlay(app, painter, &po, !width_tool, hover_on(part.id), false);
            }
        }
        vector_overlay(app, painter, &o, !width_tool, hover_on(ve.id), true);
        if width_tool {
            width_overlay(app, painter, &doc, &ve, zoom);
        } else if let Some(c) = app.session.vector_edit_box() {
            let label = format!("{} × {}", fmt((c[1] - c[0]).hypot()), fmt((c[3] - c[0]).hypot()));
            size_label = transform_box(app, painter, c.map(|p| app.to_screen(p.x, p.y)), accent(painter), true, 4, Some(label));
        }
    }
    // Selection.
    let editing = ds.text_edit.clone();
    if let Some((id, [dx, dy])) = app.session.tool.overlay.drag_ghost
        && let Some(source) = ds.interaction.as_ref().map(|it| &it.doc)
        && let Some(mut q) = node_quad(app, source, id)
    {
        let offset = vec2((dx * zoom) as f32, (dy * zoom) as f32);
        for point in &mut q {
            *point += offset;
        }
        quad(painter, q, stroke(accent(painter), 1.0));
    } else if !sel.is_empty() && editing.is_none() && ds.vector_edit.is_none() {
        let col = if sel.iter().any(|i| outline_colour(&doc, *i, accent(painter)) == COMPONENT) { COMPONENT } else { accent(painter) };
        for id in &sel {
            draw_outline(app, painter, selected_doc, *id, stroke(col, 1.0));
        }
        // The shape builder reshapes by regions: no transform box.
        if app.session.tool.tool != ToolId::ShapeBuilder
            && let Some(b) = selection_box(selected_doc, &sel)
            && let Some((_, w, h)) = selection_frame(selected_doc, &sel)
        {
            let is_line = sel.len() == 1 && matches!(sel.first().and_then(|i| doc.node(*i)).map(|n| &n.kind), Some(NodeKind::Line));
            let mut label = if is_line { fmt(w) } else { format!("{} × {}", fmt(w), fmt(h)) };
            let scale = app.session.tool.overlay.scale;
            if let Some(hint) = scale {
                label = format!("{label} · {}x", fmt(hint.factor.abs()));
            }
            // A layer smaller than its handles on screen shows just its outline (they still answer
            // just outside it).
            let tiny = lizard_engine::tools::box_sides_px(&b, zoom).1 < lizard_engine::tools::SMALL_BOX_PX;
            // Measuring to another layer (Alt held) shows just the outlines: no handles or size.
            let measuring = app.session.tool.overlay.measure_box.is_some();
            // Locked layers (selected in the layers panel) can't be resized: no handles either.
            let handles = if measuring || sel.iter().any(|i| lizard_engine::tools::locked_in(&doc, *i)) {
                0
            } else if is_line {
                2
            } else if tiny {
                0
            } else {
                4
            };
            let has_shape_outline = sel.len() == 1 && sel.first().and_then(|id| doc.node(*id)).is_some_and(|n| has_path_outline(&n.kind));
            size_label = transform_box(
                app,
                painter,
                b.map(|p| app.to_screen(p.x, p.y)),
                col,
                sel.len() > 1 || has_shape_outline,
                handles,
                (!measuring).then_some(label),
            );
            if !measuring {
                arc_overlay(app, painter, col);
                shape_overlay(app, painter, col);
            }
            if app.session.tool.tool == ToolId::Scale {
                // The point that stays put: the dragged pivot, else the anchor.
                let pivot = scale
                    .map(|h| Point::new(h.pivot[0], h.pivot[1]))
                    .or_else(|| lizard_engine::tools::scale_anchor_point(&doc, &sel, app.session.tool.scale_anchor));
                if let Some(p) = pivot {
                    let c = app.to_screen(p.x, p.y);
                    painter.circle_filled(c, 4.0, Color32::WHITE);
                    painter.circle_stroke(c, 4.0, stroke(col, 1.5));
                    painter.circle_filled(c, 1.5, col);
                }
            }
            // The rotation origin while it shows (Alt+R): a ring with a cross through it.
            if let Some(o) = ds.origin_handle() {
                let c = app.to_screen(o.x, o.y);
                painter.circle_filled(c, 5.0, Color32::WHITE);
                painter.circle_stroke(c, 5.0, stroke(col, 1.5));
                for d in [vec2(1.0, 0.0), vec2(0.0, 1.0)] {
                    painter.line_segment([c - d * 10.0, c - d * 5.0], stroke(col, 1.5));
                    painter.line_segment([c + d * 5.0, c + d * 10.0], stroke(col, 1.5));
                }
                painter.circle_filled(c, 1.5, col);
            }
        }
    }
    if app.session.tool.tool == ToolId::ShapeBuilder
        && let Some(arr) = lizard_engine::shapebuilder::current(&app.session)
    {
        builder_overlay(app, painter, t, &arr);
    }
    // Text editing: caret and selection, or the IME composition underlined at the caret.
    let preedit = app.preedit_view();
    let edoc: &Document = preedit.map(|p| &*p.doc).unwrap_or(&doc);
    if let Some(te) = editing
        && let Some(text) = TextOnScreen::of(edoc, te.id)
    {
        let layout = &text.layout;
        if let Some(q) = node_quad(app, edoc, te.id) {
            quad(painter, q, stroke(accent(painter), 1.0));
        }
        let (a, b) = (te.caret.min(te.anchor), te.caret.max(te.anchor));
        if a != b && preedit.is_none() {
            for q in text.selection(app, a, b) {
                let selection = t.text_selection_bg;
                painter.add(Shape::convex_polygon(
                    q.to_vec(),
                    Color32::from_rgba_unmultiplied(selection.r(), selection.g(), selection.b(), 128),
                    Stroke::NONE,
                ));
            }
        }
        if preedit.is_none() && app.ui.figma_text_review.node == Some(te.id) {
            for mark in &app.ui.figma_text_review.marks {
                for quad in text.selection(app, mark.start, mark.end) {
                    painter.line_segment([quad[3], quad[2]], stroke(mark.color, 1.5));
                }
            }
        }
        if let Some(p) = preedit {
            for r in layout.selection_rects(p.range.0, p.range.1) {
                // Underline just below the baseline, across the composed chars only.
                let Some(line) = layout.lines.iter().find(|l| (l.top - r.y0).abs() < 1e-6) else { continue };
                let x1 = layout
                    .glyphs
                    .get(line.glyph_start..line.glyph_end)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|g| g.char_end > g.char_start && g.char_start >= p.range.0 && g.char_end <= p.range.1)
                    .map(|g| g.x + g.advance)
                    .fold(r.x0, f64::max);
                let y = line.baseline + (line.top + line.height - line.baseline) * 0.35;
                painter.line_segment([text.at(app, r.x0, y), text.at(app, x1, y)], stroke(t.text, 1.0));
            }
        }
        let caret = preedit.map(|p| p.cursor).unwrap_or(te.caret);
        let blink = (painter.ctx().input(|i| i.time) * 2.0) as i64 % 2 == 0;
        if blink || a != b {
            painter.line_segment(text.caret(app, caret, zoom), stroke(t.text, 1.0));
        }
        painter.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    // Smart guides and measurements (their labels kept clear of the size label).
    crate::measure_overlay::draw(app, painter, t, size_label);
    // Marquee.
    if let Some([x0, y0, x1, y1]) = ov.marquee.or(app.ui.key_box) {
        let r = Rect::from_two_pos(app.to_screen(x0, y0), app.to_screen(x1, y1));
        painter.rect_filled(r, CornerRadius::ZERO, accent(painter).gamma_multiply(0.1));
        painter.rect_stroke(r, CornerRadius::ZERO, stroke(accent(painter), 1.0), StrokeKind::Inside);
    }
    // Pen path.
    if !ov.pen.is_empty() {
        let pts: Vec<Pos2> = ov.pen.iter().map(|[x, y]| app.to_screen(*x, *y)).collect();
        if app.session.tool.tool == ToolId::Brush {
            paint_live_brush(app, painter, ds.revision, zoom);
        } else if app.session.tool.tool == ToolId::Pencil {
            // The stroke it will make, in its colour and weight (at least a point when zoomed out).
            let style = app.session.tool.draw_style(app.session.tool.tool);
            let color = style.map_or(lizard_color::Color::BLACK, |s| s.color());
            let weight = style.map_or(2.0, |s| s.weight(app.session.tool.tool));
            painter.add(Shape::line(pts, stroke(doc_c32(painter.ctx(), color), ((weight * zoom) as f32).max(1.0))));
        } else {
            for [x0, y0, x1, y1] in &ov.pen_handles {
                bezier_handle(painter, app.to_screen(*x0, *y0), app.to_screen(*x1, *y1), 3.5, Color32::WHITE);
            }
            for p in &pts {
                painter.circle_filled(*p, 3.5, Color32::WHITE);
                painter.circle_stroke(*p, 3.5, stroke(accent(painter), 1.0));
            }
            if let Some([x0, y0, x1, y1]) = ov.rubber {
                let (a, b) = (app.to_screen(x0, y0), app.to_screen(x1, y1));
                // The segment a click would make: along the last anchor's out handle, if it has one.
                match ov.rubber_handle {
                    Some([hx, hy]) => {
                        let h = app.to_screen(hx, hy);
                        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                            [a, h, b, b],
                            false,
                            Color32::TRANSPARENT,
                            stroke(accent(painter), 1.0),
                        ));
                    }
                    None => {
                        painter.line_segment([a, b], stroke(accent(painter), 1.0));
                    }
                }
            }
        }
    }
    if let Some([x0, y0, x1, y1]) = ov.angle_guide {
        let (a, b) = (app.to_screen(x0, y0), app.to_screen(x1, y1));
        if a.distance_sq(b) > 1.0 {
            painter.add(Shape::dashed_line(&[a, b], stroke(Color32::from_rgb(236, 99, 110), 1.5), 4.0, 3.0));
        }
    }
    // Eraser brush (vector edit mode).
    if let Some([x, y, r]) = ov.brush {
        let c = app.to_screen(x, y);
        let rs = (r * app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0)) as f32;
        painter.circle_stroke(c, rs, stroke(Color32::WHITE, 3.0));
        painter.circle_stroke(c, rs, stroke(accent(painter), 1.5));
    }
    // Lasso (vector edit mode).
    if ov.lasso.len() > 1 {
        let mut pts: Vec<Pos2> = ov.lasso.iter().map(|[x, y]| app.to_screen(*x, *y)).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        painter.add(Shape::dashed_line(&pts, stroke(accent(painter), 1.0), 4.0, 3.0));
    }
    // Guides (under the rulers).
    crate::guides::draw(app, painter);
    // Rulers.
    if view.rulers {
        rulers(app, painter, t, &doc, &sel);
    }
    labels
}

/// A transform box (screen corners tl, tr, br, bl): its outline when `outline`, the first `handles`
/// corner handles and, when dimensions are shown, the size `label` (if any) under it (its box returned).
fn transform_box(
    app: &LizardApp,
    painter: &egui::Painter,
    q: [Pos2; 4],
    col: Color32,
    outline: bool,
    handles: usize,
    label: Option<String>,
) -> Option<Rect> {
    if outline {
        quad(painter, q, stroke(col, 1.0));
    }
    for c in q.iter().take(handles) {
        let r = Rect::from_center_size(*c, vec2(8.0, 8.0));
        painter.rect_filled(r, CornerRadius::same(1), Color32::WHITE);
        painter.rect_stroke(r, CornerRadius::same(1), stroke(col, 1.0), StrokeKind::Inside);
    }
    if let Some(label) = label.filter(|_| app.session.prefs.show_dimensions) {
        let bottom = q.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        let cx = q.iter().map(|p| p.x).sum::<f32>() / 4.0;
        return Some(pill_with_font(painter, pos2(cx, bottom + 9.0), Align2::CENTER_TOP, label, col, regular(10.5)));
    }
    None
}

/// A canvas readout: white `text` on a `col` pill placed at `at` by `align` (its box returned).
fn pill(painter: &egui::Painter, at: Pos2, align: Align2, text: String, col: Color32) -> Rect {
    pill_with_font(painter, at, align, text, col, semibold(11.0))
}

fn pill_with_font(painter: &egui::Painter, at: Pos2, align: Align2, text: String, col: Color32, font: egui::FontId) -> Rect {
    let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
    let r = align.anchor_size(at, galley.size() + vec2(8.0, 4.0));
    painter.rect_filled(r, CornerRadius::same(3), crate::theme::under_white(col));
    painter.galley(r.min + vec2(4.0, 2.0), galley, Color32::WHITE);
    r
}

/// The selected ellipse's arc handles (the hovered or dragged one larger, Start with a dot inside)
/// and, while one is dragged, its readout beside it.
fn arc_overlay(app: &LizardApp, painter: &egui::Painter, col: Color32) {
    use lizard_engine::arc_handles::ArcHandle;
    let Some(hs) = app.session.arc_handles() else { return };
    let ov = &app.session.tool.overlay;
    for h in [ArcHandle::Ratio, ArcHandle::Start, ArcHandle::Sweep] {
        let Some([x, y]) = hs.at(h) else { continue };
        let c = app.to_screen(x, y);
        let r = if ov.arc_hover == Some(h) { 5.0 } else { 4.0 };
        painter.circle_filled(c, r, Color32::WHITE);
        painter.circle_stroke(c, r, stroke(col, 1.0));
        if h == ArcHandle::Start {
            painter.circle_filled(c, 1.5, col);
        }
    }
    if let Some(hint) = &ov.arc {
        let c = app.to_screen(hint.at[0], hint.at[1]);
        pill(painter, c + vec2(12.0, 12.0), Align2::LEFT_TOP, hint.label.clone(), col);
    }
}

/// The selected shape's corner radius handles and a star's ratio handle (the hovered or dragged
/// one larger) and, while one is dragged, its value beside it.
fn shape_overlay(app: &LizardApp, painter: &egui::Painter, col: Color32) {
    use lizard_engine::shape_handles::ShapeHandle;
    let ov = &app.session.tool.overlay;
    for (h, [x, y]) in app.session.shape_handles() {
        let r = if ov.shape_hover == Some(h) { 4.5 } else { 3.5 };
        let c = app.to_screen(x, y);
        painter.circle_filled(c, r, Color32::WHITE);
        painter.circle_stroke(c, r, stroke(col, 1.0));
    }
    if let Some(hint) = &ov.shape {
        let c = app.to_screen(hint.at[0], hint.at[1]);
        let text = match hint.handle {
            ShapeHandle::Radius { .. } => format!("{} {}", tr("Radius"), hint.value),
            ShapeHandle::Ratio => format!("{} {}%", tr("Ratio"), hint.value),
        };
        pill(painter, c + vec2(12.0, 12.0), Align2::LEFT_TOP, text, col);
    }
}

/// Variable width tool: each width point on the path with a bar across the stroke to its two
/// width handles (the selected point filled), and a faint preview where a drag would add one.
fn width_overlay(app: &LizardApp, painter: &egui::Painter, doc: &Document, ve: &lizard_engine::vectoredit::VectorEdit, zoom: f64) {
    use lizard_doc::WidthSide;
    use lizard_engine::vectoredit::{WHit, width_handles, width_preview};
    let hover = app.session.tool.overlay.width_hover;
    let sp = |p: &[f64; 2]| app.to_screen(p[0], p[1]);
    let draw = |h: &lizard_engine::vectoredit::WidthHandle, ghost: bool| {
        let (c, a, b) = (sp(&h.at), sp(&h.sides[0]), sp(&h.sides[1]));
        let col = if ghost { accent(painter).gamma_multiply(0.5) } else { accent(painter) };
        painter.line_segment([a, b], stroke(Color32::from_white_alpha(160), 3.0));
        painter.line_segment([a, b], stroke(col, 1.0));
        for (e, side) in [(a, WidthSide::Right), (b, WidthSide::Left)] {
            let grab = !ghost && hover == Some(WHit::Handle { index: h.index, side });
            let r = if grab { 4.5 } else { 3.5 };
            painter.circle_filled(e, r, Color32::WHITE);
            painter.circle_stroke(e, r, stroke(col, 1.0));
        }
        let on = !ghost && ve.width_point == Some(h.index);
        let hovered = !ghost && hover == Some(WHit::Point { index: h.index });
        let r = if hovered { 5.0 } else { 4.0 };
        painter.circle_filled(c, r, if on { accent(painter) } else { Color32::WHITE });
        painter.circle_stroke(c, r, stroke(col, 1.0));
    };
    for h in width_handles(doc, ve, zoom) {
        draw(&h, false);
    }
    if let Some(WHit::Path { t, .. }) = hover {
        for h in width_preview(doc, ve, t, zoom) {
            draw(&h, true);
        }
    }
}

/// A bezier handle from its anchor `v` to its end `e` (screen points): the line, and a dot of
/// radius `r` filled with `fill` (vector edit mode and the Pen draw them alike).
fn bezier_handle(painter: &egui::Painter, v: Pos2, e: Pos2, r: f32, fill: Color32) {
    painter.line_segment([v, e], stroke(accent(painter), 1.0));
    painter.circle_filled(e, r, fill);
    painter.circle_stroke(e, r, stroke(accent(painter), 1.0));
}

/// Vector edit mode: a layer's segments, handles and points (`points`) with its `hover`ed part, and
/// for the `active` layer the closed area under the paint bucket and the pen's rubber band.
fn vector_overlay(
    app: &LizardApp,
    painter: &egui::Painter,
    o: &lizard_engine::vectoredit::EditOverlay,
    points: bool,
    hover: Option<lizard_engine::vectoredit::VHit>,
    active: bool,
) {
    use lizard_engine::vectoredit::VHit;
    let ov = &app.session.tool.overlay;
    let sp = |p: &[f64; 2]| app.to_screen(p[0], p[1]);
    // Paint bucket target: hatched like a fill to come.
    if active && !ov.face.is_empty() {
        let polys: Vec<Vec<Pos2>> = ov.face.iter().map(|poly| poly.iter().map(sp).collect()).collect();
        hatched(painter, &polys, accent(painter));
    }
    if active {
        region_hatch(app, painter);
    }
    let hover_seg = match hover {
        Some(VHit::Segment { seg, .. }) => Some(seg),
        _ => None,
    };
    for (i, seg) in o.segments.iter().enumerate() {
        let [a, b, c, d] = [sp(&seg[0]), sp(&seg[1]), sp(&seg[2]), sp(&seg[3])];
        let w = if o.selected_segments.contains(&i) {
            3.0
        } else if hover_seg == Some(i) {
            2.5
        } else {
            1.5
        };
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke([a, b, c, d], false, Color32::TRANSPARENT, stroke(accent(painter), w)));
    }
    if !points {
        return;
    }
    for h in &o.handles {
        let hovered = hover == Some(VHit::Handle { seg: h.seg, start: h.start });
        let on = o.selected_handles.contains(&(h.seg, h.start));
        bezier_handle(painter, sp(&h.vertex), sp(&h.handle), if hovered { 4.5 } else { 3.5 }, if on { accent(painter) } else { Color32::WHITE });
    }
    for (i, v) in o.vertices.iter().enumerate() {
        let p = sp(v);
        let hovered = matches!(hover, Some(VHit::Vertex { index }) if index == i);
        let size = if hovered { 9.0 } else { 7.0 };
        let r = Rect::from_center_size(p, vec2(size, size));
        let on = o.selected_vertices.contains(&i);
        painter.rect_filled(r, CornerRadius::same(5), if on { accent(painter) } else { Color32::WHITE });
        painter.rect_stroke(r, CornerRadius::same(5), stroke(accent(painter), 1.0), StrokeKind::Inside);
        if o.anchor == Some(i) {
            painter.circle_stroke(p, 7.5, stroke(accent(painter), 1.0));
        }
    }
    if let Some([x0, y0, x1, y1]) = ov.rubber.filter(|_| active) {
        painter.add(Shape::dashed_line(&[app.to_screen(x0, y0), app.to_screen(x1, y1)], stroke(accent(painter), 1.0), 4.0, 3.0));
    }
}

/// Width of the lines hatching the hovered and selected regions in vector edit mode (screen px).
const REGION_HATCH_WIDTH: f32 = 1.4;

/// Vector edit mode: the selected regions and the hovered one hatched in the selection colour,
/// under the segments and points (the engine lays out and clips the lines).
fn region_hatch(app: &LizardApp, painter: &egui::Painter) {
    let Ok(ds) = app.session.ds() else { return };
    let (ox, oy) = ds.view.page_to_screen(0.0, 0.0);
    let (z, min) = (ds.view.zoom, app.canvas.rect.min);
    let view = lizard_geom::Affine::new([z, 0.0, 0.0, z, f64::from(min.x) + ox, f64::from(min.y) + oy]);
    let r = painter.clip_rect();
    let clip = lizard_geom::Rect::new(f64::from(r.min.x), f64::from(r.min.y), f64::from(r.max.x), f64::from(r.max.y));
    for [a, b] in app.session.vector_region_hatch(view, Some(clip)) {
        painter.line_segment([pos2(a.x as f32, a.y as f32), pos2(b.x as f32, b.y as f32)], stroke(accent(painter), REGION_HATCH_WIDTH));
    }
}

/// Shape builder: every region's edges, and the hovered or collected regions hatched (in the
/// danger colour when releasing removes them).
fn builder_overlay(app: &LizardApp, painter: &egui::Painter, t: &Tokens, arr: &lizard_engine::shapebuilder::Arrangement) {
    let ov = &app.session.tool.overlay;
    let screen = |r: &lizard_engine::shapebuilder::Region| -> Vec<Vec<Pos2>> {
        r.polys.iter().map(|poly| poly.iter().map(|p| app.to_screen(p.x, p.y)).collect()).collect()
    };
    // Edges read on any fill: a light halo under a thin accent line.
    for r in &arr.regions {
        for poly in screen(r) {
            painter.add(Shape::closed_line(poly.clone(), stroke(Color32::from_white_alpha(150), 2.5)));
            painter.add(Shape::closed_line(poly, stroke(accent(painter), 1.0)));
        }
    }
    let col = if ov.builder_remove { t.danger } else { accent(painter) };
    for r in ov.builder_regions.iter().filter_map(|i| arr.regions.get(*i)) {
        hatched(painter, &screen(r), col);
    }
}

/// Polygons hatched and outlined in `col` (an area about to be filled, merged or removed), each
/// line on a light halo so it reads on any fill, even one in `col`.
fn hatched(painter: &egui::Painter, polys: &[Vec<Pos2>], col: Color32) {
    let halo = Color32::from_white_alpha(200);
    for l in hatch(polys, 6.0) {
        painter.line_segment(l, stroke(halo, 3.0));
        painter.line_segment(l, stroke(col, 1.25));
    }
    for poly in polys {
        painter.add(Shape::closed_line(poly.clone(), stroke(halo, 4.0)));
        painter.add(Shape::closed_line(poly.clone(), stroke(col, 2.0)));
    }
}

/// 45° hatch lines `gap` apart inside polygons (even-odd), in screen space.
fn hatch(polys: &[Vec<Pos2>], gap: f32) -> Vec<[Pos2; 2]> {
    // Rotate so the hatch is horizontal, scan, rotate back.
    let rot = |p: Pos2| pos2((p.x + p.y) * std::f32::consts::FRAC_1_SQRT_2, (p.y - p.x) * std::f32::consts::FRAC_1_SQRT_2);
    let back = |x: f32, y: f32| pos2((x - y) * std::f32::consts::FRAC_1_SQRT_2, (x + y) * std::f32::consts::FRAC_1_SQRT_2);
    let edges: Vec<(Pos2, Pos2)> = polys
        .iter()
        .flat_map(|poly| {
            let pts: Vec<Pos2> = poly.iter().map(|p| rot(*p)).collect();
            let n = pts.len();
            (0..n).filter_map(move |k| Some((*pts.get(k)?, *pts.get((k + 1) % n)?)))
        })
        .collect();
    let (y0, y1) = edges.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (a, b)| (lo.min(a.y).min(b.y), hi.max(a.y).max(b.y)));
    let mut out = Vec::new();
    if !(y0.is_finite() && y1.is_finite()) || (y1 - y0) / gap > 4000.0 {
        return out;
    }
    let mut y = (y0 / gap).ceil() * gap;
    while y < y1 {
        let mut xs: Vec<f32> = edges
            .iter()
            .filter(|(a, b)| (a.y <= y && y < b.y) || (b.y <= y && y < a.y))
            .map(|(a, b)| a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y))
            .collect();
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            if let [a, b] = pair {
                out.push([back(*a, y), back(*b, y)]);
            }
        }
        y += gap;
    }
    out
}

/// The layers that get canvas labels and layout grids, in z-order: the page's visible top-level
/// layers and, inside sections whose contents show, their visible children (sections nest).
fn boards(doc: &Document, page: NodeId) -> Vec<NodeId> {
    fn walk(doc: &Document, id: NodeId, depth: usize, out: &mut Vec<NodeId>) {
        let Some(n) = doc.node(id) else { return };
        for c in &n.children {
            let Some(cn) = doc.node(*c) else { continue };
            if !cn.visible {
                continue;
            }
            out.push(*c);
            if matches!(cn.kind, NodeKind::Section { contents_hidden: false }) && depth < 32 {
                walk(doc, *c, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(doc, page, 0, &mut out);
    out
}

/// A frame's or section's handoff status as its badge says it, with the badge colour ("Changed"
/// once a ready-for-dev layer was edited; `None` without a status).
pub(crate) fn status_label(n: &lizard_doc::Node) -> Option<(&'static str, Color32)> {
    match n.dev_status {
        lizard_doc::DevStatus::None => None,
        lizard_doc::DevStatus::ReadyForDev if n.dev_changed => Some((tr("Changed"), crate::theme::DEV_CHANGED)),
        lizard_doc::DevStatus::ReadyForDev => Some((tr("Ready for dev"), crate::theme::DEV_READY)),
        lizard_doc::DevStatus::Completed => Some((tr("Completed"), crate::theme::DEV_COMPLETED)),
    }
}

/// A frame's or section's handoff status as a small pill left-centred on `at`; the pill's area.
pub(crate) fn status_badge(painter: &egui::Painter, n: &lizard_doc::Node, at: Pos2) -> Option<Rect> {
    let (label, fill) = status_label(n)?;
    let galley = painter.layout_no_wrap(label.to_string(), semibold(10.0), Color32::WHITE);
    let r = Rect::from_min_size(pos2(at.x, at.y - galley.size().y / 2.0 - 2.0), galley.size() + vec2(10.0, 4.0));
    painter.rect_filled(r, CornerRadius::same(4), fill);
    painter.galley(r.min + vec2(5.0, 2.0), galley, Color32::WHITE);
    Some(r)
}

fn rulers(app: &LizardApp, painter: &egui::Painter, t: &Tokens, doc: &Document, sel: &[NodeId]) {
    let r = app.canvas.rect;
    let size = 20.0;
    painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), size)), CornerRadius::ZERO, t.panel);
    painter.rect_filled(Rect::from_min_size(r.min, vec2(size, r.height())), CornerRadius::ZERO, t.panel);
    let top = Rect::from_min_max(pos2(r.min.x + size, r.min.y), pos2(r.max.x, r.min.y + size));
    let side = Rect::from_min_max(pos2(r.min.x, r.min.y + size), pos2(r.min.x + size, r.max.y));
    let top_painter = painter.with_clip_rect(top);
    let side_painter = painter.with_clip_rect(side);
    let zoom = app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0).max(1e-6);
    let major_step = ruler_major_step(zoom);
    let minor_step = major_step / 5.0;
    let p0 = app.to_page(r.min);
    let selected = selection_bounds(doc, sel).map(|b| ((b.x0, b.y0), (b.x1, b.y1), app.to_screen(b.x0, b.y0), app.to_screen(b.x1, b.y1)));
    let selection_color = accent(painter);
    let selected_label_color = selection_color.gamma_multiply(0.9);
    let number_color = t.text_secondary.gamma_multiply(0.82);
    let muted_number_color = t.text_tertiary.gamma_multiply(0.85);
    let tick_color = t.text_tertiary.gamma_multiply(0.8);
    let mut top_edges = Vec::new();
    let mut side_edges = Vec::new();
    if let Some(((x0, y0), (x1, y1), a, b)) = selected {
        let fill = selection_color.gamma_multiply(0.15);
        top_painter.rect_filled(Rect::from_min_max(pos2(a.x, top.top()), pos2(b.x, top.bottom())), CornerRadius::ZERO, fill);
        side_painter.rect_filled(Rect::from_min_max(pos2(side.left(), a.y), pos2(side.right(), b.y)), CornerRadius::ZERO, fill);
        for (value, at, before) in [(x0, a.x, true), (x1, b.x, false)] {
            let galley = top_painter.layout_no_wrap(fmt(value), regular(9.0), selected_label_color);
            let x = if before { at - 5.0 - galley.size().x } else { at + 5.0 };
            let rect = Rect::from_min_size(pos2(x, top.top() + 2.0), galley.size());
            top_edges.push((rect, galley));
            top_painter.line_segment([pos2(at, top.bottom() - 7.0), pos2(at, top.bottom())], stroke(selected_label_color, 1.0));
        }
        for (value, at, before) in [(y0, a.y, true), (y1, b.y, false)] {
            let galley = side_painter.layout_no_wrap(fmt(value), regular(9.0), selected_label_color);
            let length = galley.size().x;
            let center_y = if before { at - 5.0 - length / 2.0 } else { at + 5.0 + length / 2.0 };
            let shape = vertical_ruler_label(pos2(side.left() + 7.5, center_y), galley, selected_label_color);
            side_painter.line_segment([pos2(side.right() - 7.0, at), pos2(side.right(), at)], stroke(selected_label_color, 1.0));
            side_edges.push(shape);
        }
    }
    let horizontal_ticks = ((f64::from(top.width()) / (minor_step * zoom)).ceil() as usize + 2).min(10_000);
    let first_x = (p0.x / minor_step).floor() as i64;
    for offset in 0..horizontal_ticks {
        let index = first_x.saturating_add(offset as i64);
        let value = index as f64 * minor_step;
        let x = app.to_screen(value, 0.0).x;
        if x > top.right() + 1.0 {
            break;
        }
        let major = index.rem_euclid(5) == 0;
        let tick = if major { 6.0 } else { 3.0 };
        top_painter.line_segment([pos2(x, top.bottom() - tick), pos2(x, top.bottom())], stroke(tick_color, 1.0));
        if major {
            let color = if selected.is_some_and(|(_, _, a, b)| (a.x..b.x).contains(&x)) { muted_number_color } else { number_color };
            let galley = top_painter.layout_no_wrap(fmt(value), regular(9.0), color);
            let rect = Rect::from_min_size(pos2(x - galley.size().x / 2.0, top.top() + 2.0), galley.size());
            if rect.left() >= top.left() + 2.0
                && rect.right() <= top.right() - 2.0
                && !top_edges.iter().any(|(edge, _)| edge.expand(3.0).intersects(rect))
            {
                top_painter.galley(rect.min, galley, color);
            }
        }
    }
    let vertical_ticks = ((f64::from(side.height()) / (minor_step * zoom)).ceil() as usize + 2).min(10_000);
    let first_y = (p0.y / minor_step).floor() as i64;
    for offset in 0..vertical_ticks {
        let index = first_y.saturating_add(offset as i64);
        let value = index as f64 * minor_step;
        let y = app.to_screen(0.0, value).y;
        if y > side.bottom() + 1.0 {
            break;
        }
        let major = index.rem_euclid(5) == 0;
        let tick = if major { 6.0 } else { 3.0 };
        side_painter.line_segment([pos2(side.right() - tick, y), pos2(side.right(), y)], stroke(tick_color, 1.0));
        if major {
            let color = if selected.is_some_and(|(_, _, a, b)| (a.y..b.y).contains(&y)) { muted_number_color } else { number_color };
            let galley = side_painter.layout_no_wrap(fmt(value), regular(9.0), color);
            let shape = vertical_ruler_label(pos2(side.left() + 7.5, y), galley, color);
            let bounds = shape.visual_bounding_rect();
            if bounds.top() >= side.top() + 2.0
                && bounds.bottom() <= side.bottom() - 2.0
                && !side_edges.iter().any(|edge: &egui::epaint::TextShape| edge.visual_bounding_rect().expand(3.0).intersects(bounds))
            {
                side_painter.add(Shape::Text(shape));
            }
        }
    }
    for (rect, galley) in top_edges {
        top_painter.galley(rect.min, galley, selected_label_color);
    }
    for shape in side_edges {
        side_painter.add(Shape::Text(shape));
    }
    painter.line_segment([pos2(r.min.x, r.min.y + size), pos2(r.max.x, r.min.y + size)], stroke(t.border, 1.0));
    painter.line_segment([pos2(r.min.x + size, r.min.y), pos2(r.min.x + size, r.max.y)], stroke(t.border, 1.0));
}

/// Keep labelled marks far enough apart for coordinates with four or five digits.
fn ruler_major_step(zoom: f64) -> f64 {
    let mut step = lizard_engine::guides::ruler_step(zoom);
    while step * zoom < 85.0 {
        let base = 10.0_f64.powf(step.log10().floor());
        let part = step / base;
        step = if part < 1.5 {
            2.0 * base
        } else if part < 3.5 {
            5.0 * base
        } else {
            10.0 * base
        };
    }
    step
}

fn vertical_ruler_label(center: Pos2, galley: Arc<egui::Galley>, color: Color32) -> egui::epaint::TextShape {
    egui::epaint::TextShape::new(pos2(center.x - galley.size().x / 2.0, center.y - galley.size().y / 2.0), galley, color)
        .with_angle_and_anchor(-std::f32::consts::FRAC_PI_2, Align2::CENTER_CENTER)
}

pub fn fmt(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if (r - r.round()).abs() < 1e-9 { format!("{}", r.round() as i64) } else { format!("{r}") }
}

#[cfg(test)]
#[path = "zoom_preview_tests.rs"]
mod zoom_preview_tests;

/// The canvas overlay color, adjusted for the active page background.
fn accent(p: &egui::Painter) -> Color32 {
    crate::theme::canvas_accent(p.ctx())
}

#[cfg(test)]
mod stale_render_tests {
    use std::{sync::Arc, time::Duration};

    use lizard_doc::{Document, NodeId};
    use lizard_engine::Session;
    use serde_json::json;

    use crate::layers_drag_tests::Ui;

    #[test]
    fn selected_outline_view_is_offered_to_the_gpu_backend() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::Mutex;

        struct Capture(Arc<Mutex<Vec<NodeId>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn selected_outline_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                selected: &[NodeId],
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _options: lizard_render::Outline,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.extend_from_slice(selected);
                None
            }
        }

        let mut session = Session::new();
        let created = session.execute("frame.create", &json!({"x": 10, "y": 10, "width": 80, "height": 60}))?;
        let id = lizard_engine::params::id_of(&created["id"]).ok_or("frame command returned no ID")?;
        let mut ui = Ui::new(session);
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        {
            let ds = ui.app.session.ds_mut()?;
            ds.view.outlines_selection = true;
            ds.selection = vec![id];
        }
        ui.frame(vec![]);
        let captured = seen.lock().map_err(|_| "selected outline capture lock poisoned")?;
        assert!(captured.contains(&id));
        Ok(())
    }

    #[test]
    fn focused_selected_outline_uses_gpu_at_normal_and_pixel_scale() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::Mutex;

        struct Call {
            focus: NodeId,
            selected: Vec<NodeId>,
            pixel_preview: bool,
            scale: f64,
        }
        struct Capture(Arc<Mutex<Vec<Call>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn focused_selected_outline_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                focus: NodeId,
                selected: &[NodeId],
                view: &lizard_render::View,
                _clear: [f32; 3],
                _options: lizard_render::Outline,
                _proof_srgb: bool,
                pixel_preview: bool,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.push(Call { focus, selected: selected.to_vec(), pixel_preview, scale: view.scale });
                None
            }
        }

        let mut session = Session::new();
        let created = session.execute("frame.create", &json!({"x": 10, "y": 10, "width": 80, "height": 60}))?;
        let focus = lizard_engine::params::id_of(&created["id"]).ok_or("frame command returned no ID")?;
        let mut ui = Ui::new(session);
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        {
            let ds = ui.app.session.ds_mut()?;
            ds.focus = Some(focus);
            ds.selection = vec![focus];
            ds.view.outlines_selection = true;
        }
        ui.frame(vec![]);
        {
            let ds = ui.app.session.ds_mut()?;
            ds.view.zoom = 8.0;
            ds.view.pixel_preview = true;
            ds.view.pixel_scale = 1;
        }
        ui.frame(vec![]);
        let calls = seen.lock().map_err(|_| "focused outline capture lock poisoned")?;
        assert!(calls.iter().any(|call| call.focus == focus && call.selected == [focus] && !call.pixel_preview));
        assert!(calls.iter().any(|call| call.focus == focus && call.selected == [focus] && call.pixel_preview && call.scale == 1.0));
        Ok(())
    }

    #[test]
    fn focus_view_is_offered_to_the_gpu_backend() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::Mutex;

        struct Capture(Arc<Mutex<Vec<NodeId>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn focus_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                focus: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.push(focus);
                None
            }
        }

        let mut session = Session::new();
        let created = session.execute("frame.create", &json!({"x": 10, "y": 10, "width": 80, "height": 60}))?;
        let id = lizard_engine::params::id_of(&created["id"]).ok_or("frame command returned no ID")?;
        let mut ui = Ui::new(session);
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        ui.app.session.ds_mut()?.focus = Some(id);
        ui.frame(vec![]);
        let captured = seen.lock().map_err(|_| "focus capture lock poisoned")?;
        assert!(captured.contains(&id));
        Ok(())
    }

    #[test]
    fn pixel_preview_offers_a_low_resolution_view_to_the_gpu() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::Mutex;

        struct Capture(Arc<Mutex<Vec<lizard_render::View>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn pixel_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                view: &lizard_render::View,
                _clear: [f32; 3],
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.push(view.clone());
                None
            }
        }

        let mut ui = Ui::new(Session::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        let ds = ui.app.session.ds_mut()?;
        ds.view.zoom = 8.0;
        ds.view.pixel_preview = true;
        ds.view.pixel_scale = 1;
        ui.frame(vec![]);
        let captured = seen.lock().map_err(|_| "pixel preview capture lock poisoned")?;
        assert!(captured.iter().any(|view| view.scale == 1.0 && view.width < 1440), "GPU must receive the pixel-scale view");
        Ok(())
    }

    #[test]
    fn outline_mode_is_offered_to_the_gpu_backend() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::{Arc, Mutex};

        struct Capture(Arc<Mutex<Vec<(lizard_render::Outline, bool, f64)>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn outline_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                view: &lizard_render::View,
                _clear: [f32; 3],
                options: lizard_render::Outline,
                pixel_preview: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.push((options, pixel_preview, view.scale));
                None
            }
        }

        let mut ui = Ui::new(Session::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        let ds = ui.app.session.ds_mut()?;
        ds.view.outlines = true;
        ds.view.outline_hidden = true;
        ds.view.outline_bounds = true;
        ui.frame(vec![]);
        {
            let ds = ui.app.session.ds_mut()?;
            ds.view.zoom = 8.0;
            ds.view.pixel_preview = true;
            ds.view.pixel_scale = 1;
        }
        ui.frame(vec![]);
        let captured = seen.lock().map_err(|_| "outline capture lock poisoned")?;
        assert!(captured.iter().any(|(options, pixel, _)| options.hidden && options.bounds && !pixel));
        assert!(captured.iter().any(|(options, pixel, scale)| options.hidden && options.bounds && *pixel && *scale == 1.0));
        Ok(())
    }

    #[test]
    fn focused_outline_mode_is_offered_to_the_gpu_backend() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::{Arc, Mutex};

        struct Capture(Arc<Mutex<Vec<(NodeId, bool, f64)>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn focused_outline_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                focus: NodeId,
                view: &lizard_render::View,
                _clear: [f32; 3],
                _options: lizard_render::Outline,
                pixel_preview: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.push((focus, pixel_preview, view.scale));
                None
            }
        }

        let mut ui = Ui::new(Session::new());
        let result = ui.app.session.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 30, "height": 20}))?;
        let focus = NodeId(result["id"].as_u64().ok_or("created shape id missing")?);
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        let ds = ui.app.session.ds_mut()?;
        ds.focus = Some(focus);
        ds.view.outlines = true;
        ui.frame(vec![]);
        {
            let ds = ui.app.session.ds_mut()?;
            ds.view.zoom = 8.0;
            ds.view.pixel_preview = true;
            ds.view.pixel_scale = 1;
        }
        ui.frame(vec![]);
        let captured = seen.lock().map_err(|_| "focused outline capture lock poisoned")?;
        assert!(captured.iter().any(|call| call.0 == focus && !call.1), "focused Outline should use the GPU backend");
        assert!(captured.iter().any(|call| call.0 == focus && call.1 && call.2 == 1.0));
        Ok(())
    }

    #[test]
    fn focused_pixel_preview_uses_the_gpu_pixel_scale() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::{Arc, Mutex};

        struct Capture(Arc<Mutex<Vec<(NodeId, f64)>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn focused_pixel_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                focus: NodeId,
                view: &lizard_render::View,
                _clear: [f32; 3],
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.push((focus, view.scale));
                None
            }
        }

        let mut ui = Ui::new(Session::new());
        let result = ui.app.session.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 30, "height": 20}))?;
        let focus = NodeId(result["id"].as_u64().ok_or("created shape id missing")?);
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        let ds = ui.app.session.ds_mut()?;
        ds.focus = Some(focus);
        ds.view.zoom = 8.0;
        ds.view.pixel_preview = true;
        ds.view.pixel_scale = 1;
        ui.frame(vec![]);
        let captured = seen.lock().map_err(|_| "focused pixel capture lock poisoned")?;
        assert!(captured.contains(&(focus, 1.0)), "focused Pixel Preview should use the GPU at document pixel scale");
        Ok(())
    }

    #[test]
    fn selected_outline_pixel_preview_uses_the_gpu_pixel_scale() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::{Arc, Mutex};

        struct Capture(Arc<Mutex<Vec<(NodeId, f64)>>>);
        impl super::CanvasGpuBackend for Capture {
            fn texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                _view: &lizard_render::View,
                _clear: [f32; 3],
                _gesture_active: bool,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                None
            }

            fn selected_outline_pixel_texture(
                &mut self,
                _doc: Arc<Document>,
                _page: NodeId,
                selected: &[NodeId],
                view: &lizard_render::View,
                _clear: [f32; 3],
                _options: lizard_render::Outline,
                _proof_srgb: bool,
                _ctx: &egui::Context,
            ) -> Option<egui::TextureId> {
                self.0.lock().ok()?.push((*selected.first()?, view.scale));
                None
            }
        }

        let mut ui = Ui::new(Session::new());
        let result = ui.app.session.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 30, "height": 20}))?;
        let selected = NodeId(result["id"].as_u64().ok_or("created shape id missing")?);
        let seen = Arc::new(Mutex::new(Vec::new()));
        ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&seen))));
        let ds = ui.app.session.ds_mut()?;
        ds.selection = vec![selected];
        ds.view.zoom = 8.0;
        ds.view.outlines_selection = true;
        ds.view.pixel_preview = true;
        ds.view.pixel_scale = 1;
        ui.frame(vec![]);
        let captured = seen.lock().map_err(|_| "selected pixel capture lock poisoned")?;
        assert!(captured.contains(&(selected, 1.0)), "selected Outline Pixel Preview should use GPU");
        Ok(())
    }

    #[test]
    fn current_gpu_scene_clears_the_cpu_creation_preview() -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        let created = session.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 60, "height": 40}))?;
        let id = lizard_engine::params::id_of(&created["id"]).ok_or("rectangle command returned no ID")?;
        session.ds_mut()?.selection = vec![id];
        let mut app = crate::LizardApp::new(session, Box::new(crate::NoServices));
        let ctx = egui::Context::default();
        app.canvas.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
        app.canvas.visible_view = Some(lizard_render::View { origin: (0.0, 0.0), scale: 1.0, width: 300, height: 200 });
        let frame =
            || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0))), ..Default::default() };
        let mut first = ctx.run_ui(frame(), |ui| super::paint_pending_layer(&mut app, ui.painter(), ui.ctx(), 1.0));
        first.textures_delta.clear();
        assert!(app.canvas.pending_layer.is_some(), "stale CPU canvas shows a creation preview");
        app.canvas.gpu_presented = true;
        let mut second = ctx.run_ui(frame(), |ui| super::paint_pending_layer(&mut app, ui.painter(), ui.ctx(), 1.0));
        second.textures_delta.clear();
        assert!(app.canvas.pending_layer.is_none(), "current GPU scene makes the CPU preview redundant");
        Ok(())
    }

    #[test]
    fn a_new_simple_layer_has_a_cropped_immediate_preview() -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        let created = session.execute("text.create", &json!({"text": "Pasted text", "x": 10, "y": 10}))?;
        let Some(id) = lizard_engine::params::id_of(&created["id"]) else { return Err("text command returned no ID".into()) };
        let ds = session.ds()?;
        assert!(super::simple_pending_layer(&ds.doc, ds.page, id));
        let visible = lizard_render::View { origin: (0.0, 0.0), scale: 16.0, width: 1600, height: 900 };
        let Some(crop) = super::pending_layer_view(&ds.doc, id, &visible) else { return Err("text has no visible preview".into()) };
        assert!(crop.width < visible.width && crop.height < visible.height, "a small text layer must not rasterize the whole canvas");
        session.execute("effect.add", &json!({"ids": [id], "type": "dropShadow"}))?;
        let ds = session.ds()?;
        assert!(!super::simple_pending_layer(&ds.doc, ds.page, id), "an effect needs the regular render path");
        Ok(())
    }

    #[test]
    fn deleted_text_cannot_reappear_from_an_older_render() -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        let created = session.execute("text.create", &json!({"text": "temporary", "x": 10, "y": 10}))?;
        let Some(id) = lizard_engine::params::id_of(&created["id"]) else { return Err("text command returned no ID".into()) };
        let ds = session.ds()?;
        let job = crate::render_worker::Job {
            doc: ds.doc.clone(),
            page: ds.page,
            view: lizard_render::View { origin: (0.0, 0.0), scale: 1.0, width: 256, height: 256 },
            time: 0.0,
            outline: false,
            outline_extras: Default::default(),
            outlined: Vec::new(),
            skip: Vec::new(),
            solo: None,
            proof: false,
            uid: ds.uid,
            zoom_preview: false,
            at: web_time::Instant::now(),
            texture_base: None,
            texture_view: None,
            texture_style: None,
        };
        let Some(done) = crate::render_worker::Renders::default().refine(&job) else { return Err("render failed".into()) };
        let shown = Some((job.uid, job.page));
        session.execute("edit.delete", &json!({"ids": [id]}))?;
        assert!(session.ds()?.doc.node(id).is_none(), "the delete command must remove the text");
        let mut ui = Ui::new(session);
        super::apply_render(&mut ui.app, &egui::Context::default(), done, shown);
        assert!(
            ui.app.canvas.tex_doc.as_ref().is_none_or(|doc| !Arc::ptr_eq(doc, &job.doc)),
            "the deleted text must not be uploaded from an old document snapshot"
        );
        Ok(())
    }

    #[test]
    fn a_style_change_retries_when_the_submitted_render_left_a_stale_texture() -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        let created = session.execute("shape.rectangle", &json!({"x": 40, "y": 40, "width": 80, "height": 60, "props": {"fill": "#D9D9D9"}}))?;
        let Some(rect) = lizard_engine::params::id_of(&created["id"]) else { return Err("rectangle command returned no ID".into()) };
        let mut ui = Ui::new(session);
        for _ in 0..300 {
            ui.frame(vec![]);
            if ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)), "the initial texture must be ready");
        let Some(before) = ui.app.session.ds().ok().map(|ds| ds.doc.clone()) else { return Err("no document".into()) };
        let created = ui.app.session.execute("style.create", &json!({"type": "paint", "name": "Accent", "value": "#B91C6A"}))?;
        let Some(style) = created["id"].as_str() else { return Err("style command returned no ID".into()) };
        assert!(ui.app.session.execute("style.apply", &json!({"ids": [rect], "style": style})).is_ok());
        for _ in 0..300 {
            ui.frame(vec![]);
            if ui.app.worker.as_ref().is_some_and(|worker| !worker.in_flight())
                && ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)), "applying the style must refresh the canvas");
        // A discarded render can leave the last submitted key current while the texture still
        // belongs to a prior document. The next frame must submit again and eventually recover.
        ui.app.canvas.tex_doc = Some(before.clone());
        assert!(ui.app.session.ds().ok().is_some_and(|ds| !Arc::ptr_eq(&before, &ds.doc) && !ui.app.canvas.texture_current(ds)));
        for _ in 0..300 {
            ui.frame(vec![]);
            if ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)), "the style's canvas texture must recover");
        Ok(())
    }
}
