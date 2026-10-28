//! Offscreen GPU composition of full-fidelity CPU tiles. Camera changes update one uniform.

use std::error::Error;
use std::sync::mpsc;
use std::time::Instant;

use figcraft_render::Pixmap;

const SHADER: &str = r#"
struct Camera { origin: vec2<f32>, zoom: f32, width: f32, height: f32,
                side: f32, bleed: f32, tile_size: f32 };
struct Tile { origin: vec2<f32>, span: f32, layer: u32, min_zoom: f32, _pad: f32 };
@group(0) @binding(0) var atlas: texture_2d_array<f32>;
@group(0) @binding(1) var<storage, read> tiles: array<Tile>;
@group(0) @binding(2) var<uniform> camera: Camera;
@group(0) @binding(3) var linear_sampler: sampler;
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>,
             @location(1) @interpolate(flat) layer: u32,
             @location(2) @interpolate(flat) min_zoom: f32 };
@vertex fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Out {
  let quad = array<vec2<f32>, 6>(vec2<f32>(0.0,0.0),vec2<f32>(1.0,0.0),vec2<f32>(0.0,1.0),
                                  vec2<f32>(0.0,1.0),vec2<f32>(1.0,0.0),vec2<f32>(1.0,1.0));
  let local = quad[vi];
  let tile = tiles[ii];
  let px = (tile.origin + local * tile.span - camera.origin) * camera.zoom;
  var out: Out;
  out.position = vec4<f32>(px.x / camera.width * 2.0 - 1.0, 1.0 - px.y / camera.height * 2.0, 0.0, 1.0);
  out.uv = (local * camera.tile_size + vec2<f32>(camera.bleed)) / camera.side;
  out.layer = tile.layer;
  out.min_zoom = tile.min_zoom;
  return out;
}
@fragment fn fs(in: Out) -> @location(0) vec4<f32> {
  if (camera.zoom < in.min_zoom) { discard; }
  return textureSample(atlas, linear_sampler, in.uv, i32(in.layer));
}
"#;

pub struct Tile<'a> {
    pub x: i32,
    pub y: i32,
    pub image: &'a Pixmap,
}

pub struct GpuTile<'a> {
    pub x: i32,
    pub y: i32,
    pub texture: &'a wgpu::Texture,
}

/// A cached tile with its own world-space extent. Later layers cover earlier ones.
pub struct GpuTileLayer<'a> {
    pub origin: (f32, f32),
    pub span: f32,
    pub min_zoom: f32,
    pub texture: &'a wgpu::Texture,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    atlas: wgpu::Texture,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    camera: wgpu::Buffer,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    count: u32,
    width: u32,
    height: u32,
    tile_size: u32,
    bleed: u32,
}

impl Compositor {
    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.target_view
    }

    pub fn new(width: u32, height: u32, tile_size: u32, bleed: u32, scale: f32, tiles: &[Tile<'_>]) -> Result<Self, Box<dyn Error>> {
        let first = tiles.first().ok_or("no tiles to compose")?;
        let side = first.image.width();
        if tiles.iter().any(|tile| tile.image.width() != side || tile.image.height() != side) {
            return Err("tile sizes differ".into());
        }
        if tiles.iter().any(|tile| tile.image.data().chunks_exact(4).any(|pixel| pixel[3] != 255)) {
            return Err("opaque tile compositor requires an opaque page background".into());
        }
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard benchmark tile atlas"),
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: tiles.len() as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (layer, tile) in tiles.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                tile.image.data(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(side * 4), rows_per_image: Some(side) },
                wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            );
        }
        let world = tile_size as f32 / scale;
        let positions: Vec<(f32, f32, f32, f32)> = tiles.iter().map(|tile| (tile.x as f32 * world, tile.y as f32 * world, world, 0.0)).collect();
        Self::build(width, height, tile_size, bleed, scale, device, queue, atlas, &positions)
    }

    pub fn from_gpu(
        width: u32,
        height: u32,
        tile_size: u32,
        bleed: u32,
        scale: f32,
        device: wgpu::Device,
        queue: wgpu::Queue,
        tiles: &[GpuTile<'_>],
    ) -> Result<Self, Box<dyn Error>> {
        let first = tiles.first().ok_or("no GPU tiles to compose")?;
        let side = first.texture.size().width;
        if tiles.iter().any(|tile| {
            tile.texture.size().width != side || tile.texture.size().height != side || tile.texture.format() != wgpu::TextureFormat::Rgba8Unorm
        }) {
            return Err("GPU tile sizes or formats differ".into());
        }
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard benchmark GPU-generated atlas"),
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: tiles.len() as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("GPU tile atlas copy") });
        for (layer, tile) in tiles.iter().enumerate() {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: tile.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo {
                    texture: &atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            );
        }
        queue.submit([encoder.finish()]);
        let world = tile_size as f32 / scale;
        let positions: Vec<(f32, f32, f32, f32)> = tiles.iter().map(|tile| (tile.x as f32 * world, tile.y as f32 * world, world, 0.0)).collect();
        Self::build(width, height, tile_size, bleed, scale, device, queue, atlas, &positions)
    }

    pub fn from_gpu_layers(
        width: u32,
        height: u32,
        tile_size: u32,
        bleed: u32,
        device: wgpu::Device,
        queue: wgpu::Queue,
        layers: &[GpuTileLayer<'_>],
    ) -> Result<Self, Box<dyn Error>> {
        let first = layers.first().ok_or("no GPU layers to compose")?;
        let side = first.texture.size().width;
        if layers.iter().any(|layer| {
            layer.texture.size().width != side || layer.texture.size().height != side || layer.texture.format() != wgpu::TextureFormat::Rgba8Unorm
        }) {
            return Err("GPU layer sizes or formats differ".into());
        }
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard benchmark mixed-scale tile atlas"),
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: layers.len() as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mixed-scale atlas init") });
        for (index, layer) in layers.iter().enumerate() {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: layer.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo {
                    texture: &atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: index as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            );
        }
        queue.submit([encoder.finish()]);
        let positions: Vec<(f32, f32, f32, f32)> = layers.iter().map(|layer| (layer.origin.0, layer.origin.1, layer.span, layer.min_zoom)).collect();
        Self::build(width, height, tile_size, bleed, 1.0, device, queue, atlas, &positions)
    }

    /// Allocate a fixed-size atlas. Unassigned slots are clipped outside the viewport.
    pub fn empty_slots(
        width: u32,
        height: u32,
        tile_size: u32,
        bleed: u32,
        capacity: u32,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Result<Self, Box<dyn Error>> {
        if capacity == 0 {
            return Err("tile atlas needs at least one slot".into());
        }
        let max_layers = device.limits().max_texture_array_layers;
        if capacity > max_layers {
            return Err(format!("tile atlas requests {capacity} layers, but this GPU supports {max_layers}").into());
        }
        let side = tile_size + bleed * 2;
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bounded streaming tile atlas"),
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: capacity },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let positions = vec![(1.0e9, 1.0e9, 0.0, 0.0); capacity as usize];
        Self::build(width, height, tile_size, bleed, 1.0, device, queue, atlas, &positions)
    }

    fn build(
        width: u32,
        height: u32,
        tile_size: u32,
        bleed: u32,
        _scale: f32,
        device: wgpu::Device,
        queue: wgpu::Queue,
        atlas: wgpu::Texture,
        positions: &[(f32, f32, f32, f32)],
    ) -> Result<Self, Box<dyn Error>> {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard benchmark ultrawide output"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut tile_bytes = Vec::with_capacity(positions.len() * 24);
        for (layer, &(x, y, world, min_zoom)) in positions.iter().enumerate() {
            for value in [x, y, world] {
                tile_bytes.extend_from_slice(&value.to_le_bytes());
            }
            tile_bytes.extend_from_slice(&(layer as u32).to_le_bytes());
            tile_bytes.extend_from_slice(&min_zoom.to_le_bytes());
            tile_bytes.extend_from_slice(&0f32.to_le_bytes());
        }
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard benchmark tile instances"),
            size: tile_bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&instances, 0, &tile_bytes);
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard benchmark camera"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let atlas_view =
            atlas.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard benchmark compositor bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard benchmark compositor bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 1, resource: instances.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: camera.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard benchmark compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Lizard benchmark tile compositor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Ok(Self {
            device,
            queue,
            atlas,
            pipeline,
            bind,
            camera,
            instances,
            target,
            target_view,
            count: positions.len() as u32,
            width,
            height,
            tile_size,
            bleed,
        })
    }

    pub fn replace_gpu_tile(&self, layer: u32, source: &wgpu::Texture) -> Result<f64, Box<dyn Error>> {
        if layer >= self.count || source.size().width != self.atlas.size().width || source.size().height != self.atlas.size().height {
            return Err("invalid GPU atlas tile replacement".into());
        }
        let start = Instant::now();
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("GPU tile edit patch") });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: source, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: source.size().width, height: source.size().height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(start.elapsed().as_secs_f64() * 1000.0)
    }

    /// Schedule an atlas patch in queue order; the next compose pass consumes it.
    pub fn enqueue_gpu_tile(&self, layer: u32, source: &wgpu::Texture) -> Result<(), Box<dyn Error>> {
        if layer >= self.count || source.size().width != self.atlas.size().width || source.size().height != self.atlas.size().height {
            return Err("invalid GPU atlas tile replacement".into());
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("async GPU atlas patch") });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: source, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: source.size().width, height: source.size().height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    /// Replace one slot's image and world extent without rebuilding the atlas.
    pub fn update_gpu_layer(&self, layer: u32, origin: (f32, f32), span: f32, min_zoom: f32, source: &wgpu::Texture) -> Result<(), Box<dyn Error>> {
        if layer >= self.count {
            return Err("invalid streaming atlas slot".into());
        }
        let mut bytes = [0u8; 24];
        bytes[0..4].copy_from_slice(&origin.0.to_le_bytes());
        bytes[4..8].copy_from_slice(&origin.1.to_le_bytes());
        bytes[8..12].copy_from_slice(&span.to_le_bytes());
        bytes[12..16].copy_from_slice(&layer.to_le_bytes());
        bytes[16..20].copy_from_slice(&min_zoom.to_le_bytes());
        self.queue.write_buffer(&self.instances, u64::from(layer) * 24, &bytes);
        self.enqueue_gpu_tile(layer, source)
    }

    /// Hide a filled atlas slot without copying or destroying its texture.
    pub fn hide_gpu_layer(&self, layer: u32) -> Result<(), Box<dyn Error>> {
        if layer >= self.count {
            return Err("invalid streaming atlas slot".into());
        }
        let mut bytes = [0u8; 24];
        bytes[0..4].copy_from_slice(&1.0e9f32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1.0e9f32.to_le_bytes());
        bytes[12..16].copy_from_slice(&layer.to_le_bytes());
        self.queue.write_buffer(&self.instances, u64::from(layer) * 24, &bytes);
        Ok(())
    }

    pub fn render(&self, origin: (f64, f64), zoom: f64, clear: (f64, f64, f64)) -> Result<f64, Box<dyn Error>> {
        let started = Instant::now();
        self.render_submit(origin, zoom, clear)?;
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(started.elapsed().as_secs_f64() * 1000.0)
    }

    /// Submit composition without waiting for the GPU. The caller can track queue completion.
    pub fn render_submit(&self, origin: (f64, f64), zoom: f64, clear: (f64, f64, f64)) -> Result<f64, Box<dyn Error>> {
        let mut data = Vec::with_capacity(32);
        for value in [
            origin.0 as f32,
            origin.1 as f32,
            zoom as f32,
            self.width as f32,
            self.height as f32,
            (self.tile_size + self.bleed * 2) as f32,
            self.bleed as f32,
            self.tile_size as f32,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let start = Instant::now();
        self.queue.write_buffer(&self.camera, 0, &data);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard benchmark compose") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Lizard benchmark tile composition"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: clear.0, g: clear.1, b: clear.2, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..6, 0..self.count);
        }
        self.queue.submit([encoder.finish()]);
        Ok(start.elapsed().as_secs_f64() * 1000.0)
    }

    pub fn pixels(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let stride = (self.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard benchmark compositor readback"),
            size: u64::from(stride) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(self.height) },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        receiver.recv()??;
        let data = buffer.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        for row in data.chunks_exact(stride as usize) {
            pixels.extend_from_slice(&row[..self.width as usize * 4]);
        }
        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_atlas_returns_an_error_before_gpu_validation() -> Result<(), Box<dyn Error>> {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            return Ok(());
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let too_many = device.limits().max_texture_array_layers + 1;
        let result = Compositor::empty_slots(64, 32, 16, 0, too_many, device, queue);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn streaming_slot_moves_without_leaving_old_pixels_and_respects_zoom() -> Result<(), Box<dyn Error>> {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            return Ok(());
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let compositor = Compositor::empty_slots(64, 32, 16, 0, 1, device.clone(), queue.clone())?;
        let solid = |label, rgba: [u8; 4]| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &rgba.repeat(16 * 16),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16 * 4), rows_per_image: Some(16) },
                wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            );
            texture
        };
        let red = solid("red", [255, 0, 0, 255]);
        let blue = solid("blue", [0, 0, 255, 255]);
        let pixel = |pixels: &[u8], x: usize, y: usize| -> [u8; 4] {
            let start = (y * 64 + x) * 4;
            [pixels[start], pixels[start + 1], pixels[start + 2], pixels[start + 3]]
        };
        compositor.update_gpu_layer(0, (0.0, 0.0), 16.0, 0.0, &red)?;
        compositor.render((0.0, 0.0), 1.0, (1.0, 1.0, 1.0))?;
        assert_eq!(pixel(&compositor.pixels()?, 8, 8), [255, 0, 0, 255]);
        compositor.update_gpu_layer(0, (32.0, 0.0), 16.0, 0.0, &blue)?;
        compositor.render((0.0, 0.0), 1.0, (1.0, 1.0, 1.0))?;
        let pixels = compositor.pixels()?;
        assert_eq!(pixel(&pixels, 8, 8), [255, 255, 255, 255]);
        assert_eq!(pixel(&pixels, 40, 8), [0, 0, 255, 255]);
        compositor.update_gpu_layer(0, (0.0, 0.0), 16.0, 2.0, &blue)?;
        compositor.render((0.0, 0.0), 1.0, (1.0, 1.0, 1.0))?;
        assert_eq!(pixel(&compositor.pixels()?, 8, 8), [255, 255, 255, 255]);
        compositor.render((0.0, 0.0), 2.0, (1.0, 1.0, 1.0))?;
        assert_eq!(pixel(&compositor.pixels()?, 8, 8), [0, 0, 255, 255]);
        compositor.hide_gpu_layer(0)?;
        compositor.render((0.0, 0.0), 2.0, (1.0, 1.0, 1.0))?;
        assert_eq!(pixel(&compositor.pixels()?, 8, 8), [255, 255, 255, 255]);
        Ok(())
    }
}
