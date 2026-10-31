//! Document-stable grain over an isolated layer. Both source and result stay on the GPU.

use lizard_doc::{Noise, NoiseMode};
use lizard_geom::Affine;
use wgpu::util::DeviceExt;

pub(crate) const GRAIN_HASH_WGSL: &str = include_str!("grain_hash.wgsl");

const SHADER: &str = r#"
struct Params {
    cell_x: vec4<f32>,
    cell_y: vec4<f32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    settings: vec4<f32>,
    tail: vec4<u32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var destination: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.tail.z || id.y >= params.tail.w { return; }
    let xy = vec2<i32>(id.xy);
    let src = textureLoad(source, xy, 0);
    if src.a <= 0.0 || params.settings.x <= 0.0 {
        textureStore(destination, xy, src);
        return;
    }
    let p = vec3<f32>(f32(id.x) + 0.5, f32(id.y) + 0.5, 1.0);
    let ix = i32(floor(dot(params.cell_x.xyz, p)));
    let iy = i32(floor(dot(params.cell_y.xyz, p)));
    let h = grain_hash(ix, iy, params.tail.x);
    if unit(h, 0u) >= params.settings.x {
        textureStore(destination, xy, src);
        return;
    }
    var tint = params.primary;
    if params.tail.y == 1u && (h.hi & 0x80000000u) != 0u {
        tint = params.secondary;
    }
    if params.tail.y == 2u {
        tint = vec4<f32>(unit(h, 16u), unit(h, 32u), unit(h, 48u), params.settings.y);
    }
    let alpha = clamp(tint.a * src.a, 0.0, 1.0);
    let grain = vec4<f32>(tint.rgb * alpha, alpha);
    textureStore(destination, xy, grain + src * (1.0 - alpha));
}
"#;

pub struct GpuNoise {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuNoise {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard GPU grain"),
            source: wgpu::ShaderSource::Wgsl(format!("{GRAIN_HASH_WGSL}\n{SHADER}").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard GPU grain bindings"),
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
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lizard GPU grain pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard GPU grain pass"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    pub fn submit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        noise: &Noise,
        surface_to_local: Affine,
        seed: u32,
    ) -> wgpu::Texture {
        let size = source.size();
        let result = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard GPU grain result"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let [a, b, c, d, e, f] = surface_to_local.as_coeffs();
        let (sx, sy) = noise.grain();
        let primary = noise.color.sanitized();
        let secondary = noise.secondary.sanitized();
        let mut params = Vec::with_capacity(96);
        for value in [a / (sx * 2.0), c / (sx * 2.0), e / (sx * 2.0), 0.0, b / (sy * 2.0), d / (sy * 2.0), f / (sy * 2.0), 0.0] {
            params.extend_from_slice(&(value as f32).to_le_bytes());
        }
        for value in [primary.r, primary.g, primary.b, primary.a, secondary.r, secondary.g, secondary.b, secondary.a] {
            params.extend_from_slice(&value.to_le_bytes());
        }
        for value in [noise.density.clamp(0.0, 1.0) as f32, noise.opacity.clamp(0.0, 1.0), 0.0, 0.0] {
            params.extend_from_slice(&value.to_le_bytes());
        }
        let mode = match noise.mode {
            NoiseMode::Mono => 0,
            NoiseMode::Duo => 1,
            NoiseMode::Multi => 2,
        };
        for value in [seed, mode, size.width, size.height] {
            params.extend_from_slice(&value.to_le_bytes());
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard GPU grain params"),
            contents: &params,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard GPU grain pass"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&result.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU grain compute") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard GPU grain pass"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
        result
    }
}
