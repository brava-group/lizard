//! A bounded, asynchronously prepared geometry cache for pan and zoom experiments.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

use lizard_doc::{Document, NodeId};
use lizard_geom::Rect;
use lizard_render::{Options, Pixmap, View, paint_bounds, render_view, render_view_single, retained::damage_between};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::compositor::Compositor;

const TILE_SIZE: u32 = 768;
const BASE_SCALE: f64 = 0.3;
const TIER_RATIO: f64 = 2.0;
const MIN_SUPERSAMPLE: f64 = 1.5;
const MAX_TIER: u8 = 12;
pub const DEFAULT_CAPACITY: u32 = 160;
const MAX_PENDING: usize = 16;
const MAX_GUARDED_PREFETCH_PENDING: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Key {
    x: i32,
    y: i32,
    tier: u8,
}

impl Key {
    fn scale(self) -> f64 {
        BASE_SCALE * TIER_RATIO.powi(i32::from(self.tier))
    }

    fn span(self) -> f64 {
        f64::from(TILE_SIZE) / self.scale()
    }

    fn origin(self) -> (f32, f32) {
        ((f64::from(self.x) * self.span()) as f32, (f64::from(self.y) * self.span()) as f32)
    }

    fn view(self, bleed: u32) -> View {
        let scale = self.scale();
        View {
            origin: (
                (f64::from(self.x) * f64::from(TILE_SIZE) - f64::from(bleed)) / scale,
                (f64::from(self.y) * f64::from(TILE_SIZE) - f64::from(bleed)) / scale,
            ),
            scale,
            width: TILE_SIZE + bleed * 2,
            height: TILE_SIZE + bleed * 2,
        }
    }
}

fn intersects_damage(key: Key, bleed: u32, damage: &[Rect]) -> bool {
    let view = key.view(bleed);
    let x1 = view.origin.0 + f64::from(view.width) / view.scale;
    let y1 = view.origin.1 + f64::from(view.height) / view.scale;
    damage.iter().any(|rect| view.origin.0 < rect.x1 && x1 > rect.x0 && view.origin.1 < rect.y1 && y1 > rect.y0)
}

fn content_bounds(doc: &Document, page: NodeId) -> Option<Rect> {
    let mut painted: Option<Rect> = None;
    let page = doc.node(page)?;
    for id in &page.children {
        if !doc.node(*id).is_some_and(|node| node.visible) {
            continue;
        }
        let bounds = paint_bounds(doc, *id)?;
        if ![bounds.x0, bounds.y0, bounds.x1, bounds.y1].iter().all(|value| value.is_finite()) {
            return None;
        }
        painted = Some(painted.map_or(bounds, |current| current.union(bounds)));
    }
    Some(painted.unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0)))
}

fn key_intersects_content(key: Key, bleed: u32, bounds: Option<Rect>) -> bool {
    bounds.is_none_or(|bounds| bounds.width() > 0.0 && bounds.height() > 0.0 && intersects_damage(key, bleed, &[bounds]))
}

fn tile_patch_view(key: Key, bleed: u32, damage: &[Rect]) -> Option<(u32, u32, View)> {
    let tile = key.view(bleed);
    let side = tile.width as i64;
    let mut region = (side, side, 0_i64, 0_i64);
    for rect in damage {
        if !intersects_damage(key, bleed, &[*rect]) {
            continue;
        }
        let x0 = (((rect.x0 - tile.origin.0) * tile.scale).floor() as i64 - 2).clamp(0, side);
        let y0 = (((rect.y0 - tile.origin.1) * tile.scale).floor() as i64 - 2).clamp(0, side);
        let x1 = (((rect.x1 - tile.origin.0) * tile.scale).ceil() as i64 + 2).clamp(0, side);
        let y1 = (((rect.y1 - tile.origin.1) * tile.scale).ceil() as i64 + 2).clamp(0, side);
        region = (region.0.min(x0), region.1.min(y0), region.2.max(x1), region.3.max(y1));
    }
    if region.2 <= region.0 || region.3 <= region.1 {
        return None;
    }
    let x = region.0 as u32;
    let y = region.1 as u32;
    Some((
        x,
        y,
        View {
            origin: (tile.origin.0 + f64::from(x) / tile.scale, tile.origin.1 + f64::from(y) / tile.scale),
            scale: tile.scale,
            width: (region.2 - region.0) as u32,
            height: (region.3 - region.1) as u32,
        },
    ))
}

fn tier_for_scale(scale: f64) -> u8 {
    let mut tier = 0;
    let mut raster_scale = BASE_SCALE;
    while raster_scale < scale * MIN_SUPERSAMPLE && tier < MAX_TIER {
        tier += 1;
        raster_scale *= TIER_RATIO;
    }
    tier
}

fn visible_keys(camera: &View, margin: i32) -> Vec<Key> {
    let tier = tier_for_scale(camera.scale);
    keys_for_tier(camera, margin, tier)
}

fn keys_for_tier(camera: &View, margin: i32, tier: u8) -> Vec<Key> {
    let span = Key { x: 0, y: 0, tier }.span();
    let x0 = (camera.origin.0 / span).floor() as i32 - margin;
    let y0 = (camera.origin.1 / span).floor() as i32 - margin;
    let x1 = ((camera.origin.0 + f64::from(camera.width) / camera.scale - 1e-6) / span).floor() as i32 + margin;
    let y1 = ((camera.origin.1 + f64::from(camera.height) / camera.scale - 1e-6) / span).floor() as i32 + margin;
    (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| Key { x, y, tier })).collect()
}

pub fn required_slots(camera: &View, margin: i32) -> usize {
    visible_keys(camera, margin).len()
}

pub fn distinct_tile_counts(views: &[View], margin: i32) -> Vec<(u8, usize)> {
    let mut by_tier: HashMap<u8, HashSet<Key>> = HashMap::new();
    for view in views {
        for key in visible_keys(view, margin) {
            by_tier.entry(key.tier).or_default().insert(key);
        }
    }
    let mut counts = by_tier.into_iter().map(|(tier, keys)| (tier, keys.len())).collect::<Vec<_>>();
    counts.sort_by_key(|(tier, _)| *tier);
    counts
}

fn supports_scale(camera: &View) -> bool {
    camera.scale.is_finite() && camera.scale > 0.0 && camera.scale <= BASE_SCALE * TIER_RATIO.powi(i32::from(MAX_TIER))
}

struct Ready {
    key: Key,
    generation: u64,
    live_epoch: Option<u64>,
    image: Option<Pixmap>,
}

struct Uploaded {
    key: Key,
    generation: u64,
    slot: usize,
    result: Result<(), String>,
}

pub struct GeometryAtlas {
    compositor: Arc<Compositor>,
    pool: ThreadPool,
    prefetch_pool: ThreadPool,
    patch_pool: ThreadPool,
    upload_pool: ThreadPool,
    doc: Arc<Document>,
    content_bounds: Option<Rect>,
    page: NodeId,
    bleed: u32,
    skip_effects: bool,
    tx: mpsc::Sender<Ready>,
    rx: mpsc::Receiver<Ready>,
    uploaded_tx: mpsc::Sender<Uploaded>,
    uploaded_rx: mpsc::Receiver<Uploaded>,
    slots: Vec<Option<Key>>,
    resident: HashMap<Key, usize>,
    pending: HashSet<Key>,
    guarded_prefetch: HashSet<Key>,
    overview_keys: HashSet<Key>,
    overview_tier: u8,
    overview_scale: f64,
    uploading: HashMap<(Key, u64), usize>,
    touched: Vec<u64>,
    tick: u64,
    generation: u64,
    live_epoch: Arc<AtomicU64>,
    active_tier: u8,
    display_tier: Option<u8>,
    rendered: usize,
    upload_on_owner_thread: bool,
}

impl GeometryAtlas {
    pub fn new(
        doc: Arc<Document>,
        page: NodeId,
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
        capacity: u32,
        bleed: u32,
        skip_effects: bool,
    ) -> Result<Self, Box<dyn Error>> {
        if capacity == 0 {
            return Err("geometry atlas capacity must be positive".into());
        }
        let compositor = Arc::new(Compositor::empty_slots(width, height, TILE_SIZE, bleed, capacity, device, queue)?);
        let live_workers = std::env::var("LIZARD_ATLAS_LIVE_WORKERS").ok().and_then(|value| value.parse::<usize>().ok()).unwrap_or(2).clamp(1, 8);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(live_workers).thread_name(|i| format!("geometry-tile-{i}")).build()?;
        let prefetch_pool = rayon::ThreadPoolBuilder::new().num_threads(4).thread_name(|i| format!("geometry-prefetch-{i}")).build()?;
        let patch_pool = rayon::ThreadPoolBuilder::new().num_threads(2).thread_name(|i| format!("geometry-patch-{i}")).build()?;
        let upload_pool = rayon::ThreadPoolBuilder::new().num_threads(1).thread_name(|_| "geometry-upload".to_owned()).build()?;
        let (tx, rx) = mpsc::channel();
        let (uploaded_tx, uploaded_rx) = mpsc::channel();
        let bounds = content_bounds(&doc, page);
        Ok(Self {
            compositor,
            pool,
            prefetch_pool,
            patch_pool,
            upload_pool,
            doc,
            content_bounds: bounds,
            page,
            bleed,
            skip_effects,
            tx,
            rx,
            uploaded_tx,
            uploaded_rx,
            slots: vec![None; capacity as usize],
            resident: HashMap::new(),
            pending: HashSet::new(),
            guarded_prefetch: HashSet::new(),
            overview_keys: HashSet::new(),
            overview_tier: MAX_TIER,
            overview_scale: f64::INFINITY,
            uploading: HashMap::new(),
            touched: vec![0; capacity as usize],
            tick: 0,
            generation: 0,
            live_epoch: Arc::new(AtomicU64::new(0)),
            active_tier: 0,
            display_tier: None,
            rendered: 0,
            upload_on_owner_thread: false,
        })
    }

    /// The editor shares its WGPU queue with eframe. Serializing uploads with presentation
    /// avoids a lock inversion between `write_texture` and `submit` on different threads.
    pub fn upload_on_owner_thread(&mut self) {
        self.upload_on_owner_thread = true;
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        self.compositor.output_view()
    }

    /// Retire every old tile. In-flight jobs carry a generation and cannot restore stale pixels.
    pub fn replace_document(&mut self, doc: Arc<Document>, page: NodeId) -> Result<(), Box<dyn Error>> {
        self.replace_document_inner(doc, page, None)
    }

    /// Keep tiles outside supplied complete render damage; callers must include effect reach.
    pub fn replace_document_with_damage(&mut self, doc: Arc<Document>, page: NodeId, damage: &[Rect]) -> Result<(), Box<dyn Error>> {
        if damage.is_empty() || damage.iter().any(|rect| ![rect.x0, rect.y0, rect.x1, rect.y1].iter().all(|n| n.is_finite())) {
            return Err("partial atlas replacement requires finite damage bounds".into());
        }
        self.replace_document_inner(doc, page, Some(damage))
    }

    /// Derive effect-aware damage from both documents; fall back to full invalidation if the
    /// retained renderer cannot prove that a local patch is complete.
    pub fn patch_document_auto(&mut self, doc: Arc<Document>, page: NodeId) -> Result<usize, Box<dyn Error>> {
        if self.skip_effects || page != self.page || doc.node(page).is_none() {
            self.replace_document(doc, page)?;
            return Ok(0);
        }
        let Some(damage) = self.damage_between_documents(&doc, page)? else {
            self.replace_document(doc, page)?;
            return Ok(0);
        };
        if damage.is_empty() {
            self.replace_document_inner(doc, page, Some(&[]))?;
            return Ok(0);
        }
        self.patch_document(doc, page, &damage)
    }

    /// Keep unaffected resident tiles and prepare changed regions on the normal background
    /// queue. This avoids blocking the editor's presentation thread on a synchronous patch.
    pub fn invalidate_document_auto(&mut self, doc: Arc<Document>, page: NodeId) -> Result<(), Box<dyn Error>> {
        if self.skip_effects || page != self.page || doc.node(page).is_none() {
            return self.replace_document(doc, page);
        }
        let Some(damage) = self.damage_between_documents(&doc, page)? else {
            return self.replace_document(doc, page);
        };
        self.replace_document_inner(doc, page, Some(&damage))
    }

    fn damage_between_documents(&self, doc: &Document, page: NodeId) -> Result<Option<Vec<Rect>>, Box<dyn Error>> {
        let mut tiers: HashMap<u8, (i64, i64, i64, i64)> = HashMap::new();
        for key in self.resident.keys() {
            let side = i64::from(TILE_SIZE + self.bleed * 2);
            let x0 = i64::from(key.x) * i64::from(TILE_SIZE) - i64::from(self.bleed);
            let y0 = i64::from(key.y) * i64::from(TILE_SIZE) - i64::from(self.bleed);
            tiers
                .entry(key.tier)
                .and_modify(|bounds| {
                    bounds.0 = bounds.0.min(x0);
                    bounds.1 = bounds.1.min(y0);
                    bounds.2 = bounds.2.max(x0 + side);
                    bounds.3 = bounds.3.max(y0 + side);
                })
                .or_insert((x0, y0, x0 + side, y0 + side));
        }
        let mut damage = Vec::new();
        for (tier, (x0, y0, x1, y1)) in tiers {
            let scale = Key { x: 0, y: 0, tier }.scale();
            let view =
                View { origin: (x0 as f64 / scale, y0 as f64 / scale), scale, width: u32::try_from(x1 - x0)?, height: u32::try_from(y1 - y0)? };
            let Some(rects) = damage_between(&self.doc, doc, page, &view) else { return Ok(None) };
            for rect in rects {
                damage.push(Rect::new(
                    view.origin.0 + f64::from(rect.x) / scale,
                    view.origin.1 + f64::from(rect.y) / scale,
                    view.origin.0 + f64::from(rect.x + rect.width) / scale,
                    view.origin.1 + f64::from(rect.y + rect.height) / scale,
                ));
            }
        }
        Ok(Some(damage))
    }

    /// Patch resident faithful tiles in place. The supplied damage must include all effect dependencies.
    pub fn patch_document(&mut self, doc: Arc<Document>, page: NodeId, damage: &[Rect]) -> Result<usize, Box<dyn Error>> {
        if self.skip_effects
            || page != self.page
            || doc.node(page).is_none()
            || damage.is_empty()
            || damage.iter().any(|rect| ![rect.x0, rect.y0, rect.x1, rect.y1].iter().all(|n| n.is_finite()))
        {
            return Err("faithful atlas patch requires a valid page and finite damage".into());
        }
        let next_generation = self.generation.checked_add(1).ok_or("geometry atlas generation exhausted")?;
        let targets = self
            .resident
            .iter()
            .filter_map(|(key, slot)| tile_patch_view(*key, self.bleed, damage).map(|(x, y, view)| (*key, *slot, x, y, view)))
            .collect::<Vec<_>>();
        let mut by_tier: HashMap<u8, Vec<_>> = HashMap::new();
        for (key, slot, x, y, view) in &targets {
            by_tier.entry(key.tier).or_default().push((*key, *slot, *x, *y, view.clone()));
        }
        let mut tasks = Vec::new();
        let group_side = std::env::var("LIZARD_PATCH_GROUP_SIDE").ok().and_then(|value| value.parse::<i64>().ok()).unwrap_or(3072).clamp(256, 4096);
        for (tier, group) in by_tier {
            let scale = Key { x: 0, y: 0, tier }.scale();
            let location = |key: Key, x: u32, y: u32| {
                (
                    i64::from(key.x) * i64::from(TILE_SIZE) - i64::from(self.bleed) + i64::from(x),
                    i64::from(key.y) * i64::from(TILE_SIZE) - i64::from(self.bleed) + i64::from(y),
                )
            };
            let min_x = group.iter().map(|(key, _, x, y, _)| location(*key, *x, *y).0).min().ok_or("empty patch group")?;
            let min_y = group.iter().map(|(key, _, x, y, _)| location(*key, *x, *y).1).min().ok_or("empty patch group")?;
            let max_x = group.iter().map(|(key, _, x, y, view)| location(*key, *x, *y).0 + i64::from(view.width)).max().ok_or("empty patch group")?;
            let max_y =
                group.iter().map(|(key, _, x, y, view)| location(*key, *x, *y).1 + i64::from(view.height)).max().ok_or("empty patch group")?;
            let width = max_x - min_x;
            let height = max_y - min_y;
            if width > 0 && height > 0 && width <= group_side && height <= group_side {
                let view = View { origin: (min_x as f64 / scale, min_y as f64 / scale), scale, width: width as u32, height: height as u32 };
                let uploads = group
                    .into_iter()
                    .map(|(key, slot, x, y, tile)| {
                        let (gx, gy) = location(key, x, y);
                        (slot, x, y, (gx - min_x) as u32, (gy - min_y) as u32, tile.width, tile.height)
                    })
                    .collect();
                tasks.push((view, uploads));
            } else {
                for (_, slot, x, y, view) in group {
                    let (width, height) = (view.width, view.height);
                    tasks.push((view, vec![(slot, x, y, 0, 0, width, height)]));
                }
            }
        }
        let patches = self.patch_pool.install(|| {
            tasks.par_iter().map(|(view, _)| render_view(&doc, page, view, &Options { lod: true, ..Default::default() })).collect::<Vec<_>>()
        });
        if patches.iter().any(Option::is_none) {
            return Err("atlas patch render failed".into());
        }
        self.generation = next_generation;
        self.content_bounds = content_bounds(&doc, page);
        self.doc = doc;
        self.pending.clear();
        self.guarded_prefetch.clear();
        for ((_, uploads), patch) in tasks.iter().zip(patches.iter()) {
            let image = patch.as_ref().ok_or("atlas patch missing")?;
            for (slot, x, y, source_x, source_y, width, height) in uploads {
                if let Err(error) = self.compositor.patch_cpu_layer_region(*slot as u32, *x, *y, image, *source_x, *source_y, *width, *height) {
                    if let Some(key) = self.slots[*slot].take() {
                        self.resident.remove(&key);
                    }
                    self.compositor.hide_gpu_layer(*slot as u32)?;
                    return Err(error);
                }
            }
        }
        Ok(targets.len())
    }

    fn replace_document_inner(&mut self, doc: Arc<Document>, page: NodeId, damage: Option<&[Rect]>) -> Result<(), Box<dyn Error>> {
        if doc.node(page).is_none() {
            return Err("replacement page is missing".into());
        }
        self.generation = self.generation.checked_add(1).ok_or("geometry atlas generation exhausted")?;
        self.live_epoch.fetch_add(1, Ordering::Relaxed);
        let full = self.page != page || damage.is_none();
        self.content_bounds = content_bounds(&doc, page);
        self.doc = doc;
        self.page = page;
        self.pending.clear();
        self.guarded_prefetch.clear();
        self.overview_keys.clear();
        self.overview_tier = MAX_TIER;
        self.overview_scale = f64::INFINITY;
        self.rendered = 0;
        for (slot, key) in self.slots.iter_mut().enumerate() {
            if key.is_some_and(|key| {
                full || !key_intersects_content(key, self.bleed, self.content_bounds)
                    || damage.is_some_and(|damage| intersects_damage(key, self.bleed, damage))
            }) {
                if let Some(key) = key {
                    self.resident.remove(key);
                }
                self.compositor.hide_gpu_layer(slot as u32)?;
                if !self.uploading.values().any(|held| *held == slot) {
                    *key = None;
                }
            }
        }
        Ok(())
    }

    pub fn enable_one_pass(
        &mut self,
        format: wgpu::TextureFormat,
        full: &wgpu::TextureView,
        base: &wgpu::TextureView,
        low: &View,
    ) -> Result<(), Box<dyn Error>> {
        Arc::get_mut(&mut self.compositor).ok_or("atlas compositor is already shared")?.enable_residual_surface(format, full, base, low);
        Ok(())
    }

    pub fn request(&mut self, camera: &View) -> Result<(), Box<dyn Error>> {
        self.request_margin(camera, 1)
    }

    pub fn request_margin(&mut self, camera: &View, margin: i32) -> Result<(), Box<dyn Error>> {
        if !supports_scale(camera) {
            return Ok(());
        }
        let tier = tier_for_scale(camera.scale);
        if tier != self.active_tier {
            self.live_epoch.fetch_add(1, Ordering::Relaxed);
            self.pending.retain(|key| key.tier == tier || self.guarded_prefetch.contains(key));
            self.active_tier = tier;
        }
        self.tick += 1;
        let visible = visible_keys(camera, 0);
        if camera.scale <= self.overview_scale {
            self.overview_tier = tier;
            self.overview_scale = camera.scale;
            self.overview_keys = visible.iter().copied().collect();
        }
        for key in &visible {
            if let Some(&slot) = self.resident.get(key) {
                self.touched[slot] = self.tick;
            }
        }
        let mut requested = visible;
        requested.extend(visible_keys(camera, margin));
        requested.retain(|key| key_intersects_content(*key, self.bleed, self.content_bounds));
        self.schedule_keys(requested, false);
        Ok(())
    }

    /// Prepare nearby zoom tiers while leaving the current visible tier unchanged.
    pub fn prefetch_zoom_tiers(&mut self, camera: &View, highest_tier: u8) {
        if !supports_scale(camera) {
            return;
        }
        for tier in 1..=highest_tier.min(MAX_TIER) {
            self.schedule_keys(
                keys_for_tier(camera, 0, tier).into_iter().filter(|key| key_intersects_content(*key, self.bleed, self.content_bounds)).collect(),
                false,
            );
        }
    }

    /// Prepare the projected camera's raster tier while protecting the current view and its
    /// one-tile pan margin from eviction. Call this only when the current camera is idle.
    pub fn prefetch_view_guarded(&mut self, camera: &View) {
        if !supports_scale(camera) {
            return;
        }
        self.schedule_keys(
            visible_keys(camera, 0).into_iter().filter(|key| key_intersects_content(*key, self.bleed, self.content_bounds)).collect(),
            true,
        );
    }

    pub fn prefetched_zoom_tiers_ready(&self, camera: &View, highest_tier: u8) -> bool {
        (1..=highest_tier.min(MAX_TIER)).all(|tier| {
            keys_for_tier(camera, 0, tier)
                .iter()
                .all(|key| !key_intersects_content(*key, self.bleed, self.content_bounds) || self.resident.contains_key(key))
        })
    }

    fn schedule_keys(&mut self, requested: Vec<Key>, guarded: bool) {
        for key in requested {
            if self.pending.len() >= MAX_PENDING {
                break;
            }
            if guarded && self.guarded_prefetch.len() >= MAX_GUARDED_PREFETCH_PENDING {
                break;
            }
            if !guarded {
                self.guarded_prefetch.remove(&key);
            }
            if self.resident.contains_key(&key) || self.uploading.contains_key(&(key, self.generation)) || !self.pending.insert(key) {
                continue;
            }
            if guarded {
                self.guarded_prefetch.insert(key);
            }
            let doc = Arc::clone(&self.doc);
            let page = self.page;
            let bleed = self.bleed;
            let skip_effects = self.skip_effects;
            let tx = self.tx.clone();
            let generation = self.generation;
            let live_epoch = (!guarded).then(|| self.live_epoch.load(Ordering::Relaxed));
            let current_epoch = Arc::clone(&self.live_epoch);
            let pool = if guarded { &self.prefetch_pool } else { &self.pool };
            pool.spawn(move || {
                if live_epoch.is_some_and(|epoch| epoch != current_epoch.load(Ordering::Relaxed)) {
                    return;
                }
                let image = render_view_single(&doc, page, &key.view(bleed), &Options { lod: true, skip_effects, ..Default::default() });
                let _ = tx.send(Ready { key, generation, live_epoch, image });
            });
        }
    }

    pub fn install(&mut self, camera: &View, limit: usize) -> Result<usize, Box<dyn Error>> {
        while let Ok(uploaded) = self.uploaded_rx.try_recv() {
            self.uploading.remove(&(uploaded.key, uploaded.generation));
            if uploaded.generation != self.generation {
                if self.slots.get(uploaded.slot) == Some(&Some(uploaded.key)) {
                    self.slots[uploaded.slot] = None;
                }
                continue;
            }
            uploaded.result.map_err(|message| format!("atlas tile upload: {message}"))?;
            if self.slots.get(uploaded.slot) == Some(&Some(uploaded.key)) {
                self.resident.insert(uploaded.key, uploaded.slot);
                self.touched[uploaded.slot] = self.tick;
                self.rendered += 1;
                if Some(uploaded.key.tier) == self.display_tier {
                    self.compositor.show_cpu_layer(uploaded.slot as u32, uploaded.key.origin(), uploaded.key.span() as f32)?;
                }
            }
        }
        let protected: HashSet<_> = visible_keys(camera, 0).into_iter().collect();
        let protected_margin: HashSet<_> = visible_keys(camera, 1).into_iter().collect();
        let mut scheduled = 0;
        while scheduled < limit {
            let Ok(ready) = self.rx.try_recv() else { break };
            if ready.generation != self.generation || ready.live_epoch.is_some_and(|epoch| epoch != self.live_epoch.load(Ordering::Relaxed)) {
                continue;
            }
            self.pending.remove(&ready.key);
            let guarded = self.guarded_prefetch.remove(&ready.key);
            let Some(image) = ready.image else { continue };
            if self.resident.contains_key(&ready.key) || self.uploading.contains_key(&(ready.key, self.generation)) {
                continue;
            }
            let empty = self.slots.iter().position(Option::is_none);
            let candidate = empty.or_else(|| {
                self.slots
                    .iter()
                    .enumerate()
                    .filter(|(slot, key)| {
                        key.is_some_and(|key| !(if guarded { &protected_margin } else { &protected }).contains(&key))
                            && !self.uploading.values().any(|held| held == slot)
                    })
                    .min_by_key(|(slot, key)| (usize::from(key.is_some_and(|key| self.overview_keys.contains(&key))), self.touched[*slot]))
                    .map(|(slot, _)| slot)
            });
            let Some(slot) = candidate else { continue };
            if let Some(old) = self.slots[slot].take() {
                self.resident.remove(&old);
                self.compositor.hide_gpu_layer(slot as u32)?;
            }
            self.slots[slot] = Some(ready.key);
            if self.upload_on_owner_thread {
                self.compositor.update_cpu_layer(slot as u32, &image)?;
                self.resident.insert(ready.key, slot);
                self.touched[slot] = self.tick;
                self.rendered += 1;
                if Some(ready.key.tier) == self.display_tier {
                    self.compositor.show_cpu_layer(slot as u32, ready.key.origin(), ready.key.span() as f32)?;
                }
                scheduled += 1;
                continue;
            }
            self.uploading.insert((ready.key, ready.generation), slot);
            let compositor = Arc::clone(&self.compositor);
            let tx = self.uploaded_tx.clone();
            self.upload_pool.spawn(move || {
                let result = compositor.update_cpu_layer(slot as u32, &image).map_err(|error| error.to_string());
                let _ = tx.send(Uploaded { key: ready.key, generation: ready.generation, slot, result });
            });
            scheduled += 1;
        }
        self.refresh_display_tier(camera)?;
        Ok(scheduled)
    }

    pub fn covers(&self, camera: &View) -> bool {
        self.covers_margin(camera, 0)
    }

    fn covers_tier(&self, camera: &View, tier: u8) -> bool {
        keys_for_tier(camera, 0, tier)
            .iter()
            .all(|key| !key_intersects_content(*key, self.bleed, self.content_bounds) || self.resident.contains_key(key))
    }

    fn presentable_tier(&self, camera: &View) -> Option<u8> {
        if !supports_scale(camera) {
            return None;
        }
        let desired = tier_for_scale(camera.scale);
        if self.covers_tier(camera, desired) {
            return Some(desired);
        }
        let lower = desired.checked_sub(1)?;
        (Key { x: 0, y: 0, tier: lower }.scale() >= camera.scale && self.covers_tier(camera, lower)).then_some(lower)
    }

    pub fn covers_presentable(&self, camera: &View) -> bool {
        self.presentable_tier(camera).is_some()
    }

    fn refresh_display_tier(&mut self, camera: &View) -> Result<(), Box<dyn Error>> {
        let tier = self.presentable_tier(camera);
        if self.display_tier == tier {
            return Ok(());
        }
        self.display_tier = tier;
        for (slot, key) in self.slots.iter().enumerate() {
            if let Some(key) = key.filter(|key| Some(key.tier) == tier && self.resident.contains_key(key)) {
                self.compositor.show_cpu_layer(slot as u32, key.origin(), key.span() as f32)?;
            } else {
                self.compositor.hide_gpu_layer(slot as u32)?;
            }
        }
        Ok(())
    }

    pub fn covers_margin(&self, camera: &View, margin: i32) -> bool {
        supports_scale(camera)
            && visible_keys(camera, margin)
                .iter()
                .all(|key| !key_intersects_content(*key, self.bleed, self.content_bounds) || self.resident.contains_key(key))
    }

    pub fn render_submit(&self, camera: &View, clear: [f32; 3]) -> Result<(), Box<dyn Error>> {
        self.compositor.render_submit(camera.origin, camera.scale, (f64::from(clear[0]), f64::from(clear[1]), f64::from(clear[2])))?;
        Ok(())
    }

    pub fn enable_surface(&mut self, format: wgpu::TextureFormat) -> Result<(), Box<dyn Error>> {
        Arc::get_mut(&mut self.compositor).ok_or("atlas compositor is already shared")?.enable_surface(format);
        Ok(())
    }

    pub fn render_to_surface(&self, camera: &View, clear: [f32; 3], target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        self.compositor.render_submit_to_surface(
            camera.origin,
            camera.scale,
            (f64::from(clear[0]), f64::from(clear[1]), f64::from(clear[2])),
            target,
        )?;
        Ok(())
    }

    pub fn pixels(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        self.compositor.pixels()
    }

    pub fn render_one_pass_to_surface(&self, camera: &View, clear: [f32; 3], target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        self.compositor.render_residual_submit_to_surface(
            camera.origin,
            camera.scale,
            (f64::from(clear[0]), f64::from(clear[1]), f64::from(clear[2])),
            target,
        )?;
        Ok(())
    }

    pub fn render_one_pass(&self, camera: &View, clear: [f32; 3]) -> Result<Vec<u8>, Box<dyn Error>> {
        self.compositor.render_residual_submit(camera.origin, camera.scale, (f64::from(clear[0]), f64::from(clear[1]), f64::from(clear[2])))?;
        self.compositor.pixels()
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        (self.rendered, self.resident.len(), self.pending.len() + self.uploading.len())
    }

    pub fn overview_stats(&self) -> (u8, usize, usize) {
        (self.overview_tier, self.overview_keys.len(), self.overview_keys.iter().filter(|key| self.resident.contains_key(key)).count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_tiles_do_not_consume_atlas_slots() -> Result<(), Box<dyn Error>> {
        let mut doc = Document::new("Sparse icon");
        let page = doc.current_page.ok_or("page missing")?;
        doc.create(page, lizard_doc::NodeKind::Rectangle, "Mark", 0.0, 0.0, 32.0, 32.0)?;
        let bounds = content_bounds(&doc, page);
        assert!(key_intersects_content(Key { x: 0, y: 0, tier: 0 }, 128, bounds));
        assert!(!key_intersects_content(Key { x: 100, y: 100, tier: 0 }, 128, bounds));
        assert!(key_intersects_content(Key { x: 100, y: 100, tier: 0 }, 128, None));
        Ok(())
    }

    #[test]
    fn visible_keys_include_edges_and_overscan_without_duplicate_tiles() {
        let side = (f64::from(TILE_SIZE) / MIN_SUPERSAMPLE).floor() as u32;
        let view = View { origin: (0.0, 0.0), scale: BASE_SCALE / MIN_SUPERSAMPLE, width: side, height: side };
        assert_eq!(visible_keys(&view, 0), vec![Key { x: 0, y: 0, tier: 0 }]);
        assert_eq!(visible_keys(&view, 1).len(), 9);
        assert_eq!(required_slots(&view, 1), 9);
        let moved = View { origin: (Key { x: 0, y: 0, tier: 0 }.span() - 0.001, 0.0), ..view };
        assert_eq!(visible_keys(&moved, 0), vec![Key { x: 0, y: 0, tier: 0 }, Key { x: 1, y: 0, tier: 0 }]);
    }

    #[test]
    fn atlas_scale_is_bounded_by_its_raster_resolution() {
        let side = (f64::from(TILE_SIZE) / MIN_SUPERSAMPLE).floor() as u32;
        let view = View { origin: (0.0, 0.0), scale: BASE_SCALE / MIN_SUPERSAMPLE, width: side, height: side };
        assert_eq!(required_slots(&view, 0), 1);
        assert!(supports_scale(&view));
        assert_eq!(tier_for_scale(BASE_SCALE * 1.1), 1);
        assert_eq!(tier_for_scale(BASE_SCALE * 3.4), 3);
        assert_eq!(visible_keys(&View { scale: BASE_SCALE * 1.1, ..view }, 0)[0].tier, 1);
        assert!(!supports_scale(&View { scale: BASE_SCALE * 2.0_f64.powi(i32::from(MAX_TIER)) * 1.1, ..view }));
        assert!(!supports_scale(&View { scale: f64::NAN, ..view }));
    }
}
