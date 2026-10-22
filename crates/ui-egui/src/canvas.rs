//! The canvas: the rendered page plus selection, hover, guides, labels and text-editing overlays.

use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::Receiver;

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use figcraft_doc::{Document, NodeId, NodeKind};
use figcraft_engine::Preedit;
use figcraft_engine::tools::{PointerEvent, PointerKind, ToolId, selection_box, selection_frame};
use figcraft_geom::Point;
use serde_json::json;

use crate::FigApp;
use crate::i18n::{tr, trf, trn};
use crate::render_worker::{Done, Job, RenderStyle, TextureUpdate, Worker, preview_now};
use crate::theme::{ACCENT, COMPONENT, Tokens, doc_c32, regular, semibold};

pub struct CanvasState {
    pub rect: Rect,
    texture: Option<egui::TextureHandle>,
    tiles: Vec<CanvasTile>,
    tex_view: Option<figcraft_render::View>,
    /// The file ([`figcraft_engine::DocState::uid`]) and page the texture shows: it is drawn only
    /// on them.
    tex_of: Option<(u64, NodeId)>,
    /// The document version behind the texture, so stale previews cannot replace newer pixels.
    tex_doc: Option<Arc<Document>>,
    tex_style: Option<RenderStyle>,
    /// A wider, still-current view kept while a zoomed-in texture is shown. It fills newly
    /// exposed areas on zoom-out without doing a whole-view raster pass in the input frame.
    zoom_fallback: Option<ZoomFallback>,
    /// The texture is a coarse preview (see [`crate::render_worker`]).
    pub tex_coarse: bool,
    submitted: Option<JobKey>,
    /// The view of the last render asked for.
    want_view: Option<figcraft_render::View>,
    visible_view: Option<figcraft_render::View>,
    /// What the last canvas render had to do (shown by `ui.inspect`).
    pub render_work: Option<figcraft_render::Work>,
    /// Device pixels uploaded for the last completed render (zero when its texture was reused).
    pub upload_pixels: u64,
    /// From asking for a render to its preview, and its refined image, handed to egui as the
    /// canvas texture (ms; the texture is uploaded when that frame is painted).
    pub preview_latency_ms: Option<f64>,
    pub refine_latency_ms: Option<f64>,
    dragging: bool,
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
            texture: None,
            tiles: Vec::new(),
            tex_view: None,
            tex_of: None,
            tex_doc: None,
            tex_style: None,
            zoom_fallback: None,
            tex_coarse: false,
            submitted: None,
            want_view: None,
            visible_view: None,
            render_work: None,
            upload_pixels: 0,
            preview_latency_ms: None,
            refine_latency_ms: None,
            dragging: false,
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
    view: figcraft_render::View,
    of: (u64, NodeId),
    doc: Arc<Document>,
    style: RenderStyle,
}

struct CanvasTile {
    view: figcraft_render::View,
    image: Arc<egui::ColorImage>,
    texture: egui::TextureHandle,
}

impl ZoomFallback {
    fn matches(&self, job: &Job) -> bool {
        self.of == (job.uid, job.page) && Arc::ptr_eq(&self.doc, &job.doc) && self.style == job.style()
    }
}

fn view_area(view: &figcraft_render::View) -> f64 {
    f64::from(view.width) * f64::from(view.height) / view.scale / view.scale
}

impl CanvasState {
    pub(crate) fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub(crate) fn texture_current(&self, ds: &figcraft_engine::DocState) -> bool {
        (self.texture.is_some() || !self.tiles.is_empty())
            && self.tex_of == Some((ds.uid, ds.page))
            && !self.tex_coarse
            && self.tex_doc.as_ref().is_some_and(|doc| Arc::ptr_eq(doc, &ds.doc))
    }

    /// Device pixels without any current image source. This measures texture coverage, including
    /// the retained wide view, before the page background is painted underneath it.
    pub(crate) fn uncovered_pixels(&self, ds: &figcraft_engine::DocState) -> Option<u64> {
        let visible = self.visible_view.as_ref()?;
        let current = self
            .tex_view
            .as_ref()
            .filter(|_| self.tex_of == Some((ds.uid, ds.page)) && self.tex_doc.as_ref().is_some_and(|doc| Arc::ptr_eq(doc, &ds.doc)));
        let fallback = self
            .zoom_fallback
            .as_ref()
            .filter(|cached| cached.of == (ds.uid, ds.page) && Arc::ptr_eq(&cached.doc, &ds.doc) && self.tex_style.as_ref() == Some(&cached.style));
        let area = |view: &figcraft_render::View| {
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
    base_view: figcraft_render::View,
    background_view: Option<figcraft_render::View>,
    visible_view: figcraft_render::View,
    root_view: figcraft_render::View,
    detail: Option<(figcraft_render::View, egui::TextureHandle)>,
    detail_pending: Option<Receiver<(figcraft_render::View, egui::ColorImage)>>,
    start_transform: [f64; 6],
    zoom: f64,
    center: (f64, f64),
    canvas_size: egui::Vec2,
    ready: Option<(Option<egui::TextureHandle>, egui::TextureHandle)>,
    pending: Option<Receiver<(Option<egui::ColorImage>, egui::ColorImage, f64)>>,
    prepare_ms: Option<f64>,
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
            || !matches!(n.blend, figcraft_color::BlendMode::Normal | figcraft_color::BlendMode::PassThrough)
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

#[cfg(not(target_arch = "wasm32"))]
fn drag_view_for_bounds(bounds: figcraft_geom::Rect, base: &figcraft_render::View, scale: f64, pixel_limit: f64) -> Option<figcraft_render::View> {
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
    Some(figcraft_render::View { origin: (base.origin.0 + x0 / scale, base.origin.1 + y0 / scale), scale, width: w as u32, height: h as u32 })
}

#[cfg(not(target_arch = "wasm32"))]
fn cropped_drag_view(doc: &Document, root: NodeId, base: &figcraft_render::View) -> Option<figcraft_render::View> {
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
fn drag_background_view(base: &figcraft_render::View) -> Option<figcraft_render::View> {
    let pixels = f64::from(base.width) * f64::from(base.height);
    let factor = (7_900_000.0 / pixels).sqrt().min(8190.0 / f64::from(base.width)).min(8190.0 / f64::from(base.height)).min(1.0);
    if !factor.is_finite() || factor <= 0.0 {
        return None;
    }
    Some(figcraft_render::View {
        origin: base.origin,
        scale: base.scale * factor,
        width: (f64::from(base.width) * factor).ceil() as u32,
        height: (f64::from(base.height) * factor).ceil() as u32,
    })
}

/// The part of the original frame needed at the current pointer position, with a small reserve
/// around the viewport. The preview supplies pixels until this tile is ready.
#[cfg(not(target_arch = "wasm32"))]
fn drag_detail_view(visual: &DragVisual, delta: (f64, f64)) -> Option<figcraft_render::View> {
    let frame = visual.source.world_render_bounds(visual.root)?;
    let visible = visual.visible_view.page_rect();
    let needed = figcraft_geom::Rect::new(
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
        let bounds = figcraft_geom::Rect::new(
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
fn detail_covers_visible(visual: &DragVisual, detail: &figcraft_render::View, delta: (f64, f64)) -> bool {
    let Some(frame) = visual.source.world_render_bounds(visual.root) else { return false };
    let visible = visual.visible_view.page_rect();
    let needed = figcraft_geom::Rect::new(
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
fn start_drag_visual(app: &mut FigApp, ctx: &egui::Context) {
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
    let visible = figcraft_render::View {
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
    let spawned = std::thread::Builder::new().name("figcraft-drag-layers".into()).spawn(move || {
        let started = web_time::Instant::now();
        let base = if let Some(view) = background_for_job.as_ref() {
            let opts = figcraft_render::Options { skip: vec![root], lod: true, ..Default::default() };
            let Some(pix) = figcraft_render::render_view(&job_doc, page, view, &opts) else { return };
            Some(crate::render_worker::screen_image(&pix, job_doc.color_profile))
        } else {
            None
        };
        let Some(mut moving) = figcraft_render::Pixmap::new(root_for_job.width, root_for_job.height) else { return };
        figcraft_render::render_nodes_into_preview(&job_doc, &[root], &root_for_job, &mut moving, true);
        let images = (base, crate::render_worker::screen_image(&moving, job_doc.color_profile), started.elapsed().as_secs_f64() * 1000.0);
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
fn drag_visual_matches_idle_document(app: &FigApp) -> bool {
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
fn poll_drag_visual(app: &mut FigApp, ctx: &egui::Context) {
    let Some(visual) = app.canvas.drag_visual.as_mut() else { return };
    if let Some(rx) = visual.pending.as_ref() {
        match rx.try_recv() {
            Ok((base, moving, ms)) => {
                visual.ready = Some((
                    base.map(|image| ctx.load_texture("figcraft-drag-base", image, egui::TextureOptions::LINEAR)),
                    ctx.load_texture("figcraft-drag-frame", moving, egui::TextureOptions::LINEAR),
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
                visual.detail = Some((view, ctx.load_texture("figcraft-drag-detail", image, egui::TextureOptions::LINEAR)));
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
fn request_drag_detail(app: &mut FigApp, ctx: &egui::Context) {
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
    let spawned = std::thread::Builder::new().name("figcraft-drag-detail".into()).spawn(move || {
        let Some(mut pix) = figcraft_render::Pixmap::new(view.width, view.height) else { return };
        figcraft_render::render_nodes_into_preview(&source, &[root], &view, &mut pix, true);
        let image = crate::render_worker::screen_image(&pix, source.color_profile);
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

pub(crate) fn drag_mode(app: &FigApp) -> &'static str {
    #[cfg(not(target_arch = "wasm32"))]
    if app.canvas.dragging && drag_visual_delta(app).is_some() {
        return "composited";
    }
    #[cfg(target_arch = "wasm32")]
    let _ = app;
    "render"
}

pub(crate) fn drag_prepare_ms(app: &FigApp) -> Option<f64> {
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
fn drag_visual_delta(app: &FigApp) -> Option<(f64, f64)> {
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
fn paint_drag_visual(app: &FigApp, painter: &egui::Painter) -> bool {
    // An idle prepared layer must not replace the normal canvas image. After mouse-up, keep the
    // moved layer visible until the renderer catches up with the committed document.
    if !app.canvas.dragging && drag_visual_matches_idle_document(app) {
        return false;
    }
    let Some((dx, dy)) = drag_visual_delta(app) else { return false };
    let Some(visual) = app.canvas.drag_visual.as_ref() else { return false };
    let Some((base, moving)) = visual.ready.as_ref() else { return false };
    let draw = |tex: &egui::TextureHandle, view: &figcraft_render::View, delta: (f64, f64)| {
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod drag_visual_tests {
    use super::*;
    use serde_json::json;
    use tiny_skia::{PixmapPaint, Transform};

    #[test]
    fn isolated_top_frame_matches_full_page_at_rest() {
        let mut session = figcraft_engine::Session::new();
        let id = |v: serde_json::Value| figcraft_engine::params::id_of(&v["id"]).unwrap();
        session.execute("shape.rectangle", &json!({"x": 135, "y": 90, "width": 250, "height": 200, "props": {"fill": "#406080"}})).unwrap();
        let frame =
            id(session.execute("frame.create", &json!({"x": 150, "y": 100, "width": 200, "height": 150, "props": {"fill": "#203040"}})).unwrap());
        session
            .execute("shape.rectangle", &json!({"x": 20, "y": 25, "width": 80, "height": 60, "parent": frame, "props": {"fill": "#e03030"}}))
            .unwrap();
        let ds = session.ds().unwrap();
        assert!(drag_root_is_independent(&ds.doc, ds.page, frame));
        let view = figcraft_render::View { origin: (0.0, 0.0), scale: 1.0, width: 512, height: 400 };
        let opts = figcraft_render::Options { lod: true, ..Default::default() };
        let full = figcraft_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let opts = figcraft_render::Options { skip: vec![frame], lod: true, ..Default::default() };
        let mut base = figcraft_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let crop = cropped_drag_view(&ds.doc, frame, &view).unwrap();
        let mut moving = figcraft_render::Pixmap::new(crop.width, crop.height).unwrap();
        figcraft_render::render_nodes_into_preview(&ds.doc, &[frame], &crop, &mut moving, true);
        let x = ((crop.origin.0 - view.origin.0) * view.scale).round() as i32;
        let y = ((crop.origin.1 - view.origin.1) * view.scale).round() as i32;
        base.draw_pixmap(x, y, moving.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        assert_eq!(base.data(), full.data(), "separate background and frame must have the same pixels at rest");
    }

    #[test]
    fn zoomed_frame_uses_bounded_preview_and_sharp_visible_tile() {
        let mut session = figcraft_engine::Session::new();
        let id = |v: serde_json::Value| figcraft_engine::params::id_of(&v["id"]).unwrap();
        let frame =
            id(session.execute("frame.create", &json!({"x": 100, "y": 100, "width": 2048, "height": 1210, "props": {"fill": "#203040"}})).unwrap());
        session
            .execute("shape.rectangle", &json!({"parent": frame, "x": 20, "y": 30, "width": 60, "height": 80, "props": {"fill": "#e03030"}}))
            .unwrap();
        let ds = session.ds().unwrap();
        let view = figcraft_render::View { origin: (90.0, 90.0), scale: 8.0, width: 1600, height: 900 };
        let preview = cropped_drag_view(&ds.doc, frame, &view).unwrap();
        assert!(preview.scale < view.scale);
        assert!(preview.width <= 4096 && preview.height <= 4096);
        assert!(u64::from(preview.width) * u64::from(preview.height) <= 8_000_000);
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
        let opts = figcraft_render::Options { lod: true, ..Default::default() };
        let full = figcraft_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let opts = figcraft_render::Options { skip: vec![frame], lod: true, ..Default::default() };
        let mut base = figcraft_render::render_view(&ds.doc, ds.page, &view, &opts).unwrap();
        let mut moving = figcraft_render::Pixmap::new(detail.width, detail.height).unwrap();
        figcraft_render::render_nodes_into_preview(&ds.doc, &[frame], &detail, &mut moving, true);
        let x = ((detail.origin.0 - view.origin.0) * view.scale).round() as i32;
        let y = ((detail.origin.1 - view.origin.1) * view.scale).round() as i32;
        base.draw_pixmap(x, y, moving.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        assert_eq!(base.data(), full.data(), "the sharp tile must reconstruct the visible frame pixels");
    }

    #[test]
    fn large_window_background_is_bounded_without_changing_its_page_extent() {
        let base = figcraft_render::View { origin: (45.0, -20.0), scale: 2.0, width: 9000, height: 3500 };
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

impl FigApp {
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
            let (doc, range, cursor) = figcraft_engine::textedit::preedit_doc(&self.session)?;
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
    outline_extras: figcraft_render::Outline,
    outlined: Vec<NodeId>,
    skip: Vec<NodeId>,
    solo: Option<NodeId>,
    /// Text lays out anew when fonts arrive (system fonts load in the background).
    fonts: u64,
    proof: bool,
}

impl FigApp {
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

fn pointer_event(app: &FigApp, kind: PointerKind, pos: Pos2, m: &egui::Modifiers) -> PointerEvent {
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

pub fn show(app: &mut FigApp, ui: &mut Ui, worker: &mut Worker) {
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
    crate::drop_overlay(&ctx, ctx.content_rect(), tr("Drop to import"));
    crate::guides::end_released(&ctx);
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
    if !app.canvas.dragging || !composing_drag {
        request_render(app, worker, &ctx, ppp, max_texture_side);
    }
    if fast_drag
        && worker.in_flight()
        && let Some(done) = worker.poll_for(std::time::Duration::from_millis(10))
    {
        apply_render(app, &ctx, done, shown);
    }
    let painter = ui.painter_at(rect);
    let t = Tokens::get(&ctx);
    // Page background (also behind a stale/partial texture).
    let bg = app
        .session
        .ds()
        .ok()
        .and_then(|ds| match ds.doc.node(ds.page).map(|n| &n.kind) {
            Some(NodeKind::Page { background, .. }) => Some(doc_c32(&ctx, *background)),
            _ => None,
        })
        .unwrap_or(t.canvas);
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
    app.session.tool.labels = overlays(app, &painter, &t);
    // The zoom tool's area.
    if let (Some(start), Some(p)) = (app.canvas.zoom_drag, ctx.input(|i| i.pointer.hover_pos())) {
        let area = Rect::from_two_pos(start, p);
        if area.width() > 4.0 || area.height() > 4.0 {
            painter.rect_filled(area, CornerRadius::ZERO, ACCENT.gamma_multiply(0.1));
            painter.rect_stroke(area, CornerRadius::ZERO, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
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
    // The cursor: the tool's, unless something on the canvas with its own has the pointer.
    app.canvas.cursor = None;
    if own_cursor && resp.hovered() && ctx.input(|i| i.pointer.hover_pos()).is_some() {
        let panning = app.canvas.space_pan || app.canvas.middle_pan.is_some();
        let shown = crate::cursors::on_canvas(panning, ctx.input(|i| i.pointer.any_down()), app.canvas.zoom_tool, app.session.tool.overlay.cursor);
        shown.show(&ctx, &mut app.canvas.cursor_textures);
        app.canvas.cursor = Some(shown);
    }
}

fn apply_render(app: &mut FigApp, ctx: &egui::Context, done: Done, shown: Option<(u64, NodeId)>) {
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
            *slot = Some(ctx.load_texture("figcraft-canvas", image, filter));
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
                    let texture = ctx.load_texture("figcraft-canvas-tile", (*tile.image).clone(), filter);
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
pub(super) fn presented_image_rect(tl: Pos2, view: &figcraft_render::View, zoom: f64, ppp: f32, pixel_preview: bool) -> Rect {
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
pub fn describe(s: &figcraft_engine::Session) -> String {
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
fn covers(outer: &figcraft_render::View, inner: &figcraft_render::View) -> bool {
    let (o, i) = (outer.page_rect(), inner.page_rect());
    // Rounding in the snapped origin, well under a device pixel.
    let eps = 1e-6 / inner.scale.max(1e-9);
    o.x0 <= i.x0 + eps && o.y0 <= i.y0 + eps && o.x1 >= i.x1 - eps && o.y1 >= i.y1 - eps
}

fn request_render(app: &mut FigApp, worker: &mut Worker, ctx: &egui::Context, ppp: f32, max_texture_side: u32) {
    let Ok(ds) = app.session.ds() else { return };
    let rect = app.canvas.rect;
    let (mut ox, mut oy) = ds.view.origin();
    let (scale, visible_w, visible_h) = if let Some(k) = ds.view.pixel_preview_scale() {
        // Pixel preview: render `k` pixels per page unit from a whole-pixel origin, shown magnified
        // without smoothing.
        (ox, oy) = ((ox * k).floor() / k, (oy * k).floor() / k);
        let z = ds.view.zoom.max(1e-9) / k;
        (k, (f64::from(rect.width()) / z).ceil() as u32 + 2, (f64::from(rect.height()) / z).ceil() as u32 + 2)
    } else {
        (ds.view.zoom * f64::from(ppp), (rect.width() * ppp).ceil().max(1.0) as u32, (rect.height() * ppp).ceil().max(1.0) as u32)
    };
    // The edited text is drawn live (an IME composition included); the caret is an overlay.
    let doc = app.preedit_view().map(|p| p.doc.clone()).unwrap_or_else(|| ds.doc.clone());
    let outlined = if ds.view.outlines_selection { ds.selection.clone() } else { vec![] };
    let proof = ds.view.proof_srgb && !doc.color_profile.is_srgb();
    let outline_extras = figcraft_render::Outline { hidden: ds.view.outline_hidden, bounds: ds.view.outline_bounds, light: false };
    // Render beyond the visible viewport. During a pan the previous texture then still covers
    // the screen while the worker renders newly exposed content. Request a new buffer before the
    // visible area reaches its edge, rather than exposing an empty strip each pointer move.
    let margin = overscan_margin(visible_w, visible_h, max_texture_side);
    let origin = (ox, oy);
    let visible = (visible_w, visible_h);
    let visible_view = figcraft_render::View { origin, scale, width: visible.0, height: visible.1 };
    app.canvas.visible_view = Some(visible_view.clone());
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
        fonts: figcraft_text::layout_generation(),
        proof,
    };
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
    let mut job = Job {
        doc,
        page: ds.page,
        view: figcraft_render::View { origin: requested, scale, width: size.0, height: size.1 },
        outline: ds.view.outlines,
        outline_extras,
        outlined,
        skip: vec![],
        solo: key.solo,
        proof,
        uid: ds.uid,
        zoom_preview: ds.view.pixel_preview_scale().is_none(),
        at: web_time::Instant::now(),
        texture_base: if visible_texture { app.canvas.tex_doc.clone() } else { None },
        texture_view: if visible_texture { app.canvas.tex_view.clone() } else { None },
        texture_style: if visible_texture { app.canvas.tex_style.clone() } else { None },
    };
    app.canvas.want_view = Some(job.view.clone());
    // A fast zoom-out can expose page area outside the previous texture. Draw a whole-view
    // vector preview before painting this UI frame, then let the worker refine it. Otherwise the
    // newly exposed areas show only the page background for one or more frames.
    let exposed_zoom_out = app.canvas.tex_of == Some((job.uid, job.page))
        && app.canvas.tex_view.as_ref().is_some_and(|old| scale < old.scale * 0.98 && !covers(old, &visible_view));
    let fallback_covers = app.canvas.zoom_fallback.as_ref().is_some_and(|cached| cached.matches(&job) && covers(&cached.view, &visible_view));
    if exposed_zoom_out
        && job.zoom_preview
        && !fallback_covers
        && !crate::render_worker::uses_tiles(&job.view, &job.doc, job.proof)
        && let Some(preview) = preview_now(&job)
    {
        apply_render(app, ctx, preview, Some((job.uid, job.page)));
    }
    // The UI keeps its last sharp image, or made a sharp exposed-area fill above. A delayed
    // half-resolution worker preview would replace either with visibly blurred pixels.
    job.zoom_preview = false;
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
fn zoom_tool(app: &mut FigApp, ui: &Ui, resp: &egui::Response, pos: Pos2, mods: egui::Modifiers, typing: bool) -> bool {
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
fn input(app: &mut FigApp, ui: &mut Ui, resp: &egui::Response) -> bool {
    if crate::proto_canvas::input(app, ui, resp) {
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
    // Dropped files import onto the canvas, behind a card while they do ([`crate::pending_open`]).
    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    for f in dropped {
        let path = f.path().to_path_buf();
        let pt = hover_pos.map(|h| app.to_page(h)).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped.png".into());
        let params = if path.exists() {
            json!({"path": path.to_string_lossy(), "x": pt.x, "y": pt.y})
        } else if let Some(bytes) = crate::dropped_bytes(&f) {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            json!({"data": b64, "name": name, "x": pt.x, "y": pt.y})
        } else {
            continue;
        };
        let Some(target) =
            app.session.ds().ok().map(|doc| crate::pending_open::ImportTarget { uid: doc.uid, page: doc.page, name: doc.doc.name.clone() })
        else {
            continue;
        };
        app.ui.pending_opens.push(crate::pending_open::PendingOpen { label: name, params, command: "file.import", target: Some(target) });
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
fn send(app: &mut FigApp, kind: PointerKind, pos: Pos2, m: &egui::Modifiers) {
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
pub fn open_context_menu(app: &mut FigApp, pos: Pos2, deep: bool) {
    let p = app.to_page(pos);
    if let Ok(ds) = app.session.ds() {
        let (doc, page, sel, zoom) = (ds.doc.clone(), ds.page, ds.selection.clone(), ds.view.zoom.max(1e-6));
        if let Some(leaf) = app.session.hit(&doc, page, p, 2.0 / zoom) {
            let target = figcraft_doc::geometry::select_target(&doc, leaf, &sel, deep);
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
    pub layout: Arc<figcraft_text::TextLayout>,
    /// How many characters the text has.
    pub chars: usize,
    /// The layout's vertical offset in the box (vertical alignment).
    dy: f64,
    world: figcraft_geom::Affine,
}

impl TextOnScreen {
    /// Text layer `id` of `doc` (`None`: not a text layer).
    pub(crate) fn of(doc: &Document, id: NodeId) -> Option<Self> {
        let n = doc.node(id)?;
        let NodeKind::Text(t) = &n.kind else { return None };
        let (layout, dy) = figcraft_text::layout_node(n)?;
        Some(Self { layout, chars: t.char_count(), dy, world: doc.world_transform(id) })
    }

    /// Layout point `(x, y)` on screen (on the path, for text on a path).
    pub(crate) fn at(&self, app: &FigApp, x: f64, y: f64) -> Pos2 {
        let q = self.layout.warp_point(x, y);
        let p = self.world * Point::new(q.x, q.y + self.dy);
        app.to_screen(p.x, p.y)
    }

    /// The highlight of characters `a..b`, one screen quad per line.
    pub(crate) fn selection(&self, app: &FigApp, a: usize, b: usize) -> Vec<[Pos2; 4]> {
        let rects = self.layout.selection_rects(a, b);
        rects.iter().map(|r| [self.at(app, r.x0, r.y0), self.at(app, r.x1, r.y0), self.at(app, r.x1, r.y1), self.at(app, r.x0, r.y1)]).collect()
    }

    /// The caret before character `index`: its top and bottom on screen (an empty layer: a line
    /// of default height at its origin).
    pub(crate) fn caret(&self, app: &FigApp, index: usize, zoom: f64) -> [Pos2; 2] {
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
pub(crate) fn node_quad(app: &FigApp, doc: &Document, id: NodeId) -> Option<[Pos2; 4]> {
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
fn slot_badges(app: &FigApp, painter: &egui::Painter, doc: &Document, sel: &[NodeId], drop: Option<NodeId>) {
    use figcraft_engine::components::{owning_component, owning_instance, slot_name, slots_of};
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
        let (w, h) = (galley.size().x + 14.0, galley.size().y.max(11.0));
        if (q[1] - q[0]).length() < w {
            continue;
        }
        let at = q[0] - vec2(0.0, h + 2.0);
        crate::icons::paint(painter, Rect::from_min_size(pos2(at.x, at.y + (h - 11.0) / 2.0), vec2(11.0, 11.0)), crate::icons::Icon::Slot, COMPONENT);
        painter.galley(pos2(at.x + 14.0, at.y + (h - galley.size().y) / 2.0), galley, COMPONENT);
    }
}

/// The instance an Alt-drop of a component from the Assets panel swaps: tinted and outlined in
/// the component colour, with a "Swap instance" label above its top-left corner.
fn swap_highlight(app: &FigApp, painter: &egui::Painter, doc: &Document, id: NodeId) {
    let Some(q) = node_quad(app, doc, id) else { return };
    // White on the component colour, in both themes (like the canvas's selection labels).
    let ink = Color32::WHITE;
    painter.add(Shape::convex_polygon(q.to_vec(), COMPONENT.gamma_multiply(0.12), Stroke::NONE));
    quad(painter, q, stroke(COMPONENT, 2.0));
    let galley = painter.layout_no_wrap(tr("Swap instance").to_string(), semibold(10.0), ink);
    let size = vec2(galley.size().x + 26.0, 18.0);
    let top = Rect::from_points(&q).min;
    let pill = Rect::from_min_size(pos2(top.x, top.y - size.y - 4.0), size);
    painter.rect_filled(pill, CornerRadius::same(4), COMPONENT);
    crate::icons::paint(
        painter,
        Rect::from_center_size(pos2(pill.min.x + 11.0, pill.center().y), vec2(10.0, 10.0)),
        crate::icons::Icon::Instance,
        ink,
    );
    painter.galley(pos2(pill.min.x + 20.0, pill.center().y - galley.size().y / 2.0), galley, ink);
}

/// The outline colour for a layer: purple for components, sets and instances and every layer in
/// them, blue otherwise.
fn outline_colour(doc: &Document, id: NodeId) -> Color32 {
    use figcraft_engine::components::{owning_component, owning_instance};
    let set = matches!(doc.node(id).map(|n| &n.kind), Some(NodeKind::ComponentSet { .. }));
    if set || owning_component(doc, id).is_some() || owning_instance(doc, id).is_some() { COMPONENT } else { ACCENT }
}

/// The outline path of shapes (ellipses, vectors) for hover/selection outlines.
fn outline_shape(app: &FigApp, doc: &Document, id: NodeId) -> Option<Vec<Vec<Pos2>>> {
    let n = doc.node(id)?;
    if !matches!(
        n.kind,
        NodeKind::Ellipse { .. } | NodeKind::Vector { .. } | NodeKind::Polygon { .. } | NodeKind::Star { .. } | NodeKind::BooleanOp { .. }
    ) {
        return None;
    }
    let (path, _) = figcraft_render::node_outline(doc, n)?;
    let w = doc.world_transform(id);
    let mut out = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    figcraft_geom::kurbo::flatten(path.iter(), 0.5 / app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0), |el| {
        use figcraft_geom::kurbo::PathEl::*;
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
fn draw_outline(app: &FigApp, painter: &egui::Painter, doc: &Document, id: NodeId, s: Stroke) {
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
fn overlays(app: &FigApp, painter: &egui::Painter, t: &Tokens) -> Vec<(NodeId, [Point; 4])> {
    let mut labels = Vec::new();
    let Ok(ds) = app.session.ds() else { return labels };
    let doc = ds.doc.clone();
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
        if view.layout_grids && !n.grids.is_empty() && figcraft_layout::grids_shown(&doc, *c) {
            let w = doc.world_transform(*c);
            for g in n.grids.iter().filter(|g| g.visible) {
                let col = doc_c32(painter.ctx(), g.color);
                for r in figcraft_layout::grid_lines(g, n.width, n.height) {
                    let a = w * Point::new(r.x0, r.y0);
                    let b = w * Point::new(r.x1, r.y1);
                    let (sa, sb) = (app.to_screen(a.x, a.y), app.to_screen(b.x, b.y));
                    if g.kind == figcraft_doc::GridKind::Grid {
                        painter.line_segment([sa, sb], stroke(col, 1.0));
                    } else {
                        painter.rect_filled(Rect::from_two_pos(sa, sb), CornerRadius::ZERO, col);
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
        draw_outline(app, painter, &doc, h, stroke(outline_colour(&doc, h), 1.5));
    }
    // Inside a boolean group (an operand selected or in vector edit mode): every operand's outline,
    // so the shapes making the live result stay visible while one of them changes.
    let operands = figcraft_engine::booleans::outlined_operands(&doc, &sel, ds.vector_edit.as_ref().map(|ve| ve.id));
    for id in operands.iter().filter(|o| !sel.contains(o)) {
        draw_outline(app, painter, &doc, *id, stroke(outline_colour(&doc, *id).gamma_multiply(0.6), 1.0));
    }
    // Mask outlines (a mask doesn't draw itself), frame outlines and slices, as the View menu shows.
    if view.mask_outlines || view.frame_outlines || view.show_slices {
        for id in doc.subtree(page) {
            let Some(n) = doc.node(id).filter(|n| n.visible) else { continue };
            if view.mask_outlines && n.is_mask {
                draw_outline(app, painter, &doc, id, stroke(ACCENT, 1.0));
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
        quad(painter, q, stroke(if figcraft_engine::components::is_slot(&doc, d) { COMPONENT } else { ACCENT }, 1.0));
    }
    slot_badges(app, painter, &doc, &sel, ov.drop_target);
    if let Some(t) = ov.swap_target {
        swap_highlight(app, painter, &doc, t);
    }
    if let Some([x0, y0, x1, y1]) = ov.insert_line {
        painter.line_segment([app.to_screen(x0, y0), app.to_screen(x1, y1)], stroke(ACCENT, 2.0));
    }
    if let Some([x0, y0, x1, y1]) = ov.drop_cell {
        let r = Rect::from_two_pos(app.to_screen(x0, y0), app.to_screen(x1, y1));
        painter.rect_filled(r, CornerRadius::same(2), ACCENT.gamma_multiply(0.12));
        let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.add(Shape::dashed_line(&pts, stroke(ACCENT, 1.5), 4.0, 3.0));
    }
    // Vector edit mode, with the transform box around several selected points.
    // The selection's size label, which measurement labels keep clear of.
    let mut size_label = None;
    if let Some(ve) = ds.vector_edit.clone()
        && let Some(o) = figcraft_engine::vectoredit::overlay(&doc, &ve)
    {
        // The Variable width tool shows width points in place of the path's points.
        let width_tool = ve.tool == figcraft_engine::vectoredit::VTool::Width;
        // Other layers edited at once go under the active one; the hover shows on its own layer.
        let ov = &app.session.tool.overlay;
        let hover_on = |id| ov.vector_hover.filter(|_| ov.vector_hover_on.unwrap_or(ve.id) == id);
        for part in &ve.others {
            if let Some(po) = figcraft_engine::vectoredit::overlay(&doc, &ve.view(part)) {
                vector_overlay(app, painter, &po, !width_tool, hover_on(part.id), false);
            }
        }
        vector_overlay(app, painter, &o, !width_tool, hover_on(ve.id), true);
        if width_tool {
            width_overlay(app, painter, &doc, &ve, zoom);
        } else if let Some(c) = app.session.vector_edit_box() {
            let label = format!("{} × {}", fmt((c[1] - c[0]).hypot()), fmt((c[3] - c[0]).hypot()));
            size_label = transform_box(app, painter, c.map(|p| app.to_screen(p.x, p.y)), ACCENT, true, 4, label);
        }
    }
    // Selection.
    let editing = ds.text_edit.clone();
    if !sel.is_empty() && editing.is_none() && ds.vector_edit.is_none() {
        let col = if sel.iter().any(|i| outline_colour(&doc, *i) == COMPONENT) { COMPONENT } else { ACCENT };
        for id in &sel {
            draw_outline(app, painter, &doc, *id, stroke(col, 1.0));
        }
        // The shape builder reshapes by regions: no transform box.
        if app.session.tool.tool != ToolId::ShapeBuilder
            && let Some(b) = selection_box(&doc, &sel)
            && let Some((_, w, h)) = selection_frame(&doc, &sel)
        {
            let is_line = sel.len() == 1 && matches!(sel.first().and_then(|i| doc.node(*i)).map(|n| &n.kind), Some(NodeKind::Line));
            let mut label = if is_line { fmt(w) } else { format!("{} × {}", fmt(w), fmt(h)) };
            let scale = app.session.tool.overlay.scale;
            if let Some(hint) = scale {
                label = format!("{label} · {}x", fmt(hint.factor.abs()));
            }
            // A layer smaller than its handles on screen shows just its outline (they still answer
            // just outside it).
            let tiny = figcraft_engine::tools::box_sides_px(&b, zoom).1 < figcraft_engine::tools::SMALL_BOX_PX;
            // Locked layers (selected in the layers panel) can't be resized: no handles either.
            let handles = if sel.iter().any(|i| figcraft_engine::tools::locked_in(&doc, *i)) {
                0
            } else if is_line {
                2
            } else if tiny {
                0
            } else {
                4
            };
            size_label = transform_box(app, painter, b.map(|p| app.to_screen(p.x, p.y)), col, sel.len() > 1, handles, label);
            arc_overlay(app, painter, col);
            shape_overlay(app, painter, col);
            if app.session.tool.tool == ToolId::Scale {
                // The point that stays put: the dragged pivot, else the anchor.
                let pivot = scale
                    .map(|h| Point::new(h.pivot[0], h.pivot[1]))
                    .or_else(|| figcraft_engine::tools::scale_anchor_point(&doc, &sel, app.session.tool.scale_anchor));
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
        && let Some(arr) = figcraft_engine::shapebuilder::current(&app.session)
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
            quad(painter, q, stroke(ACCENT, 1.0));
        }
        let (a, b) = (te.caret.min(te.anchor), te.caret.max(te.anchor));
        if a != b && preedit.is_none() {
            for q in text.selection(app, a, b) {
                painter.add(Shape::convex_polygon(q.to_vec(), ACCENT.gamma_multiply(0.3), Stroke::NONE));
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
        painter.rect_filled(r, CornerRadius::ZERO, ACCENT.gamma_multiply(0.1));
        painter.rect_stroke(r, CornerRadius::ZERO, stroke(ACCENT, 1.0), StrokeKind::Inside);
    }
    // Pen path.
    if !ov.pen.is_empty() {
        let pts: Vec<Pos2> = ov.pen.iter().map(|[x, y]| app.to_screen(*x, *y)).collect();
        if app.session.tool.tool == ToolId::Pencil {
            painter.add(Shape::line(pts, stroke(t.text, 2.0)));
        } else {
            for [x0, y0, x1, y1] in &ov.pen_handles {
                bezier_handle(painter, app.to_screen(*x0, *y0), app.to_screen(*x1, *y1), 3.5, Color32::WHITE);
            }
            for p in &pts {
                painter.circle_filled(*p, 3.5, Color32::WHITE);
                painter.circle_stroke(*p, 3.5, stroke(ACCENT, 1.0));
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
                            stroke(ACCENT, 1.0),
                        ));
                    }
                    None => {
                        painter.line_segment([a, b], stroke(ACCENT, 1.0));
                    }
                }
            }
        }
    }
    // Eraser brush (vector edit mode).
    if let Some([x, y, r]) = ov.brush {
        let c = app.to_screen(x, y);
        let rs = (r * app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0)) as f32;
        painter.circle_stroke(c, rs, stroke(Color32::WHITE, 3.0));
        painter.circle_stroke(c, rs, stroke(ACCENT, 1.5));
    }
    // Lasso (vector edit mode).
    if ov.lasso.len() > 1 {
        let mut pts: Vec<Pos2> = ov.lasso.iter().map(|[x, y]| app.to_screen(*x, *y)).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        painter.add(Shape::dashed_line(&pts, stroke(ACCENT, 1.0), 4.0, 3.0));
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
/// corner handles and, when dimensions are shown, the size `label` under it (its box returned).
fn transform_box(app: &FigApp, painter: &egui::Painter, q: [Pos2; 4], col: Color32, outline: bool, handles: usize, label: String) -> Option<Rect> {
    if outline {
        quad(painter, q, stroke(col, 1.0));
    }
    for c in q.iter().take(handles) {
        let r = Rect::from_center_size(*c, vec2(8.0, 8.0));
        painter.rect_filled(r, CornerRadius::same(1), Color32::WHITE);
        painter.rect_stroke(r, CornerRadius::same(1), stroke(col, 1.0), StrokeKind::Inside);
    }
    if app.session.prefs.show_dimensions {
        let bottom = q.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        let cx = q.iter().map(|p| p.x).sum::<f32>() / 4.0;
        return Some(pill(painter, pos2(cx, bottom + 9.0), Align2::CENTER_TOP, label, col));
    }
    None
}

/// A canvas readout: white `text` on a `col` pill placed at `at` by `align` (its box returned).
fn pill(painter: &egui::Painter, at: Pos2, align: Align2, text: String, col: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text, semibold(11.0), Color32::WHITE);
    let r = align.anchor_size(at, galley.size() + vec2(8.0, 4.0));
    painter.rect_filled(r, CornerRadius::same(3), col);
    painter.galley(r.min + vec2(4.0, 2.0), galley, Color32::WHITE);
    r
}

/// The selected ellipse's arc handles (the hovered or dragged one larger, Start with a dot inside)
/// and, while one is dragged, its readout beside it.
fn arc_overlay(app: &FigApp, painter: &egui::Painter, col: Color32) {
    use figcraft_engine::arc_handles::ArcHandle;
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
fn shape_overlay(app: &FigApp, painter: &egui::Painter, col: Color32) {
    use figcraft_engine::shape_handles::ShapeHandle;
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
fn width_overlay(app: &FigApp, painter: &egui::Painter, doc: &Document, ve: &figcraft_engine::vectoredit::VectorEdit, zoom: f64) {
    use figcraft_doc::WidthSide;
    use figcraft_engine::vectoredit::{WHit, width_handles, width_preview};
    let hover = app.session.tool.overlay.width_hover;
    let sp = |p: &[f64; 2]| app.to_screen(p[0], p[1]);
    let draw = |h: &figcraft_engine::vectoredit::WidthHandle, ghost: bool| {
        let (c, a, b) = (sp(&h.at), sp(&h.sides[0]), sp(&h.sides[1]));
        let col = if ghost { ACCENT.gamma_multiply(0.5) } else { ACCENT };
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
        painter.circle_filled(c, r, if on { ACCENT } else { Color32::WHITE });
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
    painter.line_segment([v, e], stroke(ACCENT, 1.0));
    painter.circle_filled(e, r, fill);
    painter.circle_stroke(e, r, stroke(ACCENT, 1.0));
}

/// Vector edit mode: a layer's segments, handles and points (`points`) with its `hover`ed part, and
/// for the `active` layer the closed area under the paint bucket and the pen's rubber band.
fn vector_overlay(
    app: &FigApp,
    painter: &egui::Painter,
    o: &figcraft_engine::vectoredit::EditOverlay,
    points: bool,
    hover: Option<figcraft_engine::vectoredit::VHit>,
    active: bool,
) {
    use figcraft_engine::vectoredit::VHit;
    let ov = &app.session.tool.overlay;
    let sp = |p: &[f64; 2]| app.to_screen(p[0], p[1]);
    // Paint bucket target: hatched like a fill to come.
    if active && !ov.face.is_empty() {
        let polys: Vec<Vec<Pos2>> = ov.face.iter().map(|poly| poly.iter().map(sp).collect()).collect();
        hatched(painter, &polys, ACCENT);
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
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke([a, b, c, d], false, Color32::TRANSPARENT, stroke(ACCENT, w)));
    }
    if !points {
        return;
    }
    for h in &o.handles {
        let hovered = hover == Some(VHit::Handle { seg: h.seg, start: h.start });
        let on = o.selected_handles.contains(&(h.seg, h.start));
        bezier_handle(painter, sp(&h.vertex), sp(&h.handle), if hovered { 4.5 } else { 3.5 }, if on { ACCENT } else { Color32::WHITE });
    }
    for (i, v) in o.vertices.iter().enumerate() {
        let p = sp(v);
        let hovered = matches!(hover, Some(VHit::Vertex { index }) if index == i);
        let size = if hovered { 9.0 } else { 7.0 };
        let r = Rect::from_center_size(p, vec2(size, size));
        let on = o.selected_vertices.contains(&i);
        painter.rect_filled(r, CornerRadius::same(5), if on { ACCENT } else { Color32::WHITE });
        painter.rect_stroke(r, CornerRadius::same(5), stroke(ACCENT, 1.0), StrokeKind::Inside);
        if o.anchor == Some(i) {
            painter.circle_stroke(p, 7.5, stroke(ACCENT, 1.0));
        }
    }
    if let Some([x0, y0, x1, y1]) = ov.rubber.filter(|_| active) {
        painter.add(Shape::dashed_line(&[app.to_screen(x0, y0), app.to_screen(x1, y1)], stroke(ACCENT, 1.0), 4.0, 3.0));
    }
}

/// Width of the lines hatching the hovered and selected regions in vector edit mode (screen px).
const REGION_HATCH_WIDTH: f32 = 1.4;

/// Vector edit mode: the selected regions and the hovered one hatched in the selection colour,
/// under the segments and points (the engine lays out and clips the lines).
fn region_hatch(app: &FigApp, painter: &egui::Painter) {
    let Ok(ds) = app.session.ds() else { return };
    let (ox, oy) = ds.view.page_to_screen(0.0, 0.0);
    let (z, min) = (ds.view.zoom, app.canvas.rect.min);
    let view = figcraft_geom::Affine::new([z, 0.0, 0.0, z, f64::from(min.x) + ox, f64::from(min.y) + oy]);
    let r = painter.clip_rect();
    let clip = figcraft_geom::Rect::new(f64::from(r.min.x), f64::from(r.min.y), f64::from(r.max.x), f64::from(r.max.y));
    for [a, b] in app.session.vector_region_hatch(view, Some(clip)) {
        painter.line_segment([pos2(a.x as f32, a.y as f32), pos2(b.x as f32, b.y as f32)], stroke(ACCENT, REGION_HATCH_WIDTH));
    }
}

/// Shape builder: every region's edges, and the hovered or collected regions hatched (in the
/// danger colour when releasing removes them).
fn builder_overlay(app: &FigApp, painter: &egui::Painter, t: &Tokens, arr: &figcraft_engine::shapebuilder::Arrangement) {
    let ov = &app.session.tool.overlay;
    let screen = |r: &figcraft_engine::shapebuilder::Region| -> Vec<Vec<Pos2>> {
        r.polys.iter().map(|poly| poly.iter().map(|p| app.to_screen(p.x, p.y)).collect()).collect()
    };
    // Edges read on any fill: a light halo under a thin accent line.
    for r in &arr.regions {
        for poly in screen(r) {
            painter.add(Shape::closed_line(poly.clone(), stroke(Color32::from_white_alpha(150), 2.5)));
            painter.add(Shape::closed_line(poly, stroke(ACCENT, 1.0)));
        }
    }
    let col = if ov.builder_remove { t.danger } else { ACCENT };
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
pub(crate) fn status_label(n: &figcraft_doc::Node) -> Option<(&'static str, Color32)> {
    match n.dev_status {
        figcraft_doc::DevStatus::None => None,
        figcraft_doc::DevStatus::ReadyForDev if n.dev_changed => Some((tr("Changed"), crate::theme::DEV_CHANGED)),
        figcraft_doc::DevStatus::ReadyForDev => Some((tr("Ready for dev"), crate::theme::DEV_READY)),
        figcraft_doc::DevStatus::Completed => Some((tr("Completed"), crate::theme::DEV_COMPLETED)),
    }
}

/// A frame's or section's handoff status as a small pill left-centred on `at`; the pill's area.
pub(crate) fn status_badge(painter: &egui::Painter, n: &figcraft_doc::Node, at: Pos2) -> Option<Rect> {
    let (label, fill) = status_label(n)?;
    let galley = painter.layout_no_wrap(label.to_string(), semibold(10.0), Color32::WHITE);
    let r = Rect::from_min_size(pos2(at.x, at.y - galley.size().y / 2.0 - 2.0), galley.size() + vec2(10.0, 4.0));
    painter.rect_filled(r, CornerRadius::same(4), fill);
    painter.galley(r.min + vec2(5.0, 2.0), galley, Color32::WHITE);
    Some(r)
}

fn rulers(app: &FigApp, painter: &egui::Painter, t: &Tokens, doc: &Document, sel: &[NodeId]) {
    let r = app.canvas.rect;
    let size = 20.0;
    painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), size)), CornerRadius::ZERO, t.panel);
    painter.rect_filled(Rect::from_min_size(r.min, vec2(size, r.height())), CornerRadius::ZERO, t.panel);
    let zoom = app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0);
    let step = figcraft_engine::guides::ruler_step(zoom);
    let p0 = app.to_page(r.min);
    let p1 = app.to_page(r.max);
    // Highlight the selection's span.
    if let Some(b) = doc.bounds_of(sel) {
        let (a, bb) = (app.to_screen(b.x0, b.y0), app.to_screen(b.x1, b.y1));
        painter.rect_filled(Rect::from_min_max(pos2(a.x, r.min.y), pos2(bb.x, r.min.y + size)), CornerRadius::ZERO, ACCENT.gamma_multiply(0.15));
        painter.rect_filled(Rect::from_min_max(pos2(r.min.x, a.y), pos2(r.min.x + size, bb.y)), CornerRadius::ZERO, ACCENT.gamma_multiply(0.15));
    }
    let mut x = (p0.x / step).floor() * step;
    while x <= p1.x {
        let s = app.to_screen(x, 0.0).x;
        painter.line_segment([pos2(s, r.min.y + size - 6.0), pos2(s, r.min.y + size)], stroke(t.text_tertiary, 1.0));
        painter.text(pos2(s + 2.0, r.min.y + 3.0), Align2::LEFT_TOP, fmt(x), regular(9.0), t.text_secondary);
        x += step;
    }
    let mut y = (p0.y / step).floor() * step;
    while y <= p1.y {
        let s = app.to_screen(0.0, y).y;
        painter.line_segment([pos2(r.min.x + size - 6.0, s), pos2(r.min.x + size, s)], stroke(t.text_tertiary, 1.0));
        painter.text(pos2(r.min.x + 3.0, s + 2.0), Align2::LEFT_TOP, fmt(y), regular(9.0), t.text_secondary);
        y += step;
    }
    painter.line_segment([pos2(r.min.x, r.min.y + size), pos2(r.max.x, r.min.y + size)], stroke(t.border, 1.0));
    painter.line_segment([pos2(r.min.x + size, r.min.y), pos2(r.min.x + size, r.max.y)], stroke(t.border, 1.0));
}

pub fn fmt(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if (r - r.round()).abs() < 1e-9 { format!("{}", r.round() as i64) } else { format!("{r}") }
}

#[cfg(test)]
#[path = "zoom_preview_tests.rs"]
mod zoom_preview_tests;
