//! Canvas rendering off the UI thread (desktop) or inline (web). The latest request wins (queued
//! older ones are skipped); the UI shows each finished image, re-projected to the current view,
//! until the next one arrives.
//! Small viewports keep a whole image ([`figcraft_render::Retained`]): a pan renders only the
//! uncovered strips and an edit only the areas of the layers that changed. Wide viewports keep
//! page-aligned regions, rendered in parallel and composed as separate GPU textures. The UI keeps
//! its current image during zoom rather than displaying the worker's lower-resolution preview.
//! Direct worker callers may still request that preview through [`Job::zoom_preview`].

use std::sync::Arc;

use figcraft_color::ColorProfile;
use figcraft_doc::{Document, NodeId};
use figcraft_render::{DamageRect, Pixmap, Retained, View, Work};
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;
// A monotonic clock in the browser too: preview decisions rest on how long renders take there.
use web_time::Instant;

/// A preview has this many times fewer pixels each way than the view it stands in for.
pub const PREVIEW_FACTOR: f64 = 2.0;
/// Full renders quicker than this (about a frame) show without a preview first.
pub const PREVIEW_AFTER_MS: f64 = 12.0;
/// Wide Retina canvases are split into GPU textures of this size instead of one viewport image.
pub const TILE_SIDE: u32 = 512;
const TILED_MIN_PIXELS: u64 = 4_000_000;
const HIGH_ZOOM_TILED_MIN_PIXELS: u64 = 1_000_000;
/// Keep only a few recently visited full views. Large canvases already use tiles and do not
/// benefit from retaining several complete viewport images in CPU memory.
const ZOOM_CACHE_MAX_PIXELS: u64 = 3_000_000;
const ZOOM_CACHE_MAX_VIEWS: usize = 2;

pub fn uses_tiles(view: &View, doc: &Document, proof: bool) -> bool {
    let pixels = u64::from(view.width) * u64::from(view.height);
    // Each tile would otherwise build and blur the same padded layer again. A broad shadow can
    // cost more across 16 tiles than one viewport render, even with parallel workers.
    let effect_reach = doc.nodes().filter(|n| n.visible).map(figcraft_render::draw::effect_margin).fold(0.0, f64::max) * view.scale;
    (pixels >= TILED_MIN_PIXELS || view.scale >= 8.0 && pixels >= HIGH_ZOOM_TILED_MIN_PIXELS)
        && !proof
        && effect_reach <= f64::from(TILE_SIDE)
        // A huge backdrop kernel would make every tile redraw most of its neighbours.
        && doc.max_backdrop_radius() * view.scale * 1.5 + 3.0 <= 256.0
        && tile_views(view).is_some()
}

#[derive(Clone)]
pub struct RenderTile {
    pub view: View,
    pub image: Arc<egui::ColorImage>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RenderStyle {
    outline: bool,
    outline_extras: figcraft_render::Outline,
    outlined: Vec<NodeId>,
    skip: Vec<NodeId>,
    solo: Option<NodeId>,
    proof: bool,
}

#[derive(Clone)]
pub struct Job {
    pub doc: Arc<Document>,
    pub page: NodeId,
    pub view: View,
    /// Animation time, quantized by the canvas clock. Zero for static scenes.
    pub time: f64,
    pub outline: bool,
    /// Outline mode's options (hidden layers, object bounds).
    pub outline_extras: figcraft_render::Outline,
    /// Layers drawn as outlines (outlines for the selection).
    pub outlined: Vec<NodeId>,
    pub skip: Vec<NodeId>,
    /// Only this layer (the focus view).
    pub solo: Option<NodeId>,
    /// Soft-proof sRGB: stripe the colours sRGB can't show.
    pub proof: bool,
    /// The document's [`figcraft_engine::DocState::uid`]: images of other files are never shown.
    pub uid: u64,
    /// A coarse preview may come first when the zoom changes (not in pixel preview, whose pixels
    /// are the point).
    pub zoom_preview: bool,
    /// When the UI asked for it: how long the image took to reach the screen.
    pub at: Instant,
    /// The full-resolution texture currently shown by the UI, if any. A patch is sent only when
    /// the renderer's retained base is that same document and view.
    pub texture_base: Option<Arc<Document>>,
    pub texture_view: Option<View>,
    pub texture_style: Option<RenderStyle>,
}

impl Job {
    pub(crate) fn style(&self) -> RenderStyle {
        RenderStyle {
            outline: self.outline,
            outline_extras: self.outline_extras,
            outlined: self.outlined.clone(),
            skip: self.skip.clone(),
            solo: self.solo,
            proof: self.proof,
        }
    }
}

pub enum TextureUpdate {
    Full(egui::ColorImage),
    Partial(Vec<([usize; 2], egui::ColorImage)>),
    Reused,
    Tiles(Vec<RenderTile>),
}

pub struct Done {
    /// Pixel data to send to the texture. Edits at the same view convert only damaged regions.
    pub update: TextureUpdate,
    /// Full image for pixel-equivalence assertions in renderer tests.
    #[cfg(test)]
    pub image: egui::ColorImage,
    /// Either document the patch can update: the texture seen when submitted and the previous
    /// render that may have reached the UI while this job was running.
    pub base_docs: Vec<Arc<Document>>,
    /// The document rendered, so a later preview cannot replace newer pixels.
    pub doc: Arc<Document>,
    /// The view the image shows (the requested one snapped outwards to whole device pixels).
    pub view: View,
    pub style: RenderStyle,
    pub ms: f64,
    pub work: Work,
    /// What it is an image of ([`Job::uid`], [`Job::page`]).
    pub uid: u64,
    pub page: NodeId,
    /// A coarse preview, refined by a later image unless a newer request replaced it.
    pub coarse: bool,
    /// [`Job::at`].
    pub at: Instant,
}

/// A render of a document in `profile` as a texture image for the screen (sRGB).
pub fn screen_image(pix: &Pixmap, profile: ColorProfile) -> egui::ColorImage {
    let size = [pix.width() as usize, pix.height() as usize];
    if profile.is_srgb() {
        return egui::ColorImage::from_rgba_premultiplied(size, pix.data());
    }
    let mut data = pix.data().to_vec();
    profile.conversion_to(ColorProfile::Srgb).apply_pixels(&mut data);
    egui::ColorImage::from_rgba_premultiplied(size, &data)
}

/// Soft proof: grey diagonal stripes over the pixels whose colours sRGB can't show.
fn mark_outside_srgb(image: &mut egui::ColorImage, pix: &Pixmap, profile: ColorProfile) {
    let w = image.size[0].max(1);
    let clipped = profile.conversion_to(ColorProfile::Srgb).clipped_pixels(pix.data());
    for (i, (px, out)) in image.pixels.iter_mut().zip(clipped).enumerate() {
        if out && ((i % w) + (i / w)) % 8 < 4 {
            *px = egui::Color32::from_gray(128);
        }
    }
}

fn patch_image(pix: &Pixmap, profile: ColorProfile, r: DamageRect) -> Option<([usize; 2], egui::ColorImage)> {
    let (x, y, w, h) = (r.x as usize, r.y as usize, r.width as usize, r.height as usize);
    let stride = pix.width() as usize;
    if w == 0 || h == 0 || x.checked_add(w)? > stride || y.checked_add(h)? > pix.height() as usize {
        return None;
    }
    let mut rgba = Vec::with_capacity(w.checked_mul(h)?.checked_mul(4)?);
    for row in y..y + h {
        let start = row.checked_mul(stride)?.checked_add(x)?.checked_mul(4)?;
        rgba.extend_from_slice(pix.data().get(start..start + w * 4)?);
    }
    if !profile.is_srgb() {
        profile.conversion_to(ColorProfile::Srgb).apply_pixels(&mut rgba);
    }
    Some(([x, y], egui::ColorImage::from_rgba_premultiplied([w, h], &rgba)))
}

fn merge_damage(mut regions: Vec<DamageRect>) -> Vec<DamageRect> {
    let mut i = 0;
    while i < regions.len() {
        let mut merged = false;
        let mut j = i + 1;
        while j < regions.len() {
            let a = regions[i];
            let b = regions[j];
            let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
            let (x1, y1) =
                (a.x.saturating_add(a.width).max(b.x.saturating_add(b.width)), a.y.saturating_add(a.height).max(b.y.saturating_add(b.height)));
            let union_area = u64::from(x1 - x0) * u64::from(y1 - y0);
            let separate_area = u64::from(a.width) * u64::from(a.height) + u64::from(b.width) * u64::from(b.height);
            if union_area <= separate_area {
                regions[i] = DamageRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
                regions.swap_remove(j);
                merged = true;
            } else {
                j += 1;
            }
        }
        if !merged {
            i += 1;
        }
    }
    regions
}

fn same_view(a: &View, b: &View) -> bool {
    a.origin.0.to_bits() == b.origin.0.to_bits()
        && a.origin.1.to_bits() == b.origin.1.to_bits()
        && a.scale.to_bits() == b.scale.to_bits()
        && a.width == b.width
        && a.height == b.height
}

/// Grid cells have stable page coordinates at one scale. Panning across a cell boundary can
/// therefore keep the existing GPU textures and render only the newly visible column or row.
fn tile_views(view: &View) -> Option<(View, Vec<View>)> {
    let scale = view.scale;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let side = f64::from(TILE_SIDE);
    let (x0, y0) = ((view.origin.0 * scale / side).floor(), (view.origin.1 * scale / side).floor());
    let (x1, y1) =
        (((view.origin.0 * scale + f64::from(view.width)) / side).ceil(), ((view.origin.1 * scale + f64::from(view.height)) / side).ceil());
    if ![x0, y0, x1, y1].iter().all(|n| n.is_finite() && n.abs() < 1e12) {
        return None;
    }
    let (nx, ny) = ((x1 - x0) as u32, (y1 - y0) as u32);
    if nx == 0 || ny == 0 || u64::from(nx) * u64::from(ny) > 512 {
        return None;
    }
    let mut tiles = Vec::with_capacity((nx * ny) as usize);
    for y in 0..ny {
        for x in 0..nx {
            tiles.push(View {
                origin: ((x0 + f64::from(x)) * side / scale, (y0 + f64::from(y)) * side / scale),
                scale,
                width: TILE_SIDE,
                height: TILE_SIDE,
            });
        }
    }
    let covered = View { origin: (x0 * side / scale, y0 * side / scale), scale, width: nx * TILE_SIDE, height: ny * TILE_SIDE };
    Some((covered, tiles))
}

fn render_tile(job: &Job, view: &View) -> Option<Arc<egui::ColorImage>> {
    let bleed = (job.doc.max_backdrop_radius() * view.scale * 1.5 + 3.0).ceil().clamp(0.0, 256.0) as u32;
    let expanded = View {
        origin: (view.origin.0 - f64::from(bleed) / view.scale, view.origin.1 - f64::from(bleed) / view.scale),
        scale: view.scale,
        width: view.width + bleed * 2,
        height: view.height + bleed * 2,
    };
    let opts = figcraft_render::Options {
        time: job.time,
        outline_mode: job.outline,
        outline: job.outline_extras,
        outlined: job.outlined.clone(),
        skip: job.skip.clone(),
        solo: job.solo,
        lod: true,
        ..Default::default()
    };
    let pix = figcraft_render::render_view(&job.doc, job.page, &expanded, &opts)?;
    let (_, image) = patch_image(&pix, job.doc.color_profile, DamageRect { x: bleed, y: bleed, width: TILE_SIDE, height: TILE_SIDE })?;
    Some(Arc::new(image))
}

/// Render `view` of the job (its own view, or the coarse one standing in for it).
fn run(job: &Job, view: &View, retained: &mut Retained, coarse: bool, skip_effects: bool, base_doc: Option<Arc<Document>>) -> Option<Done> {
    let t0 = Instant::now();
    // The canvas is a preview: what's too small to see in detail at this zoom is approximated.
    let opts = figcraft_render::Options {
        time: job.time,
        outline_mode: job.outline,
        outline: job.outline_extras,
        outlined: job.outlined.clone(),
        skip: job.skip.clone(),
        solo: job.solo,
        lod: true,
        skip_effects,
        ..Default::default()
    };
    let (view, pix, work, damage) = retained.render_with_damage(&job.doc, job.page, view, &opts)?;
    let style = job.style();
    let same_texture_view =
        !coarse && !job.proof && job.texture_style.as_ref() == Some(&style) && job.texture_view.as_ref().is_some_and(|shown| same_view(shown, &view));
    let mut base_docs = Vec::new();
    let mut regions = Vec::new();
    if same_texture_view {
        if let Some(shown) = &job.texture_base {
            let from_shown = if base_doc.as_ref().is_some_and(|base| Arc::ptr_eq(base, shown)) {
                damage.clone()
            } else if work != Work::Full {
                figcraft_render::retained::damage_between(shown, &job.doc, job.page, &view)
            } else {
                None
            };
            if let Some(found) = from_shown {
                regions.extend(found);
                base_docs.push(shown.clone());
            }
        }
        if let Some(previous) = &base_doc
            && !base_docs.iter().any(|base| Arc::ptr_eq(base, previous))
            && let Some(found) = damage
        {
            regions.extend(found);
            base_docs.push(previous.clone());
        }
    }
    // A patch against only the worker's latest image cannot update an older image still on
    // screen. Some edits (including changes to style definitions) have no safe damage map from
    // the visible document, even though the worker can reuse its own newer render.
    let visible_base_supported = job.texture_base.as_ref().is_some_and(|shown| base_docs.iter().any(|base| Arc::ptr_eq(base, shown)));
    let update = if same_texture_view && visible_base_supported {
        let regions = merge_damage(regions);
        if regions.is_empty() {
            TextureUpdate::Reused
        } else if regions.iter().map(|r| u64::from(r.width) * u64::from(r.height)).sum::<u64>() * 5
            > u64::from(pix.width()) * u64::from(pix.height()) * 3
        {
            TextureUpdate::Full(screen_image(pix, job.doc.color_profile))
        } else {
            match regions.into_iter().map(|r| patch_image(pix, job.doc.color_profile, r)).collect::<Option<Vec<_>>>() {
                Some(patches) => TextureUpdate::Partial(patches),
                None => TextureUpdate::Full(screen_image(pix, job.doc.color_profile)),
            }
        }
    } else {
        let mut image = screen_image(pix, job.doc.color_profile);
        if job.proof {
            mark_outside_srgb(&mut image, pix, job.doc.color_profile);
        }
        TextureUpdate::Full(image)
    };
    #[cfg(test)]
    let mut image = screen_image(pix, job.doc.color_profile);
    #[cfg(test)]
    if job.proof {
        mark_outside_srgb(&mut image, pix, job.doc.color_profile);
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    Some(Done {
        update,
        #[cfg(test)]
        image,
        base_docs,
        doc: job.doc.clone(),
        view,
        style,
        ms,
        work,
        uid: job.uid,
        page: job.page,
        coarse,
        at: job.at,
    })
}

/// `view` with [`PREVIEW_FACTOR`] times fewer pixels each way, showing at least the same area.
pub fn coarse_view(view: &View) -> View {
    let shrink = |n: u32| (f64::from(n) / PREVIEW_FACTOR).ceil().max(1.0) as u32;
    View { origin: view.origin, scale: view.scale / PREVIEW_FACTOR, width: shrink(view.width), height: shrink(view.height) }
}

/// At a large zoom, enlarging a previous texture is especially noticeable. Draw the page's
/// geometry at the requested device resolution first, then replace it with the full effect pass.
fn sharp_geometry_preview(job: &Job) -> bool {
    !job.proof
        && !job.outline
        && job.view.scale >= 4.0
        && job.doc.nodes().any(|node| node.visible && node.effects.iter().any(|effect| effect.visible))
}

/// What the render thread keeps between jobs: the canvas image, the previews standing in for it
/// while the zoom changes, and how long the last full render took.
#[derive(Clone)]
struct CachedZoom {
    scale: u64,
    pixels: u64,
    view: View,
    retained: Retained,
}

#[derive(Clone)]
pub struct Renders {
    full: Retained,
    zoom_cache: Vec<CachedZoom>,
    full_style: Option<RenderStyle>,
    full_time: u64,
    full_pixels: u64,
    full_view: Option<View>,
    preview: Retained,
    tiles: Vec<RenderTile>,
    tile_doc: Option<Arc<Document>>,
    tile_style: Option<RenderStyle>,
    tile_page: Option<(u64, NodeId)>,
    tile_time: u64,
    /// The last full render's document, page and scale.
    last: Option<(u64, NodeId, f64)>,
    last_doc: Option<Arc<Document>>,
    /// How long the last render from scratch took.
    full_ms: f64,
    /// Previews come first only when renders from scratch take at least this long.
    preview_after_ms: f64,
}

impl Default for Renders {
    fn default() -> Self {
        Self::with_threshold(PREVIEW_AFTER_MS)
    }
}

impl Renders {
    /// Previews when renders from scratch take at least `ms` (tests: 0, always).
    pub fn with_threshold(ms: f64) -> Self {
        Self {
            full: Retained::default(),
            zoom_cache: Vec::new(),
            full_style: None,
            full_time: 0,
            full_pixels: 0,
            full_view: None,
            preview: Retained::default(),
            tiles: Vec::new(),
            tile_doc: None,
            tile_style: None,
            tile_page: None,
            tile_time: 0,
            last: None,
            last_doc: None,
            full_ms: 0.0,
            preview_after_ms: ms,
        }
    }

    /// Only a document edit at the same scale can reuse the retained pixels after cancellation.
    /// Cloning a whole viewport on every zoom step doubles peak image memory for no benefit.
    #[cfg(not(target_arch = "wasm32"))]
    fn needs_cancel_snapshot(&self, job: &Job) -> bool {
        self.full_ms > 100.0
            && self.last.is_some_and(|(uid, page, scale)| uid == job.uid && page == job.page && scale.to_bits() == job.view.scale.to_bits())
            && self.last_doc.as_ref().is_some_and(|old| !Arc::ptr_eq(old, &job.doc))
            && !uses_tiles(&job.view, &job.doc, job.proof)
    }

    /// Whether `job` gets a preview first. A nearby zoom whose currently visible, full-resolution
    /// texture covers the requested area keeps that texture while the vector render catches up.
    /// Replacing it with a lower-resolution preview makes a smooth pinch visibly pixelated.
    fn wants_preview(&self, job: &Job) -> bool {
        let sharp = sharp_geometry_preview(job);
        if !job.zoom_preview || !sharp && (self.full_ms < self.preview_after_ms || uses_tiles(&job.view, &job.doc, job.proof)) {
            return false;
        }
        // A complete image at this scale is already waiting in the zoom cache. A coarse render
        // would delay it and briefly replace sharp pixels with blurry ones.
        if self.last.is_some_and(|(uid, page, _)| uid == job.uid && page == job.page)
            && self.last_doc.as_ref().is_some_and(|old| Arc::ptr_eq(old, &job.doc))
            && self.full_style.as_ref() == Some(&job.style())
            && self.full_time == job.time.to_bits()
            && figcraft_render::retained::snap(&job.view).is_some_and(|(requested, _)| {
                self.zoom_cache.iter().any(|entry| entry.scale == job.view.scale.to_bits() && same_view(&entry.view, &requested))
            })
        {
            return false;
        }
        let changed = self.last.is_some_and(|(uid, page, scale)| uid != job.uid || page != job.page || scale.to_bits() != job.view.scale.to_bits());
        if !(changed || sharp && self.last.is_none()) {
            return false;
        }
        if sharp {
            return true;
        }
        let old_covers = job.texture_base.as_ref().is_some_and(|old| Arc::ptr_eq(old, &job.doc))
            && job.texture_style.as_ref() == Some(&job.style())
            && job.texture_view.as_ref().is_some_and(|old| {
                let ratio = job.view.scale / old.scale;
                // The cached image has at least as many source pixels as a half-resolution
                // preview would at this zoom. Reproject it until the fresh vector render lands.
                if !(0.0..=PREVIEW_FACTOR).contains(&ratio) {
                    return false;
                }
                let (a, b) = (old.page_rect(), job.view.page_rect());
                let eps = 1e-6 / job.view.scale.max(1e-9);
                a.x0 <= b.x0 + eps && a.y0 <= b.y0 + eps && a.x1 >= b.x1 - eps && a.y1 >= b.y1 - eps
            });
        !old_covers
    }

    /// The coarse preview of the whole of `job`'s view, when one should come first.
    pub fn preview(&mut self, job: &Job) -> Option<Done> {
        if !self.wants_preview(job) {
            return None;
        }
        let sharp = sharp_geometry_preview(job);
        let view = if sharp { job.view.clone() } else { coarse_view(&job.view) };
        guarded(&mut self.preview, |r| run(job, &view, r, true, sharp, None))
    }

    /// The image of `job`'s view, built on the kept one where it can be.
    pub fn refine(&mut self, job: &Job) -> Option<Done> {
        if uses_tiles(&job.view, &job.doc, job.proof) {
            self.full.clear();
            self.zoom_cache.clear();
            self.full_style = None;
            self.full_pixels = 0;
            self.full_view = None;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.refine_tiles(job)));
            return match result {
                Ok(done) => done,
                Err(_) => {
                    self.tiles.clear();
                    self.tile_doc = None;
                    self.tile_style = None;
                    self.tile_page = None;
                    None
                }
            };
        }
        self.tiles.clear();
        self.tile_doc = None;
        self.tile_style = None;
        self.tile_page = None;
        let style = job.style();
        let same_content = self.last.is_some_and(|(uid, page, _)| uid == job.uid && page == job.page)
            && self.last_doc.as_ref().is_some_and(|old| Arc::ptr_eq(old, &job.doc))
            && self.full_style.as_ref() == Some(&style)
            && self.full_time == job.time.to_bits();
        if !same_content {
            self.zoom_cache.clear();
        } else if let Some((_, _, old_scale)) = self.last
            && old_scale.to_bits() != job.view.scale.to_bits()
        {
            let previous = std::mem::take(&mut self.full);
            let cached = self.zoom_cache.iter().position(|entry| entry.scale == job.view.scale.to_bits());
            let incoming = cached.map(|index| self.zoom_cache.remove(index));
            let previous_view = self.full_view.take();
            if let Some(entry) = incoming {
                self.full = entry.retained;
                self.full_view = Some(entry.view);
            } else {
                self.full_view = None;
            }
            if self.full_pixels <= ZOOM_CACHE_MAX_PIXELS
                && let Some(view) = previous_view
            {
                self.zoom_cache.push(CachedZoom { scale: old_scale.to_bits(), pixels: self.full_pixels, view, retained: previous });
            }
            while self.zoom_cache.len() > ZOOM_CACHE_MAX_VIEWS
                || self.zoom_cache.iter().map(|entry| entry.pixels).sum::<u64>() > ZOOM_CACHE_MAX_PIXELS
            {
                self.zoom_cache.remove(0);
            }
        }
        let base_doc = same_content.then(|| job.doc.clone()).or_else(|| self.last_doc.clone());
        let done = guarded(&mut self.full, |r| run(job, &job.view, r, false, false, base_doc));
        if let Some(d) = &done {
            self.last = Some((job.uid, job.page, job.view.scale));
            self.last_doc = Some(job.doc.clone());
            self.full_style = Some(style);
            self.full_time = job.time.to_bits();
            self.full_pixels = u64::from(d.view.width) * u64::from(d.view.height);
            self.full_view = Some(d.view.clone());
            if d.work == Work::Full {
                self.full_ms = d.ms;
            }
        }
        done
    }

    fn refine_tiles(&mut self, job: &Job) -> Option<Done> {
        let start = Instant::now();
        let (view, desired) = tile_views(&job.view)?;
        let style = job.style();
        let same_content =
            self.tile_page == Some((job.uid, job.page)) && self.tile_style.as_ref() == Some(&style) && self.tile_time == job.time.to_bits();
        let make_tile = |tile_view: &View| -> Option<(RenderTile, bool)> {
            let cached = same_content.then(|| self.tiles.iter().find(|tile| same_view(&tile.view, tile_view))).flatten();
            let reuse = cached.is_some_and(|_| {
                self.tile_doc.as_ref().is_some_and(|old| {
                    Arc::ptr_eq(old, &job.doc)
                        || figcraft_render::retained::damage_between(old, &job.doc, job.page, tile_view).is_some_and(|damage| damage.is_empty())
                })
            });
            let image = if reuse { cached?.image.clone() } else { render_tile(job, tile_view)? };
            Some((RenderTile { view: tile_view.clone(), image }, reuse))
        };
        #[cfg(not(target_arch = "wasm32"))]
        let rendered: Vec<_> = desired.par_iter().map(make_tile).collect();
        #[cfg(target_arch = "wasm32")]
        let rendered: Vec<_> = desired.iter().map(make_tile).collect();
        let rendered: Vec<_> = rendered.into_iter().collect::<Option<Vec<_>>>()?;
        let reused = rendered.iter().filter(|(_, reused)| *reused).count() as u64 * u64::from(TILE_SIDE) * u64::from(TILE_SIDE);
        let tiles: Vec<_> = rendered.into_iter().map(|(tile, _)| tile).collect();
        let total = u64::from(view.width) * u64::from(view.height);
        let work = if reused == total {
            Work::Reused
        } else if reused == 0 {
            Work::Full
        } else {
            Work::Partial { pixels: total - reused }
        };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        self.tiles = tiles.clone();
        self.tile_doc = Some(job.doc.clone());
        self.tile_style = Some(style.clone());
        self.tile_page = Some((job.uid, job.page));
        self.tile_time = job.time.to_bits();
        self.last = Some((job.uid, job.page, job.view.scale));
        self.last_doc = Some(job.doc.clone());
        if work == Work::Full {
            self.full_ms = ms;
        }
        Some(Done {
            update: TextureUpdate::Tiles(tiles),
            #[cfg(test)]
            image: egui::ColorImage::from_rgba_premultiplied([1, 1], &[0, 0, 0, 0]),
            base_docs: Vec::new(),
            doc: job.doc.clone(),
            view,
            style,
            ms,
            work,
            uid: job.uid,
            page: job.page,
            coarse: false,
            at: job.at,
        })
    }
}

/// Run `f` on `retained`; after a panic, forget the image it may have left half drawn.
fn guarded(retained: &mut Retained, f: impl FnOnce(&mut Retained) -> Option<Done>) -> Option<Done> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(retained))) {
        Ok(d) => d,
        Err(_) => {
            retained.clear();
            None
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Worker {
    tx: std::sync::mpsc::Sender<(u64, Job)>,
    rx: std::sync::mpsc::Receiver<Done>,
    /// Jobs submitted so far.
    submitted: u64,
    /// Updated by the UI before enqueueing, so a long CPU effect can abandon obsolete work.
    latest: Arc<std::sync::atomic::AtomicU64>,
    /// The newest job the thread is done with: rendered (or skipped for a newer one) and the UI
    /// woken for it.
    finished: Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Worker {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, jobs) = std::sync::mpsc::channel::<(u64, Job)>();
        let (done_tx, rx) = std::sync::mpsc::channel::<Done>();
        let finished = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let done_with = finished.clone();
        let latest = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let requested = latest.clone();
        // The window that renders with it (each window has its own worker).
        let viewport = ctx.viewport_id();
        let spawned = std::thread::Builder::new().name("figcraft-render".into()).spawn(move || {
            let mut renders = Renders::default();
            let mut next = None;
            loop {
                let Some((mut seq, mut job)) = next.take().or_else(|| jobs.recv().ok()) else { break };
                // Skip to the newest queued job.
                while let Ok(newer) = jobs.try_recv() {
                    (seq, job) = newer;
                }
                if requested.load(std::sync::atomic::Ordering::Acquire) != seq {
                    continue;
                }
                // Keep a stable base while an expensive effect is being edited. If a newer edit
                // interrupts this render, the next small change (such as pasted text) can still
                // patch the last complete canvas instead of redrawing every effect from scratch.
                if renders.last_doc.as_ref().is_some_and(|old| !Arc::ptr_eq(old, &job.doc)) {
                    renders.zoom_cache.clear();
                }
                let saved = renders.needs_cancel_snapshot(&job).then(|| renders.clone());
                let preview = figcraft_render::with_interruption(requested.clone(), seq, || renders.preview(&job));
                if requested.load(std::sync::atomic::Ordering::Acquire) != seq {
                    renders = saved.unwrap_or_default();
                    continue;
                }
                if let Some(d) = preview {
                    if done_tx.send(d).is_err() {
                        break;
                    }
                    ctx.request_repaint_of(viewport);
                    // A newer request makes refining this one moot: it gets its own preview.
                    if let Ok(newer) = jobs.try_recv() {
                        next = Some(newer);
                        continue;
                    }
                }
                let refined = figcraft_render::with_interruption(requested.clone(), seq, || renders.refine(&job));
                if requested.load(std::sync::atomic::Ordering::Acquire) != seq {
                    renders = saved.unwrap_or_default();
                    continue;
                }
                if let Some(d) = refined
                    && done_tx.send(d).is_err()
                {
                    break;
                }
                ctx.request_repaint_of(viewport);
                done_with.store(seq, std::sync::atomic::Ordering::Release);
            }
        });
        if let Err(e) = spawned {
            log::error!("render thread: {e}");
        }
        Self { tx, rx, submitted: 0, latest, finished }
    }

    pub fn submit(&mut self, job: Job) {
        let next = self.submitted + 1;
        self.latest.store(next, std::sync::atomic::Ordering::Release);
        if self.tx.send((next, job)).is_ok() {
            self.submitted += 1;
        }
    }

    /// Whether a submitted job is still rendering or hasn't woken the UI yet (a finished image
    /// may still wait for [`Self::poll`]).
    pub fn in_flight(&self) -> bool {
        self.finished.load(std::sync::atomic::Ordering::Acquire) < self.submitted
    }

    pub fn poll(&mut self) -> Option<Done> {
        let mut last = None;
        while let Ok(d) = self.rx.try_recv() {
            last = Some(d);
        }
        last
    }

    /// Give a cheap vector drag a short chance to finish in the same UI frame that received its
    /// pointer move. More expensive renders stay asynchronous so they do not stall input.
    pub fn poll_for(&mut self, duration: std::time::Duration) -> Option<Done> {
        let first = self.rx.recv_timeout(duration).ok()?;
        Some(self.poll().unwrap_or(first))
    }
}

/// The browser's worker: renders inline, one image a frame: a preview first when one is due, the
/// refined image in the next frame unless a newer request replaced the job. Built everywhere so
/// tests run it.
pub struct Inline {
    ctx: egui::Context,
    pending: Option<Job>,
    previewed: bool,
    renders: Renders,
}

#[cfg(target_arch = "wasm32")]
pub type Worker = Inline;

impl Inline {
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx, pending: None, previewed: false, renders: Renders::default() }
    }
    pub fn submit(&mut self, job: Job) {
        self.pending = Some(job);
        self.previewed = false;
    }
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }
    pub fn poll(&mut self) -> Option<Done> {
        if !self.previewed {
            self.previewed = true;
            if let Some(d) = self.pending.as_ref().and_then(|job| self.renders.preview(job)) {
                self.ctx.request_repaint();
                return Some(d);
            }
        }
        let job = self.pending.take()?;
        self.renders.refine(&job)
    }

    pub fn poll_for(&mut self, _duration: std::time::Duration) -> Option<Done> {
        None
    }
}

/// Render synchronously from scratch (tests, screenshots).
pub fn render_now(job: &Job) -> Option<Done> {
    run(job, &job.view, &mut Retained::default(), false, false, None)
}

/// Fill an exposed viewport immediately while a zoom-out's vector render is still pending.
/// Keep native device resolution: the earlier half-resolution fill was visibly blurry during
/// trackpad zoom. The worker's retained image is still refined independently.
pub fn preview_now(job: &Job) -> Option<Done> {
    guarded(&mut Retained::default(), |retained| run(job, &job.view, retained, true, false, None))
}

#[cfg(test)]
#[test]
fn local_patch_matches_full_screen_conversion_in_both_profiles() {
    let mut pix = Pixmap::new(5, 4).unwrap();
    for (index, rgba) in pix.data_mut().chunks_exact_mut(4).enumerate() {
        rgba.copy_from_slice(&[(index * 11) as u8, (index * 7) as u8, (index * 3) as u8, 255]);
    }
    let region = DamageRect { x: 1, y: 1, width: 2, height: 2 };
    for profile in ColorProfile::ALL {
        let full = screen_image(&pix, profile);
        let (at, patch) = patch_image(&pix, profile, region).unwrap();
        assert_eq!(at, [1, 1]);
        for row in 0..patch.size[1] {
            let start = (at[1] + row) * full.size[0] + at[0];
            let part = row * patch.size[0];
            assert_eq!(&patch.pixels[part..part + patch.size[0]], &full.pixels[start..start + patch.size[0]]);
        }
    }
    assert!(patch_image(&pix, ColorProfile::Srgb, DamageRect { x: 4, y: 1, width: 2, height: 2 }).is_none());
}

#[cfg(test)]
#[test]
fn stable_tile_grid_covers_pans_without_changing_existing_cells() {
    let at = |x| View { origin: (x, -20.0), scale: 2.0, width: 2400, height: 1400 };
    let (a, first) = tile_views(&at(0.0)).unwrap();
    let (b, second) = tile_views(&at(100.0)).unwrap();
    assert!(a.page_rect().x0 <= 0.0 && a.page_rect().x1 >= 1200.0);
    assert!(b.page_rect().x0 <= 100.0 && b.page_rect().x1 >= 1300.0);
    assert!(first.iter().any(|one| second.iter().any(|two| same_view(one, two))));
    assert!(first.iter().all(|tile| tile.width == TILE_SIDE && tile.height == TILE_SIDE));
}

#[cfg(test)]
#[test]
fn high_zoom_uses_regions_before_a_whole_view_effect_buffer_grows_large() {
    let doc = figcraft_doc::Document::new("high zoom");
    let view = View { origin: (0.0, 0.0), scale: 8.0, width: 1300, height: 1200 };
    assert!(uses_tiles(&view, &doc, false));
    assert!(!uses_tiles(&View { scale: 4.0, ..view.clone() }, &doc, false));
    assert!(!uses_tiles(&View { width: 900, height: 900, ..view }, &doc, false));

    let mut session = figcraft_engine::Session::new();
    let shape = session.execute("shape.rectangle", &serde_json::json!({"x": 0, "y": 0, "width": 100, "height": 100})).unwrap();
    let id = figcraft_engine::params::id_of(&shape["id"]).unwrap();
    session.execute("effect.add", &serde_json::json!({"ids": [id], "type": "dropShadow"})).unwrap();
    let mut with_shadow = (*session.ds().unwrap().doc).clone();
    let node = with_shadow.node_mut(id).unwrap();
    if let figcraft_doc::EffectKind::DropShadow(shadow) = &mut node.effects[0].kind {
        shadow.radius = 80.0;
    }
    assert!(!uses_tiles(&view, &with_shadow, false));
}

#[cfg(test)]
#[test]
fn zoom_changes_do_not_clone_the_retained_view_for_cancellation() {
    let mut session = figcraft_engine::Session::new();
    let ds = session.ds().unwrap();
    let mut job = Job {
        doc: ds.doc.clone(),
        page: ds.page,
        view: View { origin: (0.0, 0.0), scale: 1.0, width: 800, height: 600 },
        time: 0.0,
        outline: false,
        outline_extras: Default::default(),
        outlined: Vec::new(),
        skip: Vec::new(),
        solo: None,
        proof: false,
        uid: ds.uid,
        zoom_preview: false,
        at: Instant::now(),
        texture_base: None,
        texture_view: None,
        texture_style: None,
    };
    let renders = Renders { last: Some((job.uid, job.page, job.view.scale)), last_doc: Some(job.doc.clone()), full_ms: 250.0, ..Default::default() };
    job.view.scale = 8.0;
    assert!(!renders.needs_cancel_snapshot(&job));

    job.view.scale = 1.0;
    session.execute("shape.rectangle", &serde_json::json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    job.doc = session.ds().unwrap().doc.clone();
    assert!(renders.needs_cancel_snapshot(&job), "an edit at the same zoom can reuse its completed base");
}

#[cfg(test)]
#[test]
fn adjacent_tiles_match_one_vector_render_through_a_blur() {
    use serde_json::json;
    let mut session = figcraft_engine::Session::new();
    let back = session.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 2048, "height": 1024})).unwrap();
    let front = session.execute("shape.rectangle", &json!({"x": 980, "y": 160, "width": 120, "height": 600})).unwrap();
    let _ = back;
    let id = front["id"].as_u64().unwrap();
    session.execute("effect.add", &json!({"ids": [id], "type": "BACKGROUND_BLUR"})).unwrap();
    let ds = session.ds().unwrap();
    let view = View { origin: (0.0, 0.0), scale: 1.0, width: 2048, height: 1024 };
    let job = Job {
        doc: ds.doc.clone(),
        page: ds.page,
        view: view.clone(),
        outline: false,
        outline_extras: Default::default(),
        outlined: Vec::new(),
        skip: Vec::new(),
        solo: None,
        proof: false,
        uid: ds.uid,
        zoom_preview: false,
        time: 0.0,
        at: Instant::now(),
        texture_base: None,
        texture_view: None,
        texture_style: None,
    };
    let reference = figcraft_render::render_view(&ds.doc, ds.page, &view, &figcraft_render::Options { lod: true, ..Default::default() }).unwrap();
    let full = screen_image(&reference, ds.doc.color_profile);
    for x in [0.0, 512.0, 1024.0, 1536.0] {
        for y0 in [0.0, 512.0] {
            let tile = View { origin: (x, y0), scale: 1.0, width: TILE_SIDE, height: TILE_SIDE };
            let image = render_tile(&job, &tile).unwrap();
            for y in 0..TILE_SIDE as usize {
                let start = (y0 as usize + y) * full.size[0] + x as usize;
                let end = start + TILE_SIDE as usize;
                let local = y * TILE_SIDE as usize;
                assert_eq!(&image.pixels[local..local + TILE_SIDE as usize], &full.pixels[start..end], "tile x={x}, y={y0}, row={y}");
            }
        }
    }
}

#[cfg(test)]
#[test]
fn wide_pan_reuses_gpu_tile_images() {
    let session = figcraft_engine::Session::new();
    let ds = session.ds().unwrap();
    let mut job = Job {
        doc: ds.doc.clone(),
        page: ds.page,
        view: View { origin: (0.0, 0.0), scale: 1.0, width: 2048, height: 2048 },
        outline: false,
        outline_extras: Default::default(),
        outlined: Vec::new(),
        skip: Vec::new(),
        solo: None,
        proof: false,
        uid: ds.uid,
        zoom_preview: true,
        time: 0.0,
        at: Instant::now(),
        texture_base: None,
        texture_view: None,
        texture_style: None,
    };
    assert!(uses_tiles(&job.view, &job.doc, job.proof));
    let mut renders = Renders::default();
    assert!(renders.preview(&job).is_none(), "wide zoom never substitutes a blurry half-resolution image");
    let first = renders.refine(&job).unwrap();
    let TextureUpdate::Tiles(first) = first.update else { panic!("expected tiles") };
    assert_eq!(first.len(), 16);
    job.view.origin.0 = 100.0;
    let second = renders.refine(&job).unwrap();
    assert!(matches!(second.work, Work::Partial { .. }));
    let TextureUpdate::Tiles(second_tiles) = second.update else { panic!("expected tiles") };
    assert_eq!(second_tiles.len(), 20);
    assert_eq!(second_tiles.iter().filter(|tile| first.iter().any(|old| Arc::ptr_eq(&old.image, &tile.image))).count(), 16);
}

#[cfg(test)]
#[test]
fn wide_edit_replaces_only_the_affected_tile() {
    use serde_json::json;
    let mut session = figcraft_engine::Session::new();
    let made = session.execute("shape.rectangle", &json!({"x": 80, "y": 80, "width": 100, "height": 100})).unwrap();
    let id = figcraft_engine::params::id_of(&made["id"]).unwrap();
    let ds = session.ds().unwrap();
    let mut job = Job {
        doc: ds.doc.clone(),
        page: ds.page,
        view: View { origin: (0.0, 0.0), scale: 1.0, width: 2048, height: 2048 },
        outline: false,
        outline_extras: Default::default(),
        outlined: Vec::new(),
        skip: Vec::new(),
        solo: None,
        proof: false,
        uid: ds.uid,
        zoom_preview: false,
        time: 0.0,
        at: Instant::now(),
        texture_base: None,
        texture_view: None,
        texture_style: None,
    };
    let mut renders = Renders::default();
    let first = renders.refine(&job).unwrap();
    let TextureUpdate::Tiles(before) = first.update else { panic!("expected tiles") };
    session.execute("node.set", &json!({"id": id, "props": {"x": 160}})).unwrap();
    job.doc = session.ds().unwrap().doc.clone();
    let second = renders.refine(&job).unwrap();
    let TextureUpdate::Tiles(after) = second.update else { panic!("expected tiles") };
    assert_eq!(after.iter().filter(|tile| before.iter().any(|old| Arc::ptr_eq(&old.image, &tile.image))).count(), 15);
    assert_eq!(second.work, Work::Partial { pixels: u64::from(TILE_SIDE) * u64::from(TILE_SIDE) });
}
