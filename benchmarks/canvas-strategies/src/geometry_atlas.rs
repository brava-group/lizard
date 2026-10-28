//! A bounded, asynchronously prepared geometry cache for pan and zoom experiments.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::{Arc, mpsc};

use figcraft_doc::{Document, NodeId};
use figcraft_render::{Options, Pixmap, View, render_view};
use rayon::ThreadPool;

use crate::compositor::Compositor;

const TILE_SIZE: u32 = 1024;
const BLEED: u32 = 2;
const SCALE: f64 = 2.4;
pub const DEFAULT_CAPACITY: u32 = 160;
const MAX_PENDING: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Key {
    x: i32,
    y: i32,
}

impl Key {
    fn span() -> f64 {
        f64::from(TILE_SIZE) / SCALE
    }

    fn view(self) -> View {
        View {
            origin: (
                (f64::from(self.x) * f64::from(TILE_SIZE) - f64::from(BLEED)) / SCALE,
                (f64::from(self.y) * f64::from(TILE_SIZE) - f64::from(BLEED)) / SCALE,
            ),
            scale: SCALE,
            width: TILE_SIZE + BLEED * 2,
            height: TILE_SIZE + BLEED * 2,
        }
    }
}

fn visible_keys(camera: &View, margin: i32) -> Vec<Key> {
    let span = Key::span();
    let x0 = (camera.origin.0 / span).floor() as i32 - margin;
    let y0 = (camera.origin.1 / span).floor() as i32 - margin;
    let x1 = ((camera.origin.0 + f64::from(camera.width) / camera.scale - 1e-6) / span).floor() as i32 + margin;
    let y1 = ((camera.origin.1 + f64::from(camera.height) / camera.scale - 1e-6) / span).floor() as i32 + margin;
    (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| Key { x, y })).collect()
}

pub fn required_slots(camera: &View, margin: i32) -> usize {
    visible_keys(camera, margin).len()
}

struct Ready {
    key: Key,
    image: Option<Pixmap>,
}

struct Uploaded {
    key: Key,
    slot: usize,
    result: Result<(), String>,
}

pub struct GeometryAtlas {
    compositor: Arc<Compositor>,
    pool: ThreadPool,
    upload_pool: ThreadPool,
    doc: Arc<Document>,
    page: NodeId,
    tx: mpsc::Sender<Ready>,
    rx: mpsc::Receiver<Ready>,
    uploaded_tx: mpsc::Sender<Uploaded>,
    uploaded_rx: mpsc::Receiver<Uploaded>,
    slots: Vec<Option<Key>>,
    resident: HashMap<Key, usize>,
    pending: HashSet<Key>,
    uploading: HashMap<Key, usize>,
    touched: Vec<u64>,
    tick: u64,
    rendered: usize,
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
    ) -> Result<Self, Box<dyn Error>> {
        if capacity == 0 {
            return Err("geometry atlas capacity must be positive".into());
        }
        let compositor = Arc::new(Compositor::empty_slots(width, height, TILE_SIZE, BLEED, capacity, device, queue)?);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).thread_name(|i| format!("geometry-tile-{i}")).build()?;
        let upload_pool = rayon::ThreadPoolBuilder::new().num_threads(1).thread_name(|_| "geometry-upload".to_owned()).build()?;
        let (tx, rx) = mpsc::channel();
        let (uploaded_tx, uploaded_rx) = mpsc::channel();
        Ok(Self {
            compositor,
            pool,
            upload_pool,
            doc,
            page,
            tx,
            rx,
            uploaded_tx,
            uploaded_rx,
            slots: vec![None; capacity as usize],
            resident: HashMap::new(),
            pending: HashSet::new(),
            uploading: HashMap::new(),
            touched: vec![0; capacity as usize],
            tick: 0,
            rendered: 0,
        })
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        self.compositor.output_view()
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

    pub fn request(&mut self, camera: &View) {
        self.request_margin(camera, 1);
    }

    pub fn request_margin(&mut self, camera: &View, margin: i32) {
        self.tick += 1;
        let visible = visible_keys(camera, 0);
        for key in &visible {
            if let Some(&slot) = self.resident.get(key) {
                self.touched[slot] = self.tick;
            }
        }
        let mut requested = visible;
        requested.extend(visible_keys(camera, margin));
        for key in requested {
            if self.pending.len() >= MAX_PENDING {
                break;
            }
            if self.resident.contains_key(&key) || self.uploading.contains_key(&key) || !self.pending.insert(key) {
                continue;
            }
            let doc = Arc::clone(&self.doc);
            let page = self.page;
            let tx = self.tx.clone();
            self.pool.spawn(move || {
                let image = render_view(&doc, page, &key.view(), &Options { lod: true, skip_effects: true, ..Default::default() });
                let _ = tx.send(Ready { key, image });
            });
        }
    }

    pub fn install(&mut self, camera: &View, limit: usize) -> Result<usize, Box<dyn Error>> {
        while let Ok(uploaded) = self.uploaded_rx.try_recv() {
            self.uploading.remove(&uploaded.key);
            uploaded.result.map_err(|message| format!("atlas tile upload: {message}"))?;
            if self.slots.get(uploaded.slot) == Some(&Some(uploaded.key)) {
                self.resident.insert(uploaded.key, uploaded.slot);
                self.touched[uploaded.slot] = self.tick;
                self.rendered += 1;
            }
        }
        let protected: HashSet<_> = visible_keys(camera, 0).into_iter().collect();
        let mut scheduled = 0;
        while scheduled < limit {
            let Ok(ready) = self.rx.try_recv() else { break };
            self.pending.remove(&ready.key);
            let Some(image) = ready.image else { continue };
            if self.resident.contains_key(&ready.key) {
                continue;
            }
            let candidate = self.slots.iter().position(Option::is_none).or_else(|| {
                self.slots
                    .iter()
                    .enumerate()
                    .filter(|(slot, key)| key.is_some_and(|key| !protected.contains(&key)) && !self.uploading.values().any(|held| held == slot))
                    .min_by_key(|(slot, _)| self.touched[*slot])
                    .map(|(slot, _)| slot)
            });
            let Some(slot) = candidate else { continue };
            if let Some(old) = self.slots[slot].take() {
                self.resident.remove(&old);
            }
            self.slots[slot] = Some(ready.key);
            self.uploading.insert(ready.key, slot);
            let compositor = Arc::clone(&self.compositor);
            let tx = self.uploaded_tx.clone();
            self.upload_pool.spawn(move || {
                let result = compositor
                    .update_cpu_layer(
                        slot as u32,
                        ((f64::from(ready.key.x) * Key::span()) as f32, (f64::from(ready.key.y) * Key::span()) as f32),
                        Key::span() as f32,
                        &image,
                    )
                    .map_err(|error| error.to_string());
                let _ = tx.send(Uploaded { key: ready.key, slot, result });
            });
            scheduled += 1;
        }
        Ok(scheduled)
    }

    pub fn covers(&self, camera: &View) -> bool {
        self.covers_margin(camera, 0)
    }

    pub fn covers_margin(&self, camera: &View, margin: i32) -> bool {
        visible_keys(camera, margin).iter().all(|key| self.resident.contains_key(key))
    }

    pub fn render_submit(&self, camera: &View, clear: [f32; 3]) -> Result<(), Box<dyn Error>> {
        self.compositor.render_submit(camera.origin, camera.scale, (f64::from(clear[0]), f64::from(clear[1]), f64::from(clear[2])))?;
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_keys_include_edges_and_overscan_without_duplicate_tiles() {
        let view = View { origin: (0.0, 0.0), scale: SCALE, width: TILE_SIZE, height: TILE_SIZE };
        assert_eq!(visible_keys(&view, 0), vec![Key { x: 0, y: 0 }]);
        assert_eq!(visible_keys(&view, 1).len(), 9);
        assert_eq!(required_slots(&view, 1), 9);
        let moved = View { origin: (Key::span() - 0.001, 0.0), ..view };
        assert_eq!(visible_keys(&moved, 0), vec![Key { x: 0, y: 0 }, Key { x: 1, y: 0 }]);
    }
}
