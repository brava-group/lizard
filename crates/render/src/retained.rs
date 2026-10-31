//! Retained canvas rendering: keep the last rendered view and, for the next one, re-render only
//! what changed: the strips a pan uncovers and the areas of layers that were edited.
//!
//! Views are snapped to whole device pixels so that, at one zoom, the pixels of every render line
//! up and a pan is an integer shift of the previous image. Anything the partial path can't vouch
//! for (another zoom, page or set of options, new fonts, a page-level edit, a large change) is a
//! full render, the same as [`render_view`].

use std::sync::Arc;

use lizard_doc::{Document, EffectKind, NodeId, NodeKind};
use lizard_geom::Rect;
use tiny_skia::Pixmap;

use crate::{Options, View, draw, render_view};

/// What a render had to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// Nothing changed: the previous image as is.
    Reused,
    /// The previous image, shifted and/or patched where things changed.
    Partial {
        /// Device pixels rendered afresh.
        pixels: u64,
    },
    /// The whole view.
    Full,
}

/// A region of the retained image whose device pixels changed. Half-open coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Re-rendered areas beyond this many get merged into their bounding box.
const MAX_RECTS: usize = 8;
/// A change covering more than this share of the view renders it whole.
const MAX_PARTIAL_SHARE: f64 = 0.6;
/// More changed layers than this render the view whole.
const MAX_CHANGED: usize = 4096;
/// Effect margins beyond this many device pixels render the view whole.
const MAX_MARGIN_PX: f64 = 1024.0;

/// What a retained image was rendered from.
#[derive(Clone)]
struct Frame {
    doc: Arc<Document>,
    page: NodeId,
    outline: bool,
    outline_extras: crate::Outline,
    outlined: Vec<NodeId>,
    lod: bool,
    skip_effects: bool,
    skip: Vec<NodeId>,
    solo: Option<NodeId>,
    background: Option<lizard_color::Color>,
    time: u64,
    scale: u64,
    /// The view's top-left in device pixels (page origin × scale).
    px: (i64, i64),
    fonts: u64,
    /// Areas (page coordinates) of layers whose look depends on where a render cuts them, and the
    /// largest background blur (what backdrops read around).
    sensitive: Arc<Sensitive>,
    pix: Pixmap,
}

#[derive(Default)]
struct Sensitive {
    areas: Vec<Rect>,
    backdrop_blur: f64,
    /// Pattern fill sources: an edit inside one shows wherever the pattern does.
    pattern_sources: Vec<NodeId>,
}

/// Grain effects (noise, texture) work on their layer's pixels, which end where a render cuts the
/// layer (texture scatters pixels from across the cut): grainy layers are redrawn whole.
fn sensitive(doc: &Document) -> Sensitive {
    let noisy = doc.nodes().filter(|n| n.effects.iter().any(|e| e.visible && matches!(e.kind, EffectKind::Noise(_) | EffectKind::Texture(_))));
    let pattern_sources = doc
        .nodes()
        .flat_map(|n| n.fills.iter().chain(&n.strokes))
        .filter_map(|p| match &p.kind {
            lizard_color::PaintKind::Pattern(pt) => Some(NodeId(pt.source)),
            _ => None,
        })
        .collect();
    Sensitive { areas: noisy.filter_map(|n| draw::paint_bounds(doc, n.id)).collect(), backdrop_blur: crate::max_backdrop_blur(doc), pattern_sources }
}

/// Whether `id` is a pattern source or inside one in `doc`.
fn in_pattern_source(doc: &Document, s: &Sensitive, id: NodeId) -> bool {
    s.pattern_sources.iter().any(|src| *src == id || doc.is_ancestor(*src, id))
}

/// A view snapped outwards to whole device pixels: the origin on its zoom's pixel grid and one
/// more pixel each way. Returns the view and its origin in device pixels.
pub fn snap(view: &View) -> Option<(View, (i64, i64))> {
    let s = view.scale;
    let (fx, fy) = ((view.origin.0 * s).floor(), (view.origin.1 * s).floor());
    if !(s.is_finite() && s > 0.0 && fx.is_finite() && fy.is_finite() && fx.abs() < 1e15 && fy.abs() < 1e15) {
        return None;
    }
    let v = View {
        origin: (fx / s, fy / s),
        scale: s,
        width: view.width.saturating_add(1).clamp(1, 16_384),
        height: view.height.saturating_add(1).clamp(1, 16_384),
    };
    Some((v, (fx as i64, fy as i64)))
}

/// Device-pixel rectangle `[x0, x1) × [y0, y1)` in the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Px {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl Px {
    fn area(&self) -> u64 {
        (self.x1 - self.x0).max(0) as u64 * (self.y1 - self.y0).max(0) as u64
    }
    fn union(&self, o: &Px) -> Px {
        Px { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    fn clamp(&self, w: i64, h: i64) -> Option<Px> {
        let r = Px { x0: self.x0.max(0), y0: self.y0.max(0), x1: self.x1.min(w), y1: self.y1.min(h) };
        (r.x1 > r.x0 && r.y1 > r.y0).then_some(r)
    }
}

/// Keeps the last rendered view; see the module docs.
#[derive(Clone, Default)]
pub struct Retained {
    last: Option<Frame>,
    /// The frame before a simple topmost layer was added. Removing that layer can restore these
    /// pixels without rendering all of the effects underneath it again.
    overlay_base: Option<Frame>,
}

impl Retained {
    /// Forget the kept image (the next render is full).
    pub fn clear(&mut self) {
        self.last = None;
        self.overlay_base = None;
    }

    /// Render `view` of `page`, reusing the previous render where nothing changed. Returns the
    /// view actually drawn (snapped to device pixels, see [`snap`]), the image and what it took.
    pub fn render(&mut self, doc: &Arc<Document>, page: NodeId, view: &View, opts: &Options) -> Option<(View, &Pixmap, Work)> {
        self.render_with_damage(doc, page, view, opts).map(|(view, pix, work, _)| (view, pix, work))
    }

    /// As [`Self::render`], with damage for a same-view texture upload. `None` means the entire
    /// texture changed; an empty list means its pixels can be kept. A pan shifts kept CPU pixels,
    /// so its texture still needs a full upload even if only the exposed strips were rendered.
    pub fn render_with_damage(
        &mut self,
        doc: &Arc<Document>,
        page: NodeId,
        view: &View,
        opts: &Options,
    ) -> Option<(View, &Pixmap, Work, Option<Vec<DamageRect>>)> {
        // A view that can't be put on a pixel grid (non-finite or absurd) draws nothing.
        let Some((snapped, px)) = snap(view) else {
            self.last = None;
            self.overlay_base = None;
            return None;
        };
        let fonts = lizard_text::layout_generation();
        let mut work = Work::Full;
        let mut sens = None;
        if let Some(last) = self.last.take()
            && last.page == page
            && last.outline == opts.outline_mode
            && last.outline_extras == opts.outline
            // Hidden layers drawn as outlines aren't in the bounds patches are cut from.
            && !opts.outline.hidden
            && last.outlined == opts.outlined
            && last.lod == opts.lod
            && last.skip_effects == opts.skip_effects
            && last.skip == opts.skip
            && last.solo == opts.solo
            && last.background == opts.background
            && last.time == opts.time.to_bits()
            && last.scale == snapped.scale.to_bits()
            && last.pix.width() == snapped.width
            && last.pix.height() == snapped.height
            && last.fonts == fonts
            // Images are decoded into the file's colour space: a new profile redraws everything.
            && last.doc.color_profile == doc.color_profile
        {
            let same_doc = Arc::ptr_eq(&last.doc, doc);
            let s = if same_doc { last.sensitive.clone() } else { Arc::new(sensitive(doc)) };
            if last.px == px {
                if self.overlay_base.as_ref().is_some_and(|base| base.px == px && base.doc.changed_ids(doc, 1).is_some_and(|ids| ids.is_empty()))
                    && let Some(base) = self.overlay_base.take()
                {
                    let frame = self.keep(doc, page, (&snapped, px, fonts), opts, base.pix, base.sensitive);
                    return Some((snapped, &frame.pix, Work::Reused, None));
                }
                if let Some(id) = topmost_simple_addition(&last.doc, doc, page, opts) {
                    let mut pix = last.pix.clone();
                    crate::render_nodes_into_preview(doc, &[id], &snapped, &mut pix, opts.lod);
                    let pixels = draw::paint_bounds(doc, id)
                        .and_then(|bounds| to_px(bounds, snapped.scale, px))
                        .and_then(|bounds| bounds.clamp(i64::from(snapped.width), i64::from(snapped.height)))
                        .map_or(0, |bounds| bounds.area());
                    self.overlay_base = Some(last);
                    let frame = self.keep(doc, page, (&snapped, px, fonts), opts, pix, s);
                    return Some((snapped, &frame.pix, Work::Partial { pixels }, None));
                }
            }
            if !same_doc {
                self.overlay_base = None;
            }
            if let Some((pix, pixels, damage)) = patch(last, doc, page, &snapped, px, opts, &s) {
                work = if pixels == 0 { Work::Reused } else { Work::Partial { pixels } };
                let frame = self.keep(doc, page, (&snapped, px, fonts), opts, pix, s);
                return Some((snapped, &frame.pix, work, damage));
            }
            sens = Some(s);
        }
        self.overlay_base = None;
        let pix = render_view(doc, page, &snapped, opts)?;
        let s = sens.unwrap_or_else(|| Arc::new(sensitive(doc)));
        let frame = self.keep(doc, page, (&snapped, px, fonts), opts, pix, s);
        Some((snapped, &frame.pix, work, None))
    }

    /// Keep `pix`, the render of `at`: the snapped view, its top-left device pixel and the font
    /// generation the render started with (fonts arriving meanwhile leave it stale).
    fn keep(
        &mut self,
        doc: &Arc<Document>,
        page: NodeId,
        at: (&View, (i64, i64), u64),
        opts: &Options,
        pix: Pixmap,
        sensitive: Arc<Sensitive>,
    ) -> &Frame {
        let (view, px, fonts) = at;
        self.last.insert(Frame {
            doc: doc.clone(),
            page,
            outline: opts.outline_mode,
            outline_extras: opts.outline,
            outlined: opts.outlined.clone(),
            lod: opts.lod,
            skip_effects: opts.skip_effects,
            skip: opts.skip.clone(),
            solo: opts.solo,
            background: opts.background,
            time: opts.time.to_bits(),
            scale: view.scale.to_bits(),
            px,
            fonts,
            sensitive,
            pix,
        })
    }
}

/// Adding a simple layer above everything already drawn cannot change the pixels underneath it.
/// This is especially useful for pasted text over a large blurred frame: the usual damage area
/// covers the viewport and would otherwise redraw the frame before showing the text.
fn topmost_simple_addition(old: &Document, new: &Document, page: NodeId, opts: &Options) -> Option<NodeId> {
    if opts.outline_mode || !opts.outlined.is_empty() || !opts.skip.is_empty() || opts.solo.is_some() || opts.time != 0.0 {
        return None;
    }
    if new.node_count() != old.node_count().checked_add(1)? {
        return None;
    }
    let before = &old.node(page)?.children;
    let after = &new.node(page)?.children;
    let (&id, prefix) = after.split_last()?;
    if prefix != before || old.node(id).is_some() {
        return None;
    }
    // A mask below this layer could clip it. Pattern sources and document-wide edits also need
    // the normal dependency-aware path.
    if new.nodes().any(|n| n.is_mask && new.page_of(n.id) == Some(page)) {
        return None;
    }
    let changes = new.changed_ids(old, 3)?;
    if changes.len() != 2 || !changes.contains(&page) || !changes.contains(&id) {
        return None;
    }
    let n = new.node(id)?;
    if n.parent != Some(page) || !n.children.is_empty() || n.is_mask || !n.effects.is_empty() {
        return None;
    }
    match n.kind {
        NodeKind::Rectangle
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Line
        | NodeKind::Vector { .. }
        | NodeKind::Text(_) => Some(id),
        _ => None,
    }
}

/// Page rectangle → device pixels of a frame whose top-left is `px`, rounded outwards plus a pixel
/// of anti-aliasing.
fn to_px(r: Rect, scale: f64, px: (i64, i64)) -> Option<Px> {
    let (x0, y0, x1, y1) = ((r.x0 * scale).floor() - 1.0, (r.y0 * scale).floor() - 1.0, (r.x1 * scale).ceil() + 1.0, (r.y1 * scale).ceil() + 1.0);
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite())
        || x0.abs() > 1e15
        || y0.abs() > 1e15
        || x1.abs() > 1e15
        || y1.abs() > 1e15
    {
        return None;
    }
    Some(Px { x0: x0 as i64 - px.0, y0: y0 as i64 - px.1, x1: x1 as i64 - px.0, y1: y1 as i64 - px.1 })
}

/// Page areas a change to `id` in `doc` affects on `page`; `None` when only a full render will do.
fn changed_areas(doc: &Document, page: NodeId, id: NodeId, out: &mut Vec<Rect>) -> Option<()> {
    let Some(n) = doc.node(id) else { return Some(()) };
    if n.is_page() {
        // Page background, page order or top-level layers.
        return if id == page { None } else { Some(()) };
    }
    if doc.page_of(id) != Some(page) {
        return Some(());
    }
    out.push(draw::paint_bounds(doc, id)?);
    // A mask clips the layers above it.
    if n.is_mask {
        let parent = n.parent?;
        if parent == page {
            return None;
        }
        out.push(draw::paint_bounds(doc, parent)?);
    }
    // Ancestors that paint from their children's pixels (booleans, effects, masks): all of them.
    for a in doc.ancestors(id) {
        let Some(an) = doc.node(a) else { break };
        if an.is_page() {
            break;
        }
        if matches!(an.kind, NodeKind::BooleanOp { .. }) || an.is_mask || an.effects.iter().any(|e| e.visible) {
            out.push(draw::paint_bounds(doc, a)?);
        }
    }
    Some(())
}

/// Pixel regions needed to bring an already displayed image of `old` up to date with `new`.
/// This also works when render jobs between the two documents were skipped during a fast drag.
/// The caller must ensure both images have the same view and rendering options.
pub fn damage_between(old: &Document, new: &Document, page: NodeId, view: &View) -> Option<Vec<DamageRect>> {
    if old.color_profile != new.color_profile {
        return None;
    }
    let (fx, fy) = (view.origin.0 * view.scale, view.origin.1 * view.scale);
    if !(fx.is_finite() && fy.is_finite() && fx.abs() < 1e15 && fy.abs() < 1e15 && view.scale.is_finite() && view.scale > 0.0) {
        return None;
    }
    let px = (fx.round() as i64, fy.round() as i64);
    let old_sensitive = sensitive(old);
    let new_sensitive = sensitive(new);
    let reach = crate::backdrop_reach(old_sensitive.backdrop_blur.max(new_sensitive.backdrop_blur), view.scale).ceil();
    if !reach.is_finite() || reach > MAX_MARGIN_PX {
        return None;
    }
    let reach = reach as i64;
    let mut areas = Vec::new();
    let topmost_addition = topmost_simple_addition(old, new, page, &Options::default());
    for id in new.changed_ids(old, MAX_CHANGED)? {
        if id == page && topmost_addition.is_some() {
            continue;
        }
        if in_pattern_source(old, &old_sensitive, id) || in_pattern_source(new, &new_sensitive, id) {
            return None;
        }
        changed_areas(old, page, id, &mut areas)?;
        changed_areas(new, page, id, &mut areas)?;
    }
    let (w, h) = (i64::from(view.width), i64::from(view.height));
    let mut damage: Vec<Px> = areas
        .into_iter()
        .filter_map(|r| to_px(r, view.scale, px))
        .filter_map(|p| Px { x0: p.x0 - reach, y0: p.y0 - reach, x1: p.x1 + reach, y1: p.y1 + reach }.clamp(w, h))
        .collect();
    let sensitive: Vec<Px> = old_sensitive.areas.iter().chain(new_sensitive.areas.iter()).filter_map(|r| to_px(*r, view.scale, px)).collect();
    for _ in 0..4 {
        let mut grew = false;
        for d in &mut damage {
            for a in &sensitive {
                let hits = a.x0 < d.x1 && d.x0 < a.x1 && a.y0 < d.y1 && d.y0 < a.y1;
                if hits && a.union(d) != *d {
                    *d = a.union(d);
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut damage = merge(damage.into_iter().filter_map(|d| d.clamp(w, h)).collect());
    if damage.len() > MAX_RECTS {
        let all = damage.iter().skip(1).fold(*damage.first()?, |a, b| a.union(b));
        damage = vec![all];
    }
    if damage.iter().map(Px::area).sum::<u64>() as f64 > MAX_PARTIAL_SHARE * (w * h) as f64 {
        return None;
    }
    damage
        .into_iter()
        .map(|d| {
            Some(DamageRect {
                x: u32::try_from(d.x0).ok()?,
                y: u32::try_from(d.y0).ok()?,
                width: u32::try_from(d.x1 - d.x0).ok()?,
                height: u32::try_from(d.y1 - d.y0).ok()?,
            })
        })
        .collect()
}

/// The previous frame brought up to date for `doc` at `px`, rendering only what changed; `None`
/// when a full render is the better (or only correct) choice. Also returns the pixels rendered.
fn patch(
    last: Frame,
    doc: &Arc<Document>,
    page: NodeId,
    view: &View,
    px: (i64, i64),
    opts: &Options,
    s: &Sensitive,
) -> Option<(Pixmap, u64, Option<Vec<DamageRect>>)> {
    let (w, h) = (i64::from(view.width), i64::from(view.height));
    let (dx, dy) = (px.0.checked_sub(last.px.0)?, px.1.checked_sub(last.px.1)?);
    if dx.abs() >= w || dy.abs() >= h {
        return None;
    }
    // Background blurs read the backdrop around them, so a change shows that far off: under a blur
    // beside a changed layer, and at the old view's edge (where the backdrop used to end).
    let reach = crate::backdrop_reach(s.backdrop_blur.max(last.sensitive.backdrop_blur), view.scale).ceil();
    if !reach.is_finite() || reach > MAX_MARGIN_PX {
        return None;
    }
    let reach = reach as i64;
    let mut damage: Vec<Px> = Vec::new();
    // Strips a pan uncovers.
    if dx > 0 {
        damage.push(Px { x0: w - dx - reach, y0: 0, x1: w, y1: h });
    } else if dx < 0 {
        damage.push(Px { x0: 0, y0: 0, x1: reach - dx, y1: h });
    }
    if dy > 0 {
        damage.push(Px { x0: 0, y0: h - dy - reach, x1: w, y1: h });
    } else if dy < 0 {
        damage.push(Px { x0: 0, y0: 0, x1: w, y1: reach - dy });
    }
    // Layers that changed, where they were and where they are now.
    if !Arc::ptr_eq(&last.doc, doc) {
        let mut areas = Vec::new();
        for id in doc.changed_ids(&last.doc, MAX_CHANGED)? {
            // shortcut: an edited pattern source redraws the whole view; patch the patterned layers' areas when that shows up in profiles.
            if in_pattern_source(doc, s, id) || in_pattern_source(&last.doc, &last.sensitive, id) {
                return None;
            }
            changed_areas(&last.doc, page, id, &mut areas)?;
            changed_areas(doc, page, id, &mut areas)?;
        }
        for r in areas {
            if let Some(p) = to_px(r, view.scale, px) {
                damage.push(Px { x0: p.x0 - reach, y0: p.y0 - reach, x1: p.x1 + reach, y1: p.y1 + reach });
            }
        }
    }
    let mut damage: Vec<Px> = damage.iter().filter_map(|d| d.clamp(w, h)).collect();
    if damage.is_empty() && dx == 0 && dy == 0 {
        return Some((last.pix, 0, Some(Vec::new())));
    }
    // Layers whose look depends on where they are cut are redrawn whole.
    let sensitive: Vec<Px> = s.areas.iter().chain(last.sensitive.areas.iter()).filter_map(|r| to_px(*r, view.scale, px)).collect();
    for _ in 0..4 {
        let mut grew = false;
        for d in damage.iter_mut() {
            for a in &sensitive {
                let hits = a.x0 < d.x1 && d.x0 < a.x1 && a.y0 < d.y1 && d.y0 < a.y1;
                if hits && a.union(d) != *d {
                    *d = a.union(d);
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut damage = merge(damage.iter().filter_map(|d| d.clamp(w, h)).collect());
    if damage.len() > MAX_RECTS {
        let all = damage.iter().skip(1).fold(*damage.first()?, |a, b| a.union(b));
        damage = vec![all];
    }
    let pixels: u64 = damage.iter().map(Px::area).sum();
    if pixels as f64 > MAX_PARTIAL_SHARE * (w * h) as f64 {
        return None;
    }
    // Render with the backdrop reach as a margin (so blurs see their whole backdrop), keep the inside.
    let margin = reach + 1;
    let mut pix = last.pix;
    shift(&mut pix, dx, dy)?;
    for d in &damage {
        render_into(doc, page, view, px, d, margin, opts, &mut pix)?;
    }
    let regions = if dx == 0 && dy == 0 {
        Some(
            damage
                .into_iter()
                .map(|d| {
                    Some(DamageRect {
                        x: u32::try_from(d.x0).ok()?,
                        y: u32::try_from(d.y0).ok()?,
                        width: u32::try_from(d.x1 - d.x0).ok()?,
                        height: u32::try_from(d.y1 - d.y0).ok()?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        )
    } else {
        None
    };
    Some((pix, pixels, regions))
}

/// Move the image's content by `(-dx, -dy)` pixels in place (what was at `(x, y)` goes to
/// `(x - dx, y - dy)`); the uncovered strips keep stale pixels for the caller to re-render.
fn shift(pix: &mut Pixmap, dx: i64, dy: i64) -> Option<()> {
    if dx == 0 && dy == 0 {
        return Some(());
    }
    let (w, h) = (i64::from(pix.width()), i64::from(pix.height()));
    let row = usize::try_from(w * 4).ok()?;
    let data = pix.data_mut();
    // Columns that survive the horizontal move, as byte ranges within a row.
    let (src_x, dst_x, n) = if dx >= 0 { (dx, 0, w - dx) } else { (0, -dx, w + dx) };
    let (src_x, dst_x, n) = (usize::try_from(src_x * 4).ok()?, usize::try_from(dst_x * 4).ok()?, usize::try_from(n * 4).ok()?);
    let ys: Box<dyn Iterator<Item = i64>> = if dy >= 0 { Box::new(0..h - dy) } else { Box::new((-dy..h).rev()) };
    for y in ys {
        let (src, dst) = (usize::try_from(y + dy).ok()? * row, usize::try_from(y).ok()? * row);
        if src + src_x + n > data.len() || dst + dst_x + n > data.len() {
            return None;
        }
        data.copy_within(src + src_x..src + src_x + n, dst + dst_x);
    }
    Some(())
}

/// Merge rectangles that overlap so much that their bounding box is no bigger than the two
/// (where a layer was and where it is now, a frame and its child…): no pixel renders twice.
fn merge(mut rects: Vec<Px>) -> Vec<Px> {
    let mut i = 0;
    while i < rects.len() {
        let mut j = i + 1;
        let mut merged = false;
        while j < rects.len() {
            let (Some(a), Some(b)) = (rects.get(i).copied(), rects.get(j).copied()) else { break };
            let u = a.union(&b);
            if u.area() <= a.area() + b.area() {
                if let Some(slot) = rects.get_mut(i) {
                    *slot = u;
                }
                rects.swap_remove(j);
                merged = true;
            } else {
                j += 1;
            }
        }
        if !merged {
            i += 1;
        }
    }
    rects
}

/// Render the frame pixels `d` (with `margin` more around, discarded) into `pix`.
#[allow(clippy::too_many_arguments)]
fn render_into(doc: &Document, page: NodeId, view: &View, px: (i64, i64), d: &Px, margin: i64, opts: &Options, pix: &mut Pixmap) -> Option<()> {
    let s = view.scale;
    let (ox, oy) = (d.x0 - margin, d.y0 - margin);
    let (rw, rh) = (u32::try_from(d.x1 - d.x0 + 2 * margin).ok()?, u32::try_from(d.y1 - d.y0 + 2 * margin).ok()?);
    // The same pixel grid as the frame: origin (px + offset) / scale.
    let sub = View { origin: ((px.0 + ox) as f64 / s, (px.1 + oy) as f64 / s), scale: s, width: rw, height: rh };
    let part = render_view(doc, page, &sub, opts)?;
    // Both pixmaps are premultiplied RGBA on the same pixel grid. Copying the inner rows is
    // exactly a Source blit, without cloning a temporary pixmap or running the shader pipeline.
    let (width, height) = (usize::try_from(d.x1 - d.x0).ok()?, usize::try_from(d.y1 - d.y0).ok()?);
    let (dst_x, dst_y, margin) = (usize::try_from(d.x0).ok()?, usize::try_from(d.y0).ok()?, usize::try_from(margin).ok()?);
    let src_stride = usize::try_from(part.width()).ok()?.checked_mul(4)?;
    let dst_stride = usize::try_from(pix.width()).ok()?.checked_mul(4)?;
    let row_bytes = width.checked_mul(4)?;
    let src_x = margin.checked_mul(4)?;
    let dst_x = dst_x.checked_mul(4)?;
    for y in 0..height {
        let src_start = margin.checked_add(y)?.checked_mul(src_stride)?.checked_add(src_x)?;
        let dst_start = dst_y.checked_add(y)?.checked_mul(dst_stride)?.checked_add(dst_x)?;
        let src = part.data().get(src_start..src_start.checked_add(row_bytes)?)?;
        let dst = pix.data_mut().get_mut(dst_start..dst_start.checked_add(row_bytes)?)?;
        dst.copy_from_slice(src);
    }
    Some(())
}
