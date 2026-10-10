//! wgpu picture of the plaza. The window is the last field so the surface
//! is dropped before the window.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::geom::{self, Vertex};
use crate::mark::Shape;
use crate::scene::{Frame, TileView, HERE_RING_HEIGHT, HERE_RING_SCALE, HERE_RING_Y};

/// Which pattern is drawn on the plaza floor. `grid` is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Floor {
    Grid,
    Hex,
    Checker,
}

impl Floor {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "grid" => Some(Self::Grid),
            "hex" => Some(Self::Hex),
            "checker" => Some(Self::Checker),
            _ => None,
        }
    }

    pub fn names() -> &'static str {
        "grid, hex, checker"
    }

    fn code(self) -> f32 {
        match self {
            Self::Grid => 0.0,
            Self::Hex => 1.0,
            Self::Checker => 2.0,
        }
    }
}

const SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    extras: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) m3: vec4<f32>,
    @location(6) n0: vec4<f32>,
    @location(7) n1: vec4<f32>,
    @location(8) n2: vec4<f32>,
    @location(9) n3: vec4<f32>,
    @location(10) color: vec4<f32>,
    @location(11) params: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec4<f32>,
};

@vertex
fn vs_mesh(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    let nmat = mat4x4<f32>(in.n0, in.n1, in.n2, in.n3);
    // A command segment is built as 1 radian of arc. params.y is its center
    // angle and params.z is the sweep, so one mesh fits every count.
    var pos = in.position;
    var nrm = in.normal;
    if (in.params.w > 1.5) {
        let rest = atan2(pos.z, pos.x);
        let delta = in.params.y + rest * (in.params.z - 1.0);
        let c = cos(delta);
        let s = sin(delta);
        pos = vec3<f32>(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c);
        nrm = vec3<f32>(nrm.x * c - nrm.z * s, nrm.y, nrm.x * s + nrm.z * c);
    }
    let world = model * vec4<f32>(pos, 1.0);
    var out: VsOut;
    out.clip = u.view_proj * world;
    out.world = world.xyz;
    out.normal = normalize((nmat * vec4<f32>(nrm, 0.0)).xyz);
    out.color = in.color;
    out.params = in.params;
    return out;
}

@fragment
fn fs_mesh(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let light = normalize(vec3<f32>(0.28, 0.9, 0.32));
    let view = normalize(u.camera_pos.xyz - in.world);
    let ndotl = max(dot(n, light), 0.0);
    let hemi = n.y * 0.5 + 0.5;
    var rgb = in.color.rgb * (0.14 + 0.5 * hemi + 0.5 * ndotl);
    let cap = smoothstep(0.6, 0.98, n.y);
    rgb = mix(rgb, in.color.rgb * vec3<f32>(1.32, 1.28, 1.16) + vec3<f32>(0.07, 0.07, 0.05), cap);
    let half_dir = normalize(light + view);
    rgb = rgb + vec3<f32>(pow(max(dot(n, half_dir), 0.0), 48.0) * 0.22);
    let rim = pow(1.0 - max(dot(n, view), 0.0), 2.6);
    rgb = rgb + vec3<f32>(0.55, 0.82, 0.78) * rim * 0.32;
    rgb = rgb + in.color.rgb * in.params.x;
    let dist = distance(in.world, u.camera_pos.xyz);
    rgb = mix(rgb, vec3<f32>(0.04, 0.055, 0.08), smoothstep(22.0, 72.0, dist));
    return vec4<f32>(rgb, 1.0);
}

@fragment
fn fs_blend(in: VsOut) -> @location(0) vec4<f32> {
    if (in.params.w > 0.5) {
        let d = length(in.world.xz - in.params.yz);
        let radius = max(in.params.x, 0.001);
        let alpha = smoothstep(radius, radius * 0.15, d) * 0.38;
        if (alpha < 0.02) {
            discard;
        }
        return vec4<f32>(0.01, 0.015, 0.02, alpha);
    }
    let dist = length(in.world.xz);
    let radius = max(in.params.x, 1.0);
    let fade = 1.0 - smoothstep(radius * 0.35, radius, dist);
    // params.y picks the floor: 0 grid, 1 hex, 2 checker.
    if (in.params.y > 1.5) {
        let tone = checker(in.world.xz, 1.7);
        let rgb = mix(vec3<f32>(0.30, 0.42, 0.46), vec3<f32>(0.36, 0.50, 0.54), tone);
        let alpha = mix(0.12, 0.18, tone) * fade;
        if (alpha < 0.015) {
            discard;
        }
        return vec4<f32>(rgb, alpha);
    }
    var line: f32;
    if (in.params.y > 0.5) {
        line = hex_line(in.world.xz, 0.95);
    } else {
        let fine = grid_line(in.world.xz, 0.85, 0.006);
        let coarse = grid_line(in.world.xz, 4.25, 0.012);
        line = max(fine * 0.65, coarse);
    }
    let alpha = line * 0.7 * fade;
    if (alpha < 0.02) {
        discard;
    }
    return vec4<f32>(in.color.rgb, alpha);
}

// Distance to the nearest grid line, in world units. `width` is the solid core.
fn grid_line(xz: vec2<f32>, cell: f32, width: f32) -> f32 {
    let scaled = xz / cell;
    let along = abs(fract(scaled - 0.5) - 0.5) * cell;
    let fw = max(fwidth(xz), vec2<f32>(0.0005));
    let ax = 1.0 - smoothstep(width, width + fw.x * 1.25, along.x);
    let ay = 1.0 - smoothstep(width, width + fw.y * 1.25, along.y);
    return max(ax, ay);
}

// Fine honeycomb. `size` is the center-to-corner distance of one hex.
fn hex_line(xz: vec2<f32>, size: f32) -> f32 {
    let p = xz / size;
    // Flat-top, so a side faces the camera and the cells still read as hexes.
    let q = (2.0 / 3.0) * p.x;
    let r = (-1.0 / 3.0) * p.x + (1.7320508 / 3.0) * p.y;
    let s = -q - r;
    var rq = round(q);
    var rr = round(r);
    let rs = round(s);
    let dq = abs(rq - q);
    let dr = abs(rr - r);
    let ds = abs(rs - s);
    if (dq > dr && dq > ds) {
        rq = -rr - rs;
    } else if (dr > ds) {
        rr = -rq - rs;
    }
    let lq = q - rq;
    let lr = r - rr;
    let edge = max(max(abs(lq), abs(lr)), abs(-lq - lr));
    let dist = (0.5 - edge) * size * 1.7320508;
    // Use the ground's own pixel size. The hex distance folds at each edge, so its
    // derivative blows up there and would paint the cells solid.
    let px = max(length(fwidth(xz)), 0.0005);
    return 1.0 - smoothstep(0.006, 0.006 + px * 1.25, dist);
}

fn checker(xz: vec2<f32>, cell: f32) -> f32 {
    let id = floor(xz / cell);
    return step(0.25, fract((id.x + id.y) * 0.5));
}

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    var pos = vec2<f32>(-1.0, -1.0);
    if (vi == 1u) { pos = vec2<f32>(3.0, -1.0); }
    if (vi == 2u) { pos = vec2<f32>(-1.0, 3.0); }
    var out: SkyOut;
    out.clip = vec4<f32>(pos, 0.0, 1.0);
    return out;
}

@fragment
fn fs_sky(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(max(u.extras.z, 1.0), max(u.extras.y, 1.0));
    let uv = frag_pos.xy / size;
    let t = clamp(1.0 - uv.y, 0.0, 1.0);
    var col = mix(vec3<f32>(0.018, 0.022, 0.035), vec3<f32>(0.11, 0.15, 0.23), pow(t, 0.85));
    let glow = exp(-abs(uv.y - 0.74) * 8.0);
    col = col + vec3<f32>(0.04, 0.14, 0.16) * glow;
    let vignette = distance(uv, vec2<f32>(0.5, 0.42));
    col = col * mix(1.0, 0.72, smoothstep(0.25, 0.95, vignette));
    return vec4<f32>(col, 1.0);
}

struct PlateIn {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct PlateOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_plate(in: PlateIn, @builtin(vertex_index) vi: u32) -> PlateOut {
    var corner = vec2<f32>(0.0, 0.0);
    if (vi == 1u || vi == 3u || vi == 4u) { corner.x = 1.0; }
    if (vi == 2u || vi == 4u || vi == 5u) { corner.y = 1.0; }
    let px = in.rect.x + corner.x * in.rect.z;
    let py = in.rect.y + corner.y * in.rect.w;
    let size = vec2<f32>(max(u.extras.z, 1.0), max(u.extras.y, 1.0));
    var out: PlateOut;
    out.clip = vec4<f32>(px / size.x * 2.0 - 1.0, 1.0 - py / size.y * 2.0, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_plate(in: PlateOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 4],
    color: [f32; 4],
    params: [f32; 4],
}

impl InstanceRaw {
    fn new(model: Mat4, color: [f32; 4], params: [f32; 4]) -> Self {
        let normal = model.inverse().transpose();
        Self {
            model: model.to_cols_array_2d(),
            normal: normal.to_cols_array_2d(),
            color,
            params,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    extras: [f32; 4],
}

struct MeshRange {
    first: u32,
    count: u32,
    base: i32,
}

struct DrawnText {
    key: (String, u32),
    left: f32,
    top: f32,
    color: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PlateRaw {
    rect: [f32; 4],
    color: [f32; 4],
}

pub struct Gpu {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    plate_buffer: wgpu::Buffer,
    plate_capacity: usize,
    ground: MeshRange,
    disc: MeshRange,
    ring: MeshRange,
    shapes: [MeshRange; Shape::COUNT],
    sky_pipeline: wgpu::RenderPipeline,
    blend_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    plate_pipeline: wgpu::RenderPipeline,
    depth: wgpu::TextureView,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    labels: HashMap<(String, u32), Buffer>,
    text_warned: bool,
    floor: Floor,
    pub(crate) window: Arc<Window>,
}

impl Gpu {
    pub async fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        floor: Floor,
    ) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|err| {
                format!("could not open a Wayland surface, so the window has nothing to draw on. ({err})")
            })?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|err| format!("no GPU that can draw a Wayland window was found. ({err})"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|err| format!("a GPU was found, but it could not be opened. ({err})"))?;

        let caps = surface.get_capabilities(&adapter);
        if caps.formats.is_empty() {
            return Err("the window surface has no color format, so nothing can be drawn".into());
        }
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(caps.formats[0]);
        let alpha = caps
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque)
            .or_else(|| caps.alpha_modes.first().copied())
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let present_mode = caps
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::Fifo)
            .unwrap_or(wgpu::PresentMode::Fifo);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: alpha,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let (vertices, indices, ground, disc, ring, shapes) = build_geometry();
        let vertex_bytes = bytemuck::cast_slice::<Vertex, u8>(&vertices);
        let index_bytes = bytemuck::cast_slice::<u16, u8>(&indices);
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, vertex_bytes);
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("indices"),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, index_bytes);

        let instance_capacity = 256;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (instance_capacity * std::mem::size_of::<InstanceRaw>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let plate_capacity = 64;
        let plate_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("plates"),
            size: (plate_capacity * std::mem::size_of::<PlateRaw>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let shader_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nav"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nav"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let no_buffers: &[Option<wgpu::VertexBufferLayout>] = &[];
        let sky_pipeline = make_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_sky",
            "fs_sky",
            format,
            false,
            false,
            no_buffers,
        );
        let mesh_buffers = [Some(vertex_layout()), Some(instance_layout())];
        let blend_pipeline = make_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_mesh",
            "fs_blend",
            format,
            true,
            false,
            &mesh_buffers,
        );
        let mesh_pipeline = make_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_mesh",
            "fs_mesh",
            format,
            false,
            true,
            &mesh_buffers,
        );
        let plate_pipeline = make_plate_pipeline(&device, &pipeline_layout, &shader, format);
        if let Some(err) = shader_scope.pop().await {
            return Err(format!(
                "the GPU rejected a shader or pipeline, so the window would be blank. ({err})"
            ));
        }

        let font_system = FontSystem::new();
        require_label_font(font_system.db())?;
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let depth = depth_view(&device, config.width, config.height);

        Ok(Self {
            instance,
            device,
            queue,
            surface,
            config,
            uniform_buffer,
            bind_group,
            vertex_buffer,
            index_buffer,
            instance_buffer,
            instance_capacity,
            plate_buffer,
            plate_capacity,
            ground,
            disc,
            ring,
            shapes,
            sky_pipeline,
            blend_pipeline,
            mesh_pipeline,
            plate_pipeline,
            depth,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            labels: HashMap::new(),
            text_warned: false,
            floor,
            window,
        })
    }

    pub fn size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = depth_view(&self.device, width, height);
    }

    pub fn render(&mut self, frame: &Frame) -> Result<(), String> {
        if self.config.width == 0 || self.config.height == 0 {
            return Ok(());
        }
        let built = build_instances(frame, self.floor);
        self.ensure_instances(built.instances.len());
        self.queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&built.instances));
        let plates = self.collect_plates(frame);
        if !plates.is_empty() {
            self.ensure_plates(plates.len());
            self.queue
                .write_buffer(&self.plate_buffer, 0, bytemuck::cast_slice(&plates));
        }
        let uniforms = Uniforms {
            view_proj: frame.view_proj.to_cols_array_2d(),
            camera_pos: [
                frame.camera_pos.x,
                frame.camera_pos.y,
                frame.camera_pos.z,
                1.0,
            ],
            extras: [
                frame.time,
                self.config.height as f32,
                self.config.width as f32,
                0.0,
            ],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let drawn = self.collect_text(frame);
        self.prepare_text(&drawn);

        let surface_frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .map_err(|err| format!("could not reopen the surface: {err}"))?;
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface validation failed".into());
            }
        };
        let view = surface_frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nav"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("plaza"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.025,
                            b: 0.04,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let shadow_count = built.shadow_count;
            let ring_at = 1 + shadow_count;
            let mut shape_at = ring_at + 1;
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.blend_pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(self.ground.first..self.ground.first + self.ground.count, self.ground.base, 0..1);
            if shadow_count > 0 {
                pass.draw_indexed(
                    self.disc.first..self.disc.first + self.disc.count,
                    self.disc.base,
                    1..1 + shadow_count,
                );
            }
            pass.set_pipeline(&self.mesh_pipeline);
            pass.draw_indexed(self.ring.first..self.ring.first + self.ring.count, self.ring.base, ring_at..ring_at + 1);
            for (index, mesh) in self.shapes.iter().enumerate() {
                let count = built.shape_counts[index];
                if count == 0 {
                    continue;
                }
                pass.draw_indexed(mesh.first..mesh.first + mesh.count, mesh.base, shape_at..shape_at + count);
                shape_at += count;
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !plates.is_empty() {
                pass.set_pipeline(&self.plate_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.plate_buffer.slice(..));
                pass.draw(0..6, 0..plates.len() as u32);
            }
            if let Err(err) = self.text_renderer.render(&self.atlas, &self.viewport, &mut pass) {
                if !self.text_warned {
                    self.text_warned = true;
                    eprintln!("nav: text: {err}");
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(surface_frame);
        self.atlas.trim();
        Ok(())
    }

    fn ensure_plates(&mut self, count: usize) {
        if count <= self.plate_capacity {
            return;
        }
        let capacity = count.max(self.plate_capacity * 2).max(32);
        self.plate_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("plates"),
            size: (capacity * std::mem::size_of::<PlateRaw>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.plate_capacity = capacity;
    }

    fn collect_plates(&mut self, frame: &Frame) -> Vec<PlateRaw> {
        let mut plates = Vec::new();
        for label in &frame.labels {
            if label.text.is_empty() {
                continue;
            }
            self.ensure_label(&label.text, label.size);
            let key = (label.text.clone(), label.size.to_bits());
            let measured = self.labels.get(&key).map(buffer_width).unwrap_or(0.0);
            if measured <= 0.0 {
                continue;
            }
            let rect = label_plate(label.center_x, label.top, label.size, measured);
            plates.push(PlateRaw {
                rect,
                color: [0.02, 0.028, 0.04, 0.92],
            });
        }
        for tile in &frame.tiles {
            plates.extend(tile_plates(tile));
        }
        plates
    }

    fn ensure_instances(&mut self, count: usize) {
        if count <= self.instance_capacity {
            return;
        }
        let capacity = count.max(self.instance_capacity * 2).max(64);
        self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * std::mem::size_of::<InstanceRaw>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = capacity;
    }

    fn collect_text(&mut self, frame: &Frame) -> Vec<DrawnText> {
        let mut drawn = Vec::new();
        self.push_text(&mut drawn, &frame.title, 28.0, 22.0, 26.0, [244, 247, 246], false);
        self.push_text(&mut drawn, &frame.meta, 28.0, 56.0, 16.0, [176, 196, 198], false);
        let help_top = (self.config.height as f32 - 40.0).max(80.0);
        self.push_text(&mut drawn, &frame.help, 28.0, help_top, 15.0, [150, 168, 172], false);
        for label in &frame.labels {
            self.push_text(
                &mut drawn,
                &label.text,
                label.center_x,
                label.top,
                label.size,
                label.color,
                true,
            );
        }
        for tile in &frame.tiles {
            self.push_text(
                &mut drawn,
                "←",
                tile.arrow[0] + tile.arrow[2] * 0.5,
                tile.arrow[1] + 4.0,
                16.0,
                [232, 236, 234],
                true,
            );
            self.push_text(
                &mut drawn,
                &tile.name,
                tile.arrow[0] + tile.arrow[2] + 8.0,
                tile.rect[1] + 12.0,
                15.0,
                [244, 247, 246],
                false,
            );
            self.push_text(
                &mut drawn,
                "×",
                tile.close[0] + tile.close[2] * 0.5,
                tile.close[1] + 3.0,
                16.0,
                [236, 224, 214],
                true,
            );
            for (index, line) in tile.lines.iter().enumerate() {
                self.push_text(
                    &mut drawn,
                    line,
                    tile.rect[0] + 12.0,
                    tile.rect[1] + 40.0 + index as f32 * 16.0,
                    13.0,
                    [168, 188, 190],
                    false,
                );
            }
            if tile.tag[2] > 0.0 {
                self.push_text(
                    &mut drawn,
                    "Finished",
                    tile.tag[0] + tile.tag[2] * 0.5,
                    tile.tag[1] + 4.0,
                    13.0,
                    [214, 230, 222],
                    true,
                );
            }
        }
        let used: HashSet<_> = drawn.iter().map(|item| item.key.clone()).collect();
        self.labels.retain(|key, _| used.contains(key));
        drawn
    }

    fn push_text(
        &mut self,
        drawn: &mut Vec<DrawnText>,
        text: &str,
        left: f32,
        top: f32,
        size: f32,
        color: [u8; 3],
        center: bool,
    ) {
        if text.is_empty() {
            return;
        }
        self.ensure_label(text, size);
        let key = (text.to_string(), size.to_bits());
        let measured = self.labels.get(&key).map(buffer_width).unwrap_or(0.0);
        let left = if center { left - measured * 0.5 } else { left };
        drawn.push(DrawnText { key, left, top, color });
    }

    fn ensure_label(&mut self, text: &str, size: f32) {
        let key = (text.to_string(), size.to_bits());
        if self.labels.contains_key(&key) {
            return;
        }
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(size, size * 1.25));
        buffer.set_size(Some(4096.0), Some(size * 2.0));
        buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.labels.insert(key, buffer);
    }

    fn prepare_text(&mut self, drawn: &[DrawnText]) {
        let mut areas = Vec::with_capacity(drawn.len());
        for item in drawn {
            let Some(buffer) = self.labels.get(&item.key) else {
                continue;
            };
            areas.push(TextArea {
                buffer,
                left: item.left,
                top: item.top,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: self.config.width as i32,
                    bottom: self.config.height as i32,
                },
                default_color: Color::rgb(item.color[0], item.color[1], item.color[2]),
                custom_glyphs: &[],
            });
        }
        if let Err(err) = self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        ) {
            if !self.text_warned {
                self.text_warned = true;
                eprintln!("nav: text: {err}");
            }
        }
    }
}

struct BuiltInstances {
    instances: Vec<InstanceRaw>,
    shape_counts: [u32; Shape::COUNT],
    shadow_count: u32,
}

fn build_instances(frame: &Frame, floor: Floor) -> BuiltInstances {
    let mut instances = Vec::with_capacity(frame.nodes.len() * 2 + 2);
    instances.push(InstanceRaw::new(
        Mat4::from_scale_rotation_translation(
            Vec3::new(frame.ground_radius, 1.0, frame.ground_radius),
            Quat::IDENTITY,
            Vec3::new(0.0, -0.05, 0.0),
        ),
        [0.45, 0.72, 0.74, 1.0],
        [frame.ground_radius, floor.code(), 0.0, 0.0],
    ));
    let mut shadow_count = 0u32;
    for node in &frame.nodes {
        if node.foot <= 0.0 {
            continue;
        }
        shadow_count += 1;
        let pos = node.model.w_axis.truncate();
        let radius = (node.foot * 2.35).max(0.08);
        instances.push(InstanceRaw::new(
            Mat4::from_scale_rotation_translation(
                Vec3::new(radius, 1.0, radius),
                Quat::IDENTITY,
                Vec3::new(pos.x, 0.012, pos.z),
            ),
            [0.0, 0.0, 0.0, 1.0],
            [radius, pos.x, pos.z, 1.0],
        ));
    }
    let pulse = HERE_RING_SCALE + (frame.time.sin() * 0.035);
    instances.push(InstanceRaw::new(
        Mat4::from_scale_rotation_translation(
            Vec3::new(pulse, HERE_RING_HEIGHT, pulse),
            Quat::IDENTITY,
            Vec3::new(0.0, HERE_RING_Y, 0.0),
        ),
        [0.42, 0.82, 0.76, 1.0],
        [frame.ring_emissive, 0.0, 0.0, 0.0],
    ));
    let mut grouped: [Vec<InstanceRaw>; Shape::COUNT] = std::array::from_fn(|_| Vec::new());
    for node in &frame.nodes {
        let params = if node.arc[1] > 0.0 {
            [node.emissive, node.arc[0], node.arc[1], 2.0]
        } else {
            [node.emissive, 0.0, 0.0, 0.0]
        };
        grouped[node.shape.index()].push(InstanceRaw::new(node.model, node.color, params));
    }
    let mut shape_counts = [0u32; Shape::COUNT];
    for (index, group) in grouped.iter().enumerate() {
        shape_counts[index] = group.len() as u32;
        instances.extend(group);
    }
    BuiltInstances {
        instances,
        shape_counts,
        shadow_count,
    }
}

fn build_geometry() -> (
    Vec<Vertex>,
    Vec<u16>,
    MeshRange,
    MeshRange,
    MeshRange,
    [MeshRange; Shape::COUNT],
) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let ground = append(&mut vertices, &mut indices, geom::ground());
    let disc = append(&mut vertices, &mut indices, geom::disc(24));
    let ring = append(&mut vertices, &mut indices, geom::ring(48));
    let shapes = Shape::ALL.map(|shape| append(&mut vertices, &mut indices, shape_mesh(shape)));
    (vertices, indices, ground, disc, ring, shapes)
}

fn shape_mesh(shape: Shape) -> geom::Mesh {
    match shape {
        Shape::Prism => geom::prism(),
        Shape::Cylinder => geom::cylinder(),
        Shape::Box => geom::block(),
        Shape::Cone => geom::cone(),
        Shape::Pyramid => geom::pyramid(),
        Shape::Crystal => geom::crystal(),
        Shape::Obelisk => geom::obelisk(),
        Shape::Blob => geom::ring_segment(24),
    }
}

fn append(vertices: &mut Vec<Vertex>, indices: &mut Vec<u16>, mesh: geom::Mesh) -> MeshRange {
    let base = vertices.len() as i32;
    let first = indices.len() as u32;
    let count = mesh.indices.len() as u32;
    vertices.extend(mesh.vertices);
    indices.extend(mesh.indices);
    MeshRange { first, count, base }
}

fn depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRS,
    }
}

fn instance_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<InstanceRaw>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &INSTANCE_ATTRS,
    }
}

const VERTEX_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
];

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
    2 => Float32x4,
    3 => Float32x4,
    4 => Float32x4,
    5 => Float32x4,
    6 => Float32x4,
    7 => Float32x4,
    8 => Float32x4,
    9 => Float32x4,
    10 => Float32x4,
    11 => Float32x4,
];

/// Folder names are drawn from the system font list. An empty list leaves the plaza unlabeled.
fn require_label_font(db: &glyphon::fontdb::Database) -> Result<(), String> {
    if !db.is_empty() {
        return Ok(());
    }
    Err(
        "no font was found, so folder names cannot be drawn. Install a sans-serif such as DejaVu Sans or Noto Sans where fontconfig can see it."
            .into(),
    )
}

fn make_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    format: wgpu::TextureFormat,
    blend: bool,
    depth_write: bool,
    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fs),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(if depth_write {
                wgpu::CompareFunction::Less
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn tile_plates(tile: &TileView) -> Vec<PlateRaw> {
    let button = if_hot;
    let mut plates = vec![
        PlateRaw {
            rect: tile.rect,
            color: [0.025, 0.034, 0.05, 0.94],
        },
        PlateRaw {
            rect: [tile.rect[0], tile.rect[1], 3.0, tile.rect[3]],
            color: if tile.running {
                [0.95, 0.72, 0.28, 1.0]
            } else {
                [0.38, 0.46, 0.48, 1.0]
            },
        },
        PlateRaw {
            rect: tile.arrow,
            color: button(tile.arrow_hot),
        },
        PlateRaw {
            rect: tile.close,
            color: button(tile.close_hot),
        },
    ];
    if tile.tag[2] > 0.0 {
        plates.push(PlateRaw {
            rect: tile.tag,
            color: [0.16, 0.24, 0.22, 1.0],
        });
    }
    plates
}

fn if_hot(hot: bool) -> [f32; 4] {
    if hot {
        [0.24, 0.28, 0.32, 1.0]
    } else {
        [0.07, 0.09, 0.12, 1.0]
    }
}

fn buffer_width(buffer: &Buffer) -> f32 {
    buffer.layout_runs().map(|run| run.line_w).fold(0.0, f32::max)
}

/// A dark rectangle behind a directory name. `top` is the top of the text line.
fn label_plate(center_x: f32, top: f32, size: f32, text_width: f32) -> [f32; 4] {
    let pad_x = (size * 0.32).max(4.0);
    let pad_y = (size * 0.12).max(2.0);
    let width = text_width + pad_x * 2.0;
    let y = top + size * 0.16 - pad_y;
    let height = size * 1.16 + pad_y * 2.0;
    [center_x - width * 0.5, y, width, height]
}

const PLATE_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    0 => Float32x4,
    1 => Float32x4,
];

fn make_plate_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<PlateRaw>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &PLATE_ATTRS,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("plate"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_plate"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_plate"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod plate_tests {
    use super::label_plate;

    #[test]
    fn the_backing_covers_the_directory_name() {
        let rect = label_plate(200.0, 80.0, 22.0, 90.0);
        assert!(rect[0] < 200.0 - 45.0);
        assert!(rect[0] + rect[2] > 200.0 + 45.0);
        assert!(rect[1] < 80.0 + 4.0);
        assert!(rect[1] + rect[3] > 80.0 + 22.0);
        assert!(rect[2] < 90.0 + 40.0);
    }
}

#[cfg(test)]
mod startup_tests {
    use super::require_label_font;

    #[test]
    fn an_empty_font_list_stops_startup() {
        let empty = glyphon::fontdb::Database::new();
        let err = require_label_font(&empty).unwrap_err();
        assert!(err.contains("no font was found"));
        assert!(err.contains("DejaVu Sans"));
        assert!(err.contains("Noto Sans"));
        assert!(!err.starts_with("nav:"));
    }
}
