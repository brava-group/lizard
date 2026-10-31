//! Progressive layer blur: blur levels and per-pixel interpolation stay on the GPU.

use lizard_doc::Blur;
use lizard_geom::Point;
use wgpu::util::DeviceExt;

use crate::gpu_blur::GpuBlur;

const MAX_LEVELS: usize = 8;
const SHADER: &str = r#"
struct Params {
    size: vec4<u32>,
    levels0: vec4<f32>,
    levels1: vec4<f32>,
    line: vec4<f32>,
    endpoints: vec4<f32>,
};
@group(0) @binding(0) var t0: texture_2d<f32>;
@group(0) @binding(1) var t1: texture_2d<f32>;
@group(0) @binding(2) var t2: texture_2d<f32>;
@group(0) @binding(3) var t3: texture_2d<f32>;
@group(0) @binding(4) var t4: texture_2d<f32>;
@group(0) @binding(5) var t5: texture_2d<f32>;
@group(0) @binding(6) var t6: texture_2d<f32>;
@group(0) @binding(7) var t7: texture_2d<f32>;
@group(0) @binding(8) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(9) var<uniform> params: Params;

fn level(i: u32) -> f32 {
    if i < 4u { return params.levels0[i]; }
    return params.levels1[i - 4u];
}
fn pixel(i: u32, xy: vec2<i32>) -> vec4<f32> {
    switch i {
        case 0u: { return textureLoad(t0, xy, 0); }
        case 1u: { return textureLoad(t1, xy, 0); }
        case 2u: { return textureLoad(t2, xy, 0); }
        case 3u: { return textureLoad(t3, xy, 0); }
        case 4u: { return textureLoad(t4, xy, 0); }
        case 5u: { return textureLoad(t5, xy, 0); }
        case 6u: { return textureLoad(t6, xy, 0); }
        default: { return textureLoad(t7, xy, 0); }
    }
}
@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.size.x || id.y >= params.size.y { return; }
    let xy = vec2<i32>(id.xy);
    let direction = params.line.zw - params.line.xy;
    let length2 = dot(direction, direction);
    var fraction = 1.0;
    if length2 > 0.000000000001 {
        fraction = clamp(dot(vec2<f32>(id.xy) + vec2<f32>(0.5) - params.line.xy, direction) / length2, 0.0, 1.0);
    }
    let sigma = mix(params.endpoints.x, params.endpoints.y, fraction);
    let count = params.size.z;
    var below = 0u;
    for (var i = 1u; i + 1u < count; i += 1u) {
        if sigma >= level(i) { below = i; }
    }
    if count == 1u { textureStore(output, xy, pixel(0u, xy)); return; }
    let low = level(below);
    let high = level(below + 1u);
    let weight = select(0.0, clamp((sigma - low) / (high - low), 0.0, 1.0), high > low);
    // The reference renderer quantizes the interpolation weight to eight bits.
    let quantized = round(weight * 255.0) / 255.0;
    textureStore(output, xy, mix(pixel(below, xy), pixel(below + 1u, xy), quantized));
}
"#;

pub struct GpuProgressiveBlur {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuProgressiveBlur {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard progressive blur mix shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let mut entries = Vec::with_capacity(10);
        for binding in 0..8 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 9,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        });
        let layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Lizard progressive blur bindings"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lizard progressive blur pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard progressive blur mix"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    fn levels(low: f64, high: f64) -> Vec<f64> {
        let mut levels = Vec::with_capacity(MAX_LEVELS);
        let base = low.max(1.0);
        if low < base {
            levels.push(low);
        }
        if high <= base {
            if high > low || levels.is_empty() {
                levels.push(high);
            }
            return levels;
        }
        let steps = ((high / base).ln() / 1.5_f64.ln()).ceil().clamp(1.0, (MAX_LEVELS - 2) as f64) as usize;
        levels.extend((0..steps).map(|k| base * (high / base).powf(k as f64 / steps as f64)));
        levels.push(high);
        levels
    }

    pub fn submit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur_pass: &GpuBlur,
        source: &wgpu::Texture,
        blur: &Blur,
        pixels_per_unit: f64,
        start: Point,
        end: Point,
    ) -> wgpu::Texture {
        let b = blur.sanitized();
        let s0 = b.start_radius * pixels_per_unit / 2.0;
        let s1 = b.radius * pixels_per_unit / 2.0;
        let levels = Self::levels(s0.min(s1), s0.max(s1));
        let textures: Vec<_> = levels
            .iter()
            .map(|sigma| if *sigma < 0.05 { source.clone() } else { blur_pass.submit(device, queue, source, *sigma as f32, 1, None) })
            .collect();
        let size = source.size();
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard progressive blur output"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let fallback = textures.first().unwrap_or(source);
        let views: Vec<_> =
            (0..MAX_LEVELS).map(|i| textures.get(i).unwrap_or(fallback).create_view(&wgpu::TextureViewDescriptor::default())).collect();
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut bytes = Vec::with_capacity(80);
        for value in [size.width, size.height, levels.len() as u32, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for i in 0..MAX_LEVELS {
            bytes.extend_from_slice(&(levels.get(i).copied().unwrap_or(0.0) as f32).to_le_bytes());
        }
        for value in [start.x, start.y, end.x, end.y, s0, s1, 0.0, 0.0] {
            bytes.extend_from_slice(&(value as f32).to_le_bytes());
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard progressive blur params"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut entries: Vec<_> = views
            .iter()
            .enumerate()
            .map(|(i, view)| wgpu::BindGroupEntry { binding: i as u32, resource: wgpu::BindingResource::TextureView(view) })
            .collect();
        entries.push(wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&output_view) });
        entries.push(wgpu::BindGroupEntry { binding: 9, resource: uniform.as_entire_binding() });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard progressive blur mix"),
            layout: &self.layout,
            entries: &entries,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard progressive blur encoder") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard progressive blur mix pass"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
        output
    }
}
