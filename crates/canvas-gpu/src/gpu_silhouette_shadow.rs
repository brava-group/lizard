//! Inner and outer shadows of a silhouette, with masks and blur passes on the GPU.

use figcraft_color::Color;
use wgpu::util::DeviceExt;

use crate::gpu_blur::GpuBlur;

const SHADER: &str = r#"
struct Params {
    offset: vec4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var silhouette: texture_2d<f32>;
@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn invert_offset(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y { return; }
    let shifted = vec2<i32>(id.xy) - vec2<i32>(round(params.offset.xy));
    var coverage = 0.0;
    if shifted.x >= 0 && shifted.y >= 0 && shifted.x < i32(size.x) && shifted.y < i32(size.y) {
        coverage = textureLoad(source, shifted, 0).a;
    }
    let inverse = 1.0 - coverage;
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(inverse));
}

@compute @workgroup_size(16, 16)
fn tint_inside(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let alpha = textureLoad(source, xy, 0).a * textureLoad(silhouette, xy, 0).a * params.color.a;
    textureStore(output, xy, vec4<f32>(params.color.rgb * alpha, alpha));
}

@compute @workgroup_size(16, 16)
fn compose_source(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let shadow = textureLoad(source, xy, 0);
    let base = textureLoad(silhouette, xy, 0);
    textureStore(output, xy, shadow + base * (1.0 - shadow.a));
}

@compute @workgroup_size(16, 16)
fn offset_alpha(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y { return; }
    let shifted = vec2<i32>(id.xy) - vec2<i32>(round(params.offset.xy));
    var coverage = 0.0;
    if shifted.x >= 0 && shifted.y >= 0 && shifted.x < i32(size.x) && shifted.y < i32(size.y) {
        coverage = textureLoad(source, shifted, 0).a;
    }
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(coverage));
}

@compute @workgroup_size(16, 16)
fn compose_drop(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let base = textureLoad(silhouette, xy, 0);
    let mask = textureLoad(source, xy, 0).a;
    let clipped = mask * params.color.a * (1.0 - base.a);
    let shadow = vec4<f32>(params.color.rgb * clipped, clipped);
    textureStore(output, xy, base + shadow * (1.0 - base.a));
}

@compute @workgroup_size(16, 16)
fn compose_drop_behind(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let base = textureLoad(silhouette, xy, 0);
    let alpha = textureLoad(source, xy, 0).a * params.color.a;
    let shadow = vec4<f32>(params.color.rgb * alpha, alpha);
    textureStore(output, xy, base + shadow * (1.0 - base.a));
}
"#;

pub struct GpuSilhouetteShadow {
    invert: wgpu::ComputePipeline,
    tint: wgpu::ComputePipeline,
    compose: wgpu::ComputePipeline,
    drop_offset: wgpu::ComputePipeline,
    drop_compose: wgpu::ComputePipeline,
    drop_behind_compose: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuSilhouetteShadow {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard GPU inner shadow"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let sampled = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard GPU inner shadow bindings"),
            entries: &[
                sampled(0),
                sampled(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lizard GPU inner shadow pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Lizard GPU inner shadow pass"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            invert: pipeline("invert_offset"),
            tint: pipeline("tint_inside"),
            compose: pipeline("compose_source"),
            drop_offset: pipeline("offset_alpha"),
            drop_compose: pipeline("compose_drop"),
            drop_behind_compose: pipeline("compose_drop_behind"),
            layout,
        }
    }

    fn texture(device: &wgpu::Device, size: wgpu::Extent3d, label: &'static str) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &wgpu::ComputePipeline,
        source: &wgpu::Texture,
        silhouette: &wgpu::Texture,
        output: &wgpu::Texture,
        offset: (f32, f32),
        color: Color,
    ) {
        let mut bytes = Vec::with_capacity(32);
        for value in [offset.0, offset.1, 0.0, 0.0, color.r, color.g, color.b, color.a] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard GPU inner shadow parameters"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard GPU inner shadow group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&silhouette.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&output.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 3, resource: params.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU inner shadow compute") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard GPU inner shadow pass"), timestamp_writes: None });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(source.width().div_ceil(16), source.height().div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
    }

    pub fn submit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur: &GpuBlur,
        silhouette: &wgpu::Texture,
        offset: (f32, f32),
        sigma: f32,
        color: Color,
    ) -> wgpu::Texture {
        self.submit_with_caster(device, queue, blur, silhouette, silhouette, offset, sigma, color)
    }

    fn submit_with_caster(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur: &GpuBlur,
        caster: &wgpu::Texture,
        silhouette: &wgpu::Texture,
        offset: (f32, f32),
        sigma: f32,
        color: Color,
    ) -> wgpu::Texture {
        let inverse = Self::texture(device, silhouette.size(), "Lizard GPU inverse silhouette");
        self.run(device, queue, &self.invert, caster, caster, &inverse, offset, color);
        let blurred = blur.submit(device, queue, &inverse, sigma, 1, None);
        let result = Self::texture(device, silhouette.size(), "Lizard GPU inner shadow result");
        self.run(device, queue, &self.tint, &blurred, silhouette, &result, offset, color);
        result
    }

    pub fn submit_composited(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur: &GpuBlur,
        source: &wgpu::Texture,
        caster: &wgpu::Texture,
        offset: (f32, f32),
        sigma: f32,
        color: Color,
    ) -> wgpu::Texture {
        let shadow = self.submit_with_caster(device, queue, blur, caster, source, offset, sigma, color);
        let result = Self::texture(device, source.size(), "Lizard GPU composited inner shadow");
        self.run(device, queue, &self.compose, &shadow, source, &result, offset, color);
        result
    }

    pub fn submit_drop_composited(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur: &GpuBlur,
        source: &wgpu::Texture,
        caster: &wgpu::Texture,
        offset: (f32, f32),
        sigma: f32,
        color: Color,
        show_behind: bool,
    ) -> wgpu::Texture {
        let shifted = Self::texture(device, source.size(), "Lizard GPU offset drop silhouette");
        self.run(device, queue, &self.drop_offset, caster, caster, &shifted, offset, color);
        let blurred = blur.submit(device, queue, &shifted, sigma, 1, None);
        let result = Self::texture(device, source.size(), "Lizard GPU composited drop shadow");
        let pipeline = if show_behind { &self.drop_behind_compose } else { &self.drop_compose };
        self.run(device, queue, pipeline, &blurred, source, &result, offset, color);
        result
    }
}
