//! Separable Gaussian blur on RGBA textures. The source and result stay on the GPU.

use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Params {
    width: u32,
    height: u32,
    radius: u32,
    horizontal: u32,
    sigma: f32,
    factor: u32,
    clip_left: u32,
    clip_top: u32,
    clip_right: u32,
    clip_bottom: u32,
    clip_enabled: u32,
    pad: u32,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var destination: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

fn read_source(xy: vec2<i32>) -> vec4<f32> {
    if params.clip_enabled != 0u {
        if xy.x < i32(params.clip_left) || xy.y < i32(params.clip_top) || xy.x >= i32(params.clip_right) || xy.y >= i32(params.clip_bottom) {
            return vec4<f32>(0.0);
        }
    }
    return textureLoad(source, xy, 0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.width || id.y >= params.height { return; }
    let xy = vec2<i32>(id.xy);
    var total = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var i = -i32(params.radius); i <= i32(params.radius); i += 1) {
        let delta = select(vec2<i32>(0, i), vec2<i32>(i, 0), params.horizontal != 0u);
        let sample_xy = clamp(xy + delta, vec2<i32>(0), vec2<i32>(i32(params.width) - 1, i32(params.height) - 1));
        let x = f32(i) / params.sigma;
        let weight = exp(-0.5 * x * x);
        total += read_source(sample_xy) * weight;
        weight_sum += weight;
    }
    textureStore(destination, xy, total / weight_sum);
}

@compute @workgroup_size(16, 16)
fn box_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.width || id.y >= params.height { return; }
    let xy = vec2<i32>(id.xy);
    var total = vec4<u32>(0u);
    for (var i = -i32(params.radius); i <= i32(params.radius); i += 1) {
        let delta = select(vec2<i32>(0, i), vec2<i32>(i, 0), params.horizontal != 0u);
        let sample_xy = xy + delta;
        if sample_xy.x >= 0 && sample_xy.y >= 0 && sample_xy.x < i32(params.width) && sample_xy.y < i32(params.height) {
            total += vec4<u32>(round(read_source(sample_xy) * 255.0));
        }
    }
    let divisor = 2u * params.radius + 1u;
    textureStore(destination, xy, vec4<f32>(total / divisor) / 255.0);
}

@compute @workgroup_size(16, 16)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.width || id.y >= params.height { return; }
    let original = textureDimensions(source);
    var sum = vec4<f32>(0.0);
    var count = 0.0;
    for (var y = 0u; y < params.factor; y += 1u) {
        for (var x = 0u; x < params.factor; x += 1u) {
            let sample_xy = id.xy * params.factor + vec2<u32>(x, y);
            if sample_xy.x < original.x && sample_xy.y < original.y {
                sum += read_source(vec2<i32>(sample_xy));
                count += 1.0;
            }
        }
    }
    textureStore(destination, vec2<i32>(id.xy), sum / count);
}

@compute @workgroup_size(16, 16)
fn upsample(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.width || id.y >= params.height { return; }
    let small = vec2<i32>(textureDimensions(source));
    let full = vec2<f32>(f32(params.width), f32(params.height));
    let position = (vec2<f32>(id.xy) + vec2<f32>(0.5)) * vec2<f32>(small) / full - vec2<f32>(0.5);
    let lower = vec2<i32>(floor(position));
    let fraction = fract(position);
    let a = textureLoad(source, clamp(lower, vec2<i32>(0), small - vec2<i32>(1)), 0);
    let b = textureLoad(source, clamp(lower + vec2<i32>(1, 0), vec2<i32>(0), small - vec2<i32>(1)), 0);
    let c = textureLoad(source, clamp(lower + vec2<i32>(0, 1), vec2<i32>(0), small - vec2<i32>(1)), 0);
    let d = textureLoad(source, clamp(lower + vec2<i32>(1, 1), vec2<i32>(0), small - vec2<i32>(1)), 0);
    textureStore(destination, vec2<i32>(id.xy), mix(mix(a, b, fraction.x), mix(c, d, fraction.x), fraction.y));
}
"#;

pub struct GpuBlur {
    pipeline: wgpu::ComputePipeline,
    box_pipeline: wgpu::ComputePipeline,
    downsample_pipeline: wgpu::ComputePipeline,
    upsample_pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuBlur {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard blur shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard blur bindings"),
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
            label: Some("Lizard blur pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard separable Gaussian blur"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let box_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard separable three box blur"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("box_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let downsample_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard blur downsample"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("downsample"),
            compilation_options: Default::default(),
            cache: None,
        });
        let upsample_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard blur upsample"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("upsample"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, box_pipeline, downsample_pipeline, upsample_pipeline, layout }
    }

    fn box_radii(sigma: f32) -> [u32; 3] {
        let sigma = f64::from(sigma);
        let ideal = (4.0 * sigma * sigma + 1.0).sqrt();
        let mut lower = ideal.floor() as i64;
        if lower % 2 == 0 {
            lower -= 1;
        }
        let upper = lower + 2;
        let count = ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f64 - 12.0 * lower as f64 - 9.0) / (-4.0 * lower as f64 - 4.0)).round() as i64;
        std::array::from_fn(|index| {
            let width = if (index as i64) < count { lower } else { upper };
            ((width.max(1) - 1) / 2) as u32
        })
    }

    pub fn submit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        sigma: f32,
        scale: u32,
        clip: Option<[u32; 4]>,
    ) -> wgpu::Texture {
        let scale = scale.max(1);
        let source_size = source.size();
        let size = wgpu::Extent3d { width: source_size.width.div_ceil(scale), height: source_size.height.div_ceil(scale), depth_or_array_layers: 1 };
        let texture = |label| {
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
        };
        let smaller_source = (scale > 1).then(|| texture("Lizard blur downsampled source"));
        let scratch = texture("Lizard blur horizontal");
        let result = texture("Lizard blur vertical");
        let small_sigma = (sigma / scale as f32).max(0.1);
        let radius = (small_sigma * 3.0).ceil().min(256.0) as u32;
        let bind = |input: &wgpu::Texture,
                    output: &wgpu::Texture,
                    pass_radius: u32,
                    horizontal: u32,
                    pass_sigma: f32,
                    factor: u32,
                    pass_clip: Option<[u32; 4]>| {
            let mut params = Vec::with_capacity(48);
            for value in [size.width, size.height, pass_radius, horizontal] {
                params.extend_from_slice(&value.to_le_bytes());
            }
            params.extend_from_slice(&pass_sigma.to_le_bytes());
            params.extend_from_slice(&factor.to_le_bytes());
            let bounds = pass_clip.unwrap_or([0; 4]);
            for value in bounds {
                params.extend_from_slice(&value.to_le_bytes());
            }
            params.extend_from_slice(&u32::from(pass_clip.is_some()).to_le_bytes());
            params.resize(48, 0);
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Lizard blur params"),
                contents: &params,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lizard blur pass"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&input.create_view(&wgpu::TextureViewDescriptor::default())),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&output.create_view(&wgpu::TextureViewDescriptor::default())),
                    },
                    wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                ],
            });
            (uniform, group)
        };
        let downsample = smaller_source.as_ref().map(|small| bind(source, small, 0, 0, 1.0, scale, clip));
        let first_input = smaller_source.as_ref().unwrap_or(source);
        let box_kernel = std::env::var("LIZARD_GPU_BLUR_KERNEL").is_ok_and(|value| value == "box");
        let groups = if box_kernel {
            let radii = Self::box_radii(small_sigma);
            (0..6)
                .map(|pass| {
                    let input = if pass == 0 {
                        first_input
                    } else if pass % 2 == 0 {
                        &result
                    } else {
                        &scratch
                    };
                    let output = if pass % 2 == 0 { &scratch } else { &result };
                    bind(input, output, radii[pass % 3], u32::from(pass < 3), small_sigma, 1, if pass == 0 && scale == 1 { clip } else { None })
                })
                .collect::<Vec<_>>()
        } else {
            vec![
                bind(first_input, &scratch, radius, 1, small_sigma, 1, if scale == 1 { clip } else { None }),
                bind(&scratch, &result, radius, 0, small_sigma, 1, None),
            ]
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard blur compute") });
        if let Some((_, group)) = &downsample {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard downsample pass"), timestamp_writes: None });
            pass.set_pipeline(&self.downsample_pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        for (_, group) in &groups {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard blur pass"), timestamp_writes: None });
            pass.set_pipeline(if box_kernel { &self.box_pipeline } else { &self.pipeline });
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
        result
    }

    /// Blur at a smaller resolution, then reconstruct a native-size result for compositing.
    /// Large smooth kernels retain their shape while avoiding a full-resolution sample loop.
    pub fn submit_upsampled(&self, device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture, sigma: f32, scale: u32) -> wgpu::Texture {
        let scale = scale.max(1);
        if scale == 1 {
            return self.submit(device, queue, source, sigma, 1, None);
        }
        let small = self.submit(device, queue, source, sigma, scale, None);
        let size = source.size();
        let result = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard blur native-resolution result"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut params = Vec::with_capacity(48);
        for value in [size.width, size.height, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] {
            params.extend_from_slice(&value.to_le_bytes());
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard blur upsample params"),
            contents: &params,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard blur upsample bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&small.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&result.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard blur upsample encoder") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard blur upsample pass"), timestamp_writes: None });
            pass.set_pipeline(&self.upsample_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
        result
    }

    pub fn submit_frost(&self, device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture, sigma: f32) -> wgpu::Texture {
        let scale = if sigma >= 32.0 {
            8
        } else if sigma >= 12.0 {
            4
        } else if sigma >= 6.0 {
            2
        } else {
            1
        };
        self.submit_upsampled(device, queue, source, sigma, scale)
    }
}
