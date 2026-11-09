//! Backdrop refraction using GPU distance fields and compositing passes.

use lizard_doc::Glass;
use wgpu::util::DeviceExt;

use crate::gpu_blur::GpuBlur;

const SHADER: &str = r#"
@group(0) @binding(0) var init_mask: texture_2d<f32>;
@group(0) @binding(1) var init_out: texture_storage_2d<rgba16sint, write>;
@compute @workgroup_size(16, 16)
fn initialize(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(init_mask);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let alpha = textureLoad(init_mask, xy, 0).a;
    textureStore(init_out, xy, select(vec4<i32>(xy, 0, 0), vec4<i32>(-1, -1, 0, 0), alpha >= 0.5));
}

struct Step { value: vec4<u32> };
@group(0) @binding(2) var step_input: texture_2d<i32>;
@group(0) @binding(3) var step_out: texture_storage_2d<rgba16sint, write>;
@group(0) @binding(4) var<uniform> step: Step;
@compute @workgroup_size(16, 16)
fn propagate(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(step_input);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    var best = textureLoad(step_input, xy, 0).xy;
    var best_sq = 1e30;
    if best.x >= 0 {
        let delta = vec2<f32>(xy - best);
        best_sq = dot(delta, delta);
    }
    for (var yy = -1; yy <= 1; yy += 1) {
        for (var xx = -1; xx <= 1; xx += 1) {
            let neighbor = xy + vec2<i32>(xx, yy) * i32(step.value.x);
            if neighbor.x < 0 || neighbor.y < 0 || neighbor.x >= i32(size.x) || neighbor.y >= i32(size.y) { continue; }
            let seed = textureLoad(step_input, neighbor, 0).xy;
            if seed.x < 0 { continue; }
            let delta = vec2<f32>(xy - seed);
            let dist_sq = dot(delta, delta);
            if dist_sq < best_sq { best_sq = dist_sq; best = seed; }
        }
    }
    textureStore(step_out, xy, vec4<i32>(best, 0, 0));
}

@group(0) @binding(5) var distance_seed: texture_2d<i32>;
@group(0) @binding(6) var distance_mask: texture_2d<f32>;
@group(0) @binding(7) var distance_out: texture_storage_2d<r32float, write>;
@compute @workgroup_size(16, 16)
fn distances(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(distance_seed);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let seed = textureLoad(distance_seed, xy, 0).xy;
    let alpha = textureLoad(distance_mask, xy, 0).a;
    var d = 0.0;
    if alpha >= 0.5 {
        d = select(1e9, max(length(vec2<f32>(xy - seed)) - 0.5, 0.0), seed.x >= 0);
    }
    textureStore(distance_out, xy, vec4<f32>(d, 0.0, 0.0, 0.0));
}

@group(0) @binding(8) var normal_distance: texture_2d<f32>;
@group(0) @binding(9) var normal_out: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(16, 16)
fn normals(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(normal_distance);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let lo = max(xy - vec2<i32>(1), vec2<i32>(0));
    let hi = min(xy + vec2<i32>(1), vec2<i32>(size) - vec2<i32>(1));
    let gx = (textureLoad(normal_distance, vec2<i32>(hi.x, xy.y), 0).r - textureLoad(normal_distance, vec2<i32>(lo.x, xy.y), 0).r) * 0.5;
    let gy = (textureLoad(normal_distance, vec2<i32>(xy.x, hi.y), 0).r - textureLoad(normal_distance, vec2<i32>(xy.x, lo.y), 0).r) * 0.5;
    textureStore(normal_out, xy, vec4<f32>(clamp(vec2<f32>(gx, gy) * 0.25 + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0)), 0.0, 1.0));
}

struct RoundedRectParams {
    rect: vec4<f32>,
    shape: vec4<f32>,
};
@group(0) @binding(16) var<uniform> rounded: RoundedRectParams;
@group(0) @binding(17) var rounded_distance: texture_storage_2d<r32float, write>;
@group(0) @binding(18) var rounded_normals: texture_storage_2d<rgba8unorm, write>;
fn rounded_rect_data(point: vec2<f32>) -> vec3<f32> {
    let center = (rounded.rect.xy + rounded.rect.zw) * 0.5;
    let half = max((rounded.rect.zw - rounded.rect.xy) * 0.5, vec2<f32>(0.001));
    let radius = clamp(rounded.shape.x, 0.0, min(half.x, half.y));
    let q = abs(point - center) - (half - vec2<f32>(radius));
    let corner = max(q, vec2<f32>(0.0));
    let signed_distance = length(corner) + min(max(q.x, q.y), 0.0) - radius;
    let distance = -signed_distance;
    var inward = vec2<f32>(0.0);
    if length(corner) > 0.0001 {
        inward = -normalize(sign(point - center) * corner);
    } else if q.x > q.y {
        inward.x = -sign(point.x - center.x);
    } else {
        inward.y = -sign(point.y - center.y);
    }
    return vec3<f32>(distance, inward);
}
@compute @workgroup_size(16, 16)
fn rounded_rect_geometry(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(rounded_distance);
    if id.x >= size.x || id.y >= size.y { return; }
    let data = rounded_rect_data(vec2<f32>(id.xy) + vec2<f32>(0.5));
    textureStore(rounded_distance, vec2<i32>(id.xy), vec4<f32>(data.x, 0.0, 0.0, 0.0));
    textureStore(rounded_normals, vec2<i32>(id.xy), vec4<f32>(data.yz * 0.5 + vec2<f32>(0.5), 0.0, 1.0));
}

struct GlassParams {
    dimensions: vec4<u32>,
    lens: vec4<f32>,
    light: vec4<f32>,
    falloff: vec4<f32>,
};
@group(0) @binding(10) var glass_backdrop: texture_2d<f32>;
@group(0) @binding(11) var glass_mask: texture_2d<f32>;
@group(0) @binding(12) var glass_distance: texture_2d<f32>;
@group(0) @binding(13) var glass_normals: texture_2d<f32>;
@group(0) @binding(14) var glass_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(15) var<uniform> glass: GlassParams;
@group(0) @binding(19) var backdrop_sampler: sampler;

fn sample_backdrop(at: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(glass.dimensions.xy);
    // Preserve a transparent edge while the sampler performs bilinear filtering.
    let coverage = clamp(at + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0))
        * clamp(size + vec2<f32>(0.5) - at, vec2<f32>(0.0), vec2<f32>(1.0));
    return textureSampleLevel(glass_backdrop, backdrop_sampler, at / size, 0.0) * coverage.x * coverage.y;
}

fn shade_glass(point: vec2<f32>, d: f32, coverage: f32, inward: vec2<f32>) -> vec4<f32> {
    if coverage <= 0.0 { return vec4<f32>(0.0); }
    let bend = glass.lens.y * pow(1.0 - min(d / glass.lens.x, 1.0), 2.0);
    var base: vec4<f32>;
    if bend > 0.0 {
        if glass.lens.z <= 0.0 {
            let sampled = sample_backdrop(point + inward * bend);
            base = vec4<f32>(min(sampled.rgb, vec3<f32>(sampled.a)), sampled.a);
        } else {
            let red = sample_backdrop(point + inward * bend * (1.0 - glass.lens.z));
            let green = sample_backdrop(point + inward * bend);
            let blue = sample_backdrop(point + inward * bend * (1.0 + glass.lens.z));
            let alpha = max(red.a, max(green.a, blue.a));
            base = vec4<f32>(min(red.r, alpha), min(green.g, alpha), min(blue.b, alpha), alpha);
        }
    } else {
        base = sample_backdrop(point);
    }
    var highlight = 0.0;
    if d < glass.falloff.x && glass.falloff.z > 0.0 {
        let facing = -dot(inward, glass.light.xy);
        let spec = pow(max(facing, 0.0), glass.light.w) + 0.5 * pow(max(-facing, 0.0), glass.light.w);
        highlight = clamp(glass.light.z * spec * (exp(-d / glass.lens.w) - glass.falloff.y) / glass.falloff.z, 0.0, 1.0);
    }
    return mix(base, vec4<f32>(1.0), highlight) * coverage;
}

@compute @workgroup_size(16, 16)
fn composite(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= glass.dimensions.x || id.y >= glass.dimensions.y { return; }
    let xy = vec2<i32>(id.xy);
    let d = textureLoad(glass_distance, xy, 0).r;
    let coverage = select(textureLoad(glass_mask, xy, 0).a, clamp(d + 0.5, 0.0, 1.0), glass.dimensions.z != 0u);
    let n = textureLoad(glass_normals, xy, 0).xy * 2.0 - vec2<f32>(1.0);
    var inward = vec2<f32>(0.0);
    if dot(n, n) > 0.00001 { inward = normalize(n); }
    textureStore(glass_out, xy, shade_glass(vec2<f32>(id.xy) + vec2<f32>(0.5), d, coverage, inward));
}

@compute @workgroup_size(16, 16)
fn composite_rounded(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= glass.dimensions.x || id.y >= glass.dimensions.y { return; }
    let point = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let data = rounded_rect_data(point);
    let coverage = clamp(data.x + 0.5, 0.0, 1.0);
    var inward = vec2<f32>(0.0);
    if dot(data.yz, data.yz) > 0.00001 { inward = normalize(data.yz); }
    textureStore(glass_out, vec2<i32>(id.xy), shade_glass(point, data.x, coverage, inward));
}
"#;

pub struct GpuGlass {
    initialize: wgpu::ComputePipeline,
    propagate: wgpu::ComputePipeline,
    distances: wgpu::ComputePipeline,
    normals: wgpu::ComputePipeline,
    rounded_rect: wgpu::ComputePipeline,
    composite: wgpu::ComputePipeline,
    composite_rounded: wgpu::ComputePipeline,
    layouts: Vec<wgpu::BindGroupLayout>,
    backdrop_sampler: wgpu::Sampler,
}

impl GpuGlass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard glass shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let storage = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let f = wgpu::TextureSampleType::Float { filterable: false };
        let filterable = wgpu::TextureSampleType::Float { filterable: true };
        let sampler = wgpu::BindGroupLayoutEntry {
            binding: 19,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let i = wgpu::TextureSampleType::Sint;
        let specs = [
            vec![texture(0, f), storage(1, wgpu::TextureFormat::Rgba16Sint)],
            vec![texture(2, i), storage(3, wgpu::TextureFormat::Rgba16Sint), uniform(4)],
            vec![texture(5, i), texture(6, f), storage(7, wgpu::TextureFormat::R32Float)],
            vec![texture(8, f), storage(9, wgpu::TextureFormat::Rgba8Unorm)],
            vec![
                texture(10, filterable),
                texture(11, f),
                texture(12, f),
                texture(13, f),
                storage(14, wgpu::TextureFormat::Rgba8Unorm),
                uniform(15),
                sampler,
            ],
            vec![uniform(16), storage(17, wgpu::TextureFormat::R32Float), storage(18, wgpu::TextureFormat::Rgba8Unorm)],
            vec![texture(10, filterable), storage(14, wgpu::TextureFormat::Rgba8Unorm), uniform(15), uniform(16), sampler],
        ];
        let layouts: Vec<_> = specs
            .iter()
            .enumerate()
            .map(|(index, entries)| {
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(match index {
                        0 => "Lizard glass seed layout",
                        1 => "Lizard glass flood layout",
                        2 => "Lizard glass distance layout",
                        3 => "Lizard glass normal layout",
                        _ => "Lizard glass composite layout",
                    }),
                    entries,
                })
            })
            .collect();
        let pipeline = |index: usize, entry_point: &str| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Lizard glass pipeline layout"),
                bind_group_layouts: &[Some(&layouts[index])],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Lizard glass compute"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            initialize: pipeline(0, "initialize"),
            propagate: pipeline(1, "propagate"),
            distances: pipeline(2, "distances"),
            normals: pipeline(3, "normals"),
            rounded_rect: pipeline(5, "rounded_rect_geometry"),
            composite: pipeline(4, "composite"),
            composite_rounded: pipeline(6, "composite_rounded"),
            layouts,
            backdrop_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Lizard glass backdrop linear sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    pub fn submit(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur: &GpuBlur,
        backdrop: &wgpu::Texture,
        mask: &wgpu::Texture,
        effect: &Glass,
        scale: f64,
    ) -> wgpu::Texture {
        self.submit_with_shape(device, queue, blur, backdrop, mask, effect, scale, None)
    }

    pub fn submit_rounded_rect(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur: &GpuBlur,
        backdrop: &wgpu::Texture,
        effect: &Glass,
        scale: f64,
        rect: [f32; 4],
        radius: f32,
    ) -> wgpu::Texture {
        self.submit_with_shape(device, queue, blur, backdrop, backdrop, effect, scale, Some((rect, radius)))
    }

    fn submit_with_shape(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blur: &GpuBlur,
        backdrop: &wgpu::Texture,
        mask: &wgpu::Texture,
        effect: &Glass,
        scale: f64,
        rounded_rect: Option<([f32; 4], f32)>,
    ) -> wgpu::Texture {
        let g = effect.sanitized();
        let size = backdrop.size();
        let make = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let result = make("Lizard glass output", wgpu::TextureFormat::Rgba8Unorm);
        let frost = if g.frost > 0.0 { blur.submit_frost(device, queue, backdrop, (g.frost * scale / 2.0) as f32) } else { backdrop.clone() };
        let depth = (g.depth * scale).clamp(1.0, 1024.0) as f32;
        let smooth_radius = (depth * 0.1).clamp(1.0, 8.0);
        let make_group = |index: usize, textures: &[(&wgpu::Texture, u32)], uniform: Option<(&wgpu::Buffer, u32)>| {
            let views: Vec<_> = textures.iter().map(|(texture, _)| texture.create_view(&wgpu::TextureViewDescriptor::default())).collect();
            let mut entries: Vec<_> = views
                .iter()
                .zip(textures)
                .map(|(view, (_, binding))| wgpu::BindGroupEntry { binding: *binding, resource: wgpu::BindingResource::TextureView(view) })
                .collect();
            if let Some((buffer, binding)) = uniform {
                entries.push(wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() });
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lizard glass pass bindings"),
                layout: &self.layouts[index],
                entries: &entries,
            })
        };
        let mut params = Vec::with_capacity(64);
        for v in [size.width, size.height, u32::from(rounded_rect.is_some()), 0] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        let band = (depth * (0.15 + 0.35 * g.splay as f32)).max(1.0);
        let max_bend = g.refraction as f32 * depth;
        for v in [depth, max_bend, g.dispersion as f32 * 0.25, band] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        let angle = g.light_angle.to_radians();
        for v in [angle.sin() as f32, -angle.cos() as f32, g.light_intensity as f32, (6.0 - 5.0 * g.splay) as f32] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        let k = 255.0 * g.light_intensity / 0.5;
        let reach = if k > 1.0 { (f64::from(band) * k.ln()).min(512.0) as f32 } else { 0.0 };
        let floor = (-reach / band).exp();
        for v in [reach, floor, 1.0 - floor, 0.0] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        let composite_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard glass params"),
            contents: &params,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let dispatch = |encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::ComputePipeline, group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard glass pass"), timestamp_writes: None });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        };
        if let Some((rect, radius)) = rounded_rect {
            let bytes: Vec<u8> = rect.into_iter().chain([radius, 0.0, 0.0, 0.0]).flat_map(f32::to_le_bytes).collect();
            let rounded_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Lizard glass rounded rectangle"),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let backdrop_view = frost.create_view(&wgpu::TextureViewDescriptor::default());
            let result_view = result.create_view(&wgpu::TextureViewDescriptor::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lizard fused rounded glass bindings"),
                layout: &self.layouts[6],
                entries: &[
                    wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(&backdrop_view) },
                    wgpu::BindGroupEntry { binding: 14, resource: wgpu::BindingResource::TextureView(&result_view) },
                    wgpu::BindGroupEntry { binding: 15, resource: composite_uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 16, resource: rounded_uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 19, resource: wgpu::BindingResource::Sampler(&self.backdrop_sampler) },
                ],
            });
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard fused rounded glass encoder") });
            dispatch(&mut encoder, &self.composite_rounded, &group);
            queue.submit([encoder.finish()]);
            return result;
        }
        let field = make("Lizard glass distance field", wgpu::TextureFormat::R32Float);
        let gradients = make("Lizard glass gradients", wgpu::TextureFormat::Rgba8Unorm);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard glass geometry encoder") });
        if let Some((rect, radius)) = rounded_rect {
            let bytes: Vec<u8> = rect.into_iter().chain([radius, 0.0, 0.0, 0.0]).flat_map(f32::to_le_bytes).collect();
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Lizard glass rounded rectangle"),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let group = make_group(5, &[(&field, 17), (&gradients, 18)], Some((&uniform, 16)));
            dispatch(&mut encoder, &self.rounded_rect, &group);
        } else {
            // Signed 16-bit coordinates cover every supported 2D texture dimension.
            let seed_a = make("Lizard glass seeds A", wgpu::TextureFormat::Rgba16Sint);
            let seed_b = make("Lizard glass seeds B", wgpu::TextureFormat::Rgba16Sint);
            let group_init = make_group(0, &[(mask, 0), (&seed_a, 1)], None);
            let group_distance_a = make_group(2, &[(&seed_a, 5), (mask, 6), (&field, 7)], None);
            let group_distance_b = make_group(2, &[(&seed_b, 5), (mask, 6), (&field, 7)], None);
            let group_normals = make_group(3, &[(&field, 8), (&gradients, 9)], None);
            dispatch(&mut encoder, &self.initialize, &group_init);
            let mut current_a = true;
            let mut stride = size.width.max(size.height).next_power_of_two() / 2;
            while stride > 0 {
                let bytes: Vec<u8> = [stride, 0, 0, 0].into_iter().flat_map(u32::to_le_bytes).collect();
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Lizard glass flood step"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let (from, to) = if current_a { (&seed_a, &seed_b) } else { (&seed_b, &seed_a) };
                let group = make_group(1, &[(from, 2), (to, 3)], Some((&buffer, 4)));
                dispatch(&mut encoder, &self.propagate, &group);
                current_a = !current_a;
                stride /= 2;
            }
            dispatch(&mut encoder, &self.distances, if current_a { &group_distance_a } else { &group_distance_b });
            dispatch(&mut encoder, &self.normals, &group_normals);
        }
        queue.submit([encoder.finish()]);
        let smoothed = if rounded_rect.is_some() { gradients } else { blur.submit(device, queue, &gradients, smooth_radius, 1, None) };
        let group_composite = {
            let views = [
                frost.create_view(&wgpu::TextureViewDescriptor::default()),
                mask.create_view(&wgpu::TextureViewDescriptor::default()),
                field.create_view(&wgpu::TextureViewDescriptor::default()),
                smoothed.create_view(&wgpu::TextureViewDescriptor::default()),
                result.create_view(&wgpu::TextureViewDescriptor::default()),
            ];
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lizard glass composite bindings"),
                layout: &self.layouts[4],
                entries: &[
                    wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(&views[0]) },
                    wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::TextureView(&views[1]) },
                    wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::TextureView(&views[2]) },
                    wgpu::BindGroupEntry { binding: 13, resource: wgpu::BindingResource::TextureView(&views[3]) },
                    wgpu::BindGroupEntry { binding: 14, resource: wgpu::BindingResource::TextureView(&views[4]) },
                    wgpu::BindGroupEntry { binding: 15, resource: composite_uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 19, resource: wgpu::BindingResource::Sampler(&self.backdrop_sampler) },
                ],
            })
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard glass composite encoder") });
        dispatch(&mut encoder, &self.composite, &group_composite);
        queue.submit([encoder.finish()]);
        result
    }
}
