//! GPU-generated diamond gradient texture, matching the document gradient handles.

use lizard_color::Gradient;
use wgpu::util::DeviceExt;

const SIZE: u32 = 256;
const SHADER: &str = r#"
struct Params { count: u32, pad0: u32, pad1: u32, pad2: u32 };
struct Stop { position_rgb: vec4<f32>, alpha: vec4<f32> };

@group(0) @binding(0) var destination: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(1) var<storage, read> stops: array<Stop>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= 256u || id.y >= 256u { return; }
    let x = (f32(id.x) + 0.5 - 128.0) / 128.0;
    let y = (f32(id.y) + 0.5 - 128.0) / 128.0;
    let t = clamp(abs(x) + abs(y), 0.0, 1.0);
    var left = 0u;
    var right = params.count;
    // Find the first stop at or beyond t, including duplicate stop positions.
    loop {
        if left >= right { break; }
        let middle = (left + right) / 2u;
        if stops[middle].position_rgb.x < t { left = middle + 1u; }
        else { right = middle; }
    }
    var rgba: vec4<f32>;
    if left == 0u {
        rgba = vec4<f32>(stops[0].position_rgb.yzw, stops[0].alpha.x);
    } else if left >= params.count {
        let last = stops[params.count - 1u];
        rgba = vec4<f32>(last.position_rgb.yzw, last.alpha.x);
    } else {
        let a = stops[left - 1u];
        let b = stops[left];
        let span = b.position_rgb.x - a.position_rgb.x;
        let fraction = select(1.0, clamp((t - a.position_rgb.x) / span, 0.0, 1.0), span > 0.000001);
        rgba = mix(vec4<f32>(a.position_rgb.yzw, a.alpha.x), vec4<f32>(b.position_rgb.yzw, b.alpha.x), fraction);
    }
    textureStore(destination, vec2<i32>(id.xy), vec4<f32>(rgba.rgb * rgba.a, rgba.a));
}
"#;

pub struct GpuDiamond {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuDiamond {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard GPU diamond gradient"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard GPU diamond bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
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
            label: Some("Lizard GPU diamond pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard GPU diamond pass"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    pub fn submit(&self, device: &wgpu::Device, queue: &wgpu::Queue, gradient: &Gradient, opacity: f32) -> wgpu::Texture {
        let sorted = gradient.sorted_stops();
        let mut stop_bytes = Vec::with_capacity(sorted.len() * 32);
        for stop in &sorted {
            for value in [stop.position, stop.color.r, stop.color.g, stop.color.b, stop.color.a * opacity, 0.0, 0.0, 0.0] {
                stop_bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let stop_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard diamond stops"),
            contents: &stop_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut params = [0u8; 16];
        params[..4].copy_from_slice(&(sorted.len() as u32).to_le_bytes());
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard diamond params"),
            contents: &params,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard diamond gradient texture"),
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard diamond group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 1, resource: stop_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: params_buffer.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard diamond gradient") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard diamond gradient pass"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(SIZE / 16, SIZE / 16, 1);
        }
        queue.submit([encoder.finish()]);
        texture
    }
}
