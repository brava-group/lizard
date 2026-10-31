//! GPU composition of a sharp geometry image with a low-resolution effect residual.

use std::error::Error;
use std::sync::mpsc;

use lizard_render::{Pixmap, View};

const SHADER: &str = r#"
@group(0) @binding(0) var sharp: texture_2d_array<f32>;
@group(0) @binding(1) var low_full: texture_2d<f32>;
@group(0) @binding(2) var low_delta: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(5) var sharp_sampler: sampler;
struct Mapping { camera: vec4<f32>, source: vec4<f32>, dimensions: vec4<f32> };
@group(0) @binding(4) var<uniform> mapping: Mapping;
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = array<vec2<f32>, 6>(vec2<f32>(0.0,0.0),vec2<f32>(1.0,0.0),vec2<f32>(0.0,1.0),
                                  vec2<f32>(0.0,1.0),vec2<f32>(1.0,0.0),vec2<f32>(1.0,1.0))[i];
    var out: Out;
    out.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}
@fragment fn fs(input: Out) -> @location(0) vec4<f32> {
    let world = mapping.camera.xy + input.position.xy / mapping.camera.z;
    let source_pixel = (world - mapping.source.xy) * mapping.source.z;
    let uv = source_pixel / mapping.dimensions.zw;
    if (any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    let split = ceil(mapping.dimensions.z * 0.5);
    let layer = select(0, 1, source_pixel.x >= split);
    let tile_width = split + 2.0;
    let tile_x = select(source_pixel.x, source_pixel.x - split + 2.0, layer == 1);
    let base = textureSample(sharp, sharp_sampler, vec2<f32>(tile_x / tile_width, uv.y), layer);
    if (mapping.source.w > 0.5) {
        return base;
    }
    let effect = textureSample(low_delta, linear_sampler, uv);
    return clamp(base + effect, vec4<f32>(0.0), vec4<f32>(1.0));
}
"#;

const DELTA_SHADER: &str = r#"
@group(0) @binding(0) var low_full: texture_2d<f32>;
@group(0) @binding(1) var low_base: texture_2d<f32>;
@group(0) @binding(2) var low_delta: texture_storage_2d<rgba16float, write>;
@compute @workgroup_size(8, 8)
fn make_delta(@builtin(global_invocation_id) id: vec3<u32>) {
    let xy = vec2<i32>(id.xy);
    if (id.x >= textureDimensions(low_full).x || id.y >= textureDimensions(low_full).y) {
        return;
    }
    let delta = textureLoad(low_full, xy, 0) - textureLoad(low_base, xy, 0);
    textureStore(low_delta, xy, delta);
}
"#;

pub struct GpuResidual {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sharp: wgpu::Texture,
    low_full: wgpu::Texture,
    low_base: wgpu::Texture,
    _low_delta: wgpu::Texture,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    surface_pipeline: Option<wgpu::RenderPipeline>,
    delta_pipeline: wgpu::ComputePipeline,
    delta_bind: wgpu::BindGroup,
    bind: wgpu::BindGroup,
    mapping: wgpu::Buffer,
    width: u32,
    height: u32,
    low_width: u32,
    low_height: u32,
    output_width: u32,
    output_height: u32,
    baked: bool,
}

impl GpuResidual {
    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
        low_width: u32,
        low_height: u32,
        output_width: u32,
        output_height: u32,
        baked: bool,
        surface_format: Option<wgpu::TextureFormat>,
    ) -> Self {
        let make_texture = |label, width, height, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            })
        };
        let input_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let sharp = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("two-column sharp geometry"),
            size: wgpu::Extent3d { width: width.div_ceil(2) + 2, height, depth_or_array_layers: 2 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: input_usage,
            view_formats: &[],
        });
        let low_full = make_texture("faithful low resolution", low_width, low_height, input_usage);
        let low_base = make_texture("low resolution geometry", low_width, low_height, input_usage);
        let low_delta = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("signed effect residual"),
            size: wgpu::Extent3d { width: low_width, height: low_height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output = make_texture(
            "sharp geometry with soft effects",
            output_width,
            output_height,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect residual inputs"),
            entries: &[0, 1, 2]
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: if binding == 0 { wgpu::TextureViewDimension::D2Array } else { wgpu::TextureViewDimension::D2 },
                        multisampled: false,
                    },
                    count: None,
                })
                .into_iter()
                .chain([
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ])
                .collect::<Vec<_>>(),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sharp_filter = if std::env::args().any(|arg| arg == "--sharp-nearest") { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
        let sharp_sampler =
            device.create_sampler(&wgpu::SamplerDescriptor { min_filter: sharp_filter, mag_filter: sharp_filter, ..Default::default() });
        let mapping = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("effect residual camera mapping"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let views = [
            sharp.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() }),
            low_full.create_view(&Default::default()),
            low_delta.create_view(&Default::default()),
        ];
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("effect residual bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[0]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[1]) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[2]) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: mapping.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&sharp_sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effect residual shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effect residual pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("effect residual pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let surface_pipeline = surface_format.map(|format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("effect residual direct surface pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        });
        let delta_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect residual delta inputs"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let delta_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("effect residual delta bind group"),
            layout: &delta_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&low_full.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&low_base.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&low_delta.create_view(&Default::default())) },
            ],
        });
        let delta_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effect residual delta shader"),
            source: wgpu::ShaderSource::Wgsl(DELTA_SHADER.into()),
        });
        let delta_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effect residual delta pipeline layout"),
            bind_group_layouts: &[Some(&delta_layout)],
            immediate_size: 0,
        });
        let delta_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("effect residual delta pipeline"),
            layout: Some(&delta_pipeline_layout),
            module: &delta_shader,
            entry_point: Some("make_delta"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            device,
            queue,
            sharp,
            low_full,
            low_base,
            _low_delta: low_delta,
            output,
            output_view,
            pipeline,
            surface_pipeline,
            delta_pipeline,
            delta_bind,
            bind,
            mapping,
            width,
            height,
            low_width,
            low_height,
            output_width,
            output_height,
            baked,
        }
    }

    fn upload(&self, texture: &wgpu::Texture, image: &Pixmap, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
        if image.width() != width || image.height() != height {
            return Err("residual texture dimensions differ".into());
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            image.data(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        Ok(())
    }

    pub fn upload_images(&self, sharp: &Pixmap, low_full: &Pixmap, low_base: &Pixmap) -> Result<(), Box<dyn Error>> {
        if sharp.width() != self.width || sharp.height() != self.height {
            return Err("sharp texture dimensions differ".into());
        }
        let split = self.width.div_ceil(2);
        let tile_width = split + 2;
        for layer in 0..2 {
            let source_x = if layer == 0 { 0 } else { split.saturating_sub(2) };
            let mut pixels = vec![0u8; tile_width as usize * self.height as usize * 4];
            let copied = (self.width - source_x).min(tile_width) as usize;
            let row_bytes = tile_width as usize * 4;
            let copied_bytes = copied * 4;
            for y in 0..self.height as usize {
                let from = (y * self.width as usize + source_x as usize) * 4;
                let to = y * row_bytes;
                pixels[to..to + copied_bytes].copy_from_slice(&sharp.data()[from..from + copied_bytes]);
                if copied < tile_width as usize {
                    let edge = [
                        sharp.data()[from + copied_bytes - 4],
                        sharp.data()[from + copied_bytes - 3],
                        sharp.data()[from + copied_bytes - 2],
                        sharp.data()[from + copied_bytes - 1],
                    ];
                    for x in copied..tile_width as usize {
                        let at = to + x * 4;
                        pixels[at..at + 4].copy_from_slice(&edge);
                    }
                }
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.sharp,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(tile_width * 4), rows_per_image: Some(self.height) },
                wgpu::Extent3d { width: tile_width, height: self.height, depth_or_array_layers: 1 },
            );
        }
        self.upload(&self.low_full, low_full, self.low_width, self.low_height)?;
        self.upload(&self.low_base, low_base, self.low_width, self.low_height)?;
        if !self.baked {
            let mut encoder =
                self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("effect residual delta preparation") });
            {
                let mut pass = encoder
                    .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("effect residual delta preparation"), timestamp_writes: None });
                pass.set_pipeline(&self.delta_pipeline);
                pass.set_bind_group(0, &self.delta_bind, &[]);
                pass.dispatch_workgroups(self.low_width.div_ceil(8), self.low_height.div_ceil(8), 1);
            }
            self.queue.submit([encoder.finish()]);
        }
        Ok(())
    }

    fn render_target(&self, source: &View, camera: &View, target: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline) {
        let mut data = Vec::with_capacity(48);
        for value in [
            camera.origin.0 as f32,
            camera.origin.1 as f32,
            camera.scale as f32,
            0.0,
            source.origin.0 as f32,
            source.origin.1 as f32,
            source.scale as f32,
            if self.baked { 1.0 } else { 0.0 },
            self.output_width as f32,
            self.output_height as f32,
            self.width as f32,
            self.height as f32,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        self.queue.write_buffer(&self.mapping, 0, &data);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("effect residual composition") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("effect residual pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }

    pub fn render_submit(&self, source: &View, camera: &View) {
        self.render_target(source, camera, &self.output_view, &self.pipeline);
    }

    pub fn render_submit_into(&self, source: &View, camera: &View, target: &wgpu::TextureView) {
        self.render_target(source, camera, target, &self.pipeline);
    }

    pub fn render_submit_to_surface(&self, source: &View, camera: &View, target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        let pipeline = self.surface_pipeline.as_ref().ok_or("direct surface pipeline unavailable")?;
        self.render_target(source, camera, target, pipeline);
        Ok(())
    }

    pub fn wait(&self) -> Result<(), Box<dyn Error>> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }

    pub fn pixels(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let stride = (self.output_width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("effect residual readback"),
            size: u64::from(stride) * u64::from(self.output_height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("effect residual readback copy") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(self.output_height) },
            },
            wgpu::Extent3d { width: self.output_width, height: self.output_height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.wait()?;
        receiver.recv()??;
        let data = buffer.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity(self.output_width as usize * self.output_height as usize * 4);
        for row in data.chunks_exact(stride as usize) {
            pixels.extend_from_slice(&row[..self.output_width as usize * 4]);
        }
        Ok(pixels)
    }
}
